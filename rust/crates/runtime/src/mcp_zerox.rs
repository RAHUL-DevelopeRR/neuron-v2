//! Hosted connectors through Zero-X (`"type": "zerox"` servers).
//!
//! The gateway serves each connector the user linked on the Zero-X dashboard as a
//! Streamable HTTP MCP endpoint at `<gateway origin>/mcp/<connector id>` (contract
//! v1.3). Neuron reaches it with the Zero-X sign-in (a `ses_` session or an `nzx_`
//! key) as the bearer, never a per-connector token, so no third-party token sits on
//! the machine. The gateway answers:
//!
//! - `401`: the sign-in is gone; the user signs in again.
//! - `403` with `{"error": "needs_connect", "connect_url": ...}`: the user has not
//!   linked that connector; the server's status is `needs_connect` until they do.
//! - a JSON-RPC error for a destructive-tier tool call that does not carry
//!   `x-neuron-confirmed: 1`, which Neuron sends only on calls the user approved in
//!   the permission prompt.
//!
//! Hosted connectors are off until `NEURON_HOSTED_CONNECTORS=1` (nothing is deployed
//! yet); while off, `zerox` servers are listed as unsupported and never contacted.
//!
//! The sign-in lives in the CLI, so the CLI installs a [`set_zerox_token_provider`]
//! hook; the bearer only ever goes to the gateway's own origin.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use reqwest::Url;
use serde_json::Value;

/// Turns hosted connectors on (`1`, `true`, `yes` or `on`).
pub const ZEROX_HOSTED_CONNECTORS_ENV: &str = "NEURON_HOSTED_CONNECTORS";
/// The gateway when `NEURON_API_BASE` does not name another one.
pub const ZEROX_DEFAULT_ORIGIN: &str = "https://zero-x.live";
/// Where a user links connectors, on the gateway's origin.
pub const ZEROX_CONNECT_PATH: &str = "/dashboard#connectors";
/// Sent on a `tools/call` the user approved in the permission prompt.
pub const ZEROX_CONFIRMED_HEADER: &str = "x-neuron-confirmed";

/// The Zero-X sign-in to send as the bearer, or why there is none.
pub type ZeroxTokenProvider = Arc<dyn Fn() -> Result<String, String> + Send + Sync>;

static TOKEN_PROVIDER: RwLock<Option<ZeroxTokenProvider>> = RwLock::new(None);
/// Hosted servers the gateway said are not linked, with where to link them.
static NEEDS_CONNECT: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());
#[cfg(test)]
static TEST_ORIGIN: RwLock<Option<String>> = RwLock::new(None);

/// Whether hosted connectors are on (`NEURON_HOSTED_CONNECTORS`; off by default).
#[must_use]
pub fn hosted_connectors_enabled() -> bool {
    std::env::var(ZEROX_HOSTED_CONNECTORS_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Why a `zerox` server is not started while hosted connectors are off.
#[must_use]
pub fn hosted_connectors_off_reason() -> String {
    format!(
        "hosted connectors are off in this build; set {ZEROX_HOSTED_CONNECTORS_ENV}=1 to try them"
    )
}

/// Installs (or, with `None`, removes) where hosted connectors get the Zero-X sign-in.
pub fn set_zerox_token_provider(provider: Option<ZeroxTokenProvider>) {
    *TOKEN_PROVIDER
        .write()
        .unwrap_or_else(PoisonError::into_inner) = provider;
}

/// The Zero-X sign-in as the bearer for a call to the gateway's own origin (web search), or
/// why there is none. Never log or print the value.
pub fn zerox_bearer_token() -> Result<String, String> {
    zerox_token()
}

/// The Zero-X sign-in for a hosted connector.
pub(crate) fn zerox_token() -> Result<String, String> {
    let provider = TOKEN_PROVIDER
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let token = match provider {
        Some(provider) => provider()?,
        None => return Err(not_signed_in()),
    };
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(not_signed_in());
    }
    Ok(token)
}

fn not_signed_in() -> String {
    "not signed in to Zero-X; run `neuron login` (or set NEURON_TOKEN) to use hosted connectors"
        .to_string()
}

/// The 401 answer: the sign-in the gateway was sent is no longer good.
pub(crate) fn sign_in_again(server_name: &str) -> String {
    format!(
        "Zero-X refused the sign-in for the hosted connector `{server_name}`; run `neuron login` to sign in again"
    )
}

/// The gateway's origin: `NEURON_API_BASE`'s (HTTPS, or HTTP on loopback), else
/// [`ZEROX_DEFAULT_ORIGIN`].
pub fn zerox_origin() -> Result<String, String> {
    if let Some(origin) = test_origin() {
        return Ok(origin);
    }
    match std::env::var("NEURON_API_BASE") {
        Ok(base) if !base.trim().is_empty() => origin_of(base.trim()).ok_or_else(|| {
            "NEURON_API_BASE must be HTTPS or loopback HTTP, without credentials".to_string()
        }),
        _ => Ok(ZEROX_DEFAULT_ORIGIN.to_string()),
    }
}

#[cfg(test)]
fn test_origin() -> Option<String> {
    TEST_ORIGIN
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[cfg(not(test))]
fn test_origin() -> Option<String> {
    None
}

fn origin_of(base: &str) -> Option<String> {
    let url = Url::parse(base).ok()?;
    let local = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    );
    let allowed = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !allowed || !url.username().is_empty() || url.password().is_some() {
        return None;
    }
    Some(url.origin().ascii_serialization())
}

/// A connector id as the gateway names it: lowercase letters, digits, `-` and `_`.
#[must_use]
pub fn valid_connector_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
}

/// `<origin>/mcp/<id>`.
#[must_use]
pub fn zerox_endpoint(origin: &str, id: &str) -> String {
    format!("{}/mcp/{id}", origin.trim_end_matches('/'))
}

/// Where the user links connectors: `<origin>/dashboard#connectors`.
#[must_use]
pub fn zerox_connect_url(origin: &str) -> String {
    format!("{}{ZEROX_CONNECT_PATH}", origin.trim_end_matches('/'))
}

/// The endpoint a `zerox` server with connector `id` reaches. A `url` in its settings
/// (the older spelling wrote one) must be that endpoint: the server is sent the Zero-X
/// sign-in, which must never reach another host.
pub(crate) fn resolve_zerox_endpoint(
    server_name: &str,
    configured_url: &str,
    id: &str,
) -> Result<String, String> {
    if !valid_connector_id(id) {
        return Err(format!(
            "the hosted connector `{server_name}` has an invalid id `{id}` (lowercase letters, digits, - and _)"
        ));
    }
    let endpoint = zerox_endpoint(&zerox_origin()?, id);
    let configured = configured_url.trim();
    if configured.is_empty() || configured.trim_end_matches('/') == endpoint {
        return Ok(endpoint);
    }
    Err(format!(
        "the hosted connector `{server_name}` can only reach {endpoint}; remove its \"url\" setting"
    ))
}

/// The link from a `403` body when it is `needs_connect`: its `connect_url` when that is
/// on the gateway's origin, else the dashboard's connectors page. `None` for any other
/// `403`.
pub(crate) fn needs_connect_url(body: &str, origin: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("error").and_then(Value::as_str) != Some("needs_connect") {
        return None;
    }
    let same_origin = |url: &str| {
        Url::parse(url).is_ok_and(|url| {
            matches!(url.scheme(), "https" | "http") && url.origin().ascii_serialization() == origin
        })
    };
    Some(
        value
            .get("connect_url")
            .and_then(Value::as_str)
            .filter(|url| same_origin(url))
            .map_or_else(|| zerox_connect_url(origin), str::to_string),
    )
}

/// Why a hosted server cannot be used until its connector is linked.
pub(crate) fn needs_connect_message(server_name: &str, connect_url: &str) -> String {
    format!(
        "the hosted connector `{server_name}` is not linked to your Zero-X account; connect it at {connect_url} (or run /mcp connect {server_name}), then try again"
    )
}

/// The link to connect the hosted server `server_name`, while the gateway says it is
/// not linked; `None` once it answered a request (or before it was asked).
#[must_use]
pub fn zerox_needs_connect(server_name: &str) -> Option<String> {
    NEEDS_CONNECT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(server_name)
        .cloned()
}

/// Marks the hosted server `server_name` as not linked, with where to link it.
pub fn note_needs_connect(server_name: &str, connect_url: &str) {
    NEEDS_CONNECT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(server_name.to_string(), connect_url.to_string());
}

/// Clears that mark: the gateway answered for `server_name`.
pub fn note_connected(server_name: &str) {
    NEEDS_CONNECT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(server_name);
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use super::{
        needs_connect_url, resolve_zerox_endpoint, set_zerox_token_provider, valid_connector_id,
        zerox_token, TEST_ORIGIN, ZEROX_HOSTED_CONNECTORS_ENV,
    };

    static GUARD: Mutex<()> = Mutex::new(());

    /// Holds the gateway origin, the sign-in and `NEURON_HOSTED_CONNECTORS` that hosted
    /// connector tests use; one test at a time, since all three are process-wide.
    pub(crate) struct ZeroxTestGateway {
        _guard: MutexGuard<'static, ()>,
    }

    impl ZeroxTestGateway {
        /// Hosted connectors on, the gateway at `origin`, signed in with `token`.
        pub(crate) fn new(origin: &str, token: Option<&str>) -> Self {
            let gateway = Self::hold(origin);
            std::env::set_var(ZEROX_HOSTED_CONNECTORS_ENV, "1");
            let token = token.map(str::to_string);
            set_zerox_token_provider(Some(Arc::new(move || {
                token
                    .clone()
                    .ok_or_else(|| "not signed in to Zero-X".to_string())
            })));
            gateway
        }

        /// Hosted connectors off, signed in.
        pub(crate) fn off(origin: &str) -> Self {
            let gateway = Self::hold(origin);
            std::env::remove_var(ZEROX_HOSTED_CONNECTORS_ENV);
            set_zerox_token_provider(Some(Arc::new(|| Ok("ses_linked".to_string()))));
            gateway
        }

        fn hold(origin: &str) -> Self {
            let guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner);
            *TEST_ORIGIN.write().unwrap_or_else(PoisonError::into_inner) =
                Some(origin.trim_end_matches('/').to_string());
            Self { _guard: guard }
        }
    }

    impl Drop for ZeroxTestGateway {
        fn drop(&mut self) {
            *TEST_ORIGIN.write().unwrap_or_else(PoisonError::into_inner) = None;
            std::env::remove_var(ZEROX_HOSTED_CONNECTORS_ENV);
            set_zerox_token_provider(None);
        }
    }

    #[test]
    fn connector_ids_are_plain_names() {
        assert!(valid_connector_id("github"));
        assert!(valid_connector_id("google-drive_2"));
        for id in [
            "",
            "GitHub",
            "../admin",
            "a/b",
            "a?b",
            "a.b",
            &"x".repeat(65),
        ] {
            assert!(!valid_connector_id(id), "{id}");
        }
    }

    #[test]
    fn the_endpoint_is_always_the_gateways_own() {
        let _gateway = ZeroxTestGateway::new("https://gw.example", Some("ses_x"));
        assert_eq!(
            resolve_zerox_endpoint("github", "", "github").unwrap(),
            "https://gw.example/mcp/github"
        );
        assert_eq!(
            resolve_zerox_endpoint("github", "https://gw.example/mcp/github/", "github").unwrap(),
            "https://gw.example/mcp/github"
        );
        let elsewhere =
            resolve_zerox_endpoint("github", "https://evil.example/mcp/github", "github")
                .unwrap_err();
        assert!(elsewhere.contains("can only reach https://gw.example/mcp/github"));
        assert!(resolve_zerox_endpoint("github", "", "../x").is_err());
        assert_eq!(zerox_token().unwrap(), "ses_x");
    }

    #[test]
    fn a_connect_url_off_the_gateway_is_replaced_by_the_dashboard() {
        let origin = "https://gw.example";
        assert_eq!(
            needs_connect_url(
                r#"{"error":"needs_connect","connect_url":"https://gw.example/dashboard#connectors"}"#,
                origin
            )
            .as_deref(),
            Some("https://gw.example/dashboard#connectors")
        );
        assert_eq!(
            needs_connect_url(
                r#"{"error":"needs_connect","connect_url":"https://phish.example/x"}"#,
                origin
            )
            .as_deref(),
            Some("https://gw.example/dashboard#connectors")
        );
        assert_eq!(
            needs_connect_url(r#"{"error":"needs_connect"}"#, origin).as_deref(),
            Some("https://gw.example/dashboard#connectors")
        );
        assert_eq!(needs_connect_url(r#"{"error":"forbidden"}"#, origin), None);
        assert_eq!(needs_connect_url("not json", origin), None);
    }
}
