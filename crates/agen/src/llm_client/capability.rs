//! Model capability metadata.
//!
//! `ModelCapability` represents differences in model features. Reasoning support
//! and prompt caching strategies may vary even within one scheme, so this
//! metadata is kept separate from the scheme.
//!
//! The client construction layer supplies model-specific capabilities and overrides.
//! Schemes provide only wire-level defaults through `Scheme::default_capability`;
//! this module does not own a model registry or application configuration format.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Model capability metadata.
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
    /// Conservative default with no supported features, used for unknown models.
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

/// Tool calling support.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallingSupport {
    /// Unsupported.
    None,
    /// Only one tool per response.
    Sequential,
    /// Multiple parallel tools per response.
    Parallel,
}

/// Structured output support.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StructuredOutput {
    None,
    /// `json_object` mode (enforces JSON without a schema).
    JsonObject,
    /// Structured output with a JSON Schema.
    JsonSchema,
}

/// Reasoning (extended thinking) support.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningSupport {
    /// OpenAI format: `reasoning.effort` (low/medium/high).
    Effort,
    /// Anthropic format: `thinking.budget_tokens`.
    BudgetTokens,
    /// Both (handled internally as shared `ReasoningControl`, projected by each scheme).
    Both,
}

/// Prompt caching strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CacheStrategy {
    /// Anthropic: insert explicit `cache_control` markers.
    Explicit { max_breakpoints: u8 },
    /// Other providers: automatic server-side prefix caching, or unsupported.
    Auto,
}

/// Shared reasoning control, projected into each provider's format by the scheme.
///
/// Strings are provider-native effort labels; numbers are provider-native
/// thinking token budgets. The type represents only one at a time.
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
