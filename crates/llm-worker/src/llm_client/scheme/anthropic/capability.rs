//! `model_id → ModelCapability` 静的テーブル。
//!
//! 既知モデルのみ網羅する。未知モデルは `None` を返し、呼び出し側
//! （`HttpTransport` 構築時）に scheme 既定へフォールバックさせる。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};

/// Anthropic 公式モデルの既定 capability。
///
/// `claude-sonnet-*` / `claude-opus-*` / `claude-haiku-*` に対応する。
/// `cache_control` は公式のみ有効で、最大 4 breakpoint（公式仕様）。
pub(crate) fn lookup(model_id: &str) -> Option<ModelCapability> {
    if !model_id.starts_with("claude-") {
        return None;
    }
    Some(ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: Some(ReasoningSupport::BudgetTokens),
        vision: true,
        prompt_caching: CacheStrategy::Explicit { max_breakpoints: 4 },
    })
}

/// Scheme 既定の capability。
///
/// Ollama の `/v1/messages` 流用を想定して `cache_control` を送らない
/// `CacheStrategy::Auto` にする。Anthropic 本家の未知モデル（新 Claude）
/// も tool_calling / vision を備える想定で Parallel / true を返す。
pub(crate) fn default_capability() -> ModelCapability {
    ModelCapability {
        tool_calling: ToolCallingSupport::Parallel,
        structured_output: StructuredOutput::JsonSchema,
        reasoning: None,
        vision: false,
        prompt_caching: CacheStrategy::Auto,
    }
}
