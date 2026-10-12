//! What AI picks tells the model about the user's taste: recent shows, an
//! aggregate taste profile, dropped shows, and the people (cast, creators,
//! directors, writers) who recur in shows they liked.
//!
//! It is all worked out before the request and sent in the prompt, so a pick
//! is a single model call. History comes from data already on the device;
//! favourite people are looked up on Trakt, all shows at once, and kept in a
//! cache the caller holds between requests.

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
/// Recently watched shows sent in full.
const RECENT_SHOWS: usize = 15;
/// Dropped and disliked shows sent in full.
const DROPPED_SHOWS: usize = 20;

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

pub struct TasteContext {
    /// Watched shows, most recently watched first.
    shows: Vec<ShowFacts>,
    /// Shows hidden or dropped without being watched.
    unwatched_dropped: Vec<ShowFacts>,
}

impl TasteContext {
    pub fn new(items: &[ProgressItem], labels: &Labels, now: DateTime<Utc>) -> Self {
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
        }
    }

    /// Everything the model gets to know about the user, for the prompt.
    /// Favourite people come from `people` or Trakt; shows Trakt fails on are
    /// left out.
    pub async fn to_prompt_json(
        &self,
        client: &TraktClient,
        people: &mut HashMap<u64, ShowPeople>,
    ) -> Value {
        json!({
            "taste_profile": self.taste_profile(),
            "recent_shows": self.recent_shows(),
            "dropped_shows": self.dropped_shows(),
            "favourite_people": self.favourite_people(client, people).await,
        })
    }

    fn recent_shows(&self) -> Value {
        json!(self.shows.iter().take(RECENT_SHOWS).collect::<Vec<_>>())
    }

    fn dropped_shows(&self) -> Value {
        json!(
            self.shows
                .iter()
                .filter(|s| s.verdict == "not for me")
                .chain(&self.unwatched_dropped)
                .take(DROPPED_SHOWS)
                .collect::<Vec<_>>()
        )
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

    async fn favourite_people(
        &self,
        client: &TraktClient,
        cache: &mut HashMap<u64, ShowPeople>,
    ) -> Value {
        let liked: Vec<(u64, String)> = self
            .shows
            .iter()
            .filter(|s| s.verdict == "liked")
            .take(PEOPLE_SHOWS)
            .map(|s| (s.id, s.title.clone()))
            .collect();
        let ids: Vec<u64> = liked.iter().map(|(id, _)| *id).collect();
        fetch_people(client, &ids, cache).await;

        // person -> (roles, shows)
        let mut seen: HashMap<String, (HashSet<String>, Vec<String>)> = HashMap::new();
        for (id, title) in &liked {
            let Some(people) = cache.get(id) else {
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

/// Looks up the people of the shows not in `cache` yet, all at once, and
/// adds them. Shows that fail to load are left out and retried next time.
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

    fn context() -> TasteContext {
        let items = [
            item(1, "Old Favourite", &["drama"], 20, 300),
            item(2, "Recent Comedy", &["comedy"], 12, 1),
            item(3, "Abandoned Reality", &["reality"], 1, 200),
        ];
        let mut labels = Labels::default();
        labels.set(3, Some(ShowStatus::Dropped), now() - Duration::days(150));
        TasteContext::new(&items, &labels, now())
    }

    #[test]
    fn recent_shows_are_newest_first_with_status() {
        let recent = context().recent_shows();
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

    #[test]
    fn taste_profile_aggregates_likes_and_dislikes() {
        let profile = context().taste_profile();
        assert_eq!(profile["liked"], 2);
        assert_eq!(profile["not_for_me"], 1);
        assert_eq!(profile["disliked_genres"][0][0], "reality");
        assert_eq!(profile["liked_networks"][0], json!(["HBO", 2]));
    }

    #[tokio::test]
    async fn prompt_json_has_everything_and_survives_trakt_failing() {
        // Not signed in, so the people lookups fail.
        let all = context()
            .to_prompt_json(&TraktClient::new("id"), &mut HashMap::new())
            .await;
        assert_eq!(all["taste_profile"]["shows_watched"], 3);
        assert_eq!(all["recent_shows"].as_array().unwrap().len(), 3);
        assert_eq!(all["dropped_shows"][0]["title"], "Abandoned Reality");
        assert_eq!(
            all["favourite_people"]["people_in_several_liked_shows"],
            json!([])
        );
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
        // Both liked shows are cached, so nothing is fetched from Trakt
        // (which would fail: not signed in).
        let mut cache = HashMap::from([
            (1, cast(&["Shared Actor", "Only Old"])),
            (2, cast(&["Shared Actor", "Only Recent"])),
        ]);
        let people = context()
            .favourite_people(&TraktClient::new("id"), &mut cache)
            .await;
        let repeat = &people["people_in_several_liked_shows"];
        assert_eq!(repeat.as_array().unwrap().len(), 1);
        assert_eq!(repeat[0]["name"], "Shared Actor");
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn key_people_orders_crew_by_episodes() {
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
        assert_eq!(crew_by(&people, "directing"), ["Main", "Rare"]);
        assert_eq!(key_people(&people).len(), 3);
    }
}
