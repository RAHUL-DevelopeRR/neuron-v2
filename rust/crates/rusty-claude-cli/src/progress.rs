//! Internal prompt progress tracking and reporting.
//!
//! Progress UI for bughunter, ultraplan, commit, PR, and issue commands.

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
        let _ = writeln!(stdout, "{line}");
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
    let status = status_bits.join(" Â· ");
    match event {
        InternalPromptProgressEvent::Started => {
            format!(
                "ðŸ§­ {} status Â· planning started Â· {status}",
                snapshot.command_label
            )
        }
        InternalPromptProgressEvent::Update => {
            format!("â€¦ {} status Â· {status}", snapshot.command_label)
        }
        InternalPromptProgressEvent::Heartbeat => format!(
            "â€¦ {} heartbeat Â· {elapsed_seconds}s elapsed Â· {status}",
            snapshot.command_label
        ),
        InternalPromptProgressEvent::Complete => format!(
            "âœ” {} status Â· completed Â· {elapsed_seconds}s elapsed Â· {} steps total",
            snapshot.command_label, snapshot.step
        ),
        InternalPromptProgressEvent::Failed => format!(
            "âœ˜ {} status Â· failed Â· {elapsed_seconds}s elapsed Â· {}",
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
    emit_output: bool,
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
        emit_output,
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
    emit_output: bool,
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
        emit_output,
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
    system_prompt: Vec<String>,
    enable_tools: bool,
    emit_output: bool,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    progress_reporter: Option<InternalPromptProgressReporter>,
    runtime_plugin_state: RuntimePluginState,
    max_iterations_setting: Option<usize>,
) -> Result<ConversationRuntime<AnthropicRuntimeClient, CliToolExecutor>, Box<dyn std::error::Error>>
{
    // Persist the model in session metadata so resumed sessions can report it.
    if session.model.is_none() {
        session.model = Some(model.clone());
    }
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
    register_subagent_parent(&model, permission_mode);
    let mut runtime = ConversationRuntime::new_with_features(
        session,
        AnthropicRuntimeClient::new(
            session_id,
            model,
            enable_tools,
            emit_output,
            allowed_tools.clone(),
            tool_registry.clone(),
            progress_reporter,
        )?,
        CliToolExecutor::new(
            allowed_tools.clone(),
            emit_output,
            tool_registry.clone(),
            mcp_state.clone(),
        ),
        policy,
        system_prompt,
        &feature_config,
    )
    .with_context_manager(context_manager)
    .with_max_iterations(runtime::resolve_max_iterations(max_iterations_setting));
    if emit_output {
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
/// model, permission mode and provider credentials.
fn register_subagent_parent(model: &str, permission_mode: PermissionMode) {
    let factory: tools::SubagentClientFactory = Arc::new(|model: &str| {
        crate::api_client::build_provider_client(model, None).map_err(|error| error.to_string())
    });
    tools::set_subagent_parent_context(tools::SubagentParentContext {
        model: model.to_string(),
        permission_mode,
        client_factory: Some(factory),
    });
}

/// Reads the `maxIterations` setting. (The JSON value type is not nameable
/// outside the runtime crate, hence the closure.)
///
/// Settings validation (`runtime::config_validate`) still rejects the key as
/// unknown, so today this only takes effect once that schema accepts it;
/// `NEURON_MAX_ITERATIONS` works regardless.
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
    max_iterations_setting: Option<usize>,
}

impl SharedRuntimePluginState {
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
        if let Err(error) = self.plugin_registry.shutdown() {
            eprintln!("warning: plugin shutdown failed: {error}");
        }
        if let Some(mcp_state) = self.mcp_state {
            if let Err(error) = mcp_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown()
            {
                eprintln!("warning: MCP shutdown failed: {error}");
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
    let runtime_config = loader.load()?;
    let plugins = build_plugin_manager(cwd, loader, &runtime_config).plugin_registry()?;
    let key = SharedPluginStateKey {
        cwd: cwd.to_path_buf(),
        config_home: loader.config_home().to_path_buf(),
        config: runtime_config.clone(),
        plugins,
    };
    SHARED_PLUGIN_STATE.with(|slot| {
        if let Some(existing) = slot.borrow().as_ref().filter(|state| state.key == key) {
            return Ok(existing.handle());
        }
        if let Some(stale) = slot.borrow_mut().take() {
            stale.shutdown();
        }
        let state = build_runtime_plugin_state_with_loader(cwd, loader, &runtime_config)?;
        state.plugin_registry.initialize()?;
        let generation = SHARED_PLUGIN_GENERATION.with(|counter| {
            counter.set(counter.get() + 1);
            counter.get()
        });
        let shared = SharedRuntimePluginState {
            key,
            generation,
            feature_config: state.feature_config,
            tool_registry: state.tool_registry,
            plugin_registry: state.plugin_registry,
            mcp_state: state.mcp_state,
            max_iterations_setting: max_iterations_setting(&runtime_config),
        };
        let handle = shared.handle();
        *slot.borrow_mut() = Some(shared);
        Ok(handle)
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
        let slot = slot.borrow();
        let Some(shared) = slot.as_ref() else {
            return false;
        };
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
            } => eprintln!(
                "[hook {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Completed {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook done {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
            runtime::HookProgressEvent::Cancelled {
                event,
                tool_name,
                command,
            } => eprintln!(
                "[hook cancelled {event_name}] {tool_name}: {command}",
                event_name = event.as_str()
            ),
        }
    }
}

#[cfg(test)]
mod shared_state_tests {
    use super::{
        estimate_tool_schema_tokens, runtime_uses_current_shared_state,
        shared_runtime_plugin_state_with, shutdown_shared_runtime_state,
    };
    use runtime::ConfigLoader;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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
