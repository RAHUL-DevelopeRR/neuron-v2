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
    if let Some(problem) = path_problem(file) {
        return Err(problem.into());
    }
    assert_safe(root)?;
    let head = git(
        root,
        &[
            "--literal-pathspecs",
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
            "--literal-pathspecs",
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

/// Why a path may not be reverted or opened, or `None` when it is a plain
/// relative path (no `..`, nothing absolute or option-like).
pub fn path_problem(file: &str) -> Option<&'static str> {
    if file.is_empty() || file.chars().any(char::is_control) {
        return Some("That is not a file path.");
    }
    let drive = file.len() >= 2 && file.as_bytes()[1] == b':';
    if file.starts_with('-')
        || file.starts_with(['/', '\\'])
        || drive
        || Path::new(file).is_absolute()
        || file.split(['/', '\\']).any(|part| part == "..")
    {
        return Some("That path is outside the workspace.");
    }
    None
}

// Branch changes: everything a chat's copy changed since its branch left its
// base. The result's shape is ported from Freebuff, common/src/desktop-cloud/
// changes.ts (`ChangedFile`, `totals`), commit 0dc95cb
// (https://github.com/CodebuffAI/freebuff). Copyright Codebuff, Apache License
// 2.0. Changed for Neuron: Rust types, `commits` and `truncated` added, no
// modification times. See ui/neuron-desktop/NOTICE.

/// The most files the branch scope lists.
const MAX_BRANCH_FILES: usize = 2_000;
/// New files up to this size have their lines counted.
const MAX_COUNTED_BYTES: u64 = 512 * 1024;

/// One file the branch changed, committed or not.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BranchFile {
    /// Relative to the copy's top folder, with `/`.
    pub path: String,
    /// `added`, `modified` or `deleted`.
    pub status: String,
    /// Lines added and removed; `None` for a binary file.
    pub adds: Option<u64>,
    pub dels: Option<u64>,
    pub binary: bool,
    /// New, and not known to git yet.
    pub untracked: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct BranchTotals {
    pub files: usize,
    pub adds: u64,
    pub dels: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BranchChanges {
    pub branch: Option<String>,
    /// What the branch is compared with: the branch or commit it was made from.
    pub base: String,
    /// Commits on the branch since it left the base.
    pub commits: u64,
    pub files: Vec<BranchFile>,
    pub totals: BranchTotals,
    /// More files changed than are listed.
    pub truncated: bool,
}

/// `git diff --numstat -z`: `adds\tdels\tpath` records; `-` counts for a binary file.
fn parse_numstat(text: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
    text.split('\0')
        .filter_map(|record| {
            let mut parts = record.splitn(3, '\t');
            let adds = parts.next()?.trim();
            let dels = parts.next()?;
            let path = parts.next()?;
            (!path.is_empty()).then(|| (path.to_string(), adds.parse().ok(), dels.parse().ok()))
        })
        .collect()
}

/// `git diff --name-status -z --no-renames`: a status letter, then the path.
fn parse_name_status(text: &str) -> std::collections::HashMap<String, &'static str> {
    let mut found = std::collections::HashMap::new();
    let mut parts = text.split('\0').filter(|part| !part.is_empty());
    while let (Some(code), Some(path)) = (parts.next(), parts.next()) {
        let status = match code.chars().next() {
            Some('A') => "added",
            Some('D') => "deleted",
            _ => "modified",
        };
        found.insert(path.to_string(), status);
    }
    found
}

/// A commit or branch name to compare with: the same check as a branch the window
/// names, so nothing git would read as an option, a range or a revision expression.
fn plausible_base(base: &str) -> bool {
    crate::worktree::plausible_branch(base)
}

/// Where the branch left `base`: their merge base.
fn merge_base(root: &Path, base: &str) -> Result<String, String> {
    if !plausible_base(base) {
        return Err("That is not a branch to compare with.".into());
    }
    let run = git(root, &["merge-base", base, "HEAD"], true)?;
    let commit = run.stdout.trim().to_string();
    if !run.ok || commit.is_empty() {
        return Err(format!("Could not find where this branch left {base}."));
    }
    Ok(commit)
}

/// Lines in a new file, when it is text and small enough to count.
fn count_lines(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_COUNTED_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let lines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    Some(lines + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n")))
}

/// Everything the copy at `root` changed since its branch left `base`: the
/// commits on the branch (`<merge-base>..HEAD`) and what is not committed yet,
/// together, as `git diff <merge-base>` sees the working tree, plus new files.
pub fn branch_changes(root: &Path, base: &str) -> Result<BranchChanges, String> {
    assert_safe(root)?;
    let since = merge_base(root, base)?;
    let head = git(root, &["rev-parse", "--abbrev-ref", "HEAD"], true)?;
    let branch = Some(head.stdout.trim().to_string())
        .filter(|name| head.ok && !name.is_empty() && name != "HEAD");
    let range = format!("{since}..HEAD");
    let commits = git(root, &["rev-list", "--count", &range], true)?;
    let commits = commits.stdout.trim().parse().unwrap_or(0);
    let common = [
        "--no-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
    ];
    let mut numstat = vec!["diff", "--numstat", "-z"];
    numstat.extend(common);
    numstat.extend([since.as_str(), "--"]);
    let counts = git(root, &numstat, true)?;
    let mut names = vec!["diff", "--name-status", "-z"];
    names.extend(common);
    names.extend([since.as_str(), "--"]);
    let names = git(root, &names, true)?;
    if !counts.ok || !names.ok {
        return Err("git could not compare this branch.".into());
    }
    let statuses = parse_name_status(&names.stdout);
    let mut files: Vec<BranchFile> = parse_numstat(&counts.stdout)
        .into_iter()
        .map(|(path, adds, dels)| BranchFile {
            status: statuses
                .get(&path)
                .copied()
                .unwrap_or("modified")
                .to_string(),
            binary: adds.is_none() && dels.is_none(),
            path,
            adds,
            dels,
            untracked: false,
        })
        .collect();
    let untracked = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
        true,
    )?;
    if untracked.ok {
        for path in untracked.stdout.split('\0').filter(|path| !path.is_empty()) {
            let lines = count_lines(&root.join(path));
            files.push(BranchFile {
                path: path.to_string(),
                status: "added".into(),
                adds: lines,
                dels: lines.map(|_| 0),
                binary: lines.is_none(),
                untracked: true,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let totals = BranchTotals {
        files: files.len(),
        adds: files.iter().filter_map(|file| file.adds).sum(),
        dels: files.iter().filter_map(|file| file.dels).sum(),
    };
    let truncated = files.len() > MAX_BRANCH_FILES;
    files.truncate(MAX_BRANCH_FILES);
    Ok(BranchChanges {
        branch,
        base: base.to_string(),
        commits,
        files,
        totals,
        truncated,
    })
}

/// One file's change since the branch left `base`, committed or not, as a
/// unified diff. A new file shows as all added.
pub fn branch_diff(root: &Path, base: &str, file: &str) -> Result<String, String> {
    if let Some(problem) = path_problem(file) {
        return Err(problem.into());
    }
    assert_safe(root)?;
    let since = merge_base(root, base)?;
    let untracked = git(
        root,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            file,
        ],
        true,
    )?;
    if untracked.ok && untracked.stdout.split('\0').any(|listed| listed == file) {
        // `--no-index` compares the empty file with the new one; it exits 1 when they differ.
        let added = git(
            root,
            &[
                "diff",
                "--no-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--",
                "/dev/null",
                file,
            ],
            true,
        )?;
        return Ok(added.stdout);
    }
    let run = git(
        root,
        &[
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            &since,
            "--",
            file,
        ],
        true,
    )?;
    Ok(if run.ok { run.stdout } else { String::new() })
}

/// Git's unmerged status pairs: `DD`, `AU`, `UD`, `UA`, `DU`, `AA`, `UU`, and
/// anything with a `U` in either column.
fn is_unmerged(index: &str, worktree: &str) -> bool {
    matches!(
        (index, worktree),
        ("D", "D") | ("A", "U") | ("U", "D") | ("U", "A") | ("D", "U") | ("A", "A") | ("U", "U")
    ) || index == "U"
        || worktree == "U"
}

/// L2: review and git. Puts one changed file back as HEAD has it. A new
/// (untracked) file is deleted. The path must be one of the files `git status`
/// lists right now, and must stay inside the open folder.
pub fn revert(root: &Path, file: &str) -> Result<(), String> {
    if let Some(problem) = path_problem(file) {
        return Err(problem.into());
    }
    if file.ends_with('/') {
        return Err(
            "Neuron reverts files one at a time. Delete this new folder yourself if you want it gone."
                .into(),
        );
    }
    assert_safe(root)?;
    // Re-read the status here: the window's list may be stale.
    let current = status(root)?.ok_or("This project is not a git repository.")?;
    let change = current
        .changes
        .iter()
        .find(|change| change.path == file)
        .ok_or("That file has no changes to revert.")?;
    let (index, worktree) = (change.index.as_str(), change.worktree.as_str());
    // A conflicted file is never touched: `git rm -f` would delete it.
    if is_unmerged(index, worktree) {
        return Err(
            "Neuron can't revert a renamed or conflicted file. Use git for this one.".into(),
        );
    }
    let target = root.join(file);
    let inside = match std::fs::canonicalize(&target) {
        Ok(real) => real.starts_with(root),
        // A deleted file is gone, but its folder is not: check that instead.
        Err(_) => target
            .parent()
            .and_then(|parent| std::fs::canonicalize(parent).ok())
            .is_some_and(|parent| parent.starts_with(root)),
    };
    if !inside {
        return Err("That path is outside the workspace.".into());
    }
    if index == "?" {
        // `std::fs` removes the file itself (or a link, never what it points at).
        let meta = std::fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
        if meta.is_dir() {
            return Err("Neuron reverts files one at a time.".into());
        }
        return std::fs::remove_file(&target).map_err(|error| error.to_string());
    }
    if matches!(index, "R" | "C") {
        return Err(
            "Neuron can't revert a renamed or conflicted file. Use git for this one.".into(),
        );
    }
    let run = if index == "A" {
        git(
            root,
            &["--literal-pathspecs", "rm", "-q", "-f", "--", file],
            true,
        )?
    } else {
        git(
            root,
            &[
                "--literal-pathspecs",
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--",
                file,
            ],
            true,
        )?
    };
    if run.ok {
        Ok(())
    } else {
        Err("git could not revert that file.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo_with_changes(label: &str) -> std::path::PathBuf {
        let repo = temp(label);
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/a.txt"), "a\n").unwrap();
        fs::write(repo.join("b.txt"), "b\n").unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-q", "-m", "init"]);
        repo
    }

    #[test]
    fn revert_restores_a_tracked_file_and_deletes_an_untracked_one() {
        let repo = repo_with_changes("revert");
        fs::write(repo.join("src/a.txt"), "changed\n").unwrap();
        fs::write(repo.join("b.txt"), "staged\n").unwrap();
        run(&repo, &["add", "b.txt"]);
        fs::write(repo.join("new.txt"), "new\n").unwrap();
        let root = fs::canonicalize(&repo).unwrap();

        revert(&root, "src/a.txt").unwrap();
        // Git may check files out with CRLF (autocrlf on Windows).
        let text = |path: &str| fs::read_to_string(repo.join(path)).unwrap();
        assert_eq!(text("src/a.txt").trim_end(), "a");
        revert(&root, "b.txt").unwrap();
        assert_eq!(text("b.txt").trim_end(), "b");
        revert(&root, "new.txt").unwrap();
        assert!(!repo.join("new.txt").exists());
        assert!(status(&root).unwrap().unwrap().changes.is_empty());
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn revert_puts_back_a_deleted_file_and_removes_a_staged_new_one() {
        let repo = repo_with_changes("revert-deleted");
        fs::remove_file(repo.join("b.txt")).unwrap();
        fs::write(repo.join("added.txt"), "x\n").unwrap();
        run(&repo, &["add", "added.txt"]);
        let root = fs::canonicalize(&repo).unwrap();
        revert(&root, "b.txt").unwrap();
        assert!(repo.join("b.txt").exists());
        revert(&root, "added.txt").unwrap();
        assert!(!repo.join("added.txt").exists());
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn revert_refuses_paths_that_are_unsafe_or_not_in_the_status() {
        let repo = repo_with_changes("revert-refuse");
        fs::write(repo.join("b.txt"), "changed\n").unwrap();
        let outside = repo.parent().unwrap().join("neuron-outside.txt");
        fs::write(&outside, "keep\n").unwrap();
        let root = fs::canonicalize(&repo).unwrap();
        let absolute = outside.display().to_string();
        for bad in [
            "../neuron-outside.txt",
            "src/../../neuron-outside.txt",
            "..\\neuron-outside.txt",
            "/etc/passwd",
            "C:\\Windows\\win.ini",
            absolute.as_str(),
            "-rf",
            "",
        ] {
            let error = revert(&root, bad).unwrap_err();
            assert!(
                error.contains("outside the workspace") || error.contains("not a file path"),
                "{bad}: {error}"
            );
        }
        // Clean tracked files and unknown files are not in the status.
        assert!(revert(&root, "src/a.txt")
            .unwrap_err()
            .contains("no changes"));
        assert!(revert(&root, "nothing.txt")
            .unwrap_err()
            .contains("no changes"));
        // `.` opens the folder in an editor (L4's chat bar); it never reverts it.
        assert!(revert(&root, ".").unwrap_err().contains("no changes"));
        assert!(outside.exists());
        assert_eq!(fs::read_to_string(repo.join("b.txt")).unwrap(), "changed\n");
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn revert_leaves_whole_untracked_folders_alone() {
        let repo = repo_with_changes("revert-dir");
        fs::create_dir_all(repo.join("fresh")).unwrap();
        fs::write(repo.join("fresh/x.txt"), "x\n").unwrap();
        let root = fs::canonicalize(&repo).unwrap();
        assert!(revert(&root, "fresh/").is_err());
        assert!(repo.join("fresh/x.txt").exists());
        let _ = fs::remove_dir_all(repo);
    }

    /// Two branches that both add `clash.txt` (AA), or both rename `b.txt` apart (DD).
    fn conflicted(label: &str, rename: bool) -> std::path::PathBuf {
        let repo = repo_with_changes(label);
        run(&repo, &["checkout", "-q", "-b", "other"]);
        if rename {
            run(&repo, &["mv", "b.txt", "b-other.txt"]);
        } else {
            fs::write(
                repo.join("clash.txt"),
                "other
",
            )
            .unwrap();
            run(&repo, &["add", "clash.txt"]);
        }
        run(&repo, &["commit", "-q", "-m", "other"]);
        run(&repo, &["checkout", "-q", "main"]);
        if rename {
            run(&repo, &["mv", "b.txt", "b-main.txt"]);
        } else {
            fs::write(
                repo.join("clash.txt"),
                "main
",
            )
            .unwrap();
            run(&repo, &["add", "clash.txt"]);
        }
        run(&repo, &["commit", "-q", "-m", "main"]);
        // The merge stops on a conflict: the command itself reports failure.
        let _ = Command::new("git")
            .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
            .args(["merge", "other", "-q", "-m", "merge"])
            .current_dir(&repo)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        repo
    }

    #[test]
    fn revert_refuses_a_both_added_conflict_and_keeps_the_file() {
        let repo = conflicted("revert-aa", false);
        let root = fs::canonicalize(&repo).unwrap();
        let view = status(&root).unwrap().unwrap();
        let change = view.changes.iter().find(|c| c.path == "clash.txt").unwrap();
        assert_eq!(
            (change.index.as_str(), change.worktree.as_str()),
            ("A", "A")
        );
        assert!(revert(&root, "clash.txt")
            .unwrap_err()
            .contains("conflicted"));
        assert!(repo.join("clash.txt").exists());
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn revert_refuses_a_both_deleted_conflict() {
        let repo = conflicted("revert-dd", true);
        let root = fs::canonicalize(&repo).unwrap();
        let view = status(&root).unwrap().unwrap();
        let change = view.changes.iter().find(|c| c.path == "b.txt").unwrap();
        assert_eq!(
            (change.index.as_str(), change.worktree.as_str()),
            ("D", "D")
        );
        assert!(revert(&root, "b.txt").unwrap_err().contains("conflicted"));
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn every_unmerged_pair_is_recognised() {
        for pair in ["DD", "AU", "UD", "UA", "DU", "AA", "UU", "UM", "MU"] {
            assert!(is_unmerged(&pair[0..1], &pair[1..2]), "{pair}");
        }
        for pair in ["MM", " M", "A ", "??", "D ", " D", "R "] {
            assert!(!is_unmerged(&pair[0..1], &pair[1..2]), "{pair}");
        }
    }

    #[test]
    fn diff_refuses_pathspec_magic_and_odd_paths() {
        let repo = repo_with_changes("diff-magic");
        let root = fs::canonicalize(&repo).unwrap();
        for bad in [":(top)b.txt", ":/b.txt", "", "/abs", "-x", "a/../b"] {
            let result = diff(&root, bad);
            if bad.starts_with(':') {
                // Not path_problem material, but literal: it matches no file.
                assert_eq!(result.unwrap(), "", "{bad}");
            } else {
                assert!(result.is_err(), "{bad}");
            }
        }
        let _ = fs::remove_dir_all(repo);
    }

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
    fn numstat_and_name_status_records_are_read() {
        assert_eq!(
            parse_numstat("3\t1\tsrc/a.rs\0-\t-\tlogo.png\x0010\t0\tsp ace.txt\0"),
            [
                ("src/a.rs".to_string(), Some(3), Some(1)),
                ("logo.png".to_string(), None, None),
                ("sp ace.txt".to_string(), Some(10), Some(0)),
            ]
        );
        assert!(parse_numstat("").is_empty());
        let statuses = parse_name_status("A\0new.txt\0D\0gone.txt\0M\0a.rs\0T\0link\0");
        assert_eq!(statuses["new.txt"], "added");
        assert_eq!(statuses["gone.txt"], "deleted");
        assert_eq!(statuses["a.rs"], "modified");
        assert_eq!(statuses["link"], "modified");
    }

    #[test]
    fn branch_changes_cover_commits_since_the_base_and_uncommitted_work() {
        let repo = repo_with_changes("branch");
        run(&repo, &["checkout", "-q", "-b", "neuron/work"]);
        fs::write(repo.join("src/a.txt"), "a\nmore\nlines\n").unwrap();
        fs::remove_file(repo.join("b.txt")).unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "work"]);
        // Main moves on after the branch left it: that is not the branch's change.
        run(&repo, &["checkout", "-q", "main"]);
        fs::write(repo.join("main-only.txt"), "m\n").unwrap();
        run(&repo, &["add", "-A"]);
        run(&repo, &["commit", "-q", "-m", "main"]);
        run(&repo, &["checkout", "-q", "neuron/work"]);
        // Not committed yet: a change and a new file.
        fs::write(repo.join("src/a.txt"), "a\nmore\nlines\nand one\n").unwrap();
        fs::write(repo.join("new.txt"), "one\ntwo").unwrap();
        let root = fs::canonicalize(&repo).unwrap();

        let changes = branch_changes(&root, "main").unwrap();
        assert_eq!(changes.branch.as_deref(), Some("neuron/work"));
        assert_eq!(changes.commits, 1);
        let rows: Vec<_> = changes
            .files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.status.as_str(),
                    f.adds,
                    f.dels,
                    f.untracked,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("b.txt", "deleted", Some(0), Some(1), false),
                ("new.txt", "added", Some(2), Some(0), true),
                ("src/a.txt", "modified", Some(3), Some(0), false),
            ]
        );
        assert_eq!(
            changes.totals,
            BranchTotals {
                files: 3,
                adds: 5,
                dels: 1
            }
        );
        assert!(!changes.files.iter().any(|f| f.path == "main-only.txt"));

        let diff = branch_diff(&root, "main", "src/a.txt").unwrap();
        assert!(
            diff.contains("+more") && diff.contains("+and one"),
            "{diff}"
        );
        let added = branch_diff(&root, "main", "new.txt").unwrap();
        assert!(added.contains("+one"), "{added}");
        assert!(branch_diff(&root, "main", "../x").is_err());
        let _ = fs::remove_dir_all(repo);
    }

    #[test]
    fn branch_changes_refuse_odd_bases() {
        let repo = repo_with_changes("branch-base");
        let root = fs::canonicalize(&repo).unwrap();
        for bad in ["", "--output=x", "main..HEAD", "a b", "nope"] {
            assert!(branch_changes(&root, bad).is_err(), "{bad:?}");
        }
        let none = branch_changes(&root, "main").unwrap();
        assert_eq!((none.commits, none.files.len()), (0, 0));
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
