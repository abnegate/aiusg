#[cfg(feature = "login")]
mod loopback;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use sha2::{Digest, Sha256};

#[cfg(feature = "login")]
pub use loopback::Loopback;

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

pub fn decode_jwt_claims(token: &str) -> Result<serde_json::Value> {
    let payload = token.split('.').nth(1).context("token is not a JWT")?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .context("decoding the JWT payload")?;
    serde_json::from_slice(&decoded).context("parsing the JWT payload")
}

#[cfg(feature = "login")]
pub fn prompt_open(url: &str) {
    println!("  Opening your browser to authorize.");
    println!("  If it does not open, visit:\n  {url}\n");
    let _ = webbrowser::open(url);
}
