//! Posters and backdrops from TMDB, cached so each show is looked up once a
//! month at most, and where a show can be streamed (data from JustWatch).
//!
//! This product uses the TMDB API but is not endorsed or certified by TMDB.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use futures::{StreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const API_URL: &str = "https://api.themoviedb.org/3";
const IMAGE_URL: &str = "https://image.tmdb.org/t/p";
const CONCURRENCY: usize = 8;
const MAX_AGE_DAYS: i64 = 30;

#[derive(Debug, Clone)]
pub struct TmdbClient {
    http: reqwest::Client,
    api_key: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Artwork {
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
}

/// A streaming service a show is on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    pub logo: Option<String>,
}

#[derive(Deserialize)]
struct ProvidersResponse {
    #[serde(default)]
    results: HashMap<String, RegionProviders>,
}

#[derive(Deserialize)]
struct RegionProviders {
    #[serde(default)]
    flatrate: Vec<RawProvider>,
    #[serde(default)]
    free: Vec<RawProvider>,
    #[serde(default)]
    ads: Vec<RawProvider>,
}

#[derive(Deserialize)]
struct RawProvider {
    provider_name: String,
    logo_path: Option<String>,
    #[serde(default)]
    display_priority: i64,
}

impl TmdbClient {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            api_key: api_key.into(),
        }
    }

    pub async fn tv_artwork(&self, tmdb_id: u64) -> Result<Artwork> {
        self.get(&format!("/tv/{tmdb_id}")).await
    }

    /// Services that include the show in a subscription or for free in
    /// `region` (ISO 3166-1, e.g. "AU"). Buy and rent offers are left out.
    pub async fn tv_streaming(&self, tmdb_id: u64, region: &str) -> Result<Vec<Provider>> {
        let res: ProvidersResponse = self.get(&format!("/tv/{tmdb_id}/watch/providers")).await?;
        let Some(region) = res.results.get(&region.to_uppercase()) else {
            return Ok(Vec::new());
        };
        let mut all: Vec<&RawProvider> = region
            .flatrate
            .iter()
            .chain(&region.free)
            .chain(&region.ads)
            .collect();
        all.sort_by_key(|p| p.display_priority);
        let mut seen = HashSet::new();
        Ok(all
            .into_iter()
            .filter(|p| seen.insert(p.provider_name.clone()))
            .map(|p| Provider {
                name: p.provider_name.clone(),
                logo: p
                    .logo_path
                    .as_ref()
                    .map(|path| format!("{IMAGE_URL}/w92{path}")),
            })
            .collect())
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T> {
        let res = self
            .http
            .get(format!("{API_URL}{path}"))
            .query(&[("api_key", self.api_key.as_str())])
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(Error::Api {
                status: res.status().as_u16(),
                body: res.text().await.unwrap_or_default(),
            });
        }
        Ok(res.json().await?)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtworkCache {
    tv: HashMap<u64, CachedArtwork>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedArtwork {
    artwork: Artwork,
    fetched_at: DateTime<Utc>,
}

/// Image width for [`ArtworkCache::poster_url`] and friends.
#[derive(Debug, Clone, Copy)]
pub enum Size {
    Thumb,
    Poster,
    Backdrop,
}

impl Size {
    fn path(self) -> &'static str {
        match self {
            Size::Thumb => "w154",
            Size::Poster => "w342",
            Size::Backdrop => "w780",
        }
    }
}

impl ArtworkCache {
    /// Looks up shows that are missing or stale. Failures are skipped; the
    /// show simply has no poster until the next try. Returns whether anything
    /// was fetched, so the caller knows to save.
    pub async fn fill(
        &mut self,
        client: &TmdbClient,
        tmdb_ids: impl IntoIterator<Item = u64>,
        now: DateTime<Utc>,
    ) -> bool {
        let wanted: HashSet<u64> = tmdb_ids
            .into_iter()
            .filter(|id| {
                self.tv
                    .get(id)
                    .is_none_or(|c| now - c.fetched_at > Duration::days(MAX_AGE_DAYS))
            })
            .collect();
        let mut fetched = stream::iter(wanted)
            .map(|id| async move { (id, client.tv_artwork(id).await) })
            .buffer_unordered(CONCURRENCY);
        let mut changed = false;
        while let Some((id, result)) = fetched.next().await {
            if let Ok(artwork) = result {
                self.tv.insert(
                    id,
                    CachedArtwork {
                        artwork,
                        fetched_at: now,
                    },
                );
                changed = true;
            }
        }
        changed
    }

    pub fn poster_url(&self, tmdb_id: Option<u64>, size: Size) -> Option<String> {
        let path = self.tv.get(&tmdb_id?)?.artwork.poster_path.as_deref()?;
        Some(format!("{IMAGE_URL}/{}{path}", size.path()))
    }

    pub fn backdrop_url(&self, tmdb_id: Option<u64>) -> Option<String> {
        let path = self.tv.get(&tmdb_id?)?.artwork.backdrop_path.as_deref()?;
        Some(format!("{IMAGE_URL}/{}{path}", Size::Backdrop.path()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_image_urls_from_cached_paths() {
        let mut cache = ArtworkCache::default();
        cache.tv.insert(
            7,
            CachedArtwork {
                artwork: Artwork {
                    poster_path: Some("/p.jpg".into()),
                    backdrop_path: None,
                },
                fetched_at: Utc::now(),
            },
        );
        assert_eq!(
            cache.poster_url(Some(7), Size::Thumb).as_deref(),
            Some("https://image.tmdb.org/t/p/w154/p.jpg")
        );
        assert_eq!(cache.backdrop_url(Some(7)), None);
        assert_eq!(cache.poster_url(Some(8), Size::Thumb), None);
        assert_eq!(cache.poster_url(None, Size::Thumb), None);
    }
}
