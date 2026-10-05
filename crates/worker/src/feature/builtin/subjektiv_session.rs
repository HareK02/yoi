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
    SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES, SubjektivSessionBackendOperation,
    SubjektivSessionBackendRequest, SubjektivSessionBackendResponse, SubjektivSessionBackendResult,
    SubjektivSessionDiagnosticCode, SubjektivSessionErrorResponse, SubjektivSessionListRequest,
    SubjektivSessionReadRequest, SubjektivSessionSearchRequest,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{WorkspaceClient, WorkspaceServerOperation};

const LIST_TOOL: &str = "SubjektivSessionList";
const SEARCH_TOOL: &str = "SubjektivSessionSearch";
const READ_TOOL: &str = "SubjektivSessionRead";

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
}

impl SubjektivSessionFeature {
    pub(crate) fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        client: Arc<dyn WorkspaceClient>,
    ) -> std::io::Result<Option<Self>> {
        if !config.execution_enabled() {
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
            state: SubjektivSessionState { client },
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
        Ok(())
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
    use std::sync::Mutex;

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

    fn list_response() -> SubjektivSessionBackendResponse {
        SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::List(server_api::SubjektivSessionListResponse {
                items: Vec::new(),
                next_cursor: None,
                has_more: false,
            }),
        }
    }

    fn state(client: Arc<dyn WorkspaceClient>) -> SubjektivSessionState {
        SubjektivSessionState { client }
    }

    fn feature(client: Arc<dyn WorkspaceClient>) -> SubjektivSessionFeature {
        SubjektivSessionFeature {
            state: state(client),
        }
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
        assert!(descriptor.hooks.is_empty());

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
                state: state(client.clone()),
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
            state: state(client.clone()),
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
            state: state(Arc::new(SessionToolClient::new(typed_error.clone()))),
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

        let escaped_content = "\0".repeat(8 * 1024);
        let paged_read = SubjektivSessionBackendResponse::Ok {
            result: SubjektivSessionBackendResult::Read(server_api::SubjektivSessionReadResponse {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),
                entry_ref: "Eentry-1".into(),
                kind: server_api::SubjektivSessionEntryKind::User,
                origin: protocol::SessionEntryProvenance::HumanInput,
                lineage: server_api::SubjektivSessionLineage {
                    kind: server_api::SubjektivSessionLineageKind::Root,
                    parent_segment_id: None,
                    at_turn_index: None,
                },
                mode: server_api::SubjektivSessionReadMode::Full,
                content: escaped_content.clone(),
                truncated: true,
                next_cursor: Some("next-page".into()),
                has_more: true,
            }),
        };
        let output = model_visible_response(paged_read).unwrap();
        let content = output.content.unwrap();
        assert!(content.len() <= SUBJEKTIV_SESSION_MAX_TOOL_CONTENT_BYTES);
        let visible: SubjektivSessionBackendResponse = serde_json::from_str(&content).unwrap();
        assert!(matches!(
            visible,
            SubjektivSessionBackendResponse::Ok {
                result: SubjektivSessionBackendResult::Read(
                    server_api::SubjektivSessionReadResponse {
                        content,
                        next_cursor: Some(cursor),
                        has_more: true,
                        ..
                    }
                )
            } if content == escaped_content && cursor == "next-page"
        ));
    }
}
