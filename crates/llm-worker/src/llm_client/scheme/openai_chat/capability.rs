//! `model_id → ModelCapability` 静的テーブル（OpenAI Chat Completions）。
//!
//! OpenAI 本家の主要モデルのみ網羅する。OpenRouter / xAI / Groq 等は
//! モデル ID が各社独自なので、マニフェスト側で明示 override する
//! 前提。
//!
//! [`classify`] はモデル ID から family を判定する一次情報で、
//! `scheme/openai_responses` からも参照される。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};

/// OpenAI 本家のモデル family 分類。
///
/// `openai_chat` と `openai_responses` で共有する一次情報。各 scheme は
/// この分類に自 scheme 固有の `ReasoningSupport` 等を当てはめて
/// `ModelCapability` を組み立てる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAiFamily {
    /// GPT-5 / o1 / o3 / o4 系 — reasoning 対応
    Reasoning,
    /// GPT-4o / GPT-4 系
    Gpt4,
    /// GPT-3.5 系（旧式）
    Gpt35,
}

/// モデル ID の prefix から family を判定する。未知は `None`。
pub(crate) fn classify(model_id: &str) -> Option<OpenAiFamily> {
    if model_id.starts_with("gpt-5")
        || model_id.starts_with("o1")
        || model_id.starts_with("o3")
        || model_id.starts_with("o4")
    {
        return Some(OpenAiFamily::Reasoning);
    }
    if model_id.starts_with("gpt-4") {
        return Some(OpenAiFamily::Gpt4);
    }
    if model_id.starts_with("gpt-3.5") {
        return Some(OpenAiFamily::Gpt35);
    }
    None
}

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
