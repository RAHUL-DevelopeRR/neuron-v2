//! Blocking HTTP helpers for the Zero-X account endpoints (`/auth/*`,
//! `/api/*`, `/v1/models`). Errors are parsed into the gateway's
//! actionable envelope so callers can tell an expired session apart from an
//! outage.

use std::fmt::{Display, Formatter};
use std::time::Duration;

use api::{GatewayError, GatewayErrorCode};
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::Value;

/// Timeout for interactive account calls.
pub const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum GatewayHttpError {
    /// 401: the session expired or was revoked.
    InvalidSession(GatewayError),
    /// Any other non-2xx answer.
    Api(GatewayError),
    /// The server could not be reached.
    Transport(String),
    /// The server answered with something that is not the expected JSON.
    InvalidResponse(String),
}

impl GatewayHttpError {
    pub fn is_invalid_session(&self) -> bool {
        matches!(self, Self::InvalidSession(_))
    }
}

impl Display for GatewayHttpError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSession(error) | Self::Api(error) => write!(f, "{error}"),
            Self::Transport(message) => write!(f, "could not reach Zero-X: {message}"),
            Self::InvalidResponse(message) => {
                write!(f, "unexpected response from Zero-X: {message}")
            }
        }
    }
}

impl std::error::Error for GatewayHttpError {}

/// Origin for account endpoints: the gateway base without its `/v1` suffix.
pub fn origin(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    trimmed
        .strip_suffix("/v1")
        .unwrap_or(trimmed)
        .trim_end_matches('/')
        .to_string()
}

/// `origin(base_url) + path`.
pub fn account_url(base_url: &str, path: &str) -> String {
    format!("{}{path}", origin(base_url))
}

pub fn client(timeout: Duration) -> Result<Client, GatewayHttpError> {
    Client::builder()
        .user_agent(api::USER_AGENT)
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .timeout(timeout)
        .build()
        .map_err(|error| GatewayHttpError::Transport(error.to_string()))
}

fn authorize(request: RequestBuilder, token: Option<&str>) -> RequestBuilder {
    match token {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

/// `GET url` and decode the JSON body.
pub fn get_json(
    url: &str,
    token: Option<&str>,
    timeout: Duration,
) -> Result<Value, GatewayHttpError> {
    let response = authorize(client(timeout)?.get(url), token)
        .header("accept", "application/json")
        .send()
        .map_err(|error| GatewayHttpError::Transport(error.to_string()))?;
    let response = check_status(response)?;
    response
        .json::<Value>()
        .map_err(|error| GatewayHttpError::InvalidResponse(error.to_string()))
}

/// `DELETE url`, ignoring the body.
pub fn delete(url: &str, token: Option<&str>, timeout: Duration) -> Result<(), GatewayHttpError> {
    let response = authorize(client(timeout)?.delete(url), token)
        .send()
        .map_err(|error| GatewayHttpError::Transport(error.to_string()))?;
    check_status(response).map(|_| ())
}

fn check_status(
    response: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, GatewayHttpError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response.text().unwrap_or_default();
    Err(classify_failure(status, &body, request_id))
}

/// Maps a non-2xx answer to the matching error. Any 401 from Zero-X means
/// the credential is no longer accepted, even from builds that predate the
/// `invalid_session` code.
pub fn classify_failure(
    status: reqwest::StatusCode,
    body: &str,
    request_id: Option<String>,
) -> GatewayHttpError {
    let mut error = GatewayError::from_body(status, body)
        .unwrap_or_else(|| GatewayError::from_status(status))
        .with_request_id_fallback(request_id);
    if status == reqwest::StatusCode::UNAUTHORIZED
        || error.code_kind() == Some(GatewayErrorCode::InvalidSession)
    {
        error.code = Some(GatewayErrorCode::InvalidSession.as_str().to_string());
        GatewayHttpError::InvalidSession(error)
    } else {
        GatewayHttpError::Api(error)
    }
}

/// Reads `value[key]` as a string (numbers are stringified).
pub fn string_field(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Reads `value[key]` as an unsigned integer (accepting numeric strings).
pub fn u64_field(value: &Value, key: &str) -> Option<u64> {
    match value.get(key)? {
        Value::Number(number) => number.as_u64().or_else(|| {
            number
                .as_f64()
                .and_then(|float| Duration::try_from_secs_f64(float).ok())
                .map(|rounded| rounded.as_secs())
        }),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_strips_the_v1_suffix() {
        assert_eq!(origin("https://zero-x.live/v1"), "https://zero-x.live");
        assert_eq!(origin("https://zero-x.live/v1/"), "https://zero-x.live");
        assert_eq!(origin("http://127.0.0.1:8787"), "http://127.0.0.1:8787");
        assert_eq!(
            account_url("http://127.0.0.1:8787/v1", "/api/account"),
            "http://127.0.0.1:8787/api/account"
        );
    }

    #[test]
    fn any_401_is_an_invalid_session() {
        let legacy = classify_failure(
            reqwest::StatusCode::UNAUTHORIZED,
            r#"{"error":"Invalid or expired session"}"#,
            Some("req_1".into()),
        );
        assert!(legacy.is_invalid_session());
        assert!(legacy.to_string().starts_with(api::INVALID_SESSION_MESSAGE));

        let quota = classify_failure(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            r#"{"error":{"message":"Daily quota reached","code":"insufficient_quota","upgrade_url":"https://zero-x.live/pricing"}}"#,
            None,
        );
        assert!(!quota.is_invalid_session());
        assert!(quota
            .to_string()
            .contains("upgrade: https://zero-x.live/pricing"));
    }

    #[test]
    fn numeric_fields_accept_numbers_and_strings() {
        let value = serde_json::json!({"a": 3, "b": "4", "c": 2.0, "d": null, "e": "x"});
        assert_eq!(u64_field(&value, "a"), Some(3));
        assert_eq!(u64_field(&value, "b"), Some(4));
        assert_eq!(u64_field(&value, "c"), Some(2));
        assert_eq!(u64_field(&value, "d"), None);
        assert_eq!(string_field(&value, "e").as_deref(), Some("x"));
        assert_eq!(string_field(&value, "a").as_deref(), Some("3"));
    }
}
