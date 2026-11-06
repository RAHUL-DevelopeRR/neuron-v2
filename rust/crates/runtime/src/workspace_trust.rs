//! Workspace trust shared by the CLI trust prompt and the config loader.
//!
//! A directory becomes trusted when the user accepts the trust prompt (the
//! path is appended to `trusted_dirs.txt` in Neuron's home), when it sits
//! under a `trustedRoots` entry from *user* settings, or when
//! `NEURON_TRUST_WORKSPACE=1` is set for headless runs. Until then, settings
//! that a cloned repository ships in `.neuron.json`, `.neuron/settings*.json`
//! or the `.claw` equivalents cannot run hooks, start MCP servers, widen
//! permissions or choose providers.
//!
//! Only absolute paths count: a relative store location, store line or root
//! would resolve against the (possibly hostile) working directory.
//!
//! Trust also binds to what the folder asked for. When the folder is trusted, a
//! fingerprint of every project setting that applies once trusted (hooks, MCP servers,
//! allow or ask rules, `env`, `model`, plugins, `apiKeyHelper`, a non-restrictive
//! permission mode, ...; settings that only restrict are left out) is stored next to the
//! trust record (`trusted_config.json`, per folder), with a digest per key. A later start
//! whose fingerprint differs (a `git pull` or an edit added a hook or changed `env`) is
//! untrusted again until the user accepts the folder trust screen, which lists those exact
//! commands and the keys that changed. The screen's accept records what the screen showed
//! ([`trust_workspace_root_shown`]): files that changed in between are not trusted.
//!
//! The folder trust screen trusts a whole repository: [`trust_workspace_root`] records the
//! git root, and a folder inside a recorded git root is trusted too. A nested repository
//! has its own root. The home folder (or a folder above it) is trusted for this process
//! only, so it is asked about at every start.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Environment variable that trusts the current workspace without a prompt.
pub const TRUST_WORKSPACE_ENV_VAR: &str = "NEURON_TRUST_WORKSPACE";

const TRUST_STORE_FILE: &str = "trusted_dirs.txt";
const TRUST_FINGERPRINT_FILE: &str = "trusted_config.json";

static TRUST_STORE_OVERRIDE: OnceLock<PathBuf> = OnceLock::new();

/// Directories trusted during this process, so a failed write to the store
/// does not leave the prompt and the loader disagreeing.
static TRUSTED_THIS_PROCESS: OnceLock<Mutex<Vec<PathBuf>>> = OnceLock::new();

/// Fingerprints of what trusted folders asked for, recorded during this process: the
/// home folder is trusted for the process only, and a failed write must not leave the
/// prompt and the loader disagreeing.
static FINGERPRINTS_THIS_PROCESS: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();

/// Per-key setting digests of trusted folders, like [`FINGERPRINTS_THIS_PROCESS`].
type SettingDigests = BTreeMap<String, String>;
static DIGESTS_THIS_PROCESS: OnceLock<Mutex<BTreeMap<String, SettingDigests>>> = OnceLock::new();

/// Pins the trust store location for this process. The CLI calls this at
/// startup so the loader and the trust prompt read the same file; later calls
/// are ignored.
pub fn set_trust_store_path(path: impl Into<PathBuf>) {
    let _ = TRUST_STORE_OVERRIDE.set(path.into());
}

/// Location of the trusted-directories list, or `None` when no absolute
/// location can be resolved (no home directory).
#[must_use]
pub fn trust_store_path() -> Option<PathBuf> {
    TRUST_STORE_OVERRIDE
        .get()
        .cloned()
        .or_else(|| default_neuron_home().map(|home| home.join(TRUST_STORE_FILE)))
        .filter(|path| path.is_absolute())
}

/// Where the per-folder fingerprints live: next to the trust store.
fn fingerprint_store_path() -> Option<PathBuf> {
    trust_store_path().map(|store| store.with_file_name(TRUST_FINGERPRINT_FILE))
}

/// The key a folder's fingerprint is stored under: the folder as the filesystem names
/// it, lower-cased on Windows where paths ignore case.
fn fingerprint_key(cwd: &Path) -> String {
    let path = canonical(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let key = path.display().to_string();
    if cfg!(windows) {
        key.to_ascii_lowercase()
    } else {
        key
    }
}

/// What the folder asked for when the user trusted it (see [`config_fingerprint_matches`]):
/// the empty string when it asked for nothing, `None` for a record made before
/// fingerprints existed.
#[must_use]
pub fn recorded_config_fingerprint(cwd: &Path) -> Option<String> {
    let key = fingerprint_key(cwd);
    if let Some(found) = FINGERPRINTS_THIS_PROCESS.get().and_then(|map| {
        map.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }) {
        return Some(found);
    }
    let content = fs::read_to_string(fingerprint_store_path()?).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    parsed
        .get("folders")?
        .get(&key)?
        .as_str()
        .map(str::to_string)
}

/// The per-key digests of what `cwd` asked for when the user trusted it, for naming the
/// keys that changed since; `None` for a record made without them.
#[must_use]
pub fn recorded_setting_digests(cwd: &Path) -> Option<SettingDigests> {
    let key = fingerprint_key(cwd);
    if let Some(found) = DIGESTS_THIS_PROCESS.get().and_then(|map| {
        map.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
    }) {
        return Some(found);
    }
    let content = fs::read_to_string(fingerprint_store_path()?).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&content).ok()?;
    let digests = parsed.get("settings")?.get(&key)?.as_object()?;
    Some(
        digests
            .iter()
            .filter_map(|(name, digest)| Some((name.clone(), digest.as_str()?.to_string())))
            .collect(),
    )
}

/// Records what `cwd` asks for (`None`: nothing) as what the user just trusted.
pub fn record_config_fingerprint(cwd: &Path, fingerprint: Option<&str>) -> io::Result<()> {
    record_config_state(cwd, fingerprint, None)
}

/// [`record_config_fingerprint`], with the per-key digests behind it (`None`: keep none).
fn record_config_state(
    cwd: &Path,
    fingerprint: Option<&str>,
    digests: Option<&SettingDigests>,
) -> io::Result<()> {
    let key = fingerprint_key(cwd);
    let value = fingerprint.unwrap_or("").to_string();
    FINGERPRINTS_THIS_PROCESS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key.clone(), value.clone());
    {
        let mut recorded = DIGESTS_THIS_PROCESS
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match digests {
            Some(digests) => recorded.insert(key.clone(), digests.clone()),
            None => recorded.remove(&key),
        };
    }
    let path = fingerprint_store_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory to keep the trust store in",
        )
    })?;
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
    let section = |name: &str| {
        existing
            .as_ref()
            .and_then(|parsed| {
                parsed
                    .get(name)
                    .and_then(|found| found.as_object().cloned())
            })
            .unwrap_or_default()
    };
    let mut folders = section("folders");
    let mut settings = section("settings");
    folders.insert(key.clone(), serde_json::Value::String(value));
    match digests {
        Some(digests) => settings.insert(key, serde_json::json!(digests)),
        None => settings.remove(&key),
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::json!({ "folders": folders, "settings": settings }).to_string();
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, body)?;
    fs::rename(&temp, &path)
}

/// Whether a trust record made earlier still covers what `cwd` asks for now.
/// `current` is the fingerprint of the folder's hooks, MCP servers and rules (`None`:
/// it asks for nothing, which needs no approval). A folder trusted by the environment
/// override is not checked.
#[must_use]
pub fn config_fingerprint_matches(cwd: &Path, current: Option<&str>) -> bool {
    if trust_env_override() {
        return true;
    }
    let Some(current) = current else {
        return true;
    };
    recorded_config_fingerprint(cwd).as_deref() == Some(current)
}

/// Whether `NEURON_TRUST_WORKSPACE` asks to trust the current workspace.
#[must_use]
pub fn trust_env_override() -> bool {
    std::env::var(TRUST_WORKSPACE_ENV_VAR).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    })
}

/// Whether `cwd` is trusted by the environment override, this process, or the
/// trust store: itself, or the git root it is in.
#[must_use]
pub fn is_workspace_trusted(cwd: &Path) -> bool {
    if trust_env_override() || trusted_this_process(cwd) {
        return true;
    }
    let root = trust_root(cwd);
    let git_root = (root.is_git_root && root.root != cwd).then_some(root.root);
    if git_root.as_deref().is_some_and(trusted_this_process) {
        return true;
    }
    trust_store_path().is_some_and(|store| trusted_by_store(&store, cwd, git_root.as_deref()))
}

/// Whether `store` lists `cwd`, or the git root it is in.
fn trusted_by_store(store: &Path, cwd: &Path, git_root: Option<&Path>) -> bool {
    is_trusted_in(store, cwd) || git_root.is_some_and(|root| is_trusted_in(store, root))
}

/// Where trust for a folder is recorded, and for how long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRoot {
    /// The git root `cwd` is in, or `cwd` itself outside git.
    pub root: PathBuf,
    pub is_git_root: bool,
    /// The home folder or a folder above it: trusted for this process only.
    pub session_only: bool,
}

/// Where trusting `cwd` records the trust: its git root, unless that is the home folder or
/// above it, else `cwd` itself.
#[must_use]
pub fn trust_root(cwd: &Path) -> TrustRoot {
    let home = user_home();
    let at_or_above_home = |path: &Path| {
        path.parent().is_none()
            || home
                .as_deref()
                .is_some_and(|home| is_ancestor_or_same(path, home))
    };
    let session_only = at_or_above_home(cwd);
    let git_root = (!session_only)
        .then(|| git_root_of(cwd))
        .flatten()
        .filter(|root| !at_or_above_home(root));
    match git_root {
        Some(root) => TrustRoot {
            root,
            is_git_root: true,
            session_only,
        },
        None => TrustRoot {
            root: cwd.to_path_buf(),
            is_git_root: false,
            session_only,
        },
    }
}

/// Why trusting a folder was refused.
#[derive(Debug)]
pub enum TrustWorkspaceError {
    /// The folder's settings are not what the trust screen showed: they changed after it
    /// was drawn, so nothing was trusted and the screen must be shown again.
    ChangedSinceShown,
    Io(io::Error),
}

impl std::fmt::Display for TrustWorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChangedSinceShown => write!(
                f,
                "the project's settings changed after the trust screen was shown"
            ),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TrustWorkspaceError {}

impl From<io::Error> for TrustWorkspaceError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Trusts `cwd` as the folder trust screen does: records its [`trust_root`] (so a whole
/// repository), or, for the home folder, trusts it for this process only. What is recorded
/// is what the folder asks for now; use [`trust_workspace_root_shown`] when a screen listed
/// it first.
pub fn trust_workspace_root(cwd: &Path) -> io::Result<TrustRoot> {
    trust_workspace_root_checked(cwd, None).map_err(|error| match error {
        TrustWorkspaceError::Io(error) => error,
        TrustWorkspaceError::ChangedSinceShown => {
            io::Error::new(io::ErrorKind::Other, error.to_string())
        }
    })
}

/// [`trust_workspace_root`] for a trust screen that listed the folder's settings: `shown` is
/// the fingerprint it was drawn from (`ConfigLoader::workspace_config_fingerprint`, `None`
/// when the folder asked for nothing). When the files now give a different fingerprint,
/// nothing is trusted ([`TrustWorkspaceError::ChangedSinceShown`]).
pub fn trust_workspace_root_shown(
    cwd: &Path,
    shown: Option<&str>,
) -> Result<TrustRoot, TrustWorkspaceError> {
    trust_workspace_root_checked(cwd, Some(shown))
}

fn trust_workspace_root_checked(
    cwd: &Path,
    shown: Option<Option<&str>>,
) -> Result<TrustRoot, TrustWorkspaceError> {
    let root = trust_root(cwd);
    // One read of the files decides both the comparison and what is recorded.
    let preview = crate::ConfigLoader::default_for(cwd).workspace_trust_preview();
    let fingerprint = preview.config_fingerprint();
    if shown.is_some_and(|shown| shown != fingerprint.as_deref()) {
        return Err(TrustWorkspaceError::ChangedSinceShown);
    }
    let fingerprint_result =
        record_config_state(cwd, fingerprint.as_deref(), Some(&preview.setting_digests));
    if root.session_only {
        let cwd = absolute_cwd(cwd)?;
        TRUSTED_THIS_PROCESS
            .get_or_init(|| Mutex::new(Vec::new()))
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(cwd);
        return Ok(root);
    }
    record_trusted_workspace(&root.root)?;
    fingerprint_result?;
    Ok(root)
}

/// The nearest folder at or above `cwd` with a `.git` entry (a folder, or the file of a
/// worktree or submodule).
fn git_root_of(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Whether `path` is `other` or one of its parents, compared after canonicalization.
fn is_ancestor_or_same(path: &Path, other: &Path) -> bool {
    let path = canonical(path).unwrap_or_else(|| path.to_path_buf());
    let other = canonical(other).unwrap_or_else(|| other.to_path_buf());
    other.starts_with(&path)
}

/// The user's home (`USERPROFILE` on Windows, `HOME` elsewhere), when absolute.
fn user_home() -> Option<PathBuf> {
    let (primary, secondary) = if cfg!(windows) {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    };
    non_empty_env(primary)
        .or_else(|| non_empty_env(secondary))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Whether `store` lists `cwd`. Lines match exactly or after canonicalization,
/// so `C:\repo` and `c:\repo\` refer to the same directory. Relative lines are
/// ignored.
#[must_use]
pub fn is_trusted_in(store: &Path, cwd: &Path) -> bool {
    let Ok(content) = fs::read_to_string(store) else {
        return false;
    };
    let lines: Vec<&Path> = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(Path::new)
        .filter(|line| line.is_absolute())
        .collect();
    if lines.contains(&cwd) {
        return true;
    }
    let Some(cwd) = canonical(cwd) else {
        return false;
    };
    lines
        .iter()
        .any(|line| *line == cwd || canonical(line).as_deref() == Some(cwd.as_path()))
}

/// Whether `cwd` is `root` or inside it, compared component-wise after
/// canonicalization so `/repo-evil` is not inside `/repo`. Relative or empty
/// roots never match.
#[must_use]
pub fn path_is_under_root(cwd: &Path, root: &Path) -> bool {
    if !root.is_absolute() {
        return false;
    }
    let cwd = canonical(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let root = canonical(root).unwrap_or_else(|| root.to_path_buf());
    cwd.is_absolute() && cwd.starts_with(&root)
}

/// Trusts `cwd` for this process and appends it to the trust store.
pub fn record_trusted_workspace(cwd: &Path) -> io::Result<()> {
    let cwd = absolute_cwd(cwd)?;
    TRUSTED_THIS_PROCESS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(cwd.clone());
    let store = trust_store_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no home directory to keep the trust store in",
        )
    })?;
    record_trusted_in(&store, &cwd)
}

/// Appends `cwd` to `store`, creating parent directories when needed.
pub fn record_trusted_in(store: &Path, cwd: &Path) -> io::Result<()> {
    let cwd = absolute_cwd(cwd)?;
    if is_trusted_in(store, &cwd) {
        return Ok(());
    }
    if let Some(parent) = store.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(store)?;
    writeln!(file, "{}", cwd.display())
}

fn trusted_this_process(cwd: &Path) -> bool {
    let Some(trusted) = TRUSTED_THIS_PROCESS.get() else {
        return false;
    };
    let trusted = trusted.lock().unwrap_or_else(PoisonError::into_inner);
    if trusted.iter().any(|path| path == cwd) {
        return true;
    }
    let cwd = canonical(cwd);
    trusted
        .iter()
        .any(|path| cwd.is_some() && canonical(path) == cwd)
}

/// The store is one path per line, read back trimmed, so only absolute paths
/// without control characters or surrounding whitespace can be recorded.
fn absolute_cwd(cwd: &Path) -> io::Result<PathBuf> {
    let display = cwd.display().to_string();
    if !cwd.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to trust relative path {display}"),
        ));
    }
    if display.trim() != display || display.chars().any(char::is_control) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to trust a path with control characters or surrounding whitespace",
        ));
    }
    Ok(cwd.to_path_buf())
}

fn canonical(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

/// Neuron's home: `NEURON_HOME`, otherwise `.neuroncli` in the user's home
/// (`USERPROFILE` on Windows, `HOME` elsewhere), matching the CLI. `None` when
/// no absolute home is available.
#[must_use]
pub fn default_neuron_home() -> Option<PathBuf> {
    if let Some(home) = non_empty_env("NEURON_HOME") {
        return Some(PathBuf::from(home)).filter(|path| path.is_absolute());
    }
    user_home().map(|home| home.join(".neuroncli"))
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        is_trusted_in, path_is_under_root, record_trusted_in, trust_root, trusted_by_store,
        user_home,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("neuron-trust-{label}-{nanos}"))
    }

    #[test]
    fn unknown_workspace_is_untrusted_until_recorded() {
        let root = temp_dir("record");
        let workspace = root.join("repo");
        fs::create_dir_all(&workspace).expect("workspace");
        let store = root.join("home").join("trusted_dirs.txt");

        assert!(!is_trusted_in(&store, &workspace));
        record_trusted_in(&store, &workspace).expect("record trust");
        assert!(is_trusted_in(&store, &workspace));

        record_trusted_in(&store, &workspace).expect("record trust twice");
        let content = fs::read_to_string(&store).expect("store");
        assert_eq!(content.lines().count(), 1, "trust is recorded once");

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn trust_is_not_inherited_by_sibling_or_child_directories() {
        let root = temp_dir("scope");
        let workspace = root.join("repo");
        let child = workspace.join("vendor").join("dep");
        let sibling = root.join("repo-evil");
        fs::create_dir_all(&child).expect("child");
        fs::create_dir_all(&sibling).expect("sibling");
        let store = root.join("trusted_dirs.txt");
        record_trusted_in(&store, &workspace).expect("record trust");

        assert!(!is_trusted_in(&store, &child));
        assert!(!is_trusted_in(&store, &sibling));

        fs::remove_dir_all(root).expect("cleanup");
    }

    /// The trust screen records a repository's git root: its folders are trusted with it,
    /// a nested repository is not.
    #[test]
    fn trust_recorded_for_a_git_root_covers_its_folders_but_not_nested_repositories() {
        let root = temp_dir("git-root");
        let repo = root.join("repo");
        let src = repo.join("src").join("deep");
        let nested = repo.join("vendor").join("lib");
        fs::create_dir_all(repo.join(".git")).expect("repo");
        fs::create_dir_all(&src).expect("src");
        fs::create_dir_all(nested.join(".git")).expect("nested repo");
        let store = root.join("trusted_dirs.txt");

        let found = trust_root(&src);
        assert_eq!(found.root, repo);
        assert!(found.is_git_root);
        assert!(!found.session_only);
        assert_eq!(trust_root(&nested).root, nested);

        record_trusted_in(&store, &found.root).expect("record trust");
        assert!(trusted_by_store(&store, &src, Some(&found.root)));
        let nested_root = trust_root(&nested);
        assert!(!trusted_by_store(
            &store,
            &nested,
            nested_root
                .is_git_root
                .then_some(nested_root.root.as_path())
        ));

        // Outside git, the folder itself.
        let plain = root.join("plain");
        fs::create_dir_all(&plain).expect("plain");
        let outside = trust_root(&plain);
        if !outside.is_git_root {
            assert_eq!(outside.root, plain);
        }
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn the_home_folder_is_trusted_for_one_session_only() {
        let Some(home) = user_home() else {
            return;
        };
        let found = trust_root(&home);
        assert!(found.session_only);
        assert!(!found.is_git_root);
        assert_eq!(found.root, home);
        if let Some(above) = home.parent() {
            assert!(trust_root(above).session_only);
        }
    }

    #[test]
    fn relative_store_lines_and_roots_never_match() {
        let root = temp_dir("relative");
        let workspace = root.join("repo");
        fs::create_dir_all(&workspace).expect("workspace");
        let store = root.join("trusted_dirs.txt");
        fs::write(&store, ".\n\nrepo\n..\n").expect("store");

        assert!(!is_trusted_in(&store, &workspace));
        assert!(!path_is_under_root(&workspace, Path::new(".")));
        assert!(!path_is_under_root(&workspace, Path::new("")));
        assert!(path_is_under_root(&workspace, &root));
        assert!(record_trusted_in(&store, Path::new("repo")).is_err());

        fs::remove_dir_all(root).expect("cleanup");
    }
}
