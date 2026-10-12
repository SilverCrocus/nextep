//! Minimal Trakt API v2 client covering what Nextep needs: sign-in, watched
//! progress, the episode calendar, the watchlist and marking episodes watched.
//!
//! Docs: <https://developer.trakt.tv/>. Spec: <https://github.com/trakt/trakt-api>.
//!
//! Nextep is a public client: it only has a Client ID. Trakt's `client_secret`
//! is deprecated for apps that run on the user's device and is never sent.

mod auth;

pub use auth::{DeviceCode, DevicePoll, Token};

use chrono::{DateTime, NaiveDate, Utc};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::models::{CalendarEntry, Season, Show, ShowProgress, WatchedShow, WatchlistShow};
use crate::{Error, Result};

pub const API_URL: &str = "https://api.trakt.tv";
/// Every OAuth request (device codes, token refresh, revoke) goes here.
pub const AUTH_URL: &str = "https://auth.trakt.tv";
/// Registered in the Trakt app settings. The device flow never opens it, but
/// token refresh must send a URI that matches the settings exactly.
pub const REDIRECT_URI: &str = "https://silvercrocus.github.io/nextep/callback";

const USER_AGENT: &str = concat!("Nextep/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct TraktClient {
    http: reqwest::Client,
    api_url: String,
    auth_url: String,
    client_id: String,
    token: Option<Token>,
}

impl TraktClient {
    pub fn new(client_id: impl Into<String>) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert("trakt-api-version", HeaderValue::from_static("2"));
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            // Android freezes backgrounded apps; don't reuse connections the
            // server has long since closed.
            .pool_idle_timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("static reqwest config is valid");
        Self {
            http,
            api_url: API_URL.to_owned(),
            auth_url: AUTH_URL.to_owned(),
            client_id: client_id.into(),
            token: None,
        }
    }

    /// Points API and OAuth requests at another host, e.g. a local mock in tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.api_url = base_url.into();
        self.auth_url = self.api_url.clone();
        self
    }

    pub fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    pub fn set_token(&mut self, token: Option<Token>) {
        self.token = token;
    }

    // ---- auth -------------------------------------------------------------

    pub async fn device_code(&self) -> Result<DeviceCode> {
        let req = self
            .oauth("/oauth/device/code")
            .json(&json!({ "client_id": self.client_id }));
        send_json(req).await
    }

    pub async fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll> {
        let res = self
            .oauth("/oauth/device/token")
            .json(&json!({ "code": device_code, "client_id": self.client_id }))
            .send()
            .await?;
        let status = res.status();
        if status == StatusCode::OK {
            return Ok(DevicePoll::Authorized(res.json().await?));
        }
        DevicePoll::from_status(status.as_u16()).ok_or(Error::Api {
            status: status.as_u16(),
            body: res.text().await.unwrap_or_default(),
        })
    }

    /// Refresh tokens are single-use: persist the returned token before doing
    /// anything else, or the user has to sign in again.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Token> {
        let req = self.oauth("/oauth/token").json(&json!({
            "refresh_token": refresh_token,
            "client_id": self.client_id,
            "redirect_uri": REDIRECT_URI,
            "grant_type": "refresh_token",
        }));
        send_json(req).await
    }

    // ---- sync -------------------------------------------------------------

    /// Every show the user has watched at least one episode of.
    pub async fn watched_shows(&self) -> Result<Vec<WatchedShow>> {
        send_json(self.authed(Method::GET, "/sync/watched/shows?extended=noseasons,full")?).await
    }

    /// Watched/aired counts and the next aired episode to watch for one show,
    /// counted from the last episode the user watched.
    pub async fn show_progress(&self, show_trakt_id: u64) -> Result<ShowProgress> {
        let path = format!(
            "/shows/{show_trakt_id}/progress/watched?hidden=false&specials=false&count_specials=false&last_activity=watched&extended=full"
        );
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn show(&self, show_trakt_id: u64) -> Result<Show> {
        send_json(self.authed(
            Method::GET,
            &format!("/shows/{show_trakt_id}?extended=full"),
        )?)
        .await
    }

    /// All seasons with their episodes (titles and air dates). Large; only
    /// for the show page.
    pub async fn seasons(&self, show_trakt_id: u64) -> Result<Vec<Season>> {
        let path = format!("/shows/{show_trakt_id}/seasons?extended=full,episodes");
        send_json(self.authed(Method::GET, &path)?).await
    }

    /// Episodes airing for shows the user follows, starting at `start`.
    pub async fn calendar(&self, start: NaiveDate, days: u32) -> Result<Vec<CalendarEntry>> {
        let path = format!("/calendars/my/shows/{}/{days}", start.format("%Y-%m-%d"));
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn watchlist_shows(&self) -> Result<Vec<WatchlistShow>> {
        send_json(self.authed(Method::GET, "/sync/watchlist/shows?extended=full")?).await
    }

    pub async fn mark_episode_watched(
        &self,
        episode_trakt_id: u64,
        watched_at: DateTime<Utc>,
    ) -> Result<()> {
        let req = self.authed(Method::POST, "/sync/history")?.json(&json!({
            "episodes": [{
                "ids": { "trakt": episode_trakt_id },
                "watched_at": watched_at.to_rfc3339(),
            }]
        }));
        check(req.send().await?).await.map(drop)
    }

    // ---- plumbing ---------------------------------------------------------

    fn oauth(&self, path: &str) -> RequestBuilder {
        self.http.post(format!("{}{path}", self.auth_url))
    }

    fn authed(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let token = self.token.as_ref().ok_or(Error::NotAuthenticated)?;
        Ok(self
            .http
            .request(method, format!("{}{path}", self.api_url))
            .header("trakt-api-key", &self.client_id)
            .bearer_auth(&token.access_token))
    }
}

async fn check(res: reqwest::Response) -> Result<reqwest::Response> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    if status == StatusCode::UNAUTHORIZED {
        return Err(Error::NotAuthenticated);
    }
    Err(Error::Api {
        status: status.as_u16(),
        body: res.text().await.unwrap_or_default(),
    })
}

async fn send_json<T: DeserializeOwned>(req: RequestBuilder) -> Result<T> {
    Ok(check(req.send().await?).await?.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authed_calls_require_a_token() {
        let client = TraktClient::new("id");
        assert!(matches!(
            client.watched_shows().await,
            Err(Error::NotAuthenticated)
        ));
    }
}
