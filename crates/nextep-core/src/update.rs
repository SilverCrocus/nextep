//! Checks GitHub Releases for a newer version of Nextep.
//!
//! Releases are built and signed by `.github/workflows/release.yml`. Apps
//! like Obtainium install updates automatically; this check is for everyone
//! else.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const LATEST_URL: &str = "https://api.github.com/repos/SilverCrocus/nextep/releases/latest";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub page_url: String,
    pub apk_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

/// The latest release, if it is newer than `current`.
pub async fn check(current: &str) -> Result<Option<Release>> {
    let res = reqwest::Client::new()
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("Nextep/", env!("CARGO_PKG_VERSION")))
        .send()
        .await?;
    // No release published yet.
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !res.status().is_success() {
        return Err(Error::Api {
            status: res.status().as_u16(),
            body: res.text().await.unwrap_or_default(),
        });
    }
    let latest: GitHubRelease = res.json().await?;
    let version = latest.tag_name.trim_start_matches('v').to_owned();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    Ok(Some(Release {
        apk_url: latest
            .assets
            .into_iter()
            .find(|a| a.name.ends_with(".apk"))
            .map(|a| a.browser_download_url),
        page_url: latest.html_url,
        version,
    }))
}

/// Downloads a release asset (follows GitHub's redirect to its file host).
pub async fn download(url: &str) -> Result<Vec<u8>> {
    let res = reqwest::Client::new()
        .get(url)
        .header("User-Agent", concat!("Nextep/", env!("CARGO_PKG_VERSION")))
        .send()
        .await?;
    if !res.status().is_success() {
        return Err(Error::Api {
            status: res.status().as_u16(),
            body: "Couldn't download the update.".into(),
        });
    }
    Ok(res.bytes().await?.to_vec())
}

/// Compares `major.minor.patch` versions. Anything unparsable is not newer.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> Option<(u64, u64, u64)> {
        let mut parts = v.split('.').map(|p| p.parse::<u64>().ok());
        let version = (parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(version)
    }
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.1.0"));
        assert!(!is_newer("0.2", "0.1.0"));
    }
}
