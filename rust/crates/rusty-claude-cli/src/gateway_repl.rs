//! REPL handlers for the Zero-X account commands: `/login`, `/logout`,
//! `/usage`, `/upgrade`, and the catalog-aware parts of `/model`.
//!
//! Errors are printed instead of returned so a failed sign-in or an
//! unreachable gateway never ends the REPL session.

use super::*;
use crate::provider::{active_provider, ProviderId};

impl LiveCli {
    /// `/login`: runs the browser login. When this session already talks to
    /// the gateway, the new credential takes effect immediately.
    pub(crate) fn gateway_login(&mut self) -> Result<bool, Box<dyn std::error::Error>> {
        let credential = match crate::auth::login(crate::auth::LoginOptions::default()) {
            Ok(credential) => credential,
            Err(error) => {
                eprintln!("Sign-in failed: {error}");
                return Ok(false);
            }
        };
        let Some(active) = active_provider().filter(|active| active.id == ProviderId::ZeroX) else {
            eprintln!(
                "Signed in. This session uses another provider; restart neuron (or pass --provider zero-x) to use the Zero-X gateway."
            );
            return Ok(false);
        };
        let base = active
            .base_url
            .clone()
            .unwrap_or_else(|| crate::auth::GATEWAY_BASE_URL.to_string());
        api::install_openai_compat_endpoint(Some(api::OpenAiCompatEndpoint::new(
            api::OpenAiCompatConfig::zero_x(),
            base.clone(),
            credential.token.expose(),
        )));
        if let Err(error) = crate::gateway_catalog::load(&base, Some(credential.token.expose())) {
            eprintln!("warning: {error}");
        }
        let session = self.runtime.session().clone();
        let runtime = build_runtime(
            session,
            &self.session.id,
            self.model.clone(),
            self.system_prompt.clone(),
            true,
            true,
            self.allowed_tools.clone(),
            self.permission_mode,
            None,
        )?;
        self.replace_runtime(runtime)?;
        println!("This session now uses your new Zero-X login.");
        Ok(true)
    }

    /// `/logout`: revokes and removes the stored login. Always succeeds.
    pub(crate) fn gateway_logout() -> bool {
        crate::auth::logout();
        if active_provider().is_some_and(|active| active.id == ProviderId::ZeroX) {
            eprintln!("Requests in this session will fail until you run /login again.");
        }
        false
    }

    /// `/usage`: plan, today's usage against limits, reset time, credits.
    pub(crate) fn gateway_usage() -> bool {
        match crate::account::usage_report() {
            Ok(report) => println!("{report}"),
            Err(error) => eprintln!("{error}"),
        }
        false
    }

    /// `/upgrade`: prints and opens the pricing page.
    pub(crate) fn gateway_upgrade() -> bool {
        println!("{}", crate::account::upgrade(true));
        false
    }
}

/// The session's catalog, fetched again with the session's gateway
/// credential when it was dropped (after `/upgrade` or a plan change).
fn session_catalog() -> Option<std::sync::Arc<crate::gateway_catalog::Catalog>> {
    active_provider().filter(|active| active.id == ProviderId::ZeroX)?;
    if let Some(catalog) = crate::gateway_catalog::current() {
        return Some(catalog);
    }
    let endpoint = api::installed_openai_compat_endpoint()?;
    crate::gateway_catalog::load(&endpoint.base_url, Some(endpoint.api_key()))
        .map_err(|error| runtime::ui_hooks::notice(&format!("warning: {error}")))
        .ok()
}

/// Catalog listing appended to `/model` when the session runs on Zero-X.
pub(crate) fn gateway_model_listing(current_model: &str) -> Option<String> {
    let catalog = session_catalog()?;
    Some(crate::gateway_catalog::render_model_list(
        &catalog,
        current_model,
    ))
}

/// Warning for `/model <id>` on Zero-X when the id is unknown or not
/// available on the user's plan.
pub(crate) fn gateway_model_warning(model: &str) -> Option<String> {
    let catalog = session_catalog()?;
    crate::gateway_catalog::model_choice_warning(&catalog, model)
}
