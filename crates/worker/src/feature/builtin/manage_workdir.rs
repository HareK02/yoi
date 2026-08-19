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
use workdir::http::{WorkdirSessionOperation, WorkdirSessionOperationResult};
use workdir::workspace::{
    WorkingDirectoryDetailResponse as WorkdirDetailResponse,
    WorkingDirectoryListResponse as WorkdirListResponse, WorkspaceWorkdirSessionFence,
    WorkspaceWorkdirSessionOperationRequest,
};
use workdir::{
    CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest, CommandStatus, EditRequest,
    EditResult, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest, ListResult,
    ReadRequest, ReadResult, StatRequest, StatResult, Workdir, WorkdirError, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionHandle, WriteRequest, WriteResult,
};

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{
    WorkspaceClient, WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
    WorkspaceResponse,
};

const FEATURE_ID: &str = "manage-workdir";
const FEATURE_NAME: &str = "Manage Workdir";
const FEATURE_DESCRIPTION: &str =
    "Workspace-authority tools for listing, materializing, and deleting persistent Workdirs.";

const LIST_TOOL: &str = "WorkdirList";
const CREATE_TOOL: &str = "WorkdirCreate";
const ATTACH_TOOL: &str = "WorkdirAttach";
const DETACH_TOOL: &str = "WorkdirDetach";
const DELETE_TOOL: &str = "WorkdirDelete";

const LIST_DESCRIPTION: &str = "List persistent Workdirs in the current Workspace through Backend Workspace API authority. The result contains safe summaries and diagnostics, never host paths or Runtime connection details.";
const CREATE_DESCRIPTION: &str = "Materialize a persistent Workdir on a selected Runtime from a Workspace repository and optional selector. This does not change this Worker's attachment; use WorkdirAttach explicitly after creation.";
const ATTACH_DESCRIPTION: &str = "Attach this Worker to one existing Workdir. The Backend enforces one active Workdir per Worker and one active Worker per Workdir, then opens an ephemeral operation session.";
const DETACH_DESCRIPTION: &str = "Detach this Worker from its active Workdir and release Workdir occupancy. Any ephemeral operation session is closed.";
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
            .with_tool(ToolDeclaration::new(ATTACH_TOOL, ATTACH_DESCRIPTION))
            .with_tool(ToolDeclaration::new(DETACH_TOOL, DETACH_DESCRIPTION))
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
                ATTACH_TOOL,
                workdir_tool(
                    ATTACH_TOOL,
                    ATTACH_DESCRIPTION,
                    attach_schema(),
                    backend.clone(),
                    WorkdirOperation::Attach,
                ),
            ),
            (
                DETACH_TOOL,
                workdir_tool(
                    DETACH_TOOL,
                    DETACH_DESCRIPTION,
                    detach_schema(),
                    backend.clone(),
                    WorkdirOperation::Detach,
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

/// Worker-local Workdir handle whose operation authority remains in the Workspace Backend.
///
/// The Backend resolves the caller from the identity-bound [`WorkspaceClient`] and routes each
/// operation to that Worker's active attachment. Runtime endpoints, credentials, and ephemeral
/// session ids remain outside model-visible tool contracts.
#[derive(Debug)]
pub struct WorkspaceAttachedWorkdirSession {
    client: Arc<dyn WorkspaceClient>,
    workdir: Workdir,
    expected_session_fence: Option<String>,
    delegation: Option<workdir::WorkdirDelegationRequest>,
}

impl WorkspaceAttachedWorkdirSession {
    pub fn handle(client: Arc<dyn WorkspaceClient>) -> WorkdirSessionHandle {
        Arc::new(Self {
            client,
            workdir: Workdir::new("workspace-attachment"),
            expected_session_fence: None,
            delegation: None,
        })
    }

    fn operate(
        &self,
        operation: WorkdirSessionOperation,
    ) -> Result<WorkdirSessionOperationResult, WorkdirError> {
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            WorkdirError::Unavailable("Workspace identity is unavailable".to_string())
        })?;
        let request = WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!(
                "/api/w/{}/workers/self/workdir-session/operations",
                encode_path_segment(workspace_id)
            ),
            serde_json::to_string(&WorkspaceWorkdirSessionOperationRequest {
                expected_session_fence: self.expected_session_fence.clone(),
                delegation: self.delegation.clone(),
                operation,
            })
            .map_err(|error| {
                WorkdirError::Transport(format!(
                    "failed to encode Workspace Workdir operation: {error}"
                ))
            })?,
        );
        let response = self
            .client
            .execute(request)
            .map_err(workspace_workdir_error)?;
        if !response.is_success() {
            return Err(WorkdirError::Transport(format!(
                "Workspace Workdir API returned HTTP {}: {}",
                response.status,
                bounded_error_body(&response.body)
            )));
        }
        serde_json::from_str(&response.body).map_err(|error| {
            WorkdirError::Transport(format!(
                "failed to decode Workspace Workdir operation result: {error}"
            ))
        })
    }

    fn mismatch(expected: &str) -> WorkdirError {
        WorkdirError::Transport(format!(
            "Workspace Backend returned a mismatched Workdir operation result; expected {expected}"
        ))
    }
}

fn workspace_workdir_error(error: WorkspaceClientError) -> WorkdirError {
    match error {
        WorkspaceClientError::Unavailable(message) => WorkdirError::Unavailable(message),
        other => WorkdirError::Transport(other.to_string()),
    }
}

#[async_trait]
impl WorkdirSession for WorkspaceAttachedWorkdirSession {
    fn workdir(&self) -> &Workdir {
        &self.workdir
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        WorkdirSessionCapabilities::ALL
    }

    async fn capture_delegation_source(
        &self,
        request: &workdir::WorkdirDelegationRequest,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        let expected_session_fence = if let Some(fence) = &self.expected_session_fence {
            fence.clone()
        } else {
            let workspace_id = self.client.workspace_id().ok_or_else(|| {
                WorkdirError::Unavailable("Workspace identity is unavailable".to_string())
            })?;
            let response = self
                .client
                .execute(WorkspaceRequest {
                    method: WorkspaceRequestMethod::Get,
                    path: format!(
                        "/api/w/{}/workers/self/workdir-session/fence",
                        encode_path_segment(workspace_id)
                    ),
                    body: None,
                })
                .map_err(|error| {
                    WorkdirError::Unavailable(format!(
                        "failed to capture Workdir attachment fence: {error}"
                    ))
                })?;
            let fence: WorkspaceWorkdirSessionFence = serde_json::from_str(&response.body)
                .map_err(|error| {
                    WorkdirError::Unavailable(format!(
                        "invalid Workdir attachment fence response: {error}"
                    ))
                })?;
            fence.value
        };
        Ok(Arc::new(Self {
            client: self.client.clone(),
            workdir: self.workdir.clone(),
            expected_session_fence: Some(expected_session_fence),
            delegation: Some(request.clone()),
        }))
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Stat(request))? {
            WorkdirSessionOperationResult::Stat(result) => Ok(result),
            _ => Err(Self::mismatch("stat")),
        }
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Read(request))? {
            WorkdirSessionOperationResult::Read(result) => Ok(result),
            _ => Err(Self::mismatch("read")),
        }
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Write(request))? {
            WorkdirSessionOperationResult::Write(result) => Ok(result),
            _ => Err(Self::mismatch("write")),
        }
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Edit(request))? {
            WorkdirSessionOperationResult::Edit(result) => Ok(result),
            _ => Err(Self::mismatch("edit")),
        }
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::List(request))? {
            WorkdirSessionOperationResult::List(result) => Ok(result),
            _ => Err(Self::mismatch("list")),
        }
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Glob(request))? {
            WorkdirSessionOperationResult::Glob(result) => Ok(result),
            _ => Err(Self::mismatch("glob")),
        }
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        match self.operate(WorkdirSessionOperation::Grep(request))? {
            WorkdirSessionOperationResult::Grep(result) => Ok(result),
            _ => Err(Self::mismatch("grep")),
        }
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        match self.operate(WorkdirSessionOperation::CommandStart(request))? {
            WorkdirSessionOperationResult::CommandStart(result) => Ok(result),
            _ => Err(Self::mismatch("command_start")),
        }
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        match self.operate(WorkdirSessionOperation::CommandStatus(handle))? {
            WorkdirSessionOperationResult::CommandStatus(result) => Ok(result),
            _ => Err(Self::mismatch("command_status")),
        }
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        match self.operate(WorkdirSessionOperation::CommandOutput(request))? {
            WorkdirSessionOperationResult::CommandOutput(result) => Ok(result),
            _ => Err(Self::mismatch("command_output")),
        }
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        match self.operate(WorkdirSessionOperation::CommandCancel(handle))? {
            WorkdirSessionOperationResult::CommandCancel => Ok(()),
            _ => Err(Self::mismatch("command_cancel")),
        }
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        Ok(())
    }
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

    fn attach(&self, input: WorkdirAttachInput) -> Result<ToolOutput, ToolError> {
        let workdir_id = validate_identity(&input.workdir_id, ATTACH_TOOL, "workdir_id")?;
        let response = self.attach_response(workdir_id)?;
        workdir_output(format!("Attached to Workdir {workdir_id}"), &response)
    }

    fn attach_response(&self, workdir_id: &str) -> Result<WorkdirAttachmentResponse, ToolError> {
        let workspace_id = encode_path_segment(self.workspace_id()?);
        self.execute_json::<WorkdirAttachmentResponse>(WorkspaceRequest::json(
            WorkspaceRequestMethod::Post,
            format!("/api/w/{workspace_id}/workers/self/workdir-attachment"),
            serde_json::to_string(&WorkdirAttachRequest {
                workdir_id: workdir_id.to_string(),
            })
            .map_err(decode_error)?,
        ))
    }

    fn detach(&self) -> Result<ToolOutput, ToolError> {
        let workspace_id = encode_path_segment(self.workspace_id()?);
        let response = self.execute_json::<WorkdirAttachmentResponse>(WorkspaceRequest {
            method: WorkspaceRequestMethod::Delete,
            path: format!("/api/w/{workspace_id}/workers/self/workdir-attachment"),
            body: None,
        })?;
        workdir_output(
            format!("Detached from Workdir {}", response.workdir_id),
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
    Attach,
    Detach,
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
            WorkdirOperation::Attach => self
                .backend
                .attach(parse_input::<WorkdirAttachInput>(input_json)?),
            WorkdirOperation::Detach => {
                let _input = parse_input::<WorkdirDetachInput>(input_json)?;
                self.backend.detach()
            }
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
        attachments: Vec::new(),
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

fn attach_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["workdir_id"],
        "properties": {
            "workdir_id": {"type": "string", "minLength": 1}
        }
    })
}

fn detach_schema() -> serde_json::Value {
    list_schema()
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
struct WorkdirAttachInput {
    workdir_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirDetachInput {}

#[derive(Debug, Serialize)]
struct WorkdirAttachRequest {
    workdir_id: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkdirAttachmentResponse {
    workspace_id: String,
    workdir_id: String,
    attached: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkdirDeleteInput {
    working_directory_id: String,
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
            "cleanliness": "clean",
            "occupied_by": {
                "runtime_id": "arcadia",
                "worker_id": "worker-opaque-64",
                "display_name": "Coder",
                "linked_at": "2026-08-12T00:00:00Z"
            }
        })
    }

    #[test]
    fn workspace_request_failure_is_not_reported_as_session_unavailable() {
        let error = workspace_workdir_error(WorkspaceClientError::Request(
            "Workspace API POST /workdir timed out".to_string(),
        ));
        assert!(matches!(error, WorkdirError::Transport(_)));
        let message = error.to_string();
        assert!(message.contains("timed out"), "{message}");
        assert!(!message.contains("session is unavailable"), "{message}");
    }

    #[test]
    fn workspace_unavailable_remains_a_session_error() {
        let error = workspace_workdir_error(WorkspaceClientError::Unavailable(
            "Workspace API could not connect".to_string(),
        ));
        assert!(matches!(error, WorkdirError::Unavailable(_)));
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
        assert_eq!(
            names,
            [
                LIST_TOOL,
                CREATE_TOOL,
                ATTACH_TOOL,
                DETACH_TOOL,
                DELETE_TOOL
            ]
        );
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
            [
                LIST_TOOL,
                CREATE_TOOL,
                ATTACH_TOOL,
                DETACH_TOOL,
                DELETE_TOOL
            ]
        );
        assert_eq!(
            names,
            [
                LIST_TOOL,
                CREATE_TOOL,
                ATTACH_TOOL,
                DETACH_TOOL,
                DELETE_TOOL
            ]
        );
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
        assert_eq!(attach_schema()["required"], json!(["workdir_id"]));
        assert!(attach_schema()["properties"].get("session_id").is_none());
        assert_eq!(delete_schema()["required"], json!(["working_directory_id"]));
    }

    #[test]
    fn legacy_runtime_worker_id_is_rejected_by_workspace_workdir_contract() {
        let mut response = json!({
            "workspace_id": "workspace/one",
            "items": [workdir_json("wd-1")],
            "diagnostics": []
        });
        response["items"][0]["occupied_by"]["runtime_worker_id"] = json!(64);

        assert!(serde_json::from_value::<WorkdirListResponse>(response).is_err());
    }

    #[test]
    fn list_create_explicit_attach_detach_and_delete_use_scoped_workspace_authority_paths() {
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
                "workdir_id": "wd-created",
                "attached": true
            })),
            response(json!({
                "workspace_id": "workspace/test",
                "workdir_id": "wd-created",
                "attached": false
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

        let listed = backend.list().unwrap();
        let listed: serde_json::Value =
            serde_json::from_str(listed.content.as_deref().unwrap()).unwrap();
        assert_eq!(
            listed["items"][0]["occupied_by"]["worker_id"],
            "worker-opaque-64"
        );
        assert!(
            listed["items"][0]["occupied_by"]
                .get("runtime_worker_id")
                .is_none()
        );
        let created = backend
            .create(WorkdirCreateInput {
                runtime_id: "runtime/one".to_string(),
                repository_id: "main".to_string(),
                selector: Some("refs/heads/topic".to_string()),
            })
            .unwrap();
        assert_eq!(created.summary, "Created Workdir wd-created");
        let created: serde_json::Value =
            serde_json::from_str(created.content.as_deref().unwrap()).unwrap();
        assert_eq!(created["item"]["working_directory_id"], "wd-created");
        assert!(created.get("attachment").is_none());
        assert_eq!(client.requests().len(), 2);
        backend
            .attach(WorkdirAttachInput {
                workdir_id: "wd-created".to_string(),
            })
            .unwrap();
        backend.detach().unwrap();
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
            "/api/w/workspace%2Ftest/workers/self/workdir-attachment"
        );
        assert_eq!(requests[2].method, WorkspaceRequestMethod::Post);
        assert_eq!(
            requests[3].path,
            "/api/w/workspace%2Ftest/workers/self/workdir-attachment"
        );
        assert_eq!(requests[3].method, WorkspaceRequestMethod::Delete);
        assert_eq!(
            requests[4].path,
            "/api/w/workspace%2Ftest/working-directories/wd-created"
        );
        assert_eq!(requests[4].method, WorkspaceRequestMethod::Delete);
    }

    #[tokio::test]
    async fn attached_session_proxies_operations_without_runtime_or_session_arguments() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "operation": "stat",
            "result": {"path": "visible.txt", "kind": "file", "size": 8}
        }))]));
        let session = WorkspaceAttachedWorkdirSession::handle(client.clone());
        let result = session
            .stat(StatRequest {
                path: workdir::WorkdirPath::new("visible.txt").unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(result.size, 8);
        let requests = client.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].path,
            "/api/w/workspace%2Ftest/workers/self/workdir-session/operations"
        );
        let body: serde_json::Value =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["operation"]["operation"], "stat");
        assert!(body.get("expected_session_fence").is_none());
        assert!(body.get("runtime_id").is_none());
        assert!(body.get("session_id").is_none());
    }

    #[tokio::test]
    async fn delegated_attached_session_carries_captured_fence_on_operations() {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![
            response(json!({"value": "attachment-fence"})),
            response(json!({
                "operation": "stat",
                "result": {"path": "visible.txt", "kind": "file", "size": 8}
            })),
        ]));
        let parent = workdir::delegation_capable_session(WorkspaceAttachedWorkdirSession::handle(
            client.clone(),
        ));
        let delegation = parent
            .delegate(workdir::WorkdirDelegationRequest {
                rules: vec![workdir::WorkdirDelegationRule {
                    target: workdir::WorkdirPath::new("").unwrap(),
                    permission: workdir::WorkdirDelegationPermission::Read,
                    recursive: false,
                }],
                cwd: workdir::WorkdirPath::new("").unwrap(),
            })
            .await
            .unwrap();
        delegation
            .scoped_session
            .stat(StatRequest {
                path: workdir::WorkdirPath::new("visible.txt").unwrap(),
            })
            .await
            .unwrap();

        let requests = client.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].path,
            "/api/w/workspace%2Ftest/workers/self/workdir-session/fence"
        );
        let body: serde_json::Value =
            serde_json::from_str(requests[1].body.as_deref().unwrap()).unwrap();
        assert_eq!(body["expected_session_fence"], "attachment-fence");
        assert_eq!(body["operation"]["operation"], "stat");
        assert_eq!(body["delegation"]["rules"][0]["target"], "");
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
