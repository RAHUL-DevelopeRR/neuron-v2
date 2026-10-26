//! The computer's side of the relay, without any I/O: the port of the apps' reference
//! `ui/neuron-mobile/src/relay/share-host.ts`. The driver (`crate::run`) feeds it what the relay
//! and the local session hosts say and carries out the [`Action`]s it returns.
//!
//! It offers one-time pairing codes, answers pairings (checking the phone's proof), accepts
//! encrypted channels from paired phones and bridges each one to the session's local
//! remote-protocol-1 host. The bridge lets through only what a phone may do: `hello` (with the
//! local host's token put in), `send` (but not trusting the workspace or stopping the engine),
//! `ping`, and the read-only `rpc` methods. Terminals and engine restarts stay with clients on the
//! computer itself. It also keeps each session's sealed presence current and sends a contentless
//! `notify` when a session needs the user or finishes; the relay decides whether to push.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Map, Value};

use crate::channel::{ChannelCrypto, Opened};
use crate::config::{HostIdentity, PairedPhone};
use crate::contract::{HostFrame, NotifyKind, RelayFrame, Role};
use crate::crypto::{
    accept_confirm, b64u, bytes_of, derive_channel_keys, derive_pair_key, key_pair,
    new_pairing_code, pair_id_for, pair_proof, pair_secrets_aad, presence_aad, proof_matches,
    public_key_of, seal_blob, subkey, E2eError,
};
use crate::pairing::PairingPayload;
use crate::presence::{Presence, ShareStatus};

/// The read-only methods a phone may call on the local host.
pub const PHONE_RPC: [&str; 5] = ["fs.list", "fs.read", "fs.search", "git.status", "git.diff"];
/// Engine commands that stay with the computer: trusting the workspace loads its hooks and MCP
/// servers, and the computer's own clients decide when the engine stops.
pub const COMPUTER_ONLY_COMMANDS: [&str; 3] = ["trust_workspace", "shutdown", "capabilities"];
const CLIENT_KINDS: [&str; 4] = ["desktop", "mobile", "web", "cli"];

/// What the driver does for the share host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send this frame to the relay.
    Relay(String),
    /// Connect a client of the share's local host for this channel.
    OpenLocal { ch: String, share_id: String },
    /// Send a frame to the channel's local client.
    LocalSend { ch: String, text: String },
    /// Close the channel's local client (if any).
    CloseLocal { ch: String },
    /// The paired phones changed: save them.
    SavePhones,
    /// A phone paired.
    Paired { name: String },
    /// A pairing code is live; show it.
    OfferReady(PairingPayload),
    /// A pairing code could not be offered.
    OfferFailed(String),
}

struct Bridge {
    phone_id: String,
    share_id: String,
    crypto: ChannelCrypto,
    local_open: bool,
    /// Phone frames that arrived before the local client connected.
    pending: Vec<String>,
}

struct Share {
    /// The local host's token, put into the phone's `hello`. Never leaves the computer.
    token: String,
    presence: Option<Presence>,
    last: String,
    status: Option<ShareStatus>,
}

struct Offer {
    code: String,
    payload: PairingPayload,
}

pub struct ShareHost {
    identity: HostIdentity,
    secret: [u8; 32],
    public: [u8; 32],
    presence_key: [u8; 32],
    phones: BTreeMap<String, PairedPhone>,
    shares: BTreeMap<String, Share>,
    offers: HashMap<String, Offer>,
    bridges: HashMap<String, Bridge>,
    ready: bool,
    account_tag: Option<String>,
}

fn relay(frame: &HostFrame) -> Action {
    Action::Relay(frame.to_json())
}

impl ShareHost {
    pub fn new(identity: HostIdentity, phones: Vec<PairedPhone>) -> Result<Self, E2eError> {
        let secret = bytes_of::<32>(&identity.secret_key, "The host key")?;
        let presence_key = bytes_of::<32>(&identity.presence_key, "The presence key")?;
        Ok(Self {
            public: public_key_of(&secret),
            secret,
            presence_key,
            identity,
            phones: phones
                .into_iter()
                .map(|phone| (phone.phone_id.clone(), phone))
                .collect(),
            shares: BTreeMap::new(),
            offers: HashMap::new(),
            bridges: HashMap::new(),
            ready: false,
            account_tag: None,
        })
    }

    #[must_use]
    pub fn identity(&self) -> &HostIdentity {
        &self.identity
    }

    /// The computer's public key, as the QR code carries it.
    #[must_use]
    pub fn public_key(&self) -> String {
        b64u(&self.public)
    }

    #[must_use]
    pub fn phones(&self) -> Vec<PairedPhone> {
        self.phones.values().cloned().collect()
    }

    /// Channels attached right now.
    #[must_use]
    pub fn attached(&self) -> usize {
        self.bridges.len()
    }

    /// The relay said `joined`: share every session again (the relay forgot them).
    pub fn relay_joined(&mut self, account_tag: Option<String>) -> Vec<Action> {
        self.ready = true;
        self.account_tag = account_tag;
        let mut actions = Vec::new();
        let ids: Vec<String> = self.shares.keys().cloned().collect();
        for share_id in ids {
            if let Some(share) = self.shares.get_mut(&share_id) {
                share.last.clear();
            }
            actions.extend(self.send_presence(&share_id));
        }
        actions
    }

    /// The relay socket dropped. Channels do not survive this end's reconnect: phones attach
    /// again, and codes not yet used are gone with the socket.
    pub fn relay_lost(&mut self) -> Vec<Action> {
        self.ready = false;
        let mut actions = Vec::new();
        if !self.offers.is_empty() {
            self.offers.clear();
            actions.push(Action::OfferFailed(
                "The relay connection dropped before the code was used.".into(),
            ));
        }
        let channels: Vec<String> = self.bridges.keys().cloned().collect();
        for ch in channels {
            actions.extend(self.end_bridge(&ch, false));
        }
        actions
    }

    /// Starts sharing a session; its presence goes out once known.
    pub fn share(&mut self, share_id: &str, token: &str) -> Vec<Action> {
        self.shares.insert(
            share_id.to_string(),
            Share {
                token: token.to_string(),
                presence: None,
                last: String::new(),
                status: None,
            },
        );
        self.send_presence(share_id)
    }

    pub fn unshare(&mut self, share_id: &str) -> Vec<Action> {
        if self.shares.remove(share_id).is_none() {
            return Vec::new();
        }
        let mut actions = Vec::new();
        let channels: Vec<String> = self
            .bridges
            .iter()
            .filter(|(_, bridge)| bridge.share_id == share_id)
            .map(|(ch, _)| ch.clone())
            .collect();
        for ch in channels {
            actions.extend(self.end_bridge(&ch, true));
        }
        if self.ready {
            actions.push(relay(&HostFrame::Unshare {
                share_id: share_id.to_string(),
            }));
        }
        actions
    }

    /// The session changed: send a new presence blob if it differs, and `notify` when it starts
    /// waiting for the user or a turn ends.
    pub fn update_presence(&mut self, share_id: &str, presence: Presence) -> Vec<Action> {
        let Some(share) = self.shares.get_mut(share_id) else {
            return Vec::new();
        };
        share.presence = Some(presence);
        self.send_presence(share_id)
    }

    fn send_presence(&mut self, share_id: &str) -> Vec<Action> {
        let ready = self.ready;
        let host_id = self.identity.host_id.clone();
        let presence_key = self.presence_key;
        let Some(share) = self.shares.get_mut(share_id) else {
            return Vec::new();
        };
        let Some(presence) = share.presence.as_ref() else {
            return Vec::new();
        };
        if !ready {
            return Vec::new();
        }
        let mut actions = Vec::new();
        let fingerprint = presence.fingerprint();
        if fingerprint != share.last {
            share.last = fingerprint;
            let blob = serde_json::to_string(presence).expect("presence serializes");
            actions.push(relay(&HostFrame::Share {
                share_id: share_id.to_string(),
                sealed: Some(seal_blob(
                    &presence_key,
                    &presence_aad(&host_id, share_id),
                    &blob,
                )),
            }));
        }
        // A session that needs the user, or a turn that ended: the relay may push.
        let previous = share.status.replace(presence.status);
        let kind = match (previous, presence.status) {
            (previous, ShareStatus::NeedsMe) if previous != Some(ShareStatus::NeedsMe) => {
                Some(NotifyKind::NeedsMe)
            }
            (Some(ShareStatus::Running), ShareStatus::Idle | ShareStatus::Failed) => {
                Some(NotifyKind::Finished)
            }
            _ => None,
        };
        if let Some(kind) = kind {
            actions.push(relay(&HostFrame::Notify {
                share_id: share_id.to_string(),
                kind,
            }));
        }
        actions
    }

    /// A one-time pairing code for the QR, valid for `ttl_s` seconds once the relay confirms it.
    pub fn offer_pairing(&mut self, relay_url: &str, ttl_s: u64) -> Vec<Action> {
        if !self.ready {
            return vec![Action::OfferFailed("Not connected to the relay.".into())];
        }
        let code = new_pairing_code();
        let pair_id = pair_id_for(&code).expect("a new code is 16 bytes");
        let payload = PairingPayload {
            relay_url: relay_url.to_string(),
            host_id: self.identity.host_id.clone(),
            code: code.clone(),
            host_key: self.public_key(),
            name: Some(self.identity.name.clone()).filter(|name| !name.is_empty()),
            account_tag: self.account_tag.clone(),
            expires_at: None,
        };
        self.offers.insert(pair_id.clone(), Offer { code, payload });
        vec![relay(&HostFrame::PairOffer { pair_id, ttl_s })]
    }

    /// Forgets a phone: it can no longer attach.
    pub fn forget_phone(&mut self, phone_id: &str) -> Vec<Action> {
        if self.phones.remove(phone_id).is_none() {
            return Vec::new();
        }
        let mut actions = vec![Action::SavePhones];
        let channels: Vec<String> = self
            .bridges
            .iter()
            .filter(|(_, bridge)| bridge.phone_id == phone_id)
            .map(|(ch, _)| ch.clone())
            .collect();
        for ch in channels {
            actions.extend(self.end_bridge(&ch, true));
        }
        actions
    }

    /// Ends every channel and stops sharing (the command is quitting).
    pub fn stop(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let ids: Vec<String> = self.shares.keys().cloned().collect();
        for share_id in ids {
            actions.extend(self.unshare(&share_id));
        }
        actions
    }

    /// A frame from the relay.
    pub fn relay_frame(&mut self, frame: RelayFrame, now_ms: u64) -> Vec<Action> {
        match frame {
            RelayFrame::Joined { account } => self.relay_joined(account.map(|account| account.tag)),
            RelayFrame::Pong | RelayFrame::Peer { .. } => Vec::new(),
            RelayFrame::PairOfferOk {
                pair_id,
                expires_at,
            } => match self.offers.get_mut(&pair_id) {
                Some(offer) => {
                    offer.payload.expires_at = Some(expires_at.max(now_ms));
                    vec![Action::OfferReady(offer.payload.clone())]
                }
                None => Vec::new(),
            },
            RelayFrame::Error {
                message,
                ch,
                pair_id,
                ..
            } => {
                let mut actions = Vec::new();
                if let Some(pair_id) = pair_id {
                    if self.offers.remove(&pair_id).is_some() {
                        actions.push(Action::OfferFailed(message));
                    }
                }
                if let Some(ch) = ch {
                    actions.extend(self.end_bridge(&ch, false));
                }
                actions
            }
            RelayFrame::Pair {
                pair_id,
                phone_id,
                public,
                proof,
                name,
            } => self.pair(&pair_id, &phone_id, &public, &proof, &name, now_ms),
            RelayFrame::Open {
                ch,
                phone_id,
                share_id,
                e,
            } => self.open(&ch, &phone_id, &share_id, &e),
            RelayFrame::Data { ch, n, c } => self.data(&ch, n, &c),
            RelayFrame::Close { ch, .. } => self.end_bridge(&ch, false),
            RelayFrame::Unpaired { phone_id } => self.forget_phone(&phone_id),
        }
    }

    fn pair(
        &mut self,
        pair_id: &str,
        phone_id: &str,
        phone_public: &str,
        proof: &str,
        name: &str,
        now_ms: u64,
    ) -> Vec<Action> {
        let deny = |code: &str| {
            vec![relay(&HostFrame::PairDenied {
                pair_id: pair_id.to_string(),
                phone_id: phone_id.to_string(),
                code: code.to_string(),
            })]
        };
        let Some(offer) = self.offers.get(pair_id) else {
            return deny("pair_not_found");
        };
        let Ok(pair_key) = bytes_of::<32>(phone_public, "The phone's key").and_then(|phone| {
            derive_pair_key(&self.secret, &phone, &offer.code, &self.public, &phone)
        }) else {
            return deny("bad_proof");
        };
        if !proof_matches(&pair_proof(&pair_key, Role::Phone, pair_id), proof) {
            return deny("bad_proof");
        }
        // One phone per code: the code is spent.
        self.offers.remove(pair_id);
        let name = if name.is_empty() { "phone" } else { name };
        let secrets =
            json!({ "presence_key": b64u(&self.presence_key), "name": self.identity.name });
        let phone = PairedPhone {
            phone_id: phone_id.to_string(),
            name: name.to_string(),
            pair_key: b64u(&pair_key),
            paired_at: now_ms,
        };
        self.phones.insert(phone.phone_id.clone(), phone);
        let mut actions = vec![
            Action::SavePhones,
            relay(&HostFrame::Paired {
                pair_id: pair_id.to_string(),
                phone_id: phone_id.to_string(),
                proof: pair_proof(&pair_key, Role::Host, pair_id),
                sealed: seal_blob(
                    &subkey(&pair_key, "pair-secrets"),
                    &pair_secrets_aad(pair_id),
                    &secrets.to_string(),
                ),
            }),
            Action::Paired {
                name: name.to_string(),
            },
        ];
        // The new phone lists this computer's sessions next: make sure the relay has them.
        let ids: Vec<String> = self.shares.keys().cloned().collect();
        for share_id in ids {
            if let Some(share) = self.shares.get_mut(&share_id) {
                share.last.clear();
            }
            actions.extend(self.send_presence(&share_id));
        }
        actions
    }

    fn open(
        &mut self,
        ch: &str,
        phone_id: &str,
        share_id: &str,
        phone_ephemeral: &str,
    ) -> Vec<Action> {
        let reject = |code: &str| {
            vec![relay(&HostFrame::Reject {
                ch: ch.to_string(),
                code: code.to_string(),
            })]
        };
        let Some(phone) = self.phones.get(phone_id) else {
            return reject("not_paired");
        };
        if !self.shares.contains_key(share_id) {
            return reject("unknown_share");
        }
        if self.bridges.contains_key(ch) {
            return reject("bad_frame");
        }
        let ephemeral = key_pair();
        let keys =
            bytes_of::<32>(phone_ephemeral, "The phone's channel key").and_then(|phone_key| {
                let pair_key = bytes_of::<32>(&phone.pair_key, "The pair key")?;
                derive_channel_keys(
                    &ephemeral.secret,
                    &phone_key,
                    &pair_key,
                    &phone_key,
                    &ephemeral.public,
                    ch,
                )
            });
        let Ok(keys) = keys else {
            return reject("bad_key");
        };
        let confirm = accept_confirm(&keys, ch);
        self.bridges.insert(
            ch.to_string(),
            Bridge {
                phone_id: phone_id.to_string(),
                share_id: share_id.to_string(),
                crypto: ChannelCrypto::host(ch, keys),
                local_open: false,
                pending: Vec::new(),
            },
        );
        vec![
            relay(&HostFrame::Accept {
                ch: ch.to_string(),
                e: b64u(&ephemeral.public),
                confirm,
            }),
            Action::OpenLocal {
                ch: ch.to_string(),
                share_id: share_id.to_string(),
            },
        ]
    }

    fn data(&mut self, ch: &str, n: u64, c: &str) -> Vec<Action> {
        let Some(bridge) = self.bridges.get_mut(ch) else {
            return Vec::new();
        };
        match bridge.crypto.open(n, c) {
            // Not from the paired phone: end the channel rather than guess.
            Err(_) => self.end_bridge(ch, true),
            Ok(Opened::Frame(text)) => {
                if bridge.local_open {
                    self.pass_to_local(ch, &text)
                } else {
                    bridge.pending.push(text);
                    Vec::new()
                }
            }
            Ok(Opened::Duplicate | Opened::Gap { .. }) => Vec::new(),
        }
    }

    /// The channel's local client connected: pass on what the phone already sent.
    pub fn local_opened(&mut self, ch: &str) -> Vec<Action> {
        let Some(bridge) = self.bridges.get_mut(ch) else {
            return vec![Action::CloseLocal { ch: ch.to_string() }];
        };
        bridge.local_open = true;
        let pending = std::mem::take(&mut bridge.pending);
        pending
            .iter()
            .flat_map(|text| self.pass_to_local(ch, text))
            .collect()
    }

    /// The local host sent a frame for the phone: seal it.
    pub fn local_message(&mut self, ch: &str, text: &str) -> Vec<Action> {
        let Some(bridge) = self.bridges.get_mut(ch) else {
            return Vec::new();
        };
        let (n, c) = bridge.crypto.seal(text);
        vec![relay(&HostFrame::Data {
            ch: ch.to_string(),
            n,
            c,
        })]
    }

    /// The local client went away (the host stopped, or refused it).
    pub fn local_closed(&mut self, ch: &str) -> Vec<Action> {
        self.end_bridge(ch, true)
    }

    /// Passes one phone frame to the local host, if a phone may send it.
    fn pass_to_local(&mut self, ch: &str, text: &str) -> Vec<Action> {
        let Some(share_id) = self.bridges.get(ch).map(|bridge| bridge.share_id.clone()) else {
            return Vec::new();
        };
        let token = self
            .shares
            .get(&share_id)
            .map(|share| share.token.clone())
            .unwrap_or_default();
        let send = |frame: Value| {
            vec![Action::LocalSend {
                ch: ch.to_string(),
                text: frame.to_string(),
            }]
        };
        let Ok(Value::Object(frame)) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        match frame.get("t").and_then(Value::as_str) {
            Some("hello") => {
                let Some(protocol) = frame.get("protocol").and_then(Value::as_i64) else {
                    return Vec::new();
                };
                if frame.get("token").and_then(Value::as_str).is_none() {
                    return Vec::new();
                }
                let client = frame.get("client").cloned().unwrap_or(Value::Null);
                let name: String = client
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("client")
                    .chars()
                    .take(80)
                    .collect();
                let kind = client
                    .get("kind")
                    .and_then(Value::as_str)
                    .filter(|kind| CLIENT_KINDS.contains(kind))
                    .unwrap_or("web");
                send(
                    json!({ "t": "hello", "protocol": protocol, "token": token, "client": { "name": name, "kind": kind } }),
                )
            }
            Some("ping") => send(json!({ "t": "ping" })),
            Some("send") => match phone_message(frame.get("message")) {
                Some(message) => send(json!({ "t": "send", "message": message })),
                None => self.reply_error(ch, "Only the computer running Neuron can do that."),
            },
            Some("rpc") => {
                let id = frame.get("id").and_then(Value::as_i64);
                let method = frame.get("method").and_then(Value::as_str);
                let (Some(id), Some(method)) = (id, method) else {
                    return Vec::new();
                };
                if PHONE_RPC.contains(&method) {
                    let params = frame
                        .get("params")
                        .filter(|params| params.is_object())
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Map::new()));
                    send(json!({ "t": "rpc", "id": id, "method": method, "params": params }))
                } else {
                    self.reply(ch, &json!({ "t": "rpc_result", "id": id, "error": "Not available from the phone." }))
                }
            }
            // Terminal frames are for clients on this computer only.
            _ => Vec::new(),
        }
    }

    fn reply_error(&mut self, ch: &str, message: &str) -> Vec<Action> {
        self.reply(
            ch,
            &json!({ "t": "error", "message": message, "fatal": false }),
        )
    }

    fn reply(&mut self, ch: &str, frame: &Value) -> Vec<Action> {
        self.local_message(ch, &frame.to_string())
    }

    fn end_bridge(&mut self, ch: &str, tell_relay: bool) -> Vec<Action> {
        if self.bridges.remove(ch).is_none() {
            return Vec::new();
        }
        let mut actions = Vec::new();
        if tell_relay && self.ready {
            actions.push(relay(&HostFrame::Close {
                ch: ch.to_string(),
                code: None,
            }));
        }
        actions.push(Action::CloseLocal { ch: ch.to_string() });
        actions
    }
}

/// A `send` message a phone may pass on, or `None`. The local host checks its shape again.
fn phone_message(message: Option<&Value>) -> Option<Value> {
    let message = message?.as_object()?;
    match message.get("kind").and_then(Value::as_str)? {
        "permission_reply" => Some(Value::Object(message.clone())),
        "command" => {
            let command = message.get("type").and_then(Value::as_str)?;
            (!COMPUTER_ONLY_COMMANDS.contains(&command)).then(|| Value::Object(message.clone()))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::parse_relay_frame;
    use crate::crypto::{open_blob, open_frame, seal_frame, ChannelKeys, Direction};

    struct Phone {
        id: String,
        pair_key: [u8; 32],
    }

    fn frames(actions: &[Action]) -> Vec<Value> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Relay(text) => Some(serde_json::from_str(text).unwrap()),
                _ => None,
            })
            .collect()
    }

    fn feed(host: &mut ShareHost, frame: &Value) -> Vec<Action> {
        host.relay_frame(parse_relay_frame(&frame.to_string()).unwrap(), 1_000)
    }

    fn paired_host() -> (ShareHost, Phone) {
        let mut host = ShareHost::new(HostIdentity::generate("Studio"), Vec::new()).unwrap();
        assert_eq!(
            host.offer_pairing("wss://relay.example/v1/relay", 600),
            vec![Action::OfferFailed("Not connected to the relay.".into())]
        );
        let host_id = host.identity().host_id.clone();
        feed(
            &mut host,
            &json!({ "t": "joined", "relay": 1, "role": "host", "device_id": host_id, "account": { "tag": "0123456789abcdef" } }),
        );
        let offer = frames(&host.offer_pairing("wss://relay.example/v1/relay", 600));
        let pair_id = offer[0]["pair_id"].as_str().unwrap().to_string();
        let ready = feed(
            &mut host,
            &json!({ "t": "pair_offer_ok", "pair_id": pair_id, "expires_at": 600_000 }),
        );
        let Action::OfferReady(payload) = &ready[0] else {
            panic!("{ready:?}")
        };
        assert_eq!(payload.account_tag.as_deref(), Some("0123456789abcdef"));
        assert_eq!(payload.host_key, host.public_key());

        // The phone's side, from the QR's fields.
        let phone_keys = key_pair();
        let host_key = bytes_of::<32>(&payload.host_key, "key").unwrap();
        let pair_key = derive_pair_key(
            &phone_keys.secret,
            &host_key,
            &payload.code,
            &host_key,
            &phone_keys.public,
        )
        .unwrap();
        let bad = feed(
            &mut host,
            &json!({ "t": "pair", "pair_id": pair_id, "phone_id": "phone-0001", "pub": b64u(&phone_keys.public), "proof": "AAAAAAAAAAAAAAAAAAAAAA", "name": "Pixel" }),
        );
        assert_eq!(frames(&bad)[0]["code"], "bad_proof");
        let actions = feed(
            &mut host,
            &json!({ "t": "pair", "pair_id": pair_id, "phone_id": "phone-0001", "pub": b64u(&phone_keys.public), "proof": pair_proof(&pair_key, Role::Phone, &pair_id), "name": "Pixel" }),
        );
        assert!(actions.contains(&Action::SavePhones));
        assert!(actions.contains(&Action::Paired {
            name: "Pixel".into()
        }));
        let paired = &frames(&actions)[0];
        assert_eq!(paired["t"], "paired");
        assert_eq!(paired["proof"], pair_proof(&pair_key, Role::Host, &pair_id));
        let secrets: Value = serde_json::from_str(
            &open_blob(
                &subkey(&pair_key, "pair-secrets"),
                &pair_secrets_aad(&pair_id),
                paired["sealed"].as_str().unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(secrets["name"], "Studio");
        let again = feed(
            &mut host,
            &json!({ "t": "pair", "pair_id": pair_id, "phone_id": "phone-0002", "pub": b64u(&phone_keys.public), "proof": pair_proof(&pair_key, Role::Phone, &pair_id), "name": "x" }),
        );
        assert_eq!(
            frames(&again)[0]["code"],
            "pair_not_found",
            "a code works once"
        );
        (
            host,
            Phone {
                id: "phone-0001".into(),
                pair_key,
            },
        )
    }

    fn presence(status: ShareStatus) -> Presence {
        Presence {
            v: 1,
            project: "neuron".into(),
            title: None,
            branch: None,
            model: None,
            status,
            last_seq: 1,
            updated_at_ms: 5,
        }
    }

    // Opens a channel from the phone; returns its id and keys.
    fn attach(host: &mut ShareHost, phone: &Phone, share_id: &str) -> (String, ChannelKeys) {
        let ch = "channel-0000000001".to_string();
        let ephemeral = key_pair();
        let actions = feed(
            host,
            &json!({ "t": "open", "ch": ch, "phone_id": phone.id, "share_id": share_id, "e": b64u(&ephemeral.public) }),
        );
        let accept = &frames(&actions)[0];
        assert_eq!(accept["t"], "accept");
        assert!(actions.contains(&Action::OpenLocal {
            ch: ch.clone(),
            share_id: share_id.into()
        }));
        let host_ephemeral = bytes_of::<32>(accept["e"].as_str().unwrap(), "e").unwrap();
        let keys = derive_channel_keys(
            &ephemeral.secret,
            &host_ephemeral,
            &phone.pair_key,
            &ephemeral.public,
            &host_ephemeral,
            &ch,
        )
        .unwrap();
        assert_eq!(accept["confirm"], accept_confirm(&keys, &ch));
        (ch, keys)
    }

    #[test]
    #[allow(clippy::too_many_lines)] // one channel, frame by frame
    fn pairs_and_bridges_only_what_a_phone_may_send() {
        let (mut host, phone) = paired_host();
        assert!(
            frames(&host.share("share-0001", "local-token")).is_empty(),
            "no presence yet"
        );
        let shared = frames(&host.update_presence("share-0001", presence(ShareStatus::Running)));
        assert_eq!(shared[0]["t"], "share");
        assert!(
            frames(&host.update_presence(
                "share-0001",
                Presence {
                    updated_at_ms: 9,
                    ..presence(ShareStatus::Running)
                }
            ))
            .is_empty(),
            "the time alone is no change"
        );
        let unknown = feed(
            &mut host,
            &json!({ "t": "open", "ch": "channel-0000000009", "phone_id": "phone-9999", "share_id": "share-0001", "e": b64u(&[9; 32]) }),
        );
        assert_eq!(
            frames(&unknown)[0],
            json!({ "t": "reject", "ch": "channel-0000000009", "code": "not_paired" })
        );

        let (ch, keys) = attach(&mut host, &phone, "share-0001");
        let mut n = 0;
        let mut say = |host: &mut ShareHost, frame: Value| {
            let c = seal_frame(&keys.p2h, &ch, Direction::P2h, n, &frame.to_string());
            n += 1;
            feed(host, &json!({ "t": "data", "ch": ch, "n": n - 1, "c": c }))
        };
        // Before the local client connects, frames wait.
        assert_eq!(
            say(
                &mut host,
                json!({ "t": "hello", "protocol": 1, "token": "", "client": { "name": "Pixel", "kind": "mobile" } })
            ),
            Vec::<Action>::new()
        );
        let opened = host.local_opened(&ch);
        let Action::LocalSend { text, .. } = &opened[0] else {
            panic!("{opened:?}")
        };
        let hello: Value = serde_json::from_str(text).unwrap();
        assert_eq!(hello["token"], "local-token", "the local token is put in");
        assert_eq!(hello["client"]["kind"], "mobile");

        let local = |actions: Vec<Action>| -> Vec<Value> {
            actions
                .into_iter()
                .filter_map(|action| match action {
                    Action::LocalSend { text, .. } => Some(serde_json::from_str(&text).unwrap()),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            local(say(&mut host, json!({ "t": "ping" }))),
            vec![json!({ "t": "ping" })]
        );
        let submit = json!({ "t": "send", "message": { "kind": "command", "type": "submit", "text": "fix it", "attachments": [] } });
        assert_eq!(local(say(&mut host, submit.clone())), vec![submit]);
        let reply = json!({ "t": "send", "message": { "kind": "permission_reply", "id": 3, "decision": { "kind": "deny", "note": "not that" } } });
        assert_eq!(local(say(&mut host, reply.clone())), vec![reply]);
        assert_eq!(
            local(say(
                &mut host,
                json!({ "t": "rpc", "id": 1, "method": "git.diff", "params": { "path": "a.rs" } })
            ))
            .len(),
            1
        );
        for refused in [
            json!({ "t": "rpc", "id": 2, "method": "term.open", "params": {} }),
            json!({ "t": "rpc", "id": 3, "method": "engine.restart" }),
            json!({ "t": "send", "message": { "kind": "command", "type": "trust_workspace" } }),
            json!({ "t": "send", "message": { "kind": "command", "type": "shutdown" } }),
            json!({ "t": "term_input", "id": 1, "data": "rm -rf /\n" }),
        ] {
            let actions = say(&mut host, refused.clone());
            assert!(
                local(actions.clone()).is_empty(),
                "{refused} never reaches the local host"
            );
            // rpc and send refusals are answered on the channel, sealed.
            for frame in frames(&actions) {
                let text = open_frame(
                    &keys.h2p,
                    &ch,
                    Direction::H2p,
                    frame["n"].as_u64().unwrap(),
                    frame["c"].as_str().unwrap(),
                )
                .unwrap();
                assert!(
                    text.contains("rpc_result") || text.contains("Only the computer"),
                    "{text}"
                );
            }
        }

        // The local host's frames go to the phone sealed, counted from where the replies left off.
        let out = frames(&host.local_message(&ch, r#"{"t":"pong"}"#));
        let n_out = out[0]["n"].as_u64().unwrap();
        assert_eq!(
            open_frame(
                &keys.h2p,
                &ch,
                Direction::H2p,
                n_out,
                out[0]["c"].as_str().unwrap()
            )
            .unwrap(),
            r#"{"t":"pong"}"#
        );

        // A frame that does not open ends the channel.
        let forged = feed(
            &mut host,
            &json!({ "t": "data", "ch": ch, "n": n, "c": seal_frame(&[0; 32], &ch, Direction::P2h, n, "{}") }),
        );
        assert_eq!(frames(&forged)[0], json!({ "t": "close", "ch": ch }));
        assert!(forged.contains(&Action::CloseLocal { ch: ch.clone() }));
        assert_eq!(host.attached(), 0);
    }

    #[test]
    fn presence_and_notify_follow_the_session() {
        let (mut host, phone) = paired_host();
        host.share("share-0001", "token");
        let notify = |actions: &[Action]| {
            frames(actions)
                .into_iter()
                .filter(|frame| frame["t"] == "notify")
                .map(|frame| frame["kind"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            notify(&host.update_presence("share-0001", presence(ShareStatus::Running))),
            Vec::<String>::new()
        );
        assert_eq!(
            notify(&host.update_presence("share-0001", presence(ShareStatus::NeedsMe))),
            vec!["needs_me"]
        );
        assert!(
            notify(&host.update_presence("share-0001", presence(ShareStatus::NeedsMe))).is_empty(),
            "once per wait"
        );
        assert_eq!(
            notify(&host.update_presence("share-0001", presence(ShareStatus::Running))),
            Vec::<String>::new()
        );
        assert_eq!(
            notify(&host.update_presence("share-0001", presence(ShareStatus::Idle))),
            vec!["finished"]
        );
        // A dropped relay ends channels locally; joining again shares again.
        attach(&mut host, &phone, "share-0001");
        let lost = host.relay_lost();
        assert!(
            frames(&lost).is_empty(),
            "nothing is sent to a relay that is gone"
        );
        assert!(lost.contains(&Action::CloseLocal {
            ch: "channel-0000000001".into()
        }));
        assert!(
            frames(&host.update_presence("share-0001", presence(ShareStatus::Running))).is_empty(),
            "nothing while disconnected"
        );
        let rejoined = frames(&feed(&mut host, &json!({ "t": "joined", "relay": 1 })));
        assert_eq!(rejoined[0]["t"], "share");
        assert_eq!(rejoined[0]["share_id"], "share-0001");
        // Unpaired from the phone: it can no longer attach.
        assert_eq!(
            feed(&mut host, &json!({ "t": "unpaired", "phone_id": phone.id })),
            vec![Action::SavePhones]
        );
        let refused = feed(
            &mut host,
            &json!({ "t": "open", "ch": "channel-0000000002", "phone_id": phone.id, "share_id": "share-0001", "e": b64u(&key_pair().public) }),
        );
        assert_eq!(frames(&refused)[0]["code"], "not_paired");
        assert_eq!(
            frames(&host.stop()),
            vec![json!({ "t": "unshare", "share_id": "share-0001" })]
        );
    }
}
