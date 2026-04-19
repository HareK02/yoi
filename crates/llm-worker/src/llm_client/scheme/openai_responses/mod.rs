//! OpenAI Responses API スキーマ (`/v1/responses`)
//!
//! Chat Completions とは別物の item-based wire format。reasoning item と
//! function_call item が first-class で、SSE イベントも `response.*` 名前空間で
//! 流れる。ChatGPT OAuth 経路 (codex) は本 scheme 必須。
//!
//! - リクエスト JSON 生成: [`request`]
//! - SSE イベントパース → [`Event`](crate::llm_client::event::Event) 変換: [`events`]

mod capability;
mod events;
mod request;
mod scheme_impl;

pub use scheme_impl::OpenAIResponsesState;

/// OpenAI Responses scheme 本体。
///
/// `store` / `include_encrypted_content` は scheme 固定の wire 設定で、
/// デフォルトは stateless + ZDR 相当 (`store=false`, `include=[...]`)。
/// 将来 ZDR 非対応環境で `store=true` にしたくなった場合に限り override
/// する。`ModelCapability` には入れない（これはモデルの能力ではなく、
/// クライアントの運用方針）。
#[derive(Debug, Clone)]
pub struct OpenAIResponsesScheme {
    /// サーバ側に response を保存するか。ZDR/stateless 運用では `false`。
    pub store: bool,
    /// `include: ["reasoning.encrypted_content"]` を付けるか。
    /// `store=false` で reasoning を使うなら必須。
    pub include_encrypted_content: bool,
}

impl Default for OpenAIResponsesScheme {
    fn default() -> Self {
        Self {
            store: false,
            include_encrypted_content: true,
        }
    }
}

impl OpenAIResponsesScheme {
    /// デフォルト設定 (`store=false`, `include=["reasoning.encrypted_content"]`)。
    pub fn new() -> Self {
        Self::default()
    }

    /// `store` を上書き。
    pub fn with_store(mut self, store: bool) -> Self {
        self.store = store;
        self
    }

    /// `include: ["reasoning.encrypted_content"]` の有無を上書き。
    pub fn with_include_encrypted_content(mut self, include: bool) -> Self {
        self.include_encrypted_content = include;
        self
    }
}
