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
    limits: Vec<Limit>,
    #[serde(default)]
    five_hour: Option<LegacyLimit>,
    #[serde(default)]
    seven_day: Option<LegacyLimit>,
    #[serde(default)]
    seven_day_opus: Option<LegacyLimit>,
    #[serde(default)]
    seven_day_sonnet: Option<LegacyLimit>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct Limit {
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    percent: Option<f64>,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    scope: Option<Scope>,
}

#[derive(Debug, Deserialize)]
struct Scope {
    #[serde(default)]
    model: Option<Model>,
}

#[derive(Debug, Deserialize)]
struct Model {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacyLimit {
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
    monthly_limit: Option<f64>,
    #[serde(default)]
    used_credits: Option<f64>,
    #[serde(default)]
    utilization: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct Profile {
    #[serde(default)]
    account: Option<ProfileAccount>,
    #[serde(default)]
    organization: Option<ProfileOrganization>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileAccount {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    full_name: Option<String>,
    #[serde(default)]
    uuid: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProfileOrganization {
    #[serde(default)]
    rate_limit_tier: Option<String>,
    #[serde(default)]
    billing_type: Option<String>,
}

impl Limit {
    fn into_window(self) -> Option<Window> {
        let percent = self.percent?;
        let model = self
            .scope
            .and_then(|scope| scope.model)
            .and_then(|model| model.display_name);

        let name = match (self.kind.as_deref(), model) {
            (Some("session"), _) => "5h".to_owned(),
            (Some("weekly_all"), _) => "7d".to_owned(),
            (Some("weekly_scoped"), Some(model)) => format!("7d {model}"),
            (Some("weekly_scoped"), None) => "7d scoped".to_owned(),
            (Some(kind), Some(model)) => format!("{} {model}", kind.replace('_', " ")),
            (Some(kind), None) => kind.replace('_', " "),
            (None, Some(model)) => model,
            (None, None) => return None,
        };
        Some(Window::from_percent(name, percent).resetting_at(self.resets_at))
    }
}

fn windows(usage: UsageResponse) -> Vec<Window> {
    let mut windows: Vec<Window> = usage
        .limits
        .into_iter()
        .filter_map(Limit::into_window)
        .collect();

    if windows.is_empty() {
        for (name, limit) in [
            ("5h", usage.five_hour),
            ("7d", usage.seven_day),
            ("7d Opus", usage.seven_day_opus),
            ("7d Sonnet", usage.seven_day_sonnet),
        ] {
            if let Some(limit) = limit
                && let Some(utilization) = limit.utilization
            {
                windows.push(Window::from_percent(name, utilization).resetting_at(limit.resets_at));
            }
        }
    }

    if let Some(extra) = usage.extra_usage.filter(|extra| extra.is_enabled) {
        match (extra.used_credits, extra.monthly_limit) {
            (Some(used), Some(limit)) if limit > 0.0 => windows.push(Window::from_count(
                "Extra usage",
                used.max(0.0).round() as u64,
                limit.round() as u64,
            )),
            _ => {
                if let Some(utilization) = extra.utilization {
                    windows.push(Window::from_percent("Extra usage", utilization));
                }
            }
        }
    }

    windows
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let response = request(http, credential, USAGE_URL)
        .send()
        .await
        .context("requesting Claude usage")?;
    let usage: UsageResponse = crate::provider::read_json(response, "Claude usage").await?;

    Ok(Fetched {
        plan: plan(http, credential).await,
        windows: windows(usage),
    })
}

fn request(http: &reqwest::Client, credential: &Credential, url: &str) -> reqwest::RequestBuilder {
    http.get(url)
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("anthropic-beta", OAUTH_BETA)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
}

async fn profile(http: &reqwest::Client, credential: &Credential) -> Option<Profile> {
    request(http, credential, PROFILE_URL)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()
}

async fn plan(http: &reqwest::Client, credential: &Credential) -> Option<String> {
    let profile = profile(http, credential).await?;
    let organization = profile.organization?;
    organization.rate_limit_tier.or(organization.billing_type)
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
    profile(http, credential)
        .await
        .and_then(|profile| profile.account)
        .and_then(|account| {
            account
                .email
                .or(account.display_name)
                .or(account.full_name)
                .or_else(|| account.uuid.map(|uuid| uuid.chars().take(8).collect()))
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"five_hour":{"utilization":0.0,"resets_at":null},"seven_day":{"utilization":100.0,"resets_at":"2026-09-13T11:59:59.968827+00:00"},"nimbus_quill":{"utilization":0.0,"resets_at":null},"limits":[{"kind":"session","group":"session","percent":0,"severity":"normal","resets_at":null,"scope":null,"is_active":false},{"kind":"weekly_all","group":"weekly","percent":100,"severity":"critical","resets_at":"2026-09-13T11:59:59.968827+00:00","scope":null,"is_active":true},{"kind":"weekly_scoped","group":"weekly","percent":29,"severity":"normal","resets_at":"2026-09-13T11:59:59.969130+00:00","scope":{"model":{"id":null,"display_name":"Fable"}},"is_active":false}],"extra_usage":{"is_enabled":false,"monthly_limit":null,"used_credits":0.0,"utilization":null,"currency":"NZD","decimal_places":2,"disabled_reason":"out_of_credits","credits_ever_enabled":true}}"#;

    #[test]
    fn float_credits_do_not_break_parsing() {
        serde_json::from_str::<UsageResponse>(LIVE)
            .expect("used_credits is a float on the wire and must parse");
    }

    #[test]
    fn prefers_the_limits_array_over_the_legacy_keys() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);

        assert_eq!(windows.len(), 3, "one window per limits[] entry");
        assert_eq!(windows[0].name, "5h", "session maps to the 5h window");
        assert_eq!(windows[1].name, "7d");
        assert_eq!(
            windows[2].name, "7d Fable",
            "a scoped limit is named by its model"
        );
    }

    #[test]
    fn carries_percent_and_reset_through() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);
        assert_eq!(windows[1].used_percent, Some(100.0));
        assert!(windows[1].is_exhausted());
        assert!(windows[1].resets_at.is_some());
        assert!(
            windows[0].resets_at.is_none(),
            "a null reset must stay absent"
        );
    }

    #[test]
    fn a_disabled_extra_usage_block_is_skipped() {
        let usage: UsageResponse = serde_json::from_str(LIVE).unwrap();
        let windows = windows(usage);
        assert!(!windows.iter().any(|window| window.name == "Extra usage"));
    }

    #[test]
    fn falls_back_to_legacy_windows_when_limits_is_empty() {
        let usage: UsageResponse = serde_json::from_str(
            r#"{"five_hour":{"utilization":12.5,"resets_at":null},"seven_day":{"utilization":80.0,"resets_at":null}}"#,
        )
        .unwrap();
        let windows = windows(usage);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].name, "5h");
        assert_eq!(windows[0].used_percent, Some(12.5));
    }

    #[test]
    fn the_profile_is_read_from_its_nested_shape() {
        let profile: Profile = serde_json::from_str(
            r#"{"account":{"uuid":"abc12345-ffff","email":"jake@example.com","display_name":"Jake"},"organization":{"rate_limit_tier":"default_claude_max_20x","billing_type":"stripe"}}"#,
        )
        .expect("profile should parse");

        assert_eq!(
            profile.account.unwrap().email.as_deref(),
            Some("jake@example.com"),
            "the account label comes from account.email, not a flat field"
        );
        assert_eq!(
            profile.organization.unwrap().rate_limit_tier.as_deref(),
            Some("default_claude_max_20x")
        );
    }
}
