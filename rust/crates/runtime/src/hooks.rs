// Output to the user goes through crate::ui_hooks, so a full-screen UI can own the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::ffi::OsStr;
use std::fmt::Write as FmtWrite;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
use crate::hook_matcher::HookCommand;
use crate::permissions::PermissionOverride;

const HOOK_PREVIEW_CHAR_LIMIT: usize = 160;

pub type HookPermissionDecision = PermissionOverride;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    /// A session starts, resumes, is cleared or continues after compaction.
    SessionStart,
    /// The user submitted a prompt; may block it or add context.
    UserPromptSubmit,
    /// The model finished a turn; may ask it to keep going.
    Stop,
    /// The transcript is about to be compacted.
    PreCompact,
    /// A sub-agent finished.
    SubagentStop,
}

impl HookEvent {
    /// Every event, in the order settings and `/hooks` list them.
    pub const ALL: [Self; 8] = [
        Self::PreToolUse,
        Self::PostToolUse,
        Self::PostToolUseFailure,
        Self::SessionStart,
        Self::UserPromptSubmit,
        Self::Stop,
        Self::PreCompact,
        Self::SubagentStop,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Stop => "Stop",
            Self::PreCompact => "PreCompact",
            Self::SubagentStop => "SubagentStop",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|event| event.as_str() == name)
    }

    /// What a matcher is compared with: the tool name, the session source
    /// (`startup`, `resume`, `clear`, `compact`) or the compaction trigger
    /// (`manual`, `auto`). The other events run every hook.
    #[must_use]
    pub fn matcher_target(self) -> &'static str {
        match self {
            Self::PreToolUse | Self::PostToolUse | Self::PostToolUseFailure => "tool name",
            Self::SessionStart => "session source",
            Self::PreCompact => "compaction trigger",
            Self::UserPromptSubmit | Self::Stop | Self::SubagentStop => "",
        }
    }
}

/// How one hook command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// Exit 0: allowed, maybe with output.
    Ok,
    /// Exit 2 or a JSON `block`/`deny` decision.
    Blocked,
    /// Any other exit, a signal, output that is not valid hook JSON, or a failed start.
    Error,
}

impl HookOutcome {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }
}

/// Progress of hook commands, for the terminal spinner and the full-screen UI.
/// `tool_name` is the tool for tool events and the matcher target (session
/// source, compaction trigger) or empty for the others. `command` is the
/// command as configured: show it to the user only, shortened, never to the
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookProgressEvent {
    Started {
        event: HookEvent,
        tool_name: String,
        command: String,
        matcher: Option<String>,
        tool_use_id: Option<String>,
    },
    Completed {
        event: HookEvent,
        tool_name: String,
        command: String,
        matcher: Option<String>,
        tool_use_id: Option<String>,
        outcome: HookOutcome,
        duration_ms: u64,
        /// The hook's first message, shortened.
        message: Option<String>,
    },
    Cancelled {
        event: HookEvent,
        tool_name: String,
        command: String,
        matcher: Option<String>,
        tool_use_id: Option<String>,
        duration_ms: u64,
    },
}

impl HookProgressEvent {
    #[must_use]
    pub fn event(&self) -> HookEvent {
        match self {
            Self::Started { event, .. }
            | Self::Completed { event, .. }
            | Self::Cancelled { event, .. } => *event,
        }
    }

    /// The same event for the tool call `id`.
    #[must_use]
    pub fn with_tool_use_id(mut self, id: Option<&str>) -> Self {
        let slot = match &mut self {
            Self::Started { tool_use_id, .. }
            | Self::Completed { tool_use_id, .. }
            | Self::Cancelled { tool_use_id, .. } => tool_use_id,
        };
        *slot = id.map(ToOwned::to_owned);
        self
    }
}

pub trait HookProgressReporter {
    fn on_event(&mut self, event: &HookProgressEvent);
}

/// Passes events on with the tool call's id filled in.
pub struct ToolUseIdReporter<'a> {
    inner: &'a mut dyn HookProgressReporter,
    tool_use_id: Option<&'a str>,
}

impl<'a> ToolUseIdReporter<'a> {
    pub fn new(inner: &'a mut dyn HookProgressReporter, tool_use_id: Option<&'a str>) -> Self {
        Self { inner, tool_use_id }
    }
}

impl HookProgressReporter for ToolUseIdReporter<'_> {
    fn on_event(&mut self, event: &HookProgressEvent) {
        self.inner
            .on_event(&event.clone().with_tool_use_id(self.tool_use_id));
    }
}

/// Input of a lifecycle hook ([`HookRunner::run_lifecycle`]): what its matchers
/// compare with, and its event's fields of the JSON on stdin.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LifecycleHookInput {
    target: Option<String>,
    fields: serde_json::Map<String, Value>,
}

impl LifecycleHookInput {
    fn new(target: Option<&str>) -> Self {
        Self {
            target: target.map(ToOwned::to_owned),
            fields: serde_json::Map::new(),
        }
    }

    #[must_use]
    pub fn field(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.fields.insert(key.to_string(), value.into());
        self
    }

    /// `source` is `startup`, `resume`, `clear` or `compact`.
    #[must_use]
    pub fn session_start(source: &str) -> Self {
        Self::new(Some(source)).field("source", source)
    }

    #[must_use]
    pub fn user_prompt_submit(prompt: &str) -> Self {
        Self::new(None).field("prompt", prompt)
    }

    /// `stop_hook_active` is true when the turn already continued because of a
    /// Stop hook, so a hook can avoid asking forever.
    #[must_use]
    pub fn stop(stop_hook_active: bool, last_assistant_message: &str) -> Self {
        Self::new(None)
            .field("stop_hook_active", stop_hook_active)
            .field("last_assistant_message", last_assistant_message)
    }

    /// `trigger` is `manual` (`/compact`) or `auto`.
    #[must_use]
    pub fn pre_compact(trigger: &str) -> Self {
        Self::new(Some(trigger)).field("trigger", trigger)
    }

    #[must_use]
    pub fn subagent_stop(agent_id: &str, agent_type: &str, status: &str, result: &str) -> Self {
        Self::new(None)
            .field("agent_id", agent_id)
            .field("agent_type", agent_type)
            .field("status", status)
            .field("result", result)
            .field("stop_hook_active", false)
    }

    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }
}
#[derive(Debug, Clone, Default)]
pub struct HookAbortSignal {
    aborted: Arc<AtomicBool>,
}

impl HookAbortSignal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRunResult {
    denied: bool,
    failed: bool,
    cancelled: bool,
    messages: Vec<String>,
    permission_override: Option<PermissionOverride>,
    permission_reason: Option<String>,
    updated_input: Option<String>,
}

impl HookRunResult {
    #[must_use]
    pub fn allow(messages: Vec<String>) -> Self {
        Self {
            denied: false,
            failed: false,
            cancelled: false,
            messages,
            permission_override: None,
            permission_reason: None,
            updated_input: None,
        }
    }

    #[must_use]
    pub fn is_denied(&self) -> bool {
        self.denied
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    #[must_use]
    pub fn permission_override(&self) -> Option<PermissionOverride> {
        self.permission_override
    }

    #[must_use]
    pub fn permission_decision(&self) -> Option<HookPermissionDecision> {
        self.permission_override
    }

    #[must_use]
    pub fn permission_reason(&self) -> Option<&str> {
        self.permission_reason.as_deref()
    }

    #[must_use]
    pub fn updated_input(&self) -> Option<&str> {
        self.updated_input.as_deref()
    }

    #[must_use]
    pub fn updated_input_json(&self) -> Option<&str> {
        self.updated_input()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookRunner {
    config: RuntimeHookConfig,
    session_id: Option<String>,
}

/// What one run of an event's hooks is about.
struct HookCall<'a> {
    event: HookEvent,
    /// Compared with each hook's matcher.
    target: Option<&'a str>,
    /// The tool, or the matcher target, for progress and diagnostics.
    tool_name: &'a str,
    /// How messages name what the hook was handling, such as "`bash`".
    subject: String,
    payload: Value,
    env: Vec<(&'static str, String)>,
}

impl HookRunner {
    #[must_use]
    pub fn new(config: RuntimeHookConfig) -> Self {
        Self {
            config,
            session_id: None,
        }
    }

    #[must_use]
    pub fn from_feature_config(feature_config: &RuntimeFeatureConfig) -> Self {
        Self::new(feature_config.hooks().clone())
    }

    #[must_use]
    pub fn config(&self) -> &RuntimeHookConfig {
        &self.config
    }

    /// The session id hooks receive as `session_id`.
    pub fn set_session_id(&mut self, session_id: impl Into<String>) {
        self.session_id = Some(session_id.into());
    }

    /// Whether any hook is configured for `event` (whatever its matcher).
    #[must_use]
    pub fn has_hooks(&self, event: HookEvent) -> bool {
        self.config.has(event)
    }

    #[must_use]
    pub fn run_pre_tool_use(&self, tool_name: &str, tool_input: &str) -> HookRunResult {
        self.run_pre_tool_use_with_context(tool_name, tool_input, None, None)
    }

    #[must_use]
    pub fn run_pre_tool_use_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_tool_event(
            HookEvent::PreToolUse,
            tool_name,
            tool_input,
            None,
            false,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_pre_tool_use_with_signal(
        &self,
        tool_name: &str,
        tool_input: &str,
        abort_signal: Option<&HookAbortSignal>,
    ) -> HookRunResult {
        self.run_pre_tool_use_with_context(tool_name, tool_input, abort_signal, None)
    }

    #[must_use]
    pub fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_output: &str,
        is_error: bool,
    ) -> HookRunResult {
        self.run_post_tool_use_with_context(
            tool_name,
            tool_input,
            tool_output,
            is_error,
            None,
            None,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_output: &str,
        is_error: bool,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_tool_event(
            HookEvent::PostToolUse,
            tool_name,
            tool_input,
            Some(tool_output),
            is_error,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_with_signal(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_output: &str,
        is_error: bool,
        abort_signal: Option<&HookAbortSignal>,
    ) -> HookRunResult {
        self.run_post_tool_use_with_context(
            tool_name,
            tool_input,
            tool_output,
            is_error,
            abort_signal,
            None,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_error: &str,
    ) -> HookRunResult {
        self.run_post_tool_use_failure_with_context(tool_name, tool_input, tool_error, None, None)
    }

    #[must_use]
    pub fn run_post_tool_use_failure_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_error: &str,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_tool_event(
            HookEvent::PostToolUseFailure,
            tool_name,
            tool_input,
            Some(tool_error),
            true,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_failure_with_signal(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_error: &str,
        abort_signal: Option<&HookAbortSignal>,
    ) -> HookRunResult {
        self.run_post_tool_use_failure_with_context(
            tool_name,
            tool_input,
            tool_error,
            abort_signal,
            None,
        )
    }

    /// Runs a lifecycle event's hooks (`SessionStart`, `UserPromptSubmit`, `Stop`,
    /// `PreCompact`, `SubagentStop`). The result is denied when a hook blocked (exit 2,
    /// or `"decision": "block"` / `"continue": false` on stdout); its messages are
    /// the hooks' output (the context to add, or the reason).
    #[must_use]
    pub fn run_lifecycle(
        &self,
        event: HookEvent,
        input: LifecycleHookInput,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        let mut payload = serde_json::Map::new();
        payload.insert("hook_event_name".into(), event.as_str().into());
        for (key, value) in input.fields {
            payload.insert(key, value);
        }
        let subject = match event {
            HookEvent::SessionStart => "the session start",
            HookEvent::UserPromptSubmit => "the prompt",
            HookEvent::Stop => "the end of the turn",
            HookEvent::PreCompact => "compaction",
            HookEvent::SubagentStop => "the sub-agent's end",
            HookEvent::PreToolUse | HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                "the tool call"
            }
        };
        let target = input.target.as_deref();
        self.run_commands(
            HookCall {
                event,
                target,
                tool_name: target.unwrap_or(""),
                subject: subject.to_string(),
                payload: Value::Object(payload),
                env: Vec::new(),
            },
            abort_signal,
            reporter,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn run_tool_event(
        &self,
        event: HookEvent,
        tool_name: &str,
        tool_input: &str,
        tool_output: Option<&str>,
        is_error: bool,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        let mut env = vec![
            ("HOOK_TOOL_NAME", tool_name.to_string()),
            ("HOOK_TOOL_INPUT", tool_input.to_string()),
            (
                "HOOK_TOOL_IS_ERROR",
                if is_error { "1" } else { "0" }.to_string(),
            ),
        ];
        if let Some(tool_output) = tool_output {
            env.push(("HOOK_TOOL_OUTPUT", tool_output.to_string()));
        }
        self.run_commands(
            HookCall {
                event,
                target: Some(tool_name),
                tool_name,
                subject: format!("`{tool_name}`"),
                payload: hook_payload(event, tool_name, tool_input, tool_output, is_error),
                env,
            },
            abort_signal,
            reporter,
        )
    }

    fn run_commands(
        &self,
        call: HookCall<'_>,
        abort_signal: Option<&HookAbortSignal>,
        mut reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        let event = call.event;
        let commands: Vec<&HookCommand> = self
            .config
            .commands(event)
            .into_iter()
            .filter(|command| command.matches(call.target))
            .collect();
        if commands.is_empty() {
            return HookRunResult::allow(Vec::new());
        }

        if abort_signal.is_some_and(HookAbortSignal::is_aborted) {
            return HookRunResult {
                denied: false,
                failed: false,
                cancelled: true,
                messages: vec![format!(
                    "{} hook cancelled before execution",
                    event.as_str()
                )],
                permission_override: None,
                permission_reason: None,
                updated_input: None,
            };
        }

        let mut payload = call.payload.clone();
        if let Value::Object(fields) = &mut payload {
            if let Some(session_id) = &self.session_id {
                fields
                    .entry("session_id")
                    .or_insert_with(|| session_id.clone().into());
            }
            if let Ok(cwd) = std::env::current_dir() {
                fields
                    .entry("cwd")
                    .or_insert_with(|| cwd.display().to_string().into());
            }
        }
        let payload = payload.to_string();
        let mut result = HookRunResult::allow(Vec::new());

        for (index, hook) in commands.iter().enumerate() {
            let command = &hook.command;
            // Hook messages reach the model, so hooks are named by event and
            // position; their commands (which can embed tokens) and stderr
            // stay on the user's terminal.
            let label = format!("{} hook #{}", event.as_str(), index + 1);
            if let Some(reporter) = reporter.as_deref_mut() {
                reporter.on_event(&HookProgressEvent::Started {
                    event,
                    tool_name: call.tool_name.to_string(),
                    command: command.clone(),
                    matcher: hook.matcher.clone(),
                    tool_use_id: None,
                });
            }
            let started = Instant::now();
            let outcome = Self::run_command(command, &label, &call, &payload, abort_signal);
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            let completed =
                |outcome: HookOutcome, parsed: &ParsedHookOutput| HookProgressEvent::Completed {
                    event,
                    tool_name: call.tool_name.to_string(),
                    command: command.clone(),
                    matcher: hook.matcher.clone(),
                    tool_use_id: None,
                    outcome,
                    duration_ms,
                    message: parsed.primary_message().and_then(bounded_hook_preview),
                };

            match outcome {
                HookCommandOutcome::Allow { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&completed(HookOutcome::Ok, &parsed));
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                }
                HookCommandOutcome::Deny { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&completed(HookOutcome::Blocked, &parsed));
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                    result.denied = true;
                    return result;
                }
                HookCommandOutcome::Failed { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&completed(HookOutcome::Error, &parsed));
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                    result.failed = true;
                    return result;
                }
                HookCommandOutcome::Cancelled { message } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&HookProgressEvent::Cancelled {
                            event,
                            tool_name: call.tool_name.to_string(),
                            command: command.clone(),
                            matcher: hook.matcher.clone(),
                            tool_use_id: None,
                            duration_ms,
                        });
                    }
                    result.cancelled = true;
                    result.messages.push(message);
                    return result;
                }
            }
        }

        result
    }

    fn run_command(
        command: &str,
        label: &str,
        call: &HookCall<'_>,
        payload: &str,
        abort_signal: Option<&HookAbortSignal>,
    ) -> HookCommandOutcome {
        let event = call.event;
        let subject = &call.subject;
        let mut child = shell_command(command);
        child.stdin(Stdio::piped());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        child.env("HOOK_EVENT", event.as_str());
        for (key, value) in &call.env {
            child.env(key, value);
        }

        match child.output_with_stdin(payload.as_bytes(), abort_signal) {
            Ok(CommandExecution::Finished(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let parsed = parse_hook_output(event, call.tool_name, label, &stdout);
                // A failing exit is reported below, with the same details.
                if parsed.invalid_output && matches!(output.status.code(), Some(0 | 2)) {
                    report_hook_locally(
                        label,
                        command,
                        "printed output that is not valid hook JSON",
                        &stderr,
                    );
                }
                let primary_message = parsed.primary_message().map(ToOwned::to_owned);
                match output.status.code() {
                    Some(0) => {
                        if parsed.deny {
                            HookCommandOutcome::Deny { parsed }
                        } else {
                            HookCommandOutcome::Allow { parsed }
                        }
                    }
                    Some(2) => {
                        // Exit 2 blocks. Like stdout on other exits, stderr stays on
                        // the user's terminal: the reason the model sees is stdout's.
                        let fallback = if is_tool_event(event) {
                            format!("{} hook denied tool {subject}", event.as_str())
                        } else {
                            format!("{} hook blocked {subject}", event.as_str())
                        };
                        HookCommandOutcome::Deny {
                            parsed: parsed.with_fallback_message(fallback),
                        }
                    }
                    Some(code) => {
                        report_hook_locally(
                            label,
                            command,
                            &format!("exited with status {code}"),
                            &stderr,
                        );
                        HookCommandOutcome::Failed {
                            parsed: parsed.with_fallback_message(format_hook_failure(
                                label,
                                code,
                                primary_message.as_deref(),
                            )),
                        }
                    }
                    None => {
                        report_hook_locally(label, command, "was terminated by a signal", &stderr);
                        HookCommandOutcome::Failed {
                            parsed: parsed.with_fallback_message(format!(
                                "{label} terminated by signal while handling {subject}"
                            )),
                        }
                    }
                }
            }
            Ok(CommandExecution::Cancelled) => HookCommandOutcome::Cancelled {
                message: format!("{label} cancelled while handling {subject}"),
            },
            Err(error) => {
                report_hook_locally(label, command, &format!("failed to start: {error}"), "");
                HookCommandOutcome::Failed {
                    parsed: ParsedHookOutput {
                        messages: vec![format!("{label} failed to start for {subject}: {error}")],
                        ..ParsedHookOutput::default()
                    },
                }
            }
        }
    }
}

fn is_tool_event(event: HookEvent) -> bool {
    matches!(
        event,
        HookEvent::PreToolUse | HookEvent::PostToolUse | HookEvent::PostToolUseFailure
    )
}
enum HookCommandOutcome {
    Allow { parsed: ParsedHookOutput },
    Deny { parsed: ParsedHookOutput },
    Failed { parsed: ParsedHookOutput },
    Cancelled { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ParsedHookOutput {
    messages: Vec<String>,
    /// Stdout looked like JSON but did not parse as a hook response.
    invalid_output: bool,
    deny: bool,
    permission_override: Option<PermissionOverride>,
    permission_reason: Option<String>,
    updated_input: Option<String>,
}

impl ParsedHookOutput {
    fn with_fallback_message(mut self, fallback: String) -> Self {
        if self.messages.is_empty() {
            self.messages.push(fallback);
        }
        self
    }

    fn primary_message(&self) -> Option<&str> {
        self.messages.first().map(String::as_str)
    }
}

fn merge_parsed_hook_output(target: &mut HookRunResult, parsed: ParsedHookOutput) {
    target.messages.extend(parsed.messages);
    if parsed.permission_override.is_some() {
        target.permission_override = parsed.permission_override;
    }
    if parsed.permission_reason.is_some() {
        target.permission_reason = parsed.permission_reason;
    }
    if parsed.updated_input.is_some() {
        target.updated_input = parsed.updated_input;
    }
}

fn parse_hook_output(
    event: HookEvent,
    tool_name: &str,
    label: &str,
    stdout: &str,
) -> ParsedHookOutput {
    if stdout.is_empty() {
        return ParsedHookOutput::default();
    }

    let root = match serde_json::from_str::<Value>(stdout) {
        Ok(Value::Object(root)) => root,
        Ok(value) => {
            return ParsedHookOutput {
                messages: vec![format_invalid_hook_output(
                    event,
                    tool_name,
                    label,
                    &format!(
                        "expected top-level JSON object, got {}",
                        json_type_name(&value)
                    ),
                    stdout,
                )],
                invalid_output: true,
                ..ParsedHookOutput::default()
            };
        }
        Err(error) if looks_like_json_attempt(stdout) => {
            return ParsedHookOutput {
                messages: vec![format_invalid_hook_output(
                    event,
                    tool_name,
                    label,
                    &error.to_string(),
                    stdout,
                )],
                invalid_output: true,
                ..ParsedHookOutput::default()
            };
        }
        Err(_) => {
            return ParsedHookOutput {
                messages: vec![stdout.to_string()],
                ..ParsedHookOutput::default()
            };
        }
    };

    let mut parsed = ParsedHookOutput::default();

    if let Some(message) = root.get("systemMessage").and_then(Value::as_str) {
        parsed.messages.push(message.to_string());
    }
    if let Some(message) = root.get("reason").and_then(Value::as_str) {
        parsed.messages.push(message.to_string());
    }
    if root.get("continue").and_then(Value::as_bool) == Some(false)
        || root.get("decision").and_then(Value::as_str) == Some("block")
    {
        parsed.deny = true;
    }

    if let Some(Value::Object(specific)) = root.get("hookSpecificOutput") {
        if let Some(Value::String(additional_context)) = specific.get("additionalContext") {
            parsed.messages.push(additional_context.clone());
        }
        if let Some(decision) = specific.get("permissionDecision").and_then(Value::as_str) {
            parsed.permission_override = match decision {
                "allow" => Some(PermissionOverride::Allow),
                "deny" => Some(PermissionOverride::Deny),
                "ask" => Some(PermissionOverride::Ask),
                _ => None,
            };
        }
        if let Some(reason) = specific
            .get("permissionDecisionReason")
            .and_then(Value::as_str)
        {
            parsed.permission_reason = Some(reason.to_string());
        }
        if let Some(updated_input) = specific.get("updatedInput") {
            parsed.updated_input = serde_json::to_string(updated_input).ok();
        }
    }

    if parsed.messages.is_empty() {
        parsed.messages.push(stdout.to_string());
    }

    parsed
}

fn hook_payload(
    event: HookEvent,
    tool_name: &str,
    tool_input: &str,
    tool_output: Option<&str>,
    is_error: bool,
) -> Value {
    match event {
        HookEvent::PostToolUseFailure => json!({
            "hook_event_name": event.as_str(),
            "tool_name": tool_name,
            "tool_input": parse_tool_input(tool_input),
            "tool_input_json": tool_input,
            "tool_error": tool_output,
            "tool_result_is_error": true,
        }),
        _ => json!({
            "hook_event_name": event.as_str(),
            "tool_name": tool_name,
            "tool_input": parse_tool_input(tool_input),
            "tool_input_json": tool_input,
            "tool_output": tool_output,
            "tool_result_is_error": is_error,
        }),
    }
}

fn parse_tool_input(tool_input: &str) -> Value {
    serde_json::from_str(tool_input).unwrap_or_else(|_| json!({ "raw": tool_input }))
}

fn format_invalid_hook_output(
    event: HookEvent,
    tool_name: &str,
    label: &str,
    detail: &str,
    stdout: &str,
) -> String {
    let stdout_preview = bounded_hook_preview(stdout).unwrap_or_else(|| "<empty>".to_string());

    format!(
        "hook_invalid_json: phase={} tool={} hook={label} detail={} stdout_preview={}",
        event.as_str(),
        tool_name,
        detail,
        stdout_preview,
    )
}

/// Tells the user, not the model, which hook command misbehaved and what it
/// wrote to stderr.
fn report_hook_locally(label: &str, command: &str, problem: &str, stderr: &str) {
    let command = bounded_hook_preview(command).unwrap_or_else(|| "<empty>".to_string());
    match bounded_hook_preview(stderr) {
        Some(stderr) => crate::ui_hooks::notice(&format!(
            "warning: {label} `{command}` {problem}; stderr: {stderr}"
        )),
        None => crate::ui_hooks::notice(&format!("warning: {label} `{command}` {problem}")),
    }
}

fn bounded_hook_preview(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut preview = String::new();
    for (count, ch) in trimmed.chars().enumerate() {
        if count == HOOK_PREVIEW_CHAR_LIMIT {
            preview.push('…');
            break;
        }
        match ch {
            '\n' => preview.push_str("\\n"),
            '\r' => preview.push_str("\\r"),
            '\t' => preview.push_str("\\t"),
            control if control.is_control() => {
                let _ = write!(&mut preview, "\\u{{{:x}}}", control as u32);
            }
            _ => preview.push(ch),
        }
    }
    Some(preview)
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn looks_like_json_attempt(value: &str) -> bool {
    matches!(value.trim_start().chars().next(), Some('{' | '['))
}

fn format_hook_failure(label: &str, code: i32, stdout: Option<&str>) -> String {
    let mut message = format!("{label} exited with status {code}");
    if let Some(stdout) = stdout.filter(|stdout| !stdout.is_empty()) {
        message.push_str(": ");
        message.push_str(stdout);
    }
    message
}

fn shell_command(command: &str) -> CommandWithStdin {
    #[cfg(windows)]
    let command_builder = {
        use std::os::windows::process::CommandExt;

        // `cmd /d /s /c "<command>"` with the command line as written: Rust's own
        // quoting escapes inner quotes as `\"`, which cmd.exe does not understand, so a
        // command with a quoted path always failed (and a failing PreToolUse hook
        // denies every tool call).
        let mut command_builder = plugins::subprocess::windows_cmd_command(command);
        // Not in the user's console, whose title `cmd.exe` would change.
        command_builder.creation_flags(plugins::subprocess::CREATE_NO_WINDOW);
        CommandWithStdin::new(command_builder)
    };

    #[cfg(not(windows))]
    let command_builder = {
        let mut command_builder = Command::new("sh");
        command_builder.arg("-lc").arg(command);
        CommandWithStdin::new(command_builder)
    };

    command_builder
}

struct CommandWithStdin {
    command: Command,
}

impl CommandWithStdin {
    fn new(command: Command) -> Self {
        Self { command }
    }

    fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.env(key, value);
        self
    }

    fn output_with_stdin(
        &mut self,
        stdin: &[u8],
        abort_signal: Option<&HookAbortSignal>,
    ) -> std::io::Result<CommandExecution> {
        let mut child = self.command.spawn()?;
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin.write_all(stdin)?;
        }

        loop {
            if abort_signal.is_some_and(HookAbortSignal::is_aborted) {
                let _ = child.kill();
                let _ = child.wait_with_output();
                return Ok(CommandExecution::Cancelled);
            }

            match child.try_wait()? {
                Some(_) => return child.wait_with_output().map(CommandExecution::Finished),
                None => thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

enum CommandExecution {
    Finished(std::process::Output),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        HookAbortSignal, HookEvent, HookProgressEvent, HookProgressReporter, HookRunResult,
        HookRunner,
    };
    use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
    use crate::permissions::PermissionOverride;

    struct RecordingReporter {
        events: Vec<HookProgressEvent>,
    }

    impl HookProgressReporter for RecordingReporter {
        fn on_event(&mut self, event: &HookProgressEvent) {
            self.events.push(event.clone());
        }
    }

    #[test]
    #[cfg(unix)]
    fn allows_exit_code_zero_and_captures_stdout() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("printf 'pre ok'")],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Read", r#"{"path":"README.md"}"#);

        assert_eq!(result, HookRunResult::allow(vec!["pre ok".to_string()]));
    }

    #[test]
    #[cfg(unix)]
    fn denies_exit_code_two() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("printf 'blocked by hook'; exit 2")],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Bash", r#"{"command":"pwd"}"#);

        assert!(result.is_denied());
        assert_eq!(result.messages(), &["blocked by hook".to_string()]);
    }

    #[test]
    fn propagates_other_non_zero_statuses_as_failures() {
        let failure_hook = if cfg!(windows) {
            "echo warning hook & exit /b 1"
        } else {
            "printf 'warning hook'; exit 1"
        };
        let runner = HookRunner::from_feature_config(&RuntimeFeatureConfig::default().with_hooks(
            RuntimeHookConfig::new(vec![shell_snippet(failure_hook)], Vec::new(), Vec::new()),
        ));

        // given
        // when
        let result = runner.run_pre_tool_use("Edit", r#"{"file":"src/lib.rs"}"#);

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("warning hook")));
    }

    #[test]
    #[cfg(unix)]
    fn parses_pre_hook_permission_override_and_updated_input() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(
                r#"printf '%s' '{"systemMessage":"updated","hookSpecificOutput":{"permissionDecision":"allow","permissionDecisionReason":"hook ok","updatedInput":{"command":"git status"}}}'"#,
            )],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("bash", r#"{"command":"pwd"}"#);

        assert_eq!(
            result.permission_override(),
            Some(PermissionOverride::Allow)
        );
        assert_eq!(result.permission_reason(), Some("hook ok"));
        assert_eq!(result.updated_input(), Some(r#"{"command":"git status"}"#));
        assert!(result.messages().iter().any(|message| message == "updated"));
    }

    #[test]
    #[cfg(unix)]
    fn runs_post_tool_use_failure_hooks() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            Vec::new(),
            Vec::new(),
            vec![shell_snippet("printf 'failure hook ran'")],
        ));

        // when
        let result =
            runner.run_post_tool_use_failure("bash", r#"{"command":"false"}"#, "command failed");

        // then
        assert!(!result.is_denied());
        assert_eq!(result.messages(), &["failure hook ran".to_string()]);
    }

    #[test]
    fn stops_running_failure_hooks_after_failure() {
        // given
        let (broken_hook, later_hook) = if cfg!(windows) {
            (
                "echo broken failure hook & exit /b 1",
                "echo later failure hook",
            )
        } else {
            (
                "printf 'broken failure hook'; exit 1",
                "printf 'later failure hook'",
            )
        };
        let runner = HookRunner::new(RuntimeHookConfig::new(
            Vec::new(),
            Vec::new(),
            vec![shell_snippet(broken_hook), shell_snippet(later_hook)],
        ));

        // when
        let result =
            runner.run_post_tool_use_failure("bash", r#"{"command":"false"}"#, "command failed");

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("broken failure hook")));
        assert!(!result
            .messages()
            .iter()
            .any(|message| message == "later failure hook"));
    }

    #[test]
    #[cfg(unix)]
    fn executes_hooks_in_configured_order() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![
                shell_snippet("printf 'first'"),
                shell_snippet("printf 'second'"),
            ],
            Vec::new(),
            Vec::new(),
        ));
        let mut reporter = RecordingReporter { events: Vec::new() };

        // when
        let result = runner.run_pre_tool_use_with_context(
            "Read",
            r#"{"path":"README.md"}"#,
            None,
            Some(&mut reporter),
        );

        // then
        assert_eq!(
            result,
            HookRunResult::allow(vec!["first".to_string(), "second".to_string()])
        );
        assert_eq!(reporter.events.len(), 4);
        assert!(matches!(
            &reporter.events[0],
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == "printf 'first'"
        ));
        assert!(matches!(
            &reporter.events[1],
            HookProgressEvent::Completed {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == "printf 'first'"
        ));
        assert!(matches!(
            &reporter.events[2],
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == "printf 'second'"
        ));
        assert!(matches!(
            &reporter.events[3],
            HookProgressEvent::Completed {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == "printf 'second'"
        ));
    }

    #[test]
    fn stops_running_hooks_after_failure() {
        // given
        let (broken_hook, later_hook) = if cfg!(windows) {
            ("echo broken & exit /b 1", "echo later")
        } else {
            ("printf 'broken'; exit 1", "printf 'later'")
        };
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(broken_hook), shell_snippet(later_hook)],
            Vec::new(),
            Vec::new(),
        ));

        // when
        let result = runner.run_pre_tool_use("Edit", r#"{"file":"src/lib.rs"}"#);

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("broken")));
        assert!(!result.messages().iter().any(|message| message == "later"));
    }

    #[test]
    #[cfg(unix)]
    fn malformed_nonempty_hook_output_reports_explicit_diagnostic_with_previews() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(
                "printf '{not-json\nsecond line'; printf 'stderr warning' >&2; exit 1",
            )],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Edit", r#"{"file":"src/lib.rs"}"#);

        assert!(result.is_failed());
        let rendered = result.messages().join("\n");
        assert!(rendered.contains("hook_invalid_json:"));
        assert!(rendered.contains("phase=PreToolUse"));
        assert!(rendered.contains("tool=Edit"));
        assert!(rendered.contains("hook=PreToolUse hook #1"));
        assert!(rendered.contains("detail=key must be a string"));
        assert!(rendered.contains("stdout_preview={not-json"));
        assert!(!rendered.contains("printf"), "{rendered}");
        assert!(!rendered.contains("stderr warning"), "{rendered}");
    }

    #[test]
    fn hook_results_name_hooks_without_their_commands_or_stderr() {
        let (quiet, noisy) = if cfg!(windows) {
            (
                "exit /b 0 & rem Bearer secret-hook-token-1",
                "echo leaked-stderr-tok 1>&2 & exit /b 3 & rem Bearer secret-hook-token-2",
            )
        } else {
            (
                "exit 0 # Bearer secret-hook-token-1",
                "echo leaked-stderr-tok >&2; exit 3 # Bearer secret-hook-token-2",
            )
        };
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(quiet), shell_snippet(noisy)],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("bash", r#"{"command":"pwd"}"#);

        assert!(result.is_failed());
        let rendered = result.messages().join("\n");
        assert!(
            rendered.contains("PreToolUse hook #2 exited with status 3"),
            "{rendered}"
        );
        assert!(!rendered.contains("secret-hook-token"), "{rendered}");
        assert!(!rendered.contains("leaked-stderr-tok"), "{rendered}");
    }

    #[test]
    #[cfg(unix)]
    fn abort_signal_cancels_long_running_hook_and_reports_progress() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("sleep 5")],
            Vec::new(),
            Vec::new(),
        ));
        let abort_signal = HookAbortSignal::new();
        let abort_signal_for_thread = abort_signal.clone();
        let mut reporter = RecordingReporter { events: Vec::new() };

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            abort_signal_for_thread.abort();
        });

        let result = runner.run_pre_tool_use_with_context(
            "bash",
            r#"{"command":"sleep 5"}"#,
            Some(&abort_signal),
            Some(&mut reporter),
        );

        assert!(result.is_cancelled());
        assert!(reporter.events.iter().any(|event| matches!(
            event,
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                ..
            }
        )));
        assert!(reporter.events.iter().any(|event| matches!(
            event,
            HookProgressEvent::Cancelled {
                event: HookEvent::PreToolUse,
                ..
            }
        )));
    }

    /// A hook command that quotes paths with spaces in them, like
    /// `node "<dir>\hook.mjs" "<dir>\hooks.log"`: Rust's argument quoting turned its
    /// quotes into `\"`, which cmd.exe does not understand, so it always failed and
    /// the failing `PreToolUse` hook denied every tool call.
    #[test]
    #[cfg(windows)]
    fn a_hook_command_with_quoted_paths_runs_on_windows() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("neuron hook test {}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("hook dir");
        let script = dir.join("pre hook.cmd");
        std::fs::write(&script, "@echo off\r\necho hook saw %~1\r\nexit /b 0\r\n")
            .expect("hook script");
        let log = dir.join("hooks log.txt");
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![format!("\"{}\" \"{}\"", script.display(), log.display())],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("bash", r#"{"command":"pwd"}"#);

        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            result,
            HookRunResult::allow(vec![format!("hook saw {}", log.display())])
        );

        // A quoted program path, as written in settings.
        let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![format!(
                r#""{system_root}\System32\cmd.exe" /d /c echo quoted-exe-ok"#
            )],
            Vec::new(),
            Vec::new(),
        ));
        assert_eq!(
            runner.run_pre_tool_use("bash", r#"{"command":"pwd"}"#),
            HookRunResult::allow(vec!["quoted-exe-ok".to_string()])
        );
    }

    /// A hook that prints `text` and exits with `code`, in the platform's shell.
    fn echo_hook(text: &str, code: i32) -> String {
        if cfg!(windows) {
            format!("echo {text}& exit /b {code}")
        } else {
            format!("echo '{text}'; exit {code}")
        }
    }

    #[test]
    fn tool_hooks_run_only_for_matching_tools() {
        use crate::hook_matcher::{HookCommand, HookSource};
        let config = RuntimeHookConfig::default()
            .with(
                HookEvent::PreToolUse,
                HookCommand::new(echo_hook("any", 0), HookSource::User),
            )
            .with(
                HookEvent::PreToolUse,
                HookCommand::new(echo_hook("files", 0), HookSource::Project)
                    .with_matcher(Some("write_file|edit_file".into())),
            )
            .with(
                HookEvent::PreToolUse,
                HookCommand::new(echo_hook("mcp", 0), HookSource::Plugin)
                    .with_matcher(Some("mcp__.*".into())),
            );
        let runner = HookRunner::new(config);

        assert_eq!(runner.run_pre_tool_use("bash", "{}").messages(), ["any"]);
        assert_eq!(
            runner.run_pre_tool_use("edit_file", "{}").messages(),
            ["any", "files"]
        );
        assert_eq!(
            runner
                .run_pre_tool_use("mcp__github__issues", "{}")
                .messages(),
            ["any", "mcp"]
        );
    }

    #[test]
    fn progress_reports_outcome_duration_matcher_and_tool_call() {
        use super::{HookOutcome, ToolUseIdReporter};
        use crate::hook_matcher::{HookCommand, HookSource};
        let runner = HookRunner::new(
            RuntimeHookConfig::default().with(
                HookEvent::PreToolUse,
                HookCommand::new(echo_hook("not here", 2), HookSource::User)
                    .with_matcher(Some("bash".into())),
            ),
        );
        let mut recording = RecordingReporter { events: Vec::new() };
        let result = runner.run_pre_tool_use_with_context(
            "bash",
            "{}",
            None,
            Some(&mut ToolUseIdReporter::new(&mut recording, Some("call-7"))),
        );

        assert!(result.is_denied());
        assert_eq!(recording.events.len(), 2);
        let HookProgressEvent::Completed {
            outcome,
            matcher,
            tool_use_id,
            message,
            ..
        } = &recording.events[1]
        else {
            panic!("expected a completed event: {:?}", recording.events);
        };
        assert_eq!(*outcome, HookOutcome::Blocked);
        assert_eq!(matcher.as_deref(), Some("bash"));
        assert_eq!(tool_use_id.as_deref(), Some("call-7"));
        assert_eq!(message.as_deref(), Some("not here"));
    }

    #[test]
    fn lifecycle_hooks_get_the_event_payload_and_can_block() {
        use super::LifecycleHookInput;
        use crate::hook_matcher::{HookCommand, HookSource};
        // The hook echoes its stdin, so the payload comes back as its message.
        let cat = if cfg!(windows) { "more" } else { "cat" };
        let mut runner = HookRunner::new(RuntimeHookConfig::default().with(
            HookEvent::UserPromptSubmit,
            HookCommand::new(cat, HookSource::User),
        ));
        runner.set_session_id("session-1");
        let result = runner.run_lifecycle(
            HookEvent::UserPromptSubmit,
            LifecycleHookInput::user_prompt_submit("hello"),
            None,
            None,
        );
        assert!(!result.is_denied() && !result.is_failed(), "{result:?}");
        // Valid JSON without hook fields: the whole stdout is the message.
        let payload: serde_json::Value =
            serde_json::from_str(result.messages()[0].trim()).expect("payload JSON");
        assert_eq!(payload["hook_event_name"], "UserPromptSubmit");
        assert_eq!(payload["prompt"], "hello");
        assert_eq!(payload["session_id"], "session-1");
        assert!(payload["cwd"].is_string());

        let blocking = HookRunner::new(RuntimeHookConfig::default().with(
            HookEvent::Stop,
            HookCommand::new(
                if cfg!(windows) {
                    r#"echo {"decision":"block","reason":"run the tests"}"#.to_string()
                } else {
                    r#"echo '{"decision":"block","reason":"run the tests"}'"#.to_string()
                },
                HookSource::User,
            ),
        ));
        let stop = blocking.run_lifecycle(
            HookEvent::Stop,
            LifecycleHookInput::stop(false, "done"),
            None,
            None,
        );
        assert!(stop.is_denied());
        assert_eq!(stop.messages(), ["run the tests"]);
    }

    #[cfg(windows)]
    fn shell_snippet(script: &str) -> String {
        script.replace('\'', "\"")
    }

    #[cfg(not(windows))]
    fn shell_snippet(script: &str) -> String {
        script.to_string()
    }
}
