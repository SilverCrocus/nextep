# Roadmap

Scope comes from a walkthrough of Showly 3.72.0 on Android (October 2026):
what gets used daily stays, the rest is cut or deferred.

## v0.1: daily driver

- [ ] **Trakt sign-in** with the device-code flow (enter a code at trakt.tv/activate)
- [ ] **Progress**: shows with an episode to watch next
  - [ ] One-tap ✓ to mark the next episode watched (synced to Trakt)
  - [ ] Sections: *New episodes*, *Watching*, *Upcoming*
  - [ ] Sort: recently watched, title, episodes left, newest episode
  - [ ] Pull to refresh; background refresh on open
- [ ] **Calendar**: upcoming episodes for followed shows, grouped by day
- [ ] **Show page**: poster, overview, network, status, Trakt/IMDb rating,
      seasons with watched progress, mark episodes or seasons watched
- [ ] **Library**: My Shows and Watchlist
- [ ] **Search** shows and add to watchlist
- [ ] Local cache (SQLite) so the app opens instantly and works offline
- [ ] Artwork from TMDB

## Later

- Movies (watchlist and watched)
- Trakt lists (including third-party lists such as couchmoney and mdblist)
- New-episode notifications
- Home-screen widget
- Statistics (time watched, top genres)
- Where to stream (JustWatch data via TMDB)
- "On hold" / pin shows in Progress
- History view

## Cut

Present in Showly, intentionally left out:

- News feed
- Comments, trailers, "You may also like", cast and crew pages
- Metascore and Rotten Tomatoes links
- Premium tier, subscriptions, analytics, error reporting, social links
- Spoiler settings, widget themes, date-format and most other settings
  (target: fewer than 10 settings)

## Progress ordering

Implemented in [`nextep-core/src/progress.rs`](../crates/nextep-core/src/progress.rs):

1. Shows with no next episode (caught up on an ended show) are hidden.
2. **New episodes**: next episode aired in the last 7 days, newest first
   (can be turned off).
3. **Watching**: everything else that is out, ordered by the chosen sort.
   Default is most recently watched.
4. **Upcoming**: caught up and waiting, soonest air date first; unknown dates last.
