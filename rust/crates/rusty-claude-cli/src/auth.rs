//! Zero-X sign-in: browser loopback login, headless token login, session
//! lifecycle (`/auth/session`), logout and `neuron auth status`.

use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use reqwest::Url;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::credentials::{
    classify_token, CredentialLocation, CredentialStore, StoredCredential, TokenKind,
};
use crate::gateway_http::{self, GatewayHttpError, ACCOUNT_TIMEOUT};
use crate::vault::{self, SecureString};

pub const GATEWAY_BASE_URL: &str = api::DEFAULT_ZERO_X_BASE_URL;
const LOGIN_URL: &str = "https://zero-x.live/neuroncli/login/";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const AUTH_USAGE: &str =
    "Usage: neuron auth <login [--no-browser | --with-token] | logout | status>";

pub fn gateway_base_url() -> io::Result<String> {
    let base = env::var("NEURON_API_BASE").unwrap_or_else(|_| GATEWAY_BASE_URL.to_string());
    validate_gateway_base(&base)?;
    Ok(base)
}

fn validate_gateway_base(base: &str) -> io::Result<()> {
    let url = Url::parse(base).map_err(io::Error::other)?;
    let local = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
    );
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(io::Error::other("NEURON_API_BASE must be HTTPS or loopback HTTP, without credentials, query, or fragment"));
    }
    Ok(())
}

/// Where the active Zero-X credential came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSource {
    /// `NEURON_TOKEN` in the environment.
    EnvToken,
    /// The stored login.
    Stored(CredentialLocation),
}

impl CredentialSource {
    pub fn describe(self) -> String {
        match self {
            Self::EnvToken => "NEURON_TOKEN".to_string(),
            Self::Stored(location) => format!("stored login, {}", location.describe()),
        }
    }
}

/// A Zero-X credential ready to use, never printed.
#[derive(Debug, Clone)]
pub struct GatewayCredential {
    pub token: SecureString,
    pub kind: TokenKind,
    pub source: CredentialSource,
    pub expires_at: Option<u64>,
    pub plan: Option<String>,
}

impl GatewayCredential {
    fn from_stored(stored: StoredCredential, location: CredentialLocation) -> Option<Self> {
        let kind = classify_token(stored.token.expose()).ok()?;
        Some(Self {
            token: stored.token,
            kind,
            source: CredentialSource::Stored(location),
            expires_at: stored.expires_at,
            plan: stored.plan,
        })
    }

    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at.is_some_and(|expires_at| expires_at <= now)
    }
}

/// `NEURON_TOKEN` (`ses_` or `nzx_`), validated. An invalid value is an
/// error rather than silently ignored.
pub fn env_token() -> io::Result<Option<SecureString>> {
    let Ok(token) = env::var("NEURON_TOKEN") else {
        return Ok(None);
    };
    let token = token.trim();
    if token.is_empty() {
        return Ok(None);
    }
    classify_token(token).map_err(|error| {
        io::Error::new(io::ErrorKind::InvalidData, format!("NEURON_TOKEN: {error}"))
    })?;
    Ok(Some(SecureString::new(token.to_string())))
}

fn env_credential() -> io::Result<Option<GatewayCredential>> {
    Ok(env_token()?.map(|token| GatewayCredential {
        kind: classify_token(token.expose()).unwrap_or(TokenKind::Session),
        token,
        source: CredentialSource::EnvToken,
        expires_at: None,
        plan: None,
    }))
}

/// The stored login, read-only (no migration, never fails).
pub fn stored_credential() -> Option<GatewayCredential> {
    let loaded = CredentialStore::open_default().load()?;
    GatewayCredential::from_stored(loaded.credential, loaded.location)
}

/// The stored login for a session that is about to use it; migrates a
/// legacy `gateway.enc` on first use.
pub fn stored_credential_for_use() -> Option<GatewayCredential> {
    let loaded = CredentialStore::open_default().load_and_migrate()?;
    GatewayCredential::from_stored(loaded.credential, loaded.location)
}

/// `NEURON_TOKEN` first, then the stored login (read-only).
pub fn current_credential() -> io::Result<Option<GatewayCredential>> {
    Ok(env_credential()?.or_else(stored_credential))
}

/// Compatibility helper for callers that only need the token.
pub fn cached_gateway_token() -> io::Result<Option<SecureString>> {
    Ok(current_credential()?.map(|credential| credential.token))
}

fn interactive() -> bool {
    io::stdin().is_terminal() && io::stderr().is_terminal()
}

/// Credential for a model session: `NEURON_TOKEN`, else the stored login
/// (migrated if needed). An expired stored session, or no login at all,
/// prints an actionable message and, in an interactive terminal, offers to
/// start the browser login.
pub fn credential_for_session() -> io::Result<GatewayCredential> {
    if let Some(credential) = env_credential()? {
        return Ok(credential);
    }
    if let Some(credential) = stored_credential_for_use() {
        if !credential.is_expired(crate::clock::now_unix()) {
            return Ok(credential);
        }
        return recover_invalid_session();
    }
    if !interactive() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Sign in with `neuron auth login` (or `neuron auth login --with-token` for CI), or set NEURON_TOKEN",
        ));
    }
    eprintln!("Sign in to your Zero-X account to continue.");
    login(LoginOptions::default())
}

/// The session is dead: in a terminal, says so and offers to sign in again.
/// Otherwise (or when the offer is declined) returns the invalid-session
/// message as the error, which the caller prints exactly once.
pub fn recover_invalid_session() -> io::Result<GatewayCredential> {
    if interactive() {
        eprintln!("{}", api::INVALID_SESSION_MESSAGE);
        if confirm("Sign in again now? [Y/n] ") {
            return login(LoginOptions::default());
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not signed in; run `neuron auth login` when you are ready",
        ));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        api::INVALID_SESSION_MESSAGE,
    ))
}

fn confirm(prompt: &str) -> bool {
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    )
}

/// Session metadata from `GET /auth/session` (or `/api/account` for keys).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionInfo {
    pub plan: Option<String>,
    pub expires_at: Option<u64>,
    pub email: Option<String>,
}

/// Reads plan, expiry and email from a `/auth/session` or `/api/account`
/// body. Expiry comes from `expires_at`, or from `ttl_remaining_seconds`
/// for gateways that predate it.
pub fn parse_session_info(value: &Value, now: u64) -> SessionInfo {
    let plan = match value.get("plan") {
        Some(Value::String(plan)) if !plan.trim().is_empty() => Some(plan.trim().to_string()),
        Some(plan @ Value::Object(_)) => gateway_http::string_field(plan, "name")
            .or_else(|| gateway_http::string_field(plan, "id")),
        _ => None,
    };
    let expires_at = match value.get("expires_at") {
        Some(Value::String(text)) => crate::clock::parse_timestamp(text),
        Some(Value::Number(_)) => gateway_http::u64_field(value, "expires_at")
            .and_then(|stamp| crate::clock::parse_timestamp(&stamp.to_string())),
        _ => None,
    }
    .or_else(|| {
        gateway_http::u64_field(value, "ttl_remaining_seconds")
            .map(|remaining| now.saturating_add(remaining))
    });
    let email = gateway_http::string_field(value, "email").or_else(|| {
        value
            .get("user")
            .and_then(|user| gateway_http::string_field(user, "email"))
    });
    SessionInfo {
        plan,
        expires_at,
        email,
    }
}

/// Validates `token` with the server and returns its session metadata.
/// Session tokens use `GET /auth/session`; API keys (not accepted on
/// `/auth/*`) use `GET /api/account`.
pub fn fetch_session(
    base_url: &str,
    token: &SecureString,
    kind: TokenKind,
) -> Result<SessionInfo, GatewayHttpError> {
    let path = match kind {
        TokenKind::Session => "/auth/session",
        TokenKind::ApiKey => "/api/account",
    };
    let value = gateway_http::get_json(
        &gateway_http::account_url(base_url, path),
        Some(token.expose()),
        ACCOUNT_TIMEOUT,
    )?;
    Ok(parse_session_info(&value, crate::clock::now_unix()))
}

fn auth_url(callback: &str, state: &str) -> String {
    let mut url = Url::parse(LOGIN_URL).expect("fixed login URL");
    url.query_pairs_mut()
        .append_pair("callback_url", callback)
        .append_pair("state", state);
    url.into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginOptions {
    /// Open the sign-in page in the default browser (`--no-browser` clears it).
    pub open_browser: bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self { open_browser: true }
    }
}

/// Browser login through a loopback callback on an OS-assigned port.
pub fn login(options: LoginOptions) -> io::Result<GatewayCredential> {
    let base = gateway_base_url()?;
    // Bind before opening the browser.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let state = general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let url = auth_url(&format!("http://127.0.0.1:{port}/callback"), &state);
    if options.open_browser {
        eprintln!("Sign in to your Zero-X account in the browser.\n{url}");
        if webbrowser::open(&url).is_err() {
            eprintln!("Could not open a browser; open the URL above to continue.");
        }
    } else {
        eprintln!(
            "Open this URL in a browser on this machine to sign in:\n{url}\n\
             Over SSH, forward the callback port first (ssh -L {port}:127.0.0.1:{port} <host>), \
             or use `neuron auth login --with-token`."
        );
    }
    let store = CredentialStore::open_default();
    let token = accept_login(
        &listener,
        &state,
        port,
        Instant::now() + LOGIN_TIMEOUT,
        |token| {
            store
                .save(&StoredCredential::new(token.clone()))
                .map(|_| ())
                .map_err(io::Error::other)
        },
    )?;
    eprintln!("Zero-X sign-in received. Return to your terminal.");
    finish_login(&store, &base, token)
}

/// Waits for the browser's form POST on `listener` (non-blocking, so the
/// deadline is checked between polls), validates it, lets `persist` store
/// the token and answers the browser.
pub(crate) fn accept_login(
    listener: &TcpListener,
    state: &str,
    port: u16,
    deadline: Instant,
    mut persist: impl FnMut(&SecureString) -> io::Result<()>,
) -> io::Result<SecureString> {
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, address)) if address.ip().is_loopback() => {
                // Accepted sockets inherit the listener's non-blocking mode on
                // some platforms (Windows, BSDs). Reads must block, honouring
                // the timeouts set in receive_callback, or a request that has
                // not fully arrived yet fails with WouldBlock.
                let received = stream
                    .set_nonblocking(false)
                    .and_then(|()| receive_callback(&mut stream, state, port));
                match received {
                    Ok(token) => {
                        let token = SecureString::new(token);
                        if let Err(error) = persist(&token) {
                            respond(&mut stream, false);
                            return Err(error);
                        }
                        respond(&mut stream, true);
                        return Ok(token);
                    }
                    Err(error) => {
                        eprintln!("Rejected Neuron callback: {error}");
                        respond(&mut stream, false);
                    }
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Sign-in timed out. Run `neuron auth login` again",
    ))
}

/// Records session metadata after the token is stored and reports where.
fn finish_login(
    store: &CredentialStore,
    base_url: &str,
    token: SecureString,
) -> io::Result<GatewayCredential> {
    let kind = classify_token(token.expose()).map_err(io::Error::other)?;
    let mut stored = StoredCredential::new(token.clone());
    match fetch_session(base_url, &token, kind) {
        Ok(info) => {
            stored.expires_at = info.expires_at;
            stored.plan = info.plan;
        }
        Err(error) if error.is_invalid_session() => {
            let _ = store.clear();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Zero-X rejected the new credential: {error}"),
            ));
        }
        Err(error) => {
            eprintln!("warning: signed in, but the session details are unavailable ({error})");
        }
    }
    let location = store.save(&stored).map_err(io::Error::other)?;
    crate::gateway_catalog::invalidate_cache();
    eprintln!("{}", describe_login(&stored, location));
    Ok(GatewayCredential {
        token,
        kind,
        source: CredentialSource::Stored(location),
        expires_at: stored.expires_at,
        plan: stored.plan,
    })
}

fn describe_login(stored: &StoredCredential, location: CredentialLocation) -> String {
    let mut line = "Signed in to Zero-X".to_string();
    if let Some(plan) = &stored.plan {
        line.push_str(&format!(" (plan {plan})"));
    }
    if let Some(expires_at) = stored.expires_at {
        line.push_str(&format!(
            "; session expires {}",
            crate::clock::format_utc(expires_at)
        ));
    }
    line.push_str(&format!(". Login stored in the {}.", location.describe()));
    line
}

/// `neuron auth login --with-token`: reads a `ses_` or `nzx_` token from
/// stdin (one line), verifies it with the server when reachable and stores it.
fn login_with_token(input: &mut dyn BufRead, prompt: bool) -> io::Result<GatewayCredential> {
    if prompt {
        eprint!("Paste a Zero-X session token (ses_...) or API key (nzx_...): ");
        let _ = io::stderr().flush();
    }
    let mut line = Zeroizing::new(String::new());
    input.read_line(&mut line)?;
    let token = line.trim();
    classify_token(token).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let base = gateway_base_url()?;
    let store = CredentialStore::open_default();
    finish_login(&store, &base, SecureString::new(token.to_string()))
}

fn receive_callback(stream: &mut TcpStream, state: &str, port: u16) -> io::Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?.take(16_384));
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim_end() != "POST /callback HTTP/1.1" {
        return Err(io::Error::other("Expected POST /callback"));
    }
    let mut headers = std::collections::HashMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(io::Error::other("Incomplete callback headers"));
        }
        if line == "\r\n" {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| io::Error::other("Invalid header"))?;
        if headers
            .insert(name.to_ascii_lowercase(), value.trim().to_string())
            .is_some()
        {
            return Err(io::Error::other("Duplicate callback header"));
        }
    }
    if headers.get("host") != Some(&format!("127.0.0.1:{port}"))
        || !trusted_callback_origin(&headers)
        || !headers.get("content-type").is_some_and(|value| {
            value.split(';').next() == Some("application/x-www-form-urlencoded")
        })
        || headers.contains_key("transfer-encoding")
    {
        return Err(io::Error::other("Invalid callback origin or content type"));
    }
    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .filter(|length| *length <= 8192)
        .ok_or_else(|| io::Error::other("Invalid callback length"))?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let body = std::str::from_utf8(&body).map_err(io::Error::other)?;
    let mut parsed = Url::parse("http://localhost/").expect("fixed form URL");
    parsed.set_query(Some(body));
    let mut fields = std::collections::HashMap::new();
    for (name, value) in parsed.query_pairs() {
        if fields
            .insert(name.into_owned(), value.into_owned())
            .is_some()
        {
            return Err(io::Error::other("Duplicate callback field"));
        }
    }
    let received_state = fields
        .get("state")
        .ok_or_else(|| io::Error::other("Missing callback state"))?;
    if !constant_time_eq::constant_time_eq(received_state.as_bytes(), state.as_bytes()) {
        return Err(io::Error::other("Callback state mismatch"));
    }
    let token = fields
        .remove("session_token")
        .ok_or_else(|| io::Error::other("Missing session token"))?;
    // The browser handoff only ever carries CLI session tokens.
    if classify_token(&token) != Ok(TokenKind::Session) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid gateway session token",
        ));
    }
    Ok(token)
}

fn trusted_callback_origin(headers: &std::collections::HashMap<String, String>) -> bool {
    if headers.get("origin").is_some_and(|origin| {
        matches!(
            origin.as_str(),
            "https://zero-x.live" | "https://www.zero-x.live"
        )
    }) {
        return true;
    }
    headers.get("referer").is_some_and(|referer| {
        Url::parse(referer).is_ok_and(|url| {
            url.scheme() == "https"
                && matches!(url.host_str(), Some("zero-x.live" | "www.zero-x.live"))
        })
    })
}

fn respond(stream: &mut TcpStream, success: bool) {
    let (status, body) = if success {
        ("200 OK", "<!doctype html><title>Neuron connected</title><h1>Neuron connected</h1><p>Your Zero-X session is saved. Close this tab and return to your terminal.</p>")
    } else {
        ("400 Bad Request", "<!doctype html><title>Sign-in failed</title><h1>Sign-in failed</h1><p>Return to Zero-X and try the handoff again.</p>")
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\r\n{body}", body.len());
    let _ = stream.write_all(response.as_bytes());
}

/// Signs out: revokes the session on the server when possible and removes
/// every local copy. Always succeeds; problems are reported as warnings.
pub fn logout() {
    let store = CredentialStore::open_default();
    if let Some(loaded) = store.load() {
        let is_session = classify_token(loaded.credential.token.expose()) == Ok(TokenKind::Session);
        match gateway_base_url() {
            Ok(base) if is_session => match gateway_http::delete(
                &gateway_http::account_url(&base, "/auth/session"),
                Some(loaded.credential.token.expose()),
                ACCOUNT_TIMEOUT,
            ) {
                Ok(()) => eprintln!("Zero-X session revoked."),
                Err(error) if error.is_invalid_session() => {}
                Err(error) => eprintln!(
                    "warning: could not revoke the session on Zero-X ({error}); it expires on its own"
                ),
            },
            Ok(_) => {}
            Err(error) => eprintln!("warning: skipping server sign-out: {error}"),
        }
    }
    for warning in store.clear() {
        eprintln!("warning: {warning}");
    }
    crate::gateway_catalog::invalidate_cache();
    println!("Signed out of Zero-X.");
    if env::var("NEURON_TOKEN").is_ok_and(|token| !token.trim().is_empty()) {
        eprintln!(
            "NEURON_TOKEN is still set in this shell; unset it to stop using that credential."
        );
    }
}

/// `neuron auth status`: validates the credential with the server and prints
/// plan, expiry and today's usage.
fn status() -> Result<(), Box<dyn std::error::Error>> {
    let Some(credential) = env_credential()?.or_else(stored_credential_for_use) else {
        println!("Signed out of Zero-X. Run `neuron auth login`.");
        return Ok(());
    };
    let base = gateway_base_url()?;
    let now = crate::clock::now_unix();
    println!("Zero-X");
    println!("  Signed in        yes ({})", credential.source.describe());
    println!("  Gateway          {base}");
    let session = match fetch_session(&base, &credential.token, credential.kind) {
        Ok(info) => {
            if let CredentialSource::Stored(_) = credential.source {
                refresh_stored_metadata(&credential, &info);
            }
            println!("  Session          valid");
            info
        }
        Err(error) if error.is_invalid_session() => {
            recover_invalid_session()?;
            return Ok(());
        }
        Err(error) => {
            println!("  Session          not verified: {error}");
            SessionInfo {
                plan: credential.plan.clone(),
                expires_at: credential.expires_at,
                email: None,
            }
        }
    };
    if credential.kind == TokenKind::Session {
        println!(
            "  Session expires  {}",
            describe_expiry(session.expires_at, now)
        );
    }
    match crate::account::fetch_account(&base, &credential.token) {
        Ok(account) => {
            for line in crate::account::usage_lines(&account, now) {
                println!("{line}");
            }
        }
        Err(error) if error.is_invalid_session() => {
            recover_invalid_session()?;
        }
        Err(error) => {
            if let Some(plan) = &session.plan {
                println!("  Plan             {plan}");
            }
            println!("  Usage            unavailable: {error}");
        }
    }
    Ok(())
}

fn describe_expiry(expires_at: Option<u64>, now: u64) -> String {
    expires_at.map_or_else(
        || "not reported".to_string(),
        |expires_at| {
            format!(
                "{} ({})",
                crate::clock::format_utc(expires_at),
                crate::clock::describe_relative(expires_at, now)
            )
        },
    )
}

fn refresh_stored_metadata(credential: &GatewayCredential, info: &SessionInfo) {
    if credential.plan == info.plan && credential.expires_at == info.expires_at {
        return;
    }
    if credential.plan.is_some() && credential.plan != info.plan {
        // Model availability is per plan; refetch the catalog next time.
        crate::gateway_catalog::invalidate_cache();
    }
    let mut stored = StoredCredential::new(credential.token.clone());
    stored.plan.clone_from(&info.plan);
    stored.expires_at = info.expires_at;
    if let Err(error) = CredentialStore::open_default().save(&stored) {
        eprintln!("warning: could not update the stored session details: {error}");
    }
}

pub fn run_auth_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args.first().map(String::as_str) {
        Some("login") => {
            let mut options = LoginOptions::default();
            let mut with_token = false;
            for flag in &args[1..] {
                match flag.as_str() {
                    "--no-browser" => options.open_browser = false,
                    "--with-token" => with_token = true,
                    other => {
                        return Err(format!(
                            "unknown option for `neuron auth login`: {other}\n{AUTH_USAGE}"
                        )
                        .into())
                    }
                }
            }
            if with_token {
                let stdin = io::stdin();
                let prompt = stdin.is_terminal();
                login_with_token(&mut stdin.lock(), prompt)?;
            } else {
                login(options)?;
            }
        }
        Some("logout") if args.len() == 1 => logout(),
        Some("status") if args.len() == 1 => status()?,
        None | Some("help" | "--help" | "-h") => println!("{AUTH_USAGE}"),
        _ => return Err(AUTH_USAGE.into()),
    }
    Ok(())
}

pub fn check_trust(cwd: &std::path::Path) -> bool {
    let trust_file = vault::vault_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("trusted_dirs.txt");
    if fs::read_to_string(&trust_file).is_ok_and(|content| {
        content
            .lines()
            .any(|line| line.trim() == cwd.display().to_string())
    }) {
        return true;
    }
    println!(
        "\nNeuronCLI can read, write, and execute commands in:\n{}",
        cwd.display()
    );
    print!("Press 1 to trust this directory: ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok() && input.trim() == "1" {
        if let Some(parent) = trust_file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trust_file)
        {
            let _ = writeln!(file, "{}", cwd.display());
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_endpoint_rejects_unencrypted_remote_token_delivery() {
        assert!(validate_gateway_base("https://zero-x.live/v1").is_ok());
        assert!(validate_gateway_base("http://127.0.0.1:42137/v1").is_ok());
        assert!(validate_gateway_base("http://example.com/v1").is_err());
        assert!(validate_gateway_base("https://user:secret@example.com/v1").is_err());
        assert!(validate_gateway_base("https://example.com/v1?redirect=other").is_err());
    }

    #[test]
    fn callback_accepts_origin_or_same_site_referer() {
        let mut headers = std::collections::HashMap::new();
        headers.insert(
            "referer".to_string(),
            "https://zero-x.live/neuroncli/login/".to_string(),
        );
        assert!(trusted_callback_origin(&headers));
        headers.insert("referer".to_string(), "https://attacker.test/".to_string());
        assert!(!trusted_callback_origin(&headers));
    }

    fn callback(method: &str, origin: &str, body: &str) -> io::Result<String> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        write!(client, "{method} /callback HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: {origin}\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}", body.len())?;
        let (mut stream, _) = listener.accept()?;
        receive_callback(&mut stream, "expected-state", port)
    }

    #[test]
    fn handoff_requires_post_origin_state_and_gateway_token() {
        let token = "ses_012345678901234567890123456789";
        let body = format!("state=expected-state&session_token={token}");
        assert_eq!(
            callback("POST", "https://zero-x.live", &body).unwrap(),
            token
        );
        assert!(callback("GET", "https://zero-x.live", &body).is_err());
        assert!(callback("POST", "https://attacker.test", &body).is_err());
        assert!(callback(
            "POST",
            "https://zero-x.live",
            &body.replace("expected-state", "wrong-state")
        )
        .is_err());
        assert!(callback(
            "POST",
            "https://zero-x.live",
            &format!("{body}&state=expected-state")
        )
        .is_err());
        assert!(callback(
            "POST",
            "https://zero-x.live",
            "state=expected-state&session_token=sk-provider-secret"
        )
        .is_err());
        // API keys are never delivered through the browser handoff.
        assert!(callback(
            "POST",
            "https://zero-x.live",
            "state=expected-state&session_token=nzx_0123456789abcdefghij"
        )
        .is_err());
    }

    /// Drives the real acceptor through a non-blocking listener. The fake
    /// browser connects, waits, and only then sends its form POST, so a
    /// socket left non-blocking after `accept` would fail with WouldBlock
    /// instead of reading the request.
    #[test]
    fn loopback_acceptor_reads_a_delayed_browser_post_from_a_nonblocking_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let token = "ses_abcdefghijklmnopqrstuvwxyz012345";

        let browser = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            // Let the acceptor pick up the connection before any byte arrives.
            thread::sleep(Duration::from_millis(400));
            let body = format!("state=loopback-state&session_token={token}");
            write!(
                stream,
                "POST /callback HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://zero-x.live\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.flush().unwrap();
            thread::sleep(Duration::from_millis(100));
            stream.write_all(body.as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });

        let mut persisted = Vec::new();
        let received = accept_login(
            &listener,
            "loopback-state",
            port,
            Instant::now() + Duration::from_secs(20),
            |token| {
                persisted.push(token.expose().to_string());
                Ok(())
            },
        )
        .expect("acceptor should read the delayed POST");

        assert_eq!(received.expose(), token);
        assert_eq!(persisted, vec![token.to_string()]);
        let response = browser.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(response.contains("Neuron connected"));
    }

    #[test]
    fn acceptor_reports_persist_failures_to_the_browser() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body = "state=s&session_token=ses_012345678901234567890123456789";
            write!(stream, "POST /callback HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://zero-x.live\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            response
        });
        let error = accept_login(
            &listener,
            "s",
            port,
            Instant::now() + Duration::from_secs(20),
            |_| Err(io::Error::other("disk full")),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "disk full");
        assert!(browser.join().unwrap().starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn acceptor_times_out_without_a_callback() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let error = accept_login(&listener, "s", 1, Instant::now(), |_| Ok(())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn browser_url_encodes_random_loopback_callback() {
        let url = Url::parse(&auth_url("http://127.0.0.1:42137/callback", "abc-def")).unwrap();
        assert_eq!(url.path(), "/neuroncli/login/");
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["callback_url"], "http://127.0.0.1:42137/callback");
        assert_eq!(query["state"], "abc-def");
    }

    #[test]
    fn session_info_reads_expiry_plan_and_ttl_fallback() {
        let now = 1_000;
        let contract = serde_json::json!({
            "plan": "pro", "email": "dev@example.com", "expires_at": "2026-10-17T00:00:00Z"
        });
        assert_eq!(
            parse_session_info(&contract, now),
            SessionInfo {
                plan: Some("pro".into()),
                expires_at: Some(1_792_195_200),
                email: Some("dev@example.com".into()),
            }
        );
        let legacy = serde_json::json!({"plan": "free", "ttl_remaining_seconds": 86_399.5});
        assert_eq!(parse_session_info(&legacy, now).expires_at, Some(87_399));
        let account = serde_json::json!({
            "user": {"email": "ci@example.com"}, "plan": {"id": "max", "name": "Max"}
        });
        let info = parse_session_info(&account, now);
        assert_eq!(info.plan.as_deref(), Some("Max"));
        assert_eq!(info.email.as_deref(), Some("ci@example.com"));
        assert_eq!(info.expires_at, None);
    }

    #[test]
    fn with_token_rejects_values_that_are_not_zero_x_credentials() {
        let mut input = io::Cursor::new(b"sk-ant-api03-not-a-zero-x-token\n".to_vec());
        let error = login_with_token(&mut input, false).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let mut empty = io::Cursor::new(Vec::new());
        assert!(login_with_token(&mut empty, false).is_err());
    }

    #[test]
    fn credential_source_descriptions_never_contain_secrets() {
        assert_eq!(CredentialSource::EnvToken.describe(), "NEURON_TOKEN");
        assert_eq!(
            CredentialSource::Stored(CredentialLocation::Keychain).describe(),
            "stored login, OS keychain"
        );
    }
}
