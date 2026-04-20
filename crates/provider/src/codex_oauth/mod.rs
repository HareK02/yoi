//! ChatGPT OAuth (`~/.codex/auth.json`) を読んで `Authorization` /
//! `ChatGPT-Account-Id` / `X-OpenAI-Fedramp` ヘッダを組み立てる
//! [`AuthProvider`] 実装。
//!
//! 設計:
//!
//! - llm-worker は [`AuthProvider`] trait しか知らず、実体である
//!   [`CodexAuthProvider`] はこのクレートに置く（feedback_llm_worker_scope）
//! - access_token JWT の `exp` を読み、`now` 以下で proactive refresh
//!   （Codex CLI と同じバッファなし）
//! - 並行する Codex CLI / 別 insomnia の refresh と取り違えないよう、
//!   refresh 直前に再 load して account_id 一致を確認（guarded reload）
//! - ファイルロックは取らず、書込前に再 load + diff merge で吸収
//! - Codex の Keyring storage は対象外。auth.json 不在ならエラーで案内

mod auth_json;
mod error;
mod jwt;
mod refresh;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use llm_worker::llm_client::{
    ClientError,
    auth::AuthProvider,
};
use reqwest::header::{HeaderName, HeaderValue};
use tokio::sync::Mutex;

use auth_json::AuthSnapshot;
use error::CodexAuthError;

pub use error::PermanentReason;

/// Codex CLI の `last_refresh` ベース fallback 期限（Codex CLI 準拠で 8 日）。
const TOKEN_REFRESH_INTERVAL_DAYS: i64 = 8;

/// `~/.codex/auth.json` を読んで Codex 互換のヘッダを返す provider。
pub struct CodexAuthProvider {
    auth_path: PathBuf,
    refresh_endpoint: String,
    http: reqwest::Client,
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
    /// `CODEX_HOME` → `$HOME/.codex` の順で auth.json の場所を決める。
    pub fn from_default_home() -> Result<Self, ClientError> {
        let codex_home = if let Ok(p) = std::env::var("CODEX_HOME") {
            PathBuf::from(p)
        } else {
            let home = std::env::var("HOME")
                .map_err(|_| ClientError::Config("HOME not set".into()))?;
            PathBuf::from(home).join(".codex")
        };
        Ok(Self::new(codex_home))
    }

    /// 任意の codex_home から構築する。
    pub fn new(codex_home: PathBuf) -> Self {
        Self {
            auth_path: codex_home.join("auth.json"),
            refresh_endpoint: refresh::REFRESH_URL.to_string(),
            http: reqwest::Client::new(),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// テスト用に refresh エンドポイントを差し替える。
    #[cfg(test)]
    fn with_refresh_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.refresh_endpoint = endpoint.into();
        self
    }

    /// テスト用に HTTP クライアントを差し替える。
    #[cfg(test)]
    fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http = client;
        self
    }

    async fn ensure_fresh(&self) -> Result<AuthSnapshot, CodexAuthError> {
        let mut state = self.state.lock().await;

        // 1. 常にディスクから最新を読む（他プロセスの更新を反映）
        let snap = auth_json::load(&self.auth_path).await?;

        // 2. stale でなければそのまま返す
        if !is_stale(&snap) {
            state.cached = Some(snap.clone());
            return Ok(snap);
        }

        // 3. Refresh 直前に再 load し account_id 一致を確認（guarded reload）。
        //    一致しなければ他プロセスが先に更新済 → 自分は refresh しない
        let pre_refresh = auth_json::load(&self.auth_path).await?;
        if !is_stale(&pre_refresh) {
            state.cached = Some(pre_refresh.clone());
            return Ok(pre_refresh);
        }
        if pre_refresh.account_id != snap.account_id {
            // account が切り替わった → 新しい auth を採用、refresh はしない
            state.cached = Some(pre_refresh.clone());
            return Ok(pre_refresh);
        }

        // 4. Refresh 実行
        let refreshed = refresh::request_refresh(
            &self.http,
            &self.refresh_endpoint,
            &pre_refresh.refresh_token,
        )
        .await?;

        // 5. 書き戻し（書込前に再 load + merge は persist_refreshed 内で実施）
        let new_snap = auth_json::persist_refreshed(
            &self.auth_path,
            refreshed.id_token,
            refreshed.access_token,
            refreshed.refresh_token,
        )
        .await?;
        state.cached = Some(new_snap.clone());
        Ok(new_snap)
    }

    fn build_headers(snap: &AuthSnapshot) -> Result<Vec<(HeaderName, HeaderValue)>, CodexAuthError> {
        let mut out = Vec::with_capacity(3);

        let auth_val = HeaderValue::from_str(&format!("Bearer {}", snap.access_token))
            .map_err(|e| CodexAuthError::InvalidHeader(format!("Authorization: {e}")))?;
        out.push((HeaderName::from_static("authorization"), auth_val));

        let acc_val = HeaderValue::from_str(&snap.account_id)
            .map_err(|e| CodexAuthError::InvalidHeader(format!("ChatGPT-Account-Id: {e}")))?;
        out.push((
            HeaderName::from_static("chatgpt-account-id"),
            acc_val,
        ));

        // FedRAMP 組織は id_token JWT 内の claim で判定
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
        let snap = self.ensure_fresh().await.map_err(CodexAuthError::to_client_error)?;
        Self::build_headers(&snap).map_err(CodexAuthError::to_client_error)
    }
}

/// `access_token` の JWT `exp` を見て、期限切れなら true。
/// `exp` が読めない場合は `last_refresh + 8 日` で判定（Codex CLI と同じ）。
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

    fn make_jwt(payload: serde_json::Value) -> String {
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap());
        format!("h.{p}.s")
    }

    fn write_auth(dir: &std::path::Path, exp: i64, fedramp: bool, refresh: &str) -> PathBuf {
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
        // 期限を遠い未来に
        write_auth(dir.path(), Utc::now().timestamp() + 3600, false, "rt");
        let provider = CodexAuthProvider::new(dir.path().to_path_buf());

        let headers = provider.headers().await.unwrap();
        let names: Vec<_> = headers.iter().map(|(n, _)| n.as_str().to_string()).collect();
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
        let path = write_auth(dir.path(), Utc::now().timestamp() - 60, false, "old-refresh");

        // refresh エンドポイントを mock。新しい JWT (将来 exp) を返す
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

        // ファイルに新しい refresh_token が書き戻されている
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
        write_auth(dir.path(), Utc::now().timestamp() - 60, false, "bad-refresh");

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
