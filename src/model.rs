//! The accounts, usage windows and reports shared by every provider.

mod availability;

use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub use availability::Availability;

const COPILOT_FEATURE: &str = "copilot";
const CURSOR_FEATURE: &str = "cursor";
const GROKBOT_FEATURE: &str = "grokbot";

/// An AI service whose usage limits aiusg reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Anthropic's Claude, through a Claude Code login.
    Claude,
    /// OpenAI's Codex, through a ChatGPT login.
    Codex,
    /// Google's Gemini Code Assist.
    Gemini,
    /// GitHub Copilot.
    Copilot,
    /// xAI's Grok CLI.
    Grok,
    /// xAI's Grok Bot app.
    GrokBot,
    /// The Cursor editor.
    Cursor,
}

impl Provider {
    /// Every provider, built into this build or not.
    pub const ALL: [Provider; 7] = [
        Provider::Claude,
        Provider::Codex,
        Provider::Gemini,
        Provider::Copilot,
        Provider::Grok,
        Provider::GrokBot,
        Provider::Cursor,
    ];

    /// The lowercase identifier used in account ids, the CLI and JSON.
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

    /// The human-readable name.
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

    /// The Cargo feature that adds this provider, or `None` when every build
    /// has it.
    pub fn feature(self) -> Option<&'static str> {
        match self {
            Provider::Copilot => Some(COPILOT_FEATURE),
            Provider::Cursor => Some(CURSOR_FEATURE),
            Provider::GrokBot => Some(GROKBOT_FEATURE),
            Provider::Claude | Provider::Codex | Provider::Gemini | Provider::Grok => None,
        }
    }

    /// Whether this build has the provider. Calling one it lacks returns
    /// [`Unsupported`](crate::provider::Unsupported).
    pub fn is_built(self) -> bool {
        match self {
            Provider::Copilot => cfg!(feature = "copilot"),
            Provider::Cursor => cfg!(feature = "cursor"),
            Provider::GrokBot => cfg!(feature = "grokbot"),
            Provider::Claude | Provider::Codex | Provider::Gemini | Provider::Grok => true,
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

/// A string that names no [`Provider`] slug.
#[derive(Debug, thiserror::Error)]
#[error(
    "unknown provider '{0}' (expected one of: claude, codex, gemini, copilot, grok, grokbot, cursor)"
)]
pub struct ProviderParseError(pub String);

/// An account's stable identity: `<provider slug>:<label>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    /// The id of the `provider` account labelled `label`.
    pub fn new(provider: Provider, label: &str) -> Self {
        Self(format!("{}:{}", provider.slug(), label))
    }

    /// The id as `<provider slug>:<label>`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Parses an id in its displayed form, or `None` when the provider is
    /// unknown or the label is empty.
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

/// A signed-in account with one provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    /// The account's identity, derived from `provider` and `label`.
    pub id: AccountId,
    /// The provider the account belongs to.
    pub provider: Provider,
    /// The name the account is shown by, usually its email.
    pub label: String,
    /// The subscription plan, when the provider reports one.
    pub plan: Option<String>,
    /// When the account was added.
    pub added_at: DateTime<Utc>,
}

impl Account {
    /// A `provider` account labelled `label`, added now.
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

/// One usage limit, such as a 5-hour session or a weekly cap.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Window {
    /// The limit's name, such as `5h` or `7d`.
    pub name: String,
    /// How much of the limit is used, from 0 up; above 100 is overage.
    pub used_percent: Option<f64>,
    /// The units used, when the provider counts them.
    pub used: Option<u64>,
    /// The units allowed, when the provider counts them.
    pub limit: Option<u64>,
    /// When the limit resets.
    pub resets_at: Option<DateTime<Utc>>,
}

impl Window {
    /// A window `used_percent` used, with no counts or reset.
    pub fn from_percent(name: impl Into<String>, used_percent: f64) -> Self {
        Self {
            name: name.into(),
            used_percent: Some(used_percent),
            used: None,
            limit: None,
            resets_at: None,
        }
    }

    /// A window of `used` out of `limit` units, with no percentage when
    /// `limit` is zero.
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

    /// This window, resetting at `resets_at`.
    pub fn resetting_at(mut self, resets_at: Option<DateTime<Utc>>) -> Self {
        self.resets_at = resets_at;
        self
    }

    /// Whether the window is at or over 100% used.
    pub fn is_exhausted(&self) -> bool {
        self.used_percent.is_some_and(|used| used >= 100.0)
    }
}

/// The usage of one account at one moment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    /// The account the usage belongs to.
    pub account: AccountId,
    /// The account's provider.
    pub provider: Provider,
    /// The account's label.
    pub label: String,
    /// The account's plan.
    pub plan: Option<String>,
    /// Every limit the provider reported.
    pub windows: Vec<Window>,
    /// When the usage was read.
    pub fetched_at: DateTime<Utc>,
}

impl Usage {
    /// The most used window that reports a percentage.
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

    /// The percentage left in the [limiting window](Self::limiting_window),
    /// or 100 when no window reports one.
    pub fn headroom(&self) -> f64 {
        self.limiting_window()
            .and_then(|window| window.used_percent)
            .map_or(100.0, |used| 100.0 - used)
    }

    /// When the account can take work again: [`Availability::Now`] when no
    /// window is spent, otherwise when the last spent window resets, or
    /// [`Availability::Unknown`] when a spent window reports no reset.
    pub fn availability(&self) -> Availability {
        self.windows
            .iter()
            .filter(|window| window.is_exhausted())
            .map(|window| {
                window
                    .resets_at
                    .map_or(Availability::Unknown, Availability::At)
            })
            .max()
            .unwrap_or(Availability::Now)
    }
}

/// The outcome of reading one account's usage.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Report {
    /// The usage was read.
    Ok(Usage),
    /// The provider refused the stored credential.
    #[serde(rename = "signed_out")]
    SignedOut {
        /// The account that is signed out.
        account: AccountId,
        /// The account's provider.
        provider: Provider,
        /// The account's label.
        label: String,
    },
    /// The usage could not be read for another reason.
    Failed {
        /// The account that failed.
        account: AccountId,
        /// The account's provider.
        provider: Provider,
        /// The account's label.
        label: String,
        /// What went wrong.
        message: String,
    },
}

impl Report {
    /// The provider of the reported account.
    pub fn provider(&self) -> Provider {
        match self {
            Report::Ok(usage) => usage.provider,
            Report::SignedOut { provider, .. } | Report::Failed { provider, .. } => *provider,
        }
    }

    /// The label of the reported account.
    pub fn label(&self) -> &str {
        match self {
            Report::Ok(usage) => &usage.label,
            Report::SignedOut { label, .. } | Report::Failed { label, .. } => label,
        }
    }

    /// Whether the report is [`Report::SignedOut`].
    pub fn is_signed_out(&self) -> bool {
        matches!(self, Report::SignedOut { .. })
    }

    /// How usable the reported account is right now.
    pub fn usability(&self) -> Usability {
        match self {
            Report::Ok(usage) => {
                let headroom = usage.headroom();
                if headroom > 0.0 {
                    Usability::Available { headroom }
                } else {
                    Usability::Exhausted {
                        availability: usage.availability(),
                    }
                }
            }
            Report::Failed { .. } => Usability::Failing,
            Report::SignedOut { .. } => Usability::SignedOut,
        }
    }
}

/// How usable an account is, ordered from most to least usable.
#[derive(Clone, Copy, Debug)]
pub enum Usability {
    /// The account has room left; more headroom orders first.
    Available {
        /// The percentage left in the limiting window.
        headroom: f64,
    },
    /// A limit is spent; the soonest to come back orders first.
    Exhausted {
        /// When the account can take work again.
        availability: Availability,
    },
    /// The usage could not be read.
    Failing,
    /// The provider refused the stored credential.
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
                Usability::Exhausted { availability: left },
                Usability::Exhausted {
                    availability: right,
                },
            ) => left.cmp(right),
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

/// Sorts `reports` from most to least usable, keeping the order of equals.
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

    fn usage_of(windows: Vec<Window>) -> Usage {
        let Report::Ok(usage) = usage("account", windows) else {
            unreachable!("usage builds an ok report");
        };
        usage
    }

    #[test]
    fn an_account_with_nothing_spent_is_available_now() {
        let usage = usage_of(vec![
            Window::from_percent("5h", 99.0),
            Window::from_percent("7d", 40.0).resetting_at(Some(Utc::now())),
        ]);
        assert_eq!(usage.availability(), Availability::Now);
        assert_eq!(usage_of(Vec::new()).availability(), Availability::Now);
    }

    #[test]
    fn an_exhausted_window_without_a_reset_is_not_usable_now() {
        let usage = usage_of(vec![spent("7d", None)]);
        assert_eq!(
            usage.availability(),
            Availability::Unknown,
            "a spent window with no reset time must not read as usable now"
        );

        let partly_known = usage_of(vec![
            spent("5h", Some(Duration::hours(1))),
            spent("7d", None),
        ]);
        assert_eq!(
            partly_known.availability(),
            Availability::Unknown,
            "one known reset does not make the account usable while another is unknown"
        );
    }

    #[test]
    fn an_exhausted_account_is_available_when_its_last_spent_window_resets() {
        let soon = Utc::now() + Duration::hours(1);
        let later = Utc::now() + Duration::days(3);
        let usage = usage_of(vec![
            Window::from_percent("5h", 100.0).resetting_at(Some(soon)),
            Window::from_percent("7d", 100.0).resetting_at(Some(later)),
            Window::from_percent("Opus", 20.0).resetting_at(Some(later + Duration::days(1))),
        ]);
        assert_eq!(usage.availability(), Availability::At(later));
    }

    #[test]
    fn availability_orders_now_then_by_time_then_unknown() {
        let soon = Utc::now();
        let later = soon + Duration::hours(1);
        let mut order = vec![
            Availability::Unknown,
            Availability::At(later),
            Availability::Now,
            Availability::At(soon),
        ];
        order.sort();
        assert_eq!(
            order,
            [
                Availability::Now,
                Availability::At(soon),
                Availability::At(later),
                Availability::Unknown,
            ]
        );
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
    fn optional_providers_name_their_feature() {
        assert_eq!(Provider::Copilot.feature(), Some("copilot"));
        assert_eq!(Provider::Cursor.feature(), Some("cursor"));
        assert_eq!(Provider::GrokBot.feature(), Some("grokbot"));
        for provider in [
            Provider::Claude,
            Provider::Codex,
            Provider::Gemini,
            Provider::Grok,
        ] {
            assert_eq!(provider.feature(), None, "{provider} is always built");
        }
    }

    #[test]
    fn a_provider_is_built_exactly_when_its_feature_is_enabled() {
        assert_eq!(Provider::Copilot.is_built(), cfg!(feature = "copilot"));
        assert_eq!(Provider::Cursor.is_built(), cfg!(feature = "cursor"));
        assert_eq!(Provider::GrokBot.is_built(), cfg!(feature = "grokbot"));
        for provider in Provider::ALL
            .into_iter()
            .filter(|provider| provider.feature().is_none())
        {
            assert!(provider.is_built(), "{provider} needs no feature");
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
