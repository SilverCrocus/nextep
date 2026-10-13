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
/// Pull to refresh refetches every show watched this recently...
const FORCE_DAYS: i64 = 30;
/// ...unless it was fetched this recently.
const FORCE_COOLDOWN_MINUTES: i64 = 5;
/// Progress fetched this soon after a change on Trakt isn't trusted (Trakt
/// can still answer from before it); it's fetched again after.
const SETTLE_MINUTES: i64 = 10;
/// Calendar window: the past week catches newly aired episodes, the rest
/// fills the Upcoming section.
const CALENDAR_PAST_DAYS: i64 = 7;
const CALENDAR_DAYS: u32 = 30;

/// Per-show progress from earlier refreshes. Persist it between launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressCache {
    shows: HashMap<u64, CachedProgress>,
}

impl ProgressCache {
    /// Trakt ids of every show with cached progress (i.e. watched shows).
    pub fn show_ids(&self) -> impl Iterator<Item = u64> + '_ {
        self.shows.keys().copied()
    }

    /// Makes the next refresh fetch a show's progress again. The old progress
    /// stays until then, so the show doesn't vanish if that fetch fails.
    pub fn mark_stale(&mut self, show_id: u64) {
        if let Some(entry) = self.shows.get_mut(&show_id) {
            entry.plays = None;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedProgress {
    /// The watched entry when this was fetched: if any of these change, the
    /// progress has. `last_updated_at` alone missed episodes added to a show.
    last_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    last_watched_at: Option<DateTime<Utc>>,
    /// `None` in caches from before it was kept, so those are fetched again.
    #[serde(default)]
    plays: Option<u32>,
    fetched_at: DateTime<Utc>,
    progress: ShowProgress,
}

/// Fetches what changed and returns one item per watched show, with the next
/// upcoming episode filled in from the calendar for caught-up shows, and the
/// calendar itself (the past week and the next three).
pub async fn refresh_progress(
    client: &TraktClient,
    cache: &mut ProgressCache,
    now: DateTime<Utc>,
    force: bool,
) -> Result<(Vec<ProgressItem>, Vec<CalendarEntry>)> {
    let watched = client.watched_shows().await?;
    let start = (now - Duration::days(CALENDAR_PAST_DAYS)).date_naive();
    let calendar = client.calendar(start, CALENDAR_DAYS).await?;

    let stale: Vec<u64> = watched
        .iter()
        .filter_map(|w| {
            let id = w.show.ids.trakt?;
            needs_fetch(cache.shows.get(&id), w, &calendar, now, force).then_some(id)
        })
        .collect();
    let entries: HashMap<u64, &WatchedShow> = watched
        .iter()
        .filter_map(|w| Some((w.show.ids.trakt?, w)))
        .collect();

    let mut results = stream::iter(stale)
        .map(|id| async move { (id, client.show_progress(id).await) })
        .buffer_unordered(CONCURRENCY);
    while let Some((id, result)) = results.next().await {
        match result {
            Ok(mut progress) => {
                // Per-episode detail is only needed on the show page.
                progress.seasons.clear();
                let watched = entries.get(&id);
                let entry = CachedProgress {
                    last_updated_at: watched.and_then(|w| w.last_updated_at),
                    last_watched_at: watched.and_then(|w| w.last_watched_at),
                    plays: watched.map(|w| w.plays),
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

    let watched_ids: HashSet<u64> = entries.keys().copied().collect();
    cache.shows.retain(|id, _| watched_ids.contains(id));

    let items = watched
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
        .collect();
    Ok((items, calendar))
}

/// Whether a show's progress must be fetched again. `force` (pull to
/// refresh) refetches every show watched in the last [`FORCE_DAYS`].
fn needs_fetch(
    cached: Option<&CachedProgress>,
    watched: &WatchedShow,
    calendar: &[CalendarEntry],
    now: DateTime<Utc>,
    force: bool,
) -> bool {
    let Some(cached) = cached else { return true };
    let show_id = watched.show.ids.trakt;
    let aired_since_fetch = calendar.iter().any(|e| {
        e.show.ids.trakt == show_id && e.first_aired > cached.fetched_at && e.first_aired <= now
    });
    // Right after episodes are marked, Trakt's watched list can already have
    // them while the progress call still answers from before, which left a
    // show stuck a few episodes behind. So progress fetched within minutes of
    // the change is fetched again later...
    let settling = watched
        .last_updated_at
        .is_some_and(|at| cached.fetched_at - at < Duration::minutes(SETTLE_MINUTES));
    // ...and progress that ends before the watched list's last watch is
    // fetched again, for a day after that watch: progress leaves out
    // specials, so a special watched last never catches up.
    let lagging = cached.progress.last_watched_at < watched.last_watched_at
        && watched
            .last_watched_at
            .is_some_and(|at| cached.fetched_at - at < Duration::days(1));
    // Watched or changed in the last month (a watch dated in the past still
    // changes `last_updated_at`).
    let recent = force
        && now - cached.fetched_at > Duration::minutes(FORCE_COOLDOWN_MINUTES)
        && watched
            .last_updated_at
            .max(watched.last_watched_at)
            .is_some_and(|at| now - at < Duration::days(FORCE_DAYS));
    let entry_changed = cached.last_updated_at != watched.last_updated_at
        || cached.last_watched_at != watched.last_watched_at
        || cached.plays != Some(watched.plays);
    entry_changed
        || aired_since_fetch
        || settling
        || lagging
        || recent
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
            last_watched_at: None,
            plays: Some(1),
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
        assert!(needs_fetch(None, &watched(3), &[], now(), false));
        assert!(!needs_fetch(
            Some(&cached(1, 3)),
            &watched(3),
            &[],
            now(),
            false
        ));
        assert!(needs_fetch(
            Some(&cached(1, 3)),
            &watched(0),
            &[],
            now(),
            false
        ));
    }

    #[test]
    fn fetches_when_plays_or_last_watch_change_without_last_updated_at() {
        let more_plays = WatchedShow {
            plays: 3,
            ..watched(3)
        };
        assert!(needs_fetch(
            Some(&cached(1, 3)),
            &more_plays,
            &[],
            now(),
            false
        ));
        let watched_again = WatchedShow {
            last_watched_at: Some(now()),
            ..watched(3)
        };
        assert!(needs_fetch(
            Some(&cached(1, 3)),
            &watched_again,
            &[],
            now(),
            false
        ));
        // Caches from before plays were kept are fetched once.
        let mut old = cached(1, 3);
        old.plays = None;
        assert!(needs_fetch(Some(&old), &watched(3), &[], now(), false));
    }

    #[test]
    fn fetches_when_an_episode_aired_since_last_fetch() {
        let cal = [airing(1, -1)];
        assert!(needs_fetch(
            Some(&cached(2, 3)),
            &watched(3),
            &cal,
            now(),
            false
        ));
        // Aired before the fetch: already counted.
        assert!(!needs_fetch(
            Some(&cached(0, 3)),
            &watched(3),
            &cal,
            now(),
            false
        ));
        // Another show's episode doesn't matter.
        assert!(!needs_fetch(
            Some(&cached(2, 3)),
            &watched(3),
            &[airing(9, -1)],
            now(),
            false
        ));
    }

    #[test]
    fn fetches_again_while_progress_lags_the_watched_list() {
        let watched_at = now() - Duration::hours(3);
        let w = WatchedShow {
            last_watched_at: Some(watched_at),
            last_updated_at: Some(watched_at),
            ..watched(0)
        };
        // Fetched after the watch, but the progress still ends earlier.
        let mut stale = cached(0, 0);
        stale.last_updated_at = w.last_updated_at;
        stale.last_watched_at = w.last_watched_at;
        stale.fetched_at = watched_at + Duration::minutes(30);
        stale.progress.last_watched_at = Some(watched_at - Duration::days(2));
        assert!(needs_fetch(Some(&stale), &w, &[], now(), false));

        // Caught up.
        let mut fresh = stale.clone();
        fresh.progress.last_watched_at = Some(watched_at);
        assert!(!needs_fetch(Some(&fresh), &w, &[], now(), false));

        // Still behind a day later (a special watched last): leave it.
        let mut special = stale.clone();
        special.fetched_at = watched_at + Duration::days(2);
        assert!(!needs_fetch(
            Some(&special),
            &w,
            &[],
            watched_at + Duration::days(2),
            false
        ));
    }

    #[test]
    fn fetches_again_when_fetched_right_after_a_change() {
        let changed = now() - Duration::hours(1);
        let w = WatchedShow {
            last_updated_at: Some(changed),
            ..watched(0)
        };
        let mut c = cached(0, 0);
        c.last_updated_at = w.last_updated_at;
        c.fetched_at = changed + Duration::minutes(2);
        assert!(needs_fetch(Some(&c), &w, &[], now(), false));
        c.fetched_at = changed + Duration::minutes(20);
        assert!(!needs_fetch(Some(&c), &w, &[], now(), false));
    }

    #[test]
    fn marked_stale_shows_are_fetched_but_kept() {
        let mut cache = ProgressCache::default();
        cache.shows.insert(1, cached(1, 3));
        cache.mark_stale(1);
        assert!(needs_fetch(
            cache.shows.get(&1),
            &watched(3),
            &[],
            now(),
            false
        ));
        assert_eq!(cache.show_ids().count(), 1);
    }

    #[test]
    fn pull_to_refresh_refetches_recently_watched_shows() {
        let recent = WatchedShow {
            last_watched_at: Some(now() - Duration::days(3)),
            ..watched(3)
        };
        let mut up_to_date = cached(1, 3);
        up_to_date.last_watched_at = recent.last_watched_at;
        up_to_date.progress.last_watched_at = recent.last_watched_at;
        assert!(!needs_fetch(Some(&up_to_date), &recent, &[], now(), false));
        assert!(needs_fetch(Some(&up_to_date), &recent, &[], now(), true));
        // Not again within a few minutes.
        up_to_date.fetched_at = now() - Duration::minutes(1);
        assert!(!needs_fetch(Some(&up_to_date), &recent, &[], now(), true));

        let old = WatchedShow {
            last_watched_at: Some(now() - Duration::days(90)),
            ..watched(90)
        };
        let mut old_cached = cached(1, 90);
        old_cached.last_watched_at = old.last_watched_at;
        old_cached.progress.last_watched_at = old.last_watched_at;
        assert!(!needs_fetch(Some(&old_cached), &old, &[], now(), true));
    }

    #[test]
    fn fetches_when_cache_is_old() {
        assert!(needs_fetch(
            Some(&cached(8, 30)),
            &watched(30),
            &[],
            now(),
            false
        ));
    }

    #[test]
    fn next_airing_picks_earliest_future_episode() {
        let cal = [airing(1, 5), airing(1, -2), airing(1, 2), airing(2, 1)];
        let next = next_airing(&cal, 1, now()).unwrap();
        assert_eq!(next.first_aired, Some(now() + Duration::days(2)));
        assert!(next_airing(&cal, 3, now()).is_none());
    }
}
