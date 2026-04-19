//! モデル能力メタデータ
//!
//! `ModelCapability` はモデルが持つ機能差を表現する。scheme は同じでも
//! モデルごとに reasoning 可否や prompt caching 方式が違うため、scheme
//! から分離して保持する。
//!
//! 値の供給経路は 2 通り:
//! 1. scheme 実装側の `model_id → ModelCapability` 静的テーブル（既知モデル）
//! 2. `ModelConfig::capability` での明示 override（未知モデル、または上書き）

use serde::{Deserialize, Serialize};

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

/// Reasoning 制御（共通型、scheme 側で各社形式に投影）
///
/// `effort` / `budget_tokens` はユーザー設定から任意で渡される。Scheme
/// 側は自身の `ReasoningSupport` に応じて片方だけ使う。両方が宣言
/// されている場合の優先順位は scheme 実装が決める。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReasoningControl {
    #[serde(default)]
    pub effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub budget_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
}
