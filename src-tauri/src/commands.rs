//! Commands the UI calls through `invoke`. Tauri maps camelCase arguments
//! from JavaScript to the snake_case parameters here.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use nextep_core::ai;
use nextep_core::ai_tools::ToolBox;
use nextep_core::chrono::{DateTime, Duration, Utc};
use nextep_core::labels::ShowStatus;
use nextep_core::models::{Episode, Show, TraktList};
use nextep_core::progress::{
    self, ProgressItem, ProgressOptions, ProgressView, SortBy, title_sort_key,
};
use nextep_core::taste::{self, TasteModel};
use nextep_core::tmdb::{Media, Provider, Size};
use nextep_core::trakt::{DeviceCode, DevicePoll};
use nextep_core::update::{self, Release};
use nextep_core::{Error, TraktClient};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;

use crate::state::{AppState, to_ui_error};
use crate::{TMDB_API_KEY, TRAKT_CLIENT_ID};

// ---- status & sign-in -------------------------------------------------------

#[derive(Serialize)]
pub struct AppStatus {
    version: &'static str,
    trakt_configured: bool,
    tmdb_configured: bool,
    signed_in: bool,
}

#[tauri::command]
pub async fn app_status(state: State<'_, AppState>) -> Result<AppStatus, String> {
    Ok(AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        trakt_configured: TRAKT_CLIENT_ID.is_some(),
        tmdb_configured: TMDB_API_KEY.is_some(),
        signed_in: state.trakt.lock().await.token().is_some(),
    })
}

/// Starts the device flow: the UI shows `user_code` and opens `verification_url`.
#[tauri::command]
pub async fn sign_in_start(state: State<'_, AppState>) -> Result<DeviceCode, String> {
    let client = state.trakt.lock().await.clone();
    client.device_code().await.map_err(to_ui_error)
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignInPoll {
    Pending,
    SlowDown,
    SignedIn,
    Denied,
    Expired,
}

#[tauri::command]
pub async fn sign_in_poll(
    state: State<'_, AppState>,
    device_code: String,
) -> Result<SignInPoll, String> {
    let client = state.trakt.lock().await.clone();
    Ok(
        match client
            .poll_device_token(&device_code)
            .await
            .map_err(to_ui_error)?
        {
            DevicePoll::Authorized(token) => {
                state.save_token(&token)?;
                state.trakt.lock().await.set_token(Some(token));
                SignInPoll::SignedIn
            }
            DevicePoll::Pending => SignInPoll::Pending,
            DevicePoll::SlowDown => SignInPoll::SlowDown,
            DevicePoll::Denied => SignInPoll::Denied,
            DevicePoll::Expired | DevicePoll::AlreadyUsed | DevicePoll::InvalidCode => {
                SignInPoll::Expired
            }
        },
    )
}

#[tauri::command]
pub async fn sign_out(state: State<'_, AppState>) -> Result<(), String> {
    state.sign_out().await;
    Ok(())
}

/// Opens a link in the system browser. Limited to Trakt and Nextep's own
/// GitHub releases so the UI can't be used to open arbitrary links.
#[tauri::command]
pub fn open_link(app: AppHandle, url: String) -> Result<(), String> {
    const ALLOWED: [&str; 3] = [
        "https://auth.trakt.tv/",
        "https://trakt.tv/",
        "https://github.com/SilverCrocus/nextep/releases/",
    ];
    if !ALLOWED.iter().any(|prefix| url.starts_with(prefix)) {
        return Err("this link can't be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// A newer release on GitHub, if there is one.
#[tauri::command]
pub async fn check_update() -> Result<Option<Release>, String> {
    update::check(env!("CARGO_PKG_VERSION"))
        .await
        .map_err(to_ui_error)
}

// ---- progress ---------------------------------------------------------------

/// `cached` returns the last saved result instantly without calling Trakt.
#[tauri::command]
pub async fn progress(
    state: State<'_, AppState>,
    sort: Option<SortBy>,
    force: Option<bool>,
    cached: Option<bool>,
) -> Result<ProgressView, String> {
    let mut items: Vec<ProgressItem> = if cached.unwrap_or(false) {
        state.snapshot("items")?
    } else {
        state.items(force.unwrap_or(false)).await?
    };
    let labels = state.labels.lock().await;
    items.retain(|i| !labels.is_hidden(i.show.ids.trakt));
    let opts = ProgressOptions {
        sort: sort.unwrap_or_default(),
        ..Default::default()
    };
    Ok(progress::arrange(items, &opts, Utc::now()))
}

#[tauri::command]
pub async fn mark_watched(state: State<'_, AppState>, episode_id: u64) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    client
        .mark_episode_watched(episode_id, Utc::now())
        .await
        .map_err(to_ui_error)?;
    state.invalidate().await;
    Ok(())
}

/// Saves the label on Trakt (see `nextep_core::labels`). `None` puts the
/// show back to watching.
#[tauri::command]
pub async fn set_show_status(
    state: State<'_, AppState>,
    show_id: u64,
    status: Option<ShowStatus>,
) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    let mut labels = state.labels.lock().await;
    let current = labels.status(show_id);
    if current == status {
        return Ok(());
    }
    if let Some(current) = current {
        client
            .unhide_show(current.trakt_section(), show_id)
            .await
            .map_err(to_ui_error)?;
    }
    if let Some(status) = status {
        client
            .hide_show(status.trakt_section(), show_id)
            .await
            .map_err(to_ui_error)?;
    }
    labels.set(show_id, status, Utc::now());
    state.save_labels(&labels).await
}

// ---- search & watchlist -----------------------------------------------------

#[derive(Serialize)]
pub struct SearchItem {
    /// "show" or "movie".
    kind: &'static str,
    id: u64,
    title: String,
    year: Option<i32>,
    /// Network for shows, runtime for movies.
    note: Option<String>,
    poster: Option<String>,
    watched: bool,
    on_watchlist: bool,
}

/// Shows and movies matching `query`, most relevant first.
#[tauri::command]
pub async fn search(state: State<'_, AppState>, query: String) -> Result<Vec<SearchItem>, String> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let client = state.signed_in_client().await?;
    let (shows, movies) = futures::join!(client.search_shows(query), client.search_movies(query));
    if let (Err(e), Err(_)) = (&shows, &movies) {
        return Err(e.to_string());
    }
    let (shows, movies) = (shows.unwrap_or_default(), movies.unwrap_or_default());
    let watchlist = state.watchlist_ids(&client).await.unwrap_or_default();
    let watched = state.watched_ids().await;
    let (watched_movies, movie_watchlist) =
        state.movie_data(&client, false).await.unwrap_or_default();
    let watched_movies: HashSet<u64> = watched_movies
        .iter()
        .filter_map(|w| w.movie.ids.trakt)
        .collect();
    let movie_watchlist: HashSet<u64> = movie_watchlist
        .iter()
        .filter_map(|w| w.movie.ids.trakt)
        .collect();
    state
        .fetch_artwork(shows.iter().filter_map(|r| r.show.ids.tmdb))
        .await;
    state
        .fetch_movie_artwork(movies.iter().filter_map(|r| r.movie.ids.tmdb))
        .await;

    let artwork = state.artwork.lock().await;
    let mut results: Vec<(f64, SearchItem)> = shows
        .into_iter()
        .filter_map(|r| {
            let id = r.show.ids.trakt?;
            Some((
                r.score.unwrap_or_default(),
                SearchItem {
                    kind: "show",
                    id,
                    title: r.show.display_title().to_owned(),
                    year: r.show.year,
                    note: r.show.network.clone(),
                    poster: artwork.poster_url(r.show.ids.tmdb, Size::Thumb),
                    watched: watched.contains(&id),
                    on_watchlist: watchlist.contains(&id),
                },
            ))
        })
        .collect();
    results.extend(movies.into_iter().filter_map(|r| {
        let id = r.movie.ids.trakt?;
        Some((
            r.score.unwrap_or_default(),
            SearchItem {
                kind: "movie",
                id,
                title: r.movie.display_title().to_owned(),
                year: r.movie.year,
                note: r
                    .movie
                    .runtime
                    .filter(|m| *m > 0)
                    .map(|m| format!("{m} min")),
                poster: artwork.poster(Media::Movie, r.movie.ids.tmdb, Size::Thumb),
                watched: watched_movies.contains(&id),
                on_watchlist: movie_watchlist.contains(&id),
            },
        ))
    }));
    results.sort_by(|a, b| b.0.total_cmp(&a.0));
    Ok(results.into_iter().map(|(_, item)| item).collect())
}

#[tauri::command]
pub async fn set_watchlist(
    state: State<'_, AppState>,
    show_id: u64,
    on: bool,
) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    if on {
        client.add_to_watchlist(show_id).await
    } else {
        client.remove_from_watchlist(show_id).await
    }
    .map_err(to_ui_error)?;
    state.update_watchlist(show_id, on).await;
    Ok(())
}

// ---- calendar ---------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct CalendarItem {
    first_aired: DateTime<Utc>,
    /// "episode" or "movie".
    kind: String,
    /// Show id for episodes, movie id for movies.
    id: Option<u64>,
    title: String,
    episode: Option<Episode>,
    poster: Option<String>,
}

/// Episodes airing in the next month for followed shows (minus shows on hold
/// or dropped) and releases of unwatched movies on the watchlist. Starts a
/// day early so the UI can cut at local midnight.
#[tauri::command]
pub async fn calendar(
    state: State<'_, AppState>,
    cached: Option<bool>,
) -> Result<Vec<CalendarItem>, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("calendar");
    }
    let client = state.signed_in_client().await?;
    let start = (Utc::now() - Duration::days(1)).date_naive();
    let (episodes, movies) =
        futures::join!(client.calendar(start, 32), client.movie_calendar(start, 32));
    let episodes = episodes.map_err(to_ui_error)?;
    let watched_movies: HashSet<u64> = state
        .movie_data(&client, false)
        .await
        .map(|(watched, _)| watched.iter().filter_map(|w| w.movie.ids.trakt).collect())
        .unwrap_or_default();
    let movies: Vec<_> = movies
        .unwrap_or_default()
        .into_iter()
        .filter(|m| {
            m.movie
                .ids
                .trakt
                .is_some_and(|id| !watched_movies.contains(&id))
        })
        .collect();

    let labels = state.labels.lock().await;
    let episodes: Vec<_> = episodes
        .into_iter()
        .filter(|e| !labels.is_hidden(e.show.ids.trakt))
        .collect();
    drop(labels);

    state
        .fetch_artwork(episodes.iter().filter_map(|e| e.show.ids.tmdb))
        .await;
    state
        .fetch_movie_artwork(movies.iter().filter_map(|m| m.movie.ids.tmdb))
        .await;
    let artwork = state.artwork.lock().await;
    let mut items: Vec<CalendarItem> = episodes
        .into_iter()
        .map(|e| CalendarItem {
            first_aired: e.first_aired,
            kind: "episode".into(),
            id: e.show.ids.trakt,
            title: e.show.display_title().to_owned(),
            poster: artwork.poster_url(e.show.ids.tmdb, Size::Thumb),
            episode: Some(e.episode),
        })
        .collect();
    items.extend(movies.into_iter().filter_map(|m| {
        // Release dates have no time; noon UTC lands on the same local day
        // almost everywhere.
        let day = m.movie.released_on().or_else(|| m.released.parse().ok())?;
        Some(CalendarItem {
            first_aired: day.and_hms_opt(12, 0, 0)?.and_utc(),
            kind: "movie".into(),
            id: m.movie.ids.trakt,
            title: m.movie.display_title().to_owned(),
            poster: artwork.poster(Media::Movie, m.movie.ids.tmdb, Size::Thumb),
            episode: None,
        })
    }));
    items.sort_by_key(|i| i.first_aired);
    state.save_snapshot("calendar", &items);
    Ok(items)
}

// ---- history ----------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct HistoryItem {
    watched_at: DateTime<Utc>,
    show_id: Option<u64>,
    show_title: String,
    episode: Episode,
    poster: Option<String>,
}

/// The last 100 episodes watched, newest first.
#[tauri::command]
pub async fn history(
    state: State<'_, AppState>,
    cached: Option<bool>,
) -> Result<Vec<HistoryItem>, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("history");
    }
    let client = state.signed_in_client().await?;
    let entries = client.episode_history(100).await.map_err(to_ui_error)?;
    state
        .fetch_artwork(entries.iter().filter_map(|e| e.show.ids.tmdb))
        .await;
    let artwork = state.artwork.lock().await;
    let items: Vec<HistoryItem> = entries
        .into_iter()
        .map(|e| HistoryItem {
            watched_at: e.watched_at,
            show_id: e.show.ids.trakt,
            show_title: e.show.display_title().to_owned(),
            poster: artwork.poster_url(e.show.ids.tmdb, Size::Thumb),
            episode: e.episode,
        })
        .collect();
    state.save_snapshot("history", &items);
    Ok(items)
}

// ---- library ----------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct Card {
    show_id: u64,
    title: String,
    poster: Option<String>,
    note: String,
}

#[derive(Default, Serialize, Deserialize)]
pub struct LibraryView {
    watching: Vec<Card>,
    up_to_date: Vec<Card>,
    finished: Vec<Card>,
    on_hold: Vec<Card>,
    dropped: Vec<Card>,
    watchlist: Vec<Card>,
}

#[tauri::command]
pub async fn library(
    state: State<'_, AppState>,
    force: Option<bool>,
    cached: Option<bool>,
) -> Result<LibraryView, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("library");
    }
    let mut items = state.items(force.unwrap_or(false)).await?;
    let client = state.signed_in_client().await?;
    let watchlist = client.watchlist_shows().await.map_err(to_ui_error)?;
    state
        .remember_watchlist(watchlist.iter().filter_map(|w| w.show.ids.trakt).collect())
        .await;
    state
        .fetch_artwork(watchlist.iter().filter_map(|w| w.show.ids.tmdb))
        .await;

    // Most recently watched first within each group.
    items.sort_by_key(|i| Reverse(i.last_watched_at));
    let now = Utc::now();
    let labels = state.labels.lock().await;
    let mut view = LibraryView::default();
    for item in &items {
        let Some(show_id) = item.show.ids.trakt else {
            continue;
        };
        let left = item.episodes_left();
        let card = |note: String| Card {
            show_id,
            title: item.show.display_title().to_owned(),
            poster: item.poster.clone(),
            note,
        };
        match labels.status(show_id) {
            Some(ShowStatus::OnHold) => view.on_hold.push(card(left_note(left))),
            Some(ShowStatus::Dropped) => view.dropped.push(card(left_note(left))),
            // Same rule as Progress: the next episode has aired.
            None if item.next_airs_at().is_some_and(|at| at <= now) => {
                view.watching.push(card(left_note(left)))
            }
            None if is_over(&item.show) => view.finished.push(card(status_note(&item.show))),
            None => view.up_to_date.push(card(next_note(item))),
        }
    }

    // Labelled shows that were never watched (e.g. hidden in Showly).
    let watched: HashSet<u64> = items.iter().filter_map(|i| i.show.ids.trakt).collect();
    let unwatched: Vec<(u64, ShowStatus, Show)> = labels
        .iter()
        .filter(|(id, _)| !watched.contains(id))
        .filter_map(|(id, l)| Some((id, l.status, l.show.clone()?)))
        .collect();
    drop(labels);
    state
        .fetch_artwork(unwatched.iter().filter_map(|(_, _, s)| s.ids.tmdb))
        .await;

    let artwork = state.artwork.lock().await;
    for (show_id, status, show) in unwatched {
        let card = Card {
            show_id,
            title: show.display_title().to_owned(),
            poster: artwork.poster_url(show.ids.tmdb, Size::Thumb),
            note: "Not started".into(),
        };
        match status {
            ShowStatus::OnHold => view.on_hold.push(card),
            ShowStatus::Dropped => view.dropped.push(card),
        }
    }
    view.watchlist = watchlist
        .into_iter()
        .filter_map(|w| {
            let show_id = w.show.ids.trakt.filter(|id| !watched.contains(id))?;
            Some(Card {
                show_id,
                title: w.show.display_title().to_owned(),
                poster: artwork.poster_url(w.show.ids.tmdb, Size::Thumb),
                note: w.show.year.map(|y| y.to_string()).unwrap_or_default(),
            })
        })
        .collect();
    for group in [&mut view.finished, &mut view.on_hold, &mut view.dropped] {
        group.sort_by_key(|c| title_sort_key(&c.title));
    }
    state.save_snapshot("library", &view);
    Ok(view)
}

fn left_note(left: u32) -> String {
    match left {
        0 => "Up to date".into(),
        1 => "1 episode left".into(),
        n => format!("{n} episodes left"),
    }
}

fn is_over(show: &Show) -> bool {
    matches!(show.status.as_deref(), Some("ended" | "canceled"))
}

fn status_note(show: &Show) -> String {
    match show.status.as_deref() {
        Some("canceled") => "Canceled".into(),
        _ => "Ended".into(),
    }
}

fn next_note(item: &ProgressItem) -> String {
    match item.next_airs_at() {
        Some(at) => format!("Next: {}", at.format("%-d %b")),
        None => "Waiting for new episodes".into(),
    }
}

// ---- discover ---------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct DiscoverItem {
    show_id: u64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    /// How likely the taste model thinks you'd like it, 0–100.
    match_pct: Option<u8>,
}

#[derive(Serialize, Deserialize)]
pub struct DiscoverRow {
    title: String,
    items: Vec<DiscoverItem>,
}

#[derive(Serialize, Deserialize)]
pub struct DiscoverView {
    rows: Vec<DiscoverRow>,
    /// Shows the taste model learned from (0 if it isn't used).
    learned_from: usize,
}

/// Everything Discover and AI picks need: the taste model and candidate rows.
struct Discovery {
    model: TasteModel,
    examples: usize,
    rows: Vec<(String, Vec<Show>)>,
}

async fn gather(state: &AppState, client: &TraktClient, force: bool) -> Result<Discovery, String> {
    let mut items = state.items(force).await?;
    let now = Utc::now();
    items.sort_by_key(|i| Reverse(i.last_watched_at));
    // Train on your history, including shows hidden without watching.
    let labels = state.labels.lock().await;
    let mut examples: Vec<(Vec<String>, bool)> = Vec::new();
    for item in &items {
        let status = item.show.ids.trakt.and_then(|id| labels.status(id));
        if let Some(label) = taste::label(item, status, now) {
            examples.push((taste::features(&item.show), label));
        }
    }
    let watched: HashSet<u64> = items.iter().filter_map(|i| i.show.ids.trakt).collect();
    for (_, label) in labels.iter().filter(|(id, _)| !watched.contains(id)) {
        if let Some(show) = &label.show {
            examples.push((taste::features(show), false));
        }
    }
    let exclude: HashSet<u64> = watched
        .iter()
        .copied()
        .chain(labels.iter().map(|(id, _)| id))
        .collect();
    drop(labels);
    let model = TasteModel::train(&examples);

    // The three shows watched most recently seed "Because you watched".
    let recent: Vec<Show> = items
        .iter()
        .filter(|i| i.show.ids.trakt.is_some())
        .take(3)
        .map(|i| i.show.clone())
        .collect();

    let (recommended, trending, related) = futures::join!(
        client.recommended_shows(40),
        client.trending_shows(30),
        futures::future::join_all(
            recent
                .iter()
                .map(|s| client.related_shows(s.ids.trakt.unwrap_or_default(), 20))
        ),
    );
    let recommended = recommended.unwrap_or_default();
    let trending: Vec<Show> = trending
        .unwrap_or_default()
        .into_iter()
        .map(|t| t.show)
        .collect();
    let related: Vec<Vec<Show>> = related.into_iter().map(Result::unwrap_or_default).collect();

    let fresh = |shows: Vec<Show>| -> Vec<Show> {
        let mut seen = HashSet::new();
        shows
            .into_iter()
            .filter(|s| {
                s.ids
                    .trakt
                    .is_some_and(|id| !exclude.contains(&id) && seen.insert(id))
            })
            .collect()
    };
    let by_match = |mut shows: Vec<Show>| -> Vec<Show> {
        if model.is_useful() {
            shows.sort_by(|a, b| model.score(b).total_cmp(&model.score(a)));
        }
        shows
    };

    let mut rows: Vec<(String, Vec<Show>)> = Vec::new();
    if model.is_useful() {
        let everything: Vec<Show> = recommended
            .iter()
            .chain(related.iter().flatten())
            .chain(trending.iter())
            .cloned()
            .collect();
        let mut picked = by_match(fresh(everything));
        picked.truncate(20);
        rows.push(("Picked for you".into(), picked));
    }
    for (seed, shows) in recent.iter().zip(related) {
        rows.push((
            format!("Because you watched {}", seed.display_title()),
            by_match(fresh(shows)),
        ));
    }
    rows.push(("Recommended by Trakt".into(), fresh(recommended)));
    rows.push(("Trending".into(), fresh(trending)));
    rows.retain(|(_, shows)| !shows.is_empty());

    Ok(Discovery {
        examples: examples.len(),
        model,
        rows,
    })
}

fn match_pct(model: &TasteModel, show: &Show) -> Option<u8> {
    model
        .is_useful()
        .then(|| (model.score(show) * 100.0).round() as u8)
}

/// Rows of shows you haven't watched: picked by the on-device taste model,
/// related to what you watched last, Trakt's recommendations, and trending.
#[tauri::command]
pub async fn discover(
    state: State<'_, AppState>,
    force: Option<bool>,
    cached: Option<bool>,
) -> Result<DiscoverView, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("discover");
    }
    let client = state.signed_in_client().await?;
    let found = gather(&state, &client, force.unwrap_or(false)).await?;

    state
        .fetch_artwork(
            found
                .rows
                .iter()
                .flat_map(|(_, s)| s)
                .filter_map(|s| s.ids.tmdb),
        )
        .await;
    let artwork = state.artwork.lock().await;
    let view = DiscoverView {
        learned_from: if found.model.is_useful() {
            found.examples
        } else {
            0
        },
        rows: found
            .rows
            .iter()
            .map(|(title, shows)| DiscoverRow {
                title: title.clone(),
                items: shows
                    .iter()
                    .filter_map(|s| {
                        Some(DiscoverItem {
                            show_id: s.ids.trakt?,
                            title: s.display_title().to_owned(),
                            year: s.year,
                            poster: artwork.poster_url(s.ids.tmdb, Size::Thumb),
                            match_pct: match_pct(&found.model, s),
                        })
                    })
                    .collect(),
            })
            .collect(),
    };
    drop(artwork);
    state.save_snapshot("discover", &view);
    Ok(view)
}

// ---- AI picks -----------------------------------------------------------------

/// Settings as the UI sees them; the key itself never goes back to the UI.
#[derive(Serialize)]
pub struct AiSettingsView {
    configured: bool,
    base_url: String,
    model: String,
    reasoning_effort: String,
    /// Last four characters of the saved key, to recognise it.
    key_hint: Option<String>,
}

#[tauri::command]
pub async fn ai_settings(state: State<'_, AppState>) -> Result<AiSettingsView, String> {
    let config = state.ai_config().await;
    Ok(AiSettingsView {
        configured: config.as_ref().is_some_and(|c| !c.api_key.is_empty()),
        base_url: config
            .as_ref()
            .map_or(ai::DEFAULT_BASE_URL.into(), |c| c.base_url.clone()),
        model: config
            .as_ref()
            .map_or(ai::DEFAULT_MODEL.into(), |c| c.model.clone()),
        reasoning_effort: config
            .as_ref()
            .map_or(Some(ai::DEFAULT_REASONING_EFFORT.into()), |c| {
                c.reasoning_effort.clone()
            })
            .unwrap_or_default(),
        key_hint: config.as_ref().and_then(|c| {
            let chars: Vec<char> = c.api_key.chars().collect();
            (chars.len() >= 8).then(|| chars[chars.len() - 4..].iter().collect())
        }),
    })
}

/// Checks a key (the new one, or the saved one if `api_key` is empty) by
/// listing the provider's models.
#[tauri::command]
pub async fn ai_models(
    state: State<'_, AppState>,
    base_url: String,
    api_key: Option<String>,
) -> Result<Vec<String>, String> {
    let config = ai::AiConfig {
        base_url: base_url.trim().to_owned(),
        api_key: resolve_key(&state, api_key).await?,
        model: String::new(),
        reasoning_effort: None,
    };
    ai::list_models(&config).await.map_err(to_ui_error)
}

/// Saves the provider, model and (if given) a new key. The key is stored only
/// in this app's private storage on the device.
#[tauri::command]
pub async fn save_ai_settings(
    state: State<'_, AppState>,
    base_url: String,
    model: String,
    reasoning_effort: Option<String>,
    api_key: Option<String>,
) -> Result<(), String> {
    let config = ai::AiConfig {
        base_url: base_url.trim().to_owned(),
        api_key: resolve_key(&state, api_key).await?,
        model: model.trim().to_owned(),
        // Empty means "provider default".
        reasoning_effort: reasoning_effort.filter(|e| !e.is_empty()),
    };
    state.set_ai_config(Some(config)).await
}

#[tauri::command]
pub async fn clear_ai_key(state: State<'_, AppState>) -> Result<(), String> {
    state.set_ai_config(None).await
}

async fn resolve_key(state: &AppState, api_key: Option<String>) -> Result<String, String> {
    match api_key
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
    {
        Some(key) => Ok(key),
        None => state
            .ai_config()
            .await
            .map(|c| c.api_key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| "Paste an API key first.".into()),
    }
}

#[derive(Serialize)]
pub struct AiPickItem {
    show_id: u64,
    title: String,
    year: Option<i32>,
    poster: Option<String>,
    reason: String,
    match_pct: Option<u8>,
}

/// Up to ten shows for a mood, chosen by the configured AI from the same
/// candidates Discover shows.
#[tauri::command]
pub async fn ai_picks(state: State<'_, AppState>, mood: String) -> Result<Vec<AiPickItem>, String> {
    let mood = mood.trim();
    if mood.is_empty() {
        return Err("Describe what you're in the mood for.".into());
    }
    let config = state
        .ai_config()
        .await
        .filter(|c| !c.api_key.is_empty())
        .ok_or("Add an AI API key in Settings first.")?;
    let client = state.signed_in_client().await?;
    let found = gather(&state, &client, false).await?;

    // Unique candidates, best taste match first, capped to keep requests small.
    let mut seen = HashSet::new();
    let mut shows: Vec<&Show> = found
        .rows
        .iter()
        .flat_map(|(_, s)| s)
        .filter(|s| s.ids.trakt.is_some_and(|id| seen.insert(id)))
        .collect();
    if found.model.is_useful() {
        shows.sort_by(|a, b| found.model.score(b).total_cmp(&found.model.score(a)));
    }
    shows.truncate(120);
    let candidates: Vec<ai::Candidate> = shows
        .iter()
        .filter_map(|s| {
            Some(ai::Candidate {
                id: s.ids.trakt?,
                title: s.display_title().to_owned(),
                year: s.year,
                genres: s.genres.clone().unwrap_or_default(),
                overview: s.overview.clone(),
            })
        })
        .collect();

    // The AI looks at your history and cast & crew through these tools.
    let items = state.items(false).await?;
    let mut tools = {
        let labels = state.labels.lock().await;
        ToolBox::new(&items, &labels, client.clone(), Utc::now())
    };
    let picks = ai::pick(&config, mood, &candidates, &mut tools)
        .await
        .map_err(to_ui_error)?;
    if picks.is_empty() {
        return Err("The AI didn't return any picks. Try describing the mood differently.".into());
    }

    let artwork = state.artwork.lock().await;
    Ok(picks
        .into_iter()
        .filter_map(|p| {
            let show = shows.iter().find(|s| s.ids.trakt == Some(p.id))?;
            Some(AiPickItem {
                show_id: p.id,
                title: show.display_title().to_owned(),
                year: show.year,
                poster: artwork.poster_url(show.ids.tmdb, Size::Thumb),
                reason: p.reason,
                match_pct: match_pct(&found.model, show),
            })
        })
        .collect())
}

// ---- lists ------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize)]
pub struct ListSummary {
    id: u64,
    name: String,
    item_count: u32,
    /// Service that regenerates the list (read-only in Nextep), if any.
    managed_by: Option<String>,
}

impl From<&TraktList> for ListSummary {
    fn from(list: &TraktList) -> Self {
        Self {
            id: list.ids.trakt,
            name: list.name.clone(),
            item_count: list.item_count,
            managed_by: list.managed_by().map(str::to_owned),
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ListsView {
    lists: Vec<ListSummary>,
    /// Personal lists the account may have, if Trakt says.
    limit: Option<u32>,
}

#[tauri::command]
pub async fn lists(
    state: State<'_, AppState>,
    force: Option<bool>,
    cached: Option<bool>,
) -> Result<ListsView, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("lists");
    }
    let client = state.signed_in_client().await?;
    let lists = state.lists(&client, force.unwrap_or(false)).await?;
    let view = ListsView {
        lists: lists.iter().map(ListSummary::from).collect(),
        limit: client.list_limit().await.ok().flatten(),
    };
    state.save_snapshot("lists", &view);
    Ok(view)
}

#[derive(Serialize)]
pub struct ListDetail {
    list: ListSummary,
    shows: Vec<Card>,
}

#[tauri::command]
pub async fn list_detail(state: State<'_, AppState>, list_id: u64) -> Result<ListDetail, String> {
    let client = state.signed_in_client().await?;
    let lists = state.lists(&client, false).await?;
    let list = lists
        .iter()
        .find(|l| l.ids.trakt == list_id)
        .ok_or("This list no longer exists.")?;
    let shows = client.list_shows(list_id).await.map_err(to_ui_error)?;
    state
        .remember_list_members(
            list_id,
            shows.iter().filter_map(|s| s.show.ids.trakt).collect(),
        )
        .await;
    state
        .fetch_artwork(shows.iter().filter_map(|s| s.show.ids.tmdb))
        .await;

    let artwork = state.artwork.lock().await;
    Ok(ListDetail {
        list: ListSummary::from(list),
        shows: shows
            .into_iter()
            .filter_map(|s| {
                Some(Card {
                    show_id: s.show.ids.trakt?,
                    title: s.show.display_title().to_owned(),
                    poster: artwork.poster_url(s.show.ids.tmdb, Size::Thumb),
                    note: s.show.year.map(|y| y.to_string()).unwrap_or_default(),
                })
            })
            .collect(),
    })
}

#[tauri::command]
pub async fn create_list(state: State<'_, AppState>, name: String) -> Result<ListSummary, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the list a name.".into());
    }
    let client = state.signed_in_client().await?;
    let list = client.create_list(name).await.map_err(list_error)?;
    state.invalidate_lists().await;
    state
        .remember_list_members(list.ids.trakt, HashSet::new())
        .await;
    Ok(ListSummary::from(&list))
}

#[tauri::command]
pub async fn rename_list(
    state: State<'_, AppState>,
    list_id: u64,
    name: String,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the list a name.".into());
    }
    let client = state.signed_in_client().await?;
    client
        .rename_list(list_id, name)
        .await
        .map_err(list_error)?;
    state.invalidate_lists().await;
    Ok(())
}

#[tauri::command]
pub async fn delete_list(state: State<'_, AppState>, list_id: u64) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    client.delete_list(list_id).await.map_err(list_error)?;
    state.invalidate_lists().await;
    Ok(())
}

#[tauri::command]
pub async fn set_in_list(
    state: State<'_, AppState>,
    list_id: u64,
    show_id: u64,
    on: bool,
) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    if on {
        client.add_to_list(list_id, show_id).await
    } else {
        client.remove_from_list(list_id, show_id).await
    }
    .map_err(list_error)?;
    state.update_list_member(list_id, show_id, on).await;
    state.invalidate_lists().await;
    Ok(())
}

#[derive(Serialize)]
pub struct ShowListEntry {
    id: u64,
    name: String,
    contains: bool,
}

/// The user's own (editable) lists, and whether each contains the show.
#[tauri::command]
pub async fn show_lists(
    state: State<'_, AppState>,
    show_id: u64,
) -> Result<Vec<ShowListEntry>, String> {
    let client = state.signed_in_client().await?;
    let lists = state.lists(&client, false).await?;
    let mut entries = Vec::new();
    for list in lists.iter().filter(|l| l.managed_by().is_none()) {
        let members = state.list_members(&client, list.ids.trakt).await?;
        entries.push(ShowListEntry {
            id: list.ids.trakt,
            name: list.name.clone(),
            contains: members.contains(&show_id),
        });
    }
    Ok(entries)
}

/// Trakt answers 420 when an account limit (number of lists, or items per
/// list) is reached.
fn list_error(e: Error) -> String {
    match e {
        Error::Api { status: 420, .. } => {
            "Trakt's limit for your account is reached (free accounts: 12 lists, 1,000 shows each)."
                .into()
        }
        e => to_ui_error(e),
    }
}

// ---- statistics -------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct Stats {
    episodes: u64,
    minutes: u64,
    shows: u64,
    top_genres: Vec<String>,
}

#[tauri::command]
pub async fn stats(state: State<'_, AppState>, cached: Option<bool>) -> Result<Stats, String> {
    if cached.unwrap_or(false) {
        return state.snapshot("stats");
    }
    let client = state.signed_in_client().await?;
    let totals = client.user_stats().await.map_err(to_ui_error)?;
    let items = state.items(false).await?;
    let stats = Stats {
        episodes: totals.episodes.watched,
        minutes: totals.episodes.minutes,
        shows: totals.shows.watched,
        top_genres: progress::top_genres(&items, 3),
    };
    state.save_snapshot("stats", &stats);
    Ok(stats)
}

// ---- show page --------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct EpisodeRow {
    number: u32,
    title: Option<String>,
    trakt_id: Option<u64>,
    first_aired: Option<DateTime<Utc>>,
    watched: bool,
}

#[derive(Serialize, Deserialize)]
pub struct SeasonRow {
    number: u32,
    aired: u32,
    completed: u32,
    episodes: Vec<EpisodeRow>,
}

#[derive(Serialize, Deserialize)]
pub struct ShowDetails {
    show: Show,
    poster: Option<String>,
    backdrop: Option<String>,
    status: Option<ShowStatus>,
    on_watchlist: bool,
    aired: u32,
    completed: u32,
    next_episode: Option<Episode>,
    seasons: Vec<SeasonRow>,
    /// Subscription and free services in the user's region.
    streaming: Vec<Provider>,
}

/// `region` is an ISO 3166-1 country code for streaming availability.
#[tauri::command]
/// `cached` returns the copy saved when the page was last opened, so a show
/// you've seen before opens instantly.
pub async fn show_details(
    state: State<'_, AppState>,
    show_id: u64,
    region: Option<String>,
    cached: Option<bool>,
) -> Result<ShowDetails, String> {
    let snapshot = format!("show_{show_id}");
    if cached.unwrap_or(false) {
        return state.snapshot(&snapshot);
    }
    let client = state.signed_in_client().await?;

    // Best effort: a failed lookup just means no "where to watch" row.
    let streaming_for = |tmdb_id: Option<u64>| {
        let region = region.clone();
        let tmdb = state.tmdb().cloned();
        async move {
            match (tmdb, tmdb_id, region) {
                (Some(tmdb), Some(id), Some(region)) => tmdb
                    .streaming(Media::Tv, id, &region)
                    .await
                    .unwrap_or_default(),
                _ => Vec::new(),
            }
        }
    };
    // Everything at once. A show you watch already has its TMDB id cached,
    // so "where to watch" doesn't have to wait for the show itself.
    let known_tmdb = state.cached_show(show_id).await.and_then(|s| s.ids.tmdb);
    let (core, watchlist, early_streaming) = futures::join!(
        async {
            futures::try_join!(
                client.show(show_id),
                client.show_progress(show_id),
                client.seasons(show_id),
            )
        },
        state.watchlist_ids(&client),
        streaming_for(known_tmdb),
    );
    let (show, progress, seasons) = core.map_err(to_ui_error)?;
    let streaming = if known_tmdb.is_some() {
        early_streaming
    } else {
        streaming_for(show.ids.tmdb).await
    };

    state.fetch_artwork(show.ids.tmdb).await;
    let (poster, backdrop) = {
        let artwork = state.artwork.lock().await;
        (
            artwork.poster_url(show.ids.tmdb, Size::Poster),
            artwork.backdrop_url(show.ids.tmdb),
        )
    };

    // Watched flags by (season, episode) from the progress endpoint.
    let watched: HashMap<(u32, u32), bool> = progress
        .seasons
        .iter()
        .flat_map(|s| {
            s.episodes
                .iter()
                .map(|e| ((s.number, e.number), e.completed))
        })
        .collect();
    let counts: HashMap<u32, (u32, u32)> = progress
        .seasons
        .iter()
        .map(|s| (s.number, (s.aired, s.completed)))
        .collect();

    let seasons = seasons
        .into_iter()
        // Specials (season 0) are left out, like Trakt's own progress.
        .filter(|s| s.number > 0)
        .map(|s| {
            let (aired, completed) = counts.get(&s.number).copied().unwrap_or_default();
            SeasonRow {
                number: s.number,
                aired,
                completed,
                episodes: s
                    .episodes
                    .into_iter()
                    .map(|e| EpisodeRow {
                        watched: watched.get(&(s.number, e.number)).copied().unwrap_or(false),
                        number: e.number,
                        title: e.title,
                        trakt_id: e.ids.trakt,
                        first_aired: e.first_aired,
                    })
                    .collect(),
            }
        })
        .collect();

    let status = state.labels.lock().await.status(show_id);
    let details = ShowDetails {
        show,
        poster,
        backdrop,
        status,
        on_watchlist: watchlist?.contains(&show_id),
        aired: progress.aired,
        completed: progress.completed,
        next_episode: progress.next_episode,
        seasons,
        streaming,
    };
    state.save_snapshot(&snapshot, &details);
    Ok(details)
}
