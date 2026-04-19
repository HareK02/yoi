//! `model_id → ModelCapability` 静的テーブル（OpenAI Chat Completions）。
//!
//! OpenAI 本家の主要モデルのみ網羅する。OpenRouter / xAI / Groq 等は
//! モデル ID が各社独自なので、マニフェスト側で明示 override する
//! 前提。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};

pub(crate) fn lookup(model_id: &str) -> Option<ModelCapability> {
    // GPT-5 / o1 / o3 / o4 reasoning 系
    if model_id.starts_with("gpt-5")
        || model_id.starts_with("o1")
        || model_id.starts_with("o3")
        || model_id.starts_with("o4")
    {
        return Some(ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: Some(ReasoningSupport::Effort),
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        });
    }
    // GPT-4o / GPT-4 系
    if model_id.starts_with("gpt-4") {
        return Some(ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonSchema,
            reasoning: None,
            vision: true,
            prompt_caching: CacheStrategy::Auto,
        });
    }
    // GPT-3.5 系（旧式・structured output 限定）
    if model_id.starts_with("gpt-3.5") {
        return Some(ModelCapability {
            tool_calling: ToolCallingSupport::Parallel,
            structured_output: StructuredOutput::JsonObject,
            reasoning: None,
            vision: false,
            prompt_caching: CacheStrategy::Auto,
        });
    }
    None
}

/// Scheme 既定の capability。OpenAI 互換ルーター系（xAI / Groq / OpenRouter 等）
/// で未知モデル ID を受けたときのフォールバックに使う。
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
