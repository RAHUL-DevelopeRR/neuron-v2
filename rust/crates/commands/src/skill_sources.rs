//! Skills that come from outside this machine: the public skills.sh registry and
//! folders on GitHub. This module parses sources and responses, picks the files of a
//! skill package and installs them; the HTTP calls are a [`SkillRemote`] the CLI
//! provides, so everything here runs without a network in tests.
//!
//! Ported in part from Freebuff (Apache-2.0, Codebuff),
//! common/src/util/skill-registry.ts: the search ranking (most installs first, one row
//! per skill name, ten rows), a 404 meaning "no such skill" while other failures say
//! what to do, and choosing SKILL.md, then AGENTS.md, then any .md file as the
//! instructions with the package rooted at that file's folder. Modified for Neuron:
//! Rust, GitHub folders as a second source, path and size limits, staged installs.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

/// At most this many files in one installed skill.
pub const MAX_SKILL_FILES: usize = 200;
/// At most this many bytes in one installed skill, all files together.
pub const MAX_SKILL_BYTES: usize = 2 * 1024 * 1024;
/// Rows a search shows.
pub const SEARCH_RESULT_LIMIT: usize = 10;

/// One skill the registry found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSearchHit {
    /// `owner/repo/slug`: what `neuron skills install` takes.
    pub id: String,
    pub name: String,
    pub slug: String,
    /// `owner/repo` of the repository that holds it.
    pub source: String,
    pub installs: u64,
}

/// One file of a skill package, at a `/`-separated path inside the package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillFile {
    pub path: String,
    pub contents: Vec<u8>,
}

/// Where a skill to install lives, when it is not a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteSkillSource {
    /// A registry skill: `owner/repo/slug`, or a bare `slug` that a search resolves.
    Registry {
        source: Option<String>,
        slug: String,
    },
    /// `github:owner/repo[/path][@ref]`: a folder (or one markdown file) on GitHub.
    GitHub {
        owner: String,
        repo: String,
        path: String,
        reference: Option<String>,
    },
}

impl RemoteSkillSource {
    /// How the source is shown in reports.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Registry {
                source: Some(source),
                slug,
            } => format!("skills.sh registry: {source}/{slug}"),
            Self::Registry { source: None, slug } => format!("skills.sh registry: {slug}"),
            Self::GitHub {
                owner,
                repo,
                path,
                reference,
            } => {
                let mut label = format!("github:{owner}/{repo}");
                if !path.is_empty() {
                    label.push('/');
                    label.push_str(path);
                }
                if let Some(reference) = reference {
                    label.push('@');
                    label.push_str(reference);
                }
                label
            }
        }
    }

    /// The name to install under when the skill's front matter has none.
    fn fallback_name(&self) -> Option<String> {
        match self {
            Self::Registry { slug, .. } => Some(slug.clone()),
            Self::GitHub { repo, path, .. } => {
                let last = path.rsplit('/').find(|segment| !segment.is_empty());
                Some(
                    last.map_or(repo.as_str(), |segment| {
                        segment.strip_suffix(".md").unwrap_or(segment)
                    })
                    .to_string(),
                )
            }
        }
    }
}

/// The network side of remote skills. The CLI implements it over HTTPS; tests use a
/// stand-in.
pub trait SkillRemote {
    /// Registry search, ranked by [`parse_search_response`].
    fn search(&self, query: &str) -> Result<Vec<SkillSearchHit>, String>;
    /// The registry's files for `source` (`owner/repo`) and `slug`; `Ok(None)` when the
    /// registry has no such skill.
    fn registry_download(&self, source: &str, slug: &str)
        -> Result<Option<Vec<SkillFile>>, String>;
    /// The files under `path` in a GitHub repository (or the one file `path` names),
    /// at paths relative to `path`.
    fn github_files(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        reference: Option<&str>,
    ) -> Result<Vec<SkillFile>, String>;
}

/// Used until the CLI installs its network client: says so instead of failing late.
struct NoRemote;

impl SkillRemote for NoRemote {
    fn search(&self, _query: &str) -> Result<Vec<SkillSearchHit>, String> {
        Err(NO_REMOTE.to_string())
    }
    fn registry_download(
        &self,
        _source: &str,
        _slug: &str,
    ) -> Result<Option<Vec<SkillFile>>, String> {
        Err(NO_REMOTE.to_string())
    }
    fn github_files(
        &self,
        _owner: &str,
        _repo: &str,
        _path: &str,
        _reference: Option<&str>,
    ) -> Result<Vec<SkillFile>, String> {
        Err(NO_REMOTE.to_string())
    }
}

const NO_REMOTE: &str = "this build cannot reach skill registries; install from a local path";

static SKILL_REMOTE: OnceLock<Box<dyn SkillRemote + Send + Sync>> = OnceLock::new();

/// Sets the network client `/skills search` and remote installs use. The first call
/// wins.
pub fn set_skill_remote(remote: Box<dyn SkillRemote + Send + Sync>) {
    let _ = SKILL_REMOTE.set(remote);
}

pub(crate) fn skill_remote() -> &'static dyn SkillRemote {
    static FALLBACK: NoRemote = NoRemote;
    match SKILL_REMOTE.get() {
        Some(remote) => remote.as_ref(),
        None => &FALLBACK,
    }
}

/// Reads `text` as a remote source: `github:owner/repo[/path][@ref]`,
/// `owner/repo/slug`, or a bare registry `slug`. `None` for anything else (a local
/// path is checked before this is asked).
#[must_use]
pub fn parse_remote_skill_source(text: &str) -> Option<RemoteSkillSource> {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("github:") {
        let (location, reference) = match rest.rsplit_once('@') {
            Some((location, reference)) if valid_reference(reference) => {
                (location, Some(reference.to_string()))
            }
            Some(_) => return None,
            None => (rest, None),
        };
        let mut segments = location.split('/').filter(|segment| !segment.is_empty());
        let owner = segments.next().filter(|owner| valid_name(owner))?;
        let repo = segments.next().filter(|repo| valid_name(repo))?;
        let path = segments.collect::<Vec<_>>();
        if path
            .iter()
            .any(|segment| matches!(*segment, "." | "..") || segment.contains('\\'))
        {
            return None;
        }
        return Some(RemoteSkillSource::GitHub {
            owner: owner.to_string(),
            repo: repo.to_string(),
            path: path.join("/"),
            reference,
        });
    }
    let segments = text.split('/').collect::<Vec<_>>();
    match segments.as_slice() {
        [owner, repo, slug] if valid_name(owner) && valid_name(repo) && valid_slug(slug) => {
            Some(RemoteSkillSource::Registry {
                source: Some(format!("{owner}/{repo}")),
                slug: (*slug).to_string(),
            })
        }
        [slug] if valid_slug(slug) => Some(RemoteSkillSource::Registry {
            source: None,
            slug: (*slug).to_string(),
        }),
        _ => None,
    }
}

fn valid_name(text: &str) -> bool {
    !text.is_empty()
        && !matches!(text, "." | "..")
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn valid_slug(text: &str) -> bool {
    valid_name(text) && !text.starts_with('.') && !text.ends_with(".md")
}

fn valid_reference(text: &str) -> bool {
    !text.is_empty()
        && !text.contains("..")
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/'))
}

/// The registry's search answer as ranked hits: most installs first, one row per skill
/// name (the registry lists the same name from several repositories), at most
/// [`SEARCH_RESULT_LIMIT`]. Rows without a slug or a source are dropped.
#[must_use]
pub fn parse_search_response(body: &Value) -> Vec<SkillSearchHit> {
    let mut hits = body
        .get("skills")
        .and_then(Value::as_array)
        .map(|skills| {
            skills
                .iter()
                .filter_map(|raw| {
                    let text = |key: &str| raw.get(key).and_then(Value::as_str).unwrap_or("");
                    let slug = text("skillId").to_string();
                    let source = text("source").to_string();
                    if !valid_slug(&slug) || source.split('/').count() != 2 {
                        return None;
                    }
                    let name = match text("name") {
                        "" => slug.clone(),
                        name => name.to_string(),
                    };
                    Some(SkillSearchHit {
                        id: format!("{source}/{slug}"),
                        name,
                        slug,
                        source,
                        installs: raw.get("installs").and_then(Value::as_u64).unwrap_or(0),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    hits.sort_by(|left, right| right.installs.cmp(&left.installs));
    let mut seen = std::collections::BTreeSet::new();
    hits.retain(|hit| seen.insert(hit.name.clone()));
    hits.truncate(SEARCH_RESULT_LIMIT);
    hits
}

/// The registry's download answer as files. Entries that are not `{path, contents}`
/// strings are left out.
#[must_use]
pub fn parse_download_response(body: &Value) -> Vec<SkillFile> {
    body.get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| {
                    Some(SkillFile {
                        path: file.get("path")?.as_str()?.to_string(),
                        contents: file.get("contents")?.as_str()?.as_bytes().to_vec(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The skill package in `files`: the instructions are SKILL.md, else AGENTS.md, else
/// the first markdown file; the package is rooted at that file's folder (files
/// elsewhere are left out) and the instructions are saved as SKILL.md. `None` when
/// there is no markdown file.
#[must_use]
pub fn select_skill_package(files: Vec<SkillFile>) -> Option<Vec<SkillFile>> {
    let file_name = |file: &SkillFile| {
        file.path
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let pick = files
        .iter()
        .find(|file| file_name(file) == "skill.md")
        .or_else(|| files.iter().find(|file| file_name(file) == "agents.md"))
        .or_else(|| {
            files
                .iter()
                .find(|file| file.path.to_ascii_lowercase().ends_with(".md"))
        })?
        .path
        .clone();
    let prefix = pick
        .rfind('/')
        .map_or("", |slash| &pick[..=slash])
        .to_string();
    Some(
        files
            .into_iter()
            .filter_map(|file| {
                let relative = file.path.strip_prefix(&prefix)?.to_string();
                let path = if file.path == pick {
                    "SKILL.md".to_string()
                } else if relative.eq_ignore_ascii_case("skill.md") {
                    // A second SKILL.md next to the picked instructions keeps its text
                    // under another name instead of replacing them.
                    "SKILL.original.md".to_string()
                } else {
                    relative
                };
                Some(SkillFile {
                    path,
                    contents: file.contents,
                })
            })
            .collect(),
    )
}

/// `path` as a relative path that stays inside the package, or `None` (absolute,
/// `..`, a drive or a stream name, or empty).
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.contains('\\') || path.contains(':') || path.contains('\0') {
        return None;
    }
    let candidate = Path::new(path);
    let mut safe = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => safe.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    (!safe.as_os_str().is_empty()).then_some(safe)
}

/// Fetches the skill `source` names through `remote` and installs it under
/// `registry_root`, the way a local install does: refused when a skill of that name
/// is installed already.
pub(crate) fn install_remote_skill(
    source: &RemoteSkillSource,
    remote: &dyn SkillRemote,
    registry_root: &Path,
) -> std::io::Result<super::InstalledSkill> {
    let other = |message: String| std::io::Error::other(message);
    let (source, files) = match source {
        RemoteSkillSource::Registry { source: None, slug } => {
            let resolved = resolve_registry_slug(slug, remote).map_err(other)?;
            let files = registry_files(&resolved, remote)?;
            (resolved, files)
        }
        RemoteSkillSource::Registry { .. } => (source.clone(), registry_files(source, remote)?),
        RemoteSkillSource::GitHub {
            owner,
            repo,
            path,
            reference,
        } => {
            let files = remote
                .github_files(owner, repo, path, reference.as_deref())
                .map_err(other)?;
            (source.clone(), files)
        }
    };
    let files = select_skill_package(files).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} has no SKILL.md or other markdown file", source.label()),
        )
    })?;
    check_package_limits(&files, &source)?;

    let instructions = files
        .iter()
        .find(|file| file.path == "SKILL.md")
        .map(|file| String::from_utf8_lossy(&file.contents).to_string())
        .unwrap_or_default();
    let display_name = super::parse_skill_frontmatter(&instructions).0;
    let invocation_name = [display_name.clone(), source.fallback_name()]
        .into_iter()
        .flatten()
        .find_map(|candidate| super::sanitize_skill_invocation_name(&candidate))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unable to derive a skill name from {}", source.label()),
            )
        })?;
    let installed_path = registry_root.join(&invocation_name);
    if installed_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "skill '{invocation_name}' is already installed at {}",
                installed_path.display()
            ),
        ));
    }
    write_package(&files, registry_root, &installed_path)?;
    Ok(super::InstalledSkill {
        invocation_name,
        display_name,
        source: PathBuf::from(source.label()),
        registry_root: registry_root.to_path_buf(),
        installed_path,
    })
}

/// The registry skill a bare `slug` names: the search hit with exactly that slug and
/// the most installs.
fn resolve_registry_slug(
    slug: &str,
    remote: &dyn SkillRemote,
) -> Result<RemoteSkillSource, String> {
    remote
        .search(slug)?
        .into_iter()
        .filter(|hit| hit.slug.eq_ignore_ascii_case(slug))
        .max_by_key(|hit| hit.installs)
        .map(|hit| RemoteSkillSource::Registry {
            source: Some(hit.source),
            slug: hit.slug,
        })
        .ok_or_else(|| {
            format!(
                "no local path `{slug}` and no registry skill with that exact name; try `neuron skills search {slug}`"
            )
        })
}

fn registry_files(
    source: &RemoteSkillSource,
    remote: &dyn SkillRemote,
) -> std::io::Result<Vec<SkillFile>> {
    let RemoteSkillSource::Registry {
        source: Some(repository),
        slug,
    } = source
    else {
        return Err(std::io::Error::other("not a registry skill"));
    };
    remote
        .registry_download(repository, slug)
        .map_err(std::io::Error::other)?
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("the registry has no skill {repository}/{slug}"),
            )
        })
}

fn check_package_limits(files: &[SkillFile], source: &RemoteSkillSource) -> std::io::Result<()> {
    let total = files.iter().map(|file| file.contents.len()).sum::<usize>();
    if files.len() > MAX_SKILL_FILES || total > MAX_SKILL_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} is too large to install as a skill ({} files, {total} bytes; the limits are {MAX_SKILL_FILES} files and {MAX_SKILL_BYTES} bytes)",
                source.label(),
                files.len()
            ),
        ));
    }
    if let Some(file) = files
        .iter()
        .find(|file| safe_relative_path(&file.path).is_none())
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "{} has a file outside its folder ({}); not installed",
                source.label(),
                file.path
            ),
        ));
    }
    Ok(())
}

/// Writes `files` into a staging folder under `registry_root`, then moves it to
/// `installed_path`, so a failure leaves no half-installed skill.
fn write_package(
    files: &[SkillFile],
    registry_root: &Path,
    installed_path: &Path,
) -> std::io::Result<()> {
    fs::create_dir_all(registry_root)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let staging = registry_root.join(format!(".installing-{nanos}"));
    let written = (|| {
        for file in files {
            let relative = safe_relative_path(&file.path).ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, file.path.clone())
            })?;
            let target = staging.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(target, &file.contents)?;
        }
        fs::rename(&staging, installed_path)
    })();
    if written.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::RefCell;

    fn file(path: &str, contents: &str) -> SkillFile {
        SkillFile {
            path: path.to_string(),
            contents: contents.as_bytes().to_vec(),
        }
    }

    #[test]
    fn parses_remote_sources_and_refuses_paths_that_climb() {
        assert_eq!(
            parse_remote_skill_source("github:acme/tools/skills/triage@v1.2"),
            Some(RemoteSkillSource::GitHub {
                owner: "acme".into(),
                repo: "tools".into(),
                path: "skills/triage".into(),
                reference: Some("v1.2".into()),
            })
        );
        assert_eq!(
            parse_remote_skill_source("github:acme/tools"),
            Some(RemoteSkillSource::GitHub {
                owner: "acme".into(),
                repo: "tools".into(),
                path: String::new(),
                reference: None,
            })
        );
        assert_eq!(
            parse_remote_skill_source("affaan-m/ecc/docker-patterns"),
            Some(RemoteSkillSource::Registry {
                source: Some("affaan-m/ecc".into()),
                slug: "docker-patterns".into(),
            })
        );
        assert_eq!(
            parse_remote_skill_source("docker-patterns"),
            Some(RemoteSkillSource::Registry {
                source: None,
                slug: "docker-patterns".into(),
            })
        );
        for refused in [
            "github:acme/tools/../secrets",
            "github:acme",
            "github:acme/tools@../x",
            "./skills/triage",
            "notes.md",
            "a/b/c/d",
            "C:\\skills\\x",
        ] {
            assert_eq!(parse_remote_skill_source(refused), None, "{refused}");
        }
    }

    /// Most installs first, one row per name, ten rows at most, broken rows dropped.
    #[test]
    fn ranks_search_results_like_the_registry_client() {
        let mut skills = vec![
            json!({"skillId": "docker-patterns", "name": "docker-patterns", "source": "b/two", "installs": 15}),
            json!({"skillId": "docker-patterns", "name": "docker-patterns", "source": "a/one", "installs": 90}),
            json!({"skillId": "", "source": "a/one", "installs": 1000}),
            json!({"skillId": "x", "source": "no-slash", "installs": 1000}),
        ];
        for index in 0..12 {
            skills.push(
                json!({"skillId": format!("s{index}"), "source": "c/three", "installs": index}),
            );
        }
        let hits = parse_search_response(&json!({ "skills": skills }));
        assert_eq!(hits.len(), SEARCH_RESULT_LIMIT);
        assert_eq!(hits[0].id, "a/one/docker-patterns");
        assert_eq!(hits[0].installs, 90);
        assert_eq!(
            hits.iter()
                .filter(|hit| hit.name == "docker-patterns")
                .count(),
            1
        );
        assert_eq!(hits[1].name, "s11", "a missing name falls back to the slug");
        assert!(parse_search_response(&json!({"error": "x"})).is_empty());
    }

    #[test]
    fn picks_skill_md_then_agents_md_then_any_markdown_and_roots_the_package() {
        let package = select_skill_package(vec![
            file("README.md", "top readme"),
            file("pkg/AGENTS.md", "agents"),
            file("pkg/SKILL.md", "skill"),
            file("pkg/scripts/run.sh", "echo"),
        ])
        .expect("package");
        let paths = package
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["AGENTS.md", "SKILL.md", "scripts/run.sh"]);
        assert_eq!(package[1].contents, b"skill");

        let package = select_skill_package(vec![
            file("docs/guide.md", "guide"),
            file("docs/AGENTS.md", "agents"),
        ])
        .expect("package");
        let skill = package
            .iter()
            .find(|file| file.path == "SKILL.md")
            .expect("skill");
        assert_eq!(skill.contents, b"agents");

        let package = select_skill_package(vec![file("notes/howto.md", "how")]).expect("package");
        assert_eq!(package, vec![file("SKILL.md", "how")]);
        assert!(select_skill_package(vec![file("run.sh", "echo")]).is_none());
        assert_eq!(
            parse_download_response(&json!({"files": [
                {"path": "SKILL.md", "contents": "x"},
                {"path": 3, "contents": "y"},
                "junk"
            ]})),
            vec![file("SKILL.md", "x")]
        );
    }

    #[test]
    fn relative_paths_must_stay_inside_the_package() {
        assert_eq!(
            safe_relative_path("a/b.md"),
            Some(PathBuf::from("a").join("b.md"))
        );
        assert_eq!(safe_relative_path("./a.md"), Some(PathBuf::from("a.md")));
        for refused in [
            "../a",
            "/etc/passwd",
            "a/../../b",
            "C:/x",
            "a\\b",
            "x:stream",
            "",
        ] {
            assert_eq!(safe_relative_path(refused), None, "{refused}");
        }
    }

    /// A stand-in registry and GitHub that record what was asked.
    #[derive(Default)]
    struct FakeRemote {
        asked: RefCell<Vec<String>>,
        files: Vec<SkillFile>,
    }

    impl SkillRemote for FakeRemote {
        fn search(&self, query: &str) -> Result<Vec<SkillSearchHit>, String> {
            self.asked.borrow_mut().push(format!("search {query}"));
            Ok(vec![
                SkillSearchHit {
                    id: "small/repo/triage".into(),
                    name: "triage".into(),
                    slug: "triage".into(),
                    source: "small/repo".into(),
                    installs: 3,
                },
                SkillSearchHit {
                    id: "big/repo/triage".into(),
                    name: "triage".into(),
                    slug: "triage".into(),
                    source: "big/repo".into(),
                    installs: 300,
                },
            ])
        }
        fn registry_download(
            &self,
            source: &str,
            slug: &str,
        ) -> Result<Option<Vec<SkillFile>>, String> {
            self.asked
                .borrow_mut()
                .push(format!("download {source}/{slug}"));
            Ok((slug != "missing").then(|| self.files.clone()))
        }
        fn github_files(
            &self,
            owner: &str,
            repo: &str,
            path: &str,
            reference: Option<&str>,
        ) -> Result<Vec<SkillFile>, String> {
            self.asked.borrow_mut().push(format!(
                "github {owner}/{repo}/{path}@{}",
                reference.unwrap_or("-")
            ));
            Ok(self.files.clone())
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("skill-sources-{label}-{nanos}"))
    }

    #[test]
    fn installs_a_bare_slug_from_the_most_installed_repository() {
        let root = temp_root("slug");
        let remote = FakeRemote {
            files: vec![
                file(
                    "SKILL.md",
                    "---\nname: triage\ndescription: Sort issues\n---\nSteps\n",
                ),
                file("scripts/label.sh", "echo label"),
            ],
            ..FakeRemote::default()
        };
        let source = parse_remote_skill_source("triage").expect("source");
        let installed = install_remote_skill(&source, &remote, &root).expect("installs");
        assert_eq!(installed.invocation_name, "triage");
        assert_eq!(
            installed.source,
            PathBuf::from("skills.sh registry: big/repo/triage")
        );
        assert!(root.join("triage").join("SKILL.md").is_file());
        assert!(root
            .join("triage")
            .join("scripts")
            .join("label.sh")
            .is_file());
        assert_eq!(
            remote.asked.borrow().as_slice(),
            ["search triage", "download big/repo/triage"]
        );
        // No staging folder is left behind, and a second install is refused.
        let leftovers = fs::read_dir(&root)
            .expect("root")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".installing")
            })
            .count();
        assert_eq!(leftovers, 0);
        let again = install_remote_skill(&source, &remote, &root).unwrap_err();
        assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn installs_a_github_folder_and_names_it_after_the_folder() {
        let root = temp_root("github");
        let remote = FakeRemote {
            files: vec![file("SKILL.md", "No front matter.\n")],
            ..FakeRemote::default()
        };
        let source = parse_remote_skill_source("github:acme/tools/skills/Release Notes@main")
            .expect("source");
        let installed = install_remote_skill(&source, &remote, &root).expect("installs");
        assert_eq!(installed.invocation_name, "release-notes");
        assert_eq!(
            remote.asked.borrow().as_slice(),
            ["github acme/tools/skills/Release Notes@main"]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn refuses_packages_that_escape_or_are_too_large_and_missing_skills() {
        let root = temp_root("refused");
        let source = parse_remote_skill_source("acme/tools/triage").expect("source");
        let escaping = FakeRemote {
            files: vec![file("SKILL.md", "x"), file("../evil.sh", "rm")],
            ..FakeRemote::default()
        };
        let error = install_remote_skill(&source, &escaping, &root).unwrap_err();
        assert!(error.to_string().contains("outside its folder"), "{error}");

        let large = FakeRemote {
            files: vec![file("SKILL.md", &"x".repeat(MAX_SKILL_BYTES + 1))],
            ..FakeRemote::default()
        };
        let error = install_remote_skill(&source, &large, &root).unwrap_err();
        assert!(error.to_string().contains("too large"), "{error}");

        let missing = parse_remote_skill_source("acme/tools/missing").expect("source");
        let error = install_remote_skill(&missing, &FakeRemote::default(), &root).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(!root.join("triage").exists());
        let _ = fs::remove_dir_all(root);
    }
}
