//! The connector catalog in the engine: [`EngineCommand::ListConnectorCatalog`],
//! [`EngineCommand::AddMcpServer`], [`EngineCommand::RemoveMcpServer`] and
//! [`EngineCommand::ApproveMcpServer`]. Each change says what it did and lists the
//! extensions again; servers restart from the next turn, as after `/mcp enable`.

use std::env;
use std::path::Path;

use neuron_tui::protocol::{ConnectorInfo, ConnectorVariable, EngineCommand, UiEvent};
use runtime::{ConfigLoader, McpCatalog, McpInstallRequest, McpInstallScope};

/// `ListConnectorCatalog`: the connectors matching `query`, with the servers added
/// from each.
pub(crate) fn connector_catalog_event(query: &str) -> UiEvent {
    connector_catalog_event_in(&env::current_dir().unwrap_or_default(), query)
}

fn connector_catalog_event_in(cwd: &Path, query: &str) -> UiEvent {
    let added = ConfigLoader::default_for(cwd)
        .load()
        .map(|config| runtime::mcp_catalog_servers(config.mcp()))
        .unwrap_or_default();
    let catalog = McpCatalog::builtin();
    UiEvent::ConnectorCatalog {
        version: catalog.version.clone(),
        connectors: catalog
            .search(query)
            .into_iter()
            .map(|entry| ConnectorInfo {
                id: entry.id.clone(),
                name: entry.name.clone(),
                description: entry.description.clone(),
                publisher: entry.publisher.clone(),
                official: entry.official,
                licence: entry.licence.clone(),
                source: entry.source.clone(),
                transport: if entry.is_remote() { "http" } else { "stdio" }.to_string(),
                pinned: entry.pinned_label(),
                oauth: entry.install.oauth,
                default_scope: entry.default_scope.clone(),
                variables: entry
                    .env
                    .iter()
                    .map(|variable| ConnectorVariable {
                        name: variable.name.clone(),
                        label: variable.label.clone(),
                        secret: variable.secret,
                        required: variable.required,
                        default: variable.default.clone(),
                        help_url: variable.help_url.clone(),
                    })
                    .collect(),
                added: added.get(&entry.id).cloned().unwrap_or_default(),
            })
            .collect(),
    }
}

/// `AddMcpServer`, `RemoveMcpServer` or `ApproveMcpServer`: a notice (or an error), then
/// the extensions again. Other commands answer nothing.
pub(crate) fn mcp_change_events(command: EngineCommand) -> Vec<UiEvent> {
    let cwd = env::current_dir().unwrap_or_default();
    let first = mcp_change_notice(&ConfigLoader::default_for(&cwd), command);
    match first {
        Some(first) => vec![first, crate::engine::extensions_event()],
        None => Vec::new(),
    }
}

fn mcp_change_notice(loader: &ConfigLoader, command: EngineCommand) -> Option<UiEvent> {
    let notice = |message: String| UiEvent::Notice {
        message,
        kind: None,
    };
    let scope = |scope: Option<String>| scope.as_deref().map(McpInstallScope::parse).transpose();
    Some(match command {
        EngineCommand::AddMcpServer {
            id,
            name,
            scope: requested,
            secrets,
        } => {
            let outcome = scope(requested).and_then(|scope| {
                runtime::install_mcp_server(
                    loader,
                    &McpInstallRequest {
                        id: id.clone(),
                        name,
                        scope,
                        values: secrets.0,
                    },
                )
            });
            match outcome {
                Err(error) => UiEvent::error(format!("could not add {id}: {error}")),
                Ok(outcome) => {
                    let mut message = if outcome.already {
                        format!("MCP server {} was already added", outcome.name)
                    } else {
                        format!(
                            "Added MCP server {} ({}) to the {} settings",
                            outcome.name,
                            outcome.pinned,
                            outcome.scope.as_str()
                        )
                    };
                    if !outcome.secrets_stored.is_empty() {
                        message.push_str(&format!(
                            "; {} kept in the OS keychain",
                            outcome.secrets_stored.join(", ")
                        ));
                    }
                    if outcome.needs_login {
                        message.push_str(&format!(
                            ". Sign in with `neuron mcp login {}` in a terminal",
                            outcome.name
                        ));
                    }
                    message.push_str(". It starts from the next turn.");
                    notice(message)
                }
            }
        }
        EngineCommand::RemoveMcpServer {
            name,
            scope: requested,
        } => match scope(requested)
            .and_then(|scope| runtime::remove_mcp_server(loader, &name, scope))
        {
            Err(error) => UiEvent::error(format!("could not remove MCP server {name}: {error}")),
            Ok(outcome) => {
                let mut message = format!(
                    "Removed MCP server {} from {}",
                    outcome.name,
                    outcome.settings_path.display()
                );
                if !outcome.secrets_deleted.is_empty() {
                    message.push_str(&format!(
                        "; deleted {} from the OS keychain",
                        outcome.secrets_deleted.join(", ")
                    ));
                }
                message.push('.');
                notice(message)
            }
        },
        EngineCommand::ApproveMcpServer { name } => {
            match runtime::approve_mcp_server(loader, &name) {
                Err(error) => {
                    UiEvent::error(format!("could not approve MCP server {name}: {error}"))
                }
                Ok(outcome) if outcome.disabled => notice(format!(
                    "MCP server {name} is approved for this workspace; it starts once turned on."
                )),
                Ok(_) => notice(format!(
                    "MCP server {name} is approved for this workspace; it starts from the next turn."
                )),
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::sync::{Arc, Mutex, OnceLock};

    use neuron_tui::protocol::{EngineCommand, SecretValues, UiEvent};
    use runtime::{ConfigLoader, McpTokenStore, ProjectTrust};

    use super::{connector_catalog_event_in, mcp_change_notice};

    #[derive(Default)]
    struct MemorySecrets(Mutex<BTreeMap<String, String>>);

    impl McpTokenStore for MemorySecrets {
        fn load(&self, key: &str) -> Result<Option<String>, String> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        fn save(&self, key: &str, value: &str) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), String> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn message(event: &UiEvent) -> String {
        match event {
            UiEvent::Notice { message, .. } | UiEvent::Error { message, .. } => message.clone(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn add_remove_and_approve_commands_change_the_settings_and_never_echo_a_secret() {
        static STORE: OnceLock<()> = OnceLock::new();
        STORE.get_or_init(|| {
            runtime::set_mcp_token_store(Some(Arc::new(MemorySecrets::default())));
        });
        let root = std::env::temp_dir().join(format!("neuron-engine-mcp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (cwd, home) = (root.join("repo"), root.join("home"));
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("settings.json"), r#"{"defaultMcpServers":false}"#).unwrap();
        let loader = ConfigLoader::new(&cwd, &home)
            .with_project_trust(ProjectTrust::Trusted)
            .with_mcp_approvals(true);
        let sentinel = "ghp_SENTINEL_engine_test";

        let added = mcp_change_notice(
            &loader,
            EngineCommand::AddMcpServer {
                id: "github".into(),
                name: Some("gh-engine".into()),
                scope: Some("project".into()),
                secrets: SecretValues(BTreeMap::from([(
                    "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                    sentinel.to_string(),
                )])),
            },
        )
        .unwrap();
        let text = message(&added);
        assert!(text.starts_with("Added MCP server gh-engine"), "{text}");
        assert!(!text.contains(sentinel));
        let settings = fs::read_to_string(cwd.join(".neuron").join("settings.json")).unwrap();
        assert!(!settings.contains(sentinel), "{settings}");
        assert_eq!(
            runtime::load_mcp_secret("gh-engine", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            Some(sentinel.to_string())
        );
        // Added by the user here, so approved as written.
        assert!(loader.load().unwrap().mcp().get("gh-engine").is_some());

        let refused = mcp_change_notice(
            &loader,
            EngineCommand::AddMcpServer {
                id: "github".into(),
                name: None,
                scope: Some("team".into()),
                secrets: SecretValues::default(),
            },
        )
        .unwrap();
        assert!(matches!(refused, UiEvent::Error { .. }));
        assert!(message(&refused).contains("unknown scope"));

        let not_waiting = mcp_change_notice(
            &loader,
            EngineCommand::ApproveMcpServer {
                name: "gh-engine".into(),
            },
        )
        .unwrap();
        assert!(
            message(&not_waiting).contains("approved"),
            "{not_waiting:?}"
        );

        let removed = mcp_change_notice(
            &loader,
            EngineCommand::RemoveMcpServer {
                name: "gh-engine".into(),
                scope: None,
            },
        )
        .unwrap();
        assert!(
            message(&removed).contains("deleted GITHUB_PERSONAL_ACCESS_TOKEN"),
            "{removed:?}"
        );
        assert_eq!(
            runtime::load_mcp_secret("gh-engine", "GITHUB_PERSONAL_ACCESS_TOKEN").unwrap(),
            None
        );
        assert!(mcp_change_notice(&loader, EngineCommand::ListExtensions).is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_catalog_event_lists_matching_connectors_with_their_variables() {
        let cwd = std::env::temp_dir();
        let UiEvent::ConnectorCatalog {
            version,
            connectors,
        } = connector_catalog_event_in(&cwd, "github")
        else {
            panic!("a catalog event");
        };
        assert!(!version.is_empty());
        assert_eq!(connectors.len(), 1);
        assert_eq!(connectors[0].transport, "http");
        assert!(connectors[0].variables[0].secret);
        let UiEvent::ConnectorCatalog { connectors, .. } = connector_catalog_event_in(&cwd, "")
        else {
            panic!("a catalog event");
        };
        assert!(connectors.len() >= 10);
    }
}
