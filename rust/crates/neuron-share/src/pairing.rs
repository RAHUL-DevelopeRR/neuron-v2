//! The pairing link the QR code carries (`RELAY_CONTRACT.md` §2), built like the apps'
//! `buildPairingLink` (`ui/neuron-mobile/src/relay/pairing.ts`), which parses it:
//!
//! ```text
//! neuron://pair?v=1&r=<relay URL>&h=<host id>&c=<one-time code>&k=<host key>&n=<name>&u=<account tag>&e=<expiry>
//! ```
//!
//! The https form carries the same fields in the fragment, which browsers never send to a server.

use url::form_urlencoded;

pub const PAIRING_VERSION: &str = "1";
pub const WEB_PAIRING_BASE: &str = "https://neuron.zero-x.live/pair";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingPayload {
    /// The relay's WebSocket URL.
    pub relay_url: String,
    pub host_id: String,
    /// The one-time code: 16 bytes, base64url. Never sent to the relay.
    pub code: String,
    /// The computer's X25519 public key: 32 bytes, base64url.
    pub host_key: String,
    pub name: Option<String>,
    /// The account tag from the relay's `joined`.
    pub account_tag: Option<String>,
    /// When the code stops working (ms since the epoch).
    pub expires_at: Option<u64>,
}

fn fields(payload: &PairingPayload) -> String {
    let mut fields = form_urlencoded::Serializer::new(String::new());
    fields
        .append_pair("v", PAIRING_VERSION)
        .append_pair("r", &payload.relay_url)
        .append_pair("h", &payload.host_id)
        .append_pair("c", &payload.code)
        .append_pair("k", &payload.host_key);
    if let Some(name) = payload.name.as_deref().filter(|name| !name.is_empty()) {
        fields.append_pair("n", name);
    }
    if let Some(tag) = payload.account_tag.as_deref().filter(|tag| !tag.is_empty()) {
        fields.append_pair("u", tag);
    }
    if let Some(expires_at) = payload.expires_at {
        fields.append_pair("e", &(expires_at / 1000).to_string());
    }
    fields.finish()
}

/// The `neuron://pair` link the QR code shows.
#[must_use]
pub fn build_pairing_link(payload: &PairingPayload) -> String {
    format!("neuron://pair?{}", fields(payload))
}

/// The same link in its https form, for phones that open it from the camera app.
#[must_use]
pub fn build_pairing_web_link(payload: &PairingPayload) -> String {
    format!("{WEB_PAIRING_BASE}#{}", fields(payload))
}

/// The link as a QR code for a terminal: two rows of modules per line of text.
#[must_use]
pub fn terminal_qr(link: &str) -> Option<String> {
    let code = qrcode::QrCode::new(link.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}
