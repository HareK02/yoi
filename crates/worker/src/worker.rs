#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agen::llm_client::RequestConfig;
use agen::llm_client::client::LlmClient;
use agen::llm_client::types::Role;
use agen::state::Mutable;
use agen::{
    Engine, EngineError, EngineResult, EngineRunExit, History, HistoryEntry, Item, StopReason,
    ToolExecutionPolicy, ToolOutputLimits, UsageRecord,
};
use arc_swap::ArcSwap;
use session_store::{
    LogEntry, PromptRenderProvenance, SegmentId, SessionExtension, SessionId, Store, StoreError,
    SystemItem, segment_log,
};
use session_store::{
    WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore, WorkerReclaimedChild,
    WorkerSpawnedChild, WorkerSpawnedScopeRule, WorkerStoreError,
};
use tracing::{info, warn};

use crate::segment_log_sink::SegmentLogSink;
use crate::session_history::{
    SessionHistoryDerivation, SessionHistoryMetadata, WorkerHistoryProvenance, history_entry,
    metadata as new_history_metadata, restore_history_entries, to_logged_history_entry,
    worker_subject,
};

use manifest::{
    DelegationScope, Permission, ResolveError, Scope, ScopeConfig, ScopeError, ScopeRule,
    SharedScope, WorkerManifest, WorkerManifestConfig,
};

use crate::compact::state::CompactState;
use crate::compact::usage_tracker::UsageTracker;
use crate::feature::builtin::memory::WorkspaceMemoryBackendError;
use crate::feature::builtin::{
    MemoryExtractFeature, MemoryExtractState, SessionExploreFeature, SessionExploreState,
    TaskFeature, WorkerObservationProvider, render_extract_input,
};
use crate::feature::{
    FeatureInstructionDeclaration, FeatureInstructionId, FeatureRegistryBuilder,
    FeatureRegistryInstallReport, dedupe_instruction_contributions,
};
use crate::hook::{
    Hook, HookRegistryBuilder, OnAbort, OnPromptSubmit, OnTurnEnd, PostToolCall, PreLlmRequest,
    PreToolCall,
};
use crate::in_flight::InFlightEvents;
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerIdentity, InternalWorkerSpec, InternalWorkerVisibility,
    prepare_internal_worker_from_spec, run_internal_worker, run_internal_worker_with_cancel_sender,
};

const COMPACTION_EXTENSION_DOMAIN: &str = "yoi.compaction";
const WORKER_ORCHESTRATION_INSTRUCTION_ID: &str = "worker.orchestration";
const WORKER_ORCHESTRATION_PROMPT_REF: &str = "common.worker_orchestration";

fn worker_orchestration_instruction() -> FeatureInstructionDeclaration {
    FeatureInstructionDeclaration::new(
        FeatureInstructionId::builtin(WORKER_ORCHESTRATION_INSTRUCTION_ID),
        WORKER_ORCHESTRATION_PROMPT_REF,
        "Worker orchestration guidance",
    )
    .expect("static Worker orchestration instruction declaration is valid")
}
use crate::ipc::alerter::Alerter;
use crate::ipc::interceptor::WorkerInterceptor;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::prompt::agents_md::read_agents_md;
use crate::prompt::catalog::{CatalogError, PromptCatalog, WorkspacePromptProjection};
use crate::prompt::source::PromptCatalogSource;
use crate::prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
use crate::runtime::dir;
use crate::runtime::worker_allocation::{self, ScopeAllocationGuard, ScopeLockError};
use crate::skill::{SkillActivationResponse, SkillClientError};
#[cfg(test)]
use async_trait::async_trait;
use protocol::{
    AlertLevel, AlertSource, CompactionLifecycle, CompactionLifecycleState, ErrorCode, Event,
    RewindSummary, RewindTarget, RewindTargetId, Segment,
};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use workdir::{
    LocalWorkdirSession, ReadOnlyWorkdirSession, WorkdirSessionCapabilities, WorkdirSessionHandle,
};

const RESTORE_RECONCILIATION_REACHABILITY_TIMEOUT: Duration = Duration::from_millis(500);

/// Explicit filesystem authority held by a Worker.
///
/// `None` means the Worker has no local filesystem authority: no cwd, no
/// filesystem view, and no filesystem/Bash tool surface. Workspace context may
/// still exist separately for memory, workflows, and project records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerFilesystemAuthority {
    None,
    Local(LocalWorkingDirectory),
}

impl WorkerFilesystemAuthority {
    pub fn local(root: PathBuf, cwd: PathBuf) -> Self {
        Self::Local(LocalWorkingDirectory { root, cwd })
    }

    pub fn as_local(&self) -> Option<&LocalWorkingDirectory> {
        match self {
            Self::None => None,
            Self::Local(local) => Some(local),
        }
    }
}

/// Local filesystem authority for a Worker.
///
/// `root` is the authority root retained for control-plane semantics;
/// `cwd` is the default working directory used by filesystem tools, Bash,
/// file references, and local worktree-scoped features.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalWorkingDirectory {
    pub root: PathBuf,
    pub cwd: PathBuf,
}

/// Path-free workspace identity carried by a Worker.
///
/// The value is intentionally opaque to Worker code: Runtime/host layers own
/// backend lookup, endpoint/auth/secret materialisation, and any mapping from a
/// local checkout path to an id. Worker code may only compare/log the id and pass
/// it through to narrow workspace-aware handles.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    pub fn new(id: impl Into<String>) -> Result<Self, WorkspaceIdError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(WorkspaceIdError::Empty);
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum WorkspaceIdError {
    #[error("workspace id must not be empty")]
    Empty,
}

/// One authority-bound operation sent through the Runtime-supplied Workspace client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRequest {
    pub method: WorkspaceRequestMethod,
    pub path: String,
    pub body: Option<String>,
}

impl WorkspaceRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: WorkspaceRequestMethod::Get,
            path: path.into(),
            body: None,
        }
    }

    pub fn json(
        method: WorkspaceRequestMethod,
        path: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            method,
            path: path.into(),
            body: Some(body.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceRequestMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceResponse {
    pub status: u16,
    pub body: String,
}

impl WorkspaceResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceClientError {
    #[error("workspace client is unavailable: {0}")]
    Unavailable(String),
    #[error("workspace request path must start with '/': {0}")]
    InvalidPath(String),
    #[error("workspace request failed: {0}")]
    Request(String),
}

#[derive(Clone)]
pub struct WorkspacePromptCatalogResolution {
    pub projection: Arc<WorkspacePromptProjection>,
    pub catalog: Arc<PromptCatalog>,
}

impl std::fmt::Debug for WorkspacePromptCatalogResolution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspacePromptCatalogResolution")
            .field("workspace_id", &self.projection.workspace_id)
            .field("config_revision", &self.projection.config_revision)
            .field("source_digest", &self.projection.source_digest)
            .field("projection_digest", &self.projection.projection_digest)
            .finish_non_exhaustive()
    }
}

impl WorkspacePromptCatalogResolution {
    pub fn new(projection: WorkspacePromptProjection) -> Result<Self, CatalogError> {
        projection.validate()?;
        let catalog = PromptCatalog::load(
            &PromptCatalogSource::builtins_only()
                .with_effective_catalog(projection.catalog.clone()),
        )?;
        Ok(Self {
            projection: Arc::new(projection),
            catalog,
        })
    }
}

/// Path-free Workspace operation authority injected by Runtime/host code.
///
/// Workers receive this trait object rather than a Backend URL. The concrete
/// implementation is responsible for binding Runtime/Worker identity and
/// forwarding operations to the Workspace authority.
pub trait WorkspaceClient: std::fmt::Debug + Send + Sync {
    fn workspace_id(&self) -> Option<&str>;
    fn kind(&self) -> &str;
    fn is_available(&self) -> bool;
    fn execute(&self, request: WorkspaceRequest)
    -> Result<WorkspaceResponse, WorkspaceClientError>;

    /// Resolve the Workspace's current immutable Prompt projection for future
    /// operation boundaries. Creation and restore continue to use persisted
    /// launch/session state; this hook never reconstructs historical prompts.
    fn current_prompt_projection(
        &self,
        _minimum_revision: Option<u64>,
    ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
        Ok(None)
    }

    /// Executes the destructive WorkerRemove operation through Runtime-owned source proof.
    /// Target identity is operation data; source identity and permission are never caller inputs.
    fn execute_worker_remove(
        &self,
        _target_runtime_id: &str,
        _target_worker_id: &str,
        _reason: &str,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(
            "Runtime-owned WorkerRemove forwarding is unavailable".to_string(),
        ))
    }

    /// Trusted review capability context is injected by the Internal SubWorker spawn layer.
    /// It is never accepted from a model-visible tool argument.
    fn reviewer_context(&self) -> Option<&ReviewerContext> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerContext {
    pub ticket_id: String,
}

#[derive(Debug)]
pub struct ReviewerChildWorkspaceClient {
    inner: Arc<dyn WorkspaceClient>,
    context: ReviewerContext,
    capability_token: String,
}

impl ReviewerChildWorkspaceClient {
    pub fn new(
        inner: Arc<dyn WorkspaceClient>,
        context: ReviewerContext,
        capability_token: String,
    ) -> Self {
        Self {
            inner,
            context,
            capability_token,
        }
    }
}

impl WorkspaceClient for ReviewerChildWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        self.inner.workspace_id()
    }
    fn kind(&self) -> &str {
        "runtime-reviewer-child"
    }
    fn is_available(&self) -> bool {
        self.inner.is_available()
    }
    fn reviewer_context(&self) -> Option<&ReviewerContext> {
        Some(&self.context)
    }

    fn current_prompt_projection(
        &self,
        minimum_revision: Option<u64>,
    ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
        self.inner.current_prompt_projection(minimum_revision)
    }

    fn execute(
        &self,
        mut request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let workspace_id = self.workspace_id().unwrap_or_default();
        let ticket_id = &self.context.ticket_id;
        let ticket_query_path = format!("/api/w/{workspace_id}/tickets/query");
        let ticket_show_path = format!("/api/w/{workspace_id}/tickets/{ticket_id}/show");
        let review_path =
            format!("/api/w/{workspace_id}/tickets/{ticket_id}/merge-request/reviews");
        let read_allowed = request.method == WorkspaceRequestMethod::Get
            || (request.method == WorkspaceRequestMethod::Post
                && (request.path == ticket_query_path || request.path == ticket_show_path));
        if request.method == WorkspaceRequestMethod::Post && request.path == review_path {
            let body = request.body.take().ok_or_else(|| {
                WorkspaceClientError::Request("review submission requires a JSON body".to_string())
            })?;
            let mut value: serde_json::Value = serde_json::from_str(&body)
                .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
            let object = value.as_object_mut().ok_or_else(|| {
                WorkspaceClientError::Request(
                    "review submission body must be an object".to_string(),
                )
            })?;
            object.insert(
                "capability_token".to_string(),
                serde_json::Value::String(self.capability_token.clone()),
            );
            request.body = Some(
                serde_json::to_string(&value)
                    .map_err(|error| WorkspaceClientError::Request(error.to_string()))?,
            );
        } else if !read_allowed {
            return Err(WorkspaceClientError::Unavailable(
                "Reviewer child Workspace authority is read-only except for its one attested Merge Request review submission".to_string(),
            ));
        }
        self.inner.execute(request)
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct TestWorkspaceHttpClient {
    workspace_id: String,
    base_url: String,
}

#[cfg(test)]
impl TestWorkspaceHttpClient {
    pub(crate) fn new(workspace_id: impl Into<String>, base_url: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
        }
    }
}

#[cfg(test)]
impl WorkspaceClient for TestWorkspaceHttpClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(&self.workspace_id)
    }

    fn kind(&self) -> &str {
        "test-http"
    }

    fn is_available(&self) -> bool {
        true
    }

    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let base_url = self.base_url.clone();
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::spawn(move || execute_test_workspace_http(&base_url, request))
                .join()
                .map_err(|_| {
                    WorkspaceClientError::Request(
                        "test workspace request thread panicked".to_string(),
                    )
                })?
        } else {
            execute_test_workspace_http(&base_url, request)
        }
    }
}

#[cfg(test)]
fn execute_test_workspace_http(
    base_url: &str,
    request: WorkspaceRequest,
) -> Result<WorkspaceResponse, WorkspaceClientError> {
    if !request.path.starts_with('/') || request.path.starts_with("//") {
        return Err(WorkspaceClientError::InvalidPath(request.path));
    }
    let method = match request.method {
        WorkspaceRequestMethod::Get => reqwest::Method::GET,
        WorkspaceRequestMethod::Post => reqwest::Method::POST,
        WorkspaceRequestMethod::Put => reqwest::Method::PUT,
        WorkspaceRequestMethod::Patch => reqwest::Method::PATCH,
        WorkspaceRequestMethod::Delete => reqwest::Method::DELETE,
    };
    let client = reqwest::blocking::Client::new();
    let mut builder = client.request(method, format!("{base_url}{}", request.path));
    if let Some(body) = request.body {
        builder = builder.body(body);
    }
    let response = builder
        .send()
        .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
    Ok(WorkspaceResponse { status, body })
}

#[derive(Debug)]
struct MarkerWorkspaceClient {
    workspace_id: Option<String>,
    kind: String,
    available: bool,
    reason: String,
}

impl WorkspaceClient for MarkerWorkspaceClient {
    fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    fn is_available(&self) -> bool {
        self.available
    }

    fn execute(
        &self,
        _request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(self.reason.clone()))
    }
}

#[cfg(test)]
mod reviewer_client_tests {
    use super::*;

    #[derive(Debug, Default)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("ws")
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
            self.requests.lock().expect("recording lock").push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: "{}".into(),
            })
        }
    }

    fn reviewer_client(inner: Arc<dyn WorkspaceClient>) -> ReviewerChildWorkspaceClient {
        ReviewerChildWorkspaceClient::new(
            inner,
            ReviewerContext {
                ticket_id: "T1".into(),
            },
            "secret".into(),
        )
    }

    #[test]
    fn reviewer_child_client_allows_typed_ticket_reads() {
        let inner = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner.clone());

        for request in [
            WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/query",
                "{}".to_string(),
            ),
            WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/T1/show",
                "{}".to_string(),
            ),
            WorkspaceRequest::get("/api/w/ws/merge-requests/MR1"),
        ] {
            client.execute(request).expect("read should be forwarded");
        }

        assert_eq!(inner.requests.lock().expect("recording lock").len(), 3);
    }

    #[test]
    fn reviewer_child_client_rejects_other_ticket_and_mutation_posts() {
        let inner: Arc<dyn WorkspaceClient> = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner);

        for request in [
            WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/T2/show",
                "{}".to_string(),
            ),
            WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/T1/comments",
                "{}".to_string(),
            ),
            WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/T1/merge-request",
                "{}".to_string(),
            ),
        ] {
            let error = client.execute(request).unwrap_err();
            assert!(error.to_string().contains("read-only"));
        }
    }

    #[test]
    fn reviewer_child_client_injects_capability_only_for_attested_review() {
        let inner = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner.clone());
        client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/tickets/T1/merge-request/reviews",
                r#"{"decision":"approve"}"#.to_string(),
            ))
            .expect("attested review should be forwarded");

        let requests = inner.requests.lock().expect("recording lock");
        let body: serde_json::Value =
            serde_json::from_str(requests[0].body.as_deref().expect("review body"))
                .expect("review JSON");
        assert_eq!(body["capability_token"], "secret");
    }
}

pub fn unavailable_workspace_client(
    workspace_id: Option<&WorkspaceId>,
    reason: impl Into<String>,
) -> Arc<dyn WorkspaceClient> {
    Arc::new(MarkerWorkspaceClient {
        workspace_id: workspace_id.map(|id| id.as_str().to_string()),
        kind: "unavailable".to_string(),
        available: false,
        reason: reason.into(),
    })
}

pub fn marker_workspace_client(
    workspace_id: Option<&WorkspaceId>,
    kind: impl Into<String>,
) -> Arc<dyn WorkspaceClient> {
    let kind = kind.into();
    Arc::new(MarkerWorkspaceClient {
        workspace_id: workspace_id.map(|id| id.as_str().to_string()),
        reason: format!("workspace client kind `{kind}` does not expose Workspace operations"),
        kind,
        available: true,
    })
}

/// Workspace context supplied to a Worker separately from filesystem authority.
#[derive(Clone)]
pub struct WorkerWorkspaceContext {
    workspace_id: Option<WorkspaceId>,
    client: Arc<dyn WorkspaceClient>,
}

impl std::fmt::Debug for WorkerWorkspaceContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerWorkspaceContext")
            .field("workspace_id", &self.workspace_id)
            .field("client_kind", &self.client.kind())
            .field("client_available", &self.client.is_available())
            .finish()
    }
}

impl WorkerWorkspaceContext {
    pub fn no_workspace() -> Self {
        Self {
            workspace_id: None,
            client: unavailable_workspace_client(None, "no workspace configured"),
        }
    }

    pub fn unavailable(workspace_id: Option<WorkspaceId>, reason: impl Into<String>) -> Self {
        let client = unavailable_workspace_client(workspace_id.as_ref(), reason);
        Self {
            workspace_id,
            client,
        }
    }

    pub fn with_client(
        workspace_id: Option<WorkspaceId>,
        client: Arc<dyn WorkspaceClient>,
    ) -> Self {
        Self {
            workspace_id,
            client,
        }
    }

    pub fn local_filesystem(workspace_id: Option<WorkspaceId>) -> Self {
        let client = marker_workspace_client(workspace_id.as_ref(), "local-filesystem");
        Self {
            workspace_id,
            client,
        }
    }

    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        self.workspace_id.as_ref()
    }

    pub fn client(&self) -> &dyn WorkspaceClient {
        self.client.as_ref()
    }

    pub fn client_handle(&self) -> Arc<dyn WorkspaceClient> {
        self.client.clone()
    }
}

/// `(SessionId, SegmentId)` pair the Worker is currently writing to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentLocation {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
}

type WorkerMetadataWriter =
    Arc<dyn Fn(WorkerMetadata) -> Result<(), WorkerStoreError> + Send + Sync>;

fn worker_metadata_writer_for_store<St>(store: &St) -> WorkerMetadataWriter
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    let store = store.clone();
    Arc::new(move |metadata| {
        store
            .set_active_with_workspace_context(
                &metadata.worker_name,
                metadata.active,
                metadata.resolved_manifest_snapshot,
                metadata.workspace_id,
                metadata.workspace_root,
            )
            .map(|_| ())
    })
}

/// Lock-free shared session/segment pointer.
///
/// Holds the current `(SessionId, SegmentId)` pair and the append tally
/// so that the Worker and every `LogWriterHandle` clone see a consistent
/// view through `Arc`-shared lock-free reads. The location is wrapped in
/// `ArcSwap` so fork (a rare, run-start-only event) can atomically swap
/// session_id + segment_id together without taking a mutex on the
/// append hot path. `entries_written` is an `AtomicUsize` bumped on
/// every successful append; the writer's tally is compared against the
/// store's on-disk count to detect concurrent writers in
/// `ensure_segment_head`.
pub struct SegmentState {
    location: ArcSwap<SegmentLocation>,
    entries_written: AtomicUsize,
}

impl SegmentState {
    pub fn new(session_id: SessionId, segment_id: SegmentId, entries_written: usize) -> Arc<Self> {
        Arc::new(Self {
            location: ArcSwap::from_pointee(SegmentLocation {
                session_id,
                segment_id,
            }),
            entries_written: AtomicUsize::new(entries_written),
        })
    }

    pub fn location(&self) -> SegmentLocation {
        **self.location.load()
    }

    pub fn session_id(&self) -> SessionId {
        self.location().session_id
    }

    pub fn segment_id(&self) -> SegmentId {
        self.location().segment_id
    }

    pub fn set_location(&self, loc: SegmentLocation) {
        self.location.store(Arc::new(loc));
    }

    pub fn entries_written(&self) -> usize {
        self.entries_written.load(Ordering::Acquire)
    }

    pub fn set_entries_written(&self, n: usize) {
        self.entries_written.store(n, Ordering::Release);
    }

    fn increment_entries(&self) {
        self.entries_written.fetch_add(1, Ordering::Release);
    }
}

struct EmptyTurnRollbackSnapshot {
    history_len: usize,
    user_segments_len: usize,
    entries_written: usize,
    sink_len: usize,
    pending_attachments: Vec<SystemItem>,
    usage_history_len: usize,
    ai_activity_count: usize,
    last_run_interrupted: bool,
    active_run_turn_count: Option<usize>,
    flow_runtime_state: Option<flow::FlowRuntimeState>,
}

fn active_run_checkpoint_entry(
    active_run_turn_count: Option<usize>,
    total_turn_count: usize,
) -> Option<LogEntry> {
    active_run_turn_count.map(|active_turn_count| LogEntry::ActiveRunCheckpoint {
        ts: segment_log::now_millis(),
        active_turn_count,
        total_turn_count,
    })
}

fn is_ai_materialized_item(item: &Item) -> bool {
    match item {
        Item::Message { role, .. } => *role == Role::Assistant,
        Item::ToolCall { .. } | Item::ToolResult { .. } | Item::Reasoning { .. } => true,
    }
}

fn history_annotator<St>(
    annotation_writer: LogWriterHandle<St>,
    pending_input: Vec<HistoryEntry<SessionHistoryMetadata>>,
    pending_committed_history: Arc<
        Mutex<std::collections::VecDeque<HistoryEntry<SessionHistoryMetadata>>>,
    >,
) -> impl FnMut(&Item) -> Result<SessionHistoryMetadata, String>
where
    St: Store + Clone,
{
    let mut pending_input = std::collections::VecDeque::from(pending_input);
    move |item: &Item| {
        if let Some(entry) = pending_input.pop_front() {
            return Ok(entry.annotation);
        }
        if let Some(entry) = {
            let mut pending = pending_committed_history
                .lock()
                .expect("pending committed history poisoned");
            pending
                .front()
                .filter(|entry| entry.item == *item)
                .cloned()
                .map(|entry| {
                    pending.pop_front();
                    entry
                })
        } {
            return Ok(entry.annotation);
        }

        let subject = worker_subject(annotation_writer.state.location().session_id);
        let origin = if item.is_tool_result() {
            WorkerHistoryProvenance::ToolOutput { worker: subject }
        } else if item.is_assistant_message() || item.is_tool_call() || item.is_reasoning() {
            WorkerHistoryProvenance::ModelOutput { worker: subject }
        } else {
            // Unknown user/system append paths fail closed. Trusted system
            // producers must precommit through `SystemItemCommitter`.
            WorkerHistoryProvenance::LegacyUnknown
        };
        let metadata = new_history_metadata(origin, None);
        let entry = session_store::LoggedHistoryEntry {
            item: item.clone().into(),
            metadata: metadata.clone(),
        };
        let log_entry = if item.is_tool_result() {
            LogEntry::AnnotatedToolResult {
                ts: segment_log::now_millis(),
                entry,
            }
        } else {
            LogEntry::AnnotatedAssistantItem {
                ts: segment_log::now_millis(),
                entry,
            }
        };
        annotation_writer
            .append_entry(log_entry)
            .map_err(|error| error.to_string())?;
        Ok(metadata)
    }
}

/// Cheap-cloneable bundle of (store + shared session pointer + sink)
/// handed to the worker callback and the interceptor so they can
/// commit `LogEntry` values directly without going through an mpsc
/// ferry. All fields are `Clone` (`store` per its `Clone` impl,
/// `state` and `sink` as `Arc` clones).
#[derive(Clone)]
pub struct LogWriterHandle<St: Clone> {
    pub store: St,
    pub state: Arc<SegmentState>,
    pub sink: SegmentLogSink,
    pub in_flight: Option<InFlightEvents>,
}

impl<St> LogWriterHandle<St>
where
    St: Store + Clone,
{
    /// Append `entry` to the log: disk write → counter bump → in-memory
    /// mirror push → broadcast. The Store owns physical write ordering and
    /// partial-write recovery; publication happens only after it returns Ok.
    pub fn append_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        let loc = self.state.location();
        self.store.append(loc.session_id, loc.segment_id, &entry)?;
        self.state.increment_entries();
        if let Some(in_flight) = &self.in_flight {
            let committed_item = match &entry {
                LogEntry::AssistantItem { item, .. } => Some(item.clone()),
                LogEntry::AnnotatedAssistantItem { entry, .. } => Some(entry.item.clone()),
                _ => None,
            };
            if let Some(item_for_clear) = committed_item {
                in_flight.clear_for_committed_item_then(&item_for_clear, || {
                    self.sink.publish(entry);
                });
                return Ok(());
            }
        }
        self.sink.publish(entry);
        Ok(())
    }

    /// Append a debug trace record alongside the current segment log. Trace
    /// writes deliberately do not affect the segment entry counter or live
    /// replay sink because they are not conversation history.
    pub fn append_trace(&self, entry: &session_store::TraceEntry) -> Result<(), StoreError> {
        let loc = self.state.location();
        self.store
            .append_trace(loc.session_id, loc.segment_id, entry)
    }
}

/// Type-erased commit handle for the interceptor. Lets the
/// interceptor commit `SystemItem`s without being generic over the
/// concrete `Store` type.
pub trait SystemItemCommitter: Send + Sync {
    fn commit_log_entry(&self, entry: LogEntry) -> Result<(), StoreError>;

    fn commit_system_item(
        &self,
        item: SystemItem,
    ) -> Result<HistoryEntry<SessionHistoryMetadata>, StoreError> {
        let metadata = new_history_metadata(
            WorkerHistoryProvenance::BackendInstruction { operation_id: None },
            None,
        );
        let history_item = item.to_history_item();
        self.commit_log_entry(LogEntry::AnnotatedSystemItem {
            ts: segment_log::now_millis(),
            entry: session_store::LoggedSystemHistoryEntry {
                item,
                metadata: metadata.clone(),
            },
        })?;
        Ok(HistoryEntry::new(history_item, metadata))
    }
}

impl<St> SystemItemCommitter for LogWriterHandle<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    fn commit_log_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        self.append_entry(entry)
    }
}

struct SessionFlowRuntimeStateCommitter<St: Store + Clone> {
    writer: LogWriterHandle<St>,
}

impl<St> crate::feature::builtin::flow_transition::FlowRuntimeStateCommitter
    for SessionFlowRuntimeStateCommitter<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    fn commit(&self, state: &flow::FlowRuntimeState) -> Result<(), String> {
        let payload = serde_json::to_value(state)
            .map_err(|error| format!("serialize Flow runtime state: {error}"))?;
        self.writer
            .append_entry(LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: FLOW_RUNTIME_EXTENSION_DOMAIN.to_string(),
                payload,
            })
            .map_err(|error| error.to_string())
    }
}

pub const WORKER_INPUT_SUBMISSION_EXTENSION_DOMAIN: &str = "worker.input-submission.v1";

#[derive(Clone)]
struct PreparedFlowProjection {
    selector: String,
    instructions: String,
    definition_id: String,
    definition_revision: u64,
    instance_id: String,
    state_id: String,
}

/// Sole live owner of committed model-visible Worker history.
///
/// `Engine` borrows this history only while executing a run. The revision is
/// advanced together with every live rewrite so projections can fence stale
/// observations without maintaining a second transcript.
#[derive(Clone)]
pub struct WorkerSession {
    session_id: SessionId,
    revision: u64,
    history: History<SessionHistoryMetadata>,
}

impl WorkerSession {
    fn new(session_id: SessionId, entries: Vec<HistoryEntry<SessionHistoryMetadata>>) -> Self {
        let revision = u64::try_from(entries.len()).unwrap_or(u64::MAX);
        Self {
            session_id,
            revision,
            history: History::from_entries(entries),
        }
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn history(&self) -> &History<SessionHistoryMetadata> {
        &self.history
    }

    fn history_mut(&mut self) -> &mut History<SessionHistoryMetadata> {
        &mut self.history
    }

    fn note_mutation(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }

    fn replace_history(&mut self, entries: Vec<HistoryEntry<SessionHistoryMetadata>>) {
        self.history.replace_entries(entries);
        self.note_mutation();
    }
}

/// An independent agent execution unit.
///
/// Holds a [`Engine`] directly and persists session state via
/// `session-store` functions after each turn.
pub struct Worker<C: LlmClient, St: Store> {
    manifest: WorkerManifest,
    /// Always `Some` outside of `run()`/`resume()`.
    engine: Option<Engine<C, Mutable, SessionHistoryMetadata>>,
    /// Sole live authority for committed model-visible history.
    session: WorkerSession,
    /// Worker-owned interruption recovery marker.
    last_run_interrupted: bool,
    store: St,
    /// Optional write-through hook for name-keyed Worker metadata. Production
    /// constructors install this from the same FsStore that owns the session
    /// logs; low-level `Worker::new` tests leave it absent.
    worker_metadata_writer: Option<WorkerMetadataWriter>,
    /// Shared session pointer. Source of truth for the Worker's current
    /// `segment_id` and append tally. `self.segment_id()` is a thin
    /// wrapper over `segment_state.segment_id()`.
    segment_state: Arc<SegmentState>,
    /// Explicit local filesystem authority, or `None` for Workers with no
    /// local cwd and no filesystem/Bash tool surface.
    filesystem_authority: WorkerFilesystemAuthority,
    /// Live WorkdirSession provider derived once from the Worker–Workdir binding.
    /// Local tools, file views, and compaction workers clone this handle.
    workdir_session: Option<WorkdirSessionHandle>,
    /// Path-free workspace identity/client context injected by Runtime/host.
    /// This never grants local filesystem authority.
    workspace_context: WorkerWorkspaceContext,
    /// Runtime-owned durable Flow state reconstructed from Worker session
    /// extensions on restore.
    flow_runtime_state: Arc<Mutex<Option<flow::FlowRuntimeState>>>,
    flow_feature_enabled: bool,
    /// Shared, atomically-swappable view of the Worker's resolved scope.
    /// Cloned into local WorkdirSession providers used by builtin tools, fs_view,
    /// and compaction so updates propagate at the next permission check.
    scope: SharedScope,
    /// Filesystem authority this Worker may pass to spawned children. Direct tools
    /// continue to use `scope`; SubWorkerSpawn validates requested child scope here.
    delegation_scope: DelegationScope,
    hook_builder: HookRegistryBuilder,
    interceptor_installed: bool,
    /// Shared compaction state (present when threshold is configured).
    compact_state: Option<Arc<CompactState>>,
    /// Per-LLM-request Usage tracker. Always present after construction.
    /// Captures `(history_len, UsageEvent)` pairs during a run; drained
    /// in `persist_turn` and persisted as `LogEntry::LlmUsage` entries.
    usage_tracker: Arc<UsageTracker>,
    /// Sync-side buffer for `Metric` values queued from inside Engine
    /// callbacks (currently the prune observer). Drained in `persist_turn`
    /// and written via `session_metrics::record_metric` alongside
    /// `LogEntry::LlmUsage`. Always present after construction.
    metrics_tracker: Arc<crate::compact::metrics_tracker::MetricsTracker>,
    /// Cumulative Usage measurement timeline, one entry per LLM call.
    /// Restored from session log on `restore`, appended on each persist.
    /// Read by token-accounting APIs (`Worker::total_tokens`, etc.).
    ///
    /// Wrapped in `Arc<Mutex>` so that callbacks injected into the
    /// Engine (e.g. the savings estimator used by the prune projection)
    /// can share the same view via [`Worker::usage_history_handle`].
    usage_history: Arc<Mutex<Vec<UsageRecord>>>,
    /// Worker-lifetime file-operation tracker from the builtin `tools`
    /// crate. Populated by the Controller when it registers the builtin
    /// tools so that Worker-owned operations (e.g. compaction) can consult
    /// the recency of touched files.
    tracker: Option<tools::Tracker>,
    /// Built-in Task feature state shared by Task tools, reminder hooks, and
    /// the narrow snapshot/restore surface Worker needs for compaction and rewind.
    /// Store/reminder ownership stays inside the Task feature module.
    task_feature: TaskFeature,
    /// Host-owned projection of Worker sessions explicitly granted to this Worker.
    /// The provider reauthorizes every capture and never derives authority from model input.
    worker_observation_provider: Option<Arc<dyn WorkerObservationProvider>>,
    /// Parsed system-prompt template awaiting first-turn materialisation.
    /// `Some` until `ensure_system_prompt_materialized` renders it once,
    /// then `None` forever — including after compaction.
    system_prompt_template: Option<SystemPromptTemplate>,
    /// Mandatory prompt sections contributed by enabled Worker features.
    /// These are appended by Rust-owned prompt assembly so authored top-level
    /// templates cannot accidentally omit feature workflow guidance.
    feature_instructions: Vec<FeatureInstructionDeclaration>,
    /// User-facing notification sink attached by the Controller at
    /// spawn time. `None` in tests / direct `Worker::new` usage.
    alerter: Option<Alerter>,
    /// Broadcast sender for typed lifecycle `Event`s (compact progress,
    /// etc.). Attached by the Controller alongside `alerter`. Unlike
    /// notifications, events sent here are NOT replayed to clients that
    /// connect after the fact — they are fire-and-forget broadcasts.
    event_tx: Option<broadcast::Sender<Event>>,
    /// Parent-owned projection/control boundary for observable Internal service Workers.
    /// Service Workers are never exposed through the model-facing SubWorker control surface.
    internal_worker_registry: Option<Arc<crate::spawn::registry::SpawnedWorkerRegistry>>,
    in_flight: Option<InFlightEvents>,
    /// Monotonic counter incremented by worker event bridges when an
    /// assistant-side execution artifact becomes visible to clients before
    /// it is necessarily committed to history (e.g. streaming text deltas).
    /// `Worker::run` uses it to avoid rolling back a turn after the UI has
    /// already observed AI output.
    ai_activity_counter: Arc<AtomicUsize>,
    /// Queue of pending `Method::Notify` notifications awaiting
    /// injection into the next LLM request. Shared with the
    /// WorkerInterceptor installed in `ensure_interceptor_installed`.
    pending_notifies: NotifyBuffer,
    /// Submit-scoped stash for resolver-produced system messages
    /// (currently `@<path>` file content). `Worker::run` fills this
    /// before handing off to the worker; `WorkerInterceptor::on_prompt_submit`
    /// drains it and returns `ContinueWith` so the items land in
    /// history right after the user message that referenced them.
    pending_attachments: Arc<Mutex<Vec<SystemItem>>>,
    /// Ephemeral handoff for system items that were durably committed by the
    /// interceptor before Agen applies them to live typed history.
    pending_committed_history:
        Arc<Mutex<std::collections::VecDeque<HistoryEntry<SessionHistoryMetadata>>>>,
    /// Scope allocation in the machine-wide lock file. `Some` for
    /// Workers built via `from_manifest` / `from_manifest_spawned` /
    /// `restore_from_manifest` (production paths); `None` for the
    /// low-level `Worker::new` constructor used in tests, which bypasses
    /// the registry. Kept purely for its `Drop` impl, which releases
    /// the allocation when the Worker is dropped.
    #[allow(dead_code)]
    scope_allocation: Option<ScopeAllocationGuard>,
    /// Socket path of the spawning Worker. `Some` only for Workers built via
    /// `from_manifest_spawned`. Consumed by the controller to fire
    /// `Method::WorkerEvent` reports upward (turn end, error, shutdown,
    /// scope sub-delegation).
    callback_socket: Option<PathBuf>,
    /// Transient launch role for Ticket role sessions. This is process-local
    /// runtime identity used by controller policy; it is not model-visible and
    /// is not persisted into Ticket claim/session records.
    runtime_ticket_role: Option<String>,
    /// Central catalog of Worker-level prompt strings (compaction system
    /// prompt, notification wrapper, interrupt notes, trailing system
    /// sections, ...). Built from the 4-layer overlay in
    /// [`Self::from_manifest`], or defaults to the builtin pack when a
    /// Worker is constructed through lower-level paths that have no loader.
    prompts: Arc<ArcSwap<PromptCatalog>>,
    /// When true (default), the system-prompt assembler may append resident
    /// context from the workspace Memory document. Internal disposable
    /// workers disable this so resident memory exposure is opt-in per Worker.
    inject_resident_summary: bool,
    /// When true (default), the system-prompt assembler may append resident
    /// resident context. This is intentionally independent from
    /// summary residency: each section has its own gate.
    /// extract (memory.extract) reentry guard. `true` while an extract
    /// worker is running; subsequent triggers are skipped per spec
    /// (`docs/plan/memory.md` §Extract 並走防止). `Arc<AtomicBool>` so
    /// the flag survives across `try_post_run_extract` calls without a
    /// `&mut self` race.
    extract_in_flight: Arc<AtomicBool>,
    /// consolidation (memory.consolidation) in-process reentry guard.
    consolidation_in_flight: Arc<AtomicBool>,
    /// Last completed extract boundary. `None` means no extract has
    /// run yet on this session — next extract starts from entry 0.
    /// Restored from `RestoredState.extensions` on `restore`, updated
    /// after each successful extract via `save_extension`.
    extract_pointer: Arc<Mutex<Option<memory::ExtractPointerPayload>>>,
    /// extract/consolidation memory job running outside the controller method loop.
    /// The task owns the extract/consolidate worker execution and is joined
    /// at shutdown. A single slot is enough: extract/consolidation implementations loop
    /// until thresholds fall below their trigger points, and concurrent
    /// triggers are coalesced by skipping when this handle is still active.
    memory_task: Option<JoinHandle<()>>,
    /// Typed user submissions in submit order. K-th entry corresponds to
    /// the K-th `Item::user_message` in `worker.history()` (modulo seed
    /// history loaded via `SegmentStart.history`, whose original segments
    /// are not preserved). Populated from log on `restore_from_manifest`,
    /// appended after `save_user_input` on each `run`. Pre-`Event::Snapshot`
    /// this fed `WorkerSharedState.user_segments`; the new wire format
    /// carries typed atoms via `LogEntry::UserInput { segments }` so
    /// this remains purely an in-memory tracker for compact alignment.
    user_segments: Vec<Vec<Segment>>,
    /// Worker-side session-log mirror + broadcast sink. Populated alongside
    /// every successful `session_store::append_entry` write so connected
    /// clients see a `(snapshot, live)` stream consistent with what's
    /// on disk.
    sink: SegmentLogSink,
    /// `true` once `wire_history_persistence` has installed the
    /// `Engine::on_history_append` callback that commits each appended
    /// item as a singular `LogEntry::AssistantItem` / `ToolResult`
    /// directly through the writer. Tests that drive `Worker::new` without
    /// going through the controller leave this `false`; `persist_turn`
    /// then walks the post-`history_before` slice inline so entries
    /// still land on disk.
    history_persistence_wired: bool,
    /// Type-erased commit handle wired by the controller (or by tests
    /// via `attach_log_writer`). The interceptor uses it to commit
    /// `SystemItem`s directly without being generic over `St`. `None`
    /// in low-level test paths that bypass the controller — those
    /// paths skip SystemItem disk commits but still see the rendered
    /// `Item::system_message` in worker history.
    log_writer: Option<Arc<dyn SystemItemCommitter>>,
}

impl<C: LlmClient + 'static, St: Store + 'static> Worker<C, St> {
    pub async fn wait_for_memory_jobs(&mut self) {
        if let Some(handle) = self.memory_task.take()
            && let Err(e) = handle.await
        {
            tracing::warn!(error = %e, "Post-run memory task join failed");
        }
    }
}

impl<C: LlmClient + Clone + 'static, St: Store + Clone + 'static> Worker<C, St> {
    fn clone_for_memory_task(&self) -> Self {
        // The cloned Worker's worker exists only as a snapshot for the memory
        // task: `run_extract_once` reads `worker.history()`, and the
        // extract/consolidate workers are built fresh inside their own
        // methods using `worker.client()` as fallback when no override
        // model is configured. system_prompt / request_config / cache_key
        // are unused on this path, so we deliberately skip copying them.
        let source_worker = self.engine.as_ref().expect("worker present");
        let worker = Engine::<C, Mutable, SessionHistoryMetadata>::new_annotated(
            source_worker.client().clone(),
        );
        Self {
            manifest: self.manifest.clone(),
            engine: Some(worker),
            session: self.session.clone(),
            last_run_interrupted: false,
            store: self.store.clone(),
            worker_metadata_writer: None,
            segment_state: self.segment_state.clone(),
            filesystem_authority: self.filesystem_authority.clone(),
            workdir_session: self.workdir_session.clone(),
            workspace_context: self.workspace_context.clone(),
            flow_runtime_state: self.flow_runtime_state.clone(),
            flow_feature_enabled: self.flow_feature_enabled,
            scope: self.scope.clone(),
            delegation_scope: self.delegation_scope.clone(),
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: self.usage_history.clone(),
            tracker: None,
            task_feature: self.task_feature.clone(),
            worker_observation_provider: None,
            system_prompt_template: None,
            feature_instructions: self.feature_instructions.clone(),
            alerter: self.alerter.clone(),
            event_tx: self.event_tx.clone(),
            internal_worker_registry: self.internal_worker_registry.clone(),
            in_flight: self.in_flight.clone(),
            ai_activity_counter: self.ai_activity_counter.clone(),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: None,
            callback_socket: None,
            runtime_ticket_role: None,
            prompts: self.prompts.clone(),
            inject_resident_summary: self.inject_resident_summary,
            extract_in_flight: self.extract_in_flight.clone(),
            consolidation_in_flight: self.consolidation_in_flight.clone(),
            extract_pointer: self.extract_pointer.clone(),
            memory_task: None,
            user_segments: self.user_segments.clone(),
            // The memory-task clone never appends to the session log
            // (it only reads `worker.history()`), so a fresh sink is
            // fine — nothing observes its broadcast.
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        }
    }
}

impl<C: LlmClient + 'static, St: Store + Clone + 'static> Worker<C, St> {
    /// Build a `LogWriterHandle` carrying everything the worker
    /// callback / interceptor needs to commit `LogEntry` values
    /// directly: store handle, the shared session pointer, and the
    /// broadcast sink. All three are cheap clones.
    pub fn log_writer_handle(&self) -> LogWriterHandle<St> {
        LogWriterHandle {
            store: self.store.clone(),
            state: self.segment_state.clone(),
            sink: self.sink.clone(),
            in_flight: self.in_flight.clone(),
        }
    }

    /// Attach a type-erased system-item commit handle. The controller
    /// calls this once during spawn so the interceptor can commit
    /// `SystemItem`s directly without owning a generic store handle.
    /// Idempotent: subsequent calls overwrite the previous handle.
    pub fn attach_log_writer(&mut self, writer: Arc<dyn SystemItemCommitter>) {
        self.log_writer = Some(writer);
    }

    pub fn attach_in_flight_events(&mut self, in_flight: InFlightEvents) {
        self.in_flight = Some(in_flight);
    }

    pub fn clear_in_flight_events(&self) {
        if let Some(in_flight) = &self.in_flight {
            in_flight.clear();
        }
    }

    /// Wire `Engine::on_history_append` to commit each appended item
    /// directly as a singular `LogEntry::AssistantItem` / `ToolResult`
    /// through the writer. The controller calls this once per spawned
    /// Worker after the worker is built; tests that drive `Worker::new` may
    /// opt in to the same wiring or leave it off (in which case
    /// `persist_turn`'s inline fallback writes entries at turn end).
    ///
    /// `user_message` items are skipped because they are committed
    /// up-front via `commit_entry(LogEntry::UserInput { segments })`.
    /// `role:system` items are committed as typed `LogEntry::SystemItem`
    /// entries by their producers (for example `WorkerInterceptor` and
    /// interrupted-turn prep) before they reach the worker's history, so this
    /// callback would otherwise double-write them.
    pub fn wire_history_persistence(&mut self) {
        // History records are committed by the annotation callback before Agen
        // applies the corresponding entry. A second observer callback would
        // create an unannotated duplicate and is intentionally not installed.
        if self.manifest.session.record_event_trace {
            let writer = self.log_writer_handle();
            self.engine_mut()
                .on_stream_event(move |turn, llm_call, event| {
                    let entry = session_store::TraceEntry {
                        ts: segment_log::now_millis(),
                        turn,
                        llm_call: Some(llm_call),
                        payload: session_store::TracePayload::StreamEvent {
                            event: event.clone(),
                        },
                    };
                    if let Err(err) = writer.append_trace(&entry) {
                        warn!(error = %err, "stream event trace commit failed; dropping");
                    }
                });
            let writer = self.log_writer_handle();
            self.engine_mut()
                .on_lifecycle_trace(move |turn, llm_call, label, data| {
                    let entry = session_store::TraceEntry {
                        ts: segment_log::now_millis(),
                        turn,
                        llm_call: Some(llm_call),
                        payload: session_store::TracePayload::Lifecycle {
                            label: label.to_string(),
                            data: data.clone(),
                        },
                    };
                    if let Err(err) = writer.append_trace(&entry) {
                        warn!(error = %err, "lifecycle trace commit failed; dropping");
                    }
                });
        }
        self.history_persistence_wired = true;
    }
}

impl<C: LlmClient + Clone + 'static, St: Store + Clone + 'static> Worker<C, St> {
    pub fn spawn_post_run_memory_jobs(&mut self) {
        // Drop a finished prior handle so we can spawn a fresh task.
        // If the prior task is still running, coalesce by skipping —
        // extract/consolidation implementations re-evaluate thresholds on completion.
        self.cleanup_finished_memory_task();
        if self.memory_task.is_some() {
            return;
        }

        let mut worker = self.clone_for_memory_task();
        self.memory_task = Some(tokio::spawn(async move {
            if let Err(e) = worker.try_post_run_extract().await {
                tracing::warn!(error = %e, "Post-run memory extract task error");
            }
            if let Err(e) = worker.try_post_run_consolidate().await {
                tracing::warn!(error = %e, "Post-run memory consolidate task error");
            }
        }));
    }
}

impl<C: LlmClient + 'static, St: Store> Worker<C, St> {
    /// Create a new Worker from a pre-built Engine and store.
    ///
    /// Callers must pass path-free workspace context separately from explicit
    /// filesystem authority and build a [`Scope`] — typically via
    /// [`Scope::from_config`] when coming from a manifest, or [`Scope::writable`]
    /// in tests. Use [`WorkerFilesystemAuthority::None`] for no-workdir Workers.
    ///
    /// Note: this constructor does **not** parse `manifest.worker.system_prompt`
    /// as a template. `Worker::from_manifest` is the production path for
    /// templated prompts; callers of `Worker::new` that want a template
    /// should parse it themselves and call [`set_system_prompt_template`].
    pub async fn new(
        manifest: WorkerManifest,
        worker: Engine<C, Mutable, SessionHistoryMetadata>,
        store: St,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        scope: Scope,
    ) -> Result<Self, WorkerError> {
        // Segment creation is deferred to `ensure_segment_head` at first
        // run so a later-installed system-prompt template (see
        // `set_system_prompt_template`) can be captured by `SegmentStart`.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let prompts = Arc::new(ArcSwap::from(PromptCatalog::builtins_only()?));
        let delegation_scope =
            DelegationScope::from_config(&manifest.delegation_scope).map_err(WorkerError::Scope)?;
        let scope = SharedScope::new(scope);
        let workdir_session = workdir_session_from_authority(&filesystem_authority, &scope);
        let mut worker = Self {
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer: None,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority,
            workdir_session,
            workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            delegation_scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::<UsageRecord>::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            system_prompt_template: None,
            feature_instructions: Vec::new(),
            alerter: None,
            event_tx: None,
            internal_worker_registry: None,
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: None,
            callback_socket: None,
            runtime_ticket_role: None,
            prompts,
            inject_resident_summary: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        Ok(worker)
    }

    /// Install a parsed system-prompt template that will be rendered
    /// exactly once, immediately before the first LLM turn. Mirrors the
    /// path used by `Worker::from_manifest` and is exposed for tests and
    /// other callers that build a Worker without going through a manifest.
    pub fn set_system_prompt_template(&mut self, template: SystemPromptTemplate) {
        self.system_prompt_template = Some(template);
    }

    pub fn register_feature_instruction(&mut self, instruction: FeatureInstructionDeclaration) {
        let mut instructions = self.feature_instructions.clone();
        instructions.push(instruction);
        self.feature_instructions = dedupe_instruction_contributions(instructions);
    }

    pub fn register_worker_orchestration_instruction(&mut self) {
        self.register_feature_instruction(worker_orchestration_instruction());
    }

    /// Toggle all resident sections in the system prompt.
    ///
    /// Default `true`: normal Workers may expose each resident section according
    /// to its own gate and manifest settings. Internal disposable workers set
    /// suppressed while explicit tools remain available.
    pub fn set_resident_memory_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
    }

    /// Toggle workspace Memory document resident injection in the system prompt.
    pub fn set_resident_summary_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
    }

    pub fn prompts(&self) -> Arc<ArcSwap<PromptCatalog>> {
        Arc::clone(&self.prompts)
    }

    fn prompt_render_provenance(&self, logical_name: &str) -> PromptRenderProvenance {
        let prompts = self.prompts.load();
        let projection = prompts.projection();
        PromptRenderProvenance {
            workspace_id: self
                .workspace_context
                .workspace_id()
                .map(|workspace_id| workspace_id.as_str().to_string()),
            config_revision: projection.config_revision,
            source_digest: projection.source_digest.clone(),
            projection_digest: projection.catalog_digest.clone(),
            logical_name: logical_name.to_string(),
        }
    }

    fn refresh_prompt_projection_for_future_operations(&self) -> Result<(), WorkerError> {
        // The launch catalog remains authoritative until the initial system
        // Prompt has been rendered and committed. Later operation boundaries
        // may adopt the Workspace's current immutable projection.
        if self.system_prompt_template.is_some() {
            return Ok(());
        }
        let Some(resolution) = self
            .workspace_context
            .client()
            .current_prompt_projection(None)
            .map_err(|source| WorkerError::WorkspacePromptProjection {
                message: source.to_string(),
            })?
        else {
            return Ok(());
        };
        let projection = &resolution.projection;
        let current = self.prompts.load();
        if current.projection().config_revision == projection.config_revision
            && current.projection().source_digest == projection.source_digest
            && current.projection().catalog_digest == projection.projection_digest
            && Arc::ptr_eq(&current, &resolution.catalog)
        {
            return Ok(());
        }
        self.prompts.store(resolution.catalog);
        Ok(())
    }

    /// The current segment ID. Read lock-free from the shared session
    /// pointer so fork-time swaps are observed immediately.
    pub fn segment_id(&self) -> SegmentId {
        self.segment_state.segment_id()
    }

    /// The Session this Worker belongs to. Stable across compaction and
    /// auto-fork (both stay within the same Session); there is no
    /// Worker-level operation today that moves a running Worker to a different
    /// Session.
    pub fn session_id(&self) -> SessionId {
        self.segment_state.session_id()
    }

    /// The Worker's manifest.
    pub fn manifest(&self) -> &WorkerManifest {
        &self.manifest
    }

    /// Process-local Ticket role marker supplied by the role launcher.
    pub fn runtime_ticket_role(&self) -> Option<&str> {
        self.runtime_ticket_role.as_deref()
    }

    /// Set the process-local Ticket role marker. Intended for entrypoint
    /// launch metadata, not for model-visible prompts or durable claims.
    pub fn set_runtime_ticket_role(&mut self, role: Option<String>) {
        self.runtime_ticket_role = role;
    }

    /// Explicit filesystem authority held by this Worker.
    pub fn filesystem_authority(&self) -> &WorkerFilesystemAuthority {
        &self.filesystem_authority
    }

    /// Local working directory when this Worker has local filesystem authority.
    pub fn local_working_directory(&self) -> Option<&LocalWorkingDirectory> {
        self.filesystem_authority.as_local()
    }

    pub fn workdir_session(&self) -> Option<&WorkdirSessionHandle> {
        self.workdir_session.as_ref()
    }

    /// Replace the constructor fallback with the provider binding resolved by
    /// the owning Runtime. Runtime calls this before the Worker controller is
    /// spawned, so tools only ever observe the Runtime-bound handle.
    pub fn bind_workdir_session(&mut self, workdir_session: Option<WorkdirSessionHandle>) {
        self.workdir_session = workdir_session;
    }

    /// Path-free workspace identity, if Runtime/host associated this Worker
    /// with a workspace.
    pub fn workspace_id(&self) -> Option<&WorkspaceId> {
        self.workspace_context.workspace_id()
    }

    /// Narrow workspace client/availability handle injected by Runtime/host.
    /// This never grants local filesystem authority.
    pub fn workspace_client(&self) -> &dyn WorkspaceClient {
        self.workspace_context.client()
    }

    pub(crate) fn workspace_context_handle(&self) -> WorkerWorkspaceContext {
        self.workspace_context.clone()
    }

    pub fn workspace_client_handle(&self) -> Arc<dyn WorkspaceClient> {
        self.workspace_context.client_handle()
    }

    /// Bind the host-owned Worker-session observation projection. The provider
    /// is responsible for workspace authorization and per-capture revalidation.
    pub fn bind_worker_observation_provider(
        &mut self,
        provider: Option<Arc<dyn WorkerObservationProvider>>,
    ) {
        self.worker_observation_provider = provider;
    }

    pub(crate) fn worker_observation_provider(&self) -> Option<Arc<dyn WorkerObservationProvider>> {
        self.worker_observation_provider.clone()
    }

    async fn resident_summary_from_workspace_authority(
        &self,
    ) -> Result<Option<String>, WorkerError> {
        let result = self
            .workspace_client()
            .execute_memory_backend_operation(
                memory::backend::MemoryBackendOperation::ResidentSummary(
                    memory::backend::MemoryResidentSummaryOperation::default(),
                ),
            )
            .await?;
        match result {
            memory::backend::MemoryBackendOperationResult::ToolOutput(output) => Ok(output.content),
            other => Err(WorkerError::FeatureInstall(format!(
                "unexpected memory backend result for resident summary: {other:?}"
            ))),
        }
    }

    /// Activate an Agent Skill through the Workspace backend/client and commit
    /// the returned SKILL.md body to history before it can influence an LLM run.
    ///
    /// This deliberately does not scan `.yoi/skills` locally: when a Workspace
    /// HTTP client is available, catalog/detail/activation authority belongs to
    /// the Workspace backend API.
    pub fn activate_skill(&mut self, name: &str) -> Result<SkillActivationResponse, WorkerError>
    where
        St: Clone + 'static,
    {
        let activation = self.workspace_client().activate_skill(name)?;
        self.ensure_segment_head()?;
        let body = format!(
            "Agent Skill `{}` activated from {}.\n\n{}",
            activation.name, activation.provenance.id, activation.body
        );
        let skill_metadata = new_history_metadata(
            WorkerHistoryProvenance::BackendInstruction { operation_id: None },
            None,
        );
        self.commit_entry(LogEntry::AnnotatedSystemItem {
            ts: segment_log::now_millis(),
            entry: session_store::LoggedSystemHistoryEntry {
                item: SystemItem::SkillActivation {
                    name: activation.name.clone(),
                    body: body.clone(),
                },
                metadata: skill_metadata.clone(),
            },
        })?;
        let history_entry = HistoryEntry::new(agen::Item::system_message(body), skill_metadata);
        let mut annotate = history_annotator(
            self.log_writer_handle(),
            vec![history_entry.clone()],
            self.pending_committed_history.clone(),
        );
        let (engine, session) = (
            self.engine.as_mut().expect("worker present"),
            &mut self.session,
        );
        engine.append_history_with(
            session.history_mut(),
            std::iter::once(history_entry.item),
            &mut annotate,
        )?;
        session.note_mutation();
        Ok(activation)
    }

    /// The Worker's directory scope, as a shared atomically-swappable
    /// handle. Clone it to share scope state with another consumer
    /// (e.g. a tool that needs to mutate scope dynamically).
    pub fn scope(&self) -> &SharedScope {
        &self.scope
    }

    /// Snapshot the current scope as an owned `Arc<Scope>`. Subsequent
    /// scope mutations do not affect the returned snapshot.
    pub fn scope_snapshot(&self) -> Arc<Scope> {
        self.scope.snapshot()
    }

    /// Apply `extra_allow` to the Worker's runtime scope. Future tool
    /// permission checks (read/write/glob/grep) reflect the broadened
    /// scope; in-flight tool calls keep the snapshot they captured at
    /// invocation time.
    pub fn add_scope_rules(
        &self,
        extra_allow: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<(), ScopeError> {
        let extra: Vec<ScopeRule> = extra_allow.into_iter().collect();
        self.scope
            .update(|cur| cur.with_added_allow_rules(extra.clone()))
    }

    /// Strip `revoke` rules from the Worker's runtime scope by adding
    /// matching deny rules. A `Permission::Write` revoke caps effective
    /// access at `Read` (mirroring the worker-allocation `effective_write`
    /// semantics — Write is the only permission tracked across Workers).
    /// A `Permission::Read` revoke removes access entirely.
    pub fn revoke_scope_rules(
        &self,
        revoke: impl IntoIterator<Item = ScopeRule>,
    ) -> Result<(), ScopeError> {
        let revoke: Vec<ScopeRule> = revoke.into_iter().collect();
        self.scope
            .update(|cur| cur.with_added_deny_rules(revoke.clone()))
    }

    /// Append `entry` to the session log AND publish it through the
    /// broadcast sink. The Store is the commit boundary: a failed write is
    /// never counted or published.
    pub(crate) fn commit_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        let loc = self.segment_state.location();
        self.store.append(loc.session_id, loc.segment_id, &entry)?;
        self.segment_state.increment_entries();
        self.sink.publish(entry);
        Ok(())
    }

    /// Cloneable sink handle. Exposed to the controller so the IPC
    /// layer can `subscribe_with_snapshot` and stream entries to
    /// clients without consulting any other state.
    pub fn sink(&self) -> SegmentLogSink {
        self.sink.clone()
    }

    /// Direct access to the underlying Engine.
    pub fn engine(&self) -> &Engine<C, Mutable, SessionHistoryMetadata> {
        self.engine.as_ref().expect("worker taken during run")
    }

    /// Mutable access to the underlying Engine.
    ///
    /// Use this to register tools, hooks, or subscribers before calling
    /// [`run`](Self::run).
    pub fn engine_mut(&mut self) -> &mut Engine<C, Mutable, SessionHistoryMetadata> {
        self.engine.as_mut().expect("worker taken during run")
    }

    #[cfg(test)]
    fn set_history_for_test(&mut self, items: Vec<Item>) {
        let entries = items
            .into_iter()
            .map(|item| HistoryEntry::new(item, SessionHistoryMetadata::legacy_unknown()))
            .collect();
        self.session.replace_history(entries);
    }

    /// Install enabled feature modules into the Worker host surfaces.
    pub fn install_features(
        &mut self,
        registry: FeatureRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        let worker = self.engine.as_mut().expect("worker taken during run");
        let report = registry.install_into_engine(worker, &mut self.hook_builder);
        for instruction in report.installed_instruction_contributions() {
            self.register_feature_instruction(instruction);
        }
        report
    }

    /// Install the Flow transition feature against this Worker's Runtime-owned
    /// durable Flow state. Workspace authority is used only when resolving a
    /// new immutable source snapshot during Submit.
    pub fn install_runtime_flow_transition_feature(
        &mut self,
    ) -> Result<FeatureRegistryInstallReport, String>
    where
        C: Clone + Send + Sync + 'static,
        St: Clone + Send + Sync + 'static,
    {
        self.flow_feature_enabled = true;
        let writer = LogWriterHandle {
            store: self.store.clone(),
            state: self.segment_state.clone(),
            sink: self.sink.clone(),
            in_flight: self.in_flight.clone(),
        };
        let coordinator = Arc::new(
            crate::feature::builtin::flow_transition::RuntimeFlowCoordinatorClient::new(
                self.flow_runtime_state.clone(),
                Arc::new(SessionFlowRuntimeStateCommitter { writer }),
            ),
        );
        Ok(self.install_flow_transition_feature(coordinator))
    }

    /// Install the Flow transition tool for a host-authorized active Flow instance.
    ///
    /// The coordinator is already bound to the authenticated Workspace/Worker;
    /// model input cannot select an instance or Worker identity. The verifier uses
    /// an immutable capture of the current committed segment and, for local
    /// Workdirs, exposes only Read/Glob/Grep through a read-only session.
    pub fn install_flow_transition_feature(
        &mut self,
        coordinator: Arc<dyn crate::feature::builtin::flow_transition::FlowCoordinatorClient>,
    ) -> FeatureRegistryInstallReport
    where
        C: Clone + Send + Sync + 'static,
        St: Clone + Send + Sync + 'static,
    {
        let location = self.segment_state.location();
        let capture = crate::feature::builtin::flow_transition::StoreFlowParentCapture::new(
            self.store.clone(),
            location.session_id,
            location.segment_id,
        );
        let read_only_tools = self
            .workdir_session
            .clone()
            .map(|source| Arc::new(ReadOnlyWorkdirSession::new(source)) as WorkdirSessionHandle)
            .map(tools::read_only_builtin_tools)
            .unwrap_or_default();
        let client = self
            .engine
            .as_ref()
            .expect("worker taken during run")
            .client()
            .clone();
        let verifier = Arc::new(
            crate::feature::builtin::flow_transition::WorkerBackedFlowVerifier::new(
                client,
                self.manifest.clone(),
                capture,
                read_only_tools,
            ),
        );
        let state = crate::feature::builtin::flow_transition::FlowTransitionState::new(
            coordinator,
            verifier,
        );
        self.install_features(FeatureRegistryBuilder::new().with_module(
            crate::feature::builtin::flow_transition::FlowTransitionFeature::new(state),
        ))
    }

    /// Reference to the store.
    pub fn store(&self) -> &St {
        &self.store
    }

    /// List user-submitted turns in newest-first order for the manual rewind picker.
    pub fn list_rewind_targets(&self) -> Result<(usize, Vec<RewindTarget>), RewindError> {
        let loc = self.segment_state.location();
        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        Ok((
            entries.len(),
            build_rewind_targets(loc.segment_id, &entries),
        ))
    }

    /// Truncate the current segment to just before a previously listed user input.
    pub fn rewind_to(
        &mut self,
        target: RewindTargetId,
        expected_head_entries: usize,
    ) -> Result<RewindAppliedState, RewindError> {
        let loc = self.segment_state.location();
        if target.segment_id != loc.segment_id {
            return Err(RewindError::Invalid(
                "rewind target belongs to a different segment".into(),
            ));
        }

        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        if entries.len() != expected_head_entries {
            return Err(RewindError::Invalid(format!(
                "session head changed since picker opened (expected {expected_head_entries}, current {})",
                entries.len()
            )));
        }

        let input = match entries.get(target.user_input_entry_index) {
            Some(LogEntry::UserInput { segments, .. })
            | Some(LogEntry::AnnotatedUserInput { segments, .. }) => segments.clone(),
            _ => {
                return Err(RewindError::Invalid(
                    "rewind target is no longer a user message".into(),
                ));
            }
        };
        let truncate_entries = rewind_truncate_entries(&entries, target.user_input_entry_index);
        let retained = entries[..truncate_entries].to_vec();
        let tool_side_effect_warning = suffix_has_tool_side_effects(&entries[truncate_entries..]);
        let state = segment_log::collect_state(&retained);
        let extract_pointer = memory::extract::fold_pointer(&state.extensions);
        let summary = RewindSummary {
            truncated_to_entries: truncate_entries,
            discarded_entries: entries.len().saturating_sub(truncate_entries),
            tool_side_effect_warning,
        };

        self.store
            .truncate(loc.session_id, loc.segment_id, truncate_entries)?;
        self.segment_state.set_entries_written(truncate_entries);
        self.sink.truncate_silent(truncate_entries);

        let history_entries = restore_history_entries(loc.session_id, loc.segment_id, &retained)
            .map_err(|error| RewindError::Invalid(error.into()))?;
        let projected_history = history_entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        self.task_feature.restore_from_history(&projected_history);
        self.session.replace_history(history_entries);
        self.engine_mut().set_request_config(state.config);
        self.engine_mut().set_turn_count(state.turn_count);
        self.last_run_interrupted = state.last_run_interrupted;
        self.engine_mut()
            .set_active_run_turn_count(state.active_run_turn_count);
        self.user_segments = state.user_segments;
        *self.usage_history.lock().expect("usage_history poisoned") = state.usage_history;
        *self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned") = Vec::new();
        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = extract_pointer;

        Ok(RewindAppliedState {
            entries: retained,
            input,
            summary,
        })
    }

    fn worker_metadata(&self, active: Option<WorkerActiveSegmentRef>) -> WorkerMetadata {
        worker_metadata_for_manifest(
            &self.manifest,
            self.workspace_id(),
            self.filesystem_authority
                .as_local()
                .map(|local| local.root.as_path()),
            active,
        )
    }

    fn write_worker_metadata_pending(&self) -> Result<(), WorkerError> {
        let Some(writer) = &self.worker_metadata_writer else {
            return Ok(());
        };
        writer(
            self.worker_metadata(Some(WorkerActiveSegmentRef::pending_segment(
                self.session_id(),
            ))),
        )?;
        Ok(())
    }

    fn write_worker_metadata_active(&self, loc: SegmentLocation) -> Result<(), WorkerError> {
        let Some(writer) = &self.worker_metadata_writer else {
            return Ok(());
        };
        writer(
            self.worker_metadata(Some(WorkerActiveSegmentRef::active_segment(
                loc.session_id,
                loc.segment_id,
            ))),
        )?;
        Ok(())
    }

    /// Enable name-keyed Worker metadata write-through for Workers built through
    /// the low-level constructor. High-level manifest constructors enable it
    /// automatically; this hook lets tests and custom embedders opt into the
    /// same persistence behavior without changing `Worker::new`'s minimal bounds.
    pub fn enable_worker_metadata_write_through(&mut self) -> Result<(), WorkerError>
    where
        St: WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        self.worker_metadata_writer = Some(worker_metadata_writer_for_store(&self.store));
        self.write_worker_metadata_pending()
    }

    /// Provider-visible projection of the current typed Worker session history.
    /// The authoritative item+provenance entries remain owned by `WorkerSession`.
    pub fn history(&self) -> Vec<Item> {
        self.session.history().items_cloned()
    }

    pub fn session_history(&self) -> &History<SessionHistoryMetadata> {
        self.session.history()
    }

    pub fn worker_session(&self) -> &WorkerSession {
        &self.session
    }

    /// Snapshot of the cumulative LLM Usage measurement timeline.
    ///
    /// One entry per LLM call. Restored on `restore` and appended in
    /// `persist_turn`. Used by token-accounting APIs in [`token_counter`].
    /// Returns a clone since the underlying vector is shared with hooks
    /// running on the Engine.
    pub fn usage_history(&self) -> Vec<UsageRecord> {
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clone()
    }

    /// Snapshot of the extract (memory.extract) boundary pointer.
    ///
    /// `None` means no extract has run yet on the current session — the
    /// next extract will start from entry 0. Updated by
    /// [`try_post_run_extract`](Self::try_post_run_extract) on success
    /// and reset by [`compact`](Self::compact) (the new compacted
    /// session has a fresh log with no `LogEntry::Extension` entries).
    /// Cheap clone via `Option<Clone>`.
    /// Snapshot of the typed user segments tracked alongside worker
    /// history. The K-th entry corresponds to the K-th `Item::user_message`
    /// derived from `LogEntry::UserInput` entries (post-compaction); seed
    /// history loaded via `SegmentStart.history` does not contribute,
    /// which is acceptable because the original segments are unrecoverable.
    pub fn user_segments(&self) -> &[Vec<Segment>] {
        &self.user_segments
    }

    pub fn extract_pointer(&self) -> Option<memory::ExtractPointerPayload> {
        self.extract_pointer
            .lock()
            .expect("extract_pointer poisoned")
            .clone()
    }

    /// Test/diagnostic handle to the consolidation in-flight guard. Production
    /// callers do not need this; tests use it to assert that the reentry
    /// guard skips an in-progress consolidation without losing data.
    #[doc(hidden)]
    pub fn consolidation_in_flight_handle(&self) -> Arc<AtomicBool> {
        self.consolidation_in_flight.clone()
    }

    /// Shared handle to the cumulative Usage history.
    ///
    /// Callbacks that need live access to the latest measurements (e.g.
    /// the savings estimator that `attach_prune` installs on the Engine)
    /// clone this `Arc` and read it at request time. The handle outlives
    /// any individual run.
    ///
    /// **Locking contract:** the inner `Mutex` is held only for a short
    /// clone (`lock().unwrap().clone()`) and released immediately.
    /// Callers must not hold the guard across `.await` points, I/O, or
    /// long computations — the guard is implicitly assumed to be
    /// non-contended at every Worker lifecycle event.
    pub fn usage_history_handle(&self) -> Arc<Mutex<Vec<UsageRecord>>> {
        self.usage_history.clone()
    }

    /// Handle to the per-LLM-request `UsageTracker`.
    ///
    /// Sibling modules (e.g. the prune observer) clone this `Arc` to stash
    /// per-request side state (e.g. a `correlation_id`) that pairs with
    /// the next `LlmUsage`.
    pub(crate) fn usage_tracker_handle(&self) -> Arc<UsageTracker> {
        self.usage_tracker.clone()
    }

    /// Handle to the synchronous `MetricsTracker` buffer.
    ///
    /// Engine callbacks (e.g. the prune observer) clone this `Arc` and
    /// `.push(metric)` into it; Worker drains it in `persist_turn` and
    /// writes each metric via `session_metrics::record_metric`.
    pub(crate) fn metrics_tracker_handle(
        &self,
    ) -> Arc<crate::compact::metrics_tracker::MetricsTracker> {
        self.metrics_tracker.clone()
    }

    /// Attach the session-scoped file-operation tracker from the builtin
    /// `tools` crate. Called by the Controller immediately after it
    /// registers the builtin tools on the Engine. Overwrites any
    /// previously attached tracker.
    pub fn attach_tracker(&mut self, tracker: tools::Tracker) {
        self.tracker = Some(tracker);
    }

    /// Built-in Task feature module and snapshot/restore facade.
    pub(crate) fn task_feature(&self) -> TaskFeature {
        self.task_feature.clone()
    }

    /// The attached session-scoped file-operation tracker, if any.
    pub fn tracker(&self) -> Option<&tools::Tracker> {
        self.tracker.as_ref()
    }

    /// Attach a user-facing notification sink.
    ///
    /// Called by the Controller immediately after spawning so that
    /// Worker-internal operations (compaction failures, AGENTS.md
    /// ingestion warnings) can surface messages to connected clients.
    pub fn attach_alerter(&mut self, alerter: Alerter) {
        self.alerter = Some(alerter);
    }

    /// Attach the broadcast sender used for typed lifecycle `Event`s.
    ///
    /// The Controller wires this alongside [`attach_alerter`] so that
    /// Worker-internal operations (currently: compaction) can surface
    /// progress to connected clients.
    pub fn attach_event_tx(&mut self, event_tx: broadcast::Sender<Event>) {
        let session_id = self.session_id().to_string();
        let registry = self.internal_worker_registry.get_or_insert_with(
            crate::spawn::registry::SpawnedWorkerRegistry::new_for_internal_services,
        );
        registry.attach_parent_protocol(event_tx.clone(), session_id);
        self.event_tx = Some(event_tx);
    }

    pub(crate) fn attach_internal_worker_registry(
        &mut self,
        registry: Arc<crate::spawn::registry::SpawnedWorkerRegistry>,
    ) {
        self.internal_worker_registry = Some(registry);
    }

    /// Shared activity counter incremented by worker event bridges when any
    /// assistant-side output is surfaced before history persistence.
    pub fn ai_activity_counter(&self) -> Arc<AtomicUsize> {
        self.ai_activity_counter.clone()
    }

    fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        if let Some(n) = self.alerter.as_ref() {
            n.alert(level, source, message);
        }
    }

    /// Append a metric, swallowing errors so observability writes never
    /// fail the surrounding turn. On failure the head hash stays put
    /// (the entry is dropped) and a `Warn` alert + `tracing::warn!` are
    /// emitted so the failure isn't completely silent.
    fn try_record_metric(&mut self, metric: &session_metrics::Metric) {
        let payload = serde_json::to_value(metric).expect("Metric is Serialize");
        let entry = LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: session_metrics::DOMAIN.into(),
            payload,
        };
        if let Err(err) = self.commit_entry(entry) {
            warn!(name = %metric.name, error = %err, "failed to record session metric; dropping");
            self.alert(
                AlertLevel::Warn,
                AlertSource::Worker,
                format!("failed to record metric `{}`: {}", metric.name, err),
            );
        }
    }

    /// Broadcast a typed `Event` to connected clients. No-op when no
    /// `event_tx` is attached (tests / direct `Worker::new` usage) or when
    /// no clients are currently subscribed.
    fn send_event(&self, event: Event) {
        if let Some(tx) = self.event_tx.as_ref() {
            let _ = tx.send(event);
        }
    }

    /// Push a `Method::Notify` entry onto the pending buffer.
    ///
    /// The notification will be appended to `worker.history` as an
    /// `Item::system_message` just before the next LLM request, via
    /// `WorkerInterceptor::pending_history_appends`. See [`NotifyBuffer`]
    /// for overflow behaviour and the lane-of-record rationale.
    pub fn push_notify(&self, message: String, auto_run: bool) {
        self.pending_notifies.push_notify(message, auto_run);
    }

    /// Push an agent-visible typed `WorkerEvent` entry onto the pending buffer.
    ///
    /// Callers must classify control-plane-only WorkerEvents before invoking this.
    /// Same lifecycle as [`push_notify`](Self::push_notify) but
    /// preserves the typed `WorkerEvent` payload so the IPC layer can
    /// emit `SystemItem::WorkerEvent { event, body }` with structured
    /// data for clients.
    pub fn push_worker_event_notify(&self, event: protocol::WorkerEvent) {
        self.pending_notifies.push_worker_event(event);
    }

    /// Shared handle to the pending notification buffer.
    ///
    /// The Controller holds a clone so that `Method::Notify` arriving
    /// while `worker.run()` is in flight can still reach the interceptor.
    pub fn notify_buffer_handle(&self) -> NotifyBuffer {
        self.pending_notifies.clone()
    }

    /// Parent callback socket set by `from_manifest_spawned`.
    ///
    /// Consumed by the Controller to fire `Method::WorkerEvent` upward on
    /// lifecycle transitions. `None` for top-level Workers, in which case
    /// the Controller silently skips the send.
    pub fn callback_socket(&self) -> Option<&PathBuf> {
        self.callback_socket.as_ref()
    }

    // --- Hook registration ---

    fn assert_hooks_open(&self) {
        assert!(
            !self.interceptor_installed,
            "cannot add hooks after run() or resume() has been called"
        );
    }

    /// Register a hook that runs after receiving user input.
    pub fn add_on_prompt_submit_hook(&mut self, hook: impl Hook<OnPromptSubmit> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_prompt_submit(hook);
    }

    /// Register a hook that runs before each LLM request.
    pub fn add_pre_llm_request_hook(&mut self, hook: impl Hook<PreLlmRequest> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_pre_llm_request(hook);
    }

    /// Register a hook that runs before each tool call.
    pub fn add_pre_tool_call_hook(&mut self, hook: impl Hook<PreToolCall> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_pre_tool_call(hook);
    }

    /// Register a hook that runs after each tool call.
    pub fn add_post_tool_call_hook(&mut self, hook: impl Hook<PostToolCall> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_post_tool_call(hook);
    }

    /// Register a hook that runs at the end of a turn.
    pub fn add_on_turn_end_hook(&mut self, hook: impl Hook<OnTurnEnd> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_turn_end(hook);
    }

    /// Register a hook that runs when execution is aborted.
    pub fn add_on_abort_hook(&mut self, hook: impl Hook<OnAbort> + 'static) {
        self.assert_hooks_open();
        self.hook_builder.add_on_abort(hook);
    }

    /// Install the hook-based interceptor on the Engine if not already done.
    ///
    /// When either compaction threshold (`threshold` or
    /// `request_threshold`) is configured in the manifest, allocates
    /// a shared [`CompactState`] and wires the interceptor to read current
    /// occupancy through the `UsageRecord` timeline.
    fn ensure_interceptor_installed(&mut self) {
        if !self.interceptor_installed {
            let builder = std::mem::take(&mut self.hook_builder);
            let registry = Arc::new(builder.build());

            let (post_run_threshold, request_threshold, retained) = self
                .manifest
                .compaction
                .as_ref()
                .map(|c| (c.threshold, c.request_threshold, c.retained_tokens))
                .unwrap_or((None, None, manifest::defaults::COMPACT_RETAINED_TOKENS));

            let tracker_for_usage = self.usage_tracker.clone();
            self.engine_mut().on_usage(move |event| {
                tracker_for_usage.record_usage(event);
            });

            let compact_state = if post_run_threshold.is_some() || request_threshold.is_some() {
                if let (Some(post), Some(req)) = (post_run_threshold, request_threshold) {
                    if post > req {
                        warn!(
                            post_run_threshold = post,
                            request_threshold = req,
                            "threshold > request_threshold; \
                             proactive check will never fire before the safety net"
                        );
                    }
                }
                let state = Arc::new(CompactState::new(
                    post_run_threshold,
                    request_threshold,
                    retained,
                ));
                self.compact_state = Some(state.clone());
                Some(state)
            } else {
                None
            };

            let usage_history_handle = compact_state.as_ref().map(|_| self.usage_history.clone());

            let interceptor = WorkerInterceptor::new_with_history_queue(
                registry,
                compact_state,
                usage_history_handle,
                self.pending_notifies.clone(),
                self.pending_attachments.clone(),
                self.prompts.clone(),
                self.log_writer.clone(),
                self.pending_committed_history.clone(),
            )
            .with_usage_tracker(self.usage_tracker.clone())
            .with_prompt_workspace_id(
                self.workspace_context
                    .workspace_id()
                    .map(|workspace_id| workspace_id.as_str().to_string()),
            );
            self.engine_mut().set_interceptor(interceptor);
            self.interceptor_installed = true;
        }
    }

    /// Render the manifest-supplied instruction template exactly once,
    /// just before the first LLM turn, append the fixed trailing
    /// section (scope summary + optional AGENTS.md), and hand the
    /// resulting string to the Engine via `set_system_prompt`.
    /// Subsequent invocations are no-ops: the template field is
    /// consumed with `Option::take()`, so the materialised value
    /// persists across all later turns and compaction.
    async fn ensure_system_prompt_materialized(&mut self) -> Result<(), WorkerError> {
        let Some(template) = self.system_prompt_template.take() else {
            return Ok(());
        };
        let is_memory_consolidation = self.manifest.profile.as_ref().is_some_and(|snapshot| {
            matches!(
                &snapshot.source,
                manifest::ProfileSource::Registry {
                    source: manifest::ProfileRegistrySource::Builtin,
                    name,
                    ..
                } if name == "memory-consolidation"
            )
        });
        if is_memory_consolidation {
            let memory_config = self.manifest.memory.as_ref().ok_or_else(|| {
                WorkerError::InvalidState(
                    "Memory consolidation Worker has no Memory configuration".to_string(),
                )
            })?;
            let language = memory_language(memory_config)?;
            let rendered = self
                .prompts
                .load_full()
                .memory_consolidation_system(&language)?;
            self.engine
                .as_mut()
                .expect("worker present")
                .set_system_prompt(rendered);
            return Ok(());
        }
        let alerter = self.alerter.clone();
        let tool_names: Vec<String> = {
            let worker = self.engine.as_mut().expect("worker present");
            worker.tool_server_handle().flush_pending();
            worker
                .tool_server_handle()
                .tool_definitions_sorted()
                .into_iter()
                .map(|d| d.name)
                .collect()
        };
        let agents_md_read = self
            .filesystem_authority
            .as_local()
            .map(|local| read_agents_md(&local.root));
        if let Some(read) = agents_md_read.as_ref() {
            for warning in &read.warnings {
                if let Some(n) = alerter.as_ref() {
                    n.alert(AlertLevel::Warn, AlertSource::AgentsMd, warning.clone());
                }
            }
        }
        let inject_summary = self.inject_resident_summary
            && self
                .manifest
                .memory
                .as_ref()
                .is_some_and(|m| m.inject_summary.unwrap_or(true));
        let resident_summary: Option<String> = if inject_summary {
            match self.resident_summary_from_workspace_authority().await {
                Ok(summary) => summary,
                Err(error) => {
                    tracing::debug!(%error, "resident memory summary unavailable");
                    None
                }
            }
        } else {
            None
        };
        let worker_language = worker_language(&self.manifest.engine);
        let scope_snapshot = self.scope.snapshot();
        let cwd_for_prompt = self
            .local_working_directory()
            .map(|local| local.cwd.display().to_string())
            .unwrap_or_else(|| "no local working directory".to_string());
        let prompt_catalog = self.prompts.load_full();
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            cwd: cwd_for_prompt.into(),
            language: worker_language,
            scope: &scope_snapshot,
            tool_names,
            feature_instructions: &self.feature_instructions,
            agents_md: agents_md_read.and_then(|read| read.body),
            resident_summary: resident_summary.as_deref(),
            prompts: &prompt_catalog,
        };
        let rendered = template
            .render(&ctx)
            .map_err(|source| WorkerError::SystemPromptRender { source })?;
        self.engine
            .as_mut()
            .expect("worker present")
            .set_system_prompt(rendered);
        Ok(())
    }

    /// Convenience: run with a single `Segment::Text`.
    ///
    /// Equivalent to `run(vec![Segment::text(s)])`. The dumb-client
    /// counterpart of [`protocol::Method::run_text`]; primarily for
    /// tests and tools that have only a string in hand.
    pub async fn run_text(&mut self, s: impl Into<String>) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.run(vec![Segment::text(s)]).await
    }

    /// Drop the prior memory_task handle if it has finished. Keep it if
    /// still running so callers can decide whether to wait or coalesce.
    fn cleanup_finished_memory_task(&mut self) {
        if self.memory_task.as_ref().is_some_and(|h| h.is_finished()) {
            self.memory_task = None;
        }
    }

    /// Wait for the in-flight memory task (if any) to finish. Used before
    /// compact rewrites history (extract reads the same history).
    async fn join_memory_task(&mut self) {
        if let Some(handle) = self.memory_task.take()
            && let Err(e) = handle.await
        {
            tracing::warn!(error = %e, "Memory task join failed");
        }
    }

    /// Whether `try_pre_run_compact` would actually compact. The same
    /// check is duplicated inside `try_pre_run_compact` itself for
    /// defensive reasons; this is the gate for joining the memory task
    /// before the compact runs.
    fn should_pre_run_compact(&self) -> bool {
        self.compact_state.as_ref().is_some_and(|s| {
            !s.is_disabled()
                && !s.just_compacted()
                && s.exceeds_post_run(self.total_tokens().tokens)
        })
    }

    /// Prelude shared by `run` / `run_for_notification` / `resume`.
    /// Wires up worker hooks, ensures the session is materialized on the
    /// store, and runs pre-run compact (joining any in-flight memory task
    /// first so extract sees a stable history range).
    async fn prepare_for_run(&mut self) -> Result<(), WorkerError>
    where
        St: Clone + 'static,
    {
        self.refresh_prompt_projection_for_future_operations()?;
        self.ensure_interceptor_installed();
        self.ensure_system_prompt_materialized().await?;
        self.cleanup_finished_memory_task();
        self.ensure_segment_head()?;
        if self.should_pre_run_compact() {
            self.join_memory_task().await;
        }
        self.try_pre_run_compact().await;
        Ok(())
    }

    fn capture_empty_turn_rollback_snapshot(&self) -> EmptyTurnRollbackSnapshot {
        let pending_attachments = self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned")
            .clone();
        let usage_history_len = self
            .usage_history
            .lock()
            .expect("usage_history poisoned")
            .len();
        EmptyTurnRollbackSnapshot {
            history_len: self.session.history().len(),
            user_segments_len: self.user_segments.len(),
            entries_written: self.segment_state.entries_written(),
            sink_len: self.sink.len(),
            pending_attachments,
            usage_history_len,
            ai_activity_count: self.ai_activity_counter.load(Ordering::SeqCst),
            last_run_interrupted: self.last_run_interrupted,
            active_run_turn_count: self.engine().active_run_turn_count(),
            flow_runtime_state: self
                .flow_runtime_state
                .lock()
                .expect("flow_runtime_state poisoned")
                .clone(),
        }
    }

    fn should_rollback_empty_turn(
        &self,
        result: &EngineRunExit,
        snapshot: &EmptyTurnRollbackSnapshot,
    ) -> bool {
        if !matches!(
            result,
            EngineRunExit::Paused | EngineRunExit::Interrupted(StopReason::Cancelled)
        ) {
            return false;
        }
        if self.ai_activity_counter.load(Ordering::SeqCst) != snapshot.ai_activity_count {
            return false;
        }
        !self.session.history().entries()[snapshot.history_len..]
            .iter()
            .map(|entry| &entry.item)
            .any(is_ai_materialized_item)
    }

    fn rollback_empty_turn(
        &mut self,
        snapshot: EmptyTurnRollbackSnapshot,
    ) -> Result<(), StoreError> {
        self.session.history_mut().truncate(snapshot.history_len);
        self.session.note_mutation();
        self.last_run_interrupted = snapshot.last_run_interrupted;
        self.engine_mut()
            .set_active_run_turn_count(snapshot.active_run_turn_count);
        *self
            .flow_runtime_state
            .lock()
            .expect("flow_runtime_state poisoned") = snapshot.flow_runtime_state;
        self.user_segments.truncate(snapshot.user_segments_len);
        *self
            .pending_attachments
            .lock()
            .expect("pending_attachments poisoned") = snapshot.pending_attachments;
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .truncate(snapshot.usage_history_len);
        let _ = self.usage_tracker.drain();
        let _ = self.metrics_tracker.drain();
        let loc = self.segment_state.location();
        self.store
            .truncate(loc.session_id, loc.segment_id, snapshot.entries_written)?;
        self.segment_state
            .set_entries_written(snapshot.entries_written);
        self.sink.truncate_silent(snapshot.sink_len);
        Ok(())
    }

    fn prepare_flow_input(
        &self,
        input: Vec<Segment>,
    ) -> Result<
        (
            Vec<Segment>,
            Option<flow::FlowRuntimeState>,
            Option<PreparedFlowProjection>,
        ),
        WorkerError,
    > {
        let flow_segments = input
            .iter()
            .filter_map(|segment| match segment {
                Segment::Flow { selector } => Some(selector.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if flow_segments.is_empty() {
            return Ok((input, None, None));
        }
        if flow_segments.len() != 1 {
            return Err(WorkerError::FlowInput(
                "one Submit may contain at most one Flow segment".to_string(),
            ));
        }
        if !self.flow_feature_enabled {
            return Err(WorkerError::FlowInput(
                "resolved Profile does not enable feature.flow".to_string(),
            ));
        }
        if self
            .flow_runtime_state
            .lock()
            .expect("flow_runtime_state poisoned")
            .as_ref()
            .is_some_and(|state| state.instance.status == flow::FlowInstanceStatus::Active)
        {
            return Err(WorkerError::FlowInput(
                "Worker already has an active Flow instance".to_string(),
            ));
        }

        let selector = flow_segments[0]
            .parse::<flow::FlowSelector>()
            .map_err(|error| WorkerError::FlowInput(error.to_string()))?;
        let workspace = self.workspace_context.client_handle();
        if !workspace.is_available() {
            return Err(WorkerError::FlowInput(
                "Workspace client is unavailable".to_string(),
            ));
        }
        let workspace_id = workspace
            .workspace_id()
            .filter(|workspace_id| !workspace_id.trim().is_empty())
            .ok_or_else(|| {
                WorkerError::FlowInput("Workspace client has no Workspace scope".to_string())
            })?;
        let request = flow::FlowSourceResolveRequest {
            selector: selector.clone(),
        };
        let response = workspace
            .execute(WorkspaceRequest {
                method: WorkspaceRequestMethod::Post,
                path: format!("/api/w/{workspace_id}/flows/resolve"),
                body: Some(serde_json::to_string(&request).map_err(|error| {
                    WorkerError::FlowInput(format!("serialize Flow source request: {error}"))
                })?),
            })
            .map_err(|error| WorkerError::FlowInput(error.to_string()))?;
        if !(200..300).contains(&response.status) {
            let message = serde_json::from_str::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|body| {
                    body.get("error")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| "Workspace rejected Flow source resolution".to_string());
            return Err(WorkerError::FlowInput(format!(
                "{message} (HTTP {})",
                response.status
            )));
        }
        let source: flow::ResolvedFlowSource = serde_json::from_str(&response.body)
            .map_err(|error| WorkerError::FlowInput(format!("decode Flow source: {error}")))?;
        if source.workspace_id != workspace_id || source.selector != selector {
            return Err(WorkerError::FlowInput(
                "Workspace returned a Flow source outside the requested scope".to_string(),
            ));
        }
        let (state, initial_instructions) =
            flow::FlowRuntimeState::start(&source, uuid::Uuid::now_v7().to_string())
                .map_err(|error| WorkerError::FlowInput(error.to_string()))?;
        let projection = PreparedFlowProjection {
            selector: selector.to_string(),
            instructions: initial_instructions,
            definition_id: state.instance.definition_id.clone(),
            definition_revision: state.instance.definition_revision,
            instance_id: state.instance.instance_id.clone(),
            state_id: state.instance.current_state.to_string(),
        };
        Ok((input, Some(state), Some(projection)))
    }

    /// Send user input and run until the LLM turn completes.
    ///
    /// `input` is a typed segment list (see [`protocol::Segment`]). The
    /// Worker flattens it into a single user-message string for the
    /// underlying Engine, expanding paste content inline, resolving file refs
    /// into adjacent attachments where possible, and surfacing alerts for
    /// unresolved refs / unsupported segment kinds.
    ///
    /// If the between-turns compaction threshold is exceeded mid-run,
    /// the Engine is aborted, history is compacted, and execution resumes
    /// automatically.
    pub async fn run(&mut self, input: Vec<Segment>) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.run_with_input_extensions(input, Vec::new()).await
    }

    pub(crate) async fn run_with_input_extensions(
        &mut self,
        input: Vec<Segment>,
        input_extensions: Vec<SessionExtension>,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.run_with_input_extensions_and_commit_hook(input, input_extensions, || {})
            .await
    }

    /// Run user input and invoke `on_input_committed` only after the annotated
    /// input has crossed both the durable Store and live SegmentLogSink commit
    /// boundaries. The Controller uses this fence before exposing `Running`, so
    /// every in-flight snapshot for a user turn includes its committed input.
    pub(crate) async fn run_with_input_extensions_and_commit_hook<F>(
        &mut self,
        input: Vec<Segment>,
        mut input_extensions: Vec<SessionExtension>,
        on_input_committed: F,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
        F: FnOnce(),
    {
        let (input, pending_flow_state, flow_projection) = self.prepare_flow_input(input)?;
        if let Some(state) = pending_flow_state.as_ref() {
            let payload = serde_json::to_value(state).map_err(|error| {
                WorkerError::FlowInput(format!("serialize Flow runtime state: {error}"))
            })?;
            input_extensions.push(SessionExtension::new(
                FLOW_RUNTIME_EXTENSION_DOMAIN,
                payload,
            ));
        }

        // Paused→Run transition: if the previous turn was cut short,
        // any `Item::ToolCall` whose tool never produced a matching
        // `ToolResult` is closed with a synthetic one, and a short
        // system note explaining the interruption is appended — so the
        // next request is wire-valid (Anthropic) and the LLM knows
        // prior work was abandoned. Driven by the worker's own
        // `last_run_interrupted` flag; `Worker::resume` reuses the prior
        // context via a different entry point and never triggers this
        // path.
        self.prepare_interrupted_history_for_fresh_run()?;

        self.prepare_for_run().await?;

        let rollback_snapshot = self.capture_empty_turn_rollback_snapshot();

        // IDLE → active marker. Commits first so the next UserInput entry
        // is contained inside this Invoke range. See `tickets/invoke-turn-llmcall-semantics.md`.
        self.commit_entry(LogEntry::Invoke {
            ts: segment_log::now_millis(),
            trigger: protocol::InvokeKind::UserSend,
        })?;

        let projected_input = self.projected_input_history(&input, flow_projection.as_ref());

        // Persist original typed segments together with the exact ordered
        // model-visible item+origin projection before any entry becomes live.
        self.commit_entry(LogEntry::AnnotatedUserInput {
            ts: segment_log::now_millis(),
            segments: input.clone(),
            extensions: input_extensions,
            history: projected_input
                .iter()
                .map(to_logged_history_entry)
                .collect(),
        })?;
        if let Some(state) = pending_flow_state {
            *self
                .flow_runtime_state
                .lock()
                .expect("flow_runtime_state poisoned") = Some(state);
        }
        self.user_segments.push(input.clone());
        on_input_committed();

        // Resolve `@<path>` file refs to system messages stashed for the
        // WorkerInterceptor to attach right after the user message. Resolution
        // failures are non-fatal alerts.
        let attachments = self.resolve_file_refs(&input).await;
        if !attachments.is_empty() {
            *self
                .pending_attachments
                .lock()
                .expect("pending_attachments poisoned") = attachments;
        }

        let history_before = self.session.history().len();
        let pending_input = projected_input;
        let input_entry = pending_input
            .last()
            .cloned()
            .expect("projected Worker input is never empty");
        let prefix_items = pending_input
            .iter()
            .take(pending_input.len().saturating_sub(1))
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let input_string = input_entry.item.as_text().unwrap_or_default();
        let mut annotate = history_annotator(
            self.log_writer_handle(),
            pending_input,
            self.pending_committed_history.clone(),
        );

        if !prefix_items.is_empty() {
            let (engine, session) = (
                self.engine.as_mut().expect("worker present"),
                &mut self.session,
            );
            engine
                .append_history_with(session.history_mut(), prefix_items, &mut annotate)
                .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
        }

        let worker = self.engine.take().expect("worker taken during run");
        let mut locked = worker.lock(self.session.history());
        let result = locked
            .run_with_annotation(self.session.history_mut(), input_string, &mut annotate)
            .await;
        self.engine = Some(locked.unlock());
        self.session.note_mutation();

        if self.should_rollback_empty_turn(&result, &rollback_snapshot) {
            self.rollback_empty_turn(rollback_snapshot)?;
            return Ok(WorkerRunResult::RolledBack);
        }

        self.handle_worker_result(result, history_before).await
    }

    /// Resolve every `Segment::FileRef` in `segments` to a `[File: <path>]`
    /// or shallow `[Dir: <path>]` system message via `WorkerFsView`. Resolution
    /// failures (out-of-scope, not-found, binary, I/O, unsupported symlink
    /// directory) surface as `AlertLevel::Warn` Alerts and are skipped — the
    /// unresolved placeholder stays in the flattened user message so the LLM
    /// still sees the intent.
    async fn resolve_file_refs(&self, segments: &[Segment]) -> Vec<SystemItem> {
        let Some(workdir) = self.workdir_session.clone() else {
            for seg in segments {
                if let Segment::FileRef { path } = seg {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Worker,
                        format!("file ref @{path} could not be resolved: Worker has no local filesystem authority"),
                    );
                }
            }
            return Vec::new();
        };
        let view = crate::fs_view::WorkerFsView::new(workdir);
        let mut out = Vec::new();
        for seg in segments {
            let Segment::FileRef { path } = seg else {
                continue;
            };
            match view
                .resolve_file_ref(path, self.manifest.engine.file_upload.max_bytes)
                .await
            {
                Ok(item) => {
                    // `resolve_file_ref` returns an `Item::system_message`
                    // whose text already carries the `[File: <path>]` or
                    // `[Dir: <path>]` header (plus any truncation hint).
                    // Persist that body verbatim — it is what the LLM
                    // actually saw, so resume produces byte-identical
                    // history.
                    let body = item.as_text().unwrap_or_default().to_string();
                    out.push(SystemItem::FileAttachment {
                        path: path.clone(),
                        body,
                    });
                }
                Err(e) => {
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Worker,
                        format!("file ref @{path} could not be resolved: {e}"),
                    );
                }
            }
        }
        out
    }

    /// Close interrupted history before a fresh user/notification run.
    ///
    /// Clearing the interrupted flag also ends the old logical-run budget and
    /// must happen before `prepare_for_run`: proactive compaction checkpoints
    /// only resumable runs, never the run this invocation is abandoning.
    fn prepare_interrupted_history_for_fresh_run(&mut self) -> Result<(), WorkerError> {
        if self.last_run_interrupted {
            self.apply_interrupt_prep()?;
            self.last_run_interrupted = false;
            self.engine_mut().set_active_run_turn_count(None);
        }
        Ok(())
    }

    /// Durably close every unanswered ToolCall before the interrupted run's
    /// final lifecycle record/status is published.
    fn terminalize_orphan_tool_calls(&mut self) -> Result<(), WorkerError> {
        let tool_result_summary = self
            .prompts()
            .load_full()
            .interrupt_tool_result_summary()
            .map_err(WorkerError::from)?;
        let history_items = self.history();
        let closures = crate::interrupt_prep::orphan_tool_result_closures(
            &history_items,
            &tool_result_summary,
        );
        if closures.is_empty() {
            return Ok(());
        }

        let subject = worker_subject(self.session.session_id());
        for item in closures {
            let entry = HistoryEntry::new(
                item,
                new_history_metadata(
                    WorkerHistoryProvenance::ToolOutput {
                        worker: subject.clone(),
                    },
                    None,
                ),
            );
            self.commit_entry(LogEntry::AnnotatedToolResult {
                ts: segment_log::now_millis(),
                entry: to_logged_history_entry(&entry),
            })?;
            self.session.history_mut().push_entry(entry);
            self.session.note_mutation();
        }
        Ok(())
    }

    fn apply_interrupt_prep(&mut self) -> Result<(), WorkerError> {
        self.terminalize_orphan_tool_calls()?;
        let system_note = self
            .prompts()
            .load_full()
            .interrupt_system_note()
            .map_err(WorkerError::from)?;

        let interrupt_prompt_provenance =
            self.prompt_render_provenance("internal.interrupt_system_note");
        let interrupt_metadata = new_history_metadata(
            WorkerHistoryProvenance::BackendInstruction { operation_id: None },
            None,
        );
        self.commit_entry(LogEntry::AnnotatedSystemItem {
            ts: segment_log::now_millis(),
            entry: session_store::LoggedSystemHistoryEntry {
                item: SystemItem::Interrupt {
                    body: system_note.clone(),
                    prompt_provenance: Some(interrupt_prompt_provenance),
                },
                metadata: interrupt_metadata.clone(),
            },
        })?;
        let interrupt_entry =
            HistoryEntry::new(agen::Item::system_message(system_note), interrupt_metadata);
        self.session.history_mut().push_entry(interrupt_entry);
        self.session.note_mutation();
        Ok(())
    }

    /// Abandon a paused/interrupted turn without resuming it.
    ///
    /// This uses the same explicit interrupt preparation as the next fresh
    /// `run` would have used, then clears the worker's interrupted marker so
    /// future input is treated as a normal new turn instead of a resume.
    /// The explicit `PausedTurnAbandoned` marker preserves durable lifecycle
    /// semantics without claiming another `run` / `resume` completed.
    pub fn cancel_paused_turn(&mut self) -> Result<(), WorkerError> {
        if !self.last_run_interrupted {
            return Ok(());
        }

        self.apply_interrupt_prep()?;
        self.last_run_interrupted = false;
        self.commit_entry(LogEntry::PausedTurnAbandoned {
            ts: segment_log::now_millis(),
        })?;
        Ok(())
    }

    fn projected_input_history(
        &self,
        input: &[Segment],
        flow_projection: Option<&PreparedFlowProjection>,
    ) -> Vec<HistoryEntry<SessionHistoryMetadata>> {
        if let Some(flow) = flow_projection {
            return input
                .iter()
                .map(|segment| match segment {
                    Segment::Flow { .. } => history_entry(
                        Item::user_message(flow.instructions.clone()),
                        WorkerHistoryProvenance::FlowInstruction {
                            selector: flow.selector.clone(),
                            definition_id: flow.definition_id.clone(),
                            definition_revision: flow.definition_revision,
                            instance_id: flow.instance_id.clone(),
                            state_id: flow.state_id.clone(),
                        },
                    ),
                    other => history_entry(
                        Item::user_message(Segment::flatten_to_text(std::slice::from_ref(other))),
                        // Current public submit transport does not carry a
                        // trusted account/Worker subject envelope. Fail closed
                        // instead of promoting role=user to HumanInput.
                        WorkerHistoryProvenance::LegacyUnknown,
                    ),
                })
                .collect();
        }

        vec![history_entry(
            Item::user_message(Segment::flatten_to_text(input)),
            WorkerHistoryProvenance::LegacyUnknown,
        )]
    }

    /// Run a turn triggered by `Method::Notify` while the Worker is idle.
    ///
    /// Unlike [`run`](Self::run), no user message is appended to
    /// history. The `WorkerInterceptor::pre_llm_request` drains the
    /// pending-notification buffer and injects each entry as an
    /// `Item::system_message` into the per-request context, then the
    /// Engine's resume path issues the LLM request without a new
    /// user turn.
    pub async fn run_for_notification(
        &mut self,
        kind: protocol::InvokeKind,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        debug_assert!(
            matches!(
                kind,
                protocol::InvokeKind::Notify
                    | protocol::InvokeKind::WorkerEvent
                    | protocol::InvokeKind::SystemReminder
                    | protocol::InvokeKind::Wakeup
            ),
            "run_for_notification expects a non-UserSend InvokeKind; got {kind:?}"
        );
        self.prepare_interrupted_history_for_fresh_run()?;
        self.prepare_for_run().await?;

        // IDLE → active marker for the buffered notification / worker-event
        // drain. The trailing SystemItem entries (drained by the
        // WorkerInterceptor) carry the actual payload.
        self.commit_entry(LogEntry::Invoke {
            ts: segment_log::now_millis(),
            trigger: kind,
        })?;

        let history_before = self.session.history().len();
        let mut annotate = history_annotator(
            self.log_writer_handle(),
            Vec::new(),
            self.pending_committed_history.clone(),
        );
        let worker = self.engine.take().expect("worker taken during run");
        let mut locked = worker.lock(self.session.history());
        let result = locked
            .resume_with_annotation(self.session.history_mut(), &mut annotate)
            .await;
        self.engine = Some(locked.unlock());
        self.session.note_mutation();

        self.handle_worker_result(result, history_before).await
    }

    /// Resume from a paused state.
    pub async fn resume(&mut self) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.prepare_for_run().await?;

        let history_before = self.session.history().len();
        let mut annotate = history_annotator(
            self.log_writer_handle(),
            Vec::new(),
            self.pending_committed_history.clone(),
        );
        let worker = self.engine.take().expect("worker taken during run");
        let mut locked = worker.lock(self.session.history());
        let result = locked
            .resume_with_annotation(self.session.history_mut(), &mut annotate)
            .await;
        self.engine = Some(locked.unlock());
        self.session.note_mutation();

        self.handle_worker_result(result, history_before).await
    }

    /// Ensure the session exists and the writer's tally still matches
    /// the on-disk entry count.
    ///
    /// On the first call for a Worker built via `from_manifest`, the session
    /// has not been written to the store yet — this is when we append the
    /// initial `SegmentStart` entry, carrying the system prompt that
    /// `ensure_system_prompt_materialized` has just rendered. Subsequent
    /// calls fall through to entry-count comparison, which auto-forks
    /// when another writer has appended behind our back.
    fn ensure_segment_head(&mut self) -> Result<(), WorkerError> {
        let w = self.engine.as_ref().unwrap();
        let loc = self.segment_state.location();
        let entries_written = self.segment_state.entries_written();
        if entries_written == 0 {
            let initial = LogEntry::AnnotatedSegmentStart {
                ts: segment_log::now_millis(),
                session_id: loc.session_id,
                system_prompt: w.get_system_prompt().map(String::from),
                config: w.request_config().clone(),
                history: self
                    .session
                    .history()
                    .entries()
                    .iter()
                    .map(to_logged_history_entry)
                    .collect(),
                forked_from: None,
                compacted_from: None,
            };
            self.commit_entry(initial)?;
            self.write_worker_metadata_active(loc)?;
            return Ok(());
        }
        // Check store count + auto-fork if it drifted.
        let store_count = self
            .store
            .read_entry_count(loc.session_id, loc.segment_id)
            .map_err(WorkerError::from)?;
        if store_count == entries_written {
            return Ok(());
        }
        // Auto-fork within the same Session: mint a fresh Segment and
        // switch to it. The source segment is left immutable (no terminal
        // marker is written back); the fork relationship is recorded
        // forward on the new segment's `forked_from`, with `at_turn_index`
        // = the writer's current turn (its in-memory history reflects
        // state up to that turn). The new SegmentStart replaces the mirror
        // and is broadcast through the sink so existing subscribers reset
        // their view.
        let fork_segment_id = session_store::new_segment_id();
        let entry = LogEntry::AnnotatedSegmentStart {
            ts: segment_log::now_millis(),
            session_id: loc.session_id,
            system_prompt: w.get_system_prompt().map(String::from),
            config: w.request_config().clone(),
            history: self
                .session
                .history()
                .entries()
                .iter()
                .map(to_logged_history_entry)
                .collect(),
            forked_from: Some(session_store::SegmentOrigin {
                segment_id: loc.segment_id,
                at_turn_index: w.turn_count(),
            }),
            compacted_from: None,
        };
        let mut initial_entries = vec![entry.clone()];
        if let Some(checkpoint) =
            active_run_checkpoint_entry(w.active_run_turn_count(), w.turn_count())
        {
            initial_entries.push(checkpoint);
        }
        self.store
            .create_segment(loc.session_id, fork_segment_id, &initial_entries)
            .map_err(WorkerError::from)?;
        self.segment_state.set_location(SegmentLocation {
            session_id: loc.session_id,
            segment_id: fork_segment_id,
        });
        self.segment_state
            .set_entries_written(initial_entries.len());
        self.sink
            .reset_with_initial_entries(initial_entries.clone());
        if self.scope_allocation.is_some() {
            worker_allocation::update_segment(&self.manifest.worker.name, fork_segment_id)?;
        }
        self.write_worker_metadata_active(SegmentLocation {
            session_id: loc.session_id,
            segment_id: fork_segment_id,
        })?;
        Ok(())
    }

    /// Handle Engine result: always persist the turn first, then if
    /// `Yielded`, perform compaction and resume.
    ///
    /// Persisting before compaction ensures that if compact fails, the
    /// turn is fully recorded in the old session (interrupted, outcome
    /// `Yielded`), so restore remains consistent.
    async fn handle_worker_result(
        &mut self,
        result: EngineRunExit,
        history_before: usize,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        if matches!(&result, EngineRunExit::Interrupted(_)) {
            self.terminalize_orphan_tool_calls()?;
        }
        self.persist_turn(history_before, &result).await?;

        if matches!(result, EngineRunExit::Yielded) {
            self.last_run_interrupted = true;
            return self.do_compact_and_resume().await;
        }

        if !matches!(result, EngineRunExit::Interrupted(_)) {
            if let Some(ref state) = self.compact_state {
                state.set_just_compacted(false);
            }
        }

        match result {
            EngineRunExit::Finished => {
                self.last_run_interrupted = false;
                Ok(WorkerRunResult::Finished)
            }
            EngineRunExit::Paused => {
                self.last_run_interrupted = true;
                Ok(WorkerRunResult::Paused)
            }
            EngineRunExit::Interrupted(StopReason::LimitReached) => {
                self.last_run_interrupted = false;
                Ok(WorkerRunResult::LimitReached)
            }
            EngineRunExit::Interrupted(reason) => {
                self.last_run_interrupted = true;
                Ok(WorkerRunResult::Interrupted {
                    code: stop_reason_error_code(&reason),
                    message: stop_reason_message(&reason),
                })
            }
            EngineRunExit::Yielded => unreachable!("yielded handled above"),
        }
    }

    fn persist_compaction_lifecycle(
        &mut self,
        lifecycle: &CompactionLifecycle,
    ) -> Result<(), WorkerError> {
        Ok(self.commit_entry(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: COMPACTION_EXTENSION_DOMAIN.into(),
            payload: serde_json::to_value(lifecycle).map_err(|error| {
                WorkerError::InvalidState(format!(
                    "serialize compaction lifecycle {}: {error}",
                    lifecycle.compaction_id
                ))
            })?,
        })?)
    }

    fn persist_and_send_compact_start(
        &mut self,
        lifecycle: CompactionLifecycle,
    ) -> Result<(), WorkerError> {
        self.persist_compaction_lifecycle(&lifecycle)?;
        self.send_event(Event::CompactStart { lifecycle });
        Ok(())
    }

    fn persist_and_send_compact_done(
        &mut self,
        lifecycle: CompactionLifecycle,
    ) -> Result<(), WorkerError> {
        self.persist_compaction_lifecycle(&lifecycle)?;
        self.send_event(Event::CompactDone { lifecycle });
        Ok(())
    }

    fn persist_and_send_compact_failed(
        &mut self,
        lifecycle: CompactionLifecycle,
    ) -> Result<(), WorkerError> {
        self.persist_compaction_lifecycle(&lifecycle)?;
        self.send_event(Event::CompactFailed { lifecycle });
        Ok(())
    }

    /// Perform compaction after a `compact_needed` abort and resume execution.
    ///
    /// Uses `Box::pin` for the recursive `resume()` call to break the
    /// async layout cycle (`run → handle_worker_result → do_compact_and_resume → resume`).
    fn do_compact_and_resume(
        &mut self,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<WorkerRunResult, WorkerError>> + Send + '_>,
    >
    where
        St: Clone + 'static,
    {
        Box::pin(async move {
            // Thrash detection: if we just compacted and hit the threshold again,
            // something is wrong.
            if let Some(ref state) = self.compact_state {
                if state.just_compacted() {
                    state.set_just_compacted(false);
                    return Err(WorkerError::CompactThrash);
                }
            }

            let retained = self
                .compact_state
                .as_ref()
                .map(|s| s.retained_tokens())
                .unwrap_or(manifest::defaults::COMPACT_RETAINED_TOKENS);

            match self.compact(retained).await {
                Ok(new_segment_id) => {
                    info!(
                        new_segment_id = %new_segment_id,
                        "Compaction succeeded, resuming execution"
                    );
                    if let Some(ref state) = self.compact_state {
                        state.record_compact_success();
                    }
                    self.resume().await
                }
                Err(e) => {
                    warn!(error = %e, "Compaction failed during run");
                    self.alert(
                        AlertLevel::Error,
                        AlertSource::Compactor,
                        format!("mid-run compaction failed: {e}"),
                    );
                    if let Some(ref state) = self.compact_state {
                        state.record_compact_failure();
                    }
                    Err(e)
                }
            }
        })
    }

    /// Attempt proactive compaction at the beginning of a controller Run.
    ///
    /// This used to run in the controller's post-run path. Keeping it here
    /// preserves the ordering requirement that the next turn starts with a
    /// compacted history, without introducing a separate Busy controller state.
    /// Best-effort: failures are logged and surfaced, but do not abort the
    /// user turn that triggered the check.
    pub async fn try_pre_run_compact(&mut self) {
        let state = match self.compact_state.as_ref() {
            Some(s) if !s.is_disabled() && !s.just_compacted() => s.clone(),
            _ => return,
        };
        let current_tokens = self.total_tokens().tokens;
        if !state.exceeds_post_run(current_tokens) {
            return;
        }

        let retained = state.retained_tokens();
        match self.compact(retained).await {
            Ok(new_segment_id) => {
                info!(
                    new_segment_id = %new_segment_id,
                    "Proactive pre-run compaction succeeded"
                );
                state.record_compact_success();
            }
            Err(e) => {
                warn!(error = %e, "Proactive pre-run compaction failed");
                self.alert(
                    AlertLevel::Warn,
                    AlertSource::Compactor,
                    format!("pre-run compaction failed: {e}"),
                );
                state.record_compact_failure();
            }
        }
    }

    /// Run an explicit user-requested compaction between turns.
    ///
    /// The controller only calls this while Idle. Paused turns keep their
    /// interrupted Engine state intact and are intentionally rejected before
    /// this method is reached.
    pub async fn manual_compact(&mut self) -> Result<ManualCompactResult, WorkerError> {
        if self.manifest.compaction.is_none() {
            let message =
                "manual compact is unavailable because [compaction] is not configured".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        if self.history().is_empty() {
            let message = "manual compact skipped: no conversation history to compact".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        self.ensure_interceptor_installed();
        self.cleanup_finished_memory_task();
        self.ensure_segment_head()?;

        let state = self.compact_state.clone();
        if state.as_ref().is_some_and(|s| s.is_disabled()) {
            let message =
                "manual compact is disabled after repeated compaction failures".to_string();
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        let retained = state
            .as_ref()
            .map(|s| s.retained_tokens())
            .or_else(|| self.manifest.compaction.as_ref().map(|c| c.retained_tokens))
            .unwrap_or(manifest::defaults::COMPACT_RETAINED_TOKENS);
        let current_tokens = self.total_tokens().tokens;
        let cut = self.split_for_retained(retained);
        if cut.index == 0 {
            let message = format!(
                "manual compact skipped: current context is within the retained tail ({current_tokens} <= {retained} tokens)"
            );
            self.alert(AlertLevel::Warn, AlertSource::Compactor, message.clone());
            return Ok(ManualCompactResult::Skipped { message });
        }

        self.join_memory_task().await;
        match self.compact(retained).await {
            Ok(new_segment_id) => {
                info!(new_segment_id = %new_segment_id, "Manual compaction succeeded");
                if let Some(ref state) = state {
                    state.record_compact_success();
                }
                Ok(ManualCompactResult::Compacted { new_segment_id })
            }
            Err(e) => {
                warn!(error = %e, "Manual compaction failed");
                self.alert(
                    AlertLevel::Error,
                    AlertSource::Compactor,
                    format!("manual compaction failed: {e}"),
                );
                if let Some(ref state) = state {
                    state.record_compact_failure();
                }
                Err(e)
            }
        }
    }

    /// Persist delta + turn end + outcome after a run/resume.
    async fn persist_turn(
        &mut self,
        history_before: usize,
        result: &EngineRunExit,
    ) -> Result<(), StoreError> {
        // Per-item commits for AssistantItem / ToolResult / SystemItem
        // entries are expected to have landed synchronously: the
        // worker `on_history_append` callback (wired by the controller
        // via `wire_history_persistence`) commits each appended item
        // directly through the writer, and the interceptor commits
        // SystemItem entries up-front in `on_prompt_submit` /
        // `pending_history_appends` before returning the matching
        // `Item::system_message`s.
        //
        // Low-level test paths that build `Worker::new` without wiring
        // the callback fall through this branch: they classify the
        // slice from `history_before` inline so the test's
        // `restore`-style assertions still see entries on disk.
        if !self.history_persistence_wired {
            let new_items: Vec<Item> = self.session.history().entries()[history_before..]
                .iter()
                .map(|entry| entry.item.clone())
                .collect();
            let ts = segment_log::now_millis();
            for item in &new_items {
                if item.is_user_message() {
                    continue;
                }
                if matches!(
                    item,
                    Item::Message {
                        role: agen::Role::System,
                        ..
                    }
                ) {
                    continue;
                }
                let entry = session_store::classify_history_item(item, ts);
                self.commit_entry(entry)?;
            }
        }

        let turn_count = self.engine.as_ref().unwrap().turn_count();
        self.commit_entry(LogEntry::TurnEnd {
            ts: segment_log::now_millis(),
            turn_count,
        })?;

        // Flush any sync-buffered metrics from this run first
        // (currently `prune.fire` / `prune.skip` from the prune observer).
        // Ordered before LlmUsage so that a `prune.fire` and the
        // `prune.post_request` derived from the matching usage record
        // appear in the log close together.
        //
        // Metric writes are intentionally non-fatal: a failure here
        // surfaces as a `Warn` alert + `tracing::warn!` and the loop
        // continues. Metrics are observability data, not load-bearing
        // for run correctness, so a transient FS error must not poison
        // the turn record (`save_delta` / `save_turn_end` already landed
        // by this point, and `save_run_completed` still needs to land).
        let pending_metrics = self.metrics_tracker.drain();
        for metric in pending_metrics {
            self.try_record_metric(&metric);
        }

        // Persist any LLM Usage measurements collected during this run.
        // One LogEntry::LlmUsage per LLM call (the tool loop may have run
        // many calls within a single Worker::run). Each is also appended to
        // the in-memory `usage_history` so token-accounting APIs see it
        // before the next run. Records carrying a `correlation_id` (set
        // by an upstream observer such as the prune projection) also get
        // a paired `prune.post_request` metric so cache_read/write can be
        // joined back to the originating event.
        let usage_records = self.usage_tracker.drain();
        for recorded in usage_records {
            let crate::compact::usage_tracker::RecordedUsage {
                record,
                correlation_id,
            } = recorded;
            self.commit_entry(LogEntry::LlmUsage {
                ts: segment_log::now_millis(),
                history_len: record.history_len,
                input_total_tokens: record.input_total_tokens,
                cache_read_tokens: record.cache_read_tokens,
                cache_write_tokens: record.cache_write_tokens,
                output_tokens: record.output_tokens,
            })?;
            if let Some(id) = correlation_id {
                let metric = session_metrics::Metric::now("prune.post_request")
                    .with_correlation_id(&id)
                    .with_value(record.cache_read_tokens as f64)
                    .with_dimension("cache_write_tokens", record.cache_write_tokens.to_string())
                    .with_dimension("history_len", record.history_len.to_string());
                self.try_record_metric(&metric);
            }
            self.usage_history
                .lock()
                .expect("usage_history poisoned")
                .push(record);
        }

        let interrupted = matches!(
            result,
            EngineRunExit::Paused
                | EngineRunExit::Yielded
                | EngineRunExit::Interrupted(StopReason::Cancelled)
                | EngineRunExit::Interrupted(StopReason::ContextWindowExceeded)
                | EngineRunExit::Interrupted(StopReason::Unexpected(_))
        );
        let active_run_turn_count = self.engine.as_ref().unwrap().active_run_turn_count();
        match result {
            EngineRunExit::Finished | EngineRunExit::Paused | EngineRunExit::Yielded => {
                let result = match result {
                    EngineRunExit::Finished => EngineResult::Finished,
                    EngineRunExit::Paused => EngineResult::Paused,
                    EngineRunExit::Yielded => EngineResult::Yielded,
                    EngineRunExit::Interrupted(_) => unreachable!(),
                };
                self.commit_entry(LogEntry::RunCompleted {
                    ts: segment_log::now_millis(),
                    interrupted,
                    result,
                    active_run_turn_count,
                })?;
            }
            EngineRunExit::Interrupted(StopReason::LimitReached) => {
                self.commit_entry(LogEntry::RunCompleted {
                    ts: segment_log::now_millis(),
                    interrupted: false,
                    result: EngineResult::LimitReached,
                    active_run_turn_count,
                })?;
            }
            EngineRunExit::Interrupted(reason) => {
                self.commit_entry(LogEntry::RunErrored {
                    ts: segment_log::now_millis(),
                    interrupted,
                    message: stop_reason_message(reason),
                })?;
            }
        }

        Ok(())
    }

    /// Runs one parent-owned observable compaction service and returns the new
    /// Segment ID. Lifecycle revisions are committed before they are broadcast.
    pub async fn compact(&mut self, retained_tokens: u64) -> Result<SegmentId, WorkerError> {
        let mut lifecycle = CompactionLifecycle {
            schema_version: 2,
            compaction_id: uuid::Uuid::now_v7().to_string(),
            revision: 1,
            internal_worker: None,
            state: CompactionLifecycleState::Running,
            started_at_ms: segment_log::now_millis(),
            ended_at_ms: None,
            summary: None,
            error: None,
            new_segment_id: None,
        };
        self.persist_and_send_compact_start(lifecycle.clone())?;
        match self.compact_impl(retained_tokens, &mut lifecycle).await {
            Ok((new_segment_id, summary)) => {
                lifecycle.revision = lifecycle.revision.saturating_add(1);
                lifecycle.state = CompactionLifecycleState::Done;
                lifecycle.ended_at_ms = Some(segment_log::now_millis());
                lifecycle.summary = Some(summary);
                lifecycle.new_segment_id = Some(new_segment_id.to_string());
                let terminal = self.persist_and_send_compact_done(lifecycle.clone());
                self.release_compaction_service(&lifecycle).await;
                terminal?;
                Ok(new_segment_id)
            }
            Err(error) => {
                lifecycle.revision = lifecycle.revision.saturating_add(1);
                lifecycle.state = if matches!(error, WorkerError::CompactCancelled) {
                    CompactionLifecycleState::Interrupted
                } else {
                    CompactionLifecycleState::Failed
                };
                lifecycle.ended_at_ms = Some(segment_log::now_millis());
                lifecycle.error = Some(error.to_string().chars().take(2_000).collect());
                let terminal = self.persist_and_send_compact_failed(lifecycle.clone());
                self.release_compaction_service(&lifecycle).await;
                terminal?;
                Err(error)
            }
        }
    }

    async fn release_compaction_service(&self, lifecycle: &CompactionLifecycle) {
        let Some(session_id) = lifecycle
            .internal_worker
            .as_ref()
            .map(|worker| worker.session_id.as_str())
        else {
            return;
        };
        let Some(registry) = &self.internal_worker_registry else {
            return;
        };
        if let Err(error) = registry.stop_service(session_id).await {
            warn!(
                compaction_id = %lifecycle.compaction_id,
                internal_worker_session_id = %session_id,
                error = %error,
                "failed to release terminal compaction Internal Worker"
            );
        }
    }

    async fn compact_impl(
        &mut self,
        retained_tokens: u64,
        lifecycle: &mut CompactionLifecycle,
    ) -> Result<(SegmentId, String), WorkerError> {
        use crate::compact::worker::{
            CompactWorkerContext, CompactWorkerInterceptor, CompactionOutputFeature,
        };
        use crate::fs_view::WorkerFsView;

        // Decide the cut point by projecting the UsageRecord timeline onto
        // the current history: keep the tail whose estimated token count is
        // within `retained_tokens`. Item-granular, turn boundaries ignored.
        let cut = self.split_for_retained(retained_tokens);

        let history_entries = self.session.history().entries();
        let retain_from = cut.index.min(history_entries.len());
        let retained_history_entries = history_entries[retain_from..].to_vec();
        let retained_items = retained_history_entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let entries_to_summarise = history_entries[..retain_from].to_vec();
        let items_to_summarise = entries_to_summarise
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        // Compaction-related knobs. Fall through to manifest defaults when
        // `[compaction]` is omitted entirely.
        let (
            auto_read_budget,
            worker_context_max_tokens,
            finish_warning_remaining_tokens,
            final_reserve_tokens,
            worker_max_turns,
            overview_target_tokens,
            overview_warning_tokens,
            overview_deadline_tokens,
            summary_target_tokens,
            summary_max_tokens,
            result_context_max_tokens,
        ) = self
            .manifest
            .compaction
            .as_ref()
            .map(|c| {
                (
                    c.auto_read_budget_tokens,
                    c.worker_context_max_tokens,
                    c.finish_warning_remaining_tokens,
                    c.final_reserve_tokens,
                    c.worker_max_turns,
                    c.overview_target_tokens,
                    c.overview_warning_tokens,
                    c.overview_deadline_tokens,
                    c.summary_target_tokens,
                    c.summary_max_tokens,
                    c.result_context_max_tokens,
                )
            })
            .unwrap_or((
                manifest::defaults::COMPACT_AUTO_READ_BUDGET,
                manifest::defaults::COMPACT_WORKER_MAX_INPUT_TOKENS,
                manifest::defaults::COMPACT_FINISH_WARNING_REMAINING_TOKENS,
                manifest::defaults::COMPACT_FINAL_RESERVE_TOKENS,
                manifest::defaults::COMPACT_WORKER_MAX_TURNS,
                manifest::defaults::COMPACT_OVERVIEW_TARGET_TOKENS,
                manifest::defaults::COMPACT_OVERVIEW_WARNING_TOKENS,
                manifest::defaults::COMPACT_OVERVIEW_DEADLINE_TOKENS,
                manifest::defaults::COMPACT_SUMMARY_TARGET_TOKENS,
                manifest::defaults::COMPACT_SUMMARY_MAX_TOKENS,
                manifest::defaults::COMPACT_RESULT_CONTEXT_MAX_TOKENS,
            ));

        // Default references: the N most-recently-touched files in the
        // session, surfaced so the compact worker can inspect them and
        // decide which (if any) the next session needs.
        let default_refs: Vec<PathBuf> = self
            .tracker
            .as_ref()
            .map(|t| t.recent_files(manifest::defaults::COMPACT_DEFAULT_REFERENCE_COUNT))
            .unwrap_or_default();

        // Input text fed to the compact worker. Includes the default
        // references, current TaskStore snapshot, current TaskStore snapshot, and the (pruned) conversation text.
        let task_snapshot_text = self.task_feature.snapshot_text();
        let summary_input = build_summary_input(
            &items_to_summarise,
            &default_refs,
            Some(task_snapshot_text.as_str()),
            SummaryInputOptions {
                overview_target_tokens,
                overview_warning_tokens,
                overview_deadline_tokens,
                summary_target_tokens,
            },
        );
        if summary_input.warning_exceeded {
            self.alert(
                AlertLevel::Warn,
                AlertSource::Compactor,
                format!(
                    "compact overview is larger than expected (≈{} tokens; warning threshold {})",
                    summary_input.overview_tokens, overview_warning_tokens
                ),
            );
        }
        if summary_input.deadline_fallback_used {
            self.alert(
                AlertLevel::Warn,
                AlertSource::Compactor,
                format!(
                    "compact overview exceeded deadline ({} tokens); using coarse fallback",
                    overview_deadline_tokens
                ),
            );
        }

        // Engine-side state collected by the compact worker's tool calls.
        let ctx = Arc::new(std::sync::Mutex::new(CompactWorkerContext::with_budget(
            auto_read_budget,
        )));

        // Build a normal parent-owned Internal Worker over a pinned immutable
        // capture. Only SessionExplore and compaction output tools are installed.
        let workdir = self.workdir_session.clone();
        let read_only_workdir = workdir.clone().map(|session| {
            Arc::new(ReadOnlyWorkdirSession::new(session)) as workdir::WorkdirSessionHandle
        });
        let summary_tracker = tools::Tracker::new();
        let summary_client: Box<dyn LlmClient> = self.build_compactor_client()?;
        let summary_system_prompt = self
            .prompts
            .load_full()
            .compact_system()
            .map_err(WorkerError::PromptCatalog)?;
        let summary_usage_tracker = Arc::new(UsageTracker::new());
        let compactor_warning_cb = self.alerter.clone().map(|alerter| {
            Arc::new(move |message: String| {
                alerter.alert(AlertLevel::Warn, AlertSource::Compactor, message);
            }) as Arc<dyn Fn(String) + Send + Sync>
        });
        let interceptor = CompactWorkerInterceptor::new(
            summary_usage_tracker.clone(),
            worker_context_max_tokens,
            finish_warning_remaining_tokens,
            final_reserve_tokens,
            compactor_warning_cb,
        );
        let tracker_for_engine = summary_usage_tracker.clone();
        let features = crate::feature::FeatureRegistryBuilder::new()
            .with_module(
                crate::feature::builtin::session_explore::SessionExploreFeature::new(
                    crate::feature::builtin::session_explore::SessionExploreState::new(
                        crate::session_capture::SessionCapture::from_history_entries(
                            self.segment_id().to_string(),
                            entries_to_summarise.clone(),
                        ),
                    ),
                ),
            )
            .with_module(CompactionOutputFeature::new(
                read_only_workdir.clone(),
                summary_tracker,
                ctx.clone(),
            ));
        let required_tools: &'static [&'static str] = if read_only_workdir.is_some() {
            &[
                "ShowOverview",
                "SearchEntries",
                "ReadEntry",
                "Read",
                "mark_read_required",
                "add_reference",
                "write_summary",
            ]
        } else {
            &[
                "ShowOverview",
                "SearchEntries",
                "ReadEntry",
                "add_reference",
                "write_summary",
            ]
        };
        let handle = prepare_internal_worker_from_spec(
            InternalWorkerSpec {
                identity: InternalWorkerIdentity {
                    kind: "compaction",
                    run_id: uuid::Uuid::parse_str(&lifecycle.compaction_id).map_err(|error| {
                        WorkerError::InvalidState(format!("invalid compaction id: {error}"))
                    })?,
                },
                manifest: self.manifest.clone(),
                client: summary_client,
                system_prompt: summary_system_prompt,
                input: summary_input.text.clone(),
                cache_key: Some(self.segment_id().to_string()),
                max_turns: worker_max_turns,
                engine_configurator: Some(Box::new(move |engine| {
                    let tracker = tracker_for_engine;
                    engine.on_usage(move |event| {
                        tracker.record_usage(event);
                    });
                    engine.set_interceptor(interceptor);
                })),
                features,
                required_tools,
                authority: InternalWorkerAuthority {
                    workspace: WorkerWorkspaceContext::no_workspace(),
                    filesystem: WorkerFilesystemAuthority::None,
                    scope: Scope::empty(),
                    workdir_session: read_only_workdir,
                },
            },
            InternalWorkerVisibility::ParentClient,
        )
        .await
        .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
        let registry = self
            .internal_worker_registry
            .get_or_insert_with(
                crate::spawn::registry::SpawnedWorkerRegistry::new_for_internal_services,
            )
            .clone();
        let internal_ref = registry
            .attach_service(crate::spawn::registry::InternalServiceWorkerRecord::new(
                "compaction",
                "Compaction",
                handle.clone(),
            ))
            .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
        lifecycle.revision = lifecycle.revision.saturating_add(1);
        lifecycle.internal_worker = Some(internal_ref);
        self.persist_and_send_compact_start(lifecycle.clone())?;

        if let Err(error) = handle.send(summary_input.text).await {
            let _ = registry.remove_service(&handle.session_id_string());
            return Err(WorkerError::InvalidState(error.to_string()));
        }
        match handle.wait_until_idle().await {
            crate::internal_worker::InternalWorkerSessionStatus::Idle => {}
            crate::internal_worker::InternalWorkerSessionStatus::Stopped => {
                return Err(WorkerError::CompactCancelled);
            }
            crate::internal_worker::InternalWorkerSessionStatus::Failed => {
                return Err(WorkerError::InvalidState(
                    handle
                        .protocol_snapshot()
                        .error
                        .unwrap_or_else(|| "compactor Internal Worker failed".into()),
                ));
            }
            status => {
                return Err(WorkerError::InvalidState(format!(
                    "compactor Internal Worker ended in {status:?}"
                )));
            }
        }

        // Guard: nudge the worker once more if the expected outputs
        // (summary, and any auto-read nominations when default refs
        // existed) were not produced on the first pass. `write_summary`
        // is idempotent-by-overwrite so a second call is safe.
        let nudge = {
            let snapshot = ctx.lock().expect("compact ctx poisoned").clone();
            if snapshot.summary.is_none() {
                Some(
                    "You have not called `write_summary` yet. Deliver the structured \
                     summary now (Completed Tasks / Active Task / Key Decisions / \
                     User Directives / Current Work) and nominate any files the next \
                     session needs with `mark_read_required`."
                        .to_string(),
                )
            } else if snapshot.read_required.is_empty() && !default_refs.is_empty() {
                Some(
                    "Summary received. If any of the referenced files are required \
                     for the next session to continue the task, call \
                     `mark_read_required` on them now. Otherwise reply briefly to \
                     close out."
                        .to_string(),
                )
            } else {
                None
            }
        };
        if let Some(prompt) = nudge {
            handle
                .send(prompt)
                .await
                .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
            match handle.wait_until_idle().await {
                crate::internal_worker::InternalWorkerSessionStatus::Idle => {}
                crate::internal_worker::InternalWorkerSessionStatus::Stopped => {
                    return Err(WorkerError::CompactCancelled);
                }
                _ => {
                    return Err(WorkerError::InvalidState(
                        handle
                            .protocol_snapshot()
                            .error
                            .unwrap_or_else(|| "compactor Internal Worker failed".into()),
                    ));
                }
            }
        }

        let mut final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
        let mut summary_text = final_ctx
            .summary
            .clone()
            .ok_or(WorkerError::CompactSummaryMissing)?;
        let mut summary_tokens = estimate_text_tokens(summary_text.len());
        if summary_max_tokens > 0 && summary_tokens > summary_max_tokens {
            let prompt = format!(
                "Your `write_summary` output is too large (≈{summary_tokens} tokens; max \
                 {summary_max_tokens}). Rewrite it now with `write_summary`, preserving the \
                 same five sections but making it concise. Target ≈{summary_target_tokens} tokens."
            );
            handle
                .send(prompt)
                .await
                .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
            match handle.wait_until_idle().await {
                crate::internal_worker::InternalWorkerSessionStatus::Idle => {}
                crate::internal_worker::InternalWorkerSessionStatus::Stopped => {
                    return Err(WorkerError::CompactCancelled);
                }
                _ => {
                    return Err(WorkerError::InvalidState(
                        handle
                            .protocol_snapshot()
                            .error
                            .unwrap_or_else(|| "compactor Internal Worker failed".into()),
                    ));
                }
            }
            final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
            summary_text = final_ctx
                .summary
                .clone()
                .ok_or(WorkerError::CompactSummaryMissing)?;
            summary_tokens = estimate_text_tokens(summary_text.len());
            if summary_tokens > summary_max_tokens {
                return Err(WorkerError::CompactSummaryTooLarge {
                    tokens: summary_tokens,
                    max: summary_max_tokens,
                });
            }
        }

        // Re-read each auto-read target via the Worker FS view. Errors are
        // logged and skipped inside `render_auto_read` rather than
        // aborting compaction — a missing / moved file should not fail
        // the whole compact.
        let auto_read_messages = if let Some(workdir) = workdir {
            WorkerFsView::new(workdir)
                .render_auto_read(&final_ctx.read_required)
                .await
        } else {
            Vec::new()
        };

        // Reference list as a single system message; omitted when empty.
        let reference_message = (!final_ctx.references.is_empty()).then(|| {
            let list = final_ctx
                .references
                .iter()
                .map(|p| format!("- {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Item::system_message(format!(
                "[Referenced files — read before compaction, contents not included]\n\
                 {list}\n\
                 Use read_file to access current contents if needed."
            ))
        });

        // Count surviving user_messages before consuming `retained_items`
        // — needed to align `self.user_segments` after the swap below.
        let retained_user_msgs = retained_items
            .iter()
            .filter(|i| i.is_user_message())
            .count();

        // Build new history: [summary, ...auto-read, references, ...retained, task snapshot, TaskList synthetic call/result].
        // The TaskStore snapshot trails the retained items so that, on resume,
        // `replay_history` walks any pre-compact Task* calls preserved verbatim
        // in retained_items first and the trailing snapshot's `replace_with`
        // is the final word — pre-compact `TaskCreate` calls cannot leak as
        // duplicate entries.
        let mut new_history = Vec::with_capacity(
            1 + auto_read_messages.len()
                + 3
                + reference_message.is_some() as usize
                + retained_items.len(),
        );
        let mut compact_introduced_system_messages =
            Vec::with_capacity(2 + auto_read_messages.len() + reference_message.is_some() as usize);
        let summary_message =
            Item::system_message(format!("[Compacted context summary]\n\n{summary_text}"));
        compact_introduced_system_messages.push(summary_message.clone());
        compact_introduced_system_messages.extend(auto_read_messages.iter().cloned());
        if let Some(msg) = reference_message.as_ref() {
            compact_introduced_system_messages.push(msg.clone());
        }
        let task_snapshot_message = Item::system_message(format!(
            "[Session TaskStore snapshot]\n\n{task_snapshot_text}\n\n\
             This is the retained session task list preserved across compaction. \
             The following TaskList tool result presents the same state through the tool lane."
        ));
        compact_introduced_system_messages.push(task_snapshot_message.clone());

        new_history.push(summary_message);
        new_history.extend(auto_read_messages);
        if let Some(msg) = reference_message {
            new_history.push(msg);
        }
        new_history.extend(retained_items);
        new_history.push(task_snapshot_message);
        new_history.push(Item::tool_call("compact-tasklist", "TaskList", "{}"));
        new_history.push(Item::tool_result_with_content(
            "compact-tasklist",
            self.task_feature.snapshot_overview(),
            task_snapshot_text.clone(),
        ));
        let result_estimate = agen::token_counter::total_tokens(&new_history, &[]);
        if result_context_max_tokens > 0 && result_estimate.tokens > result_context_max_tokens {
            return Err(WorkerError::CompactResultContextTooLarge {
                tokens: result_estimate.tokens,
                max: result_context_max_tokens,
            });
        }
        let original_entries = self.session.history().entries();
        let derived_sources = original_entries
            .iter()
            .map(|entry| entry.annotation.entry_id.clone())
            .collect::<Vec<_>>();
        let mut original_cursor = 0usize;
        let compacted_history_entries = new_history
            .iter()
            .cloned()
            .map(|item| {
                if let Some((offset, original)) = original_entries[original_cursor..]
                    .iter()
                    .enumerate()
                    .find(|(_, original)| original.item == item)
                {
                    original_cursor += offset + 1;
                    HistoryEntry::new(item, original.annotation.clone())
                } else {
                    HistoryEntry::new(
                        item,
                        new_history_metadata(
                            WorkerHistoryProvenance::DerivedSummary,
                            Some(SessionHistoryDerivation {
                                sources: derived_sources.clone(),
                            }),
                        ),
                    )
                }
            })
            .collect::<Vec<_>>();

        // Build the SegmentStart entry for the new compacted segment.
        // Inherits the source Segment's session_id so the compacted
        // lineage stays grouped under the same Session. Atomically
        // rotate: create on disk, swap location, reset the broadcast
        // sink so existing subscribers see the new `SegmentStart
        // { compacted_from }` and reset their view.
        let new_segment_id = session_store::new_segment_id();
        let old_loc = self.segment_state.location();
        let source_turn_count = self.engine.as_ref().unwrap().turn_count();
        let w = self.engine.as_ref().unwrap();
        let entry = LogEntry::AnnotatedSegmentStart {
            ts: segment_log::now_millis(),
            session_id: old_loc.session_id,
            system_prompt: w.get_system_prompt().map(String::from),
            config: w.request_config().clone(),
            history: compacted_history_entries
                .iter()
                .map(to_logged_history_entry)
                .collect(),
            forked_from: None,
            compacted_from: Some(session_store::SegmentOrigin {
                segment_id: old_loc.segment_id,
                at_turn_index: source_turn_count,
            }),
        };
        let mut initial_entries = vec![entry.clone()];
        if let Some(checkpoint) =
            active_run_checkpoint_entry(w.active_run_turn_count(), source_turn_count)
        {
            initial_entries.push(checkpoint);
        }
        if let Some(flow_state) = self
            .flow_runtime_state
            .lock()
            .expect("flow_runtime_state poisoned")
            .as_ref()
        {
            initial_entries.push(LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: FLOW_RUNTIME_EXTENSION_DOMAIN.to_string(),
                payload: serde_json::to_value(flow_state).map_err(|error| {
                    WorkerError::InvalidState(format!(
                        "serialize Flow runtime state during compaction: {error}"
                    ))
                })?,
            });
        }
        self.store
            .create_segment(old_loc.session_id, new_segment_id, &initial_entries)?;
        self.segment_state.set_location(SegmentLocation {
            session_id: old_loc.session_id,
            segment_id: new_segment_id,
        });
        self.segment_state
            .set_entries_written(initial_entries.len());
        // Broadcast the complete compacted prefix. Runtime-owned extensions
        // must remain visible and restorable with the replacement segment.
        self.sink
            .reset_with_initial_entries(initial_entries.clone());
        // Keep workers.json pointing at the live segment_id. Without this
        // a concurrent `restore_from_manifest(new_segment_id)` would
        // see no live writer and grab the session this Worker just moved
        // into, causing two writers to race on the same jsonl. Skipped
        // when no allocation is installed (e.g. compact under
        // `Worker::new` in tests).
        if self.scope_allocation.is_some() {
            worker_allocation::update_segment(&self.manifest.worker.name, new_segment_id)?;
        }
        self.write_worker_metadata_active(SegmentLocation {
            session_id: old_loc.session_id,
            segment_id: new_segment_id,
        })?;
        // Align user_segments with the post-compaction history. Items
        // before `retain_from` (now folded into the summary) lose their
        // segments; only the user_messages surviving in retained_items
        // keep them. They are always the trailing K entries of
        // `self.user_segments` because submissions are appended in order.
        let drop_n = self.user_segments.len().saturating_sub(retained_user_msgs);
        if drop_n > 0 {
            self.user_segments.drain(..drop_n);
        }

        self.session.replace_history(compacted_history_entries);
        // Compaction-introduced system messages are part of the new
        // SegmentStart's history (broadcast above) — clients derive
        // their blocks from `SegmentStart.history`. No per-item
        // broadcast is required.
        let _ = &compact_introduced_system_messages;
        let worker = self.engine.as_mut().unwrap();
        // Anchor the prompt cache at the summary item so that Anthropic
        // can place a durable `cache_control` breakpoint there — our
        // compact layout guarantees history[0] is the summary.
        worker.set_cache_anchor(Some(0));
        // Re-key the OpenAI Responses prompt cache namespace to the new
        // segment_id so post-compact turns share a key with extract /
        // consolidate workers running in the same session.
        worker.set_cache_key(Some(new_segment_id.to_string()));
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clear();
        // Reset extract pointer alongside usage_history: the compacted
        // session has a fresh log with no `LogEntry::Extension` entries
        // yet, so a cold restore here would set extract_pointer to None
        // via fold_pointer. The in-memory pointer must match — otherwise
        // `tokens_added_since(old_history_len)` would treat the new
        // (shorter) history as if it had already been processed, and
        // extract would stop firing for the rest of the process's
        // lifetime.
        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = None;

        Ok((new_segment_id, summary_text))
    }

    /// Build the LlmClient for the compactor Engine.
    ///
    /// Uses `compaction.model` from manifest if set, otherwise clones
    /// the main client.
    fn build_compactor_client(&self) -> Result<Box<dyn LlmClient>, WorkerError> {
        if let Some(ref compaction) = self.manifest.compaction {
            if let Some(ref model_config) = compaction.model {
                let client = crate::model_client::build_client(model_config)?;
                return Ok(client);
            }
        }
        let worker = self.engine.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }

    /// Build the LlmClient for the extract (memory.extract) Engine.
    ///
    /// Uses `memory.extract_model` from manifest if set, otherwise clones
    /// the main client.
    fn build_extractor_client(
        &self,
        memory_cfg: &manifest::MemoryConfig,
    ) -> Result<Box<dyn LlmClient>, WorkerError> {
        if let Some(ref m) = memory_cfg.extract_model {
            let client = crate::model_client::build_client(m)?;
            return Ok(client);
        }
        let worker = self.engine.as_ref().expect("worker taken during run");
        Ok(worker.client().clone_boxed())
    }

    /// pointer 以降に増えたプロンプト全長の推定。extract trigger が
    /// 閾値判定に使う。
    ///
    /// `total_tokens_at(now) - total_tokens_at(pointer)` の差分で、
    /// compact と同じ accounting (measured / interpolated / extrapolated)
    /// に乗る。`history_len_pointer == 0` は「未抽出」扱いで現プロンプト
    /// 全長そのものが返る。
    ///
    /// 素朴な `usage_history.input_total_tokens` の合計は使わない:
    /// `input_total_tokens` は **送信時の prompt prefix 全長** であって
    /// 増分ではないので、長い turn 内の連続 LLM call では super-set を
    /// 何度も足し込んでしまい実消費の数倍に膨らむ。
    fn tokens_added_since(&self, history_len_pointer: usize) -> u64 {
        let now = self.history().len();
        let total_now = self.total_tokens_at(now).tokens;
        let total_at_pointer = self.total_tokens_at(history_len_pointer).tokens;
        total_now.saturating_sub(total_at_pointer)
    }

    /// extract (memory.extract) post-run trigger.
    ///
    /// Called by the Controller before spawning the background memory task so
    /// the extract worker sees a stable session-log entry range while compact
    /// is deferred until the next turn starts. Best-effort: failures are
    /// logged but not propagated.
    ///
    /// Behaviour follows `docs/plan/memory.md` §Extract 並走防止:
    /// in-flight 中の trigger は skip し、完了時点で閾値再評価する
    /// (the loop below). Pending state is not retained — the
    /// re-evaluation happens naturally because the in-memory pointer
    /// has advanced.
    pub async fn try_post_run_extract(&mut self) -> Result<(), WorkerError> {
        let Some(memory_cfg) = self.manifest.memory.clone() else {
            return Ok(());
        };
        // `Some(0)` means disabled, same as `None`. Otherwise the
        // `tokens_since >= 0` comparison would fire on every post-run.
        let Some(threshold) = memory_cfg.extract_threshold.filter(|n| *n > 0) else {
            let model = memory_cfg
                .extract_model
                .as_ref()
                .unwrap_or(&self.manifest.model);
            WorkerAuditBase::new(
                memory::audit::AuditWorker::MemoryExtract,
                memory::audit::AuditTrigger::TokenThreshold,
                Some(model_audit_from_manifest(model)),
            )
            .with_memory_settings(&memory_cfg)
            .emit(
                self.workspace_client(),
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Skipped,
                "extract_threshold_disabled",
                None,
                None,
                None,
            )
            .await;
            return Ok(());
        };

        loop {
            // CAS the in-flight flag. If another task is already running
            // an extract for this Worker, skip per spec.
            if self
                .extract_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                let model = memory_cfg
                    .extract_model
                    .as_ref()
                    .unwrap_or(&self.manifest.model);
                WorkerAuditBase::new(
                    memory::audit::AuditWorker::MemoryExtract,
                    memory::audit::AuditTrigger::TokenThreshold,
                    Some(model_audit_from_manifest(model)),
                )
                .with_memory_settings(&memory_cfg)
                .emit(
                    self.workspace_client(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "extract_already_in_flight",
                    None,
                    None,
                    None,
                )
                .await;
                return Ok(());
            }
            let result = self.run_extract_once(&memory_cfg, threshold).await;
            self.extract_in_flight.store(false, Ordering::Release);

            match result {
                Ok(ExtractDecision::Skipped) => return Ok(()),
                Ok(ExtractDecision::Completed) => {
                    // Re-evaluate threshold against the newly advanced
                    // pointer. In the current synchronous architecture
                    // this normally exits via Skipped on the next pass,
                    // but the loop is forward-looking for the case
                    // where new activity piles up while extract runs.
                    continue;
                }
                Err(e) => {
                    tracing::warn!(error = %e, "extract failed");
                    self.alert(
                        AlertLevel::Warn,
                        AlertSource::Worker,
                        format!("memory extract failed: {e}"),
                    );
                    return Ok(());
                }
            }
        }
    }

    /// Single extract iteration: snapshot pointer, decide whether to
    /// fire, run the worker if so, persist results and the new pointer.
    async fn run_extract_once(
        &mut self,
        memory_cfg: &manifest::MemoryConfig,
        threshold: u64,
    ) -> Result<ExtractDecision, WorkerError> {
        self.run_extract_once_with_cancel_observer(memory_cfg, threshold, None)
            .await
    }

    async fn run_extract_once_with_cancel_observer(
        &mut self,
        memory_cfg: &manifest::MemoryConfig,
        threshold: u64,
        cancel_observer: Option<Box<dyn FnOnce(tokio::sync::mpsc::Sender<()>) + Send + 'static>>,
    ) -> Result<ExtractDecision, WorkerError> {
        use memory::extract;

        let model = memory_cfg
            .extract_model
            .as_ref()
            .unwrap_or(&self.manifest.model);
        let audit = WorkerAuditBase::new(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::AuditTrigger::TokenThreshold,
            Some(model_audit_from_manifest(model)),
        )
        .with_memory_settings(memory_cfg);
        let event_tx = self.event_tx.as_ref();

        let pointer_snapshot = self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned")
            .clone();
        let processed_history_len = pointer_snapshot
            .as_ref()
            .map(|p| p.processed_through_history_len)
            .unwrap_or(0);

        let tokens_since = self.tokens_added_since(processed_history_len);
        if tokens_since < threshold {
            audit.emit(
                self.workspace_client(),
                event_tx,
                memory::audit::WorkerLifecycleStatus::Skipped,
                format!(
                    "token_threshold_not_reached tokens_since={tokens_since} threshold={threshold}"
                ),
                None,
                None,
                None,
            ).await;
            return Ok(ExtractDecision::Skipped);
        }

        let current_history_len = self.session.history().len();
        if current_history_len <= processed_history_len {
            audit
                .emit(
                    self.workspace_client(),
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "no_new_history_items",
                    None,
                    Some(memory::audit::ExtractAudit {
                        history_range: Some([
                            processed_history_len as u64,
                            current_history_len as u64,
                        ]),
                        ..Default::default()
                    }),
                    None,
                )
                .await;
            return Ok(ExtractDecision::Skipped);
        }

        // Read the session log to get the current entry count. This is
        // the boundary for the source.range end_entry. Called once per
        // extract, on a small local file.
        let entries_now = self
            .store
            .read_all(self.session_id(), self.segment_id())?
            .len();
        if entries_now == 0 {
            audit
                .emit(
                    self.workspace_client(),
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "empty_segment_log",
                    None,
                    None,
                    None,
                )
                .await;
            return Ok(ExtractDecision::Skipped);
        }
        let end_entry = entries_now - 1;
        let start_entry = pointer_snapshot
            .as_ref()
            .map(|p| p.processed_through_entry + 1)
            .unwrap_or(0);
        if start_entry > end_entry {
            audit
                .emit(
                    self.workspace_client(),
                    event_tx,
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "no_new_segment_entries",
                    None,
                    Some(memory::audit::ExtractAudit {
                        session_id: Some(self.session_id().to_string()),
                        segment_id: Some(self.segment_id().to_string()),
                        entry_range: Some([start_entry as u64, end_entry as u64]),
                        history_range: Some([
                            processed_history_len as u64,
                            current_history_len as u64,
                        ]),
                        ..Default::default()
                    }),
                    None,
                )
                .await;
            return Ok(ExtractDecision::Skipped);
        }

        let extract_audit_base = memory::audit::ExtractAudit {
            session_id: Some(self.session_id().to_string()),
            segment_id: Some(self.segment_id().to_string()),
            entry_range: Some([start_entry as u64, end_entry as u64]),
            history_range: Some([processed_history_len as u64, current_history_len as u64]),
            ..Default::default()
        };
        audit
            .emit(
                self.workspace_client(),
                event_tx,
                memory::audit::WorkerLifecycleStatus::Started,
                format!(
                    "token_threshold_reached tokens_since={tokens_since} threshold={threshold}"
                ),
                None,
                Some(extract_audit_base.clone()),
                None,
            )
            .await;

        let entries_to_extract =
            self.session.history().entries()[processed_history_len..current_history_len].to_vec();

        let extract_worker_max_turns = memory_cfg
            .extract_worker_max_turns
            .or(manifest::defaults::MEMORY_EXTRACT_WORKER_MAX_TURNS);

        let client = match self.build_extractor_client(memory_cfg) {
            Ok(client) => client,
            Err(err) => {
                audit
                    .emit(
                        self.workspace_client(),
                        event_tx,
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("client_build_failed: {err}"),
                        None,
                        Some(extract_audit_base),
                        None,
                    )
                    .await;
                return Err(err);
            }
        };
        let memory_language = memory_language(memory_cfg)?;
        let extract_system_prompt = match self
            .prompts
            .load_full()
            .memory_extract_system(&memory_language)
        {
            Ok(prompt) => prompt,
            Err(err) => {
                audit
                    .emit(
                        self.workspace_client(),
                        event_tx,
                        memory::audit::WorkerLifecycleStatus::Failed,
                        format!("prompt_render_failed: {err}"),
                        None,
                        Some(extract_audit_base),
                        None,
                    )
                    .await;
                return Err(WorkerError::PromptCatalog(err));
            }
        };
        let source_segment_id = self.segment_state.segment_id();
        let source = memory::schema::SourceRef {
            segment_id: source_segment_id.to_string(),
            range: [start_entry as u64, end_entry as u64],
        };
        let session_view = crate::session_capture::SessionCapture::from_history_entries(
            source_segment_id.to_string(),
            entries_to_extract,
        );
        let session_explore_state = SessionExploreState::new(session_view.clone());
        let memory_extract_state = MemoryExtractState::new(
            session_view,
            self.workspace_client_handle(),
            source,
            audit.run_id.to_string(),
        );
        let input_text = render_extract_input(session_explore_state.view());
        let features = FeatureRegistryBuilder::new()
            .with_module(SessionExploreFeature::new(session_explore_state.clone()))
            .with_module(MemoryExtractFeature::new(memory_extract_state.clone()));
        let mut internal_manifest = self.manifest.clone();
        internal_manifest.model = model.clone();
        let internal_spec = InternalWorkerSpec {
            identity: InternalWorkerIdentity {
                kind: "memory-extract",
                run_id: audit.run_id,
            },
            manifest: internal_manifest,
            client,
            system_prompt: extract_system_prompt,
            input: input_text,
            cache_key: Some(self.segment_id().to_string()),
            max_turns: extract_worker_max_turns,
            engine_configurator: None,
            features,
            required_tools: &[
                "ShowOverview",
                "SearchEntries",
                "ReadEntry",
                "StageMemoryCandidate",
                "FinishMemoryExtraction",
            ],
            authority: InternalWorkerAuthority {
                workspace: self.workspace_context.clone(),
                filesystem: WorkerFilesystemAuthority::None,
                scope: Scope::empty(),
                workdir_session: None,
            },
        };
        let internal_result = match cancel_observer {
            Some(observer) => run_internal_worker_with_cancel_sender(internal_spec, observer).await,
            None => run_internal_worker(internal_spec).await,
        };
        let usage = match internal_result {
            Ok(result) => {
                tracing::debug!(
                    internal_worker_kind = result.identity.kind,
                    internal_worker_run_id = %result.identity.run_id,
                    history_entries = result.history_entries,
                    lifecycle = ?result.lifecycle,
                    "internal Worker execution completed"
                );
                let usage = result.usage.as_ref().map(usage_audit_from_event);
                if let Some(error) = extract_internal_worker_lifecycle_error(&result.lifecycle) {
                    audit
                        .emit(
                            self.workspace_client(),
                            event_tx,
                            memory::audit::WorkerLifecycleStatus::Cancelled,
                            "worker_cancelled: internal Worker run rolled back before AI output",
                            usage,
                            Some(extract_audit_base),
                            None,
                        )
                        .await;
                    return Err(error);
                }
                usage
            }
            Err(err) => {
                tracing::debug!(
                    internal_worker_kind = err.identity.kind,
                    internal_worker_run_id = %err.identity.run_id,
                    history_entries = err.history_entries,
                    "internal Worker execution failed"
                );
                let usage = err.usage.as_ref().map(usage_audit_from_event);
                audit
                    .emit(
                        self.workspace_client(),
                        event_tx,
                        lifecycle_status_for_worker_error(&err.source),
                        format!("worker_failed: {}", err.source),
                        usage,
                        Some(extract_audit_base),
                        None,
                    )
                    .await;
                return Err(err.source);
            }
        };

        let staging_results = memory_extract_state.staged();
        if !memory_extract_state.is_finished() {
            tracing::warn!(
                staged_count = staging_results.len(),
                "extract worker did not call FinishMemoryExtraction; advancing pointer with staged output"
            );
        }
        let staging_id = staging_results.first().cloned().unwrap_or_default();

        let pointer_payload = extract::ExtractPointerPayload {
            processed_through_entry: end_entry,
            processed_through_history_len: current_history_len,
            staging_id: staging_id.clone(),
        };
        let payload_value = serde_json::to_value(&pointer_payload)
            .expect("ExtractPointerPayload is always JSON-serializable");
        self.commit_entry(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: extract::EXTRACT_DOMAIN.into(),
            payload: payload_value,
        })?;

        *self
            .extract_pointer
            .lock()
            .expect("extract_pointer poisoned") = Some(pointer_payload);

        let mut extract_audit = extract_audit_base;
        extract_audit.staging_count = staging_results.len();
        for id in &staging_results {
            extract_audit.staging_ids.push(id.clone());
        }
        let reason = if staging_id.is_empty() {
            "completed_no_staging_output"
        } else {
            "completed_staging_written"
        };
        audit
            .emit(
                self.workspace_client(),
                event_tx,
                memory::audit::WorkerLifecycleStatus::Completed,
                reason,
                usage,
                Some(extract_audit),
                None,
            )
            .await;

        Ok(ExtractDecision::Completed)
    }

    /// Request Backend-managed Memory staging consolidation after a Worker turn.
    ///
    /// Worker has no local Workspace memory authority. It only asks the Backend
    /// Workspace to notify or spawn the dedicated consolidater Worker.
    pub async fn try_post_run_consolidate(&mut self) -> Result<(), WorkerError> {
        let Some(memory_cfg) = self.manifest.memory.clone() else {
            return Ok(());
        };
        let model = memory_cfg
            .consolidation_model
            .as_ref()
            .unwrap_or(&self.manifest.model);
        let files_threshold = memory_cfg.consolidation_threshold_files.filter(|n| *n > 0);
        let bytes_threshold = memory_cfg.consolidation_threshold_bytes.filter(|n| *n > 0);
        if files_threshold.is_none() && bytes_threshold.is_none() {
            WorkerAuditBase::new(
                memory::audit::AuditWorker::MemoryConsolidation,
                memory::audit::AuditTrigger::StagingBacklog,
                Some(model_audit_from_manifest(model)),
            )
            .with_memory_settings(&memory_cfg)
            .emit(
                self.workspace_client(),
                self.event_tx.as_ref(),
                memory::audit::WorkerLifecycleStatus::Skipped,
                "consolidation_threshold_disabled",
                None,
                None,
                None,
            )
            .await;
            return Ok(());
        }

        match self
            .workspace_client()
            .request_memory_staging_consolidation(
                memory::backend::MemoryConsolidateStagingOperation {
                    force: false,
                    threshold_files: files_threshold,
                    threshold_bytes: bytes_threshold,
                },
            )
            .await
        {
            Ok(output) => {
                tracing::debug!(
                    status = output.status.as_str(),
                    summary = output.summary.as_str(),
                    "requested backend memory staging consolidation"
                );
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "failed to request backend memory staging consolidation"
                );
                WorkerAuditBase::new(
                    memory::audit::AuditWorker::MemoryConsolidation,
                    memory::audit::AuditTrigger::StagingBacklog,
                    Some(model_audit_from_manifest(model)),
                )
                .with_memory_settings(&memory_cfg)
                .emit(
                    self.workspace_client(),
                    self.event_tx.as_ref(),
                    memory::audit::WorkerLifecycleStatus::Skipped,
                    "consolidation_backend_operation_failed",
                    None,
                    None,
                    None,
                )
                .await;
            }
        }
        Ok(())
    }
}

fn extract_internal_worker_lifecycle_error(lifecycle: &WorkerRunResult) -> Option<WorkerError> {
    match lifecycle {
        WorkerRunResult::RolledBack => Some(WorkerError::Engine(EngineError::Cancelled)),
        WorkerRunResult::Interrupted { message, .. } => {
            Some(WorkerError::Engine(EngineError::Aborted(message.clone())))
        }
        WorkerRunResult::Finished | WorkerRunResult::Paused | WorkerRunResult::LimitReached => None,
    }
}

fn lifecycle_status_for_worker_error(err: &WorkerError) -> memory::audit::WorkerLifecycleStatus {
    if matches!(err, WorkerError::Engine(EngineError::Cancelled)) {
        memory::audit::WorkerLifecycleStatus::Cancelled
    } else {
        memory::audit::WorkerLifecycleStatus::Failed
    }
}

fn usage_audit_from_event(
    event: &agen::llm_client::event::UsageEvent,
) -> memory::audit::UsageAudit {
    memory::audit::UsageAudit {
        input_tokens: event.input_tokens,
        output_tokens: event.output_tokens,
        total_tokens: event.total_tokens,
        cache_read_input_tokens: event.cache_read_input_tokens,
        cache_creation_input_tokens: event.cache_creation_input_tokens,
    }
}

fn model_audit_from_manifest(model: &manifest::ModelManifest) -> memory::audit::ModelAudit {
    memory::audit::ModelAudit {
        ref_: model.ref_.clone(),
        scheme: model.scheme.map(|scheme| format!("{scheme:?}")),
        model_id: model.model_id.clone(),
    }
}

fn emit_memory_worker_event(
    event_tx: Option<&broadcast::Sender<Event>>,
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    status: memory::audit::WorkerLifecycleStatus,
    trigger: memory::audit::AuditTrigger,
    reason: &str,
) {
    let Some(event_tx) = event_tx else {
        return;
    };
    let message = format!("memory {} {}: {reason}", worker.label(), status.label());
    let _ = event_tx.send(Event::MemoryWorker(protocol::MemoryWorkerEvent {
        worker: worker.label().to_string(),
        status: status.label().to_string(),
        run_id: run_id.to_string(),
        trigger: trigger.label().to_string(),
        reason: reason.to_string(),
        message,
        timestamp_ms: segment_log::now_millis() as i64,
    }));
}

#[derive(Debug, Clone)]
struct WorkerAuditBase {
    run_id: uuid::Uuid,
    worker: memory::audit::AuditWorker,
    trigger: memory::audit::AuditTrigger,
    model: Option<memory::audit::ModelAudit>,
    memory_settings: Option<memory::audit::MemorySettingsAudit>,
}

impl WorkerAuditBase {
    fn new(
        worker: memory::audit::AuditWorker,
        trigger: memory::audit::AuditTrigger,
        model: Option<memory::audit::ModelAudit>,
    ) -> Self {
        Self {
            run_id: uuid::Uuid::now_v7(),
            worker,
            trigger,
            model,
            memory_settings: None,
        }
    }

    fn with_memory_settings(mut self, memory_config: &manifest::MemoryConfig) -> Self {
        self.memory_settings =
            memory_config
                .workspace_settings()
                .map(|snapshot| memory::audit::MemorySettingsAudit {
                    workspace_id: snapshot.workspace_id,
                    settings_revision: snapshot.settings_revision,
                    language: snapshot.language,
                });
        self
    }

    async fn emit(
        &self,
        workspace_client: &dyn WorkspaceClient,
        event_tx: Option<&broadcast::Sender<Event>>,
        status: memory::audit::WorkerLifecycleStatus,
        reason: impl Into<String>,
        usage: Option<memory::audit::UsageAudit>,
        extract: Option<memory::audit::ExtractAudit>,
        consolidation: Option<memory::audit::ConsolidationAudit>,
    ) {
        let reason = reason.into();
        let payload = memory::audit::WorkerLifecycleAudit {
            run_id: self.run_id,
            worker: self.worker,
            status,
            trigger: self.trigger,
            reason: reason.clone(),
            memory_settings: self.memory_settings.clone(),
            model: self.model.clone(),
            usage,
            extract,
            consolidation,
        };
        let _ = workspace_client
            .execute_memory_backend_operation(memory::backend::MemoryBackendOperation::AppendAudit(
                memory::backend::MemoryAppendAuditOperation {
                    event: memory::audit::AuditEvent::new(
                        memory::audit::AuditPayload::WorkerLifecycle(payload),
                    ),
                },
            ))
            .await;
        if should_emit_memory_worker_event(self.worker, status, &reason) {
            emit_memory_worker_event(
                event_tx,
                self.run_id,
                self.worker,
                status,
                self.trigger,
                &reason,
            );
        }
    }
}

fn should_emit_memory_worker_event(
    worker: memory::audit::AuditWorker,
    status: memory::audit::WorkerLifecycleStatus,
    reason: &str,
) -> bool {
    if worker == memory::audit::AuditWorker::MemoryConsolidation
        && status == memory::audit::WorkerLifecycleStatus::Skipped
    {
        return !is_idle_consolidation_skip_reason(reason);
    }
    true
}

fn is_idle_consolidation_skip_reason(reason: &str) -> bool {
    reason == "no_staging_entries"
        || reason == "consolidation_threshold_disabled"
        || reason.starts_with("threshold_not_reached")
}

fn memory_language(cfg: &manifest::MemoryConfig) -> Result<String, WorkerError> {
    cfg.workspace_settings()
        .map(|snapshot| snapshot.language)
        .ok_or_else(|| {
            WorkerError::InvalidState(
                "Memory operation requires a bound Workspace Memory settings snapshot".to_string(),
            )
        })
}

fn worker_language(cfg: &manifest::EngineManifest) -> &str {
    let language = cfg.language.trim();
    if language.is_empty() {
        manifest::defaults::WORKER_LANGUAGE
    } else {
        language
    }
}

/// Outcome of a single extract iteration. Internal to
/// `try_post_run_extract` / `run_extract_once`.
enum ExtractDecision {
    /// Threshold not reached, or no items to extract.
    Skipped,
    /// Extract ran and pointer advanced. Caller re-evaluates threshold.
    Completed,
}

impl<St> Worker<Box<dyn LlmClient>, St>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    /// Create a Worker entirely from a validated manifest.
    ///
    /// The Worker's working directory is captured once here from the
    /// process's `std::env::current_dir()` — callers that want a
    /// different cwd must `cd` before constructing the Worker (e.g. the
    /// `SubWorkerSpawn` tool sets `Command::current_dir` on the child). The
    /// captured cwd is canonicalised and validated against
    /// `manifest.scope`.
    ///
    /// `loader` is installed into the system-prompt template
    /// environment so that `{% include "name" %}` /
    /// `{% import "name" %}` references resolve against the three-layer
    /// prompt asset library.
    pub async fn from_manifest(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
    ) -> Result<Self, WorkerError> {
        let cwd = current_cwd()?;
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        Self::from_manifest_with_context(manifest, store, loader, workspace_context, authority)
            .await
    }

    pub async fn from_manifest_with_context(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
    ) -> Result<Self, WorkerError> {
        Self::from_manifest_with_context_and_model_client(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            None,
        )
        .await
    }

    pub(crate) async fn from_manifest_with_context_and_model_client(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
    ) -> Result<Self, WorkerError> {
        validate_workspace_memory_snapshot(&manifest.worker.name, &manifest, &workspace_context)?;
        let common = prepare_worker_common_with_context_and_model_client(
            &manifest,
            &loader,
            /* parse_template */ true,
            workspace_context,
            filesystem_authority,
            manifest.scope.clone(),
            model_client,
        )?;

        // Segment creation is deferred to the first run (see
        // `ensure_segment_head`) so the SegmentStart entry can capture
        // the rendered system prompt, not the raw template source. The
        // session_id + segment_id are allocated here so the worker-allocation
        // registration can record them from the start.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();

        // Register this Worker in the machine-wide worker-allocation
        // before building anything else, so a spawn that conflicts on
        // scope fails fast.
        let socket_path = dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.worker.name)
            .join("sock");
        let scope_allocation = worker_allocation::install_top_level(
            manifest.worker.name.clone(),
            std::process::id(),
            socket_path,
            common.scope.allow_rules(),
            segment_id,
        )?;

        let mut worker =
            Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                common.client,
            );
        apply_worker_manifest(&mut worker, &manifest.engine);
        worker.set_cache_key(Some(segment_id.to_string()));
        let worker_metadata_writer = Some(worker_metadata_writer_for_store(&store));
        let scope = SharedScope::new(common.scope);
        let workdir_session = workdir_session_from_authority(&common.filesystem_authority, &scope);

        let mut worker = Self {
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority: common.filesystem_authority,
            workdir_session,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            delegation_scope: common.delegation_scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            system_prompt_template: common.system_prompt_template,
            feature_instructions: common.feature_instructions,
            alerter: None,
            event_tx: None,
            internal_worker_registry: None,
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        worker.write_worker_metadata_pending()?;
        Ok(worker)
    }

    /// Build an in-process Internal Worker without machine-wide allocation or durable Worker metadata.
    pub(crate) async fn from_internal_manifest_with_context(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        client_override: Option<Box<dyn LlmClient>>,
    ) -> Result<Self, WorkerError> {
        let mut common = prepare_worker_common_with_context(
            &manifest,
            &loader,
            true,
            workspace_context,
            filesystem_authority,
            manifest.scope.clone(),
        )?;
        if let Some(client) = client_override {
            common.client = client;
        }
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let mut engine =
            Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                common.client,
            );
        apply_worker_manifest(&mut engine, &manifest.engine);
        engine.set_cache_key(Some(segment_id.to_string()));
        let scope = SharedScope::new(common.scope);
        let workdir_session = workdir_session_from_authority(&common.filesystem_authority, &scope);
        let mut worker = Self {
            manifest,
            engine: Some(engine),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer: None,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority: common.filesystem_authority,
            workdir_session,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            delegation_scope: common.delegation_scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            system_prompt_template: common.system_prompt_template,
            feature_instructions: common.feature_instructions,
            alerter: None,
            event_tx: None,
            internal_worker_registry: None,
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: None,
            callback_socket: None,
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        Ok(worker)
    }

    /// Build a Worker spawned by another Worker (sibling process).
    ///
    /// Behaves like [`Worker::from_manifest`] but claims the scope
    /// allocation that the spawner pre-registered via
    /// [`worker_allocation::delegate_scope`], rather than installing a new
    /// top-level entry. `callback_socket` carries the spawner's
    /// Unix-socket path so the spawned Worker can send `Method::Notify`
    /// back to the spawner.
    pub async fn from_manifest_spawned(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        callback_socket: PathBuf,
    ) -> Result<Self, WorkerError> {
        let cwd = current_cwd()?;
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        Self::from_manifest_spawned_with_context(
            manifest,
            store,
            loader,
            callback_socket,
            workspace_context,
            authority,
        )
        .await
    }

    pub async fn from_manifest_spawned_with_context(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        callback_socket: PathBuf,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
    ) -> Result<Self, WorkerError> {
        validate_workspace_memory_snapshot(&manifest.worker.name, &manifest, &workspace_context)?;
        let common = prepare_worker_common_with_context(
            &manifest,
            &loader,
            /* parse_template */ true,
            workspace_context,
            filesystem_authority,
            manifest.scope.clone(),
        )?;

        // A spawned child starts its own conversation, so it mints a
        // fresh Session rather than joining the spawner's.
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let scope_allocation = worker_allocation::adopt_allocation(
            manifest.worker.name.clone(),
            std::process::id(),
            segment_id,
        )?;

        let mut worker =
            Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                common.client,
            );
        apply_worker_manifest(&mut worker, &manifest.engine);
        worker.set_cache_key(Some(segment_id.to_string()));
        let worker_metadata_writer = Some(worker_metadata_writer_for_store(&store));
        let scope = SharedScope::new(common.scope);
        let workdir_session = workdir_session_from_authority(&common.filesystem_authority, &scope);

        let mut worker = Self {
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority: common.filesystem_authority,
            workdir_session,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            delegation_scope: common.delegation_scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            system_prompt_template: common.system_prompt_template,
            feature_instructions: common.feature_instructions,
            alerter: None,
            event_tx: None,
            internal_worker_registry: None,
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: Some(callback_socket),
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(None)),
            memory_task: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        worker.write_worker_metadata_pending()?;
        Ok(worker)
    }

    /// Restore a Worker by resolving its name-keyed metadata to an active
    /// `(SessionId, SegmentId)` and then using the normal session-log restore
    /// path. The metadata stores only the active pointer; lineage and origin
    /// remain authoritative in the session log.
    pub async fn restore_from_worker_metadata(
        worker_name: &str,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
    ) -> Result<Self, WorkerError> {
        let cwd = current_cwd()?;
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        Self::restore_from_worker_metadata_with_context(
            worker_name,
            manifest,
            store,
            loader,
            workspace_context,
            authority,
        )
        .await
    }

    pub async fn restore_from_worker_metadata_with_context(
        worker_name: &str,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
    ) -> Result<Self, WorkerError> {
        let metadata =
            store
                .read_by_name(worker_name)?
                .ok_or_else(|| WorkerError::WorkerMetadataMissing {
                    worker_name: worker_name.to_string(),
                })?;
        if workspace_context.workspace_id().is_some()
            && metadata.resolved_manifest_snapshot.is_none()
        {
            return Err(WorkerError::WorkerMetadataManifestSnapshotMissing {
                worker_name: worker_name.to_string(),
            });
        }
        let active = metadata
            .active
            .ok_or_else(|| WorkerError::WorkerMetadataInactive {
                worker_name: worker_name.to_string(),
            })?;
        let segment_id = active
            .segment_id
            .ok_or_else(|| WorkerError::WorkerMetadataPending {
                worker_name: worker_name.to_string(),
                session_id: active.session_id,
            })?;
        let manifest = restore_manifest_from_worker_metadata_snapshot(
            worker_name,
            metadata.resolved_manifest_snapshot,
            manifest,
        )?;
        Self::restore_from_manifest_with_context(
            active.session_id,
            segment_id,
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
        )
        .await
    }

    /// Recreate a pending Worker whose metadata has a session id but no
    /// materialized segment yet.
    ///
    /// Pending Workers have already had their profile source resolved at creation
    /// time, but they have not rendered the system prompt or written
    /// `SegmentStart`. Restore therefore uses only the resolved manifest snapshot
    /// stored in Worker metadata and never re-resolves the profile source.
    pub async fn restore_pending_from_worker_metadata_with_context(
        worker_name: &str,
        fallback: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
    ) -> Result<Self, WorkerError> {
        let metadata =
            store
                .read_by_name(worker_name)?
                .ok_or_else(|| WorkerError::WorkerMetadataMissing {
                    worker_name: worker_name.to_string(),
                })?;
        if workspace_context.workspace_id().is_some()
            && metadata.resolved_manifest_snapshot.is_none()
        {
            return Err(WorkerError::WorkerMetadataManifestSnapshotMissing {
                worker_name: worker_name.to_string(),
            });
        }
        let active = metadata
            .active
            .ok_or_else(|| WorkerError::WorkerMetadataInactive {
                worker_name: worker_name.to_string(),
            })?;
        if let Some(segment_id) = active.segment_id {
            return Self::restore_from_manifest_with_context(
                active.session_id,
                segment_id,
                restore_manifest_from_worker_metadata_snapshot(
                    worker_name,
                    metadata.resolved_manifest_snapshot,
                    fallback,
                )?,
                store,
                loader,
                workspace_context,
                filesystem_authority,
            )
            .await;
        }
        let snapshot = metadata.resolved_manifest_snapshot.ok_or_else(|| {
            WorkerError::WorkerMetadataManifestSnapshotMissing {
                worker_name: worker_name.to_string(),
            }
        })?;
        let manifest =
            restore_manifest_from_worker_metadata_snapshot(worker_name, Some(snapshot), fallback)?;
        Self::from_manifest_with_context(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
        )
        .await
    }

    /// Restore a Worker from an existing session log.
    ///
    /// Uses the resolved manifest supplied by the caller, seeds a
    /// fresh Engine from the source session's `RestoredState`, and
    /// reuses the same `segment_id` so subsequent turns append to the
    /// source jsonl as a continuation of the same conversation.
    ///
    /// Concurrent writers are prevented by the worker-allocation:
    /// the registration carries `segment_id`, and this constructor
    /// refuses to start when `worker_allocation::lookup_segment` already finds
    /// a live Worker writing to `segment_id`. So there is no need to fork —
    /// resume is "the same session, a different process owning it".
    ///
    /// `system_prompt` is replayed verbatim from the session log —
    /// templates are not re-rendered on restore so a long-running
    /// session keeps a stable cache prefix even when the manifest's
    /// instruction template would render differently today.
    pub async fn restore_from_manifest(
        session_id: SessionId,
        segment_id: SegmentId,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
    ) -> Result<Self, WorkerError> {
        let cwd = current_cwd()?;
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
        Self::restore_from_manifest_with_context(
            session_id,
            segment_id,
            manifest,
            store,
            loader,
            workspace_context,
            authority,
        )
        .await
    }

    pub async fn restore_from_manifest_with_context(
        session_id: SessionId,
        segment_id: SegmentId,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
    ) -> Result<Self, WorkerError> {
        // Read raw entries once so we can both reconstruct state and
        // seed the broadcast sink's mirror with the same prefix that
        // sits on disk.
        let raw_entries = store.read_all(session_id, segment_id)?;
        let state = session_store::collect_state(&raw_entries);
        if state.entries_count == 0 {
            return Err(WorkerError::SegmentEmpty { segment_id });
        }
        if state.system_prompt.is_none() {
            return Err(WorkerError::SegmentSystemPromptMissing { segment_id });
        }
        let mirror_entries: Vec<LogEntry> = raw_entries.clone();
        let scope_config = effective_restore_scope_config(&store, &manifest)?;

        let common = prepare_worker_common_with_context(
            &manifest,
            &loader,
            /* parse_template */ false,
            workspace_context,
            filesystem_authority,
            scope_config,
        )?;

        // Atomic: register_worker inside install_top_level rejects when
        // another live allocation already holds `segment_id`. Wrapping
        // the lookup + install inside a single `LockFileGuard` is what
        // makes "no two live Workers write to the same session log"
        // actually structural rather than a hopeful pre-check.
        let socket_path = dir::default_base()
            .map_err(ScopeLockError::from)?
            .join(&manifest.worker.name)
            .join("sock");
        let scope_allocation = worker_allocation::install_top_level_with_deny(
            manifest.worker.name.clone(),
            std::process::id(),
            socket_path,
            common.scope.allow_rules(),
            common.scope.deny_rules(),
            segment_id,
        )?;

        // Build the worker and apply the manifest defaults first, then
        // overwrite the pieces the session log is authoritative for.
        let mut worker =
            Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                common.client,
            );
        apply_worker_manifest(&mut worker, &manifest.engine);
        worker.set_cache_key(Some(segment_id.to_string()));
        if let Some(ref prompt) = state.system_prompt {
            worker.set_system_prompt(prompt);
        }
        // A leading `Role::System` item can only come from `compact`
        // (the Worker's one and only write path that prepends a summary at
        // history[0]). Restoring the anchor lets Anthropic re-use a
        // stable cache prefix for long-lived restored sessions.
        let restored_history_entries =
            restore_history_entries(session_id, segment_id, &raw_entries).map_err(|error| {
                WorkerError::InvalidState(format!("restore typed Worker session history: {error}"))
            })?;
        let restored_history = restored_history_entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let anchored_on_summary = matches!(
            restored_history.first(),
            Some(Item::Message {
                role: agen::Role::System,
                ..
            })
        );
        worker.set_request_config(state.config.clone());
        worker.set_turn_count(state.turn_count);
        worker.set_active_run_turn_count(state.active_run_turn_count);
        if anchored_on_summary {
            worker.set_cache_anchor(Some(0));
        }

        let extract_pointer = memory::extract::fold_pointer(&state.extensions);
        let task_feature = TaskFeature::from_history(&state.history);
        let worker_metadata_writer = Some(worker_metadata_writer_for_store(&store));
        let scope = SharedScope::new(common.scope);
        let workdir_session = workdir_session_from_authority(&common.filesystem_authority, &scope);

        let mut worker = Self {
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, restored_history_entries),
            last_run_interrupted: state.last_run_interrupted,
            store,
            worker_metadata_writer,
            segment_state: SegmentState::new(session_id, segment_id, state.entries_count),
            filesystem_authority: common.filesystem_authority,
            workdir_session,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(restored_flow_runtime_state(
                &state.extensions,
            )?)),
            flow_feature_enabled: false,
            scope,
            delegation_scope: common.delegation_scope,
            hook_builder: HookRegistryBuilder::new(),
            interceptor_installed: false,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(state.usage_history)),
            tracker: None,
            task_feature,
            worker_observation_provider: None,
            // Restore replays the saved system_prompt verbatim — no
            // template re-render on resume.
            system_prompt_template: None,
            feature_instructions: common.feature_instructions,
            alerter: None,
            event_tx: None,
            internal_worker_registry: None,
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            scope_allocation: Some(scope_allocation),
            callback_socket: None,
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            extract_in_flight: Arc::new(AtomicBool::new(false)),
            consolidation_in_flight: Arc::new(AtomicBool::new(false)),
            extract_pointer: Arc::new(Mutex::new(extract_pointer)),
            memory_task: None,
            user_segments: state.user_segments,
            // Seed the mirror with the entries we just replayed so a
            // late-attaching client sees the full prefix without an
            // extra round trip.
            sink: SegmentLogSink::with_initial(mirror_entries),
            history_persistence_wired: false,
            log_writer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        worker.write_worker_metadata_active(SegmentLocation {
            session_id,
            segment_id,
        })?;
        worker.reconcile_restored_delegations().await?;
        Ok(worker)
    }

    async fn reconcile_restored_delegations(&mut self) -> Result<(), WorkerError> {
        let worker_name = self.manifest.worker.name.clone();
        let Some(metadata) = self.store.read_by_name(&worker_name)? else {
            return Ok(());
        };

        let mut reclaimed = Vec::new();
        for child in metadata.spawned_children {
            if restored_child_reachable(&child).await {
                continue;
            }
            let delegated_scope = spawned_child_scope_rules(&child);
            if !delegated_scope.is_empty() {
                let lock_path =
                    worker_allocation::default_allocation_path().map_err(ScopeLockError::from)?;
                let mut guard = worker_allocation::LockFileGuard::open(&lock_path)
                    .map_err(ScopeLockError::from)?;
                worker_allocation::reclaim_delegated_scope(
                    &mut guard,
                    &worker_name,
                    &child.worker_name,
                    &delegated_scope,
                )?;
                let write_rules = delegated_scope
                    .iter()
                    .filter(|rule| rule.permission == Permission::Write)
                    .cloned()
                    .collect::<Vec<_>>();
                self.scope
                    .update(|current| current.with_removed_deny_rules(write_rules))
                    .map_err(WorkerError::Scope)?;
            }
            reclaimed.push(WorkerReclaimedChild {
                worker_name: child.worker_name,
                scope_delegated: child.scope_delegated,
            });
        }

        if reclaimed.is_empty() {
            return Ok(());
        }

        self.store
            .reclaim_spawned_children(&worker_name, reclaimed)?;
        self.push_notify(
            "Restored Worker state contained missing or unreachable delegated child Workers; their delegated write scopes were reclaimed before resume."
                .to_string(),
            false,
        );
        Ok(())
    }

    /// Convenience: build a Worker from a single-layer TOML manifest string.
    ///
    /// Parses the TOML into a [`WorkerManifestConfig`], converts to a
    /// validated [`WorkerManifest`] via `TryFrom`, then delegates to
    /// [`Worker::from_manifest`]. Useful for tests, debugging, and any
    /// caller that wants to skip the cascade entirely.
    pub async fn from_manifest_toml(toml: &str, store: St) -> Result<Self, WorkerError> {
        let config = WorkerManifestConfig::from_toml(toml).map_err(WorkerError::ManifestParse)?;
        let manifest = WorkerManifest::try_from(config).map_err(WorkerError::ManifestResolve)?;
        Self::from_manifest(manifest, store, PromptCatalogSource::builtins_only()).await
    }
}

/// Apply worker-level manifest settings to a Engine.
///
/// Note: `system_prompt` is intentionally not applied here. It is a
/// minijinja template that is parsed by `Worker::from_manifest` and
/// rendered once at first turn in `ensure_system_prompt_materialized`.
pub fn apply_worker_manifest<C: LlmClient + 'static, A>(
    worker: &mut Engine<C, Mutable, A>,
    wm: &manifest::EngineManifest,
) {
    worker.set_request_config(request_config_from_engine_manifest(wm));
    worker.set_max_turns(wm.max_turns.map(|n| n.get()));
    // Worker owns the lifecycle strategy for already-started tool operations.
    // The provider must first accept cooperative cancellation, then confirm a
    // terminal result before this bounded deadline; Agen handles only the
    // mechanical per-call terminalization.
    worker.set_tool_execution_policy(ToolExecutionPolicy {
        pause_safe_boundary_timeout: Duration::from_millis(100),
        cancellation_request_timeout: Duration::from_millis(250),
        terminal_confirmation_timeout: Duration::from_millis(500),
    });
    worker.set_tool_output_limits(Some(ToolOutputLimits {
        default_max_bytes: wm.tool_output.default_max_bytes,
        per_tool: wm.tool_output.per_tool.clone(),
    }));
}

fn request_config_from_engine_manifest(wm: &manifest::EngineManifest) -> RequestConfig {
    let mut config = RequestConfig::new();
    if let Some(max_tokens) = wm.max_tokens {
        config.max_tokens = Some(max_tokens);
    }
    if let Some(temperature) = wm.temperature {
        config.temperature = Some(temperature);
    }
    if let Some(top_p) = wm.top_p {
        config.top_p = Some(top_p);
    }
    if let Some(top_k) = wm.top_k {
        config.top_k = Some(top_k);
    }
    config.stop_sequences = wm.stop_sequences.clone();
    config.reasoning = wm.reasoning.clone();
    config
}

fn worker_metadata_for_manifest(
    manifest: &WorkerManifest,
    workspace_id: Option<&WorkspaceId>,
    local_workspace_root: Option<&std::path::Path>,
    active: Option<WorkerActiveSegmentRef>,
) -> WorkerMetadata {
    let mut metadata = WorkerMetadata::new(manifest.worker.name.clone(), active);
    if let Some(workspace_id) = workspace_id {
        metadata = metadata.with_workspace_id(workspace_id.as_str().to_owned());
    }
    if let Some(local_workspace_root) = local_workspace_root {
        metadata = metadata.with_workspace_root(local_workspace_root.to_path_buf());
    }
    if should_persist_resolved_manifest_snapshot(manifest) {
        metadata.resolved_manifest_snapshot = serde_json::to_value(manifest).ok();
    }
    metadata
}

fn validate_workspace_memory_snapshot(
    worker_name: &str,
    manifest: &WorkerManifest,
    workspace_context: &WorkerWorkspaceContext,
) -> Result<(), WorkerError> {
    let Some(workspace_id) = workspace_context.workspace_id() else {
        return Ok(());
    };
    let snapshot = manifest
        .memory
        .as_ref()
        .and_then(manifest::MemoryConfig::workspace_settings)
        .ok_or_else(|| {
            WorkerError::InvalidState(format!(
                "Workspace Worker {worker_name} has no complete persisted Memory settings snapshot"
            ))
        })?;
    if snapshot.workspace_id != workspace_id.as_str() {
        return Err(WorkerError::InvalidState(format!(
            "Workspace Worker {worker_name} Memory settings belong to {} instead of {}",
            snapshot.workspace_id,
            workspace_id.as_str()
        )));
    }
    if snapshot.settings_revision == 0
        || !manifest::is_normalized_workspace_memory_language(&snapshot.language)
    {
        return Err(WorkerError::InvalidState(format!(
            "Workspace Worker {worker_name} has corrupt Memory settings snapshot metadata"
        )));
    }
    Ok(())
}

fn should_persist_resolved_manifest_snapshot(manifest: &WorkerManifest) -> bool {
    manifest.profile.is_some()
        || manifest.plugins.has_resolved_plan()
        || manifest
            .memory
            .as_ref()
            .and_then(manifest::MemoryConfig::workspace_settings)
            .is_some()
}

fn restore_manifest_from_worker_metadata_snapshot(
    worker_name: &str,
    snapshot: Option<serde_json::Value>,
    fallback: WorkerManifest,
) -> Result<WorkerManifest, WorkerError> {
    match snapshot {
        Some(snapshot) => serde_json::from_value(snapshot).map_err(|source| {
            WorkerError::WorkerMetadataManifestSnapshot {
                worker_name: worker_name.to_string(),
                source,
            }
        }),
        None => Ok(fallback),
    }
}

fn stop_reason_error_code(reason: &StopReason) -> ErrorCode {
    match reason {
        StopReason::ContextWindowExceeded | StopReason::Unexpected(EngineError::Client(_)) => {
            ErrorCode::ProviderError
        }
        StopReason::Unexpected(EngineError::Tool(_)) => ErrorCode::ToolError,
        StopReason::LimitReached
        | StopReason::Cancelled
        | StopReason::Unexpected(
            EngineError::Aborted(_)
            | EngineError::Cancelled
            | EngineError::PauseRequested
            | EngineError::ConfigWarnings(_)
            | EngineError::HistoryAppend(_)
            | EngineError::ToolAttemptFence(_),
        ) => ErrorCode::Internal,
    }
}

fn stop_reason_message(reason: &StopReason) -> String {
    match reason {
        StopReason::LimitReached => "engine turn limit reached".to_string(),
        StopReason::ContextWindowExceeded => "model context window reached".to_string(),
        StopReason::Cancelled => "engine run cancelled".to_string(),
        StopReason::Unexpected(error) => format!("unexpected engine failure: {error}"),
    }
}

/// Result of a Worker run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerRunResult {
    /// The LLM finished its turn normally.
    Finished,
    /// The LLM paused (e.g. awaiting user confirmation via a hook).
    Paused,
    /// The worker reached its configured max_turns limit.
    LimitReached,
    /// The run was interrupted by a known or unexpected terminal cause.
    Interrupted { code: ErrorCode, message: String },
    /// The submit-time user turn was rolled back because no AI output was materialized.
    RolledBack,
}

/// Result of a manual compaction request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualCompactResult {
    /// The history was compacted into a new segment.
    Compacted { new_segment_id: SegmentId },
    /// No compaction was run; the message has already been surfaced as an alert.
    Skipped { message: String },
}

impl From<EngineResult> for WorkerRunResult {
    fn from(r: EngineResult) -> Self {
        match r {
            EngineResult::Finished => WorkerRunResult::Finished,
            EngineResult::Paused => WorkerRunResult::Paused,
            EngineResult::LimitReached => WorkerRunResult::LimitReached,
            // Yielded is internal to Worker: it's always caught by
            // handle_worker_result and never converted to WorkerRunResult.
            EngineResult::Yielded => unreachable!("Yielded never converts to WorkerRunResult"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SummaryInputOptions {
    overview_target_tokens: u64,
    overview_warning_tokens: u64,
    overview_deadline_tokens: u64,
    summary_target_tokens: u64,
}

#[derive(Debug)]
struct SummaryInputBuild {
    text: String,
    overview_tokens: u64,
    warning_exceeded: bool,
    deadline_fallback_used: bool,
}

/// Build the compact worker's input: default-reference instructions,
/// the list of recently-touched files, task snapshot,
/// and a bounded overview rather than a prefix-wide transcript.
fn build_summary_input(
    items: &[Item],
    default_refs: &[PathBuf],
    task_snapshot: Option<&str>,
    options: SummaryInputOptions,
) -> SummaryInputBuild {
    let overview = build_summary_overview(
        items,
        options.overview_target_tokens,
        options.overview_deadline_tokens,
    );
    let overview_tokens = estimate_text_tokens(overview.len());
    let warning_exceeded =
        options.overview_warning_tokens > 0 && overview_tokens > options.overview_warning_tokens;
    let deadline_fallback_used =
        options.overview_deadline_tokens > 0 && overview_tokens > options.overview_deadline_tokens;
    let overview = if deadline_fallback_used {
        build_coarse_summary_overview(items, options.overview_deadline_tokens)
    } else {
        overview
    };
    let overview_tokens = estimate_text_tokens(overview.len());

    let mut out = String::new();
    out.push_str(&format!(
        "Summarise this session into a structured summary of about {} tokens and \
         nominate files the next session needs. The conversation below is a \
         bounded overview/index, not the full transcript. Use tools to inspect \
         current files when deciding auto-read/reference output.\n\n",
        options.summary_target_tokens
    ));
    if !default_refs.is_empty() {
        out.push_str(
            "These files were touched recently in this session. Use `read_file` \
             on them as needed, then call `mark_read_required` for any whose \
             contents the next session must have, and `add_reference` for files \
             it should know about by name only.\n\n## Referenced files\n",
        );
        for p in default_refs {
            out.push_str("- ");
            out.push_str(&p.display().to_string());
            out.push('\n');
        }
        out.push('\n');
    }
    if let Some(task_snapshot) = task_snapshot {
        out.push_str(
            "## Current Session TaskStore\n\
             This is the full current task list. Use it as source material for the \
             summary, especially active (pending/inprogress) tasks, but do not edit tasks \
             from the compact worker.\n",
        );
        out.push_str(task_snapshot);
        out.push_str("\n\n");
    }
    out.push_str("## Conversation overview/index\n");
    out.push_str(&overview);
    out.push_str("\n\nWhen you are done, call `write_summary` with the final 5-section text.");

    SummaryInputBuild {
        text: out,
        overview_tokens,
        warning_exceeded,
        deadline_fallback_used,
    }
}

fn build_summary_overview(items: &[Item], target_tokens: u64, deadline_tokens: u64) -> String {
    let target_bytes = token_budget_bytes(target_tokens).max(1024);
    let deadline_bytes = token_budget_bytes(deadline_tokens).max(target_bytes);
    let mut out = String::new();
    write_overview_header(items, &mut out);
    out.push_str("\n## Recent user/assistant/system messages\n");

    let mut selected = Vec::new();
    let mut omitted_messages = 0usize;
    for (idx, item) in items.iter().enumerate().rev() {
        let Some(entry) = message_overview_entry(idx, item, 2_000) else {
            continue;
        };
        let projected = out
            .len()
            .saturating_add(selected.iter().map(String::len).sum::<usize>())
            .saturating_add(entry.len())
            .saturating_add(2);
        if projected > target_bytes && !selected.is_empty() {
            omitted_messages += 1;
            continue;
        }
        selected.push(entry);
        if projected >= target_bytes {
            break;
        }
    }
    selected.reverse();
    for entry in selected {
        out.push_str(&entry);
        out.push_str("\n\n");
    }
    if omitted_messages > 0 {
        out.push_str(&format!(
            "[Overview omitted {omitted_messages} older message(s) to stay near target.]\n\n"
        ));
    }

    append_tool_index(items, &mut out, target_bytes, deadline_bytes);
    out
}

fn build_coarse_summary_overview(items: &[Item], deadline_tokens: u64) -> String {
    let deadline_bytes = token_budget_bytes(deadline_tokens).max(1024);
    let mut out = String::new();
    write_overview_header(items, &mut out);
    out.push_str("\n## Coarse recent message index\n");
    for (idx, item) in items.iter().enumerate().rev() {
        let Some(entry) = message_overview_entry(idx, item, 240) else {
            continue;
        };
        if out.len().saturating_add(entry.len()).saturating_add(2) > deadline_bytes {
            break;
        }
        out.push_str(&entry);
        out.push_str("\n\n");
    }
    out
}

fn write_overview_header(items: &[Item], out: &mut String) {
    let mut messages = 0usize;
    let mut tool_calls = 0usize;
    let mut tool_results = 0usize;
    let mut reasoning = 0usize;
    for item in items {
        match item {
            Item::Message { .. } => messages += 1,
            Item::ToolCall { .. } => tool_calls += 1,
            Item::ToolResult { .. } => tool_results += 1,
            Item::Reasoning { .. } => reasoning += 1,
        }
    }
    out.push_str(&format!(
        "Items summarized: {} total; {messages} message(s), {tool_calls} tool call(s), \
         {tool_results} tool result(s), {reasoning} reasoning item(s). Tool call \
         arguments, tool result full content, and reasoning bodies are omitted from \
         this initial input.\n",
        items.len()
    ));
}

fn append_tool_index(items: &[Item], out: &mut String, target_bytes: usize, deadline_bytes: usize) {
    let mut entries = Vec::new();
    for (idx, item) in items.iter().enumerate().rev() {
        match item {
            Item::ToolCall { name, .. } => entries.push(format!("[{idx} ToolCall] {name}")),
            Item::ToolResult { summary, .. } => entries.push(format!(
                "[{idx} ToolResult] {}",
                truncate_chars(summary, 240)
            )),
            _ => {}
        }
        if entries.len() >= 24 {
            break;
        }
    }
    if entries.is_empty() {
        return;
    }
    entries.reverse();
    out.push_str("## Recent tool index (content omitted)\n");
    for entry in entries {
        let projected = out.len().saturating_add(entry.len()).saturating_add(1);
        if projected > deadline_bytes || (projected > target_bytes && out.contains("ToolResult")) {
            out.push_str("[Additional tool index entries omitted.]\n");
            break;
        }
        out.push_str(&entry);
        out.push('\n');
    }
}

fn message_overview_entry(idx: usize, item: &Item, max_chars: usize) -> Option<String> {
    let Item::Message { role, content, .. } = item else {
        return None;
    };
    let role_label = match role {
        agen::Role::User => "User",
        agen::Role::Assistant => "Assistant",
        agen::Role::System => "System",
    };
    let text: String = content
        .iter()
        .map(|p| p.as_text())
        .collect::<Vec<_>>()
        .join("");
    Some(format!(
        "[{idx} {role_label}] {}",
        truncate_chars(&text, max_chars)
    ))
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text.chars().take(max_chars).collect::<String>();
    out.push_str("… [truncated]");
    out
}

fn estimate_text_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

fn token_budget_bytes(tokens: u64) -> usize {
    tokens.saturating_mul(4).min(usize::MAX as u64) as usize
}

/// Worker errors.
#[derive(Debug, thiserror::Error)]
pub enum RewindError {
    #[error(transparent)]
    Store(#[from] StoreError),

    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug)]
pub struct RewindAppliedState {
    pub entries: Vec<LogEntry>,
    pub input: Vec<Segment>,
    pub summary: RewindSummary,
}

fn build_rewind_targets(segment_id: uuid::Uuid, entries: &[LogEntry]) -> Vec<RewindTarget> {
    let head_entries = entries.len();
    let mut turn_index = 0usize;
    let mut targets = Vec::new();
    for (entry_index, entry) in entries.iter().enumerate() {
        let (segments, ts) = match entry {
            LogEntry::UserInput { segments, ts, .. }
            | LogEntry::AnnotatedUserInput { segments, ts, .. } => (segments, ts),
            _ => continue,
        };
        turn_index += 1;
        let truncate_entries = rewind_truncate_entries(entries, entry_index);
        let tool_warning = suffix_has_tool_side_effects(&entries[truncate_entries..]);
        targets.push(RewindTarget {
            id: RewindTargetId {
                segment_id,
                user_input_entry_index: entry_index,
            },
            expected_head_entries: head_entries,
            truncate_entries,
            turn_index,
            timestamp_ms: Some(*ts),
            preview: preview_segments(segments),
            eligible: true,
            disabled_reason: None,
            warning: tool_warning.then(|| {
                "history suffix will be discarded; tool side effects are not undone".into()
            }),
        });
    }
    targets.reverse();
    targets
}

fn rewind_truncate_entries(entries: &[LogEntry], user_input_entry_index: usize) -> usize {
    if user_input_entry_index > 0
        && matches!(
            entries.get(user_input_entry_index - 1),
            Some(LogEntry::Invoke { .. })
        )
    {
        user_input_entry_index - 1
    } else {
        user_input_entry_index
    }
}

fn suffix_has_tool_side_effects(entries: &[LogEntry]) -> bool {
    entries.iter().any(|entry| match entry {
        LogEntry::ToolResult { .. } | LogEntry::AnnotatedToolResult { .. } => true,
        LogEntry::AssistantItem { item, .. } => logged_item_is_tool_call(item),
        LogEntry::AnnotatedAssistantItem { entry, .. } => logged_item_is_tool_call(&entry.item),
        _ => false,
    })
}

fn logged_item_is_tool_call(item: &session_store::LoggedItem) -> bool {
    matches!(item, session_store::LoggedItem::ToolCall { .. })
}

fn preview_segments(segments: &[Segment]) -> String {
    let mut preview = String::new();
    for segment in segments {
        if !preview.is_empty() {
            preview.push(' ');
        }
        match segment {
            Segment::Text { content } => preview.push_str(content.trim()),
            Segment::Paste { content, .. } => preview.push_str(content.trim()),
            Segment::FileRef { path } => {
                preview.push('@');
                preview.push_str(path);
            }
            Segment::Flow { selector } => {
                preview.push_str("[Flow: ");
                preview.push_str(selector);
                preview.push(']');
            }
            Segment::Unknown => preview.push_str("[unknown input segment]"),
        }
    }
    let preview = preview.replace(['\n', '\r'], " ");
    let mut chars = preview.chars();
    let mut out: String = chars.by_ref().take(120).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

const FLOW_RUNTIME_EXTENSION_DOMAIN: &str = "flow.runtime.v1";

fn restored_flow_runtime_state(
    extensions: &[(String, serde_json::Value)],
) -> Result<Option<flow::FlowRuntimeState>, WorkerError> {
    extensions
        .iter()
        .rev()
        .find(|(domain, _)| domain == FLOW_RUNTIME_EXTENSION_DOMAIN)
        .map(|(_, payload)| {
            serde_json::from_value(payload.clone()).map_err(|error| {
                WorkerError::InvalidState(format!("invalid persisted Flow runtime state: {error}"))
            })
        })
        .transpose()
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    #[error("invalid durable Worker state: {0}")]
    InvalidState(String),

    #[error("Flow input rejected: {0}")]
    FlowInput(String),

    #[error(transparent)]
    Engine(#[from] EngineError),

    #[error(transparent)]
    Store(#[from] StoreError),

    #[error(transparent)]
    WorkerStore(#[from] WorkerStoreError),

    #[error(transparent)]
    Scope(ScopeError),

    #[error("local filesystem authority root is not readable under the configured scope: {}", .root.display())]
    LocalFilesystemRootOutsideScope { root: PathBuf },

    #[error("cwd is not readable under the configured scope: {}", .cwd.display())]
    CwdOutsideScope { cwd: PathBuf },

    #[error("failed to resolve local filesystem authority root {}: {source}", .root.display())]
    InvalidLocalFilesystemRoot {
        root: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to resolve cwd {}: {source}", .cwd.display())]
    InvalidCwd {
        cwd: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse manifest TOML: {0}")]
    ManifestParse(#[source] toml::de::Error),

    #[error("failed to resolve manifest config: {0}")]
    ManifestResolve(#[source] ResolveError),

    #[error(transparent)]
    Provider(#[from] crate::model_client::ProviderError),

    #[error("compaction thrash: context still exceeds threshold immediately after compact")]
    CompactThrash,

    #[error("compact worker did not produce a summary (write_summary was never called)")]
    CompactSummaryMissing,

    #[error("compaction was cancelled")]
    CompactCancelled,

    #[error("compact summary too large: {tokens} tokens exceeds max {max}")]
    CompactSummaryTooLarge { tokens: u64, max: u64 },

    #[error("compacted result context too large: {tokens} tokens exceeds max {max}")]
    CompactResultContextTooLarge { tokens: u64, max: u64 },

    #[error("invalid system prompt template: {source}")]
    InvalidSystemPromptTemplate {
        #[source]
        source: SystemPromptError,
    },

    #[error("failed to render system prompt template: {source}")]
    SystemPromptRender {
        #[source]
        source: SystemPromptError,
    },

    #[error(transparent)]
    ScopeLock(#[from] ScopeLockError),

    #[error(transparent)]
    PromptCatalog(#[from] CatalogError),

    #[error("failed to resolve current Workspace Prompt projection: {message}")]
    WorkspacePromptProjection { message: String },

    #[error(transparent)]
    Skill(#[from] SkillClientError),

    #[error(transparent)]
    WorkspaceMemoryBackend(#[from] WorkspaceMemoryBackendError),

    #[error("feature install failed: {0}")]
    FeatureInstall(String),

    #[error("session {segment_id} has no entries to restore")]
    SegmentEmpty { segment_id: SegmentId },

    #[error("session {segment_id} has no committed system prompt to restore")]
    SegmentSystemPromptMissing { segment_id: SegmentId },

    #[error("worker metadata for {worker_name} was not found")]
    WorkerMetadataMissing { worker_name: String },

    #[error("worker metadata for {worker_name} has no active session")]
    WorkerMetadataInactive { worker_name: String },

    #[error(
        "worker metadata for {worker_name} points to session {session_id} but no segment is materialized yet"
    )]
    WorkerMetadataPending {
        worker_name: String,
        session_id: SessionId,
    },

    #[error(
        "worker metadata for {worker_name} does not include a trusted resolved manifest snapshot; a replacement Worker is required"
    )]
    WorkerMetadataManifestSnapshotMissing { worker_name: String },

    #[error(
        "worker metadata for {worker_name} contains an invalid resolved manifest snapshot: {source}"
    )]
    WorkerMetadataManifestSnapshot {
        worker_name: String,
        #[source]
        source: serde_json::Error,
    },
}

fn workdir_session_from_authority(
    authority: &WorkerFilesystemAuthority,
    scope: &SharedScope,
) -> Option<WorkdirSessionHandle> {
    authority.as_local().map(|local| {
        Arc::new(LocalWorkdirSession::materialized(
            local.root.clone(),
            local.cwd.clone(),
            scope.clone(),
            WorkdirSessionCapabilities::ALL,
        )) as WorkdirSessionHandle
    })
}

/// Bundle of resources that every high-level Worker constructor needs:
/// filesystem authority, path-free workspace context, scope, an LLM client, the prompt catalog,
/// and (optionally) a parsed system-prompt template. Built once by
/// [`prepare_worker_common_with_context`] from the resolved manifest and then split into Worker
/// fields.
struct WorkerCommon {
    filesystem_authority: WorkerFilesystemAuthority,
    workspace_context: WorkerWorkspaceContext,
    scope: Scope,
    delegation_scope: DelegationScope,
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    system_prompt_template: Option<SystemPromptTemplate>,
    feature_instructions: Vec<FeatureInstructionDeclaration>,
}

async fn restored_child_reachable(child: &WorkerSpawnedChild) -> bool {
    tokio::time::timeout(
        RESTORE_RECONCILIATION_REACHABILITY_TIMEOUT,
        UnixStream::connect(&child.socket_path),
    )
    .await
    .map(|result| result.is_ok())
    .unwrap_or(false)
}

fn spawned_child_scope_rules(child: &WorkerSpawnedChild) -> Vec<ScopeRule> {
    child
        .scope_delegated
        .iter()
        .filter_map(|rule| delegated_scope_rule_to_scope_rule(rule.clone()))
        .collect()
}

fn delegated_scope_rule_to_scope_rule(rule: WorkerSpawnedScopeRule) -> Option<ScopeRule> {
    let permission = match rule.permission.as_str() {
        "read" => Permission::Read,
        "write" => Permission::Write,
        other => {
            warn!(permission = %other, "ignoring invalid delegated child scope permission");
            return None;
        }
    };
    Some(ScopeRule {
        target: rule.target,
        permission,
        recursive: rule.recursive,
    })
}

fn effective_restore_scope_config<St>(
    store: &St,
    manifest: &WorkerManifest,
) -> Result<ScopeConfig, WorkerStoreError>
where
    St: WorkerMetadataStore,
{
    let mut scope = manifest.scope.clone();
    let Some(metadata) = store.read_by_name(&manifest.worker.name)? else {
        return Ok(scope);
    };
    for child in metadata.spawned_children {
        for rule in child.scope_delegated {
            if let Some(deny) = delegated_write_rule_to_deny(rule) {
                scope.deny.push(deny);
            }
        }
    }
    Ok(scope)
}

fn delegated_write_rule_to_deny(rule: WorkerSpawnedScopeRule) -> Option<ScopeRule> {
    let rule = delegated_scope_rule_to_scope_rule(rule)?;
    (rule.permission == Permission::Write).then_some(rule)
}

/// Build the runtime pieces that are derivable directly from the resolved
/// manifest. Used by new, spawned, and restored Workers so they share one
/// definition of "what pieces fall out of a manifest".
///
/// `parse_template` controls whether the manifest's instruction is parsed as a
/// system-prompt template. New Workers always parse so the template is rendered at
/// first turn; restored Workers skip parsing because the saved session log replays
/// a previously-rendered `system_prompt` verbatim.
fn prepare_worker_common_with_context(
    manifest: &WorkerManifest,
    loader: &PromptCatalogSource,
    parse_template: bool,
    workspace_context: WorkerWorkspaceContext,
    filesystem_authority: WorkerFilesystemAuthority,
    scope_config: ScopeConfig,
) -> Result<WorkerCommon, WorkerError> {
    prepare_worker_common_with_context_and_model_client(
        manifest,
        loader,
        parse_template,
        workspace_context,
        filesystem_authority,
        scope_config,
        None,
    )
}

fn prepare_worker_common_with_context_and_model_client(
    manifest: &WorkerManifest,
    loader: &PromptCatalogSource,
    parse_template: bool,
    workspace_context: WorkerWorkspaceContext,
    filesystem_authority: WorkerFilesystemAuthority,
    scope_config: ScopeConfig,
    model_client: Option<Box<dyn LlmClient>>,
) -> Result<WorkerCommon, WorkerError> {
    let filesystem_authority = match filesystem_authority {
        WorkerFilesystemAuthority::None => WorkerFilesystemAuthority::None,
        WorkerFilesystemAuthority::Local(local) => {
            let root = std::fs::canonicalize(&local.root).map_err(|source| {
                WorkerError::InvalidLocalFilesystemRoot {
                    root: local.root.clone(),
                    source,
                }
            })?;
            let cwd =
                std::fs::canonicalize(&local.cwd).map_err(|source| WorkerError::InvalidCwd {
                    cwd: local.cwd.clone(),
                    source,
                })?;
            WorkerFilesystemAuthority::Local(LocalWorkingDirectory { root, cwd })
        }
    };
    let mut scope_config = scope_config;
    if let (Some(mem), Some(local)) = (manifest.memory.as_ref(), filesystem_authority.as_local()) {
        let layout = memory::WorkspaceLayout::resolve(mem, &local.root);
        scope_config.deny.extend(memory::deny_write_rules(&layout));
    }
    let scope = if scope_config.allow.is_empty() && filesystem_authority.as_local().is_none() {
        Scope::empty()
    } else {
        Scope::from_config(&scope_config).map_err(WorkerError::Scope)?
    };
    prepare_worker_common_from_scope(
        manifest,
        loader,
        parse_template,
        workspace_context,
        filesystem_authority,
        scope,
        model_client,
    )
}

fn prepare_worker_common_from_scope(
    manifest: &WorkerManifest,
    loader: &PromptCatalogSource,
    parse_template: bool,
    workspace_context: WorkerWorkspaceContext,
    filesystem_authority: WorkerFilesystemAuthority,
    scope: Scope,
    model_client: Option<Box<dyn LlmClient>>,
) -> Result<WorkerCommon, WorkerError> {
    if let Some(local) = filesystem_authority.as_local() {
        if !scope.is_readable(&local.root) {
            return Err(WorkerError::LocalFilesystemRootOutsideScope {
                root: local.root.clone(),
            });
        }
        if !scope.is_readable(&local.cwd) {
            return Err(WorkerError::CwdOutsideScope {
                cwd: local.cwd.clone(),
            });
        }
    }
    let delegation_scope =
        DelegationScope::from_config(&manifest.delegation_scope).map_err(WorkerError::Scope)?;

    let client = match model_client {
        Some(client) => client,
        None => crate::model_client::build_client(&manifest.model)?,
    };
    let prompts = Arc::new(ArcSwap::from(PromptCatalog::load(loader)?));
    let system_prompt_template = if parse_template {
        Some(
            SystemPromptTemplate::parse(&manifest.engine.instruction, loader.clone())
                .map_err(|source| WorkerError::InvalidSystemPromptTemplate { source })?,
        )
    } else {
        None
    };

    Ok(WorkerCommon {
        filesystem_authority,
        workspace_context,
        scope,
        delegation_scope,
        client,
        prompts,
        system_prompt_template,
        feature_instructions: Vec::new(),
    })
}

/// Snapshot the process's current working directory as the Worker's cwd,
/// canonicalising symlinks and any `.`/`..` components. The Worker keeps
/// this value for its lifetime; changes to the process-wide cwd after
/// construction do not affect scope checks or the system prompt.
fn current_cwd() -> Result<PathBuf, WorkerError> {
    let cwd = std::env::current_dir().map_err(|source| WorkerError::InvalidCwd {
        cwd: PathBuf::from("."),
        source,
    })?;
    cwd.canonicalize()
        .map_err(|source| WorkerError::InvalidCwd { cwd: cwd, source })
}

#[cfg(test)]
mod spawned_context_tests {
    use super::*;

    #[test]
    fn spawn_worker_context_separates_workspace_identity_from_tool_pwd() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_root = tmp.path().join("workspace-root");
        let cwd = tmp.path().join("child-worktree");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        let mut manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
        manifest.memory = Some(manifest::MemoryConfig::default());
        let common = prepare_worker_common_with_context(
            &manifest,
            &PromptCatalogSource::builtins_only(),
            false,
            WorkerWorkspaceContext::local_filesystem(Some(WorkspaceId::new("ws-test").unwrap())),
            WorkerFilesystemAuthority::local(workspace_root.clone(), cwd.clone()),
            manifest.scope.clone(),
        )
        .unwrap();

        assert_eq!(
            common
                .workspace_context
                .workspace_id()
                .map(WorkspaceId::as_str),
            Some("ws-test")
        );
        assert_eq!(
            common.filesystem_authority.as_local().unwrap().root,
            workspace_root.canonicalize().unwrap()
        );
        assert_eq!(
            common.filesystem_authority.as_local().unwrap().cwd,
            cwd.canonicalize().unwrap()
        );
    }

    #[test]
    fn workspace_identity_and_client_do_not_grant_filesystem_authority() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_root = tmp.path().join("workspace-root");
        let cwd = workspace_root.join("nested");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
        manifest.memory = Some(manifest::MemoryConfig::default());
        let loader = PromptCatalogSource::builtins_only();
        let workspace_id = WorkspaceId::new("ws-api-only").unwrap();
        let common = prepare_worker_common_with_context(
            &manifest,
            &loader,
            false,
            WorkerWorkspaceContext::with_client(
                Some(workspace_id.clone()),
                marker_workspace_client(Some(&workspace_id), "test-api"),
            ),
            WorkerFilesystemAuthority::None,
            manifest.scope.clone(),
        )
        .unwrap();

        assert_eq!(common.filesystem_authority, WorkerFilesystemAuthority::None);
        assert_eq!(
            common
                .workspace_context
                .workspace_id()
                .map(WorkspaceId::as_str),
            Some(workspace_id.as_str())
        );
        assert!(common.workspace_context.client().is_available());
    }

    #[test]
    fn prepare_context_reports_local_filesystem_root_when_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_root = tmp.path().join("workspace-root");
        let cwd = tmp.path().join("child-worktree");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        let manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
        let err = match prepare_worker_common_with_context(
            &manifest,
            &PromptCatalogSource::builtins_only(),
            false,
            WorkerWorkspaceContext::local_filesystem(Some(WorkspaceId::new("ws-test").unwrap())),
            WorkerFilesystemAuthority::local(workspace_root.clone(), cwd.clone()),
            ScopeConfig {
                allow: vec![ScopeRule {
                    target: cwd.clone(),
                    permission: Permission::Read,
                    recursive: true,
                }],
                deny: Vec::new(),
            },
        ) {
            Ok(_) => panic!("expected local filesystem root scope error"),
            Err(err) => err,
        };

        match err {
            WorkerError::LocalFilesystemRootOutsideScope { root: got } => {
                assert_eq!(got, workspace_root.canonicalize().unwrap());
            }
            other => panic!("expected local filesystem root scope error, got {other:?}"),
        }
    }

    #[test]
    fn prepare_context_reports_cwd_when_only_cwd_is_unreadable() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace_root = tmp.path().join("workspace-root");
        let cwd = tmp.path().join("child-worktree");
        std::fs::create_dir_all(&workspace_root).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();

        let manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
        let err = match prepare_worker_common_with_context(
            &manifest,
            &PromptCatalogSource::builtins_only(),
            false,
            WorkerWorkspaceContext::local_filesystem(Some(WorkspaceId::new("ws-test").unwrap())),
            WorkerFilesystemAuthority::local(workspace_root.clone(), cwd.clone()),
            ScopeConfig {
                allow: vec![ScopeRule {
                    target: workspace_root.clone(),
                    permission: Permission::Read,
                    recursive: true,
                }],
                deny: Vec::new(),
            },
        ) {
            Ok(_) => panic!("expected cwd scope error"),
            Err(err) => err,
        };

        match err {
            WorkerError::CwdOutsideScope { cwd: got } => {
                assert_eq!(got, cwd.canonicalize().unwrap());
            }
            other => panic!("expected cwd scope error, got {other:?}"),
        }
    }

    fn minimal_manifest_for_context_test(workspace_root: &Path, cwd: &Path) -> WorkerManifest {
        let toml_str = format!(
            r#"
[worker]
name = "spawn-context-test"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]

[[scope.allow]]
target = "{}"
permission = "read"

[[scope.allow]]
target = "{}"
permission = "write"
"#,
            workspace_root.display(),
            cwd.display()
        );
        let mut manifest = WorkerManifest::from_toml(&toml_str).unwrap();
        manifest.model.auth = Some(manifest::AuthRef::None);
        manifest
    }
}

#[cfg(test)]
mod worker_metadata_restore_manifest_tests {
    use super::*;

    #[test]
    fn metadata_writer_persists_workspace_id_through_store_update() {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap();
        let writer = worker_metadata_writer_for_store(&store);

        writer(WorkerMetadata::new("runtime-worker", None).with_workspace_id("ws-test")).unwrap();

        let stored = store.read_by_name("runtime-worker").unwrap().unwrap();
        assert_eq!(stored.workspace_id.as_deref(), Some("ws-test"));
    }

    #[test]
    fn snapshot_preserves_saved_scope_over_current_manifest() {
        let saved = WorkerManifest::from_toml(
            r#"
[worker]
name = "restore-scope"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
instruction = "saved"

[feature.merge_request]
show = true
open = false
review = true
readiness_check = false
complete = false

[[scope.allow]]
target = "/snapshot/workspace"
permission = "read"

[[delegation_scope.allow]]
target = "/snapshot/workspace/.worktree"
permission = "write"
"#,
        )
        .unwrap();
        let current = WorkerManifest::from_toml(
            r#"
[worker]
name = "restore-scope"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
instruction = "current"

[feature.merge_request]
show = true
open = true
review = true
readiness_check = true
complete = true

[[scope.allow]]
target = "/current/workspace"
permission = "write"

[[delegation_scope.allow]]
target = "/current/workspace"
permission = "write"
"#,
        )
        .unwrap();

        let restored = restore_manifest_from_worker_metadata_snapshot(
            "restore-scope",
            Some(serde_json::to_value(&saved).unwrap()),
            current,
        )
        .unwrap();

        assert_eq!(restored.engine.instruction, "saved");
        assert_eq!(
            restored.feature.merge_request,
            manifest::MergeRequestFeatureConfig {
                show: true,
                review: true,
                ..Default::default()
            }
        );
        assert_eq!(restored.scope.allow.len(), 1);
        assert_eq!(
            restored.scope.allow[0].target,
            std::path::PathBuf::from("/snapshot/workspace")
        );
        assert_eq!(restored.scope.allow[0].permission, Permission::Read);
        assert_eq!(restored.delegation_scope.allow.len(), 1);
        assert_eq!(
            restored.delegation_scope.allow[0].target,
            std::path::PathBuf::from("/snapshot/workspace/.worktree")
        );
        assert_eq!(
            restored.delegation_scope.allow[0].permission,
            Permission::Write
        );
    }

    #[test]
    fn workspace_memory_settings_snapshot_is_persisted_and_scope_checked() {
        let mut manifest = WorkerManifest::from_toml(
            r#"
[worker]
name = "memory-snapshot"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
instruction = "default"

[[scope.allow]]
target = "/workspace"
permission = "read"

[[delegation_scope.allow]]
target = "/workspace"
permission = "read"
"#,
        )
        .unwrap();
        manifest.memory = Some(manifest::MemoryConfig::default());
        manifest.memory.as_mut().unwrap().bind_workspace_settings(
            &manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-a".to_string(),
                settings_revision: 7,
                language: "Japanese".to_string(),
            },
        );

        let metadata = worker_metadata_for_manifest(&manifest, None, None, None);
        let restored: WorkerManifest = serde_json::from_value(
            metadata
                .resolved_manifest_snapshot
                .expect("Memory settings require a resolved manifest snapshot"),
        )
        .unwrap();
        assert_eq!(
            restored.memory.unwrap().workspace_settings(),
            Some(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-a".to_string(),
                settings_revision: 7,
                language: "Japanese".to_string(),
            })
        );
        assert!(
            validate_workspace_memory_snapshot(
                "memory-snapshot",
                &manifest,
                &WorkerWorkspaceContext::unavailable(
                    Some(WorkspaceId::new("workspace-a").unwrap()),
                    "test",
                )
            )
            .is_ok()
        );
        assert!(
            validate_workspace_memory_snapshot(
                "memory-snapshot",
                &manifest,
                &WorkerWorkspaceContext::unavailable(
                    Some(WorkspaceId::new("workspace-b").unwrap()),
                    "test",
                )
            )
            .is_err()
        );

        let mut missing = manifest.clone();
        missing.memory.as_mut().unwrap().settings_revision = None;
        assert!(
            validate_workspace_memory_snapshot(
                "memory-snapshot",
                &missing,
                &WorkerWorkspaceContext::unavailable(
                    Some(WorkspaceId::new("workspace-a").unwrap()),
                    "test",
                )
            )
            .is_err()
        );
    }

    #[test]
    fn plugin_resolved_manifest_snapshot_is_persisted_without_profile() {
        let mut manifest = WorkerManifest::from_toml(
            r#"
[worker]
name = "plugin-snapshot"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]
instruction = "saved"

[[scope.allow]]
target = "/snapshot/workspace"
permission = "read"
"#,
        )
        .unwrap();
        assert!(manifest.profile.is_none());
        assert!(
            worker_metadata_for_manifest(&manifest, None, None, None)
                .resolved_manifest_snapshot
                .is_none()
        );

        manifest.plugins.resolved = vec![manifest::plugin::ResolvedPluginRecord {
            identity: manifest::plugin::SourceQualifiedPluginId::new(
                manifest::plugin::PluginSourceKind::Project,
                "example",
            ),
            source: manifest::plugin::PluginSourceKind::Project,
            package_path: PathBuf::from("/snapshot/workspace/.yoi/plugins/example.yoi-plugin"),
            package_label: "example.yoi-plugin".to_string(),
            digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                .to_string(),
            version: "0.1.0".to_string(),
            manifest: manifest::plugin::PluginPackageManifest {
                schema_version: 1,
                id: "example".to_string(),
                name: "Example".to_string(),
                version: "0.1.0".to_string(),
                description: None,
                surfaces: vec![manifest::plugin::PluginSurface::Hook],
                runtime: None,
                hooks: vec![],
                tools: vec![],
                services: vec![],
                ingresses: vec![],
                permissions: vec![],
                request: vec![],
                websocket: vec![],
            },
            enabled_surfaces: vec![manifest::plugin::PluginSurface::Hook],
            grants: manifest::plugin::PluginGrantConfig::default(),
            config: None,
        }];

        let metadata = worker_metadata_for_manifest(&manifest, None, None, None);
        let snapshot = metadata
            .resolved_manifest_snapshot
            .expect("plugin-resolved manifest should be snapshotted");
        let restored: WorkerManifest = serde_json::from_value(snapshot).unwrap();

        assert!(restored.profile.is_none());
        assert_eq!(restored.plugins.resolved.len(), 1);
        assert_eq!(
            restored.plugins.resolved[0].digest,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(restored.plugins.resolved[0].version, "0.1.0");
    }
}

#[cfg(test)]
mod memory_worker_event_tests {
    use super::*;

    #[test]
    fn suppresses_idle_consolidation_skip_worker_events() {
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "no_staging_entries",
        ));
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "threshold_not_reached files=1 bytes=64 min_files=2 min_bytes=1048576",
        ));
        assert!(!should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "consolidation_threshold_disabled",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "no_valid_staging_entries invalid=1",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryConsolidation,
            memory::audit::WorkerLifecycleStatus::Completed,
            "completed",
        ));
        assert!(should_emit_memory_worker_event(
            memory::audit::AuditWorker::MemoryExtract,
            memory::audit::WorkerLifecycleStatus::Skipped,
            "threshold_not_reached files=1",
        ));
    }
}

#[cfg(test)]
mod build_summary_prompt_tests {
    use super::*;

    fn test_summary_input(items: &[Item]) -> String {
        build_summary_input(
            items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 512,
                overview_warning_tokens: 1024,
                overview_deadline_tokens: 2048,
                summary_target_tokens: 256,
            },
        )
        .text
    }

    #[test]
    fn strips_tool_call_arguments() {
        let items = vec![Item::tool_call_json(
            "call-1",
            "read_file",
            serde_json::json!({ "path": "src/main.rs" }),
        )];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 ToolCall] read_file"));
        assert!(!prompt.contains("src/main.rs"));
    }

    #[test]
    fn strips_tool_result_content() {
        let items = vec![Item::tool_result_with_content(
            "call-1",
            "read 3 lines",
            "fn main() { println!(\"hello\"); }",
        )];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 ToolResult] read 3 lines"));
        assert!(!prompt.contains("println"));
    }

    #[test]
    fn drops_reasoning_entirely() {
        let items = vec![
            Item::user_message("hi"),
            Item::reasoning("internal deliberation"),
            Item::assistant_message("hello"),
        ];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 User] hi"));
        assert!(prompt.contains("[2 Assistant] hello"));
        assert!(!prompt.contains("Reasoning"));
        assert!(!prompt.contains("deliberation"));
    }

    #[test]
    fn overview_warning_does_not_drop_input() {
        let items = vec![Item::user_message("x".repeat(4_000))];
        let built = build_summary_input(
            &items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 10,
                overview_warning_tokens: 100,
                overview_deadline_tokens: 2_000,
                summary_target_tokens: 256,
            },
        );
        assert!(built.warning_exceeded);
        assert!(!built.deadline_fallback_used);
        assert!(built.text.contains("[0 User]"));
    }

    #[test]
    fn overview_deadline_falls_back_to_coarse_index() {
        let items = vec![Item::user_message("x".repeat(4_000))];
        let built = build_summary_input(
            &items,
            &[],
            None,
            SummaryInputOptions {
                overview_target_tokens: 10,
                overview_warning_tokens: 10,
                overview_deadline_tokens: 100,
                summary_target_tokens: 256,
            },
        );
        assert!(built.deadline_fallback_used);
        assert!(built.text.contains("## Coarse recent message index"));
    }

    #[test]
    fn engine_manifest_generation_settings_become_request_config() {
        let manifest = manifest::EngineManifest {
            instruction: "unused".into(),
            language: manifest::defaults::WORKER_LANGUAGE.into(),
            max_tokens: Some(1024),
            max_turns: None,
            temperature: Some(0.2),
            top_p: Some(0.9),
            top_k: Some(40),
            stop_sequences: vec!["\n\n".into(), "</stop>".into()],
            reasoning: None,
            tool_output: manifest::ToolOutputLimits::default(),
            file_upload: manifest::FileUploadLimits::default(),
        };

        let config = request_config_from_engine_manifest(&manifest);

        assert_eq!(config.max_tokens, Some(1024));
        assert_eq!(config.temperature, Some(0.2));
        assert_eq!(config.top_p, Some(0.9));
        assert_eq!(config.top_k, Some(40));
        assert_eq!(config.stop_sequences, vec!["\n\n", "</stop>"]);
    }

    #[test]
    fn keeps_user_and_assistant_messages() {
        let items = vec![
            Item::user_message("fix the bug"),
            Item::assistant_message("done"),
        ];
        let prompt = test_summary_input(&items);
        assert!(prompt.contains("[0 User] fix the bug"));
        assert!(prompt.contains("[1 Assistant] done"));
    }

    #[derive(Clone)]
    struct CancelBeforeAiExtractClient {
        cancel_tx: Arc<Mutex<Option<tokio::sync::mpsc::Sender<()>>>>,
    }

    #[async_trait]
    impl LlmClient for CancelBeforeAiExtractClient {
        async fn stream(
            &self,
            _request: agen::llm_client::Request,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                agen::llm_client::event::Event,
                                agen::llm_client::ClientError,
                            >,
                        > + Send,
                >,
            >,
            agen::llm_client::ClientError,
        > {
            let tx = self
                .cancel_tx
                .lock()
                .expect("cancel sender lock")
                .clone()
                .expect("extract caller must install the Internal Worker cancel sender");
            tx.send(()).await.expect("cancel Internal Worker");
            Ok(Box::pin(futures::stream::pending()))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    #[derive(Debug, Default)]
    struct RecordingAuditWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl RecordingAuditWorkspaceClient {
        fn lifecycle_audits(&self) -> Vec<memory::audit::WorkerLifecycleAudit> {
            self.requests
                .lock()
                .expect("recorded workspace requests lock")
                .iter()
                .filter_map(|request| {
                    let operation: memory::backend::MemoryBackendOperation = serde_json::from_str(
                        request
                            .body
                            .as_deref()
                            .expect("memory backend operation body"),
                    )
                    .expect("memory backend operation");
                    match operation {
                        memory::backend::MemoryBackendOperation::AppendAudit(operation) => {
                            match operation.event.payload {
                                memory::audit::AuditPayload::WorkerLifecycle(audit) => Some(audit),
                                _ => None,
                            }
                        }
                        _ => None,
                    }
                })
                .collect()
        }
    }

    impl WorkspaceClient for RecordingAuditWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "recording-audit"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests
                .lock()
                .expect("recorded workspace requests lock")
                .push(request);
            Err(WorkspaceClientError::Unavailable(
                "audit response is irrelevant to this regression test".to_string(),
            ))
        }
    }

    #[derive(Debug, Default)]
    struct FlowSourceWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for FlowSourceWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "flow-source-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests
                .lock()
                .expect("Flow source request lock")
                .push(request);
            let definition = flow::compile_flow_source(
                r#"{
                    schema_version = 1;
                    name = "coder-review";
                    initial = "implement";
                    states = {
                        implement = {
                            instructions = "Implement the Ticket and request review.";
                            transitions = {
                                done = { target = "done"; condition = "The work is approved."; };
                            };
                        };
                        done = { instructions = "Complete."; terminal = true; };
                    };
                }"#,
            )
            .unwrap();
            let source = flow::ResolvedFlowSource {
                selector: "builtin:coder-review".parse().unwrap(),
                workspace_id: "workspace-test".to_string(),
                flow_id: "flow-source-1".to_string(),
                revision: 3,
                content_digest: definition.content_digest.clone(),
                definition,
            };
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&source).unwrap(),
            })
        }
    }

    #[derive(Clone)]
    struct PauseResumeClient {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PauseResumeClient {
        fn new() -> Self {
            Self {
                calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait::async_trait]
    impl agen::llm_client::LlmClient for PauseResumeClient {
        async fn stream(
            &self,
            _request: agen::llm_client::Request,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<agen::llm_client::Event, agen::llm_client::ClientError>,
                        > + Send,
                >,
            >,
            agen::llm_client::ClientError,
        > {
            use agen::llm_client::{Event, ResponseStatus, StatusEvent};
            let call = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let events = if call == 0 {
                vec![
                    Event::tool_use_start(0, "call_pending", "pending_once"),
                    Event::tool_input_delta(0, r#"{}"#),
                    Event::tool_use_stop(0),
                    Event::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]
            } else {
                vec![
                    Event::text_block_start(0),
                    Event::text_delta(0, "done"),
                    Event::text_block_stop(0, None),
                    Event::Status(StatusEvent {
                        status: ResponseStatus::Completed,
                    }),
                ]
            };
            Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
        }

        fn clone_boxed(&self) -> Box<dyn agen::llm_client::LlmClient> {
            Box::new(self.clone())
        }
    }

    #[derive(Clone)]
    struct CountingPendingTool {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl agen::tool::Tool for CountingPendingTool {
        async fn execute(
            &self,
            _input_json: &str,
            _ctx: agen::ToolExecutionContext,
        ) -> Result<agen::tool::ToolOutput, agen::tool::ToolError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("executed once".to_string().into())
        }
    }

    fn counting_pending_tool(
        calls: Arc<std::sync::atomic::AtomicUsize>,
    ) -> agen::tool::ToolDefinition {
        Arc::new(move || {
            let meta = agen::tool::ToolMeta::new("pending_once")
                .description("Counts resumable pending execution")
                .input_schema(serde_json::json!({"type": "object"}));
            (
                meta,
                Arc::new(CountingPendingTool {
                    calls: calls.clone(),
                }) as Arc<dyn agen::tool::Tool>,
            )
        })
    }

    #[derive(Clone)]
    struct PauseOnceHook {
        should_pause: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl crate::hook::Hook<crate::hook::PreToolCall> for PauseOnceHook {
        async fn call(
            &self,
            _input: &crate::hook::ToolCallSummary,
        ) -> crate::hook::HookPreToolAction {
            if self
                .should_pause
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                crate::hook::HookPreToolAction::Pause
            } else {
                crate::hook::HookPreToolAction::Continue
            }
        }
    }

    #[derive(Clone)]
    struct NoopClient;

    #[async_trait]
    impl LlmClient for NoopClient {
        async fn stream(
            &self,
            _request: agen::llm_client::Request,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                agen::llm_client::event::Event,
                                agen::llm_client::ClientError,
                            >,
                        > + Send,
                >,
            >,
            agen::llm_client::ClientError,
        > {
            Ok(Box::pin(futures::stream::empty()))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    fn text_segment(text: &str) -> Segment {
        Segment::Text {
            content: text.into(),
        }
    }

    #[tokio::test]
    async fn fresh_run_clears_interrupted_budget_before_pre_run_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        worker.last_run_interrupted = true;
        worker.engine_mut().set_active_run_turn_count(Some(3));

        worker.prepare_interrupted_history_for_fresh_run().unwrap();

        assert!(!worker.last_run_interrupted);
        assert_eq!(worker.engine().active_run_turn_count(), None);
        let checkpoint = active_run_checkpoint_entry(
            worker.engine().active_run_turn_count(),
            worker.engine().turn_count(),
        );
        assert!(checkpoint.is_none());

        let mut replacement_entries = vec![LogEntry::SegmentStart {
            ts: segment_log::now_millis(),
            session_id: uuid::Uuid::nil(),
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        }];
        replacement_entries.extend(checkpoint);
        let restored = session_store::collect_state(&replacement_entries);
        assert!(!restored.last_run_interrupted);
        assert_eq!(restored.active_run_turn_count, None);
    }

    #[tokio::test]
    async fn auto_fork_checkpoints_interrupted_run_budget_for_restore() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        worker.engine_mut().set_turn_count(7);
        worker.last_run_interrupted = true;
        worker.engine_mut().set_active_run_turn_count(Some(3));

        let session_id = worker.session_id();
        let source_segment_id = worker.segment_id();
        worker
            .store()
            .append(
                session_id,
                source_segment_id,
                &LogEntry::Extension {
                    ts: segment_log::now_millis(),
                    domain: "test.auto_fork_drift".into(),
                    payload: serde_json::json!({}),
                },
            )
            .unwrap();

        worker.ensure_segment_head().unwrap();

        let fork_segment_id = worker.segment_id();
        assert_ne!(fork_segment_id, source_segment_id);
        let fork_entries = worker
            .store()
            .read_all(session_id, fork_segment_id)
            .unwrap();
        assert!(matches!(
            fork_entries.as_slice(),
            [
                LogEntry::AnnotatedSegmentStart { .. },
                LogEntry::ActiveRunCheckpoint {
                    active_turn_count: 3,
                    total_turn_count: 7,
                    ..
                }
            ]
        ));
        let restored = session_store::collect_state(&fork_entries);
        assert!(restored.last_run_interrupted);
        assert_eq!(restored.turn_count, 7);
        assert_eq!(restored.active_run_turn_count, Some(3));
    }

    #[tokio::test]
    async fn flow_transition_feature_installs_runtime_local_coordinator() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let workspace_client = Arc::new(RecordingAuditWorkspaceClient::default());
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                workspace_client,
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        let report = worker
            .install_runtime_flow_transition_feature()
            .expect("scoped Workspace Flow feature");
        assert_eq!(report.reports.len(), 1);
        assert!(report.reports[0].installed);
        assert_eq!(
            report.reports[0]
                .installed_tools
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["RequestFlowTransition"]
        );
    }

    #[tokio::test]
    async fn flow_submit_persists_runtime_state_atomically_with_worker_input() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let workspace_client = Arc::new(FlowSourceWorkspaceClient::default());
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                workspace_client.clone(),
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        let disabled = worker.prepare_flow_input(vec![Segment::Flow {
            selector: "builtin:coder-review".to_string(),
        }]);
        assert!(
            matches!(disabled, Err(WorkerError::FlowInput(message)) if message.contains("feature.flow"))
        );
        worker.install_runtime_flow_transition_feature().unwrap();
        let multiple = worker.prepare_flow_input(vec![
            Segment::Flow {
                selector: "builtin:coder-review".to_string(),
            },
            Segment::Flow {
                selector: "workspace:coder-review".to_string(),
            },
        ]);
        assert!(
            matches!(multiple, Err(WorkerError::FlowInput(message)) if message.contains("at most one"))
        );
        let invalid = worker.prepare_flow_input(vec![Segment::Flow {
            selector: "coder-review".to_string(),
        }]);
        assert!(matches!(invalid, Err(WorkerError::FlowInput(_))));

        let (segments, state, projection) = worker
            .prepare_flow_input(vec![
                Segment::Flow {
                    selector: "builtin:coder-review".to_string(),
                },
                Segment::text("Implement Ticket 00001"),
            ])
            .unwrap();
        let state = state.expect("new Flow runtime state");
        let extension = SessionExtension::new(
            FLOW_RUNTIME_EXTENSION_DOMAIN,
            serde_json::to_value(&state).unwrap(),
        );
        let projected = worker.projected_input_history(&segments, projection.as_ref());
        worker
            .commit_entry(LogEntry::AnnotatedUserInput {
                ts: segment_log::now_millis(),
                segments: segments.clone(),
                extensions: vec![extension],
                history: projected.iter().map(to_logged_history_entry).collect(),
            })
            .unwrap();
        *worker
            .flow_runtime_state
            .lock()
            .expect("flow runtime state lock") = Some(state.clone());

        assert!(matches!(
            &segments[0],
            Segment::Flow { selector } if selector == "builtin:coder-review"
        ));
        assert_eq!(segments[1], Segment::text("Implement Ticket 00001"));
        assert_eq!(
            projected[0].item.as_text().as_deref(),
            Some("Implement the Ticket and request review.")
        );
        assert!(matches!(
            projected[0].annotation.origin,
            WorkerHistoryProvenance::FlowInstruction { .. }
        ));
        assert_eq!(state.instance.definition_revision, 3);
        assert_eq!(state.instance.current_state.as_str(), "implement");
        assert_eq!(workspace_client.requests.lock().unwrap().len(), 1);

        let location = worker.segment_state.location();
        let restored = session_store::collect_state(
            &store
                .read_all(location.session_id, location.segment_id)
                .unwrap(),
        );
        let restored_flow = restored_flow_runtime_state(&restored.extensions)
            .unwrap()
            .expect("restored Flow state");
        assert_eq!(restored_flow, state);
        assert_eq!(restored.user_segments, vec![segments]);

        let duplicate = worker.prepare_flow_input(vec![Segment::Flow {
            selector: "builtin:coder-review".to_string(),
        }]);
        assert!(
            matches!(duplicate, Err(WorkerError::FlowInput(message)) if message.contains("active Flow"))
        );

        let mut detached = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            session_store::FsStore::new(dir.path().join("detached-sessions")).unwrap(),
            WorkerWorkspaceContext::unavailable(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                "test unavailable",
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        detached.install_runtime_flow_transition_feature().unwrap();
        let unavailable = detached.prepare_flow_input(vec![Segment::Flow {
            selector: "builtin:coder-review".to_string(),
        }]);
        assert!(
            matches!(unavailable, Err(WorkerError::FlowInput(message)) if message.contains("unavailable"))
        );
    }

    async fn rewind_test_worker() -> (
        tempfile::TempDir,
        Worker<NoopClient, session_store::FsStore>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::local_filesystem(None),
            authority,
            scope,
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        (dir, worker)
    }

    fn append_test_entry(worker: &Worker<NoopClient, session_store::FsStore>, entry: LogEntry) {
        let loc = worker.segment_state.location();
        worker
            .store
            .append(loc.session_id, loc.segment_id, &entry)
            .unwrap();
    }

    fn append_annotated_user_turn(
        worker: &Worker<NoopClient, session_store::FsStore>,
        ts: u64,
        text: &str,
    ) -> Vec<SessionHistoryMetadata> {
        let user = history_entry(
            Item::user_message(text),
            WorkerHistoryProvenance::HumanInput {
                account_id: "account-1".into(),
            },
        );
        let assistant = history_entry(
            Item::assistant_message(format!("answer: {text}")),
            WorkerHistoryProvenance::ModelOutput {
                worker: worker_subject(worker.session.session_id()),
            },
        );
        append_test_entry(
            worker,
            LogEntry::Invoke {
                ts,
                trigger: protocol::InvokeKind::UserSend,
            },
        );
        append_test_entry(
            worker,
            LogEntry::AnnotatedUserInput {
                ts: ts + 1,
                segments: vec![Segment::text(text)],
                extensions: Vec::new(),
                history: vec![to_logged_history_entry(&user)],
            },
        );
        append_test_entry(
            worker,
            LogEntry::AnnotatedAssistantItem {
                ts: ts + 2,
                entry: to_logged_history_entry(&assistant),
            },
        );
        append_test_entry(
            worker,
            LogEntry::TurnEnd {
                ts: ts + 3,
                turn_count: 1,
            },
        );
        vec![user.annotation, assistant.annotation]
    }

    fn append_user_turn(worker: &Worker<NoopClient, session_store::FsStore>, ts: u64, text: &str) {
        append_test_entry(
            worker,
            LogEntry::Invoke {
                ts,
                trigger: protocol::InvokeKind::UserSend,
            },
        );
        append_test_entry(
            worker,
            LogEntry::UserInput {
                ts: ts + 1,
                extensions: vec![],
                segments: vec![text_segment(text)],
            },
        );
        append_test_entry(
            worker,
            LogEntry::TurnEnd {
                ts: ts + 2,
                turn_count: 1,
            },
        );
    }

    #[tokio::test]
    async fn rewind_target_listing_is_newest_first_and_warns_on_tool_suffix() {
        let (_dir, worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        append_user_turn(&worker, 20, "second message");
        append_test_entry(
            &worker,
            LogEntry::ToolResult {
                ts: 30,
                item: session_store::LoggedItem::ToolResult {
                    call_id: "call-1".into(),
                    summary: "wrote a file".into(),
                    content: None,
                    attachments: Vec::new(),
                    disposition: Default::default(),
                    is_error: false,
                },
            },
        );

        let (head_entries, targets) = worker.list_rewind_targets().unwrap();
        let loc = worker.segment_state.location();

        assert_eq!(
            head_entries,
            worker
                .store
                .read_all(loc.session_id, loc.segment_id)
                .unwrap()
                .len()
        );
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].preview, "second message");
        assert_eq!(targets[1].preview, "first message");
        assert!(
            targets[0]
                .warning
                .as_ref()
                .unwrap()
                .contains("tool side effects")
        );
    }

    #[tokio::test]
    async fn rewind_apply_truncates_log_and_restores_selected_input() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        append_user_turn(&worker, 20, "second message");
        append_test_entry(
            &worker,
            LogEntry::ToolResult {
                ts: 30,
                item: session_store::LoggedItem::ToolResult {
                    call_id: "call-1".into(),
                    summary: "wrote a file".into(),
                    content: None,
                    attachments: Vec::new(),
                    disposition: Default::default(),
                    is_error: false,
                },
            },
        );
        let (head_entries, targets) = worker.list_rewind_targets().unwrap();
        let expected_truncate_entries = targets[0].truncate_entries;
        let target = targets[0].id.clone();

        let applied = worker.rewind_to(target, head_entries).unwrap();

        assert_eq!(preview_segments(&applied.input), "second message");
        assert_eq!(
            applied.summary.truncated_to_entries,
            expected_truncate_entries
        );
        assert!(applied.summary.tool_side_effect_warning);
        let loc = worker.segment_state.location();
        assert_eq!(
            worker
                .store
                .read_all(loc.session_id, loc.segment_id)
                .unwrap()
                .len(),
            expected_truncate_entries
        );
        assert_eq!(worker.history().len(), 1);
        assert_eq!(worker.history()[0].as_text().unwrap(), "first message");
    }

    #[tokio::test]
    async fn annotated_history_rewind_commits_authoritative_prefix() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let expected_metadata = append_annotated_user_turn(&worker, 10, "first message");
        append_annotated_user_turn(&worker, 20, "second message");
        append_test_entry(
            &worker,
            LogEntry::AnnotatedToolResult {
                ts: 30,
                entry: session_store::LoggedHistoryEntry {
                    item: session_store::LoggedItem::ToolResult {
                        call_id: "call-v2".into(),
                        summary: "side effect".into(),
                        content: None,
                        attachments: Vec::new(),
                        disposition: Default::default(),
                        is_error: false,
                    },
                    metadata: new_history_metadata(
                        WorkerHistoryProvenance::ToolOutput {
                            worker: worker_subject(worker.session.session_id()),
                        },
                        None,
                    ),
                },
            },
        );

        let (head_entries, targets) = worker.list_rewind_targets().unwrap();
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].preview, "second message");
        assert!(targets[0].truncate_entries > 0);
        assert!(targets[0].warning.is_some());

        let applied = worker
            .rewind_to(targets[0].id.clone(), head_entries)
            .unwrap();
        assert_eq!(applied.summary.truncated_to_entries, 5);
        assert!(matches!(
            applied.entries.first(),
            Some(LogEntry::AnnotatedSegmentStart { .. })
        ));
        let retained_metadata = worker
            .session_history()
            .entries()
            .iter()
            .map(|entry| entry.annotation.clone())
            .collect::<Vec<_>>();
        assert_eq!(retained_metadata, expected_metadata);

        let source_location = worker.segment_state.location();
        let persisted_prefix = worker
            .store
            .read_all(source_location.session_id, source_location.segment_id)
            .unwrap();
        let restored_prefix = restore_history_entries(
            source_location.session_id,
            source_location.segment_id,
            &persisted_prefix,
        )
        .unwrap();
        assert_eq!(
            restored_prefix
                .iter()
                .map(|entry| entry.annotation.clone())
                .collect::<Vec<_>>(),
            expected_metadata
        );

        // Simulate a stale concurrent writer so the next head check forks.
        append_test_entry(
            &worker,
            LogEntry::Extension {
                ts: 31,
                domain: "test.concurrent-writer".into(),
                payload: serde_json::json!({"value": true}),
            },
        );
        worker.ensure_segment_head().unwrap();
        let fork_location = worker.segment_state.location();
        assert_ne!(fork_location.segment_id, source_location.segment_id);
        let fork_entries = worker
            .store
            .read_all(fork_location.session_id, fork_location.segment_id)
            .unwrap();
        let fork_history = restore_history_entries(
            fork_location.session_id,
            fork_location.segment_id,
            &fork_entries,
        )
        .unwrap();
        assert_eq!(
            fork_history
                .iter()
                .map(|entry| entry.annotation.clone())
                .collect::<Vec<_>>(),
            expected_metadata
        );
    }

    #[tokio::test]
    async fn rewind_apply_rejects_stale_head() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        let (head_entries, targets) = worker.list_rewind_targets().unwrap();
        append_user_turn(&worker, 20, "newer message");

        let err = worker
            .rewind_to(targets[0].id.clone(), head_entries)
            .unwrap_err()
            .to_string();

        assert!(err.contains("session head changed"));
    }

    #[tokio::test]
    async fn hook_paused_pending_tool_resumes_and_executes_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut engine =
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(PauseResumeClient::new());
        engine.register_tool(counting_pending_tool(calls.clone()));
        let mut worker = Worker::new(
            minimal_manifest(),
            engine,
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        let should_pause = Arc::new(std::sync::atomic::AtomicBool::new(true));
        worker.add_pre_tool_call_hook(PauseOnceHook { should_pause });

        assert_eq!(
            worker.run_text("start").await.unwrap(),
            WorkerRunResult::Paused
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(worker.history().iter().any(|item| matches!(
            item,
            Item::ToolCall { call_id, .. } if call_id == "call_pending"
        )));
        assert!(!worker.history().iter().any(|item| matches!(
            item,
            Item::ToolResult { call_id, .. } if call_id == "call_pending"
        )));

        assert_eq!(worker.resume().await.unwrap(), WorkerRunResult::Finished);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            worker
                .history()
                .iter()
                .filter(|item| matches!(
                    item,
                    Item::ToolResult {
                        call_id,
                        disposition: agen::ToolResultDisposition::Success,
                        ..
                    } if call_id == "call_pending"
                ))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn interrupted_result_terminalizes_orphan_before_run_completed() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::local_filesystem(None),
            authority,
            scope,
        )
        .await
        .unwrap();

        worker.ensure_segment_head().unwrap();
        worker.wire_history_persistence();
        worker.set_history_for_test(vec![
            Item::tool_call("call-known", "Read", "{}"),
            Item::tool_result_item_with_disposition_and_attachments(
                "call-known",
                "known result",
                Some("confirmed output".to_string()),
                agen::ToolResultDisposition::Success,
                Vec::new(),
            ),
            Item::tool_call("call-orphan", "Bash", "{}"),
        ]);
        let _ = worker
            .handle_worker_result(
                EngineRunExit::Interrupted(StopReason::Cancelled),
                worker.history().len(),
            )
            .await
            .unwrap();

        let history = worker.history();
        assert_eq!(
            history
                .iter()
                .filter(|item| matches!(
                    item,
                    Item::ToolResult {
                        call_id,
                        disposition: agen::ToolResultDisposition::Success,
                        ..
                    } if call_id == "call-known"
                ))
                .count(),
            1
        );
        assert!(!history.iter().any(|item| matches!(
            item,
            Item::ToolResult {
                call_id,
                disposition: agen::ToolResultDisposition::OutcomeUnknown,
                ..
            } if call_id == "call-known"
        )));
        assert_eq!(
            history
                .iter()
                .filter(|item| matches!(
                    item,
                    Item::ToolResult {
                        call_id,
                        disposition: agen::ToolResultDisposition::OutcomeUnknown,
                        ..
                    } if call_id == "call-orphan"
                ))
                .count(),
            1
        );

        let entries = worker
            .store
            .read_all(
                worker.segment_state.session_id(),
                worker.segment_state.segment_id(),
            )
            .unwrap();
        let terminal_index = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    LogEntry::AnnotatedToolResult {
                        entry: session_store::LoggedHistoryEntry {
                            item: session_store::LoggedItem::ToolResult {
                                call_id,
                                disposition: agen::ToolResultDisposition::OutcomeUnknown,
                                ..
                            },
                            ..
                        },
                        ..
                    } if call_id == "call-orphan"
                )
            })
            .expect("durable OutcomeUnknown closure");
        let final_index = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    LogEntry::RunCompleted { .. } | LogEntry::RunErrored { .. }
                )
            })
            .expect("durable final run status");
        assert!(terminal_index < final_index);
    }

    #[tokio::test]
    async fn apply_interrupt_prep_appends_via_callback_and_logs_independent_entries() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::local_filesystem(None),
            authority,
            scope,
        )
        .await
        .unwrap();

        worker.ensure_segment_head().unwrap();
        worker.wire_history_persistence();
        worker.set_history_for_test(vec![Item::tool_call("call-1", "Read", "{}")]);

        worker.apply_interrupt_prep().unwrap();

        let history = worker.history();
        assert_eq!(history.len(), 3);
        assert!(matches!(history[1], Item::ToolResult { ref call_id, .. } if call_id == "call-1"));
        assert!(matches!(
            history[2],
            Item::Message {
                role: Role::System,
                ..
            }
        ));

        let interrupt_note = history[2].as_text().unwrap().to_string();
        let entries = worker
            .store
            .read_all(
                worker.segment_state.session_id(),
                worker.segment_state.segment_id(),
            )
            .unwrap();
        let tool_result_count = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    LogEntry::AnnotatedToolResult {
                        entry: session_store::LoggedHistoryEntry {
                            item: session_store::LoggedItem::ToolResult { call_id, .. },
                            ..
                        },
                        ..
                    } if call_id == "call-1"
                )
            })
            .count();
        let interrupt_system_count = entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry,
                    LogEntry::AnnotatedSystemItem {
                        entry: session_store::LoggedSystemHistoryEntry {
                            item: SystemItem::Interrupt { body, .. },
                            ..
                        },
                        ..
                    } if body == &interrupt_note
                )
            })
            .count();

        assert_eq!(tool_result_count, 1);
        assert_eq!(interrupt_system_count, 1);
    }

    #[tokio::test]
    async fn notification_run_closes_interrupted_tool_call_before_engine_resume() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::local_filesystem(None),
            authority,
            scope,
        )
        .await
        .unwrap();

        worker.ensure_segment_head().unwrap();
        worker.wire_history_persistence();
        let dangling_call = Item::tool_call("call-1", "SideEffect", "{}");
        worker
            .commit_entry(LogEntry::AssistantItem {
                ts: segment_log::now_millis(),
                item: dangling_call.clone().into(),
            })
            .unwrap();
        worker.set_history_for_test(vec![dangling_call]);
        worker.last_run_interrupted = true;

        worker
            .run_for_notification(protocol::InvokeKind::Notify)
            .await
            .unwrap();

        let history = worker.history();
        assert!(matches!(
            history.get(1),
            Some(Item::ToolResult { call_id, .. }) if call_id == "call-1"
        ));
        assert!(matches!(
            history.get(2),
            Some(Item::Message {
                role: Role::System,
                ..
            })
        ));
    }

    #[derive(Clone, Copy)]
    struct ResidentInjectionGates {
        summary: bool,
    }

    impl ResidentInjectionGates {
        fn all(enabled: bool) -> Self {
            Self { summary: enabled }
        }
    }

    #[tokio::test]
    async fn memory_consolidation_prompt_uses_bound_workspace_language() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut manifest = minimal_manifest();
        manifest.profile = Some(manifest::ProfileManifestSnapshot {
            source: manifest::ProfileSource::Registry {
                source: manifest::ProfileRegistrySource::Builtin,
                name: "memory-consolidation".to_string(),
                path: None,
                provenance: None,
            },
            profile: None,
        });
        let mut memory = manifest::MemoryConfig::default();
        memory.bind_workspace_settings(&manifest::WorkspaceMemorySettingsSnapshot {
            workspace_id: "workspace-test".to_string(),
            settings_revision: 3,
            language: "Japanese".to_string(),
        });
        manifest.memory = Some(memory);
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        worker.set_system_prompt_template(
            SystemPromptTemplate::parse(
                "default",
                crate::prompt::source::PromptCatalogSource::builtins_only(),
            )
            .unwrap(),
        );
        worker.ensure_system_prompt_materialized().await.unwrap();
        let prompt = worker.engine().get_system_prompt().unwrap();
        assert!(prompt.contains("`language`: `Japanese`"));
        assert!(!prompt.contains("`language`: `English`"));
    }

    async fn render_system_prompt_with_summary(
        summary_doc: Option<&str>,
        memory_config: Option<manifest::MemoryConfig>,
        resident_injection: bool,
    ) -> String {
        render_system_prompt_with_resident_sections(
            summary_doc,
            memory_config,
            ResidentInjectionGates::all(resident_injection),
            false,
        )
        .await
    }

    async fn render_system_prompt_with_resident_sections(
        summary_doc: Option<&str>,
        memory_config: Option<manifest::MemoryConfig>,
        gates: ResidentInjectionGates,
        _unused: bool,
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut manifest = minimal_manifest();
        manifest.memory = memory_config.clone();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = if memory_config
            .as_ref()
            .is_some_and(|cfg| cfg.inject_summary.unwrap_or(true))
            && gates.summary
        {
            stub_memory_backend_context(summary_doc.and_then(summary_content_for_backend))
        } else {
            WorkerWorkspaceContext::local_filesystem(None)
        };
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            workspace_context,
            authority,
            scope,
        )
        .await
        .unwrap();
        worker.set_resident_memory_injection(gates.summary);
        let template = SystemPromptTemplate::parse(
            "default",
            crate::prompt::source::PromptCatalogSource::builtins_only(),
        )
        .unwrap();
        worker.set_system_prompt_template(template);
        worker.ensure_system_prompt_materialized().await.unwrap();
        worker.engine().get_system_prompt().unwrap().to_string()
    }

    fn summary_doc(body: &str) -> String {
        format!("---\nupdated_at: 2026-01-01T00:00:00Z\n---\n{body}")
    }

    fn summary_content_for_backend(doc: &str) -> Option<String> {
        if doc.contains("this is not yaml") {
            return None;
        }
        if let Some(rest) = doc.strip_prefix("---\n") {
            if let Some((_, body)) = rest.split_once("\n---\n") {
                return Some(body.to_string());
            }
        }
        Some(doc.to_string())
    }

    fn stub_memory_backend_context(content: Option<String>) -> WorkerWorkspaceContext {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            let body = serde_json::json!({
                "status": "ok",
                "result": {
                    "kind": "tool_output",
                    "summary": if content.is_some() {
                        "resident memory summary collected"
                    } else {
                        "resident memory summary unavailable"
                    },
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
        WorkerWorkspaceContext::with_client(
            Some(WorkspaceId::new("test-memory").unwrap()),
            Arc::new(TestWorkspaceHttpClient::new(
                "test-memory",
                format!("http://{addr}"),
            )),
        )
    }

    #[tokio::test]
    async fn resident_summary_body_is_injected_without_frontmatter() {
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("summary body for resident prompt\n")),
            Some(manifest::MemoryConfig::default()),
            true,
        )
        .await;

        assert!(rendered.contains("summary body for resident prompt"));
        assert!(!rendered.contains("updated_at: 2026-01-01T00:00:00Z"));
        assert!(!rendered.contains("---\nupdated_at"));
    }

    #[tokio::test]
    async fn resident_summary_injection_can_be_disabled_by_manifest() {
        let memory = manifest::MemoryConfig {
            inject_summary: Some(false),
            ..manifest::MemoryConfig::default()
        };
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("disabled summary body\n")),
            Some(memory),
            true,
        )
        .await;

        assert!(!rendered.contains("disabled summary body"));
    }

    #[tokio::test]
    async fn resident_summary_is_absent_without_memory_config() {
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("memory-disabled summary body\n")),
            None,
            true,
        )
        .await;

        assert!(!rendered.contains("memory-disabled summary body"));
    }

    #[tokio::test]
    async fn malformed_resident_summary_does_not_fail_render() {
        let rendered = render_system_prompt_with_summary(
            Some("---\nthis is not yaml: : :\n---\nbad summary body\n"),
            Some(manifest::MemoryConfig::default()),
            true,
        )
        .await;

        assert!(!rendered.trim().is_empty());
        assert!(!rendered.contains("bad summary body"));
    }

    #[tokio::test]
    async fn resident_summary_gate_false_omits_only_summary() {
        let prompt = render_system_prompt_with_resident_sections(
            Some(&summary_doc("resident summary marker")),
            Some(manifest::MemoryConfig::default()),
            ResidentInjectionGates { summary: false },
            true,
        )
        .await;

        assert!(!prompt.contains("resident summary marker"));
    }

    #[test]
    fn activate_skill_commits_and_appends_history_before_future_context_use() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert!(
                request_line
                    .starts_with("GET /api/w/ws-skill/skills/triage-errors/activate HTTP/1.1")
            );
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let body = serde_json::json!({
                "name": "triage-errors",
                "provenance": { "kind": "workspace", "id": "workspace:triage-errors" },
                "diagnostics": [],
                "body": "---\nname: triage-errors\ndescription: Use when testing activation history.\n---\n\n# Triage Errors\n\nCommitted Skill body."
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let manifest = minimal_manifest();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut worker = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(Worker::new(
                manifest,
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
                store,
                WorkerWorkspaceContext::with_client(
                    Some(WorkspaceId::new("ws-skill").unwrap()),
                    Arc::new(TestWorkspaceHttpClient::new(
                        "ws-skill",
                        format!("http://{addr}"),
                    )),
                ),
                authority,
                scope,
            ))
            .unwrap();

        let activation = worker.activate_skill("triage-errors").unwrap();

        assert_eq!(activation.name, "triage-errors");
        server.join().unwrap();
        let history = worker.history();
        assert_eq!(history.len(), 1);
        let history_text = history[0].as_text().unwrap();
        assert!(
            history_text
                .contains("Agent Skill `triage-errors` activated from workspace:triage-errors")
        );
        assert!(history_text.contains("# Triage Errors"));
        assert!(history_text.contains("Committed Skill body."));

        let entries = worker
            .store
            .read_all(
                worker.segment_state.session_id(),
                worker.segment_state.segment_id(),
            )
            .unwrap();
        assert!(entries.iter().any(|entry| {
            matches!(
                entry,
                LogEntry::AnnotatedSystemItem {
                    entry: session_store::LoggedSystemHistoryEntry {
                        item: SystemItem::SkillActivation { name, body },
                        ..
                    },
                    ..
                } if name == "triage-errors"
                    && body.contains("# Triage Errors")
                    && body == history_text
            )
        }));
    }

    #[tokio::test]
    async fn cancelled_internal_extract_does_not_commit_pointer_or_completed_audit() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cancel_tx = Arc::new(Mutex::new(None));
        let client = CancelBeforeAiExtractClient {
            cancel_tx: cancel_tx.clone(),
        };
        let audit_client = Arc::new(RecordingAuditWorkspaceClient::default());
        let mut manifest = minimal_manifest();
        manifest.memory = Some(manifest::MemoryConfig {
            extract_threshold: Some(1),
            workspace_id: Some("workspace-test".to_string()),
            settings_revision: Some(1),
            language: Some("English".to_string()),
            ..Default::default()
        });
        let memory_config = manifest.memory.clone().unwrap();
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client),
            store,
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                audit_client.clone(),
            ),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().unwrap();
        worker.wire_history_persistence();
        let evidence = Item::user_message(
            "The cancellation regression must leave this evidence available for retry.",
        );
        worker.set_history_for_test(vec![evidence.clone()]);
        worker
            .commit_entry(LogEntry::UserInput {
                ts: segment_log::now_millis(),
                extensions: vec![],
                segments: vec![text_segment(
                    "The cancellation regression must leave this evidence available for retry.",
                )],
            })
            .unwrap();
        worker
            .usage_history
            .lock()
            .expect("usage history lock")
            .push(UsageRecord {
                history_len: 1,
                input_total_tokens: 100,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                output_tokens: 0,
            });

        let entries_before = worker
            .store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert!(
            worker
                .extract_pointer
                .lock()
                .expect("extract pointer lock")
                .is_none()
        );

        let cancel_tx_for_extract = cancel_tx.clone();
        let error = match worker
            .run_extract_once_with_cancel_observer(
                &memory_config,
                1,
                Some(Box::new(move |cancel_sender| {
                    *cancel_tx_for_extract
                        .lock()
                        .expect("cancel sender slot lock") = Some(cancel_sender);
                })),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("pre-AI cancellation must not complete extraction"),
        };

        assert!(matches!(error, WorkerError::Engine(EngineError::Cancelled)));
        assert!(
            worker
                .extract_pointer
                .lock()
                .expect("extract pointer lock")
                .is_none()
        );
        assert_eq!(worker.history(), &[evidence]);

        let entries_after = worker
            .store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert_eq!(entries_after.len(), entries_before.len());
        assert!(!entries_after.iter().any(|entry| matches!(
            entry,
            LogEntry::Extension { domain, .. } if domain == memory::extract::EXTRACT_DOMAIN
        )));

        let audits = audit_client.lifecycle_audits();
        assert_eq!(audits.len(), 2);
        assert_eq!(audits[0].run_id, audits[1].run_id);
        assert_eq!(audits[0].worker, memory::audit::AuditWorker::MemoryExtract);
        assert!(audits.iter().all(|audit| {
            audit.memory_settings
                == Some(memory::audit::MemorySettingsAudit {
                    workspace_id: "workspace-test".to_string(),
                    settings_revision: 1,
                    language: "English".to_string(),
                })
        }));
        assert_eq!(
            audits.iter().map(|audit| audit.status).collect::<Vec<_>>(),
            vec![
                memory::audit::WorkerLifecycleStatus::Started,
                memory::audit::WorkerLifecycleStatus::Cancelled,
            ]
        );
        assert!(
            !audits
                .iter()
                .any(|audit| { audit.status == memory::audit::WorkerLifecycleStatus::Completed })
        );
    }

    #[test]
    fn successful_internal_extract_lifecycles_enter_the_commit_path() {
        for lifecycle in [
            WorkerRunResult::Finished,
            WorkerRunResult::Paused,
            WorkerRunResult::LimitReached,
        ] {
            assert!(extract_internal_worker_lifecycle_error(&lifecycle).is_none());
        }
    }

    fn minimal_manifest() -> WorkerManifest {
        let toml_str = r#"
[worker]
name = "x"

[model]
scheme = "anthropic"
model_id = "claude-sonnet-4-20250514"

[engine]

[[scope.allow]]
target = "/abs/scope"
permission = "write"
"#;
        WorkerManifest::from_toml(toml_str).unwrap()
    }
}
