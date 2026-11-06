//! The connector catalog: MCP servers Neuron has vetted, with a pinned version, how to
//! start each, the variables and secrets it needs, and the permission tier of each of
//! its tools. The catalog is compiled in (`mcp-catalog/catalog.json`; what was checked
//! is in `mcp-catalog/SOURCES.md`) and trusted because it ships in the binary. An
//! update the gateway serves is accepted only with a valid ed25519 signature and a
//! version newer than the catalog in use ([`verify_mcp_catalog_update`]). Nothing
//! fetches updates yet: the gateway serves them with hosted connectors (P5).
//!
//! A local entry also pins what it installs, so the signature covers it: an npm
//! package's whole dependency tree with each tarball's `sha512` integrity, a Python
//! package's requirements with their wheel and sdist `sha256` hashes, a release
//! binary's `sha256`, or a container image's digest. `mcp_install` installs from these
//! and checks every hash before a server starts.
//!
//! A configured server counts as a catalog server only when its launch spec is exactly
//! what the catalog entry writes ([`McpCatalog::entry_for_config`]), so a settings file
//! cannot borrow an entry's tiers by naming it.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config::McpServerConfig;
use crate::permissions::PermissionMode;

const BUILTIN_CATALOG_JSON: &str = include_str!("../mcp-catalog/catalog.json");

/// OWNER STEP: the ed25519 public key (32 bytes) that signs the catalog updates the
/// gateway serves. While it is `None`, every remote update is refused and the compiled-in
/// catalog is the only one. Set it to the production key; never commit a private key.
pub const MCP_CATALOG_UPDATE_PUBLIC_KEY: Option<[u8; 32]> = None;

/// What a tool can do, as the catalog entry pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpToolTier {
    /// Reads, changes nothing.
    Read,
    /// Creates or changes something that can be put back.
    Write,
    /// Deletes, overwrites, deploys, runs arbitrary code or SQL, or costs money.
    Destructive,
    /// Reaches arbitrary hosts or acts on the outside world in ways the entry cannot bound.
    External,
}

impl McpToolTier {
    /// The permission a call of a tool in this tier needs. `write` changes the workspace
    /// for a local (stdio) server that keeps its data there, and something outside it
    /// (`outside`: a remote server, or a local one that writes elsewhere), which asks as
    /// full access does.
    #[must_use]
    pub fn permission_mode(self, outside: bool) -> PermissionMode {
        match self {
            Self::Read => PermissionMode::ReadOnly,
            Self::Write if !outside => PermissionMode::WorkspaceWrite,
            Self::Write | Self::Destructive | Self::External => PermissionMode::DangerFullAccess,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Destructive => "destructive",
            Self::External => "external",
        }
    }
}

/// How an entry's server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCatalogTransport {
    Stdio,
    Http,
}

/// How an entry's server is installed and started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpInstallKind {
    /// An npm package, written in settings as `npx -y <package>@<version>` and run from
    /// a verified install of its `lock` (or of its `release` binary).
    Npm,
    /// A Python package, written as `uvx <package>@<version>` and run from a virtual
    /// environment installed from its hashed `requirements`.
    Uv,
    /// A release binary, written as its `package` name and run from a verified copy of
    /// the `release` asset for this platform.
    Binary,
    /// A container image, run with `docker run` by its digest.
    Docker,
    /// A remote server the vendor runs.
    Remote,
}

/// One package of an npm entry's dependency tree, as `package-lock.json` (v3) lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpNpmLockEntry {
    /// Where it goes, `node_modules/<name>` (nested for a second copy).
    pub path: String,
    pub version: String,
    /// The registry tarball.
    pub resolved: String,
    /// `sha512-<base64>` of the tarball.
    pub integrity: String,
    /// The platforms it is for (`win32`, `darwin`, `linux`); every one when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub os: Vec<String>,
    /// The CPUs it is for (`x64`, `arm64`); every one when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

/// One pinned Python requirement with the hashes of its wheels and sdist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpPythonRequirement {
    pub name: String,
    pub version: String,
    /// An environment marker (`sys_platform == 'win32'`), when it is not always needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub marker: Option<String>,
    /// `sha256:<hex>` of each file it may install from.
    pub hashes: Vec<String>,
}

/// How a release asset holds its program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpReleaseArchive {
    #[serde(rename = "zip")]
    Zip,
    #[serde(rename = "tar.gz")]
    TarGz,
    /// The asset is the program itself.
    #[serde(rename = "none")]
    None,
}

/// A release download for one platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpReleaseAsset {
    pub url: String,
    /// The sha256 (hex) of the download.
    pub sha256: String,
    pub archive: McpReleaseArchive,
    /// The program's path in the archive (its file name for `none`).
    pub member: String,
}

/// A file a local server keeps its data in. The server would keep it in its own package
/// folder, which a reinstall replaces, so Neuron points `variable` at
/// `<config home>/mcp-data/<id>/<file>` instead (`mcp_install`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpDataFile {
    /// The variable that names the file (one of the entry's plain variables).
    pub variable: String,
    /// The file's name in the data folder when the user does not set the variable.
    pub file: String,
}

/// The `release` key of this platform: `<os>-<arch>` as Rust names them
/// (`windows-x86_64`, `macos-aarch64`, `linux-x86_64`, ...).
#[must_use]
pub fn mcp_release_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// What an entry installs and how it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogInstall {
    pub kind: McpInstallKind,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// The package's npm `sha512-...` integrity, its wheel `sha256:...`, or an image's
    /// `sha256:...` digest.
    #[serde(default)]
    pub integrity: Option<String>,
    /// An npm entry's program: its path in the package (`cli.js`); a Python entry's
    /// console script when it is not named after the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<String>,
    /// An npm entry's dependency tree, the package itself included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lock: Vec<McpNpmLockEntry>,
    /// A Python entry's requirements, the package itself included.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<McpPythonRequirement>,
    /// Release downloads by platform ([`mcp_release_platform`]): a binary entry's
    /// program, or the program an npm package's install script would download (the
    /// script never runs; Neuron fetches and checks the binary itself).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub release: BTreeMap<String, McpReleaseAsset>,
    /// Arguments after the package.
    #[serde(default)]
    pub args: Vec<String>,
    /// A remote server's URL.
    #[serde(default)]
    pub url: Option<String>,
    /// A remote server's headers; a value may hold `${secret:NAME}`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Environment a stdio server always gets.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The file a local server keeps its data in, outside its install folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<McpDataFile>,
    /// The server signs in with OAuth (`neuron mcp login <server>`).
    #[serde(default)]
    pub oauth: bool,
}

/// A variable the user supplies when adding an entry: a secret (kept in the OS
/// keychain, referenced as `${secret:NAME}`) or a plain setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogVariable {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    /// Used when the user gives none (plain settings only).
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub help_url: Option<String>,
}

/// What a server may reach, for the stdio sandbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogSandbox {
    #[serde(default)]
    pub fs_read: Vec<String>,
    #[serde(default)]
    pub fs_write: Vec<String>,
    /// Hosts it reaches; `*` for any.
    #[serde(default)]
    pub network: Vec<String>,
    /// Inherited environment variables it needs besides the default set.
    #[serde(default)]
    pub env_allow: Vec<String>,
}

/// One vetted connector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher: String,
    /// Published by the service's vendor or the MCP project.
    #[serde(default)]
    pub official: bool,
    pub licence: String,
    pub source: String,
    pub transport: McpCatalogTransport,
    pub install: McpCatalogInstall,
    #[serde(default)]
    pub env: Vec<McpCatalogVariable>,
    /// Each tool's tier; a tool not listed gets `default_tier`.
    #[serde(default)]
    pub tools: BTreeMap<String, McpToolTier>,
    pub default_tier: McpToolTier,
    #[serde(default)]
    pub sandbox: McpCatalogSandbox,
    /// A local server whose `write` tools change files outside the workspace (its own
    /// data in a package or cache folder, or a path the user names), so they need full
    /// access as a remote server's do.
    #[serde(default)]
    pub writes_outside_workspace: bool,
    /// `user`, `project` or `local`.
    #[serde(default = "default_scope")]
    pub default_scope: String,
}

fn default_scope() -> String {
    "user".to_string()
}

/// The connector catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalog {
    pub version: String,
    pub connectors: Vec<McpCatalogEntry>,
}

/// `${secret:NAME}`: how settings refer to a secret kept in the keychain.
#[must_use]
pub fn mcp_secret_reference(name: &str) -> String {
    format!("${{secret:{name}}}")
}

/// The names of the `${secret:NAME}` references in `value`, in order.
#[must_use]
pub fn mcp_secret_references(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("${secret:") {
        let after = &rest[start + "${secret:".len()..];
        let Some(end) = after.find('}') else {
            break;
        };
        if !after[..end].is_empty() {
            names.push(after[..end].to_string());
        }
        rest = &after[end + 1..];
    }
    names
}

impl McpCatalog {
    /// The compiled-in catalog.
    #[must_use]
    pub fn builtin() -> &'static Self {
        static CATALOG: OnceLock<McpCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            Self::parse(BUILTIN_CATALOG_JSON).expect("the compiled-in MCP catalog parses")
        })
    }

    /// Parses and checks a catalog: unique ids, and every entry complete for its kind.
    pub fn parse(text: &str) -> Result<Self, String> {
        let catalog: Self =
            serde_json::from_str(text).map_err(|error| format!("invalid MCP catalog: {error}"))?;
        let mut seen = std::collections::BTreeSet::new();
        for entry in &catalog.connectors {
            if !seen.insert(entry.id.as_str()) {
                return Err(format!("MCP catalog lists `{}` twice", entry.id));
            }
            entry.check()?;
        }
        Ok(catalog)
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&McpCatalogEntry> {
        self.connectors.iter().find(|entry| entry.id == id)
    }

    /// The entries whose id, name, publisher or description contain every word of
    /// `query` (case-insensitive); all of them for an empty query.
    #[must_use]
    pub fn search(&self, query: &str) -> Vec<&McpCatalogEntry> {
        let words = query
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        self.connectors
            .iter()
            .filter(|entry| {
                let text = format!(
                    "{} {} {} {}",
                    entry.id, entry.name, entry.publisher, entry.description
                )
                .to_lowercase();
                words.iter().all(|word| text.contains(word.as_str()))
            })
            .collect()
    }

    /// The entry `config` was written from, when its launch spec is exactly that entry's
    /// (command and arguments or URL, and only the entry's own environment and headers).
    #[must_use]
    pub fn entry_for_config(&self, config: &McpServerConfig) -> Option<&McpCatalogEntry> {
        self.connectors.iter().find(|entry| entry.matches(config))
    }

    /// The entries `config` runs the same server as, launched any way: a local server
    /// whose command or an argument names the entry's package at any version (or none),
    /// a remote one on the entry's host, or the hosted connector of that id. Unlike
    /// [`McpCatalog::entry_for_config`] this vouches for nothing; a deny list uses it so
    /// that renaming a server or changing its version does not get past it.
    #[must_use]
    pub fn entries_resembling(&self, config: &McpServerConfig) -> Vec<&McpCatalogEntry> {
        self.connectors
            .iter()
            .filter(|entry| entry.matches(config) || entry.resembles(config))
            .collect()
    }
}

/// Where npm entries' tarballs come from.
pub(crate) const NPM_REGISTRY: &str = "https://registry.npmjs.org/";

/// 64 lower-case hex digits.
pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// A relative path that stays in its folder: no `..`, no root, no drive, no backslash.
pub(crate) fn relative_member(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// A package name as npm or Python tools compare it: lower case, with `_` and `.` as
/// `-` (so `mcp_server_git` and the module `awslabs.aws_documentation_mcp_server` name
/// their packages).
fn normalize_package(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace(['_', '.'], "-")
}

/// Drops a version after the last `@` that does not start the word: `pkg@1.2` and
/// `@scope/pkg@latest` name `pkg` and `@scope/pkg`.
fn without_npm_version(word: &str) -> &str {
    match word.rfind('@') {
        Some(at) if at > 0 => &word[..at],
        _ => word,
    }
}

/// Whether a command-line word names `package` (normalized) at any version or none:
/// `pkg`, `pkg@1.2`, `@scope/pkg@latest`, `pkg==1.2`, `pkg>=1`, or a path whose last
/// part is `pkg` or `pkg.exe`. A scoped npm package is matched only whole.
fn names_package(word: &str, package: &str) -> bool {
    let word = word.trim().trim_matches(['"', '\'']);
    if normalize_package(without_npm_version(word)) == package {
        return true;
    }
    if package.starts_with('@') {
        return false;
    }
    let last = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let name = last
        .split(['=', '>', '<', '~', '!', '['])
        .next()
        .unwrap_or(last);
    let name = without_npm_version(name);
    let name = [".exe", ".cmd", ".bat", ".js", ".py"]
        .into_iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name);
    normalize_package(name) == package
}

impl McpCatalogEntry {
    /// Whether `config` runs this entry's package or reaches its host, launched any way
    /// (see [`McpCatalog::entries_resembling`]).
    fn resembles(&self, config: &McpServerConfig) -> bool {
        match config {
            McpServerConfig::Stdio(stdio) => {
                let Some(package) = self.install.package.as_deref() else {
                    return false;
                };
                let package = normalize_package(package);
                std::iter::once(&stdio.command)
                    .chain(&stdio.args)
                    .any(|word| names_package(word, &package))
            }
            McpServerConfig::Http(remote) | McpServerConfig::Sse(remote) => {
                let host = |url: &str| {
                    crate::permissions::url_host(url).map(|host| host.to_ascii_lowercase())
                };
                let ours = self.install.url.as_deref().and_then(host);
                ours.is_some() && ours == host(&remote.url)
            }
            McpServerConfig::ManagedProxy(proxy) => proxy.id == self.id,
            McpServerConfig::Ws(_) | McpServerConfig::Sdk(_) => false,
        }
    }

    fn check(&self) -> Result<(), String> {
        let missing = |what: &str| format!("MCP catalog entry `{}` has no {what}", self.id);
        match (self.transport, self.install.kind) {
            (
                McpCatalogTransport::Stdio,
                McpInstallKind::Npm
                | McpInstallKind::Uv
                | McpInstallKind::Binary
                | McpInstallKind::Docker,
            ) => {
                if self
                    .install
                    .package
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
                {
                    return Err(missing("package"));
                }
                if self
                    .install
                    .version
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
                {
                    return Err(missing("pinned version"));
                }
                self.check_verified_install()?;
            }
            (McpCatalogTransport::Http, McpInstallKind::Remote) => {
                if self.install.url.as_deref().unwrap_or_default().is_empty() {
                    return Err(missing("url"));
                }
            }
            _ => {
                return Err(format!(
                    "MCP catalog entry `{}` pairs a {:?} transport with a {:?} install",
                    self.id, self.transport, self.install.kind
                ))
            }
        }
        for name in self
            .install
            .headers
            .values()
            .chain(self.install.env.values())
            .flat_map(|value| mcp_secret_references(value))
        {
            if !self
                .env
                .iter()
                .any(|variable| variable.name == name && variable.secret)
            {
                return Err(format!(
                    "MCP catalog entry `{}` refers to the undeclared secret {name}",
                    self.id
                ));
            }
        }
        Ok(())
    }

    /// Checks that a local entry pins everything it installs: the package in its own
    /// lock or requirements with the entry's integrity, a hash on every download, and
    /// downloads only from the package registry or over HTTPS.
    #[allow(clippy::too_many_lines)]
    fn check_verified_install(&self) -> Result<(), String> {
        let id = &self.id;
        let install = &self.install;
        let package = install.package.as_deref().unwrap_or_default();
        let version = install.version.as_deref().unwrap_or_default();
        if let Some(data) = &install.data {
            let declared = self
                .env
                .iter()
                .any(|variable| variable.name == data.variable && !variable.secret);
            if !declared || !relative_member(&data.file) || data.file.contains('/') {
                return Err(format!(
                    "MCP catalog entry `{id}` keeps its data in `{}` through {}, which is not one of its plain variables and a file name",
                    data.file, data.variable
                ));
            }
        }
        for (platform, asset) in &install.release {
            if !asset.url.starts_with("https://") || !is_sha256_hex(&asset.sha256) {
                return Err(format!(
                    "MCP catalog entry `{id}` has a {platform} release asset without an https URL and a sha256"
                ));
            }
            if !relative_member(&asset.member) {
                return Err(format!(
                    "MCP catalog entry `{id}` names the {platform} release member `{}`, which leaves its folder",
                    asset.member
                ));
            }
        }
        match install.kind {
            McpInstallKind::Npm if install.lock.is_empty() && install.release.is_empty() => Err(
                format!("MCP catalog entry `{id}` has neither a lock nor a release binary"),
            ),
            McpInstallKind::Npm if install.lock.is_empty() => Ok(()),
            McpInstallKind::Npm => {
                if install
                    .bin
                    .as_deref()
                    .is_none_or(|bin| !relative_member(bin))
                {
                    return Err(format!("MCP catalog entry `{id}` has no program (`bin`)"));
                }
                for entry in &install.lock {
                    let in_tree = entry
                        .path
                        .strip_prefix("node_modules/")
                        .is_some_and(relative_member);
                    if !in_tree
                        || !entry.resolved.starts_with(NPM_REGISTRY)
                        || !entry.integrity.starts_with("sha512-")
                    {
                        return Err(format!(
                            "MCP catalog entry `{id}` locks `{}` without a registry tarball and a sha512 integrity",
                            entry.path
                        ));
                    }
                }
                let own = install
                    .lock
                    .iter()
                    .find(|entry| entry.path == format!("node_modules/{package}"));
                match own {
                    Some(entry)
                        if entry.version == version
                            && Some(entry.integrity.as_str()) == install.integrity.as_deref() =>
                    {
                        Ok(())
                    }
                    _ => Err(format!(
                        "MCP catalog entry `{id}` does not lock {package}@{version} with its integrity"
                    )),
                }
            }
            McpInstallKind::Uv => {
                for requirement in &install.requirements {
                    let hashed = !requirement.hashes.is_empty()
                        && requirement
                            .hashes
                            .iter()
                            .all(|hash| hash.strip_prefix("sha256:").is_some_and(is_sha256_hex));
                    if !hashed {
                        return Err(format!(
                            "MCP catalog entry `{id}` requires {} without sha256 hashes",
                            requirement.name
                        ));
                    }
                }
                let own = install.requirements.iter().find(|requirement| {
                    normalize_package(&requirement.name) == normalize_package(package)
                });
                match (own, install.integrity.as_deref()) {
                    (Some(requirement), Some(integrity))
                        if requirement.version == version
                            && requirement.hashes.iter().any(|hash| hash == integrity) =>
                    {
                        Ok(())
                    }
                    _ => Err(format!(
                        "MCP catalog entry `{id}` does not require {package}=={version} with its hash"
                    )),
                }
            }
            McpInstallKind::Binary if install.release.is_empty() => {
                Err(format!("MCP catalog entry `{id}` has no release binary"))
            }
            McpInstallKind::Docker
                if !install
                    .integrity
                    .as_deref()
                    .and_then(|digest| digest.strip_prefix("sha256:"))
                    .is_some_and(is_sha256_hex) =>
            {
                Err(format!(
                    "MCP catalog entry `{id}` has no image digest (`sha256:<hex>`)"
                ))
            }
            _ => Ok(()),
        }
    }

    /// A remote server (its `write` tools change something outside the workspace).
    #[must_use]
    pub fn is_remote(&self) -> bool {
        self.transport == McpCatalogTransport::Http
    }

    /// The permission a call of a tool in `tier` needs on this entry's server.
    #[must_use]
    pub fn permission_mode(&self, tier: McpToolTier) -> PermissionMode {
        tier.permission_mode(self.is_remote() || self.writes_outside_workspace)
    }

    /// The tier the entry pins for `tool`, or `None` when it does not list it.
    #[must_use]
    pub fn pinned_tier(&self, tool: &str) -> Option<McpToolTier> {
        self.tools.get(tool).copied()
    }

    /// `<package>@<version>`, or the URL of a remote server.
    #[must_use]
    pub fn pinned_label(&self) -> String {
        match (
            &self.install.package,
            &self.install.version,
            &self.install.url,
        ) {
            (Some(package), Some(version), _) => format!("{package}@{version}"),
            (_, _, Some(url)) => url.clone(),
            _ => self.id.clone(),
        }
    }

    /// The command and arguments that start a stdio entry on this platform.
    #[must_use]
    pub fn stdio_command(&self) -> Option<(String, Vec<String>)> {
        self.stdio_command_for(cfg!(windows))
    }

    fn stdio_command_for(&self, windows: bool) -> Option<(String, Vec<String>)> {
        let package = format!(
            "{}@{}",
            self.install.package.as_deref()?,
            self.install.version.as_deref()?
        );
        let (command, mut args) = match self.install.kind {
            McpInstallKind::Npm if windows => (
                "cmd.exe".to_string(),
                ["/d", "/s", "/c", "npx", "-y"]
                    .into_iter()
                    .map(str::to_string)
                    .chain([package])
                    .collect::<Vec<_>>(),
            ),
            McpInstallKind::Npm => ("npx".to_string(), vec!["-y".to_string(), package]),
            McpInstallKind::Uv => ("uvx".to_string(), vec![package]),
            // The settings name the program; it starts from the verified copy.
            McpInstallKind::Binary => (self.install.package.clone()?, Vec::new()),
            McpInstallKind::Docker => (
                "docker".to_string(),
                vec![
                    "run".to_string(),
                    "-i".to_string(),
                    "--rm".to_string(),
                    format!(
                        "{}@{}",
                        self.install.package.as_deref()?,
                        self.install.integrity.as_deref()?
                    ),
                ],
            ),
            McpInstallKind::Remote => return None,
        };
        args.extend(self.install.args.iter().cloned());
        Some((command, args))
    }

    /// The `mcpServers` entry to write, given the variables the user supplied (`values`,
    /// secrets included: they are written only as `${secret:NAME}`). A header or
    /// environment value that refers to a secret the user did not supply is left out.
    #[must_use]
    pub fn settings_entry(&self, values: &BTreeMap<String, String>) -> Value {
        let supplied = |value: &str| {
            mcp_secret_references(value)
                .iter()
                .all(|name| values.contains_key(name))
        };
        match self.transport {
            McpCatalogTransport::Http => {
                let mut entry = Map::new();
                entry.insert("type".to_string(), json!("http"));
                entry.insert(
                    "url".to_string(),
                    json!(self.install.url.clone().unwrap_or_default()),
                );
                let headers = self
                    .install
                    .headers
                    .iter()
                    .filter(|(_, value)| supplied(value))
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect::<Map<_, _>>();
                if !headers.is_empty() {
                    entry.insert("headers".to_string(), Value::Object(headers));
                }
                Value::Object(entry)
            }
            McpCatalogTransport::Stdio => {
                let (command, args) = self.stdio_command().unwrap_or_default();
                let mut env = self
                    .install
                    .env
                    .iter()
                    .filter(|(_, value)| supplied(value))
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect::<Map<_, _>>();
                for variable in &self.env {
                    let value = if variable.secret {
                        values
                            .contains_key(&variable.name)
                            .then(|| mcp_secret_reference(&variable.name))
                    } else {
                        values
                            .get(&variable.name)
                            .cloned()
                            .or_else(|| variable.default.clone())
                    };
                    if let Some(value) = value {
                        env.insert(variable.name.clone(), json!(value));
                    }
                }
                let mut entry = Map::new();
                entry.insert("command".to_string(), json!(command));
                entry.insert("args".to_string(), json!(args));
                if !env.is_empty() {
                    entry.insert("env".to_string(), Value::Object(env));
                }
                Value::Object(entry)
            }
        }
    }

    fn matches(&self, config: &McpServerConfig) -> bool {
        match (self.transport, config) {
            (McpCatalogTransport::Stdio, McpServerConfig::Stdio(stdio)) => {
                let launches = [false, true].into_iter().any(|windows| {
                    self.stdio_command_for(windows)
                        .is_some_and(|(command, args)| {
                            command == stdio.command && args == stdio.args
                        })
                });
                launches
                    && stdio.env.iter().all(|(name, value)| {
                        if let Some(fixed) = self.install.env.get(name) {
                            return fixed == value;
                        }
                        self.env.iter().any(|variable| {
                            variable.name == *name
                                && (!variable.secret || *value == mcp_secret_reference(name))
                        })
                    })
            }
            (McpCatalogTransport::Http, McpServerConfig::Http(remote)) => {
                self.install.url.as_deref() == Some(remote.url.as_str())
                    && remote.headers_helper.is_none()
                    && remote
                        .headers
                        .iter()
                        .all(|(name, value)| self.install.headers.get(name) == Some(value))
            }
            _ => false,
        }
    }
}

/// Verifies a catalog update the gateway serves: `signature_hex` is the ed25519
/// signature (hex) of exactly `body`, by [`MCP_CATALOG_UPDATE_PUBLIC_KEY`], and the
/// catalog's version is newer than `current_version` (the catalog in use), so an older
/// signed catalog cannot be served again to roll entries back. Refused while the key is
/// unset.
pub fn verify_mcp_catalog_update(
    body: &[u8],
    signature_hex: &str,
    current_version: &str,
) -> Result<McpCatalog, String> {
    verify_mcp_catalog_update_with(
        MCP_CATALOG_UPDATE_PUBLIC_KEY.as_ref(),
        body,
        signature_hex,
        current_version,
    )
}

/// A catalog version, `YYYY-MM-DD.N`, as something to compare: the date, then `N`.
fn catalog_version_order(version: &str) -> Option<(&str, u32)> {
    let (date, serial) = version.split_once('.')?;
    let well_formed = date.len() == 10
        && date.char_indices().all(|(index, c)| match index {
            4 | 7 => c == '-',
            _ => c.is_ascii_digit(),
        });
    if !well_formed || serial.is_empty() || !serial.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((date, serial.parse().ok()?))
}

fn verify_mcp_catalog_update_with(
    public_key: Option<&[u8; 32]>,
    body: &[u8],
    signature_hex: &str,
    current_version: &str,
) -> Result<McpCatalog, String> {
    let Some(public_key) = public_key else {
        return Err(
            "catalog updates are refused: this build has no catalog signing key".to_string(),
        );
    };
    let signature = decode_hex(signature_hex.trim())
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(|| "the catalog signature is not 64 bytes of hex".to_string())?;
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
        .verify(body, &signature)
        .map_err(|_| "the catalog signature does not match".to_string())?;
    let text = std::str::from_utf8(body).map_err(|_| "the catalog is not UTF-8".to_string())?;
    let catalog = McpCatalog::parse(text)?;
    let current = catalog_version_order(current_version).ok_or_else(|| {
        format!("the catalog in use has no comparable version ({current_version})")
    })?;
    match catalog_version_order(&catalog.version) {
        Some(update) if update > current => Ok(catalog),
        Some(_) => Err(format!(
            "the catalog update {} is not newer than the catalog in use ({current_version})",
            catalog.version
        )),
        None => Err(format!(
            "the catalog update's version {} is not YYYY-MM-DD.N",
            catalog.version
        )),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect()
}

/// Hex of `bytes`, for signatures in tests and tools.
#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ring::signature::KeyPair;
    use serde_json::json;

    use super::{
        encode_hex, mcp_secret_references, verify_mcp_catalog_update,
        verify_mcp_catalog_update_with, McpCatalog, McpToolTier, BUILTIN_CATALOG_JSON,
    };
    use crate::config::{McpRemoteServerConfig, McpServerConfig, McpStdioServerConfig};
    use crate::permissions::PermissionMode;

    #[test]
    fn the_builtin_catalog_parses_and_every_package_is_pinned() {
        let catalog = McpCatalog::builtin();
        assert!(catalog.connectors.len() >= 10);
        for entry in &catalog.connectors {
            if entry.install.package.is_some() {
                let version = entry.install.version.as_deref().expect("pinned");
                assert!(
                    version.chars().next().is_some_and(|c| c.is_ascii_digit()),
                    "{} pins an exact version, not {version}",
                    entry.id
                );
                assert!(entry.install.integrity.is_some(), "{}", entry.id);
            }
            assert!(!entry.licence.is_empty() && entry.source.starts_with("https://"));
        }
        for id in [
            "github",
            "filesystem",
            "git",
            "fetch",
            "memory",
            "playwright",
        ] {
            assert!(catalog.get(id).is_some(), "{id}");
        }
    }

    #[test]
    fn search_matches_every_word_in_id_name_publisher_or_description() {
        let catalog = McpCatalog::builtin();
        assert_eq!(catalog.search("").len(), catalog.connectors.len());
        let ids = |query: &str| {
            catalog
                .search(query)
                .into_iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("GITHUB"), vec!["github"]);
        assert!(ids("documentation").contains(&"aws-docs"));
        assert!(ids("nothing-like-this").is_empty());
    }

    #[test]
    fn a_settings_entry_holds_secret_references_and_is_recognised_as_its_entry() {
        let catalog = McpCatalog::builtin();
        let github = catalog.get("github").unwrap();
        let values = BTreeMap::from([(
            "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
            "ghp_sentinel".to_string(),
        )]);
        let written = github.settings_entry(&values);
        assert_eq!(
            written,
            json!({
                "type": "http",
                "url": "https://api.githubcopilot.com/mcp/",
                "headers": {"Authorization": "Bearer ${secret:GITHUB_PERSONAL_ACCESS_TOKEN}"}
            })
        );
        assert!(!written.to_string().contains("ghp_sentinel"));

        let config = McpServerConfig::Http(McpRemoteServerConfig {
            url: "https://api.githubcopilot.com/mcp/".to_string(),
            headers: BTreeMap::from([(
                "Authorization".to_string(),
                "Bearer ${secret:GITHUB_PERSONAL_ACCESS_TOKEN}".to_string(),
            )]),
            headers_helper: None,
            oauth: None,
        });
        assert_eq!(
            catalog.entry_for_config(&config).map(|e| e.id.as_str()),
            Some("github")
        );

        // A header the entry does not write, or a headers helper, is not that entry.
        let mut extra = config.clone();
        if let McpServerConfig::Http(remote) = &mut extra {
            remote
                .headers
                .insert("X-Other".to_string(), "1".to_string());
        }
        assert!(catalog.entry_for_config(&extra).is_none());
        let mut helper = config;
        if let McpServerConfig::Http(remote) = &mut helper {
            remote.headers_helper = Some("print-headers".to_string());
        }
        assert!(catalog.entry_for_config(&helper).is_none());
    }

    #[test]
    fn an_optional_secret_left_out_drops_the_header_that_needs_it() {
        let context7 = McpCatalog::builtin().get("context7").unwrap();
        let written = context7.settings_entry(&BTreeMap::new());
        assert!(written.get("headers").is_none(), "{written}");
    }

    #[test]
    fn stdio_entries_match_on_either_platform_and_only_with_their_own_environment() {
        let catalog = McpCatalog::builtin();
        let aws = catalog.get("aws-docs").unwrap();
        let written = aws.settings_entry(&BTreeMap::new());
        assert_eq!(written["command"], "uvx");
        assert_eq!(
            written["args"],
            json!(["awslabs.aws-documentation-mcp-server@1.2.4"])
        );
        assert_eq!(written["env"]["AWS_DOCUMENTATION_PARTITION"], "aws");
        assert_eq!(written["env"]["FASTMCP_LOG_LEVEL"], "ERROR");

        let stdio = |command: &str, args: &[&str], env: &[(&str, &str)]| {
            McpServerConfig::Stdio(McpStdioServerConfig {
                command: command.to_string(),
                args: args.iter().map(|arg| (*arg).to_string()).collect(),
                env: env
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
                tool_call_timeout_ms: None,
                sandbox: None,
            })
        };
        let package = "@modelcontextprotocol/server-filesystem@2.1.0";
        for config in [
            stdio("npx", &["-y", package, "."], &[]),
            stdio(
                "cmd.exe",
                &["/d", "/s", "/c", "npx", "-y", package, "."],
                &[],
            ),
        ] {
            assert_eq!(
                catalog.entry_for_config(&config).map(|e| e.id.as_str()),
                Some("filesystem")
            );
        }
        // Another version, another folder or an environment the entry does not set is
        // not the vetted server.
        for config in [
            stdio(
                "npx",
                &["-y", "@modelcontextprotocol/server-filesystem@2.0.0", "."],
                &[],
            ),
            stdio("npx", &["-y", package, "/"], &[]),
            stdio(
                "npx",
                &["-y", package, "."],
                &[("NODE_OPTIONS", "--require x")],
            ),
        ] {
            assert!(catalog.entry_for_config(&config).is_none(), "{config:?}");
        }
        let uvx = "awslabs.aws-documentation-mcp-server@1.2.4";
        assert!(catalog
            .entry_for_config(&stdio("uvx", &[uvx], &[("FASTMCP_LOG_LEVEL", "DEBUG")]))
            .is_none());
        assert!(catalog
            .entry_for_config(&stdio(
                "uvx",
                &[uvx],
                &[("AWS_DOCUMENTATION_PARTITION", "aws-cn")]
            ))
            .is_some());
    }

    #[test]
    fn tiers_become_permissions_and_remote_writes_ask_as_full_access() {
        assert_eq!(
            McpToolTier::Read.permission_mode(true),
            PermissionMode::ReadOnly
        );
        assert_eq!(
            McpToolTier::Write.permission_mode(false),
            PermissionMode::WorkspaceWrite
        );
        assert_eq!(
            McpToolTier::Write.permission_mode(true),
            PermissionMode::DangerFullAccess
        );
        assert_eq!(
            McpToolTier::Destructive.permission_mode(false),
            PermissionMode::DangerFullAccess
        );
        let catalog = McpCatalog::builtin();
        // Memory keeps its graph in its package folder (or MEMORY_FILE_PATH), and
        // codebase-memory its index in the user's cache: outside the workspace.
        for (id, tool) in [
            ("memory", "create_entities"),
            ("memory", "unlisted_tool"),
            ("codebase-memory", "index_repository"),
        ] {
            let entry = catalog.get(id).unwrap();
            let tier = entry.pinned_tier(tool).unwrap_or(entry.default_tier);
            assert_eq!(tier, McpToolTier::Write, "{id} {tool}");
            assert_eq!(
                entry.permission_mode(tier),
                PermissionMode::DangerFullAccess,
                "{id} {tool}"
            );
        }
        let git = catalog.get("git").unwrap();
        assert_eq!(
            git.permission_mode(git.pinned_tier("git_commit").unwrap()),
            PermissionMode::WorkspaceWrite
        );
        let github = catalog.get("github").unwrap();
        assert_eq!(github.pinned_tier("get_me"), Some(McpToolTier::Read));
        assert_eq!(
            github.pinned_tier("merge_pull_request"),
            Some(McpToolTier::Destructive)
        );
        assert_eq!(github.pinned_tier("brand_new_tool"), None);
    }

    #[test]
    fn a_catalog_with_a_secret_it_does_not_declare_or_an_unpinned_package_is_refused() {
        let undeclared = r#"{"version":"t","connectors":[{"id":"x","name":"X","description":"","publisher":"p","licence":"MIT","source":"https://example.com","transport":"http","install":{"kind":"remote","url":"https://example.com/mcp","headers":{"Authorization":"Bearer ${secret:TOKEN}"}},"default_tier":"external"}]}"#;
        assert!(McpCatalog::parse(undeclared).unwrap_err().contains("TOKEN"));
        let unpinned = r#"{"version":"t","connectors":[{"id":"x","name":"X","description":"","publisher":"p","licence":"MIT","source":"https://example.com","transport":"stdio","install":{"kind":"npm","package":"x"},"default_tier":"write"}]}"#;
        assert!(McpCatalog::parse(unpinned).unwrap_err().contains("version"));
    }

    /// A local entry as JSON with `install` replaced, to check what installs need.
    fn with_install(install: &serde_json::Value) -> String {
        json!({"version": "t", "connectors": [{
            "id": "x", "name": "X", "description": "", "publisher": "p", "licence": "MIT",
            "source": "https://example.com", "transport": "stdio", "install": install,
            "default_tier": "write"
        }]})
        .to_string()
    }

    #[test]
    fn a_local_entry_must_pin_every_download_it_installs() {
        let sha = "a".repeat(64);
        let integrity = "sha512-AAAA";
        let lock = |path: &str, resolved: &str, integrity: &str| json!([{"path": path, "version": "1.0.0", "resolved": resolved, "integrity": integrity}]);
        let npm = |lock: serde_json::Value| {
            with_install(&json!({"kind": "npm", "package": "x", "version": "1.0.0",
                "integrity": integrity, "bin": "cli.js", "lock": lock}))
        };
        let registry = "https://registry.npmjs.org/x/-/x-1.0.0.tgz";
        assert!(McpCatalog::parse(&npm(lock("node_modules/x", registry, integrity))).is_ok());
        for (bad, why) in [
            (json!([]), "neither a lock"),
            (
                lock("node_modules/y", registry, integrity),
                "does not lock x@1.0.0",
            ),
            (
                lock("node_modules/x", registry, "sha512-BBBB"),
                "does not lock x@1.0.0",
            ),
            (
                lock("node_modules/x", "https://evil.example/x.tgz", integrity),
                "registry tarball",
            ),
            (
                lock("node_modules/../x", registry, integrity),
                "registry tarball",
            ),
            (lock("node_modules/x", registry, "sha1-AAAA"), "sha512"),
        ] {
            let error = McpCatalog::parse(&npm(bad)).unwrap_err();
            assert!(error.contains(why), "{why}: {error}");
        }

        let uv = |hashes: serde_json::Value| {
            with_install(&json!({"kind": "uv", "package": "x_server", "version": "2",
                "integrity": format!("sha256:{sha}"),
                "requirements": [{"name": "x-server", "version": "2", "hashes": hashes}]}))
        };
        assert!(McpCatalog::parse(&uv(json!([format!("sha256:{sha}")]))).is_ok());
        assert!(McpCatalog::parse(&uv(json!([])))
            .unwrap_err()
            .contains("without sha256"));
        assert!(
            McpCatalog::parse(&uv(json!([format!("sha256:{}", "b".repeat(64))])))
                .unwrap_err()
                .contains("with its hash")
        );

        let binary = |member: &str, digest: &str| {
            with_install(&json!({"kind": "binary", "package": "x", "version": "1",
                "release": {"linux-x86_64": {"url": "https://example.com/x.tar.gz",
                    "sha256": digest, "archive": "tar.gz", "member": member}}}))
        };
        assert!(McpCatalog::parse(&binary("x", &sha)).is_ok());
        assert!(McpCatalog::parse(&binary("../x", &sha))
            .unwrap_err()
            .contains("leaves its folder"));
        assert!(McpCatalog::parse(&binary("x", "nothex"))
            .unwrap_err()
            .contains("sha256"));
        let no_release = with_install(&json!({"kind": "binary", "package": "x", "version": "1"}));
        assert!(McpCatalog::parse(&no_release)
            .unwrap_err()
            .contains("no release"));

        let docker = |digest: &str| {
            with_install(&json!({"kind": "docker", "package": "ghcr.io/example/x",
                "version": "1.0", "integrity": digest}))
        };
        assert!(McpCatalog::parse(&docker("latest"))
            .unwrap_err()
            .contains("image digest"));
        let catalog = McpCatalog::parse(&docker(&format!("sha256:{sha}"))).unwrap();
        let (command, args) = catalog.connectors[0].stdio_command().unwrap();
        assert_eq!(command, "docker");
        assert_eq!(
            args,
            vec![
                "run".to_string(),
                "-i".to_string(),
                "--rm".to_string(),
                format!("ghcr.io/example/x@sha256:{sha}")
            ]
        );
    }

    #[test]
    fn a_data_file_is_named_by_a_plain_variable_and_kept_in_its_data_folder() {
        let sha = "a".repeat(64);
        let with_data = |data: serde_json::Value| {
            let mut value: serde_json::Value = serde_json::from_str(&with_install(&json!({
                "kind": "binary", "package": "x", "version": "1",
                "release": {"linux-x86_64": {"url": "https://example.com/x.tar.gz",
                    "sha256": sha, "archive": "tar.gz", "member": "x"}},
                "data": data
            })))
            .unwrap();
            value["connectors"][0]["env"] =
                json!([{"name": "X_FILE", "label": "", "secret": false, "required": false}]);
            value.to_string()
        };
        assert!(
            McpCatalog::parse(&with_data(json!({"variable": "X_FILE", "file": "x.db"}))).is_ok()
        );
        for bad in [
            json!({"variable": "OTHER", "file": "x.db"}),
            json!({"variable": "X_FILE", "file": "../x.db"}),
            json!({"variable": "X_FILE", "file": "a/x.db"}),
        ] {
            assert!(McpCatalog::parse(&with_data(bad))
                .unwrap_err()
                .contains("keeps its data"));
        }
        let memory = McpCatalog::builtin().get("memory").unwrap();
        assert_eq!(
            memory
                .install
                .data
                .as_ref()
                .map(|data| data.variable.as_str()),
            Some("MEMORY_FILE_PATH")
        );
    }

    #[test]
    fn secret_references_are_found_in_order() {
        assert_eq!(
            mcp_secret_references("Bearer ${secret:A} ${HOME} ${secret:B}"),
            vec!["A", "B"]
        );
        assert!(mcp_secret_references("${secret:}${secret:open").is_empty());
    }

    #[test]
    fn remote_updates_need_a_valid_signature_and_are_refused_without_a_key() {
        // A test key from a fixed seed; the production key is an owner step.
        let pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap();
        let public: [u8; 32] = pair.public_key().as_ref().try_into().unwrap();
        let body = BUILTIN_CATALOG_JSON.as_bytes();
        let signature = encode_hex(pair.sign(body).as_ref());
        let older = "2026-01-01.1";

        let catalog =
            verify_mcp_catalog_update_with(Some(&public), body, &signature, older).unwrap();
        assert_eq!(&catalog, McpCatalog::builtin());

        let mut altered = body.to_vec();
        altered[10] ^= 1;
        assert!(
            verify_mcp_catalog_update_with(Some(&public), &altered, &signature, older)
                .unwrap_err()
                .contains("does not match")
        );
        assert!(verify_mcp_catalog_update_with(Some(&public), body, "abcd", older).is_err());
        assert!(verify_mcp_catalog_update(body, &signature, older)
            .unwrap_err()
            .contains("no catalog signing key"));

        // A validly signed catalog that is not newer is a rollback.
        let current = McpCatalog::builtin().version.as_str();
        for in_use in [current, "2026-11-06.1", "2027-01-01.1"] {
            let error = verify_mcp_catalog_update_with(Some(&public), body, &signature, in_use)
                .unwrap_err();
            assert!(error.contains("is not newer"), "{in_use}: {error}");
        }
        let unversioned = McpCatalog {
            version: "latest".to_string(),
            ..McpCatalog::builtin().clone()
        };
        let unversioned = serde_json::to_vec(&unversioned).unwrap();
        let signed = encode_hex(pair.sign(&unversioned).as_ref());
        assert!(
            verify_mcp_catalog_update_with(Some(&public), &unversioned, &signed, older)
                .unwrap_err()
                .contains("YYYY-MM-DD.N")
        );
        assert!(
            super::catalog_version_order("2026-11-04.10")
                > super::catalog_version_order("2026-11-04.9")
        );
        assert!(super::catalog_version_order("2026-11-4.1").is_none());
    }
}
