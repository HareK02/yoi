//! OpenAI Chat Completions scheme の wire-level 既定 capability。
//!
//! モデル ID 固有のテーブル(`gpt-5` 系など)は client construction layer
//! の責務。ここでは wire の保守的 default のみ。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme 既定の capability。OpenAI 互換ルーター系(xAI / Groq / OpenRouter 等)
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
