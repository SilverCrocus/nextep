use std::fs;
use std::path::{Path, PathBuf};

use nextep_core::chrono::{Duration, Utc};
use nextep_core::progress::{self, ProgressOptions, ProgressView, SortBy};
use nextep_core::sync::{self, ProgressCache};
use nextep_core::trakt::{DeviceCode, DevicePoll};
use nextep_core::{Error, TraktClient};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;

const TRAKT_CLIENT_ID: Option<&str> = option_env!("TRAKT_CLIENT_ID");
const TMDB_API_KEY: Option<&str> = option_env!("TMDB_API_KEY");

const TOKEN_FILE: &str = "trakt_token.json";
const CACHE_FILE: &str = "progress_cache.json";

/// Error string the UI checks for to show the sign-in screen.
const NOT_SIGNED_IN: &str = "not_signed_in";

struct AppState {
    dir: PathBuf,
    trakt: Mutex<TraktClient>,
    cache: Mutex<ProgressCache>,
}

impl AppState {
    fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let json = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        // Write then rename so a crash never leaves a half-written file.
        let tmp = self.dir.join(format!("{name}.tmp"));
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.dir.join(name)).map_err(|e| e.to_string())
    }

    async fn sign_out(&self) {
        self.trakt.lock().await.set_token(None);
        *self.cache.lock().await = ProgressCache::default();
        for name in [TOKEN_FILE, CACHE_FILE] {
            let _ = fs::remove_file(self.dir.join(name));
        }
    }

    /// A client with a usable access token, refreshing it when it expires
    /// within a day (tokens last 7 days).
    async fn signed_in_client(&self) -> Result<TraktClient, String> {
        let mut trakt = self.trakt.lock().await;
        let token = trakt.token().cloned().ok_or(NOT_SIGNED_IN)?;
        if token.needs_refresh(Utc::now(), Duration::days(1)) {
            match trakt.refresh_token(&token.refresh_token).await {
                Ok(fresh) => {
                    // Refresh tokens are single-use: persist before anything else.
                    self.save(TOKEN_FILE, &fresh)?;
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
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(trakt.clone())
    }
}

fn load<T: DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    serde_json::from_slice(&fs::read(dir.join(name)).ok()?).ok()
}

fn to_ui_error(e: Error) -> String {
    match e {
        Error::NotAuthenticated => NOT_SIGNED_IN.into(),
        e => e.to_string(),
    }
}

#[derive(Serialize)]
struct AppStatus {
    version: &'static str,
    trakt_configured: bool,
    tmdb_configured: bool,
    signed_in: bool,
}

#[tauri::command]
async fn app_status(state: State<'_, AppState>) -> Result<AppStatus, String> {
    Ok(AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        trakt_configured: TRAKT_CLIENT_ID.is_some(),
        tmdb_configured: TMDB_API_KEY.is_some(),
        signed_in: state.trakt.lock().await.token().is_some(),
    })
}

/// Starts the device flow: the UI shows `user_code` and opens `verification_url`.
#[tauri::command]
async fn sign_in_start(state: State<'_, AppState>) -> Result<DeviceCode, String> {
    let client = state.trakt.lock().await.clone();
    client.device_code().await.map_err(to_ui_error)
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum SignInPoll {
    Pending,
    SlowDown,
    SignedIn,
    Denied,
    Expired,
}

#[tauri::command]
async fn sign_in_poll(
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
                state.save(TOKEN_FILE, &token)?;
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
async fn sign_out(state: State<'_, AppState>) -> Result<(), String> {
    state.sign_out().await;
    Ok(())
}

/// Opens a Trakt page in the system browser. Limited to Trakt so the UI can't
/// be used to open arbitrary links.
#[tauri::command]
fn open_trakt(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://auth.trakt.tv/") || url.starts_with("https://trakt.tv/")) {
        return Err("only Trakt links can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn progress(
    state: State<'_, AppState>,
    sort: Option<SortBy>,
) -> Result<ProgressView, String> {
    let client = state.signed_in_client().await?;
    let now = Utc::now();
    let mut cache = state.cache.lock().await;
    let items = sync::refresh_progress(&client, &mut cache, now)
        .await
        .map_err(to_ui_error)?;
    state.save(CACHE_FILE, &*cache)?;
    let opts = ProgressOptions {
        sort: sort.unwrap_or_default(),
        ..Default::default()
    };
    Ok(progress::arrange(items, &opts, now))
}

#[tauri::command]
async fn mark_watched(state: State<'_, AppState>, episode_id: u64) -> Result<(), String> {
    let client = state.signed_in_client().await?;
    client
        .mark_episode_watched(episode_id, Utc::now())
        .await
        .map_err(to_ui_error)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            let mut trakt = TraktClient::new(TRAKT_CLIENT_ID.unwrap_or_default());
            trakt.set_token(load(&dir, TOKEN_FILE));
            let cache = load(&dir, CACHE_FILE).unwrap_or_default();
            app.manage(AppState {
                dir,
                trakt: Mutex::new(trakt),
                cache: Mutex::new(cache),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_status,
            sign_in_start,
            sign_in_poll,
            sign_out,
            open_trakt,
            progress,
            mark_watched,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
