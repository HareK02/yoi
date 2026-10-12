//! API scheme definitions.
//!
//! Conversion logic for each API scheme:
//! - Request conversion: Request → provider-specific JSON.
//! - Response conversion: SSE events → Event.
//!
//! The [`Scheme`] trait delegates all scheme-specific differences from
//! `HttpTransport<S>` (paths, headers, authentication requirements, body
//! generation, and SSE parsing).

pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use serde_json::Value;

use super::auth::AuthRequirement;
use super::capability::ModelCapability;
use super::client::{ConfigWarning, ToolCallCompletionSupport};
use super::error::ClientError;
use super::event::Event;
use super::types::{Request, RequestConfig};

/// Wire scheme abstraction, implemented once per provider API specification.
///
/// `HttpTransport<S: Scheme>` handles URL assembly, authentication header
/// insertion, and the SSE parsing loop; `Scheme` implementations supply only
/// specification-specific differences.
///
/// # State
///
/// Schemes requiring state across SSE frames (e.g. to compensate for Anthropic
/// omitting `block_type` from `BlockStop`) define an intermediate state type in
/// [`Scheme::State`]. Stateless schemes use `type State = ()`.
pub trait Scheme: Clone + Send + Sync + 'static {
    /// State shared across SSE frames. `HttpTransport` creates it once via
    /// `Default::default()` at stream start and passes it by `&mut` for each frame.
    type State: Default + Send + 'static;

    /// Default base URL for callers that do not supply an override.
    fn default_base_url(&self) -> &'static str;

    /// Relative request path. Accepts a model ID because some providers, such as
    /// Gemini, embed the model name in the path.
    fn path(&self, model_id: &str) -> String;

    /// Authentication format required by this scheme. The caller checks it
    /// against configured credentials when constructing the client.
    fn required_auth(&self) -> AuthRequirement;

    /// Additional headers beyond `Content-Type`, e.g. `anthropic-version` / `anthropic-beta`.
    fn additional_headers(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }

    /// Build the request body. `capability` lets the scheme select behavior
    /// based on `CacheStrategy` and `ReasoningSupport`.
    fn build_request_body(
        &self,
        model_id: &str,
        request: &Request,
        capability: &ModelCapability,
    ) -> Value;

    /// Convert one SSE event to zero or more [`Event`]s.
    ///
    /// `event_type` is the SSE frame's `event:` field; `data` is its `data:` field.
    /// Implementations detect terminal markers such as `[DONE]`.
    /// `state` is mutable state shared for the stream.
    fn parse_sse(
        &self,
        event_type: &str,
        data: &str,
        state: &mut Self::State,
    ) -> Result<Vec<Event>, ClientError>;

    /// Scheme default capability: the minimum common features safe to send over
    /// this wire, regardless of model ID. Known-model capability tables belong
    /// to the higher-level client construction layer, not the scheme.
    fn default_capability(&self) -> ModelCapability;

    /// Strongest safe completion boundary for streamed tool calls.
    fn tool_call_completion_support(&self) -> ToolCallCompletionSupport {
        ToolCallCompletionSupport::ResponseComplete
    }

    /// Return warnings for `RequestConfig` fields unsupported by the scheme
    /// (e.g. OpenAI Chat does not support `top_k`). Defaults to an empty Vec.
    fn validate_config(&self, config: &RequestConfig) -> Vec<ConfigWarning> {
        let _ = config;
        Vec::new()
    }
}

#[cfg(test)]
mod completion_support_tests {
    use super::*;
    use crate::llm_client::ToolCallCompletionSupport;

    #[test]
    fn only_schemes_with_authoritative_per_call_done_events_enable_early_dispatch() {
        assert_eq!(
            anthropic::AnthropicScheme::new().tool_call_completion_support(),
            ToolCallCompletionSupport::PerBlock
        );
        assert_eq!(
            openai_responses::OpenAIResponsesScheme::new().tool_call_completion_support(),
            ToolCallCompletionSupport::PerBlock
        );
        assert_eq!(
            openai_chat::OpenAIScheme::new().tool_call_completion_support(),
            ToolCallCompletionSupport::ResponseComplete
        );
        assert_eq!(
            gemini::GeminiScheme::new().tool_call_completion_support(),
            ToolCallCompletionSupport::ResponseComplete
        );
    }
}
