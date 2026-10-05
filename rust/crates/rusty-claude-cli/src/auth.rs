use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use base64::{engine::general_purpose, Engine as _};
use rand::RngCore;
use reqwest::{blocking::Client, Url};

use crate::vault::{self, SecureString};

pub const GATEWAY_BASE_URL: &str = "https://zero-x.live/v1";
const LOGIN_URL: &str = "https://zero-x.live/neuroncli/login/";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

pub fn gateway_vault_path() -> PathBuf {
    vault::vault_path().with_file_name("gateway.enc")
}

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

fn validate_token(token: &str) -> io::Result<()> {
    if !token.starts_with("ses_")
        || !(24..=512).contains(&token.len())
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid gateway session token",
        ));
    }
    Ok(())
}

pub fn cached_gateway_token() -> io::Result<Option<SecureString>> {
    if let Ok(token) = env::var("NEURON_TOKEN") {
        if !token.trim().is_empty() {
            validate_token(token.trim())?;
            return Ok(Some(SecureString::new(token.trim().to_string())));
        }
    }
    let path = gateway_vault_path();
    if !path.exists() {
        return Ok(None);
    }
    let token = vault::decrypt_from_vault(&path).map_err(io::Error::other)?;
    validate_token(token.expose())?;
    Ok(Some(token))
}

pub fn ensure_gateway_token() -> io::Result<SecureString> {
    if let Some(token) = cached_gateway_token()? {
        return Ok(token);
    }
    if !io::stdin().is_terminal() {
        return Err(io::Error::other(
            "Sign in with `neuron auth login`, or set NEURON_TOKEN and NEURON_API_BASE",
        ));
    }
    login()
}

fn auth_url(callback: &str, state: &str) -> String {
    let mut url = Url::parse(LOGIN_URL).expect("fixed login URL");
    url.query_pairs_mut()
        .append_pair("callback_url", callback)
        .append_pair("state", state);
    url.into()
}

fn login() -> io::Result<SecureString> {
    // Bind before opening the browser; use an OS-assigned port.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let state = general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let url = auth_url(&format!("http://127.0.0.1:{port}/callback"), &state);
    eprintln!("Sign in to your Zero-X account in the browser.\n{url}");
    if webbrowser::open(&url).is_err() {
        eprintln!("Open the sign-in URL above to continue.");
    }
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, address)) if address.ip().is_loopback() => {
                match receive_callback(&mut stream, &state, port) {
                    Ok(token) => {
                        if let Err(error) = vault::encrypt_and_store(&token, &gateway_vault_path())
                        {
                            respond(&mut stream, false);
                            return Err(io::Error::other(error));
                        }
                        respond(&mut stream, true);
                        eprintln!("Zero-X gateway session saved. Return to your terminal.");
                        return Ok(SecureString::new(token));
                    }
                    Err(_) => respond(&mut stream, false),
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Sign-in timed out. Run `neuron auth login` again",
    ))
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
        || !headers.get("origin").is_some_and(|origin| {
            matches!(
                origin.as_str(),
                "https://zero-x.live" | "https://www.zero-x.live"
            )
        })
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
    validate_token(&token)?;
    Ok(token)
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

pub fn run_auth_command(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    match args {
        [command] if command == "login" => {
            login()?;
        }
        [command] if command == "logout" => {
            let token = cached_gateway_token()?;
            let path = gateway_vault_path();
            if path.exists() {
                fs::remove_file(path)?;
            }
            eprintln!(
                "Local gateway session removed. Clear NEURON_TOKEN if you set it in your shell."
            );
            if let Some(token) = token {
                let base = gateway_base_url()?;
                let endpoint = format!(
                    "{}/auth/session",
                    base.trim_end_matches('/').trim_end_matches("/v1")
                );
                Client::builder()
                    .timeout(Duration::from_secs(10))
                    .build()?
                    .delete(endpoint)
                    .bearer_auth(token.expose())
                    .send()?
                    .error_for_status()?;
            }
        }
        [command] if command == "status" => {
            eprintln!(
                "{}",
                if cached_gateway_token()?.is_some() {
                    "Gateway credential available. Use /status after connecting for session details."
                } else {
                    "Signed out. Run `neuron auth login`."
                }
            );
        }
        [] => eprintln!("Usage: neuron auth <login|logout|status>"),
        _ => return Err("Usage: neuron auth <login|logout|status>".into()),
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
    }

    #[test]
    fn browser_url_encodes_random_loopback_callback() {
        let url = Url::parse(&auth_url("http://127.0.0.1:42137/callback", "abc-def")).unwrap();
        assert_eq!(url.path(), "/neuroncli/login/");
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["callback_url"], "http://127.0.0.1:42137/callback");
        assert_eq!(query["state"], "abc-def");
    }
}
