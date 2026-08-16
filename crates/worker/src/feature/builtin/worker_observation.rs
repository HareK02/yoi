use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::Item;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::collect_state;

use super::manage_worker::{WORKER_CONTROL_SERVICE_ID, WorkerControlService};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureInstructionContribution,
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureModule, ServiceId,
    ServiceRequirement, ToolContribution, ToolDeclaration,
};
use crate::session_capture::{
    ReadDetail, ReadOptions, ReadSelector, ReferenceKind, SearchOptions, SessionCapture,
    SessionEntryRef, ToolPart,
};
use crate::spawn::registry::SpawnedWorkerRegistry;

const MAX_SUBJECTS: usize = 100;
const DEFAULT_PAGE_LIMIT: usize = 20;
const MAX_PAGE_LIMIT: usize = 100;
const MAX_READ_BYTES: usize = 16 * 1024;
const OBSERVATION_INSTRUCTION_ID: &str = "worker-observation.policy";
const OBSERVATION_PROMPT_REF: &str = "common.worker_observation";

fn observation_instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(OBSERVATION_INSTRUCTION_ID),
        OBSERVATION_PROMPT_REF,
        "Worker session observation authority and privacy policy",
    )
    .expect("static worker-observation instruction declaration is valid")
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerObservationSubjectRef {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
    SubWorker {
        name: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerObservationSubject {
    pub subject: WorkerObservationSubjectRef,
    pub display_name: String,
    pub relation: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct WorkerSessionCapture {
    pub segment_id: String,
    pub items: Vec<Item>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerObservationError {
    #[error("worker session was not found or is not accessible")]
    NotFound,
    #[error("worker session observation failed: {0}")]
    Unavailable(String),
}

#[async_trait]
pub trait WorkerObservationProvider: Send + Sync {
    /// Returns only subjects already authorized for the current Worker.
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError>;

    /// Reauthorizes and captures the latest committed session for one subject.
    /// Unauthorized and missing subjects must both return `NotFound`.
    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError>;
}

#[derive(Debug, Deserialize)]
struct WorkspaceWorkerObservationListResponse {
    sessions: Vec<WorkerObservationSubject>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceWorkerObservationCaptureResponse {
    segment_id: String,
    entries: Vec<serde_json::Value>,
}

pub struct WorkspaceClientWorkerObservationProvider {
    client: Arc<dyn crate::worker::WorkspaceClient>,
}

impl WorkspaceClientWorkerObservationProvider {
    pub fn new(client: Arc<dyn crate::worker::WorkspaceClient>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl WorkerObservationProvider for WorkspaceClientWorkerObservationProvider {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            WorkerObservationError::Unavailable(
                "Workspace observation requires a scoped Workspace client".to_string(),
            )
        })?;
        let response = self
            .client
            .execute(crate::worker::WorkspaceRequest::get(format!(
                "/api/w/{}/worker-observation/sessions",
                workspace_id
            )))
            .map_err(workspace_client_error)?;
        let body = workspace_response_body(response)?;
        serde_json::from_str::<WorkspaceWorkerObservationListResponse>(&body)
            .map(|response| response.sessions)
            .map_err(|error| WorkerObservationError::Unavailable(error.to_string()))
    }

    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        let body = serde_json::to_string(subject)
            .map_err(|error| WorkerObservationError::Unavailable(error.to_string()))?;
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            WorkerObservationError::Unavailable(
                "Workspace observation requires a scoped Workspace client".to_string(),
            )
        })?;
        let response = self
            .client
            .execute(crate::worker::WorkspaceRequest::json(
                crate::worker::WorkspaceRequestMethod::Post,
                format!("/api/w/{}/worker-observation/session", workspace_id),
                body,
            ))
            .map_err(workspace_client_error)?;
        let body = workspace_response_body(response)?;
        let response = serde_json::from_str::<WorkspaceWorkerObservationCaptureResponse>(&body)
            .map_err(|error| WorkerObservationError::Unavailable(error.to_string()))?;
        let entries = response
            .entries
            .into_iter()
            .map(|entry| {
                serde_json::from_value(entry)
                    .map_err(|error| WorkerObservationError::Unavailable(error.to_string()))
            })
            .collect::<Result<Vec<session_store::LogEntry>, _>>()?;
        let state = collect_state(&entries);
        Ok(WorkerSessionCapture {
            segment_id: response.segment_id,
            items: state.history,
        })
    }
}

fn workspace_response_body(
    response: crate::worker::WorkspaceResponse,
) -> Result<String, WorkerObservationError> {
    match response.status {
        200..=299 => Ok(response.body),
        403 | 404 => Err(WorkerObservationError::NotFound),
        status => Err(WorkerObservationError::Unavailable(format!(
            "Workspace observation request failed with status {status}: {}",
            response.body
        ))),
    }
}

fn workspace_client_error(error: crate::worker::WorkspaceClientError) -> WorkerObservationError {
    WorkerObservationError::Unavailable(error.to_string())
}

#[derive(Clone)]
struct ControlAuthorizedObservationProvider {
    control: Arc<dyn WorkerControlService>,
    inner: Arc<dyn WorkerObservationProvider>,
}

#[async_trait]
impl WorkerObservationProvider for ControlAuthorizedObservationProvider {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        let candidates = self.inner.list_worker_sessions().await?;
        let mut granted = Vec::new();
        for candidate in candidates {
            if self
                .control
                .ensure_permission(&candidate.subject, "observe")
                .await
                .is_ok()
            {
                granted.push(candidate);
            }
        }
        Ok(granted)
    }

    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        self.control
            .ensure_permission(subject, "observe")
            .await
            .map_err(|_| WorkerObservationError::NotFound)?;
        self.inner.capture_worker_session(subject).await
    }
}

pub struct WorkerObservationFeature {
    provider: Arc<dyn WorkerObservationProvider>,
}

impl WorkerObservationFeature {
    pub fn new(provider: Arc<dyn WorkerObservationProvider>) -> Self {
        Self { provider }
    }
}

impl FeatureModule for WorkerObservationFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("worker-observation", "Worker Observation")
            .with_description(
                "Read-only exploration of explicitly granted active Worker sessions.",
            )
            .with_service_requirement(ServiceRequirement::required(
                ServiceId::builtin(WORKER_CONTROL_SERVICE_ID),
                "Worker observation extends the known-Worker control authority",
            ))
            .with_instruction(observation_instruction())
            .with_tool(ToolDeclaration::new(
                "ViewSessionOverview",
                "Show a sparse overview of the latest committed capture for one granted Worker session.",
            ))
            .with_tool(ToolDeclaration::new(
                "SearchSessionEntries",
                "Search or compactly list a bounded range in one granted Worker session.",
            ))
            .with_tool(ToolDeclaration::new(
                "ReadSessionEntry",
                "Read one committed entry from one granted Worker session by SessionEntryRef.",
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context
            .instructions()
            .register(FeatureInstructionContribution::new(
                observation_instruction(),
            ))?;
        let control = context
            .services()
            .require::<dyn WorkerControlService>(&ServiceId::builtin(WORKER_CONTROL_SERVICE_ID))?;
        let provider: Arc<dyn WorkerObservationProvider> =
            Arc::new(ControlAuthorizedObservationProvider {
                control,
                inner: self.provider.clone(),
            });
        context.tools().register(ToolContribution::new(
            "ViewSessionOverview",
            overview_definition(provider.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "SearchSessionEntries",
            search_definition(provider.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "ReadSessionEntry",
            read_definition(provider),
        ))?;
        Ok(())
    }
}

pub struct CompositeWorkerObservationProvider {
    providers: Vec<Arc<dyn WorkerObservationProvider>>,
}

impl CompositeWorkerObservationProvider {
    pub fn new(providers: Vec<Arc<dyn WorkerObservationProvider>>) -> Self {
        Self { providers }
    }
}

#[async_trait]
impl WorkerObservationProvider for CompositeWorkerObservationProvider {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        let mut seen = std::collections::HashSet::new();
        let mut subjects = Vec::new();
        let mut unavailable = None;
        for provider in &self.providers {
            let provider_subjects = match provider.list_worker_sessions().await {
                Ok(subjects) => subjects,
                Err(WorkerObservationError::NotFound) => continue,
                Err(error) => {
                    unavailable.get_or_insert(error);
                    continue;
                }
            };
            for subject in provider_subjects {
                if seen.insert(subject.subject.clone()) {
                    subjects.push(subject);
                    if subjects.len() == MAX_SUBJECTS {
                        return Ok(subjects);
                    }
                }
            }
        }
        if subjects.is_empty() {
            if let Some(error) = unavailable {
                return Err(error);
            }
        }
        Ok(subjects)
    }

    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        for provider in &self.providers {
            match provider.capture_worker_session(subject).await {
                Ok(capture) => return Ok(capture),
                Err(WorkerObservationError::NotFound) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(WorkerObservationError::NotFound)
    }
}

pub(crate) struct SpawnedSubWorkerObservationProvider {
    registry: Arc<SpawnedWorkerRegistry>,
}

impl SpawnedSubWorkerObservationProvider {
    pub(crate) fn new(registry: Arc<SpawnedWorkerRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl WorkerObservationProvider for SpawnedSubWorkerObservationProvider {
    async fn list_worker_sessions(
        &self,
    ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
        let subjects = self
            .registry
            .list_internal()
            .into_iter()
            .take(MAX_SUBJECTS)
            .map(|record| WorkerObservationSubject {
                subject: WorkerObservationSubjectRef::SubWorker {
                    name: record.worker_name.clone(),
                },
                display_name: record.worker_name,
                relation: "subworker".to_string(),
                status: format!("{:?}", record.session.status()).to_lowercase(),
            })
            .collect();
        Ok(subjects)
    }

    async fn capture_worker_session(
        &self,
        subject: &WorkerObservationSubjectRef,
    ) -> Result<WorkerSessionCapture, WorkerObservationError> {
        let WorkerObservationSubjectRef::SubWorker { name } = subject else {
            return Err(WorkerObservationError::NotFound);
        };
        let record = self
            .registry
            .get_internal(name)
            .ok_or(WorkerObservationError::NotFound)?;
        let entries = record.session.entries();
        let state = collect_state(&entries);
        Ok(WorkerSessionCapture {
            segment_id: format!("subworker:{name}"),
            items: state.history,
        })
    }
}

fn overview_definition(provider: Arc<dyn WorkerObservationProvider>) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(ViewSessionOverviewParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("ViewSessionOverview")
            .description("Show a sparse bounded index for one granted Worker session.")
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(ViewSessionOverviewTool {
            provider: provider.clone(),
        });
        (meta, tool)
    })
}

fn search_definition(provider: Arc<dyn WorkerObservationProvider>) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(SearchSessionEntriesParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("SearchSessionEntries")
            .description("Search or list a bounded range in one granted Worker session.")
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(SearchSessionEntriesTool {
            provider: provider.clone(),
        });
        (meta, tool)
    })
}

fn read_definition(provider: Arc<dyn WorkerObservationProvider>) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(ReadSessionEntryParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("ReadSessionEntry")
            .description("Read one entry by SessionEntryRef from one granted Worker session.")
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(ReadSessionEntryTool {
            provider: provider.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ViewSessionOverviewParams {
    subject: WorkerObservationSubjectRef,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchSessionEntriesParams {
    subject: WorkerObservationSubjectRef,
    #[serde(default)]
    query: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tool_part: Option<String>,
    #[serde(default)]
    tool_name: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    through: Option<String>,
    #[serde(default)]
    offset: usize,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadSessionEntryParams {
    subject: WorkerObservationSubjectRef,
    entry_ref: String,
    #[serde(default = "default_read_mode")]
    mode: String,
}

fn default_read_mode() -> String {
    "compact".to_string()
}

struct ViewSessionOverviewTool {
    provider: Arc<dyn WorkerObservationProvider>,
}

#[async_trait]
impl Tool for ViewSessionOverviewTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ViewSessionOverviewParams = parse_input("ViewSessionOverview", input_json)?;
        let view = latest_view(&*self.provider, &params.subject).await?;
        let limit = bounded_limit(params.limit);
        let entries = view
            .overview()
            .iter()
            .skip(params.offset)
            .take(limit)
            .map(|entry| {
                serde_json::json!({
                    "entry_ref": entry.id,
                    "entry_range": entry.entry_range,
                    "kind": entry.kind.as_str(),
                    "label": entry.label,
                    "text": entry.text,
                    "intervening_entries": entry.intervening_entries,
                })
            })
            .collect::<Vec<_>>();
        let has_more = params.offset.saturating_add(entries.len()) < view.overview().len();
        json_output(
            format!(
                "Showing {} Worker session overview entrie(s).",
                entries.len()
            ),
            serde_json::json!({
                "subject": params.subject,
                "entries": entries,
                "next_offset": has_more.then_some(params.offset + entries.len()),
            }),
        )
    }
}

struct SearchSessionEntriesTool {
    provider: Arc<dyn WorkerObservationProvider>,
}

#[async_trait]
impl Tool for SearchSessionEntriesTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SearchSessionEntriesParams = parse_input("SearchSessionEntries", input_json)?;
        let view = latest_view(&*self.provider, &params.subject).await?;
        let from = params.from.as_deref().map(parse_entry_ref).transpose()?;
        let through = params.through.as_deref().map(parse_entry_ref).transpose()?;
        if let (Some(from), Some(through)) = (&from, &through) {
            if from.source_index() > through.source_index() {
                return Err(ToolError::InvalidArgument(
                    "SearchSessionEntries from must not be after through".to_string(),
                ));
            }
        }
        let entries = view
            .search(&SearchOptions {
                query: params.query,
                kind: params.kind.as_deref().map(parse_kind).transpose()?,
                tool_part: params
                    .tool_part
                    .as_deref()
                    .map(parse_tool_part)
                    .transpose()?,
                tool_name: params.tool_name,
                limit: Some(bounded_limit(params.limit)),
                min_entry_index: None,
                from,
                through,
                offset: params.offset,
            })
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "entry_ref": entry.id,
                    "entry_range": entry.entry_range,
                    "kind": entry.kind.as_str(),
                    "tool_part": entry.tool_part.map(|part| format!("{part:?}").to_lowercase()),
                    "tool_name": entry.tool_name,
                    "label": entry.label,
                    "text": entry.summary,
                })
            })
            .collect::<Vec<_>>();
        json_output(
            format!("Found {} Worker session entrie(s).", entries.len()),
            serde_json::json!({ "subject": params.subject, "entries": entries }),
        )
    }
}

struct ReadSessionEntryTool {
    provider: Arc<dyn WorkerObservationProvider>,
}

#[async_trait]
impl Tool for ReadSessionEntryTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadSessionEntryParams = parse_input("ReadSessionEntry", input_json)?;
        let entry_ref = parse_entry_ref(&params.entry_ref)?;
        let detail = match params.mode.as_str() {
            "compact" => ReadDetail::Compact,
            "full" => ReadDetail::Full,
            other => {
                return Err(ToolError::InvalidArgument(format!(
                    "invalid mode {other:?}; expected compact or full"
                )));
            }
        };
        let view = latest_view(&*self.provider, &params.subject).await?;
        let read = view.read(
            ReadSelector::Id(entry_ref.as_str()),
            ReadOptions {
                include_tools: true,
                tool_part: ToolPart::Both,
                detail,
                max_items: 1,
                max_bytes: MAX_READ_BYTES,
            },
        );
        let entries = read
            .entries
            .into_iter()
            .map(|entry| {
                serde_json::json!({
                    "entry_ref": entry.id,
                    "entry_range": entry.entry_range,
                    "kind": entry.kind.as_str(),
                    "tool_part": entry.tool_part.map(|part| format!("{part:?}").to_lowercase()),
                    "tool_name": entry.tool_name,
                    "label": entry.label,
                    "text": entry.text,
                })
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return Err(ToolError::ExecutionFailed(
                "worker session was not found or is not accessible".to_string(),
            ));
        }
        json_output(
            format!("Read {} Worker session entry.", entries.len()),
            serde_json::json!({
                "subject": params.subject,
                "entries": entries,
                "truncated": read.truncated,
            }),
        )
    }
}

async fn latest_view(
    provider: &dyn WorkerObservationProvider,
    subject: &WorkerObservationSubjectRef,
) -> Result<SessionCapture, ToolError> {
    let capture = provider
        .capture_worker_session(subject)
        .await
        .map_err(tool_error)?;
    Ok(SessionCapture::new(capture.segment_id, capture.items))
}

fn parse_input<T: serde::de::DeserializeOwned>(
    tool_name: &str,
    input_json: &str,
) -> Result<T, ToolError> {
    serde_json::from_str(input_json)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool_name} input: {error}")))
}

fn parse_entry_ref(value: &str) -> Result<SessionEntryRef, ToolError> {
    SessionEntryRef::parse(value)
        .ok_or_else(|| ToolError::InvalidArgument(format!("invalid SessionEntryRef {value:?}")))
}

fn parse_kind(value: &str) -> Result<ReferenceKind, ToolError> {
    ReferenceKind::parse(value)
        .ok_or_else(|| ToolError::InvalidArgument(format!("invalid entry kind {value:?}")))
}

fn parse_tool_part(value: &str) -> Result<ToolPart, ToolError> {
    ToolPart::parse(value)
        .ok_or_else(|| ToolError::InvalidArgument(format!("invalid tool_part {value:?}")))
}

fn bounded_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT)
}

fn tool_error(error: WorkerObservationError) -> ToolError {
    match error {
        WorkerObservationError::NotFound => ToolError::ExecutionFailed(
            "worker session was not found or is not accessible".to_string(),
        ),
        WorkerObservationError::Unavailable(message) => ToolError::ExecutionFailed(message),
    }
}

fn json_output(summary: String, value: serde_json::Value) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string_pretty(&value)
        .map_err(|error| ToolError::ExecutionFailed(format!("serialize tool output: {error}")))?;
    Ok(ToolOutput {
        summary,
        content: Some(content),
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use llm_engine::Role;

    use crate::feature::{FeatureRegistryBuilder, HookRegistryBuilder};

    use super::*;

    struct FakeProvider {
        captures: Mutex<Vec<Item>>,
    }

    fn granted_subject() -> WorkerObservationSubjectRef {
        WorkerObservationSubjectRef::RuntimeWorker {
            runtime_id: "runtime-1".to_string(),
            worker_id: "granted".to_string(),
        }
    }

    #[async_trait]
    impl WorkerObservationProvider for FakeProvider {
        async fn list_worker_sessions(
            &self,
        ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
            Ok(vec![WorkerObservationSubject {
                subject: granted_subject(),
                display_name: "Granted".to_string(),
                relation: "peer".to_string(),
                status: "idle".to_string(),
            }])
        }

        async fn capture_worker_session(
            &self,
            subject: &WorkerObservationSubjectRef,
        ) -> Result<WorkerSessionCapture, WorkerObservationError> {
            if subject != &granted_subject() {
                return Err(WorkerObservationError::NotFound);
            }
            Ok(WorkerSessionCapture {
                segment_id: "segment".to_string(),
                items: self.captures.lock().unwrap().clone(),
            })
        }
    }

    fn message(_id: &str, role: Role, content: &str) -> Item {
        match role {
            Role::User => Item::user_message(content),
            Role::Assistant => Item::assistant_message(content),
            Role::System => Item::system_message(content),
        }
    }

    #[test]
    fn prompt_source_names_the_worker_observation_contract() {
        let catalog = crate::PromptCatalog::builtins_only().unwrap();
        let source = &catalog.projection().templates["common.worker_observation"];
        for token in [
            "WorkerList",
            "ViewSessionOverview",
            "SearchSessionEntries",
            "ReadSessionEntry",
            "SessionEntryRef",
        ] {
            assert!(source.contains(token), "missing {token}");
        }
    }

    #[test]
    fn worker_observation_requires_worker_control_service() {
        let provider = Arc::new(FakeProvider {
            captures: Mutex::new(Vec::new()),
        });
        let mut pending_tools = Vec::new();
        let mut hook_builder = HookRegistryBuilder::default();
        let report = FeatureRegistryBuilder::new()
            .with_module(WorkerObservationFeature::new(provider))
            .install_into_pending(&mut pending_tools, &mut hook_builder);
        assert!(!report.reports[0].installed);
        assert!(report.installed_tool_names().is_empty());
        let descriptor = WorkerObservationFeature::new(Arc::new(FakeProvider {
            captures: Mutex::new(Vec::new()),
        }))
        .descriptor();
        assert_eq!(
            descriptor.requires_services[0].id,
            ServiceId::builtin(WORKER_CONTROL_SERVICE_ID)
        );
    }

    #[tokio::test]
    async fn provider_grants_hide_unauthorized_subjects_and_latest_capture_preserves_refs() {
        let provider = Arc::new(FakeProvider {
            captures: Mutex::new(vec![message("u1", Role::User, "first")]),
        });
        let read = read_definition(provider.clone())().1;
        let hidden = read
            .execute(
                r#"{"subject":{"kind":"runtime_worker","runtime_id":"runtime-1","worker_id":"unauthorized"},"entry_ref":"E00000000"}"#,
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(format!("{hidden:?}").contains("not found or is not accessible"));

        provider
            .captures
            .lock()
            .unwrap()
            .push(message("a1", Role::Assistant, "second"));
        let output = read
            .execute(
                r#"{"subject":{"kind":"runtime_worker","runtime_id":"runtime-1","worker_id":"granted"},"entry_ref":"E00000000"}"#,
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(output.content.unwrap().contains("first"));

        let output = read
            .execute(
                r#"{"subject":{"kind":"runtime_worker","runtime_id":"runtime-1","worker_id":"granted"},"entry_ref":"E00000001"}"#,
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert!(output.content.unwrap().contains("second"));
    }
}
