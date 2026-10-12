//! Data shapes returned by the Trakt API (v2).
//!
//! Fields are optional wherever Trakt is known to send `null`, so a single odd
//! record never fails a whole sync.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Ids {
    pub trakt: Option<u64>,
    pub slug: Option<String>,
    pub tvdb: Option<u64>,
    pub imdb: Option<String>,
    pub tmdb: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Show {
    pub title: Option<String>,
    pub year: Option<i32>,
    pub ids: Ids,
    // The fields below are only present with `?extended=full`.
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub network: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub runtime: Option<u32>,
    #[serde(default)]
    pub rating: Option<f64>,
    #[serde(default)]
    pub aired_episodes: Option<u32>,
    #[serde(default)]
    pub genres: Option<Vec<String>>,
}

impl Show {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().unwrap_or("Untitled")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    pub season: u32,
    pub number: u32,
    pub title: Option<String>,
    pub ids: Ids,
    #[serde(default)]
    pub first_aired: Option<DateTime<Utc>>,
    #[serde(default)]
    pub runtime: Option<u32>,
    #[serde(default)]
    pub overview: Option<String>,
}

impl Episode {
    /// "S01E05"-style label.
    pub fn code(&self) -> String {
        format!("S{:02}E{:02}", self.season, self.number)
    }
}

/// One entry of `GET /sync/watched/shows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchedShow {
    pub plays: u32,
    pub last_watched_at: Option<DateTime<Utc>>,
    pub last_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reset_at: Option<DateTime<Utc>>,
    pub show: Show,
}

/// `GET /shows/{id}/progress/watched`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShowProgress {
    pub aired: u32,
    pub completed: u32,
    pub last_watched_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reset_at: Option<DateTime<Utc>>,
    pub next_episode: Option<Episode>,
    #[serde(default)]
    pub last_episode: Option<Episode>,
    /// Per-episode detail. Only aired episodes are listed.
    #[serde(default)]
    pub seasons: Vec<SeasonProgress>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SeasonProgress {
    pub number: u32,
    pub aired: u32,
    pub completed: u32,
    #[serde(default)]
    pub episodes: Vec<EpisodeProgress>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EpisodeProgress {
    pub number: u32,
    pub completed: bool,
    #[serde(default)]
    pub last_watched_at: Option<DateTime<Utc>>,
}

/// One entry of `GET /shows/{id}/seasons?extended=full,episodes`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Season {
    pub number: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub episodes: Vec<Episode>,
}

/// One entry of `GET /calendars/my/shows/{start}/{days}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalendarEntry {
    pub first_aired: DateTime<Utc>,
    pub episode: Episode,
    pub show: Show,
}

/// A personal list (`GET /users/me/lists`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraktList {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub item_count: u32,
    pub ids: ListIds,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListIds {
    pub trakt: u64,
}

impl TraktList {
    /// Lists that a service such as couchmoney or mdblist regenerates. Edits
    /// made in Nextep would be overwritten, so these are read-only.
    pub fn managed_by(&self) -> Option<&'static str> {
        let description = self
            .description
            .as_deref()
            .unwrap_or_default()
            .to_lowercase();
        if description.contains("couchmoney") {
            Some("couchmoney.tv")
        } else if description.contains("mdblist") {
            Some("mdblist")
        } else {
            None
        }
    }
}

/// One show on a list (`GET /users/me/lists/{id}/items/show`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedShow {
    #[serde(default)]
    pub listed_at: Option<DateTime<Utc>>,
    pub show: Show,
}

/// `GET /users/settings` (only the account limits).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserSettings {
    #[serde(default)]
    pub limits: Option<Limits>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    #[serde(default)]
    pub list: Option<ListLimit>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ListLimit {
    pub count: u32,
    pub item_count: u32,
}

/// `GET /users/me/stats` (only the parts Nextep shows).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserStats {
    pub shows: ShowStats,
    pub episodes: EpisodeStats,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShowStats {
    pub watched: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EpisodeStats {
    pub watched: u64,
    pub minutes: u64,
}

/// One entry of `GET /sync/history/episodes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: u64,
    pub watched_at: DateTime<Utc>,
    pub episode: Episode,
    pub show: Show,
}

/// One entry of `GET /users/hidden/{section}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HiddenShow {
    pub hidden_at: Option<DateTime<Utc>>,
    pub show: Option<Show>,
}

/// One entry of `GET /search/show`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResult {
    pub score: Option<f64>,
    pub show: Show,
}

/// `GET /shows/{id}/people`: cast, and crew by department ("created by",
/// "directing", "writing", ...).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShowPeople {
    #[serde(default)]
    pub cast: Option<Vec<CastMember>>,
    #[serde(default)]
    pub crew: Option<std::collections::HashMap<String, Vec<CrewMember>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CastMember {
    #[serde(default)]
    pub characters: Vec<String>,
    #[serde(default)]
    pub episode_count: Option<u32>,
    pub person: Person,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewMember {
    #[serde(default)]
    pub jobs: Vec<String>,
    #[serde(default)]
    pub episode_count: Option<u32>,
    pub person: Person,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
}

/// One entry of `GET /shows/trending`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrendingShow {
    #[serde(default)]
    pub watchers: u32,
    pub show: Show,
}

/// One entry of `GET /sync/watchlist/shows`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WatchlistShow {
    pub rank: Option<u32>,
    pub listed_at: DateTime<Utc>,
    pub show: Show,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_lists_managed_by_services() {
        let list = |description: &str| TraktList {
            name: "x".into(),
            description: Some(description.into()),
            item_count: 0,
            ids: ListIds { trakt: 1 },
        };
        assert_eq!(
            list("This list is managed by couchmoney -- to edit or delete it, go to couchmoney.tv")
                .managed_by(),
            Some("couchmoney.tv")
        );
        assert_eq!(
            list("This list is maintained by mdblist.com").managed_by(),
            Some("mdblist")
        );
        assert_eq!(list("Shows to watch with Sam").managed_by(), None);
    }

    #[test]
    fn parses_progress_with_nulls() {
        let json = r#"{
            "aired": 18, "completed": 6,
            "last_watched_at": "2026-09-26T10:00:00.000Z",
            "reset_at": null,
            "seasons": [],
            "next_episode": {
                "season": 1, "number": 13, "title": "Smoke Alarm on the Water",
                "ids": {"trakt": 1, "tvdb": null, "imdb": null, "tmdb": 2, "tvrage": null}
            },
            "last_episode": null
        }"#;
        let p: ShowProgress = serde_json::from_str(json).unwrap();
        assert_eq!(p.aired - p.completed, 12);
        assert_eq!(p.next_episode.unwrap().code(), "S01E13");
    }
}
