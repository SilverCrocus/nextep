mod commands;
mod installer;
mod movies;
mod state;

use std::fs;

use tauri::Manager;

use crate::state::AppState;

const TRAKT_CLIENT_ID: Option<&str> = option_env!("TRAKT_CLIENT_ID");
const TMDB_API_KEY: Option<&str> = option_env!("TMDB_API_KEY");

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(installer::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            app.manage(AppState::load(
                dir,
                TRAKT_CLIENT_ID.unwrap_or_default(),
                TMDB_API_KEY,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_status,
            commands::sign_in_start,
            commands::sign_in_poll,
            commands::sign_out,
            commands::open_link,
            commands::check_update,
            installer::install_update,
            movies::movies,
            movies::movie_details,
            movies::mark_movie_watched,
            movies::set_movie_watchlist,
            commands::progress,
            commands::mark_watched,
            commands::set_show_status,
            commands::calendar,
            commands::history,
            commands::stats,
            commands::discover,
            commands::ai_settings,
            commands::ai_models,
            commands::save_ai_settings,
            commands::clear_ai_key,
            commands::ai_picks,
            commands::lists,
            commands::list_detail,
            commands::create_list,
            commands::rename_list,
            commands::delete_list,
            commands::set_in_list,
            commands::show_lists,
            commands::library,
            commands::show_details,
            commands::search,
            commands::set_watchlist,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
