//! `impl Scheme for AnthropicScheme`
//!
//! Anthropic Messages API の wire 表現に必要な URL・ヘッダ・SSE パース・
//! リクエスト body 生成を共通 `Scheme` trait にぶら下げる。

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    capability::ModelCapability,
    event::{BlockStop, BlockType, Event},
    auth::AuthRequirement,
    scheme::Scheme,
    types::Request,
};

use super::AnthropicScheme;

/// Anthropic の SSE パースで必要な状態。
///
/// `content_block_stop` イベントは `block_type` を持たない仕様なので、
/// 直前の `content_block_start` で観測した `block_type` を保持して
/// `BlockStop` に書き戻す。
#[derive(Debug, Default)]
pub struct AnthropicState {
    current_block_type: Option<BlockType>,
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
        let Some(mut event) = self.parse_event(event_type, data)? else {
            return Ok(Vec::new());
        };
        match &event {
            Event::BlockStart(start) => {
                state.current_block_type = Some(start.block_type);
            }
            Event::BlockStop(stop) => {
                if let Some(block_type) = state.current_block_type.take() {
                    event = Event::BlockStop(BlockStop {
                        block_type,
                        ..stop.clone()
                    });
                }
            }
            _ => {}
        }
        Ok(vec![event])
    }

    fn capability_for(&self, model_id: &str) -> Option<ModelCapability> {
        super::capability::lookup(model_id)
    }
}
