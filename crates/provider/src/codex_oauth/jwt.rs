//! JWT payload の最小限のパース（署名検証なし）。
//!
//! Codex CLI と同じく、access_token / id_token の payload を base64url
//! デコードして `exp` や ChatGPT 固有 claims を取り出すためだけに使う。

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;

/// `Authorization: Bearer` で送る access_token JWT の `exp` を読む。
pub fn parse_exp(jwt: &str) -> Option<DateTime<Utc>> {
    #[derive(Deserialize)]
    struct Claims {
        exp: Option<i64>,
    }
    let claims: Claims = decode_payload(jwt).ok()?;
    DateTime::<Utc>::from_timestamp(claims.exp?, 0)
}

/// id_token JWT から ChatGPT 固有 claims を取り出す。
pub fn parse_chatgpt_claims(jwt: &str) -> Option<ChatGptClaims> {
    #[derive(Deserialize)]
    struct IdClaims {
        #[serde(rename = "https://api.openai.com/auth", default)]
        auth: Option<AuthClaims>,
    }
    #[derive(Deserialize)]
    struct AuthClaims {
        #[serde(default)]
        chatgpt_account_id: Option<String>,
        #[serde(default)]
        chatgpt_account_is_fedramp: bool,
    }
    let claims: IdClaims = decode_payload(jwt).ok()?;
    let auth = claims.auth?;
    Some(ChatGptClaims {
        account_id: auth.chatgpt_account_id,
        is_fedramp: auth.chatgpt_account_is_fedramp,
    })
}

#[derive(Debug, Clone)]
pub struct ChatGptClaims {
    pub account_id: Option<String>,
    pub is_fedramp: bool,
}

fn decode_payload<T: for<'de> Deserialize<'de>>(jwt: &str) -> Result<T, JwtError> {
    let payload = jwt.split('.').nth(1).ok_or(JwtError::InvalidFormat)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| JwtError::InvalidBase64)?;
    serde_json::from_slice(&bytes).map_err(|_| JwtError::InvalidJson)
}

#[derive(Debug)]
enum JwtError {
    InvalidFormat,
    InvalidBase64,
    InvalidJson,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_jwt(payload: &serde_json::Value) -> String {
        let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(payload).unwrap());
        format!("h.{payload_b64}.s")
    }

    #[test]
    fn parses_exp() {
        let jwt = make_jwt(&serde_json::json!({ "exp": 1_700_000_000_i64 }));
        let dt = parse_exp(&jwt).unwrap();
        assert_eq!(dt.timestamp(), 1_700_000_000);
    }

    #[test]
    fn parses_chatgpt_claims() {
        let jwt = make_jwt(&serde_json::json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-123",
                "chatgpt_account_is_fedramp": true,
            }
        }));
        let c = parse_chatgpt_claims(&jwt).unwrap();
        assert_eq!(c.account_id.as_deref(), Some("acc-123"));
        assert!(c.is_fedramp);
    }

    #[test]
    fn missing_exp_returns_none() {
        let jwt = make_jwt(&serde_json::json!({}));
        assert!(parse_exp(&jwt).is_none());
    }

    #[test]
    fn malformed_jwt_returns_none() {
        assert!(parse_exp("not-a-jwt").is_none());
        assert!(parse_chatgpt_claims("x.y").is_none());
    }
}
