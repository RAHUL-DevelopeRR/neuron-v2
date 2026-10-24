//! Core runtime primitives for the `claw` CLI and supporting crates.
//!
//! This crate owns session persistence, permission evaluation, prompt assembly,
//! MCP plumbing, tool-facing file operations, and the core conversation loop
//! that drives interactive and one-shot turns.

#![allow(
    dead_code,
    unused_imports,
    clippy::duration_suboptimal_units,
    clippy::unnecessary_wraps
)]

mod bash;
pub mod bash_validation;
mod bootstrap;
pub mod branch_lock;
pub mod cancel;
mod compact;
mod config;
pub mod config_validate;
pub mod context_manager;
mod conversation;
mod default_skill_files;
pub mod default_skills;
mod file_ops;
mod git_context;
pub mod green_contract;
mod hooks;
mod json;
mod lane_events;
pub mod lsp_client;
mod mcp;
mod mcp_client;
pub mod mcp_lifecycle_hardened;
pub mod mcp_server;
mod mcp_stdio;
pub mod mcp_tool_bridge;
pub mod model_registry;
mod oauth;
pub mod permission_enforcer;
mod permissions;
pub mod plugin_lifecycle;
mod policy_engine;
mod prompt;
pub mod recovery_recipes;
mod remote;
pub mod sandbox;
pub mod sensitive_paths;
mod session;
pub mod session_control;
pub use session_control::SessionStore;
pub mod experience;
pub mod project_checks;
mod sse;
pub mod stale_base;
pub mod stale_branch;
pub mod summary_compression;
pub mod task_packet;
pub mod task_registry;
pub mod team_cron_registry;
pub mod tool_input;
pub mod transcript;
#[cfg(test)]
mod trust_resolver;
pub mod ui_hooks;
mod usage;
pub mod worker_boot;
pub mod workspace_trust;

pub use bash::{
    bash_tool_shell_description, effective_bash_timeout_ms, execute_bash, execute_bash_with_cancel,
    stop_background_commands, BashCommandInput, BashCommandOutput, DEFAULT_BASH_TIMEOUT_MS,
    MAX_BASH_TIMEOUT_MS,
};
pub use bootstrap::{BootstrapPhase, BootstrapPlan};
pub use branch_lock::{detect_branch_lock_collisions, BranchLockCollision, BranchLockIntent};
pub use cancel::CancellationToken;
pub use compact::{
    compact_session, compact_session_to_token_budget, compact_session_with_policy,
    estimate_session_tokens, format_compact_summary, get_compact_continuation_message,
    should_compact, CompactionConfig, CompactionResult, TokenCompaction, AUTO_CONTINUE_PREFIX,
    DEFAULT_MAX_SUMMARY_TOKENS, DERIVED_CONTEXT_NOTE,
};
pub use config::{
    ConfigEntry, ConfigError, ConfigLoader, ConfigSource, IgnoredProjectSettings,
    McpConfigCollection, McpManagedProxyServerConfig, McpOAuthConfig, McpRemoteServerConfig,
    McpSdkServerConfig, McpServerConfig, McpStdioServerConfig, McpTransport,
    McpWebSocketServerConfig, OAuthConfig, PreviewHook, PreviewMcpServer, PreviewPermissionRule,
    ProjectSettingsAction, ProjectTrust, ProviderFallbackConfig, ResolvedPermissionMode,
    RuntimeConfig, RuntimeFeatureConfig, RuntimeHookConfig, RuntimePermissionRuleConfig,
    RuntimePluginConfig, ScopedMcpServerConfig, WorkspaceTrustPreview, CLAW_SETTINGS_SCHEMA_NAME,
    UNTRUSTED_WORKSPACE_KEYS,
};
pub use config_validate::{
    check_unsupported_format, format_diagnostics, validate_config_file, ConfigDiagnostic,
    DiagnosticKind, ValidationResult,
};
pub use conversation::{
    auto_compaction_threshold_from_env, auto_compaction_threshold_override_from_env,
    is_truncation_stop_reason, is_turn_end_note, resolve_max_iterations, ApiClient, ApiRequest,
    AssistantEvent, AutoCompactionEvent, ChangeReviewer, ConversationRuntime, HarnessConfig,
    PromptCacheEvent, ReviewRequest, RuntimeError, RuntimeErrorKind, StaticToolExecutor, ToolError,
    ToolExecutor, TurnOutcome, TurnSummary, AUTO_CHECKS_ENV_VAR, COMPLETION_CHECK_ENV_VAR,
    DEFAULT_MAX_ITERATIONS, FAILED_RESPONSE_NOTE_PREFIX, INTERRUPTED_RESPONSE_NOTE,
    INTERRUPTED_TOOL_RESULT, MAX_ITERATIONS_ENV_VAR,
};
pub use file_ops::{
    display_path, edit_file, edit_file_in_workspace, glob_search, grep_search,
    is_blank_search_path, read_file, read_file_in_workspace, write_file, write_file_in_workspace,
    EditFileOutput, GlobSearchOutput, GrepSearchInput, GrepSearchOutput, ReadFileOutput,
    StructuredPatchHunk, TextFilePayload, WriteFileOutput,
};
pub use git_context::{GitCommitEntry, GitContext};
pub use hooks::{
    HookAbortSignal, HookEvent, HookProgressEvent, HookProgressReporter, HookRunResult, HookRunner,
};
pub use lane_events::{
    compute_event_fingerprint, dedupe_superseded_commit_events, dedupe_terminal_events,
    is_terminal_event, EventProvenance, LaneCommitProvenance, LaneEvent, LaneEventBlocker,
    LaneEventBuilder, LaneEventMetadata, LaneEventName, LaneEventStatus, LaneFailureClass,
    LaneOwnership, SessionIdentity, WatcherAction,
};
pub use mcp::{
    mcp_server_signature, mcp_tool_name, mcp_tool_prefix, normalize_name_for_mcp,
    scoped_mcp_config_hash, unwrap_ccr_proxy_url,
};
pub use mcp_client::{
    McpClientAuth, McpClientBootstrap, McpClientTransport, McpManagedProxyTransport,
    McpRemoteTransport, McpSdkTransport, McpStdioTransport,
};
pub use mcp_lifecycle_hardened::{
    McpDegradedReport, McpErrorSurface, McpFailedServer, McpLifecyclePhase, McpLifecycleState,
    McpLifecycleValidator, McpPhaseResult,
};
pub use mcp_server::{McpServer, McpServerSpec, ToolCallHandler, MCP_SERVER_PROTOCOL_VERSION};
pub use mcp_stdio::{
    set_mcp_server_log, spawn_mcp_stdio_process, JsonRpcError, JsonRpcId, JsonRpcRequest,
    JsonRpcResponse, ManagedMcpTool, McpDiscoveryFailure, McpInitializeClientInfo,
    McpInitializeParams, McpInitializeResult, McpInitializeServerInfo, McpListResourcesParams,
    McpListResourcesResult, McpListToolsParams, McpListToolsResult, McpReadResourceParams,
    McpReadResourceResult, McpResource, McpResourceContents, McpServerManager,
    McpServerManagerError, McpStdioProcess, McpTool, McpToolCallContent, McpToolCallParams,
    McpToolCallResult, McpToolDiscoveryReport, UnsupportedMcpServer,
};
pub use oauth::{
    clear_oauth_credentials, code_challenge_s256, credentials_path, generate_pkce_pair,
    generate_state, load_oauth_credentials, loopback_redirect_uri, parse_oauth_callback_query,
    parse_oauth_callback_request_target, save_oauth_credentials, OAuthAuthorizationRequest,
    OAuthCallbackParams, OAuthRefreshRequest, OAuthTokenExchangeRequest, OAuthTokenSet,
    PkceChallengeMethod, PkceCodePair,
};
pub use permissions::{
    parse_permission_response, render_permission_prompt, url_host, ConsolePermissionPrompter,
    PermissionContext, PermissionMode, PermissionOutcome, PermissionOverride, PermissionPolicy,
    PermissionPromptDecision, PermissionPrompter, PermissionRequest, SessionApprovals,
    ToolCallAssessment, NON_INTERACTIVE_PERMISSION_HINT,
};
pub use plugin_lifecycle::{
    DegradedMode, DiscoveryResult, PluginHealthcheck, PluginLifecycle, PluginLifecycleEvent,
    PluginState, ResourceInfo, ServerHealth, ServerStatus, ToolInfo,
};
pub use policy_engine::{
    evaluate, DiffScope, GreenLevel, LaneBlocker, LaneContext, PolicyAction, PolicyCondition,
    PolicyEngine, PolicyRule, ReconcileReason, ReviewStatus,
};
pub use prompt::{
    load_system_prompt, prepend_bullets, set_prompt_model, workspace_git_status, ContextFile,
    ProjectContext, PromptBuildError, SystemPromptBuilder, SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
};
pub use recovery_recipes::{
    attempt_recovery, recipe_for, EscalationPolicy, FailureScenario, RecoveryContext,
    RecoveryEvent, RecoveryRecipe, RecoveryResult, RecoveryStep,
};
pub use remote::{
    inherited_upstream_proxy_env, no_proxy_list, read_token, upstream_proxy_ws_url,
    RemoteSessionContext, UpstreamProxyBootstrap, UpstreamProxyState, DEFAULT_REMOTE_BASE_URL,
    DEFAULT_SESSION_TOKEN_PATH, DEFAULT_SYSTEM_CA_BUNDLE, NO_PROXY_HOSTS, UPSTREAM_PROXY_ENV_KEYS,
};
pub use sandbox::{
    build_linux_sandbox_command, detect_container_environment, detect_container_environment_from,
    resolve_sandbox_status, resolve_sandbox_status_for_request, ContainerEnvironment,
    FilesystemIsolationMode, LinuxSandboxCommand, SandboxConfig, SandboxDetectionInputs,
    SandboxDirs, SandboxRequest, SandboxStatus,
};
pub use session::{
    ContentBlock, ConversationMessage, MessageRole, Session, SessionCompaction, SessionError,
    SessionFork, SessionLoadDiagnostics, SessionPromptEntry,
};
pub use sse::{IncrementalSseParser, SseEvent};
pub use stale_base::{
    check_base_commit, format_stale_base_warning, read_claw_base_file, resolve_expected_base,
    BaseCommitSource, BaseCommitState,
};
pub use stale_branch::{
    apply_policy, check_freshness, BranchFreshness, StaleBranchAction, StaleBranchEvent,
    StaleBranchPolicy,
};
pub use task_packet::{validate_packet, TaskPacket, TaskPacketValidationError, ValidatedPacket};
#[cfg(test)]
pub use trust_resolver::{TrustConfig, TrustDecision, TrustEvent, TrustPolicy, TrustResolver};
pub use usage::{
    format_usd, pricing_for_model, ModelPricing, TokenUsage, UsageCostEstimate, UsageTracker,
};
pub use worker_boot::{
    Worker, WorkerEvent, WorkerEventKind, WorkerEventPayload, WorkerFailure, WorkerFailureKind,
    WorkerPromptTarget, WorkerReadySnapshot, WorkerRegistry, WorkerStatus, WorkerTrustResolution,
};

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
