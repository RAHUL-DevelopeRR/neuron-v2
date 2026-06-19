use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use api::{
    ContentBlockDelta, InputContentBlock, InputMessage, MessageRequest, OutputContentBlock,
    PromptCache, ProviderClient as ApiProviderClient, StreamEvent as ApiStreamEvent, ToolChoice,
    ToolDefinition, ToolResultContentBlock,
};
use commands::{
    classify_skills_slash_command, handle_agents_slash_command, handle_agents_slash_command_json,
    handle_mcp_slash_command, handle_mcp_slash_command_json, handle_plugins_slash_command,
    handle_skills_slash_command, handle_skills_slash_command_json, render_slash_command_help,
    render_slash_command_help_filtered, resolve_skill_invocation, resume_supported_slash_commands,
    slash_command_specs, validate_slash_command_input, SkillSlashDispatch, SlashCommand,
};
use runtime::{
    format_stale_base_warning, format_usd, pricing_for_model, ApiClient, ApiRequest,
    AssistantEvent, CompactionConfig, ConfigLoader, ConfigSource, ContentBlock,
    ConversationMessage, ConversationRuntime, McpTool, MessageRole, PermissionMode,
    PermissionPolicy, ResolvedPermissionMode, RuntimeError, Session, TokenUsage, ToolError,
    ToolExecutor, UsageTracker,
};
use serde_json::json;
use tools::{
    execute_tool, mvp_tool_specs, GlobalToolRegistry, RuntimeToolDefinition, ToolSearchOutput,
};

use crate::git_workspace::*;
use crate::*;

impl LiveCli {
    pub(crate) fn new(
        model: String,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        permission_mode: PermissionMode,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let system_prompt = build_system_prompt()?;
        let session_state = new_cli_session()?;
        let session = create_managed_session_handle(&session_state.session_id)?;
        let runtime = build_runtime(
            session_state.with_persistence_path(session.path.clone()),
            &session.id,
            model.clone(),
            system_prompt.clone(),
            enable_tools,
            true,
            allowed_tools.clone(),
            permission_mode,
            None,
        )?;
        let cli = Self {
            model,
            allowed_tools,
            permission_mode,
            plan_mode: false,
            plan_just_exited: false,
            orchestration_mode: None,
            system_prompt,
            runtime,
            session,
            prompt_history: Vec::new(),
        };
        cli.persist_session()?;
        Ok(cli)
    }

    pub(crate) fn set_reasoning_effort(&mut self, effort: Option<String>) {
        if let Some(rt) = self.runtime.runtime.as_mut() {
            rt.api_client_mut().set_reasoning_effort(effort);
        }
    }

    pub(crate) fn startup_banner(&self) -> String {
        // ── Option 3: Block █ borders — premium, no box-drawing chars ──
        // Faithfully ported from neuron_banner_fixed.py Option 3.
        // Uses brand::strip_ansi_len() for ANSI-aware padding (the core fix).
        use crate::brand::*;

        let cwd = env::current_dir().map_or_else(
            |_| "<unknown>".to_string(),
            |path| {
                let s = path.display().to_string();
                if s.len() > 50 {
                    format!("~/{}", s.rsplit_once(['/', '\\']).map_or(&*s, |p| p.1))
                } else {
                    s
                }
            },
        );
        let model_short = self.model.split('/').last().unwrap_or(&self.model);
        let quota = crate::quota::QuotaState::load();
        let quota_str = quota.display_compact();

        // Resolve the actual provider label from the struct field or env.
        // Check OPENAI_BASE_URL to detect OpenRouter (configure_provider_for_model
        // injects OpenRouter creds into OPENAI_API_KEY, so we can't rely on that alone).
        let provider_label = if let Ok(azure_key) = env::var("AZURE_OPENAI_API_KEY") {
            if !azure_key.is_empty() && !quota.is_azure_exhausted() {
                "Azure"
            } else {
                "OpenRouter"
            }
        } else if env::var("OPENAI_BASE_URL").map_or(false, |u| u.contains("openrouter.ai"))
            || self.model.ends_with(":free")
            || self.model.ends_with(":beta")
            || self.model.ends_with(":extended")
        {
            "OpenRouter"
        } else if env::var("OPENAI_API_KEY").map_or(false, |k| !k.is_empty()) {
            "OpenAI"
        } else {
            "OpenRouter"
        };

        // ── Block-character letter definitions (8 cols × 5 rows each) ──
        // N (blue)
        let letter_n_upper: &[&str] = &["██▄   ██", "████  ██", "██ ██ ██", "██  ████", "██   ▀██"];
        // e (red)
        let letter_e: &[&str] = &["        ", "  ▄██▄  ", " █▄▄▄█▀ ", " █▀▀▀▀  ", "  ▀██▀  "];
        // u (orange)
        let letter_u: &[&str] = &["        ", " ██  ██ ", " ██  ██ ", " ██  ██ ", "  ▀██▀  "];
        // r (orange)
        let letter_r: &[&str] = &["        ", " ██▄▄▄  ", " ███▀▀  ", " ██     ", " ██     "];
        // o (green)
        let letter_o: &[&str] = &["        ", "  ▄██▄  ", " ██  ██ ", " ██  ██ ", "  ▀██▀  "];
        // n (green)
        let letter_n_lower: &[&str] = &["        ", " ██▄▄█  ", " ██  ██ ", " ██  ██ ", " ██  ██ "];

        let letters: &[(&[&str], &str)] = &[
            (letter_n_upper, BLUE),
            (letter_e, RED),
            (letter_u, ORANGE),
            (letter_r, ORANGE),
            (letter_o, GREEN),
            (letter_n_lower, GREEN),
        ];

        // ── Colorize: paint block chars (█▄▀▓▒░▐▟▙▜▛) with color, spaces stay plain ──
        fn colorize_row(row: &str, color: &str) -> String {
            let mut out = String::new();
            for ch in row.chars() {
                if "█▄▀▓▒░▐▟▙▜▛".contains(ch) {
                    out.push_str("\x1b[1m");
                    out.push_str(color);
                    out.push(ch);
                    out.push_str("\x1b[0m");
                } else {
                    out.push(ch);
                }
            }
            out
        }

        // ── Compose logo: join all letters side-by-side with 1-char gap ──
        let mut logo_lines = Vec::new();
        for row_idx in 0..5 {
            let mut parts = Vec::new();
            for (rows, color) in letters {
                parts.push(colorize_row(rows[row_idx], color));
            }
            logo_lines.push(parts.join(" "));
        }

        // ── ANSI-aware padding helper (same logic as Python pad_right) ──
        fn pad_right(s: &str, total_visible: usize) -> String {
            let current = crate::brand::strip_ansi_len(s);
            if current < total_visible {
                format!("{}{}", s, " ".repeat(total_visible - current))
            } else {
                s.to_string()
            }
        }

        // ── Build the banner ──
        let w: usize = 60; // inner visible width
        let b = BLUE;
        let d = DIM;
        let r = R;
        let bd = BOLD;

        let mut lines = Vec::new();

        // Top border: solid block row
        lines.push(format!("  {bd}{b}{bar}{r}", bar = "█".repeat(w + 2)));

        // Empty row
        lines.push(format!("  {bd}{b}█{r}{sp}{bd}{b}█{r}", sp = " ".repeat(w)));

        // Logo rows
        for logo_row in &logo_lines {
            let content = format!("  {}", logo_row);
            let padded = pad_right(&content, w);
            lines.push(format!("  {bd}{b}█{r}{padded}{bd}{b}█{r}"));
        }

        // Empty row after logo
        lines.push(format!("  {bd}{b}█{r}{sp}{bd}{b}█{r}", sp = " ".repeat(w)));

        // Info line: model · provider · quota
        let info_line = format!(
            "   {GREEN}{model}{r} {d}·{r} {b}{prov}{r} {d}· Quota:{r} {ORANGE}{qs}{r}",
            model = model_short,
            prov = provider_label,
            qs = quota_str,
        );
        let padded_info = pad_right(&info_line, w);
        lines.push(format!("  {bd}{b}█{r}{padded_info}{bd}{b}█{r}"));

        // CWD line
        let cwd_line = format!("   {d}{cwd}{r}");
        let padded_cwd = pad_right(&cwd_line, w);
        lines.push(format!("  {bd}{b}█{r}{padded_cwd}{bd}{b}█{r}"));

        // Empty row
        lines.push(format!("  {bd}{b}█{r}{sp}{bd}{b}█{r}", sp = " ".repeat(w)));

        // Bottom border
        lines.push(format!("  {bd}{b}{bar}{r}", bar = "█".repeat(w + 2)));

        lines.join("\n")
    }

    pub(crate) fn repl_completion_candidates(
        &self,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        Ok(slash_command_completion_candidates_with_sessions(
            &self.model,
            Some(&self.session.id),
            list_managed_sessions()?
                .into_iter()
                .map(|session| session.id)
                .collect(),
        ))
    }

    pub(crate) fn prepare_turn_runtime(
        &self,
        emit_output: bool,
    ) -> Result<(BuiltRuntime, HookAbortMonitor), Box<dyn std::error::Error>> {
        // â”€â”€ Plan mode: structurally strip write tools â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
        // Instead of asking the LLM "please don't write", we physically
        // remove write tools from the tool list.  The LLM never sees
        // bash, write_file, or edit_file â€” it can only read and search.
        // This is the Claude Code approach: structural gating, not prompting.
        let effective_tools = if self.plan_mode {
            let mut read_only: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for tool in &["read_file", "glob_search", "grep_search", "list_directory"] {
                read_only.insert(tool.to_string());
            }
            Some(read_only)
        } else {
            self.allowed_tools.clone()
        };

        let hook_abort_signal = runtime::HookAbortSignal::new();
        let runtime = build_runtime(
            self.runtime.session().clone(),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            emit_output,
            effective_tools,
            self.permission_mode,
            None,
        )?
        .with_hook_abort_signal(hook_abort_signal.clone());
        let hook_abort_monitor = HookAbortMonitor::spawn(hook_abort_signal);

        Ok((runtime, hook_abort_monitor))
    }

    pub(crate) fn replace_runtime(
        &mut self,
        runtime: BuiltRuntime,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.shutdown_plugins()?;
        self.runtime = runtime;
        Ok(())
    }

    pub(crate) fn run_turn(&mut self, input: &str) -> Result<(), Box<dyn std::error::Error>> {
        // â”€â”€ Plan-mode prompt wrapping â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
        // When plan mode is active, wrap the user's prompt with strict
        // instructions that produce an architecture plan, NOT full code.
        let effective_input: String;
        let actual_input = if self.plan_mode {
            effective_input = format!(
                "[PLAN MODE â€” RESEARCH FIRST, THEN PLAN]\n\
                 You are in plan mode. Follow this two-phase workflow:\n\n\
                 PHASE 1 â€” RESEARCH:\n\
                 Use read_file, glob_search, grep_search to explore the codebase.\n\
                 Understand existing patterns, file structure, and dependencies.\n\
                 Read NEURON.md or README.md if they exist.\n\
                 Do NOT skip this â€” your plan quality depends on real context.\n\n\
                 PHASE 2 â€” STRUCTURED PLAN:\n\
                 After researching, produce a detailed implementation plan:\n\n\
                 ## Goal\n\
                 What we are building and why (2-3 sentences).\n\n\
                 ## Current State\n\
                 What exists now â€” relevant files, patterns, tech stack found.\n\n\
                 ## Architecture\n\
                 For EACH file (new or modified):\n\
                 ### `filename`\n\
                 - Purpose: what this file does\n\
                 - Key functions/components to create or modify\n\
                 - How it connects to other files (data flow)\n\n\
                 ## Implementation Steps\n\
                 Numbered, detailed steps. Each step should explain:\n\
                 - What to do\n\
                 - Why this approach (design rationale)\n\
                 - Edge cases to handle\n\n\
                 ## Dependencies\n\
                 Packages needed with versions.\n\n\
                 ## Risks & Edge Cases\n\
                 What could go wrong, browser compat, error handling.\n\n\
                 ## Verification\n\
                 Specific test steps to validate the implementation.\n\n\
                 RULES:\n\
                 - Do NOT generate full file contents\n\
                 - Code snippets max 3-5 lines (illustrative only)\n\
                 - Be detailed in WHAT and WHY, not in raw code\n\n\
                 User's request: {}\n",
                input
            );
            effective_input.as_str()
        } else if self.plan_just_exited {
            // First prompt after /plan off â€” clear the LLM's context
            self.plan_just_exited = false;
            effective_input = format!(
                "[PLAN MODE ENDED â€” FULL ACCESS RESTORED]\n\
                 Plan mode has been turned OFF. You now have full write access.\n\
                 You can and SHOULD use write_file, bash, and all tools to implement.\n\
                 Execute the changes directly â€” do not ask for permission.\n\n\
                 User's request: {}\n",
                input
            );
            effective_input.as_str()
        } else if let Some(ref mode) = self.orchestration_mode {
            // â”€â”€ Orchestration mode â€” delegates to orchestrator module â”€â”€
            let api_key = orchestrator::azure_api_key();
            effective_input = match mode.as_str() {
                "divide" => orchestrator::build_divide_prompt(input),
                "chain" => orchestrator::run_chain(&api_key, input),
                "power" => orchestrator::run_power(&api_key, input),
                _ => input.to_string(),
            };
            effective_input.as_str()
        } else {
            input
        };

        let (mut runtime, hook_abort_monitor) = self.prepare_turn_runtime(true)?;
        let mut spinner = Spinner::new();
        let mut stdout = io::stdout();
        let spinner_label = if self.plan_mode {
            "\x1b[38;2;65;105;195m\u{25E6}\x1b[0m Planning..."
        } else if self.orchestration_mode.is_some() {
            match self.orchestration_mode.as_deref() {
                Some("divide") => "\x1b[38;2;240;160;40m\u{25C8}\x1b[0m Dividing...",
                Some("chain") => "\x1b[38;2;65;105;195m\u{25C9}\x1b[0m Chaining...",
                Some("power") => "\x1b[38;2;200;50;40m\u{25B8}\x1b[0m Power mode...",
                _ => "\x1b[38;2;65;105;195m\u{25E6}\x1b[0m Thinking...",
            }
        } else {
            "\x1b[38;2;65;105;195m\u{25E6}\x1b[0m Thinking..."
        };
        spinner.tick(
            spinner_label,
            TerminalRenderer::new().color_theme(),
            &mut stdout,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = runtime.run_turn(actual_input, Some(&mut permission_prompter));
        hook_abort_monitor.stop();
        match result {
            Ok(summary) => {
                self.replace_runtime(runtime)?;
                spinner.finish("Done", TerminalRenderer::new().color_theme(), &mut stdout)?;
                println!();

                // â”€â”€ Record Azure quota usage â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
                // Persist output token count to ~/.neuroncli/quota.json
                // so the banner shows accurate usage across sessions.
                if std::env::var("OPENAI_BASE_URL").map_or(false, |u| u.contains("azure")) {
                    let mut quota = crate::quota::QuotaState::load();
                    quota.record_azure_usage(summary.usage.output_tokens as u32);
                }

                if let Some(event) = summary.auto_compaction {
                    println!(
                        "{}",
                        format_auto_compaction_notice(event.removed_message_count)
                    );
                }
                self.persist_session()?;
                Ok(())
            }
            Err(error) => {
                runtime.shutdown_plugins()?;
                spinner.fail(
                    "Request failed",
                    TerminalRenderer::new().color_theme(),
                    &mut stdout,
                )?;
                Err(Box::new(error))
            }
        }
    }

    pub(crate) fn run_turn_with_output(
        &mut self,
        input: &str,
        output_format: CliOutputFormat,
        compact: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match output_format {
            CliOutputFormat::Text if compact => self.run_prompt_compact(input),
            CliOutputFormat::Text => self.run_turn(input),
            CliOutputFormat::Json => self.run_prompt_json(input),
        }
    }

    pub(crate) fn run_prompt_compact(
        &mut self,
        input: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (mut runtime, hook_abort_monitor) = self.prepare_turn_runtime(false)?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = runtime.run_turn(input, Some(&mut permission_prompter));
        hook_abort_monitor.stop();
        let summary = result?;
        self.replace_runtime(runtime)?;
        self.persist_session()?;
        let final_text = final_assistant_text(&summary);
        println!("{final_text}");
        Ok(())
    }

    pub(crate) fn run_prompt_json(
        &mut self,
        input: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (mut runtime, hook_abort_monitor) = self.prepare_turn_runtime(false)?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let result = runtime.run_turn(input, Some(&mut permission_prompter));
        hook_abort_monitor.stop();
        let summary = result?;
        self.replace_runtime(runtime)?;
        self.persist_session()?;
        println!(
            "{}",
            json!({
                "message": final_assistant_text(&summary),
                "model": self.model,
                "iterations": summary.iterations,
                "auto_compaction": summary.auto_compaction.map(|event| json!({
                    "removed_messages": event.removed_message_count,
                    "notice": format_auto_compaction_notice(event.removed_message_count),
                })),
                "tool_uses": collect_tool_uses(&summary),
                "tool_results": collect_tool_results(&summary),
                "prompt_cache_events": collect_prompt_cache_events(&summary),
                "usage": {
                    "input_tokens": summary.usage.input_tokens,
                    "output_tokens": summary.usage.output_tokens,
                    "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
                    "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
                },
                "estimated_cost": format_usd(
                    summary.usage.estimate_cost_usd_with_pricing(
                        pricing_for_model(&self.model)
                            .unwrap_or_else(runtime::ModelPricing::default_sonnet_tier)
                    ).total_cost_usd()
                )
            })
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle_repl_command(
        &mut self,
        command: SlashCommand,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(match command {
            SlashCommand::Help => {
                println!("{}", render_repl_help());
                false
            }
            SlashCommand::Status => {
                self.print_status();
                false
            }
            SlashCommand::Bughunter { scope } => {
                self.run_bughunter(scope.as_deref())?;
                false
            }
            SlashCommand::Commit => {
                self.run_commit(None)?;
                false
            }
            SlashCommand::Pr { context } => {
                self.run_pr(context.as_deref())?;
                false
            }
            SlashCommand::Issue { context } => {
                self.run_issue(context.as_deref())?;
                false
            }
            SlashCommand::Ultraplan { task } => {
                self.run_ultraplan(task.as_deref())?;
                false
            }
            SlashCommand::Teleport { target } => {
                Self::run_teleport(target.as_deref())?;
                false
            }
            SlashCommand::DebugToolCall => {
                self.run_debug_tool_call(None)?;
                false
            }
            SlashCommand::Sandbox => {
                Self::print_sandbox_status();
                false
            }
            SlashCommand::Compact => {
                self.compact()?;
                false
            }
            SlashCommand::Model { model } => self.set_model(model)?,
            SlashCommand::Permissions { mode } => self.set_permissions(mode)?,
            SlashCommand::Clear { confirm } => self.clear_session(confirm)?,
            SlashCommand::Cost => {
                self.print_cost();
                false
            }
            SlashCommand::Resume { session_path } => self.resume_session(session_path)?,
            SlashCommand::Config { section } => {
                Self::print_config(section.as_deref())?;
                false
            }
            SlashCommand::Mcp { action, target } => {
                let args = match (action.as_deref(), target.as_deref()) {
                    (None, None) => None,
                    (Some(action), None) => Some(action.to_string()),
                    (Some(action), Some(target)) => Some(format!("{action} {target}")),
                    (None, Some(target)) => Some(target.to_string()),
                };
                Self::print_mcp(args.as_deref(), CliOutputFormat::Text)?;
                false
            }
            SlashCommand::Memory => {
                Self::print_memory()?;
                false
            }
            SlashCommand::Init => {
                run_init(CliOutputFormat::Text)?;
                false
            }
            SlashCommand::Diff => {
                Self::print_diff()?;
                false
            }
            SlashCommand::Version => {
                Self::print_version(CliOutputFormat::Text);
                false
            }
            SlashCommand::Export { path } => {
                self.export_session(path.as_deref())?;
                false
            }
            SlashCommand::Session { action, target } => {
                self.handle_session_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Plugins { action, target } => {
                self.handle_plugins_command(action.as_deref(), target.as_deref())?
            }
            SlashCommand::Agents { args } => {
                Self::print_agents(args.as_deref(), CliOutputFormat::Text)?;
                false
            }
            SlashCommand::Skills { args } => {
                match classify_skills_slash_command(args.as_deref()) {
                    SkillSlashDispatch::Invoke(prompt) => self.run_turn(&prompt)?,
                    SkillSlashDispatch::Local => {
                        Self::print_skills(args.as_deref(), CliOutputFormat::Text)?;
                    }
                }
                false
            }
            SlashCommand::Doctor => {
                println!("{}", render_doctor_report()?.render());
                false
            }
            SlashCommand::History { count } => {
                self.print_prompt_history(count.as_deref());
                false
            }
            SlashCommand::Stats => {
                let usage = UsageTracker::from_session(self.runtime.session()).cumulative_usage();
                println!("{}", format_cost_report(usage));
                false
            }
            SlashCommand::Login
            | SlashCommand::Logout
            | SlashCommand::Vim
            | SlashCommand::Upgrade
            | SlashCommand::Share
            | SlashCommand::Feedback
            | SlashCommand::Files
            | SlashCommand::Fast
            | SlashCommand::Exit
            | SlashCommand::Summary
            | SlashCommand::Desktop
            | SlashCommand::Brief
            | SlashCommand::Advisor
            | SlashCommand::Stickers
            | SlashCommand::Insights
            | SlashCommand::Thinkback
            | SlashCommand::ReleaseNotes
            | SlashCommand::SecurityReview
            | SlashCommand::Keybindings
            | SlashCommand::PrivacySettings
            | SlashCommand::Review { .. }
            | SlashCommand::Tasks { .. }
            | SlashCommand::Theme { .. }
            | SlashCommand::Voice { .. }
            | SlashCommand::Usage { .. }
            | SlashCommand::Rename { .. }
            | SlashCommand::Copy { .. }
            | SlashCommand::Hooks { .. }
            | SlashCommand::Context { .. }
            | SlashCommand::Color { .. }
            | SlashCommand::Effort { .. }
            | SlashCommand::Branch { .. }
            | SlashCommand::Rewind { .. }
            | SlashCommand::Ide { .. }
            | SlashCommand::Tag { .. }
            | SlashCommand::OutputStyle { .. }
            | SlashCommand::AddDir { .. } => {
                let cmd_name = command.slash_name();
                eprintln!("{cmd_name} is not yet implemented in this build.");
                false
            }
            // â”€â”€ /plan [on|off] â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            // Toggles read-only plan mode.  In plan mode the agent can
            // analyze the codebase, search files, and create plans but
            // is restricted from modifying files or executing commands.
            SlashCommand::Plan { mode } => {
                match mode.as_deref().map(str::trim) {
                    Some("on") | None => {
                        if self.plan_mode {
                            eprintln!("\x1b[33m[plan]\x1b[0m Plan mode is already active.");
                        } else {
                            self.plan_mode = true;
                            self.permission_mode = PermissionMode::ReadOnly;
                            eprintln!("\x1b[33m[plan]\x1b[0m \x1b[1mPlan mode ON\x1b[0m");
                            eprintln!(
                                "  \x1b[2mThe agent will generate architecture plans, not full code.\x1b[0m"
                            );
                            eprintln!(
                                "  \x1b[2mType /plan off to exit, then ask the agent to execute.\x1b[0m"
                            );
                        }
                    }
                    Some("off") => {
                        if !self.plan_mode {
                            eprintln!("\x1b[32m>\x1b[0m Plan mode is not active.");
                        } else {
                            self.plan_mode = false;
                            self.plan_just_exited = true;
                            self.permission_mode = PermissionMode::DangerFullAccess;
                            eprintln!(
                                "\x1b[32m>\x1b[0m \x1b[1mPlan mode OFF\x1b[0m \x1b[2m-- full access restored\x1b[0m"
                            );
                            eprintln!(
                                "  \x1b[2mType \"execute the plan\" to implement it now.\x1b[0m"
                            );
                        }
                    }
                    Some(other) => {
                        eprintln!("Unknown plan argument: \"{other}\". Usage: /plan [on|off]");
                    }
                }
                false
            }
            // â”€â”€ Orchestration modes â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
            SlashCommand::Divide { task } => {
                match task.as_deref().map(str::trim) {
                    Some("off") => {
                        self.orchestration_mode = None;
                        eprintln!("\x1b[36m[divide]\x1b[0m \x1b[1mDivide mode OFF\x1b[0m");
                    }
                    _ => {
                        self.orchestration_mode = Some("divide".to_string());
                        eprintln!(
                            "\x1b[36m[divide]\x1b[0m \x1b[1mDivide mode ON\x1b[0m \x1b[2m-- multi-file parallel strategy\x1b[0m"
                        );
                        eprintln!(
                            "  \x1b[2mEach file/module assigned to a different model agent.\x1b[0m"
                        );
                        eprintln!("  \x1b[2mAn integrator agent stitches outputs together.\x1b[0m");
                        eprintln!("  \x1b[2mType /divide off to deactivate.\x1b[0m");
                    }
                }
                false
            }
            SlashCommand::Chain { task } => {
                match task.as_deref().map(str::trim) {
                    Some("off") => {
                        self.orchestration_mode = None;
                        eprintln!("\x1b[35m[chain]\x1b[0m \x1b[1mChain mode OFF\x1b[0m");
                    }
                    _ => {
                        self.orchestration_mode = Some("chain".to_string());
                        eprintln!(
                            "\x1b[35m[chain]\x1b[0m \x1b[1mChain mode ON\x1b[0m \x1b[2m-- architect > coder > reviewer\x1b[0m"
                        );
                        eprintln!("  \x1b[2mPhase 1: Architect agent designs the approach.\x1b[0m");
                        eprintln!("  \x1b[2mPhase 2: Coder agent implements the design.\x1b[0m");
                        eprintln!(
                            "  \x1b[2mPhase 3: Reviewer agent hardens and fixes bugs.\x1b[0m"
                        );
                        eprintln!("  \x1b[2mType /chain off to deactivate.\x1b[0m");
                    }
                }
                false
            }
            SlashCommand::Power { task } => {
                match task.as_deref().map(str::trim) {
                    Some("off") => {
                        self.orchestration_mode = None;
                        eprintln!("\x1b[31m[power]\x1b[0m \x1b[1mPower mode OFF\x1b[0m");
                    }
                    _ => {
                        self.orchestration_mode = Some("power".to_string());
                        eprintln!(
                            "\x1b[31m[power]\x1b[0m \x1b[1mPower mode ON\x1b[0m \x1b[2m-- ensemble merge (maximum quality)\x1b[0m"
                        );
                        eprintln!(
                            "  \x1b[2mAll models generate the same module simultaneously.\x1b[0m"
                        );
                        eprintln!(
                            "  \x1b[2mA merge agent combines the BEST PARTS from each.\x1b[0m"
                        );
                        eprintln!("  \x1b[2mType /power off to deactivate.\x1b[0m");
                    }
                }
                false
            }
            SlashCommand::Unknown(name) => {
                eprintln!("{}", format_unknown_slash_command(&name));
                false
            }
        })
    }

    pub(crate) fn persist_session(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.runtime.session().save_to_path(&self.session.path)?;
        Ok(())
    }

    pub(crate) fn print_status(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        let latest = self.runtime.usage().current_turn_usage();
        println!(
            "{}",
            format_status_report(
                &self.model,
                StatusUsage {
                    message_count: self.runtime.session().messages.len(),
                    turns: self.runtime.usage().turns(),
                    latest,
                    cumulative,
                    estimated_tokens: self.runtime.estimated_tokens(),
                },
                self.permission_mode.as_str(),
                &status_context(Some(&self.session.path)).expect("status context should load"),
            )
        );
    }

    pub(crate) fn record_prompt_history(&mut self, prompt: &str) {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map_or(self.runtime.session().updated_at_ms, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        let entry = PromptHistoryEntry {
            timestamp_ms,
            text: prompt.to_string(),
        };
        self.prompt_history.push(entry);
        if let Err(error) = self.runtime.session_mut().push_prompt_entry(prompt) {
            eprintln!("warning: failed to persist prompt history: {error}");
        }
    }

    pub(crate) fn print_prompt_history(&self, count: Option<&str>) {
        let limit = match parse_history_count(count) {
            Ok(limit) => limit,
            Err(message) => {
                eprintln!("{message}");
                return;
            }
        };
        let session_entries = &self.runtime.session().prompt_history;
        let entries = if session_entries.is_empty() {
            if self.prompt_history.is_empty() {
                collect_session_prompt_history(self.runtime.session())
            } else {
                self.prompt_history
                    .iter()
                    .map(|entry| PromptHistoryEntry {
                        timestamp_ms: entry.timestamp_ms,
                        text: entry.text.clone(),
                    })
                    .collect()
            }
        } else {
            session_entries
                .iter()
                .map(|entry| PromptHistoryEntry {
                    timestamp_ms: entry.timestamp_ms,
                    text: entry.text.clone(),
                })
                .collect()
        };
        println!("{}", render_prompt_history_report(&entries, limit));
    }

    pub(crate) fn print_sandbox_status() {
        let cwd = env::current_dir().expect("current dir");
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader
            .load()
            .unwrap_or_else(|_| runtime::RuntimeConfig::empty());
        println!(
            "{}",
            format_sandbox_report(&resolve_sandbox_status(runtime_config.sandbox(), &cwd))
        );
    }

    pub(crate) fn set_model(
        &mut self,
        model: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(model) = model else {
            println!(
                "{}",
                format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                )
            );
            return Ok(false);
        };

        let model = resolve_model_alias_with_config(&model);

        if model == self.model {
            println!(
                "{}",
                format_model_report(
                    &self.model,
                    self.runtime.session().messages.len(),
                    self.runtime.usage().turns(),
                )
            );
            return Ok(false);
        }

        let previous = self.model.clone();
        let session = self.runtime.session().clone();
        let message_count = session.messages.len();
        let runtime = build_runtime(
            session,
            &self.session.id,
            model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.model.clone_from(&model);
        println!(
            "{}",
            format_model_switch_report(&previous, &model, message_count)
        );
        Ok(true)
    }

    pub(crate) fn set_permissions(
        &mut self,
        mode: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(mode) = mode else {
            println!(
                "{}",
                format_permissions_report(self.permission_mode.as_str())
            );
            return Ok(false);
        };

        let normalized = normalize_permission_mode(&mode).ok_or_else(|| {
            format!(
                "unsupported permission mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            )
        })?;

        if normalized == self.permission_mode.as_str() {
            println!("{}", format_permissions_report(normalized));
            return Ok(false);
        }

        let previous = self.permission_mode.as_str().to_string();
        let session = self.runtime.session().clone();
        self.permission_mode = permission_mode_from_label(normalized);
        let runtime = build_runtime(
            session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        println!(
            "{}",
            format_permissions_switch_report(&previous, normalized)
        );
        Ok(true)
    }

    pub(crate) fn clear_session(
        &mut self,
        confirm: bool,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        if !confirm {
            println!(
                "clear: confirmation required; run /clear --confirm to start a fresh session."
            );
            return Ok(false);
        }

        let previous_session = self.session.clone();
        let session_state = new_cli_session()?;
        self.session = create_managed_session_handle(&session_state.session_id)?;
        let runtime = build_runtime(
            session_state.with_persistence_path(self.session.path.clone()),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        println!(
            "Session cleared\n  Mode             fresh session\n  Previous session {}\n  Resume previous  /resume {}\n  Preserved model  {}\n  Permission mode  {}\n  New session      {}\n  Session file     {}",
            previous_session.id,
            previous_session.id,
            self.model,
            self.permission_mode.as_str(),
            self.session.id,
            self.session.path.display(),
        );
        Ok(true)
    }

    pub(crate) fn print_cost(&self) {
        let cumulative = self.runtime.usage().cumulative_usage();
        println!("{}", format_cost_report(cumulative));
    }

    pub(crate) fn resume_session(
        &mut self,
        session_path: Option<String>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let Some(session_ref) = session_path else {
            println!("{}", render_resume_usage());
            return Ok(false);
        };

        let (handle, session) = load_session_reference(&session_ref)?;
        let message_count = session.messages.len();
        let session_id = session.session_id.clone();
        let runtime = build_runtime(
            session,
            &handle.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.session = SessionHandle {
            id: session_id,
            path: handle.path,
        };
        println!(
            "{}",
            format_resume_report(
                &self.session.path.display().to_string(),
                message_count,
                self.runtime.usage().turns(),
            )
        );
        Ok(true)
    }

    pub(crate) fn print_config(section: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_config_report(section)?);
        Ok(())
    }

    pub(crate) fn print_memory() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_memory_report()?);
        Ok(())
    }

    pub(crate) fn print_agents(
        args: Option<&str>,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        match output_format {
            CliOutputFormat::Text => println!("{}", handle_agents_slash_command(args, &cwd)?),
            CliOutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&handle_agents_slash_command_json(args, &cwd)?)?
            ),
        }
        Ok(())
    }

    pub(crate) fn print_mcp(
        args: Option<&str>,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // `neuron mcp serve` starts a stdio MCP server exposing neuron's built-in
        // tools. All other `mcp` subcommands fall through to the existing
        // configured-server reporter (`list`, `status`, ...).
        if matches!(args.map(str::trim), Some("serve")) {
            return run_mcp_serve();
        }
        let cwd = env::current_dir()?;
        match output_format {
            CliOutputFormat::Text => println!("{}", handle_mcp_slash_command(args, &cwd)?),
            CliOutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&handle_mcp_slash_command_json(args, &cwd)?)?
            ),
        }
        Ok(())
    }

    pub(crate) fn print_skills(
        args: Option<&str>,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        match output_format {
            CliOutputFormat::Text => println!("{}", handle_skills_slash_command(args, &cwd)?),
            CliOutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&handle_skills_slash_command_json(args, &cwd)?)?
            ),
        }
        Ok(())
    }

    pub(crate) fn print_plugins(
        action: Option<&str>,
        target: Option<&str>,
        output_format: CliOutputFormat,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader.load()?;
        let mut manager = build_plugin_manager(&cwd, &loader, &runtime_config);
        let result = handle_plugins_slash_command(action, target, &mut manager)?;
        match output_format {
            CliOutputFormat::Text => println!("{}", result.message),
            CliOutputFormat::Json => println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "kind": "plugin",
                    "action": action.unwrap_or("list"),
                    "target": target,
                    "message": result.message,
                    "reload_runtime": result.reload_runtime,
                }))?
            ),
        }
        Ok(())
    }

    pub(crate) fn print_diff() -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", render_diff_report()?);
        Ok(())
    }

    pub(crate) fn print_version(output_format: CliOutputFormat) {
        let _ = crate::print_version(output_format);
    }

    pub(crate) fn export_session(
        &self,
        requested_path: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let export_path = resolve_export_path(requested_path, self.runtime.session())?;
        fs::write(&export_path, render_export_text(self.runtime.session()))?;
        println!(
            "Export\n  Result           wrote transcript\n  File             {}\n  Messages         {}",
            export_path.display(),
            self.runtime.session().messages.len(),
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn handle_session_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        match action {
            None | Some("list") => {
                println!("{}", render_session_list(&self.session.id)?);
                Ok(false)
            }
            Some("switch") => {
                let Some(target) = target else {
                    println!("Usage: /session switch <session-id>");
                    return Ok(false);
                };
                let (handle, session) = load_session_reference(target)?;
                let message_count = session.messages.len();
                let session_id = session.session_id.clone();
                let runtime = build_runtime(
                    session,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.session = SessionHandle {
                    id: session_id,
                    path: handle.path,
                };
                println!(
                    "Session switched\n  Active session   {}\n  File             {}\n  Messages         {}",
                    self.session.id,
                    self.session.path.display(),
                    message_count,
                );
                Ok(true)
            }
            Some("fork") => {
                let forked = self.runtime.fork_session(target.map(ToOwned::to_owned));
                let parent_session_id = self.session.id.clone();
                let handle = create_managed_session_handle(&forked.session_id)?;
                let branch_name = forked
                    .fork
                    .as_ref()
                    .and_then(|fork| fork.branch_name.clone());
                let forked = forked.with_persistence_path(handle.path.clone());
                let message_count = forked.messages.len();
                forked.save_to_path(&handle.path)?;
                let runtime = build_runtime(
                    forked,
                    &handle.id,
                    self.model.clone(),
                    self.system_prompt.clone(),
                    true,
                    true,
                    self.allowed_tools.clone(),
                    self.permission_mode,
                    None,
                )?;
                self.replace_runtime(runtime)?;
                self.session = handle;
                println!(
                    "Session forked\n  Parent session   {}\n  Active session   {}\n  Branch           {}\n  File             {}\n  Messages         {}",
                    parent_session_id,
                    self.session.id,
                    branch_name.as_deref().unwrap_or("(unnamed)"),
                    self.session.path.display(),
                    message_count,
                );
                Ok(true)
            }
            Some("delete") => {
                let Some(target) = target else {
                    println!("Usage: /session delete <session-id> [--force]");
                    return Ok(false);
                };
                let handle = resolve_session_reference(target)?;
                if handle.id == self.session.id {
                    println!(
                        "delete: refusing to delete the active session '{}'.\nSwitch to another session first with /session switch <session-id>.",
                        handle.id
                    );
                    return Ok(false);
                }
                if !confirm_session_deletion(&handle.id) {
                    println!("delete: cancelled.");
                    return Ok(false);
                }
                delete_managed_session(&handle.path)?;
                println!(
                    "Session deleted\n  Deleted session  {}\n  File             {}",
                    handle.id,
                    handle.path.display(),
                );
                Ok(false)
            }
            Some("delete-force") => {
                let Some(target) = target else {
                    println!("Usage: /session delete <session-id> [--force]");
                    return Ok(false);
                };
                let handle = resolve_session_reference(target)?;
                if handle.id == self.session.id {
                    println!(
                        "delete: refusing to delete the active session '{}'.\nSwitch to another session first with /session switch <session-id>.",
                        handle.id
                    );
                    return Ok(false);
                }
                delete_managed_session(&handle.path)?;
                println!(
                    "Session deleted\n  Deleted session  {}\n  File             {}",
                    handle.id,
                    handle.path.display(),
                );
                Ok(false)
            }
            Some(other) => {
                println!(
                    "Unknown /session action '{other}'. Use /session list, /session switch <session-id>, /session fork [branch-name], or /session delete <session-id> [--force]."
                );
                Ok(false)
            }
        }
    }

    pub(crate) fn handle_plugins_command(
        &mut self,
        action: Option<&str>,
        target: Option<&str>,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        let cwd = env::current_dir()?;
        let loader = ConfigLoader::default_for(&cwd);
        let runtime_config = loader.load()?;
        let mut manager = build_plugin_manager(&cwd, &loader, &runtime_config);
        let result = handle_plugins_slash_command(action, target, &mut manager)?;
        println!("{}", result.message);
        if result.reload_runtime {
            self.reload_runtime_features()?;
        }
        Ok(false)
    }

    pub(crate) fn reload_runtime_features(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let runtime = build_runtime(
            self.runtime.session().clone(),
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.persist_session()
    }

    pub(crate) fn compact(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let result = self.runtime.compact(CompactionConfig::default());
        let removed = result.removed_message_count;
        let kept = result.compacted_session.messages.len();
        let skipped = removed == 0;
        let runtime = build_runtime(
            result.compacted_session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        self.persist_session()?;
        println!("{}", format_compact_report(removed, kept, skipped));
        Ok(())
    }

    pub(crate) fn run_internal_prompt_text_with_progress(
        &self,
        prompt: &str,
        enable_tools: bool,
        progress: Option<InternalPromptProgressReporter>,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let session = self.runtime.session().clone();
        let mut runtime = build_runtime(
            session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            enable_tools,
            false,
            self.allowed_tools.clone(),
            self.permission_mode,
            progress,
        )?;
        let mut permission_prompter = CliPermissionPrompter::new(self.permission_mode);
        let summary = runtime.run_turn(prompt, Some(&mut permission_prompter))?;
        let text = final_assistant_text(&summary).trim().to_string();
        runtime.shutdown_plugins()?;
        Ok(text)
    }

    pub(crate) fn run_internal_prompt_text(
        &self,
        prompt: &str,
        enable_tools: bool,
    ) -> Result<String, Box<dyn std::error::Error>> {
        self.run_internal_prompt_text_with_progress(prompt, enable_tools, None)
    }

    pub(crate) fn run_bughunter(
        &self,
        scope: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_bughunter_report(scope));
        Ok(())
    }

    pub(crate) fn run_ultraplan(
        &self,
        task: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_ultraplan_report(task));
        Ok(())
    }

    pub(crate) fn run_teleport(target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let Some(target) = target.map(str::trim).filter(|value| !value.is_empty()) else {
            println!("Usage: /teleport <symbol-or-path>");
            return Ok(());
        };

        println!("{}", render_teleport_report(target)?);
        Ok(())
    }

    pub(crate) fn run_debug_tool_call(
        &self,
        args: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/debug-tool-call", args)?;
        println!("{}", render_last_tool_debug_report(self.runtime.session())?);
        Ok(())
    }

    pub(crate) fn run_commit(
        &mut self,
        args: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        validate_no_args("/commit", args)?;
        let status = git_output(&["status", "--short", "--branch"])?;
        let summary = parse_git_workspace_summary(Some(&status));
        let branch = parse_git_status_branch(Some(&status));
        if summary.is_clean() {
            println!("{}", format_commit_skipped_report());
            return Ok(());
        }

        println!(
            "{}",
            format_commit_preflight_report(branch.as_deref(), summary)
        );
        Ok(())
    }

    pub(crate) fn run_pr(&self, context: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
        let branch =
            resolve_git_branch_for(&env::current_dir()?).unwrap_or_else(|| "unknown".to_string());
        println!("{}", format_pr_report(&branch, context));
        Ok(())
    }

    pub(crate) fn run_issue(
        &self,
        context: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        println!("{}", format_issue_report(context));
        Ok(())
    }
}
