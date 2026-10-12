//! Tools the AI can call during AI picks to learn the user's taste: recent
//! shows, an aggregate taste profile, dropped shows, and the people (cast,
//! creators, directors, writers) behind shows.
//!
//! History tools answer from data already on the device. The people tools
//! look shows up on Trakt; the answers can be kept between requests with
//! [`ToolBox::with_people`] and [`ToolBox::into_people`].

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use crate::TraktClient;
use crate::labels::{Labels, ShowStatus};
use crate::models::{Show, ShowPeople};
use crate::progress::ProgressItem;
use crate::taste;

/// Liked shows whose people `favourite_people` looks at.
const PEOPLE_SHOWS: usize = 12;

#[derive(Debug, Clone, Serialize)]
struct ShowFacts {
    id: u64,
    title: String,
    year: Option<i32>,
    genres: Vec<String>,
    network: Option<String>,
    episode_minutes: Option<u32>,
    trakt_rating: Option<f64>,
    episodes_watched: u32,
    episodes_aired: u32,
    /// "in progress", "caught up", "finished", "on hold" or "dropped".
    status: &'static str,
    /// "liked", "not for me" or "undecided" (same rule as the taste model).
    verdict: &'static str,
    last_watched: Option<String>,
}

pub struct ToolBox {
    /// Watched shows, most recently watched first.
    shows: Vec<ShowFacts>,
    /// Shows hidden or dropped without being watched.
    unwatched_dropped: Vec<ShowFacts>,
    client: TraktClient,
    people: HashMap<u64, ShowPeople>,
}

impl ToolBox {
    pub fn new(
        items: &[ProgressItem],
        labels: &Labels,
        client: TraktClient,
        now: DateTime<Utc>,
    ) -> Self {
        let mut items: Vec<&ProgressItem> = items.iter().collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.last_watched_at));
        let shows: Vec<ShowFacts> = items
            .into_iter()
            .filter_map(|item| {
                let id = item.show.ids.trakt?;
                let label = labels.status(id);
                let status = match label {
                    Some(ShowStatus::OnHold) => "on hold",
                    Some(ShowStatus::Dropped) => "dropped",
                    None if item.episodes_left() > 0 => "in progress",
                    None if matches!(item.show.status.as_deref(), Some("ended" | "canceled")) => {
                        "finished"
                    }
                    None => "caught up",
                };
                let verdict = match taste::label(item, label, now) {
                    Some(true) => "liked",
                    Some(false) => "not for me",
                    None => "undecided",
                };
                Some(facts(id, &item.show, item, status, verdict))
            })
            .collect();
        let watched: HashSet<u64> = shows.iter().map(|s| s.id).collect();
        let unwatched_dropped = labels
            .iter()
            .filter(|(id, _)| !watched.contains(id))
            .filter_map(|(id, label)| {
                let show = label.show.as_ref()?;
                let status = match label.status {
                    ShowStatus::OnHold => "on hold",
                    ShowStatus::Dropped => "dropped",
                };
                Some(facts(
                    id,
                    show,
                    &ProgressItem::default_for(show),
                    status,
                    "not for me",
                ))
            })
            .collect();
        Self {
            shows,
            unwatched_dropped,
            client,
            people: HashMap::new(),
        }
    }

    /// Starts from people already looked up by an earlier request.
    pub fn with_people(mut self, people: HashMap<u64, ShowPeople>) -> Self {
        self.people = people;
        self
    }

    /// The people looked up so far, to reuse in the next request.
    pub fn into_people(self) -> HashMap<u64, ShowPeople> {
        self.people
    }

    /// Tool definitions in the OpenAI / DeepSeek `tools` format.
    pub fn specs() -> Value {
        let tool = |name: &str, description: &str, parameters: Value| {
            json!({
                "type": "function",
                "function": { "name": name, "description": description, "parameters": parameters },
            })
        };
        let none = json!({ "type": "object", "properties": {} });
        json!([
            tool(
                "recent_shows",
                "The user's most recently watched shows, newest first, with genres, network, \
                 episode length, how far they got, status and whether they liked it.",
                json!({
                    "type": "object",
                    "properties": {
                        "count": { "type": "integer", "enum": [5, 10, 15, 25], "description": "How many shows." }
                    },
                    "required": ["count"],
                }),
            ),
            tool(
                "taste_profile",
                "Aggregates over the user's whole history: favourite and disliked genres, \
                 networks, decades, episode lengths, and how many shows they finish or drop.",
                none.clone(),
            ),
            tool(
                "dropped_shows",
                "Shows the user dropped, put on hold, or abandoned early.",
                none.clone(),
            ),
            tool(
                "show_people",
                "Creators, directors, writers and main cast (with characters) of one show, by its \
                 id (a candidate id or an id from recent_shows).",
                json!({
                    "type": "object",
                    "properties": { "show_id": { "type": "integer" } },
                    "required": ["show_id"],
                }),
            ),
            tool(
                "favourite_people",
                "Actors, creators, directors and writers who appear in several of the shows the \
                 user liked recently.",
                none,
            ),
        ])
    }

    /// Runs one tool call. Errors come back as `{"error": ...}` so the model
    /// can carry on.
    pub async fn call(&mut self, name: &str, args: &Value) -> Value {
        match name {
            "recent_shows" => {
                let count = args["count"].as_u64().unwrap_or(10).clamp(1, 25) as usize;
                json!(self.shows.iter().take(count).collect::<Vec<_>>())
            }
            "taste_profile" => self.taste_profile(),
            "dropped_shows" => json!(
                self.shows
                    .iter()
                    .filter(|s| s.verdict == "not for me")
                    .chain(&self.unwatched_dropped)
                    .take(30)
                    .collect::<Vec<_>>()
            ),
            "show_people" => match args["show_id"].as_u64() {
                Some(id) => match self.people_of(id).await {
                    Ok(people) => summarize_people(people),
                    Err(e) => json!({ "error": e }),
                },
                None => json!({ "error": "show_id is required" }),
            },
            "favourite_people" => self.favourite_people().await,
            _ => json!({ "error": format!("unknown tool {name}") }),
        }
    }

    fn taste_profile(&self) -> Value {
        let liked: Vec<&ShowFacts> = self.shows.iter().filter(|s| s.verdict == "liked").collect();
        let disliked: Vec<&ShowFacts> = self
            .shows
            .iter()
            .filter(|s| s.verdict == "not for me")
            .chain(&self.unwatched_dropped)
            .collect();

        let tally = |shows: &[&ShowFacts], key: &dyn Fn(&ShowFacts) -> Vec<String>| {
            let mut counts: HashMap<String, usize> = HashMap::new();
            for show in shows {
                for k in key(show) {
                    *counts.entry(k).or_default() += 1;
                }
            }
            let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            counts.truncate(10);
            counts
        };
        let genres = |s: &ShowFacts| s.genres.clone();
        let network = |s: &ShowFacts| s.network.iter().cloned().collect();
        let decade = |s: &ShowFacts| {
            s.year
                .map(|y| format!("{}s", y / 10 * 10))
                .into_iter()
                .collect()
        };
        let length = |s: &ShowFacts| {
            s.episode_minutes
                .map(|m| match m {
                    0..=30 => "up to 30 min".to_string(),
                    31..=50 => "31-50 min".into(),
                    _ => "over 50 min".into(),
                })
                .into_iter()
                .collect()
        };
        let ratings: Vec<f64> = liked.iter().filter_map(|s| s.trakt_rating).collect();

        json!({
            "shows_watched": self.shows.len(),
            "liked": liked.len(),
            "not_for_me": disliked.len(),
            "finished": self.shows.iter().filter(|s| s.status == "finished").count(),
            "liked_genres": tally(&liked, &genres),
            "disliked_genres": tally(&disliked, &genres),
            "liked_networks": tally(&liked, &network),
            "liked_decades": tally(&liked, &decade),
            "liked_episode_lengths": tally(&liked, &length),
            "average_trakt_rating_of_liked": (!ratings.is_empty())
                .then(|| (ratings.iter().sum::<f64>() / ratings.len() as f64 * 10.0).round() / 10.0),
        })
    }

    async fn people_of(&mut self, show_id: u64) -> Result<&ShowPeople, String> {
        if !self.people.contains_key(&show_id) {
            let people = self
                .client
                .show_people(show_id)
                .await
                .map_err(|e| e.to_string())?;
            self.people.insert(show_id, people);
        }
        Ok(&self.people[&show_id])
    }

    /// Actors, creators, directors and writers in several recently liked
    /// shows. Looks up the shows' people all at once.
    pub async fn favourite_people(&mut self) -> Value {
        let liked: Vec<(u64, String)> = self
            .shows
            .iter()
            .filter(|s| s.verdict == "liked")
            .take(PEOPLE_SHOWS)
            .map(|s| (s.id, s.title.clone()))
            .collect();
        let ids: Vec<u64> = liked.iter().map(|(id, _)| *id).collect();
        fetch_people(&self.client, &ids, &mut self.people).await;

        // person -> (roles, shows)
        let mut seen: HashMap<String, (HashSet<String>, Vec<String>)> = HashMap::new();
        for (id, title) in &liked {
            let Some(people) = self.people.get(id) else {
                continue;
            };
            for (name, role) in key_people(people) {
                let entry = seen.entry(name).or_default();
                entry.0.insert(role);
                if !entry.1.contains(title) {
                    entry.1.push(title.clone());
                }
            }
        }
        let mut repeat: Vec<_> = seen
            .into_iter()
            .filter(|(_, (_, shows))| shows.len() >= 2)
            .collect();
        repeat.sort_by(|a, b| b.1.1.len().cmp(&a.1.1.len()).then(a.0.cmp(&b.0)));
        repeat.truncate(20);
        json!({
            "looked_at": liked.iter().map(|(_, t)| t).collect::<Vec<_>>(),
            "people_in_several_liked_shows": repeat
                .into_iter()
                .map(|(name, (roles, shows))| {
                    let mut roles: Vec<String> = roles.into_iter().collect();
                    roles.sort();
                    json!({ "name": name, "roles": roles, "shows": shows })
                })
                .collect::<Vec<_>>(),
        })
    }
}

/// Looks up the people of the shows not in `cache` yet, in parallel, and
/// adds them. Shows that fail to load are left out.
pub async fn fetch_people(
    client: &TraktClient,
    show_ids: &[u64],
    cache: &mut HashMap<u64, ShowPeople>,
) {
    let mut seen = HashSet::new();
    let missing: Vec<u64> = show_ids
        .iter()
        .copied()
        .filter(|id| !cache.contains_key(id) && seen.insert(*id))
        .collect();
    let found = futures::future::join_all(missing.iter().map(|&id| client.show_people(id))).await;
    for (id, people) in missing.into_iter().zip(found) {
        if let Ok(people) = people {
            cache.insert(id, people);
        }
    }
}

fn facts(
    id: u64,
    show: &Show,
    item: &ProgressItem,
    status: &'static str,
    verdict: &'static str,
) -> ShowFacts {
    ShowFacts {
        id,
        title: show.display_title().to_owned(),
        year: show.year,
        genres: show.genres.clone().unwrap_or_default(),
        network: show.network.clone(),
        episode_minutes: show.runtime,
        trakt_rating: show.rating.map(|r| (r * 10.0).round() / 10.0),
        episodes_watched: item.completed,
        episodes_aired: item.aired,
        status,
        verdict,
        last_watched: item
            .last_watched_at
            .map(|t| t.format("%Y-%m-%d").to_string()),
    }
}

impl ProgressItem {
    /// An item for a show with no watch history.
    fn default_for(show: &Show) -> Self {
        ProgressItem {
            show: show.clone(),
            aired: show.aired_episodes.unwrap_or_default(),
            completed: 0,
            last_watched_at: None,
            next_episode: None,
            poster: None,
        }
    }
}

/// Main cast, creators, directors and writers, as (name, role) pairs.
fn key_people(people: &ShowPeople) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = people
        .cast
        .iter()
        .flatten()
        .take(15)
        .map(|c| (c.person.name.clone(), "actor".to_string()))
        .collect();
    for (department, role, limit) in [
        ("created by", "creator", 5),
        ("directing", "director", 5),
        ("writing", "writer", 5),
    ] {
        out.extend(
            crew_by(people, department)
                .into_iter()
                .take(limit)
                .map(|name| (name, role.to_string())),
        );
    }
    out
}

/// Names in a crew department, most episodes first.
fn crew_by(people: &ShowPeople, department: &str) -> Vec<String> {
    let mut members: Vec<_> = people
        .crew
        .as_ref()
        .and_then(|c| c.get(department))
        .into_iter()
        .flatten()
        .collect();
    members.sort_by_key(|m| std::cmp::Reverse(m.episode_count.unwrap_or_default()));
    let mut seen = HashSet::new();
    members
        .into_iter()
        .map(|m| m.person.name.clone())
        .filter(|n| seen.insert(n.clone()))
        .collect()
}

fn summarize_people(people: &ShowPeople) -> Value {
    let take = |department: &str, n: usize| {
        crew_by(people, department)
            .into_iter()
            .take(n)
            .collect::<Vec<_>>()
    };
    json!({
        "created_by": take("created by", 5),
        "directors": take("directing", 6),
        "writers": take("writing", 6),
        "cast": people
            .cast
            .iter()
            .flatten()
            .take(12)
            .map(|c| json!({ "name": c.person.name, "character": c.characters.join(" / ") }))
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CastMember, CrewMember, Ids, Person};
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn item(id: u64, title: &str, genres: &[&str], completed: u32, days_ago: i64) -> ProgressItem {
        ProgressItem {
            show: Show {
                title: Some(title.into()),
                ids: Ids {
                    trakt: Some(id),
                    ..Default::default()
                },
                genres: Some(genres.iter().map(|g| g.to_string()).collect()),
                network: Some("HBO".into()),
                status: Some("ended".into()),
                ..Default::default()
            },
            aired: 20,
            completed,
            last_watched_at: Some(now() - Duration::days(days_ago)),
            next_episode: None,
            poster: None,
        }
    }

    fn toolbox() -> ToolBox {
        let items = [
            item(1, "Old Favourite", &["drama"], 20, 300),
            item(2, "Recent Comedy", &["comedy"], 12, 1),
            item(3, "Abandoned Reality", &["reality"], 1, 200),
        ];
        let mut labels = Labels::default();
        labels.set(3, Some(ShowStatus::Dropped), now() - Duration::days(150));
        ToolBox::new(&items, &labels, TraktClient::new("id"), now())
    }

    #[tokio::test]
    async fn recent_shows_are_newest_first_with_status() {
        let mut tools = toolbox();
        let recent = tools.call("recent_shows", &json!({ "count": 5 })).await;
        let titles: Vec<&str> = recent
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["title"].as_str().unwrap())
            .collect();
        assert_eq!(
            titles,
            ["Recent Comedy", "Abandoned Reality", "Old Favourite"]
        );
        assert_eq!(recent[0]["verdict"], "liked");
        assert_eq!(recent[1]["status"], "dropped");
        assert_eq!(recent[2]["status"], "finished");
    }

    #[tokio::test]
    async fn taste_profile_aggregates_likes_and_dislikes() {
        let mut tools = toolbox();
        let profile = tools.call("taste_profile", &json!({})).await;
        assert_eq!(profile["liked"], 2);
        assert_eq!(profile["not_for_me"], 1);
        assert_eq!(profile["disliked_genres"][0][0], "reality");
        assert_eq!(profile["liked_networks"][0], json!(["HBO", 2]));
    }

    #[tokio::test]
    async fn unknown_tools_and_bad_arguments_return_errors() {
        let mut tools = toolbox();
        assert!(tools.call("nope", &json!({})).await["error"].is_string());
        assert!(tools.call("show_people", &json!({})).await["error"].is_string());
    }

    #[tokio::test]
    async fn favourite_people_reuses_people_from_earlier_requests() {
        let cast = |names: &[&str]| ShowPeople {
            cast: Some(
                names
                    .iter()
                    .map(|n| CastMember {
                        characters: vec![],
                        episode_count: None,
                        person: Person { name: (*n).into() },
                    })
                    .collect(),
            ),
            crew: None,
        };
        // Both liked shows are cached, so nothing is fetched from Trakt.
        let mut tools = toolbox().with_people(HashMap::from([
            (1, cast(&["Shared Actor", "Only Old"])),
            (2, cast(&["Shared Actor", "Only Recent"])),
        ]));
        let people = tools.call("favourite_people", &json!({})).await;
        let repeat = &people["people_in_several_liked_shows"];
        assert_eq!(repeat.as_array().unwrap().len(), 1);
        assert_eq!(repeat[0]["name"], "Shared Actor");
        assert_eq!(tools.into_people().len(), 2);
    }

    #[test]
    fn people_summary_orders_crew_by_episodes() {
        let person = |name: &str| Person { name: name.into() };
        let people = ShowPeople {
            cast: Some(vec![CastMember {
                characters: vec!["Mordecai".into()],
                episode_count: Some(40),
                person: person("J.G. Quintel"),
            }]),
            crew: Some(HashMap::from([(
                "directing".to_string(),
                vec![
                    CrewMember {
                        jobs: vec!["Director".into()],
                        episode_count: Some(2),
                        person: person("Rare"),
                    },
                    CrewMember {
                        jobs: vec!["Director".into()],
                        episode_count: Some(30),
                        person: person("Main"),
                    },
                ],
            )])),
        };
        let summary = summarize_people(&people);
        assert_eq!(summary["directors"], json!(["Main", "Rare"]));
        assert_eq!(summary["cast"][0]["character"], "Mordecai");
        assert_eq!(key_people(&people).len(), 3);
    }
}
