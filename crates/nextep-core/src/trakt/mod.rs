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

use crate::models::{
    CalendarEntry, CalendarMovie, HiddenShow, HistoryEntry, ListedShow, Movie, MovieSearchResult,
    SearchResult, Season, Show, ShowPeople, ShowProgress, TraktList, TrendingMovie, TrendingShow,
    UserSettings, UserStats, WatchedMovie, WatchedShow, WatchlistMovie, WatchlistShow,
};
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

    pub async fn user_stats(&self) -> Result<UserStats> {
        send_json(self.authed(Method::GET, "/users/me/stats")?).await
    }

    /// The most recently watched episodes, newest first.
    pub async fn episode_history(&self, limit: u32) -> Result<Vec<HistoryEntry>> {
        let path = format!("/sync/history/episodes?limit={limit}&extended=full");
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
        let body = json!({
            "episodes": [{
                "ids": { "trakt": episode_trakt_id },
                "watched_at": watched_at.to_rfc3339(),
            }]
        });
        self.post("/sync/history", body).await
    }

    pub async fn add_to_watchlist(&self, show_trakt_id: u64) -> Result<()> {
        self.post("/sync/watchlist", shows_body(show_trakt_id))
            .await
    }

    pub async fn remove_from_watchlist(&self, show_trakt_id: u64) -> Result<()> {
        self.post("/sync/watchlist/remove", shows_body(show_trakt_id))
            .await
    }

    // ---- hidden items -----------------------------------------------------

    /// Shows hidden in one of Trakt's hidden sections (`progress_watched`,
    /// `dropped`, ...), across all pages.
    pub async fn hidden_shows(&self, section: &str) -> Result<Vec<HiddenShow>> {
        const PAGE_SIZE: usize = 250;
        let mut all = Vec::new();
        for page in 1..=20 {
            let path = format!(
                "/users/hidden/{section}?type=show&extended=full&limit={PAGE_SIZE}&page={page}"
            );
            let batch: Vec<HiddenShow> = send_json(self.authed(Method::GET, &path)?).await?;
            let last = batch.len() < PAGE_SIZE;
            all.extend(batch);
            if last {
                break;
            }
        }
        Ok(all)
    }

    pub async fn hide_show(&self, section: &str, show_trakt_id: u64) -> Result<()> {
        self.post(
            &format!("/users/hidden/{section}"),
            shows_body(show_trakt_id),
        )
        .await
    }

    pub async fn unhide_show(&self, section: &str, show_trakt_id: u64) -> Result<()> {
        let path = format!("/users/hidden/{section}/remove");
        self.post(&path, shows_body(show_trakt_id)).await
    }

    // ---- movies -----------------------------------------------------------

    pub async fn watched_movies(&self) -> Result<Vec<WatchedMovie>> {
        send_json(self.authed(Method::GET, "/sync/watched/movies?extended=full")?).await
    }

    pub async fn watchlist_movies(&self) -> Result<Vec<WatchlistMovie>> {
        send_json(self.authed(Method::GET, "/sync/watchlist/movies?extended=full")?).await
    }

    pub async fn movie(&self, movie_trakt_id: u64) -> Result<Movie> {
        send_json(self.authed(
            Method::GET,
            &format!("/movies/{movie_trakt_id}?extended=full"),
        )?)
        .await
    }

    /// Releases of movies the user follows (watchlist), starting at `start`.
    pub async fn movie_calendar(&self, start: NaiveDate, days: u32) -> Result<Vec<CalendarMovie>> {
        let path = format!(
            "/calendars/my/movies/{}/{days}?extended=full",
            start.format("%Y-%m-%d")
        );
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn search_movies(&self, query: &str) -> Result<Vec<MovieSearchResult>> {
        let req = self.authed(Method::GET, "/search/movie")?.query(&[
            ("query", query),
            ("extended", "full"),
            ("limit", "20"),
        ]);
        send_json(req).await
    }

    pub async fn mark_movie_watched(
        &self,
        movie_trakt_id: u64,
        watched_at: DateTime<Utc>,
    ) -> Result<()> {
        let body = json!({
            "movies": [{ "ids": { "trakt": movie_trakt_id }, "watched_at": watched_at.to_rfc3339() }]
        });
        self.post("/sync/history", body).await
    }

    pub async fn add_movie_to_watchlist(&self, movie_trakt_id: u64) -> Result<()> {
        self.post("/sync/watchlist", movies_body(movie_trakt_id))
            .await
    }

    pub async fn remove_movie_from_watchlist(&self, movie_trakt_id: u64) -> Result<()> {
        self.post("/sync/watchlist/remove", movies_body(movie_trakt_id))
            .await
    }

    pub async fn movie_people(&self, movie_trakt_id: u64) -> Result<ShowPeople> {
        send_json(self.authed(Method::GET, &format!("/movies/{movie_trakt_id}/people"))?).await
    }

    // ---- lists ------------------------------------------------------------

    pub async fn my_lists(&self) -> Result<Vec<TraktList>> {
        send_json(self.authed(Method::GET, "/users/me/lists")?).await
    }

    /// New lists are private.
    pub async fn create_list(&self, name: &str) -> Result<TraktList> {
        let body = json!({ "name": name, "privacy": "private" });
        let res = self
            .write(Method::POST, "/users/me/lists", Some(&body))
            .await?;
        Ok(res.json().await?)
    }

    pub async fn rename_list(&self, list_id: u64, name: &str) -> Result<()> {
        let body = json!({ "name": name });
        let path = format!("/users/me/lists/{list_id}");
        self.write(Method::PUT, &path, Some(&body)).await.map(drop)
    }

    pub async fn delete_list(&self, list_id: u64) -> Result<()> {
        let path = format!("/users/me/lists/{list_id}");
        self.write(Method::DELETE, &path, None).await.map(drop)
    }

    pub async fn list_shows(&self, list_id: u64) -> Result<Vec<ListedShow>> {
        let path = format!("/users/me/lists/{list_id}/items/show?extended=full&limit=all");
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn add_to_list(&self, list_id: u64, show_trakt_id: u64) -> Result<()> {
        let path = format!("/users/me/lists/{list_id}/items");
        self.post(&path, shows_body(show_trakt_id)).await
    }

    pub async fn remove_from_list(&self, list_id: u64, show_trakt_id: u64) -> Result<()> {
        let path = format!("/users/me/lists/{list_id}/items/remove");
        self.post(&path, shows_body(show_trakt_id)).await
    }

    /// How many personal lists the account may have (12 on free accounts).
    pub async fn list_limit(&self) -> Result<Option<u32>> {
        let settings: UserSettings =
            send_json(self.authed(Method::GET, "/users/settings")?).await?;
        Ok(settings.limits.and_then(|l| l.list).map(|l| l.count))
    }

    // ---- discovery --------------------------------------------------------

    /// Trakt's personalised recommendations from the user's history.
    pub async fn recommended_shows(&self, limit: u32) -> Result<Vec<Show>> {
        let path = format!(
            "/recommendations/shows?limit={limit}&ignore_collected=true&ignore_watchlisted=false&extended=full"
        );
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn related_shows(&self, show_trakt_id: u64, limit: u32) -> Result<Vec<Show>> {
        let path = format!("/shows/{show_trakt_id}/related?limit={limit}&extended=full");
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn show_people(&self, show_trakt_id: u64) -> Result<ShowPeople> {
        send_json(self.authed(Method::GET, &format!("/shows/{show_trakt_id}/people"))?).await
    }

    pub async fn trending_shows(&self, limit: u32) -> Result<Vec<TrendingShow>> {
        let path = format!("/shows/trending?limit={limit}&extended=full");
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn recommended_movies(&self, limit: u32) -> Result<Vec<Movie>> {
        let path = format!(
            "/recommendations/movies?limit={limit}&ignore_collected=true&ignore_watchlisted=false&extended=full"
        );
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn related_movies(&self, movie_trakt_id: u64, limit: u32) -> Result<Vec<Movie>> {
        let path = format!("/movies/{movie_trakt_id}/related?limit={limit}&extended=full");
        send_json(self.authed(Method::GET, &path)?).await
    }

    pub async fn trending_movies(&self, limit: u32) -> Result<Vec<TrendingMovie>> {
        let path = format!("/movies/trending?limit={limit}&extended=full");
        send_json(self.authed(Method::GET, &path)?).await
    }

    // ---- search -----------------------------------------------------------

    pub async fn search_shows(&self, query: &str) -> Result<Vec<SearchResult>> {
        let req = self.authed(Method::GET, "/search/show")?.query(&[
            ("query", query),
            ("extended", "full"),
            ("limit", "20"),
        ]);
        send_json(req).await
    }

    // ---- plumbing ---------------------------------------------------------

    async fn post(&self, path: &str, body: serde_json::Value) -> Result<()> {
        self.write(Method::POST, path, Some(&body)).await.map(drop)
    }

    /// Writes are limited to one per second; on a 429 wait as told and retry
    /// once.
    async fn write(
        &self,
        method: Method,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<reqwest::Response> {
        let request = || -> Result<RequestBuilder> {
            let req = self.authed(method.clone(), path)?;
            Ok(match body {
                Some(body) => req.json(body),
                None => req,
            })
        };
        let res = request()?.send().await?;
        if res.status() != StatusCode::TOO_MANY_REQUESTS {
            return check(res).await;
        }
        let wait = res
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1)
            .min(10);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        check(request()?.send().await?).await
    }

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

fn movies_body(movie_trakt_id: u64) -> serde_json::Value {
    json!({ "movies": [{ "ids": { "trakt": movie_trakt_id } }] })
}

fn shows_body(show_trakt_id: u64) -> serde_json::Value {
    json!({ "shows": [{ "ids": { "trakt": show_trakt_id } }] })
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
