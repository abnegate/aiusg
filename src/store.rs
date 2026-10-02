//! The accounts aiusg tracks and their credentials, kept on disk or in the
//! keychain.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[cfg(not(feature = "keychain"))]
use anyhow::bail;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{Account, AccountId, Provider};

#[cfg(feature = "keychain")]
const KEYRING_SERVICE: &str = "aiusg";
const ACCOUNTS_FILE: &str = "accounts.json";
const CREDENTIALS_FILE: &str = "credentials.json";
const KEYCHAIN_ENV: &str = "AIUSG_KEYCHAIN";
const HOME_ENV: &str = "AIUSG_HOME";

/// What a provider needs to read an account's usage.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Credential {
    /// The token sent with each request.
    pub access_token: String,
    /// The token that renews `access_token`, when the provider issues one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// When `access_token` stops working, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Provider-specific values, such as an account or team id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, String>,
}

impl Credential {
    /// A credential holding only `access_token`.
    pub fn bearer(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            ..Default::default()
        }
    }

    /// This credential with the extra value `key` set to `value`.
    pub fn with(mut self, key: &str, value: impl Into<String>) -> Self {
        self.extra.insert(key.to_owned(), value.into());
        self
    }

    /// The extra value stored under `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.extra.get(key).map(String::as_str)
    }

    /// Whether `expires_at` has passed.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|at| at <= Utc::now())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    File,
    #[cfg(feature = "keychain")]
    Keychain,
}

impl Backend {
    fn from_setting(setting: Option<&str>) -> Result<Self> {
        match setting {
            Some("1" | "true") => Self::keychain(),
            _ => Ok(Backend::File),
        }
    }

    #[cfg(feature = "keychain")]
    fn keychain() -> Result<Self> {
        Ok(Backend::Keychain)
    }

    #[cfg(not(feature = "keychain"))]
    fn keychain() -> Result<Self> {
        bail!("keychain storage is not in this build; enable the `keychain` feature")
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    accounts: Vec<Account>,
}

/// The accounts aiusg tracks and their credentials.
///
/// It lives in `AIUSG_HOME`, by default `aiusg` under `XDG_CONFIG_HOME` or
/// `~/.config` on Unix, and under the platform configuration directory
/// elsewhere. Credentials go in the keychain
/// when `AIUSG_KEYCHAIN` is `1` or `true`, otherwise in a file only the user
/// can read.
#[derive(Clone, Debug)]
pub struct Store {
    directory: PathBuf,
    backend: Backend,
    guard: Arc<Mutex<()>>,
}

impl Store {
    /// Opens the store, creating its directory when it does not exist.
    pub fn open() -> Result<Self> {
        Self::open_with(
            std::env::var_os(HOME_ENV),
            std::env::var(KEYCHAIN_ENV).ok().as_deref(),
            legacy_directory(),
        )
    }

    fn open_with(
        home: Option<OsString>,
        keychain: Option<&str>,
        legacy: Option<PathBuf>,
    ) -> Result<Self> {
        let backend = Backend::from_setting(keychain)?;
        let home = home.filter(|path| !path.is_empty());
        let legacy = legacy.filter(|path| home.is_none() && path.exists());
        let directory = directory(home)?;
        if !directory.exists()
            && let Some(previous) = legacy
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
            backend,
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

    /// Every tracked account, ordered by provider then label.
    pub fn accounts(&self) -> Result<Vec<Account>> {
        Ok(self.read_manifest()?.accounts)
    }

    /// The tracked accounts of `provider`, or every account when it is `None`.
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

    /// Tracks `account` with `credential`, replacing any account with its id.
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

    /// Stops tracking the account `id` and forgets its credential, returning
    /// whether it was tracked.
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

    /// The stored credential of the account `id`.
    pub fn credential(&self, id: &AccountId) -> Result<Credential> {
        match self.backend {
            #[cfg(feature = "keychain")]
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

    /// Stores `credential` for the account `id`, replacing any it had.
    pub fn write_credential(&self, id: &AccountId, credential: &Credential) -> Result<()> {
        match self.backend {
            #[cfg(feature = "keychain")]
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
            #[cfg(feature = "keychain")]
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

    /// [`credential`](Self::credential) off the async runtime's threads.
    pub async fn credential_async(&self, id: &AccountId) -> Result<Credential> {
        let store = self.clone();
        let id = id.clone();
        tokio::task::spawn_blocking(move || store.credential(&id))
            .await
            .context("reading a credential")?
    }

    /// [`write_credential`](Self::write_credential) off the async runtime's
    /// threads.
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

fn directory(home: Option<OsString>) -> Result<PathBuf> {
    match home.filter(|path| !path.is_empty()) {
        Some(path) => Ok(PathBuf::from(path)),
        None => default_directory(),
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

#[cfg(feature = "keychain")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_kept_in_a_file_unless_the_keychain_is_asked_for() {
        for setting in [None, Some(""), Some("0"), Some("false"), Some("yes")] {
            assert_eq!(
                Backend::from_setting(setting).unwrap(),
                Backend::File,
                "{setting:?} does not ask for the keychain"
            );
        }
    }

    #[cfg(feature = "keychain")]
    #[test]
    fn asking_for_the_keychain_uses_it() {
        for setting in ["1", "true"] {
            assert_eq!(
                Backend::from_setting(Some(setting)).unwrap(),
                Backend::Keychain
            );
        }
    }

    #[test]
    fn aiusg_home_names_the_store_directory() {
        assert_eq!(
            directory(Some(OsString::from("/srv/aiusg"))).unwrap(),
            PathBuf::from("/srv/aiusg")
        );
    }

    #[test]
    fn an_empty_aiusg_home_counts_as_unset() {
        assert_eq!(
            directory(Some(OsString::new())).unwrap(),
            default_directory().unwrap(),
            "an empty value must not resolve to the working directory"
        );
        assert_eq!(directory(None).unwrap(), default_directory().unwrap());
    }

    #[test]
    fn a_store_opens_in_the_directory_it_is_given() {
        let home = tempfile::tempdir().unwrap();
        let store = Store::open_with(Some(home.path().into()), None, None).unwrap();

        assert_eq!(store.directory, home.path());
        assert_eq!(store.backend, Backend::File);
        assert!(store.accounts().unwrap().is_empty());
    }

    #[test]
    fn an_explicit_aiusg_home_leaves_the_legacy_store_in_place() {
        let root = tempfile::tempdir().unwrap();
        let legacy = root.path().join("legacy");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join(ACCOUNTS_FILE), "{}").unwrap();
        let home = root.path().join("home");

        let store =
            Store::open_with(Some(home.clone().into()), None, Some(legacy.clone())).unwrap();

        assert_eq!(store.directory, home);
        assert!(
            legacy.join(ACCOUNTS_FILE).exists(),
            "an explicit AIUSG_HOME must not move the existing store"
        );
        assert!(
            !home.join(ACCOUNTS_FILE).exists(),
            "the new home starts empty"
        );
    }

    #[cfg(not(feature = "keychain"))]
    #[test]
    fn a_refused_keychain_leaves_the_filesystem_untouched() {
        let home = tempfile::tempdir().unwrap();
        let directory = home.path().join("aiusg");

        Store::open_with(Some(directory.clone().into()), Some("1"), None)
            .expect_err("plaintext storage must not stand in for the keychain");

        assert!(
            !directory.exists(),
            "the store directory must not be created before the backend is decided"
        );
    }

    #[cfg(not(feature = "keychain"))]
    #[test]
    fn asking_for_the_keychain_without_the_feature_is_refused() {
        for setting in ["1", "true"] {
            let error = Backend::from_setting(Some(setting))
                .expect_err("plaintext storage must not stand in for the keychain");
            assert_eq!(
                error.to_string(),
                "keychain storage is not in this build; enable the `keychain` feature"
            );
        }
    }
}
