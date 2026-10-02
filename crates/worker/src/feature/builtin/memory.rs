//! Workspace-HTTP backed Memory tools.
//!
//! Runtime workers may have Workspace authority without direct local filesystem
//! authority. In that case model-visible Memory tools must go through the
//! workspace backend instead of resolving `.yoi/memory` from a Worker workdir.

use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use memory::backend::{
    MemoryBackendHttpResponse, MemoryBackendOperation, MemoryBackendOperationResult,
    MemoryConsolidateStagingOperation, MemoryConsolidationOutput, MemoryDocumentReadOperation,
    MemoryDocumentUpdateOperation, MemoryQueryOperation, MemoryStagingCloseOperation,
    MemoryStagingListOperation, MemoryStagingReadOperation, MemoryToolOutput,
};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::worker::{
    SystemPromptContribution, SystemPromptContributionSource, WorkspaceClient,
    WorkspaceClientError, WorkspaceRequest, WorkspaceRequestMethod,
};

#[derive(Clone, Debug)]
pub struct WorkspaceHttpMemoryBackend {
    client: Arc<dyn WorkspaceClient>,
}

impl WorkspaceHttpMemoryBackend {
    pub fn new(client: Arc<dyn WorkspaceClient>) -> Self {
        Self { client }
    }

    pub async fn execute_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_memory_backend(self.client.as_ref(), operation).await
    }

    async fn execute(&self, operation: MemoryBackendOperation) -> Result<ToolOutput, ToolError> {
        match self.execute_operation(operation).await {
            Ok(MemoryBackendOperationResult::ToolOutput(output)) => Ok(tool_output(output)),
            Ok(result) => Err(ToolError::ExecutionFailed(format!(
                "unexpected memory backend result for model-visible tool: {result:?}"
            ))),
            Err(error) => Err(ToolError::ExecutionFailed(error.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceMemoryBackendError {
    #[error("workspace memory backend is unavailable: {reason}")]
    Unavailable { reason: String },
    #[error("workspace memory backend request failed: {0}")]
    Request(#[from] WorkspaceClientError),
    #[error("workspace memory backend returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("decode memory backend response: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("workspace memory backend rejected operation: {0}")]
    Backend(String),
}

impl dyn WorkspaceClient + '_ {
    pub async fn execute_memory_backend_operation(
        &self,
        operation: MemoryBackendOperation,
    ) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
        execute_memory_backend(self, operation).await
    }

    pub async fn request_memory_staging_consolidation(
        &self,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
        execute_memory_consolidation(self, operation, false).await
    }

    pub async fn request_subjektiv_memory_staging_consolidation(
        &self,
        operation: MemoryConsolidateStagingOperation,
    ) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
        execute_memory_consolidation(self, operation, true).await
    }

    pub async fn prepare_subjektiv_memory_surface(
        &self,
    ) -> Result<server_api::SubjektivSurfacePrepareResponse, WorkspaceMemoryBackendError> {
        match execute_subjektiv_memory_operation(
            self,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                server_api::SubjektivSurfacePrepareRequest {},
            ),
        )
        .await?
        {
            server_api::SubjektivMemoryBackendResponse::SurfacePrepared(output) => Ok(output),
            other => Err(WorkspaceMemoryBackendError::Backend(format!(
                "unexpected surface preparation response: {other:?}"
            ))),
        }
    }

    pub async fn publish_subjektiv_memory_surface(
        &self,
        input: server_api::SubjektivSurfacePublishRequest,
    ) -> Result<server_api::SubjektivSurfacePublishResponse, WorkspaceMemoryBackendError> {
        match execute_subjektiv_memory_operation(
            self,
            server_api::SubjektivMemoryBackendOperation::PublishSurface(input),
        )
        .await?
        {
            server_api::SubjektivMemoryBackendResponse::SurfacePublished(output) => Ok(output),
            other => Err(WorkspaceMemoryBackendError::Backend(format!(
                "unexpected surface publication response: {other:?}"
            ))),
        }
    }

    pub async fn fail_subjektiv_memory_surface(
        &self,
        input: server_api::SubjektivSurfaceFailureRequest,
    ) -> Result<server_api::SubjektivSurfaceFailureResponse, WorkspaceMemoryBackendError> {
        match execute_subjektiv_memory_operation(
            self,
            server_api::SubjektivMemoryBackendOperation::FailSurface(input),
        )
        .await?
        {
            server_api::SubjektivMemoryBackendResponse::SurfaceFailed(output) => Ok(output),
            other => Err(WorkspaceMemoryBackendError::Backend(format!(
                "unexpected surface failure response: {other:?}"
            ))),
        }
    }
}

async fn execute_subjektiv_memory_operation(
    client: &dyn WorkspaceClient,
    operation: server_api::SubjektivMemoryBackendOperation,
) -> Result<server_api::SubjektivMemoryBackendResponse, WorkspaceMemoryBackendError> {
    let workspace_id =
        client
            .workspace_id()
            .ok_or_else(|| WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{}` has no workspace id",
                    client.kind()
                ),
            })?;
    let response = client.execute(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        format!("/api/w/{workspace_id}/subjektiv/memory"),
        serde_json::to_string(&server_api::SubjektivMemoryBackendRequest { operation })?,
    ))?;
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceMemoryBackendError::Http {
            status,
            body: response.body,
        });
    }
    Ok(serde_json::from_str(&response.body)?)
}

async fn execute_memory_backend(
    client: &dyn WorkspaceClient,
    operation: MemoryBackendOperation,
) -> Result<MemoryBackendOperationResult, WorkspaceMemoryBackendError> {
    let workspace_id =
        client
            .workspace_id()
            .ok_or_else(|| WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{}` has no workspace id",
                    client.kind()
                ),
            })?;
    let response = client.execute(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        format!("/api/w/{workspace_id}/memory/backend"),
        serde_json::to_string(&server_api::MemoryBackendRequest(operation))?,
    ))?;
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceMemoryBackendError::Http {
            status,
            body: response.body,
        });
    }
    match serde_json::from_str::<server_api::MemoryBackendResponse>(&response.body)?.0 {
        MemoryBackendHttpResponse::Ok { result } => Ok(result),
        MemoryBackendHttpResponse::Error { message } => {
            Err(WorkspaceMemoryBackendError::Backend(message))
        }
    }
}

async fn execute_memory_consolidation(
    client: &dyn WorkspaceClient,
    operation: MemoryConsolidateStagingOperation,
    subjektiv: bool,
) -> Result<MemoryConsolidationOutput, WorkspaceMemoryBackendError> {
    let workspace_id =
        client
            .workspace_id()
            .ok_or_else(|| WorkspaceMemoryBackendError::Unavailable {
                reason: format!(
                    "workspace client kind `{}` has no workspace id",
                    client.kind()
                ),
            })?;
    let response = client.execute(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        format!(
            "/api/w/{workspace_id}/{}",
            if subjektiv {
                "subjektiv/consolidation"
            } else {
                "memory/consolidation"
            }
        ),
        serde_json::to_string(&server_api::MemoryConsolidateStagingRequest {
            force: operation.force,
        })?,
    ))?;
    let status = reqwest::StatusCode::from_u16(response.status)
        .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    if !response.is_success() {
        return Err(WorkspaceMemoryBackendError::Http {
            status,
            body: response.body,
        });
    }
    let response = serde_json::from_str::<server_api::MemoryConsolidationResponse>(&response.body)?;
    Ok(MemoryConsolidationOutput {
        status: response.status,
        summary: response.summary,
        candidate_count: response.candidate_count,
        total_bytes: response.total_bytes,
    })
}

pub fn workspace_http_memory_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    let backend = WorkspaceHttpMemoryBackend::new(client);
    vec![
        memory_tool(
            "MemoryReadDocument",
            READ_DOCUMENT_DESCRIPTION,
            document_read_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::ReadDocument(parse_input::<
                    MemoryDocumentReadOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryUpdateDocument",
            UPDATE_DOCUMENT_DESCRIPTION,
            document_update_schema(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::UpdateDocument(parse_input::<
                    MemoryDocumentUpdateOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryQuery",
            QUERY_DESCRIPTION,
            query_schema(),
            backend,
            |input| {
                Ok(MemoryBackendOperation::Query(parse_input::<
                    MemoryQueryOperation,
                >(input)?))
            },
        ),
    ]
}

pub fn workspace_http_subjektiv_consolidation_tools(
    client: Arc<dyn WorkspaceClient>,
) -> Vec<ToolDefinition> {
    use SubjectConsolidationOperation as Operation;
    [
        (
            "SubjektivMemoryQuery",
            "Search current Memory revisions for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryQueryRequest>(),
            Operation::Query,
        ),
        (
            "SubjektivMemoryRead",
            "Read one current or historical Memory revision for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryReadRequest>(),
            Operation::Read,
        ),
        (
            "SubjektivMemoryListRevisions",
            "List immutable revisions of one Memory for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryListRevisionsRequest>(),
            Operation::ListRevisions,
        ),
        (
            "MemoryStagingList",
            "List pending staging candidates for the delegated subject.",
            schema_for::<server_api::SubjektivMemoryCandidateListRequest>(),
            Operation::ListCandidates,
        ),
        (
            "MemoryStagingRead",
            "Read one pending candidate, including typed revision proposal metadata and provenance.",
            schema_for::<server_api::SubjektivMemoryCandidateReadRequest>(),
            Operation::ReadCandidate,
        ),
        (
            "MemoryApplyCandidate",
            "Atomically apply or reject one subject candidate. A request_id is idempotent; application revalidates exact revision proposal metadata and returns concrete affected revision refs.",
            schema_for::<server_api::SubjektivMemoryCandidateDecisionRequest>(),
            Operation::DecideCandidate,
        ),
    ]
    .into_iter()
    .map(|(name, description, schema, operation)| {
        let client = Arc::clone(&client);
        Arc::new(move || {
            (
                ToolMeta::new(name)
                    .description(description)
                    .input_schema(schema.clone()),
                Arc::new(SubjectConsolidationTool {
                    client: Arc::clone(&client),
                    operation,
                }) as Arc<dyn Tool>,
            )
        }) as ToolDefinition
    })
    .collect()
}

#[derive(Clone, Copy)]
enum SubjectConsolidationOperation {
    Query,
    Read,
    ListRevisions,
    ListCandidates,
    ReadCandidate,
    DecideCandidate,
}

struct SubjectConsolidationTool {
    client: Arc<dyn WorkspaceClient>,
    operation: SubjectConsolidationOperation,
}

#[async_trait]
impl Tool for SubjectConsolidationTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        use SubjectConsolidationOperation as Operation;
        let operation = match self.operation {
            Operation::Query => {
                server_api::SubjektivMemoryBackendOperation::Query(parse_input(input_json)?)
            }
            Operation::Read => {
                server_api::SubjektivMemoryBackendOperation::Read(parse_input(input_json)?)
            }
            Operation::ListRevisions => {
                server_api::SubjektivMemoryBackendOperation::ListRevisions(parse_input(input_json)?)
            }
            Operation::ListCandidates => {
                server_api::SubjektivMemoryBackendOperation::ListCandidates(parse_input(
                    input_json,
                )?)
            }
            Operation::ReadCandidate => {
                server_api::SubjektivMemoryBackendOperation::ReadCandidate(parse_input(input_json)?)
            }
            Operation::DecideCandidate => {
                server_api::SubjektivMemoryBackendOperation::DecideCandidate(parse_input(
                    input_json,
                )?)
            }
        };
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "subjektiv consolidation requires Workspace authority".into(),
            )
        })?;
        let response = self
            .client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                format!("/api/w/{workspace_id}/subjektiv/memory"),
                serde_json::to_string(&server_api::SubjektivMemoryBackendRequest { operation })
                    .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
            ))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            let parsed =
                serde_json::from_str::<server_api::RepositoryApiError>(&response.body).ok();
            if let Some((code, message)) = parsed.as_ref().and_then(|error| {
                error
                    .diagnostics
                    .iter()
                    .find(|diagnostic| {
                        matches!(
                            diagnostic.code.as_str(),
                            "revision_conflict"
                                | "candidate_decision_conflict"
                                | "subject_scope_mismatch"
                        )
                    })
                    .map(|diagnostic| (diagnostic.code.clone(), error.message.clone()))
            }) {
                return Ok(ToolOutput {
                    summary: format!("Candidate decision requires reread: {code}."),
                    content: Some(
                        serde_json::json!({
                            "status": "error",
                            "error": { "code": code, "message": message }
                        })
                        .to_string(),
                    ),
                    attachments: Vec::new(),
                });
            }
            let detail = parsed.map(|error| error.message).unwrap_or(response.body);
            return Err(if matches!(response.status, 400 | 404 | 409 | 422) {
                ToolError::InvalidArgument(detail)
            } else {
                ToolError::ExecutionFailed(detail)
            });
        }
        let response: server_api::SubjektivMemoryBackendResponse =
            serde_json::from_str(&response.body)
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let summary = match &response {
            server_api::SubjektivMemoryBackendResponse::Query(value) => {
                format!("Found {} subject Memory item(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Read(value) => {
                format!(
                    "Read Memory {} revision {}.",
                    value.memory_id, value.revision
                )
            }
            server_api::SubjektivMemoryBackendResponse::ListRevisions(value) => {
                format!("Listed {} revision(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Candidates(value) => {
                format!("Listed {} pending candidate(s).", value.items.len())
            }
            server_api::SubjektivMemoryBackendResponse::Candidate(value) => {
                format!("Read pending candidate {}.", value.candidate_id)
            }
            server_api::SubjektivMemoryBackendResponse::CandidateDecided(value) => {
                format!("Candidate {} decision committed.", value.candidate_id)
            }
            _ => {
                return Err(ToolError::ExecutionFailed(
                    "unexpected subjektiv consolidation response".into(),
                ));
            }
        };
        Ok(ToolOutput {
            summary,
            content: Some(
                serde_json::to_string_pretty(&response)
                    .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
            ),
            attachments: Vec::new(),
        })
    }
}

pub fn workspace_http_memory_consolidation_tools(
    client: Arc<dyn WorkspaceClient>,
) -> Vec<ToolDefinition> {
    let mut tools = workspace_http_memory_tools(client.clone());
    let backend = WorkspaceHttpMemoryBackend::new(client);
    tools.extend([
        memory_tool(
            "MemoryStagingList",
            STAGING_LIST_DESCRIPTION,
            schema_for::<MemoryStagingListOperation>(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::StagingList(parse_input::<
                    MemoryStagingListOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryStagingRead",
            STAGING_READ_DESCRIPTION,
            schema_for::<MemoryStagingReadOperation>(),
            backend.clone(),
            |input| {
                Ok(MemoryBackendOperation::StagingRead(parse_input::<
                    MemoryStagingReadOperation,
                >(
                    input
                )?))
            },
        ),
        memory_tool(
            "MemoryStagingClose",
            STAGING_CLOSE_DESCRIPTION,
            schema_for::<MemoryStagingCloseOperation>(),
            backend,
            |input| {
                Ok(MemoryBackendOperation::StagingClose(parse_input::<
                    MemoryStagingCloseOperation,
                >(
                    input
                )?))
            },
        ),
    ]);
    tools
}

type OperationBuilder = fn(&str) -> Result<MemoryBackendOperation, ToolError>;

fn memory_tool(
    name: &'static str,
    description: &'static str,
    schema: serde_json::Value,
    backend: WorkspaceHttpMemoryBackend,
    build: OperationBuilder,
) -> ToolDefinition {
    Arc::new(move || {
        (
            ToolMeta::new(name)
                .description(description)
                .input_schema(schema.clone()),
            Arc::new(WorkspaceHttpMemoryTool {
                backend: backend.clone(),
                build,
            }) as Arc<dyn Tool>,
        )
    })
}

#[derive(Clone)]
struct WorkspaceHttpMemoryTool {
    backend: WorkspaceHttpMemoryBackend,
    build: OperationBuilder,
}

#[async_trait]
impl Tool for WorkspaceHttpMemoryTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let operation = (self.build)(input_json)?;
        self.backend.execute(operation).await
    }
}

fn parse_input<T: DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}

fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).expect("memory tool schema should serialize")
}

fn tool_output(output: MemoryToolOutput) -> ToolOutput {
    ToolOutput {
        summary: output.summary,
        content: output.content,
        attachments: Vec::new(),
    }
}

const READ_DOCUMENT_DESCRIPTION: &str =
    "Read the Workspace memory Markdown document through Workspace authority.";
const UPDATE_DOCUMENT_DESCRIPTION: &str =
    "Edit the Workspace memory Markdown document by replacing an exact old_string with new_string.";
const QUERY_DESCRIPTION: &str = "Query the Workspace memory document through Workspace authority.";
const STAGING_LIST_DESCRIPTION: &str =
    "List pending Memory staging candidates without loading full record payloads.";
const STAGING_READ_DESCRIPTION: &str = "Read one pending Memory staging candidate by candidate_id.";
const STAGING_CLOSE_DESCRIPTION: &str = "Close one staging candidate with a required reason; records disposition and deletes the staging record.";

fn document_read_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "offset":{"type":["integer","null"],"minimum":0},
            "limit":{"type":["integer","null"],"minimum":0}
        }
    })
}

fn document_update_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "required":["old_string", "new_string"],
        "properties":{
            "old_string":{"type":"string", "minLength": 1},
            "new_string":{"type":"string"},
            "replace_all":{"type":"boolean", "default": false}
        }
    })
}

fn query_schema() -> serde_json::Value {
    json!({
        "type":"object",
        "additionalProperties": false,
        "properties":{
            "query":{"type":["string","null"]}
        }
    })
}

struct WorkspaceResidentSummarySource {
    client: Arc<dyn WorkspaceClient>,
}

#[async_trait]
impl SystemPromptContributionSource for WorkspaceResidentSummarySource {
    async fn load(&self) -> SystemPromptContribution {
        match self
            .client
            .execute_memory_backend_operation(
                memory::backend::MemoryBackendOperation::ResidentSummary(
                    memory::backend::MemoryResidentSummaryOperation::default(),
                ),
            )
            .await
        {
            Ok(memory::backend::MemoryBackendOperationResult::ResidentSummary(output))
                if output.availability
                    == memory::backend::MemoryResidentSummaryAvailability::Ready =>
            {
                SystemPromptContribution::Ready(output.content.unwrap_or_default())
            }
            Ok(memory::backend::MemoryBackendOperationResult::ResidentSummary(_)) => {
                SystemPromptContribution::Unavailable
            }
            Ok(other) => {
                tracing::debug!(?other, "unexpected resident Memory Backend result");
                SystemPromptContribution::Unavailable
            }
            Err(error) => {
                tracing::debug!(%error, "resident Memory summary unavailable");
                SystemPromptContribution::Unavailable
            }
        }
    }
}

pub(crate) struct MemoryFeatureInstallPlan {
    pub(crate) module: MemoryToolsFeature,
    pub(crate) resident_summary_source: Option<Arc<dyn SystemPromptContributionSource>>,
    pub(crate) system_prompt_override: Option<String>,
    pub(crate) resolved_config: manifest::ResolvedMemoryFeatureConfig,
}

impl MemoryFeatureInstallPlan {
    pub fn prepare(
        manifest: &manifest::WorkerManifest,
        client: Arc<dyn WorkspaceClient>,
        prompts: Arc<crate::prompt::catalog::PromptCatalog>,
    ) -> std::io::Result<Option<Self>> {
        Self::prepare_resolved(
            manifest.feature.memory.clone(),
            client,
            prompts,
            manifest.profile.clone(),
        )
    }

    fn prepare_resolved(
        config: manifest::ResolvedMemoryFeatureConfig,
        client: Arc<dyn WorkspaceClient>,
        prompts: Arc<crate::prompt::catalog::PromptCatalog>,
        profile: Option<manifest::ProfileManifestSnapshot>,
    ) -> std::io::Result<Option<Self>> {
        let profile_name = profile
            .as_ref()
            .and_then(|snapshot| match &snapshot.source {
                manifest::ProfileSource::Registry {
                    source: manifest::ProfileRegistrySource::Builtin,
                    name,
                    ..
                } => Some(name.as_str()),
                _ => None,
            });
        let memory_consolidation_worker = matches!(
            profile_name,
            Some("memory-consolidation" | "subjektiv-memory-consolidation")
        );
        let subjektiv_consolidation_worker = profile_name == Some("subjektiv-memory-consolidation");
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if !config.profile.enabled {
            return Ok(None);
        }
        let workspace_id = client.workspace_id().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Memory tools require Backend Workspace API authority",
            )
        })?;
        let settings = config
            .workspace_settings()
            .expect("validated enabled Memory config has Workspace settings");
        if settings.workspace_id != workspace_id {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Memory settings belong to {} instead of {}",
                    settings.workspace_id, workspace_id
                ),
            ));
        }

        let resident_summary_source = config.profile.resident.inject_summary.then(|| {
            Arc::new(WorkspaceResidentSummarySource {
                client: Arc::clone(&client),
            }) as Arc<dyn SystemPromptContributionSource>
        });
        let system_prompt_override = if memory_consolidation_worker {
            let language = settings.language;
            Some(
                if subjektiv_consolidation_worker {
                    prompts.subjektiv_memory_consolidation_system(&language)
                } else {
                    prompts.memory_consolidation_system(&language)
                }
                .map_err(|error| std::io::Error::other(error.to_string()))?,
            )
        } else {
            None
        };

        let module = if subjektiv_consolidation_worker {
            MemoryToolsFeature::from_tools(workspace_http_subjektiv_consolidation_tools(client))
        } else {
            MemoryToolsFeature::new(client, config.profile.staging_tools)
        };
        Ok(Some(Self {
            module,
            resident_summary_source,
            system_prompt_override,
            resolved_config: config,
        }))
    }
}

#[derive(Clone)]
pub struct MemoryToolsFeature {
    tools: Vec<ToolDefinition>,
}

impl MemoryToolsFeature {
    pub fn new(client: Arc<dyn WorkspaceClient>, staging_tools: bool) -> Self {
        let tools = if staging_tools {
            workspace_http_memory_consolidation_tools(client)
        } else {
            workspace_http_memory_tools(client)
        };
        Self { tools }
    }

    fn from_tools(tools: Vec<ToolDefinition>) -> Self {
        Self { tools }
    }
}

impl FeatureModule for MemoryToolsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin("memory", "Memory")
            .with_description("Workspace Memory document, query, and staging tools.");
        for tool in &self.tools {
            let (meta, _) = tool();
            descriptor = descriptor.with_tool(ToolDeclaration::new(meta.name, meta.description));
        }
        descriptor
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for tool in &self.tools {
            let (meta, _) = tool();
            context
                .tools()
                .register(ToolContribution::new(meta.name, tool.clone()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::tool::ToolDefinition;

    fn test_client() -> Arc<dyn WorkspaceClient> {
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            "http://backend",
        ))
    }

    fn resident_client(content: &str) -> Arc<dyn WorkspaceClient> {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let content = content.to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            let body = serde_json::json!({
                "status": "ok",
                "result": {
                    "kind": "resident_summary",
                    "availability": "ready",
                    "content": content,
                }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            format!("http://{addr}"),
        ))
    }

    fn repository_error_client(status: u16, code: &str) -> Arc<dyn WorkspaceClient> {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = serde_json::json!({
            "error": "request rejected",
            "message": format!("{code}: retry after reread"),
            "diagnostics": [{
                "code": code,
                "severity": "error",
                "message": format!("{code}: retry after reread"),
            }],
        })
        .to_string();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            let response = format!(
                "HTTP/1.1 {status} rejected\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        Arc::new(crate::worker::TestWorkspaceHttpClient::new(
            "workspace",
            format!("http://{addr}"),
        ))
    }

    fn tool_names(definitions: Vec<ToolDefinition>) -> Vec<String> {
        let mut names = definitions
            .into_iter()
            .map(|tool| tool().0.name)
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    fn tool_meta(definitions: Vec<ToolDefinition>, name: &str) -> serde_json::Value {
        definitions
            .into_iter()
            .map(|tool| tool().0)
            .find(|meta| meta.name == name)
            .unwrap_or_else(|| panic!("missing tool meta for {name}"))
            .input_schema
    }

    #[tokio::test]
    async fn memory_install_plan_is_the_fail_closed_config_boundary() {
        let prompts = crate::prompt::catalog::PromptCatalog::builtins_only().unwrap();
        let disabled = MemoryFeatureInstallPlan::prepare_resolved(
            manifest::ResolvedMemoryFeatureConfig::default(),
            test_client(),
            prompts.clone(),
            None,
        )
        .unwrap();
        assert!(disabled.is_none());

        let mut enabled = manifest::ResolvedMemoryFeatureConfig::default();
        enabled.profile.enabled = true;
        enabled.profile.resident.inject_summary = false;
        assert!(
            MemoryFeatureInstallPlan::prepare_resolved(
                enabled.clone(),
                test_client(),
                prompts.clone(),
                None,
            )
            .is_err()
        );
        enabled
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            })
            .unwrap();
        let mut foreign = enabled.clone();
        foreign.workspace_settings.as_mut().unwrap().workspace_id = "other-workspace".to_string();
        assert!(
            MemoryFeatureInstallPlan::prepare_resolved(
                foreign,
                test_client(),
                prompts.clone(),
                None,
            )
            .is_err()
        );
        let plan = MemoryFeatureInstallPlan::prepare_resolved(
            enabled.clone(),
            test_client(),
            prompts.clone(),
            None,
        )
        .unwrap()
        .unwrap();
        assert!(plan.resident_summary_source.is_none());
        assert!(plan.system_prompt_override.is_none());

        enabled.profile.resident.inject_summary = true;
        let plan = MemoryFeatureInstallPlan::prepare_resolved(
            enabled,
            resident_client("# Durable Memory"),
            prompts,
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            plan.resident_summary_source.unwrap().load().await,
            SystemPromptContribution::Ready("# Durable Memory".into())
        );
    }

    #[tokio::test]
    async fn memory_prompt_contribution_defers_resident_summary_until_loaded() {
        let prompts = crate::prompt::catalog::PromptCatalog::builtins_only().unwrap();
        let mut config = manifest::ResolvedMemoryFeatureConfig::default();
        config.profile.enabled = true;
        config
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace".to_string(),
                settings_revision: 1,
                language: "English".to_string(),
            })
            .unwrap();

        let first = MemoryFeatureInstallPlan::prepare_resolved(
            config.clone(),
            resident_client("first resident summary"),
            prompts.clone(),
            None,
        )
        .unwrap()
        .unwrap();
        let restored = MemoryFeatureInstallPlan::prepare_resolved(
            config,
            resident_client("updated resident summary"),
            prompts,
            None,
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            first.resident_summary_source.unwrap().load().await,
            SystemPromptContribution::Ready("first resident summary".into())
        );
        assert_eq!(
            restored.resident_summary_source.unwrap().load().await,
            SystemPromptContribution::Ready("updated resident summary".into())
        );
    }

    #[test]
    fn memory_feature_owns_normal_and_staging_tool_surfaces() {
        let normal = MemoryToolsFeature::new(test_client(), false);
        let normal_names = tool_names(normal.tools);
        assert!(normal_names.contains(&"MemoryQuery".to_string()));
        assert!(!normal_names.contains(&"MemoryStagingList".to_string()));

        let staging = MemoryToolsFeature::new(test_client(), true);
        assert_eq!(staging.descriptor().id.as_str(), "builtin:memory");
        let staging_names = tool_names(staging.tools);
        assert!(staging_names.contains(&"MemoryQuery".to_string()));
        assert!(staging_names.contains(&"MemoryStagingList".to_string()));
    }

    #[test]
    fn normal_workspace_memory_tools_do_not_include_staging_tools() {
        let names = tool_names(workspace_http_memory_tools(test_client()));

        assert!(names.contains(&"MemoryQuery".to_string()));
        assert!(names.contains(&"MemoryReadDocument".to_string()));
        assert!(names.contains(&"MemoryUpdateDocument".to_string()));
        assert!(!names.contains(&"MemoryRead".to_string()));
        assert!(!names.contains(&"MemoryWrite".to_string()));
        assert!(!names.contains(&"MemoryEdit".to_string()));
        assert!(!names.contains(&"MemoryDelete".to_string()));
        assert!(!names.contains(&"MemoryStagingList".to_string()));
        assert!(!names.contains(&"MemoryStagingRead".to_string()));
        assert!(!names.contains(&"MemoryStagingClose".to_string()));
    }

    #[test]
    fn document_update_schema_is_edit_like_and_staging_close_has_no_legacy_kinds() {
        let update_schema = tool_meta(
            workspace_http_memory_tools(test_client()),
            "MemoryUpdateDocument",
        );
        assert_eq!(
            update_schema["required"],
            serde_json::json!(["old_string", "new_string"])
        );
        assert!(update_schema["properties"].get("old_string").is_some());
        assert!(update_schema["properties"].get("new_string").is_some());
        assert!(update_schema["properties"].get("replace_all").is_some());
        assert!(update_schema["properties"].get("body_md").is_none());

        let close_schema_text = tool_meta(
            workspace_http_memory_consolidation_tools(test_client()),
            "MemoryStagingClose",
        )
        .to_string();
        assert!(close_schema_text.contains("affected_memory"));
        assert!(close_schema_text.contains("edit"));
        assert!(!close_schema_text.contains("summary"));
        assert!(!close_schema_text.contains("decision"));
        assert!(!close_schema_text.contains("request"));
        assert!(!close_schema_text.contains("slug"));
    }

    #[test]
    fn consolidation_workspace_memory_tools_include_staging_tools() {
        let names = tool_names(workspace_http_memory_consolidation_tools(test_client()));

        assert!(names.contains(&"MemoryQuery".to_string()));
        assert!(names.contains(&"MemoryReadDocument".to_string()));
        assert!(names.contains(&"MemoryUpdateDocument".to_string()));
        assert!(names.contains(&"MemoryStagingList".to_string()));
        assert!(names.contains(&"MemoryStagingRead".to_string()));
        assert!(names.contains(&"MemoryStagingClose".to_string()));
    }

    #[tokio::test]
    async fn subjektiv_candidate_conflicts_survive_workspace_http_transport() {
        let input = serde_json::json!({
            "request_id": "decision-request-1",
            "candidate_id": "candidate-1",
            "reason": "candidate is already covered",
            "decision": {
                "kind": "close",
                "action": "already_covered",
                "affected_memory": [],
            },
        })
        .to_string();
        for (status, code) in [
            (409, "candidate_decision_conflict"),
            (403, "subject_scope_mismatch"),
        ] {
            let tool = SubjectConsolidationTool {
                client: repository_error_client(status, code),
                operation: SubjectConsolidationOperation::DecideCandidate,
            };
            let output = tool
                .execute(&input, ToolExecutionContext::direct())
                .await
                .expect("typed conflicts are model-visible retry output");
            let content: serde_json::Value =
                serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(content["status"], "error");
            assert_eq!(content["error"]["code"], code);
            assert!(output.summary.contains("requires reread"));
        }
    }

    #[test]
    fn subjektiv_consolidation_has_atomic_decision_without_host_scope_fields() {
        let tools = workspace_http_subjektiv_consolidation_tools(test_client());
        let names = tool_names(tools.clone());
        assert_eq!(
            names,
            vec![
                "MemoryApplyCandidate",
                "MemoryStagingList",
                "MemoryStagingRead",
                "SubjektivMemoryListRevisions",
                "SubjektivMemoryQuery",
                "SubjektivMemoryRead",
            ]
        );
        let decision = tool_meta(tools, "MemoryApplyCandidate").to_string();
        assert!(decision.contains("request_id"));
        assert!(decision.contains("expected_revision"));
        assert!(decision.contains("already_covered"));
        assert!(!decision.contains("subject_id"));
        assert!(!decision.contains("workspace_id"));
        assert!(!decision.contains("runtime_id"));
        assert!(!decision.contains("worker_id"));
        assert!(!decision.contains("source_candidate_ids"));
    }
}
