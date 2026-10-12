//! Minimal Trakt API v2 client covering what Nextep needs: sign-in, watched
//! progress, the episode calendar, the watchlist and marking episodes watched.
//!
//! Docs: <https://trakt.docs.apiary.io/>

mod auth;

pub use auth::{DeviceCode, DevicePoll, Token};

use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{Method, RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::models::{CalendarEntry, ShowProgress, WatchedShow, WatchlistShow};
use crate::{Error, Result};

pub const API_URL: &str = "https://api.trakt.tv";
const USER_AGENT: &str = concat!(
    "Nextep/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/SilverCrocus/nextep)"
);

#[derive(Debug, Clone)]
pub struct TraktClient {
    http: reqwest::Client,
    base_url: String,
    client_id: String,
    client_secret: String,
    token: Option<Token>,
}

impl TraktClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .build()
            .expect("static reqwest config is valid");
        Self {
            http,
            base_url: API_URL.to_owned(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token: None,
        }
    }

    /// Points the client at another host, e.g. a local mock in tests.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
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
            .request(Method::POST, "/oauth/device/code")
            .json(&json!({ "client_id": self.client_id }));
        send_json(req).await
    }

    pub async fn poll_device_token(&self, device_code: &str) -> Result<DevicePoll> {
        let res = self
            .request(Method::POST, "/oauth/device/token")
            .json(&json!({
                "code": device_code,
                "client_id": self.client_id,
                "client_secret": self.client_secret,
            }))
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

    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Token> {
        let req = self.request(Method::POST, "/oauth/token").json(&json!({
            "refresh_token": refresh_token,
            "client_id": self.client_id,
            "client_secret": self.client_secret,
            "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
            "grant_type": "refresh_token",
        }));
        send_json(req).await
    }

    // ---- sync -------------------------------------------------------------

    /// Every show the user has watched at least one episode of.
    pub async fn watched_shows(&self) -> Result<Vec<WatchedShow>> {
        send_json(self.authed(Method::GET, "/sync/watched/shows?extended=noseasons,full")?).await
    }

    /// Watched/aired counts and the next episode to watch for one show.
    pub async fn show_progress(&self, show_trakt_id: u64) -> Result<ShowProgress> {
        let path = format!(
            "/shows/{show_trakt_id}/progress/watched?hidden=false&specials=false&count_specials=false"
        );
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

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.client_id)
    }

    fn authed(&self, method: Method, path: &str) -> Result<RequestBuilder> {
        let token = self.token.as_ref().ok_or(Error::NotAuthenticated)?;
        Ok(self.request(method, path).bearer_auth(&token.access_token))
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
        let client = TraktClient::new("id", "secret");
        assert!(matches!(
            client.watched_shows().await,
            Err(Error::NotAuthenticated)
        ));
    }
}
