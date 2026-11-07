//! `/pr` and `/issue` through the GitHub CLI (`gh`), for the line REPL and the engine.
//!
//! Nothing reaches GitHub without a second, explicit step, as with `/commit`:
//! - `/pr [--base <branch>] [context]` drafts a title and body with the session's
//!   model from `git log <base>..HEAD` and the diff stat (sizes capped; file names
//!   only, never file contents), and shows it.
//! - `/pr create [--draft] [--base <branch>]` opens the pull request shown last, if
//!   `HEAD` has not moved since: it pushes the branch (only when the remote lacks its
//!   commits) and runs `gh pr create --title … --body-file -`. With no draft shown for
//!   this `HEAD`, it drafts one and asks for `/pr create` again.
//! - `/issue <title>` drafts an issue; `/issue create` opens the drafted one with
//!   `gh issue create`.
//!
//! A missing `gh`, or one not signed in (`gh auth status`), is reported with how to fix
//! it. Every program runs through a [`CommandRunner`], so tests never touch GitHub.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use runtime::CancellationToken;

use crate::source_control::recent_prompts;

/// Most commits the draft request lists.
const MAX_LOG_COMMITS: usize = 50;
/// Most bytes of `git log` text the draft request carries.
const MAX_LOG_BYTES: usize = 6 * 1024;
/// Most bytes of `git diff --stat` the draft request carries.
const MAX_STAT_BYTES: usize = 4 * 1024;
/// Longest title, in characters.
const MAX_TITLE_CHARS: usize = 72;
/// Most commit subjects a fallback body lists.
const MAX_FALLBACK_SUBJECTS: usize = 20;
/// Prompts of the session a draft quotes for intent.
const DRAFT_PROMPTS: usize = 5;
/// Output tokens a draft may take.
const DRAFT_MAX_TOKENS: u32 = 2_048;
/// How long a draft may take before a body built from the commits is used.
const DRAFT_TIMEOUT: Duration = Duration::from_secs(45);
/// Longest prompt quoted in a draft request, in characters.
const MAX_PROMPT_CHARS: usize = 300;

pub(crate) const PR_DRAFT_INSTRUCTIONS: &str = "You write GitHub pull request descriptions. Reply with the title on the first line (at most 72 characters, plain text, no `Title:` prefix), then a blank line, then the body in GitHub Markdown: one or two sentences on what changed and why, then a bulleted list of the main changes, and a short Testing section only when the commits mention tests. Use only what the commits, files and requests show; do not invent details. No code fence around the reply, no trailers.";

pub(crate) const ISSUE_DRAFT_INSTRUCTIONS: &str = "You write GitHub issues. Reply with the issue body only, in GitHub Markdown: what is wrong or wanted, what was observed, and, when it applies, steps to reproduce and the expected result. Keep it short and factual, use only what the title and requests show, and do not invent details. No code fence around the reply.";

const GH_MISSING: &str = "The GitHub CLI (gh) is not installed or not on PATH. Install it from https://cli.github.com, run `gh auth login` in a terminal, then try again.";

const GH_SIGNED_OUT: &str = "The GitHub CLI (gh) is not signed in. Run `gh auth login` in a terminal (or set GH_TOKEN), then try again.";

/// A program's result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CmdOutput {
    pub(crate) success: bool,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Runs `git` and `gh` in the workspace. Tests put a fake in its place.
pub(crate) trait CommandRunner {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<CmdOutput>;
}

/// The real programs, in `cwd`. Git runs as [`runtime::workspace_git_command`] allows
/// (in an untrusted workspace, no repository config that runs programs). Neither may
/// prompt on the terminal.
pub(crate) struct SystemRunner {
    cwd: PathBuf,
}

impl SystemRunner {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }
}

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<CmdOutput> {
        let mut command = if program == "git" {
            runtime::workspace_git_command(&self.cwd).ok_or_else(|| {
                io::Error::other("This workspace is not trusted and its git configuration runs programs, so Neuron does not run git here. Trust the workspace first.")
            })?
        } else {
            let mut command = Command::new(program);
            command.current_dir(&self.cwd);
            command
        };
        command
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("NO_COLOR", "1")
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        let writer = child.stdin.take().map(|mut pipe| {
            let input = stdin.unwrap_or_default().as_bytes().to_vec();
            std::thread::spawn(move || {
                let _ = pipe.write_all(&input);
            })
        });
        let output = child.wait_with_output()?;
        if let Some(writer) = writer {
            let _ = writer.join();
        }
        Ok(CmdOutput {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// What a draft is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GithubKind {
    PullRequest,
    Issue,
}

/// A draft shown to the user, waiting for `/pr create` or `/issue create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Proposal {
    pub(crate) kind: GithubKind,
    pub(crate) title: String,
    pub(crate) body: String,
    /// Pull requests: the branch they merge into, the branch they come from, and the
    /// commit the draft was made at.
    pub(crate) base: Option<String>,
    pub(crate) head: Option<String>,
    pub(crate) head_sha: Option<String>,
}

/// Drafts text with a model: `(instructions, request)` to the reply.
pub(crate) type Drafter<'a> = dyn FnMut(&str, &str) -> Result<String, String> + 'a;

/// The draft shown last in this process.
static PENDING: Mutex<Option<Proposal>> = Mutex::new(None);

/// `/pr` and `/issue` of the running session: drafts with its model, runs `git` and
/// `gh` in the current directory.
impl crate::LiveCli {
    pub(crate) fn github_command(
        &self,
        kind: GithubKind,
        args: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let runner = SystemRunner::new(cwd);
        let prompts = recent_prompts(self.runtime.session(), DRAFT_PROMPTS);
        let client = self.runtime.api_client();
        let mut draft = |instructions: &str, request: &str| {
            client.complete_text(
                instructions,
                request,
                DRAFT_MAX_TOKENS,
                DRAFT_TIMEOUT,
                cancel,
            )
        };
        let mut pending = PENDING.lock().unwrap_or_else(PoisonError::into_inner);
        match kind {
            GithubKind::PullRequest => {
                pr_command(&runner, args, &mut pending, &mut draft, &prompts)
            }
            GithubKind::Issue => issue_command(&runner, args, &mut pending, &mut draft, &prompts),
        }
    }
}

/// `/pr` arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PrArgs {
    create: bool,
    draft: bool,
    base: Option<String>,
    context: Option<String>,
}

fn parse_pr_args(args: &str) -> Result<PrArgs, String> {
    let mut parsed = PrArgs::default();
    let mut words = args.split_whitespace().peekable();
    if words.peek() == Some(&"create") {
        words.next();
        parsed.create = true;
    }
    let mut context = Vec::new();
    while let Some(word) = words.next() {
        match word {
            "--draft" => parsed.draft = true,
            "--base" => {
                let base = words
                    .next()
                    .ok_or_else(|| "--base needs a branch name: /pr --base main".to_string())?;
                parsed.base = Some(base.to_string());
            }
            _ if word.starts_with("--base=") => {
                parsed.base = Some(word.trim_start_matches("--base=").to_string());
            }
            _ if parsed.create => {
                return Err(format!(
                    "Unknown argument {word}. Usage: /pr create [--draft] [--base <branch>]"
                ));
            }
            _ => context.push(word),
        }
    }
    if parsed.draft && !parsed.create {
        return Err("--draft goes with /pr create: /pr create --draft".to_string());
    }
    if !context.is_empty() {
        parsed.context = Some(context.join(" "));
    }
    Ok(parsed)
}

/// `/pr [create] …` (see the module docs). `pending` is the draft shown last.
pub(crate) fn pr_command(
    runner: &dyn CommandRunner,
    args: &str,
    pending: &mut Option<Proposal>,
    draft: &mut Drafter<'_>,
    prompts: &[String],
) -> Result<String, String> {
    let args = parse_pr_args(args)?;
    if args.create {
        check_gh(runner)?;
    }
    let branch = PrBranch::read(runner, args.base.as_deref())?;
    if args.create {
        let confirmed = pending.as_ref().is_some_and(|proposal| {
            proposal.kind == GithubKind::PullRequest
                && proposal.head.as_deref() == Some(branch.head.as_str())
                && proposal.head_sha.as_deref() == Some(branch.head_sha.as_str())
                && proposal.base.as_deref() == Some(branch.base.as_str())
        });
        if confirmed {
            let proposal = pending.take().expect("a confirmed draft");
            return create_pr(runner, &branch, &proposal, args.draft);
        }
    }
    let (proposal, note) = draft_pr(runner, &branch, args.context.as_deref(), prompts, draft)?;
    let mut text = note.map(|note| format!("{note}\n\n")).unwrap_or_default();
    text.push_str(&render_pr_proposal(&branch, &proposal));
    text.push_str("\n\n");
    if args.create {
        text.push_str(&format!(
            "Run /pr create{} again to push {} and open this pull request.",
            if args.draft { " --draft" } else { "" },
            branch.head
        ));
    } else {
        text.push_str(&format!(
            "To push {} and open this pull request: /pr create (add --draft for a draft).",
            branch.head
        ));
    }
    *pending = Some(proposal);
    Ok(text)
}

/// `/issue <title>`, `/issue create`, or `/issue` alone.
pub(crate) fn issue_command(
    runner: &dyn CommandRunner,
    args: &str,
    pending: &mut Option<Proposal>,
    draft: &mut Drafter<'_>,
    prompts: &[String],
) -> Result<String, String> {
    let args = args.trim();
    let drafted = pending
        .as_ref()
        .filter(|proposal| proposal.kind == GithubKind::Issue);
    if args == "create" {
        check_gh(runner)?;
        let Some(proposal) = drafted.cloned() else {
            return Err("There is no drafted issue. Draft one first with /issue <title>.".into());
        };
        let output = run(
            runner,
            "gh",
            &[
                "issue",
                "create",
                "--title",
                &proposal.title,
                "--body-file",
                "-",
            ],
            Some(&proposal.body),
        )?;
        *pending = None;
        return Ok(format!("Opened issue: {}", last_line(&output)));
    }
    if args.is_empty() {
        return Ok(match drafted {
            Some(proposal) => format!(
                "{}\n\nOpen it on GitHub with /issue create.",
                render_issue_proposal(proposal)
            ),
            None => "Usage: /issue <title> drafts an issue from the title and this conversation; /issue create then opens it on GitHub.".to_string(),
        });
    }
    let title = cut_chars(args, MAX_TITLE_CHARS);
    let request = issue_draft_request(&title, prompts);
    let (body, note) = match draft(ISSUE_DRAFT_INSTRUCTIONS, &request) {
        Ok(reply) if !strip_fence(&reply).trim().is_empty() => {
            (strip_fence(&reply).trim().to_string(), None)
        }
        Ok(_) => (
            fallback_issue_body(&title, prompts),
            Some("The model drafted no body; this one is built from the conversation.".to_string()),
        ),
        Err(error) => (
            fallback_issue_body(&title, prompts),
            Some(format!(
                "Could not draft the issue with the model ({error}); this one is built from the conversation."
            )),
        ),
    };
    let proposal = Proposal {
        kind: GithubKind::Issue,
        title,
        body,
        base: None,
        head: None,
        head_sha: None,
    };
    let mut text = note.map(|note| format!("{note}\n\n")).unwrap_or_default();
    text.push_str(&render_issue_proposal(&proposal));
    text.push_str("\n\nOpen it on GitHub with /issue create.");
    *pending = Some(proposal);
    Ok(text)
}

/// `gh` is there and signed in.
pub(crate) fn check_gh(runner: &dyn CommandRunner) -> Result<(), String> {
    match runner.run("gh", &["--version"], None) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(GH_MISSING.into()),
        Err(error) => return Err(format!("Could not run gh: {error}")),
        Ok(output) if !output.success => return Err(GH_MISSING.into()),
        Ok(_) => {}
    }
    match runner.run("gh", &["auth", "status"], None) {
        Ok(output) if output.success => Ok(()),
        Ok(_) => Err(GH_SIGNED_OUT.into()),
        Err(error) => Err(format!("Could not run gh: {error}")),
    }
}

/// The branch a pull request would come from, what it merges into, and what it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PrBranch {
    head: String,
    head_sha: String,
    /// The base branch's name on GitHub.
    base: String,
    /// What the commits are counted against: `origin/<base>` when the remote has it.
    base_ref: String,
    /// `(subject, body)` of each commit, oldest first.
    commits: Vec<(String, String)>,
    /// Commits on the branch past the base (the list may hold fewer).
    count: usize,
    /// `git diff --stat` against the base, capped.
    stat: String,
}

impl PrBranch {
    fn read(runner: &dyn CommandRunner, base: Option<&str>) -> Result<Self, String> {
        let head = git(runner, &["symbolic-ref", "--short", "-q", "HEAD"])
            .map_err(|_| {
                "HEAD is not on a branch (or this is not a git repository). Check out the branch the pull request should come from.".to_string()
            })?;
        let head_sha = git(runner, &["rev-parse", "HEAD"])
            .map_err(|_| "This branch has no commits yet. Commit first (/commit).".to_string())?;
        let (base, base_ref) = match base {
            Some(base) => named_base(runner, base)?,
            None => default_base(runner).ok_or_else(|| {
                "Could not tell which branch the pull request merges into. Name it: /pr --base <branch>.".to_string()
            })?,
        };
        if base == head {
            return Err(format!(
                "{head} is the base branch. Create a branch for the pull request first (git switch -c <name>)."
            ));
        }
        let range = format!("{base_ref}..HEAD");
        let count = git(runner, &["rev-list", "--count", "--no-merges", &range])?
            .parse::<usize>()
            .unwrap_or_default();
        if count == 0 {
            return Err(format!(
                "{head} has no commits that {base} lacks. Commit your changes first (/commit)."
            ));
        }
        let limit = format!("-n{MAX_LOG_COMMITS}");
        let log = git(
            runner,
            &[
                "log",
                "--no-merges",
                "--reverse",
                "--no-color",
                &limit,
                "--format=%x1e%s%x1f%b",
                &range,
            ],
        )?;
        let commits = log
            .split('\u{1e}')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                let (subject, body) = entry.split_once('\u{1f}').unwrap_or((entry, ""));
                (subject.trim().to_string(), body.trim().to_string())
            })
            .collect();
        let three_dot = format!("{base_ref}...HEAD");
        let stat = git(
            runner,
            &[
                "diff",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--stat=120",
                &three_dot,
            ],
        )
        .unwrap_or_default();
        Ok(Self {
            head,
            head_sha,
            base,
            base_ref,
            commits,
            count,
            stat: cut_bytes(&stat, MAX_STAT_BYTES),
        })
    }
}

/// The base named by the user: `origin/<base>` when the remote has it, else the local
/// branch.
fn named_base(runner: &dyn CommandRunner, base: &str) -> Result<(String, String), String> {
    let base = base.trim().trim_start_matches("origin/").to_string();
    let remote = format!("refs/remotes/origin/{base}");
    if git(runner, &["rev-parse", "--verify", "-q", &remote]).is_ok() {
        return Ok((base.clone(), format!("origin/{base}")));
    }
    let local = format!("refs/heads/{base}");
    if git(runner, &["rev-parse", "--verify", "-q", &local]).is_ok() {
        return Ok((base.clone(), base));
    }
    Err(format!("There is no branch {base}, here or on origin."))
}

/// The remote's default branch (`origin/HEAD`), else `main` or `master`.
fn default_base(runner: &dyn CommandRunner) -> Option<(String, String)> {
    if let Ok(name) = git(
        runner,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if let Some(base) = name.strip_prefix("origin/").filter(|base| !base.is_empty()) {
            return Some((base.to_string(), name.clone()));
        }
    }
    ["main", "master"]
        .into_iter()
        .find_map(|name| named_base(runner, name).ok())
}

/// Drafts the pull request with the model; on failure, from the commits. The note
/// says when the model's draft was not used.
fn draft_pr(
    _runner: &dyn CommandRunner,
    branch: &PrBranch,
    context: Option<&str>,
    prompts: &[String],
    draft: &mut Drafter<'_>,
) -> Result<(Proposal, Option<String>), String> {
    let request = pr_draft_request(branch, context, prompts);
    let (title, body, note) = match draft(PR_DRAFT_INSTRUCTIONS, &request) {
        Ok(reply) => match split_pr_draft(&reply) {
            Some((title, body)) => (title, body, None),
            None => {
                let (title, body) = fallback_pr(branch);
                (
                    title,
                    body,
                    Some(
                        "The model drafted nothing usable; this one lists the commits.".to_string(),
                    ),
                )
            }
        },
        Err(error) => {
            let (title, body) = fallback_pr(branch);
            (
                title,
                body,
                Some(format!(
                    "Could not draft the pull request with the model ({error}); this one lists the commits."
                )),
            )
        }
    };
    Ok((
        Proposal {
            kind: GithubKind::PullRequest,
            title,
            body,
            base: Some(branch.base.clone()),
            head: Some(branch.head.clone()),
            head_sha: Some(branch.head_sha.clone()),
        },
        note,
    ))
}

/// The request for a drafted pull request: what the user said, the commits and the
/// changed files with their line counts. No file contents.
fn pr_draft_request(branch: &PrBranch, context: Option<&str>, prompts: &[String]) -> String {
    let mut request = format!(
        "Write the pull request for branch {} into {}.\n",
        branch.head, branch.base
    );
    if let Some(context) = context.filter(|context| !context.trim().is_empty()) {
        request.push_str(&format!("\nThe user adds: {}\n", context.trim()));
    }
    push_prompts(&mut request, prompts);
    let mut log = String::new();
    for (subject, body) in &branch.commits {
        log.push_str(&format!("- {subject}\n"));
        for line in body.lines().filter(|line| !line.trim().is_empty()) {
            log.push_str(&format!("  {}\n", line.trim_end()));
        }
    }
    let shown = branch.commits.len();
    request.push_str(&format!(
        "\nCommits ({} in all{}), oldest first:\n{}",
        branch.count,
        if shown < branch.count {
            format!(", the first {shown} listed")
        } else {
            String::new()
        },
        cut_bytes(&log, MAX_LOG_BYTES)
    ));
    if !branch.stat.trim().is_empty() {
        request.push_str(&format!("\nFiles changed:\n{}\n", branch.stat.trim_end()));
    }
    request
}

fn issue_draft_request(title: &str, prompts: &[String]) -> String {
    let mut request = format!("Write the body of a GitHub issue titled: {title}\n");
    push_prompts(&mut request, prompts);
    request
}

fn push_prompts(request: &mut String, prompts: &[String]) {
    if prompts.is_empty() {
        return;
    }
    request.push_str("\nWhat the user asked for in this session, most recent last:\n");
    for prompt in prompts {
        let line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
        request.push_str(&format!("- {}\n", cut_chars(&line, MAX_PROMPT_CHARS)));
    }
}

/// The model's reply as `(title, body)`: the first line is the title (prefixes,
/// heading marks and quotes taken off, cut to [`MAX_TITLE_CHARS`]); the rest is the
/// body. `None` without a title.
pub(crate) fn split_pr_draft(reply: &str) -> Option<(String, String)> {
    let reply = strip_fence(reply);
    let mut lines = reply.lines();
    let title = lines
        .by_ref()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("Title:")
        .trim()
        .trim_matches(['"', '\'', '`', '*'])
        .trim();
    if title.is_empty() {
        return None;
    }
    let body = lines
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .trim_start_matches("Body:")
        .trim()
        .to_string();
    Some((cut_chars(title, MAX_TITLE_CHARS), body))
}

/// A reply wrapped whole in one code fence, unwrapped.
fn strip_fence(reply: &str) -> &str {
    let trimmed = reply.trim();
    if let Some(inner) = trimmed.strip_prefix("```") {
        if let Some(inner) = inner.strip_suffix("```") {
            return inner.split_once('\n').map_or("", |(_, rest)| rest);
        }
    }
    trimmed
}

/// A pull request built from the commits: the only commit's subject, or the branch
/// name, as the title; the subjects as the body.
fn fallback_pr(branch: &PrBranch) -> (String, String) {
    let title = match branch.commits.as_slice() {
        [(subject, _)] if !subject.is_empty() => subject.clone(),
        _ => {
            let name = branch.head.rsplit('/').next().unwrap_or(&branch.head);
            let words = name.replace(['-', '_'], " ");
            let mut chars = words.trim().chars();
            chars.next().map_or_else(
                || branch.head.clone(),
                |first| first.to_uppercase().chain(chars).collect(),
            )
        }
    };
    let mut body = String::from("## Changes\n\n");
    for (subject, _) in branch.commits.iter().take(MAX_FALLBACK_SUBJECTS) {
        body.push_str(&format!("- {subject}\n"));
    }
    if branch.count > MAX_FALLBACK_SUBJECTS {
        body.push_str(&format!(
            "- … and {} more commits\n",
            branch.count - MAX_FALLBACK_SUBJECTS
        ));
    }
    (
        cut_chars(&title, MAX_TITLE_CHARS),
        body.trim_end().to_string(),
    )
}

fn fallback_issue_body(title: &str, prompts: &[String]) -> String {
    let mut body = format!("{title}\n");
    if !prompts.is_empty() {
        body.push_str("\n## Context\n\n");
        for prompt in prompts {
            let line = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
            body.push_str(&format!("- {}\n", cut_chars(&line, MAX_PROMPT_CHARS)));
        }
    }
    body.trim_end().to_string()
}

/// Pushes the branch when the remote lacks its commits, then opens the pull request.
fn create_pr(
    runner: &dyn CommandRunner,
    branch: &PrBranch,
    proposal: &Proposal,
    draft: bool,
) -> Result<String, String> {
    let remote_key = format!("branch.{}.remote", branch.head);
    let remote = git(runner, &["config", "--get", &remote_key])
        .ok()
        .filter(|remote| !remote.is_empty() && remote != ".")
        .unwrap_or_else(|| "origin".to_string());
    if git(runner, &["remote", "get-url", &remote]).is_err() {
        return Err(format!(
            "There is no git remote named {remote}. Add the GitHub repository first: git remote add {remote} <url>."
        ));
    }
    let behind_remote = match git(
        runner,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
    ) {
        Ok(_) => {
            git(runner, &["rev-list", "--count", "@{u}..HEAD"]).map_or(true, |count| count != "0")
        }
        Err(_) => true,
    };
    let mut lines = Vec::new();
    if behind_remote {
        let output = runner
            .run(
                "git",
                &["push", "--set-upstream", &remote, &branch.head],
                None,
            )
            .map_err(|error| format!("Could not run git: {error}"))?;
        if !output.success {
            return Err(format!(
                "git push to {remote} failed:\n{}",
                failure_text(&output)
            ));
        }
        lines.push(format!("Pushed {} to {remote}.", branch.head));
    }
    let mut args = vec![
        "pr",
        "create",
        "--title",
        proposal.title.as_str(),
        "--body-file",
        "-",
        "--base",
        branch.base.as_str(),
        "--head",
        branch.head.as_str(),
    ];
    if draft {
        args.push("--draft");
    }
    let output = run(runner, "gh", &args, Some(&proposal.body))?;
    lines.push(format!(
        "Opened {}pull request: {}",
        if draft { "draft " } else { "" },
        last_line(&output)
    ));
    Ok(lines.join("\n"))
}

fn render_pr_proposal(branch: &PrBranch, proposal: &Proposal) -> String {
    format!(
        "Pull request draft: {} into {} ({} commit{})\n\nTitle: {}\n\n{}",
        branch.head,
        branch.base,
        branch.count,
        if branch.count == 1 { "" } else { "s" },
        proposal.title,
        proposal.body
    )
}

fn render_issue_proposal(proposal: &Proposal) -> String {
    format!(
        "Issue draft\n\nTitle: {}\n\n{}",
        proposal.title, proposal.body
    )
}

/// `git <args>`, trimmed stdout; an error with git's message when it fails.
fn git(runner: &dyn CommandRunner, args: &[&str]) -> Result<String, String> {
    run(runner, "git", args, None).map(|output| output.trim().to_string())
}

/// `program <args>`'s stdout; an error with its output when it fails.
fn run(
    runner: &dyn CommandRunner,
    program: &str,
    args: &[&str],
    stdin: Option<&str>,
) -> Result<String, String> {
    let output =
        runner
            .run(program, args, stdin)
            .map_err(|error| match (program, error.kind()) {
                ("gh", io::ErrorKind::NotFound) => GH_MISSING.to_string(),
                _ => format!("Could not run {program}: {error}"),
            })?;
    if output.success {
        Ok(output.stdout)
    } else {
        let name = args.iter().take(2).copied().collect::<Vec<_>>().join(" ");
        Err(format!(
            "{program} {name} failed:\n{}",
            failure_text(&output)
        ))
    }
}

fn failure_text(output: &CmdOutput) -> String {
    let text = [output.stderr.trim(), output.stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.is_empty() {
        "(no output)".to_string()
    } else {
        cut_chars(&text, 2_000)
    }
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn cut_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut = text.chars().take(max - 1).collect::<String>();
    cut.push('…');
    cut
}

fn cut_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n(cut short)\n", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::{
        check_gh, issue_command, pr_command, split_pr_draft, CmdOutput, CommandRunner, GithubKind,
        Proposal, SystemRunner, GH_MISSING, GH_SIGNED_OUT, PR_DRAFT_INSTRUCTIONS,
    };
    use std::cell::RefCell;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A recorded call: the program and its arguments joined by spaces, and stdin.
    type Call = (String, Option<String>);

    /// Answers calls whose `program args…` starts with a scripted prefix (the first
    /// match wins); anything else fails. `git` may go to a real runner instead.
    #[derive(Default)]
    struct FakeRunner {
        script: Vec<(String, Result<CmdOutput, io::ErrorKind>)>,
        real_git: Option<SystemRunner>,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeRunner {
        fn ok(mut self, prefix: &str, stdout: &str) -> Self {
            self.script.push((
                prefix.to_string(),
                Ok(CmdOutput {
                    success: true,
                    stdout: stdout.to_string(),
                    stderr: String::new(),
                }),
            ));
            self
        }

        fn fail(mut self, prefix: &str, stderr: &str) -> Self {
            self.script.push((
                prefix.to_string(),
                Ok(CmdOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: stderr.to_string(),
                }),
            ));
            self
        }

        fn missing(mut self, prefix: &str) -> Self {
            self.script
                .push((prefix.to_string(), Err(io::ErrorKind::NotFound)));
            self
        }

        /// A signed-in gh.
        fn gh_ready(self) -> Self {
            self.ok("gh --version", "gh version 2.60.0\n")
                .ok("gh auth status", "")
        }

        /// A repository on `feature/shop-page`, two commits past `origin/main`.
        fn feature_branch(self, head_sha: &str) -> Self {
            self.ok("git symbolic-ref --short -q HEAD", "feature/shop-page\n")
                .ok("git rev-parse HEAD", &format!("{head_sha}\n"))
                .ok(
                    "git symbolic-ref --quiet --short refs/remotes/origin/HEAD",
                    "origin/main\n",
                )
                .ok("git rev-list --count --no-merges origin/main..HEAD", "2\n")
                .ok(
                    "git log",
                    "\u{1e}Add the shop page\u{1f}With a cart.\n\u{1e}Style the shop\u{1f}\n",
                )
                .ok(
                    "git diff",
                    " shop/index.html | 40 ++++\n .env | 1 +\n 2 files changed, 41 insertions(+)\n",
                )
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }

        fn ran(&self, prefix: &str) -> bool {
            self.calls()
                .iter()
                .any(|(call, _)| call.starts_with(prefix))
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> io::Result<CmdOutput> {
            let call = format!("{program} {}", args.join(" "));
            self.calls
                .borrow_mut()
                .push((call.clone(), stdin.map(str::to_string)));
            if program == "git" {
                if let Some(real) = &self.real_git {
                    return real.run(program, args, stdin);
                }
            }
            match self
                .script
                .iter()
                .find(|(prefix, _)| call.starts_with(prefix.as_str()))
            {
                Some((_, Ok(output))) => Ok(output.clone()),
                Some((_, Err(kind))) => Err(io::Error::from(*kind)),
                None => Ok(CmdOutput::default()),
            }
        }
    }

    fn reply(text: &str) -> impl FnMut(&str, &str) -> Result<String, String> + '_ {
        move |_, _| Ok(text.to_string())
    }

    #[test]
    fn gh_missing_or_signed_out_says_how_to_fix_it() {
        let missing = FakeRunner::default().missing("gh --version");
        assert_eq!(check_gh(&missing), Err(GH_MISSING.to_string()));
        assert!(GH_MISSING.contains("https://cli.github.com"));
        let signed_out = FakeRunner::default()
            .ok("gh --version", "gh version 2.60.0")
            .fail(
                "gh auth status",
                "You are not logged into any GitHub hosts.",
            );
        assert_eq!(check_gh(&signed_out), Err(GH_SIGNED_OUT.to_string()));
        assert!(GH_SIGNED_OUT.contains("gh auth login"));

        // /pr create stops there: nothing is pushed or opened.
        let runner = FakeRunner::default()
            .missing("gh --version")
            .feature_branch("abc123");
        let mut pending = None;
        let error = pr_command(&runner, "create", &mut pending, &mut reply("T\n\nB"), &[])
            .expect_err("no gh");
        assert_eq!(error, GH_MISSING);
        assert!(!runner.ran("git push"));
    }

    #[test]
    fn pr_drafts_from_the_log_and_stat_and_creates_nothing() {
        let runner = FakeRunner::default().feature_branch("abc123");
        let mut pending = None;
        let mut seen = Vec::new();
        let mut draft = |instructions: &str, request: &str| {
            seen.push((instructions.to_string(), request.to_string()));
            Ok(
                "Title: Add the shop page\n\nAdds a shop page with a cart.\n\n- New page"
                    .to_string(),
            )
        };
        let text = pr_command(
            &runner,
            "for the spring sale",
            &mut pending,
            &mut draft,
            &["build a shop page".to_string()],
        )
        .expect("a draft");
        let (instructions, request) = seen.pop().expect("one draft request");
        assert_eq!(instructions, PR_DRAFT_INSTRUCTIONS);
        assert!(
            request.contains("branch feature/shop-page into main"),
            "{request}"
        );
        assert!(request.contains("The user adds: for the spring sale"));
        assert!(request.contains("- build a shop page"));
        assert!(
            request.contains("- Add the shop page\n  With a cart."),
            "{request}"
        );
        assert!(request.contains("shop/index.html | 40"), "{request}");
        assert!(text.contains("Pull request draft: feature/shop-page into main (2 commits)"));
        assert!(text.contains("Title: Add the shop page"));
        assert!(text.contains("/pr create"));
        let proposal = pending.expect("the draft waits for /pr create");
        assert_eq!(proposal.kind, GithubKind::PullRequest);
        assert_eq!(proposal.body, "Adds a shop page with a cart.\n\n- New page");
        assert_eq!(proposal.head_sha.as_deref(), Some("abc123"));
        assert!(!runner.ran("gh"));
        assert!(!runner.ran("git push"));
    }

    #[test]
    fn pr_create_opens_the_shown_draft_after_pushing() {
        let runner = FakeRunner::default()
            .gh_ready()
            .feature_branch("abc123")
            .ok("git config --get branch.feature/shop-page.remote", "")
            .ok("git remote get-url origin", "https://github.com/o/r.git\n")
            .fail(
                "git rev-parse --abbrev-ref --symbolic-full-name @{u}",
                "no upstream",
            )
            .ok("git push --set-upstream origin feature/shop-page", "")
            .ok("gh pr create", "https://github.com/o/r/pull/7\n");
        let mut pending = Some(Proposal {
            kind: GithubKind::PullRequest,
            title: "Add the shop page".into(),
            body: "Body text".into(),
            base: Some("main".into()),
            head: Some("feature/shop-page".into()),
            head_sha: Some("abc123".into()),
        });
        let mut never = |_: &str, _: &str| -> Result<String, String> {
            panic!("a shown draft is not drafted again")
        };
        let text =
            pr_command(&runner, "create --draft", &mut pending, &mut never, &[]).expect("created");
        assert_eq!(
            text,
            "Pushed feature/shop-page to origin.\nOpened draft pull request: https://github.com/o/r/pull/7"
        );
        assert!(pending.is_none());
        let (call, stdin) = runner
            .calls()
            .into_iter()
            .find(|(call, _)| call.starts_with("gh pr create"))
            .expect("gh pr create ran");
        assert_eq!(
            call,
            "gh pr create --title Add the shop page --body-file - --base main --head feature/shop-page --draft"
        );
        assert_eq!(stdin.as_deref(), Some("Body text"));
    }

    #[test]
    fn pr_create_without_a_shown_draft_or_after_head_moved_only_proposes() {
        let runner = FakeRunner::default().gh_ready().feature_branch("def456");
        // A draft made at an older HEAD does not count.
        let mut pending = Some(Proposal {
            kind: GithubKind::PullRequest,
            title: "Old".into(),
            body: "Old".into(),
            base: Some("main".into()),
            head: Some("feature/shop-page".into()),
            head_sha: Some("abc123".into()),
        });
        let text = pr_command(
            &runner,
            "create",
            &mut pending,
            &mut reply("New title\n\nNew body"),
            &[],
        )
        .expect("a fresh draft");
        assert!(text.contains("Run /pr create again"), "{text}");
        assert!(!runner.ran("git push"));
        assert!(!runner.ran("gh pr create"));
        assert_eq!(pending.expect("pending").title, "New title");
    }

    #[test]
    fn pr_refuses_the_base_branch_and_falls_back_without_the_model() {
        let on_main = FakeRunner::default()
            .ok("git symbolic-ref --short -q HEAD", "main\n")
            .ok("git rev-parse HEAD", "abc\n")
            .ok(
                "git symbolic-ref --quiet --short refs/remotes/origin/HEAD",
                "origin/main\n",
            );
        let error =
            pr_command(&on_main, "", &mut None, &mut reply("x"), &[]).expect_err("base branch");
        assert!(error.contains("main is the base branch"), "{error}");

        let runner = FakeRunner::default().feature_branch("abc123");
        let mut pending = None;
        let mut offline = |_: &str, _: &str| Err("network down".to_string());
        let text = pr_command(&runner, "", &mut pending, &mut offline, &[]).expect("fallback");
        assert!(text.contains("Could not draft the pull request with the model (network down)"));
        let proposal = pending.expect("pending");
        assert_eq!(proposal.title, "Shop page");
        assert_eq!(
            proposal.body,
            "## Changes\n\n- Add the shop page\n- Style the shop"
        );
    }

    #[test]
    fn issue_drafts_then_creates_only_on_issue_create() {
        let runner = FakeRunner::default()
            .gh_ready()
            .ok("gh issue create", "https://github.com/o/r/issues/3\n");
        let mut pending = None;
        let error = issue_command(&runner, "create", &mut pending, &mut reply(""), &[])
            .expect_err("nothing drafted");
        assert!(error.contains("/issue <title>"));

        let text = issue_command(
            &runner,
            "Footer tokens lag behind",
            &mut pending,
            &mut reply("The footer shows last turn's tokens."),
            &[],
        )
        .expect("drafted");
        assert!(text.contains("Title: Footer tokens lag behind"));
        assert!(text.contains("/issue create"));
        assert!(!runner.ran("gh issue create"));

        let text =
            issue_command(&runner, "create", &mut pending, &mut reply(""), &[]).expect("created");
        assert_eq!(text, "Opened issue: https://github.com/o/r/issues/3");
        let (call, stdin) = runner
            .calls()
            .into_iter()
            .find(|(call, _)| call.starts_with("gh issue create"))
            .expect("gh issue create ran");
        assert_eq!(
            call,
            "gh issue create --title Footer tokens lag behind --body-file -"
        );
        assert_eq!(
            stdin.as_deref(),
            Some("The footer shows last turn's tokens.")
        );
        assert!(pending.is_none());
    }

    #[test]
    fn drafts_are_split_into_a_title_and_a_body() {
        assert_eq!(
            split_pr_draft("```markdown\n# Title: \"Fix the footer\"\n\nBody line\n```"),
            Some(("Fix the footer".to_string(), "Body line".to_string()))
        );
        assert_eq!(split_pr_draft("  \n\n"), None);
        let long = "x".repeat(100);
        let (title, body) = split_pr_draft(&long).expect("title");
        assert_eq!(title.chars().count(), 72);
        assert_eq!(body, "");
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn git(args: &[&str], cwd: &Path) {
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
    }

    /// Real git against a bare "GitHub" remote, with gh faked: the branch is pushed and
    /// gh is asked to open the pull request.
    #[test]
    fn a_real_repository_is_pushed_before_gh_opens_the_pull_request() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp = TempDir(std::env::temp_dir().join(format!(
            "neuron-github-actions-{}-{nanos}",
            std::process::id()
        )));
        let remote = temp.0.join("remote.git");
        let work = temp.0.join("work");
        fs::create_dir_all(&remote).expect("remote");
        fs::create_dir_all(&work).expect("work");
        git(&["init", "--quiet", "--bare", "-b", "main"], &remote);
        git(&["init", "--quiet", "-b", "main"], &work);
        git(&["config", "user.email", "tests@example.com"], &work);
        git(&["config", "user.name", "Neuron Tests"], &work);
        git(&["config", "commit.gpgsign", "false"], &work);
        git(
            &["remote", "add", "origin", &remote.to_string_lossy()],
            &work,
        );
        fs::write(work.join("a.txt"), "a\n").expect("a");
        git(&["add", "-A"], &work);
        git(&["commit", "-q", "-m", "init"], &work);
        git(&["push", "-q", "origin", "main"], &work);
        git(&["remote", "set-head", "origin", "main"], &work);
        git(&["switch", "-q", "-c", "fix/footer"], &work);
        fs::write(work.join("b.txt"), "b\n").expect("b");
        git(&["add", "-A"], &work);
        git(&["commit", "-q", "-m", "Fix the footer"], &work);

        let runner = FakeRunner {
            real_git: Some(SystemRunner::new(work.clone())),
            ..FakeRunner::default()
        }
        .gh_ready()
        .ok("gh pr create", "https://github.com/o/r/pull/9\n");
        let mut pending = None;
        let shown = pr_command(
            &runner,
            "",
            &mut pending,
            &mut |_: &str, _: &str| Err("offline".to_string()),
            &[],
        )
        .expect("a draft");
        assert!(shown.contains("fix/footer into main (1 commit)"), "{shown}");
        assert_eq!(pending.as_ref().expect("pending").title, "Fix the footer");

        let text =
            pr_command(&runner, "create", &mut pending, &mut reply(""), &[]).expect("created");
        assert_eq!(
            text,
            "Pushed fix/footer to origin.\nOpened pull request: https://github.com/o/r/pull/9"
        );
        let pushed = Command::new("git")
            .args(["rev-parse", "--verify", "-q", "refs/heads/fix/footer"])
            .current_dir(&remote)
            .output()
            .expect("git");
        assert!(pushed.status.success(), "the branch reached the remote");
        let (call, _) = runner
            .calls()
            .into_iter()
            .find(|(call, _)| call.starts_with("gh pr create"))
            .expect("gh pr create ran");
        assert!(call.ends_with("--base main --head fix/footer"), "{call}");
    }
}
