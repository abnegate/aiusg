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

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::model::{Account, Provider, Window};
use crate::store::Credential;

pub use unsupported::Unsupported;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct SignedOut(pub String);

pub async fn read_json<T: DeserializeOwned>(response: reqwest::Response, what: &str) -> Result<T> {
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

pub fn nullable<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Debug, Default)]
pub struct Fetched {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
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

#[cfg(all(test, not(feature = "copilot")))]
mod tests {
    use super::*;

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
