//! Pins for stdio MCP servers that are not catalog connectors. A catalog connector is
//! pinned by the catalog (version and hashes); any other stdio server runs whatever its
//! command resolves to today, so `/mcp list` marks it `unpinned`. `/mcp pin <name>`
//! records what it runs, keyed by its launch spec (command, arguments, environment):
//!
//! - `npx [-y] <package>[@version] ...`: npm resolves the package's dependency tree once
//!   (`npm install --package-lock-only --ignore-scripts`), Neuron installs it verified
//!   (as a catalog npm entry) and the server starts from that install from then on, at
//!   that version, offline.
//! - `uvx <package>[@version|==version] ...`: `uv pip compile --generate-hashes` resolves
//!   it once; it installs and starts as a catalog Python entry does.
//! - Anything else: the sha256 of the program file and of each script file its
//!   arguments name. A start where one of them changed is refused until it is pinned
//!   again. These files are resolved from the workspace, so the key also covers their
//!   paths: the same `node server.js` in another workspace runs another file and is
//!   unpinned there.
//!
//! A changed launch spec is a different key, so the server shows as unpinned again.
//! Pins are in `<config home>/mcp/pins.json`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::config::{ConfigLoader, ConfigSource, McpServerConfig};
use crate::mcp_catalog::{McpCatalog, McpNpmLockEntry, McpPythonRequirement, NPM_REGISTRY};
use crate::mcp_install::{
    McpFetch, McpHttpFetch, McpInstallStore, McpVerifiedSource, McpVerifiedSpec,
};
use crate::mcp_toggle::{read_object, write_object};

/// The pins file in the install store.
pub const MCP_PINS_FILE: &str = "pins.json";

/// What a pin records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpPin {
    /// An npm package at a version, with its dependency tree.
    Npm {
        package: String,
        version: String,
        bin: String,
        lock: Vec<McpNpmLockEntry>,
        args: Vec<String>,
    },
    /// A Python package at a version, with its hashed requirements.
    Python {
        package: String,
        version: String,
        script: String,
        requirements: Vec<McpPythonRequirement>,
        args: Vec<String>,
    },
    /// Program and script files by path, with their sha256.
    Program { files: BTreeMap<String, String> },
}

impl McpPin {
    /// `<package>@<version>`, or the files it checks.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Npm {
                package, version, ..
            } => format!("{package}@{version} (npm)"),
            Self::Python {
                package, version, ..
            } => format!("{package}=={version} (Python)"),
            Self::Program { files } => files
                .iter()
                .map(|(path, sha256)| {
                    format!("{path} (sha256 {})", &sha256[..12.min(sha256.len())])
                })
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// The verified install an npm or Python pin starts from, in the folder `name`.
    fn verified_spec(&self, name: &str) -> Option<McpVerifiedSpec> {
        match self {
            Self::Npm {
                package,
                version,
                bin,
                lock,
                args,
            } => Some(McpVerifiedSpec {
                name: name.to_string(),
                version: version.clone(),
                source: McpVerifiedSource::Npm {
                    package: package.clone(),
                    bin: bin.clone(),
                    lock: lock.clone(),
                },
                args: args.clone(),
            }),
            Self::Python {
                version,
                script,
                requirements,
                args,
                ..
            } => Some(McpVerifiedSpec {
                name: name.to_string(),
                version: version.clone(),
                source: McpVerifiedSource::Python {
                    script: script.clone(),
                    requirements: requirements.clone(),
                },
                args: args.clone(),
            }),
            Self::Program { .. } => None,
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The key of a stdio launch spec: the sha256 (hex) of its command, arguments and
/// environment.
#[must_use]
pub fn mcp_pin_key(command: &str, args: &[String], env: &BTreeMap<String, String>) -> String {
    hex(&Sha256::digest(
        json!([command, args, env]).to_string().as_bytes(),
    ))
}

/// The install folder of a pin: `pinned-<first 16 hex of its key>`.
fn pin_folder(key: &str) -> String {
    format!("pinned-{}", &key[..16.min(key.len())])
}

/// The pins in an install store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPins {
    path: PathBuf,
}

impl McpPins {
    #[must_use]
    pub fn in_store(store: &McpInstallStore) -> Self {
        Self {
            path: store.root().join(MCP_PINS_FILE),
        }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The pin of the launch spec `key`.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<McpPin> {
        let object = read_object(&self.path).ok()?;
        serde_json::from_value(object.get(key)?.get("pin")?.clone()).ok()
    }

    fn set(&self, key: &str, server: &str, pin: &McpPin) -> Result<(), String> {
        let mut object = read_object(&self.path)?;
        object.insert(key.to_string(), json!({"server": server, "pin": pin}));
        write_object(&self.path, &object)
    }
}

/// The key a launch spec's pin is kept under in `cwd`: [`mcp_pin_key`] for `npx` and
/// `uvx` (the package does not depend on the workspace), else the key of the launch
/// spec with the program and script files it resolves to there, so a pin made in one
/// workspace does not vouch for another workspace's file of the same name.
fn launch_key(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> String {
    let launcher = launcher_words("npx", command, args).is_some()
        || launcher_words("uvx", command, args).is_some();
    match program_files(command, args, cwd) {
        Ok(files) if !launcher => hex(&Sha256::digest(
            json!([command, args, env, files]).to_string().as_bytes(),
        )),
        _ => mcp_pin_key(command, args, env),
    }
}

/// The pin of a launch spec in `cwd` and its key. A program pin counts only while its
/// files are exactly the ones the spec resolves to in `cwd`.
fn pin_for(
    store: &McpInstallStore,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<(String, McpPin)> {
    let key = launch_key(command, args, env, cwd);
    let pin = McpPins::in_store(store).get(&key)?;
    if let McpPin::Program { files } = &pin {
        let resolved = program_files(command, args, cwd).ok()?;
        if !files.keys().eq(resolved.iter()) {
            return None;
        }
    }
    Some((key, pin))
}

/// What a stdio launch spec starts from once pins apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpPinnedLaunch {
    /// Not pinned: it starts as written.
    Unpinned,
    /// An npm or Python pin: its verified install.
    Verified(McpVerifiedSpec),
    /// A program pin whose files all still match: it starts as written.
    Checked,
}

/// Applies the pin of a stdio launch spec, if it has one: a program pin's files are
/// checked (relative paths from `cwd`), and an npm or Python pin gives its install.
pub(crate) fn mcp_pinned_launch(
    store: &McpInstallStore,
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<McpPinnedLaunch, String> {
    let Some((key, pin)) = pin_for(store, command, args, env, cwd) else {
        return Ok(McpPinnedLaunch::Unpinned);
    };
    if let McpPin::Program { files } = &pin {
        for (path, sha256) in files {
            let actual = fs::read(path)
                .map(|bytes| hex(&Sha256::digest(&bytes)))
                .map_err(|_| format!("the pinned file {path} is missing"))?;
            if &actual != sha256 {
                return Err(format!(
                    "the pinned file {path} changed since it was pinned (sha256 {actual}, pinned {sha256}); check it, then pin it again with /mcp pin"
                ));
            }
        }
        return Ok(McpPinnedLaunch::Checked);
    }
    Ok(pin
        .verified_spec(&pin_folder(&key))
        .map_or(McpPinnedLaunch::Unpinned, McpPinnedLaunch::Verified))
}

/// Whether the configured server `config` from `scope` is unpinned in the workspace
/// `cwd`: a stdio server that Neuron does not ship, that is not a catalog connector, and
/// whose launch spec has no pin there. A remote server runs what its host serves, so it
/// is never unpinned.
#[must_use]
pub fn mcp_server_unpinned(
    store: &McpInstallStore,
    scope: ConfigSource,
    config: &McpServerConfig,
    cwd: &Path,
) -> bool {
    let McpServerConfig::Stdio(stdio) = config else {
        return false;
    };
    scope != ConfigSource::Bundled
        && McpCatalog::builtin().entry_for_config(config).is_none()
        && pin_for(store, &stdio.command, &stdio.args, &stdio.env, cwd).is_none()
}

/// How a pin resolves a package: npm and uv in the CLI, fixtures in tests.
pub trait McpPinResolve {
    /// The `package-lock.json` (v3) text of an empty project that depends on `spec`.
    fn npm_lock(&self, spec: &str) -> Result<String, String>;
    /// `uv pip compile --generate-hashes` output for `requirement`.
    fn python_requirements(&self, requirement: &str) -> Result<String, String>;
}

/// Resolves with the `npm` and `uv` on the path, in a temporary folder, running no
/// package scripts.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpToolResolve;

impl McpPinResolve for McpToolResolve {
    fn npm_lock(&self, spec: &str) -> Result<String, String> {
        let dir = std::env::temp_dir().join(format!(
            "neuron-mcp-pin-{}-{}",
            std::process::id(),
            &hex(&Sha256::digest(spec.as_bytes()))[..8]
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        fs::write(dir.join("package.json"), "{\"private\":true}\n")
            .map_err(|error| error.to_string())?;
        let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
        let output = Command::new(npm)
            .args([
                "install",
                "--package-lock-only",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--save-exact",
                spec,
            ])
            .current_dir(&dir)
            .output()
            .map_err(|error| format!("cannot run npm (install Node.js to pin it): {error}"));
        let result = output.and_then(|output| {
            if output.status.success() {
                fs::read_to_string(dir.join("package-lock.json")).map_err(|error| error.to_string())
            } else {
                Err(format!(
                    "npm could not resolve {spec}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ))
            }
        });
        let _ = fs::remove_dir_all(&dir);
        result
    }

    fn python_requirements(&self, requirement: &str) -> Result<String, String> {
        let dir = std::env::temp_dir().join(format!("neuron-mcp-pin-{}", std::process::id()));
        fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let input = dir.join("requirements.in");
        fs::write(&input, format!("{requirement}\n")).map_err(|error| error.to_string())?;
        let uv = std::env::var("NEURON_MCP_UV").unwrap_or_else(|_| "uv".to_string());
        let output = Command::new(&uv)
            .args(["pip", "compile", "--universal", "--generate-hashes"])
            .args([
                "--python-version",
                "3.10",
                "--no-header",
                "--no-annotate",
                "--quiet",
            ])
            .arg(&input)
            .current_dir(&dir)
            .output()
            .map_err(|error| format!("cannot run uv (install uv to pin it): {error}"));
        let _ = fs::remove_dir_all(&dir);
        let output = output?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(format!(
                "uv could not resolve {requirement}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// The program name of a command: its file name, lower case, without `.exe`/`.cmd`.
fn program_name(command: &str) -> String {
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let name = name.to_ascii_lowercase();
    [".exe", ".cmd", ".bat"]
        .into_iter()
        .find_map(|suffix| name.strip_suffix(suffix).map(str::to_string))
        .unwrap_or(name)
}

/// The words after the launcher `launcher` (`npx`, `uvx`), started directly or through
/// `cmd.exe /c`.
fn launcher_words<'a>(launcher: &str, command: &str, args: &'a [String]) -> Option<&'a [String]> {
    if program_name(command) == launcher {
        return Some(args);
    }
    if program_name(command) != "cmd" {
        return None;
    }
    let at = args.iter().position(|arg| program_name(arg) == launcher)?;
    args[..at]
        .iter()
        .all(|arg| arg.starts_with('/'))
        .then(|| &args[at + 1..])
}

/// The package word and the server's own arguments after a launcher's flags (`-y`,
/// `--yes`, `-q`, `--quiet`). Any other flag before the package is refused.
fn package_and_args(words: &[String]) -> Result<(String, Vec<String>), String> {
    let mut rest = words.iter();
    for word in rest.by_ref() {
        match word.as_str() {
            "-y" | "--yes" | "-q" | "--quiet" => {}
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "the launcher option `{flag}` is not understood by /mcp pin; name the package right after the launcher"
                ))
            }
            package => return Ok((package.to_string(), rest.cloned().collect())),
        }
    }
    Err("the launcher names no package".to_string())
}

/// An npm package word as `(name, version or range)`: `@scope/x@1.2` gives
/// `("@scope/x", "1.2")`, `x` gives `("x", "latest")`.
fn npm_package(word: &str) -> (String, String) {
    match word.rfind('@') {
        Some(at) if at > 0 => (word[..at].to_string(), word[at + 1..].to_string()),
        _ => (word.to_string(), "latest".to_string()),
    }
}

/// A Python package word as a requirement: `x@1.2` and `x==1.2` give `x==1.2`.
fn python_requirement(word: &str) -> (String, String) {
    if let Some((name, version)) = word.split_once("==") {
        return (name.to_string(), format!("{name}=={version}"));
    }
    match word.split_once('@') {
        Some((name, version)) if !version.is_empty() && version != "latest" => {
            (name.to_string(), format!("{name}=={version}"))
        }
        Some((name, _)) => (name.to_string(), name.to_string()),
        None => (word.to_string(), word.to_string()),
    }
}

/// The lock entries of a `package-lock.json` (v3), and the program of `package`.
fn parse_npm_lock(
    text: &str,
    package: &str,
) -> Result<(String, String, Vec<McpNpmLockEntry>), String> {
    let lock: Value =
        serde_json::from_str(text).map_err(|error| format!("npm wrote no lock: {error}"))?;
    let packages = lock
        .get("packages")
        .and_then(Value::as_object)
        .ok_or_else(|| "the npm lock lists no packages".to_string())?;
    let mut entries = Vec::new();
    for (path, entry) in packages.iter().filter(|(path, _)| !path.is_empty()) {
        let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_string);
        let list = |key: &str| {
            entry
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let (Some(version), Some(resolved), Some(integrity)) =
            (text("version"), text("resolved"), text("integrity"))
        else {
            return Err(format!(
                "the npm lock has no tarball and integrity for {path}"
            ));
        };
        if !resolved.starts_with(NPM_REGISTRY) || !integrity.starts_with("sha512-") {
            return Err(format!(
                "{path} does not come from the npm registry with a sha512 integrity"
            ));
        }
        entries.push(McpNpmLockEntry {
            path: path.clone(),
            version,
            resolved,
            integrity,
            os: list("os"),
            cpu: list("cpu"),
            optional: entry.get("optional").and_then(Value::as_bool) == Some(true),
        });
    }
    let own = packages
        .get(&format!("node_modules/{package}"))
        .ok_or_else(|| format!("the npm lock does not list {package}"))?;
    let version = own
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let bin = match own.get("bin") {
        Some(Value::String(bin)) => bin.clone(),
        Some(Value::Object(bins)) => {
            let short = package.rsplit('/').next().unwrap_or(package);
            bins.get(short)
                .or_else(|| (bins.len() == 1).then(|| bins.values().next()).flatten())
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| {
                    format!("{package} has several programs; it cannot be pinned by name")
                })?
        }
        _ => return Err(format!("{package} has no program (`bin`) to start")),
    };
    let bin = bin.trim_start_matches("./").to_string();
    Ok((version, bin, entries))
}

/// The requirements of `uv pip compile --generate-hashes` output.
fn parse_requirements(text: &str) -> Result<Vec<McpPythonRequirement>, String> {
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = lines.last_mut() {
                last.push(' ');
                last.push_str(line.trim());
            }
        } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            lines.push(line.to_string());
        }
    }
    lines
        .iter()
        .map(|line| {
            let line = line.replace('\\', " ");
            let spec = line.split("--hash=").next().unwrap_or_default().trim();
            let (spec, marker) = match spec.split_once(';') {
                Some((spec, marker)) => (spec.trim(), Some(marker.trim().to_string())),
                None => (spec, None),
            };
            let (name, version) = spec
                .split_once("==")
                .ok_or_else(|| format!("`{spec}` is not pinned to one version"))?;
            let hashes = line
                .split_whitespace()
                .filter_map(|word| word.strip_prefix("--hash="))
                .map(str::to_string)
                .collect::<Vec<_>>();
            if hashes.is_empty() {
                return Err(format!("{name} has no hash"));
            }
            Ok(McpPythonRequirement {
                name: name.trim().to_string(),
                version: version.trim().to_string(),
                marker,
                hashes,
            })
        })
        .collect()
}

/// The file `command` runs: itself when it names a path (from `cwd`), else the first
/// match on `PATH` (with `PATHEXT` on Windows).
fn resolve_program(command: &str, cwd: &Path) -> Option<PathBuf> {
    if command.contains(['/', '\\']) {
        let path = cwd.join(command);
        return path.is_file().then_some(path);
    }
    let extensions = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(str::to_string)
            .chain([String::new()])
            .collect::<Vec<_>>()
    } else {
        vec![String::new()]
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        extensions
            .iter()
            .map(|extension| dir.join(format!("{command}{extension}")))
            .find(|candidate| candidate.is_file())
    })
}

/// The program file and each argument that names a file, resolved from `cwd`, as
/// sorted paths.
fn program_files(command: &str, args: &[String], cwd: &Path) -> Result<Vec<String>, String> {
    let program = resolve_program(command, cwd)
        .ok_or_else(|| format!("`{command}` is not a file or on the PATH"))?;
    let mut files = std::iter::once(program)
        .chain(
            args.iter()
                .map(|arg| cwd.join(arg))
                .filter(|path| path.is_file()),
        )
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

/// The sha256 of the program file and of each argument that names a file.
fn program_pin(command: &str, args: &[String], cwd: &Path) -> Result<McpPin, String> {
    let mut files = BTreeMap::new();
    for path in program_files(command, args, cwd)? {
        let bytes = fs::read(&path).map_err(|error| format!("{path}: {error}"))?;
        files.insert(path, hex(&Sha256::digest(&bytes)));
    }
    Ok(McpPin::Program { files })
}

/// What [`pin_mcp_server`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPinOutcome {
    pub name: String,
    pub pin: McpPin,
    /// The verified install it now starts from (npm and Python pins).
    pub installed: Option<PathBuf>,
    /// It was pinned this way already.
    pub already: bool,
    pub pins_path: PathBuf,
}

/// Pins the configured stdio server `name` (see the module docs). Refused for a remote
/// server, a bundled one and a catalog connector (the catalog pins them).
pub fn pin_mcp_server(loader: &ConfigLoader, name: &str) -> Result<McpPinOutcome, String> {
    pin_mcp_server_with(
        loader,
        name,
        &McpInstallStore::in_config_home(loader.config_home()),
        &McpToolResolve,
        &McpHttpFetch,
    )
}

pub(crate) fn pin_mcp_server_with(
    loader: &ConfigLoader,
    name: &str,
    store: &McpInstallStore,
    resolve: &dyn McpPinResolve,
    fetch: &dyn McpFetch,
) -> Result<McpPinOutcome, String> {
    let name = name.trim();
    let config = loader.load().map_err(|error| error.to_string())?;
    let mcp = config.mcp();
    let server = mcp
        .get(name)
        .or_else(|| mcp.disabled().get(name))
        .or_else(|| mcp.needs_approval().get(name))
        .ok_or_else(|| format!("no MCP server named `{name}` is configured"))?;
    let McpServerConfig::Stdio(stdio) = &server.config else {
        return Err(format!(
            "`{name}` is a remote server: it runs what its host serves, so there is nothing to pin"
        ));
    };
    if server.scope == ConfigSource::Bundled
        || McpCatalog::builtin()
            .entry_for_config(&server.config)
            .is_some()
    {
        return Err(format!(
            "`{name}` is a catalog connector: the catalog pins its version and hashes already"
        ));
    }
    let key = launch_key(&stdio.command, &stdio.args, &stdio.env, loader.cwd());
    let pins = McpPins::in_store(store);
    let pin = if let Some(words) = launcher_words("npx", &stdio.command, &stdio.args) {
        let (word, args) = package_and_args(words)?;
        let (package, wanted) = npm_package(&word);
        let lock = resolve.npm_lock(&format!("{package}@{wanted}"))?;
        let (version, bin, lock) = parse_npm_lock(&lock, &package)?;
        McpPin::Npm {
            package,
            version,
            bin,
            lock,
            args,
        }
    } else if let Some(words) = launcher_words("uvx", &stdio.command, &stdio.args) {
        let (word, args) = package_and_args(words)?;
        let (package, requirement) = python_requirement(&word);
        let requirements = parse_requirements(&resolve.python_requirements(&requirement)?)?;
        let normalized = |text: &str| text.to_ascii_lowercase().replace(['_', '.'], "-");
        let version = requirements
            .iter()
            .find(|item| normalized(&item.name) == normalized(&package))
            .map(|item| item.version.clone())
            .ok_or_else(|| format!("uv did not resolve {package}"))?;
        McpPin::Python {
            script: package.clone(),
            package,
            version,
            requirements,
            args,
        }
    } else {
        program_pin(&stdio.command, &stdio.args, loader.cwd())?
    };
    let already = pins.get(&key).as_ref() == Some(&pin);
    let installed = match pin.verified_spec(&pin_folder(&key)) {
        Some(spec) => store.ensure(&spec, fetch)?.dir,
        None => None,
    };
    pins.set(&key, name, &pin)?;
    Ok(McpPinOutcome {
        name: name.to_string(),
        pin,
        installed,
        already,
        pins_path: pins.path().to_path_buf(),
    })
}

/// The names of the configured servers that are unpinned in the workspace `cwd` (see
/// [`mcp_server_unpinned`]), started or not.
#[must_use]
pub fn mcp_unpinned_servers(
    store: &McpInstallStore,
    mcp: &crate::config::McpConfigCollection,
    cwd: &Path,
) -> Vec<String> {
    let mut names = mcp
        .servers()
        .iter()
        .chain(mcp.disabled())
        .chain(mcp.needs_approval())
        .filter(|(_, server)| mcp_server_unpinned(store, server.scope, &server.config, cwd))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use sha2::{Digest, Sha512};

    use super::{
        launcher_words, mcp_pinned_launch, mcp_server_unpinned, npm_package, package_and_args,
        parse_npm_lock, parse_requirements, pin_mcp_server_with, python_requirement, McpPin,
        McpPinResolve, McpPinnedLaunch,
    };
    use crate::config::{ConfigLoader, ConfigSource, McpServerConfig, McpStdioServerConfig};
    use crate::mcp_install::{McpFetch, McpInstallStore};

    fn temp_root(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("neuron-mcp-pins-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("repo")).expect("workspace");
        fs::create_dir_all(root.join("home")).expect("config home");
        root
    }

    fn user_servers(root: &Path, servers: &serde_json::Value) -> ConfigLoader {
        fs::write(
            root.join("home").join("settings.json"),
            serde_json::json!({"mcpServers": servers, "defaultMcpServers": false}).to_string(),
        )
        .unwrap();
        ConfigLoader::new(root.join("repo"), root.join("home"))
    }

    fn integrity(bytes: &[u8]) -> String {
        format!(
            "sha512-{}",
            base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                Sha512::digest(bytes)
            )
        )
    }

    /// Resolves from fixtures and serves their tarballs; never the network.
    struct Fixture {
        lock: String,
        requirements: String,
        tarballs: BTreeMap<String, Vec<u8>>,
    }

    impl McpPinResolve for Fixture {
        fn npm_lock(&self, _spec: &str) -> Result<String, String> {
            Ok(self.lock.clone())
        }
        fn python_requirements(&self, _requirement: &str) -> Result<String, String> {
            Ok(self.requirements.clone())
        }
    }

    impl McpFetch for Fixture {
        fn fetch_to(&self, url: &str, dest: &Path) -> Result<(), String> {
            let bytes = self
                .tarballs
                .get(url)
                .ok_or_else(|| format!("offline: {url}"))?;
            fs::write(dest, bytes).map_err(|error| error.to_string())
        }
    }

    fn npm_fixture() -> Fixture {
        let tarball = crate::mcp_install::archive_fixtures::tar_gz(&[
            ("package/package.json", br#"{"name":"notes-mcp"}"#, b'0'),
            ("package/bin/server.js", b"// notes", b'0'),
        ]);
        let url = "https://registry.npmjs.org/notes-mcp/-/notes-mcp-1.4.2.tgz";
        let lock = serde_json::json!({
            "lockfileVersion": 3,
            "packages": {
                "": {"dependencies": {"notes-mcp": "1.4.2"}},
                "node_modules/notes-mcp": {
                    "version": "1.4.2", "resolved": url, "integrity": integrity(&tarball),
                    "bin": {"notes-mcp": "bin/server.js"}
                }
            }
        });
        Fixture {
            lock: lock.to_string(),
            requirements: String::new(),
            tarballs: BTreeMap::from([(url.to_string(), tarball)]),
        }
    }

    #[test]
    fn launcher_words_and_package_words_are_read_as_npx_and_uvx_read_them() {
        let words = |items: &[&str]| {
            items
                .iter()
                .map(|item| (*item).to_string())
                .collect::<Vec<_>>()
        };
        let args = words(&["/d", "/s", "/c", "npx", "-y", "notes-mcp", "--dir", "x"]);
        let after = launcher_words("npx", "cmd.exe", &args).unwrap();
        assert_eq!(
            package_and_args(after).unwrap(),
            ("notes-mcp".to_string(), words(&["--dir", "x"]))
        );
        assert!(launcher_words("npx", "node", &args).is_none());
        assert!(package_and_args(&words(&["--package", "x"])).is_err());
        assert_eq!(
            npm_package("@scope/x@1.2"),
            ("@scope/x".to_string(), "1.2".to_string())
        );
        assert_eq!(npm_package("x"), ("x".to_string(), "latest".to_string()));
        assert_eq!(python_requirement("x@1.2").1, "x==1.2");
        assert_eq!(python_requirement("x==1.2").1, "x==1.2");
        assert_eq!(python_requirement("x").1, "x");
    }

    #[test]
    fn lock_and_requirements_output_parse_into_pins_and_unhashed_output_is_refused() {
        let fixture = npm_fixture();
        let (version, bin, lock) = parse_npm_lock(&fixture.lock, "notes-mcp").unwrap();
        assert_eq!((version.as_str(), bin.as_str()), ("1.4.2", "bin/server.js"));
        assert_eq!(lock.len(), 1);
        let foreign = fixture
            .lock
            .replace("https://registry.npmjs.org/", "https://evil.example/");
        assert!(parse_npm_lock(&foreign, "notes-mcp").is_err());

        let text = "anyio==4.16.0 \\\n    --hash=sha256:aa \\\n    --hash=sha256:bb\npywin32==311 ; sys_platform == 'win32' \\\n    --hash=sha256:cc\n";
        let requirements = parse_requirements(text).unwrap();
        assert_eq!(requirements.len(), 2);
        assert_eq!(requirements[0].hashes, vec!["sha256:aa", "sha256:bb"]);
        assert_eq!(
            requirements[1].marker.as_deref(),
            Some("sys_platform == 'win32'")
        );
        assert!(parse_requirements("anyio==4.16.0\n").is_err());
    }

    #[test]
    fn pinning_an_npx_server_records_its_version_and_it_starts_from_the_verified_install() {
        let root = temp_root("npm");
        let loader = user_servers(
            &root,
            &serde_json::json!({"notes": {"command": "npx", "args": ["-y", "notes-mcp", "--dir", "notes"]}}),
        );
        let store = McpInstallStore::in_config_home(&root.join("home"));
        let config = loader.load().unwrap();
        let server = config.mcp().get("notes").unwrap();
        assert!(mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            loader.cwd()
        ));
        assert_eq!(
            super::mcp_unpinned_servers(&store, config.mcp(), loader.cwd()),
            vec!["notes"]
        );

        let fixture = npm_fixture();
        let outcome = pin_mcp_server_with(&loader, "notes", &store, &fixture, &fixture).unwrap();
        assert!(matches!(&outcome.pin, McpPin::Npm { version, .. } if version == "1.4.2"));
        assert!(outcome.installed.is_some());
        assert!(!mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            loader.cwd()
        ));

        let McpServerConfig::Stdio(stdio) = &server.config else {
            unreachable!()
        };
        let launch = mcp_pinned_launch(
            &store,
            &stdio.command,
            &stdio.args,
            &stdio.env,
            loader.cwd(),
        )
        .unwrap();
        let McpPinnedLaunch::Verified(spec) = launch else {
            panic!("{launch:?}");
        };
        let started = store.ensure(&spec, &fixture).unwrap();
        assert!(started.args[0].ends_with("server.js"), "{:?}", started.args);
        assert_eq!(&started.args[1..], ["--dir", "notes"]);

        // Another launch spec under the same name is unpinned again.
        let changed = McpServerConfig::Stdio(McpStdioServerConfig {
            args: vec!["-y".to_string(), "notes-mcp@2".to_string()],
            ..stdio.clone()
        });
        assert!(mcp_server_unpinned(
            &store,
            ConfigSource::User,
            &changed,
            loader.cwd()
        ));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_pinned_program_whose_file_changes_is_refused_until_pinned_again() {
        let root = temp_root("program");
        let program = root.join("repo").join("server.py");
        fs::write(&program, "print('v1')").unwrap();
        let interpreter = root.join("bin").join("python-fixture");
        fs::create_dir_all(interpreter.parent().unwrap()).unwrap();
        fs::write(&interpreter, "interpreter").unwrap();
        let loader = user_servers(
            &root,
            &serde_json::json!({"local": {
                "command": interpreter.to_string_lossy(),
                "args": [program.to_string_lossy()]
            }}),
        );
        let store = McpInstallStore::in_config_home(&root.join("home"));
        let fixture = npm_fixture();
        let outcome = pin_mcp_server_with(&loader, "local", &store, &fixture, &fixture).unwrap();
        let McpPin::Program { files } = &outcome.pin else {
            panic!("{:?}", outcome.pin);
        };
        assert_eq!(files.len(), 2, "the program and its script");
        assert!(outcome.installed.is_none());
        let again = pin_mcp_server_with(&loader, "local", &store, &fixture, &fixture).unwrap();
        assert!(again.already);

        let command = interpreter.to_string_lossy().into_owned();
        let args = vec![program.to_string_lossy().into_owned()];
        let env = BTreeMap::new();
        assert_eq!(
            mcp_pinned_launch(&store, &command, &args, &env, loader.cwd()).unwrap(),
            McpPinnedLaunch::Checked
        );
        fs::write(&program, "print('v2: something else')").unwrap();
        let error = mcp_pinned_launch(&store, &command, &args, &env, loader.cwd()).unwrap_err();
        assert!(error.contains("changed since it was pinned"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_program_pin_does_not_vouch_for_the_same_relative_launch_in_another_workspace() {
        let root = temp_root("two-workspaces");
        let interpreter = root.join("bin").join("node-fixture");
        fs::create_dir_all(interpreter.parent().unwrap()).unwrap();
        fs::write(&interpreter, "interpreter").unwrap();
        let other = root.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(root.join("repo").join("server.js"), "// pinned here").unwrap();
        fs::write(other.join("server.js"), "// never hashed").unwrap();
        // The same relative launch spec in both workspaces (a project `.mcp.json`, say).
        let loader = user_servers(
            &root,
            &serde_json::json!({"local": {
                "command": interpreter.to_string_lossy(),
                "args": ["server.js"]
            }}),
        );
        let elsewhere = ConfigLoader::new(&other, root.join("home"));
        let store = McpInstallStore::in_config_home(&root.join("home"));
        let fixture = npm_fixture();
        pin_mcp_server_with(&loader, "local", &store, &fixture, &fixture).unwrap();

        let config = loader.load().unwrap();
        let server = config.mcp().get("local").unwrap();
        let McpServerConfig::Stdio(stdio) = &server.config else {
            unreachable!()
        };
        assert!(!mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            loader.cwd()
        ));
        assert_eq!(
            mcp_pinned_launch(
                &store,
                &stdio.command,
                &stdio.args,
                &stdio.env,
                loader.cwd()
            )
            .unwrap(),
            McpPinnedLaunch::Checked
        );
        // In the other workspace `server.js` is another file: unpinned, not checked.
        assert!(mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            &other
        ));
        assert_eq!(
            mcp_pinned_launch(&store, &stdio.command, &stdio.args, &stdio.env, &other).unwrap(),
            McpPinnedLaunch::Unpinned
        );

        // Pinned there too, each workspace keeps its own pin.
        let outcome = pin_mcp_server_with(&elsewhere, "local", &store, &fixture, &fixture).unwrap();
        assert!(!outcome.already);
        assert!(!mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            &other
        ));
        assert!(!mcp_server_unpinned(
            &store,
            server.scope,
            &server.config,
            loader.cwd()
        ));
        fs::write(other.join("server.js"), "// changed").unwrap();
        assert!(
            mcp_pinned_launch(&store, &stdio.command, &stdio.args, &stdio.env, &other).is_err()
        );
        assert_eq!(
            mcp_pinned_launch(
                &store,
                &stdio.command,
                &stdio.args,
                &stdio.env,
                loader.cwd()
            )
            .unwrap(),
            McpPinnedLaunch::Checked
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_bundled_and_catalog_servers_are_not_pinned() {
        let root = temp_root("refused");
        let loader = user_servers(
            &root,
            &serde_json::json!({
                "remote": {"type": "http", "url": "https://example.com/mcp"},
                "fs": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem@2.1.0", "."]}
            }),
        );
        let store = McpInstallStore::in_config_home(&root.join("home"));
        let fixture = npm_fixture();
        for (name, why) in [
            ("remote", "nothing to pin"),
            ("fs", "catalog pins"),
            ("missing", "no MCP server"),
        ] {
            let error = pin_mcp_server_with(&loader, name, &store, &fixture, &fixture).unwrap_err();
            assert!(error.contains(why), "{name}: {error}");
        }
        let config = loader.load().unwrap();
        assert!(super::mcp_unpinned_servers(&store, config.mcp(), loader.cwd()).is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
