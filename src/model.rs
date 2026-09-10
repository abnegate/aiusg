use std::cmp::Ordering;
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

impl Usage {
    pub fn limiting_window(&self) -> Option<&Window> {
        self.windows
            .iter()
            .filter(|window| window.used_percent.is_some())
            .max_by(|left, right| {
                left.used_percent
                    .unwrap_or(0.0)
                    .total_cmp(&right.used_percent.unwrap_or(0.0))
            })
    }

    pub fn headroom(&self) -> f64 {
        self.limiting_window()
            .and_then(|window| window.used_percent)
            .map_or(100.0, |used| 100.0 - used)
    }

    pub fn usable_at(&self) -> Option<DateTime<Utc>> {
        let mut latest: Option<DateTime<Utc>> = None;
        for window in self.windows.iter().filter(|window| window.is_exhausted()) {
            let resets_at = window.resets_at?;
            latest = Some(latest.map_or(resets_at, |current| current.max(resets_at)));
        }
        latest
    }
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

    pub fn usability(&self) -> Usability {
        match self {
            Report::Ok(usage) => {
                let headroom = usage.headroom();
                if headroom > 0.0 {
                    Usability::Available { headroom }
                } else {
                    Usability::Exhausted {
                        usable_at: usage.usable_at(),
                    }
                }
            }
            Report::Failed { .. } => Usability::Failing,
            Report::SignedOut { .. } => Usability::SignedOut,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Usability {
    Available { headroom: f64 },
    Exhausted { usable_at: Option<DateTime<Utc>> },
    Failing,
    SignedOut,
}

impl Usability {
    fn tier(self) -> u8 {
        match self {
            Usability::Available { .. } => 0,
            Usability::Exhausted { .. } => 1,
            Usability::Failing => 2,
            Usability::SignedOut => 3,
        }
    }
}

impl Ord for Usability {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Usability::Available { headroom: left }, Usability::Available { headroom: right }) => {
                right.total_cmp(left)
            }
            (
                Usability::Exhausted { usable_at: left },
                Usability::Exhausted { usable_at: right },
            ) => match (left, right) {
                (Some(left), Some(right)) => left.cmp(right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            _ => self.tier().cmp(&other.tier()),
        }
    }
}

impl PartialOrd for Usability {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Usability {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for Usability {}

pub fn rank(reports: &mut [Report]) {
    reports.sort_by_key(|report| report.usability());
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn usage(label: &str, windows: Vec<Window>) -> Report {
        Report::Ok(Usage {
            account: AccountId::new(Provider::Claude, label),
            provider: Provider::Claude,
            label: label.to_owned(),
            plan: None,
            windows,
            fetched_at: Utc::now(),
        })
    }

    fn spent(name: &str, resets_in: Option<Duration>) -> Window {
        Window::from_percent(name, 100.0).resetting_at(resets_in.map(|ahead| Utc::now() + ahead))
    }

    fn signed_out(label: &str) -> Report {
        Report::SignedOut {
            account: AccountId::new(Provider::Codex, label),
            provider: Provider::Codex,
            label: label.to_owned(),
        }
    }

    fn failed(label: &str) -> Report {
        Report::Failed {
            account: AccountId::new(Provider::Grok, label),
            provider: Provider::Grok,
            label: label.to_owned(),
            message: "timeout".to_owned(),
        }
    }

    fn ranked(mut reports: Vec<Report>) -> Vec<String> {
        rank(&mut reports);
        reports
            .iter()
            .map(|report| report.label().to_owned())
            .collect()
    }

    #[test]
    fn the_least_used_account_ranks_first() {
        let order = ranked(vec![
            usage("half", vec![Window::from_percent("7d", 50.0)]),
            usage(
                "busy",
                vec![
                    Window::from_percent("5h", 5.0),
                    Window::from_percent("7d", 90.0),
                ],
            ),
            usage("fresh", vec![Window::from_percent("7d", 1.0)]),
        ]);

        assert_eq!(order, ["fresh", "half", "busy"]);
    }

    #[test]
    fn an_account_reporting_no_limits_counts_as_untouched() {
        let order = ranked(vec![
            usage("known", vec![Window::from_percent("7d", 1.0)]),
            usage("unknown", vec![]),
        ]);

        assert_eq!(order, ["unknown", "known"]);
    }

    #[test]
    fn accounts_with_the_same_headroom_keep_the_stored_order() {
        let order = ranked(vec![
            usage("first", vec![Window::from_percent("7d", 40.0)]),
            usage("second", vec![Window::from_percent("7d", 40.0)]),
        ]);

        assert_eq!(order, ["first", "second"]);
    }

    #[test]
    fn exhausted_accounts_sink_below_usable_ones() {
        let order = ranked(vec![
            usage("spent", vec![spent("7d", Some(Duration::hours(1)))]),
            usage("scarce", vec![Window::from_percent("7d", 99.9)]),
        ]);

        assert_eq!(order, ["scarce", "spent"]);
    }

    #[test]
    fn the_exhausted_account_that_comes_back_soonest_ranks_higher() {
        let order = ranked(vec![
            usage("later", vec![spent("7d", Some(Duration::days(2)))]),
            usage("unknown", vec![spent("7d", None)]),
            usage("sooner", vec![spent("5h", Some(Duration::hours(3)))]),
        ]);

        assert_eq!(order, ["sooner", "later", "unknown"]);
    }

    #[test]
    fn an_account_is_back_only_once_every_spent_window_resets() {
        let order = ranked(vec![
            usage(
                "two windows",
                vec![
                    spent("5h", Some(Duration::hours(1))),
                    spent("7d", Some(Duration::days(3))),
                ],
            ),
            usage("one window", vec![spent("7d", Some(Duration::days(1)))]),
        ]);

        assert_eq!(order, ["one window", "two windows"]);
    }

    #[test]
    fn accounts_that_cannot_answer_rank_last() {
        let order = ranked(vec![
            signed_out("gone"),
            failed("broken"),
            usage("spent", vec![spent("7d", Some(Duration::hours(1)))]),
            usage("fine", vec![Window::from_percent("7d", 10.0)]),
        ]);

        assert_eq!(order, ["fine", "spent", "broken", "gone"]);
    }

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
