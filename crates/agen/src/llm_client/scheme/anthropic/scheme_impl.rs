//! `impl Scheme for AnthropicScheme`
//!
//! Implements the URLs, headers, SSE parsing, and request body generation needed
//! for the Anthropic Messages API wire format through the shared `Scheme` trait.

use serde_json::Value;

use crate::llm_client::{
    ClientError,
    auth::AuthRequirement,
    capability::ModelCapability,
    client::ToolCallCompletionSupport,
    event::{BlockType, Event, ReasoningBlockData},
    scheme::Scheme,
    types::Request,
};

use super::AnthropicScheme;

/// State required for Anthropic SSE parsing.
///
/// 1. `content_block_stop` events lack `block_type`, so retain the type observed
///    in the preceding `content_block_start` and write it back into `BlockStop`.
/// 2. Accumulate `thinking_delta` text and `signature_delta` signatures from
///    `thinking` blocks, plus `data` from `redacted_thinking` blocks. Return them
///    as Thinking block metadata at `content_block_stop` for round-trip persistence.
#[derive(Debug, Default)]
pub struct AnthropicState {
    pub(crate) current_block_type: Option<BlockType>,
    pub(crate) pending_thinking: Option<PendingThinking>,
}

/// Accumulation buffer for one `thinking` or `redacted_thinking` content_block.
#[derive(Debug, Default)]
pub(crate) struct PendingThinking {
    pub(crate) text: String,
    pub(crate) signature: Option<String>,
    pub(crate) redacted_data: Option<String>,
}

impl PendingThinking {
    pub(crate) fn into_reasoning(self) -> ReasoningBlockData {
        ReasoningBlockData {
            id: None,
            text: Some(self.text),
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
        // Anthropic uses XApiKey. Compatible endpoints such as Ollama can opt out
        // through ResolvedAuth::None, which the transport accepts for any auth
        // requirement and handles without sending authentication headers.
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

    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::PerBlock
    }
}
