use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

const VAULT_VERSION: u8 = 1;
const KDF_ITERATIONS: u32 = 100_000;

/// A string scrubbed from memory on drop.
#[derive(Clone, ZeroizeOnDrop)]
pub struct SecureString(String);

impl SecureString {
    pub fn new(s: String) -> Self {
        Self(s)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecureString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecureString(***)")
    }
}

#[derive(Serialize, Deserialize)]
pub struct VaultFile {
    pub version: u8,
    pub salt: String,
    pub nonce: String,
    pub ciphertext: String,
    pub integrity: String,
    pub machine_fp: String,
    pub created_at: u64,
}

pub fn vault_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".neuroncli").join("vault.enc")
}

pub fn key_fingerprint(key: &str) -> String {
    let hash = Sha256::digest(key.as_bytes());
    let hex_part = hex::encode(&hash[..8]);
    let prefix = if key.len() > 6 { &key[..6] } else { "sk-???" };
    format!("{prefix}...:{hex_part}")
}

fn machine_fingerprint() -> String {
    let machine_id = machine_id().unwrap_or_else(|| "unknown-machine".to_string());
    let username = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unknown-user".to_string());
    let input = format!("{machine_id}:{username}:neuroncli-vault-v1");
    let hash = Sha256::digest(input.as_bytes());
    hex::encode(&hash[..16])
}

fn machine_id() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("wmic")
            .args(["csproduct", "get", "UUID"])
            .output()
            .ok()
            .and_then(|o| {
                let s = String::from_utf8_lossy(&o.stdout);
                s.lines().nth(1).map(|l| l.trim().to_string())
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|s| s.trim().to_string())
    }
}

fn derive_vault_key(salt: &[u8]) -> [u8; 32] {
    let fp = machine_fingerprint();
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(fp.as_bytes(), salt, KDF_ITERATIONS, &mut key);
    key
}

fn compute_integrity(version: u8, salt: &[u8], nonce: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let fp = machine_fingerprint();
    let mut mac =
        <Hmac<Sha256> as Mac>::new_from_slice(fp.as_bytes()).expect("HMAC accepts any key size");
    mac.update(&[version]);
    mac.update(salt);
    mac.update(nonce);
    mac.update(ciphertext);
    mac.finalize().into_bytes().to_vec()
}

pub fn encrypt_and_store(key: &str, path: &Path) -> Result<(), String> {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);

    let mut vault_key = derive_vault_key(&salt);
    let cipher = Aes256Gcm::new_from_slice(&vault_key).map_err(|e| format!("cipher init: {e}"))?;
    vault_key.zeroize();

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, key.as_bytes())
        .map_err(|e| format!("encrypt: {e}"))?;

    let integrity = compute_integrity(VAULT_VERSION, &salt, &nonce_bytes, &ciphertext);

    let vault = VaultFile {
        version: VAULT_VERSION,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce_bytes),
        ciphertext: B64.encode(&ciphertext),
        integrity: B64.encode(&integrity),
        machine_fp: machine_fingerprint(),
        created_at: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }

    let json = serde_json::to_string_pretty(&vault).map_err(|e| format!("json: {e}"))?;

    // Atomic write: write to .tmp then rename
    let tmp = path.with_extension("enc.tmp");
    fs::write(&tmp, &json).map_err(|e| format!("write: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;

    // Restrictive permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

pub fn decrypt_from_vault(path: &Path) -> Result<SecureString, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let vault: VaultFile =
        serde_json::from_str(&content).map_err(|e| format!("parse vault: {e}"))?;

    if vault.version != VAULT_VERSION {
        return Err(format!("unsupported vault version: {}", vault.version));
    }

    // Check machine fingerprint
    if vault.machine_fp != machine_fingerprint() {
        return Err("vault was created on a different machine".into());
    }

    let salt = B64
        .decode(&vault.salt)
        .map_err(|e| format!("salt b64: {e}"))?;
    let nonce_bytes = B64
        .decode(&vault.nonce)
        .map_err(|e| format!("nonce b64: {e}"))?;
    let ciphertext = B64
        .decode(&vault.ciphertext)
        .map_err(|e| format!("ct b64: {e}"))?;
    let stored_integrity = B64
        .decode(&vault.integrity)
        .map_err(|e| format!("hmac b64: {e}"))?;

    // Verify integrity before decryption
    let expected = compute_integrity(vault.version, &salt, &nonce_bytes, &ciphertext);
    if !constant_time_eq::constant_time_eq(&stored_integrity, &expected) {
        return Err("vault integrity check failed (tampered?)".into());
    }

    let mut vault_key = derive_vault_key(&salt);
    let cipher = Aes256Gcm::new_from_slice(&vault_key).map_err(|e| format!("cipher init: {e}"))?;
    vault_key.zeroize();

    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| "vault decryption failed (wrong machine or corrupted)".to_string())?;

    let key = String::from_utf8(plaintext).map_err(|e| format!("utf8: {e}"))?;
    Ok(SecureString::new(key))
}

pub fn delete_vault(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn health_check_path() -> PathBuf {
    vault_path().with_extension("health")
}

pub fn needs_health_check() -> bool {
    let path = health_check_path();
    match fs::metadata(&path) {
        Ok(meta) => {
            let age = meta
                .modified()
                .ok()
                .and_then(|t| t.elapsed().ok())
                .unwrap_or(std::time::Duration::MAX);
            age > std::time::Duration::from_secs(86400)
        }
        Err(_) => true,
    }
}

pub fn record_health_check() {
    let path = health_check_path();
    let _ = fs::write(
        &path,
        format!(
            "{}",
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ),
    );
}
