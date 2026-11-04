//! `neuron colony "<task>"` and `/colony <task>` in the line REPL: Colony runs
//! ([`tools::colony`]) from the command line.
//!
//! The run's limits come from the account (`GET /api/account`, `limits.colony`), with
//! the Free numbers when the account says nothing or cannot be read. Progress goes to
//! stderr, one line whenever an agent starts or ends; the report goes to stdout (as
//! JSON with `--output-format json`). On a terminal the run asks before the workers
//! start, when it nears its token cap, and before it applies the changes. Without a
//! terminal nobody can answer, so `--yes` is needed to start, the run stops at 80% of
//! the cap, and the changes stay on their branch unless `--yes` was given. Ctrl-C
//! stops the run (a second press quits); its branches stay for the user.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::{Mutex, PoisonError};

use runtime::PermissionMode;
use tools::colony::{
    describe_plan, run_colony, session_agent_runner, AgentStatus, ColonyAnswer, ColonyConfig,
    ColonyEvent, ColonyHandle, ColonyLimits, ColonyObserver, ColonyQuestion, ColonyReport,
};

pub(crate) const COLONY_USAGE: &str = "neuron colony \"<task>\" [--workers N] [--budget 400k] [--review-model MODEL] [--no-merge] [--allow-web] [--yes]";

/// What `neuron colony` and `/colony` were asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColonyOptions {
    pub task: String,
    pub workers: Option<usize>,
    pub budget: Option<u64>,
    pub review_model: Option<String>,
    pub merge: bool,
    pub allow_web: bool,
    /// Start without asking, and apply the changes without asking.
    pub yes: bool,
}

/// Reads `[--workers N] [--budget 400k] [--review-model M] [--no-merge] [--allow-web]
/// [--yes] <task words>`; options may come before or after the task.
pub(crate) fn parse_colony_args(args: &[String]) -> Result<ColonyOptions, String> {
    let mut options = ColonyOptions {
        task: String::new(),
        workers: None,
        budget: None,
        review_model: None,
        merge: true,
        allow_web: false,
        yes: false,
    };
    let mut words = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (arg, None),
        };
        let mut value = |name: &str| -> Result<String, String> {
            if let Some(value) = inline.clone() {
                return Ok(value);
            }
            index += 1;
            args.get(index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value. Usage: {COLONY_USAGE}"))
        };
        match flag {
            "--workers" => {
                let text = value("--workers")?;
                let workers = text
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|workers| *workers > 0)
                    .ok_or_else(|| format!("--workers takes a number from 1 up, not `{text}`"))?;
                options.workers = Some(workers);
            }
            "--budget" => options.budget = Some(parse_token_count(&value("--budget")?)?),
            "--review-model" => {
                options.review_model = Some(value("--review-model")?.trim().to_string());
            }
            "--no-merge" => options.merge = false,
            "--allow-web" => options.allow_web = true,
            "--yes" | "-y" => options.yes = true,
            "-h" | "--help" | "help" if words.is_empty() => {
                return Err(format!("Usage: {COLONY_USAGE}"));
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option `{other}`. Usage: {COLONY_USAGE}"));
            }
            _ => words.push(arg.to_string()),
        }
        index += 1;
    }
    options.task = words.join(" ").trim().to_string();
    if options.task.is_empty() {
        return Err(format!("no task. Usage: {COLONY_USAGE}"));
    }
    Ok(options)
}

/// `400k`, `2m`, `1.5m`, `300000` or `300_000` tokens.
pub(crate) fn parse_token_count(text: &str) -> Result<u64, String> {
    let cleaned = text.trim().to_ascii_lowercase().replace(['_', ','], "");
    let (number, scale) = match cleaned.chars().last() {
        Some('k') => (&cleaned[..cleaned.len() - 1], 1_000.0),
        Some('m') => (&cleaned[..cleaned.len() - 1], 1_000_000.0),
        _ => (cleaned.as_str(), 1.0),
    };
    number
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|value| value * scale)
        .filter(|tokens| *tokens >= 1.0 && *tokens <= 1e12)
        // In range and positive, so the cast neither wraps nor truncates a sign.
        .map(|tokens| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let tokens = tokens.round() as u64;
            tokens
        })
        .ok_or_else(|| format!("`{text}` is not a token count (for example 400k or 2m)"))
}

/// The account's Colony limits, or the Free ones with the reason they are used.
pub(crate) fn account_limits() -> (ColonyLimits, Option<String>) {
    match read_account() {
        Ok(account) => {
            let limits = ColonyLimits::from_account(&account);
            let note = account["limits"]["colony"]
                .is_null()
                .then(|| "The account does not list Colony limits yet; using the Free ones (1 worker, 300,000 tokens).".to_string());
            (limits, note)
        }
        Err(reason) => (
            ColonyLimits::FREE,
            Some(format!(
                "Using the Free Colony limits (1 worker, 300,000 tokens): {reason}"
            )),
        ),
    }
}

fn read_account() -> Result<serde_json::Value, String> {
    let base = crate::auth::gateway_base_url().map_err(|error| error.to_string())?;
    let credential = crate::auth::current_credential()
        .map_err(|error| error.to_string())?
        .ok_or("not signed in to Zero-X")?;
    crate::gateway_http::get_json(
        &crate::gateway_http::account_url(&base, "/api/account"),
        Some(credential.token.expose()),
        crate::gateway_http::ACCOUNT_TIMEOUT,
    )
    .map_err(|error| error.to_string())
}

/// The run's settings from the options, the account limits and the session.
/// `session_tools` is the session's tool allow-list (`--allowedTools`, plan mode);
/// agents get only tools on it and on the registered parent's list.
pub(crate) fn build_config(
    options: &ColonyOptions,
    limits: ColonyLimits,
    session_model: &str,
    permission_mode: PermissionMode,
    session_tools: Option<&BTreeSet<String>>,
) -> Result<ColonyConfig, String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let home = runtime::workspace_trust::default_neuron_home()
        .unwrap_or_else(|| std::env::temp_dir().join("neuron"));
    let parent = tools::registered_subagent_parent();
    let main_model = parent
        .as_ref()
        .map_or_else(|| session_model.to_string(), |parent| parent.model.clone());
    let worker_model = parent
        .as_ref()
        .and_then(|parent| parent.subagent_model.clone())
        .unwrap_or_else(|| main_model.clone());
    let reviewer_model = options
        .review_model
        .clone()
        .filter(|model| !model.is_empty())
        .or_else(|| {
            std::env::var(tools::REVIEW_MODEL_ENV_VAR)
                .ok()
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        })
        .or_else(reviewer_model_setting)
        .map_or_else(
            || worker_model.clone(),
            |model| crate::model_routing::transport_model(&main_model, &model),
        );
    Ok(ColonyConfig {
        task: options.task.clone(),
        limits,
        workers: options.workers,
        token_cap: options.budget,
        main_model,
        worker_model,
        reviewer_model,
        permission_mode,
        allowed_tools: both_allow_lists(
            session_tools,
            parent
                .as_ref()
                .and_then(|parent| parent.allowed_tools.as_ref()),
        ),
        permission_rules: parent
            .map(|parent| parent.permission_rules)
            .unwrap_or_default(),
        allow_web: options.allow_web,
        merge: options.merge,
        workspace,
        home,
    })
}

/// The tools on both lists (`None`: no list).
fn both_allow_lists(
    first: Option<&BTreeSet<String>>,
    second: Option<&BTreeSet<String>>,
) -> Option<BTreeSet<String>> {
    match (first, second) {
        (Some(first), Some(second)) => Some(first.intersection(second).cloned().collect()),
        (Some(list), None) | (None, Some(list)) => Some(list.clone()),
        (None, None) => None,
    }
}

fn reviewer_model_setting() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let config = runtime::ConfigLoader::default_for(&cwd).load().ok()?;
    config
        .feature_config()
        .model_routing()
        .reviewer_model()
        .map(str::to_string)
}

/// Shows progress on stderr and asks on the terminal.
pub(crate) struct TerminalObserver {
    interactive: bool,
    yes: bool,
    statuses: Mutex<BTreeMap<String, AgentStatus>>,
    asking: Mutex<()>,
}

impl TerminalObserver {
    pub(crate) fn new(interactive: bool, yes: bool) -> Self {
        Self {
            interactive,
            yes,
            statuses: Mutex::new(BTreeMap::new()),
            asking: Mutex::new(()),
        }
    }

    fn ask_yes_no(&self, question: &str) -> bool {
        let _one_at_a_time = self.asking.lock().unwrap_or_else(PoisonError::into_inner);
        eprint!("{question} [y/N] ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).is_err() {
            return false;
        }
        matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    }
}

impl ColonyObserver for TerminalObserver {
    fn event(&self, event: &ColonyEvent) {
        match event {
            ColonyEvent::Planned {
                tasks,
                workers,
                token_cap,
                ..
            } => {
                // Each subtask's instructions too: they are what the start approves.
                eprintln!(
                    "[colony] plan: {} subtask(s), {workers} at a time, token cap {}{}",
                    tasks.len(),
                    crate::account::group_digits(*token_cap),
                    describe_plan(tasks)
                );
            }
            ColonyEvent::Agent { row, .. } => {
                let mut statuses = self.statuses.lock().unwrap_or_else(PoisonError::into_inner);
                if statuses.get(&row.id) == Some(&row.status) {
                    return;
                }
                statuses.insert(row.id.clone(), row.status);
                if row.status == AgentStatus::Waiting {
                    return;
                }
                let budget = row.token_budget.map_or(String::new(), |budget| {
                    format!(" of {}", crate::account::group_digits(budget))
                });
                let note = row
                    .note
                    .as_deref()
                    .map_or(String::new(), |note| format!(" ({note})"));
                eprintln!(
                    "[colony] {} {}: {}, {} tokens{budget}{note}",
                    row.role.as_str(),
                    row.title,
                    status_word(row.status),
                    crate::account::group_digits(row.tokens_used),
                );
            }
            ColonyEvent::Budget { .. } | ColonyEvent::Finished { .. } => {}
        }
    }

    fn ask(&self, question: &ColonyQuestion) -> ColonyAnswer {
        match question {
            ColonyQuestion::Start {
                allow_web,
                read_only,
                ..
            } => {
                if let Some(reason) = read_only {
                    eprintln!("[colony] {reason}");
                }
                if self.yes {
                    return ColonyAnswer::Continue;
                }
                if !self.interactive {
                    return ColonyAnswer::Stop;
                }
                let web = if *allow_web { "on" } else { "off" };
                if self.ask_yes_no(&format!("[colony] Web reads are {web}. Start the workers?")) {
                    ColonyAnswer::Continue
                } else {
                    ColonyAnswer::Stop
                }
            }
            ColonyQuestion::NearCap { used, limit } => {
                if !self.interactive {
                    eprintln!(
                        "[colony] {} of {} tokens used; stopping (nobody can answer here).",
                        crate::account::group_digits(*used),
                        crate::account::group_digits(*limit)
                    );
                    return ColonyAnswer::Stop;
                }
                if self.ask_yes_no(&format!(
                    "[colony] {} of {} tokens used. Go on up to the cap?",
                    crate::account::group_digits(*used),
                    crate::account::group_digits(*limit)
                )) {
                    ColonyAnswer::Continue
                } else {
                    ColonyAnswer::Stop
                }
            }
            ColonyQuestion::Merge {
                branch, diff_stat, ..
            } => {
                eprintln!("[colony] the changes, on branch {branch}:\n{diff_stat}");
                if self.yes {
                    return ColonyAnswer::Merge;
                }
                if self.interactive
                    && self.ask_yes_no("[colony] Apply them to your working tree (uncommitted)?")
                {
                    ColonyAnswer::Merge
                } else {
                    ColonyAnswer::Keep
                }
            }
        }
    }
}

fn status_word(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Waiting => "waiting",
        AgentStatus::Running => "running",
        AgentStatus::Done => "done",
        AgentStatus::Failed => "failed",
        AgentStatus::Stopped => "stopped",
    }
}

/// The report as text.
pub(crate) fn render_report(report: &ColonyReport) -> String {
    use std::fmt::Write as _;
    let mut text = format!(
        "Colony run {}: {}\nTokens: {} of {}\n",
        report.run_id,
        report.status,
        crate::account::group_digits(report.tokens_used),
        crate::account::group_digits(report.token_cap)
    );
    for task in &report.tasks {
        let _ = write!(
            text,
            "\n{} {} [{}]",
            task.id,
            task.title,
            status_word(task.status)
        );
        if let Some(branch) = &task.branch {
            let _ = write!(text, " on {branch}");
        }
        let _ = write!(text, "\n{}\n", task.summary.trim());
        if let Some(review) = &task.review {
            let _ = writeln!(text, "Review: {}", review.trim());
        }
    }
    if let Some(conflict) = &report.conflict {
        let _ = writeln!(
            text,
            "\nMerging stopped at a conflict in {} ({}). Not merged: {}. Merge those branches yourself.",
            conflict.branch,
            conflict.files.join(", "),
            if conflict.not_merged.is_empty() {
                "none".to_string()
            } else {
                conflict.not_merged.join(", ")
            }
        );
    }
    if let Some(synthesis) = &report.synthesis {
        let _ = write!(text, "\n{}\n", synthesis.trim());
    }
    if report.merged {
        text.push_str("\nThe changes are in your working tree, not committed.\n");
    } else if let Some(branch) = &report.synthesis_branch {
        let _ = writeln!(
            text,
            "\nThe changes are on branch {branch}; merge it with `git merge {branch}` when you are ready."
        );
    }
    if let Some(note) = &report.note {
        let _ = writeln!(text, "\n{note}");
    }
    text
}

/// Runs `neuron colony` (and the REPL's `/colony`) in this process, which must have
/// registered its sub-agent parent context (a built `LiveCli`).
pub(crate) fn run(
    options: &ColonyOptions,
    session_model: &str,
    permission_mode: PermissionMode,
    session_tools: Option<&BTreeSet<String>>,
    json: bool,
) -> Result<ColonyReport, String> {
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    if !interactive && !options.yes {
        return Err(
            "a Colony run asks before its workers start, and nobody can answer here. Pass --yes to start without asking."
                .to_string(),
        );
    }
    let (limits, note) = account_limits();
    if let Some(note) = note {
        eprintln!("[colony] {note}");
    }
    let config = build_config(
        options,
        limits,
        session_model,
        permission_mode,
        session_tools,
    )?;
    // Ctrl-C stops the run as it stops a turn: every agent and any further start.
    let cancel = runtime::CancellationToken::new();
    let handle = ColonyHandle::with_cancel(config.token_cap(), cancel.clone());
    let interrupts =
        crate::HookAbortMonitor::spawn_for_turn(runtime::HookAbortSignal::new(), cancel);
    let observer = TerminalObserver::new(interactive, options.yes);
    let rules = config.permission_rules.clone();
    let runner = session_agent_runner(rules);
    let report = run_colony(&config, &runner, &observer, &handle);
    interrupts.stop();
    if json {
        let value = serde_json::json!({ "type": "colony", "report": report });
        println!("{value}");
    } else {
        println!("{}", render_report(&report));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{
        both_allow_lists, parse_colony_args, parse_token_count, render_report, ColonyOptions,
    };
    use std::collections::BTreeSet;
    use tools::colony::{AgentStatus, ColonyReport, MergeConflict, TaskReport};

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn options_come_before_or_after_the_task() {
        let options = parse_colony_args(&args(&[
            "--workers",
            "2",
            "add",
            "tests",
            "--budget=400k",
            "--no-merge",
            "--allow-web",
            "--review-model",
            "rev",
            "-y",
        ]))
        .expect("parses");
        assert_eq!(
            options,
            ColonyOptions {
                task: "add tests".to_string(),
                workers: Some(2),
                budget: Some(400_000),
                review_model: Some("rev".to_string()),
                merge: false,
                allow_web: true,
                yes: true,
            }
        );
        assert!(parse_colony_args(&args(&[]))
            .unwrap_err()
            .contains("no task"));
        assert!(parse_colony_args(&args(&["--workers", "0", "x"]))
            .unwrap_err()
            .contains("--workers"));
        assert!(parse_colony_args(&args(&["--frobnicate", "x"]))
            .unwrap_err()
            .contains("unknown option"));
        assert!(parse_colony_args(&args(&["x", "--budget"]))
            .unwrap_err()
            .contains("needs a value"));
    }

    #[test]
    fn agents_get_only_tools_on_both_allow_lists() {
        let set = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<BTreeSet<_>>()
        };
        let session = set(&["read_file", "bash"]);
        let parent = set(&["read_file", "write_file"]);
        assert_eq!(
            both_allow_lists(Some(&session), Some(&parent)),
            Some(set(&["read_file"]))
        );
        assert_eq!(
            both_allow_lists(Some(&session), None),
            Some(session.clone())
        );
        assert_eq!(both_allow_lists(None, Some(&parent)), Some(parent.clone()));
        assert_eq!(both_allow_lists(None, None), None);
    }

    #[test]
    fn token_counts_take_suffixes() {
        assert_eq!(parse_token_count("400k"), Ok(400_000));
        assert_eq!(parse_token_count("1.5M"), Ok(1_500_000));
        assert_eq!(parse_token_count("300_000"), Ok(300_000));
        assert!(parse_token_count("lots").is_err());
        assert!(parse_token_count("-5k").is_err());
        assert!(parse_token_count("0").is_err());
    }

    #[test]
    fn the_report_names_branches_conflicts_and_what_to_do() {
        let report = ColonyReport {
            run_id: "colony-1".to_string(),
            task: "t".to_string(),
            status: "conflict".to_string(),
            tasks: vec![TaskReport {
                id: "t1".to_string(),
                title: "One".to_string(),
                writes: true,
                status: AgentStatus::Done,
                branch: Some("neuron/agent/colony-1-t1".to_string()),
                summary: "Wrote a.txt.".to_string(),
                review: Some("LGTM".to_string()),
                tokens: 10,
            }],
            synthesis: Some("Summary.".to_string()),
            synthesis_branch: Some("neuron/agent/colony-1-synthesis".to_string()),
            conflict: Some(MergeConflict {
                branch: "neuron/agent/colony-1-t2".to_string(),
                files: vec!["a.txt".to_string()],
                not_merged: Vec::new(),
            }),
            merged: false,
            tokens_used: 1_234,
            token_cap: 300_000,
            agents: Vec::new(),
            note: None,
        };
        let text = render_report(&report);
        assert!(text.starts_with("Colony run colony-1: conflict\nTokens: 1,234 of 300,000"));
        assert!(text.contains("t1 One [done] on neuron/agent/colony-1-t1"));
        assert!(text.contains("Review: LGTM"));
        assert!(text.contains("conflict in neuron/agent/colony-1-t2 (a.txt)"));
        assert!(text.contains("git merge neuron/agent/colony-1-synthesis"));
    }
}
