//! LLM provider resolution and connectivity probing.
//!
//! Determines which API backend to use: Azure AI Foundry, OpenRouter, or a custom endpoint.

use crate::brand::*;
use api::{detect_provider_kind, ProviderKind};
use std::env;

/// Resolves the LLM provider in priority order:
///   1. Auth server proxy (localhost:19284) — keyless Azure via session token
///   2. Azure AI Foundry raw env vars (fallback if auth server unavailable)
///   3. Environment overrides (OPENAI_API_KEY + OPENAI_BASE_URL already set)
///   4. OpenRouter free tier (fallback)
///
/// Returns (api_key, base_url, model, provider_label) tuple.
pub fn resolve_provider(requested_model: &str) -> (String, String, String, &'static str) {
    let quota = crate::quota::QuotaState::load();

    // Priority 1a: Gateway server — CLI authenticates via session token,
    // server holds all provider API keys. No raw API key on the client.
    if !quota.is_azure_exhausted() {
        if let Some(session) = try_auth_server_session() {
            let gateway_base = env::var("NEURON_GATEWAY_URL")
                .unwrap_or_else(|_| "https://api.zero-x.live".to_string());
            let model = env::var("AZURE_OPENAI_MODEL").unwrap_or_else(|_| "Kimi-K2.5".to_string());
            eprintln!(
                "\x1b[32m\u{2713}\x1b[0m NeuronCLI Gateway \u{2192} {} \u{00b7} Quota: {}",
                model,
                quota.display_compact()
            );
            // Use the gateway's /v1 endpoint — openai_compat appends /chat/completions
            return (session, format!("{}/v1", gateway_base), model, "azure");
        }
    }

    // Priority 1b: Azure AI Foundry raw env vars (direct, no auth server)
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
                    return (azure_key, azure_base, azure_model, "azure");
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
            return (key, url, model, "openai");
        }
    }

    // Priority 3: OpenRouter free (via existing PKCE auth)
    if let Ok(openrouter_key) = env::var("OPENROUTER_API_KEY") {
        if !openrouter_key.is_empty() {
            return (
                openrouter_key,
                "https://openrouter.ai/api/v1".to_string(),
                env::var("OPENROUTER_MODEL")
                    .unwrap_or_else(|_| "qwen/qwen3-coder:free".to_string()),
                "openrouter",
            );
        }
    }
    if let Some(openrouter_key) = crate::auth::ensure_api_key() {
        return (
            openrouter_key,
            "https://openrouter.ai/api/v1".to_string(),
            env::var("OPENROUTER_MODEL").unwrap_or_else(|_| "qwen/qwen3-coder:free".to_string()),
            "openrouter",
        );
    }

    // Nothing works
    eprintln!("\x1b[31m\u{2717}\x1b[0m No API provider available. Set OPENAI_API_KEY or authenticate via neuron auth.");
    std::process::exit(1);
}

/// Quick non-blocking probe to check if the Azure AI Foundry endpoint is reachable.
/// Uses the Models-as-a-Service path: /models/chat/completions
pub fn azure_api_probe(api_key: &str, base_url: &str) -> bool {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Azure AI Foundry MaaS uses /models/chat/completions (NOT /chat/completions)
    let url = format!(
        "{}/models/chat/completions?api-version=2024-05-01-preview",
        base_url.trim_end_matches('/')
    );
    let body = serde_json::json!({
        "model": "Kimi-K2.5",
        "messages": [{"role": "user", "content": "ping"}],
        "max_tokens": 1
    });
    match client
        .post(&url)
        .header("content-type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            // 200=ok, 429=rate-limited (still reachable), 400=bad request (model alive)
            status == 200 || status == 429 || status == 400
        }
        Err(_) => false,
    }
}

/// Try to obtain a session token from the NeuronCLI Gateway Server.
/// The server holds all provider API keys. Client only gets a session token.
/// Returns `Some(session_token)` if the gateway is running and responds.
/// Returns `None` if the server is unreachable (falls through to other providers).
fn try_auth_server_session() -> Option<String> {
    // Gateway server URL — localhost for dev, zero-x.live for production
    let gateway_base =
        env::var("NEURON_GATEWAY_URL").unwrap_or_else(|_| "https://api.zero-x.live".to_string());

    // Check for cached session token first
    let session_path = dirs::home_dir()
        .unwrap_or_default()
        .join(".neuroncli")
        .join("session.json");

    if let Ok(content) = std::fs::read_to_string(&session_path) {
        if let Ok(cached) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(token) = cached["session_token"].as_str() {
                if !token.is_empty() {
                    // Verify session is still valid
                    let client = reqwest::blocking::Client::builder()
                        .timeout(std::time::Duration::from_secs(2))
                        .build()
                        .ok()?;
                    let resp = client
                        .get(format!("{}/auth/session", gateway_base))
                        .header("Authorization", format!("Bearer {}", token))
                        .send()
                        .ok()?;
                    if resp.status().is_success() {
                        return Some(token.to_string());
                    }
                }
            }
        }
    }

    // No cached session — try to create one
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .ok()?;

    let fingerprint = format!(
        "{}-{}",
        env::var("USERNAME")
            .or_else(|_| env::var("USER"))
            .unwrap_or_else(|_| "unknown".into()),
        env::var("COMPUTERNAME")
            .or_else(|_| env::var("HOSTNAME"))
            .unwrap_or_else(|_| "unknown".into())
    );

    let resp = client
        .post(format!("{}/auth/session", gateway_base))
        .json(&serde_json::json!({
            "machine_fingerprint": fingerprint,
            "version": "6.3.0"
        }))
        .send()
        .ok()?;

    if !resp.status().is_success() {
        return None;
    }

    let body: serde_json::Value = resp.json().ok()?;
    let token = body["session_token"].as_str()?.to_string();

    // Cache the session
    if let Some(parent) = session_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &session_path,
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    );

    Some(token)
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
    fn openai_env_resolution_respects_requested_model() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let original_key = std::env::var("OPENAI_API_KEY").ok();
        let original_url = std::env::var("OPENAI_BASE_URL").ok();
        let original_model = std::env::var("NEURON_MODEL").ok();
        let original_gateway = std::env::var("NEURON_GATEWAY_URL").ok();
        let original_azure_key = std::env::var("AZURE_OPENAI_API_KEY").ok();
        let original_azure_endpoint = std::env::var("AZURE_OPENAI_ENDPOINT").ok();

        std::env::set_var("OPENAI_API_KEY", "test-key");
        std::env::remove_var("OPENAI_BASE_URL");
        std::env::remove_var("NEURON_MODEL");
        std::env::set_var("NEURON_GATEWAY_URL", "http://127.0.0.1:9");
        std::env::remove_var("AZURE_OPENAI_API_KEY");
        std::env::remove_var("AZURE_OPENAI_ENDPOINT");

        let (key, base_url, model, label) = resolve_provider("gpt-4o-mini");

        assert_eq!(key, "test-key");
        assert_eq!(base_url, "https://api.openai.com/v1");
        assert_eq!(model, "gpt-4o-mini");
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
        match original_gateway {
            Some(value) => std::env::set_var("NEURON_GATEWAY_URL", value),
            None => std::env::remove_var("NEURON_GATEWAY_URL"),
        }
        match original_azure_key {
            Some(value) => std::env::set_var("AZURE_OPENAI_API_KEY", value),
            None => std::env::remove_var("AZURE_OPENAI_API_KEY"),
        }
        match original_azure_endpoint {
            Some(value) => std::env::set_var("AZURE_OPENAI_ENDPOINT", value),
            None => std::env::remove_var("AZURE_OPENAI_ENDPOINT"),
        }
    }
}
