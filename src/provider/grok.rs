use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::model::{Account, Provider, Window};
use crate::oauth::{Loopback, Pkce, decode_jwt_claims, prompt_open, random_token};
use crate::provider::{Discovered, Fetched};
use crate::store::Credential;

const DEFAULT_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
const AUTHORIZE_URL: &str = "https://auth.x.ai/oauth2/authorize";
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPES: &str = "openid profile email offline_access grok-cli:access api:access billing:read";
const CALLBACK_PORTS: [u16; 4] = [8111, 8112, 8113, 0];
const TOKEN_AUTH: &str = "xai-grok-cli";
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"));
const TEAM_ID: &str = "team_id";
const USER_ID: &str = "user_id";

fn base() -> String {
    std::env::var("GROK_CLI_CHAT_PROXY_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.to_owned())
}

#[derive(Debug, Default, Deserialize)]
struct BillingEnvelope {
    #[serde(default)]
    config: Billing,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Billing {
    #[serde(default)]
    credit_usage_percent: Option<f64>,
    #[serde(default)]
    monthly_limit: Option<f64>,
    #[serde(default)]
    included_used: Option<f64>,
    #[serde(default)]
    total_used: Option<f64>,
    #[serde(default)]
    on_demand_used: Option<Amount>,
    #[serde(default)]
    on_demand_cap: Option<Amount>,
    #[serde(default)]
    product_usage: Vec<ProductUsage>,
    #[serde(default)]
    billing_period_end: Option<DateTime<Utc>>,
    #[serde(default)]
    current_period: Option<Period>,
}

#[derive(Debug, Default, Deserialize)]
struct Amount {
    #[serde(default)]
    val: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProductUsage {
    #[serde(default)]
    product: Option<String>,
    #[serde(default)]
    usage_percent: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
struct Period {
    #[serde(default)]
    end: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize)]
struct Settings {
    #[serde(default)]
    subscription_tier_display: Option<String>,
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let base = base();
    let response = http
        .get(format!("{base}/billing?format=credits"))
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("X-XAI-Token-Auth", TOKEN_AUTH)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .context("requesting Grok billing")?;

    let envelope: BillingEnvelope = crate::provider::read_json(response, "Grok billing").await?;
    let windows = windows(envelope.config);

    Ok(Fetched {
        plan: tier(http, credential)
            .await
            .or_else(|| jwt_tier(credential)),
        windows,
    })
}

fn windows(billing: Billing) -> Vec<Window> {
    let resets_at = billing.billing_period_end.or_else(|| {
        billing
            .current_period
            .as_ref()
            .and_then(|period| period.end)
    });

    let mut windows = Vec::new();

    for usage in &billing.product_usage {
        if let Some(percent) = usage.usage_percent {
            let name = usage
                .product
                .clone()
                .unwrap_or_else(|| "Credits".to_owned());
            windows.push(Window::from_percent(name, percent).resetting_at(resets_at));
        }
    }

    match (
        billing.included_used.or(billing.total_used),
        billing.monthly_limit,
    ) {
        (Some(used), Some(limit)) if limit > 0.0 => windows.push(
            Window::from_count("Credits", used.max(0.0) as u64, limit as u64)
                .resetting_at(resets_at),
        ),
        _ => {
            if windows.is_empty()
                && let Some(percent) = billing.credit_usage_percent
            {
                windows.push(Window::from_percent("Credits", percent).resetting_at(resets_at));
            }
        }
    }

    if let (Some(used), Some(cap)) = (&billing.on_demand_used, &billing.on_demand_cap)
        && cap.val > 0.0
    {
        windows.push(
            Window::from_count("On demand", used.val.max(0.0) as u64, cap.val as u64)
                .resetting_at(resets_at),
        );
    }

    windows
}

async fn tier(http: &reqwest::Client, credential: &Credential) -> Option<String> {
    let settings: Settings = http
        .get(format!("{}/settings", base()))
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("X-XAI-Token-Auth", TOKEN_AUTH)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    settings.subscription_tier_display
}

fn jwt_tier(credential: &Credential) -> Option<String> {
    let claims = decode_jwt_claims(&credential.access_token).ok()?;
    let tier = claims.get("tier")?.as_i64()?;
    Some(format!("tier {tier}"))
}

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let pkce = Pkce::generate();
    let state = random_token(32);
    let loopback = Loopback::bind(&CALLBACK_PORTS).await?;
    let redirect = format!("http://127.0.0.1:{}/callback", loopback.port());

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
        .context("exchanging the Grok authorization code")?
        .json()
        .await
        .context("parsing the Grok token response")?;

    let credential = token.into_credential();
    let label = identify(&credential).unwrap_or_else(|| "grok".to_owned());

    Ok(Discovered {
        account: Account::new(Provider::Grok, label, jwt_tier(&credential)),
        credential,
    })
}

fn identify(credential: &Credential) -> Option<String> {
    let claims = decode_jwt_claims(&credential.access_token).ok()?;
    claims
        .get("email")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
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
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", CLIENT_ID),
        ])
        .send()
        .await
        .context("refreshing the Grok token")?
        .error_for_status()
        .context("xAI rejected the refresh token")?
        .json()
        .await
        .context("parsing the refreshed Grok token")?;

    let mut refreshed = token.into_credential();
    if refreshed.refresh_token.is_none() {
        refreshed.refresh_token = credential.refresh_token.clone();
    }
    refreshed.extra = credential.extra.clone();
    Ok(Some(refreshed))
}

#[derive(Debug, Deserialize)]
struct StoredAccount {
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    user_id: Option<String>,
    #[serde(default)]
    team_id: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<DateTime<Utc>>,
}

fn auth_path() -> Option<PathBuf> {
    let base = match std::env::var_os("GROK_HOME") {
        Some(directory) => PathBuf::from(directory),
        None => dirs::home_dir()?.join(".grok"),
    };
    Some(base.join("auth.json"))
}

pub fn discover() -> Result<Vec<Discovered>> {
    let Some(path) = auth_path().filter(|path| path.exists()) else {
        return Ok(Vec::new());
    };
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let stored: BTreeMap<String, StoredAccount> =
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;

    let mut found = Vec::new();
    for (issuer, account) in stored {
        let Some(key) = account.key.filter(|key| !key.is_empty()) else {
            continue;
        };
        let label = account
            .email
            .clone()
            .or_else(|| issuer.rsplit("::").next().map(str::to_owned))
            .unwrap_or_else(|| "grok".to_owned());

        let mut credential = Credential {
            access_token: key,
            refresh_token: account.refresh_token,
            expires_at: account.expires_at,
            extra: Default::default(),
        };
        if let Some(team) = account.team_id {
            credential = credential.with(TEAM_ID, team);
        }
        if let Some(user) = account.user_id {
            credential = credential.with(USER_ID, user);
        }

        let plan = jwt_tier(&credential);
        found.push(Discovered {
            account: Account::new(Provider::Grok, label, plan),
            credential,
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"config":{"currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2026-09-04T02:07:17.527597+00:00","end":"2026-09-11T02:07:17.527597+00:00"},"creditUsagePercent":100.0,"onDemandCap":{"val":0},"onDemandUsed":{"val":0},"productUsage":[{"product":"GrokBuild","usagePercent":100.0}],"isUnifiedBillingUser":true,"prepaidBalance":{"val":0},"topUpMethod":"TOP_UP_METHOD_SAVED_PAYMENT_METHOD","billingPeriodStart":"2026-09-04T02:07:17.527597+00:00","billingPeriodEnd":"2026-09-11T02:07:17.527597+00:00"}}"#;

    #[test]
    fn maps_live_billing_payload() {
        let envelope: BillingEnvelope = serde_json::from_str(LIVE).expect("payload should parse");
        let windows = windows(envelope.config);

        assert_eq!(
            windows.len(),
            1,
            "only the metered product should produce a window"
        );
        assert_eq!(windows[0].name, "GrokBuild");
        assert_eq!(windows[0].used_percent, Some(100.0));
        assert!(windows[0].is_exhausted(), "100% should read as exhausted");
        assert!(
            windows[0].resets_at.is_some(),
            "reset time should come from the billing period"
        );
    }

    #[test]
    fn zero_on_demand_cap_produces_no_window() {
        let envelope: BillingEnvelope = serde_json::from_str(LIVE).unwrap();
        let windows = windows(envelope.config);
        assert!(
            !windows.iter().any(|window| window.name == "On demand"),
            "a zero cap must not render an on-demand window"
        );
    }

    #[test]
    fn falls_back_to_credit_percent_without_product_usage() {
        let envelope: BillingEnvelope =
            serde_json::from_str(r#"{"config":{"creditUsagePercent":42.0}}"#).unwrap();
        let windows = windows(envelope.config);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used_percent, Some(42.0));
    }

    #[test]
    fn prefers_absolute_counts_when_present() {
        let envelope: BillingEnvelope = serde_json::from_str(
            r#"{"config":{"includedUsed":250.0,"monthlyLimit":1000.0,"creditUsagePercent":25.0}}"#,
        )
        .unwrap();
        let windows = windows(envelope.config);
        assert_eq!(windows[0].used, Some(250));
        assert_eq!(windows[0].limit, Some(1000));
        assert_eq!(windows[0].used_percent, Some(25.0));
    }
}
