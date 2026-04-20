//! `model_id → ModelCapability` 静的テーブル（OpenAI Responses API）。
//!
//! モデル family 判定は `scheme/openai_chat/capability.rs::classify` を
//! 共有する。Responses 側は `ReasoningSupport::Effort` 固定で、prompt
//! caching はサーバ側自動（`CacheStrategy::Auto`）。
//!
//! `gpt-5-codex` は `gpt-5` prefix 経由で Reasoning 扱いされるが、
//! `codex-mini-latest` 等 `codex-` prefix のモデルは ChatGPT backend
//! 経由（CodexOAuth）でしか使えないため、このテーブルでだけ Reasoning
//! にフォールバックする。

use crate::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};
use crate::llm_client::scheme::openai_chat::capability::{OpenAiFamily, classify};

pub(crate) fn lookup(model_id: &str) -> Option<ModelCapability> {
    let family = classify(model_id).or_else(|| {
        if model_id.starts_with("codex-") {
            Some(OpenAiFamily::Reasoning)
        } else {
            None
        }
    })?;
    Some(match family {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_5_codex_is_reasoning() {
        // `gpt-5` prefix で classify される
        let cap = lookup("gpt-5-codex").unwrap();
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn codex_mini_latest_is_reasoning() {
        // ChatGPT backend 専用モデル。`codex-` prefix で Reasoning にフォールバック
        let cap = lookup("codex-mini-latest").unwrap();
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(lookup("foo-bar-3000").is_none());
    }
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
