use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Account, AccountId, Provider};

const KEYRING_SERVICE: &str = "aiusg";
const ACCOUNTS_FILE: &str = "accounts.json";
const CREDENTIALS_FILE: &str = "credentials.json";
const KEYCHAIN_ENV: &str = "AIUSG_KEYCHAIN";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl Credential {
    pub fn bearer(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            ..Default::default()
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.extra.insert(key.to_owned(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.extra.get(key).map(String::as_str)
    }

    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|at| at <= Utc::now())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    File,
    Keychain,
}

impl Backend {
    fn from_env() -> Self {
        match std::env::var(KEYCHAIN_ENV).as_deref() {
            Ok("1") | Ok("true") => Backend::Keychain,
            _ => Backend::File,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    accounts: Vec<Account>,
}

#[derive(Clone, Debug)]
pub struct Store {
    directory: PathBuf,
    backend: Backend,
    guard: Arc<Mutex<()>>,
}

impl Store {
    pub fn open() -> Result<Self> {
        let directory = match std::env::var_os("AIUSG_HOME") {
            Some(path) => PathBuf::from(path),
            None => default_directory()?,
        };
        if !directory.exists()
            && let Some(previous) = legacy_directory().filter(|path| path.exists())
        {
            if let Some(parent) = directory.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating {}", parent.display()))?;
            }
            fs::rename(&previous, &directory).with_context(|| {
                format!("moving {} to {}", previous.display(), directory.display())
            })?;
        }
        fs::create_dir_all(&directory)
            .with_context(|| format!("creating {}", directory.display()))?;
        restrict(&directory)?;

        Ok(Self {
            directory,
            backend: Backend::from_env(),
            guard: Arc::new(Mutex::new(())),
        })
    }

    fn manifest_path(&self) -> PathBuf {
        self.directory.join(ACCOUNTS_FILE)
    }

    fn credentials_path(&self) -> PathBuf {
        self.directory.join(CREDENTIALS_FILE)
    }

    fn read_manifest(&self) -> Result<Manifest> {
        read_json(&self.manifest_path())
    }

    fn write_manifest(&self, manifest: &Manifest) -> Result<()> {
        write_json(&self.manifest_path(), manifest)
    }

    pub fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.read_manifest()?.accounts)
    }

    pub fn accounts_for(&self, provider: Option<Provider>) -> Result<Vec<Account>> {
        let accounts = self.accounts()?;
        Ok(match provider {
            Some(provider) => accounts
                .into_iter()
                .filter(|account| account.provider == provider)
                .collect(),
            None => accounts,
        })
    }

    pub fn save(&self, account: &Account, credential: &Credential) -> Result<()> {
        let mut manifest = self.read_manifest()?;
        manifest
            .accounts
            .retain(|existing| existing.id != account.id);
        manifest.accounts.push(account.clone());
        manifest.accounts.sort_by(|left, right| {
            (left.provider, &left.label).cmp(&(right.provider, &right.label))
        });
        self.write_manifest(&manifest)?;
        self.write_credential(&account.id, credential)
    }

    pub fn remove(&self, id: &AccountId) -> Result<bool> {
        let mut manifest = self.read_manifest()?;
        let before = manifest.accounts.len();
        manifest.accounts.retain(|existing| &existing.id != id);
        let removed = manifest.accounts.len() != before;
        if removed {
            self.write_manifest(&manifest)?;
            let _ = self.forget_credential(id);
        }
        Ok(removed)
    }

    pub fn credential(&self, id: &AccountId) -> Result<Credential> {
        match self.backend {
            Backend::Keychain => {
                let raw = entry(id)?
                    .get_password()
                    .with_context(|| format!("no stored credential for {id}"))?;
                serde_json::from_str(&raw)
                    .with_context(|| format!("parsing the credential for {id}"))
            }
            Backend::File => {
                let _lock = self.guard.lock().unwrap_or_else(|error| error.into_inner());
                let all: BTreeMap<String, Credential> = read_json(&self.credentials_path())?;
                all.get(id.as_str())
                    .cloned()
                    .with_context(|| format!("no stored credential for {id}"))
            }
        }
    }

    pub fn write_credential(&self, id: &AccountId, credential: &Credential) -> Result<()> {
        match self.backend {
            Backend::Keychain => {
                let raw = serde_json::to_string(credential)?;
                entry(id)?
                    .set_password(&raw)
                    .with_context(|| format!("storing the credential for {id}"))
            }
            Backend::File => {
                let _lock = self.guard.lock().unwrap_or_else(|error| error.into_inner());
                let path = self.credentials_path();
                let mut all: BTreeMap<String, Credential> = read_json(&path)?;
                all.insert(id.to_string(), credential.clone());
                write_json(&path, &all)
            }
        }
    }

    fn forget_credential(&self, id: &AccountId) -> Result<()> {
        match self.backend {
            Backend::Keychain => entry(id)?.delete_credential().map_err(Into::into),
            Backend::File => {
                let _lock = self.guard.lock().unwrap_or_else(|error| error.into_inner());
                let path = self.credentials_path();
                let mut all: BTreeMap<String, Credential> = read_json(&path)?;
                all.remove(id.as_str());
                write_json(&path, &all)
            }
        }
    }

    pub async fn credential_async(&self, id: &AccountId) -> Result<Credential> {
        let store = self.clone();
        let id = id.clone();
        tokio::task::spawn_blocking(move || store.credential(&id))
            .await
            .context("reading a credential")?
    }

    pub async fn write_credential_async(
        &self,
        id: &AccountId,
        credential: &Credential,
    ) -> Result<()> {
        let store = self.clone();
        let id = id.clone();
        let credential = credential.clone();
        tokio::task::spawn_blocking(move || store.write_credential(&id, &credential))
            .await
            .context("storing a credential")?
    }
}

#[cfg(unix)]
fn default_directory() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => dirs::home_dir()
            .context("could not determine a home directory")?
            .join(".config"),
    };
    Ok(base.join("aiusg"))
}

#[cfg(not(unix))]
fn default_directory() -> Result<PathBuf> {
    Ok(dirs::config_dir()
        .context("could not determine a config directory")?
        .join("aiusg"))
}

#[cfg(target_os = "macos")]
fn legacy_directory() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("aiusg"))
}

#[cfg(not(target_os = "macos"))]
fn legacy_directory() -> Option<PathBuf> {
    None
}

fn read_json<T: Default + serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let raw = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format!("{body}\n"))
        .with_context(|| format!("writing {}", temporary.display()))?;
    restrict(&temporary)?;
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

fn entry(id: &AccountId) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, id.as_str())
        .with_context(|| format!("opening the keyring entry for {id}"))
}

#[cfg(unix)]
fn restrict(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("restricting {}", path.display()))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<()> {
    Ok(())
}
