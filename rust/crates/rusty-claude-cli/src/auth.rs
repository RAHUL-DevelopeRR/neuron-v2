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
use crate::device_login::{
    self, ClientInfo, DeviceLoginObserver, HttpTransport, SystemClock, TerminalObserver,
};
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
    /// Known for a stored login the gateway reported it for.
    pub email: Option<String>,
    /// The account's display name, likewise.
    pub name: Option<String>,
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
            email: stored.email,
            name: stored.name,
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
        email: None,
        name: None,
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
    // Ctrl-C ends the process here; the full-screen UI passes its own cancel check.
    login_with(options, &TerminalObserver, &|| false).map(|(credential, _email)| credential)
}

/// [`login`] that shows the code through `observer` and gives up when `cancelled` turns
/// true. Also returns the account's email when the gateway sent it.
pub fn login_with(
    options: LoginOptions,
    observer: &dyn DeviceLoginObserver,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<(GatewayCredential, Option<String>)> {
    let base = gateway_base_url()?;
    let allowed =
        device_login::browser_allowed(options.open_browser, &|name| env::var(name).ok(), &|path| {
            std::path::Path::new(path).exists()
        });
    let open_browser = |link: &str| allowed && webbrowser::open(link).is_ok();
    let clock = SystemClock::new(cancelled);
    let grant = device_login::run(
        &base,
        &HttpTransport,
        &clock,
        observer,
        &ClientInfo::current(),
        &open_browser,
    )?;
    let store = CredentialStore::open_default();
    let credential = finish_login(&store, &base, grant.token)?;
    Ok((credential, grant.email))
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
            stored.email = info.email;
        }
        Err(error) if error.is_invalid_session() => {
            let _ = store.clear();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("Zero-X rejected the new credential: {error}"),
            ));
        }
        Err(error) => {
            runtime::ui_hooks::notice(&format!(
                "warning: signed in, but the session details are unavailable ({error})"
            ));
        }
    }
    let location = store.save(&stored).map_err(io::Error::other)?;
    crate::gateway_catalog::invalidate_cache();
    runtime::ui_hooks::notice(&describe_login(&stored, location));
    Ok(GatewayCredential {
        token,
        kind,
        source: CredentialSource::Stored(location),
        expires_at: stored.expires_at,
        plan: stored.plan,
        email: stored.email,
        name: stored.name,
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
    sign_out();
    // Stdout on the terminal; a UI that took over notices (and may own stdout) hears it.
    if runtime::ui_hooks::notice_handler_installed() {
        runtime::ui_hooks::notice("Signed out of Zero-X.");
    } else {
        println!("Signed out of Zero-X.");
    }
    warn_about_env_token();
}

/// [`logout`] without the closing line. Returns whether the server revoked a session.
fn sign_out() -> bool {
    let store = CredentialStore::open_default();
    let mut revoked = false;
    if let Some(loaded) = store.load() {
        let is_session = classify_token(loaded.credential.token.expose()) == Ok(TokenKind::Session);
        match gateway_base_url() {
            Ok(base) if is_session => match gateway_http::delete(
                &gateway_http::account_url(&base, "/auth/session"),
                Some(loaded.credential.token.expose()),
                ACCOUNT_TIMEOUT,
            ) {
                Ok(()) => {
                    revoked = true;
                    runtime::ui_hooks::notice("Zero-X session revoked.");
                }
                Err(error) if error.is_invalid_session() => {}
                Err(error) => runtime::ui_hooks::notice(&format!(
                    "warning: could not revoke the session on Zero-X ({error}); it expires on its own"
                )),
            },
            Ok(_) => {}
            Err(error) => {
                runtime::ui_hooks::notice(&format!("warning: skipping server sign-out: {error}"));
            }
        }
    }
    for warning in store.clear() {
        runtime::ui_hooks::notice(&format!("warning: {warning}"));
    }
    crate::gateway_catalog::invalidate_cache();
    revoked
}

/// After signing out: a `NEURON_TOKEN` in the environment still signs Neuron in.
fn warn_about_env_token() {
    if env::var("NEURON_TOKEN").is_ok_and(|token| !token.trim().is_empty()) {
        runtime::ui_hooks::notice(
            "NEURON_TOKEN is still set in this shell; unset it to stop using that credential.",
        );
    }
}

/// `neuron auth status`: validates the credential with the server and prints
/// plan, expiry and today's usage.
fn status() -> Result<(), Box<dyn std::error::Error>> {
    let Some(credential) = env_credential()?.or_else(stored_credential_for_use) else {
        println!("Signed out of Zero-X. Run `neuron auth login`.");
        std::process::exit(SIGNED_OUT_EXIT_CODE);
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

/// `neuron auth status --output-format json`: what [`status`] prints, as JSON, and
/// whether a usable login was found. Never includes the token.
fn status_json() -> Result<(Value, bool), Box<dyn std::error::Error>> {
    let Some(credential) = env_credential()?.or_else(stored_credential_for_use) else {
        return Ok((serde_json::json!({ "signed_in": false }), false));
    };
    let base = gateway_base_url()?;
    let mut report = serde_json::json!({
        "signed_in": true,
        "source": credential.source.describe(),
        "gateway": base,
        "plan": credential.plan,
        "expires_at": credential.expires_at,
    });
    match fetch_session(&base, &credential.token, credential.kind) {
        Ok(info) => {
            if let CredentialSource::Stored(_) = credential.source {
                refresh_stored_metadata(&credential, &info);
            }
            report["session"] = "valid".into();
            report["plan"] = info.plan.into();
            report["expires_at"] = info.expires_at.into();
            report["email"] = info.email.into();
        }
        Err(error) if error.is_invalid_session() => {
            report["signed_in"] = false.into();
            report["session"] = "invalid".into();
            report["error"] = api::INVALID_SESSION_MESSAGE.into();
            return Ok((report, false));
        }
        Err(error) => {
            report["session"] = "not_verified".into();
            report["error"] = error.to_string().into();
        }
    }
    match crate::account::fetch_account(&base, &credential.token) {
        Ok(account) => report["account"] = serde_json::to_value(account)?,
        Err(error) => report["account_error"] = error.to_string().into(),
    }
    Ok((report, true))
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
    // A missing email keeps the stored one.
    let email = info.email.clone().or_else(|| credential.email.clone());
    if credential.plan == info.plan
        && credential.expires_at == info.expires_at
        && credential.email == email
    {
        return;
    }
    if credential.plan.is_some() && credential.plan != info.plan {
        // Model availability is per plan; refetch the catalog next time.
        crate::gateway_catalog::invalidate_cache();
    }
    let mut stored = StoredCredential::new(credential.token.clone());
    stored.plan.clone_from(&info.plan);
    stored.expires_at = info.expires_at;
    stored.email = email;
    stored.name.clone_from(&credential.name);
    if let Err(error) = CredentialStore::open_default().save(&stored) {
        eprintln!("warning: could not update the stored session details: {error}");
    }
}

/// Global flags that take a value (`parse_args` reads them).
const GLOBAL_VALUE_FLAGS: [&str; 7] = [
    "--model",
    "--output-format",
    "--permission-mode",
    "--base-commit",
    "--reasoning-effort",
    "--allowedTools",
    "--allowed-tools",
];

/// The account commands: `auth`, `login`, `logout`, `usage` and `upgrade`. `neuron login …`
/// and `neuron logout` are the short forms of `neuron auth login …` and `neuron auth logout`.
#[derive(Debug, PartialEq, Eq)]
pub struct AccountCommand {
    pub name: String,
    /// What follows the command, then a `--output-format` given before it.
    pub args: Vec<String>,
}

/// Finds an account command after any global flags (`neuron --output-format json auth
/// status`). A `--output-format` before it moves to its arguments; the other global flags
/// don't apply to it and are dropped. `None` when the first word is not one, such as a
/// prompt, and when what follows is not what the command takes: an unquoted prompt that
/// starts with a command's name (`neuron --model m login to the server and fix it`) is
/// left to the argument parser, which asks for quotes.
pub fn account_command(args: &[String]) -> Option<AccountCommand> {
    let mut output_format = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if !arg.starts_with('-') {
            if !matches!(
                arg.as_str(),
                "auth" | "login" | "logout" | "usage" | "upgrade"
            ) {
                return None;
            }
            let mut rest = args[index + 1..].to_vec();
            rest.extend(output_format);
            if !takes_arguments(arg, &rest) {
                return None;
            }
            return Some(AccountCommand {
                name: arg.clone(),
                args: rest,
            });
        }
        // `-p`, `--print` and the help and version flags end the search: what follows is
        // theirs.
        if matches!(
            arg.as_str(),
            "-p" | "--print" | "--help" | "-h" | "--version" | "-V"
        ) {
            return None;
        }
        let takes_value = GLOBAL_VALUE_FLAGS.contains(&arg.as_str());
        if arg == "--output-format" || arg.starts_with("--output-format=") {
            output_format.extend(
                args[index..]
                    .iter()
                    .take(1 + usize::from(takes_value))
                    .cloned(),
            );
        }
        index += 1 + usize::from(takes_value);
    }
    None
}

/// Whether `args` are arguments account command `name` takes. A bad `--output-format`
/// value counts: the command itself reports it.
fn takes_arguments(name: &str, args: &[String]) -> bool {
    let Ok((args, _)) = take_output_format(args) else {
        return true;
    };
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let login_flags = |flags: &[&str]| {
        flags
            .iter()
            .all(|flag| matches!(*flag, "--no-browser" | "--with-token"))
    };
    match (name, words.as_slice()) {
        ("login", flags) | ("auth", ["login", flags @ ..]) => login_flags(flags),
        ("logout" | "usage" | "upgrade", [])
        | ("upgrade", ["--no-browser"])
        | ("auth", [] | ["help" | "--help" | "-h" | "status" | "logout"]) => true,
        _ => false,
    }
}

/// Runs an account command found by [`account_command`].
pub fn run_account_command(command: &AccountCommand) -> Result<(), Box<dyn std::error::Error>> {
    match command.name.as_str() {
        "auth" => run_auth_command(&command.args),
        "login" | "logout" => {
            let mut args = vec![command.name.clone()];
            args.extend(command.args.iter().cloned());
            run_auth_command(&args)
        }
        "usage" => {
            let (args, json) = take_output_format(&command.args)?;
            crate::account::run_usage_command(&args, json)
        }
        _ => {
            let (args, json) = take_output_format(&command.args)?;
            crate::account::run_upgrade_command(&args, json)
        }
    }
}

/// Prints an account command's `--output-format json` result.
pub(crate) fn print_json(report: &Value) -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

/// `neuron login --output-format json`: who is signed in now. Never includes the token.
fn login_json(credential: &GatewayCredential) -> Value {
    serde_json::json!({
        "type": "login",
        "signed_in": true,
        "source": credential.source.describe(),
        "plan": credential.plan,
        "expires_at": credential.expires_at,
        "email": credential.email,
    })
}

/// Takes `--output-format json|text` (or `--output-format=…`) out of `args`.
fn take_output_format(args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut rest = Vec::new();
    let mut json = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if arg == "--output-format" {
            Some(iter.next().map_or("", String::as_str))
        } else {
            arg.strip_prefix("--output-format=")
        };
        match value {
            Some("json") => json = true,
            Some("text") => json = false,
            Some(other) => {
                return Err(format!(
                    "unsupported value for --output-format: {other} (expected text or json)"
                ))
            }
            None => rest.push(arg.clone()),
        }
    }
    Ok((rest, json))
}

/// The exit code of `neuron auth status` when signed out, so scripts can tell.
const SIGNED_OUT_EXIT_CODE: i32 = 1;

pub fn run_auth_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (args, json) = take_output_format(args)?;
    if json && args.first().map(String::as_str) == Some("status") && args.len() == 1 {
        let (report, signed_in) = status_json()?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        if !signed_in {
            std::process::exit(SIGNED_OUT_EXIT_CODE);
        }
        return Ok(());
    }
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
            let credential = if with_token {
                let stdin = io::stdin();
                let prompt = stdin.is_terminal();
                login_with_token(&mut stdin.lock(), prompt)?
            } else {
                login(options)?
            };
            // The sign-in steps and the result line went to stderr.
            if json {
                print_json(&login_json(&credential))?;
            }
        }
        Some("logout") if args.len() == 1 && json => {
            let revoked = sign_out();
            warn_about_env_token();
            print_json(&serde_json::json!({
                "type": "logout",
                "signed_out": true,
                "session_revoked": revoked,
            }))?;
        }
        Some("logout") if args.len() == 1 => logout(),
        Some("status") if args.len() == 1 => status()?,
        None | Some("help" | "--help" | "-h") if json => {
            print_json(&serde_json::json!({ "kind": "help", "message": AUTH_USAGE }))?;
        }
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
    let answered = io::stdin().read_line(&mut input);
    if answered.is_ok() && input.trim() == "1" {
        if let Err(error) = runtime::workspace_trust::record_trusted_workspace(cwd) {
            eprintln!(
                "warning: could not save trust for {}: {error}",
                cwd.display()
            );
        }
        return true;
    }
    // The caller exits; say why rather than stopping silently.
    println!();
    eprintln!("{}", untrusted_reason(matches!(answered, Ok(0) | Err(_))));
    false
}

/// Why Neuron stopped at the trust question: nobody could answer it (no input), or
/// the answer was not 1.
fn untrusted_reason(no_input: bool) -> &'static str {
    if no_input {
        "Not trusted: there was no input to answer the trust question. Run neuron in a terminal \
to trust this directory, or add it to `trustedRoots` in your user settings."
    } else {
        "Not trusted, so Neuron stopped. Run neuron again and press 1 to trust this directory."
    }
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

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn command(name: &str, args: &[&str]) -> Option<AccountCommand> {
        Some(AccountCommand {
            name: name.to_string(),
            args: strings(args),
        })
    }

    #[test]
    fn login_and_logout_work_without_the_auth_prefix() {
        assert_eq!(account_command(&strings(&["login"])), command("login", &[]));
        assert_eq!(
            account_command(&strings(&["login", "--no-browser"])),
            command("login", &["--no-browser"])
        );
        assert_eq!(
            account_command(&strings(&["logout"])),
            command("logout", &[])
        );
        assert_eq!(
            account_command(&strings(&["status"])),
            None,
            "`neuron status` stays the REPL command"
        );
        assert_eq!(account_command(&strings(&["prompt", "login"])), None);
        assert_eq!(account_command(&[]), None);
    }

    #[test]
    fn account_commands_are_found_after_global_flags_and_keep_the_output_format() {
        assert_eq!(
            account_command(&strings(&["--output-format", "json", "auth", "status"])),
            command("auth", &["status", "--output-format", "json"])
        );
        assert_eq!(
            account_command(&strings(&["auth", "status", "--output-format=json"])),
            command("auth", &["status", "--output-format=json"])
        );
        assert_eq!(
            account_command(&strings(&[
                "--model",
                "auth",
                "--dangerously-skip-permissions",
                "login"
            ])),
            command("login", &[]),
            "a flag's value is not the command"
        );
        assert_eq!(
            account_command(&strings(&["--model", "opus", "fix the auth bug"])),
            None
        );
        assert_eq!(account_command(&strings(&["-p", "auth", "status"])), None);
        assert_eq!(account_command(&strings(&["--help", "auth"])), None);
    }

    #[test]
    fn an_unquoted_prompt_that_starts_with_a_command_name_is_not_the_command() {
        let prompt = |first: &str| {
            let mut args = strings(&["--model", "m", first]);
            args.extend(strings(&["to", "the", "server", "and", "fix", "it"]));
            args
        };
        for name in ["auth", "login", "logout", "usage", "upgrade"] {
            assert_eq!(account_command(&prompt(name)), None, "{name}");
        }
        // The commands themselves, with what they take, still are.
        assert_eq!(
            account_command(&strings(&["--model", "m", "login"])),
            command("login", &[])
        );
        assert_eq!(
            account_command(&strings(&[
                "login",
                "--with-token",
                "--output-format",
                "json"
            ])),
            command("login", &["--with-token", "--output-format", "json"])
        );
        assert_eq!(
            account_command(&strings(&["--output-format", "json", "usage"])),
            command("usage", &["--output-format", "json"])
        );
        assert_eq!(
            account_command(&strings(&["upgrade", "--no-browser"])),
            command("upgrade", &["--no-browser"])
        );
        assert_eq!(
            account_command(&strings(&["auth", "login", "--no-browser"])),
            command("auth", &["login", "--no-browser"])
        );
        assert_eq!(account_command(&strings(&["auth"])), command("auth", &[]));
        assert_eq!(account_command(&strings(&["logout", "now"])), None);
        assert_eq!(account_command(&strings(&["auth", "status", "now"])), None);
        // A bad format value is the command's to report.
        assert_eq!(
            account_command(&strings(&["usage", "--output-format", "yaml"])),
            command("usage", &["--output-format", "yaml"])
        );
    }

    #[test]
    fn login_json_names_the_account_but_never_the_token() {
        let credential = GatewayCredential {
            token: SecureString::new("ses_abcdefghijklmnopqrstuvwxyz012345".to_string()),
            kind: TokenKind::Session,
            source: CredentialSource::Stored(CredentialLocation::Keychain),
            expires_at: Some(4_070_908_800),
            plan: Some("free".to_string()),
            email: Some("dev@example.com".to_string()),
            name: None,
        };
        let report = login_json(&credential);
        assert_eq!(report["type"], "login");
        assert_eq!(report["signed_in"], true);
        assert_eq!(report["plan"], "free");
        assert_eq!(report["expires_at"], 4_070_908_800_u64);
        assert!(!report.to_string().contains("ses_"));
    }

    #[test]
    fn the_output_format_is_taken_out_of_auth_arguments() {
        assert_eq!(
            take_output_format(&strings(&["status", "--output-format", "json"])),
            Ok((strings(&["status"]), true))
        );
        assert_eq!(
            take_output_format(&strings(&["--output-format=text", "status"])),
            Ok((strings(&["status"]), false))
        );
        assert!(take_output_format(&strings(&["status", "--output-format", "yaml"])).is_err());
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
