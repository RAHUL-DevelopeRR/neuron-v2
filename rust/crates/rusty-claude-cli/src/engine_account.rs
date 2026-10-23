//! The Zero-X account as the engine's UIs show it ([`UiEvent::Account`]): the login (email,
//! name and plan stored with it, so it shows at once and offline), then the plan's limits
//! and today's usage from `GET /api/account`, read on a thread of its own, and kept current
//! from the quota headers of each gateway reply.
//!
//! What `/api/account` says about the profile (email, name, plan) is stored back with the
//! login, so the next start shows it before the network answers.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use neuron_tui::protocol::{AccountDetails, ErrorDetail, UiEvent, UsageAmounts};

use crate::account::AccountSummary;
use crate::auth::{CredentialSource, GatewayCredential};
use crate::credentials::{CredentialStore, StoredCredential};
use crate::error_detail::DetailedError;

/// What the engine knows of the account beyond the stored login.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AccountCache {
    /// The last `/api/account` answer, with today's usage kept current from the quota
    /// headers since.
    summary: Option<AccountSummary>,
    /// When `summary` was read (Unix seconds).
    fetched_at: Option<u64>,
    /// Why the last read failed, until one succeeds.
    error: Option<String>,
    /// The quota headers `summary`'s usage was last adjusted from.
    quota_seen: Option<api::QuotaRemaining>,
}

/// The cache, shared with the refresh threads.
pub(crate) type SharedAccountCache = Arc<Mutex<AccountCache>>;

/// The account event for the login there is now (`NEURON_TOKEN`, else the stored one).
/// `email` is what a sign-in in this engine reported, which wins over what is stored.
pub(crate) fn account_event(cache: &SharedAccountCache, email: Option<&str>) -> UiEvent {
    let cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    match crate::auth::current_credential() {
        Ok(Some(credential)) => UiEvent::Account {
            signed_in: !credential.is_expired(crate::clock::now_unix()),
            account: Some(account_details(&credential, &cache, email)),
        },
        _ => UiEvent::Account {
            signed_in: false,
            account: None,
        },
    }
}

/// The account a UI shows: the stored profile, overlaid with the last `/api/account`.
fn account_details(
    credential: &GatewayCredential,
    cache: &AccountCache,
    email: Option<&str>,
) -> AccountDetails {
    let summary = cache.summary.as_ref();
    let base = crate::auth::gateway_base_url()
        .unwrap_or_else(|_| crate::auth::GATEWAY_BASE_URL.to_string());
    AccountDetails {
        email: email
            .map(str::to_string)
            .or_else(|| summary.and_then(|summary| summary.email.clone()))
            .or_else(|| credential.email.clone()),
        name: summary
            .and_then(|summary| summary.name.clone())
            .or_else(|| credential.name.clone()),
        plan: summary
            .and_then(|summary| summary.plan_id.clone())
            .or_else(|| credential.plan.clone()),
        plan_name: summary
            .and_then(|summary| summary.plan_name.clone())
            .or_else(|| credential.plan.clone()),
        source: credential.source.describe(),
        expires_at: credential.expires_at,
        limits: summary.map(|summary| UsageAmounts {
            requests: summary.daily_requests_limit,
            tokens: summary.daily_tokens_limit,
        }),
        usage_today: summary.map(|summary| UsageAmounts {
            requests: summary.requests_today,
            tokens: summary.tokens_today,
        }),
        resets_at: summary.and_then(|summary| summary.resets_at.clone()),
        credit_balance_cents: summary
            .and_then(|summary| summary.credit_balance_usd)
            .and_then(usd_to_cents),
        suspended: summary.is_some_and(|summary| summary.suspended),
        dashboard_url: format!("{}/dashboard", crate::gateway_http::origin(&base)),
        pricing_url: crate::account::pricing_url(),
        usage_updated_at: cache.fetched_at,
        usage_error: cache.error.clone(),
    }
}

/// Whole cents of a dollar amount; `None` for an amount that is not one.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn usd_to_cents(usd: f64) -> Option<u64> {
    let cents = (usd * 100.0).round();
    // Below 2^53 every whole number is exact, so the cast keeps it.
    (cents.is_finite() && (0.0..9_007_199_254_740_992.0).contains(&cents)).then(|| cents as u64)
}

/// Reads `/api/account` on a thread of its own, then sends the account event. Without a
/// login nothing is read or sent, and this returns false. `email` is passed on to
/// [`account_event`].
pub(crate) fn refresh_in_background(
    cache: &SharedAccountCache,
    events: &Sender<UiEvent>,
    email: Option<String>,
) -> bool {
    let Ok(Some(credential)) = crate::auth::current_credential() else {
        return false;
    };
    let thread_cache = Arc::clone(cache);
    let thread_events = Sender::clone(events);
    let thread_email = email.clone();
    let spawned = thread::Builder::new()
        .name("neuron-account".to_string())
        .spawn(move || {
            refresh(&thread_cache, &credential);
            let _ = thread_events.send(account_event(&thread_cache, thread_email.as_deref()));
        });
    if spawned.is_err() {
        // No thread to read the account: the UI still gets the stored login.
        let _ = events.send(account_event(cache, email.as_deref()));
    }
    true
}

/// Reads `/api/account` for `credential` into `cache`, and stores the profile it reports
/// with the login.
fn refresh(cache: &SharedAccountCache, credential: &GatewayCredential) {
    let _ = read_into(cache, credential);
}

/// Reads `/api/account` into `cache` now, on this thread, for a report that needs it
/// current (`/usage`). Without a login, says so with the `sign_in_required` code.
pub(crate) fn read_now(cache: &SharedAccountCache) -> Result<AccountSummary, DetailedError> {
    let credential = crate::auth::current_credential()
        .ok()
        .flatten()
        .ok_or_else(|| DetailedError {
            message: "Not signed in to Zero-X. Run /login to see your plan and usage.".to_string(),
            detail: Some(ErrorDetail {
                code: "sign_in_required".to_string(),
                status: None,
                upgrade_url: None,
                resets_at: None,
                retry_after_seconds: None,
            }),
        })?;
    read_into(cache, &credential)
}

/// Reads `/api/account` for `credential` into `cache` (its error too), and stores the
/// profile it reports with the login.
fn read_into(
    cache: &SharedAccountCache,
    credential: &GatewayCredential,
) -> Result<AccountSummary, DetailedError> {
    let result = crate::auth::gateway_base_url()
        .map_err(|error| DetailedError {
            message: error.to_string(),
            detail: None,
        })
        .and_then(|base| {
            crate::account::fetch_account(&base, &credential.token).map_err(|error| DetailedError {
                message: error.to_string(),
                detail: crate::error_detail::from_gateway_http_error(&error),
            })
        });
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    match &result {
        Ok(summary) => {
            remember_profile(credential, summary);
            cache.summary = Some(summary.clone());
            cache.fetched_at = Some(crate::clock::now_unix());
            cache.error = None;
            cache.quota_seen = None;
        }
        Err(error) => cache.error = Some(error.message.clone()),
    }
    result
}

/// Stores the email, name and plan `/api/account` reported with the stored login they
/// belong to, when they changed. `NEURON_TOKEN` logins are not stored.
fn remember_profile(credential: &GatewayCredential, summary: &AccountSummary) {
    let CredentialSource::Stored(_) = credential.source else {
        return;
    };
    let email = summary.email.clone().or_else(|| credential.email.clone());
    let name = summary.name.clone().or_else(|| credential.name.clone());
    let plan = summary
        .plan_name
        .clone()
        .or_else(|| summary.plan_id.clone())
        .or_else(|| credential.plan.clone());
    if email == credential.email && name == credential.name && plan == credential.plan {
        return;
    }
    let store = CredentialStore::open_default();
    // Only over the same login: a sign-in may have replaced it meanwhile.
    let Some(loaded) = store.load() else {
        return;
    };
    if loaded.credential.token != credential.token {
        return;
    }
    let stored = StoredCredential {
        email,
        name,
        plan,
        ..loaded.credential
    };
    let _ = store.save(&stored);
}

/// Adjusts today's usage from the quota headers of the last gateway reply, when they
/// changed. Returns whether the account changed (and so needs a new event).
pub(crate) fn apply_quota_headers(cache: &SharedAccountCache) -> bool {
    let Some(quota) = api::last_quota_remaining() else {
        return false;
    };
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    apply_quota(&mut cache, quota)
}

fn apply_quota(cache: &mut AccountCache, quota: api::QuotaRemaining) -> bool {
    if cache.quota_seen == Some(quota) {
        return false;
    }
    cache.quota_seen = Some(quota);
    let Some(summary) = cache.summary.as_mut() else {
        return false;
    };
    let mut changed = false;
    if let (Some(limit), Some(left)) = (summary.daily_requests_limit, quota.requests) {
        summary.requests_today = Some(limit.saturating_sub(left));
        changed = true;
    }
    if let (Some(limit), Some(left)) = (summary.daily_tokens_limit, quota.tokens) {
        summary.tokens_today = Some(limit.saturating_sub(left));
        changed = true;
    }
    changed
}

/// Forgets the account (after `/logout`).
pub(crate) fn forget(cache: &SharedAccountCache) {
    *cache.lock().unwrap_or_else(PoisonError::into_inner) = AccountCache::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::TokenKind;
    use crate::vault::SecureString;

    fn credential() -> GatewayCredential {
        GatewayCredential {
            token: SecureString::new("ses_abcdefghijklmnopqrstuvwxyz012345".to_string()),
            kind: TokenKind::Session,
            source: CredentialSource::EnvToken,
            expires_at: Some(4_070_908_800),
            plan: Some("Free".to_string()),
            email: Some("stored@example.com".to_string()),
            name: Some("Stored Name".to_string()),
        }
    }

    fn summary() -> AccountSummary {
        crate::account::parse_account(&serde_json::json!({
            "user": {"email": "dev@example.com", "name": "Dev Example"},
            "plan": {"id": "pro", "name": "Pro"},
            "limits": {"daily_requests": 2000, "daily_tokens": 10_000_000},
            "usage_today": {"requests": 37, "tokens": 120_431, "resets_at": "2026-10-24T00:00:00Z"},
            "credits": {"balance_usd": 7.42},
            "flags": {"suspended": false}
        }))
    }

    #[test]
    fn the_stored_login_shows_until_the_account_is_read() {
        let details = account_details(&credential(), &AccountCache::default(), None);
        assert_eq!(details.email.as_deref(), Some("stored@example.com"));
        assert_eq!(details.name.as_deref(), Some("Stored Name"));
        assert_eq!(details.plan_name.as_deref(), Some("Free"));
        assert_eq!(details.limits, None);
        assert_eq!(details.usage_today, None);
        assert_eq!(details.usage_updated_at, None);
        assert!(details.pricing_url.starts_with("http"));
        assert!(details.dashboard_url.ends_with("/dashboard"));

        let cache = AccountCache {
            summary: Some(summary()),
            fetched_at: Some(100),
            ..AccountCache::default()
        };
        let details = account_details(&credential(), &cache, Some("signed-in@example.com"));
        assert_eq!(details.email.as_deref(), Some("signed-in@example.com"));
        assert_eq!(details.name.as_deref(), Some("Dev Example"));
        assert_eq!(details.plan.as_deref(), Some("pro"));
        assert_eq!(details.plan_name.as_deref(), Some("Pro"));
        assert_eq!(
            details.limits,
            Some(UsageAmounts {
                requests: Some(2000),
                tokens: Some(10_000_000)
            })
        );
        assert_eq!(
            details.usage_today,
            Some(UsageAmounts {
                requests: Some(37),
                tokens: Some(120_431)
            })
        );
        assert_eq!(details.resets_at.as_deref(), Some("2026-10-24T00:00:00Z"));
        assert_eq!(details.credit_balance_cents, Some(742));
        assert_eq!(details.usage_updated_at, Some(100));
    }

    #[test]
    fn quota_headers_move_todays_usage_once() {
        let mut cache = AccountCache {
            summary: Some(summary()),
            ..AccountCache::default()
        };
        let quota = api::QuotaRemaining {
            requests: Some(1_900),
            tokens: None,
        };
        assert!(apply_quota(&mut cache, quota));
        let usage = cache.summary.as_ref().expect("a summary");
        assert_eq!(usage.requests_today, Some(100));
        assert_eq!(usage.tokens_today, Some(120_431), "no token header");
        assert!(!apply_quota(&mut cache, quota), "the same headers again");

        let mut unread = AccountCache::default();
        assert!(!apply_quota(&mut unread, quota), "no limits to count from");
    }

    #[test]
    fn dollars_become_whole_cents() {
        assert_eq!(usd_to_cents(7.42), Some(742));
        assert_eq!(usd_to_cents(0.0), Some(0));
        assert_eq!(usd_to_cents(-1.0), None);
        assert_eq!(usd_to_cents(f64::NAN), None);
    }
}
