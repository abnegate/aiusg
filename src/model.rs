use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
    Gemini,
    Copilot,
    Grok,
    GrokBot,
    Cursor,
}

impl Provider {
    pub const ALL: [Provider; 7] = [
        Provider::Claude,
        Provider::Codex,
        Provider::Gemini,
        Provider::Copilot,
        Provider::Grok,
        Provider::GrokBot,
        Provider::Cursor,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Gemini => "gemini",
            Provider::Copilot => "copilot",
            Provider::Grok => "grok",
            Provider::GrokBot => "grokbot",
            Provider::Cursor => "cursor",
        }
    }

    pub fn display(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
            Provider::Gemini => "Gemini",
            Provider::Copilot => "Copilot",
            Provider::Grok => "Grok",
            Provider::GrokBot => "Grok Bot",
            Provider::Cursor => "Cursor",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.slug())
    }
}

impl FromStr for Provider {
    type Err = ProviderParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Provider::ALL
            .into_iter()
            .find(|provider| provider.slug().eq_ignore_ascii_case(value))
            .ok_or_else(|| ProviderParseError(value.to_owned()))
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "unknown provider '{0}' (expected one of: claude, codex, gemini, copilot, grok, grokbot, cursor)"
)]
pub struct ProviderParseError(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    pub fn new(provider: Provider, label: &str) -> Self {
        Self(format!("{}:{}", provider.slug(), label))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_display(value: &str) -> Option<Self> {
        let (provider, label) = value.split_once(':')?;
        let provider: Provider = provider.parse().ok()?;
        (!label.is_empty()).then(|| Self::new(provider, label))
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub id: AccountId,
    pub provider: Provider,
    pub label: String,
    pub plan: Option<String>,
    pub added_at: DateTime<Utc>,
}

impl Account {
    pub fn new(provider: Provider, label: impl Into<String>, plan: Option<String>) -> Self {
        let label = label.into();
        Self {
            id: AccountId::new(provider, &label),
            provider,
            label,
            plan,
            added_at: Utc::now(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    pub name: String,
    pub used_percent: Option<f64>,
    pub used: Option<u64>,
    pub limit: Option<u64>,
    pub resets_at: Option<DateTime<Utc>>,
}

impl Window {
    pub fn from_percent(name: impl Into<String>, used_percent: f64) -> Self {
        Self {
            name: name.into(),
            used_percent: Some(used_percent),
            used: None,
            limit: None,
            resets_at: None,
        }
    }

    pub fn from_count(name: impl Into<String>, used: u64, limit: u64) -> Self {
        let used_percent = if limit == 0 {
            None
        } else {
            Some((used as f64 / limit as f64) * 100.0)
        };
        Self {
            name: name.into(),
            used_percent,
            used: Some(used),
            limit: Some(limit),
            resets_at: None,
        }
    }

    pub fn resetting_at(mut self, resets_at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = resets_at;
        self
    }

    pub fn is_exhausted(&self) -> bool {
        self.used_percent.is_some_and(|used| used >= 100.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    pub account: AccountId,
    pub provider: Provider,
    pub label: String,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    pub fetched_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Report {
    Ok(Usage),
    #[serde(rename = "signed_out")]
    SignedOut {
        account: AccountId,
        provider: Provider,
        label: String,
    },
    Failed {
        account: AccountId,
        provider: Provider,
        label: String,
        message: String,
    },
}

impl Report {
    pub fn provider(&self) -> Provider {
        match self {
            Report::Ok(usage) => usage.provider,
            Report::SignedOut { provider, .. } | Report::Failed { provider, .. } => *provider,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Report::Ok(usage) => &usage.label,
            Report::SignedOut { label, .. } | Report::Failed { label, .. } => label,
        }
    }

    pub fn is_signed_out(&self) -> bool {
        matches!(self, Report::SignedOut { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_become_percentages() {
        let window = Window::from_count("Premium", 750, 1500);
        assert_eq!(window.used_percent, Some(50.0));
    }

    #[test]
    fn overage_exceeds_one_hundred_percent() {
        let window = Window::from_count("Premium", 1506, 1500);
        assert!(window.used_percent.is_some_and(|percent| percent > 100.0));
        assert!(window.is_exhausted());
    }

    #[test]
    fn a_zero_limit_yields_no_percentage() {
        let window = Window::from_count("Chat", 0, 0);
        assert!(
            window.used_percent.is_none(),
            "0/0 must not be reported as a percentage"
        );
        assert!(!window.is_exhausted());
    }

    #[test]
    fn providers_round_trip_through_their_slug() {
        for provider in Provider::ALL {
            let parsed: Provider = provider.slug().parse().expect("slug should parse");
            assert_eq!(parsed, provider);
        }
    }

    #[test]
    fn provider_parsing_is_case_insensitive() {
        assert_eq!("Claude".parse::<Provider>().unwrap(), Provider::Claude);
        assert!("nope".parse::<Provider>().is_err());
    }

    #[test]
    fn account_ids_round_trip() {
        let id = AccountId::new(Provider::Claude, "jake@appwrite.io");
        let parsed = AccountId::from_display(id.as_str()).expect("id should parse");
        assert_eq!(parsed, id);
    }

    #[test]
    fn account_ids_keep_labels_containing_colons() {
        let id = AccountId::from_display("grok:https://auth.x.ai::abc").expect("id should parse");
        assert_eq!(id.as_str(), "grok:https://auth.x.ai::abc");
    }

    #[test]
    fn malformed_account_ids_are_rejected() {
        assert!(AccountId::from_display("claude").is_none());
        assert!(AccountId::from_display("claude:").is_none());
        assert!(AccountId::from_display("bogus:label").is_none());
    }
}
