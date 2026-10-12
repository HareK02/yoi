//! Wire-level default capability for the OpenAI Responses scheme.
//!
//! Model ID capability tables belong to the higher-level client construction
//! layer. This module provides only conservative wire defaults.

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
