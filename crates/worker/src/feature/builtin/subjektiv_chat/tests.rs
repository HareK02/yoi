use super::*;
use crate::worker::{WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
use server_api::{
    ChatConversation, ChatMessage, ChatMessagePage, ChatMessageSender, ChatWorkerRequest,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicUsize, Ordering};

#[derive(Debug)]
struct ChatBackendFixture {
    allowed: Mutex<Vec<&'static str>>,
    requests: Mutex<Vec<WorkspaceRequest>>,
    status: AtomicU16,
    unavailable: AtomicBool,
    wrong_variant: AtomicBool,
    page_text_bytes: AtomicUsize,
}

impl Default for ChatBackendFixture {
    fn default() -> Self {
        Self {
            allowed: Mutex::new(vec!["chat.read", "chat.send"]),
            requests: Mutex::new(Vec::new()),
            status: AtomicU16::new(200),
            unavailable: AtomicBool::new(false),
            wrong_variant: AtomicBool::new(false),
            page_text_bytes: AtomicUsize::new(0),
        }
    }
}

fn message(request: ChatSendMessageRequest) -> ChatMessage {
    ChatMessage {
        id: "message-committed".into(),
        conversation_id: request.conversation_id,
        sequence: 7,
        sender: ChatMessageSender::Subject {
            subject_id: "subject-fixture".into(),
        },
        text: request.text,
        reply_to: request.reply_to,
        work_ref: request.work_ref,
        operation_id: request.operation_id,
        last_user_message_id: request.expected_last_user_message_id,
    }
}

fn send_input() -> serde_json::Value {
    serde_json::json!({
        "conversation_id": "conversation-1",
        "operation_id": "stable-operation-1",
        "text": "I evaluated the background report.",
        "reply_to": "user-message-1",
        "work_ref": "T-766",
        "expected_last_user_message_id": "user-message-3"
    })
}

impl WorkspaceClient for ChatBackendFixture {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-chat")
    }
    fn kind(&self) -> &str {
        "chat-test"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.requests.lock().unwrap().push(request.clone());
        if let Some(response) = crate::subjektiv::test_subject_authorization_response(
            &request,
            "workspace-chat",
            &self.allowed.lock().unwrap(),
        ) {
            return Ok(response);
        }
        assert_eq!(request.path, "/api/w/workspace-chat/subjektiv/chat/worker");
        assert_eq!(request.method, crate::WorkspaceRequestMethod::Post);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(WorkspaceClientError::Unavailable(
                "response lost; acceptance unknown".into(),
            ));
        }
        let status = self.status.load(Ordering::SeqCst);
        if status != 200 {
            return Ok(WorkspaceResponse {
                status,
                body: serde_json::json!({"message":"Backend rejected chat operation"}).to_string(),
            });
        }
        let input: ChatWorkerRequest =
            serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
        let response = match input.operation {
            ChatWorkerOperation::Send { request } if !self.wrong_variant.load(Ordering::SeqCst) => {
                ChatWorkerResponse::Sent {
                    message: message(request),
                }
            }
            _ => ChatWorkerResponse::Messages {
                page: ChatMessagePage {
                    conversation: ChatConversation {
                        id: "conversation-1".into(),
                        subject_id: "subject-fixture".into(),
                        participant_account_id: "account-fixture".into(),
                        last_user_message_id: Some("user-message-3".into()),
                    },
                    messages: if self.page_text_bytes.load(Ordering::SeqCst) > 0 {
                        let mut request: ChatSendMessageRequest =
                            serde_json::from_value(send_input()).unwrap();
                        request.text = "x".repeat(self.page_text_bytes.load(Ordering::SeqCst));
                        vec![message(request)]
                    } else {
                        Vec::new()
                    },
                    has_more: false,
                },
            },
        };
        Ok(WorkspaceResponse {
            status: 200,
            body: serde_json::to_string(&response).unwrap(),
        })
    }
}

impl ChatBackendFixture {
    fn chat_requests(&self) -> Vec<ChatWorkerRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.path.ends_with("/chat/worker"))
            .map(|request| serde_json::from_str(request.body.as_deref().unwrap()).unwrap())
            .collect()
    }
}

fn tool(client: Arc<ChatBackendFixture>, operation: Operation) -> ConversationTool {
    ConversationTool {
        host: crate::subjektiv::test_connection(client),
        operation,
    }
}

async fn invoke(
    tool: &ConversationTool,
    input: serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    tool.execute(
        &input.to_string(),
        ToolExecutionContext::new("call-chat", "batch", 0),
    )
    .await
}

#[test]
fn feature_is_opt_in_and_requires_explicit_connection_without_hooks() {
    let mut config = manifest::ResolvedSubjektivFeatureConfig::default();
    config.profile.enabled = true;
    assert!(
        SubjektivChatFeature::from_resolved_config(&config, None)
            .unwrap()
            .is_none()
    );
    config.profile.chat.enabled = true;
    assert!(
        SubjektivChatFeature::from_resolved_config(&config, None)
            .err()
            .unwrap()
            .to_string()
            .contains("explicit live Subject")
    );
    let host = crate::subjektiv::test_connection(Arc::new(ChatBackendFixture::default()));
    let feature = SubjektivChatFeature::from_resolved_config(&config, Some(host.clone()))
        .unwrap()
        .unwrap();
    let descriptor = feature.descriptor();
    assert_eq!(
        descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [READ_TOOL, SEND_TOOL]
    );
    assert!(
        descriptor.hooks.is_empty(),
        "no assistant/output/turn hooks may publish chat"
    );
    config.profile.consolidation_tools = true;
    assert!(SubjektivChatFeature::from_resolved_config(&config, Some(host)).is_err());
}

#[tokio::test]
async fn registry_installs_only_explicit_tools_and_does_no_startup_io() {
    let client = Arc::new(ChatBackendFixture::default());
    let feature = SubjektivChatFeature {
        host: crate::subjektiv::test_connection(client.clone()),
    };
    let mut tools = Vec::new();
    let mut hooks = crate::hook::HookRegistryBuilder::new();
    let report = crate::feature::FeatureRegistryBuilder::new()
        .with_module(feature)
        .install_into_pending(&mut tools, &mut hooks);
    assert!(!report.has_errors());
    assert_eq!(
        tools.iter().map(|tool| tool().0.name).collect::<Vec<_>>(),
        [READ_TOOL, SEND_TOOL]
    );
    assert!(client.requests.lock().unwrap().is_empty());
    let (_, read) = tools[0]();
    let (_, send) = tools[1]();
    read.execute(
        r#"{"conversation_id":"conversation-1"}"#,
        ToolExecutionContext::new("read", "batch", 0),
    )
    .await
    .unwrap();
    for ordinal in 0..2 {
        let mut input = send_input();
        input["operation_id"] = serde_json::json!(format!("explicit-send-{ordinal}"));
        send.execute(
            &input.to_string(),
            ToolExecutionContext::new("send", "batch", ordinal),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        client.chat_requests().len(),
        3,
        "one read and two explicit sends, no implicit publication"
    );
}

#[test]
fn schemas_are_explicit_text_only_and_never_accept_identity() {
    let send = schema::<ChatSendMessageRequest>();
    let read = schema::<ReadParams>();
    for schema in [&send, &read] {
        assert_eq!(schema["additionalProperties"], false);
        for forbidden in [
            "sender",
            "subject_id",
            "account_id",
            "session_id",
            "worker_id",
            "attachments",
        ] {
            assert!(
                schema["properties"].get(forbidden).is_none(),
                "{forbidden} must not be model input"
            );
        }
    }
    for required in [
        "conversation_id",
        "operation_id",
        "text",
        "expected_last_user_message_id",
    ] {
        assert!(
            send["required"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(required))
        );
    }
    assert!(read["properties"].get("after_sequence").is_some());
}

#[tokio::test]
async fn send_preserves_observed_input_reply_work_and_stable_retry_identity() {
    let client = Arc::new(ChatBackendFixture::default());
    let tool = tool(client.clone(), Operation::Send);
    let input = send_input();
    for _ in 0..2 {
        let output = invoke(&tool, input.clone()).await.unwrap();
        let committed: ChatMessage =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(committed.id, "message-committed");
        assert_eq!(committed.operation_id, input["operation_id"]);
        assert_eq!(
            committed.last_user_message_id.as_deref(),
            Some("user-message-3")
        );
        assert!(output.attachments.is_empty());
    }
    let requests = client.chat_requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        serde_json::to_value(&requests[0]).unwrap(),
        serde_json::to_value(&requests[1]).unwrap()
    );
    let ChatWorkerOperation::Send { request } = &requests[0].operation else {
        panic!("Send required")
    };
    assert_eq!(serde_json::to_value(request).unwrap(), input);
    // The fixture confirms forwarding, not storage exactly-once or real model quality.
}

#[tokio::test]
async fn read_preserves_cursor_and_returns_last_user_message_with_small_default_page() {
    let client = Arc::new(ChatBackendFixture::default());
    let tool = tool(client.clone(), Operation::Read);
    let output = invoke(
        &tool,
        serde_json::json!({"conversation_id":"conversation-1","after_sequence":6}),
    )
    .await
    .unwrap();
    let page: ChatMessagePage = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        page.conversation.last_user_message_id.as_deref(),
        Some("user-message-3")
    );
    let requests = client.chat_requests();
    let ChatWorkerOperation::Read {
        conversation_id,
        query,
    } = &requests[0].operation
    else {
        panic!("Read required")
    };
    assert_eq!(conversation_id, "conversation-1");
    assert_eq!(query.after_sequence, Some(6));
    assert_eq!(query.limit, Some(5));
    assert_eq!(
        requests.len(),
        1,
        "read must not automatically send anything"
    );
}

#[tokio::test]
async fn read_and_send_use_distinct_live_bound_subject_permissions() {
    for (operation, input, allowed) in [
        (
            Operation::Read,
            serde_json::json!({"conversation_id":"conversation-1"}),
            "chat.send",
        ),
        (Operation::Send, send_input(), "chat.read"),
    ] {
        let client = Arc::new(ChatBackendFixture::default());
        *client.allowed.lock().unwrap() = vec![allowed];
        let tool = tool(client.clone(), operation);
        let error = invoke(&tool, input.clone()).await.err().unwrap();
        assert!(error.to_string().contains("permission denied"));
        assert!(
            client.chat_requests().is_empty(),
            "denied content must not be read or written"
        );
        *client.allowed.lock().unwrap() = vec!["chat.read", "chat.send"];
        invoke(&tool, input.clone()).await.unwrap();
        client.allowed.lock().unwrap().clear();
        assert!(
            invoke(&tool, input).await.is_err(),
            "saved connection must not cache permission"
        );
        assert_eq!(client.chat_requests().len(), 1);
    }
}

#[tokio::test]
async fn backend_rejection_conflict_and_unknown_acceptance_never_report_sent_or_auto_retry() {
    for status in [403, 409, 500] {
        let client = Arc::new(ChatBackendFixture::default());
        client.status.store(status, Ordering::SeqCst);
        let error = invoke(&tool(client.clone(), Operation::Send), send_input())
            .await
            .err()
            .unwrap();
        if status == 409 {
            assert!(matches!(error, ToolError::StructuredConflict { .. }));
        } else {
            assert!(matches!(error, ToolError::ExecutionFailed(_)));
        }
        assert_eq!(client.chat_requests().len(), 1);
    }
    let client = Arc::new(ChatBackendFixture::default());
    client.unavailable.store(true, Ordering::SeqCst);
    assert!(
        invoke(&tool(client.clone(), Operation::Send), send_input())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("acceptance unknown")
    );
    assert_eq!(client.chat_requests().len(), 1);
}

#[tokio::test]
async fn invalid_or_identity_claiming_requests_are_rejected_before_host_access() {
    let client = Arc::new(ChatBackendFixture::default());
    let send = tool(client.clone(), Operation::Send);
    for forbidden in ["sender", "subject_id", "account_id", "attachments"] {
        let mut input = send_input();
        input[forbidden] = serde_json::json!("forged");
        assert!(matches!(
            invoke(&send, input).await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    for (field, value) in [
        ("conversation_id", serde_json::json!(" ")),
        ("operation_id", serde_json::json!("")),
        ("text", serde_json::json!(" ")),
        (
            "text",
            serde_json::json!("x".repeat(server_api::CHAT_MAX_TEXT_BYTES + 1)),
        ),
        ("expected_last_user_message_id", serde_json::json!(-1)),
    ] {
        let mut input = send_input();
        input[field] = value;
        assert!(matches!(
            invoke(&send, input).await,
            Err(ToolError::InvalidArgument(_))
        ));
    }
    let read = tool(client.clone(), Operation::Read);
    for limit in [0, server_api::CHAT_MAX_PAGE_LIMIT + 1] {
        assert!(
            invoke(
                &read,
                serde_json::json!({"conversation_id":"conversation-1","limit":limit})
            )
            .await
            .is_err()
        );
    }
    assert!(client.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unexpected_response_variants_and_oversized_reads_fail_closed() {
    let client = Arc::new(ChatBackendFixture::default());
    client.wrong_variant.store(true, Ordering::SeqCst);
    assert!(
        invoke(&tool(client.clone(), Operation::Send), send_input())
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("unexpected chat")
    );
    client
        .page_text_bytes
        .store(MAX_TOOL_OUTPUT_BYTES + 1, Ordering::SeqCst);
    let error = invoke(
        &tool(client.clone(), Operation::Read),
        serde_json::json!({"conversation_id":"conversation-1"}),
    )
    .await
    .err()
    .unwrap();
    assert!(error.to_string().contains("smaller page"));
    assert_eq!(client.chat_requests().len(), 2);
}
