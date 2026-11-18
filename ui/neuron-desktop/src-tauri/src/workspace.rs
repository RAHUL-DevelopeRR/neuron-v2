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

// L3: composer. Staged images and saved chats.

/// The most a staged image may weigh.
pub const MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;
/// The most a saved chat may weigh.
pub const MAX_EXPORT_BYTES: usize = 20 * 1024 * 1024;

/// An image copied into the app's staging folder, for a prompt to name.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StagedAttachment {
    pub path: String,
    pub media_type: String,
}

/// A path as people read it: without Windows' `\\?\` prefix.
fn plain_path(path: &Path) -> String {
    let shown = path.display().to_string();
    shown
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .or_else(|| shown.strip_prefix(r"\\?\").map(ToString::to_string))
        .unwrap_or(shown)
}

/// The file extension for an image type, when `bytes` really are that image.
fn image_extension(media_type: &str, bytes: &[u8]) -> Result<&'static str, String> {
    let (extension, matches) = match media_type {
        "image/png" => ("png", bytes.starts_with(b"\x89PNG\r\n\x1a\n")),
        "image/jpeg" => ("jpg", bytes.starts_with(&[0xFF, 0xD8, 0xFF])),
        "image/gif" => (
            "gif",
            bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        ),
        "image/webp" => (
            "webp",
            bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
        ),
        _ => return Err("Only PNG, JPEG, GIF and WebP images can be attached.".to_string()),
    };
    if matches {
        Ok(extension)
    } else {
        Err("That file is not the kind of image it says it is.".to_string())
    }
}

/// Copies an image into `dir` under a new unique name; nothing is overwritten.
pub fn stage_attachment(
    dir: &Path,
    name: &str,
    media_type: &str,
    bytes: &[u8],
) -> Result<StagedAttachment, String> {
    if bytes.is_empty() {
        return Err("That image is empty.".to_string());
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err("Images can be at most 10 MB.".to_string());
    }
    let media_type = media_type.trim().to_lowercase();
    let extension = image_extension(&media_type, bytes)?;
    fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let stem = base.rsplit_once('.').map_or(base, |(stem, _)| stem);
    let mut stem: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(40)
        .collect();
    if stem.trim_matches('-').is_empty() {
        stem = "image".to_string();
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    for attempt in 0..10_000u32 {
        let file = dir.join(format!("{stem}-{stamp}-{attempt}.{extension}"));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
        {
            Ok(mut handle) => {
                use std::io::Write;
                if let Err(error) = handle.write_all(bytes) {
                    drop(handle);
                    let _ = fs::remove_file(&file);
                    return Err(error.to_string());
                }
                let absolute = fs::canonicalize(&file).unwrap_or(file);
                return Ok(StagedAttachment {
                    path: plain_path(&absolute),
                    media_type,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("Could not find a free name for the image.".to_string())
}

/// Reads `%XX` escapes (header values carry file names that way); bad escapes stay as typed.
pub fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let escape = bytes
                .get(at + 1..at + 3)
                .and_then(|pair| std::str::from_utf8(pair).ok())
                .and_then(|pair| u8::from_str_radix(pair, 16).ok());
            if let Some(value) = escape {
                out.push(value);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A suggested file name for the save dialog: a plain name ending in `.md` or `.json`.
pub fn export_file_name(suggested: &str) -> String {
    let base = suggested.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"|?*".contains(c) {
                '-'
            } else {
                c
            }
        })
        .take(80)
        .collect();
    let cleaned = cleaned.trim_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        return "neuron-chat.md".to_string();
    }
    let lower = cleaned.to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".json") {
        cleaned
    } else {
        format!("{cleaned}.md")
    }
}

/// Writes a chat export to the file the user picked in the save dialog.
pub fn write_export(path: &Path, content: &str) -> Result<(), String> {
    if content.len() > MAX_EXPORT_BYTES {
        return Err("That chat is too large to save.".to_string());
    }
    let name = path
        .file_name()
        .ok_or_else(|| "Pick a file, not a folder.".to_string())?
        .to_string_lossy()
        .into_owned();
    // Written beside the target and renamed over it, so a failed or cut-short
    // write never leaves half a file where the user's file was.
    let temp = path.with_file_name(format!(".{name}.neuron-{}.tmp", std::process::id()));
    if let Err(error) = fs::write(&temp, content) {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        if path.is_dir() {
            "Pick a file, not a folder.".to_string()
        } else {
            error.to_string()
        }
    })
}

/// How long a staged image stays in the app's staging folder.
pub const STAGED_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// Deletes files in the staging folder `dir` last changed more than `max_age`
/// before `now`. Only plain files directly inside `dir`: folders and links are
/// left alone, and nothing outside `dir` is ever touched. Returns how many went.
pub fn prune_staged(dir: &Path, now: std::time::SystemTime, max_age: std::time::Duration) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > max_age);
        if old && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
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
                        // L3: a folder is listed too, with a trailing `/`, for `@folder`.
                        if !needle.is_empty() {
                            if let Some(at) = path.to_lowercase().find(&needle) {
                                let in_name = name.to_lowercase().contains(&needle);
                                found.push((
                                    (if in_name { 0 } else { 1000 }) + at * 100 + path.len(),
                                    format!("{path}/"),
                                ));
                            }
                        }
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

    // L3: composer. Staging and saving.
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n0000";
    const PNG_OTHER: &[u8] = b"\x89PNG\r\n\x1a\nother";
    const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0abcd";
    const GIF: &[u8] = b"GIF89a....";
    const WEBP: &[u8] = b"RIFF\x04\x00\x00\x00WEBPVP8 ";

    fn staging_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "neuron-desktop-stage-{}-{}",
            std::process::id(),
            rand_suffix()
        ))
    }

    #[test]
    fn stages_the_four_image_types_with_the_right_extension() {
        let dir = staging_dir();
        for (name, kind, bytes, ext) in [
            ("a.png", "image/png", PNG, "png"),
            ("b.jpg", "image/jpeg", JPEG, "jpg"),
            ("c.gif", "image/gif", GIF, "gif"),
            ("d.webp", "image/webp", WEBP, "webp"),
        ] {
            let staged = stage_attachment(&dir, name, kind, bytes).unwrap();
            assert_eq!(staged.media_type, kind);
            assert!(staged.path.ends_with(&format!(".{ext}")), "{}", staged.path);
            assert_eq!(fs::read(&staged.path).unwrap(), bytes);
            assert!(Path::new(&staged.path).is_absolute());
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn refuses_other_types_wrong_bytes_empty_and_oversize() {
        let dir = staging_dir();
        assert!(stage_attachment(&dir, "a.svg", "image/svg+xml", b"<svg/>").is_err());
        assert!(stage_attachment(&dir, "a.pdf", "application/pdf", PNG).is_err());
        // Labelled png, but not one.
        assert!(stage_attachment(&dir, "a.png", "image/png", b"not an image").is_err());
        assert!(stage_attachment(&dir, "a.png", "image/png", b"").is_err());
        let mut big = PNG.to_vec();
        big.resize(MAX_ATTACHMENT_BYTES + 1, 0);
        assert!(stage_attachment(&dir, "big.png", "image/png", &big)
            .unwrap_err()
            .contains("10 MB"));
        // Exactly the cap is fine.
        big.truncate(MAX_ATTACHMENT_BYTES);
        assert!(stage_attachment(&dir, "big.png", "image/png", &big).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn staged_names_are_unique_and_never_overwrite() {
        let dir = staging_dir();
        let first = stage_attachment(&dir, "shot.png", "image/png", PNG).unwrap();
        let mut seen = vec![first.path.clone()];
        for _ in 0..20 {
            let next = stage_attachment(&dir, "shot.png", "image/png", PNG_OTHER).unwrap();
            assert!(!seen.contains(&next.path));
            seen.push(next.path);
        }
        // The first file still holds its own bytes.
        assert_eq!(fs::read(&first.path).unwrap(), PNG);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn staged_names_cannot_escape_the_folder() {
        let dir = staging_dir();
        let staged = stage_attachment(&dir, r"..\..\evil/../x.png", "image/png", PNG).unwrap();
        let canonical = fs::canonicalize(&dir).unwrap();
        assert!(fs::canonicalize(&staged.path)
            .unwrap()
            .starts_with(&canonical));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn export_names_are_plain_file_names() {
        assert_eq!(export_file_name("chat.md"), "chat.md");
        assert_eq!(export_file_name("Chat.JSON"), "Chat.JSON");
        assert_eq!(export_file_name("../../etc/passwd"), "passwd.md");
        assert_eq!(export_file_name("a:b*c?.md"), "a-b-c-.md");
        assert_eq!(export_file_name(""), "neuron-chat.md");
        assert_eq!(export_file_name("  .. "), "neuron-chat.md");
        assert!(export_file_name(&"x".repeat(500)).len() <= 83);
    }

    #[test]
    fn writes_an_export_and_caps_its_size() {
        let dir = staging_dir();
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("chat.md");
        write_export(&file, "# hi\n").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "# hi\n");
        write_export(&file, "second").unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "second");
        assert!(write_export(&file, &"x".repeat(MAX_EXPORT_BYTES + 1)).is_err());
        assert!(write_export(&dir, "x").is_err());
        // Nothing but the file is left behind, not even on a refused write.
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn staged_images_older_than_a_week_are_pruned_and_nothing_else() {
        use std::time::{Duration, SystemTime};
        let base = staging_dir();
        let staging = base.join("attachments");
        fs::create_dir_all(staging.join("sub")).unwrap();
        let outside = base.join("outside.png");
        fs::write(&outside, "keep").unwrap();
        let age = |path: &Path, days: u64| {
            let file = fs::OpenOptions::new().write(true).open(path).unwrap();
            file.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400))
                .unwrap();
        };
        for (name, days) in [("old.png", 8), ("older.png", 30), ("fresh.png", 1)] {
            fs::write(staging.join(name), "x").unwrap();
            age(&staging.join(name), days);
        }
        fs::write(staging.join("sub/inner.png"), "x").unwrap();
        age(&staging.join("sub/inner.png"), 30);
        age(&outside, 30);

        let removed = prune_staged(&staging, SystemTime::now(), STAGED_MAX_AGE);
        assert_eq!(removed, 2);
        assert!(!staging.join("old.png").exists());
        assert!(!staging.join("older.png").exists());
        assert!(staging.join("fresh.png").exists());
        assert!(
            staging.join("sub/inner.png").exists(),
            "folders are left alone"
        );
        assert!(outside.exists(), "nothing outside the staging folder");
        assert_eq!(
            prune_staged(&base.join("missing"), SystemTime::now(), STAGED_MAX_AGE),
            0
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn search_lists_folders_with_a_trailing_slash() {
        let (base, workspace) = fixture();
        let found = workspace.search("src", 10);
        assert!(found.contains(&"src/".to_string()), "{found:?}");
        assert!(found.contains(&"src/main.rs".to_string()));
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn percent_decoding_reads_header_names() {
        assert_eq!(percent_decode("a%20b%C3%A9.png"), "a b\u{e9}.png");
        assert_eq!(percent_decode("100%"), "100%");
    }
}
