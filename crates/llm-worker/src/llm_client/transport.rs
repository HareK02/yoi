//! `HttpTransport<S: Scheme>`: すべての LLM wire scheme を共通の 1 本の
//! HTTP クライアントで扱う。
//!
//! 旧 `providers/{anthropic,openai,gemini,ollama}.rs` を置き換える。
//! scheme 固有の差分は [`Scheme`] trait 実装に委譲する。

use std::pin::Pin;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

use super::auth::AuthRequirement;
use super::capability::ModelCapability;
use super::client::{ConfigWarning, LlmClient};
use super::error::ClientError;
use super::event::Event;
use super::scheme::Scheme;
use super::types::{Request, RequestConfig};

/// `AuthRef` を解決したランタイム表現。`crates/provider` が構築する。
///
/// `AuthRef::ApiKey` → 読み取った文字列、`AuthRef::None` → `None`。
/// `CodexOAuth` 等、動的に更新される認証は別途 `Custom` バリアントを
/// 追加する余地を残す（本チケットでは未実装）。
#[derive(Debug, Clone)]
pub enum ResolvedAuth {
    None,
    ApiKey(String),
}

impl ResolvedAuth {
    /// 認証要件と実際の解決値が噛み合うか検査する。構築時検証用。
    ///
    /// `ResolvedAuth::None` は認証を付けないという宣言なので、どの
    /// `AuthRequirement` でも受け入れる（Ollama の Anthropic scheme
    /// 流用は `required_auth = XApiKey` だが認証ヘッダなしで動く）。
    pub fn matches(&self, req: AuthRequirement) -> bool {
        match (self, req) {
            (Self::None, _) => true,
            (
                Self::ApiKey(_),
                AuthRequirement::Bearer | AuthRequirement::XApiKey | AuthRequirement::QueryParam { .. },
            ) => true,
            _ => false,
        }
    }
}

/// scheme 共通の HTTP 通信層。
pub struct HttpTransport<S: Scheme> {
    http_client: reqwest::Client,
    scheme: S,
    model_id: String,
    base_url: String,
    auth: ResolvedAuth,
    capability: ModelCapability,
}

impl<S: Scheme> HttpTransport<S> {
    /// 新しい transport を作る。`base_url` は末尾スラッシュの有無を
    /// どちらでも受け付ける（内部で正規化）。
    pub fn new(
        scheme: S,
        model_id: impl Into<String>,
        base_url: impl Into<String>,
        auth: ResolvedAuth,
        capability: ModelCapability,
    ) -> Self {
        let base_url = base_url.into();
        let base_url = base_url.trim_end_matches('/').to_string();
        Self {
            http_client: reqwest::Client::new(),
            scheme,
            model_id: model_id.into(),
            base_url,
            auth,
            capability,
        }
    }

    /// カスタム HTTP クライアントを差し込む（テスト等）。
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    fn build_url(&self) -> String {
        let path = self.scheme.path(&self.model_id);
        let url = format!("{}{}", self.base_url, path);
        // Gemini のようにクエリパラメータで認証する場合は URL にキーを追記する
        if let (AuthRequirement::QueryParam { name }, ResolvedAuth::ApiKey(key)) =
            (self.scheme.required_auth(), &self.auth)
        {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}{name}={key}")
        } else {
            url
        }
    }

    fn build_headers(&self) -> Result<HeaderMap, ClientError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        match (self.scheme.required_auth(), &self.auth) {
            (AuthRequirement::None, _) | (_, ResolvedAuth::None) => {}
            (AuthRequirement::Bearer, ResolvedAuth::ApiKey(key)) => {
                let mut val = HeaderValue::from_str(&format!("Bearer {key}"))
                    .map_err(|e| ClientError::Config(format!("invalid api key: {e}")))?;
                val.set_sensitive(true);
                headers.insert("Authorization", val);
            }
            (AuthRequirement::XApiKey, ResolvedAuth::ApiKey(key)) => {
                let mut val = HeaderValue::from_str(key.as_str())
                    .map_err(|e| ClientError::Config(format!("invalid api key: {e}")))?;
                val.set_sensitive(true);
                headers.insert("x-api-key", val);
            }
            (AuthRequirement::QueryParam { .. }, _) => {
                // クエリパラメータは `build_url` で付与済み
            }
            (AuthRequirement::Custom, _) => {
                // 今チケットでは Custom は使わない。Codex OAuth で追加予定
            }
        }

        for (name, value) in self.scheme.additional_headers() {
            let hv = HeaderValue::from_str(&value)
                .map_err(|e| ClientError::Config(format!("invalid header {name}: {e}")))?;
            headers.insert(name, hv);
        }

        Ok(headers)
    }
}

impl<S: Scheme + Clone> Clone for HttpTransport<S> {
    fn clone(&self) -> Self {
        Self {
            http_client: self.http_client.clone(),
            scheme: self.scheme.clone(),
            model_id: self.model_id.clone(),
            base_url: self.base_url.clone(),
            auth: self.auth.clone(),
            capability: self.capability.clone(),
        }
    }
}

#[async_trait]
impl<S: Scheme + Clone + 'static> LlmClient for HttpTransport<S> {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        self.scheme.validate_config(config)
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        let url = self.build_url();
        let headers = self.build_headers()?;
        let body = self
            .scheme
            .build_request_body(&self.model_id, &request, &self.capability);

        let response = self
            .http_client
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                let error = json.get("error").unwrap_or(&json);
                let code = error.get("type").and_then(|v| v.as_str()).map(String::from);
                let message = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&text)
                    .to_string();
                return Err(ClientError::Api {
                    status: Some(status),
                    code,
                    message,
                });
            }
            return Err(ClientError::Api {
                status: Some(status),
                code: None,
                message: text,
            });
        }

        let scheme = self.scheme.clone();
        let byte_stream = response.bytes_stream().map_err(std::io::Error::other);
        let event_stream = byte_stream.eventsource();

        // scheme 固有のパース状態をストリーム単位で保持する
        let mut state = <S::State as Default>::default();

        let stream = event_stream
            .map(move |result| match result {
                Ok(frame) => match scheme.parse_sse(&frame.event, &frame.data, &mut state) {
                    Ok(events) => Ok(events),
                    Err(e) => Err(e),
                },
                Err(e) => Err(ClientError::Sse(e.to_string())),
            })
            .map(|res| {
                let s: Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>> = match res {
                    Ok(events) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                    Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
                };
                s
            })
            .flatten();

        Ok(Box::pin(stream))
    }
}
