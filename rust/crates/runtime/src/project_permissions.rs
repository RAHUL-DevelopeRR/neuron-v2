//! "Always allow in this project": `permissions.allow` rules saved to the project's
//! `.neuron/settings.local.json`, the per-user, per-project settings file that is
//! meant to stay out of version control. Like every workspace settings file, its
//! allow rules apply only once the workspace is trusted.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::mcp_toggle::{read_object, write_object};

/// The project settings file "always allow in this project" writes to.
#[must_use]
pub fn project_local_settings_path(workspace: &Path) -> PathBuf {
    workspace.join(".neuron").join("settings.local.json")
}

/// Adds `rules` to `permissions.allow` in `<workspace>/.neuron/settings.local.json`,
/// keeping every other setting and rule, and returns the file's path. Rules already
/// there are not added twice.
pub fn add_project_allow_rules(workspace: &Path, rules: &[String]) -> Result<PathBuf, String> {
    let path = project_local_settings_path(workspace);
    let mut object = read_object(&path)?;
    let permissions = object
        .entry("permissions".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(permissions) = permissions else {
        return Err(format!(
            "`permissions` in {} is not a JSON object",
            path.display()
        ));
    };
    let allow = permissions
        .entry("allow".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let Value::Array(allow) = allow else {
        return Err(format!(
            "`permissions.allow` in {} is not a list",
            path.display()
        ));
    };
    let mut changed = false;
    for rule in rules {
        if !allow.iter().any(|existing| existing.as_str() == Some(rule)) {
            allow.push(Value::String(rule.clone()));
            changed = true;
        }
    }
    if changed {
        write_object(&path, &object)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{add_project_allow_rules, project_local_settings_path};

    fn temp_workspace() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("neuron-project-rules-{nanos}"));
        fs::create_dir_all(&dir).expect("create workspace");
        dir
    }

    #[test]
    fn rules_are_added_once_and_other_settings_are_kept() {
        let workspace = temp_workspace();
        let path = project_local_settings_path(&workspace);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"model":"auto","permissions":{"allow":["bash(cargo:*)"],"deny":["bash(rm:*)"]}}"#,
        )
        .unwrap();

        let rules = vec!["bash(npm:*)".to_string(), "bash(cargo:*)".to_string()];
        assert_eq!(add_project_allow_rules(&workspace, &rules).unwrap(), path);
        add_project_allow_rules(&workspace, &rules).unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["model"], "auto");
        assert_eq!(
            saved["permissions"]["allow"],
            serde_json::json!(["bash(cargo:*)", "bash(npm:*)"])
        );
        assert_eq!(
            saved["permissions"]["deny"],
            serde_json::json!(["bash(rm:*)"])
        );
        fs::remove_dir_all(workspace).ok();
    }

    #[test]
    fn a_missing_file_is_created_and_a_malformed_one_is_left_alone() {
        let workspace = temp_workspace();
        let path = add_project_allow_rules(&workspace, &["bash(make:*)".to_string()]).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["permissions"]["allow"],
            serde_json::json!(["bash(make:*)"])
        );

        fs::write(&path, r#"{"permissions":{"allow":"bash(make:*)"}}"#).unwrap();
        let error = add_project_allow_rules(&workspace, &["bash(go:*)".to_string()]).unwrap_err();
        assert!(error.contains("is not a list"), "{error}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"permissions":{"allow":"bash(make:*)"}}"#
        );
        fs::remove_dir_all(workspace).ok();
    }
}
