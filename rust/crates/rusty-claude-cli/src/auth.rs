//! Zero-X sign-in: device-code login (`device_login.rs`), headless token
//! login, session lifecycle (`/auth/session`), logout and `neuron auth status`.

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};

use reqwest::Url;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::credentials::{
    classify_token, CredentialLocation, CredentialStore, StoredCredential, TokenKind,
};
use crate::device_login::{self, ClientInfo, HttpTransport, SystemClock, TerminalObserver};
use crate::gateway_http::{self, GatewayHttpError, ACCOUNT_TIMEOUT};
use crate::vault::SecureString;

pub const GATEWAY_BASE_URL: &str = api::DEFAULT_ZERO_X_BASE_URL;
const AUTH_USAGE: &str =
    "Usage: neuron login [--no-browser | --with-token] | neuron logout | neuron auth status\n\
     (`neuron auth login|logout|status` also work)";

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoginOptions {
    /// Try to open the sign-in page in a browser (`--no-browser` clears it).
    /// The link and code are printed either way.
    pub open_browser: bool,
}

impl Default for LoginOptions {
    fn default() -> Self {
        Self { open_browser: true }
    }
}

/// Device-code sign-in: prints a link and a short code, opens the browser
/// when this machine has one, and polls Zero-X until the code is approved on
/// <https://neuron.zero-x.live/device>. No local port is opened.
pub fn login(options: LoginOptions) -> io::Result<GatewayCredential> {
    let base = gateway_base_url()?;
    let allowed =
        device_login::browser_allowed(options.open_browser, &|name| env::var(name).ok(), &|path| {
            std::path::Path::new(path).exists()
        });
    let open_browser = |link: &str| allowed && webbrowser::open(link).is_ok();
    // Ctrl-C ends the process here; the full-screen UI passes its own cancel check.
    let clock = SystemClock::new(&|| false);
    let grant = device_login::run(
        &base,
        &HttpTransport,
        &clock,
        &TerminalObserver,
        &ClientInfo::current(),
        &open_browser,
    )?;
    let store = CredentialStore::open_default();
    finish_login(&store, &base, grant.token)
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

/// `neuron login …` and `neuron logout` are the short forms of
/// `neuron auth login …` and `neuron auth logout`: returns the arguments for
/// [`run_auth_command`] when `args` is one of them.
pub fn auth_alias(args: &[String]) -> Option<&[String]> {
    matches!(args.first().map(String::as_str), Some("login" | "logout")).then_some(args)
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

/// Asks once per directory whether to trust it. Trusting also lets the
/// directory's own settings files run hooks, start MCP servers and change
/// permissions; until then the config loader ignores those keys.
pub fn check_trust(cwd: &std::path::Path) -> bool {
    // Same decision as the config loader: trust store, environment, or a
    // `trustedRoots` entry in user settings.
    if runtime::ConfigLoader::default_for(cwd).workspace_is_trusted() {
        return true;
    }
    println!(
        "\nNeuronCLI can read, write, and execute commands in:\n{}\nTrusting it also lets this directory's settings files run hooks, start MCP servers and change permissions.",
        cwd.display()
    );
    print!("Press 1 to trust this directory: ");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok() && input.trim() == "1" {
        if let Err(error) = runtime::workspace_trust::record_trusted_workspace(cwd) {
            eprintln!(
                "warning: could not save trust for {}: {error}",
                cwd.display()
            );
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
    fn login_and_logout_work_without_the_auth_prefix() {
        let args = |list: &[&str]| list.iter().map(ToString::to_string).collect::<Vec<_>>();
        assert_eq!(auth_alias(&args(&["login"])), Some(&args(&["login"])[..]));
        assert_eq!(
            auth_alias(&args(&["login", "--no-browser"])),
            Some(&args(&["login", "--no-browser"])[..])
        );
        assert_eq!(auth_alias(&args(&["logout"])), Some(&args(&["logout"])[..]));
        assert_eq!(
            auth_alias(&args(&["status"])),
            None,
            "`neuron status` stays the REPL command"
        );
        assert_eq!(auth_alias(&args(&["prompt", "login"])), None);
        assert_eq!(auth_alias(&[]), None);
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
