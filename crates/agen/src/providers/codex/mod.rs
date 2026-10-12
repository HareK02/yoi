//! [`AuthProvider`] implementation that reads ChatGPT OAuth credentials
//! (`~/.codex/auth.json`) and builds `Authorization` /
//! `ChatGPT-Account-Id` / `X-OpenAI-Fedramp` headers.
//!
//! Design:
//!
//! - The HTTP transport sees only the [`AuthProvider`] trait; the concrete
//!   [`CodexAuthProvider`] lives in this optional module
//! - Read the access_token JWT's `exp` and proactively refresh when `exp` <= `now`
//!   (no buffer, as in Codex CLI)
//! - Reload immediately before refreshing and check that account_id matches
//!   to avoid confusing concurrent Codex CLI / other-process refreshes (guarded reload)
//! - No file locking; reload and merge differences before writing
//! - Codex Keyring storage is not supported. If auth.json is missing, return an error with guidance

mod auth_json;
mod error;
mod jwt;
mod refresh;
#[cfg(test)]
mod refresh_tests;

use std::path::PathBuf;
use std::sync::Arc;

use crate::llm_client::{ClientError, auth::AuthProvider};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use reqwest::header::{HeaderName, HeaderValue};
use tokio::sync::Mutex;

use auth_json::AuthSnapshot;
use error::CodexAuthError;

pub use error::PermanentReason;

/// Codex CLI's `last_refresh`-based fallback interval (8 days, matching Codex CLI).
const TOKEN_REFRESH_INTERVAL_DAYS: i64 = 8;

/// Provider that reads `~/.codex/auth.json` and returns Codex-compatible headers.
pub struct CodexAuthProvider {
    auth_path: PathBuf,
    refresh_endpoint: String,
    http: reqwest::Client,
    refresh_limits: refresh::RefreshLimits,
    #[cfg(test)]
    before_guarded_reload: Option<Arc<tokio::sync::Barrier>>,
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for CodexAuthProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodexAuthProvider")
            .field("auth_path", &self.auth_path)
            .field("refresh_endpoint", &self.refresh_endpoint)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
struct State {
    cached: Option<AuthSnapshot>,
}

impl CodexAuthProvider {
    /// Locate auth.json using `CODEX_HOME`, then `$HOME/.codex` as a fallback.
    pub fn from_default_home() -> Result<Self, ClientError> {
        let codex_home = if let Ok(p) = std::env::var("CODEX_HOME") {
            PathBuf::from(p)
        } else {
            let home =
                std::env::var("HOME").map_err(|_| ClientError::Config("HOME not set".into()))?;
            PathBuf::from(home).join(".codex")
        };
        Ok(Self::new(codex_home))
    }

    /// Construct from a custom codex_home.
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            auth_path: codex_home.join("auth.json"),
            refresh_endpoint: refresh::REFRESH_URL.to_string(),
            http: reqwest::Client::new(),
            refresh_limits: refresh::RefreshLimits::default(),
            #[cfg(test)]
            before_guarded_reload: None,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Override the refresh endpoint for tests.
    #[cfg(test)]
    fn with_refresh_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.refresh_endpoint = endpoint.into();
        self
    }

    /// Override the HTTP client for tests.
    #[cfg(test)]
    fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }

    async fn ensure_fresh(&self) -> Result<AuthSnapshot, CodexAuthError> {
        let mut state = self.state.lock().await;

        // 1. Always read the latest from disk (reflect updates by other processes)
        let snap = auth_json::load(&self.auth_path).await?;

        // 2. Return as-is if not stale
        if !is_stale(&snap) {
            state.cached = Some(snap.clone());
            return Ok(snap);
        }

        // 3. Reload immediately before refreshing (guarded reload).
        //    Skip refresh if another process supplied fresh tokens or changed account_id.
        #[cfg(test)]
        if let Some(barrier) = &self.before_guarded_reload {
            barrier.wait().await;
            barrier.wait().await;
        }
        let pre_refresh = auth_json::load(&self.auth_path).await?;
        if !is_stale(&pre_refresh) {
            state.cached = Some(pre_refresh.clone());
            return Ok(pre_refresh);
        }
        if pre_refresh.account_id != snap.account_id {
            // The account has changed → use the new auth without refreshing
            state.cached = Some(pre_refresh.clone());
            return Ok(pre_refresh);
        }

        // 4. Perform the refresh
        let refreshed = refresh::request_refresh(
            &self.http,
            &self.refresh_endpoint,
            &pre_refresh.refresh_token,
            &self.refresh_limits,
        )
        .await?;

        // 5. Write back (persist_refreshed reloads and merges before writing)
        let new_snap = auth_json::persist_refreshed(
            &self.auth_path,
            &pre_refresh,
            refreshed.id_token,
            refreshed.access_token,
            refreshed.refresh_token,
        )
        .await?;
        state.cached = Some(new_snap.clone());
        Ok(new_snap)
    }

    fn build_headers(
        snap: &AuthSnapshot,
    ) -> Result<Vec<(HeaderName, HeaderValue)>, CodexAuthError> {
        let mut out = Vec::with_capacity(5);

        let auth_val = HeaderValue::from_str(&format!("Bearer {}", snap.access_token))
            .map_err(|e| CodexAuthError::InvalidHeader(format!("Authorization: {e}")))?;
        out.push((HeaderName::from_static("authorization"), auth_val));

        let acc_val = HeaderValue::from_str(&snap.account_id)
            .map_err(|e| CodexAuthError::InvalidHeader(format!("ChatGPT-Account-Id: {e}")))?;
        out.push((HeaderName::from_static("chatgpt-account-id"), acc_val));

        // Cloudflare WAF identifies clients accessing the compatible backend by `originator` /
        // `User-Agent`. Without the fixed values sent by Codex CLI,
        // it returns an HTML challenge (403), preventing access to SSE.
        out.push((
            HeaderName::from_static("originator"),
            HeaderValue::from_static("codex_cli_rs"),
        ));
        out.push((
            HeaderName::from_static("user-agent"),
            HeaderValue::from_static("codex_cli_rs/0.60.0"),
        ));

        // Identify FedRAMP organizations using a claim in the id_token JWT
        if jwt::parse_chatgpt_claims(&snap.id_token)
            .map(|c| c.is_fedramp)
            .unwrap_or(false)
        {
            out.push((
                HeaderName::from_static("x-openai-fedramp"),
                HeaderValue::from_static("true"),
            ));
        }

        Ok(out)
    }
}

#[async_trait]
impl AuthProvider for CodexAuthProvider {
    async fn headers(&self) -> Result<Vec<(HeaderName, HeaderValue)>, ClientError> {
        let snap = self
            .ensure_fresh()
            .await
            .map_err(CodexAuthError::to_client_error)?;
        Self::build_headers(&snap).map_err(CodexAuthError::to_client_error)
    }
}

/// Return true if the `access_token` JWT's `exp` indicates expiration.
/// If `exp` cannot be read, use `last_refresh + 8 days` (as in Codex CLI).
fn is_stale(snap: &AuthSnapshot) -> bool {
    if let Some(exp) = jwt::parse_exp(&snap.access_token) {
        return exp <= Utc::now();
    }
    match snap.last_refresh {
        Some(last) => last < Utc::now() - Duration::days(TOKEN_REFRESH_INTERVAL_DAYS),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;
    use wiremock::matchers::{method, path as url_path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(super) fn make_jwt(payload: serde_json::Value) -> String {
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("h.{p}.s")
    }

    pub(super) fn write_auth(
        dir: &std::path::Path,
        exp: i64,
        fedramp: bool,
        refresh: &str,
    ) -> PathBuf {
        let id_token = make_jwt(json!({
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acc-xyz",
                "chatgpt_account_is_fedramp": fedramp,
            }
        }));
        let access_token = make_jwt(json!({ "exp": exp }));
        let auth = json!({
            "tokens": {
                "id_token": id_token,
                "access_token": access_token,
                "refresh_token": refresh,
                "account_id": "acc-xyz",
            },
            "last_refresh": "2026-04-20T00:00:00Z",
        });
        let path = dir.join("auth.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&auth).unwrap()).unwrap();
        path
    }

    #[tokio::test]
    async fn returns_headers_for_fresh_token() {
        let dir = tempfile::tempdir().unwrap();
        // Set expiration far in the future
        write_auth(dir.path(), Utc::now().timestamp() + 3600, false, "rt");
        let provider = CodexAuthProvider::new(dir.path().to_path_buf());

        let headers = provider.headers().await.unwrap();
        let names: Vec<_> = headers
            .iter()
            .map(|(n, _)| n.as_str().to_string())
            .collect();
        assert!(names.contains(&"authorization".to_string()));
        assert!(names.contains(&"chatgpt-account-id".to_string()));
        assert!(!names.contains(&"x-openai-fedramp".to_string()));

        let acc = headers
            .iter()
            .find(|(n, _)| n.as_str() == "chatgpt-account-id")
            .unwrap();
        assert_eq!(acc.1.to_str().unwrap(), "acc-xyz");
    }

    #[tokio::test]
    async fn fedramp_header_added_when_claim_set() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), Utc::now().timestamp() + 3600, true, "rt");
        let provider = CodexAuthProvider::new(dir.path().to_path_buf());

        let headers = provider.headers().await.unwrap();
        let fedramp = headers
            .iter()
            .find(|(n, _)| n.as_str() == "x-openai-fedramp");
        assert!(fedramp.is_some());
        assert_eq!(fedramp.unwrap().1, "true");
    }

    #[tokio::test]
    async fn refreshes_when_expired_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_auth(
            dir.path(),
            Utc::now().timestamp() - 60,
            false,
            "old-refresh",
        );

        // Mock the refresh endpoint to return a new JWT (future exp)
        let server = MockServer::start().await;
        let new_access = make_jwt(json!({ "exp": Utc::now().timestamp() + 3600 }));
        let new_refresh = "new-refresh-token";
        Mock::given(method("POST"))
            .and(url_path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": new_access,
                "refresh_token": new_refresh,
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = CodexAuthProvider::new(dir.path().to_path_buf())
            .with_refresh_endpoint(format!("{}/oauth/token", server.uri()))
            .with_http_client(reqwest::Client::new());

        let headers = provider.headers().await.unwrap();
        let auth = headers
            .iter()
            .find(|(n, _)| n.as_str() == "authorization")
            .unwrap();
        assert_eq!(auth.1.to_str().unwrap(), format!("Bearer {new_access}"));

        // The new refresh_token has been written back to the file
        let on_disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            on_disk["tokens"]["refresh_token"].as_str(),
            Some(new_refresh)
        );
    }

    #[tokio::test]
    async fn permanent_refresh_failure_surfaces_login_message() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(
            dir.path(),
            Utc::now().timestamp() - 60,
            false,
            "bad-refresh",
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(url_path("/oauth/token"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "error": { "code": "refresh_token_expired" }
            })))
            .mount(&server)
            .await;

        let provider = CodexAuthProvider::new(dir.path().to_path_buf())
            .with_refresh_endpoint(format!("{}/oauth/token", server.uri()))
            .with_http_client(reqwest::Client::new());

        let err = provider.headers().await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("codex login"), "expected hint, got: {msg}");
    }

    #[tokio::test]
    async fn missing_auth_json_reports_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let provider = CodexAuthProvider::new(dir.path().to_path_buf());
        let err = provider.headers().await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not logged in"), "got: {msg}");
    }
}
