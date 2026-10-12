//! "On hold" and "Dropped" labels for shows.
//!
//! Labels only hide a show from Progress and Calendar; watch history is
//! untouched. They are stored on Trakt as hidden items, so they survive a
//! reinstall and show up in other Trakt apps:
//!
//! - On hold: hidden from watched progress (`progress_watched`), which is also
//!   where Showly put the shows it hid.
//! - Dropped: Trakt's `dropped` list.
//!
//! Watching any episode after a show was labelled clears the label, so the
//! show comes back on its own.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{HiddenShow, Show};
use crate::progress::ProgressItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowStatus {
    OnHold,
    Dropped,
}

impl ShowStatus {
    /// The Trakt hidden-items section that stores this label.
    pub fn trakt_section(self) -> &'static str {
        match self {
            ShowStatus::OnHold => "progress_watched",
            ShowStatus::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub status: ShowStatus,
    pub since: DateTime<Utc>,
    /// Basic show info from Trakt, so labelled shows that were never watched
    /// can still be listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show: Option<Show>,
}

/// Labels by Trakt show id. The device keeps a copy for offline use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Labels {
    shows: HashMap<u64, Label>,
}

impl Labels {
    /// Rebuilds labels from Trakt's hidden lists. A show in both counts as
    /// dropped. When Trakt has no `hidden_at`, the date from `previous` (or
    /// `now`) is used so the show isn't immediately un-hidden.
    pub fn from_trakt(
        on_hold: &[HiddenShow],
        dropped: &[HiddenShow],
        previous: &Labels,
        now: DateTime<Utc>,
    ) -> Self {
        let mut labels = Labels::default();
        let lists = [
            (ShowStatus::OnHold, on_hold),
            (ShowStatus::Dropped, dropped),
        ];
        for (status, list) in lists {
            for hidden in list {
                let Some(id) = hidden.show.as_ref().and_then(|s| s.ids.trakt) else {
                    continue;
                };
                let since = hidden
                    .hidden_at
                    .or_else(|| previous.get(id).map(|l| l.since))
                    .unwrap_or(now);
                let show = hidden.show.clone();
                labels.shows.insert(
                    id,
                    Label {
                        status,
                        since,
                        show,
                    },
                );
            }
        }
        labels
    }

    pub fn get(&self, show_id: u64) -> Option<&Label> {
        self.shows.get(&show_id)
    }

    pub fn status(&self, show_id: u64) -> Option<ShowStatus> {
        self.get(show_id).map(|l| l.status)
    }

    /// `None` puts the show back to watching.
    pub fn set(&mut self, show_id: u64, status: Option<ShowStatus>, now: DateTime<Utc>) {
        match status {
            Some(status) => {
                let show = self.shows.get(&show_id).and_then(|l| l.show.clone());
                self.shows.insert(
                    show_id,
                    Label {
                        status,
                        since: now,
                        show,
                    },
                )
            }
            None => self.shows.remove(&show_id),
        };
    }

    /// Clears labels of shows watched after they were labelled and returns
    /// what was cleared, so the caller can un-hide them on Trakt too.
    pub fn clear_rewatched(&mut self, items: &[ProgressItem]) -> Vec<(u64, ShowStatus)> {
        let mut cleared = Vec::new();
        for item in items {
            let (Some(id), Some(watched)) = (item.show.ids.trakt, item.last_watched_at) else {
                continue;
            };
            if let Some(label) = self.shows.get(&id)
                && watched > label.since
            {
                cleared.push((id, label.status));
                self.shows.remove(&id);
            }
        }
        cleared
    }

    pub fn iter(&self) -> impl Iterator<Item = (u64, &Label)> {
        self.shows.iter().map(|(id, label)| (*id, label))
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

    fn show(id: u64) -> Show {
        Show {
            ids: Ids {
                trakt: Some(id),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn item(id: u64, watched_days_ago: i64) -> ProgressItem {
        ProgressItem {
            show: show(id),
            aired: 10,
            completed: 5,
            last_watched_at: Some(now() - Duration::days(watched_days_ago)),
            next_episode: None,
            poster: None,
        }
    }

    fn hidden(id: u64, days_ago: Option<i64>) -> HiddenShow {
        HiddenShow {
            hidden_at: days_ago.map(|d| now() - Duration::days(d)),
            show: Some(show(id)),
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
        let cleared = labels.clear_rewatched(&[item(1, 1), item(2, 7)]);
        assert_eq!(cleared, [(1, ShowStatus::Dropped)]);
        assert_eq!(labels.status(1), None);
        assert_eq!(labels.status(2), Some(ShowStatus::OnHold));
        assert!(labels.clear_rewatched(&[item(2, 7)]).is_empty());
    }

    #[test]
    fn built_from_trakt_hidden_lists() {
        let mut previous = Labels::default();
        previous.set(3, Some(ShowStatus::OnHold), now() - Duration::days(9));

        let on_hold = [hidden(1, Some(5)), hidden(2, Some(5)), hidden(3, None)];
        let dropped = [hidden(2, Some(1))];
        let labels = Labels::from_trakt(&on_hold, &dropped, &previous, now());

        assert_eq!(labels.status(1), Some(ShowStatus::OnHold));
        // In both lists: dropped wins.
        assert_eq!(labels.status(2), Some(ShowStatus::Dropped));
        // No hidden_at from Trakt: keep the date we already had.
        assert_eq!(labels.get(3).unwrap().since, now() - Duration::days(9));
        assert_eq!(labels.status(4), None);
    }
}
