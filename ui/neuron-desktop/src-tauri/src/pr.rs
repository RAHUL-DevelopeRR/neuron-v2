// Shapes ported from Freebuff, common/src/desktop-cloud/pull-request.ts, commit
// 0dc95cb (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache
// License 2.0. Changed for Neuron: Rust types instead of zod schemas, `conflict`
// added, and the checks passed on raw. See ui/neuron-desktop/NOTICE.

//! The pull request of a chat's branch, read with the GitHub CLI (`gh pr view`)
//! when it is installed and signed in. Nothing here prompts or fails loudly: no
//! `gh`, no sign-in, no pull request and any other trouble are all "nothing to
//! show". Answers are kept for a minute per folder and branch.
//!
//! The snapshot's shape follows Freebuff's desktop-cloud `pull-request.ts`
//! (`PullRequestStatus`, `PullRequestSnapshot`); the checks are passed on raw
//! and summed up in the window (`src/pullRequest.ts`). See ui/neuron-desktop/NOTICE.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

/// How long an answer is reused.
pub const PR_CACHE_TTL: Duration = Duration::from_secs(60);
const GH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_CHECKS: usize = 200;
const MAX_TITLE: usize = 300;
const FIELDS: &str = "url,number,title,state,isDraft,mergeable,statusCheckRollup";

/// One entry of GitHub's `statusCheckRollup` (a check run or a commit status),
/// reduced to what decides its state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CheckEntry {
    pub status: Option<String>,
    pub state: Option<String>,
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PullRequestView {
    pub url: String,
    pub number: u64,
    pub title: String,
    /// `open`, `merged` or `closed`.
    pub state: String,
    pub draft: bool,
    /// The pull request no longer merges cleanly.
    pub conflict: bool,
    /// `None` when GitHub sent no list (the window says "unknown").
    pub checks: Option<Vec<CheckEntry>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestSnapshot {
    pub pull_request: Option<PullRequestView>,
    /// `gh` could not answer (missing, signed out, offline): nothing is known.
    pub unavailable: bool,
}

impl PullRequestSnapshot {
    fn unavailable() -> Self {
        Self {
            pull_request: None,
            unavailable: true,
        }
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.chars().any(char::is_control))
        .map(str::to_string)
}

/// `gh pr view --json ...` output, or `None` when it is not a pull request.
pub fn parse_view(json: &str) -> Option<PullRequestView> {
    let value: Value = serde_json::from_str(json).ok()?;
    let url = text(&value, "url").filter(|url| url.starts_with("https://"))?;
    let number = value.get("number").and_then(Value::as_u64)?;
    let title: String = value
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TITLE)
        .collect();
    let state = match text(&value, "state")?.to_uppercase().as_str() {
        "MERGED" => "merged",
        "CLOSED" => "closed",
        _ => "open",
    }
    .to_string();
    let checks = value
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .take(MAX_CHECKS)
                .map(|entry| CheckEntry {
                    status: text(entry, "status"),
                    state: text(entry, "state"),
                    conclusion: text(entry, "conclusion"),
                })
                .collect()
        });
    Some(PullRequestView {
        url,
        number,
        title,
        state,
        draft: value
            .get("isDraft")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        conflict: text(&value, "mergeable").as_deref() == Some("CONFLICTING"),
        checks,
    })
}

/// What a finished `gh pr view` means: its JSON, "no pull request" (exit 1 with
/// gh's own words), or nothing known.
pub fn interpret(ok: bool, stdout: &str, stderr: &str) -> PullRequestSnapshot {
    if ok {
        return parse_view(stdout).map_or_else(PullRequestSnapshot::unavailable, |view| {
            PullRequestSnapshot {
                pull_request: Some(view),
                unavailable: false,
            }
        });
    }
    if stderr.to_lowercase().contains("no pull requests found") {
        return PullRequestSnapshot::default();
    }
    PullRequestSnapshot::unavailable()
}

/// Runs `gh pr view <branch>` in `dir`: no shell, no prompts, a time limit.
/// A missing `gh` and every failure are "nothing known".
pub fn gh_pr_view(dir: &Path, branch: &str) -> PullRequestSnapshot {
    if !crate::worktree::plausible_branch(branch) {
        return PullRequestSnapshot::unavailable();
    }
    let mut command = Command::new("gh");
    command
        .args(["pr", "view", branch, "--json", FIELDS])
        .current_dir(dir)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let Ok(mut child) = command.spawn() else {
        return PullRequestSnapshot::unavailable();
    };
    let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) else {
        let _ = child.kill();
        let _ = child.wait();
        return PullRequestSnapshot::unavailable();
    };
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + GH_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return PullRequestSnapshot::unavailable();
            }
        }
    };
    interpret(
        status.success(),
        &out.join().unwrap_or_default(),
        &err.join().unwrap_or_default(),
    )
}

/// Answers per folder and branch, each reused for `ttl`.
pub struct PrCache {
    ttl: Duration,
    entries: Mutex<HashMap<(PathBuf, String), (Instant, PullRequestSnapshot)>>,
}

impl Default for PrCache {
    fn default() -> Self {
        Self::new(PR_CACHE_TTL)
    }
}

impl PrCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The answer for `dir` and `branch`: a fresh cached one, or `fetch`'s (run
    /// with the cache unlocked).
    pub fn get(
        &self,
        dir: &Path,
        branch: &str,
        now: Instant,
        fetch: impl FnOnce() -> PullRequestSnapshot,
    ) -> PullRequestSnapshot {
        let key = (dir.to_path_buf(), branch.to_string());
        if let Some((at, snapshot)) = crate::lock(&self.entries).get(&key) {
            if now.saturating_duration_since(*at) < self.ttl {
                return snapshot.clone();
            }
        }
        let snapshot = fetch();
        let mut entries = crate::lock(&self.entries);
        entries.retain(|_, (at, _)| now.saturating_duration_since(*at) < self.ttl);
        entries.insert(key, (now, snapshot.clone()));
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: &str = r#"{
        "url": "https://github.com/acme/app/pull/12",
        "number": 12,
        "title": "Fix the login redirect",
        "state": "OPEN",
        "isDraft": true,
        "mergeable": "MERGEABLE",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "StatusContext", "context": "ci", "state": "PENDING"}
        ]
    }"#;

    #[test]
    fn a_pull_request_is_read_from_gh_json() {
        let view = parse_view(VIEW).unwrap();
        assert_eq!(view.number, 12);
        assert_eq!(view.url, "https://github.com/acme/app/pull/12");
        assert_eq!(view.state, "open");
        assert!(view.draft && !view.conflict);
        let checks = view.checks.unwrap();
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].conclusion.as_deref(), Some("SUCCESS"));
        assert_eq!(checks[1].state.as_deref(), Some("PENDING"));
        assert_eq!(checks[1].status, None);
    }

    #[test]
    fn states_conflicts_and_missing_checks() {
        let merged = parse_view(
            r#"{"url":"https://x/pull/1","number":1,"title":"t","state":"MERGED","isDraft":false,"mergeable":"CONFLICTING"}"#,
        )
        .unwrap();
        assert_eq!(merged.state, "merged");
        assert!(merged.conflict);
        assert_eq!(merged.checks, None);
        let closed = parse_view(
            r#"{"url":"https://x/pull/2","number":2,"title":"a\u0007b","state":"CLOSED","statusCheckRollup":[]}"#,
        )
        .unwrap();
        assert_eq!(closed.state, "closed");
        assert_eq!(closed.title, "a b");
        assert_eq!(closed.checks, Some(vec![]));
    }

    #[test]
    fn odd_json_is_no_pull_request() {
        for bad in [
            "",
            "not json",
            "[]",
            r#"{"number":1,"state":"OPEN"}"#,
            r#"{"url":"javascript:alert(1)","number":1,"state":"OPEN"}"#,
            r#"{"url":"http://x/pull/1","number":1,"state":"OPEN"}"#,
            r#"{"url":"https://x/pull/1","number":-1,"state":"OPEN"}"#,
        ] {
            assert_eq!(parse_view(bad), None, "{bad}");
        }
    }

    #[test]
    fn gh_failures_are_quiet() {
        assert_eq!(
            interpret(
                false,
                "",
                "no pull requests found for branch \"neuron/x\"\n"
            ),
            PullRequestSnapshot::default()
        );
        for stderr in [
            "To get started with GitHub CLI, please run:  gh auth login",
            "error connecting to api.github.com",
            "",
        ] {
            assert_eq!(
                interpret(false, "", stderr),
                PullRequestSnapshot::unavailable(),
                "{stderr}"
            );
        }
        assert_eq!(
            interpret(true, "garbage", ""),
            PullRequestSnapshot::unavailable()
        );
        let found = interpret(true, VIEW, "");
        assert!(!found.unavailable);
        assert_eq!(found.pull_request.unwrap().number, 12);
    }

    #[test]
    fn an_odd_branch_never_reaches_gh() {
        assert!(gh_pr_view(Path::new("."), "--web").unavailable);
        assert!(gh_pr_view(Path::new("."), "").unavailable);
    }

    #[test]
    fn the_snapshot_reads_as_the_window_expects() {
        let json = serde_json::to_value(interpret(true, VIEW, "")).unwrap();
        assert_eq!(json["unavailable"], false);
        assert_eq!(json["pullRequest"]["number"], 12);
        assert_eq!(json["pullRequest"]["draft"], true);
        assert!(json["pullRequest"]["checks"].is_array());
    }

    #[test]
    fn answers_are_reused_for_a_minute_per_folder_and_branch() {
        let cache = PrCache::new(Duration::from_secs(60));
        let calls = std::cell::Cell::new(0);
        let fetch = || {
            calls.set(calls.get() + 1);
            interpret(true, VIEW, "")
        };
        let start = Instant::now();
        let dir = Path::new("/work/app");
        cache.get(dir, "neuron/a", start, fetch);
        cache.get(dir, "neuron/a", start + Duration::from_secs(59), fetch);
        assert_eq!(calls.get(), 1);
        cache.get(dir, "neuron/b", start, fetch);
        cache.get(Path::new("/work/other"), "neuron/a", start, fetch);
        assert_eq!(calls.get(), 3);
        cache.get(dir, "neuron/a", start + Duration::from_secs(61), fetch);
        assert_eq!(calls.get(), 4);
    }
}
