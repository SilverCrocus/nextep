# Roadmap

Scope comes from a walkthrough of Showly 3.72.0 on Android (October 2026):
what gets used daily stays, the rest is cut or deferred.

## v0.1: daily driver

- [x] **Trakt sign-in** with the device-code flow (enter a code at auth.trakt.tv/activate)
- [x] **Progress**: shows with an episode to watch next
  - [x] One-tap ✓ to mark the next episode watched (synced to Trakt)
  - [x] Sections: *New episodes*, *Upcoming*, *Watching*
  - [x] Sort: recently watched, title, episodes left, newest episode
  - [ ] Pull to refresh (a Refresh button exists)
- [x] **On hold / Dropped** labels: hide a show from Progress and Calendar
      without touching Trakt history; watching it again clears the label
- [x] **Calendar**: upcoming episodes for followed shows, grouped by day
- [x] **Show page**: artwork, overview, network, status, Trakt rating,
      seasons with watched progress, mark individual episodes watched
- [x] **Library**: Watching, Up to date, On hold, Dropped, Finished, Watchlist
- [x] Artwork from TMDB (cached)
- [ ] **Search** shows and add to watchlist
- [ ] Sync On hold / Dropped to Trakt so they survive a reinstall
- [ ] Local cache (SQLite) so the app opens instantly and works offline

## Later

- Movies (watchlist and watched)
- Trakt lists (including third-party lists such as couchmoney and mdblist)
- New-episode notifications
- Home-screen widget
- Statistics (time watched, top genres)
- Where to stream (JustWatch data via TMDB)
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

1. Shows on hold or dropped are hidden (see
   [`labels.rs`](../crates/nextep-core/src/labels.rs)).
2. Shows with no next episode (fully watched) are hidden.
3. **New episodes**: next episode aired in the last 7 days, newest first.
4. **Upcoming**: only shows you are caught up on whose next episode airs in
   the next 30 days, soonest first. Shows you are behind on never appear here,
   and episodes without an air date are left out.
5. **Watching**: everything else with an aired episode to watch, ordered by
   the chosen sort. Default is most recently watched.

The next episode follows the last episode you watched (Trakt's
`last_activity=watched`), like Showly's "last watched episode" setting.
