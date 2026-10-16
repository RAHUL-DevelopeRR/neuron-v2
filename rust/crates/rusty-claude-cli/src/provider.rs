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
use std::sync::{OnceLock, PoisonError, RwLock};

use crate::brand::*;
use api::{detect_provider_kind, OpenAiCompatConfig, OpenAiCompatEndpoint, ProviderKind};

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

    fn names() -> String {
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

fn provider_flag() -> Option<ProviderId> {
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

fn settings_provider() -> Result<Option<ProviderId>, String> {
    let Ok(cwd) = env::current_dir() else {
        return Ok(None);
    };
    let Ok(config) = runtime::ConfigLoader::default_for(&cwd).load() else {
        return Ok(None);
    };
    match config.get("provider").and_then(|value| value.as_str()) {
        Some(value) if !value.trim().is_empty() => {
            parse_provider_id(value, "settings \"provider\"").map(Some)
        }
        _ => Ok(None),
    }
}

/// Snapshot of the environment, settings and stored login.
fn gather_inputs(requested_model: &str) -> Result<ProviderInputs, Box<dyn std::error::Error>> {
    let neuron_token = crate::auth::env_token()?.is_some();
    let env_choice = non_empty_env("NEURON_PROVIDER")
        .map(|value| parse_provider_id(&value, "NEURON_PROVIDER"))
        .transpose()?;
    let settings_choice = settings_provider()?;
    let flag = provider_flag();
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
    eprintln!("{}", active.startup_line());
    let model = active.model.clone();
    set_active_provider(active);
    Ok(model)
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
            Some(OpenAiCompatEndpoint::new(
                OpenAiCompatConfig::openai(),
                base,
                key,
            ))
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
                eprintln!("warning: {error}");
            }
            Ok(renewed)
        }
        Err(error) => {
            eprintln!("warning: {error}; model limits fall back to built-in defaults");
            Ok(credential)
        }
    }
}

/// Ambient Azure is used only when its daily quota is left and the endpoint
/// answers.
fn azure_usable() -> bool {
    let quota = crate::quota::QuotaState::load();
    if quota.is_azure_exhausted() {
        eprintln!(
            "\x1b[33m\u{26a0}\x1b[0m Azure daily quota exhausted ({}) \u{2013} using fallback",
            quota.display_compact()
        );
        return false;
    }
    let (Some(key), Some(base)) = (
        non_empty_env("AZURE_OPENAI_API_KEY"),
        non_empty_env("AZURE_OPENAI_ENDPOINT"),
    ) else {
        return false;
    };
    if azure_api_probe(&key, &base) {
        true
    } else {
        eprintln!("\x1b[33m\u{26a0}\x1b[0m Azure unavailable \u{2013} using fallback");
        false
    }
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
