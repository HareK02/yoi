//! OpenAI Responses API スキーマ (`/v1/responses`)
//!
//! Chat Completions とは別物の item-based wire format。reasoning item と
//! function_call item が first-class で、SSE イベントも `response.*` 名前空間で
//! 流れる。
//!
//! - リクエスト JSON 生成: `request`
//! - SSE イベントパース → [`Event`](crate::llm_client::event::Event) 変換: `events`

mod capability;
mod events;
mod request;
mod scheme_impl;

pub use scheme_impl::OpenAIResponsesState;

/// OpenAI Responses scheme 本体。
///
/// `store` / `include_encrypted_content` / `send_max_output_tokens` /
/// `send_sampling_params` は scheme 固定の wire 設定で、デフォルトは
/// 公式 OpenAI Responses API 向け (stateless + ZDR + `max_output_tokens`
/// / `temperature` / `top_p` 送出可)。受理パラメータが subset の
/// 互換 backend では client 構築層で `send_max_output_tokens=false` /
/// `send_sampling_params=false` に上書きする。`ModelCapability` には
/// 入れない（モデル能力ではなく wire policy）。
#[derive(Debug, Clone)]
pub struct OpenAIResponsesScheme {
    /// サーバ側に response を保存するか。ZDR/stateless 運用では `false`。
    pub store: bool,
    /// `include: ["reasoning.encrypted_content"]` を付けるか。
    /// `store=false` で reasoning を使うなら必須。
    pub include_encrypted_content: bool,
    /// `max_output_tokens` を body に載せるか。公式 OpenAI Responses API は
    /// 受理するが、互換 backend によっては `Unsupported parameter` で
    /// 400 を返すため、その経路では `false` にする。
    pub send_max_output_tokens: bool,
    /// `temperature` / `top_p` を body に載せるか。公式 OpenAI Responses API
    /// は受理するが、互換 backend によっては `Unsupported parameter` で
    /// 400 を返すため、その経路では `false` にする。
    pub send_sampling_params: bool,
}

impl Default for OpenAIResponsesScheme {
    fn default() -> Self {
        Self {
            store: false,
            include_encrypted_content: true,
            send_max_output_tokens: true,
            send_sampling_params: true,
        }
    }
}

impl OpenAIResponsesScheme {
    /// デフォルト設定 (`store=false`, `include=["reasoning.encrypted_content"]`,
    /// `send_max_output_tokens=true`, `send_sampling_params=true`)。
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

    /// `max_output_tokens` を body に載せるかを上書き。
    pub fn with_send_max_output_tokens(mut self, send: bool) -> Self {
        self.send_max_output_tokens = send;
        self
    }

    /// `temperature` / `top_p` を body に載せるかを上書き。
    pub fn with_send_sampling_params(mut self, send: bool) -> Self {
        self.send_sampling_params = send;
        self
    }
}
