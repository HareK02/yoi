//! `model_id → ModelCapability` 静的テーブル（Google Gemini）。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};

/// Scheme 既定の capability（未知モデル / 未明示モデル用）。
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: true,
        prompt_caching: CacheStrategy::Auto,
    }
}

pub(crate) fn lookup(model_id: &str) -> Option<ModelCapability> {
    if !model_id.starts_with("gemini-") {
        return None;
    }
    // 2.5 系以降は thinking / reasoning を持つ
    let reasoning = if model_id.starts_with("gemini-2.5")
        || model_id.starts_with("gemini-3")
    {
        Some(ReasoningSupport::BudgetTokens)
    } else {
        None
    };
    Some(ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning,
        vision: true,
        prompt_caching: CacheStrategy::Auto,
    })
}
