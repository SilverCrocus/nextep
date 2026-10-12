use serde::Serialize;

const TRAKT_CLIENT_ID: Option<&str> = option_env!("TRAKT_CLIENT_ID");
const TRAKT_CLIENT_SECRET: Option<&str> = option_env!("TRAKT_CLIENT_SECRET");
const TMDB_API_KEY: Option<&str> = option_env!("TMDB_API_KEY");

#[derive(Serialize)]
struct AppStatus {
    version: &'static str,
    trakt_configured: bool,
    tmdb_configured: bool,
}

/// Lets the UI explain what is missing before the user tries to sign in.
#[tauri::command]
fn app_status() -> AppStatus {
    AppStatus {
        version: env!("CARGO_PKG_VERSION"),
        trakt_configured: TRAKT_CLIENT_ID.is_some() && TRAKT_CLIENT_SECRET.is_some(),
        tmdb_configured: TMDB_API_KEY.is_some(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![app_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
