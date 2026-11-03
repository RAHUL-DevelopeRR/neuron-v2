//! What a failure needs from the user, as the engine's `error` events carry it
//! ([`ErrorDetail`]): the gateway's code, the HTTP status, an upgrade link, a reset time or
//! a retry delay, so a UI can offer sign-in, upgrade or wait instead of a raw message.
//!
//! The runtime reports a failed turn as text only, so the model client records the typed
//! failure of its last model call here (per thread: a sub-agent's calls run on threads of
//! their own) and the engine takes it when the turn fails.

use std::cell::RefCell;
use std::fmt::{Display, Formatter};

use neuron_tui::protocol::ErrorDetail;

use crate::device_login::DeviceLoginError;
use crate::gateway_http::GatewayHttpError;

thread_local! {
    static LAST_MODEL_CALL_FAILURE: RefCell<Option<ErrorDetail>> = const { RefCell::new(None) };
}

/// Records what the failed model call on this thread needs from the user (nothing for a
/// failure that has no detail to act on).
pub(crate) fn record_model_call_failure(error: &api::ApiError) {
    let detail = from_api_error(error);
    LAST_MODEL_CALL_FAILURE.with(|last| *last.borrow_mut() = detail);
}

/// Forgets the last model call's failure: a new call started, or one succeeded.
pub(crate) fn clear_model_call_failure() {
    LAST_MODEL_CALL_FAILURE.with(|last| *last.borrow_mut() = None);
}

/// The last model call's failure on this thread, once.
pub(crate) fn take_model_call_failure() -> Option<ErrorDetail> {
    LAST_MODEL_CALL_FAILURE.with(|last| last.borrow_mut().take())
}

/// The detail of a provider error: the gateway's envelope when there is one, else what the
/// HTTP status or a transport failure says.
pub(crate) fn from_api_error(error: &api::ApiError) -> Option<ErrorDetail> {
    if let Some(gateway) = error.gateway_error() {
        let status = gateway.status.as_u16();
        return Some(ErrorDetail {
            code: gateway
                .code
                .clone()
                .filter(|code| !code.trim().is_empty())
                .unwrap_or_else(|| code_for_status(status).to_string()),
            status: Some(status),
            upgrade_url: gateway.upgrade_url.clone(),
            resets_at: gateway.resets_at.clone(),
            retry_after_seconds: gateway.retry_after_seconds,
        });
    }
    match error {
        api::ApiError::RetriesExhausted { last_error, .. } => from_api_error(last_error),
        api::ApiError::Http(http) if http.is_connect() || http.is_timeout() => {
            Some(code_only(UNREACHABLE))
        }
        api::ApiError::Api { status, .. } => {
            let status = status.as_u16();
            let code = code_for_status(status);
            (code != OTHER).then(|| ErrorDetail {
                status: Some(status),
                ..code_only(code)
            })
        }
        _ => None,
    }
}

/// The detail of a failed device sign-in: the gateway out of reach, not offering device
/// sign-in yet, limiting attempts, or down. A denied or expired sign-in has none.
pub(crate) fn from_device_login_error(error: &DeviceLoginError) -> Option<ErrorDetail> {
    match error {
        DeviceLoginError::Unavailable(_) => Some(code_only(UNREACHABLE)),
        DeviceLoginError::Unsupported => Some(ErrorDetail {
            status: Some(404),
            ..code_only(NOT_AVAILABLE)
        }),
        DeviceLoginError::RateLimited(seconds) => Some(ErrorDetail {
            status: Some(429),
            retry_after_seconds: *seconds,
            ..code_only("rate_limited")
        }),
        DeviceLoginError::GatewayError { status, .. } if *status >= 500 => Some(ErrorDetail {
            status: Some(*status),
            ..code_only("service_unavailable")
        }),
        DeviceLoginError::GatewayError { .. }
        | DeviceLoginError::Denied
        | DeviceLoginError::Expired
        | DeviceLoginError::Cancelled
        | DeviceLoginError::Invalid(_) => None,
    }
}

/// The detail of a failed account call (`/api/account`, `/v1/models`).
pub(crate) fn from_gateway_http_error(error: &GatewayHttpError) -> Option<ErrorDetail> {
    match error {
        GatewayHttpError::InvalidSession(gateway) | GatewayHttpError::Api(gateway) => {
            from_api_error(&api::ApiError::Gateway(Box::new(gateway.clone())))
        }
        GatewayHttpError::Transport(_) => Some(code_only(UNREACHABLE)),
        GatewayHttpError::InvalidResponse(_) => None,
    }
}

/// The detail of an error from the sign-in path: a [`DeviceLoginError`] inside an
/// [`std::io::Error`], as `auth::login_with` returns it.
pub(crate) fn from_io_error(error: &std::io::Error) -> Option<ErrorDetail> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<DeviceLoginError>())
        .and_then(from_device_login_error)
}

/// An error the engine reports with its [`ErrorDetail`]; it reads as its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailedError {
    pub(crate) message: String,
    pub(crate) detail: Option<ErrorDetail>,
}

impl DetailedError {
    pub(crate) fn from_io(error: &std::io::Error) -> Self {
        Self {
            message: error.to_string(),
            detail: from_io_error(error),
        }
    }

    /// The detail of `error` when it is (or wraps) a [`DetailedError`].
    pub(crate) fn detail_of(error: &(dyn std::error::Error + 'static)) -> Option<ErrorDetail> {
        error
            .downcast_ref::<Self>()
            .and_then(|detailed| detailed.detail.clone())
    }
}

impl Display for DetailedError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DetailedError {}

const UNREACHABLE: &str = "unreachable";
const NOT_AVAILABLE: &str = "not_available";
const OTHER: &str = "";

fn code_only(code: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        status: None,
        upgrade_url: None,
        resets_at: None,
        retry_after_seconds: None,
    }
}

/// The gateway contract's code for an answer that carried none.
fn code_for_status(status: u16) -> &'static str {
    match status {
        401 => "invalid_session",
        402 => "insufficient_credits",
        403 => "forbidden",
        404 => NOT_AVAILABLE,
        429 => "rate_limited",
        500..=599 => "service_unavailable",
        _ => OTHER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gateway(status: u16, body: &str) -> api::ApiError {
        let status = reqwest::StatusCode::from_u16(status).expect("status");
        api::ApiError::Gateway(Box::new(
            api::GatewayError::from_body(status, body).expect("an error envelope"),
        ))
    }

    #[test]
    fn gateway_envelopes_keep_their_code_and_hints() {
        let quota = gateway(
            429,
            r#"{"error":{"message":"Daily request limit reached","code":"insufficient_quota","upgrade_url":"https://zero-x.live/pricing","retry_after_seconds":3600,"resets_at":"2026-10-24T00:00:00Z"}}"#,
        );
        let detail = from_api_error(&quota).expect("a detail");
        assert_eq!(detail.code, "insufficient_quota");
        assert_eq!(detail.status, Some(429));
        assert_eq!(
            detail.upgrade_url.as_deref(),
            Some("https://zero-x.live/pricing")
        );
        assert_eq!(detail.resets_at.as_deref(), Some("2026-10-24T00:00:00Z"));
        assert_eq!(detail.retry_after_seconds, Some(3600));

        let wrapped = api::ApiError::RetriesExhausted {
            attempts: 3,
            last_error: Box::new(gateway(
                403,
                r#"{"error":{"message":"Pro only","code":"upgrade_required","upgrade_url":"https://zero-x.live/pricing"}}"#,
            )),
        };
        assert_eq!(
            from_api_error(&wrapped).map(|detail| detail.code),
            Some("upgrade_required".to_string())
        );
        // An answer without a code reads its status.
        let bare = gateway(503, r#"{"error":"down for maintenance"}"#);
        assert_eq!(
            from_api_error(&bare).map(|detail| detail.code),
            Some("service_unavailable".to_string())
        );
    }

    #[test]
    fn plain_failures_have_a_detail_only_when_it_helps() {
        let rate_limited = api::ApiError::Api {
            status: reqwest::StatusCode::TOO_MANY_REQUESTS,
            error_type: None,
            message: None,
            request_id: None,
            body: String::new(),
            retryable: true,
            suggested_action: None,
        };
        assert_eq!(
            from_api_error(&rate_limited),
            Some(ErrorDetail {
                status: Some(429),
                ..code_only("rate_limited")
            })
        );
        let bad_request = api::ApiError::Api {
            status: reqwest::StatusCode::BAD_REQUEST,
            error_type: None,
            message: None,
            request_id: None,
            body: String::new(),
            retryable: false,
            suggested_action: None,
        };
        assert_eq!(from_api_error(&bad_request), None);
        assert_eq!(from_api_error(&api::ApiError::InvalidSseFrame("x")), None);
    }

    #[test]
    fn sign_in_failures_say_whether_zero_x_could_be_reached() {
        let unreachable: std::io::Error =
            DeviceLoginError::Unavailable("connection refused".into()).into();
        assert_eq!(from_io_error(&unreachable), Some(code_only(UNREACHABLE)));
        // The message is the sign-in's own.
        assert!(unreachable
            .to_string()
            .starts_with("Could not reach Zero-X"));

        let not_deployed: std::io::Error = DeviceLoginError::Unsupported.into();
        assert_eq!(
            from_io_error(&not_deployed).map(|detail| (detail.code, detail.status)),
            Some((NOT_AVAILABLE.to_string(), Some(404)))
        );
        let limited: std::io::Error = DeviceLoginError::RateLimited(Some(30)).into();
        assert_eq!(
            from_io_error(&limited).and_then(|detail| detail.retry_after_seconds),
            Some(30)
        );
        let denied: std::io::Error = DeviceLoginError::Denied.into();
        assert_eq!(from_io_error(&denied), None);
        assert_eq!(from_io_error(&std::io::Error::other("plain")), None);
    }

    #[test]
    fn the_last_model_call_failure_is_taken_once() {
        clear_model_call_failure();
        record_model_call_failure(&gateway(
            401,
            r#"{"error":{"message":"expired","code":"invalid_session"}}"#,
        ));
        assert_eq!(
            take_model_call_failure().map(|detail| detail.code),
            Some("invalid_session".to_string())
        );
        assert_eq!(take_model_call_failure(), None);
        record_model_call_failure(&api::ApiError::InvalidSseFrame("x"));
        assert_eq!(take_model_call_failure(), None);
    }

    #[test]
    fn detailed_errors_read_as_their_message_and_keep_the_detail() {
        let error: Box<dyn std::error::Error> = Box::new(DetailedError {
            message: "Could not reach Zero-X".into(),
            detail: Some(code_only(UNREACHABLE)),
        });
        assert_eq!(error.to_string(), "Could not reach Zero-X");
        assert_eq!(
            DetailedError::detail_of(error.as_ref()).map(|detail| detail.code),
            Some(UNREACHABLE.to_string())
        );
        let plain: Box<dyn std::error::Error> = "plain".into();
        assert_eq!(DetailedError::detail_of(plain.as_ref()), None);
    }
}
