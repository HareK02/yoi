//! Workspace-authority Workdir lifecycle tools for embedded Orchestrators.
//!
//! The model-visible contract contains only stable Workspace, Runtime,
//! repository, selector, and Workdir identities. Repository paths, Runtime
//! endpoints, credentials, materializer handles, and operation sessions stay
//! behind [`WorkspaceClient`].

use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{
    Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod, WorkspaceResponse};

const FEATURE_ID: &str = "manage-workdir";
const FEATURE_NAME: &str = "Manage Workdir";
const FEATURE_DESCRIPTION: &str =
    "Workspace-authority tools for listing, materializing, and deleting persistent Workdirs.";

const LIST_TOOL: &str = "WorkdirList";
const CREATE_TOOL: &str = "WorkdirCreate";
const DELETE_TOOL: &str = "WorkdirDelete";

const LIST_DESCRIPTION: &str = "List persistent Workdirs in the current Workspace through Backend Workspace API authority. The result contains safe summaries and diagnostics, never host paths or Runtime connection details.";
const CREATE_DESCRIPTION: &str = "Materialize a persistent Workdir on a selected Runtime from a Workspace repository and optional selector. Repository resolution and materialization remain Backend/Runtime authority.";
const DELETE_DESCRIPTION: &str = "Delete one persistent Workdir by id through Backend Workspace API authority. Occupied, blocked, or dirty Workdirs requiring confirmation are rejected.";

#[derive(Clone, Debug)]
pub struct ManageWorkdirFeature {
    client: Arc<dyn WorkspaceClient>,
}

impl ManageWorkdirFeature {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }
}

pub fn manage_workdir_feature(client: Arc<dyn WorkspaceClient>) -> ManageWorkdirFeature {
    ManageWorkdirFeature::new(client)
}

impl FeatureModule for ManageWorkdirFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(FEATURE_ID, FEATURE_NAME)
            .with_description(FEATURE_DESCRIPTION)
            .with_tool(ToolDeclaration::new(LIST_TOOL, LIST_DESCRIPTION))
            .with_tool(ToolDeclaration::new(CREATE_TOOL, CREATE_DESCRIPTION))
            .with_tool(ToolDeclaration::new(DELETE_TOOL, DELETE_DESCRIPTION))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        let backend = WorkspaceHttpWorkdirBackend::new(self.client.clone());
        for (name, definition) in [
            (
                LIST_TOOL,
                workdir_tool(
                    LIST_TOOL,
                    LIST_DESCRIPTION,
                    list_schema(),
                    backend.clone(),
                    WorkdirOperation::List,
                ),
            ),
            (
                CREATE_TOOL,
                workdir_tool(
                    CREATE_TOOL,
                    CREATE_DESCRIPTION,
                    create_schema(),
                    backend.clone(),
                    WorkdirOperation::Create,
                ),
            ),
            (
                DELETE_TOOL,
                workdir_tool(
                    DELETE_TOOL,
                    DELETE_DESCRIPTION,
                    delete_schema(),
                    backend.clone(),
                    WorkdirOperation::Delete,
                ),
            ),
        ] {
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct WorkspaceHttpWorkdirBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpWorkdirBackend {
    fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    fn workspace_id(&self) -> Result<&str, ToolError> {
        let Some(workspace_id) = self.client.workspace_id() else {
            return Err(ToolError::ExecutionFailed(
                "manage Workdir tools require a Workspace identity".to_string(),
            ));
        };
        if workspace_id.is_empty() || workspace_id.chars().any(char::is_control) {
            return Err(ToolError::ExecutionFailed(
                "manage Workdir tools require a valid Workspace identity".to_string(),
            ));
        }
        Ok(workspace_id)
    }

    fn list(&self) -> Result<ToolOutput, ToolError> {
        let workspace_id = encode_path_segment(self.workspace_id()?);
        let response = self.execute_json::<WorkdirListResponse>(WorkspaceRequest::get(format!(
            "/api/w/{workspace_id}/working-directories"
        )))?;
        let count = response.items.len();
        workdir_output(format!("Listed {count} Workdir(s)"), &response)
    }

    fn create(&self, input: WorkdirCreateInput) -> Result<ToolOutput, ToolError> {
        let runtime_id = validate_identity(&input.runtime_id, CREATE_TOOL, "runtime_id")?;
        let repository_id = validate_identity(&input.repository_id, CREATE_TOOL, "repository_id")?;
        let selector = validate_optional_selector(input.selector)?;
        let workspace_id = encode_path_segment(self.workspace_id()?);
        let runtime_path = encode_path_segment(runtime_id);
        let request = WorkdirCreateRequest {
            runtime_id: runtime_id.to_string(),
            repository_id: repository_id.to_string(),
            selector,
        };
        let response = self.execute_json::<WorkdirDetailResponse>(WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("/api/w/{workspace_id}/runtimes/{runtime_path}/working-directories"),
            serde_json::to_string(&request).map_err(decode_error)?,
        ))?;
        workdir_output(
            format!("Created Workdir {}", response.item.working_directory_id),
            &response,
        )
    }

    fn delete(&self, input: WorkdirDeleteInput) -> Result<ToolOutput, ToolError> {
        let workdir_id = validate_identity(
            &input.working_directory_id,
            DELETE_TOOL,
            "working_directory_id",
        )?;
        let workspace_id = encode_path_segment(self.workspace_id()?);
        let workdir_path = encode_path_segment(workdir_id);
        let response = self.execute_json::<WorkdirDetailResponse>(WorkspaceRequest {
            method: WorkspaceRequestMethod::Delete,
            path: format!("/api/w/{workspace_id}/working-directories/{workdir_path}"),
            body: None,
        })?;
        workdir_output(format!("Deleted Workdir {workdir_id}"), &response)
    }

    fn execute_json<T: for<'de> Deserialize<'de>>(
        &self,
        request: WorkspaceRequest,
    ) -> Result<T, ToolError> {
        let response = self.client.execute(request).map_err(|error| {
            ToolError::ExecutionFailed(format!("Workspace Workdir request failed: {error}"))
        })?;
        decode_response(response)
    }
}

#[derive(Clone, Copy)]
enum WorkdirOperation {
    List,
    Create,
    Delete,
}

fn workdir_tool(
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    backend: WorkspaceHttpWorkdirBackend,
    operation: WorkdirOperation,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema.clone()),
            Arc::new(WorkspaceHttpWorkdirTool {
                backend: backend.clone(),
                operation,
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct WorkspaceHttpWorkdirTool {
    backend: WorkspaceHttpWorkdirBackend,
    operation: WorkdirOperation,
}

#[async_trait]
impl Tool for WorkspaceHttpWorkdirTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        match self.operation {
            WorkdirOperation::List => {
                let _input = parse_input::<WorkdirListInput>(input_json)?;
                self.backend.list()
            }
            WorkdirOperation::Create => self
                .backend
                .create(parse_input::<WorkdirCreateInput>(input_json)?),
            WorkdirOperation::Delete => self
                .backend
                .delete(parse_input::<WorkdirDeleteInput>(input_json)?),
        }
    }
}

fn parse_input<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

fn decode_response<T: for<'de> Deserialize<'de>>(
    response: WorkspaceResponse,
) -> Result<T, ToolError> {
    if !response.is_success() {
        return Err(ToolError::ExecutionFailed(format!(
            "Workspace Workdir API returned HTTP {}: {}",
            response.status,
            bounded_error_body(&response.body)
        )));
    }
    serde_json::from_str(&response.body).map_err(decode_error)
}

fn decode_error(error: serde_json::Error) -> ToolError {
    ToolError::ExecutionFailed(format!("decode Workspace Workdir response: {error}"))
}

fn workdir_output<T: Serialize>(summary: String, value: &T) -> Result<ToolOutput, ToolError> {
    Ok(ToolOutput {
        summary,
        content: Some(serde_json::to_string_pretty(value).map_err(decode_error)?),
    })
}

fn validate_identity<'a>(
    value: &'a str,
    tool_name: &str,
    field: &str,
) -> Result<&'a str, ToolError> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ToolError::InvalidArgument(format!(
            "{tool_name} requires non-empty {field} without control characters"
        )));
    }
    Ok(value)
}

fn validate_optional_selector(selector: Option<String>) -> Result<Option<String>, ToolError> {
    let Some(selector) = selector else {
        return Ok(None);
    };
    let selector = selector.trim();
    if selector.is_empty() || selector.chars().any(char::is_control) {
        return Err(ToolError::InvalidArgument(
            "WorkdirCreate selector must be non-empty and contain no control characters"
                .to_string(),
        ));
    }
    Ok(Some(selector.to_string()))
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn bounded_error_body(body: &str) -> String {
    const MAX_CHARS: usize = 4096;
    let mut chars = body.chars();
    let bounded: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}…")
    } else {
        bounded
    }
}

fn list_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {}
    })
}

fn create_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["runtime_id", "repository_id"],
        "properties": {
            "runtime_id": {"type": "string", "minLength": 1},
            "repository_id": {"type": "string", "minLength": 1},
            "selector": {"type": ["string", "null"], "minLength": 1}
        }
    })
}

fn delete_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["working_directory_id"],
        "properties": {
            "working_directory_id": {"type": "string", "minLength": 1}
        }
    })
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirListInput {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirCreateInput {
    runtime_id: String,
    repository_id: String,
    #[serde(default)]
    selector: Option<String>,
}

#[derive(Debug, Serialize)]
struct WorkdirCreateRequest {
    runtime_id: String,
    repository_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirDeleteInput {
    working_directory_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirListResponse {
    workspace_id: String,
    items: Vec<WorkdirSummary>,
    diagnostics: Vec<WorkdirDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirDetailResponse {
    workspace_id: String,
    item: WorkdirSummary,
    diagnostics: Vec<WorkdirDiagnostic>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirSummary {
    working_directory_id: String,
    repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_ref: Option<String>,
    materializer_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_target: Option<WorkdirCleanupTarget>,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    primary_worker_id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    occupied_by: Option<WorkdirOccupancy>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirCleanupTarget {
    kind: String,
    working_directory_id: String,
    repository_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirOccupancy {
    runtime_id: String,
    runtime_worker_id: u64,
    worker_id: String,
    display_name: String,
    linked_at: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirDiagnostic {
    code: String,
    severity: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::feature::{FeatureModule, FeatureRegistryBuilder};
    use crate::hook::HookRegistryBuilder;
    use crate::worker::{WorkspaceClientError, WorkspaceResponse};

    #[derive(Debug)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
        responses: Mutex<Vec<WorkspaceResponse>>,
    }

    impl RecordingWorkspaceClient {
        fn new(responses: Vec<WorkspaceResponse>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into_iter().rev().collect()),
            }
        }

        fn requests(&self) -> Vec<WorkspaceRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace/test")
        }

        fn kind(&self) -> &str {
            "recording"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request);
            self.responses
                .lock()
                .unwrap()
                .pop()
                .ok_or_else(|| WorkspaceClientError::Request("missing response".to_string()))
        }
    }

    fn response(body: serde_json::Value) -> WorkspaceResponse {
        WorkspaceResponse {
            status: 200,
            body: serde_json::to_string(&body).unwrap(),
        }
    }

    fn workdir_json(id: &str) -> serde_json::Value {
        json!({
            "working_directory_id": id,
            "repository_id": "main",
            "creation_selector": "refs/heads/main",
            "creation_ref": "0123456789abcdef",
            "materializer_kind": "local_git_worktree",
            "cleanup_target": {
                "kind": "git_worktree",
                "working_directory_id": id,
                "repository_id": "main"
            },
            "status": "active",
            "cleanliness": "clean"
        })
    }

    #[test]
    fn descriptor_declares_only_workdir_lifecycle_tools() {
        let feature = manage_workdir_feature(Arc::new(RecordingWorkspaceClient::new(Vec::new())));
        let descriptor = feature.descriptor();
        let names = descriptor
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();

        assert_eq!(descriptor.id.as_str(), "builtin:manage-workdir");
        assert_eq!(names, [LIST_TOOL, CREATE_TOOL, DELETE_TOOL]);
    }

    #[test]
    fn feature_installs_all_declared_tools_through_registry() {
        let feature = manage_workdir_feature(Arc::new(RecordingWorkspaceClient::new(Vec::new())));
        let mut pending_tools = Vec::new();
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(feature)
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        let names = pending_tools
            .iter()
            .map(|definition| definition().0.name)
            .collect::<Vec<_>>();

        assert!(report.reports[0].installed);
        assert_eq!(
            report.installed_tool_names(),
            [LIST_TOOL, CREATE_TOOL, DELETE_TOOL]
        );
        assert_eq!(names, [LIST_TOOL, CREATE_TOOL, DELETE_TOOL]);
    }

    #[test]
    fn missing_workspace_identity_fails_closed() {
        #[derive(Debug)]
        struct MissingWorkspaceClient;

        impl WorkspaceClient for MissingWorkspaceClient {
            fn workspace_id(&self) -> Option<&str> {
                None
            }

            fn kind(&self) -> &str {
                "missing"
            }

            fn is_available(&self) -> bool {
                false
            }

            fn execute(
                &self,
                _request: WorkspaceRequest,
            ) -> Result<WorkspaceResponse, WorkspaceClientError> {
                panic!("missing Workspace client must not execute a request")
            }
        }

        let backend = WorkspaceHttpWorkdirBackend::new(Arc::new(MissingWorkspaceClient));
        let error = backend.list().unwrap_err();
        assert!(matches!(error, ToolError::ExecutionFailed(_)));
        assert!(error.to_string().contains("Workspace identity"));
    }

    #[test]
    fn schemas_expose_identities_without_paths_or_session_handles() {
        let create = create_schema();
        assert_eq!(create["required"], json!(["runtime_id", "repository_id"]));
        assert!(create["properties"].get("path").is_none());
        assert!(create["properties"].get("session_id").is_none());
        assert_eq!(delete_schema()["required"], json!(["working_directory_id"]));
    }

    #[test]
    fn list_create_and_delete_use_scoped_workspace_authority_paths() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(json!({
                "workspace_id": "workspace/test",
                "items": [workdir_json("wd-list")],
                "diagnostics": []
            })),
            response(json!({
                "workspace_id": "workspace/test",
                "item": workdir_json("wd-created"),
                "diagnostics": []
            })),
            response(json!({
                "workspace_id": "workspace/test",
                "item": {
                    "working_directory_id": "wd-created",
                    "repository_id": "main",
                    "materializer_kind": "local_git_worktree",
                    "status": "not_found"
                },
                "diagnostics": []
            })),
        ]));
        let backend = WorkspaceHttpWorkdirBackend::new(client.clone());

        backend.list().unwrap();
        backend
            .create(WorkdirCreateInput {
                runtime_id: "runtime/one".to_string(),
                repository_id: "main".to_string(),
                selector: Some("refs/heads/topic".to_string()),
            })
            .unwrap();
        backend
            .delete(WorkdirDeleteInput {
                working_directory_id: "wd-created".to_string(),
            })
            .unwrap();

        let requests = client.requests();
        assert_eq!(
            requests[0].path,
            "/api/w/workspace%2Ftest/working-directories"
        );
        assert_eq!(requests[0].method, WorkspaceRequestMethod::Get);
        assert_eq!(
            requests[1].path,
            "/api/w/workspace%2Ftest/runtimes/runtime%2Fone/working-directories"
        );
        assert_eq!(requests[1].method, WorkspaceRequestMethod::Post);
        let body: serde_json::Value =
            serde_json::from_str(requests[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["repository_id"], "main");
        assert_eq!(body["selector"], "refs/heads/topic");
        assert_eq!(
            requests[2].path,
            "/api/w/workspace%2Ftest/working-directories/wd-created"
        );
        assert_eq!(requests[2].method, WorkspaceRequestMethod::Delete);
    }

    #[test]
    fn invalid_or_extra_inputs_are_rejected_before_workspace_request() {
        let client = Arc::new(RecordingWorkspaceClient::new(Vec::new()));
        let backend = WorkspaceHttpWorkdirBackend::new(client.clone());
        let error = backend
            .create(WorkdirCreateInput {
                runtime_id: " ".to_string(),
                repository_id: "main".to_string(),
                selector: None,
            })
            .unwrap_err();
        assert!(matches!(error, ToolError::InvalidArgument(_)));
        assert!(client.requests().is_empty());
        assert!(parse_input::<WorkdirListInput>(r#"{"path":"/tmp"}"#).is_err());
    }
}
