//! OpenAI Responses scheme の wire-level 既定 capability。
//!
//! モデル ID 固有のテーブル(`gpt-5` / `codex-` 系など)は高レベル構築層
//! (`provider::capability`)の責務。ここでは wire の保守的 default のみ。

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
