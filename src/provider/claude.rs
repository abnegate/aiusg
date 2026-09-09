use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::model::{Account, Provider, Window};
use crate::oauth::{Loopback, Pkce, prompt_open, random_token};
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
const CALLBACK_PORTS: [u16; 4] = [54545, 54546, 54547, 0];
const USER_AGENT: &str = "claude-code/2.1.251";

#[derive(Debug, Default, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    five_hour: Option<Limit>,
    #[serde(default)]
    seven_day: Option<Limit>,
    #[serde(default)]
    seven_day_opus: Option<Limit>,
    #[serde(default)]
    seven_day_sonnet: Option<Limit>,
    #[serde(default)]
    seven_day_oauth_apps: Option<Limit>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct Limit {
    #[serde(default)]
    utilization: Option<f64>,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    monthly_limit: Option<i64>,
    #[serde(default)]
    used_credits: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct Profile {
    #[serde(default)]
    subscription_type: Option<String>,
    #[serde(default)]
    rate_limit_tier: Option<String>,
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let response = http
        .get(USAGE_URL)
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("anthropic-beta", OAUTH_BETA)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("requesting Claude usage")?;

    if !response.status().is_success() {
        bail!("Claude usage request failed: HTTP {}", response.status());
    }

    let usage: UsageResponse = response.json().await.context("parsing Claude usage")?;

    let mut windows = Vec::new();
    for (name, limit) in [
        ("5h", &usage.five_hour),
        ("7d", &usage.seven_day),
        ("7d Opus", &usage.seven_day_opus),
        ("7d Sonnet", &usage.seven_day_sonnet),
        ("7d apps", &usage.seven_day_oauth_apps),
    ] {
        if let Some(limit) = limit
            && let Some(utilization) = limit.utilization
        {
            windows.push(Window::from_percent(name, utilization).resetting_at(limit.resets_at));
        }
    }

    if let Some(extra) = usage.extra_usage.filter(|extra| extra.is_enabled)
        && let (Some(limit), Some(used)) = (extra.monthly_limit, extra.used_credits)
        && limit > 0
    {
        windows.push(Window::from_count(
            "Extra usage",
            used.max(0) as u64,
            limit as u64,
        ));
    }

    Ok(Fetched {
        plan: plan(http, credential).await,
        windows,
    })
}

async fn plan(http: &reqwest::Client, credential: &Credential) -> Option<String> {
    let profile: Profile = http
        .get(PROFILE_URL)
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("anthropic-beta", OAUTH_BETA)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    profile.rate_limit_tier.or(profile.subscription_type)
}

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let pkce = Pkce::generate();
    let state = random_token(32);
    let loopback = Loopback::bind(&CALLBACK_PORTS).await?;
    let redirect = format!("http://localhost:{}/callback", loopback.port());

    let authorize = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("code", "true"),
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("code_challenge", &pkce.challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
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
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect,
            "client_id": CLIENT_ID,
            "code_verifier": pkce.verifier,
            "state": state,
        }))
        .send()
        .await
        .context("exchanging the Claude authorization code")?
        .json()
        .await
        .context("parsing the Claude token response")?;

    let credential = token.into_credential();
    let label = identify(http, &credential).await;
    Ok(Discovered {
        account: Account::new(Provider::Claude, label, None),
        credential,
    })
}

async fn identify(http: &reqwest::Client, credential: &Credential) -> String {
    #[derive(Deserialize)]
    struct Named {
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        email: Option<String>,
    }

    let named: Option<Named> = async {
        http.get(PROFILE_URL)
            .header(
                "Authorization",
                format!("Bearer {}", credential.access_token),
            )
            .header("anthropic-beta", OAUTH_BETA)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()
    }
    .await;

    named
        .and_then(|named| named.email.or(named.display_name))
        .unwrap_or_else(|| "claude".to_owned())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

impl TokenResponse {
    fn into_credential(self) -> Credential {
        Credential {
            access_token: self.access_token,
            refresh_token: self.refresh_token,
            expires_at: self
                .expires_in
                .map(|seconds| Utc::now() + chrono::Duration::seconds(seconds)),
            extra: Default::default(),
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
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": CLIENT_ID,
            "scope": SCOPES,
        }))
        .send()
        .await
        .context("refreshing the Claude token")?
        .error_for_status()
        .context("Claude rejected the refresh token")?
        .json()
        .await
        .context("parsing the refreshed Claude token")?;

    let mut refreshed = token.into_credential();
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = credential.refresh_token.clone();
    }
    Ok(Some(refreshed))
}

#[derive(Debug, Deserialize)]
struct StoredCredentials {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<StoredOauth>,
}

#[derive(Debug, Deserialize)]
struct StoredOauth {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken", default)]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt", default)]
    expires_at: Option<i64>,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: Option<String>,
}

pub fn discover() -> Result<Vec<Discovered>> {
    let stored = keychain_credentials()
        .or_else(fallback_file_credentials)
        .unwrap_or_default();

    let Some(oauth) = stored.and_then(|stored| stored.claude_ai_oauth) else {
        return Ok(Vec::new());
    };
    if oauth.access_token.is_empty() {
        return Ok(Vec::new());
    }

    let plan = oauth
        .rate_limit_tier
        .clone()
        .or(oauth.subscription_type.clone());
    let label = plan
        .clone()
        .map(|plan| format!("cli ({plan})"))
        .unwrap_or_else(|| "cli".to_owned());

    Ok(vec![Discovered {
        account: Account::new(Provider::Claude, label, plan),
        credential: Credential {
            access_token: oauth.access_token,
            refresh_token: oauth.refresh_token,
            expires_at: oauth
                .expires_at
                .filter(|value| *value > 0)
                .and_then(|millis| Utc.timestamp_millis_opt(millis).single()),
            extra: Default::default(),
        },
    }])
}

fn keychain_service() -> String {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(directory) if !directory.is_empty() => {
            let digest = Sha256::digest(directory.as_bytes());
            let hash: String = digest
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
                .chars()
                .take(8)
                .collect();
            format!("Claude Code-credentials-{hash}")
        }
        _ => "Claude Code-credentials".to_owned(),
    }
}

fn keychain_credentials() -> Option<Option<StoredCredentials>> {
    let user = std::env::var("USER").ok()?;
    let raw = keyring::Entry::new(&keychain_service(), &user)
        .ok()?
        .get_password()
        .ok()?;
    Some(serde_json::from_str(&raw).ok())
}

fn fallback_path() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => dirs::home_dir()?.join(".claude"),
    };
    Some(base.join(".credentials.json"))
}

fn fallback_file_credentials() -> Option<Option<StoredCredentials>> {
    let raw = std::fs::read_to_string(fallback_path()?).ok()?;
    Some(serde_json::from_str(&raw).ok())
}
