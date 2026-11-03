//! Source control for the full-screen UI: the working tree's changes against `HEAD`
//! (`/diff`), and commits (`/commit`), with the messages drafted for them.
//!
//! Git runs at the repository's top level through [`runtime::workspace_git_command`] (no
//! optional locks; in an untrusted workspace no repository config that runs programs),
//! so every path here is relative to the top level, with `/`. Diffs never run external
//! diff programs or textconv filters.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use neuron_tui::protocol::{ChangedFile, DiffFile};
use runtime::sensitive_paths::sensitive_path_reason;

use crate::line_diff::{diff_lines, is_binary, line_breaks};

/// Most files a diff event carries; later ones are left out and the event says so.
pub(crate) const MAX_DIFF_FILES: usize = 200;
/// Most patch text, in bytes, the files of a diff event carry together.
pub(crate) const MAX_PATCH_BYTES: usize = 256 * 1024;
/// Files larger than this are counted, but not diffed.
pub(crate) const MAX_TEXT_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Longest commit subject.
pub(crate) const MAX_SUBJECT_CHARS: usize = 72;
/// Most diff text a request for a drafted commit message carries.
pub(crate) const MAX_DRAFT_DIFF_BYTES: usize = 8 * 1024;
/// What the model is told when it drafts a commit message.
pub(crate) const DRAFT_INSTRUCTIONS: &str = "You write git commit messages. Reply with the message only: a subject line of at most 72 characters in the imperative mood, in the conventional-commit style (for example `feat(ui): add the changes panel` or `fix: keep the footer in sync`) when it fits, then, only if the change needs explaining, a blank line and a short body wrapped at 72 characters. No code fences, no quotes, no trailers.";
/// Patch text read from git before the rest is dropped.
const MAX_GIT_PATCH_READ: usize = 2 * MAX_PATCH_BYTES;
/// Of git's output for a failed command, the part an error shows.
const MAX_ERROR_CHARS: usize = 2_000;
/// Prompts of the session a draft request quotes, and how much of each.
const MAX_DRAFT_PROMPT_CHARS: usize = 300;

/// What a diff compares with `HEAD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffScope {
    /// The working tree, staged and unstaged, and the untracked files.
    WorkingTree,
    /// The index: what a commit would take now.
    Staged,
}

/// The files a diff found, and whether any were left out or cut.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RepoDiff {
    pub(crate) files: Vec<DiffFile>,
    pub(crate) truncated: bool,
}

/// One change `git status` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StatusEntry {
    /// The index column (`X`): `' '` when nothing is staged, `?` when untracked.
    pub(crate) index: char,
    /// The working-tree column (`Y`).
    pub(crate) worktree: char,
    pub(crate) path: String,
}

impl StatusEntry {
    fn is_staged(&self) -> bool {
        !matches!(self.index, ' ' | '?' | '!')
    }

    fn is_conflicted(&self) -> bool {
        self.index == 'U'
            || self.worktree == 'U'
            || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
    }
}

/// What a commit takes: what is staged, or (with `stage_all`) every change except the
/// files that can hold credentials, which are left out and named.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommitPlan {
    pub(crate) stage_all: bool,
    /// Changes left out because they can hold credentials.
    pub(crate) left_out: Vec<String>,
    /// With `stage_all`: the changes to stage.
    to_stage: Vec<String>,
}

impl CommitPlan {
    /// Nothing would be committed.
    pub(crate) fn is_empty(&self) -> bool {
        self.stage_all && self.to_stage.is_empty()
    }
}

/// A commit just made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Committed {
    /// The abbreviated hash.
    pub(crate) hash: String,
    pub(crate) branch: Option<String>,
    pub(crate) subject: String,
    pub(crate) files: usize,
}

impl Committed {
    /// `Committed abc1234 on main: Add the footer (3 files)`, and what was left out.
    pub(crate) fn notice(&self, left_out: &[String]) -> String {
        let place = self.branch.as_deref().map_or_else(
            || " (detached HEAD)".to_string(),
            |branch| format!(" on {branch}"),
        );
        let mut notice = format!(
            "Committed {}{place}: {} ({} file{})",
            self.hash,
            self.subject,
            self.files,
            plural(self.files)
        );
        if !left_out.is_empty() {
            notice.push('\n');
            notice.push_str(&left_out_notice(left_out));
        }
        notice
    }
}

/// Names the changes a commit leaves out because they can hold credentials.
pub(crate) fn left_out_notice(left_out: &[String]) -> String {
    format!(
        "Left out because they can hold credentials (stage them yourself to commit them): {}",
        left_out.join(", ")
    )
}

/// What `/commit` says when there is nothing to commit.
pub(crate) fn nothing_to_commit(left_out: &[String]) -> String {
    if left_out.is_empty() {
        "Nothing to commit.".to_string()
    } else {
        format!("Nothing to commit. {}", left_out_notice(left_out))
    }
}

/// A git repository, by its top-level folder.
#[derive(Debug, Clone)]
pub(crate) struct Repo {
    top: PathBuf,
}

impl Repo {
    /// The repository `cwd` is in; `None` outside one, or without git. An error when
    /// git must not run there: an untrusted workspace whose repository defines filter
    /// drivers.
    pub(crate) fn discover(cwd: &Path) -> Result<Option<Self>, String> {
        let mut command = runtime::workspace_git_command(cwd).ok_or_else(untrusted)?;
        let Ok(output) = command
            .args(["rev-parse", "--show-toplevel"])
            .stdin(Stdio::null())
            .output()
        else {
            return Ok(None);
        };
        let top = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((output.status.success() && !top.is_empty()).then(|| Self {
            top: PathBuf::from(top),
        }))
    }

    pub(crate) fn top(&self) -> &Path {
        &self.top
    }

    /// The checked-out branch; `None` on a detached `HEAD`.
    pub(crate) fn branch(&self) -> Option<String> {
        let output = self.run(&["symbolic-ref", "--short", "-q", "HEAD"]).ok()?;
        let branch = String::from_utf8_lossy(&output).trim().to_string();
        (!branch.is_empty()).then_some(branch)
    }

    /// The heading of a `/diff` of `path` (all changes when `None`).
    pub(crate) fn diff_title(&self, path: Option<&str>) -> String {
        let what = path.map_or_else(
            || "Changes".to_string(),
            |path| format!("Changes to {path}"),
        );
        let against = if self.has_commits() {
            "against HEAD"
        } else {
            "before the first commit"
        };
        match self.branch() {
            Some(branch) => format!("{what} {against} on {branch}"),
            None => format!("{what} {against}"),
        }
    }

    /// `path` as the user gave it (relative to `cwd`), as a pathspec from the top level.
    pub(crate) fn pathspec(&self, cwd: &Path, path: &str) -> Result<String, String> {
        let top = resolve(&self.top);
        let full = resolve(&cwd.join(path.trim()));
        let relative = full
            .strip_prefix(&top)
            .map_err(|_| format!("{path} is outside the repository at {}", self.top.display()))?;
        let spec = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        Ok(if spec.is_empty() {
            ".".to_string()
        } else {
            spec
        })
    }

    /// The changes of `scope` (limited to `pathspec`), each with its line counts and
    /// hunks. The patch text git writes is read up to twice [`MAX_PATCH_BYTES`]; files
    /// past that have no hunks, and the diff says it was cut.
    pub(crate) fn diff(
        &self,
        scope: DiffScope,
        pathspec: Option<&str>,
    ) -> Result<RepoDiff, String> {
        let base = self.base()?;
        let args = |extra: &[&'static str]| {
            let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "--no-color", "-M"];
            if scope == DiffScope::Staged {
                args.push("--cached");
            }
            args.extend(extra);
            args.push(base.as_str());
            args.push("--");
            args.extend(pathspec);
            args
        };
        let statuses = parse_name_status(&self.run(&args(&["--name-status", "-z"]))?);
        let counts = parse_numstat(&self.run(&args(&["--numstat", "-z"]))?);
        let (patch, cut) = self.run_capped(&args(&["--patch"]), MAX_GIT_PATCH_READ)?;
        let blocks = split_patch(&String::from_utf8_lossy(&patch), cut);
        let mut files = statuses
            .into_iter()
            .enumerate()
            .map(|(at, (status, path))| {
                let (added, removed, binary) = counts.get(at).copied().unwrap_or_default();
                let patch = blocks
                    .get(at)
                    .filter(|block| block.names(&path))
                    .map(|block| block.hunks.clone())
                    .unwrap_or_default();
                DiffFile {
                    path,
                    status: status.to_string(),
                    added,
                    removed,
                    patch,
                    binary,
                }
            })
            .collect::<Vec<_>>();
        if scope == DiffScope::WorkingTree {
            files.extend(self.untracked(pathspec)?);
            files.sort_by(|left, right| left.path.cmp(&right.path));
        }
        Ok(RepoDiff {
            files,
            truncated: cut,
        })
    }

    /// Every change, as `git status` lists it: untracked files one by one, renames as a
    /// removal and an addition.
    pub(crate) fn status(&self) -> Result<Vec<StatusEntry>, String> {
        let output = self.run(&[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])?;
        Ok(parse_status(&output))
    }

    /// What a commit would take now. An error while a merge conflict is unresolved.
    pub(crate) fn plan_commit(&self) -> Result<CommitPlan, String> {
        let entries = self.status()?;
        if let Some(conflict) = entries.iter().find(|entry| entry.is_conflicted()) {
            return Err(format!(
                "Resolve the merge conflict in {} first.",
                conflict.path
            ));
        }
        if entries.iter().any(StatusEntry::is_staged) {
            return Ok(CommitPlan::default());
        }
        let (left_out, to_stage) = entries
            .into_iter()
            .map(|entry| entry.path)
            .partition(|path| sensitive_path_reason(path).is_some());
        Ok(CommitPlan {
            stage_all: true,
            left_out,
            to_stage,
        })
    }

    /// Stages what `plan` says (`git add -A`, which respects `.gitignore`, minus the
    /// files left out), then commits with `message` as given (`git commit -F -`), hooks
    /// and all. A failing hook is an error with its output.
    pub(crate) fn commit(&self, plan: &CommitPlan, message: &str) -> Result<Committed, String> {
        if plan.is_empty() {
            return Err(nothing_to_commit(&plan.left_out));
        }
        if plan.stage_all && plan.left_out.is_empty() {
            self.run(&["add", "-A"])?;
        } else if plan.stage_all {
            let paths = plan.to_stage.join("\0");
            let args = ["add", "-A", "--pathspec-from-file=-", "--pathspec-file-nul"];
            let output = self.run_with_input(&args, paths.as_bytes())?;
            if !output.status.success() {
                return Err(git_failure(&args, &output));
            }
        }
        let output = self.run_with_input(&["commit", "-F", "-"], message.as_bytes())?;
        if !output.status.success() {
            return Err(git_failure(&["commit"], &output));
        }
        let hash = String::from_utf8_lossy(&self.run(&["rev-parse", "--short", "HEAD"])?)
            .trim()
            .to_string();
        let files = self.run(&[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "-r",
            "-z",
            "HEAD",
        ])?;
        Ok(Committed {
            hash,
            branch: self.branch(),
            subject: message
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string(),
            files: split_nul(&files).count(),
        })
    }

    fn has_commits(&self) -> bool {
        self.run(&["rev-parse", "--verify", "-q", "HEAD"]).is_ok()
    }

    /// What the working tree is compared with: `HEAD`, or the empty tree before the
    /// first commit.
    fn base(&self) -> Result<String, String> {
        if self.has_commits() {
            return Ok("HEAD".to_string());
        }
        let args = ["hash-object", "-t", "tree", "--stdin"];
        let output = self.run_with_input(&args, b"")?;
        if !output.status.success() {
            return Err(git_failure(&args, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Untracked files that are not ignored, as created files.
    fn untracked(&self, pathspec: Option<&str>) -> Result<Vec<DiffFile>, String> {
        let mut args = vec!["ls-files", "--others", "--exclude-standard", "-z", "--"];
        args.extend(pathspec);
        let listed = self.run(&args)?;
        Ok(split_nul(&listed)
            .map(|path| self.untracked_file(path))
            .collect())
    }

    fn untracked_file(&self, path: String) -> DiffFile {
        let full = self.top.join(&path);
        let mut file = DiffFile {
            path,
            status: "created".to_string(),
            added: 0,
            removed: 0,
            patch: String::new(),
            binary: false,
        };
        match fs::metadata(&full) {
            Ok(metadata) if metadata.len() <= MAX_TEXT_FILE_BYTES => {
                if let Ok(bytes) = fs::read(&full) {
                    if is_binary(&bytes) {
                        file.binary = true;
                    } else {
                        let diff = diff_lines("", &String::from_utf8_lossy(&bytes));
                        file.added = diff.added;
                        file.patch = diff.patch;
                    }
                }
            }
            Ok(_) => match count_file_lines(&full) {
                Ok(Some(lines)) => file.added = lines,
                Ok(None) => file.binary = true,
                Err(_) => {}
            },
            Err(_) => {}
        }
        file
    }

    fn command(&self) -> Result<Command, String> {
        let mut command = runtime::workspace_git_command(&self.top).ok_or_else(untrusted)?;
        command
            .args(["-c", "core.quotepath=false", "--literal-pathspecs"])
            .stdin(Stdio::null());
        Ok(command)
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        let output = self
            .command()?
            .args(args)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(git_failure(args, &output))
        }
    }

    /// Runs git with `input` on its stdin, written from a thread of its own so a large
    /// input cannot stall against the output.
    fn run_with_input(&self, args: &[&str], input: &[u8]) -> Result<Output, String> {
        let mut child = self
            .command()?
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run git: {error}"))?;
        let stdin = child.stdin.take();
        let input = input.to_vec();
        let writer = std::thread::spawn(move || {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(&input);
            }
        });
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not run git: {error}"));
        let _ = writer.join();
        output
    }

    /// Reads at most `cap` bytes of git's output; when there is more, git is stopped and
    /// the second value is `true`.
    fn run_capped(&self, args: &[&str], cap: usize) -> Result<(Vec<u8>, bool), String> {
        let mut child = self
            .command()?
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not run git: {error}"))?;
        let mut output = Vec::new();
        let read = child.stdout.take().map(|stdout| {
            stdout
                .take(u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1))
                .read_to_end(&mut output)
        });
        let cut = output.len() > cap;
        if cut {
            output.truncate(cap);
            let _ = child.kill();
        }
        let status = child
            .wait()
            .map_err(|error| format!("could not run git: {error}"))?;
        if let Some(Err(error)) = read {
            return Err(format!("could not read git's output: {error}"));
        }
        if !cut && !status.success() {
            return Err(format!("git {} failed", args.join(" ")));
        }
        Ok((output, cut))
    }
}

fn untrusted() -> String {
    "This workspace is not trusted and its git configuration runs programs, so Neuron does not run git here. Trust the workspace first.".to_string()
}

/// `git <args>` failed: its trimmed output, stdout and stderr together (a hook may
/// write to either).
fn git_failure(args: &[&str], output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(stderr.trim());
    }
    let name = args.first().copied().unwrap_or("git");
    if text.is_empty() {
        return format!("git {name} failed ({})", output.status);
    }
    if text.chars().count() > MAX_ERROR_CHARS {
        text = text.chars().take(MAX_ERROR_CHARS).collect::<String>() + "…";
    }
    format!("git {name} failed:\n{text}")
}

/// A path with `.` and `..` worked out, through the deepest folder on the way that
/// exists (so symlinks and Windows path forms compare equal), without needing the rest
/// to exist.
fn resolve(path: &Path) -> PathBuf {
    let mut lexical = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                lexical.pop();
            }
            Component::CurDir => {}
            other => lexical.push(other),
        }
    }
    let mut existing = lexical.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(canonical) = fs::canonicalize(existing) {
            return rest
                .iter()
                .rev()
                .fold(canonical, |path, part| path.join(part));
        }
        let (Some(parent), Some(name)) = (existing.parent(), existing.file_name()) else {
            return lexical;
        };
        rest.push(name.to_os_string());
        existing = parent;
    }
}

fn split_nul(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
}

/// `git diff --name-status -z`: each change's status and path (the new one for a
/// rename or copy).
fn parse_name_status(bytes: &[u8]) -> Vec<(&'static str, String)> {
    let mut fields = split_nul(bytes);
    let mut entries = Vec::new();
    while let Some(code) = fields.next() {
        let (status, paths) = match code.chars().next() {
            Some('A') => ("created", 1),
            Some('D') => ("deleted", 1),
            Some('R') => ("renamed", 2),
            Some('C') => ("created", 2),
            _ => ("modified", 1),
        };
        let mut path = None;
        for _ in 0..paths {
            path = fields.next();
        }
        if let Some(path) = path {
            entries.push((status, path));
        }
    }
    entries
}

/// `git diff --numstat -z`: each change's lines added and removed, and whether git
/// calls it binary (`-` counts). A rename's entry is followed by its two paths.
fn parse_numstat(bytes: &[u8]) -> Vec<(u64, u64, bool)> {
    let mut fields = bytes.split(|byte| *byte == 0);
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            continue;
        }
        let text = String::from_utf8_lossy(field);
        let mut parts = text.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if path.is_empty() {
            // A rename: the old and the new path follow.
            fields.next();
            fields.next();
        }
        let binary = added == "-" && removed == "-";
        entries.push((
            added.parse().unwrap_or(0),
            removed.parse().unwrap_or(0),
            binary,
        ));
    }
    entries
}

/// One file of `git diff --patch`: its header line and its hunks.
struct PatchBlock {
    header: String,
    hunks: String,
}

impl PatchBlock {
    /// Whether this block is the one of `path` (git quotes unusual names in headers;
    /// such a file just shows no hunks).
    fn names(&self, path: &str) -> bool {
        self.header.ends_with(&format!(" b/{path}"))
    }
}

/// The blocks of a patch, each from its `diff --git` line; the hunks start at the
/// first `@@`. When the text was `cut`, its last block is incomplete and left out.
fn split_patch(text: &str, cut: bool) -> Vec<PatchBlock> {
    let mut blocks: Vec<PatchBlock> = Vec::new();
    let mut in_hunks = false;
    for line in text.split_inclusive('\n') {
        if let Some(header) = line.strip_prefix("diff --git ") {
            blocks.push(PatchBlock {
                header: format!("diff --git {}", header.trim_end()),
                hunks: String::new(),
            });
            in_hunks = false;
            continue;
        }
        let Some(block) = blocks.last_mut() else {
            continue;
        };
        in_hunks |= line.starts_with("@@");
        if in_hunks {
            block.hunks.push_str(line);
        }
    }
    if cut {
        blocks.pop();
    }
    blocks
}

/// `git status --porcelain=v1 -z --no-renames`.
fn parse_status(bytes: &[u8]) -> Vec<StatusEntry> {
    split_nul(bytes)
        .filter_map(|record| {
            let mut chars = record.chars();
            let index = chars.next()?;
            let worktree = chars.next()?;
            let path = record.get(3..)?.to_string();
            (!path.is_empty()).then_some(StatusEntry {
                index,
                worktree,
                path,
            })
        })
        .collect()
}

/// Lines of a large file, read in pieces; `None` when it looks binary.
fn count_file_lines(path: &Path) -> io::Result<Option<u64>> {
    let mut reader = BufReader::new(File::open(path)?);
    if is_binary(reader.fill_buf()?) {
        return Ok(None);
    }
    let mut lines = 0_u64;
    let mut last = b'\n';
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            break;
        }
        lines += u64::try_from(line_breaks(buffer)).unwrap_or(u64::MAX);
        last = buffer[buffer.len() - 1];
        let used = buffer.len();
        reader.consume(used);
    }
    Ok(Some(lines + u64::from(last != b'\n')))
}

/// `diff` as a diff event carries it: at most [`MAX_DIFF_FILES`] files and
/// [`MAX_PATCH_BYTES`] of hunks. A file's hunks are kept whole while they fit; `truncated`
/// says when anything was left out.
pub(crate) fn cap_diff(diff: RepoDiff) -> RepoDiff {
    let mut truncated = diff.truncated || diff.files.len() > MAX_DIFF_FILES;
    let mut budget = MAX_PATCH_BYTES;
    let files = diff
        .files
        .into_iter()
        .take(MAX_DIFF_FILES)
        .map(|mut file| {
            if file.patch.len() > budget {
                file.patch = whole_hunks(&file.patch, budget).to_string();
                truncated = true;
            }
            budget -= file.patch.len();
            file
        })
        .collect();
    RepoDiff { files, truncated }
}

/// The leading hunks of `patch` that fit in `budget` bytes.
fn whole_hunks(patch: &str, budget: usize) -> &str {
    let mut end = 0;
    for (at, _) in patch.match_indices("\n@@") {
        if at + 1 > budget {
            break;
        }
        end = at + 1;
    }
    &patch[..end]
}

/// A changed file without its hunks.
pub(crate) fn changed_file(file: &DiffFile) -> ChangedFile {
    ChangedFile {
        path: file.path.clone(),
        status: file.status.clone(),
        added: file.added,
        removed: file.removed,
    }
}

/// A diff as one plain-text notice: the heading, the totals, then each file with its
/// hunks.
pub(crate) fn render_diff_text(title: &str, diff: &RepoDiff) -> String {
    if diff.files.is_empty() {
        return format!("{title}: no changes.");
    }
    let added = diff.files.iter().map(|file| file.added).sum::<u64>();
    let removed = diff.files.iter().map(|file| file.removed).sum::<u64>();
    let count = diff.files.len();
    let mut text = format!(
        "{title}\n{count} file{}, +{added} -{removed}\n",
        plural(count)
    );
    for file in &diff.files {
        text.push_str(&format!(
            "\n{} {} (+{} -{})\n",
            file.status, file.path, file.added, file.removed
        ));
        if file.binary {
            text.push_str("(binary)\n");
        } else {
            text.push_str(&file.patch);
        }
    }
    if diff.truncated {
        text.push_str("\n… more was left out to keep this short.\n");
    }
    text
}

/// A commit message from the file names alone: `Add site/style.css`,
/// `Update 3 files in src/ui`.
pub(crate) fn heuristic_message(files: &[ChangedFile]) -> String {
    let verb = |status: &str| match status {
        "created" => "Add",
        "deleted" => "Remove",
        "renamed" => "Rename",
        _ => "Update",
    };
    let subject = match files {
        [] => "Update files".to_string(),
        [file] => {
            let verb = verb(&file.status);
            let full = format!("{verb} {}", file.path);
            if full.chars().count() <= MAX_SUBJECT_CHARS {
                full
            } else {
                let name = file.path.rsplit('/').next().unwrap_or(&file.path);
                format!("{verb} {name}")
            }
        }
        _ => {
            let first = verb(&files[0].status);
            let verb = if files.iter().all(|file| verb(&file.status) == first)
                && matches!(first, "Add" | "Remove")
            {
                first
            } else {
                "Update"
            };
            let subject = format!("{verb} {} files", files.len());
            match common_folder(files) {
                Some(folder) => {
                    let longer = format!("{subject} in {folder}");
                    if longer.chars().count() <= MAX_SUBJECT_CHARS {
                        longer
                    } else {
                        subject
                    }
                }
                None => subject,
            }
        }
    };
    cut_subject(&subject)
}

/// The deepest folder every file is in, if any.
fn common_folder(files: &[ChangedFile]) -> Option<String> {
    let folders = files
        .iter()
        .map(|file| {
            let mut parts = file.path.split('/').collect::<Vec<_>>();
            parts.pop();
            parts
        })
        .collect::<Vec<_>>();
    let first = folders.first()?;
    let depth = (0..first.len())
        .take_while(|at| folders.iter().all(|parts| parts.get(*at) == first.get(*at)))
        .count();
    (depth > 0).then(|| first[..depth].join("/"))
}

/// A subject of at most [`MAX_SUBJECT_CHARS`], cut at a word when it is longer.
fn cut_subject(subject: &str) -> String {
    let subject = subject.trim();
    if subject.chars().count() <= MAX_SUBJECT_CHARS {
        return subject.to_string();
    }
    let head = subject.chars().take(MAX_SUBJECT_CHARS).collect::<String>();
    match head.rfind(' ') {
        Some(at) if at > MAX_SUBJECT_CHARS / 2 => head[..at].trim_end().to_string(),
        _ => head,
    }
}

/// The request for a drafted commit message: what the user asked for in the session,
/// the files, and their diff cut to [`MAX_DRAFT_DIFF_BYTES`]. Files that can hold
/// credentials are named, never shown.
pub(crate) fn draft_request(files: &[DiffFile], prompts: &[String]) -> String {
    let mut request = String::from("Write the commit message for these changes.\n");
    if !prompts.is_empty() {
        request.push_str("\nWhat the user asked for in this session, most recent last:\n");
        for prompt in prompts {
            let line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
            let mut short = line
                .chars()
                .take(MAX_DRAFT_PROMPT_CHARS)
                .collect::<String>();
            if short.len() < line.len() {
                short.push('…');
            }
            request.push_str(&format!("- {short}\n"));
        }
    }
    request.push_str("\nFiles:\n");
    for file in files {
        request.push_str(&format!(
            "- {} {} (+{} -{})\n",
            file.status, file.path, file.added, file.removed
        ));
    }
    let mut diff = String::new();
    let mut left_out = false;
    for file in files {
        if file.binary || file.patch.is_empty() {
            continue;
        }
        if sensitive_path_reason(&file.path).is_some() {
            diff.push_str(&format!(
                "--- {0}\n(contents not shown: the file can hold credentials)\n",
                file.path
            ));
            continue;
        }
        let section = format!("--- {}\n{}", file.path, file.patch);
        let room = MAX_DRAFT_DIFF_BYTES.saturating_sub(diff.len());
        if section.len() <= room {
            diff.push_str(&section);
        } else {
            let mut end = room;
            while !section.is_char_boundary(end) {
                end -= 1;
            }
            diff.push_str(&section[..end]);
            left_out = true;
            break;
        }
    }
    if !diff.is_empty() {
        request.push_str("\nDiff");
        request.push_str(if left_out { " (cut short):\n" } else { ":\n" });
        request.push_str(&diff);
        if !diff.ends_with('\n') {
            request.push('\n');
        }
    }
    request
}

/// The model's draft as a commit message: no code fences, quotes or trailers, a subject
/// of at most [`MAX_SUBJECT_CHARS`], and a blank line before any body. `None` when no
/// subject is left.
pub(crate) fn clean_draft(reply: &str) -> Option<String> {
    let lines = reply
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim_start().starts_with("```"))
        .filter(|line| {
            let lower = line.trim_start().to_ascii_lowercase();
            !(lower.starts_with("co-authored-by:") || lower.starts_with("signed-off-by:"))
        })
        .collect::<Vec<_>>();
    let start = lines.iter().position(|line| !line.trim().is_empty())?;
    let subject = lines[start]
        .trim()
        .trim_start_matches("Subject:")
        .trim()
        .trim_matches(['"', '\'', '`'])
        .trim();
    if subject.is_empty() {
        return None;
    }
    let mut message = cut_subject(subject);
    let body = lines[start + 1..]
        .iter()
        .skip_while(|line| line.trim().is_empty())
        .copied()
        .collect::<Vec<_>>();
    let end = body
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(0, |at| at + 1);
    if end > 0 {
        message.push_str("\n\n");
        message.push_str(&body[..end].join("\n"));
    }
    Some(message)
}

/// The text of the session's last `count` prompts, oldest first.
pub(crate) fn recent_prompts(session: &runtime::Session, count: usize) -> Vec<String> {
    let mut prompts = session
        .messages
        .iter()
        .rev()
        .filter(|message| crate::status_ui::is_prompt(message))
        .filter_map(|message| {
            let text = message
                .blocks
                .iter()
                .filter_map(|block| match block {
                    runtime::ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        })
        .take(count)
        .collect::<Vec<_>>();
    prompts.reverse();
    prompts
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cap_diff, clean_draft, draft_request, heuristic_message, nothing_to_commit,
        parse_name_status, parse_numstat, render_diff_text, DiffScope, Repo, RepoDiff,
        MAX_DIFF_FILES, MAX_DRAFT_DIFF_BYTES, MAX_PATCH_BYTES,
    };
    use neuron_tui::protocol::{ChangedFile, DiffFile};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh repository with one commit of `tracked.txt` and `.gitignore`, removed when
    /// dropped.
    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "neuron-source-control-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("root");
            git(&["init", "--quiet", "-b", "main"], &root);
            git(&["config", "user.email", "tests@example.com"], &root);
            git(&["config", "user.name", "Neuron Tests"], &root);
            git(&["config", "commit.gpgsign", "false"], &root);
            fs::write(root.join("tracked.txt"), "one\ntwo\nthree\n").expect("tracked");
            fs::write(root.join(".gitignore"), "ignored.txt\n").expect("gitignore");
            git(&["add", "-A"], &root);
            git(&["commit", "-q", "-m", "init"], &root);
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, contents: &str) {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("parent")).expect("folder");
            fs::write(path, contents).expect("write");
        }

        fn repo(&self) -> Repo {
            Repo::discover(&self.0)
                .expect("git may run")
                .expect("a repository")
        }
    }

    impl Drop for TempRepo {
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
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn changed(path: &str, status: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            status: status.to_string(),
            added: 1,
            removed: 0,
        }
    }

    fn summary(diff: &RepoDiff) -> Vec<(String, String, u64, u64)> {
        diff.files
            .iter()
            .map(|file| {
                (
                    file.path.clone(),
                    file.status.clone(),
                    file.added,
                    file.removed,
                )
            })
            .collect()
    }

    #[test]
    fn the_working_tree_diff_has_staged_unstaged_and_untracked_changes() {
        let temp = TempRepo::new("working-tree");
        temp.write("tracked.txt", "one\n2\nthree\nfour\n");
        temp.write("src/new.rs", "fn main() {}\n");
        temp.write("ignored.txt", "never shown\n");
        temp.write("staged.txt", "staged\n");
        git(&["add", "staged.txt"], temp.path());
        fs::write(temp.path().join("logo.png"), b"\x89PNG\0\0\x01").expect("binary");
        let repo = temp.repo();

        let diff = repo.diff(DiffScope::WorkingTree, None).expect("diff");
        assert_eq!(
            summary(&diff),
            [
                ("logo.png".into(), "created".into(), 0, 0),
                ("src/new.rs".into(), "created".into(), 1, 0),
                ("staged.txt".into(), "created".into(), 1, 0),
                ("tracked.txt".into(), "modified".into(), 2, 1),
            ]
        );
        assert!(!diff.truncated);
        assert!(diff.files[0].binary && diff.files[0].patch.is_empty());
        assert_eq!(diff.files[1].patch, "@@ -0,0 +1 @@\n+fn main() {}\n");
        let tracked = &diff.files[3].patch;
        assert!(
            tracked.starts_with("@@ -1,3 +1,4 @@\n one\n-two\n+2\n"),
            "{tracked}"
        );
        assert_eq!(repo.diff_title(None), "Changes against HEAD on main");

        // Only what is staged; then one path.
        let staged = repo.diff(DiffScope::Staged, None).expect("staged diff");
        assert_eq!(
            summary(&staged),
            [("staged.txt".into(), "created".into(), 1, 0)]
        );
        let spec = repo
            .pathspec(&temp.path().join("src"), "new.rs")
            .expect("in the repository");
        assert_eq!(spec, "src/new.rs");
        let one = repo
            .diff(DiffScope::WorkingTree, Some(&spec))
            .expect("diff");
        assert_eq!(
            summary(&one),
            [("src/new.rs".into(), "created".into(), 1, 0)]
        );
        assert!(repo.pathspec(temp.path(), "../elsewhere").is_err());
    }

    #[test]
    fn renames_and_removals_keep_their_status() {
        let temp = TempRepo::new("renames");
        git(&["mv", "tracked.txt", "moved.txt"], temp.path());
        fs::remove_file(temp.path().join(".gitignore")).expect("remove");
        let diff = temp
            .repo()
            .diff(DiffScope::WorkingTree, None)
            .expect("diff");
        assert_eq!(
            summary(&diff),
            [
                (".gitignore".into(), "deleted".into(), 0, 1),
                ("moved.txt".into(), "renamed".into(), 0, 0),
            ]
        );
    }

    #[test]
    fn name_status_and_numstat_parse_renames_and_binary_files() {
        assert_eq!(
            parse_name_status(b"M\0a.rs\0R087\0old.rs\0new.rs\0A\0b.png\0"),
            [
                ("modified", "a.rs".to_string()),
                ("renamed", "new.rs".to_string()),
                ("created", "b.png".to_string()),
            ]
        );
        assert_eq!(
            parse_numstat(b"3\t1\ta.rs\x001\t1\t\0old.rs\0new.rs\0-\t-\tb.png\0"),
            [(3, 1, false), (1, 1, false), (0, 0, true)]
        );
    }

    #[test]
    fn a_diff_event_keeps_at_most_its_file_and_patch_limits() {
        let file = |at: usize, patch: String| DiffFile {
            path: format!("f{at:04}.txt"),
            status: "modified".into(),
            added: 1,
            removed: 1,
            patch,
            binary: false,
        };
        let many = RepoDiff {
            files: (0..MAX_DIFF_FILES + 5)
                .map(|at| file(at, "@@ -1 +1 @@\n-a\n+b\n".into()))
                .collect(),
            truncated: false,
        };
        let capped = cap_diff(many);
        assert_eq!(capped.files.len(), MAX_DIFF_FILES);
        assert!(capped.truncated);

        let hunk = format!("@@ -1 +1 @@\n-{}\n+b\n", "a".repeat(1000));
        let big = hunk.repeat(MAX_PATCH_BYTES / hunk.len() + 10);
        let capped = cap_diff(RepoDiff {
            files: vec![file(0, big), file(1, hunk.clone())],
            truncated: false,
        });
        assert!(capped.truncated);
        assert!(capped.files[0].patch.len() <= MAX_PATCH_BYTES);
        assert!(capped.files[0].patch.starts_with("@@") && capped.files[0].patch.ends_with("+b\n"));
        assert_eq!(capped.files[1].patch, "", "no room left");
        let small = cap_diff(RepoDiff {
            files: vec![file(0, hunk.clone())],
            truncated: false,
        });
        assert!(!small.truncated);
        assert_eq!(small.files[0].patch, hunk);
    }

    #[test]
    fn staging_leaves_out_credentials_and_keeps_what_is_staged() {
        let temp = TempRepo::new("staging");
        temp.write("app.js", "run()\n");
        temp.write(".env", "TOKEN=secret\n");
        temp.write("tracked.txt", "changed\n");
        let repo = temp.repo();

        let plan = repo.plan_commit().expect("plan");
        assert!(plan.stage_all);
        assert_eq!(plan.left_out, [".env"]);
        let committed = repo
            .commit(&plan, "Add the app\n\nRuns it once.\n")
            .expect("commit");
        assert_eq!(committed.subject, "Add the app");
        assert_eq!(committed.files, 2);
        assert_eq!(committed.branch.as_deref(), Some("main"));
        assert!(committed.notice(&plan.left_out).starts_with(&format!(
            "Committed {} on main: Add the app (2 files)\nLeft out",
            committed.hash
        )));
        assert_eq!(
            git(&["log", "-1", "--format=%B"], temp.path()),
            "Add the app\n\nRuns it once.\n\n"
        );
        assert_eq!(git(&["status", "--porcelain"], temp.path()), "?? .env\n");

        // Something staged: only that is committed.
        temp.write("a.txt", "a\n");
        temp.write("b.txt", "b\n");
        git(&["add", "a.txt"], temp.path());
        let plan = repo.plan_commit().expect("plan");
        assert!(!plan.stage_all && plan.left_out.is_empty());
        let committed = repo.commit(&plan, "Add a").expect("commit");
        assert_eq!(committed.files, 1);
        let left = git(&["status", "--porcelain"], temp.path());
        assert!(
            left.contains("?? b.txt") && left.contains("?? .env"),
            "{left}"
        );

        // Only credentials left: nothing to commit.
        fs::remove_file(temp.path().join("b.txt")).expect("remove");
        let plan = repo.plan_commit().expect("plan");
        assert!(plan.is_empty());
        assert!(repo.commit(&plan, "Nothing").is_err());
        assert!(nothing_to_commit(&plan.left_out).starts_with("Nothing to commit. Left out"));
    }

    #[test]
    fn a_failing_hook_is_an_error_with_its_output() {
        let temp = TempRepo::new("hook");
        let hook = temp.path().join(".git").join("hooks").join("pre-commit");
        fs::create_dir_all(hook.parent().expect("hooks")).expect("hooks folder");
        fs::write(
            &hook,
            "#!/bin/sh\necho 'lint failed: style.css' >&2\nexit 1\n",
        )
        .expect("hook");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("executable");
        }
        temp.write("style.css", "body {}\n");
        let repo = temp.repo();
        let plan = repo.plan_commit().expect("plan");
        let error = repo
            .commit(&plan, "Add style")
            .expect_err("the hook refuses");
        assert!(error.contains("lint failed: style.css"), "{error}");
        assert_eq!(
            git(&["rev-list", "--count", "HEAD"], temp.path()).trim(),
            "1"
        );
    }

    #[test]
    fn heuristic_messages_name_one_file_or_count_several() {
        assert_eq!(
            heuristic_message(&[changed("grocery-shop/style.css", "created")]),
            "Add grocery-shop/style.css"
        );
        assert_eq!(
            heuristic_message(&[changed("old.txt", "deleted")]),
            "Remove old.txt"
        );
        let long = format!("{}/deep/file.rs", "folder".repeat(15));
        assert_eq!(
            heuristic_message(&[changed(&long, "modified")]),
            "Update file.rs"
        );
        assert_eq!(
            heuristic_message(&[
                changed("src/ui/a.ts", "modified"),
                changed("src/ui/b.ts", "created"),
                changed("src/ui/c/d.ts", "modified"),
            ]),
            "Update 3 files in src/ui"
        );
        assert_eq!(
            heuristic_message(&[changed("a.txt", "created"), changed("b.txt", "created")]),
            "Add 2 files"
        );
    }

    #[test]
    fn a_draft_request_shows_names_of_credentials_never_their_contents() {
        let file = |path: &str, patch: &str| DiffFile {
            path: path.to_string(),
            status: "modified".into(),
            added: 1,
            removed: 1,
            patch: patch.to_string(),
            binary: false,
        };
        let request = draft_request(
            &[
                file(".env", "@@ -1 +1 @@\n-KEY=old\n+KEY=new-secret\n"),
                file(
                    "src/app.ts",
                    &format!("@@ -1 +1 @@\n-a\n+{}\n", "b".repeat(20_000)),
                ),
            ],
            &["make the header blue".to_string()],
        );
        assert!(request.contains("- make the header blue\n"), "{request}");
        assert!(request.contains("- modified .env (+1 -1)"), "{request}");
        assert!(!request.contains("new-secret"), "{request}");
        assert!(request.contains("Diff (cut short):"), "{request}");
        assert!(
            request.len() < MAX_DRAFT_DIFF_BYTES + 1_000,
            "{}",
            request.len()
        );
    }

    #[test]
    fn drafts_lose_fences_quotes_and_trailers_and_keep_a_short_subject() {
        assert_eq!(
            clean_draft("```\n\"feat(site): add the stylesheet\"\n\nSets the margin.\nCo-Authored-By: someone\n```\n")
                .as_deref(),
            Some("feat(site): add the stylesheet\n\nSets the margin.")
        );
        let long = format!("fix: {}", "word ".repeat(30));
        let cleaned = clean_draft(&long).expect("a subject");
        assert!(
            cleaned.chars().count() <= 72 && !cleaned.ends_with(' '),
            "{cleaned}"
        );
        assert_eq!(clean_draft("  \n```\n```"), None);
    }

    #[test]
    fn a_plain_text_diff_lists_each_file_with_its_hunks() {
        let diff = RepoDiff {
            files: vec![DiffFile {
                path: "a.txt".into(),
                status: "modified".into(),
                added: 1,
                removed: 1,
                patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
                binary: false,
            }],
            truncated: true,
        };
        let text = render_diff_text("Changes against HEAD on main", &diff);
        assert!(text.starts_with("Changes against HEAD on main\n1 file, +1 -1\n\nmodified a.txt (+1 -1)\n@@ -1 +1 @@\n-a\n+b\n"), "{text}");
        assert!(text.ends_with("left out to keep this short.\n"), "{text}");
        assert_eq!(
            render_diff_text("Changes", &RepoDiff::default()),
            "Changes: no changes."
        );
    }
}
