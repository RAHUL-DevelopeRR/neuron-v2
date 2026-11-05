//! Sign-in for remote MCP servers: `neuron mcp login <server>` and
//! `neuron mcp logout <server>`, and the store the runtime reads remote servers'
//! tokens (entry `mcp-oauth:<server>`) and connector secrets (entry
//! `mcp-secret:<server>:<NAME>`) from: the OS keychain (`neuron-cli` service), or an
//! owner-only file under `~/.neuroncli/mcp-oauth` or `~/.neuroncli/mcp-secrets`.
//! Hosted connectors (`zerox` servers) use the Zero-X sign-in instead.

use std::env;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use runtime::{
    ConfigLoader, McpLoginOptions, McpServerConfig, McpTokenStore, ScopedMcpServerConfig,
};

use crate::auth::GatewayCredential;
use crate::credentials::{
    keychain_entry_for, neuron_home, BackendError, FileBackend, SecretBackend,
};

/// The keychain, falling back to a file per server.
struct KeychainTokenStore;

fn file_for(key: &str) -> FileBackend {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let folder = if key.starts_with("mcp-secret:") {
        "mcp-secrets"
    } else {
        "mcp-oauth"
    };
    FileBackend::new(neuron_home().join(folder).join(format!("{safe}.json")))
}

impl McpTokenStore for KeychainTokenStore {
    fn load(&self, key: &str) -> Result<Option<String>, String> {
        if let Some(keychain) = keychain_entry_for(key) {
            match keychain.read() {
                Ok(Some(secret)) => return Ok(Some(secret.to_string())),
                Ok(None) | Err(BackendError::Unavailable(_)) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        file_for(key)
            .read()
            .map(|secret| secret.map(|secret| secret.to_string()))
            .map_err(|error| error.to_string())
    }

    fn save(&self, key: &str, value: &str) -> Result<(), String> {
        if let Some(keychain) = keychain_entry_for(key) {
            if keychain.write(value).is_ok() {
                let _ = file_for(key).delete();
                return Ok(());
            }
        }
        file_for(key)
            .write(value)
            .map_err(|error| error.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        if let Some(keychain) = keychain_entry_for(key) {
            if let Err(BackendError::Failed(error)) = keychain.delete() {
                return Err(error);
            }
        }
        file_for(key).delete().map_err(|error| error.to_string())
    }
}

/// Lets remote MCP servers find their tokens, and hosted connectors the Zero-X
/// sign-in (`NEURON_TOKEN`, else the stored login; read on each new connection).
pub(crate) fn install_token_store() {
    runtime::set_mcp_token_store(Some(Arc::new(KeychainTokenStore)));
    runtime::set_zerox_token_provider(Some(Arc::new(|| {
        zerox_bearer(crate::auth::current_credential(), crate::clock::now_unix())
    })));
}

/// The bearer a hosted connector sends, or why there is none (never the token).
fn zerox_bearer(
    credential: io::Result<Option<GatewayCredential>>,
    now: u64,
) -> Result<String, String> {
    match credential {
        Ok(Some(credential)) if credential.is_expired(now) => Err(
            "your Zero-X sign-in has expired; run `neuron login` to sign in again".to_string(),
        ),
        Ok(Some(credential)) => Ok(credential.token.expose().to_string()),
        Ok(None) => Err(
            "not signed in to Zero-X; run `neuron login` (or set NEURON_TOKEN) to use hosted connectors"
                .to_string(),
        ),
        Err(error) => Err(error.to_string()),
    }
}

/// `login <server>` / `logout <server>`: what it did, or `None` for other `mcp`
/// arguments.
pub(crate) fn handle_mcp_auth_command(
    args: Option<&str>,
) -> Option<Result<String, Box<dyn std::error::Error>>> {
    let words = args?.split_whitespace().collect::<Vec<_>>();
    match words.as_slice() {
        ["login", name] => Some(login(name)),
        ["logout", name] => Some(logout(name)),
        ["login" | "logout"] => Some(Err(
            "name the MCP server: `neuron mcp login <server>`".into()
        )),
        _ => None,
    }
}

fn find_server(loader: &ConfigLoader, name: &str) -> Result<ScopedMcpServerConfig, String> {
    let config = loader.load().map_err(|error| error.to_string())?;
    config
        .mcp()
        .get(name)
        .or_else(|| config.mcp().disabled().get(name))
        .cloned()
        .ok_or_else(|| format!("no MCP server named `{name}` is configured"))
}

pub(crate) fn login(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let cwd: PathBuf = env::current_dir()?;
    Ok(login_with(&ConfigLoader::default_for(&cwd), name, |url| {
        eprintln!("Opening your browser to sign in to the MCP server `{name}`.");
        eprintln!("If it does not open, visit:\n  {url}");
        let _ = webbrowser::open(url);
    })?)
}

/// Signs in to the remote MCP server `name` that `loader` configures and stores its
/// tokens; `open_browser` gets the sign-in URL.
pub(crate) fn login_with(
    loader: &ConfigLoader,
    name: &str,
    open_browser: impl FnOnce(&str),
) -> Result<String, String> {
    let server = find_server(loader, name)?;
    let remote = match &server.config {
        McpServerConfig::Http(remote) | McpServerConfig::Sse(remote) => remote,
        _ => {
            return Err(format!(
                "`{name}` is not a remote (http or sse) MCP server; only those sign in"
            ))
        }
    };
    let options = McpLoginOptions {
        server_name: name.to_string(),
        server_url: remote.url.clone(),
        headers: remote
            .headers
            .iter()
            .map(|(key, value)| {
                runtime::expand_mcp_header(name, &remote.url, value)
                    .map(|value| (key.clone(), value))
            })
            .collect::<Result<_, _>>()?,
        client_id: remote
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.client_id.clone()),
        callback_port: remote.oauth.as_ref().and_then(|oauth| oauth.callback_port),
        auth_server_metadata_url: remote
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.auth_server_metadata_url.clone()),
        scope: None,
    };
    let tokio = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let tokens = tokio.block_on(runtime::login_mcp_server(&options, open_browser))?;
    runtime::save_mcp_tokens(name, &tokens)?;
    Ok(format!(
        "Signed in to the MCP server `{name}`. Its tokens are stored in your OS keychain (or ~/.neuroncli/mcp-oauth)."
    ))
}

fn logout(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    runtime::clear_mcp_tokens(name)?;
    Ok(format!("Signed out of the MCP server `{name}`."))
}

#[cfg(test)]
mod tests {
    use super::{file_for, handle_mcp_auth_command, zerox_bearer};
    use crate::auth::{CredentialSource, GatewayCredential};
    use crate::credentials::TokenKind;
    use crate::vault::SecureString;

    #[test]
    fn hosted_connectors_get_the_zerox_sign_in_while_it_is_good() {
        let credential = |expires_at| GatewayCredential {
            token: SecureString::new("ses_hosted_connector".to_string()),
            kind: TokenKind::Session,
            source: CredentialSource::EnvToken,
            expires_at,
            plan: None,
            email: None,
            name: None,
        };
        assert_eq!(
            zerox_bearer(Ok(Some(credential(None))), 100).as_deref(),
            Ok("ses_hosted_connector")
        );
        let expired = zerox_bearer(Ok(Some(credential(Some(50)))), 100).unwrap_err();
        assert!(expired.contains("neuron login"), "{expired}");
        assert!(!expired.contains("ses_hosted_connector"));
        let signed_out = zerox_bearer(Ok(None), 100).unwrap_err();
        assert!(
            signed_out.contains("not signed in to Zero-X"),
            "{signed_out}"
        );
        let invalid = zerox_bearer(
            Err(std::io::Error::other("NEURON_TOKEN: unknown token kind")),
            100,
        )
        .unwrap_err();
        assert!(invalid.starts_with("NEURON_TOKEN"), "{invalid}");
    }

    #[test]
    fn only_login_and_logout_are_handled_here() {
        assert!(handle_mcp_auth_command(Some("list")).is_none());
        assert!(handle_mcp_auth_command(None).is_none());
        assert!(handle_mcp_auth_command(Some("login")).unwrap().is_err());
    }

    #[test]
    fn token_files_have_safe_names() {
        let file = file_for("mcp-oauth:../../evil");
        let name = file
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(name, "mcp-oauth_______evil.json");
        let secret = file_for("mcp-secret:github:GITHUB_TOKEN");
        assert_eq!(
            secret.path().parent().unwrap().file_name().unwrap(),
            "mcp-secrets"
        );
        assert_eq!(
            secret.path().file_name().unwrap(),
            "mcp-secret_github_GITHUB_TOKEN.json"
        );
    }
}
