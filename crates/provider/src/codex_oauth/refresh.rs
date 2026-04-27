//! ChatGPT OAuth トークンの refresh HTTP 呼出。
//!
//! Codex CLI と同じ `POST https://auth.openai.com/oauth/token` 形式。
//! 401 + `error.code` で永続失敗を分類する。

use serde::{Deserialize, Serialize};

use super::error::{CodexAuthError, PermanentReason};

pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const REFRESH_URL: &str = "https://auth.openai.com/oauth/token";

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

/// refresh_token を使って新しいトークン群を取得する。
///
/// 永続失敗（401 + `refresh_token_(expired|reused|invalidated)`）は
/// `RefreshPermanent`、それ以外は `RefreshTransient`。
pub async fn request_refresh(
    client: &reqwest::Client,
    endpoint: &str,
    refresh_token: &str,
) -> Result<RefreshResponse, CodexAuthError> {
    let body = RefreshRequest {
        client_id: CLIENT_ID,
        grant_type: "refresh_token",
        refresh_token,
    };
    let response = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| CodexAuthError::RefreshTransient(format!("send: {e}")))?;

    let status = response.status();
    if status.is_success() {
        response
            .json::<RefreshResponse>()
            .await
            .map_err(|e| CodexAuthError::RefreshTransient(format!("parse response: {e}")))
    } else {
        let body = response.text().await.unwrap_or_default();
        if status == reqwest::StatusCode::UNAUTHORIZED {
            let (reason, message) = classify_permanent(&body);
            Err(CodexAuthError::RefreshPermanent { reason, message })
        } else {
            Err(CodexAuthError::RefreshTransient(format!(
                "{status}: {body}"
            )))
        }
    }
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
        PermanentReason::Other => format!("Unknown 401 from refresh endpoint: {body}"),
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
