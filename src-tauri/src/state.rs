//! App state: the Trakt and TMDB clients plus everything persisted in the
//! app's private data directory.

use std::fs;
use std::path::{Path, PathBuf};

use nextep_core::chrono::{DateTime, Duration, Utc};
use nextep_core::labels::Labels;
use nextep_core::progress::ProgressItem;
use nextep_core::sync::{self, ProgressCache};
use nextep_core::tmdb::{ArtworkCache, Size, TmdbClient};
use nextep_core::{Error, TraktClient};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tauri::async_runtime::Mutex;

/// Error string the UI checks for to show the sign-in screen.
pub const NOT_SIGNED_IN: &str = "not_signed_in";

const TOKEN_FILE: &str = "trakt_token.json";
const PROGRESS_FILE: &str = "progress_cache.json";
const LABELS_FILE: &str = "labels.json";
const ARTWORK_FILE: &str = "artwork_cache.json";

/// Switching between tabs reuses a refresh this recent instead of calling
/// Trakt again.
const REUSE_SECS: i64 = 120;

pub struct AppState {
    dir: PathBuf,
    pub trakt: Mutex<TraktClient>,
    tmdb: Option<TmdbClient>,
    progress: Mutex<ProgressCache>,
    pub labels: Mutex<Labels>,
    pub artwork: Mutex<ArtworkCache>,
    /// Latest refresh, shared by Progress and Library.
    items: Mutex<Option<(DateTime<Utc>, Vec<ProgressItem>)>>,
}

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

    /// Signing out forgets the account and its cached data. Labels stay, so
    /// signing back in restores them.
    pub async fn sign_out(&self) {
        self.trakt.lock().await.set_token(None);
        *self.progress.lock().await = ProgressCache::default();
        *self.items.lock().await = None;
        for name in [TOKEN_FILE, PROGRESS_FILE] {
            let _ = fs::remove_file(self.dir.join(name));
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
        let mut items = {
            let mut cache = self.progress.lock().await;
            let items = sync::refresh_progress(&client, &mut cache, now)
                .await
                .map_err(to_ui_error)?;
            self.save(PROGRESS_FILE, &*cache)?;
            items
        };

        let mut labels = self.labels.lock().await;
        if labels.clear_rewatched(&items) {
            self.save_labels(&labels).await?;
        }
        drop(labels);

        self.fetch_artwork(items.iter().filter_map(|i| i.show.ids.tmdb))
            .await;
        let artwork = self.artwork.lock().await;
        for item in &mut items {
            item.poster = artwork.poster_url(item.show.ids.tmdb, Size::Thumb);
        }
        drop(artwork);

        *self.items.lock().await = Some((now, items.clone()));
        Ok(items)
    }

    /// Forces the next [`Self::items`] call to ask Trakt again.
    pub async fn invalidate(&self) {
        *self.items.lock().await = None;
    }

    /// Looks up posters for shows that don't have one cached yet.
    pub async fn fetch_artwork(&self, tmdb_ids: impl IntoIterator<Item = u64>) {
        let Some(tmdb) = &self.tmdb else { return };
        let mut artwork = self.artwork.lock().await;
        if artwork.fill(tmdb, tmdb_ids, Utc::now()).await {
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
