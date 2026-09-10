pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod gemini;
pub mod grok;
pub mod grokbot;

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;

use crate::model::{Account, Provider, Window};
use crate::store::Credential;

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
        Provider::Copilot => copilot::fetch(http, credential).await,
        Provider::Grok => grok::fetch(http, credential).await,
        Provider::GrokBot => grokbot::fetch(http, credential).await,
        Provider::Cursor => cursor::fetch(http, credential).await,
    }
}

pub async fn login(provider: Provider, http: &reqwest::Client) -> Result<Discovered> {
    match provider {
        Provider::Claude => claude::login(http).await,
        Provider::Codex => codex::login(http).await,
        Provider::Gemini => gemini::login(http).await,
        Provider::Copilot => copilot::login(http).await,
        Provider::Grok => grok::login(http).await,
        Provider::GrokBot => grokbot::login(http).await,
        Provider::Cursor => cursor::login(http).await,
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
        Provider::Copilot => Ok(None),
        Provider::Grok => grok::refresh(http, credential).await,
        Provider::GrokBot => grokbot::refresh(http, credential).await,
        Provider::Cursor => cursor::refresh(http, credential).await,
    }
}

pub fn discover(provider: Provider) -> Result<Vec<Discovered>> {
    match provider {
        Provider::Claude => claude::discover(),
        Provider::Codex => codex::discover(),
        Provider::Gemini => gemini::discover(),
        Provider::Copilot => copilot::discover(),
        Provider::Grok => grok::discover(),
        Provider::GrokBot => grokbot::discover(),
        Provider::Cursor => cursor::discover(),
    }
}
