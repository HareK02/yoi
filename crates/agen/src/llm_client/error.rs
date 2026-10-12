//! Report provider communication failures to turn control.
//!
//! Context limit rejections are classified from structured provider codes;
//! fuzzy body matches must not imply success or retryability. The Engine maps
//! client errors to typed execution results, which the host records as the Run's
//! terminal outcome. Live diagnostic display and persistence are separate concerns.

use std::{fmt, time::Duration};

/// LLM client error.
#[derive(Debug)]
pub enum ClientError {
    /// HTTP request error.
    Http(reqwest::Error),
    /// JSON parse error.
    Json(serde_json::Error),
    /// SSE parse error.
    Sse(String),
    /// API error (an error response from the provider).
    Api {
        status: Option<u16>,
        code: Option<String>,
        message: String,
        retry_after: Option<Duration>,
    },
    /// The provider rejected the request because it exceeded the model context window.
    /// Classified only from a structured provider error code, never message text.
    ContextWindowExceeded,
    /// A request lifecycle phase exceeded its hard timeout.
    Timeout {
        phase: &'static str,
        timeout: Duration,
    },
    /// Configuration error.
    Config(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Http(e) => write!(f, "HTTP error: {}", e),
            ClientError::Json(e) => write!(f, "JSON parse error: {}", e),
            ClientError::Sse(msg) => write!(f, "SSE parse error: {}", msg),
            ClientError::Api {
                status,
                code,
                message,
                ..
            } => {
                write!(f, "API error")?;
                if let Some(s) = status {
                    write!(f, " (status: {})", s)?;
                }
                if let Some(c) = code {
                    write!(f, " [{}]", c)?;
                }
                write!(f, ": {}", message)
            }
            ClientError::ContextWindowExceeded => write!(f, "Model context window reached"),
            ClientError::Timeout { phase, timeout } => {
                write!(f, "{phase} timed out after {}s", timeout.as_secs())
            }
            ClientError::Config(msg) => write!(f, "Config error: {}", msg),
        }
    }
}

impl std::error::Error for ClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ClientError::Http(e) => Some(e),
            ClientError::Json(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(err: reqwest::Error) -> Self {
        ClientError::Http(err)
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        ClientError::Json(err)
    }
}

impl ClientError {
    pub fn status(&self) -> Option<u16> {
        match self {
            ClientError::Api { status, .. } => *status,
            _ => None,
        }
    }

    /// A provider-declared image size rejection, not a generic invalid input.
    /// Some providers (including OpenAI) expose only `invalid_value` here, so
    /// narrowly recognize their image-size diagnostic without hardcoded limits.
    pub fn is_image_size_rejection(&self) -> bool {
        let Self::Api {
            status: Some(400 | 413 | 422),
            code,
            message,
            ..
        } = self
        else {
            return false;
        };
        is_image_size_diagnostic(code.as_deref(), message)
    }

    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            ClientError::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

/// Shared diagnostic policy for HTTP rejections and provider Error events.
/// The caller must establish the rejection path and whether replay is safe;
/// this predicate alone never authorizes a retry.
pub(crate) fn is_image_size_diagnostic(code: Option<&str>, message: &str) -> bool {
    if matches!(code, Some("image_too_large" | "image_size_exceeded")) {
        return true;
    }
    // OpenAI Responses appends structured diagnostic details for observation.
    // Extra fields (including echoed input) are not the error message itself.
    let message = message
        .split(" | diagnostic=")
        .next()
        .unwrap_or(message)
        .to_ascii_lowercase();
    message.contains("image")
        && ((message.contains("patches")
            && message.contains("exceed")
            && message.contains("limit"))
            || message.contains("image is too large")
            || message.contains("image too large")
            || (message.contains("image dimensions") && message.contains("exceed")))
}

/// Determine whether a failure is transient and eligible for retry.
///
/// Eligible failures:
/// - `Api { status }` with 408 / 425 / 429 / 500 / 502 / 503 / 504 / 529.
/// - `Http(reqwest::Error)` with `is_connect()` or `is_timeout()`.
/// - Lifecycle hard timeouts from `Timeout { .. }`.
///
/// Other failures (Json, Sse, Config, and other Api statuses) return false.
/// Callers propagate failures after SSE reading begins as `Sse`, so excluding
/// them here automatically prevents retries.
pub fn is_retryable(error: &ClientError) -> bool {
    match error {
        ClientError::Api {
            status: Some(code), ..
        } => matches!(*code, 408 | 425 | 429 | 500 | 502 | 503 | 504 | 529),
        ClientError::Api { status: None, .. } => false,
        ClientError::Timeout { .. } => true,
        ClientError::Http(e) => e.is_connect() || e.is_timeout(),
        ClientError::ContextWindowExceeded
        | ClientError::Json(_)
        | ClientError::Sse(_)
        | ClientError::Config(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_err(status: Option<u16>) -> ClientError {
        ClientError::Api {
            status,
            code: None,
            message: String::new(),
            retry_after: None,
        }
    }

    #[test]
    fn retryable_status_codes() {
        for code in [408u16, 425, 429, 500, 502, 503, 504, 529] {
            assert!(
                is_retryable(&api_err(Some(code))),
                "status {code} should be retryable",
            );
        }
    }

    #[test]
    fn non_retryable_status_codes() {
        for code in [400u16, 401, 403, 404, 409, 410, 422, 501] {
            assert!(
                !is_retryable(&api_err(Some(code))),
                "status {code} should not be retryable",
            );
        }
    }

    #[test]
    fn api_without_status_not_retryable() {
        assert!(!is_retryable(&api_err(None)));
    }

    #[test]
    fn lifecycle_timeout_is_retryable() {
        assert!(is_retryable(&ClientError::Timeout {
            phase: "stream_open",
            timeout: Duration::from_secs(30),
        }));
    }

    #[test]
    fn json_sse_config_not_retryable() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        assert!(!is_retryable(&ClientError::Json(json_err)));
        assert!(!is_retryable(&ClientError::Sse("boom".into())));
        assert!(!is_retryable(&ClientError::Config("boom".into())));
    }
}
