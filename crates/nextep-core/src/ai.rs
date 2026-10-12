//! Optional "AI picks": an OpenAI-compatible chat API (DeepSeek by default)
//! chooses shows for a mood the user describes, given a summary of the user's
//! history and the people behind shows they liked (see `ai_context`).
//!
//! The model only ranks and explains real candidates that Nextep found on
//! Trakt; any id it returns that isn't a candidate is dropped, so it can't
//! recommend shows that don't exist. The user brings their own API key, which
//! stays on the device. Requests include recent shows, a taste profile,
//! dropped shows and favourite cast and crew.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// How hard thinking models think: DeepSeek takes `none`, `low`, `high`, `max`.
/// Low by default: every tool round thinks again, so higher effort multiplies
/// the wait. Users can raise it in Settings.
pub const DEFAULT_REASONING_EFFORT: &str = "low";
const MAX_PICKS: usize = 10;
/// Characters of each candidate's overview sent to the model.
const OVERVIEW_CHARS: usize = 100;

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

/// Asks the model for up to ten picks for `mood` in one request. `taste` is
/// what the model knows about the user (see
/// [`TasteContext`](crate::ai_context::TasteContext)).
pub async fn pick(
    config: &AiConfig,
    mood: &str,
    candidates: &[Candidate],
    taste: &Value,
) -> Result<Vec<Pick>> {
    let (system, user) = prompt(mood, candidates, taste);
    // No max_tokens: thinking models count their reasoning towards it, and a
    // small cap leaves nothing for the answer. No JSON mode either: not every
    // provider allows it, so the prompt asks for JSON and the reply is parsed
    // leniently.
    let mut body = json!({
        "model": config.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
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
    Ok(parse_picks(
        reply["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default(),
        candidates,
    ))
}

fn prompt(mood: &str, candidates: &[Candidate], taste: &Value) -> (String, String) {
    let system = format!(
        "You recommend TV shows. You are given the user's taste as JSON: a taste profile \
         aggregated over their whole history, the shows they watched most recently (with how far \
         they got and whether they liked them), shows they dropped or didn't like, and people \
         (actors, creators, directors, writers) who recur in shows they liked. Use it to \
         understand their taste, then choose only from the numbered candidates you are given; \
         never suggest anything else. Pick up to {MAX_PICKS} that best fit the user's mood and \
         taste, best first. Reply with JSON only: {{\"picks\": [{{\"id\": <candidate id>, \"reason\": \"<one \
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
                .take(OVERVIEW_CHARS)
                .collect();
            format!("{} | {}{year}{genres} | {overview}", c.id, c.title)
        })
        .collect();
    let user = format!(
        "My mood: {mood}\n\nMy taste:\n{taste}\n\nCandidates (id | title | about):\n{}",
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

/// One client for every request, so later asks reuse the open connection
/// instead of paying for a new TLS handshake.
fn client() -> Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        // High or max thinking can take a while.
        .timeout(Duration::from_secs(300))
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
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

    #[tokio::test]
    async fn pick_is_one_request_without_tools() {
        let answers = r#"{"choices":[{"message":{"role":"assistant","reasoning_content":"Spy drama fits.","content":"Here you go: {\"picks\":[{\"id\":33,\"reason\":\"Tense and witty.\"}]}"}}]}"#;
        let (url, server) = fake_api(vec![answers]);
        let config = AiConfig {
            base_url: format!("{url}/"),
            api_key: "test-key".into(),
            model: "deepseek-flash".into(),
            reasoning_effort: Some("max".into()),
        };
        let taste = json!({ "taste_profile": { "shows_watched": 42 } });
        let picks = pick(&config, "spies", &candidates(), &taste).await.unwrap();
        assert_eq!(
            picks,
            [Pick {
                id: 33,
                reason: "Tense and witty.".into()
            }]
        );

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.starts_with("POST /chat/completions HTTP/1.1"));
        assert!(
            request
                .to_lowercase()
                .contains("authorization: bearer test-key")
        );
        assert!(request.contains(r#""model":"deepseek-flash""#));
        assert!(request.contains(r#""reasoning_effort":"max""#));
        assert!(request.contains("shows_watched"));
        assert!(!request.contains(r#""tools""#));
        assert!(!request.contains("max_tokens"));
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
    fn prompt_has_taste_and_candidates() {
        let taste = json!({ "recent_shows": [{ "title": "Slow Horses" }] });
        let (system, user) = prompt("something cosy", &candidates(), &taste);
        assert!(system.contains("only from the numbered candidates"));
        assert!(user.contains("My mood: something cosy"));
        assert!(user.contains(r#"{"recent_shows":[{"title":"Slow Horses"}]}"#));
        assert!(user.contains("22 | Ted Lasso (2022) [drama] | About things."));
    }

    #[test]
    fn prompt_shortens_overviews() {
        let mut long = candidates();
        long[0].overview = Some("x".repeat(500));
        let (_, user) = prompt("anything", &long, &json!({}));
        assert!(user.contains(&format!("| {}\n", "x".repeat(OVERVIEW_CHARS))));
        assert!(!user.contains(&"x".repeat(OVERVIEW_CHARS + 1)));
    }
}
