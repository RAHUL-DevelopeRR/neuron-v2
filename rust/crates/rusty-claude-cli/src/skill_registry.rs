//! The network side of `neuron skills search` and remote `neuron skills install`: the
//! public skills.sh registry and GitHub's contents API, over HTTPS with short timeouts.
//! What the answers mean (ranking, package selection, limits, installing) lives in
//! `commands::skill_sources`.
//!
//! Ported in part from Freebuff (Apache-2.0, Codebuff), common/src/util/skill-registry.ts:
//! the registry endpoints (`/api/search?q=&limit=16`, `/api/download/<owner>/<repo>/<slug>`),
//! the 8 s search and 15 s download timeouts, and the messages that tell a rate-limited
//! or offline user what to do instead of "no such skill". Modified for Neuron: Rust,
//! blocking client, GitHub folders, size limits while downloading.

use std::sync::OnceLock;
use std::time::Duration;

use commands::{SkillFile, SkillRemote, SkillSearchHit, MAX_SKILL_BYTES, MAX_SKILL_FILES};
use reqwest::blocking::{Client, Response};
use reqwest::StatusCode;
use serde_json::Value;

const REGISTRY_BASE: &str = "https://skills.sh";
const GITHUB_API_BASE: &str = "https://api.github.com";
const SEARCH_TIMEOUT: Duration = Duration::from_secs(8);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);
/// Folder levels a GitHub skill may have below the folder named.
const MAX_GITHUB_DEPTH: usize = 5;

/// Searches and downloads skills over HTTPS.
pub(crate) struct HttpSkillRemote {
    registry_base: String,
    github_base: String,
    /// Built on first use, so runs that never search pay nothing.
    client: OnceLock<Client>,
}

impl HttpSkillRemote {
    /// The public registry and GitHub, or the hosts in `NEURON_SKILL_REGISTRY_URL` and
    /// `NEURON_GITHUB_API_URL` (a mirror, or a test server).
    pub(crate) fn from_env() -> Self {
        let base = |name: &str, default: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| default.to_string())
        };
        Self::new(
            &base("NEURON_SKILL_REGISTRY_URL", REGISTRY_BASE),
            &base("NEURON_GITHUB_API_URL", GITHUB_API_BASE),
        )
    }

    pub(crate) fn new(registry_base: &str, github_base: &str) -> Self {
        Self {
            registry_base: registry_base.trim_end_matches('/').to_string(),
            github_base: github_base.trim_end_matches('/').to_string(),
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> &Client {
        self.client.get_or_init(|| {
            Client::builder()
                .user_agent(api::USER_AGENT)
                .connect_timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| Client::new())
        })
    }

    /// One GitHub contents request for `path` (segments encoded), as JSON or raw bytes.
    fn github_get(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        reference: Option<&str>,
        raw: bool,
    ) -> Result<Response, String> {
        let encoded = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .map(encode_segment)
            .collect::<Vec<_>>()
            .join("/");
        let mut url = format!(
            "{}/repos/{}/{}/contents/{encoded}",
            self.github_base,
            encode_segment(owner),
            encode_segment(repo)
        );
        if let Some(reference) = reference {
            url.push_str("?ref=");
            url.push_str(&encode_segment(reference));
        }
        let accept = if raw {
            "application/vnd.github.raw"
        } else {
            "application/vnd.github+json"
        };
        let response = self
            .client()
            .get(&url)
            .header("Accept", accept)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .map_err(|error| unreachable_message("GitHub", &error))?;
        match response.status() {
            status if status.is_success() => Ok(response),
            StatusCode::NOT_FOUND => Err(format!(
                "GitHub has no {owner}/{repo}/{path}{} (or the repository is private)",
                reference
                    .map(|reference| format!(" at {reference}"))
                    .unwrap_or_default()
            )),
            StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS => Err(
                "GitHub is rate limiting requests from this machine. Try again in a while."
                    .to_string(),
            ),
            status => Err(format!(
                "GitHub is unavailable right now (HTTP {}). Try again shortly.",
                status.as_u16()
            )),
        }
    }

    /// Adds the files under `path` (relative to `root`) to `files`.
    #[allow(clippy::too_many_arguments)]
    fn collect_github_dir(
        &self,
        owner: &str,
        repo: &str,
        root: &str,
        path: &str,
        reference: Option<&str>,
        depth: usize,
        files: &mut Vec<SkillFile>,
        total: &mut usize,
    ) -> Result<(), String> {
        let listing: Value = self
            .github_get(owner, repo, path, reference, false)?
            .json()
            .map_err(|_| "GitHub returned a malformed answer.".to_string())?;
        let Some(entries) = listing.as_array() else {
            return Err(format!("{path} is not a folder"));
        };
        for entry in entries {
            let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
            let Some(entry_path) = entry.get("path").and_then(Value::as_str) else {
                continue;
            };
            match kind {
                "file" => {
                    if files.len() >= MAX_SKILL_FILES {
                        return Err(too_large());
                    }
                    let contents = self.github_file(owner, repo, entry_path, reference, total)?;
                    files.push(SkillFile {
                        path: relative_to(root, entry_path),
                        contents,
                    });
                }
                "dir" if depth < MAX_GITHUB_DEPTH => self.collect_github_dir(
                    owner,
                    repo,
                    root,
                    entry_path,
                    reference,
                    depth + 1,
                    files,
                    total,
                )?,
                // Deeper folders, symlinks and submodules are left out.
                _ => {}
            }
        }
        Ok(())
    }

    fn github_file(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        reference: Option<&str>,
        total: &mut usize,
    ) -> Result<Vec<u8>, String> {
        let bytes = self
            .github_get(owner, repo, path, reference, true)?
            .bytes()
            .map_err(|error| unreachable_message("GitHub", &error))?;
        *total += bytes.len();
        if *total > MAX_SKILL_BYTES {
            return Err(too_large());
        }
        Ok(bytes.to_vec())
    }
}

impl SkillRemote for HttpSkillRemote {
    fn search(&self, query: &str) -> Result<Vec<SkillSearchHit>, String> {
        let response = self
            .client()
            .get(format!("{}/api/search", self.registry_base))
            // Over-fetch a little: one row per name still leaves about ten.
            .query(&[("q", query), ("limit", "16")])
            .timeout(SEARCH_TIMEOUT)
            .send()
            .map_err(|error| unreachable_message("the skill registry", &error))?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            return Err(rate_limited());
        }
        if !response.status().is_success() {
            return Err(format!(
                "The skill registry is unavailable right now (HTTP {}). Try again shortly.",
                response.status().as_u16()
            ));
        }
        let body: Value = response
            .json()
            .map_err(|_| "The skill registry returned a malformed response.".to_string())?;
        Ok(commands::parse_search_response(&body))
    }

    fn registry_download(
        &self,
        source: &str,
        slug: &str,
    ) -> Result<Option<Vec<SkillFile>>, String> {
        let Some((owner, repo)) = source.split_once('/') else {
            return Ok(None);
        };
        let url = format!(
            "{}/api/download/{}/{}/{}",
            self.registry_base,
            encode_segment(owner),
            encode_segment(repo),
            encode_segment(slug)
        );
        let response = self
            .client()
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send()
            .map_err(|error| unreachable_message("the skill registry", &error))?;
        match response.status() {
            // Only a 404 is the registry saying the skill is not there.
            StatusCode::NOT_FOUND => return Ok(None),
            StatusCode::TOO_MANY_REQUESTS => return Err(rate_limited()),
            status if !status.is_success() => {
                return Err(format!(
                    "The skill registry is unavailable right now (HTTP {}). Try again shortly.",
                    status.as_u16()
                ))
            }
            _ => {}
        }
        if response
            .content_length()
            .is_some_and(|length| length > (MAX_SKILL_BYTES as u64) * 2)
        {
            return Err(too_large());
        }
        let body: Value = response
            .json()
            .map_err(|_| "The skill registry returned a malformed response.".to_string())?;
        Ok(Some(commands::parse_download_response(&body)))
    }

    fn github_files(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        reference: Option<&str>,
    ) -> Result<Vec<SkillFile>, String> {
        let mut total = 0;
        // One markdown file: the whole skill.
        if path.to_ascii_lowercase().ends_with(".md") {
            let contents = self.github_file(owner, repo, path, reference, &mut total)?;
            let name = path.rsplit('/').next().unwrap_or(path).to_string();
            return Ok(vec![SkillFile {
                path: name,
                contents,
            }]);
        }
        let mut files = Vec::new();
        self.collect_github_dir(
            owner, repo, path, path, reference, 0, &mut files, &mut total,
        )?;
        Ok(files)
    }
}

/// `path` relative to the folder `root` (both as GitHub prints them).
fn relative_to(root: &str, path: &str) -> String {
    let root = root.trim_matches('/');
    if root.is_empty() {
        return path.to_string();
    }
    path.strip_prefix(root)
        .map_or(path, |rest| rest.trim_start_matches('/'))
        .to_string()
}

/// Percent-encodes everything outside the URL path's unreserved characters.
fn encode_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn unreachable_message(service: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("{service} took too long to respond. Try again in a moment.")
    } else {
        format!("Could not reach {service}. Check your connection and try again.")
    }
}

fn rate_limited() -> String {
    "The skill registry is rate limiting this machine. Try again in a few minutes.".to_string()
}

fn too_large() -> String {
    format!(
        "That skill is too large to install (the limits are {MAX_SKILL_FILES} files and {MAX_SKILL_BYTES} bytes)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// A tiny HTTP server answering GET requests from `routes` (path and query to
    /// status and body); it records each request line and Accept header.
    fn serve(routes: BTreeMap<String, (u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                let mut request = String::new();
                if reader.read_line(&mut request).is_err() {
                    continue;
                }
                let target = request.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut accept = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("accept:") {
                        accept = value.trim().to_string();
                    }
                }
                log.lock()
                    .expect("log")
                    .push(format!("{target} [{accept}]"));
                let (status, body) = routes
                    .get(&target)
                    .cloned()
                    .unwrap_or((404, "{}".to_string()));
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (base, seen)
    }

    #[test]
    fn searches_and_downloads_from_the_registry() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "/api/search?q=docker+files&limit=16".to_string(),
            (
                200,
                r#"{"skills":[{"skillId":"docker-patterns","name":"docker-patterns","source":"a/one","installs":7}]}"#
                    .to_string(),
            ),
        );
        routes.insert(
            "/api/download/a/one/docker-patterns".to_string(),
            (
                200,
                r#"{"files":[{"path":"SKILL.md","contents":"hi"}]}"#.to_string(),
            ),
        );
        routes.insert(
            "/api/download/a/one/busy".to_string(),
            (429, "{}".to_string()),
        );
        let (base, _) = serve(routes);
        let remote = HttpSkillRemote::new(&base, &base);

        let hits = remote.search("docker files").expect("search");
        assert_eq!(hits[0].id, "a/one/docker-patterns");
        let files = remote
            .registry_download("a/one", "docker-patterns")
            .expect("download")
            .expect("found");
        assert_eq!(files[0].contents, b"hi");
        assert_eq!(remote.registry_download("a/one", "missing"), Ok(None));
        let busy = remote.registry_download("a/one", "busy").unwrap_err();
        assert!(busy.contains("rate limiting"), "{busy}");
    }

    #[test]
    fn reads_a_github_folder_with_its_subfolders() {
        let mut routes = BTreeMap::new();
        routes.insert(
            "/repos/acme/tools/contents/skills/triage?ref=main".to_string(),
            (
                200,
                r#"[{"type":"file","path":"skills/triage/SKILL.md"},
                    {"type":"dir","path":"skills/triage/scripts"},
                    {"type":"symlink","path":"skills/triage/link"}]"#
                    .to_string(),
            ),
        );
        routes.insert(
            "/repos/acme/tools/contents/skills/triage/scripts?ref=main".to_string(),
            (
                200,
                r#"[{"type":"file","path":"skills/triage/scripts/run.sh"}]"#.to_string(),
            ),
        );
        routes.insert(
            "/repos/acme/tools/contents/skills/triage/SKILL.md?ref=main".to_string(),
            (200, "---\nname: triage\n---\n".to_string()),
        );
        routes.insert(
            "/repos/acme/tools/contents/skills/triage/scripts/run.sh?ref=main".to_string(),
            (200, "echo run".to_string()),
        );
        let (base, seen) = serve(routes);
        let remote = HttpSkillRemote::new(&base, &base);

        let files = remote
            .github_files("acme", "tools", "skills/triage", Some("main"))
            .expect("files");
        let paths = files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["SKILL.md", "scripts/run.sh"]);
        assert_eq!(files[1].contents, b"echo run");
        let seen = seen.lock().expect("log").clone();
        assert!(
            seen.iter()
                .any(|line| line.ends_with("SKILL.md?ref=main [application/vnd.github.raw]")),
            "{seen:?}"
        );

        let missing = remote
            .github_files("acme", "tools", "nothing", None)
            .unwrap_err();
        assert!(
            missing.contains("GitHub has no acme/tools/nothing"),
            "{missing}"
        );
    }

    #[test]
    fn encodes_path_segments_and_strips_the_root() {
        assert_eq!(encode_segment("Release Notes"), "Release%20Notes");
        assert_eq!(encode_segment("v1.2"), "v1.2");
        assert_eq!(relative_to("skills/a", "skills/a/x/y.md"), "x/y.md");
        assert_eq!(relative_to("", "SKILL.md"), "SKILL.md");
    }
}
