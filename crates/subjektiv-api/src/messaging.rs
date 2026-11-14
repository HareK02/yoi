//! Dedicated conversation commands and committed pages over the Workspace multiplexer.
//!
//! Subscription cursors are saved sequence observations, not contiguous counters. A page is
//! recoverable from storage; neither its arrival nor a Notify receipt asserts model processing.
use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MessagingSelector {
    pub subject_id: String,
    pub conversation_id: String,
    #[serde(deserialize_with = "crate::chat::bounded_counter")]
    #[schemars(range(min = 0, max = 9_007_199_254_740_991_u64))]
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub after_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MessagingRequest {
    pub request_id: String,
    pub command: MessagingCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessagingCommand {
    Open {
        request: ChatCreateConversationRequest,
    },
    History {
        selector: MessagingSelector,
        query: ChatMessagePageQuery,
    },
    Post {
        selector: MessagingSelector,
        request: ChatUserMessageRequest,
    },
    Subscribe {
        selector: MessagingSelector,
    },
    Unsubscribe {
        subscription_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MessagingResponse {
    pub request_id: String,
    pub result: MessagingResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessagingResult {
    Opened {
        conversation: ChatConversation,
    },
    History {
        page: ChatMessagePage,
    },
    Saved {
        response: ChatReceiveResponse,
    },
    Subscribed {
        subscription_id: String,
        selector: MessagingSelector,
        page: ChatMessagePage,
    },
    Unsubscribed {
        subscription_id: String,
    },
    Rejected {
        status: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MessagingEvent {
    Messages {
        subscription_id: String,
        page: ChatMessagePage,
    },
    Closed {
        subscription_id: String,
        reason: MessagingTermination,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MessagingTermination {
    AccessRevoked,
    Lagged,
    ServerShutdown,
}

/// These limits are shared with storage admission. IDs are opaque, not authority claims.
pub fn valid_chat_identifier(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}
pub fn valid_chat_text(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= CHAT_MAX_TEXT_BYTES
}
impl MessagingSelector {
    pub fn validate(&self) -> bool {
        valid_chat_identifier(&self.subject_id)
            && valid_chat_identifier(&self.conversation_id)
            && self.after_sequence <= CHAT_MAX_COUNTER
    }
    pub fn matches(&self, page: &ChatMessagePage) -> bool {
        page.conversation.id == self.conversation_id
            && page.conversation.subject_id == self.subject_id
            && valid_chat_page(page, self.after_sequence)
    }
}
impl MessagingRequest {
    pub fn validate(&self) -> bool {
        valid_chat_identifier(&self.request_id)
            && match &self.command {
                MessagingCommand::Open { request } => {
                    valid_chat_identifier(&request.operation_id)
                        && valid_chat_identifier(&request.subject_id)
                }
                MessagingCommand::History { selector, query } => {
                    selector.validate()
                        && valid_chat_query(query)
                        && query.after_sequence.unwrap_or(0) == selector.after_sequence
                }
                MessagingCommand::Post { selector, request } => {
                    selector.validate()
                        && valid_chat_identifier(&request.operation_id)
                        && valid_chat_text(&request.text)
                        && valid_reference(&request.reply_to)
                        && valid_reference(&request.work_ref)
                }
                MessagingCommand::Subscribe { selector } => selector.validate(),
                MessagingCommand::Unsubscribe { subscription_id } => {
                    valid_chat_identifier(subscription_id)
                }
            }
    }
}
fn valid_reference(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(valid_chat_identifier)
}
pub fn valid_chat_query(query: &ChatMessagePageQuery) -> bool {
    query.after_sequence.is_none_or(|n| n <= CHAT_MAX_COUNTER)
        && query
            .limit
            .is_none_or(|n| (1..=CHAT_MAX_PAGE_LIMIT).contains(&n))
}
pub fn valid_chat_conversation(c: &ChatConversation) -> bool {
    [&c.id, &c.subject_id, &c.participant_account_id]
        .into_iter()
        .all(|v| valid_chat_identifier(v))
        && valid_reference(&c.last_user_message_id)
}
pub fn valid_chat_message(m: &ChatMessage) -> bool {
    [&m.id, &m.conversation_id, &m.operation_id]
        .into_iter()
        .all(|v| valid_chat_identifier(v))
        && valid_chat_text(&m.text)
        && (1..=CHAT_MAX_COUNTER).contains(&m.sequence)
        && valid_reference(&m.reply_to)
        && valid_reference(&m.work_ref)
        && valid_reference(&m.last_user_message_id)
        && match &m.sender {
            ChatMessageSender::User { account_id } => valid_chat_identifier(account_id),
            ChatMessageSender::Subject { subject_id } => valid_chat_identifier(subject_id),
        }
}
pub fn valid_chat_page(p: &ChatMessagePage, after: u64) -> bool {
    let mut previous = after;
    let mut ids = std::collections::HashSet::new();
    valid_chat_conversation(&p.conversation)
        && p.messages.len() <= CHAT_MAX_PAGE_LIMIT as usize
        && (!p.has_more || !p.messages.is_empty())
        && p.messages.iter().all(|m| {
            let valid = valid_chat_message(m)
                && m.conversation_id == p.conversation.id
                && m.sequence > previous
                && ids.insert(&m.id)
                && match &m.sender {
                    ChatMessageSender::User { account_id } => {
                        account_id == &p.conversation.participant_account_id
                    }
                    ChatMessageSender::Subject { subject_id } => {
                        subject_id == &p.conversation.subject_id
                    }
                };
            previous = m.sequence;
            valid
        })
}
impl MessagingResponse {
    pub fn validate(&self) -> bool {
        valid_chat_identifier(&self.request_id)
            && match &self.result {
                MessagingResult::Opened { conversation } => valid_chat_conversation(conversation),
                MessagingResult::History { page } => valid_chat_page(page, 0),
                MessagingResult::Saved { response } => valid_chat_message(&response.message),
                MessagingResult::Subscribed {
                    subscription_id,
                    selector,
                    page,
                } => {
                    valid_chat_identifier(subscription_id)
                        && selector.validate()
                        && selector.matches(page)
                }
                MessagingResult::Unsubscribed { subscription_id } => {
                    valid_chat_identifier(subscription_id)
                }
                MessagingResult::Rejected { status } => (400..=599).contains(status),
            }
    }
}
impl MessagingEvent {
    pub fn subscription_id(&self) -> &str {
        match self {
            Self::Messages {
                subscription_id, ..
            }
            | Self::Closed {
                subscription_id, ..
            } => subscription_id,
        }
    }
    pub fn validate(&self) -> bool {
        valid_chat_identifier(self.subscription_id())
            && match self {
                Self::Messages { page, .. } => valid_chat_page(page, 0),
                Self::Closed { .. } => true,
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn messaging_boundary_rejects_unknown_payloads_and_preserves_sparse_cursor() {
        let wire = json!({"request_id":"request","command":{"kind":"subscribe","selector":{"subject_id":"subject","conversation_id":"conversation","after_sequence":17}}});
        let request: MessagingRequest = serde_json::from_value(wire.clone()).unwrap();
        assert!(request.validate());
        assert_eq!(serde_json::to_value(request).unwrap(), wire);
        for extra in ["account_id", "workspace_id", "worker_id", "queue"] {
            let mut forged = wire.clone();
            forged["command"]["selector"][extra] = json!("forged");
            assert!(serde_json::from_value::<MessagingRequest>(forged).is_err());
        }
        for cursor in [
            json!(-1),
            json!(1.5),
            json!(CHAT_MAX_COUNTER + 1),
            json!(null),
        ] {
            let mut invalid = wire.clone();
            invalid["command"]["selector"]["after_sequence"] = cursor;
            assert!(serde_json::from_value::<MessagingRequest>(invalid).is_err());
        }
        let mut invalid = wire;
        invalid["command"]["kind"] = json!("worker_method");
        assert!(serde_json::from_value::<MessagingRequest>(invalid).is_err());
    }

    #[test]
    fn messaging_and_storage_share_byte_limits_without_loosening_http_admission() {
        let mut request = MessagingRequest {
            request_id: "r".into(),
            command: MessagingCommand::Post {
                selector: MessagingSelector {
                    subject_id: "s".into(),
                    conversation_id: "c".into(),
                    after_sequence: 0,
                },
                request: ChatUserMessageRequest {
                    operation_id: "op".into(),
                    text: "é".repeat(CHAT_MAX_TEXT_BYTES / 2),
                    reply_to: None,
                    work_ref: None,
                },
            },
        };
        assert!(request.validate());
        if let MessagingCommand::Post { request, .. } = &mut request.command {
            request.text.push('x');
        }
        assert!(!request.validate());
        for id in ["\0", "\n", " "] {
            assert!(!valid_chat_identifier(id));
        }
        assert!(!valid_chat_identifier(&"é".repeat(257)));
        assert!(valid_chat_identifier(&"é".repeat(256)));
        // Existing HTTP omission defaults remain valid and serialize to explicit nullable references.
        let http: ChatUserMessageRequest =
            serde_json::from_value(json!({"operation_id":"op","text":"input"})).unwrap();
        assert_eq!(
            serde_json::to_value(http).unwrap(),
            json!({"operation_id":"op","text":"input","reply_to":null,"work_ref":null})
        );
    }

    #[test]
    fn nullable_saved_message_fields_are_required_and_schema_preserves_null() {
        let wire = json!({"id":"m","conversation_id":"c","sequence":7,"sender":{"kind":"subject","subject_id":"s"},"text":"public","reply_to":null,"work_ref":null,"operation_id":"op","last_user_message_id":null});
        let message: ChatMessage = serde_json::from_value(wire.clone()).unwrap();
        assert!(valid_chat_message(&message));
        assert_eq!(serde_json::to_value(message).unwrap(), wire);
        for key in ["reply_to", "work_ref", "last_user_message_id"] {
            let mut missing = wire.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<ChatMessage>(missing).is_err());
        }
        let schema = serde_json::to_value(schemars::schema_for!(ChatMessage)).unwrap();
        assert_eq!(
            schema["properties"]["reply_to"]["type"],
            json!(["string", "null"])
        );
    }
}

#[cfg(test)]
#[test]
fn unicode_whitespace_corpus_preserves_rust_storage_admission() {
    let corpus: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../fixtures/chat-whitespace.json")).unwrap();
    for case in corpus {
        let text = case["text"].as_str().unwrap();
        assert_eq!(
            valid_chat_text(text),
            case["nonblank"].as_bool().unwrap(),
            "{text:?}"
        );
    }
}
