use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{TimeZone, Utc};
use serde::Deserialize;

use crate::model::{Account, Provider, Window};
use crate::oauth::{Loopback, Pkce, decode_jwt_claims, prompt_open, random_token};
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const SCOPES: &str =
    "openid profile email offline_access api.connectors.read api.connectors.invoke";
const CALLBACK_PORTS: [u16; 2] = [1455, 1457];
const ORIGINATOR: &str = "codex_cli_rs";
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"), " (codex_cli_rs)");
const ACCOUNT_ID: &str = "account_id";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    additional_rate_limits: Vec<AdditionalLimit>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    #[serde(default)]
    primary_window: Option<WindowSnapshot>,
    #[serde(default)]
    secondary_window: Option<WindowSnapshot>,
}

#[derive(Debug, Deserialize)]
struct WindowSnapshot {
    #[serde(default)]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct AdditionalLimit {
    #[serde(default)]
    limit_name: Option<String>,
    #[serde(default)]
    metered_feature: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
}

impl WindowSnapshot {
    fn into_window(self, fallback: &str) -> Option<Window> {
        let used_percent = self.used_percent?;
        let name = self
            .limit_window_seconds
            .filter(|seconds| *seconds > 0)
            .map(describe_window)
            .unwrap_or_else(|| fallback.to_owned());

        let resets_at = self
            .reset_at
            .filter(|value| *value > 0)
            .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
            .or_else(|| {
                self.reset_after_seconds
                    .filter(|value| *value > 0)
                    .map(|seconds| Utc::now() + chrono::Duration::seconds(seconds))
            });

        Some(Window::from_percent(name, used_percent).resetting_at(resets_at))
    }
}

fn describe_window(seconds: i64) -> String {
    let minutes = (seconds + 59) / 60;
    match minutes {
        0..=90 => format!("{minutes}m"),
        91..=1439 => format!("{}h", (minutes as f64 / 60.0).round() as i64),
        _ => format!("{}d", (minutes as f64 / 1440.0).round() as i64),
    }
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let mut request = http
        .get(USAGE_URL)
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("User-Agent", USER_AGENT)
        .header("originator", ORIGINATOR)
        .header("Accept", "application/json");

    if let Some(account) = credential.get(ACCOUNT_ID) {
        request = request.header("ChatGPT-Account-ID", account);
    }

    let response = request.send().await.context("requesting Codex usage")?;
    let usage: UsageResponse = crate::provider::read_json(response, "Codex usage").await?;

    Ok(Fetched {
        plan: usage.plan_type.clone(),
        windows: windows(usage),
    })
}

fn windows(usage: UsageResponse) -> Vec<Window> {
    let mut windows = Vec::new();

    if let Some(limit) = usage.rate_limit {
        windows.extend(collect(limit, "primary", "secondary"));
    }

    for additional in usage.additional_rate_limits {
        let label = additional
            .limit_name
            .or(additional.metered_feature)
            .unwrap_or_else(|| "extra".to_owned());
        if let Some(limit) = additional.rate_limit {
            for mut window in collect(limit, &label, &label) {
                window.name = format!("{label} {}", window.name);
                windows.push(window);
            }
        }
    }
    windows
}

fn collect(limit: RateLimit, primary: &str, secondary: &str) -> Vec<Window> {
    [
        limit
            .primary_window
            .and_then(|window| window.into_window(primary)),
        limit
            .secondary_window
            .and_then(|window| window.into_window(secondary)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let pkce = Pkce::generate();
    let state = random_token(32);
    let loopback = Loopback::bind(&CALLBACK_PORTS).await?;
    let redirect = format!("http://localhost:{}/auth/callback", loopback.port());

    let authorize = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
            ("originator", ORIGINATOR),
        ],
    )?;
    prompt_open(authorize.as_str());

    let params = loopback.wait().await?;
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        bail!("the authorization state did not match; aborting");
    }
    let code = params
        .get("code")
        .context("no authorization code returned")?;

    let token: TokenResponse = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("client_id", CLIENT_ID),
            ("code_verifier", pkce.verifier.as_str()),
        ])
        .send()
        .await
        .context("exchanging the Codex authorization code")?
        .json()
        .await
        .context("parsing the Codex token response")?;

    Ok(token.into_discovered())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

struct Identity {
    label: String,
    plan: Option<String>,
    account_id: Option<String>,
}

fn identify(id_token: Option<&str>) -> Identity {
    let claims = id_token.and_then(|token| decode_jwt_claims(token).ok());
    let Some(claims) = claims else {
        return Identity {
            label: "codex".to_owned(),
            plan: None,
            account_id: None,
        };
    };

    let auth = claims.get(AUTH_CLAIM);
    let email = claims
        .get("email")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            claims
                .get("https://api.openai.com/profile")?
                .get("email")?
                .as_str()
        });

    Identity {
        label: email.unwrap_or("codex").to_owned(),
        plan: auth
            .and_then(|auth| auth.get("chatgpt_plan_type"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        account_id: auth
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}

impl TokenResponse {
    fn into_discovered(self) -> Discovered {
        let identity = identify(self.id_token.as_deref());
        let mut credential = Credential {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at: self
                .expires_in
                .map(|seconds| Utc::now() + chrono::Duration::seconds(seconds)),
            extra: Default::default(),
        };
        if let Some(account_id) = identity.account_id {
            credential = credential.with(ACCOUNT_ID, account_id);
        }
        Discovered {
            account: Account::new(Provider::Codex, identity.label, identity.plan),
            credential,
        }
    }
}

pub async fn refresh(
    http: &reqwest::Client,
    credential: &Credential,
) -> Result<Option<Credential>> {
    let Some(refresh_token) = credential.refresh_token.as_deref() else {
        return Ok(None);
    };

    let token: TokenResponse = http
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }))
        .send()
        .await
        .context("refreshing the Codex token")?
        .error_for_status()
        .context("OpenAI rejected the refresh token")?
        .json()
        .await
        .context("parsing the refreshed Codex token")?;

    let mut refreshed = token.into_discovered().credential;
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = credential.refresh_token.clone();
    }
    if let Some(account) = credential.get(ACCOUNT_ID) {
        refreshed = refreshed.with(ACCOUNT_ID, account);
    }
    Ok(Some(refreshed))
}

#[derive(Debug, Deserialize)]
struct StoredAuth {
    #[serde(default)]
    tokens: Option<StoredTokens>,
}

#[derive(Debug, Deserialize)]
struct StoredTokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

fn auth_path() -> Option<PathBuf> {
    let base = match std::env::var_os("CODEX_HOME") {
        Some(directory) => PathBuf::from(directory),
        None => dirs::home_dir()?.join(".codex"),
    };
    Some(base.join("auth.json"))
}

pub fn discover() -> Result<Vec<Discovered>> {
    let Some(path) = auth_path().filter(|path| path.exists()) else {
        return Ok(Vec::new());
    };
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let stored: StoredAuth =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

    let Some(tokens) = stored
        .tokens
        .filter(|tokens| !tokens.access_token.is_empty())
    else {
        return Ok(Vec::new());
    };

    let identity = identify(tokens.id_token.as_deref());
    let expires_at = tokens
        .id_token
        .as_deref()
        .and_then(|token| decode_jwt_claims(token).ok())
        .and_then(|claims| claims.get("exp")?.as_i64())
        .and_then(|seconds| Utc.timestamp_opt(seconds, 0).single());

    let mut credential = Credential {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at,
        extra: Default::default(),
    };
    if let Some(account_id) = tokens.account_id.or(identity.account_id) {
        credential = credential.with(ACCOUNT_ID, account_id);
    }

    Ok(vec![Discovered {
        account: Account::new(Provider::Codex, identity.label, identity.plan),
        credential,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"plan_type":"pro","rate_limit":{"allowed":false,"limit_reached":true,"primary_window":{"used_percent":100,"limit_window_seconds":604800,"reset_after_seconds":508532,"reset_at":1789435639},"secondary_window":null},"additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","metered_feature":"codex_bengalfox","rate_limit":{"primary_window":{"used_percent":0,"limit_window_seconds":17940,"reset_at":1788945000},"secondary_window":{"used_percent":0,"limit_window_seconds":604800,"reset_at":1789435639}}}]}"#;

    #[test]
    fn maps_live_payload() {
        let usage: UsageResponse = serde_json::from_str(LIVE).expect("payload should parse");
        assert_eq!(usage.plan_type.as_deref(), Some("pro"));

        let windows = windows(usage);
        assert_eq!(
            windows.len(),
            3,
            "one main window plus two per-model windows"
        );
        assert_eq!(windows[0].name, "7d", "604800s should render as 7d");
        assert_eq!(windows[0].used_percent, Some(100.0));
        assert!(windows[0].resets_at.is_some());
    }

    #[test]
    fn null_secondary_window_is_skipped() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);
        assert!(
            windows.iter().filter(|window| window.name == "7d").count() == 1,
            "a null secondary_window must not produce a window"
        );
    }

    #[test]
    fn additional_limits_are_prefixed_by_model() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);
        assert_eq!(windows[1].name, "GPT-5.3-Codex-Spark 5h");
        assert_eq!(windows[2].name, "GPT-5.3-Codex-Spark 7d");
    }

    #[test]
    fn describes_windows_from_seconds() {
        assert_eq!(describe_window(17940), "5h", "299 minutes rounds to 5h");
        assert_eq!(describe_window(604800), "7d");
        assert_eq!(describe_window(3600), "60m");
    }

    #[test]
    fn falls_back_to_relative_reset_when_absolute_missing() {
        let usage: UsageResponse = serde_json::from_str(
            r#"{"rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":17940,"reset_after_seconds":600}}}"#,
        )
        .unwrap();
        let windows = windows(usage);
        assert!(
            windows[0].resets_at.is_some(),
            "reset_after_seconds should be used when reset_at is absent"
        );
    }
}
