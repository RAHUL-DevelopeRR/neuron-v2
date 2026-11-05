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
//!   proxy that reaches none.
//! - Windows: a kill-on-close job object (`winjob`) holds the server and every process
//!   it starts, so stopping the server or ending the engine ends the whole tree (no
//!   orphan `node.exe`). File limits are not enforced (there is no restricted-token spawn
//!   path, see [`crate::os_sandbox`]) and the proxy is advice a program may ignore.
//! - macOS: a `sandbox-exec` profile. File contents in the home folder are unreadable
//!   except the declared roots and the toolchain folders (npm, uv, nvm, ...), writes are
//!   allowed only to the write roots and temp, and outbound connections go only to the
//!   proxy (a host list) or nowhere (no network).
//! - Linux: bubblewrap (`bwrap`) when it is installed: a read-only root, an empty home
//!   with the declared roots and toolchain folders bound back, the write roots and temp
//!   writable, no network namespace for no network, and `--die-with-parent`. Without
//!   `bwrap` only the environment allowlist and the proxy apply, and the server log says
//!   so.

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
    "XDG_RUNTIME_DIR",
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

/// Toolchain folders a server's launcher writes (`npx` and `uvx` caches).
const TOOLCHAIN_WRITE: &[&str] = &[".npm", ".local/share/uv"];

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

fn sandbox_roots(transport: &McpStdioTransport, host: &McpSandboxHost) -> Roots {
    let profile = &transport.sandbox;
    let mut write = profile
        .fs_write
        .iter()
        .filter_map(|entry| host.expand(entry))
        .collect::<Vec<_>>();
    write.push(normalize(&host.temp));
    if cfg!(target_os = "macos") {
        write.push(PathBuf::from("/private/tmp"));
        write.push(PathBuf::from("/private/var/folders"));
    }
    if cfg!(not(windows)) {
        write.push(PathBuf::from("/tmp"));
    }
    if let Some(home) = &host.home {
        write.extend(TOOLCHAIN_WRITE.iter().map(|dir| home.join(dir)));
    }
    if let Some(cache) = &host.cache {
        write.push(cache.join("uv"));
    }
    let mut read = profile
        .fs_read
        .iter()
        .filter_map(|entry| host.expand(entry))
        .collect::<Vec<_>>();
    if let Some(home) = &host.home {
        read.extend(TOOLCHAIN_READ.iter().map(|dir| home.join(dir)));
    }
    read.extend(
        std::iter::once(&transport.command)
            .chain(&transport.args)
            .filter_map(|word| program_root(Path::new(word))),
    );
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
    read.retain(|path| !write.contains(path));
    tidy(&mut read);
    Roots { read, write }
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
/// devices, an empty home with the read roots bound back read-only, the write roots
/// writable, and no network namespace when `network` is none. The server starts in the
/// workspace, or in `/` when the workspace is in the hidden home folder.
#[must_use]
pub fn mcp_bwrap_args(
    read_roots: &[PathBuf],
    write_roots: &[PathBuf],
    home: Option<&Path>,
    workspace: &Path,
    no_network: bool,
) -> Vec<String> {
    let mut args = [
        "--die-with-parent",
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
    if let Some(home) = home {
        args.push("--tmpfs".to_string());
        args.push(home.to_string_lossy().into_owned());
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
    let visible = home.is_none_or(|home| !workspace.starts_with(home))
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
    let mut env = mcp_sandbox_env(profile, &transport.env, inherited, windows);
    if let Some(proxy) = proxy {
        for (name, value) in proxy_env(proxy) {
            set_env(&mut env, &name, &value, windows);
        }
    }
    let mut notes = vec![format!("sandbox: {}", describe_mcp_sandbox(profile))];
    let roots = sandbox_roots(transport, host);
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
            let text =
                mcp_seatbelt_profile(&roots.read, &roots.write, host.home.as_deref(), network);
            wrapped(SANDBOX_EXEC.to_string(), vec!["-p".to_string(), text])
        }
        McpSandboxPlatform::Linux { bwrap: Some(bwrap) } => {
            let mut args = mcp_bwrap_args(
                &roots.read,
                &roots.write,
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
/// missing write roots in the home folder (a bound folder must exist).
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
        if let Some(home) = &host.home {
            for root in sandbox_roots(transport, &host).write {
                if root.starts_with(home) && !root.exists() {
                    let _ = std::fs::create_dir_all(&root);
                }
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
        assert!(profile.fs_write.is_empty());
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
        assert!(before.contains(&"--die-with-parent".to_string()));

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
        let write = [PathBuf::from("/tmp"), PathBuf::from("/home/ada/.npm")];
        let args = mcp_bwrap_args(
            &read,
            &write,
            Some(Path::new("/home/ada")),
            Path::new("/home/ada/repo"),
            false,
        );
        let joined = args.join(" ");
        assert!(joined.starts_with("--die-with-parent --new-session --ro-bind / / --dev-bind /dev /dev --tmpfs /home/ada"), "{joined}");
        assert!(joined.contains("--ro-bind-try /home/ada/repo /home/ada/repo"));
        assert!(joined.contains("--bind-try /home/ada/.npm /home/ada/.npm"));
        assert!(joined.ends_with("--chdir /home/ada/repo"));
        assert!(!joined.contains("--unshare-net"));
        assert!(!joined.contains(".ssh"));

        // A server that may not read the workspace does not start in it.
        let args = mcp_bwrap_args(
            &[],
            &write,
            Some(Path::new("/home/ada")),
            Path::new("/home/ada/repo"),
            true,
        );
        let joined = args.join(" ");
        assert!(joined.ends_with("--unshare-net --chdir /"), "{joined}");
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
        let McpSandboxNetwork::Hosts(hosts) = memory.network else {
            panic!("codebase-memory reaches listed hosts only");
        };
        assert!(hosts.contains(&"registry.npmjs.org".to_string()));
    }
}
