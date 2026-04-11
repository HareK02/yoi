//! Ollama プロバイダ実装
//!
//! OllamaはOpenAI互換APIを提供するため、OpenAIクライアントと互換性がある。
//! デフォルトのベースURLと認証設定が異なる。

use std::pin::Pin;

use crate::llm_client::{
    ClientError, LlmClient, Request, event::Event, providers::openai::OpenAIClient,
    scheme::openai::OpenAIScheme,
};
use async_trait::async_trait;
use futures::Stream;

/// Ollama クライアント
///
/// 内部的にOpenAIClientを使用するラッパー、もしくはOpenAIClientと同様の実装を持つ。
/// ここではOpenAIClient構成をカスタマイズして提供する。
#[derive(Clone)]
pub struct OllamaClient {
    inner: OpenAIClient,
}

impl OllamaClient {
    /// 新しいOllamaクライアントを作成
    pub fn new(model: impl Into<String>) -> Self {
        // Ollama usually runs on localhost:11434/v1
        // API key is "ollama" or ignored
        let base_url = "http://localhost:11434";

        let scheme = OpenAIScheme::new().with_legacy_max_tokens(true);

        let client = OpenAIClient::new("ollama", model)
            .with_base_url(base_url)
            .with_scheme(scheme);
        // Currently OpenAIScheme sets include_usage: true. Ollama supports checks?
        // Assuming Ollama modern versions support usage.

        Self { inner: client }
    }

    /// ベースURLを設定
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.inner = self.inner.with_base_url(url);
        self
    }

    /// カスタムHTTPクライアントを設定
    pub fn with_http_client(mut self, client: reqwest::Client) -> Self {
        self.inner = self.inner.with_http_client(client);
        self
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }

    async fn stream(
        &self,
        request: Request,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Event, ClientError>> + Send>>, ClientError> {
        self.inner.stream(request).await
    }
}
