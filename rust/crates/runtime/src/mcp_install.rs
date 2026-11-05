//! Adding a catalog connector to a settings file and removing a server again, and the
//! secrets they need. A secret goes to the store the host installed for MCP tokens (the
//! OS keychain in the CLI) as `mcp-secret:<server>:<NAME>`; the settings file holds
//! only the reference `${secret:NAME}`, resolved when the server starts
//! ([`resolve_mcp_secret_references`]). Settings are written with the same atomic
//! read-modify-write as the on/off switches (`mcp_toggle`).
//!
//! A secret is bound to what it was added for: a remote server's URL, or a stdio
//! server's command, arguments and environment ([`mcp_secret_target`]). A settings file
//! that later points a server of the same name somewhere else, or starts it with other
//! environment such as `NODE_OPTIONS` (a pulled project file, say), does not get the
//! secret.
//!
//! A catalog connector's stdio server starts from a verified install
//! ([`prepare_stdio_transport`], see `verified`), never through `npx` or `uvx`; so does
//! a server pinned with `/mcp pin` (`mcp_pins`).

mod archive;
mod verified;

#[cfg(test)]
pub(crate) use archive::fixtures as archive_fixtures;

pub use verified::{
    mcp_requirements_text, McpFetch, McpHttpFetch, McpInstallState, McpInstallStore,
    McpVerifiedLaunch, McpVerifiedSource, McpVerifiedSpec,
};

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::path::PathBuf;

use serde_json::{json, Map, Value};

use crate::config::{parse_mcp_server_json, ConfigLoader, ConfigSource, McpServerConfig};
use crate::mcp_approvals::{mcp_approval_workspace, McpApprovals};
use crate::mcp_catalog::{mcp_secret_references, McpCatalog, McpCatalogEntry};
use crate::mcp_oauth::token_store;
use crate::mcp_toggle::{read_object, write_object};

/// The keychain key of the secret `name` of the MCP server `server`.
#[must_use]
pub fn mcp_secret_key(server: &str, name: &str) -> String {
    format!("mcp-secret:{server}:{name}")
}

/// What a secret is bound to: `url:<url>` for a remote server, `cmd:<command, arguments
/// and environment as written, as JSON>` for a stdio one, since another environment
/// (`NODE_OPTIONS`, a package registry) can run another program. `None` for other
/// transports.
#[must_use]
pub fn mcp_secret_target(config: &McpServerConfig) -> Option<String> {
    match config {
        McpServerConfig::Stdio(stdio) => {
            Some(stdio_target(&stdio.command, &stdio.args, &stdio.env))
        }
        McpServerConfig::Http(remote) | McpServerConfig::Sse(remote) => {
            Some(remote_target(&remote.url))
        }
        _ => None,
    }
}

fn stdio_target(command: &str, args: &[String], env: &BTreeMap<String, String>) -> String {
    format!("cmd:{}", json!([command, args, env]))
}

fn remote_target(url: &str) -> String {
    format!("url:{url}")
}

/// Stores the secret `name` of `server`, bound to `target` ([`mcp_secret_target`]).
pub fn save_mcp_secret(server: &str, name: &str, value: &str, target: &str) -> Result<(), String> {
    let store = token_store().ok_or_else(|| "no secret store is available".to_string())?;
    let record = json!({"value": value, "target": target}).to_string();
    store.save(&mcp_secret_key(server, name), &record)
}

/// The stored secret `name` of `server` and what it is bound to.
fn load_bound_secret(server: &str, name: &str) -> Result<Option<(String, String)>, String> {
    let Some(store) = token_store() else {
        return Ok(None);
    };
    let Some(record) = store.load(&mcp_secret_key(server, name))? else {
        return Ok(None);
    };
    let record = serde_json::from_str::<Value>(&record).unwrap_or(Value::Null);
    match (
        record.get("value").and_then(Value::as_str),
        record.get("target").and_then(Value::as_str),
    ) {
        (Some(value), Some(target)) => Ok(Some((value.to_string(), target.to_string()))),
        _ => Err("the stored entry is not a Neuron secret record".to_string()),
    }
}

/// The stored secret `name` of `server`, whatever it is bound to.
pub fn load_mcp_secret(server: &str, name: &str) -> Result<Option<String>, String> {
    Ok(load_bound_secret(server, name)?.map(|(value, _)| value))
}

/// Forgets the secret `name` of `server`.
pub fn delete_mcp_secret(server: &str, name: &str) -> Result<(), String> {
    match token_store() {
        Some(store) => store.delete(&mcp_secret_key(server, name)),
        None => Ok(()),
    }
}

/// `value` with each `${secret:NAME}` replaced by the stored secret of `server`, when
/// it was stored for `target`. A missing secret, or one stored for another target, is
/// an error that names it, never a value.
pub fn resolve_mcp_secret_references(
    server: &str,
    target: &str,
    value: &str,
) -> Result<String, String> {
    if !value.contains("${secret:") {
        return Ok(value.to_string());
    }
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${secret:") {
        output.push_str(&rest[..start]);
        let after = &rest[start + "${secret:".len()..];
        let Some(end) = after.find('}') else {
            output.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let name = &after[..end];
        match load_bound_secret(server, name) {
            Ok(Some((secret, bound))) if bound == target => output.push_str(&secret),
            Ok(Some(_)) => {
                return Err(format!(
                    "the secret {name} of MCP server `{server}` was added for another command, environment or URL, so it is not sent here; add the server again with `neuron mcp add`"
                ))
            }
            Ok(None) => {
                return Err(format!(
                    "the secret {name} of MCP server `{server}` is not stored; add it again with `neuron mcp add`"
                ))
            }
            Err(error) => {
                return Err(format!(
                    "cannot read the secret {name} of MCP server `{server}`: {error}"
                ))
            }
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// `transport` with the `${secret:NAME}` references in its environment resolved for
/// `server`, to start it with.
pub(crate) fn stdio_transport_with_secrets(
    server: &str,
    transport: &crate::mcp_client::McpStdioTransport,
) -> std::io::Result<crate::mcp_client::McpStdioTransport> {
    let target = stdio_target(&transport.command, &transport.args, &transport.env);
    let mut resolved = transport.clone();
    for value in resolved.env.values_mut() {
        *value = resolve_mcp_secret_references(server, &target, value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::NotFound, error))?;
    }
    Ok(resolved)
}

/// The verified install `transport` starts from: a catalog connector's, when its launch
/// spec is exactly that entry's. `None` for any other server.
pub fn mcp_verified_spec_for(
    transport: &crate::mcp_client::McpStdioTransport,
) -> Result<Option<McpVerifiedSpec>, String> {
    let config = McpServerConfig::Stdio(crate::config::McpStdioServerConfig {
        command: transport.command.clone(),
        args: transport.args.clone(),
        env: transport.env.clone(),
        tool_call_timeout_ms: None,
        sandbox: None,
    });
    match McpCatalog::builtin().entry_for_config(&config) {
        Some(entry) => McpVerifiedSpec::for_entry(entry),
        None => Ok(None),
    }
}

/// `transport` ready to start: its secrets resolved, and a catalog connector switched
/// from the `npx`/`uvx`/program it names in settings to its verified install (installed
/// on first use, checked on every start). The install folder becomes readable in its
/// sandbox, as its program.
pub(crate) fn prepare_stdio_transport(
    server: &str,
    transport: &crate::mcp_client::McpStdioTransport,
) -> std::io::Result<crate::mcp_client::McpStdioTransport> {
    prepare_stdio_transport_in(
        server,
        transport,
        &McpInstallStore::in_config_home(&crate::config::default_config_home()),
        &McpHttpFetch,
    )
}

fn prepare_stdio_transport_in(
    server: &str,
    transport: &crate::mcp_client::McpStdioTransport,
    store: &McpInstallStore,
    fetch: &dyn McpFetch,
) -> std::io::Result<crate::mcp_client::McpStdioTransport> {
    let mut resolved = stdio_transport_with_secrets(server, transport)?;
    let refused = |error: String| {
        std::io::Error::other(format!("MCP server `{server}` was not started: {error}"))
    };
    let spec = match mcp_verified_spec_for(transport).map_err(refused)? {
        Some(spec) => spec,
        // Any other server: a pinned npm or Python package starts from its install, a
        // pinned program only when its files still match.
        None => match crate::mcp_pins::mcp_pinned_launch(
            store,
            &transport.command,
            &transport.args,
            &transport.env,
            &std::env::current_dir().unwrap_or_default(),
        )
        .map_err(refused)?
        {
            crate::mcp_pins::McpPinnedLaunch::Verified(spec) => spec,
            _ => return Ok(resolved),
        },
    };
    let launch = store.ensure(&spec, fetch).map_err(refused)?;
    resolved.command = launch.command;
    resolved.args = launch.args;
    if let Some(dir) = launch.dir {
        resolved
            .sandbox
            .fs_read
            .push(dir.to_string_lossy().into_owned());
    }
    Ok(resolved)
}

/// A header value of the remote server `server` at `url`: `${secret:NAME}` from the
/// keychain (when stored for that URL), `${NAME}` from the environment (empty when
/// unset). A secret's own text is not expanded again.
pub fn expand_mcp_header(server: &str, url: &str, value: &str) -> Result<String, String> {
    let target = remote_target(url);
    let mut output = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            output.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let name = &after[..end];
        match name.strip_prefix("secret:") {
            Some(secret) => output.push_str(&resolve_mcp_secret_references(
                server,
                &target,
                &crate::mcp_catalog::mcp_secret_reference(secret),
            )?),
            None => output.push_str(&std::env::var(name).unwrap_or_default()),
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

/// Which settings file a server is added to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpInstallScope {
    /// `<config home>/settings.json`: every workspace.
    User,
    /// `<workspace>/.neuron/settings.json`: shared with the repository.
    Project,
    /// `<workspace>/.neuron/settings.local.json`: this checkout only.
    Local,
}

impl McpInstallScope {
    /// `user`, `project` or `local`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "user" => Ok(Self::User),
            "project" => Ok(Self::Project),
            "local" => Ok(Self::Local),
            other => Err(format!(
                "unknown scope `{other}`: use user, project or local"
            )),
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
        }
    }

    /// The settings file of this scope.
    #[must_use]
    pub fn settings_path(self, loader: &ConfigLoader) -> PathBuf {
        match self {
            Self::User => loader.config_home().join("settings.json"),
            Self::Project => loader.cwd().join(".neuron").join("settings.json"),
            Self::Local => loader.cwd().join(".neuron").join("settings.local.json"),
        }
    }

    /// The scopes Neuron writes, in the order `remove` looks for a server.
    const ALL: [Self; 3] = [Self::Local, Self::Project, Self::User];

    fn of(source: ConfigSource) -> Option<Self> {
        match source {
            ConfigSource::User => Some(Self::User),
            ConfigSource::Project => Some(Self::Project),
            ConfigSource::Local => Some(Self::Local),
            ConfigSource::Bundled | ConfigSource::Managed => None,
        }
    }
}

/// What to add: a catalog id, the server's name (the id when `None`), the scope (the
/// entry's default when `None`) and the variables the user supplied, secrets included.
#[derive(Clone, PartialEq, Eq)]
pub struct McpInstallRequest {
    pub id: String,
    pub name: Option<String>,
    pub scope: Option<McpInstallScope>,
    pub values: BTreeMap<String, String>,
}

impl fmt::Debug for McpInstallRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpInstallRequest")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("scope", &self.scope)
            .field("values", &self.values.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// What [`install_mcp_server`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInstallOutcome {
    pub name: String,
    pub id: String,
    pub scope: McpInstallScope,
    pub settings_path: PathBuf,
    /// The server was configured this way already (its secrets were stored again).
    pub already: bool,
    /// The names of the secrets stored in the keychain.
    pub secrets_stored: Vec<String>,
    /// A workspace server, approved for this workspace as written.
    pub approved: bool,
    /// The server signs in with OAuth: `neuron mcp login <name>`.
    pub needs_login: bool,
    /// `<package>@<version>` or the URL.
    pub pinned: String,
}

/// The variables `entry` needs that `values` does not supply and has no default for.
#[must_use]
pub fn missing_mcp_variables<'e>(
    entry: &'e McpCatalogEntry,
    values: &BTreeMap<String, String>,
) -> Vec<&'e crate::mcp_catalog::McpCatalogVariable> {
    entry
        .env
        .iter()
        .filter(|variable| variable.required && variable.default.is_none())
        .filter(|variable| values.get(&variable.name).is_none_or(String::is_empty))
        .collect()
}

fn valid_server_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Adds the catalog entry `request.id` to the settings file of its scope. Refused for an
/// unknown id, a missing required variable, a project or local scope in a folder that is
/// not trusted (its settings would not load), and a name already configured differently
/// in that file. Secrets go to the keychain; the file gets `${secret:NAME}`.
pub fn install_mcp_server(
    loader: &ConfigLoader,
    request: &McpInstallRequest,
) -> Result<McpInstallOutcome, String> {
    install_from_catalog(loader, McpCatalog::builtin(), request)
}

#[allow(clippy::too_many_lines)]
fn install_from_catalog(
    loader: &ConfigLoader,
    catalog: &McpCatalog,
    request: &McpInstallRequest,
) -> Result<McpInstallOutcome, String> {
    let id = request.id.trim();
    let entry = catalog.get(id).ok_or_else(|| unknown_id(catalog, id))?;
    let scope = match request.scope {
        Some(scope) => scope,
        None => McpInstallScope::parse(&entry.default_scope)?,
    };
    if scope != McpInstallScope::User && !loader.workspace_is_trusted() {
        return Err(format!(
            "this folder is not trusted, so Neuron does not load its {} settings and `{id}` would never start there. Trust the folder first, or add it for yourself with --scope user",
            scope.as_str()
        ));
    }
    let name = request.name.as_deref().unwrap_or(id).trim().to_string();
    if !valid_server_name(&name) {
        return Err(format!(
            "`{name}` cannot name an MCP server: use letters, digits, - and _"
        ));
    }
    let missing = missing_mcp_variables(entry, &request.values);
    if !missing.is_empty() {
        return Err(format!(
            "`{id}` needs {}",
            missing
                .iter()
                .map(|variable| format!("{} ({})", variable.label, variable.name))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let values = entry
        .env
        .iter()
        .filter_map(|variable| {
            request
                .values
                .get(&variable.name)
                .filter(|value| !value.is_empty())
                .map(|value| (variable.name.clone(), value.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let written = entry.settings_entry(&values);
    let config =
        parse_mcp_server_json(&name, &written.to_string()).map_err(|error| error.to_string())?;
    // A server the connector policy refuses would never start: say so instead of
    // writing it.
    if let Ok(loaded) = loader.load() {
        let candidate = crate::config::ScopedMcpServerConfig {
            scope: ConfigSource::User,
            config: config.clone(),
        };
        if let Some(reason) = loaded.mcp().policy().refusal(&name, &candidate) {
            return Err(format!("`{id}` cannot be added: {reason}"));
        }
    }

    let settings_path = scope.settings_path(loader);
    let mut object = read_object(&settings_path)?;
    let mut servers = match object.get("mcpServers") {
        None => Map::new(),
        Some(Value::Object(servers)) => servers.clone(),
        Some(_) => {
            return Err(format!(
                "{}: mcpServers is not an object",
                settings_path.display()
            ))
        }
    };
    // Secrets and sign-ins are kept per server name, not per scope: a second server of
    // the same name in another scope would replace this one's, and removing either
    // would sign the other out.
    if let Some((other, _)) = entries_in_other_scopes(loader, &name, scope).first() {
        return Err(format!(
            "an MCP server named `{name}` is already in the {} settings ({}); remove it there first (/mcp remove {name} --scope {}) or choose another name with --name",
            other.as_str(),
            other.settings_path(loader).display(),
            other.as_str()
        ));
    }
    let already = match servers.get(&name) {
        Some(existing) if *existing == written => true,
        Some(_) => {
            return Err(format!(
                "an MCP server named `{name}` is already configured in {}; remove it first (/mcp remove {name}) or choose another name with --name",
                settings_path.display()
            ))
        }
        None => false,
    };

    let target = mcp_secret_target(&config).unwrap_or_default();
    let mut secrets_stored: Vec<String> = Vec::new();
    for variable in entry.env.iter().filter(|variable| variable.secret) {
        if let Some(value) = values.get(&variable.name) {
            if let Err(error) = save_mcp_secret(&name, &variable.name, value, &target) {
                for stored in &secrets_stored {
                    let _ = delete_mcp_secret(&name, stored);
                }
                return Err(format!("cannot store {}: {error}", variable.name));
            }
            secrets_stored.push(variable.name.clone());
        }
    }
    if !already {
        servers.insert(name.clone(), written);
        object.insert("mcpServers".to_string(), Value::Object(servers));
        if let Err(error) = write_object(&settings_path, &object) {
            for stored in &secrets_stored {
                let _ = delete_mcp_secret(&name, stored);
            }
            return Err(error);
        }
    }
    // The user added it here themselves: it starts without asking again, until its
    // launch spec changes.
    let approved = scope != McpInstallScope::User;
    if approved {
        McpApprovals::in_config_home(loader.config_home()).approve(
            &mcp_approval_workspace(loader.cwd()),
            &name,
            &config,
        )?;
    }
    Ok(McpInstallOutcome {
        name,
        id: id.to_string(),
        scope,
        settings_path,
        already,
        secrets_stored,
        approved,
        needs_login: entry.install.oauth,
        pinned: entry.pinned_label(),
    })
}

fn unknown_id(catalog: &McpCatalog, id: &str) -> String {
    let close = catalog
        .connectors
        .iter()
        .filter(|entry| {
            entry.id.contains(id) || id.contains(entry.id.as_str()) || {
                let first = |text: &str| text.chars().next();
                first(&entry.id) == first(id) && !id.is_empty()
            }
        })
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    let mut message = format!("`{id}` is not in the connector catalog");
    if !close.is_empty() {
        let _ = write!(message, "; did you mean {}?", close.join(", "));
    }
    message.push_str(" `/mcp catalog` lists it");
    message
}

/// What [`remove_mcp_server`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRemoveOutcome {
    pub name: String,
    pub settings_path: PathBuf,
    /// The secrets deleted from the keychain.
    pub secrets_deleted: Vec<String>,
    /// A remote server: its OAuth tokens were forgotten too.
    pub signed_out: bool,
    /// Another scope still configures a server of this name, so the secrets and the
    /// sign-in it uses were kept.
    pub still_used_by: Option<McpInstallScope>,
}

/// Removes the server `name` from a settings file Neuron writes (the one for `scope`, or
/// the first of local, project and user that has it), and deletes the secrets its entry
/// refers to and, for a remote server, its sign-in. Refused for a bundled server (turn it
/// off instead) and for one configured only in a file Neuron does not edit.
pub fn remove_mcp_server(
    loader: &ConfigLoader,
    name: &str,
    scope: Option<McpInstallScope>,
) -> Result<McpRemoveOutcome, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name an MCP server".to_string());
    }
    let required = loader.load().ok().and_then(|config| {
        config
            .mcp()
            .required()
            .get(name)
            .map(|required| required.by)
    });
    if let Some(by) = required {
        return Err(format!(
            "`{name}` is required by {} and cannot be removed",
            by.describe()
        ));
    }
    let scopes = match scope {
        Some(scope) => vec![scope],
        None => McpInstallScope::ALL.to_vec(),
    };
    for scope in scopes.iter().copied() {
        let path = scope.settings_path(loader);
        let mut object = read_object(&path)?;
        let Some(Value::Object(mut servers)) = object.get("mcpServers").cloned() else {
            continue;
        };
        let Some(removed) = servers.remove(name) else {
            continue;
        };
        if servers.is_empty() {
            object.remove("mcpServers");
        } else {
            object.insert("mcpServers".to_string(), Value::Object(servers));
        }
        write_object(&path, &object)?;
        let remaining = entries_in_other_scopes(loader, name, scope);
        return Ok(forget_credentials(name, &removed, &remaining, path));
    }
    Err(remove_refusal(loader, name, scope))
}

/// The servers named `name` in the settings of the scopes other than `except`, which
/// share its keychain entries. An unreadable file counts as not configuring it.
fn entries_in_other_scopes(
    loader: &ConfigLoader,
    name: &str,
    except: McpInstallScope,
) -> Vec<(McpInstallScope, Value)> {
    McpInstallScope::ALL
        .into_iter()
        .filter(|scope| *scope != except)
        .filter_map(|scope| {
            let object = read_object(&scope.settings_path(loader)).ok()?;
            let entry = object.get("mcpServers")?.get(name)?.clone();
            Some((scope, entry))
        })
        .collect()
}

/// The `${secret:NAME}` references in a settings entry's environment and headers.
fn entry_secret_references(entry: &Value) -> Vec<String> {
    let mut secrets = Vec::new();
    for key in ["env", "headers"] {
        if let Some(Value::Object(values)) = entry.get(key) {
            for value in values.values().filter_map(Value::as_str) {
                for secret in mcp_secret_references(value) {
                    if !secrets.contains(&secret) {
                        secrets.push(secret);
                    }
                }
            }
        }
    }
    secrets
}

/// Deletes the secrets `removed` referred to and, for a remote server, its sign-in,
/// except what a server of the same name in another scope (`remaining`) still uses.
fn forget_credentials(
    name: &str,
    removed: &Value,
    remaining: &[(McpInstallScope, Value)],
    settings_path: PathBuf,
) -> McpRemoveOutcome {
    let still_used = remaining
        .iter()
        .flat_map(|(_, entry)| entry_secret_references(entry))
        .collect::<Vec<_>>();
    let secrets_deleted = entry_secret_references(removed)
        .into_iter()
        .filter(|secret| !still_used.contains(secret))
        .filter(|secret| delete_mcp_secret(name, secret).is_ok())
        .collect();
    let remote = |entry: &Value| entry.get("url").is_some();
    let signed_out = remote(removed)
        && !remaining.iter().any(|(_, entry)| remote(entry))
        && crate::mcp_oauth::clear_mcp_tokens(name).is_ok();
    McpRemoveOutcome {
        name: name.to_string(),
        settings_path,
        secrets_deleted,
        signed_out,
        still_used_by: remaining.first().map(|(scope, _)| *scope),
    }
}

fn remove_refusal(loader: &ConfigLoader, name: &str, scope: Option<McpInstallScope>) -> String {
    let configured = loader.load().ok().and_then(|config| {
        let mcp = config.mcp();
        mcp.get(name)
            .or_else(|| mcp.disabled().get(name))
            .or_else(|| mcp.needs_approval().get(name))
            .or_else(|| mcp.blocked().get(name).map(|blocked| &blocked.server))
            .map(|server| server.scope)
    });
    match (configured, scope) {
        (Some(ConfigSource::Managed), _) => format!(
            "`{name}` is set by the managed settings; ask the administrator of this computer to change them"
        ),
        (Some(ConfigSource::Bundled), _) => format!(
            "`{name}` ships with Neuron; turn it off with /mcp disable {name}, or set \"defaultMcpServers\": false"
        ),
        (Some(source), Some(scope)) if McpInstallScope::of(source) != Some(scope) => format!(
            "`{name}` is not in the {} settings ({})",
            scope.as_str(),
            scope.settings_path(loader).display()
        ),
        (Some(_), _) => format!(
            "`{name}` is configured in a settings file Neuron does not edit (such as .claw.json or .mcp.json); remove it there"
        ),
        (None, _) => format!("no MCP server named `{name}` is configured"),
    }
}

/// The configured servers (started, switched off or waiting) written from each catalog
/// entry: names by catalog id.
#[must_use]
pub fn mcp_catalog_servers(
    mcp: &crate::config::McpConfigCollection,
) -> BTreeMap<String, Vec<String>> {
    let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let servers = mcp
        .servers()
        .iter()
        .chain(mcp.disabled())
        .chain(mcp.needs_approval())
        .chain(
            mcp.blocked()
                .iter()
                .map(|(name, blocked)| (name, &blocked.server)),
        );
    for (name, server) in servers {
        if server.scope == ConfigSource::Bundled {
            continue;
        }
        if let Some(entry) = mcp_catalog_entry_for(&server.config) {
            found
                .entry(entry.id.clone())
                .or_default()
                .push(name.clone());
        }
    }
    found
}

/// Whether `config` was written from a catalog entry (see
/// [`McpCatalog::entry_for_config`]), and which.
#[must_use]
pub fn mcp_catalog_entry_for(config: &McpServerConfig) -> Option<&'static McpCatalogEntry> {
    McpCatalog::builtin().entry_for_config(config)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        install_mcp_server, load_mcp_secret, remove_mcp_server, resolve_mcp_secret_references,
        save_mcp_secret, McpInstallRequest, McpInstallScope,
    };
    use crate::config::{ConfigLoader, ProjectTrust};
    use crate::mcp_oauth::tests::install_memory_store;

    fn temp_root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("neuron-mcp-install-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("repo")).expect("workspace");
        fs::create_dir_all(root.join("home")).expect("config home");
        root
    }

    fn loader(root: &Path, trust: ProjectTrust) -> ConfigLoader {
        ConfigLoader::new(root.join("repo"), root.join("home"))
            .with_project_trust(trust)
            .with_mcp_approvals(true)
    }

    fn github(name: &str, token: &str, scope: Option<McpInstallScope>) -> McpInstallRequest {
        McpInstallRequest {
            id: "github".to_string(),
            name: Some(name.to_string()),
            scope,
            values: BTreeMap::from([(
                "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                token.to_string(),
            )]),
        }
    }

    #[test]
    fn the_connector_policy_refuses_adding_a_denied_connector_and_removing_a_required_one() {
        install_memory_store();
        let root = temp_root("policy");
        let managed = root.join("managed-settings.json");
        fs::write(
            &managed,
            r#"{"mcpPolicy":{"deny":["github"],"required":["fetch"]}}"#,
        )
        .unwrap();
        let loader =
            loader(&root, ProjectTrust::Trusted).with_managed_settings(Some(managed.clone()));

        let refused = install_mcp_server(&loader, &github("gh-denied", "ghp_x", None))
            .expect_err("a denied connector is not added");
        assert!(refused.contains("mcpPolicy.deny"), "{refused}");
        let user_settings = root.join("home").join("settings.json");
        assert!(
            !user_settings.exists()
                || !fs::read_to_string(&user_settings)
                    .unwrap()
                    .contains("gh-denied"),
            "nothing is written"
        );
        assert_eq!(
            super::load_mcp_secret("gh-denied", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None,
            "nor kept in the keychain"
        );

        let refused = remove_mcp_server(&loader, "fetch", None)
            .expect_err("a required connector is not removed");
        assert!(
            refused.contains("required by managed settings"),
            "{refused}"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn adding_github_stores_the_token_in_the_keychain_and_only_a_reference_in_settings() {
        install_memory_store();
        let root = temp_root("github");
        let loader = loader(&root, ProjectTrust::Trusted);
        let sentinel = "ghp_SENTINEL_install_test_1";
        let outcome =
            install_mcp_server(&loader, &github("gh-install", sentinel, None)).expect("add");
        assert_eq!(outcome.scope, McpInstallScope::User);
        assert_eq!(outcome.secrets_stored, vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]);
        assert!(!outcome.approved, "a user server needs no approval");

        let written = fs::read_to_string(&outcome.settings_path).unwrap();
        assert!(!written.contains(sentinel), "{written}");
        assert!(written.contains("${secret:GITHUB_PERSONAL_ACCESS_TOKEN}"));
        assert_eq!(
            load_mcp_secret("gh-install", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some(sentinel.to_string())
        );
        let github_url = "url:https://api.githubcopilot.com/mcp/";
        assert_eq!(
            resolve_mcp_secret_references(
                "gh-install",
                github_url,
                "Bearer ${secret:GITHUB_PERSONAL_ACCESS_TOKEN}"
            )
            .unwrap(),
            format!("Bearer {sentinel}")
        );
        // A server of the same name that points elsewhere does not get it.
        let elsewhere = resolve_mcp_secret_references(
            "gh-install",
            "url:https://collector.example.com/mcp",
            "Bearer ${secret:GITHUB_PERSONAL_ACCESS_TOKEN}",
        )
        .unwrap_err();
        assert!(
            elsewhere.contains("another command, environment or URL"),
            "{elsewhere}"
        );
        assert!(!elsewhere.contains(sentinel));

        let config = loader.load().unwrap();
        let server = config.mcp().get("gh-install").expect("configured");
        assert_eq!(
            super::mcp_catalog_entry_for(&server.config).map(|entry| entry.id.as_str()),
            Some("github")
        );

        // Adding it again the same way stores the token again (a new one replaces it).
        let again =
            install_mcp_server(&loader, &github("gh-install", "ghp_rotated", None)).expect("again");
        assert!(again.already);
        assert_eq!(
            load_mcp_secret("gh-install", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some("ghp_rotated".to_string())
        );

        let removed = remove_mcp_server(&loader, "gh-install", None).expect("remove");
        assert_eq!(
            removed.secrets_deleted,
            vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]
        );
        assert!(removed.signed_out);
        assert_eq!(
            load_mcp_secret("gh-install", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None
        );
        assert!(loader.load().unwrap().mcp().get("gh-install").is_none());
        assert!(remove_mcp_server(&loader, "gh-install", None)
            .unwrap_err()
            .contains("no MCP server"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_secret_names_it_and_never_a_value() {
        install_memory_store();
        let error =
            resolve_mcp_secret_references("never-stored", "url:x", "Bearer ${secret:TOKEN}")
                .unwrap_err();
        assert!(error.contains("TOKEN") && error.contains("never-stored"));
        save_mcp_secret("stored-one", "TOKEN", "value-xyz", "url:x").unwrap();
        assert_eq!(
            resolve_mcp_secret_references("stored-one", "url:x", "${HOME} ${secret:TOKEN}")
                .unwrap(),
            "${HOME} value-xyz"
        );
    }

    #[test]
    fn header_values_take_secrets_from_the_keychain_and_the_rest_from_the_environment() {
        install_memory_store();
        let url = "https://mcp.example.com/mcp";
        save_mcp_secret("header-test", "KEY", "k-${HOME}-1", &format!("url:{url}")).unwrap();
        std::env::set_var("NEURON_TEST_MCP_INSTALL_HEADER", "env-1");
        assert_eq!(
            super::expand_mcp_header(
                "header-test",
                url,
                "Bearer ${secret:KEY} ${NEURON_TEST_MCP_INSTALL_HEADER}"
            )
            .unwrap(),
            "Bearer k-${HOME}-1 env-1"
        );
        assert!(
            super::expand_mcp_header("header-test", url, "${secret:OTHER}")
                .unwrap_err()
                .contains("OTHER")
        );
        assert!(super::expand_mcp_header(
            "header-test",
            "https://other.example.com/mcp",
            "${secret:KEY}"
        )
        .is_err());

        let env = BTreeMap::from([
            ("API_KEY".to_string(), "${secret:STDIO_KEY}".to_string()),
            ("PLAIN".to_string(), "${HOME}".to_string()),
        ]);
        save_mcp_secret(
            "header-test",
            "STDIO_KEY",
            "s-1",
            &super::stdio_target("server", &[], &env),
        )
        .unwrap();
        let transport = crate::mcp_client::McpStdioTransport {
            command: "server".to_string(),
            args: Vec::new(),
            env,
            tool_call_timeout_ms: None,
            sandbox: crate::config::McpSandboxProfile::default(),
        };
        let resolved = super::stdio_transport_with_secrets("header-test", &transport).unwrap();
        assert_eq!(resolved.env["API_KEY"], "s-1");
        assert_eq!(
            resolved.env["PLAIN"], "${HOME}",
            "other text is left as written"
        );
        assert!(super::stdio_transport_with_secrets("no-such-server", &transport).is_err());
        let mut moved = transport.clone();
        moved.args.push("--other".to_string());
        assert!(super::stdio_transport_with_secrets("header-test", &moved).is_err());
        // Extra environment can run another program with the same command line.
        for (name, value) in [
            ("NODE_OPTIONS", "--require ./x.js"),
            ("npm_config_registry", "https://registry.example.com"),
        ] {
            let mut injected = transport.clone();
            injected.env.insert(name.to_string(), value.to_string());
            let error = super::stdio_transport_with_secrets("header-test", &injected)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("another command, environment or URL"),
                "{error}"
            );
            assert!(!error.contains("s-1"), "{error}");
        }
    }

    #[test]
    fn a_project_scope_add_in_an_untrusted_folder_is_refused_with_the_reason() {
        install_memory_store();
        let root = temp_root("untrusted");
        let untrusted = loader(&root, ProjectTrust::Untrusted);
        let error = install_mcp_server(
            &untrusted,
            &github("gh-untrusted", "ghp_x", Some(McpInstallScope::Project)),
        )
        .unwrap_err();
        assert!(error.contains("not trusted"), "{error}");
        assert!(error.contains("--scope user"), "{error}");
        assert!(!root.join("repo").join(".neuron").exists());
        assert_eq!(
            load_mcp_secret("gh-untrusted", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None,
            "nothing stored when refused"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_project_scope_add_is_approved_as_written_and_holds_no_secret() {
        install_memory_store();
        let root = temp_root("project");
        let trusted = loader(&root, ProjectTrust::Trusted);
        let sentinel = "ghp_SENTINEL_project_test_2";
        let outcome = install_mcp_server(
            &trusted,
            &github("gh-project", sentinel, Some(McpInstallScope::Project)),
        )
        .expect("add");
        assert!(outcome.approved);
        assert_eq!(
            outcome.settings_path,
            root.join("repo").join(".neuron").join("settings.json")
        );
        assert!(!fs::read_to_string(&outcome.settings_path)
            .unwrap()
            .contains(sentinel));
        let config = trusted.load().unwrap();
        assert!(
            config.mcp().get("gh-project").is_some(),
            "approved, so it loads"
        );
        assert!(config.mcp().needs_approval().is_empty());
        let _ = fs::remove_dir_all(root);
    }

    /// Secrets and sign-ins are kept per name: one name in two scopes would share them.
    /// Adding it in a second scope is refused, and removing one of two that the settings
    /// already hold keeps what the other still uses.
    #[test]
    fn a_name_in_two_scopes_is_refused_and_removing_one_keeps_the_others_secret() {
        use crate::mcp_oauth::{mcp_token_key, save_mcp_tokens, token_store, McpOAuthTokens};

        install_memory_store();
        let root = temp_root("two-scopes");
        let loader = loader(&root, ProjectTrust::Trusted);
        let user = install_mcp_server(
            &loader,
            &github("gh-both", "ghp_user_1", Some(McpInstallScope::User)),
        )
        .expect("user add");
        let refused = install_mcp_server(
            &loader,
            &github("gh-both", "ghp_project_2", Some(McpInstallScope::Project)),
        )
        .unwrap_err();
        assert!(
            refused.contains("already in the user settings"),
            "{refused}"
        );
        assert!(refused.contains("--name"), "{refused}");
        assert_eq!(
            load_mcp_secret("gh-both", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some("ghp_user_1".to_string()),
            "the user server's token is not replaced"
        );
        assert!(!root.join("repo").join(".neuron").exists());

        // The same entry in the project settings too, written by hand or an older build.
        let project_settings = root.join("repo").join(".neuron").join("settings.json");
        fs::create_dir_all(project_settings.parent().unwrap()).unwrap();
        fs::write(
            &project_settings,
            fs::read_to_string(&user.settings_path).unwrap(),
        )
        .unwrap();
        save_mcp_tokens(
            "gh-both",
            &McpOAuthTokens {
                access_token: "oauth-access".to_string(),
                refresh_token: None,
                expires_at: None,
                token_endpoint: "https://example.com/token".to_string(),
                client_id: "client".to_string(),
                client_secret: None,
                resource: "https://api.githubcopilot.com/mcp".to_string(),
                scope: None,
            },
        )
        .unwrap();
        let signed_in = || {
            token_store()
                .unwrap()
                .load(&mcp_token_key("gh-both"))
                .unwrap()
                .is_some()
        };

        let removed = remove_mcp_server(&loader, "gh-both", Some(McpInstallScope::Project))
            .expect("remove the project one");
        assert!(removed.secrets_deleted.is_empty(), "{removed:?}");
        assert!(!removed.signed_out);
        assert_eq!(removed.still_used_by, Some(McpInstallScope::User));
        assert_eq!(
            load_mcp_secret("gh-both", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some("ghp_user_1".to_string()),
            "the user server still starts"
        );
        assert!(signed_in());

        let last = remove_mcp_server(&loader, "gh-both", None).expect("remove the user one");
        assert_eq!(last.secrets_deleted, vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]);
        assert!(last.signed_out);
        assert_eq!(last.still_used_by, None);
        assert_eq!(
            load_mcp_secret("gh-both", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None
        );
        assert!(!signed_in());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refusals_name_what_is_wrong() {
        install_memory_store();
        let root = temp_root("refusals");
        let loader = loader(&root, ProjectTrust::Trusted);
        let request = |id: &str| McpInstallRequest {
            id: id.to_string(),
            name: None,
            scope: None,
            values: BTreeMap::new(),
        };
        assert!(install_mcp_server(&loader, &request("githb"))
            .unwrap_err()
            .contains("not in the connector catalog"));
        let missing = install_mcp_server(&loader, &request("github")).unwrap_err();
        assert!(
            missing.contains("GITHUB_PERSONAL_ACCESS_TOKEN"),
            "{missing}"
        );

        // An optional secret may be left out; a name in use with another spec is refused.
        install_mcp_server(&loader, &request("context7")).expect("context7");
        let clash = McpInstallRequest {
            id: "cloudflare-docs".to_string(),
            name: Some("context7".to_string()),
            scope: None,
            values: BTreeMap::new(),
        };
        assert!(install_mcp_server(&loader, &clash)
            .unwrap_err()
            .contains("already configured"));
        let bad_name = McpInstallRequest {
            name: Some("../x".to_string()),
            ..request("fetch")
        };
        assert!(install_mcp_server(&loader, &bad_name).is_err());
        assert!(remove_mcp_server(&loader, "playwright", None)
            .unwrap_err()
            .contains("ships with Neuron"));
        let _ = fs::remove_dir_all(root);
    }

    /// The acceptance flow with the shipped `github` entry, its URL pointed at a fake
    /// server (the only change): `add github --scope user` stores the token in the
    /// keychain and only a reference in settings, the server is ready (no approval to
    /// wait for) and starts with its tools, gets the token as a header, and removing it
    /// deletes the keychain entry.
    #[test]
    fn an_added_github_connector_starts_with_its_tools_using_the_keychain_secret() {
        use std::sync::Arc;

        use crate::mcp_catalog::McpCatalog;
        use crate::mcp_stdio::McpServerManager;
        use crate::mcp_test_http::{mcp_reply, FakeRequest, FakeResponse, FakeServer};

        install_memory_store();
        let server = FakeServer::spawn(Arc::new(|request: &FakeRequest| {
            if request.method != "POST" {
                return FakeResponse::status(405);
            }
            let message = request.json();
            if message.get("method").and_then(serde_json::Value::as_str) == Some("tools/list") {
                return FakeResponse::json(
                    200,
                    &serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {"tools": [
                            {"name": "get_me", "inputSchema": {"type": "object"}},
                            {"name": "merge_pull_request", "inputSchema": {"type": "object"}}
                        ]}
                    }),
                );
            }
            match mcp_reply(&message) {
                None => FakeResponse::status(202),
                Some(reply) => FakeResponse::json(200, &reply),
            }
        }));
        let mut entry = McpCatalog::builtin().get("github").unwrap().clone();
        entry.install.url = Some(format!("{}/mcp/", server.url));
        let catalog = McpCatalog {
            version: "test".to_string(),
            connectors: vec![entry],
        };
        let root = temp_root("e2e");
        let loader = loader(&root, ProjectTrust::Trusted);
        let sentinel = "ghp_SENTINEL_e2e_3";
        let mut request = github("github", sentinel, Some(McpInstallScope::User));
        request.name = None;
        let outcome = super::install_from_catalog(&loader, &catalog, &request).expect("add");
        assert_eq!(outcome.name, "github");
        assert_eq!(
            outcome.settings_path,
            root.join("home").join("settings.json")
        );
        let written = fs::read_to_string(&outcome.settings_path).unwrap();
        assert!(!written.contains(sentinel), "{written}");
        assert!(written.contains("${secret:GITHUB_PERSONAL_ACCESS_TOKEN}"));
        assert_eq!(
            load_mcp_secret("github", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some(sentinel.to_string())
        );

        let config = loader.load().unwrap();
        assert!(config.mcp().needs_approval().is_empty());
        let servers = config
            .mcp()
            .servers()
            .iter()
            .filter(|(name, _)| *name == "github")
            .map(|(name, server)| (name.clone(), server.clone()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(servers.len(), 1, "github is ready");
        let mut manager = McpServerManager::from_servers(&servers);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let report = runtime.block_on(manager.discover_tools_best_effort());
        assert!(
            report.failed_servers.is_empty(),
            "{:?}",
            report.failed_servers
        );
        assert_eq!(
            report
                .tools
                .iter()
                .map(|tool| tool.qualified_name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp__github__get_me", "mcp__github__merge_pull_request"]
        );
        runtime.block_on(manager.shutdown()).unwrap();
        let expected = format!("Bearer {sentinel}");
        let posts = server
            .requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .collect::<Vec<_>>();
        assert!(!posts.is_empty());
        assert!(posts
            .iter()
            .all(|request| request.header("authorization") == Some(expected.as_str())));

        remove_mcp_server(&loader, "github", None).expect("remove");
        assert_eq!(
            load_mcp_secret("github", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_request_debug_output_hides_values() {
        let text = format!("{:?}", github("x", "ghp_hidden_value", None));
        assert!(!text.contains("ghp_hidden_value"), "{text}");
        assert!(text.contains("GITHUB_PERSONAL_ACCESS_TOKEN"));
    }

    /// Fails every download, as a computer with no network does.
    struct Offline(std::sync::Mutex<Vec<String>>);

    impl super::McpFetch for Offline {
        fn fetch_to(&self, url: &str, _dest: &Path) -> Result<(), String> {
            self.0.lock().unwrap().push(url.to_string());
            Err(format!("offline: {url}"))
        }
    }

    fn bundled_transport(name: &str) -> crate::mcp_client::McpStdioTransport {
        let config = crate::config::bundled_mcp_server_config(name).expect("bundled");
        match crate::mcp_client::McpClientTransport::from_named_config(
            name,
            Some(crate::config::ConfigSource::Bundled),
            &config,
        ) {
            crate::mcp_client::McpClientTransport::Stdio(transport) => transport,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn with_no_network_the_bundled_servers_start_from_their_verified_installs() {
        if std::env::var_os("NEURON_PLAYWRIGHT_MCP_ENTRY").is_some() {
            return; // the npm launcher's own install is used instead
        }
        let root = temp_root("offline-bundled");
        let store = super::McpInstallStore::in_config_home(&root.join("home"));
        let offline = Offline(std::sync::Mutex::new(Vec::new()));

        // Before the first install there is nothing to start, and `npx` is never run.
        let codebase = bundled_transport("codebase-memory");
        let error =
            super::prepare_stdio_transport_in("codebase-memory", &codebase, &store, &offline)
                .unwrap_err()
                .to_string();
        assert!(
            error.contains("offline: https://github.com/DeusData/"),
            "{error}"
        );

        // Installed earlier (here: recorded as an install leaves it), both start offline.
        let codebase_spec = super::mcp_verified_spec_for(&codebase).unwrap().unwrap();
        let program = if cfg!(windows) {
            "codebase-memory-mcp.exe"
        } else {
            "codebase-memory-mcp"
        };
        let codebase_dir =
            super::verified::record_install(&store, &codebase_spec, &[(program, b"binary")]);
        let started =
            super::prepare_stdio_transport_in("codebase-memory", &codebase, &store, &offline)
                .unwrap();
        assert_eq!(
            started.command,
            codebase_dir.join(program).to_string_lossy()
        );
        assert!(started.args.is_empty());
        assert!(started
            .sandbox
            .fs_read
            .contains(&codebase_dir.to_string_lossy().into_owned()));

        let playwright = bundled_transport("playwright");
        let playwright_spec = super::mcp_verified_spec_for(&playwright).unwrap().unwrap();
        let cli = "node_modules/@playwright/mcp/cli.js";
        let playwright_dir =
            super::verified::record_install(&store, &playwright_spec, &[(cli, b"// cli")]);
        let started =
            super::prepare_stdio_transport_in("playwright", &playwright, &store, &offline).unwrap();
        assert_ne!(started.command, "npx");
        assert_ne!(started.command, "cmd.exe");
        assert_eq!(started.args.len(), 1);
        assert_eq!(Path::new(&started.args[0]), playwright_dir.join(cli));
        assert_eq!(
            offline.0.lock().unwrap().len(),
            1,
            "only the first try fetched"
        );

        // A server that is not a catalog connector starts as written.
        let mut custom = playwright.clone();
        custom.args.push("--headless".to_string());
        let started =
            super::prepare_stdio_transport_in("custom", &custom, &store, &offline).unwrap();
        assert_eq!(
            (started.command, started.args),
            (custom.command, custom.args)
        );
        let _ = fs::remove_dir_all(root);
    }
}
