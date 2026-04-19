//! `model_id → ModelCapability` 静的テーブル（OpenAI Responses API）。
//!
//! モデル family 判定は `scheme/openai_chat/capability.rs::classify` を
//! 共有する。Responses 側は `ReasoningSupport::Effort` 固定で、prompt
//! caching はサーバ側自動（`CacheStrategy::Auto`）。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};
use crate::llm_client::scheme::openai_chat::capability::{OpenAiFamily, classify};

pub(crate) fn lookup(model_id: &str) -> Option<ModelCapability> {
    classify(model_id).map(|family| match family {
        OpenAiFamily::Reasoning => ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: Some(ReasoningSupport::Effort),
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        },
        OpenAiFamily::Gpt4 => ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: None,
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        },
        OpenAiFamily::Gpt35 => ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonObject,
            reasoning: None,
            vision: false,
            prompt_caching: CacheStrategy::Auto,
        },
    })
}

pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
