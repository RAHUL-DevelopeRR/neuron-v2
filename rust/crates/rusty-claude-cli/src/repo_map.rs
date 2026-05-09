// ── Repo Map — Codebase Context for AI ───────────────────────────────────────
//
// Windsurf Cascade equivalent for terminal.  Automatically generates a
// compressed structural representation of the workspace that helps the AI
// understand file relationships, functions, and symbols.
//
// This is NOT a slash command — it runs silently on startup and injects
// a `[CODEBASE_CONTEXT]` block into the system prompt.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum number of files to index (prevent OOM on huge repos).
const MAX_FILES: usize = 150;
/// Maximum total characters for the repo map output.
const MAX_MAP_CHARS: usize = 6_000;
/// File extensions we know how to extract symbols from.
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "java", "rb", "c", "cpp", "h", "hpp", "toml",
    "yaml", "yml", "json", "md",
];

/// Directories to always skip.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".neuron",
    ".claw",
    ".venv",
    "venv",
    ".tox",
    "vendor",
    ".next",
];

/// A single file entry in the repo map.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub relative_path: String,
    pub symbols: Vec<String>,
    pub size_bytes: u64,
}

/// The full repo map for a workspace.
#[derive(Debug, Clone)]
pub struct RepoMap {
    pub root: PathBuf,
    pub entries: Vec<FileEntry>,
    pub total_files: usize,
    pub total_symbols: usize,
}

impl RepoMap {
    /// Build a repo map by walking the workspace directory.
    #[must_use]
    pub fn build(workspace_root: &Path) -> Self {
        let mut file_map: BTreeMap<String, FileEntry> = BTreeMap::new();
        let mut total_symbols = 0;

        walk_dir(workspace_root, workspace_root, &mut file_map, &mut 0);

        for entry in file_map.values() {
            total_symbols += entry.symbols.len();
        }

        let total_files = file_map.len();
        let entries: Vec<FileEntry> = file_map.into_values().collect();

        Self {
            root: workspace_root.to_path_buf(),
            entries,
            total_files,
            total_symbols,
        }
    }

    /// Render the repo map as a compact text block suitable for system prompt
    /// injection.
    #[must_use]
    pub fn render(&self) -> String {
        let mut output = String::with_capacity(MAX_MAP_CHARS);
        output.push_str("[CODEBASE_CONTEXT]\n");
        output.push_str(&format!(
            "Workspace: {} ({} files, {} symbols)\n\n",
            self.root.display(),
            self.total_files,
            self.total_symbols,
        ));

        for entry in &self.entries {
            let line = if entry.symbols.is_empty() {
                format!("  {}\n", entry.relative_path)
            } else {
                format!("  {} — {}\n", entry.relative_path, entry.symbols.join(", "))
            };
            if output.len() + line.len() > MAX_MAP_CHARS {
                output.push_str("  … (truncated)\n");
                break;
            }
            output.push_str(&line);
        }

        output.push_str("[/CODEBASE_CONTEXT]\n");
        output
    }

    /// Human-readable summary for `/status` display.
    #[must_use]
    pub fn status_line(&self) -> String {
        format!(
            "Indexed: {} files, {} symbols",
            self.total_files, self.total_symbols
        )
    }
}

/// Recursively walk directory, collecting file entries.
fn walk_dir(
    root: &Path,
    current: &Path,
    map: &mut BTreeMap<String, FileEntry>,
    file_count: &mut usize,
) {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if *file_count >= MAX_FILES {
            return;
        }

        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if SKIP_DIRS.contains(&file_name.as_str()) || file_name.starts_with('.') {
                continue;
            }
            walk_dir(root, &path, map, file_count);
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if !SUPPORTED_EXTENSIONS.contains(&ext) {
                continue;
            }

            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");

            let metadata = fs::metadata(&path).ok();
            let size_bytes = metadata.map_or(0, |m| m.len());

            // Only extract symbols for source code files (skip configs/docs)
            let symbols = match ext {
                "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "java" | "rb" | "c" | "cpp"
                | "h" | "hpp" => extract_symbols(&path, ext),
                "toml" | "yaml" | "yml" => extract_config_keys(&path, ext),
                _ => Vec::new(),
            };

            map.insert(
                relative.clone(),
                FileEntry {
                    relative_path: relative,
                    symbols,
                    size_bytes,
                },
            );
            *file_count += 1;
        }
    }
}

/// Extract key symbol names from a source code file using simple line-level
/// pattern matching.  This is deliberately lightweight — no AST parsing, just
/// regex-free keyword detection for speed.
fn extract_symbols(path: &Path, ext: &str) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Vec::new(),
    };

    let mut symbols = Vec::new();
    let max_symbols = 15; // Cap per file to keep the map compact

    for line in content.lines() {
        if symbols.len() >= max_symbols {
            break;
        }
        let trimmed = line.trim();

        match ext {
            "rs" => {
                if let Some(name) = extract_rust_symbol(trimmed) {
                    symbols.push(name);
                }
            }
            "py" => {
                if let Some(name) = extract_python_symbol(trimmed) {
                    symbols.push(name);
                }
            }
            "js" | "ts" | "tsx" | "jsx" => {
                if let Some(name) = extract_js_symbol(trimmed) {
                    symbols.push(name);
                }
            }
            "go" => {
                if let Some(name) = extract_go_symbol(trimmed) {
                    symbols.push(name);
                }
            }
            _ => {
                if let Some(name) = extract_generic_symbol(trimmed) {
                    symbols.push(name);
                }
            }
        }
    }
    symbols
}

fn extract_rust_symbol(line: &str) -> Option<String> {
    // Match common Rust declaration patterns
    let prefixes = [
        "pub fn ",
        "fn ",
        "pub struct ",
        "struct ",
        "pub enum ",
        "enum ",
        "pub trait ",
        "trait ",
        "impl ",
        "pub mod ",
        "mod ",
        "pub(crate) fn ",
        "pub(crate) struct ",
        "pub(crate) enum ",
        "pub(crate) mod ",
    ];
    for prefix in &prefixes {
        if line.starts_with(prefix) {
            let rest = &line[prefix.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() && name != "self" && name != "crate" {
                // Use a cleaned display prefix
                let display_prefix = if prefix.starts_with("pub(crate)") {
                    prefix.replace("pub(crate) ", "pub ")
                } else {
                    prefix.trim().to_string()
                };
                return Some(format!("{} {name}", display_prefix.trim_end()));
            }
        }
    }
    None
}

fn extract_python_symbol(line: &str) -> Option<String> {
    if line.starts_with("def ") {
        let name: String = line[4..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(format!("def {name}()"));
        }
    }
    if line.starts_with("class ") {
        let name: String = line[6..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(format!("class {name}"));
        }
    }
    if line.starts_with("async def ") {
        let name: String = line[10..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(format!("async def {name}()"));
        }
    }
    None
}

fn extract_js_symbol(line: &str) -> Option<String> {
    for prefix in &[
        "export function ",
        "export default function ",
        "function ",
        "export class ",
        "class ",
        "export const ",
        "export default ",
    ] {
        if line.starts_with(prefix) {
            let rest = &line[prefix.len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '$')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn extract_go_symbol(line: &str) -> Option<String> {
    if line.starts_with("func ") {
        let rest = &line[5..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(format!("func {name}"));
        }
    }
    if line.starts_with("type ") && (line.contains(" struct") || line.contains(" interface")) {
        let rest = &line[5..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return Some(format!("type {name}"));
        }
    }
    None
}

fn extract_generic_symbol(line: &str) -> Option<String> {
    if (line.starts_with("public ")
        || line.starts_with("private ")
        || line.starts_with("protected "))
        && (line.contains(" class ") || line.contains(" void ") || line.contains(" int "))
    {
        // Java/C# style — just grab the method/class name
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.len() >= 3 {
            let name = words[2].trim_end_matches('(').trim_end_matches('{');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Extract top-level keys from config files (Cargo.toml, package.json, etc.)
fn extract_config_keys(path: &Path, ext: &str) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut keys = Vec::new();
    match ext {
        "toml" => {
            for line in content.lines().take(30) {
                let trimmed = line.trim();
                if trimmed.starts_with('[') && trimmed.ends_with(']') {
                    keys.push(trimmed.to_string());
                }
            }
        }
        "yaml" | "yml" => {
            for line in content.lines().take(20) {
                if !line.starts_with(' ') && !line.starts_with('#') && line.contains(':') {
                    if let Some(key) = line.split(':').next() {
                        keys.push(key.trim().to_string());
                    }
                }
            }
        }
        _ => {}
    }
    keys.truncate(10);
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_symbol_extraction() {
        assert_eq!(
            extract_rust_symbol("pub fn hello_world(x: i32) -> bool {"),
            Some("pub fn hello_world".to_string())
        );
        assert_eq!(
            extract_rust_symbol("struct MyStruct {"),
            Some("struct MyStruct".to_string())
        );
        assert_eq!(
            extract_rust_symbol("impl Display for Foo {"),
            Some("impl Display".to_string())
        );
    }

    #[test]
    fn python_symbol_extraction() {
        assert_eq!(
            extract_python_symbol("def process_data(items):"),
            Some("def process_data()".to_string())
        );
        assert_eq!(
            extract_python_symbol("class UserManager:"),
            Some("class UserManager".to_string())
        );
    }

    #[test]
    fn js_symbol_extraction() {
        assert_eq!(
            extract_js_symbol("export function fetchData(url) {"),
            Some("fetchData".to_string())
        );
        assert_eq!(
            extract_js_symbol("export class Router {"),
            Some("Router".to_string())
        );
    }

    #[test]
    fn strip_ansi_len_works() {
        assert_eq!(crate::brand::strip_ansi_len("hello"), 5);
    }
}
