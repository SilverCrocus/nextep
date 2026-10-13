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
/// Picks the model chooses itself, from everything it knows. The caller
/// caps these after looking them up, so a few more are kept from the reply
/// to stand in for ones Trakt doesn't have or the user has seen.
pub const MAX_OWN: usize = 8;
const SPARE_OWN: usize = 4;
/// Picks it may take from the candidates. They come in a separate list with
/// a hard cap: otherwise the model leans on them even when the mood calls for
/// something else entirely.
pub const MAX_LISTED: usize = 2;
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

/// How well a title found on Trakt matches the one the model named, best
/// first: 0 the same title and year; 1 the same title within two years
/// (festival premieres and wide releases differ); 2 the same title with a
/// year missing on one side; 3 one is the other without its subtitle
/// ("Borat" for "Borat: Cultural Learnings…"), same year. `None` for a
/// different title. Titles compare ignoring case, punctuation, "&" vs "and"
/// and a leading "The".
pub fn title_match(
    found: &str,
    found_year: Option<i32>,
    named: &str,
    named_year: Option<i32>,
) -> Option<u8> {
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
        return None;
    }
    if full == norm(named) {
        return match (found_year, named_year) {
            (Some(a), Some(b)) if a == b => Some(0),
            (Some(a), Some(b)) if (a - b).abs() <= 2 => Some(1),
            (Some(_), Some(_)) => None,
            _ => Some(2),
        };
    }
    // One side is the other without its subtitle; two different subtitles
    // ("Fear Street: 1978" and "…: 1994") are different titles.
    let short_for = |short: &str, long: &str| {
        main_title(short) == short && norm(short) == norm(main_title(long))
    };
    let same_year = matches!((found_year, named_year), (Some(a), Some(b)) if a == b);
    (same_year && (short_for(named, found) || short_for(found, named))).then_some(3)
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
    let Some(reply) = reply_object(content) else {
        return Vec::new();
    };
    let entries = |key: &str| {
        reply
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let known: HashSet<u64> = candidates.iter().map(|c| c.id).collect();
    let mut listed = HashSet::new();
    let mut own: Vec<(String, Option<i32>)> = Vec::new();
    let mut picks = Vec::new();
    // Own choices first, then the list's; the caps hold wherever an entry
    // turns up.
    for p in entries("picks").iter().chain(&entries("from_list")) {
        let reason = p
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_owned();
        // A candidate id wins; an id that isn't a candidate is ignored and
        // the title is looked up instead.
        if let Some(id) = p.get("id").and_then(number).filter(|id| known.contains(id)) {
            if listed.len() < MAX_LISTED && listed.insert(id) {
                picks.push(Pick {
                    choice: Choice::Candidate(id),
                    reason,
                });
            }
            continue;
        }
        // Titles can come back as numbers ("1917").
        let title = match p.get("title") {
            Some(Value::String(t)) => t.trim().to_owned(),
            Some(Value::Number(n)) => n.to_string(),
            _ => continue,
        };
        let year = p.get("year").and_then(year_of);
        let lower = title.to_lowercase();
        // The same title twice is one pick, unless the years tell them apart
        // ("Dune" 1984 and 2021).
        let repeat = own
            .iter()
            .any(|(t, y)| *t == lower && (year.is_none() || y.is_none() || *y == year));
        if title.is_empty() || repeat || own.len() >= MAX_OWN + SPARE_OWN {
            continue;
        }
        own.push((lower, year));
        picks.push(Pick {
            choice: Choice::Title { title, year },
            reason,
        });
    }
    picks
}

/// The reply's JSON object. Models sometimes wrap it in a ```json fence, a
/// sentence or a `<think>` block that can itself contain braces, so this
/// takes the first object that has picks in it.
fn reply_object(content: &str) -> Option<serde_json::Map<String, Value>> {
    let content = match content.rfind("</think>") {
        Some(end) => &content[end + "</think>".len()..],
        None => content,
    };
    content.match_indices('{').find_map(|(start, _)| {
        let mut values = serde_json::Deserializer::from_str(&content[start..]).into_iter::<Value>();
        match values.next() {
            Some(Ok(Value::Object(map)))
                if map.contains_key("picks") || map.contains_key("from_list") =>
            {
                Some(map)
            }
            _ => None,
        }
    })
}

/// A whole number, written as a number or in a string.
fn number(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| {
            v.as_f64()
                .filter(|f| f.fract() == 0.0 && *f >= 0.0)
                .map(|f| f as u64)
        })
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A year, also from strings like "2019–2023" or "2019-".
fn year_of(v: &Value) -> Option<i32> {
    if let Some(n) = number(v) {
        return i32::try_from(n).ok();
    }
    let s = v.as_str()?;
    let start = s.find(|c: char| c.is_ascii_digit())?;
    let digits: String = s[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
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
        assert_eq!(picks.len(), MAX_OWN + SPARE_OWN + MAX_LISTED);
        // Own choices come first.
        assert!(matches!(picks[0].choice, Choice::Title { .. }));
    }

    #[test]
    fn matches_titles_the_model_names() {
        let m = title_match;
        assert_eq!(m("The Office", Some(2005), "Office", Some(2005)), Some(0));
        assert_eq!(
            m(
                "Parks and Recreation",
                Some(2009),
                "Parks & Recreation",
                None
            ),
            Some(2)
        );
        assert_eq!(
            m(
                "What We Do in the Shadows",
                Some(2019),
                "What We Do In The Shadows!",
                Some(2018)
            ),
            Some(1)
        );
        assert_eq!(
            m(
                "Borat: Cultural Learnings of America for Make Benefit Glorious Nation of Kazakhstan",
                Some(2006),
                "Borat",
                Some(2006)
            ),
            Some(3)
        );
        // The UK Office is a different show; so are remakes and other films
        // that only start the same.
        assert_eq!(m("The Office", Some(2001), "The Office", Some(2005)), None);
        assert_eq!(m("Up in the Air", Some(2009), "Up", Some(2009)), None);
        assert_eq!(
            m(
                "Borat Subsequent Moviefilm",
                Some(2020),
                "Borat",
                Some(2006)
            ),
            None
        );
        // Same series, different entries in the same year.
        assert_eq!(
            m(
                "Fear Street: 1978",
                Some(2021),
                "Fear Street: 1994",
                Some(2021)
            ),
            None
        );
        assert_eq!(m(": x", Some(2020), "— y", Some(2020)), None);
    }

    #[test]
    fn reads_replies_leniently() {
        let picks = |reply: &str| parse_picks(reply, &candidates());
        // Nulls where a list or text was expected.
        let nulls = r#"{"picks": [{"title": "Notting Hill", "year": 1999, "reason": null}], "from_list": null}"#;
        assert_eq!(picks(nulls).len(), 1);
        // A title that's a number, and a year in a string range.
        let numeric = r#"{"picks": [{"title": 1917, "year": "2019–2020", "reason": "War."}]}"#;
        assert_eq!(
            picks(numeric)[0].choice,
            Choice::Title {
                title: "1917".into(),
                year: Some(2019)
            }
        );
        // Thinking (with braces) before the answer, and a note after it.
        let wrapped = "<think>maybe {\"picks\": []} no</think>Sure: {\"picks\": [{\"title\": \"Heat\", \"year\": 1995}]} {enjoy}";
        assert_eq!(picks(wrapped).len(), 1);
        // Same title in different years are two picks; repeats are one.
        let dune = r#"{"picks": [{"title": "Dune", "year": 1984}, {"title": "Dune", "year": 2021}, {"title": "dune"}]}"#;
        assert_eq!(picks(dune).len(), 2);
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
