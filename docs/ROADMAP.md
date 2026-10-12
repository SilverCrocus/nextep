# Roadmap

Scope comes from a walkthrough of Showly 3.72.0 on Android (October 2026):
what gets used daily stays, the rest is cut or deferred.

## v0.1: daily driver

- [x] **Trakt sign-in** with the device-code flow (enter a code at auth.trakt.tv/activate)
- [x] **Progress**: shows with an episode to watch next
  - [x] One-tap ✓ to mark the next episode watched (synced to Trakt)
  - [x] Sections: *New episodes*, *Upcoming*, *Watching*
  - [x] Sort: recently watched, title, episodes left, newest episode
  - [x] Pull to refresh
- [x] **On hold / Dropped** labels: hide a show from Progress and Calendar
      without touching Trakt history; watching it again clears the label
- [x] **Calendar**: upcoming episodes for followed shows, grouped by day
- [x] **Show page**: artwork, overview, network, status, Trakt rating,
      seasons with watched progress, mark individual episodes watched
- [x] **Library**: Watching, Up to date, On hold, Dropped, Finished, Watchlist
- [x] Artwork from TMDB (cached)
- [x] **Search** shows and add to watchlist
- [x] On hold / Dropped saved on Trakt (hidden from progress / dropped)
- [x] Updates: signed releases on GitHub, Obtainium support, in-app banner
- [x] Opens instantly and works offline (last good result of each screen is saved)

## Next

- [x] **Lists** on top of Trakt lists (they sync to Stremio's Trakt add-on):
      create, rename, delete, add and remove shows; couchmoney / mdblist
      lists shown read-only as recommendations; "5 of 12 lists" counter
- [ ] **Discovery** page (Discover tab)
  - [x] "Because you watched …" rows (Trakt related shows for the last three)
  - [x] "Recommended by Trakt" (personalised recommendations)
  - [x] Trending
  - [x] "Would I like it?" model: a small on-device decision model trained on
        your own history (finished / watching = liked, dropped / on hold =
        not), scoring candidates by genre, network, rating, year and length
  - [x] Optional AI picks: describe a mood and an OpenAI-compatible API
        (DeepSeek by default, deepseek-flash with adjustable thinking) picks
        and explains real candidates from the rows above in a single
        request that carries your history (recent shows, taste profile,
        dropped shows) and the cast & crew of shows you liked.
        Bring-your-own key, stored only on the device
  - [x] Shows | Movies switch: the same rows and AI picks for movies
        (related to your last three movies, Trakt's movie picks, trending),
        scored by the taste model on the genres, decade and rating movies
        share with shows
- [x] In-app updates: the update banner downloads the release and opens
      Android's installer (no browser or Obtainium needed)
- [x] **Movies**: Shows | Movies switch on Progress (to watch / coming soon)
      and Library (watched / watchlist), movie pages (mark watched,
      watchlist, where to watch, director and cast), movies in Search, and
      watchlist releases in Calendar
- [ ] Movies in Lists
- [ ] New-episode notifications
- [ ] Home-screen widget

## Done from "Later"

- [x] Where to stream (JustWatch data via TMDB), on the show page
- [x] History view (Calendar → History)
- [x] Statistics (time watched, top genres), at the bottom of Library

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
