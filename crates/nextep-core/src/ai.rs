//! Optional "AI picks": an OpenAI-compatible chat API (DeepSeek by default)
//! chooses shows for a mood the user describes, calling tools to look at the
//! user's history and the people behind shows (see `ai_tools`).
//!
//! The model only ranks and explains real candidates that Nextep found on
//! Trakt; any id it returns that isn't a candidate is dropped, so it can't
//! recommend shows that don't exist. The user brings their own API key, which
//! stays on the device. Requests include whatever the model asks the tools
//! for: recent shows, a taste profile, dropped shows and cast and crew.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::ai_tools::ToolBox;
use crate::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// How hard thinking models think: DeepSeek takes `none`, `low`, `high`, `max`.
pub const DEFAULT_REASONING_EFFORT: &str = "max";
const MAX_PICKS: usize = 10;
/// Tool-call rounds before giving up.
pub const MAX_ROUNDS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    /// Sent as `reasoning_effort` when set; `None` leaves the provider default.
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

/// A show the model may pick from.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: u64,
    pub title: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pick {
    pub id: u64,
    pub reason: String,
}

/// Model ids the API offers, which also checks that the key works.
pub async fn list_models(config: &AiConfig) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Models {
        data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        id: String,
    }
    let res = client()?
        .get(format!("{}/models", base(config)))
        .bearer_auth(&config.api_key)
        .send()
        .await?;
    let models: Models = check(res).await?.json().await?;
    Ok(models.data.into_iter().map(|m| m.id).collect())
}

/// Asks the model for up to ten picks for `mood`. The model can call the
/// [`ToolBox`] tools (recent shows, taste profile, people, ...) as often as it
/// likes within [`MAX_ROUNDS`] before answering.
pub async fn pick(
    config: &AiConfig,
    mood: &str,
    candidates: &[Candidate],
    tools: &mut ToolBox,
) -> Result<Vec<Pick>> {
    let (system, user) = prompt(mood, candidates);
    let mut messages = vec![
        json!({ "role": "system", "content": system }),
        json!({ "role": "user", "content": user }),
    ];
    for _ in 0..MAX_ROUNDS {
        // No max_tokens: thinking models count their reasoning towards it, and
        // a small cap leaves nothing for the answer. No JSON mode either: not
        // every provider allows it together with tools, so the prompt asks
        // for JSON and the reply is parsed leniently.
        let mut body = json!({
            "model": config.model,
            "messages": messages,
            "tools": ToolBox::specs(),
        });
        if let Some(effort) = config.reasoning_effort.as_deref().filter(|e| !e.is_empty()) {
            body["reasoning_effort"] = json!(effort);
        }
        let res = client()?
            .post(format!("{}/chat/completions", base(config)))
            .bearer_auth(&config.api_key)
            .json(&body)
            .send()
            .await?;
        let reply: Value = check(res).await?.json().await?;
        let message = reply["choices"][0]["message"].clone();
        let calls = message["tool_calls"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        // Sent back exactly as received: DeepSeek requires the
        // reasoning_content of earlier steps when tools are in use.
        messages.push(message.clone());
        if calls.is_empty() {
            return Ok(parse_picks(
                message["content"].as_str().unwrap_or_default(),
                candidates,
            ));
        }
        for call in calls {
            let name = call["function"]["name"].as_str().unwrap_or_default();
            let args: Value = call["function"]["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_else(|| json!({}));
            let result = tools.call(name, &args).await;
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call["id"],
                "content": result.to_string(),
            }));
        }
    }
    Err(Error::Api {
        status: 0,
        body: "The AI kept looking things up without answering. Try again.".into(),
    })
}

fn prompt(mood: &str, candidates: &[Candidate]) -> (String, String) {
    let system = format!(
        "You recommend TV shows. First use the tools to understand the user's taste: what they \
         watched recently, their taste profile, what they dropped, and the people (actors, \
         creators, directors, writers) behind shows they love or that you are considering. Then \
         choose only from the numbered candidates you are given; never suggest anything else. \
         Pick up to {MAX_PICKS} that best fit the user's mood and taste, best first. Your final \
         reply must be JSON only: {{\"picks\": [{{\"id\": <candidate id>, \"reason\": \"<one \
         short sentence, addressed to the user, that can mention what in their history it \
         matches>\"}}]}}"
    );
    let candidate_lines: Vec<String> = candidates
        .iter()
        .map(|c| {
            let year = c.year.map(|y| format!(" ({y})")).unwrap_or_default();
            let genres = if c.genres.is_empty() {
                String::new()
            } else {
                format!(" [{}]", c.genres.join(", "))
            };
            let overview: String = c
                .overview
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(180)
                .collect();
            format!("{} | {}{year}{genres} | {overview}", c.id, c.title)
        })
        .collect();
    let user = format!(
        "My mood: {mood}\n\nCandidates (id | title | about):\n{}",
        candidate_lines.join("\n")
    );
    (system, user)
}

/// Reads the model's JSON, keeping only real candidates, each once.
fn parse_picks(content: &str, candidates: &[Candidate]) -> Vec<Pick> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        picks: Vec<RawPick>,
    }
    #[derive(Deserialize)]
    struct RawPick {
        id: serde_json::Value,
        #[serde(default)]
        reason: String,
    }

    // Models sometimes wrap the JSON in a ```json fence or a sentence; take
    // the outermost object.
    let json = match (content.find('{'), content.rfind('}')) {
        (Some(start), Some(end)) if start < end => &content[start..=end],
        _ => return Vec::new(),
    };
    let Ok(reply) = serde_json::from_str::<Reply>(json) else {
        return Vec::new();
    };
    let known: HashSet<u64> = candidates.iter().map(|c| c.id).collect();
    let mut seen = HashSet::new();
    reply
        .picks
        .into_iter()
        .filter_map(|p| {
            // Accept the id as a number or a numeric string.
            let id =
                p.id.as_u64()
                    .or_else(|| p.id.as_str().and_then(|s| s.trim().parse().ok()))?;
            (known.contains(&id) && seen.insert(id)).then(|| Pick {
                id,
                reason: p.reason.trim().to_owned(),
            })
        })
        .take(MAX_PICKS)
        .collect()
}

fn base(config: &AiConfig) -> &str {
    config.base_url.trim_end_matches('/')
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        // Maximum thinking over a hundred candidates can take a while.
        .timeout(Duration::from_secs(300))
        .build()?)
}

async fn check(res: reqwest::Response) -> Result<reqwest::Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    let message = match status.as_u16() {
        401 => "The AI provider rejected the API key.".to_string(),
        402 => "The AI account is out of credit.".to_string(),
        429 => "The AI provider is rate limiting; try again shortly.".to_string(),
        _ => body,
    };
    Err(Error::Api {
        status: status.as_u16(),
        body: message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates() -> Vec<Candidate> {
        [(11, "Severance"), (22, "Ted Lasso"), (33, "Slow Horses")]
            .into_iter()
            .map(|(id, title)| Candidate {
                id,
                title: title.into(),
                year: Some(2022),
                genres: vec!["drama".into()],
                overview: Some("About things.".into()),
            })
            .collect()
    }

    #[test]
    fn keeps_only_real_candidates_once() {
        let reply = r#"{"picks": [
            {"id": 22, "reason": "Warm and funny."},
            {"id": 999, "reason": "Made up."},
            {"id": "33", "reason": "Spy drama."},
            {"id": 22, "reason": "Again."}
        ]}"#;
        let picks = parse_picks(reply, &candidates());
        assert_eq!(
            picks,
            [
                Pick {
                    id: 22,
                    reason: "Warm and funny.".into()
                },
                Pick {
                    id: 33,
                    reason: "Spy drama.".into()
                },
            ]
        );
    }

    #[test]
    fn tolerates_code_fences_and_garbage() {
        let fenced = "```json\n{\"picks\": [{\"id\": 11, \"reason\": \"Eerie.\"}]}\n```";
        assert_eq!(parse_picks(fenced, &candidates()).len(), 1);
        assert!(parse_picks("Sorry, I can't help.", &candidates()).is_empty());
    }

    /// Local HTTP server that answers each request with the next body and
    /// returns the requests it received.
    fn fake_api(bodies: Vec<&'static str>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for body in bodies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 65536];
                // Read headers, then as much body as Content-Length says.
                loop {
                    let n = stream.read(&mut buf).unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).unwrap();
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        });
        (url, handle)
    }

    fn toolbox() -> ToolBox {
        ToolBox::new(
            &[],
            &crate::labels::Labels::default(),
            crate::TraktClient::new("id"),
            chrono::Utc::now(),
        )
    }

    #[tokio::test]
    async fn pick_runs_tools_then_parses_the_answer() {
        let asks_for_tool = r#"{"choices":[{"message":{"role":"assistant","content":"","reasoning_content":"Let me check their taste.","tool_calls":[{"id":"call_1","type":"function","function":{"name":"taste_profile","arguments":"{}"}}]}}]}"#;
        let answers = r#"{"choices":[{"message":{"role":"assistant","reasoning_content":"Spy drama fits.","content":"Here you go: {\"picks\":[{\"id\":33,\"reason\":\"Tense and witty.\"}]}"}}]}"#;
        let (url, server) = fake_api(vec![asks_for_tool, answers]);
        let config = AiConfig {
            base_url: format!("{url}/"),
            api_key: "test-key".into(),
            model: "deepseek-flash".into(),
            reasoning_effort: Some("max".into()),
        };
        let picks = pick(&config, "spies", &candidates(), &mut toolbox())
            .await
            .unwrap();
        assert_eq!(
            picks,
            [Pick {
                id: 33,
                reason: "Tense and witty.".into()
            }]
        );

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        let first = &requests[0];
        assert!(first.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(
            first
                .to_lowercase()
                .contains("authorization: bearer test-key")
        );
        assert!(first.contains(r#""model":"deepseek-flash""#));
        assert!(first.contains(r#""reasoning_effort":"max""#));
        assert!(first.contains(r#""name":"favourite_people""#));
        assert!(!first.contains("max_tokens"));
        // The second request carries the reasoning back and the tool's answer.
        let second = &requests[1];
        assert!(second.contains(r#""reasoning_content":"Let me check their taste.""#));
        assert!(second.contains(r#""role":"tool""#));
        assert!(second.contains(r#""tool_call_id":"call_1""#));
        assert!(second.contains("shows_watched"));
    }

    #[tokio::test]
    async fn list_models_reads_ids() {
        let (url, server) = fake_api(vec![
            r#"{"object":"list","data":[{"id":"deepseek-flash"},{"id":"deepseek-v4-pro"}]}"#,
        ]);
        let config = AiConfig {
            base_url: url,
            api_key: "k".into(),
            model: String::new(),
            reasoning_effort: None,
        };
        assert_eq!(
            list_models(&config).await.unwrap(),
            ["deepseek-flash", "deepseek-v4-pro"]
        );
        assert!(server.join().unwrap()[0].starts_with("GET /models HTTP/1.1"));
    }

    #[test]
    fn prompt_lists_candidates_and_asks_for_tools() {
        let (system, user) = prompt("something cosy", &candidates());
        assert!(system.contains("only from the numbered candidates"));
        assert!(system.contains("use the tools"));
        assert!(user.contains("My mood: something cosy"));
        assert!(user.contains("22 | Ted Lasso (2022) [drama] | About things."));
    }
}
