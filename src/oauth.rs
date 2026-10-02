#[cfg(feature = "login")]
mod loopback;
#[cfg(feature = "login")]
mod pkce;

use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
#[cfg(feature = "login")]
use rand::Rng;

#[cfg(feature = "login")]
pub use loopback::Loopback;
#[cfg(feature = "login")]
pub use pkce::Pkce;

#[cfg(feature = "login")]
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
