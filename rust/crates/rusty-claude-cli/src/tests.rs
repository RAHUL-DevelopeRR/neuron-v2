use super::*;
use super::{
    build_runtime_plugin_state_with_loader, build_runtime_with_plugin_state,
    collect_session_prompt_history, create_managed_session_handle, describe_tool_progress,
    filter_tool_specs, format_bughunter_report, format_commit_preflight_report,
    format_commit_skipped_report, format_compact_report, format_connected_line, format_cost_report,
    format_history_timestamp, format_internal_prompt_progress_line, format_issue_report,
    format_model_report, format_model_switch_report, format_permissions_report,
    format_permissions_switch_report, format_pr_report, format_resume_report, format_status_report,
    format_tool_call_start, format_tool_result, format_ultraplan_report,
    format_unknown_slash_command, format_unknown_slash_command_message,
    format_user_visible_api_error, merge_prompt_with_stdin, normalize_permission_mode, parse_args,
    parse_export_args, parse_git_status_branch, parse_git_status_metadata_for,
    parse_git_workspace_summary, parse_history_count, permission_policy, print_help_to,
    push_output_block, render_config_report, render_diff_report, render_diff_report_for,
    render_memory_report, render_prompt_history_report, render_repl_help, render_resume_usage,
    render_session_markdown, resolve_model_alias, resolve_model_alias_with_config,
    resolve_repl_model, resolve_session_reference, response_to_events,
    resume_supported_slash_commands, run_resume_command, short_tool_id,
    slash_command_completion_candidates_with_sessions, status_context,
    summarize_tool_payload_for_markdown, try_resolve_bare_skill_prompt, validate_no_args,
    CliAction, CliOutputFormat, CliToolExecutor, GitWorkspaceSummary, InternalPromptProgressEvent,
    InternalPromptProgressState, LiveCli, LocalHelpTopic, PromptHistoryEntry, SlashCommand,
    StatusUsage, DEFAULT_MODEL, LATEST_SESSION_REFERENCE, STUB_COMMANDS,
};
use api::{ApiError, MessageResponse, OutputContentBlock, Usage};
use plugins::{
    PluginManager, PluginManagerConfig, PluginTool, PluginToolDefinition, PluginToolPermission,
};
use runtime::{
    load_oauth_credentials, save_oauth_credentials, AssistantEvent, ConfigLoader, ContentBlock,
    ConversationMessage, MessageRole, OAuthConfig, PermissionMode, Session, ToolExecutor,
};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tools::GlobalToolRegistry;

fn registry_with_plugin_tool() -> GlobalToolRegistry {
    GlobalToolRegistry::with_plugin_tools(vec![PluginTool::new(
        "plugin-demo@external",
        "plugin-demo",
        PluginToolDefinition {
            name: "plugin_echo".to_string(),
            description: Some("Echo plugin payload".to_string()),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string" }
                },
                "required": ["message"],
                "additionalProperties": false
            }),
        },
        "echo".to_string(),
        Vec::new(),
        PluginToolPermission::WorkspaceWrite,
        None,
    )])
    .expect("plugin tool registry should build")
}

#[test]
fn opaque_provider_wrapper_surfaces_failure_class_session_and_trace() {
    let error = ApiError::Api {
        status: "500".parse().expect("status"),
        error_type: Some("api_error".to_string()),
        message: Some(
            "Something went wrong while processing your request. Please try again, or use /new to start a fresh session."
                .to_string(),
        ),
        request_id: Some("req_jobdori_789".to_string()),
        body: String::new(),
        retryable: true,
        suggested_action: None,
    };

    let rendered = format_user_visible_api_error("session-issue-22", &error);
    assert!(rendered.contains("provider_internal"));
    assert!(rendered.contains("session session-issue-22"));
    assert!(rendered.contains("trace req_jobdori_789"));
}

#[test]
fn retry_exhaustion_uses_retry_failure_class_for_generic_provider_wrapper() {
    let error = ApiError::RetriesExhausted {
        attempts: 3,
        last_error: Box::new(ApiError::Api {
            status: "502".parse().expect("status"),
            error_type: Some("api_error".to_string()),
            message: Some(
                "Something went wrong while processing your request. Please try again, or use /new to start a fresh session."
                    .to_string(),
            ),
            request_id: Some("req_jobdori_790".to_string()),
            body: String::new(),
            retryable: true,
            suggested_action: None,
        }),
    };

    let rendered = format_user_visible_api_error("session-issue-22", &error);
    assert!(rendered.contains("provider_retry_exhausted"), "{rendered}");
    assert!(rendered.contains("session session-issue-22"));
    assert!(rendered.contains("trace req_jobdori_790"));
}

#[test]
fn context_window_preflight_errors_render_recovery_steps() {
    let error = ApiError::ContextWindowExceeded {
        model: "claude-sonnet-4-6".to_string(),
        estimated_input_tokens: 182_000,
        requested_output_tokens: 64_000,
        estimated_total_tokens: 246_000,
        context_window_tokens: 200_000,
    };

    let rendered = format_user_visible_api_error("session-issue-32", &error);
    assert!(rendered.contains("Context window blocked"), "{rendered}");
    assert!(rendered.contains("context_window_blocked"), "{rendered}");
    assert!(
        rendered.contains("Session          session-issue-32"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Model            claude-sonnet-4-6"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Input estimate   ~182000 tokens (heuristic)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Total estimate   ~246000 tokens (heuristic)"),
        "{rendered}"
    );
    assert!(rendered.contains("Compact          /compact"), "{rendered}");
    assert!(
        rendered.contains("Resume compact   neuron --resume session-issue-32 /compact"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Fresh session    /clear --confirm"),
        "{rendered}"
    );
    assert!(rendered.contains("Reduce scope"), "{rendered}");
    assert!(rendered.contains("Retry            rerun"), "{rendered}");
}

#[test]
fn provider_context_window_errors_are_reframed_with_same_guidance() {
    let error = ApiError::Api {
        status: "400".parse().expect("status"),
        error_type: Some("invalid_request_error".to_string()),
        message: Some(
            "This model's maximum context length is 200000 tokens, but your request used 230000 tokens."
                .to_string(),
        ),
        request_id: Some("req_ctx_456".to_string()),
        body: String::new(),
        retryable: false,
        suggested_action: None,
    };

    let rendered = format_user_visible_api_error("session-issue-32", &error);
    assert!(rendered.contains("context_window_blocked"), "{rendered}");
    assert!(
        rendered.contains("Trace            req_ctx_456"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Detail           This model's maximum context length is 200000 tokens"),
        "{rendered}"
    );
    assert!(rendered.contains("Compact          /compact"), "{rendered}");
    assert!(
        rendered.contains("Fresh session    /clear --confirm"),
        "{rendered}"
    );
}

#[test]
fn retry_wrapped_context_window_errors_keep_recovery_guidance() {
    let error = ApiError::RetriesExhausted {
        attempts: 2,
        last_error: Box::new(ApiError::Api {
            status: "413".parse().expect("status"),
            error_type: Some("invalid_request_error".to_string()),
            message: Some("Request is too large for this model's context window.".to_string()),
            request_id: Some("req_ctx_retry_789".to_string()),
            body: String::new(),
            retryable: false,
            suggested_action: None,
        }),
    };

    let rendered = format_user_visible_api_error("session-issue-32", &error);
    assert!(rendered.contains("Context window blocked"), "{rendered}");
    assert!(rendered.contains("context_window_blocked"), "{rendered}");
    assert!(
        rendered.contains("Trace            req_ctx_retry_789"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Detail           Request is too large for this model's context window."),
        "{rendered}"
    );
    assert!(rendered.contains("Compact          /compact"), "{rendered}");
    assert!(
        rendered.contains("Resume compact   neuron --resume session-issue-32 /compact"),
        "{rendered}"
    );
}

fn temp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("rusty-claude-cli-{nanos}-{unique}"))
}

fn git(args: &[&str], cwd: &Path) {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .expect("git command should run");
    assert!(
        status.success(),
        "git command failed: git {}",
        args.join(" ")
    );
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_current_dir<T>(cwd: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = cwd_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let previous = std::env::current_dir().expect("cwd should load");
    std::env::set_current_dir(cwd).expect("cwd should change");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::env::set_current_dir(previous).expect("cwd should restore");
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn write_skill_fixture(root: &Path, name: &str, description: &str) {
    let skill_dir = root.join(name);
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n"),
    )
    .expect("skill file should write");
}

fn write_plugin_fixture(root: &Path, name: &str, include_hooks: bool, include_lifecycle: bool) {
    fs::create_dir_all(root.join(".claude-plugin")).expect("manifest dir");
    if include_hooks {
        fs::create_dir_all(root.join("hooks")).expect("hooks dir");
        fs::write(
            root.join("hooks").join("pre.sh"),
            "#!/bin/sh\nprintf 'plugin pre hook'\n",
        )
        .expect("write hook");
    }
    if include_lifecycle {
        fs::create_dir_all(root.join("lifecycle")).expect("lifecycle dir");
        fs::write(
            root.join("lifecycle").join("init.sh"),
            "#!/bin/sh\nprintf 'init\\n' >> lifecycle.log\n",
        )
        .expect("write init lifecycle");
        fs::write(
            root.join("lifecycle").join("shutdown.sh"),
            "#!/bin/sh\nprintf 'shutdown\\n' >> lifecycle.log\n",
        )
        .expect("write shutdown lifecycle");
    }

    let hooks = if include_hooks {
        ",\n  \"hooks\": {\n    \"PreToolUse\": [\"./hooks/pre.sh\"]\n  }"
    } else {
        ""
    };
    let lifecycle = if include_lifecycle {
        ",\n  \"lifecycle\": {\n    \"Init\": [\"./lifecycle/init.sh\"],\n    \"Shutdown\": [\"./lifecycle/shutdown.sh\"]\n  }"
    } else {
        ""
    };
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        format!(
            "{{\n  \"name\": \"{name}\",\n  \"version\": \"1.0.0\",\n  \"description\": \"runtime plugin fixture\"{hooks}{lifecycle}\n}}"
        ),
    )
    .expect("write plugin manifest");
}
#[test]
fn defaults_to_repl_when_no_args() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    assert_eq!(
        parse_args(&[]).expect("args should parse"),
        CliAction::Repl {
            model: DEFAULT_MODEL.to_string(),
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn default_permission_mode_uses_project_config_when_env_is_unset() {
    let _guard = env_lock();
    let root = temp_dir();
    let cwd = root.join("project");
    let config_home = root.join("config-home");
    std::fs::create_dir_all(cwd.join(".neuron")).expect("project config dir should exist");
    std::fs::create_dir_all(&config_home).expect("config home should exist");
    std::fs::write(
        cwd.join(".neuron").join("settings.json"),
        r#"{"permissionMode":"acceptEdits"}"#,
    )
    .expect("project config should write");

    let original_config_home = std::env::var("CLAW_CONFIG_HOME").ok();
    let original_permission_mode = std::env::var("RUSTY_CLAUDE_PERMISSION_MODE").ok();
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");

    let resolved = with_current_dir(&cwd, super::default_permission_mode);

    match original_config_home {
        Some(value) => std::env::set_var("CLAW_CONFIG_HOME", value),
        None => std::env::remove_var("CLAW_CONFIG_HOME"),
    }
    match original_permission_mode {
        Some(value) => std::env::set_var("RUSTY_CLAUDE_PERMISSION_MODE", value),
        None => std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE"),
    }
    std::fs::remove_dir_all(root).expect("temp config root should clean up");

    assert_eq!(resolved, PermissionMode::WorkspaceWrite);
}

#[test]
fn env_permission_mode_overrides_project_config_default() {
    let _guard = env_lock();
    let root = temp_dir();
    let cwd = root.join("project");
    let config_home = root.join("config-home");
    std::fs::create_dir_all(cwd.join(".neuron")).expect("project config dir should exist");
    std::fs::create_dir_all(&config_home).expect("config home should exist");
    std::fs::write(
        cwd.join(".neuron").join("settings.json"),
        r#"{"permissionMode":"acceptEdits"}"#,
    )
    .expect("project config should write");

    let original_config_home = std::env::var("CLAW_CONFIG_HOME").ok();
    let original_permission_mode = std::env::var("RUSTY_CLAUDE_PERMISSION_MODE").ok();
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);
    std::env::set_var("RUSTY_CLAUDE_PERMISSION_MODE", "read-only");

    let resolved = with_current_dir(&cwd, super::default_permission_mode);

    match original_config_home {
        Some(value) => std::env::set_var("CLAW_CONFIG_HOME", value),
        None => std::env::remove_var("CLAW_CONFIG_HOME"),
    }
    match original_permission_mode {
        Some(value) => std::env::set_var("RUSTY_CLAUDE_PERMISSION_MODE", value),
        None => std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE"),
    }
    std::fs::remove_dir_all(root).expect("temp config root should clean up");

    assert_eq!(resolved, PermissionMode::ReadOnly);
}

#[test]
fn resolve_cli_auth_source_ignores_saved_oauth_credentials() {
    let _guard = env_lock();
    let config_home = temp_dir();
    std::fs::create_dir_all(&config_home).expect("config home should exist");

    let original_config_home = std::env::var("CLAW_CONFIG_HOME").ok();
    let original_api_key = std::env::var("ANTHROPIC_API_KEY").ok();
    let original_auth_token = std::env::var("ANTHROPIC_AUTH_TOKEN").ok();
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

    save_oauth_credentials(&runtime::OAuthTokenSet {
        access_token: "expired-access-token".to_string(),
        refresh_token: Some("refresh-token".to_string()),
        expires_at: Some(0),
        scopes: vec!["org:create_api_key".to_string(), "user:profile".to_string()],
    })
    .expect("save expired oauth credentials");

    let error = super::resolve_cli_auth_source_for_cwd()
        .expect_err("saved oauth should be ignored without env auth");

    match original_config_home {
        Some(value) => std::env::set_var("CLAW_CONFIG_HOME", value),
        None => std::env::remove_var("CLAW_CONFIG_HOME"),
    }
    match original_api_key {
        Some(value) => std::env::set_var("ANTHROPIC_API_KEY", value),
        None => std::env::remove_var("ANTHROPIC_API_KEY"),
    }
    match original_auth_token {
        Some(value) => std::env::set_var("ANTHROPIC_AUTH_TOKEN", value),
        None => std::env::remove_var("ANTHROPIC_AUTH_TOKEN"),
    }
    std::fs::remove_dir_all(config_home).expect("temp config home should clean up");

    assert!(error.to_string().contains("ANTHROPIC_API_KEY"));
}

#[test]
fn parses_prompt_subcommand() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec![
        "prompt".to_string(),
        "hello".to_string(),
        "world".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::Prompt {
            prompt: "hello world".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn merge_prompt_with_stdin_returns_prompt_unchanged_when_no_pipe() {
    // given
    let prompt = "Review this";

    // when
    let merged = merge_prompt_with_stdin(prompt, None);

    // then
    assert_eq!(merged, "Review this");
}

#[test]
fn merge_prompt_with_stdin_ignores_whitespace_only_pipe() {
    // given
    let prompt = "Review this";
    let piped = "   \n\t\n  ";

    // when
    let merged = merge_prompt_with_stdin(prompt, Some(piped));

    // then
    assert_eq!(merged, "Review this");
}

#[test]
fn merge_prompt_with_stdin_appends_piped_content_as_context() {
    // given
    let prompt = "Review this";
    let piped = "fn main() { println!(\"hi\"); }\n";

    // when
    let merged = merge_prompt_with_stdin(prompt, Some(piped));

    // then
    assert_eq!(merged, "Review this\n\nfn main() { println!(\"hi\"); }");
}

#[test]
fn merge_prompt_with_stdin_trims_surrounding_whitespace_on_pipe() {
    // given
    let prompt = "Summarize";
    let piped = "\n\n  some notes  \n\n";

    // when
    let merged = merge_prompt_with_stdin(prompt, Some(piped));

    // then
    assert_eq!(merged, "Summarize\n\nsome notes");
}

#[test]
fn merge_prompt_with_stdin_returns_pipe_when_prompt_is_empty() {
    // given
    let prompt = "";
    let piped = "standalone body";

    // when
    let merged = merge_prompt_with_stdin(prompt, Some(piped));

    // then
    assert_eq!(merged, "standalone body");
}

#[test]
fn parses_bare_prompt_and_json_output_flag() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec![
        "--output-format=json".to_string(),
        "--model".to_string(),
        "claude-opus".to_string(),
        "explain".to_string(),
        "this".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::Prompt {
            prompt: "explain this".to_string(),
            model: "claude-opus".to_string(),
            output_format: CliOutputFormat::Json,
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn parses_compact_flag_for_prompt_mode() {
    // given a bare prompt invocation that includes the --compact flag
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec![
        "--compact".to_string(),
        "summarize".to_string(),
        "this".to_string(),
    ];

    // when parse_args interprets the flag
    let parsed = parse_args(&args).expect("args should parse");

    // then compact mode is propagated and other defaults stay unchanged
    assert_eq!(
        parsed,
        CliAction::Prompt {
            prompt: "summarize this".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            compact: true,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn prompt_subcommand_defaults_compact_to_false() {
    // given a `prompt` subcommand invocation without --compact
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec!["prompt".to_string(), "hello".to_string()];

    // when parse_args runs
    let parsed = parse_args(&args).expect("args should parse");

    // then compact stays false (opt-in flag)
    match parsed {
        CliAction::Prompt { compact, .. } => assert!(!compact),
        other => panic!("expected Prompt action, got {other:?}"),
    }
}

#[test]
fn resolves_model_aliases_in_args() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec![
        "--model".to_string(),
        "opus".to_string(),
        "explain".to_string(),
        "this".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::Prompt {
            prompt: "explain this".to_string(),
            model: "claude-opus-4-6".to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: PermissionMode::WorkspaceWrite,
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn resolves_known_model_aliases() {
    assert_eq!(resolve_model_alias("opus"), "claude-opus-4-6");
    assert_eq!(resolve_model_alias("sonnet"), "claude-sonnet-4-6");
    assert_eq!(resolve_model_alias("haiku"), "claude-haiku-4-5-20251213");
    assert_eq!(resolve_model_alias("claude-opus"), "claude-opus");
}

#[test]
fn user_defined_aliases_resolve_before_provider_dispatch() {
    // given
    let _guard = env_lock();
    let root = temp_dir();
    let cwd = root.join("project");
    let config_home = root.join("config-home");
    std::fs::create_dir_all(cwd.join(".neuron")).expect("project config dir should exist");
    std::fs::create_dir_all(&config_home).expect("config home should exist");
    std::fs::write(
        cwd.join(".neuron").join("settings.json"),
        r#"{"aliases":{"fast":"claude-haiku-4-5-20251213","smart":"opus","cheap":"grok-3-mini"}}"#,
    )
    .expect("project config should write");

    let original_config_home = std::env::var("CLAW_CONFIG_HOME").ok();
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);

    // when
    let direct = with_current_dir(&cwd, || resolve_model_alias_with_config("fast"));
    let chained = with_current_dir(&cwd, || resolve_model_alias_with_config("smart"));
    let cross_provider = with_current_dir(&cwd, || resolve_model_alias_with_config("cheap"));
    let unknown = with_current_dir(&cwd, || resolve_model_alias_with_config("unknown-model"));
    let builtin = with_current_dir(&cwd, || resolve_model_alias_with_config("haiku"));

    match original_config_home {
        Some(value) => std::env::set_var("CLAW_CONFIG_HOME", value),
        None => std::env::remove_var("CLAW_CONFIG_HOME"),
    }
    std::fs::remove_dir_all(root).expect("temp config root should clean up");

    // then
    assert_eq!(direct, "claude-haiku-4-5-20251213");
    assert_eq!(chained, "claude-opus-4-6");
    assert_eq!(cross_provider, "grok-3-mini");
    assert_eq!(unknown, "unknown-model");
    assert_eq!(builtin, "claude-haiku-4-5-20251213");
}

#[test]
fn parses_version_flags_without_initializing_prompt_mode() {
    assert_eq!(
        parse_args(&["--version".to_string()]).expect("args should parse"),
        CliAction::Version {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["-V".to_string()]).expect("args should parse"),
        CliAction::Version {
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_permission_mode_flag() {
    let args = vec!["--permission-mode=read-only".to_string()];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::Repl {
            model: DEFAULT_MODEL.to_string(),
            allowed_tools: None,
            permission_mode: PermissionMode::ReadOnly,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn dangerously_skip_permissions_flag_forces_danger_full_access_in_repl() {
    let _guard = env_lock();
    std::env::set_var("RUSTY_CLAUDE_PERMISSION_MODE", "read-only");
    let args = vec!["--dangerously-skip-permissions".to_string()];
    let parsed = parse_args(&args).expect("args should parse");
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");

    assert_eq!(
        parsed,
        CliAction::Repl {
            model: DEFAULT_MODEL.to_string(),
            allowed_tools: None,
            permission_mode: PermissionMode::DangerFullAccess,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn dangerously_skip_permissions_flag_applies_to_prompt_subcommand() {
    let _guard = env_lock();
    std::env::set_var("RUSTY_CLAUDE_PERMISSION_MODE", "read-only");
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "prompt".to_string(),
        "do".to_string(),
        "the".to_string(),
        "thing".to_string(),
    ];
    let parsed = parse_args(&args).expect("args should parse");
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");

    assert_eq!(
        parsed,
        CliAction::Prompt {
            prompt: "do the thing".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: PermissionMode::DangerFullAccess,
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn parses_allowed_tools_flags_with_aliases_and_lists() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec![
        "--allowedTools".to_string(),
        "read,glob".to_string(),
        "--allowed-tools=write_file".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::Repl {
            model: DEFAULT_MODEL.to_string(),
            allowed_tools: Some(
                ["glob_search", "read_file", "write_file"]
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            ),
            permission_mode: PermissionMode::WorkspaceWrite,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn rejects_unknown_allowed_tools() {
    let error = parse_args(&["--allowedTools".to_string(), "teleport".to_string()])
        .expect_err("tool should be rejected");
    assert!(error.contains("unsupported tool in --allowedTools: teleport"));
}

#[test]
fn parses_system_prompt_options() {
    let args = vec![
        "system-prompt".to_string(),
        "--cwd".to_string(),
        "/tmp/project".to_string(),
        "--date".to_string(),
        "2026-04-01".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::PrintSystemPrompt {
            cwd: PathBuf::from("/tmp/project"),
            date: "2026-04-01".to_string(),
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn login_parses_and_logout_errors_helpfully() {
    assert_eq!(
        parse_args(&["login".to_string()]).expect("login should parse"),
        CliAction::Auth {
            action: AuthAction::Login,
            output_format: CliOutputFormat::Text,
        }
    );
    let logout = parse_args(&["logout".to_string()]).expect_err("logout should be removed");
    assert!(logout.contains("neuron auth reset"));
    assert_eq!(
        parse_args(&["doctor".to_string()]).expect("doctor should parse"),
        CliAction::Doctor {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["state".to_string()]).expect("state should parse"),
        CliAction::State {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&[
            "state".to_string(),
            "--output-format".to_string(),
            "json".to_string()
        ])
        .expect("state --output-format json should parse"),
        CliAction::State {
            output_format: CliOutputFormat::Json,
        }
    );
    assert_eq!(
        parse_args(&["init".to_string()]).expect("init should parse"),
        CliAction::Init {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["agents".to_string()]).expect("agents should parse"),
        CliAction::Agents {
            args: None,
            output_format: CliOutputFormat::Text
        }
    );
    assert_eq!(
        parse_args(&["mcp".to_string()]).expect("mcp should parse"),
        CliAction::Mcp {
            args: None,
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["skills".to_string()]).expect("skills should parse"),
        CliAction::Skills {
            args: None,
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&[
            "skills".to_string(),
            "help".to_string(),
            "overview".to_string()
        ])
        .expect("skills help overview should invoke"),
        CliAction::Prompt {
            prompt: "$help overview".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: crate::default_permission_mode(),
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
    assert_eq!(
        parse_args(&["agents".to_string(), "--help".to_string()])
            .expect("agents help should parse"),
        CliAction::Agents {
            args: Some("--help".to_string()),
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn dump_manifests_subcommand_accepts_explicit_manifest_dir() {
    assert_eq!(
        parse_args(&[
            "dump-manifests".to_string(),
            "--manifests-dir".to_string(),
            "/tmp/upstream".to_string(),
        ])
        .expect("dump-manifests should parse"),
        CliAction::DumpManifests {
            output_format: CliOutputFormat::Text,
            manifests_dir: Some(PathBuf::from("/tmp/upstream")),
        }
    );
    assert_eq!(
        parse_args(&[
            "dump-manifests".to_string(),
            "--manifests-dir=/tmp/upstream".to_string()
        ])
        .expect("inline dump-manifests flag should parse"),
        CliAction::DumpManifests {
            output_format: CliOutputFormat::Text,
            manifests_dir: Some(PathBuf::from("/tmp/upstream")),
        }
    );
}

#[test]
fn parses_acp_command_surfaces() {
    assert_eq!(
        parse_args(&["acp".to_string()]).expect("acp should parse"),
        CliAction::Acp {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["acp".to_string(), "serve".to_string()]).expect("acp serve should parse"),
        CliAction::Acp {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["--acp".to_string()]).expect("--acp should parse"),
        CliAction::Acp {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["-acp".to_string()]).expect("-acp should parse"),
        CliAction::Acp {
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn local_command_help_flags_stay_on_the_local_parser_path() {
    assert_eq!(
        parse_args(&["status".to_string(), "--help".to_string()])
            .expect("status help should parse"),
        CliAction::HelpTopic(LocalHelpTopic::Status)
    );
    assert_eq!(
        parse_args(&["sandbox".to_string(), "-h".to_string()]).expect("sandbox help should parse"),
        CliAction::HelpTopic(LocalHelpTopic::Sandbox)
    );
    assert_eq!(
        parse_args(&["doctor".to_string(), "--help".to_string()])
            .expect("doctor help should parse"),
        CliAction::HelpTopic(LocalHelpTopic::Doctor)
    );
    assert_eq!(
        parse_args(&["acp".to_string(), "--help".to_string()]).expect("acp help should parse"),
        CliAction::HelpTopic(LocalHelpTopic::Acp)
    );
}

#[test]
fn parses_single_word_command_aliases_without_falling_back_to_prompt_mode() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    assert_eq!(
        parse_args(&["help".to_string()]).expect("help should parse"),
        CliAction::Help {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["version".to_string()]).expect("version should parse"),
        CliAction::Version {
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["status".to_string()]).expect("status should parse"),
        CliAction::Status {
            model: DEFAULT_MODEL.to_string(),
            permission_mode: PermissionMode::WorkspaceWrite,
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["sandbox".to_string()]).expect("sandbox should parse"),
        CliAction::Sandbox {
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_bare_export_subcommand_targeting_latest_session() {
    // given
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    let args = vec!["export".to_string()];

    // when
    let parsed = parse_args(&args).expect("bare export should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: LATEST_SESSION_REFERENCE.to_string(),
            output_path: None,
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_export_subcommand_with_positional_output_path() {
    // given
    let args = vec!["export".to_string(), "conversation.md".to_string()];

    // when
    let parsed = parse_args(&args).expect("export with path should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: LATEST_SESSION_REFERENCE.to_string(),
            output_path: Some(PathBuf::from("conversation.md")),
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_export_subcommand_with_session_and_output_flags() {
    // given
    let args = vec![
        "export".to_string(),
        "--session".to_string(),
        "session-alpha".to_string(),
        "--output".to_string(),
        "/tmp/share.md".to_string(),
    ];

    // when
    let parsed = parse_args(&args).expect("export flags should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: "session-alpha".to_string(),
            output_path: Some(PathBuf::from("/tmp/share.md")),
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_export_subcommand_with_inline_flag_values() {
    // given
    let args = vec![
        "export".to_string(),
        "--session=session-beta".to_string(),
        "--output=/tmp/beta.md".to_string(),
    ];

    // when
    let parsed = parse_args(&args).expect("export inline flags should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: "session-beta".to_string(),
            output_path: Some(PathBuf::from("/tmp/beta.md")),
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_export_subcommand_with_json_output_format() {
    // given
    let args = vec![
        "--output-format=json".to_string(),
        "export".to_string(),
        "/tmp/notes.md".to_string(),
    ];

    // when
    let parsed = parse_args(&args).expect("json export should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: LATEST_SESSION_REFERENCE.to_string(),
            output_path: Some(PathBuf::from("/tmp/notes.md")),
            output_format: CliOutputFormat::Json,
        }
    );
}

#[test]
fn rejects_unknown_export_options_with_helpful_message() {
    // given
    let args = vec!["export".to_string(), "--bogus".to_string()];

    // when
    let error = parse_args(&args).expect_err("unknown export option should fail");

    // then
    assert!(error.contains("unknown export option: --bogus"));
}

#[test]
fn rejects_export_with_extra_positional_after_path() {
    // given
    let args = vec![
        "export".to_string(),
        "first.md".to_string(),
        "second.md".to_string(),
    ];

    // when
    let error = parse_args(&args).expect_err("multiple positionals should fail");

    // then
    assert!(error.contains("unexpected export argument: second.md"));
}

#[test]
fn parse_export_args_helper_defaults_to_latest_reference_and_no_output() {
    // given
    let args: Vec<String> = vec![];

    // when
    let parsed =
        parse_export_args(&args, CliOutputFormat::Text).expect("empty export args should parse");

    // then
    assert_eq!(
        parsed,
        CliAction::Export {
            session_reference: LATEST_SESSION_REFERENCE.to_string(),
            output_path: None,
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn render_session_markdown_includes_header_and_summarized_tool_calls() {
    // given
    let mut session = Session::new();
    session.session_id = "session-export-test".to_string();
    session.messages = vec![
        ConversationMessage::user_text("How do I list files?"),
        ConversationMessage::assistant(vec![
            ContentBlock::Text {
                text: "I'll run a tool.".to_string(),
            },
            ContentBlock::ToolUse {
                id: "toolu_abcdefghijklmnop".to_string(),
                name: "bash".to_string(),
                input: r#"{"command":"ls -la"}"#.to_string(),
            },
        ]),
        ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_abcdefghijklmnop".to_string(),
                tool_name: "bash".to_string(),
                output: "total 8\ndrwxr-xr-x  2 user staff   64 Apr  7 12:00 .".to_string(),
                is_error: false,
            }],
            usage: None,
        },
    ];

    // when
    let markdown = render_session_markdown(
        &session,
        "session-export-test",
        std::path::Path::new("/tmp/sessions/session-export-test.jsonl"),
    );

    // then
    assert!(markdown.starts_with("# Conversation Export"));
    assert!(markdown.contains("- **Session**: `session-export-test`"));
    assert!(markdown.contains("- **Messages**: 3"));
    assert!(markdown.contains("## 1. User"));
    assert!(markdown.contains("How do I list files?"));
    assert!(markdown.contains("## 2. Assistant"));
    assert!(markdown.contains("**Tool call** `bash`"));
    assert!(markdown.contains("toolu_abcdefâ€¦"));
    assert!(markdown.contains("ls -la"));
    assert!(markdown.contains("## 3. Tool"));
    assert!(markdown.contains("**Tool result** `bash`"));
    assert!(markdown.contains("ok"));
    assert!(markdown.contains("total 8"));
}

#[test]
fn render_session_markdown_marks_tool_errors_and_skips_empty_summaries() {
    // given
    let mut session = Session::new();
    session.session_id = "errs".to_string();
    session.messages = vec![ConversationMessage {
        role: MessageRole::Tool,
        blocks: vec![ContentBlock::ToolResult {
            tool_use_id: "short".to_string(),
            tool_name: "read_file".to_string(),
            output: "   ".to_string(),
            is_error: true,
        }],
        usage: None,
    }];

    // when
    let markdown = render_session_markdown(&session, "errs", std::path::Path::new("errs.jsonl"));

    // then
    assert!(markdown.contains("**Tool result** `read_file` _(id `short`, error)_"));
    // an empty summary should not produce a stray blockquote line
    assert!(!markdown.contains("> \n"));
}

#[test]
fn summarize_tool_payload_for_markdown_compacts_json_and_truncates_overflow() {
    // given
    let json_payload = r#"{
        "command":   "ls -la",
        "cwd": "/tmp"
    }"#;
    let long_payload = "a".repeat(600);

    // when
    let compacted = summarize_tool_payload_for_markdown(json_payload);
    let truncated = summarize_tool_payload_for_markdown(&long_payload);

    // then
    assert_eq!(compacted, r#"{"command":"ls -la","cwd":"/tmp"}"#);
    assert!(truncated.ends_with('\u{2026}'));
    assert!(truncated.chars().count() <= 281);
}

#[test]
fn short_tool_id_truncates_long_identifiers_with_ellipsis() {
    // given
    let long = "toolu_01ABCDEFGHIJKLMN";
    let short = "tool_1";

    // when
    let trimmed_long = short_tool_id(long);
    let trimmed_short = short_tool_id(short);

    // then
    assert_eq!(trimmed_long, "toolu_01ABCDâ€¦");
    assert_eq!(trimmed_short, "tool_1");
}

#[test]
fn parses_json_output_for_mcp_and_skills_commands() {
    assert_eq!(
        parse_args(&["--output-format=json".to_string(), "mcp".to_string()])
            .expect("json mcp should parse"),
        CliAction::Mcp {
            args: None,
            output_format: CliOutputFormat::Json,
        }
    );
    assert_eq!(
        parse_args(&[
            "--output-format=json".to_string(),
            "/skills".to_string(),
            "help".to_string(),
        ])
        .expect("json /skills help should parse"),
        CliAction::Skills {
            args: Some("help".to_string()),
            output_format: CliOutputFormat::Json,
        }
    );
}

#[test]
fn single_word_slash_command_names_return_guidance_instead_of_hitting_prompt_mode() {
    let error = parse_args(&["cost".to_string()]).expect_err("cost should return guidance");
    assert!(error.contains("slash command"));
    assert!(error.contains("/cost"));
}

#[test]
fn multi_word_prompt_still_uses_shorthand_prompt_mode() {
    let _guard = env_lock();
    std::env::remove_var("RUSTY_CLAUDE_PERMISSION_MODE");
    // Input is ["help", "me", "debug"] so the joined prompt shorthand
    // must be "help me debug". A previous batch accidentally rewrote
    // the expected string to "$help overview" (copy-paste slip).
    assert_eq!(
        parse_args(&["help".to_string(), "me".to_string(), "debug".to_string()])
            .expect("prompt shorthand should still work"),
        CliAction::Prompt {
            prompt: "help me debug".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: crate::default_permission_mode(),
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
}

#[test]
fn parses_direct_agents_mcp_and_skills_slash_commands() {
    assert_eq!(
        parse_args(&["/agents".to_string()]).expect("/agents should parse"),
        CliAction::Agents {
            args: None,
            output_format: CliOutputFormat::Text
        }
    );
    assert_eq!(
        parse_args(&["/mcp".to_string(), "show".to_string(), "demo".to_string()])
            .expect("/mcp show demo should parse"),
        CliAction::Mcp {
            args: Some("show demo".to_string()),
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["/skills".to_string()]).expect("/skills should parse"),
        CliAction::Skills {
            args: None,
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["/skill".to_string()]).expect("/skill should parse"),
        CliAction::Skills {
            args: None,
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["/skills".to_string(), "help".to_string()])
            .expect("/skills help should parse"),
        CliAction::Skills {
            args: Some("help".to_string()),
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["/skill".to_string(), "list".to_string()]).expect("/skill list should parse"),
        CliAction::Skills {
            args: Some("list".to_string()),
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&[
            "/skills".to_string(),
            "help".to_string(),
            "overview".to_string()
        ])
        .expect("/skills help overview should invoke"),
        CliAction::Prompt {
            prompt: "$help overview".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: crate::default_permission_mode(),
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
    assert_eq!(
        parse_args(&[
            "/skills".to_string(),
            "install".to_string(),
            "./fixtures/help-skill".to_string(),
        ])
        .expect("/skills install should parse"),
        CliAction::Skills {
            args: Some("install ./fixtures/help-skill".to_string()),
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["/skills".to_string(), "/test".to_string()])
            .expect("/skills /test should normalize to a single skill prompt prefix"),
        CliAction::Prompt {
            prompt: "$test".to_string(),
            model: DEFAULT_MODEL.to_string(),
            output_format: CliOutputFormat::Text,
            allowed_tools: None,
            permission_mode: crate::default_permission_mode(),
            compact: false,
            base_commit: None,
            reasoning_effort: None,
            allow_broad_cwd: false,
        }
    );
    let error = parse_args(&["/status".to_string()])
        .expect_err("/status should remain REPL-only when invoked directly");
    assert!(error.contains("interactive-only"));
    assert!(error.contains("neuron --resume SESSION.jsonl /status"));
}

#[test]
fn direct_slash_commands_surface_shared_validation_errors() {
    let compact_error = parse_args(&["/compact".to_string(), "now".to_string()])
        .expect_err("invalid /compact shape should be rejected");
    assert!(compact_error.contains("Unexpected arguments for /compact."));
    assert!(compact_error.contains("Usage            /compact"));

    let plugins_error = parse_args(&[
        "/plugins".to_string(),
        "list".to_string(),
        "extra".to_string(),
    ])
    .expect_err("invalid /plugins list shape should be rejected");
    assert!(plugins_error.contains("Usage: /plugin list"));
    assert!(plugins_error.contains("Aliases          /plugins, /marketplace"));
}

#[test]
fn formats_unknown_slash_command_with_suggestions() {
    let report = format_unknown_slash_command_message("statsu");
    assert!(report.contains("unknown slash command: /statsu"));
    assert!(report.contains("Did you mean"));
    assert!(report.contains("Use /help"));
}

#[test]
fn formats_namespaced_omc_slash_command_with_contract_guidance() {
    let report = format_unknown_slash_command_message("oh-my-claudecode:hud");
    assert!(report.contains("unknown slash command: /oh-my-claudecode:hud"));
    assert!(report.contains("Claude Code/OMC plugin command"));
    assert!(report.contains("plugin slash commands"));
    assert!(report.contains("statusline"));
    assert!(report.contains("session hooks"));
}

#[test]
fn parses_resume_flag_with_slash_command() {
    let args = vec![
        "--resume".to_string(),
        "session.jsonl".to_string(),
        "/compact".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("session.jsonl"),
            commands: vec!["/compact".to_string()],
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_resume_flag_without_path_as_latest_session() {
    assert_eq!(
        parse_args(&["--resume".to_string()]).expect("args should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("latest"),
            commands: vec![],
            output_format: CliOutputFormat::Text,
        }
    );
    assert_eq!(
        parse_args(&["--resume".to_string(), "/status".to_string()])
            .expect("resume shortcut should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("latest"),
            commands: vec!["/status".to_string()],
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_resume_flag_with_multiple_slash_commands() {
    let args = vec![
        "--resume".to_string(),
        "session.jsonl".to_string(),
        "/status".to_string(),
        "/compact".to_string(),
        "/cost".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("session.jsonl"),
            commands: vec![
                "/status".to_string(),
                "/compact".to_string(),
                "/cost".to_string(),
            ],
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn rejects_unknown_options_with_helpful_guidance() {
    let error = parse_args(&["--resum".to_string()]).expect_err("unknown option should fail");
    assert!(error.contains("unknown option: --resum"));
    assert!(error.contains("Did you mean --resume?"));
    assert!(error.contains("neuron --help"));
}

#[test]
fn parses_resume_flag_with_slash_command_arguments() {
    let args = vec![
        "--resume".to_string(),
        "session.jsonl".to_string(),
        "/export".to_string(),
        "notes.txt".to_string(),
        "/clear".to_string(),
        "--confirm".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("session.jsonl"),
            commands: vec![
                "/export notes.txt".to_string(),
                "/clear --confirm".to_string(),
            ],
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn parses_resume_flag_with_absolute_export_path() {
    let args = vec![
        "--resume".to_string(),
        "session.jsonl".to_string(),
        "/export".to_string(),
        "/tmp/notes.txt".to_string(),
        "/status".to_string(),
    ];
    assert_eq!(
        parse_args(&args).expect("args should parse"),
        CliAction::ResumeSession {
            session_path: PathBuf::from("session.jsonl"),
            commands: vec!["/export /tmp/notes.txt".to_string(), "/status".to_string()],
            output_format: CliOutputFormat::Text,
        }
    );
}

#[test]
fn filtered_tool_specs_respect_allowlist() {
    let allowed = ["read_file", "grep_search"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let filtered = filter_tool_specs(&GlobalToolRegistry::builtin(), Some(&allowed));
    let names = filtered
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["read_file", "grep_search"]);
}

#[test]
fn filtered_tool_specs_include_plugin_tools() {
    let filtered = filter_tool_specs(&registry_with_plugin_tool(), None);
    let names = filtered
        .into_iter()
        .map(|definition| definition.name)
        .collect::<Vec<_>>();
    assert!(names.contains(&"bash".to_string()));
    assert!(names.contains(&"plugin_echo".to_string()));
}

#[test]
fn permission_policy_uses_plugin_tool_permissions() {
    let feature_config = runtime::RuntimeFeatureConfig::default();
    let policy = permission_policy(
        PermissionMode::ReadOnly,
        &feature_config,
        &registry_with_plugin_tool(),
    )
    .expect("permission policy should build");
    let required = policy.required_mode_for("plugin_echo");
    assert_eq!(required, PermissionMode::WorkspaceWrite);
}

#[test]
fn shared_help_uses_resume_annotation_copy() {
    let help = commands::render_slash_command_help();
    assert!(help.contains("Slash commands"));
    assert!(help.contains("works with --resume SESSION.jsonl"));
}

#[test]
fn bare_skill_dispatch_resolves_known_project_skill_to_prompt() {
    let _guard = env_lock();
    let workspace = temp_dir();
    write_skill_fixture(
        &workspace.join(".codex").join("skills"),
        "caveman",
        "Project skill fixture",
    );

    let prompt = try_resolve_bare_skill_prompt(&workspace, "caveman sharpen club")
        .expect("known bare skill should dispatch");
    assert_eq!(prompt, "$caveman sharpen club");

    fs::remove_dir_all(workspace).expect("workspace should clean up");
}

#[test]
fn bare_skill_dispatch_ignores_unknown_or_non_skill_input() {
    let _guard = env_lock();
    let workspace = temp_dir();
    fs::create_dir_all(&workspace).expect("workspace should exist");

    assert_eq!(
        try_resolve_bare_skill_prompt(&workspace, "not-a-known-skill do thing"),
        None
    );
    assert_eq!(try_resolve_bare_skill_prompt(&workspace, "/status"), None);

    fs::remove_dir_all(workspace).expect("workspace should clean up");
}

#[test]
fn repl_help_includes_shared_commands_and_exit() {
    let help = render_repl_help();
    assert!(help.contains("REPL"));
    assert!(help.contains("/help"));
    assert!(help.contains("Complete commands, modes, and recent sessions"));
    assert!(help.contains("/status"));
    assert!(help.contains("/sandbox"));
    assert!(help.contains("/model [model]"));
    assert!(help.contains("/permissions [read-only|workspace-write|danger-full-access]"));
    assert!(help.contains("/clear [--confirm]"));
    assert!(help.contains("/cost"));
    assert!(help.contains("/resume <session-path>"));
    assert!(help.contains("/config [env|hooks|model|plugins]"));
    assert!(help.contains("/mcp [list|show <server>|help]"));
    assert!(help.contains("/memory"));
    assert!(help.contains("/init"));
    assert!(help.contains("/diff"));
    assert!(help.contains("/version"));
    assert!(help.contains("/export [file]"));
    // Batch 5 added `/session delete`; match on the stable core rather than
    // the trailing bracket so future additions don't re-break this.
    assert!(help.contains("/session [list|switch <session-id>|fork [branch-name]"));
    assert!(help.contains(
        "/plugin [list|install <path>|enable <name>|disable <name>|uninstall <id>|update <id>]"
    ));
    assert!(help.contains("aliases: /plugins, /marketplace"));
    assert!(help.contains("/agents"));
    assert!(help.contains("/skills"));
    assert!(help.contains("/exit"));
    assert!(help.contains("Auto-save            .neuron/sessions/<session-id>.jsonl"));
    assert!(help.contains("Resume latest        /resume latest"));
}

#[test]
fn completion_candidates_include_workflow_shortcuts_and_dynamic_sessions() {
    let completions = slash_command_completion_candidates_with_sessions(
        "sonnet",
        Some("session-current"),
        vec!["session-old".to_string()],
    );

    assert!(completions.contains(&"/model claude-sonnet-4-6".to_string()));
    assert!(completions.contains(&"/permissions workspace-write".to_string()));
    assert!(completions.contains(&"/session list".to_string()));
    assert!(completions.contains(&"/session switch session-current".to_string()));
    assert!(completions.contains(&"/resume session-old".to_string()));
    assert!(completions.contains(&"/mcp list".to_string()));
    assert!(completions.contains(&"/ultraplan ".to_string()));
}

#[test]
#[ignore = "LiveCli::new can block on Windows background runtime setup"]
fn startup_banner_mentions_workflow_completions() {
    let _guard = env_lock();
    // Inject dummy credentials so LiveCli can construct without real Anthropic key
    std::env::set_var("ANTHROPIC_API_KEY", "test-dummy-key-for-banner-test");
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");

    let banner = with_current_dir(&root, || {
        LiveCli::new(
            "claude-sonnet-4-6".to_string(),
            true,
            None,
            PermissionMode::DangerFullAccess,
        )
        .expect("cli should initialize")
        .startup_banner()
    });

    assert!(banner.contains("Tips for getting started"));
    assert!(banner.contains("What's new"));

    fs::remove_dir_all(root).expect("cleanup temp dir");
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[test]
fn format_connected_line_renders_anthropic_provider_for_claude_model() {
    let model = "claude-sonnet-4-6";

    let line = format_connected_line(model);

    assert!(line.contains("Connected:"));
    assert!(line.contains("claude-sonnet-4-6"));
    assert!(line.contains("anthropic"));
}

#[test]
fn format_connected_line_renders_xai_provider_for_grok_model() {
    let model = "grok-3";

    let line = format_connected_line(model);

    assert!(line.contains("Connected:"));
    assert!(line.contains("grok-3"));
    assert!(line.contains("xai"));
}

#[test]
fn resolve_repl_model_returns_user_supplied_model_unchanged_when_explicit() {
    let user_model = "claude-sonnet-4-6".to_string();

    let resolved = resolve_repl_model(user_model);

    assert_eq!(resolved, "claude-sonnet-4-6");
}

#[test]
fn resolve_repl_model_falls_back_to_anthropic_model_env_when_default() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    let config_home = root.join("config");
    fs::create_dir_all(&config_home).expect("config home dir");
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_MODEL");
    std::env::set_var("ANTHROPIC_MODEL", "sonnet");

    let resolved = with_current_dir(&root, || resolve_repl_model(DEFAULT_MODEL.to_string()));

    assert_eq!(resolved, "claude-sonnet-4-6");

    std::env::remove_var("ANTHROPIC_MODEL");
    std::env::remove_var("CLAW_CONFIG_HOME");
    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn resolve_repl_model_returns_default_when_env_unset_and_no_config() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    let config_home = root.join("config");
    fs::create_dir_all(&config_home).expect("config home dir");
    std::env::set_var("CLAW_CONFIG_HOME", &config_home);
    std::env::remove_var("ANTHROPIC_MODEL");

    let resolved = with_current_dir(&root, || resolve_repl_model(DEFAULT_MODEL.to_string()));

    assert_eq!(resolved, DEFAULT_MODEL);

    std::env::remove_var("CLAW_CONFIG_HOME");
    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn resume_supported_command_list_matches_expected_surface() {
    let names = resume_supported_slash_commands()
        .into_iter()
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    // Now with 135+ slash commands, verify minimum resume support
    assert!(
        names.len() >= 39,
        "expected at least 39 resume-supported commands, got {}",
        names.len()
    );
    // Verify key resume commands still exist
    assert!(names.contains(&"help"));
    assert!(names.contains(&"status"));
    assert!(names.contains(&"compact"));
}

#[test]
fn resume_report_uses_sectioned_layout() {
    let report = format_resume_report("session.jsonl", 14, 6);
    assert!(report.contains("Session resumed"));
    assert!(report.contains("Session file     session.jsonl"));
    assert!(report.contains("Messages         14"));
    assert!(report.contains("Turns            6"));
}

#[test]
fn compact_report_uses_structured_output() {
    let compacted = format_compact_report(8, 5, false);
    assert!(compacted.contains("Compact"));
    assert!(compacted.contains("Result           compacted"));
    assert!(compacted.contains("Messages removed 8"));
    let skipped = format_compact_report(0, 3, true);
    assert!(skipped.contains("Result           skipped"));
}

#[test]
fn cost_report_uses_sectioned_layout() {
    let report = format_cost_report(runtime::TokenUsage {
        input_tokens: 20,
        output_tokens: 8,
        cache_creation_input_tokens: 3,
        cache_read_input_tokens: 1,
    });
    assert!(report.contains("Cost"));
    assert!(report.contains("Input tokens     20"));
    assert!(report.contains("Output tokens    8"));
    assert!(report.contains("Cache create     3"));
    assert!(report.contains("Cache read       1"));
    assert!(report.contains("Total tokens     32"));
}

#[test]
fn permissions_report_uses_sectioned_layout() {
    let report = format_permissions_report("workspace-write");
    assert!(report.contains("Permissions"));
    assert!(report.contains("Active mode      workspace-write"));
    assert!(report.contains("Modes"));
    assert!(report.contains("read-only"));
    assert!(report.contains("workspace-write"));
    assert!(report.contains("danger-full-access"));
}

#[test]
fn permissions_switch_report_is_structured() {
    let report = format_permissions_switch_report("read-only", "workspace-write");
    assert!(report.contains("Permissions updated"));
    assert!(report.contains("Result           mode switched"));
    assert!(report.contains("Previous mode    read-only"));
    assert!(report.contains("Active mode      workspace-write"));
    assert!(report.contains("Applies to       subsequent tool calls"));
}

#[test]
fn init_help_mentions_direct_subcommand() {
    let mut help = Vec::new();
    print_help_to(&mut help).expect("help should render");
    let help = String::from_utf8(help).expect("help should be utf8");
    assert!(help.contains("neuron help"));
    assert!(help.contains("neuron version"));
    assert!(help.contains("neuron status"));
    assert!(help.contains("neuron sandbox"));
    assert!(help.contains("neuron init"));
    assert!(help.contains("neuron acp [serve]"));
    assert!(help.contains("neuron agents"));
    assert!(help.contains("neuron mcp"));
    assert!(help.contains("neuron skills"));
    assert!(help.contains("neuron /skills"));
    assert!(help.contains("RAHUL-DevelopeRR/claw-code"));
    assert!(help.contains("cargo install neuron-cli"));
    assert!(!help.contains("neuron login"));
    assert!(!help.contains("neuron logout"));
}

#[test]
fn model_report_uses_sectioned_layout() {
    let report = format_model_report("claude-sonnet", 12, 4);
    assert!(report.contains("Model"));
    assert!(report.contains("Current model    claude-sonnet"));
    assert!(report.contains("Session messages 12"));
    assert!(report.contains("Switch models with /model <name>"));
}

#[test]
fn model_switch_report_preserves_context_summary() {
    let report = format_model_switch_report("claude-sonnet", "claude-opus", 9);
    assert!(report.contains("Model updated"));
    assert!(report.contains("Previous         claude-sonnet"));
    assert!(report.contains("Current          claude-opus"));
    assert!(report.contains("Preserved msgs   9"));
}

#[test]
fn status_line_reports_model_and_token_totals() {
    let status = format_status_report(
        "claude-sonnet",
        StatusUsage {
            message_count: 7,
            turns: 3,
            latest: runtime::TokenUsage {
                input_tokens: 5,
                output_tokens: 4,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 0,
            },
            cumulative: runtime::TokenUsage {
                input_tokens: 20,
                output_tokens: 8,
                cache_creation_input_tokens: 2,
                cache_read_input_tokens: 1,
            },
            estimated_tokens: 128,
        },
        "workspace-write",
        &super::StatusContext {
            cwd: PathBuf::from("/tmp/project"),
            session_path: Some(PathBuf::from("session.jsonl")),
            loaded_config_files: 2,
            discovered_config_files: 3,
            memory_file_count: 4,
            project_root: Some(PathBuf::from("/tmp")),
            git_branch: Some("main".to_string()),
            git_summary: GitWorkspaceSummary {
                changed_files: 3,
                staged_files: 1,
                unstaged_files: 1,
                untracked_files: 1,
                conflicted_files: 0,
            },
            sandbox_status: runtime::SandboxStatus::default(),
        },
    );
    assert!(status.contains("Status"));
    assert!(status.contains("Model            claude-sonnet"));
    assert!(status.contains("Permission mode  workspace-write"));
    assert!(status.contains("Messages         7"));
    assert!(status.contains("Latest total     10"));
    assert!(status.contains("Cumulative total 31"));
    assert!(status.contains("Cwd              /tmp/project"));
    assert!(status.contains("Project root     /tmp"));
    assert!(status.contains("Git branch       main"));
    assert!(status.contains("Git state        dirty · 3 files · 1 staged, 1 unstaged, 1 untracked"));
    assert!(status.contains("Changed files    3"));
    assert!(status.contains("Staged           1"));
    assert!(status.contains("Unstaged         1"));
    assert!(status.contains("Untracked        1"));
    assert!(status.contains("Session          session.jsonl"));
    assert!(status.contains("Config files     loaded 2/3"));
    assert!(status.contains("Memory files     4"));
    assert!(status.contains("Suggested flow   /status â†’ /diff â†’ /commit"));
}

#[test]
fn commit_reports_surface_workspace_context() {
    let summary = GitWorkspaceSummary {
        changed_files: 2,
        staged_files: 1,
        unstaged_files: 1,
        untracked_files: 0,
        conflicted_files: 0,
    };

    let preflight = format_commit_preflight_report(Some("feature/ux"), summary);
    assert!(preflight.contains("Result           ready"));
    assert!(preflight.contains("Branch           feature/ux"));
    assert!(preflight.contains("Workspace        dirty · 2 files · 1 staged, 1 unstaged"));
    assert!(preflight
        .contains("Action           create a git commit from the current workspace changes"));
}

#[test]
fn commit_skipped_report_points_to_next_steps() {
    let report = format_commit_skipped_report();
    assert!(report.contains("Reason           no workspace changes"));
    assert!(
        report.contains("Action           create a git commit from the current workspace changes")
    );
    assert!(report.contains("/status to inspect context"));
    assert!(report.contains("/diff to inspect repo changes"));
}

#[test]
fn runtime_slash_reports_describe_command_behavior() {
    let bughunter = format_bughunter_report(Some("runtime"));
    assert!(bughunter.contains("Scope            runtime"));
    assert!(bughunter.contains("inspect the selected code for likely bugs"));

    let ultraplan = format_ultraplan_report(Some("ship the release"));
    assert!(ultraplan.contains("Task             ship the release"));
    assert!(ultraplan.contains("break work into a multi-step execution plan"));

    let pr = format_pr_report("feature/ux", Some("ready for review"));
    assert!(pr.contains("Branch           feature/ux"));
    assert!(pr.contains("draft or create a pull request"));

    let issue = format_issue_report(Some("flaky test"));
    assert!(issue.contains("Context          flaky test"));
    assert!(issue.contains("draft or create a GitHub issue"));
}

#[test]
fn no_arg_commands_reject_unexpected_arguments() {
    assert!(validate_no_args("/commit", None).is_ok());

    let error = validate_no_args("/commit", Some("now"))
        .expect_err("unexpected arguments should fail")
        .to_string();
    assert!(error.contains("/commit does not accept arguments"));
    assert!(error.contains("Received: now"));
}

#[test]
fn config_report_supports_section_views() {
    let report = render_config_report(Some("env")).expect("config report should render");
    assert!(report.contains("Merged section: env"));
    let plugins_report =
        render_config_report(Some("plugins")).expect("plugins config report should render");
    assert!(plugins_report.contains("Merged section: plugins"));
}

#[test]
fn memory_report_uses_sectioned_layout() {
    let report = render_memory_report().expect("memory report should render");
    assert!(report.contains("Memory"));
    assert!(report.contains("Working directory"));
    assert!(report.contains("Instruction files"));
    assert!(report.contains("Discovered files"));
}

#[test]
fn config_report_uses_sectioned_layout() {
    let report = render_config_report(None).expect("config report should render");
    assert!(report.contains("Config"));
    assert!(report.contains("Discovered files"));
    assert!(report.contains("Merged JSON"));
}

#[test]
fn parses_git_status_metadata() {
    let _guard = env_lock();
    let temp_root = temp_dir();
    fs::create_dir_all(&temp_root).expect("root dir");
    let (project_root, branch) = parse_git_status_metadata_for(
        &temp_root,
        Some(
            "## rcc/cli...origin/rcc/cli
 M src/main.rs",
        ),
    );
    assert_eq!(
        parse_git_status_branch(Some("## rcc/cli...origin/rcc/cli\n M src/main.rs")).as_deref(),
        Some("rcc/cli")
    );
    let _ = project_root;
    fs::remove_dir_all(temp_root).expect("cleanup temp dir");
}

#[test]
fn parses_detached_head_from_status_snapshot() {
    let _guard = env_lock();
    assert_eq!(
        parse_git_status_branch(Some(
            "## HEAD (no branch)
 M src/main.rs"
        )),
        Some("detached HEAD".to_string())
    );
}

#[test]
fn parses_git_workspace_summary_counts() {
    let summary = parse_git_workspace_summary(Some(
        "## feature/ux
M  src/main.rs
 M README.md
?? notes.md
UU conflicted.rs",
    ));

    assert_eq!(
        summary,
        GitWorkspaceSummary {
            changed_files: 4,
            staged_files: 2,
            unstaged_files: 2,
            untracked_files: 1,
            conflicted_files: 1,
        }
    );
    assert_eq!(
        summary.headline(),
        "dirty · 4 files · 2 staged, 2 unstaged, 1 untracked, 1 conflicted"
    );
}

#[test]
fn render_diff_report_shows_clean_tree_for_committed_repo() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    git(&["init", "--quiet"], &root);
    git(&["config", "user.email", "tests@example.com"], &root);
    git(&["config", "user.name", "Rusty Claude Tests"], &root);
    fs::write(root.join("tracked.txt"), "hello\n").expect("write file");
    git(&["add", "tracked.txt"], &root);
    git(&["commit", "-m", "init", "--quiet"], &root);

    let report = render_diff_report_for(&root).expect("diff report should render");
    assert!(report.contains("clean working tree"));

    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn render_diff_report_includes_staged_and_unstaged_sections() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    git(&["init", "--quiet"], &root);
    git(&["config", "user.email", "tests@example.com"], &root);
    git(&["config", "user.name", "Rusty Claude Tests"], &root);
    fs::write(root.join("tracked.txt"), "hello\n").expect("write file");
    git(&["add", "tracked.txt"], &root);
    git(&["commit", "-m", "init", "--quiet"], &root);

    fs::write(root.join("tracked.txt"), "hello\nstaged\n").expect("update file");
    git(&["add", "tracked.txt"], &root);
    fs::write(root.join("tracked.txt"), "hello\nstaged\nunstaged\n").expect("update file twice");

    let report = render_diff_report_for(&root).expect("diff report should render");
    assert!(report.contains("Staged Changes"));
    assert!(report.contains("Unstaged Changes"));
    assert!(report.contains("tracked.txt"));

    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn render_diff_report_omits_ignored_files() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    git(&["init", "--quiet"], &root);
    git(&["config", "user.email", "tests@example.com"], &root);
    git(&["config", "user.name", "Rusty Claude Tests"], &root);
    fs::write(root.join(".gitignore"), ".omx/\nignored.txt\n").expect("write gitignore");
    fs::write(root.join("tracked.txt"), "hello\n").expect("write tracked");
    git(&["add", ".gitignore", "tracked.txt"], &root);
    git(&["commit", "-m", "init", "--quiet"], &root);
    fs::create_dir_all(root.join(".omx")).expect("write omx dir");
    fs::write(root.join(".omx").join("state.json"), "{}").expect("write ignored omx");
    fs::write(root.join("ignored.txt"), "secret\n").expect("write ignored file");
    fs::write(root.join("tracked.txt"), "hello\nworld\n").expect("write tracked change");

    let report = render_diff_report_for(&root).expect("diff report should render");
    assert!(report.contains("tracked.txt"));
    assert!(!report.contains("+++ b/ignored.txt"));
    assert!(!report.contains("+++ b/.omx/state.json"));

    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn resume_diff_command_renders_report_for_saved_session() {
    let _guard = env_lock();
    let root = temp_dir();
    fs::create_dir_all(&root).expect("root dir");
    git(&["init", "--quiet"], &root);
    git(&["config", "user.email", "tests@example.com"], &root);
    git(&["config", "user.name", "Rusty Claude Tests"], &root);
    fs::write(root.join("tracked.txt"), "hello\n").expect("write tracked");
    git(&["add", "tracked.txt"], &root);
    git(&["commit", "-m", "init", "--quiet"], &root);
    fs::write(root.join("tracked.txt"), "hello\nworld\n").expect("modify tracked");
    let session_path = root.join("session.json");
    Session::new()
        .save_to_path(&session_path)
        .expect("session should save");

    let session = Session::load_from_path(&session_path).expect("session should load");
    let outcome = with_current_dir(&root, || {
        run_resume_command(&session_path, &session, &SlashCommand::Diff)
            .expect("resume diff should work")
    });
    let message = outcome.message.expect("diff message should exist");
    assert!(message.contains("Unstaged Changes"));
    assert!(message.contains("tracked.txt"));

    fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn status_context_reads_real_workspace_metadata() {
    let context = status_context(None).expect("status context should load");
    assert!(context.cwd.is_absolute());
    assert!(context.discovered_config_files >= context.loaded_config_files);
    assert!(context.loaded_config_files <= context.discovered_config_files);
}

#[test]
fn normalizes_supported_permission_modes() {
    assert_eq!(normalize_permission_mode("read-only"), Some("read-only"));
    assert_eq!(
        normalize_permission_mode("workspace-write"),
        Some("workspace-write")
    );
    assert_eq!(
        normalize_permission_mode("danger-full-access"),
        Some("danger-full-access")
    );
    assert_eq!(normalize_permission_mode("unknown"), None);
}

#[test]
fn clear_command_requires_explicit_confirmation_flag() {
    assert_eq!(
        SlashCommand::parse("/clear"),
        Ok(Some(SlashCommand::Clear { confirm: false }))
    );
    assert_eq!(
        SlashCommand::parse("/clear --confirm"),
        Ok(Some(SlashCommand::Clear { confirm: true }))
    );
}

#[test]
fn parses_resume_and_config_slash_commands() {
    assert_eq!(
        SlashCommand::parse("/resume saved-session.jsonl"),
        Ok(Some(SlashCommand::Resume {
            session_path: Some("saved-session.jsonl".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/clear --confirm"),
        Ok(Some(SlashCommand::Clear { confirm: true }))
    );
    assert_eq!(
        SlashCommand::parse("/config"),
        Ok(Some(SlashCommand::Config { section: None }))
    );
    assert_eq!(
        SlashCommand::parse("/config env"),
        Ok(Some(SlashCommand::Config {
            section: Some("env".to_string())
        }))
    );
    assert_eq!(
        SlashCommand::parse("/memory"),
        Ok(Some(SlashCommand::Memory))
    );
    assert_eq!(SlashCommand::parse("/init"), Ok(Some(SlashCommand::Init)));
    assert_eq!(
        SlashCommand::parse("/session fork incident-review"),
        Ok(Some(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: Some("incident-review".to_string())
        }))
    );
}

#[test]
fn help_mentions_jsonl_resume_examples() {
    let mut help = Vec::new();
    print_help_to(&mut help).expect("help should render");
    let help = String::from_utf8(help).expect("help should be utf8");
    assert!(help.contains("neuron --resume [SESSION.jsonl|session-id|latest]"));
    assert!(help.contains("Use `latest` with --resume, /resume, or /session switch"));
    assert!(help.contains("neuron --resume latest"));
    assert!(help.contains("neuron --resume latest /status /diff /export notes.txt"));
}

#[test]
fn managed_sessions_default_to_jsonl_and_resolve_legacy_json() {
    let _guard = cwd_guard();
    let workspace = temp_workspace("session-resolution");
    std::fs::create_dir_all(&workspace).expect("workspace should create");
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&workspace).expect("switch cwd");

    let handle = create_managed_session_handle("session-alpha").expect("jsonl handle");
    assert!(handle.path.ends_with("session-alpha.jsonl"));

    let legacy_path = workspace.join(".claw/sessions/legacy.json");
    std::fs::create_dir_all(
        legacy_path
            .parent()
            .expect("legacy path should have parent directory"),
    )
    .expect("session dir should exist");
    Session::new()
        .with_workspace_root(workspace.clone())
        .with_persistence_path(legacy_path.clone())
        .save_to_path(&legacy_path)
        .expect("legacy session should save");

    let resolved = resolve_session_reference("legacy").expect("legacy session should resolve");
    assert_eq!(
        resolved
            .path
            .canonicalize()
            .expect("resolved path should exist"),
        legacy_path
            .canonicalize()
            .expect("legacy path should exist")
    );

    std::env::set_current_dir(previous).expect("restore cwd");
    std::fs::remove_dir_all(workspace).expect("workspace should clean up");
}

#[test]
fn latest_session_alias_resolves_most_recent_managed_session() {
    let _guard = cwd_guard();
    let workspace = temp_workspace("latest-session-alias");
    std::fs::create_dir_all(&workspace).expect("workspace should create");
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&workspace).expect("switch cwd");

    let older = create_managed_session_handle("session-older").expect("older handle");
    Session::new()
        .with_persistence_path(older.path.clone())
        .save_to_path(&older.path)
        .expect("older session should save");
    std::thread::sleep(Duration::from_millis(20));
    let newer = create_managed_session_handle("session-newer").expect("newer handle");
    Session::new()
        .with_persistence_path(newer.path.clone())
        .save_to_path(&newer.path)
        .expect("newer session should save");

    let resolved = resolve_session_reference("latest").expect("latest session should resolve");
    assert_eq!(
        resolved
            .path
            .canonicalize()
            .expect("resolved path should exist"),
        newer.path.canonicalize().expect("newer path should exist")
    );

    std::env::set_current_dir(previous).expect("restore cwd");
    std::fs::remove_dir_all(workspace).expect("workspace should clean up");
}

#[test]
fn load_session_reference_rejects_workspace_mismatch() {
    let _guard = cwd_guard();
    let workspace_a = temp_workspace("session-mismatch-a");
    let workspace_b = temp_workspace("session-mismatch-b");
    std::fs::create_dir_all(&workspace_a).expect("workspace a should create");
    std::fs::create_dir_all(&workspace_b).expect("workspace b should create");
    let previous = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&workspace_b).expect("switch cwd");

    let session_path = workspace_a.join(".neuron/sessions/legacy-cross.jsonl");
    std::fs::create_dir_all(
        session_path
            .parent()
            .expect("session path should have parent directory"),
    )
    .expect("session dir should exist");
    Session::new()
        .with_workspace_root(workspace_a.clone())
        .with_persistence_path(session_path.clone())
        .save_to_path(&session_path)
        .expect("session should save");

    let error = crate::load_session_reference(&session_path.display().to_string())
        .expect_err("mismatched workspace should fail");
    assert!(
        error.to_string().contains("session workspace mismatch"),
        "unexpected error: {error}"
    );
    assert!(
        error
            .to_string()
            .contains(&workspace_b.display().to_string()),
        "expected current workspace in error: {error}"
    );
    assert!(
        error
            .to_string()
            .contains(&workspace_a.display().to_string()),
        "expected originating workspace in error: {error}"
    );

    std::env::set_current_dir(previous).expect("restore cwd");
    std::fs::remove_dir_all(workspace_a).expect("workspace a should clean up");
    std::fs::remove_dir_all(workspace_b).expect("workspace b should clean up");
}

#[test]
fn unknown_slash_command_guidance_suggests_nearby_commands() {
    let message = format_unknown_slash_command("stats");
    assert!(message.contains("Unknown slash command: /stats"));
    assert!(message.contains("/status"));
    assert!(message.contains("/help"));
}

#[test]
fn unknown_omc_slash_command_guidance_explains_runtime_gap() {
    let message = format_unknown_slash_command("oh-my-claudecode:hud");
    assert!(message.contains("Unknown slash command: /oh-my-claudecode:hud"));
    assert!(message.contains("Claude Code/OMC plugin command"));
    assert!(message.contains("does not yet load plugin slash commands"));
}

#[test]
fn resume_usage_mentions_latest_shortcut() {
    let usage = render_resume_usage();
    assert!(usage.contains("/resume <session-path|session-id|latest>"));
    assert!(usage.contains(".neuron/sessions/<session-id>.jsonl"));
    assert!(usage.contains("/session list"));
}

fn cwd_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn cwd_guard() -> MutexGuard<'static, ()> {
    cwd_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn cwd_guard_recovers_after_poisoning() {
    let poisoned = std::thread::spawn(|| {
        let _guard = cwd_guard();
        panic!("poison cwd lock");
    })
    .join();
    assert!(poisoned.is_err(), "poisoning thread should panic");

    let _guard = cwd_guard();
}

fn temp_workspace(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("neuron-cli-{label}-{nanos}"))
}

#[test]
fn init_template_mentions_detected_rust_workspace() {
    let _guard = cwd_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let rendered = crate::init::render_init_claude_md(&workspace_root);
    assert!(rendered.contains("# NEURON.md"));
    assert!(rendered.contains("cargo clippy --workspace --all-targets -- -D warnings"));
}

#[test]
fn converts_tool_roundtrip_messages() {
    let messages = vec![
        ConversationMessage::user_text("hello"),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "bash".to_string(),
            input: "{\"command\":\"pwd\"}".to_string(),
        }]),
        ConversationMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "tool-1".to_string(),
                tool_name: "bash".to_string(),
                output: "ok".to_string(),
                is_error: false,
            }],
            usage: None,
        },
    ];

    let converted = super::convert_messages(&messages);
    assert_eq!(converted.len(), 3);
    assert_eq!(converted[1].role, "assistant");
    assert_eq!(converted[2].role, "user");
}
#[test]
fn repl_help_mentions_history_completion_and_multiline() {
    let help = render_repl_help();
    assert!(help.contains("Up/Down"));
    assert!(help.contains("Tab"));
    assert!(help.contains("Shift+Enter/Ctrl+J"));
    assert!(help.contains("Ctrl-R"));
    assert!(help.contains("Reverse-search prompt history"));
    assert!(help.contains("/history [count]"));
}

#[test]
fn parse_history_count_defaults_to_twenty_when_missing() {
    // given
    let raw: Option<&str> = None;

    // when
    let parsed = parse_history_count(raw);

    // then
    assert_eq!(parsed, Ok(20));
}

#[test]
fn parse_history_count_accepts_positive_integers() {
    // given
    let raw = Some("25");

    // when
    let parsed = parse_history_count(raw);

    // then
    assert_eq!(parsed, Ok(25));
}

#[test]
fn parse_history_count_rejects_zero() {
    // given
    let raw = Some("0");

    // when
    let parsed = parse_history_count(raw);

    // then
    assert!(parsed.is_err());
    assert!(parsed.unwrap_err().contains("greater than 0"));
}

#[test]
fn parse_history_count_rejects_non_numeric() {
    // given
    let raw = Some("abc");

    // when
    let parsed = parse_history_count(raw);

    // then
    assert!(parsed.is_err());
    assert!(parsed.unwrap_err().contains("invalid count 'abc'"));
}

#[test]
fn format_history_timestamp_renders_iso8601_utc() {
    // given
    // 2023-01-15T12:34:56.789Z -> 1673786096789 ms
    let timestamp_ms: u64 = 1_673_786_096_789;

    // when
    let formatted = format_history_timestamp(timestamp_ms);

    // then
    assert_eq!(formatted, "2023-01-15T12:34:56.789Z");
}

#[test]
fn format_history_timestamp_renders_unix_epoch_origin() {
    // given
    let timestamp_ms: u64 = 0;

    // when
    let formatted = format_history_timestamp(timestamp_ms);

    // then
    assert_eq!(formatted, "1970-01-01T00:00:00.000Z");
}

#[test]
fn render_prompt_history_report_lists_entries_with_timestamps() {
    // given
    let entries = vec![
        PromptHistoryEntry {
            timestamp_ms: 1_673_786_096_000,
            text: "first prompt".to_string(),
        },
        PromptHistoryEntry {
            timestamp_ms: 1_673_786_100_000,
            text: "second prompt".to_string(),
        },
    ];

    // when
    let rendered = render_prompt_history_report(&entries, 10);

    // then
    assert!(rendered.contains("Prompt history"));
    assert!(rendered.contains("Total            2"));
    assert!(rendered.contains("Showing          2 most recent"));
    assert!(rendered.contains("Reverse search   Ctrl-R in the REPL"));
    assert!(rendered.contains("2023-01-15T12:34:56.000Z"));
    assert!(rendered.contains("first prompt"));
    assert!(rendered.contains("second prompt"));
}

#[test]
fn render_prompt_history_report_truncates_to_limit_from_the_tail() {
    // given
    let entries = vec![
        PromptHistoryEntry {
            timestamp_ms: 1_000,
            text: "older".to_string(),
        },
        PromptHistoryEntry {
            timestamp_ms: 2_000,
            text: "middle".to_string(),
        },
        PromptHistoryEntry {
            timestamp_ms: 3_000,
            text: "latest".to_string(),
        },
    ];

    // when
    let rendered = render_prompt_history_report(&entries, 2);

    // then
    assert!(rendered.contains("Total            3"));
    assert!(rendered.contains("Showing          2 most recent"));
    assert!(!rendered.contains("older"));
    assert!(rendered.contains("middle"));
    assert!(rendered.contains("latest"));
}

#[test]
fn render_prompt_history_report_handles_empty_history() {
    // given
    let entries: Vec<PromptHistoryEntry> = Vec::new();

    // when
    let rendered = render_prompt_history_report(&entries, 10);

    // then
    assert!(rendered.contains("no prompts recorded yet"));
}

#[test]
fn collect_session_prompt_history_extracts_user_text_blocks() {
    // given
    let mut session = Session::new();
    session.push_user_text("hello").unwrap();
    session.push_user_text("world").unwrap();

    // when
    let entries = collect_session_prompt_history(&session);

    // then
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].text, "hello");
    assert_eq!(entries[1].text, "world");
}

#[test]
fn tool_rendering_helpers_compact_output() {
    let start = format_tool_call_start("read_file", r#"{"path":"src/main.rs"}"#);
    assert!(start.contains("read_file"));
    assert!(start.contains("src/main.rs"));

    let done = format_tool_result(
        "read_file",
        r#"{"file":{"filePath":"src/main.rs","content":"hello","numLines":1,"startLine":1,"totalLines":1}}"#,
        false,
    );
    assert!(done.contains("Read src/main.rs"));
    assert!(done.contains("hello"));
}

#[test]
fn tool_rendering_truncates_large_read_output_for_display_only() {
    let content = (0..200)
        .map(|index| format!("line {index:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = json!({
        "file": {
            "filePath": "src/main.rs",
            "content": content,
            "numLines": 200,
            "startLine": 1,
            "totalLines": 200
        }
    })
    .to_string();

    let rendered = format_tool_result("read_file", &output, false);

    assert!(rendered.contains("line 000"));
    assert!(rendered.contains("line 079"));
    assert!(!rendered.contains("line 199"));
    assert!(rendered.contains("full result preserved in session"));
    assert!(output.contains("line 199"));
}

#[test]
fn tool_rendering_truncates_large_bash_output_for_display_only() {
    let stdout = (0..120)
        .map(|index| format!("stdout {index:03}"))
        .collect::<Vec<_>>()
        .join("\n");
    let output = json!({
        "stdout": stdout,
        "stderr": "",
        "returnCodeInterpretation": "completed successfully"
    })
    .to_string();

    let rendered = format_tool_result("bash", &output, false);

    assert!(rendered.contains("stdout 000"));
    assert!(rendered.contains("stdout 059"));
    assert!(!rendered.contains("stdout 119"));
    assert!(rendered.contains("full result preserved in session"));
    assert!(output.contains("stdout 119"));
}

#[test]
fn tool_rendering_truncates_generic_long_output_for_display_only() {
    let items = (0..120)
        .map(|index| format!("payload {index:03}"))
        .collect::<Vec<_>>();
    let output = json!({
        "summary": "plugin payload",
        "items": items,
    })
    .to_string();

    let rendered = format_tool_result("plugin_echo", &output, false);

    assert!(rendered.contains("plugin_echo"));
    assert!(rendered.contains("payload 000"));
    assert!(rendered.contains("payload 040"));
    assert!(!rendered.contains("payload 080"));
    assert!(!rendered.contains("payload 119"));
    assert!(rendered.contains("full result preserved in session"));
    assert!(output.contains("payload 119"));
}

#[test]
fn tool_rendering_truncates_raw_generic_output_for_display_only() {
    let output = (0..120)
        .map(|index| format!("raw {index:03}"))
        .collect::<Vec<_>>()
        .join("\n");

    let rendered = format_tool_result("plugin_echo", &output, false);

    assert!(rendered.contains("plugin_echo"));
    assert!(rendered.contains("raw 000"));
    assert!(rendered.contains("raw 059"));
    assert!(!rendered.contains("raw 119"));
    assert!(rendered.contains("full result preserved in session"));
    assert!(output.contains("raw 119"));
}

#[test]
fn ultraplan_progress_lines_include_phase_step_and_elapsed_status() {
    let snapshot = InternalPromptProgressState {
        command_label: "Ultraplan",
        task_label: "ship plugin progress".to_string(),
        step: 3,
        phase: "running read_file".to_string(),
        detail: Some("reading rust/crates/rusty-claude-cli/src/main.rs".to_string()),
        saw_final_text: false,
    };

    let started = format_internal_prompt_progress_line(
        InternalPromptProgressEvent::Started,
        &snapshot,
        Duration::from_secs(0),
        None,
    );
    let heartbeat = format_internal_prompt_progress_line(
        InternalPromptProgressEvent::Heartbeat,
        &snapshot,
        Duration::from_secs(9),
        None,
    );
    let completed = format_internal_prompt_progress_line(
        InternalPromptProgressEvent::Complete,
        &snapshot,
        Duration::from_secs(12),
        None,
    );
    let failed = format_internal_prompt_progress_line(
        InternalPromptProgressEvent::Failed,
        &snapshot,
        Duration::from_secs(12),
        Some("network timeout"),
    );

    assert!(started.contains("planning started"));
    assert!(started.contains("current step 3"));
    assert!(heartbeat.contains("heartbeat"));
    assert!(heartbeat.contains("9s elapsed"));
    assert!(heartbeat.contains("phase running read_file"));
    assert!(completed.contains("completed"));
    assert!(completed.contains("3 steps total"));
    assert!(failed.contains("failed"));
    assert!(failed.contains("network timeout"));
}

#[test]
fn describe_tool_progress_summarizes_known_tools() {
    assert_eq!(
        describe_tool_progress("read_file", r#"{"path":"src/main.rs"}"#),
        "reading src/main.rs"
    );
    assert!(
        describe_tool_progress("bash", r#"{"command":"cargo test -p rusty-claude-cli"}"#)
            .contains("cargo test -p rusty-claude-cli")
    );
    assert_eq!(
        describe_tool_progress("grep_search", r#"{"pattern":"ultraplan","path":"rust"}"#),
        "grep `ultraplan` in rust"
    );
}

#[test]
fn push_output_block_renders_markdown_text() {
    let mut out = Vec::new();
    let mut events = Vec::new();
    let mut pending_tool = None;
    let mut block_has_thinking_summary = false;

    push_output_block(
        OutputContentBlock::Text {
            text: "# Heading".to_string(),
        },
        &mut out,
        &mut events,
        &mut pending_tool,
        false,
        &mut block_has_thinking_summary,
    )
    .expect("text block should render");

    let rendered = String::from_utf8(out).expect("utf8");
    assert!(rendered.contains("Heading"));
    assert!(rendered.contains('\u{1b}'));
}

#[test]
fn push_output_block_skips_empty_object_prefix_for_tool_streams() {
    let mut out = Vec::new();
    let mut events = Vec::new();
    let mut pending_tool = None;
    let mut block_has_thinking_summary = false;

    push_output_block(
        OutputContentBlock::ToolUse {
            id: "tool-1".to_string(),
            name: "read_file".to_string(),
            input: json!({}),
        },
        &mut out,
        &mut events,
        &mut pending_tool,
        true,
        &mut block_has_thinking_summary,
    )
    .expect("tool block should accumulate");

    assert!(events.is_empty());
    assert_eq!(
        pending_tool,
        Some(("tool-1".to_string(), "read_file".to_string(), String::new(),))
    );
}

#[test]
fn response_to_events_preserves_empty_object_json_input_outside_streaming() {
    let mut out = Vec::new();
    let events = response_to_events(
        MessageResponse {
            id: "msg-1".to_string(),
            kind: "message".to_string(),
            model: "claude-opus-4-6".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "read_file".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            stop_sequence: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            request_id: None,
        },
        &mut out,
    )
    .expect("response conversion should succeed");

    assert!(matches!(
        &events[0],
        AssistantEvent::ToolUse { name, input, .. }
            if name == "read_file" && input == "{}"
    ));
}

#[test]
fn response_to_events_preserves_non_empty_json_input_outside_streaming() {
    let mut out = Vec::new();
    let events = response_to_events(
        MessageResponse {
            id: "msg-2".to_string(),
            kind: "message".to_string(),
            model: "claude-opus-4-6".to_string(),
            role: "assistant".to_string(),
            content: vec![OutputContentBlock::ToolUse {
                id: "tool-2".to_string(),
                name: "read_file".to_string(),
                input: json!({ "path": "rust/Cargo.toml" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            stop_sequence: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            request_id: None,
        },
        &mut out,
    )
    .expect("response conversion should succeed");

    assert!(matches!(
        &events[0],
        AssistantEvent::ToolUse { name, input, .. }
            if name == "read_file" && input == "{\"path\":\"rust/Cargo.toml\"}"
    ));
}

#[test]
fn response_to_events_renders_collapsed_thinking_summary() {
    let mut out = Vec::new();
    let events = response_to_events(
        MessageResponse {
            id: "msg-3".to_string(),
            kind: "message".to_string(),
            model: "claude-opus-4-6".to_string(),
            role: "assistant".to_string(),
            content: vec![
                OutputContentBlock::Thinking {
                    thinking: "step 1".to_string(),
                    signature: Some("sig_123".to_string()),
                },
                OutputContentBlock::Text {
                    text: "Final answer".to_string(),
                },
            ],
            stop_reason: Some("end_turn".to_string()),
            stop_sequence: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 1,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
            },
            request_id: None,
        },
        &mut out,
    )
    .expect("response conversion should succeed");

    assert!(matches!(
        &events[0],
        AssistantEvent::TextDelta(text) if text == "Final answer"
    ));
    let rendered = String::from_utf8(out).expect("utf8");
    assert!(rendered.contains("â–¶ Thinking (6 chars hidden)"));
    assert!(!rendered.contains("step 1"));
}

#[test]
fn build_runtime_plugin_state_merges_plugin_hooks_into_runtime_features() {
    let config_home = temp_dir();
    let workspace = temp_dir();
    let source_root = temp_dir();
    fs::create_dir_all(&config_home).expect("config home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&source_root).expect("source root");
    write_plugin_fixture(&source_root, "hook-runtime-demo", true, false);

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    manager
        .install(source_root.to_str().expect("utf8 source path"))
        .expect("plugin install should succeed");
    let loader = ConfigLoader::new(&workspace, &config_home);
    let runtime_config = loader.load().expect("runtime config should load");
    let state = build_runtime_plugin_state_with_loader(&workspace, &loader, &runtime_config)
        .expect("plugin state should load");
    let pre_hooks = state.feature_config.hooks().pre_tool_use();
    assert_eq!(pre_hooks.len(), 1);
    assert!(
        pre_hooks[0].ends_with("hooks/pre.sh"),
        "expected installed plugin hook path, got {pre_hooks:?}"
    );

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(source_root);
}

#[test]
#[allow(clippy::too_many_lines)]
fn build_runtime_plugin_state_discovers_mcp_tools_and_surfaces_pending_servers() {
    let config_home = temp_dir();
    let workspace = temp_dir();
    fs::create_dir_all(&config_home).expect("config home");
    fs::create_dir_all(&workspace).expect("workspace");
    let script_path = workspace.join("fixture-mcp.py");
    write_mcp_server_fixture(&script_path);
    fs::write(
        config_home.join("settings.json"),
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "alpha": {
                    "command": "python3",
                    "args": [script_path.to_string_lossy()]
                },
                "broken": {
                    "command": "python3",
                    "args": ["-c", "import sys; sys.exit(0)"]
                }
            }
        }))
        .expect("mcp settings should serialize"),
    )
    .expect("write mcp settings");

    let loader = ConfigLoader::new(&workspace, &config_home);
    let runtime_config = loader.load().expect("runtime config should load");
    let state = build_runtime_plugin_state_with_loader(&workspace, &loader, &runtime_config)
        .expect("runtime plugin state should load");

    let allowed = state
        .tool_registry
        .normalize_allowed_tools(&["mcp__alpha__echo".to_string(), "MCPTool".to_string()])
        .expect("mcp tools should be allow-listable")
        .expect("allow-list should exist");
    assert!(allowed.contains("mcp__alpha__echo"));
    assert!(allowed.contains("MCPTool"));

    let mut executor = CliToolExecutor::new(
        None,
        false,
        state.tool_registry.clone(),
        state.mcp_state.clone(),
    );

    let tool_output = executor
        .execute("mcp__alpha__echo", r#"{"text":"hello"}"#)
        .expect("discovered mcp tool should execute");
    let tool_json: serde_json::Value =
        serde_json::from_str(&tool_output).expect("tool output should be json");
    assert_eq!(tool_json["structuredContent"]["echoed"], "hello");

    let wrapped_output = executor
        .execute(
            "MCPTool",
            r#"{"qualifiedName":"mcp__alpha__echo","arguments":{"text":"wrapped"}}"#,
        )
        .expect("generic mcp wrapper should execute");
    let wrapped_json: serde_json::Value =
        serde_json::from_str(&wrapped_output).expect("wrapped output should be json");
    assert_eq!(wrapped_json["structuredContent"]["echoed"], "wrapped");

    let search_output = executor
        .execute("ToolSearch", r#"{"query":"alpha echo","max_results":5}"#)
        .expect("tool search should execute");
    let search_json: serde_json::Value =
        serde_json::from_str(&search_output).expect("search output should be json");
    assert_eq!(search_json["matches"][0], "mcp__alpha__echo");
    assert_eq!(search_json["pending_mcp_servers"][0], "broken");
    assert_eq!(
        search_json["mcp_degraded"]["failed_servers"][0]["server_name"],
        "broken"
    );
    assert_eq!(
        search_json["mcp_degraded"]["failed_servers"][0]["phase"],
        "tool_discovery"
    );
    assert_eq!(
        search_json["mcp_degraded"]["available_tools"][0],
        "mcp__alpha__echo"
    );

    let listed = executor
        .execute("ListMcpResourcesTool", r#"{"server":"alpha"}"#)
        .expect("resources should list");
    let listed_json: serde_json::Value =
        serde_json::from_str(&listed).expect("resource output should be json");
    assert_eq!(listed_json["resources"][0]["uri"], "file://guide.txt");

    let read = executor
        .execute(
            "ReadMcpResourceTool",
            r#"{"server":"alpha","uri":"file://guide.txt"}"#,
        )
        .expect("resource should read");
    let read_json: serde_json::Value =
        serde_json::from_str(&read).expect("resource read output should be json");
    assert_eq!(
        read_json["contents"][0]["text"],
        "contents for file://guide.txt"
    );

    if let Some(mcp_state) = state.mcp_state {
        mcp_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .shutdown()
            .expect("mcp shutdown should succeed");
    }

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
fn build_runtime_plugin_state_surfaces_unsupported_mcp_servers_structurally() {
    let config_home = temp_dir();
    let workspace = temp_dir();
    fs::create_dir_all(&config_home).expect("config home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::write(
        config_home.join("settings.json"),
        r#"{
          "mcpServers": {
            "remote": {
              "url": "https://example.test/mcp"
            }
          }
        }"#,
    )
    .expect("write mcp settings");

    let loader = ConfigLoader::new(&workspace, &config_home);
    let runtime_config = loader.load().expect("runtime config should load");
    let state = build_runtime_plugin_state_with_loader(&workspace, &loader, &runtime_config)
        .expect("runtime plugin state should load");
    let mut executor = CliToolExecutor::new(
        None,
        false,
        state.tool_registry.clone(),
        state.mcp_state.clone(),
    );

    let search_output = executor
        .execute("ToolSearch", r#"{"query":"remote","max_results":5}"#)
        .expect("tool search should execute");
    let search_json: serde_json::Value =
        serde_json::from_str(&search_output).expect("search output should be json");
    assert_eq!(search_json["pending_mcp_servers"][0], "remote");
    assert_eq!(
        search_json["mcp_degraded"]["failed_servers"][0]["server_name"],
        "remote"
    );
    assert_eq!(
        search_json["mcp_degraded"]["failed_servers"][0]["phase"],
        "server_registration"
    );
    assert_eq!(
        search_json["mcp_degraded"]["failed_servers"][0]["error"]["context"]["transport"],
        "http"
    );

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(workspace);
}

#[test]
#[cfg(unix)]
fn build_runtime_runs_plugin_lifecycle_init_and_shutdown() {
    // Serialize access to process-wide env vars so parallel tests that
    // set/remove ANTHROPIC_API_KEY do not race with this test.
    let _guard = env_lock();
    let config_home = temp_dir();
    // Inject a dummy API key so runtime construction succeeds without real credentials.
    // This test only exercises plugin lifecycle (init/shutdown), never calls the API.
    std::env::set_var("ANTHROPIC_API_KEY", "test-dummy-key-for-plugin-lifecycle");
    let workspace = temp_dir();
    let source_root = temp_dir();
    fs::create_dir_all(&config_home).expect("config home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&source_root).expect("source root");
    write_plugin_fixture(&source_root, "lifecycle-runtime-demo", false, true);

    let mut manager = PluginManager::new(PluginManagerConfig::new(&config_home));
    let install = manager
        .install(source_root.to_str().expect("utf8 source path"))
        .expect("plugin install should succeed");
    let log_path = install.install_path.join("lifecycle.log");
    let loader = ConfigLoader::new(&workspace, &config_home);
    let runtime_config = loader.load().expect("runtime config should load");
    let runtime_plugin_state =
        build_runtime_plugin_state_with_loader(&workspace, &loader, &runtime_config)
            .expect("plugin state should load");
    let mut runtime = build_runtime_with_plugin_state(
        Session::new(),
        "runtime-plugin-lifecycle",
        DEFAULT_MODEL.to_string(),
        vec!["test system prompt".to_string()],
        true,
        false,
        None,
        PermissionMode::DangerFullAccess,
        None,
        runtime_plugin_state,
    )
    .expect("runtime should build");

    assert_eq!(
        fs::read_to_string(&log_path).expect("init log should exist"),
        "init\n"
    );

    runtime
        .shutdown_plugins()
        .expect("plugin shutdown should succeed");

    assert_eq!(
        fs::read_to_string(&log_path).expect("shutdown log should exist"),
        "init\nshutdown\n"
    );

    let _ = fs::remove_dir_all(config_home);
    let _ = fs::remove_dir_all(workspace);
    let _ = fs::remove_dir_all(source_root);
    std::env::remove_var("ANTHROPIC_API_KEY");
}

#[test]
fn rejects_invalid_reasoning_effort_value() {
    let err = parse_args(&[
        "--reasoning-effort".to_string(),
        "turbo".to_string(),
        "prompt".to_string(),
        "hello".to_string(),
    ])
    .unwrap_err();
    assert!(
        err.contains("invalid value for --reasoning-effort"),
        "unexpected error: {err}"
    );
    assert!(err.contains("turbo"), "unexpected error: {err}");
}

#[test]
fn accepts_valid_reasoning_effort_values() {
    for value in ["low", "medium", "high"] {
        let result = parse_args(&[
            "--reasoning-effort".to_string(),
            value.to_string(),
            "prompt".to_string(),
            "hello".to_string(),
        ]);
        assert!(
            result.is_ok(),
            "--reasoning-effort {value} should be accepted, got: {result:?}"
        );
        if let Ok(CliAction::Prompt {
            reasoning_effort, ..
        }) = result
        {
            assert_eq!(reasoning_effort.as_deref(), Some(value));
        }
    }
}

#[test]
fn stub_commands_absent_from_repl_completions() {
    let candidates =
        slash_command_completion_candidates_with_sessions("claude-3-5-sonnet", None, vec![]);
    for stub in STUB_COMMANDS {
        let with_slash = format!("/{stub}");
        assert!(
            !candidates.contains(&with_slash),
            "stub command {with_slash} should not appear in REPL completions"
        );
    }
}

fn write_mcp_server_fixture(script_path: &Path) {
    let script = [
        "#!/usr/bin/env python3",
        "import json, sys",
        "",
        "def read_message():",
        "    header = b''",
        r"    while not header.endswith(b'\r\n\r\n'):",
        "        chunk = sys.stdin.buffer.read(1)",
        "        if not chunk:",
        "            return None",
        "        header += chunk",
        "    length = 0",
        r"    for line in header.decode().split('\r\n'):",
        r"        if line.lower().startswith('content-length:'):",
        "            length = int(line.split(':', 1)[1].strip())",
        "    payload = sys.stdin.buffer.read(length)",
        "    return json.loads(payload.decode())",
        "",
        "def send_message(message):",
        "    payload = json.dumps(message).encode()",
        r"    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)",
        "    sys.stdout.buffer.flush()",
        "",
        "while True:",
        "    request = read_message()",
        "    if request is None:",
        "        break",
        "    method = request['method']",
        "    if method == 'initialize':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {",
        "                'protocolVersion': request['params']['protocolVersion'],",
        "                'capabilities': {'tools': {}, 'resources': {}},",
        "                'serverInfo': {'name': 'fixture', 'version': '1.0.0'}",
        "            }",
        "        })",
        "    elif method == 'tools/list':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {",
        "                'tools': [",
        "                    {",
        "                        'name': 'echo',",
        "                        'description': 'Echo from MCP fixture',",
        "                        'inputSchema': {",
        "                            'type': 'object',",
        "                            'properties': {'text': {'type': 'string'}},",
        "                            'required': ['text'],",
        "                            'additionalProperties': False",
        "                        },",
        "                        'annotations': {'readOnlyHint': True}",
        "                    }",
        "                ]",
        "            }",
        "        })",
        "    elif method == 'tools/call':",
        "        args = request['params'].get('arguments') or {}",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {",
        "                'content': [{'type': 'text', 'text': f\"echo:{args.get('text', '')}\"}],",
        "                'structuredContent': {'echoed': args.get('text', '')},",
        "                'isError': False",
        "            }",
        "        })",
        "    elif method == 'resources/list':",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {",
        "                'resources': [{'uri': 'file://guide.txt', 'name': 'guide', 'mimeType': 'text/plain'}]",
        "            }",
        "        })",
        "    elif method == 'resources/read':",
        "        uri = request['params']['uri']",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'result': {",
        "                'contents': [{'uri': uri, 'mimeType': 'text/plain', 'text': f'contents for {uri}'}]",
        "            }",
        "        })",
        "    else:",
        "        send_message({",
        "            'jsonrpc': '2.0',",
        "            'id': request['id'],",
        "            'error': {'code': -32601, 'message': method}",
        "        })",
        "",
    ]
    .join("\n");
    fs::write(script_path, script).expect("mcp fixture script should write");
}

#[cfg(test)]
mod sandbox_report_tests {
    use crate::{format_sandbox_report, HookAbortMonitor};
    use runtime::HookAbortSignal;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn sandbox_report_renders_expected_fields() {
        let report = format_sandbox_report(&runtime::SandboxStatus::default());
        assert!(report.contains("Sandbox"));
        assert!(report.contains("Enabled"));
        assert!(report.contains("Filesystem mode"));
        assert!(report.contains("Fallback reason"));
    }

    #[test]
    fn hook_abort_monitor_stops_without_aborting() {
        let abort_signal = HookAbortSignal::new();
        let (ready_tx, ready_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |stop_rx, abort_signal| {
                ready_tx.send(()).expect("ready signal");
                let _ = stop_rx.recv();
                assert!(!abort_signal.is_aborted());
            },
        );

        ready_rx.recv().expect("waiter should be ready");
        monitor.stop();

        assert!(!abort_signal.is_aborted());
    }

    #[test]
    fn hook_abort_monitor_propagates_interrupt() {
        let abort_signal = HookAbortSignal::new();
        let (done_tx, done_rx) = mpsc::channel();
        let monitor = HookAbortMonitor::spawn_with_waiter(
            abort_signal.clone(),
            move |_stop_rx, abort_signal| {
                abort_signal.abort();
                done_tx.send(()).expect("done signal");
            },
        );

        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("interrupt should complete");
        monitor.stop();

        assert!(abort_signal.is_aborted());
    }
}

#[cfg(test)]
mod dump_manifests_tests {
    use crate::{dump_manifests_at_path, CliOutputFormat};
    use std::fs;

    #[test]
    fn dump_manifests_shows_helpful_error_when_manifests_missing() {
        let root = std::env::temp_dir().join(format!(
            "claw_test_missing_manifests_{}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("failed to create temp workspace");

        let result = dump_manifests_at_path(&workspace, None, CliOutputFormat::Text);
        assert!(
            result.is_err(),
            "expected an error when manifests are missing"
        );

        let error_msg = result.unwrap_err().to_string();

        assert!(
            error_msg.contains("Manifest source files are missing"),
            "error message should mention missing manifest sources: {error_msg}"
        );
        assert!(
            error_msg.contains(&root.display().to_string()),
            "error message should contain the resolved repo root path: {error_msg}"
        );
        assert!(
            error_msg.contains("src/commands.ts"),
            "error message should mention missing commands.ts: {error_msg}"
        );
        assert!(
            error_msg.contains("CLAUDE_CODE_UPSTREAM"),
            "error message should explain how to supply the upstream path: {error_msg}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dump_manifests_uses_explicit_manifest_dir() {
        let root = std::env::temp_dir().join(format!(
            "claw_test_explicit_manifest_dir_{}",
            std::process::id()
        ));
        let workspace = root.join("workspace");
        let upstream = root.join("upstream");
        fs::create_dir_all(workspace.join("nested")).expect("workspace should exist");
        fs::create_dir_all(upstream.join("src/entrypoints"))
            .expect("upstream fixture should exist");
        fs::write(
            upstream.join("src/commands.ts"),
            "import FooCommand from './commands/foo'\n",
        )
        .expect("commands fixture should write");
        fs::write(
            upstream.join("src/tools.ts"),
            "import ReadTool from './tools/read'\n",
        )
        .expect("tools fixture should write");
        fs::write(
            upstream.join("src/entrypoints/cli.tsx"),
            "startupProfiler()\n",
        )
        .expect("cli fixture should write");

        let result = dump_manifests_at_path(&workspace, Some(&upstream), CliOutputFormat::Text);
        assert!(
            result.is_ok(),
            "explicit manifest dir should succeed: {result:?}"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
