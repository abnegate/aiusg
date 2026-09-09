use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;

use crate::model::{Account, Provider, Window};
use crate::oauth::{Loopback, prompt_open, random_token};
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const CODE_ASSIST: &str = "https://cloudcode-pa.googleapis.com/v1internal";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const CLIENT_ID_ENV: &str = "AIUSG_GEMINI_CLIENT_ID";
const CLIENT_SECRET_ENV: &str = "AIUSG_GEMINI_CLIENT_SECRET";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";
const CALLBACK_PORTS: [u16; 4] = [8085, 8086, 8087, 0];
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"), " (GeminiCLI)");
const PROJECT: &str = "project";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoadResponse {
    #[serde(default)]
    current_tier: Option<Tier>,
    #[serde(default)]
    paid_tier: Option<Tier>,
    #[serde(default)]
    cloudaicompanion_project: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Tier {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    buckets: Vec<Bucket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Bucket {
    #[serde(default)]
    remaining_amount: Option<String>,
    #[serde(default)]
    remaining_fraction: Option<f64>,
    #[serde(default)]
    reset_time: Option<DateTime<Utc>>,
    #[serde(default)]
    model_id: Option<String>,
}

impl Bucket {
    fn into_window(self) -> Option<Window> {
        let name = self
            .model_id
            .clone()
            .unwrap_or_else(|| "requests".to_owned());
        let remaining = self
            .remaining_amount
            .as_deref()
            .and_then(|value| value.parse::<i64>().ok());

        let window = match (remaining, self.remaining_fraction) {
            (Some(remaining), Some(fraction)) if fraction > 0.0 => {
                let limit = (remaining as f64 / fraction).round().max(0.0) as u64;
                Window::from_count(name, limit.saturating_sub(remaining.max(0) as u64), limit)
            }
            (_, Some(fraction)) => {
                Window::from_percent(name, ((1.0 - fraction) * 100.0).clamp(0.0, 100.0))
            }
            _ => return None,
        };
        Some(window.resetting_at(self.reset_time))
    }
}

async fn load(http: &reqwest::Client, credential: &Credential) -> Result<LoadResponse> {
    let response = http
        .post(format!("{CODE_ASSIST}:loadCodeAssist"))
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("User-Agent", USER_AGENT)
        .json(&json!({
            "metadata": {
                "ideType": "IDE_UNSPECIFIED",
                "platform": "PLATFORM_UNSPECIFIED",
                "pluginType": "GEMINI",
            }
        }))
        .send()
        .await
        .context("requesting the Gemini Code Assist profile")?;

    crate::provider::read_json(response, "Gemini profile").await
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let loaded = load(http, credential).await?;
    let plan = loaded
        .paid_tier
        .as_ref()
        .or(loaded.current_tier.as_ref())
        .and_then(|tier| tier.name.clone().or_else(|| tier.id.clone()));

    let project = loaded
        .cloudaicompanion_project
        .or_else(|| credential.get(PROJECT).map(str::to_owned))
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT").ok());

    let Some(project) = project else {
        return Ok(Fetched {
            plan,
            windows: Vec::new(),
        });
    };

    let response = http
        .post(format!("{CODE_ASSIST}:retrieveUserQuota"))
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("User-Agent", USER_AGENT)
        .json(&json!({ "project": project }))
        .send()
        .await
        .context("requesting Gemini quota")?;

    let quota: QuotaResponse = crate::provider::read_json(response, "Gemini quota").await?;
    Ok(Fetched {
        plan,
        windows: quota
            .buckets
            .into_iter()
            .filter_map(Bucket::into_window)
            .collect(),
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

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let client = client()?;
    let state = random_token(32);
    let loopback = Loopback::bind(&CALLBACK_PORTS).await?;
    let redirect = format!("http://127.0.0.1:{}/oauth2callback", loopback.port());

    let authorize = url::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("client_id", client.id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", &redirect),
            ("scope", SCOPES),
            ("state", &state),
            ("access_type", "offline"),
            ("prompt", "consent"),
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
            ("client_id", client.id.as_str()),
            ("client_secret", client.secret.as_str()),
        ])
        .send()
        .await
        .context("exchanging the Gemini authorization code")?
        .json()
        .await
        .context("parsing the Gemini token response")?;

    let credential = token.into_credential();
    let label = email(http, &credential)
        .await
        .unwrap_or_else(|| "gemini".to_owned());

    Ok(Discovered {
        account: Account::new(Provider::Gemini, label, None),
        credential,
    })
}

async fn email(http: &reqwest::Client, credential: &Credential) -> Option<String> {
    #[derive(Deserialize)]
    struct Info {
        email: Option<String>,
    }

    let info: Info = http
        .get("https://www.googleapis.com/oauth2/v3/userinfo")
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    info.email
}

pub async fn refresh(
    http: &reqwest::Client,
    credential: &Credential,
) -> Result<Option<Credential>> {
    let Some(refresh_token) = credential.refresh_token.as_deref() else {
        return Ok(None);
    };
    let client = client()?;

    let token: TokenResponse = http
        .post(TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client.id.as_str()),
            ("client_secret", client.secret.as_str()),
        ])
        .send()
        .await
        .context("refreshing the Gemini token")?
        .error_for_status()
        .context("Google rejected the refresh token")?
        .json()
        .await
        .context("parsing the refreshed Gemini token")?;

    let mut refreshed = token.into_credential();
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = credential.refresh_token.clone();
    }
    refreshed.extra = credential.extra.clone();
    Ok(Some(refreshed))
}

#[derive(Debug, Deserialize)]
struct StoredCredentials {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expiry_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct StoredAccounts {
    #[serde(default)]
    active: Option<String>,
}

struct Client {
    id: String,
    secret: String,
}

fn client() -> Result<Client> {
    let id = std::env::var(CLIENT_ID_ENV).unwrap_or_default();
    let secret = std::env::var(CLIENT_SECRET_ENV).unwrap_or_default();
    if id.is_empty() || secret.is_empty() {
        bail!(
            "Gemini needs an OAuth client: set {CLIENT_ID_ENV} and {CLIENT_SECRET_ENV} \
             (see the Gemini section of the README)"
        );
    }
    Ok(Client { id, secret })
}

fn home() -> Option<PathBuf> {
    match std::env::var_os("GEMINI_CLI_HOME") {
        Some(directory) => Some(PathBuf::from(directory)),
        None => Some(dirs::home_dir()?.join(".gemini")),
    }
}

pub fn discover() -> Result<Vec<Discovered>> {
    let Some(home) = home() else {
        return Ok(Vec::new());
    };
    let path = home.join("oauth_creds.json");
    if !path.exists() {
        return Ok(Vec::new());
    }

    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let stored: StoredCredentials =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
    if stored.access_token.is_empty() {
        return Ok(Vec::new());
    }

    let label = std::fs::read_to_string(home.join("google_accounts.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<StoredAccounts>(&raw).ok())
        .and_then(|accounts| accounts.active)
        .unwrap_or_else(|| "gemini".to_owned());

    Ok(vec![Discovered {
        account: Account::new(Provider::Gemini, label, None),
        credential: Credential {
            access_token: stored.access_token,
            refresh_token: stored.refresh_token,
            expires_at: stored
                .expiry_date
                .filter(|value| *value > 0)
                .and_then(|millis| Utc.timestamp_millis_opt(millis).single()),
            extra: Default::default(),
        },
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_computes_the_limit_from_the_remaining_fraction() {
        let bucket: Bucket = serde_json::from_str(
            r#"{"remainingAmount":"750","remainingFraction":0.5,"modelId":"gemini-2.5-pro","resetTime":"2026-09-10T00:00:00Z"}"#,
        )
        .unwrap();
        let window = bucket.into_window().expect("bucket should map");

        assert_eq!(
            window.limit,
            Some(1500),
            "750 remaining at 50% implies a 1500 limit"
        );
        assert_eq!(window.used, Some(750));
        assert_eq!(window.name, "gemini-2.5-pro");
        assert!(window.resets_at.is_some());
    }

    #[test]
    fn zero_fraction_does_not_divide_by_zero() {
        let bucket: Bucket =
            serde_json::from_str(r#"{"remainingAmount":"0","remainingFraction":0.0}"#).unwrap();
        let window = bucket.into_window().expect("bucket should still map");
        assert_eq!(
            window.used_percent,
            Some(100.0),
            "no fraction left means fully used"
        );
        assert!(
            window.limit.is_none(),
            "a limit must not be invented from a zero fraction"
        );
    }

    #[test]
    fn fraction_only_bucket_becomes_a_percentage() {
        let bucket: Bucket = serde_json::from_str(r#"{"remainingFraction":0.25}"#).unwrap();
        let window = bucket.into_window().expect("bucket should map");
        assert_eq!(window.used_percent, Some(75.0));
    }

    #[test]
    fn empty_bucket_maps_to_nothing() {
        let bucket: Bucket = serde_json::from_str(r#"{"modelId":"gemini-2.5-flash"}"#).unwrap();
        assert!(bucket.into_window().is_none());
    }
}
