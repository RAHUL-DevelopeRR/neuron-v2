//! The workspace as the file tree and the side panel read it: read-only, and
//! never outside the workspace (symlinks that lead out are refused too).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

const MAX_LIST_ENTRIES: usize = 2_000;
const MAX_READ_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    /// Relative to the workspace, with `/`.
    pub path: String,
    pub dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DirListing {
    pub path: String,
    pub entries: Vec<DirEntry>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileContent {
    pub path: String,
    pub content: String,
    pub truncated: bool,
    pub binary: bool,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = fs::canonicalize(root.as_ref())
            .map_err(|error| format!("{}: {error}", root.as_ref().display()))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The root as people read it: without Windows' `\\?\` prefix.
    pub fn display_root(&self) -> String {
        let shown = self.root.display().to_string();
        shown
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .or_else(|| shown.strip_prefix(r"\\?\").map(ToString::to_string))
            .unwrap_or(shown)
    }

    /// The folder's own name.
    pub fn name(&self) -> String {
        self.root.file_name().map_or_else(
            || self.display_root(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Files whose path contains `query` (case-insensitive), matches in the file name
    /// first, then shorter paths. Build and dependency folders are skipped.
    pub fn search(&self, query: &str, limit: usize) -> Vec<String> {
        const SKIPPED: &[&str] = &[
            ".git",
            "node_modules",
            "target",
            "dist",
            "build",
            ".neuron",
            ".next",
            ".venv",
            "__pycache__",
        ];
        const MAX_SCANNED: usize = 20_000;
        let needle = query.trim().to_lowercase().replace('\\', "/");
        let mut found: Vec<(usize, String)> = Vec::new();
        let mut scanned = 0;
        let mut stack = vec![(self.root.clone(), String::new())];
        while let Some((dir, relative)) = stack.pop() {
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if scanned >= MAX_SCANNED {
                    break;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                };
                // Never follow a symlink out of the workspace.
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir() {
                    if !SKIPPED.contains(&name.as_str()) {
                        stack.push((entry.path(), path));
                    }
                    continue;
                }
                scanned += 1;
                let lower = path.to_lowercase();
                let Some(at) = (if needle.is_empty() {
                    Some(0)
                } else {
                    lower.find(&needle)
                }) else {
                    continue;
                };
                let in_name = name.to_lowercase().contains(&needle);
                found.push((
                    (if in_name { 0 } else { 1000 }) + at * 100 + path.len(),
                    path,
                ));
            }
        }
        found.sort();
        found
            .into_iter()
            .take(limit)
            .map(|(_, path)| path)
            .collect()
    }

    /// `relative` resolved inside the workspace, and its path relative to it.
    fn inside(&self, relative: &str) -> Result<(PathBuf, String), String> {
        let absolute = fs::canonicalize(self.root.join(relative))
            .map_err(|_| format!("No such file or folder: {relative}"))?;
        let inner = absolute
            .strip_prefix(&self.root)
            .map_err(|_| "That path is outside the workspace.".to_string())?;
        let shown = inner
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Ok((
            absolute.clone(),
            if shown.is_empty() { ".".into() } else { shown },
        ))
    }

    pub fn list(&self, relative: &str) -> Result<DirListing, String> {
        let (absolute, shown) = self.inside(relative)?;
        let mut entries = Vec::new();
        let mut total = 0;
        for entry in fs::read_dir(&absolute)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            total += 1;
            if entries.len() >= MAX_LIST_ENTRIES {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let meta = entry.metadata().ok();
            let dir = meta.as_ref().is_some_and(fs::Metadata::is_dir);
            entries.push(DirEntry {
                path: if shown == "." {
                    name.clone()
                } else {
                    format!("{shown}/{name}")
                },
                name,
                dir,
                size: if dir {
                    None
                } else {
                    meta.map(|meta| meta.len())
                },
            });
        }
        entries.sort_by(|a, b| {
            b.dir
                .cmp(&a.dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(DirListing {
            path: shown,
            entries,
            truncated: total > MAX_LIST_ENTRIES,
        })
    }

    pub fn read(&self, relative: &str) -> Result<FileContent, String> {
        let (absolute, shown) = self.inside(relative)?;
        let file = fs::File::open(&absolute).map_err(|error| error.to_string())?;
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        let mut bytes = Vec::new();
        file.take(MAX_READ_BYTES)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let binary = bytes.iter().take(8_000).any(|byte| *byte == 0);
        Ok(FileContent {
            path: shown,
            content: if binary {
                String::new()
            } else {
                String::from_utf8_lossy(&bytes).into_owned()
            },
            truncated: size > MAX_READ_BYTES,
            binary,
            size,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PathBuf, Workspace) {
        let base = std::env::temp_dir().join(format!(
            "neuron-desktop-ws-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        fs::create_dir_all(base.join("ws/src")).unwrap();
        fs::write(base.join("ws/src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(base.join("ws/README.md"), "# hi\n").unwrap();
        fs::write(base.join("secret.txt"), "outside").unwrap();
        let workspace = Workspace::new(base.join("ws")).unwrap();
        (base, workspace)
    }

    fn rand_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    }

    #[test]
    fn lists_folders_first_and_reads_files() {
        let (base, workspace) = fixture();
        let listing = workspace.list(".").unwrap();
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| (entry.path.as_str(), entry.dir))
                .collect::<Vec<_>>(),
            [("src", true), ("README.md", false)]
        );
        assert_eq!(
            workspace.list("src").unwrap().entries[0].path,
            "src/main.rs"
        );
        let file = workspace.read("src/main.rs").unwrap();
        assert_eq!(
            (file.path.as_str(), file.content.as_str(), file.binary),
            ("src/main.rs", "fn main() {}\n", false)
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn refuses_paths_outside_the_workspace() {
        let (base, workspace) = fixture();
        assert_eq!(
            workspace.read("../secret.txt").unwrap_err(),
            "That path is outside the workspace."
        );
        assert!(workspace.list(&base.to_string_lossy()).is_err());
        assert!(workspace
            .read("missing.txt")
            .unwrap_err()
            .starts_with("No such file"));
        let _ = fs::remove_dir_all(base);
    }
}
