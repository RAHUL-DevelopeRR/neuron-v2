use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::vault::{self, SecureString};

const CALLBACK_PORT: u16 = 19284;
const CALLBACK_URL: &str = "https://zero-x.live/neuroncli/callback";
const OPENROUTER_AUTH_URL: &str = "https://openrouter.ai/auth";
const OPENROUTER_EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";
const MAX_EXCHANGE_RETRIES: u32 = 3;
const EXCHANGE_TIMEOUT_SECS: u64 = 15;
const PKCE_SESSION_TIMEOUT_SECS: u64 = 300;

// ──────────────────────────── Error Types ────────────────────────────

#[derive(Debug)]
pub enum AuthError {
    SessionExpired,
    StateMismatch,
    MissingCode,
    ExchangeFailed(String),
    NetworkError(String),
    Timeout,
    InvalidKeyFormat(&'static str),
    InvalidKey,
    KeyRevoked,
    ValidationFailed(u16),
    VaultCorrupted(String),
    VaultDecryptFailed(String),
    IoError(io::Error),
    PortInUse(u16),
    BrowserFailed,
    Cancelled,
}

impl AuthError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NetworkError(_)
                | Self::Timeout
                | Self::ExchangeFailed(_)
                | Self::ValidationFailed(429 | 500..=599)
        )
    }

    fn user_message(&self) -> &str {
        match self {
            Self::SessionExpired => "Auth session expired. Please try again.",
            Self::StateMismatch => "Security check failed (CSRF). Please try again.",
            Self::InvalidKey => "API key is invalid. Please re-authenticate.",
            Self::KeyRevoked => "API key revoked. Re-authenticating...",
            Self::VaultDecryptFailed(_) => "Cannot decrypt credentials (different machine?).",
            Self::PortInUse(_) => "Auth port in use. Close other NeuronCLI instances.",
            Self::Cancelled => "No API key provided.",
            _ => "Authentication failed. Please try again.",
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_message())
    }
}

impl From<io::Error> for AuthError {
    fn from(e: io::Error) -> Self {
        Self::IoError(e)
    }
}

// ──────────────────────────── PKCE Session ────────────────────────────
//
// CSRF protection note: OpenRouter's PKCE flow does NOT echo back a `state`
// parameter. Instead, the `code_verifier` held exclusively in this process's
// memory serves as the CSRF guard — only the CLI instance that generated the
// challenge can successfully exchange the authorization code for a key.

struct PkceSession {
    verifier: String,
    challenge: String,
    created_at: Instant,
}

impl PkceSession {
    fn new() -> Self {
        let mut verifier_bytes = [0u8; 48];
        rand::thread_rng().fill_bytes(&mut verifier_bytes);
        let verifier = general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);

        let challenge = {
            let mut hasher = Sha256::new();
            hasher.update(verifier.as_bytes());
            general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
        };

        Self {
            verifier,
            challenge,
            created_at: Instant::now(),
        }
    }

    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > Duration::from_secs(PKCE_SESSION_TIMEOUT_SECS)
    }

    fn validate_callback(&self, code: &str) -> Result<(), AuthError> {
        if self.is_expired() {
            return Err(AuthError::SessionExpired);
        }
        if code.is_empty() {
            return Err(AuthError::MissingCode);
        }
        Ok(())
    }

    fn build_auth_url(&self) -> String {
        format!(
            "{}?callback_url={}&code_challenge={}&code_challenge_method=S256",
            OPENROUTER_AUTH_URL,
            url_encode(CALLBACK_URL),
            self.challenge
        )
    }
}

// ──────────────────────────── Key Validation ────────────────────────────

#[derive(Debug, PartialEq)]
enum KeyHealth {
    Valid,
    RateLimited,
}

fn validate_key_format(key: &str) -> Result<(), AuthError> {
    let k = key.trim();
    if k.len() < 20 {
        return Err(AuthError::InvalidKeyFormat("too short"));
    }
    if !k.starts_with("sk-or-") && !k.starts_with("sk-") {
        return Err(AuthError::InvalidKeyFormat("must start with sk-or- or sk-"));
    }
    Ok(())
}

fn validate_key_live(key: &str) -> Result<KeyHealth, AuthError> {
    validate_key_format(key)?;

    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| AuthError::NetworkError(e.to_string()))?;

    let res = client
        .get("https://openrouter.ai/api/v1/models")
        .header("Authorization", format!("Bearer {}", key.trim()))
        .send()
        .map_err(|e| {
            if e.is_timeout() {
                AuthError::Timeout
            } else {
                AuthError::NetworkError(e.to_string())
            }
        })?;

    match res.status().as_u16() {
        200 => Ok(KeyHealth::Valid),
        401 => Err(AuthError::InvalidKey),
        403 => Err(AuthError::KeyRevoked),
        429 => Ok(KeyHealth::RateLimited),
        code => Err(AuthError::ValidationFailed(code)),
    }
}

// ──────────────────────────── Exchange with Retry ────────────────────────────

#[derive(Deserialize)]
struct ExchangeResponse {
    key: String,
}

fn exchange_with_retry(code: &str, session: &PkceSession) -> Result<String, AuthError> {
    let mut last_err = AuthError::ExchangeFailed("no attempts".into());

    for attempt in 0..=MAX_EXCHANGE_RETRIES {
        if attempt > 0 {
            let delay = calculate_backoff(attempt);
            eprintln!(
                "  \x1b[90m[RETRY] Attempt {}/{} in {:.1}s...\x1b[0m",
                attempt + 1,
                MAX_EXCHANGE_RETRIES + 1,
                delay.as_secs_f64()
            );
            thread::sleep(delay);
        }

        match try_exchange(code, &session.verifier) {
            Ok(key) => return Ok(key),
            Err(e) if e.is_retryable() => {
                last_err = e;
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_err)
}

fn try_exchange(code: &str, verifier: &str) -> Result<String, AuthError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(EXCHANGE_TIMEOUT_SECS))
        .build()
        .map_err(|e| AuthError::NetworkError(e.to_string()))?;

    let payload = json!({
        "code": code,
        "code_verifier": verifier,
        "code_challenge_method": "S256"
    });

    let res = client
        .post(OPENROUTER_EXCHANGE_URL)
        .json(&payload)
        .send()
        .map_err(|e| {
            if e.is_timeout() {
                AuthError::Timeout
            } else {
                AuthError::NetworkError(e.to_string())
            }
        })?;

    let status = res.status().as_u16();
    if status == 200 {
        let exchange: ExchangeResponse = res
            .json()
            .map_err(|e| AuthError::ExchangeFailed(format!("bad response: {e}")))?;
        Ok(exchange.key)
    } else if matches!(status, 429 | 500..=599) {
        Err(AuthError::ExchangeFailed(format!(
            "status {status} (retryable)"
        )))
    } else {
        let body = res.text().unwrap_or_default();
        Err(AuthError::ExchangeFailed(format!(
            "status {status}: {body}"
        )))
    }
}

fn calculate_backoff(attempt: u32) -> Duration {
    let base_ms = 1000u64;
    let exponential = base_ms * 2u64.pow(attempt.saturating_sub(1));
    let jitter = (rand::random::<f64>() * 0.5 - 0.25) * exponential as f64;
    let total = (exponential as f64 + jitter).max(500.0) as u64;
    Duration::from_millis(total.min(30_000))
}

// ──────────────────────────── Callback Server ────────────────────────────

struct CallbackServer;

impl CallbackServer {
    fn serve(&self, sender: mpsc::Sender<Result<String, AuthError>>) {
        let listener = match TcpListener::bind(format!("127.0.0.1:{CALLBACK_PORT}")) {
            Ok(l) => l,
            Err(_) => {
                let _ = sender.send(Err(AuthError::PortInUse(CALLBACK_PORT)));
                return;
            }
        };

        listener.set_nonblocking(true).ok();
        let deadline = Instant::now() + Duration::from_secs(PKCE_SESSION_TIMEOUT_SECS);

        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, addr)) => {
                    if !addr.ip().is_loopback() {
                        continue;
                    }
                    match self.handle_request(stream) {
                        Ok(code) => {
                            let _ = sender.send(Ok(code));
                            return;
                        }
                        Err(_) => continue,
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(200));
                }
                Err(_) => break,
            }
        }
    }

    fn handle_request(&self, mut stream: TcpStream) -> Result<String, AuthError> {
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut reader = BufReader::new(stream.try_clone().map_err(AuthError::IoError)?);
        let mut request_line = String::new();
        reader
            .read_line(&mut request_line)
            .map_err(AuthError::IoError)?;

        // Handle OPTIONS (CORS preflight)
        if request_line.starts_with("OPTIONS") {
            let resp = "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\n\r\n";
            let _ = stream.write_all(resp.as_bytes());
            return Err(AuthError::MissingCode);
        }

        // Parse query params
        let params = parse_query_params(&request_line);
        let code = params.get("code").cloned().unwrap_or_default();

        if code.is_empty() {
            self.send_response(&mut stream, false, "No auth code received");
            return Err(AuthError::MissingCode);
        }

        self.send_response(&mut stream, true, "");
        Ok(code)
    }

    fn send_response(&self, stream: &mut TcpStream, success: bool, error_msg: &str) {
        let body = if success {
            "<html><head><style>\
            body { font-family: -apple-system, 'Segoe UI', Roboto, sans-serif; display: flex; justify-content: center; align-items: center; min-height: 100vh; margin: 0; background: linear-gradient(135deg, #0f0f23 0%, #1a1a3e 100%); color: #e0e0e0; }\
            .card { text-align: center; padding: 60px; background: rgba(255,255,255,0.05); border: 1px solid rgba(255,255,255,0.1); border-radius: 24px; backdrop-filter: blur(20px); }\
            h1 { color: #f0a028; font-size: 2em; }\
            p { color: #aaa; font-size: 1.1em; }\
            </style></head><body><div class=\"card\"><h1>&#10003; Neuron Connected</h1>\
            <p>Your OpenRouter API key has been provisioned.<br>You can close this tab and return to the terminal.</p></div></body></html>"
                .to_string()
        } else {
            format!("<html><body><h1>Error: {error_msg}</h1></body></html>")
        };
        let status = if success { "200 OK" } else { "400 Bad Request" };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nAccess-Control-Allow-Origin: *\r\n\r\n{body}"
        );
        let _ = stream.write_all(resp.as_bytes());
    }
}

fn parse_query_params(request_line: &str) -> std::collections::HashMap<String, String> {
    let mut params = std::collections::HashMap::new();
    if let Some(path) = request_line.split_whitespace().nth(1) {
        if let Some(query) = path.split('?').nth(1) {
            for pair in query.split('&') {
                let mut kv = pair.split('=');
                if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
                    params.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    params
}

fn url_encode(s: &str) -> String {
    s.replace(':', "%3A").replace('/', "%2F")
}

// ──────────────────────────── Main Entry Point ────────────────────────────

/// Engineering-grade credential resolution pipeline.
/// Replaces the old `ensure_api_key()`.
///
/// Resolution order:
///   1. $OPENROUTER_API_KEY env var
///   2. Encrypted vault (~/.neuroncli/vault.enc)
///   3. PKCE OAuth flow (with retry + backoff)
///   4. Manual key paste (fallback)
///   5. Fail with actionable error
pub fn ensure_api_key() -> Option<String> {
    match resolve_credential() {
        Ok(secure) => {
            let key = secure.expose().to_string();
            println!(
                "  \x1b[92m\x1b[1m[OK]\x1b[0m Key loaded (fingerprint: {})\x1b[0m",
                vault::key_fingerprint(&key)
            );
            Some(key)
        }
        Err(e) => {
            eprintln!("\n  \x1b[91m[AUTH ERROR] {}\x1b[0m\n", e.user_message());
            None
        }
    }
}

fn resolve_credential() -> Result<SecureString, AuthError> {
    // ── Stage 1: Environment variable ──
    if let Ok(env_key) = env::var("OPENROUTER_API_KEY") {
        if !env_key.is_empty() {
            validate_key_format(&env_key)?;
            return Ok(SecureString::new(env_key));
        }
    }

    // ── Stage 2: Encrypted vault ──
    let vpath = vault::vault_path();
    if vpath.exists() {
        match vault::decrypt_from_vault(&vpath) {
            Ok(key) => {
                // Periodic health check (once per 24h)
                if vault::needs_health_check() {
                    match validate_key_live(key.expose()) {
                        Ok(_) => vault::record_health_check(),
                        Err(AuthError::KeyRevoked) | Err(AuthError::InvalidKey) => {
                            eprintln!("  \x1b[93m[WARN] Stored key is revoked/invalid. Re-authenticating...\x1b[0m");
                            vault::delete_vault(&vpath);
                            // Fall through to PKCE
                            return run_pkce_flow();
                        }
                        Err(_) => {
                            // Network error — use cached key (graceful degradation)
                        }
                    }
                }
                return Ok(key);
            }
            Err(e) => {
                eprintln!("  \x1b[93m[WARN] Vault issue: {e}. Re-authenticating...\x1b[0m");
                vault::delete_vault(&vpath);
            }
        }
    }

    // ── Stage 3: PKCE OAuth flow ──
    run_pkce_flow()
}

fn run_pkce_flow() -> Result<SecureString, AuthError> {
    println!("\n  \x1b[96m\x1b[1m╔══════════════════════════════════════════════════╗");
    println!("  ║  Welcome to NeuronCLI!                            ║");
    println!("  ║  Free AI coding agent — one-time setup below.     ║");
    println!("  ╚══════════════════════════════════════════════════╝\x1b[0m");

    let session = PkceSession::new();
    let auth_url = session.build_auth_url();

    // Start callback server in background
    let (cb_tx, cb_rx) = mpsc::channel::<Result<String, AuthError>>();
    thread::spawn(move || {
        CallbackServer.serve(cb_tx);
    });

    // Open browser
    println!("\n  \x1b[96m\x1b[1m[AUTH]\x1b[0m Opening browser for OpenRouter login...");
    if webbrowser::open(&auth_url).is_err() {
        eprintln!("  \x1b[93m[WARN] Could not open browser automatically.\x1b[0m");
    }

    println!("  \x1b[90m──────────────────────────────────────────────\x1b[0m");
    println!("  \x1b[97mSign up / log in to OpenRouter in your browser.\x1b[0m");
    println!("  \x1b[97mAfter login, you'll be redirected back here.\x1b[0m");
    println!("  \x1b[90m──────────────────────────────────────────────\x1b[0m");
    println!("  \x1b[90mIf auto-redirect doesn't work:\x1b[0m");
    println!("  \x1b[90m1. Go to: \x1b[4mhttps://openrouter.ai/settings/keys\x1b[0m");
    println!("  \x1b[90m2. Click 'Create Key' -> copy the key\x1b[0m");
    println!("  \x1b[90m3. Paste it below\x1b[0m");
    println!("  \x1b[90m──────────────────────────────────────────────\x1b[0m\n");
    println!("  \x1b[93mPaste your API key below (or wait for auto-redirect):\x1b[0m");
    print!("  \x1b[93m> \x1b[0m");
    io::stdout().flush().unwrap();

    // ── Stage 3a: Race stdin paste vs callback ──
    let (input_tx, input_rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let mut line = String::new();
        if io::stdin().read_line(&mut line).is_ok() {
            let _ = input_tx.send(line.trim().to_string());
        }
    });

    let poll_limit = (PKCE_SESSION_TIMEOUT_SECS / 2) as usize;
    for _ in 0..poll_limit {
        thread::sleep(Duration::from_secs(2));

        // Check manual paste
        if let Ok(pasted) = input_rx.try_recv() {
            if !pasted.is_empty() {
                match validate_and_store(&pasted) {
                    Ok(secure) => return Ok(secure),
                    Err(e) => {
                        eprintln!("  \x1b[91m{}\x1b[0m", e.user_message());
                        // Continue waiting for callback
                    }
                }
            }
        }

        // Check callback
        if let Ok(result) = cb_rx.try_recv() {
            match result {
                Ok(code) => {
                    println!("\n  \x1b[92m[✓] Auto-redirect succeeded! Exchanging code...\x1b[0m");
                    let key = exchange_with_retry(&code, &session)?;
                    return validate_and_store(&key);
                }
                Err(AuthError::StateMismatch) => {
                    eprintln!("  \x1b[91m[SECURITY] CSRF state mismatch detected. Ignoring callback.\x1b[0m");
                }
                Err(e) if e.is_retryable() => continue,
                Err(e) => return Err(e),
            }
        }
    }

    Err(AuthError::Cancelled)
}

/// Validate key (format + optional live probe) then store in encrypted vault.
fn validate_and_store(key: &str) -> Result<SecureString, AuthError> {
    let trimmed = key.trim();
    validate_key_format(trimmed)?;

    // Live validation (best-effort — don't fail on network issues)
    match validate_key_live(trimmed) {
        Ok(KeyHealth::Valid) => {
            println!("  \x1b[92m[✓] Key validated successfully.\x1b[0m");
        }
        Ok(KeyHealth::RateLimited) => {
            println!("  \x1b[93m[!] Key is valid but currently rate-limited.\x1b[0m");
        }
        Err(AuthError::InvalidKey) => return Err(AuthError::InvalidKey),
        Err(AuthError::KeyRevoked) => return Err(AuthError::KeyRevoked),
        Err(AuthError::InvalidKeyFormat(msg)) => return Err(AuthError::InvalidKeyFormat(msg)),
        Err(_) => {
            // Network error — proceed with storing (graceful degradation)
            println!("  \x1b[93m[!] Could not verify key online. Storing anyway.\x1b[0m");
        }
    }

    // Store in encrypted vault
    let vpath = vault::vault_path();
    vault::encrypt_and_store(trimmed, &vpath).map_err(|e| AuthError::VaultCorrupted(e))?;
    vault::record_health_check();

    println!(
        "  \x1b[92m\x1b[1m[OK] API key encrypted and saved to {}\x1b[0m",
        vpath.display()
    );
    println!(
        "  \x1b[92m\x1b[1m     Fingerprint: {}\x1b[0m",
        vault::key_fingerprint(trimmed)
    );
    println!("  \x1b[92m\x1b[1m     You won't be asked again.\x1b[0m\n");

    Ok(SecureString::new(trimmed.to_string()))
}

// ──────────────────────────── Trust Check ────────────────────────────

pub fn check_trust(cwd: &std::path::Path) -> bool {
    let trust_file = vault::vault_path()
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("trusted_dirs.txt");

    if let Ok(content) = fs::read_to_string(&trust_file) {
        if content
            .lines()
            .any(|l| l.trim() == cwd.display().to_string())
        {
            return true;
        }
    }

    println!("\n  \x1b[38;2;240;160;40m\x1b[1mSecurity Check\x1b[0m");
    println!("  \x1b[2mNeuronCLI can read, write, and execute commands in:\x1b[0m");
    println!("  \x1b[1m{}\x1b[0m\n", cwd.display());
    print!("  \x1b[38;2;240;160;40mPress 1 to trust this directory:\x1b[0m ");
    let _ = io::stdout().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok() && input.trim() == "1" {
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&trust_file)
        {
            let _ = writeln!(f, "{}", cwd.display());
        }
        println!("  \x1b[38;2;45;140;60m\x1b[1m✓\x1b[0m Directory trusted.\n");
        return true;
    }

    println!("  \x1b[38;2;200;50;40m✗\x1b[0m Directory not trusted. Exiting.\n");
    false
}
