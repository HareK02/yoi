//! Wire-level default capability for the OpenAI Chat Completions scheme.
//!
//! Model ID tables (e.g. the `gpt-5` family) belong to the client construction
//! layer. This module provides only conservative wire defaults.

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme default capability, used as a fallback for unknown model IDs on
/// OpenAI-compatible routers (xAI / Groq / OpenRouter, etc.).
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
