//! `neuron auth login openrouter`: OpenRouter's documented OAuth PKCE flow
//! for third-party apps (<https://openrouter.ai/docs/use-cases/oauth-pkce>).
//!
//! 1. Open `https://openrouter.ai/auth?callback_url=…&code_challenge=…&code_challenge_method=S256&state=…`.
//! 2. OpenRouter redirects the browser to the loopback callback with `code`
//!    (and our `state`, checked here).
//! 3. `POST https://openrouter.ai/api/v1/auth/keys` with `code`,
//!    `code_verifier` and `code_challenge_method` returns `{ "key": … }`, a
//!    user-controlled API key, which the caller stores in the keychain.
//!
//! The listener binds 127.0.0.1 on a random port and gives up after
//! [`CALLBACK_TIMEOUT`]; codes expire after ten minutes on OpenRouter's side.

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use base64::Engine as _;
use rand::Rng as _;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

const AUTH_URL: &str = "https://openrouter.ai/auth";
const API_BASE: &str = "https://openrouter.ai/api/v1";
const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const KEY_LABEL: &str = "Neuron CLI";

const URL_SAFE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

fn random_string(length: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| char::from(URL_SAFE[rng.gen_range(0..URL_SAFE.len())]))
        .collect()
}

/// A PKCE verifier and its S256 challenge (RFC 7636).
pub struct Pkce {
    pub verifier: Zeroizing<String>,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Self {
        Self::from_verifier(random_string(64))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = s256_challenge(&verifier);
        Self {
            verifier: Zeroizing::new(verifier),
            challenge,
        }
    }
}

/// `BASE64URL-ENCODE(SHA256(ASCII(verifier)))` without padding.
pub fn s256_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The browser URL that starts the sign-in.
pub fn authorize_url(callback_url: &str, challenge: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(AUTH_URL).expect("static URL parses");
    url.query_pairs_mut()
        .append_pair("callback_url", callback_url)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("key_label", KEY_LABEL)
        .append_pair("state", state);
    url.to_string()
}

/// What one request to the loopback listener means.
#[derive(Debug, PartialEq, Eq)]
pub enum Callback {
    Code(String),
    /// Not the callback (a favicon request, a probe): keep waiting.
    Ignore,
    /// The callback, but unusable: stop with this message.
    Failed(String),
}

/// Reads the HTTP request line `GET /callback?code=…&state=… HTTP/1.1`.
pub fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Ignore;
    };
    let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
        return Callback::Ignore;
    };
    if url.path() != CALLBACK_PATH {
        return Callback::Ignore;
    }
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if let Some(error) = error {
        return Callback::Failed(format!("OpenRouter sign-in failed: {error}"));
    }
    let Some(code) = code.filter(|code| !code.is_empty()) else {
        return Callback::Failed("OpenRouter sign-in was cancelled (no code)".to_string());
    };
    if state.as_deref() != Some(expected_state) {
        return Callback::Failed(
            "OpenRouter sign-in answered with an unexpected state; nothing was stored".to_string(),
        );
    }
    Callback::Code(code)
}

fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let body = format!("<!doctype html><meta charset=utf-8><title>Neuron</title><p>{message}</p>");
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

fn read_request_line(stream: &mut TcpStream) -> io::Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !buffer.windows(2).any(|pair| pair == b"\r\n") && buffer.len() < 8192 {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let text = String::from_utf8_lossy(&buffer);
    Ok(text.lines().next().unwrap_or_default().to_string())
}

/// Waits on `listener` for the callback carrying `state`, up to `timeout`.
pub fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    timeout: Duration,
) -> Result<Zeroizing<String>, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let Ok(line) = read_request_line(&mut stream) else {
                    continue;
                };
                match parse_callback(&line, state) {
                    Callback::Code(code) => {
                        respond(
                            &mut stream,
                            "200 OK",
                            "Signed in to OpenRouter. You can close this tab and return to Neuron.",
                        );
                        return Ok(Zeroizing::new(code));
                    }
                    Callback::Ignore => respond(&mut stream, "404 Not Found", "Not found."),
                    Callback::Failed(message) => {
                        respond(&mut stream, "400 Bad Request", &message);
                        return Err(message);
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "timed out after {} minutes waiting for the OpenRouter sign-in",
                        timeout.as_secs() / 60
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Exchanges the code for a user API key at `{api_base}/auth/keys`.
pub fn exchange_code(
    api_base: &str,
    code: &str,
    verifier: &str,
) -> Result<Zeroizing<String>, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(api::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(format!("{}/auth/keys", api_base.trim_end_matches('/')))
        .json(&serde_json::json!({
            "code": code,
            "code_verifier": verifier,
            "code_challenge_method": "S256",
        }))
        .send()
        .map_err(|error| format!("could not reach OpenRouter: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "OpenRouter refused the sign-in code (HTTP {}); run `neuron auth login openrouter` again",
            status.as_u16()
        ));
    }
    let body = response
        .json::<serde_json::Value>()
        .map_err(|error| format!("unreadable answer from OpenRouter: {error}"))?;
    body.get("key")
        .and_then(serde_json::Value::as_str)
        .filter(|key| !key.is_empty())
        .map(|key| Zeroizing::new(key.to_string()))
        .ok_or_else(|| "OpenRouter's answer carried no key".to_string())
}

/// Runs the whole flow and returns the new key (not yet stored).
pub fn login(open_browser: bool) -> Result<Zeroizing<String>, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let callback_url = format!("http://localhost:{port}{CALLBACK_PATH}");
    let pkce = Pkce::generate();
    let state = random_string(32);
    let url = authorize_url(&callback_url, &pkce.challenge, &state);
    eprintln!("Sign in to OpenRouter in your browser to create a key for Neuron:\n  {url}");
    if open_browser && webbrowser::open(&url).is_err() {
        eprintln!("(Could not open a browser; open the link above.)");
    }
    eprintln!("Waiting for OpenRouter (up to 5 minutes)...");
    let code = wait_for_code(&listener, &state, CALLBACK_TIMEOUT)?;
    Ok(exchange_code(API_BASE, &code, &pkce.verifier)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_challenge_matches_the_rfc_7636_example() {
        // RFC 7636 appendix B.
        assert_eq!(
            s256_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let pkce = Pkce::generate();
        assert_eq!(pkce.verifier.len(), 64);
        assert_eq!(pkce.challenge, s256_challenge(&pkce.verifier));
        assert_ne!(Pkce::generate().verifier, pkce.verifier);
    }

    #[test]
    fn authorize_url_carries_the_documented_parameters() {
        let url = reqwest::Url::parse(&authorize_url(
            "http://localhost:4567/callback",
            "CHALLENGE",
            "STATE",
        ))
        .unwrap();
        assert_eq!(url.host_str(), Some("openrouter.ai"));
        assert_eq!(url.path(), "/auth");
        let pairs = url.query_pairs().into_owned().collect::<Vec<_>>();
        let get = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(get("callback_url"), Some("http://localhost:4567/callback"));
        assert_eq!(get("code_challenge"), Some("CHALLENGE"));
        assert_eq!(get("code_challenge_method"), Some("S256"));
        assert_eq!(get("state"), Some("STATE"));
    }

    #[test]
    fn callback_parsing_checks_path_state_and_code() {
        assert_eq!(
            parse_callback("GET /callback?code=abc&state=s1 HTTP/1.1", "s1"),
            Callback::Code("abc".to_string())
        );
        assert_eq!(
            parse_callback("GET /favicon.ico HTTP/1.1", "s1"),
            Callback::Ignore
        );
        assert_eq!(
            parse_callback("POST /callback HTTP/1.1", "s1"),
            Callback::Ignore
        );
        assert!(matches!(
            parse_callback("GET /callback?code=abc&state=evil HTTP/1.1", "s1"),
            Callback::Failed(message) if message.contains("state")
        ));
        assert!(matches!(
            parse_callback("GET /callback?code=abc HTTP/1.1", "s1"),
            Callback::Failed(_)
        ));
        assert!(matches!(
            parse_callback("GET /callback?state=s1 HTTP/1.1", "s1"),
            Callback::Failed(message) if message.contains("cancelled")
        ));
        assert!(matches!(
            parse_callback("GET /callback?error=access_denied HTTP/1.1", "s1"),
            Callback::Failed(message) if message.contains("access_denied")
        ));
    }

    fn browser_hit(port: u16, target: &'static str) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(stream, "GET {target} HTTP/1.1\r\nhost: localhost\r\n\r\n").unwrap();
            let mut response = String::new();
            let _ = stream.read_to_string(&mut response);
            response
        })
    }

    #[test]
    fn loopback_listener_ignores_strays_and_returns_the_code() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let stray = browser_hit(port, "/favicon.ico");
        std::thread::sleep(Duration::from_millis(200));
        let hit = browser_hit(port, "/callback?code=the-code&state=st");
        let code = wait_for_code(&listener, "st", Duration::from_secs(10)).unwrap();
        assert_eq!(code.as_str(), "the-code");
        assert!(stray.join().unwrap().starts_with("HTTP/1.1 404"));
        assert!(hit.join().unwrap().starts_with("HTTP/1.1 200"));
    }

    #[test]
    fn loopback_listener_times_out_and_rejects_a_wrong_state() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let error = wait_for_code(&listener, "st", Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("timed out"), "{error}");
        let port = listener.local_addr().unwrap().port();
        let hit = browser_hit(port, "/callback?code=c&state=other");
        let error = wait_for_code(&listener, "st", Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("state"), "{error}");
        assert!(hit.join().unwrap().starts_with("HTTP/1.1 400"));
    }

    #[test]
    fn code_exchange_posts_the_verifier_and_reads_the_key() {
        let (base, server) = crate::provider_auth::tests::fake_server(
            200,
            r#"{"key":"sk-or-v1-fromexchange","user_id":"u"}"#,
        );
        let key = exchange_code(&base, "the-code", "the-verifier").unwrap();
        assert_eq!(key.as_str(), "sk-or-v1-fromexchange");
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /auth/keys"), "{request}");
        let body: serde_json::Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["code"], "the-code");
        assert_eq!(body["code_verifier"], "the-verifier");
        assert_eq!(body["code_challenge_method"], "S256");

        let (base, server) = crate::provider_auth::tests::fake_server(403, r#"{"error":"x"}"#);
        let error = exchange_code(&base, "c", "v").unwrap_err();
        assert!(error.contains("403"), "{error}");
        server.join().unwrap();
    }
}
