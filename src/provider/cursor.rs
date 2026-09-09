use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::model::{Account, Provider, Window};
use crate::oauth::decode_jwt_claims;
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const USAGE_URL: &str = "https://cursor.com/api/usage-summary";
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"));
const SUBJECT: &str = "subject";
const DATABASE_ENV: &str = "AIUSG_CURSOR_DB";

const ACCESS_TOKEN_KEY: &str = "cursorAuth/accessToken";
const REFRESH_TOKEN_KEY: &str = "cursorAuth/refreshToken";
const EMAIL_KEY: &str = "cursorAuth/cachedEmail";
const MEMBERSHIP_KEY: &str = "cursorAuth/stripeMembershipType";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSummary {
    #[serde(default)]
    membership_type: Option<String>,
    #[serde(default)]
    is_unlimited: bool,
    #[serde(default)]
    billing_cycle_end: Option<DateTime<Utc>>,
    #[serde(default)]
    individual_usage: Option<Usage>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Usage {
    #[serde(default)]
    plan: Option<Bucket>,
    #[serde(default)]
    on_demand: Option<Bucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    used: Option<f64>,
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    total_percent_used: Option<f64>,
}

impl Bucket {
    fn to_window(&self, name: &str) -> Option<Window> {
        if !self.enabled {
            return None;
        }
        let percent = self.total_percent_used.or_else(|| {
            let limit = self.limit.filter(|limit| *limit > 0.0)?;
            Some((self.used? / limit) * 100.0)
        })?;

        let mut window = match (self.used, self.limit) {
            (Some(used), Some(limit)) if limit > 0.0 => {
                Window::from_count(name, dollars(used), dollars(limit))
            }
            _ => Window::from_percent(name, percent),
        };
        window.used_percent = Some(percent);
        Some(window)
    }
}

fn dollars(cents: f64) -> u64 {
    (cents.max(0.0) / 100.0).round() as u64
}

fn windows(summary: &UsageSummary) -> Vec<Window> {
    if summary.is_unlimited {
        return Vec::new();
    }
    let Some(usage) = &summary.individual_usage else {
        return Vec::new();
    };

    [
        usage.plan.as_ref().map(|bucket| (bucket, "Included usage")),
        usage.on_demand.as_ref().map(|bucket| (bucket, "On demand")),
    ]
    .into_iter()
    .flatten()
    .filter_map(|(bucket, name)| {
        bucket
            .to_window(name)
            .map(|window| window.resetting_at(summary.billing_cycle_end))
    })
    .collect()
}

fn cookie(credential: &Credential) -> Result<String> {
    let subject = credential
        .get(SUBJECT)
        .map(str::to_owned)
        .or_else(|| subject_of(&credential.access_token))
        .context("could not determine the Cursor user from the stored token")?;

    let encoded: String = url::form_urlencoded::byte_serialize(subject.as_bytes()).collect();
    Ok(format!(
        "WorkosCursorSessionToken={encoded}%3A%3A{}",
        credential.access_token
    ))
}

fn subject_of(token: &str) -> Option<String> {
    decode_jwt_claims(token)
        .ok()?
        .get("sub")?
        .as_str()
        .map(str::to_owned)
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let response = http
        .get(USAGE_URL)
        .header("Cookie", cookie(credential)?)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .context("requesting Cursor usage")?;

    let summary: UsageSummary = crate::provider::read_json(response, "Cursor usage").await?;
    Ok(Fetched {
        plan: summary.membership_type.clone(),
        windows: windows(&summary),
    })
}

pub async fn login(_http: &reqwest::Client) -> Result<Discovered> {
    discover()?
        .into_iter()
        .next()
        .context("sign in to the Cursor app first, then run `aiusg login cursor` again")
}

pub async fn refresh(
    _http: &reqwest::Client,
    _credential: &Credential,
) -> Result<Option<Credential>> {
    match discover()?.into_iter().next() {
        Some(found) => Ok(Some(found.credential)),
        None => bail!("sign in to the Cursor app again to renew the session"),
    }
}

fn database() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(DATABASE_ENV) {
        return Some(PathBuf::from(path));
    }
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()?
            .join("Library")
            .join("Application Support")
    } else {
        dirs::config_dir()?
    };
    Some(
        base.join("Cursor")
            .join("User")
            .join("globalStorage")
            .join("state.vscdb"),
    )
}

fn decode(bytes: &[u8]) -> String {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !pairs.is_empty() && rest.is_empty() && bytes[1] == 0 {
        let units: Vec<u16> = pairs.iter().copied().map(u16::from_le_bytes).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

pub fn discover() -> Result<Vec<Discovered>> {
    let Some(path) = database().filter(|path| path.exists()) else {
        return Ok(Vec::new());
    };

    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening {}", path.display()))?;

    let read = |key: &str| -> Option<String> {
        connection
            .query_row("select value from ItemTable where key = ?1", [key], |row| {
                row.get::<_, String>(0)
                    .or_else(|_| row.get::<_, Vec<u8>>(0).map(|bytes| decode(&bytes)))
            })
            .ok()
            .filter(|value| !value.is_empty())
    };

    let Some(access_token) = read(ACCESS_TOKEN_KEY) else {
        return Ok(Vec::new());
    };

    let subject = subject_of(&access_token);
    let label = read(EMAIL_KEY)
        .or_else(|| subject.clone())
        .unwrap_or_else(|| "cursor".to_owned());

    let expires_at = decode_jwt_claims(&access_token)
        .ok()
        .and_then(|claims| claims.get("exp")?.as_i64())
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single());

    let mut credential = Credential {
        access_token,
        refresh_token: read(REFRESH_TOKEN_KEY),
        expires_at,
        extra: Default::default(),
    };
    if let Some(subject) = subject {
        credential = credential.with(SUBJECT, subject);
    }

    Ok(vec![Discovered {
        account: Account::new(Provider::Cursor, label, read(MEMBERSHIP_KEY)),
        credential,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"billingCycleStart":"2026-08-17T03:54:43.000Z","billingCycleEnd":"2026-09-17T03:54:43.000Z","membershipType":"ultra","limitType":"user","isUnlimited":false,"autoModelSelectedDisplayMessage":"You've used 100% of your included total usage","individualUsage":{"plan":{"enabled":true,"used":40000,"limit":40000,"remaining":0,"breakdown":{"included":40000,"bonus":310150,"total":350150},"autoPercentUsed":100,"apiPercentUsed":100,"totalPercentUsed":100},"onDemand":{"enabled":false,"used":0,"limit":null,"remaining":null}},"teamUsage":{}}"#;

    #[test]
    fn maps_the_live_summary() {
        let summary: UsageSummary = serde_json::from_str(LIVE).expect("payload should parse");
        assert_eq!(summary.membership_type.as_deref(), Some("ultra"));

        let windows = windows(&summary);
        assert_eq!(
            windows.len(),
            1,
            "a disabled on-demand bucket must not render"
        );
        assert_eq!(windows[0].name, "Included usage");
        assert_eq!(windows[0].used_percent, Some(100.0));
        assert!(
            windows[0].resets_at.is_some(),
            "reset comes from billingCycleEnd"
        );
    }

    #[test]
    fn amounts_are_cents_and_render_as_dollars() {
        let summary: UsageSummary = serde_json::from_str(LIVE).unwrap();
        let windows = windows(&summary);
        assert_eq!(windows[0].used, Some(400), "40000 cents is $400");
        assert_eq!(windows[0].limit, Some(400));
    }

    #[test]
    fn the_reported_percentage_wins_over_the_ratio() {
        let summary: UsageSummary = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"enabled":true,"used":5000,"limit":40000,"totalPercentUsed":97}}}"#,
        )
        .unwrap();
        let windows = windows(&summary);
        assert_eq!(
            windows[0].used_percent,
            Some(97.0),
            "totalPercentUsed accounts for spend the included bucket does not show"
        );
        assert_eq!(
            windows[0].used,
            Some(50),
            "counts come from the cents fields"
        );
    }

    #[test]
    fn utf16_blob_values_decode() {
        let utf16: Vec<u8> = "ultra".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode(&utf16), "ultra");
        assert_eq!(decode(b"ultra"), "ultra", "plain utf-8 text still decodes");
    }

    #[test]
    fn an_unlimited_plan_reports_no_windows() {
        let summary: UsageSummary =
            serde_json::from_str(r#"{"isUnlimited":true,"membershipType":"enterprise"}"#).unwrap();
        assert!(windows(&summary).is_empty());
    }

    #[test]
    fn an_enabled_on_demand_bucket_renders() {
        let summary: UsageSummary = serde_json::from_str(
            r#"{"individualUsage":{"plan":{"enabled":true,"totalPercentUsed":50},"onDemand":{"enabled":true,"used":25,"limit":100}}}"#,
        )
        .unwrap();
        let windows = windows(&summary);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[1].name, "On demand");
        assert_eq!(
            windows[1].used_percent,
            Some(25.0),
            "derived from used/limit"
        );
    }

    #[test]
    fn the_session_cookie_encodes_the_subject() {
        let credential = Credential::bearer("token-value").with(SUBJECT, "google-oauth2|user_ABC");
        let cookie = cookie(&credential).expect("cookie should build");
        assert_eq!(
            cookie, "WorkosCursorSessionToken=google-oauth2%7Cuser_ABC%3A%3Atoken-value",
            "the pipe must be percent-encoded and :: sent as %3A%3A"
        );
    }

    #[test]
    fn a_missing_subject_is_an_error_not_a_bad_cookie() {
        let credential = Credential::bearer("not-a-jwt");
        assert!(cookie(&credential).is_err());
    }
}
