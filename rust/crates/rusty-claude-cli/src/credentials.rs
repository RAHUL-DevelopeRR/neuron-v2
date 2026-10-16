//! Zero-X credential storage.
//!
//! The login (a `ses_` session token or an `nzx_` API key, plus its expiry
//! and plan) is kept in the OS credential store on macOS and Windows. On
//! Linux, when the keychain is unavailable, or when the Neuron home is
//! overridden (`NEURON_HOME`, a non-default `HOME`), it is written to
//! `~/.neuroncli/credentials.json` with owner-only permissions and a
//! one-time warning. `NEURON_CREDENTIAL_STORE=file|keychain` forces a
//! backend.
//!
//! Reading never fails hard: a corrupt or foreign entry produces a warning
//! and is treated as "signed out". An existing legacy `gateway.enc` is
//! migrated once into the current backend and then deleted.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::vault::{self, SecureString};

/// Keychain service name for the Zero-X login.
pub const KEYRING_SERVICE: &str = "neuron-cli";
/// Keychain account name for the Zero-X login.
pub const KEYRING_ACCOUNT: &str = "zero-x";
const CREDENTIALS_FILE: &str = "credentials.json";
const LEGACY_VAULT_FILE: &str = "gateway.enc";
const RECORD_VERSION: u32 = 1;

/// `~/.neuroncli`, or `NEURON_HOME` when set. `HOME`/`USERPROFILE` are
/// honoured before the OS profile lookup so a test (or a portable install)
/// can isolate every file Neuron writes.
pub fn neuron_home() -> PathBuf {
    non_empty_env("NEURON_HOME").map_or_else(|| user_home().join(".neuroncli"), PathBuf::from)
}

fn user_home() -> PathBuf {
    non_empty_env("HOME")
        .or_else(|| non_empty_env("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Whether the Neuron home differs from the OS user profile. An overridden
/// home keeps credentials inside it rather than in the shared OS keychain.
fn home_is_overridden() -> bool {
    if non_empty_env("NEURON_HOME").is_some() {
        return true;
    }
    dirs::home_dir().is_none_or(|profile| !same_path(&user_home(), &profile))
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string()
    };
    if cfg!(windows) {
        normalize(left).eq_ignore_ascii_case(&normalize(right))
    } else {
        normalize(left) == normalize(right)
    }
}

/// Kind of Zero-X credential, decided by its prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// `ses_…` CLI session from the browser login.
    Session,
    /// `nzx_…` personal API key for headless and CI runs.
    ApiKey,
}

/// Validates the shape of a Zero-X credential without contacting the server.
pub fn classify_token(token: &str) -> Result<TokenKind, String> {
    let (kind, min_len) = if token.starts_with("ses_") {
        (TokenKind::Session, 24)
    } else if token.starts_with("nzx_") {
        (TokenKind::ApiKey, 20)
    } else {
        return Err("expected a Zero-X session token (ses_...) or API key (nzx_...)".to_string());
    };
    if !(min_len..=512).contains(&token.len())
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err("malformed Zero-X credential".to_string());
    }
    Ok(kind)
}

/// A stored Zero-X login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCredential {
    pub token: SecureString,
    /// Unix seconds after which the server no longer accepts the token.
    pub expires_at: Option<u64>,
    pub plan: Option<String>,
    pub saved_at: u64,
}

impl StoredCredential {
    pub fn new(token: SecureString) -> Self {
        Self {
            token,
            expires_at: None,
            plan: None,
            saved_at: crate::clock::now_unix(),
        }
    }

    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }

    fn to_json(&self) -> Zeroizing<String> {
        let record = CredentialRecord {
            version: RECORD_VERSION,
            token: self.token.expose().to_string(),
            expires_at: self.expires_at,
            plan: self.plan.clone(),
            saved_at: self.saved_at,
        };
        Zeroizing::new(serde_json::to_string_pretty(&record).expect("record serializes"))
    }

    fn from_json(text: &str) -> Result<Self, String> {
        let mut record = serde_json::from_str::<CredentialRecord>(text)
            .map_err(|error| format!("not a Neuron credential record ({error})"))?;
        if record.version != RECORD_VERSION {
            return Err(format!(
                "unsupported credential record version {}",
                record.version
            ));
        }
        classify_token(&record.token)?;
        Ok(Self {
            token: SecureString::new(std::mem::take(&mut record.token)),
            expires_at: record.expires_at,
            plan: record.plan.take(),
            saved_at: record.saved_at,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct CredentialRecord {
    version: u32,
    token: String,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    saved_at: u64,
}

impl Drop for CredentialRecord {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}

/// Why a backend operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// No usable store on this machine (no keychain service, locked, ...).
    Unavailable(String),
    Failed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

/// A place that can hold one secret string.
pub trait SecretBackend: Send + Sync {
    fn read(&self) -> Result<Option<Zeroizing<String>>, BackendError>;
    fn write(&self, secret: &str) -> Result<(), BackendError>;
    fn delete(&self) -> Result<(), BackendError>;
}

/// Windows Credential Manager / macOS Keychain via the `keyring` crate.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct KeychainBackend {
    service: String,
    account: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl KeychainBackend {
    pub fn new(service: &str, account: &str) -> Self {
        Self {
            service: service.to_string(),
            account: account.to_string(),
        }
    }

    fn entry(&self) -> Result<keyring::Entry, BackendError> {
        keyring::Entry::new(&self.service, &self.account).map_err(keychain_error)
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn keychain_error(error: keyring::Error) -> BackendError {
    match error {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            BackendError::Unavailable(error.to_string())
        }
        other => BackendError::Failed(other.to_string()),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl SecretBackend for KeychainBackend {
    fn read(&self) -> Result<Option<Zeroizing<String>>, BackendError> {
        match self.entry()?.get_password() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keychain_error(error)),
        }
    }

    fn write(&self, secret: &str) -> Result<(), BackendError> {
        self.entry()?.set_password(secret).map_err(keychain_error)
    }

    fn delete(&self) -> Result<(), BackendError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keychain_error(error)),
        }
    }
}

/// `credentials.json`, written atomically with owner-only permissions.
pub struct FileBackend {
    path: PathBuf,
}

impl FileBackend {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SecretBackend for FileBackend {
    fn read(&self) -> Result<Option<Zeroizing<String>>, BackendError> {
        match fs::read_to_string(&self.path) {
            Ok(text) => Ok(Some(Zeroizing::new(text))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(BackendError::Failed(format!(
                "cannot read {}: {error}",
                self.path.display()
            ))),
        }
    }

    fn write(&self, secret: &str) -> Result<(), BackendError> {
        let failed = |error: io::Error| {
            BackendError::Failed(format!("cannot write {}: {error}", self.path.display()))
        };
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        // A stale temporary file could carry broader permissions; start fresh.
        let _ = fs::remove_file(&temporary);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(failed)?;
        file.write_all(secret.as_bytes()).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        drop(file);
        fs::rename(&temporary, &self.path).map_err(failed)
    }

    fn delete(&self) -> Result<(), BackendError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(BackendError::Failed(format!(
                "cannot remove {}: {error}",
                self.path.display()
            ))),
        }
    }
}

/// Where a credential was found or stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialLocation {
    Keychain,
    File,
    /// The retired self-decrypting `gateway.enc`.
    Legacy,
}

impl CredentialLocation {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Keychain => "OS keychain",
            Self::File => "credentials file",
            Self::Legacy => "legacy gateway.enc",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCredential {
    pub credential: StoredCredential,
    pub location: CredentialLocation,
}

/// Result of a read-only lookup, with the warnings it produced.
#[derive(Debug, Default)]
pub struct LoadReport {
    pub credential: Option<LoadedCredential>,
    pub warnings: Vec<String>,
}

/// The Zero-X login store (keychain or file, plus the legacy vault).
pub struct CredentialStore {
    keychain: Option<Box<dyn SecretBackend>>,
    file: FileBackend,
    legacy_path: PathBuf,
}

impl CredentialStore {
    /// Store rooted at [`neuron_home`], honouring `NEURON_CREDENTIAL_STORE`.
    pub fn open_default() -> Self {
        let preference =
            non_empty_env("NEURON_CREDENTIAL_STORE").map(|value| value.trim().to_ascii_lowercase());
        let keychain = match preference.as_deref() {
            Some("file") => None,
            Some("keychain") => platform_keychain(),
            _ if home_is_overridden() => None,
            _ => platform_keychain(),
        };
        Self::new(&neuron_home(), keychain)
    }

    pub fn new(dir: &Path, keychain: Option<Box<dyn SecretBackend>>) -> Self {
        Self {
            keychain,
            file: FileBackend::new(dir.join(CREDENTIALS_FILE)),
            legacy_path: dir.join(LEGACY_VAULT_FILE),
        }
    }

    pub fn file_path(&self) -> &Path {
        self.file.path()
    }

    /// Read-only lookup: keychain, then file, then the legacy vault. Never
    /// writes and never fails; problems become warnings.
    pub fn inspect(&self) -> LoadReport {
        let mut report = LoadReport::default();
        if let Some(keychain) = &self.keychain {
            match keychain.read() {
                Ok(Some(text)) => match StoredCredential::from_json(&text) {
                    Ok(credential) => {
                        report.credential = Some(LoadedCredential {
                            credential,
                            location: CredentialLocation::Keychain,
                        });
                        return report;
                    }
                    Err(error) => report.warnings.push(format!(
                        "Ignoring the Zero-X entry in the OS keychain: {error}. Run `neuron auth login` to sign in again."
                    )),
                },
                Ok(None) | Err(BackendError::Unavailable(_)) => {}
                Err(BackendError::Failed(error)) => report
                    .warnings
                    .push(format!("Could not read the OS keychain: {error}")),
            }
        }
        match self.file.read() {
            Ok(Some(text)) => match StoredCredential::from_json(&text) {
                Ok(credential) => {
                    report.credential = Some(LoadedCredential {
                        credential,
                        location: CredentialLocation::File,
                    });
                    return report;
                }
                Err(error) => report.warnings.push(format!(
                    "Ignoring unreadable credential file {}: {error}. Run `neuron auth login` to sign in again, or `neuron auth logout` to remove it.",
                    self.file.path().display()
                )),
            },
            Ok(None) => {}
            Err(error) => report.warnings.push(error.to_string()),
        }
        if self.legacy_path.exists() {
            match vault::decrypt_legacy_vault(&self.legacy_path).and_then(|token| {
                classify_token(token.expose()).map(|_| token)
            }) {
                Ok(token) => {
                    report.credential = Some(LoadedCredential {
                        credential: StoredCredential::new(token),
                        location: CredentialLocation::Legacy,
                    });
                }
                Err(error) => report.warnings.push(format!(
                    "Ignoring unreadable legacy credential file {}: {error}. Run `neuron auth login` to sign in again, or `neuron auth logout` to remove it.",
                    self.legacy_path.display()
                )),
            }
        }
        report
    }

    /// [`CredentialStore::inspect`], printing its warnings once per process.
    pub fn load(&self) -> Option<LoadedCredential> {
        let report = self.inspect();
        for warning in &report.warnings {
            warn_once(warning);
        }
        report.credential
    }

    /// Like [`CredentialStore::load`], but moves a legacy `gateway.enc`
    /// into the current backend (once) and deletes the old file.
    pub fn load_and_migrate(&self) -> Option<LoadedCredential> {
        let loaded = self.load()?;
        if loaded.location != CredentialLocation::Legacy {
            return Some(loaded);
        }
        match self.save(&loaded.credential) {
            Ok(location) => {
                eprintln!(
                    "Moved your Zero-X login from {} to the {}.",
                    self.legacy_path.display(),
                    location.describe()
                );
                Some(LoadedCredential {
                    credential: loaded.credential,
                    location,
                })
            }
            Err(error) => {
                warn_once(&format!(
                    "Could not migrate the legacy Zero-X login: {error}"
                ));
                Some(loaded)
            }
        }
    }

    /// Stores `credential`, preferring the keychain. Removes copies left in
    /// other locations (including the legacy vault).
    pub fn save(&self, credential: &StoredCredential) -> Result<CredentialLocation, String> {
        let record = credential.to_json();
        if let Some(keychain) = &self.keychain {
            match keychain.write(&record) {
                Ok(()) => {
                    let _ = self.file.delete();
                    self.remove_legacy();
                    return Ok(CredentialLocation::Keychain);
                }
                Err(error) => warn_once(&format!(
                    "The OS keychain is unavailable ({error}); storing the Zero-X login in a file instead."
                )),
            }
        }
        let existed = self.file.path().exists();
        self.file
            .write(&record)
            .map_err(|error| error.to_string())?;
        if !existed {
            warn_once(&format!(
                "Zero-X login stored unencrypted in {} ({}). Remove it with `neuron auth logout`.",
                self.file.path().display(),
                if cfg!(unix) {
                    "permissions 0600, readable only by you"
                } else {
                    "protected by your user profile permissions"
                }
            ));
        }
        self.remove_legacy();
        Ok(CredentialLocation::File)
    }

    /// Removes the login from every location. Never fails; returns warnings
    /// for anything that could not be removed.
    pub fn clear(&self) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(keychain) = &self.keychain {
            if let Err(BackendError::Failed(error)) = keychain.delete() {
                warnings.push(format!("Could not remove the OS keychain entry: {error}"));
            }
        }
        if let Err(error) = self.file.delete() {
            warnings.push(error.to_string());
        }
        if let Err(error) = fs::remove_file(&self.legacy_path) {
            if error.kind() != io::ErrorKind::NotFound {
                warnings.push(format!(
                    "cannot remove {}: {error}",
                    self.legacy_path.display()
                ));
            }
        }
        warnings
    }

    fn remove_legacy(&self) {
        if self.legacy_path.exists() {
            if let Err(error) = fs::remove_file(&self.legacy_path) {
                warn_once(&format!(
                    "Could not delete the legacy credential file {}: {error}",
                    self.legacy_path.display()
                ));
            }
        }
    }
}

fn platform_keychain() -> Option<Box<dyn SecretBackend>> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Some(Box::new(KeychainBackend::new(
            KEYRING_SERVICE,
            KEYRING_ACCOUNT,
        )))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        None
    }
}

/// Prints `warning: <message>` the first time a given message is seen.
pub fn warn_once(message: &str) {
    static SEEN: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let first = SEEN
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(message.to_string());
    if first {
        eprintln!("warning: {message}");
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// In-memory keychain so tests never touch the real OS store.
    #[derive(Clone, Default)]
    pub(crate) struct FakeKeychain {
        pub secret: Arc<Mutex<Option<String>>>,
        pub unavailable: bool,
    }

    impl SecretBackend for FakeKeychain {
        fn read(&self) -> Result<Option<Zeroizing<String>>, BackendError> {
            if self.unavailable {
                return Err(BackendError::Unavailable("no keychain".into()));
            }
            Ok(self.secret.lock().unwrap().clone().map(Zeroizing::new))
        }
        fn write(&self, secret: &str) -> Result<(), BackendError> {
            if self.unavailable {
                return Err(BackendError::Unavailable("no keychain".into()));
            }
            *self.secret.lock().unwrap() = Some(secret.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<(), BackendError> {
            if self.unavailable {
                return Err(BackendError::Unavailable("no keychain".into()));
            }
            *self.secret.lock().unwrap() = None;
            Ok(())
        }
    }

    pub(crate) fn temp_home(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "neuron-credentials-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const TOKEN: &str = "ses_012345678901234567890123456789";

    #[test]
    fn token_classification_accepts_sessions_and_api_keys_only() {
        assert_eq!(classify_token(TOKEN), Ok(TokenKind::Session));
        assert_eq!(
            classify_token("nzx_0123456789abcdefghij"),
            Ok(TokenKind::ApiKey)
        );
        assert!(classify_token("sk-provider-secret-0123456789").is_err());
        assert!(classify_token("ses_short").is_err());
        assert!(classify_token("ses_0123456789012345678901234567 89").is_err());
    }

    #[test]
    fn legacy_vault_is_migrated_once_into_the_keychain_and_deleted() {
        let home = temp_home("migrate-keychain");
        let legacy = home.join(LEGACY_VAULT_FILE);
        vault::write_legacy_vault_for_test(TOKEN, &legacy, "legacy-fingerprint");
        let keychain = FakeKeychain::default();
        let store = CredentialStore::new(&home, Some(Box::new(keychain.clone())));

        // Read-only lookups (doctor) see the legacy login but change nothing.
        let peek = store.inspect();
        assert_eq!(
            peek.credential.as_ref().map(|loaded| loaded.location),
            Some(CredentialLocation::Legacy)
        );
        assert!(legacy.exists());
        assert!(keychain.secret.lock().unwrap().is_none());

        let migrated = store.load_and_migrate().expect("migrated credential");
        assert_eq!(migrated.location, CredentialLocation::Keychain);
        assert_eq!(migrated.credential.token.expose(), TOKEN);
        assert!(!legacy.exists(), "legacy vault must be deleted");
        assert!(!store.file_path().exists());
        let record = keychain.secret.lock().unwrap().clone().expect("stored");
        assert!(record.contains("\"version\": 1"));

        let again = store.load_and_migrate().expect("still signed in");
        assert_eq!(again.location, CredentialLocation::Keychain);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn migration_falls_back_to_an_owner_only_file_without_a_keychain() {
        let home = temp_home("migrate-file");
        let legacy = home.join(LEGACY_VAULT_FILE);
        vault::write_legacy_vault_for_test(TOKEN, &legacy, "fp");
        let unavailable = FakeKeychain {
            unavailable: true,
            ..FakeKeychain::default()
        };
        let store = CredentialStore::new(&home, Some(Box::new(unavailable)));

        let migrated = store.load_and_migrate().expect("migrated");
        assert_eq!(migrated.location, CredentialLocation::File);
        assert!(!legacy.exists());
        let text = fs::read_to_string(store.file_path()).unwrap();
        assert!(text.contains(TOKEN));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(store.file_path())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn credentials_roundtrip_with_expiry_and_plan() {
        let home = temp_home("roundtrip");
        let store = CredentialStore::new(&home, None);
        let mut credential = StoredCredential::new(SecureString::new(TOKEN.to_string()));
        credential.expires_at = Some(1_792_195_200);
        credential.plan = Some("pro".to_string());

        assert_eq!(store.save(&credential), Ok(CredentialLocation::File));
        let loaded = store.load().expect("stored credential");
        assert_eq!(loaded.credential, credential);
        assert!(loaded.credential.is_expired(1_792_195_200));
        assert!(!loaded.credential.is_expired(1_792_195_199));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn corrupt_or_foreign_files_warn_and_read_as_signed_out() {
        let home = temp_home("corrupt");
        let store = CredentialStore::new(&home, None);
        for contents in [
            "{not json",
            r#"{"version":9,"token":"ses_012345678901234567890123456789"}"#,
            r#"{"version":1,"token":"sk-ant-someone-elses-provider-key"}"#,
            r#"{"access_token":"foreign tool file"}"#,
        ] {
            fs::write(store.file_path(), contents).unwrap();
            let report = store.inspect();
            assert!(report.credential.is_none(), "{contents} must not load");
            assert_eq!(report.warnings.len(), 1, "{contents}");
            assert!(report.warnings[0].contains("credentials.json"));
        }

        fs::write(home.join(LEGACY_VAULT_FILE), "garbage").unwrap();
        let report = store.inspect();
        assert!(report.credential.is_none());
        assert_eq!(report.warnings.len(), 2);

        assert_eq!(
            store.clear(),
            Vec::<String>::new(),
            "logout always succeeds"
        );
        assert!(!store.file_path().exists());
        assert!(!home.join(LEGACY_VAULT_FILE).exists());
        assert_eq!(store.inspect().warnings, Vec::<String>::new());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn keychain_save_replaces_a_fallback_file_and_clear_removes_both() {
        let home = temp_home("keychain-wins");
        let file_only = CredentialStore::new(&home, None);
        file_only
            .save(&StoredCredential::new(SecureString::new(TOKEN.to_string())))
            .unwrap();
        assert!(file_only.file_path().exists());

        let keychain = FakeKeychain::default();
        let store = CredentialStore::new(&home, Some(Box::new(keychain.clone())));
        store
            .save(&StoredCredential::new(SecureString::new(
                "nzx_0123456789abcdefghij".to_string(),
            )))
            .unwrap();
        assert!(!store.file_path().exists());
        assert_eq!(
            store
                .load()
                .map(|loaded| loaded.credential.token.expose().to_string()),
            Some("nzx_0123456789abcdefghij".to_string())
        );

        assert_eq!(store.clear(), Vec::<String>::new());
        assert!(keychain.secret.lock().unwrap().is_none());
        assert!(store.load().is_none());
        fs::remove_dir_all(home).unwrap();
    }
}
