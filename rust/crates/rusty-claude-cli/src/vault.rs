//! In-memory secret handling and the reader for the retired `gateway.enc`
//! format.
//!
//! The old vault derived its AES key with PBKDF2 from a "machine
//! fingerprint" that was stored, in clear, inside the same file, so anyone
//! able to read the file could decrypt it. It also depended on `wmic`, which
//! current Windows builds no longer ship. Credentials now live in the OS
//! keychain or a 0600 file (see `credentials.rs`); this module only keeps
//! what is needed to migrate an existing `gateway.enc` once.

use std::fs;
use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

const LEGACY_VAULT_VERSION: u8 = 1;
const LEGACY_KDF_ITERATIONS: u32 = 100_000;

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

impl PartialEq for SecureString {
    fn eq(&self, other: &Self) -> bool {
        constant_time_eq::constant_time_eq(self.0.as_bytes(), other.0.as_bytes())
    }
}

impl Eq for SecureString {}

/// On-disk layout of the retired `gateway.enc` vault.
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

/// `~/.neuroncli/vault.enc`; its directory also holds `trusted_dirs.txt`.
pub fn vault_path() -> PathBuf {
    crate::credentials::neuron_home().join("vault.enc")
}

/// Decrypts a legacy `gateway.enc`. The PBKDF2 password and HMAC key were
/// the `machine_fp` value stored in the file itself, so the file decrypts on
/// any machine; that is exactly why it is being retired.
pub fn decrypt_legacy_vault(path: &Path) -> Result<SecureString, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
    let vault: VaultFile =
        serde_json::from_str(&content).map_err(|e| format!("parse vault: {e}"))?;

    if vault.version != LEGACY_VAULT_VERSION {
        return Err(format!("unsupported vault version: {}", vault.version));
    }
    let salt = B64
        .decode(&vault.salt)
        .map_err(|e| format!("salt b64: {e}"))?;
    let nonce_bytes = B64
        .decode(&vault.nonce)
        .map_err(|e| format!("nonce b64: {e}"))?;
    if salt.len() != 16 || nonce_bytes.len() != 12 {
        return Err("invalid vault salt or nonce length".into());
    }
    let ciphertext = B64
        .decode(&vault.ciphertext)
        .map_err(|e| format!("ct b64: {e}"))?;
    let stored_integrity = B64
        .decode(&vault.integrity)
        .map_err(|e| format!("hmac b64: {e}"))?;

    let expected = legacy_integrity(
        &vault.machine_fp,
        vault.version,
        &salt,
        &nonce_bytes,
        &ciphertext,
    );
    if !constant_time_eq::constant_time_eq(&stored_integrity, &expected) {
        return Err("vault integrity check failed".into());
    }

    let mut vault_key = legacy_key(&vault.machine_fp, &salt);
    let cipher = Aes256Gcm::new_from_slice(&vault_key).map_err(|e| format!("cipher init: {e}"))?;
    vault_key.zeroize();
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| "vault decryption failed (corrupted file)".to_string())?;
    String::from_utf8(plaintext)
        .map(SecureString::new)
        .map_err(|e| format!("utf8: {e}"))
}

fn legacy_key(fingerprint: &str, salt: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    pbkdf2_hmac::<Sha256>(
        fingerprint.as_bytes(),
        salt,
        LEGACY_KDF_ITERATIONS,
        &mut key,
    );
    key
}

fn legacy_integrity(
    fingerprint: &str,
    version: u8,
    salt: &[u8],
    nonce: &[u8],
    ciphertext: &[u8],
) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(fingerprint.as_bytes())
        .expect("HMAC accepts any key size");
    mac.update(&[version]);
    mac.update(salt);
    mac.update(nonce);
    mac.update(ciphertext);
    mac.finalize().into_bytes().to_vec()
}

/// Writes a vault in the retired format. Only used by tests that exercise
/// the one-time migration.
#[cfg(test)]
pub fn write_legacy_vault_for_test(token: &str, path: &Path, fingerprint: &str) {
    use rand::RngCore;

    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let cipher = Aes256Gcm::new_from_slice(&legacy_key(fingerprint, &salt)).unwrap();
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), token.as_bytes())
        .unwrap();
    let vault = VaultFile {
        version: LEGACY_VAULT_VERSION,
        salt: B64.encode(salt),
        nonce: B64.encode(nonce),
        integrity: B64.encode(legacy_integrity(
            fingerprint,
            LEGACY_VAULT_VERSION,
            &salt,
            &nonce,
            &ciphertext,
        )),
        ciphertext: B64.encode(&ciphertext),
        machine_fp: fingerprint.to_string(),
        created_at: 0,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, serde_json::to_string_pretty(&vault).unwrap()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neuron-legacy-vault-{label}-{}-{}.enc",
            std::process::id(),
            crate::clock::now_unix()
        ))
    }

    #[test]
    fn legacy_vault_decrypts_without_the_original_machine_id() {
        let path = temp_path("roundtrip");
        let token = "ses_012345678901234567890123456789";
        // A fingerprint that depended on `wmic`; current builds cannot
        // recompute it, but the file carries it.
        write_legacy_vault_for_test(token, &path, "0f1e2d3c4b5a69788796a5b4c3d2e1f0");
        assert_eq!(decrypt_legacy_vault(&path).unwrap().expose(), token);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn legacy_vault_rejects_tampering_and_malformed_nonces() {
        let path = temp_path("tamper");
        write_legacy_vault_for_test("ses_012345678901234567890123456789", &path, "fp");
        let mut stored: VaultFile =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        stored.machine_fp = "other".to_string();
        fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
        assert!(decrypt_legacy_vault(&path)
            .unwrap_err()
            .contains("integrity"));
        stored.nonce = B64.encode([0u8; 11]);
        fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();
        assert!(decrypt_legacy_vault(&path)
            .unwrap_err()
            .contains("nonce length"));
        fs::write(&path, "{not json").unwrap();
        assert!(decrypt_legacy_vault(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn secure_string_debug_never_prints_the_secret() {
        let secret = SecureString::new("ses_topsecret".to_string());
        assert_eq!(format!("{secret:?}"), "SecureString(***)");
    }
}
