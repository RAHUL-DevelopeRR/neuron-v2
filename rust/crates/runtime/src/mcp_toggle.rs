//! Turning MCP servers on and off: `disabledMcpServers` in the USER settings file
//! (`<config home>/settings.json`). The user file is chosen because it is always
//! loaded, whatever the workspace trust, and a switch flipped in one workspace's UI
//! should not write into a repository. `disabledMcpServers` adds up across settings
//! files, so a server a project file turns off stays off; enabling reports that.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::config::ConfigLoader;

/// What [`set_mcp_server_enabled`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToggleOutcome {
    /// The settings file that holds the switch.
    pub settings_path: PathBuf,
    /// The file changed (the server was not already in the requested state there).
    pub changed: bool,
    /// Other settings files that still turn the server off.
    pub still_disabled_by: Vec<PathBuf>,
}

fn disabled_names(object: &Map<String, Value>) -> Vec<String> {
    object
        .get("disabledMcpServers")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn read_object(path: &Path) -> Result<Map<String, Value>, String> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(Map::new()),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(object)) => Ok(object),
            Ok(_) => Err(format!("{} is not a JSON object", path.display())),
            Err(error) => Err(format!("cannot parse {}: {error}", path.display())),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Map::new()),
        Err(error) => Err(format!("cannot read {}: {error}", path.display())),
    }
}

fn write_object(path: &Path, object: &Map<String, Value>) -> Result<(), String> {
    let failed = |error: io::Error| format!("cannot write {}: {error}", path.display());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(failed)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(object.clone()))
        .map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, format!("{text}\n")).map_err(failed)?;
    fs::rename(&temporary, path).map_err(failed)
}

/// Turns the MCP server `name` on or off in the user settings. Turning off a server
/// no settings file configures (nor the bundled defaults) is refused.
pub fn set_mcp_server_enabled(
    loader: &ConfigLoader,
    name: &str,
    enabled: bool,
) -> Result<McpToggleOutcome, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name an MCP server".to_string());
    }
    let settings_path = loader.config_home().join("settings.json");
    if !enabled {
        let config = loader.load().map_err(|error| error.to_string())?;
        if config.mcp().get(name).is_none() && config.mcp().disabled().get(name).is_none() {
            return Err(format!("no MCP server named `{name}` is configured"));
        }
    }
    let mut object = read_object(&settings_path)?;
    let mut names = disabled_names(&object);
    let present = names.iter().any(|existing| existing == name);
    let changed = if enabled {
        names.retain(|existing| existing != name);
        present
    } else {
        if !present {
            names.push(name.to_string());
        }
        !present
    };
    if changed {
        if names.is_empty() {
            object.remove("disabledMcpServers");
        } else {
            object.insert(
                "disabledMcpServers".to_string(),
                Value::Array(names.into_iter().map(Value::String).collect()),
            );
        }
        write_object(&settings_path, &object)?;
    }
    let still_disabled_by = if enabled {
        loader
            .discover()
            .into_iter()
            .map(|entry| entry.path)
            .filter(|path| *path != settings_path)
            .filter(|path| {
                read_object(path)
                    .map(|object| disabled_names(&object).iter().any(|other| other == name))
                    .unwrap_or(false)
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(McpToggleOutcome {
        settings_path,
        changed,
        still_disabled_by,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::set_mcp_server_enabled;
    use crate::config::{ConfigLoader, ProjectTrust};

    fn temp_root() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("neuron-mcp-toggle-{nanos}-{}", std::process::id()))
    }

    #[test]
    fn disabling_and_enabling_round_trips_through_user_settings() {
        let root = temp_root();
        let cwd = root.join("project");
        let home = root.join("home").join(".neuron");
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("settings.json"),
            r#"{"defaultMcpServers":false,"model":"keep-me","mcpServers":{"docs":{"command":"docs-mcp"},"remote":{"type":"http","url":"https://mcp.example.com/mcp"}}}"#,
        )
        .unwrap();
        let loader = ConfigLoader::new(&cwd, &home);

        let outcome = set_mcp_server_enabled(&loader, "remote", false).unwrap();
        assert!(outcome.changed);
        let config = loader.load().unwrap();
        assert!(config.mcp().get("remote").is_none());
        assert!(config.mcp().disabled().contains_key("remote"));
        assert!(config.mcp().get("docs").is_some());
        let written = fs::read_to_string(home.join("settings.json")).unwrap();
        assert!(written.contains("keep-me"), "other settings survive");

        assert!(
            !set_mcp_server_enabled(&loader, "remote", false)
                .unwrap()
                .changed
        );
        assert!(set_mcp_server_enabled(&loader, "missing", false).is_err());

        let outcome = set_mcp_server_enabled(&loader, "remote", true).unwrap();
        assert!(outcome.changed);
        assert!(outcome.still_disabled_by.is_empty());
        let config = loader.load().unwrap();
        assert!(config.mcp().get("remote").is_some());
        assert!(config.mcp().disabled().is_empty());
        let written = fs::read_to_string(home.join("settings.json")).unwrap();
        assert!(!written.contains("disabledMcpServers"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disabled_lists_add_up_across_files_and_enabling_reports_the_other_file() {
        let root = temp_root();
        let cwd = root.join("project");
        let home = root.join("home").join(".neuron");
        fs::create_dir_all(cwd.join(".neuron")).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(
            home.join("settings.json"),
            r#"{"defaultMcpServers":false,"mcpServers":{"a":{"command":"a"},"b":{"command":"b"}},"disabledMcpServers":["a"]}"#,
        )
        .unwrap();
        fs::write(
            cwd.join(".neuron").join("settings.local.json"),
            r#"{"disabledMcpServers":["b"]}"#,
        )
        .unwrap();
        let loader = ConfigLoader::new(&cwd, &home).with_project_trust(ProjectTrust::Trusted);
        let config = loader.load().unwrap();
        assert!(config.mcp().get("a").is_none());
        assert!(config.mcp().get("b").is_none());
        let outcome = set_mcp_server_enabled(&loader, "b", true).unwrap();
        assert!(!outcome.changed);
        assert_eq!(outcome.still_disabled_by.len(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
