//! Refreshes the Progress screen from Trakt while keeping API calls low.
//!
//! Trakt allows 500 authenticated GETs per 5 minutes, and progress is one call
//! per show. A refresh therefore costs two calls (watched shows + calendar)
//! plus one per show that actually changed since it was last fetched.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::models::{CalendarEntry, Episode, ShowProgress, WatchedShow};
use crate::progress::ProgressItem;
use crate::{Error, Result, TraktClient};

const CONCURRENCY: usize = 6;
/// Refetch every show at least this often, in case something was missed.
const MAX_AGE_DAYS: i64 = 7;
/// Calendar window: the past week catches newly aired episodes, the rest
/// fills the Upcoming section.
const CALENDAR_PAST_DAYS: i64 = 7;
const CALENDAR_DAYS: u32 = 30;

/// Per-show progress from earlier refreshes. Persist it between launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressCache {
    shows: HashMap<u64, CachedProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProgress {
    /// `last_updated_at` of the watched entry when this was fetched.
    last_updated_at: Option<DateTime<Utc>>,
    fetched_at: DateTime<Utc>,
    progress: ShowProgress,
}

/// Fetches what changed and returns one item per watched show, with the next
/// upcoming episode filled in from the calendar for caught-up shows.
pub async fn refresh_progress(
    client: &TraktClient,
    cache: &mut ProgressCache,
    now: DateTime<Utc>,
) -> Result<Vec<ProgressItem>> {
    let watched = client.watched_shows().await?;
    let start = (now - Duration::days(CALENDAR_PAST_DAYS)).date_naive();
    let calendar = client.calendar(start, CALENDAR_DAYS).await?;

    let stale: Vec<u64> = watched
        .iter()
        .filter_map(|w| {
            let id = w.show.ids.trakt?;
            needs_fetch(cache.shows.get(&id), w, &calendar, now).then_some(id)
        })
        .collect();
    let updated_at: HashMap<u64, Option<DateTime<Utc>>> = watched
        .iter()
        .filter_map(|w| Some((w.show.ids.trakt?, w.last_updated_at)))
        .collect();

    let mut results = stream::iter(stale)
        .map(|id| async move { (id, client.show_progress(id).await) })
        .buffer_unordered(CONCURRENCY);
    while let Some((id, result)) = results.next().await {
        match result {
            Ok(progress) => {
                let entry = CachedProgress {
                    last_updated_at: updated_at.get(&id).copied().flatten(),
                    fetched_at: now,
                    progress,
                };
                cache.shows.insert(id, entry);
            }
            Err(Error::NotAuthenticated) => return Err(Error::NotAuthenticated),
            // Keep whatever was cached; a single failing show shouldn't
            // break the whole screen.
            Err(_) => {}
        }
    }

    let watched_ids: HashSet<u64> = updated_at.keys().copied().collect();
    cache.shows.retain(|id, _| watched_ids.contains(id));

    Ok(watched
        .into_iter()
        .filter_map(|w| {
            let id = w.show.ids.trakt?;
            let cached = cache.shows.get(&id)?;
            let mut item = ProgressItem::from_trakt(w, cached.progress.clone());
            if item.next_episode.is_none() {
                item.next_episode = next_airing(&calendar, id, now);
            }
            Some(item)
        })
        .collect())
}

fn needs_fetch(
    cached: Option<&CachedProgress>,
    watched: &WatchedShow,
    calendar: &[CalendarEntry],
    now: DateTime<Utc>,
) -> bool {
    let Some(cached) = cached else { return true };
    let show_id = watched.show.ids.trakt;
    let aired_since_fetch = calendar.iter().any(|e| {
        e.show.ids.trakt == show_id && e.first_aired > cached.fetched_at && e.first_aired <= now
    });
    cached.last_updated_at != watched.last_updated_at
        || aired_since_fetch
        || now - cached.fetched_at > Duration::days(MAX_AGE_DAYS)
}

/// The earliest not-yet-aired episode of a show in the calendar.
fn next_airing(calendar: &[CalendarEntry], show_id: u64, now: DateTime<Utc>) -> Option<Episode> {
    calendar
        .iter()
        .filter(|e| e.show.ids.trakt == Some(show_id) && e.first_aired > now)
        .min_by_key(|e| e.first_aired)
        .map(|e| Episode {
            first_aired: Some(e.first_aired),
            ..e.episode.clone()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Ids, Show};
    use chrono::TimeZone;

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

    fn watched(updated_days_ago: i64) -> WatchedShow {
        WatchedShow {
            plays: 1,
            last_watched_at: None,
            last_updated_at: Some(now() - Duration::days(updated_days_ago)),
            reset_at: None,
            show: show(1),
        }
    }

    fn cached(fetched_days_ago: i64, updated_days_ago: i64) -> CachedProgress {
        CachedProgress {
            last_updated_at: Some(now() - Duration::days(updated_days_ago)),
            fetched_at: now() - Duration::days(fetched_days_ago),
            progress: ShowProgress::default(),
        }
    }

    fn airing(show_id: u64, days_from_now: i64) -> CalendarEntry {
        CalendarEntry {
            first_aired: now() + Duration::days(days_from_now),
            episode: Episode {
                season: 2,
                number: (days_from_now + 10) as u32,
                ..Default::default()
            },
            show: show(show_id),
        }
    }

    #[test]
    fn fetches_when_not_cached_or_changed() {
        assert!(needs_fetch(None, &watched(3), &[], now()));
        assert!(!needs_fetch(Some(&cached(1, 3)), &watched(3), &[], now()));
        assert!(needs_fetch(Some(&cached(1, 3)), &watched(0), &[], now()));
    }

    #[test]
    fn fetches_when_an_episode_aired_since_last_fetch() {
        let cal = [airing(1, -1)];
        assert!(needs_fetch(Some(&cached(2, 3)), &watched(3), &cal, now()));
        // Aired before the fetch: already counted.
        assert!(!needs_fetch(Some(&cached(0, 3)), &watched(3), &cal, now()));
        // Another show's episode doesn't matter.
        assert!(!needs_fetch(
            Some(&cached(2, 3)),
            &watched(3),
            &[airing(9, -1)],
            now()
        ));
    }

    #[test]
    fn fetches_when_cache_is_old() {
        assert!(needs_fetch(Some(&cached(8, 30)), &watched(30), &[], now()));
    }

    #[test]
    fn next_airing_picks_earliest_future_episode() {
        let cal = [airing(1, 5), airing(1, -2), airing(1, 2), airing(2, 1)];
        let next = next_airing(&cal, 1, now()).unwrap();
        assert_eq!(next.first_aired, Some(now() + Duration::days(2)));
        assert!(next_airing(&cal, 3, now()).is_none());
    }
}
