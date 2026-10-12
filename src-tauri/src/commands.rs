//! Commands the UI calls through `invoke`. Tauri maps camelCase arguments
//! from JavaScript to the snake_case parameters here.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

use nextep_core::Error;
use nextep_core::chrono::{DateTime, Duration, Utc};
use nextep_core::labels::ShowStatus;
use nextep_core::models::{Episode, Show, TraktList};
use nextep_core::progress::{
    self, ProgressItem, ProgressOptions, ProgressView, SortBy, title_sort_key,
};
use nextep_core::tmdb::{Provider, Size};
use nextep_core::trakt::{DeviceCode, DevicePoll};
use nextep_core::update::{self, Release};
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
    show_id: u64,
    title: String,
    year: Option<i32>,
    network: Option<String>,
    poster: Option<String>,
    watched: bool,
    on_watchlist: bool,
}

#[tauri::command]
pub async fn search(state: State<'_, AppState>, query: String) -> Result<Vec<SearchItem>, String> {
    let query = query.trim();
    if query.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let client = state.signed_in_client().await?;
    let results = client.search_shows(query).await.map_err(to_ui_error)?;
    let watchlist = state.watchlist_ids(&client).await?;
    let watched = state.watched_ids().await;
    state
        .fetch_artwork(results.iter().filter_map(|r| r.show.ids.tmdb))
        .await;

    let artwork = state.artwork.lock().await;
    Ok(results
        .into_iter()
        .filter_map(|r| {
            let show_id = r.show.ids.trakt?;
            Some(SearchItem {
                show_id,
                title: r.show.display_title().to_owned(),
                year: r.show.year,
                network: r.show.network.clone(),
                poster: artwork.poster_url(r.show.ids.tmdb, Size::Thumb),
                watched: watched.contains(&show_id),
                on_watchlist: watchlist.contains(&show_id),
            })
        })
        .collect())
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
    show_id: Option<u64>,
    show_title: String,
    episode: Episode,
    poster: Option<String>,
}

/// Episodes airing in the next month for followed shows, minus shows that are
/// on hold or dropped. Starts a day early so the UI can cut at local midnight.
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
    let items: Vec<CalendarItem> = entries
        .into_iter()
        .map(|e| CalendarItem {
            first_aired: e.first_aired,
            show_id: e.show.ids.trakt,
            show_title: e.show.display_title().to_owned(),
            poster: artwork.poster_url(e.show.ids.tmdb, Size::Thumb),
            episode: e.episode,
        })
        .collect();
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
pub async fn show_details(
    state: State<'_, AppState>,
    show_id: u64,
    region: Option<String>,
) -> Result<ShowDetails, String> {
    let client = state.signed_in_client().await?;
    let (show, progress, seasons) = futures::try_join!(
        client.show(show_id),
        client.show_progress(show_id),
        client.seasons(show_id),
    )
    .map_err(to_ui_error)?;

    // Best effort: a failed lookup just means no "where to watch" row.
    let streaming = match (state.tmdb(), show.ids.tmdb, region.as_deref()) {
        (Some(tmdb), Some(tmdb_id), Some(region)) => {
            tmdb.tv_streaming(tmdb_id, region).await.unwrap_or_default()
        }
        _ => Vec::new(),
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
    let on_watchlist = state.watchlist_ids(&client).await?.contains(&show_id);
    Ok(ShowDetails {
        show,
        poster,
        backdrop,
        status,
        on_watchlist,
        aired: progress.aired,
        completed: progress.completed,
        next_episode: progress.next_episode,
        seasons,
        streaming,
    })
}
