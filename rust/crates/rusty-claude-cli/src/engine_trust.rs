//! The folder trust screen's data ([`UiEvent::TrustPreview`]): where trust would be
//! recorded and what the workspace's own settings would turn on, read from its files
//! without starting or running anything.

use std::path::Path;

use neuron_tui::protocol::{HookPreview, McpServerPreview, PermissionRulePreview, UiEvent};
use runtime::ConfigLoader;

/// How long a hook command shown on the trust screen gets, in characters.
const HOOK_COMMAND_CHARS: usize = 200;

/// What trusting `cwd` would turn on, and where the trust would be recorded.
pub(crate) fn trust_preview_event(cwd: &Path) -> UiEvent {
    let root = runtime::workspace_trust::trust_root(cwd);
    let loader = ConfigLoader::default_for(cwd);
    let preview = loader.workspace_trust_preview();
    let shown = |path: &Path| path.display().to_string();
    UiEvent::TrustPreview {
        cwd: shown(cwd),
        root: shown(&root.root),
        is_git_root: root.is_git_root,
        session_only: root.session_only,
        trusted: loader.workspace_is_trusted(),
        settings_files: preview
            .settings_files
            .iter()
            .map(|path| shown(path))
            .collect(),
        hooks: preview
            .hooks
            .iter()
            .map(|hook| HookPreview {
                event: hook.event.to_string(),
                command: shorten(&hook.command),
                source: shown(&hook.source),
            })
            .collect(),
        mcp_servers: preview
            .mcp_servers
            .iter()
            .map(|server| McpServerPreview {
                name: server.name.clone(),
                transport: crate::engine::mcp_transport_id(server.transport).to_string(),
                target: server.target.clone(),
                source: shown(&server.source),
            })
            .collect(),
        permission_rules: preview
            .permission_rules
            .iter()
            .map(|rule| PermissionRulePreview {
                effect: rule.effect.to_string(),
                rule: rule.rule.clone(),
                source: shown(&rule.source),
            })
            .collect(),
        other_settings: preview.other_settings,
    }
}

fn shorten(command: &str) -> String {
    if command.chars().count() <= HOOK_COMMAND_CHARS {
        return command.to_string();
    }
    let kept: String = command.chars().take(HOOK_COMMAND_CHARS - 1).collect();
    format!("{kept}\u{2026}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_folder_with_settings_previews_them_without_running_them() {
        let root = std::env::temp_dir().join(format!(
            "neuron-trust-preview-{}-{}",
            std::process::id(),
            crate::clock::now_unix()
        ));
        let repo = root.join("repo");
        let cwd = repo.join("app");
        fs::create_dir_all(repo.join(".git")).expect("repo");
        fs::create_dir_all(cwd.join(".neuron")).expect("settings dir");
        let marker = cwd.join("ran.txt");
        fs::write(
            cwd.join(".neuron").join("settings.json"),
            serde_json::json!({
                "hooks": {"PreToolUse": [format!("echo ran > {}", marker.display()), "x".repeat(300)]},
                "mcpServers": {"postgres-tools": {"command": "uvx", "args": ["pg-mcp"]}},
                "permissions": {"allow": ["bash(npm test)"]}
            })
            .to_string(),
        )
        .expect("settings");

        match trust_preview_event(&cwd) {
            UiEvent::TrustPreview {
                root: trust_root,
                is_git_root,
                session_only,
                trusted,
                settings_files,
                hooks,
                mcp_servers,
                permission_rules,
                ..
            } => {
                assert_eq!(trust_root, repo.display().to_string());
                assert!(is_git_root);
                assert!(!session_only);
                assert!(!trusted);
                assert_eq!(settings_files.len(), 1);
                assert_eq!(hooks.len(), 2);
                assert_eq!(hooks[1].command.chars().count(), HOOK_COMMAND_CHARS);
                assert_eq!(mcp_servers[0].transport, "stdio");
                assert_eq!(mcp_servers[0].target.as_deref(), Some("uvx"));
                assert_eq!(permission_rules[0].effect, "allow");
            }
            other => panic!("a trust preview, got {other:?}"),
        }
        assert!(!marker.exists(), "nothing ran");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
