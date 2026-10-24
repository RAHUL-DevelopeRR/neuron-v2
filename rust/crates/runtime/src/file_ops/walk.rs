//! Workspace-aware directory walking for `glob_search` and `grep_search`.
//!
//! Skips version-control and build output directories (`.git`,
//! `node_modules`, `target`, `dist`, `build`) and paths ignored by
//! `.gitignore` files, so searches stay fast and relevant. The walk root
//! itself is never skipped: searching inside `node_modules/foo` explicitly
//! still works.

use std::fs;
use std::path::{Path, PathBuf};

use glob::{MatchOptions, Pattern};

/// Directory names skipped at any depth below the walk root.
pub const DEFAULT_SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", "build"];

/// Upper bound on visited entries so a search of a huge tree terminates.
const MAX_WALK_ENTRIES: usize = 250_000;

#[derive(Debug, Clone)]
struct IgnoreRule {
    pattern: Pattern,
    negated: bool,
    dir_only: bool,
    /// Pattern contains a `/` before its end: match the path relative to the
    /// `.gitignore` directory instead of the file name at any depth.
    anchored: bool,
}

#[derive(Debug, Clone)]
struct IgnoreFile {
    base: PathBuf,
    rules: Vec<IgnoreRule>,
}

impl IgnoreFile {
    fn load(dir: &Path) -> Option<Self> {
        let contents = fs::read_to_string(dir.join(".gitignore")).ok()?;
        let rules = contents
            .lines()
            .filter_map(parse_ignore_line)
            .collect::<Vec<_>>();
        (!rules.is_empty()).then(|| Self {
            base: dir.to_path_buf(),
            rules,
        })
    }

    /// `Some(true)` ignored, `Some(false)` re-included, `None` no opinion.
    fn verdict(&self, path: &Path, is_dir: bool) -> Option<bool> {
        let relative = path.strip_prefix(&self.base).ok()?;
        let relative = to_slash_path(relative);
        let name = path.file_name()?.to_string_lossy().into_owned();
        let options = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        let mut verdict = None;
        for rule in &self.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            let matched = if rule.anchored {
                rule.pattern.matches_with(&relative, options)
            } else {
                rule.pattern.matches_with(&name, options)
                    || rule.pattern.matches_with(&relative, options)
            };
            if matched {
                verdict = Some(!rule.negated);
            }
        }
        verdict
    }
}

fn parse_ignore_line(line: &str) -> Option<IgnoreRule> {
    let line = line.trim_end();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (negated, line) = match line.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, line.strip_prefix('\\').unwrap_or(line)),
    };
    let (dir_only, line) = match line.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, line),
    };
    let anchored = line.contains('/');
    let line = line.trim_start_matches('/');
    if line.is_empty() {
        return None;
    }
    let pattern = Pattern::new(line).ok()?;
    Some(IgnoreRule {
        pattern,
        negated,
        dir_only,
        anchored,
    })
}

fn to_slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `.gitignore` files from the repository root (nearest ancestor holding
/// `.git`) down to `root`, so rules written above the walk root still apply.
fn ancestor_ignore_files(root: &Path) -> Vec<IgnoreFile> {
    if root.join(".git").exists() {
        return Vec::new();
    }
    let mut chain = Vec::new();
    let mut cursor = root.parent();
    while let Some(dir) = cursor {
        chain.push(dir.to_path_buf());
        if dir.join(".git").exists() {
            chain.reverse();
            return chain
                .iter()
                .filter_map(|dir| IgnoreFile::load(dir))
                .collect();
        }
        cursor = dir.parent();
    }
    // Not inside a repository: only rules at or below the walk root apply.
    Vec::new()
}

fn is_ignored(ignore_files: &[IgnoreFile], path: &Path, is_dir: bool) -> bool {
    let mut ignored = false;
    for file in ignore_files {
        if let Some(verdict) = file.verdict(path, is_dir) {
            ignored = verdict;
        }
    }
    ignored
}

/// Collects regular files below `root`, skipping [`DEFAULT_SKIPPED_DIRS`] and
/// `.gitignore`d paths. Returns the files and whether the walk was cut short.
pub fn collect_workspace_files(root: &Path) -> (Vec<PathBuf>, bool) {
    if root.is_file() {
        return (vec![root.to_path_buf()], false);
    }
    let mut ignore_files = ancestor_ignore_files(root);
    let mut files = Vec::new();
    let mut visited = 0;
    let truncated = walk_dir(root, &mut ignore_files, &mut files, &mut visited);
    (files, truncated)
}

fn walk_dir(
    dir: &Path,
    ignore_files: &mut Vec<IgnoreFile>,
    files: &mut Vec<PathBuf>,
    visited: &mut usize,
) -> bool {
    let pushed = IgnoreFile::load(dir)
        .map(|file| ignore_files.push(file))
        .is_some();
    let mut truncated = false;
    let Ok(entries) = fs::read_dir(dir) else {
        if pushed {
            ignore_files.pop();
        }
        return false;
    };
    let mut entries = entries.flatten().collect::<Vec<_>>();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        *visited += 1;
        if *visited > MAX_WALK_ENTRIES {
            truncated = true;
            break;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // Follow symlinks only to decide file vs directory; never recurse
        // through a symlinked directory (it may leave the workspace or loop).
        let is_dir = file_type.is_dir();
        let is_file = file_type.is_file() || (file_type.is_symlink() && path.is_file());
        if is_dir {
            let name = entry.file_name();
            if DEFAULT_SKIPPED_DIRS
                .iter()
                .any(|skipped| name.to_string_lossy() == *skipped)
                || is_ignored(ignore_files, &path, true)
            {
                continue;
            }
            if walk_dir(&path, ignore_files, files, visited) {
                truncated = true;
                break;
            }
        } else if is_file && !is_ignored(ignore_files, &path, false) {
            files.push(path);
        }
    }
    if pushed {
        ignore_files.pop();
    }
    truncated
}

/// Splits a glob pattern into the literal directory to walk and the pattern
/// to match against paths relative to it (`/a/b/**/*.rs` -> (`/a/b`, `**/*.rs`)).
#[must_use]
pub fn split_glob_root(pattern: &Path) -> (PathBuf, Option<String>) {
    let mut root = PathBuf::new();
    let mut rest = Vec::new();
    for component in pattern.components() {
        let text = component.as_os_str().to_string_lossy();
        // `\\?\C:` verbatim prefixes contain `?` but are never glob syntax.
        let is_literal = !matches!(component, std::path::Component::Normal(_))
            || !text.contains(['*', '?', '[']);
        if rest.is_empty() && is_literal {
            root.push(component.as_os_str());
        } else {
            rest.push(text.into_owned());
        }
    }
    let rest = (!rest.is_empty()).then(|| rest.join("/"));
    (root, rest)
}

/// Matches `path` (relative to the walk root) against a glob pattern where
/// `*` stays within one path component and `**` spans directories.
#[must_use]
pub fn glob_matches(pattern: &Pattern, relative: &Path) -> bool {
    pattern.matches_with(
        &to_slash_path(relative),
        MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::{collect_workspace_files, split_glob_root};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("neuron-walk-{name}-{nanos}"))
    }

    fn names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        let mut names = files
            .iter()
            .map(|file| {
                file.strip_prefix(root)
                    .expect("under root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn skips_default_dirs_and_gitignored_paths() {
        let root = temp_dir("skip");
        for dir in [
            "src",
            ".git",
            "node_modules/pkg",
            "target/debug",
            "dist",
            "build",
            "logs",
            "src/generated",
        ] {
            fs::create_dir_all(root.join(dir)).expect("create dir");
        }
        for file in [
            "src/main.rs",
            ".git/config",
            "node_modules/pkg/index.js",
            "target/debug/app",
            "dist/bundle.js",
            "build/out.o",
            "logs/today.log",
            "src/generated/schema.rs",
            "src/keep.log",
            "notes.tmp",
        ] {
            fs::write(root.join(file), "x").expect("write file");
        }
        fs::write(
            root.join(".gitignore"),
            "# comment\nlogs/\n*.log\n!src/keep.log\n/notes.tmp\n",
        )
        .expect("write gitignore");
        fs::write(root.join("src").join(".gitignore"), "generated/\n").expect("nested");

        let (files, truncated) = collect_workspace_files(&root);
        assert!(!truncated);
        assert_eq!(
            names(&root, &files),
            vec![
                ".gitignore",
                "src/.gitignore",
                "src/keep.log",
                "src/main.rs"
            ]
        );

        let (inside_skipped, _) = collect_workspace_files(&root.join("node_modules"));
        assert_eq!(
            names(&root, &inside_skipped),
            vec!["node_modules/pkg/index.js"],
            "an explicitly requested skipped dir is still searchable"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn splits_glob_patterns_into_literal_root_and_rest() {
        let (root, rest) = split_glob_root(Path::new("/a/b/**/*.rs"));
        assert_eq!(root, PathBuf::from("/a/b"));
        assert_eq!(rest.as_deref(), Some("**/*.rs"));
        let (root, rest) = split_glob_root(Path::new("src/main.rs"));
        assert_eq!(root, PathBuf::from("src/main.rs"));
        assert_eq!(rest, None);
    }
}
