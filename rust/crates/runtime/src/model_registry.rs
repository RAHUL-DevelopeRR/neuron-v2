//! Process-wide registry of model capabilities.
//!
//! The registry answers "how large is this model's context window, how many
//! output tokens may we request, and does it accept tool definitions?". It is
//! populated at startup from the Zero-X gateway catalog (`GET /v1/models`,
//! see `docs/GATEWAY_CONTRACT.md`) and falls back to a small built-in table of
//! models whose limits were verified against the upstream providers.
//!
//! Consumers (context management, request preflight, `max_tokens` selection)
//! must treat a `None` lookup as "unknown" and use conservative defaults
//! rather than assuming a large window.

use std::collections::HashMap;
use std::sync::{OnceLock, PoisonError, RwLock};

/// Context window assumed when nothing is known about a model.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 32_768;
/// Output budget assumed when nothing is known about a model.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8_192;

/// Capabilities of a single model as advertised by the gateway catalog or the
/// built-in fallback table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCapability {
    /// Canonical model id (gateway public id, e.g. `groq/openai/gpt-oss-120b`).
    pub id: String,
    /// Maximum input + output tokens the model accepts.
    pub context_window: u32,
    /// Maximum output tokens the model may generate per request.
    pub max_output_tokens: u32,
    /// Whether the model accepts OpenAI-style function tools.
    pub supports_tools: bool,
}

impl ModelCapability {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        context_window: u32,
        max_output_tokens: u32,
        supports_tools: bool,
    ) -> Self {
        Self {
            id: id.into(),
            context_window,
            max_output_tokens,
            supports_tools,
        }
    }
}

fn registry() -> &'static RwLock<HashMap<String, ModelCapability>> {
    static REGISTRY: OnceLock<RwLock<HashMap<String, ModelCapability>>> = OnceLock::new();
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Normalizes a model id for lookup: trims, lowercases and strips the
/// `openai/` transport prefix that OpenAI-compatible clients prepend.
#[must_use]
pub fn normalize_model_id(model: &str) -> String {
    let lowered = model.trim().to_ascii_lowercase();
    match lowered.strip_prefix("openai/") {
        // `openai/gpt-oss-*` on Groq is a real upstream id, so only strip the
        // prefix when what remains is itself a provider-qualified id.
        Some(rest) if rest.contains('/') || rest == "auto" || rest == "default" => rest.to_string(),
        _ => lowered,
    }
}

/// Registers (or replaces) capabilities, typically from the gateway catalog.
pub fn register(capabilities: impl IntoIterator<Item = ModelCapability>) {
    let mut map = registry().write().unwrap_or_else(PoisonError::into_inner);
    for capability in capabilities {
        map.insert(normalize_model_id(&capability.id), capability);
    }
}

/// Removes every registered capability. Intended for tests.
pub fn clear() {
    registry()
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// Looks a model up, preferring registered catalog data over the built-in
/// fallback table.
#[must_use]
pub fn lookup(model: &str) -> Option<ModelCapability> {
    let key = normalize_model_id(model);
    let registered = registry()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    registered.or_else(|| builtin_fallback(&key))
}

/// Context window for `model`, or [`DEFAULT_CONTEXT_WINDOW`] when unknown.
#[must_use]
pub fn context_window_or_default(model: &str) -> u32 {
    lookup(model).map_or(DEFAULT_CONTEXT_WINDOW, |cap| cap.context_window)
}

/// Output-token limit for `model`, or [`DEFAULT_MAX_OUTPUT_TOKENS`] when unknown.
#[must_use]
pub fn max_output_tokens_or_default(model: &str) -> u32 {
    lookup(model).map_or(DEFAULT_MAX_OUTPUT_TOKENS, |cap| cap.max_output_tokens)
}

/// Built-in limits for models verified against their providers on 2026-10-16.
/// Gateway catalog data always takes precedence over this table.
#[must_use]
pub fn builtin_fallback(model: &str) -> Option<ModelCapability> {
    let key = normalize_model_id(model);
    let (context_window, max_output_tokens, supports_tools) = match key.as_str() {
        // `auto` is resolved by the gateway, which only routes to candidates
        // whose window fits the request. Budget conservatively for the
        // smallest mainstream free window so compaction happens in time.
        "auto" | "default" => (131_072, 32_768, true),
        "groq/openai/gpt-oss-120b" | "groq/openai/gpt-oss-20b" => (131_072, 65_536, true),
        "groq/qwen/qwen3.8-27b" => (131_072, 16_384, true),
        "gemini/gemini-2.5-flash" | "gemini/gemini-3.5-flash" | "gemini/gemini-2.5-pro" => {
            (1_048_576, 65_536, true)
        }
        "cohere/north-mini-code:free" => (256_000, 64_000, true),
        "poolside/laguna-s-2.1:free" => (262_144, 32_768, true),
        "nvidia/nvidia/nemotron-3-super-120b-a12b" => (262_144, 65_536, true),
        "@cf/meta/llama-3.3-70b-instruct-fp8-fast" => (24_000, 8_192, true),
        "@cf/openai/gpt-oss-120b" => (128_000, 32_768, true),
        // Provider-native models reached without the gateway (direct API keys).
        "claude-opus-4-6" => (200_000, 32_000, true),
        "claude-sonnet-4-6" | "claude-haiku-4-5-20251213" => (200_000, 64_000, true),
        "grok-3" | "grok-3-mini" => (131_072, 64_000, true),
        _ => return None,
    };
    Some(ModelCapability::new(
        key,
        context_window,
        max_output_tokens,
        supports_tools,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_fallback, context_window_or_default, normalize_model_id, ModelCapability,
        DEFAULT_CONTEXT_WINDOW,
    };

    #[test]
    fn normalizes_transport_prefix_only_for_qualified_ids() {
        assert_eq!(
            normalize_model_id("openai/gemini/gemini-2.5-flash"),
            "gemini/gemini-2.5-flash"
        );
        assert_eq!(normalize_model_id("openai/auto"), "auto");
        assert_eq!(
            normalize_model_id("openai/gpt-oss-120b"),
            "openai/gpt-oss-120b"
        );
        assert_eq!(
            normalize_model_id("  GROQ/openai/gpt-oss-20b "),
            "groq/openai/gpt-oss-20b"
        );
    }

    #[test]
    fn builtin_table_covers_verified_free_models() {
        let cap = builtin_fallback("gemini/gemini-2.5-flash").expect("known model");
        assert_eq!(cap.context_window, 1_048_576);
        assert!(cap.supports_tools);
        assert!(builtin_fallback("unknown/model").is_none());
    }

    #[test]
    fn registered_entries_override_fallback() {
        // Use an id outside the fallback table so parallel tests cannot race.
        super::register([ModelCapability::new(
            "test/registry-only",
            4_096,
            1_024,
            false,
        )]);
        let cap = super::lookup("openai/test/registry-only").expect("registered");
        assert_eq!(cap.context_window, 4_096);
        assert_eq!(
            context_window_or_default("test/not-registered"),
            DEFAULT_CONTEXT_WINDOW
        );
    }
}
