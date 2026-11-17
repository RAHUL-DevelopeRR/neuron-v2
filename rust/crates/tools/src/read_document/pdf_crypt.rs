//! The PDF standard security handler, for files that open without a
//! password: those protected only against copying, printing or editing
//! (an owner password), whose user password is empty.
//!
//! Revisions 2 to 4 (RC4 and AES-128, keyed through MD5) and 5 and 6
//! (AES-256, keyed through SHA-2) are supported. A file whose empty user
//! password does not authenticate needs a password to open and is refused.

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::{Aes128, Aes256, Block};
use sha2::{Digest, Sha256, Sha384, Sha512};

/// The padding string of the PDF standard security handler (Algorithm 2).
pub(super) const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// How one kind of data (strings or streams) is encrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Method {
    /// Not encrypted (`/Identity`).
    Identity,
    Rc4,
    /// AES-128 in CBC mode with a per-object key.
    Aes128,
    /// AES-256 in CBC mode with the file key.
    Aes256,
}

impl Method {
    /// The method a crypt filter's `/CFM` names; `None` for one this reader
    /// does not know.
    pub(super) fn from_cfm(name: Option<&str>) -> Option<Self> {
        match name {
            None | Some("None") => Some(Self::Identity),
            Some("V2") => Some(Self::Rc4),
            Some("AESV2") => Some(Self::Aes128),
            Some("AESV3") => Some(Self::Aes256),
            Some(_) => None,
        }
    }
}

/// The entries of an `/Encrypt` dictionary this handler reads.
#[derive(Debug, Clone)]
pub(super) struct Settings {
    /// The handler version (`/V`), which sets the key length when
    /// `/Length` is missing.
    pub(super) version: u32,
    pub(super) revision: u32,
    /// Key length in bits (`/Length`), when given.
    pub(super) length_bits: Option<u32>,
    pub(super) owner: Vec<u8>,
    pub(super) user: Vec<u8>,
    pub(super) user_key: Vec<u8>,
    pub(super) permissions: i32,
    pub(super) encrypt_metadata: bool,
    /// The first entry of the trailer's `/ID`.
    pub(super) id: Vec<u8>,
    pub(super) strings: Method,
    pub(super) streams: Method,
}

/// A file key that the empty user password unlocked.
#[derive(Debug)]
pub(super) struct Security {
    key: Vec<u8>,
    strings: Method,
    streams: Method,
}

impl Security {
    /// Unlocks the file with the empty user password. The error says why
    /// the file cannot be read.
    pub(super) fn open(settings: &Settings) -> Result<Self, String> {
        let key = match settings.revision {
            2..=4 => legacy_key_lengths(settings)
                .into_iter()
                .map(|length| legacy_key(settings, length))
                .find(|key| legacy_user_matches(settings, key))
                .ok_or_else(needs_password)?,
            5 | 6 => modern_key(settings)?,
            _ => return Err(unsupported()),
        };
        Ok(Self {
            key,
            strings: settings.strings,
            streams: settings.streams,
        })
    }

    /// Whether streams are encrypted with AES, whose data is whole blocks.
    pub(super) fn streams_use_aes(&self) -> bool {
        matches!(self.streams, Method::Aes128 | Method::Aes256)
    }

    /// Decrypts a string (or with `stream`, a stream's data) of object
    /// `number`, generation `generation`.
    pub(super) fn decrypt(
        &self,
        number: u32,
        generation: u16,
        data: &[u8],
        stream: bool,
    ) -> Vec<u8> {
        let method = if stream { self.streams } else { self.strings };
        match method {
            Method::Identity => data.to_vec(),
            Method::Rc4 => rc4(&self.object_key(number, generation, false), data),
            Method::Aes128 => aes_cbc_decrypt(&self.object_key(number, generation, true), data),
            Method::Aes256 => aes_cbc_decrypt(&self.key, data),
        }
    }

    /// Algorithm 1: the key for one object's strings and streams.
    fn object_key(&self, number: u32, generation: u16, aes: bool) -> Vec<u8> {
        let mut input = self.key.clone();
        input.extend_from_slice(&number.to_le_bytes()[..3]);
        input.extend_from_slice(&generation.to_le_bytes());
        if aes {
            input.extend_from_slice(b"sAlT");
        }
        let hash = md5(&input);
        hash[..(self.key.len() + 5).min(16)].to_vec()
    }
}

fn needs_password() -> String {
    "this PDF is encrypted with a password needed to open it; its text cannot be read without decrypting it first".to_string()
}

pub(super) fn unsupported() -> String {
    "this PDF is encrypted with a method read_document does not support; its text cannot be read without decrypting it first".to_string()
}

/// The key lengths (bytes) to try for revisions 2 to 4, most likely first.
/// Revision 2 always uses 40 bits. Otherwise `/Length` gives the length;
/// without it ISO 32000 defaults to 40 bits for versions 1 to 3, and
/// version 4 means 128 bits. A writer that left `/Length` out may not have
/// followed the default, so the other length is tried as well, checked
/// against `/U` like the first.
fn legacy_key_lengths(settings: &Settings) -> Vec<usize> {
    if settings.revision == 2 {
        return vec![5];
    }
    match settings.length_bits {
        Some(bits) => vec![usize::try_from(bits / 8).unwrap_or(16).clamp(5, 16)],
        None if settings.version >= 4 => vec![16, 5],
        None => vec![5, 16],
    }
}

/// Algorithm 2: the file key of revisions 2 to 4 for the empty password,
/// `length` bytes long.
fn legacy_key(settings: &Settings, length: usize) -> Vec<u8> {
    let mut input = PADDING.to_vec();
    input.extend_from_slice(&settings.owner[..settings.owner.len().min(32)]);
    input.extend_from_slice(&settings.permissions.to_le_bytes());
    input.extend_from_slice(&settings.id);
    if settings.revision >= 4 && !settings.encrypt_metadata {
        input.extend_from_slice(&[0xFF; 4]);
    }
    let mut hash = md5(&input);
    if settings.revision >= 3 {
        for _ in 0..50 {
            hash = md5(&hash[..length]);
        }
    }
    hash[..length].to_vec()
}

/// Algorithms 4 and 5: whether `key` is the one the empty user password
/// gives, checked against `/U`.
fn legacy_user_matches(settings: &Settings, key: &[u8]) -> bool {
    if settings.revision == 2 {
        return settings.user.len() >= 32 && rc4(key, &PADDING) == settings.user[..32];
    }
    let mut input = PADDING.to_vec();
    input.extend_from_slice(&settings.id);
    let mut value = rc4(key, &md5(&input));
    for round in 1..=19u8 {
        let round_key = key.iter().map(|byte| byte ^ round).collect::<Vec<_>>();
        value = rc4(&round_key, &value);
    }
    settings.user.len() >= 16 && value[..16] == settings.user[..16]
}

/// Algorithms 2.A and 11: the AES-256 file key of revisions 5 and 6 for
/// the empty password, from `/U` and `/UE`.
fn modern_key(settings: &Settings) -> Result<Vec<u8>, String> {
    if settings.user.len() < 48 || settings.user_key.len() < 32 {
        return Err(unsupported());
    }
    let hash = |salt: &[u8]| -> Vec<u8> {
        if settings.revision == 5 {
            Sha256::digest(salt).to_vec()
        } else {
            hash_2b(&[], salt)
        }
    };
    if hash(&settings.user[32..40]) != settings.user[..32] {
        return Err(needs_password());
    }
    let intermediate = hash(&settings.user[40..48]);
    let cipher = Aes256::new_from_slice(&intermediate).map_err(|_| unsupported())?;
    let mut key = Vec::with_capacity(32);
    let mut previous = [0u8; 16];
    for chunk in settings.user_key[..32].as_chunks::<16>().0 {
        let mut block = Block::from(*chunk);
        cipher.decrypt_block(&mut block);
        key.extend(block.iter().zip(previous).map(|(byte, iv)| byte ^ iv));
        previous = *chunk;
    }
    Ok(key)
}

/// Algorithm 2.B (revision 6) for an empty user password: a hash that
/// stops after 64 to 288 rounds.
fn hash_2b(password: &[u8], salt: &[u8]) -> Vec<u8> {
    let mut input = password.to_vec();
    input.extend_from_slice(salt);
    let mut key = Sha256::digest(&input).to_vec();
    let mut round = 0u32;
    loop {
        let mut repeated = Vec::with_capacity(64 * (password.len() + key.len()));
        for _ in 0..64 {
            repeated.extend_from_slice(password);
            repeated.extend_from_slice(&key);
        }
        let encrypted = aes128_cbc_encrypt(&key[..16], &key[16..32], &repeated);
        let sum = encrypted[..16]
            .iter()
            .map(|byte| u32::from(*byte))
            .sum::<u32>();
        key = match sum % 3 {
            0 => Sha256::digest(&encrypted).to_vec(),
            1 => Sha384::digest(&encrypted).to_vec(),
            _ => Sha512::digest(&encrypted).to_vec(),
        };
        round += 1;
        let last = encrypted.last().copied().map_or(0, u32::from);
        if round >= 64 && last + 32 <= round {
            break;
        }
    }
    key.truncate(32);
    key
}

/// AES-128-CBC without padding; `data` is a whole number of blocks.
fn aes128_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
    let Ok(cipher) = Aes128::new_from_slice(key) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(data.len());
    let mut previous = Block::clone_from_slice(iv);
    for chunk in data.as_chunks::<16>().0 {
        let mut block = Block::from(*chunk);
        for (byte, prior) in block.iter_mut().zip(previous.iter()) {
            *byte ^= prior;
        }
        cipher.encrypt_block(&mut block);
        out.extend_from_slice(&block);
        previous = block;
    }
    out
}

/// AES-CBC decryption of PDF data: a 16-byte IV, then the blocks, with
/// PKCS#5 padding removed when it is well formed. The key is 16 or 32
/// bytes. Damaged data decrypts as far as whole blocks go.
pub(super) fn aes_cbc_decrypt(key: &[u8], data: &[u8]) -> Vec<u8> {
    if data.len() < 32 {
        return Vec::new();
    }
    let (iv, body) = data.split_at(16);
    let decrypt: Box<dyn Fn(&mut Block)> = match key.len() {
        16 => match Aes128::new_from_slice(key) {
            Ok(cipher) => Box::new(move |block| cipher.decrypt_block(block)),
            Err(_) => return Vec::new(),
        },
        32 => match Aes256::new_from_slice(key) {
            Ok(cipher) => Box::new(move |block| cipher.decrypt_block(block)),
            Err(_) => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let mut out = Vec::with_capacity(body.len());
    let mut previous: &[u8] = iv;
    for chunk in body.as_chunks::<16>().0 {
        let mut block = Block::from(*chunk);
        decrypt(&mut block);
        out.extend(block.iter().zip(previous).map(|(byte, prior)| byte ^ prior));
        previous = chunk.as_slice();
    }
    if let Some(&pad) = out.last() {
        let pad = usize::from(pad);
        if (1..=16).contains(&pad)
            && pad <= out.len()
            && out[out.len() - pad..]
                .iter()
                .all(|byte| usize::from(*byte) == pad)
        {
            out.truncate(out.len() - pad);
        }
    }
    out
}

pub(super) fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    let mut state: [u8; 256] = std::array::from_fn(|index| u8::try_from(index).unwrap_or(0));
    let mut j = 0u8;
    for i in 0..256 {
        j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
        state.swap(i, usize::from(j));
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|byte| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(state[usize::from(i)]);
            state.swap(usize::from(i), usize::from(j));
            let k = state[usize::from(state[usize::from(i)].wrapping_add(state[usize::from(j)]))];
            byte ^ k
        })
        .collect()
}

/// Per-round rotations of MD5.
const MD5_SHIFTS: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9,
    14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15,
    21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];
/// MD5's per-round constants, from the sine function.
const MD5_SINES: [u32; 64] = [
    0xd76a_a478,
    0xe8c7_b756,
    0x2420_70db,
    0xc1bd_ceee,
    0xf57c_0faf,
    0x4787_c62a,
    0xa830_4613,
    0xfd46_9501,
    0x6980_98d8,
    0x8b44_f7af,
    0xffff_5bb1,
    0x895c_d7be,
    0x6b90_1122,
    0xfd98_7193,
    0xa679_438e,
    0x49b4_0821,
    0xf61e_2562,
    0xc040_b340,
    0x265e_5a51,
    0xe9b6_c7aa,
    0xd62f_105d,
    0x0244_1453,
    0xd8a1_e681,
    0xe7d3_fbc8,
    0x21e1_cde6,
    0xc337_07d6,
    0xf4d5_0d87,
    0x455a_14ed,
    0xa9e3_e905,
    0xfcef_a3f8,
    0x676f_02d9,
    0x8d2a_4c8a,
    0xfffa_3942,
    0x8771_f681,
    0x6d9d_6122,
    0xfde5_380c,
    0xa4be_ea44,
    0x4bde_cfa9,
    0xf6bb_4b60,
    0xbebf_bc70,
    0x289b_7ec6,
    0xeaa1_27fa,
    0xd4ef_3085,
    0x0488_1d05,
    0xd9d4_d039,
    0xe6db_99e5,
    0x1fa2_7cf8,
    0xc4ac_5665,
    0xf429_2244,
    0x432a_ff97,
    0xab94_23a7,
    0xfc93_a039,
    0x655b_59c3,
    0x8f0c_cc92,
    0xffef_f47d,
    0x8584_5dd1,
    0x6fa8_7e4f,
    0xfe2c_e6e0,
    0xa301_4314,
    0x4e08_11a1,
    0xf753_7e82,
    0xbd3a_f235,
    0x2ad7_d2bb,
    0xeb86_d391,
];

/// MD5 (RFC 1321), which the PDF security handler keys revisions 2 to 4
/// with. Not used for anything that needs a secure hash.
pub(super) fn md5(data: &[u8]) -> [u8; 16] {
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_le_bytes());
    for chunk in message.as_chunks::<64>().0 {
        let words: [u32; 16] = std::array::from_fn(|index| {
            u32::from_le_bytes([
                chunk[index * 4],
                chunk[index * 4 + 1],
                chunk[index * 4 + 2],
                chunk[index * 4 + 3],
            ])
        });
        let [mut a, mut b, mut c, mut d] = state;
        for round in 0..64 {
            let (mixed, index) = match round / 16 {
                0 => ((b & c) | (!b & d), round),
                1 => ((d & b) | (!d & c), (5 * round + 1) % 16),
                2 => (b ^ c ^ d, (3 * round + 5) % 16),
                _ => (c ^ (b | !d), (7 * round) % 16),
            };
            let rotated = a
                .wrapping_add(mixed)
                .wrapping_add(MD5_SINES[round])
                .wrapping_add(words[index])
                .rotate_left(MD5_SHIFTS[round]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (index, word) in state.iter().enumerate() {
        out[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

#[cfg(test)]
pub(super) mod fixtures {
    //! Encryption for test files, the inverse of the reader.

    use super::{
        aes128_cbc_encrypt, legacy_key, legacy_key_lengths, md5, rc4, Method, Settings, PADDING,
    };

    /// Algorithm 2.B for the empty password and `salt`.
    pub(crate) fn hash_2b(salt: &[u8]) -> Vec<u8> {
        super::hash_2b(&[], salt)
    }

    /// `/U` for the empty user password of revision 3 or 4 with `key`.
    pub(crate) fn legacy_user(settings: &Settings, key: &[u8]) -> Vec<u8> {
        let mut input = PADDING.to_vec();
        input.extend_from_slice(&settings.id);
        let mut value = rc4(key, &md5(&input));
        for round in 1..=19u8 {
            let round_key = key.iter().map(|byte| byte ^ round).collect::<Vec<_>>();
            value = rc4(&round_key, &value);
        }
        value.resize(32, 0);
        value
    }

    /// The file key of `settings` (revisions 2 to 4).
    pub(crate) fn file_key(settings: &Settings) -> Vec<u8> {
        legacy_key(settings, legacy_key_lengths(settings)[0])
    }

    /// Encrypts `data` of object `number` as the reader expects.
    pub(crate) fn encrypt(key: &[u8], method: Method, number: u32, data: &[u8]) -> Vec<u8> {
        let mut input = key.to_vec();
        input.extend_from_slice(&number.to_le_bytes()[..3]);
        input.extend_from_slice(&[0, 0]);
        if method == Method::Aes128 {
            input.extend_from_slice(b"sAlT");
        }
        let object_key = md5(&input)[..(key.len() + 5).min(16)].to_vec();
        match method {
            Method::Rc4 => rc4(&object_key, data),
            Method::Aes128 => {
                let iv = [7u8; 16];
                let pad = 16 - data.len() % 16;
                let mut padded = data.to_vec();
                padded.extend(std::iter::repeat_n(u8::try_from(pad).unwrap_or(16), pad));
                let mut out = iv.to_vec();
                out.extend(aes128_cbc_encrypt(&object_key, &iv, &padded));
                out
            }
            _ => data.to_vec(),
        }
    }
}
