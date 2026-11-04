//! Colony runs against a scripted provider in a temporary git repository: the real
//! conversation runtime, tools, worktrees and merges, with model replies from a
//! script.

use super::{
    parse_plan, run_colony, AgentRequest, AgentRole, AgentStatus, ColonyAnswer, ColonyConfig,
    ColonyEvent, ColonyHandle, ColonyLimits, ColonyObserver, ColonyQuestion, RuntimeAgentRunner,
};
use runtime::{
    ApiClient, ApiRequest, AssistantEvent, PermissionMode, RuntimeError,
    RuntimePermissionRuleConfig, TokenUsage,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOKENS_PER_RESPONSE: u32 = 1_000;

struct TempRepo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl TempRepo {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "neuron-colony-{label}-{}-{nanos}",
            std::process::id()
        ));
        let repo = root.join("repo");
        let home = root.join("home");
        std::fs::create_dir_all(&repo).expect("repo dir");
        std::fs::create_dir_all(&home).expect("home dir");
        git(&repo, &["init", "--quiet"]);
        git(&repo, &["config", "user.name", "Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("README.md"), "hello\n").expect("readme");
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "--quiet", "-m", "init"]);
        Self { root, repo, home }
    }

    fn branches(&self) -> Vec<String> {
        let output = Command::new("git")
            .args(["branch", "--list", "neuron/*", "--format=%(refname:short)"])
            .current_dir(&self.repo)
            .output()
            .expect("git branch");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

/// Replies from a script, billed [`TOKENS_PER_RESPONSE`] each, a little slowly so
/// agents that run at the same time overlap.
struct ScriptedClient {
    replies: VecDeque<Vec<AssistantEvent>>,
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    counts: bool,
    /// Wait (up to 3 s) for a second worker to be in a request too, so a test of
    /// parallel workers does not depend on timing.
    await_overlap: bool,
}

impl ApiClient for ScriptedClient {
    fn stream(&mut self, _request: ApiRequest) -> Result<Vec<AssistantEvent>, RuntimeError> {
        if self.counts {
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
        }
        if self.counts && self.await_overlap {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while self.peak.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        std::thread::sleep(Duration::from_millis(80));
        if self.counts {
            self.running.fetch_sub(1, Ordering::SeqCst);
        }
        let mut events = self
            .replies
            .pop_front()
            .unwrap_or_else(|| vec![AssistantEvent::TextDelta("done".to_string())]);
        events.push(AssistantEvent::Usage(TokenUsage {
            input_tokens: TOKENS_PER_RESPONSE,
            ..TokenUsage::default()
        }));
        events.push(AssistantEvent::MessageStop);
        Ok(events)
    }
}

fn text(reply: &str) -> Vec<AssistantEvent> {
    vec![AssistantEvent::TextDelta(reply.to_string())]
}

fn write(path: &str, content: &str) -> Vec<AssistantEvent> {
    vec![AssistantEvent::ToolUse {
        id: format!("write-{path}"),
        name: "write_file".to_string(),
        input: serde_json::json!({ "path": path, "content": content }).to_string(),
    }]
}

/// The script: replies per agent id.
struct Script {
    replies: BTreeMap<String, Vec<Vec<AssistantEvent>>>,
    running: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    seen: Mutex<Vec<(String, AgentRole, PermissionMode)>>,
    await_overlap: bool,
}

impl Script {
    fn new(replies: Vec<(&str, Vec<Vec<AssistantEvent>>)>) -> Self {
        Self {
            replies: replies
                .into_iter()
                .map(|(id, replies)| (id.to_string(), replies))
                .collect(),
            running: Arc::default(),
            peak: Arc::default(),
            seen: Mutex::new(Vec::new()),
            await_overlap: false,
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn client(&self, request: &AgentRequest) -> Result<ScriptedClient, String> {
        self.seen
            .lock()
            .unwrap()
            .push((request.id.clone(), request.role, request.permission_mode));
        let replies = self
            .replies
            .get(&request.id)
            .cloned()
            .unwrap_or_else(|| vec![text("LGTM")]);
        Ok(ScriptedClient {
            replies: replies.into(),
            running: Arc::clone(&self.running),
            peak: Arc::clone(&self.peak),
            counts: request.role == AgentRole::Worker,
            await_overlap: self.await_overlap,
        })
    }
}

/// Records events and answers questions; checks the working tree when asked to merge.
struct Recorder {
    events: Mutex<Vec<ColonyEvent>>,
    questions: Mutex<Vec<ColonyQuestion>>,
    start: ColonyAnswer,
    near_cap: ColonyAnswer,
    merge: ColonyAnswer,
    repo: PathBuf,
    tree_untouched_at_merge: Mutex<Option<bool>>,
}

impl Recorder {
    fn new(repo: &Path, near_cap: ColonyAnswer, merge: ColonyAnswer) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            questions: Mutex::new(Vec::new()),
            start: ColonyAnswer::Continue,
            near_cap,
            merge,
            repo: repo.to_path_buf(),
            tree_untouched_at_merge: Mutex::new(None),
        }
    }
}

impl ColonyObserver for Recorder {
    fn event(&self, event: &ColonyEvent) {
        self.events.lock().unwrap().push(event.clone());
    }

    fn ask(&self, question: &ColonyQuestion) -> ColonyAnswer {
        self.questions.lock().unwrap().push(question.clone());
        match question {
            ColonyQuestion::Start { .. } => self.start,
            ColonyQuestion::NearCap { .. } => self.near_cap,
            ColonyQuestion::Merge { .. } => {
                *self.tree_untouched_at_merge.lock().unwrap() =
                    Some(!self.repo.join("a.txt").exists() && !self.repo.join("b.txt").exists());
                self.merge
            }
        }
    }
}

fn config(repo: &TempRepo, limits: ColonyLimits) -> ColonyConfig {
    ColonyConfig {
        task: "Add a.txt and b.txt, and say what README.md says".to_string(),
        limits,
        workers: None,
        token_cap: None,
        main_model: "main-model".to_string(),
        worker_model: "worker-model".to_string(),
        reviewer_model: "review-model".to_string(),
        permission_mode: PermissionMode::WorkspaceWrite,
        permission_rules: RuntimePermissionRuleConfig::default(),
        allow_web: false,
        merge: true,
        workspace: repo.repo.clone(),
        home: repo.home.clone(),
    }
}

const THREE_TASKS: &str = r#"Here is the plan:
```json
{"tasks": [
  {"id": "t1", "title": "Add a.txt", "prompt": "Create a.txt", "writes": true, "files": ["a.txt"]},
  {"id": "t2", "title": "Add b.txt", "prompt": "Create b.txt", "writes": true, "files": ["b.txt"]},
  {"id": "t3", "title": "Read the readme", "prompt": "Say what README.md says", "writes": false}
]}
```"#;

#[test]
fn three_workers_plan_work_review_and_merge_after_approval() {
    let repo = TempRepo::new("three");
    let mut script = Script::new(vec![
        ("planner", vec![text(THREE_TASKS)]),
        (
            "worker-t1",
            vec![write("a.txt", "alpha\n"), text("Wrote a.txt.")],
        ),
        (
            "worker-t2",
            vec![write("b.txt", "beta\n"), text("Wrote b.txt.")],
        ),
        ("worker-t3", vec![text("README.md says hello.")]),
        ("synthesis", vec![text("All three subtasks finished.")]),
    ]);
    script.await_overlap = true;
    let runner = RuntimeAgentRunner::new(
        |request: &AgentRequest| script.client(request),
        RuntimePermissionRuleConfig::default(),
    );
    let recorder = Recorder::new(&repo.repo, ColonyAnswer::Continue, ColonyAnswer::Merge);
    let config = config(
        &repo,
        ColonyLimits {
            max_workers: 3,
            max_tokens_per_run: 2_000_000,
        },
    );
    let handle = ColonyHandle::new(config.token_cap());

    let report = run_colony(&config, &runner, &recorder, &handle);

    assert_eq!(report.status, "completed", "{report:#?}");
    assert_eq!(report.tasks.len(), 3);
    assert!(report
        .tasks
        .iter()
        .all(|task| task.status == AgentStatus::Done));
    assert_eq!(report.tasks[2].summary, "README.md says hello.");
    assert_eq!(report.tasks[0].review.as_deref(), Some("LGTM"));
    assert_eq!(
        report.tasks[2].review, None,
        "read-only work is not reviewed"
    );
    assert_eq!(
        report.synthesis.as_deref(),
        Some("All three subtasks finished.")
    );
    // Workers overlapped; the tree changed only after the merge was approved.
    assert!(
        script.peak.load(Ordering::SeqCst) >= 2,
        "workers ran one at a time"
    );
    assert_eq!(
        *recorder.tree_untouched_at_merge.lock().unwrap(),
        Some(true)
    );
    assert!(report.merged);
    assert_eq!(
        std::fs::read_to_string(repo.repo.join("a.txt"))
            .expect("a.txt")
            .replace("\r\n", "\n"),
        "alpha\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.repo.join("b.txt"))
            .expect("b.txt")
            .replace("\r\n", "\n"),
        "beta\n"
    );
    assert!(repo.branches().is_empty(), "{:?}", repo.branches());
    // At least: the planner, 3 workers (2 + 2 + 1 responses), 2 reviewers and the
    // synthesis (the runtime's completion check may add a response per writer). The
    // run's total is every agent's usage.
    assert!(report.tokens_used >= 9 * u64::from(TOKENS_PER_RESPONSE));
    assert_eq!(handle.meter().used(), report.tokens_used);
    assert_eq!(
        report.agents.iter().map(|row| row.tokens_used).sum::<u64>(),
        report.tokens_used
    );
    // Writers write in a worktree; the others only read.
    let seen = script.seen.lock().unwrap().clone();
    for (id, role, mode) in &seen {
        let writer = matches!(id.as_str(), "worker-t1" | "worker-t2");
        let expected = if writer {
            PermissionMode::WorkspaceWrite
        } else {
            PermissionMode::ReadOnly
        };
        assert_eq!(*mode, expected, "{id} ({role:?})");
    }
    assert_eq!(seen.len(), 7);
    // Live rows: every agent got one, and ended done.
    let events = recorder.events.lock().unwrap().clone();
    assert!(events.iter().any(|event| matches!(
        event,
        ColonyEvent::Planned { tasks, workers: 3, .. } if tasks.len() == 3
    )));
    assert_eq!(report.agents.len(), 7);
    assert!(report
        .agents
        .iter()
        .all(|row| row.status == AgentStatus::Done && row.tokens_used > 0));
    assert!(matches!(events.last(), Some(ColonyEvent::Finished { .. })));
}

#[test]
fn a_conflict_stops_the_merge_and_names_the_branch_and_files() {
    let repo = TempRepo::new("conflict");
    let plan = r#"{"tasks": [
      {"id": "t1", "title": "One", "prompt": "Write a.txt", "writes": true},
      {"id": "t2", "title": "Two", "prompt": "Write a.txt too", "writes": true}
    ]}"#;
    let script = Script::new(vec![
        ("planner", vec![text(plan)]),
        ("worker-t1", vec![write("a.txt", "one\n"), text("Done.")]),
        ("worker-t2", vec![write("a.txt", "two\n"), text("Done.")]),
    ]);
    let runner = RuntimeAgentRunner::new(
        |request: &AgentRequest| script.client(request),
        RuntimePermissionRuleConfig::default(),
    );
    let recorder = Recorder::new(&repo.repo, ColonyAnswer::Continue, ColonyAnswer::Merge);
    let config = config(
        &repo,
        ColonyLimits {
            max_workers: 2,
            max_tokens_per_run: 2_000_000,
        },
    );
    let handle = ColonyHandle::new(config.token_cap());

    let report = run_colony(&config, &runner, &recorder, &handle);

    assert_eq!(report.status, "conflict", "{report:#?}");
    let conflict = report.conflict.as_ref().expect("a conflict");
    assert_eq!(conflict.files, vec!["a.txt".to_string()]);
    assert_eq!(Some(&conflict.branch), report.tasks[1].branch.as_ref());
    assert!(!report.merged);
    assert!(
        !repo.repo.join("a.txt").exists(),
        "the tree stays untouched"
    );
    assert!(recorder
        .questions
        .lock()
        .unwrap()
        .iter()
        .all(|question| !matches!(question, ColonyQuestion::Merge { .. })));
    // The branches stay for the user.
    assert!(repo.branches().len() >= 2, "{:?}", repo.branches());
}

#[test]
fn free_runs_one_worker_at_a_time_and_stops_when_asked_near_the_cap() {
    let repo = TempRepo::new("free");
    let script = Script::new(vec![
        ("planner", vec![text(THREE_TASKS)]),
        (
            "worker-t1",
            vec![write("a.txt", "alpha\n"), text("Wrote a.txt.")],
        ),
        (
            "worker-t2",
            vec![write("b.txt", "beta\n"), text("Wrote b.txt.")],
        ),
        ("worker-t3", vec![text("README.md says hello.")]),
    ]);
    let runner = RuntimeAgentRunner::new(
        |request: &AgentRequest| script.client(request),
        RuntimePermissionRuleConfig::default(),
    );
    let recorder = Recorder::new(&repo.repo, ColonyAnswer::Stop, ColonyAnswer::Merge);
    let mut config = config(&repo, ColonyLimits::FREE);
    // 6,000 tokens: the planner and two workers reach 5,000 (over 80%).
    config.token_cap = Some(6_000);
    let handle = ColonyHandle::new(config.token_cap());

    let report = run_colony(&config, &runner, &recorder, &handle);

    assert_eq!(
        script.peak.load(Ordering::SeqCst),
        1,
        "Free runs workers in turn"
    );
    let questions = recorder.questions.lock().unwrap().clone();
    assert_eq!(questions.len(), 2, "{questions:?}");
    assert!(matches!(
        &questions[0],
        ColonyQuestion::Start { tasks, workers: 1, token_cap: 6_000, allow_web: false }
            if tasks.len() == 3
    ));
    assert_eq!(
        questions[1],
        ColonyQuestion::NearCap {
            used: 5_000,
            limit: 6_000
        }
    );
    assert_eq!(report.status, "stopped", "{report:#?}");
    assert_eq!(report.tasks[2].status, AgentStatus::Stopped);
    assert!(!report.merged);
    assert!(report.tokens_used <= 6_000);
    // The third worker never reached the provider.
    let seen = script.seen.lock().unwrap().clone();
    assert!(!seen.iter().any(|(id, _, _)| id == "worker-t3"), "{seen:?}");
}

#[test]
fn a_read_only_session_turns_every_subtask_read_only() {
    let repo = TempRepo::new("readonly");
    let script = Script::new(vec![
        ("planner", vec![text(THREE_TASKS)]),
        ("worker-t1", vec![text("Would write a.txt.")]),
        ("worker-t2", vec![text("Would write b.txt.")]),
        ("worker-t3", vec![text("README.md says hello.")]),
    ]);
    let runner = RuntimeAgentRunner::new(
        |request: &AgentRequest| script.client(request),
        RuntimePermissionRuleConfig::default(),
    );
    let recorder = Recorder::new(&repo.repo, ColonyAnswer::Continue, ColonyAnswer::Merge);
    let mut config = config(&repo, ColonyLimits::FREE);
    config.permission_mode = PermissionMode::ReadOnly;
    let handle = ColonyHandle::new(config.token_cap());

    let report = run_colony(&config, &runner, &recorder, &handle);

    assert_eq!(report.status, "completed", "{report:#?}");
    assert!(report.tasks.iter().all(|task| !task.writes));
    assert!(repo.branches().is_empty());
    assert!(script
        .seen
        .lock()
        .unwrap()
        .iter()
        .all(|(_, _, mode)| *mode == PermissionMode::ReadOnly));
}

#[test]
fn plans_are_read_from_fenced_or_bare_json_and_capped() {
    let tasks = parse_plan(THREE_TASKS, 6).expect("a plan");
    assert_eq!(tasks.len(), 3);
    assert!(tasks[0].writes && !tasks[2].writes);
    assert_eq!(tasks[0].files, vec!["a.txt".to_string()]);
    let many = format!(
        r#"{{"tasks": [{}]}}"#,
        (0..9)
            .map(|index| format!(r#"{{"title": "T{index}", "prompt": "P"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let capped = parse_plan(&many, 6).expect("a plan");
    assert_eq!(capped.len(), 6);
    assert_eq!(capped[5].id, "t6");
    assert!(parse_plan("no plan here", 6).is_err());
    assert!(parse_plan(r#"{"tasks": [{"title": "x"}]}"#, 6).is_err());
    assert!(parse_plan(r#"{"tasks": []}"#, 6).is_err());
}

#[test]
fn account_limits_fall_back_to_free() {
    let pro = serde_json::json!({
        "limits": {"colony": {"max_workers": 4, "max_tokens_per_run": 2_000_000}}
    });
    assert_eq!(
        ColonyLimits::from_account(&pro),
        ColonyLimits {
            max_workers: 4,
            max_tokens_per_run: 2_000_000
        }
    );
    assert_eq!(
        ColonyLimits::from_account(&serde_json::json!({})),
        ColonyLimits::FREE
    );
    let odd =
        serde_json::json!({"limits": {"colony": {"max_workers": 0, "max_tokens_per_run": "x"}}});
    assert_eq!(ColonyLimits::from_account(&odd), ColonyLimits::FREE);
    let huge =
        serde_json::json!({"limits": {"colony": {"max_workers": 99, "max_tokens_per_run": 1}}});
    assert_eq!(
        ColonyLimits::from_account(&huge).max_workers,
        super::MAX_COLONY_WORKERS
    );
}

#[test]
fn declining_the_plan_runs_nothing_but_the_planner() {
    let repo = TempRepo::new("decline");
    let script = Script::new(vec![("planner", vec![text(THREE_TASKS)])]);
    let runner = RuntimeAgentRunner::new(
        |request: &AgentRequest| script.client(request),
        RuntimePermissionRuleConfig::default(),
    );
    let mut recorder = Recorder::new(&repo.repo, ColonyAnswer::Continue, ColonyAnswer::Merge);
    recorder.start = ColonyAnswer::Stop;
    let config = config(&repo, ColonyLimits::FREE);
    let handle = ColonyHandle::new(config.token_cap());

    let report = run_colony(&config, &runner, &recorder, &handle);

    assert_eq!(report.status, "stopped");
    assert!(report.tasks.is_empty());
    let seen = script.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert!(repo.branches().is_empty());
}
