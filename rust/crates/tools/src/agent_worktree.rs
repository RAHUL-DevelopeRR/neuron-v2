//! Git worktrees for sub-agents (`Agent` with `isolation: "worktree"`).
//!
//! The sub-agent gets a worktree of its own at
//! `<NEURON_HOME>/worktrees/<workspace>/<agent id>`, on a new branch cut from `HEAD`,
//! and works there: its thread resolves paths against the worktree
//! ([`runtime::workdir`]), so the parent's working tree is untouched. When it ends,
//! a worktree it changed nothing in is removed with its branch; otherwise both stay
//! and the result names the branch to review or merge.
//!
//! `<workspace>` is the repository's fingerprint (as for the session store), since the
//! tools do not know the parent's session id.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::{Deserialize, Serialize};

/// The value of the `Agent` tool's `isolation` input that asks for a worktree.
pub(crate) const ISOLATION_WORKTREE: &str = "worktree";

/// Branches made for sub-agents start with this.
const BRANCH_PREFIX: &str = "neuron/agent/";

/// A sub-agent's worktree while it runs.
#[derive(Debug, Clone)]
pub(crate) struct AgentWorktree {
    /// The repository the worktree belongs to (its main working tree's top level).
    repo_top: PathBuf,
    /// The worktree's top level.
    path: PathBuf,
    /// Where the sub-agent works: the worktree's counterpart of the parent's folder.
    workdir: PathBuf,
    branch: String,
    /// The commit the branch was cut from.
    base: String,
}

/// The worktree in the `Agent` result and manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorktreeInfo {
    pub(crate) branch: String,
    pub(crate) path: String,
    /// The commit the branch was cut from.
    pub(crate) base: String,
    /// Set when the sub-agent has ended: whether it changed anything (a commit on the
    /// branch, or uncommitted changes in the worktree).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) changed: Option<bool>,
    /// Commits on the branch since `base`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) commits: usize,
    /// Changes in the worktree not committed yet.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) uncommitted: bool,
    /// The worktree and branch were removed because nothing changed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) removed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl AgentWorktree {
    /// A new worktree for `agent_id` of the repository `cwd` is in, under `home`
    /// (`<home>/worktrees/<workspace>/<agent id>`), on a new branch from `HEAD`.
    pub(crate) fn create(cwd: &Path, home: &Path, agent_id: &str) -> Result<Self, String> {
        let top = git_text(cwd, &["rev-parse", "--show-toplevel"]).map_err(|_| {
            format!(
                "isolation \"worktree\" needs a git repository, and {} is not in one. Leave isolation out, or run Neuron inside a repository.",
                cwd.display()
            )
        })?;
        let repo_top = PathBuf::from(top);
        let base = git_text(&repo_top, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| {
            "isolation \"worktree\" needs a commit to branch from, and this repository has none yet. Commit first.".to_string()
        })?;
        let prefix = git_text(cwd, &["rev-parse", "--show-prefix"]).unwrap_or_default();
        let path = home
            .join("worktrees")
            .join(runtime::session_control::workspace_fingerprint(&repo_top))
            .join(agent_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let branch = format!("{BRANCH_PREFIX}{agent_id}");
        let path_text = path.to_string_lossy().into_owned();
        git_text(
            &repo_top,
            &[
                "worktree", "add", "--quiet", "-b", &branch, &path_text, &base,
            ],
        )?;
        let workdir = prefix
            .split('/')
            .filter(|part| !part.is_empty())
            .fold(path.clone(), |dir, part| dir.join(part));
        let workdir = if workdir.is_dir() {
            workdir
        } else {
            path.clone()
        };
        Ok(Self {
            repo_top,
            path,
            workdir,
            branch,
            base,
        })
    }

    /// Where the sub-agent works.
    pub(crate) fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// The worktree's top level.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// The repository's main working tree.
    pub(crate) fn repo_top(&self) -> &Path {
        &self.repo_top
    }

    pub(crate) fn branch(&self) -> &str {
        &self.branch
    }

    /// The commit the branch was cut from.
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /// Removes the worktree, keeping its branch.
    pub(crate) fn remove_keeping_branch(&self) -> Result<(), String> {
        let path_text = self.path.to_string_lossy().into_owned();
        git_text(
            &self.repo_top,
            &["worktree", "remove", "--force", &path_text],
        )
        .map(|_| ())
    }

    pub(crate) fn info(&self) -> WorktreeInfo {
        WorktreeInfo {
            branch: self.branch.clone(),
            path: self.path.display().to_string(),
            base: self.base.clone(),
            ..WorktreeInfo::default()
        }
    }

    /// After the sub-agent ended: what it changed. Without changes, the worktree and
    /// its branch are removed.
    pub(crate) fn finish(&self) -> WorktreeInfo {
        let mut info = self.info();
        let range = format!("{}..{}", self.base, self.branch);
        let commits = git_text(&self.repo_top, &["rev-list", "--count", &range])
            .ok()
            .and_then(|count| count.parse::<usize>().ok());
        let status = git_text(
            &self.path,
            &["status", "--porcelain", "--untracked-files=all"],
        );
        let (Some(commits), Ok(status)) = (commits, status) else {
            info.note = Some(
                "Could not tell whether the sub-agent changed anything; the worktree was kept."
                    .to_string(),
            );
            return info;
        };
        info.commits = commits;
        info.uncommitted = !status.is_empty();
        let changed = commits > 0 || info.uncommitted;
        info.changed = Some(changed);
        if changed {
            info.note = Some(format!(
                "The sub-agent's work is on branch {} in {}; review it there, merge it (git merge {}), or remove it (git worktree remove {} && git branch -D {}).",
                self.branch,
                self.path.display(),
                self.branch,
                self.path.display(),
                self.branch
            ));
            return info;
        }
        let path_text = self.path.to_string_lossy().into_owned();
        let removed = git_text(
            &self.repo_top,
            &["worktree", "remove", "--force", &path_text],
        )
        .and_then(|_| git_text(&self.repo_top, &["branch", "-D", &self.branch]));
        match removed {
            Ok(_) => info.removed = true,
            Err(error) => {
                info.note = Some(format!(
                    "Nothing changed, but the worktree could not be removed: {error}"
                ));
            }
        }
        info
    }
}

/// `git <args>` in `cwd`, trimmed stdout; an error with git's message when it fails.
/// In an untrusted workspace whose repository config would run programs, git does
/// not run at all.
pub(crate) fn git_text(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let mut command: Command = runtime::workspace_git_command(cwd).ok_or_else(|| {
        "This workspace is not trusted and its git configuration runs programs, so Neuron does not run git here. Trust the workspace first.".to_string()
    })?;
    let output = command
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(failure(args, &output))
    }
}

fn failure(args: &[&str], output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let name = args.first().copied().unwrap_or("git");
    if stderr.is_empty() {
        format!("git {name} failed ({})", output.status)
    } else {
        format!("git {name} failed: {stderr}")
    }
}

#[cfg(test)]
mod tests {
    use super::AgentWorktree;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "neuron-agent-worktree-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("root");
            Self(root)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(args: &[&str], cwd: &Path) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository with one commit and a `src` folder.
    fn repo(root: &Path) -> PathBuf {
        let repo = root.join("repo");
        fs::create_dir_all(repo.join("src")).expect("src");
        git(&["init", "--quiet", "-b", "main"], &repo);
        git(&["config", "user.email", "tests@example.com"], &repo);
        git(&["config", "user.name", "Neuron Tests"], &repo);
        git(&["config", "commit.gpgsign", "false"], &repo);
        fs::write(repo.join("src").join("lib.txt"), "one\n").expect("file");
        git(&["add", "-A"], &repo);
        git(&["commit", "-q", "-m", "init"], &repo);
        repo
    }

    fn branches(repo: &Path) -> String {
        git(&["branch", "--list", "neuron/agent/*"], repo)
    }

    #[test]
    fn an_unchanged_worktree_is_removed_with_its_branch() {
        let temp = TempDir::new("unchanged");
        let repo = repo(&temp.0);
        let home = temp.0.join("home");
        let worktree = AgentWorktree::create(&repo, &home, "agent-1").expect("worktree");
        let info = worktree.info();
        assert_eq!(info.branch, "neuron/agent/agent-1");
        assert!(Path::new(&info.path).starts_with(home.join("worktrees")));
        assert!(Path::new(&info.path).ends_with("agent-1"));
        assert!(worktree.workdir().join("src").join("lib.txt").is_file());
        assert!(branches(&repo).contains("neuron/agent/agent-1"));

        let finished = worktree.finish();
        assert_eq!(finished.changed, Some(false));
        assert!(finished.removed, "{finished:?}");
        assert!(!Path::new(&info.path).exists());
        assert_eq!(branches(&repo), "");
    }

    #[test]
    fn a_changed_worktree_is_kept_and_reports_its_branch() {
        let temp = TempDir::new("changed");
        let repo = repo(&temp.0);
        let home = temp.0.join("home");
        // From a subfolder, the sub-agent works in the same subfolder of its worktree.
        let worktree =
            AgentWorktree::create(&repo.join("src"), &home, "agent-2").expect("worktree");
        assert!(worktree.workdir().ends_with("src"));
        fs::write(worktree.workdir().join("lib.txt"), "two\n").expect("edit");
        let path = PathBuf::from(worktree.info().path);
        git(&["commit", "-qam", "agent change"], &path);
        fs::write(worktree.workdir().join("new.txt"), "new\n").expect("new");

        let finished = worktree.finish();
        assert_eq!(finished.changed, Some(true));
        assert_eq!(finished.commits, 1);
        assert!(finished.uncommitted);
        assert!(!finished.removed);
        assert!(finished
            .note
            .as_deref()
            .is_some_and(|note| note.contains("neuron/agent/agent-2")));
        assert!(path.exists());
        // The parent's working tree is untouched.
        assert_eq!(
            fs::read_to_string(repo.join("src").join("lib.txt")).expect("read"),
            "one\n"
        );
        git(
            &["worktree", "remove", "--force", &path.to_string_lossy()],
            &repo,
        );
    }

    #[test]
    fn outside_a_repository_the_worktree_is_refused_clearly() {
        let temp = TempDir::new("no-repo");
        let plain = temp.0.join("plain");
        fs::create_dir_all(&plain).expect("plain");
        let error = AgentWorktree::create(&plain, &temp.0.join("home"), "agent-3")
            .expect_err("no repository");
        // A folder above the temp folder may itself be an empty repository (it is on
        // some machines): then the refusal is for the missing commit.
        assert!(
            error.contains("needs a git repository") || error.contains("needs a commit"),
            "{error}"
        );
    }
}
