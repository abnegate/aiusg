mod profile;
mod profile_account;
mod profile_organization;

use std::path::{Path, PathBuf};

#[cfg(feature = "login")]
use anyhow::bail;
use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
#[cfg(feature = "keychain")]
use sha2::{Digest, Sha256};

use crate::model::{Account, Provider, Window};
#[cfg(feature = "login")]
use crate::oauth::{Loopback, Pkce, prompt_open, random_token};
use crate::provider::{Discovered, Fetched, endpoint, load, read_json};
use crate::store::Credential;

pub use profile::Profile;
pub use profile_account::ProfileAccount;
pub use profile_organization::ProfileOrganization;

pub const BASE: &str = "https://api.anthropic.com";
const USAGE_PATH: &str = "/api/oauth/usage";
const PROFILE_PATH: &str = "/api/oauth/profile";
const CREDENTIALS_FILE: &str = ".credentials.json";
const CONFIG_DIRECTORY: &str = ".claude";
const CONFIG_ENV: &str = "CLAUDE_CONFIG_DIR";
#[cfg(feature = "login")]
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_BETA: &str = "oauth-2025-04-20";
const SCOPES: &str =
    "user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload";
#[cfg(feature = "login")]
const CALLBACK_PORTS: [u16; 4] = [54545, 54546, 54547, 0];
const USER_AGENT: &str = "claude-code/2.1.251";

#[derive(Debug, Default, Deserialize)]
struct UsageResponse {
    #[serde(default, deserialize_with = "crate::provider::nullable")]
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
    fetch_at(http, BASE, credential).await
}

pub async fn fetch_at(
    http: &reqwest::Client,
    base: &str,
    credential: &Credential,
) -> Result<Fetched> {
    let (usage, profile) =
        tokio::try_join!(usage_at(http, base, &credential.access_token), async {
            anyhow::Ok(profile_at(http, base, &credential.access_token).await.ok())
        },)?;

    Ok(Fetched {
        plan: profile.and_then(|profile| profile.plan().map(str::to_owned)),
        windows: windows(usage),
    })
}

async fn usage_at(http: &reqwest::Client, base: &str, access_token: &str) -> Result<UsageResponse> {
    let response = request(http, access_token, &endpoint(base, USAGE_PATH))
        .send()
        .await
        .context("requesting Claude usage")?;
    read_json(response, "Claude usage").await
}

pub async fn profile(http: &reqwest::Client, access_token: &str) -> Result<Profile> {
    profile_at(http, BASE, access_token).await
}

pub async fn profile_at(http: &reqwest::Client, base: &str, access_token: &str) -> Result<Profile> {
    let response = request(http, access_token, &endpoint(base, PROFILE_PATH))
        .send()
        .await
        .context("requesting the Claude profile")?;
    read_json(response, "Claude profile").await
}

fn request(http: &reqwest::Client, access_token: &str, url: &str) -> reqwest::RequestBuilder {
    http.get(url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", OAUTH_BETA)
        .header("Content-Type", "application/json")
        .header("User-Agent", USER_AGENT)
}

#[cfg(feature = "login")]
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
    let label = profile(http, &credential.access_token)
        .await
        .ok()
        .and_then(|profile| profile.label().map(str::to_owned))
        .unwrap_or_else(|| "claude".to_owned());
    Ok(Discovered {
        account: Account::new(Provider::Claude, label, None),
        credential,
    })
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
    if let Some(stored) = keychain_credentials() {
        return Ok(stored.map(discovered).unwrap_or_default());
    }
    match config_dir() {
        Some(directory) => discover_in(&directory),
        None => Ok(Vec::new()),
    }
}

pub fn discover_in(config_dir: &Path) -> Result<Vec<Discovered>> {
    let stored: Option<StoredCredentials> = load(&config_dir.join(CREDENTIALS_FILE))?;
    Ok(stored.map(discovered).unwrap_or_default())
}

fn discovered(stored: StoredCredentials) -> Vec<Discovered> {
    let Some(oauth) = stored
        .claude_ai_oauth
        .filter(|oauth| !oauth.access_token.is_empty())
    else {
        return Vec::new();
    };

    let plan = oauth
        .rate_limit_tier
        .clone()
        .or(oauth.subscription_type.clone());
    let label = plan
        .clone()
        .map(|plan| format!("cli ({plan})"))
        .unwrap_or_else(|| "cli".to_owned());

    vec![Discovered {
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
    }]
}

#[cfg(feature = "keychain")]
fn keychain_service() -> String {
    match std::env::var(CONFIG_ENV) {
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

#[cfg(feature = "keychain")]
fn keychain_credentials() -> Option<Option<StoredCredentials>> {
    let user = std::env::var("USER").ok()?;
    let raw = keyring::Entry::new(&keychain_service(), &user)
        .ok()?
        .get_password()
        .ok()?;
    Some(serde_json::from_str(&raw).ok())
}

#[cfg(not(feature = "keychain"))]
fn keychain_credentials() -> Option<Option<StoredCredentials>> {
    None
}

fn config_dir() -> Option<PathBuf> {
    match std::env::var_os(CONFIG_ENV) {
        Some(directory) => Some(PathBuf::from(directory)),
        None => Some(dirs::home_dir()?.join(CONFIG_DIRECTORY)),
    }
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

    fn config_dir_with(credentials: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(CREDENTIALS_FILE), credentials).unwrap();
        directory
    }

    #[test]
    fn discover_in_reads_the_credentials_file_of_the_config_dir_it_is_given() {
        let directory = config_dir_with(
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-token","refreshToken":"sk-ant-ort01-refresh","expiresAt":1790000000000,"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#,
        );

        let found = discover_in(directory.path()).unwrap();

        assert_eq!(found.len(), 1);
        let discovered = &found[0];
        assert_eq!(discovered.account.provider, Provider::Claude);
        assert_eq!(
            discovered.account.plan.as_deref(),
            Some("default_claude_max_20x"),
            "the rate limit tier wins over the subscription type"
        );
        assert_eq!(discovered.credential.access_token, "sk-ant-oat01-token");
        assert_eq!(
            discovered.credential.refresh_token.as_deref(),
            Some("sk-ant-ort01-refresh")
        );
        assert_eq!(
            discovered.credential.expires_at,
            Utc.timestamp_millis_opt(1_790_000_000_000).single(),
            "expiresAt is in milliseconds"
        );
    }

    #[test]
    fn discover_in_a_config_dir_without_credentials_finds_nothing() {
        let directory = tempfile::tempdir().unwrap();
        assert!(discover_in(directory.path()).unwrap().is_empty());
        assert!(
            discover_in(&directory.path().join("never-created"))
                .unwrap()
                .is_empty(),
            "a config dir that does not exist yet is not an error"
        );
    }

    #[test]
    fn discover_in_skips_an_empty_token() {
        let directory = config_dir_with(r#"{"claudeAiOauth":{"accessToken":""}}"#);
        assert!(discover_in(directory.path()).unwrap().is_empty());

        let signed_out = config_dir_with("{}");
        assert!(discover_in(signed_out.path()).unwrap().is_empty());
    }

    #[test]
    fn discover_in_fails_on_unparsable_credentials() {
        let directory = config_dir_with("{\"claudeAiOauth\":");
        let error = discover_in(directory.path()).unwrap_err();
        assert!(
            format!("{error:#}").contains(CREDENTIALS_FILE),
            "the error names the file: {error:#}"
        );
    }
}
