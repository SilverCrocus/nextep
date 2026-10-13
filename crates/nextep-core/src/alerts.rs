//! New-episode alerts: which upcoming episodes get a notification, and when.
//!
//! Only shows the user is caught up on (nothing aired left to watch, the
//! same shows as Progress's Upcoming) and hasn't labelled on hold or
//! dropped. Each alert fires when its episode airs. The app schedules them
//! with the OS whenever Progress refreshes, so they keep working while it's
//! closed for up to the calendar's window.

use chrono::{DateTime, Utc};

use crate::labels::Labels;
use crate::models::CalendarEntry;
use crate::progress::ProgressItem;

/// Android allows an app 500 alarms; a month of episodes fits well within this.
const MAX_ALERTS: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// Stable per episode, so rescheduling replaces rather than duplicates.
    pub id: i32,
    pub at: DateTime<Utc>,
    pub title: String,
    pub body: String,
}

/// Alerts for the calendar's episodes that air after `now`, soonest first.
pub fn plan(
    items: &[ProgressItem],
    calendar: &[CalendarEntry],
    labels: &Labels,
    now: DateTime<Utc>,
) -> Vec<Alert> {
    let caught_up = |show_id: u64| {
        items
            .iter()
            .any(|i| i.show.ids.trakt == Some(show_id) && i.episodes_left() == 0)
    };
    let mut alerts: Vec<Alert> = calendar
        .iter()
        .filter(|e| e.first_aired > now)
        .filter_map(|e| {
            let show_id = e.show.ids.trakt?;
            let episode_id = e.episode.ids.trakt?;
            if labels.is_hidden(Some(show_id)) || !caught_up(show_id) {
                return None;
            }
            let body = match e.episode.title.as_deref().filter(|t| !t.is_empty()) {
                Some(title) => format!("New episode: {} · {title}", e.episode.code()),
                None => format!("New episode: {}", e.episode.code()),
            };
            Some(Alert {
                id: (episode_id % i32::MAX as u64) as i32,
                at: e.first_aired,
                title: e.show.display_title().to_owned(),
                body,
            })
        })
        .collect();
    alerts.sort_by_key(|a| a.at);
    alerts.dedup_by_key(|a| a.id);
    alerts.truncate(MAX_ALERTS);
    alerts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::labels::ShowStatus;
    use crate::models::{Episode, Ids, Show};
    use chrono::{Duration, TimeZone};

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
    }

    fn show(id: u64, title: &str) -> Show {
        Show {
            title: Some(title.into()),
            ids: Ids {
                trakt: Some(id),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn item(id: u64, aired: u32, completed: u32) -> ProgressItem {
        ProgressItem {
            show: show(id, "Show"),
            aired,
            completed,
            last_watched_at: None,
            next_episode: None,
            poster: None,
        }
    }

    fn airing(show_id: u64, episode_id: u64, hours: i64, title: Option<&str>) -> CalendarEntry {
        CalendarEntry {
            first_aired: now() + Duration::hours(hours),
            episode: Episode {
                season: 1,
                number: 4,
                title: title.map(Into::into),
                ids: Ids {
                    trakt: Some(episode_id),
                    ..Default::default()
                },
                ..Default::default()
            },
            show: show(show_id, "Avatar: Seven Havens"),
        }
    }

    #[test]
    fn alerts_for_caught_up_shows_only_soonest_first() {
        let items = [item(1, 3, 3), item(2, 5, 4), item(3, 2, 2)];
        let calendar = [
            airing(1, 100, 48, Some("The Faceless")),
            airing(2, 200, 24, None), // behind on this show
            airing(3, 300, 12, None),
            airing(1, 99, -2, None), // already aired
            airing(9, 900, 6, None), // not a watched show
        ];
        let alerts = plan(&items, &calendar, &Labels::default(), now());
        let ids: Vec<i32> = alerts.iter().map(|a| a.id).collect();
        assert_eq!(ids, [300, 100]);
        assert_eq!(alerts[1].title, "Avatar: Seven Havens");
        assert_eq!(alerts[1].body, "New episode: S01E04 · The Faceless");
        assert_eq!(alerts[0].body, "New episode: S01E04");
        assert_eq!(alerts[1].at, now() + Duration::hours(48));
    }

    #[test]
    fn no_alerts_for_on_hold_or_dropped_shows() {
        let mut labels = Labels::default();
        labels.set(1, Some(ShowStatus::OnHold), now());
        let alerts = plan(&[item(1, 3, 3)], &[airing(1, 100, 5, None)], &labels, now());
        assert!(alerts.is_empty());
    }
}
