//! "On hold" and "Dropped" labels for shows.
//!
//! Labels only hide a show from Progress and Calendar; watch history on Trakt
//! is untouched. Watching any episode after a show was labelled clears the
//! label, so the show comes back on its own.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::progress::ProgressItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowStatus {
    OnHold,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub status: ShowStatus,
    pub since: DateTime<Utc>,
}

/// Labels by Trakt show id. Persist it between launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Labels {
    shows: HashMap<u64, Label>,
}

impl Labels {
    pub fn get(&self, show_id: u64) -> Option<&Label> {
        self.shows.get(&show_id)
    }

    pub fn status(&self, show_id: u64) -> Option<ShowStatus> {
        self.get(show_id).map(|l| l.status)
    }

    /// `None` puts the show back to watching.
    pub fn set(&mut self, show_id: u64, status: Option<ShowStatus>, now: DateTime<Utc>) {
        match status {
            Some(status) => self.shows.insert(show_id, Label { status, since: now }),
            None => self.shows.remove(&show_id),
        };
    }

    /// Clears labels of shows watched after they were labelled. Returns
    /// whether anything changed, so the caller knows to save.
    pub fn clear_rewatched(&mut self, items: &[ProgressItem]) -> bool {
        let before = self.shows.len();
        for item in items {
            let (Some(id), Some(watched)) = (item.show.ids.trakt, item.last_watched_at) else {
                continue;
            };
            if self
                .shows
                .get(&id)
                .is_some_and(|label| watched > label.since)
            {
                self.shows.remove(&id);
            }
        }
        self.shows.len() != before
    }

    pub fn is_hidden(&self, show_id: Option<u64>) -> bool {
        show_id.is_some_and(|id| self.shows.contains_key(&id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Ids, Show};
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn item(id: u64, watched_days_ago: i64) -> ProgressItem {
        ProgressItem {
            show: Show {
                ids: Ids {
                    trakt: Some(id),
                    ..Default::default()
                },
                ..Default::default()
            },
            aired: 10,
            completed: 5,
            last_watched_at: Some(now() - Duration::days(watched_days_ago)),
            next_episode: None,
            poster: None,
        }
    }

    #[test]
    fn labelled_shows_are_hidden_until_cleared() {
        let mut labels = Labels::default();
        labels.set(1, Some(ShowStatus::OnHold), now());
        assert!(labels.is_hidden(Some(1)));
        assert!(!labels.is_hidden(Some(2)));
        labels.set(1, None, now());
        assert!(!labels.is_hidden(Some(1)));
    }

    #[test]
    fn watching_again_clears_the_label() {
        let mut labels = Labels::default();
        labels.set(1, Some(ShowStatus::Dropped), now() - Duration::days(3));
        labels.set(2, Some(ShowStatus::OnHold), now() - Duration::days(3));

        // Show 1 was watched yesterday (after dropping); show 2 last a week ago.
        let changed = labels.clear_rewatched(&[item(1, 1), item(2, 7)]);
        assert!(changed);
        assert_eq!(labels.status(1), None);
        assert_eq!(labels.status(2), Some(ShowStatus::OnHold));
        assert!(!labels.clear_rewatched(&[item(2, 7)]));
    }
}
