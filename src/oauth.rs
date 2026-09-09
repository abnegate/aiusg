use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n\
<!doctype html><meta charset=utf-8><title>aiusg</title>\
<body style=\"font:15px system-ui;display:grid;place-items:center;height:100vh;margin:0\">\
<div style=\"text-align:center\"><h2>Signed in</h2><p>You can close this tab and return to your terminal.</p></div>";

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        let verifier = random_token(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

pub fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(&buffer)
}

pub struct Loopback {
    listener: TcpListener,
    port: u16,
}

impl Loopback {
    pub async fn bind(ports: &[u16]) -> Result<Self> {
        for &port in ports {
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                return Ok(Self { listener, port });
            }
        }
        bail!("could not bind any of the callback ports {ports:?}");
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub async fn wait(self) -> Result<HashMap<String, String>> {
        let (mut stream, _) = self
            .listener
            .accept()
            .await
            .context("waiting for the OAuth callback")?;

        let mut buffer = [0_u8; 8192];
        let read = stream
            .read(&mut buffer)
            .await
            .context("reading the callback request")?;
        let request = String::from_utf8_lossy(&buffer[..read]);

        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .context("malformed callback request")?;

        let _ = stream.write_all(RESPONSE.as_bytes()).await;
        let _ = stream.shutdown().await;

        let url = url::Url::parse(&format!("http://localhost{target}"))
            .context("parsing the callback URL")?;
        let params: HashMap<String, String> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();

        if let Some(error) = params.get("error") {
            let description = params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("no description");
            bail!("authorization failed: {error} ({description})");
        }
        Ok(params)
    }
}

pub fn decode_jwt_claims(token: &str) -> Result<serde_json::Value> {
    let payload = token.split('.').nth(1).context("token is not a JWT")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decoding the JWT payload")?;
    serde_json::from_slice(&decoded).context("parsing the JWT payload")
}

pub fn prompt_open(url: &str) {
    println!("  Opening your browser to authorize.");
    println!("  If it does not open, visit:\n  {url}\n");
    let _ = webbrowser::open(url);
}
