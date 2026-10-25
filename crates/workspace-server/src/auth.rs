//! Resolves Backend request actors from human credentials.
//!
//! User identifies the human and Account the Workspace owner; request actors retain both.
//! The store resolves valid API tokens or browser sessions, checking expiry and User existence.
//! Tokens are matched by hash; session cookies use HttpOnly/SameSite.
//! This resolution does not authorize individual Workspace resources. Server admission points
//! and domain services check ownership, Origin, and operation permissions. worker_source
//! separately verifies Runtime signing proofs.

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
    unique_cookie(headers, cookie_name)
        .ok()
        .flatten()
        .filter(|value| !value.is_empty())
}

// Cookie has no path/domain metadata on requests. Duplicates (including identical values
// and multiple Cookie headers) cannot be disambiguated safely after cookie tossing.
fn unique_cookie(
    headers: &HeaderMap,
    cookie_name: &str,
) -> std::result::Result<Option<String>, ()> {
    let mut found = None;
    for header in headers.get_all(header::COOKIE) {
        let header = header.to_str().map_err(|_| ())?;
        for pair in header.split(';') {
            let pair = pair.trim();
            let Some((name, value)) = pair.split_once('=') else {
                if pair == cookie_name {
                    return Err(());
                }
                continue;
            };
            if name == cookie_name {
                if found.is_some() {
                    return Err(());
                }
                found = Some(value.to_owned());
            }
        }
    }
    Ok(found)
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
pub(crate) fn resolve_request_actor_sync(
    store: &dyn ControlPlaneStore,
    bearer: Option<String>,
    session: Option<String>,
) -> Result<Option<RequestActor>> {
    Ok(resolve_credentials_sync(store, bearer, session)?.map(|(actor, _)| actor))
}

fn resolve_credentials_sync(
    store: &dyn ControlPlaneStore,
    bearer: Option<String>,
    session: Option<String>,
) -> Result<Option<(RequestActor, Option<String>)>> {
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
            return Ok(
                actor_for_user(store, &api_token.user_id, ActorAuthMethod::ApiToken)?
                    .map(|actor| (actor, None)),
            );
        }
    }

    if let Some(session_token) = session {
        if let Some(session) = store.resolve_browser_session(&token_hash(&session_token))? {
            if is_expired(&session.expires_at) {
                return Ok(None);
            }
            return Ok(
                actor_for_user(store, &session.user_id, ActorAuthMethod::BrowserSession)?
                    .map(|actor| (actor, Some(session.expires_at))),
            );
        }
    }

    Ok(None)
}

// This marker is a non-authenticating reference to the existing non-revoked session row.
// It outlives the auth cookie so Max-Age expiry remains distinguishable from a new browser.
// No second session store is introduced; clearing all cookies necessarily loses this evidence.
// Keep the established 14-day authentication lifetime. The additional week is only a bounded
// return-navigation window (including a week away); it never extends authentication. The store
// reclaims expired rows past this same boundary on subsequent session issuance. Neither whoami
// nor other reads refresh the marker or the row's absolute expiry.
pub const BROWSER_SESSION_SECONDS: i64 = 14 * 24 * 60 * 60;
pub const SESSION_EXPIRY_GRACE_SECONDS: i64 = 7 * 24 * 60 * 60;

pub fn session_marker_cookie_name(cookie_name: &str) -> String {
    format!("{cookie_name}_expiry")
}

pub async fn resolve_whoami(
    store: std::sync::Arc<dyn ControlPlaneStore>,
    headers: &HeaderMap,
    cookie_name: &str,
) -> Result<server_api::WhoamiResponse> {
    let bearer = parse_bearer(headers);
    // Even an empty/invalid auth cookie takes precedence over an older marker.
    // An ambiguous primary is invalid, not absent; never fall back to an older marker.
    let session = unique_cookie(headers, cookie_name).unwrap_or_else(|()| Some(String::new()));
    let marker = parse_cookie(headers, &session_marker_cookie_name(cookie_name));
    let executor = store.db_executor();
    executor
        .run(move || resolve_whoami_sync(store.as_ref(), bearer, session, marker))
        .await
}

fn resolve_whoami_sync(
    store: &dyn ControlPlaneStore,
    bearer: Option<String>,
    session_token: Option<String>,
    marker: Option<String>,
) -> Result<server_api::WhoamiResponse> {
    use server_api::{BrowserSessionState, WhoamiResponse};
    let anonymous = || WhoamiResponse {
        actor: None,
        authentication_reference: None,
        session: BrowserSessionState::Anonymous {},
    };
    // Preserve the existing API-token precedence and fallback rules, including expired tokens.
    if let Some((actor, expires_at)) =
        resolve_credentials_sync(store, bearer.clone(), session_token.clone())?
    {
        let credential = match actor.auth_method {
            ActorAuthMethod::ApiToken => bearer.as_deref(),
            ActorAuthMethod::BrowserSession => session_token.as_deref(),
        }
        .ok_or_else(|| auth_error("auth_required", "authentication binding missing"))?;
        let authentication_reference = Some(authentication_reference(&token_hash(credential)));
        return Ok(WhoamiResponse {
            authentication_reference,
            actor: Some(actor),
            session: BrowserSessionState::Authenticated { expires_at },
        });
    }
    let hash = match session_token {
        Some(token) => token_hash(&token),
        None => match marker {
            Some(hash) => hash,
            None => return Ok(anonymous()),
        },
    };
    let Some(session) = store.resolve_browser_session(&hash)? else {
        return Ok(anonymous());
    };
    if store.get_user(&session.user_id)?.is_none() {
        return Ok(anonymous());
    }
    if within_session_expiry_grace(&session.expires_at, Utc::now()) {
        return Ok(WhoamiResponse {
            actor: None,
            authentication_reference: None,
            session: BrowserSessionState::Expired {},
        });
    }
    // A marker for a still-valid session cannot restore authentication after partial deletion.
    Ok(anonymous())
}

/// Domain-separated public comparison value; never accepted as a credential.
pub(crate) fn authentication_reference(credential_hash: &str) -> String {
    token_hash(&format!(
        "passkey-authentication-reference:{credential_hash}"
    ))
}

fn within_session_expiry_grace(expires_at: &str, now: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(expires_at).is_ok_and(|expires_at| {
        expires_at <= now && now < expires_at + Duration::seconds(SESSION_EXPIRY_GRACE_SECONDS)
    })
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
            SqliteWorkspaceStore::in_memory_fixture()
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
        let whoami = resolve_whoami(store.clone(), &headers, "session")
            .await
            .unwrap();
        assert_eq!(
            whoami.session,
            server_api::BrowserSessionState::Authenticated { expires_at: None }
        );
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
        let actor_result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            resolve_request_actor(store.clone(), &headers, "session"),
        )
        .await;
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            resolve_whoami(store, &headers, "session"),
        )
        .await;
        release.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        queued.await.unwrap();
        assert!(matches!(
            actor_result.expect("actor lookup rejects capacity"),
            Err(Error::DbCapacityExceeded)
        ));
        assert!(matches!(
            result.expect("capacity rejection must not wait for queued SQL"),
            Err(Error::DbCapacityExceeded)
        ));
    }

    #[tokio::test]
    async fn whoami_classifies_only_live_store_backed_session_expiry() {
        use server_api::BrowserSessionState;
        let store = seeded_store();
        for (secret, offset, revoked) in [
            ("recent", -60, false),
            ("old", -SESSION_EXPIRY_GRACE_SECONDS - 60, false),
            ("revoked", -60, true),
        ] {
            store
                .create_browser_session(&BrowserSessionRecord {
                    session_id: format!("session-{secret}"),
                    token_hash: token_hash(secret),
                    user_id: "user-auth".into(),
                    created_at: "1".into(),
                    expires_at: rfc3339_after(Duration::seconds(offset)),
                    revoked_at: revoked.then(|| now_rfc3339()),
                })
                .unwrap();
        }
        let cases = [
            (String::new(), BrowserSessionState::Anonymous {}),
            ("session=unknown".into(), BrowserSessionState::Anonymous {}),
            ("session=recent".into(), BrowserSessionState::Expired {}),
            (
                format!("session_expiry={}", token_hash("recent")),
                BrowserSessionState::Expired {},
            ),
            (
                format!("session_expiry={}", token_hash("session-secret")),
                BrowserSessionState::Anonymous {},
            ),
            (
                "session_expiry=unknown".into(),
                BrowserSessionState::Anonymous {},
            ),
            (
                "session_expiry=recent".into(),
                BrowserSessionState::Anonymous {},
            ),
            (
                format!("session=unknown; session_expiry={}", token_hash("recent")),
                BrowserSessionState::Anonymous {},
            ),
            (
                format!("session=; session_expiry={}", token_hash("recent")),
                BrowserSessionState::Anonymous {},
            ),
            ("session=revoked".into(), BrowserSessionState::Anonymous {}),
            (
                format!("session_expiry={}", token_hash("revoked")),
                BrowserSessionState::Anonymous {},
            ),
            ("session=old".into(), BrowserSessionState::Anonymous {}),
            (
                format!("session_expiry={}", token_hash("old")),
                BrowserSessionState::Anonymous {},
            ),
            (
                format!(
                    "session=session-secret; session_expiry={}",
                    token_hash("recent")
                ),
                BrowserSessionState::Authenticated {
                    expires_at: Some("9999-01-01T00:00:00Z".into()),
                },
            ),
        ];
        for (cookie, expected) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(header::COOKIE, cookie.parse().unwrap());
            let response = resolve_whoami(store.clone(), &headers, "session")
                .await
                .unwrap();
            assert_eq!(response.session, expected, "cookie: {cookie}");
            assert_eq!(
                response.actor.is_some(),
                matches!(expected, BrowserSessionState::Authenticated { .. })
            );
            if !matches!(expected, BrowserSessionState::Authenticated { .. }) {
                assert!(
                    resolve_request_actor(store.clone(), &headers, "session")
                        .await
                        .unwrap()
                        .is_none()
                );
            }
        }
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

    #[test]
    fn expiry_grace_includes_expiry_but_excludes_its_absolute_end() {
        let expiry = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for (offset, expected) in [
            (-1, false),
            (0, true),
            (SESSION_EXPIRY_GRACE_SECONDS - 1, true),
            (SESSION_EXPIRY_GRACE_SECONDS, false),
        ] {
            assert_eq!(
                within_session_expiry_grace(
                    &expiry.to_rfc3339(),
                    expiry + Duration::seconds(offset)
                ),
                expected
            );
        }
        assert!(!within_session_expiry_grace("invalid", expiry));
    }

    #[tokio::test]
    async fn duplicate_cookies_never_select_an_actor_or_promote_an_ambiguous_marker() {
        use server_api::BrowserSessionState;
        let store = seeded_store();
        let expired_hash = token_hash("expired-duplicate");
        store
            .create_browser_session(&BrowserSessionRecord {
                session_id: "expired-duplicate".into(),
                token_hash: expired_hash.clone(),
                user_id: "user-auth".into(),
                created_at: now_rfc3339(),
                expires_at: rfc3339_after(Duration::seconds(-60)),
                revoked_at: None,
            })
            .unwrap();
        for cookies in [
            vec!["session=session-secret; session=unknown".to_owned()],
            vec!["session=session-secret; session=session-secret".to_owned()],
            vec![
                "session=session-secret".to_owned(),
                "session=unknown".to_owned(),
            ],
            vec![format!(
                "session=; session=session-secret; session_expiry={expired_hash}"
            )],
            vec![format!(
                "session_expiry={expired_hash}; session_expiry={expired_hash}"
            )],
            vec![format!("session; session_expiry={expired_hash}")],
        ] {
            let mut headers = HeaderMap::new();
            for cookie in cookies {
                headers.append(header::COOKIE, cookie.parse().unwrap());
            }
            assert!(
                resolve_request_actor(store.clone(), &headers, "session")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                resolve_whoami(store.clone(), &headers, "session")
                    .await
                    .unwrap()
                    .session,
                BrowserSessionState::Anonymous {}
            );
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            format!("session=session-secret; session_expiry={expired_hash}")
                .parse()
                .unwrap(),
        );
        assert!(matches!(
            resolve_whoami(store, &headers, "session")
                .await
                .unwrap()
                .session,
            BrowserSessionState::Authenticated { .. }
        ));
    }

    #[test]
    fn session_issuance_reclaims_only_rows_beyond_recovery_grace() {
        let store = seeded_store();
        let record = |secret: &str, offset| BrowserSessionRecord {
            session_id: secret.into(),
            token_hash: token_hash(secret),
            user_id: "user-auth".into(),
            created_at: now_rfc3339(),
            expires_at: rfc3339_after(Duration::seconds(offset)),
            revoked_at: None,
        };
        store
            .create_browser_session(&record("past-grace", -SESSION_EXPIRY_GRACE_SECONDS - 60))
            .unwrap();
        assert!(
            store
                .resolve_browser_session(&token_hash("past-grace"))
                .unwrap()
                .is_some()
        );
        store
            .create_browser_session(&record("within-grace", -SESSION_EXPIRY_GRACE_SECONDS + 60))
            .unwrap();
        assert!(
            store
                .resolve_browser_session(&token_hash("past-grace"))
                .unwrap()
                .is_none()
        );
        store
            .create_browser_session(&record("new-session", BROWSER_SESSION_SECONDS))
            .unwrap();
        assert!(
            store
                .resolve_browser_session(&token_hash("within-grace"))
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .resolve_browser_session(&token_hash("new-session"))
                .unwrap()
                .is_some()
        );
    }
}
