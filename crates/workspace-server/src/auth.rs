use axum::http::{HeaderMap, header};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Error, Result, store::ControlPlaneStore};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthPublicConfig {
    pub rp_id: String,
    pub origin: String,
    pub public_base_url: String,
    pub cookie_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequestActor {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
    pub auth_method: ActorAuthMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActorAuthMethod {
    BrowserSession,
    ApiToken,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
}

impl RequestActor {
    pub fn user(&self) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: self.user_id.clone(),
            account_id: self.account_id.clone(),
            handle: self.handle.clone(),
            display_name: self.display_name.clone(),
        }
    }
}

pub fn normalize_handle(handle: &str) -> Result<String> {
    let normalized = handle.trim().to_ascii_lowercase();
    let valid = !normalized.is_empty()
        && normalized.len() <= 64
        && normalized
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !valid {
        return Err(auth_error(
            "invalid_handle",
            "handle must be 1-64 chars of ascii lowercase letters, digits, '-' or '_'",
        ));
    }
    Ok(normalized)
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn rfc3339_after(duration: Duration) -> String {
    (Utc::now() + duration).to_rfc3339()
}

pub fn is_expired(expires_at: &str) -> bool {
    match DateTime::parse_from_rfc3339(expires_at) {
        Ok(expires_at) => expires_at.with_timezone(&Utc) <= Utc::now(),
        Err(_) => true,
    }
}

pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::now_v7())
}

pub fn new_challenge() -> String {
    format!("chal-{}-{}", Uuid::now_v7(), Uuid::now_v7())
}

pub fn mint_secret(prefix: &str) -> String {
    format!("{prefix}_{}_{}", Uuid::now_v7(), Uuid::now_v7())
}

pub fn token_hash(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(digest.len() * 2 + "sha256:".len());
    encoded.push_str("sha256:");
    for byte in digest {
        encoded.push_str(&format!("{byte:02x}"));
    }
    encoded
}

pub fn new_user_code() -> String {
    let hex = Uuid::now_v7().simple().to_string().to_ascii_uppercase();
    format!("{}-{}", &hex[0..4], &hex[4..8])
}

pub fn parse_bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub fn parse_cookie(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == cookie_name && !value.is_empty()).then(|| value.to_string())
    })
}

pub async fn resolve_request_actor<S: ControlPlaneStore + ?Sized>(
    store: &S,
    headers: &HeaderMap,
    cookie_name: &str,
) -> Result<Option<RequestActor>> {
    if let Some(token) = parse_bearer(headers) {
        let token_hash = token_hash(&token);
        if let Some(api_token) = store.resolve_api_token(&token_hash)? {
            if api_token
                .expires_at
                .as_deref()
                .is_some_and(|expires_at| is_expired(expires_at))
            {
                return Ok(None);
            }
            store.mark_api_token_used(&token_hash, &now_rfc3339())?;
            return actor_for_user(store, &api_token.user_id, ActorAuthMethod::ApiToken);
        }
    }

    if let Some(session_token) = parse_cookie(headers, cookie_name) {
        if let Some(session) = store.resolve_browser_session(&token_hash(&session_token))? {
            if is_expired(&session.expires_at) {
                return Ok(None);
            }
            return actor_for_user(store, &session.user_id, ActorAuthMethod::BrowserSession);
        }
    }

    Ok(None)
}

fn actor_for_user<S: ControlPlaneStore + ?Sized>(
    store: &S,
    user_id: &str,
    auth_method: ActorAuthMethod,
) -> Result<Option<RequestActor>> {
    let Some(user) = store.get_user(user_id)? else {
        return Ok(None);
    };
    Ok(Some(RequestActor {
        user_id: user.user_id,
        account_id: user.account_id,
        handle: user.handle,
        display_name: user.display_name,
        auth_method,
    }))
}

pub fn session_set_cookie(cookie_name: &str, token: &str, max_age_seconds: i64) -> String {
    format!("{cookie_name}={token}; Max-Age={max_age_seconds}; Path=/; HttpOnly; SameSite=Lax")
}

pub fn auth_error(code: &str, message: &str) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-auth".to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
}
