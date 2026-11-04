//! Sign-in for remote MCP servers: `neuron mcp login <server>` and
//! `neuron mcp logout <server>`, and the token store the runtime reads remote
//! servers' tokens from (the OS keychain, entry `mcp-oauth:<server>` of the
//! `neuron-cli` service, or an owner-only file under `~/.neuroncli/mcp-oauth`).

use std::env;
use std::path::PathBuf;
use std::sync::Arc;

use runtime::{
    ConfigLoader, McpLoginOptions, McpServerConfig, McpTokenStore, ScopedMcpServerConfig,
};

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
    FileBackend::new(neuron_home().join("mcp-oauth").join(format!("{safe}.json")))
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

/// Lets remote MCP servers find their tokens.
pub(crate) fn install_token_store() {
    runtime::set_mcp_token_store(Some(Arc::new(KeychainTokenStore)));
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

fn find_server(name: &str) -> Result<ScopedMcpServerConfig, String> {
    let cwd: PathBuf = env::current_dir().map_err(|error| error.to_string())?;
    let config = ConfigLoader::default_for(&cwd)
        .load()
        .map_err(|error| error.to_string())?;
    config
        .mcp()
        .get(name)
        .or_else(|| config.mcp().disabled().get(name))
        .cloned()
        .ok_or_else(|| format!("no MCP server named `{name}` is configured"))
}

fn login(name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let server = find_server(name)?;
    let remote = match &server.config {
        McpServerConfig::Http(remote) | McpServerConfig::Sse(remote) => remote,
        _ => {
            return Err(format!(
                "`{name}` is not a remote (http or sse) MCP server; only those sign in"
            )
            .into())
        }
    };
    let options = McpLoginOptions {
        server_name: name.to_string(),
        server_url: remote.url.clone(),
        headers: remote
            .headers
            .iter()
            .map(|(key, value)| (key.clone(), runtime::expand_mcp_header_value(value)))
            .collect(),
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
        .build()?;
    let tokens = tokio.block_on(runtime::login_mcp_server(&options, |url| {
        eprintln!("Opening your browser to sign in to the MCP server `{name}`.");
        eprintln!("If it does not open, visit:\n  {url}");
        let _ = webbrowser::open(url);
    }))?;
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
    use super::{file_for, handle_mcp_auth_command};

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
    }
}
