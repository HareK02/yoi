//! Public text-only conversations. Stored messages are not Runtime delivery receipts.
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

pub const CHAT_MAX_TEXT_BYTES: usize = 16 * 1024;
pub const CHAT_MAX_PAGE_LIMIT: u32 = 100;
pub const CHAT_DEFAULT_PAGE_LIMIT: u32 = 50;
/// Largest counter representable without loss by the JSON/browser contract.
pub const CHAT_MAX_COUNTER: u64 = 9_007_199_254_740_991;

fn bounded_counter<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    if value > CHAT_MAX_COUNTER {
        return Err(serde::de::Error::custom(
            "chat counter exceeds the JSON safe-integer range",
        ));
    }
    Ok(value)
}

fn optional_bounded_counter<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    let value = Option::<u64>::deserialize(deserializer)?;
    if value.is_some_and(|value| value > CHAT_MAX_COUNTER) {
        return Err(serde::de::Error::custom(
            "chat counter exceeds the JSON safe-integer range",
        ));
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatConversation {
    pub id: String,
    pub subject_id: String,
    pub participant_account_id: String,
    /// Latest user input in this conversation; empty conversations use null.
    pub last_user_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatMessageSender {
    User { account_id: String },
    Subject { subject_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatMessage {
    pub id: String,
    pub conversation_id: String,
    #[serde(deserialize_with = "bounded_counter")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub sequence: u64,
    pub sender: ChatMessageSender,
    pub text: String,
    pub reply_to: Option<String>,
    /// Opaque related-work reference, not a grant or an attachment path.
    pub work_ref: Option<String>,
    pub operation_id: String,
    /// User messages refer to themselves; Subject messages pin the latest observed user input.
    pub last_user_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatCreateConversationRequest {
    pub operation_id: String,
    pub subject_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatUserMessageRequest {
    pub operation_id: String,
    pub text: String,
    pub reply_to: Option<String>,
    pub work_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatSendMessageRequest {
    pub conversation_id: String,
    pub operation_id: String,
    pub text: String,
    pub reply_to: Option<String>,
    pub work_ref: Option<String>,
    /// Observed user input, independent of the optional reply target. Empty conversations use null.
    #[serde(deserialize_with = "required_user_message_identity")]
    #[schemars(required)]
    pub expected_last_user_message_id: Option<String>,
}

// An explicit null is an observation of an empty conversation; omission is not.
fn required_user_message_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatMessagePageQuery {
    #[serde(default, deserialize_with = "optional_bounded_counter")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub after_sequence: Option<u64>,
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatMessagePage {
    pub conversation: ChatConversation,
    pub messages: Vec<ChatMessage>,
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_only_requests_reject_attachments_and_claimed_sender() {
        for field in ["attachments", "sender", "subject_id", "account_id"] {
            let mut input = json!({"operation_id":"op", "text":"hello"});
            input[field] = json!([]);
            assert!(serde_json::from_value::<ChatUserMessageRequest>(input).is_err());
        }
        for field in ["attachments", "sender", "subject_id", "account_id"] {
            let mut send = json!({"conversation_id":"c", "operation_id":"op", "text":"hello", "expected_last_user_message_id":null});
            send[field] = json!([]);
            assert!(serde_json::from_value::<ChatSendMessageRequest>(send).is_err());
        }
        assert!(
            serde_json::from_value::<ChatCreateConversationRequest>(
                json!({"operation_id":"op", "subject_id":"s", "participant_account_id":"forged"})
            )
            .is_err()
        );
    }

    #[test]
    fn browser_counters_reject_lossy_json_and_schema_keeps_numeric_bounds() {
        assert!(
            serde_json::from_value::<ChatMessagePageQuery>(
                json!({"after_sequence":CHAT_MAX_COUNTER+1})
            )
            .is_err()
        );
        let schema = serde_json::to_value(schemars::schema_for!(ChatMessage)).unwrap();
        assert_eq!(
            schema["properties"]["sequence"]["maximum"],
            json!(CHAT_MAX_COUNTER)
        );
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn observed_user_identity_is_nullable_text_not_a_number() {
        assert!(
            serde_json::from_value::<ChatSendMessageRequest>(
                json!({"conversation_id":"c", "operation_id":"op", "text":"answer"})
            )
            .is_err()
        );
        for baseline in [serde_json::Value::Null, json!("user-message-id")] {
            let value = json!({"conversation_id":"c", "operation_id":"op", "text":"answer", "expected_last_user_message_id":baseline});
            let request: ChatSendMessageRequest = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(request).unwrap()["expected_last_user_message_id"],
                baseline
            );
        }
        assert!(serde_json::from_value::<ChatSendMessageRequest>(json!({"conversation_id":"c", "operation_id":"op", "text":"answer", "expected_last_user_message_id":1})).is_err());
        let mut message = json!({"id":"m", "conversation_id":"c", "sequence":CHAT_MAX_COUNTER, "sender":{"kind":"subject","subject_id":"s"}, "text":"answer", "operation_id":"op", "last_user_message_id":null});
        assert!(serde_json::from_value::<ChatMessage>(message.clone()).is_ok());
        message["sequence"] = json!(CHAT_MAX_COUNTER + 1);
        assert!(serde_json::from_value::<ChatMessage>(message).is_err());
        for baseline in [serde_json::Value::Null, json!("user-message-id")] {
            let conversation: ChatConversation = serde_json::from_value(json!({"id":"c", "subject_id":"s", "participant_account_id":"a", "last_user_message_id":baseline})).unwrap();
            assert_eq!(
                serde_json::to_value(conversation).unwrap()["last_user_message_id"],
                baseline
            );
        }
    }

    #[test]
    fn sender_boundary_rejects_extra_fields_and_unknown_kinds() {
        assert!(
            serde_json::from_value::<ChatMessageSender>(
                json!({"kind":"user","account_id":"a","subject_id":"s"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<ChatMessageSender>(json!({"kind":"worker","account_id":"a"}))
                .is_err()
        );
        let sender = ChatMessageSender::Subject {
            subject_id: "s".into(),
        };
        assert_eq!(
            serde_json::to_value(sender).unwrap(),
            json!({"kind":"subject","subject_id":"s"})
        );
    }
}
