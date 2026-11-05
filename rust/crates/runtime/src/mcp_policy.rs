//! Connector governance: which MCP servers an administrator's managed settings and the
//! Zero-X plan allow, refuse and require. Applied at the end of
//! [`ConfigLoader::load`](crate::ConfigLoader::load), after `disabledMcpServers` and
//! before the deny rules and workspace approvals.
//!
//! Two sources, the first one winning:
//! - **Managed settings**: `managed-settings.json` in a machine-wide folder
//!   ([`managed_settings_path`]), read only when no one but an administrator can have
//!   written it or its folders ([`check_managed_settings_owner`]). It loads last
//!   ([`ConfigSource::Managed`]), so no other settings file overrides it, and only there
//!   is `mcpPolicy` read ([`ManagedMcpPolicy`]).
//! - **The Zero-X plan**: `policy.connectors` in `GET /api/account` (contract v1.3),
//!   kept in `<config home>/policy-cache.json` ([`store_gateway_policy`]) so it applies
//!   on restart and offline. A cache older than [`GATEWAY_POLICY_TTL_SECS`] is still used,
//!   with a warning (fail open), unless the managed settings set `mcpPolicy.failClosed`:
//!   then every server the managed settings neither configure nor require is blocked until
//!   the account is read again. Without a cache (never signed in, or signed out) there is
//!   no plan policy.
//!
//! A refused server is listed as blocked, with the reason, and never starts. A required
//! connector is added with [`ConfigSource::Managed`] scope: it needs no workspace
//! approval, starts in one-shot runs too, and cannot be turned off or removed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config::{
    expect_object, optional_bool, optional_string_array, parse_mcp_server_json, BlockedMcpServer,
    ConfigError, ConfigSource, McpServerConfig, McpTransport, RuntimePermissionRuleConfig,
    ScopedMcpServerConfig,
};
use crate::json::JsonValue;
use crate::mcp_catalog::McpCatalog;
use crate::mcp_toggle::{read_object, write_object};

/// The managed settings file's name, in [`managed_settings_path`]'s folder.
pub const MANAGED_SETTINGS_FILE: &str = "managed-settings.json";

/// The plan's connector policy, cached in the config home.
pub const GATEWAY_POLICY_CACHE_FILE: &str = "policy-cache.json";

/// How long a cached plan policy counts as current (a day). An older one is still used,
/// with a warning, unless the managed settings fail closed.
pub const GATEWAY_POLICY_TTL_SECS: u64 = 24 * 60 * 60;

/// The system folder that holds the managed settings' own folder: Program Files on
/// Windows (as Windows reports it, not the `ProgramFiles` environment variable),
/// `/Library/Application Support` on macOS, `/etc` elsewhere. By default only an
/// administrator can add to it; [`check_managed_settings_owner`] verifies that before
/// the file is trusted.
#[must_use]
pub fn managed_settings_root() -> PathBuf {
    #[cfg(windows)]
    {
        winsec::program_files_dir().unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
    }
    #[cfg(not(windows))]
    {
        if cfg!(target_os = "macos") {
            PathBuf::from("/Library/Application Support")
        } else {
            PathBuf::from("/etc")
        }
    }
}

/// The machine-wide managed settings file: `C:\Program Files\Neuron\managed-settings.json`
/// on Windows, `/Library/Application Support/Neuron/managed-settings.json` on macOS,
/// `/etc/neuron/managed-settings.json` elsewhere. No environment variable or settings
/// file can move it.
#[must_use]
pub fn managed_settings_path() -> Option<PathBuf> {
    let folder = if cfg!(any(windows, target_os = "macos")) {
        "Neuron"
    } else {
        "neuron"
    };
    Some(
        managed_settings_root()
            .join(folder)
            .join(MANAGED_SETTINGS_FILE),
    )
}

/// Fails, saying why, unless only an administrator can have written `path`: it and
/// every folder above it up to `root` must be owned by an administrator (SYSTEM,
/// Administrators or `TrustedInstaller` on Windows, root elsewhere), and no one else
/// may change them. Otherwise any user could put settings there that apply to every
/// account on the computer.
pub fn check_managed_settings_owner(path: &Path, root: &Path) -> Result<(), String> {
    if !path.starts_with(root) {
        return Err(format!(
            "{} is not under {}",
            path.display(),
            root.display()
        ));
    }
    for current in path.ancestors() {
        admin_only(current)?;
        if current == root {
            break;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn admin_only(path: &Path) -> Result<(), String> {
    winsec::check_admin_only(path)
}

/// Owned by root, and writable by no one else: not by others, and by the group only
/// when it is root's (gid 0).
#[cfg(unix)]
fn admin_only(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("cannot read who may change {}: {error}", path.display()))?;
    if metadata.uid() != 0 {
        return Err(format!(
            "{} is owned by uid {}, not by root",
            path.display(),
            metadata.uid()
        ));
    }
    let mode = metadata.mode();
    if mode & 0o002 != 0 || (mode & 0o020 != 0 && metadata.gid() != 0) {
        return Err(format!(
            "{} can be changed by users other than root (mode {:o})",
            path.display(),
            mode & 0o7777
        ));
    }
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn admin_only(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// `mcpPolicy` in the managed settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedMcpPolicy {
    /// Only these may start: catalog ids (`github`), `catalog:*` (any catalog
    /// connector) or server-name patterns (`corp-*`). `None`: no allow list.
    pub allow: Option<Vec<String>>,
    /// Never start: catalog ids, `catalog:*` or server-name patterns.
    pub deny: Vec<String>,
    /// Always configured and on: catalog ids, or servers the managed settings define.
    pub required: Vec<String>,
    /// Servers that are not catalog connectors may start (default true).
    pub allow_unvetted: bool,
    /// Local (stdio) servers may start (default true).
    pub allow_stdio: bool,
    /// Remote servers may reach only these hosts (`example.com`, `*.example.com`).
    /// `None`: any host.
    pub allowed_remote_hosts: Option<Vec<String>>,
    /// Permission rules added to everyone's (`toolRules.deny|ask|allow`); a deny rule
    /// wins over any allow rule.
    pub tool_rules: RuntimePermissionRuleConfig,
    /// Block every server the managed settings neither configure nor require while the
    /// plan's policy cannot be read or is out of date.
    pub fail_closed: bool,
}

impl ManagedMcpPolicy {
    /// Parses `mcpPolicy` (`context` names it in errors).
    pub fn parse(value: &JsonValue, context: &str) -> Result<Self, ConfigError> {
        let object = expect_object(value, context)?;
        let list = |key: &str| optional_string_array(object, key, context);
        let tool_rules = match object.get("toolRules") {
            Some(rules) => {
                let context = format!("{context}.toolRules");
                let rules = expect_object(rules, &context)?;
                let list = |key: &str| {
                    optional_string_array(rules, key, &context).map(Option::unwrap_or_default)
                };
                RuntimePermissionRuleConfig::new(list("allow")?, list("deny")?, list("ask")?)
            }
            None => RuntimePermissionRuleConfig::default(),
        };
        Ok(Self {
            allow: list("allow")?,
            deny: list("deny")?.unwrap_or_default(),
            required: list("required")?.unwrap_or_default(),
            allow_unvetted: optional_bool(object, "allowUnvetted", context)?.unwrap_or(true),
            allow_stdio: optional_bool(object, "allowStdio", context)?.unwrap_or(true),
            allowed_remote_hosts: list("allowedRemoteHosts")?,
            tool_rules,
            fail_closed: optional_bool(object, "failClosed", context)?.unwrap_or(false),
        })
    }
}

/// `policy.connectors` from `GET /api/account` (contract v1.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayConnectorPolicy {
    /// Catalog connectors the plan allows (ids or `catalog:*`); `None` when the field is
    /// missing or null: no allow list.
    pub allowed: Option<Vec<String>>,
    pub denied: Vec<String>,
    pub required: Vec<String>,
    /// Hosted connectors the plan includes.
    pub hosted: Vec<String>,
    /// Servers that are not catalog connectors may start (default true).
    pub allow_unvetted: bool,
    pub catalog_version: Option<String>,
}

impl GatewayConnectorPolicy {
    /// From the account's `policy` block; `None` when it has no `connectors` object.
    #[must_use]
    pub fn from_account_policy(policy: &Value) -> Option<Self> {
        let connectors = policy.get("connectors").filter(|value| value.is_object())?;
        let ids = |key: &str| {
            connectors.get(key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
        };
        Some(Self {
            allowed: ids("allowed"),
            denied: ids("denied").unwrap_or_default(),
            required: ids("required").unwrap_or_default(),
            hosted: ids("hosted").unwrap_or_default(),
            allow_unvetted: connectors
                .get("allow_unvetted")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            catalog_version: connectors
                .get("catalog_version")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// What the policy cache holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayPolicyCache {
    /// No cache: never signed in to Zero-X, or signed out.
    Missing,
    /// Read within [`GATEWAY_POLICY_TTL_SECS`]; `policy` is `None` when the account has
    /// no connector policy.
    Current {
        policy: Option<GatewayConnectorPolicy>,
        fetched_at: u64,
    },
    /// Older than [`GATEWAY_POLICY_TTL_SECS`]: the account could not be read since.
    Stale {
        policy: Option<GatewayConnectorPolicy>,
        fetched_at: u64,
    },
    /// The cache file cannot be read or parsed (why).
    Unreadable(String),
}

impl GatewayPolicyCache {
    fn policy(&self) -> Option<&GatewayConnectorPolicy> {
        match self {
            Self::Current { policy, .. } | Self::Stale { policy, .. } => policy.as_ref(),
            Self::Missing | Self::Unreadable(_) => None,
        }
    }
}

/// `<config home>/policy-cache.json`.
#[must_use]
pub fn gateway_policy_cache_path(config_home: &Path) -> PathBuf {
    config_home.join(GATEWAY_POLICY_CACHE_FILE)
}

/// Seconds since the Unix epoch.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// Caches the `policy` block of an `/api/account` answer read at `now` (`None` when
/// the answer had none: the plan sets no policy, which replaces an older one).
pub fn store_gateway_policy(
    config_home: &Path,
    policy: Option<&Value>,
    now: u64,
) -> Result<(), String> {
    let mut object = Map::new();
    object.insert("version".to_string(), json!(1));
    object.insert("fetchedAt".to_string(), json!(now));
    object.insert(
        "expiresAt".to_string(),
        json!(now.saturating_add(GATEWAY_POLICY_TTL_SECS)),
    );
    object.insert(
        "policy".to_string(),
        policy
            .filter(|policy| policy.is_object())
            .cloned()
            .unwrap_or(Value::Null),
    );
    write_object(&gateway_policy_cache_path(config_home), &object)
}

/// Forgets the cached plan policy (on sign-out).
pub fn clear_gateway_policy(config_home: &Path) -> Result<(), String> {
    let path = gateway_policy_cache_path(config_home);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot remove {}: {error}", path.display())),
    }
}

/// Reads the cached plan policy as it stands at `now`.
#[must_use]
pub fn read_gateway_policy(config_home: &Path, now: u64) -> GatewayPolicyCache {
    let path = gateway_policy_cache_path(config_home);
    if !path.is_file() {
        return GatewayPolicyCache::Missing;
    }
    let object = match read_object(&path) {
        Ok(object) => object,
        Err(error) => return GatewayPolicyCache::Unreadable(error),
    };
    let Some(fetched_at) = object.get("fetchedAt").and_then(Value::as_u64) else {
        return GatewayPolicyCache::Unreadable(format!("{} has no fetchedAt", path.display()));
    };
    let expires_at = object
        .get("expiresAt")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| fetched_at.saturating_add(GATEWAY_POLICY_TTL_SECS));
    let policy = object
        .get("policy")
        .and_then(GatewayConnectorPolicy::from_account_policy);
    if now < expires_at {
        GatewayPolicyCache::Current { policy, fetched_at }
    } else {
        GatewayPolicyCache::Stale { policy, fetched_at }
    }
}

/// Who set a requirement or a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpPolicySource {
    /// `mcpPolicy` in the managed settings.
    Managed,
    /// The Zero-X plan (`policy.connectors`).
    Gateway,
}

impl McpPolicySource {
    /// As a sentence names it: `managed settings`, `your Zero-X plan`.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Managed => "managed settings",
            Self::Gateway => "your Zero-X plan",
        }
    }
}

/// A server the policy requires: it is on and cannot be turned off or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequirement {
    pub by: McpPolicySource,
    /// The catalog connector, when the requirement names one.
    pub catalog_id: Option<String>,
}

/// Where the plan's policy stands, as `/mcp` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum McpGatewayPolicyStatus {
    /// No plan policy: none cached, or the plan sets none.
    #[default]
    None,
    /// Read within the last day.
    Current,
    /// Older than a day; still applied unless the managed settings fail closed.
    Stale,
    /// The cache cannot be read.
    Unreadable,
}

/// Which policies applied while loading, and what they say (for servers added later).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpPolicyStatus {
    /// The managed settings file whose `mcpPolicy` applied.
    pub managed: Option<PathBuf>,
    pub gateway: McpGatewayPolicyStatus,
    /// The managed settings fail closed and the plan's policy is not current, so only
    /// the servers they configure or require may start; says why.
    pub closed: Option<String>,
    managed_policy: Option<ManagedMcpPolicy>,
    /// The plan policy that applied (none while closed).
    gateway_policy: Option<GatewayConnectorPolicy>,
}

impl McpPolicyStatus {
    /// Any policy applies.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.managed.is_some()
            || self.gateway != McpGatewayPolicyStatus::None
            || self.closed.is_some()
    }

    /// Why the policy would refuse `server` under `name`, if it would: for a server about
    /// to be added. Servers the managed settings define are never refused.
    #[must_use]
    pub fn refusal(&self, name: &str, server: &ScopedMcpServerConfig) -> Option<String> {
        self.check(name, server).map(|refusal| refusal.reason)
    }

    fn check(&self, name: &str, server: &ScopedMcpServerConfig) -> Option<Refusal> {
        if server.scope == ConfigSource::Managed {
            return None;
        }
        if let Some(reason) = &self.closed {
            return Some(Refusal {
                rule: "mcpPolicy.failClosed".to_string(),
                reason: reason.clone(),
                by: McpPolicySource::Managed,
            });
        }
        let catalog_id = server_catalog_id(name, server);
        self.managed_policy
            .as_ref()
            .and_then(|policy| managed_refusal(policy, name, &server.config, catalog_id.as_deref()))
            .or_else(|| {
                self.gateway_policy.as_ref().and_then(|policy| {
                    gateway_refusal(policy, name, &server.config, catalog_id.as_deref())
                })
            })
    }
}

/// A policy refusal: the policy entry, the reason shown, and whose policy it is.
struct Refusal {
    rule: String,
    reason: String,
    by: McpPolicySource,
}

impl Refusal {
    fn block(self, server: ScopedMcpServerConfig) -> BlockedMcpServer {
        BlockedMcpServer {
            server,
            rule: self.rule,
            reason: self.reason,
            policy: Some(self.by),
        }
    }
}

/// What [`apply_mcp_policy`] decided.
#[derive(Debug, Default)]
pub(crate) struct AppliedMcpPolicy {
    pub(crate) blocked: BTreeMap<String, BlockedMcpServer>,
    pub(crate) required: BTreeMap<String, McpRequirement>,
    pub(crate) status: McpPolicyStatus,
    /// Shown once per process.
    pub(crate) warnings: Vec<String>,
}

/// What [`apply_mcp_policy`] reads.
pub(crate) struct McpPolicyInputs<'a> {
    pub(crate) managed: Option<&'a ManagedMcpPolicy>,
    /// The managed settings file `managed` came from.
    pub(crate) managed_path: Option<&'a Path>,
    pub(crate) gateway: &'a GatewayPolicyCache,
    /// The launch spec of a server Neuron ships, by name (`codebase-memory`).
    pub(crate) bundled: &'a dyn Fn(&str) -> Option<McpServerConfig>,
    /// When the policy is applied (to describe a stale cache).
    pub(crate) now: u64,
}

/// Applies the managed and plan policies to the configured servers: adds and switches on
/// the required ones (taking them out of `disabled`), and moves the refused ones out of
/// `servers` into the result's `blocked`.
pub(crate) fn apply_mcp_policy(
    servers: &mut BTreeMap<String, ScopedMcpServerConfig>,
    disabled: &mut BTreeMap<String, ScopedMcpServerConfig>,
    inputs: &McpPolicyInputs<'_>,
) -> AppliedMcpPolicy {
    let mut applied = AppliedMcpPolicy {
        status: policy_status(inputs),
        ..AppliedMcpPolicy::default()
    };
    if applied.status.closed.is_none() {
        match inputs.gateway {
            GatewayPolicyCache::Stale { fetched_at, policy } if policy.is_some() => {
                applied.warnings.push(format!(
                    "your Zero-X plan's connector policy was last read {} ago; Neuron uses it until it can reach Zero-X again",
                    describe_age(inputs.now.saturating_sub(*fetched_at))
                ));
            }
            GatewayPolicyCache::Unreadable(error) => applied.warnings.push(format!(
                "cannot read the cached connector policy ({error}); no plan policy applies until Neuron reads the account again"
            )),
            _ => {}
        }
    }

    let mut wanted: Vec<(String, McpPolicySource)> = Vec::new();
    for (ids, by) in [
        (
            applied
                .status
                .managed_policy
                .as_ref()
                .map(|policy| &policy.required),
            McpPolicySource::Managed,
        ),
        (
            applied
                .status
                .gateway_policy
                .as_ref()
                .map(|policy| &policy.required),
            McpPolicySource::Gateway,
        ),
    ] {
        for id in ids.into_iter().flatten() {
            if !wanted.iter().any(|(seen, _)| seen == id) {
                wanted.push((id.clone(), by));
            }
        }
    }
    for (id, by) in wanted {
        let Some((server, catalog_id)) = required_server(&id, servers, disabled, inputs.bundled)
        else {
            applied.warnings.push(format!(
                "{} require the MCP server `{id}`, which is neither a catalog connector nor configured in the managed settings; it is left out",
                by.describe()
            ));
            continue;
        };
        if by == McpPolicySource::Gateway {
            // The managed settings outrank the plan.
            let refusal = applied.status.managed_policy.as_ref().and_then(|policy| {
                managed_refusal(policy, &id, &server.config, catalog_id.as_deref())
            });
            if let Some(refusal) = refusal {
                servers.remove(&id);
                disabled.remove(&id);
                applied.blocked.insert(id, refusal.block(server));
                continue;
            }
        }
        disabled.remove(&id);
        servers.insert(id.clone(), server);
        applied
            .required
            .insert(id, McpRequirement { by, catalog_id });
    }

    // Servers the managed settings define or require (`Managed` scope) are never
    // refused; a plan requirement was checked against the managed policy above.
    let refused = servers
        .iter()
        .filter_map(|(name, server)| {
            applied
                .status
                .check(name, server)
                .map(|refusal| (name.clone(), refusal))
        })
        .collect::<Vec<_>>();
    for (name, refusal) in refused {
        if let Some(server) = servers.remove(&name) {
            applied.blocked.insert(name, refusal.block(server));
        }
    }
    applied
}

/// Which policies apply, given the managed policy and the plan policy cache.
fn policy_status(inputs: &McpPolicyInputs<'_>) -> McpPolicyStatus {
    let managed = inputs.managed;
    let gateway = match inputs.gateway {
        GatewayPolicyCache::Missing | GatewayPolicyCache::Current { policy: None, .. } => {
            McpGatewayPolicyStatus::None
        }
        GatewayPolicyCache::Current { .. } => McpGatewayPolicyStatus::Current,
        GatewayPolicyCache::Stale { .. } => McpGatewayPolicyStatus::Stale,
        GatewayPolicyCache::Unreadable(_) => McpGatewayPolicyStatus::Unreadable,
    };
    let fail_closed = managed.is_some_and(|policy| policy.fail_closed);
    let closed = (fail_closed && !matches!(inputs.gateway, GatewayPolicyCache::Current { .. }))
        .then(|| {
            format!(
                "refused because the managed settings fail closed (mcpPolicy.failClosed) and your Zero-X plan's connector policy {}",
                match inputs.gateway {
                    GatewayPolicyCache::Stale { fetched_at, .. } => format!(
                        "was last read {} ago",
                        describe_age(inputs.now.saturating_sub(*fetched_at))
                    ),
                    _ => "has not been read; sign in to Zero-X, or run neuron usage once online"
                        .to_string(),
                }
            )
        });
    McpPolicyStatus {
        managed: managed.and(inputs.managed_path.map(Path::to_path_buf)),
        gateway,
        // A plan policy that is not current does not apply while closed: only what the
        // managed settings configure or require starts.
        gateway_policy: if closed.is_some() {
            None
        } else {
            inputs.gateway.policy().cloned()
        },
        closed,
        managed_policy: managed.cloned(),
    }
}

/// The server a requirement for `id` starts, and its catalog id: a server the managed
/// settings define under that name; else the user's own (or Neuron's bundled) server
/// written from that catalog entry; else one written from the catalog entry (or the
/// bundled launch). Workspace files cannot supply it: their variables are not vetted.
fn required_server(
    id: &str,
    servers: &BTreeMap<String, ScopedMcpServerConfig>,
    disabled: &BTreeMap<String, ScopedMcpServerConfig>,
    bundled: &dyn Fn(&str) -> Option<McpServerConfig>,
) -> Option<(ScopedMcpServerConfig, Option<String>)> {
    let entry = McpCatalog::builtin().get(id);
    let catalog_id = entry.map(|entry| entry.id.clone());
    let configured = servers.get(id).or_else(|| disabled.get(id));
    if let Some(server) = configured {
        let keep = match server.scope {
            ConfigSource::Managed => true,
            ConfigSource::Bundled => entry.is_some(),
            ConfigSource::User => {
                entry.is_some() && server_catalog_id(id, server).as_deref() == Some(id)
            }
            ConfigSource::Project | ConfigSource::Local => false,
        };
        if keep {
            return Some((
                ScopedMcpServerConfig {
                    scope: ConfigSource::Managed,
                    config: server.config.clone(),
                },
                catalog_id,
            ));
        }
    }
    let entry = entry?;
    let config = match bundled(id) {
        Some(config) => config,
        None => {
            parse_mcp_server_json(id, &entry.settings_entry(&BTreeMap::new()).to_string()).ok()?
        }
    };
    Some((
        ScopedMcpServerConfig {
            scope: ConfigSource::Managed,
            config,
        },
        catalog_id,
    ))
}

/// The catalog connector `server` is: a bundled server by its name, any other one when
/// its launch spec is exactly the entry's.
#[must_use]
pub fn server_catalog_id(name: &str, server: &ScopedMcpServerConfig) -> Option<String> {
    let catalog = McpCatalog::builtin();
    if server.scope == ConfigSource::Bundled {
        return catalog.get(name).map(|entry| entry.id.clone());
    }
    catalog
        .entry_for_config(&server.config)
        .map(|entry| entry.id.clone())
}

/// Why `mcpPolicy` refuses the server.
fn managed_refusal(
    policy: &ManagedMcpPolicy,
    name: &str,
    config: &McpServerConfig,
    catalog_id: Option<&str>,
) -> Option<Refusal> {
    let refuse = |rule: &str, reason: String| {
        Some(Refusal {
            rule: rule.to_string(),
            reason: format!("refused by managed settings: {reason}"),
            by: McpPolicySource::Managed,
        })
    };
    let resembling = resembling_ids(config);
    if let Some(entry) = policy
        .deny
        .iter()
        .find(|entry| denies(entry, name, catalog_id, &resembling))
    {
        return refuse(
            &format!("mcpPolicy.deny: {entry}"),
            format!("mcpPolicy.deny lists \"{entry}\""),
        );
    }
    if !policy.allow_stdio && config.transport() == McpTransport::Stdio {
        return refuse(
            "mcpPolicy.allowStdio",
            "local (stdio) MCP servers are turned off (mcpPolicy.allowStdio)".to_string(),
        );
    }
    if let Some(hosts) = &policy.allowed_remote_hosts {
        if let Some(host) = remote_host(config) {
            if !hosts.iter().any(|allowed| host_matches(allowed, &host)) {
                return refuse(
                    "mcpPolicy.allowedRemoteHosts",
                    format!("{host} is not in mcpPolicy.allowedRemoteHosts"),
                );
            }
        }
    }
    if !policy.allow_unvetted && catalog_id.is_none() {
        return refuse(
            "mcpPolicy.allowUnvetted",
            "only catalog connectors may start (mcpPolicy.allowUnvetted)".to_string(),
        );
    }
    if let Some(allow) = &policy.allow {
        if !allow.iter().any(|entry| allows(entry, name, catalog_id)) {
            return refuse(
                "mcpPolicy.allow",
                "not on the mcpPolicy.allow list".to_string(),
            );
        }
    }
    None
}

/// Why the plan's policy refuses the server.
fn gateway_refusal(
    policy: &GatewayConnectorPolicy,
    name: &str,
    config: &McpServerConfig,
    catalog_id: Option<&str>,
) -> Option<Refusal> {
    let refuse = |rule: &str, reason: String| {
        Some(Refusal {
            rule: format!("policy.connectors.{rule}"),
            reason: format!("refused by your Zero-X plan: {reason}"),
            by: McpPolicySource::Gateway,
        })
    };
    let resembling = resembling_ids(config);
    if let Some(id) = policy
        .denied
        .iter()
        .find(|id| denies(id, name, catalog_id, &resembling))
    {
        return refuse(
            &format!("denied: {id}"),
            format!("its connector policy denies \"{id}\""),
        );
    }
    if let McpServerConfig::ManagedProxy(proxy) = config {
        if !policy.hosted.contains(&proxy.id) {
            return refuse(
                "hosted",
                format!("it does not include the hosted connector \"{}\"", proxy.id),
            );
        }
    }
    match catalog_id {
        None if !policy.allow_unvetted => refuse(
            "allow_unvetted",
            "only catalog connectors may start".to_string(),
        ),
        Some(id)
            if policy.allowed.as_ref().is_some_and(|allowed| {
                !allowed.iter().any(|entry| allows(entry, "", Some(id)))
            }) =>
        {
            refuse(
                "allowed",
                format!("the catalog connector \"{id}\" is not included"),
            )
        }
        _ => None,
    }
}

/// An allow entry: `catalog:*` (any catalog connector), `catalog:<id>` or a catalog id
/// (that connector, as the catalog writes it), or a server-name pattern (`*` matches any
/// run of characters). A catalog id never matches by name, so naming a server `github`
/// does not pass an allow list that lists the GitHub connector.
fn allows(entry: &str, name: &str, catalog_id: Option<&str>) -> bool {
    if entry == "catalog:*" {
        return catalog_id.is_some();
    }
    if let Some(id) = entry.strip_prefix("catalog:") {
        return catalog_id == Some(id);
    }
    if McpCatalog::builtin().get(entry).is_some() {
        return catalog_id == Some(entry);
    }
    glob_matches(entry, name)
}

/// A deny entry: as [`allows`], but a catalog id also refuses a server of that name and
/// one that runs the connector's package at another version or reaches its host
/// (`resembling`, from [`resembling_ids`]), and `catalog:*` refuses those too. Still a
/// best effort: a server can run the same code some other way, so only an allow list
/// or `allowUnvetted: false` keeps everything else out.
fn denies(entry: &str, name: &str, catalog_id: Option<&str>, resembling: &[String]) -> bool {
    if allows(entry, name, catalog_id) || glob_matches(entry, name) {
        return true;
    }
    if entry == "catalog:*" {
        return !resembling.is_empty();
    }
    let id = entry.strip_prefix("catalog:").unwrap_or(entry);
    resembling.iter().any(|resembled| resembled == id)
}

/// The catalog connectors `config` runs the same server as, however it is launched.
fn resembling_ids(config: &McpServerConfig) -> Vec<String> {
    McpCatalog::builtin()
        .entries_resembling(config)
        .into_iter()
        .map(|entry| entry.id.clone())
        .collect()
}

/// `pattern` matches all of `text`; `*` matches any run of characters.
fn glob_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let text = text.chars().collect::<Vec<_>>();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] != '*' && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|ch| *ch == '*')
}

/// The host a remote server reaches; `None` for a local one.
fn remote_host(config: &McpServerConfig) -> Option<String> {
    let url = match config {
        McpServerConfig::Sse(remote) | McpServerConfig::Http(remote) => &remote.url,
        McpServerConfig::Ws(socket) => &socket.url,
        McpServerConfig::ManagedProxy(proxy) => &proxy.url,
        McpServerConfig::Stdio(_) | McpServerConfig::Sdk(_) => return None,
    };
    // `url_host` reads http(s) URLs; a WebSocket URL has the same authority.
    let web = url
        .strip_prefix("wss://")
        .map(|rest| format!("https://{rest}"))
        .or_else(|| {
            url.strip_prefix("ws://")
                .map(|rest| format!("http://{rest}"))
        })
        .unwrap_or_else(|| url.clone());
    // An unreadable URL is kept whole: it matches no allowed host.
    Some(crate::permissions::url_host(&web).unwrap_or_else(|| url.clone()))
}

/// `allowed` is the host itself, or `*.example.com` and the host is under it.
fn host_matches(allowed: &str, host: &str) -> bool {
    let allowed = allowed.trim().to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    match allowed.strip_prefix("*.") {
        Some(domain) => host.ends_with(&format!(".{domain}")),
        None => host == allowed,
    }
}

/// `3 days`, `5 hours`, `12 minutes`.
fn describe_age(seconds: u64) -> String {
    let (count, unit) = if seconds >= 2 * 86_400 {
        (seconds / 86_400, "days")
    } else if seconds >= 2 * 3_600 {
        (seconds / 3_600, "hours")
    } else {
        ((seconds / 60).max(1), "minutes")
    };
    format!("{count} {unit}")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::{
        glob_matches, host_matches, read_gateway_policy, store_gateway_policy,
        GatewayConnectorPolicy, GatewayPolicyCache, McpGatewayPolicyStatus, McpPolicySource,
        GATEWAY_POLICY_TTL_SECS,
    };
    use crate::config::{ConfigLoader, ConfigSource, McpServerConfig};

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("neuron-mcp-policy-{label}-{nanos}"))
    }

    /// A workspace, a config home and a managed settings file.
    struct Fixture {
        root: PathBuf,
        cwd: PathBuf,
        home: PathBuf,
        managed: PathBuf,
    }

    impl Fixture {
        fn new(label: &str) -> Self {
            let root = temp_dir(label);
            let cwd = root.join("project");
            let home = root.join("home").join(".neuron");
            let managed = root.join("machine").join("managed-settings.json");
            fs::create_dir_all(&cwd).expect("workspace");
            fs::create_dir_all(&home).expect("config home");
            fs::create_dir_all(managed.parent().expect("parent")).expect("machine folder");
            Self {
                root,
                cwd,
                home,
                managed,
            }
        }

        fn user(&self, settings: &str) -> &Self {
            fs::write(self.home.join("settings.json"), settings).expect("user settings");
            self
        }

        fn managed(&self, settings: &str) -> &Self {
            fs::write(&self.managed, settings).expect("managed settings");
            self
        }

        fn loader(&self) -> ConfigLoader {
            ConfigLoader::new(&self.cwd, &self.home)
                .with_managed_settings(Some(self.managed.clone()))
        }

        fn gateway(&self, connectors: &serde_json::Value, fetched_at: u64) -> &Self {
            store_gateway_policy(
                &self.home,
                Some(&json!({ "connectors": connectors })),
                fetched_at,
            )
            .expect("cache the plan policy");
            self
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn now() -> u64 {
        super::unix_now()
    }

    #[test]
    fn a_managed_deny_blocks_a_user_server_by_name_with_the_reason() {
        let fixture = Fixture::new("deny");
        fixture
            .user(r#"{"defaultMcpServers":false,"mcpServers":{"slack":{"command":"slack-mcp"},"docs":{"command":"docs-mcp"}}}"#)
            .managed(r#"{"mcpPolicy":{"deny":["slack"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        assert!(
            config.mcp().get("slack").is_none(),
            "a denied server does not start"
        );
        assert!(config.mcp().get("docs").is_some());
        let blocked = &config.mcp().blocked()["slack"];
        assert_eq!(blocked.rule, "mcpPolicy.deny: slack");
        assert!(
            blocked.reason.contains("managed settings"),
            "{}",
            blocked.reason
        );
        assert_eq!(blocked.server.scope, ConfigSource::User);
        assert!(config.mcp().policy().managed.is_some());
    }

    #[test]
    fn a_managed_deny_pattern_and_catalog_wildcard_match_names_and_connectors() {
        let fixture = Fixture::new("patterns");
        fixture
            .user(r#"{"mcpServers":{"corp-wiki":{"command":"wiki"},"corp-jira":{"command":"jira"},"mine":{"command":"mine"}}}"#)
            .managed(r#"{"mcpPolicy":{"deny":["corp-*"],"allow":["catalog:*","mi*"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        let mut blocked = config.mcp().blocked().keys().cloned().collect::<Vec<_>>();
        blocked.sort();
        assert_eq!(blocked, ["corp-jira", "corp-wiki"]);
        assert!(
            config.mcp().get("mine").is_some(),
            "allowed by name pattern"
        );
        // The bundled servers are catalog connectors.
        assert!(config.mcp().get("codebase-memory").is_some());
        assert!(config.mcp().get("playwright").is_some());
    }

    #[test]
    fn a_deny_naming_a_connector_also_refuses_its_package_and_host_under_other_names() {
        let fixture = Fixture::new("deny-lookalike");
        fixture
            .user(
                r#"{"defaultMcpServers":false,"mcpServers":{
                "gh":{"type":"http","url":"https://API.githubcopilot.com/mcp/x"},
                "pw":{"command":"npx","args":["-y","@playwright/mcp@latest","--headless"]},
                "git2":{"command":"uvx","args":["mcp-server-git==1.0"]},
                "gitmod":{"command":"python","args":["-m","mcp_server_git"]},
                "mine":{"command":"npx","args":["-y","@someone/mcp@1.0"]}}}"#,
            )
            .managed(r#"{"mcpPolicy":{"deny":["github","playwright","git"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        for name in ["gh", "pw", "git2", "gitmod"] {
            assert!(config.mcp().get(name).is_none(), "{name} starts");
            assert!(
                config.mcp().blocked()[name]
                    .rule
                    .starts_with("mcpPolicy.deny"),
                "{name}"
            );
        }
        assert!(
            config.mcp().get("mine").is_some(),
            "another scope's `mcp` package is not Playwright's"
        );

        // The plan's `denied` list matches the same way.
        fixture
            .managed("{}")
            .gateway(&json!({"denied": ["playwright"]}), now());
        let config = fixture.loader().load().expect("config loads");
        assert_eq!(
            config.mcp().blocked()["pw"].rule,
            "policy.connectors.denied: playwright"
        );
        assert!(config.mcp().get("gh").is_some());
    }

    #[test]
    fn an_allow_list_naming_a_catalog_id_does_not_let_a_lookalike_server_through() {
        let fixture = Fixture::new("lookalike");
        fixture
            .user(
                r#"{"defaultMcpServers":false,"mcpServers":{"github":{"command":"evil-github"}}}"#,
            )
            .managed(r#"{"mcpPolicy":{"allow":["github"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        assert!(config.mcp().get("github").is_none());
        assert_eq!(config.mcp().blocked()["github"].rule, "mcpPolicy.allow");
    }

    #[test]
    fn unvetted_stdio_and_remote_host_rules_block_what_they_name() {
        let fixture = Fixture::new("kinds");
        fixture
            .user(r#"{"defaultMcpServers":false,"mcpServers":{"local":{"command":"local-mcp"},"remote":{"type":"http","url":"https://mcp.evil.test/mcp"},"ok":{"type":"http","url":"https://api.corp.test/mcp"}}}"#)
            .managed(r#"{"mcpPolicy":{"allowStdio":false,"allowedRemoteHosts":["*.corp.test"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        assert_eq!(config.mcp().blocked()["local"].rule, "mcpPolicy.allowStdio");
        assert_eq!(
            config.mcp().blocked()["remote"].rule,
            "mcpPolicy.allowedRemoteHosts"
        );
        assert!(config.mcp().get("ok").is_some());

        fixture.managed(r#"{"mcpPolicy":{"allowUnvetted":false}}"#);
        let config = fixture.loader().load().expect("config loads");
        assert_eq!(
            config.mcp().blocked()["local"].rule,
            "mcpPolicy.allowUnvetted"
        );
    }

    #[test]
    fn a_required_bundled_server_survives_default_servers_off_and_being_disabled() {
        let fixture = Fixture::new("required");
        fixture
            .user(r#"{"defaultMcpServers":false,"disabledMcpServers":["codebase-memory"]}"#)
            .managed(r#"{"mcpPolicy":{"required":["codebase-memory"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        let server = config
            .mcp()
            .get("codebase-memory")
            .expect("a required server is configured and on");
        assert_eq!(server.scope, ConfigSource::Managed);
        assert!(!config.mcp().disabled().contains_key("codebase-memory"));
        let requirement = &config.mcp().required()["codebase-memory"];
        assert_eq!(requirement.by, McpPolicySource::Managed);
        assert_eq!(requirement.catalog_id.as_deref(), Some("codebase-memory"));
        // Playwright is not required, so defaultMcpServers:false still leaves it out.
        assert!(config.mcp().get("playwright").is_none());

        let refused = crate::set_mcp_server_enabled(&fixture.loader(), "codebase-memory", false)
            .expect_err("a required server cannot be turned off");
        assert!(
            refused.contains("required by managed settings"),
            "{refused}"
        );
    }

    #[test]
    fn only_a_managed_deny_rule_blocks_a_required_server() {
        let fixture = Fixture::new("required-deny");
        fs::create_dir_all(fixture.cwd.join(".neuron")).expect("project folder");
        fs::write(
            fixture.cwd.join(".neuron").join("settings.json"),
            r#"{"permissions":{"deny":["mcp__codebase-memory__*"]}}"#,
        )
        .expect("project settings");
        fixture
            .user(r#"{"defaultMcpServers":false,"permissions":{"deny":["mcp__codebase-memory"]}}"#)
            .managed(r#"{"mcpPolicy":{"required":["codebase-memory"]}}"#);

        // Neither the user's rule nor an untrusted workspace's turns it off.
        for trust in [crate::ProjectTrust::Trusted, crate::ProjectTrust::Untrusted] {
            let config = fixture
                .loader()
                .with_project_trust(trust)
                .load()
                .expect("config loads");
            assert!(config.mcp().get("codebase-memory").is_some(), "{trust:?}");
            assert!(!config.mcp().blocked().contains_key("codebase-memory"));
            assert!(config.mcp().required().contains_key("codebase-memory"));
            // Their rules still refuse its tools when called.
            let policy = crate::PermissionPolicy::new(crate::PermissionMode::DangerFullAccess)
                .with_permission_rules(config.permission_rules());
            assert!(matches!(
                policy.authorize("mcp__codebase-memory__search_graph", "{}", None),
                crate::PermissionOutcome::Deny { .. }
            ));
        }

        // The managed settings' own deny rule does block it, and it is no longer
        // listed as required.
        for managed in [
            r#"{"permissions":{"deny":["mcp__codebase-memory"]},"mcpPolicy":{"required":["codebase-memory"]}}"#,
            r#"{"mcpPolicy":{"required":["codebase-memory"],"toolRules":{"deny":["mcp__codebase-memory__*"]}}}"#,
        ] {
            fixture.managed(managed);
            let config = fixture.loader().load().expect("config loads");
            assert!(config.mcp().get("codebase-memory").is_none(), "{managed}");
            assert!(config.mcp().blocked().contains_key("codebase-memory"));
            assert!(!config.mcp().required().contains_key("codebase-memory"));
        }
    }

    #[test]
    fn a_required_catalog_connector_is_written_from_its_entry_not_the_workspace() {
        let fixture = Fixture::new("required-catalog");
        fs::create_dir_all(fixture.cwd.join(".neuron")).expect("project folder");
        fs::write(
            fixture.cwd.join(".neuron").join("settings.json"),
            r#"{"mcpServers":{"fetch":{"command":"pulled-fetch"}}}"#,
        )
        .expect("project settings");
        fixture
            .user(r#"{"defaultMcpServers":false}"#)
            .managed(r#"{"mcpPolicy":{"required":["fetch"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        let server = config.mcp().get("fetch").expect("required");
        assert_eq!(server.scope, ConfigSource::Managed);
        let McpServerConfig::Stdio(stdio) = &server.config else {
            panic!("fetch is a stdio connector");
        };
        assert_ne!(stdio.command, "pulled-fetch");
        assert_eq!(
            crate::mcp_policy::server_catalog_id("fetch", server).as_deref(),
            Some("fetch")
        );
    }

    #[test]
    fn the_plan_policy_applies_from_the_cache_on_every_load() {
        let fixture = Fixture::new("gateway");
        fixture
            .user(r#"{"defaultMcpServers":false,"mcpServers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"},"own":{"command":"own"}}}"#)
            .gateway(&json!({"denied": ["github"], "allowed": ["catalog:*"], "hosted": []}), now());

        // A fresh loader, as after a restart: the cached policy still applies.
        let config = fixture.loader().load().expect("config loads");

        let blocked = &config.mcp().blocked()["github"];
        assert_eq!(blocked.rule, "policy.connectors.denied: github");
        assert!(blocked.reason.contains("Zero-X plan"), "{}", blocked.reason);
        assert!(
            config.mcp().get("own").is_some(),
            "unvetted servers stay allowed"
        );
        assert_eq!(
            config.mcp().policy().gateway,
            McpGatewayPolicyStatus::Current
        );
    }

    #[test]
    fn a_stale_plan_policy_still_applies_unless_managed_settings_fail_closed() {
        let fixture = Fixture::new("stale");
        let long_ago = now() - GATEWAY_POLICY_TTL_SECS - 60;
        fixture
            .user(r#"{"defaultMcpServers":false,"mcpServers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/"},"own":{"command":"own"}}}"#)
            .gateway(&json!({"denied": ["github"]}), long_ago);

        let config = fixture.loader().load().expect("config loads");
        assert!(config.mcp().blocked().contains_key("github"));
        assert!(config.mcp().get("own").is_some());
        assert_eq!(config.mcp().policy().gateway, McpGatewayPolicyStatus::Stale);
        assert!(config.mcp().policy().closed.is_none());

        fixture.managed(
            r#"{"mcpPolicy":{"failClosed":true},"mcpServers":{"corp":{"command":"corp-mcp"}}}"#,
        );
        let config = fixture.loader().load().expect("config loads");
        assert!(config.mcp().policy().closed.is_some());
        assert_eq!(config.mcp().blocked()["own"].rule, "mcpPolicy.failClosed");
        assert!(
            config.mcp().get("corp").is_some(),
            "servers the managed settings define still start"
        );
    }

    #[test]
    fn fail_closed_without_any_cache_blocks_all_but_managed_servers() {
        let fixture = Fixture::new("closed-missing");
        fixture
            .user(r#"{"mcpServers":{"own":{"command":"own"}}}"#)
            .managed(r#"{"mcpPolicy":{"failClosed":true,"required":["playwright"]}}"#);

        let config = fixture.loader().load().expect("config loads");

        assert!(config.mcp().blocked().contains_key("own"));
        assert!(config.mcp().blocked().contains_key("codebase-memory"));
        assert!(
            config.mcp().get("playwright").is_some(),
            "required by managed"
        );
    }

    #[test]
    fn a_plan_requirement_cannot_override_a_managed_deny() {
        let fixture = Fixture::new("plan-vs-managed");
        fixture
            .user(r#"{"defaultMcpServers":false}"#)
            .managed(r#"{"mcpPolicy":{"deny":["fetch"]}}"#)
            .gateway(&json!({"required": ["fetch", "git"]}), now());

        let config = fixture.loader().load().expect("config loads");

        assert!(config.mcp().get("fetch").is_none());
        assert_eq!(
            config.mcp().blocked()["fetch"].rule,
            "mcpPolicy.deny: fetch"
        );
        assert_eq!(config.mcp().required()["git"].by, McpPolicySource::Gateway);
    }

    #[test]
    fn mcp_policy_outside_the_managed_settings_is_ignored() {
        let fixture = Fixture::new("user-policy");
        fixture.user(r#"{"defaultMcpServers":false,"mcpPolicy":{"deny":["own"]},"mcpServers":{"own":{"command":"own"}}}"#);

        let config = fixture.loader().load().expect("config loads");

        assert!(config.mcp().get("own").is_some());
        assert!(!config.mcp().policy().is_active());
    }

    #[test]
    fn managed_tool_rules_join_the_permission_rules_and_keep_user_rules() {
        let fixture = Fixture::new("tool-rules");
        fixture
            .user(r#"{"permissions":{"allow":["mcp__github__delete_repository"],"deny":["bash(rm:*)"]}}"#)
            .managed(r#"{"permissions":{"deny":["WebFetch"]},"mcpPolicy":{"toolRules":{"deny":["mcp__github__delete_repository"],"ask":["mcp__github"]}}}"#);

        let config = fixture.loader().load().expect("config loads");
        let rules = config.permission_rules();

        assert!(rules
            .deny()
            .iter()
            .any(|rule| rule == "mcp__github__delete_repository"));
        assert!(
            rules.deny().iter().any(|rule| rule == "bash(rm:*)"),
            "user deny rules stay"
        );
        assert!(rules.deny().iter().any(|rule| rule == "WebFetch"));
        assert!(rules.ask().iter().any(|rule| rule == "mcp__github"));

        // The managed deny wins over the user's allow.
        let policy = crate::PermissionPolicy::new(crate::PermissionMode::DangerFullAccess)
            .with_permission_rules(rules);
        assert!(matches!(
            policy.authorize("mcp__github__delete_repository", "{}", None),
            crate::PermissionOutcome::Deny { .. }
        ));
    }

    #[test]
    fn the_cache_keeps_the_policy_block_and_reads_back_current_or_stale() {
        let root = temp_dir("cache");
        fs::create_dir_all(&root).expect("home");
        assert_eq!(read_gateway_policy(&root, 10), GatewayPolicyCache::Missing);

        store_gateway_policy(
            &root,
            Some(&json!({"connectors": {"denied": ["github"], "allow_unvetted": false, "catalog_version": "2026-11-04.1"}})),
            1_000,
        )
        .expect("cache");
        let policy = GatewayConnectorPolicy {
            allowed: None,
            denied: vec!["github".to_string()],
            required: Vec::new(),
            hosted: Vec::new(),
            allow_unvetted: false,
            catalog_version: Some("2026-11-04.1".to_string()),
        };
        assert_eq!(
            read_gateway_policy(&root, 1_001),
            GatewayPolicyCache::Current {
                policy: Some(policy.clone()),
                fetched_at: 1_000
            }
        );
        assert_eq!(
            read_gateway_policy(&root, 1_000 + GATEWAY_POLICY_TTL_SECS),
            GatewayPolicyCache::Stale {
                policy: Some(policy),
                fetched_at: 1_000
            }
        );

        // An account without a policy block replaces the old one.
        store_gateway_policy(&root, None, 2_000).expect("cache");
        assert_eq!(
            read_gateway_policy(&root, 2_001),
            GatewayPolicyCache::Current {
                policy: None,
                fetched_at: 2_000
            }
        );
        super::clear_gateway_policy(&root).expect("clear");
        assert_eq!(
            read_gateway_policy(&root, 2_001),
            GatewayPolicyCache::Missing
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn patterns_and_hosts_match_as_documented() {
        assert!(glob_matches("corp-*", "corp-wiki"));
        assert!(glob_matches("*", "anything"));
        assert!(glob_matches("a*c*e", "abcde"));
        assert!(!glob_matches("corp-*", "my-corp-wiki"));
        assert!(!glob_matches("slack", "slack2"));
        assert!(host_matches("*.corp.test", "api.corp.test"));
        assert!(!host_matches("*.corp.test", "corp.test.evil"));
        assert!(host_matches("Example.com", "example.com"));
        assert!(!host_matches("example.com", "api.example.com"));
    }

    #[test]
    fn the_os_managed_settings_path_is_fixed() {
        let path = super::managed_settings_path().expect("a path on every platform");
        assert!(path.ends_with(Path::new("managed-settings.json")));
        assert!(path.is_absolute());
        assert!(path.starts_with(super::managed_settings_root()));
    }

    #[cfg(windows)]
    #[test]
    fn the_windows_managed_settings_folder_ignores_the_environment() {
        let _env = crate::test_env_lock();
        let names = ["ProgramFiles", "ProgramData"];
        let before = names.map(std::env::var_os);
        let elsewhere = temp_dir("program-files");
        for name in names {
            std::env::set_var(name, &elsewhere);
        }
        let path = super::managed_settings_path().expect("a path");
        for (name, value) in names.into_iter().zip(before) {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        assert!(!path.starts_with(&elsewhere), "{}", path.display());
    }

    /// A file a user could write: refused with the reason. As root (on Unix), the file
    /// is made writable by everyone so the check still has something to refuse.
    #[test]
    fn a_managed_settings_file_a_user_can_write_is_refused() {
        let fixture = Fixture::new("owner");
        fixture.managed(r#"{"mcpPolicy":{"deny":["own"]}}"#);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&fixture.managed, fs::Permissions::from_mode(0o666))
                .expect("chmod");
        }
        let root = fixture.root.join("machine");

        let refused = super::check_managed_settings_owner(&fixture.managed, &root)
            .expect_err("a user's file is not trusted");
        assert!(refused.contains("managed-settings.json"), "{refused}");
        assert!(super::check_managed_settings_owner(&fixture.managed, &fixture.cwd).is_err());

        // A loader that checks the owner ignores the file; one that does not reads it.
        fixture.user(r#"{"defaultMcpServers":false,"mcpServers":{"own":{"command":"own"}}}"#);
        let config = fixture
            .loader()
            .with_managed_settings_root(Some(root))
            .load()
            .expect("config loads");
        assert!(config.mcp().get("own").is_some());
        assert!(!config.mcp().policy().is_active());
        let config = fixture.loader().load().expect("config loads");
        assert!(config.mcp().get("own").is_none());
    }
}
