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

/// What each bundled skill is for and when to load it. The system prompt
/// carries only this index; the Skill tool returns the full `SKILL.md`.
pub const DEFAULT_SKILL_INDEX: &[(&str, &str)] = &[
    ("ponytail", "the full simplest-solution ladder and lite/full/ultra levels; load for design choices or when asked for minimal, lazy or YAGNI work"),
    ("antislop", "the core filter against generic AI output (purpose test, craft standard, rule catalogue); load before building or reviewing UI, prose or code comments"),
    ("antislop-ui", "color, layout, components and motion; load for interface work"),
    ("antislop-copywriting", "headlines, tone, calls to action and AI-writing patterns; load when writing or editing prose"),
    ("antislop-code", "code comment hygiene; load before adding or cleaning up comments"),
    ("antislop-human", "contrast, keyboard, focus and states, with a contrast checker; load for accessibility work"),
    ("antislop-layoutmobile", "layouts that reflow from phone to desktop; load for responsive layout work"),
    ("gstack", "router to the gstack workflows below"),
    ("gstack-investigate", "systematic root-cause debugging; load when a failure is not explained by a first look at the code"),
    ("gstack-review", "pre-landing code review; load when asked to review a diff or PR"),
    ("gstack-qa", "find, fix and verify bugs; load for authorized repair with verification"),
    ("gstack-qa-only", "report bugs without changing code; load for read-only QA"),
    ("gstack-ship", "release workflow (tests, changelog, version, PR); load only for authorized release work"),
    ("agent-reach", "internet research across the web, GitHub, video and social platforms through the agent-reach CLI (run `agent-reach doctor --json` first; the skill does not install the CLI); load for online research"),
];

/// The behavioral core of the Ponytail and `AntiSlop` skills, sent every turn.
/// Kept short: the full rule sets load on demand through the Skill tool.
pub const ALWAYS_ON_RULES: &str = "Always on (short form; the Skill tool returns the full text):
- Ponytail: read the code a change touches first, then take the first rung that holds: not needed (say so), reuse what the codebase has, stdlib, native platform feature, installed dependency, one line, only then new code. Fix root causes where all callers route through. No unrequested abstractions, scaffolding, config or new dependencies. Never simplify away validation at trust boundaries, error handling that prevents data loss, security or accessibility. Non-trivial logic leaves one small runnable check. After code, at most three short lines: what was skipped and when to add it.
- AntiSlop: every visual, copy and comment choice needs a purpose you can name; drop generic AI defaults (template sections, decorative gradients or badges, stock phrases, comments that restate the code). Controls must work, text must stay readable and accessible, and the result needs its own character.";

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

/// The system prompt section for the bundled skills: loading rules, the
/// always-on summary and a one-line index. Skill bodies are not included.
pub fn default_skill_sections(
    config: &RuntimeConfig,
    config_home: &Path,
) -> std::io::Result<Vec<String>> {
    if !default_skills_enabled(config) {
        return Ok(Vec::new());
    }
    let root = bundled_skill_root(config_home)?;
    let index = DEFAULT_SKILL_INDEX
        .iter()
        .map(|(name, purpose)| format!("- {name}: {purpose}"))
        .collect::<Vec<_>>()
        .join("\n");
    let presets = [
        ("codebase-memory", "code graph"),
        ("playwright", "browser, not the desktop"),
        ("computer-use", "desktop control"),
    ]
    .iter()
    .map(|(name, role)| {
        let status = if config.mcp().get(name).is_some() {
            "configured"
        } else {
            "not configured"
        };
        format!("{name} ({role}): {status}")
    })
    .collect::<Vec<_>>()
    .join("; ");
    Ok(vec![format!(
        "# Neuron default skills\n\
Bundled skills load on demand: their full instructions are not in this prompt. When a task needs a skill's workflow, call the Skill tool with its name (for example `{{\"skill\": \"gstack-review\"}}`) and follow what it returns; small, clear tasks need none. Specific user instructions and project or user skills of the same name take precedence. Skills grant no permission to publish, contact people, transmit repository data or operate computers. Skip upstream onboarding, telemetry and absent helper scripts, use Neuron's tools in place of host-specific ones, and report a missing tool instead of counting its check as passed.\n\n\
{ALWAYS_ON_RULES}\n\n\
Skills:\n{index}\n\n\
Bundle root: {} (upstream sources and MIT licenses: SOURCES.json, licenses/).\n\
MCP presets: {presets}. Report a tool as available only after discovery; MCP tool schemas are deferred, so find them with ToolSearch before calling them.",
        root.display()
    )])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLoader;

    /// The skill section is sent with every request, so its size is a budget.
    const SKILL_SECTION_BUDGET_CHARS: usize = 4_500;

    #[test]
    fn defaults_work_outside_checkout_and_can_be_disabled() {
        let root = std::env::temp_dir().join(format!("neuron-defaults-{}", std::process::id()));
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        let loader = ConfigLoader::new(&root, &config_home);
        let sections = default_skill_sections(&loader.load().unwrap(), &config_home)
            .unwrap()
            .join("\n");
        assert!(sections.contains("# Neuron default skills"));
        assert!(sections.contains("call the Skill tool"));
        for name in DEFAULT_SKILL_NAMES {
            assert!(
                sections.contains(&format!("\n- {name}: ")),
                "{name} indexed"
            );
        }
        assert!(sections.contains("agent-reach doctor --json"));
        assert!(sections.contains("codebase-memory (code graph): configured"));
        assert!(sections.contains("computer-use (desktop control): not configured"));
        // Full bodies stay out of the prompt and on disk for the Skill tool.
        assert!(!sections.contains("# Ponytail\n"));
        assert!(!sections.contains("R-38"));
        let bundle = bundled_skill_root(&config_home).unwrap();
        assert!(sections.contains(&bundle.display().to_string()));
        assert!(fs::read_to_string(bundle.join("ponytail/SKILL.md"))
            .unwrap()
            .contains("# Ponytail"));
        assert!(fs::read_to_string(bundle.join("antislop/SKILL.md"))
            .unwrap()
            .contains("R-38"));
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

    #[test]
    fn skill_section_and_always_on_rules_stay_within_budget() {
        let root = std::env::temp_dir().join(format!("neuron-skill-budget-{}", std::process::id()));
        let config_home = root.join("config");
        fs::create_dir_all(&config_home).unwrap();
        let loader = ConfigLoader::new(&root, &config_home);
        let section = default_skill_sections(&loader.load().unwrap(), &config_home)
            .unwrap()
            .join("\n");
        let bundle = bundled_skill_root(&config_home).unwrap();
        // The bundle path depends on the machine; measure without it.
        let measured = section.replace(&bundle.display().to_string(), "");
        assert!(
            measured.chars().count() <= SKILL_SECTION_BUDGET_CHARS,
            "default skill section grew to {} chars",
            measured.chars().count()
        );
        assert!(ALWAYS_ON_RULES.chars().count() < 1_500);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_bundled_skill_is_indexed_once_and_has_a_skill_file() {
        let indexed = DEFAULT_SKILL_INDEX
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>();
        assert_eq!(indexed, DEFAULT_SKILL_NAMES);
        for name in DEFAULT_SKILL_NAMES {
            let has_file = FILES.iter().any(|(path, contents)| {
                path.ends_with("SKILL.md")
                    && contents
                        .lines()
                        .any(|line| line.trim_end() == format!("name: {name}"))
            });
            assert!(has_file, "{name} has an embedded SKILL.md");
        }
        // Attribution ships with the bundle.
        for license in ["agent-reach", "antislop", "gstack", "ponytail"] {
            assert!(FILES
                .iter()
                .any(|(path, _)| *path == format!("licenses/{license}.txt")));
        }
        assert!(FILES.iter().any(|(path, _)| *path == "SOURCES.json"));
    }
}
