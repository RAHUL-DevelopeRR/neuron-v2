//! The relay's end-to-end encryption (`neuron-relay/1`, `RELAY_CONTRACT.md` §3): the same
//! constructions as the apps' `ui/neuron-mobile/src/relay/crypto.ts`, checked against its test
//! vectors. The relay routes frames and never holds a key:
//!
//! - Pairing: the QR code carries the computer's X25519 public key and a one-time code. Both ends
//!   derive the pair key with HKDF-SHA256, salted with the code; the relay only sees a hash of the
//!   code (the pair id), so it cannot pair in the phone's place.
//! - Channels: each attach makes fresh X25519 keys on both ends and the pair key salts the HKDF,
//!   so a relay that swaps keys derives nothing useful.
//! - Frames: AES-256-GCM, one key per direction, the frame counter is the nonce, and the channel
//!   id and direction are the associated data.

use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::rngs::OsRng;
use rand::RngCore;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::contract::Role;

pub const LABEL: &str = "neuron-relay/1";

/// Which way a frame goes: phone to computer, or computer to phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    P2h,
    H2p,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Self::P2h => "p2h",
            Self::H2p => "h2p",
        }
    }
}

/// Why a key, proof or frame was refused. `code` is the contract's (`bad_key`, `bad_frame`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct E2eError {
    pub code: &'static str,
    pub message: String,
}

impl E2eError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for E2eError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for E2eError {}

/// base64url without padding (RFC 4648 §5).
#[must_use]
pub fn b64u(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Decodes base64url without padding; `None` when it is not exactly that (padding, other
/// characters, or leftover bits that are not zero, so every byte string has one spelling).
#[must_use]
pub fn from_b64u(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

/// Decodes base64url of exactly `N` bytes, or a `bad_key` error naming `what`.
pub fn bytes_of<const N: usize>(text: &str, what: &str) -> Result<[u8; N], E2eError> {
    from_b64u(text)
        .and_then(|bytes| <[u8; N]>::try_from(bytes).ok())
        .ok_or_else(|| E2eError::new("bad_key", format!("{what} is not {N} bytes of base64url.")))
}

/// `N` bytes from the operating system's secure random source.
#[must_use]
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

/// A random id: 16 bytes of base64url (22 characters).
#[must_use]
pub fn random_id() -> String {
    b64u(&random_bytes::<16>())
}

/// An X25519 key pair.
#[derive(Clone)]
pub struct KeyPair {
    pub secret: [u8; 32],
    pub public: [u8; 32],
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPair")
            .field("public", &b64u(&self.public))
            .finish_non_exhaustive()
    }
}

#[must_use]
pub fn key_pair() -> KeyPair {
    let secret = random_bytes::<32>();
    KeyPair {
        secret,
        public: public_key_of(&secret),
    }
}

#[must_use]
pub fn public_key_of(secret: &[u8; 32]) -> [u8; 32] {
    PublicKey::from(&StaticSecret::from(*secret)).to_bytes()
}

fn shared_secret(secret: &[u8; 32], peer_public: &[u8; 32]) -> Result<[u8; 32], E2eError> {
    let shared = StaticSecret::from(*secret)
        .diffie_hellman(&PublicKey::from(*peer_public))
        .to_bytes();
    // A small-order point gives all zeros: refuse it rather than derive from nothing.
    if shared.iter().all(|byte| *byte == 0) {
        return Err(E2eError::new("bad_key", "The other end sent a weak key."));
    }
    Ok(shared)
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

fn hkdf_into(ikm: &[u8], salt: Option<&[u8]>, info: &[u8], out: &mut [u8]) {
    Hkdf::<Sha256>::new(salt, ikm)
        .expand(info, out)
        .expect("HKDF-SHA256 gives up to 8160 bytes");
}

fn hmac16(key: &[u8], message: &[u8]) -> [u8; 16] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(message);
    let digest = mac.finalize().into_bytes();
    let mut out = [0_u8; 16];
    out.copy_from_slice(&digest[..16]);
    out
}

/// The one-time code in the QR: 16 random bytes.
#[must_use]
pub fn new_pairing_code() -> String {
    b64u(&random_bytes::<16>())
}

/// What the relay routes a pairing by: a hash of the code, so the relay never learns it.
pub fn pair_id_for(code: &str) -> Result<String, E2eError> {
    let code = bytes_of::<16>(code, "The pairing code")?;
    let mut hash = Sha256::new();
    hash.update(format!("{LABEL} pair-id").as_bytes());
    hash.update(code);
    Ok(b64u(&hash.finalize()[..16]))
}

/// The key a phone and a computer share once paired. Each end passes its own secret key and the
/// other's public key; `host_public` and `phone_public` are the same on both ends.
pub fn derive_pair_key(
    secret: &[u8; 32],
    peer_public: &[u8; 32],
    code: &str,
    host_public: &[u8; 32],
    phone_public: &[u8; 32],
) -> Result<[u8; 32], E2eError> {
    let shared = shared_secret(secret, peer_public)?;
    let salt = bytes_of::<16>(code, "The pairing code")?;
    let info = [
        format!("{LABEL} pair").as_bytes(),
        host_public,
        phone_public,
    ]
    .concat();
    let mut key = [0_u8; 32];
    hkdf_into(&shared, Some(&salt), &info, &mut key);
    Ok(key)
}

/// Proves knowing the pair key (so the code): 16 bytes of HMAC, base64url.
#[must_use]
pub fn pair_proof(pair_key: &[u8; 32], role: Role, pair_id: &str) -> String {
    b64u(&hmac16(
        pair_key,
        format!("{LABEL} {}-proof {pair_id}", role.as_str()).as_bytes(),
    ))
}

/// Compares two base64url proofs in constant time.
#[must_use]
pub fn proof_matches(expected: &str, given: &str) -> bool {
    match (from_b64u(expected), from_b64u(given)) {
        (Some(a), Some(b)) => constant_time_eq(&a, &b),
        _ => false,
    }
}

/// A key for one purpose from the pair key (`pair-secrets` seals what the computer sends at
/// pairing).
#[must_use]
pub fn subkey(pair_key: &[u8; 32], purpose: &str) -> [u8; 32] {
    let mut key = [0_u8; 32];
    hkdf_into(
        pair_key,
        None,
        format!("{LABEL} {purpose}").as_bytes(),
        &mut key,
    );
    key
}

/// The keys of one channel (one attach of a phone to a shared session).
#[derive(Clone, PartialEq, Eq)]
pub struct ChannelKeys {
    /// Seals what the phone sends.
    pub p2h: [u8; 32],
    /// Seals what the computer sends.
    pub h2p: [u8; 32],
}

impl fmt::Debug for ChannelKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ChannelKeys(..)")
    }
}

/// Both ends pass their own ephemeral secret and the other's ephemeral public key.
pub fn derive_channel_keys(
    secret: &[u8; 32],
    peer_public: &[u8; 32],
    pair_key: &[u8; 32],
    phone_ephemeral: &[u8; 32],
    host_ephemeral: &[u8; 32],
    ch: &str,
) -> Result<ChannelKeys, E2eError> {
    let shared = shared_secret(secret, peer_public)?;
    let info = [
        format!("{LABEL} channel {ch}").as_bytes(),
        phone_ephemeral,
        host_ephemeral,
    ]
    .concat();
    let mut okm = [0_u8; 64];
    hkdf_into(&shared, Some(pair_key), &info, &mut okm);
    let mut keys = ChannelKeys {
        p2h: [0; 32],
        h2p: [0; 32],
    };
    keys.p2h.copy_from_slice(&okm[..32]);
    keys.h2p.copy_from_slice(&okm[32..]);
    Ok(keys)
}

/// The computer's proof, in `accept`, that it holds the pair key.
#[must_use]
pub fn accept_confirm(keys: &ChannelKeys, ch: &str) -> String {
    b64u(&hmac16(
        &keys.h2p,
        format!("{LABEL} accept {ch}").as_bytes(),
    ))
}

fn frame_nonce(n: u64) -> [u8; 12] {
    // 4 zero bytes, then n as 64-bit big-endian.
    let mut nonce = [0_u8; 12];
    nonce[4..].copy_from_slice(&n.to_be_bytes());
    nonce
}

fn frame_aad(ch: &str, dir: Direction) -> String {
    format!("{LABEL}|{ch}|{}", dir.as_str())
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new(key.into())
}

/// Seals one inner frame (JSON text) as the `c` of a `data` frame.
#[must_use]
pub fn seal_frame(key: &[u8; 32], ch: &str, dir: Direction, n: u64, plaintext: &str) -> String {
    let sealed = cipher(key)
        .encrypt(
            Nonce::from_slice(&frame_nonce(n)),
            Payload {
                msg: plaintext.as_bytes(),
                aad: frame_aad(ch, dir).as_bytes(),
            },
        )
        .expect("AES-GCM sealing does not fail for frames under 64 GB");
    b64u(&sealed)
}

/// Opens the `c` of a `data` frame; `bad_frame` when it was not sealed with this key, channel,
/// direction and counter.
pub fn open_frame(
    key: &[u8; 32],
    ch: &str,
    dir: Direction,
    n: u64,
    c: &str,
) -> Result<String, E2eError> {
    let sealed = from_b64u(c)
        .filter(|bytes| bytes.len() >= 16)
        .ok_or_else(|| E2eError::new("bad_frame", "A frame is not sealed data."))?;
    let plain = cipher(key)
        .decrypt(
            Nonce::from_slice(&frame_nonce(n)),
            Payload {
                msg: &sealed,
                aad: frame_aad(ch, dir).as_bytes(),
            },
        )
        .map_err(|_| {
            E2eError::new(
                "bad_frame",
                "A frame did not open: it was changed, replayed or sealed with another key.",
            )
        })?;
    String::from_utf8(plain).map_err(|_| E2eError::new("bad_frame", "A frame is not text."))
}

/// Seals a small value with a random nonce (presence and the pairing secrets): base64url of
/// nonce ‖ ciphertext.
#[must_use]
pub fn seal_blob(key: &[u8; 32], aad: &str, plaintext: &str) -> String {
    seal_blob_with_nonce(key, aad, plaintext, random_bytes::<12>())
}

pub(crate) fn seal_blob_with_nonce(
    key: &[u8; 32],
    aad: &str,
    plaintext: &str,
    nonce: [u8; 12],
) -> String {
    let sealed = cipher(key)
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext.as_bytes(),
                aad: aad.as_bytes(),
            },
        )
        .expect("AES-GCM sealing does not fail for small values");
    b64u(&[nonce.as_slice(), &sealed].concat())
}

pub fn open_blob(key: &[u8; 32], aad: &str, sealed: &str) -> Result<String, E2eError> {
    let bytes = from_b64u(sealed)
        .filter(|bytes| bytes.len() >= 28)
        .ok_or_else(|| E2eError::new("bad_frame", "A sealed value is not sealed data."))?;
    let plain = cipher(key)
        .decrypt(
            Nonce::from_slice(&bytes[..12]),
            Payload {
                msg: &bytes[12..],
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| E2eError::new("bad_frame", "A sealed value did not open."))?;
    String::from_utf8(plain).map_err(|_| E2eError::new("bad_frame", "A sealed value is not text."))
}

#[must_use]
pub fn presence_aad(host_id: &str, share_id: &str) -> String {
    format!("{LABEL} presence|{host_id}|{share_id}")
}

#[must_use]
pub fn pair_secrets_aad(pair_id: &str) -> String {
    format!("{LABEL} pair-secrets|{pair_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range<const N: usize>(start: u8) -> [u8; N] {
        let mut bytes = [0_u8; N];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = start + u8::try_from(index).unwrap();
        }
        bytes
    }

    // RELAY_CONTRACT.md §3.7: the vectors the apps' test/crypto.test.ts checks.
    #[test]
    fn matches_the_contract_vectors() {
        let host_secret = range::<32>(1);
        let phone_secret = range::<32>(0x40);
        let host_ephemeral_secret = range::<32>(0x80);
        let phone_ephemeral_secret = range::<32>(0xa0);
        let code = b64u(&range::<16>(0xc0));
        let ch = "channel-0000000001";

        let host_key = public_key_of(&host_secret);
        let phone = public_key_of(&phone_secret);
        assert_eq!(
            b64u(&host_key),
            "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw"
        );
        assert_eq!(b64u(&phone), "eaYx7t4b-cmPEgMs3q3Q56B5OY_HhriMyEbsia-FpRo");
        let pair_id = pair_id_for(&code).unwrap();
        assert_eq!(pair_id, "xMm4z_g3jDfuYMKwidZHBg");
        let pair_key = derive_pair_key(&phone_secret, &host_key, &code, &host_key, &phone).unwrap();
        assert_eq!(
            b64u(&pair_key),
            "4D5wivpGleJpH83D0sn-YabpzY7zC8PG-6GXqDdUalk"
        );
        assert_eq!(
            derive_pair_key(&host_secret, &phone, &code, &host_key, &phone).unwrap(),
            pair_key,
            "both ends derive the same pair key"
        );
        assert_eq!(
            pair_proof(&pair_key, Role::Phone, &pair_id),
            "cbwHSSkvuXNM06i3rkMJkw"
        );
        assert_eq!(
            pair_proof(&pair_key, Role::Host, &pair_id),
            "A9pJGL2ru9oPNO9AkqUO9A"
        );
        assert_eq!(
            b64u(&subkey(&pair_key, "pair-secrets")),
            "ml4UAMqUKZdHGLgVhmj2Kn-ArGEsmn2CFyoqF-3mJmo"
        );

        let phone_ephemeral = public_key_of(&phone_ephemeral_secret);
        let host_ephemeral = public_key_of(&host_ephemeral_secret);
        let keys = derive_channel_keys(
            &phone_ephemeral_secret,
            &host_ephemeral,
            &pair_key,
            &phone_ephemeral,
            &host_ephemeral,
            ch,
        )
        .unwrap();
        assert_eq!(
            derive_channel_keys(
                &host_ephemeral_secret,
                &phone_ephemeral,
                &pair_key,
                &phone_ephemeral,
                &host_ephemeral,
                ch
            )
            .unwrap(),
            keys
        );
        assert_eq!(
            b64u(&keys.p2h),
            "N174o0i91fATCDiCxybPvW1IVSik5YrI1VXfwhySPl8"
        );
        assert_eq!(
            b64u(&keys.h2p),
            "CqAUf_LDYB8oW3HicMS7OV03SzZQgPpuGpyutLS3Ii4"
        );
        assert_eq!(accept_confirm(&keys, ch), "WmXZ4JoZr6XvkScveQ9llQ");
        assert_eq!(
            seal_frame(&keys.p2h, ch, Direction::P2h, 0, r#"{"t":"ping"}"#),
            "FHfS93E8PCTjaHu3pzTqB3pme1Hi1W-Z3o793A"
        );
        assert_eq!(
            seal_frame(&keys.h2p, ch, Direction::H2p, 1, r#"{"t":"pong"}"#),
            "7-VzfWIbvw7QbuxBgJrSWrbWheJ1f2hUUvjEfA"
        );
        assert_eq!(
            open_frame(
                &keys.p2h,
                ch,
                Direction::P2h,
                0,
                "FHfS93E8PCTjaHu3pzTqB3pme1Hi1W-Z3o793A"
            )
            .unwrap(),
            r#"{"t":"ping"}"#
        );
    }

    #[test]
    fn refuses_tampered_frames_and_weak_keys() {
        let key = [7_u8; 32];
        let sealed = seal_frame(&key, "channel-1", Direction::H2p, 3, "hello");
        assert!(
            open_frame(&key, "channel-1", Direction::H2p, 4, &sealed).is_err(),
            "another counter"
        );
        assert!(
            open_frame(&key, "channel-2", Direction::H2p, 3, &sealed).is_err(),
            "another channel"
        );
        assert!(
            open_frame(&key, "channel-1", Direction::P2h, 3, &sealed).is_err(),
            "another direction"
        );
        assert_eq!(
            open_frame(&key, "channel-1", Direction::H2p, 3, &sealed).unwrap(),
            "hello"
        );
        let blob = seal_blob(&key, "aad", "{}");
        assert_eq!(open_blob(&key, "aad", &blob).unwrap(), "{}");
        assert!(open_blob(&key, "other", &blob).is_err());
        // The all-zero point is small order.
        let error =
            derive_pair_key(&[9; 32], &[0; 32], &b64u(&[1; 16]), &[0; 32], &[0; 32]).unwrap_err();
        assert_eq!(error.code, "bad_key");
        assert!(proof_matches(
            "cbwHSSkvuXNM06i3rkMJkw",
            "cbwHSSkvuXNM06i3rkMJkw"
        ));
        assert!(!proof_matches(
            "cbwHSSkvuXNM06i3rkMJkw",
            "cbwHSSkvuXNM06i3rkMJkA"
        ));
        assert!(from_b64u("abc=").is_none(), "no padding");
        assert!(from_b64u("ab+/").is_none(), "base64url only");
        assert!(from_b64u("AB").is_none(), "leftover bits must be zero");
    }
}
