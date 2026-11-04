//! Per-server approval for MCP servers a workspace brings. Trusting a folder lets
//! its settings apply, but a `git pull` can later add or change an `mcpServers`
//! entry; without this, that new process would start silently. So a Project or
//! Local server (from `.neuron/settings*.json`, `.claw/...`, `.neuron.json` or a
//! project `.mcp.json`) starts only once the user approved that exact launch spec
//! for that workspace: until then it is listed with status `needs_approval` and
//! nothing is spawned.
//!
//! Approvals live in `<config home>/mcp-approvals.json`, keyed by the workspace
//! root (its git root, or the folder outside git), the server name and the sha256
//! of its launch spec: transport, command, arguments, the names (not values) of
//! its environment variables and headers, the headers helper and the URL. Changing
//! any of these asks again. Bundled and User servers need no approval.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::config::{ConfigLoader, ConfigSource, McpServerConfig};
use crate::mcp_toggle::{read_object, write_object};

/// The approvals file in the config home.
pub const MCP_APPROVALS_FILE: &str = "mcp-approvals.json";

/// Whether a server from `scope` starts only once approved.
#[must_use]
pub fn mcp_scope_needs_approval(scope: ConfigSource) -> bool {
    matches!(scope, ConfigSource::Project | ConfigSource::Local)
}

/// The sha256 (hex) of what starts or reaches the server; secrets (environment and
/// header values) are left out.
#[must_use]
pub fn mcp_launch_fingerprint(config: &McpServerConfig) -> String {
    let keys =
        |map: &std::collections::BTreeMap<String, String>| map.keys().cloned().collect::<Vec<_>>();
    let spec = match config {
        McpServerConfig::Stdio(stdio) => json!({
            "type": "stdio",
            "command": stdio.command,
            "args": stdio.args,
            "envKeys": keys(&stdio.env),
        }),
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => json!({
            "type": if matches!(config, McpServerConfig::Sse(_)) { "sse" } else { "http" },
            "url": remote.url,
            "headerKeys": keys(&remote.headers),
            "headersHelper": remote.headers_helper,
        }),
        McpServerConfig::Ws(socket) => json!({
            "type": "ws",
            "url": socket.url,
            "headerKeys": keys(&socket.headers),
            "headersHelper": socket.headers_helper,
        }),
        McpServerConfig::Sdk(sdk) => json!({"type": "sdk", "name": sdk.name}),
        McpServerConfig::ManagedProxy(proxy) => {
            json!({"type": "zerox", "url": proxy.url, "id": proxy.id})
        }
    };
    let digest = Sha256::digest(spec.to_string().as_bytes());
    digest
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The folder approvals for `cwd` are recorded under: its git root, or `cwd`
/// outside git, resolved when it can be.
#[must_use]
pub fn mcp_approval_workspace(cwd: &Path) -> PathBuf {
    let root = crate::workspace_trust::trust_root(cwd).root;
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// The approvals recorded in `<config home>/mcp-approvals.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpApprovals {
    path: PathBuf,
}

impl McpApprovals {
    #[must_use]
    pub fn in_config_home(config_home: &Path) -> Self {
        Self {
            path: config_home.join(MCP_APPROVALS_FILE),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether the server `name` with this launch spec was approved for `workspace`.
    /// An unreadable file approves nothing.
    #[must_use]
    pub fn is_approved(&self, workspace: &Path, name: &str, config: &McpServerConfig) -> bool {
        let fingerprint = mcp_launch_fingerprint(config);
        read_object(&self.path).is_ok_and(|object| {
            object
                .get("workspaces")
                .and_then(|workspaces| workspaces.get(workspace_key(workspace)))
                .and_then(|servers| servers.get(name))
                .and_then(|server| server.get("sha256"))
                .and_then(Value::as_str)
                == Some(fingerprint.as_str())
        })
    }

    /// Records the server `name` with this launch spec as approved for `workspace`.
    pub fn approve(
        &self,
        workspace: &Path,
        name: &str,
        config: &McpServerConfig,
    ) -> Result<(), String> {
        let mut object = read_object(&self.path)?;
        object.insert("version".to_string(), json!(1));
        let workspaces = object
            .entry("workspaces".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !workspaces.is_object() {
            *workspaces = Value::Object(Map::new());
        }
        let servers = workspaces
            .as_object_mut()
            .expect("an object")
            .entry(workspace_key(workspace))
            .or_insert_with(|| Value::Object(Map::new()));
        if !servers.is_object() {
            *servers = Value::Object(Map::new());
        }
        let approved_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        servers.as_object_mut().expect("an object").insert(
            name.to_string(),
            json!({"sha256": mcp_launch_fingerprint(config), "approvedAt": approved_at}),
        );
        write_object(&self.path, &object)
    }
}

fn workspace_key(workspace: &Path) -> String {
    workspace.display().to_string()
}

/// What [`approve_mcp_server`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpApprovalOutcome {
    /// The workspace the approval is recorded for.
    pub workspace: PathBuf,
    /// The server was approved already (with this launch spec).
    pub already: bool,
    /// The approvals file.
    pub approvals_path: PathBuf,
}

/// Approves the workspace's MCP server `name` as it is configured now. Refused for a
/// server that is not configured, and for one the workspace does not bring (bundled
/// and user servers need no approval).
pub fn approve_mcp_server(loader: &ConfigLoader, name: &str) -> Result<McpApprovalOutcome, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name an MCP server".to_string());
    }
    let config = loader.load().map_err(|error| error.to_string())?;
    let approvals = McpApprovals::in_config_home(loader.config_home());
    let outcome = |already| McpApprovalOutcome {
        workspace: mcp_approval_workspace(loader.cwd()),
        already,
        approvals_path: approvals.path().to_path_buf(),
    };
    let Some(server) = config.mcp().needs_approval().get(name) else {
        if let Some(blocked) = config.mcp().blocked().get(name) {
            return Err(format!(
                "`{name}` is refused by the permissions.deny rule '{}', so it does not start",
                blocked.rule
            ));
        }
        let known = config
            .mcp()
            .get(name)
            .or_else(|| config.mcp().disabled().get(name));
        return match known {
            Some(server) if mcp_scope_needs_approval(server.scope) => Ok(outcome(true)),
            Some(_) => Err(format!(
                "`{name}` is not a workspace server, so it needs no approval"
            )),
            None if !loader.workspace_is_trusted() => Err(format!(
                "this folder is not trusted, so its MCP servers are not loaded; trust it first, then approve `{name}`"
            )),
            None => Err(format!("no MCP server named `{name}` is configured")),
        };
    };
    approvals.approve(&mcp_approval_workspace(loader.cwd()), name, &server.config)?;
    Ok(outcome(false))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use super::{approve_mcp_server, mcp_launch_fingerprint, McpApprovals};
    use crate::config::{
        ConfigLoader, ConfigSource, McpServerConfig, McpStdioServerConfig, ProjectTrust,
    };

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "neuron-mcp-approvals-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("repo").join(".neuron")).expect("workspace");
        fs::create_dir_all(root.join("home")).expect("config home");
        root
    }

    fn write_servers(path: &std::path::Path, servers: &serde_json::Value) {
        fs::write(
            path,
            serde_json::json!({ "mcpServers": servers }).to_string(),
        )
        .expect("settings");
    }

    fn loader(root: &std::path::Path, trust: ProjectTrust) -> ConfigLoader {
        ConfigLoader::new(root.join("repo"), root.join("home"))
            .with_project_trust(trust)
            .with_mcp_approvals(true)
    }

    #[test]
    fn a_pulled_server_waits_for_approval_and_a_changed_one_asks_again() {
        let root = temp_root("pull");
        fs::write(
            root.join("home").join("settings.json"),
            r#"{"defaultMcpServers":false,"mcpServers":{"mine":{"command":"node","args":["mine.js"]}}}"#,
        )
        .expect("user settings");
        let project = root.join("repo").join(".neuron").join("settings.json");
        write_servers(
            &project,
            &serde_json::json!({"pulled": {"command": "node", "args": ["pulled.js"]}}),
        );
        let trusted = loader(&root, ProjectTrust::Trusted);

        let config = trusted.load().expect("config");
        // The user's own server needs no approval; the workspace's does.
        assert!(config.mcp().get("mine").is_some());
        assert!(config.mcp().get("pulled").is_none());
        assert_eq!(
            config
                .mcp()
                .needs_approval()
                .get("pulled")
                .map(|server| server.scope),
            Some(ConfigSource::Project)
        );

        let outcome = approve_mcp_server(&trusted, "pulled").expect("approve");
        assert!(!outcome.already);
        assert!(outcome.approvals_path.starts_with(root.join("home")));
        let config = trusted.load().expect("config");
        assert!(config.mcp().get("pulled").is_some());
        assert!(config.mcp().needs_approval().is_empty());
        assert!(
            approve_mcp_server(&trusted, "pulled")
                .expect("again")
                .already
        );
        assert!(approve_mcp_server(&trusted, "mine").is_err());
        assert!(approve_mcp_server(&trusted, "ghost").is_err());

        // A later pull changes what it runs: approval is asked again.
        write_servers(
            &project,
            &serde_json::json!({"pulled": {"command": "node", "args": ["evil.js"]}}),
        );
        let config = trusted.load().expect("config");
        assert!(config.mcp().get("pulled").is_none());
        assert!(config.mcp().needs_approval().contains_key("pulled"));

        // Without approvals (an explicit loader), the workspace server loads as before.
        let config = ConfigLoader::new(root.join("repo"), root.join("home"))
            .load()
            .expect("config");
        assert!(config.mcp().get("pulled").is_some());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_mcp_json_is_a_project_server_behind_trust_and_approval() {
        let root = temp_root("mcp-json");
        fs::write(
            root.join("home").join("settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .expect("user settings");
        write_servers(
            &root.join("repo").join(".mcp.json"),
            &serde_json::json!({
                "docs": {"command": "npx", "args": ["docs-server"]},
                "shared": {"type": "http", "url": "https://mcp.example.com/mcp"}
            }),
        );
        // The workspace's own settings win over .mcp.json for the same name.
        write_servers(
            &root.join("repo").join(".neuron").join("settings.json"),
            &serde_json::json!({"shared": {"command": "node", "args": ["local.js"]}}),
        );

        let untrusted = loader(&root, ProjectTrust::Untrusted)
            .load()
            .expect("config");
        assert!(untrusted.mcp().servers().is_empty());
        assert!(untrusted.mcp().needs_approval().is_empty());

        let trusted = loader(&root, ProjectTrust::Trusted);
        let config = trusted.load().expect("config");
        assert!(config.mcp().servers().is_empty());
        let waiting = config.mcp().needs_approval();
        assert_eq!(waiting.len(), 2);
        assert_eq!(waiting["docs"].scope, ConfigSource::Project);
        assert!(matches!(
            waiting["shared"].config,
            McpServerConfig::Stdio(_)
        ));

        approve_mcp_server(&trusted, "docs").expect("approve");
        let config = trusted.load().expect("config");
        assert!(config.mcp().get("docs").is_some());

        let preview = trusted.workspace_trust_preview();
        assert!(preview
            .mcp_servers
            .iter()
            .any(|server| server.name == "docs" && server.source.ends_with(".mcp.json")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_deny_rule_for_a_whole_server_blocks_it() {
        let root = temp_root("blocked");
        fs::write(
            root.join("home").join("settings.json"),
            r#"{"defaultMcpServers":false,"permissions":{"deny":["mcp__alpha"]},"mcpServers":{"alpha":{"command":"node"},"beta":{"command":"node"}}}"#,
        )
        .expect("user settings");
        let config = loader(&root, ProjectTrust::Trusted).load().expect("config");
        assert!(config.mcp().get("alpha").is_none());
        assert_eq!(config.mcp().blocked()["alpha"].rule, "mcp__alpha");
        assert!(config.mcp().get("beta").is_some());
        let _ = fs::remove_dir_all(&root);
    }

    fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpServerConfig {
        McpServerConfig::Stdio(McpStdioServerConfig {
            command: command.to_string(),
            args: args.iter().map(ToString::to_string).collect(),
            env: env
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<BTreeMap<_, _>>(),
            tool_call_timeout_ms: None,
        })
    }

    #[test]
    fn the_fingerprint_follows_the_launch_spec_but_not_secret_values() {
        let base = mcp_launch_fingerprint(&stdio("node", &["server.js"], &[("TOKEN", "a")]));
        assert_eq!(base.len(), 64);
        assert_eq!(
            base,
            mcp_launch_fingerprint(&stdio("node", &["server.js"], &[("TOKEN", "b")]))
        );
        for changed in [
            stdio("node", &["evil.js"], &[("TOKEN", "a")]),
            stdio("python", &["server.js"], &[("TOKEN", "a")]),
            stdio("node", &["server.js"], &[("OTHER", "a")]),
        ] {
            assert_ne!(base, mcp_launch_fingerprint(&changed), "{changed:?}");
        }
    }

    #[test]
    fn an_approval_covers_one_workspace_name_and_spec() {
        let home = std::env::temp_dir().join(format!(
            "neuron-mcp-approvals-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&home);
        let approvals = McpApprovals::in_config_home(&home);
        let repo = PathBuf::from("/work/repo");
        let server = stdio("node", &["server.js"], &[]);
        assert!(!approvals.is_approved(&repo, "docs", &server));

        approvals.approve(&repo, "docs", &server).expect("approve");
        assert!(approvals.is_approved(&repo, "docs", &server));
        assert!(!approvals.is_approved(&PathBuf::from("/work/other"), "docs", &server));
        assert!(!approvals.is_approved(&repo, "other", &server));
        assert!(!approvals.is_approved(&repo, "docs", &stdio("node", &["x.js"], &[])));

        // A second approval keeps the first.
        approvals.approve(&repo, "other", &server).expect("approve");
        assert!(approvals.is_approved(&repo, "docs", &server));
        let _ = fs::remove_dir_all(&home);
    }
}
