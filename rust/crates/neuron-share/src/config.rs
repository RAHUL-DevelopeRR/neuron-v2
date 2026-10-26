//! The computer's relay identity and paired phones, kept in the Neuron config
//! (`<neuron home>/remote.json`, readable only by the user): the host id, the X25519 secret
//! whose public half the QR code carries, the presence key that seals the sessions list, each
//! paired phone's pair key, and the share id of each shared project (so a phone sees the same
//! session after a restart).

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto::{b64u, random_bytes, random_id};

pub const CONFIG_FILE: &str = "remote.json";

/// The computer's own keys; nothing but the public key ever leaves it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostIdentity {
    pub host_id: String,
    pub name: String,
    /// X25519 secret key, base64url.
    pub secret_key: String,
    /// Seals presence for every paired phone, base64url.
    pub presence_key: String,
}

impl fmt::Debug for HostIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostIdentity")
            .field("host_id", &self.host_id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl HostIdentity {
    #[must_use]
    pub fn generate(name: &str) -> Self {
        Self {
            host_id: random_id(),
            name: name.to_string(),
            secret_key: b64u(&random_bytes::<32>()),
            presence_key: b64u(&random_bytes::<32>()),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedPhone {
    pub phone_id: String,
    pub name: String,
    /// The key both ends derived at pairing, base64url.
    pub pair_key: String,
    /// Unix milliseconds.
    pub paired_at: u64,
}

impl fmt::Debug for PairedPhone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairedPhone")
            .field("phone_id", &self.phone_id)
            .field("name", &self.name)
            .field("paired_at", &self.paired_at)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareConfig {
    #[serde(default)]
    pub identity: Option<HostIdentity>,
    #[serde(default)]
    pub phones: Vec<PairedPhone>,
    /// Project folder → share id.
    #[serde(default)]
    pub shares: BTreeMap<String, String>,
}

impl ShareConfig {
    /// The identity, made on first use; its name follows `name` when one is given.
    pub fn identity_named(&mut self, name: &str) -> &HostIdentity {
        let identity = self
            .identity
            .get_or_insert_with(|| HostIdentity::generate(name));
        if !name.is_empty() {
            identity.name = name.to_string();
        }
        identity
    }

    /// The share id of a project, made on first use.
    pub fn share_id_for(&mut self, project: &str) -> String {
        self.shares
            .entry(project.to_string())
            .or_insert_with(random_id)
            .clone()
    }
}

/// Reads and writes `remote.json`.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join(CONFIG_FILE),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved config, or an empty one on first use. A damaged file is an error rather than a
    /// silent new identity (that would unpair every phone).
    pub fn load(&self) -> io::Result<ShareConfig> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is damaged: {error}", self.path.display()),
                )
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ShareConfig::default()),
            Err(error) => Err(error),
        }
    }

    /// Writes the config whole (a temporary file renamed over it), readable only by the user.
    pub fn save(&self, config: &ShareConfig) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let temporary = self.path.with_extension("json.tmp");
        let text = serde_json::to_string_pretty(config).map_err(io::Error::other)?;
        {
            let mut file = private_file(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&temporary, &self.path)
    }
}

#[cfg(unix)]
fn private_file(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

// Windows: the file inherits the user profile folder's ACL (only the user and administrators).
#[cfg(not(unix))]
fn private_file(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_identity_phones_and_share_ids() {
        let dir = std::env::temp_dir().join(format!("neuron-share-config-{}", random_id()));
        let store = ConfigStore::in_dir(&dir);
        let mut config = store.load().unwrap();
        assert_eq!(config, ShareConfig::default());
        let identity = config.identity_named("Studio").clone();
        assert_eq!(identity.host_id.len(), 22);
        let share = config.share_id_for("/work/neuron");
        config.phones.push(PairedPhone {
            phone_id: "phone-0001".into(),
            name: "Pixel".into(),
            pair_key: b64u(&[3; 32]),
            paired_at: 1,
        });
        store.save(&config).unwrap();
        let mut again = store.load().unwrap();
        assert_eq!(again, config);
        assert_eq!(
            again.identity_named("Desk").host_id,
            identity.host_id,
            "the identity outlives a rename"
        );
        assert_eq!(again.share_id_for("/work/neuron"), share);
        assert!(
            !format!("{identity:?}").contains(&identity.secret_key),
            "secrets stay out of logs"
        );
        fs::write(store.path(), "{not json").unwrap();
        assert!(
            store.load().is_err(),
            "a damaged file is not replaced silently"
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
