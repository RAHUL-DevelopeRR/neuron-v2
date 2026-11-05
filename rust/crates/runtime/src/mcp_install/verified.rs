//! Verified installs of local MCP servers: a catalog connector (or a pinned server) is
//! installed into `<config home>/mcp/<id>/<version>` from downloads whose hashes the
//! signed catalog pins, and started from there. Nothing runs `npx` or `uvx` at start.
//!
//! - npm: every package of the entry's lock is fetched from the registry, checked
//!   against its `sha512` integrity and unpacked by Neuron itself, so no install script
//!   (`preinstall`, `postinstall`) ever runs. The server starts as `node <bin>`.
//! - Python: `uv pip install --require-hashes --no-deps --only-binary :all:` into a
//!   virtual environment of its own from the entry's hashed requirements (wheels only,
//!   so no build script runs either).
//! - Release binary: the asset for this platform is fetched, checked against its
//!   `sha256`, and its program unpacked. An npm package that only downloads a release
//!   binary in its install script (Codebase memory) is installed this way, and its
//!   script never runs.
//! - Docker: the image is pulled by its digest and run with `--pull=never`.
//!
//! The first start installs (the network is needed once); later starts are offline.
//! Each start checks the install record: a file whose size or modification time
//! changed is hashed again, and one that no longer matches blocks the start. A cached
//! npm tarball is checked before it is unpacked; one that does not match its integrity
//! is removed and blocks the start, so the next start downloads it again.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256, Sha512};

use super::archive::{extract_tar_gz_member, extract_zip_member, set_mode, unpack_tar_gz};
use crate::mcp_catalog::{
    mcp_release_platform, McpCatalogEntry, McpInstallKind, McpNpmLockEntry, McpPythonRequirement,
    McpReleaseArchive, McpReleaseAsset, NPM_REGISTRY,
};

/// The install record in each install folder.
const MANIFEST: &str = ".neuron-install.json";
/// The largest download accepted (a release binary archive is ~100 MB).
const MAX_DOWNLOAD_BYTES: u64 = 1 << 30;

/// Fetches a URL into a file. The CLI fetches over HTTPS; tests serve fixtures.
pub trait McpFetch: Sync {
    /// Writes the body of `url` to `dest`.
    fn fetch_to(&self, url: &str, dest: &Path) -> Result<(), String>;
}

/// Fetches over HTTPS only (redirects too), up to 1 GiB, on a thread of its own so it
/// can be called from inside the engine's async runtime.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpHttpFetch;

impl McpFetch for McpHttpFetch {
    fn fetch_to(&self, url: &str, dest: &Path) -> Result<(), String> {
        if !url.starts_with("https://") {
            return Err(format!("{url} is not an https URL"));
        }
        let (url, dest) = (url.to_string(), dest.to_path_buf());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?;
            runtime.block_on(download(&url, &dest))
        })
        .join()
        .map_err(|_| "the download stopped unexpectedly".to_string())?
    }
}

async fn download(url: &str, dest: &Path) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(15 * 60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("too many redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("redirected away from https")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| format!("cannot download {url}: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} answered {}", response.status()));
    }
    let mut file = fs::File::create(dest).map_err(|error| error.to_string())?;
    let mut total = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("cannot download {url}: {error}"))?
    {
        total += chunk.len() as u64;
        if total > MAX_DOWNLOAD_BYTES {
            return Err(format!("{url} is larger than 1 GiB"));
        }
        file.write_all(&chunk).map_err(|error| error.to_string())?;
    }
    file.flush().map_err(|error| error.to_string())
}

/// What a verified install fetches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpVerifiedSource {
    /// An npm dependency tree; `bin` is the program's path in `package`.
    Npm {
        package: String,
        bin: String,
        lock: Vec<McpNpmLockEntry>,
    },
    /// Python requirements; `script` is the console script that starts the server.
    Python {
        script: String,
        requirements: Vec<McpPythonRequirement>,
    },
    /// A release binary for this platform.
    Release(McpReleaseAsset),
    /// A container image by digest (`image@sha256:...`).
    Docker { image: String },
}

/// A server to install: its folder name, version, what it fetches, and the arguments
/// after its program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpVerifiedSpec {
    pub name: String,
    pub version: String,
    pub source: McpVerifiedSource,
    pub args: Vec<String>,
}

impl McpVerifiedSpec {
    /// How the local catalog entry `entry` is installed; `None` for a remote one, an
    /// error when it has no download for this platform.
    pub fn for_entry(entry: &McpCatalogEntry) -> Result<Option<Self>, String> {
        let install = &entry.install;
        let package = install.package.clone().unwrap_or_default();
        let source = match install.kind {
            McpInstallKind::Remote => return Ok(None),
            McpInstallKind::Npm | McpInstallKind::Binary if !install.release.is_empty() => {
                let platform = mcp_release_platform();
                let asset = install.release.get(&platform).ok_or_else(|| {
                    format!("`{}` has no verified download for {platform}", entry.id)
                })?;
                McpVerifiedSource::Release(asset.clone())
            }
            McpInstallKind::Npm => McpVerifiedSource::Npm {
                package,
                bin: install.bin.clone().unwrap_or_default(),
                lock: install.lock.clone(),
            },
            McpInstallKind::Uv => McpVerifiedSource::Python {
                script: install.bin.clone().unwrap_or(package),
                requirements: install.requirements.clone(),
            },
            McpInstallKind::Binary => {
                return Err(format!("`{}` has no release binary", entry.id));
            }
            McpInstallKind::Docker => McpVerifiedSource::Docker {
                image: format!(
                    "{package}@{}",
                    install.integrity.clone().unwrap_or_default()
                ),
            },
        };
        Ok(Some(Self {
            name: entry.id.clone(),
            version: install.version.clone().unwrap_or_default(),
            source,
            args: install.args.clone(),
        }))
    }

    /// The sha256 (hex) of what is installed: a catalog that changes it reinstalls.
    #[must_use]
    pub fn digest(&self) -> String {
        let source = match &self.source {
            McpVerifiedSource::Npm { package, bin, lock } => {
                json!({"npm": package, "bin": bin, "lock": lock})
            }
            McpVerifiedSource::Python {
                script,
                requirements,
            } => json!({"python": script, "requirements": requirements}),
            McpVerifiedSource::Release(asset) => json!({"release": asset}),
            McpVerifiedSource::Docker { image } => json!({"docker": image}),
        };
        hex(&Sha256::digest(source.to_string().as_bytes()))
    }

    /// `npm`, `python`, `release` or `docker`.
    #[must_use]
    pub fn kind_label(&self) -> &'static str {
        match self.source {
            McpVerifiedSource::Npm { .. } => "npm",
            McpVerifiedSource::Python { .. } => "python",
            McpVerifiedSource::Release(_) => "release",
            McpVerifiedSource::Docker { .. } => "docker",
        }
    }
}

/// What a verified install starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpVerifiedLaunch {
    pub command: String,
    pub args: Vec<String>,
    /// The install folder (none for an image).
    pub dir: Option<PathBuf>,
}

/// Whether a server is installed yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpInstallState {
    /// Installed and recorded in this folder.
    Installed(PathBuf),
    /// Not installed (or installed from another spec): the next start installs it.
    NotInstalled(PathBuf),
    /// An image, pulled by Docker on the first start.
    Image,
}

/// Where verified installs live: `<config home>/mcp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpInstallStore {
    root: PathBuf,
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// A folder name part: letters, digits and `._+-`, not starting with a dot.
fn safe_part(text: &str) -> Result<&str, String> {
    let fine = !text.is_empty()
        && !text.starts_with('.')
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    fine.then_some(text)
        .ok_or_else(|| format!("`{text}` cannot name an install folder"))
}

fn io_text(error: &io::Error) -> String {
    error.to_string()
}

/// The sha256 (hex) and the sha512 of a file, read once.
fn file_digests(path: &Path) -> io::Result<(String, Vec<u8>)> {
    let mut file = fs::File::open(path)?;
    let mut sha256 = Sha256::new();
    let mut sha512 = Sha512::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha256.update(&buffer[..read]);
        sha512.update(&buffer[..read]);
    }
    Ok((hex(&sha256.finalize()), sha512.finalize().to_vec()))
}

fn modified_nanos(metadata: &fs::Metadata) -> String {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or_else(String::new, |since| since.as_nanos().to_string())
}

/// `sha512-<base64>` of a sha512 digest.
fn npm_integrity(sha512: &[u8]) -> String {
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(sha512)
    )
}

/// npm's name for this platform and CPU.
fn npm_platform() -> (&'static str, &'static str) {
    let os = match std::env::consts::OS {
        "windows" => "win32",
        "macos" => "darwin",
        other => other,
    };
    let cpu = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    };
    (os, cpu)
}

/// Whether an `os` or `cpu` list of a lock entry admits `value` (`!value` excludes it).
fn admits(list: &[String], value: &str) -> bool {
    if list.is_empty() {
        return true;
    }
    if list
        .iter()
        .any(|item| item.strip_prefix('!') == Some(value))
    {
        return false;
    }
    let positive = list.iter().filter(|item| !item.starts_with('!'));
    let mut positive = positive.peekable();
    positive.peek().is_none() || positive.any(|item| item == value)
}

/// The Node.js that runs npm servers: `NEURON_MCP_NODE`, else `node` from the path.
fn node_command() -> String {
    std::env::var("NEURON_MCP_NODE").unwrap_or_else(|_| "node".to_string())
}

impl McpInstallStore {
    /// The store in `config_home`.
    #[must_use]
    pub fn in_config_home(config_home: &Path) -> Self {
        Self {
            root: config_home.join("mcp"),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn install_dir(&self, spec: &McpVerifiedSpec) -> Result<PathBuf, String> {
        Ok(self
            .root
            .join(safe_part(&spec.name)?)
            .join(safe_part(&spec.version)?))
    }

    /// Where the cached npm tarball with this integrity is kept.
    fn npm_cache_path(&self, integrity: &str) -> Result<PathBuf, String> {
        let digest = integrity
            .strip_prefix("sha512-")
            .and_then(|text| base64::engine::general_purpose::STANDARD.decode(text).ok())
            .filter(|digest| digest.len() == 64)
            .ok_or_else(|| format!("`{integrity}` is not a sha512 integrity"))?;
        Ok(self
            .root
            .join("cache")
            .join("npm")
            .join(format!("{}.tgz", hex(&digest))))
    }

    /// Whether `spec` is installed (without checking its files).
    pub fn state(&self, spec: &McpVerifiedSpec) -> Result<McpInstallState, String> {
        if matches!(spec.source, McpVerifiedSource::Docker { .. }) {
            return Ok(McpInstallState::Image);
        }
        let dir = self.install_dir(spec)?;
        let installed = read_manifest(&dir)
            .is_some_and(|manifest| manifest["spec"].as_str() == Some(spec.digest().as_str()));
        Ok(if installed {
            McpInstallState::Installed(dir)
        } else {
            McpInstallState::NotInstalled(dir)
        })
    }

    /// Installs `spec` when it is not installed yet (fetching with `fetch`), checks the
    /// installed files, and returns what starts it.
    pub fn ensure(
        &self,
        spec: &McpVerifiedSpec,
        fetch: &dyn McpFetch,
    ) -> Result<McpVerifiedLaunch, String> {
        if let McpVerifiedSource::Docker { image } = &spec.source {
            ensure_image(image)?;
            let mut args = ["run", "-i", "--rm", "--pull=never"]
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>();
            args.push(image.clone());
            args.extend(spec.args.iter().cloned());
            return Ok(McpVerifiedLaunch {
                command: "docker".to_string(),
                args,
                dir: None,
            });
        }
        let dir = self.install_dir(spec)?;
        let digest = spec.digest();
        match read_manifest(&dir) {
            Some(manifest) if manifest["spec"].as_str() == Some(digest.as_str()) => {
                if let Err(problem) = check_files(&dir, &manifest) {
                    let _ = fs::remove_dir_all(&dir);
                    return Err(format!(
                        "integrity check failed: {problem} in {}, which changed since it was installed; the copy was removed, so the next start installs it again",
                        dir.display()
                    ));
                }
            }
            _ => self.install(spec, &dir, &digest, fetch)?,
        }
        Ok(launch_of(spec, &dir))
    }

    fn install(
        &self,
        spec: &McpVerifiedSpec,
        dir: &Path,
        digest: &str,
        fetch: &dyn McpFetch,
    ) -> Result<(), String> {
        let parent = dir
            .parent()
            .ok_or_else(|| "the install folder has no parent".to_string())?;
        fs::create_dir_all(parent).map_err(|error| io_text(&error))?;
        let _ = fs::remove_dir_all(dir);
        // npm trees and binaries are unpacked beside the folder and renamed into place,
        // so a start never sees half an install. A virtual environment records its own
        // path, so it is built in place and recorded last.
        let staging = match spec.source {
            McpVerifiedSource::Python { .. } => dir.to_path_buf(),
            _ => parent.join(format!(
                ".{}.{}.tmp",
                dir.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("x"),
                std::process::id()
            )),
        };
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging).map_err(|error| io_text(&error))?;
        let result = match &spec.source {
            McpVerifiedSource::Npm { lock, .. } => self.install_npm(lock, &staging, fetch),
            McpVerifiedSource::Release(asset) => self.install_release(asset, &staging, fetch),
            McpVerifiedSource::Python { requirements, .. } => {
                install_python(requirements, &staging).map(|()| Vec::new())
            }
            McpVerifiedSource::Docker { .. } => Ok(Vec::new()),
        };
        let files = match result {
            Ok(files) => files,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let manifest = manifest_for(&staging, digest, spec, &files)?;
        fs::write(
            staging.join(MANIFEST),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )
        .map_err(|error| io_text(&error))?;
        if staging != dir {
            fs::rename(&staging, dir).map_err(|error| {
                let _ = fs::remove_dir_all(&staging);
                format!("cannot move the install into {}: {error}", dir.display())
            })?;
        }
        Ok(())
    }

    /// The cached tarball of `entry`, fetched when it is not cached, checked against its
    /// integrity either way.
    fn npm_tarball(
        &self,
        entry: &McpNpmLockEntry,
        fetch: &dyn McpFetch,
    ) -> Result<PathBuf, String> {
        let cached = self.npm_cache_path(&entry.integrity)?;
        let label = format!(
            "{}@{}",
            entry.path.trim_start_matches("node_modules/"),
            entry.version
        );
        if cached.is_file() {
            let (_, sha512) = file_digests(&cached).map_err(|error| io_text(&error))?;
            if npm_integrity(&sha512) == entry.integrity {
                return Ok(cached);
            }
            let _ = fs::remove_file(&cached);
            return Err(format!(
                "integrity check failed for {label}: the cached tarball {} does not match {}; it was removed, so the next start downloads it again",
                cached.display(),
                entry.integrity
            ));
        }
        if !entry.resolved.starts_with(NPM_REGISTRY) {
            return Err(format!(
                "{label} is not from the npm registry ({})",
                entry.resolved
            ));
        }
        let folder = cached
            .parent()
            .ok_or_else(|| "the npm cache has no folder".to_string())?;
        fs::create_dir_all(folder).map_err(|error| io_text(&error))?;
        let partial = cached.with_extension(format!("{}.part", std::process::id()));
        let fetched = fetch.fetch_to(&entry.resolved, &partial).and_then(|()| {
            let (_, sha512) = file_digests(&partial).map_err(|error| io_text(&error))?;
            if npm_integrity(&sha512) == entry.integrity {
                Ok(())
            } else {
                Err(format!(
                    "integrity check failed for {label}: the download from {} does not match {}",
                    entry.resolved, entry.integrity
                ))
            }
        });
        if let Err(error) = fetched {
            let _ = fs::remove_file(&partial);
            return Err(error);
        }
        fs::rename(&partial, &cached).map_err(|error| io_text(&error))?;
        Ok(cached)
    }

    fn install_npm(
        &self,
        lock: &[McpNpmLockEntry],
        staging: &Path,
        fetch: &dyn McpFetch,
    ) -> Result<Vec<String>, String> {
        let (os, cpu) = npm_platform();
        let mut files = Vec::new();
        for entry in lock {
            if !admits(&entry.os, os) || !admits(&entry.cpu, cpu) {
                if entry.optional {
                    continue;
                }
                return Err(format!(
                    "{} is not made for this platform ({os}, {cpu})",
                    entry.path
                ));
            }
            let tarball = self.npm_tarball(entry, fetch)?;
            let into = staging.join(&entry.path);
            let written = unpack_tar_gz(&tarball, &into, 1)
                .map_err(|error| format!("cannot unpack {}: {error}", entry.path))?;
            files.extend(
                written
                    .into_iter()
                    .map(|file| format!("{}/{file}", entry.path)),
            );
        }
        Ok(files)
    }

    fn install_release(
        &self,
        asset: &McpReleaseAsset,
        staging: &Path,
        fetch: &dyn McpFetch,
    ) -> Result<Vec<String>, String> {
        let folder = self.root.join("cache").join("release");
        fs::create_dir_all(&folder).map_err(|error| io_text(&error))?;
        let download = folder.join(format!("{}.{}.part", asset.sha256, std::process::id()));
        let result = fetch.fetch_to(&asset.url, &download).and_then(|()| {
            let (sha256, _) = file_digests(&download).map_err(|error| io_text(&error))?;
            if sha256 != asset.sha256 {
                return Err(format!(
                    "integrity check failed: the download from {} has sha256 {sha256}, not {}",
                    asset.url, asset.sha256
                ));
            }
            let program = asset.member.rsplit('/').next().unwrap_or(&asset.member);
            let out = staging.join(program);
            match asset.archive {
                McpReleaseArchive::Zip => extract_zip_member(&download, &asset.member, &out),
                McpReleaseArchive::TarGz => extract_tar_gz_member(&download, &asset.member, &out),
                McpReleaseArchive::None => fs::copy(&download, &out).map(|_| ()),
            }
            .and_then(|()| set_mode(&out, 0o755))
            .map_err(|error| format!("cannot unpack {}: {error}", asset.member))?;
            Ok(vec![program.to_string()])
        });
        // The archive is not kept: the program is, with its hash in the install record.
        let _ = fs::remove_file(&download);
        result
    }
}

/// Records `files` as the install of `spec` in `store`, as a finished install leaves
/// it, for tests that start a server from a cache that is already there.
#[cfg(test)]
pub(crate) fn record_install(
    store: &McpInstallStore,
    spec: &McpVerifiedSpec,
    files: &[(&str, &[u8])],
) -> PathBuf {
    let dir = store.install_dir(spec).expect("install folder");
    for (file, contents) in files {
        let path = dir.join(file);
        fs::create_dir_all(path.parent().expect("parent")).expect("folder");
        fs::write(path, contents).expect("file");
    }
    let names = files
        .iter()
        .map(|(file, _)| (*file).to_string())
        .collect::<Vec<_>>();
    let manifest = manifest_for(&dir, &spec.digest(), spec, &names).expect("manifest");
    fs::write(dir.join(MANIFEST), manifest.to_string()).expect("record");
    dir
}

fn read_manifest(dir: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(dir.join(MANIFEST)).ok()?).ok()
}

fn manifest_for(
    dir: &Path,
    digest: &str,
    spec: &McpVerifiedSpec,
    files: &[String],
) -> Result<Value, String> {
    let mut recorded = BTreeMap::new();
    for file in files {
        let path = dir.join(file);
        let metadata = fs::metadata(&path).map_err(|error| io_text(&error))?;
        let (sha256, _) = file_digests(&path).map_err(|error| io_text(&error))?;
        recorded.insert(
            file.clone(),
            json!({"sha256": sha256, "size": metadata.len(), "modified": modified_nanos(&metadata)}),
        );
    }
    Ok(json!({
        "spec": digest,
        "name": spec.name,
        "version": spec.version,
        "kind": spec.kind_label(),
        "installed": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_secs()),
        "files": recorded,
    }))
}

/// Checks the recorded files: one whose size or modification time changed is hashed
/// again. The first that is missing or no longer matches is the error.
fn check_files(dir: &Path, manifest: &Value) -> Result<(), String> {
    let Some(files) = manifest["files"].as_object() else {
        return Err("the install record lists no files".to_string());
    };
    for (file, recorded) in files {
        let path = dir.join(file);
        let metadata = fs::metadata(&path).map_err(|_| format!("{file} is missing"))?;
        let same_stamp = recorded["size"].as_u64() == Some(metadata.len())
            && recorded["modified"].as_str() == Some(modified_nanos(&metadata).as_str());
        if same_stamp {
            continue;
        }
        let (sha256, _) = file_digests(&path).map_err(|error| io_text(&error))?;
        if recorded["sha256"].as_str() != Some(sha256.as_str()) {
            return Err(format!("{file} does not match its recorded sha256"));
        }
    }
    Ok(())
}

fn launch_of(spec: &McpVerifiedSpec, dir: &Path) -> McpVerifiedLaunch {
    let (command, mut args) = match &spec.source {
        McpVerifiedSource::Npm { package, bin, .. } => {
            let mut script = dir.join("node_modules");
            script.extend(package.split('/').chain(bin.split('/')));
            (node_command(), vec![script.to_string_lossy().into_owned()])
        }
        McpVerifiedSource::Python { script, .. } => {
            let program = if cfg!(windows) {
                dir.join("venv")
                    .join("Scripts")
                    .join(format!("{script}.exe"))
            } else {
                dir.join("venv").join("bin").join(script)
            };
            (program.to_string_lossy().into_owned(), Vec::new())
        }
        McpVerifiedSource::Release(asset) => {
            let program = asset.member.rsplit('/').next().unwrap_or(&asset.member);
            (dir.join(program).to_string_lossy().into_owned(), Vec::new())
        }
        McpVerifiedSource::Docker { image } => ("docker".to_string(), vec![image.clone()]),
    };
    args.extend(spec.args.iter().cloned());
    McpVerifiedLaunch {
        command,
        args,
        dir: Some(dir.to_path_buf()),
    }
}

/// `requirements` as a hashed requirements file: one line per requirement with its
/// marker and every `--hash`.
#[must_use]
pub fn mcp_requirements_text(requirements: &[McpPythonRequirement]) -> String {
    let mut text = String::new();
    for requirement in requirements {
        let _ = write!(text, "{}=={}", requirement.name, requirement.version);
        if let Some(marker) = &requirement.marker {
            let _ = write!(text, " ; {marker}");
        }
        for hash in &requirement.hashes {
            let _ = write!(text, " --hash={hash}");
        }
        text.push('\n');
    }
    text
}

/// The `uv` steps that install `requirements` into `dir/venv`.
fn python_steps(dir: &Path) -> [Vec<String>; 2] {
    let venv = dir.join("venv").to_string_lossy().into_owned();
    let requirements = dir.join("requirements.txt").to_string_lossy().into_owned();
    [
        vec![
            "venv".to_string(),
            venv.clone(),
            "--python".to_string(),
            ">=3.10".to_string(),
            "--quiet".to_string(),
        ],
        vec![
            "pip".to_string(),
            "install".to_string(),
            "--python".to_string(),
            venv,
            "--require-hashes".to_string(),
            "--no-deps".to_string(),
            "--only-binary".to_string(),
            ":all:".to_string(),
            "--quiet".to_string(),
            "-r".to_string(),
            requirements,
        ],
    ]
}

fn install_python(requirements: &[McpPythonRequirement], dir: &Path) -> Result<(), String> {
    fs::write(
        dir.join("requirements.txt"),
        mcp_requirements_text(requirements),
    )
    .map_err(|error| io_text(&error))?;
    let uv = std::env::var("NEURON_MCP_UV").unwrap_or_else(|_| "uv".to_string());
    for step in python_steps(dir) {
        let output = Command::new(&uv)
            .args(&step)
            .current_dir(dir)
            .output()
            .map_err(|error| {
                format!("cannot run `{uv}` (install uv to use this server): {error}")
            })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let tail = stderr.lines().rev().take(4).collect::<Vec<_>>();
            return Err(format!(
                "`uv {}` failed: {}",
                step[..2].join(" "),
                tail.into_iter().rev().collect::<Vec<_>>().join(" / ")
            ));
        }
    }
    Ok(())
}

/// Pulls `image` (`name@sha256:...`) unless Docker has it already.
fn ensure_image(image: &str) -> Result<(), String> {
    let present = Command::new("docker")
        .args(["image", "inspect", "--format", "{{.Id}}", image])
        .output()
        .map_err(|error| format!("cannot run docker: {error}"))?
        .status
        .success();
    if present {
        return Ok(());
    }
    let output = Command::new("docker")
        .args(["pull", "--quiet", image])
        .output()
        .map_err(|error| format!("cannot run docker: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`docker pull {image}` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    use sha2::{Digest, Sha256, Sha512};

    use super::super::archive::fixtures::{tar_gz, zip};
    use super::{
        admits, hex, mcp_requirements_text, npm_integrity, python_steps, McpFetch, McpInstallState,
        McpInstallStore, McpVerifiedSource, McpVerifiedSpec,
    };
    use crate::mcp_catalog::{
        mcp_release_platform, McpCatalog, McpNpmLockEntry, McpPythonRequirement, McpReleaseArchive,
        McpReleaseAsset,
    };

    /// Serves fixture bytes by URL and records what was fetched; never the network.
    #[derive(Default)]
    struct Fixtures {
        files: BTreeMap<String, Vec<u8>>,
        fetched: Mutex<Vec<String>>,
    }

    impl McpFetch for Fixtures {
        fn fetch_to(&self, url: &str, dest: &Path) -> Result<(), String> {
            self.fetched.lock().unwrap().push(url.to_string());
            let bytes = self
                .files
                .get(url)
                .ok_or_else(|| format!("offline: {url}"))?;
            fs::write(dest, bytes).map_err(|error| error.to_string())
        }
    }

    /// No network at all.
    struct Offline;

    impl McpFetch for Offline {
        fn fetch_to(&self, url: &str, _dest: &Path) -> Result<(), String> {
            Err(format!("offline: {url}"))
        }
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "neuron-mcp-verified-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock_entry(path: &str, version: &str, tarball: &[u8]) -> McpNpmLockEntry {
        let name = path.trim_start_matches("node_modules/");
        McpNpmLockEntry {
            path: path.to_string(),
            version: version.to_string(),
            resolved: format!("https://registry.npmjs.org/{name}/-/{name}-{version}.tgz"),
            integrity: npm_integrity(&Sha512::digest(tarball)),
            os: Vec::new(),
            cpu: Vec::new(),
            optional: false,
        }
    }

    /// A two-package npm server (`demo-server` needing `dep`) and the fixtures that
    /// serve its tarballs.
    fn npm_fixture() -> (McpVerifiedSpec, Fixtures) {
        let server = tar_gz(&[
            ("package/package.json", br#"{"name":"demo-server"}"#, b'0'),
            ("package/cli.js", b"require('dep')", b'0'),
        ]);
        let dep = tar_gz(&[
            ("package/package.json", br#"{"name":"dep"}"#, b'0'),
            ("package/index.js", b"module.exports = 1", b'0'),
        ]);
        let lock = vec![
            lock_entry("node_modules/demo-server", "1.0.0", &server),
            lock_entry("node_modules/dep", "2.0.0", &dep),
        ];
        let fixtures = Fixtures {
            files: BTreeMap::from([
                (lock[0].resolved.clone(), server),
                (lock[1].resolved.clone(), dep),
            ]),
            ..Fixtures::default()
        };
        let spec = McpVerifiedSpec {
            name: "demo".to_string(),
            version: "1.0.0".to_string(),
            source: McpVerifiedSource::Npm {
                package: "demo-server".to_string(),
                bin: "cli.js".to_string(),
                lock,
            },
            args: vec!["--stdio".to_string()],
        };
        (spec, fixtures)
    }

    #[test]
    fn an_npm_server_installs_from_its_lock_and_later_starts_offline() {
        let home = temp("npm");
        let store = McpInstallStore::in_config_home(&home);
        let (spec, fixtures) = npm_fixture();
        assert!(matches!(
            store.state(&spec).unwrap(),
            McpInstallState::NotInstalled(_)
        ));
        let launch = store.ensure(&spec, &fixtures).unwrap();
        let dir = home.join("mcp").join("demo").join("1.0.0");
        let cli = dir.join("node_modules").join("demo-server").join("cli.js");
        assert_eq!(
            launch.args,
            vec![cli.to_string_lossy().into_owned(), "--stdio".to_string()]
        );
        assert_eq!(fs::read_to_string(&cli).unwrap(), "require('dep')");
        assert!(dir.join("node_modules/dep/index.js").is_file());
        assert_eq!(fixtures.fetched.lock().unwrap().len(), 2);
        assert_eq!(store.state(&spec).unwrap(), McpInstallState::Installed(dir));

        // No network now: the installed copy starts, and nothing is fetched.
        assert_eq!(store.ensure(&spec, &Offline).unwrap(), launch);
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn a_tampered_cached_tarball_blocks_the_start_with_an_integrity_error() {
        let home = temp("tamper-cache");
        let store = McpInstallStore::in_config_home(&home);
        let (spec, fixtures) = npm_fixture();
        store.ensure(&spec, &fixtures).unwrap();
        let McpVerifiedSource::Npm { lock, .. } = &spec.source else {
            unreachable!()
        };
        let cached = store.npm_cache_path(&lock[1].integrity).unwrap();
        let mut bytes = fs::read(&cached).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&cached, bytes).unwrap();
        // The install is gone (a cleaned folder, or a newer catalog): it comes from the
        // cache again, and the cache no longer matches.
        fs::remove_dir_all(home.join("mcp").join("demo")).unwrap();
        let error = store.ensure(&spec, &Offline).unwrap_err();
        assert!(
            error.contains("integrity check failed for dep@2.0.0"),
            "{error}"
        );
        assert!(!cached.exists(), "the bad tarball is removed");
        assert!(
            !home.join("mcp/demo/1.0.0").exists(),
            "nothing was installed"
        );
        // Online, the next start downloads and checks it again.
        store.ensure(&spec, &fixtures).unwrap();
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn a_changed_installed_file_blocks_the_start_and_a_bad_download_installs_nothing() {
        let home = temp("tamper-file");
        let store = McpInstallStore::in_config_home(&home);
        let (spec, fixtures) = npm_fixture();
        store.ensure(&spec, &fixtures).unwrap();
        let cli = home.join("mcp/demo/1.0.0/node_modules/demo-server/cli.js");
        fs::write(&cli, "require('child_process').exec('evil')").unwrap();
        let error = store.ensure(&spec, &Offline).unwrap_err();
        assert!(error.contains("integrity check failed"), "{error}");
        assert!(error.contains("cli.js"), "{error}");
        assert!(!cli.exists());

        // A registry that serves other bytes than the lock pins.
        let other = temp("bad-download");
        let store = McpInstallStore::in_config_home(&other);
        let (spec, mut fixtures) = npm_fixture();
        for bytes in fixtures.files.values_mut() {
            bytes.push(0);
        }
        let error = store.ensure(&spec, &fixtures).unwrap_err();
        assert!(error.contains("does not match"), "{error}");
        assert!(matches!(
            store.state(&spec).unwrap(),
            McpInstallState::NotInstalled(_)
        ));
        assert!(!other
            .join("mcp/cache/npm")
            .read_dir()
            .unwrap()
            .any(|_| true));
        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(other);
    }

    #[test]
    fn packages_for_another_platform_are_skipped_when_optional() {
        assert!(admits(&[], "win32"));
        assert!(admits(&["win32".to_string()], "win32"));
        assert!(!admits(&["darwin".to_string()], "win32"));
        assert!(!admits(&["!win32".to_string()], "win32"));
        assert!(admits(&["!darwin".to_string()], "win32"));

        let home = temp("platform");
        let store = McpInstallStore::in_config_home(&home);
        let (mut spec, fixtures) = npm_fixture();
        let push = |spec: &mut McpVerifiedSpec, optional: bool| {
            if let McpVerifiedSource::Npm { lock, .. } = &mut spec.source {
                let mut elsewhere = lock[1].clone();
                elsewhere.path = "node_modules/fsevents".to_string();
                elsewhere.os = vec!["aix".to_string()];
                elsewhere.optional = optional;
                lock.push(elsewhere);
            }
        };
        push(&mut spec, true);
        store.ensure(&spec, &fixtures).unwrap();
        assert!(!home.join("mcp/demo/1.0.0/node_modules/fsevents").exists());
        push(&mut spec, false);
        let _ = fs::remove_dir_all(home.join("mcp/demo"));
        assert!(store
            .ensure(&spec, &fixtures)
            .unwrap_err()
            .contains("not made for this platform"));
        let _ = fs::remove_dir_all(home);
    }

    fn release_spec(archive: McpReleaseArchive, bytes: &[u8], member: &str) -> McpVerifiedSpec {
        McpVerifiedSpec {
            name: "tool".to_string(),
            version: "0.1.0".to_string(),
            source: McpVerifiedSource::Release(McpReleaseAsset {
                url: "https://github.com/example/tool/releases/download/v0.1.0/tool.bin"
                    .to_string(),
                sha256: hex(&Sha256::digest(bytes)),
                archive,
                member: member.to_string(),
            }),
            args: Vec::new(),
        }
    }

    #[test]
    fn a_release_binary_is_hash_checked_unpacked_and_its_archive_not_kept() {
        let program = b"\x7fELF fixture program".to_vec();
        for (archive, bytes, member) in [
            (
                McpReleaseArchive::Zip,
                zip(&[
                    ("LICENSE", b"MIT"),
                    ("install.ps1", b"iwr | iex"),
                    ("tool.exe", &program),
                ]),
                "tool.exe",
            ),
            (
                McpReleaseArchive::TarGz,
                tar_gz(&[("LICENSE", b"MIT", b'0'), ("tool", &program, b'0')]),
                "tool",
            ),
            (McpReleaseArchive::None, program.clone(), "tool"),
        ] {
            let home = temp("release");
            let store = McpInstallStore::in_config_home(&home);
            let spec = release_spec(archive, &bytes, member);
            let McpVerifiedSource::Release(asset) = &spec.source else {
                unreachable!()
            };
            let fixtures = Fixtures {
                files: BTreeMap::from([(asset.url.clone(), bytes.clone())]),
                ..Fixtures::default()
            };
            let launch = store.ensure(&spec, &fixtures).unwrap();
            let dir = home.join("mcp").join("tool").join("0.1.0");
            assert_eq!(launch.command, dir.join(member).to_string_lossy());
            assert_eq!(fs::read(dir.join(member)).unwrap(), program);
            // Only the program is unpacked: the archive's install script is not.
            assert!(!dir.join("install.ps1").exists());
            assert!(!home
                .join("mcp/cache/release")
                .read_dir()
                .unwrap()
                .any(|_| true));
            assert_eq!(store.ensure(&spec, &Offline).unwrap(), launch);

            // Another binary under the same URL is refused.
            let _ = fs::remove_dir_all(&dir);
            let mut tampered = bytes.clone();
            tampered[0] ^= 1;
            let fixtures = Fixtures {
                files: BTreeMap::from([(asset.url.clone(), tampered)]),
                ..Fixtures::default()
            };
            let error = store.ensure(&spec, &fixtures).unwrap_err();
            assert!(error.contains("integrity check failed"), "{error}");
            assert!(!dir.exists());
            let _ = fs::remove_dir_all(home);
        }
    }

    #[test]
    fn codebase_memory_runs_its_hash_checked_release_binary_not_its_install_script() {
        let entry = McpCatalog::builtin().get("codebase-memory").unwrap();
        for platform in [
            "windows-x86_64",
            "windows-aarch64",
            "macos-x86_64",
            "macos-aarch64",
            "linux-x86_64",
            "linux-aarch64",
        ] {
            let asset = &entry.install.release[platform];
            assert!(
                asset.url.starts_with(
                    "https://github.com/DeusData/codebase-memory-mcp/releases/download/v0.11.0/"
                ),
                "{platform}"
            );
            assert_eq!(asset.sha256.len(), 64, "{platform}");
        }
        let spec = McpVerifiedSpec::for_entry(entry).unwrap().unwrap();
        let McpVerifiedSource::Release(asset) = &spec.source else {
            panic!("codebase-memory installs its release binary");
        };
        assert_eq!(asset, &entry.install.release[&mcp_release_platform()]);
    }

    #[test]
    fn every_local_catalog_entry_has_a_verified_install_for_this_platform() {
        for entry in &McpCatalog::builtin().connectors {
            let spec = McpVerifiedSpec::for_entry(entry).unwrap();
            assert_eq!(spec.is_some(), !entry.is_remote(), "{}", entry.id);
        }
        let playwright = McpCatalog::builtin().get("playwright").unwrap();
        let spec = McpVerifiedSpec::for_entry(playwright).unwrap().unwrap();
        let McpVerifiedSource::Npm { package, bin, lock } = &spec.source else {
            panic!("npm");
        };
        assert_eq!(
            (package.as_str(), bin.as_str()),
            ("@playwright/mcp", "cli.js")
        );
        assert!(lock
            .iter()
            .any(|entry| entry.path == "node_modules/playwright-core"));
    }

    #[test]
    fn python_servers_install_wheels_with_required_hashes_and_no_dependencies_of_their_own() {
        let requirements = vec![
            McpPythonRequirement {
                name: "mcp-server-demo".to_string(),
                version: "1.0".to_string(),
                marker: None,
                hashes: vec![format!("sha256:{}", "a".repeat(64))],
            },
            McpPythonRequirement {
                name: "pywin32".to_string(),
                version: "311".to_string(),
                marker: Some("sys_platform == 'win32'".to_string()),
                hashes: vec![
                    format!("sha256:{}", "b".repeat(64)),
                    format!("sha256:{}", "c".repeat(64)),
                ],
            },
        ];
        assert_eq!(
            mcp_requirements_text(&requirements),
            format!(
                "mcp-server-demo==1.0 --hash=sha256:{}\npywin32==311 ; sys_platform == 'win32' --hash=sha256:{} --hash=sha256:{}\n",
                "a".repeat(64),
                "b".repeat(64),
                "c".repeat(64)
            )
        );
        let [venv, install] = python_steps(Path::new("/x"));
        assert_eq!(venv[0], "venv");
        for flag in ["--require-hashes", "--no-deps", "--only-binary", ":all:"] {
            assert!(install.iter().any(|arg| arg == flag), "{flag}");
        }

        let git = McpCatalog::builtin().get("git").unwrap();
        let spec = McpVerifiedSpec::for_entry(git).unwrap().unwrap();
        let McpVerifiedSource::Python { script, .. } = &spec.source else {
            panic!("python");
        };
        assert_eq!(script, "mcp-server-git");
        let launch = super::launch_of(&spec, Path::new("/home/me/.neuron/mcp/git/1"));
        assert!(launch.command.contains("venv"), "{}", launch.command);
        assert_eq!(launch.args, vec!["--repository", "."]);
    }

    #[test]
    fn install_folders_are_named_safely() {
        let home = temp("names");
        let store = McpInstallStore::in_config_home(&home);
        let (mut spec, fixtures) = npm_fixture();
        spec.version = "../../escape".to_string();
        assert!(store
            .ensure(&spec, &fixtures)
            .unwrap_err()
            .contains("cannot name an install folder"));
        let _ = fs::remove_dir_all(home);
    }
}
