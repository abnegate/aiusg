use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use crate::model::{Account, Provider, Window};
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const USAGE_URL: &str = "https://api.github.com/copilot_internal/user";
const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const CLIENT_ID: &str = "Ov23ctr1Udn5GokVCVJf";
const KEYCHAIN_SERVICE: &str = "github-copilot-app";
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Deserialize)]
struct UsageResponse {
    login: Option<String>,
    copilot_plan: Option<String>,
    quota_reset_date: Option<String>,
    quota_reset_date_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    quota_snapshots: BTreeMap<String, Snapshot>,
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    #[serde(default)]
    entitlement: i64,
    #[serde(default)]
    remaining: i64,
    #[serde(default)]
    percent_remaining: f64,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    overage_permitted: bool,
    #[serde(default)]
    credits_used: Option<f64>,
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let response = http
        .get(USAGE_URL)
        .header(
            "Authorization",
            format!("token {}", credential.access_token),
        )
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .context("requesting Copilot usage")?;

    let usage: UsageResponse = crate::provider::read_json(response, "Copilot usage").await?;
    Ok(Fetched {
        plan: usage.copilot_plan.clone().or_else(|| usage.login.clone()),
        windows: windows(usage),
    })
}

fn windows(usage: UsageResponse) -> Vec<Window> {
    let resets_at = usage
        .quota_reset_date_utc
        .or_else(|| parse_reset_date(usage.quota_reset_date.as_deref()));

    usage
        .quota_snapshots
        .into_iter()
        .filter(|(_, snapshot)| !snapshot.unlimited)
        .map(|(name, snapshot)| snapshot_to_window(&name, &snapshot).resetting_at(resets_at))
        .collect()
}

fn snapshot_to_window(name: &str, snapshot: &Snapshot) -> Window {
    let label = humanise(name);
    let used = snapshot
        .credits_used
        .unwrap_or_else(|| (snapshot.entitlement - snapshot.remaining) as f64);

    if snapshot.entitlement > 0 {
        let mut window =
            Window::from_count(label, used.max(0.0) as u64, snapshot.entitlement as u64);
        if snapshot.overage_permitted {
            window.name = format!("{} (overage on)", window.name);
        }
        window
    } else {
        Window::from_percent(
            label,
            (100.0 - snapshot.percent_remaining).clamp(0.0, 100.0),
        )
    }
}

fn humanise(name: &str) -> String {
    match name {
        "premium_interactions" => "Premium requests".to_owned(),
        "chat" => "Chat".to_owned(),
        "completions" => "Completions".to_owned(),
        other => other.replace('_', " "),
    }
}

fn parse_reset_date(value: Option<&str>) -> Option<DateTime<Utc>> {
    let date = NaiveDate::parse_from_str(value?, "%Y-%m-%d").ok()?;
    Utc.from_local_datetime(&date.and_hms_opt(0, 0, 0)?)
        .single()
}

#[derive(Debug, Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let device: DeviceCode = http
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({ "client_id": CLIENT_ID, "scope": "read:user" }))
        .send()
        .await
        .context("requesting GitHub device code")?
        .json()
        .await
        .context("parsing GitHub device code")?;

    println!(
        "  Open {} and enter code: {}",
        device.verification_uri, device.user_code
    );
    let _ = webbrowser::open(&device.verification_uri);

    let token = poll_for_token(http, &device).await?;
    let login = current_login(http, &token).await?;

    Ok(Discovered {
        account: Account::new(Provider::Copilot, login, None),
        credential: Credential::bearer(token),
    })
}

async fn poll_for_token(http: &reqwest::Client, device: &DeviceCode) -> Result<String> {
    let mut interval = Duration::from_secs(device.interval);
    loop {
        tokio::time::sleep(interval).await;
        let response: TokenResponse = http
            .post(ACCESS_TOKEN_URL)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": device.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .context("polling GitHub for authorization")?
            .json()
            .await
            .context("parsing GitHub token response")?;

        if let Some(token) = response.access_token {
            return Ok(token);
        }
        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => bail!("the device code expired before you authorized it"),
            Some("access_denied") => bail!("authorization was denied"),
            Some(other) => bail!("GitHub returned '{other}'"),
            None => bail!("GitHub returned no token and no error"),
        }
    }
}

async fn current_login(http: &reqwest::Client, token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }

    let user: User = http
        .get("https://api.github.com/user")
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .context("identifying the authorized GitHub user")?
        .json()
        .await
        .context("parsing the GitHub user response")?;
    Ok(user.login)
}

pub fn discover() -> Result<Vec<Discovered>> {
    let mut found = from_apps_file().unwrap_or_default();
    if found.is_empty()
        && let Ok(mut stored) = from_database()
    {
        found.append(&mut stored);
    }
    found.sort_by(|left, right| left.account.label.cmp(&right.account.label));
    found.dedup_by(|left, right| left.account.label == right.account.label);
    Ok(found)
}

fn apps_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("github-copilot").join("apps.json"))
}

fn from_apps_file() -> Result<Vec<Discovered>> {
    #[derive(Deserialize)]
    struct App {
        user: String,
        oauth_token: String,
    }

    let path = apps_path().context("no config directory")?;
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let apps: BTreeMap<String, App> = serde_json::from_str(&raw)?;

    Ok(apps
        .into_values()
        .map(|app| Discovered {
            account: Account::new(Provider::Copilot, app.user, None),
            credential: Credential::bearer(app.oauth_token),
        })
        .collect())
}

fn from_database() -> Result<Vec<Discovered>> {
    let path = dirs::home_dir()
        .context("no home directory")?
        .join(".copilot")
        .join("data.db");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .with_context(|| format!("opening {}", path.display()))?;

    let mut statement = connection.prepare("select id, login, access_token from accounts")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut found = Vec::new();
    for row in rows {
        let (id, login, stored) = row?;
        let token = stored
            .filter(|value| !value.is_empty())
            .or_else(|| keychain_token(&id));
        if let Some(token) = token {
            found.push(Discovered {
                account: Account::new(Provider::Copilot, login, None),
                credential: Credential::bearer(token),
            });
        }
    }
    Ok(found)
}

fn keychain_token(account: &str) -> Option<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account)
        .ok()?
        .get_password()
        .ok()
        .filter(|value| !value.is_empty())
}

pub async fn refresh(
    _http: &reqwest::Client,
    _credential: &Credential,
) -> Result<Option<Credential>> {
    Err(anyhow!(
        "GitHub OAuth tokens do not expire and cannot be refreshed"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"login":"abnegate","copilot_plan":"individual","quota_reset_date":"2026-10-01","quota_reset_date_utc":"2026-10-01T00:00:00.000Z","quota_snapshots":{"premium_interactions":{"quota_id":"premium_interactions","entitlement":1500,"remaining":-7,"quota_remaining":-6.9,"percent_remaining":0.0,"unlimited":false,"has_quota":false,"overage_count":0,"overage_permitted":false,"overage_entitlement":0,"credits_used":1506,"token_based_billing":true,"quota_reset_at":0},"chat":{"entitlement":0,"remaining":0,"percent_remaining":0.0,"unlimited":true},"completions":{"entitlement":0,"remaining":0,"percent_remaining":0.0,"unlimited":true}}}"#;

    #[test]
    fn maps_live_payload_and_skips_unlimited_buckets() {
        let usage: UsageResponse = serde_json::from_str(LIVE).expect("payload should parse");
        let windows = windows(usage);

        assert_eq!(
            windows.len(),
            1,
            "unlimited chat/completions must not render"
        );
        assert_eq!(windows[0].name, "Premium requests");
        assert_eq!(windows[0].used, Some(1506));
        assert_eq!(windows[0].limit, Some(1500));
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn negative_remaining_still_reports_over_quota() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);
        let percent = windows[0].used_percent.expect("percent should be computed");
        assert!(
            percent > 100.0,
            "1506 used of 1500 must exceed 100%, got {percent}"
        );
        assert!(windows[0].is_exhausted());
    }

    #[test]
    fn falls_back_to_entitlement_minus_remaining_without_credits_used() {
        let usage: UsageResponse = serde_json::from_str(
            r#"{"quota_snapshots":{"premium_interactions":{"entitlement":300,"remaining":75,"percent_remaining":25.0,"unlimited":false}}}"#,
        )
        .unwrap();
        let windows = windows(usage);
        assert_eq!(
            windows[0].used,
            Some(225),
            "300 entitlement minus 75 remaining"
        );
        assert_eq!(windows[0].limit, Some(300));
    }

    #[test]
    fn parses_date_only_reset() {
        let at = parse_reset_date(Some("2026-10-01")).expect("date should parse");
        assert_eq!(at.to_rfc3339(), "2026-10-01T00:00:00+00:00");
    }
}
