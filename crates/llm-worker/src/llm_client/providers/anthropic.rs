//! Anthropic プロバイダ実装
//!
//! Anthropic Messages APIと通信し、Eventストリームを出力

use std::pin::Pin;

use crate::llm_client::{
    ClientError, LlmClient, Request, event::Event, scheme::anthropic::AnthropicScheme,
};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::{Stream, StreamExt, TryStreamExt, future::ready};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue};

/// Anthropic クライアント
pub struct AnthropicClient {
    /// HTTPクライアント
    http_client: reqwest::Client,
    /// APIキー
    api_key: String,
    /// モデル名
    model: String,
    /// スキーマ
    scheme: AnthropicScheme,
    /// ベースURL
    base_url: String,
}

impl AnthropicClient {
    /// 新しいAnthropicクライアントを作成
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
            scheme: AnthropicScheme::default(),
            base_url: "https://api.anthropic.com".to_string(),
        }
    }

    /// カスタムHTTPクライアントを設定
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.http_client = client;
        self
    }

    /// スキーマを設定
    pub fn with_scheme(mut self, scheme: AnthropicScheme) -> Self {
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
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key)
                .map_err(|e| ClientError::Config(format!("Invalid API key: {}", e)))?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_str(&self.scheme.api_version)
                .map_err(|e| ClientError::Config(format!("Invalid API version: {}", e)))?,
        );

        // 細粒度ツールストリーミングを有効にする場合
        if self.scheme.fine_grained_tool_streaming {
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static("fine-grained-tool-streaming-2025-05-14"),
            );
        }

        Ok(headers)
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        let url = format!("{}/v1/messages", self.base_url);
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

        // AnthropicはBlockStopイベントに正しいblock_typeを含まないため、
        // クライアント側で状態を追跡して補完する
        let mut current_block_type = None;

        let stream = event_stream.filter_map(move |result| {
            ready(match result {
                Ok(event) => {
                    // SSEイベントをパース
                    match scheme.parse_event(&event.event, &event.data) {
                        Ok(Some(mut evt)) => {
                            // ブロックタイプの追跡と修正
                            match &evt {
                                Event::BlockStart(start) => {
                                    current_block_type = Some(start.block_type);
                                }
                                Event::BlockStop(stop) => {
                                    if let Some(block_type) = current_block_type.take() {
                                        // 正しいブロックタイプで上書き
                                        // (Event::BlockStopの中身を置換)
                                        evt =
                                            Event::BlockStop(crate::llm_client::event::BlockStop {
                                                block_type,
                                                ..stop.clone()
                                            });
                                    }
                                }
                                _ => {}
                            }
                            Some(Ok(evt))
                        }
                        Ok(None) => None,
                        Err(e) => Some(Err(e)),
                    }
                }
                Err(e) => Some(Err(ClientError::Sse(e.to_string()))),
            })
        });

        Ok(Box::pin(stream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_creation() {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-20250514");
        assert_eq!(client.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_build_headers() {
        let client = AnthropicClient::new("test-key", "claude-sonnet-4-20250514");
        let headers = client.build_headers().unwrap();

        assert!(headers.contains_key("x-api-key"));
        assert!(headers.contains_key("anthropic-version"));
        assert!(headers.contains_key("anthropic-beta"));
    }
}
