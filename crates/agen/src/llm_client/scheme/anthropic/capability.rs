//! Anthropic scheme の wire-level 既定 capability。
//!
//! モデル ID 固有のテーブル(`claude-*` など)は client construction layer
//! の責務。ここでは未知モデルでも「この wire で
//! 安全に送れる最小共通項」を返すだけに留める。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, StructuredOutput, ToolCallingSupport,
};

/// Scheme 既定の capability。
///
/// Ollama の `/v1/messages` 流用を想定して `cache_control` を送らない
/// `CacheStrategy::Auto` にする。
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
