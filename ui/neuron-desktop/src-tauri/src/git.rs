//! The project's git changes for the Changes panel, read the way the engine reads
//! git for itself (runtime/src/git_context.rs): no optional locks, no fsmonitor,
//! no external diff or textconv, nothing at all in a repository whose own config
//! names a program for git to run, and only the open folder's part of the
//! repository. A folder no repository tracks (say, under a home directory kept
//! in git) is no repository.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Git gets this long; a repository too big to answer in time shows no changes.
const GIT_TIMEOUT: Duration = Duration::from_secs(8);

const PROGRAM_CONFIG: &str = r"^(core\.fsmonitor|filter\..*\.(clean|smudge|process)|diff\.external|diff\..*\.(command|textconv))$";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitChange {
    /// Relative to the open folder, with `/`.
    pub path: String,
    pub index: String,
    pub worktree: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GitStatusView {
    pub branch: Option<String>,
    pub changes: Vec<GitChange>,
}

struct Run {
    ok: bool,
    code: Option<i32>,
    stdout: String,
}

fn git(root: &Path, args: &[&str], hardened: bool) -> Result<Run, String> {
    let mut command = Command::new("git");
    if hardened {
        command.args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "log.showSignature=false",
        ]);
    }
    command
        .args(args)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    let mut stdout = child.stdout.take().ok_or("git has no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + GIT_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("git took too long for this repository.".into());
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    Ok(Run {
        ok: status.success(),
        code: status.code(),
        stdout: reader.join().unwrap_or_default(),
    })
}

/// Refuses a repository whose own (local or worktree) config names a program git
/// would run for status or diffs. Reading config runs nothing.
fn assert_safe(root: &Path) -> Result<(), String> {
    let config = git(
        root,
        &["config", "--show-scope", "--get-regexp", PROGRAM_CONFIG],
        false,
    )?;
    if config.code == Some(1) {
        return Ok(());
    }
    let repo_scoped = !config.ok
        || config
            .stdout
            .lines()
            .any(|line| matches!(line.split('\t').next(), Some("local" | "worktree")));
    if repo_scoped {
        return Err("This repository's own git config runs programs for status and diffs, so Neuron doesn't run git here.".into());
    }
    Ok(())
}

/// Where the folder sits in its repository (`""` at the top, `dir/` inside), or
/// `None` when no repository tracks it.
fn repository_prefix(root: &Path) -> Result<Option<String>, String> {
    let prefix = git(root, &["rev-parse", "--show-prefix"], true)?;
    if !prefix.ok {
        return Ok(None);
    }
    let dir = prefix.stdout.trim().to_string();
    if dir.is_empty() {
        return Ok(Some(dir));
    }
    // Pathspecs are relative to the folder git runs in: `.` is the open folder.
    let tracked = git(root, &["ls-tree", "--name-only", "HEAD", "--", "."], true)?;
    Ok((tracked.ok && !tracked.stdout.trim().is_empty()).then_some(dir))
}

pub fn status(root: &Path) -> Result<Option<GitStatusView>, String> {
    assert_safe(root)?;
    let Some(prefix) = repository_prefix(root)? else {
        return Ok(None);
    };
    let run = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--branch",
            "--untracked-files=normal",
            "--",
            ".",
        ],
        true,
    )?;
    if !run.ok {
        return Ok(None);
    }
    let mut branch = None;
    let mut changes = Vec::new();
    let mut records = run.stdout.split('\0').filter(|record| !record.is_empty());
    while let Some(record) = records.next() {
        if let Some(head) = record.strip_prefix("## ") {
            let name = head.split("...").next().unwrap_or_default();
            let name = name.strip_prefix("No commits yet on ").unwrap_or(name);
            branch = (!name.is_empty()).then(|| name.to_string());
            continue;
        }
        if record.len() < 4 {
            continue;
        }
        let (index, worktree) = (&record[0..1], &record[1..2]);
        // Porcelain paths are relative to the repository's top.
        if let Some(path) = record[3..].strip_prefix(prefix.as_str()) {
            changes.push(GitChange {
                path: path.to_string(),
                index: index.to_string(),
                worktree: worktree.to_string(),
            });
        }
        // A rename's source path follows as its own record.
        if matches!(index, "R" | "C") {
            records.next();
        }
    }
    Ok(Some(GitStatusView { branch, changes }))
}

/// One file's change against HEAD (staged and unstaged), as a unified diff.
pub fn diff(root: &Path, file: &str) -> Result<String, String> {
    if file.starts_with('-')
        || Path::new(file).is_absolute()
        || file.split(['/', '\\']).any(|part| part == "..")
    {
        return Err("That path is outside the workspace.".into());
    }
    assert_safe(root)?;
    let head = git(
        root,
        &[
            "diff",
            "HEAD",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            file,
        ],
        true,
    )?;
    if head.ok {
        return Ok(head.stdout);
    }
    let cached = git(
        root,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            file,
        ],
        true,
    )?;
    Ok(if cached.ok {
        cached.stdout
    } else {
        String::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(args)
            .current_dir(dir)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("git runs")
            .success();
        assert!(ok, "git {args:?}");
    }

    fn temp(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("neuron-desktop-git-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn changes_of_the_open_folder_only_with_their_diffs() {
        let repo = temp("repo");
        fs::create_dir_all(repo.join("pkg")).unwrap();
        fs::write(repo.join("pkg/a.txt"), "a\n").unwrap();
        fs::write(repo.join("outside.txt"), "x\n").unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        fs::write(repo.join("pkg/a.txt"), "changed\n").unwrap();
        fs::write(repo.join("pkg/new.txt"), "new\n").unwrap();
        fs::write(repo.join("outside.txt"), "changed too\n").unwrap();

        let pkg = repo.join("pkg");
        let view = status(&pkg).unwrap().expect("a repository");
        assert_eq!(view.branch.as_deref(), Some("main"));
        let mut paths: Vec<_> = view
            .changes
            .iter()
            .map(|change| (change.path.as_str(), change.worktree.as_str()))
            .collect();
        paths.sort();
        assert_eq!(paths, [("a.txt", "M"), ("new.txt", "?")]);
        assert!(diff(&pkg, "a.txt").unwrap().contains("+changed"));
        assert_eq!(
            diff(&pkg, "../outside.txt").unwrap_err(),
            "That path is outside the workspace."
        );
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn untracked_folders_and_programs_in_repo_config() {
        let repo = temp("untracked");
        fs::write(repo.join("tracked.txt"), "t\n").unwrap();
        run(&repo, &["init", "-q"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        let loose = repo.join("loose");
        fs::create_dir_all(&loose).unwrap();
        fs::write(loose.join("x.txt"), "x\n").unwrap();
        assert_eq!(
            status(&loose).unwrap(),
            None,
            "an untracked folder is no repository"
        );

        run(&repo, &["config", "core.fsmonitor", "echo nope"]);
        assert!(status(&repo).unwrap_err().contains("runs programs"));
        let _ = fs::remove_dir_all(repo);
    }
}
