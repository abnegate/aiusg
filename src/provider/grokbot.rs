use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;

use crate::model::{Account, Provider, Window};
use crate::oauth::decode_jwt_claims;
use crate::provider::{Discovered, Fetched, cursor};
use crate::store::Credential;

const USAGE_URL: &str = "https://api2.cursor.sh/aiserver.v1.DashboardService/GetSandUsageStatus";
const USER_AGENT: &str = concat!("aiusg/", env!("CARGO_PKG_VERSION"));
const CLIENT_TYPE: &str = "sand";
const SECRETS_ENV: &str = "AIUSG_GROKBOT_SECRETS";
const ACCESS_TOKEN_KEY: &str = "cursor-access-token";
const REFRESH_TOKEN_KEY: &str = "cursor-refresh-token";
const TEAM_ID_KEY: &str = "cursor-selected-team-id";
const PLAINTEXT: &str = "plaintext:v1:";
const ACCOUNT_KEY: &str = "account";
const TEAM_ID: &str = "team_id";
const WINDOW: &str = "Included usage";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageStatus {
    #[serde(default)]
    usage_percent: Option<f64>,
    #[serde(default)]
    next_reset_timestamp_utc: Option<DateTime<Utc>>,
    #[serde(default)]
    uses_pooled_enterprise_allowance: bool,
    #[serde(default)]
    grok_plan_label: Option<String>,
    #[serde(default)]
    included_usage_super_grok_plan: Option<String>,
}

pub async fn fetch(http: &reqwest::Client, credential: &Credential) -> Result<Fetched> {
    let mut request = http
        .post(USAGE_URL)
        .header(
            "Authorization",
            format!("Bearer {}", credential.access_token),
        )
        .header("Connect-Protocol-Version", "1")
        .header("X-Cursor-Client-Type", CLIENT_TYPE)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .json(&serde_json::json!({}));

    if let Some(team) = credential.get(TEAM_ID) {
        request = request.header("X-Cursor-Team-Id", team);
    }

    let response = request.send().await.context("requesting Grok Bot usage")?;

    let status: UsageStatus = crate::provider::read_json(response, "Grok Bot usage").await?;

    Ok(Fetched {
        plan: plan(&status),
        windows: windows(&status),
    })
}

fn plan(status: &UsageStatus) -> Option<String> {
    status
        .grok_plan_label
        .clone()
        .or_else(|| status.included_usage_super_grok_plan.clone())
}

fn windows(status: &UsageStatus) -> Vec<Window> {
    if status.uses_pooled_enterprise_allowance {
        return Vec::new();
    }
    let Some(percent) = status.usage_percent.filter(|percent| percent.is_finite()) else {
        return Vec::new();
    };

    vec![
        Window::from_percent(WINDOW, percent.max(0.0))
            .resetting_at(status.next_reset_timestamp_utc),
    ]
}

pub async fn login(_http: &reqwest::Client) -> Result<Discovered> {
    discover()?
        .into_iter()
        .next()
        .context("sign in to the Grok Bot app first, then run `aiusg login grokbot` again")
}

pub async fn refresh(
    _http: &reqwest::Client,
    credential: &Credential,
) -> Result<Option<Credential>> {
    Ok(Some(refreshed_credential(credential, discover()?)))
}

fn refreshed_credential(credential: &Credential, sessions: Vec<Discovered>) -> Credential {
    let Some(account) = credential.get(ACCOUNT_KEY) else {
        return credential.clone();
    };
    sessions
        .into_iter()
        .find(|found| found.credential.get(ACCOUNT_KEY) == Some(account))
        .map(|found| found.credential)
        .unwrap_or_else(|| credential.clone())
}

#[derive(Debug, Default, Deserialize)]
struct Secrets {
    #[serde(default)]
    active: Option<String>,
    #[serde(default)]
    accounts: BTreeMap<String, BTreeMap<String, String>>,
}

fn secrets_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SECRETS_ENV) {
        return Some(PathBuf::from(path));
    }
    let base = if cfg!(target_os = "macos") {
        dirs::home_dir()?
            .join("Library")
            .join("Application Support")
    } else {
        dirs::config_dir()?
    };
    Some(base.join("Grok Bot").join("sand-secrets.json"))
}

fn readable(value: &str) -> Option<&str> {
    value
        .strip_prefix(PLAINTEXT)
        .filter(|rest| !rest.is_empty())
}

fn credentials(secrets: Secrets) -> Vec<Credential> {
    let ordered = secrets
        .active
        .into_iter()
        .chain(secrets.accounts.keys().cloned())
        .collect::<Vec<_>>();

    let mut seen = Vec::new();
    let mut found = Vec::new();
    for scope in ordered {
        if seen.contains(&scope) {
            continue;
        }
        let Some(values) = secrets.accounts.get(&scope) else {
            continue;
        };
        seen.push(scope.clone());

        let Some(access_token) = values
            .get(ACCESS_TOKEN_KEY)
            .and_then(|value| readable(value))
        else {
            continue;
        };

        let mut credential = Credential {
            access_token: access_token.to_owned(),
            refresh_token: values
                .get(REFRESH_TOKEN_KEY)
                .and_then(|value| readable(value))
                .map(str::to_owned),
            expires_at: expiry(access_token),
            extra: Default::default(),
        }
        .with(ACCOUNT_KEY, scope);
        if let Some(team) = values.get(TEAM_ID_KEY).and_then(|value| readable(value)) {
            credential = credential.with(TEAM_ID, team.to_owned());
        }
        found.push(credential);
    }
    found
}

fn expiry(token: &str) -> Option<DateTime<Utc>> {
    let seconds = decode_jwt_claims(token).ok()?.get("exp")?.as_i64()?;
    Utc.timestamp_opt(seconds, 0).single()
}

fn label(credential: &Credential) -> String {
    decode_jwt_claims(&credential.access_token)
        .ok()
        .and_then(|claims| {
            claims
                .get("email")
                .or_else(|| claims.get("sub"))?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "grokbot".to_owned())
}

pub fn discover() -> Result<Vec<Discovered>> {
    let stored = match secrets_path().filter(|path| path.exists()) {
        Some(path) => {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let secrets: Secrets = serde_json::from_str(&raw)
                .with_context(|| format!("parsing {}", path.display()))?;
            credentials(secrets)
                .into_iter()
                .map(|credential| (label(&credential), credential))
                .collect()
        }
        None => Vec::new(),
    };

    let sessions: Vec<(String, Credential)> = if stored.is_empty() {
        cursor::discover()?
            .into_iter()
            .map(|found| (found.account.label, found.credential))
            .collect()
    } else {
        stored
    };

    Ok(sessions
        .into_iter()
        .map(|(label, credential)| Discovered {
            account: Account::new(Provider::GrokBot, label, None),
            credential,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIVE: &str = r#"{"currentPeriodStart":"2026-09-07T04:00:16.825Z","nextResetTimestampUtc":"2026-09-14T04:00:16.825Z","usagePercent":42.984557,"hasAvailableUsage":true,"hasNonZeroIncludedLimit":true,"onDemandSettings":{"visible":true,"eligible":true,"dashboardUrl":"https://cursor.com/dashboard/spending"},"grokPlanLabel":"Grok Bot Plan"}"#;

    #[test]
    fn maps_the_live_status() {
        let status: UsageStatus = serde_json::from_str(LIVE).expect("payload should parse");
        assert_eq!(plan(&status).as_deref(), Some("Grok Bot Plan"));

        let windows = windows(&status);
        assert_eq!(windows.len(), 1, "the included window is the only one");
        assert_eq!(windows[0].name, WINDOW);
        assert_eq!(windows[0].used_percent, Some(42.984557));
        assert!(
            windows[0].resets_at.is_some(),
            "reset comes from nextResetTimestampUtc"
        );
    }

    #[test]
    fn pooled_enterprise_usage_reports_no_window() {
        let status: UsageStatus =
            serde_json::from_str(r#"{"usagePercent":80.0,"usesPooledEnterpriseAllowance":true}"#)
                .unwrap();
        assert!(
            windows(&status).is_empty(),
            "a pooled allowance is not this account's to report"
        );
    }

    #[test]
    fn status_without_a_percent_reports_no_window() {
        let status: UsageStatus =
            serde_json::from_str(r#"{"nextResetTimestampUtc":"2026-09-14T04:00:16.825Z"}"#)
                .unwrap();
        assert!(windows(&status).is_empty());
    }

    #[test]
    fn falls_back_to_the_super_grok_plan_name() {
        let status: UsageStatus =
            serde_json::from_str(r#"{"includedUsageSuperGrokPlan":"SuperGrok Heavy"}"#).unwrap();
        assert_eq!(plan(&status).as_deref(), Some("SuperGrok Heavy"));
    }

    #[test]
    fn reads_plaintext_secrets_and_skips_encrypted_ones() {
        let secrets: Secrets = serde_json::from_str(
            r#"{"active":"two","accounts":{
                "one":{"cursor-access-token":"scoped:v1:AQAAA"},
                "two":{"cursor-access-token":"plaintext:v1:token-two","cursor-refresh-token":"plaintext:v1:refresh-two","cursor-selected-team-id":"plaintext:v1:42"}
            }}"#,
        )
        .unwrap();

        let credentials = credentials(secrets);
        assert_eq!(
            credentials.len(),
            1,
            "a safeStorage value cannot be decrypted here"
        );
        assert_eq!(credentials[0].access_token, "token-two");
        assert_eq!(credentials[0].refresh_token.as_deref(), Some("refresh-two"));
        assert_eq!(credentials[0].get(TEAM_ID), Some("42"));
        assert_eq!(credentials[0].get(ACCOUNT_KEY), Some("two"));
    }

    #[test]
    fn reads_the_active_account_first() {
        let secrets: Secrets = serde_json::from_str(
            r#"{"active":"second","accounts":{
                "first":{"cursor-access-token":"plaintext:v1:first"},
                "second":{"cursor-access-token":"plaintext:v1:second"}
            }}"#,
        )
        .unwrap();

        let tokens: Vec<_> = credentials(secrets)
            .into_iter()
            .map(|credential| credential.access_token)
            .collect();
        assert_eq!(tokens, ["second", "first"]);
    }

    #[test]
    fn refresh_uses_the_matching_account_session() {
        let stored = Credential::bearer("stored").with(ACCOUNT_KEY, "second");
        let sessions = [
            Credential::bearer("first").with(ACCOUNT_KEY, "first"),
            Credential::bearer("second").with(ACCOUNT_KEY, "second"),
        ]
        .into_iter()
        .map(|credential| Discovered {
            account: Account::new(Provider::GrokBot, "grokbot", None),
            credential,
        })
        .collect();

        assert_eq!(
            refreshed_credential(&stored, sessions).access_token,
            "second"
        );
    }

    #[test]
    fn refresh_preserves_the_stored_credential_without_a_matching_session() {
        let stored = Credential::bearer("stored").with(ACCOUNT_KEY, "missing");
        let sessions = vec![Discovered {
            account: Account::new(Provider::GrokBot, "grokbot", None),
            credential: Credential::bearer("other").with(ACCOUNT_KEY, "other"),
        }];

        assert_eq!(
            refreshed_credential(&stored, sessions).access_token,
            "stored"
        );
    }
}
