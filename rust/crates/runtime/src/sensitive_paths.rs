//! Files whose contents are credentials or settings that can hold them:
//! `.env` files, Neuron/Claw/Claude settings (MCP tokens, auth headers, `env`
//! blocks, OAuth config) and stored credentials. Whatever the model reads is
//! sent to the upstream provider, so reading one of these needs the user's
//! approval and directory searches skip them.

use std::path::{Path, PathBuf};

/// Directories holding Neuron, Claw or Claude settings and credentials.
const CONFIG_DIRS: &[&str] = &[".neuron", ".neuroncli", ".claw", ".claude"];

/// Settings files kept at a workspace root.
const CONFIG_FILES: &[&str] = &[".neuron.json", ".claw.json", ".claude.json"];

/// Files inside [`CONFIG_DIRS`] that hold settings or credentials (globs are
/// tested against these names).
const CONFIG_DIR_FILES: &[&str] = &["settings.json", "settings.local.json", "credentials.json"];

/// `.env` variants that by convention hold placeholders only.
const ENV_TEMPLATE_SUFFIXES: &[&str] = &[".example", ".sample", ".template", ".dist"];

/// Names a glob is tested against to decide whether it could match an
/// environment file.
const ENV_FILE_SAMPLES: &[&str] = &[".env", ".env.local", ".envrc"];

/// Why reading `path` would expose credentials, or `None`. A glob component
/// counts when it could match a sensitive name (`.e*`, `.neuron/*`); as in
/// POSIX shells, only a pattern starting with `.` matches a dotfile.
#[must_use]
pub fn sensitive_path_reason(path: &str) -> Option<String> {
    let components: Vec<String> = path
        .trim_matches(['"', '\''])
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != ".")
        .map(str::to_ascii_lowercase)
        .collect();
    let (file_name, parents) = components.split_last()?;
    if is_env_file(file_name) {
        return Some(format!(
            "`{path}` is an environment file, which can hold credentials"
        ));
    }
    let in_config_dir = parents.iter().any(|parent| {
        CONFIG_DIRS
            .iter()
            .any(|name| component_matches(parent, name))
    });
    let is_config_file = CONFIG_FILES
        .iter()
        .any(|name| component_matches(file_name, name))
        || (in_config_dir
            && CONFIG_DIR_FILES
                .iter()
                .any(|name| component_matches(file_name, name)));
    is_config_file.then(|| {
        format!("`{path}` holds Neuron settings or credentials, which can include tokens and keys")
    })
}

/// Like [`sensitive_path_reason`], but also checks where `path` resolves from
/// each of `bases`, so a symlink (or Windows short name) that leads to a
/// sensitive file counts too.
#[must_use]
pub fn sensitive_path_reason_resolved(path: &str, bases: &[PathBuf]) -> Option<String> {
    if let Some(reason) = sensitive_path_reason(path) {
        return Some(reason);
    }
    if path.is_empty() || has_glob(path) {
        return None;
    }
    let candidate = Path::new(path);
    let resolved: Vec<PathBuf> = if candidate.is_absolute() {
        vec![candidate.to_path_buf()]
    } else {
        bases.iter().map(|base| base.join(candidate)).collect()
    };
    resolved
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .find_map(|canonical| {
            sensitive_path_reason(&canonical.to_string_lossy()).map(|_| {
                format!(
                    "`{path}` resolves to `{}`, which can hold credentials",
                    canonical.display()
                )
            })
        })
}

fn is_env_file(name: &str) -> bool {
    if has_glob(name) {
        return ENV_FILE_SAMPLES
            .iter()
            .any(|sample| glob_matches(name, sample));
    }
    name.starts_with(".env")
        && !ENV_TEMPLATE_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn component_matches(component: &str, name: &str) -> bool {
    if has_glob(component) {
        glob_matches(component, name)
    } else {
        component == name
    }
}

fn has_glob(text: &str) -> bool {
    text.contains(['*', '?', '[', '{'])
}

/// Glob match with `*`, `?` and `[...]` (any character in a class counts) and
/// `{a,b}` treated as `*`; over-approximates so a pattern that might match is
/// reported.
fn glob_matches(pattern: &str, name: &str) -> bool {
    if name.starts_with('.') && !pattern.starts_with('.') {
        return false;
    }
    let mut simplified = Vec::new();
    let mut in_braces = false;
    for ch in pattern.chars() {
        match ch {
            '{' if !in_braces => {
                in_braces = true;
                simplified.push('*');
            }
            '}' if in_braces => in_braces = false,
            _ if in_braces => {}
            _ => simplified.push(ch),
        }
    }
    let name: Vec<char> = name.chars().collect();
    glob_matches_from(&simplified, &name)
}

fn glob_matches_from(pattern: &[char], name: &[char]) -> bool {
    match pattern.first() {
        None => name.is_empty(),
        Some('*') => (0..=name.len()).any(|skip| glob_matches_from(&pattern[1..], &name[skip..])),
        Some('?') => !name.is_empty() && glob_matches_from(&pattern[1..], &name[1..]),
        Some('[') => match pattern.iter().position(|ch| *ch == ']') {
            Some(end) if end > 1 => {
                !name.is_empty() && glob_matches_from(&pattern[end + 1..], &name[1..])
            }
            _ => name.first() == Some(&'[') && glob_matches_from(&pattern[1..], &name[1..]),
        },
        Some(ch) => name.first() == Some(ch) && glob_matches_from(&pattern[1..], &name[1..]),
    }
}

#[cfg(test)]
mod tests {
    use super::{sensitive_path_reason, sensitive_path_reason_resolved};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn env_files_and_neuron_settings_are_sensitive() {
        for path in [
            ".env",
            ".env.local",
            ".ENV.production",
            "./services/api/.env",
            "services\\api\\.env.development",
            ".envrc",
            ".neuron.json",
            ".claw.json",
            ".neuron/settings.json",
            ".neuron/settings.local.json",
            ".claw/settings.local.json",
            ".claude/settings.json",
            "/home/me/.neuroncli/credentials.json",
            "C:\\Users\\me\\.neuron\\credentials.json",
            "\"./.env\"",
        ] {
            assert!(sensitive_path_reason(path).is_some(), "{path}");
        }
    }

    #[test]
    fn globs_that_could_match_sensitive_files_are_sensitive() {
        for path in [
            ".env*",
            ".e*",
            ".*",
            ".neuron/*",
            ".n*/settings*.json",
            ".{env,x}",
        ] {
            assert!(sensitive_path_reason(path).is_some(), "{path}");
        }
    }

    #[test]
    fn ordinary_files_templates_and_plain_globs_are_not_sensitive() {
        for path in [
            "src/main.rs",
            ".env.example",
            ".env.sample",
            "config/.env.template",
            "settings.json",
            ".neuron/NEURON.md",
            ".claude/commands/review.md",
            ".neuron",
            "*",
            "*.rs",
            "src/*",
            "environment.ts",
        ] {
            assert!(sensitive_path_reason(path).is_none(), "{path}");
        }
    }

    #[test]
    fn a_link_to_an_env_file_is_sensitive() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("runtime-sensitive-paths-{nanos}"));
        fs::create_dir_all(&root).expect("root");
        fs::write(root.join(".env"), "OPENAI_API_KEY=sk-test").expect("write env");
        let linked = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(root.join(".env"), root.join("notes.txt")).is_ok()
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(root.join(".env"), root.join("notes.txt"))
                    .is_ok()
            }
        };

        if linked {
            assert!(
                sensitive_path_reason_resolved("notes.txt", std::slice::from_ref(&root)).is_some()
            );
        }
        assert!(sensitive_path_reason_resolved(".env", std::slice::from_ref(&root)).is_some());
        fs::write(root.join("plain.txt"), "hello").expect("write plain");
        assert!(sensitive_path_reason_resolved("plain.txt", std::slice::from_ref(&root)).is_none());

        fs::remove_dir_all(root).expect("cleanup");
    }
}
