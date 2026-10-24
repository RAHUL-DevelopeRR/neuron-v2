//! Tool executor, permission prompting, message conversion, and help output.
//!
//! Extracted from main.rs.

// A turn's output goes through a TurnSink or runtime::ui_hooks, so a full-screen UI can own
// the terminal.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::*;
use neuron_tui::protocol::{PermissionDecision, PermissionReply, PermissionView, UiEvent};
use std::env;
use std::io::{self, Write};

pub(crate) struct CliPermissionPrompter {
    current_mode: PermissionMode,
}

impl CliPermissionPrompter {
    pub fn new(current_mode: PermissionMode) -> Self {
        Self { current_mode }
    }
}

// The line REPL prompts on its own terminal.
#[allow(clippy::print_stdout)]
impl runtime::PermissionPrompter for CliPermissionPrompter {
    fn decide(
        &mut self,
        request: &runtime::PermissionRequest,
    ) -> runtime::PermissionPromptDecision {
        // Nobody can answer (a script, CI, `< /dev/null`): no prompt. The model gets the
        // reason as the tool's result, the user sees it now and in the run's last warning.
        if crate::headless::stdin_is_unattended() {
            let reason = crate::headless::denial_reason(
                &request.tool_name,
                self.current_mode,
                request.required_mode,
            );
            crate::headless::record_denial(&request.tool_name, &reason);
            return runtime::PermissionPromptDecision::Deny { reason };
        }
        println!();
        println!("Permission approval required");
        println!("  Tool             {}", request.tool_name);
        println!("  Current mode     {}", self.current_mode.as_str());
        println!("  Required mode    {}", request.required_mode.as_str());
        if let Some(reason) = &request.reason {
            println!("  Reason           {reason}");
        }
        println!("  Input            {}", request.input);
        print!("Approve this tool call? [y/N]: ");
        let _ = io::stdout().flush();

        let mut response = String::new();
        match io::stdin().read_line(&mut response) {
            Ok(_) => {
                let normalized = response.trim().to_ascii_lowercase();
                if matches!(normalized.as_str(), "y" | "yes") {
                    runtime::PermissionPromptDecision::Allow
                } else {
                    runtime::PermissionPromptDecision::Deny {
                        reason: format!(
                            "tool '{}' denied by user approval prompt",
                            request.tool_name
                        ),
                    }
                }
            }
            Err(error) => runtime::PermissionPromptDecision::Deny {
                reason: format!("permission approval failed: {error}"),
            },
        }
    }
}

/// How often a pending UI approval checks whether the turn was cancelled.
const PERMISSION_CANCEL_POLL: Duration = Duration::from_millis(250);

/// Asks the full-screen UI to approve a tool call and waits for its answer. An
/// interrupted turn or a closed UI denies the call, so nothing runs unapproved.
pub(crate) struct ChannelPermissionPrompter<'a> {
    current_mode: PermissionMode,
    events: Sender<UiEvent>,
    replies: &'a Receiver<PermissionReply>,
    cancel: runtime::CancellationToken,
    next_id: u64,
}

impl<'a> ChannelPermissionPrompter<'a> {
    pub(crate) fn new(
        current_mode: PermissionMode,
        events: Sender<UiEvent>,
        replies: &'a Receiver<PermissionReply>,
        cancel: runtime::CancellationToken,
    ) -> Self {
        Self {
            current_mode,
            events,
            replies,
            cancel,
            next_id: 0,
        }
    }

    /// Numbers this prompter's requests after `last_id`, so a late answer to an earlier
    /// turn's request never matches one of this turn's.
    pub(crate) fn numbered_after(mut self, last_id: u64) -> Self {
        self.next_id = last_id;
        self
    }

    /// The id of the last request sent (or the starting point when none was).
    pub(crate) fn last_id(&self) -> u64 {
        self.next_id
    }
}

impl runtime::PermissionPrompter for ChannelPermissionPrompter<'_> {
    fn decide(
        &mut self,
        request: &runtime::PermissionRequest,
    ) -> runtime::PermissionPromptDecision {
        let deny = |reason: String| runtime::PermissionPromptDecision::Deny { reason };
        self.next_id += 1;
        let id = self.next_id;
        let asked = self.events.send(UiEvent::PermissionRequested {
            id,
            request: PermissionView {
                tool: request.tool_name.clone(),
                current_mode: self.current_mode.as_str().to_string(),
                required_mode: request.required_mode.as_str().to_string(),
                reason: request.reason.clone(),
                input: request.input.clone(),
                session_prefixes: request.session_prefixes.clone(),
            },
        });
        if asked.is_err() {
            return deny("permission approval failed: the Neuron UI closed".to_string());
        }
        loop {
            if self.cancel.is_cancelled() {
                return deny(format!(
                    "tool '{}' not run: the turn was interrupted",
                    request.tool_name
                ));
            }
            match self.replies.recv_timeout(PERMISSION_CANCEL_POLL) {
                Ok(reply) if reply.id == id => {
                    return match reply.decision {
                        PermissionDecision::AllowOnce => runtime::PermissionPromptDecision::Allow,
                        PermissionDecision::AllowForSession
                            if !request.session_prefixes.is_empty() =>
                        {
                            runtime::PermissionPromptDecision::AllowForSession
                        }
                        // Nothing to remember for the session: approve this call only.
                        PermissionDecision::AllowForSession => {
                            runtime::PermissionPromptDecision::Allow
                        }
                        PermissionDecision::Deny { note } => deny(match note {
                            Some(note) if !note.trim().is_empty() => format!(
                                "tool '{}' denied by the user: {}",
                                request.tool_name,
                                note.trim()
                            ),
                            _ => format!(
                                "tool '{}' denied by user approval prompt",
                                request.tool_name
                            ),
                        }),
                    };
                }
                // An answer to an earlier request the turn already gave up on.
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return deny("permission approval failed: the Neuron UI closed".to_string());
                }
            }
        }
    }
}

// NOTE: Despite the historical name `AnthropicRuntimeClient`, this struct
// now holds an `ApiProviderClient` which dispatches to Anthropic, xAI,
// OpenAI, or DashScope at construction time based on
// `detect_provider_kind(&model)`. The struct name is kept to avoid
// churning `BuiltRuntime` and every Deref/DerefMut site that references
// it. See ROADMAP #29 for the provider-dispatch routing fix.
// -- API client (extracted to api_client.rs) --
pub(crate) struct CliToolExecutor {
    output: Box<dyn TurnSink>,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
}

impl CliToolExecutor {
    pub fn new(
        allowed_tools: Option<AllowedToolSet>,
        output: impl Into<TurnOutput>,
        tool_registry: GlobalToolRegistry,
        mcp_state: Option<Arc<Mutex<RuntimeMcpState>>>,
    ) -> Self {
        Self {
            output: output.into().tool_sink(),
            allowed_tools,
            tool_registry,
            mcp_state,
        }
    }

    pub fn execute_search_tool(&self, value: serde_json::Value) -> Result<String, ToolError> {
        let input: ToolSearchRequest = serde_json::from_value(value)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        let (pending_mcp_servers, failed_mcp_servers, mcp_degraded, mcp_starting) = self
            .mcp_state
            .as_ref()
            .map_or((None, None, None, None), |state| {
                let state = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (
                    state.pending_servers(),
                    state.failed_server_names(),
                    state.degraded_report(),
                    state.starting_note(),
                )
            });
        let output = self.tool_registry.search(
            &input.query,
            input.max_results.unwrap_or(5),
            pending_mcp_servers,
            mcp_degraded,
        );
        let mut output =
            serde_json::to_value(output).map_err(|error| ToolError::new(error.to_string()))?;
        if let Some(failed) = failed_mcp_servers {
            output["failed_mcp_servers"] = json!(failed);
        }
        let mut notes = Vec::new();
        if let Some(note) = mcp_starting {
            // Like the MCP tools say: no match yet is not "no such tool".
            notes.push(note);
        }
        let left_out = crate::progress::left_out_bundled_mcp_servers();
        if !left_out.is_empty() {
            // A one-shot run: without this the model keeps searching for them.
            notes.push(format!(
                "This one-shot run (`neuron prompt`) does not load the bundled MCP servers ({}), so their tools are not available and no search will find them. Interactive sessions load them.",
                left_out.join(", ")
            ));
        }
        if !notes.is_empty() {
            output["note"] = json!(notes.join(" "));
        }
        serde_json::to_string_pretty(&output).map_err(|error| ToolError::new(error.to_string()))
    }

    /// Runs an MCP tool or wrapper; `cancel` abandons a tool call that is still running.
    pub fn execute_runtime_tool(
        &self,
        tool_name: &str,
        value: serde_json::Value,
        cancel: &runtime::CancellationToken,
    ) -> Result<String, ToolError> {
        let Some(mcp_state) = &self.mcp_state else {
            return Err(ToolError::new(format!(
                "runtime tool `{tool_name}` is unavailable without configured MCP servers"
            )));
        };
        let mut mcp_state = mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match tool_name {
            "MCPTool" => {
                let input: McpToolRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
                let qualified_name = input
                    .qualified_name
                    .or(input.tool)
                    .ok_or_else(|| ToolError::new("missing required field `qualifiedName`"))?;
                // Models send the object as JSON text too; servers want the object.
                let arguments = tools::mcp_call_arguments(input.arguments)
                    .map_err(|error| ToolError::new(format!("invalid MCPTool input: {error}")))?;
                mcp_state.call_tool(&qualified_name, arguments, cancel)
            }
            "ListMcpResourcesTool" => {
                let input: ListMcpResourcesRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
                match input.server {
                    Some(server_name) => mcp_state.list_resources_for_server(&server_name),
                    None => mcp_state.list_resources_for_all_servers(),
                }
            }
            "ReadMcpResourceTool" => {
                let input: ReadMcpResourceRequest = serde_json::from_value(value)
                    .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
                mcp_state.read_resource(&input.server, &input.uri)
            }
            _ => mcp_state.call_tool(tool_name, Some(value), cancel),
        }
    }
}

impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
        self.execute_with_cancellation(tool_name, input, &runtime::CancellationToken::new())
    }

    /// An interrupted turn cancels `cancel`: `bash`, `PowerShell` and `REPL` then kill
    /// the command's whole process tree and return the output captured so far.
    fn execute_with_cancellation(
        &mut self,
        tool_name: &str,
        input: &str,
        cancel: &runtime::CancellationToken,
    ) -> Result<String, ToolError> {
        if self
            .allowed_tools
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(tool_name))
        {
            return Err(ToolError::new(format!(
                "tool `{tool_name}` is not enabled by the current --allowedTools setting"
            )));
        }
        let value = serde_json::from_str(input)
            .map_err(|error| ToolError::new(format!("invalid tool input JSON: {error}")))?;
        let result = if tool_name == "ToolSearch" {
            self.execute_search_tool(value)
        } else if self.tool_registry.has_runtime_tool(tool_name) {
            self.execute_runtime_tool(tool_name, value, cancel)
        } else {
            self.tool_registry
                .execute_with_cancel(tool_name, &value, cancel)
                .map_err(ToolError::new)
        };
        match result {
            Ok(output) => {
                self.output
                    .tool_result(tool_name, &output, false)
                    .map_err(|error| ToolError::new(error.to_string()))?;
                Ok(output)
            }
            Err(error) => {
                self.output
                    .tool_result(tool_name, &error.to_string(), true)
                    .map_err(|stream_error| ToolError::new(stream_error.to_string()))?;
                Err(error)
            }
        }
    }

    fn tool_started(&mut self, id: &str, name: &str, input: &str) {
        let _ = self.output.tool_started(id, name, input);
    }

    fn tool_call_finished(&mut self, id: &str, name: &str, output: &str, is_error: bool) {
        // A closed UI also fails the turn's next stream write, which ends the turn.
        let _ = self.output.tool_call_finished(id, name, output, is_error);
    }
}

pub(crate) fn permission_policy(
    mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) -> Result<PermissionPolicy, String> {
    Ok(tool_registry.permission_specs(None)?.into_iter().fold(
        PermissionPolicy::new(mode).with_permission_rules(feature_config.permission_rules()),
        |policy, (name, required_permission)| {
            policy.with_tool_requirement(name, required_permission)
        },
    ))
}

pub(crate) fn convert_messages(messages: &[ConversationMessage]) -> Vec<InputMessage> {
    messages
        .iter()
        .filter_map(|message| {
            let role = match message.role {
                MessageRole::System | MessageRole::User | MessageRole::Tool => "user",
                MessageRole::Assistant => "assistant",
            };
            let content = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => InputContentBlock::Text { text: text.clone() },
                    ContentBlock::ToolUse { id, name, input } => InputContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: serde_json::from_str(input)
                            .unwrap_or_else(|_| serde_json::json!({ "raw": input })),
                    },
                    ContentBlock::ToolResult {
                        tool_use_id,
                        output,
                        is_error,
                        ..
                    } => InputContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: vec![ToolResultContentBlock::Text {
                            text: output.clone(),
                        }],
                        is_error: *is_error,
                    },
                })
                .collect::<Vec<_>>();
            (!content.is_empty()).then(|| InputMessage {
                role: role.to_string(),
                content,
            })
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
pub(crate) fn print_help_to(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "neuron v{VERSION}")?;
    writeln!(out)?;
    writeln!(out, "Usage:")?;
    writeln!(
        out,
        "  neuron auth <login|logout|status> [--output-format text|json]"
    )?;
    writeln!(
        out,
        "      Connect your Zero-X account to the coding gateway"
    )?;
    writeln!(out, "  neuron login [--no-browser | --with-token]")?;
    writeln!(
        out,
        "      Sign in to Zero-X (short for `neuron auth login`; --with-token reads a token from stdin)"
    )?;
    writeln!(out, "  neuron logout")?;
    writeln!(
        out,
        "      Sign out of Zero-X (short for `neuron auth logout`)"
    )?;
    writeln!(out, "  neuron usage")?;
    writeln!(
        out,
        "      Show your Zero-X plan, today's usage and credits"
    )?;
    writeln!(out, "  neuron upgrade [--no-browser]")?;
    writeln!(out, "      Open the Zero-X plans page")?;
    writeln!(
        out,
        "      (These account commands take --output-format text|json. To send a prompt that"
    )?;
    writeln!(
        out,
        "      starts with one of their names, quote it or use `neuron prompt \"...\"`.)"
    )?;
    writeln!(
        out,
        "  neuron [--model MODEL] [--allowedTools TOOL[,TOOL...]]"
    )?;
    writeln!(
        out,
        "      Start the full-screen UI on a terminal (the line REPL with --no-tui, or without neuron-tui)"
    )?;
    writeln!(
        out,
        "  neuron [--model MODEL] [--output-format text|json] prompt TEXT"
    )?;
    writeln!(out, "      Send one prompt and exit")?;
    writeln!(
        out,
        "  neuron [--model MODEL] [--output-format text|json] TEXT"
    )?;
    writeln!(out, "      Shorthand non-interactive prompt mode")?;
    writeln!(
        out,
        "  neuron --resume [SESSION.jsonl|session-id|latest] [/status] [/compact] [...]"
    )?;
    writeln!(
        out,
        "      Inspect or maintain a saved session without entering the REPL"
    )?;
    writeln!(out, "  neuron help")?;
    writeln!(out, "      Alias for --help")?;
    writeln!(out, "  neuron version")?;
    writeln!(out, "      Alias for --version")?;
    writeln!(out, "  neuron status")?;
    writeln!(
        out,
        "      Show the current local workspace status snapshot"
    )?;
    writeln!(out, "  neuron sandbox")?;
    writeln!(out, "      Show the current sandbox isolation snapshot")?;
    writeln!(out, "  neuron doctor")?;
    writeln!(
        out,
        "      Diagnose local auth, config, workspace, and sandbox health"
    )?;
    writeln!(out, "  neuron acp [serve]")?;
    writeln!(
        out,
        "      Show ACP/Zed editor integration status (currently unsupported; aliases: --acp, -acp)"
    )?;
    writeln!(out, "  neuron dump-manifests [--manifests-dir PATH]")?;
    writeln!(out, "  neuron bootstrap-plan")?;
    writeln!(out, "  neuron agents")?;
    writeln!(out, "  neuron mcp")?;
    writeln!(out, "  neuron skills")?;
    writeln!(
        out,
        "  neuron system-prompt [--cwd PATH] [--date YYYY-MM-DD]"
    )?;
    writeln!(out, "  neuron init")?;
    writeln!(
        out,
        "  neuron export [PATH] [--session SESSION] [--output PATH]"
    )?;
    writeln!(
        out,
        "      Dump the latest (or named) session as markdown; writes to PATH or stdout"
    )?;
    writeln!(out)?;
    writeln!(out, "Flags:")?;
    writeln!(
        out,
        "  --model MODEL              Override the active model"
    )?;
    writeln!(
        out,
        "  --output-format FORMAT     Non-interactive output format: text or json"
    )?;
    writeln!(
        out,
        "  --compact                  Strip tool call details; print only the final assistant text (text mode only; useful for piping)"
    )?;
    writeln!(
        out,
        "  --permission-mode MODE     Set read-only, workspace-write, or danger-full-access"
    )?;
    writeln!(
        out,
        "  --dangerously-skip-permissions  Skip all permission checks"
    )?;
    writeln!(
        out,
        "  --provider NAME            Choose the model provider: {}",
        crate::provider::ProviderId::names()
    )?;
    writeln!(
        out,
        "  --reasoning-effort LEVEL   low, medium or high, for models that support it"
    )?;
    writeln!(out, "  --allowedTools TOOLS       Restrict enabled tools (repeatable; comma-separated aliases supported)")?;
    writeln!(
        out,
        "  --no-tui                   Use the line REPL even where the full-screen UI is installed (or set NEURON_NO_TUI=1)"
    )?;
    writeln!(
        out,
        "  --continue [CHAT]          Reopen a chat in the full-screen UI (the latest, or CHAT)"
    )?;
    writeln!(
        out,
        "  --version, -V              Print version and build information locally"
    )?;
    writeln!(out)?;
    writeln!(out, "Interactive slash commands:")?;
    writeln!(out, "{}", render_slash_command_help_filtered(STUB_COMMANDS))?;
    writeln!(out)?;
    let resume_commands = resume_supported_slash_commands()
        .into_iter()
        .map(|spec| match spec.argument_hint {
            Some(argument_hint) => format!("/{} {}", spec.name, argument_hint),
            None => format!("/{}", spec.name),
        })
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "Resume-safe commands: {resume_commands}")?;
    writeln!(out)?;
    writeln!(out, "Session shortcuts:")?;
    writeln!(
        out,
        "  REPL turns auto-save to .neuron/sessions/<session-id>.{PRIMARY_SESSION_EXTENSION}"
    )?;
    writeln!(
        out,
        "  Use `{LATEST_SESSION_REFERENCE}` with --resume, /resume, or /session switch to target the newest saved session"
    )?;
    writeln!(
        out,
        "  Use /session list in the REPL to browse managed sessions"
    )?;
    writeln!(out, "Examples:")?;
    writeln!(out, "  neuron --model auto \"summarize this repo\"")?;
    writeln!(
        out,
        "  neuron --output-format json prompt \"explain src/main.rs\""
    )?;
    writeln!(out, "  neuron --compact \"summarize Cargo.toml\" | wc -l")?;
    writeln!(
        out,
        "  neuron --allowedTools read,glob \"summarize Cargo.toml\""
    )?;
    writeln!(out, "  neuron --resume {LATEST_SESSION_REFERENCE}")?;
    writeln!(
        out,
        "  neuron --resume {LATEST_SESSION_REFERENCE} /status /diff /export notes.txt"
    )?;
    writeln!(out, "  neuron agents")?;
    writeln!(out, "  neuron mcp show my-server")?;
    writeln!(out, "  neuron /skills")?;
    writeln!(out, "  neuron doctor")?;
    writeln!(out, "  neuron init")?;
    writeln!(out, "  neuron export")?;
    writeln!(out, "  neuron export conversation.md")?;
    writeln!(out)?;
    writeln!(out, "Install and update:")?;
    writeln!(
        out,
        "  npm install -g {NPM_PACKAGE}, pipx install {PYPI_PACKAGE}, or install.sh / install.ps1 from {NEURON_REPO_URL}/releases"
    )?;
    writeln!(out, "  Source: {NEURON_REPO_URL}")?;
    Ok(())
}

#[allow(clippy::print_stdout)]
pub(crate) fn print_help(output_format: CliOutputFormat) -> Result<(), Box<dyn std::error::Error>> {
    let mut buffer = Vec::new();
    print_help_to(&mut buffer)?;
    let message = String::from_utf8(buffer)?;
    match output_format {
        CliOutputFormat::Text => print!("{message}"),
        CliOutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "help",
                "message": message,
            }))?
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tool_search_tests {
    use super::CliToolExecutor;
    use runtime::ToolExecutor;
    use tools::GlobalToolRegistry;

    /// A one-shot run leaves out the bundled MCP servers; one model searched for their
    /// tools 17 times. ToolSearch says they are not loaded.
    #[test]
    fn tool_search_says_a_one_shot_run_left_out_the_bundled_mcp_servers() {
        crate::progress::set_left_out_bundled_mcp_servers(vec![
            "codebase-memory".to_string(),
            "playwright".to_string(),
        ]);
        let mut executor = CliToolExecutor::new(None, false, GlobalToolRegistry::builtin(), None);
        let output = executor.execute("ToolSearch", r#"{"query":"browser navigate"}"#);
        crate::progress::set_left_out_bundled_mcp_servers(Vec::new());

        let output: serde_json::Value =
            serde_json::from_str(&output.expect("ToolSearch runs")).expect("json");
        let note = output["note"].as_str().unwrap_or_default();
        assert!(
            note.contains("does not load the bundled MCP servers (codebase-memory, playwright)"),
            "{output}"
        );
    }
}

#[cfg(test)]
mod channel_prompter_tests {
    use super::ChannelPermissionPrompter;
    use neuron_tui::protocol::{PermissionDecision, PermissionReply, UiEvent};
    use runtime::{
        CancellationToken, PermissionMode, PermissionPromptDecision, PermissionPrompter,
        PermissionRequest,
    };
    use std::sync::mpsc;
    use std::thread;

    fn request(session_prefixes: &[&str]) -> PermissionRequest {
        PermissionRequest {
            tool_name: "bash".to_string(),
            input: r#"{"command":"cargo test"}"#.to_string(),
            current_mode: PermissionMode::WorkspaceWrite,
            required_mode: PermissionMode::DangerFullAccess,
            reason: None,
            session_prefixes: session_prefixes.iter().map(ToString::to_string).collect(),
        }
    }

    /// Runs one prompt while `ui` plays the user: it gets the request's id and the
    /// reply sender, and returns once it has answered (or walked away).
    fn decide_with(
        session_prefixes: &[&str],
        cancel: &CancellationToken,
        ui: impl FnOnce(u64, mpsc::Sender<PermissionReply>) + Send,
    ) -> PermissionPromptDecision {
        let (events_tx, events_rx) = mpsc::channel();
        let (replies_tx, replies_rx) = mpsc::channel();
        thread::scope(|scope| {
            scope.spawn(move || {
                let Ok(UiEvent::PermissionRequested { id, request }) = events_rx.recv() else {
                    panic!("the UI is asked first");
                };
                assert_eq!(request.tool, "bash");
                assert_eq!(request.current_mode, "workspace-write");
                assert_eq!(request.required_mode, "danger-full-access");
                ui(id, replies_tx);
            });
            ChannelPermissionPrompter::new(
                PermissionMode::WorkspaceWrite,
                events_tx,
                &replies_rx,
                cancel.clone(),
            )
            .decide(&request(session_prefixes))
        })
    }

    #[test]
    fn the_user_answer_decides_and_stale_answers_are_ignored() {
        let cancel = CancellationToken::new();
        let allowed = decide_with(&[], &cancel, |id, replies| {
            replies
                .send(PermissionReply {
                    id: id + 41,
                    decision: PermissionDecision::AllowOnce,
                })
                .unwrap();
            replies
                .send(PermissionReply {
                    id,
                    decision: PermissionDecision::AllowOnce,
                })
                .unwrap();
        });
        assert_eq!(allowed, PermissionPromptDecision::Allow);

        let for_session = decide_with(&["cargo test"], &cancel, |id, replies| {
            replies
                .send(PermissionReply {
                    id,
                    decision: PermissionDecision::AllowForSession,
                })
                .unwrap();
        });
        assert_eq!(for_session, PermissionPromptDecision::AllowForSession);

        let nothing_to_remember = decide_with(&[], &cancel, |id, replies| {
            replies
                .send(PermissionReply {
                    id,
                    decision: PermissionDecision::AllowForSession,
                })
                .unwrap();
        });
        assert_eq!(nothing_to_remember, PermissionPromptDecision::Allow);

        let denied = decide_with(&[], &cancel, |id, replies| {
            replies
                .send(PermissionReply {
                    id,
                    decision: PermissionDecision::Deny {
                        note: Some("use --dry-run first".into()),
                    },
                })
                .unwrap();
        });
        let PermissionPromptDecision::Deny { reason } = denied else {
            panic!("denied");
        };
        assert!(reason.contains("use --dry-run first"), "{reason}");
    }

    #[test]
    fn an_interrupted_turn_denies_the_pending_call() {
        let cancel = CancellationToken::new();
        let turn = cancel.clone();
        let decision = decide_with(&[], &cancel, move |_id, replies| {
            turn.cancel();
            // The UI is still open but never answers.
            thread::sleep(std::time::Duration::from_millis(600));
            drop(replies);
        });
        let PermissionPromptDecision::Deny { reason } = decision else {
            panic!("an interrupted turn runs nothing");
        };
        assert!(reason.contains("interrupted"), "{reason}");
    }

    #[test]
    fn a_closed_ui_denies_the_call() {
        let cancel = CancellationToken::new();
        let decision = decide_with(&[], &cancel, |_id, replies| drop(replies));
        assert!(matches!(
            decision,
            PermissionPromptDecision::Deny { reason } if reason.contains("UI closed")
        ));

        let (events_tx, events_rx) = mpsc::channel();
        drop(events_rx);
        let (_replies_tx, replies_rx) = mpsc::channel();
        let decision = ChannelPermissionPrompter::new(
            PermissionMode::WorkspaceWrite,
            events_tx,
            &replies_rx,
            cancel,
        )
        .decide(&request(&[]));
        assert!(matches!(decision, PermissionPromptDecision::Deny { .. }));
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::CliToolExecutor;
    use runtime::{CancellationToken, ToolExecutor};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tools::GlobalToolRegistry;

    /// Where the command under test writes the PID of the long-running child it starts.
    fn pid_file(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        std::env::temp_dir().join(format!(
            "neuron-cancel-{label}-{}-{nanos}.pid",
            std::process::id()
        ))
    }

    /// Whether process `pid` still runs (a zombie waiting to be reaped does not).
    fn is_running(pid: u32) -> bool {
        if cfg!(windows) {
            // Not `tasklist`: it hangs while the WMI service is stuck.
            Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &format!(
                        "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ 'running' }}"
                    ),
                ])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("running"))
        } else {
            Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .is_ok_and(|output| {
                    let state = String::from_utf8_lossy(&output.stdout);
                    let state = state.trim();
                    !state.is_empty() && !state.starts_with('Z')
                })
        }
    }

    /// Runs `tool` through the CLI executor, interrupts the turn once the command's child
    /// wrote its PID to `pid_file`, and checks that the call returns promptly with an
    /// interrupted result and that the child is gone with the shell that started it.
    fn assert_interrupt_kills_the_process_tree(tool: &str, input: &Value, pid_file: &Path) {
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let watched = pid_file.to_path_buf();
        let interrupter = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let pid = loop {
                if let Some(pid) = std::fs::read_to_string(&watched)
                    .ok()
                    .and_then(|text| text.trim().parse::<u32>().ok())
                {
                    break pid;
                }
                assert!(
                    Instant::now() < deadline,
                    "the command never started its child"
                );
                thread::sleep(Duration::from_millis(50));
            };
            let running_before = is_running(pid);
            trigger.cancel();
            (pid, running_before, Instant::now())
        });
        let mut executor = CliToolExecutor::new(None, false, GlobalToolRegistry::builtin(), None);
        let result = executor.execute_with_cancellation(tool, &input.to_string(), &cancel);
        let returned = Instant::now();
        let (pid, running_before, interrupted_at) = interrupter.join().expect("interrupter");
        let _ = std::fs::remove_file(pid_file);

        assert!(running_before, "child {pid} runs until the interrupt");
        let error = result.expect_err("a cancelled command is an error result");
        let output: Value = serde_json::from_str(&error.to_string()).expect("the JSON envelope");
        assert_eq!(output["interrupted"], true, "{output}");
        assert_eq!(output["returnCodeInterpretation"], "cancelled", "{output}");
        let took = returned.saturating_duration_since(interrupted_at);
        assert!(
            took < Duration::from_secs(5),
            "returned {took:?} after the interrupt"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while is_running(pid) {
            assert!(
                Instant::now() < deadline,
                "child {pid} still runs after the interrupt"
            );
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn an_interrupt_kills_a_running_bash_command_and_its_children() {
        let pid_file = pid_file("bash");
        // The shell starts a child that would run for 30 s; the child reports its PID.
        let command = if cfg!(windows) {
            format!(
                "powershell -NoProfile -NonInteractive -Command \"Set-Content -LiteralPath '{}' -Value $PID; Start-Sleep -Seconds 30\"",
                pid_file.display()
            )
        } else {
            format!("sleep 30 & echo $! > '{}'; wait", pid_file.display())
        };
        assert_interrupt_kills_the_process_tree(
            "bash",
            &json!({ "command": command, "timeout": 90_000 }),
            &pid_file,
        );
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn an_interrupt_kills_a_running_powershell_command_and_its_children() {
        let shell = if cfg!(windows) { "powershell" } else { "pwsh" };
        if Command::new(shell)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .is_err()
        {
            eprintln!("skipping: {shell} is not installed");
            return;
        }
        let pid_file = pid_file("powershell");
        let (program, arguments) = if cfg!(windows) {
            ("ping.exe", "'-n','30','127.0.0.1'")
        } else {
            ("sleep", "'30'")
        };
        let command = format!(
            "$child = Start-Process -FilePath {program} -ArgumentList {arguments} -NoNewWindow -PassThru; Set-Content -LiteralPath '{}' -Value $child.Id; $child.WaitForExit()",
            pid_file.display()
        );
        assert_interrupt_kills_the_process_tree(
            "PowerShell",
            &json!({ "command": command, "timeout": 90_000 }),
            &pid_file,
        );
    }

    /// An MCP server (newline-delimited JSON-RPC) with `echo`, which reports the
    /// arguments it got, and `wait`, which writes its PID to `argv[1]` and sleeps.
    const MCP_FIXTURE: &str = r"import json, os, sys, time
pid_file = sys.argv[1]
while True:
    line = sys.stdin.readline()
    if not line:
        break
    if not line.strip():
        continue
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request['method']
    if method == 'initialize':
        result = {'protocolVersion': request['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1.0.0'}}
    elif method == 'tools/list':
        result = {'tools': [
            {'name': 'echo', 'description': 'Reports its arguments', 'inputSchema': {'type': 'object', 'properties': {'text': {'type': 'string'}}}},
            {'name': 'wait', 'description': 'Sleeps for a minute', 'inputSchema': {'type': 'object', 'properties': {}}},
        ]}
    elif method == 'tools/call':
        args = request['params'].get('arguments')
        if request['params']['name'] == 'wait':
            with open(pid_file, 'w') as out:
                out.write(str(os.getpid()))
            time.sleep(60)
        result = {'content': [{'type': 'text', 'text': type(args).__name__ + ':' + json.dumps(args)}], 'isError': False}
    else:
        sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': method}}) + '\n')
        sys.stdout.flush()
        continue
    sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}) + '\n')
    sys.stdout.flush()
";

    /// The Python that runs [`MCP_FIXTURE`]: the `py -3` launcher on Windows, or `python`
    /// where only that is installed.
    fn python() -> (&'static str, Vec<String>) {
        if !cfg!(windows) {
            return ("python3", Vec::new());
        }
        if Command::new("py").arg("--version").output().is_ok() {
            ("py", vec!["-3".to_string()])
        } else {
            ("python", Vec::new())
        }
    }

    /// `MCPTool` arguments sent as JSON text reach the server as an object, and an
    /// interrupt abandons a running MCP call instead of waiting for it (it held up the
    /// turn and shutdown for 42 s): the call returns an "interrupted" error, the server
    /// is stopped, and the next call starts it again.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn mcp_calls_take_arguments_as_text_and_an_interrupt_abandons_them() {
        let root = pid_file("mcp-workspace").with_extension("");
        let workspace = root.join("workspace");
        let config_home = root.join("config");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&config_home).expect("config home");
        let script = root.join("fixture-mcp.py");
        std::fs::write(&script, MCP_FIXTURE).expect("fixture");
        let wait_pid = root.join("wait.pid");
        let (python, mut args) = python();
        args.push(script.display().to_string());
        args.push(wait_pid.display().to_string());
        std::fs::write(
            config_home.join("settings.json"),
            json!({
                "defaultMcpServers": false,
                "mcpServers": { "alpha": { "command": python, "args": args } }
            })
            .to_string(),
        )
        .expect("settings");
        let loader = runtime::ConfigLoader::new(&workspace, &config_home);
        let config = loader.load().expect("config");
        let state = crate::build_runtime_plugin_state_with_loader(&workspace, &loader, &config)
            .expect("plugin state");
        let mut executor = CliToolExecutor::new(
            None,
            false,
            state.tool_registry.clone(),
            state.mcp_state.clone(),
        );

        // As text, parsed; as an object, unchanged.
        let from_text = executor
            .execute(
                "MCPTool",
                &json!({"qualifiedName": "mcp__alpha__echo", "arguments": "{\"text\": \"hi\"}"})
                    .to_string(),
            )
            .expect("the call runs");
        assert!(
            from_text.contains(r#"dict:{\"text\": \"hi\"}"#),
            "{from_text}"
        );
        let from_object = executor
            .execute(
                "MCPTool",
                &json!({"qualifiedName": "mcp__alpha__echo", "arguments": {"text": "hi"}})
                    .to_string(),
            )
            .expect("the call runs");
        assert_eq!(from_object, from_text);
        let refused = executor
            .execute(
                "MCPTool",
                &json!({"qualifiedName": "mcp__alpha__echo", "arguments": "text=hi"}).to_string(),
            )
            .expect_err("not an object");
        assert!(
            refused.to_string().contains("must be a JSON object"),
            "{refused}"
        );

        // Interrupted while the server works on the call.
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        let watched = wait_pid.clone();
        let interrupter = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            let pid = loop {
                if let Some(pid) = std::fs::read_to_string(&watched)
                    .ok()
                    .and_then(|text| text.trim().parse::<u32>().ok())
                {
                    break pid;
                }
                assert!(
                    Instant::now() < deadline,
                    "the call never reached the server"
                );
                thread::sleep(Duration::from_millis(50));
            };
            trigger.cancel();
            (pid, Instant::now())
        });
        let result = executor.execute_with_cancellation("mcp__alpha__wait", "{}", &cancel);
        let returned = Instant::now();
        let (pid, interrupted_at) = interrupter.join().expect("interrupter");
        let error = result.expect_err("an abandoned call is an error result");
        assert!(error.to_string().contains("interrupted"), "{error}");
        let took = returned.saturating_duration_since(interrupted_at);
        assert!(
            took < Duration::from_secs(5),
            "returned {took:?} after the interrupt"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while is_running(pid) {
            assert!(Instant::now() < deadline, "server {pid} still runs");
            thread::sleep(Duration::from_millis(100));
        }

        // The next call starts the server again.
        let again = executor
            .execute("mcp__alpha__echo", r#"{"text":"again"}"#)
            .expect("the server restarts");
        assert!(again.contains("again"), "{again}");

        if let Some(mcp_state) = state.mcp_state {
            let _ = mcp_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .shutdown();
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
