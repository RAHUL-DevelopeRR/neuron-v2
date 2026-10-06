use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::default_skill_files::FILES;
use crate::json::JsonValue;

pub const DEFAULT_SKILL_NAMES: &[&str] = &[
    "ponytail",
    "antislop",
    "antislop-ui",
    "antislop-copywriting",
    "antislop-code",
    "antislop-human",
    "antislop-layoutmobile",
    "gstack",
    "gstack-investigate",
    "gstack-review",
    "gstack-qa",
    "gstack-qa-only",
    "gstack-ship",
    "agent-reach",
];

/// Embedded instructions remain available in an installed binary, outside a checkout.
pub fn bundled_skill_root(config_home: &Path) -> std::io::Result<PathBuf> {
    let mut digest = Sha256::new();
    for (path, contents) in FILES {
        digest.update(path.as_bytes());
        digest.update(contents.as_bytes());
    }
    let root = config_home
        .join("bundled-skills")
        .join(format!("{:x}", digest.finalize()));
    for (relative, contents) in FILES {
        let target = root.join(relative);
        if fs::read_to_string(&target).ok().as_deref() != Some(contents) {
            fs::create_dir_all(target.parent().expect("bundled paths have parents"))?;
            fs::write(target, contents)?;
        }
    }
    Ok(root)
}

#[must_use]
pub fn default_skills_enabled(config: &RuntimeConfig) -> bool {
    !matches!(
        config
            .as_json()
            .as_object()
            .and_then(|entries| entries.get("defaultSkills")),
        Some(JsonValue::Bool(false))
    )
}

pub fn default_skill_sections(
    config: &RuntimeConfig,
    config_home: &Path,
) -> std::io::Result<Vec<String>> {
    if !default_skills_enabled(config) {
        return Ok(Vec::new());
    }
    let root = bundled_skill_root(config_home)?;
    let instruction = |path: &str| {
        FILES
            .iter()
            .find(|(name, _)| *name == path)
            .expect("compiled skill exists")
            .1
            .to_string()
    };
    let mut sections = vec![
        "# Neuron default skills\nPonytail, AntiSlop and the gstack router are active on every conversation turn. Apply the appropriate rules to the task. Specific user instructions take precedence. These skills do not grant permission to publish, contact people, transmit repository data or operate computers. Do not run onboarding or telemetry. Report tools as available only after actual discovery. Load an applicable bundled gstack or AntiSlop companion through the Skill tool before using its workflow.".to_string(),
        instruction("ponytail/SKILL.md"),
        instruction("antislop/SKILL.md"),
        instruction("gstack/SKILL.md"),
        instruction("agent-reach/SKILL.md"),
        format!("# Gstack routing (Neuron adapter)\nUse gstack-investigate for debugging; gstack-review for code review; gstack-qa for authorized repair and verification; gstack-qa-only for read-only QA; gstack-ship for authorized release work. Their actual upstream workflows are bundled. Preserve evidence, verification gates and permission boundaries. Use Neuron's available tools in place of host-specific tools. Skip upstream onboarding, telemetry, external-helper scripts and Conductor/GBrain bookkeeping when those dependencies are absent. Missing browser automation or helper tools must be reported and never counted as a passed check.\n\nBundle root: {}\nUpstream gstack references: {}\n\nAvailable bundled skills: {}. Use the Skill tool to load their full instructions. User/project skill files can override these names.", root.display(), root.join("gstack").display(), DEFAULT_SKILL_NAMES.join(", ")),
    ];
    sections.push("Agent Reach instructions are active. Agent Reach is a CLI toolkit; verify `agent-reach doctor --json` before using an installed backend. Its skill does not make the CLI available by itself.".to_string());
    for name in ["codebase-memory", "computer-use", "playwright"] {
        let status = if config.mcp().get(name).is_some() {
            "configured; verify MCP discovery/initialization before use"
        } else {
            "pending; no MCP server command or URL has been configured"
        };
        sections.push(format!("Integration preset {name}: {status}. CodebaseMap uses DeusData/codebase-memory-mcp. Prefer its discovered mapping and graph tools for code exploration. Playwright controls a browser, not the desktop. Full computer use requires a separately installed local computer-use server and a model that can interpret its observations."));
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLoader;

    #[test]
    fn defaults_work_outside_checkout_and_can_be_disabled() {
        let root = std::env::temp_dir().join(format!("neuron-defaults-{}", std::process::id()));
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        let loader = ConfigLoader::new(&root, &config_home);
        let sections = default_skill_sections(&loader.load().unwrap(), &config_home)
            .unwrap()
            .join("\n");
        assert!(sections.contains("# Ponytail"));
        assert!(sections.contains("R-38"));
        assert!(sections.contains("gstack-investigate"));
        assert!(sections.contains("Agent Reach instructions are active"));
        assert!(sections.contains("Integration preset codebase-memory: configured"));
        let bundle = bundled_skill_root(&config_home).unwrap();
        assert!(
            fs::read_to_string(bundle.join("gstack/review/checklist.md"))
                .unwrap()
                .contains("SQL")
        );
        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultSkills":false}"#,
        )
        .unwrap();
        assert_eq!(
            default_skill_sections(&loader.load().unwrap(), &config_home).unwrap(),
            Vec::<String>::new()
        );
        fs::remove_dir_all(root).unwrap();
    }
}
