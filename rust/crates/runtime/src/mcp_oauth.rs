//! OAuth for remote (HTTP/SSE) MCP servers, after the MCP authorization specification
//! (2025-06-18): a `401` names the protected-resource metadata (RFC 9728), which names
//! the authorization server, whose metadata (RFC 8414) gives the endpoints. The client
//! registers itself when the server allows it (RFC 7591), signs in with the
//! authorization-code grant and PKCE (S256) through a loopback redirect on 127.0.0.1,
//! and sends the RFC 8707 `resource` parameter on every authorization and token request.
//!
//! Tokens live in a [`McpTokenStore`] the host installs ([`set_mcp_token_store`]; the
//! CLI uses the OS keychain). They are only ever sent to the server whose canonical URI
//! they were issued for.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::oauth::{base64url_encode, code_challenge_s256, parse_oauth_callback_request_target};

/// Seconds before expiry at which an access token is refreshed instead of used.
const REFRESH_MARGIN_SECS: u64 = 60;
/// How long `login` waits for the browser to come back.
const LOGIN_WAIT: Duration = Duration::from_secs(300);
const CLIENT_NAME: &str = "Neuron";

/// Tokens for one remote MCP server, with what refreshing them needs.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpOAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub token_endpoint: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    /// The canonical URI of the MCP server the tokens are for.
    pub resource: String,
    #[serde(default)]
    pub scope: Option<String>,
}

impl fmt::Debug for McpOAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpOAuthTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("expires_at", &self.expires_at)
            .field("token_endpoint", &self.token_endpoint)
            .field("client_id", &self.client_id)
            .field("resource", &self.resource)
            .finish_non_exhaustive()
    }
}

/// Where MCP OAuth tokens are kept: one secret string per key.
pub trait McpTokenStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<String>, String>;
    fn save(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

static TOKEN_STORE: RwLock<Option<Arc<dyn McpTokenStore>>> = RwLock::new(None);

/// Installs the store remote MCP servers read their tokens from (`None`: no tokens).
pub fn set_mcp_token_store(store: Option<Arc<dyn McpTokenStore>>) {
    *TOKEN_STORE.write().unwrap_or_else(PoisonError::into_inner) = store;
}

pub(crate) fn token_store() -> Option<Arc<dyn McpTokenStore>> {
    TOKEN_STORE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[must_use]
pub fn mcp_token_key(server_name: &str) -> String {
    format!("mcp-oauth:{server_name}")
}

/// The stored tokens for `server_name`, when they were issued for `resource`.
#[must_use]
pub fn load_mcp_tokens(server_name: &str, resource: &str) -> Option<McpOAuthTokens> {
    let text = token_store()?.load(&mcp_token_key(server_name)).ok()??;
    let tokens = serde_json::from_str::<McpOAuthTokens>(&text).ok()?;
    (tokens.resource == resource).then_some(tokens)
}

pub fn save_mcp_tokens(server_name: &str, tokens: &McpOAuthTokens) -> Result<(), String> {
    let store = token_store().ok_or_else(|| "no MCP token store is available".to_string())?;
    let text = serde_json::to_string(tokens).map_err(|error| error.to_string())?;
    store.save(&mcp_token_key(server_name), &text)
}

pub fn clear_mcp_tokens(server_name: &str) -> Result<(), String> {
    match token_store() {
        Some(store) => store.delete(&mcp_token_key(server_name)),
        None => Ok(()),
    }
}

/// The canonical URI of an MCP server (RFC 8707): lowercase scheme and host, no
/// fragment or query, no trailing slash on an empty path.
pub fn canonical_resource_uri(url: &str) -> Result<String, String> {
    let mut parsed = Url::parse(url).map_err(|error| format!("invalid URL {url}: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("{url} is not an http(s) URL"));
    }
    parsed.set_fragment(None);
    parsed.set_query(None);
    let text = parsed.to_string();
    Ok(if parsed.path() == "/" {
        text.trim_end_matches('/').to_string()
    } else {
        text
    })
}

/// A parsed `WWW-Authenticate` challenge: its scheme and parameters.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WwwAuthenticate {
    pub scheme: String,
    pub params: BTreeMap<String, String>,
}

impl WwwAuthenticate {
    #[must_use]
    pub fn resource_metadata(&self) -> Option<&str> {
        self.params.get("resource_metadata").map(String::as_str)
    }
}

/// Parses the first challenge of a `WWW-Authenticate` header
/// (`Bearer realm="x", resource_metadata="https://..."`).
#[must_use]
pub fn parse_www_authenticate(header: &str) -> WwwAuthenticate {
    let header = header.trim();
    let (scheme, mut rest) = header
        .split_once(char::is_whitespace)
        .map_or((header, ""), |(scheme, rest)| (scheme, rest));
    let mut params = BTreeMap::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c == ',' || c.is_whitespace());
        let Some((name, after)) = rest.split_once('=') else {
            break;
        };
        let name = name.trim().to_ascii_lowercase();
        let after = after.trim_start();
        let (value, remaining) = if let Some(quoted) = after.strip_prefix('"') {
            let mut value = String::new();
            let mut chars = quoted.char_indices();
            let mut end = quoted.len();
            while let Some((index, c)) = chars.next() {
                match c {
                    '\\' => {
                        if let Some((_, escaped)) = chars.next() {
                            value.push(escaped);
                        }
                    }
                    '"' => {
                        end = index + 1;
                        break;
                    }
                    other => value.push(other),
                }
            }
            (value, &quoted[end.min(quoted.len())..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim().to_string(), &after[end..])
        };
        // A bare word after a parameter starts another challenge: stop there.
        if name.contains(char::is_whitespace) {
            break;
        }
        params.insert(name, value);
        rest = remaining;
    }
    WwwAuthenticate {
        scheme: scheme.to_string(),
        params,
    }
}

fn origin_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn url_path(url: &Url) -> String {
    let path = url.path().trim_end_matches('/');
    path.to_string()
}

/// Where RFC 9728 puts the protected-resource metadata of `server_url`: the
/// path-aware well-known URI first, then the root one.
pub fn protected_resource_metadata_urls(server_url: &str) -> Result<Vec<String>, String> {
    let url = Url::parse(server_url).map_err(|error| error.to_string())?;
    let origin = origin_of(&url);
    let path = url_path(&url);
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!(
            "{origin}/.well-known/oauth-protected-resource{path}"
        ));
    }
    urls.push(format!("{origin}/.well-known/oauth-protected-resource"));
    Ok(urls)
}

/// Where the metadata of the authorization server `issuer` may be: RFC 8414 with the
/// path inserted, then `OpenID Connect` discovery (inserted, then appended).
pub fn authorization_server_metadata_urls(issuer: &str) -> Result<Vec<String>, String> {
    let url = Url::parse(issuer).map_err(|error| error.to_string())?;
    let origin = origin_of(&url);
    let path = url_path(&url);
    Ok(if path.is_empty() {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server"),
            format!("{origin}/.well-known/openid-configuration"),
        ]
    } else {
        vec![
            format!("{origin}/.well-known/oauth-authorization-server{path}"),
            format!("{origin}/.well-known/openid-configuration{path}"),
            format!("{origin}{path}/.well-known/openid-configuration"),
        ]
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProtectedResourceMetadata {
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub authorization_servers: Vec<String>,
    #[serde(default)]
    pub scopes_supported: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthorizationServerMetadata {
    #[serde(default)]
    pub issuer: Option<String>,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(default)]
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Option<Vec<String>>,
}

impl AuthorizationServerMetadata {
    /// Endpoints of a server that publishes no metadata (the 2025-03-26 fallback):
    /// `/authorize`, `/token` and `/register` at the origin.
    fn defaults_for(origin: &str) -> Self {
        Self {
            issuer: Some(origin.to_string()),
            authorization_endpoint: format!("{origin}/authorize"),
            token_endpoint: format!("{origin}/token"),
            registration_endpoint: Some(format!("{origin}/register")),
            code_challenge_methods_supported: None,
        }
    }

    fn check(&self) -> Result<(), String> {
        require_secure(&self.authorization_endpoint)?;
        require_secure(&self.token_endpoint)?;
        if let Some(registration) = &self.registration_endpoint {
            require_secure(registration)?;
        }
        match &self.code_challenge_methods_supported {
            Some(methods) if !methods.iter().any(|method| method == "S256") => {
                Err("the authorization server does not support PKCE with S256".to_string())
            }
            _ => Ok(()),
        }
    }
}

/// Authorization endpoints must use HTTPS; plain HTTP is accepted on loopback only.
fn require_secure(endpoint: &str) -> Result<(), String> {
    let url = Url::parse(endpoint).map_err(|error| format!("invalid URL {endpoint}: {error}"))?;
    let loopback = matches!(
        url.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    if url.scheme() == "https" || (url.scheme() == "http" && loopback) {
        Ok(())
    } else {
        Err(format!("{endpoint} is not an HTTPS endpoint"))
    }
}

/// A PKCE verifier (43 base64url characters from 32 random bytes) and its S256
/// challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPkce {
    pub verifier: String,
    pub challenge: String,
}

#[must_use]
pub fn generate_mcp_pkce() -> McpPkce {
    let verifier = random_token();
    McpPkce {
        challenge: code_challenge_s256(&verifier),
        verifier,
    }
}

fn random_token() -> String {
    let mut bytes = [0_u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64url_encode(&bytes)
}

/// The browser URL of an authorization-code request with PKCE and `resource`.
#[must_use]
pub fn build_authorization_url(
    metadata: &AuthorizationServerMetadata,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    pkce: &McpPkce,
    resource: &str,
    scope: Option<&str>,
) -> String {
    let Ok(mut url) = Url::parse(&metadata.authorization_endpoint) else {
        return metadata.authorization_endpoint.clone();
    };
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("state", state)
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("resource", resource);
        if let Some(scope) = scope {
            query.append_pair("scope", scope);
        }
    }
    url.to_string()
}

/// A token endpoint's answer.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl fmt::Debug for TokenResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenResponse")
            .field("token_type", &self.token_type)
            .field("expires_in", &self.expires_in)
            .finish_non_exhaustive()
    }
}

/// Parses a token endpoint body: tokens, or the OAuth error it names.
pub fn parse_token_response(body: &str) -> Result<TokenResponse, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|_| "the token endpoint did not answer JSON")?;
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        let description = value
            .get("error_description")
            .and_then(Value::as_str)
            .map(|text| format!(": {text}"))
            .unwrap_or_default();
        return Err(format!("the token endpoint refused: {error}{description}"));
    }
    let response: TokenResponse = serde_json::from_value(value)
        .map_err(|_| "the token endpoint's answer has no access_token")?;
    if let Some(kind) = &response.token_type {
        if !kind.eq_ignore_ascii_case("bearer") {
            return Err(format!("unsupported token type {kind}"));
        }
    }
    Ok(response)
}

impl McpOAuthTokens {
    /// Takes a refresh answer: a new access token, the refresh token rotated when the
    /// server sent a new one.
    pub fn apply_refresh(&mut self, response: TokenResponse, now: u64) {
        self.access_token = response.access_token;
        if response.refresh_token.is_some() {
            self.refresh_token = response.refresh_token;
        }
        self.expires_at = response.expires_in.map(|seconds| now + seconds);
        if response.scope.is_some() {
            self.scope = response.scope;
        }
    }
}

/// What to do with the stored tokens before a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenAction {
    /// Send this access token.
    Use(String),
    /// The access token expired (or is about to): refresh it first.
    Refresh,
    /// No usable tokens: the user has to sign in (`neuron mcp login`).
    Login,
}

#[must_use]
pub fn token_action(tokens: Option<&McpOAuthTokens>, now: u64) -> TokenAction {
    let Some(tokens) = tokens else {
        return TokenAction::Login;
    };
    let expiring = tokens
        .expires_at
        .is_some_and(|expires_at| expires_at <= now + REFRESH_MARGIN_SECS);
    match (expiring, tokens.refresh_token.is_some()) {
        (false, _) => TokenAction::Use(tokens.access_token.clone()),
        (true, true) => TokenAction::Refresh,
        (true, false) => TokenAction::Login,
    }
}

#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub(crate) fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .unwrap_or_default()
}

/// Uses the refresh token: the new tokens, already stored.
pub async fn refresh_mcp_tokens(
    server_name: &str,
    tokens: &McpOAuthTokens,
) -> Result<McpOAuthTokens, String> {
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| "no refresh token".to_string())?;
    let mut form = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", tokens.client_id.as_str()),
        ("resource", tokens.resource.as_str()),
    ];
    if let Some(secret) = &tokens.client_secret {
        form.push(("client_secret", secret));
    }
    let response = post_form(&tokens.token_endpoint, &form).await?;
    let mut refreshed = tokens.clone();
    refreshed.apply_refresh(response, unix_now());
    save_mcp_tokens(server_name, &refreshed)?;
    Ok(refreshed)
}

async fn post_form(endpoint: &str, form: &[(&str, &str)]) -> Result<TokenResponse, String> {
    require_secure(endpoint)?;
    let response = http_client()
        .post(endpoint)
        .header("Accept", "application/json")
        .form(form)
        .send()
        .await
        .map_err(|error| format!("token request failed: {}", error.without_url()))?;
    let body = response
        .text()
        .await
        .map_err(|error| format!("token request failed: {}", error.without_url()))?;
    parse_token_response(&body)
}

/// The access token to send to `server_name` (at `resource`): the stored one,
/// refreshed when it expired or `force_refresh` (the server answered 401 to it).
/// `Ok(None)`: no tokens are stored for this server.
pub(crate) async fn bearer_token(
    server_name: &str,
    resource: &str,
    force_refresh: bool,
) -> io::Result<Option<String>> {
    let Some(tokens) = load_mcp_tokens(server_name, resource) else {
        return Ok(None);
    };
    let action = if force_refresh {
        if tokens.refresh_token.is_some() {
            TokenAction::Refresh
        } else {
            TokenAction::Login
        }
    } else {
        token_action(Some(&tokens), unix_now())
    };
    match action {
        TokenAction::Use(token) => Ok(Some(token)),
        TokenAction::Refresh => refresh_mcp_tokens(server_name, &tokens)
            .await
            .map(|tokens| Some(tokens.access_token))
            .map_err(|error| auth_required(server_name, Some(&error))),
        TokenAction::Login => Err(auth_required(server_name, None)),
    }
}

pub(crate) fn auth_required(server_name: &str, detail: Option<&str>) -> io::Error {
    let detail = detail.map(|text| format!(" ({text})")).unwrap_or_default();
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("authorization required{detail}; run `neuron mcp login {server_name}`"),
    )
}

/// What `neuron mcp login` needs to know about the server.
#[derive(Debug, Clone, Default)]
pub struct McpLoginOptions {
    pub server_name: String,
    pub server_url: String,
    /// The server's static headers (sent on the discovery probe only).
    pub headers: Vec<(String, String)>,
    pub client_id: Option<String>,
    pub callback_port: Option<u16>,
    pub auth_server_metadata_url: Option<String>,
    pub scope: Option<String>,
}

/// What discovery found: the authorization server's endpoints and the scopes the
/// resource advertises.
#[derive(Debug, Clone)]
pub struct McpAuthDiscovery {
    pub resource: String,
    pub metadata: AuthorizationServerMetadata,
    pub scopes_supported: Option<Vec<String>>,
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Option<T> {
    let response = http_client()
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    response.json::<T>().await.ok()
}

/// Finds the authorization server of the MCP server at `options.server_url`.
pub async fn discover_mcp_authorization(
    options: &McpLoginOptions,
) -> Result<McpAuthDiscovery, String> {
    let resource = canonical_resource_uri(&options.server_url)?;
    if let Some(url) = &options.auth_server_metadata_url {
        let metadata = get_json::<AuthorizationServerMetadata>(url)
            .await
            .ok_or_else(|| format!("could not read authorization server metadata at {url}"))?;
        metadata.check()?;
        return Ok(McpAuthDiscovery {
            resource,
            metadata,
            scopes_supported: None,
        });
    }

    // Probe without a token: a protected server answers 401 naming its metadata.
    let mut probe = http_client()
        .post(&options.server_url)
        .header("Accept", "application/json, text/event-stream")
        .header("Content-Type", "application/json")
        .body(
            serde_json::json!({
                "jsonrpc": "2.0", "id": 0, "method": "initialize",
                "params": crate::mcp_http::initialize_params_json()
            })
            .to_string(),
        );
    for (name, value) in &options.headers {
        probe = probe.header(name, value);
    }
    let mut candidates = Vec::new();
    if let Ok(response) = probe.send().await {
        if let Some(header) = response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok())
        {
            if let Some(url) = parse_www_authenticate(header).resource_metadata() {
                candidates.push(url.to_string());
            }
        }
    }
    candidates.extend(protected_resource_metadata_urls(&options.server_url)?);

    let mut resource_metadata = None;
    for url in &candidates {
        if let Some(metadata) = get_json::<ProtectedResourceMetadata>(url).await {
            if !metadata.authorization_servers.is_empty() {
                resource_metadata = Some(metadata);
                break;
            }
        }
    }

    let server_origin = origin_of(&Url::parse(&options.server_url).map_err(|e| e.to_string())?);
    let (issuer, scopes_supported) = match &resource_metadata {
        Some(metadata) => (
            metadata.authorization_servers[0].clone(),
            metadata.scopes_supported.clone(),
        ),
        None => (server_origin.clone(), None),
    };
    let mut metadata = None;
    for url in authorization_server_metadata_urls(&issuer)? {
        if let Some(found) = get_json::<AuthorizationServerMetadata>(&url).await {
            metadata = Some(found);
            break;
        }
    }
    let metadata = match metadata {
        Some(metadata) => metadata,
        None if resource_metadata.is_none() => {
            AuthorizationServerMetadata::defaults_for(&server_origin)
        }
        None => {
            return Err(format!(
                "could not read the metadata of authorization server {issuer}"
            ))
        }
    };
    metadata.check()?;
    Ok(McpAuthDiscovery {
        resource,
        metadata,
        scopes_supported,
    })
}

#[derive(Deserialize)]
struct RegisteredClient {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

async fn register_client(
    endpoint: &str,
    redirect_uri: &str,
) -> Result<(String, Option<String>), String> {
    require_secure(endpoint)?;
    let response = http_client()
        .post(endpoint)
        .json(&serde_json::json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .map_err(|error| format!("client registration failed: {}", error.without_url()))?;
    if !response.status().is_success() {
        return Err(format!(
            "client registration failed: HTTP {}",
            response.status().as_u16()
        ));
    }
    let client = response
        .json::<RegisteredClient>()
        .await
        .map_err(|_| "client registration answered without a client_id".to_string())?;
    Ok((client.client_id, client.client_secret))
}

/// Signs in to the MCP server: discovery, registration when needed, then the
/// browser (`open_browser` gets the URL) and the loopback redirect. Returns the
/// tokens; the caller stores them ([`save_mcp_tokens`]).
pub async fn login_mcp_server(
    options: &McpLoginOptions,
    open_browser: impl FnOnce(&str),
) -> Result<McpOAuthTokens, String> {
    let discovery = discover_mcp_authorization(options).await?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", options.callback_port.unwrap_or(0)))
        .await
        .map_err(|error| format!("cannot listen for the sign-in redirect: {error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let (client_id, client_secret) = match &options.client_id {
        Some(client_id) => (client_id.clone(), None),
        None => match &discovery.metadata.registration_endpoint {
            Some(endpoint) => register_client(endpoint, &redirect_uri).await?,
            None => {
                return Err(
                    "the authorization server does not support dynamic client registration; \
                     set `oauth.clientId` for this server"
                        .to_string(),
                )
            }
        },
    };
    let pkce = generate_mcp_pkce();
    let state = random_token();
    let scope = options.scope.clone().or_else(|| {
        discovery
            .scopes_supported
            .as_ref()
            .filter(|scopes| !scopes.is_empty())
            .map(|scopes| scopes.join(" "))
    });
    let url = build_authorization_url(
        &discovery.metadata,
        &client_id,
        &redirect_uri,
        &state,
        &pkce,
        &discovery.resource,
        scope.as_deref(),
    );
    open_browser(&url);

    let code = tokio::time::timeout(LOGIN_WAIT, wait_for_callback(&listener, &state))
        .await
        .map_err(|_| "timed out waiting for the browser sign-in".to_string())??;

    let mut form = vec![
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("code_verifier", pkce.verifier.as_str()),
        ("resource", discovery.resource.as_str()),
    ];
    if let Some(secret) = &client_secret {
        form.push(("client_secret", secret));
    }
    let response = post_form(&discovery.metadata.token_endpoint, &form).await?;
    let now = unix_now();
    Ok(McpOAuthTokens {
        access_token: response.access_token,
        refresh_token: response.refresh_token,
        expires_at: response.expires_in.map(|seconds| now + seconds),
        token_endpoint: discovery.metadata.token_endpoint.clone(),
        client_id,
        client_secret,
        resource: discovery.resource,
        scope: response.scope.or(scope),
    })
}

/// Answers requests on the loopback listener until `/callback` arrives with the
/// expected `state`; returns its code.
async fn wait_for_callback(
    listener: &tokio::net::TcpListener,
    state: &str,
) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|error| format!("sign-in redirect failed: {error}"))?;
        let mut buffer = vec![0_u8; 8192];
        let mut read = 0;
        while read < buffer.len() {
            let count = stream.read(&mut buffer[read..]).await.unwrap_or(0);
            if count == 0 {
                break;
            }
            read += count;
            if buffer[..read]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }
        let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
        let target = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or_default()
            .to_string();
        let Ok(params) = parse_oauth_callback_request_target(&target) else {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        };
        let (outcome, page) = if let Some(error) = params.error {
            let detail = params
                .error_description
                .map(|text| format!(": {text}"))
                .unwrap_or_default();
            (
                Err(format!("sign-in failed: {error}{detail}")),
                "Sign-in failed. You can close this tab.",
            )
        } else if params.state.as_deref() != Some(state) {
            (
                Err("sign-in answered with the wrong state".to_string()),
                "Sign-in failed (state mismatch). You can close this tab.",
            )
        } else if let Some(code) = params.code {
            (
                Ok(code),
                "Signed in to the MCP server. You can close this tab.",
            )
        } else {
            (
                Err("sign-in answered without a code".to_string()),
                "Sign-in failed. You can close this tab.",
            )
        };
        let body = format!("<!doctype html><title>Neuron</title><p>{page}</p>");
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(reply.as_bytes()).await;
        let _ = stream.flush().await;
        return outcome;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::io::{Read as _, Write as _};
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};

    use serde_json::json;

    use super::*;
    use crate::mcp_test_http::{FakeResponse, FakeServer};

    /// An in-memory store shared by every test (each test uses its own server names).
    #[derive(Default)]
    pub(crate) struct MemoryStore(Mutex<BTreeMap<String, String>>);

    impl McpTokenStore for MemoryStore {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(key)
                .cloned())
        }
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(key);
            Ok(())
        }
    }

    pub(crate) fn install_memory_store() {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| set_mcp_token_store(Some(Arc::new(MemoryStore::default()))));
    }

    pub(crate) fn sample_tokens(resource: &str, token_endpoint: &str) -> McpOAuthTokens {
        McpOAuthTokens {
            access_token: "access-1".into(),
            refresh_token: Some("refresh-1".into()),
            expires_at: Some(unix_now() + 3600),
            token_endpoint: token_endpoint.into(),
            client_id: "client-1".into(),
            client_secret: None,
            resource: resource.into(),
            scope: None,
        }
    }

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        // RFC 7636 appendix B.
        assert_eq!(
            code_challenge_s256("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let pkce = generate_mcp_pkce();
        assert_eq!(pkce.verifier.len(), 43);
        assert!(pkce
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(pkce.challenge, code_challenge_s256(&pkce.verifier));
        assert_ne!(generate_mcp_pkce().verifier, pkce.verifier);
    }

    #[test]
    fn parses_www_authenticate_challenges() {
        let parsed = parse_www_authenticate(
            r#"Bearer realm="mcp", error="invalid_token", resource_metadata="https://mcp.example.com/.well-known/oauth-protected-resource/mcp""#,
        );
        assert_eq!(parsed.scheme, "Bearer");
        assert_eq!(parsed.params.get("realm").map(String::as_str), Some("mcp"));
        assert_eq!(
            parsed.resource_metadata(),
            Some("https://mcp.example.com/.well-known/oauth-protected-resource/mcp")
        );
        let unquoted = parse_www_authenticate("Bearer error=invalid_token, scope=\"a \\\"b\\\"\"");
        assert_eq!(
            unquoted.params.get("error").map(String::as_str),
            Some("invalid_token")
        );
        assert_eq!(
            unquoted.params.get("scope").map(String::as_str),
            Some("a \"b\"")
        );
        assert_eq!(parse_www_authenticate("Bearer").resource_metadata(), None);
    }

    #[test]
    fn builds_well_known_metadata_urls() {
        assert_eq!(
            protected_resource_metadata_urls("https://mcp.example.com/server/mcp").unwrap(),
            vec![
                "https://mcp.example.com/.well-known/oauth-protected-resource/server/mcp",
                "https://mcp.example.com/.well-known/oauth-protected-resource",
            ]
        );
        assert_eq!(
            protected_resource_metadata_urls("https://mcp.example.com/").unwrap(),
            vec!["https://mcp.example.com/.well-known/oauth-protected-resource"]
        );
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.com/tenant1").unwrap(),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
        assert_eq!(
            authorization_server_metadata_urls("https://auth.example.com").unwrap(),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server",
                "https://auth.example.com/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn canonical_resource_uris_follow_rfc_8707() {
        assert_eq!(
            canonical_resource_uri("HTTPS://MCP.Example.com/").unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com/mcp?x=1#frag").unwrap(),
            "https://mcp.example.com/mcp"
        );
        assert_eq!(
            canonical_resource_uri("https://mcp.example.com:8443").unwrap(),
            "https://mcp.example.com:8443"
        );
        assert!(canonical_resource_uri("mcp.example.com").is_err());
    }

    #[test]
    fn parses_metadata_documents_and_rejects_plain_s256_less_servers() {
        let resource: ProtectedResourceMetadata = serde_json::from_value(json!({
            "resource": "https://mcp.example.com/mcp",
            "authorization_servers": ["https://auth.example.com"],
            "scopes_supported": ["mcp:read"],
            "bearer_methods_supported": ["header"]
        }))
        .unwrap();
        assert_eq!(
            resource.authorization_servers,
            vec!["https://auth.example.com"]
        );
        let server: AuthorizationServerMetadata = serde_json::from_value(json!({
            "issuer": "https://auth.example.com",
            "authorization_endpoint": "https://auth.example.com/authorize",
            "token_endpoint": "https://auth.example.com/token",
            "registration_endpoint": "https://auth.example.com/register",
            "code_challenge_methods_supported": ["S256"]
        }))
        .unwrap();
        assert!(server.check().is_ok());
        let plain = AuthorizationServerMetadata {
            code_challenge_methods_supported: Some(vec!["plain".into()]),
            ..server.clone()
        };
        assert!(plain.check().is_err());
        let insecure = AuthorizationServerMetadata {
            token_endpoint: "http://auth.example.com/token".into(),
            ..server
        };
        assert!(insecure.check().unwrap_err().contains("HTTPS"));
    }

    #[test]
    fn authorization_url_carries_pkce_state_and_resource() {
        let metadata = AuthorizationServerMetadata::defaults_for("https://auth.example.com");
        let pkce = McpPkce {
            verifier: "v".into(),
            challenge: "challenge".into(),
        };
        let url = build_authorization_url(
            &metadata,
            "client 1",
            "http://127.0.0.1:5555/callback",
            "st",
            &pkce,
            "https://mcp.example.com/mcp",
            Some("mcp:read"),
        );
        let parsed = Url::parse(&url).unwrap();
        let query: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(parsed.path(), "/authorize");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "client 1");
        assert_eq!(query["code_challenge"], "challenge");
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["state"], "st");
        assert_eq!(query["resource"], "https://mcp.example.com/mcp");
        assert_eq!(query["scope"], "mcp:read");
    }

    #[test]
    fn token_state_machine_uses_refreshes_or_asks_for_login() {
        let now = 1_000_000;
        let mut tokens = sample_tokens("https://r", "https://a/token");
        tokens.expires_at = Some(now + 3600);
        assert_eq!(
            token_action(Some(&tokens), now),
            TokenAction::Use("access-1".into())
        );
        tokens.expires_at = Some(now + 30);
        assert_eq!(token_action(Some(&tokens), now), TokenAction::Refresh);
        tokens.refresh_token = None;
        assert_eq!(token_action(Some(&tokens), now), TokenAction::Login);
        tokens.expires_at = None;
        assert_eq!(
            token_action(Some(&tokens), now),
            TokenAction::Use("access-1".into())
        );
        assert_eq!(token_action(None, now), TokenAction::Login);

        let mut tokens = sample_tokens("https://r", "https://a/token");
        tokens.apply_refresh(
            parse_token_response(
                r#"{"access_token":"access-2","token_type":"Bearer","expires_in":60}"#,
            )
            .unwrap(),
            now,
        );
        assert_eq!(tokens.access_token, "access-2");
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
        assert_eq!(tokens.expires_at, Some(now + 60));
        tokens.apply_refresh(
            parse_token_response(r#"{"access_token":"access-3","refresh_token":"refresh-2"}"#)
                .unwrap(),
            now,
        );
        assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-2"));
        assert_eq!(tokens.expires_at, None);
        assert!(parse_token_response(r#"{"error":"invalid_grant"}"#)
            .unwrap_err()
            .contains("invalid_grant"));
        assert!(parse_token_response(r#"{"access_token":"x","token_type":"mac"}"#).is_err());
    }

    #[test]
    fn debug_output_never_shows_tokens() {
        let tokens = sample_tokens("https://r", "https://a/token");
        let text = format!("{tokens:?}");
        assert!(!text.contains("access-1"));
        assert!(!text.contains("refresh-1"));
    }

    #[test]
    fn tokens_for_another_resource_are_not_used() {
        install_memory_store();
        let tokens = sample_tokens("https://one.example.com/mcp", "https://a/token");
        save_mcp_tokens("oauth-resource-check", &tokens).unwrap();
        assert!(load_mcp_tokens("oauth-resource-check", "https://one.example.com/mcp").is_some());
        assert!(load_mcp_tokens("oauth-resource-check", "https://two.example.com/mcp").is_none());
        clear_mcp_tokens("oauth-resource-check").unwrap();
        assert!(load_mcp_tokens("oauth-resource-check", "https://one.example.com/mcp").is_none());
    }

    #[test]
    fn expired_tokens_are_refreshed_and_stored() {
        install_memory_store();
        let server = FakeServer::spawn(Arc::new(|request| {
            let form: BTreeMap<_, _> = request.form().into_iter().collect();
            if request.path() == "/token"
                && form.get("grant_type").map(String::as_str) == Some("refresh_token")
                && form.get("refresh_token").map(String::as_str) == Some("refresh-1")
                && form.get("resource").map(String::as_str) == Some("https://mcp.refresh.test")
            {
                FakeResponse::json(
                    200,
                    &json!({"access_token": "tok-new-7f3", "token_type": "Bearer", "expires_in": 3600, "refresh_token": "refresh-2"}),
                )
            } else {
                FakeResponse::json(400, &json!({"error": "invalid_grant"}))
            }
        }));
        let mut tokens =
            sample_tokens("https://mcp.refresh.test", &format!("{}/token", server.url));
        tokens.expires_at = Some(unix_now() - 1);
        save_mcp_tokens("oauth-refresh", &tokens).unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let token = runtime
            .block_on(bearer_token(
                "oauth-refresh",
                "https://mcp.refresh.test",
                false,
            ))
            .unwrap();
        assert_eq!(token.as_deref(), Some("tok-new-7f3"));
        let stored = load_mcp_tokens("oauth-refresh", "https://mcp.refresh.test").unwrap();
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-2"));
        // A refused refresh asks for a login.
        let mut tokens = stored;
        tokens.refresh_token = Some("revoked".into());
        save_mcp_tokens("oauth-refresh", &tokens).unwrap();
        let error = runtime
            .block_on(bearer_token(
                "oauth-refresh",
                "https://mcp.refresh.test",
                true,
            ))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("neuron mcp login oauth-refresh"));
        assert!(!error.to_string().contains("tok-new-7f3"));
    }

    #[test]
    fn login_discovers_registers_and_exchanges_the_code() {
        let registered = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&registered);
        let server = FakeServer::spawn(Arc::new(move |request| {
            let base = format!("http://{}", request.header("host").unwrap_or_default());
            match (request.method.as_str(), request.path()) {
                ("POST", "/mcp") => FakeResponse::status(401).with_header(
                    "WWW-Authenticate",
                    &format!(r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#),
                ),
                ("GET", "/.well-known/oauth-protected-resource/mcp") => FakeResponse::json(
                    200,
                    &json!({"resource": format!("{base}/mcp"), "authorization_servers": [format!("{base}/auth")]}),
                ),
                ("GET", "/.well-known/oauth-authorization-server/auth") => FakeResponse::json(
                    200,
                    &json!({
                        "issuer": format!("{base}/auth"),
                        "authorization_endpoint": format!("{base}/auth/authorize"),
                        "token_endpoint": format!("{base}/auth/token"),
                        "registration_endpoint": format!("{base}/auth/register"),
                        "code_challenge_methods_supported": ["S256"]
                    }),
                ),
                ("POST", "/auth/register") => {
                    seen.lock().unwrap().push(request.body.clone());
                    FakeResponse::json(201, &json!({"client_id": "dyn-client"}))
                }
                ("POST", "/auth/token") => {
                    let form: BTreeMap<_, _> = request.form().into_iter().collect();
                    let verifier_ok = form
                        .get("code_verifier")
                        .is_some_and(|verifier| verifier.len() == 43);
                    if form.get("code").map(String::as_str) == Some("the-code")
                        && form.get("client_id").map(String::as_str) == Some("dyn-client")
                        && form.get("resource") == Some(&format!("{base}/mcp"))
                        && verifier_ok
                    {
                        FakeResponse::json(
                            200,
                            &json!({"access_token": "granted", "token_type": "Bearer", "expires_in": 600, "refresh_token": "r"}),
                        )
                    } else {
                        FakeResponse::json(400, &json!({"error": "invalid_grant"}))
                    }
                }
                _ => FakeResponse::status(404),
            }
        }));
        let options = McpLoginOptions {
            server_name: "oauth-login".into(),
            server_url: format!("{}/mcp", server.url),
            ..McpLoginOptions::default()
        };
        // The "browser": follows the authorization URL straight to the redirect.
        let browser = |url: &str| {
            let parsed = Url::parse(url).unwrap();
            let query: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
            assert_eq!(query["code_challenge_method"], "S256");
            let redirect = Url::parse(&query["redirect_uri"]).unwrap();
            assert_eq!(redirect.host_str(), Some("127.0.0.1"));
            let state = query["state"].clone();
            let port = redirect.port().unwrap();
            std::thread::spawn(move || {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(
                    stream,
                    "GET /callback?code=the-code&state={state} HTTP/1.1\r\nHost: x\r\n\r\n"
                )
                .unwrap();
                let mut reply = String::new();
                let _ = stream.read_to_string(&mut reply);
                assert!(reply.contains("Signed in"));
            });
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let tokens = runtime
            .block_on(login_mcp_server(&options, browser))
            .unwrap();
        assert_eq!(tokens.access_token, "granted");
        assert_eq!(tokens.client_id, "dyn-client");
        assert_eq!(tokens.resource, format!("{}/mcp", server.url));
        assert!(tokens.token_endpoint.ends_with("/auth/token"));
        let registration: Value = serde_json::from_str(&registered.lock().unwrap()[0]).unwrap();
        assert_eq!(registration["token_endpoint_auth_method"], "none");
        assert!(registration["redirect_uris"][0]
            .as_str()
            .unwrap()
            .starts_with("http://127.0.0.1:"));
    }

    #[test]
    fn login_rejects_a_redirect_with_the_wrong_state() {
        let server = FakeServer::spawn(Arc::new(|request| {
            let base = format!("http://{}", request.header("host").unwrap_or_default());
            match request.path() {
                "/.well-known/oauth-authorization-server" => FakeResponse::json(
                    200,
                    &json!({
                        "authorization_endpoint": format!("{base}/authorize"),
                        "token_endpoint": format!("{base}/token")
                    }),
                ),
                _ => FakeResponse::status(404),
            }
        }));
        let options = McpLoginOptions {
            server_name: "oauth-state".into(),
            server_url: format!("{}/mcp", server.url),
            client_id: Some("static".into()),
            ..McpLoginOptions::default()
        };
        let browser = |url: &str| {
            let parsed = Url::parse(url).unwrap();
            let query: BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
            let port = Url::parse(&query["redirect_uri"]).unwrap().port().unwrap();
            std::thread::spawn(move || {
                let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
                write!(
                    stream,
                    "GET /callback?code=c&state=forged HTTP/1.1\r\nHost: x\r\n\r\n"
                )
                .unwrap();
                let mut reply = String::new();
                let _ = stream.read_to_string(&mut reply);
            });
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(login_mcp_server(&options, browser))
            .unwrap_err();
        assert!(error.contains("wrong state"), "{error}");
    }
}
