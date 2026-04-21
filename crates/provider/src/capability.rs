//! `SchemeKind` × `model_id` → [`ModelCapability`] の既知モデル静的テーブル。
//!
//! このテーブルは「モデル ID の知識」であり、wire 実装(`llm-worker`)の
//! 責務ではなく高レベル構築層(`crates/provider`)の責務として置く。
//! llm-worker の scheme には `default_capability()` のみ残し、未知モデル
//! 時は scheme 既定にフォールバックする。
//!
//! 解決順(`build_client` が呼ぶ):
//!   1. `ModelConfig.capability` 明示指定
//!   2. [`lookup`] (本モジュール)
//!   3. `Scheme::default_capability()` (llm-worker)

use llm_worker::llm_client::capability::{
    CacheStrategy, ModelCapability, ReasoningSupport, StructuredOutput, ToolCallingSupport,
};
use manifest::SchemeKind;

/// `scheme` と `model_id` から既知モデルの capability を返す。
/// 未知なら `None`(呼び出し側が `Scheme::default_capability()` へフォールバック)。
pub fn lookup(scheme: SchemeKind, model_id: &str) -> Option<ModelCapability> {
    match scheme {
        SchemeKind::Anthropic => anthropic_lookup(model_id),
        SchemeKind::OpenaiChat => openai_chat_lookup(model_id),
        SchemeKind::OpenaiResponses => openai_responses_lookup(model_id),
        SchemeKind::Gemini => gemini_lookup(model_id),
    }
}

// --- Anthropic --------------------------------------------------------------

fn anthropic_lookup(model_id: &str) -> Option<ModelCapability> {
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

// --- OpenAI (chat / responses で共有の family 判定) --------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiFamily {
    /// GPT-5 / o1 / o3 / o4 系 — reasoning 対応
    Reasoning,
    /// GPT-4o / GPT-4 系
    Gpt4,
    /// GPT-3.5 系(旧式)
    Gpt35,
}

fn openai_classify(model_id: &str) -> Option<OpenAiFamily> {
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

fn openai_chat_lookup(model_id: &str) -> Option<ModelCapability> {
    openai_classify(model_id).map(|family| match family {
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

fn openai_responses_lookup(model_id: &str) -> Option<ModelCapability> {
    // `codex-` prefix は ChatGPT backend 経由(CodexOAuth)でのみ使える
    // Reasoning モデル family。`gpt-5` 系と同じ扱い。
    let family = openai_classify(model_id).or_else(|| {
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

// --- Gemini -----------------------------------------------------------------

fn gemini_lookup(model_id: &str) -> Option<ModelCapability> {
    if !model_id.starts_with("gemini-") {
        return None;
    }
    // 2.5 系以降は thinking / reasoning を持つ
    let reasoning = if model_id.starts_with("gemini-2.5") || model_id.starts_with("gemini-3") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_known_claude() {
        let cap = lookup(SchemeKind::Anthropic, "claude-sonnet-4-6").unwrap();
        assert!(matches!(
            cap.prompt_caching,
            CacheStrategy::Explicit { max_breakpoints: 4 }
        ));
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn anthropic_unknown_is_none() {
        assert!(lookup(SchemeKind::Anthropic, "llama3").is_none());
    }

    #[test]
    fn openai_chat_gpt5_is_reasoning() {
        let cap = lookup(SchemeKind::OpenaiChat, "gpt-5-xyz").unwrap();
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn openai_responses_codex_prefix_is_reasoning() {
        let cap = lookup(SchemeKind::OpenaiResponses, "codex-mini-latest").unwrap();
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn openai_responses_gpt5_codex_is_reasoning() {
        // gpt-5 prefix 経由で classify される
        let cap = lookup(SchemeKind::OpenaiResponses, "gpt-5-codex").unwrap();
        assert!(cap.reasoning.is_some());
    }

    #[test]
    fn gemini_25_has_reasoning() {
        let cap = lookup(SchemeKind::Gemini, "gemini-2.5-pro").unwrap();
        assert!(matches!(cap.reasoning, Some(ReasoningSupport::BudgetTokens)));
    }

    #[test]
    fn gemini_legacy_has_no_reasoning() {
        let cap = lookup(SchemeKind::Gemini, "gemini-1.5-pro").unwrap();
        assert!(cap.reasoning.is_none());
    }

    #[test]
    fn unknown_model_is_none_across_schemes() {
        assert!(lookup(SchemeKind::OpenaiChat, "foo-bar").is_none());
        assert!(lookup(SchemeKind::OpenaiResponses, "foo-bar").is_none());
        assert!(lookup(SchemeKind::Gemini, "foo-bar").is_none());
    }
}
