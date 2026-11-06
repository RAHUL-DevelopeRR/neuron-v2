//! Sandboxed stdio MCP servers.
//!
//! Every stdio server runs with a [`McpSandboxProfile`]: the paths it may read and
//! write, the hosts it may reach and the inherited environment variables it sees. The
//! profile comes from the server's own `sandbox` setting, else from its catalog entry
//! (a bundled server uses the entry of its name), else it is the unvetted default: the
//! workspace read-only, no network, the default environment only
//! ([`resolve_mcp_sandbox`]). Workspace settings cannot turn the sandbox off.
//!
//! What is enforced where:
//! - Every platform: the environment is an allowlist ([`mcp_sandbox_env`]), not the
//!   inherited environment minus credentials, and Zero-X or provider credentials never
//!   pass it. A server limited to some hosts gets proxy variables for a loopback proxy
//!   ([`crate::net_proxy`]) that reaches only those hosts; one with no network gets a
//!   proxy that reaches none. `NODE_USE_ENV_PROXY` makes Node's own `fetch` use it too
//!   (Node 22.21 and 24.5 or later).
//! - Every platform that enforces file limits: the shared `npx` and `uvx` caches are
//!   writable only by a server that may reach that registry (it could fetch whatever it
//!   would plant there anyway); any other server gets a cache of its own, so it cannot
//!   plant code that the user's own `npx`, or another server, runs later.
//! - Windows: a kill-on-close job object (`winjob`) holds the server and every process
//!   it starts, so stopping the server or ending the engine ends the whole tree (no
//!   orphan `node.exe`). File limits are not enforced (there is no restricted-token spawn
//!   path, see [`crate::os_sandbox`]) and the proxy is advice a program may ignore.
//! - macOS: a `sandbox-exec` profile. File contents in the home folder are unreadable
//!   except the declared roots and the toolchain folders (npm, uv, nvm, ...), writes are
//!   allowed only to the write roots and temp, and outbound connections go only to the
//!   proxy (a host list) or nowhere (no network).
//! - Linux: bubblewrap (`bwrap`) when it is installed: a read-only root, an empty home
//!   with the declared roots and toolchain folders bound back, the write roots writable,
//!   a private `/tmp`, and `/run/user` and `/run/dbus` hidden (a path socket there, such
//!   as the session bus or an ssh agent, stays reachable without a network namespace),
//!   and no network namespace for no network. Not `--die-with-parent`: its signal fires
//!   when the *thread* that started the server ends, and servers are started from
//!   short-lived threads; the server ends with the engine through its closed stdin and
//!   the engine's shutdown instead. Without `bwrap` only the environment allowlist and
//!   the proxy apply, and the server log says so.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::config::{ConfigSource, McpSandboxNetwork, McpSandboxProfile, McpServerConfig};
use crate::mcp_catalog::{McpCatalog, McpCatalogSandbox};
use crate::mcp_client::McpStdioTransport;
use crate::net_allow::{proxy_env, NetworkAllowlist};
use crate::net_proxy::AllowlistProxy;
use crate::sandbox::{sbpl_string, SeatbeltNetwork, SANDBOX_EXEC};

/// Inherited variables every sandboxed server sees: the search path, home, temp,
/// locale, terminal, proxy and certificate settings, toolchain locations, and the
/// Windows system variables programs fail without.
const DEFAULT_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TZ",
    "TMPDIR",
    "TMP",
    "TEMP",
    // Not `XDG_RUNTIME_DIR`: it holds the session bus and agent sockets (Linux hides it
    // under bubblewrap unless `envAllow` names it).
    "XDG_CACHE_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NVM_DIR",
    "VOLTA_HOME",
    "npm_config_cache",
    "npm_config_prefix",
    "npm_config_registry",
    "UV_CACHE_DIR",
    "UV_PYTHON_INSTALL_DIR",
    // Windows
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "PATHEXT",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "ProgramW6432",
    "CommonProgramFiles",
    "CommonProgramFiles(x86)",
    "CommonProgramW6432",
    "ALLUSERSPROFILE",
    "PUBLIC",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "PROCESSOR_IDENTIFIER",
    "OS",
    "USERNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    "PSModulePath",
];

/// Inherited variables with these prefixes pass too (`LC_ALL`, `LC_CTYPE`, ...).
const DEFAULT_ENV_PREFIXES: &[&str] = &["LC_"];

/// Folders and files under the home folder that hold toolchains a server is started
/// with (npm, nvm, uv, ...) and git's own settings (git stops when it cannot read
/// them): readable by every sandboxed server. `.npmrc`, which can hold registry
/// tokens, is not: npm carries on without it.
const TOOLCHAIN_READ: &[&str] = &[
    ".gitconfig",
    ".config/git",
    ".npm",
    ".npm-global",
    ".npm-packages",
    ".nvm",
    ".volta",
    ".fnm",
    ".bun",
    ".deno",
    ".pyenv",
    ".asdf",
    ".cargo/bin",
    ".local/bin",
    ".local/share/uv",
    ".local/share/pnpm",
    ".local/share/fnm",
    "Library/pnpm",
];

/// A cache a server's launcher writes (`npx`, `uvx`) and the user's own runs share.
struct LauncherCache {
    /// Its folders under the home folder.
    home: &'static [&'static str],
    /// Its folders under the OS cache folder.
    cache: &'static [&'static str],
    /// The registry hosts whose reach makes the shared cache writable.
    registries: &'static [&'static str],
    /// The variable that points the launcher at another cache.
    variable: &'static str,
    /// The folder of a server's own cache for it.
    private: &'static str,
}

const LAUNCHER_CACHES: &[LauncherCache] = &[
    LauncherCache {
        home: &[".npm"],
        cache: &[],
        registries: &["registry.npmjs.org"],
        variable: "npm_config_cache",
        private: "npm",
    },
    LauncherCache {
        home: &[".local/share/uv", ".cache/uv"],
        cache: &["uv"],
        registries: &["pypi.org", "files.pythonhosted.org"],
        variable: "UV_CACHE_DIR",
        private: "uv",
    },
];

/// Commands that run a script named in their arguments: that script's folder is
/// readable (it is the server's program), and `${package}` in a profile path is its
/// npm package.
const INTERPRETERS: &[&str] = &[
    "node", "nodejs", "bun", "deno", "tsx", "ts-node", "python", "python3", "pythonw", "py", "uv",
];

/// The file extensions of a script an interpreter runs.
const SCRIPT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "ts", "mts", "cts", "py", "pyz"];

/// Where a server's profile came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSandboxOrigin {
    /// The server's own `sandbox` setting.
    Settings,
    /// The catalog entry with this id.
    Catalog(String),
    /// The default for a server the catalog does not vouch for.
    Unvetted,
}

impl McpSandboxOrigin {
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Settings => "settings".to_string(),
            Self::Catalog(id) => format!("catalog `{id}`"),
            Self::Unvetted => "unvetted default".to_string(),
        }
    }
}

impl From<&McpCatalogSandbox> for McpSandboxProfile {
    fn from(sandbox: &McpCatalogSandbox) -> Self {
        Self {
            off: false,
            fs_read: sandbox.fs_read.clone(),
            fs_write: sandbox.fs_write.clone(),
            network: McpSandboxNetwork::from_hosts(&sandbox.network),
            env_allow: sandbox.env_allow.clone(),
        }
    }
}

/// The profile the stdio server `server_name` (from `scope`, when known) runs with, and
/// where it came from. Not a stdio server: the unvetted default (never used).
#[must_use]
pub fn resolve_mcp_sandbox(
    server_name: &str,
    scope: Option<ConfigSource>,
    config: &McpServerConfig,
) -> (McpSandboxProfile, McpSandboxOrigin) {
    let McpServerConfig::Stdio(stdio) = config else {
        return (McpSandboxProfile::default(), McpSandboxOrigin::Unvetted);
    };
    if let Some(profile) = &stdio.sandbox {
        let workspace_scope = matches!(scope, Some(ConfigSource::Project | ConfigSource::Local));
        if !(profile.off && workspace_scope) {
            return (profile.clone(), McpSandboxOrigin::Settings);
        }
        warn_once(&format!(
            "warning: ignoring \"sandbox\": false for the MCP server `{server_name}`: workspace settings cannot turn a server's sandbox off; set it in your user settings instead"
        ));
    }
    let catalog = McpCatalog::builtin();
    let entry = catalog.entry_for_config(config).or_else(|| {
        (scope == Some(ConfigSource::Bundled))
            .then(|| catalog.get(server_name))
            .flatten()
    });
    match entry {
        Some(entry) => (
            McpSandboxProfile::from(&entry.sandbox),
            McpSandboxOrigin::Catalog(entry.id.clone()),
        ),
        None => (McpSandboxProfile::default(), McpSandboxOrigin::Unvetted),
    }
}

fn warn_once(message: &str) {
    static WARNED: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    let mut warned = WARNED
        .get_or_init(|| Mutex::new(BTreeSet::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if warned.insert(message.to_string()) {
        crate::ui_hooks::notice(message);
    }
}

/// One line saying what `profile` allows, for `/mcp show` and the server log.
#[must_use]
pub fn describe_mcp_sandbox(profile: &McpSandboxProfile) -> String {
    use std::fmt::Write as _;
    if profile.off {
        return "off".to_string();
    }
    let list = |items: &[String]| {
        if items.is_empty() {
            "nothing".to_string()
        } else {
            items.join(", ")
        }
    };
    let network = match &profile.network {
        McpSandboxNetwork::None => "none".to_string(),
        McpSandboxNetwork::Any => "any host".to_string(),
        McpSandboxNetwork::Hosts(hosts) => hosts.join(", "),
    };
    let mut text = format!(
        "read {}; write {}; network {network}",
        list(&profile.fs_read),
        list(&profile.fs_write)
    );
    if !profile.env_allow.is_empty() {
        let _ = write!(text, "; env + {}", profile.env_allow.join(", "));
    }
    text
}

/// What a profile's limits come to on this platform, for `/mcp show`.
#[must_use]
pub fn mcp_sandbox_enforcement(profile: &McpSandboxProfile) -> &'static str {
    if profile.off {
        return "not sandboxed";
    }
    match McpSandboxPlatform::detect() {
        McpSandboxPlatform::Windows => {
            "Windows: process tree contained and environment allowlisted; file limits not enforced, network limits advisory"
        }
        McpSandboxPlatform::MacOs { seatbelt: true } => "macOS: enforced by sandbox-exec",
        McpSandboxPlatform::Linux { bwrap: Some(_) } => {
            "Linux: files and no-network enforced by bubblewrap; host lists advisory"
        }
        McpSandboxPlatform::Linux { bwrap: None } => {
            "Linux: install bubblewrap (bwrap) to enforce file and network limits; environment allowlisted"
        }
        McpSandboxPlatform::MacOs { seatbelt: false } | McpSandboxPlatform::Other => {
            "environment allowlisted only"
        }
    }
}

/// The folders a profile's paths are resolved against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSandboxHost {
    pub workspace: PathBuf,
    pub home: Option<PathBuf>,
    /// The OS cache folder: `%LOCALAPPDATA%`, `~/Library/Caches` or `~/.cache`.
    pub cache: Option<PathBuf>,
    pub temp: PathBuf,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

impl McpSandboxHost {
    /// This process's working directory, home, cache and temp folders.
    #[must_use]
    pub fn current() -> Self {
        let home = if cfg!(windows) {
            env_path("USERPROFILE").or_else(|| env_path("HOME"))
        } else {
            env_path("HOME")
        };
        let cache = if cfg!(windows) {
            env_path("LOCALAPPDATA")
        } else if cfg!(target_os = "macos") {
            home.as_ref()
                .map(|home| home.join("Library").join("Caches"))
        } else {
            env_path("XDG_CACHE_HOME").or_else(|| home.as_ref().map(|home| home.join(".cache")))
        };
        Self {
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            home,
            cache,
            temp: std::env::temp_dir(),
        }
    }

    /// A profile path: `.` or a relative path in the workspace, `~` or `~/...` in the
    /// home folder, `${cache}/...`, `${temp}/...` or `${workspace}/...`, or absolute.
    /// `None` when it names a folder this host does not have.
    #[must_use]
    pub fn expand(&self, entry: &str) -> Option<PathBuf> {
        let entry = entry.trim();
        if entry.is_empty() {
            return None;
        }
        let rest_of = |prefix: &str| {
            entry
                .strip_prefix(prefix)
                .filter(|rest| rest.is_empty() || rest.starts_with(['/', '\\']))
                .map(|rest| rest.trim_start_matches(['/', '\\']))
        };
        let joined = |base: &Path, rest: &str| {
            if rest.is_empty() {
                base.to_path_buf()
            } else {
                base.join(rest)
            }
        };
        let path = if let Some(rest) = rest_of("~") {
            joined(self.home.as_deref()?, rest)
        } else if let Some(rest) = rest_of("${cache}") {
            joined(self.cache.as_deref()?, rest)
        } else if let Some(rest) = rest_of("${temp}") {
            joined(&self.temp, rest)
        } else if let Some(rest) = rest_of("${workspace}") {
            joined(&self.workspace, rest)
        } else if Path::new(entry).is_absolute() {
            PathBuf::from(entry)
        } else {
            self.workspace.join(entry)
        };
        Some(normalize(&path))
    }
}

/// Drops `.` parts and folds `..` without touching the disk, then resolves symlinks
/// when the path exists (a seatbelt profile matches real paths: `/var` is
/// `/private/var` on macOS).
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    if cfg!(windows) {
        return out;
    }
    std::fs::canonicalize(&out).unwrap_or(out)
}

/// The machine a server is started on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpSandboxPlatform {
    Windows,
    MacOs { seatbelt: bool },
    Linux { bwrap: Option<PathBuf> },
    Other,
}

impl McpSandboxPlatform {
    #[must_use]
    pub fn detect() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs {
                seatbelt: Path::new(SANDBOX_EXEC).exists(),
            }
        } else if cfg!(target_os = "linux") {
            Self::Linux {
                bwrap: find_on_path("bwrap"),
            }
        } else {
            Self::Other
        }
    }
}

fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// Whether an inherited variable is a Zero-X or provider credential, which no sandbox
/// setting passes on.
fn is_core_credential(key: &str, value: &std::ffi::OsStr) -> bool {
    let upper = key.to_ascii_uppercase();
    let value = value.to_string_lossy();
    plugins::subprocess::CREDENTIAL_ENV_VARS.contains(&upper.as_str())
        || plugins::subprocess::CREDENTIAL_VALUE_PREFIXES
            .iter()
            .any(|prefix| value.starts_with(prefix))
}

/// The name a variable is compared by: Windows names ignore case.
fn env_key(name: &str, windows: bool) -> String {
    if windows {
        name.to_ascii_uppercase()
    } else {
        name.to_string()
    }
}

/// The whole environment of a sandboxed server: the inherited variables on the
/// default list or the profile's `env_allow`, then the server's own `env` block (which
/// wins). Zero-X and provider credentials never pass; another secret-looking variable
/// (`GITHUB_TOKEN`) passes only when `env_allow` names it.
#[must_use]
pub fn mcp_sandbox_env(
    profile: &McpSandboxProfile,
    configured: &BTreeMap<String, String>,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    windows: bool,
) -> Vec<(OsString, OsString)> {
    let defaults = DEFAULT_ENV
        .iter()
        .map(|name| env_key(name, windows))
        .collect::<BTreeSet<_>>();
    let explicit = profile
        .env_allow
        .iter()
        .map(|name| env_key(name.trim(), windows))
        .collect::<BTreeSet<_>>();
    let mut env = BTreeMap::new();
    for (key, value) in inherited {
        let Some(name) = key.to_str() else {
            continue;
        };
        let compared = env_key(name, windows);
        let listed = explicit.contains(&compared);
        let default = defaults.contains(&compared)
            || DEFAULT_ENV_PREFIXES
                .iter()
                .any(|prefix| compared.starts_with(&env_key(prefix, windows)));
        if !(listed || default) || is_core_credential(name, &value) {
            continue;
        }
        if !listed && plugins::subprocess::is_credential_env(&key, &value) {
            continue;
        }
        env.insert(compared, (key, value));
    }
    for (name, value) in configured {
        env.insert(
            env_key(name, windows),
            (OsString::from(name), OsString::from(value)),
        );
    }
    env.into_values().collect()
}

/// Sets `name=value` in `env`, replacing a variable of that name.
fn set_env(env: &mut Vec<(OsString, OsString)>, name: &str, value: &str, windows: bool) {
    let compared = env_key(name, windows);
    env.retain(|(key, _)| {
        key.to_str()
            .is_none_or(|key| env_key(key, windows) != compared)
    });
    env.push((OsString::from(name), OsString::from(value)));
}

/// The allowlist the server's proxy enforces (empty: no host), or `None` when it needs
/// no proxy: unsandboxed, any host, or no network on Linux under bubblewrap (which takes
/// the network away, so the loopback proxy would be out of reach anyway).
#[must_use]
pub fn mcp_proxy_allowlist(
    profile: &McpSandboxProfile,
    platform: &McpSandboxPlatform,
) -> Option<Vec<String>> {
    if profile.off {
        return None;
    }
    match &profile.network {
        McpSandboxNetwork::Any => None,
        McpSandboxNetwork::None
            if matches!(platform, McpSandboxPlatform::Linux { bwrap: Some(_) }) =>
        {
            None
        }
        McpSandboxNetwork::None => Some(Vec::new()),
        McpSandboxNetwork::Hosts(hosts) => Some(hosts.clone()),
    }
}

/// A loopback proxy for `entries`, one per distinct allowlist, started on first use
/// and kept for the life of the process (the bash tools' shared proxy is left alone).
pub fn mcp_proxy(entries: &[String]) -> io::Result<SocketAddr> {
    static PROXIES: OnceLock<Mutex<BTreeMap<Vec<String>, AllowlistProxy>>> = OnceLock::new();
    let mut key = entries
        .iter()
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    key.sort();
    key.dedup();
    let mut proxies = PROXIES
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(proxy) = proxies.get(&key) {
        return Ok(proxy.addr());
    }
    let proxy = AllowlistProxy::start(NetworkAllowlist::new(&key))?;
    let addr = proxy.addr();
    proxies.insert(key, proxy);
    Ok(addr)
}

/// How to start a server: the program and arguments (the server's own, or a sandbox
/// launcher wrapping them), its environment and its containment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpSandboxLaunch {
    pub program: String,
    pub args: Vec<String>,
    /// The child's whole environment; `None` for an unsandboxed server, which inherits
    /// this process's environment minus credentials, plus its `env` block.
    pub env: Option<Vec<(OsString, OsString)>>,
    /// Hold the server's process tree in a kill-on-close job object (Windows).
    pub contain: bool,
    /// What the profile allows, and what this platform does not enforce, for the server
    /// log.
    pub notes: Vec<String>,
}

/// The folders a sandboxed server may read and write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Roots {
    read: Vec<PathBuf>,
    write: Vec<PathBuf>,
    /// The temp folders: writable under seatbelt; bubblewrap gives a private `/tmp`.
    temp: Vec<PathBuf>,
    /// The server's own launcher caches, as (variable, folder), when file limits are
    /// enforced and it may not write the shared ones.
    private_caches: Vec<(&'static str, PathBuf)>,
    /// The folders of the program it runs, readable beyond its profile.
    program: Vec<PathBuf>,
    /// Write roots to create when missing (a bound folder must exist).
    create: Vec<PathBuf>,
}

/// The folder that holds a launched file: the outermost `node_modules` above it (a
/// package's dependencies sit beside it), else its folder.
fn program_root(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || !path.exists() {
        return None;
    }
    let mut outermost = None;
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if component.as_os_str() == "node_modules" && outermost.is_none() {
            outermost = Some(current.clone());
        }
    }
    outermost.or_else(|| {
        if path.is_dir() {
            Some(path.to_path_buf())
        } else {
            path.parent().map(Path::to_path_buf)
        }
    })
}

fn is_interpreter(command: &str) -> bool {
    Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| INTERPRETERS.contains(&stem.to_ascii_lowercase().as_str()))
}

/// The script an interpreter command runs: its first argument that is an existing file
/// with a script extension. Any other argument is data, not the program: a folder or a
/// key file named there must not become readable.
fn launched_script(command: &str, args: &[String]) -> Option<PathBuf> {
    if !is_interpreter(command) {
        return None;
    }
    args.iter()
        .map(Path::new)
        .find(|path| {
            path.is_absolute()
                && path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        SCRIPT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                    })
        })
        .map(Path::to_path_buf)
}

/// The installed npm package that holds `script`: the nearest folder above it with a
/// `package.json`, inside a `node_modules` folder.
fn package_dir(script: &Path) -> Option<PathBuf> {
    script
        .ancestors()
        .skip(1)
        .take_while(|dir| dir.file_name().is_some_and(|name| name != "node_modules"))
        .find(|dir| dir.join("package.json").is_file())
        .filter(|dir| {
            dir.components()
                .any(|component| component.as_os_str() == "node_modules")
        })
        .map(Path::to_path_buf)
}

/// The folders a stdio server's program is read from beyond its profile: the command's
/// folder when it names a file, and the folder of the script an interpreter runs (for a
/// package, the `node_modules` folder its dependencies sit in).
#[must_use]
pub fn mcp_program_roots(command: &str, args: &[String]) -> Vec<PathBuf> {
    let mut roots = program_root(Path::new(command))
        .into_iter()
        .chain(
            launched_script(command, args)
                .as_deref()
                .and_then(program_root),
        )
        .map(|root| normalize(&root))
        .collect::<Vec<_>>();
    roots.dedup();
    roots
}

/// A profile path, where `${package}` is the npm package of the script the server
/// runs (no such package: the entry is skipped).
fn expand_profile_path(
    host: &McpSandboxHost,
    entry: &str,
    package: Option<&Path>,
) -> Option<PathBuf> {
    let entry = entry.trim();
    match entry.strip_prefix("${package}") {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => {
            let rest = rest.trim_start_matches(['/', '\\']);
            // Resolved before the join: the folder named may not exist yet.
            let package = normalize(package?);
            Some(if rest.is_empty() {
                package
            } else {
                normalize(&package.join(rest))
            })
        }
        _ => host.expand(entry),
    }
}

/// Whether `network` reaches any of `hosts` over HTTPS.
fn network_reaches(network: &McpSandboxNetwork, hosts: &[&str]) -> bool {
    match network {
        McpSandboxNetwork::None => false,
        McpSandboxNetwork::Any => true,
        McpSandboxNetwork::Hosts(list) => {
            let allowlist = NetworkAllowlist::new(list);
            hosts.iter().any(|host| allowlist.allows(host, Some(443)))
        }
    }
}

/// The folder of a server's own launcher caches: under the OS cache folder (else temp),
/// named after its program and a hash of its command line, so it stays the same from
/// one start to the next.
fn private_cache_dir(transport: &McpStdioTransport, host: &McpSandboxHost) -> PathBuf {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let mut hasher = Sha256::new();
    hasher.update(transport.command.as_bytes());
    for arg in &transport.args {
        hasher.update([0]);
        hasher.update(arg.as_bytes());
    }
    let digest = hasher.finalize();
    let mut name = launched_script(&transport.command, &transport.args)
        .as_deref()
        .unwrap_or_else(|| Path::new(&transport.command))
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .filter(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.'))
        .take(32)
        .collect::<String>();
    if name.is_empty() {
        name.push_str("server");
    }
    name.push('-');
    for byte in &digest[..6] {
        let _ = write!(name, "{byte:02x}");
    }
    let base = host.cache.as_deref().unwrap_or(&host.temp);
    normalize(&base.join("neuron-mcp").join(name))
}

/// The roots of `transport`'s sandbox. `private_caches`: file limits are enforced, so a
/// server that may not reach a registry gets its own launcher cache instead of write
/// access to the shared one.
fn sandbox_roots(
    transport: &McpStdioTransport,
    host: &McpSandboxHost,
    private_caches: bool,
) -> Roots {
    let profile = &transport.sandbox;
    let script = launched_script(&transport.command, &transport.args);
    let package = script.as_deref().and_then(package_dir);
    let mut create = Vec::new();
    let mut write = Vec::new();
    for entry in &profile.fs_write {
        let Some(path) = expand_profile_path(host, entry, package.as_deref()) else {
            continue;
        };
        let in_home = host
            .home
            .as_ref()
            .is_some_and(|home| path.starts_with(home));
        if in_home || entry.trim().starts_with("${package}") {
            create.push(path.clone());
        }
        write.push(path);
    }
    let mut temp = vec![normalize(&host.temp)];
    if cfg!(target_os = "macos") {
        temp.push(PathBuf::from("/private/tmp"));
        temp.push(PathBuf::from("/private/var/folders"));
    }
    if cfg!(not(windows)) {
        temp.push(PathBuf::from("/tmp"));
    }
    let mut private = Vec::new();
    for cache in LAUNCHER_CACHES {
        if private_caches && !network_reaches(&profile.network, cache.registries) {
            let dir = private_cache_dir(transport, host).join(cache.private);
            create.push(dir.clone());
            write.push(dir.clone());
            private.push((cache.variable, dir));
            continue;
        }
        if let Some(home) = &host.home {
            write.extend(cache.home.iter().map(|dir| home.join(dir)));
        }
        if let Some(cache_dir) = &host.cache {
            write.extend(cache.cache.iter().map(|dir| cache_dir.join(dir)));
        }
    }
    let mut read = profile
        .fs_read
        .iter()
        .filter_map(|entry| expand_profile_path(host, entry, package.as_deref()))
        .collect::<Vec<_>>();
    if let Some(home) = &host.home {
        read.extend(TOOLCHAIN_READ.iter().map(|dir| home.join(dir)));
    }
    let program = mcp_program_roots(&transport.command, &transport.args);
    read.extend(program.iter().cloned());
    let tidy = |paths: &mut Vec<PathBuf>| {
        paths.sort_by(|left, right| {
            left.components()
                .count()
                .cmp(&right.components().count())
                .then_with(|| left.cmp(right))
        });
        paths.dedup();
    };
    tidy(&mut write);
    tidy(&mut temp);
    read.retain(|path| !write.contains(path) && !temp.contains(path));
    tidy(&mut read);
    Roots {
        read,
        write,
        temp,
        private_caches: private,
        program,
        create,
    }
}

impl Roots {
    /// The server-log lines for what these roots add to the profile.
    fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();
        if let Some((_, dir)) = self.private_caches.first() {
            notes.push(format!(
                "sandbox: its own launcher cache in {} (it may not reach the registry, so it may not write the shared one)",
                dir.parent().unwrap_or(dir).display()
            ));
        }
        if !self.program.is_empty() {
            notes.push(format!(
                "sandbox: also readable as its program: {}",
                self.program
                    .iter()
                    .map(|root| root.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        notes
    }
}

/// Whether file limits are enforced on `platform` (so a private launcher cache helps).
fn enforces_files(platform: &McpSandboxPlatform) -> bool {
    matches!(
        platform,
        McpSandboxPlatform::MacOs { seatbelt: true } | McpSandboxPlatform::Linux { bwrap: Some(_) }
    )
}

/// The folders bubblewrap replaces with an empty one: `/tmp` (the server gets a private
/// one), and `/run/user`, `/run/dbus` and the runtime folder `runtime_dir` when they
/// exist. A path socket in them (the session and system bus, an ssh or gpg agent) is
/// reachable without a network namespace, and the session bus can start an unsandboxed
/// program (`systemd-run --user`).
fn bwrap_hidden_dirs(runtime_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut hidden = vec![PathBuf::from("/tmp")];
    let candidates = [Path::new("/run/user"), Path::new("/run/dbus")]
        .into_iter()
        .map(Path::to_path_buf)
        .chain(runtime_dir.map(Path::to_path_buf));
    for dir in candidates {
        if dir.is_absolute() && dir.is_dir() && !hidden.iter().any(|outer| dir.starts_with(outer)) {
            hidden.push(dir);
        }
    }
    hidden
}

/// The sockets a server's environment grants it under bubblewrap, bound back read-only
/// (a read-only bind still lets it connect): the X server's folder for `DISPLAY`, the
/// `XAUTHORITY` file and the runtime folder when `envAllow` passes `XDG_RUNTIME_DIR`.
fn granted_socket_roots(env: &[(OsString, OsString)]) -> Vec<PathBuf> {
    let value = |name: &str| {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| PathBuf::from(value))
            .filter(|path| !path.as_os_str().is_empty())
    };
    let mut roots = Vec::new();
    if value("DISPLAY").is_some() {
        roots.push(PathBuf::from("/tmp/.X11-unix"));
    }
    roots.extend(
        ["XAUTHORITY", "XDG_RUNTIME_DIR"]
            .into_iter()
            .filter_map(value)
            .filter(|path| path.is_absolute()),
    );
    roots
}

/// A seatbelt profile for an MCP server: file contents in `home` unreadable except under
/// the read and write roots, writes only under the write roots, and outbound network as
/// `network` says. Later rules win in SBPL, so each allow comes after its deny.
#[must_use]
pub fn mcp_seatbelt_profile(
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    home: Option<&Path>,
    network: SeatbeltNetwork,
) -> String {
    use std::fmt::Write as _;
    let subpaths = |roots: &[PathBuf]| {
        roots
            .iter()
            .map(|root| format!("(subpath {})", sbpl_string(&root.to_string_lossy())))
            .collect::<Vec<_>>()
    };
    let mut profile = String::from("(version 1)\n(allow default)\n");
    let mut writable = vec![
        "(literal \"/dev/null\")".to_string(),
        "(literal \"/dev/zero\")".to_string(),
        "(literal \"/dev/dtracehelper\")".to_string(),
        "(regex #\"^/dev/tty\")".to_string(),
        "(subpath \"/dev/fd\")".to_string(),
    ];
    writable.extend(subpaths(write_roots));
    let _ = writeln!(
        profile,
        "(deny file-write* (require-not (require-any {})))",
        writable.join(" ")
    );
    if let Some(home) = home {
        let _ = writeln!(
            profile,
            "(deny file-read-data (subpath {}))",
            sbpl_string(&home.to_string_lossy())
        );
        let readable = subpaths(
            &read_roots
                .iter()
                .chain(write_roots)
                .filter(|root| root.starts_with(home))
                .cloned()
                .collect::<Vec<_>>(),
        );
        if !readable.is_empty() {
            let _ = writeln!(profile, "(allow file-read-data {})", readable.join(" "));
        }
    }
    match network {
        SeatbeltNetwork::Allow => {}
        SeatbeltNetwork::Deny => profile.push_str("(deny network-outbound (remote ip))\n"),
        SeatbeltNetwork::ProxyOnly(port) => {
            profile.push_str("(deny network-outbound (remote ip))\n");
            let _ = writeln!(
                profile,
                "(allow network-outbound (remote ip \"localhost:{port}\"))"
            );
        }
    }
    profile
}

/// The bubblewrap arguments before `--` and the server's command: a read-only root,
/// devices, an empty home and an empty folder over each of `hidden`
/// ([`bwrap_hidden_dirs`]), the read roots bound back read-only, the write roots
/// writable, and no network namespace when `network` is none. The server starts in the
/// workspace, or in `/` when the workspace is in a hidden folder.
///
/// No `--die-with-parent`: its signal (`PR_SET_PDEATHSIG`) fires when the thread that
/// started bubblewrap ends, and MCP servers are started from short-lived threads.
#[must_use]
pub fn mcp_bwrap_args(
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    hidden: &[PathBuf],
    home: Option<&Path>,
    workspace: &Path,
    no_network: bool,
) -> Vec<String> {
    let mut args = [
        "--new-session",
        "--ro-bind",
        "/",
        "/",
        "--dev-bind",
        "/dev",
        "/dev",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();
    for dir in home.into_iter().chain(hidden.iter().map(PathBuf::as_path)) {
        args.push("--tmpfs".to_string());
        args.push(dir.to_string_lossy().into_owned());
    }
    for (flag, roots) in [("--ro-bind-try", read_roots), ("--bind-try", write_roots)] {
        for root in roots {
            let root = root.to_string_lossy().into_owned();
            args.extend([flag.to_string(), root.clone(), root]);
        }
    }
    if no_network {
        args.push("--unshare-net".to_string());
    }
    let visible = !home
        .into_iter()
        .chain(hidden.iter().map(PathBuf::as_path))
        .any(|dir| workspace.starts_with(dir))
        || read_roots
            .iter()
            .chain(write_roots)
            .any(|root| workspace.starts_with(root));
    args.push("--chdir".to_string());
    args.push(if visible {
        workspace.to_string_lossy().into_owned()
    } else {
        "/".to_string()
    });
    args
}

/// How to start `transport` on `platform`, given the inherited environment and the
/// proxy for its network allowlist (see [`mcp_proxy_allowlist`]).
#[must_use]
pub fn plan_mcp_launch(
    transport: &McpStdioTransport,
    host: &McpSandboxHost,
    platform: &McpSandboxPlatform,
    inherited: impl IntoIterator<Item = (OsString, OsString)>,
    proxy: Option<SocketAddr>,
) -> McpSandboxLaunch {
    let profile = &transport.sandbox;
    if profile.off {
        return McpSandboxLaunch {
            program: transport.command.clone(),
            args: transport.args.clone(),
            env: None,
            contain: true,
            notes: vec!["sandbox: off".to_string()],
        };
    }
    let windows = *platform == McpSandboxPlatform::Windows;
    let inherited = inherited.into_iter().collect::<Vec<_>>();
    let runtime_dir = inherited
        .iter()
        .find(|(key, _)| key == "XDG_RUNTIME_DIR")
        .map(|(_, value)| PathBuf::from(value));
    let mut env = mcp_sandbox_env(profile, &transport.env, inherited, windows);
    if let Some(proxy) = proxy {
        for (name, value) in proxy_env(proxy) {
            set_env(&mut env, &name, &value, windows);
        }
        set_env(&mut env, "NODE_USE_ENV_PROXY", "1", windows);
    }
    let mut notes = vec![format!("sandbox: {}", describe_mcp_sandbox(profile))];
    let roots = sandbox_roots(transport, host, enforces_files(platform));
    for (variable, dir) in &roots.private_caches {
        set_env(&mut env, variable, &dir.to_string_lossy(), windows);
    }
    notes.extend(roots.notes());
    let no_network = profile.network == McpSandboxNetwork::None;
    let wrapped = |program: String, mut args: Vec<String>| {
        args.push(transport.command.clone());
        args.extend(transport.args.iter().cloned());
        (program, args)
    };
    let (program, args) = match platform {
        McpSandboxPlatform::MacOs { seatbelt: true } => {
            let network = match (&profile.network, proxy) {
                (McpSandboxNetwork::Any, _) => SeatbeltNetwork::Allow,
                (McpSandboxNetwork::Hosts(_), Some(proxy)) => {
                    SeatbeltNetwork::ProxyOnly(proxy.port())
                }
                _ => SeatbeltNetwork::Deny,
            };
            let mut write = roots.write.clone();
            write.extend(roots.temp.iter().cloned());
            let text = mcp_seatbelt_profile(&roots.read, &write, host.home.as_deref(), network);
            wrapped(SANDBOX_EXEC.to_string(), vec!["-p".to_string(), text])
        }
        McpSandboxPlatform::Linux { bwrap: Some(bwrap) } => {
            let mut read = roots.read.clone();
            read.extend(granted_socket_roots(&env));
            // Its own `/tmp`: the inherited TMPDIR may be a folder it cannot see.
            set_env(&mut env, "TMPDIR", "/tmp", windows);
            let mut args = mcp_bwrap_args(
                &read,
                &roots.write,
                &bwrap_hidden_dirs(runtime_dir.as_deref()),
                host.home.as_deref(),
                &host.workspace,
                no_network,
            );
            args.push("--".to_string());
            wrapped(bwrap.to_string_lossy().into_owned(), args)
        }
        McpSandboxPlatform::Windows => {
            notes.push(
                "sandbox: Windows contains the process tree; file limits are not enforced"
                    .to_string(),
            );
            (transport.command.clone(), transport.args.clone())
        }
        McpSandboxPlatform::Linux { bwrap: None } => {
            notes.push(
                "sandbox: bubblewrap (bwrap) is not installed, so file and network limits are not enforced"
                    .to_string(),
            );
            (transport.command.clone(), transport.args.clone())
        }
        McpSandboxPlatform::MacOs { seatbelt: false } | McpSandboxPlatform::Other => {
            notes.push("sandbox: file and network limits are not enforced here".to_string());
            (transport.command.clone(), transport.args.clone())
        }
    };
    let enforced_network = matches!(platform, McpSandboxPlatform::MacOs { seatbelt: true })
        || (no_network && matches!(platform, McpSandboxPlatform::Linux { bwrap: Some(_) }));
    if proxy.is_some() && !enforced_network {
        notes.push(
            "sandbox: the network limit is a proxy setting, which a program may ignore".to_string(),
        );
    }
    McpSandboxLaunch {
        program,
        args,
        env: Some(env),
        contain: true,
        notes,
    }
}

/// [`plan_mcp_launch`] for this machine: starts the proxy the profile needs and creates
/// missing write roots in the home folder, its own launcher caches and `${package}`
/// folders (a bound folder must exist).
pub fn prepare_mcp_launch(transport: &McpStdioTransport) -> io::Result<McpSandboxLaunch> {
    let platform = McpSandboxPlatform::detect();
    let host = McpSandboxHost::current();
    let proxy = match mcp_proxy_allowlist(&transport.sandbox, &platform) {
        Some(entries) => Some(mcp_proxy(&entries).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("the MCP sandbox could not start its network proxy: {error}"),
            )
        })?),
        None => None,
    };
    if !transport.sandbox.off && platform != McpSandboxPlatform::Windows {
        let roots = sandbox_roots(transport, &host, enforces_files(&platform));
        let in_home = |root: &Path| {
            host.home
                .as_ref()
                .is_some_and(|home| root.starts_with(home))
        };
        for root in roots
            .write
            .iter()
            .filter(|root| in_home(root))
            .chain(&roots.create)
        {
            if !root.exists() {
                let _ = std::fs::create_dir_all(root);
            }
        }
    }
    Ok(plan_mcp_launch(
        transport,
        &host,
        &platform,
        std::env::vars_os(),
        proxy,
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};

    use super::{
        mcp_bwrap_args, mcp_proxy_allowlist, mcp_sandbox_env, mcp_seatbelt_profile,
        plan_mcp_launch, resolve_mcp_sandbox, McpSandboxHost, McpSandboxOrigin, McpSandboxPlatform,
    };
    use crate::config::{
        ConfigSource, McpSandboxNetwork, McpSandboxProfile, McpServerConfig, McpStdioServerConfig,
    };
    use crate::mcp_catalog::McpCatalog;
    use crate::mcp_client::McpStdioTransport;
    use crate::sandbox::SeatbeltNetwork;

    fn stdio(command: &str, args: &[&str], sandbox: Option<McpSandboxProfile>) -> McpServerConfig {
        McpServerConfig::Stdio(McpStdioServerConfig {
            command: command.to_string(),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            env: BTreeMap::new(),
            tool_call_timeout_ms: None,
            sandbox,
        })
    }

    fn host() -> McpSandboxHost {
        McpSandboxHost {
            workspace: PathBuf::from("/work/repo"),
            home: Some(PathBuf::from("/home/ada")),
            cache: Some(PathBuf::from("/home/ada/.cache")),
            temp: PathBuf::from("/tmp"),
        }
    }

    fn env_pairs(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    fn env_map(env: &[(OsString, OsString)]) -> BTreeMap<String, String> {
        env.iter()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn an_unvetted_server_gets_the_workspace_read_only_and_no_network() {
        let (profile, origin) = resolve_mcp_sandbox(
            "mine",
            Some(ConfigSource::User),
            &stdio("node", &["server.js"], None),
        );
        assert_eq!(origin, McpSandboxOrigin::Unvetted);
        assert_eq!(profile.fs_read, vec!["."]);
        assert_eq!(profile.fs_write, [] as [std::string::String; 0]);
        assert_eq!(profile.network, McpSandboxNetwork::None);
        assert!(!profile.off);
    }

    #[test]
    fn a_catalog_server_gets_its_entry_profile_and_a_bundled_one_its_namesake() {
        let catalog = McpCatalog::builtin();
        let entry = catalog.get("playwright").expect("playwright entry");
        let (command, args) = entry.stdio_command().expect("stdio entry");
        let args = args.iter().map(String::as_str).collect::<Vec<_>>();
        let (profile, origin) = resolve_mcp_sandbox(
            "browser",
            Some(ConfigSource::User),
            &stdio(&command, &args, None),
        );
        assert_eq!(origin, McpSandboxOrigin::Catalog("playwright".to_string()));
        assert_eq!(profile.network, McpSandboxNetwork::Any);

        // A bundled server started from the locally installed entry script.
        let (profile, origin) = resolve_mcp_sandbox(
            "codebase-memory",
            Some(ConfigSource::Bundled),
            &stdio(
                "node",
                &["/opt/neuron/node_modules/codebase-memory-mcp/bin.js"],
                None,
            ),
        );
        assert_eq!(
            origin,
            McpSandboxOrigin::Catalog("codebase-memory".to_string())
        );
        assert_eq!(profile.fs_read, vec!["."]);
        assert!(matches!(profile.network, McpSandboxNetwork::Hosts(_)));

        // The same name from user settings is not the bundled server.
        let (_, origin) = resolve_mcp_sandbox(
            "codebase-memory",
            Some(ConfigSource::User),
            &stdio("node", &["/elsewhere/bin.js"], None),
        );
        assert_eq!(origin, McpSandboxOrigin::Unvetted);
    }

    #[test]
    fn a_server_setting_wins_but_workspace_settings_cannot_turn_the_sandbox_off() {
        let custom = McpSandboxProfile {
            network: McpSandboxNetwork::Hosts(vec!["api.example.com".to_string()]),
            ..McpSandboxProfile::default()
        };
        let (profile, origin) = resolve_mcp_sandbox(
            "mine",
            Some(ConfigSource::Project),
            &stdio("node", &["s.js"], Some(custom.clone())),
        );
        assert_eq!((profile, origin), (custom, McpSandboxOrigin::Settings));

        let off = Some(McpSandboxProfile::unsandboxed());
        let (profile, _) = resolve_mcp_sandbox(
            "mine",
            Some(ConfigSource::User),
            &stdio("node", &[], off.clone()),
        );
        assert!(profile.off, "user settings may turn it off");
        for scope in [ConfigSource::Project, ConfigSource::Local] {
            let (profile, origin) =
                resolve_mcp_sandbox("mine", Some(scope), &stdio("node", &[], off.clone()));
            assert!(!profile.off, "{scope:?} cannot turn it off");
            assert_eq!(origin, McpSandboxOrigin::Unvetted);
        }
    }

    #[test]
    fn the_environment_is_an_allowlist_that_never_passes_zero_x_credentials() {
        let inherited = env_pairs(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/home/ada"),
            ("LC_ALL", "C"),
            ("AWS_PROFILE", "prod"),
            ("SOME_SETTING", "x"),
            ("GITHUB_TOKEN", "ghp_secret"),
            ("NEURON_SESSION_TOKEN", "ses_abc"),
            ("OPENAI_API_KEY", "sk-1"),
            ("DISPLAY", ":0"),
            ("LEAKY", "nzx_key"),
        ]);
        let profile = McpSandboxProfile {
            env_allow: vec![
                "DISPLAY".to_string(),
                "GITHUB_TOKEN".to_string(),
                "NEURON_SESSION_TOKEN".to_string(),
                "LEAKY".to_string(),
            ],
            ..McpSandboxProfile::default()
        };
        let configured = BTreeMap::from([("API_URL".to_string(), "https://x".to_string())]);
        let env = env_map(&mcp_sandbox_env(
            &profile,
            &configured,
            inherited.clone(),
            false,
        ));
        assert_eq!(env.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert_eq!(env.get("HOME").map(String::as_str), Some("/home/ada"));
        assert_eq!(env.get("LC_ALL").map(String::as_str), Some("C"));
        assert_eq!(env.get("DISPLAY").map(String::as_str), Some(":0"));
        assert_eq!(env.get("API_URL").map(String::as_str), Some("https://x"));
        assert_eq!(
            env.get("GITHUB_TOKEN").map(String::as_str),
            Some("ghp_secret"),
            "a secret the profile names passes"
        );
        for absent in [
            "AWS_PROFILE",
            "SOME_SETTING",
            "NEURON_SESSION_TOKEN",
            "OPENAI_API_KEY",
            "LEAKY",
        ] {
            assert!(!env.contains_key(absent), "{absent} must not pass: {env:?}");
        }

        // Without the allowance a secret-looking variable stays out.
        let env = env_map(&mcp_sandbox_env(
            &McpSandboxProfile::default(),
            &BTreeMap::new(),
            inherited,
            false,
        ));
        assert!(!env.contains_key("GITHUB_TOKEN"));
        assert!(!env.contains_key("DISPLAY"));
    }

    #[test]
    fn windows_variable_names_match_without_case() {
        let inherited = env_pairs(&[("Path", "C:\\Windows"), ("SYSTEMROOT", "C:\\Windows")]);
        let configured = BTreeMap::from([("path".to_string(), "C:\\mine".to_string())]);
        let env = mcp_sandbox_env(&McpSandboxProfile::default(), &configured, inherited, true);
        let env = env_map(&env);
        assert_eq!(
            env.get("SYSTEMROOT").map(String::as_str),
            Some("C:\\Windows")
        );
        assert_eq!(
            env.len(),
            2,
            "the configured path replaces the inherited one: {env:?}"
        );
        assert_eq!(env.get("path").map(String::as_str), Some("C:\\mine"));
    }

    #[test]
    fn profile_paths_expand_against_the_workspace_home_and_cache() {
        let host = McpSandboxHost {
            workspace: PathBuf::from("C:\\work\\repo"),
            home: Some(PathBuf::from("C:\\Users\\ada")),
            cache: Some(PathBuf::from("C:\\Users\\ada\\AppData\\Local")),
            temp: PathBuf::from("C:\\Temp"),
        };
        if cfg!(windows) {
            assert_eq!(host.expand("."), Some(PathBuf::from("C:\\work\\repo")));
            assert_eq!(
                host.expand("docs/../src"),
                Some(PathBuf::from("C:\\work\\repo\\src"))
            );
            assert_eq!(host.expand("~"), Some(PathBuf::from("C:\\Users\\ada")));
            assert_eq!(
                host.expand("~/.cache/x"),
                Some(PathBuf::from("C:\\Users\\ada\\.cache\\x"))
            );
            assert_eq!(
                host.expand("${cache}/ms-playwright"),
                Some(PathBuf::from(
                    "C:\\Users\\ada\\AppData\\Local\\ms-playwright"
                ))
            );
            assert_eq!(host.expand("${temp}"), Some(PathBuf::from("C:\\Temp")));
        }
        assert_eq!(host.expand("  "), None);
        assert_eq!(host.expand("~other"), Some(host.workspace.join("~other")));
        let homeless = McpSandboxHost {
            home: None,
            cache: None,
            ..host
        };
        assert_eq!(homeless.expand("~/.ssh"), None);
        assert_eq!(homeless.expand("${cache}/x"), None);
    }

    #[test]
    fn only_a_limited_network_gets_a_proxy() {
        let windows = McpSandboxPlatform::Windows;
        let linux = McpSandboxPlatform::Linux {
            bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
        };
        let none = McpSandboxProfile::default();
        assert_eq!(mcp_proxy_allowlist(&none, &windows), Some(Vec::new()));
        assert_eq!(mcp_proxy_allowlist(&none, &linux), None, "bwrap cuts it");
        let hosts = McpSandboxProfile {
            network: McpSandboxNetwork::Hosts(vec!["github.com".to_string()]),
            ..McpSandboxProfile::default()
        };
        assert_eq!(
            mcp_proxy_allowlist(&hosts, &linux),
            Some(vec!["github.com".to_string()])
        );
        let any = McpSandboxProfile {
            network: McpSandboxNetwork::Any,
            ..McpSandboxProfile::default()
        };
        assert_eq!(mcp_proxy_allowlist(&any, &windows), None);
        assert_eq!(
            mcp_proxy_allowlist(&McpSandboxProfile::unsandboxed(), &windows),
            None
        );
    }

    fn transport(sandbox: McpSandboxProfile) -> McpStdioTransport {
        McpStdioTransport {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            env: BTreeMap::from([("FROM_CONFIG".to_string(), "1".to_string())]),
            tool_call_timeout_ms: None,
            sandbox,
        }
    }

    #[test]
    fn a_windows_launch_is_contained_with_an_allowlisted_environment_and_the_proxy() {
        let proxy: SocketAddr = "127.0.0.1:4100".parse().unwrap();
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::Windows,
            env_pairs(&[
                ("Path", "C:\\bin"),
                ("SECRETISH", "x"),
                ("HTTPS_PROXY", "corp"),
            ]),
            Some(proxy),
        );
        assert_eq!(launch.program, "node");
        assert_eq!(launch.args, vec!["server.js"]);
        assert!(launch.contain);
        let env = env_map(launch.env.as_deref().expect("a whole environment"));
        assert_eq!(env.get("Path").map(String::as_str), Some("C:\\bin"));
        assert_eq!(env.get("FROM_CONFIG").map(String::as_str), Some("1"));
        assert!(!env.contains_key("SECRETISH"));
        let proxies = env
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case("HTTPS_PROXY"))
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            proxies,
            ["http://127.0.0.1:4100"],
            "the sandbox proxy replaces an inherited one"
        );
        assert!(launch
            .notes
            .iter()
            .any(|note| note.contains("file limits are not enforced")));
    }

    #[test]
    fn an_unsandboxed_launch_keeps_the_command_and_its_containment() {
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::unsandboxed()),
            &host(),
            &McpSandboxPlatform::Linux {
                bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
            },
            Vec::new(),
            None,
        );
        assert_eq!(launch.program, "node");
        assert_eq!(launch.env, None);
        assert!(launch.contain);
    }

    #[test]
    fn a_linux_launch_wraps_the_server_in_bubblewrap() {
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::Linux {
                bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
            },
            Vec::new(),
            None,
        );
        assert_eq!(launch.program, "/usr/bin/bwrap");
        let separator = launch.args.iter().position(|arg| arg == "--").unwrap();
        assert_eq!(&launch.args[separator + 1..], ["node", "server.js"]);
        let before = &launch.args[..separator];
        assert!(before.contains(&"--unshare-net".to_string()));
        // Its signal would fire when the thread that started the server ends.
        assert!(!before.contains(&"--die-with-parent".to_string()));
        let joined = before.join(" ");
        assert!(joined.contains("--tmpfs /tmp"), "a private /tmp: {joined}");
        assert!(
            !joined.contains("--bind-try /tmp "),
            "the shared /tmp is not bound back: {joined}"
        );
        let env = env_map(launch.env.as_deref().unwrap());
        assert_eq!(env.get("TMPDIR").map(String::as_str), Some("/tmp"));

        let missing = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::Linux { bwrap: None },
            Vec::new(),
            None,
        );
        assert_eq!(missing.program, "node");
        assert!(missing.notes.iter().any(|note| note.contains("bubblewrap")));
    }

    #[test]
    fn bubblewrap_hides_the_home_folder_and_binds_back_only_the_roots() {
        let read = [
            PathBuf::from("/home/ada/repo"),
            PathBuf::from("/home/ada/.nvm"),
        ];
        let write = [PathBuf::from("/home/ada/.npm")];
        let hidden = [
            PathBuf::from("/tmp"),
            PathBuf::from("/run/user"),
            PathBuf::from("/run/dbus"),
        ];
        let args = mcp_bwrap_args(
            &read,
            &write,
            &hidden,
            Some(Path::new("/home/ada")),
            Path::new("/home/ada/repo"),
            false,
        );
        let joined = args.join(" ");
        assert!(
            joined.starts_with(
                "--new-session --ro-bind / / --dev-bind /dev /dev --tmpfs /home/ada --tmpfs /tmp --tmpfs /run/user --tmpfs /run/dbus --ro-bind-try"
            ),
            "the hidden folders come before the roots bound back: {joined}"
        );
        assert!(!joined.contains("--die-with-parent"));
        assert!(joined.contains("--ro-bind-try /home/ada/repo /home/ada/repo"));
        assert!(joined.contains("--bind-try /home/ada/.npm /home/ada/.npm"));
        assert!(joined.ends_with("--chdir /home/ada/repo"));
        assert!(!joined.contains("--unshare-net"));
        assert!(!joined.contains(".ssh"));

        // A server that may not read the workspace does not start in it.
        let args = mcp_bwrap_args(
            &[],
            &write,
            &hidden,
            Some(Path::new("/home/ada")),
            Path::new("/home/ada/repo"),
            true,
        );
        let joined = args.join(" ");
        assert!(joined.ends_with("--unshare-net --chdir /"), "{joined}");

        // Nor in a workspace under the private /tmp it cannot read.
        let args = mcp_bwrap_args(&[], &write, &hidden, None, Path::new("/tmp/repo"), true);
        assert!(args.join(" ").ends_with("--chdir /"));
    }

    #[test]
    fn bubblewrap_hides_the_runtime_folder_and_binds_back_only_granted_sockets() {
        // The runtime folder (the session bus, agent sockets) is not a default variable.
        let env = env_map(&mcp_sandbox_env(
            &McpSandboxProfile::default(),
            &BTreeMap::new(),
            env_pairs(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("PATH", "/bin")]),
            false,
        ));
        assert!(!env.contains_key("XDG_RUNTIME_DIR"), "{env:?}");

        let hidden = super::bwrap_hidden_dirs(None);
        assert_eq!(hidden[0], PathBuf::from("/tmp"));
        for dir in &hidden[1..] {
            assert!(dir.is_dir(), "only existing folders are hidden: {dir:?}");
        }

        assert_eq!(
            super::granted_socket_roots(&env_pairs(&[("PATH", "/bin")])),
            [] as [std::path::PathBuf; 0]
        );
        let granted = super::granted_socket_roots(&env_pairs(&[
            ("DISPLAY", ":0"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]));
        assert!(granted.contains(&PathBuf::from("/tmp/.X11-unix")));
        if cfg!(not(windows)) {
            assert!(granted.contains(&PathBuf::from("/run/user/1000")));
        }
    }

    fn unique_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("neuron-mcp-sandbox-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("create the test folder");
        dir
    }

    #[test]
    fn an_unvetted_server_gets_its_own_launcher_cache_where_files_are_enforced() {
        let roots = super::sandbox_roots(&transport(McpSandboxProfile::default()), &host(), true);
        let home = PathBuf::from("/home/ada");
        for shared in [".npm", ".local/share/uv", ".cache/uv"] {
            assert!(
                !roots.write.contains(&home.join(shared)),
                "{shared} must not be writable: {:?}",
                roots.write
            );
        }
        assert!(!roots.write.contains(&home.join(".cache").join("uv")));
        let variables = roots
            .private_caches
            .iter()
            .map(|(variable, _)| *variable)
            .collect::<Vec<_>>();
        assert_eq!(variables, ["npm_config_cache", "UV_CACHE_DIR"]);
        for (_, dir) in &roots.private_caches {
            assert!(
                dir.starts_with(home.join(".cache").join("neuron-mcp")),
                "{dir:?}"
            );
            assert!(roots.write.contains(dir) && roots.create.contains(dir));
        }

        // The launch points npm and uv at those caches, on macOS and Linux.
        for platform in [
            McpSandboxPlatform::MacOs { seatbelt: true },
            McpSandboxPlatform::Linux {
                bwrap: Some(PathBuf::from("/usr/bin/bwrap")),
            },
        ] {
            let launch = plan_mcp_launch(
                &transport(McpSandboxProfile::default()),
                &host(),
                &platform,
                env_pairs(&[("npm_config_cache", "/home/ada/.npm")]),
                None,
            );
            let env = env_map(launch.env.as_deref().unwrap());
            assert_eq!(
                env.get("npm_config_cache").map(PathBuf::from),
                Some(roots.private_caches[0].1.clone()),
                "{platform:?}"
            );
            let writable_npm = launch.args.windows(2).any(|pair| {
                (pair[0] == "--bind-try" && Path::new(&pair[1]).ends_with(".npm"))
                    || pair[1]
                        .lines()
                        .any(|line| line.starts_with("(deny file-write*") && line.contains(".npm"))
            });
            assert!(!writable_npm, "{:?}", launch.args);
            assert!(launch
                .notes
                .iter()
                .any(|note| note.contains("launcher cache")));
        }

        // A server that may reach npm writes the shared npm cache, not the uv one.
        let npm = McpSandboxProfile {
            network: McpSandboxNetwork::Hosts(vec!["registry.npmjs.org".to_string()]),
            ..McpSandboxProfile::default()
        };
        let roots = super::sandbox_roots(&transport(npm), &host(), true);
        assert!(roots.write.contains(&home.join(".npm")));
        assert!(!roots.write.contains(&home.join(".local/share/uv")));
        assert_eq!(roots.private_caches.len(), 1);
        assert_eq!(roots.private_caches[0].0, "UV_CACHE_DIR");

        // Where file limits are not enforced nothing changes.
        let roots = super::sandbox_roots(&transport(McpSandboxProfile::default()), &host(), false);
        assert_eq!(roots.private_caches, [] as [(&str, std::path::PathBuf); 0]);
    }

    #[test]
    fn only_the_program_and_its_script_are_readable_beyond_the_profile() {
        let dir = unique_dir("program");
        let package = dir.join("node_modules").join("notes-mcp");
        std::fs::create_dir_all(package.join("dist")).unwrap();
        std::fs::write(package.join("package.json"), "{}").unwrap();
        let script = package.join("dist").join("index.js");
        std::fs::write(&script, "").unwrap();
        let secrets = dir.join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        let key = secrets.join("id_ed25519");
        std::fs::write(&key, "").unwrap();

        let args = vec![
            "--key".to_string(),
            key.to_string_lossy().into_owned(),
            script.to_string_lossy().into_owned(),
            dir.to_string_lossy().into_owned(),
        ];
        let roots = super::mcp_program_roots("node", &args);
        assert_eq!(roots, vec![super::normalize(&dir.join("node_modules"))]);

        // Not an interpreter: its arguments are data.
        assert_eq!(
            super::mcp_program_roots("my-server", &args),
            [] as [std::path::PathBuf; 0]
        );

        let mut server = transport(McpSandboxProfile::default());
        server.args = args;
        let launch = plan_mcp_launch(
            &server,
            &host(),
            &McpSandboxPlatform::Windows,
            Vec::new(),
            None,
        );
        assert!(
            launch
                .notes
                .iter()
                .any(|note| note.contains("readable as its program")
                    && note.contains("node_modules")),
            "{:?}",
            launch.notes
        );
        let roots = super::sandbox_roots(&server, &host(), true);
        assert!(!roots.read.iter().any(|root| root.ends_with("secrets")));
        assert!(!roots.read.contains(&super::normalize(&dir)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_package_path_names_the_package_of_the_script_the_server_runs() {
        let dir = unique_dir("package");
        let package = dir.join("node_modules").join("codebase-memory-mcp");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("package.json"), "{}").unwrap();
        let script = package.join("bin.js");
        std::fs::write(&script, "").unwrap();
        let profile = McpSandboxProfile {
            fs_write: vec!["${package}/bin".to_string()],
            ..McpSandboxProfile::default()
        };
        let mut server = transport(profile.clone());
        server.args = vec![script.to_string_lossy().into_owned()];
        let roots = super::sandbox_roots(&server, &host(), true);
        let bin = super::normalize(&package).join("bin");
        assert!(roots.write.contains(&bin), "{:?}", roots.write);
        assert!(roots.create.contains(&bin));

        // `npx` runs no script Neuron can see: the entry is skipped, not the workspace.
        let mut npx = transport(profile);
        npx.command = "npx".to_string();
        npx.args = vec!["-y".to_string(), "codebase-memory-mcp@0.11.0".to_string()];
        let roots = super::sandbox_roots(&npx, &host(), true);
        assert!(!roots
            .write
            .iter()
            .any(|root| root.to_string_lossy().contains("package")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_proxied_server_has_node_use_the_proxy() {
        let proxy: SocketAddr = "127.0.0.1:4100".parse().unwrap();
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::Windows,
            Vec::new(),
            Some(proxy),
        );
        let env = env_map(launch.env.as_deref().unwrap());
        assert_eq!(env.get("NODE_USE_ENV_PROXY").map(String::as_str), Some("1"));
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::Windows,
            Vec::new(),
            None,
        );
        assert!(!env_map(launch.env.as_deref().unwrap()).contains_key("NODE_USE_ENV_PROXY"));
    }

    #[test]
    fn the_seatbelt_profile_hides_home_contents_except_the_roots() {
        let profile = mcp_seatbelt_profile(
            &[
                PathBuf::from("/Users/ada/repo"),
                PathBuf::from("/opt/tools"),
            ],
            &[
                PathBuf::from("/private/tmp"),
                PathBuf::from("/Users/ada/.npm"),
            ],
            Some(Path::new("/Users/ada")),
            SeatbeltNetwork::ProxyOnly(4100),
        );
        let deny_read = profile
            .find("(deny file-read-data (subpath \"/Users/ada\"))")
            .unwrap();
        let allow_read = profile.find("(allow file-read-data").unwrap();
        assert!(
            deny_read < allow_read,
            "the allow comes after the deny:\n{profile}"
        );
        assert!(profile.contains("(subpath \"/Users/ada/repo\")"));
        assert!(profile.contains("(subpath \"/Users/ada/.npm\")"));
        assert!(
            !profile[allow_read..].contains("/opt/tools"),
            "outside home needs no allow"
        );
        assert!(profile.contains("(deny file-write* (require-not (require-any"));
        assert!(profile.contains("(allow network-outbound (remote ip \"localhost:4100\"))"));
    }

    #[test]
    fn a_macos_launch_runs_the_server_under_sandbox_exec() {
        let launch = plan_mcp_launch(
            &transport(McpSandboxProfile::default()),
            &host(),
            &McpSandboxPlatform::MacOs { seatbelt: true },
            Vec::new(),
            None,
        );
        assert_eq!(launch.program, super::SANDBOX_EXEC);
        assert_eq!(launch.args[0], "-p");
        assert!(launch.args[1].contains("(deny network-outbound (remote ip))"));
        assert_eq!(&launch.args[2..], ["node", "server.js"]);
    }

    #[test]
    fn the_bundled_servers_declare_what_they_need() {
        let catalog = McpCatalog::builtin();
        let playwright = McpSandboxProfile::from(&catalog.get("playwright").unwrap().sandbox);
        assert_eq!(playwright.network, McpSandboxNetwork::Any);
        assert!(playwright
            .fs_write
            .iter()
            .any(|path| path.contains("ms-playwright")));
        let memory = McpSandboxProfile::from(&catalog.get("codebase-memory").unwrap().sandbox);
        assert!(memory
            .fs_write
            .iter()
            .any(|path| path.contains("codebase-memory-mcp")));
        assert!(
            memory.fs_write.iter().any(|path| path == "${package}/bin"),
            "its bin.js installs the binary into its package's bin folder"
        );
        let McpSandboxNetwork::Hosts(hosts) = memory.network else {
            panic!("codebase-memory reaches listed hosts only");
        };
        assert!(hosts.contains(&"registry.npmjs.org".to_string()));
    }
}
