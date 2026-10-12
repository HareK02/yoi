//! Internal error types for the Codex OAuth submodule.
//!
//! Convert to `ClientError` using `to_client_error` at the `LlmClient`
//! boundary. `Permanent` errors (invalidated refresh_token) include
//! a message instructing the user to run `codex login` again.

use std::path::PathBuf;

use crate::llm_client::ClientError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodexAuthError {
    #[error(
        "not logged in to ChatGPT: {0} not found. Run `codex login` and ensure cli_auth_credentials_store = \"file\"."
    )]
    NotLoggedIn(PathBuf),

    #[error("malformed ~/.codex/auth.json: {0}")]
    MalformedAuthJson(String),

    #[error("io error: {0}")]
    Io(String),

    #[error("token refresh failed (transient): {0}")]
    RefreshTransient(String),

    /// refresh_token is permanently invalidated. Logging in again is required.
    #[error("token refresh failed permanently ({reason:?}): {message}")]
    RefreshPermanent {
        reason: PermanentReason,
        message: String,
    },

    #[error("invalid header value: {0}")]
    InvalidHeader(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermanentReason {
    Expired,
    Reused,
    Revoked,
    Other,
}

impl CodexAuthError {
    /// Convert for the `LlmClient` transport boundary.
    pub fn to_client_error(self) -> ClientError {
        match self {
            CodexAuthError::NotLoggedIn(_)
            | CodexAuthError::MalformedAuthJson(_)
            | CodexAuthError::Io(_)
            | CodexAuthError::InvalidHeader(_) => ClientError::Config(self.to_string()),
            CodexAuthError::RefreshTransient(msg) => ClientError::Api {
                status: None,
                code: Some("refresh_transient".into()),
                message: msg,
                retry_after: None,
            },
            CodexAuthError::RefreshPermanent { reason, message } => ClientError::Api {
                status: Some(401),
                code: Some(match reason {
                    PermanentReason::Expired => "refresh_token_expired".into(),
                    PermanentReason::Reused => "refresh_token_reused".into(),
                    PermanentReason::Revoked => "refresh_token_invalidated".into(),
                    PermanentReason::Other => "refresh_token_failed".into(),
                }),
                message: format!("{message}. Please run `codex login` again."),
                retry_after: None,
            },
        }
    }
}
