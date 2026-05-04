//! Persistence-stable mirror of `llm_worker::Item`.
//!
//! `LogEntry` does not embed `Item` directly because that couples the on-disk
//! schema to the LLM worker's internal type — a field rename or addition there
//! would break every existing log. Instead, history-bearing variants serialize
//! a [`LoggedItem`] that lives in this crate, and conversions translate at the
//! save / replay boundaries.
//!
//! Fields kept here are limited to what is needed to reconstruct a worker
//! `Item` for replay. `id` and `status` annotations are intentionally dropped
//! (they are output-side metadata; replayed items synthesize fresh `None`).
//! `Reasoning::encrypted_content` is preserved because OpenAI Responses ZDR
//! requires it on stateless re-send.

use llm_worker::llm_client::types::{ContentPart, Item, Role};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedItem {
    Message {
        role: LoggedRole,
        content: Vec<LoggedContentPart>,
    },
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolResult {
        call_id: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        summary: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        encrypted_content: Option<String>,
        /// Anthropic extended thinking signature。新世代 Claude で round-trip
        /// 必須。OpenAI Responses など他 scheme では `None`。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoggedRole {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedContentPart {
    Text { text: String },
    Refusal { refusal: String },
}

// ---------------------------------------------------------------------------
// Item ↔ LoggedItem
// ---------------------------------------------------------------------------

impl From<&Item> for LoggedItem {
    fn from(item: &Item) -> Self {
        match item {
            Item::Message { role, content, .. } => Self::Message {
                role: (*role).into(),
                content: content.iter().map(LoggedContentPart::from).collect(),
            },
            Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => Self::ToolCall {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments: arguments.clone(),
            },
            Item::ToolResult {
                call_id,
                summary,
                content,
                ..
            } => Self::ToolResult {
                call_id: call_id.clone(),
                summary: summary.clone(),
                content: content.clone(),
            },
            Item::Reasoning {
                text,
                summary,
                encrypted_content,
                signature,
                ..
            } => Self::Reasoning {
                text: text.clone(),
                summary: summary.clone(),
                encrypted_content: encrypted_content.clone(),
                signature: signature.clone(),
            },
        }
    }
}

impl From<Item> for LoggedItem {
    fn from(item: Item) -> Self {
        Self::from(&item)
    }
}

impl From<LoggedItem> for Item {
    fn from(logged: LoggedItem) -> Self {
        match logged {
            LoggedItem::Message { role, content } => Item::Message {
                id: None,
                role: role.into(),
                content: content.into_iter().map(Into::into).collect(),
                status: None,
            },
            LoggedItem::ToolCall {
                call_id,
                name,
                arguments,
            } => Item::ToolCall {
                id: None,
                call_id,
                name,
                arguments,
                status: None,
            },
            LoggedItem::ToolResult {
                call_id,
                summary,
                content,
            } => Item::ToolResult {
                id: None,
                call_id,
                summary,
                content,
            },
            LoggedItem::Reasoning {
                text,
                summary,
                encrypted_content,
                signature,
            } => Item::Reasoning {
                id: None,
                text,
                summary,
                encrypted_content,
                signature,
                status: None,
            },
        }
    }
}

/// Convert a slice of worker items into logged form.
pub fn to_logged(items: &[Item]) -> Vec<LoggedItem> {
    items.iter().map(LoggedItem::from).collect()
}

/// Convert logged items back into worker form.
pub fn from_logged(items: Vec<LoggedItem>) -> Vec<Item> {
    items.into_iter().map(Item::from).collect()
}

// ---------------------------------------------------------------------------
// Role ↔ LoggedRole
// ---------------------------------------------------------------------------

impl From<Role> for LoggedRole {
    fn from(role: Role) -> Self {
        match role {
            Role::User => Self::User,
            Role::Assistant => Self::Assistant,
            Role::System => Self::System,
        }
    }
}

impl From<LoggedRole> for Role {
    fn from(role: LoggedRole) -> Self {
        match role {
            LoggedRole::User => Self::User,
            LoggedRole::Assistant => Self::Assistant,
            LoggedRole::System => Self::System,
        }
    }
}

// ---------------------------------------------------------------------------
// ContentPart ↔ LoggedContentPart
// ---------------------------------------------------------------------------

impl From<&ContentPart> for LoggedContentPart {
    fn from(part: &ContentPart) -> Self {
        match part {
            ContentPart::Text { text } => Self::Text { text: text.clone() },
            ContentPart::Refusal { refusal } => Self::Refusal {
                refusal: refusal.clone(),
            },
        }
    }
}

impl From<LoggedContentPart> for ContentPart {
    fn from(part: LoggedContentPart) -> Self {
        match part {
            LoggedContentPart::Text { text } => Self::Text { text },
            LoggedContentPart::Refusal { refusal } => Self::Refusal { refusal },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_user_message_text() {
        let original = Item::user_message("hello");
        let logged: LoggedItem = (&original).into();
        let restored: Item = logged.into();
        // id / status are dropped by design; compare semantically.
        match restored {
            Item::Message { role, content, .. } => {
                assert_eq!(role, Role::User);
                assert_eq!(content.len(), 1);
                match &content[0] {
                    ContentPart::Text { text } => assert_eq!(text, "hello"),
                    other => panic!("unexpected content: {other:?}"),
                }
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn round_trip_tool_call() {
        let original = Item::tool_call("call_42", "get_weather", r#"{"city":"Tokyo"}"#);
        let logged: LoggedItem = (&original).into();
        let json = serde_json::to_string(&logged).unwrap();
        let parsed: LoggedItem = serde_json::from_str(&json).unwrap();
        match Item::from(parsed) {
            Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                assert_eq!(call_id, "call_42");
                assert_eq!(name, "get_weather");
                assert_eq!(arguments, r#"{"city":"Tokyo"}"#);
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn round_trip_reasoning_preserves_encrypted_content() {
        let original = Item::reasoning("step-by-step")
            .with_reasoning_summary(vec!["s1".into(), "s2".into()])
            .with_encrypted_content("opaque-blob");
        let logged: LoggedItem = (&original).into();
        let json = serde_json::to_string(&logged).unwrap();
        let parsed: LoggedItem = serde_json::from_str(&json).unwrap();
        match Item::from(parsed) {
            Item::Reasoning {
                text,
                summary,
                encrypted_content,
                ..
            } => {
                assert_eq!(text, "step-by-step");
                assert_eq!(summary, vec!["s1".to_string(), "s2".to_string()]);
                assert_eq!(encrypted_content.as_deref(), Some("opaque-blob"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn round_trip_tool_result_with_content() {
        let original = Item::tool_result_with_content("call_1", "ok", "full output");
        let logged: LoggedItem = (&original).into();
        match Item::from(logged) {
            Item::ToolResult {
                call_id,
                summary,
                content,
                ..
            } => {
                assert_eq!(call_id, "call_1");
                assert_eq!(summary, "ok");
                assert_eq!(content.as_deref(), Some("full output"));
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn message_serialization_uses_kind_tag() {
        let logged: LoggedItem = (&Item::assistant_message("hi")).into();
        let value: serde_json::Value = serde_json::to_value(&logged).unwrap();
        assert_eq!(value["kind"], "message");
        assert_eq!(value["role"], "assistant");
        assert_eq!(value["content"][0]["kind"], "text");
        assert_eq!(value["content"][0]["text"], "hi");
    }
}
