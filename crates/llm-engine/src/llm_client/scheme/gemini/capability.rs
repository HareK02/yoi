//! Gemini scheme の wire-level 既定 capability。
//!
//! モデル ID 固有のテーブル(`gemini-*` バージョン別の reasoning 有無)は
//! 高レベル構築層(`provider::capability`)の責務。ここでは wire の
//! 保守的 default のみ。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme 既定の capability(未知モデル / 未明示モデル用)。
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: true,
        prompt_caching: CacheStrategy::Auto,
    }
}
