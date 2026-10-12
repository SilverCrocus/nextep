//! Trakt OAuth device flow: the app shows a short code, the user enters it at
//! trakt.tv/activate, and the app polls until the token is issued. This needs
//! no redirect URI or deep-link handling on the phone.

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    /// Seconds until `device_code` stops working.
    pub expires_in: u64,
    /// Seconds to wait between polls.
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// Lifetime in seconds, counted from `created_at`.
    pub expires_in: i64,
    /// Unix timestamp (seconds).
    pub created_at: i64,
    #[serde(default)]
    pub scope: Option<String>,
}

impl Token {
    pub fn expires_at(&self) -> DateTime<Utc> {
        Utc.timestamp_opt(self.created_at + self.expires_in, 0)
            .single()
            .unwrap_or_else(Utc::now)
    }

    /// True when the token expires within `margin`, so it should be refreshed
    /// before the next request.
    pub fn needs_refresh(&self, now: DateTime<Utc>, margin: Duration) -> bool {
        self.expires_at() - margin <= now
    }
}

/// Outcome of one poll of `POST /oauth/device/token`.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePoll {
    Authorized(Token),
    /// The user has not entered the code yet; poll again after `interval`.
    Pending,
    /// Polling too fast; add a few seconds to the interval.
    SlowDown,
    Denied,
    Expired,
    AlreadyUsed,
    InvalidCode,
}

impl DevicePoll {
    /// Maps Trakt's documented non-200 status codes.
    pub(crate) fn from_status(status: u16) -> Option<Self> {
        Some(match status {
            400 => Self::Pending,
            404 => Self::InvalidCode,
            409 => Self::AlreadyUsed,
            410 => Self::Expired,
            418 => Self::Denied,
            429 => Self::SlowDown,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_margin() {
        let token = Token {
            access_token: "a".into(),
            refresh_token: "r".into(),
            token_type: "bearer".into(),
            expires_in: 86_400,
            created_at: 1_760_000_000,
            scope: None,
        };
        let issued = Utc.timestamp_opt(token.created_at, 0).unwrap();
        assert!(!token.needs_refresh(issued, Duration::hours(1)));
        assert!(token.needs_refresh(issued + Duration::hours(23), Duration::hours(1)));
    }
}
