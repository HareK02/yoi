//! Chat publication and Notify admission are independent results. No delivery state
//! below asserts committed Session incorporation, model processing or task completion.
use crate::{ChatMessage, ChatMessagePage, ChatMessagePageQuery, ChatSendMessageRequest};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatDelivery {
    NoCurrentBody,
    Accepted {
        runtime_id: String,
        worker_id: String,
    },
    Rejected {
        runtime_id: String,
        worker_id: String,
        detail: String,
    },
    Unknown {
        runtime_id: String,
        worker_id: String,
        detail: String,
    },
    Unavailable {
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatReceiveResponse {
    pub message: ChatMessage,
    pub delivery: ChatDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ChatWorkerRequest {
    pub operation: ChatWorkerOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatWorkerOperation {
    Read {
        conversation_id: String,
        query: ChatMessagePageQuery,
    },
    Send {
        request: ChatSendMessageRequest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ChatWorkerResponse {
    Messages { page: ChatMessagePage },
    Sent { message: ChatMessage },
}
