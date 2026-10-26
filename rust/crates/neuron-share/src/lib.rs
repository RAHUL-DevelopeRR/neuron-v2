//! `neuron engine --share`: shares a Neuron session with the Neuron phone app through the Zero-X
//! relay (`neuron-relay/1`; the contract is `ui/neuron-mobile/docs/RELAY_CONTRACT.md`). This is the
//! Rust port of the apps' reference, `ui/neuron-mobile/src/relay/share-host.ts`.
//!
//! The session keeps running on this computer. The phone pairs once by scanning a QR code, then
//! attaches over end-to-end encrypted channels (X25519, HKDF-SHA256, AES-256-GCM) that the relay
//! cannot read. Each channel is bridged to the session's local remote-protocol-1 host, letting
//! through only what a phone may do.
//!
//! - [`crypto`], [`channel`]: the end-to-end encryption, checked against the contract's vectors.
//! - [`contract`]: the relay frames. [`pairing`]: the QR link.
//! - [`presence`]: what the phone's sessions list shows, from the session's stream.
//! - [`host`]: the share host without I/O. [`run()`]: the sockets around it.
//! - [`config`]: the computer's identity and paired phones in the Neuron config.

pub mod channel;
pub mod config;
pub mod contract;
pub mod crypto;
pub mod host;
pub mod pairing;
pub mod presence;
mod run;

pub use run::{run, validate_local_url, validate_relay_url, Reporter, ShareError, ShareOptions};
