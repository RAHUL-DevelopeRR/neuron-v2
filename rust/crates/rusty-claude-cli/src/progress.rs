//! Internal prompt progress tracking and reporting.
//!
//! Progress UI for bughunter, ultraplan, commit, PR, and issue commands.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::*;
use crate::brand::*;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InternalPromptProgressState {
    pub(crate) command_label: &'static str,
    pub(crate) task_label: String,
    pub(crate) step: usize,
    pub(crate) phase: String,
    pub(crate) detail: Option<String>,
    pub(crate) saw_final_text: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InternalPromptProgressEvent {
    Started,
    Update,
    Heartbeat,
    Complete,
    Failed,
}

#[derive(Debug)]
pub(crate) struct InternalPromptProgressShared {
    state: Mutex<InternalPromptProgressState>,
    output_lock: Mutex<()>,
    started_at: Instant,
}

#[derive(Debug, Clone)]
pub(crate) struct InternalPromptProgressReporter {
    shared: Arc<InternalPromptProgressShared>,
}

#[derive(Debug)]
pub(crate) struct InternalPromptProgressRun {
    reporter: InternalPromptProgressReporter,
    heartbeat_stop: Option<mpsc::Sender<()>>,
    heartbeat_handle: Option<thread::JoinHandle<()>>,
}

impl InternalPromptProgressReporter {
    pub fn ultraplan(task: &str) -> Self {
        Self {
            shared: Arc::new(InternalPromptProgressShared {
                state: Mutex::new(InternalPromptProgressState {
                    command_label: "Ultraplan",
                    task_label: task.to_string(),
                    step: 0,
                    phase: "planning started".to_string(),
                    detail: Some(format!("task: {task}")),
                    saw_final_text: false,
                }),
                output_lock: Mutex::new(()),
                started_at: Instant::now(),
            }),
        }
    }

    pub fn emit(&self, event: InternalPromptProgressEvent, error: Option<&str>) {
        let snapshot = self.snapshot();
        let line = format_internal_prompt_progress_line(event, &snapshot, self.elapsed(), error);
        self.write_line(&line);
    }

    pub fn mark_model_phase(&self) {
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = if state.step == 1 {
                "analyzing request".to_string()
            } else {
                "reviewing findings".to_string()
            };
            state.detail = Some(format!("task: {}", state.task_label));
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    pub fn mark_tool_phase(&self, name: &str, input: &str) {
        let detail = describe_tool_progress(name, input);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            state.step += 1;
            state.phase = format!("running {name}");
            state.detail = Some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    pub fn mark_text_phase(&self, text: &str) {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let detail = truncate_for_summary(first_visible_line(trimmed), 120);
        let snapshot = {
            let mut state = self
                .shared
                .state
                .lock()
                .expect("internal prompt progress state poisoned");
            if state.saw_final_text {
                return;
            }
            state.saw_final_text = true;
            state.step += 1;
            state.phase = "drafting final plan".to_string();
            state.detail = (!detail.is_empty()).then_some(detail);
            state.clone()
        };
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Update,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    pub fn emit_heartbeat(&self) {
        let snapshot = self.snapshot();
        self.write_line(&format_internal_prompt_progress_line(
            InternalPromptProgressEvent::Heartbeat,
            &snapshot,
            self.elapsed(),
            None,
        ));
    }

    pub fn snapshot(&self) -> InternalPromptProgressState {
        self.shared
            .state
            .lock()
            .expect("internal prompt progress state poisoned")
            .clone()
    }

    pub fn elapsed(&self) -> Duration {
        self.shared.started_at.elapsed()
    }

    pub fn write_line(&self, line: &str) {
        let _guard = self
            .shared
            .output_lock
            .lock()
            .expect("internal prompt progress output lock poisoned");
        let mut stdout = io::stdout();
        let _ = writeln!(stdout, "{}", crate::console::stdout_text(line));
        let _ = stdout.flush();
    }
}

impl InternalPromptProgressRun {
    pub fn start_ultraplan(task: &str) -> Self {
        let reporter = InternalPromptProgressReporter::ultraplan(task);
        reporter.emit(InternalPromptProgressEvent::Started, None);

        let (heartbeat_stop, heartbeat_rx) = mpsc::channel();
        let heartbeat_reporter = reporter.clone();
        let heartbeat_handle = thread::spawn(move || loop {
            match heartbeat_rx.recv_timeout(INTERNAL_PROGRESS_HEARTBEAT_INTERVAL) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => heartbeat_reporter.emit_heartbeat(),
            }
        });

        Self {
            reporter,
            heartbeat_stop: Some(heartbeat_stop),
            heartbeat_handle: Some(heartbeat_handle),
        }
    }

    pub fn reporter(&self) -> InternalPromptProgressReporter {
        self.reporter.clone()
    }

    pub fn finish_success(&mut self) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Complete, None);
    }

    pub fn finish_failure(&mut self, error: &str) {
        self.stop_heartbeat();
        self.reporter
            .emit(InternalPromptProgressEvent::Failed, Some(error));
    }

    pub fn stop_heartbeat(&mut self) {
        if let Some(sender) = self.heartbeat_stop.take() {
            let _ = sender.send(());
        }
        if let Some(handle) = self.heartbeat_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for InternalPromptProgressRun {
    fn drop(&mut self) {
        self.stop_heartbeat();
    }
}

pub(crate) fn format_internal_prompt_progress_line(
    event: InternalPromptProgressEvent,
    snapshot: &InternalPromptProgressState,
    elapsed: Duration,
    error: Option<&str>,
) -> String {
    let elapsed_seconds = elapsed.as_secs();
    let step_label = if snapshot.step == 0 {
        "current step pending".to_string()
    } else {
        format!("current step {}", snapshot.step)
    };
    let mut status_bits = vec![step_label, format!("phase {}", snapshot.phase)];
    if let Some(detail) = snapshot
        .detail
        .as_deref()
        .filter(|detail| !detail.is_empty())
    {
        status_bits.push(detail.to_string());
    }
    let status = status_bits.join(" · ");
    match event {
        InternalPromptProgressEvent::Started => {
            format!(
                "🧭 {} status · planning started · {status}",
                snapshot.command_label
            )
        }
        InternalPromptProgressEvent::Update => {
            format!("… {} status · {status}", snapshot.command_label)
        }
        InternalPromptProgressEvent::Heartbeat => format!(
            "… {} heartbeat · {elapsed_seconds}s elapsed · {status}",
            snapshot.command_label
        ),
        InternalPromptProgressEvent::Complete => format!(
            "✔ {} status · completed · {elapsed_seconds}s elapsed · {} steps total",
            snapshot.command_label, snapshot.step
        ),
        InternalPromptProgressEvent::Failed => format!(
            "✘ {} status · failed · {elapsed_seconds}s elapsed · {}",
            snapshot.command_label,
            error.unwrap_or("unknown error")
        ),
    }
}

pub(crate) fn describe_tool_progress(name: &str, input: &str) -> String {
    let parsed: serde_json::Value =
        serde_json::from_str(input).unwrap_or(serde_json::Value::String(input.to_string()));
    match name {
        "bash" | "Bash" => {
            let command = parsed
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if command.is_empty() {
                "running shell command".to_string()
            } else {
                format!("command {}", truncate_for_summary(command.trim(), 100))
            }
        }
        "read_file" | "Read" => format!("reading {}", extract_tool_path(&parsed)),
        "write_file" | "Write" => format!("writing {}", extract_tool_path(&parsed)),
        "edit_file" | "Edit" => format!("editing {}", extract_tool_path(&parsed)),
        "glob_search" | "Glob" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("glob `{pattern}` in {scope}")
        }
        "grep_search" | "Grep" => {
            let pattern = parsed
                .get("pattern")
                .and_then(|value| value.as_str())
                .unwrap_or("?");
            let scope = parsed
                .get("path")
                .and_then(|value| value.as_str())
                .unwrap_or(".");
            format!("grep `{pattern}` in {scope}")
        }
        "web_search" | "WebSearch" => parsed
            .get("query")
            .and_then(|value| value.as_str())
            .map_or_else(
                || "running web search".to_string(),
                |query| format!("query {}", truncate_for_summary(query, 100)),
            ),
        _ => {
            let summary = summarize_tool_payload(input);
            if summary.is_empty() {
                format!("running {name}")
            } else {
                format!("{name}: {summary}")
            }
        }
    }
}

/// Builds a conversation runtime that shares the process-wide MCP servers,
/// plugin registry and tool registry (see [`shared_runtime_plugin_state`]).
///
/// The returned [`BuiltRuntime`] does not own that shared state: dropping or
/// replacing it never shuts MCP servers or plugins down. The REPL and one-shot
/// paths call [`shutdown_shared_runtime_state`] when the process is done.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_runtime(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    output: impl Into<TurnOutput>,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    let shared = shared_runtime_plugin_state()?;
    let plugin_registry = shared.state.plugin_registry.clone();
    let mcp_state = shared.state.mcp_state.clone();
    let runtime = assemble_conversation_runtime(
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        output.into(),
        allowed_tools,
        permission_mode,
        progress_reporter,
        shared.state,
        shared.max_iterations_setting,
    )?;
    Ok(BuiltRuntime {
        runtime: Some(runtime),
        plugin_registry,
        plugins_active: false,
        mcp_state,
        mcp_active: false,
    })
}

/// Builds a runtime that owns `runtime_plugin_state`: plugins are initialized
/// here and shut down (with the MCP servers) when the runtime is dropped.
#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_runtime_with_plugin_state(
    session: Session,
    session_id: &str,
    model: String,
    system_prompt: Vec<String>,
    enable_tools: bool,
    output: impl Into<TurnOutput>,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    runtime_plugin_state: RuntimePluginState,
) -> Result<BuiltRuntime, Box<dyn std::error::Error>> {
    runtime_plugin_state.plugin_registry.initialize()?;
    let plugin_registry = runtime_plugin_state.plugin_registry.clone();
    let mcp_state = runtime_plugin_state.mcp_state.clone();
    let runtime = assemble_conversation_runtime(
        session,
        session_id,
        model,
        system_prompt,
        enable_tools,
        output.into(),
        allowed_tools,
        permission_mode,
        progress_reporter,
        runtime_plugin_state,
        None,
    )?;
    Ok(BuiltRuntime::new(runtime, plugin_registry, mcp_state))
}

#[allow(clippy::needless_pass_by_value)]
#[allow(clippy::too_many_arguments)]
fn assemble_conversation_runtime(
    mut session: Session,
    session_id: &str,
    model: String,
    mut system_prompt: Vec<String>,
    enable_tools: bool,
    output: TurnOutput,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    runtime_plugin_state: RuntimePluginState,
    max_iterations_setting: Option<usize>,
) -> Result<ConversationRuntime<AnthropicRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>>
{
    // The model the session runs on, kept in its metadata so a resumed session reports
    // it and continues on it. Stored as people see it: provider setup on resume adds the
    // `openai/` routing prefix again where the transport needs it.
    session.model = Some(crate::provider::shown_model(&model).to_string());
    // The prompt is kept across model switches: it names the model this runtime runs.
    runtime::set_prompt_model(
        &mut system_prompt,
        crate::gateway_catalog::prompt_model_name(&model).as_deref(),
    );
    let RuntimePluginState {
        feature_config,
        mut tool_registry,
        plugin_registry: _,
        mcp_state,
    } = runtime_plugin_state;
    let policy = permission_policy(permission_mode, &feature_config, &tool_registry)
        .map_err(std::io::Error::other)?;
    tool_registry.set_enforcer(runtime::permission_enforcer::PermissionEnforcer::new(
        policy.clone(),
    ));
    let tool_schema_tokens = if enable_tools {
        estimate_tool_schema_tokens(&filter_tool_specs(&tool_registry, allowed_tools.as_ref()))
    } else {
        0
    };
    let context_manager = runtime::context_manager::ContextManager::new(
        runtime::context_manager::ContextBudget::for_model(
            &model,
            max_tokens_for_model(&model),
            tool_schema_tokens,
        ),
        runtime::context_manager::ContextPolicy::from_env(),
    );
    let harness = runtime::HarnessConfig::from_env().with_model(model.clone());
    let reviewer = tools::SubagentReviewer::from_env(&model);
    register_subagent_parent(
        &model,
        permission_mode,
        &feature_config,
        allowed_tools.as_ref(),
    );
    let mut runtime = ConversationRuntime::new_with_features(
        session,
        AnthropicRuntimeClient::new(
            session_id,
            model,
            enable_tools,
            output.clone(),
            allowed_tools.clone(),
            tool_registry.clone(),
            progress_reporter,
        )?,
        CliToolExecutor::new(
            allowed_tools.clone(),
            output.clone(),
            tool_registry.clone(),
            mcp_state.clone(),
        ),
        policy,
        system_prompt,
        &feature_config,
    )
    .with_context_manager(context_manager)
    .with_max_iterations(runtime::resolve_max_iterations(max_iterations_setting))
    .with_harness_config(harness);
    if let Some(reviewer) = reviewer {
        runtime = runtime.with_change_reviewer(Box::new(reviewer));
    }
    if output.is_visible() {
        runtime = runtime.with_hook_progress_reporter(Box::new(CliHookProgressReporter));
    }
    Ok(runtime)
}

/// Estimated tokens of the tool definitions sent with every request.
pub(crate) fn estimate_tool_schema_tokens(definitions: &[ToolDefinition]) -> usize {
    serde_json::to_string(definitions).map_or(0, |serialized| {
        runtime::context_manager::estimate_text_tokens(&serialized)
    })
}

/// Lets sub-agents spawned by the `Agent` tool inherit the parent's resolved
/// model and provider credentials, and never more authority than the parent:
/// its permission mode, permission rules, tool hooks and tool allow-list.
fn register_subagent_parent(
    model: &str,
    permission_mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    allowed_tools: Option<&AllowedToolSet>,
) {
    tools::set_subagent_parent_context(subagent_parent_context_for(
        model,
        permission_mode,
        feature_config,
        allowed_tools,
    ));
}

fn subagent_parent_context_for(
    model: &str,
    permission_mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    allowed_tools: Option<&AllowedToolSet>,
) -> tools::SubagentParentContext {
    let factory: tools::SubagentClientFactory = Arc::new(|model: &str| {
        crate::api_client::build_provider_client(model, None).map_err(|error| error.to_string())
    });
    // Sub-agents get the same map of the workspace as the main conversation.
    let repo_map = crate::repo_map::RepoMap::build(&std::env::current_dir().unwrap_or_default());
    let context_sections = if repo_map.total_files > 0 {
        vec![repo_map.render()]
    } else {
        Vec::new()
    };
    tools::SubagentParentContext {
        permission_rules: feature_config.permission_rules().clone(),
        hooks: feature_config.hooks().clone(),
        allowed_tools: allowed_tools.cloned(),
        client_factory: Some(factory),
        context_sections,
        ..tools::SubagentParentContext::new(model, permission_mode)
    }
}

/// Reads the `maxIterations` setting; `NEURON_MAX_ITERATIONS` overrides it.
/// (The JSON value type is not nameable outside the runtime crate, hence the
/// closure.)
#[allow(clippy::redundant_closure_for_method_calls)]
fn max_iterations_setting(runtime_config: &runtime::RuntimeConfig) -> Option<usize> {
    runtime_config
        .get("maxIterations")
        .and_then(|value| value.as_i64())
        .and_then(|value| usize::try_from(value).ok())
}

/// Identity of the configuration the shared plugin state was built from.
/// Any change (settings files, MCP servers, installed or enabled plugins)
/// triggers a rebuild on the next [`build_runtime`].
#[derive(Debug, Clone, PartialEq)]
struct SharedPluginStateKey {
    cwd: PathBuf,
    config_home: PathBuf,
    config: runtime::RuntimeConfig,
    plugins: PluginRegistry,
}

struct SharedRuntimePluginState {
    key: SharedPluginStateKey,
    generation: u64,
    feature_config: runtime::RuntimeFeatureConfig,
    tool_registry: GlobalToolRegistry,
    plugin_registry: PluginRegistry,
    mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    /// The MCP servers while they start in the background; `mcp_state` is a stand-in.
    background_mcp: Option<BackgroundMcp>,
    max_iterations_setting: Option<usize>,
}

/// When the MCP servers start (see [`set_mcp_startup`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum McpStartup {
    /// Before the first runtime: its tools include theirs.
    Wait,
    /// The same, without the bundled default servers: a one-shot prompt, which would
    /// pay their start for tools it rarely uses.
    WaitWithoutBundled,
    /// On a thread of their own, so the engine and the REPL start at once; their tools
    /// join the first turn after they started.
    Background,
}

static MCP_STARTUP: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// How this process starts its MCP servers; [`McpStartup::Wait`] until set.
pub(crate) fn set_mcp_startup(startup: McpStartup) {
    let value = match startup {
        McpStartup::Wait => 0,
        McpStartup::WaitWithoutBundled => 1,
        McpStartup::Background => 2,
    };
    MCP_STARTUP.store(value, std::sync::atomic::Ordering::Relaxed);
}

fn mcp_startup() -> McpStartup {
    match MCP_STARTUP.load(std::sync::atomic::Ordering::Relaxed) {
        1 => McpStartup::WaitWithoutBundled,
        2 => McpStartup::Background,
        _ => McpStartup::Wait,
    }
}

/// The bundled MCP servers [`McpStartup::WaitWithoutBundled`] left out.
static LEFT_OUT_BUNDLED_MCP: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Records the bundled MCP servers a one-shot run did not start, for ToolSearch to tell
/// the model (it searched for their tools over and over).
pub(crate) fn set_left_out_bundled_mcp_servers(servers: Vec<String>) {
    *LEFT_OUT_BUNDLED_MCP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = servers;
}

/// The bundled MCP servers this run did not start; none outside one-shot runs.
pub(crate) fn left_out_bundled_mcp_servers() -> Vec<String> {
    LEFT_OUT_BUNDLED_MCP
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn next_shared_plugin_generation() -> u64 {
    SHARED_PLUGIN_GENERATION.with(|counter| {
        counter.set(counter.get() + 1);
        counter.get()
    })
}

impl SharedRuntimePluginState {
    /// Takes the MCP servers once they started in the background: they and their tools
    /// replace the stand-in, so runtimes built before no longer count as current.
    fn adopt_started_mcp(&mut self) {
        let Some(finished) = self
            .background_mcp
            .as_ref()
            .and_then(BackgroundMcp::take_finished)
        else {
            return;
        };
        self.background_mcp = None;
        match finished {
            Ok((state, tools)) => {
                match self.tool_registry.clone().with_runtime_tools(tools) {
                    Ok(registry) => self.tool_registry = registry,
                    Err(error) => {
                        runtime::ui_hooks::notice(&format!("warning: MCP tools left out: {error}"));
                    }
                }
                if let Some(stand_in) = self.mcp_state.replace(Arc::new(Mutex::new(state))) {
                    let _ = stand_in
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .shutdown();
                }
                self.generation = next_shared_plugin_generation();
            }
            Err(error) => {
                if let Some(stand_in) = &self.mcp_state {
                    let mut stand_in = stand_in
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    stand_in.starting = false;
                    // Their tools never came: ToolSearch lists them as failed.
                    stand_in.failed_servers = stand_in.server_names();
                }
                runtime::ui_hooks::notice(&format!(
                    "warning: the MCP servers could not start: {error}"
                ));
            }
        }
    }

    fn handle(&self) -> SharedPluginStateHandle {
        SharedPluginStateHandle {
            generation: self.generation,
            state: RuntimePluginState {
                feature_config: self.feature_config.clone(),
                tool_registry: self.tool_registry.clone(),
                plugin_registry: self.plugin_registry.clone(),
                mcp_state: self.mcp_state.clone(),
            },
            max_iterations_setting: self.max_iterations_setting,
        }
    }

    fn shutdown(self) {
        if let Some(background) = self.background_mcp {
            background.stop();
        }
        if let Err(error) = self.plugin_registry.shutdown() {
            runtime::ui_hooks::notice(&format!("warning: plugin shutdown failed: {error}"));
        }
        if let Some(mcp_state) = self.mcp_state {
            if let Err(error) = mcp_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown()
            {
                runtime::ui_hooks::notice(&format!("warning: MCP shutdown failed: {error}"));
            }
        }
    }
}

/// Clones of the shared plugin state for one runtime.
struct SharedPluginStateHandle {
    generation: u64,
    state: RuntimePluginState,
    max_iterations_setting: Option<usize>,
}

thread_local! {
    static SHARED_PLUGIN_STATE: std::cell::RefCell<Option<SharedRuntimePluginState>> =
        const { std::cell::RefCell::new(None) };
    static SHARED_PLUGIN_GENERATION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// MCP servers, plugins and tool registry for the current workspace, built
/// once and reused by every runtime of this REPL session. Rebuilt only when
/// the configuration or plugin set changed, or after
/// [`shutdown_shared_runtime_state`] (explicit reload).
fn shared_runtime_plugin_state() -> Result<SharedPluginStateHandle, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    shared_runtime_plugin_state_with(&cwd, &loader)
}

fn shared_runtime_plugin_state_with(
    cwd: &Path,
    loader: &ConfigLoader,
) -> Result<SharedPluginStateHandle, Box<dyn std::error::Error>> {
    shared_runtime_plugin_state_starting(cwd, loader, mcp_startup())
}

fn shared_runtime_plugin_state_starting(
    cwd: &Path,
    loader: &ConfigLoader,
    startup: McpStartup,
) -> Result<SharedPluginStateHandle, Box<dyn std::error::Error>> {
    let runtime_config = loader.load()?;
    let plugins = build_plugin_manager(cwd, loader, &runtime_config).plugin_registry()?;
    let key = SharedPluginStateKey {
        cwd: cwd.to_path_buf(),
        config_home: loader.config_home().to_path_buf(),
        config: runtime_config.clone(),
        plugins,
    };
    SHARED_PLUGIN_STATE.with(|slot| {
        if let Some(existing) = slot.borrow_mut().as_mut().filter(|state| state.key == key) {
            existing.adopt_started_mcp();
            return Ok(existing.handle());
        }
        if let Some(stale) = slot.borrow_mut().take() {
            stale.shutdown();
        }
        let (state, background_mcp) =
            build_runtime_plugin_state_starting_mcp(cwd, loader, &runtime_config, startup)?;
        state.plugin_registry.initialize()?;
        let shared = SharedRuntimePluginState {
            key,
            generation: next_shared_plugin_generation(),
            feature_config: state.feature_config,
            tool_registry: state.tool_registry,
            plugin_registry: state.plugin_registry,
            mcp_state: state.mcp_state,
            background_mcp,
            max_iterations_setting: max_iterations_setting(&runtime_config),
        };
        let handle = shared.handle();
        *slot.borrow_mut() = Some(shared);
        Ok(handle)
    })
}

/// What the MCP servers are doing now, as the shared state sees them.
pub(crate) struct McpRuntimeView {
    /// The servers still start in the background.
    pub(crate) starting: bool,
    /// Servers the manager runs (stdio).
    pub(crate) running: Vec<String>,
    /// Servers that failed, with why.
    pub(crate) failed: Vec<(String, String)>,
    /// Servers with a transport Neuron does not speak yet, with why.
    pub(crate) unsupported: Vec<(String, String)>,
    /// The discovered tools, as `mcp__<server>__<tool>`.
    pub(crate) tools: Vec<String>,
}

/// The engine's live MCP servers and plugins, for a UI's extensions view.
pub(crate) struct ExtensionsSnapshot {
    pub(crate) mcp: Option<McpRuntimeView>,
    pub(crate) plugins: Vec<plugins::PluginSummary>,
}

/// The shared state's MCP servers and plugins as they are now, taking servers that
/// finished starting in the background. `None` before any runtime was built on this
/// thread.
pub(crate) fn extensions_snapshot() -> Option<ExtensionsSnapshot> {
    SHARED_PLUGIN_STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = slot.as_mut()?;
        state.adopt_started_mcp();
        let mcp = state.mcp_state.as_ref().map(|mcp| {
            let mcp = mcp
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Why each failed, where the start reported it; the background start
            // names its failures without a reason.
            let mut failed: Vec<(String, String)> = mcp
                .degraded_report
                .as_ref()
                .map(|report| {
                    report
                        .failed_servers
                        .iter()
                        .map(|failed| (failed.server_name.clone(), failed.error.message.clone()))
                        .collect()
                })
                .unwrap_or_default();
            for name in mcp.failed_server_names().unwrap_or_default() {
                if !failed.iter().any(|(server, _)| *server == name) {
                    failed.push((name, "could not start".to_string()));
                }
            }
            McpRuntimeView {
                starting: mcp.pending_servers().is_some(),
                running: mcp.server_names(),
                failed,
                unsupported: mcp
                    .manager
                    .unsupported_servers()
                    .iter()
                    .map(|server| (server.server_name.clone(), server.reason.clone()))
                    .collect(),
                tools: state.tool_registry.runtime_tool_names(),
            }
        });
        Some(ExtensionsSnapshot {
            mcp,
            plugins: state.plugin_registry.summaries(),
        })
    })
}

/// Shuts down the shared MCP servers and plugins. The next [`build_runtime`]
/// rebuilds them, so this doubles as an explicit reload.
pub(crate) fn shutdown_shared_runtime_state() {
    if let Some(state) = SHARED_PLUGIN_STATE.with(|slot| slot.borrow_mut().take()) {
        state.shutdown();
    }
}

/// Whether `runtime` still uses the live shared MCP/plugin state. Returns
/// `false` after the shared state was rebuilt (configuration change) or shut
/// down (explicit reload), so the caller rebuilds instead of talking to
/// stopped MCP servers. Runtimes that own their plugin state are always
/// current.
pub(crate) fn runtime_uses_current_shared_state(runtime: &BuiltRuntime) -> bool {
    if runtime.plugins_active || runtime.mcp_active {
        return true;
    }
    SHARED_PLUGIN_STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(shared) = slot.as_mut() else {
            return false;
        };
        shared.adopt_started_mcp();
        let same_mcp = match (&shared.mcp_state, &runtime.mcp_state) {
            (Some(current), Some(mine)) => Arc::ptr_eq(current, mine),
            (None, None) => true,
            _ => false,
        };
        same_mcp && shared.plugin_registry == runtime.plugin_registry
    })
}

pub(crate) struct CliHookProgressReporter;

impl runtime::HookProgressReporter for CliHookProgressReporter {
    fn on_event(&mut self, event: &runtime::HookProgressEvent) {
        if std::env::var("NEURON_DEBUG").is_err() {
            return;
        }
        match event {
            runtime::HookProgressEvent::Started {
                event,
                tool_name,
                command,
            } => runtime::ui_hooks::notice(&format!(
                "[hook {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            )),
            runtime::HookProgressEvent::Completed {
                event,
                tool_name,
                command,
            } => runtime::ui_hooks::notice(&format!(
                "[hook done {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            )),
            runtime::HookProgressEvent::Cancelled {
                event,
                tool_name,
                command,
            } => runtime::ui_hooks::notice(&format!(
                "[hook cancelled {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            )),
        }
    }
}

#[cfg(test)]
mod shared_state_tests {
    use super::{
        estimate_tool_schema_tokens, runtime_uses_current_shared_state,
        shared_runtime_plugin_state_starting, shared_runtime_plugin_state_with,
        shutdown_shared_runtime_state, McpStartup, SHARED_PLUGIN_STATE,
    };
    use runtime::ConfigLoader;
    use std::fs;
    use std::sync::Arc;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    #[test]
    fn mcp_servers_started_in_the_background_replace_the_stand_in_with_their_tools() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "neuron-background-mcp-{}-{nanos}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&config_home).expect("config home");
        let script_path = workspace.join("fixture-mcp.py");
        crate::write_mcp_server_fixture(&script_path);
        let (command, mut args) = if !cfg!(windows) {
            ("python3", Vec::new())
        } else if std::process::Command::new("py")
            .arg("--version")
            .output()
            .is_ok()
        {
            ("py", vec!["-3".to_string()])
        } else {
            ("python", Vec::new())
        };
        args.push(script_path.to_string_lossy().into_owned());
        fs::write(
            config_home.join("settings.json"),
            serde_json::json!({
                "defaultMcpServers": false,
                "mcpServers": {"alpha": {
                    "command": command,
                    "args": args,
                    "env": {"NEURON_MCP_FRAMING": "content-length"}
                }}
            })
            .to_string(),
        )
        .expect("settings");
        let loader = ConfigLoader::new(&workspace, &config_home);
        let names = |handle: &super::SharedPluginStateHandle| {
            handle
                .state
                .tool_registry
                .definitions(None)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };

        // MCP tools are deferred (found through ToolSearch), so they are not among the
        // default definitions; the allow-list knows every tool.
        let knows_echo = |handle: &super::SharedPluginStateHandle| {
            handle
                .state
                .tool_registry
                .normalize_allowed_tools(&["mcp__alpha__echo".to_string()])
                .is_ok()
        };

        let first =
            shared_runtime_plugin_state_starting(&workspace, &loader, McpStartup::Background)
                .expect("builds without waiting for the servers");
        assert!(!knows_echo(&first));
        assert!(names(&first).contains(&"MCPTool".to_string()));
        let stand_in = first.state.mcp_state.clone().expect("a stand-in state");

        let deadline = Instant::now() + Duration::from_secs(60);
        let started = loop {
            let next =
                shared_runtime_plugin_state_starting(&workspace, &loader, McpStartup::Background)
                    .expect("same configuration");
            let current = next.state.mcp_state.clone().expect("mcp state");
            if !Arc::ptr_eq(&current, &stand_in) {
                break next;
            }
            assert!(Instant::now() < deadline, "the servers never joined");
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(knows_echo(&started), "the started server's tools join");
        assert!(
            names(&started).contains(&"MCPTool".to_string()),
            "the wrapper tools stay"
        );
        assert!(started.generation > first.generation);
        assert!(SHARED_PLUGIN_STATE.with(|slot| slot
            .borrow()
            .as_ref()
            .is_some_and(|state| state.background_mcp.is_none())));

        shutdown_shared_runtime_state();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_and_mcp_state_is_built_once_and_rebuilt_only_on_change_or_reload() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "neuron-shared-state-{}-{nanos}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&config_home).expect("config home");
        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultMcpServers":false}"#,
        )
        .expect("settings");
        let loader = ConfigLoader::new(&workspace, &config_home);

        let first = shared_runtime_plugin_state_with(&workspace, &loader).expect("first build");
        let second = shared_runtime_plugin_state_with(&workspace, &loader).expect("reuse");
        assert_eq!(first.generation, second.generation, "reused across turns");

        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultMcpServers":false,"model":"sonnet"}"#,
        )
        .expect("settings change");
        let third = shared_runtime_plugin_state_with(&workspace, &loader).expect("rebuild");
        assert_ne!(
            second.generation, third.generation,
            "config change rebuilds"
        );

        // A runtime built on the current state is current until a reload.
        let built = crate::BuiltRuntime {
            runtime: None,
            plugin_registry: third.state.plugin_registry.clone(),
            plugins_active: false,
            mcp_state: third.state.mcp_state.clone(),
            mcp_active: false,
        };
        assert!(runtime_uses_current_shared_state(&built));

        shutdown_shared_runtime_state();
        assert!(
            !runtime_uses_current_shared_state(&built),
            "a reloaded state makes live runtimes stale"
        );
        let fourth = shared_runtime_plugin_state_with(&workspace, &loader).expect("reload");
        assert_ne!(
            third.generation, fourth.generation,
            "explicit reload rebuilds"
        );

        shutdown_shared_runtime_state();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subagents_inherit_the_parent_rules_hooks_and_tool_allow_list() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "neuron-subagent-parent-{}-{nanos}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&workspace).expect("workspace");
        fs::create_dir_all(&config_home).expect("config home");
        fs::write(
            config_home.join("settings.json"),
            r#"{"defaultMcpServers":false,"permissions":{"deny":["bash(git push:*)"],"ask":["write_file"]},"hooks":{"PreToolUse":["audit-hook"]}}"#,
        )
        .expect("settings");
        let config = ConfigLoader::new(&workspace, &config_home)
            .load()
            .expect("settings load");
        let allowed: crate::AllowedToolSet = ["read_file".to_string(), "bash".to_string()]
            .into_iter()
            .collect();

        let context = super::subagent_parent_context_for(
            "openai/groq/openai/gpt-oss-120b",
            runtime::PermissionMode::DangerFullAccess,
            config.feature_config(),
            Some(&allowed),
        );

        assert_eq!(context.model, "openai/groq/openai/gpt-oss-120b");
        assert_eq!(
            context.permission_mode,
            runtime::PermissionMode::DangerFullAccess
        );
        assert_eq!(context.permission_rules.deny(), ["bash(git push:*)"]);
        assert_eq!(context.permission_rules.ask(), ["write_file"]);
        assert_eq!(context.hooks.pre_tool_use(), ["audit-hook"]);
        assert_eq!(context.allowed_tools.as_ref(), Some(&allowed));
        assert!(context.client_factory.is_some());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tool_schema_estimate_grows_with_the_tool_set() {
        let registry = tools::GlobalToolRegistry::builtin();
        let all = estimate_tool_schema_tokens(&registry.definitions(None));
        let one = estimate_tool_schema_tokens(
            &registry.definitions(Some(&["read_file".to_string()].into_iter().collect())),
        );
        assert!(one > 0);
        assert!(all > one * 5, "all={all} one={one}");
    }
}
