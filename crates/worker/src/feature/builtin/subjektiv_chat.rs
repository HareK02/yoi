//! Explicit public conversation access for the connected Subject body.
//!
//! This module installs no output or lifecycle hooks. Internal assistant text,
//! WorkerNotify reports and turn completion never become public messages here.
//! Current body ownership, conversation access and observed-input/idempotency checks
//! remain Backend-owned; the model supplies neither sender nor Subject identity.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use server_api::{
    ChatMessagePageQuery, ChatSendMessageRequest, ChatWorkerOperation, ChatWorkerResponse,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::subjektiv::SubjektivHostConnection;

const READ_TOOL: &str = "ConversationRead";
const SEND_TOOL: &str = "ConversationSend";
const READ_DESCRIPTION: &str = "Read a bounded page of committed text-only messages in an authorized conversation for the explicitly connected Subject. Returns the current last_user_message_id; use after_sequence for continuation. This reads public messages, not internal Worker logs or notification-delivery evidence.";
const SEND_DESCRIPTION: &str = "Explicitly send one text-only public message as the connected Subject to an authorized conversation. Use a stable operation_id and the observed expected_last_user_message_id. Sender identity is Host-bound. Exact retries retain the full request and operation_id; conflicts require rereading and reconsideration, while a lost response is not proof of failure. This is the only operation here that publishes assistant text; turn completion and background WorkerNotify reports do not send messages. No attachments or file-path attachment substitution.";
const MAX_TOOL_OUTPUT_BYTES: usize = 128 * 1024;

#[derive(Clone)]
pub(crate) struct SubjektivChatFeature {
    host: SubjektivHostConnection,
}

impl SubjektivChatFeature {
    pub(crate) fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        host: Option<SubjektivHostConnection>,
    ) -> std::io::Result<Option<Self>> {
        if !config.profile.enabled || !config.profile.chat.enabled {
            return Ok(None);
        }
        config.validate_execution().map_err(std::io::Error::other)?;
        let host = host.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv chat requires an explicit live Subject Host connection",
            )
        })?;
        if config.profile.consolidation_tools {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv chat is a body capability, not a consolidation Job capability",
            ));
        }
        Ok(Some(Self { host }))
    }
}

impl FeatureModule for SubjektivChatFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv-chat", "Subject Conversation")
            .with_description("Explicit text-only public conversation read and send operations.")
            .with_tool(ToolDeclaration::new(READ_TOOL, READ_DESCRIPTION))
            .with_tool(ToolDeclaration::new(SEND_TOOL, SEND_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for operation in [Operation::Read, Operation::Send] {
            context.tools().register(ToolContribution::new(
                operation.name(),
                tool_definition(self.host.clone(), operation),
            ))?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Operation {
    Read,
    Send,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Read => READ_TOOL,
            Self::Send => SEND_TOOL,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Read => READ_DESCRIPTION,
            Self::Send => SEND_DESCRIPTION,
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadParams {
    conversation_id: String,
    after_sequence: Option<u64>,
    limit: Option<u32>,
}

fn schema<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("serializable chat Tool schema")
}

fn tool_definition(host: SubjektivHostConnection, operation: Operation) -> ToolDefinition {
    Arc::new(move || {
        let schema = match operation {
            Operation::Read => schema::<ReadParams>(),
            Operation::Send => schema::<ChatSendMessageRequest>(),
        };
        let tool: Arc<dyn Tool> = Arc::new(ConversationTool {
            host: host.clone(),
            operation,
        });
        (
            ToolMeta::new(operation.name())
                .description(operation.description())
                .input_schema(schema),
            tool,
        )
    })
}

struct ConversationTool {
    host: SubjektivHostConnection,
    operation: Operation,
}

fn parse<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

#[async_trait]
impl Tool for ConversationTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let operation = match self.operation {
            Operation::Read => {
                let input: ReadParams = parse(input_json)?;
                if input.conversation_id.trim().is_empty()
                    || input.limit.is_some_and(|limit| {
                        !(1..=server_api::CHAT_MAX_PAGE_LIMIT).contains(&limit)
                    })
                {
                    return Err(ToolError::InvalidArgument(
                        "conversation_id is required and limit must be within 1..=100".into(),
                    ));
                }
                ChatWorkerOperation::Read {
                    conversation_id: input.conversation_id,
                    query: ChatMessagePageQuery {
                        after_sequence: input.after_sequence,
                        // Small default pages keep the chat body responsive; callers
                        // can explicitly request more within the Backend ceiling.
                        limit: Some(input.limit.unwrap_or(5)),
                    },
                }
            }
            Operation::Send => {
                let request: ChatSendMessageRequest = parse(input_json)?;
                if request.conversation_id.trim().is_empty()
                    || request.operation_id.trim().is_empty()
                    || request.text.trim().is_empty()
                    || request.text.len() > server_api::CHAT_MAX_TEXT_BYTES
                {
                    return Err(ToolError::InvalidArgument(
                        "conversation_id, operation_id and nonblank text are required; text exceeds the limit if larger than 16 KiB".into(),
                    ));
                }
                ChatWorkerOperation::Send { request }
            }
        };
        let response = self.host.chat(operation).map_err(ToolError::from)?;
        let (summary, value) = match (self.operation, response) {
            (Operation::Read, ChatWorkerResponse::Messages { page }) => (
                format!(
                    "Read {} committed conversation message(s).",
                    page.messages.len()
                ),
                serde_json::to_value(page),
            ),
            (Operation::Send, ChatWorkerResponse::Sent { message }) => (
                "Committed conversation message.".to_string(),
                serde_json::to_value(message),
            ),
            _ => {
                return Err(ToolError::ExecutionFailed(
                    "unexpected chat Backend response".into(),
                ));
            }
        };
        let content = serde_json::to_string(
            &value.map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
        )
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if content.len() > MAX_TOOL_OUTPUT_BYTES {
            return Err(ToolError::ExecutionFailed(
                "conversation page exceeds the model-visible output budget; read a smaller page"
                    .into(),
            ));
        }
        Ok(ToolOutput {
            summary,
            content: Some(content),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests;
