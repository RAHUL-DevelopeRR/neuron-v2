//! A per-session, content-addressed store for what the agent produces: tool outputs too
//! large for the conversation, generated reports, other files worth keeping.
//!
//! # Storage
//!
//! ```text
//! <NEURON_HOME>/artifacts/<session-id>/
//! ├── manifest.json         every artifact: id, name, source, kind, size, time
//! └── objects/3f/a29c…      contents, named by SHA-256 (as in checkpoints)
//! ```
//!
//! An artifact's id is the first [`ID_LEN`] hex digits of its content hash, so storing
//! the same bytes twice keeps one object and one manifest entry. One artifact holds at
//! most [`ArtifactLimits::max_artifact_bytes`], a session at most
//! [`ArtifactLimits::max_session_bytes`]; past either, [`ArtifactStore::put`] refuses and
//! the caller keeps its old behaviour (a truncated tool result). Session folders idle for
//! longer than [`DEFAULT_RETENTION`] are removed when a store opens.
//!
//! # Tool results
//!
//! [`offload_tool_result`] is the runtime's hook: a tool output longer than the context
//! cap is saved whole, and the model gets the capped preview with a note naming the
//! artifact id, its size and its path.

use std::fmt::{self, Display, Formatter};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::checkpoints::{hex_digest, now_ms, temp_sibling, valid_hash, valid_session_id};

/// Hex digits of the content hash that name an artifact.
pub const ID_LEN: usize = 12;
/// The folder under Neuron's home that holds every session's artifacts.
pub const ARTIFACTS_DIR: &str = "artifacts";
/// Session folders untouched this long are removed when a store opens.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(14 * 24 * 60 * 60);
/// Turns the store off for tool results (`0`, `off`, `false`).
pub const ARTIFACTS_ENV_VAR: &str = "NEURON_ARTIFACTS";

const MANIFEST_FILE: &str = "manifest.json";
const OBJECTS_DIR: &str = "objects";

/// Size caps of a store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactLimits {
    /// The largest single artifact.
    pub max_artifact_bytes: u64,
    /// All of a session's artifacts together.
    pub max_session_bytes: u64,
}

impl Default for ArtifactLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 32 * 1024 * 1024,
            max_session_bytes: 256 * 1024 * 1024,
        }
    }
}

/// One stored artifact, as the manifest lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEntry {
    pub id: String,
    /// SHA-256 of the contents, hex.
    pub hash: String,
    /// A display name (`bash output`, `report.md`).
    pub name: String,
    /// What produced it: a tool name, `agent`, ...
    pub source: String,
    /// `tool_output`, `report`, `file`.
    pub kind: String,
    pub bytes: u64,
    pub created_ms: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    #[serde(default)]
    artifacts: Vec<ArtifactEntry>,
}

/// Why an artifact was not stored.
#[derive(Debug)]
pub enum ArtifactError {
    /// The session id cannot name a folder.
    InvalidSession(String),
    /// The artifact alone is over [`ArtifactLimits::max_artifact_bytes`].
    TooLarge {
        bytes: u64,
        limit: u64,
    },
    /// The session's artifacts would go over [`ArtifactLimits::max_session_bytes`].
    SessionFull {
        used: u64,
        limit: u64,
    },
    Io {
        path: PathBuf,
        source: io::Error,
    },
}

impl Display for ArtifactError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSession(id) => write!(f, "invalid session id for artifacts: {id}"),
            Self::TooLarge { bytes, limit } => {
                write!(
                    f,
                    "artifact of {bytes} bytes is over the {limit}-byte limit"
                )
            }
            Self::SessionFull { used, limit } => write!(
                f,
                "the session's artifacts use {used} of {limit} bytes; no room for more"
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for ArtifactError {}

fn io_error(path: &Path, source: io::Error) -> ArtifactError {
    ArtifactError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// The artifacts of one session.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    dir: PathBuf,
    session_id: String,
    limits: ArtifactLimits,
    manifest_entries: Vec<ArtifactEntry>,
}

impl ArtifactStore {
    /// Opens (without creating anything yet) the artifacts of `session_id` under `root`
    /// (`<NEURON_HOME>/artifacts`), removing other sessions' folders idle for longer
    /// than [`DEFAULT_RETENTION`].
    pub fn open(root: &Path, session_id: &str) -> Result<Self, ArtifactError> {
        Self::open_with_limits(root, session_id, ArtifactLimits::default())
    }

    pub fn open_with_limits(
        root: &Path,
        session_id: &str,
        limits: ArtifactLimits,
    ) -> Result<Self, ArtifactError> {
        if !valid_session_id(session_id) {
            return Err(ArtifactError::InvalidSession(session_id.to_string()));
        }
        cleanup_expired(root, session_id, DEFAULT_RETENTION);
        let dir = root.join(session_id);
        let manifest: Manifest = fs::read(dir.join(MANIFEST_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            dir,
            session_id: session_id.to_string(),
            limits,
            manifest_entries: manifest
                .artifacts
                .into_iter()
                .filter(|entry| valid_hash(&entry.hash))
                .collect(),
        })
    }

    /// The store of `session_id` in Neuron's home ([`default_root`]).
    pub fn open_default(session_id: &str) -> Result<Self, ArtifactError> {
        let root = default_root().ok_or_else(|| ArtifactError::Io {
            path: PathBuf::from(ARTIFACTS_DIR),
            source: io::Error::new(io::ErrorKind::NotFound, "no home folder for Neuron"),
        })?;
        Self::open(&root, session_id)
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn limits(&self) -> ArtifactLimits {
        self.limits
    }

    /// Every artifact, oldest first.
    #[must_use]
    pub fn list(&self) -> &[ArtifactEntry] {
        &self.manifest_entries
    }

    /// Bytes all artifacts take together.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.manifest_entries.iter().map(|entry| entry.bytes).sum()
    }

    /// The artifact `id` (or a longer prefix of its hash).
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&ArtifactEntry> {
        let id = id.trim();
        if id.len() < 4 {
            return None;
        }
        self.manifest_entries
            .iter()
            .find(|entry| entry.id == id || entry.hash.starts_with(id))
    }

    /// Where the contents of `entry` are.
    #[must_use]
    pub fn object_path(&self, entry: &ArtifactEntry) -> PathBuf {
        self.dir
            .join(OBJECTS_DIR)
            .join(&entry.hash[..2])
            .join(&entry.hash[2..])
    }

    /// The contents of artifact `id`.
    #[must_use]
    pub fn read(&self, id: &str) -> Option<Vec<u8>> {
        let entry = self.get(id)?;
        fs::read(self.object_path(entry)).ok()
    }

    /// Stores `contents`. The same bytes stored again return the existing entry.
    pub fn put(
        &mut self,
        name: &str,
        source: &str,
        kind: &str,
        contents: &[u8],
    ) -> Result<ArtifactEntry, ArtifactError> {
        let bytes = contents.len() as u64;
        if bytes > self.limits.max_artifact_bytes {
            return Err(ArtifactError::TooLarge {
                bytes,
                limit: self.limits.max_artifact_bytes,
            });
        }
        let hash = hex_digest(&Sha256::digest(contents));
        if let Some(existing) = self
            .manifest_entries
            .iter()
            .find(|entry| entry.hash == hash)
        {
            return Ok(existing.clone());
        }
        let used = self.total_bytes();
        if used.saturating_add(bytes) > self.limits.max_session_bytes {
            return Err(ArtifactError::SessionFull {
                used,
                limit: self.limits.max_session_bytes,
            });
        }
        let entry = ArtifactEntry {
            id: hash[..ID_LEN].to_string(),
            hash,
            name: name.to_string(),
            source: source.to_string(),
            kind: kind.to_string(),
            bytes,
            created_ms: now_ms(),
        };
        let object = self.object_path(&entry);
        if let Some(parent) = object.parent() {
            fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
        }
        if !object.exists() {
            write_atomic(&object, contents)?;
        }
        self.manifest_entries.push(entry.clone());
        if let Err(error) = self.save_manifest() {
            self.manifest_entries.pop();
            return Err(error);
        }
        Ok(entry)
    }

    fn save_manifest(&self) -> Result<(), ArtifactError> {
        let path = self.dir.join(MANIFEST_FILE);
        let manifest = Manifest {
            artifacts: self.manifest_entries.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| io_error(&path, io::Error::other(error)))?;
        write_atomic(&path, &bytes)
    }
}

/// `<NEURON_HOME>/artifacts`, when Neuron has a home folder.
#[must_use]
pub fn default_root() -> Option<PathBuf> {
    crate::workspace_trust::default_neuron_home().map(|home| home.join(ARTIFACTS_DIR))
}

/// Whether tool results go to the artifact store: on unless [`ARTIFACTS_ENV_VAR`] says
/// `0`, `off` or `false`.
#[must_use]
pub fn enabled_from_env() -> bool {
    !std::env::var(ARTIFACTS_ENV_VAR).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        )
    })
}

/// A tool `output` longer than `limit` bytes is stored whole in `store` and comes back
/// as `cap(output)` followed by a note naming the artifact. Shorter outputs, or any
/// output when the store refuses it, come back as `cap(output)`.
pub fn offload_tool_result(
    store: Option<&mut ArtifactStore>,
    tool_name: &str,
    output: String,
    limit: usize,
    cap: impl FnOnce(String) -> String,
) -> String {
    let Some(store) = store else {
        return cap(output);
    };
    if output.len() <= limit {
        return cap(output);
    }
    match store.put(
        &format!("{tool_name} output"),
        tool_name,
        "tool_output",
        output.as_bytes(),
    ) {
        Ok(entry) => {
            let path = store.object_path(&entry);
            format!(
                "{}\n\n[The full output ({} bytes) is saved as artifact {}: {}. Read that file for the parts left out above.]",
                cap(output),
                entry.bytes,
                entry.id,
                path.display()
            )
        }
        Err(_) => cap(output),
    }
}

/// The `/artifacts` listing.
#[must_use]
pub fn format_listing(store: &ArtifactStore) -> String {
    let entries = store.list();
    if entries.is_empty() {
        return format!(
            "Artifacts\n  No artifacts in this session yet.\n  Folder           {}",
            store.dir().display()
        );
    }
    let mut lines = vec![format!(
        "Artifacts ({} \u{b7} {})",
        entries.len(),
        human_bytes(store.total_bytes())
    )];
    for entry in entries {
        lines.push(format!(
            "  {}  {:>9}  {:<12} {}",
            entry.id,
            human_bytes(entry.bytes),
            entry.kind,
            entry.name
        ));
    }
    lines.push(format!("  Folder           {}", store.dir().display()));
    lines.join("\n")
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * 1024;
    if bytes >= MIB {
        format!("{}.{} MiB", bytes / MIB, (bytes % MIB) * 10 / MIB)
    } else if bytes >= KIB {
        format!("{}.{} KiB", bytes / KIB, (bytes % KIB) * 10 / KIB)
    } else {
        format!("{bytes} B")
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ArtifactError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    }
    let temp = temp_sibling(path);
    if let Err(error) = fs::write(&temp, bytes).and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(io_error(path, error));
    }
    Ok(())
}

/// Removes session folders under `root` (other than `keep`) whose manifest is older
/// than `retention`.
fn cleanup_expired(root: &Path, keep: &str, retention: Duration) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name == keep || !valid_session_id(name) {
            continue;
        }
        let path = entry.path();
        let modified = fs::metadata(path.join(MANIFEST_FILE))
            .or_else(|_| fs::metadata(&path))
            .and_then(|metadata| metadata.modified());
        if let Ok(modified) = modified {
            if now
                .duration_since(modified)
                .is_ok_and(|age| age >= retention)
            {
                let _ = fs::remove_dir_all(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "neuron-artifacts-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn put_stores_content_addressed_objects_with_a_manifest() {
        let root = TempRoot::new("put");
        let mut store = ArtifactStore::open(&root.0, "session-a").unwrap();
        assert!(store.list().is_empty());
        let entry = store
            .put("report.md", "agent", "report", b"# Report\n")
            .unwrap();
        assert_eq!(entry.id.len(), ID_LEN);
        assert!(entry.hash.starts_with(&entry.id));
        assert_eq!(entry.bytes, 9);
        let object = store.object_path(&entry);
        assert!(object.starts_with(root.0.join("session-a").join("objects")));
        assert_eq!(fs::read(&object).unwrap(), b"# Report\n");
        assert_eq!(store.read(&entry.id).unwrap(), b"# Report\n");

        // The same bytes again: one entry, one object.
        let again = store.put("copy", "agent", "report", b"# Report\n").unwrap();
        assert_eq!(again, entry);
        assert_eq!(store.list().len(), 1);

        // A reopened store reads the manifest.
        let reopened = ArtifactStore::open(&root.0, "session-a").unwrap();
        assert_eq!(reopened.list(), store.list());
        assert_eq!(reopened.get(&entry.id), Some(&entry));
    }

    #[test]
    fn size_caps_refuse_large_artifacts_and_full_sessions() {
        let root = TempRoot::new("caps");
        let limits = ArtifactLimits {
            max_artifact_bytes: 10,
            max_session_bytes: 15,
        };
        let mut store = ArtifactStore::open_with_limits(&root.0, "s", limits).unwrap();
        assert!(matches!(
            store.put("big", "t", "tool_output", &[b'x'; 11]),
            Err(ArtifactError::TooLarge { bytes: 11, .. })
        ));
        store.put("a", "t", "tool_output", &[b'a'; 10]).unwrap();
        assert!(matches!(
            store.put("b", "t", "tool_output", &[b'b'; 6]),
            Err(ArtifactError::SessionFull { used: 10, .. })
        ));
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn invalid_session_ids_are_refused() {
        let root = TempRoot::new("ids");
        assert!(ArtifactStore::open(&root.0, "../escape").is_err());
        assert!(ArtifactStore::open(&root.0, "").is_err());
    }

    #[test]
    fn large_tool_results_are_offloaded_with_a_preview_and_id() {
        let root = TempRoot::new("offload");
        let mut store = ArtifactStore::open(&root.0, "s1").unwrap();
        let output = "line\n".repeat(100);
        let cap = |text: String| text.chars().take(20).collect::<String>();

        let small = offload_tool_result(Some(&mut store), "bash", "short".to_string(), 100, cap);
        assert_eq!(small, "short");
        assert!(store.list().is_empty());

        let preview = offload_tool_result(Some(&mut store), "bash", output.clone(), 100, cap);
        let entry = store.list()[0].clone();
        assert!(preview.starts_with(&output[..20]));
        assert!(preview.contains(&format!("artifact {}", entry.id)));
        assert!(preview.contains("500 bytes"));
        assert_eq!(entry.source, "bash");
        assert_eq!(entry.kind, "tool_output");
        assert_eq!(store.read(&entry.id).unwrap(), output.as_bytes());

        // Without a store, or when the store refuses: the plain cap.
        assert_eq!(
            offload_tool_result(None, "bash", output.clone(), 100, cap),
            output[..20]
        );
        let mut tiny = ArtifactStore::open_with_limits(
            &root.0,
            "s2",
            ArtifactLimits {
                max_artifact_bytes: 10,
                max_session_bytes: 10,
            },
        )
        .unwrap();
        assert_eq!(
            offload_tool_result(Some(&mut tiny), "bash", output.clone(), 100, cap),
            output[..20]
        );
    }

    #[test]
    fn listing_names_each_artifact_and_the_folder() {
        let root = TempRoot::new("list");
        let mut store = ArtifactStore::open(&root.0, "s").unwrap();
        assert!(format_listing(&store).contains("No artifacts"));
        let entry = store
            .put(
                "grep_search output",
                "grep_search",
                "tool_output",
                &[b'z'; 2048],
            )
            .unwrap();
        let listing = format_listing(&store);
        assert!(listing.contains(&entry.id));
        assert!(listing.contains("2.0 KiB"));
        assert!(listing.contains("grep_search output"));
    }

    #[test]
    fn idle_sessions_are_removed_when_a_store_opens() {
        let root = TempRoot::new("cleanup");
        let mut old = ArtifactStore::open(&root.0, "old").unwrap();
        old.put("x", "t", "tool_output", b"x").unwrap();
        cleanup_expired(&root.0, "new", Duration::ZERO);
        assert!(!root.0.join("old").exists());
    }
}
