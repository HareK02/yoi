use axum::http::{HeaderMap, header};
use chrono::{DateTime, Duration, Utc};
pub use server_api::{ActorAuthMethod, AuthPublicConfig, AuthenticatedUser, RequestActor};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{Error, Result, store::ControlPlaneStore};

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
    user_code_from_uuid(Uuid::now_v7())
}

fn user_code_from_uuid(id: Uuid) -> String {
    let hex = id.simple().to_string().to_ascii_uppercase();
    let random_tail = &hex[24..32];
    format!("{}-{}", &random_tail[..4], &random_tail[4..])
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

pub async fn resolve_request_actor(
    store: std::sync::Arc<dyn ControlPlaneStore>,
    headers: &HeaderMap,
    cookie_name: &str,
) -> Result<Option<RequestActor>> {
    let bearer = parse_bearer(headers);
    let session = parse_cookie(headers, cookie_name);
    if bearer.is_none() && session.is_none() {
        return Ok(None);
    }
    let executor = store.db_executor();
    executor
        .run(move || resolve_request_actor_sync(store.as_ref(), bearer, session))
        .await
}

// One owned synchronous stage includes token usage marking and the user read.
// Do not call async store wrappers here: they use the same executor themselves.
fn resolve_request_actor_sync(
    store: &dyn ControlPlaneStore,
    bearer: Option<String>,
    session: Option<String>,
) -> Result<Option<RequestActor>> {
    if let Some(token) = bearer {
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

    if let Some(session_token) = session {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionCookiePolicy<'a> {
    pub cookie_name: &'a str,
    pub path: &'a str,
    pub domain: Option<&'a str>,
    pub secure: bool,
}

pub fn session_set_cookie(
    policy: SessionCookiePolicy<'_>,
    token: &str,
    max_age_seconds: i64,
) -> String {
    let domain = policy
        .domain
        .map(|domain| format!("; Domain={domain}"))
        .unwrap_or_default();
    let secure = if policy.secure { "; Secure" } else { "" };
    format!(
        "{}={token}; Max-Age={max_age_seconds}; Path={}; HttpOnly; SameSite=Lax{domain}{secure}",
        policy.cookie_name, policy.path
    )
}

pub fn auth_error(code: &str, message: &str) -> Error {
    Error::RuntimeOperationFailed {
        runtime_id: "workspace-auth".to_string(),
        code: code.to_string(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        AccountRecord, ApiTokenRecord, BrowserSessionRecord, SqliteWorkspaceStore, UserRecord,
    };
    use std::sync::Arc;

    fn seeded_store() -> Arc<SqliteWorkspaceStore> {
        let executor = crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
            workers: std::num::NonZeroUsize::new(1).unwrap(),
            queued: std::num::NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        let store = Arc::new(
            SqliteWorkspaceStore::in_memory_for_test()
                .unwrap()
                .with_db_executor(executor),
        );
        store
            .upsert_account(&AccountRecord {
                account_id: "account-auth".into(),
                kind: "user".into(),
                handle: "auth".into(),
                display_name: "Auth".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        store
            .upsert_user(&UserRecord {
                user_id: "user-auth".into(),
                account_id: "account-auth".into(),
                handle: "auth".into(),
                display_name: "Auth".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        store
            .create_browser_session(&BrowserSessionRecord {
                session_id: "session-auth".into(),
                token_hash: token_hash("session-secret"),
                user_id: "user-auth".into(),
                created_at: "1".into(),
                expires_at: "9999-01-01T00:00:00Z".into(),
                revoked_at: None,
            })
            .unwrap();
        store
    }

    #[tokio::test]
    async fn api_token_auth_marks_usage_and_takes_precedence_over_cookie() {
        let store = seeded_store();
        let hash = token_hash("api-secret");
        store
            .create_api_token(&ApiTokenRecord {
                token_id: "token-auth".into(),
                token_hash: hash.clone(),
                user_id: "user-auth".into(),
                label: "Auth".into(),
                created_at: "1".into(),
                expires_at: None,
                revoked_at: None,
                last_used_at: None,
            })
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer api-secret".parse().unwrap());
        headers.insert(header::COOKIE, "session=session-secret".parse().unwrap());
        let actor = resolve_request_actor(store.clone(), &headers, "session")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actor.user_id, "user-auth");
        assert_eq!(actor.auth_method, ActorAuthMethod::ApiToken);
        assert!(
            store
                .resolve_api_token(&hash)
                .unwrap()
                .unwrap()
                .last_used_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn expired_api_token_does_not_fall_back_to_valid_browser_session() {
        let store = seeded_store();
        let hash = token_hash("expired-secret");
        store
            .create_api_token(&ApiTokenRecord {
                token_id: "token-auth".into(),
                token_hash: hash.clone(),
                user_id: "user-auth".into(),
                label: "Auth".into(),
                created_at: "1".into(),
                expires_at: Some("2000-01-01T00:00:00Z".into()),
                revoked_at: None,
                last_used_at: None,
            })
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer expired-secret".parse().unwrap(),
        );
        headers.insert(header::COOKIE, "session=session-secret".parse().unwrap());
        assert!(
            resolve_request_actor(store.clone(), &headers, "session")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .resolve_api_token(&hash)
                .unwrap()
                .unwrap()
                .last_used_at
                .is_none()
        );
    }

    #[tokio::test]
    async fn unknown_bearer_can_fall_back_to_valid_browser_session() {
        let store = seeded_store();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer unknown-secret".parse().unwrap(),
        );
        headers.insert(header::COOKIE, "session=session-secret".parse().unwrap());
        let actor = resolve_request_actor(store, &headers, "session")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actor.user_id, "user-auth");
        assert_eq!(actor.auth_method, ActorAuthMethod::BrowserSession);
    }

    #[tokio::test]
    async fn saturated_actor_lookup_returns_capacity_error_not_anonymous_actor() {
        let store = seeded_store();
        let executor = store.db_executor();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let blocker_executor = executor.clone();
        let blocker = tokio::spawn(async move {
            blocker_executor
                .run(move || -> Result<()> {
                    let _ = started.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        let mut queued = Box::pin(executor.run(|| -> Result<()> { Ok(()) }));
        assert!(futures::poll!(queued.as_mut()).is_pending());
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "session=session-secret".parse().unwrap());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            resolve_request_actor(store, &headers, "session"),
        )
        .await;
        release.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        queued.await.unwrap();
        assert!(matches!(
            result.expect("capacity rejection must not wait for queued SQL"),
            Err(Error::DbCapacityExceeded)
        ));
    }

    #[test]
    fn user_code_uses_the_uuid_random_tail() {
        let id = Uuid::parse_str("01234567-89ab-7cde-8123-456789abcdef").expect("UUID");
        assert_eq!(user_code_from_uuid(id), "89AB-CDEF");
    }

    #[test]
    fn user_code_preserves_the_human_readable_format() {
        let code = new_user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(
            code.chars()
                .enumerate()
                .all(|(index, value)| index == 4 || value.is_ascii_hexdigit())
        );
        assert_eq!(code, code.to_ascii_uppercase());
    }
}
