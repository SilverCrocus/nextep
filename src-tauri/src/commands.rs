//! Commands the UI calls through `invoke`. Tauri maps camelCase arguments
//! from JavaScript to the snake_case parameters here.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use nextep_core::chrono::{DateTime, Duration, Utc};
use nextep_core::labels::ShowStatus;
use nextep_core::models::{Episode, Show};
use nextep_core::progress::{
    self, ProgressItem, ProgressOptions, ProgressView, SortBy, title_sort_key,
};
use nextep_core::tmdb::Size;
use nextep_core::trakt::{DeviceCode, DevicePoll};
use serde::Serialize;
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

/// Opens a Trakt page in the system browser. Limited to Trakt so the UI can't
/// be used to open arbitrary links.
#[tauri::command]
pub fn open_trakt(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://auth.trakt.tv/") || url.starts_with("https://trakt.tv/")) {
        return Err("only Trakt links can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

// ---- progress ---------------------------------------------------------------

#[tauri::command]
pub async fn progress(
    state: State<'_, AppState>,
    sort: Option<SortBy>,
    force: Option<bool>,
) -> Result<ProgressView, String> {
    let mut items = state.items(force.unwrap_or(false)).await?;
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

/// `None` puts the show back to watching.
#[tauri::command]
pub async fn set_show_status(
    state: State<'_, AppState>,
    show_id: u64,
    status: Option<ShowStatus>,
) -> Result<(), String> {
    let mut labels = state.labels.lock().await;
    labels.set(show_id, status, Utc::now());
    state.save_labels(&labels).await
}

// ---- calendar ---------------------------------------------------------------

#[derive(Serialize)]
pub struct CalendarItem {
    first_aired: DateTime<Utc>,
    show_id: Option<u64>,
    show_title: String,
    episode: Episode,
    poster: Option<String>,
}

/// Episodes airing in the next month for followed shows, minus shows that are
/// on hold or dropped. Starts a day early so the UI can cut at local midnight.
#[tauri::command]
pub async fn calendar(state: State<'_, AppState>) -> Result<Vec<CalendarItem>, String> {
    let client = state.signed_in_client().await?;
    let start = (Utc::now() - Duration::days(1)).date_naive();
    let entries = client.calendar(start, 32).await.map_err(to_ui_error)?;

    let labels = state.labels.lock().await;
    let entries: Vec<_> = entries
        .into_iter()
        .filter(|e| !labels.is_hidden(e.show.ids.trakt))
        .collect();
    drop(labels);

    state
        .fetch_artwork(entries.iter().filter_map(|e| e.show.ids.tmdb))
        .await;
    let artwork = state.artwork.lock().await;
    Ok(entries
        .into_iter()
        .map(|e| CalendarItem {
            first_aired: e.first_aired,
            show_id: e.show.ids.trakt,
            show_title: e.show.display_title().to_owned(),
            poster: artwork.poster_url(e.show.ids.tmdb, Size::Thumb),
            episode: e.episode,
        })
        .collect())
}

// ---- library ----------------------------------------------------------------

#[derive(Serialize)]
pub struct Card {
    show_id: u64,
    title: String,
    poster: Option<String>,
    note: String,
}

#[derive(Default, Serialize)]
pub struct LibraryView {
    watching: Vec<Card>,
    up_to_date: Vec<Card>,
    finished: Vec<Card>,
    on_hold: Vec<Card>,
    dropped: Vec<Card>,
    watchlist: Vec<Card>,
}

#[tauri::command]
pub async fn library(state: State<'_, AppState>) -> Result<LibraryView, String> {
    let mut items = state.items(false).await?;
    let client = state.signed_in_client().await?;
    let watchlist = client.watchlist_shows().await.map_err(to_ui_error)?;
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
            // Same rule as Progress: there is an aired episode to watch next.
            None if item.next_episode.is_some()
                && !item.next_airs_at().is_some_and(|at| at > now) =>
            {
                view.watching.push(card(left_note(left)))
            }
            None if is_over(&item.show) => view.finished.push(card(status_note(&item.show))),
            None => view.up_to_date.push(card(next_note(item))),
        }
    }
    drop(labels);

    let watched: HashSet<u64> = items.iter().filter_map(|i| i.show.ids.trakt).collect();
    let artwork = state.artwork.lock().await;
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

// ---- show page --------------------------------------------------------------

#[derive(Serialize)]
pub struct EpisodeRow {
    number: u32,
    title: Option<String>,
    trakt_id: Option<u64>,
    first_aired: Option<DateTime<Utc>>,
    watched: bool,
}

#[derive(Serialize)]
pub struct SeasonRow {
    number: u32,
    aired: u32,
    completed: u32,
    episodes: Vec<EpisodeRow>,
}

#[derive(Serialize)]
pub struct ShowDetails {
    show: Show,
    poster: Option<String>,
    backdrop: Option<String>,
    status: Option<ShowStatus>,
    aired: u32,
    completed: u32,
    next_episode: Option<Episode>,
    seasons: Vec<SeasonRow>,
}

#[tauri::command]
pub async fn show_details(state: State<'_, AppState>, show_id: u64) -> Result<ShowDetails, String> {
    let client = state.signed_in_client().await?;
    let (show, progress, seasons) = futures::try_join!(
        client.show(show_id),
        client.show_progress(show_id),
        client.seasons(show_id),
    )
    .map_err(to_ui_error)?;

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
    Ok(ShowDetails {
        show,
        poster,
        backdrop,
        status,
        aired: progress.aired,
        completed: progress.completed,
        next_episode: progress.next_episode,
        seasons,
    })
}
