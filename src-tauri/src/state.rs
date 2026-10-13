//! App state: the Trakt and TMDB clients plus everything persisted in the
//! app's private data directory.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use nextep_core::ai::AiConfig;
use nextep_core::alerts::{self, Alert};
use nextep_core::chrono::{DateTime, Duration, Utc};
use nextep_core::labels::{Labels, ShowStatus};
use nextep_core::models::{
    CalendarEntry, ItemId, ListItem, Movie, Show, ShowPeople, TraktList, WatchedMovie,
    WatchlistMovie,
};
use nextep_core::progress::ProgressItem;
use nextep_core::sync::{self, ProgressCache};
use nextep_core::tmdb::{ArtworkCache, Media, Size, TmdbClient};
use nextep_core::{Error, TraktClient};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tauri::async_runtime::Mutex;

/// Error string the UI checks for to show the sign-in screen.
pub const NOT_SIGNED_IN: &str = "not_signed_in";

const TOKEN_FILE: &str = "trakt_token.json";
const PROGRESS_FILE: &str = "progress_cache.json";
const LABELS_FILE: &str = "labels.json";
const ARTWORK_FILE: &str = "artwork_cache.json";
const SNAPSHOT_PREFIX: &str = "snapshot_";
/// AI provider settings, including the user's API key. Device-only.
const AI_FILE: &str = "ai_settings.json";
const ALERTS_FILE: &str = "alerts.json";

/// Switching between tabs reuses a refresh this recent instead of calling
/// Trakt again.
const REUSE_SECS: i64 = 120;

/// Trakt's recommendations and trending shift slowly, so Discover and AI
/// picks reuse them for this long unless Discover is refreshed.
const FEEDS_REUSE_SECS: i64 = 30 * 60;

/// Candidate shows (or movies) from Trakt for Discover and AI picks. `None`
/// and missing entries haven't been fetched (or failed) yet.
#[derive(Clone)]
pub struct Feeds<T> {
    pub recommended: Option<Vec<T>>,
    pub trending: Option<Vec<T>>,
    /// Related to a watched show (or movie), by its Trakt id.
    pub related: HashMap<u64, Vec<T>>,
}

impl<T> Default for Feeds<T> {
    fn default() -> Self {
        Self {
            recommended: None,
            trending: None,
            related: HashMap::new(),
        }
    }
}

impl<T: Clone> Feeds<T> {
    /// Adds what `other` has that this doesn't.
    pub fn merge_from(&mut self, other: &Feeds<T>) {
        self.merge(other.clone());
    }
}

impl<T> Feeds<T> {
    /// Adds what `other` has that this doesn't.
    fn merge(&mut self, other: Feeds<T>) {
        if self.recommended.is_none() {
            self.recommended = other.recommended;
        }
        if self.trending.is_none() {
            self.trending = other.trending;
        }
        for (id, items) in other.related {
            self.related.entry(id).or_insert(items);
        }
    }
}

/// Feeds kept in [`AppState`], with when they were first fetched.
type FeedSlot<T> = Mutex<Option<(DateTime<Utc>, Feeds<T>)>>;

/// Which fetch of the feeds a caller started from, to hand back to
/// [`AppState::remember_show_feeds`] / [`AppState::remember_movie_feeds`].
pub struct FeedsTicket {
    account: u64,
    at: DateTime<Utc>,
}

pub struct AppState {
    dir: PathBuf,
    pub trakt: Mutex<TraktClient>,
    tmdb: Option<TmdbClient>,
    progress: Mutex<ProgressCache>,
    pub labels: Mutex<Labels>,
    pub artwork: Mutex<ArtworkCache>,
    /// Latest refresh, shared by Progress and Library.
    items: Mutex<Option<(DateTime<Utc>, Vec<ProgressItem>)>>,
    /// Trakt ids on the watchlist, from the latest fetch.
    watchlist: Mutex<Option<(DateTime<Utc>, HashSet<u64>)>>,
    /// Personal lists, and the shows on each list once loaded.
    lists: Mutex<Option<(DateTime<Utc>, Vec<TraktList>)>>,
    list_members: Mutex<HashMap<u64, HashSet<ItemId>>>,
    ai: Mutex<Option<AiConfig>>,
    /// Watched movies and the movie watchlist, from the latest fetch.
    movies: Mutex<Option<(DateTime<Utc>, MovieData)>>,
    show_feeds: FeedSlot<Show>,
    movie_feeds: FeedSlot<Movie>,
    /// Bumped on sign-out, so fetches that started before it are dropped.
    account: AtomicU64,
    /// Bumped whenever the items are invalidated, so a refresh that started
    /// before a change can't store what it fetched.
    items_generation: AtomicU64,
    /// Cast and crew of shows, kept between AI picks requests.
    pub people: Mutex<HashMap<u64, ShowPeople>>,
    /// Followed shows' episodes from the latest refresh.
    calendar: Mutex<Vec<CalendarEntry>>,
    alerts: Mutex<AlertSettings>,
    /// What was last handed to the OS to notify, so an unchanged plan isn't
    /// scheduled again.
    scheduled: Mutex<Option<Vec<Alert>>>,
}

/// New-episode alerts setting, saved on the device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlertSettings {
    pub enabled: bool,
}

type MovieData = (Vec<WatchedMovie>, Vec<WatchlistMovie>);

impl AppState {
    pub fn load(dir: PathBuf, trakt_client_id: &str, tmdb_api_key: Option<&str>) -> Self {
        let mut trakt = TraktClient::new(trakt_client_id);
        trakt.set_token(read(&dir, TOKEN_FILE));
        Self {
            trakt: Mutex::new(trakt),
            tmdb: tmdb_api_key.map(TmdbClient::new),
            progress: Mutex::new(read(&dir, PROGRESS_FILE).unwrap_or_default()),
            labels: Mutex::new(read(&dir, LABELS_FILE).unwrap_or_default()),
            artwork: Mutex::new(read(&dir, ARTWORK_FILE).unwrap_or_default()),
            items: Mutex::new(None),
            watchlist: Mutex::new(None),
            lists: Mutex::new(None),
            list_members: Mutex::new(HashMap::new()),
            ai: Mutex::new(read(&dir, AI_FILE)),
            movies: Mutex::new(None),
            show_feeds: Mutex::new(None),
            movie_feeds: Mutex::new(None),
            account: AtomicU64::new(0),
            items_generation: AtomicU64::new(0),
            people: Mutex::new(HashMap::new()),
            calendar: Mutex::new(Vec::new()),
            alerts: Mutex::new(read(&dir, ALERTS_FILE).unwrap_or_default()),
            scheduled: Mutex::new(None),
            dir,
        }
    }

    fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        // Write then rename so a crash never leaves a half-written file.
        let tmp = self.dir.join(format!("{name}.tmp"));
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.dir.join(name)).map_err(|e| e.to_string())
    }

    pub fn save_token(&self, token: &nextep_core::trakt::Token) -> Result<(), String> {
        self.save(TOKEN_FILE, token)
    }

    pub async fn save_labels(&self, labels: &Labels) -> Result<(), String> {
        self.save(LABELS_FILE, labels)
    }

    /// Saves the last good result of a screen so the next launch can show it
    /// instantly (and offline) while fresh data loads.
    pub fn save_snapshot<T: Serialize>(&self, name: &str, value: &T) {
        let _ = self.save(&format!("{SNAPSHOT_PREFIX}{name}.json"), value);
    }

    pub fn snapshot<T: DeserializeOwned>(&self, name: &str) -> Result<T, String> {
        read(&self.dir, &format!("{SNAPSHOT_PREFIX}{name}.json"))
            .ok_or_else(|| "no_snapshot".into())
    }

    pub async fn ai_config(&self) -> Option<AiConfig> {
        self.ai.lock().await.clone()
    }

    /// `None` removes the saved provider and key.
    pub async fn set_ai_config(&self, config: Option<AiConfig>) -> Result<(), String> {
        match &config {
            Some(config) => self.save(AI_FILE, config)?,
            None => {
                let _ = fs::remove_file(self.dir.join(AI_FILE));
            }
        }
        *self.ai.lock().await = config;
        Ok(())
    }

    pub fn tmdb(&self) -> Option<&TmdbClient> {
        self.tmdb.as_ref()
    }

    /// Signing out forgets the account and its cached data. Labels live on
    /// Trakt, so signing back in restores them.
    pub async fn sign_out(&self) {
        self.trakt.lock().await.set_token(None);
        *self.progress.lock().await = ProgressCache::default();
        *self.labels.lock().await = Labels::default();
        *self.items.lock().await = None;
        *self.watchlist.lock().await = None;
        *self.lists.lock().await = None;
        self.list_members.lock().await.clear();
        *self.movies.lock().await = None;
        self.account.fetch_add(1, Ordering::SeqCst);
        self.items_generation.fetch_add(1, Ordering::SeqCst);
        *self.show_feeds.lock().await = None;
        *self.movie_feeds.lock().await = None;
        self.people.lock().await.clear();
        self.calendar.lock().await.clear();
        for name in [TOKEN_FILE, PROGRESS_FILE, LABELS_FILE] {
            let _ = fs::remove_file(self.dir.join(name));
        }
        if let Ok(entries) = fs::read_dir(&self.dir) {
            for entry in entries.flatten() {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(SNAPSHOT_PREFIX)
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }

    /// A client with a usable access token, refreshing it when it expires
    /// within a day (tokens last 7 days).
    pub async fn signed_in_client(&self) -> Result<TraktClient, String> {
        let mut trakt = self.trakt.lock().await;
        let token = trakt.token().cloned().ok_or(NOT_SIGNED_IN)?;
        if token.needs_refresh(Utc::now(), Duration::days(1)) {
            match trakt.refresh_token(&token.refresh_token).await {
                Ok(fresh) => {
                    // Refresh tokens are single-use: persist before anything else.
                    self.save_token(&fresh)?;
                    trakt.set_token(Some(fresh));
                }
                Err(Error::Api {
                    status: 400 | 401, ..
                })
                | Err(Error::NotAuthenticated) => {
                    drop(trakt);
                    self.sign_out().await;
                    return Err(NOT_SIGNED_IN.into());
                }
                Err(e) => return Err(to_ui_error(e)),
            }
        }
        Ok(trakt.clone())
    }

    /// Every watched show with its progress and poster. Reuses a refresh from
    /// the last couple of minutes unless `force` is set. Also clears the
    /// labels of shows that were watched again.
    pub async fn items(&self, force: bool) -> Result<Vec<ProgressItem>, String> {
        if !force
            && let Some((at, items)) = &*self.items.lock().await
            && Utc::now() - *at < Duration::seconds(REUSE_SECS)
        {
            return Ok(items.clone());
        }

        let client = self.signed_in_client().await?;
        let now = Utc::now();
        let generation = self.items_generation.load(Ordering::SeqCst);
        let mut items = {
            let mut cache = self.progress.lock().await;
            let (items, calendar) = sync::refresh_progress(&client, &mut cache, now, force)
                .await
                .map_err(to_ui_error)?;
            self.save(PROGRESS_FILE, &*cache)?;
            *self.calendar.lock().await = calendar;
            items
        };

        // Labels live on Trakt; refresh them along with the progress. If that
        // fails, the copy on the device is used.
        let hidden = futures::try_join!(
            client.hidden_shows(ShowStatus::OnHold.trakt_section()),
            client.hidden_shows(ShowStatus::Dropped.trakt_section()),
        );
        let mut labels = self.labels.lock().await;
        if let Ok((on_hold, dropped)) = hidden {
            *labels = Labels::from_trakt(&on_hold, &dropped, &labels, now);
        }
        for (show_id, status) in labels.clear_rewatched(&items) {
            // If this fails the show stays hidden on Trakt and the next
            // refresh clears it again.
            let _ = client.unhide_show(status.trakt_section(), show_id).await;
        }
        self.save_labels(&labels).await?;
        drop(labels);

        self.fetch_artwork(items.iter().filter_map(|i| i.show.ids.tmdb))
            .await;
        let artwork = self.artwork.lock().await;
        for item in &mut items {
            item.poster = artwork.poster_url(item.show.ids.tmdb, Size::Thumb);
        }
        drop(artwork);

        // An episode marked (or a sign-out) while this ran makes it out of
        // date: return it to this caller, but don't keep it for others.
        if self.items_generation.load(Ordering::SeqCst) == generation {
            self.save_snapshot("items", &items);
            *self.items.lock().await = Some((now, items.clone()));
        }
        Ok(items)
    }

    /// Trakt ids of watched shows, from the progress cache (no API call).
    pub async fn watched_ids(&self) -> HashSet<u64> {
        self.progress.lock().await.show_ids().collect()
    }

    /// Trakt ids on the watchlist, reusing a fetch from the last couple of
    /// minutes.
    /// Whether a show is on the watchlist, from the last fetch (however old);
    /// `None` if it hasn't been fetched.
    pub async fn on_watchlist_cached(&self, show_id: u64) -> Option<bool> {
        let watchlist = self.watchlist.lock().await;
        watchlist.as_ref().map(|(_, ids)| ids.contains(&show_id))
    }

    /// [`Self::on_watchlist_cached`] for movies.
    pub async fn movie_on_watchlist_cached(&self, movie_id: u64) -> Option<bool> {
        let movies = self.movies.lock().await;
        movies.as_ref().map(|(_, (_, watchlist))| {
            watchlist
                .iter()
                .any(|w| w.movie.ids.trakt == Some(movie_id))
        })
    }

    pub async fn watchlist_ids(&self, client: &TraktClient) -> Result<HashSet<u64>, String> {
        if let Some((at, ids)) = &*self.watchlist.lock().await
            && Utc::now() - *at < Duration::seconds(REUSE_SECS)
        {
            return Ok(ids.clone());
        }
        let list = client.watchlist_shows().await.map_err(to_ui_error)?;
        let ids: HashSet<u64> = list.iter().filter_map(|w| w.show.ids.trakt).collect();
        self.remember_watchlist(ids.clone()).await;
        Ok(ids)
    }

    pub async fn remember_watchlist(&self, ids: HashSet<u64>) {
        *self.watchlist.lock().await = Some((Utc::now(), ids));
    }

    pub async fn update_watchlist(&self, show_id: u64, on: bool) {
        if let Some((_, ids)) = &mut *self.watchlist.lock().await {
            if on {
                ids.insert(show_id);
            } else {
                ids.remove(&show_id);
            }
        }
    }

    /// Personal lists, reusing a fetch from the last couple of minutes unless
    /// `force` is set.
    pub async fn lists(&self, client: &TraktClient, force: bool) -> Result<Vec<TraktList>, String> {
        if !force
            && let Some((at, lists)) = &*self.lists.lock().await
            && Utc::now() - *at < Duration::seconds(REUSE_SECS)
        {
            return Ok(lists.clone());
        }
        let lists = client.my_lists().await.map_err(to_ui_error)?;
        *self.lists.lock().await = Some((Utc::now(), lists.clone()));
        if force {
            self.list_members.lock().await.clear();
        }
        Ok(lists)
    }

    /// Lists changed (created, renamed, deleted): fetch them again next time.
    pub async fn invalidate_lists(&self) {
        *self.lists.lock().await = None;
    }

    /// Trakt ids of the shows on a list, fetched once and then kept current
    /// by [`Self::update_list_member`].
    pub async fn list_members(
        &self,
        client: &TraktClient,
        list_id: u64,
    ) -> Result<HashSet<ItemId>, String> {
        if let Some(ids) = self.list_members.lock().await.get(&list_id) {
            return Ok(ids.clone());
        }
        let items = client.list_items(list_id).await.map_err(to_ui_error)?;
        let ids: HashSet<ItemId> = items.iter().filter_map(ListItem::id).collect();
        self.remember_list_members(list_id, ids.clone()).await;
        Ok(ids)
    }

    pub async fn remember_list_members(&self, list_id: u64, ids: HashSet<ItemId>) {
        self.list_members.lock().await.insert(list_id, ids);
    }

    pub async fn update_list_member(&self, list_id: u64, item: ItemId, on: bool) {
        if let Some(ids) = self.list_members.lock().await.get_mut(&list_id) {
            if on {
                ids.insert(item);
            } else {
                ids.remove(&item);
            }
        }
    }

    /// Watched movies and the movie watchlist (two calls), reusing a fetch
    /// from the last couple of minutes unless `force` is set.
    pub async fn movie_data(&self, client: &TraktClient, force: bool) -> Result<MovieData, String> {
        if !force
            && let Some((at, data)) = &*self.movies.lock().await
            && Utc::now() - *at < Duration::seconds(REUSE_SECS)
        {
            return Ok(data.clone());
        }
        let data = futures::try_join!(client.watched_movies(), client.watchlist_movies())
            .map_err(to_ui_error)?;
        *self.movies.lock().await = Some((Utc::now(), data.clone()));
        Ok(data)
    }

    /// The show feeds fetched in the last half hour; empty if `force` is set
    /// or they are older. Pass the ticket back with what was fetched.
    pub async fn show_feeds(&self, force: bool) -> (FeedsTicket, Feeds<Show>) {
        self.cached_feeds(&self.show_feeds, force).await
    }

    pub async fn remember_show_feeds(&self, ticket: FeedsTicket, feeds: Feeds<Show>) {
        self.remember_feeds(&self.show_feeds, ticket, feeds).await;
    }

    /// [`Self::show_feeds`] for movies.
    pub async fn movie_feeds(&self, force: bool) -> (FeedsTicket, Feeds<Movie>) {
        self.cached_feeds(&self.movie_feeds, force).await
    }

    pub async fn remember_movie_feeds(&self, ticket: FeedsTicket, feeds: Feeds<Movie>) {
        self.remember_feeds(&self.movie_feeds, ticket, feeds).await;
    }

    async fn cached_feeds<T: Clone>(
        &self,
        slot: &FeedSlot<T>,
        force: bool,
    ) -> (FeedsTicket, Feeds<T>) {
        let account = self.account.load(Ordering::SeqCst);
        let mut slot = slot.lock().await;
        match &*slot {
            Some((at, feeds))
                if !force && Utc::now() - *at < Duration::seconds(FEEDS_REUSE_SECS) =>
            {
                (FeedsTicket { account, at: *at }, feeds.clone())
            }
            _ => {
                // Start afresh; later fetches from this one merge in.
                let at = Utc::now();
                *slot = Some((at, Feeds::default()));
                (FeedsTicket { account, at }, Feeds::default())
            }
        }
    }

    /// Merges what a fetch found into the kept feeds, unless they were reset
    /// (pull to refresh, sign-out) since it started.
    async fn remember_feeds<T>(&self, slot: &FeedSlot<T>, ticket: FeedsTicket, feeds: Feeds<T>) {
        if self.account.load(Ordering::SeqCst) != ticket.account {
            return;
        }
        if let Some((at, kept)) = &mut *slot.lock().await
            && *at == ticket.at
        {
            kept.merge(feeds);
        }
    }

    pub async fn invalidate_movies(&self) {
        *self.movies.lock().await = None;
    }

    /// A watched show from the latest refresh, without calling Trakt.
    pub async fn cached_show(&self, show_id: u64) -> Option<nextep_core::models::Show> {
        let items = self.items.lock().await;
        let (_, items) = items.as_ref()?;
        items
            .iter()
            .find(|i| i.show.ids.trakt == Some(show_id))
            .map(|i| i.show.clone())
    }

    /// Forces the next [`Self::items`] call to ask Trakt again.
    pub async fn invalidate(&self) {
        self.items_generation.fetch_add(1, Ordering::SeqCst);
        *self.items.lock().await = None;
    }

    pub async fn alerts_enabled(&self) -> bool {
        self.alerts.lock().await.enabled
    }

    pub async fn set_alerts_enabled(&self, enabled: bool) -> Result<(), String> {
        let mut alerts = self.alerts.lock().await;
        alerts.enabled = enabled;
        self.save(ALERTS_FILE, &*alerts)
    }

    /// New-episode alerts for the latest refresh (none when they're off).
    pub async fn alert_plan(&self) -> Vec<Alert> {
        if !self.alerts_enabled().await {
            return Vec::new();
        }
        let items = self.items.lock().await;
        let Some((_, items)) = items.as_ref() else {
            return Vec::new();
        };
        let calendar = self.calendar.lock().await;
        let labels = self.labels.lock().await;
        alerts::plan(items, &calendar, &labels, Utc::now())
    }

    /// Records `plan` as scheduled; false if it's what was scheduled last.
    pub async fn note_scheduled(&self, plan: &[Alert]) -> bool {
        let mut scheduled = self.scheduled.lock().await;
        if scheduled.as_deref() == Some(plan) {
            return false;
        }
        *scheduled = Some(plan.to_vec());
        true
    }

    /// Makes the next refresh fetch this show's progress from Trakt.
    pub async fn mark_progress_stale(&self, show_id: u64) {
        self.progress.lock().await.mark_stale(show_id);
    }

    /// Looks up posters for shows that don't have one cached yet.
    pub async fn fetch_artwork(&self, tmdb_ids: impl IntoIterator<Item = u64>) {
        let Some(tmdb) = &self.tmdb else { return };
        let mut artwork = self.artwork.lock().await;
        if artwork.fill(tmdb, Media::Tv, tmdb_ids, Utc::now()).await {
            let _ = self.save(ARTWORK_FILE, &*artwork);
        }
    }

    /// Looks up posters for movies that don't have one cached yet.
    pub async fn fetch_movie_artwork(&self, tmdb_ids: impl IntoIterator<Item = u64>) {
        let Some(tmdb) = &self.tmdb else { return };
        let mut artwork = self.artwork.lock().await;
        if artwork.fill(tmdb, Media::Movie, tmdb_ids, Utc::now()).await {
            let _ = self.save(ARTWORK_FILE, &*artwork);
        }
    }
}

fn read<T: DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    serde_json::from_slice(&fs::read(dir.join(name)).ok()?).ok()
}

pub fn to_ui_error(e: Error) -> String {
    match e {
        Error::NotAuthenticated => NOT_SIGNED_IN.into(),
        // reqwest's top-level message hides the cause; include the chain.
        Error::Http(e) => {
            let mut message = String::from("Network error");
            let mut source: Option<&dyn std::error::Error> = Some(&e);
            while let Some(err) = source {
                message.push_str(&format!(": {err}"));
                source = err.source();
            }
            message
        }
        e => e.to_string(),
    }
}
