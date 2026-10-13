//! Checks GitHub Releases for a newer version of Nextep.
//!
//! Releases are built and signed by `.github/workflows/release.yml` and
//! named `nextep-<version>.apk`. The check reads where GitHub's "latest
//! release" page redirects to, which isn't rate-limited like the API (60
//! calls an hour per IP address, shared by everyone behind a mobile
//! carrier's NAT); the API is the fallback.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

const REPO_URL: &str = "https://github.com/SilverCrocus/nextep";
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
    let latest = match latest_from_redirect().await {
        Some(release) => release,
        None => match latest_from_api().await? {
            Some(release) => release,
            None => return Ok(None),
        },
    };
    Ok(is_newer(&latest.version, current).then_some(latest))
}

/// `/releases/latest` redirects to `/releases/tag/<tag>`.
async fn latest_from_redirect() -> Option<Release> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .ok()?;
    let res = client
        .head(format!("{REPO_URL}/releases/latest"))
        .header("User-Agent", concat!("Nextep/", env!("CARGO_PKG_VERSION")))
        .send()
        .await
        .ok()?;
    let location = res
        .headers()
        .get(reqwest::header::LOCATION)?
        .to_str()
        .ok()?;
    release_at(location)
}

/// The release a `/releases/tag/<tag>` URL points to, with its APK's
/// download URL (from the naming the release workflow uses).
fn release_at(location: &str) -> Option<Release> {
    let tag = location
        .split("/releases/tag/")
        .nth(1)?
        .trim_end_matches('/');
    let version = tag.strip_prefix('v')?;
    if tag.is_empty() || tag.contains(['/', '?', '#']) {
        return None;
    }
    Some(Release {
        version: version.to_owned(),
        page_url: format!("{REPO_URL}/releases/tag/{tag}"),
        apk_url: Some(format!(
            "{REPO_URL}/releases/download/{tag}/nextep-{version}.apk"
        )),
    })
}

async fn latest_from_api() -> Result<Option<Release>> {
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
    fn reads_the_release_from_the_redirect() {
        let release =
            release_at("https://github.com/SilverCrocus/nextep/releases/tag/v0.9.0").unwrap();
        assert_eq!(release.version, "0.9.0");
        assert_eq!(
            release.apk_url.as_deref(),
            Some(
                "https://github.com/SilverCrocus/nextep/releases/download/v0.9.0/nextep-0.9.0.apk"
            )
        );
        assert!(release_at("https://github.com/SilverCrocus/nextep/releases").is_none());
        assert!(release_at("https://github.com/x/releases/tag/latest").is_none());
    }

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
