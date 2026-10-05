//! LLM provider resolution and connectivity probing.
//!
//! Resolves the account gateway and explicit provider overrides.

use crate::brand::*;
use api::{detect_provider_kind, ProviderKind};
use std::env;

/// Resolves the LLM provider in priority order:
///   1. Azure AI Foundry, when explicitly configured
///   2. Environment overrides (OPENAI_API_KEY + OPENAI_BASE_URL already set)
///   3. Zero-X account gateway
///
/// Returns (api_key, base_url, model, provider_label) tuple.
pub fn resolve_provider(
    requested_model: &str,
) -> Result<(String, String, String, &'static str), std::io::Error> {
    if env::var("NEURON_TOKEN").is_ok_and(|token| !token.trim().is_empty()) {
        return gateway_provider(requested_model);
    }
    // Priority 1: Azure AI Foundry, only when the user explicitly provides credentials.
    let quota = crate::quota::QuotaState::load();
    if !quota.is_azure_exhausted() {
        if let (Ok(azure_key), Ok(azure_base)) = (
            env::var("AZURE_OPENAI_API_KEY"),
            env::var("AZURE_OPENAI_ENDPOINT"),
        ) {
            if !azure_key.is_empty() && !azure_base.is_empty() {
                let azure_model =
                    env::var("AZURE_OPENAI_MODEL").unwrap_or_else(|_| "Kimi-K2.5".to_string());

                if azure_api_probe(&azure_key, &azure_base) {
                    eprintln!(
                        "\x1b[32m\u{2713}\x1b[0m Azure AI Foundry ({}) \u{00b7} Quota: {}",
                        azure_model,
                        quota.display_compact()
                    );
                    return Ok((azure_key, azure_base, openai_model(&azure_model), "azure"));
                }
                eprintln!(
                    "\x1b[33m\u{26a0}\x1b[0m Azure unavailable \u{2013} falling back to OpenRouter"
                );
            }
        }
    } else {
        eprintln!(
            "\x1b[33m\u{26a0}\x1b[0m Azure daily quota exhausted ({}) \u{2013} using fallback",
            quota.display_compact()
        );
    }

    // Priority 2: Explicit OpenAI-compatible override.
    if let Ok(key) = env::var("OPENAI_API_KEY") {
        if !key.is_empty() {
            let url = env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let model = env::var("NEURON_MODEL").unwrap_or_else(|_| requested_model.to_string());
            return Ok((key, url, openai_model(&model), "openai"));
        }
    }

    // Local OpenAI-compatible servers may deliberately require no credential.
    if let Ok(url) = env::var("OPENAI_BASE_URL") {
        let parsed = reqwest::Url::parse(&url).map_err(std::io::Error::other)?;
        if matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1")) {
            return Ok((String::new(), url, openai_model(requested_model), "openai"));
        }
    }

    // Direct OpenRouter remains available when explicitly configured.
    if let Ok(openrouter_key) = env::var("OPENROUTER_API_KEY") {
        if !openrouter_key.is_empty() {
            return Ok((
                openrouter_key,
                "https://openrouter.ai/api/v1".to_string(),
                openai_model(
                    &env::var("OPENROUTER_MODEL")
                        .unwrap_or_else(|_| "qwen/qwen3-coder-480b-a35b-instruct:free".to_string()),
                ),
                "openrouter",
            ));
        }
    }
    gateway_provider(requested_model)
}

fn gateway_provider(
    requested_model: &str,
) -> Result<(String, String, String, &'static str), std::io::Error> {
    let token = crate::auth::ensure_gateway_token()?;
    let base = crate::auth::gateway_base_url()?;
    let model = env::var("NEURON_MODEL").unwrap_or_else(|_| requested_model.to_string());
    // Force OpenAI transport even for catalog IDs that resemble another provider.
    let model = openai_model(&model);
    Ok((token.expose().to_string(), base, model, "zero-x"))
}

fn openai_model(model: &str) -> String {
    if model.starts_with("openai/") {
        model.to_string()
    } else {
        format!("openai/{model}")
    }
}

/// Quick non-blocking probe to check if the Azure endpoint is reachable.
pub fn azure_api_probe(api_key: &str, base_url: &str) -> bool {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
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

pub fn format_connected_line(model: &str) -> String {
    let provider = provider_label(detect_provider_kind(model));
    format!("{GREEN}{BOLD}\u{2713}{R} {DIM}Connected:{R} {BLUE}{BOLD}{model}{R} {DIM}via{R} {ORANGE}{provider}{R}")
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};

    use super::resolve_provider;

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn compatible_transport_prefix_preserves_vendor_model_ids() {
        assert_eq!(
            super::openai_model("qwen/qwen3-coder:free"),
            "openai/qwen/qwen3-coder:free"
        );
        assert_eq!(
            super::openai_model("claude-sonnet-4-6"),
            "openai/claude-sonnet-4-6"
        );
        assert_eq!(super::openai_model("openai/auto"), "openai/auto");
    }

    #[test]
    fn openai_env_resolution_respects_requested_model() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let original_key = std::env::var("OPENAI_API_KEY").ok();
        let original_url = std::env::var("OPENAI_BASE_URL").ok();
        let original_model = std::env::var("NEURON_MODEL").ok();

        std::env::set_var("OPENAI_API_KEY", "test-key");
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("NEURON_MODEL");

        let (key, base_url, model, label) = resolve_provider("gpt-4o-mini").unwrap();

        assert_eq!(key, "test-key");
        assert_eq!(base_url, "https://api.openai.com/v1");
        assert_eq!(model, "openai/gpt-4o-mini");
        assert_eq!(label, "openai");

        match original_key {
            Some(value) => std::env::set_var("OPENAI_API_KEY", value),
            None => std::env::remove_var("OPENAI_API_KEY"),
        }
        match original_url {
            Some(value) => std::env::set_var("OPENAI_BASE_URL", value),
            None => std::env::remove_var("OPENAI_BASE_URL"),
        }
        match original_model {
            Some(value) => std::env::set_var("NEURON_MODEL", value),
            None => std::env::remove_var("NEURON_MODEL"),
        }
    }
}
