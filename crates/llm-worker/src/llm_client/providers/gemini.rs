//! Gemini プロバイダ実装
//!
//! Google Gemini APIと通信し、Eventストリームを出力

use std::pin::Pin;

use crate::llm_client::{
    ClientError, LlmClient, Request, event::Event, scheme::gemini::GeminiScheme,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt, TryStreamExt};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

/// Gemini クライアント
#[derive(Clone)]
pub struct GeminiClient {
    /// HTTPクライアント
    http_client: reqwest::Client,
    /// APIキー
    api_key: String,
    /// モデル名
    model: String,
    /// スキーマ
    scheme: GeminiScheme,
    /// ベースURL
    base_url: String,
}

impl GeminiClient {
    /// 新しいGeminiクライアントを作成
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            scheme: GeminiScheme::default(),
            base_url: "https://generativelanguage.googleapis.com".to_string(),
        }
    }

    /// カスタムHTTPクライアントを設定
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    /// スキーマを設定
    pub fn with_scheme(mut self, scheme: GeminiScheme) -> Self {
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

        Ok(headers)
    }
}

#[async_trait]
impl LlmClient for GeminiClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        // URL構築: base_url/v1beta/models/{model}:streamGenerateContent?alt=sse&key={api_key}
        let url = format!(
            "{}/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
            self.base_url, self.model, self.api_key
        );

        let headers = self.build_headers()?;
        let body = self.scheme.build_request(&request);

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
                // Gemini error format: { "error": { "code": xxx, "message": "...", "status": "..." } }
                let error = json.get("error").unwrap_or(&json);
                let code = error
                    .get("status")
                    .and_then(|v| v.as_str())
                    .map(String::from);
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
                        // Geminiは "data: {...}" 形式で送る
                        match scheme.parse_event(&event.data) {
                            Ok(Some(events)) => Ok(Some(events)),
                            Ok(None) => Ok(None),
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(ClientError::Sse(e.to_string())),
                }
            })
            // flatten Option<Vec<Event>> stream to Stream<Event>
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = GeminiClient::new("test-key", "gemini-2.0-flash");
        assert_eq!(client.model, "gemini-2.0-flash");
    }

    #[test]
    fn test_build_headers() {
        let client = GeminiClient::new("test-key", "gemini-2.0-flash");
        let headers = client.build_headers().unwrap();

        assert!(headers.contains_key("content-type"));
    }

    #[test]
    fn test_custom_base_url() {
        let client = GeminiClient::new("test-key", "gemini-2.0-flash")
            .with_base_url("https://custom.api.example.com");
        assert_eq!(client.base_url, "https://custom.api.example.com");
    }
}
