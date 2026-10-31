//! Status display, help rendering, and sandbox reporting.
//!
//! Extracted from main.rs.

use super::*;
use crate::brand::*;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;

pub(crate) fn render_repl_help() -> String {
    [
        "REPL".to_string(),
        "  /exit                Quit the REPL".to_string(),
        "  /quit                Quit the REPL".to_string(),
        "  Up/Down              Navigate prompt history".to_string(),
        "  Ctrl-R               Reverse-search prompt history".to_string(),
        "  Tab                  Complete commands, modes, and recent sessions".to_string(),
        "  Ctrl-C               Cancel input (exit on empty prompt)".to_string(),
        "  Ctrl-D               Exit the session".to_string(),
        "  Shift+Enter/Ctrl+J   Insert a newline".to_string(),
        "  ?                    Show keyboard shortcuts panel".to_string(),
        "  !<command>           Run a shell command directly".to_string(),
        String::new(),
        "Session".to_string(),
        "  Auto-save            .neuron/sessions/<session-id>.jsonl".to_string(),
        "  Resume latest        /resume latest".to_string(),
        "  Browse sessions      /session list".to_string(),
        "  Show prompt history  /history [count]".to_string(),
        String::new(),
        "Modes".to_string(),
        "  /plan [on|off]       Toggle read-only plan mode".to_string(),
        "  /permissions [mode]  Set permission level".to_string(),
        "  /model [name]        Switch model".to_string(),
        String::new(),
        render_slash_command_help_filtered(STUB_COMMANDS),
    ]
    .join(
        "
",
    )
}

// ── Shortcuts panel ─────────────────────────────────────────────────
// Rendered when the user types `?` at the prompt.  Modeled after
// Claude Code's compact two-column layout so power users can scan
// everything at a glance without scrolling.

pub(crate) fn render_shortcuts_panel() -> String {
    let bc = "\x1b[38;2;100;100;100m"; // border / chrome gray
    let hd = "\x1b[1;38;2;65;105;195m"; // heading blue
    let ky = "\x1b[1;38;2;240;160;40m"; // key gold
    let ds = "\x1b[38;2;160;160;170m"; // description muted
    let dm = "\x1b[2m"; // dim
    let r = "\x1b[0m"; // reset

    // Unicode box-drawing provides a polished, terminal-safe frame
    // that renders cleanly in Windows Terminal, iTerm2, and VS Code.
    let w = 60; // inner width
    let top = format!("  {bc}\u{256d}{}{bc}\u{256e}{r}", "\u{2500}".repeat(w));
    let bot = format!("  {bc}\u{2570}{}{bc}\u{256f}{r}", "\u{2500}".repeat(w));
    let sep = format!("  {bc}\u{251c}{}{bc}\u{2524}{r}", "\u{2500}".repeat(w));
    let blank = format!("  {bc}\u{2502}{r}{}{bc}\u{2502}{r}", " ".repeat(w));

    // Pad a line to fill the box width exactly.
    let row = |left: &str, right: &str| -> String {
        // left column: 28 chars, right column: 28 chars, 4 chars padding
        let left_vis = brand::strip_ansi_len(left);
        let right_vis = brand::strip_ansi_len(right);
        let left_pad = 28usize.saturating_sub(left_vis);
        let right_pad = 28usize.saturating_sub(right_vis);
        format!(
            "  {bc}\u{2502}{r}  {left}{}{right}{}{}  {bc}\u{2502}{r}",
            " ".repeat(left_pad),
            " ".repeat(right_pad),
            "", // no extra padding needed since we account for left/right
        )
    };

    // Color-formatted key-description pair.
    let kv = |key: &str, desc: &str| -> String { format!("{ky}{key:<14}{r} {ds}{desc}{r}") };

    let title_line = format!(
        "  {bc}\u{2502}{r}  {hd}\u{2328}  Neuron Shortcuts{r}{}  {bc}\u{2502}{r}",
        " ".repeat(w - 23)
    );

    [
        top,
        title_line,
        sep.clone(),
        blank.clone(),
        row(&format!("{hd}Navigation{r}"), &format!("{hd}Session{r}")),
        row(
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
        ),
        row(&kv("Up/Down", "History"), &kv("/compact", "Compress ctx")),
        row(&kv("Ctrl+R", "Search"), &kv("/clear", "Reset session")),
        row(&kv("Tab", "Complete"), &kv("/resume", "Resume prev")),
        row(&kv("Ctrl+C", "Cancel"), &kv("/export", "Export session")),
        blank.clone(),
        row(&format!("{hd}Input{r}"), &format!("{hd}Workspace{r}")),
        row(
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
        ),
        row(&kv("Ctrl+D", "Exit"), &kv("/status", "Show status")),
        row(&kv("Ctrl+J", "Newline"), &kv("/diff", "Show changes")),
        row(&kv("Shift+Enter", "Newline"), &kv("/init", "NEURON.md")),
        row(&kv("!cmd", "Shell cmd"), &kv("/commit", "Commit")),
        blank.clone(),
        row(&format!("{hd}Modes{r}"), &format!("{hd}Info{r}")),
        row(
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
        ),
        row(&kv("/plan", "Plan mode"), &kv("/cost", "Token usage")),
        row(
            &kv("/model", "Switch model"),
            &kv("/doctor", "Health check"),
        ),
        row(
            &kv("/permissions", "Set perms"),
            &kv("/version", "Show version"),
        ),
        row(&kv("?", "This panel"), &kv("/help", "Full cmd list")),
        blank.clone(),
        row(&format!("{hd}Orchestration{r}"), &format!("{hd}{r}")),
        row(
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
            &format!("{dm}{}{r}", "\u{2500}".repeat(14)),
        ),
        row(
            &kv("/divide", "Multi-file split"),
            &kv("/chain", "Arch>Code>Review"),
        ),
        row(&kv("/power", "Ensemble merge"), &kv("", "")),
        blank,
        bot,
    ]
    .join("\n")
}

// ── Shell escape ────────────────────────────────────────────────────
// When the user types `!<command>` at the prompt, execute it directly
// in the system shell.  This mirrors Claude Code's `!` prefix behavior
// and avoids burning LLM tokens on simple shell operations.

pub(crate) fn run_shell_escape(cmd: &str) {
    let dm = "\x1b[2m";
    let r = "\x1b[0m";
    let gn = "\x1b[38;2;45;140;60m";
    let rd = "\x1b[31m";

    eprintln!("{dm}\u{2192} Running: {cmd}{r}");

    let result = if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/C", cmd])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
    } else {
        std::process::Command::new("sh")
            .args(["-c", cmd])
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
    };

    match result {
        Ok(status) => {
            let code = status.code().unwrap_or(-1);
            if code == 0 {
                eprintln!("{gn}\u{2713}{r} {dm}(exit 0){r}");
            } else {
                eprintln!("{rd}\u{2717}{r} {dm}(exit {code}){r}");
            }
        }
        Err(err) => eprintln!("{rd}Shell error:{r} {err}"),
    }
}

// ── Mode-aware prompt ───────────────────────────────────────────────
// Dynamically generates the input prompt string to show the current
// permission mode, mirroring how Claude Code displays the active mode.
//
// IMPORTANT: No ANSI escape codes here!  Rustyline counts escape
// sequences as visible characters when calculating cursor position,
// which causes the cursor to drift right.  Keep the prompt plain.
//
// Examples:
//   "> "               (default / full access)
//   "[plan] > "        (plan mode)
//   "[edit] > "        (workspace-write)
//   "[auto] > "        (auto-allow)

pub(crate) fn mode_aware_prompt(
    mode: &PermissionMode,
    plan_mode: bool,
    orchestration_mode: Option<&str>,
) -> String {
    if plan_mode {
        return "[plan] > ".to_string();
    }
    if let Some(orch) = orchestration_mode {
        return match orch {
            "divide" => "[divide] > ".to_string(),
            "chain" => "[chain] > ".to_string(),
            "power" => "[power] > ".to_string(),
            _ => "> ".to_string(),
        };
    }
    match mode {
        PermissionMode::ReadOnly => "[read] > ".to_string(),
        PermissionMode::WorkspaceWrite => "[edit] > ".to_string(),
        PermissionMode::DangerFullAccess => "> ".to_string(),
        PermissionMode::Prompt => "[ask] > ".to_string(),
        PermissionMode::Allow => "[auto] > ".to_string(),
    }
}

pub(crate) fn print_status_snapshot(
    model: &str,
    permission_mode: PermissionMode,
    output_format: CliOutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let usage = StatusUsage {
        message_count: 0,
        turns: 0,
        latest: TokenUsage::default(),
        cumulative: TokenUsage::default(),
        estimated_tokens: 0,
    };
    let context = status_context(None)?;
    match output_format {
        CliOutputFormat::Text => println!(
            "{}",
            format_status_report(model, usage, permission_mode.as_str(), &context)
        ),
        CliOutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&status_json_value(
                Some(model),
                usage,
                permission_mode.as_str(),
                &context,
            ))?
        ),
    }
    Ok(())
}

pub(crate) fn status_json_value(
    model: Option<&str>,
    usage: StatusUsage,
    permission_mode: &str,
    context: &StatusContext,
) -> serde_json::Value {
    json!({
        "kind": "status",
        "model": model.map(crate::provider::shown_model),
        "permission_mode": permission_mode,
        "usage": {
            "messages": usage.message_count,
            "turns": usage.turns,
            "latest_total": usage.latest.total_tokens(),
            "cumulative_input": usage.cumulative.input_tokens,
            "cumulative_output": usage.cumulative.output_tokens,
            "cumulative_total": usage.cumulative.total_tokens(),
            "estimated_tokens": usage.estimated_tokens,
        },
        "workspace": {
            "cwd": context.cwd,
            "project_root": context.project_root,
            "git_branch": context.git_branch,
            "git_state": context.git_summary.headline(),
            "changed_files": context.git_summary.changed_files,
            "staged_files": context.git_summary.staged_files,
            "unstaged_files": context.git_summary.unstaged_files,
            "untracked_files": context.git_summary.untracked_files,
            "session": context.session_path.as_ref().map_or_else(|| "live-repl".to_string(), |path| path.display().to_string()),
            "session_id": context.session_path.as_ref().and_then(|path| {
                // Session files are named <session-id>.jsonl directly under
                // .neuron/sessions/. Extract the stem (drop the .jsonl extension).
                path.file_stem().map(|n| n.to_string_lossy().into_owned())
            }),
            "loaded_config_files": context.loaded_config_files,
            "discovered_config_files": context.discovered_config_files,
            "memory_file_count": context.memory_file_count,
        },
        "sandbox": {
            "enabled": context.sandbox_status.enabled,
            "active": context.sandbox_status.active,
            "supported": context.sandbox_status.supported,
            "in_container": context.sandbox_status.in_container,
            "requested_namespace": context.sandbox_status.requested.namespace_restrictions,
            "active_namespace": context.sandbox_status.namespace_active,
            "requested_network": context.sandbox_status.requested.network_isolation,
            "active_network": context.sandbox_status.network_active,
            "filesystem_mode": context.sandbox_status.filesystem_mode.as_str(),
            "filesystem_active": context.sandbox_status.filesystem_active,
            "allowed_mounts": context.sandbox_status.allowed_mounts,
            "markers": context.sandbox_status.container_markers,
            "fallback_reason": context.sandbox_status.fallback_reason,
        }
    })
}

pub(crate) fn status_context(
    session_path: Option<&Path>,
) -> Result<StatusContext, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered_config_files = loader.discover().len();
    let runtime_config = loader.load()?;
    let project_context = ProjectContext::discover_with_git(&cwd, DEFAULT_DATE)?;
    let (project_root, git_branch) =
        parse_git_status_metadata(project_context.git_status.as_deref());
    let git_summary = parse_git_workspace_summary(project_context.git_status.as_deref());
    let sandbox_status = resolve_sandbox_status(runtime_config.sandbox(), &cwd);
    Ok(StatusContext {
        cwd,
        session_path: session_path.map(Path::to_path_buf),
        loaded_config_files: runtime_config.loaded_entries().len(),
        discovered_config_files,
        memory_file_count: project_context.instruction_files.len(),
        project_root,
        git_branch,
        git_summary,
        sandbox_status,
    })
}

/// The turns of a session as the user counts them: the prompts sent, which are its user
/// messages (tool results are messages of their own) less the runtime's automatic
/// "continue" nudges. The usage tracker's `turns` counts model requests instead.
pub(crate) fn prompt_count(session: &Session) -> u32 {
    let prompts = session
        .messages
        .iter()
        .filter(|message| is_prompt(message))
        .count();
    u32::try_from(prompts).unwrap_or(u32::MAX)
}

/// Whether `message` is a prompt the user sent, not one the runtime added to continue a
/// turn.
pub(crate) fn is_prompt(message: &ConversationMessage) -> bool {
    message.role == MessageRole::User
        && !message.blocks.iter().any(|block| {
            matches!(block, ContentBlock::Text { text } if text.starts_with(runtime::AUTO_CONTINUE_PREFIX))
        })
}

pub(crate) fn format_status_report(
    model: &str,
    usage: StatusUsage,
    permission_mode: &str,
    context: &StatusContext,
) -> String {
    let model = crate::provider::shown_model(model);
    [
        format!(
            "Status
  Model            {model}
  Permission mode  {permission_mode}
  Messages         {}
  Turns            {}
  Estimated tokens {}",
            usage.message_count, usage.turns, usage.estimated_tokens,
        ),
        format!(
            "Usage
  Latest total     {}
  Cumulative input {}
  Cumulative output {}
  Cumulative total {}",
            usage.latest.total_tokens(),
            usage.cumulative.input_tokens,
            usage.cumulative.output_tokens,
            usage.cumulative.total_tokens(),
        ),
        format!(
            "Workspace
  Cwd              {}
  Project root     {}
  Git branch       {}
  Git state        {}
  Changed files    {}
  Staged           {}
  Unstaged         {}
  Untracked        {}
  Session          {}
  Config files     loaded {}/{}
  Memory files     {}
  Suggested flow   /status → /diff → /commit",
            context.cwd.display(),
            context
                .project_root
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |path| path.display().to_string()),
            context.git_branch.as_deref().unwrap_or("unknown"),
            context.git_summary.headline(),
            context.git_summary.changed_files,
            context.git_summary.staged_files,
            context.git_summary.unstaged_files,
            context.git_summary.untracked_files,
            context.session_path.as_ref().map_or_else(
                || "live-repl".to_string(),
                |path| path.display().to_string()
            ),
            context.loaded_config_files,
            context.discovered_config_files,
            context.memory_file_count,
        ),
        format_status_sandbox_section(&context.sandbox_status, cfg!(target_os = "linux")),
    ]
    .join(
        "

",
    )
}

/// The `/status` Sandbox section: the report on Linux, where the sandbox (namespaces,
/// `unshare`) exists; one line elsewhere. `neuron sandbox` keeps the full report.
pub(crate) fn format_status_sandbox_section(
    status: &runtime::SandboxStatus,
    supported_os: bool,
) -> String {
    if supported_os {
        format_sandbox_report(status)
    } else {
        "Sandbox\n  Available         not on this OS (Linux only)".to_string()
    }
}

/// The banner's greeting in at most `width` columns: "Welcome back" once this
/// workspace has a saved session (`returning`), "Welcome" before; to the signed-in
/// account (its email, or the part before `@` when the whole one does not fit), else
/// to the OS user.
pub(crate) fn banner_greeting(
    email: Option<&str>,
    os_user: &str,
    returning: bool,
    width: usize,
) -> String {
    let opening = if returning { "Welcome back" } else { "Welcome" };
    // ", " and "!".
    let room = width.saturating_sub(opening.len() + 3);
    let name = match email.map(str::trim).filter(|email| !email.is_empty()) {
        Some(email) if email.chars().count() <= room => email,
        Some(email) => email.split('@').next().unwrap_or(email),
        None => os_user,
    };
    let name = if name.chars().count() <= room {
        name.to_string()
    } else {
        let kept = name
            .chars()
            .take(room.saturating_sub(3))
            .collect::<String>();
        format!("{kept}...")
    };
    format!("{opening}, {name}!")
}

pub(crate) fn format_sandbox_report(status: &runtime::SandboxStatus) -> String {
    format!(
        "Sandbox
  Enabled           {}
  Active            {}
  Supported         {}
  In container      {}
  Requested ns      {}
  Active ns         {}
  Requested net     {}
  Active net        {}
  Filesystem mode   {}
  Filesystem active {}
  Allowed mounts    {}
  Markers           {}
  Fallback reason   {}",
        status.enabled,
        status.active,
        status.supported,
        status.in_container,
        status.requested.namespace_restrictions,
        status.namespace_active,
        status.requested.network_isolation,
        status.network_active,
        status.filesystem_mode.as_str(),
        status.filesystem_active,
        if status.allowed_mounts.is_empty() {
            "<none>".to_string()
        } else {
            status.allowed_mounts.join(", ")
        },
        if status.container_markers.is_empty() {
            "<none>".to_string()
        } else {
            status.container_markers.join(", ")
        },
        status
            .fallback_reason
            .clone()
            .unwrap_or_else(|| "<none>".to_string()),
    )
}

pub(crate) fn format_commit_preflight_report(
    branch: Option<&str>,
    summary: GitWorkspaceSummary,
) -> String {
    format!(
        "Commit
  Result           ready
  Branch           {}
  Workspace        {}
  Changed files    {}
  Action           create a git commit from the current workspace changes",
        branch.unwrap_or("unknown"),
        summary.headline(),
        summary.changed_files,
    )
}

pub(crate) fn format_commit_skipped_report() -> String {
    "Commit
  Result           skipped
  Reason           no workspace changes
  Action           create a git commit from the current workspace changes
  Next             /status to inspect context · /diff to inspect repo changes"
        .to_string()
}

pub(crate) fn print_sandbox_status_snapshot(
    output_format: CliOutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let runtime_config = loader
        .load()
        .unwrap_or_else(|_| runtime::RuntimeConfig::empty());
    let status = resolve_sandbox_status(runtime_config.sandbox(), &cwd);
    match output_format {
        CliOutputFormat::Text => println!("{}", format_sandbox_report(&status)),
        CliOutputFormat::Json => println!(
            "{}",
            serde_json::to_string_pretty(&sandbox_json_value(&status))?
        ),
    }
    Ok(())
}

pub(crate) fn sandbox_json_value(status: &runtime::SandboxStatus) -> serde_json::Value {
    json!({
        "kind": "sandbox",
        "enabled": status.enabled,
        "active": status.active,
        "supported": status.supported,
        "in_container": status.in_container,
        "requested_namespace": status.requested.namespace_restrictions,
        "active_namespace": status.namespace_active,
        "requested_network": status.requested.network_isolation,
        "active_network": status.network_active,
        "filesystem_mode": status.filesystem_mode.as_str(),
        "filesystem_active": status.filesystem_active,
        "allowed_mounts": status.allowed_mounts,
        "markers": status.container_markers,
        "fallback_reason": status.fallback_reason,
    })
}

pub(crate) fn render_help_topic(topic: LocalHelpTopic) -> String {
    match topic {
        LocalHelpTopic::Status => "Status
  Usage            neuron status
  Purpose          show the local workspace snapshot without entering the REPL
  Output           model, permissions, git state, config files, and sandbox status
  Related          /status · neuron --resume latest /status"
            .to_string(),
        LocalHelpTopic::Sandbox => "Sandbox
  Usage            neuron sandbox
  Purpose          inspect the resolved sandbox and isolation state for the current directory
  Output           namespace, network, filesystem, and fallback details
  Related          /sandbox · neuron status"
            .to_string(),
        LocalHelpTopic::Doctor => "Doctor
  Usage            neuron doctor
  Purpose          diagnose local auth, config, workspace, sandbox, and build metadata
  Output           local-only health report; no provider request or session resume required
  Related          /doctor · neuron --resume latest /doctor"
            .to_string(),
        LocalHelpTopic::Acp => "ACP / Zed
  Usage            neuron acp [serve]
  Aliases          neuron --acp · neuron -acp
  Purpose          explain the current editor-facing ACP/Zed launch contract without starting the runtime
  Status           discoverability only; `serve` is a status alias and does not launch a daemon yet
  Related          ROADMAP #64a (discoverability) · ROADMAP #76 (real ACP support) · neuron --help"
            .to_string(),
    }
}

pub(crate) fn print_help_topic(topic: LocalHelpTopic) {
    println!("{}", render_help_topic(topic));
}

pub(crate) fn print_acp_status(
    output_format: CliOutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let message = "ACP/Zed editor integration is not implemented in neuron yet. `neuron acp serve` is only a discoverability alias today; it does not launch a daemon or Zed-specific protocol endpoint. Use the normal terminal surfaces for now and track ROADMAP #76 for real ACP support.";
    match output_format {
        CliOutputFormat::Text => {
            println!(
                "ACP / Zed\n  Status           discoverability only\n  Launch           `neuron acp serve` / `neuron --acp` / `neuron -acp` report status only; no editor daemon is available yet\n  Today            use `neuron prompt`, the REPL, or `neuron doctor` for local verification\n  Tracking         ROADMAP #76\n  Message          {message}"
            );
        }
        CliOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "kind": "acp",
                    "status": "discoverability_only",
                    "supported": false,
                    "serve_alias_only": true,
                    "message": message,
                    "launch_command": serde_json::Value::Null,
                    "aliases": ["acp", "--acp", "-acp"],
                    "discoverability_tracking": "ROADMAP #64a",
                    "tracking": "ROADMAP #76",
                    "recommended_workflows": [
                        "neuron prompt TEXT",
                        "neuron",
                        "neuron doctor"
                    ],
                }))?
            );
        }
    }
    Ok(())
}

pub(crate) fn render_config_report(
    section: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered = loader.discover();
    let runtime_config = loader.load()?;

    let mut lines = vec![
        format!(
            "Config
  Working directory {}
  Loaded files      {}
  Merged keys       {}",
            cwd.display(),
            runtime_config.loaded_entries().len(),
            runtime_config.merged().len()
        ),
        "Discovered files".to_string(),
    ];
    for entry in discovered {
        let source = match entry.source {
            ConfigSource::Bundled => "bundled",
            ConfigSource::User => "user",
            ConfigSource::Project => "project",
            ConfigSource::Local => "local",
        };
        let status = if runtime_config
            .loaded_entries()
            .iter()
            .any(|loaded_entry| loaded_entry.path == entry.path)
        {
            "loaded"
        } else {
            "missing"
        };
        lines.push(format!(
            "  {source:<7} {status:<7} {}",
            entry.path.display()
        ));
    }

    if let Some(section) = section {
        lines.push(format!("Merged section: {section}"));
        let value = match section {
            "env" => runtime_config.get("env"),
            "hooks" => runtime_config.get("hooks"),
            "model" => runtime_config.get("model"),
            "plugins" => runtime_config
                .get("plugins")
                .or_else(|| runtime_config.get("enabledPlugins")),
            other => {
                lines.push(format!(
                    "  Unsupported config section '{other}'. Use env, hooks, model, or plugins."
                ));
                return Ok(lines.join(
                    "
",
                ));
            }
        };
        lines.push(format!(
            "  {}",
            match value {
                Some(value) => value.render(),
                None => "<unset>".to_string(),
            }
        ));
        return Ok(lines.join(
            "
",
        ));
    }

    lines.push("Merged JSON".to_string());
    lines.push(format!("  {}", runtime_config.as_json().render()));
    Ok(lines.join(
        "
",
    ))
}

pub(crate) fn render_config_json(
    _section: Option<&str>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let loader = ConfigLoader::default_for(&cwd);
    let discovered = loader.discover();
    let runtime_config = loader.load()?;

    let loaded_paths: Vec<_> = runtime_config
        .loaded_entries()
        .iter()
        .map(|e| e.path.display().to_string())
        .collect();

    let files: Vec<_> = discovered
        .iter()
        .map(|e| {
            let source = match e.source {
                ConfigSource::Bundled => "bundled",
                ConfigSource::User => "user",
                ConfigSource::Project => "project",
                ConfigSource::Local => "local",
            };
            let is_loaded = runtime_config
                .loaded_entries()
                .iter()
                .any(|le| le.path == e.path);
            serde_json::json!({
                "path": e.path.display().to_string(),
                "source": source,
                "loaded": is_loaded,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "kind": "config",
        "cwd": cwd.display().to_string(),
        "loaded_files": loaded_paths.len(),
        "merged_keys": runtime_config.merged().len(),
        "files": files,
    }))
}

pub(crate) fn render_memory_report() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let project_context = ProjectContext::discover(&cwd, DEFAULT_DATE)?;
    let mut lines = vec![format!(
        "Memory
  Working directory {}
  Instruction files {}",
        cwd.display(),
        project_context.instruction_files.len()
    )];
    if project_context.instruction_files.is_empty() {
        lines.push("Discovered files".to_string());
        lines.push(
            "  No instruction files (NEURON.md, AGENTS.md) in this folder or the ones above it."
                .to_string(),
        );
    } else {
        lines.push("Discovered files".to_string());
        for (index, file) in project_context.instruction_files.iter().enumerate() {
            let preview = file.content.lines().next().unwrap_or("").trim();
            let preview = if preview.is_empty() {
                "<empty>"
            } else {
                preview
            };
            lines.push(format!("  {}. {}", index + 1, file.path.display(),));
            lines.push(format!(
                "     lines={} preview={}",
                file.content.lines().count(),
                preview
            ));
        }
    }
    Ok(lines.join(
        "
",
    ))
}

pub(crate) fn render_memory_json() -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    let project_context = ProjectContext::discover(&cwd, DEFAULT_DATE)?;
    let files: Vec<_> = project_context
        .instruction_files
        .iter()
        .map(|f| {
            json!({
                "path": f.path.display().to_string(),
                "lines": f.content.lines().count(),
                "preview": f.content.lines().next().unwrap_or("").trim(),
            })
        })
        .collect();
    Ok(json!({
        "kind": "memory",
        "cwd": cwd.display().to_string(),
        "instruction_files": files.len(),
        "files": files,
    }))
}

pub(crate) fn init_claude_md() -> Result<String, Box<dyn std::error::Error>> {
    let cwd = env::current_dir()?;
    Ok(initialize_repo(&cwd)?.render())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--help`, `/help`, the help topics and every command summary never name another
    /// product. Provider and model ids (`anthropic`, `claude-*`) are lowercase identifiers
    /// and stay allowed.
    #[test]
    fn help_text_names_no_other_product() {
        let mut cli_help = Vec::new();
        crate::executor::print_help_to(&mut cli_help).expect("help renders");
        let mut texts = vec![
            String::from_utf8(cli_help).expect("help is utf-8"),
            render_repl_help(),
        ];
        texts.extend(
            [
                LocalHelpTopic::Status,
                LocalHelpTopic::Sandbox,
                LocalHelpTopic::Doctor,
                LocalHelpTopic::Acp,
            ]
            .map(render_help_topic),
        );
        texts.extend(
            commands::slash_command_specs()
                .iter()
                .map(|spec| format!("/{}: {}", spec.name, spec.summary)),
        );
        for text in texts {
            for brand in ["Claude", "Anthropic"] {
                assert!(!text.contains(brand), "{brand} in: {text}");
            }
        }
    }

    #[test]
    fn the_shortcuts_panel_title_names_neuron_and_fits_the_frame() {
        let panel = render_shortcuts_panel();
        assert!(panel.contains("Neuron Shortcuts"), "{panel}");
        assert!(!panel.contains("NeuronCLI"), "{panel}");
        let mut lines = panel.lines().map(crate::brand::strip_ansi_len);
        let (top, title) = (lines.next(), lines.next());
        assert!(top.is_some() && top == title, "{top:?} {title:?}");
    }

    #[test]
    fn the_banner_welcomes_a_new_account_and_names_it() {
        assert_eq!(
            banner_greeting(Some("dev@zero-x.live"), "rahul", false, 36),
            "Welcome, dev@zero-x.live!"
        );
        assert_eq!(
            banner_greeting(Some("dev@zero-x.live"), "rahul", true, 36),
            "Welcome back, dev@zero-x.live!"
        );
        // Signed out: the OS user.
        assert_eq!(
            banner_greeting(None, "rahul", true, 36),
            "Welcome back, rahul!"
        );
        // An email that does not fit: the part before `@`, cut when even that is long.
        assert_eq!(
            banner_greeting(Some("tcli-verify.e2e@zero-x.test"), "rahul", true, 36),
            "Welcome back, tcli-verify.e2e!"
        );
        let long = banner_greeting(Some("a-very-long-mailbox-name-here@x.io"), "r", true, 36);
        assert!(long.chars().count() <= 36, "{long}");
        assert!(long.ends_with("...!"), "{long}");
    }

    #[test]
    fn status_shows_the_sandbox_report_only_where_it_applies() {
        let status = runtime::SandboxStatus::default();
        let linux = format_status_sandbox_section(&status, true);
        assert!(linux.contains("Fallback reason"), "{linux}");
        let other = format_status_sandbox_section(&status, false);
        assert_eq!(
            other,
            "Sandbox\n  Available         not on this OS (Linux only)"
        );
    }

    /// The `openai/` routing prefix picks a client; it is not part of the model's name, in
    /// the text report or the JSON one (a resumed session's stored model included).
    #[test]
    fn status_shows_the_model_without_its_routing_prefix() {
        let usage = StatusUsage {
            message_count: 0,
            turns: 0,
            latest: TokenUsage::default(),
            cumulative: TokenUsage::default(),
            estimated_tokens: 0,
        };
        let context = StatusContext {
            cwd: PathBuf::from("/tmp/project"),
            session_path: None,
            loaded_config_files: 0,
            discovered_config_files: 0,
            memory_file_count: 0,
            project_root: None,
            git_branch: None,
            git_summary: GitWorkspaceSummary::default(),
            sandbox_status: runtime::SandboxStatus::default(),
        };
        let text = format_status_report(
            "openai/nvidia/nvidia/nemotron-3-ultra-550b-a55b",
            usage,
            "workspace-write",
            &context,
        );
        assert!(
            text.contains("  Model            nvidia/nvidia/nemotron-3-ultra-550b-a55b\n"),
            "{text}"
        );
        let json = status_json_value(Some("openai/auto"), usage, "workspace-write", &context);
        assert_eq!(json["model"], "auto");
        assert!(status_json_value(None, usage, "workspace-write", &context)["model"].is_null());
    }

    /// Turns are the prompts sent, not the model requests a tool loop makes.
    #[test]
    fn turns_count_the_prompts_not_the_model_requests() {
        let mut session = Session::new();
        session.messages = vec![
            ConversationMessage::user_text("fix the bug"),
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "bash".to_string(),
                input: "{}".to_string(),
            }]),
            ConversationMessage::tool_result("t1", "bash", "ok", false),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "Fixed.".to_string(),
            }]),
            ConversationMessage::user_text(format!("{} continue", runtime::AUTO_CONTINUE_PREFIX)),
            ConversationMessage::user_text("now add a test"),
        ];
        assert_eq!(prompt_count(&session), 2);
    }
}
