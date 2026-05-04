//! `impl Scheme for AnthropicScheme`
//!
//! Anthropic Messages API の wire 表現に必要な URL・ヘッダ・SSE パース・
//! リクエスト body 生成を共通 `Scheme` trait にぶら下げる。

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    auth::AuthRequirement,
    capability::ModelCapability,
    event::{BlockType, Event, ReasoningItemEvent},
    scheme::Scheme,
    types::Request,
};

use super::AnthropicScheme;

/// Anthropic の SSE パースで必要な状態。
///
/// 1. `content_block_stop` イベントは `block_type` を持たない仕様なので、
///    直前の `content_block_start` で観測した `block_type` を保持して
///    `BlockStop` に書き戻す。
/// 2. `thinking` ブロック中の `thinking_delta` テキストと `signature_delta`
///    署名、および `redacted_thinking` ブロックの `data` を蓄積し、
///    `content_block_stop` で `Event::ReasoningItem` を発火する
///    （round-trip 永続化のため）。
#[derive(Debug, Default)]
pub struct AnthropicState {
    pub(crate) current_block_type: Option<BlockType>,
    pub(crate) pending_thinking: Option<PendingThinking>,
}

/// 1 つの `thinking` または `redacted_thinking` content_block の蓄積バッファ。
#[derive(Debug, Default)]
pub(crate) struct PendingThinking {
    pub(crate) text: String,
    pub(crate) signature: Option<String>,
    pub(crate) redacted_data: Option<String>,
}

impl PendingThinking {
    pub(crate) fn into_event(self) -> ReasoningItemEvent {
        ReasoningItemEvent {
            id: None,
            text: self.text,
            summary: Vec::new(),
            encrypted_content: self.redacted_data,
            signature: self.signature,
        }
    }
}

impl Scheme for AnthropicScheme {
    type State = AnthropicState;

    fn default_base_url(&self) -> &'static str {
        "https://api.anthropic.com"
    }

    fn path(&self, _model_id: &str) -> String {
        "/v1/messages".to_string()
    }

    fn required_auth(&self) -> AuthRequirement {
        // Ollama の `/v1/messages` 互換では認証が要らないが、それは
        // `AuthRef::None` + `build_headers` 側の「ResolvedAuth::None
        // なら何もしない」分岐で吸収する（`accepts` 判定で弾かれない
        // よう、現状は XApiKey を要求しつつ、None 側でもパスするよう
        // にする戦略）。
        AuthRequirement::XApiKey
    }

    fn additional_headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("anthropic-version", self.api_version.clone())];
        if self.fine_grained_tool_streaming {
            headers.push((
                "anthropic-beta",
                "fine-grained-tool-streaming-2025-05-14".to_string(),
            ));
        }
        headers
    }

    fn build_request_body(
        &self,
        model_id: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> Value {
        let req = self.build_request(model_id, request, capability);
        serde_json::to_value(&req).expect("AnthropicRequest is always serialisable")
    }

    fn parse_sse(
        &self,
        event_type: &str,
        data: &str,
        state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError> {
        self.parse_with_state(event_type, data, state)
    }

    fn default_capability(&self) -> ModelCapability {
        super::capability::default_capability()
    }
}
