//! Host-scoped tools for exploring subject-attributed committed Sessions.
//!
//! Subject, Runtime, Worker, archive, and filesystem authority stays bound by
//! the Workspace host. Model input contains only the selectors and filters from
//! the public `server_api` request types.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use schemars::JsonSchema;
use server_api::{
    SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES, SubjektivRecordSessionRequest,
    SubjektivRecordSessionResponse, SubjektivSessionBackendOperation,
    SubjektivSessionBackendRequest, SubjektivSessionBackendResponse, SubjektivSessionBackendResult,
    SubjektivSessionDiagnosticCode, SubjektivSessionErrorResponse, SubjektivSessionListRequest,
    SubjektivSessionReadRequest, SubjektivSessionSearchRequest,
};

use crate::feature::session::CommittedSessionCaptureHandle;
use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    Hook, HookError, HookErrorCategory, HookExecutionPolicy, HookFailurePolicy,
    HookPreRequestAction, PreLlmRequest, PreRequestContext, RunCommitted, RunCommittedContext,
};
use crate::worker::{WorkspaceClient, WorkspaceServerOperation};

const LIST_TOOL: &str = "SubjektivSessionList";
const SEARCH_TOOL: &str = "SubjektivSessionSearch";
const READ_TOOL: &str = "SubjektivSessionRead";
const COMMIT_HOOK: &str = "record-subjektiv-session-after-commit";
const RETRY_HOOK: &str = "retry-subjektiv-session-attribution";

const LIST_DESCRIPTION: &str = "List committed retained or archived Sessions attributed by the Host to the connected subject. Filter by exact session_id or storage and continue with the returned opaque cursor; subject, Runtime, Worker, archive, and filesystem authority are never model inputs.";
const SEARCH_DESCRIPTION: &str = "Search committed public entries across Host-authorized Sessions attributed to the connected subject, or list one Session's public entries when session_id is supplied and query is omitted. Search is case-insensitive literal matching; system prompts, hidden reasoning, traces, and attachment bodies are never exposed.";
const READ_DESCRIPTION: &str = "Read one exact committed public Session entry selected by session_id, segment_id, and entry_ref. Compact mode is the default; full mode remains bounded, and partial content must be continued with the returned opaque cursor.";

#[derive(Clone)]
pub(crate) struct SubjektivSessionFeature {
    state: SubjektivSessionState,
}

#[derive(Clone)]
struct SubjektivSessionState {
    client: Arc<dyn WorkspaceClient>,
    capture: CommittedSessionCaptureHandle,
}

impl SubjektivSessionFeature {
    pub(crate) fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        client: Arc<dyn WorkspaceClient>,
    ) -> std::io::Result<Option<Self>> {
        if !config.profile.enabled {
            return Ok(None);
        }
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if !client.is_available() || client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv Session tools require Backend Workspace API authority",
            ));
        }
        Ok(Some(Self {
            state: SubjektivSessionState { client, capture },
        }))
    }
}

impl FeatureModule for SubjektivSessionFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv-session-tools", "subjektiv Session Tools")
            .with_description(
                "Read-only discovery, search, and reading of Host-authorized subject Sessions.",
            )
            .with_tool(ToolDeclaration::new(LIST_TOOL, LIST_DESCRIPTION))
            .with_tool(ToolDeclaration::new(SEARCH_TOOL, SEARCH_DESCRIPTION))
            .with_tool(ToolDeclaration::new(READ_TOOL, READ_DESCRIPTION))
            .with_hook(HookDeclaration::new(
                COMMIT_HOOK,
                FeatureHookPoint::RunCommitted,
            ))
            .with_hook(HookDeclaration::new(
                RETRY_HOOK,
                FeatureHookPoint::PreLlmRequest,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for (name, description, operation) in [
            (LIST_TOOL, LIST_DESCRIPTION, SessionOperation::List),
            (SEARCH_TOOL, SEARCH_DESCRIPTION, SessionOperation::Search),
            (READ_TOOL, READ_DESCRIPTION, SessionOperation::Read),
        ] {
            context.tools().register(ToolContribution::new(
                name,
                tool_definition(name, description, self.state.clone(), operation),
            ))?;
        }
        context.hooks().add_run_committed(
            COMMIT_HOOK,
            HookExecutionPolicy::new(HookFailurePolicy::AttentionRequired, 30_000),
            RecordCommittedSessionHook {
                state: self.state.clone(),
            },
        )?;
        context.hooks().add_pre_request(
            RETRY_HOOK,
            RetryCommittedSessionHook {
                state: self.state.clone(),
            },
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionOperation {
    List,
    Search,
    Read,
}

fn tool_definition(
    name: &'static str,
    description: &'static str,
    state: SubjektivSessionState,
    operation: SessionOperation,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = match operation {
            SessionOperation::List => schema_for::<SubjektivSessionListRequest>(),
            SessionOperation::Search => schema_for::<SubjektivSessionSearchRequest>(),
            SessionOperation::Read => schema_for::<SubjektivSessionReadRequest>(),
        };
        let tool: Arc<dyn Tool> = Arc::new(SubjektivSessionTool {
            state: state.clone(),
            operation,
        });
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema),
            tool,
        )
    })
}

struct SubjektivSessionTool {
    state: SubjektivSessionState,
    operation: SessionOperation,
}

#[async_trait]
impl Tool for SubjektivSessionTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let operation = match self.operation {
            SessionOperation::List => {
                SubjektivSessionBackendOperation::List(parse(input_json, LIST_TOOL)?)
            }
            SessionOperation::Search => {
                SubjektivSessionBackendOperation::Search(parse(input_json, SEARCH_TOOL)?)
            }
            SessionOperation::Read => {
                SubjektivSessionBackendOperation::Read(parse(input_json, READ_TOOL)?)
            }
        };
        let response = self.state.execute(operation)?;
        model_visible_response(response)
    }
}

impl SubjektivSessionState {
    fn execute(
        &self,
        operation: SubjektivSessionBackendOperation,
    ) -> Result<SubjektivSessionBackendResponse, ToolError> {
        let response = self
            .client
            .execute_server_operation(WorkspaceServerOperation::SubjektivSession(
                SubjektivSessionBackendRequest { operation },
            ))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;

        if let Ok(response) =
            serde_json::from_str::<SubjektivSessionBackendResponse>(&response.body)
        {
            return Ok(response);
        }
        let detail = serde_json::from_str::<server_api::RepositoryApiError>(&response.body)
            .map(|error| error.message)
            .unwrap_or(response.body);
        Err(ToolError::ExecutionFailed(format!(
            "subjektiv Session backend returned HTTP {}: {detail}",
            response.status
        )))
    }

    fn record_current_committed_session(&self) -> Result<(), HookError> {
        let capture = self
            .capture
            .capture()
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
        if !capture.has_committed_run {
            return Err(HookError::new(
                HookErrorCategory::Dependency,
                "committed Session capture is not available yet",
            ));
        }
        self.record_session(&capture.session_id)
    }

    fn retry_current_committed_session(&self) -> Result<(), HookError> {
        let capture = self
            .capture
            .capture()
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
        if !capture.has_committed_run {
            return Ok(());
        }
        self.record_session(&capture.session_id)
    }

    fn record_session(&self, session_id: &str) -> Result<(), HookError> {
        let response = self
            .client
            .execute_server_operation(WorkspaceServerOperation::SubjektivRecordSession(
                SubjektivRecordSessionRequest {
                    session_id: session_id.to_string(),
                },
            ))
            .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
        if !response.is_success() {
            return Err(HookError::new(
                HookErrorCategory::Dependency,
                format!(
                    "record subjektiv Session attribution returned HTTP {}",
                    response.status
                ),
            ));
        }
        let output: SubjektivRecordSessionResponse =
            serde_json::from_str(&response.body).map_err(|error| {
                HookError::new(
                    HookErrorCategory::Dependency,
                    format!("decode subjektiv Session attribution response: {error}"),
                )
            })?;
        if output.session_id != session_id {
            return Err(HookError::new(
                HookErrorCategory::Dependency,
                "subjektiv Session attribution response changed session identity",
            ));
        }
        Ok(())
    }
}

struct RecordCommittedSessionHook {
    state: SubjektivSessionState,
}

#[async_trait]
impl Hook<RunCommitted> for RecordCommittedSessionHook {
    async fn call(&self, _input: &RunCommittedContext) -> Result<(), HookError> {
        self.state.record_current_committed_session()
    }
}

struct RetryCommittedSessionHook {
    state: SubjektivSessionState,
}

#[async_trait]
impl Hook<PreLlmRequest> for RetryCommittedSessionHook {
    async fn call(&self, _input: &PreRequestContext) -> Result<HookPreRequestAction, HookError> {
        self.state.retry_current_committed_session()?;
        Ok(HookPreRequestAction::Continue)
    }
}

fn parse<T: serde::de::DeserializeOwned>(input: &str, tool: &str) -> Result<T, ToolError> {
    serde_json::from_str(input)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool} input: {error}")))
}

fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| serde_json::json!({}))
}

fn model_visible_response(
    response: SubjektivSessionBackendResponse,
) -> Result<ToolOutput, ToolError> {
    let summary = response_summary(&response);
    let content = serde_json::to_string(&response)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    if content.len() <= SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES {
        return Ok(ToolOutput {
            summary,
            content: Some(content),
            attachments: Vec::new(),
        });
    }

    let bounded = SubjektivSessionBackendResponse::Error {
        error: SubjektivSessionErrorResponse {
            code: SubjektivSessionDiagnosticCode::ResourceLimit,
            message: "subjektiv Session response exceeded the 56 KiB model-visible JSON budget"
                .to_string(),
        },
    };
    let content = serde_json::to_string(&bounded)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    debug_assert!(content.len() <= SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES);
    Ok(ToolOutput {
        summary: "Subjektiv Session operation reached its output limit.".to_string(),
        content: Some(content),
        attachments: Vec::new(),
    })
}

fn response_summary(response: &SubjektivSessionBackendResponse) -> String {
    match response {
        SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::List(value),
        } => format!("Listed {} subject Session(s).", value.items.len()),
        SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::Search(value),
        } => format!("Found {} subject Session entries.", value.items.len()),
        SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::Read(value),
        } => format!("Read subject Session entry {}.", value.entry_ref),
        SubjektivSessionBackendResponse::Error { error } => {
            format!("Subjektiv Session operation returned {:?}.", error.code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::feature::session::{CommittedRunExit, CommittedSessionCapture};
    use crate::hook::{HookHistoryRange, HookInvocationContext, PreRequestInfo, RunCommittedExit};
    use crate::worker::{
        WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse,
    };

    #[derive(Debug)]
    struct SessionToolClient {
        response: Mutex<SubjektivSessionBackendResponse>,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl SessionToolClient {
        fn new(response: SubjektivSessionBackendResponse) -> Self {
            Self {
                response: Mutex::new(response),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl WorkspaceClient for SessionToolClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "subjektiv-session-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&*self.response.lock().unwrap()).unwrap(),
            })
        }
    }

    #[derive(Debug, Default)]
    struct AttributionClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
        recorded: Mutex<HashSet<String>>,
        insertions: AtomicUsize,
        failures: AtomicUsize,
    }

    impl WorkspaceClient for AttributionClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "subjektiv-attribution-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Ok(WorkspaceResponse {
                    status: 503,
                    body: "temporarily unavailable".into(),
                });
            }
            let input: SubjektivRecordSessionRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            if self
                .recorded
                .lock()
                .unwrap()
                .insert(input.session_id.clone())
            {
                self.insertions.fetch_add(1, Ordering::SeqCst);
            }
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&SubjektivRecordSessionResponse {
                    subject_id: "subject-1".into(),
                    session_id: input.session_id,
                })
                .unwrap(),
            })
        }
    }

    fn capture(has_committed_run: bool) -> CommittedSessionCaptureHandle {
        CommittedSessionCaptureHandle::new(move || {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),
                session_revision: usize::from(has_committed_run) as u64,
                entry_count: usize::from(has_committed_run),
                has_committed_run,
                run_exit: CommittedRunExit::Finished,
                history: Vec::new(),
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        })
    }

    fn list_response() -> SubjektivSessionBackendResponse {
        SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::List(server_api::SubjektivSessionListResponse {
                items: Vec::new(),
                next_cursor: None,
                has_more: false,
            }),
        }
    }

    fn state(client: Arc<dyn WorkspaceClient>, has_committed_run: bool) -> SubjektivSessionState {
        SubjektivSessionState {
            client,
            capture: capture(has_committed_run),
        }
    }

    fn feature(client: Arc<dyn WorkspaceClient>) -> SubjektivSessionFeature {
        SubjektivSessionFeature {
            state: state(client, true),
        }
    }

    fn run_committed_context() -> RunCommittedContext {
        RunCommittedContext {
            invocation: HookInvocationContext {
                workspace_id: Some("workspace-test".into()),
                worker_id: "worker-1".into(),
                session_id: "session-1".into(),
                session_revision: 1,
                run_id: Some("run-1".into()),
                turn_index: Some(0),
                call_id: None,
            },
            exit: RunCommittedExit::Finished,
            committed_history: HookHistoryRange::default(),
        }
    }

    fn pre_request_context() -> PreRequestContext {
        PreRequestContext::new(
            PreRequestInfo {
                item_count: 0,
                estimated_tokens: None,
                turn_index: 0,
                tool_calls_this_turn: 0,
            },
            None,
        )
    }

    #[test]
    fn descriptor_and_definitions_register_exact_session_tools_and_typed_schemas() {
        let feature = feature(Arc::new(SessionToolClient::new(list_response())));
        let descriptor = feature.descriptor();
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| (tool.name.as_str(), tool.description.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (LIST_TOOL, LIST_DESCRIPTION),
                (SEARCH_TOOL, SEARCH_DESCRIPTION),
                (READ_TOOL, READ_DESCRIPTION),
            ]
        );
        assert_eq!(
            descriptor
                .hooks
                .iter()
                .map(|hook| (hook.name.as_str(), hook.point.clone()))
                .collect::<Vec<_>>(),
            vec![
                (COMMIT_HOOK, FeatureHookPoint::RunCommitted),
                (RETRY_HOOK, FeatureHookPoint::PreLlmRequest),
            ]
        );

        for (name, description, operation, expected_schema) in [
            (
                LIST_TOOL,
                LIST_DESCRIPTION,
                SessionOperation::List,
                schema_for::<SubjektivSessionListRequest>(),
            ),
            (
                SEARCH_TOOL,
                SEARCH_DESCRIPTION,
                SessionOperation::Search,
                schema_for::<SubjektivSessionSearchRequest>(),
            ),
            (
                READ_TOOL,
                READ_DESCRIPTION,
                SessionOperation::Read,
                schema_for::<SubjektivSessionReadRequest>(),
            ),
        ] {
            let (meta, _) = tool_definition(name, description, feature.state.clone(), operation)();
            assert_eq!(meta.name, name);
            assert_eq!(meta.description, description);
            assert_eq!(meta.input_schema, expected_schema);
            let properties = meta.input_schema["properties"].as_object().unwrap();
            for forbidden in [
                "subject_id",
                "runtime_id",
                "worker_id",
                "archive_id",
                "archive_path",
                "path",
            ] {
                assert!(!properties.contains_key(forbidden));
            }
        }
    }

    #[test]
    fn disabled_subjektiv_feature_registers_nothing() {
        let config = manifest::ResolvedSubjektivFeatureConfig::default();
        let result = SubjektivSessionFeature::from_resolved_config(
            &config,
            capture(false),
            Arc::new(SessionToolClient::new(list_response())),
        )
        .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn tools_reject_model_supplied_host_scope_and_storage_paths() {
        let client = Arc::new(SessionToolClient::new(list_response()));
        for (operation, input) in [
            (
                SessionOperation::List,
                r#"{"subject_id":"subject-1","archive_path":"/tmp/archive"}"#,
            ),
            (
                SessionOperation::Search,
                r#"{"query":"needle","runtime_id":"runtime-1","worker_id":"worker-1"}"#,
            ),
            (
                SessionOperation::Read,
                r#"{"session_id":"session-1","segment_id":"segment-1","entry_ref":"E1","path":"session.jsonl"}"#,
            ),
        ] {
            let error = SubjektivSessionTool {
                state: state(client.clone(), true),
                operation,
            }
            .execute(input, ToolExecutionContext::new("call", "batch", 0))
            .await
            .unwrap_err();
            assert!(matches!(error, ToolError::InvalidArgument(_)));
        }
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn tool_routes_one_host_bound_operation_to_session_history() {
        let client = Arc::new(SessionToolClient::new(list_response()));
        let tool = SubjektivSessionTool {
            state: state(client.clone(), true),
            operation: SessionOperation::List,
        };
        let output = tool
            .execute(
                r#"{"storage":"retained","limit":20}"#,
                ToolExecutionContext::new("call-1", "batch-1", 0),
            )
            .await
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Post);
        assert_eq!(
            requests[0].path,
            "/api/w/workspace-test/subjektiv/session-history"
        );
        let body: SubjektivSessionBackendRequest =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert!(matches!(
            body.operation,
            SubjektivSessionBackendOperation::List(SubjektivSessionListRequest {
                session_id: None,
                storage: server_api::SubjektivSessionStorageFilter::Retained,
                limit: Some(20),
                cursor: None,
            })
        ));
        let visible: SubjektivSessionBackendResponse =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert!(matches!(
            visible,
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::List(_)
            }
        ));
    }

    #[tokio::test]
    async fn typed_errors_are_visible_and_oversized_json_becomes_bounded_resource_limit() {
        let typed_error = SubjektivSessionBackendResponse::Error {
            error: SubjektivSessionErrorResponse {
                code: SubjektivSessionDiagnosticCode::StaleCursor,
                message: "restart the search".into(),
            },
        };
        let error_tool = SubjektivSessionTool {
            state: state(Arc::new(SessionToolClient::new(typed_error.clone())), true),
            operation: SessionOperation::Search,
        };
        let output = error_tool
            .execute(
                r#"{"query":"needle"}"#,
                ToolExecutionContext::new("call-error", "batch", 0),
            )
            .await
            .unwrap();
        let visible: SubjektivSessionBackendResponse =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert!(matches!(
            visible,
            SubjektivSessionBackendResponse::Error {
                error: SubjektivSessionErrorResponse {
                    code: SubjektivSessionDiagnosticCode::StaleCursor,
                    ..
                }
            }
        ));

        let oversized = SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::List(server_api::SubjektivSessionListResponse {
                items: vec![server_api::SubjektivSessionListItem {
                    session_id: "session-1".into(),
                    attributed_at: "2026-01-01T00:00:00Z".into(),
                    storage: server_api::SubjektivSessionStorage::Retained,
                    availability: server_api::SubjektivSessionAvailability::Unavailable,
                    reason: Some("\\\u{0000}".repeat(SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES)),
                }],
                next_cursor: Some("cursor".into()),
                has_more: true,
            }),
        };
        let output = model_visible_response(oversized).unwrap();
        let content = output.content.unwrap();
        assert!(content.len() <= SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES);
        let visible: SubjektivSessionBackendResponse = serde_json::from_str(&content).unwrap();
        assert!(matches!(
            visible,
            SubjektivSessionBackendResponse::Error {
                error: SubjektivSessionErrorResponse {
                    code: SubjektivSessionDiagnosticCode::ResourceLimit,
                    ..
                }
            }
        ));
    }

    #[tokio::test]
    async fn candidate_free_commit_records_session_and_pre_request_waits_for_committed_capture() {
        let client = Arc::new(AttributionClient::default());
        let commit_hook = RecordCommittedSessionHook {
            state: state(client.clone(), true),
        };
        commit_hook.call(&run_committed_context()).await.unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/api/w/workspace-test/subjektiv/sessions");
        assert!(!requests[0].path.contains("staging"));
        drop(requests);

        let before_commit = Arc::new(AttributionClient::default());
        let premature_commit_hook = RecordCommittedSessionHook {
            state: state(before_commit.clone(), false),
        };
        assert!(
            premature_commit_hook
                .call(&run_committed_context())
                .await
                .is_err()
        );
        let retry_hook = RetryCommittedSessionHook {
            state: state(before_commit.clone(), false),
        };
        assert_eq!(
            retry_hook.call(&pre_request_context()).await.unwrap(),
            HookPreRequestAction::Continue
        );
        assert!(before_commit.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn transient_attribution_failure_is_a_hook_failure_then_retries_exactly() {
        let client = Arc::new(AttributionClient::default());
        client.failures.store(1, Ordering::SeqCst);
        let state = state(client.clone(), true);
        let commit_hook = RecordCommittedSessionHook {
            state: state.clone(),
        };
        let retry_hook = RetryCommittedSessionHook { state };

        let error = commit_hook
            .call(&run_committed_context())
            .await
            .unwrap_err();
        assert_eq!(error.category, HookErrorCategory::Dependency);
        assert_eq!(
            retry_hook.call(&pre_request_context()).await.unwrap(),
            HookPreRequestAction::Continue
        );

        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].body, requests[1].body);
        assert_eq!(client.insertions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repeated_commit_and_pre_request_use_exact_request_and_backend_dedupes() {
        let client = Arc::new(AttributionClient::default());
        let state = state(client.clone(), true);
        let commit_hook = RecordCommittedSessionHook {
            state: state.clone(),
        };
        let retry_hook = RetryCommittedSessionHook { state };

        commit_hook.call(&run_committed_context()).await.unwrap();
        commit_hook.call(&run_committed_context()).await.unwrap();
        retry_hook.call(&pre_request_context()).await.unwrap();

        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|request| request.body == requests[0].body)
        );
        assert_eq!(client.recorded.lock().unwrap().len(), 1);
        assert_eq!(client.insertions.load(Ordering::SeqCst), 1);
    }
}
