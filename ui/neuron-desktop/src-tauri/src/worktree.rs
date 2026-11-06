//! A chat's own copy of the project: a git worktree on a `neuron/<name>`
//! branch, kept under the app's data folder (not beside the repository), so
//! several chats can edit at once without touching each other or the folder
//! the user is working in.
//!
//! Every git call is `Command::new("git").args([...])`: no shell, no prompts.
//! Nothing here takes a path from the window: the record of a worktree comes
//! from the registry, and removal only ever happens inside the worktree root
//! for worktrees git itself lists.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// Creating a worktree checks out every file; give a big repository time.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);
const SLUG_MAX: usize = 40;
const REMOVE_TRIES: usize = 3;
const REMOVE_WAIT: Duration = Duration::from_millis(300);

/// Names Windows will not make a folder of.
const RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// A worktree the app made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRecord {
    /// The repository's top folder.
    pub repo: PathBuf,
    /// Where this repository's worktrees live (`<app data>/wt/<hash>`).
    pub root: PathBuf,
    /// The worktree's top folder, inside `root`.
    pub path: PathBuf,
    /// The folder the chat was opened in, relative to the repository top.
    pub subdir: PathBuf,
    pub slug: String,
    /// `neuron/<slug>`.
    pub branch: String,
    /// What it was made from: a branch name, or a short commit id.
    pub base: String,
}

impl WorktreeRecord {
    /// The folder the chat's engine runs in.
    pub fn cwd(&self) -> PathBuf {
        let inside = self.path.join(&self.subdir);
        if inside.is_dir() {
            inside
        } else {
            self.path.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorktreeStatus {
    pub branch: String,
    pub path: String,
    /// Changed or new files in the worktree.
    pub dirty: usize,
    /// Commits on the branch that the main folder's HEAD does not have.
    pub unmerged: usize,
}

/// `Fix Login!` becomes `fix-login`: lowercase letters, digits and single
/// dashes, 1 to 40 characters. Anything that looks like a path, and the names
/// Windows reserves, are refused rather than cleaned.
pub fn slugify(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.contains(['/', '\\']) || name.contains("..") || name.chars().any(char::is_control) {
        return Err("A name cannot contain slashes or dots in a row.".into());
    }
    let mut slug = String::new();
    for ch in name.trim().chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-');
    let slug: String = slug.chars().take(SLUG_MAX).collect();
    let slug = slug.trim_end_matches('-').to_string();
    if slug.is_empty() {
        return Err("Give the copy a name with letters or numbers.".into());
    }
    if RESERVED.contains(&slug.as_str()) {
        return Err(format!("\"{slug}\" cannot be used as a name."));
    }
    Ok(slug)
}

/// FNV-1a: small, and the same on every platform and Rust version.
fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// Without Windows' `\\?\` prefix, which git does not read.
pub fn plain(path: &Path) -> PathBuf {
    let shown = path.to_string_lossy();
    if let Some(rest) = shown.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = shown.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// `<app data>/wt/<8 hex of the repository's path>`: short paths for Windows.
pub fn worktree_root(app_data: &Path, repo: &Path) -> PathBuf {
    let key = plain(repo).to_string_lossy().to_lowercase();
    app_data.join("wt").join(format!("{:08x}", fnv(&key)))
}

struct Run {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn git(dir: &Path, args: &[&str]) -> Result<Run, String> {
    let mut command = Command::new("git");
    command
        .args(["-c", "core.longpaths=true", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not run git: {error}"))?;
    let mut out = child.stdout.take().ok_or("git has no stdout")?;
    let mut err = child.stderr.take().ok_or("git has no stderr")?;
    let out = thread::spawn(move || {
        let mut text = String::new();
        let _ = out.read_to_string(&mut text);
        text
    });
    let err = thread::spawn(move || {
        let mut text = String::new();
        let _ = err.read_to_string(&mut text);
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
            return Err("git took too long.".into());
        }
        thread::sleep(Duration::from_millis(15));
    };
    Ok(Run {
        ok: status.success(),
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// The branch checked out in `dir`; `None` outside a repository or when detached.
pub fn current_branch(dir: &Path) -> Option<String> {
    let run = git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let name = first_line(&run.stdout);
    (run.ok && !name.is_empty() && name != "HEAD").then_some(name)
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok_and(|run| run.ok)
}

/// A name for a copy the user did not name: `chat-` and six hex digits of the clock.
fn default_slug() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.subsec_nanos() ^ (elapsed.as_secs() as u32)
        });
    format!("chat-{:06x}", nanos & 0x00ff_ffff)
}

/// A worktree on a new `neuron/<name>` branch, made from the HEAD of the
/// repository that holds `project`.
pub fn create(
    app_data: &Path,
    project: &Path,
    name: Option<&str>,
) -> Result<WorktreeRecord, String> {
    let project =
        fs::canonicalize(project).map_err(|error| format!("{}: {error}", project.display()))?;
    let top = git(&project, &["rev-parse", "--show-toplevel"])?;
    if !top.ok {
        return Err(
            "This folder is not in a git repository, so it cannot get its own copy.".into(),
        );
    }
    let top = fs::canonicalize(first_line(&top.stdout))
        .map_err(|error| format!("could not read the repository folder: {error}"))?;
    if !git(&top, &["rev-parse", "--verify", "--quiet", "HEAD"])?.ok {
        return Err("This repository has no commits yet, so it cannot be copied.".into());
    }
    let subdir = project
        .strip_prefix(&top)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let base = current_branch(&top).unwrap_or_else(|| {
        git(&top, &["rev-parse", "--short", "HEAD"])
            .map(|run| first_line(&run.stdout))
            .unwrap_or_default()
    });

    let wanted = match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => slugify(name)?,
        None => default_slug(),
    };
    let root = worktree_root(app_data, &top);
    fs::create_dir_all(&root)
        .map_err(|error| format!("could not make {}: {error}", root.display()))?;
    let root = fs::canonicalize(&root).map_err(|error| error.to_string())?;

    let mut slug = wanted.clone();
    for count in 2..100 {
        if !root.join(&slug).exists() && !branch_exists(&top, &format!("neuron/{slug}")) {
            break;
        }
        let suffix = format!("-{count}");
        let keep = SLUG_MAX - suffix.len();
        let base: String = wanted.chars().take(keep).collect();
        slug = format!("{}{suffix}", base.trim_end_matches('-'));
    }
    let branch = format!("neuron/{slug}");
    let path = root.join(&slug);
    let made = git(
        &top,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            &plain(&path).to_string_lossy(),
            "HEAD",
        ],
    )?;
    if !made.ok {
        return Err(format!(
            "Could not make the copy: {}",
            first_line(&made.stderr)
        ));
    }
    Ok(WorktreeRecord {
        repo: top,
        root,
        path,
        subdir,
        slug,
        branch,
        base,
    })
}

pub fn status(record: &WorktreeRecord) -> Result<WorktreeStatus, String> {
    let changes = git(&record.path, &["status", "--porcelain"])?;
    if !changes.ok {
        return Err(format!(
            "Could not read the copy: {}",
            first_line(&changes.stderr)
        ));
    }
    let dirty = changes
        .stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Ok(WorktreeStatus {
        branch: record.branch.clone(),
        path: plain(&record.path).to_string_lossy().into_owned(),
        dirty,
        unmerged: unmerged(record)?,
    })
}

fn unmerged(record: &WorktreeRecord) -> Result<usize, String> {
    let range = format!("HEAD..{}", record.branch);
    let count = git(&record.repo, &["rev-list", "--count", &range])?;
    if !count.ok {
        return Err(format!(
            "Could not read the branch: {}",
            first_line(&count.stderr)
        ));
    }
    Ok(first_line(&count.stdout).parse().unwrap_or(0))
}

/// How a path reads for comparison: forward slashes, no trailing one, and
/// lowercase where the file system ignores case.
fn comparable(path: &Path) -> String {
    let text = plain(path).to_string_lossy().replace('\\', "/");
    let text = text.trim_end_matches('/').to_string();
    if cfg!(windows) || cfg!(target_os = "macos") {
        text.to_lowercase()
    } else {
        text
    }
}

/// Everything that must hold before anything is removed: the record is one the
/// app made (a `neuron/<slug>` branch, a path inside the worktree root after
/// canonicalizing, listed by git), and nothing would be lost without the
/// user's say-so. Returns the canonical path.
pub fn check_discard(
    record: &WorktreeRecord,
    force: bool,
    delete_unmerged: bool,
) -> Result<PathBuf, String> {
    if slugify(&record.slug).as_deref() != Ok(record.slug.as_str())
        || record.branch != format!("neuron/{}", record.slug)
    {
        return Err("This is not a copy Neuron made.".into());
    }
    let root = fs::canonicalize(&record.root).map_err(|error| error.to_string())?;
    let path = fs::canonicalize(&record.path)
        .map_err(|error| format!("The copy is not there any more: {error}"))?;
    if path == root || !path.starts_with(&root) {
        return Err("This is not a copy Neuron made.".into());
    }
    let listed = git(&record.repo, &["worktree", "list", "--porcelain"])?;
    let wanted = comparable(&path);
    let known = listed
        .stdout
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|line| comparable(Path::new(line)) == wanted);
    if !listed.ok || !known {
        return Err("Git does not list this copy.".into());
    }
    let count = unmerged(record)?;
    if count > 0 && !delete_unmerged {
        return Err(format!("unmerged:{count}"));
    }
    if !force {
        let changes = git(&path, &["status", "--porcelain"])?;
        let dirty = changes
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        if dirty > 0 {
            return Err(format!("dirty:{dirty}"));
        }
    }
    Ok(path)
}

/// Removes the worktree and its branch. The caller has stopped the chat's
/// engine and closed its terminals. `force` is for a copy with changes the
/// user has been shown; `delete_unmerged` for a branch with commits the main
/// folder lacks.
pub fn discard(record: &WorktreeRecord, force: bool, delete_unmerged: bool) -> Result<(), String> {
    let path = check_discard(record, force, delete_unmerged)?;
    let shown = plain(&path).to_string_lossy().into_owned();
    let mut last = String::new();
    for attempt in 0..REMOVE_TRIES {
        let mut args = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.extend(["--", &shown]);
        let run = git(&record.repo, &args)?;
        if run.ok {
            last.clear();
            break;
        }
        last = first_line(&run.stderr);
        // Windows keeps a folder locked a moment after its processes end.
        if attempt + 1 < REMOVE_TRIES {
            thread::sleep(REMOVE_WAIT);
        }
    }
    if !last.is_empty() {
        return Err(format!("Could not remove the copy: {last}"));
    }
    let _ = git(&record.repo, &["worktree", "prune"]);
    let flag = if delete_unmerged { "-D" } else { "-d" };
    let deleted = git(&record.repo, &["branch", flag, &record.branch])?;
    if !deleted.ok {
        return Err(format!(
            "The copy is gone, but its branch stayed: {}",
            first_line(&deleted.stderr)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_plain_and_short() {
        assert_eq!(slugify("Fix Login!").unwrap(), "fix-login");
        assert_eq!(slugify("  a   b  ").unwrap(), "a-b");
        assert_eq!(slugify("Ünï 2").unwrap(), "n-2");
        let long = slugify(&"a".repeat(100)).unwrap();
        assert_eq!(long.len(), 40);
        let dashed = slugify(&format!("{}-b", "a".repeat(39))).unwrap();
        assert!(!dashed.ends_with('-') && dashed.len() <= 40);
    }

    #[test]
    fn unusable_names_are_refused() {
        for name in [
            "", "   ", "---", "!!!", "con", "NUL", "com1", "../x", "a/b", "a\\b", "x\n",
        ] {
            assert!(slugify(name).is_err(), "{name:?} should be refused");
        }
    }

    #[test]
    fn the_worktree_root_is_short_and_stable() {
        let a = worktree_root(Path::new("/data"), Path::new("/work/app"));
        assert_eq!(a, worktree_root(Path::new("/data"), Path::new("/work/app")));
        assert_ne!(
            a,
            worktree_root(Path::new("/data"), Path::new("/work/other"))
        );
        assert_eq!(a.parent().unwrap(), Path::new("/data").join("wt"));
        let name = a.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name.len(), 8);
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // Integration: a real repository in a temporary folder.

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("neuron-wt-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("repo")).unwrap();
            let sandbox = Self {
                dir: fs::canonicalize(&dir).unwrap(),
            };
            sandbox.git(&sandbox.repo(), &["init", "-q", "-b", "main"]);
            fs::write(sandbox.repo().join("a.txt"), "one\n").unwrap();
            sandbox.commit(&sandbox.repo(), "first");
            sandbox
        }
        fn repo(&self) -> PathBuf {
            self.dir.join("repo")
        }
        fn data(&self) -> PathBuf {
            self.dir.join("data")
        }
        fn git(&self, dir: &Path, args: &[&str]) {
            let run = git(dir, args).unwrap();
            assert!(run.ok, "git {args:?}: {}", run.stderr);
        }
        fn commit(&self, dir: &Path, message: &str) {
            self.git(dir, &["add", "-A"]);
            self.git(
                dir,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-q",
                    "-m",
                    message,
                ],
            );
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            // Worktrees hold .git files; a plain delete is enough for a temp folder.
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn creates_a_worktree_on_its_own_branch() {
        let sandbox = Sandbox::new("create");
        let record = create(&sandbox.data(), &sandbox.repo(), Some("Fix Login!")).unwrap();
        assert_eq!(record.slug, "fix-login");
        assert_eq!(record.branch, "neuron/fix-login");
        assert_eq!(record.base, "main");
        assert!(record.path.join("a.txt").is_file());
        assert!(record.path.starts_with(&record.root));
        assert!(record
            .root
            .starts_with(fs::canonicalize(sandbox.data()).unwrap()));
        assert_eq!(
            current_branch(&record.path).as_deref(),
            Some("neuron/fix-login")
        );
        // The same name again gets a number.
        let second = create(&sandbox.data(), &sandbox.repo(), Some("Fix Login!")).unwrap();
        assert_eq!(second.slug, "fix-login-2");
        assert_eq!(second.branch, "neuron/fix-login-2");
    }

    #[test]
    fn a_folder_outside_git_or_without_commits_is_refused() {
        let sandbox = Sandbox::new("refuse");
        let plain_dir = sandbox.dir.join("plain");
        fs::create_dir_all(&plain_dir).unwrap();
        // A temp folder sits in no repository, unless the machine keeps one at its root.
        if git(&plain_dir, &["rev-parse", "--show-toplevel"])
            .unwrap()
            .ok
        {
            return;
        }
        assert!(create(&sandbox.data(), &plain_dir, None).is_err());
        let empty = sandbox.dir.join("empty");
        fs::create_dir_all(&empty).unwrap();
        sandbox.git(&empty, &["init", "-q"]);
        let error = create(&sandbox.data(), &empty, None).unwrap_err();
        assert!(error.contains("no commits"), "{error}");
    }

    #[test]
    fn status_counts_changes_and_unmerged_commits() {
        let sandbox = Sandbox::new("status");
        let record = create(&sandbox.data(), &sandbox.repo(), Some("work")).unwrap();
        let clean = status(&record).unwrap();
        assert_eq!((clean.dirty, clean.unmerged), (0, 0));
        assert_eq!(clean.branch, "neuron/work");

        fs::write(record.path.join("b.txt"), "x\n").unwrap();
        fs::write(record.path.join("a.txt"), "changed\n").unwrap();
        assert_eq!(status(&record).unwrap().dirty, 2);

        sandbox.commit(&record.path, "work");
        fs::write(record.path.join("c.txt"), "y\n").unwrap();
        let after = status(&record).unwrap();
        assert_eq!((after.dirty, after.unmerged), (1, 1));
    }

    #[test]
    fn discard_needs_a_say_so_for_what_it_would_lose() {
        let sandbox = Sandbox::new("discard");
        let record = create(&sandbox.data(), &sandbox.repo(), Some("work")).unwrap();
        fs::write(record.path.join("b.txt"), "x\n").unwrap();
        sandbox.commit(&record.path, "work");
        fs::write(record.path.join("c.txt"), "y\n").unwrap();

        // Unmerged commits come first, and nothing is removed.
        assert_eq!(discard(&record, true, false).unwrap_err(), "unmerged:1");
        assert!(record.path.is_dir());
        // Then a dirty tree needs `force`.
        assert_eq!(discard(&record, false, true).unwrap_err(), "dirty:1");
        assert!(record.path.is_dir());

        discard(&record, true, true).unwrap();
        assert!(!record.path.exists());
        assert!(!branch_exists(&sandbox.repo(), "neuron/work"));
    }

    #[test]
    fn a_clean_merged_copy_is_removed_without_force() {
        let sandbox = Sandbox::new("clean");
        let record = create(&sandbox.data(), &sandbox.repo(), Some("tidy")).unwrap();
        discard(&record, false, false).unwrap();
        assert!(!record.path.exists());
        assert!(!branch_exists(&sandbox.repo(), "neuron/tidy"));
    }

    #[test]
    fn discard_stays_inside_the_worktree_root() {
        let sandbox = Sandbox::new("contain");
        let record = create(&sandbox.data(), &sandbox.repo(), Some("work")).unwrap();
        let outside = sandbox.dir.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "mine").unwrap();

        // A path that climbs out with `..` is judged by where it lands.
        let escaping = WorktreeRecord {
            path: record.root.join("..").join("..").join("outside"),
            ..record.clone()
        };
        assert!(discard(&escaping, true, true).is_err());
        // The root itself is not a worktree either.
        let root = WorktreeRecord {
            path: record.root.clone(),
            ..record.clone()
        };
        assert!(discard(&root, true, true).is_err());
        // A folder inside the root that git does not list is left alone.
        let stray = record.root.join("stray");
        fs::create_dir_all(&stray).unwrap();
        let unlisted = WorktreeRecord {
            path: stray.clone(),
            ..record.clone()
        };
        assert!(discard(&unlisted, true, true).is_err());
        // A record with someone else's branch name is not ours.
        let foreign = WorktreeRecord {
            branch: "main".into(),
            ..record.clone()
        };
        assert!(discard(&foreign, true, true).is_err());

        assert!(outside.join("keep.txt").is_file());
        assert!(stray.is_dir());
        assert!(record.path.is_dir());
    }
}
