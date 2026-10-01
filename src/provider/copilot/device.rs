use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::model::{Account, Provider};
use crate::provider::Discovered;
use crate::provider::copilot::USER_AGENT;
use crate::store::Credential;

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const CLIENT_ID: &str = "Ov23ctr1Udn5GokVCVJf";

#[derive(Debug, Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default = "default_interval")]
    interval: u64,
}

fn default_interval() -> u64 {
    5
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    error: Option<String>,
}

pub async fn login(http: &reqwest::Client) -> Result<Discovered> {
    let device: DeviceCode = http
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .json(&serde_json::json!({ "client_id": CLIENT_ID, "scope": "read:user" }))
        .send()
        .await
        .context("requesting GitHub device code")?
        .json()
        .await
        .context("parsing GitHub device code")?;

    println!(
        "  Open {} and enter code: {}",
        device.verification_uri, device.user_code
    );
    let _ = webbrowser::open(&device.verification_uri);

    let token = poll_for_token(http, &device).await?;
    let login = current_login(http, &token).await?;

    Ok(Discovered {
        account: Account::new(Provider::Copilot, login, None),
        credential: Credential::bearer(token),
    })
}

async fn poll_for_token(http: &reqwest::Client, device: &DeviceCode) -> Result<String> {
    let mut interval = Duration::from_secs(device.interval);
    loop {
        tokio::time::sleep(interval).await;
        let response: TokenResponse = http
            .post(ACCESS_TOKEN_URL)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": device.device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .context("polling GitHub for authorization")?
            .json()
            .await
            .context("parsing GitHub token response")?;

        if let Some(token) = response.access_token {
            return Ok(token);
        }
        match response.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += Duration::from_secs(5),
            Some("expired_token") => bail!("the device code expired before you authorized it"),
            Some("access_denied") => bail!("authorization was denied"),
            Some(other) => bail!("GitHub returned '{other}'"),
            None => bail!("GitHub returned no token and no error"),
        }
    }
}

async fn current_login(http: &reqwest::Client, token: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct User {
        login: String,
    }

    let user: User = http
        .get("https://api.github.com/user")
        .header("Authorization", format!("token {token}"))
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/json")
        .send()
        .await
        .context("identifying the authorized GitHub user")?
        .json()
        .await
        .context("parsing the GitHub user response")?;
    Ok(user.login)
}
