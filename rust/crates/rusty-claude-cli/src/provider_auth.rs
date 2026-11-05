//! Bring-your-own provider keys: `neuron auth set|list|remove <provider>`.
//!
//! Keys are kept in the OS keychain (service `neuron-cli`, account
//! `provider:<id>`) under the same policy as the Zero-X login
//! ([`crate::credentials::keychain_for_account`]); without a keychain they go
//! to an owner-only file `~/.neuroncli/provider-keys/<id>.key`.
//!
//! Lookup order for a provider's key (documented in `neuron auth help`):
//!   1. its environment variable (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`,
//!      `GEMINI_API_KEY`/`GOOGLE_API_KEY`, `OPENROUTER_API_KEY`, `XAI_API_KEY`),
//!   2. a trusted workspace's `.env`,
//!   3. the stored key.
//!
//! Stored keys reach the API client in memory ([`api::install_user_key`]),
//! never through the process environment, so tool subprocesses do not see
//! them. Keys are never printed: listings show the last four characters.
//!
//! Only API keys are accepted. Consumer-subscription sign-in (Claude.ai,
//! ChatGPT, Google account OAuth) is not offered; see the lane report's
//! "Provider sign-in policy".

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value;
use zeroize::Zeroizing;

use crate::credentials::{BackendError, FileBackend, SecretBackend};

const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// A provider whose API key the user can store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum KeyProvider {
    Anthropic,
    OpenAi,
    Gemini,
    OpenRouter,
    Xai,
}

impl KeyProvider {
    pub const ALL: [Self; 5] = [
        Self::Anthropic,
        Self::OpenAi,
        Self::Gemini,
        Self::OpenRouter,
        Self::Xai,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Self::Anthropic,
            "openai" => Self::OpenAi,
            "gemini" | "google" => Self::Gemini,
            "openrouter" => Self::OpenRouter,
            "xai" | "grok" => Self::Xai,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Gemini => "gemini",
            Self::OpenRouter => "openrouter",
            Self::Xai => "xai",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Gemini => "Google Gemini",
            Self::OpenRouter => "OpenRouter",
            Self::Xai => "xAI",
        }
    }

    /// Environment variables that supply the key, the canonical one first.
    pub const fn env_vars(self) -> &'static [&'static str] {
        match self {
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::OpenAi => &["OPENAI_API_KEY"],
            Self::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::OpenRouter => &["OPENROUTER_API_KEY"],
            Self::Xai => &["XAI_API_KEY"],
        }
    }

    pub const fn primary_env(self) -> &'static str {
        self.env_vars()[0]
    }

    fn account(self) -> String {
        format!("provider:{}", self.as_str())
    }

    /// Where to create a key, for error messages.
    const fn key_page(self) -> &'static str {
        match self {
            Self::Anthropic => "https://console.anthropic.com/settings/keys",
            Self::OpenAi => "https://platform.openai.com/api-keys",
            Self::Gemini => "https://aistudio.google.com/apikey",
            Self::OpenRouter => "https://openrouter.ai/settings/keys",
            Self::Xai => "https://console.x.ai",
        }
    }

    const fn default_base(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com/v1",
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Gemini => "https://generativelanguage.googleapis.com/v1beta",
            Self::OpenRouter => "https://openrouter.ai/api/v1",
            Self::Xai => "https://api.x.ai/v1",
        }
    }

    pub(crate) fn names() -> String {
        Self::ALL
            .iter()
            .map(|provider| provider.as_str())
            .collect::<Vec<_>>()
            .join("|")
    }
}

/// Checks a key's shape without contacting the provider.
pub fn validate_key(provider: KeyProvider, key: &str) -> Result<(), String> {
    let name = provider.display_name();
    if key.is_empty() {
        return Err(format!("no {name} key given"));
    }
    if !(16..=512).contains(&key.len())
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(format!(
            "that does not look like a {name} API key (create one at {})",
            provider.key_page()
        ));
    }
    let prefix = match provider {
        KeyProvider::Anthropic => {
            if key.starts_with("sk-ant-oat") {
                return Err(
                    "that is a subscription sign-in token, not an API key; Neuron accepts Anthropic API keys only (https://console.anthropic.com/settings/keys)"
                        .to_string(),
                );
            }
            "sk-ant-"
        }
        KeyProvider::OpenAi => "sk-",
        KeyProvider::OpenRouter => "sk-or-",
        KeyProvider::Xai => "xai-",
        // Google issues several key formats; length and characters are checked above.
        KeyProvider::Gemini => "",
    };
    if !key.starts_with(prefix) {
        return Err(format!(
            "{name} API keys start with `{prefix}` (create one at {})",
            provider.key_page()
        ));
    }
    if provider == KeyProvider::OpenAi && key.starts_with("sk-or-") {
        return Err("that is an OpenRouter key; use `neuron auth set openrouter`".to_string());
    }
    if provider == KeyProvider::OpenAi && key.starts_with("sk-ant-") {
        return Err("that is an Anthropic key; use `neuron auth set anthropic`".to_string());
    }
    Ok(())
}

/// `…abcd`: enough to tell keys apart, never enough to use one.
pub fn mask_key(key: &str) -> String {
    let chars = key.chars().collect::<Vec<_>>();
    if chars.len() < 12 {
        return "\u{2026}".to_string();
    }
    let tail = chars[chars.len() - 4..].iter().collect::<String>();
    format!("\u{2026}{tail}")
}

/// Where a provider key comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    Env(&'static str),
    Keychain,
    File,
}

impl KeySource {
    pub fn describe(&self) -> String {
        match self {
            Self::Env(name) => format!("env {name}"),
            Self::Keychain => "keychain".to_string(),
            Self::File => "file".to_string(),
        }
    }
}

/// One provider's key status. Never carries the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyStatus {
    pub provider: KeyProvider,
    pub source: Option<KeySource>,
    pub masked: Option<String>,
    /// A stored key exists but the environment overrides it.
    pub shadowed_store: bool,
}

type KeychainFactory = Box<dyn Fn(KeyProvider) -> Option<Box<dyn SecretBackend>> + Send + Sync>;

/// The stored provider keys: keychain per provider, file fallback.
pub struct ProviderKeyStore {
    keychain: KeychainFactory,
    dir: PathBuf,
}

impl ProviderKeyStore {
    pub fn open_default() -> Self {
        Self::new(
            &crate::credentials::neuron_home().join("provider-keys"),
            Box::new(|provider: KeyProvider| {
                crate::credentials::keychain_for_account(&provider.account())
            }),
        )
    }

    pub fn new(dir: &Path, keychain: KeychainFactory) -> Self {
        Self {
            keychain,
            dir: dir.to_path_buf(),
        }
    }

    fn file(&self, provider: KeyProvider) -> FileBackend {
        FileBackend::new(self.dir.join(format!("{}.key", provider.as_str())))
    }

    /// The stored key and where it is, or `None`.
    pub fn get(
        &self,
        provider: KeyProvider,
    ) -> Result<Option<(Zeroizing<String>, KeySource)>, String> {
        if let Some(keychain) = (self.keychain)(provider) {
            match keychain.read() {
                Ok(Some(key)) if !key.trim().is_empty() => {
                    return Ok(Some((
                        Zeroizing::new(key.trim().to_string()),
                        KeySource::Keychain,
                    )))
                }
                Ok(_) | Err(BackendError::Unavailable(_)) => {}
                Err(BackendError::Failed(error)) => {
                    return Err(format!("could not read the OS keychain: {error}"))
                }
            }
        }
        match self.file(provider).read() {
            Ok(Some(key)) if !key.trim().is_empty() => Ok(Some((
                Zeroizing::new(key.trim().to_string()),
                KeySource::File,
            ))),
            Ok(_) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    /// Stores `key`, preferring the keychain; removes any other copy.
    pub fn set(&self, provider: KeyProvider, key: &str) -> Result<KeySource, String> {
        validate_key(provider, key)?;
        let file = self.file(provider);
        if let Some(keychain) = (self.keychain)(provider) {
            match keychain.write(key) {
                Ok(()) => {
                    let _ = file.delete();
                    return Ok(KeySource::Keychain);
                }
                Err(error) => crate::credentials::warn_once(&format!(
                    "The OS keychain is unavailable ({error}); storing the {} key in a file instead.",
                    provider.display_name()
                )),
            }
        }
        file.write(key).map_err(|error| error.to_string())?;
        Ok(KeySource::File)
    }

    /// Removes the stored key everywhere. Returns whether one existed.
    pub fn remove(&self, provider: KeyProvider) -> Result<bool, String> {
        let existed = self.get(provider).ok().flatten().is_some();
        if let Some(keychain) = (self.keychain)(provider) {
            if let Err(BackendError::Failed(error)) = keychain.delete() {
                return Err(format!("could not remove the OS keychain entry: {error}"));
            }
        }
        self.file(provider)
            .delete()
            .map_err(|error| error.to_string())?;
        Ok(existed)
    }

    /// Every provider's status; `env` reads an environment variable.
    pub fn status(&self, env: &dyn Fn(&str) -> Option<String>) -> Vec<KeyStatus> {
        KeyProvider::ALL
            .iter()
            .map(|&provider| {
                let stored = self.get(provider).ok().flatten();
                let from_env = provider
                    .env_vars()
                    .iter()
                    .find_map(|name| env(name).map(|value| (*name, value)));
                match (from_env, stored) {
                    (Some((name, value)), stored) => KeyStatus {
                        provider,
                        source: Some(KeySource::Env(name)),
                        masked: Some(mask_key(value.trim())),
                        shadowed_store: stored.is_some(),
                    },
                    (None, Some((key, source))) => KeyStatus {
                        provider,
                        source: Some(source),
                        masked: Some(mask_key(&key)),
                        shadowed_store: false,
                    },
                    (None, None) => KeyStatus {
                        provider,
                        source: None,
                        masked: None,
                        shadowed_store: false,
                    },
                }
            })
            .collect()
    }

    /// Hands every stored key whose environment variables are all unset to the
    /// API client (in memory). Returns the providers it installed.
    pub fn install_into_api(&self, env: &dyn Fn(&str) -> Option<String>) -> Vec<KeyProvider> {
        let mut installed = Vec::new();
        for provider in KeyProvider::ALL {
            if provider.env_vars().iter().any(|name| env(name).is_some()) {
                continue;
            }
            match self.get(provider) {
                Ok(Some((key, _))) => {
                    api::install_user_key(provider.primary_env(), &key);
                    installed.push(provider);
                }
                Ok(None) => {}
                Err(error) => crate::credentials::warn_once(&format!(
                    "Could not read the stored {} key: {error}",
                    provider.display_name()
                )),
            }
        }
        installed
    }
}

fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// Loads stored keys into the API client once per process. Unit tests never
/// read the real keychain.
pub fn load_stored_keys() {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| {
        if cfg!(test) {
            return;
        }
        ProviderKeyStore::open_default().install_into_api(&non_empty_env);
    });
}

/// A provider's key for this process: environment first, then the stored key
/// installed by [`load_stored_keys`].
pub fn resolve_key(provider: KeyProvider) -> Option<(String, KeySource)> {
    for name in provider.env_vars() {
        if let Some(value) = non_empty_env(name) {
            return Some((value.trim().to_string(), KeySource::Env(name)));
        }
    }
    api::user_key(provider.primary_env()).map(|key| (key, KeySource::Keychain))
}

// ---------------------------------------------------------------------------
// Live checks and model lists
// ---------------------------------------------------------------------------

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(api::USER_AGENT)
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())
}

fn authorized(
    request: reqwest::blocking::RequestBuilder,
    provider: KeyProvider,
    key: &str,
) -> reqwest::blocking::RequestBuilder {
    match provider {
        KeyProvider::Anthropic => request
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01"),
        KeyProvider::Gemini => request.header("x-goog-api-key", key),
        KeyProvider::OpenAi | KeyProvider::OpenRouter | KeyProvider::Xai => {
            request.bearer_auth(key)
        }
    }
}

fn get_json(url: &str, provider: KeyProvider, key: &str) -> Result<Value, String> {
    let response = authorized(http_client()?.get(url), provider, key)
        .send()
        .map_err(|error| format!("could not reach {}: {error}", provider.display_name()))?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(format!(
            "{} rejected the key (HTTP {})",
            provider.display_name(),
            status.as_u16()
        ));
    }
    if !status.is_success() {
        return Err(format!(
            "{} answered HTTP {}",
            provider.display_name(),
            status.as_u16()
        ));
    }
    response.json::<Value>().map_err(|error| {
        format!(
            "unreadable answer from {}: {error}",
            provider.display_name()
        )
    })
}

/// A cheap authenticated call proving the key works: the models list, or for
/// OpenRouter (whose model list is public) `GET /key`.
pub fn check_key(provider: KeyProvider, key: &str, base: Option<&str>) -> Result<(), String> {
    let base = base
        .unwrap_or(provider.default_base())
        .trim_end_matches('/');
    let path = if provider == KeyProvider::OpenRouter {
        "/key"
    } else {
        "/models"
    };
    get_json(&format!("{base}{path}"), provider, key).map(|_| ())
}

/// The provider's model ids, sorted.
pub fn list_models(
    provider: KeyProvider,
    key: &str,
    base: Option<&str>,
) -> Result<Vec<String>, String> {
    let base = base
        .unwrap_or(provider.default_base())
        .trim_end_matches('/');
    let body = get_json(&format!("{base}/models"), provider, key)?;
    Ok(parse_model_ids(provider, &body))
}

fn parse_model_ids(provider: KeyProvider, body: &Value) -> Vec<String> {
    let mut ids = if provider == KeyProvider::Gemini {
        body.get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|model| {
                model
                    .get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_none_or(|methods| {
                        methods
                            .iter()
                            .any(|method| method.as_str() == Some("generateContent"))
                    })
            })
            .filter_map(|model| model.get("name").and_then(Value::as_str))
            .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
            .collect::<Vec<_>>()
    } else {
        body.get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    ids.sort();
    ids.dedup();
    ids
}

/// The [`KeyProvider`] behind an active provider, if it is a BYO-key one.
pub fn key_provider_for(id: crate::provider::ProviderId) -> Option<KeyProvider> {
    use crate::provider::ProviderId;
    match id {
        ProviderId::Anthropic => Some(KeyProvider::Anthropic),
        ProviderId::OpenAi => Some(KeyProvider::OpenAi),
        ProviderId::Gemini => Some(KeyProvider::Gemini),
        ProviderId::OpenRouter => Some(KeyProvider::OpenRouter),
        ProviderId::Xai => Some(KeyProvider::Xai),
        ProviderId::ZeroX | ProviderId::Azure | ProviderId::Local => None,
    }
}

/// The active BYO-key provider's models (fetched once per process).
pub fn active_provider_models() -> Option<(KeyProvider, Vec<String>)> {
    type ModelCache = std::sync::Mutex<Option<(KeyProvider, Vec<String>)>>;
    static CACHE: OnceLock<ModelCache> = OnceLock::new();
    let active = crate::provider::active_provider()?;
    let provider = key_provider_for(active.id)?;
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut cached = cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((cached_provider, models)) = cached.as_ref() {
        if *cached_provider == provider {
            return Some((provider, models.clone()));
        }
    }
    let (key, _) = resolve_key(provider)?;
    // OpenAI's base can be redirected (`OPENAI_BASE_URL`); the listing follows it.
    let base = (provider == KeyProvider::OpenAi)
        .then(|| active.base_url.clone())
        .flatten();
    match list_models(provider, &key, base.as_deref()) {
        Ok(models) => {
            *cached = Some((provider, models.clone()));
            Some((provider, models))
        }
        Err(error) => {
            runtime::ui_hooks::notice(&format!("warning: could not list models: {error}"));
            None
        }
    }
}

/// The models for the UI's model picker.
pub fn active_provider_model_infos() -> Vec<neuron_tui::protocol::ModelInfo> {
    let Some((provider, models)) = active_provider_models() else {
        return Vec::new();
    };
    models
        .into_iter()
        .map(|id| neuron_tui::protocol::ModelInfo {
            display_name: Some(format!("{id} ({})", provider.display_name())),
            id,
            available: true,
            unavailable_reason: None,
            context_window: None,
            tiers: Vec::new(),
            status: None,
            available_again_at: None,
            supports_images: None,
        })
        .collect()
}

/// Text listing for `/model` on a BYO-key provider.
pub fn active_provider_model_listing(current_model: &str) -> Option<String> {
    let (provider, models) = active_provider_models()?;
    Some(render_model_listing(provider, &models, current_model))
}

fn render_model_listing(provider: KeyProvider, models: &[String], current_model: &str) -> String {
    let current = crate::provider::shown_model(current_model);
    let mut text = format!(
        "{} models ({}, from your key):",
        provider.display_name(),
        models.len()
    );
    for model in models {
        let marker = if model == current { "*" } else { " " };
        text.push_str(&format!("\n {marker} {model}"));
    }
    text
}

// ---------------------------------------------------------------------------
// Reports
// ---------------------------------------------------------------------------

/// `neuron auth list` and `/providers`: provider, masked suffix, source.
pub fn render_status(statuses: &[KeyStatus], active: Option<&str>) -> String {
    let mut text = String::from("Provider keys (env vars take precedence over stored keys):");
    for status in statuses {
        let name = status.provider.display_name();
        let line = match (&status.source, &status.masked) {
            (Some(source), Some(masked)) => {
                let mut line = format!("  {name:<14} {masked:<6} {}", source.describe());
                if status.shadowed_store {
                    line.push_str(" (stored key overridden)");
                }
                line
            }
            _ => format!("  {name:<14} -      not set"),
        };
        text.push('\n');
        text.push_str(&line);
        if active == Some(status.provider.as_str()) {
            text.push_str("  [active]");
        }
    }
    text.push_str(
        "\nSet one with `neuron auth set <provider>`, or sign in to OpenRouter with `neuron auth login openrouter`.",
    );
    text
}

pub fn status_json(statuses: &[KeyStatus]) -> Value {
    Value::Array(
        statuses
            .iter()
            .map(|status| {
                serde_json::json!({
                    "provider": status.provider.as_str(),
                    "configured": status.source.is_some(),
                    "source": status.source.as_ref().map(KeySource::describe),
                    "suffix": status.masked,
                    "stored_key_overridden": status.shadowed_store,
                })
            })
            .collect(),
    )
}

/// The `/providers` report for the REPL and the full-screen UI.
pub fn providers_report() -> String {
    let store = ProviderKeyStore::open_default();
    let active = crate::provider::active_provider();
    let mut text = String::new();
    if let Some(active) = &active {
        text.push_str(&format!(
            "Active: {} ({})\n",
            active.id.display_name(),
            active.credential_source
        ));
    }
    let active_name = active
        .as_ref()
        .and_then(|active| key_provider_for(active.id))
        .map(KeyProvider::as_str);
    text.push_str(&render_status(&store.status(&non_empty_env), active_name));
    text
}

// ---------------------------------------------------------------------------
// `neuron auth set|list|remove|login openrouter`
// ---------------------------------------------------------------------------

pub const PROVIDER_AUTH_USAGE: &str = "Provider keys (bring your own account):\n\
  neuron auth set <provider> [--check]   store an API key (read from a hidden prompt or stdin)\n\
  neuron auth list                       show providers, masked keys and their source\n\
  neuron auth remove <provider>          delete a stored key\n\
  neuron auth login openrouter [--no-browser]  sign in to OpenRouter (OAuth PKCE) and store the key\n\
  providers: anthropic|openai|gemini|openrouter|xai\n\
  Order: environment variable, then a trusted workspace .env, then the stored key.";

/// Whether `words` (after `auth`) are a provider-key command.
pub fn is_provider_auth_command(words: &[&str]) -> bool {
    match words {
        ["list"] => true,
        ["set", provider, flags @ ..] => {
            KeyProvider::parse(provider).is_some() && flags.iter().all(|flag| *flag == "--check")
        }
        ["remove", provider] => KeyProvider::parse(provider).is_some(),
        ["login", "openrouter", flags @ ..] => flags.iter().all(|flag| *flag == "--no-browser"),
        _ => false,
    }
}

fn parse_provider(value: &str) -> Result<KeyProvider, String> {
    KeyProvider::parse(value).ok_or_else(|| {
        format!(
            "unknown provider '{value}'; expected one of {}",
            KeyProvider::names()
        )
    })
}

/// Runs a provider-key command (`args` after `auth`).
pub fn run(args: &[String], json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let store = ProviderKeyStore::open_default();
    match words.as_slice() {
        ["list"] => {
            let statuses = store.status(&non_empty_env);
            if json {
                crate::auth::print_json(&serde_json::json!({
                    "kind": "provider_keys",
                    "providers": status_json(&statuses),
                }))?;
            } else {
                println!("{}", render_status(&statuses, None));
            }
        }
        ["set", provider, flags @ ..] => {
            let provider = parse_provider(provider)?;
            let check = flags.contains(&"--check");
            let stdin = io::stdin();
            let key = if stdin.is_terminal() {
                read_hidden(&format!("{} API key: ", provider.display_name()))?
            } else {
                read_line(&mut stdin.lock())?
            };
            set_key(&store, provider, &key, check, json)?;
        }
        ["remove", provider] => {
            let provider = parse_provider(provider)?;
            let existed = store.remove(provider)?;
            if json {
                crate::auth::print_json(&serde_json::json!({
                    "kind": "provider_key_removed",
                    "provider": provider.as_str(),
                    "removed": existed,
                }))?;
            } else if existed {
                println!("Removed the stored {} key.", provider.display_name());
            } else {
                println!("No stored {} key.", provider.display_name());
            }
            if provider
                .env_vars()
                .iter()
                .any(|name| non_empty_env(name).is_some())
            {
                eprintln!(
                    "note: {} is still set in the environment and is used instead.",
                    provider.env_vars().join(" or ")
                );
            }
        }
        ["login", "openrouter", flags @ ..] => {
            let open_browser = !flags.contains(&"--no-browser");
            let key = crate::openrouter_oauth::login(open_browser)?;
            set_key(&store, KeyProvider::OpenRouter, &key, false, json)?;
        }
        _ => return Err(PROVIDER_AUTH_USAGE.into()),
    }
    Ok(())
}

/// Validates, optionally checks with a live call (`--check`), and stores `key`.
fn set_key(
    store: &ProviderKeyStore,
    provider: KeyProvider,
    key: &str,
    check: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let key = key.trim();
    validate_key(provider, key)?;
    if check {
        check_key(provider, key, None)?;
        eprintln!("{} accepted the key.", provider.display_name());
    }
    let source = store.set(provider, key)?;
    let masked = mask_key(key);
    if json {
        crate::auth::print_json(&serde_json::json!({
            "kind": "provider_key_stored",
            "provider": provider.as_str(),
            "suffix": masked,
            "source": source.describe(),
        }))?;
    } else {
        println!(
            "Stored the {} key {masked} in the {}.",
            provider.display_name(),
            match source {
                KeySource::Keychain => "OS keychain".to_string(),
                _ => "provider key file (owner-only)".to_string(),
            }
        );
    }
    if let Some(name) = provider
        .env_vars()
        .iter()
        .find(|name| non_empty_env(name).is_some())
    {
        eprintln!(
            "note: {name} is set in the environment and takes precedence over the stored key."
        );
    }
    Ok(())
}

/// One line from a pipe; the key never appears on the command line.
fn read_line(input: &mut dyn BufRead) -> io::Result<Zeroizing<String>> {
    let mut line = Zeroizing::new(String::new());
    input.read_line(&mut line)?;
    Ok(Zeroizing::new(line.trim().to_string()))
}

/// Reads a line from the terminal without echoing it.
fn read_hidden(prompt: &str) -> io::Result<Zeroizing<String>> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
    eprint!("{prompt}");
    io::stderr().flush()?;
    crossterm::terminal::enable_raw_mode()?;
    let mut buffer = Zeroizing::new(String::new());
    let result = loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(character) => buffer.push(character),
                _ => {}
            },
            Ok(Event::Paste(text)) => buffer.push_str(&text),
            Ok(_) => {}
            Err(error) => break Err(error),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    eprintln!();
    result.map(|()| Zeroizing::new(buffer.trim().to_string()))
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::credentials::tests::{temp_home, FakeKeychain};

    type Shared = Arc<Mutex<BTreeMap<KeyProvider, FakeKeychain>>>;

    fn store(dir: &Path, keychains: &Shared, unavailable: bool) -> ProviderKeyStore {
        let keychains = Arc::clone(keychains);
        ProviderKeyStore::new(
            dir,
            Box::new(move |provider| {
                let mut map = keychains.lock().unwrap();
                let fake = map.entry(provider).or_insert_with(|| FakeKeychain {
                    unavailable,
                    ..FakeKeychain::default()
                });
                Some(Box::new(fake.clone()) as Box<dyn SecretBackend>)
            }),
        )
    }

    const ANTHROPIC: &str = "sk-ant-api03-abcdefghijklmnop1234";
    const OPENROUTER: &str = "sk-or-v1-0123456789abcdefWXYZ";

    #[test]
    fn keys_are_validated_by_provider_shape() {
        assert!(validate_key(KeyProvider::Anthropic, ANTHROPIC).is_ok());
        assert!(
            validate_key(KeyProvider::Anthropic, "sk-ant-oat01-abcdefghijklmnop")
                .unwrap_err()
                .contains("API keys only")
        );
        assert!(validate_key(KeyProvider::OpenAi, "sk-proj-abcdefghijklmnopqrst").is_ok());
        assert!(validate_key(KeyProvider::OpenAi, OPENROUTER)
            .unwrap_err()
            .contains("OpenRouter"));
        assert!(validate_key(KeyProvider::OpenRouter, OPENROUTER).is_ok());
        assert!(validate_key(KeyProvider::Gemini, "AIzaSyA-abcdefghijklmnopqrstuvwxyz012").is_ok());
        assert!(validate_key(KeyProvider::Xai, "xai-abcdefghijklmnopqrstu").is_ok());
        assert!(validate_key(KeyProvider::Xai, "sk-abcdefghijklmnopqrstu").is_err());
        assert!(validate_key(KeyProvider::OpenAi, "sk-short").is_err());
        assert!(validate_key(KeyProvider::OpenAi, "sk-has space inside the key").is_err());
        assert!(validate_key(KeyProvider::OpenAi, "").is_err());
    }

    #[test]
    fn masks_show_only_the_last_four_characters() {
        assert_eq!(mask_key(ANTHROPIC), "\u{2026}1234");
        assert_eq!(mask_key("short"), "\u{2026}");
    }

    #[test]
    fn set_get_remove_round_trip_through_the_keychain() {
        let dir = temp_home("provider-keys");
        let keychains = Shared::default();
        let store = store(&dir, &keychains, false);
        assert_eq!(
            store.set(KeyProvider::Anthropic, ANTHROPIC),
            Ok(KeySource::Keychain)
        );
        let (key, source) = store.get(KeyProvider::Anthropic).unwrap().unwrap();
        assert_eq!(key.as_str(), ANTHROPIC);
        assert_eq!(source, KeySource::Keychain);
        assert!(!dir.join("anthropic.key").exists());
        assert!(store.get(KeyProvider::OpenAi).unwrap().is_none());
        assert_eq!(store.remove(KeyProvider::Anthropic), Ok(true));
        assert!(store.get(KeyProvider::Anthropic).unwrap().is_none());
        assert_eq!(store.remove(KeyProvider::Anthropic), Ok(false));
        assert!(store
            .set(KeyProvider::Anthropic, "not-a-key-at-all-x")
            .is_err());
    }

    #[test]
    fn unavailable_keychain_falls_back_to_a_file() {
        let dir = temp_home("provider-keys-file");
        let keychains = Shared::default();
        let store = store(&dir, &keychains, true);
        assert_eq!(
            store.set(KeyProvider::OpenRouter, OPENROUTER),
            Ok(KeySource::File)
        );
        assert!(dir.join("openrouter.key").exists());
        let (key, source) = store.get(KeyProvider::OpenRouter).unwrap().unwrap();
        assert_eq!((key.as_str(), source), (OPENROUTER, KeySource::File));
        assert_eq!(store.remove(KeyProvider::OpenRouter), Ok(true));
        assert!(!dir.join("openrouter.key").exists());
    }

    #[test]
    fn status_prefers_env_and_never_contains_the_key() {
        let dir = temp_home("provider-keys-status");
        let keychains = Shared::default();
        let store = store(&dir, &keychains, false);
        store.set(KeyProvider::Anthropic, ANTHROPIC).unwrap();
        store.set(KeyProvider::OpenRouter, OPENROUTER).unwrap();
        let env = |name: &str| {
            (name == "OPENROUTER_API_KEY").then(|| "sk-or-v1-envenvenvenv9876".to_string())
        };
        let statuses = store.status(&env);
        let anthropic = &statuses[0];
        assert_eq!(anthropic.source, Some(KeySource::Keychain));
        assert_eq!(anthropic.masked.as_deref(), Some("\u{2026}1234"));
        let openrouter = statuses
            .iter()
            .find(|status| status.provider == KeyProvider::OpenRouter)
            .unwrap();
        assert_eq!(
            openrouter.source,
            Some(KeySource::Env("OPENROUTER_API_KEY"))
        );
        assert!(openrouter.shadowed_store);
        let text = render_status(&statuses, Some("anthropic"));
        let json = status_json(&statuses).to_string();
        for secret in [ANTHROPIC, OPENROUTER, "sk-or-v1-envenvenvenv9876"] {
            assert!(!text.contains(secret), "{text}");
            assert!(!json.contains(secret), "{json}");
        }
        assert!(text.contains("[active]"));
        assert!(text.contains("not set"));
    }

    #[test]
    fn install_into_api_skips_providers_set_in_the_environment() {
        let dir = temp_home("provider-keys-install");
        let keychains = Shared::default();
        let store = store(&dir, &keychains, false);
        store
            .set(KeyProvider::Gemini, "AIzaSyA-abcdefghijklmnopqrstuvwxyz012")
            .unwrap();
        store
            .set(KeyProvider::Xai, "xai-abcdefghijklmnopqrstu")
            .unwrap();
        let env = |name: &str| (name == "GOOGLE_API_KEY").then(|| "AIza-env".to_string());
        let installed = store.install_into_api(&env);
        assert_eq!(installed, vec![KeyProvider::Xai]);
        assert_eq!(
            api::user_key("XAI_API_KEY").as_deref(),
            Some("xai-abcdefghijklmnopqrstu")
        );
        api::install_user_key("XAI_API_KEY", "");
    }

    #[test]
    fn provider_auth_commands_are_recognised() {
        assert!(is_provider_auth_command(&["list"]));
        assert!(is_provider_auth_command(&["set", "anthropic"]));
        assert!(is_provider_auth_command(&["set", "google", "--check"]));
        assert!(is_provider_auth_command(&["remove", "openai"]));
        assert!(is_provider_auth_command(&["login", "openrouter"]));
        assert!(is_provider_auth_command(&[
            "login",
            "openrouter",
            "--no-browser"
        ]));
        assert!(!is_provider_auth_command(&[
            "set",
            "anthropic",
            "sk-ant-key"
        ]));
        assert!(!is_provider_auth_command(&["set", "claude-ai"]));
        assert!(!is_provider_auth_command(&["login"]));
        assert!(!is_provider_auth_command(&["login", "chatgpt"]));
    }

    #[test]
    fn model_ids_parse_from_each_list_shape() {
        let openai = serde_json::json!({"data": [{"id": "gpt-b"}, {"id": "gpt-a"}]});
        assert_eq!(
            parse_model_ids(KeyProvider::OpenAi, &openai),
            vec!["gpt-a", "gpt-b"]
        );
        let gemini = serde_json::json!({"models": [
            {"name": "models/gemini-2.5-pro", "supportedGenerationMethods": ["generateContent"]},
            {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]}
        ]});
        assert_eq!(
            parse_model_ids(KeyProvider::Gemini, &gemini),
            vec!["gemini-2.5-pro"]
        );
        let listing = render_model_listing(
            KeyProvider::Anthropic,
            &["claude-a".to_string(), "claude-b".to_string()],
            "claude-b",
        );
        assert!(listing.starts_with("Anthropic models (2"));
        assert!(listing.contains(" * claude-b"));
    }

    /// One-shot HTTP server answering `status` with `body`; returns its base
    /// URL and the request it saw.
    pub(crate) fn fake_server(
        status: u16,
        body: &'static str,
    ) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                if read == 0 {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
            String::from_utf8_lossy(&request).to_string()
        });
        (format!("http://{address}"), handle)
    }

    #[test]
    fn live_check_uses_the_provider_auth_header_and_reports_rejection() {
        let (base, server) = fake_server(200, r#"{"data":[]}"#);
        check_key(KeyProvider::Anthropic, ANTHROPIC, Some(&base)).unwrap();
        let request = server.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /models"), "{request}");
        assert!(request.contains(&format!("x-api-key: {}", ANTHROPIC.to_ascii_lowercase())));
        assert!(request.contains("anthropic-version: 2023-06-01"));

        let (base, server) = fake_server(401, r#"{"error":"bad"}"#);
        let error = check_key(KeyProvider::OpenRouter, OPENROUTER, Some(&base)).unwrap_err();
        assert!(error.contains("rejected"), "{error}");
        assert!(!error.contains(OPENROUTER));
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /key"), "{request}");
        assert!(
            request.contains(&format!("Bearer {OPENROUTER}"))
                || request.to_ascii_lowercase().contains("bearer")
        );
    }
}
