//! Device-code sign-in for `neuron login` (Zero-X `docs/PLATFORM_CONTRACT.md` §8.2, RFC 8628
//! shape). The terminal shows a short code and <https://neuron.zero-x.live/device>; the user
//! approves there while signed in to the website, and Neuron polls until the gateway hands
//! over the session. Nothing listens on a local port, so this works over SSH, in WSL and in
//! containers, and the session never appears in a URL.
//!
//! The protocol is separate from the network ([`Transport`]), the clock ([`Clock`]) and the
//! screen ([`DeviceLoginObserver`]) so the full-screen UI can drive the same flow and tests
//! can script it.

use std::time::Duration;

use reqwest::Url;
use serde_json::{json, Value};

use crate::credentials::{classify_token, TokenKind};
use crate::gateway_http::{self, ACCOUNT_TIMEOUT};
use crate::vault::SecureString;

pub const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const CLIENT_ID: &str = "neuron-cli";
const USER_CODE_ALPHABET: &str = "BCDFGHJKLMNPQRSTVWXZ";
const MIN_INTERVAL: Duration = Duration::from_secs(5);
const MAX_INTERVAL: Duration = Duration::from_secs(60);
const SLOW_DOWN_STEP: Duration = Duration::from_secs(5);
const MIN_EXPIRY: Duration = Duration::from_secs(60);
const MAX_EXPIRY: Duration = Duration::from_secs(1800);
/// Hosts allowed to appear in the printed sign-in link.
const VERIFICATION_HOSTS: &[&str] = &["neuron.zero-x.live", "zero-x.live"];

/// What the gateway issued for this sign-in.
#[derive(Debug, Clone)]
pub struct DeviceCode {
    /// The polling secret (never shown).
    secret: SecureString,
    /// `XXXX-XXXX`, shown to the user.
    pub user_code: String,
    /// The page to open (always printed).
    pub verification_uri: String,
    /// The same page with the code filled in (opened in the browser when possible).
    pub verification_uri_complete: Option<String>,
    pub expires_in: Duration,
    pub interval: Duration,
}

/// The signed-in result: the session token and the account fields the gateway returned.
#[derive(Debug)]
pub struct DeviceGrant {
    pub token: SecureString,
    pub email: Option<String>,
    pub plan: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceLoginError {
    /// The approval page denied the sign-in.
    Denied,
    /// The code expired before it was approved.
    Expired,
    /// The user cancelled (Ctrl-C in the UI).
    Cancelled,
    /// The gateway does not offer device sign-in (an older deployment).
    Unsupported,
    /// The gateway answered with something this client must not trust or cannot use.
    Invalid(String),
    /// The gateway could not be reached when the sign-in started.
    Unavailable(String),
}

impl std::fmt::Display for DeviceLoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => write!(f, "Sign-in was denied in the browser."),
            Self::Expired => write!(f, "The sign-in code expired. Run `neuron login` again."),
            Self::Cancelled => write!(f, "Sign-in cancelled."),
            Self::Unsupported => write!(
                f,
                "This Zero-X gateway does not support device sign-in yet. Use `neuron auth login --with-token`."
            ),
            Self::Invalid(message) => write!(f, "Sign-in failed: {message}"),
            Self::Unavailable(message) => write!(f, "Could not reach Zero-X: {message}"),
        }
    }
}

impl std::error::Error for DeviceLoginError {}

impl From<DeviceLoginError> for std::io::Error {
    fn from(error: DeviceLoginError) -> Self {
        let kind = match error {
            DeviceLoginError::Denied => std::io::ErrorKind::PermissionDenied,
            DeviceLoginError::Expired => std::io::ErrorKind::TimedOut,
            DeviceLoginError::Cancelled => std::io::ErrorKind::Interrupted,
            DeviceLoginError::Unavailable(_) => std::io::ErrorKind::ConnectionRefused,
            DeviceLoginError::Unsupported | DeviceLoginError::Invalid(_) => {
                std::io::ErrorKind::InvalidData
            }
        };
        Self::new(kind, error.to_string())
    }
}

/// `POST url` with a JSON body; returns the status and the parsed body (`Null` when the body
/// is not JSON). `Err` is a transport failure.
pub trait Transport {
    fn post_json(&self, url: &str, body: &Value) -> Result<(u16, Value), String>;
}

/// Time for the polling loop. `sleep` returns early, with `false`, when the sign-in was
/// cancelled.
pub trait Clock {
    fn now(&self) -> Duration;
    fn sleep(&self, duration: Duration) -> bool;
}

/// Where the flow reports progress: the line terminal today, the full-screen UI later.
pub trait DeviceLoginObserver {
    fn show_code(&self, code: &DeviceCode, browser_opened: bool);
    fn waiting(&self, remaining: Duration);
}

/// What this device reports about itself; the approval page shows it as "reported by the
/// device", so it is informational only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    pub version: String,
    pub device_name: Option<String>,
    pub os: String,
}

impl ClientInfo {
    #[must_use]
    pub fn current() -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            device_name: device_name(),
            os: std::env::consts::OS.to_string(),
        }
    }
}

fn device_name() -> Option<String> {
    let from_env = ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok());
    let name = from_env.or_else(|| std::fs::read_to_string("/etc/hostname").ok())?;
    let name: String = name
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(64)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Asks the gateway for a code. Everything printed later comes from this answer, so it is
/// validated here: a code from the expected alphabet and an https link on a Zero-X host
/// (or the local dev gateway's own origin).
pub fn request_code(
    base_url: &str,
    transport: &dyn Transport,
    client: &ClientInfo,
) -> Result<DeviceCode, DeviceLoginError> {
    let body = json!({
        "client_id": CLIENT_ID,
        "client_version": client.version,
        "device_name": client.device_name,
        "os": client.os,
    });
    let url = gateway_http::account_url(base_url, "/auth/device/code");
    let (status, value) = transport
        .post_json(&url, &body)
        .map_err(DeviceLoginError::Unavailable)?;
    if status == 404 || status == 405 {
        return Err(DeviceLoginError::Unsupported);
    }
    if !(200..300).contains(&status) {
        return Err(DeviceLoginError::Unavailable(error_message(
            &value,
            &format!("HTTP {status}"),
        )));
    }
    parse_device_code(base_url, &value)
}

fn parse_device_code(base_url: &str, value: &Value) -> Result<DeviceCode, DeviceLoginError> {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let invalid =
        |what: &str| DeviceLoginError::Invalid(format!("the gateway sent an invalid {what}"));
    let device_code = text("device_code")
        .filter(|code| valid_device_code(code))
        .ok_or_else(|| invalid("device code"))?;
    let user_code = text("user_code")
        .filter(|code| valid_user_code(code))
        .ok_or_else(|| invalid("user code"))?;
    let verification_uri = text("verification_uri")
        .filter(|uri| trusted_link(base_url, uri))
        .ok_or_else(|| invalid("sign-in link"))?;
    let verification_uri_complete = text("verification_uri_complete")
        .filter(|uri| trusted_link(base_url, uri) && same_page(&verification_uri, uri));
    let seconds = |key: &str, fallback: Duration| {
        gateway_http::u64_field(value, key).map_or(fallback, Duration::from_secs)
    };
    Ok(DeviceCode {
        secret: SecureString::new(device_code),
        user_code,
        verification_uri,
        verification_uri_complete,
        expires_in: seconds("expires_in", Duration::from_secs(600)).clamp(MIN_EXPIRY, MAX_EXPIRY),
        interval: seconds("interval", MIN_INTERVAL).clamp(MIN_INTERVAL, MAX_INTERVAL),
    })
}

fn valid_device_code(code: &str) -> bool {
    code.strip_prefix("zdc_").is_some_and(|rest| {
        rest.len() == 43
            && rest
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    })
}

fn valid_user_code(code: &str) -> bool {
    let bytes = code.as_bytes();
    bytes.len() == 9
        && bytes[4] == b'-'
        && code
            .chars()
            .enumerate()
            .all(|(index, ch)| index == 4 || USER_CODE_ALPHABET.contains(ch))
}

/// An https link on a Zero-X host, or, when the gateway itself is a local dev server, a link
/// on that same origin. Never credentials, never a fragment.
fn trusted_link(base_url: &str, link: &str) -> bool {
    let Ok(url) = Url::parse(link) else {
        return false;
    };
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return false;
    }
    if url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| VERIFICATION_HOSTS.contains(&host))
    {
        return url.port().is_none();
    }
    let Ok(base) = Url::parse(base_url) else {
        return false;
    };
    let local = matches!(
        base.host_str(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    );
    local && url.origin() == base.origin()
}

fn same_page(uri: &str, complete: &str) -> bool {
    match (Url::parse(uri), Url::parse(complete)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin() && a.path() == b.path(),
        _ => false,
    }
}

fn error_message(value: &Value, fallback: &str) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .map_or_else(|| fallback.to_string(), str::to_string)
}

fn error_code(value: &Value) -> Option<&str> {
    value
        .get("error")
        .and_then(|error| error.get("code").or(Some(error)))
        .and_then(Value::as_str)
}

/// Polls until the code is approved, denied or expired. Waits `interval` before each poll,
/// raises it on `slow_down` (to the server's value, else by 5 s; never above 60 s) and backs
/// off on transport errors or 5xx until the code expires.
pub fn poll_for_grant(
    base_url: &str,
    code: &DeviceCode,
    transport: &dyn Transport,
    clock: &dyn Clock,
    observer: &dyn DeviceLoginObserver,
) -> Result<DeviceGrant, DeviceLoginError> {
    let url = gateway_http::account_url(base_url, "/auth/device/token");
    let body = json!({ "grant_type": DEVICE_GRANT_TYPE, "device_code": code.secret.expose() });
    let deadline = clock.now() + code.expires_in;
    let mut interval = code.interval;
    loop {
        let now = clock.now();
        if now >= deadline {
            return Err(DeviceLoginError::Expired);
        }
        observer.waiting(deadline.saturating_sub(now));
        if !clock.sleep(interval.min(deadline.saturating_sub(now))) {
            return Err(DeviceLoginError::Cancelled);
        }
        if clock.now() >= deadline {
            return Err(DeviceLoginError::Expired);
        }
        let (status, value) = match transport.post_json(&url, &body) {
            Ok(answer) => answer,
            Err(_) => {
                interval = (interval * 2).min(MAX_INTERVAL);
                continue;
            }
        };
        if (200..300).contains(&status) {
            return grant_from(&value);
        }
        match error_code(&value) {
            Some("authorization_pending") => {}
            Some("slow_down") => {
                let suggested = value
                    .get("error")
                    .and_then(|error| gateway_http::u64_field(error, "interval"))
                    .map(Duration::from_secs);
                interval = suggested
                    .unwrap_or(interval + SLOW_DOWN_STEP)
                    .clamp(MIN_INTERVAL, MAX_INTERVAL);
            }
            Some("access_denied") => return Err(DeviceLoginError::Denied),
            Some("expired_token") => return Err(DeviceLoginError::Expired),
            Some("invalid_grant" | "unsupported_grant_type") => {
                return Err(DeviceLoginError::Invalid(error_message(
                    &value,
                    "the sign-in code is no longer valid",
                )))
            }
            _ if status == 429 || status >= 500 => interval = (interval * 2).min(MAX_INTERVAL),
            _ => {
                return Err(DeviceLoginError::Invalid(error_message(
                    &value,
                    &format!("HTTP {status}"),
                )))
            }
        }
    }
}

fn grant_from(value: &Value) -> Result<DeviceGrant, DeviceLoginError> {
    let token = value
        .get("access_token")
        .or_else(|| value.get("session_token"))
        .and_then(Value::as_str)
        .ok_or_else(|| DeviceLoginError::Invalid("the gateway sent no session".into()))?;
    // Device sign-in only ever yields a CLI session, never an API key or anything else.
    if classify_token(token) != Ok(TokenKind::Session) {
        return Err(DeviceLoginError::Invalid(
            "the gateway sent an invalid session".into(),
        ));
    }
    Ok(DeviceGrant {
        token: SecureString::new(token.to_string()),
        email: gateway_http::string_field(value, "email"),
        plan: gateway_http::string_field(value, "plan"),
    })
}

/// The whole flow: request a code, show it (opening the browser when `open_browser`),
/// then poll. `open_browser` returns whether a browser was opened.
pub fn run(
    base_url: &str,
    transport: &dyn Transport,
    clock: &dyn Clock,
    observer: &dyn DeviceLoginObserver,
    client: &ClientInfo,
    open_browser: &dyn Fn(&str) -> bool,
) -> Result<DeviceGrant, DeviceLoginError> {
    let code = request_code(base_url, transport, client)?;
    let link = code
        .verification_uri_complete
        .as_deref()
        .unwrap_or(&code.verification_uri);
    let opened = open_browser(link);
    observer.show_code(&code, opened);
    poll_for_grant(base_url, &code, transport, clock, observer)
}

/// Whether to try opening a browser here. Never over SSH, in a container or on a Linux box
/// without a display; always when the user did not opt out elsewhere. The link and code are
/// printed either way.
#[must_use]
pub fn browser_allowed(
    requested: bool,
    env: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&str) -> bool,
) -> bool {
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    if !requested || set("NEURON_NO_BROWSER") {
        return false;
    }
    if set("SSH_CONNECTION") || set("SSH_TTY") || set("SSH_CLIENT") {
        return false;
    }
    if set("container") || exists("/.dockerenv") || exists("/run/.containerenv") {
        return false;
    }
    if cfg!(target_os = "linux") && !set("WSL_DISTRO_NAME") {
        return set("DISPLAY") || set("WAYLAND_DISPLAY");
    }
    true
}

// ── Real implementations ──

/// Blocking HTTP through the same client settings as the other account calls.
pub struct HttpTransport;

impl Transport for HttpTransport {
    fn post_json(&self, url: &str, body: &Value) -> Result<(u16, Value), String> {
        let response = gateway_http::client(ACCOUNT_TIMEOUT)
            .map_err(|error| error.to_string())?
            .post(url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .map_err(|error| error.to_string())?;
        let status = response.status().as_u16();
        let text = response.text().map_err(|error| error.to_string())?;
        Ok((status, serde_json::from_str(&text).unwrap_or(Value::Null)))
    }
}

/// Wall clock with a cancellation check between short naps.
pub struct SystemClock<'a> {
    started: std::time::Instant,
    cancelled: &'a dyn Fn() -> bool,
}

impl<'a> SystemClock<'a> {
    #[must_use]
    pub fn new(cancelled: &'a dyn Fn() -> bool) -> Self {
        Self {
            started: std::time::Instant::now(),
            cancelled,
        }
    }
}

impl Clock for SystemClock<'_> {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn sleep(&self, duration: Duration) -> bool {
        let end = self.started.elapsed() + duration;
        loop {
            if (self.cancelled)() {
                return false;
            }
            let now = self.started.elapsed();
            if now >= end {
                return true;
            }
            std::thread::sleep(end.saturating_sub(now).min(Duration::from_millis(200)));
        }
    }
}

/// Line-terminal progress on stderr.
pub struct TerminalObserver;

impl DeviceLoginObserver for TerminalObserver {
    fn show_code(&self, code: &DeviceCode, browser_opened: bool) {
        eprintln!("\nSign in to Zero-X to start Neuron.");
        eprintln!("  1. Open        {}", code.verification_uri);
        eprintln!("  2. Enter code  {}", code.user_code);
        if browser_opened {
            eprintln!("  The page is open in your browser; check that it shows the same code.");
        } else {
            eprintln!("  Open the link on any device (your phone works), then enter the code.");
        }
        eprintln!(
            "Waiting for approval (the code expires in {} min; Ctrl+C to cancel)...",
            code.expires_in.as_secs().div_ceil(60)
        );
    }

    fn waiting(&self, _remaining: Duration) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    const DEVICE_CODE: &str = "zdc_0123456789abcdefghijABCDEFGHIJ_-01234567890";
    const SESSION: &str = "ses_abcdefghijklmnopqrstuvwxyz012345";
    const BASE: &str = "https://zero-x.live/v1";

    struct Script {
        answers: RefCell<VecDeque<Result<(u16, Value), String>>>,
        requests: RefCell<Vec<(String, Value)>>,
    }

    impl Script {
        fn new(answers: Vec<Result<(u16, Value), String>>) -> Self {
            Self {
                answers: RefCell::new(answers.into()),
                requests: RefCell::new(Vec::new()),
            }
        }
    }

    impl Transport for Script {
        fn post_json(&self, url: &str, body: &Value) -> Result<(u16, Value), String> {
            self.requests
                .borrow_mut()
                .push((url.to_string(), body.clone()));
            self.answers
                .borrow_mut()
                .pop_front()
                .expect("an answer for every request")
        }
    }

    struct FakeClock {
        now: Cell<Duration>,
        sleeps: RefCell<Vec<Duration>>,
        cancel_after: Option<usize>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: Cell::new(Duration::ZERO),
                sleeps: RefCell::new(Vec::new()),
                cancel_after: None,
            }
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.now.get()
        }
        fn sleep(&self, duration: Duration) -> bool {
            if self.cancel_after == Some(self.sleeps.borrow().len()) {
                return false;
            }
            self.sleeps.borrow_mut().push(duration);
            self.now.set(self.now.get() + duration);
            true
        }
    }

    struct Seen {
        shown: RefCell<Vec<(String, String, bool)>>,
    }

    impl DeviceLoginObserver for Seen {
        fn show_code(&self, code: &DeviceCode, browser_opened: bool) {
            self.shown.borrow_mut().push((
                code.user_code.clone(),
                code.verification_uri.clone(),
                browser_opened,
            ));
        }
        fn waiting(&self, _remaining: Duration) {}
    }

    fn seen() -> Seen {
        Seen {
            shown: RefCell::new(Vec::new()),
        }
    }

    fn code_answer() -> Result<(u16, Value), String> {
        Ok((
            200,
            json!({
                "device_code": DEVICE_CODE, "user_code": "BDWP-HQPK",
                "verification_uri": "https://neuron.zero-x.live/device",
                "verification_uri_complete": "https://neuron.zero-x.live/device?user_code=BDWP-HQPK",
                "expires_in": 600, "interval": 5
            }),
        ))
    }

    fn pending(code: &str) -> Result<(u16, Value), String> {
        Ok((400, json!({"error": {"code": code, "message": code}})))
    }

    fn approved() -> Result<(u16, Value), String> {
        Ok((
            200,
            json!({"access_token": SESSION, "token_type": "Bearer", "email": "dev@example.com", "plan": "free"}),
        ))
    }

    fn client() -> ClientInfo {
        ClientInfo {
            version: "7.0.0".into(),
            device_name: Some("laptop".into()),
            os: "windows".into(),
        }
    }

    #[test]
    fn approves_after_pending_and_opens_the_prefilled_link() {
        let transport = Script::new(vec![
            code_answer(),
            pending("authorization_pending"),
            approved(),
        ]);
        let clock = FakeClock::new();
        let observer = seen();
        let opened = RefCell::new(Vec::new());
        let grant = run(BASE, &transport, &clock, &observer, &client(), &|link| {
            opened.borrow_mut().push(link.to_string());
            true
        })
        .unwrap();
        assert_eq!(grant.token.expose(), SESSION);
        assert_eq!(grant.email.as_deref(), Some("dev@example.com"));
        assert_eq!(
            opened.borrow().as_slice(),
            ["https://neuron.zero-x.live/device?user_code=BDWP-HQPK"]
        );
        assert_eq!(
            observer.shown.borrow().as_slice(),
            [(
                "BDWP-HQPK".to_string(),
                "https://neuron.zero-x.live/device".to_string(),
                true
            )]
        );
        assert_eq!(
            clock.sleeps.borrow().as_slice(),
            [Duration::from_secs(5); 2]
        );
        let requests = transport.requests.borrow();
        assert_eq!(requests[0].0, "https://zero-x.live/auth/device/code");
        assert_eq!(requests[0].1["client_id"], "neuron-cli");
        assert_eq!(requests[0].1["os"], "windows");
        assert_eq!(requests[1].0, "https://zero-x.live/auth/device/token");
        assert_eq!(requests[1].1["grant_type"], DEVICE_GRANT_TYPE);
        assert_eq!(requests[1].1["device_code"], DEVICE_CODE);
    }

    #[test]
    fn slow_down_raises_the_interval_within_bounds() {
        let transport = Script::new(vec![
            code_answer(),
            Ok((400, json!({"error": {"code": "slow_down", "interval": 10}}))),
            pending("slow_down"),
            Ok((
                400,
                json!({"error": {"code": "slow_down", "interval": 900}}),
            )),
            approved(),
        ]);
        let clock = FakeClock::new();
        run(BASE, &transport, &clock, &seen(), &client(), &|_| false).unwrap();
        let sleeps: Vec<u64> = clock
            .sleeps
            .borrow()
            .iter()
            .map(Duration::as_secs)
            .collect();
        assert_eq!(
            sleeps,
            [5, 10, 15, 60],
            "server value, then +5 s, capped at 60 s"
        );
    }

    #[test]
    fn server_intervals_and_expiry_are_clamped() {
        let mut answer = code_answer().unwrap().1;
        answer["interval"] = json!(1);
        answer["expires_in"] = json!(999_999);
        let code = parse_device_code(BASE, &answer).unwrap();
        assert_eq!(code.interval, MIN_INTERVAL);
        assert_eq!(code.expires_in, MAX_EXPIRY);
    }

    #[test]
    fn denied_expired_and_reused_codes_end_the_flow() {
        for (answer, expected) in [
            (pending("access_denied"), DeviceLoginError::Denied),
            (pending("expired_token"), DeviceLoginError::Expired),
        ] {
            let transport = Script::new(vec![code_answer(), answer]);
            let error = run(
                BASE,
                &transport,
                &FakeClock::new(),
                &seen(),
                &client(),
                &|_| false,
            )
            .unwrap_err();
            assert_eq!(error, expected);
        }
        let transport = Script::new(vec![code_answer(), pending("invalid_grant")]);
        let error = run(
            BASE,
            &transport,
            &FakeClock::new(),
            &seen(),
            &client(),
            &|_| false,
        )
        .unwrap_err();
        assert!(matches!(error, DeviceLoginError::Invalid(_)));
    }

    #[test]
    fn the_code_expires_on_the_clock_even_if_the_server_keeps_saying_pending() {
        let mut answer = code_answer().unwrap().1;
        answer["expires_in"] = json!(60);
        answer["interval"] = json!(30);
        let transport = Script::new(vec![
            Ok((200, answer)),
            pending("authorization_pending"),
            pending("authorization_pending"),
        ]);
        let clock = FakeClock::new();
        let error = run(BASE, &transport, &clock, &seen(), &client(), &|_| false).unwrap_err();
        assert_eq!(error, DeviceLoginError::Expired);
        assert_eq!(clock.now.get(), Duration::from_secs(60));
    }

    #[test]
    fn outages_back_off_and_keep_polling() {
        let transport = Script::new(vec![
            code_answer(),
            Err("connection reset".into()),
            Ok((503, json!({"error": {"code": "service_unavailable"}}))),
            approved(),
        ]);
        let clock = FakeClock::new();
        run(BASE, &transport, &clock, &seen(), &client(), &|_| false).unwrap();
        let sleeps: Vec<u64> = clock
            .sleeps
            .borrow()
            .iter()
            .map(Duration::as_secs)
            .collect();
        assert_eq!(sleeps, [5, 10, 20]);
    }

    #[test]
    fn cancelling_stops_polling() {
        let transport = Script::new(vec![code_answer(), pending("authorization_pending")]);
        let clock = FakeClock {
            cancel_after: Some(1),
            ..FakeClock::new()
        };
        let error = run(BASE, &transport, &clock, &seen(), &client(), &|_| false).unwrap_err();
        assert_eq!(error, DeviceLoginError::Cancelled);
    }

    #[test]
    fn untrusted_answers_are_refused_before_anything_is_printed() {
        let tampered = |key: &str, value: Value| {
            let mut answer = code_answer().unwrap().1;
            answer[key] = value;
            parse_device_code(BASE, &answer).unwrap_err()
        };
        assert!(matches!(
            tampered(
                "verification_uri",
                json!("http://neuron.zero-x.live/device")
            ),
            DeviceLoginError::Invalid(_)
        ));
        assert!(matches!(
            tampered("verification_uri", json!("https://evil.example/device")),
            DeviceLoginError::Invalid(_)
        ));
        assert!(matches!(
            tampered(
                "verification_uri",
                json!("https://user@neuron.zero-x.live/device")
            ),
            DeviceLoginError::Invalid(_)
        ));
        assert!(matches!(
            tampered("user_code", json!("AEIO-UAEI")),
            DeviceLoginError::Invalid(_)
        ));
        assert!(matches!(
            tampered("user_code", json!("BDWPHQPK")),
            DeviceLoginError::Invalid(_)
        ));
        assert!(matches!(
            tampered("device_code", json!("ses_abc")),
            DeviceLoginError::Invalid(_)
        ));
        // A prefilled link on another page is dropped, not trusted.
        let mut answer = code_answer().unwrap().1;
        answer["verification_uri_complete"] =
            json!("https://zero-x.live/other?user_code=BDWP-HQPK");
        assert!(parse_device_code(BASE, &answer)
            .unwrap()
            .verification_uri_complete
            .is_none());
        // A local dev gateway may send its own origin.
        let mut local = code_answer().unwrap().1;
        local["verification_uri"] = json!("http://127.0.0.1:8787/neuroncli/device/");
        local["verification_uri_complete"] =
            json!("http://127.0.0.1:8787/neuroncli/device/?user_code=BDWP-HQPK");
        assert!(parse_device_code("http://127.0.0.1:8787/v1", &local).is_ok());
        assert!(parse_device_code(BASE, &local).is_err());
    }

    #[test]
    fn only_a_cli_session_is_accepted() {
        for token in ["nzx_0123456789abcdefghij", "sk-provider-secret", ""] {
            let transport = Script::new(vec![
                code_answer(),
                Ok((200, json!({"access_token": token}))),
            ]);
            let error = run(
                BASE,
                &transport,
                &FakeClock::new(),
                &seen(),
                &client(),
                &|_| false,
            )
            .unwrap_err();
            assert!(matches!(error, DeviceLoginError::Invalid(_)), "{token}");
        }
    }

    #[test]
    fn older_gateways_report_device_sign_in_as_unsupported() {
        let transport = Script::new(vec![Ok((404, json!({"error": {"code": "not_found"}})))]);
        let error = run(
            BASE,
            &transport,
            &FakeClock::new(),
            &seen(),
            &client(),
            &|_| false,
        )
        .unwrap_err();
        assert_eq!(error, DeviceLoginError::Unsupported);
    }

    #[test]
    fn browser_is_skipped_over_ssh_in_containers_and_when_asked() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_string())
            }
        };
        let none = |_: &str| false;
        let desktop = env(&[("DISPLAY", ":0")]);
        assert!(browser_allowed(true, &desktop, &none));
        assert!(!browser_allowed(false, &desktop, &none));
        assert!(!browser_allowed(
            true,
            &env(&[("DISPLAY", ":0"), ("NEURON_NO_BROWSER", "1")]),
            &none
        ));
        assert!(!browser_allowed(
            true,
            &env(&[("DISPLAY", ":0"), ("SSH_CONNECTION", "1 2 3 4")]),
            &none
        ));
        assert!(!browser_allowed(
            true,
            &env(&[("DISPLAY", ":0"), ("SSH_TTY", "/dev/pts/1")]),
            &none
        ));
        assert!(!browser_allowed(
            true,
            &env(&[("DISPLAY", ":0"), ("container", "podman")]),
            &none
        ));
        assert!(!browser_allowed(true, &desktop, &|path: &str| path == "/.dockerenv"));
        if cfg!(target_os = "linux") {
            assert!(!browser_allowed(true, &env(&[]), &none), "no display");
            assert!(browser_allowed(
                true,
                &env(&[("WSL_DISTRO_NAME", "Ubuntu")]),
                &none
            ));
        }
    }
}
