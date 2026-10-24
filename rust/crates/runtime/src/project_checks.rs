//! Project checks for the completion check (see `ConversationRuntime`): the fast
//! commands that tell whether a turn's edits still build, chosen for the files the
//! turn changed.
//!
//! A project can list its own in `.neuron/checks.json`
//! (`{"checks": [{"name": "types", "command": "npm run typecheck", "timeoutMs": 120000}]}`);
//! otherwise they are detected: `cargo check` for Rust, the `typecheck` (or `lint`)
//! script or `tsc --noEmit` for JavaScript and TypeScript, `go vet` for Go and
//! `python -m py_compile` for Python. Detection only proposes commands: whether one
//! runs without asking is the permission policy's decision, made by the runtime.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Checks run (or proposed) per completion check.
pub const MAX_CHECKS: usize = 3;
const DEFAULT_TIMEOUT_MS: u64 = 180_000;
const RUST_TIMEOUT_MS: u64 = 300_000;
const MAX_PY_FILES: usize = 20;

/// One check command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectCheck {
    pub name: String,
    pub command: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Deserialize)]
struct ChecksFile {
    #[serde(default)]
    checks: Vec<ChecksFileEntry>,
}

#[derive(Debug, Deserialize)]
struct ChecksFileEntry {
    name: Option<String>,
    command: String,
    #[serde(rename = "timeoutMs")]
    timeout_ms: Option<u64>,
}

/// The checks for a turn that changed `edited` (paths as the tools received them,
/// absolute or relative to `root`).
#[must_use]
pub fn detect_checks(root: &Path, edited: &[String]) -> Vec<ProjectCheck> {
    if let Some(configured) = configured_checks(root) {
        return configured;
    }
    let files = edited
        .iter()
        .map(|path| {
            let path = Path::new(path);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                root.join(path)
            }
        })
        .collect::<Vec<_>>();
    let mut checks = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |check: ProjectCheck| {
        if seen.insert(check.command.clone()) && checks.len() < MAX_CHECKS {
            checks.push(check);
        }
    };
    let mut python = Vec::new();
    for file in &files {
        match extension(file).as_deref() {
            Some("rs") => {
                if let Some(manifest) = nearest(file, root, &["Cargo.toml"]) {
                    push(ProjectCheck {
                        name: "cargo check".to_string(),
                        command: format!(
                            "cargo check --quiet --message-format short --manifest-path {}",
                            quote(&manifest)
                        ),
                        timeout_ms: RUST_TIMEOUT_MS,
                    });
                }
            }
            Some(
                "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" | "vue" | "svelte",
            ) => {
                if let Some(check) = node_check(file, root) {
                    push(check);
                }
            }
            Some("go") => {
                if let Some(module) = nearest(file, root, &["go.mod"]) {
                    let dir = module.parent().unwrap_or(root);
                    push(ProjectCheck {
                        name: "go vet".to_string(),
                        command: format!("cd {} && go vet ./...", quote(dir)),
                        timeout_ms: DEFAULT_TIMEOUT_MS,
                    });
                }
            }
            Some("py") => python.push(file.clone()),
            _ => {}
        }
    }
    if !python.is_empty() {
        let files = python
            .iter()
            .take(MAX_PY_FILES)
            .map(|file| quote(file))
            .collect::<Vec<_>>()
            .join(" ");
        push(ProjectCheck {
            name: "python syntax".to_string(),
            command: format!("python -m py_compile {files}"),
            timeout_ms: 60_000,
        });
    }
    checks
}

fn configured_checks(root: &Path) -> Option<Vec<ProjectCheck>> {
    let text = fs::read_to_string(root.join(".neuron").join("checks.json")).ok()?;
    let parsed = serde_json::from_str::<ChecksFile>(&text).ok()?;
    Some(
        parsed
            .checks
            .into_iter()
            .filter(|entry| !entry.command.trim().is_empty())
            .take(MAX_CHECKS)
            .map(|entry| ProjectCheck {
                name: entry.name.unwrap_or_else(|| entry.command.clone()),
                command: entry.command,
                timeout_ms: entry
                    .timeout_ms
                    .unwrap_or(DEFAULT_TIMEOUT_MS)
                    .clamp(1_000, 600_000),
            })
            .collect(),
    )
}

fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

/// The nearest of `names` in the directories from `file`'s up to `root`.
fn nearest(file: &Path, root: &Path, names: &[&str]) -> Option<PathBuf> {
    let mut dir = file.parent();
    while let Some(current) = dir {
        for name in names {
            let candidate = current.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if current == root || !current.starts_with(root) {
            break;
        }
        dir = current.parent();
    }
    None
}

fn node_check(file: &Path, root: &Path) -> Option<ProjectCheck> {
    let manifest = nearest(file, root, &["package.json"])?;
    let dir = manifest.parent().unwrap_or(root);
    let scripts = fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("scripts").cloned())
        .unwrap_or_default();
    let runner = if dir.join("bun.lock").is_file() || dir.join("bun.lockb").is_file() {
        "bun run"
    } else if dir.join("pnpm-lock.yaml").is_file() {
        "pnpm run"
    } else if dir.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm run"
    };
    for script in ["typecheck", "type-check", "check-types", "tsc", "lint"] {
        if scripts
            .get(script)
            .is_some_and(serde_json::Value::is_string)
        {
            return Some(ProjectCheck {
                name: format!("{runner} {script}"),
                command: format!("cd {} && {runner} {script}", quote(dir)),
                timeout_ms: DEFAULT_TIMEOUT_MS,
            });
        }
    }
    let has_tsc = dir.join("node_modules").join(".bin").join("tsc").exists()
        || dir
            .join("node_modules")
            .join(".bin")
            .join("tsc.cmd")
            .exists();
    if dir.join("tsconfig.json").is_file() && has_tsc {
        return Some(ProjectCheck {
            name: "tsc --noEmit".to_string(),
            command: format!("cd {} && npx --no-install tsc --noEmit", quote(dir)),
            timeout_ms: DEFAULT_TIMEOUT_MS,
        });
    }
    None
}

/// `path` quoted for a POSIX shell when it needs it.
fn quote(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "/._-:".contains(ch))
    {
        text
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::detect_checks;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neuron-checks-{name}-{unique}"));
        fs::create_dir_all(&root).expect("root");
        root
    }

    #[test]
    fn checks_follow_the_languages_that_changed() {
        let root = temp_root("detect");
        fs::create_dir_all(root.join("rust/src")).expect("rust");
        fs::write(root.join("rust/Cargo.toml"), "[package]\nname = \"x\"\n").expect("cargo");
        fs::create_dir_all(root.join("web/src")).expect("web");
        fs::write(
            root.join("web/package.json"),
            r#"{"scripts":{"lint":"eslint .","typecheck":"tsc --noEmit"}}"#,
        )
        .expect("pkg");
        fs::write(root.join("web/bun.lock"), "").expect("lock");

        let checks = detect_checks(
            &root,
            &[
                "rust/src/lib.rs".to_string(),
                "web/src/app.ts".to_string(),
                "web/src/other.tsx".to_string(),
                "tool.py".to_string(),
                "README.md".to_string(),
            ],
        );
        let commands = checks
            .iter()
            .map(|check| check.command.as_str())
            .collect::<Vec<_>>();
        assert_eq!(checks.len(), 3, "{commands:?}");
        assert!(
            commands[0].starts_with("cargo check --quiet --message-format short --manifest-path "),
            "{commands:?}"
        );
        assert!(commands[0].ends_with("rust/Cargo.toml"), "{commands:?}");
        assert!(
            commands[1].ends_with("&& bun run typecheck"),
            "typecheck preferred over lint: {commands:?}"
        );
        assert!(
            commands[2].starts_with("python -m py_compile "),
            "{commands:?}"
        );
        assert!(
            detect_checks(&root, &["README.md".to_string()]).is_empty(),
            "docs need no check"
        );
    }

    #[test]
    fn a_project_checks_file_replaces_detection() {
        let root = temp_root("configured");
        fs::create_dir_all(root.join(".neuron")).expect("dir");
        fs::write(
            root.join(".neuron/checks.json"),
            r#"{"checks":[{"name":"unit","command":"make test","timeoutMs":5}, {"command":"  "}]}"#,
        )
        .expect("checks");
        let checks = detect_checks(&root, &["src/main.rs".to_string()]);
        assert_eq!(checks.len(), 1);
        assert_eq!(
            (
                checks[0].name.as_str(),
                checks[0].command.as_str(),
                checks[0].timeout_ms
            ),
            ("unit", "make test", 1_000)
        );
    }
}
