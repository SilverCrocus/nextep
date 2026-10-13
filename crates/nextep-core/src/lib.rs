//! Platform-independent core of Nextep.
//!
//! Everything that does not touch the UI lives here so it can be unit tested on
//! the host and reused by the Tauri shell on Android.

pub mod ai;
pub mod ai_context;
pub mod alerts;
pub mod labels;
pub mod models;
pub mod progress;
pub mod sync;
pub mod taste;
pub mod tmdb;
pub mod trakt;
pub mod update;

pub use chrono;
pub use trakt::TraktClient;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Trakt returned {status}: {body}")]
    Api { status: u16, body: String },

    #[error("not signed in to Trakt")]
    NotAuthenticated,
}

pub type Result<T> = std::result::Result<T, Error>;
