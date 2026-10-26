//! Runs a share around the sans-I/O [`ShareHost`]: the relay socket (join, keepalive, reconnect
//! with backoff), one client of the session's local host per phone channel, and an observer
//! client that turns the session's stream into presence.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt as _, StreamExt as _};
use rand::Rng as _;
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{sleep, sleep_until, Instant};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use url::Url;

use crate::config::{ConfigStore, ShareConfig};
use crate::contract::{
    close, fatal_close_message, is_fatal_close, parse_relay_frame, DeviceInfo, HostFrame,
    RelayFrame, Role, PING, RELAY_SUBPROTOCOL,
};
use crate::crypto::E2eError;
use crate::host::{Action, ShareHost};
use crate::pairing::PairingPayload;
use crate::presence::{Presence, PresenceTracker};

const HEARTBEAT: Duration = Duration::from_secs(25);
const PONG_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_BASE_MS: u64 = 500;
const BACKOFF_MAX_MS: u64 = 30_000;
/// At most a few presence updates a second, however busy the session.
const PRESENCE_EVERY: Duration = Duration::from_millis(150);
const LOCAL_RETRY: Duration = Duration::from_secs(2);

/// Where the share's messages for the user go (the terminal, or a test).
pub trait Reporter: Send {
    fn status(&mut self, line: &str);
    /// A pairing code is live: show its QR code and link.
    fn offer(&mut self, payload: &PairingPayload);
    fn paired(&mut self, phone_name: &str);
}

pub struct ShareOptions {
    /// The relay's WebSocket URL (`wss://relay.zero-x.live/v1/relay`).
    pub relay_url: String,
    /// The Zero-X session (`ses_…`): sent only in the relay's `Authorization` header, never logged.
    pub token: String,
    /// Where `remote.json` lives (the Neuron home).
    pub config_dir: PathBuf,
    /// The computer's name on the phone.
    pub name: String,
    pub platform: String,
    /// The session's local remote-protocol-1 host (`ws://127.0.0.1:<port>/neuron`).
    pub local_url: String,
    /// The local host's pairing token: put into each phone's `hello`, never sent to the relay.
    pub local_token: String,
    pub pair_ttl_s: u64,
    /// Offer a pairing code as soon as the relay is joined.
    pub offer_at_start: bool,
    /// Each line on standard input offers a new code.
    pub read_stdin: bool,
    /// Stops the share (besides Ctrl+C).
    pub stop: Option<oneshot::Receiver<()>>,
}

#[derive(Debug)]
pub enum ShareError {
    Config(io::Error),
    Key(E2eError),
    Address(String),
    /// The relay or the local host refused for good (a sign-in is needed, the token is wrong).
    Refused(String),
}

impl fmt::Display for ShareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => {
                write!(f, "The share settings could not be read or saved: {error}")
            }
            Self::Key(error) => write!(f, "The share settings hold a damaged key: {error}"),
            Self::Address(message) | Self::Refused(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ShareError {}

fn private_host(host: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.rsplit('.').next() == Some("local")
        || host == "::1"
    {
        return true;
    }
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Err(_) => host.starts_with("fc") || host.starts_with("fd") || host.starts_with("fe80:"),
    }
}

/// A relay address: `wss://`, or `ws://` on this machine or a private network (development), with
/// no credentials, query or fragment.
pub fn validate_relay_url(text: &str) -> Result<Url, String> {
    let url =
        Url::parse(text).map_err(|_| format!("The relay address {text} is not an address."))?;
    let private = url.host_str().is_some_and(private_host);
    if !(url.scheme() == "wss" || (url.scheme() == "ws" && private)) {
        return Err(
            "The relay address must be wss:// (or ws:// on this machine or your own network)."
                .into(),
        );
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("The relay address must not carry credentials, a query or a fragment.".into());
    }
    Ok(url)
}

/// The local host's address: on this machine only, since the phone's frames and the local token
/// go there.
pub fn validate_local_url(text: &str) -> Result<Url, String> {
    let url = Url::parse(text)
        .map_err(|_| format!("The Neuron host address {text} is not an address."))?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if !matches!(url.scheme(), "ws" | "wss") || !loopback {
        return Err("The Neuron host must be a ws:// address on this computer (127.0.0.1).".into());
    }
    Ok(url)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The delay before reconnect attempt `attempt` (0-based): full jitter over a capped exponential.
fn backoff(attempt: u32) -> Duration {
    let window = BACKOFF_BASE_MS
        .saturating_mul(1_u64 << attempt.min(16))
        .min(BACKOFF_MAX_MS);
    Duration::from_millis(rand::thread_rng().gen_range(BACKOFF_BASE_MS / 10..=window))
}

enum Event {
    RelayFrame(String),
    RelayClosed {
        code: Option<u16>,
        reason: String,
    },
    LocalOpened(String),
    LocalMessage(String, String),
    LocalClosed(String),
    /// The observer reached the local host: its project folder.
    SessionFound(String),
    Presence(Presence),
    /// The local host refused the observer (a wrong token).
    LocalRefused(String),
    /// The local host did not answer the first time.
    LocalUnreachable,
    NewCode,
    Quit,
}

type Events = mpsc::UnboundedSender<Event>;

/// Shares the session until Ctrl+C, `options.stop`, or a refusal the user must fix.
pub async fn run(mut options: ShareOptions, reporter: Box<dyn Reporter>) -> Result<(), ShareError> {
    // One TLS provider for the relay socket, whatever else the binary links.
    let _ = rustls::crypto::ring::default_provider().install_default();
    validate_relay_url(&options.relay_url).map_err(ShareError::Address)?;
    validate_local_url(&options.local_url).map_err(ShareError::Address)?;
    let store = ConfigStore::in_dir(&options.config_dir);
    let mut config: ShareConfig = store.load().map_err(ShareError::Config)?;
    let identity = config.identity_named(&options.name).clone();
    store.save(&config).map_err(ShareError::Config)?;
    let mut host =
        ShareHost::new(identity.clone(), config.phones.clone()).map_err(ShareError::Key)?;

    let (events_tx, mut events) = mpsc::unbounded_channel::<Event>();
    let (relay_tx, relay_rx) = mpsc::unbounded_channel::<String>();
    let device = DeviceInfo {
        id: identity.host_id.clone(),
        name: identity.name.clone(),
        platform: options.platform.clone(),
    };
    let relay = tokio::spawn(relay_task(
        options.relay_url.clone(),
        options.token.clone(),
        device,
        relay_rx,
        events_tx.clone(),
    ));
    let observer = tokio::spawn(observer_task(
        options.local_url.clone(),
        options.local_token.clone(),
        events_tx.clone(),
    ));
    if options.read_stdin {
        let lines = events_tx.clone();
        tokio::spawn(async move {
            let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(_)) = input.next_line().await {
                if lines.send(Event::NewCode).is_err() {
                    break;
                }
            }
        });
    }
    if let Some(stop) = options.stop.take() {
        let quit = events_tx.clone();
        tokio::spawn(async move {
            let _ = stop.await;
            let _ = quit.send(Event::Quit);
        });
    }
    let interrupt = events_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = interrupt.send(Event::Quit);
        }
    });

    let mut driver = Driver {
        options,
        store,
        config,
        relay_tx,
        locals: HashMap::new(),
        events_tx,
        share_id: None,
        offered: false,
        reporter,
    };
    driver.reporter.status(&format!(
        "Sharing the Neuron session at {} as \"{}\" through {}.",
        driver.options.local_url, identity.name, driver.options.relay_url
    ));
    let result = driver.run(&mut host, &mut events).await;
    // Give the relay writer a moment to send the last frames (unshare, closes).
    sleep(Duration::from_millis(200)).await;
    relay.abort();
    observer.abort();
    result
}

struct Driver {
    options: ShareOptions,
    store: ConfigStore,
    config: ShareConfig,
    relay_tx: mpsc::UnboundedSender<String>,
    locals: HashMap<String, mpsc::UnboundedSender<String>>,
    events_tx: Events,
    share_id: Option<String>,
    offered: bool,
    reporter: Box<dyn Reporter>,
}

impl Driver {
    async fn run(
        &mut self,
        host: &mut ShareHost,
        events: &mut mpsc::UnboundedReceiver<Event>,
    ) -> Result<(), ShareError> {
        while let Some(event) = events.recv().await {
            let actions = match event {
                Event::RelayFrame(text) => match parse_relay_frame(&text) {
                    Some(frame) => {
                        let joined = matches!(frame, RelayFrame::Joined { .. });
                        let mut actions = host.relay_frame(frame, now_ms());
                        if joined {
                            self.reporter.status("Connected to the relay.");
                            if self.options.offer_at_start && !self.offered {
                                self.offered = true;
                                actions.extend(host.offer_pairing(
                                    &self.options.relay_url,
                                    self.options.pair_ttl_s,
                                ));
                            }
                        }
                        actions
                    }
                    None => Vec::new(),
                },
                Event::RelayClosed { code, reason } => {
                    if let Some(code) = code.filter(|code| is_fatal_close(*code)) {
                        return Err(ShareError::Refused(fatal_close_message(code).to_string()));
                    }
                    let why = if reason.is_empty() {
                        "no reason given"
                    } else {
                        reason.as_str()
                    };
                    self.reporter.status(&format!(
                        "The relay connection dropped ({why}); reconnecting."
                    ));
                    host.relay_lost()
                }
                Event::LocalOpened(ch) => host.local_opened(&ch),
                Event::LocalMessage(ch, text) => host.local_message(&ch, &text),
                Event::LocalClosed(ch) => {
                    self.locals.remove(&ch);
                    host.local_closed(&ch)
                }
                Event::SessionFound(project) => {
                    if self.share_id.is_some() {
                        Vec::new()
                    } else {
                        let share_id = self.config.share_id_for(&project);
                        self.store.save(&self.config).map_err(ShareError::Config)?;
                        self.share_id = Some(share_id.clone());
                        self.reporter
                            .status(&format!("Found the session in {project}."));
                        host.share(&share_id, &self.options.local_token)
                    }
                }
                Event::Presence(presence) => match self.share_id.clone() {
                    Some(share_id) => host.update_presence(&share_id, presence),
                    None => Vec::new(),
                },
                Event::LocalRefused(message) => return Err(ShareError::Refused(message)),
                Event::LocalUnreachable => {
                    self.reporter.status(&format!(
                        "Waiting for the Neuron host at {} (in ui/neuron-client: bun run host).",
                        self.options.local_url
                    ));
                    Vec::new()
                }
                Event::NewCode => {
                    host.offer_pairing(&self.options.relay_url, self.options.pair_ttl_s)
                }
                Event::Quit => {
                    let actions = host.stop();
                    self.execute(host, actions);
                    self.reporter.status("Stopped sharing.");
                    return Ok(());
                }
            };
            self.execute(host, actions);
        }
        Ok(())
    }

    fn execute(&mut self, host: &ShareHost, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Relay(text) => {
                    let _ = self.relay_tx.send(text);
                }
                Action::OpenLocal { ch, .. } => {
                    let (tx, rx) = mpsc::unbounded_channel();
                    self.locals.insert(ch.clone(), tx);
                    tokio::spawn(local_task(
                        ch,
                        self.options.local_url.clone(),
                        rx,
                        self.events_tx.clone(),
                    ));
                }
                Action::LocalSend { ch, text } => {
                    if let Some(local) = self.locals.get(&ch) {
                        let _ = local.send(text);
                    }
                }
                // Dropping the sender ends the local client.
                Action::CloseLocal { ch } => {
                    self.locals.remove(&ch);
                }
                Action::SavePhones => {
                    self.config.phones = host.phones();
                    if let Err(error) = self.store.save(&self.config) {
                        self.reporter
                            .status(&format!("The paired phones could not be saved: {error}"));
                    }
                }
                Action::Paired { name } => self.reporter.paired(&name),
                Action::OfferReady(payload) => self.reporter.offer(&payload),
                Action::OfferFailed(message) => {
                    self.reporter.status(&format!(
                        "No pairing code: {message} Press Enter to try again."
                    ));
                }
            }
        }
    }
}

/// The relay socket: connects with the session in the `Authorization` header, joins, keeps the
/// socket alive and reconnects with backoff until a close code says not to.
async fn relay_task(
    url: String,
    token: String,
    device: DeviceInfo,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    events: Events,
) {
    let join = HostFrame::Join {
        role: Role::Host,
        device,
    }
    .to_json();
    let mut attempt = 0_u32;
    loop {
        let (code, reason) = match connect_relay(&url, &token).await {
            Ok(socket) => {
                let (mut write, mut read) = socket.split();
                // Frames meant for the last socket are stale.
                while outgoing.try_recv().is_ok() {}
                if write.send(Message::text(join.clone())).await.is_err() {
                    (None, "the relay closed the connection".to_string())
                } else {
                    let mut heartbeat = Instant::now() + HEARTBEAT;
                    let mut awaiting_pong = false;
                    loop {
                        tokio::select! {
                            message = read.next() => match message {
                                Some(Ok(Message::Text(text))) => {
                                    heartbeat = Instant::now() + HEARTBEAT;
                                    awaiting_pong = false;
                                    if text.as_str().contains("\"joined\"") {
                                        attempt = 0;
                                    }
                                    if events.send(Event::RelayFrame(text.as_str().to_string())).is_err() {
                                        return;
                                    }
                                }
                                Some(Ok(Message::Close(frame))) => {
                                    break match frame {
                                        Some(frame) => (Some(u16::from(frame.code)), frame.reason.as_str().to_string()),
                                        None => (None, String::new()),
                                    };
                                }
                                Some(Ok(_)) => {}
                                Some(Err(error)) => break (None, error.to_string()),
                                None => break (None, "the connection ended".to_string()),
                            },
                            frame = outgoing.recv() => {
                                let Some(text) = frame else {
                                    let _ = write.close().await;
                                    return;
                                };
                                if let Err(error) = write.send(Message::text(text)).await {
                                    break (None, error.to_string());
                                }
                            }
                            () = sleep_until(heartbeat) => {
                                if awaiting_pong {
                                    break (None, "the relay stopped answering".to_string());
                                }
                                awaiting_pong = true;
                                heartbeat = Instant::now() + PONG_TIMEOUT;
                                if write.send(Message::text(PING)).await.is_err() {
                                    break (None, "the relay closed the connection".to_string());
                                }
                            }
                        }
                    }
                }
            }
            Err(failure) => failure,
        };
        if events.send(Event::RelayClosed { code, reason }).is_err() {
            return;
        }
        if code.is_some_and(is_fatal_close) {
            return;
        }
        let delay = if code == Some(close::LIMITED) {
            Duration::from_millis(BACKOFF_MAX_MS)
        } else {
            backoff(attempt)
        };
        attempt = attempt.saturating_add(1);
        tokio::select! {
            () = sleep(delay) => {}
            frame = outgoing.recv() => if frame.is_none() { return },
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_relay(url: &str, token: &str) -> Result<Socket, (Option<u16>, String)> {
    let mut request = url
        .into_client_request()
        .map_err(|error| (None, error.to_string()))?;
    let headers = request.headers_mut();
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| (Some(close::UNAUTHORIZED), String::new()))?,
    );
    headers.insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static(RELAY_SUBPROTOCOL),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => Ok(socket),
        // A server without WebSocket refusals (the Node gateway) answers with a status.
        Err(WsError::Http(response)) => Err(match response.status().as_u16() {
            401 => (Some(close::UNAUTHORIZED), "sign-in needed".into()),
            403 => (Some(close::FORBIDDEN), "not allowed".into()),
            status => (None, format!("the relay answered HTTP {status}")),
        }),
        Err(error) => Err((None, error.to_string())),
    }
}

/// One phone channel's client of the local host. Ends when its sender is dropped.
async fn local_task(
    ch: String,
    url: String,
    mut outgoing: mpsc::UnboundedReceiver<String>,
    events: Events,
) {
    let Ok((socket, _)) = tokio_tungstenite::connect_async(url.as_str()).await else {
        let _ = events.send(Event::LocalClosed(ch));
        return;
    };
    let (mut write, mut read) = socket.split();
    let _ = events.send(Event::LocalOpened(ch.clone()));
    loop {
        tokio::select! {
            message = read.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if events.send(Event::LocalMessage(ch.clone(), text.as_str().to_string())).is_err() {
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            frame = outgoing.recv() => {
                let Some(text) = frame else {
                    let _ = write.close().await;
                    return;
                };
                if write.send(Message::text(text)).await.is_err() {
                    break;
                }
            }
        }
    }
    let _ = events.send(Event::LocalClosed(ch));
}

/// Follows the shared session as a client of its local host, and reports its presence. While the
/// host is unreachable the session shows as failed, and the observer keeps trying.
async fn observer_task(url: String, token: String, events: Events) {
    let mut tracker: Option<PresenceTracker> = None;
    let mut waited = false;
    loop {
        let connected = tokio_tungstenite::connect_async(url.as_str()).await;
        if connected.is_err() && !waited {
            waited = true;
            if events.send(Event::LocalUnreachable).is_err() {
                return;
            }
        }
        if let Ok((socket, _)) = connected {
            let (mut write, mut read) = socket.split();
            let hello = json!({ "t": "hello", "protocol": 1, "token": token, "client": { "name": "Neuron share", "kind": "cli" } });
            if write.send(Message::text(hello.to_string())).await.is_ok() {
                let mut due: Option<Instant> = None;
                loop {
                    tokio::select! {
                        message = read.next() => {
                            let text = match message {
                                Some(Ok(Message::Text(text))) => text.as_str().to_string(),
                                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                                Some(Ok(_)) => continue,
                            };
                            let Ok(frame) = serde_json::from_str::<Value>(&text) else { continue };
                            match frame.get("t").and_then(Value::as_str) {
                                Some("welcome") => {
                                    let cwd = frame.pointer("/host/cwd").and_then(Value::as_str).unwrap_or("session").to_string();
                                    tracker = Some(PresenceTracker::new(&cwd));
                                    if events.send(Event::SessionFound(cwd)).is_err() {
                                        return;
                                    }
                                }
                                Some("snapshot") => {
                                    if let Some(tracker) = tracker.as_mut() {
                                        tracker.snapshot(frame.get("seq").and_then(Value::as_u64).unwrap_or(0), frame.get("state").unwrap_or(&Value::Null));
                                    }
                                }
                                Some("action") => {
                                    if let (Some(tracker), Some(action)) = (tracker.as_mut(), frame.get("action")) {
                                        tracker.action(frame.get("seq").and_then(Value::as_u64).unwrap_or(0), action);
                                    }
                                }
                                Some("error") if frame.get("fatal").and_then(Value::as_bool) == Some(true) => {
                                    let message = frame.get("message").and_then(Value::as_str).unwrap_or("refused");
                                    let _ = events.send(Event::LocalRefused(format!("The Neuron host at {url} refused this share: {message}")));
                                    return;
                                }
                                _ => continue,
                            }
                            if due.is_none() {
                                due = Some(Instant::now() + PRESENCE_EVERY);
                            }
                        }
                        () = async { sleep_until(due.unwrap_or_else(Instant::now)).await }, if due.is_some() => {
                            due = None;
                            if let Some(tracker) = tracker.as_ref() {
                                if events.send(Event::Presence(tracker.presence(now_ms()))).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            // The host went away: the session cannot be driven until it is back.
            if let Some(tracker) = tracker.as_mut() {
                tracker.action(
                    tracker.seq(),
                    &json!({ "type": "local/disconnected", "error": null }),
                );
                if events
                    .send(Event::Presence(tracker.presence(now_ms())))
                    .is_err()
                {
                    return;
                }
            }
        }
        sleep(LOCAL_RETRY).await;
    }
}
