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
        let (pending_mcp_servers, mcp_degraded) =
            self.mcp_state.as_ref().map_or((None, None), |state| {
                let state = state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                (state.pending_servers(), state.degraded_report())
            });
        serde_json::to_string_pretty(&self.tool_registry.search(
            &input.query,
            input.max_results.unwrap_or(5),
            pending_mcp_servers,
            mcp_degraded,
        ))
        .map_err(|error| ToolError::new(error.to_string()))
    }

    pub fn execute_runtime_tool(
        &self,
        tool_name: &str,
        value: serde_json::Value,
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
                mcp_state.call_tool(&qualified_name, input.arguments)
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
            _ => mcp_state.call_tool(tool_name, Some(value)),
        }
    }
}

impl ToolExecutor for CliToolExecutor {
    fn execute(&mut self, tool_name: &str, input: &str) -> Result<String, ToolError> {
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
            self.execute_runtime_tool(tool_name, value)
        } else {
            self.tool_registry
                .execute(tool_name, &value)
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
    writeln!(out, "  neuron auth <login|logout|status>")?;
    writeln!(
        out,
        "      Connect your Zero-X account to the coding gateway"
    )?;
    writeln!(
        out,
        "  neuron [--model MODEL] [--allowedTools TOOL[,TOOL...]]"
    )?;
    writeln!(out, "      Start the interactive REPL")?;
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
    writeln!(out, "  npm run tui -- [-- --model MODEL]")?;
    writeln!(out, "      Launch the optional React/Ink terminal frontend")?;
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
    writeln!(out, "  neuron --model claude-opus \"summarize this repo\"")?;
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
