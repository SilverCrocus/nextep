//! Builds the "Progress" screen: which shows have an episode to watch next and
//! in what order.
//!
//! Kept free of I/O so the ordering rules are easy to test and change.

use std::cmp::{Ordering, Reverse};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{Episode, Show, ShowProgress, WatchedShow};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressItem {
    pub show: Show,
    pub aired: u32,
    pub completed: u32,
    pub last_watched_at: Option<DateTime<Utc>>,
    pub next_episode: Option<Episode>,
}

impl ProgressItem {
    pub fn from_trakt(watched: WatchedShow, progress: ShowProgress) -> Self {
        Self {
            show: watched.show,
            aired: progress.aired,
            completed: progress.completed,
            last_watched_at: progress.last_watched_at.or(watched.last_watched_at),
            next_episode: progress.next_episode,
        }
    }

    pub fn episodes_left(&self) -> u32 {
        self.aired.saturating_sub(self.completed)
    }

    fn next_airs_at(&self) -> Option<DateTime<Utc>> {
        self.next_episode.as_ref().and_then(|e| e.first_aired)
    }

    /// The next episode exists but has not aired yet (or has no air date).
    fn is_upcoming(&self, now: DateTime<Utc>) -> bool {
        self.next_episode.is_some() && self.next_airs_at().is_none_or(|at| at > now)
    }

    /// The next episode aired within `window`.
    fn is_new(&self, now: DateTime<Utc>, window: Duration) -> bool {
        self.next_airs_at()
            .is_some_and(|at| at <= now && now - at <= window)
    }

    /// Sort key for titles: case-insensitive and ignoring a leading "The ".
    fn title_key(&self) -> String {
        let title = self.show.display_title().to_lowercase();
        match title.strip_prefix("the ") {
            Some(rest) => rest.to_owned(),
            None => title,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    /// Most recently watched show first.
    #[default]
    RecentlyWatched,
    /// A to Z, ignoring "The".
    Title,
    /// Closest to caught up first.
    EpisodesLeft,
    /// Most recently aired next episode first.
    NewestEpisode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressOptions {
    pub sort: SortBy,
    /// Pull shows whose next episode just aired into their own section on top.
    pub new_episodes_first: bool,
    /// How long after airing an episode counts as "new".
    pub new_window_days: i64,
}

impl Default for ProgressOptions {
    fn default() -> Self {
        Self {
            sort: SortBy::RecentlyWatched,
            new_episodes_first: true,
            new_window_days: 7,
        }
    }
}

/// The Progress screen, split into sections in display order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProgressView {
    /// Next episode aired recently (only when `new_episodes_first` is on).
    pub new_episodes: Vec<ProgressItem>,
    /// Next episode is out; ordered by the chosen sort.
    pub watching: Vec<ProgressItem>,
    /// Caught up, next episode not aired yet; soonest first.
    pub upcoming: Vec<ProgressItem>,
}

/// Splits and orders progress items. Shows with no next episode (fully watched
/// and ended, or no episodes) are dropped.
pub fn arrange(
    items: Vec<ProgressItem>,
    opts: &ProgressOptions,
    now: DateTime<Utc>,
) -> ProgressView {
    let window = Duration::days(opts.new_window_days);
    let mut view = ProgressView::default();

    for item in items.into_iter().filter(|i| i.next_episode.is_some()) {
        if item.is_upcoming(now) {
            view.upcoming.push(item);
        } else if opts.new_episodes_first && item.is_new(now, window) {
            view.new_episodes.push(item);
        } else {
            view.watching.push(item);
        }
    }

    view.new_episodes.sort_by(|a, b| {
        b.next_airs_at()
            .cmp(&a.next_airs_at())
            .then_with(|| by_title(a, b))
    });
    view.watching.sort_by(|a, b| compare(opts.sort, a, b));
    // Unknown air dates go last.
    view.upcoming
        .sort_by_key(|i| (i.next_airs_at().is_none(), i.next_airs_at(), i.title_key()));
    view
}

fn compare(sort: SortBy, a: &ProgressItem, b: &ProgressItem) -> Ordering {
    let primary = match sort {
        // `Reverse(None)` sorts after any date, so never-watched shows go last.
        SortBy::RecentlyWatched => Reverse(a.last_watched_at).cmp(&Reverse(b.last_watched_at)),
        SortBy::Title => Ordering::Equal,
        SortBy::EpisodesLeft => a.episodes_left().cmp(&b.episodes_left()),
        SortBy::NewestEpisode => Reverse(a.next_airs_at()).cmp(&Reverse(b.next_airs_at())),
    };
    primary.then_with(|| by_title(a, b))
}

fn by_title(a: &ProgressItem, b: &ProgressItem) -> Ordering {
    a.title_key().cmp(&b.title_key())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 12, 12, 0, 0).unwrap()
    }

    fn item(
        title: &str,
        left: u32,
        watched_days_ago: Option<i64>,
        airs_in_days: Option<i64>,
    ) -> ProgressItem {
        ProgressItem {
            show: Show {
                title: Some(title.into()),
                ..Default::default()
            },
            aired: 10 + left,
            completed: 10,
            last_watched_at: watched_days_ago.map(|d| now() - Duration::days(d)),
            next_episode: Some(Episode {
                season: 1,
                number: 11,
                first_aired: airs_in_days.map(|d| now() + Duration::days(d)),
                ..Default::default()
            }),
        }
    }

    fn titles(items: &[ProgressItem]) -> Vec<&str> {
        items.iter().map(|i| i.show.display_title()).collect()
    }

    #[test]
    fn recently_watched_first_and_never_watched_last() {
        let items = vec![
            item("Old", 3, Some(30), Some(-100)),
            item("Never", 3, None, Some(-100)),
            item("Recent", 3, Some(1), Some(-100)),
        ];
        let view = arrange(items, &ProgressOptions::default(), now());
        assert_eq!(titles(&view.watching), ["Recent", "Old", "Never"]);
    }

    #[test]
    fn new_episodes_get_their_own_section() {
        let items = vec![
            item("Backlog", 20, Some(1), Some(-300)),
            item("Fresh", 1, Some(9), Some(-2)),
        ];
        let view = arrange(items.clone(), &ProgressOptions::default(), now());
        assert_eq!(titles(&view.new_episodes), ["Fresh"]);
        assert_eq!(titles(&view.watching), ["Backlog"]);

        let off = ProgressOptions {
            new_episodes_first: false,
            ..Default::default()
        };
        let view = arrange(items, &off, now());
        assert!(view.new_episodes.is_empty());
        assert_eq!(titles(&view.watching), ["Backlog", "Fresh"]);
    }

    #[test]
    fn upcoming_sorted_by_air_date_with_unknown_last() {
        let items = vec![
            item("TBA", 0, Some(1), None),
            item("Next week", 0, Some(1), Some(7)),
            item("Tomorrow", 0, Some(1), Some(1)),
        ];
        let view = arrange(items, &ProgressOptions::default(), now());
        assert_eq!(titles(&view.upcoming), ["Tomorrow", "Next week", "TBA"]);
    }

    #[test]
    fn title_sort_ignores_leading_the() {
        let items = vec![
            item("Wizards", 1, None, Some(-100)),
            item("The Legend of Vox Machina", 1, None, Some(-100)),
            item("Arrested Development", 1, None, Some(-100)),
        ];
        let opts = ProgressOptions {
            sort: SortBy::Title,
            ..Default::default()
        };
        let view = arrange(items, &opts, now());
        assert_eq!(
            titles(&view.watching),
            [
                "Arrested Development",
                "The Legend of Vox Machina",
                "Wizards"
            ]
        );
    }

    #[test]
    fn caught_up_shows_without_next_episode_are_dropped() {
        let mut done = item("Done", 0, Some(1), None);
        done.next_episode = None;
        let view = arrange(vec![done], &ProgressOptions::default(), now());
        assert_eq!(view, ProgressView::default());
    }
}
