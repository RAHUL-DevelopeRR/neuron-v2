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
pub(crate) fn session_catalog() -> Option<std::sync::Arc<crate::gateway_catalog::Catalog>> {
    active_provider().filter(|active| active.id == ProviderId::ZeroX)?;
    if let Some(catalog) = crate::gateway_catalog::current() {
        return Some(catalog);
    }
    let endpoint = api::installed_openai_compat_endpoint()?;
    crate::gateway_catalog::load(&endpoint.base_url, Some(endpoint.api_key()))
        .map_err(|error| runtime::ui_hooks::notice(&format!("warning: {error}")))
        .ok()
}

/// Catalog listing appended to `/model` when the session runs on Zero-X, with the
/// models' status as of now.
pub(crate) fn gateway_model_listing(current_model: &str) -> Option<String> {
    let catalog = session_catalog()?;
    Some(crate::gateway_catalog::render_model_list(
        &crate::gateway_catalog::with_session_status(&catalog),
        current_model,
        crate::clock::now_unix(),
    ))
}

/// Warning for `/model <id>` on Zero-X when the id is unknown, not available
/// to the user, or out of capacity now.
pub(crate) fn gateway_model_warning(model: &str) -> Option<String> {
    let catalog = session_catalog()?;
    crate::gateway_catalog::model_choice_warning(
        &crate::gateway_catalog::with_session_status(&catalog),
        model,
        crate::clock::now_unix(),
        false,
    )
}

/// `neuron prompt` on Zero-X with a model other than `auto`: the same warning,
/// on stderr before the first request, from the gateway's current status (asked
/// for once; without an answer the catalog's own status counts). It only warns:
/// the request is still sent.
pub(crate) fn warn_about_prompt_model(model: &str) {
    if crate::provider::shown_model(model).eq_ignore_ascii_case(crate::provider::AUTO_MODEL) {
        return;
    }
    let on_gateway = active_provider().is_some_and(|active| active.id == ProviderId::ZeroX);
    let Some(catalog) = crate::gateway_catalog::current().filter(|_| on_gateway) else {
        return;
    };
    let live = crate::gateway_catalog::with_session_status(&catalog);
    if let Some(warning) =
        crate::gateway_catalog::model_choice_warning(&live, model, crate::clock::now_unix(), true)
    {
        crate::provider::print_warning(&warning);
    }
}
