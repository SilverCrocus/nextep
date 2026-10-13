//! What the home-screen widget shows: the next few episodes from Progress.
//!
//! New episodes and shows in progress come first (what can be watched now),
//! then upcoming episodes. The widget formats dates itself, in the phone's
//! time zone and language.

use serde::Serialize;

use crate::progress::{ProgressItem, ProgressView};

/// Rows the widget has room for.
pub const ROWS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetRow {
    pub title: String,
    /// "S01E04".
    pub code: String,
    pub episode: Option<String>,
    /// Episodes left to watch, for shows that can be watched now.
    pub left: Option<u32>,
    /// When it airs (milliseconds since 1970), for upcoming episodes.
    pub airs_at: Option<i64>,
}

pub fn rows(view: &ProgressView) -> Vec<WidgetRow> {
    let now = view
        .new_episodes
        .iter()
        .chain(&view.watching)
        .filter_map(|item| row(item, false));
    let upcoming = view.upcoming.iter().filter_map(|item| row(item, true));
    now.chain(upcoming).take(ROWS).collect()
}

fn row(item: &ProgressItem, upcoming: bool) -> Option<WidgetRow> {
    let episode = item.next_episode.as_ref()?;
    Some(WidgetRow {
        title: item.show.display_title().to_owned(),
        code: episode.code(),
        episode: episode.title.clone().filter(|t| !t.is_empty()),
        left: (!upcoming).then(|| item.episodes_left()),
        airs_at: upcoming
            .then_some(episode.first_aired)
            .flatten()
            .map(|at| at.timestamp_millis()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Episode, Ids, Show};
    use chrono::{TimeZone, Utc};

    fn item(title: &str, number: u32, aired: u32, completed: u32) -> ProgressItem {
        ProgressItem {
            show: Show {
                title: Some(title.into()),
                ids: Ids::default(),
                ..Default::default()
            },
            aired,
            completed,
            last_watched_at: None,
            next_episode: Some(Episode {
                season: 1,
                number,
                title: Some(format!("Episode {number}")),
                first_aired: Some(Utc.with_ymd_and_hms(2026, 10, 16, 7, 0, 0).unwrap()),
                ..Default::default()
            }),
            poster: None,
        }
    }

    #[test]
    fn watchable_first_then_upcoming_up_to_four() {
        let view = ProgressView {
            new_episodes: vec![item("New", 2, 3, 1)],
            upcoming: vec![item("Soon", 4, 3, 3), item("Later", 1, 0, 0)],
            watching: vec![item("A", 5, 10, 4), item("B", 6, 10, 5)],
        };
        let rows = rows(&view);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["New", "A", "B", "Soon"]);
        assert_eq!(rows[0].code, "S01E02");
        assert_eq!(rows[0].left, Some(2));
        assert_eq!(rows[0].airs_at, None);
        assert_eq!(rows[3].left, None);
        assert_eq!(
            rows[3].airs_at,
            Some(
                Utc.with_ymd_and_hms(2026, 10, 16, 7, 0, 0)
                    .unwrap()
                    .timestamp_millis()
            )
        );
    }
}
