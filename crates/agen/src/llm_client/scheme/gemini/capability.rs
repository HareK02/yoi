//! Wire-level default capability for the Gemini scheme.
//!
//! Model ID tables (reasoning support by `gemini-*` version) belong to the
//! client construction layer. This module provides only conservative wire
//! defaults.

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme default capability (for unknown or unspecified models).
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: true,
        prompt_caching: CacheStrategy::Auto,
    }
}
