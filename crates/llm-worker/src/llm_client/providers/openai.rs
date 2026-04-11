//! OpenAI プロバイダ実装
//!
//! OpenAI Chat Completions APIと通信し、Eventストリームを出力

use std::pin::Pin;

use crate::llm_client::{
    ClientError, ConfigWarning, LlmClient, Request, RequestConfig, event::Event,
    scheme::openai::OpenAIScheme,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

/// OpenAI クライアント
#[derive(Clone)]
pub struct OpenAIClient {
    /// HTTPクライアント
    http_client: reqwest::Client,
    /// APIキー
    api_key: String,
    /// モデル名
    model: String,
    /// スキーマ
    scheme: OpenAIScheme,
    /// ベースURL
    base_url: String,
}

impl OpenAIClient {
    /// 新しいOpenAIクライアントを作成
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            scheme: OpenAIScheme::default(),
            base_url: "https://api.openai.com".to_string(),
        }
    }

    /// カスタムHTTPクライアントを設定
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    /// スキーマを設定
    pub fn with_scheme(mut self, scheme: OpenAIScheme) -> Self {
        self.scheme = scheme;
        self
    }

    /// ベースURLを設定
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = url.into();
        self
    }

    /// リクエストヘッダーを構築
    fn build_headers(&self) -> Result<HeaderMap, ClientError> {
        let mut headers = HeaderMap::new();

        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let api_key_val = if self.api_key.is_empty() {
            // For providers like Ollama, API key might be empty/dummy.
            // But typical OpenAI requires it.
            // We'll allow empty if user intends it, but usually it's checked.
            HeaderValue::from_static("")
        } else {
            let mut val = HeaderValue::from_str(&format!("Bearer {}", self.api_key))
                .map_err(|e| ClientError::Config(format!("Invalid API key: {}", e)))?;
            val.set_sensitive(true);
            val
        };

        if !api_key_val.is_empty() {
            headers.insert("Authorization", api_key_val);
        }

        Ok(headers)
    }
}

#[async_trait]
impl LlmClient for OpenAIClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        // Construct the URL: base_url usually ends without slash, path starts with slash or vice versa.
        // Standard OpenAI base is "https://api.openai.com". Endpoint is "/v1/chat/completions".
        // If external base_url includes /v1, we should be careful.
        // Let's assume defaults. If user provides "http://localhost:11434/v1", we append "/chat/completions".
        // Or cleaner: user provides full base up to version?
        // Anthropic client uses "{}/v1/messages".
        // Let's stick to appending "/v1/chat/completions" if base is just host,
        // OR assume base includes /v1 if user overrides it?
        // Let's use robust joining or simple assumption matching Anthropic pattern:
        // Default: https://api.openai.com -> https://api.openai.com/v1/chat/completions

        // However, Ollama default is http://localhost:11434/v1/chat/completions if using OpenAI compact.
        // If we configure base_url via `with_base_url`, it's flexible.
        // Let's try to detect if /v1 is present or just append consistently.
        // Ideally `base_url` should be the root passed to `new`.

        let url = if self.base_url.ends_with("/v1") {
            format!("{}/chat/completions", self.base_url)
        } else if self.base_url.ends_with("/") {
            format!("{}v1/chat/completions", self.base_url)
        } else {
            format!("{}/v1/chat/completions", self.base_url)
        };

        let headers = self.build_headers()?;
        let body = self.scheme.build_request(&self.model, &request);

        let response = self
            .http_client
            .post(&url)
            .headers(headers)
            .json(&body)
            .send()
            .await?;

        // エラーレスポンスをチェック
        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();

            // JSONでエラーをパースしてみる
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                // OpenAI error format: { "error": { "message": "...", "type": "...", ... } }
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

        // SSEストリームを構築
        let scheme = self.scheme.clone();
        let byte_stream = response
            .bytes_stream()
            .map_err(|e| std::io::Error::other(e));
        let event_stream = byte_stream.eventsource();

        let stream = event_stream
            .map(move |result| {
                match result {
                    Ok(event) => {
                        // SSEイベントをパース
                        // OpenAI stream events are "data: {...}"
                        // event.event is usually "message" (default) or empty.
                        // parse_event takes data string.

                        if event.data == "[DONE]" {
                            // End of stream handled inside parse_event usually returning None
                            Ok(None)
                        } else {
                            match scheme.parse_event(&event.data) {
                                Ok(Some(events)) => Ok(Some(events)),
                                Ok(None) => Ok(None),
                                Err(e) => Err(e),
                            }
                        }
                    }
                    Err(e) => Err(ClientError::Sse(e.to_string())),
                }
            })
            // flatten Option<Vec<Event>> stream to Stream<Event>
            // map returns Result<Option<Vec<Event>>, Error>
            // We want Stream<Item = Result<Event, Error>>
            .map(|res| {
                let s: Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>> = match res {
                    Ok(Some(events)) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                    Ok(None) => Box::pin(futures::stream::empty()),
                    Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
                };
                s
            })
            .flatten();

        Ok(Box::pin(stream))
    }

    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        let mut warnings = Vec::new();

        // OpenAI does not support top_k
        if config.top_k.is_some() {
            warnings.push(ConfigWarning::unsupported("top_k", "OpenAI"));
        }

        warnings
    }
}
