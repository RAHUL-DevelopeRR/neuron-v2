//! Builds the conversation's [`runtime::ModelRouting`] from the settings
//! (`toolCapableModels`, `autoToolModel`, `providerFallbacks`) and the gateway
//! catalog. The runtime does the switching (see `runtime::model_routing`).

use std::collections::BTreeSet;

use runtime::{ModelRouting, ModelRoutingSettings, PermissionMode, RuntimeFeatureConfig};

use crate::gateway_catalog::Catalog;
use crate::provider::{shown_model, AUTO_MODEL};

/// Catalog id fragments of models seen to call tools reliably, best first: the
/// retry targets for a chat-only answer when the session runs on `auto` and
/// `toolCapableModels` is not set. (`auto` once answered a build request in chat on
/// `groq/openai/gpt-oss-120b`; `nemotron-3-ultra` writes files.)
const DEFAULT_TOOL_MODEL_PATTERNS: &[&str] =
    &["nemotron-3-ultra", "nemotron-ultra", "nemotron-3-super"];

/// Tools whose presence makes a session able to change files.
const WRITE_TOOLS: &[&str] = &["write_file", "edit_file", "NotebookEdit", "bash"];

/// `model` as the client for a session on `session_model` sends it: with the
/// `openai/` transport prefix when the session's model has one.
pub(crate) fn transport_model(session_model: &str, model: &str) -> String {
    if session_model.starts_with("openai/") && !model.starts_with("openai/") {
        format!("openai/{model}")
    } else {
        model.to_string()
    }
}

fn is_auto(model: &str) -> bool {
    shown_model(model) == AUTO_MODEL
}

/// The model requests go to: `autoToolModel` in place of `auto` when tools are
/// attached, the session's model otherwise.
pub(crate) fn request_model(
    settings: &ModelRoutingSettings,
    session_model: &str,
    enable_tools: bool,
) -> String {
    match settings.auto_tool_model() {
        Some(model) if enable_tools && is_auto(session_model) => {
            transport_model(session_model, model)
        }
        _ => session_model.to_string(),
    }
}

/// Models a chat-only answer is retried on, as the client sends them:
/// `toolCapableModels` when set; for a session on `auto`, the catalog's available
/// tool-capable models matching [`DEFAULT_TOOL_MODEL_PATTERNS`]; none otherwise.
pub(crate) fn tool_retry_models(
    settings: &ModelRoutingSettings,
    session_model: &str,
    catalog: Option<&Catalog>,
) -> Vec<String> {
    if !settings.tool_capable_models().is_empty() {
        return settings
            .tool_capable_models()
            .iter()
            .map(|model| transport_model(session_model, model))
            .collect();
    }
    let (true, Some(catalog)) = (is_auto(session_model), catalog) else {
        return Vec::new();
    };
    let mut models = Vec::new();
    for pattern in DEFAULT_TOOL_MODEL_PATTERNS {
        for model in &catalog.models {
            let id = model.id.to_ascii_lowercase();
            if model.available
                && model.supports_tools
                && id.contains(pattern)
                && !models.contains(&model.id)
            {
                models.push(model.id.clone());
            }
        }
    }
    models
        .into_iter()
        .map(|model| transport_model(session_model, &model))
        .collect()
}

/// Whether the session can change files: tools attached, a permission mode that
/// allows writing and an allow-list (if any) with a tool that writes.
pub(crate) fn is_agent_mode(
    enable_tools: bool,
    permission_mode: PermissionMode,
    allowed_tools: Option<&BTreeSet<String>>,
) -> bool {
    enable_tools
        && permission_mode != PermissionMode::ReadOnly
        && allowed_tools.is_none_or(|tools| WRITE_TOOLS.iter().any(|tool| tools.contains(*tool)))
}

/// The routing of a conversation on `session_model`.
pub(crate) fn conversation_routing(
    feature_config: &RuntimeFeatureConfig,
    session_model: &str,
    agent_mode: bool,
    catalog: Option<&Catalog>,
) -> ModelRouting {
    ModelRouting {
        agent_mode,
        tool_retry_models: tool_retry_models(
            feature_config.model_routing(),
            session_model,
            catalog,
        ),
        fallbacks: feature_config
            .provider_fallbacks()
            .fallbacks()
            .iter()
            .map(|model| transport_model(session_model, model))
            .collect(),
    }
}

/// `summaryModel` as the client sends it, when set.
pub(crate) fn summary_model(
    settings: &ModelRoutingSettings,
    session_model: &str,
) -> Option<String> {
    settings
        .summary_model()
        .map(|model| transport_model(session_model, model))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway_catalog::CatalogModel;

    fn model(id: &str, tools: bool, available: bool) -> CatalogModel {
        CatalogModel {
            id: id.to_string(),
            display_name: None,
            context_window: None,
            max_output_tokens: None,
            supports_tools: tools,
            tiers: Vec::new(),
            available,
            unavailable_reason: None,
            status: None,
            available_again_at: None,
            supports_images: None,
        }
    }

    fn catalog(models: Vec<CatalogModel>) -> Catalog {
        Catalog {
            base_url: "https://gateway.test".to_string(),
            credential_key: "anonymous".to_string(),
            fetched_at: 0,
            models,
        }
    }

    fn settings() -> ModelRoutingSettings {
        ModelRoutingSettings::default()
    }

    #[test]
    fn auto_retries_on_available_nemotron_class_models_from_the_catalog() {
        let catalog = catalog(vec![
            model("auto", true, true),
            model("groq/openai/gpt-oss-120b", true, true),
            model("nvidia/nvidia/nemotron-3-super-120b-a12b", true, true),
            model("nvidia/nvidia/nemotron-3-ultra-550b-a55b", true, true),
            model("nvidia/nemotron-ultra-offline", true, false),
        ]);
        assert_eq!(
            tool_retry_models(&settings(), "openai/auto", Some(&catalog)),
            vec![
                "openai/nvidia/nvidia/nemotron-3-ultra-550b-a55b".to_string(),
                "openai/nvidia/nvidia/nemotron-3-super-120b-a12b".to_string(),
            ]
        );
        assert!(
            tool_retry_models(
                &settings(),
                "openai/gemini/gemini-2.5-flash",
                Some(&catalog)
            )
            .is_empty(),
            "a model the user picked is not second-guessed without toolCapableModels"
        );
        assert!(tool_retry_models(&settings(), "openai/auto", None).is_empty());
    }

    #[test]
    fn configured_tool_capable_models_win() {
        let settings = ModelRoutingSettings {
            tool_capable_models: vec!["kimi-k2".to_string()],
            ..ModelRoutingSettings::default()
        };
        assert_eq!(
            tool_retry_models(&settings, "openai/gemini/gemini-2.5-flash", None),
            vec!["openai/kimi-k2".to_string()]
        );
        assert_eq!(
            tool_retry_models(&settings, "claude-opus-4-6", None),
            vec!["kimi-k2".to_string()]
        );
    }

    #[test]
    fn auto_tool_model_replaces_auto_only_with_tools() {
        let settings = ModelRoutingSettings {
            auto_tool_model: Some("nemotron-3-ultra".to_string()),
            ..ModelRoutingSettings::default()
        };
        assert_eq!(
            request_model(&settings, "openai/auto", true),
            "openai/nemotron-3-ultra"
        );
        assert_eq!(
            request_model(&settings, "openai/auto", false),
            "openai/auto"
        );
        assert_eq!(
            request_model(&settings, "openai/kimi-k2", true),
            "openai/kimi-k2"
        );
        assert_eq!(
            request_model(&ModelRoutingSettings::default(), "openai/auto", true),
            "openai/auto"
        );
    }

    #[test]
    fn agent_mode_needs_tools_that_write_and_a_mode_that_allows_them() {
        let read_only = ["read_file".to_string(), "grep_search".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let writer = ["write_file".to_string()]
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert!(is_agent_mode(true, PermissionMode::WorkspaceWrite, None));
        assert!(is_agent_mode(true, PermissionMode::Prompt, Some(&writer)));
        assert!(!is_agent_mode(false, PermissionMode::WorkspaceWrite, None));
        assert!(!is_agent_mode(true, PermissionMode::ReadOnly, None));
        assert!(!is_agent_mode(
            true,
            PermissionMode::WorkspaceWrite,
            Some(&read_only)
        ));
    }

    #[test]
    fn fallbacks_and_summary_model_use_the_session_transport() {
        let feature_config = RuntimeFeatureConfig::default();
        let routing = conversation_routing(&feature_config, "openai/auto", true, None);
        assert!(routing.agent_mode);
        assert!(routing.fallbacks.is_empty());

        let settings = ModelRoutingSettings {
            summary_model: Some("groq/openai/gpt-oss-20b".to_string()),
            ..ModelRoutingSettings::default()
        };
        assert_eq!(
            summary_model(&settings, "openai/auto").as_deref(),
            Some("openai/groq/openai/gpt-oss-20b")
        );
        assert_eq!(summary_model(&ModelRoutingSettings::default(), "x"), None);
    }
}
