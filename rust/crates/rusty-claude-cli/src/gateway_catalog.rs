//! Zero-X model catalog (`GET <base>/models`, contract §3.2).
//!
//! Fetched at most once per process, cached on disk for ten minutes in
//! `~/.neuroncli/catalog.json`, and registered into
//! `runtime::model_registry` so context management and request preflight
//! know each gateway model's window, output budget and tool support.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::Duration;

use api::GatewayError;
use runtime::model_registry::{self, ModelCapability};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::gateway_http::{self, string_field, u64_field, GatewayHttpError};

/// How long the on-disk catalog stays fresh.
pub const CACHE_TTL: Duration = Duration::from_secs(600);
/// The catalog is fetched on startup, so keep the wait short.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const CATALOG_FILE: &str = "catalog.json";
/// Version 2 keys the cache by [`credential_key`].
const CACHE_VERSION: u64 = 2;

/// One `data[]` entry of `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogModel {
    pub id: String,
    pub display_name: Option<String>,
    pub context_window: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub supports_tools: bool,
    pub tiers: Vec<String>,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    /// How the gateway sees it now: `available`, `degraded`, `busy` or `blurred`
    /// (greyed out until `available_again_at`).
    pub status: Option<String>,
    /// When a `blurred` model can be used again (ISO 8601).
    pub available_again_at: Option<String>,
}

impl CatalogModel {
    fn from_value(value: &Value) -> Option<Self> {
        let id = string_field(value, "id")?;
        let u32_of =
            |key: &str| u64_field(value, key).and_then(|number| u32::try_from(number).ok());
        let capabilities = value.get("capabilities");
        Some(Self {
            display_name: string_field(value, "display_name"),
            context_window: u32_of("context_window"),
            max_output_tokens: u32_of("max_output_tokens"),
            supports_tools: capabilities
                .and_then(|capabilities| capabilities.get("tools"))
                .and_then(Value::as_bool)
                .or_else(|| value.get("supports_tools").and_then(Value::as_bool))
                .unwrap_or(false),
            tiers: value
                .get("tiers")
                .and_then(Value::as_array)
                .map(|tiers| {
                    tiers
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            available: value
                .get("available")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            unavailable_reason: string_field(value, "unavailable_reason"),
            status: string_field(value, "status"),
            available_again_at: string_field(value, "available_again_at"),
            id,
        })
    }

    /// Registry entry; models without a context window are left to the
    /// built-in fallback table.
    pub fn capability(&self) -> Option<ModelCapability> {
        let context_window = self.context_window.filter(|window| *window > 0)?;
        let max_output = self
            .max_output_tokens
            .filter(|tokens| *tokens > 0)
            .unwrap_or(model_registry::DEFAULT_MAX_OUTPUT_TOKENS)
            .min(context_window);
        Some(ModelCapability::new(
            self.id.clone(),
            context_window,
            max_output,
            self.supports_tools,
        ))
    }

    fn tier_label(&self) -> String {
        if self.tiers.is_empty() {
            "-".to_string()
        } else {
            self.tiers.join("+")
        }
    }
}

/// A parsed catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub base_url: String,
    /// [`credential_key`] of the caller it was fetched for: `available` is
    /// computed per caller plan (contract §3.2).
    pub credential_key: String,
    pub fetched_at: u64,
    pub models: Vec<CatalogModel>,
}

/// Identifies whose catalog this is without storing the credential: a
/// truncated, domain-separated SHA-256 of the token, or `anonymous`.
pub fn credential_key(token: Option<&str>) -> String {
    token.map_or_else(
        || "anonymous".to_string(),
        |token| {
            let digest = Sha256::new()
                .chain_update(b"neuron-catalog-cache\0")
                .chain_update(token.as_bytes())
                .finalize();
            hex::encode(&digest[..8])
        },
    )
}

impl Catalog {
    /// Finds a model by id, ignoring case and the `openai/` transport prefix.
    pub fn find(&self, model: &str) -> Option<&CatalogModel> {
        let wanted = model_registry::normalize_model_id(model);
        self.models
            .iter()
            .find(|candidate| model_registry::normalize_model_id(&candidate.id) == wanted)
    }

    /// Available models first, keeping the gateway's order within each group.
    pub fn sorted(&self) -> Vec<&CatalogModel> {
        let mut models = self.models.iter().collect::<Vec<_>>();
        models.sort_by_key(|model| !model.available);
        models
    }
}

/// Parses the `GET /v1/models` body.
pub fn parse_catalog(value: &Value) -> Result<Vec<CatalogModel>, String> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "catalog response has no data[] array".to_string())?;
    Ok(data.iter().filter_map(CatalogModel::from_value).collect())
}

/// Registers every catalog entry that carries limits.
pub fn register(models: &[CatalogModel]) {
    model_registry::register(models.iter().filter_map(CatalogModel::capability));
}

#[derive(Debug)]
pub enum CatalogError {
    /// The gateway rejected the credential.
    InvalidSession(GatewayError),
    /// Network failure or bad response and no cached copy.
    Unavailable(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSession(error) => write!(f, "{error}"),
            Self::Unavailable(message) => write!(f, "model catalog unavailable: {message}"),
        }
    }
}

impl std::error::Error for CatalogError {}

fn loaded_slot() -> &'static RwLock<Option<Arc<Catalog>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<Catalog>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// The catalog loaded by this process, if any.
pub fn current() -> Option<Arc<Catalog>> {
    loaded_slot()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn set_current(catalog: Option<Arc<Catalog>>) {
    *loaded_slot()
        .write()
        .unwrap_or_else(PoisonError::into_inner) = catalog;
}

fn cache_path() -> PathBuf {
    crate::credentials::neuron_home().join(CATALOG_FILE)
}

/// Forgets the in-memory and on-disk catalog (plan changes after login,
/// logout or upgrade).
pub fn invalidate_cache() {
    set_current(None);
    let _ = fs::remove_file(cache_path());
}

/// Loads the catalog once per process: memory, then a fresh disk cache,
/// then the network, then a stale disk cache. Registers it into the model
/// registry. A 401 is reported as [`CatalogError::InvalidSession`].
pub fn load(base_url: &str, token: Option<&str>) -> Result<Arc<Catalog>, CatalogError> {
    let key = credential_key(token);
    if let Some(catalog) =
        current().filter(|catalog| catalog.base_url == base_url && catalog.credential_key == key)
    {
        return Ok(catalog);
    }
    let now = crate::clock::now_unix();
    let path = cache_path();
    let cached = read_cache(&path, base_url, &key);
    let catalog = match cached {
        Some(catalog) if now.saturating_sub(catalog.fetched_at) < CACHE_TTL.as_secs() => catalog,
        cached => match fetch(base_url, token, now) {
            Ok((catalog, raw)) => {
                write_cache(&path, &catalog, &raw);
                catalog
            }
            Err(GatewayHttpError::InvalidSession(error)) => {
                return Err(CatalogError::InvalidSession(error));
            }
            Err(error) => match cached {
                Some(stale) => stale,
                None => return Err(CatalogError::Unavailable(error.to_string())),
            },
        },
    };
    register(&catalog.models);
    let catalog = Arc::new(catalog);
    set_current(Some(Arc::clone(&catalog)));
    Ok(catalog)
}

fn fetch(
    base_url: &str,
    token: Option<&str>,
    now: u64,
) -> Result<(Catalog, Value), GatewayHttpError> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let value = gateway_http::get_json(&url, token, FETCH_TIMEOUT)?;
    let models = parse_catalog(&value).map_err(GatewayHttpError::InvalidResponse)?;
    let raw = value.get("data").cloned().unwrap_or_else(|| json!([]));
    Ok((
        Catalog {
            base_url: base_url.to_string(),
            credential_key: credential_key(token),
            fetched_at: now,
            models,
        },
        raw,
    ))
}

/// The cached catalog, only when it was fetched from `base_url` for the
/// caller identified by `key`.
fn read_cache(path: &Path, base_url: &str, key: &str) -> Option<Catalog> {
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
    if u64_field(&value, "version") != Some(CACHE_VERSION)
        || value.get("base_url").and_then(Value::as_str) != Some(base_url)
        || value.get("credential").and_then(Value::as_str) != Some(key)
    {
        return None;
    }
    Some(Catalog {
        base_url: base_url.to_string(),
        credential_key: key.to_string(),
        fetched_at: u64_field(&value, "fetched_at")?,
        models: parse_catalog(&value).ok()?,
    })
}

fn write_cache(path: &Path, catalog: &Catalog, raw: &Value) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let document = json!({
        "version": CACHE_VERSION,
        "base_url": catalog.base_url,
        "credential": catalog.credential_key,
        "fetched_at": catalog.fetched_at,
        "data": raw,
    });
    if let Ok(text) = serde_json::to_string(&document) {
        let _ = fs::write(path, text);
    }
}

/// `1048576` -> `1M`, `131072` -> `128K`, `24000` -> `24K`.
pub fn format_window(tokens: u32) -> String {
    if tokens >= 1 << 20 && tokens.is_multiple_of(1 << 20) {
        format!("{}M", tokens >> 20)
    } else if tokens >= 1 << 10 && tokens.is_multiple_of(1 << 10) {
        format!("{}K", tokens >> 10)
    } else if tokens >= 1_000_000 {
        format!("{:.1}M", f64::from(tokens) / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// `/model` listing: available models first, with tier, window, tools and
/// the reason a model is unavailable.
pub fn render_model_list(catalog: &Catalog, current_model: &str) -> String {
    let current = model_registry::normalize_model_id(current_model);
    let width = catalog
        .models
        .iter()
        .map(|model| model.id.len())
        .max()
        .unwrap_or(0)
        .clamp(8, 48);
    let mut lines = vec![format!(
        "Zero-X models ({} in catalog, fetched {})",
        catalog.models.len(),
        crate::clock::describe_relative(catalog.fetched_at, crate::clock::now_unix())
    )];
    let mut printed_unavailable_heading = false;
    for model in catalog.sorted() {
        if !model.available && !printed_unavailable_heading {
            lines.push("  Not available on your plan".to_string());
            printed_unavailable_heading = true;
        }
        let marker = if model_registry::normalize_model_id(&model.id) == current {
            '*'
        } else {
            ' '
        };
        let mut line = format!(
            "  {marker} {:<width$}  {:<9}  {:>5}",
            model.id,
            model.tier_label(),
            model
                .context_window
                .map_or_else(|| "-".to_string(), format_window),
        );
        if model.supports_tools {
            line.push_str("  tools");
        }
        if let Some(name) = &model.display_name {
            line.push_str(&format!("  {name}"));
        }
        if !model.available {
            line.push_str(&format!(
                "  [{}]",
                model.unavailable_reason.as_deref().unwrap_or("unavailable")
            ));
        }
        lines.push(line.trim_end().to_string());
    }
    lines.push(format!(
        "Switch with /model <id>. Plans: {}",
        crate::account::pricing_url()
    ));
    lines.join("\n")
}

/// Outcome of checking a `/model <id>` choice against the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelCheck {
    Available,
    Unavailable { reason: String },
    Unknown { suggestions: Vec<String> },
}

pub fn check_model(catalog: &Catalog, model: &str) -> ModelCheck {
    match catalog.find(model) {
        Some(found) if found.available => ModelCheck::Available,
        Some(found) => ModelCheck::Unavailable {
            reason: found
                .unavailable_reason
                .clone()
                .unwrap_or_else(|| "unavailable".to_string()),
        },
        None => {
            let wanted = model_registry::normalize_model_id(model);
            let leaf = wanted.rsplit('/').next().unwrap_or(&wanted).to_string();
            let suggestions = catalog
                .sorted()
                .into_iter()
                .filter(|candidate| candidate.available)
                .map(|candidate| candidate.id.clone())
                .filter(|id| {
                    let id = id.to_ascii_lowercase();
                    id.contains(&leaf) || leaf.contains(id.rsplit('/').next().unwrap_or(&id))
                })
                .take(3)
                .collect();
            ModelCheck::Unknown { suggestions }
        }
    }
}

/// Warning to print for a `/model <id>` choice, if any.
pub fn model_choice_warning(catalog: &Catalog, model: &str) -> Option<String> {
    let check = check_model(catalog, model);
    let model = crate::provider::shown_model(model);
    match check {
        ModelCheck::Available => None,
        ModelCheck::Unavailable { reason } => Some(match reason.as_str() {
            // Not a plan matter: this gateway has no provider set up for it.
            "not_configured" => format!(
                "warning: {model} is not configured on this Zero-X gateway; requests will fail. Pick another with /model."
            ),
            "region_unavailable" => format!(
                "warning: {model} is not available in your region; requests will fail. Pick another with /model."
            ),
            _ => format!(
                "warning: {model} is not available on your Zero-X plan ({reason}); requests will fail until you upgrade: {}",
                crate::account::pricing_url()
            ),
        }),
        ModelCheck::Unknown { suggestions } => {
            let mut message = format!(
                "warning: {model} is not in the Zero-X catalog; requests may fail. Run /model to list models."
            );
            if !suggestions.is_empty() {
                message.push_str(&format!(" Did you mean {}?", suggestions.join(", ")));
            }
            Some(message)
        }
    }
}

/// When the session runs on Zero-X, replaces the Anthropic alias
/// completions (`/model opus` ...) with catalog ids.
pub fn adjust_model_completions(candidates: Vec<String>) -> Vec<String> {
    let on_gateway = crate::provider::active_provider()
        .is_some_and(|active| active.id == crate::provider::ProviderId::ZeroX);
    match (on_gateway, current()) {
        (true, Some(catalog)) => with_catalog_completions(candidates, &catalog),
        _ => candidates,
    }
}

fn with_catalog_completions(candidates: Vec<String>, catalog: &Catalog) -> Vec<String> {
    let mut adjusted = candidates
        .into_iter()
        .filter(|candidate| match candidate.strip_prefix("/model ") {
            Some(argument) => {
                let argument = argument.trim();
                argument.is_empty() || catalog.find(argument).is_some()
            }
            None => true,
        })
        .collect::<Vec<_>>();
    for model in catalog.sorted() {
        let completion = format!("/model {}", model.id);
        if !adjusted.contains(&completion) {
            adjusted.push(completion);
        }
    }
    adjusted.sort();
    adjusted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_value() -> Value {
        json!({
            "object": "list",
            "data": [
                {"id": "auto", "object": "model", "created": 0, "owned_by": "zero-x", "display_name": "Auto", "context_window": null, "max_output_tokens": null, "capabilities": {"tools": true, "streaming": true}, "tiers": ["free"], "available": true, "unavailable_reason": null, "pricing": null},
                {"id": "test-gw/gemini-unit-flash", "object": "model", "display_name": "Gemini Flash", "context_window": 1_048_576, "max_output_tokens": 65_536, "capabilities": {"tools": true, "streaming": true}, "tiers": ["free", "paid"], "available": true, "unavailable_reason": null},
                {"id": "test-gw/paid-only-unit", "object": "model", "display_name": "Paid Only", "context_window": 262_144, "max_output_tokens": 32_768, "capabilities": {"tools": false, "streaming": true}, "tiers": ["paid"], "available": false, "unavailable_reason": "upgrade_required", "pricing": {"input_per_mtok_usd": 0.3, "output_per_mtok_usd": 2.5}},
                {"object": "model", "display_name": "missing id is skipped"}
            ]
        })
    }

    fn sample_catalog() -> Catalog {
        Catalog {
            base_url: "http://127.0.0.1:1/v1".into(),
            credential_key: credential_key(None),
            fetched_at: crate::clock::now_unix(),
            models: parse_catalog(&sample_value()).unwrap(),
        }
    }

    #[test]
    fn credential_keys_tell_callers_apart_without_revealing_the_token() {
        let token = "ses_012345678901234567890123456789";
        let key = credential_key(Some(token));
        assert_eq!(key, credential_key(Some(token)), "stable across runs");
        assert_eq!(key.len(), 16);
        assert!(key.bytes().all(|byte| byte.is_ascii_hexdigit()), "{key}");
        assert_ne!(
            key,
            credential_key(Some("ses_another0123456789abcdefghijk"))
        );
        assert_eq!(credential_key(None), "anonymous");
    }

    #[test]
    fn disk_cache_is_only_reused_for_the_same_gateway_and_credential() {
        let dir =
            std::env::temp_dir().join(format!("neuron-catalog-cache-unit-{}", std::process::id()));
        let path = dir.join(CATALOG_FILE);
        let mine = credential_key(Some("ses_012345678901234567890123456789"));
        let catalog = Catalog {
            credential_key: mine.clone(),
            ..sample_catalog()
        };
        write_cache(&path, &catalog, &sample_value()["data"]);

        let cached = read_cache(&path, &catalog.base_url, &mine).expect("same caller");
        assert_eq!(cached.models, catalog.models);
        assert_eq!(cached.credential_key, mine);
        assert!(read_cache(&path, &catalog.base_url, &credential_key(None)).is_none());
        assert!(read_cache(
            &path,
            &catalog.base_url,
            &credential_key(Some("nzx_otherkey0123456789ab"))
        )
        .is_none());
        assert!(read_cache(&path, "https://other.example/v1", &mine).is_none());
        let stored = fs::read_to_string(&path).unwrap();
        assert!(!stored.contains("ses_"), "the token is never written");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn parses_catalog_entries_and_registers_their_capabilities() {
        let models = parse_catalog(&sample_value()).unwrap();
        assert_eq!(models.len(), 3);
        assert_eq!(models[1].context_window, Some(1_048_576));
        assert!(models[1].supports_tools);
        assert!(!models[2].available);
        assert_eq!(
            models[2].unavailable_reason.as_deref(),
            Some("upgrade_required")
        );
        assert!(models[0].capability().is_none(), "auto keeps its fallback");

        register(&models);
        let flash = model_registry::lookup("openai/test-gw/gemini-unit-flash").unwrap();
        assert_eq!(flash.context_window, 1_048_576);
        assert_eq!(flash.max_output_tokens, 65_536);
        assert!(flash.supports_tools);
        let paid = model_registry::lookup("test-gw/paid-only-unit").unwrap();
        assert!(!paid.supports_tools);
        // The api crate's preflight now sees gateway models too.
        assert_eq!(
            api::model_token_limit("openai/test-gw/paid-only-unit")
                .map(|limit| limit.context_window_tokens),
            Some(262_144)
        );
        assert!(parse_catalog(&json!({"error": "nope"})).is_err());
    }

    #[test]
    fn listing_puts_available_models_first_and_explains_unavailable_ones() {
        let listing = render_model_list(&sample_catalog(), "openai/test-gw/gemini-unit-flash");
        let flash = listing.find("test-gw/gemini-unit-flash").unwrap();
        let paid = listing.find("test-gw/paid-only-unit").unwrap();
        let heading = listing.find("Not available on your plan").unwrap();
        assert!(flash < heading && heading < paid, "{listing}");
        assert!(listing.contains("* test-gw/gemini-unit-flash"), "{listing}");
        assert!(listing.contains("free+paid"));
        assert!(listing.contains("1M"));
        assert!(listing.contains("256K"));
        assert!(listing.contains("[upgrade_required]"));
    }

    #[test]
    fn model_choices_are_validated_against_the_catalog() {
        let catalog = sample_catalog();
        assert_eq!(
            check_model(&catalog, "openai/test-gw/gemini-unit-flash"),
            ModelCheck::Available
        );
        assert_eq!(
            check_model(&catalog, "test-gw/paid-only-unit"),
            ModelCheck::Unavailable {
                reason: "upgrade_required".into()
            }
        );
        assert_eq!(
            check_model(&catalog, "gemini-unit-flash"),
            ModelCheck::Unknown {
                suggestions: vec!["test-gw/gemini-unit-flash".into()]
            }
        );
        let warning = model_choice_warning(&catalog, "test-gw/paid-only-unit").unwrap();
        assert!(warning.contains("upgrade_required"));
        assert!(warning.contains("https://"));
        assert!(model_choice_warning(&catalog, "auto").is_none());
        assert!(model_choice_warning(&catalog, "claude-opus-4-6")
            .unwrap()
            .contains("not in the Zero-X catalog"));
    }

    #[test]
    fn model_status_is_kept_and_unconfigured_models_are_not_a_plan_matter() {
        let models = parse_catalog(&serde_json::json!({"data": [
            {"id": "gemini/gemini-2.5-flash", "available": true, "status": "blurred",
             "available_again_at": "2026-10-22T06:59:59.693Z"},
            {"id": "openai/gpt-5", "available": false, "unavailable_reason": "not_configured"},
        ]}))
        .unwrap();
        assert_eq!(models[0].status.as_deref(), Some("blurred"));
        assert_eq!(
            models[0].available_again_at.as_deref(),
            Some("2026-10-22T06:59:59.693Z")
        );
        let catalog = Catalog {
            models,
            ..sample_catalog()
        };
        let warning = model_choice_warning(&catalog, "openai/openai/gpt-5").unwrap();
        assert!(
            warning.contains("not configured on this Zero-X gateway"),
            "{warning}"
        );
        assert!(!warning.contains("upgrade"), "{warning}");
        assert!(
            warning.starts_with("warning: openai/gpt-5 "),
            "no routing prefix: {warning}"
        );
    }

    #[test]
    fn gateway_completions_replace_anthropic_aliases_with_catalog_ids() {
        let candidates = vec![
            "/model ".to_string(),
            "/model opus".to_string(),
            "/model sonnet".to_string(),
            "/model auto".to_string(),
            "/status".to_string(),
        ];
        let adjusted = with_catalog_completions(candidates, &sample_catalog());
        assert!(!adjusted.contains(&"/model opus".to_string()));
        assert!(!adjusted.contains(&"/model sonnet".to_string()));
        assert!(adjusted.contains(&"/model ".to_string()));
        assert!(adjusted.contains(&"/model auto".to_string()));
        assert!(adjusted.contains(&"/model test-gw/gemini-unit-flash".to_string()));
        assert!(adjusted.contains(&"/status".to_string()));
    }

    #[test]
    fn context_windows_are_formatted_compactly() {
        assert_eq!(format_window(1_048_576), "1M");
        assert_eq!(format_window(131_072), "128K");
        assert_eq!(format_window(24_000), "24K");
        assert_eq!(format_window(1_500_000), "1.5M");
        assert_eq!(format_window(512), "512");
    }
}
