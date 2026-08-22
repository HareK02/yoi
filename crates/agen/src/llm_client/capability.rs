//! モデル能力メタデータ
//!
//! `ModelCapability` はモデルが持つ機能差を表現する。scheme は同じでも
//! モデルごとに reasoning 可否や prompt caching 方式が違うため、scheme
//! から分離して保持する。
//!
//! 値の供給経路は 2 通り:
//! 1. scheme 実装側の `model_id → ModelCapability` 静的テーブル（既知モデル）
//! 2. `ModelConfig::capability` での明示 override（未知モデル、または上書き）

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// モデル能力メタデータ
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapability {
    pub tool_calling: ToolCallingSupport,
    pub structured_output: StructuredOutput,
    #[serde(default)]
    pub reasoning: Option<ReasoningSupport>,
    #[serde(default)]
    pub vision: bool,
    pub prompt_caching: CacheStrategy,
}

impl ModelCapability {
    /// 何もサポートしない安全側デフォルト。未知モデルのフォールバック用。
    pub const fn minimal() -> Self {
        Self {
            tool_calling: ToolCallingSupport::None,
            structured_output: StructuredOutput::None,
            reasoning: None,
            vision: false,
            prompt_caching: CacheStrategy::Auto,
        }
    }
}

/// ツール呼び出しサポート
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallingSupport {
    /// 非サポート
    None,
    /// 1 回のレスポンスで 1 ツールのみ
    Sequential,
    /// 1 回のレスポンスで複数ツール並行
    Parallel,
}

/// Structured output サポート
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutput {
    None,
    /// `json_object` モード（スキーマなし JSON 強制）
    JsonObject,
    /// JSON Schema 指定で構造化出力
    JsonSchema,
}

/// Reasoning（extended thinking）サポート
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSupport {
    /// OpenAI 形式: `reasoning.effort` (low/medium/high)
    Effort,
    /// Anthropic 形式: `thinking.budget_tokens`
    BudgetTokens,
    /// 両対応（内部では共通 `ReasoningControl` として扱い、各 scheme で投影）
    Both,
}

/// Prompt caching 戦略
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CacheStrategy {
    /// Anthropic: `cache_control` マーカーを明示挿入
    Explicit { max_breakpoints: u8 },
    /// それ以外: サーバ側自動 prefix、または未サポート
    Auto,
}

/// Reasoning 制御（共通型、scheme 側で各社形式に投影）。
///
/// 文字列は provider-native な effort label、数値は provider-native な
/// thinking budget token として扱う。どちらか一方だけを型で表現する。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ReasoningControl {
    Effort(ReasoningEffort),
    BudgetTokens(i32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReasoningEffort {
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Other(String),
}

impl ReasoningEffort {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Other(label) => label.as_str(),
        }
    }
}

impl From<String> for ReasoningEffort {
    fn from(value: String) -> Self {
        match value.as_str() {
            "minimal" => Self::Minimal,
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "xhigh" => Self::XHigh,
            _ => Self::Other(value),
        }
    }
}

impl Serialize for ReasoningEffort {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ReasoningEffort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::{ReasoningControl, ReasoningEffort};

    #[test]
    fn reasoning_control_deserializes_effort_labels() {
        let known: ReasoningControl = serde_json::from_str(r#""xhigh""#).unwrap();
        assert_eq!(known, ReasoningControl::Effort(ReasoningEffort::XHigh));

        let unknown: ReasoningControl = serde_json::from_str(r#""provider-native""#).unwrap();
        assert_eq!(
            unknown,
            ReasoningControl::Effort(ReasoningEffort::Other("provider-native".into()))
        );
    }

    #[test]
    fn reasoning_control_deserializes_signed_budget() {
        let dynamic: ReasoningControl = serde_json::from_str("-1").unwrap();
        assert_eq!(dynamic, ReasoningControl::BudgetTokens(-1));
    }
}
