//! LLM provider selection.
//!
//! Precedence (contract §5):
//!   1. `NEURON_TOKEN` (`ses_`/`nzx_`) in the environment: the Zero-X gateway.
//!   2. An explicit choice: `--provider <id>`, then `NEURON_PROVIDER`, then
//!      the settings key `"provider"`.
//!   3. The stored Zero-X login.
//!   4. Ambient provider environment keys (Anthropic, xAI, Azure, `OpenAI`,
//!      a local server, `OpenRouter`), and finally the Zero-X sign-in.
//!
//! Credentials are handed to the API client in memory
//! (`api::install_openai_compat_endpoint`); nothing is exported into the
//! process environment, so tool subprocesses never inherit them.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::brand::*;
use api::{detect_provider_kind, OpenAiCompatConfig, OpenAiCompatEndpoint, ProviderKind};
use serde_json::Value;

/// The default `--model` value: let the provider pick.
pub const AUTO_MODEL: &str = "auto";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const DEFAULT_LOCAL_BASE_URL: &str = "http://127.0.0.1:11434/v1";
const DEFAULT_OPENROUTER_MODEL: &str = "qwen/qwen3-coder-480b-a35b-instruct:free";
const DEFAULT_AZURE_MODEL: &str = "Kimi-K2.5";
const DEFAULT_ANTHROPIC_MODEL: &str = "claude-opus-4-6";
const DEFAULT_XAI_MODEL: &str = "grok-3";

/// Providers a user can choose explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderId {
    ZeroX,
    OpenAi,
    OpenRouter,
    Azure,
    Anthropic,
    Xai,
    Local,
}

impl ProviderId {
    pub const ALL: [Self; 7] = [
        Self::ZeroX,
        Self::OpenAi,
        Self::OpenRouter,
        Self::Azure,
        Self::Anthropic,
        Self::Xai,
        Self::Local,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "zero-x" | "zerox" | "zero_x" | "gateway" => Self::ZeroX,
            "openai" => Self::OpenAi,
            "openrouter" => Self::OpenRouter,
            "azure" => Self::Azure,
            "anthropic" => Self::Anthropic,
            "xai" | "grok" => Self::Xai,
            "local" => Self::Local,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ZeroX => "zero-x",
            Self::OpenAi => "openai",
            Self::OpenRouter => "openrouter",
            Self::Azure => "azure",
            Self::Anthropic => "anthropic",
            Self::Xai => "xai",
            Self::Local => "local",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ZeroX => "Zero-X gateway",
            Self::OpenAi => "OpenAI",
            Self::OpenRouter => "OpenRouter",
            Self::Azure => "Azure AI Foundry",
            Self::Anthropic => "Anthropic",
            Self::Xai => "xAI",
            Self::Local => "local OpenAI-compatible server",
        }
    }

    pub(crate) fn names() -> String {
        Self::ALL
            .iter()
            .map(|id| id.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// Parses a provider name, with an actionable error for typos.
pub fn parse_provider_id(value: &str, origin: &str) -> Result<ProviderId, String> {
    ProviderId::parse(value).ok_or_else(|| {
        format!(
            "unknown provider '{value}' from {origin}; expected one of {}",
            ProviderId::names()
        )
    })
}

/// Where an explicit provider choice came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceOrigin {
    Flag,
    Env,
    Settings,
}

impl ChoiceOrigin {
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Flag => "--provider",
            Self::Env => "NEURON_PROVIDER",
            Self::Settings => "settings \"provider\"",
        }
    }
}

/// Why a provider was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionReason {
    NeuronToken,
    Explicit(ChoiceOrigin),
    StoredLogin,
    /// An ambient credential environment variable.
    Ambient(&'static str),
    /// Nothing configured: sign in to Zero-X.
    SignInRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub provider: ProviderId,
    pub reason: SelectionReason,
}

/// Everything selection depends on, captured up front so the precedence
/// rules are a pure function. Each flag mirrors one independent source.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderInputs {
    pub neuron_token: bool,
    pub flag: Option<ProviderId>,
    pub env_choice: Option<ProviderId>,
    pub settings_choice: Option<ProviderId>,
    pub stored_login: bool,
    pub anthropic_auth: bool,
    pub xai_key: bool,
    pub azure: bool,
    pub openai_key: bool,
    pub local_base_url: bool,
    pub openrouter_key: bool,
    pub requested_model: String,
}

/// Applies the precedence rules. `azure_usable` is only consulted when an
/// ambient Azure configuration is reached (it may probe the endpoint).
pub fn select_provider(inputs: &ProviderInputs, azure_usable: &dyn Fn() -> bool) -> Selection {
    let select = |provider, reason| Selection { provider, reason };
    if inputs.neuron_token {
        return select(ProviderId::ZeroX, SelectionReason::NeuronToken);
    }
    for (choice, origin) in [
        (inputs.flag, ChoiceOrigin::Flag),
        (inputs.env_choice, ChoiceOrigin::Env),
        (inputs.settings_choice, ChoiceOrigin::Settings),
    ] {
        if let Some(provider) = choice {
            return select(provider, SelectionReason::Explicit(origin));
        }
    }
    if inputs.stored_login {
        return select(ProviderId::ZeroX, SelectionReason::StoredLogin);
    }
    let model = inputs.requested_model.as_str();
    let wants_anthropic = model == AUTO_MODEL
        || (model.starts_with("claude") && detect_provider_kind(model) == ProviderKind::Anthropic);
    if inputs.anthropic_auth && wants_anthropic {
        return select(
            ProviderId::Anthropic,
            SelectionReason::Ambient("ANTHROPIC_API_KEY"),
        );
    }
    if inputs.xai_key && model.starts_with("grok") {
        return select(ProviderId::Xai, SelectionReason::Ambient("XAI_API_KEY"));
    }
    if inputs.azure && azure_usable() {
        return select(
            ProviderId::Azure,
            SelectionReason::Ambient("AZURE_OPENAI_API_KEY"),
        );
    }
    if inputs.openai_key {
        return select(
            ProviderId::OpenAi,
            SelectionReason::Ambient("OPENAI_API_KEY"),
        );
    }
    if inputs.local_base_url {
        return select(
            ProviderId::Local,
            SelectionReason::Ambient("OPENAI_BASE_URL"),
        );
    }
    if inputs.openrouter_key {
        return select(
            ProviderId::OpenRouter,
            SelectionReason::Ambient("OPENROUTER_API_KEY"),
        );
    }
    select(ProviderId::ZeroX, SelectionReason::SignInRequired)
}

/// The provider this process talks to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveProvider {
    pub id: ProviderId,
    pub reason: SelectionReason,
    /// Human description of the credential source (never the secret).
    pub credential_source: String,
    pub base_url: Option<String>,
    pub model: String,
    pub plan: Option<String>,
}

impl ActiveProvider {
    /// The one startup line naming the provider and credential source.
    pub fn startup_line(&self) -> String {
        let mut line = format!(
            "Provider: {} · credential: {}",
            self.id.display_name(),
            self.credential_source
        );
        if let SelectionReason::Explicit(origin) = self.reason {
            line.push_str(&format!(" · selected by {}", origin.describe()));
        }
        if let Some(base_url) = &self.base_url {
            line.push_str(&format!(" · {base_url}"));
        }
        line
    }
}

fn active_slot() -> &'static RwLock<Option<ActiveProvider>> {
    static SLOT: OnceLock<RwLock<Option<ActiveProvider>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// The provider configured by [`configure_provider`], if any.
pub fn active_provider() -> Option<ActiveProvider> {
    active_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn set_active_provider(active: ActiveProvider) {
    *active_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(active);
}

/// `--provider` from the command line, recorded by `main` before parsing.
fn flag_slot() -> &'static RwLock<Option<ProviderId>> {
    static SLOT: OnceLock<RwLock<Option<ProviderId>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub fn set_provider_flag(provider: Option<ProviderId>) {
    *flag_slot().write().unwrap_or_else(PoisonError::into_inner) = provider;
}

pub(crate) fn provider_flag() -> Option<ProviderId> {
    *flag_slot().read().unwrap_or_else(PoisonError::into_inner)
}

fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.trim().is_empty())
}

fn is_loopback_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|parsed| {
        matches!(
            parsed.host_str(),
            Some("127.0.0.1" | "localhost" | "::1" | "[::1]")
        )
    })
}

/// The settings key `"provider"`, read straight from the discovered settings
/// files (later files win, as in the merged config). Reading the raw JSON
/// instead of the validated config means a broken unrelated key can never
/// silently drop the choice, and a typo here is reported with its file.
/// Files that are missing or not JSON objects are left to the config loader
/// to report.
fn settings_provider() -> Result<Option<ProviderId>, String> {
    let Ok(cwd) = env::current_dir() else {
        return Ok(None);
    };
    provider_from_settings(provider_settings_files(
        &runtime::ConfigLoader::default_for(&cwd),
    ))
}

/// Settings files allowed to choose the provider: user settings always, and
/// workspace files only once the workspace is trusted, so a cloned repository
/// cannot pick which of the user's credentials receive the conversation.
fn provider_settings_files(loader: &runtime::ConfigLoader) -> Vec<(PathBuf, String)> {
    let trusted = loader.workspace_is_trusted();
    loader
        .discover()
        .into_iter()
        .filter(|entry| {
            trusted || (entry.source == runtime::ConfigSource::User && entry.path.is_absolute())
        })
        .filter_map(|entry| {
            fs::read_to_string(&entry.path)
                .ok()
                .map(|text| (entry.path, text))
        })
        .collect()
}

fn provider_from_settings(
    files: impl IntoIterator<Item = (PathBuf, String)>,
) -> Result<Option<ProviderId>, String> {
    let mut last = None;
    for (path, text) in files {
        let Ok(Value::Object(mut object)) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        match object.remove("provider") {
            None | Some(Value::Null) => {}
            Some(Value::String(value)) => last = Some((path, value)),
            Some(_) => {
                return Err(format!(
                    "settings \"provider\" in {} must be a string, one of {}",
                    path.display(),
                    ProviderId::names()
                ))
            }
        }
    }
    match last {
        Some((path, value)) if !value.trim().is_empty() => parse_provider_id(
            &value,
            &format!("settings \"provider\" in {}", path.display()),
        )
        .map(Some),
        _ => Ok(None),
    }
}

/// Snapshot of the environment, settings and stored login.
fn gather_inputs(requested_model: &str) -> Result<ProviderInputs, Box<dyn std::error::Error>> {
    let neuron_token = crate::auth::env_token()?.is_some();
    let env_choice = non_empty_env("NEURON_PROVIDER")
        .map(|value| parse_provider_id(&value, "NEURON_PROVIDER"))
        .transpose()?;
    let flag = provider_flag();
    // Settings only matter when no higher-precedence choice was made.
    let settings_choice = if neuron_token || flag.is_some() || env_choice.is_some() {
        None
    } else {
        settings_provider()?
    };
    let explicit = flag.or(env_choice).or(settings_choice);
    // Only consult (and migrate) the stored login when it can matter.
    let stored_login =
        !neuron_token && explicit.is_none() && crate::auth::stored_credential_for_use().is_some();
    Ok(ProviderInputs {
        neuron_token,
        flag,
        env_choice,
        settings_choice,
        stored_login,
        anthropic_auth: crate::api_client::resolve_cli_auth_source().is_ok(),
        xai_key: non_empty_env("XAI_API_KEY").is_some(),
        azure: non_empty_env("AZURE_OPENAI_API_KEY").is_some()
            && non_empty_env("AZURE_OPENAI_ENDPOINT").is_some(),
        openai_key: non_empty_env("OPENAI_API_KEY").is_some(),
        local_base_url: non_empty_env("OPENAI_BASE_URL").is_some_and(|url| is_loopback_url(&url)),
        openrouter_key: non_empty_env("OPENROUTER_API_KEY").is_some(),
        requested_model: requested_model.to_string(),
    })
}

/// Model id used on the OpenAI-compatible transport (`openai/` prefix
/// selects that client; it is stripped on the wire).
pub fn openai_model(model: &str) -> String {
    if model.starts_with("openai/") {
        model.to_string()
    } else {
        format!("openai/{model}")
    }
}

/// The model as people should see it: without the `openai/` routing prefix of
/// [`openai_model`], which picks a client and is not part of its name (the
/// gateway's catalog ids have none).
pub fn shown_model(model: &str) -> &str {
    model.strip_prefix("openai/").unwrap_or(model)
}

fn requested_or(requested: &str, fallback: impl FnOnce() -> String) -> String {
    if requested == AUTO_MODEL {
        fallback()
    } else {
        requested.to_string()
    }
}

fn require_env(key: &'static str, provider: ProviderId) -> Result<String, String> {
    non_empty_env(key).ok_or_else(|| {
        format!(
            "{} needs {key}; export it or choose another provider with --provider",
            provider.display_name()
        )
    })
}

/// Selects the provider, prepares its credential, installs the endpoint
/// into the API client, prints the startup line and returns the model to
/// use.
pub fn configure_provider(requested_model: &str) -> Result<String, Box<dyn std::error::Error>> {
    let inputs = gather_inputs(requested_model)?;
    let selection = select_provider(&inputs, &azure_usable);
    let active = activate(selection, requested_model)?;
    Ok(install_active(active))
}

/// How [`start_provider`] went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStart {
    /// Configured, as [`configure_provider`] does; the model to use.
    Ready(String),
    /// Zero-X was selected but its login is missing, expired or rejected: a UI that can
    /// sign the user in starts without a provider and calls this again afterwards.
    SignInRequired(String),
}

/// [`configure_provider`] for a UI that can run the sign-in itself.
pub fn start_provider(requested_model: &str) -> Result<ProviderStart, Box<dyn std::error::Error>> {
    let inputs = gather_inputs(requested_model)?;
    let selection = select_provider(&inputs, &azure_usable);
    let zero_x = selection.provider == ProviderId::ZeroX;
    match activate(selection, requested_model) {
        Ok(active) => Ok(ProviderStart::Ready(install_active(active))),
        Err(error) if zero_x => Ok(ProviderStart::SignInRequired(error.to_string())),
        Err(error) => Err(error),
    }
}

/// Makes `active` the process's provider, prints its startup line (not in JSON mode,
/// whose stderr carries JSON only) and returns its model.
fn install_active(active: ActiveProvider) -> String {
    if !JSON_OUTPUT.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!("{}", active.startup_line());
    }
    let model = active.model.clone();
    set_active_provider(active);
    model
}

fn activate(
    selection: Selection,
    requested_model: &str,
) -> Result<ActiveProvider, Box<dyn std::error::Error>> {
    let id = selection.provider;
    let env_model = non_empty_env("NEURON_MODEL");
    let mut active = ActiveProvider {
        id,
        reason: selection.reason,
        credential_source: String::new(),
        base_url: None,
        model: String::new(),
        plan: None,
    };
    let endpoint = match id {
        ProviderId::ZeroX => {
            let credential = crate::auth::credential_for_session()?;
            let base = crate::auth::gateway_base_url()?;
            // Not a secret: orchestration and output budgeting key off it.
            env::set_var("NEURON_API_BASE", &base);
            let credential = load_catalog_checked(&base, credential)?;
            active.credential_source = credential.source.describe();
            active.plan.clone_from(&credential.plan);
            active.model = openai_model(&env_model.unwrap_or_else(|| requested_model.to_string()));
            active.base_url = Some(base.clone());
            Some(OpenAiCompatEndpoint::new(
                OpenAiCompatConfig::zero_x(),
                base,
                credential.token.expose(),
            ))
        }
        ProviderId::OpenAi => {
            let key = require_env("OPENAI_API_KEY", id)?;
            let base = non_empty_env("OPENAI_BASE_URL")
                .unwrap_or_else(|| DEFAULT_OPENAI_BASE_URL.to_string());
            active.credential_source = "OPENAI_API_KEY".to_string();
            active.model = openai_model(&env_model.unwrap_or_else(|| requested_model.to_string()));
            active.base_url = Some(base.clone());
            Some(OpenAiCompatEndpoint::new(
                OpenAiCompatConfig::openai(),
                base,
                key,
            ))
        }
        ProviderId::Local => {
            let base = non_empty_env("OPENAI_BASE_URL")
                .unwrap_or_else(|| DEFAULT_LOCAL_BASE_URL.to_string());
            let key = non_empty_env("OPENAI_API_KEY").unwrap_or_default();
            active.credential_source = if key.is_empty() {
                "none".to_string()
            } else {
                "OPENAI_API_KEY".to_string()
            };
            active.model = openai_model(&env_model.unwrap_or_else(|| requested_model.to_string()));
            active.base_url = Some(base.clone());
            Some(OpenAiCompatEndpoint::new(
                OpenAiCompatConfig::openai(),
                base,
                key,
            ))
        }
        ProviderId::OpenRouter => {
            let key = require_env("OPENROUTER_API_KEY", id)?;
            active.credential_source = "OPENROUTER_API_KEY".to_string();
            active.model = openai_model(&requested_or(requested_model, || {
                non_empty_env("OPENROUTER_MODEL")
                    .unwrap_or_else(|| DEFAULT_OPENROUTER_MODEL.to_string())
            }));
            active.base_url = Some(OPENROUTER_BASE_URL.to_string());
            Some(OpenAiCompatEndpoint::new(
                OpenAiCompatConfig::openai(),
                OPENROUTER_BASE_URL,
                key,
            ))
        }
        ProviderId::Azure => {
            let key = require_env("AZURE_OPENAI_API_KEY", id)?;
            let base = require_env("AZURE_OPENAI_ENDPOINT", id)?;
            active.credential_source = "AZURE_OPENAI_API_KEY".to_string();
            active.model = openai_model(
                &non_empty_env("AZURE_OPENAI_MODEL")
                    .unwrap_or_else(|| DEFAULT_AZURE_MODEL.to_string()),
            );
            active.base_url = Some(base.clone());
            Some(azure_endpoint(base, key, |output_tokens| {
                crate::quota::QuotaState::load().record_azure_usage(output_tokens);
            }))
        }
        ProviderId::Anthropic => {
            if let Err(error) = crate::api_client::resolve_cli_auth_source() {
                return Err(format!(
                    "{} needs ANTHROPIC_API_KEY or ANTHROPIC_AUTH_TOKEN ({error}); export one or choose another provider with --provider",
                    id.display_name()
                )
                .into());
            }
            active.credential_source = if non_empty_env("ANTHROPIC_API_KEY").is_some() {
                "ANTHROPIC_API_KEY".to_string()
            } else {
                "ANTHROPIC_AUTH_TOKEN".to_string()
            };
            active.model = requested_or(requested_model, || DEFAULT_ANTHROPIC_MODEL.to_string());
            None
        }
        ProviderId::Xai => {
            require_env("XAI_API_KEY", id)?;
            active.credential_source = "XAI_API_KEY".to_string();
            active.model = requested_or(requested_model, || DEFAULT_XAI_MODEL.to_string());
            None
        }
    };
    api::install_openai_compat_endpoint(endpoint);
    Ok(active)
}

/// Loads the Zero-X catalog (once per process). An `invalid_session`
/// answer means the credential is dead: say so and, in a terminal, offer
/// to sign in again. Other failures only cost the catalog features.
fn load_catalog_checked(
    base: &str,
    credential: crate::auth::GatewayCredential,
) -> Result<crate::auth::GatewayCredential, Box<dyn std::error::Error>> {
    match crate::gateway_catalog::load(base, Some(credential.token.expose())) {
        Ok(_) => Ok(credential),
        Err(crate::gateway_catalog::CatalogError::InvalidSession(_)) => {
            if credential.source == crate::auth::CredentialSource::EnvToken {
                return Err(format!(
                    "NEURON_TOKEN was rejected: {}",
                    api::INVALID_SESSION_MESSAGE
                )
                .into());
            }
            let renewed = crate::auth::recover_invalid_session()?;
            if let Err(error) = crate::gateway_catalog::load(base, Some(renewed.token.expose())) {
                warn(&error.to_string());
            }
            Ok(renewed)
        }
        Err(error) => {
            warn(&format!(
                "{error}; model limits fall back to built-in defaults"
            ));
            Ok(credential)
        }
    }
}

/// `warning: <message>` on stderr, or in JSON mode the same as a JSON line.
fn warn(message: &str) {
    print_warning(&format!("warning: {message}"));
}

/// A whole `warning: ...` line on stderr, or in JSON mode as a JSON line.
pub(crate) fn print_warning(line: &str) {
    if JSON_OUTPUT.load(std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "{}",
            serde_json::json!({ "type": "warning", "warning": line })
        );
    } else {
        eprintln!("{line}");
    }
}

/// The Azure endpoint. Every completed request reports its output tokens to
/// `record`, which keeps the daily quota (`~/.neuroncli/quota.json`) behind
/// the banner and the Azure-to-fallback switch up to date without exporting
/// the endpoint into the environment.
fn azure_endpoint(
    base: String,
    key: String,
    record: impl Fn(u32) + Send + Sync + 'static,
) -> OpenAiCompatEndpoint {
    OpenAiCompatEndpoint::new(OpenAiCompatConfig::openai(), base, key).with_usage_observer(
        api::UsageObserver::new(move |usage| record(usage.output_tokens)),
    )
}

/// Ambient Azure is used only when it is configured, its daily quota is left and the
/// endpoint answers. Says why when a configured Azure is skipped.
fn azure_usable() -> bool {
    let (Some(key), Some(base)) = (
        non_empty_env("AZURE_OPENAI_API_KEY"),
        non_empty_env("AZURE_OPENAI_ENDPOINT"),
    ) else {
        return false;
    };
    let quota = crate::quota::QuotaState::load();
    if quota.is_azure_exhausted() {
        warn_skipping_azure(&format!(
            "Azure daily quota exhausted ({})",
            quota.display_compact()
        ));
        return false;
    }
    if azure_api_probe(&key, &base) {
        true
    } else {
        warn_skipping_azure("Azure unavailable");
        false
    }
}

/// `--output-format json`: stderr carries only the JSON error, so notices that are
/// not part of it are left out.
static JSON_OUTPUT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether this run prints JSON (`--output-format json`).
pub(crate) fn set_json_output(json: bool) {
    JSON_OUTPUT.store(json, std::sync::atomic::Ordering::Relaxed);
}

/// `⚠ <why> – not using Azure`, yellow only on a terminal; nothing in JSON mode.
fn warn_skipping_azure(why: &str) {
    use std::io::IsTerminal as _;
    if JSON_OUTPUT.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let sign = if std::io::stderr().is_terminal() {
        "\x1b[33m\u{26a0}\x1b[0m"
    } else {
        "\u{26a0}"
    };
    eprintln!("{sign} {why} \u{2013} not using Azure");
}

/// Quick probe to check if the Azure endpoint is reachable.
pub fn azure_api_probe(api_key: &str, base_url: &str) -> bool {
    let client = reqwest::blocking::Client::builder()
        .user_agent(api::USER_AGENT)
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let Ok(client) = client else {
        return false;
    };
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": "Kimi-K2.5",
        "messages": [{"role": "user", "content": "ping"}],
        "max_completion_tokens": 1
    });
    match client
        .post(&url)
        .header("content-type", "application/json")
        .header("api-key", api_key)
        .bearer_auth(api_key)
        .json(&body)
        .send()
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            status == 200 || status == 429
        }
        Err(_) => false,
    }
}

pub fn provider_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::Xai => "xai",
        ProviderKind::OpenAi => "openai",
    }
}

/// Short provider name for the banner and connected line.
pub fn active_provider_name(model: &str) -> String {
    active_provider().map_or_else(
        || provider_label(detect_provider_kind(model)).to_string(),
        |active| active.id.as_str().to_string(),
    )
}

pub fn format_connected_line(model: &str) -> String {
    let provider = active_provider_name(model);
    let model = shown_model(model);
    format!("{GREEN}{BOLD}\u{2713}{R} {DIM}Connected:{R} {BLUE}{BOLD}{model}{R} {DIM}via{R} {ORANGE}{provider}{R}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(model: &str) -> ProviderInputs {
        ProviderInputs {
            requested_model: model.to_string(),
            ..ProviderInputs::default()
        }
    }

    fn azure_up() -> bool {
        true
    }

    fn pick(inputs: &ProviderInputs) -> Selection {
        select_provider(inputs, &azure_up)
    }

    #[test]
    fn compatible_transport_prefix_preserves_vendor_model_ids() {
        assert_eq!(
            openai_model("qwen/qwen3-coder:free"),
            "openai/qwen/qwen3-coder:free"
        );
        assert_eq!(
            openai_model("claude-sonnet-4-6"),
            "openai/claude-sonnet-4-6"
        );
        assert_eq!(openai_model("openai/auto"), "openai/auto");
        assert_eq!(
            shown_model(&openai_model("groq/openai/gpt-oss-120b")),
            "groq/openai/gpt-oss-120b"
        );
        assert_eq!(shown_model("claude-sonnet-4-6"), "claude-sonnet-4-6");
    }

    #[test]
    fn neuron_token_beats_every_other_source() {
        let mut everything = inputs(AUTO_MODEL);
        everything.neuron_token = true;
        everything.flag = Some(ProviderId::OpenAi);
        everything.env_choice = Some(ProviderId::Anthropic);
        everything.settings_choice = Some(ProviderId::Local);
        everything.stored_login = true;
        everything.anthropic_auth = true;
        everything.openai_key = true;
        assert_eq!(
            pick(&everything),
            Selection {
                provider: ProviderId::ZeroX,
                reason: SelectionReason::NeuronToken
            }
        );
    }

    #[test]
    fn explicit_choice_order_is_flag_then_env_then_settings_and_beats_stored_login() {
        let mut state = inputs(AUTO_MODEL);
        state.stored_login = true;
        state.openai_key = true;
        state.settings_choice = Some(ProviderId::Local);
        assert_eq!(
            pick(&state),
            Selection {
                provider: ProviderId::Local,
                reason: SelectionReason::Explicit(ChoiceOrigin::Settings)
            }
        );
        state.env_choice = Some(ProviderId::OpenRouter);
        assert_eq!(pick(&state).provider, ProviderId::OpenRouter);
        assert_eq!(
            pick(&state).reason,
            SelectionReason::Explicit(ChoiceOrigin::Env)
        );
        state.flag = Some(ProviderId::OpenAi);
        assert_eq!(
            pick(&state),
            Selection {
                provider: ProviderId::OpenAi,
                reason: SelectionReason::Explicit(ChoiceOrigin::Flag)
            }
        );
    }

    #[test]
    fn stored_login_beats_ambient_provider_keys() {
        let mut state = inputs(AUTO_MODEL);
        state.stored_login = true;
        state.anthropic_auth = true;
        state.openai_key = true;
        state.openrouter_key = true;
        state.azure = true;
        assert_eq!(
            pick(&state),
            Selection {
                provider: ProviderId::ZeroX,
                reason: SelectionReason::StoredLogin
            }
        );
        // Even an Anthropic-looking model stays on the gateway.
        state.requested_model = "claude-sonnet-4-6".to_string();
        assert_eq!(pick(&state).provider, ProviderId::ZeroX);
    }

    #[test]
    fn ambient_keys_apply_only_without_a_login_in_their_order() {
        let mut state = inputs(AUTO_MODEL);
        state.anthropic_auth = true;
        state.openai_key = true;
        assert_eq!(pick(&state).provider, ProviderId::Anthropic);

        // A non-Claude model skips ambient Anthropic auth.
        state.requested_model = "gpt-4o-mini".to_string();
        assert_eq!(
            pick(&state),
            Selection {
                provider: ProviderId::OpenAi,
                reason: SelectionReason::Ambient("OPENAI_API_KEY")
            }
        );

        let mut grok = inputs("grok-3");
        grok.xai_key = true;
        grok.openai_key = true;
        assert_eq!(pick(&grok).provider, ProviderId::Xai);

        let mut azure = inputs(AUTO_MODEL);
        azure.azure = true;
        azure.openai_key = true;
        assert_eq!(pick(&azure).provider, ProviderId::Azure);
        assert_eq!(
            select_provider(&azure, &|| false).provider,
            ProviderId::OpenAi,
            "an unreachable Azure endpoint falls through"
        );

        let mut local = inputs(AUTO_MODEL);
        local.local_base_url = true;
        local.openrouter_key = true;
        assert_eq!(pick(&local).provider, ProviderId::Local);
        local.local_base_url = false;
        assert_eq!(pick(&local).provider, ProviderId::OpenRouter);
    }

    #[test]
    fn nothing_configured_means_zero_x_sign_in() {
        assert_eq!(
            pick(&inputs(AUTO_MODEL)),
            Selection {
                provider: ProviderId::ZeroX,
                reason: SelectionReason::SignInRequired
            }
        );
    }

    #[test]
    fn azure_probe_is_skipped_when_a_higher_source_wins() {
        let mut state = inputs(AUTO_MODEL);
        state.azure = true;
        state.stored_login = true;
        let probed = std::cell::Cell::new(false);
        let probe = || {
            probed.set(true);
            true
        };
        assert_eq!(select_provider(&state, &probe).provider, ProviderId::ZeroX);
        assert!(!probed.get());
    }

    #[test]
    fn provider_names_parse_with_aliases_and_reject_typos() {
        for id in ProviderId::ALL {
            assert_eq!(ProviderId::parse(id.as_str()), Some(id));
        }
        assert_eq!(ProviderId::parse("ZeroX"), Some(ProviderId::ZeroX));
        let error = parse_provider_id("opnai", "--provider").unwrap_err();
        assert!(error.contains("zero-x|openai|openrouter|azure|anthropic|xai|local"));
    }

    fn settings(files: &[(&str, &str)]) -> Result<Option<ProviderId>, String> {
        provider_from_settings(
            files
                .iter()
                .map(|(path, text)| (PathBuf::from(path), (*text).to_string())),
        )
    }

    #[test]
    fn untrusted_workspace_cannot_choose_the_provider() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neuron-provider-trust-{nanos}"));
        let cwd = root.join("repo");
        let home = root.join("home").join(".neuron");
        fs::create_dir_all(&cwd).expect("repo");
        fs::create_dir_all(&home).expect("home");
        fs::write(cwd.join(".neuron.json"), r#"{"provider":"openrouter"}"#).expect("repo settings");

        let untrusted = runtime::ConfigLoader::new(&cwd, &home)
            .with_project_trust(runtime::ProjectTrust::Untrusted);
        assert_eq!(
            provider_from_settings(provider_settings_files(&untrusted)),
            Ok(None)
        );

        fs::write(home.join("settings.json"), r#"{"provider":"openai"}"#).expect("user settings");
        assert_eq!(
            provider_from_settings(provider_settings_files(&untrusted)),
            Ok(Some(ProviderId::OpenAi))
        );

        let trusted = runtime::ConfigLoader::new(&cwd, &home)
            .with_project_trust(runtime::ProjectTrust::Trusted);
        assert_eq!(
            provider_from_settings(provider_settings_files(&trusted)),
            Ok(Some(ProviderId::OpenRouter))
        );

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn settings_provider_reads_the_last_file_that_sets_it() {
        assert_eq!(
            settings(&[
                ("user/settings.json", r#"{"provider":"openai","model":"x"}"#),
                ("project/.neuron/settings.json", r#"{"provider":"local"}"#),
                ("project/.neuron/settings.local.json", r#"{"hooks":{}}"#),
            ]),
            Ok(Some(ProviderId::Local))
        );
        // Unrelated keys, broken files and blank values do not matter.
        assert_eq!(
            settings(&[
                ("a.json", r#"{"defaultMcpServers":false,"unknownKey":1}"#),
                ("b.json", "{not json"),
                ("c.json", r#"{"provider":"  "}"#),
            ]),
            Ok(None)
        );
        // An overridden typo in a lower-precedence file is harmless.
        assert_eq!(
            settings(&[
                ("user.json", r#"{"provider":"opnai"}"#),
                ("project.json", r#"{"provider":"zero-x"}"#),
            ]),
            Ok(Some(ProviderId::ZeroX))
        );
    }

    #[test]
    fn settings_provider_typos_name_the_file() {
        let error = settings(&[("cfg/settings.json", r#"{"provider":"opnai"}"#)]).unwrap_err();
        assert!(error.contains("unknown provider 'opnai'"), "{error}");
        assert!(error.contains("cfg/settings.json"), "{error}");
        assert!(error.contains("zero-x|openai"), "{error}");
        let error = settings(&[("cfg/settings.json", r#"{"provider":7}"#)]).unwrap_err();
        assert!(error.contains("must be a string"), "{error}");
    }

    #[test]
    fn azure_endpoint_reports_output_tokens_for_the_daily_quota() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let recorded = Arc::new(AtomicU32::new(0));
        let sink = Arc::clone(&recorded);
        let endpoint = azure_endpoint(
            "https://example.openai.azure.com/openai/v1".to_string(),
            "azure-key".to_string(),
            move |tokens| {
                sink.fetch_add(tokens, Ordering::SeqCst);
            },
        );
        let observer = endpoint
            .usage_observer()
            .expect("Azure requests feed the quota");
        for output_tokens in [7, 5] {
            observer.notify(&api::Usage {
                input_tokens: 100,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 0,
                output_tokens,
            });
        }
        assert_eq!(recorded.load(Ordering::SeqCst), 12);
    }

    #[test]
    fn startup_line_names_provider_and_source_but_never_the_secret() {
        let active = ActiveProvider {
            id: ProviderId::OpenAi,
            reason: SelectionReason::Explicit(ChoiceOrigin::Flag),
            credential_source: "OPENAI_API_KEY".to_string(),
            base_url: Some("https://api.openai.com/v1".to_string()),
            model: "openai/gpt-4o-mini".to_string(),
            plan: None,
        };
        assert_eq!(
            active.startup_line(),
            "Provider: OpenAI · credential: OPENAI_API_KEY · selected by --provider · https://api.openai.com/v1"
        );
        let gateway = ActiveProvider {
            id: ProviderId::ZeroX,
            reason: SelectionReason::StoredLogin,
            credential_source: crate::auth::CredentialSource::Stored(
                crate::credentials::CredentialLocation::Keychain,
            )
            .describe(),
            base_url: Some("https://zero-x.live/v1".to_string()),
            model: "openai/auto".to_string(),
            plan: Some("pro".to_string()),
        };
        let line = gateway.startup_line();
        assert!(
            line.starts_with("Provider: Zero-X gateway · credential: stored login, OS keychain")
        );
        assert!(!line.contains("ses_"));
    }
}
