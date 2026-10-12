//! HTTP calls to refresh ChatGPT OAuth tokens.
//!
//! Uses the same `POST https://auth.openai.com/oauth/token` format as Codex CLI.
//! Classifies permanent failures using 401 + `error.code`.

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::error::{CodexAuthError, PermanentReason};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";
/// Limits cover send, both response bodies, and decode, not auth file I/O.
/// OAuth responses contain only a few tokens; 64 KiB leaves ample JWT headroom.
#[derive(Clone)]
pub(super) struct RefreshLimits {
    pub timeout: Duration,
    pub max_body_bytes: usize,
    #[cfg(test)]
    pub body_started: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl Default for RefreshLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_body_bytes: 64 * 1024,
            #[cfg(test)]
            body_started: None,
        }
    }
}

#[derive(Serialize)]
struct RefreshRequest<'a> {
    client_id: &'static str,
    grant_type: &'static str,
    refresh_token: &'a str,
}

#[derive(Deserialize, Debug, Default, Clone)]
pub struct RefreshResponse {
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// Obtain new tokens using refresh_token.
///
/// Permanent failures (401 + `refresh_token_(expired|reused|invalidated)`) return
/// `RefreshPermanent`; all others return `RefreshTransient`.
pub async fn request_refresh(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
    limits: &RefreshLimits,
) -> Result<RefreshResponse, CodexAuthError> {
    let deadline = tokio::time::Instant::now() + limits.timeout;
    let result = tokio::time::timeout_at(
        deadline,
        request_refresh_bounded(client, endpoint, refresh_token, limits),
    )
    .await;
    // JSON decode is synchronous. The byte bound limits its work, and this
    // post-check prevents a decode that crosses the deadline from being persisted.
    if tokio::time::Instant::now() >= deadline {
        return Err(CodexAuthError::RefreshTransient(
            "oauth_token_refresh timed out".into(),
        ));
    }
    result.map_err(|_| CodexAuthError::RefreshTransient("oauth_token_refresh timed out".into()))?
}

async fn request_refresh_bounded(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
    limits: &RefreshLimits,
) -> Result<RefreshResponse, CodexAuthError> {
    let body = RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
    };
    let mut response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|_| CodexAuthError::RefreshTransient("send failed".into()))?;

    let status = response.status();
    if response
        .content_length()
        .is_some_and(|len| len > limits.max_body_bytes as u64)
    {
        return Err(body_too_large());
    }
    #[cfg(test)]
    if let Some(started) = &limits.body_started {
        started.notify_one();
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CodexAuthError::RefreshTransient("read response body failed".into()))?
    {
        // Content-Length can be absent or untrustworthy. Bound actual bytes too,
        // before copying a chunk into the accumulated body.
        if chunk.len() > limits.max_body_bytes.saturating_sub(bytes.len()) {
            return Err(body_too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    if status.is_success() {
        let tokens: RefreshResponse = serde_json::from_slice(&bytes).map_err(|_| {
            // Do not expose untrusted body values (which can contain secrets).
            CodexAuthError::RefreshTransient("decode response failed".into())
        })?;
        if tokens.access_token.as_deref().is_none_or(str::is_empty)
            || tokens.id_token.as_deref().is_some_and(str::is_empty)
            || tokens.refresh_token.as_deref().is_some_and(str::is_empty)
        {
            return Err(CodexAuthError::RefreshTransient(
                "incomplete token response".into(),
            ));
        }
        Ok(tokens)
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        let (reason, message) = classify_permanent(&String::from_utf8_lossy(&bytes));
        Err(CodexAuthError::RefreshPermanent { reason, message })
    } else {
        // Error bodies are consumed within the same limits but never echoed.
        Err(CodexAuthError::RefreshTransient(format!(
            "refresh endpoint returned {status}"
        )))
    }
}

fn body_too_large() -> CodexAuthError {
    CodexAuthError::RefreshTransient("response body exceeds size limit".into())
}

fn classify_permanent(body: &str) -> (PermanentReason, String) {
    let code = extract_error_code(body);
    let reason = match code.as_deref() {
        Some("refresh_token_expired") => PermanentReason::Expired,
        Some("refresh_token_reused") => PermanentReason::Reused,
        Some("refresh_token_invalidated") => PermanentReason::Revoked,
        _ => PermanentReason::Other,
    };
    let message = match reason {
        PermanentReason::Expired => "Your refresh token has expired".to_string(),
        PermanentReason::Reused => "Your refresh token was already used".to_string(),
        PermanentReason::Revoked => "Your refresh token was revoked".to_string(),
        PermanentReason::Other => "Unknown 401 from refresh endpoint".to_string(),
    };
    (reason, message)
}

fn extract_error_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    if let Some(error) = value.get("error") {
        if let Some(obj) = error.as_object() {
            if let Some(code) = obj.get("code").and_then(|v| v.as_str()) {
                return Some(code.to_string());
            }
        }
        if let Some(s) = error.as_str() {
            return Some(s.to_string());
        }
    }
    value
        .get("code")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_expired() {
        let body = r#"{"error":{"code":"refresh_token_expired"}}"#;
        let (r, _) = classify_permanent(body);
        assert_eq!(r, PermanentReason::Expired);
    }

    #[test]
    fn classify_reused() {
        let body = r#"{"error":{"code":"refresh_token_reused"}}"#;
        let (r, _) = classify_permanent(body);
        assert_eq!(r, PermanentReason::Reused);
    }

    #[test]
    fn classify_unknown_falls_to_other() {
        let body = r#"{"error":{"code":"weird"}}"#;
        let (r, _) = classify_permanent(body);
        assert_eq!(r, PermanentReason::Other);
    }

    #[test]
    fn classify_top_level_code() {
        let body = r#"{"code":"refresh_token_invalidated"}"#;
        let (r, _) = classify_permanent(body);
        assert_eq!(r, PermanentReason::Revoked);
    }
}
