pub mod claude;
pub mod codex;
#[cfg(feature = "copilot")]
pub mod copilot;
#[cfg(feature = "cursor")]
pub mod cursor;
pub mod gemini;
pub mod grok;
#[cfg(feature = "grokbot")]
pub mod grokbot;
mod unsupported;

use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::model::{Account, Provider, Usage, Window};
use crate::store::Credential;

pub use unsupported::Unsupported;

/// A provider refused the credential with HTTP 401 or 403.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SignedOut(pub String);

/// Whether `error` means the account is signed out: a [`SignedOut`], from a
/// 401 or 403, anywhere in its chain, however much context wraps it.
pub fn is_signed_out(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<SignedOut>())
}

pub(crate) async fn read_json<T: DeserializeOwned>(
    response: reqwest::Response,
    what: &str,
) -> Result<T> {
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("reading the {what} response"))?;

    if std::env::var_os("AIUSG_DEBUG").is_some() {
        eprintln!("[aiusg] {what} -> HTTP {status}\n{body}\n");
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(SignedOut(format!("{what} rejected the stored credential")).into());
    }
    if !status.is_success() {
        bail!("{what} request failed: HTTP {status}");
    }
    serde_json::from_str(&body).with_context(|| format!("parsing the {what} response"))
}

fn endpoint(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

fn home(variable: &str, default: &str) -> Option<PathBuf> {
    named(std::env::var_os(variable)).or_else(|| Some(dirs::home_dir()?.join(default)))
}

fn named(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("parsing {}", path.display()))
}

pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

/// What one fetch read from a provider, before it is tied to an account.
#[derive(Clone, Debug, Default)]
pub struct Fetched {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
}

impl Fetched {
    /// The [`Usage`] of `account`, fetched now.
    pub fn into_usage(self, account: &Account) -> Usage {
        self.into_usage_at(account, Utc::now())
    }

    /// The [`Usage`] of `account`, fetched at `fetched_at`.
    ///
    /// A plan from the fetch replaces the account's stored plan; without one,
    /// the stored plan stays.
    pub fn into_usage_at(self, account: &Account, fetched_at: DateTime<Utc>) -> Usage {
        Usage {
            account: account.id.clone(),
            provider: account.provider,
            label: account.label.clone(),
            plan: self.plan.or_else(|| account.plan.clone()),
            windows: self.windows,
            fetched_at,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Discovered {
    pub account: Account,
    pub credential: Credential,
}

pub async fn fetch(
    provider: Provider,
    http: &reqwest::Client,
    credential: &Credential,
) -> Result<Fetched> {
    match provider {
        Provider::Claude => claude::fetch(http, credential).await,
        Provider::Codex => codex::fetch(http, credential).await,
        Provider::Gemini => gemini::fetch(http, credential).await,
        #[cfg(feature = "copilot")]
        Provider::Copilot => copilot::fetch(http, credential).await,
        Provider::Grok => grok::fetch(http, credential).await,
        #[cfg(feature = "grokbot")]
        Provider::GrokBot => grokbot::fetch(http, credential).await,
        #[cfg(feature = "cursor")]
        Provider::Cursor => cursor::fetch(http, credential).await,
        #[cfg(not(all(feature = "copilot", feature = "cursor", feature = "grokbot")))]
        unbuilt => unsupported(unbuilt),
    }
}

#[cfg(feature = "login")]
pub async fn login(provider: Provider, http: &reqwest::Client) -> Result<Discovered> {
    match provider {
        Provider::Claude => claude::login(http).await,
        Provider::Codex => codex::login(http).await,
        Provider::Gemini => gemini::login(http).await,
        #[cfg(feature = "copilot")]
        Provider::Copilot => copilot::login(http).await,
        Provider::Grok => grok::login(http).await,
        #[cfg(feature = "grokbot")]
        Provider::GrokBot => grokbot::login(http).await,
        #[cfg(feature = "cursor")]
        Provider::Cursor => cursor::login(http).await,
        #[cfg(not(all(feature = "copilot", feature = "cursor", feature = "grokbot")))]
        unbuilt => unsupported(unbuilt),
    }
}

pub async fn refresh(
    provider: Provider,
    http: &reqwest::Client,
    credential: &Credential,
) -> Result<Option<Credential>> {
    match provider {
        Provider::Claude => claude::refresh(http, credential).await,
        Provider::Codex => codex::refresh(http, credential).await,
        Provider::Gemini => gemini::refresh(http, credential).await,
        #[cfg(feature = "copilot")]
        Provider::Copilot => Ok(None),
        Provider::Grok => grok::refresh(http, credential).await,
        #[cfg(feature = "grokbot")]
        Provider::GrokBot => grokbot::refresh(http, credential).await,
        #[cfg(feature = "cursor")]
        Provider::Cursor => cursor::refresh(http, credential).await,
        #[cfg(not(all(feature = "copilot", feature = "cursor", feature = "grokbot")))]
        unbuilt => unsupported(unbuilt),
    }
}

pub fn discover(provider: Provider) -> Result<Vec<Discovered>> {
    match provider {
        Provider::Claude => claude::discover(),
        Provider::Codex => codex::discover(),
        Provider::Gemini => gemini::discover(),
        #[cfg(feature = "copilot")]
        Provider::Copilot => copilot::discover(),
        Provider::Grok => grok::discover(),
        #[cfg(feature = "grokbot")]
        Provider::GrokBot => grokbot::discover(),
        #[cfg(feature = "cursor")]
        Provider::Cursor => cursor::discover(),
        #[cfg(not(all(feature = "copilot", feature = "cursor", feature = "grokbot")))]
        unbuilt => unsupported(unbuilt),
    }
}

#[cfg(not(all(feature = "copilot", feature = "cursor", feature = "grokbot")))]
fn unsupported<T>(provider: Provider) -> Result<T> {
    let feature = provider
        .feature()
        .expect("only an optional provider can be left out of the build");
    Err(Unsupported { provider, feature }.into())
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    #[test]
    fn a_signed_out_error_is_recognised_through_added_context() {
        let error = anyhow::Error::from(SignedOut("Claude usage rejected the token".to_owned()))
            .context("fetching usage")
            .context("refreshing login 3f2b9c1e");

        assert!(
            is_signed_out(&error),
            "context layers must not hide a refused token"
        );
        assert!(
            !is_signed_out(&anyhow!("Claude usage request failed: HTTP 500")),
            "an ordinary failure is not a sign-out"
        );
    }

    #[test]
    fn a_fetch_becomes_usage_for_its_account_and_keeps_the_stored_plan_when_none_came() {
        let account = Account::new(Provider::Codex, "jake@example.com", Some("pro".to_owned()));
        let fetched = Fetched {
            plan: None,
            windows: vec![Window::from_percent("7d", 40.0)],
        };

        let usage = fetched.into_usage(&account);

        assert_eq!(usage.account, account.id);
        assert_eq!(usage.provider, Provider::Codex);
        assert_eq!(usage.label, "jake@example.com");
        assert_eq!(usage.plan.as_deref(), Some("pro"), "the stored plan stays");
        assert_eq!(usage.windows.len(), 1);
        assert_eq!(usage.windows[0].used_percent, Some(40.0));

        let renamed = Fetched {
            plan: Some("plus".to_owned()),
            windows: Vec::new(),
        }
        .into_usage(&account);
        assert_eq!(
            renamed.plan.as_deref(),
            Some("plus"),
            "a plan the fetch reports replaces the stored one"
        );
    }

    #[test]
    fn a_fetch_is_stamped_now_unless_the_caller_names_the_time() {
        let account = Account::new(Provider::Claude, "jake@example.com", None);
        let named = DateTime::parse_from_rfc3339("2026-10-01T04:30:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let stamped = Fetched::default().into_usage_at(&account, named);
        assert_eq!(stamped.fetched_at, named);

        let before = Utc::now();
        let current = Fetched::default().into_usage(&account);
        assert!(
            (before..=Utc::now()).contains(&current.fetched_at),
            "into_usage stamps the time of the call"
        );
    }

    #[test]
    fn an_endpoint_joins_a_base_with_or_without_a_trailing_slash() {
        assert_eq!(
            endpoint("http://127.0.0.1:8080", "/api/oauth/usage"),
            "http://127.0.0.1:8080/api/oauth/usage"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:8080/", "/api/oauth/usage"),
            "http://127.0.0.1:8080/api/oauth/usage"
        );
    }

    #[test]
    fn a_home_variable_names_the_directory_when_it_is_set() {
        assert_eq!(
            named(Some(OsString::from("/srv/codex"))),
            Some(PathBuf::from("/srv/codex"))
        );
    }

    #[test]
    fn an_empty_home_variable_counts_as_unset() {
        assert_eq!(
            named(Some(OsString::new())),
            None,
            "an empty value must not resolve to the working directory"
        );
        assert_eq!(named(None), None);
    }

    #[test]
    fn an_unset_home_variable_falls_back_under_the_user_home() {
        assert_eq!(
            home("AIUSG_TEST_VARIABLE_THAT_IS_NEVER_SET", ".codex"),
            dirs::home_dir().map(|directory| directory.join(".codex"))
        );
    }

    #[test]
    fn a_missing_file_loads_as_nothing_and_a_broken_one_fails() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");

        let missing: Option<serde_json::Value> = load(&path).unwrap();
        assert!(missing.is_none());

        std::fs::write(&path, "{not json").unwrap();
        let broken = load::<serde_json::Value>(&path).unwrap_err();
        assert!(
            format!("{broken:#}").contains("parsing"),
            "the error names what failed: {broken:#}"
        );
    }

    #[test]
    fn a_blank_file_loads_as_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");

        for blank in ["", "  \n\t\n"] {
            std::fs::write(&path, blank).unwrap();
            let loaded: Option<serde_json::Value> = load(&path).unwrap();
            assert!(loaded.is_none(), "{blank:?} holds nothing to parse");
        }
    }

    #[cfg(not(feature = "copilot"))]
    #[tokio::test]
    async fn a_provider_left_out_of_the_build_names_the_feature_that_adds_it() {
        let expected = Unsupported {
            provider: Provider::Copilot,
            feature: "copilot",
        };
        let message = "copilot support is not in this build; enable the `copilot` feature";

        let discovered = discover(Provider::Copilot).expect_err("copilot is not in this build");
        assert_eq!(discovered.downcast_ref::<Unsupported>(), Some(&expected));
        assert_eq!(discovered.to_string(), message);

        let fetched = fetch(
            Provider::Copilot,
            &reqwest::Client::new(),
            &Credential::bearer("token"),
        )
        .await
        .expect_err("copilot is not in this build");
        assert_eq!(fetched.downcast_ref::<Unsupported>(), Some(&expected));
    }
}
