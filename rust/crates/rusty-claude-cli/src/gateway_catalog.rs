//! Zero-X model catalog (`GET <base>/models`, contract §3.2).
//!
//! Fetched at most once per process, cached on disk for ten minutes in
//! `~/.neuroncli/catalog.json`, and registered into
//! `runtime::model_registry` so context management and request preflight
//! know each gateway model's window, output budget and tool support. Model
//! status (busy, blurred ...) changes faster: model lists lay the gateway's
//! `GET <base>/models/status` over the catalog (see [`LiveStatus`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use api::GatewayError;
use runtime::model_registry::{self, ModelCapability};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::gateway_http::{self, string_field, u64_field, Conditional, GatewayHttpError};

/// How long the on-disk catalog stays fresh.
pub const CACHE_TTL: Duration = Duration::from_secs(600);
/// The catalog is fetched on startup, so keep the wait short.
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
/// How often model status is asked for again; the gateway suggests 30 s.
pub const STATUS_TTL: Duration = Duration::from_secs(30);
/// Status is fetched while the model list waits for it.
const STATUS_TIMEOUT: Duration = Duration::from_secs(3);
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
    /// The model reads images (`capabilities.vision`); `None` when the gateway does not say.
    pub supports_images: Option<bool>,
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
            supports_images: capabilities
                .and_then(|capabilities| capabilities.get("vision"))
                .and_then(Value::as_bool)
                .or_else(|| value.get("supports_vision").and_then(Value::as_bool)),
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

    /// When a blurred model is back (Unix seconds), if the gateway said and it parses.
    fn back_at(&self) -> Option<u64> {
        self.available_again_at
            .as_deref()
            .and_then(crate::clock::parse_timestamp)
    }

    /// Taken out of rotation (`blurred`) as of `now_unix`: until `available_again_at`,
    /// or without one until a later status says otherwise.
    fn is_blurred(&self, now_unix: u64) -> bool {
        self.status.as_deref() == Some("blurred")
            && self.back_at().is_none_or(|back| back > now_unix)
    }

    /// Can be used now: `available` (on the plan and configured, whatever `status`
    /// says) and not blurred.
    fn is_usable(&self, now_unix: u64) -> bool {
        self.available && !self.is_blurred(now_unix)
    }

    /// `busy` or `degraded`: usable, but worth a mark.
    fn health_marker(&self) -> Option<&str> {
        self.status
            .as_deref()
            .filter(|status| matches!(*status, "busy" | "degraded"))
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

/// Whether the catalog says `model` reads images; `None` without a catalog entry that says.
pub fn model_supports_images(model: &str) -> Option<bool> {
    current().and_then(|catalog| {
        catalog
            .find(crate::provider::shown_model(model))
            .and_then(|entry| entry.supports_images)
    })
}

/// How the system prompt names `model`: the catalog's display name with the id, or the
/// id alone. `None` for `auto`, which picks a model per request, so the prompt names
/// none rather than a wrong one.
pub fn prompt_model_name(model: &str) -> Option<String> {
    let id = crate::provider::shown_model(model).trim();
    if id.is_empty() || id.eq_ignore_ascii_case(crate::provider::AUTO_MODEL) {
        return None;
    }
    let display = current().and_then(|catalog| {
        catalog
            .find(id)
            .and_then(|entry| entry.display_name.clone())
    });
    Some(match display {
        Some(name) if !name.trim().is_empty() && !name.trim().eq_ignore_ascii_case(id) => {
            format!("{} ({id})", name.trim())
        }
        _ => id.to_string(),
    })
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
    *live_status_slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = LiveStatus::default();
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

/// One `data[]` entry of `GET /v1/models/status`: the part of a catalog entry
/// that changes at runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub id: String,
    pub available: Option<bool>,
    pub unavailable_reason: Option<String>,
    pub status: Option<String>,
    pub available_again_at: Option<String>,
}

/// Parses the `GET /v1/models/status` body.
pub fn parse_status(value: &Value) -> Result<Vec<ModelStatus>, String> {
    let data = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "status response has no data[] array".to_string())?;
    Ok(data
        .iter()
        .filter_map(|entry| {
            Some(ModelStatus {
                id: string_field(entry, "id")?,
                available: entry.get("available").and_then(Value::as_bool),
                unavailable_reason: string_field(entry, "unavailable_reason"),
                status: string_field(entry, "status"),
                available_again_at: string_field(entry, "available_again_at"),
            })
        })
        .collect())
}

/// The last `GET /v1/models/status` answer for one gateway and caller.
#[derive(Debug, Clone)]
struct StatusSnapshot {
    base_url: String,
    credential_key: String,
    checked: Instant,
    etag: Option<String>,
    models: Vec<ModelStatus>,
}

/// Model status between model lists: asked for again at most every
/// [`STATUS_TTL`], with the last `ETag` so an unchanged list costs a 304.
#[derive(Debug, Default)]
pub struct LiveStatus {
    snapshot: Option<StatusSnapshot>,
}

impl LiveStatus {
    /// `catalog` as of `now`: the latest status laid over it, and a busy or
    /// blurred model whose `available_again_at` has passed shown available.
    /// A failed fetch keeps the status known before (the catalog's own, at
    /// first).
    pub fn apply(
        &mut self,
        catalog: &Catalog,
        token: Option<&str>,
        now: Instant,
        now_unix: u64,
    ) -> Catalog {
        let key = credential_key(token);
        let known = self.snapshot.take().filter(|snapshot| {
            snapshot.base_url == catalog.base_url && snapshot.credential_key == key
        });
        let snapshot = match known {
            Some(snapshot) if now.saturating_duration_since(snapshot.checked) < STATUS_TTL => {
                snapshot
            }
            known => fetch_status(known, &catalog.base_url, key, token, now),
        };
        let merged = merge_status(catalog, &snapshot.models, now_unix);
        self.snapshot = Some(snapshot);
        merged
    }
}

fn fetch_status(
    known: Option<StatusSnapshot>,
    base_url: &str,
    credential_key: String,
    token: Option<&str>,
    now: Instant,
) -> StatusSnapshot {
    let url = format!("{}/models/status", base_url.trim_end_matches('/'));
    let sent_etag = known.as_ref().and_then(|snapshot| snapshot.etag.as_deref());
    let fresh = match gateway_http::get_json_if_changed(&url, token, sent_etag, STATUS_TIMEOUT) {
        Ok(Conditional::Changed { value, etag }) => {
            parse_status(&value).ok().map(|models| (etag, models))
        }
        Ok(Conditional::NotModified) | Err(_) => None,
    };
    let (etag, models) = fresh.unwrap_or_else(|| {
        known.map_or((None, Vec::new()), |snapshot| {
            (snapshot.etag, snapshot.models)
        })
    });
    StatusSnapshot {
        base_url: base_url.to_string(),
        credential_key,
        checked: now,
        etag,
        models,
    }
}

/// `catalog` with `statuses` laid over its entries.
fn merge_status(catalog: &Catalog, statuses: &[ModelStatus], now_unix: u64) -> Catalog {
    let mut merged = catalog.clone();
    for model in &mut merged.models {
        if let Some(live) = statuses.iter().find(|live| live.id == model.id) {
            if let Some(available) = live.available {
                model.available = available;
                model
                    .unavailable_reason
                    .clone_from(&live.unavailable_reason);
            }
            if live.status.is_some() {
                model.status.clone_from(&live.status);
                model
                    .available_again_at
                    .clone_from(&live.available_again_at);
            }
        }
        let back = model
            .available_again_at
            .as_deref()
            .and_then(crate::clock::parse_timestamp);
        if back.is_some_and(|back| back <= now_unix) {
            model.status = Some("available".to_string());
            model.available_again_at = None;
        }
    }
    merged
}

fn live_status_slot() -> &'static Mutex<LiveStatus> {
    static SLOT: OnceLock<Mutex<LiveStatus>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(LiveStatus::default()))
}

/// `catalog` with the gateway's current model status ([`LiveStatus::apply`]),
/// asked for with the credential of the session's gateway endpoint.
pub fn with_session_status(catalog: &Catalog) -> Catalog {
    let endpoint = api::installed_openai_compat_endpoint();
    live_status_slot()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .apply(
            catalog,
            endpoint.as_ref().map(api::OpenAiCompatEndpoint::api_key),
            Instant::now(),
            crate::clock::now_unix(),
        )
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

/// The gateway's `unavailable_reason` codes in words, as the full-screen UI's model
/// picker says them; other reasons as sent.
fn reason_in_words(reason: Option<&str>) -> &str {
    match reason {
        Some("upgrade_required") => "needs a plan upgrade",
        Some("region_unavailable") => "not available in your region",
        Some("not_configured") => "not configured on this gateway",
        Some(other) => other,
        None => "unavailable",
    }
}

/// `back in 13h 48m`.
fn back_in(back: u64, now_unix: u64) -> String {
    format!(
        "back in {}",
        api::describe_wait(back.saturating_sub(now_unix))
    )
}

/// `07:00 UTC` within a day of `now_unix`, with the date further out.
fn describe_until(back: u64, now_unix: u64) -> String {
    let at = crate::clock::format_utc(back);
    match at.split_once(' ') {
        Some((_, time)) if back.saturating_sub(now_unix) < 86_400 => time.to_string(),
        _ => at,
    }
}

/// `/model` listing of `catalog` as of `now_unix` (lay the live status over it
/// first, [`with_session_status`]), with tier, window and tools: the models usable
/// now, busy or degraded ones marked; then those out of rotation (blurred) with
/// when they are back; then those the caller cannot use, with the reason.
pub fn render_model_list(catalog: &Catalog, current_model: &str, now_unix: u64) -> String {
    let current = model_registry::normalize_model_id(current_model);
    let width = catalog
        .models
        .iter()
        .map(|model| model.id.len())
        .max()
        .unwrap_or(0)
        .clamp(8, 48);
    let render = |model: &CatalogModel, note: Option<&str>| {
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
        if let Some(note) = note {
            line.push_str(&format!("  [{note}]"));
        }
        line.trim_end().to_string()
    };
    let mut lines = vec![format!(
        "Zero-X models ({} in catalog, fetched {})",
        catalog.models.len(),
        crate::clock::describe_relative(catalog.fetched_at, now_unix)
    )];
    for model in &catalog.models {
        if model.is_usable(now_unix) {
            lines.push(render(model, model.health_marker()));
        }
    }
    let blurred = catalog
        .models
        .iter()
        .filter(|model| model.available && model.is_blurred(now_unix))
        .collect::<Vec<_>>();
    if !blurred.is_empty() {
        lines.push("  Temporarily unavailable".to_string());
        for model in blurred {
            let back = model.back_at().map_or_else(
                || "out of capacity for now".to_string(),
                |back| back_in(back, now_unix),
            );
            lines.push(render(model, Some(&back)));
        }
    }
    // `available: false` wins over a `status` of `available`.
    let unavailable = catalog
        .models
        .iter()
        .filter(|model| !model.available)
        .collect::<Vec<_>>();
    if !unavailable.is_empty() {
        lines.push("  Not available".to_string());
        for model in unavailable {
            let reason = reason_in_words(model.unavailable_reason.as_deref());
            lines.push(render(model, Some(reason)));
        }
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
    Unavailable {
        reason: String,
    },
    /// Out of capacity: out of rotation until `back_at` (Unix seconds), or without
    /// one until the gateway says otherwise.
    Blurred {
        back_at: Option<u64>,
    },
    Unknown {
        suggestions: Vec<String>,
    },
}

/// Checks `model` against `catalog` as of `now_unix`.
pub fn check_model(catalog: &Catalog, model: &str, now_unix: u64) -> ModelCheck {
    match catalog.find(model) {
        Some(found) if !found.available => ModelCheck::Unavailable {
            reason: found
                .unavailable_reason
                .clone()
                .unwrap_or_else(|| "unavailable".to_string()),
        },
        Some(found) if found.is_blurred(now_unix) => ModelCheck::Blurred {
            back_at: found.back_at(),
        },
        Some(_) => ModelCheck::Available,
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

/// Warning to print for choosing `model` with `/model <id>`, or with `--model` in a
/// `neuron prompt` run (`one_shot`), if any. Lay the live status over `catalog`
/// first ([`with_session_status`]) so that a blurred model is caught.
pub fn model_choice_warning(
    catalog: &Catalog,
    model: &str,
    now_unix: u64,
    one_shot: bool,
) -> Option<String> {
    let check = check_model(catalog, model, now_unix);
    let model = crate::provider::shown_model(model);
    // A `neuron prompt` run has no REPL to type `/model` into.
    let pick = if one_shot { "--model" } else { "/model" };
    match check {
        ModelCheck::Available => None,
        ModelCheck::Blurred { back_at } => Some(match back_at {
            Some(back) => format!(
                "warning: {model} is out of capacity until {} ({}); requests to it fail until then. auto routes around it: {pick} auto.",
                describe_until(back, now_unix),
                back_in(back, now_unix)
            ),
            None => format!(
                "warning: {model} is out of capacity for now; requests to it fail. auto routes around it: {pick} auto."
            ),
        }),
        ModelCheck::Unavailable { reason } => Some(match reason.as_str() {
            // Not a plan matter: this gateway has no provider set up for it.
            "not_configured" => format!(
                "warning: {model} is not configured on this Zero-X gateway; requests will fail. Pick another with {pick}."
            ),
            "region_unavailable" => format!(
                "warning: {model} is not available in your region; requests will fail. Pick another with {pick}."
            ),
            _ => format!(
                "warning: {model} is not available on your Zero-X plan ({reason}); requests will fail until you upgrade: {}",
                crate::account::pricing_url()
            ),
        }),
        ModelCheck::Unknown { suggestions } => {
            let list = if one_shot {
                "Start neuron and type /model to list models."
            } else {
                "Run /model to list models."
            };
            let mut message =
                format!("warning: {model} is not in the Zero-X catalog; requests may fail. {list}");
            if !suggestions.is_empty() {
                message.push_str(&format!(" Did you mean {}?", suggestions.join(", ")));
            }
            Some(message)
        }
    }
}

/// Up to three models to name instead of `model`, which is out of capacity: those
/// usable now as `catalog` has it (lay the live status over it first), healthy ones
/// before busy or degraded ones, in the gateway's order. `auto`, which routes around
/// models out of capacity, when there are none or no catalog.
pub fn alternatives(catalog: Option<&Catalog>, model: &str, now_unix: u64) -> Vec<String> {
    let failed = model_registry::normalize_model_id(model);
    let auto = crate::provider::AUTO_MODEL;
    let mut usable = catalog
        .map(|catalog| {
            catalog
                .models
                .iter()
                .filter(|candidate| {
                    let id = model_registry::normalize_model_id(&candidate.id);
                    candidate.is_usable(now_unix) && id != failed && id != auto
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    usable.sort_by_key(|candidate| candidate.health_marker().is_some());
    let mut ids = usable
        .into_iter()
        .take(3)
        .map(|candidate| candidate.id.clone())
        .collect::<Vec<_>>();
    if ids.is_empty() && failed != auto {
        ids.push(auto.to_string());
    }
    ids
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
                {"id": "auto", "object": "model", "created": 0, "owned_by": "zero-x", "display_name": "Auto", "context_window": null, "max_output_tokens": null, "capabilities": {"tools": true, "streaming": true, "vision": false}, "tiers": ["free"], "available": true, "unavailable_reason": null, "pricing": null},
                {"id": "test-gw/gemini-unit-flash", "object": "model", "display_name": "Gemini Flash", "context_window": 1_048_576, "max_output_tokens": 65_536, "capabilities": {"tools": true, "streaming": true, "vision": true}, "tiers": ["free", "paid"], "available": true, "unavailable_reason": null},
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
        assert_eq!(
            models[0].supports_images,
            Some(false),
            "capabilities.vision"
        );
        assert_eq!(models[1].supports_images, Some(true), "capabilities.vision");
        assert_eq!(models[2].supports_images, None, "not said");

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
        let now = crate::clock::now_unix();
        let listing = render_model_list(&sample_catalog(), "openai/test-gw/gemini-unit-flash", now);
        let flash = listing.find("test-gw/gemini-unit-flash").unwrap();
        let paid = listing.find("test-gw/paid-only-unit").unwrap();
        let heading = listing.find("Not available").unwrap();
        assert!(flash < heading && heading < paid, "{listing}");
        assert!(listing.contains("* test-gw/gemini-unit-flash"), "{listing}");
        assert!(listing.contains("free+paid"));
        assert!(listing.contains("1M"));
        assert!(listing.contains("256K"));
        assert!(listing.contains("[needs a plan upgrade]"), "{listing}");
        assert!(!listing.contains("Temporarily unavailable"), "{listing}");
    }

    /// 2026-10-26 17:12 UTC, 13h 48m before the blurred model in [`status_catalog`]
    /// is back.
    fn status_now() -> u64 {
        crate::clock::parse_timestamp("2026-10-26T17:12:00Z").unwrap()
    }

    /// A catalog with the gateway's live status laid over it: busy, degraded and
    /// blurred models, and one the gateway reports `available` although it is not
    /// configured (`available: false`).
    fn status_catalog() -> Catalog {
        let models = parse_catalog(&json!({"data": [
            {"id": "auto", "available": true, "tiers": ["free"]},
            {"id": "gemini/gemini-2.5-flash", "available": true, "tiers": ["free"],
             "status": "blurred", "available_again_at": "2026-10-27T07:00:00.000Z"},
            {"id": "test-gw/busy-unit", "available": true, "tiers": ["free"], "status": "busy"},
            {"id": "test-gw/degraded-unit", "available": true, "tiers": ["free"], "status": "degraded"},
            {"id": "gemini/gemini-2.5-pro", "available": false,
             "unavailable_reason": "not_configured", "status": "available"},
            {"id": "test-gw/paid-only-unit", "available": false, "tiers": ["paid"],
             "unavailable_reason": "upgrade_required"},
            {"id": "test-gw/healthy-unit", "available": true, "tiers": ["free"], "status": "available"},
        ]}))
        .unwrap();
        Catalog {
            fetched_at: status_now(),
            models,
            ..sample_catalog()
        }
    }

    fn line_of<'a>(listing: &'a str, id: &str) -> &'a str {
        listing
            .lines()
            .find(|line| line.split_whitespace().any(|word| word == id))
            .unwrap_or_else(|| panic!("no line for {id}:\n{listing}"))
    }

    #[test]
    fn listing_keeps_blurred_models_apart_until_they_are_back() {
        let listing = render_model_list(&status_catalog(), "auto", status_now());
        let position = |text: &str| {
            listing
                .find(text)
                .unwrap_or_else(|| panic!("no {text}:\n{listing}"))
        };
        let temporarily = position("  Temporarily unavailable");
        let not_available = position("  Not available");
        assert!(position("test-gw/healthy-unit") < temporarily, "{listing}");
        assert!(position("test-gw/busy-unit") < temporarily, "{listing}");
        let flash = position("gemini/gemini-2.5-flash");
        assert!(
            temporarily < flash && flash < not_available,
            "blurred models are not usable: {listing}"
        );
        assert!(
            line_of(&listing, "gemini/gemini-2.5-flash").ends_with("[back in 13h 48m]"),
            "{listing}"
        );
        assert!(line_of(&listing, "test-gw/busy-unit").ends_with("[busy]"));
        assert!(line_of(&listing, "test-gw/degraded-unit").ends_with("[degraded]"));
        assert!(!line_of(&listing, "test-gw/healthy-unit").contains('['));

        // Once its time has passed, it is listed with the others again.
        let later = status_now() + 14 * 3_600;
        let listing = render_model_list(&status_catalog(), "auto", later);
        assert!(!listing.contains("Temporarily unavailable"), "{listing}");
        assert!(listing.find("gemini/gemini-2.5-flash") < listing.find("  Not available"));
    }

    #[test]
    fn unavailable_models_say_why_whatever_their_status_says() {
        let listing = render_model_list(&status_catalog(), "auto", status_now());
        assert!(
            !listing.contains("on your plan"),
            "not all a plan matter: {listing}"
        );
        let not_available = listing.find("  Not available").unwrap();
        let pro = listing.find("gemini/gemini-2.5-pro").unwrap();
        assert!(
            not_available < pro,
            "`available: false` wins over status `available`: {listing}"
        );
        assert!(
            line_of(&listing, "gemini/gemini-2.5-pro")
                .ends_with("[not configured on this gateway]"),
            "{listing}"
        );
        assert!(line_of(&listing, "test-gw/paid-only-unit").ends_with("[needs a plan upgrade]"));

        let catalog = status_catalog();
        assert_eq!(
            check_model(&catalog, "gemini/gemini-2.5-pro", status_now()),
            ModelCheck::Unavailable {
                reason: "not_configured".into()
            }
        );
        let warning =
            model_choice_warning(&catalog, "gemini/gemini-2.5-pro", status_now(), false).unwrap();
        assert!(warning.contains("not configured"), "{warning}");
    }

    #[test]
    fn choosing_a_blurred_model_says_until_when_and_what_routes_around_it() {
        let catalog = status_catalog();
        let back = crate::clock::parse_timestamp("2026-10-27T07:00:00Z").unwrap();
        assert_eq!(
            check_model(&catalog, "openai/gemini/gemini-2.5-flash", status_now()),
            ModelCheck::Blurred {
                back_at: Some(back)
            }
        );
        let warning = model_choice_warning(
            &catalog,
            "openai/gemini/gemini-2.5-flash",
            status_now(),
            false,
        )
        .unwrap();
        assert!(
            warning.starts_with(
                "warning: gemini/gemini-2.5-flash is out of capacity until 07:00 UTC (back in 13h 48m)"
            ),
            "{warning}"
        );
        assert!(warning.contains("auto routes around it"), "{warning}");
        assert!(warning.contains("/model auto"), "{warning}");
        let one_shot =
            model_choice_warning(&catalog, "gemini/gemini-2.5-flash", status_now(), true).unwrap();
        assert!(one_shot.contains("--model auto"), "{one_shot}");
        assert!(!one_shot.contains("/model"), "{one_shot}");

        // Busy and degraded models can be used; once back, so can the blurred one.
        assert!(model_choice_warning(&catalog, "test-gw/busy-unit", status_now(), false).is_none());
        assert_eq!(
            check_model(&catalog, "gemini/gemini-2.5-flash", back),
            ModelCheck::Available
        );
    }

    #[test]
    fn alternatives_are_models_usable_now_or_auto() {
        let catalog = status_catalog();
        assert_eq!(
            alternatives(
                Some(&catalog),
                "openai/gemini/gemini-2.5-flash",
                status_now()
            ),
            vec![
                "test-gw/healthy-unit".to_string(),
                "test-gw/busy-unit".to_string(),
                "test-gw/degraded-unit".to_string(),
            ],
            "healthy first; never blurred, unavailable, auto or the one that failed"
        );
        assert_eq!(
            alternatives(Some(&catalog), "test-gw/healthy-unit", status_now()),
            vec![
                "test-gw/busy-unit".to_string(),
                "test-gw/degraded-unit".to_string()
            ]
        );
        assert_eq!(
            alternatives(None, "gemini/gemini-2.5-flash", status_now()),
            vec!["auto".to_string()]
        );
        assert!(alternatives(None, "auto", status_now()).is_empty());
    }

    #[test]
    fn model_choices_are_validated_against_the_catalog() {
        let catalog = sample_catalog();
        let now = crate::clock::now_unix();
        assert_eq!(
            check_model(&catalog, "openai/test-gw/gemini-unit-flash", now),
            ModelCheck::Available
        );
        assert_eq!(
            check_model(&catalog, "test-gw/paid-only-unit", now),
            ModelCheck::Unavailable {
                reason: "upgrade_required".into()
            }
        );
        assert_eq!(
            check_model(&catalog, "gemini-unit-flash", now),
            ModelCheck::Unknown {
                suggestions: vec!["test-gw/gemini-unit-flash".into()]
            }
        );
        let warning = model_choice_warning(&catalog, "test-gw/paid-only-unit", now, false).unwrap();
        assert!(warning.contains("upgrade_required"));
        assert!(warning.contains("https://"));
        assert!(model_choice_warning(&catalog, "auto", now, false).is_none());
        assert!(
            model_choice_warning(&catalog, "claude-opus-4-6", now, false)
                .unwrap()
                .contains("not in the Zero-X catalog")
        );
        let one_shot = model_choice_warning(&catalog, "claude-opus-4-6", now, true).unwrap();
        assert!(!one_shot.contains("Run /model"), "{one_shot}");
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
        let warning = model_choice_warning(
            &catalog,
            "openai/openai/gpt-5",
            crate::clock::now_unix(),
            false,
        )
        .unwrap();
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

    /// Serves `answers` to one connection each (`(status line, extra headers, body)`)
    /// and records each request's `If-None-Match`.
    fn status_server(
        answers: Vec<(&'static str, &'static str, String)>,
    ) -> (String, Arc<Mutex<Vec<Option<String>>>>) {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                let mut reader = BufReader::new(stream);
                let mut etag = None;
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 && !line.trim().is_empty() {
                    if let Some((name, value)) = line.split_once(':') {
                        if name.eq_ignore_ascii_case("if-none-match") {
                            etag = Some(value.trim().to_string());
                        }
                    }
                    line.clear();
                }
                recorded.lock().unwrap().push(etag);
                let _ = write!(
                    reader.get_mut(),
                    "HTTP/1.1 {status}\r\n{headers}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base, seen)
    }

    #[test]
    fn model_lists_refresh_status_at_most_every_30_seconds() {
        let now_unix = crate::clock::now_unix();
        let past = "2026-01-01T00:00:00.000Z";
        let soon = "2999-01-01T00:00:00.000Z".to_string();
        let back_at = crate::clock::parse_timestamp(&soon).unwrap();
        let body = json!({"object": "list", "poll_interval_seconds": 30, "data": [
            {"id": "test-gw/gemini-unit-flash", "available": true, "status": "busy", "available_again_at": past},
            {"id": "test-gw/paid-only-unit", "available": false, "unavailable_reason": "upgrade_required",
             "status": "blurred", "available_again_at": soon},
        ]})
        .to_string();
        let (base, seen) = status_server(vec![
            ("200 OK", "etag: W/\"v1\"\r\n", body),
            ("304 Not Modified", "etag: W/\"v1\"\r\n", String::new()),
            ("500 Internal Server Error", "", "{}".to_string()),
        ]);
        let catalog = Catalog {
            base_url: base,
            ..sample_catalog()
        };
        let status_of = |catalog: &Catalog, id: &str| {
            let model = catalog.find(id).unwrap();
            (model.status.clone(), model.available_again_at.clone())
        };
        let mut live = LiveStatus::default();
        let start = Instant::now();

        let first = live.apply(&catalog, None, start, now_unix);
        // Busy until some time ago: available again.
        assert_eq!(
            status_of(&first, "test-gw/gemini-unit-flash"),
            (Some("available".into()), None)
        );
        assert_eq!(
            status_of(&first, "test-gw/paid-only-unit"),
            (Some("blurred".into()), Some(soon.clone()))
        );
        assert_eq!(
            status_of(&first, "auto"),
            (None, None),
            "not in the status list"
        );

        // Within 30 s: no request.
        live.apply(&catalog, None, start + Duration::from_secs(10), now_unix);
        assert_eq!(seen.lock().unwrap().len(), 1);

        // Later: asked again with the ETag; the 304 keeps the status.
        let later = live.apply(&catalog, None, start + Duration::from_secs(31), now_unix);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![None, Some("W/\"v1\"".to_string())]
        );
        assert_eq!(
            status_of(&later, "test-gw/paid-only-unit"),
            (Some("blurred".into()), Some(soon.clone()))
        );

        // A failed request keeps it too, and once its time has passed the model is back.
        let failed = live.apply(&catalog, None, start + Duration::from_secs(62), now_unix);
        assert_eq!(seen.lock().unwrap().len(), 3);
        assert_eq!(
            status_of(&failed, "test-gw/paid-only-unit").0.as_deref(),
            Some("blurred")
        );
        let back = live.apply(&catalog, None, start + Duration::from_secs(63), back_at);
        assert_eq!(
            status_of(&back, "test-gw/paid-only-unit"),
            (Some("available".into()), None)
        );
        assert!(!back.find("test-gw/paid-only-unit").unwrap().available);
    }

    #[test]
    fn a_gateway_without_model_status_leaves_the_catalog_as_it_is() {
        let (base, _seen) = status_server(vec![(
            "404 Not Found",
            "",
            r#"{"error":"nope"}"#.to_string(),
        )]);
        let catalog = Catalog {
            base_url: base,
            ..sample_catalog()
        };
        let mut live = LiveStatus::default();
        let shown = live.apply(&catalog, None, Instant::now(), crate::clock::now_unix());
        assert_eq!(shown, catalog);
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
