use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemIsolationMode {
    Off,
    #[default]
    WorkspaceOnly,
    AllowList,
}

impl FilesystemIsolationMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::WorkspaceOnly => "workspace-only",
            Self::AllowList => "allow-list",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxConfig {
    pub enabled: Option<bool>,
    pub namespace_restrictions: Option<bool>,
    pub network_isolation: Option<bool>,
    pub filesystem_mode: Option<FilesystemIsolationMode>,
    pub allowed_mounts: Vec<String>,
    /// `sandbox.network.allow`: hosts shell commands and `WebFetch` may reach
    /// (see [`crate::net_allow`]). `None`: no allowlist.
    #[serde(default)]
    pub network_allow: Option<Vec<String>>,
    /// `sandbox.maxProcesses`: Windows job active-process limit
    /// (default [`DEFAULT_MAX_PROCESSES`]).
    #[serde(default)]
    pub max_processes: Option<u32>,
    /// `sandbox.memoryLimitMb`: Windows job memory limit (default none).
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
}

/// Active-process limit of a sandboxed command's Windows job when
/// `sandbox.maxProcesses` is unset: room for parallel builds, not for a fork
/// bomb.
pub const DEFAULT_MAX_PROCESSES: u32 = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxRequest {
    pub enabled: bool,
    pub namespace_restrictions: bool,
    pub network_isolation: bool,
    pub filesystem_mode: FilesystemIsolationMode,
    pub allowed_mounts: Vec<String>,
    #[serde(default)]
    pub network_allow: Option<Vec<String>>,
    #[serde(default)]
    pub max_processes: Option<u32>,
    #[serde(default)]
    pub memory_limit_mb: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerEnvironment {
    pub in_container: bool,
    pub markers: Vec<String>,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SandboxStatus {
    pub enabled: bool,
    pub requested: SandboxRequest,
    pub supported: bool,
    /// Whether every requested protection is in effect and at least one is.
    pub active: bool,
    pub namespace_supported: bool,
    pub namespace_active: bool,
    pub network_supported: bool,
    pub network_active: bool,
    /// The filesystem mode in effect: `off` whenever `filesystem_active` is
    /// false (the requested mode is in `requested`).
    pub filesystem_mode: FilesystemIsolationMode,
    /// Whether commands get a per-workspace `HOME` and `TMPDIR` (see
    /// [`SandboxDirs`]). Never on Windows, which has no filesystem isolation.
    pub filesystem_active: bool,
    pub allowed_mounts: Vec<String>,
    pub in_container: bool,
    pub container_markers: Vec<String>,
    pub fallback_reason: Option<String>,
    /// Windows: commands run in a kill-on-close Job Object with process and
    /// memory limits.
    #[serde(default)]
    pub process_containment_supported: bool,
    #[serde(default)]
    pub process_containment_active: bool,
    /// macOS: commands run under a generated `sandbox-exec` profile.
    #[serde(default)]
    pub seatbelt_supported: bool,
    #[serde(default)]
    pub seatbelt_active: bool,
    /// The configured network allowlist, when one is set and the sandbox is on.
    #[serde(default)]
    pub network_allowlist: Option<Vec<String>>,
    /// How the allowlist reaches shell commands (see [`crate::net_allow`]).
    #[serde(default)]
    pub network_allowlist_enforcement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDetectionInputs<'a> {
    pub env_pairs: Vec<(String, String)>,
    pub dockerenv_exists: bool,
    pub containerenv_exists: bool,
    pub proc_1_cgroup: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxSandboxCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl SandboxConfig {
    #[must_use]
    pub fn resolve_request(
        &self,
        enabled_override: Option<bool>,
        namespace_override: Option<bool>,
        network_override: Option<bool>,
        filesystem_mode_override: Option<FilesystemIsolationMode>,
        allowed_mounts_override: Option<Vec<String>>,
    ) -> SandboxRequest {
        SandboxRequest {
            enabled: enabled_override.unwrap_or(self.enabled.unwrap_or(true)),
            namespace_restrictions: namespace_override
                .unwrap_or(self.namespace_restrictions.unwrap_or(true)),
            network_isolation: network_override.unwrap_or(self.network_isolation.unwrap_or(false)),
            filesystem_mode: filesystem_mode_override
                .or(self.filesystem_mode)
                .unwrap_or_default(),
            allowed_mounts: allowed_mounts_override.unwrap_or_else(|| self.allowed_mounts.clone()),
            network_allow: self.network_allow.clone(),
            max_processes: self.max_processes,
            memory_limit_mb: self.memory_limit_mb,
        }
    }
}

#[must_use]
pub fn detect_container_environment() -> ContainerEnvironment {
    let proc_1_cgroup = fs::read_to_string("/proc/1/cgroup").ok();
    detect_container_environment_from(SandboxDetectionInputs {
        // `env::vars()` panics on non-UTF-8 entries; skip them instead.
        env_pairs: crate::remote::utf8_env_vars().collect(),
        dockerenv_exists: Path::new("/.dockerenv").exists(),
        containerenv_exists: Path::new("/run/.containerenv").exists(),
        proc_1_cgroup: proc_1_cgroup.as_deref(),
    })
}

#[must_use]
pub fn detect_container_environment_from(
    inputs: SandboxDetectionInputs<'_>,
) -> ContainerEnvironment {
    let mut markers = Vec::new();
    if inputs.dockerenv_exists {
        markers.push("/.dockerenv".to_string());
    }
    if inputs.containerenv_exists {
        markers.push("/run/.containerenv".to_string());
    }
    for (key, value) in inputs.env_pairs {
        let normalized = key.to_ascii_lowercase();
        if matches!(
            normalized.as_str(),
            "container" | "docker" | "podman" | "kubernetes_service_host"
        ) && !value.is_empty()
        {
            markers.push(format!("env:{key}={value}"));
        }
    }
    if let Some(cgroup) = inputs.proc_1_cgroup {
        for needle in ["docker", "containerd", "kubepods", "podman", "libpod"] {
            if cgroup.contains(needle) {
                markers.push(format!("/proc/1/cgroup:{needle}"));
            }
        }
    }
    markers.sort();
    markers.dedup();
    ContainerEnvironment {
        in_container: !markers.is_empty(),
        markers,
    }
}

#[must_use]
pub fn resolve_sandbox_status(config: &SandboxConfig, cwd: &Path) -> SandboxStatus {
    let request = config.resolve_request(None, None, None, None, None);
    resolve_sandbox_status_for_request(&request, cwd)
}

#[must_use]
pub fn resolve_sandbox_status_for_request(request: &SandboxRequest, cwd: &Path) -> SandboxStatus {
    let namespace_supported = cfg!(target_os = "linux") && unshare_user_namespace_works();
    resolve_status(
        request,
        cwd,
        PlatformSupport {
            namespace: namespace_supported,
            filesystem: filesystem_isolation_supported(),
            seatbelt: cfg!(target_os = "macos") && Path::new(SANDBOX_EXEC).exists(),
            job_object: cfg!(windows),
        },
    )
}

/// The macOS seatbelt launcher.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Which OS mechanisms this machine has.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
struct PlatformSupport {
    namespace: bool,
    filesystem: bool,
    seatbelt: bool,
    job_object: bool,
}

/// Filesystem isolation (a per-workspace `HOME` and `TMPDIR`) is applied on
/// Unix only; Windows commands run unisolated.
const fn filesystem_isolation_supported() -> bool {
    !cfg!(windows)
}

#[cfg(test)]
fn resolve_sandbox_status_with_support(
    request: &SandboxRequest,
    cwd: &Path,
    namespace_supported: bool,
    filesystem_supported: bool,
) -> SandboxStatus {
    resolve_status(
        request,
        cwd,
        PlatformSupport {
            namespace: namespace_supported,
            filesystem: filesystem_supported,
            ..PlatformSupport::default()
        },
    )
}

#[allow(clippy::too_many_lines)]
fn resolve_status(request: &SandboxRequest, cwd: &Path, support: PlatformSupport) -> SandboxStatus {
    let namespace_supported = support.namespace;
    let filesystem_supported = support.filesystem;
    let container = detect_container_environment();
    let network_supported = namespace_supported || support.seatbelt;
    let filesystem_requested =
        request.enabled && request.filesystem_mode != FilesystemIsolationMode::Off;
    let filesystem_active = filesystem_requested && filesystem_supported;
    let namespace_active = request.enabled && request.namespace_restrictions && namespace_supported;
    let network_active = request.enabled && request.network_isolation && network_supported;
    let mut fallback_reasons = Vec::new();

    if request.enabled && request.namespace_restrictions && !namespace_supported {
        fallback_reasons
            .push("namespace isolation unavailable (requires Linux with `unshare`)".to_string());
    }
    if request.enabled && request.network_isolation && !network_supported {
        fallback_reasons.push(
            "network isolation unavailable (requires Linux `unshare` or macOS `sandbox-exec`)"
                .to_string(),
        );
    }
    if filesystem_requested && !filesystem_supported {
        fallback_reasons.push(format!(
            "filesystem isolation ({}) unavailable on this platform",
            request.filesystem_mode.as_str()
        ));
    }
    if filesystem_active
        && request.filesystem_mode == FilesystemIsolationMode::AllowList
        && request.allowed_mounts.is_empty()
    {
        fallback_reasons
            .push("filesystem allow-list requested without configured mounts".to_string());
    }

    let network_allowlist = request.network_allow.clone().filter(|_| request.enabled);
    let seatbelt_active = request.enabled
        && support.seatbelt
        && (filesystem_active || network_active || network_allowlist.is_some());
    let process_containment_active = request.enabled && support.job_object;
    let network_allowlist_enforcement = network_allowlist.as_ref().map(|_| {
        if seatbelt_active {
            "enforced: seatbelt allows only the allowlist proxy".to_string()
        } else if network_active {
            "blocked: network namespace has no network, allowlist not applied".to_string()
        } else {
            "advisory: proxy environment variables (programs may ignore them)".to_string()
        }
    });

    let active = request.enabled
        && (!request.namespace_restrictions || namespace_supported)
        && (!request.network_isolation || network_supported)
        && (!filesystem_requested || filesystem_supported)
        && (namespace_active
            || network_active
            || filesystem_active
            || seatbelt_active
            || process_containment_active);

    let allowed_mounts = normalize_mounts(&request.allowed_mounts, cwd);

    SandboxStatus {
        enabled: request.enabled,
        requested: request.clone(),
        supported: namespace_supported,
        active,
        namespace_supported,
        namespace_active,
        network_supported,
        network_active,
        filesystem_mode: if filesystem_active {
            request.filesystem_mode
        } else {
            FilesystemIsolationMode::Off
        },
        filesystem_active,
        allowed_mounts,
        in_container: container.in_container,
        container_markers: container.markers,
        fallback_reason: (!fallback_reasons.is_empty()).then(|| fallback_reasons.join("; ")),
        process_containment_supported: support.job_object,
        process_containment_active,
        seatbelt_supported: support.seatbelt,
        seatbelt_active,
        network_allowlist,
        network_allowlist_enforcement,
    }
}

/// Network policy of a seatbelt profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatbeltNetwork {
    /// No network rule.
    Allow,
    /// No outbound IP connections (local Unix sockets still work).
    Deny,
    /// Outbound IP only to the loopback allowlist proxy on this port.
    ProxyOnly(u16),
}

fn sbpl_string(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A seatbelt (SBPL) profile: everything allowed except writes outside
/// `write_roots` (plus `/dev/null`, ttys and `/dev/fd`) when `write_roots`
/// is `Some`, and outbound network as `network` says. Later rules win in
/// SBPL, so the allow-exceptions come after the denies.
#[must_use]
pub fn seatbelt_profile(write_roots: Option<&[PathBuf]>, network: SeatbeltNetwork) -> String {
    use std::fmt::Write as _;
    let mut profile = String::from("(version 1)\n(allow default)\n");
    if let Some(roots) = write_roots {
        let mut allowed = vec![
            "(literal \"/dev/null\")".to_string(),
            "(literal \"/dev/zero\")".to_string(),
            "(literal \"/dev/dtracehelper\")".to_string(),
            "(regex #\"^/dev/tty\")".to_string(),
            "(subpath \"/dev/fd\")".to_string(),
        ];
        for root in roots {
            allowed.push(format!(
                "(subpath {})",
                sbpl_string(&root.to_string_lossy())
            ));
        }
        let _ = writeln!(
            profile,
            "(deny file-write* (require-not (require-any {})))",
            allowed.join(" ")
        );
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

/// The directories a seatbelt-confined command may write: the workspace,
/// allowed mounts, the per-workspace sandbox `HOME`/`TMPDIR`, and the system
/// temp directories (macOS resolves `/tmp` and `$TMPDIR` to `/private/...`).
#[must_use]
pub fn seatbelt_write_roots(cwd: &Path, status: &SandboxStatus) -> Vec<PathBuf> {
    let canonical = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut roots = vec![canonical(cwd)];
    roots.extend(
        status
            .allowed_mounts
            .iter()
            .map(|mount| canonical(Path::new(mount))),
    );
    let dirs = SandboxDirs::for_workspace(cwd);
    roots.push(canonical(&dirs.home));
    roots.push(canonical(&dirs.tmp));
    roots.push(canonical(&env::temp_dir()));
    roots.push(PathBuf::from("/private/tmp"));
    roots.push(PathBuf::from("/private/var/folders"));
    roots.sort();
    roots.dedup();
    roots
}

/// `sandbox-exec -p <profile> /bin/sh -lc <command>` when the seatbelt is
/// active (macOS). `proxy_port` is the allowlist proxy, when an allowlist is
/// set.
#[must_use]
pub fn build_macos_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
    proxy_port: Option<u16>,
) -> Option<LinuxSandboxCommand> {
    if !status.seatbelt_active {
        return None;
    }
    let network = match (status.network_allowlist.is_some(), proxy_port) {
        (true, Some(port)) => SeatbeltNetwork::ProxyOnly(port),
        (true, None) => SeatbeltNetwork::Deny,
        (false, _) if status.network_active => SeatbeltNetwork::Deny,
        (false, _) => SeatbeltNetwork::Allow,
    };
    let roots = status
        .filesystem_active
        .then(|| seatbelt_write_roots(cwd, status));
    let profile = seatbelt_profile(roots.as_deref(), network);
    let mut env = Vec::new();
    if status.filesystem_active {
        let dirs = SandboxDirs::for_workspace(cwd);
        env.push(("HOME".to_string(), dirs.home.display().to_string()));
        env.push(("TMPDIR".to_string(), dirs.tmp.display().to_string()));
    }
    Some(LinuxSandboxCommand {
        program: SANDBOX_EXEC.to_string(),
        args: vec![
            "-p".to_string(),
            profile,
            "/bin/sh".to_string(),
            "-lc".to_string(),
            command.to_string(),
        ],
        env,
    })
}

#[must_use]
pub fn build_linux_sandbox_command(
    command: &str,
    cwd: &Path,
    status: &SandboxStatus,
) -> Option<LinuxSandboxCommand> {
    if !cfg!(target_os = "linux")
        || !status.enabled
        || (!status.namespace_active && !status.network_active)
    {
        return None;
    }

    let mut args = vec![
        "--user".to_string(),
        "--map-root-user".to_string(),
        "--mount".to_string(),
        "--ipc".to_string(),
        "--pid".to_string(),
        "--uts".to_string(),
        "--fork".to_string(),
    ];
    if status.network_active {
        args.push("--net".to_string());
    }
    args.push("sh".to_string());
    args.push("-lc".to_string());
    args.push(command.to_string());

    let mut env = Vec::new();
    if status.filesystem_active {
        let dirs = SandboxDirs::for_workspace(cwd);
        env.push(("HOME".to_string(), dirs.home.display().to_string()));
        env.push(("TMPDIR".to_string(), dirs.tmp.display().to_string()));
    }
    env.push((
        "CLAWD_SANDBOX_FILESYSTEM_MODE".to_string(),
        status.filesystem_mode.as_str().to_string(),
    ));
    env.push((
        "CLAWD_SANDBOX_ALLOWED_MOUNTS".to_string(),
        status.allowed_mounts.join(":"),
    ));
    if let Ok(path) = env::var("PATH") {
        env.push(("PATH".to_string(), path));
    }

    Some(LinuxSandboxCommand {
        program: "unshare".to_string(),
        args,
        env,
    })
}

/// The `HOME` and `TMPDIR` a sandboxed command gets while filesystem
/// isolation is active. They live outside the project, in the user's Neuron
/// config directory (`~/.neuron/sandbox/<workspace>-<hash>/{home,tmp}`), one
/// pair per workspace, so caches persist between commands without littering
/// the project. Not the shared system temp directory: another local user
/// could pre-create a predictable path there and plant a `.profile` for the
/// login shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxDirs {
    pub home: PathBuf,
    pub tmp: PathBuf,
}

impl SandboxDirs {
    #[must_use]
    pub fn for_workspace(cwd: &Path) -> Self {
        let config_home = crate::config::default_config_home();
        let base = if config_home.is_absolute() {
            config_home
        } else {
            env::temp_dir().join("neuron")
        };
        Self::for_workspace_in(&base.join("sandbox"), cwd)
    }

    /// Directories for `cwd` under `base`, keyed by a hash of the
    /// workspace's canonical path.
    #[must_use]
    pub fn for_workspace_in(base: &Path, cwd: &Path) -> Self {
        let workspace = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let digest = Sha256::digest(workspace.to_string_lossy().as_bytes());
        let mut prefix = [0_u8; 8];
        prefix.copy_from_slice(&digest[..8]);
        let hash = format!("{:016x}", u64::from_be_bytes(prefix));
        let name = workspace
            .file_name()
            .map(|name| {
                name.to_string_lossy()
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .take(32)
                    .collect::<String>()
            })
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "workspace".to_string());
        let root = base.join(format!("{name}-{hash}"));
        Self {
            home: root.join("home"),
            tmp: root.join("tmp"),
        }
    }

    /// Creates both directories (owner-only on Unix) if they are missing.
    pub fn create(&self) -> io::Result<()> {
        for dir in [&self.home, &self.tmp] {
            create_private_dir(dir)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

fn normalize_mounts(mounts: &[String], cwd: &Path) -> Vec<String> {
    let cwd = cwd.to_path_buf();
    mounts
        .iter()
        .map(|mount| {
            let path = PathBuf::from(mount);
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        })
        .map(|path| path.display().to_string())
        .collect()
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|path| path.join(command).exists()))
}

/// Check whether `unshare --user` actually works on this system.
/// On some CI environments (e.g. GitHub Actions), the binary exists but
/// user namespaces are restricted, causing silent failures.
fn unshare_user_namespace_works() -> bool {
    use std::sync::OnceLock;
    static RESULT: OnceLock<bool> = OnceLock::new();
    *RESULT.get_or_init(|| {
        if !command_exists("unshare") {
            return false;
        }
        std::process::Command::new("unshare")
            .args(["--user", "--map-root-user", "true"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_linux_sandbox_command, detect_container_environment_from,
        resolve_sandbox_status_with_support, FilesystemIsolationMode, SandboxConfig,
        SandboxDetectionInputs, SandboxDirs, SandboxRequest,
    };
    use std::path::Path;

    #[test]
    fn detects_container_markers_from_multiple_sources() {
        let detected = detect_container_environment_from(SandboxDetectionInputs {
            env_pairs: vec![("container".to_string(), "docker".to_string())],
            dockerenv_exists: true,
            containerenv_exists: false,
            proc_1_cgroup: Some("12:memory:/docker/abc"),
        });

        assert!(detected.in_container);
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/.dockerenv"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "env:container=docker"));
        assert!(detected
            .markers
            .iter()
            .any(|marker| marker == "/proc/1/cgroup:docker"));
    }

    #[test]
    fn resolves_request_with_overrides() {
        let config = SandboxConfig {
            enabled: Some(true),
            namespace_restrictions: Some(true),
            network_isolation: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: vec!["logs".to_string()],
            ..SandboxConfig::default()
        };

        let request = config.resolve_request(
            Some(true),
            Some(false),
            Some(true),
            Some(FilesystemIsolationMode::AllowList),
            Some(vec!["tmp".to_string()]),
        );

        assert!(request.enabled);
        assert!(!request.namespace_restrictions);
        assert!(request.network_isolation);
        assert_eq!(request.filesystem_mode, FilesystemIsolationMode::AllowList);
        assert_eq!(request.allowed_mounts, vec!["tmp"]);
    }

    #[test]
    fn builds_linux_launcher_with_network_flag_when_requested() {
        let config = SandboxConfig::default();
        let status = super::resolve_sandbox_status_for_request(
            &config.resolve_request(
                Some(true),
                Some(true),
                Some(true),
                Some(FilesystemIsolationMode::WorkspaceOnly),
                None,
            ),
            Path::new("/workspace"),
        );

        if let Some(launcher) =
            build_linux_sandbox_command("printf hi", Path::new("/workspace"), &status)
        {
            assert_eq!(launcher.program, "unshare");
            assert!(launcher.args.iter().any(|arg| arg == "--mount"));
            assert_eq!(
                launcher.args.iter().any(|arg| arg == "--net"),
                status.network_active
            );
            let home = launcher
                .env
                .iter()
                .find(|(key, _)| key == "HOME")
                .map(|(_, value)| value.clone())
                .expect("filesystem isolation sets HOME");
            assert!(
                !Path::new(&home).starts_with("/workspace"),
                "HOME must not be inside the project: {home}"
            );
        }
    }

    fn workspace_only_request() -> SandboxRequest {
        SandboxConfig::default().resolve_request(
            Some(true),
            Some(false),
            Some(false),
            Some(FilesystemIsolationMode::WorkspaceOnly),
            None,
        )
    }

    /// Windows applies no filesystem isolation, so the status must not claim
    /// a live workspace-only mode.
    #[test]
    fn filesystem_isolation_is_reported_inactive_where_unsupported() {
        let cwd = Path::new("/workspace");
        let unsupported =
            resolve_sandbox_status_with_support(&workspace_only_request(), cwd, false, false);
        assert!(!unsupported.filesystem_active);
        assert_eq!(unsupported.filesystem_mode, FilesystemIsolationMode::Off);
        assert_eq!(
            unsupported.requested.filesystem_mode,
            FilesystemIsolationMode::WorkspaceOnly
        );
        assert!(!unsupported.active, "nothing is isolated: {unsupported:?}");
        assert!(unsupported
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("filesystem isolation")));

        let supported =
            resolve_sandbox_status_with_support(&workspace_only_request(), cwd, false, true);
        assert!(supported.filesystem_active);
        assert_eq!(
            supported.filesystem_mode,
            FilesystemIsolationMode::WorkspaceOnly
        );
        assert!(supported.active);
        assert_eq!(supported.fallback_reason, None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_status_never_claims_filesystem_isolation() {
        let status =
            super::resolve_sandbox_status_for_request(&workspace_only_request(), Path::new("."));
        assert!(!status.filesystem_active, "{status:?}");
        assert_eq!(status.filesystem_mode, FilesystemIsolationMode::Off);
        assert!(!status.active, "{status:?}");
    }

    #[test]
    fn sandbox_dirs_live_outside_the_workspace_one_pair_per_workspace() {
        let base = Path::new("/home/dev/.neuron/sandbox");
        let first = SandboxDirs::for_workspace_in(base, Path::new("/work/project-a"));
        let again = SandboxDirs::for_workspace_in(base, Path::new("/work/project-a"));
        let other = SandboxDirs::for_workspace_in(base, Path::new("/work/project-b"));
        assert_eq!(first, again, "stable per workspace");
        assert_ne!(first.home, other.home);
        assert!(first.home.starts_with(base) && first.tmp.starts_with(base));
        assert!(!first.home.starts_with("/work/project-a"));
        let root = first.home.parent().expect("workspace root");
        assert_eq!(first.tmp.parent(), Some(root));
        assert!(root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("project-a-")));
    }

    fn support(seatbelt: bool, job_object: bool) -> super::PlatformSupport {
        super::PlatformSupport {
            namespace: false,
            filesystem: true,
            seatbelt,
            job_object,
        }
    }

    #[test]
    fn seatbelt_profile_confines_writes_and_network() {
        use super::{seatbelt_profile, SeatbeltNetwork};
        let roots = vec![
            std::path::PathBuf::from("/Users/dev/project"),
            std::path::PathBuf::from("/private/tmp"),
        ];
        let profile = seatbelt_profile(Some(&roots), SeatbeltNetwork::ProxyOnly(4567));
        assert!(profile.starts_with("(version 1)\n(allow default)\n"));
        assert!(
            profile.contains("(deny file-write* (require-not (require-any (literal \"/dev/null\")")
        );
        assert!(profile.contains("(subpath \"/Users/dev/project\")"));
        assert!(profile.contains("(subpath \"/private/tmp\")"));
        assert!(profile.contains("(deny network-outbound (remote ip))"));
        assert!(profile.contains("(allow network-outbound (remote ip \"localhost:4567\"))"));
        let deny_at = profile.find("(deny network-outbound").expect("deny");
        let allow_at = profile.find("(allow network-outbound").expect("allow");
        assert!(allow_at > deny_at, "the later rule wins in SBPL");

        let open = seatbelt_profile(None, SeatbeltNetwork::Allow);
        assert_eq!(open, "(version 1)\n(allow default)\n");
        let no_net = seatbelt_profile(None, SeatbeltNetwork::Deny);
        assert!(no_net.contains("(deny network-outbound (remote ip))"));
        assert!(!no_net.contains("allow network-outbound"));
    }

    #[test]
    fn seatbelt_paths_are_quoted_safely() {
        use super::{seatbelt_profile, SeatbeltNetwork};
        let roots = vec![std::path::PathBuf::from("/tmp/we\"ird\\dir")];
        let profile = seatbelt_profile(Some(&roots), SeatbeltNetwork::Allow);
        assert!(
            profile.contains(r#"(subpath "/tmp/we\"ird\\dir")"#),
            "{profile}"
        );
    }

    #[test]
    fn seatbelt_status_drives_the_macos_launcher() {
        let mut request = workspace_only_request();
        request.network_allow = Some(vec!["github.com".to_string()]);
        let cwd = Path::new("/work/project");
        let status = super::resolve_status(&request, cwd, support(true, false));
        assert!(status.seatbelt_active && status.active);
        assert!(status
            .network_allowlist_enforcement
            .as_deref()
            .is_some_and(|mode| mode.starts_with("enforced")));
        let launcher = super::build_macos_sandbox_command("echo hi", cwd, &status, Some(9000))
            .expect("seatbelt launcher");
        assert_eq!(launcher.program, super::SANDBOX_EXEC);
        assert_eq!(launcher.args[0], "-p");
        assert!(launcher.args[1].contains("localhost:9000"));
        assert_eq!(&launcher.args[2..], ["/bin/sh", "-lc", "echo hi"]);

        let without = super::resolve_status(&request, cwd, support(false, false));
        assert!(!without.seatbelt_active);
        assert!(super::build_macos_sandbox_command("echo", cwd, &without, None).is_none());
        assert!(without
            .network_allowlist_enforcement
            .as_deref()
            .is_some_and(|mode| mode.starts_with("advisory")));
    }

    #[test]
    fn job_objects_count_as_active_containment() {
        let mut request = workspace_only_request();
        request.filesystem_mode = FilesystemIsolationMode::Off;
        let status = super::resolve_status(&request, Path::new("."), support(false, true));
        assert!(status.process_containment_active);
        assert!(status.active, "{status:?}");
        request.enabled = false;
        let off = super::resolve_status(&request, Path::new("."), support(false, true));
        assert!(!off.process_containment_active && !off.active);
        assert_eq!(off.network_allowlist, None);
    }

    #[test]
    fn config_limits_and_allowlist_flow_into_the_request() {
        let config = SandboxConfig {
            network_allow: Some(vec!["*.npmjs.org".to_string()]),
            max_processes: Some(64),
            memory_limit_mb: Some(2048),
            ..SandboxConfig::default()
        };
        let request = config.resolve_request(None, None, None, None, None);
        assert_eq!(request.network_allow, Some(vec!["*.npmjs.org".to_string()]));
        assert_eq!(request.max_processes, Some(64));
        assert_eq!(request.memory_limit_mb, Some(2048));
    }

    #[test]
    fn sandbox_dirs_are_created_on_request() {
        let base = std::env::temp_dir().join(format!(
            "runtime-sandbox-dirs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos())
        ));
        let dirs = SandboxDirs::for_workspace_in(&base, Path::new("/work/project"));
        assert!(!dirs.home.exists());
        dirs.create().expect("create sandbox dirs");
        assert!(dirs.home.is_dir() && dirs.tmp.is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }
}
