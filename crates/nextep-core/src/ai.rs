//! Optional "AI picks": an OpenAI-compatible chat API (DeepSeek by default)
//! chooses shows or movies for a mood the user describes, given a summary of
//! the user's history and the people behind shows they liked (see
//! `ai_context`).
//!
//! The model picks from candidates Nextep found on Trakt or names other
//! titles it knows; the caller looks those up on Trakt and drops any it can't
//! find, so nothing that doesn't exist is shown. The user brings their own
//! API key, which stays on the device. Requests include recent shows, a taste
//! profile, dropped shows, favourite cast and crew and titles already seen.

use std::collections::HashSet;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Error, Result};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
/// How hard thinking models think: DeepSeek takes `none`, `low`, `high`, `max`.
/// Off by default: measured on a phone, deepseek-flash answered in 3–4 s with
/// thinking off, 6–30 s on low and 45–80 s on max, with picks of similar
/// quality. Users can turn it up in Settings.
pub const DEFAULT_REASONING_EFFORT: &str = "none";
/// Picks the model chooses itself, from everything it knows.
const MAX_OWN: usize = 8;
/// Picks it may take from the candidates. They come in a separate list with
/// a hard cap: otherwise the model leans on them even when the mood calls for
/// something else entirely.
const MAX_LISTED: usize = 2;
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

/// What the user wants picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Shows,
    Movies,
}

impl Kind {
    fn noun(self) -> &'static str {
        match self {
            Kind::Shows => "TV shows",
            Kind::Movies => "movies",
        }
    }
}

/// A show or movie the model may pick from.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: u64,
    pub title: String,
    pub year: Option<i32>,
    pub genres: Vec<String>,
    pub overview: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub choice: Choice,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// One of the candidates, by id.
    Candidate(u64),
    /// A title the model knows that wasn't a candidate. It may not exist:
    /// look it up before showing it.
    Title { title: String, year: Option<i32> },
}

/// Whether a title found on Trakt is the one the model named: the same title
/// (ignoring case, punctuation, "&" vs "and" and a leading "The") from the
/// same year give or take two, or the same title before a subtitle ("Borat"
/// for "Borat: Cultural Learnings…") from exactly the same year.
pub fn same_title(
    found: &str,
    found_year: Option<i32>,
    named: &str,
    named_year: Option<i32>,
) -> bool {
    fn norm(title: &str) -> String {
        let lower = title.to_lowercase().replace('&', " and ");
        let words: Vec<String> = lower
            .split_whitespace()
            .map(|w| {
                w.chars()
                    .filter(|c| c.is_alphanumeric())
                    .collect::<String>()
            })
            .filter(|w| !w.is_empty())
            .collect();
        let words = match words.first().map(String::as_str) {
            Some("the") if words.len() > 1 => &words[1..],
            _ => &words[..],
        };
        words.concat()
    }
    fn main_title(title: &str) -> &str {
        title.split([':', '–', '—']).next().unwrap_or(title)
    }
    let full = norm(found);
    if full.is_empty() {
        return false;
    }
    // Festival premieres and wide releases can be a couple of years apart.
    let close_year = match (found_year, named_year) {
        (Some(a), Some(b)) => (a - b).abs() <= 2,
        _ => true,
    };
    let same_year = matches!((found_year, named_year), (Some(a), Some(b)) if a == b);
    (full == norm(named) && close_year)
        || (norm(main_title(found)) == norm(main_title(named)) && same_year)
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
    kind: Kind,
    mood: &str,
    candidates: &[Candidate],
    taste: &Value,
) -> Result<Vec<Pick>> {
    let (system, user) = prompt(kind, mood, candidates, taste);
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

fn prompt(kind: Kind, mood: &str, candidates: &[Candidate], taste: &Value) -> (String, String) {
    let noun = kind.noun();
    let system = format!(
        "You recommend {noun}. You are given the user's taste as JSON: a taste profile \
         aggregated over their TV history, the shows they watched most recently (with how far \
         they got and whether they liked them), shows they dropped or didn't like, people \
         (actors, creators, directors, writers) who recur in shows they liked, and, when \
         present, the movies they watched most recently. Taste carries over between TV and \
         film. `already_seen` lists {noun} they have watched or dropped: never recommend \
         those.\n\n\
         Recommend {noun} for the user's mood. Start from everything you know: think of the \
         {noun} that best fit the mood, well known and lesser known, then use their taste to \
         choose between them and order them. The mood comes first, even if it's a genre they \
         rarely watch; taste comes second. Only include genuinely good fits: fewer picks are \
         better than weak ones. The numbered list holds extra options found for this user on \
         Trakt (recommendations, related and new titles you may not know).\n\n\
         Reply with JSON only: {{\"picks\": [{{\"title\": \"<exact title>\", \"year\": <first \
         release year>, \"reason\": \"<why>\"}}], \"from_list\": [{{\"id\": <number from the \
         list>, \"reason\": \"<why>\"}}]}}. `picks` are your own choices, up to {MAX_OWN}, best \
         first. `from_list` holds at most {MAX_LISTED} numbered options that fit the mood just \
         as well; leave it empty when none do. Each reason is one short sentence, addressed to \
         the user, that can mention what in their history it matches."
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
        "My mood: {mood}\n\nMy taste:\n{taste}\n\nExtra options from Trakt (id | title | about):\n{}",
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
        #[serde(default)]
        from_list: Vec<RawPick>,
    }
    #[derive(Deserialize)]
    struct RawPick {
        #[serde(default)]
        id: Value,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        year: Value,
        #[serde(default)]
        reason: String,
    }
    // Numbers sometimes come back as strings.
    let number = |v: &Value| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    };

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
    let mut seen_ids = HashSet::new();
    let mut seen_titles = HashSet::new();
    let mut picks = Vec::new();
    // Own choices first, then the list's; the caps hold wherever an entry
    // turns up.
    for p in reply.picks.into_iter().chain(reply.from_list) {
        let reason = p.reason.trim().to_owned();
        // A candidate id wins; an id that isn't a candidate is ignored and
        // the title is looked up instead.
        if let Some(id) = number(&p.id).filter(|id| known.contains(id)) {
            if seen_ids.len() < MAX_LISTED && seen_ids.insert(id) {
                picks.push(Pick {
                    choice: Choice::Candidate(id),
                    reason,
                });
            }
            continue;
        }
        let Some(title) = p
            .title
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        if seen_titles.len() < MAX_OWN && seen_titles.insert(title.to_lowercase()) {
            picks.push(Pick {
                choice: Choice::Title {
                    title,
                    year: number(&p.year).and_then(|y| i32::try_from(y).ok()),
                },
                reason,
            });
        }
    }
    picks
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
    fn takes_candidates_and_other_titles_once_each() {
        let reply = r#"{"picks": [
            {"id": 22, "title": "Ted Lasso", "reason": "Warm and funny."},
            {"id": 999, "reason": "Made-up id, no title."},
            {"id": "33", "reason": "Spy drama."},
            {"id": null, "title": "Parks and Recreation", "year": "2009", "reason": "Office-style."},
            {"id": 999, "title": "Brooklyn Nine-Nine", "year": 2013, "reason": "Unknown id."},
            {"id": 22, "reason": "Again."},
            {"title": "parks and recreation", "reason": "Again, other case."}
        ]}"#;
        let picks = parse_picks(reply, &candidates());
        let choices: Vec<&Choice> = picks.iter().map(|p| &p.choice).collect();
        assert_eq!(
            choices,
            [
                &Choice::Candidate(22),
                &Choice::Candidate(33),
                &Choice::Title {
                    title: "Parks and Recreation".into(),
                    year: Some(2009)
                },
                &Choice::Title {
                    title: "Brooklyn Nine-Nine".into(),
                    year: Some(2013)
                },
            ]
        );
        assert_eq!(picks[2].reason, "Office-style.");
    }

    #[test]
    fn caps_own_and_listed_picks_separately() {
        let own: Vec<String> = (0..12)
            .map(|i| format!(r#"{{"title": "Own {i}", "reason": "r"}}"#))
            .collect();
        let reply = format!(
            r#"{{"picks": [{}], "from_list": [{{"id": 11}}, {{"id": 22}}, {{"id": 33}}]}}"#,
            own.join(",")
        );
        let picks = parse_picks(&reply, &candidates());
        let listed = picks
            .iter()
            .filter(|p| matches!(p.choice, Choice::Candidate(_)))
            .count();
        assert_eq!(listed, MAX_LISTED);
        assert_eq!(picks.len(), MAX_OWN + MAX_LISTED);
        // Own choices come first.
        assert!(matches!(picks[0].choice, Choice::Title { .. }));
    }

    #[test]
    fn matches_titles_the_model_names() {
        assert!(same_title("The Office", Some(2005), "Office", Some(2005)));
        assert!(same_title(
            "Parks and Recreation",
            Some(2009),
            "Parks & Recreation",
            None
        ));
        assert!(same_title(
            "What We Do in the Shadows",
            Some(2019),
            "What We Do In The Shadows!",
            Some(2018)
        ));
        assert!(same_title(
            "The Blackcoat's Daughter",
            Some(2017),
            "The Blackcoat's Daughter",
            Some(2015)
        ));
        assert!(same_title(
            "Borat: Cultural Learnings of America for Make Benefit Glorious Nation of Kazakhstan",
            Some(2006),
            "Borat",
            Some(2006)
        ));
        // The UK Office is a different show.
        assert!(!same_title(
            "The Office",
            Some(2001),
            "The Office",
            Some(2005)
        ));
        // Same start, different film.
        assert!(!same_title("Up in the Air", Some(2009), "Up", Some(2009)));
        assert!(!same_title(
            "Borat Subsequent Moviefilm",
            Some(2020),
            "Borat",
            Some(2006)
        ));
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
        let picks = pick(&config, Kind::Shows, "spies", &candidates(), &taste)
            .await
            .unwrap();
        assert_eq!(
            picks,
            [Pick {
                choice: Choice::Candidate(33),
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
        let (system, user) = prompt(Kind::Shows, "something cosy", &candidates(), &taste);
        assert!(system.starts_with("You recommend TV shows."));
        assert!(system.contains("Start from everything you know"));
        assert!(system.contains("never recommend those"));
        assert!(user.contains("My mood: something cosy"));
        assert!(user.contains(r#"{"recent_shows":[{"title":"Slow Horses"}]}"#));
        assert!(user.contains("22 | Ted Lasso (2022) [drama] | About things."));
    }

    #[test]
    fn prompt_shortens_overviews() {
        let mut long = candidates();
        long[0].overview = Some("x".repeat(500));
        let (_, user) = prompt(Kind::Shows, "anything", &long, &json!({}));
        assert!(user.contains(&format!("| {}\n", "x".repeat(OVERVIEW_CHARS))));
        assert!(!user.contains(&"x".repeat(OVERVIEW_CHARS + 1)));
    }
}
