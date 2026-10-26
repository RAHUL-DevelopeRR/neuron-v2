//! The relay protocol, `neuron-relay/1` (`RELAY_CONTRACT.md` §4): JSON text frames over one
//! WebSocket per device. These are the computer's frames; the shapes match the apps'
//! `ui/neuron-mobile/src/relay/contract.ts`. What a session says travels sealed in `data.c`.

use serde::{Deserialize, Serialize};

pub const RELAY_PROTOCOL: u32 = 1;
/// Offered as a WebSocket subprotocol; the relay answers with it.
pub const RELAY_SUBPROTOCOL: &str = "neuron-relay.v1";
/// Zero-X's relay.
pub const DEFAULT_RELAY_URL: &str = "wss://relay.zero-x.live/v1/relay";
/// Keepalive: the exact text, so the relay answers without waking (Durable Object auto-response).
pub const PING: &str = r#"{"t":"ping"}"#;

/// WebSocket close codes the relay uses.
pub mod close {
    /// The first frame was not a valid `join`, or a frame broke the protocol.
    pub const PROTOCOL: u16 = 4400;
    /// No session token, or it is expired or revoked: sign in again.
    pub const UNAUTHORIZED: u16 = 4401;
    /// The credential may not use the relay (API keys), or the account is suspended.
    pub const FORBIDDEN: u16 = 4403;
    /// The same device joined again elsewhere; this socket is replaced.
    pub const REPLACED: u16 = 4409;
    /// Over a limit (devices, frame rate, frame size).
    pub const LIMITED: u16 = 4429;
}

/// Close codes after which reconnecting cannot help until the user acts.
#[must_use]
pub fn is_fatal_close(code: u16) -> bool {
    matches!(
        code,
        close::PROTOCOL | close::UNAUTHORIZED | close::FORBIDDEN | close::REPLACED
    )
}

/// What a fatal close code tells the user.
#[must_use]
pub fn fatal_close_message(code: u16) -> &'static str {
    match code {
        close::UNAUTHORIZED => {
            "The relay needs a Zero-X sign-in: run `neuron login`, then share again."
        }
        close::FORBIDDEN => {
            "This Zero-X credential may not use the relay (API keys cannot share sessions)."
        }
        close::REPLACED => "This computer connected to the relay again from another process.",
        _ => "The relay did not accept this version of Neuron: update it.",
    }
}

/// Relay ids: 8-64 of `[A-Za-z0-9_-]`.
#[must_use]
pub fn is_relay_id(value: &str) -> bool {
    (8..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Phone,
}

impl Role {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Phone => "phone",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub platform: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotifyKind {
    NeedsMe,
    Finished,
}

/// Frames the computer sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HostFrame {
    Join {
        role: Role,
        device: DeviceInfo,
    },
    Share {
        share_id: String,
        sealed: Option<String>,
    },
    Unshare {
        share_id: String,
    },
    PairOffer {
        pair_id: String,
        ttl_s: u64,
    },
    PairCancel {
        pair_id: String,
    },
    Paired {
        pair_id: String,
        phone_id: String,
        proof: String,
        sealed: String,
    },
    PairDenied {
        pair_id: String,
        phone_id: String,
        code: String,
    },
    Accept {
        ch: String,
        e: String,
        confirm: String,
    },
    Reject {
        ch: String,
        code: String,
    },
    Data {
        ch: String,
        n: u64,
        c: String,
    },
    Close {
        ch: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    Notify {
        share_id: String,
        kind: NotifyKind,
    },
}

impl HostFrame {
    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("relay frames serialize")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountTag {
    pub tag: String,
}

/// Frames the relay sends to a computer. Anything else (a newer relay's frames) is ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum RelayFrame {
    Joined {
        #[serde(default)]
        account: Option<AccountTag>,
    },
    Pong,
    Error {
        code: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        ch: Option<String>,
        #[serde(default)]
        pair_id: Option<String>,
    },
    PairOfferOk {
        pair_id: String,
        expires_at: u64,
    },
    Pair {
        pair_id: String,
        phone_id: String,
        #[serde(rename = "pub")]
        public: String,
        proof: String,
        #[serde(default)]
        name: String,
    },
    Open {
        ch: String,
        phone_id: String,
        share_id: String,
        e: String,
    },
    Data {
        ch: String,
        n: u64,
        c: String,
    },
    Close {
        ch: String,
        #[serde(default)]
        code: Option<String>,
    },
    Peer {
        ch: String,
        state: String,
    },
    Unpaired {
        phone_id: String,
    },
}

/// Reads a frame from the relay; `None` for anything this computer does not act on.
#[must_use]
pub fn parse_relay_frame(text: &str) -> Option<RelayFrame> {
    serde_json::from_str(text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_the_contract_shapes() {
        assert_eq!(
            HostFrame::Join {
                role: Role::Host,
                device: DeviceInfo {
                    id: "host-0001".into(),
                    name: "Studio".into(),
                    platform: "linux".into()
                }
            }
            .to_json(),
            r#"{"t":"join","role":"host","device":{"id":"host-0001","name":"Studio","platform":"linux"}}"#
        );
        assert_eq!(
            HostFrame::Close {
                ch: "channel-1".into(),
                code: None
            }
            .to_json(),
            r#"{"t":"close","ch":"channel-1"}"#
        );
        assert_eq!(
            HostFrame::Notify {
                share_id: "share-001".into(),
                kind: NotifyKind::NeedsMe
            }
            .to_json(),
            r#"{"t":"notify","share_id":"share-001","kind":"needs_me"}"#
        );
        assert_eq!(
            HostFrame::Share {
                share_id: "share-001".into(),
                sealed: None
            }
            .to_json(),
            r#"{"t":"share","share_id":"share-001","sealed":null}"#
        );
        assert_eq!(
            parse_relay_frame(
                r#"{"t":"pair","pair_id":"p","phone_id":"phone-001","pub":"k","proof":"x","name":"Pixel"}"#
            ),
            Some(RelayFrame::Pair {
                pair_id: "p".into(),
                phone_id: "phone-001".into(),
                public: "k".into(),
                proof: "x".into(),
                name: "Pixel".into()
            })
        );
        assert_eq!(
            parse_relay_frame(
                r#"{"t":"joined","relay":1,"role":"host","device_id":"host-0001","account":{"tag":"0123456789abcdef"}}"#
            ),
            Some(RelayFrame::Joined {
                account: Some(AccountTag {
                    tag: "0123456789abcdef".into()
                })
            })
        );
        assert_eq!(parse_relay_frame(r#"{"t":"something_new"}"#), None);
        assert!(is_relay_id("host-0001") && !is_relay_id("short") && !is_relay_id("bad id here"));
        assert!(is_fatal_close(4401) && !is_fatal_close(4429) && !is_fatal_close(1006));
    }
}
