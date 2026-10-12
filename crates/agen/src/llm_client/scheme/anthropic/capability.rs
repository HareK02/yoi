//! Wire-level default capability for the Anthropic scheme.
//!
//! Model ID tables (e.g. `claude-*`) belong to the client construction layer.
//! This module returns only the minimum common features safe to send over
//! this wire, even for unknown models.

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme default capability.
///
/// Uses `CacheStrategy::Auto` to avoid sending `cache_control`, allowing reuse
/// with Ollama's `/v1/messages` endpoint.
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
