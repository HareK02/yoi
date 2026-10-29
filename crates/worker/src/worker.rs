#[cfg(test)]
#[path = "image_recovery_tests.rs"]
mod image_recovery_tests;

use std::collections::{HashMap, VecDeque};
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agen::llm_client::RequestConfig;
use agen::llm_client::client::LlmClient;
use agen::llm_client::types::Role;
use agen::state::Mutable;
use agen::{
    Engine, EngineError, EngineResult, EngineRunExit, History, HistoryEntry, Item,
    RunInterruptionReason, ToolExecutionPolicy, ToolOutputLimits, UsageRecord,
};
use arc_swap::ArcSwap;
use session_store::{
    LogEntry, PasteArtifactLimits, PromptRenderProvenance, SegmentId, SessionExtension, SessionId,
    Store, StoreError, SubjektivSessionAttributionState, SystemItem, segment_log,
};
use session_store::{
    WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore, WorkerReclaimedChild,
    WorkerSpawnedChild, WorkerSpawnedScopeRule, WorkerStoreError,
};
use tracing::{info, warn};

use crate::segment_log_sink::SegmentLogSink;
#[cfg(test)]
use crate::session_history::history_entry;
use crate::session_history::{
    SessionHistoryDerivation, SessionHistoryEntryId, SessionHistoryMetadata,
    WorkerHistoryProvenance, history_entry_with_id, metadata as new_history_metadata,
    restore_history_entries, to_logged_history_entry, worker_subject,
};

use manifest::{
    DelegationScope, Permission, ResolveError, Scope, ScopeConfig, ScopeError, ScopeRule,
    SharedScope, WorkerManifest, WorkerManifestConfig,
};

use crate::compact::state::{
    AutomaticCompactBlock, AutomaticCompactDecision, AutomaticCompactTrigger, CompactState,
    CompactionOutcome,
};
use crate::compact::telemetry::{
    CompactAttempt, CompactFailureCategory, CompactMode, CompactSuccessStats,
    CompactThresholdPolicy, new_compact_metric_correlation_id,
};
use crate::compact::usage_tracker::{UsageTracker, persist_pending_usage};
use crate::feature::background::{BackgroundTaskRewriteGuard, FeatureBackgroundTaskRegistry};
use crate::feature::builtin::TaskFeature;
use crate::feature::builtin::memory::WorkspaceMemoryBackendError;
use crate::feature::contracts::WorkerObservationProvider;
use crate::feature::session::{
    CommittedRunExit, CommittedSessionCapture, CommittedSessionCaptureHandle, FeatureSessionError,
    SessionExtensionHandle,
};
use crate::feature::{
    FeatureInstructionDeclaration, FeatureInvocationRegistry, FeatureRegistryBuilder,
    FeatureRegistryInstallReport, dedupe_instruction_contributions,
};
use crate::hook::{
    BeforeSessionRewriteAction, BeforeSessionRewriteContext, Hook, HookHistoryRange,
    HookInvocationContext, HookRegistry, HookRegistryBuilder, OnPromptSubmit, OnTurnEnd,
    PostToolCall, PreLlmRequest, PreToolCall, RestoreSystemItemAppendHandle, RunCommittedContext,
    RunCommittedExit, RunExitContext, SessionRewriteKind, WorkerRestoredContext,
    WorkerStoppingContext,
};
use crate::in_flight::InFlightEvents;
use crate::internal_worker::{
    InternalWorkerAuthority, InternalWorkerIdentity, InternalWorkerSpec, InternalWorkerVisibility,
    prepare_internal_worker_from_spec,
};

const LARGE_PASTE_INLINE_MAX_BYTES: usize = 32 * 1024;
const FEATURE_HOOK_CHAIN_TIMEOUT: Duration = Duration::from_secs(30);
const SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN: &str = "worker.pending_activations.v1";
const MAX_PENDING_SUBMISSIONS: usize = 32;
const MAX_PENDING_SUBMISSION_BYTES: u64 = 1024 * 1024;
const MAX_PENDING_ARTIFACT_REFS: usize = 64;
const MAX_ACTIVATION_REQUEST_ID_BYTES: usize = 128;
const MAX_SUBMISSION_RECEIPTS: usize = 128;
const MAX_INVOCATION_RECEIPTS: usize = 1024;
const MAX_INVOCATION_IDENTITY_BYTES: usize = 256;

/// Durable replay-prevention receipt retaining invocation start and a bounded result.
///
/// Stores the typed payload digest and qualified identity, not a copy of all arguments.
/// A start without a result cannot rule out side effects, so reexecution is rejected.
/// The same ID with a different payload is also rejected. This is not a business exactly-once ledger.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationReceipt {
    invocation_id: String,
    identity: protocol::FeatureInvocationIdentity,
    payload_digest: String,
    result: Option<protocol::FeatureInvocationResult>,
}

impl InvocationReceipt {
    fn new(
        invocation: &protocol::FeatureInvocation,
        result: Option<protocol::FeatureInvocationResult>,
    ) -> Self {
        Self {
            invocation_id: invocation.invocation_id.clone(),
            identity: invocation.identity.clone(),
            payload_digest: invocation_payload_digest(invocation),
            result,
        }
    }

    fn matches(&self, invocation: &protocol::FeatureInvocation) -> bool {
        self.invocation_id == invocation.invocation_id
            && self.identity == invocation.identity
            && self.payload_digest == invocation_payload_digest(invocation)
    }

    fn is_valid(&self) -> bool {
        !self.invocation_id.trim().is_empty()
            && self.invocation_id.len() <= protocol::invocation::MAX_FEATURE_INVOCATION_ID_BYTES
            && !self.identity.0.is_empty()
            && self.identity.0.len() <= MAX_INVOCATION_IDENTITY_BYTES
            && self.payload_digest.len() == 64
            && self
                .payload_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && self.result.as_ref().is_none_or(|result| {
                result.invocation_id == self.invocation_id
                    && result.identity == self.identity
                    && crate::feature::invocation::invocation_result_is_bounded(result)
                    && (result.status == protocol::FeatureInvocationStatus::Succeeded
                        || result.context.is_none())
            })
    }
}

fn invocation_payload_digest(invocation: &protocol::FeatureInvocation) -> String {
    submission_payload_digest(&[Segment::FeatureInvoke {
        invocation: invocation.clone(),
    }])
}

fn validate_activation_invocation_ids(input: &[Segment]) -> Result<(), String> {
    if input
        .iter()
        .any(|segment| matches!(segment, Segment::Unknown))
    {
        return Err(
            "unknown input segment rejected; unsupported intent must not be discarded".into(),
        );
    }
    const MAX_INVOCATIONS_PER_SUBMISSION: usize = 8;
    let mut ids = std::collections::BTreeSet::new();
    for invocation in input.iter().filter_map(|segment| match segment {
        Segment::FeatureInvoke { invocation } => Some(invocation),
        _ => None,
    }) {
        if invocation.invocation_id.trim().is_empty()
            || invocation.invocation_id.len()
                > protocol::invocation::MAX_FEATURE_INVOCATION_ID_BYTES
            || invocation.identity.0.is_empty()
            || invocation.identity.0.len() > MAX_INVOCATION_IDENTITY_BYTES
        {
            return Err(
                "invocation id/identity is empty or exceeds the recovery key byte limit".into(),
            );
        }
        if !ids.insert(&invocation.invocation_id) {
            return Err("duplicate invocation id in one submission".into());
        }
    }
    if ids.len() > MAX_INVOCATIONS_PER_SUBMISSION {
        return Err(format!(
            "at most {MAX_INVOCATIONS_PER_SUBMISSION} Feature invocations are allowed per submission"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingSubmission {
    pub(crate) submission_request_id: String,
    source_namespace: String,
    pub(crate) submission_id: String,
    payload_digest: String,
    accepted_at_ms: u64,
    activation_sequence: u64,
    pub(crate) provenance: WorkerHistoryProvenance,
    #[serde(default)]
    was_queued: bool,
    pub(crate) input: Vec<Segment>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SubmissionReceipt {
    submission_request_id: String,
    source_namespace: String,
    submission_id: String,
    payload_digest: String,
    disposition: protocol::SubmissionDisposition,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingNotification {
    pub(crate) notification_request_id: String,
    source_namespace: String,
    pub(crate) message: String,
    payload_digest: String,
    pub(crate) accepted_at_ms: u64,
    activation_sequence: u64,
    pub(crate) provenance: WorkerHistoryProvenance,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct NotificationReceipt {
    notification_request_id: String,
    source_namespace: String,
    payload_digest: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct PendingActivationState {
    next_activation_sequence: u64,
    /// A prepared activation remains in checkpoints until the same atomic
    /// UserInput record commits the clearing checkpoint. Restore puts it back
    /// at the FIFO head.
    activating: Option<PendingSubmission>,
    #[serde(
        default,
        rename = "activating_notification",
        deserialize_with = "deserialize_activating_notifications"
    )]
    activating_notifications: VecDeque<PendingNotification>,
    pending: VecDeque<PendingSubmission>,
    pending_notifications: VecDeque<PendingNotification>,
    receipts: VecDeque<SubmissionReceipt>,
    notification_receipts: VecDeque<NotificationReceipt>,
    /// Retained across queue checkpoints, compaction, rewind and restore. Do not
    /// evict starts/results: eviction would silently permit repeated effects.
    #[serde(default)]
    invocation_receipts: Vec<InvocationReceipt>,
    #[serde(default)]
    invocation_recovery_blocked: bool,
}

fn deserialize_activating_notifications<'de, D>(
    deserializer: D,
) -> Result<VecDeque<PendingNotification>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(PendingNotification),
        Many(VecDeque<PendingNotification>),
    }

    Ok(
        match <Option<OneOrMany> as serde::Deserialize>::deserialize(deserializer)? {
            Some(OneOrMany::One(notification)) => VecDeque::from([notification]),
            Some(OneOrMany::Many(notifications)) => notifications,
            None => VecDeque::new(),
        },
    )
}

impl PendingActivationState {
    pub(crate) fn has_unfinished_admission(&self) -> bool {
        self.activating.is_some()
            || !self.pending.is_empty()
            || !self.pending_notifications.is_empty()
            || !self.activating_notifications.is_empty()
    }
    pub(crate) fn snapshot(&self) -> protocol::PendingSubmissionsSnapshot {
        let pending_notification = self.pending_notifications.front();
        let head_id = match (self.pending.front(), pending_notification) {
            (Some(submission), Some(notification))
                if notification.activation_sequence < submission.activation_sequence =>
            {
                Some(notification_head_id(
                    &notification.source_namespace,
                    &notification.notification_request_id,
                ))
            }
            (Some(submission), _) => Some(submission.submission_id.clone()),
            (None, Some(notification)) => Some(notification_head_id(
                &notification.source_namespace,
                &notification.notification_request_id,
            )),
            (None, None) => None,
        };
        protocol::PendingSubmissionsSnapshot {
            notification_count: u32::try_from(self.pending_notifications.len()).unwrap_or(u32::MAX),
            notification_previews: self
                .pending_notifications
                .iter()
                .map(|notification| pending_input_preview(notification.message.chars()))
                .collect(),
            head_id,
            submissions: self
                .pending
                .iter()
                .map(|pending| protocol::PendingSubmissionSummary {
                    submission_id: pending.submission_id.clone(),
                    preview: Some(pending_submission_preview(&pending.input)),
                    accepted_at_ms: pending.accepted_at_ms,
                    segment_count: u32::try_from(pending.input.len()).unwrap_or(u32::MAX),
                    byte_len: submission_payload_len(&pending.input),
                })
                .collect(),
        }
    }

    fn live_artifact_pin_owner_ids(&self) -> Vec<String> {
        self.activating
            .iter()
            .chain(self.pending.iter())
            .map(|submission| submission.submission_id.clone())
            .collect()
    }

    fn remember_notification_receipt(&mut self, receipt: NotificationReceipt) {
        self.notification_receipts.push_back(receipt);
        while self.notification_receipts.len() > MAX_SUBMISSION_RECEIPTS {
            self.notification_receipts.pop_front();
        }
    }

    fn remember_receipt(&mut self, receipt: SubmissionReceipt) {
        self.receipts.push_back(receipt);
        while self.receipts.len() > MAX_SUBMISSION_RECEIPTS {
            self.receipts.pop_front();
        }
    }
}

pub const DEFAULT_NOTIFICATION_COALESCE_DELAY: Duration = Duration::from_secs(2);

fn notification_payload_digest(message: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    // Keep the original real-time notification digest domain so receipts
    // accepted before the delivery-mode field was removed still dedupe.
    hasher.update(b"auto\0");
    hasher.update(message.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn authenticated_input_provenance(
    source: &protocol::AuthenticatedInputSource,
) -> WorkerHistoryProvenance {
    match source {
        protocol::AuthenticatedInputSource::UntrustedWire => WorkerHistoryProvenance::LegacyUnknown,
        protocol::AuthenticatedInputSource::LocalOperator => {
            WorkerHistoryProvenance::LocalHumanInput
        }
        protocol::AuthenticatedInputSource::Account { account_id } => {
            WorkerHistoryProvenance::HumanInput {
                account_id: account_id.clone(),
            }
        }
        protocol::AuthenticatedInputSource::Worker {
            runtime_id,
            worker_id,
        } => WorkerHistoryProvenance::WorkerInput {
            actor: session_store::LoggedWorkerSubject {
                workspace_id: None,
                runtime_id: Some(runtime_id.clone()),
                worker_id: worker_id.clone(),
            },
        },
        protocol::AuthenticatedInputSource::SubWorker { session_id } => {
            WorkerHistoryProvenance::WorkerInput {
                actor: session_store::LoggedWorkerSubject {
                    workspace_id: None,
                    runtime_id: None,
                    worker_id: session_id.clone(),
                },
            }
        }
        protocol::AuthenticatedInputSource::Backend { operation_id } => {
            WorkerHistoryProvenance::BackendInstruction {
                operation_id: Some(operation_id.clone()),
            }
        }
    }
}

fn notification_head_id(source_namespace: &str, request_id: &str) -> String {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(source_namespace.as_bytes());
    hasher.update(b"\0");
    hasher.update(request_id.as_bytes());
    format!(
        "notification:{}",
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// Display-only preview: no artifact reads, payload clones, or input mutation.
fn pending_submission_preview(input: &[Segment]) -> String {
    let chars = input.iter().flat_map(|segment| {
        let parts: [&str; 3] = match segment {
            Segment::Text { content } | Segment::Paste { content, .. } => ["", content, ""],
            Segment::UploadedFile { file } => ["[Attached file: ", &file.file_name, "]"],
            Segment::PasteArtifact { .. } => ["[Large paste]", "", ""],
            Segment::FileRef { path } => ["@", path, ""],
            Segment::FeatureInvoke { invocation } => ["[Feature: ", &invocation.name, "]"],
            Segment::Flow { selector } => ["[Flow: ", selector, "]"],
            Segment::Unknown => ["[Unknown input]", "", ""],
        };
        parts
            .into_iter()
            .flat_map(str::chars)
            .chain(std::iter::once(' '))
    });
    pending_input_preview(chars)
}

fn pending_input_preview(chars: impl Iterator<Item = char>) -> String {
    const LIMIT: usize = 240;
    let mut preview = String::new();
    let mut count = 0;
    let mut space = false;
    for ch in chars {
        if ch.is_whitespace() || ch.is_control() {
            space = !preview.is_empty();
            continue;
        }
        if space && count < LIMIT {
            preview.push(' ');
            count += 1;
        }
        space = false;
        if count == LIMIT {
            preview.truncate(preview.trim_end().len());
            preview.push('…');
            return preview;
        }
        preview.push(ch);
        count += 1;
    }
    if preview.is_empty() {
        "[Empty input]".into()
    } else {
        preview
    }
}

fn submission_payload_len(input: &[Segment]) -> u64 {
    serde_json::to_vec(input)
        .map(|bytes| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .unwrap_or(u64::MAX)
}

fn submission_payload_digest(input: &[Segment]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(serde_json::to_vec(input).unwrap_or_default())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn submission_uploaded_file_refs(
    input: &[Segment],
) -> impl Iterator<Item = &protocol::UploadedFileRef> {
    input.iter().filter_map(|segment| match segment {
        Segment::UploadedFile { file } => Some(file),
        _ => None,
    })
}

fn submission_artifact_ref_count(input: &[Segment]) -> usize {
    input
        .iter()
        .filter(|segment| {
            matches!(
                segment,
                Segment::PasteArtifact { .. } | Segment::UploadedFile { .. }
            )
        })
        .count()
}

fn pending_activation_extension(state: &PendingActivationState) -> SessionExtension {
    SessionExtension {
        domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.to_owned(),
        payload: serde_json::to_value(state).expect("pending activation state must serialize"),
    }
}

fn hook_run_exit(exit: &EngineRunExit) -> RunCommittedExit {
    match exit {
        EngineRunExit::Finished => RunCommittedExit::Finished,
        EngineRunExit::Paused => RunCommittedExit::Paused,
        EngineRunExit::Yielded => RunCommittedExit::Yielded,
        EngineRunExit::Interrupted(_) => RunCommittedExit::Interrupted,
    }
}

use crate::ipc::alerter::Alerter;
use crate::ipc::interceptor::WorkerInterceptor;
use crate::ipc::notify_buffer::NotifyBuffer;
use crate::prompt::agents_md::read_agents_md;
use crate::prompt::catalog::{CatalogError, PromptCatalog, WorkspacePromptProjection};
use crate::prompt::source::PromptCatalogSource;
use crate::prompt::system::{SystemPromptContext, SystemPromptError, SystemPromptTemplate};
use crate::runtime::worker_allocation::{
    self, ScopeAllocationCleanup, ScopeAllocationGuard, ScopeLockError, SegmentActivationGuard,
};
use crate::skill::{SkillActivationResponse, SkillClientError};
#[cfg(test)]
use async_trait::async_trait;
use protocol::{
    AlertLevel, AlertSource, CompactionLifecycle, CompactionLifecycleState, CompactionPhase,
    CompactionTrigger, ErrorCode, Event, InFlightCompaction, RewindSummary, RunFailureKind,
    RunResumeSource, RunYieldReason, Segment,
};
use tokio::net::UnixStream;
use tokio::sync::broadcast;
use workdir::{
    LocalWorkdirSession, ReadOnlyWorkdirSession, WorkdirAttachmentAlias,
    WorkdirSessionCapabilities, WorkdirSessionHandle, WorkdirSessionRouter,
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

/// Raw-byte request through a host-bound Workspace client, separate from JSON requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBinaryRequest {
    pub method: WorkspaceRequestMethod,
    pub path: String,
    pub body: Option<Vec<u8>>,
    pub max_response_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceBinaryResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl WorkspaceBinaryResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceWorkerDiscoveryRequest {
    pub cursor: Option<String>,
    pub limit: usize,
    pub query: Option<String>,
}

#[derive(Debug, Clone)]
pub enum WorkspaceServerOperation {
    TicketAssignmentSet {
        ticket_id: String,
        role: server_api::TicketAssignmentRole,
        request: server_api::SetTicketRoleAssignmentRequest,
    },
    TicketAssignmentClear {
        ticket_id: String,
        role: server_api::TicketAssignmentRole,
        query: server_api::ClearTicketRoleAssignmentQuery,
    },
    TicketAssignmentsPage {
        ticket_id: String,
        query: server_api::TicketAssignmentsQuery,
    },
    NotificationDiagnostics {
        query: server_api::NotificationDiagnosticsQuery,
    },
    TicketAssignmentDiagnostics {
        ticket_id: String,
        query: server_api::TicketAssignmentDiagnosticsQuery,
    },
    TicketAssignmentAnswer {
        ticket_id: String,
        request: server_api::TicketAssignmentAnswerRequest,
    },
    TicketImplementationResult {
        ticket_id: String,
        request: server_api::TicketImplementationResultRequest,
    },
    TicketImplementationResultGet {
        ticket_id: String,
        result_id: String,
    },
    WorkerControlList,
    WorkerControlAuthorize {
        runtime_id: String,
        worker_id: String,
        permission: String,
    },
    WorkerControlSpawn(server_api::CreateWorkspaceWorkerRequest),
    WorkerControlInput {
        runtime_id: String,
        worker_id: String,
        request: server_api::RuntimeWorkerInputRequest,
    },
    WorkerControlCancel {
        runtime_id: String,
        worker_id: String,
        request: server_api::RuntimeWorkerLifecycleRequest,
    },
    WorkerControlStop {
        runtime_id: String,
        worker_id: String,
        request: server_api::RuntimeWorkerLifecycleRequest,
    },
    WorkerControlRestore {
        runtime_id: String,
        worker_id: String,
        request: server_api::WorkerRestoreRequest,
    },
    WorkerObservationSessions,
    WorkerObservationCapture(server_api::WorkerObservationSubjectRef),
    SubjektivStageCandidate(server_api::SubjektivStageCandidateRequest),
    SubjektivRecordSession(server_api::SubjektivRecordSessionRequest),
    SubjektivMemory(server_api::SubjektivMemoryBackendRequest),
    SubjektivSession(server_api::SubjektivSessionBackendRequest),
}

fn workspace_server_json_request<T: serde::Serialize>(
    path: String,
    body: &T,
) -> Result<WorkspaceRequest, WorkspaceClientError> {
    Ok(WorkspaceRequest::json(
        WorkspaceRequestMethod::Post,
        path,
        serde_json::to_string(body)
            .map_err(|error| WorkspaceClientError::Request(error.to_string()))?,
    ))
}

fn encode_workspace_path_segment(value: &str) -> String {
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

fn workspace_server_query_request<T: serde::Serialize>(
    method: WorkspaceRequestMethod,
    path: String,
    query: &T,
) -> Result<WorkspaceRequest, WorkspaceClientError> {
    let query = serde_json::to_value(query)
        .map_err(|error| WorkspaceClientError::Request(error.to_string()))?;
    let fields = query.as_object().ok_or_else(|| {
        WorkspaceClientError::Request("Workspace query must serialize to an object".into())
    })?;
    let mut pairs = Vec::new();
    for (name, value) in fields {
        let value = match value {
            serde_json::Value::Null => continue,
            serde_json::Value::String(value) => value.clone(),
            serde_json::Value::Number(value) => value.to_string(),
            _ => {
                return Err(WorkspaceClientError::Request(
                    "Workspace query fields must be scalar".into(),
                ));
            }
        };
        pairs.push(format!(
            "{}={}",
            encode_workspace_path_segment(name),
            encode_workspace_path_segment(&value)
        ));
    }
    Ok(WorkspaceRequest {
        method,
        path: if pairs.is_empty() {
            path
        } else {
            format!("{path}?{}", pairs.join("&"))
        },
        body: None,
    })
}

fn workspace_server_operation_request(
    workspace_id: &str,
    operation: WorkspaceServerOperation,
) -> Result<WorkspaceRequest, WorkspaceClientError> {
    let workspace_id = encode_workspace_path_segment(workspace_id);
    let base = format!("/api/w/{workspace_id}");
    match operation {
        WorkspaceServerOperation::TicketAssignmentSet {
            ticket_id,
            role,
            request,
        } => {
            let role = match role {
                server_api::TicketAssignmentRole::Orchestrator => "orchestrator",
                server_api::TicketAssignmentRole::Worker => "worker",
                server_api::TicketAssignmentRole::Owner => "owner",
                server_api::TicketAssignmentRole::Contributor => "contributor",
            };
            Ok(WorkspaceRequest::json(
                WorkspaceRequestMethod::Put,
                format!(
                    "{base}/tickets/{}/assignments/{role}",
                    encode_workspace_path_segment(&ticket_id)
                ),
                serde_json::to_string(&request)
                    .map_err(|error| WorkspaceClientError::Request(error.to_string()))?,
            ))
        }
        WorkspaceServerOperation::TicketAssignmentClear {
            ticket_id,
            role,
            query,
        } => {
            let role = match role {
                server_api::TicketAssignmentRole::Orchestrator => "orchestrator",
                server_api::TicketAssignmentRole::Worker => "worker",
                server_api::TicketAssignmentRole::Owner => "owner",
                server_api::TicketAssignmentRole::Contributor => "contributor",
            };
            workspace_server_query_request(
                WorkspaceRequestMethod::Delete,
                format!(
                    "{base}/tickets/{}/assignments/{role}",
                    encode_workspace_path_segment(&ticket_id)
                ),
                &query,
            )
        }
        WorkspaceServerOperation::TicketAssignmentsPage { ticket_id, query } => {
            workspace_server_query_request(
                WorkspaceRequestMethod::Get,
                format!(
                    "/workspaces/{workspace_id}/tickets/{}/assignment-tracking/assignments",
                    encode_workspace_path_segment(&ticket_id)
                ),
                &query,
            )
        }
        WorkspaceServerOperation::NotificationDiagnostics { query } => {
            workspace_server_query_request(
                WorkspaceRequestMethod::Get,
                format!("{base}/notification-diagnostics"),
                &query,
            )
        }
        WorkspaceServerOperation::TicketAssignmentDiagnostics { ticket_id, query } => {
            workspace_server_query_request(
                WorkspaceRequestMethod::Get,
                format!(
                    "/workspaces/{workspace_id}/tickets/{}/assignment-diagnostics",
                    encode_workspace_path_segment(&ticket_id)
                ),
                &query,
            )
        }
        WorkspaceServerOperation::TicketAssignmentAnswer { ticket_id, request } => {
            workspace_server_json_request(
                format!(
                    "/workspaces/{workspace_id}/tickets/{}/assignment-answer",
                    encode_workspace_path_segment(&ticket_id)
                ),
                &request,
            )
        }
        WorkspaceServerOperation::TicketImplementationResult { ticket_id, request } => {
            workspace_server_json_request(
                format!(
                    "/workspaces/{workspace_id}/tickets/{}/implementation-results",
                    encode_workspace_path_segment(&ticket_id)
                ),
                &request,
            )
        }
        WorkspaceServerOperation::TicketImplementationResultGet {
            ticket_id,
            result_id,
        } => Ok(WorkspaceRequest::get(format!(
            "/workspaces/{workspace_id}/tickets/{}/implementation-results/{}",
            encode_workspace_path_segment(&ticket_id),
            encode_workspace_path_segment(&result_id)
        ))),
        WorkspaceServerOperation::WorkerControlList => Ok(WorkspaceRequest::get(format!(
            "{base}/worker-control/workers"
        ))),
        WorkspaceServerOperation::WorkerControlAuthorize {
            runtime_id,
            worker_id,
            permission,
        } => Ok(WorkspaceRequest::get(format!(
            "{base}/worker-control/workers/{}/{}/permissions/{}",
            encode_workspace_path_segment(&runtime_id),
            encode_workspace_path_segment(&worker_id),
            encode_workspace_path_segment(&permission)
        ))),
        WorkspaceServerOperation::WorkerControlSpawn(request) => {
            workspace_server_json_request(format!("{base}/worker-control/workers"), &request)
        }
        WorkspaceServerOperation::WorkerControlInput {
            runtime_id,
            worker_id,
            request,
        } => workspace_server_json_request(
            format!("{base}/worker-control/workers/{runtime_id}/{worker_id}/input"),
            &request,
        ),
        WorkspaceServerOperation::WorkerControlCancel {
            runtime_id,
            worker_id,
            request,
        } => workspace_server_json_request(
            format!("{base}/worker-control/workers/{runtime_id}/{worker_id}/cancel"),
            &request,
        ),
        WorkspaceServerOperation::WorkerControlStop {
            runtime_id,
            worker_id,
            request,
        } => workspace_server_json_request(
            format!("{base}/worker-control/workers/{runtime_id}/{worker_id}/stop"),
            &request,
        ),
        WorkspaceServerOperation::WorkerControlRestore {
            runtime_id,
            worker_id,
            request,
        } => workspace_server_json_request(
            format!(
                "{base}/worker-control/workers/{}/{}/restore",
                encode_workspace_path_segment(&runtime_id),
                encode_workspace_path_segment(&worker_id)
            ),
            &request,
        ),
        WorkspaceServerOperation::WorkerObservationSessions => Ok(WorkspaceRequest::get(format!(
            "{base}/worker-observation/sessions"
        ))),
        WorkspaceServerOperation::WorkerObservationCapture(request) => {
            workspace_server_json_request(format!("{base}/worker-observation/session"), &request)
        }
        WorkspaceServerOperation::SubjektivStageCandidate(request) => {
            workspace_server_json_request(format!("{base}/subjektiv/staging"), &request)
        }
        WorkspaceServerOperation::SubjektivRecordSession(request) => {
            workspace_server_json_request(format!("{base}/subjektiv/sessions"), &request)
        }
        WorkspaceServerOperation::SubjektivMemory(request) => {
            workspace_server_json_request(format!("{base}/subjektiv/memory"), &request)
        }
        WorkspaceServerOperation::SubjektivSession(request) => {
            workspace_server_json_request(format!("{base}/subjektiv/session-history"), &request)
        }
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

    /// Executes one bounded Workspace request when the caller is on a
    /// latency-sensitive lifecycle boundary. Implementations without transport
    /// timeout support retain their existing execution semantics.
    fn execute_with_timeout(
        &self,
        request: WorkspaceRequest,
        _timeout: Duration,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        self.execute(request)
    }

    /// Ask the Host using this client's authenticated source, never a model-supplied
    /// identity. Trusted Features call this before protected effects on every surface.
    /// This is not a universal Tool executor: a missing call is a Feature bug, and
    /// operations not yet adapted to this seam retain only their existing checks.
    fn authorize_operation(
        &self,
        operation: &str,
        resource: &server_api::authorization::PermissionResource,
    ) -> Result<(), WorkspaceClientError> {
        self.authorize_operation_scope(operation, resource)
            .map(|_| ())
    }

    /// Return the Host-selected matching scope for provider-resolved containment.
    /// `None` is only the Host's explicit legacy decision, never a transport fallback.
    fn authorize_operation_scope(
        &self,
        operation: &str,
        resource: &server_api::authorization::PermissionResource,
    ) -> Result<Option<server_api::authorization::PermissionResource>, WorkspaceClientError> {
        use server_api::authorization::{
            OperationAuthorizationRequest, OperationAuthorizationResponse, ScopedPermission,
        };
        if !self.is_available() {
            return Err(WorkspaceClientError::Unavailable(
                "operation authorization requires a Host-bound Workspace client".into(),
            ));
        }
        let workspace_id = self
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                WorkspaceClientError::Unavailable(
                    "operation authorization requires Workspace identity".into(),
                )
            })?;
        ScopedPermission {
            operation: operation.into(),
            resource: resource.clone(),
        }
        .validate()
        .map_err(WorkspaceClientError::Request)?;
        let request = workspace_server_json_request(
            format!(
                "/api/w/{}/authorization/check",
                encode_workspace_path_segment(workspace_id)
            ),
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: resource.clone(),
            },
        )?;
        // Blocking transports must not run inside a Tokio runtime. The bound
        // transport enforces this timeout; no authorization success is cached.
        let execute = || self.execute_with_timeout(request, Duration::from_secs(10));
        let response = if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| scope.spawn(execute).join()).map_err(|_| {
                WorkspaceClientError::Request("authorization request thread panicked".into())
            })??
        } else {
            execute()?
        };
        if response.body.len() > 16 * 1024 {
            return Err(WorkspaceClientError::Request(
                "authorization response exceeded 16 KiB".into(),
            ));
        }
        if !response.is_success() {
            // Expose only the bounded public reason, not provider diagnostics or
            // arbitrary response data. Keep status parseable by Feature adapters.
            let value: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
            let reason: String = value
                .get("message")
                .or_else(|| value.get("error"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Host rejected authorization")
                .chars()
                .take(1024)
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect();
            return Err(WorkspaceClientError::Request(format!(
                "authorization {} for {operation}: HTTP status {}: {reason}",
                if matches!(response.status, 401 | 403) {
                    "denied"
                } else {
                    "request failed"
                },
                response.status
            )));
        }
        let authorization = serde_json::from_str::<OperationAuthorizationResponse>(&response.body)
            .map_err(|error| {
                WorkspaceClientError::Request(format!("invalid authorization response: {error}"))
            })?;
        if authorization
            .scope
            .as_ref()
            .is_some_and(|scope| !scope.covers(resource))
        {
            return Err(WorkspaceClientError::Request(
                "Host authorization scope does not cover requested resource".into(),
            ));
        }
        Ok(authorization.scope)
    }

    fn authorize_network_operation(
        &self,
        operation: &str,
        url: &str,
    ) -> Result<(), WorkspaceClientError> {
        self.authorize_operation(
            operation,
            &server_api::authorization::PermissionResource::Network { url: url.into() },
        )
    }

    /// Authorize a target resolved from the authenticated Host binding. This
    /// never lets a Feature invent its parent Worker or connected Subject ID.
    fn authorize_bound_operation(
        &self,
        operation: &str,
        target: server_api::authorization::BoundAuthorizationTarget,
    ) -> Result<(), WorkspaceClientError> {
        self.authorize_bound_operation_scope(operation, target)
            .map(|_| ())
    }

    fn authorize_bound_operation_scope(
        &self,
        operation: &str,
        target: server_api::authorization::BoundAuthorizationTarget,
    ) -> Result<Option<server_api::authorization::PermissionResource>, WorkspaceClientError> {
        use server_api::authorization::{
            BoundOperationAuthorizationRequest, OperationAuthorizationResponse,
        };
        server_api::authorization::validate_operation_name(operation)
            .map_err(WorkspaceClientError::Request)?;
        let workspace_id = self
            .workspace_id()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                WorkspaceClientError::Unavailable(
                    "bound authorization requires Workspace identity".into(),
                )
            })?;
        if !self.is_available() {
            return Err(WorkspaceClientError::Unavailable(
                "bound authorization requires a live Host connection".into(),
            ));
        }
        let request = workspace_server_json_request(
            format!(
                "/api/w/{}/authorization/bound-check",
                encode_workspace_path_segment(workspace_id)
            ),
            &BoundOperationAuthorizationRequest {
                operation: operation.into(),
                target,
            },
        )?;
        let execute = || self.execute_with_timeout(request, Duration::from_secs(10));
        let response = if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| scope.spawn(execute).join()).map_err(|_| {
                WorkspaceClientError::Request("bound authorization thread panicked".into())
            })??
        } else {
            execute()?
        };
        if response.body.len() > 16 * 1024 {
            return Err(WorkspaceClientError::Request(
                "bound authorization response exceeded 16 KiB".into(),
            ));
        }
        if !response.is_success() {
            return Err(WorkspaceClientError::Request(format!(
                "bound authorization {} for {operation}: HTTP status {}",
                if matches!(response.status, 401 | 403) {
                    "denied"
                } else {
                    "failed"
                },
                response.status,
            )));
        }
        let parsed = serde_json::from_str::<OperationAuthorizationResponse>(&response.body)
            .map_err(|error| {
                WorkspaceClientError::Request(format!(
                    "invalid bound authorization response: {error}"
                ))
            })?;
        if let Some(scope) = &parsed.scope {
            scope.validate().map_err(WorkspaceClientError::Request)?;
            if scope.workspace_id().is_some_and(|id| id != workspace_id) {
                return Err(WorkspaceClientError::Request(
                    "bound authorization returned another Workspace resource".into(),
                ));
            }
        }
        Ok(parsed.scope)
    }

    fn authorize_workdir_attachment_operation(
        &self,
        operation: &str,
        alias: &str,
        expected_connection_id: Option<&str>,
    ) -> Result<String, WorkspaceClientError> {
        let scope = self.authorize_bound_operation_scope(
            operation,
            server_api::authorization::BoundAuthorizationTarget::WorkdirAttachment {
                alias: alias.into(),
                expected_connection_id: expected_connection_id.map(str::to_owned),
            },
        )?;
        match scope {
            Some(server_api::authorization::PermissionResource::WorkdirAttachment {
                connection_id,
                ..
            }) if expected_connection_id.is_none_or(|expected| expected == connection_id) => {
                Ok(connection_id)
            }
            _ => Err(WorkspaceClientError::Request(
                "Host did not resolve the expected Workdir attachment".into(),
            )),
        }
    }

    fn authorize_subject_operation(&self, operation: &str) -> Result<(), WorkspaceClientError> {
        self.authorize_bound_operation(
            operation,
            server_api::authorization::BoundAuthorizationTarget::Subject {},
        )
    }

    fn authorize_subworker_operation(
        &self,
        operation: &str,
        name: &str,
    ) -> Result<(), WorkspaceClientError> {
        self.authorize_bound_operation(
            operation,
            server_api::authorization::BoundAuthorizationTarget::SubWorker { name: name.into() },
        )
    }

    /// Activation is retained only for the temporary MCP exception. Explicit
    /// standalone contexts with no Workspace binding keep local MCP compatibility.
    fn authorize_feature_activation(&self, feature: &str) -> Result<(), WorkspaceClientError> {
        self.authorize_operation(
            feature,
            &server_api::authorization::PermissionResource::Feature,
        )
    }

    /// Binary transport is unavailable unless a host-bound implementation opts in.
    fn execute_binary(
        &self,
        _request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(
            "Workspace binary transport is unavailable".to_string(),
        ))
    }

    fn execute_binary_with_timeout(
        &self,
        request: WorkspaceBinaryRequest,
        _timeout: Duration,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        self.execute_binary(request)
    }

    fn execute_server_operation(
        &self,
        operation: WorkspaceServerOperation,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let workspace_id = self.workspace_id().ok_or_else(|| {
            WorkspaceClientError::Unavailable(
                "Workspace Server operation requires Workspace identity".to_string(),
            )
        })?;
        self.execute(workspace_server_operation_request(workspace_id, operation)?)
    }

    /// Lists Workspace-visible Workers through dedicated Runtime-owned source
    /// proof. Implementations must not fall back to generic Workspace request
    /// authority.
    fn list_workspace_workers(
        &self,
        _request: WorkspaceWorkerDiscoveryRequest,
    ) -> Result<server_api::WorkspaceWorkerDiscoveryPage, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(
            "Workspace Worker discovery authority is unavailable".to_string(),
        ))
    }

    /// Resolve the Workspace's current immutable Prompt projection for future
    /// operation boundaries. Creation and restore continue to use persisted
    /// launch/session state; this hook never reconstructs historical prompts.
    fn current_prompt_projection(
        &self,
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
    pub merge_request_id: String,
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
    ) -> Result<Option<WorkspacePromptCatalogResolution>, WorkspaceClientError> {
        self.inner.current_prompt_projection()
    }

    fn authorize_operation(
        &self,
        operation: &str,
        resource: &server_api::authorization::PermissionResource,
    ) -> Result<(), WorkspaceClientError> {
        // The query has no protected effect. Keep the review capability's
        // mutation filter intact while asking the bound Host for live authority.
        self.inner.authorize_operation(operation, resource)
    }

    fn authorize_operation_scope(
        &self,
        operation: &str,
        resource: &server_api::authorization::PermissionResource,
    ) -> Result<Option<server_api::authorization::PermissionResource>, WorkspaceClientError> {
        self.inner.authorize_operation_scope(operation, resource)
    }

    fn authorize_bound_operation(
        &self,
        operation: &str,
        target: server_api::authorization::BoundAuthorizationTarget,
    ) -> Result<(), WorkspaceClientError> {
        self.inner.authorize_bound_operation(operation, target)
    }

    fn authorize_feature_activation(&self, feature: &str) -> Result<(), WorkspaceClientError> {
        self.inner.authorize_feature_activation(feature)
    }

    fn execute_binary(
        &self,
        request: WorkspaceBinaryRequest,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        if request.method != WorkspaceRequestMethod::Get {
            return Err(WorkspaceClientError::Unavailable(
                "Reviewer child binary Workspace authority is read-only".to_string(),
            ));
        }
        self.inner.execute_binary(request)
    }

    fn execute_binary_with_timeout(
        &self,
        request: WorkspaceBinaryRequest,
        timeout: Duration,
    ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
        if request.method != WorkspaceRequestMethod::Get {
            return Err(WorkspaceClientError::Unavailable(
                "Reviewer child binary Workspace authority is read-only".to_string(),
            ));
        }
        self.inner.execute_binary_with_timeout(request, timeout)
    }

    fn execute(
        &self,
        mut request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        let workspace_id = self.workspace_id().unwrap_or_default();
        let ticket_query_path = format!("/api/w/{workspace_id}/tickets/query");
        let ticket_show_prefix = format!("/api/w/{workspace_id}/tickets/");
        // ShowTicket uses POST for bounded read parameters. Like Ticket query
        // and GET reads, it must allow related Tickets through the bound Host's
        // read authority. Only review submission is restricted to the attested MR.
        // Match one literal reference segment, not arbitrary POST routes.
        let is_ticket_show = request
            .path
            .strip_prefix(&ticket_show_prefix)
            .and_then(|path| path.strip_suffix("/show"))
            .is_some_and(|reference| {
                !reference.is_empty()
                    && reference
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            });
        let merge_request_id = &self.context.merge_request_id;
        let review_path =
            format!("/api/w/{workspace_id}/merge-requests/{merge_request_id}/reviews");
        let read_allowed = request.method == WorkspaceRequestMethod::Get
            || (request.method == WorkspaceRequestMethod::Post
                && (request.path == ticket_query_path || is_ticket_show));
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
    legacy_local_activation: bool,
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

    fn authorize_network_operation(
        &self,
        operation: &str,
        url: &str,
    ) -> Result<(), WorkspaceClientError> {
        if self.workspace_id.is_none() && self.legacy_local_activation {
            return Ok(());
        }
        self.authorize_operation(
            operation,
            &server_api::authorization::PermissionResource::Network { url: url.into() },
        )
    }

    fn authorize_bound_operation(
        &self,
        _operation: &str,
        target: server_api::authorization::BoundAuthorizationTarget,
    ) -> Result<(), WorkspaceClientError> {
        if self.workspace_id.is_none()
            && self.legacy_local_activation
            && matches!(
                target,
                server_api::authorization::BoundAuthorizationTarget::SubWorker { .. }
            )
        {
            return Ok(());
        }
        Err(WorkspaceClientError::Unavailable(self.reason.clone()))
    }

    fn authorize_feature_activation(&self, feature: &str) -> Result<(), WorkspaceClientError> {
        // Explicit compatibility for standalone Workers with no Workspace binding.
        // A missing/broken Workspace client must never become an implicit grant.
        if self.workspace_id.is_none() && self.legacy_local_activation {
            return Ok(());
        }
        self.authorize_operation(
            feature,
            &server_api::authorization::PermissionResource::Feature,
        )
    }

    fn execute(
        &self,
        _request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        Err(WorkspaceClientError::Unavailable(self.reason.clone()))
    }
}

#[cfg(test)]
mod operation_authorization_tests {
    use super::*;
    use server_api::authorization::PermissionResource;

    #[derive(Debug)]
    struct BoundClient {
        response: Mutex<WorkspaceResponse>,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl WorkspaceClient for BoundClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace/test")
        }
        fn kind(&self) -> &str {
            "bound-authorization-fixture"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
            panic!("authorization must select the bounded transport")
        }
        fn execute_with_timeout(
            &self,
            request: WorkspaceRequest,
            timeout: Duration,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            assert_eq!(timeout, Duration::from_secs(10));
            self.requests.lock().unwrap().push(request);
            Ok(self.response.lock().unwrap().clone())
        }
    }

    #[tokio::test]
    async fn authorization_is_bounded_live_and_contains_no_caller_identity() {
        let client = BoundClient {
            response: Mutex::new(WorkspaceResponse {
                status: 200,
                body: "{}".into(),
            }),
            requests: Default::default(),
        };
        let resource = PermissionResource::Workspace {
            workspace_id: "workspace/test".into(),
        };
        client
            .authorize_operation("ticket.create", &resource)
            .unwrap();
        *client.response.lock().unwrap() = WorkspaceResponse {
            status: 403,
            body: r#"{"message":"permission revoked","diagnostics":[{"message":"private"}]}"#
                .into(),
        };
        let reason = client
            .authorize_operation("ticket.create", &resource)
            .unwrap_err()
            .to_string();
        assert!(reason.contains("HTTP status 403"));
        assert!(reason.contains("permission revoked"));
        assert!(!reason.contains("private"));
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "success must not become a cached grant");
        for request in requests.iter() {
            assert_eq!(request.method, WorkspaceRequestMethod::Post);
            assert_eq!(request.path, "/api/w/workspace%2Ftest/authorization/check");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(request.body.as_deref().unwrap())
                    .unwrap(),
                serde_json::json!({ "operation":"ticket.create", "resource":{"kind":"workspace", "workspace_id":"workspace/test"} })
            );
        }
    }

    #[test]
    fn scoped_authorization_preserves_matching_scope_and_rejects_cross_resource() {
        let resource = PermissionResource::Workdir {
            working_directory_id: "wd-1".into(),
            path: "docs/file.md".into(),
            recursive: false,
        };
        let granted = PermissionResource::Workdir {
            working_directory_id: "wd-1".into(),
            path: "docs".into(),
            recursive: true,
        };
        let client = BoundClient {
            response: Mutex::new(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"scope":granted}).to_string(),
            }),
            requests: Default::default(),
        };
        assert_eq!(
            client
                .authorize_operation_scope("filesystem.read", &resource)
                .unwrap(),
            Some(granted)
        );
        *client.response.lock().unwrap() = WorkspaceResponse {
            status: 200,
            body: serde_json::json!({"scope":{"kind":"workspace","workspace_id":"workspace/test"}})
                .to_string(),
        };
        assert!(
            client
                .authorize_operation_scope("filesystem.read", &resource)
                .is_err()
        );
    }

    #[test]
    fn reviewer_query_preserves_host_scope_and_does_not_widen_mutation_access() {
        let scope = PermissionResource::Workdir {
            working_directory_id: "wd-1".into(),
            path: "docs".into(),
            recursive: true,
        };
        let inner = Arc::new(BoundClient {
            response: Mutex::new(WorkspaceResponse {
                status: 200,
                body: serde_json::json!({"scope":scope}).to_string(),
            }),
            requests: Default::default(),
        });
        let reviewer = ReviewerChildWorkspaceClient::new(
            inner.clone(),
            ReviewerContext {
                ticket_id: "T-1".into(),
                merge_request_id: "MR-1".into(),
            },
            "capability".into(),
        );
        let requested = PermissionResource::Workdir {
            working_directory_id: "wd-1".into(),
            path: "docs/file.md".into(),
            recursive: false,
        };
        assert_eq!(
            reviewer
                .authorize_operation_scope("filesystem.read", &requested)
                .unwrap(),
            Some(scope)
        );
        assert!(
            reviewer
                .execute(WorkspaceRequest::json(
                    WorkspaceRequestMethod::Post,
                    "/api/w/workspace%2Ftest/tickets",
                    "{}"
                ))
                .is_err()
        );
        assert_eq!(
            inner.requests.lock().unwrap().len(),
            1,
            "only the bounded authorization query reaches the Host"
        );
        *inner.response.lock().unwrap() = WorkspaceResponse {
            status: 403,
            body: "{}".into(),
        };
        assert!(
            reviewer
                .authorize_operation_scope("filesystem.read", &requested)
                .is_err()
        );
    }

    #[test]
    fn malformed_and_oversized_authorization_responses_fail_closed() {
        for body in [
            "not-json".into(),
            "{\"unexpected_grant\":true}".into(),
            " ".repeat(16 * 1024 + 1),
        ] {
            let client = BoundClient {
                response: Mutex::new(WorkspaceResponse { status: 200, body }),
                requests: Default::default(),
            };
            assert!(
                client
                    .authorize_operation("ticket", &PermissionResource::Feature)
                    .is_err()
            );
        }
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
                merge_request_id: "MR1".into(),
            },
            "secret".into(),
        )
    }

    #[test]
    fn binary_transport_defaults_to_denial_without_json_fallback() {
        let client = RecordingWorkspaceClient::default();
        let request = WorkspaceBinaryRequest {
            method: WorkspaceRequestMethod::Get,
            path: "/api/w/ws/drive/entries/a/content".into(),
            body: None,
            max_response_bytes: 64 * 1024,
        };
        assert!(matches!(
            client.execute_binary(request.clone()),
            Err(WorkspaceClientError::Unavailable(_))
        ));
        assert!(matches!(
            client.execute_binary_with_timeout(request, Duration::from_secs(1)),
            Err(WorkspaceClientError::Unavailable(_))
        ));
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[derive(Debug, Default)]
    struct BinaryRecordingClient {
        requests: Mutex<Vec<(WorkspaceBinaryRequest, Option<Duration>)>>,
    }

    impl WorkspaceClient for BinaryRecordingClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("ws")
        }
        fn kind(&self) -> &str {
            "binary-recording"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
            panic!("binary request must not use JSON fallback")
        }
        fn execute_binary(
            &self,
            request: WorkspaceBinaryRequest,
        ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push((request, None));
            Ok(WorkspaceBinaryResponse {
                status: 200,
                body: vec![0, 255],
            })
        }
        fn execute_binary_with_timeout(
            &self,
            request: WorkspaceBinaryRequest,
            timeout: Duration,
        ) -> Result<WorkspaceBinaryResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push((request, Some(timeout)));
            Ok(WorkspaceBinaryResponse {
                status: 200,
                body: vec![0, 255],
            })
        }
    }

    #[test]
    fn reviewer_binary_get_preserves_request_limit_and_timeout() {
        let inner = Arc::new(BinaryRecordingClient::default());
        let client = reviewer_client(inner.clone());
        let request = WorkspaceBinaryRequest {
            method: WorkspaceRequestMethod::Get,
            path: "/api/w/ws/drive/entries/a/content?offset=2&limit=4".into(),
            body: None,
            max_response_bytes: 4,
        };
        assert_eq!(
            client.execute_binary(request.clone()).unwrap().body,
            vec![0, 255]
        );
        let timeout = Duration::from_secs(2);
        assert_eq!(
            client
                .execute_binary_with_timeout(request.clone(), timeout)
                .unwrap()
                .body,
            vec![0, 255]
        );
        assert_eq!(
            *inner.requests.lock().unwrap(),
            vec![(request.clone(), None), (request, Some(timeout))]
        );
    }

    #[test]
    fn reviewer_binary_mutations_are_denied_even_for_attested_review_path() {
        let inner = Arc::new(BinaryRecordingClient::default());
        let client = reviewer_client(inner.clone());
        for method in [
            WorkspaceRequestMethod::Post,
            WorkspaceRequestMethod::Put,
            WorkspaceRequestMethod::Patch,
            WorkspaceRequestMethod::Delete,
        ] {
            let request = WorkspaceBinaryRequest {
                method,
                path: "/api/w/ws/merge-requests/MR1/reviews".into(),
                body: Some(vec![0, 255]),
                max_response_bytes: 4,
            };
            assert!(matches!(
                client.execute_binary(request.clone()),
                Err(WorkspaceClientError::Unavailable(_))
            ));
            assert!(matches!(
                client.execute_binary_with_timeout(request, Duration::from_secs(1)),
                Err(WorkspaceClientError::Unavailable(_))
            ));
        }
        assert!(inner.requests.lock().unwrap().is_empty());
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
    fn reviewer_child_client_rejects_mutation_posts() {
        let inner: Arc<dyn WorkspaceClient> = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner);

        for request in [
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
    fn reviewer_ticket_reads_reject_route_injection_and_mutations_without_transport() {
        let inner = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner.clone());
        for path in [
            "/api/w/other/tickets/T-1/show",
            "/api/w/ws/tickets//show",
            "/api/w/ws/tickets/T-1/comments",
            "/api/w/ws/tickets/T-1/posts",
            "/api/w/ws/tickets/T-1/show/",
            "/api/w/ws/tickets/T-1/show?query=1",
            "/api/w/ws/tickets/T-1?query=1/show",
            "/api/w/ws/tickets/T-1#fragment/show",
            "/api/w/ws/tickets/../show",
            "/api/w/ws/tickets/T-1/../T-2/show",
            "/api/w/ws/tickets/T%2D1/show",
            "/api/w/ws/tickets/T-1%2fcomments/show",
            "/api/w/ws/merge-requests/MR2/reviews",
        ] {
            assert!(
                client
                    .execute(WorkspaceRequest::json(
                        WorkspaceRequestMethod::Post,
                        path,
                        "{}",
                    ))
                    .is_err(),
                "{path}"
            );
        }
        for method in [
            WorkspaceRequestMethod::Put,
            WorkspaceRequestMethod::Patch,
            WorkspaceRequestMethod::Delete,
        ] {
            assert!(
                client
                    .execute(WorkspaceRequest::json(
                        method,
                        "/api/w/ws/tickets/T-1/show",
                        "{}"
                    ))
                    .is_err()
            );
        }
        assert!(inner.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn reviewer_child_client_injects_capability_only_for_attested_review() {
        let inner = Arc::new(RecordingWorkspaceClient::default());
        let client = reviewer_client(inner.clone());
        client
            .execute(WorkspaceRequest::json(
                WorkspaceRequestMethod::Post,
                "/api/w/ws/merge-requests/MR1/reviews",
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
        legacy_local_activation: workspace_id.is_none(),
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
        legacy_local_activation: false,
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
        // This constructor explicitly selects legacy standalone activation only
        // without Workspace identity; it never authorizes protected operations.
        let client = Arc::new(MarkerWorkspaceClient {
            workspace_id: workspace_id.as_ref().map(|id| id.as_str().to_string()),
            kind: "local-filesystem".into(),
            available: true,
            legacy_local_activation: workspace_id.is_none(),
            reason: "local filesystem context does not expose Workspace operations".into(),
        });
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

use crate::metadata_lifecycle::{
    WorkerMetadataSegmentCas, WorkerMetadataWriter, worker_metadata_segment_cas_for_store,
    worker_metadata_writer_for_store,
};

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
    append_lock: Mutex<()>,
    mutation_authority: Arc<crate::subject_release::MutationAuthority>,
    activation_failed: AtomicBool,
}

impl SegmentState {
    pub fn new(session_id: SessionId, segment_id: SegmentId, entries_written: usize) -> Arc<Self> {
        Arc::new(Self {
            location: ArcSwap::from_pointee(SegmentLocation {
                session_id,
                segment_id,
            }),
            entries_written: AtomicUsize::new(entries_written),
            append_lock: Mutex::new(()),
            mutation_authority: Arc::default(),
            activation_failed: AtomicBool::new(false),
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

    fn ensure_append_allowed(&self) -> Result<(), StoreError> {
        if self.activation_failed.load(Ordering::Acquire) {
            return Err(StoreError::Io(std::io::Error::other(
                "Segment activation is incomplete; restore the Worker before accepting more input",
            )));
        }
        Ok(())
    }

    fn fail_closed_activation(&self) {
        self.activation_failed.store(true, Ordering::Release);
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
        Item::AssistantResponseBoundary { .. }
        | Item::ToolCall { .. }
        | Item::ToolResult { .. }
        | Item::Reasoning { .. } => true,
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
        } else if item.is_assistant_message()
            || matches!(item, Item::AssistantResponseBoundary { .. })
            || item.is_tool_call()
            || item.is_reasoning()
        {
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
    pub fn append_entry(&self, mut entry: LogEntry) -> Result<(), StoreError> {
        entry.ensure_session_entry_id();
        let _append_guard = self
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        self.append_entry_locked(entry)
    }

    fn append_entry_locked(&self, entry: LogEntry) -> Result<(), StoreError> {
        let _admission = self.state.mutation_authority.enter()?;
        self.state.ensure_append_allowed()?;
        let loc = self.state.location();
        self.store.append(loc.session_id, loc.segment_id, &entry)?;
        self.state.increment_entries();
        if let Some(in_flight) = &self.in_flight {
            let committed_item = match &entry {
                LogEntry::AnnotatedAssistantItem { entry, .. } => {
                    Some((entry.item.clone(), entry.metadata.entry_id.0.clone()))
                }
                _ => None,
            };
            if let Some((item_for_clear, entry_id)) = committed_item {
                in_flight.clear_for_committed_item_then(&item_for_clear, Some(&entry_id), || {
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

#[derive(Debug, Clone)]
pub(crate) enum PendingActivation {
    Submission(PendingSubmission),
}

#[derive(Debug, Clone)]
pub(crate) struct SubmissionAcceptance {
    pub(crate) submission_request_id: String,
    pub(crate) submission_id: String,
    pub(crate) disposition: protocol::SubmissionDisposition,
    pub(crate) activation: Option<PendingSubmission>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PendingSubmissionError {
    #[error("submission_request_id must not be empty")]
    EmptyRequestId,
    #[error("activation request id exceeds {MAX_ACTIVATION_REQUEST_ID_BYTES} bytes")]
    RequestIdLimit,
    #[error("submission input must contain at least one typed segment")]
    EmptyInput,
    #[error("Feature invocation rejected: {0}")]
    InvalidInvocation(String),
    #[error("submission request id was already used with a different payload")]
    IdempotencyConflict,
    #[error("pending submission queue is full (maximum {MAX_PENDING_SUBMISSIONS})")]
    CountLimit,
    #[error("pending submission bytes exceed {MAX_PENDING_SUBMISSION_BYTES}")]
    ByteLimit,
    #[error("pending submission artifact references exceed {MAX_PENDING_ARTIFACT_REFS}")]
    ArtifactLimit,
    #[error("pending queue content conflict: expected {expected:?}, current {current:?}")]
    ContentConflict {
        expected: Vec<String>,
        current: Vec<String>,
    },
    #[error("pending queue head conflict: expected {expected}, current {current:?}")]
    HeadConflict {
        expected: String,
        current: Option<String>,
    },
    #[error("pending submission not found: {0}")]
    NotFound(String),
    #[error("pending submission state persistence failed: {0}")]
    Store(#[from] StoreError),
}

#[derive(Clone)]
pub(crate) struct PendingSubmissionHandle<St: Clone> {
    state: Arc<Mutex<PendingActivationState>>,
    writer: LogWriterHandle<St>,
}

impl<St> PendingSubmissionHandle<St>
where
    St: Store + Clone,
{
    fn validate_head(
        state: &PendingActivationState,
        expected: &str,
    ) -> Result<(), PendingSubmissionError> {
        let current = state.snapshot().head_id;
        if current.as_deref() != Some(expected) {
            return Err(PendingSubmissionError::HeadConflict {
                expected: expected.to_owned(),
                current,
            });
        }
        Ok(())
    }

    fn persist_locked(&self, state: &PendingActivationState) -> Result<(), PendingSubmissionError> {
        self.writer.append_entry_locked(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.to_owned(),
            payload: serde_json::to_value(state).expect("pending activation state must serialize"),
        })?;
        Ok(())
    }

    fn pin_submission_files(
        &self,
        pending: &PendingSubmission,
    ) -> Result<(), PendingSubmissionError> {
        let session_id = self.writer.state.location().session_id;
        let mut pinned = Vec::new();
        for reference in submission_uploaded_file_refs(&pending.input) {
            if pinned.iter().any(|existing: &protocol::UploadedFileRef| {
                existing.artifact_id == reference.artifact_id
            }) {
                continue;
            }
            if let Err(pin_error) =
                self.writer
                    .store
                    .pin_uploaded_file(session_id, reference, &pending.submission_id)
            {
                let mut rollback_error = None;
                for acquired in pinned.iter().rev() {
                    if let Err(error) = self.writer.store.release_uploaded_file_pin(
                        session_id,
                        &acquired.artifact_id,
                        &pending.submission_id,
                    ) {
                        rollback_error.get_or_insert(error);
                    }
                }
                return Err(rollback_error.unwrap_or(pin_error).into());
            }
            pinned.push(reference.clone());
        }
        Ok(())
    }

    fn release_submission_files(
        &self,
        pending: &PendingSubmission,
    ) -> Result<(), PendingSubmissionError> {
        let session_id = self.writer.state.location().session_id;
        let mut released = Vec::new();
        let mut first_error = None;
        for reference in submission_uploaded_file_refs(&pending.input) {
            if released
                .iter()
                .any(|artifact_id: &String| artifact_id == &reference.artifact_id)
            {
                continue;
            }
            if let Err(error) = self.writer.store.release_uploaded_file_pin(
                session_id,
                &reference.artifact_id,
                &pending.submission_id,
            ) {
                first_error.get_or_insert(error);
            }
            released.push(reference.artifact_id.clone());
        }
        match first_error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) fn accept(
        &self,
        submission_request_id: String,
        input: Vec<Segment>,
        activate_now: bool,
    ) -> Result<SubmissionAcceptance, PendingSubmissionError> {
        self.accept_from_source(
            submission_request_id,
            input,
            self.direct_client_namespace(),
            WorkerHistoryProvenance::LegacyUnknown,
            activate_now,
        )
    }

    pub(crate) fn accept_from_source(
        &self,
        submission_request_id: String,
        input: Vec<Segment>,
        source_namespace: String,
        provenance: WorkerHistoryProvenance,
        activate_now: bool,
    ) -> Result<SubmissionAcceptance, PendingSubmissionError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        if submission_request_id.trim().is_empty() {
            return Err(PendingSubmissionError::EmptyRequestId);
        }
        if submission_request_id.len() > MAX_ACTIVATION_REQUEST_ID_BYTES {
            return Err(PendingSubmissionError::RequestIdLimit);
        }
        if input.is_empty() {
            return Err(PendingSubmissionError::EmptyInput);
        }
        validate_activation_invocation_ids(&input)
            .map_err(PendingSubmissionError::InvalidInvocation)?;
        let payload_digest = submission_payload_digest(&input);
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        self.writer.state.ensure_append_allowed()?;
        let mut current = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let original = current.clone();
        if let Some(receipt) = current.receipts.iter().find(|receipt| {
            receipt.submission_request_id == submission_request_id
                && receipt.source_namespace == source_namespace
        }) {
            if receipt.payload_digest != payload_digest {
                return Err(PendingSubmissionError::IdempotencyConflict);
            }
            return Ok(SubmissionAcceptance {
                submission_request_id,
                submission_id: receipt.submission_id.clone(),
                disposition: receipt.disposition,
                activation: None,
            });
        }

        let submission_id = uuid::Uuid::now_v7().to_string();
        let pending = PendingSubmission {
            submission_request_id: submission_request_id.clone(),
            source_namespace: source_namespace.clone(),
            submission_id: submission_id.clone(),
            payload_digest: payload_digest.clone(),
            accepted_at_ms: segment_log::now_millis(),
            activation_sequence: current.next_activation_sequence,
            provenance,
            was_queued: !activate_now,
            input,
        };
        current.next_activation_sequence = current.next_activation_sequence.saturating_add(1);
        let disposition = if activate_now {
            protocol::SubmissionDisposition::Started
        } else {
            protocol::SubmissionDisposition::Queued
        };
        current.remember_receipt(SubmissionReceipt {
            submission_request_id: submission_request_id.clone(),
            source_namespace,
            submission_id: submission_id.clone(),
            payload_digest,
            disposition,
        });

        if activate_now {
            current.activating = Some(pending.clone());
        } else {
            let count = current
                .pending
                .len()
                .saturating_add(current.pending_notifications.len())
                .saturating_add(1);
            if count > MAX_PENDING_SUBMISSIONS {
                *current = original;
                return Err(PendingSubmissionError::CountLimit);
            }
            let bytes = current
                .pending
                .iter()
                .map(|pending| submission_payload_len(&pending.input))
                .sum::<u64>()
                .saturating_add(
                    current
                        .pending_notifications
                        .iter()
                        .map(|pending| u64::try_from(pending.message.len()).unwrap_or(u64::MAX))
                        .sum::<u64>(),
                )
                .saturating_add(submission_payload_len(&pending.input));
            if bytes > MAX_PENDING_SUBMISSION_BYTES {
                *current = original;
                return Err(PendingSubmissionError::ByteLimit);
            }
            let artifact_refs = current
                .pending
                .iter()
                .map(|pending| submission_artifact_ref_count(&pending.input))
                .sum::<usize>()
                .saturating_add(submission_artifact_ref_count(&pending.input));
            if artifact_refs > MAX_PENDING_ARTIFACT_REFS {
                *current = original;
                return Err(PendingSubmissionError::ArtifactLimit);
            }
            current.pending.push_back(pending.clone());
        }
        if !activate_now {
            if let Err(error) = self.pin_submission_files(&pending) {
                *current = original;
                return Err(error);
            }
            if let Err(error) = self.persist_locked(&current) {
                let _ = self.release_submission_files(&pending);
                *current = original;
                return Err(error);
            }
        }
        Ok(SubmissionAcceptance {
            submission_request_id,
            submission_id,
            disposition,
            activation: activate_now.then_some(pending),
        })
    }

    #[cfg(test)]
    pub(crate) fn accept_notification(
        &self,
        notification_request_id: String,
        message: String,
    ) -> Result<bool, PendingSubmissionError> {
        self.accept_notification_from_source(
            notification_request_id,
            message,
            self.direct_client_namespace(),
            WorkerHistoryProvenance::LegacyUnknown,
        )
    }

    pub(crate) fn accept_notification_from_source(
        &self,
        notification_request_id: String,
        message: String,
        source_namespace: String,
        provenance: WorkerHistoryProvenance,
    ) -> Result<bool, PendingSubmissionError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        if notification_request_id.trim().is_empty() {
            return Err(PendingSubmissionError::EmptyRequestId);
        }
        if notification_request_id.len() > MAX_ACTIVATION_REQUEST_ID_BYTES {
            return Err(PendingSubmissionError::RequestIdLimit);
        }
        let payload_digest = notification_payload_digest(&message);
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        self.writer.state.ensure_append_allowed()?;
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        if let Some(receipt) = state.notification_receipts.iter().find(|receipt| {
            receipt.notification_request_id == notification_request_id
                && receipt.source_namespace == source_namespace
        }) {
            if receipt.payload_digest != payload_digest {
                return Err(PendingSubmissionError::IdempotencyConflict);
            }
            return Ok(false);
        }
        if state
            .pending
            .len()
            .saturating_add(state.pending_notifications.len())
            >= MAX_PENDING_SUBMISSIONS
        {
            return Err(PendingSubmissionError::CountLimit);
        }
        let queued_bytes = state
            .pending
            .iter()
            .map(|pending| submission_payload_len(&pending.input))
            .sum::<u64>()
            .saturating_add(
                state
                    .pending_notifications
                    .iter()
                    .map(|pending| u64::try_from(pending.message.len()).unwrap_or(u64::MAX))
                    .sum::<u64>(),
            )
            .saturating_add(u64::try_from(message.len()).unwrap_or(u64::MAX));
        if queued_bytes > MAX_PENDING_SUBMISSION_BYTES {
            return Err(PendingSubmissionError::ByteLimit);
        }
        let original = state.clone();
        let activation_sequence = state.next_activation_sequence;
        state.next_activation_sequence = state.next_activation_sequence.saturating_add(1);
        state.pending_notifications.push_back(PendingNotification {
            notification_request_id: notification_request_id.clone(),
            source_namespace: source_namespace.clone(),
            message,
            payload_digest: payload_digest.clone(),
            accepted_at_ms: segment_log::now_millis(),
            activation_sequence,
            provenance,
        });
        state.remember_notification_receipt(NotificationReceipt {
            notification_request_id,
            source_namespace,
            payload_digest,
        });
        if let Err(error) = self.persist_locked(&state) {
            *state = original;
            return Err(error);
        }
        Ok(true)
    }

    pub(crate) fn oldest_pending_notification_accepted_at_ms(&self) -> Option<u64> {
        let state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        state
            .activating_notifications
            .front()
            .into_iter()
            .chain(state.pending_notifications.front())
            .map(|notification| notification.accepted_at_ms)
            .min()
    }

    pub(crate) fn prepare_notification_batch(
        &self,
    ) -> Vec<(PendingNotification, NotificationCommitIdentity)> {
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let notifications = state.pending_notifications.drain(..).collect::<Vec<_>>();
        if notifications.is_empty() {
            return Vec::new();
        }
        state
            .activating_notifications
            .extend(notifications.iter().cloned());

        notifications
            .into_iter()
            .map(|notification| {
                let identity = NotificationCommitIdentity {
                    request_id: notification.notification_request_id.clone(),
                    source_namespace: notification.source_namespace.clone(),
                };
                (notification, identity)
            })
            .collect()
    }

    pub(crate) fn prepare_next_activation(
        &self,
        fence: Option<&str>,
    ) -> Result<Option<PendingActivation>, PendingSubmissionError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        if let Some(expected_head_id) = fence {
            Self::validate_head(&state, expected_head_id)?;
        }
        if state.activating.is_some() {
            return Ok(None);
        }
        let Some(pending) = state.pending.pop_front() else {
            return Ok(None);
        };
        state.activating = Some(pending.clone());
        Ok(Some(PendingActivation::Submission(pending)))
    }

    pub(crate) fn abort_activation(&self, pending: PendingSubmission) {
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        if state.activating.as_ref().map(|item| &item.submission_id) != Some(&pending.submission_id)
        {
            return;
        }
        state.activating = None;
        if pending.was_queued {
            state.pending.push_front(pending.clone());
        } else {
            state
                .receipts
                .retain(|receipt| receipt.submission_id != pending.submission_id);
        }
        if let Err(error) = self.persist_locked(&state) {
            tracing::error!(error = %error, "failed to persist aborted pending activation");
        }
    }

    pub(crate) fn activation_extension(&self) -> SessionExtension {
        let state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let mut committed = state.clone();
        if let Some(activating) = &committed.activating
            && let Some(receipt) = committed
                .receipts
                .iter_mut()
                .find(|receipt| receipt.submission_id == activating.submission_id)
        {
            receipt.disposition = protocol::SubmissionDisposition::Started;
        }
        committed.activating = None;
        pending_activation_extension(&committed)
    }

    pub(crate) fn finish_activation(&self, submission_id: &str) {
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        if state
            .activating
            .as_ref()
            .map(|item| item.submission_id.as_str())
            == Some(submission_id)
        {
            if let Some(receipt) = state
                .receipts
                .iter_mut()
                .find(|receipt| receipt.submission_id == submission_id)
            {
                receipt.disposition = protocol::SubmissionDisposition::Started;
            }
            state.activating = None;
        }
    }

    pub(crate) fn direct_client_namespace(&self) -> String {
        format!("direct:{}", self.writer.state.location().session_id)
    }

    pub(crate) fn has_unfinished_admission(&self) -> bool {
        self.state
            .lock()
            .expect("pending activation state poisoned")
            .has_unfinished_admission()
    }

    pub(crate) fn snapshot(&self) -> protocol::PendingSubmissionsSnapshot {
        self.state
            .lock()
            .expect("pending activation state poisoned")
            .snapshot()
    }

    fn reconcile_uploaded_file_pins(&self) -> Result<u64, StoreError> {
        let live_owner_ids = self
            .state
            .lock()
            .expect("pending activation state poisoned")
            .live_artifact_pin_owner_ids();
        Ok(self
            .writer
            .store
            .reconcile_uploaded_file_pins(self.writer.state.session_id(), &live_owner_ids)?)
    }

    pub(crate) fn cancel(
        &self,
        submission_id: &str,
    ) -> Result<protocol::PendingSubmissionsSnapshot, PendingSubmissionError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let original = state.clone();
        let Some(index) = state
            .pending
            .iter()
            .position(|pending| pending.submission_id == submission_id)
        else {
            return Err(PendingSubmissionError::NotFound(submission_id.to_owned()));
        };
        let removed = state
            .pending
            .remove(index)
            .expect("located pending submission must exist");
        if let Err(error) = self.persist_locked(&state) {
            *state = original;
            return Err(error);
        }
        self.release_submission_files(&removed)?;
        Ok(state.snapshot())
    }

    pub(crate) fn clear(
        &self,
        expected_submission_ids: &[String],
    ) -> Result<protocol::PendingSubmissionsSnapshot, PendingSubmissionError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        let _append_guard = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let original = state.clone();
        let current = state
            .pending
            .iter()
            .map(|pending| pending.submission_id.clone())
            .collect::<Vec<_>>();
        if current != expected_submission_ids {
            return Err(PendingSubmissionError::ContentConflict {
                expected: expected_submission_ids.to_vec(),
                current,
            });
        }
        let removed = state.pending.drain(..).collect::<Vec<_>>();
        if let Err(error) = self.persist_locked(&state) {
            *state = original;
            return Err(error);
        }
        for pending in &removed {
            self.release_submission_files(pending)?;
        }
        Ok(state.snapshot())
    }
}

impl PendingSubmissionHandle<session_store::FsStore> {
    #[cfg(test)]
    pub(crate) fn for_test(root: &std::path::Path) -> Self {
        let store = session_store::FsStore::new(root).expect("test session store");
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        store
            .create_segment(session_id, segment_id, &[])
            .expect("test session segment");
        Self {
            state: Arc::new(Mutex::new(PendingActivationState::default())),
            writer: LogWriterHandle {
                store,
                state: SegmentState::new(session_id, segment_id, 0),
                sink: SegmentLogSink::new(),
                in_flight: None,
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn persisted_entries_for_test(&self) -> Vec<LogEntry> {
        let location = self.writer.state.location();
        self.writer
            .store
            .read_all(location.session_id, location.segment_id)
            .expect("read test pending entries")
    }
}

/// Identity of one staged notification, never a frozen full-state checkpoint.
#[derive(Debug, Clone)]
pub struct NotificationCommitIdentity {
    pub(crate) request_id: String,
    pub(crate) source_namespace: String,
}

pub(crate) trait DurableNotificationCommitter: Send + Sync {
    fn commit_notification(
        &self,
        identity: &NotificationCommitIdentity,
        item: SystemItem,
        provenance: Option<WorkerHistoryProvenance>,
    ) -> Result<Option<HistoryEntry<SessionHistoryMetadata>>, StoreError>;
}

impl<St: Store + Clone> DurableNotificationCommitter for PendingSubmissionHandle<St> {
    fn commit_notification(
        &self,
        identity: &NotificationCommitIdentity,
        item: SystemItem,
        provenance: Option<WorkerHistoryProvenance>,
    ) -> Result<Option<HistoryEntry<SessionHistoryMetadata>>, StoreError> {
        let _admission = self.writer.state.mutation_authority.enter()?;
        let _append = self
            .writer
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let mut state = self
            .state
            .lock()
            .expect("pending activation state poisoned");
        let matches = |notification: &PendingNotification| {
            notification.notification_request_id == identity.request_id
                && notification.source_namespace == identity.source_namespace
        };
        let Some(index) = state.activating_notifications.iter().position(matches) else {
            // A partial drain may already have committed this notification. Its
            // original history entry remains authoritative; never append it twice.
            if !state.pending_notifications.iter().any(matches)
                && state.notification_receipts.iter().any(|receipt| {
                    receipt.notification_request_id == identity.request_id
                        && receipt.source_namespace == identity.source_namespace
                })
            {
                return Ok(None);
            }
            return Err(StoreError::Io(std::io::Error::other(
                "notification is not staged at the commit boundary",
            )));
        };
        if index != 0 {
            return Err(StoreError::Io(std::io::Error::other(
                "notification commit would violate FIFO order",
            )));
        }
        let notification = &state.activating_notifications[index];
        if !matches!(&item, SystemItem::Notification { message, .. } if message == &notification.message)
            || provenance.as_ref() != Some(&notification.provenance)
        {
            return Err(StoreError::Io(std::io::Error::other(
                "notification does not match accepted input authority",
            )));
        }
        let mut committed = state.clone();
        committed.activating_notifications.pop_front();
        let metadata = new_history_metadata(
            provenance.unwrap_or(WorkerHistoryProvenance::LegacyUnknown),
            None,
        );
        let history_item = item.to_history_item();
        self.writer
            .append_entry_locked(LogEntry::AnnotatedSystemItem {
                ts: segment_log::now_millis(),
                entry: session_store::LoggedSystemHistoryEntry {
                    item,
                    metadata: metadata.clone(),
                },
                extensions: vec![pending_activation_extension(&committed)],
            })?;
        *state = committed;
        Ok(Some(HistoryEntry::new(history_item, metadata)))
    }
}

/// Type-erased commit handle for the interceptor. Lets the interceptor commit `SystemItem`s without being generic over the
/// concrete `Store` type.
pub trait SystemItemCommitter: Send + Sync {
    fn commit_log_entry(&self, entry: LogEntry) -> Result<(), StoreError>;

    /// Commit one authoritative usage record while holding the same append
    /// barrier across its in-memory publication, guard rearm, and correlated
    /// post-request metrics. Metric appends remain best-effort.
    fn commit_usage_boundary(
        &self,
        entry: LogEntry,
        after_commit: &mut dyn FnMut(),
        post_metrics: &[session_metrics::Metric],
    ) -> Result<(), StoreError> {
        self.commit_log_entry(entry)?;
        after_commit();
        for metric in post_metrics {
            let payload = serde_json::to_value(metric).expect("Metric is Serialize");
            let _ = self.commit_log_entry(LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: session_metrics::DOMAIN.into(),
                payload,
            });
        }
        Ok(())
    }

    fn commit_system_item_with_extensions(
        &self,
        item: SystemItem,
        extensions: Vec<SessionExtension>,
        history_provenance: Option<WorkerHistoryProvenance>,
    ) -> Result<HistoryEntry<SessionHistoryMetadata>, StoreError> {
        self.commit_system_item_with_extensions_then(
            item,
            extensions,
            history_provenance,
            &mut |_| {},
        )
    }

    /// Notify after durable append through the writer's publication boundary.
    /// Completion is a storage fact, not a grant of continuing Feature authority;
    /// producers must separately fence their representation lifetime.
    fn commit_system_item_with_extensions_then(
        &self,
        item: SystemItem,
        extensions: Vec<SessionExtension>,
        history_provenance: Option<WorkerHistoryProvenance>,
        after_commit: &mut dyn FnMut(&HistoryEntry<SessionHistoryMetadata>),
    ) -> Result<HistoryEntry<SessionHistoryMetadata>, StoreError> {
        let metadata = new_history_metadata(
            history_provenance
                .unwrap_or(WorkerHistoryProvenance::BackendInstruction { operation_id: None }),
            None,
        );
        self.commit_annotated_system_item_then(
            session_store::LoggedSystemHistoryEntry { item, metadata },
            extensions,
            after_commit,
        )
    }

    fn commit_annotated_system_item_then(
        &self,
        entry: session_store::LoggedSystemHistoryEntry,
        extensions: Vec<SessionExtension>,
        after_commit: &mut dyn FnMut(&HistoryEntry<SessionHistoryMetadata>),
    ) -> Result<HistoryEntry<SessionHistoryMetadata>, StoreError> {
        entry.validate_input_metadata()?;
        let history = HistoryEntry::new(entry.item.to_history_item(), entry.metadata.clone());
        self.commit_usage_boundary(
            LogEntry::AnnotatedSystemItem {
                ts: segment_log::now_millis(),
                entry,
                extensions,
            },
            &mut || after_commit(&history),
            &[],
        )?;
        Ok(history)
    }
}

impl<St> SystemItemCommitter for LogWriterHandle<St>
where
    St: Store + Clone + Send + Sync + 'static,
{
    fn commit_log_entry(&self, entry: LogEntry) -> Result<(), StoreError> {
        self.append_entry(entry)
    }

    fn commit_usage_boundary(
        &self,
        entry: LogEntry,
        after_commit: &mut dyn FnMut(),
        post_metrics: &[session_metrics::Metric],
    ) -> Result<(), StoreError> {
        let _admission = self.state.mutation_authority.enter()?;
        let _append_guard = self
            .state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        self.append_entry_locked(entry)?;
        after_commit();
        for metric in post_metrics {
            let payload = serde_json::to_value(metric).expect("Metric is Serialize");
            let _ = self.append_entry_locked(LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: session_metrics::DOMAIN.into(),
                payload,
            });
        }
        Ok(())
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

#[derive(Clone)]
struct PreparedFlowProjection {
    selector: String,
    instructions: String,
    definition_id: String,
    definition_digest: String,
    instance_id: String,
    state_id: String,
}

/// Sole live owner of committed model-visible Worker history.
///
/// `Engine` borrows this history only while executing a run. Features capture
/// the committed log location and check it under the append lock before writing;
/// this live view does not maintain an independent freshness counter.
#[derive(Clone)]
pub struct WorkerSession {
    session_id: SessionId,
    history: History<SessionHistoryMetadata>,
    pending_activations: Arc<Mutex<PendingActivationState>>,
}

impl WorkerSession {
    fn new(session_id: SessionId, entries: Vec<HistoryEntry<SessionHistoryMetadata>>) -> Self {
        Self {
            session_id,
            history: History::from_entries(entries),
            pending_activations: Arc::new(Mutex::new(PendingActivationState::default())),
        }
    }

    fn restore_pending_activations(&mut self, extensions: &[(String, serde_json::Value)]) {
        let Some(payload) = extensions.iter().rev().find_map(|(domain, payload)| {
            (domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN).then_some(payload)
        }) else {
            return;
        };
        if let Ok(mut state) = serde_json::from_value::<PendingActivationState>(payload.clone()) {
            let mut ids = std::collections::BTreeSet::new();
            if state.invocation_receipts.len() > MAX_INVOCATION_RECEIPTS
                || state
                    .invocation_receipts
                    .iter()
                    .any(|receipt| !receipt.is_valid() || !ids.insert(&receipt.invocation_id))
            {
                self.pending_activations
                    .lock()
                    .expect("pending activation state poisoned")
                    .invocation_recovery_blocked = true;
                return;
            }
            if let Some(activating) = state.activating.take() {
                state.pending.push_front(activating);
            }
            if !state.activating_notifications.is_empty() {
                let mut restored = std::mem::take(&mut state.activating_notifications);
                restored.append(&mut state.pending_notifications);
                state.pending_notifications = restored;
            }
            *self
                .pending_activations
                .lock()
                .expect("pending activation state poisoned") = state;
        } else {
            // A malformed checkpoint must never turn a durable start into a
            // new executable invocation on restore.
            self.pending_activations
                .lock()
                .expect("pending activation state poisoned")
                .invocation_recovery_blocked = true;
        }
    }

    pub fn pending_submissions(&self) -> protocol::PendingSubmissionsSnapshot {
        self.pending_activations
            .lock()
            .expect("pending activation state poisoned")
            .snapshot()
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn history(&self) -> &History<SessionHistoryMetadata> {
        &self.history
    }

    fn history_mut(&mut self) -> &mut History<SessionHistoryMetadata> {
        &mut self.history
    }

    fn replace_history(&mut self, entries: Vec<HistoryEntry<SessionHistoryMetadata>>) {
        self.history.replace_entries(entries);
    }
}

pub(crate) type PromptContributionCompletion = Box<dyn FnOnce() + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SystemPromptContribution {
    Ready(String),
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentContextRefresh {
    pub body: String,
    pub behavior_md: String,
    pub prompt_provenance: Option<session_store::PromptRenderProvenance>,
}

/// Feature-owned System context at explicit creation and compaction boundaries.
/// Core persists and replays each pair unchanged; normal restore never reloads it.
#[async_trait::async_trait]
pub(crate) trait SessionContextSource: Send + Sync {
    fn protected_input_kind(&self) -> Option<&'static str> {
        None
    }

    async fn authorize_supply(&self) -> Result<(), String> {
        Ok(())
    }

    async fn authorize_retained_supply(&self, _kind: &str) -> Result<(), String> {
        self.authorize_supply().await
    }

    async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String>;

    async fn load_compaction_context(
        &self,
        _session: SessionId,
    ) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
        Ok(Vec::new())
    }
}

#[async_trait::async_trait]
pub(crate) trait SystemPromptContributionSource: Send + Sync {
    fn protected_input_kind(&self) -> Option<&'static str> {
        None
    }

    async fn load(&self) -> SystemPromptContribution;

    /// Prepare an initial contribution with an optional source-bound completion.
    /// Completion runs only after its exact prompt and active pointer commit,
    /// under the publication/release fence. It must perform bounded local updates
    /// only; no I/O, store reentry, or authority grants. Restore never replays it.
    async fn load_initial(
        &self,
    ) -> (
        SystemPromptContribution,
        Option<PromptContributionCompletion>,
    ) {
        (self.load().await, None)
    }

    /// Fail-closed initial loading for protected sources. Local-only sources may
    /// retain the infallible contribution contract.
    async fn load_initial_checked(
        &self,
    ) -> Result<
        (
            SystemPromptContribution,
            Option<PromptContributionCompletion>,
        ),
        String,
    > {
        Ok(self.load_initial().await)
    }

    /// Revalidate protected content before every model supply, including unchanged
    /// cached prompts and restored history. Local-only sources have no authority.
    async fn authorize_supply(&self) -> Result<(), String> {
        Ok(())
    }

    /// Returns a model-visible refresh only when user-managed resident context
    /// changed since its last committed representation.
    async fn load_changed_resident_context(
        &self,
    ) -> Result<Option<ResidentContextRefresh>, String> {
        Ok(None)
    }

    /// Reports a live durable system-input commit to its producer. Interpretation
    /// belongs to the Feature; restoration never invokes this notification.
    /// Runs under the publication barrier: only bounded local state updates are
    /// allowed, without Host I/O or reentry into history mutation.
    fn system_item_committed(&self, _item: &SystemItem, _metadata: &SessionHistoryMetadata) {}

    /// Invalidates the in-memory representation fence before history may be
    /// compacted or rewound.
    fn invalidate_resident_context_representation(&self) {}
}

// Retention facts, not grants: every supply still asks the bound source for
// current authority. Empty v1 provenance distinguishes local seeds from legacy
// seeds whose protected initial prompt can only be identified by saved metadata.
const PROTECTED_INPUTS_DOMAIN: &str = "worker.protected_inputs.v1";

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
enum ProtectedInputKind {
    WorkspaceMemory,
    SubjectResident,
    Journal,
    JournalStartup,
    JournalCompaction,
    SubjectConsolidationJob,
    SubjectJournalJob,
}

impl ProtectedInputKind {
    fn for_job(binding: &crate::BackendJobExecutionBinding) -> Option<Self> {
        if binding.subjektiv_consolidation {
            Some(Self::SubjectConsolidationJob)
        } else if binding.subjektiv_journal {
            Some(Self::SubjectJournalJob)
        } else {
            None
        }
    }

    fn job_operation(self) -> Option<&'static str> {
        match self {
            Self::SubjectConsolidationJob => Some("memory.consolidation.supply"),
            Self::SubjectJournalJob => Some("journal.job.input.supply"),
            _ => None,
        }
    }

    fn parse(kind: &str) -> Result<Self, String> {
        match kind {
            "workspace-memory" => Ok(Self::WorkspaceMemory),
            "subject-resident" => Ok(Self::SubjectResident),
            "journal" => Ok(Self::Journal),
            "journal-startup" => Ok(Self::JournalStartup),
            "journal-compaction" => Ok(Self::JournalCompaction),
            "subject-consolidation-job" => Ok(Self::SubjectConsolidationJob),
            "subject-journal-job" => Ok(Self::SubjectJournalJob),
            _ => Err("unknown protected input kind".into()),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::WorkspaceMemory => "workspace-memory",
            Self::SubjectResident => "subject-resident",
            Self::Journal => "journal",
            Self::JournalStartup => "journal-startup",
            Self::JournalCompaction => "journal-compaction",
            Self::SubjectConsolidationJob => "subject-consolidation-job",
            Self::SubjectJournalJob => "subject-journal-job",
        }
    }
}

// A projection of the persisted payload's retention facts, not an authorization
// response or a second Journal state machine. Required fields prevent malformed
// payloads from being mistaken for explicit empty, non-protected outcomes.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCompactionRetention {
    status: JournalRetentionStatus,
    snapshot: serde_json::Value,
    snapshot_sha256: serde_json::Value,
    #[serde(rename = "prompt_provenance")]
    _prompt_provenance: PromptRenderProvenance,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalRetentionStatus {
    Available,
    Absent,
    Disabled,
    Unavailable,
}

impl JournalCompactionRetention {
    fn is_unprotected_empty(&self) -> bool {
        !matches!(self.status, JournalRetentionStatus::Available)
            && self.snapshot.is_null()
            && self.snapshot_sha256.is_null()
    }
}

// Input identity is retained across rewrites, not execution authority. Attempt
// identity remains a live Host fence; a retry may bind a new attempt for this
// same immutable Job/input, never a different Job's current Subject permission.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedSubjectJobInput {
    kind: ProtectedInputKind,
    job_id: String,
    input_digest: Option<String>,
}
impl ProtectedSubjectJobInput {
    fn from_binding(binding: &crate::BackendJobExecutionBinding) -> Option<Self> {
        ProtectedInputKind::for_job(binding).map(|kind| Self {
            kind,
            job_id: binding.job_id.clone(),
            input_digest: binding.input_digest.clone(),
        })
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtectedInputProvenance {
    version: u8,
    kinds: std::collections::BTreeSet<ProtectedInputKind>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    subject_jobs: std::collections::BTreeSet<ProtectedSubjectJobInput>,
}

impl Default for ProtectedInputProvenance {
    fn default() -> Self {
        Self {
            version: 1,
            kinds: Default::default(),
            subject_jobs: Default::default(),
        }
    }
}

impl ProtectedInputProvenance {
    fn retain(&mut self, kind: Option<&str>) -> Result<(), String> {
        if let Some(kind) = kind {
            self.kinds.insert(ProtectedInputKind::parse(kind)?);
        }
        Ok(())
    }

    fn observe_history<'a>(
        &mut self,
        history: impl IntoIterator<Item = &'a SessionHistoryMetadata>,
    ) {
        for metadata in history {
            let journal_origin = matches!(
                &metadata.origin,
                WorkerHistoryProvenance::JournalContext
            ) || matches!(&metadata.origin, WorkerHistoryProvenance::Provided { producer } if producer == "subjektiv.journal");
            let payload = metadata
                .input_metadata
                .as_ref()
                .and_then(|input| input.feature_payload());
            let journal_payload = payload.is_some_and(|payload| {
                matches!(payload.schema(), "journal:startup" | "journal:compaction")
            });
            if journal_origin || journal_payload {
                match payload {
                    Some(payload)
                        if payload.schema() == "journal:startup" && payload.version() == 1 =>
                    {
                        self.kinds.insert(ProtectedInputKind::JournalStartup);
                    }
                    Some(payload)
                        if payload.schema() == "journal:compaction" && payload.version() == 1 =>
                    {
                        // Only explicit non-protected empty outcomes omit a marker.
                        // Unknown/malformed payloads retain legacy protection; a
                        // snapshot (even empty journals with a Subject ID) is input.
                        match payload.decode::<JournalCompactionRetention>("journal:compaction", 1)
                        {
                            Ok(facts) if facts.is_unprotected_empty() => {}
                            Ok(_) => {
                                self.kinds.insert(ProtectedInputKind::JournalCompaction);
                            }
                            Err(_) => {
                                self.kinds.insert(ProtectedInputKind::Journal);
                            }
                        }
                    }
                    _ => {
                        self.kinds.insert(ProtectedInputKind::Journal);
                    }
                }
            } else if matches!(&metadata.origin,
                WorkerHistoryProvenance::Provided { producer } if producer == "subjektiv.resident-context"
            ) {
                self.kinds.insert(ProtectedInputKind::SubjectResident);
            }
        }
    }

    fn retain_context_items(
        &mut self,
        source_kind: Option<&str>,
        items: &[session_store::LoggedSystemHistoryEntry],
    ) -> Result<(), String> {
        if !items.is_empty() && source_kind != Some("journal") {
            self.retain(source_kind)?;
        }
        self.observe_history(items.iter().map(|entry| &entry.metadata));
        Ok(())
    }

    fn extension(&self) -> LogEntry {
        LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: PROTECTED_INPUTS_DOMAIN.into(),
            payload: serde_json::to_value(self)
                .expect("protected input provenance is serializable"),
        }
    }

    fn restore(
        entries: &[LogEntry],
        history: &[HistoryEntry<SessionHistoryMetadata>],
        metadata: Option<&WorkerMetadata>,
    ) -> Result<Self, WorkerError> {
        let mut provenance = Self::default();
        let mut found = false;
        let mut read = |domain: &str, payload: &serde_json::Value| -> Result<(), WorkerError> {
            if domain != PROTECTED_INPUTS_DOMAIN {
                return Ok(());
            }
            let saved: Self = serde_json::from_value(payload.clone()).map_err(|error| {
                WorkerError::InvalidState(format!("invalid protected input provenance: {error}"))
            })?;
            if saved.version != 1 {
                return Err(WorkerError::InvalidState(
                    "unsupported protected input provenance version".into(),
                ));
            }
            if saved.kinds.iter().any(|kind| {
                kind.job_operation().is_some()
                    && !saved.subject_jobs.iter().any(|job| job.kind == *kind)
            }) || saved.subject_jobs.iter().any(|job| {
                job.kind.job_operation().is_none()
                    || !saved.kinds.contains(&job.kind)
                    || job.job_id.trim().is_empty()
                    || job
                        .input_digest
                        .as_ref()
                        .is_some_and(|digest| digest.trim().is_empty())
            }) {
                return Err(WorkerError::InvalidState(
                    "invalid protected Job input provenance".into(),
                ));
            }
            found = true;
            provenance.kinds.extend(saved.kinds);
            provenance.subject_jobs.extend(saved.subject_jobs);
            Ok(())
        };
        for entry in entries {
            match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } => read(domain, payload)?,
                LogEntry::AnnotatedSystemItem { extensions, .. }
                | LogEntry::AnnotatedUserInput { extensions, .. } => {
                    for extension in extensions {
                        read(&extension.domain, &extension.payload)?;
                    }
                }
                _ => {}
            }
        }
        // The old manifest is evidence of possible retained initial input, never
        // a permission. Do not use today's replacement config or old grants.
        if !found
            && let Some(snapshot) = metadata.and_then(|m| m.resolved_manifest_snapshot.clone())
        {
            let saved =
                manifest::read_persisted_worker_manifest_snapshot(snapshot).map_err(|error| {
                    WorkerError::InvalidState(format!(
                        "read protected input legacy manifest: {error}"
                    ))
                })?;
            if saved.feature.subjektiv.execution_enabled()
                && !saved.feature.subjektiv.profile.consolidation_tools
            {
                provenance.kinds.insert(ProtectedInputKind::SubjectResident);
            } else if saved.feature.memory.enabled()
                && saved.feature.memory.profile.resident.inject_summary
            {
                provenance.kinds.insert(ProtectedInputKind::WorkspaceMemory);
            }
        }
        provenance.observe_history(history.iter().map(|entry| &entry.annotation));
        Ok(provenance)
    }
}

#[derive(Clone, Default)]
pub(crate) struct ProtectedInputSupply {
    retained: Arc<Mutex<ProtectedInputProvenance>>,
    prompt_source: Option<Arc<dyn SystemPromptContributionSource>>,
    context_source: Option<Arc<dyn SessionContextSource>>,
    // Reconstructed only from the immutable Host execution binding, never from
    // saved provenance. Every use asks the Host for current attempt/origin/policy.
    subject_job: Option<(ProtectedSubjectJobInput, Arc<dyn WorkspaceClient>)>,
}

impl ProtectedInputSupply {
    pub(crate) fn set_prompt_source(
        &mut self,
        source: Option<Arc<dyn SystemPromptContributionSource>>,
    ) {
        self.prompt_source = source;
    }

    pub(crate) async fn authorize<'a>(
        &self,
        history: impl IntoIterator<Item = &'a SessionHistoryMetadata>,
    ) -> Result<(), String> {
        let provenance = {
            let mut retained = self
                .retained
                .lock()
                .expect("protected input provenance poisoned");
            retained.observe_history(history);
            retained.clone()
        };
        self.authorize_retained(provenance).await
    }

    pub(crate) async fn authorize_pending<'a>(
        &self,
        history: impl IntoIterator<Item = &'a SessionHistoryMetadata>,
    ) -> Result<(), String> {
        // Rejected hook output has not been retained. Check it with the saved
        // facts, without turning a failed preparation into durable provenance.
        let mut pending = self
            .retained
            .lock()
            .expect("protected input provenance poisoned")
            .clone();
        pending.observe_history(history);
        self.authorize_retained(pending).await
    }

    async fn authorize_retained(&self, provenance: ProtectedInputProvenance) -> Result<(), String> {
        for input in &provenance.subject_jobs {
            if self.subject_job.as_ref().map(|(bound, _)| bound) != Some(input) {
                return Err(
                    "retained protected Job input requires its matching Host Job/input binding"
                        .into(),
                );
            }
        }
        for kind in provenance.kinds {
            if let Some(operation) = kind.job_operation() {
                let (bound_kind, client) = self.subject_job.as_ref().ok_or_else(|| {
                    format!(
                        "retained protected input {} requires its Host Job binding",
                        kind.as_str()
                    )
                })?;
                if bound_kind.kind != kind || !provenance.subject_jobs.contains(bound_kind) {
                    return Err(
                        "retained protected Job input does not match its Host Job binding".into(),
                    );
                }
                client
                    .authorize_subject_operation(operation)
                    .map_err(|error| {
                        format!("protected Job input supply rejected for {operation}: {error}")
                    })?;
                continue;
            }
            let journal = matches!(
                kind,
                ProtectedInputKind::Journal
                    | ProtectedInputKind::JournalStartup
                    | ProtectedInputKind::JournalCompaction
            );
            let actual = if journal {
                self.context_source
                    .as_ref()
                    .and_then(|s| s.protected_input_kind())
            } else {
                self.prompt_source
                    .as_ref()
                    .and_then(|s| s.protected_input_kind())
            };
            let expected = if journal { "journal" } else { kind.as_str() };
            if actual != Some(expected) {
                return Err(format!(
                    "retained protected input {} requires its matching source",
                    kind.as_str()
                ));
            }
            if journal {
                self.context_source
                    .as_ref()
                    .expect("matching source checked")
                    .authorize_retained_supply(kind.as_str())
                    .await?;
            }
        }
        if let Some(source) = &self.prompt_source {
            source.authorize_supply().await?;
        }
        if let Some(source) = &self.context_source {
            source.authorize_supply().await?;
        }
        Ok(())
    }
}

#[derive(Clone)]
struct ProtectedCompactorClient {
    inner: Box<dyn LlmClient>,
    supply: ProtectedInputSupply,
}

#[async_trait::async_trait]
impl LlmClient for ProtectedCompactorClient {
    async fn stream(
        &self,
        request: agen::llm_client::Request,
    ) -> Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError> {
        self.supply
            .authorize(std::iter::empty())
            .await
            .map_err(agen::llm_client::ClientError::Config)?;
        self.inner.stream(request).await
    }
    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
    fn tool_call_completion_support(&self) -> agen::llm_client::ToolCallCompletionSupport {
        self.inner.tool_call_completion_support()
    }
    fn validate_config(&self, config: &RequestConfig) -> Vec<agen::llm_client::ConfigWarning> {
        self.inner.validate_config(config)
    }
}

#[derive(Debug, Clone)]
struct PendingCompactionCleanup {
    session_id: String,
    compaction_id: String,
}

/// Retains startup task scopes after a consuming controller startup fails.
///
/// Call `shutdown` only after startup has failed with no live Controller. Keep
/// this handle on errors or timeout and retry; it owns any in-flight settlement
/// future, including join handles drained from the registries. On successful
/// controller transfer, drop the factory's handle without shutting it down.
#[derive(Clone)]
pub struct WorkerStartupFeatureCleanup {
    inner: Arc<StartupFeatureCleanupInner>,
}

struct StartupFeatureCleanupInner {
    mcp_discovery: crate::feature::mcp::McpDiscoveryCleanup,
    registries: Mutex<Vec<FeatureBackgroundTaskRegistry>>,
    settlement: tokio::sync::Mutex<
        Option<futures_util::future::BoxFuture<'static, Result<(), crate::hook::HookError>>>,
    >,
}

impl WorkerStartupFeatureCleanup {
    fn new(registry: FeatureBackgroundTaskRegistry) -> Self {
        Self {
            inner: Arc::new(StartupFeatureCleanupInner {
                mcp_discovery: crate::feature::mcp::McpDiscoveryCleanup::new(),
                registries: Mutex::new(vec![registry]),
                settlement: tokio::sync::Mutex::new(None),
            }),
        }
    }

    pub(crate) fn mcp_discovery(&self) -> crate::feature::mcp::McpDiscoveryCleanup {
        self.inner.mcp_discovery.clone()
    }

    fn retain(&self, registry: FeatureBackgroundTaskRegistry) {
        self.inner
            .registries
            .lock()
            .expect("startup Feature cleanup poisoned")
            .push(registry);
    }

    /// Stops and joins every captured registry. Cancelling this call preserves
    /// its settlement future for the next call, rather than dropping task joins.
    pub async fn shutdown(&self) -> Result<(), crate::hook::HookError> {
        let mut settlement = self.inner.settlement.lock().await;
        let pending = settlement.get_or_insert_with(|| {
            let registries = self
                .inner
                .registries
                .lock()
                .expect("startup Feature cleanup poisoned")
                .clone();
            let mcp_discovery = self.inner.mcp_discovery.clone();
            Box::pin(async move {
                let mut failure = None;
                for registry in registries {
                    if let Err(error) = registry.shutdown().await {
                        failure.get_or_insert(error);
                    }
                }
                if let Err(error) = mcp_discovery.shutdown().await {
                    failure.get_or_insert_with(|| {
                        crate::hook::HookError::new(
                            crate::hook::HookErrorCategory::Dependency,
                            error.to_string(),
                        )
                    });
                }
                failure.map_or(Ok(()), Err)
            })
        });
        let result = pending.as_mut().await;
        *settlement = None;
        result
    }
}

/// A close-only candidate resource owner, not filesystem execution authority.
#[derive(Clone)]
pub struct RestoreCandidateWorkdirCleanup {
    sessions: Arc<WorkdirSessionRouter>,
}

impl RestoreCandidateWorkdirCleanup {
    pub async fn close(&self) -> Result<(), WorkerError> {
        self.sessions
            .close_all()
            .await
            .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))
    }
}

/// An unpublished, read-only reconstruction of an existing materialized Worker.
///
/// The host must hold its per-Worker operation lock through activation. Snapshot
/// checks reject drift; they do not replace that lock or current host authorization.
/// No controller, allocation, Feature installation, or repair runs during prepare.
/// The public candidate exposes no Worker execution or mutation handles. Binding
/// inside this crate is limited to the open candidate; once discard begins, only
/// cleanup retry remains valid, even if its previous attempt failed or was cancelled.
/// Use `discard` for confirmed cleanup; Drop only schedules best-effort cleanup on
/// the current Tokio runtime and cannot attest that asynchronous cleanup completed.
pub struct PreparedRestoredWorker<St: Store> {
    worker: Option<Worker<Box<dyn LlmClient>, St>>,
    closing: bool,
    activation: Option<RestoredWorkerActivation>,
}

/// Exact existing restore source retained while a Controller is initialized.
///
/// This carries no new Worker identity or execution authority. The host must keep
/// the per-Worker operation lock and invoke activation only after durable commit.
/// Controller initialization must not change the captured source before then.
pub(crate) struct RestoredWorkerActivation {
    metadata: Option<WorkerMetadata>,
    entries: serde_json::Value,
    manifest: serde_json::Value,
}

impl RestoredWorkerActivation {
    fn revalidate<St>(&self, worker: &Worker<Box<dyn LlmClient>, St>) -> Result<(), WorkerError>
    where
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        let changed = |component| WorkerError::RestoreCandidateChanged {
            worker_name: worker.manifest.worker.name.clone(),
            component,
        };
        if worker.store.read_by_name(&worker.manifest.worker.name)? != self.metadata {
            return Err(changed("metadata"));
        }
        if restore_snapshot_value(&worker.manifest)? != self.manifest {
            return Err(changed("manifest"));
        }
        let entries = worker
            .store
            .read_all(worker.session_id(), worker.segment_id())?;
        if restore_snapshot_value(&entries)? != self.entries {
            return Err(changed("segment"));
        }
        Ok(())
    }

    /// Acquire the existing top-level writer allocation after commit, checking
    /// exact source both before and after acquisition, then apply restore effects.
    /// On failure the Controller still owns the Worker; cleanup handles do not
    /// establish rollback and allocation release must wait until it is dropped.
    pub(crate) async fn activate<St>(
        &self,
        worker: &mut Worker<Box<dyn LlmClient>, St>,
    ) -> Result<(), WorkerError>
    where
        St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
    {
        self.revalidate(worker)
            .map_err(|error| worker.construction_failure(error))?;
        let allocate = || -> Result<ScopeAllocationGuard, WorkerError> {
            Ok(worker_allocation::install_top_level_with_deny(
                worker.manifest.worker.name.clone(),
                std::process::id(),
                PathBuf::new(), // Reserved legacy field; no Worker endpoint exists.
                worker.scope.snapshot().allow_rules(),
                worker.scope.snapshot().deny_rules(),
                worker.segment_id(),
            )?)
        };
        worker.scope_allocation =
            Some(allocate().map_err(|error| worker.construction_failure(error))?);
        self.revalidate(worker)
            .map_err(|error| worker.construction_failure(error))?;
        worker
            .finish_restored_construction_in_place(false)
            .await
            .map_err(|source| WorkerError::RestoreActivationEffects {
                source: Box::new(source),
            })
    }
}

impl<St> PreparedRestoredWorker<St>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    pub fn manifest(&self) -> &WorkerManifest {
        &self.worker().manifest
    }

    /// Stable Session identity used to correlate restored Plugin Tool calls.
    pub fn session_id(&self) -> SessionId {
        self.worker().session_id()
    }

    /// Bind exact Host-prepared contributions without exposing execution or
    /// reopening a candidate whose cleanup has begun.
    pub fn bind_plugin_tools(
        &mut self,
        tools: crate::plugin::PluginToolsFeature,
    ) -> Result<(), WorkerError> {
        self.worker_mut()?.bind_plugin_tools(tools);
        Ok(())
    }

    /// Retain the same Workdir resource owner for explicit cleanup, including
    /// after discard fails. This does not grant Worker input or execution access.
    pub fn workdir_cleanup(&self) -> RestoreCandidateWorkdirCleanup {
        RestoreCandidateWorkdirCleanup {
            sessions: self.worker().workdir_sessions.clone(),
        }
    }

    #[cfg(test)]
    fn workdir_sessions(&self) -> Arc<WorkdirSessionRouter> {
        self.worker().workdir_sessions.clone()
    }

    fn worker(&self) -> &Worker<Box<dyn LlmClient>, St> {
        self.worker
            .as_ref()
            .expect("restore candidate already discarded")
    }

    /// Bootstrap may bind host services before returning an open candidate.
    /// Never reopen a candidate whose cleanup has begun.
    pub(crate) fn worker_mut(
        &mut self,
    ) -> Result<&mut Worker<Box<dyn LlmClient>, St>, WorkerError> {
        if self.closing {
            return Err(WorkerError::RestoreCandidateClosing);
        }
        self.worker
            .as_mut()
            .ok_or(WorkerError::RestoreCandidateClosing)
    }

    fn revalidate(&self, worker: &Worker<Box<dyn LlmClient>, St>) -> Result<(), WorkerError> {
        self.activation
            .as_ref()
            .expect("restore candidate activation already transferred")
            .revalidate(worker)
    }

    /// Transfer the read-only reconstruction for Controller initialization, not
    /// execution. Validate once without allocation or canonical restore effects;
    /// the Controller must retain activation until its durable commit succeeds.
    pub(crate) fn into_controller_parts(
        mut self,
    ) -> Result<(Worker<Box<dyn LlmClient>, St>, RestoredWorkerActivation), WorkerError> {
        let worker = self
            .worker
            .take()
            .ok_or(WorkerError::RestoreCandidateClosing)?;
        if self.closing {
            return Err(worker.construction_failure(WorkerError::RestoreCandidateClosing));
        }
        self.revalidate(&worker)
            .map_err(|error| worker.construction_failure(error))?;
        let activation = self
            .activation
            .take()
            .expect("restore candidate activation already transferred");
        Ok((worker, activation))
    }

    /// Acquire writer allocation only after exact source validation, validate
    /// again under that allocation, then apply the existing restore mutations.
    /// An error may retain partial durable effects; ConstructionCleanup carries
    /// resource ownership and is not evidence of rollback.
    pub async fn activate(self) -> Result<Worker<Box<dyn LlmClient>, St>, WorkerError> {
        self.activate_with_allocation(|worker| {
            Ok(worker_allocation::install_top_level_with_deny(
                worker.manifest.worker.name.clone(),
                std::process::id(),
                PathBuf::new(), // Reserved legacy field; no Worker endpoint exists.
                worker.scope.snapshot().allow_rules(),
                worker.scope.snapshot().deny_rules(),
                worker.segment_id(),
            )?)
        })
        .await
    }

    async fn activate_with_allocation(
        mut self,
        allocate: impl FnOnce(
            &Worker<Box<dyn LlmClient>, St>,
        ) -> Result<ScopeAllocationGuard, WorkerError>,
    ) -> Result<Worker<Box<dyn LlmClient>, St>, WorkerError> {
        let mut worker = self
            .worker
            .take()
            .ok_or(WorkerError::RestoreCandidateClosing)?;
        if self.closing {
            // Preserve the same cleanup owner without validating or acquiring
            // allocation for a partially closed candidate.
            return Err(worker.construction_failure(WorkerError::RestoreCandidateClosing));
        }
        self.revalidate(&worker)
            .map_err(|error| worker.construction_failure(error))?;
        worker.scope_allocation =
            Some(allocate(&worker).map_err(|error| worker.construction_failure(error))?);
        self.revalidate(&worker)
            .map_err(|error| worker.construction_failure(error))?;
        worker
            .finish_restored_construction()
            .await
            .map_err(|source| WorkerError::RestoreActivationEffects {
                source: Box::new(source),
            })
    }

    /// Directly settle candidate resources, without Controller Shutdown. Failed
    /// cleanup retains the candidate so the same owner can retry it.
    pub async fn discard(&mut self) -> Result<(), WorkerError> {
        self.closing = true;
        if let Some(worker) = self.worker.as_mut() {
            worker
                .retain_startup_feature_cleanup()
                .shutdown()
                .await
                .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
            worker
                .workdir_sessions
                .close_all()
                .await
                .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
            // An unpublished candidate has no allocation guard. Activation moves
            // the Worker out and uses ConstructionCleanup on failure; never try
            // releasing a cleanup handle while its guard is still attached.
            debug_assert!(worker.scope_allocation.is_none());
        }
        self.worker = None;
        Ok(())
    }
}

impl<St: Store> Drop for PreparedRestoredWorker<St> {
    fn drop(&mut self) {
        let Some(worker) = self.worker.as_mut() else {
            return;
        };
        let sessions = worker.workdir_sessions.clone();
        let features = worker.retain_startup_feature_cleanup();
        // Candidates do not acquire allocation until consuming activation.
        debug_assert!(worker.scope_allocation.is_none());
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    if let Err(error) = features.shutdown().await {
                        tracing::warn!(%error, "restore candidate Feature cleanup unconfirmed");
                        return;
                    }
                    if let Err(error) = sessions.close_all().await {
                        tracing::warn!(%error, "restore candidate Workdir cleanup unconfirmed");
                    }
                });
            }
            Err(error) => {
                tracing::warn!(%error, "restore candidate dropped without async cleanup runtime")
            }
        }
    }
}

fn restore_snapshot_value(value: &impl serde::Serialize) -> Result<serde_json::Value, WorkerError> {
    serde_json::to_value(value)
        .map_err(|error| WorkerError::InvalidState(format!("encode restore snapshot: {error}")))
}

/// An independent agent execution unit.
///
/// Holds a [`Engine`] directly and persists session state via
/// `session-store` functions after each turn.
pub struct Worker<C: LlmClient, St: Store> {
    manifest: WorkerManifest,
    /// Host-bound once before Feature installation; never inherited by child Workers.
    backend_job: std::sync::OnceLock<Option<crate::BackendJobExecutionBinding>>,
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
    persist_restored_manifest: bool,
    worker_metadata_segment_cas: Option<WorkerMetadataSegmentCas>,
    /// Shared session pointer. Source of truth for the Worker's current
    /// `segment_id` and append tally. `self.segment_id()` is a thin
    /// wrapper over `segment_state.segment_id()`.
    segment_state: Arc<SegmentState>,
    /// Explicit local filesystem authority, or `None` for Workers with no
    /// local cwd and no filesystem/Bash tool surface.
    filesystem_authority: WorkerFilesystemAuthority,
    /// Alias-keyed live Workdir sessions. Provider transport remains hidden
    /// behind each handle and no attachment is designated primary.
    workdir_sessions: Arc<WorkdirSessionRouter>,
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
    hook_builder: HookRegistryBuilder,
    /// Frozen callback set shared by Engine interception and Worker lifecycle boundaries.
    hook_registry: Option<Arc<HookRegistry>>,
    /// Executable background tasks registered by successfully installed features.
    feature_background_tasks: FeatureBackgroundTaskRegistry,
    /// Present only while an opted-in factory is capturing startup task scopes.
    startup_feature_cleanup: Option<WorkerStartupFeatureCleanup>,
    /// Enabled chat invocation metadata and executable handlers.
    feature_invocations: FeatureInvocationRegistry,
    /// Internal Workers install an explicit Feature composition and disable
    /// manifest-derived lifecycle Features before controller startup.
    manifest_lifecycle_features_enabled: bool,
    interceptor_installed: bool,
    compaction_service_budget: Option<Arc<crate::compact::worker::CompactWorkerInterceptor>>,
    /// Shared compaction state (present when threshold is configured).
    compact_state: Option<Arc<CompactState>>,
    /// Per-LLM-request Usage tracker. Always present after construction.
    /// Captures `(history_len, UsageEvent)` pairs until their authoritative
    /// request-boundary or terminal `LlmUsage` append succeeds.
    usage_tracker: Arc<UsageTracker>,
    /// Sync-side buffer for `Metric` values queued from inside Engine
    /// callbacks (currently the prune observer). Drained at the same request
    /// boundary as usage, with terminal `persist_turn` as the fallback.
    metrics_tracker: Arc<crate::compact::metrics_tracker::MetricsTracker>,
    /// Cumulative Usage measurement timeline, one entry per LLM call.
    /// Restored from session log on `restore`, appended after each durable
    /// request-boundary usage commit.
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
    plugin_tools: Option<crate::plugin::PluginToolsFeature>,
    /// Parsed system-prompt template awaiting first-turn materialisation.
    /// Retained until the complete initial seed and active metadata are durable.
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
    working_event_tx: Option<broadcast::Sender<Event>>,
    /// Parent-owned projection/control boundary for observable Internal service Workers.
    /// Service Workers are never exposed through the model-facing SubWorker control surface.
    internal_worker_registry: Option<Arc<crate::spawn::registry::SpawnedWorkerRegistry>>,
    /// Cleanup authority survives cancellation of an individual compaction future.
    pending_compaction_cleanup: Mutex<Option<PendingCompactionCleanup>>,
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
    /// Maximum time an idle Worker waits from the first accepted notification
    /// so a burst can be delivered as one FIFO batch.
    notification_coalesce_delay: Duration,
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
    /// Machine-wide lock retained after a post-CAS allocation publication
    /// failure. Declared before `scope_allocation` so drop releases this nested
    /// authority before the outer allocation guard reacquires the same lock.
    failed_activation_authority: Mutex<Option<SegmentActivationGuard>>,
    /// Scope allocation in the machine-wide lock file. `Some` for
    /// Workers built via `from_manifest` /
    /// `restore_from_manifest` (production paths); `None` for the
    /// low-level `Worker::new` constructor used in tests, which bypasses
    /// the registry. Cleanup handles can retain the allocation beyond Worker
    /// drop; only the final shared owner attempts best-effort release.
    scope_allocation: Option<ScopeAllocationGuard>,
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
    /// Test/internal policy gate for installed resident prompt contributions.
    inject_resident_summary: bool,
    /// Deferred resident prompt source installed by an enabled Feature.
    feature_resident_summary_source: Option<Arc<dyn SystemPromptContributionSource>>,
    session_context_source: Option<Arc<dyn SessionContextSource>>,
    protected_inputs: Arc<Mutex<ProtectedInputProvenance>>,
    /// Exact complete seed retained across ambiguous Store responses. The committed
    /// segment remains authoritative; metadata must be repaired before accepting input.
    prepared_initial_seed: Option<Vec<LogEntry>>,
    prepared_initial_prompt_completion: Option<(String, PromptContributionCompletion)>,
    subjektiv_host: Option<Arc<dyn crate::subjektiv::SubjektivHost>>,
    feature_installation: crate::feature::FeatureInstallation,
    /// One-shot generic Feature lifecycle notification required after restore.
    restore_feature_lifecycle_pending: bool,
    /// Complete system prompt replacement installed by an enabled Feature.
    feature_system_prompt_override: Option<String>,
    /// Typed user submissions in submit order. K-th entry corresponds to
    /// the K-th `Item::user_message` in `worker.history()` (modulo seed
    /// history loaded via `AnnotatedSegmentStart.history`, whose original segments
    /// are not preserved). Populated from log on `restore_from_manifest`,
    /// appended after `save_user_input` on each `run`. Pre-`Event::Snapshot`
    /// this fed `WorkerSharedState.user_segments`; the new wire format
    /// carries typed atoms via `LogEntry::AnnotatedUserInput { segments }` so
    /// this remains purely an in-memory tracker for compact alignment.
    user_segments: Vec<Vec<Segment>>,
    /// Worker-side session-log mirror + broadcast sink. Populated alongside
    /// every successful `session_store::append_entry` write so connected
    /// clients see a `(snapshot, live)` stream consistent with what's
    /// on disk.
    sink: SegmentLogSink,
    /// `true` once `wire_history_persistence` has installed the
    /// `Engine::on_history_append` callback that commits each appended
    /// item as a singular `LogEntry::AnnotatedAssistantItem` /
    /// `AnnotatedToolResult`
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
    notification_committer: Option<Arc<dyn DurableNotificationCommitter>>,
}

impl<C: LlmClient, St: Store> Drop for Worker<C, St> {
    fn drop(&mut self) {
        // A failed activation authority nests the same machine-wide lock used by
        // allocation owner's final Drop. Release the nested guard first so Worker
        // teardown cannot self-deadlock while removing its outer allocation.
        let authority = match self.failed_activation_authority.get_mut() {
            Ok(authority) => authority.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        };
        drop(authority);
    }
}

impl<C: LlmClient, St: Store> Worker<C, St> {
    /// Opt in before a consuming startup call. The handle captures the current
    /// task registry and each subsequently installed registry until successful
    /// controller transfer ends capture. Repeated calls share the same capture.
    pub fn retain_startup_feature_cleanup(&mut self) -> WorkerStartupFeatureCleanup {
        self.startup_feature_cleanup
            .get_or_insert_with(|| {
                WorkerStartupFeatureCleanup::new(self.feature_background_tasks.clone())
            })
            .clone()
    }

    /// Observes explicit factory cleanup ownership without enabling capture.
    pub fn startup_feature_cleanup_handle(&self) -> Option<WorkerStartupFeatureCleanup> {
        self.startup_feature_cleanup.clone()
    }

    /// End factory capture immediately before transferring to a live Controller.
    /// The factory must also drop its handle after startup returns success.
    pub fn finish_startup_feature_cleanup_capture(&mut self) {
        self.startup_feature_cleanup = None;
    }

    /// Retain cleanup authority before passing the reconstructed Worker into a
    /// consuming factory. Explicit release is only valid after factory failure
    /// with no live Controller; drop the handle on successful controller transfer.
    pub fn restore_allocation_cleanup_handle(&self) -> Option<ScopeAllocationCleanup> {
        self.scope_allocation
            .as_ref()
            .map(ScopeAllocationGuard::cleanup_handle)
    }

    fn construction_failure(&self, error: WorkerError) -> WorkerError {
        WorkerError::ConstructionCleanup {
            source: Box::new(error),
            allocation: self.restore_allocation_cleanup_handle(),
            sessions: self.workdir_sessions.clone(),
        }
    }
}

impl<C: LlmClient + 'static, St: Store + 'static> Worker<C, St> {
    pub async fn stop_feature_runtime(&mut self, reason: impl Into<String>) {
        if let Some(hooks) = self.hook_registry.clone() {
            let context = WorkerStoppingContext {
                invocation: self.hook_invocation_context(None),
                reason: reason.into(),
            };
            match tokio::time::timeout(
                FEATURE_HOOK_CHAIN_TIMEOUT,
                hooks.on_worker_stopping(&context),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "worker-stopping hook requires attention");
                }
                Err(_) => {
                    hooks.record_chain_timeout("worker-stopping");
                    tracing::warn!("worker-stopping hook chain exceeded its host deadline");
                }
            }
        }
        if let Err(error) = self.feature_background_tasks.shutdown().await {
            tracing::warn!(error = %error, "feature background task shutdown failed");
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

    pub(crate) fn mutation_authority(&self) -> Arc<crate::subject_release::MutationAuthority> {
        self.segment_state.mutation_authority.clone()
    }

    pub(crate) fn pending_activation_state(&self) -> Arc<Mutex<PendingActivationState>> {
        self.session.pending_activations.clone()
    }

    pub(crate) fn pending_submission_handle(&self) -> PendingSubmissionHandle<St> {
        PendingSubmissionHandle {
            state: self.session.pending_activations.clone(),
            writer: self.log_writer_handle(),
        }
    }

    pub fn pending_submissions(&self) -> protocol::PendingSubmissionsSnapshot {
        self.session.pending_submissions()
    }

    /// Attach a type-erased system-item commit handle. The controller
    /// calls this once during spawn so the interceptor can commit
    /// `SystemItem`s directly without owning a generic store handle.
    /// Idempotent: subsequent calls overwrite the previous handle.
    pub fn attach_log_writer(&mut self, writer: Arc<dyn SystemItemCommitter>) {
        self.log_writer = Some(writer);
        self.notification_committer = Some(Arc::new(self.pending_submission_handle()));
    }

    pub fn attach_in_flight_events(&mut self, in_flight: InFlightEvents) {
        self.in_flight = Some(in_flight);
        if self.notification_committer.is_some() {
            self.notification_committer = Some(Arc::new(self.pending_submission_handle()));
        }
    }

    pub fn clear_in_flight_events(&self) {
        if let Some(in_flight) = &self.in_flight {
            in_flight.clear();
        }
    }

    /// Wire `Engine::on_history_append` to commit each appended item
    /// directly as a singular `LogEntry::AnnotatedAssistantItem` /
    /// `AnnotatedToolResult`
    /// through the writer. The controller calls this once per spawned
    /// Worker after the worker is built; tests that drive `Worker::new` may
    /// opt in to the same wiring or leave it off (in which case
    /// `persist_turn`'s inline fallback writes entries at turn end).
    ///
    /// `user_message` items are skipped because they are committed
    /// up-front via `commit_entry(LogEntry::AnnotatedUserInput { segments })`.
    /// `role:system` items are committed as typed `LogEntry::AnnotatedSystemItem`
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
    /// should parse it themselves and call [`Self::set_system_prompt_template`].
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
        DelegationScope::from_config(&manifest.delegation_scope).map_err(WorkerError::Scope)?;
        let scope = SharedScope::new(scope);
        let workdir_sessions = workdir_sessions_from_authority(&filesystem_authority, &scope)?;
        let mut worker = Self {
            backend_job: std::sync::OnceLock::new(),
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer: None,
            persist_restored_manifest: false,
            worker_metadata_segment_cas: None,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority,
            workdir_sessions,
            workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            hook_registry: None,
            feature_background_tasks: FeatureBackgroundTaskRegistry::default(),
            startup_feature_cleanup: None,
            feature_invocations: FeatureInvocationRegistry::default(),
            manifest_lifecycle_features_enabled: true,
            interceptor_installed: false,
            compaction_service_budget: None,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::<UsageRecord>::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            plugin_tools: None,
            system_prompt_template: None,
            feature_instructions: Vec::new(),
            alerter: None,
            working_event_tx: None,
            internal_worker_registry: None,
            pending_compaction_cleanup: Mutex::new(None),
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            notification_coalesce_delay: DEFAULT_NOTIFICATION_COALESCE_DELAY,
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            failed_activation_authority: Mutex::new(None),
            scope_allocation: None,
            runtime_ticket_role: None,
            prompts,
            inject_resident_summary: true,
            feature_resident_summary_source: None,
            session_context_source: None,
            protected_inputs: Arc::default(),
            prepared_initial_seed: None,
            prepared_initial_prompt_completion: None,
            subjektiv_host: None,
            feature_installation: crate::feature::FeatureInstallation::Activate,
            restore_feature_lifecycle_pending: false,
            feature_system_prompt_override: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
            notification_committer: None,
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

    /// Toggle all resident sections in the system prompt.
    ///
    /// Default `true`: normal Workers may expose each resident section according
    /// to its own gate and manifest settings. Internal disposable workers set
    /// suppressed while explicit tools remain available.
    pub fn set_resident_memory_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
    }

    /// Internal/test gate for installed resident prompt contributions.
    pub fn set_resident_summary_injection(&mut self, enabled: bool) {
        self.inject_resident_summary = enabled;
    }

    fn protected_input_supply(&self) -> ProtectedInputSupply {
        ProtectedInputSupply {
            retained: self.protected_inputs.clone(),
            prompt_source: self.feature_resident_summary_source.clone(),
            context_source: self.session_context_source.clone(),
            subject_job: self
                .backend_job()
                .and_then(ProtectedSubjectJobInput::from_binding)
                .map(|input| (input, self.workspace_client_handle())),
        }
    }

    fn protected_input_checkpoint(&self) -> LogEntry {
        let mut retained = self
            .protected_inputs
            .lock()
            .expect("protected input provenance poisoned");
        retained.observe_history(
            self.session
                .history()
                .entries()
                .iter()
                .map(|entry| &entry.annotation),
        );
        retained.extension()
    }

    pub(crate) fn install_session_context_source(&mut self, source: Arc<dyn SessionContextSource>) {
        self.session_context_source = Some(source);
    }

    pub(crate) fn install_system_prompt_contribution(
        &mut self,
        resident_summary_source: Option<Arc<dyn SystemPromptContributionSource>>,
        system_prompt_override: Option<String>,
    ) {
        self.prepared_initial_prompt_completion = None;
        self.feature_resident_summary_source = resident_summary_source;
        self.feature_system_prompt_override = system_prompt_override;
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
            .current_prompt_projection()
            .map_err(|source| WorkerError::WorkspacePromptProjection {
                message: source.to_string(),
            })?
        else {
            return Ok(());
        };
        let projection = &resolution.projection;
        let current = self.prompts.load();
        if current.projection().source_digest == projection.source_digest
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

    /// Immutable host-owned Job authority, absent for ordinary and child Workers.
    pub fn backend_job(&self) -> Option<&crate::BackendJobExecutionBinding> {
        self.backend_job.get().and_then(Option::as_ref)
    }

    /// Trusted host seam; immutable after binding and unavailable after Feature installation.
    pub fn bind_backend_job(
        &mut self,
        binding: crate::BackendJobExecutionBinding,
    ) -> Result<(), &'static str> {
        binding.validate()?;
        if self.interceptor_installed || self.backend_job.get().is_some() {
            return Err("Backend Job capability is already sealed");
        }
        if !self.workspace_client().is_available()
            || self.workspace_client().workspace_id().is_none()
        {
            return Err("Backend Job capability requires Backend Workspace API authority");
        }
        let retained_input = ProtectedSubjectJobInput::from_binding(&binding);
        self.backend_job
            .set(Some(binding))
            .map_err(|_| "Backend Job capability is already bound")?;
        // A retention obligation, not a grant. Persist it with every head/rewrite
        // so removing Feature configuration or the binding cannot expose input.
        if let Some(input) = retained_input {
            let mut retained = self
                .protected_inputs
                .lock()
                .expect("protected input provenance poisoned");
            retained.kinds.insert(input.kind);
            retained.subject_jobs.insert(input);
        }
        Ok(())
    }

    /// The Worker's manifest.
    pub fn manifest(&self) -> &WorkerManifest {
        &self.manifest
    }

    /// Process-local Ticket role marker supplied by the role launcher.
    pub fn runtime_ticket_role(&self) -> Option<&str> {
        self.runtime_ticket_role.as_deref()
    }

    /// Set the process-local Ticket role marker supplied by the Host at launch,
    /// not by model-visible prompts or durable claims.
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

    /// Compatibility projection for tool schemas that do not yet carry a
    /// target alias. It is available only when the attachment set has exactly
    /// one member, so multiple attachments never acquire an implicit primary.
    pub fn workdir_session(&self) -> Option<WorkdirSessionHandle> {
        self.workdir_sessions.only_session()
    }

    pub fn workdir_sessions(&self) -> Arc<WorkdirSessionRouter> {
        self.workdir_sessions.clone()
    }

    /// Replace the constructor fallback with the complete alias-keyed provider
    /// set resolved by the owning Runtime before controller startup.
    pub fn bind_workdir_sessions(&mut self, workdir_sessions: Arc<WorkdirSessionRouter>) {
        self.workdir_sessions = workdir_sessions;
    }

    /// Bind one session for direct/Internal Worker construction. Runtime Worker
    /// creation uses [`Self::bind_workdir_sessions`] with the complete set.
    pub fn bind_single_workdir_session(&mut self, workdir_session: Option<WorkdirSessionHandle>) {
        let router = Arc::new(WorkdirSessionRouter::new());
        if let Some(session) = workdir_session {
            router
                .attach(
                    WorkdirAttachmentAlias::new("workdir").expect("static alias is valid"),
                    session,
                )
                .expect("fresh Workdir router accepts its only attachment");
        }
        self.workdir_sessions = router;
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

    pub(crate) fn set_subjektiv_host(
        &mut self,
        host: Option<Arc<dyn crate::subjektiv::SubjektivHost>>,
    ) {
        self.subjektiv_host = host;
        self.session_context_source = None;
    }

    pub fn subjektiv_host_connection(
        &self,
    ) -> std::io::Result<Option<crate::subjektiv::SubjektivHostConnection>> {
        use crate::subjektiv::{
            BackendSubjektivHost, SubjektivHostConnection, SubjektivHostContext,
        };
        let config = &self.manifest.feature.subjektiv;
        config.validate_execution().map_err(std::io::Error::other)?;
        if !config.profile.enabled {
            return Ok(None);
        }
        let host = if let Some(host) = &self.subjektiv_host {
            if config.workspace_settings.is_some()
                || self.workspace_id().is_some()
                || self.workspace_client().workspace_id().is_some()
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "explicit local subjektiv Host cannot replace trusted Backend Workspace authority",
                ));
            }
            Some(host.clone())
        } else {
            BackendSubjektivHost::from_resolved_config(config, self.workspace_client_handle())?
        };
        host.map(|host| {
            SubjektivHostConnection::new(
                host,
                SubjektivHostContext {
                    worker_id: self.manifest.worker.name.clone(),
                    session_id: self.session_id().to_string(),
                },
            )
        })
        .transpose()
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

    /// Activate an Agent Skill through the Workspace backend/client and commit
    /// the returned SKILL.md body to history before it can influence an LLM run.
    ///
    /// This deliberately does not scan `.yoi/skills` locally: when a Workspace
    /// HTTP client is available, catalog/detail/activation authority belongs to
    /// the Workspace backend API.
    pub async fn activate_skill(
        &mut self,
        name: &str,
    ) -> Result<SkillActivationResponse, WorkerError>
    where
        St: Clone + 'static,
    {
        let activation = self.workspace_client().activate_skill(name)?;
        self.ensure_segment_head().await?;
        let input = crate::skill::activation_input(&activation).map_err(StoreError::from)?;
        let history_entry = HistoryEntry::new(input.item.to_history_item(), input.metadata.clone());
        self.commit_entry(LogEntry::AnnotatedSystemItem {
            ts: segment_log::now_millis(),
            entry: input,
            extensions: Vec::new(),
        })?;
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
    pub(crate) fn commit_entry(&self, mut entry: LogEntry) -> Result<(), StoreError> {
        let _admission = self.segment_state.mutation_authority.enter()?;
        entry.ensure_session_entry_id();
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

    fn hook_history_range(&self) -> HookHistoryRange {
        let entries = self.session.history().entries();
        HookHistoryRange {
            first_entry_ref: entries.first().map(|entry| {
                crate::SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id)
            }),
            last_entry_ref: entries.last().map(|entry| {
                crate::SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id)
            }),
            entry_count: entries.len(),
        }
    }

    fn hook_invocation_context(&self, run_id: Option<String>) -> HookInvocationContext {
        HookInvocationContext {
            workspace_id: self
                .workspace_context
                .workspace_id()
                .map(|id| id.as_str().to_string()),
            worker_id: self.manifest.worker.name.clone(),
            session_id: self.session.session_id().to_string(),

            run_id,
            turn_index: Some(self.engine().turn_count()),
            call_id: None,
        }
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

    pub(crate) fn disable_manifest_lifecycle_features(&mut self) {
        self.manifest_lifecycle_features_enabled = false;
    }

    pub(crate) fn manifest_lifecycle_features_enabled(&self) -> bool {
        self.manifest_lifecycle_features_enabled
    }

    pub fn feature_invocations(&self) -> FeatureInvocationRegistry {
        self.feature_invocations.clone()
    }

    pub(crate) fn authorize_feature_configuration(&self) -> Result<(), WorkspaceClientError> {
        match self.feature_installation {
            crate::feature::FeatureInstallation::Activate => {
                crate::feature::authorize_requested_manifest(
                    self.workspace_client_handle().as_ref(),
                    self.manifest(),
                )
            }
            crate::feature::FeatureInstallation::Restore => Ok(()),
        }
    }

    /// Bind Host-prepared exact Plugin contributions before controller start.
    /// This handle is never persisted in a Manifest or inherited by a SubWorker.
    pub fn bind_plugin_tools(&mut self, tools: crate::plugin::PluginToolsFeature) {
        self.plugin_tools = Some(tools);
    }

    pub(crate) fn plugin_tools(&self) -> Option<crate::plugin::PluginToolsFeature> {
        self.plugin_tools.clone()
    }

    /// Install enabled feature modules into the Worker host surfaces.
    pub fn install_features(
        &mut self,
        registry: FeatureRegistryBuilder,
    ) -> FeatureRegistryInstallReport {
        self.install_features_with_wip(registry, None)
    }

    pub(crate) fn install_features_with_wip(
        &mut self,
        registry: FeatureRegistryBuilder,
        wip_mounts: Option<&mut crate::wip::WipMountRegistry>,
    ) -> FeatureRegistryInstallReport {
        self.backend_job.get_or_init(|| None);
        let client = self.workspace_client_handle();
        let manifest = self.manifest().clone();
        let registry = registry
            .with_installation(self.feature_installation)
            .with_activation_guard(move || {
                crate::feature::authorize_requested_manifest(client.as_ref(), &manifest)
                    .map_err(|error| error.to_string())
            });
        let worker = self.engine.as_mut().expect("worker taken during run");
        let report =
            registry.install_into_engine_with_wip(worker, &mut self.hook_builder, wip_mounts);
        if report.has_errors() {
            return report;
        }
        self.feature_background_tasks = report.background_tasks.clone();
        if let Some(cleanup) = &self.startup_feature_cleanup {
            cleanup.retain(self.feature_background_tasks.clone());
        }
        self.feature_invocations = report.chat_invocations.clone();
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
        let read_only_tools = tools::read_only_routed_builtin_tools(self.workdir_sessions());
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

    /// Rewinds the current Segment to just before the specified public user input.
    ///
    /// Saves the display-history prefix and pending activation checkpoint in one Store replacement.
    /// Retains invocation-only receipts and recovery-blocked state, avoiding a failure window
    /// between truncation and checkpoint append. Rewinding history does not undo committed
    /// business side effects.
    pub async fn rewind_to(&mut self, entry_id: String) -> Result<RewindAppliedState, RewindError> {
        let _admission = self
            .segment_state
            .mutation_authority
            .enter()
            .map_err(|error| RewindError::Invalid(error.to_string()))?;
        // Reject invalid targets before invoking rewrite hooks or quiescing tasks.
        // Resolve again under the append lock after the asynchronous barrier.
        let loc = self.segment_state.location();
        resolve_rewind_input(
            &self.store.read_all(loc.session_id, loc.segment_id)?,
            &entry_id,
        )?;
        let _rewrite_guard = self
            .prepare_session_rewrite(SessionRewriteKind::Rewind)
            .await
            .map_err(|error| RewindError::Invalid(error.to_string()))?;
        // Serialize the replacement with queue admission/checkpoint appends so
        // a concurrently accepted operation cannot lose its recovery evidence.
        let segment_state = self.segment_state.clone();
        let append_guard = segment_state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        let loc = self.segment_state.location();
        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        let (entry_index, input) = resolve_rewind_input(&entries, &entry_id)?;
        let truncate_entries = rewind_truncate_entries(&entries, entry_index);
        let retained = entries[..truncate_entries].to_vec();
        let tool_side_effect_warning = suffix_has_tool_side_effects(&entries[truncate_entries..]);
        let state = segment_log::collect_state(&retained);
        let summary = RewindSummary {
            truncated_to_entries: truncate_entries,
            discarded_entries: entries.len().saturating_sub(truncate_entries),
            tool_side_effect_warning,
        };

        // Recovery evidence survives a rewind independently of visible history.
        // Persist the prefix and checkpoint in one replacement: truncating first
        // would erase receipts if the checkpoint append failed or we crashed.
        let mut replacement = retained.clone();
        let pending_state = self
            .session
            .pending_activations
            .lock()
            .expect("pending activation state poisoned")
            .clone();
        if !pending_state.pending.is_empty()
            || !pending_state.pending_notifications.is_empty()
            || pending_state.activating.is_some()
            || !pending_state.activating_notifications.is_empty()
            || !pending_state.receipts.is_empty()
            || !pending_state.notification_receipts.is_empty()
            || !pending_state.invocation_receipts.is_empty()
            || pending_state.invocation_recovery_blocked
        {
            let checkpoint = LogEntry::Extension {
                ts: segment_log::now_millis(),
                domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.to_owned(),
                payload: serde_json::to_value(&pending_state).map_err(|error| {
                    RewindError::Invalid(format!(
                        "serialize pending submissions during rewind: {error}"
                    ))
                })?,
            };
            replacement.push(checkpoint);
        }
        replacement.push(self.protected_input_checkpoint());
        self.store
            .create_segment(loc.session_id, loc.segment_id, &replacement)?;
        self.segment_state.set_entries_written(replacement.len());
        self.sink.replace_silent(replacement);
        drop(append_guard);

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

        Ok(RewindAppliedState {
            entries: retained,
            input,
            summary,
        })
    }

    fn worker_metadata(&self, active: Option<WorkerActiveSegmentRef>) -> WorkerMetadata {
        let mut metadata = worker_metadata_for_manifest(
            &self.manifest,
            self.workspace_id(),
            self.filesystem_authority
                .as_local()
                .map(|local| local.root.as_path()),
            active,
        );
        // A repaired/restored Session must not lose the saved execution policy
        // merely because its current composition also permits legacy local use.
        if self.persist_restored_manifest {
            metadata.resolved_manifest_snapshot =
                manifest::write_persisted_worker_manifest_snapshot(&self.manifest).ok();
        }
        metadata
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

    fn compare_and_swap_worker_metadata_segment(
        &self,
        expected: SegmentLocation,
        replacement: SegmentLocation,
    ) -> Result<(), WorkerError> {
        let Some(compare_and_swap) = &self.worker_metadata_segment_cas else {
            return Ok(());
        };
        let matched = compare_and_swap(
            &self.manifest.worker.name,
            &WorkerActiveSegmentRef::active_segment(expected.session_id, expected.segment_id),
            WorkerActiveSegmentRef::active_segment(replacement.session_id, replacement.segment_id),
        )?;
        if !matched {
            return Err(WorkerError::CompactActiveSegmentChanged);
        }
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
        self.worker_metadata_segment_cas = Some(worker_metadata_segment_cas_for_store(&self.store));
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
    /// `persist_turn`. Used by token-accounting APIs in [`agen::token_counter`].
    /// Returns a clone since the underlying vector is shared with hooks
    /// running on the Engine.
    pub fn usage_history(&self) -> Vec<UsageRecord> {
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clone()
    }

    /// Snapshot of the typed user segments tracked alongside worker
    /// history. The K-th entry corresponds to the K-th `Item::user_message`
    /// derived from `LogEntry::AnnotatedUserInput` entries (post-compaction); seed
    /// history loaded via `AnnotatedSegmentStart.history` does not contribute,
    /// which is acceptable because the original segments are unrecoverable.
    pub fn user_segments(&self) -> &[Vec<Segment>] {
        &self.user_segments
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

    /// Narrow read-only handle used by lifecycle Features after terminal run
    /// persistence. The capture is rebuilt from the committed log under the
    /// same append fence used by log writers.
    pub(crate) fn committed_session_capture_handle(&self) -> CommittedSessionCaptureHandle
    where
        St: Clone + Send + Sync + 'static,
    {
        let store = self.store.clone();
        let state = Arc::clone(&self.segment_state);
        CommittedSessionCaptureHandle::new(move || {
            let _append_guard = state
                .append_lock
                .lock()
                .expect("segment append lock poisoned");
            let location = state.location();
            let entries = store
                .read_all(location.session_id, location.segment_id)
                .map_err(|error| FeatureSessionError::Capture(error.to_string()))?;
            let run_exit = entries
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    LogEntry::RunCompleted {
                        result: EngineResult::Finished,
                        ..
                    } => Some(CommittedRunExit::Finished),
                    LogEntry::RunCompleted { .. }
                    | LogEntry::RunYielded { .. }
                    | LogEntry::RunResumed { .. } => Some(CommittedRunExit::NonFinal),
                    LogEntry::RunCancelled { .. } | LogEntry::RunErrored { .. } => {
                        Some(CommittedRunExit::Interrupted)
                    }
                    _ => None,
                })
                .unwrap_or(CommittedRunExit::NonFinal);
            let restored = segment_log::collect_state(&entries);
            let history =
                restore_history_entries(location.session_id, location.segment_id, &entries)
                    .map_err(|error| FeatureSessionError::Capture(error.to_string()))?;
            Ok(CommittedSessionCapture {
                session_id: location.session_id.to_string(),
                segment_id: location.segment_id.to_string(),

                entry_count: entries.len(),
                run_exit,
                history,
                usage_history: restored.usage_history,
                extensions: restored.extensions.into_iter().collect(),
            })
        })
    }

    /// Narrow fenced append authority for Feature-owned Session extensions.
    pub(crate) fn session_extension_handle(&self) -> SessionExtensionHandle
    where
        St: Clone + Send + Sync + 'static,
    {
        let writer = self.log_writer_handle();
        SessionExtensionHandle::new(move |expected, domain, payload| {
            let _append_guard = writer
                .state
                .append_lock
                .lock()
                .expect("segment append lock poisoned");
            let location = writer.state.location();
            // Compare the actual append position while holding the same lock as
            // the write. Any intervening log append or segment replacement makes
            // this captured result stale; a separate change counter adds nothing.
            if location.session_id.to_string() != expected.session_id
                || location.segment_id.to_string() != expected.segment_id
                || writer.state.entries_written() != expected.entry_count
            {
                return Ok(false);
            }
            writer
                .append_entry_locked(LogEntry::Extension {
                    ts: segment_log::now_millis(),
                    domain: domain.to_string(),
                    payload,
                })
                .map_err(|error| FeatureSessionError::Extension(error.to_string()))?;
            Ok(true)
        })
    }

    pub(crate) fn llm_client_handle(&self) -> Box<dyn LlmClient>
    where
        C: Clone,
    {
        Box::new(
            (*self
                .engine
                .as_ref()
                .expect("worker taken during run")
                .client())
            .clone(),
        )
    }

    pub(crate) fn working_event_sender(&self) -> Option<broadcast::Sender<Event>> {
        self.working_event_tx.clone()
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
    /// `.push(metric)` into it; Worker drains it at the next request accounting
    /// boundary, with terminal persistence as fallback.
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
        alerter.attach_position_source(self.sink.clone(), self.in_flight.clone());
        self.alerter = Some(alerter);
    }

    /// Attach the broadcast sender used for typed lifecycle `Event`s.
    ///
    /// The Controller wires this alongside [`Self::attach_alerter`] so that
    /// Worker-internal operations (currently: compaction) can surface
    /// progress to connected clients.
    pub fn attach_working_event_tx(&mut self, working_event_tx: broadcast::Sender<Event>) {
        let session_id = self.session_id().to_string();
        let registry = self.internal_worker_registry.get_or_insert_with(
            crate::spawn::registry::SpawnedWorkerRegistry::new_for_internal_services,
        );
        registry.attach_parent_protocol(working_event_tx.clone(), session_id);
        self.working_event_tx = Some(working_event_tx);
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
        if self.commit_entry(entry).is_err() {
            warn!(name = %metric.name, "failed to record session metric; dropping");
            let bounded_name = metric.name.chars().take(64).collect::<String>();
            self.alert(
                AlertLevel::Warn,
                AlertSource::Worker,
                format!("failed to record metric `{bounded_name}`"),
            );
        }
    }

    /// Configure the fixed idle notification coalescing window.
    ///
    /// The deadline is derived from the oldest durable notification receipt,
    /// so restart restores the remaining window rather than starting it over.
    pub fn with_notification_coalesce_delay(mut self, delay: Duration) -> Self {
        self.notification_coalesce_delay = delay;
        self
    }

    pub(crate) fn notification_coalesce_delay(&self) -> Duration {
        self.notification_coalesce_delay
    }

    pub fn push_notify(&self, message: String) {
        self.pending_notifies.push_notify(message);
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
        self.pending_notifies
            .bind_mutation_authority(self.segment_state.mutation_authority.clone());
        self.pending_notifies.clone()
    }

    pub(crate) fn install_compaction_budget(
        &mut self,
        budget: Option<crate::compact::worker::CompactWorkerInterceptor>,
    ) {
        self.assert_hooks_open();
        self.compaction_service_budget = budget.map(|budget| {
            Arc::new(
                budget.with_request_shape(
                    self.engine()
                        .get_system_prompt()
                        .unwrap_or_default()
                        .to_owned(),
                    self.engine().tool_server_handle(),
                ),
            )
        });
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
            self.hook_registry = Some(registry.clone());

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

            let compact_state = {
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
            .with_compaction_budget(self.compaction_service_budget.clone())
            .with_notification_committer(self.notification_committer.clone())
            .with_usage_tracker(self.usage_tracker.clone())
            .with_metrics_tracker(self.metrics_tracker.clone())
            .with_resident_context_source(self.feature_resident_summary_source.clone())
            .with_protected_input_supply(self.protected_input_supply())
            .with_prompt_workspace_id(
                self.workspace_context
                    .workspace_id()
                    .map(|workspace_id| workspace_id.as_str().to_string()),
            );
            self.engine_mut().set_interceptor(interceptor);
            self.interceptor_installed = true;
        }
    }

    async fn notify_features_of_restore(&mut self) -> Result<(), WorkerError>
    where
        St: Clone + 'static,
    {
        if !self.restore_feature_lifecycle_pending {
            return Ok(());
        }
        self.ensure_interceptor_installed();
        let pending_system_items = Arc::new(Mutex::new(Vec::new()));
        if let Some(hooks) = self.hook_registry.clone() {
            let context = WorkerRestoredContext::new(
                self.hook_invocation_context(None),
                RestoreSystemItemAppendHandle::new(Arc::clone(&pending_system_items)),
            );
            tokio::time::timeout(
                FEATURE_HOOK_CHAIN_TIMEOUT,
                hooks.on_worker_restored(&context),
            )
            .await
            .map_err(|_| {
                hooks.record_chain_timeout("worker-restored");
                WorkerError::FeatureLifecycle(
                    "worker-restored hook chain exceeded its host deadline".to_string(),
                )
            })?
            .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
        }

        let system_items = std::mem::take(
            &mut *pending_system_items
                .lock()
                .expect("restore system-item queue poisoned"),
        );
        for entry in system_items {
            let history_item = entry.item.to_history_item();
            let metadata = entry.metadata.clone();
            self.commit_entry(LogEntry::AnnotatedSystemItem {
                ts: segment_log::now_millis(),
                entry,
                extensions: Vec::new(),
            })?;
            let history_entry = HistoryEntry::new(history_item, metadata);
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
        }
        self.restore_feature_lifecycle_pending = false;
        Ok(())
    }

    /// Render the manifest-supplied instruction template exactly once,
    /// just before the first LLM turn, append the fixed trailing
    /// section (scope summary + optional AGENTS.md), and hand the
    /// resulting string to the Engine via `set_system_prompt`.
    /// Keep the template until the atomic initial seed is committed. A failed
    /// preparation must not accidentally turn into an ordinary restore.
    async fn ensure_system_prompt_materialized(&mut self) -> Result<(), WorkerError> {
        let Some(template) = self.system_prompt_template.as_ref() else {
            return Ok(());
        };
        if let Some(body) = self.feature_system_prompt_override.as_ref() {
            let environment =
                crate::prompt::system::workdir_environment_section(&self.workdir_sessions)
                    .map_err(|source| WorkerError::SystemPromptRender { source })?;
            self.engine
                .as_mut()
                .expect("worker present")
                .set_system_prompt(format!("{body}\n\n{environment}"));
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
        let mut completion = None;
        let resident_summary = if self.inject_resident_summary {
            match &self.feature_resident_summary_source {
                Some(source) => {
                    let (value, prepared) =
                        source.load_initial_checked().await.map_err(|reason| {
                            WorkerError::InvalidState(format!(
                                "resident context startup failed: {reason}"
                            ))
                        })?;
                    match value {
                        SystemPromptContribution::Ready(summary) if !summary.is_empty() => {
                            self.protected_inputs
                                .lock()
                                .expect("protected input provenance poisoned")
                                .retain(source.protected_input_kind())
                                .map_err(WorkerError::InvalidState)?;
                            completion = prepared;
                            Some(summary)
                        }
                        _ => None,
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let worker_language = worker_language(&self.manifest.engine);
        let scope_snapshot = self.scope.snapshot();
        let prompt_catalog = self.prompts.load_full();
        let ctx = SystemPromptContext {
            now: chrono::Utc::now(),
            workdir_sessions: &self.workdir_sessions,
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
        self.prepared_initial_prompt_completion = completion.and_then(|complete| {
            resident_summary
                .as_ref()
                .filter(|body| rendered.contains(body.as_str()))
                .map(|_| (rendered.clone(), complete))
        });
        self.engine
            .as_mut()
            .expect("worker present")
            .set_system_prompt(rendered);
        Ok(())
    }

    /// Convenience: run with a single `Segment::Text`.
    ///
    /// Equivalent to `run(vec![Segment::text(s)])`. The dumb-client
    /// counterpart of [`protocol::Method::submit_text`]; primarily for
    /// tests and tools that have only a string in hand.
    pub async fn run_text(&mut self, s: impl Into<String>) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.run(vec![Segment::text(s)]).await
    }

    /// Whether `try_pre_run_compact` would actually compact. The same
    /// check is duplicated inside `try_pre_run_compact` itself for
    /// defensive reasons; this is the gate for joining the memory task
    /// before the compact runs.
    fn should_pre_run_compact(&self) -> bool {
        self.compact_state
            .as_ref()
            .is_some_and(|state| state.pre_run_eligible(self.total_tokens().tokens))
    }

    pub(crate) fn needs_initial_session_head_materialization(&self) -> bool {
        self.segment_state.entries_written() == 0
    }

    /// Materialize the initial durable session head without running the model.
    ///
    /// Controllers call this before exposure so even a zero-input Worker has
    /// an authoritative restore snapshot independent of
    /// operation-owned launch configuration.
    pub async fn materialize_durable_session_head(&mut self) -> Result<(), WorkerError>
    where
        St: Clone + 'static,
    {
        self.refresh_prompt_projection_for_future_operations()?;
        self.ensure_segment_head().await?;
        self.notify_features_of_restore().await?;
        Ok(())
    }

    /// Prelude shared by `run` / `run_for_notification` / `resume`.
    /// Wires up worker hooks, ensures the session is materialized on the
    /// store, and runs pre-run compact (joining any in-flight memory task
    /// first so extract sees a stable history range).
    async fn prepare_for_run(&mut self) -> Result<(), WorkerError>
    where
        St: Clone + 'static,
    {
        self.materialize_durable_session_head().await?;
        let writer = self.log_writer_handle();
        let activity = self.ai_activity_counter.clone();
        self.engine_mut()
            .set_image_rejection_handler(move |original, replacement| {
                writer
                    .append_entry(LogEntry::ToolResultCorrected {
                        ts: segment_log::now_millis(),
                        entry: session_store::LoggedHistoryEntry {
                            item: replacement.clone().into(),
                            metadata: original.annotation.clone(),
                        },
                    })
                    .map_err(|error| error.to_string())?;
                // A later Cancel must not roll back this committed correction while
                // leaving the in-memory result changed (the original may predate Run).
                activity.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        self.ensure_interceptor_installed();
        if self.should_pre_run_compact() {
            self.try_pre_run_compact().await?;
        }
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
            EngineRunExit::Paused | EngineRunExit::Interrupted(RunInterruptionReason::Cancelled)
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
        let _admission = self.segment_state.mutation_authority.enter()?;
        self.session.history_mut().truncate(snapshot.history_len);

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

    /// Resolves Submit's Flow selector and prepares initial state and instruction projections.
    ///
    /// Rejects duplicate active instances, multiple Flow segments, disabled Features, and Workspace
    /// mismatches. Source resolution is read-only and does not yet change the live instance.
    /// The caller commits typed input, the instruction history projection, and Flow state in one
    /// AnnotatedUserInput before updating in-memory state.
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
        let request = server_api::FlowSourceResolveRequest {
            selector: selector.to_string(),
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
        let wire_source: server_api::ResolvedFlowSource = serde_json::from_str(&response.body)
            .map_err(|error| WorkerError::FlowInput(format!("decode Flow source: {error}")))?;
        if wire_source.workspace_id != workspace_id || wire_source.selector != selector.to_string()
        {
            return Err(WorkerError::FlowInput(
                "Workspace returned a Flow source outside the requested scope".to_string(),
            ));
        }
        let source: flow::ResolvedFlowSource =
            serde_json::from_value(serde_json::to_value(wire_source).map_err(|error| {
                WorkerError::FlowInput(format!("project Flow source transport: {error}"))
            })?)
            .map_err(|error| WorkerError::FlowInput(format!("convert Flow source: {error}")))?;
        let (state, initial_instructions) =
            flow::FlowRuntimeState::start(&source, uuid::Uuid::now_v7().to_string())
                .map_err(|error| WorkerError::FlowInput(error.to_string()))?;
        let projection = PreparedFlowProjection {
            selector: selector.to_string(),
            instructions: initial_instructions,
            definition_id: state.instance.definition_id.clone(),
            definition_digest: state.instance.definition_digest.clone(),
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
        self.run_with_input_extensions_and_commit_hook(
            input,
            input_extensions,
            WorkerHistoryProvenance::LegacyUnknown,
            || {},
        )
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
        input_provenance: WorkerHistoryProvenance,
        on_input_committed: F,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
        F: FnOnce(),
    {
        validate_activation_invocation_ids(&input).map_err(WorkerError::FeatureInvocation)?;
        let (mut input, pending_flow_state, flow_projection) = self.prepare_flow_input(input)?;
        let projected_entry_ids = if flow_projection.is_some() {
            (0..input.len())
                .map(|_| SessionHistoryEntryId::new())
                .collect::<Vec<_>>()
        } else {
            vec![SessionHistoryEntryId::new()]
        };
        self.materialize_large_pastes(&mut input, &projected_entry_ids, flow_projection.is_some())?;
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
        self.ensure_interceptor_installed();
        if let Some(state) = &self.compact_state {
            state.begin_logical_run();
        }

        self.prepare_for_run().await?;

        let rollback_snapshot = self.capture_empty_turn_rollback_snapshot();

        // IDLE → active marker. Commits first so the next UserInput entry
        // is contained inside this Invoke range. See `tickets/invoke-turn-llmcall-semantics.md`.
        self.commit_entry(LogEntry::Invoke {
            ts: segment_log::now_millis(),
            trigger: protocol::InvokeKind::UserSend,
        })?;

        let projected_input = self.projected_input_history(
            &input,
            flow_projection.as_ref(),
            &projected_entry_ids,
            &input_provenance,
        );

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
        self.finalize_uploaded_segment_bindings(
            &input,
            &projected_entry_ids,
            flow_projection.is_some(),
        );
        if let Some(state) = pending_flow_state {
            *self
                .flow_runtime_state
                .lock()
                .expect("flow_runtime_state poisoned") = Some(state);
        }
        self.user_segments.push(input.clone());
        on_input_committed();

        // Explicit invocation handlers run only after the typed input is durable
        // and before any dependent LLM request. They execute in source order;
        // the first failure fences later invocations in the same submission.
        let has_invocations = input
            .iter()
            .any(|segment| matches!(segment, Segment::FeatureInvoke { .. }));
        match self.execute_feature_invocations(&input).await {
            Ok(false) => {}
            Ok(true) => {
                self.materialize_committed_history(&projected_input);
                return Err(WorkerError::FeatureInvocation(
                    "activation preparation failed or has an unknown outcome; dependent LLM request was not sent".into(),
                ));
            }
            Err(error) => {
                self.materialize_committed_history(&projected_input);
                return Err(error);
            }
        }
        let committed_activation_input = has_invocations.then(|| projected_input.clone());

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

        if let Some(input) = &committed_activation_input {
            // A prompt hook may cancel before Engine materializes input/results.
            // Business preparation is already durable and cannot be rolled back.
            self.materialize_committed_history(input);
        }
        if !has_invocations && self.should_rollback_empty_turn(&result, &rollback_snapshot) {
            self.rollback_empty_turn(rollback_snapshot)?;
            return Ok(WorkerRunResult::RolledBack);
        }

        self.handle_worker_result(result, history_before).await
    }

    /// Processes committed Submit invocations in segment order and returns whether dependent LLM
    /// execution may proceed.
    ///
    /// Persists start in the pending activation extension before calling the handler, then appends
    /// the result and updated receipt in one AnnotatedSystemItem. If result append fails, start
    /// remains, preventing reexecution of uncertain side effects within this process or after
    /// restore, Pause/Resume, or compaction. Exact replay uses saved results but revalidates semantics
    /// and authority before reusing successful context. Receipt exhaustion rejects untrackable
    /// execution rather than dropping old evidence. The first failure/unknown result stops later
    /// invocations and dependent LLM execution without rolling back earlier side effects.
    async fn execute_feature_invocations(&self, segments: &[Segment]) -> Result<bool, WorkerError>
    where
        St: Clone + 'static,
    {
        use protocol::{FeatureInvocationResult, FeatureInvocationStatus};
        let invocations = segments
            .iter()
            .filter_map(|segment| match segment {
                Segment::FeatureInvoke { invocation } => Some(invocation),
                _ => None,
            })
            .collect::<Vec<_>>();
        let mut prior_failure = false;
        for invocation in invocations {
            let failed = |status, message: String| FeatureInvocationResult {
                invocation_id: invocation.invocation_id.clone(),
                identity: invocation.identity.clone(),
                status,
                message,
                context: None,
            };
            let handle = self.pending_submission_handle();
            // Use the queue's append/state lock order. Never hold a lock while
            // awaiting business execution. A durable start without result fences
            // both restart and same-process replay.
            let (existing, recovery_blocked) = {
                let state = handle
                    .state
                    .lock()
                    .expect("pending activation state poisoned");
                (
                    state
                        .invocation_receipts
                        .iter()
                        .find(|receipt| receipt.invocation_id == invocation.invocation_id)
                        .cloned(),
                    state.invocation_recovery_blocked,
                )
            };
            let result = if recovery_blocked {
                failed(
                    FeatureInvocationStatus::OutcomeUnknown,
                    "invocation recovery checkpoint is invalid; execution is fenced".into(),
                )
            } else if prior_failure {
                failed(
                    FeatureInvocationStatus::Failed,
                    "not executed because an earlier invocation in this submission failed".into(),
                )
            } else if let Some(receipt) = &existing {
                if !receipt.matches(invocation) {
                    failed(
                        FeatureInvocationStatus::Failed,
                        "invocation id was already used with a different payload".into(),
                    )
                } else {
                    let replay = receipt.result.clone().unwrap_or_else(|| {
                        failed(
                            FeatureInvocationStatus::OutcomeUnknown,
                            "a durable invocation start has no result; do not retry automatically"
                                .into(),
                        )
                    });
                    if replay.status == FeatureInvocationStatus::Succeeded {
                        // Recorded success is reusable context, not authority to
                        // resurrect a removed capability or revoked argument
                        // permission. Revalidate without repeating the effect.
                        match self.feature_invocations.validate(invocation) {
                            Ok(()) => replay,
                            Err(error) => failed(FeatureInvocationStatus::Failed, error.message),
                        }
                    } else {
                        replay
                    }
                }
            } else if let Err(error) = self.feature_invocations.validate(invocation) {
                failed(FeatureInvocationStatus::Failed, error.message)
            } else {
                let started = {
                    let _append = handle
                        .writer
                        .state
                        .append_lock
                        .lock()
                        .expect("segment append lock poisoned");
                    let mut state = handle
                        .state
                        .lock()
                        .expect("pending activation state poisoned");
                    if state.invocation_receipts.len() >= MAX_INVOCATION_RECEIPTS {
                        false
                    } else {
                        let mut started = state.clone();
                        started
                            .invocation_receipts
                            .push(InvocationReceipt::new(invocation, None));
                        handle
                            .persist_locked(&started)
                            .map_err(|error| WorkerError::FeatureInvocation(error.to_string()))?;
                        *state = started;
                        true
                    }
                };
                if !started {
                    failed(FeatureInvocationStatus::Failed,
                        "invocation recovery receipt capacity reached; refusing untracked execution".into())
                } else {
                    match self.feature_invocations.invoke(invocation).await {
                        Ok(result) => result,
                        Err(error) => failed(
                            if error.outcome_unknown {
                                FeatureInvocationStatus::OutcomeUnknown
                            } else {
                                FeatureInvocationStatus::Failed
                            },
                            error.message,
                        ),
                    }
                }
            };
            prior_failure |= result.status != FeatureInvocationStatus::Succeeded;
            let body = match (&result.status, &result.context) {
                (FeatureInvocationStatus::Succeeded, Some(context)) => format!(
                    "[Feature invocation succeeded: {}]\n{}",
                    result.identity, context
                ),
                (FeatureInvocationStatus::Succeeded, None) => format!(
                    "[Feature invocation succeeded: {}] {}",
                    result.identity, result.message
                ),
                (FeatureInvocationStatus::Failed, _) => format!(
                    "[Feature invocation failed: {}] {}",
                    result.identity, result.message
                ),
                (FeatureInvocationStatus::OutcomeUnknown, _) => format!(
                    "[Feature invocation outcome unknown: {}] {} Do not retry automatically.",
                    result.identity, result.message
                ),
            };
            // Commit result and recovery checkpoint atomically with the existing
            // AnnotatedSystemItem lifecycle. Failed appends leave the start fenced.
            let _append = handle
                .writer
                .state
                .append_lock
                .lock()
                .expect("segment append lock poisoned");
            let mut state = handle
                .state
                .lock()
                .expect("pending activation state poisoned");
            let mut completed = state.clone();
            if let Some(receipt) = completed
                .invocation_receipts
                .iter_mut()
                .find(|receipt| receipt.matches(invocation) && receipt.result.is_none())
            {
                receipt.result = Some(result.clone());
            } else if existing.is_none()
                && completed.invocation_receipts.len() < MAX_INVOCATION_RECEIPTS
            {
                completed
                    .invocation_receipts
                    .push(InvocationReceipt::new(invocation, Some(result.clone())));
            }
            let input =
                crate::feature::invocation::result_input(result, body).map_err(StoreError::from)?;
            let history = HistoryEntry::new(input.item.to_history_item(), input.metadata.clone());
            handle
                .writer
                .append_entry_locked(LogEntry::AnnotatedSystemItem {
                    ts: segment_log::now_millis(),
                    entry: input,
                    extensions: vec![pending_activation_extension(&completed)],
                })?;
            *state = completed;
            self.pending_committed_history
                .lock()
                .expect("pending committed history poisoned")
                .push_back(history);
        }
        Ok(prior_failure)
    }

    fn materialize_committed_history(&mut self, input: &[HistoryEntry<SessionHistoryMetadata>]) {
        let pending = self
            .pending_committed_history
            .lock()
            .expect("pending committed history poisoned")
            .drain(..)
            .collect::<Vec<_>>();
        for entry in input.iter().cloned().chain(pending) {
            if !self
                .session
                .history()
                .entries()
                .iter()
                .any(|existing| existing.annotation.entry_id == entry.annotation.entry_id)
            {
                self.session.history_mut().push_entry(entry);
            }
        }
    }

    pub(crate) fn can_schedule_notification_run(&self) -> bool {
        !self.has_failed_segment_activation()
            && !self.has_pending_compaction_cleanup()
            && self.ensure_invocation_preparation_complete().is_ok()
    }

    fn ensure_invocation_preparation_complete(&self) -> Result<(), WorkerError> {
        let state = self
            .session
            .pending_activations
            .lock()
            .expect("pending activation state poisoned");
        if state.invocation_recovery_blocked {
            return Err(WorkerError::FeatureInvocation(
                "invocation recovery checkpoint is invalid".into(),
            ));
        }
        if let Some(input) = self.user_segments.last() {
            for invocation in input.iter().filter_map(|segment| match segment {
                Segment::FeatureInvoke { invocation } => Some(invocation),
                _ => None,
            }) {
                let succeeded = state.invocation_receipts.iter().any(|receipt| {
                    receipt.matches(invocation)
                        && receipt.result.as_ref().is_some_and(|result| {
                            result.status == protocol::FeatureInvocationStatus::Succeeded
                        })
                });
                if !succeeded || self.feature_invocations.validate(invocation).is_err() {
                    return Err(WorkerError::FeatureInvocation(
                        "activation preparation is failed, incomplete, or outcome unknown; submit fresh input instead of resuming".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve every `Segment::FileRef` in `segments` to a `[File: <path>]`
    /// or shallow `[Dir: <path>]` system message via `WorkerFsView`. Resolution
    /// failures (out-of-scope, not-found, binary, I/O, unsupported symlink
    /// directory) surface as `AlertLevel::Warn` Alerts and are skipped — the
    /// unresolved placeholder stays in the flattened user message so the LLM
    /// still sees the intent.
    async fn resolve_file_refs(&self, segments: &[Segment]) -> Vec<SystemItem> {
        let Some(workdir) = self.workdir_session() else {
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
            extensions: Vec::new(),
        })?;
        let interrupt_entry =
            HistoryEntry::new(agen::Item::system_message(system_note), interrupt_metadata);
        self.session.history_mut().push_entry(interrupt_entry);

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
            entry_id: None,
            ts: segment_log::now_millis(),
        })?;
        Ok(())
    }

    fn finalize_uploaded_segment_bindings(
        &self,
        input: &[Segment],
        projected_entry_ids: &[SessionHistoryEntryId],
        one_entry_per_segment: bool,
    ) {
        for (index, segment) in input.iter().enumerate() {
            let Segment::UploadedFile { file } = segment else {
                continue;
            };
            let entry_index = if one_entry_per_segment { index } else { 0 };
            let source_entry_id = projected_entry_ids
                .get(entry_index)
                .expect("projected input id exists for every uploaded file")
                .0
                .as_str();
            if let Err(error) = self.store.finalize_uploaded_file_binding(
                self.session_id(),
                &file.artifact_id,
                source_entry_id,
            ) {
                tracing::warn!(
                    artifact_id = %file.artifact_id,
                    error = %error,
                    "deferred uploaded file pin finalization to cleanup reconciliation"
                );
            }
        }
    }

    fn materialize_large_pastes(
        &self,
        input: &mut [Segment],
        projected_entry_ids: &[SessionHistoryEntryId],
        one_entry_per_segment: bool,
    ) -> Result<(), WorkerError> {
        let uploaded_file_count = input
            .iter()
            .filter(|segment| matches!(segment, Segment::UploadedFile { .. }))
            .count();
        if uploaded_file_count > session_store::DEFAULT_MAX_FILES_PER_SUBMISSION {
            return Err(WorkerError::Store(StoreError::ArtifactQuotaExceeded));
        }
        for (index, segment) in input.iter_mut().enumerate() {
            if let Segment::UploadedFile { file } = segment {
                if file.source_entry_id.is_some()
                    || file.availability != protocol::UploadedFileAvailability::Available
                {
                    return Err(WorkerError::Store(StoreError::ArtifactIntegrityMismatch));
                }
                let entry_index = if one_entry_per_segment { index } else { 0 };
                let source_entry_id = projected_entry_ids
                    .get(entry_index)
                    .expect("projected input id exists for every uploaded file")
                    .0
                    .clone();
                *file = self
                    .store
                    .bind_uploaded_file(self.session_id(), file, &source_entry_id)
                    .map_err(WorkerError::Store)?;
                continue;
            }
            if let Segment::PasteArtifact { artifact } = segment {
                let (stored, _) = self
                    .store
                    .read_paste_artifact(self.session_id(), &artifact.artifact_id)?;
                if &stored != artifact {
                    return Err(WorkerError::Store(StoreError::PasteArtifactIntegrity(
                        artifact.artifact_id.clone(),
                    )));
                }
                continue;
            }
            let Segment::Paste { content, .. } = segment else {
                continue;
            };
            if content.len() <= LARGE_PASTE_INLINE_MAX_BYTES {
                continue;
            }
            let entry_index = if one_entry_per_segment { index } else { 0 };
            let source_entry_id = projected_entry_ids
                .get(entry_index)
                .expect("projected input id exists for every paste")
                .0
                .as_str();
            let artifact = self.store.write_paste_artifact(
                self.session_id(),
                source_entry_id,
                content,
                PasteArtifactLimits::default(),
            )?;
            *segment = Segment::PasteArtifact { artifact };
        }
        Ok(())
    }

    fn projected_input_history(
        &self,
        input: &[Segment],
        flow_projection: Option<&PreparedFlowProjection>,
        entry_ids: &[SessionHistoryEntryId],
        provenance: &WorkerHistoryProvenance,
    ) -> Vec<HistoryEntry<SessionHistoryMetadata>> {
        if let Some(flow) = flow_projection {
            return input
                .iter()
                .zip(entry_ids)
                .map(|(segment, entry_id)| match segment {
                    Segment::Flow { .. } => history_entry_with_id(
                        Item::user_message(flow.instructions.clone()),
                        entry_id.clone(),
                        WorkerHistoryProvenance::FlowInstruction {
                            selector: flow.selector.clone(),
                            definition_id: flow.definition_id.clone(),
                            definition_digest: Some(flow.definition_digest.clone()),
                            instance_id: flow.instance_id.clone(),
                            state_id: flow.state_id.clone(),
                        },
                    ),
                    other => history_entry_with_id(
                        Item::user_message(Segment::flatten_to_text(std::slice::from_ref(other))),
                        entry_id.clone(),
                        provenance.clone(),
                    ),
                })
                .collect();
        }

        vec![history_entry_with_id(
            Item::user_message(Segment::flatten_to_text(input)),
            entry_ids
                .first()
                .expect("projected Worker input always has one entry id")
                .clone(),
            provenance.clone(),
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
        self.ensure_invocation_preparation_complete()?;
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
        self.ensure_interceptor_installed();
        if let Some(state) = &self.compact_state {
            state.begin_logical_run();
        }
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

        self.handle_worker_result(result, history_before).await
    }

    /// Resume from an intentional user pause. The durable transition is
    /// committed only after preparation succeeds and immediately before Engine
    /// receives control.
    pub async fn resume(&mut self) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.resume_from(RunResumeSource::Pause).await
    }

    async fn resume_from(&mut self, source: RunResumeSource) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        self.ensure_invocation_preparation_complete()?;
        self.prepare_for_run().await?;
        self.resume_prepared(source).await
    }

    async fn resume_prepared(
        &mut self,
        source: RunResumeSource,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        let active_run_turn_count = self
            .engine()
            .active_run_turn_count()
            .expect("pause/yield resume must retain the logical-run turn budget");
        self.commit_entry(LogEntry::RunResumed {
            ts: segment_log::now_millis(),
            entry_id: None,
            source,
            active_run_turn_count,
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

        self.handle_worker_result(result, history_before).await
    }

    fn ensure_no_pending_usage_for_segment_replacement(&self) -> Result<(), WorkerError> {
        let pending_records = self.usage_tracker.pending_record_count();
        if pending_records == 0 {
            Ok(())
        } else {
            Err(WorkerError::PendingUsagePersistence { pending_records })
        }
    }

    /// Commit the complete initial context atomically and repair the active
    /// pointer from that seed before admitting any input. A read error is not
    /// an initialized-empty contribution; an ambiguous write is reconciled
    /// from the segment instead of fetching and appending again.
    async fn initialize_session_head(&mut self) -> Result<(), WorkerError> {
        let _admission = self.segment_state.mutation_authority.enter()?;
        let loc = self.segment_state.location();
        // A lost create response must be reconciled before any Host refetch. Empty
        // pre-created logs are not initialized heads and may still receive a seed.
        let committed = if self.store.exists(loc.session_id, loc.segment_id)? {
            self.store.read_all(loc.session_id, loc.segment_id)?
        } else {
            Vec::new()
        };
        let seed = if !committed.is_empty() {
            committed
        } else {
            if self.prepared_initial_seed.is_none() {
                self.ensure_system_prompt_materialized().await?;
                let items = match &self.session_context_source {
                    Some(source) => {
                        let items = source.load().await.map_err(WorkerError::InvalidState)?;
                        self.protected_inputs
                            .lock()
                            .expect("protected input provenance poisoned")
                            .retain_context_items(source.protected_input_kind(), &items)
                            .map_err(WorkerError::InvalidState)?;
                        items
                    }
                    None => Vec::new(),
                };
                let engine = self.engine.as_ref().expect("worker present");
                let mut seed = vec![LogEntry::AnnotatedSegmentStart {
                    ts: segment_log::now_millis(),
                    session_id: loc.session_id,
                    system_prompt: engine.get_system_prompt().map(String::from),
                    config: engine.request_config().clone(),
                    history: self
                        .session
                        .history()
                        .entries()
                        .iter()
                        .map(to_logged_history_entry)
                        .collect(),
                    forked_from: None,
                    compacted_from: None,
                }];
                for entry in items {
                    seed.push(LogEntry::AnnotatedSystemItem {
                        ts: segment_log::now_millis(),
                        entry,
                        extensions: Vec::new(),
                    });
                }
                seed.push(self.protected_input_checkpoint());
                for entry in &mut seed {
                    entry.ensure_session_entry_id();
                }
                self.prepared_initial_seed = Some(seed);
            }
            let seed = self.prepared_initial_seed.as_ref().expect("prepared seed");
            self.store
                .create_segment(loc.session_id, loc.segment_id, seed)?;
            seed.clone()
        };
        let Some(LogEntry::AnnotatedSegmentStart {
            session_id,
            system_prompt,
            forked_from: None,
            compacted_from: None,
            ..
        }) = seed.first()
        else {
            return Err(WorkerError::InvalidState(
                "initial segment is not a complete seed".into(),
            ));
        };
        if *session_id != loc.session_id
            || seed
                .iter()
                .skip(1)
                .any(|entry| !matches!(entry, LogEntry::AnnotatedSystemItem { .. })
                    && !matches!(entry, LogEntry::Extension { domain, .. } if domain == PROTECTED_INPUTS_DOMAIN))
        {
            return Err(WorkerError::InvalidState(
                "unexpected records in initial segment seed".into(),
            ));
        }
        let history = restore_history_entries(loc.session_id, loc.segment_id, &seed)
            .map_err(WorkerError::InvalidState)?;
        let provenance = ProtectedInputProvenance::restore(&seed, &history, None)?;
        *self
            .protected_inputs
            .lock()
            .expect("protected input provenance poisoned") = provenance;
        // Keep entries_written at zero until this succeeds, so retry repairs a
        // metadata failure instead of permitting User input after an orphaned seed.
        self.write_worker_metadata_active(loc)?;
        if let Some(prompt) = system_prompt {
            self.engine
                .as_mut()
                .expect("worker present")
                .set_system_prompt(prompt.clone());
        }
        self.session.replace_history(history);
        self.sink.reset_with_initial_entries(seed.clone());
        self.segment_state.set_entries_written(seed.len());
        self.system_prompt_template = None;
        self.feature_system_prompt_override = None;
        self.prepared_initial_seed = None;
        if let Some((prepared, complete)) = self.prepared_initial_prompt_completion.take() {
            if system_prompt.as_ref() == Some(&prepared) {
                complete();
            }
        }
        Ok(())
    }

    async fn ensure_segment_head(&mut self) -> Result<(), WorkerError> {
        let _admission = self.segment_state.mutation_authority.enter()?;
        let loc = self.segment_state.location();
        let entries_written = self.segment_state.entries_written();
        if entries_written == 0 {
            return self.initialize_session_head().await;
        }
        // Check store count + auto-fork if it drifted.
        let store_count = self
            .store
            .read_entry_count(loc.session_id, loc.segment_id)
            .map_err(WorkerError::from)?;
        if store_count == entries_written {
            return Ok(());
        }
        self.ensure_no_pending_usage_for_segment_replacement()?;
        // Auto-fork within the same Session: mint a fresh Segment and
        // switch to it. The source segment is left immutable (no terminal
        // marker is written back); the fork relationship is recorded
        // forward on the new segment's `forked_from`, with `at_turn_index`
        // = the writer's current turn (its in-memory history reflects
        // state up to that turn). The new SegmentStart replaces the mirror
        // and is broadcast through the sink so existing subscribers reset
        // their view.
        let _rewrite_guard = self
            .prepare_session_rewrite(SessionRewriteKind::Fork)
            .await?;
        let segment_state = Arc::clone(&self.segment_state);
        let _append_guard = segment_state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        segment_state.ensure_append_allowed()?;
        let loc = segment_state.location();
        let fork_segment_id = session_store::new_segment_id();
        let w = self.engine.as_ref().unwrap();
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
        let mut initial_entries = vec![
            entry.clone(),
            LogEntry::InputSegmentsCheckpoint {
                ts: segment_log::now_millis(),
                user_segments: self.user_segments.clone(),
            },
        ];
        if let Some(checkpoint) =
            active_run_checkpoint_entry(w.active_run_turn_count(), w.turn_count())
        {
            initial_entries.push(checkpoint);
        }
        initial_entries.push(self.protected_input_checkpoint());
        initial_entries.push(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.to_owned(),
            payload: serde_json::to_value(
                &*self
                    .session
                    .pending_activations
                    .lock()
                    .expect("pending activation state poisoned"),
            )
            .map_err(|error| {
                WorkerError::InvalidState(format!(
                    "serialize pending submissions during automatic fork: {error}"
                ))
            })?,
        });
        self.store
            .create_segment(loc.session_id, fork_segment_id, &initial_entries)
            .map_err(WorkerError::from)?;
        let new_location = SegmentLocation {
            session_id: loc.session_id,
            segment_id: fork_segment_id,
        };
        let allocation_activation = self
            .scope_allocation
            .as_ref()
            .map(|allocation| allocation.begin_segment_activation(loc.segment_id, fork_segment_id))
            .transpose()?;
        self.compare_and_swap_worker_metadata_segment(loc, new_location)?;
        if let Some(allocation_activation) = allocation_activation {
            if let Err(error) = allocation_activation.commit() {
                let (source, authority) = error.into_parts();
                self.fail_closed_segment_activation(authority);
                return Err(WorkerError::SegmentActivationIncomplete { source });
            }
        }
        self.segment_state.set_location(new_location);
        self.segment_state
            .set_entries_written(initial_entries.len());
        self.sink
            .reset_with_initial_entries(initial_entries.clone());
        Ok(())
    }

    /// Handle one Engine execution boundary: persist its turn and transition,
    /// then compact and resume when Engine yielded for compaction.
    ///
    /// A yield commits `RunYielded`, never `RunCompleted`. Successful preparation
    /// later commits `RunResumed` on the active replacement Segment immediately
    /// before Engine regains control; a compaction failure instead commits the
    /// same logical Run's terminal `RunErrored`.
    async fn handle_worker_result(
        &mut self,
        result: EngineRunExit,
        history_before: usize,
    ) -> Result<WorkerRunResult, WorkerError>
    where
        St: Clone + 'static,
    {
        // A producer may durably append a prefix before a later append fails.
        // Preserve that exact annotated prefix even when Engine never got to
        // materialize it; retries must neither lose nor reappend it.
        self.materialize_committed_history(&[]);
        let run_id = uuid::Uuid::now_v7().to_string();
        let hook_exit = hook_run_exit(&result);
        if let Some(hooks) = self.hook_registry.clone() {
            let context = RunExitContext {
                invocation: self.hook_invocation_context(Some(run_id.clone())),
                exit: hook_exit,
                history_len: self.session.history().len(),
            };
            match tokio::time::timeout(FEATURE_HOOK_CHAIN_TIMEOUT, hooks.on_run_exit(&context))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "run-exit hook failed; preserving terminal commit");
                }
                Err(_) => {
                    hooks.record_chain_timeout("run-exit");
                    tracing::warn!("run-exit hook chain exceeded its host deadline");
                }
            }
        }
        if matches!(&result, EngineRunExit::Interrupted(_)) {
            self.terminalize_orphan_tool_calls()?;
        }
        let request_block = self
            .compact_state
            .as_ref()
            .and_then(|state| state.take_pending_request_block());
        let request_block_message = request_block.map(|block| {
            format!(
                "mid-run compaction could not continue: {}",
                automatic_compact_block_error(block)
            )
        });
        self.persist_turn(history_before, &result, request_block_message.as_deref())
            .await?;
        let committed_invocation = self.hook_invocation_context(Some(run_id));
        if let Some(hooks) = self.hook_registry.clone() {
            let context = RunCommittedContext {
                invocation: committed_invocation.clone(),
                exit: hook_exit,
                committed_history: self.hook_history_range(),
            };
            match tokio::time::timeout(FEATURE_HOOK_CHAIN_TIMEOUT, hooks.on_run_committed(&context))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::warn!(error = %error, "run-committed hook requires attention");
                }
                Err(_) => {
                    hooks.record_chain_timeout("run-committed");
                    tracing::warn!("run-committed hook chain exceeded its host deadline");
                }
            }
        }
        if let Err(error) = self
            .feature_background_tasks
            .start_run_committed(committed_invocation)
        {
            tracing::warn!(error = %error, "run-committed background task start failed");
        }

        if let Some(block) = request_block {
            let error = automatic_compact_block_error(block);
            let error = if matches!(result, EngineRunExit::Yielded) {
                self.terminalize_compaction_run_failure(
                    request_block_message.expect("blocked request has a terminal reason"),
                    error,
                )
            } else {
                // `persist_turn` above already committed this non-yielded Run's
                // compaction terminal. Preserve Err semantics while preventing a
                // second Controller Error for the same durable entry.
                WorkerError::RunFailureAlreadyDelivered {
                    source: Box::new(error),
                }
            };
            if let Some(state) = &self.compact_state {
                state.finish_logical_run();
            }
            return Err(error);
        }

        if matches!(result, EngineRunExit::Yielded) {
            self.last_run_interrupted = true;
            return self.do_compact_and_resume().await;
        }

        if !matches!(result, EngineRunExit::Paused) {
            if let Some(state) = &self.compact_state {
                state.finish_logical_run();
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
            EngineRunExit::Interrupted(RunInterruptionReason::LimitReached) => {
                self.last_run_interrupted = false;
                Ok(WorkerRunResult::LimitReached)
            }
            EngineRunExit::Interrupted(RunInterruptionReason::Cancelled) => {
                self.last_run_interrupted = false;
                Ok(WorkerRunResult::Cancelled)
            }
            EngineRunExit::Interrupted(reason) => {
                self.last_run_interrupted = true;
                Ok(WorkerRunResult::Interrupted {
                    code: run_interruption_reason_error_code(&reason),
                    message: run_interruption_reason_message(&reason),
                })
            }
            EngineRunExit::Yielded => unreachable!("yielded handled above"),
        }
    }

    fn set_compaction_progress(&self, compaction: Option<InFlightCompaction>) {
        if let Some(in_flight) = &self.in_flight {
            in_flight.set_compaction(compaction);
        } else if let Some(tx) = &self.working_event_tx {
            let _ = tx.send(Event::CompactionProgress { compaction });
        }
    }

    fn persist_compaction_run_failure(&mut self, message: String) -> Result<(), WorkerError> {
        // The Store is the terminal authority. Only a successful append can be
        // marked as already delivered to clients through the session-log sink.
        self.commit_entry(LogEntry::RunErrored {
            ts: segment_log::now_millis(),
            entry_id: None,
            interrupted: false,
            message,
            failure: Some(RunFailureKind::Compaction),
        })?;
        self.last_run_interrupted = false;
        self.engine_mut().set_active_run_turn_count(None);
        Ok(())
    }

    fn terminalize_compaction_run_failure(
        &mut self,
        message: String,
        run_failure: WorkerError,
    ) -> WorkerError {
        match self.persist_compaction_run_failure(message) {
            Ok(()) => WorkerError::RunFailureAlreadyDelivered {
                source: Box::new(run_failure),
            },
            Err(persist_error) => WorkerError::RunTerminalPersistence {
                run_failure: Box::new(run_failure),
                persist_error: Box::new(persist_error),
            },
        }
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
            let state = self.compact_state.clone();
            if let Some(state) = &state {
                if !state.has_claimed_attempt() {
                    return Err(WorkerError::AutomaticCompactState(
                        "automatic compaction yield had no claimed attempt".to_string(),
                    ));
                }
            }

            let retained = state
                .as_ref()
                .map(|s| s.retained_tokens())
                .unwrap_or(manifest::defaults::COMPACT_RETAINED_TOKENS);

            match self
                .compact_with_cancel(retained, None, CompactionTrigger::RequestThreshold)
                .await
            {
                Ok(new_segment_id) => {
                    info!(
                        new_segment_id = %new_segment_id,
                        "Compaction succeeded, resuming execution"
                    );
                    if let Some(state) = &state {
                        let completed = state.complete_automatic(CompactionOutcome::Succeeded);
                        debug_assert!(
                            completed,
                            "automatic compaction must complete a claimed attempt"
                        );
                    }
                    if let Err(error) = self.prepare_for_run().await {
                        if let Some(state) = &state {
                            state.finish_logical_run();
                        }
                        let message =
                            format!("mid-run resume preparation failed after compaction: {error}");
                        return Err(self.terminalize_compaction_run_failure(message, error));
                    }
                    self.resume_prepared(RunResumeSource::Compaction).await
                }
                Err(e) => {
                    warn!(error = %e, "Compaction failed during run");
                    let message = format!("mid-run compaction failed: {e}");
                    if let Some(state) = &state {
                        let outcome = if matches!(e, WorkerError::CompactCancelled) {
                            CompactionOutcome::Cancelled
                        } else {
                            CompactionOutcome::Failed(compact_failure_category(&e))
                        };
                        let completed = state.complete_automatic(outcome);
                        debug_assert!(
                            completed,
                            "automatic compaction must complete a claimed attempt"
                        );
                        state.finish_logical_run();
                    }

                    // The yielded logical Run is no longer resumable. Commit its
                    // durable failure before returning a typed handled error. The
                    // Controller suppresses only that exact handled variant; if
                    // persistence fails it instead reports both failures through
                    // the non-durable error channel.
                    Err(self.terminalize_compaction_run_failure(message, e))
                }
            }
        })
    }

    /// Attempt proactive compaction at the beginning of a controller Run.
    ///
    /// This used to run in the controller's post-run path. Keeping it here
    /// preserves the ordering requirement that the next turn starts with a
    /// compacted history, without introducing a separate Busy controller state.
    /// Fatal activation failures are returned to the controller so it cannot
    /// publish Idle or start the triggering turn with inconsistent authority.
    pub async fn try_pre_run_compact(&mut self) -> Result<(), WorkerError> {
        let Some(state) = self.compact_state.clone() else {
            return Ok(());
        };
        let current_tokens = self.total_tokens().tokens;
        match state.evaluate_pre_run(current_tokens) {
            AutomaticCompactDecision::Start(AutomaticCompactTrigger::PreRun) => {}
            AutomaticCompactDecision::Continue | AutomaticCompactDecision::Block(_) => {
                return Ok(());
            }
            AutomaticCompactDecision::Start(AutomaticCompactTrigger::RequestThreshold) => {
                unreachable!("pre-run evaluation returned request-threshold trigger")
            }
        }

        let retained = state.retained_tokens();
        match self
            .compact_with_cancel(retained, None, CompactionTrigger::PreRun)
            .await
        {
            Ok(new_segment_id) => {
                info!(
                    new_segment_id = %new_segment_id,
                    "Proactive pre-run compaction succeeded"
                );
                let completed = state.complete_automatic(CompactionOutcome::Succeeded);
                debug_assert!(
                    completed,
                    "automatic compaction must complete a claimed attempt"
                );
                Ok(())
            }
            Err(e) => {
                warn!(error = %e, "Proactive pre-run compaction failed");
                self.alert(
                    AlertLevel::Warn,
                    AlertSource::Compactor,
                    format!("pre-run compaction failed: {e}"),
                );
                let outcome = if matches!(e, WorkerError::CompactCancelled) {
                    CompactionOutcome::Cancelled
                } else {
                    CompactionOutcome::Failed(compact_failure_category(&e))
                };
                let completed = state.complete_automatic(outcome);
                debug_assert!(
                    completed,
                    "automatic compaction must complete a claimed attempt"
                );
                if matches!(
                    &e,
                    WorkerError::SegmentActivationIncomplete { .. }
                        | WorkerError::CompactionCleanupPending { .. }
                ) {
                    Err(e)
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Run an explicit user-requested compaction between turns.
    ///
    /// The controller only calls this while Idle. Paused turns keep their
    /// interrupted Engine state intact and are intentionally rejected before
    /// this method is reached.
    async fn prepare_session_rewrite(
        &mut self,
        kind: SessionRewriteKind,
    ) -> Result<BackgroundTaskRewriteGuard, WorkerError> {
        let rewrite_guard = self
            .feature_background_tasks
            .begin_session_rewrite()
            .await
            .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
        if let Some(hooks) = self.hook_registry.clone() {
            let context = BeforeSessionRewriteContext {
                invocation: self.hook_invocation_context(None),
                kind,
                current_history: self.hook_history_range(),
            };
            let action = tokio::time::timeout(
                FEATURE_HOOK_CHAIN_TIMEOUT,
                hooks.before_session_rewrite(&context),
            )
            .await
            .map_err(|_| {
                hooks.record_chain_timeout("before-session-rewrite");
                WorkerError::FeatureLifecycle(
                    "session-rewrite hook chain exceeded its host deadline".to_string(),
                )
            })?
            .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?;
            match action {
                BeforeSessionRewriteAction::Continue => {}
                BeforeSessionRewriteAction::Deny(reason) => {
                    return Err(WorkerError::FeatureLifecycle(reason));
                }
            }
        }
        Ok(rewrite_guard)
    }

    /// Legacy compaction extensions are ignored on restore. Current compaction
    /// progress is runtime-only and cannot survive a process restart.
    pub async fn recover_unfinished_compaction(&mut self) -> Result<(), WorkerError> {
        Ok(())
    }

    pub async fn manual_compact(&mut self) -> Result<ManualCompactResult, WorkerError> {
        self.manual_compact_inner(None).await
    }

    pub async fn manual_compact_with_cancel(
        &mut self,
        cancel: tokio::sync::watch::Receiver<bool>,
    ) -> Result<ManualCompactResult, WorkerError> {
        self.manual_compact_inner(Some(cancel)).await
    }

    async fn manual_compact_inner(
        &mut self,
        mut cancel: Option<tokio::sync::watch::Receiver<bool>>,
    ) -> Result<ManualCompactResult, WorkerError> {
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
        self.ensure_segment_head().await?;

        let state = self.compact_state.clone();

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

        match self
            .compact_with_cancel(retained, cancel.take(), CompactionTrigger::Manual)
            .await
        {
            Ok(new_segment_id) => {
                info!(new_segment_id = %new_segment_id, "Manual compaction succeeded");
                Ok(ManualCompactResult::Compacted { new_segment_id })
            }
            Err(WorkerError::CompactCancelled) => {
                info!("Manual compaction was cancelled");
                Err(WorkerError::CompactCancelled)
            }
            Err(e) => {
                warn!(error = %e, "Manual compaction failed");
                self.alert(
                    AlertLevel::Error,
                    AlertSource::Compactor,
                    format!("manual compaction failed: {e}"),
                );
                Err(e)
            }
        }
    }

    /// Persist delta + turn end + outcome after a run/resume.
    async fn persist_turn(
        &mut self,
        history_before: usize,
        result: &EngineRunExit,
        compaction_failure: Option<&str>,
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
            let new_entries: Vec<_> = self.session.history().entries()[history_before..]
                .iter()
                .map(to_logged_history_entry)
                .collect();
            let ts = segment_log::now_millis();
            for history_entry in new_entries {
                let item = Item::from(history_entry.item.clone());
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
                let entry = session_store::classify_logged_history_entry(history_entry, ts);
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

        // Persist any request measurements that did not already cross the
        // in-run request boundary (for example partial usage from an errored or
        // cancelled stream, or low-level Workers without an attached writer).
        // `persist_pending_usage` removes each record only after its authoritative
        // LlmUsage append succeeds, so the in-run and terminal paths cannot
        // double-write or silently lose a failed append.
        let usage_tracker = self.usage_tracker.clone();
        let usage_history = self.usage_history.clone();
        let compact_state = self.compact_state.clone();
        if let Some(writer) = self.log_writer.clone() {
            let _ = persist_pending_usage(
                &usage_tracker,
                &usage_history,
                compact_state.as_deref(),
                |entry, after_commit, post_metrics| {
                    writer.commit_usage_boundary(entry, after_commit, post_metrics)
                },
            )?;
        } else {
            let post_request_metrics = persist_pending_usage(
                &usage_tracker,
                &usage_history,
                compact_state.as_deref(),
                |entry, after_commit, _post_metrics| {
                    self.commit_entry(entry)?;
                    after_commit();
                    Ok(())
                },
            )?;
            for metric in post_request_metrics {
                self.try_record_metric(&metric);
            }
        }

        let interrupted = matches!(
            result,
            EngineRunExit::Paused
                | EngineRunExit::Yielded
                | EngineRunExit::Interrupted(RunInterruptionReason::Cancelled)
                | EngineRunExit::Interrupted(RunInterruptionReason::ContextWindowExceeded)
                | EngineRunExit::Interrupted(RunInterruptionReason::Unexpected(_))
        );
        let active_run_turn_count = self.engine.as_ref().unwrap().active_run_turn_count();
        if let Some(message) = compaction_failure
            && !matches!(result, EngineRunExit::Yielded)
        {
            self.commit_entry(LogEntry::RunErrored {
                ts: segment_log::now_millis(),
                entry_id: None,
                interrupted: false,
                message: message.to_string(),
                failure: Some(RunFailureKind::Compaction),
            })?;
            return Ok(());
        }
        match result {
            EngineRunExit::Finished | EngineRunExit::Paused => {
                let result = match result {
                    EngineRunExit::Finished => EngineResult::Finished,
                    EngineRunExit::Paused => EngineResult::Paused,
                    EngineRunExit::Yielded | EngineRunExit::Interrupted(_) => unreachable!(),
                };
                self.commit_entry(LogEntry::RunCompleted {
                    entry_id: None,
                    ts: segment_log::now_millis(),
                    interrupted,
                    result,
                    active_run_turn_count,
                })?;
            }
            EngineRunExit::Yielded => {
                self.commit_entry(LogEntry::RunYielded {
                    ts: segment_log::now_millis(),
                    entry_id: None,
                    reason: RunYieldReason::Compaction,
                    active_run_turn_count: active_run_turn_count
                        .expect("yielded logical run must retain its turn budget"),
                })?;
            }
            EngineRunExit::Interrupted(RunInterruptionReason::LimitReached) => {
                self.commit_entry(LogEntry::RunCompleted {
                    entry_id: None,
                    ts: segment_log::now_millis(),
                    interrupted: false,
                    result: EngineResult::LimitReached,
                    active_run_turn_count,
                })?;
            }
            EngineRunExit::Interrupted(RunInterruptionReason::Cancelled) => {
                self.commit_entry(LogEntry::RunCancelled {
                    ts: segment_log::now_millis(),
                    entry_id: None,
                })?;
            }
            EngineRunExit::Interrupted(reason) => {
                self.commit_entry(LogEntry::RunErrored {
                    ts: segment_log::now_millis(),
                    entry_id: None,
                    interrupted,
                    message: run_interruption_reason_message(reason),
                    failure: Some(RunFailureKind::Engine),
                })?;
            }
        }

        Ok(())
    }

    /// Saves a new Segment and switches history references through parent-owned compaction.
    ///
    /// Completes pre-rewrite hooks and confirms background tasks stopped, then saves the summary
    /// and retained tail in a new Segment. Creation alone does not change current history; CAS of
    /// the metadata active reference is the commit point. Lock order is append barrier, allocation,
    /// then metadata, excluding Submit/Notify during the switch. If allocation update fails after
    /// CAS, rejects appends and retains exclusion while reporting the mismatch. Success aligns the
    /// append destination, history, and cache identity before publishing the new base to subscribers.
    ///
    /// Confirms release of the compaction Internal Worker before activation. Cancellation or future
    /// drop alone does not prove release; unconfirmed cleanup blocks the next execution. Progress
    /// is a runtime observation, not a restartable durable workflow record.
    pub async fn compact(&mut self, retained_tokens: u64) -> Result<SegmentId, WorkerError> {
        self.compact_with_cancel(retained_tokens, None, CompactionTrigger::RequestThreshold)
            .await
    }

    async fn compact_with_cancel(
        &mut self,
        retained_tokens: u64,
        mut cancel: Option<tokio::sync::watch::Receiver<bool>>,
        trigger: CompactionTrigger,
    ) -> Result<SegmentId, WorkerError> {
        self.ensure_no_pending_usage_for_segment_replacement()?;
        self.release_pending_compaction_service().await?;
        let _rewrite_guard = self
            .prepare_session_rewrite(SessionRewriteKind::Compact)
            .await?;
        let mut lifecycle = CompactionLifecycle {
            schema_version: 3,
            compaction_id: uuid::Uuid::now_v7().to_string(),
            internal_worker: None,
            state: CompactionLifecycleState::Running,
            started_at_ms: segment_log::now_millis(),
            ended_at_ms: None,
            summary: None,
            error: None,
            new_segment_id: None,
        };
        let started = Instant::now();
        let source_location = self.segment_state.location();
        let history_items = self.session.history().items_cloned();
        let usage_history = self.usage_history();
        let pre_context = agen::token_counter::total_tokens(&history_items, &usage_history);
        let metric_correlation_id = new_compact_metric_correlation_id(&lifecycle.compaction_id);
        let attempt = CompactAttempt::new(
            metric_correlation_id,
            source_location.session_id,
            source_location.segment_id,
            match trigger {
                CompactionTrigger::Manual => CompactMode::Manual,
                CompactionTrigger::PreRun | CompactionTrigger::RequestThreshold => {
                    CompactMode::Automatic
                }
            },
            match trigger {
                CompactionTrigger::Manual => CompactThresholdPolicy::Manual,
                CompactionTrigger::PreRun => CompactThresholdPolicy::PreRun,
                CompactionTrigger::RequestThreshold => CompactThresholdPolicy::RequestThreshold,
            },
            pre_context.tokens,
            pre_context.source,
            retained_tokens,
        );
        self.try_record_metric(&attempt.start_metric());
        let started_at_ms = lifecycle.started_at_ms;
        self.set_compaction_progress(Some(InFlightCompaction {
            phase: CompactionPhase::Preparing,
            started_at_ms,
            trigger,
        }));
        let outcome = if let Some(cancel) = cancel.as_mut() {
            tokio::select! {
                biased;
                changed = cancel.changed() => {
                    let _ = changed;
                    Err(WorkerError::CompactCancelled)
                }
                result = self.compact_impl(retained_tokens, &mut lifecycle, trigger) => result,
            }
        } else {
            self.compact_impl(retained_tokens, &mut lifecycle, trigger)
                .await
        };
        match outcome {
            Ok((new_segment_id, _summary, stats)) => {
                debug_assert_eq!(lifecycle.state, CompactionLifecycleState::Done);
                self.set_compaction_progress(None);
                for metric in attempt.success_metrics(new_segment_id, started.elapsed(), &stats) {
                    self.try_record_metric(&metric);
                }
                self.usage_tracker
                    .note_compaction_correlation_id(attempt.correlation_id().to_string());
                Ok(new_segment_id)
            }
            Err(error) => {
                let error = if matches!(&error, WorkerError::CompactionCleanupPending { .. }) {
                    error
                } else {
                    match self.release_compaction_service(&mut lifecycle).await {
                        Ok(()) => error,
                        Err(cleanup_error) => cleanup_error,
                    }
                };
                let observed_segment_id = self.segment_state.location().segment_id;
                for metric in attempt.failure_metrics(
                    observed_segment_id,
                    started.elapsed(),
                    compact_failure_category(&error),
                ) {
                    self.try_record_metric(&metric);
                }
                lifecycle.state = if matches!(&error, WorkerError::CompactCancelled) {
                    CompactionLifecycleState::Interrupted
                } else {
                    CompactionLifecycleState::Failed
                };
                lifecycle.ended_at_ms = Some(segment_log::now_millis());
                if !matches!(&error, WorkerError::CompactionCleanupPending { .. }) {
                    self.set_compaction_progress(None);
                }
                Err(error)
            }
        }
    }

    pub(crate) fn has_pending_compaction_cleanup(&self) -> bool {
        self.pending_compaction_cleanup
            .lock()
            .expect("pending compaction cleanup mutex poisoned")
            .is_some()
    }

    fn fail_closed_segment_activation(&self, authority: SegmentActivationGuard) {
        *self
            .failed_activation_authority
            .lock()
            .expect("failed activation authority mutex poisoned") = Some(authority);
        self.segment_state.fail_closed_activation();
    }

    pub(crate) fn has_failed_segment_activation(&self) -> bool {
        self.segment_state.activation_failed.load(Ordering::Acquire)
    }

    pub(crate) async fn retry_pending_compaction_cleanup(&self) -> Result<(), WorkerError> {
        let cleanup_pending = self.has_pending_compaction_cleanup();
        self.release_pending_compaction_service().await?;
        if cleanup_pending {
            self.set_compaction_progress(None);
        }
        Ok(())
    }

    pub(crate) async fn finish_pending_compaction_cleanup_for_shutdown(
        &self,
    ) -> Result<(), WorkerError> {
        loop {
            match self.retry_pending_compaction_cleanup().await {
                Ok(()) => return Ok(()),
                Err(error) => {
                    warn!(
                        error = %error,
                        "shutdown remains blocked on compaction service cleanup"
                    );
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn release_pending_compaction_service(&self) -> Result<(), WorkerError> {
        let cleanup = self
            .pending_compaction_cleanup
            .lock()
            .expect("pending compaction cleanup mutex poisoned")
            .clone();
        let Some(cleanup) = cleanup else {
            return Ok(());
        };
        let Some(registry) = &self.internal_worker_registry else {
            return Err(WorkerError::CompactionCleanupPending {
                source: ScopeLockError::Io(std::io::Error::other(
                    "compaction registry unavailable during cleanup",
                )),
            });
        };
        match registry.stop_service(&cleanup.session_id).await {
            Ok(_) => {
                let mut pending = self
                    .pending_compaction_cleanup
                    .lock()
                    .expect("pending compaction cleanup mutex poisoned");
                if pending
                    .as_ref()
                    .is_some_and(|pending| pending.session_id == cleanup.session_id)
                {
                    *pending = None;
                }
                Ok(())
            }
            Err(source) => {
                warn!(
                    compaction_id = %cleanup.compaction_id,
                    error = %source,
                    "compaction service cleanup failed; retaining cleanup authority"
                );
                Err(WorkerError::CompactionCleanupPending {
                    source: ScopeLockError::Io(source),
                })
            }
        }
    }

    async fn release_compaction_service(
        &self,
        lifecycle: &mut CompactionLifecycle,
    ) -> Result<(), WorkerError> {
        self.release_pending_compaction_service().await?;
        lifecycle.internal_worker = None;
        Ok(())
    }

    async fn compact_impl(
        &mut self,
        retained_tokens: u64,
        lifecycle: &mut CompactionLifecycle,
        trigger: CompactionTrigger,
    ) -> Result<(SegmentId, String, CompactSuccessStats), WorkerError> {
        use crate::compact::worker::{
            CompactWorkerContext, CompactWorkerInterceptor, CompactionOutputFeature,
        };
        use crate::fs_view::WorkerFsView;

        // Decide the cut point by projecting the UsageRecord timeline onto
        // the current history: keep the tail whose estimated token count is
        // within `retained_tokens`. Item-granular, turn boundaries ignored.
        let cut = self.split_for_retained(retained_tokens);

        // Freeze one Host-owned read before starting the compactor. Background recording
        // remains independent; never re-read when building the replacement segment.
        self.protected_input_supply()
            .authorize(
                self.session
                    .history()
                    .entries()
                    .iter()
                    .map(|entry| &entry.annotation),
            )
            .await
            .map_err(WorkerError::InvalidState)?;
        let frozen_context = match &self.session_context_source {
            Some(source) => source
                .load_compaction_context(self.session_id())
                .await
                .map_err(WorkerError::InvalidState)?,
            None => Vec::new(),
        };
        for entry in &frozen_context {
            entry
                .validate_input_metadata()
                .map_err(|e| WorkerError::InvalidState(e.to_string()))?;
        }
        self.protected_inputs
            .lock()
            .expect("protected input provenance poisoned")
            .retain_context_items(
                self.session_context_source
                    .as_ref()
                    .and_then(|s| s.protected_input_kind()),
                &frozen_context,
            )
            .map_err(WorkerError::InvalidState)?;
        let history_entries = self.session.history().entries();
        let retain_from = cut.index.min(history_entries.len());
        let initial_summary = initial_compaction_summary(history_entries)?;
        // The old summary is consumed in full by the compaction service even when
        // the cut falls before it. The replacement summary supersedes it: retaining
        // it as well would duplicate inherited context on every compact.
        let mut retained_history_entries = history_entries[retain_from..]
            .iter()
            .filter(|entry| {
                crate::session_history::compaction_use(&entry.annotation)
                    == session_store::InputCompactionUse::Retain
            })
            .cloned()
            .collect::<Vec<_>>();
        let default_workdir_alias = (self.workdir_sessions.len() == 1)
            .then(|| self.workdir_sessions.aliases().into_iter().next())
            .flatten()
            .map(|alias| alias.as_str().to_string());
        let retained_image_references = project_compaction_attachments(
            retained_history_entries
                .iter_mut()
                .map(|entry| &mut entry.item),
            default_workdir_alias.as_deref(),
        );
        // The compactor explores summaries, text details, and ViewImage call arguments. It
        // never needs durable image bodies; relevant images are nominated with add_reference.
        let mut entries_to_summarise = history_entries[..retain_from]
            .iter()
            .filter(|entry| {
                crate::session_history::compaction_use(&entry.annotation)
                    == session_store::InputCompactionUse::Retain
            })
            .cloned()
            .collect::<Vec<_>>();
        project_compactor_capture_attachments(&mut entries_to_summarise);
        // Project before Host selection so omitted summary/reference inputs never
        // rebase the original source positions of the retained prefix entries.
        let mut exploration_entries = history_entries[..retain_from].to_vec();
        project_compactor_capture_attachments(&mut exploration_entries);
        let summarised_ids = entries_to_summarise
            .iter()
            .map(|entry| entry.annotation.entry_id.0.as_str())
            .collect::<std::collections::HashSet<_>>();
        let exploration_view = crate::session_capture::SessionCapture::from_complete_history(
            self.session_id().to_string(),
            self.segment_id().to_string(),
            exploration_entries,
        )
        .retain_entries(|entry| summarised_ids.contains(entry.entry_id.as_str()));
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
        let default_refs: Vec<PathBuf> = if self.workdir_sessions.len() == 1 {
            self.tracker
                .as_ref()
                .map(|t| t.recent_files(manifest::defaults::COMPACT_DEFAULT_REFERENCE_COUNT))
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        // The full Host summary is mandatory input, separate from the bounded
        // prefix overview and current TaskStore snapshot.
        let task_snapshot_text = self.task_feature.snapshot_text();
        let mut summary_input = build_summary_input(
            &items_to_summarise,
            initial_summary.as_deref(),
            &default_refs,
            Some(task_snapshot_text.as_str()),
            SummaryInputOptions {
                overview_target_tokens,
                overview_warning_tokens,
                overview_deadline_tokens,
                summary_target_tokens,
            },
        );
        // Previous references are historical source material, not additional latest
        // Journals. In particular, a failed/empty read must not silently lose context
        // that the previous summary intentionally delegated to its injected Journal.
        // Carry them only into the compactor, never alongside the new reference seed.
        for entry in history_entries.iter().filter(|entry| {
            crate::session_history::compaction_use(&entry.annotation)
                == session_store::InputCompactionUse::ReferenceInput
        }) {
            if let Some(body) = entry.item.as_text() {
                summary_input.text.push_str(
                    "\n\n## Previous reference input (historical, not automatically retained)\n",
                );
                summary_input.text.push_str(body);
                summary_input
                    .text
                    .push_str("\n## End previous reference input\n");
            }
        }
        // Mandatory context, outside overview truncation. The internal Worker's normal
        // request budget checks include this exact body and the full previous summary.
        summary_input.text.push_str("\n\n");
        for (index, entry) in frozen_context.iter().enumerate() {
            if index != 0 {
                summary_input.text.push_str("\n\n");
            }
            summary_input.text.push_str(&entry.item.history_text());
        }
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
        let workdir = self.workdir_session();
        let read_only_workdir = workdir.clone().map(|session| {
            Arc::new(ReadOnlyWorkdirSession::new(session)) as workdir::WorkdirSessionHandle
        });
        let summary_tracker = tools::Tracker::new();
        let summary_client: Box<dyn LlmClient> = Box::new(ProtectedCompactorClient {
            inner: self.build_compactor_client()?,
            supply: self.protected_input_supply(),
        });
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
                        exploration_view,
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
                compaction_budget: Some(interceptor),
                engine_configurator: Some(Box::new(move |engine| {
                    let tracker = tracker_for_engine;
                    engine.on_usage(move |event| {
                        tracker.record_usage(event);
                    });
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
        lifecycle.internal_worker = Some(internal_ref);
        let cleanup = PendingCompactionCleanup {
            session_id: handle.session_id_string(),
            compaction_id: lifecycle.compaction_id.clone(),
        };
        *self
            .pending_compaction_cleanup
            .lock()
            .expect("pending compaction cleanup mutex poisoned") = Some(cleanup);
        self.set_compaction_progress(Some(InFlightCompaction {
            phase: CompactionPhase::Summarizing,
            started_at_ms: lifecycle.started_at_ms,
            trigger,
        }));

        if let Err(error) = handle.send(summary_input.text).await {
            // Keep the registry record and Worker-owned cleanup authority. The
            // outer compaction boundary performs the single stop attempt.
            return Err(WorkerError::InvalidState(error.to_string()));
        }
        ensure_compaction_internal_turn_succeeded(handle.wait_for_turn_outcome().await)?;

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
                     User Directives / Current Work), preserving ongoing goals and user \
                     directives from the initial summary and segment. Nominate any files the next \
                     session needs with `mark_read_required` for UTF-8 text or \
                     `add_reference` for images, binary files, and on-demand reads."
                        .to_string(),
                )
            } else if snapshot.read_required.is_empty() && !default_refs.is_empty() {
                Some(
                    "Summary received. If any of the referenced UTF-8 text files must \
                     be present in the next session, call `mark_read_required` on them. \
                     Use `add_reference` for images, binary files, or files that can be \
                     fetched on demand. Otherwise reply briefly to close out."
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
            ensure_compaction_internal_turn_succeeded(handle.wait_for_turn_outcome().await)?;
        }

        let mut final_ctx = ctx.lock().expect("compact ctx poisoned").clone();
        let mut summary_text = final_ctx
            .summary
            .clone()
            .ok_or(WorkerError::CompactSummaryMissing)?;
        let mut summary_tokens = estimate_text_tokens(summary_text.len());
        if summary_max_tokens > 0 && summary_tokens > summary_max_tokens {
            let prompt =
                summary_shrink_prompt(summary_tokens, summary_max_tokens, summary_target_tokens);
            handle
                .send(prompt)
                .await
                .map_err(|error| WorkerError::InvalidState(error.to_string()))?;
            ensure_compaction_internal_turn_succeeded(handle.wait_for_turn_outcome().await)?;
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
        let auto_read_tokens = agen::token_counter::total_tokens(&auto_read_messages, &[]).tokens;
        let retained_estimate = agen::token_counter::total_tokens(
            &retained_history_entries
                .iter()
                .map(|entry| entry.item.clone())
                .collect::<Vec<_>>(),
            &[],
        );
        let retained_item_count = u64::try_from(retained_history_entries.len()).unwrap_or(u64::MAX);
        let summarized_item_count = u64::try_from(items_to_summarise.len()).unwrap_or(u64::MAX);
        let compactor_entries = handle.entries();
        let compactor_turns = compactor_entries
            .iter()
            .filter_map(|entry| match entry {
                LogEntry::TurnEnd { turn_count, .. } => u64::try_from(*turn_count).ok(),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let compactor_tool_calls = u64::try_from(
            compactor_entries
                .iter()
                .filter(|entry| {
                    matches!(
                        entry,
                        LogEntry::AnnotatedAssistantItem { entry, .. }
                            if matches!(&entry.item, session_store::LoggedItem::ToolCall { .. })
                    )
                })
                .count(),
        )
        .unwrap_or(u64::MAX);
        let mut compactor_usage = crate::compact::usage_tracker::UsageSnapshot::default();
        let mut compactor_requests = 0_u64;
        for entry in &compactor_entries {
            if let LogEntry::LlmUsage {
                input_total_tokens,
                cache_read_tokens,
                cache_write_tokens,
                output_tokens,
                ..
            } = entry
            {
                compactor_requests = compactor_requests.saturating_add(1);
                compactor_usage.input_total_tokens = compactor_usage
                    .input_total_tokens
                    .saturating_add(*input_total_tokens);
                compactor_usage.cache_read_tokens = compactor_usage
                    .cache_read_tokens
                    .saturating_add(*cache_read_tokens);
                compactor_usage.cache_write_tokens = compactor_usage
                    .cache_write_tokens
                    .saturating_add(*cache_write_tokens);
                compactor_usage.output_tokens =
                    compactor_usage.output_tokens.saturating_add(*output_tokens);
            }
        }

        // Related files are carried by name only. Retained ViewImage payloads are
        // projected out above and become explicit on-demand image references here.
        let reference_message =
            build_related_file_message(&final_ctx.references, &retained_image_references);

        // Count surviving user_messages before consuming the annotated retained tail
        // — needed to align `self.user_segments` after the swap below.
        let retained_user_msgs = retained_history_entries
            .iter()
            .filter(|entry| entry.item.is_user_message())
            .count();

        // Build new history: [summary, ...auto-read, references, ...retained, task snapshot, TaskList synthetic call/result].
        // The TaskStore snapshot trails the retained items so that, on resume,
        // `replay_history` walks any pre-compact Task* calls preserved verbatim
        // in the retained tail first and the trailing snapshot's `replace_with`
        // is the final word — pre-compact `TaskCreate` calls cannot leak as
        // duplicate entries.
        let mut compact_introduced_system_messages =
            Vec::with_capacity(2 + auto_read_messages.len() + reference_message.is_some() as usize);
        let summary_message = Item::system_message(format!(
            "{}{summary_text}",
            crate::session_history::COMPACTION_SUMMARY_HEADER
        ));
        compact_introduced_system_messages.push(summary_message.clone());
        let context_messages = frozen_context
            .iter()
            .map(|entry| entry.item.to_history_item())
            .collect::<Vec<_>>();
        compact_introduced_system_messages.extend(context_messages.iter().cloned());
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

        let original_entries = self.session.history().entries();
        let derived_sources = original_entries
            .iter()
            .map(|entry| entry.annotation.entry_id.clone())
            .collect::<Vec<_>>();
        let generic_summary_input = session_store::SystemInputEnvelope::new(
            summary_message
                .as_text()
                .expect("compaction summary is text"),
            session_store::HistoryInputMetadata::new(
                session_store::HistoryInputUsePolicy {
                    recording_material: session_store::InputEvidenceUse::Exclude,
                    memory_evidence: session_store::InputEvidenceUse::Exclude,
                    observation: session_store::InputObservationUse::Hidden,
                    compaction: session_store::InputCompactionUse::OpeningSummary,
                },
                None,
                None,
            )
            .map_err(|e| WorkerError::InvalidState(e.to_string()))?,
        )
        .map_err(|e| WorkerError::InvalidState(e.to_string()))?;
        let summary_entry = session_store::LoggedSystemHistoryEntry::from_input(
            WorkerHistoryProvenance::Provided {
                producer: "worker.compaction".into(),
            },
            Some(SessionHistoryDerivation {
                sources: derived_sources.clone(),
            }),
            generic_summary_input,
        );
        let derived_entry = |item| {
            let mut metadata = new_history_metadata(
                WorkerHistoryProvenance::DerivedSummary,
                Some(SessionHistoryDerivation {
                    sources: derived_sources.clone(),
                }),
            );
            metadata.input_metadata = Some(
                session_store::HistoryInputMetadata::new(
                    session_store::HistoryInputUsePolicy {
                        recording_material: session_store::InputEvidenceUse::Exclude,
                        memory_evidence: session_store::InputEvidenceUse::Exclude,
                        observation: session_store::InputObservationUse::Hidden,
                        compaction: session_store::InputCompactionUse::Retain,
                    },
                    None,
                    None,
                )
                .expect("bounded private derived context policy"),
            );
            HistoryEntry::new(item, metadata)
        };
        // Retained items and annotations travel together, including when attachment
        // projection changes an item. Equal model text never selects an identity.
        let mut compacted_history_entries =
            vec![HistoryEntry::new(summary_message, summary_entry.metadata)];
        compacted_history_entries.extend(
            frozen_context.iter().map(|entry| {
                HistoryEntry::new(entry.item.to_history_item(), entry.metadata.clone())
            }),
        );
        compacted_history_entries.extend(auto_read_messages.into_iter().map(&derived_entry));
        if let Some(item) = reference_message {
            compacted_history_entries.push(derived_entry(item));
        }
        compacted_history_entries.extend(retained_history_entries);
        compacted_history_entries.push(derived_entry(task_snapshot_message));
        compacted_history_entries.push(derived_entry(Item::tool_call(
            "compact-tasklist",
            "TaskList",
            "{}",
        )));
        compacted_history_entries.push(derived_entry(Item::tool_result_with_content(
            "compact-tasklist",
            self.task_feature.snapshot_overview(),
            task_snapshot_text.clone(),
        )));
        let new_history = compacted_history_entries
            .iter()
            .map(|entry| entry.item.clone())
            .collect::<Vec<_>>();
        let result_estimate = agen::token_counter::total_tokens(&new_history, &[]);
        if result_context_max_tokens > 0 && result_estimate.tokens > result_context_max_tokens {
            // This history estimate includes retained items and auto-read files, not
            // just the summary. Treat the configured result target as advisory;
            // request-time context safety checks still apply after activation.
            warn!(
                result_tokens = result_estimate.tokens,
                warning_threshold = result_context_max_tokens,
                retained_tokens = retained_estimate.tokens,
                summary_tokens,
                auto_read_tokens,
                "compacted result context exceeds warning threshold; continuing"
            );
            self.alert(
                AlertLevel::Warn,
                AlertSource::Compactor,
                format!(
                    "compacted result context is larger than expected (≈{} tokens; warning threshold {}); continuing compaction",
                    result_estimate.tokens, result_context_max_tokens
                ),
            );
        }

        let retained_user_segments = self
            .user_segments
            .iter()
            .skip(self.user_segments.len().saturating_sub(retained_user_msgs))
            .cloned()
            .collect::<Vec<_>>();

        // Build the SegmentStart entry for the new compacted segment.
        // Inherits the source Segment's session_id so the compacted
        // lineage stays grouped under the same Session. Atomically
        // rotate: create on disk, swap location, reset the broadcast
        // sink so existing subscribers see the new `SegmentStart
        // { compacted_from }` and reset their view.
        // The Compactor has produced and validated its output. Retire it before
        // activation so no internal process or registry authority survives into
        // the replacement's live lifetime. Cleanup failures retain this future
        // (and Busy state) until a retry succeeds.
        self.release_compaction_service(lifecycle).await?;

        self.set_compaction_progress(Some(InFlightCompaction {
            phase: CompactionPhase::Committing,
            started_at_ms: lifecycle.started_at_ms,
            trigger,
        }));

        // Activation lock order is append barrier -> allocation table -> metadata.
        // From this point through live publication, every accepted Submit/Notify
        // either completed on the source Segment or waits to observe the fully
        // activated replacement.
        let segment_state = Arc::clone(&self.segment_state);
        let _append_guard = segment_state
            .append_lock
            .lock()
            .expect("segment append lock poisoned");
        segment_state.ensure_append_allowed()?;

        let new_segment_id = session_store::new_segment_id();
        let old_loc = segment_state.location();
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
        let mut initial_entries = vec![
            entry.clone(),
            LogEntry::InputSegmentsCheckpoint {
                ts: segment_log::now_millis(),
                user_segments: retained_user_segments.clone(),
            },
        ];
        if let Some(checkpoint) =
            active_run_checkpoint_entry(w.active_run_turn_count(), source_turn_count)
        {
            initial_entries.push(checkpoint);
        }
        initial_entries.push(self.protected_input_checkpoint());
        initial_entries.push(LogEntry::Extension {
            ts: segment_log::now_millis(),
            domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.to_owned(),
            payload: serde_json::to_value(
                &*self
                    .session
                    .pending_activations
                    .lock()
                    .expect("pending activation state poisoned"),
            )
            .map_err(|error| {
                WorkerError::InvalidState(format!(
                    "serialize pending submissions during compaction: {error}"
                ))
            })?,
        });
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
        let _admission = self.segment_state.mutation_authority.enter()?;
        self.store
            .create_segment(old_loc.session_id, new_segment_id, &initial_entries)?;

        // The name-keyed Worker metadata pointer is the durable commit point for
        // Segment activation. Everything above is staging: a failure leaves the
        // current Segment and live session untouched, while the unreachable
        // replacement may be collected later. The metadata store publishes the
        // complete record with an atomic replace, so restore observes either the
        // old Segment or this fully-written replacement, never a partial switch.
        let new_location = SegmentLocation {
            session_id: old_loc.session_id,
            segment_id: new_segment_id,
        };
        // The machine-wide allocation lock is acquired only after staging, but
        // before the durable commit point. Restore admission therefore cannot
        // race the metadata CAS and allocation hand-off.
        let allocation_activation = self
            .scope_allocation
            .as_ref()
            .map(|allocation| {
                allocation.begin_segment_activation(old_loc.segment_id, new_segment_id)
            })
            .transpose()?;
        self.compare_and_swap_worker_metadata_segment(old_loc, new_location)?;
        if let Some(allocation_activation) = allocation_activation {
            if let Err(error) = allocation_activation.commit() {
                let (source, authority) = error.into_parts();
                // Metadata already names the complete replacement. Do not allow
                // the old in-memory writer to accept another byte while durable
                // authorities disagree; retain the allocation lock so restore
                // admission cannot register a competing writer.
                self.fail_closed_segment_activation(authority);
                return Err(WorkerError::SegmentActivationIncomplete { source });
            }
        }

        // All live mutations after the durable commit are infallible and happen
        // before the replacement SegmentStart is broadcast. This keeps the
        // append destination, Session projection, Engine cache identity, and
        // reconnect snapshot on one side of the same activation boundary.
        self.segment_state.set_location(new_location);
        self.segment_state
            .set_entries_written(initial_entries.len());
        self.user_segments = retained_user_segments;
        self.session.replace_history(compacted_history_entries);
        let worker = self.engine.as_mut().unwrap();
        worker.set_cache_anchor(Some(0));
        worker.set_cache_key(Some(new_segment_id.to_string()));
        self.usage_history
            .lock()
            .expect("usage_history poisoned")
            .clear();

        // Broadcast only after every live authority points at the committed
        // replacement. Runtime-owned extensions stay visible and restorable.
        self.sink
            .reset_with_initial_entries(initial_entries.clone());

        // Compaction-introduced system messages are part of the new
        // SegmentStart's history (broadcast above) — clients derive
        // their blocks from `SegmentStart.history`. No per-item
        // broadcast is required.
        let _ = &compact_introduced_system_messages;
        lifecycle.state = CompactionLifecycleState::Done;
        lifecycle.ended_at_ms = Some(segment_log::now_millis());
        Ok((
            new_segment_id,
            summary_text,
            CompactSuccessStats {
                retained_items: retained_item_count,
                summarized_items: summarized_item_count,
                retained_tokens: retained_estimate.tokens,
                retained_tokens_source: retained_estimate.source,
                overview_tokens: summary_input.overview_tokens,
                summary_tokens,
                auto_read_tokens,
                result_context_tokens: result_estimate.tokens,
                result_context_source: result_estimate.source,
                usage: compactor_usage,
                requests: compactor_requests,
                turns: compactor_turns,
                tool_calls: compactor_tool_calls,
            },
        ))
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubjektivSessionAttributionLifecycle {
    NewSession,
    RestoredSession,
}

fn incomplete_subjektiv_attribution_state(
    session_id: SessionId,
    create_if_missing: bool,
    diagnostic: String,
) -> SubjektivSessionAttributionState {
    if create_if_missing {
        SubjektivSessionAttributionState::OutcomeUnknown {
            session_id,
            diagnostic,
        }
    } else {
        SubjektivSessionAttributionState::LegacyUnknown {
            session_id,
            diagnostic,
        }
    }
}

fn incomplete_subjektiv_attribution_label(create_if_missing: bool) -> &'static str {
    if create_if_missing {
        "outcome_unknown"
    } else {
        "legacy_unknown"
    }
}

fn bounded_attribution_diagnostic(mut diagnostic: String) -> String {
    const MAX_CHARS: usize = 1024;
    if diagnostic.chars().count() > MAX_CHARS {
        diagnostic = diagnostic.chars().take(MAX_CHARS).collect();
        diagnostic.push('…');
    }
    diagnostic
}

fn worker_language(cfg: &manifest::EngineManifest) -> &str {
    let language = cfg.language.trim();
    if language.is_empty() {
        manifest::defaults::WORKER_LANGUAGE
    } else {
        language
    }
}

impl<C: LlmClient + 'static, St: Store + WorkerMetadataStore> Worker<C, St> {
    /// Revalidate current body authority before Controller initialization. Historical
    /// attribution is not a publication grant, and denial must not rewrite it.
    /// This verification never creates attribution or fetches startup Journal context.
    pub(crate) fn validate_subjektiv_body_publication(&self) -> Result<(), WorkerError> {
        if self
            .backend_job()
            .is_some_and(crate::BackendJobExecutionBinding::has_subjektiv_job_grant)
        {
            return Ok(());
        }
        let Some(metadata) = self.store.read_by_name(&self.manifest.worker.name)? else {
            return Ok(());
        };
        let Some(attribution) = metadata.subjektiv_session_attribution.as_ref() else {
            return Ok(());
        };
        let denied = |message: String| WorkerError::SubjektivSessionAttribution {
            state: "publication_denied",
            message: bounded_attribution_diagnostic(message),
        };
        let session_id = self.session_id();
        if metadata.active.as_ref().map(|active| active.session_id) != Some(session_id)
            || attribution.session_id() != session_id
        {
            return Err(denied(
                "retained body attribution does not match the constructed Session".into(),
            ));
        }
        let SubjektivSessionAttributionState::Confirmed { subject_id, .. } = attribution else {
            return Err(denied(
                "body publication requires separately confirmed Session attribution".into(),
            ));
        };
        let connection = self
            .subjektiv_host_connection()
            .map_err(|error| denied(error.to_string()))?
            .ok_or_else(|| {
                denied("body publication requires authenticated Host authority".into())
            })?;
        let output = connection
            .host
            .record_session(&connection.context, false)
            .map_err(|error| denied(error.to_string()))?;
        if output.session_id != session_id.to_string() || output.subject_id != *subject_id {
            return Err(denied(
                "Host publication authorization does not match retained Session and Subject".into(),
            ));
        }
        Ok(())
    }
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
        Self::from_manifest_with_context_and_model_client_for_session(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            model_client,
            None,
        )
        .await
    }

    async fn from_manifest_with_context_and_model_client_for_session(
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
        existing_session_id: Option<SessionId>,
    ) -> Result<Self, WorkerError> {
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
        // registration can record them from the start. Pending lifecycle
        // recovery reuses its already persisted Session identity.
        let feature_installation = if existing_session_id.is_some() {
            crate::feature::FeatureInstallation::Restore
        } else {
            crate::feature::FeatureInstallation::Activate
        };
        let session_id = existing_session_id.unwrap_or_else(session_store::new_session_id);
        let segment_id = session_store::new_segment_id();

        // Register this Worker in the machine-wide worker-allocation
        // before building anything else, so a spawn that conflicts on
        // scope fails fast.
        let scope_allocation = worker_allocation::install_top_level(
            manifest.worker.name.clone(),
            std::process::id(),
            PathBuf::new(), // Reserved legacy allocation field; no Worker endpoint exists.
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
        let worker_metadata_segment_cas = Some(worker_metadata_segment_cas_for_store(&store));
        let scope = SharedScope::new(common.scope);
        let workdir_sessions =
            workdir_sessions_from_authority(&common.filesystem_authority, &scope)?;

        let worker = Self {
            backend_job: std::sync::OnceLock::new(),
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer,
            persist_restored_manifest: existing_session_id.is_some(),
            worker_metadata_segment_cas,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority: common.filesystem_authority,
            workdir_sessions,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            hook_registry: None,
            feature_background_tasks: FeatureBackgroundTaskRegistry::default(),
            startup_feature_cleanup: None,
            feature_invocations: FeatureInvocationRegistry::default(),
            manifest_lifecycle_features_enabled: true,
            interceptor_installed: false,
            compaction_service_budget: None,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            plugin_tools: None,
            system_prompt_template: common.system_prompt_template,
            feature_instructions: common.feature_instructions,
            alerter: None,
            working_event_tx: None,
            internal_worker_registry: None,
            pending_compaction_cleanup: Mutex::new(None),
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            notification_coalesce_delay: DEFAULT_NOTIFICATION_COALESCE_DELAY,
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            failed_activation_authority: Mutex::new(None),
            scope_allocation: Some(scope_allocation),
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            feature_resident_summary_source: None,
            session_context_source: None,
            protected_inputs: Arc::default(),
            prepared_initial_seed: None,
            prepared_initial_prompt_completion: None,
            subjektiv_host: None,
            feature_installation,
            restore_feature_lifecycle_pending: false,
            feature_system_prompt_override: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
            notification_committer: None,
        };
        worker.finish_pending_construction()
    }

    fn finish_pending_construction(mut self) -> Result<Self, WorkerError> {
        self.apply_permissions_from_manifest();
        self.apply_prune_from_manifest();
        self.write_worker_metadata_pending()
            .map_err(|error| self.construction_failure(error))?;
        Ok(self)
    }

    /// Complete the Host-owned Session-to-Subject attribution boundary before a
    /// runtime-managed Worker is exposed or accepts input. The authoritative
    /// attachment bit and lifecycle kind come from the Runtime create/restore
    /// request; names, Profiles, and current singleton ownership are never used
    /// as historical identity.
    ///
    /// New Sessions persist `OutcomeUnknown` before create-if-missing network I/O.
    /// A restore retries creation only for that durable state (or `Failed`). An
    /// unmarked legacy restore is verification-only and persists `LegacyUnknown`,
    /// so current ownership can never invent historical attribution.
    pub fn finalize_subjektiv_session_attribution(
        &self,
        subjektiv_attached: bool,
        lifecycle: SubjektivSessionAttributionLifecycle,
    ) -> Result<(), WorkerError> {
        let _admission = self.segment_state.mutation_authority.enter()?;
        let worker_name = self.manifest.worker.name.as_str();
        let metadata = self.store.read_by_name(worker_name)?.ok_or_else(|| {
            WorkerError::SubjektivSessionAttribution {
                state: "metadata_missing",
                message: "Worker metadata disappeared before Session attribution".to_string(),
            }
        })?;
        let active =
            metadata
                .active
                .as_ref()
                .ok_or_else(|| WorkerError::SubjektivSessionAttribution {
                    state: "session_missing",
                    message: "Worker metadata has no active Session attribution target".to_string(),
                })?;
        let session_id = self.session_id();
        if active.session_id != session_id {
            return Err(WorkerError::SubjektivSessionAttribution {
                state: "session_mismatch",
                message: format!(
                    "Worker metadata Session {} does not match constructed Session {session_id}",
                    active.session_id
                ),
            });
        }

        if self
            .backend_job()
            .is_some_and(crate::BackendJobExecutionBinding::has_subjektiv_job_grant)
        {
            if metadata.subjektiv_session_attribution.is_some() {
                return Err(WorkerError::SubjektivSessionAttribution {
                    state: "job_attribution_forbidden",
                    message: "Subject maintenance Job execution must not carry subject-body Session attribution".into(),
                });
            }
            return Ok(());
        }
        if !subjektiv_attached {
            if metadata.subjektiv_session_attribution.is_some() {
                return Err(WorkerError::SubjektivSessionAttribution {
                    state: "attachment_missing",
                    message:
                        "persisted Subjektiv Session attribution state requires an authorized attachment"
                            .to_string(),
                });
            }
            return Ok(());
        }

        let create_if_missing = match metadata.subjektiv_session_attribution.as_ref() {
            Some(state) => {
                if state.session_id() != session_id {
                    return Err(WorkerError::SubjektivSessionAttribution {
                        state: "session_mismatch",
                        message: format!(
                            "persisted Subjektiv attribution targets Session {}, not {session_id}",
                            state.session_id()
                        ),
                    });
                }
                match state {
                    SubjektivSessionAttributionState::Confirmed { subject_id, .. } => {
                        return self
                            .retain_journal_source_before_publication(session_id, subject_id);
                    }
                    SubjektivSessionAttributionState::OutcomeUnknown { .. }
                    | SubjektivSessionAttributionState::Failed { .. } => true,
                    SubjektivSessionAttributionState::LegacyUnknown { .. } => false,
                }
            }
            None => matches!(lifecycle, SubjektivSessionAttributionLifecycle::NewSession),
        };
        let initial_diagnostic = if create_if_missing {
            "attribution creation started; completion is not yet confirmed"
        } else {
            "legacy Session has no local attribution evidence; immutable Backend verification started"
        };
        let pending_attribution = incomplete_subjektiv_attribution_state(
            session_id,
            create_if_missing,
            initial_diagnostic.to_string(),
        );
        self.persist_subjektiv_session_attribution(
            metadata.subjektiv_session_attribution.as_ref(),
            pending_attribution.clone(),
        )?;
        let output = if self.subjektiv_host.is_some() {
            let connection = self
                .subjektiv_host_connection()
                .map_err(|error| WorkerError::SubjektivSessionAttribution {
                    state: "failed",
                    message: error.to_string(),
                })?
                .ok_or_else(|| WorkerError::SubjektivSessionAttribution {
                    state: "failed",
                    message: "Session attribution requires enabled explicit Host capability".into(),
                })?;
            match connection
                .host
                .record_session(&connection.context, create_if_missing)
            {
                Ok(output) => output,
                Err(error) => {
                    let message = bounded_attribution_diagnostic(error.to_string());
                    let rejected =
                        !matches!(error, crate::subjektiv::SubjektivHostError::Unavailable(_));
                    let state = if create_if_missing && rejected {
                        SubjektivSessionAttributionState::Failed {
                            session_id,
                            diagnostic: message.clone(),
                        }
                    } else {
                        incomplete_subjektiv_attribution_state(
                            session_id,
                            create_if_missing,
                            message.clone(),
                        )
                    };
                    self.persist_subjektiv_session_attribution(Some(&pending_attribution), state)?;
                    return Err(WorkerError::SubjektivSessionAttribution {
                        state: if create_if_missing && rejected {
                            "failed"
                        } else {
                            incomplete_subjektiv_attribution_label(create_if_missing)
                        },
                        message,
                    });
                }
            }
        } else {
            let response = match self.workspace_client().execute_server_operation(
                WorkspaceServerOperation::SubjektivRecordSession(
                    server_api::SubjektivRecordSessionRequest {
                        session_id: session_id.to_string(),
                        create_if_missing,
                    },
                ),
            ) {
                Ok(response) => response,
                Err(error) => {
                    let message = bounded_attribution_diagnostic(format!(
                        "Session attribution transport outcome is unknown: {error}"
                    ));
                    self.persist_subjektiv_session_attribution(
                        Some(&pending_attribution),
                        incomplete_subjektiv_attribution_state(
                            session_id,
                            create_if_missing,
                            message.clone(),
                        ),
                    )?;
                    return Err(WorkerError::SubjektivSessionAttribution {
                        state: incomplete_subjektiv_attribution_label(create_if_missing),
                        message,
                    });
                }
            };
            if !response.is_success() {
                let message = bounded_attribution_diagnostic(format!(
                    "Session attribution was rejected with HTTP {}: {}",
                    response.status, response.body
                ));
                let state = if create_if_missing {
                    SubjektivSessionAttributionState::Failed {
                        session_id,
                        diagnostic: message.clone(),
                    }
                } else {
                    SubjektivSessionAttributionState::LegacyUnknown {
                        session_id,
                        diagnostic: message.clone(),
                    }
                };
                self.persist_subjektiv_session_attribution(Some(&pending_attribution), state)?;
                return Err(WorkerError::SubjektivSessionAttribution {
                    state: if create_if_missing {
                        "failed"
                    } else {
                        "legacy_unknown"
                    },
                    message,
                });
            }
            let output: server_api::SubjektivRecordSessionResponse =
                match serde_json::from_str(&response.body) {
                    Ok(output) => output,
                    Err(error) => {
                        let message = bounded_attribution_diagnostic(format!(
                            "Session attribution response outcome is unknown: {error}"
                        ));
                        self.persist_subjektiv_session_attribution(
                            Some(&pending_attribution),
                            incomplete_subjektiv_attribution_state(
                                session_id,
                                create_if_missing,
                                message.clone(),
                            ),
                        )?;
                        return Err(WorkerError::SubjektivSessionAttribution {
                            state: incomplete_subjektiv_attribution_label(create_if_missing),
                            message,
                        });
                    }
                };
            output
        };
        if output.session_id != session_id.to_string()
            || output.subject_id.trim().is_empty()
            || output.subject_id.chars().any(char::is_control)
        {
            let message =
                "Session attribution response did not confirm the requested immutable identity"
                    .to_string();
            self.persist_subjektiv_session_attribution(
                Some(&pending_attribution),
                incomplete_subjektiv_attribution_state(
                    session_id,
                    create_if_missing,
                    message.clone(),
                ),
            )?;
            return Err(WorkerError::SubjektivSessionAttribution {
                state: incomplete_subjektiv_attribution_label(create_if_missing),
                message,
            });
        }
        self.persist_subjektiv_session_attribution(
            Some(&pending_attribution),
            SubjektivSessionAttributionState::Confirmed {
                session_id,
                subject_id: output.subject_id.clone(),
            },
        )?;
        self.retain_journal_source_before_publication(session_id, &output.subject_id)
    }

    fn retain_journal_source_before_publication(
        &self,
        session_id: SessionId,
        subject_id: &str,
    ) -> Result<(), WorkerError> {
        let Some(connection) = self.subjektiv_host_connection().map_err(|error| {
            WorkerError::SubjektivSessionAttribution {
                state: "retention_unconfirmed",
                message: error.to_string(),
            }
        })?
        else {
            return Ok(());
        };
        if !connection.host.journal_recording_granted() {
            return Ok(());
        }
        let pin = session_store::retention::SessionRetentionPin(
            match self.workspace_client().workspace_id() {
                Some(workspace) => subjektiv::journal::source_retention_pin(
                    workspace,
                    subject_id,
                    &session_id.to_string(),
                ),
                None => session_id,
            },
        );
        // Confirmation can be replayed after response loss. Persist this intent before
        // exposing the Worker to input, not after the first RunCommitted notification.
        // It does not claim that an original Session has materialized yet.
        self.store
            .retain_session_before_publication(session_id, pin)?;
        Ok(())
    }

    fn persist_subjektiv_session_attribution(
        &self,
        expected: Option<&SubjektivSessionAttributionState>,
        state: SubjektivSessionAttributionState,
    ) -> Result<(), WorkerError> {
        crate::subjektiv_metadata::compare_and_swap_attribution(
            &self.store,
            &self.manifest.worker.name,
            self.session_id(),
            expected,
            state,
        )
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
        let workdir_sessions =
            workdir_sessions_from_authority(&common.filesystem_authority, &scope)?;
        let mut worker = Self {
            backend_job: std::sync::OnceLock::new(),
            manifest,
            engine: Some(engine),
            session: WorkerSession::new(session_id, Vec::new()),
            last_run_interrupted: false,
            store,
            worker_metadata_writer: None,
            persist_restored_manifest: false,
            worker_metadata_segment_cas: None,
            segment_state: SegmentState::new(session_id, segment_id, 0),
            filesystem_authority: common.filesystem_authority,
            workdir_sessions,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(None)),
            flow_feature_enabled: false,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            hook_registry: None,
            feature_background_tasks: FeatureBackgroundTaskRegistry::default(),
            startup_feature_cleanup: None,
            feature_invocations: FeatureInvocationRegistry::default(),
            manifest_lifecycle_features_enabled: false,
            interceptor_installed: false,
            compaction_service_budget: None,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(Vec::new())),
            tracker: None,
            task_feature: TaskFeature::new(),
            worker_observation_provider: None,
            plugin_tools: None,
            system_prompt_template: common.system_prompt_template,
            feature_instructions: common.feature_instructions,
            alerter: None,
            working_event_tx: None,
            internal_worker_registry: None,
            pending_compaction_cleanup: Mutex::new(None),
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            notification_coalesce_delay: DEFAULT_NOTIFICATION_COALESCE_DELAY,
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            failed_activation_authority: Mutex::new(None),
            scope_allocation: None,
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            feature_resident_summary_source: None,
            session_context_source: None,
            protected_inputs: Arc::default(),
            prepared_initial_seed: None,
            prepared_initial_prompt_completion: None,
            subjektiv_host: None,
            feature_installation: crate::feature::FeatureInstallation::Activate,
            restore_feature_lifecycle_pending: false,
            feature_system_prompt_override: None,
            user_segments: Vec::new(),
            sink: SegmentLogSink::new(),
            history_persistence_wired: false,
            log_writer: None,
            notification_committer: None,
        };
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
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
        Self::prepare_restored_from_worker_metadata_with_context_and_model_client(
            worker_name,
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            None,
        )
        .await?
        .activate()
        .await
    }

    /// Prepare only a materialized saved Worker. Pending Session creation and
    /// Subject attribution repair must be performed explicitly by the host
    /// before entering this read-only path.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_restored_from_worker_metadata_with_context_and_model_client(
        worker_name: &str,
        fallback: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
    ) -> Result<PreparedRestoredWorker<St>, WorkerError> {
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
        let active =
            metadata
                .active
                .as_ref()
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
            metadata.resolved_manifest_snapshot.clone(),
            fallback,
        )?;
        if manifest.worker.name != worker_name {
            return Err(WorkerError::InvalidState(
                "restore manifest Worker name mismatch".into(),
            ));
        }
        Self::prepare_restored_from_manifest_snapshot(
            active.session_id,
            segment_id,
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            model_client,
            Some(metadata),
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
        Self::restore_pending_from_worker_metadata_with_context_and_model_client(
            worker_name,
            fallback,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            None,
        )
        .await
    }

    pub(crate) async fn restore_pending_from_worker_metadata_with_context_and_model_client(
        worker_name: &str,
        fallback: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
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
        // A seed may have committed while the separate active-pointer write or
        // its response failed. Pending metadata names the Session; its sole
        // committed segment is the receipt. Never refetch/reseed that Session.
        let segment_id = match active.segment_id {
            Some(id) => Some(id),
            None => {
                let segments = store.list_segments(active.session_id)?;
                match segments.as_slice() {
                    [] => None,
                    [id] => {
                        let seed = store.read_all(active.session_id, *id)?;
                        if seed.is_empty() { None } else { Some(*id) }
                    }
                    _ => {
                        return Err(WorkerError::InvalidState(
                            "ambiguous pending Session segments".into(),
                        ));
                    }
                }
            }
        };
        if let Some(segment_id) = segment_id {
            return Self::restore_from_manifest_with_context_and_model_client(
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
                model_client,
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
        Self::from_manifest_with_context_and_model_client_for_session(
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            model_client,
            Some(active.session_id),
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
        Self::restore_from_manifest_with_context_and_model_client(
            session_id,
            segment_id,
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            None,
        )
        .await
    }

    pub(crate) async fn restore_from_manifest_with_context_and_model_client(
        session_id: SessionId,
        segment_id: SegmentId,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
    ) -> Result<Self, WorkerError> {
        let metadata = store.read_by_name(&manifest.worker.name)?;
        Self::prepare_restored_from_manifest_snapshot(
            session_id,
            segment_id,
            manifest,
            store,
            loader,
            workspace_context,
            filesystem_authority,
            model_client,
            metadata,
        )
        .await?
        .activate()
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_restored_from_manifest_snapshot(
        session_id: SessionId,
        segment_id: SegmentId,
        manifest: WorkerManifest,
        store: St,
        loader: PromptCatalogSource,
        workspace_context: WorkerWorkspaceContext,
        filesystem_authority: WorkerFilesystemAuthority,
        model_client: Option<Box<dyn LlmClient>>,
        metadata: Option<WorkerMetadata>,
    ) -> Result<PreparedRestoredWorker<St>, WorkerError> {
        // Read raw entries once so we can both reconstruct state and
        // seed the broadcast sink's mirror with the same prefix that
        // sits on disk.
        let raw_entries = store.read_all(session_id, segment_id)?;
        let entries_snapshot = restore_snapshot_value(&raw_entries)?;
        let manifest_snapshot = restore_snapshot_value(&manifest)?;
        let state = session_store::collect_state(&raw_entries);
        if state.entries_count == 0 {
            return Err(WorkerError::SegmentEmpty { segment_id });
        }
        if state.system_prompt.is_none() {
            return Err(WorkerError::SegmentSystemPromptMissing { segment_id });
        }
        let restored_history_entries =
            restore_history_entries(session_id, segment_id, &raw_entries).map_err(|error| {
                WorkerError::InvalidState(format!("restore typed Worker session history: {error}"))
            })?;
        let protected_inputs = ProtectedInputProvenance::restore(
            &raw_entries,
            &restored_history_entries,
            metadata.as_ref(),
        )?;
        let flow_runtime_state = restored_flow_runtime_state(&state.extensions)?;
        let mirror_entries: Vec<LogEntry> = raw_entries.clone();
        let scope_config = effective_restore_scope_config(&store, &manifest)?;

        let common = prepare_worker_common_with_context_and_model_client(
            &manifest,
            &loader,
            /* parse_template */ false,
            workspace_context,
            filesystem_authority,
            scope_config,
            model_client,
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

        let task_feature = TaskFeature::from_history(&state.history);
        let worker_metadata_writer = Some(worker_metadata_writer_for_store(&store));
        let worker_metadata_segment_cas = Some(worker_metadata_segment_cas_for_store(&store));
        let scope = SharedScope::new(common.scope);
        let workdir_sessions =
            workdir_sessions_from_authority(&common.filesystem_authority, &scope)?;

        let mut worker = Self {
            backend_job: std::sync::OnceLock::new(),
            manifest,
            engine: Some(worker),
            session: WorkerSession::new(session_id, restored_history_entries),
            last_run_interrupted: state.last_run_interrupted,
            store,
            worker_metadata_writer,
            persist_restored_manifest: true,
            worker_metadata_segment_cas,
            segment_state: SegmentState::new(session_id, segment_id, state.entries_count),
            filesystem_authority: common.filesystem_authority,
            workdir_sessions,
            workspace_context: common.workspace_context,
            flow_runtime_state: Arc::new(Mutex::new(flow_runtime_state)),
            flow_feature_enabled: false,
            scope,
            hook_builder: HookRegistryBuilder::new(),
            hook_registry: None,
            feature_background_tasks: FeatureBackgroundTaskRegistry::default(),
            startup_feature_cleanup: None,
            feature_invocations: FeatureInvocationRegistry::default(),
            manifest_lifecycle_features_enabled: true,
            interceptor_installed: false,
            compaction_service_budget: None,
            compact_state: None,
            usage_tracker: Arc::new(UsageTracker::new()),
            metrics_tracker: Arc::new(crate::compact::metrics_tracker::MetricsTracker::new()),
            usage_history: Arc::new(Mutex::new(state.usage_history)),
            tracker: None,
            task_feature,
            worker_observation_provider: None,
            plugin_tools: None,
            // Restore replays the saved system_prompt verbatim — no
            // template re-render on resume.
            system_prompt_template: None,
            feature_instructions: common.feature_instructions,
            alerter: None,
            working_event_tx: None,
            internal_worker_registry: None,
            pending_compaction_cleanup: Mutex::new(None),
            in_flight: None,
            ai_activity_counter: Arc::new(AtomicUsize::new(0)),
            pending_notifies: NotifyBuffer::new(),
            notification_coalesce_delay: DEFAULT_NOTIFICATION_COALESCE_DELAY,
            pending_attachments: Arc::new(Mutex::new(Vec::<SystemItem>::new())),
            pending_committed_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            failed_activation_authority: Mutex::new(None),
            scope_allocation: None,
            runtime_ticket_role: None,
            prompts: common.prompts,
            inject_resident_summary: true,
            feature_resident_summary_source: None,
            session_context_source: None,
            protected_inputs: Arc::new(Mutex::new(protected_inputs)),
            prepared_initial_seed: None,
            prepared_initial_prompt_completion: None,
            subjektiv_host: None,
            feature_installation: crate::feature::FeatureInstallation::Restore,
            restore_feature_lifecycle_pending: true,
            feature_system_prompt_override: None,
            user_segments: state.user_segments,
            // Seed the mirror with the entries we just replayed so a
            // late-attaching client sees the full prefix without an
            // extra round trip.
            sink: SegmentLogSink::with_initial(mirror_entries),
            history_persistence_wired: false,
            log_writer: None,
            notification_committer: None,
        };
        worker
            .session
            .restore_pending_activations(&state.extensions);
        worker.apply_permissions_from_manifest();
        worker.apply_prune_from_manifest();
        let mut candidate = PreparedRestoredWorker {
            worker: Some(worker),
            closing: false,
            activation: Some(RestoredWorkerActivation {
                metadata,
                entries: entries_snapshot,
                manifest: manifest_snapshot,
            }),
        };
        if let Err(error) = candidate.revalidate(candidate.worker()) {
            let worker = candidate.worker.take().expect("candidate just constructed");
            return Err(worker.construction_failure(error));
        }
        Ok(candidate)
    }

    async fn finish_restored_construction(mut self) -> Result<Self, WorkerError> {
        self.finish_restored_construction_in_place(true).await?;
        Ok(self)
    }

    /// Apply the same restore effects while an initialized Controller retains
    /// ownership. Errors retain cleanup handles, but do not drop this Worker.
    async fn finish_restored_construction_in_place(
        &mut self,
        reclaim_delegations: bool,
    ) -> Result<(), WorkerError> {
        self.pending_submission_handle()
            .reconcile_uploaded_file_pins()
            .map_err(|error| self.construction_failure(error.into()))?;
        self.apply_permissions_from_manifest();
        self.apply_prune_from_manifest();
        // Persist inferred legacy retention before replacing its saved manifest.
        // A failed append leaves that older evidence intact for the next restore.
        let checkpoint = self.protected_input_checkpoint();
        let loc = self.segment_state.location();
        let entries = self.store.read_all(loc.session_id, loc.segment_id)?;
        let LogEntry::Extension {
            payload: wanted, ..
        } = &checkpoint
        else {
            unreachable!()
        };
        if !entries.iter().any(|entry| matches!(entry,
            LogEntry::Extension { domain, payload, .. } if domain == PROTECTED_INPUTS_DOMAIN && payload == wanted
        )) {
            self.log_writer_handle().append_entry(checkpoint)?;
        }
        self.write_worker_metadata_active(SegmentLocation {
            session_id: self.session_id(),
            segment_id: self.segment_id(),
        })
        .map_err(|error| self.construction_failure(error))?;
        if reclaim_delegations {
            self.reconcile_restored_delegations()
                .await
                .map_err(|error| self.construction_failure(error))?;
        }
        Ok(())
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
pub fn apply_worker_manifest<C: LlmClient + 'static, A: Send + Sync>(
    worker: &mut Engine<C, Mutable, A>,
    wm: &manifest::EngineManifest,
) {
    worker.set_request_config(request_config_from_engine_manifest(wm));
    worker.set_max_turns(wm.max_turns.map(|n| n.get()));
    worker.set_tool_call_dispatch_mode(match wm.tool_call_dispatch {
        manifest::ToolCallDispatchMode::AfterResponse => agen::ToolCallDispatchMode::AfterResponse,
        manifest::ToolCallDispatchMode::OnToolCallComplete => {
            agen::ToolCallDispatchMode::OnToolCallComplete
        }
    });
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
        metadata.resolved_manifest_snapshot =
            manifest::write_persisted_worker_manifest_snapshot(manifest).ok();
    }
    metadata
}

fn should_persist_resolved_manifest_snapshot(manifest: &WorkerManifest) -> bool {
    manifest.requires_persisted_execution_snapshot()
}

fn restore_manifest_from_worker_metadata_snapshot(
    worker_name: &str,
    snapshot: Option<serde_json::Value>,
    fallback: WorkerManifest,
) -> Result<WorkerManifest, WorkerError> {
    match snapshot {
        Some(snapshot) => {
            manifest::read_persisted_worker_manifest_snapshot(snapshot).map_err(|source| {
                WorkerError::WorkerMetadataManifestSnapshot {
                    worker_name: worker_name.to_string(),
                    source,
                }
            })
        }
        None => Ok(fallback),
    }
}

fn run_interruption_reason_error_code(reason: &RunInterruptionReason) -> ErrorCode {
    match reason {
        RunInterruptionReason::ContextWindowExceeded
        | RunInterruptionReason::Unexpected(EngineError::Client(_)) => ErrorCode::ProviderError,
        RunInterruptionReason::Unexpected(EngineError::Tool(_)) => ErrorCode::ToolError,
        RunInterruptionReason::LimitReached
        | RunInterruptionReason::Cancelled
        | RunInterruptionReason::Unexpected(
            EngineError::Aborted(_)
            | EngineError::Cancelled
            | EngineError::PauseRequested
            | EngineError::ConfigWarnings(_)
            | EngineError::HistoryAppend(_)
            | EngineError::Interceptor(_)
            | EngineError::ToolAttemptFence(_),
        ) => ErrorCode::Internal,
    }
}

fn run_interruption_reason_message(reason: &RunInterruptionReason) -> String {
    match reason {
        RunInterruptionReason::LimitReached => "engine turn limit reached".to_string(),
        RunInterruptionReason::ContextWindowExceeded => "model context window reached".to_string(),
        RunInterruptionReason::Cancelled => "engine run cancelled".to_string(),
        RunInterruptionReason::Unexpected(error) => format!("unexpected engine failure: {error}"),
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
    /// The logical Run was intentionally cancelled after observable work.
    Cancelled,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactionImageReference {
    target_workdir: Option<String>,
    path: String,
}

#[derive(serde::Deserialize)]
struct ViewImageCallArguments {
    #[serde(default)]
    target_workdir: Option<String>,
    path: String,
}

/// Remove binary bodies from the immutable capture exposed to the compactor.
fn project_compactor_capture_attachments<A>(entries: &mut [HistoryEntry<A>]) -> usize {
    let mut projected = 0;
    for entry in entries {
        if let Item::ToolResult { attachments, .. } = &mut entry.item
            && !attachments.is_empty()
        {
            attachments.clear();
            projected += 1;
        }
    }
    projected
}

/// Remove every durable binary attachment from the compacted tail and preserve
/// re-readable ViewImage origins as related-file references.
fn project_compaction_attachments<'a>(
    items: impl IntoIterator<Item = &'a mut Item>,
    default_workdir_alias: Option<&str>,
) -> Vec<CompactionImageReference> {
    let items = items.into_iter().collect::<Vec<_>>();
    let view_image_calls = items
        .iter()
        .filter_map(|item| match &**item {
            Item::ToolCall {
                call_id,
                name,
                arguments,
                ..
            } if name == "ViewImage" => serde_json::from_str::<ViewImageCallArguments>(arguments)
                .ok()
                .map(|arguments| {
                    (
                        call_id.clone(),
                        CompactionImageReference {
                            target_workdir: arguments
                                .target_workdir
                                .or_else(|| default_workdir_alias.map(str::to_string)),
                            path: arguments.path,
                        },
                    )
                }),
            _ => None,
        })
        .collect::<HashMap<_, _>>();

    let mut references = Vec::new();
    for item in items {
        let Item::ToolResult {
            call_id,
            attachments,
            ..
        } = item
        else {
            continue;
        };
        if attachments.is_empty() {
            continue;
        }
        attachments.clear();
        if let Some(reference) = view_image_calls.get(call_id)
            && !references.contains(reference)
        {
            references.push(reference.clone());
        }
    }
    references
}

fn build_related_file_message(
    references: &[PathBuf],
    image_references: &[CompactionImageReference],
) -> Option<Item> {
    if references.is_empty() && image_references.is_empty() {
        return None;
    }

    let mut lines = Vec::with_capacity(references.len() + image_references.len());
    for reference in references {
        let path = reference.display().to_string();
        if image_references
            .iter()
            .any(|image| image.target_workdir.is_none() && image.path == path)
        {
            continue;
        }
        lines.push(format!("- {path}"));
    }
    for reference in image_references {
        match reference.target_workdir.as_deref() {
            Some(target_workdir) => lines.push(format!(
                "- {} (image; use ViewImage with target_workdir {:?})",
                reference.path, target_workdir
            )),
            None => lines.push(format!("- {} (image; use ViewImage)", reference.path)),
        }
    }

    Some(Item::system_message(format!(
        "[Related files — contents not included]\n{}\n\
         Use Read for text files and ViewImage for image references when their current contents \
         are needed.",
        lines.join("\n")
    )))
}

fn summary_shrink_prompt(
    summary_tokens: u64,
    summary_max_tokens: u64,
    summary_target_tokens: u64,
) -> String {
    format!(
        "Your `write_summary` output is too large (≈{summary_tokens} tokens; max \
         {summary_max_tokens}). Rewrite it now with `write_summary`, preserving the \
         same five sections. Compress duplicate explanations and old work details \
         first; preserve ongoing goals and user directives from the initial summary \
         and segment, including scope, prohibitions, completion conditions and \
         exception-reporting duties. Apply explicit later changes/retractions and \
         retain uncertainty; do not replace the original request with recovery \
         procedures or recent progress. Target ≈{summary_target_tokens} tokens."
    )
}

/// Select only the Host's typed summary, never a marker in ordinary content.
/// Invalid/duplicate Host summaries are a broken seed contract, not permission
/// to silently omit inherited instructions.
fn initial_compaction_summary(
    entries: &[HistoryEntry<SessionHistoryMetadata>],
) -> Result<Option<String>, WorkerError> {
    let mut summaries = entries.iter().filter(|entry| {
        crate::session_history::compaction_use(&entry.annotation)
            == session_store::InputCompactionUse::OpeningSummary
    });
    let Some(entry) = summaries.next() else {
        return Ok(None);
    };
    if summaries.next().is_some() {
        return Err(WorkerError::InvalidState(
            "multiple Host compaction summaries in active history".into(),
        ));
    }
    if !matches!(
        &entry.item,
        Item::Message {
            role: agen::Role::System,
            ..
        }
    ) {
        return Err(WorkerError::InvalidState(
            "Host compaction summary must be a system message".into(),
        ));
    }
    let body = entry
        .item
        .as_text()
        .and_then(|text| text.strip_prefix(crate::session_history::COMPACTION_SUMMARY_HEADER))
        .ok_or_else(|| {
            WorkerError::InvalidState("Host compaction summary header is missing".into())
        })?;
    Ok(Some(body.to_string()))
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

/// Build mandatory full Host-summary input plus a bounded prefix overview,
/// recently-touched files and current tasks. Overview budgets never apply to
/// the summary; the complete request is checked by the compaction interceptor.
fn build_summary_input(
    items: &[Item],
    initial_summary: Option<&str>,
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
         bounded overview/index, not the full transcript. A separately supplied initial \
         compaction summary is complete, mandatory source material. Preserve ongoing \
         user goals and directives from it and from this segment's user messages; \
         confirm important wording with exploration rather than reconstructing the \
         request from recent progress. Retained tail entries remain verbatim in the \
         next session and are outside this prefix overview. Use tools to inspect \
         current files when deciding auto-read/reference output.\n\n",
        options.summary_target_tokens
    ));
    if let Some(summary) = initial_summary {
        out.push_str("## Initial compaction summary (full text)\n");
        out.push_str(summary);
        out.push_str("\n\n## End initial compaction summary\n\n");
    }
    if !default_refs.is_empty() {
        out.push_str(
            "These files were touched recently in this session. Use `read_file` \
             on text files as needed, then call `mark_read_required` for UTF-8 text \
             whose contents the next session must have. Call `add_reference` for images, \
             binary files, and files it should know about by name only.\n\n## Referenced files\n",
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
             summary, especially active (pending/inprogress) tasks, but not as a \
             replacement for user goals, directives or completion conditions. Do not edit tasks \
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
            Item::AssistantResponseBoundary { .. } => {}
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

fn resolve_rewind_input(
    entries: &[LogEntry],
    entry_id: &str,
) -> Result<(usize, Vec<Segment>), RewindError> {
    // Resolve only actual user submissions in this segment, never seeded
    // ancestor history or a display position inferred from matching text.
    entries.iter().enumerate().find_map(|(index, entry)| {
            let LogEntry::AnnotatedUserInput { history, segments, .. } = entry else { return None; };
            let user = history.iter().find(|entry| matches!(&entry.item,
                session_store::LoggedItem::Message { role: session_store::LoggedRole::User, .. }
            ))?;
            (user.metadata.entry_id.0 == entry_id).then(|| (index, segments.clone()))
        }).ok_or_else(|| RewindError::Invalid(
            "rewind entry is not a user input in the current segment; it may have been removed or belong to another history".into()
        ))
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
        LogEntry::AnnotatedToolResult { .. } => true,
        LogEntry::AnnotatedAssistantItem { entry, .. } => logged_item_is_tool_call(&entry.item),
        _ => false,
    })
}

fn logged_item_is_tool_call(item: &session_store::LoggedItem) -> bool {
    matches!(item, session_store::LoggedItem::ToolCall { .. })
}

#[cfg(test)]
fn preview_segments(segments: &[Segment]) -> String {
    let mut preview = String::new();
    for segment in segments {
        if !preview.is_empty() {
            preview.push(' ');
        }
        match segment {
            Segment::Text { content } => preview.push_str(content.trim()),
            Segment::Paste { content, .. } => preview.push_str(content.trim()),
            Segment::PasteArtifact { artifact } => {
                preview.push_str("[Large paste artifact: ");
                preview.push_str(&artifact.artifact_id);
                preview.push(']');
            }
            Segment::UploadedFile { file } => {
                preview.push_str("[Attached file: ");
                preview.push_str(&file.file_name);
                preview.push(']');
            }
            Segment::FileRef { path } => {
                preview.push('@');
                preview.push_str(path);
            }
            Segment::FeatureInvoke { invocation } => {
                preview.push_str(&invocation.display_input());
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

fn ensure_compaction_internal_turn_succeeded(
    outcome: Option<crate::internal_worker::InternalWorkerTurnOutcome>,
) -> Result<(), WorkerError> {
    use crate::internal_worker::InternalWorkerTurnOutcome;

    match outcome {
        Some(InternalWorkerTurnOutcome::Completed(WorkerRunResult::Finished)) => Ok(()),
        Some(InternalWorkerTurnOutcome::Completed(WorkerRunResult::LimitReached)) => {
            Err(WorkerError::Engine(EngineError::Aborted(
                "compactor Internal Worker reached its turn limit".to_string(),
            )))
        }
        Some(InternalWorkerTurnOutcome::Completed(WorkerRunResult::Interrupted {
            message,
            ..
        })) => Err(WorkerError::Engine(EngineError::Aborted(message))),
        Some(InternalWorkerTurnOutcome::Completed(
            WorkerRunResult::Cancelled | WorkerRunResult::RolledBack,
        )) => Err(WorkerError::CompactCancelled),
        Some(InternalWorkerTurnOutcome::Completed(WorkerRunResult::Paused)) => Err(
            WorkerError::InvalidState("compactor Internal Worker paused unexpectedly".to_string()),
        ),
        Some(InternalWorkerTurnOutcome::Failed { message }) => {
            Err(WorkerError::InvalidState(message))
        }
        None => Err(WorkerError::InvalidState(
            "compactor Internal Worker stopped without a typed turn outcome".to_string(),
        )),
    }
}

fn automatic_compact_block_error(block: AutomaticCompactBlock) -> WorkerError {
    match block {
        AutomaticCompactBlock::Thrash => WorkerError::CompactThrash,
        AutomaticCompactBlock::Failed(category) => WorkerError::AutomaticCompactFailed {
            category: category.as_str(),
        },
        AutomaticCompactBlock::Attempted => WorkerError::AutomaticCompactState(
            "automatic compaction attempt already claimed for this logical run".to_string(),
        ),
        AutomaticCompactBlock::Cancelled => WorkerError::CompactCancelled,
    }
}

fn compact_failure_category(error: &WorkerError) -> CompactFailureCategory {
    match error {
        WorkerError::CompactCancelled => CompactFailureCategory::Cancelled,
        WorkerError::CompactSummaryMissing => CompactFailureCategory::SummaryMissing,
        WorkerError::CompactSummaryTooLarge { .. } => CompactFailureCategory::SummaryTooLarge,
        WorkerError::WorkerStore(_)
        | WorkerError::CompactActiveSegmentChanged
        | WorkerError::SegmentActivationIncomplete { .. } => {
            CompactFailureCategory::ActiveSegmentCommit
        }
        WorkerError::Store(_) | WorkerError::PendingUsagePersistence { .. } => {
            CompactFailureCategory::Storage
        }
        WorkerError::InvalidState(_) | WorkerError::Engine(_) => {
            CompactFailureCategory::InternalWorker
        }
        WorkerError::Provider(_)
        | WorkerError::PromptCatalog(_)
        | WorkerError::FeatureLifecycle(_)
        | WorkerError::FeatureInstall(_) => CompactFailureCategory::Preparation,
        _ => CompactFailureCategory::Other,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerError {
    /// A construction or activation step failed after acquiring resources. Retain
    /// these handles until explicit cleanup succeeds. Consuming constructors drop
    /// the Worker before returning; in-place activation leaves its owner responsible
    /// for dropping it before allocation release.
    #[error("{source}")]
    ConstructionCleanup {
        #[source]
        source: Box<WorkerError>,
        allocation: Option<ScopeAllocationCleanup>,
        sessions: Arc<WorkdirSessionRouter>,
    },

    /// Activation entered canonical pin/metadata/delegation updates. Cleanup
    /// ownership in the source does not establish rollback of those effects.
    #[error("{source}")]
    RestoreActivationEffects {
        #[source]
        source: Box<WorkerError>,
    },

    #[error("restore candidate is closing; only cleanup retry is allowed")]
    RestoreCandidateClosing,

    #[error("restore candidate source changed for Worker {worker_name}: {component}")]
    RestoreCandidateChanged {
        worker_name: String,
        component: &'static str,
    },

    #[error("invalid durable Worker state: {0}")]
    InvalidState(String),

    #[error("feature lifecycle rejected operation: {0}")]
    FeatureLifecycle(String),

    #[error("Subjektiv Session attribution lifecycle is {state}: {message}")]
    SubjektivSessionAttribution {
        state: &'static str,
        message: String,
    },

    #[error("Feature invocation rejected: {0}")]
    FeatureInvocation(String),

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

    #[error(
        "cannot replace the active Segment while {pending_records} usage record(s) await durable persistence"
    )]
    PendingUsagePersistence { pending_records: usize },

    #[error("active Segment changed before compaction commit")]
    CompactActiveSegmentChanged,

    #[error("compaction thrash: context still exceeds threshold immediately after compact")]
    CompactThrash,

    #[error("automatic compaction failed and the provider request remained unsafe: {category}")]
    AutomaticCompactFailed { category: &'static str },

    #[error("invalid automatic compaction state: {0}")]
    AutomaticCompactState(String),

    #[error("compact worker did not produce a summary (write_summary was never called)")]
    CompactSummaryMissing,

    #[error("compaction was cancelled")]
    CompactCancelled,

    /// The logical Run failed, and its terminal entry was already committed and
    /// published through the session-log authority. Controllers must preserve the
    /// error outcome without emitting a second generic error event.
    #[error("{source}")]
    RunFailureAlreadyDelivered {
        #[source]
        source: Box<WorkerError>,
    },

    /// The logical Run failed and its terminal entry could not be committed.
    /// This remains unhandled so the Controller reports the run and persistence
    /// failures together through its non-durable error channel.
    #[error(
        "run failed ({run_failure}) and its terminal record could not be persisted: {persist_error}"
    )]
    RunTerminalPersistence {
        run_failure: Box<WorkerError>,
        persist_error: Box<WorkerError>,
    },

    #[error("compact summary too large: {tokens} tokens exceeds max {max}")]
    CompactSummaryTooLarge { tokens: u64, max: u64 },

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

    #[error(
        "Segment activation could not be completed; the Worker must be restored before accepting more input"
    )]
    SegmentActivationIncomplete {
        #[source]
        source: ScopeLockError,
    },

    #[error("compaction cleanup is pending; retry or shut down the Worker")]
    CompactionCleanupPending {
        #[source]
        source: ScopeLockError,
    },

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

fn workdir_sessions_from_authority(
    authority: &WorkerFilesystemAuthority,
    scope: &SharedScope,
) -> Result<Arc<WorkdirSessionRouter>, WorkerError> {
    let router = Arc::new(WorkdirSessionRouter::new());
    if let Some(local) = authority.as_local() {
        let invalid_cwd = |error: String| WorkerError::InvalidCwd {
            cwd: local.cwd.clone(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, error),
        };
        let relative_cwd = local.cwd.strip_prefix(&local.root).map_err(|error| {
            invalid_cwd(format!(
                "session cwd must be within its Workdir root: {error}"
            ))
        })?;
        let relative_cwd = relative_cwd
            .to_str()
            .ok_or_else(|| invalid_cwd("session cwd must be UTF-8".into()))?;
        let relative_cwd = workdir::WorkdirPath::new(relative_cwd)
            .map_err(|error| invalid_cwd(error.to_string()))?;
        // The provider keeps root-coordinate operations. Only the routed tool
        // view uses cwd; delegation reuses the same root-coordinate broker.
        let session = Arc::new(LocalWorkdirSession::materialized(
            local.root.clone(),
            local.cwd.clone(),
            scope.clone(),
            WorkdirSessionCapabilities::ALL,
        )) as WorkdirSessionHandle;
        router
            .attach_at_cwd(
                WorkdirAttachmentAlias::new("workdir").expect("static alias is valid"),
                session,
                relative_cwd,
            )
            .map_err(|error| invalid_cwd(error.to_string()))?;
    }
    Ok(router)
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
    client: Box<dyn LlmClient>,
    prompts: Arc<ArcSwap<PromptCatalog>>,
    system_prompt_template: Option<SystemPromptTemplate>,
    feature_instructions: Vec<FeatureInstructionDeclaration>,
}

/// Migration-only safety probe for retained process-child metadata.
/// New Internal SubWorkers never publish socket metadata or use this path.
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
        symlink_policy: rule.symlink_policy,
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

        let manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
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
        let manifest = minimal_manifest_for_context_test(&workspace_root, &cwd);
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
                    symlink_policy: Default::default(),
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
                    symlink_policy: Default::default(),
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
            Some(manifest::write_persisted_worker_manifest_snapshot(&saved).unwrap()),
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
    fn workspace_memory_settings_snapshot_is_persisted_through_versioned_adapter() {
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
        manifest.feature.memory.profile.enabled = true;
        manifest
            .feature
            .memory
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-a".to_string(),

                language: "Japanese".to_string(),
            })
            .unwrap();

        let metadata = worker_metadata_for_manifest(&manifest, None, None, None);
        let restored = manifest::read_persisted_worker_manifest_snapshot(
            metadata
                .resolved_manifest_snapshot
                .expect("Memory settings require a resolved manifest snapshot"),
        )
        .unwrap();
        assert_eq!(
            restored.feature.memory.workspace_settings(),
            Some(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-a".to_string(),

                language: "Japanese".to_string(),
            })
        );
    }
}

#[cfg(test)]
mod build_summary_prompt_tests {
    use super::*;

    struct TestSystemPromptContributionSource {
        value: SystemPromptContribution,
        load_count: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl SystemPromptContributionSource for TestSystemPromptContributionSource {
        async fn load(&self) -> SystemPromptContribution {
            self.load_count.fetch_add(1, Ordering::SeqCst);
            self.value.clone()
        }
    }

    fn test_system_prompt_contribution_source(
        value: Option<String>,
    ) -> (Arc<dyn SystemPromptContributionSource>, Arc<AtomicUsize>) {
        let load_count = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(TestSystemPromptContributionSource {
                value: value.map_or(
                    SystemPromptContribution::Unavailable,
                    SystemPromptContribution::Ready,
                ),
                load_count: Arc::clone(&load_count),
            }),
            load_count,
        )
    }

    #[derive(Debug)]
    struct RestoreLegacyResidentClient {
        content: String,
        load_count: Arc<AtomicUsize>,
    }

    impl WorkspaceClient for RestoreLegacyResidentClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "restore-legacy-resident-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if request.path.ends_with("/authorization/check") {
                return Ok(WorkspaceResponse {
                    status: 200,
                    body: serde_json::json!({"scope": {"kind":"workspace", "workspace_id":"workspace-test"}}).to_string(),
                });
            }
            let request: server_api::MemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap_or_default()).unwrap();
            assert!(matches!(
                request.0,
                memory::backend::MemoryBackendOperation::ResidentSummary(_)
            ));
            self.load_count.fetch_add(1, Ordering::SeqCst);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&server_api::MemoryBackendResponse(
                    memory::backend::MemoryBackendHttpResponse::Ok {
                        result: memory::backend::MemoryBackendOperationResult::ResidentSummary(
                            memory::backend::MemoryResidentSummaryOutput {
                                availability:
                                    memory::backend::MemoryResidentSummaryAvailability::Ready,
                                content: Some(self.content.clone()),
                            },
                        ),
                    },
                ))
                .unwrap(),
            })
        }
    }

    fn legacy_resident_client(content: &str) -> (Arc<dyn WorkspaceClient>, Arc<AtomicUsize>) {
        let load_count = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(RestoreLegacyResidentClient {
                content: content.to_string(),
                load_count: Arc::clone(&load_count),
            }),
            load_count,
        )
    }

    #[derive(Debug)]
    struct RestoreSubjectResidentClient {
        availability: memory::backend::MemoryResidentSummaryAvailability,
        content: Option<String>,
        load_count: Arc<AtomicUsize>,
        behavior_md: Option<Arc<Mutex<String>>>,
    }

    impl WorkspaceClient for RestoreSubjectResidentClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "restore-subject-resident-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            if request.path.ends_with("/authorization/bound-check") {
                let input: server_api::authorization::BoundOperationAuthorizationRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                assert_eq!(
                    input.target,
                    server_api::authorization::BoundAuthorizationTarget::Subject {}
                );
                assert!(matches!(
                    input.operation.as_str(),
                    "memory.resident.read" | "memory.resident.supply"
                ));
                return Ok(WorkspaceResponse {
                    status: 200,
                    body: serde_json::json!({"scope": {"kind":"subject", "workspace_id":"workspace-test", "subject_id":"subject-fixture"}}).to_string(),
                });
            }
            let request: server_api::SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap_or_default()).unwrap();
            assert!(matches!(
                request.operation,
                server_api::SubjektivMemoryBackendOperation::ResidentContext(_)
            ));
            self.load_count.fetch_add(1, Ordering::SeqCst);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(
                    &server_api::SubjektivMemoryBackendResponse::ResidentContext(
                        server_api::SubjektivResidentContextOutput {
                            behavior_md: self.behavior_md.as_ref().map_or_else(
                                || "restore behavior".to_string(),
                                |body| body.lock().unwrap().clone(),
                            ),
                            memory_surface: memory::backend::MemoryResidentSummaryOutput {
                                availability: self.availability,
                                content: self.content.clone(),
                            },
                        },
                    ),
                )
                .unwrap(),
            })
        }
    }

    fn subject_resident_source_for_restore(
        manifest: &WorkerManifest,
        availability: memory::backend::MemoryResidentSummaryAvailability,
        content: Option<&str>,
    ) -> (Arc<dyn SystemPromptContributionSource>, Arc<AtomicUsize>) {
        let load_count = Arc::new(AtomicUsize::new(0));
        let client: Arc<dyn WorkspaceClient> = Arc::new(RestoreSubjectResidentClient {
            availability,
            content: content.map(str::to_string),
            load_count: Arc::clone(&load_count),
            behavior_md: None,
        });
        let source = crate::feature::builtin::memory::ordinary_subjektiv_resident_summary_source(
            manifest,
            Some(crate::subjektiv::test_connection(client)),
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
        )
        .unwrap()
        .unwrap();
        (source, load_count)
    }

    fn test_summary_input(items: &[Item]) -> String {
        build_summary_input(
            items,
            None,
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
    fn compact_prompt_and_shrink_policy_prioritize_continuing_user_intent() {
        // The builtin resource is the default model-visible compaction contract.
        let prompt = include_str!("../../../resources/prompts/internal/compact_system.md");
        for policy in [
            "initial compaction summary in full",
            "segment's user messages",
            "prohibitions",
            "completion conditions",
            "explicit changes or retractions",
            "Preserve uncertainty",
            "retained tail stays verbatim",
            "TaskStore",
        ] {
            assert!(prompt.contains(policy), "missing prompt policy: {policy}");
        }
        let shrink = summary_shrink_prompt(2_000, 1_000, 500);
        for policy in [
            "initial summary",
            "and segment",
            "Compress duplicate",
            "prohibitions",
            "completion conditions",
            "exception-reporting duties",
            "changes/retractions",
            "uncertainty",
            "recovery",
        ] {
            assert!(shrink.contains(policy), "missing shrink policy: {policy}");
        }
        let input = test_summary_input(&[Item::user_message("keep original scope")]);
        assert!(input.contains("Preserve ongoing user goals and directives"));
        assert!(input.contains("Retained tail entries remain verbatim"));
    }

    #[test]
    fn malformed_or_duplicate_host_summaries_fail_instead_of_omitting_context() {
        use crate::session_history::history_entry;
        let opening = |item| {
            let mut entry = history_entry(
                item,
                WorkerHistoryProvenance::Provided {
                    producer: "arbitrary-summary-source".into(),
                },
            );
            entry.annotation.input_metadata = Some(
                session_store::HistoryInputMetadata::new(
                    session_store::HistoryInputUsePolicy {
                        recording_material: session_store::InputEvidenceUse::Exclude,
                        memory_evidence: session_store::InputEvidenceUse::Exclude,
                        observation: session_store::InputObservationUse::Hidden,
                        compaction: session_store::InputCompactionUse::OpeningSummary,
                    },
                    None,
                    None,
                )
                .unwrap(),
            );
            entry
        };
        let valid = opening(Item::system_message(format!(
            "{}body",
            crate::session_history::COMPACTION_SUMMARY_HEADER
        )));
        assert_eq!(
            initial_compaction_summary(std::slice::from_ref(&valid))
                .unwrap()
                .as_deref(),
            Some("body")
        );
        assert!(initial_compaction_summary(&[valid.clone(), valid]).is_err());
        for item in [
            Item::user_message("pasted"),
            Item::system_message("missing marker"),
        ] {
            assert!(initial_compaction_summary(&[opening(item)]).is_err());
        }
    }

    #[test]
    fn retained_images_become_on_demand_related_files() {
        let mut items = vec![
            Item::tool_call_json(
                "image-1",
                "ViewImage",
                serde_json::json!({
                    "path": "screenshots/result.png"
                }),
            ),
            Item::tool_result_item_with_attachments(
                "image-1",
                "Attached image screenshots/result.png",
                None,
                false,
                vec![agen::tool::Attachment::Image(
                    agen::tool::ImageAttachment::new("image/png", vec![7_u8; 100_000]),
                )],
            ),
        ];

        let mut capture_entries = items
            .iter()
            .cloned()
            .map(|item| HistoryEntry::new(item, ()))
            .collect::<Vec<_>>();
        assert_eq!(
            project_compactor_capture_attachments(&mut capture_entries),
            1
        );
        assert!(matches!(
            &capture_entries[1].item,
            Item::ToolResult { attachments, .. } if attachments.is_empty()
        ));

        let references = project_compaction_attachments(items.iter_mut(), Some("main"));

        assert_eq!(
            references,
            vec![CompactionImageReference {
                target_workdir: Some("main".to_string()),
                path: "screenshots/result.png".to_string(),
            }]
        );
        assert!(matches!(
            &items[1],
            Item::ToolResult { attachments, .. } if attachments.is_empty()
        ));
        let message = build_related_file_message(&[], &references)
            .and_then(|item| item.as_text().map(str::to_string))
            .unwrap();
        assert!(message.contains("[Related files — contents not included]"));
        assert!(message.contains("screenshots/result.png"));
        assert!(message.contains("target_workdir \"main\""));
        assert!(message.contains("ViewImage"));
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
            None,
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
            None,
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
    fn engine_manifest_generation_and_dispatch_settings_are_applied() {
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
            tool_call_dispatch: manifest::ToolCallDispatchMode::OnToolCallComplete,
            tool_output: manifest::ToolOutputLimits::default(),
            file_upload: manifest::FileUploadLimits::default(),
        };

        let config = request_config_from_engine_manifest(&manifest);

        assert_eq!(config.max_tokens, Some(1024));
        assert_eq!(config.temperature, Some(0.2));
        assert_eq!(config.top_p, Some(0.9));
        assert_eq!(config.top_k, Some(40));
        assert_eq!(config.stop_sequences, vec!["\n\n", "</stop>"]);

        let mut engine = Engine::new(NoopClient);
        apply_worker_manifest(&mut engine, &manifest);
        assert_eq!(
            engine.tool_call_dispatch_mode(),
            agen::ToolCallDispatchMode::OnToolCallComplete
        );

        let mut explicit_after_response = manifest;
        explicit_after_response.tool_call_dispatch = manifest::ToolCallDispatchMode::AfterResponse;
        apply_worker_manifest(&mut engine, &explicit_after_response);
        assert_eq!(
            engine.tool_call_dispatch_mode(),
            agen::ToolCallDispatchMode::AfterResponse
        );
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

    #[derive(Debug, Default)]
    struct RecordingAuditWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
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
            if request.path.ends_with("/authorization/check") {
                return Ok(WorkspaceResponse {
                    status: 200,
                    body: "{}".into(),
                });
            }
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
            if request.path.ends_with("/authorization/check") {
                return Ok(WorkspaceResponse {
                    status: 200,
                    body: "{}".into(),
                });
            }
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
        ) -> Result<crate::hook::HookPreToolAction, crate::hook::HookError> {
            Ok(
                if self
                    .should_pause
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    crate::hook::HookPreToolAction::Pause
                } else {
                    crate::hook::HookPreToolAction::Continue
                },
            )
        }
    }

    #[derive(Clone)]
    struct FailRunTerminalStore {
        inner: session_store::FsStore,
        fail_run_terminal: Arc<AtomicBool>,
        invocation_failure: Arc<AtomicUsize>,
    }

    impl Store for FailRunTerminalStore {
        fn append(
            &self,
            session_id: SessionId,
            segment_id: SegmentId,
            entry: &LogEntry,
        ) -> Result<(), StoreError> {
            if matches!(entry, LogEntry::RunErrored { .. })
                && self.fail_run_terminal.swap(false, Ordering::SeqCst)
            {
                return Err(StoreError::Io(std::io::Error::other(
                    "synthetic terminal append failure",
                )));
            }
            let invocation_kind = match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN
                    && payload["invocation_receipts"]
                        .as_array()
                        .is_some_and(|receipts| {
                            receipts.iter().any(|receipt| receipt["result"].is_null())
                        }) =>
                {
                    1
                }
                LogEntry::AnnotatedSystemItem { entry, .. }
                    if input_schema(entry) == Some("feature:invocation-result") =>
                {
                    2
                }
                LogEntry::AnnotatedSystemItem { entry, .. } if matches!(&entry.item, SystemItem::Notification { message, .. } if message == "second") => {
                    5
                }
                _ => 0,
            };
            if invocation_kind != 0
                && self
                    .invocation_failure
                    .compare_exchange(invocation_kind, 0, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                return Err(StoreError::Io(std::io::Error::other(
                    "synthetic invocation append failure",
                )));
            }
            self.inner.append(session_id, segment_id, entry)
        }

        fn read_all(
            &self,
            session_id: SessionId,
            segment_id: SegmentId,
        ) -> Result<Vec<LogEntry>, StoreError> {
            self.inner.read_all(session_id, segment_id)
        }

        fn list_sessions(&self) -> Result<Vec<SessionId>, StoreError> {
            self.inner.list_sessions()
        }

        fn list_segments(&self, session_id: SessionId) -> Result<Vec<SegmentId>, StoreError> {
            self.inner.list_segments(session_id)
        }

        fn lookup_session_of(
            &self,
            segment_id: SegmentId,
        ) -> Result<Option<SessionId>, StoreError> {
            self.inner.lookup_session_of(segment_id)
        }

        fn create_segment(
            &self,
            session_id: SessionId,
            segment_id: SegmentId,
            entries: &[LogEntry],
        ) -> Result<(), StoreError> {
            if self
                .invocation_failure
                .compare_exchange(3, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(StoreError::Io(std::io::Error::other(
                    "synthetic replacement failure before atomic commit",
                )));
            }
            self.inner.create_segment(session_id, segment_id, entries)?;
            if self
                .invocation_failure
                .compare_exchange(4, 0, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                // Model interruption after the atomic commit, before Worker
                // updates its mirror/counters. Restart sees the complete new log.
                return Err(StoreError::Io(std::io::Error::other(
                    "synthetic interruption after atomic replacement",
                )));
            }
            Ok(())
        }

        fn exists(&self, session_id: SessionId, segment_id: SegmentId) -> Result<bool, StoreError> {
            self.inner.exists(session_id, segment_id)
        }

        fn read_entry_count(
            &self,
            session_id: SessionId,
            segment_id: SegmentId,
        ) -> Result<usize, StoreError> {
            self.inner.read_entry_count(session_id, segment_id)
        }

        fn append_trace(
            &self,
            session_id: SessionId,
            segment_id: SegmentId,
            entry: &session_store::TraceEntry,
        ) -> Result<(), StoreError> {
            self.inner.append_trace(session_id, segment_id, entry)
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

    #[derive(Clone, Copy, Debug)]
    enum AttributionOutcome {
        Success,
        Rejected,
        Unauthorized,
        TransportUnknown,
    }

    struct AttributionLifecycleClient {
        outcomes: Mutex<VecDeque<AttributionOutcome>>,
        before_response: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        requests: Mutex<Vec<WorkspaceRequest>>,
    }

    impl AttributionLifecycleClient {
        fn new(outcomes: impl IntoIterator<Item = AttributionOutcome>) -> Self {
            Self {
                outcomes: Mutex::new(outcomes.into_iter().collect()),
                before_response: Mutex::new(None),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl std::fmt::Debug for AttributionLifecycleClient {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("AttributionLifecycleClient")
        }
    }

    impl WorkspaceClient for AttributionLifecycleClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "attribution-lifecycle-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request.clone());
            if let Some(change) = self.before_response.lock().unwrap().take() {
                change();
            }
            match self
                .outcomes
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(AttributionOutcome::Success)
            {
                AttributionOutcome::TransportUnknown => Err(WorkspaceClientError::Request(
                    "connection closed after request send".to_string(),
                )),
                AttributionOutcome::Unauthorized => Ok(WorkspaceResponse {
                    status: 401,
                    body: serde_json::to_string(&server_api::RepositoryApiError::new(
                        401,
                        "Unauthorized",
                        "invalid runtime request proof",
                        Vec::new(),
                    ))
                    .unwrap(),
                }),
                AttributionOutcome::Rejected => Ok(WorkspaceResponse {
                    status: 503,
                    body: "temporarily unavailable".to_string(),
                }),
                AttributionOutcome::Success => {
                    let input: server_api::SubjektivRecordSessionRequest =
                        serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                    Ok(WorkspaceResponse {
                        status: 200,
                        body: serde_json::to_string(&server_api::SubjektivRecordSessionResponse {
                            subject_id: "subject-test".to_string(),
                            session_id: input.session_id,
                        })
                        .unwrap(),
                    })
                }
            }
        }
    }

    async fn attribution_lifecycle_worker(
        client: Arc<AttributionLifecycleClient>,
    ) -> (
        Worker<
            Box<dyn LlmClient>,
            session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>,
        >,
        session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>,
        tempfile::TempDir,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let mut manifest = minimal_manifest();
        manifest.worker.name = format!("attribution-test-{}", uuid::Uuid::now_v7());
        let mut worker = Worker::new(
            manifest,
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(
                Box::new(NoopClient) as Box<dyn LlmClient>
            ),
            store.clone(),
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                client,
            ),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        (worker, store, temp)
    }

    #[tokio::test]
    async fn ordinary_worker_cannot_gain_job_authority_after_feature_installation() {
        let client = Arc::new(AttributionLifecycleClient::new([]));
        let (mut worker, _store, _temp) = attribution_lifecycle_worker(client).await;
        worker.install_features(FeatureRegistryBuilder::new());
        assert!(
            worker
                .bind_backend_job(crate::BackendJobExecutionBinding {
                    job_id: "job-1".into(),
                    attempt_id: "attempt-1".into(),
                    input_digest: None,
                    subjektiv_consolidation: false,
                    subjektiv_journal: false,
                })
                .is_err()
        );
        assert!(worker.backend_job().is_none());
    }

    #[tokio::test]
    async fn journal_attribution_protects_first_publication_before_any_admission_notification() {
        for enabled in [false, true] {
            let client = Arc::new(AttributionLifecycleClient::new([]));
            let (mut worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
            if enabled {
                worker.manifest.feature.subjektiv.profile.enabled = true;
                worker
                    .manifest
                    .feature
                    .subjektiv
                    .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                        workspace_id: "workspace-test".into(),
                        language: "English".into(),
                    })
                    .unwrap();
            }
            let session = worker.session_id();
            assert!(store.session_store.list_sessions().unwrap().is_empty());
            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::NewSession,
                )
                .unwrap();
            let pin = session_store::retention::SessionRetentionPin(
                subjektiv::journal::source_retention_pin(
                    "workspace-test",
                    "subject-test",
                    &session.to_string(),
                ),
            );
            let expected = if enabled { vec![pin] } else { vec![] };
            assert_eq!(
                store.session_store.retention().pins(session).unwrap(),
                expected
            );
            // Reopen/replayed confirmation does not depend on a notification or add an owner.
            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::RestoredSession,
                )
                .unwrap();
            assert_eq!(client.requests.lock().unwrap().len(), 1);
            assert_eq!(
                store.session_store.retention().pins(session).unwrap(),
                expected
            );
            worker.materialize_durable_session_head().await.unwrap();
            let segment = worker.segment_id();
            store
                .append(
                    session,
                    segment,
                    &LogEntry::TurnEnd {
                        ts: 1,
                        turn_count: 1,
                    },
                )
                .unwrap();
            // Simulate stop after the public commit but before RunCommitted/Host admission.
            if enabled {
                assert!(
                    matches!(store.session_store.delete_session(session), Err(StoreError::SessionPinned(id)) if id == session)
                );
                assert!(store.session_store.exists(session, segment).unwrap());
                store
                    .session_store
                    .release_session_pin(session, pin)
                    .unwrap();
            }
            assert!(store.session_store.delete_session(session).unwrap());
        }
    }

    #[tokio::test]
    async fn subject_maintenance_job_binding_is_immutable_and_has_no_subject_body_attribution() {
        for (subjektiv_consolidation, subjektiv_journal) in [(true, false), (false, true)] {
            let client = Arc::new(AttributionLifecycleClient::new([]));
            let (mut worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
            let binding = crate::BackendJobExecutionBinding {
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                input_digest: Some("input-content-digest".into()),
                subjektiv_consolidation,
                subjektiv_journal,
            };
            worker.bind_backend_job(binding.clone()).unwrap();
            assert_eq!(worker.backend_job(), Some(&binding));
            assert!(worker.bind_backend_job(binding).is_err());
            for lifecycle in [
                SubjektivSessionAttributionLifecycle::NewSession,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            ] {
                worker
                    .finalize_subjektiv_session_attribution(true, lifecycle)
                    .unwrap();
                worker.validate_subjektiv_body_publication().unwrap();
            }
            assert!(client.requests.lock().unwrap().is_empty());
            assert!(
                store
                    .read_by_name(&worker.manifest().worker.name)
                    .unwrap()
                    .unwrap()
                    .subjektiv_session_attribution
                    .is_none()
            );
        }
    }

    #[tokio::test]
    async fn session_attribution_finalizes_once_before_runs_and_persists_subject() {
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Success,
        ]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
        let session_id = worker.session_id();

        worker
            .finalize_subjektiv_session_attribution(
                true,
                SubjektivSessionAttributionLifecycle::NewSession,
            )
            .unwrap();
        worker
            .finalize_subjektiv_session_attribution(
                true,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            )
            .unwrap();

        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request: server_api::SubjektivRecordSessionRequest =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert!(request.create_if_missing);
        drop(requests);
        let metadata = store
            .read_by_name(&worker.manifest().worker.name)
            .unwrap()
            .unwrap();
        assert!(matches!(
            metadata.subjektiv_session_attribution,
            Some(SubjektivSessionAttributionState::Confirmed {
                session_id: stored_session_id,
                ref subject_id,
            }) if stored_session_id == session_id && subject_id == "subject-test"
        ));
    }

    // Production lifecycle, atomic Host CAS, and filesystem metadata store. Only
    // the external Workspace transport is substituted to interleave a newer owner.
    #[tokio::test]
    async fn delayed_attribution_response_cannot_overwrite_new_session_or_subject_evidence() {
        for switch_session in [true, false] {
            let client = Arc::new(AttributionLifecycleClient::new([
                AttributionOutcome::Success,
            ]));
            let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
            let worker_name = worker.manifest().worker.name.clone();
            let replacement_session = if switch_session {
                session_store::new_session_id()
            } else {
                worker.session_id()
            };
            let changed = store.clone();
            let name = worker_name.clone();
            *client.before_response.lock().unwrap() = Some(Box::new(move || {
                if switch_session {
                    worker_metadata_writer_for_store(&changed)(WorkerMetadata::new(
                        name,
                        Some(WorkerActiveSegmentRef::pending_segment(replacement_session)),
                    ))
                    .unwrap();
                } else {
                    changed
                        .update_by_name(&name, |metadata| {
                            metadata.subjektiv_session_attribution =
                                Some(SubjektivSessionAttributionState::Confirmed {
                                    session_id: replacement_session,
                                    subject_id: "new-subject".into(),
                                });
                        })
                        .unwrap();
                }
            }));
            let error = worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::NewSession,
                )
                .unwrap_err();
            assert!(
                error.to_string().contains("attribution_conflict"),
                "{error}"
            );
            let saved = store.read_by_name(&worker_name).unwrap().unwrap();
            assert_eq!(
                saved.active.as_ref().unwrap().session_id,
                replacement_session
            );
            assert_eq!(
                saved.subjektiv_session_attribution,
                if switch_session {
                    None
                } else {
                    Some(SubjektivSessionAttributionState::Confirmed {
                        session_id: replacement_session,
                        subject_id: "new-subject".into(),
                    })
                }
            );
            assert_eq!(client.requests.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn released_body_cannot_reconfirm_attribution_or_contact_host() {
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Success,
        ]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
        let before = store
            .read_by_name(&worker.manifest().worker.name)
            .unwrap()
            .unwrap();
        let control = crate::subject_release::ReleaseControl::new(worker.mutation_authority());
        let request = tokio::spawn({
            let control = control.clone();
            async move { control.request().await }
        });
        control.notified().await;
        control.respond(|| Ok(()));
        let mut reservation = request.await.unwrap().unwrap();
        reservation.commit_disconnect().unwrap();
        drop(reservation);
        assert!(
            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::NewSession
                )
                .is_err()
        );
        assert!(client.requests.lock().unwrap().is_empty());
        assert_eq!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap(),
            before
        );
    }

    #[tokio::test]
    async fn unknown_and_failed_attribution_remain_incomplete_until_lifecycle_retry() {
        for (first, expected_state) in [
            (AttributionOutcome::TransportUnknown, "outcome_unknown"),
            (AttributionOutcome::Rejected, "failed"),
            (AttributionOutcome::Unauthorized, "failed"),
        ] {
            let client = Arc::new(AttributionLifecycleClient::new([
                first,
                AttributionOutcome::Success,
            ]));
            let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
            let error = worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::NewSession,
                )
                .unwrap_err();
            assert!(error.to_string().contains(expected_state));
            if matches!(first, AttributionOutcome::Unauthorized) {
                assert!(error.to_string().contains("rejected with HTTP 401"));
                assert!(!error.to_string().contains("outcome_unknown"));
                assert_eq!(
                    client.requests.lock().unwrap().len(),
                    1,
                    "no automatic retry after auth rejection"
                );
            }
            let incomplete = store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution
                .unwrap();
            assert!(matches!(
                (expected_state, incomplete),
                (
                    "outcome_unknown",
                    SubjektivSessionAttributionState::OutcomeUnknown { .. }
                ) | ("failed", SubjektivSessionAttributionState::Failed { .. })
            ));

            worker
                .finalize_subjektiv_session_attribution(
                    true,
                    SubjektivSessionAttributionLifecycle::RestoredSession,
                )
                .unwrap();
            assert_eq!(client.requests.lock().unwrap().len(), 2);
            assert!(matches!(
                store
                    .read_by_name(&worker.manifest().worker.name)
                    .unwrap()
                    .unwrap()
                    .subjektiv_session_attribution,
                Some(SubjektivSessionAttributionState::Confirmed { .. })
            ));
        }
    }

    #[tokio::test]
    async fn unattached_worker_never_attempts_session_attribution() {
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Success,
        ]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;

        worker
            .finalize_subjektiv_session_attribution(
                false,
                SubjektivSessionAttributionLifecycle::NewSession,
            )
            .unwrap();

        assert!(client.requests.lock().unwrap().is_empty());
        assert!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution
                .is_none()
        );
    }

    #[tokio::test]
    async fn pending_restore_reuses_persisted_session_and_recovers_unknown_attribution() {
        let temp = tempfile::tempdir().unwrap();
        // Startup reads the process runtime-dir environment. Share the existing
        // sandbox lock with allocation tests so their temporary directories cannot
        // disappear between this test's path lookup and lock-file creation.
        let runtime_dir = temp.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let _runtime_sandbox =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(&runtime_dir);
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let mut manifest = minimal_manifest();
        manifest.worker.name = format!("pending-attribution-{}", uuid::Uuid::now_v7());
        let session_id = session_store::new_session_id();
        let snapshot = manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap();
        store
            .set_active_with_workspace_id(
                &manifest.worker.name,
                Some(WorkerActiveSegmentRef::pending_segment(session_id)),
                Some(snapshot),
                Some("workspace-test".to_string()),
            )
            .unwrap();
        store
            .update_by_name(&manifest.worker.name, |metadata| {
                metadata.subjektiv_session_attribution =
                    Some(SubjektivSessionAttributionState::OutcomeUnknown {
                        session_id,
                        diagnostic: "response lost before restart".to_string(),
                    });
            })
            .unwrap();
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Success,
        ]));
        let worker_name = manifest.worker.name.clone();
        let worker = Worker::restore_pending_from_worker_metadata_with_context_and_model_client(
            &worker_name,
            manifest,
            store.clone(),
            PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("workspace-test").unwrap()),
                client.clone(),
            ),
            WorkerFilesystemAuthority::None,
            Some(Box::new(NoopClient)),
        )
        .await
        .unwrap();

        assert_eq!(worker.session_id(), session_id);
        worker
            .finalize_subjektiv_session_attribution(
                true,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            )
            .unwrap();
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request: server_api::SubjektivRecordSessionRequest =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert!(request.create_if_missing);
        drop(requests);
        assert!(matches!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution,
            Some(SubjektivSessionAttributionState::Confirmed {
                session_id: stored_session_id,
                ..
            }) if stored_session_id == session_id
        ));
    }

    #[tokio::test]
    async fn materialized_legacy_session_without_backend_attribution_stays_ambiguous() {
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Rejected,
        ]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
        let session_id = worker.session_id();
        store
            .set_active(
                &worker.manifest().worker.name,
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id,
                    worker.segment_state.location().segment_id,
                )),
                None,
            )
            .unwrap();

        let error = worker
            .finalize_subjektiv_session_attribution(
                true,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            )
            .unwrap_err();

        assert!(error.to_string().contains("legacy_unknown"));
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request: server_api::SubjektivRecordSessionRequest =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(request.session_id, session_id.to_string());
        assert!(!request.create_if_missing);
        drop(requests);
        assert!(matches!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution,
            Some(SubjektivSessionAttributionState::LegacyUnknown {
                session_id: stored_session_id,
                ..
            }) if stored_session_id == session_id
        ));
    }

    #[tokio::test]
    async fn materialized_legacy_session_adopts_only_matching_backend_attribution() {
        let client = Arc::new(AttributionLifecycleClient::new([
            AttributionOutcome::Success,
        ]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
        let session_id = worker.session_id();
        store
            .set_active(
                &worker.manifest().worker.name,
                Some(WorkerActiveSegmentRef::active_segment(
                    session_id,
                    worker.segment_state.location().segment_id,
                )),
                None,
            )
            .unwrap();

        worker
            .finalize_subjektiv_session_attribution(
                true,
                SubjektivSessionAttributionLifecycle::RestoredSession,
            )
            .unwrap();

        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request: server_api::SubjektivRecordSessionRequest =
            serde_json::from_str(requests[0].body.as_deref().unwrap()).unwrap();
        assert!(!request.create_if_missing);
        drop(requests);
        assert!(matches!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution,
            Some(SubjektivSessionAttributionState::Confirmed {
                session_id: stored_session_id,
                ref subject_id,
            }) if stored_session_id == session_id && subject_id == "subject-test"
        ));
    }

    fn text_segment(text: &str) -> Segment {
        Segment::Text {
            content: text.into(),
        }
    }

    #[tokio::test]
    async fn failed_usage_append_fences_segment_rotation_until_retry_commits_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        let session_id = worker.session_id();
        let source_segment = worker.segment_id();

        worker
            .usage_tracker
            .note_compaction_correlation_id("compact-id".into());
        worker.usage_tracker.note_request(1);
        worker.usage_tracker.record_usage(&agen::event::UsageEvent {
            input_tokens: Some(10),
            output_tokens: Some(1),
            total_tokens: Some(11),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        });
        worker.usage_tracker.request_completed();

        let append_error = persist_pending_usage(
            &worker.usage_tracker,
            &worker.usage_history,
            worker.compact_state.as_deref(),
            |_, _, _| Err(StoreError::Io(std::io::Error::other("synthetic failure"))),
        )
        .expect_err("first usage append must fail");
        assert!(append_error.to_string().contains("synthetic failure"));
        assert_eq!(worker.usage_tracker.pending_record_count(), 1);

        // Simulate another writer extending the active Segment. The normal run
        // prelude must not auto-fork while the failed record still belongs to
        // this Segment.
        store
            .append(
                session_id,
                source_segment,
                &LogEntry::TurnEnd {
                    ts: segment_log::now_millis(),
                    turn_count: 99,
                },
            )
            .unwrap();
        let run_error = worker
            .run_text("must stop before provider ownership")
            .await
            .expect_err("head drift must not fork past pending usage");
        assert!(matches!(
            run_error,
            WorkerError::PendingUsagePersistence { pending_records: 1 }
        ));
        assert_eq!(worker.segment_id(), source_segment);
        assert_eq!(
            store.list_segments(session_id).unwrap(),
            vec![source_segment]
        );

        let error = worker
            .compact(0)
            .await
            .expect_err("pending usage must fence Segment replacement");
        assert!(matches!(
            error,
            WorkerError::PendingUsagePersistence { pending_records: 1 }
        ));
        assert_eq!(worker.segment_id(), source_segment);
        assert_eq!(
            store.list_segments(session_id).unwrap(),
            vec![source_segment]
        );

        let metrics = persist_pending_usage(
            &worker.usage_tracker,
            &worker.usage_history,
            worker.compact_state.as_deref(),
            |entry, after_commit, _| {
                store.append(session_id, source_segment, &entry)?;
                after_commit();
                Ok(())
            },
        )
        .expect("retry should commit on the originating Segment");
        assert_eq!(worker.usage_tracker.pending_record_count(), 0);
        assert_eq!(worker.usage_history.lock().unwrap().len(), 1);
        assert_eq!(metrics.len(), 1, "correlation must be emitted once");
        assert_eq!(metrics[0].name, "compact.post_request");

        let usage_entries = store
            .read_all(session_id, source_segment)
            .unwrap()
            .into_iter()
            .filter(|entry| matches!(entry, LogEntry::LlmUsage { .. }))
            .count();
        assert_eq!(usage_entries, 1, "retry must not duplicate accounting");
        assert_eq!(
            store.list_segments(session_id).unwrap(),
            vec![source_segment]
        );
    }

    #[tokio::test]
    async fn yielded_engine_exit_commits_intermediate_transition_not_run_completed() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        worker.engine_mut().set_active_run_turn_count(Some(2));

        worker
            .persist_turn(0, &EngineRunExit::Yielded, None)
            .await
            .unwrap();

        let entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert!(entries.iter().any(|entry| matches!(
            entry,
            LogEntry::RunYielded {
                reason: RunYieldReason::Compaction,
                active_run_turn_count: 2,
                ..
            }
        )));
        assert!(!entries.iter().any(|entry| matches!(
            entry,
            LogEntry::RunCompleted {
                result: EngineResult::Yielded,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn failed_compaction_terminalizes_the_yielded_run_without_resume() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        worker.engine_mut().set_active_run_turn_count(Some(2));
        worker
            .persist_turn(0, &EngineRunExit::Yielded, None)
            .await
            .unwrap();
        worker.last_run_interrupted = true;

        worker
            .persist_compaction_run_failure("mid-run compaction failed: summary unavailable".into())
            .unwrap();

        let entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        let yielded = entries
            .iter()
            .position(|entry| matches!(entry, LogEntry::RunYielded { .. }))
            .unwrap();
        let failed = entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    LogEntry::RunErrored {
                        interrupted: false,
                        failure: Some(RunFailureKind::Compaction),
                        ..
                    }
                )
            })
            .unwrap();
        assert!(yielded < failed);
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, LogEntry::RunResumed { .. }))
        );
        assert!(!worker.last_run_interrupted);
        assert_eq!(worker.engine().active_run_turn_count(), None);
    }

    #[tokio::test]
    async fn failed_compaction_terminal_append_stays_unhandled_and_unpublished() {
        let dir = tempfile::tempdir().unwrap();
        let store = FailRunTerminalStore {
            inner: session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
            fail_run_terminal: Arc::new(AtomicBool::new(false)),
            invocation_failure: Arc::new(AtomicUsize::new(0)),
        };
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        store.fail_run_terminal.store(true, Ordering::SeqCst);

        let error = worker.terminalize_compaction_run_failure(
            "mid-run compaction failed: summary unavailable".into(),
            WorkerError::CompactSummaryMissing,
        );

        assert!(matches!(
            error,
            WorkerError::RunTerminalPersistence {
                run_failure,
                persist_error,
            } if matches!(*run_failure, WorkerError::CompactSummaryMissing)
                && persist_error.to_string().contains("synthetic terminal append failure")
        ));
        assert!(
            store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap()
                .iter()
                .all(|entry| !matches!(entry, LogEntry::RunErrored { .. }))
        );
        assert!(
            worker
                .sink()
                .subscribe_with_snapshot()
                .0
                .iter()
                .all(|entry| !matches!(entry, LogEntry::RunErrored { .. })),
            "a failed append must not be published as durable"
        );
    }

    #[test]
    fn compaction_internal_turn_preserves_typed_stop_causes() {
        use crate::internal_worker::InternalWorkerTurnOutcome;

        let limit = ensure_compaction_internal_turn_succeeded(Some(
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::LimitReached),
        ))
        .unwrap_err();
        assert!(
            matches!(limit, WorkerError::Engine(EngineError::Aborted(message)) if message.contains("turn limit"))
        );

        let interrupted = ensure_compaction_internal_turn_succeeded(Some(
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Interrupted {
                code: ErrorCode::ProviderError,
                message: "summary provider failed".to_string(),
            }),
        ))
        .unwrap_err();
        assert!(
            matches!(interrupted, WorkerError::Engine(EngineError::Aborted(message)) if message == "summary provider failed")
        );

        let cancelled = ensure_compaction_internal_turn_succeeded(Some(
            InternalWorkerTurnOutcome::Completed(WorkerRunResult::Cancelled),
        ))
        .unwrap_err();
        assert!(matches!(cancelled, WorkerError::CompactCancelled));

        let unknown = ensure_compaction_internal_turn_succeeded(None).unwrap_err();
        assert!(
            matches!(unknown, WorkerError::InvalidState(message) if message.contains("without a typed turn outcome"))
        );
    }

    #[tokio::test]
    async fn intentional_cancel_commits_a_non_failure_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        worker.engine_mut().set_active_run_turn_count(Some(1));

        let result = worker
            .handle_worker_result(
                EngineRunExit::Interrupted(RunInterruptionReason::Cancelled),
                0,
            )
            .await
            .unwrap();

        assert_eq!(result, WorkerRunResult::Cancelled);
        let entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, LogEntry::RunCancelled { .. }))
        );
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, LogEntry::RunErrored { .. }))
        );
        assert!(!worker.last_run_interrupted);
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
        worker.ensure_segment_head().await.unwrap();
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

        let mut replacement_entries = vec![LogEntry::AnnotatedSegmentStart {
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
    async fn restore_cleanup_handle_retains_allocation_after_worker_drop() {
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
        assert!(worker.restore_allocation_cleanup_handle().is_none());
        let lock_path = dir.path().join("workers.json");
        worker.scope_allocation = Some(
            worker_allocation::install_top_level_at_for_test(
                lock_path.clone(),
                "test-worker".into(),
                std::process::id(),
                dir.path().join("worker.sock"),
                Vec::new(),
                worker.segment_id(),
            )
            .unwrap(),
        );
        let cleanup = worker.restore_allocation_cleanup_handle().unwrap();

        drop(worker);
        {
            let table = worker_allocation::LockFileGuard::open(&lock_path).unwrap();
            assert!(table.data().find("test-worker").is_some());
        }
        cleanup.release().unwrap();
        let table = worker_allocation::LockFileGuard::open(&lock_path).unwrap();
        assert!(table.data().find("test-worker").is_none());
    }

    // Real Session/metadata stores, allocation and Workdir providers; only the
    // model transport is substituted. Profile/Runtime publication belongs to the
    // factory integration suite, not this reconstruction component suite.
    mod restored_candidate_tests {
        use super::*;

        type CandidateStore =
            session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

        #[derive(Clone)]
        struct CountedClient(Arc<AtomicUsize>);

        #[async_trait]
        impl LlmClient for CountedClient {
            async fn stream(
                &self,
                request: agen::llm_client::Request,
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
                self.0.fetch_add(1, Ordering::SeqCst);
                NoopClient.stream(request).await
            }

            fn clone_boxed(&self) -> Box<dyn LlmClient> {
                Box::new(self.clone())
            }
        }

        #[derive(Debug)]
        struct FailCloseOnce {
            inner: workdir::WorkdirSessionHandle,
            attempts: AtomicUsize,
        }

        #[async_trait]
        impl workdir::WorkdirSession for FailCloseOnce {
            fn workdir(&self) -> &workdir::Workdir {
                self.inner.workdir()
            }
            fn capabilities(&self) -> workdir::WorkdirSessionCapabilities {
                self.inner.capabilities()
            }
            async fn stat(
                &self,
                request: workdir::StatRequest,
            ) -> Result<workdir::StatResult, workdir::WorkdirError> {
                self.inner.stat(request).await
            }
            async fn read(
                &self,
                request: workdir::ReadRequest,
            ) -> Result<workdir::ReadResult, workdir::WorkdirError> {
                self.inner.read(request).await
            }
            async fn write(
                &self,
                request: workdir::WriteRequest,
            ) -> Result<workdir::WriteResult, workdir::WorkdirError> {
                self.inner.write(request).await
            }
            async fn edit(
                &self,
                request: workdir::EditRequest,
            ) -> Result<workdir::EditResult, workdir::WorkdirError> {
                self.inner.edit(request).await
            }
            async fn list(
                &self,
                request: workdir::ListRequest,
            ) -> Result<workdir::ListResult, workdir::WorkdirError> {
                self.inner.list(request).await
            }
            async fn glob(
                &self,
                request: workdir::GlobRequest,
            ) -> Result<workdir::GlobResult, workdir::WorkdirError> {
                self.inner.glob(request).await
            }
            async fn grep(
                &self,
                request: workdir::GrepRequest,
            ) -> Result<workdir::GrepResult, workdir::WorkdirError> {
                self.inner.grep(request).await
            }
            async fn start_command(
                &self,
                request: workdir::CommandRequest,
            ) -> Result<workdir::CommandHandle, workdir::WorkdirError> {
                self.inner.start_command(request).await
            }
            async fn command_status(
                &self,
                handle: workdir::CommandHandle,
            ) -> Result<workdir::CommandStatus, workdir::WorkdirError> {
                self.inner.command_status(handle).await
            }
            async fn command_output(
                &self,
                request: workdir::CommandOutputRequest,
            ) -> Result<workdir::CommandOutput, workdir::WorkdirError> {
                self.inner.command_output(request).await
            }
            async fn cancel_command(
                &self,
                handle: workdir::CommandHandle,
            ) -> Result<(), workdir::WorkdirError> {
                self.inner.cancel_command(handle).await
            }
            async fn close(&self) -> Result<(), workdir::WorkdirError> {
                self.inner.close().await?;
                if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    // The real provider has closed, but its first response is
                    // uncertain: the candidate must not become activatable again.
                    Err(workdir::WorkdirError::Unavailable(
                        "close response lost".into(),
                    ))
                } else {
                    Ok(())
                }
            }
        }

        struct Fixture {
            temp: tempfile::TempDir,
            store: CandidateStore,
            manifest: WorkerManifest,
            session_id: SessionId,
            segment_id: SegmentId,
            orphan: protocol::UploadedFileRef,
            model_calls: Arc<AtomicUsize>,
        }

        impl Fixture {
            async fn new() -> Self {
                let temp = tempfile::tempdir().unwrap();
                let store = session_store::CombinedStore::new(
                    session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                    session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
                );
                let mut manifest = minimal_manifest();
                manifest.scope = ScopeConfig {
                    allow: vec![ScopeRule {
                        target: temp.path().canonicalize().unwrap(),
                        permission: Permission::Write,
                        recursive: true,
                        symlink_policy: Default::default(),
                    }],
                    deny: Vec::new(),
                };
                let model_calls = Arc::new(AtomicUsize::new(0));
                let mut worker = Worker::new(
                    manifest.clone(),
                    Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                        Box::new(CountedClient(model_calls.clone())),
                    ),
                    store.clone(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::None,
                    Scope::empty(),
                )
                .await
                .unwrap();
                worker
                    .engine_mut()
                    .set_system_prompt("saved candidate prompt");
                worker.ensure_segment_head().await.unwrap();
                let session_id = worker.session_id();
                let segment_id = worker.segment_id();
                let queue = worker.pending_submission_handle();
                queue
                    .accept(
                        "saved-submit".into(),
                        vec![Segment::Text {
                            content: "saved input".into(),
                        }],
                        false,
                    )
                    .unwrap();
                queue
                    .accept_notification("saved-notification".into(), "saved notice".into())
                    .unwrap();
                let orphan = store
                    .write_uploaded_file(
                        session_id,
                        "orphan.txt",
                        "text/plain",
                        b"orphan",
                        session_store::UploadedFileLimits::default(),
                    )
                    .unwrap();
                store
                    .pin_uploaded_file(session_id, &orphan, "acceptance-without-checkpoint")
                    .unwrap();
                let mut metadata = WorkerMetadata::new(
                    &manifest.worker.name,
                    Some(WorkerActiveSegmentRef {
                        session_id,
                        segment_id: Some(segment_id),
                    }),
                );
                // Activation replaces this stale derived path; preparation must not.
                metadata.workspace_root = Some(temp.path().join("old-root"));
                metadata.spawned_children.push(WorkerSpawnedChild {
                    worker_name: "missing-child".into(),
                    socket_path: temp.path().join("missing-child.sock"),
                    scope_delegated: Vec::new(),
                    callback_address: temp.path().join("unused-parent.sock"),
                });
                metadata.resolved_manifest_snapshot =
                    Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap());
                store.write(&metadata).unwrap();
                drop(worker);
                Self {
                    temp,
                    store,
                    manifest,
                    session_id,
                    segment_id,
                    orphan,
                    model_calls,
                }
            }

            async fn prepare(&self) -> PreparedRestoredWorker<CandidateStore> {
                Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                    &self.manifest.worker.name,
                    self.manifest.clone(),
                    self.store.clone(),
                    PromptCatalogSource::builtins_only(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::local(
                        self.temp.path().to_path_buf(),
                        self.temp.path().to_path_buf(),
                    ),
                    Some(Box::new(CountedClient(self.model_calls.clone()))),
                )
                .await
                .unwrap()
            }

            fn allocate(
                &self,
                worker: &Worker<Box<dyn LlmClient>, CandidateStore>,
            ) -> Result<ScopeAllocationGuard, WorkerError> {
                Ok(worker_allocation::install_top_level_at_for_test(
                    self.temp.path().join("allocation.json"),
                    worker.manifest.worker.name.clone(),
                    std::process::id(),
                    self.temp.path().join("worker.sock"),
                    worker.scope.snapshot().allow_rules(),
                    worker.segment_id(),
                )?)
            }

            fn snapshot(&self) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
                fn visit(
                    root: &Path,
                    path: &Path,
                    files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
                ) {
                    for entry in std::fs::read_dir(path).unwrap() {
                        let path = entry.unwrap().path();
                        if path.is_dir() {
                            visit(root, &path, files);
                        } else {
                            files.insert(
                                path.strip_prefix(root).unwrap().to_path_buf(),
                                std::fs::read(path).unwrap(),
                            );
                        }
                    }
                }
                let mut files = std::collections::BTreeMap::new();
                for name in ["sessions", "workers"] {
                    visit(self.temp.path(), &self.temp.path().join(name), &mut files);
                }
                files
            }
        }

        #[tokio::test]
        async fn prepare_and_discard_preserve_session_queue_pins_and_metadata() {
            let fixture = Fixture::new().await;
            let before = fixture.snapshot();
            let mut candidate = fixture.prepare().await;
            assert!(
                candidate
                    .worker()
                    .restore_allocation_cleanup_handle()
                    .is_none()
            );
            assert!(!fixture.temp.path().join("worker.sock").exists());
            assert_eq!(
                candidate.worker().pending_submissions().submissions.len(),
                1
            );
            assert_eq!(
                candidate
                    .worker()
                    .session
                    .pending_activations
                    .lock()
                    .unwrap()
                    .pending_notifications
                    .len(),
                1
            );
            assert_eq!(fixture.snapshot(), before);
            assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
            let session = candidate
                .worker()
                .workdir_sessions()
                .resolve(None)
                .unwrap()
                .session;
            assert!(
                session
                    .stat(workdir::StatRequest {
                        path: workdir::WorkdirPath::root()
                    })
                    .await
                    .is_ok()
            );
            candidate.discard().await.unwrap();
            candidate.discard().await.unwrap();
            assert!(
                session
                    .stat(workdir::StatRequest {
                        path: workdir::WorkdirPath::root()
                    })
                    .await
                    .is_err()
            );
            assert_eq!(fixture.snapshot(), before);
        }

        #[tokio::test]
        async fn failed_discard_allows_only_cleanup_retry_not_binding_or_activation() {
            for retry_discard in [true, false] {
                let fixture = Fixture::new().await;
                let before = fixture.snapshot();
                let mut candidate = fixture.prepare().await;
                let provider = Arc::new(FailCloseOnce {
                    inner: candidate.workdir_sessions().resolve(None).unwrap().session,
                    attempts: AtomicUsize::new(0),
                });
                candidate
                    .worker_mut()
                    .unwrap()
                    .bind_single_workdir_session(Some(provider.clone()));
                let cleanup = candidate.workdir_sessions();
                assert!(matches!(
                    candidate.discard().await,
                    Err(WorkerError::FeatureLifecycle(_))
                ));
                assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
                assert!(!cleanup.is_empty());
                assert_eq!(fixture.snapshot(), before);
                assert!(matches!(
                    candidate.worker_mut(),
                    Err(WorkerError::RestoreCandidateClosing)
                ));
                assert_eq!(
                    candidate.manifest().worker.name,
                    fixture.manifest.worker.name
                );
                assert!(Arc::ptr_eq(&candidate.workdir_sessions(), &cleanup));
                if retry_discard {
                    candidate.discard().await.unwrap();
                    assert!(cleanup.is_empty());
                    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
                    assert!(matches!(
                        candidate.worker_mut(),
                        Err(WorkerError::RestoreCandidateClosing)
                    ));
                    candidate.discard().await.unwrap();
                } else {
                    let allocations = AtomicUsize::new(0);
                    let error = match candidate
                        .activate_with_allocation(|worker| {
                            allocations.fetch_add(1, Ordering::SeqCst);
                            fixture.allocate(worker)
                        })
                        .await
                    {
                        Err(error) => error,
                        Ok(_) => panic!("partially closed candidate was activated"),
                    };
                    let WorkerError::ConstructionCleanup {
                        source,
                        allocation,
                        sessions,
                    } = error
                    else {
                        panic!("cleanup ownership lost");
                    };
                    assert!(matches!(*source, WorkerError::RestoreCandidateClosing));
                    assert!(allocation.is_none());
                    assert_eq!(allocations.load(Ordering::SeqCst), 0);
                    assert_eq!(provider.attempts.load(Ordering::SeqCst), 1);
                    assert!(Arc::ptr_eq(&sessions, &cleanup));
                    sessions.close_all().await.unwrap();
                    assert!(cleanup.is_empty());
                    assert_eq!(provider.attempts.load(Ordering::SeqCst), 2);
                }
                assert_eq!(fixture.snapshot(), before);
                assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
            }
        }

        #[tokio::test]
        async fn dropping_candidate_closes_workdir_without_consuming_saved_inputs() {
            let fixture = Fixture::new().await;
            let before = fixture.snapshot();
            let candidate = fixture.prepare().await;
            let sessions = candidate.worker().workdir_sessions();
            let session = sessions.resolve(None).unwrap().session;
            drop(candidate);
            tokio::time::timeout(Duration::from_secs(5), async {
                while !sessions.is_empty() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("candidate Drop must finish direct close");
            assert!(
                session
                    .stat(workdir::StatRequest {
                        path: workdir::WorkdirPath::root()
                    })
                    .await
                    .is_err()
            );
            assert_eq!(fixture.snapshot(), before);
            assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn activation_alone_reconciles_orphan_pins_and_updates_active_metadata() {
            let fixture = Fixture::new().await;
            let before_metadata = fixture
                .store
                .read_by_name(&fixture.manifest.worker.name)
                .unwrap()
                .unwrap();
            let before_entries = restore_snapshot_value(
                &fixture
                    .store
                    .read_all(fixture.session_id, fixture.segment_id)
                    .unwrap(),
            )
            .unwrap();
            let candidate = fixture.prepare().await;
            assert!(matches!(
                fixture
                    .store
                    .delete_uploaded_file(fixture.session_id, &fixture.orphan.artifact_id),
                Err(StoreError::ArtifactAlreadyCommitted)
            ));
            let worker = candidate
                .activate_with_allocation(|worker| fixture.allocate(worker))
                .await
                .unwrap();
            assert!(worker.restore_allocation_cleanup_handle().is_some());
            let metadata = fixture
                .store
                .read_by_name(&fixture.manifest.worker.name)
                .unwrap()
                .unwrap();
            assert_ne!(metadata.workspace_root, before_metadata.workspace_root);
            assert_eq!(metadata.active, before_metadata.active);
            assert_eq!(before_metadata.spawned_children.len(), 1);
            assert!(metadata.spawned_children.is_empty());
            assert_eq!(metadata.reclaimed_children.len(), 1);
            assert!(
                fixture
                    .store
                    .delete_uploaded_file(fixture.session_id, &fixture.orphan.artifact_id)
                    .unwrap()
            );
            assert_eq!(worker.pending_submissions().submissions.len(), 1);
            assert_eq!(
                worker
                    .session
                    .pending_activations
                    .lock()
                    .unwrap()
                    .pending_notifications
                    .len(),
                1
            );
            assert_eq!(
                restore_snapshot_value(
                    &fixture
                        .store
                        .read_all(fixture.session_id, fixture.segment_id)
                        .unwrap()
                )
                .unwrap(),
                before_entries
            );
            assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
            worker.workdir_sessions().close_all().await.unwrap();
            let allocation = worker.restore_allocation_cleanup_handle().unwrap();
            assert!(matches!(
                allocation.release(),
                Err(ScopeLockError::AllocationGuardAttached { .. })
            ));
            drop(worker);
            allocation.release().unwrap();
        }

        #[tokio::test]
        async fn metadata_or_same_entry_count_history_drift_rejects_before_allocation() {
            for component in ["metadata", "segment", "manifest"] {
                let fixture = Fixture::new().await;
                let mut candidate = fixture.prepare().await;
                match component {
                    "metadata" => {
                        fixture
                            .store
                            .update_by_name(&fixture.manifest.worker.name, |metadata| {
                                metadata.workspace_id = Some("changed-workspace".into());
                            })
                            .unwrap();
                    }
                    "segment" => {
                        let mut entries = fixture
                            .store
                            .read_all(fixture.session_id, fixture.segment_id)
                            .unwrap();
                        let LogEntry::AnnotatedSegmentStart { system_prompt, .. } = &mut entries[0]
                        else {
                            panic!("fixture seed");
                        };
                        *system_prompt = Some("different same-length history".into());
                        fixture
                            .store
                            .create_segment(fixture.session_id, fixture.segment_id, &entries)
                            .unwrap();
                    }
                    "manifest" => {
                        candidate.worker_mut().unwrap().manifest.model.model_id =
                            Some("changed-model".into());
                    }
                    _ => unreachable!(),
                }
                let before = fixture.snapshot();
                let allocations = AtomicUsize::new(0);
                let error = match candidate
                    .activate_with_allocation(|worker| {
                        allocations.fetch_add(1, Ordering::SeqCst);
                        fixture.allocate(worker)
                    })
                    .await
                {
                    Err(error) => error,
                    Ok(_) => panic!("stale candidate accepted"),
                };
                let WorkerError::ConstructionCleanup {
                    source,
                    allocation,
                    sessions,
                } = error
                else {
                    panic!("cleanup ownership lost");
                };
                assert!(
                    matches!(*source, WorkerError::RestoreCandidateChanged { component: changed, .. } if changed == component)
                );
                assert!(allocation.is_none());
                assert_eq!(allocations.load(Ordering::SeqCst), 0);
                sessions.close_all().await.unwrap();
                assert_eq!(fixture.snapshot(), before);
                assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
            }
        }

        #[tokio::test]
        async fn post_allocation_metadata_drift_retains_cleanup_without_mutating_pins() {
            let fixture = Fixture::new().await;
            let candidate = fixture.prepare().await;
            let error = match candidate
                .activate_with_allocation(|worker| {
                    let guard = fixture.allocate(worker)?;
                    fixture
                        .store
                        .update_by_name(&fixture.manifest.worker.name, |metadata| {
                            metadata.workspace_id = Some("changed-under-allocation".into());
                        })?;
                    Ok(guard)
                })
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("stale candidate accepted"),
            };
            let before = fixture.snapshot();
            let WorkerError::ConstructionCleanup {
                source,
                allocation,
                sessions,
            } = error
            else {
                panic!("cleanup ownership lost");
            };
            assert!(matches!(
                *source,
                WorkerError::RestoreCandidateChanged {
                    component: "metadata",
                    ..
                }
            ));
            let allocation =
                allocation.expect("allocation retained after second validation failure");
            assert!(
                worker_allocation::LockFileGuard::open(
                    &fixture.temp.path().join("allocation.json")
                )
                .unwrap()
                .data()
                .find(&fixture.manifest.worker.name)
                .is_some()
            );
            assert!(matches!(
                fixture
                    .store
                    .delete_uploaded_file(fixture.session_id, &fixture.orphan.artifact_id),
                Err(StoreError::ArtifactAlreadyCommitted)
            ));
            sessions.close_all().await.unwrap();
            worker_allocation::fail_next_save_for_test(
                &fixture.temp.path().join("allocation.json"),
            );
            assert!(allocation.release().is_err());
            allocation.release().unwrap();
            assert_eq!(fixture.snapshot(), before);
        }

        #[tokio::test]
        async fn pending_session_is_rejected_without_repair_or_session_creation() {
            let fixture = Fixture::new().await;
            fixture
                .store
                .update_by_name(&fixture.manifest.worker.name, |metadata| {
                    metadata.active.as_mut().unwrap().segment_id = None;
                })
                .unwrap();
            let before = fixture.snapshot();
            let result =
                Worker::prepare_restored_from_worker_metadata_with_context_and_model_client(
                    &fixture.manifest.worker.name,
                    fixture.manifest.clone(),
                    fixture.store.clone(),
                    PromptCatalogSource::builtins_only(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::None,
                    Some(Box::new(CountedClient(fixture.model_calls.clone()))),
                )
                .await;
            assert!(matches!(
                result,
                Err(WorkerError::WorkerMetadataPending { .. })
            ));
            assert_eq!(fixture.snapshot(), before);
            assert_eq!(fixture.model_calls.load(Ordering::SeqCst), 0);
        }
    }

    // Constructor finalization uses real Worker, allocation, Workdir, and storage
    // implementations. Only the model and metadata-write failure boundary are substituted.
    mod construction_cleanup_tests {
        use super::*;

        type ConstructionStore =
            session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

        async fn constructed_worker(
            temp: &tempfile::TempDir,
        ) -> Worker<Box<dyn LlmClient>, ConstructionStore> {
            let store = session_store::CombinedStore::new(
                session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
            );
            let mut worker = Worker::new(
                minimal_manifest(),
                Engine::<Box<dyn LlmClient>, Mutable, SessionHistoryMetadata>::new_annotated(
                    Box::new(NoopClient),
                ),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::local(
                    temp.path().to_path_buf(),
                    temp.path().to_path_buf(),
                ),
                Scope::writable(temp.path()).unwrap(),
            )
            .await
            .unwrap();
            worker.scope_allocation = Some(
                worker_allocation::install_top_level_at_for_test(
                    temp.path().join("workers.json"),
                    worker.manifest.worker.name.clone(),
                    std::process::id(),
                    temp.path().join("worker.sock"),
                    Vec::new(),
                    worker.segment_id(),
                )
                .unwrap(),
            );
            worker
        }

        #[tokio::test]
        async fn post_construction_failures_retain_resources_through_failed_release_retry() {
            for stage in [
                "pending_metadata",
                "restore_pins",
                "restore_metadata",
                "restore_delegations",
            ] {
                let temp = tempfile::tempdir().unwrap();
                let mut worker = constructed_worker(&temp).await;
                if stage != "pending_metadata" {
                    worker.ensure_segment_head().await.unwrap();
                }
                let name = worker.manifest.worker.name.clone();
                let lock_path = temp.path().join("workers.json");
                let sessions = Arc::downgrade(&worker.workdir_sessions());
                let session = worker.workdir_sessions().resolve(None).unwrap().session;
                let writes = Arc::new(AtomicUsize::new(0));
                let writer_calls = writes.clone();
                let metadata_path = temp
                    .path()
                    .join("workers")
                    .join(&name)
                    .join("metadata.json");
                worker.worker_metadata_writer = Some(Arc::new(move |_| {
                    writer_calls.fetch_add(1, Ordering::SeqCst);
                    if stage == "restore_delegations" {
                        // A successful publication followed by an unreadable retained
                        // record forces the subsequent delegation reconciliation to fail.
                        std::fs::create_dir_all(&metadata_path)?;
                        Ok(())
                    } else {
                        Err(WorkerStoreError::Io(std::io::Error::other(
                            "metadata write refused",
                        )))
                    }
                }));
                if stage == "restore_pins" {
                    let pins_dir = temp
                        .path()
                        .join("sessions")
                        .join(worker.session_id().to_string())
                        .join("artifacts")
                        .join("paste");
                    std::fs::create_dir_all(pins_dir.join(".aggregate.lock")).unwrap();
                }
                let result = if stage == "pending_metadata" {
                    worker.finish_pending_construction()
                } else {
                    worker.finish_restored_construction().await
                };
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("{stage} must fail after Worker construction"),
                };
                let WorkerError::ConstructionCleanup {
                    source,
                    allocation,
                    sessions: retained,
                } = &error
                else {
                    panic!("{stage} lost construction cleanup authority: {error:?}");
                };
                if stage == "restore_pins" {
                    assert!(matches!(
                        source.as_ref(),
                        WorkerError::Store(StoreError::Io(_))
                    ));
                    assert_eq!(writes.load(Ordering::SeqCst), 0);
                } else {
                    assert!(matches!(
                        source.as_ref(),
                        WorkerError::WorkerStore(WorkerStoreError::Io(_))
                    ));
                    assert_eq!(writes.load(Ordering::SeqCst), 1);
                }
                assert_eq!(error.to_string(), source.to_string());
                assert_eq!(
                    std::error::Error::source(&error).unwrap().to_string(),
                    source.to_string()
                );
                assert!(Arc::ptr_eq(&sessions.upgrade().unwrap(), retained));
                let allocation = allocation.clone().expect("constructed Worker's allocation");
                let retained = retained.clone();
                drop(error);
                {
                    let table = worker_allocation::LockFileGuard::open(&lock_path).unwrap();
                    assert!(
                        table.data().find(&name).is_some(),
                        "{stage} released during Worker Drop"
                    );
                }
                assert!(
                    session
                        .stat(workdir::StatRequest {
                            path: workdir::WorkdirPath::root()
                        })
                        .await
                        .is_ok()
                );
                retained.close_all().await.unwrap();
                assert!(retained.is_empty());
                assert!(
                    session
                        .stat(workdir::StatRequest {
                            path: workdir::WorkdirPath::root()
                        })
                        .await
                        .is_err()
                );

                worker_allocation::fail_next_save_for_test(&lock_path);
                assert!(matches!(allocation.release(), Err(ScopeLockError::Io(_))));
                {
                    let table = worker_allocation::LockFileGuard::open(&lock_path).unwrap();
                    assert!(
                        table.data().find(&name).is_some(),
                        "{stage} lost failed release"
                    );
                }
                allocation.release().unwrap();
                allocation.clone().release().unwrap();
                let table = worker_allocation::LockFileGuard::open(&lock_path).unwrap();
                assert!(table.data().find(&name).is_none());
            }
        }

        #[tokio::test]
        async fn malformed_restore_state_is_rejected_before_filesystem_preparation() {
            for corrupt_flow in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let store = session_store::CombinedStore::new(
                    session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                    session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
                );
                let session_id = session_store::new_session_id();
                let segment_id = session_store::new_segment_id();
                let mut entries = vec![LogEntry::AnnotatedSegmentStart {
                    ts: 0,
                    session_id,
                    system_prompt: Some("saved prompt".into()),
                    config: RequestConfig::default(),
                    history: Vec::new(),
                    forked_from: None,
                    compacted_from: None,
                }];
                if corrupt_flow {
                    entries.push(LogEntry::Extension {
                        ts: 1,
                        domain: FLOW_RUNTIME_EXTENSION_DOMAIN.into(),
                        payload: serde_json::Value::Null,
                    });
                } else {
                    entries.push(LogEntry::ToolResultCorrected {
                        ts: 1,
                        entry: to_logged_history_entry(&history_entry(
                            Item::user_message("correction without a target"),
                            WorkerHistoryProvenance::LegacyUnknown,
                        )),
                    });
                }
                store
                    .create_segment(session_id, segment_id, &entries)
                    .unwrap();
                let result = Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                    session_id,
                    segment_id,
                    minimal_manifest(),
                    store,
                    PromptCatalogSource::builtins_only(),
                    WorkerWorkspaceContext::no_workspace(),
                    // If preparation runs first, it must reject this missing root,
                    // without touching the process-global allocation table.
                    WorkerFilesystemAuthority::local(temp.path().join("missing"), temp.path().join("missing")),
                    Some(Box::new(NoopClient)),
                ).await;
                let error = match result {
                    Err(error) => error,
                    Ok(_) => panic!("corrupt restore state must be rejected"),
                };
                let WorkerError::InvalidState(message) = error else {
                    panic!("restore parsed state too late: {error:?}");
                };
                assert!(message.contains(if corrupt_flow {
                    "Flow runtime state"
                } else {
                    "typed Worker session history"
                }));
            }
        }
    }

    // Real Feature installation, task registries, Workdir provider/router, and
    // PreparedWorker startup; only the model and task completion barrier are test implementations.
    mod startup_feature_cleanup_tests {
        use super::*;
        use crate::feature::background::{
            BackgroundTaskCancellation, BackgroundTaskContext, BackgroundTaskSpec,
            FeatureBackgroundTask,
        };
        use crate::feature::{
            BackgroundTaskDeclaration, FeatureDescriptor, FeatureId, FeatureInstallContext,
            FeatureInstallError, FeatureModule,
        };
        use crate::hook::{HookError, HookErrorCategory};
        use tokio::sync::Notify;

        type StartupStore =
            session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

        #[derive(Default)]
        struct TaskSignals {
            entered: Notify,
            cancelled: AtomicBool,
            release: Notify,
            exited: AtomicBool,
            session: Mutex<Option<workdir::WorkdirSessionHandle>>,
        }

        #[derive(Clone)]
        struct BarrierTask {
            signals: Arc<TaskSignals>,
            _lifetime: Arc<()>,
            panic_on_release: bool,
        }

        #[async_trait]
        impl FeatureBackgroundTask for BarrierTask {
            async fn run(
                &self,
                _context: BackgroundTaskContext,
                cancellation: BackgroundTaskCancellation,
            ) -> Result<(), HookError> {
                self.signals.entered.notify_one();
                cancellation.cancelled().await;
                self.signals.cancelled.store(true, Ordering::Release);
                self.signals.release.notified().await;
                assert!(!self.panic_on_release, "injected startup task join failure");
                let session = self.signals.session.lock().unwrap().clone();
                if let Some(session) = session {
                    assert!(
                        session_open(&session).await,
                        "Feature task must retain Workdir access until it is quiescent"
                    );
                }
                self.signals.exited.store(true, Ordering::Release);
                Ok(())
            }
        }

        struct BarrierFeature(BarrierTask);

        impl FeatureModule for BarrierFeature {
            fn descriptor(&self) -> FeatureDescriptor {
                FeatureDescriptor::builtin("startup-barrier", "Startup barrier")
                    .with_background_task(BackgroundTaskDeclaration::worker_managed(
                        "barrier",
                        "Startup task completion barrier",
                    ))
            }

            fn install(
                &self,
                context: &mut FeatureInstallContext<'_>,
            ) -> Result<(), FeatureInstallError> {
                context.background_tasks().register(
                    BackgroundTaskSpec::single_flight(
                        BackgroundTaskDeclaration::worker_managed(
                            "barrier",
                            "Startup task completion barrier",
                        ),
                        Duration::from_secs(60),
                    ),
                    self.0.clone(),
                )
            }
        }

        async fn worker(temp: &tempfile::TempDir) -> Worker<NoopClient, StartupStore> {
            let store = session_store::CombinedStore::new(
                session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
            );
            Worker::new(
                minimal_manifest(),
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap()
        }

        fn install(
            worker: &mut Worker<NoopClient, StartupStore>,
            panic_on_release: bool,
        ) -> (Arc<TaskSignals>, std::sync::Weak<()>) {
            let signals = Arc::new(TaskSignals::default());
            let lifetime = Arc::new(());
            let weak = Arc::downgrade(&lifetime);
            let report = worker.install_features(FeatureRegistryBuilder::new().with_module(
                BarrierFeature(BarrierTask {
                    signals: signals.clone(),
                    _lifetime: lifetime,
                    panic_on_release,
                }),
            ));
            assert!(!report.has_errors());
            (signals, weak)
        }

        async fn start(worker: &Worker<NoopClient, StartupStore>, signals: &TaskSignals) {
            let registry = worker.feature_background_tasks.clone();
            registry
                .start(
                    &FeatureId::builtin("startup-barrier"),
                    "barrier",
                    HookInvocationContext {
                        workspace_id: None,
                        worker_id: "startup-owner".into(),
                        session_id: worker.session_id().to_string(),
                        run_id: None,
                        turn_index: None,
                        call_id: None,
                    },
                )
                .unwrap();
            // A transient registry clone must not cancel or abort the task:
            // the Worker and startup capture still own the shared registry.
            drop(registry);
            tokio::time::timeout(Duration::from_secs(5), signals.entered.notified())
                .await
                .unwrap();
        }

        fn bind_session(
            worker: &mut Worker<NoopClient, StartupStore>,
            temp: &tempfile::TempDir,
        ) -> workdir::WorkdirSessionHandle {
            let session: workdir::WorkdirSessionHandle =
                Arc::new(workdir::LocalWorkdirSession::materialized_bound(
                    workdir::Workdir::new("startup-task-cleanup"),
                    temp.path().to_path_buf(),
                    temp.path().to_path_buf(),
                    SharedScope::new(Scope::writable(temp.path()).unwrap()),
                    workdir::WorkdirSessionCapabilities::ALL,
                ));
            worker.bind_single_workdir_session(Some(session.clone()));
            session
        }

        async fn session_open(session: &workdir::WorkdirSessionHandle) -> bool {
            session
                .stat(workdir::StatRequest {
                    path: workdir::WorkdirPath::root(),
                })
                .await
                .is_ok()
        }

        async fn fail_consuming_start(
            worker: Worker<NoopClient, StartupStore>,
            temp: &tempfile::TempDir,
        ) {
            let factory_owned_cleanup = worker.startup_feature_cleanup_handle().is_some();
            let blocked_parent = temp.path().join("blocked-runtime");
            std::fs::write(&blocked_parent, "not a directory").unwrap();
            let result = crate::PreparedWorker::new(
                worker,
                crate::WorkerBootstrapLayout::RuntimeManagedRun {
                    run_dir: blocked_parent.join("1"),
                },
            )
            .start()
            .await;
            let Err(crate::WorkerBootstrapError::Controller {
                source,
                cleanup_failed,
            }) = result
            else {
                panic!("runtime parent file must reject controller startup");
            };
            assert!(matches!(
                source.kind(),
                std::io::ErrorKind::NotADirectory | std::io::ErrorKind::AlreadyExists
            ));
            assert_eq!(cleanup_failed, factory_owned_cleanup);
        }

        #[tokio::test]
        async fn consumed_worker_failure_retains_all_task_joins_across_cleanup_timeout() {
            let temp = tempfile::tempdir().unwrap();
            let mut worker = worker(&temp).await;
            let session = bind_session(&mut worker, &temp);
            let sessions = worker.workdir_sessions();
            let (first, first_lifetime) = install(&mut worker, false);
            *first.session.lock().unwrap() = Some(session.clone());
            start(&worker, &first).await;
            let cleanup = worker.retain_startup_feature_cleanup();
            let retry_owner = cleanup.clone();
            let (second, second_lifetime) = install(&mut worker, false);
            *second.session.lock().unwrap() = Some(session.clone());
            start(&worker, &second).await;
            fail_consuming_start(worker, &temp).await;

            assert!(!sessions.aliases().is_empty());
            assert!(session_open(&session).await);
            assert!(first_lifetime.upgrade().is_some());
            assert!(second_lifetime.upgrade().is_some());
            assert!(
                tokio::time::timeout(Duration::from_millis(50), cleanup.shutdown())
                    .await
                    .is_err()
            );
            assert!(first.cancelled.load(Ordering::Acquire));
            drop(cleanup);

            // A fresh shutdown of the drained registry would falsely succeed
            // once the second task finishes, abandoning the first task's join.
            second.release.notify_one();
            assert!(
                tokio::time::timeout(Duration::from_millis(50), retry_owner.shutdown())
                    .await
                    .is_err()
            );
            assert!(!first.exited.load(Ordering::Acquire));
            assert!(session_open(&session).await);
            first.release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), retry_owner.shutdown())
                .await
                .unwrap()
                .unwrap();
            assert!(first.exited.load(Ordering::Acquire));
            assert!(second.exited.load(Ordering::Acquire));
            assert!(session_open(&session).await);
            sessions.close_all().await.unwrap();
            assert!(sessions.aliases().is_empty());
            assert!(!session_open(&session).await);
            drop(retry_owner);
            assert!(first_lifetime.upgrade().is_none());
            assert!(second_lifetime.upgrade().is_none());
        }

        #[tokio::test]
        async fn startup_without_factory_cleanup_owner_still_closes_sessions_on_error() {
            let temp = tempfile::tempdir().unwrap();
            let mut worker = worker(&temp).await;
            let session = bind_session(&mut worker, &temp);
            let sessions = worker.workdir_sessions();
            assert!(session_open(&session).await);
            fail_consuming_start(worker, &temp).await;
            assert!(sessions.aliases().is_empty());
            assert!(!session_open(&session).await);
        }

        #[tokio::test]
        async fn cleanup_error_settles_other_registries_and_keeps_owner_for_retry() {
            let temp = tempfile::tempdir().unwrap();
            let mut worker = worker(&temp).await;
            let (first, first_lifetime) = install(&mut worker, true);
            start(&worker, &first).await;
            let cleanup = worker.retain_startup_feature_cleanup();
            let (second, second_lifetime) = install(&mut worker, false);
            start(&worker, &second).await;
            fail_consuming_start(worker, &temp).await;
            first.release.notify_one();
            second.release.notify_one();

            let error = tokio::time::timeout(Duration::from_secs(5), cleanup.shutdown())
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(error.category, HookErrorCategory::Internal);
            assert!(second.exited.load(Ordering::Acquire));
            assert!(first_lifetime.upgrade().is_some());
            assert!(second_lifetime.upgrade().is_some());
            tokio::time::timeout(Duration::from_secs(5), cleanup.shutdown())
                .await
                .unwrap()
                .unwrap();
            drop(cleanup);
            assert!(first_lifetime.upgrade().is_none());
            assert!(second_lifetime.upgrade().is_none());
        }

        #[tokio::test]
        async fn successful_controller_transfer_releases_old_startup_task_scopes() {
            let temp = tempfile::tempdir().unwrap();
            let mut worker = worker(&temp).await;
            let (_signals, old_scope) = install(&mut worker, false);
            let cleanup = worker.retain_startup_feature_cleanup();
            let started = crate::PreparedWorker::new(
                worker,
                crate::WorkerBootstrapLayout::RuntimeManagedRun {
                    run_dir: temp.path().join("runs/1"),
                },
            )
            .start()
            .await
            .unwrap();

            assert!(old_scope.upgrade().is_some());
            drop(cleanup);
            let released = old_scope.upgrade().is_none();
            stop_body_publication_controller(started).await;
            assert!(
                released,
                "live Controller must not retain factory startup capture"
            );
        }
    }

    #[tokio::test]
    async fn post_metadata_allocation_failure_fences_appends_and_restart_converges() {
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
        worker.ensure_segment_head().await.unwrap();
        let old_segment = worker.segment_id();
        let replacement = session_store::new_segment_id();
        let old_location = SegmentLocation {
            session_id: worker.session_id(),
            segment_id: old_segment,
        };
        let replacement_location = SegmentLocation {
            session_id: worker.session_id(),
            segment_id: replacement,
        };
        let persisted_metadata = Arc::new(Mutex::new(WorkerActiveSegmentRef::active_segment(
            old_location.session_id,
            old_location.segment_id,
        )));
        let metadata_for_cas = Arc::clone(&persisted_metadata);
        worker.worker_metadata_segment_cas =
            Some(Arc::new(move |_worker_name, expected, replacement| {
                let mut persisted = metadata_for_cas.lock().unwrap();
                if &*persisted != expected {
                    return Ok(false);
                }
                *persisted = replacement;
                Ok(true)
            }));
        let lock_path = dir.path().join("workers.json");
        let allocation = worker_allocation::install_top_level_at_for_test(
            lock_path.clone(),
            "test-worker".into(),
            std::process::id(),
            dir.path().join("worker.sock"),
            Vec::new(),
            old_segment,
        )
        .unwrap();
        let activation = allocation
            .begin_segment_activation(old_segment, replacement)
            .unwrap();
        worker.scope_allocation = Some(allocation);
        worker
            .compare_and_swap_worker_metadata_segment(old_location, replacement_location)
            .unwrap();
        assert_eq!(
            *persisted_metadata.lock().unwrap(),
            WorkerActiveSegmentRef::active_segment(
                replacement_location.session_id,
                replacement_location.segment_id,
            )
        );
        worker_allocation::fail_next_save_for_test(&lock_path);
        let (source, authority) = activation.commit().unwrap_err().into_parts();
        assert!(matches!(source, ScopeLockError::Io(_)));
        worker.fail_closed_segment_activation(authority);
        assert!(worker.segment_state.ensure_append_allowed().is_err());

        let (sent, received) = std::sync::mpsc::channel();
        let teardown = std::thread::spawn(move || {
            drop(worker);
            sent.send(()).unwrap();
        });
        received
            .recv_timeout(Duration::from_secs(1))
            .expect("Worker teardown must not deadlock on nested allocation locks");
        teardown.join().unwrap();

        // Process-stop releases the retained authority and the old allocation;
        // restart follows the replacement named by committed metadata.
        let recovered = worker_allocation::install_top_level_at_for_test(
            lock_path,
            "test-worker".into(),
            std::process::id(),
            dir.path().join("worker-restored.sock"),
            Vec::new(),
            replacement,
        )
        .unwrap();
        drop(
            recovered
                .begin_segment_activation(replacement, session_store::new_segment_id())
                .unwrap(),
        );
        drop(recovered);
    }

    #[tokio::test]
    async fn crash_after_allocation_before_live_switch_recovers_committed_replacement() {
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
        worker.ensure_segment_head().await.unwrap();
        let old_location = worker.segment_state.location();
        let replacement_location = SegmentLocation {
            session_id: old_location.session_id,
            segment_id: session_store::new_segment_id(),
        };
        let persisted_metadata = Arc::new(Mutex::new(WorkerActiveSegmentRef::active_segment(
            old_location.session_id,
            old_location.segment_id,
        )));
        let metadata_for_cas = Arc::clone(&persisted_metadata);
        worker.worker_metadata_segment_cas =
            Some(Arc::new(move |_worker_name, expected, replacement| {
                let mut persisted = metadata_for_cas.lock().unwrap();
                if &*persisted != expected {
                    return Ok(false);
                }
                *persisted = replacement;
                Ok(true)
            }));
        let lock_path = dir.path().join("workers.json");
        let allocation = worker_allocation::install_top_level_at_for_test(
            lock_path.clone(),
            "test-worker".into(),
            std::process::id(),
            dir.path().join("worker.sock"),
            Vec::new(),
            old_location.segment_id,
        )
        .unwrap();
        let activation = allocation
            .begin_segment_activation(old_location.segment_id, replacement_location.segment_id)
            .unwrap();
        worker.scope_allocation = Some(allocation);
        worker
            .compare_and_swap_worker_metadata_segment(old_location, replacement_location)
            .unwrap();
        activation.commit().unwrap();

        // Simulated crash point: durable metadata and allocation moved, while the
        // in-memory append destination still names the source Segment.
        assert_eq!(worker.segment_id(), old_location.segment_id);
        assert_eq!(
            *persisted_metadata.lock().unwrap(),
            WorkerActiveSegmentRef::active_segment(
                replacement_location.session_id,
                replacement_location.segment_id,
            )
        );
        drop(worker);

        let recovered = worker_allocation::install_top_level_at_for_test(
            lock_path,
            "test-worker".into(),
            std::process::id(),
            dir.path().join("worker-restored.sock"),
            Vec::new(),
            replacement_location.segment_id,
        )
        .unwrap();
        drop(
            recovered
                .begin_segment_activation(
                    replacement_location.segment_id,
                    session_store::new_segment_id(),
                )
                .unwrap(),
        );
        drop(recovered);
    }

    #[tokio::test]
    async fn shutdown_cleanup_stays_blocked_until_registered_service_stops() {
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
        let registry = crate::spawn::registry::SpawnedWorkerRegistry::new_for_internal_services();
        let (cleanup_session_id, _events) = registry.install_service_for_test();
        registry.fail_service_stops_for_test(&cleanup_session_id, 4);
        worker.internal_worker_registry = Some(Arc::clone(&registry));
        let (event_tx, mut event_rx) = broadcast::channel(16);
        worker.attach_working_event_tx(event_tx.clone());
        let in_flight = InFlightEvents::new(event_tx);
        worker.attach_in_flight_events(in_flight.clone());
        worker.set_compaction_progress(Some(InFlightCompaction {
            phase: CompactionPhase::Summarizing,
            started_at_ms: 1,
            trigger: CompactionTrigger::Manual,
        }));
        *worker
            .pending_compaction_cleanup
            .lock()
            .expect("pending cleanup lock poisoned") = Some(PendingCompactionCleanup {
            session_id: cleanup_session_id.clone(),
            compaction_id: "compaction".into(),
        });

        let cleanup = worker.finish_pending_compaction_cleanup_for_shutdown();
        tokio::pin!(cleanup);
        tokio::select! {
            result = &mut cleanup => panic!("shutdown cleanup completed before injected failures: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(75)) => {}
        }
        assert!(worker.has_pending_compaction_cleanup());
        assert!(registry.has_service_for_test(&cleanup_session_id));
        let in_flight_guard = in_flight.snapshot_guard();
        assert!(
            crate::in_flight::snapshot_from_guard(&in_flight_guard)
                .compaction
                .is_some(),
            "failed cleanup retries must keep reconnect compaction state active"
        );
        drop(in_flight_guard);
        while let Ok(event) = event_rx.try_recv() {
            assert!(
                !matches!(event, Event::CompactionProgress { compaction: None }),
                "terminal progress must not publish before the service stops"
            );
        }
        tokio::time::timeout(Duration::from_secs(1), cleanup)
            .await
            .expect("shutdown cleanup must converge after retry")
            .unwrap();
        assert!(!worker.has_pending_compaction_cleanup());
        assert!(!registry.has_service_for_test(&cleanup_session_id));
        let in_flight_guard = in_flight.snapshot_guard();
        assert!(
            crate::in_flight::snapshot_from_guard(&in_flight_guard)
                .compaction
                .is_none(),
            "successful cleanup retry must clear reconnect compaction state"
        );
        drop(in_flight_guard);
        let mut saw_clear = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, Event::CompactionProgress { compaction: None }) {
                saw_clear = true;
            }
        }
        assert!(
            saw_clear,
            "successful cleanup retry must publish terminal progress"
        );
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
        worker.ensure_segment_head().await.unwrap();
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

        worker.ensure_segment_head().await.unwrap();

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
                LogEntry::InputSegmentsCheckpoint { .. },
                LogEntry::ActiveRunCheckpoint {
                    active_turn_count: 3,
                    total_turn_count: 7,
                    ..
                },
                LogEntry::Extension { domain: protected, payload, .. },
                LogEntry::Extension { domain, .. }
            ] if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN
                && protected == PROTECTED_INPUTS_DOMAIN
                && payload == &serde_json::json!({"version":1, "kinds":[]})
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
        worker.ensure_segment_head().await.unwrap();
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
        worker.ensure_segment_head().await.unwrap();
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
        let projected_ids = vec![SessionHistoryEntryId::new(), SessionHistoryEntryId::new()];
        let input_provenance = WorkerHistoryProvenance::HumanInput {
            account_id: "account-1".into(),
        };
        let projected = worker.projected_input_history(
            &segments,
            projection.as_ref(),
            &projected_ids,
            &input_provenance,
        );
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
        assert_eq!(projected[1].annotation.origin, input_provenance);
        assert_eq!(
            state.instance.definition_digest,
            state.definition.content_digest
        );
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

    #[tokio::test]
    async fn uploaded_file_is_verified_and_bound_to_projected_entry_before_commit() {
        let (_dir, worker) = rewind_test_worker().await;
        let reference = worker
            .store
            .write_uploaded_file(
                worker.session_id(),
                "notes.md",
                "text/markdown",
                b"# private body",
                session_store::UploadedFileLimits::default(),
            )
            .unwrap();
        let entry_id = SessionHistoryEntryId::new();
        let mut input = vec![Segment::UploadedFile { file: reference }];

        worker
            .materialize_large_pastes(&mut input, std::slice::from_ref(&entry_id), false)
            .unwrap();
        let file = match &input[0] {
            Segment::UploadedFile { file } => file,
            other => panic!("expected uploaded file, got {other:?}"),
        };
        assert_eq!(file.source_entry_id.as_deref(), Some(entry_id.0.as_str()));
        assert_eq!(
            worker
                .store
                .read_uploaded_file(worker.session_id(), file)
                .unwrap(),
            b"# private body"
        );
        assert!(matches!(
            worker
                .store
                .delete_uploaded_file(worker.session_id(), &file.artifact_id),
            Err(StoreError::ArtifactAlreadyCommitted)
        ));
        let projected = worker.projected_input_history(
            &input,
            None,
            &[entry_id],
            &WorkerHistoryProvenance::LegacyUnknown,
        );
        let text = projected[0].item.as_text().unwrap();
        assert!(text.contains("notes.md"));
        assert!(text.contains(&file.artifact_id));
        assert!(!text.contains("private body"));

        let mut forged = input.clone();
        let Segment::UploadedFile { file } = &mut forged[0] else {
            unreachable!();
        };
        file.source_entry_id = None;
        file.sha256 = "0".repeat(64);
        assert!(matches!(
            worker.materialize_large_pastes(&mut forged, &[SessionHistoryEntryId::new()], false),
            Err(WorkerError::Store(StoreError::ArtifactIntegrityMismatch))
        ));
    }

    #[tokio::test]
    async fn large_paste_is_stored_before_compact_history_is_committed() {
        let (_dir, worker) = rewind_test_worker().await;
        let exact = "x".repeat(LARGE_PASTE_INLINE_MAX_BYTES);
        let exact_ids = vec![SessionHistoryEntryId::new()];
        let mut exact_input = vec![Segment::Paste {
            id: 1,
            chars: exact.len() as u32,
            lines: 1,
            content: exact.clone(),
        }];
        worker
            .materialize_large_pastes(&mut exact_input, &exact_ids, false)
            .unwrap();
        assert!(matches!(&exact_input[0], Segment::Paste { content, .. } if content == &exact));
        let mut empty_input = vec![Segment::Paste {
            id: 0,
            chars: 0,
            lines: 0,
            content: String::new(),
        }];
        worker
            .materialize_large_pastes(&mut empty_input, &exact_ids, false)
            .unwrap();
        assert!(matches!(
            &empty_input[0],
            Segment::Paste { content, .. } if content.is_empty()
        ));

        let body = format!("{}\n終端\n", "多".repeat(12_000));
        let entry_id = SessionHistoryEntryId::new();
        let mut input = vec![Segment::Paste {
            id: 2,
            chars: body.chars().count() as u32,
            lines: 3,
            content: body.clone(),
        }];
        worker
            .materialize_large_pastes(&mut input, std::slice::from_ref(&entry_id), false)
            .unwrap();
        let artifact = match &input[0] {
            Segment::PasteArtifact { artifact } => artifact.clone(),
            other => panic!("expected stored paste reference, got {other:?}"),
        };
        assert_eq!(artifact.source_entry_id, entry_id.0);
        assert_eq!(artifact.byte_len, body.len() as u64);
        assert_eq!(artifact.char_count, body.chars().count() as u64);
        assert_eq!(
            worker
                .store
                .read_paste_artifact(worker.session_id(), &artifact.artifact_id)
                .unwrap()
                .1,
            body
        );
        worker
            .materialize_large_pastes(&mut input, &[SessionHistoryEntryId::new()], false)
            .unwrap();
        assert!(matches!(
            &input[0],
            Segment::PasteArtifact { artifact: retained }
                if retained.source_entry_id == artifact.source_entry_id
        ));

        let history = worker.projected_input_history(
            &input,
            None,
            &[entry_id],
            &WorkerHistoryProvenance::LegacyUnknown,
        );
        assert!(!history[0].item.as_text().unwrap().contains("終端"));
        append_test_entry(
            &worker,
            LogEntry::Invoke {
                ts: segment_log::now_millis(),
                trigger: protocol::InvokeKind::UserSend,
            },
        );
        worker
            .commit_entry(LogEntry::AnnotatedUserInput {
                ts: segment_log::now_millis(),
                segments: input.clone(),
                extensions: Vec::new(),
                history: history.iter().map(to_logged_history_entry).collect(),
            })
            .unwrap();
        let location = worker.segment_state.location();
        let entries = worker
            .store
            .read_all(location.session_id, location.segment_id)
            .unwrap();
        let persisted = serde_json::to_string(&entries).unwrap();
        assert!(!persisted.contains("終端"));
        let state = session_store::collect_state(&entries);
        assert!(matches!(
            &state.user_segments[0][0],
            Segment::PasteArtifact { artifact: restored }
                if restored.artifact_id == artifact.artifact_id
        ));
    }

    #[tokio::test]
    async fn large_paste_storage_failure_commits_no_input() {
        let temp = tempfile::TempDir::new().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::unavailable(None, "test unavailable"),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();
        std::fs::write(
            temp.path()
                .join(worker.session_id().to_string())
                .join("artifacts"),
            "block artifact directory creation",
        )
        .unwrap();
        let result = worker
            .run(vec![Segment::Paste {
                id: 1,
                chars: (LARGE_PASTE_INLINE_MAX_BYTES + 1) as u32,
                lines: 1,
                content: "x".repeat(LARGE_PASTE_INLINE_MAX_BYTES + 1),
            }])
            .await;
        assert!(matches!(result, Err(WorkerError::Store(StoreError::Io(_)))));
        let location = worker.segment_state.location();
        let entries = store
            .read_all(location.session_id, location.segment_id)
            .unwrap();
        assert!(
            !entries
                .iter()
                .any(|entry| matches!(entry, LogEntry::AnnotatedUserInput { .. }))
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
        worker.ensure_segment_head().await.unwrap();
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
            LogEntry::AnnotatedUserInput {
                ts: ts + 1,
                extensions: vec![],
                history: vec![crate::session_history::test_logged_history_entry(
                    Item::user_message(text),
                )],
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

    fn rewind_test_inputs<C: LlmClient + 'static, St: Store>(
        worker: &Worker<C, St>,
    ) -> Vec<(String, usize)> {
        let loc = worker.segment_state.location();
        let entries = worker
            .store
            .read_all(loc.session_id, loc.segment_id)
            .unwrap();
        let public = session_store::public_snapshot::project_current_session_snapshot(&entries);
        public
            .entries
            .iter()
            .filter_map(|entry| {
                if !matches!(
                    entry.data,
                    protocol::SessionSnapshotEntryData::UserInput { .. }
                ) {
                    return None;
                }
                let index = entries.iter().position(|log| {
                    matches!(log, LogEntry::AnnotatedUserInput { history, .. }
                if history.iter().any(|h| h.metadata.entry_id.0 == entry.entry_id))
                })?;
                Some((
                    entry.entry_id.clone(),
                    rewind_truncate_entries(&entries, index),
                ))
            })
            .rev()
            .collect()
    }

    #[tokio::test]
    async fn rewind_resolves_duplicate_text_by_public_entry_identity() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "same input");
        append_user_turn(&worker, 20, "same input");
        let inputs = rewind_test_inputs(&worker);
        assert_ne!(inputs[0].0, inputs[1].0);
        worker.rewind_to(inputs[0].0.clone()).await.unwrap();
        assert_eq!(rewind_test_inputs(&worker), vec![inputs[1].clone()]);
    }

    #[tokio::test]
    async fn rewind_rejects_unknown_foreign_and_non_user_ids_without_changing_log() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let (_other_dir, other) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "same input");
        append_user_turn(&other, 10, "same input");
        let assistant =
            crate::session_history::test_logged_history_entry(session_store::LoggedItem::Message {
                role: session_store::LoggedRole::Assistant,
                content: vec![session_store::LoggedContentPart::Text {
                    text: "answer".into(),
                }],
            });
        let assistant_id = assistant.metadata.entry_id.0.clone();
        append_test_entry(
            &worker,
            LogEntry::AnnotatedAssistantItem {
                ts: 12,
                entry: assistant,
            },
        );
        let loc = worker.segment_state.location();
        let before = worker
            .store
            .read_all(loc.session_id, loc.segment_id)
            .unwrap();
        for id in [
            String::new(),
            "missing-entry".into(),
            rewind_test_inputs(&other)[0].0.clone(),
            assistant_id,
        ] {
            assert!(
                worker
                    .rewind_to(id)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("not a user input")
            );
            assert_eq!(
                serde_json::to_value(
                    worker
                        .store
                        .read_all(loc.session_id, loc.segment_id)
                        .unwrap()
                )
                .unwrap(),
                serde_json::to_value(&before).unwrap()
            );
        }
    }

    #[tokio::test]
    async fn rewind_rejects_user_input_from_seeded_ancestor_history() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let input = crate::session_history::test_logged_history_entry(
            session_store::LoggedItem::from(agen::Item::user_message("ancestor input")),
        );
        let id = input.metadata.entry_id.0.clone();
        let loc = worker.segment_state.location();
        let mut entries = worker
            .store
            .read_all(loc.session_id, loc.segment_id)
            .unwrap();
        let LogEntry::AnnotatedSegmentStart { history, .. } = &mut entries[0] else {
            panic!("segment start");
        };
        history.push(input);
        worker
            .store
            .create_segment(loc.session_id, loc.segment_id, &entries)
            .unwrap();
        assert!(
            worker
                .rewind_to(id)
                .await
                .unwrap_err()
                .to_string()
                .contains("current segment")
        );
        assert_eq!(
            serde_json::to_value(
                worker
                    .store
                    .read_all(loc.session_id, loc.segment_id)
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(entries).unwrap()
        );
    }

    #[tokio::test]
    async fn rewind_apply_truncates_log_and_restores_selected_input() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        append_user_turn(&worker, 20, "second message");
        append_test_entry(
            &worker,
            LogEntry::AnnotatedToolResult {
                ts: 30,
                entry: crate::session_history::test_logged_history_entry(
                    session_store::LoggedItem::ToolResult {
                        call_id: "call-1".into(),
                        summary: "wrote a file".into(),
                        content: None,
                        attachments: Vec::new(),
                        disposition: Default::default(),
                        is_error: false,
                    },
                ),
            },
        );
        let targets = rewind_test_inputs(&worker);
        let expected_truncate_entries = targets[0].1;
        let target = targets[0].0.clone();

        let applied = worker.rewind_to(target).await.unwrap();

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
            expected_truncate_entries + 1
        );
        assert_eq!(worker.history().len(), 1);
        assert_eq!(worker.history()[0].as_text().unwrap(), "first message");
    }

    #[tokio::test]
    async fn rewind_preserves_notification_only_pending_activation_checkpoint() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        append_user_turn(&worker, 20, "second message");
        worker
            .pending_submission_handle()
            .accept_notification("notification-1".into(), "keep me".into())
            .unwrap();
        let targets = rewind_test_inputs(&worker);

        worker
            .rewind_to(targets.last().unwrap().0.clone())
            .await
            .unwrap();

        let location = worker.segment_state.location();
        let entries = worker
            .store
            .read_all(location.session_id, location.segment_id)
            .unwrap();
        let restored: PendingActivationState = entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => {
                    serde_json::from_value(payload.clone()).ok()
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(restored.pending_notifications.len(), 1);
        assert_eq!(restored.notification_receipts.len(), 1);
        assert_eq!(
            restored.pending_notifications[0].notification_request_id,
            "notification-1"
        );
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

        let targets = rewind_test_inputs(&worker);
        assert_eq!(targets.len(), 2);
        assert!(targets[0].1 > 0);

        let applied = worker.rewind_to(targets[0].0.clone()).await.unwrap();
        assert_eq!(applied.summary.truncated_to_entries, 6);
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
        worker.ensure_segment_head().await.unwrap();
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
    async fn rewind_by_entry_id_includes_history_appended_after_selection() {
        let (_dir, mut worker) = rewind_test_worker().await;
        append_user_turn(&worker, 10, "first message");
        let targets = rewind_test_inputs(&worker);
        append_user_turn(&worker, 20, "newer message");

        let applied = worker.rewind_to(targets[0].0.clone()).await.unwrap();
        assert_eq!(preview_segments(&applied.input), "first message");
        assert!(rewind_test_inputs(&worker).is_empty());
        let err = worker.rewind_to(targets[0].0.clone()).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("not a user input in the current segment")
        );
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
            store.clone(),
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
        let paused_entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert!(
            !paused_entries
                .iter()
                .any(|entry| matches!(entry, LogEntry::RunResumed { .. }))
        );

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
        let resumed_entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        let resume_index = resumed_entries
            .iter()
            .position(|entry| {
                matches!(
                    entry,
                    LogEntry::RunResumed {
                        source: RunResumeSource::Pause,
                        ..
                    }
                )
            })
            .expect("pause resume transition");
        let terminal_index = resumed_entries
            .iter()
            .rposition(|entry| matches!(entry, LogEntry::RunCompleted { .. }))
            .expect("resumed run terminal");
        assert!(resume_index < terminal_index);
    }

    #[tokio::test]
    async fn cancelled_result_terminalizes_orphan_before_run_terminal() {
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

        worker.ensure_segment_head().await.unwrap();
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
                EngineRunExit::Interrupted(RunInterruptionReason::Cancelled),
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
                    LogEntry::RunCompleted { .. }
                        | LogEntry::RunCancelled { .. }
                        | LogEntry::RunErrored { .. }
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

        worker.ensure_segment_head().await.unwrap();
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

        worker.ensure_segment_head().await.unwrap();
        worker.wire_history_persistence();
        let dangling_call = Item::tool_call("call-1", "SideEffect", "{}");
        worker
            .commit_entry(LogEntry::AnnotatedAssistantItem {
                ts: segment_log::now_millis(),
                entry: crate::session_history::test_logged_history_entry(dangling_call.clone()),
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

    // Real Worker wiring, filesystem Store, log writer and reconnect buffers;
    // only the model client is inert because this test never invokes the engine.
    #[tokio::test]
    async fn alert_pending_text_anchor_resolves_at_durable_commit_before_replay() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        let (tx, _) = broadcast::channel(16);
        let in_flight = InFlightEvents::new(tx.clone());
        worker.in_flight = Some(in_flight.clone());
        let alerter = Alerter::new(tx);
        worker.attach_alerter(alerter.clone());
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "before head".into());
        assert_eq!(
            alerter.subscribe_with_snapshot().0[0].position,
            Some(protocol::AlertPosition::Start)
        );
        worker.materialize_durable_session_head().await.unwrap();
        let text = in_flight.start_text_block();
        in_flight.text_delta(text, "streamed".into());
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "during text".into());
        let (before, _) = alerter.subscribe_with_snapshot();
        let snapshot = crate::in_flight::snapshot_from_guard(&in_flight.snapshot_guard());
        assert_eq!(
            before[1].position,
            Some(protocol::AlertPosition::AfterInFlight {
                block_id: snapshot.block_ids[0]
            })
        );
        let (deferred, _) = alerter.subscribe_with_deferred_snapshot();
        let entry = crate::session_history::test_logged_history_entry(
            agen::Item::assistant_message("streamed"),
        );
        let entry_id = entry.metadata.entry_id.0.clone();
        worker
            .log_writer_handle()
            .append_entry(LogEntry::AnnotatedAssistantItem { ts: 42, entry })
            .unwrap();
        let (entries, _) = worker.sink.subscribe_with_snapshot();
        assert!(entries.iter().any(|entry| matches!(entry, LogEntry::AnnotatedAssistantItem { entry, .. } if entry.metadata.entry_id.0 == entry_id)));
        assert!(
            crate::in_flight::snapshot_from_guard(&in_flight.snapshot_guard())
                .blocks
                .is_empty()
        );
        let replay = deferred();
        assert_eq!(replay[1].id, before[1].id);
        assert_eq!(
            replay[1].position,
            Some(protocol::AlertPosition::AfterEntry {
                entry_id: entry_id.clone()
            })
        );
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "after commit".into());
        assert_eq!(
            alerter.subscribe_with_snapshot().0[2].position,
            Some(protocol::AlertPosition::AfterEntry { entry_id })
        );
    }

    #[tokio::test]
    async fn materialize_durable_session_head_commits_without_model_run() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();

        worker.materialize_durable_session_head().await.unwrap();

        assert_eq!(worker.segment_state.entries_written(), 2);
        assert!(worker.history().is_empty());
    }

    #[tokio::test]
    async fn worker_without_initial_system_prompt_does_not_load_feature_contribution() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        let (source, load_count) =
            test_system_prompt_contribution_source(Some("# Durable Memory".to_string()));
        worker.install_system_prompt_contribution(Some(source), None);

        worker.ensure_system_prompt_materialized().await.unwrap();

        assert_eq!(load_count.load(Ordering::SeqCst), 0);
        assert!(worker.history().is_empty());
    }

    #[tokio::test]
    async fn restored_worker_appends_latest_resident_surface_without_rewriting_history() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let runtime = dir.path().join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let _runtime =
            crate::runtime::worker_allocation::test_util::RuntimeDirSandbox::new(&runtime);
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(dir.path().join("workers")).unwrap(),
        );
        let mut manifest = WorkerManifest::from_toml(&format!(
            r#"
[worker]
name = "restore-resident-{}"

[model]
scheme = "anthropic"
model_id = "test-model"

[engine]
instruction = "default"

[[scope.allow]]
target = "{}"
permission = "write"
"#,
            uuid::Uuid::now_v7(),
            cwd.display()
        ))
        .unwrap();
        manifest.feature.memory.profile.enabled = true;
        manifest
            .feature
            .memory
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-test".to_string(),

                language: "English".to_string(),
            })
            .unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let scope = Scope::writable(&cwd).unwrap();
        let mut original = Worker::new(
            manifest.clone(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            authority.clone(),
            scope,
        )
        .await
        .unwrap();
        original.set_system_prompt_template(
            SystemPromptTemplate::parse(
                "default",
                crate::prompt::source::PromptCatalogSource::builtins_only(),
            )
            .unwrap(),
        );
        let (legacy_client, initial_legacy_loads) =
            legacy_resident_client("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH");
        let legacy_plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
            &manifest,
            legacy_client,
            original.prompts().load_full(),
        )
        .unwrap()
        .unwrap();
        let legacy_source = legacy_plan.resident_summary_source.clone();
        let report = original
            .install_features(FeatureRegistryBuilder::new().with_module(legacy_plan.module));
        assert!(!report.has_errors());
        original.install_system_prompt_contribution(legacy_source, None);
        original.materialize_durable_session_head().await.unwrap();
        assert_eq!(initial_legacy_loads.load(Ordering::SeqCst), 1);
        let session_id = original.session_id();
        let segment_id = original.segment_id();
        assert!(
            original
                .engine()
                .get_system_prompt()
                .unwrap()
                .contains("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH")
        );
        drop(original);

        // A legacy Memory resident source remains an initial-prompt contribution
        // only. The generic restore lifecycle does not infer subjektiv behavior
        // from the presence of a shared resident source.
        let mut legacy_only_restore =
            Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session_id,
                segment_id,
                manifest.clone(),
                store.clone(),
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                authority.clone(),
                Some(Box::new(NoopClient)),
            )
            .await
            .unwrap();
        let legacy_history = legacy_only_restore.history().to_vec();
        let (legacy_client, legacy_loads) =
            legacy_resident_client("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH");
        let legacy_plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
            &manifest,
            legacy_client,
            legacy_only_restore.prompts().load_full(),
        )
        .unwrap()
        .unwrap();
        let legacy_source = legacy_plan.resident_summary_source.clone();
        let report = legacy_only_restore
            .install_features(FeatureRegistryBuilder::new().with_module(legacy_plan.module));
        assert!(!report.has_errors());
        legacy_only_restore.install_system_prompt_contribution(legacy_source, None);
        legacy_only_restore
            .materialize_durable_session_head()
            .await
            .unwrap();
        assert_eq!(legacy_loads.load(Ordering::SeqCst), 0);
        assert_eq!(legacy_only_restore.history(), legacy_history);
        drop(legacy_only_restore);

        let mut subject_manifest = manifest.clone();
        subject_manifest.feature.memory = Default::default();
        subject_manifest.feature.subjektiv.profile.enabled = true;
        subject_manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-test".to_string(),

                language: "English".to_string(),
            })
            .unwrap();

        let mut restored =
            Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session_id,
                segment_id,
                subject_manifest.clone(),
                store.clone(),
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                authority.clone(),
                Some(Box::new(NoopClient)),
            )
            .await
            .unwrap();
        let history_before = restored.history().to_vec();
        let (current_source, current_loads) = subject_resident_source_for_restore(
            &subject_manifest,
            memory::backend::MemoryResidentSummaryAvailability::Ready,
            Some("latest subject surface"),
        );
        let report = restored.install_features(FeatureRegistryBuilder::new().with_module(
            crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::new(
                current_source,
                restored.prompts(),
                "workspace-test",
            ),
        ));
        assert!(!report.has_errors());
        restored.materialize_durable_session_head().await.unwrap();

        assert_eq!(current_loads.load(Ordering::SeqCst), 1);
        assert_eq!(&restored.history()[..history_before.len()], history_before);
        let refresh = restored
            .history()
            .last()
            .unwrap()
            .as_text()
            .unwrap()
            .to_string();
        assert!(refresh.contains("latest subject surface"));
        assert!(refresh.contains("supersedes every resident memory summary"));
        assert!(!refresh.contains("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH"));
        let refreshed_len = restored.history().len();
        restored.materialize_durable_session_head().await.unwrap();
        assert_eq!(restored.history().len(), refreshed_len);
        drop(restored);

        let mut empty_restore =
            Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session_id,
                segment_id,
                subject_manifest.clone(),
                store.clone(),
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                authority.clone(),
                Some(Box::new(NoopClient)),
            )
            .await
            .unwrap();
        let (empty_source, empty_loads) = subject_resident_source_for_restore(
            &subject_manifest,
            memory::backend::MemoryResidentSummaryAvailability::Ready,
            None,
        );
        let report = empty_restore.install_features(FeatureRegistryBuilder::new().with_module(
            crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::new(
                empty_source,
                empty_restore.prompts(),
                "workspace-test",
            ),
        ));
        assert!(!report.has_errors());
        empty_restore
            .materialize_durable_session_head()
            .await
            .unwrap();
        assert_eq!(empty_loads.load(Ordering::SeqCst), 1);
        let empty_boundary = empty_restore
            .history()
            .last()
            .unwrap()
            .as_text()
            .unwrap()
            .to_string();
        assert!(
            empty_boundary
                .contains("current generated Memory surface is ready and intentionally empty")
        );
        assert!(!empty_boundary.contains("No generated Memory surface exists yet"));
        drop(empty_restore);

        for availability in [
            memory::backend::MemoryResidentSummaryAvailability::Ungenerated,
            memory::backend::MemoryResidentSummaryAvailability::Stale,
            memory::backend::MemoryResidentSummaryAvailability::Failed,
        ] {
            let mut unavailable_restore = Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session_id,
                segment_id,
                subject_manifest.clone(),
                store.clone(),
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                authority.clone(),
                Some(Box::new(NoopClient)),
            )
            .await
            .unwrap();
            let history_before = unavailable_restore.history().to_vec();
            let (source, loads) = subject_resident_source_for_restore(
                &subject_manifest,
                availability,
                Some("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH"),
            );
            let report =
                unavailable_restore.install_features(FeatureRegistryBuilder::new().with_module(
                    crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::new(
                        source,
                        unavailable_restore.prompts(),
                        "workspace-test",
                    ),
                ));
            assert!(!report.has_errors());
            unavailable_restore
                .materialize_durable_session_head()
                .await
                .unwrap();

            assert_eq!(loads.load(Ordering::SeqCst), 1);
            assert_eq!(
                &unavailable_restore.history()[..history_before.len()],
                history_before
            );
            let tombstone = unavailable_restore
                .history()
                .last()
                .unwrap()
                .as_text()
                .unwrap()
                .to_string();
            let unavailable_message = match availability {
                memory::backend::MemoryResidentSummaryAvailability::Ungenerated => {
                    "No generated Memory surface exists yet"
                }
                memory::backend::MemoryResidentSummaryAvailability::Stale => {
                    "generated Memory surface is stale"
                }
                memory::backend::MemoryResidentSummaryAvailability::Failed => {
                    "Generation of the Memory surface failed"
                }
                memory::backend::MemoryResidentSummaryAvailability::Unavailable => {
                    "Host could not fetch the Subject resident context"
                }
                memory::backend::MemoryResidentSummaryAvailability::Ready => unreachable!(),
            };
            assert!(tombstone.contains(unavailable_message));
            assert!(!tombstone.contains("latest subject surface"));
            assert!(!tombstone.contains("LEGACY_MEMORY_SENTINEL_MUST_NOT_REFRESH"));
            let entries = store.read_all(session_id, segment_id).unwrap();
            assert!(matches!(
                entries.last(),
                Some(LogEntry::AnnotatedSystemItem { entry, .. }) if input_schema(entry) == Some("memory:resident-summary")
            ));
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
        let mut memory = manifest::ResolvedMemoryFeatureConfig::default();
        memory.profile.enabled = true;
        memory
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-test".to_string(),

                language: "Japanese".to_string(),
            })
            .unwrap();
        manifest.feature.memory = memory;
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
        let prompt_override = worker
            .prompts()
            .load_full()
            .memory_consolidation_system("Japanese")
            .unwrap();
        worker.install_system_prompt_contribution(None, Some(prompt_override));
        worker.ensure_system_prompt_materialized().await.unwrap();
        let prompt = worker.engine().get_system_prompt().unwrap();
        assert!(prompt.contains("`language`: `Japanese`"));
        assert!(!prompt.contains("`language`: `English`"));
    }

    async fn render_system_prompt_with_summary(
        summary_doc: Option<&str>,
        memory_config: Option<manifest::ResolvedMemoryFeatureConfig>,
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
        memory_config: Option<manifest::ResolvedMemoryFeatureConfig>,
        gates: ResidentInjectionGates,
        _unused: bool,
    ) -> String {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut manifest = minimal_manifest();
        manifest.feature.memory = memory_config.clone().unwrap_or_default();
        if memory_config.is_some() {
            manifest.feature.memory.profile.enabled = true;
        }
        let scope = Scope::writable(&cwd).unwrap();
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let workspace_context = WorkerWorkspaceContext::local_filesystem(None);
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
        let resident_summary_source = if memory_config
            .as_ref()
            .is_some_and(|cfg| cfg.profile.resident.inject_summary)
            && gates.summary
        {
            let summary = summary_doc.and_then(summary_content_for_backend);
            Some(test_system_prompt_contribution_source(summary).0)
        } else {
            None
        };
        worker.install_system_prompt_contribution(resident_summary_source, None);
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

    #[tokio::test]
    async fn resident_summary_body_is_injected_without_frontmatter() {
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("summary body for resident prompt\n")),
            Some(manifest::ResolvedMemoryFeatureConfig::default()),
            true,
        )
        .await;

        assert!(rendered.contains("summary body for resident prompt"));
        assert!(!rendered.contains("updated_at: 2026-01-01T00:00:00Z"));
        assert!(!rendered.contains("---\nupdated_at"));
    }

    #[tokio::test]
    async fn resident_summary_injection_can_be_disabled_by_memory_feature() {
        let mut memory = manifest::ResolvedMemoryFeatureConfig::default();
        memory.profile.resident.inject_summary = false;
        let rendered = render_system_prompt_with_summary(
            Some(&summary_doc("disabled summary body\n")),
            Some(memory),
            true,
        )
        .await;

        assert!(!rendered.contains("disabled summary body"));
    }

    #[tokio::test]
    async fn resident_summary_is_absent_when_memory_feature_is_disabled() {
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
            Some(manifest::ResolvedMemoryFeatureConfig::default()),
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
            Some(manifest::ResolvedMemoryFeatureConfig::default()),
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
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut worker = runtime
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

        let activation = runtime
            .block_on(worker.activate_skill("triage-errors"))
            .unwrap();

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
                LogEntry::AnnotatedSystemItem { entry, .. }
                    if entry.metadata.input_metadata.as_ref().and_then(|input| input.feature_payload()).and_then(|payload| payload.decode::<crate::skill::SkillActivationInput>("skill:activation", 1).ok()).is_some_and(|payload| payload.name == "triage-errors")
                    && entry.item.history_text().contains("# Triage Errors")
                    && entry.item.history_text() == history_text
            )
        }));
    }

    #[tokio::test]
    async fn feature_session_extension_is_fenced_by_exact_committed_location() {
        let dir = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(dir.path().join("sessions")).unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store,
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        worker.ensure_segment_head().await.unwrap();

        let capture_handle = worker.committed_session_capture_handle();
        let extension_handle = worker.session_extension_handle();
        let stale = capture_handle.capture().unwrap();
        worker
            .commit_entry(LogEntry::TurnEnd {
                ts: segment_log::now_millis(),
                turn_count: 1,
            })
            .unwrap();
        assert!(
            !extension_handle
                .append_if_current(
                    &stale.location(),
                    "test.feature",
                    serde_json::json!({"result": "stale"}),
                )
                .unwrap()
        );

        let current = capture_handle.capture().unwrap();
        assert!(
            extension_handle
                .append_if_current(
                    &current.location(),
                    "test.feature",
                    serde_json::json!({"result": "current"}),
                )
                .unwrap()
        );
        let entries = worker
            .store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, LogEntry::Extension { domain, .. } if domain == "test.feature"))
                .count(),
            1
        );

        worker
            .commit_entry(LogEntry::RunErrored {
                ts: segment_log::now_millis(),
                entry_id: None,
                interrupted: true,
                message: "provider failed".to_string(),
                failure: Some(RunFailureKind::Engine),
            })
            .unwrap();
        assert_eq!(
            capture_handle.capture().unwrap().run_exit,
            CommittedRunExit::Interrupted
        );
        worker
            .commit_entry(LogEntry::RunCompleted {
                entry_id: None,
                ts: segment_log::now_millis(),
                interrupted: false,
                result: EngineResult::Finished,
                active_run_turn_count: None,
            })
            .unwrap();
        assert_eq!(
            capture_handle.capture().unwrap().run_exit,
            CommittedRunExit::Finished
        );
        worker
            .commit_entry(LogEntry::Invoke {
                ts: segment_log::now_millis(),
                trigger: protocol::InvokeKind::UserSend,
            })
            .unwrap();
        worker
            .commit_entry(LogEntry::RunCancelled {
                ts: segment_log::now_millis(),
                entry_id: None,
            })
            .unwrap();
        assert_eq!(
            capture_handle.capture().unwrap().run_exit,
            CommittedRunExit::Interrupted,
            "a current cancellation must not inherit an earlier finished run"
        );
        worker
            .commit_entry(LogEntry::RunResumed {
                ts: segment_log::now_millis(),
                entry_id: None,
                source: RunResumeSource::Compaction,
                active_run_turn_count: 1,
            })
            .unwrap();
        assert_eq!(
            capture_handle.capture().unwrap().run_exit,
            CommittedRunExit::NonFinal
        );
    }

    #[test]
    fn submission_retry_identity_is_scoped_to_authenticated_source_and_keeps_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let input = vec![Segment::text("same request")];
        let account_a = WorkerHistoryProvenance::HumanInput {
            account_id: "account-a".into(),
        };
        let account_b = WorkerHistoryProvenance::HumanInput {
            account_id: "account-b".into(),
        };
        let first = handle
            .accept_from_source(
                "request-1".into(),
                input.clone(),
                "account:account-a".into(),
                account_a.clone(),
                false,
            )
            .unwrap();
        let replay = handle
            .accept_from_source(
                "request-1".into(),
                input.clone(),
                "account:account-a".into(),
                account_a.clone(),
                false,
            )
            .unwrap();
        let other_source = handle
            .accept_from_source(
                "request-1".into(),
                input,
                "account:account-b".into(),
                account_b.clone(),
                false,
            )
            .unwrap();

        assert_eq!(replay.submission_id, first.submission_id);
        assert_ne!(other_source.submission_id, first.submission_id);
        let state = handle.state.lock().unwrap();
        assert_eq!(state.pending[0].provenance, account_a);
        assert_eq!(state.pending[1].provenance, account_b);
    }

    #[test]
    fn restore_reconciliation_clears_interrupted_acceptance_pin_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let session_id = handle.writer.state.session_id();
        let segment_id = handle.writer.state.location().segment_id;
        let limits = session_store::UploadedFileLimits {
            max_file_bytes: 1024,
            max_session_bytes: 2048,
        };
        let file = handle
            .writer
            .store
            .write_uploaded_file(session_id, "retry.txt", "text/plain", b"retry", limits)
            .unwrap();
        handle
            .writer
            .store
            .pin_uploaded_file(session_id, &file, "interrupted-before-checkpoint")
            .unwrap();
        drop(handle);
        let handle = PendingSubmissionHandle {
            state: Arc::new(Mutex::new(PendingActivationState::default())),
            writer: LogWriterHandle {
                store: session_store::FsStore::new(temp.path()).unwrap(),
                state: SegmentState::new(session_id, segment_id, 0),
                sink: SegmentLogSink::new(),
                in_flight: None,
            },
        };

        assert_eq!(handle.reconcile_uploaded_file_pins().unwrap(), 1);
        let accepted = handle
            .accept(
                "request-after-restore".into(),
                vec![Segment::UploadedFile { file: file.clone() }],
                false,
            )
            .unwrap();
        assert!(!accepted.submission_id.is_empty());
        assert_eq!(handle.reconcile_uploaded_file_pins().unwrap(), 0);
        assert!(matches!(
            handle
                .writer
                .store
                .delete_uploaded_file(session_id, &file.artifact_id),
            Err(StoreError::ArtifactAlreadyCommitted)
        ));
    }

    #[test]
    fn rejected_submission_rolls_back_uploaded_file_pins_acquired_before_conflict() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let session_id = handle.writer.state.session_id();
        let limits = session_store::UploadedFileLimits {
            max_file_bytes: 1024,
            max_session_bytes: 2048,
        };
        let first = handle
            .writer
            .store
            .write_uploaded_file(session_id, "first.txt", "text/plain", b"first", limits)
            .unwrap();
        let second = handle
            .writer
            .store
            .write_uploaded_file(session_id, "second.txt", "text/plain", b"second", limits)
            .unwrap();
        handle
            .writer
            .store
            .pin_uploaded_file(session_id, &second, "other-submission")
            .unwrap();

        assert!(
            handle
                .accept(
                    "request-partial-pin".into(),
                    vec![
                        Segment::UploadedFile {
                            file: first.clone(),
                        },
                        Segment::UploadedFile {
                            file: second.clone(),
                        },
                    ],
                    false,
                )
                .is_err()
        );
        assert!(handle.snapshot().submissions.is_empty());
        assert!(
            handle
                .writer
                .store
                .delete_uploaded_file(session_id, &first.artifact_id)
                .unwrap()
        );
        assert!(matches!(
            handle
                .writer
                .store
                .delete_uploaded_file(session_id, &second.artifact_id),
            Err(StoreError::ArtifactAlreadyCommitted)
        ));
        handle
            .writer
            .store
            .release_uploaded_file_pin(session_id, &second.artifact_id, "other-submission")
            .unwrap();
        assert!(
            handle
                .writer
                .store
                .delete_uploaded_file(session_id, &second.artifact_id)
                .unwrap()
        );
    }

    #[test]
    fn queued_submission_pins_uploaded_file_until_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let session_id = handle.writer.state.session_id();
        let reference = handle
            .writer
            .store
            .write_uploaded_file(
                session_id,
                "queued.txt",
                "text/plain",
                b"queued artifact",
                session_store::UploadedFileLimits {
                    max_file_bytes: 1024,
                    max_session_bytes: 2048,
                },
            )
            .unwrap();
        let accepted = handle
            .accept(
                "artifact-request".into(),
                vec![Segment::UploadedFile {
                    file: reference.clone(),
                }],
                false,
            )
            .unwrap();

        assert_eq!(
            handle
                .writer
                .store
                .delete_uncommitted_uploaded_files(session_id)
                .unwrap(),
            0
        );
        assert!(
            handle
                .writer
                .store
                .read_uploaded_file_by_id(session_id, &reference.artifact_id)
                .is_ok()
        );

        handle.cancel(&accepted.submission_id).unwrap();
        assert_eq!(
            handle
                .writer
                .store
                .delete_uncommitted_uploaded_files(session_id)
                .unwrap(),
            1
        );
    }

    #[test]
    fn submit_and_notification_wait_for_activation_barrier_then_append_to_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let old_location = handle.writer.state.location();
        let replacement = session_store::new_segment_id();
        handle
            .writer
            .store
            .create_segment(old_location.session_id, replacement, &[])
            .unwrap();
        let old_entries = handle
            .writer
            .store
            .read_all(old_location.session_id, old_location.segment_id)
            .unwrap()
            .len();
        let append_guard = handle.writer.state.append_lock.lock().unwrap();
        let submit_handle = handle.clone();
        let notify_handle = handle.clone();
        let (started, starts) = std::sync::mpsc::channel();
        let (finished, finishes) = std::sync::mpsc::channel();
        let submit_started = started.clone();
        let submit_finished = finished.clone();
        let submit = std::thread::spawn(move || {
            submit_started.send(()).unwrap();
            submit_handle
                .accept(
                    "barrier-submit".into(),
                    vec![Segment::text("queued")],
                    false,
                )
                .unwrap();
            submit_finished.send(()).unwrap();
        });
        let notify = std::thread::spawn(move || {
            started.send(()).unwrap();
            notify_handle
                .accept_notification("barrier-notify".into(), "completed".into())
                .unwrap();
            finished.send(()).unwrap();
        });
        starts.recv_timeout(Duration::from_secs(1)).unwrap();
        starts.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(finishes.recv_timeout(Duration::from_millis(50)).is_err());

        handle.writer.state.set_location(SegmentLocation {
            session_id: old_location.session_id,
            segment_id: replacement,
        });
        drop(append_guard);
        finishes.recv_timeout(Duration::from_secs(1)).unwrap();
        finishes.recv_timeout(Duration::from_secs(1)).unwrap();
        submit.join().unwrap();
        notify.join().unwrap();

        assert_eq!(
            handle
                .writer
                .store
                .read_all(old_location.session_id, old_location.segment_id)
                .unwrap()
                .len(),
            old_entries
        );
        assert_eq!(
            handle
                .writer
                .store
                .read_all(old_location.session_id, replacement)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn pending_notification_previews_are_bounded_ordered_and_survive_queue_cancel() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let submit = handle
            .accept("submit".into(), vec![Segment::text("queued")], false)
            .unwrap();
        let long = "界".repeat(1000);
        handle
            .accept_notification("n1".into(), "通知\n\t<b>内容</b>".into())
            .unwrap();
        handle
            .accept_notification("n2".into(), long.clone())
            .unwrap();
        let before = handle.snapshot();
        assert_eq!(before.notification_count, 2);
        assert_eq!(
            before.notification_previews,
            vec![
                "通知 <b>内容</b>".to_owned(),
                format!("{}…", "界".repeat(240))
            ]
        );
        let after = handle.cancel(&submit.submission_id).unwrap();
        assert!(after.submissions.is_empty());
        assert_eq!(after.notification_previews, before.notification_previews);
        let entries = handle.persisted_entries_for_test();
        let restored: PendingActivationState = entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => {
                    Some(serde_json::from_value(payload.clone()).unwrap())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(
            restored.snapshot().notification_previews,
            before.notification_previews
        );
        let batch = handle.prepare_notification_batch();
        assert_eq!(batch[0].0.message, "通知\n\t<b>内容</b>");
        assert_eq!(batch[1].0.message, long);
        assert_eq!(handle.snapshot().notification_count, 0);
        assert!(handle.snapshot().notification_previews.is_empty());
    }

    #[test]
    fn pending_submission_preview_is_single_line_bounded_and_display_only() {
        let input = vec![
            Segment::text("  日本語\n\tの確認  "),
            Segment::Paste {
                id: 1,
                chars: 4,
                lines: 2,
                content: "<b>\r\ntext</b>".into(),
            },
            Segment::FileRef {
                path: "src/main.rs".into(),
            },
        ];
        assert_eq!(
            pending_submission_preview(&input),
            "日本語 の確認 <b> text</b> @src/main.rs"
        );
        assert_eq!(
            pending_submission_preview(&[Segment::text("\n\t")]),
            "[Empty input]"
        );
        assert_eq!(
            pending_submission_preview(&[Segment::Unknown]),
            "[Unknown input]"
        );
        assert_eq!(
            pending_submission_preview(&[Segment::text("界".repeat(240))]),
            "界".repeat(240)
        );
        let long = pending_submission_preview(&[Segment::text("界".repeat(10000))]);
        assert_eq!(long, format!("{}…", "界".repeat(240)));
        assert_eq!(long.chars().count(), 241);

        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        handle
            .accept("preview".into(), input.clone(), false)
            .unwrap();
        let snapshot = handle.snapshot();
        assert_eq!(
            snapshot.submissions[0].preview.as_deref(),
            Some("日本語 の確認 <b> text</b> @src/main.rs")
        );
        let activation = handle.prepare_next_activation(None).unwrap().unwrap();
        let PendingActivation::Submission(submission) = activation;
        assert_eq!(submission.input, input);
    }

    #[test]
    fn pending_submission_preview_labels_attachments_without_resolving_them() {
        let file: protocol::UploadedFileRef = serde_json::from_value(serde_json::json!({
            "artifact_id": "hidden-id", "file_name": "図\n面.png", "media_type": "image/png",
            "created_at_ms": 1, "availability": "available", "byte_len": 20,
            "sha256": "hidden-digest"
        }))
        .unwrap();
        assert_eq!(
            pending_submission_preview(&[Segment::UploadedFile { file }]),
            "[Attached file: 図 面.png]"
        );
        let artifact: protocol::PasteArtifactRef = serde_json::from_value(serde_json::json!({
            "artifact_id": "hidden-id", "created_at_ms": 1, "media_type": "text_plain_utf8",
            "availability": "available", "byte_len": 500, "char_count": 500, "line_count": 10,
            "sha256": "hidden-digest", "source_entry_id": "hidden-entry"
        }))
        .unwrap();
        assert_eq!(
            pending_submission_preview(&[Segment::PasteArtifact { artifact }]),
            "[Large paste]"
        );
    }

    #[test]
    fn pending_submission_queue_is_durable_idempotent_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let input = vec![Segment::text("queued")];
        let accepted = handle
            .accept("request-1".into(), input.clone(), false)
            .unwrap();
        assert_eq!(
            accepted.disposition,
            protocol::SubmissionDisposition::Queued
        );
        assert_eq!(handle.snapshot().submissions.len(), 1);

        let replay = handle
            .accept("request-1".into(), input.clone(), false)
            .unwrap();
        assert_eq!(replay.submission_id, accepted.submission_id);
        assert!(replay.activation.is_none());
        assert_eq!(handle.snapshot().submissions.len(), 1);
        assert!(matches!(
            handle.accept("request-1".into(), vec![Segment::text("different")], false),
            Err(PendingSubmissionError::IdempotencyConflict)
        ));

        let entries = handle
            .writer
            .store
            .read_all(
                handle.writer.state.session_id(),
                handle.writer.state.segment_id(),
            )
            .unwrap();
        let payload = entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => {
                    Some(payload.clone())
                }
                _ => None,
            })
            .unwrap();
        let restored: PendingActivationState = serde_json::from_value(payload).unwrap();
        assert_eq!(restored.pending.len(), 1);
        assert_eq!(restored.pending[0].submission_id, accepted.submission_id);

        let fence = handle.snapshot();
        assert!(matches!(
            handle.clear(&["stale-submission".to_owned()]),
            Err(PendingSubmissionError::ContentConflict { .. })
        ));
        assert!(matches!(
            handle.prepare_next_activation(Some("wrong-head")),
            Err(PendingSubmissionError::HeadConflict { .. })
        ));
        assert_eq!(handle.snapshot(), fence);

        let snapshot = handle.cancel(&accepted.submission_id).unwrap();
        assert!(snapshot.submissions.is_empty());
        assert!(matches!(
            handle.cancel(&accepted.submission_id),
            Err(PendingSubmissionError::NotFound(_))
        ));

        for index in 0..MAX_PENDING_SUBMISSIONS {
            handle
                .accept(
                    format!("limit-{index}"),
                    vec![Segment::text(format!("value-{index}"))],
                    false,
                )
                .unwrap();
        }
        assert!(matches!(
            handle.accept("over-limit".into(), vec![Segment::text("too much")], false),
            Err(PendingSubmissionError::CountLimit)
        ));
        assert_eq!(handle.snapshot().submissions.len(), MAX_PENDING_SUBMISSIONS);
        let cleared = handle
            .clear(
                &handle
                    .snapshot()
                    .submissions
                    .iter()
                    .map(|submission| submission.submission_id.clone())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        assert!(cleared.submissions.is_empty());
        assert_eq!(cleared.notification_count, 0);
    }

    #[test]
    fn pending_queue_fences_use_target_state_and_exact_fifo_content() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let first = handle
            .accept("first".into(), vec![Segment::text("one")], false)
            .unwrap();
        let observed = vec![first.submission_id.clone()];
        let second = handle
            .accept("second".into(), vec![Segment::text("two")], false)
            .unwrap();
        handle
            .accept_notification("notice".into(), "keep this".into())
            .unwrap();
        assert!(matches!(
            handle.clear(&observed),
            Err(PendingSubmissionError::ContentConflict { .. })
        ));
        assert!(matches!(
            handle.clear(&[second.submission_id.clone(), first.submission_id.clone()]),
            Err(PendingSubmissionError::ContentConflict { .. })
        ));
        assert_eq!(handle.snapshot().submissions.len(), 2);
        handle.cancel(&second.submission_id).unwrap();
        let activation = handle
            .prepare_next_activation(Some(&first.submission_id))
            .unwrap()
            .unwrap();
        assert!(matches!(
            handle.cancel(&first.submission_id),
            Err(PendingSubmissionError::NotFound(_))
        ));
        let PendingActivation::Submission(submission) = activation;
        handle.abort_activation(submission);
        let cleared = handle.clear(&observed).unwrap();
        assert!(cleared.submissions.is_empty());
        assert_eq!(cleared.notification_count, 1);
        assert_eq!(cleared.notification_previews, ["keep this"]);
        let entries = handle.persisted_entries_for_test();
        let payload = entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => Some(payload),
                _ => None,
            })
            .unwrap();
        let restored: PendingActivationState = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(restored.snapshot(), cleared);
    }

    #[test]
    fn durable_notification_commits_authenticated_history_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let provenance = WorkerHistoryProvenance::HumanInput {
            account_id: "account-1".into(),
        };
        let committed = handle
            .writer
            .commit_system_item_with_extensions(
                SystemItem::Notification {
                    message: "notice".into(),
                    body: "notice".into(),
                    prompt_provenance: None,
                },
                Vec::new(),
                Some(provenance.clone()),
            )
            .unwrap();
        assert_eq!(committed.annotation.origin, provenance);
    }

    #[test]
    fn notification_retry_identity_is_scoped_to_authenticated_source() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let account_a = WorkerHistoryProvenance::HumanInput {
            account_id: "account-a".into(),
        };
        let account_b = WorkerHistoryProvenance::HumanInput {
            account_id: "account-b".into(),
        };
        assert!(
            handle
                .accept_notification_from_source(
                    "request-1".into(),
                    "notice".into(),
                    "account:account-a".into(),
                    account_a.clone(),
                )
                .unwrap()
        );
        assert!(
            !handle
                .accept_notification_from_source(
                    "request-1".into(),
                    "notice".into(),
                    "account:account-a".into(),
                    account_a.clone(),
                )
                .unwrap()
        );
        assert!(
            handle
                .accept_notification_from_source(
                    "request-1".into(),
                    "notice".into(),
                    "account:account-b".into(),
                    account_b.clone(),
                )
                .unwrap()
        );
        let state = handle.state.lock().unwrap();
        assert_eq!(state.pending_notifications[0].provenance, account_a);
        assert_eq!(state.pending_notifications[1].provenance, account_b);
    }

    #[test]
    fn notification_lost_acceptance_retry_after_restore_commits_once_not_by_body() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        let source = "backend:orchestrator-attention";
        let accept = |handle: &PendingSubmissionHandle<session_store::FsStore>, id: &str| {
            handle
                .accept_notification_from_source(
                    id.into(),
                    "Queued Tickets require attention".into(),
                    source.into(),
                    WorkerHistoryProvenance::BackendInstruction { operation_id: None },
                )
                .unwrap()
        };
        // The sender lost this acceptance response; the receipt is already durable.
        assert!(accept(&handle, "attention-1"));
        let restored_state: PendingActivationState = handle
            .persisted_entries_for_test()
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LogEntry::Extension {
                    domain, payload, ..
                } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => {
                    Some(serde_json::from_value(payload.clone()).unwrap())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(restored_state.notification_receipts.len(), 1);
        let handle = PendingSubmissionHandle {
            state: Arc::new(Mutex::new(restored_state)),
            writer: handle.writer.clone(),
        };
        assert!(!accept(&handle, "attention-1"));
        let commit_batch = |handle: &PendingSubmissionHandle<session_store::FsStore>| {
            let batch = handle.prepare_notification_batch();
            let count = batch.len();
            for (notification, identity) in batch {
                handle
                    .commit_notification(
                        &identity,
                        SystemItem::Notification {
                            body: notification.message.clone(),
                            message: notification.message,
                            prompt_provenance: None,
                        },
                        Some(notification.provenance),
                    )
                    .unwrap();
            }
            count
        };
        assert_eq!(commit_batch(&handle), 1);
        assert!(!accept(&handle, "attention-1"));
        assert_eq!(commit_batch(&handle), 0);
        let notification_count = |handle: &PendingSubmissionHandle<session_store::FsStore>| {
            handle
                .persisted_entries_for_test()
                .iter()
                .filter(|entry| {
                    matches!(entry,
                        LogEntry::AnnotatedSystemItem { entry, .. }
                            if matches!(entry.item, SystemItem::Notification { .. })
                    )
                })
                .count()
        };
        assert_eq!(notification_count(&handle), 1);
        assert_eq!(handle.state.lock().unwrap().notification_receipts.len(), 1);
        // Same body with a new logical ID is legitimate, not content-deduped.
        assert!(accept(&handle, "attention-2"));
        assert_eq!(commit_batch(&handle), 1);
        assert_eq!(notification_count(&handle), 2);
        assert_eq!(handle.state.lock().unwrap().notification_receipts.len(), 2);
    }

    #[test]
    fn notification_batch_preserves_fifo_order_and_notification_dedupes() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        assert!(
            handle
                .accept_notification("notification-1".into(), "notice".into())
                .unwrap()
        );
        assert!(
            !handle
                .accept_notification("notification-1".into(), "notice".into())
                .unwrap()
        );
        assert!(matches!(
            handle.accept_notification("notification-1".into(), "different".into()),
            Err(PendingSubmissionError::IdempotencyConflict)
        ));
        assert!(
            handle
                .accept_notification("notification-2".into(), "second notice".into())
                .unwrap()
        );
        handle
            .accept("request-1".into(), vec![Segment::text("submit")], false)
            .unwrap();

        let notifications = handle.prepare_notification_batch();
        assert_eq!(notifications.len(), 2);
        assert_eq!(notifications[0].0.message, "notice");
        assert_eq!(notifications[1].0.message, "second notice");
        for (notification, identity) in notifications {
            handle
                .commit_notification(
                    &identity,
                    SystemItem::Notification {
                        body: notification.message.clone(),
                        message: notification.message,
                        prompt_provenance: None,
                    },
                    Some(notification.provenance),
                )
                .unwrap();
        }
        let committed_state = handle.state.lock().unwrap().clone();
        assert!(committed_state.pending_notifications.is_empty());
        assert!(committed_state.activating_notifications.is_empty());
        let submission = handle.prepare_next_activation(None).unwrap().unwrap();
        assert!(matches!(submission, PendingActivation::Submission(_)));
    }

    #[test]
    fn notification_commit_preserves_later_admission_and_completed_activation() {
        let temp = tempfile::tempdir().unwrap();
        let handle = PendingSubmissionHandle::for_test(temp.path());
        handle
            .accept_notification("first".into(), "first".into())
            .unwrap();
        let batch = handle.prepare_notification_batch();
        // These transitions occur after staging but before the model boundary.
        handle
            .accept("submit-1".into(), vec![Segment::text("input")], false)
            .unwrap();
        let Some(PendingActivation::Submission(submission)) =
            handle.prepare_next_activation(None).unwrap()
        else {
            panic!("expected submission")
        };
        handle.finish_activation(&submission.submission_id);
        handle
            .accept_notification("later".into(), "later".into())
            .unwrap();
        handle
            .accept("submit-2".into(), vec![Segment::text("queued")], false)
            .unwrap();
        let (notification, identity) = &batch[0];
        handle
            .commit_notification(
                identity,
                SystemItem::Notification {
                    body: notification.message.clone(),
                    message: notification.message.clone(),
                    prompt_provenance: None,
                },
                Some(notification.provenance.clone()),
            )
            .unwrap()
            .unwrap();
        let entries = handle.persisted_entries_for_test();
        let LogEntry::AnnotatedSystemItem { extensions, .. } = entries.last().unwrap() else {
            panic!("expected notification")
        };
        let checkpoint: PendingActivationState =
            serde_json::from_value(extensions[0].payload.clone()).unwrap();
        assert!(
            checkpoint.activating.is_none(),
            "must not revive completed submission"
        );
        assert!(checkpoint.activating_notifications.is_empty());
        assert_eq!(checkpoint.pending_notifications.len(), 1);
        assert_eq!(checkpoint.notification_receipts.len(), 2);
        assert_eq!(checkpoint.pending.len(), 1);
        assert_eq!(checkpoint.receipts.len(), 2);
        assert_eq!(checkpoint.snapshot(), handle.snapshot());
        let restored = PendingSubmissionHandle {
            state: Arc::new(Mutex::new(checkpoint)),
            writer: handle.writer.clone(),
        };
        assert!(
            !restored
                .accept_notification("later".into(), "later".into())
                .unwrap()
        );
        assert!(
            restored
                .accept("submit-2".into(), vec![Segment::text("queued")], false)
                .unwrap()
                .activation
                .is_none()
        );
        // Stage a newer batch before a delayed completion/retry of the old one.
        let later = restored.prepare_notification_batch();
        assert_eq!(later.len(), 1);
        assert!(
            restored
                .commit_notification(
                    identity,
                    SystemItem::Notification {
                        body: notification.message.clone(),
                        message: notification.message.clone(),
                        prompt_provenance: None,
                    },
                    Some(notification.provenance.clone())
                )
                .unwrap()
                .is_none(),
            "retry must not duplicate durable notification"
        );
        assert_eq!(
            restored
                .state
                .lock()
                .unwrap()
                .activating_notifications
                .len(),
            1,
            "an old batch completion must not clear a concurrently staged notification"
        );
        assert_eq!(restored.persisted_entries_for_test().len(), entries.len());
    }

    #[tokio::test]
    async fn notification_partial_drain_retries_only_uncommitted_entries() {
        use agen::interceptor::{Interceptor, PendingHistoryAppendsContext};
        let temp = tempfile::tempdir().unwrap();
        let original = PendingSubmissionHandle::for_test(temp.path());
        let store = FailRunTerminalStore {
            inner: original.writer.store.clone(),
            fail_run_terminal: Arc::new(AtomicBool::new(false)),
            invocation_failure: Arc::new(AtomicUsize::new(5)),
        };
        let handle = PendingSubmissionHandle {
            state: original.state.clone(),
            writer: LogWriterHandle {
                store: store.clone(),
                state: original.writer.state.clone(),
                sink: original.writer.sink.clone(),
                in_flight: None,
            },
        };
        let buffer = NotifyBuffer::new();
        for message in ["first", "second"] {
            handle
                .accept_notification(message.into(), message.into())
                .unwrap();
        }
        for (notification, identity) in handle.prepare_notification_batch() {
            buffer.push_durable_notify(notification.message, notification.provenance, identity);
        }
        let history = Arc::new(Mutex::new(VecDeque::new()));
        let interceptor = WorkerInterceptor::new_with_history_queue(
            Arc::new(HookRegistryBuilder::new().build()),
            None,
            None,
            buffer.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            Some(Arc::new(handle.writer.clone())),
            history.clone(),
        )
        .with_notification_committer(Some(Arc::new(handle.clone())));
        assert!(
            interceptor
                .pending_history_appends(PendingHistoryAppendsContext {
                    invocation: Default::default(),
                    history: &[]
                })
                .await
                .is_err()
        );
        assert_eq!(
            handle.state.lock().unwrap().activating_notifications.len(),
            1
        );
        assert_eq!(history.lock().unwrap().len(), 1);
        // New acceptance between failure and retry must survive both commits.
        handle
            .accept_notification("later".into(), "later".into())
            .unwrap();
        let appends = interceptor
            .pending_history_appends(PendingHistoryAppendsContext {
                invocation: Default::default(),
                history: &[],
            })
            .await
            .unwrap();
        assert_eq!(appends.len(), 1);
        assert_eq!(history.lock().unwrap().len(), 2);
        let entries = original.persisted_entries_for_test();
        assert_eq!(entries.iter().filter(|entry| matches!(entry, LogEntry::AnnotatedSystemItem { entry, .. } if matches!(entry.item, SystemItem::Notification { .. }))).count(), 2);
        let LogEntry::AnnotatedSystemItem { extensions, .. } = entries.last().unwrap() else {
            panic!("expected notification")
        };
        let checkpoint: PendingActivationState =
            serde_json::from_value(extensions[0].payload.clone()).unwrap();
        assert_eq!(checkpoint.notification_receipts.len(), 3);
        assert_eq!(checkpoint.pending_notifications.len(), 1);
        assert!(checkpoint.activating_notifications.is_empty());
        assert!(buffer.is_empty());
    }

    #[tokio::test]
    async fn notification_partial_drain_preserves_live_and_restored_history() {
        let temp = tempfile::tempdir().unwrap();
        let store = FailRunTerminalStore {
            inner: session_store::FsStore::new(temp.path()).unwrap(),
            fail_run_terminal: Arc::new(AtomicBool::new(false)),
            invocation_failure: Arc::new(AtomicUsize::new(5)),
        };
        let client = PauseResumeClient {
            calls: Arc::new(AtomicUsize::new(1)),
        };
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone()),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.attach_log_writer(Arc::new(worker.log_writer_handle()));
        let pending = worker.pending_submission_handle();
        for message in ["first", "second"] {
            pending
                .accept_notification(message.into(), message.into())
                .unwrap();
        }
        for (notification, identity) in pending.prepare_notification_batch() {
            worker.pending_notifies.push_durable_notify(
                notification.message,
                notification.provenance,
                identity,
            );
        }
        let result = worker
            .run_for_notification(protocol::InvokeKind::Notify)
            .await;
        assert!(
            matches!(result, Ok(WorkerRunResult::Interrupted { .. }) | Err(_)),
            "{result:?}"
        );
        assert_eq!(
            client.calls.load(Ordering::SeqCst),
            1,
            "failed drain must not call the provider"
        );
        let history = worker
            .session
            .history()
            .entries()
            .iter()
            .find(|entry| {
                entry
                    .item
                    .as_text()
                    .is_some_and(|text| text.contains("first"))
            })
            .unwrap()
            .clone();
        worker
            .run_for_notification(protocol::InvokeKind::Notify)
            .await
            .unwrap();
        assert_eq!(client.calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            worker
                .session
                .history()
                .entries()
                .iter()
                .filter(|entry| entry.annotation.entry_id == history.annotation.entry_id)
                .count(),
            1
        );
        let entries = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        let restored =
            restore_history_entries(worker.session_id(), worker.segment_id(), &entries).unwrap();
        assert_eq!(
            restored
                .iter()
                .filter(|entry| entry.annotation.entry_id == history.annotation.entry_id)
                .count(),
            1
        );
        assert_eq!(entries.iter().filter(|entry| matches!(entry, LogEntry::AnnotatedSystemItem { entry, .. } if matches!(entry.item, SystemItem::Notification { .. }))).count(), 2);
        assert!(
            pending
                .state
                .lock()
                .unwrap()
                .activating_notifications
                .is_empty()
        );
    }

    #[tokio::test]
    async fn invocation_fence_suspends_automatic_notification_until_fresh_input() {
        for scenario in ["failed", "unknown", "incomplete"] {
            let temp = tempfile::tempdir().unwrap();
            let store = FailRunTerminalStore {
                inner: session_store::FsStore::new(temp.path()).unwrap(),
                fail_run_terminal: Arc::new(AtomicBool::new(false)),
                invocation_failure: Arc::new(AtomicUsize::new(if scenario == "unknown" {
                    2
                } else {
                    0
                })),
            };
            let client = PauseResumeClient {
                calls: Arc::new(AtomicUsize::new(1)),
            };
            let mut worker = Worker::new(
                minimal_manifest(),
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone()),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            worker.attach_log_writer(Arc::new(worker.log_writer_handle()));
            let effects = Arc::new(AtomicUsize::new(0));
            install_recovery_invocation(&mut worker, effects.clone());
            let pending = worker.pending_submission_handle();
            pending
                .accept_notification("held-notify".into(), "held notification".into())
                .unwrap();
            for (notification, identity) in pending.prepare_notification_batch() {
                worker.pending_notifies.push_durable_notify(
                    notification.message,
                    notification.provenance,
                    identity,
                );
            }
            let mut input = recovery_invocation(scenario);
            if scenario == "failed" {
                let Segment::FeatureInvoke { invocation } = &mut input else {
                    unreachable!()
                };
                invocation.name = "invalid-name".into();
            }
            if scenario == "incomplete" {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), worker.run(vec![input]))
                        .await
                        .is_err()
                );
            } else {
                assert!(worker.run(vec![input]).await.is_err());
            }
            let before = worker.sink().subscribe_with_snapshot().0;
            for _ in 0..4 {
                assert!(!worker.can_schedule_notification_run(), "{scenario}");
                assert!(
                    worker
                        .run_for_notification(protocol::InvokeKind::Notify)
                        .await
                        .is_err()
                );
            }
            assert_eq!(
                worker.sink().subscribe_with_snapshot().0.len(),
                before.len()
            );
            assert_eq!(client.calls.load(Ordering::SeqCst), 1);
            assert!(!worker.pending_notifies.is_empty());
            assert_eq!(
                effects.load(Ordering::SeqCst),
                usize::from(scenario != "failed")
            );
            worker
                .run(vec![Segment::text("explicit fresh input")])
                .await
                .unwrap();
            assert!(worker.can_schedule_notification_run());
            assert!(worker.pending_notifies.is_empty());
            assert_eq!(client.calls.load(Ordering::SeqCst), 2);
            assert_eq!(
                effects.load(Ordering::SeqCst),
                usize::from(scenario != "failed"),
                "no unknown effect retry"
            );
        }
    }

    #[derive(Clone)]
    struct RecoveryInvocationFeature {
        calls: Arc<AtomicUsize>,
        sink: SegmentLogSink,
    }

    fn recovery_invocation_descriptor() -> protocol::FeatureInvocationDescriptor {
        protocol::FeatureInvocationDescriptor {
            identity: protocol::FeatureInvocationIdentity("builtin:recovery-test/prepare".into()),
            name: "prepare".into(),
            aliases: Vec::new(),
            display_name: "Prepare".into(),
            description: "Test recovery".into(),
            syntax: protocol::FeatureInvocationSyntax::Parenthesized,
            arguments: Vec::new(),
            client_adapter: None,
        }
    }

    fn recovery_invocation(id: &str) -> Segment {
        Segment::FeatureInvoke {
            invocation: protocol::FeatureInvocation {
                invocation_id: id.into(),
                identity: recovery_invocation_descriptor().identity,
                name: "prepare".into(),
                arguments: Vec::new(),
            },
        }
    }

    impl crate::feature::FeatureModule for RecoveryInvocationFeature {
        fn descriptor(&self) -> crate::feature::FeatureDescriptor {
            crate::feature::FeatureDescriptor::builtin("recovery-test", "Recovery test")
                .with_chat_invocation(recovery_invocation_descriptor())
        }
        fn install(
            &self,
            context: &mut crate::feature::FeatureInstallContext<'_>,
        ) -> Result<(), crate::feature::FeatureInstallError> {
            context
                .chat_invocations()
                .register(recovery_invocation_descriptor(), self.clone())
        }
    }

    #[async_trait]
    impl crate::feature::FeatureInvocationHandler for RecoveryInvocationFeature {
        async fn invoke(
            &self,
            context: crate::feature::FeatureInvocationContext,
            invocation: &protocol::FeatureInvocation,
        ) -> Result<protocol::FeatureInvocationResult, crate::feature::FeatureInvocationHandlerError>
        {
            let entries = self.sink.subscribe_with_snapshot().0;
            let start = entries
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    LogEntry::Extension {
                        domain, payload, ..
                    } if domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN => {
                        serde_json::from_value::<PendingActivationState>(payload.clone()).ok()
                    }
                    _ => None,
                })
                .expect("durable start must precede handler");
            assert!(
                start
                    .invocation_receipts
                    .iter()
                    .any(|receipt| { receipt.matches(invocation) && receipt.result.is_none() })
            );
            assert_eq!(context.invocation_id, invocation.invocation_id);
            self.calls.fetch_add(1, Ordering::SeqCst);
            if invocation.invocation_id == "incomplete" {
                futures::future::pending::<()>().await;
            }
            Ok(protocol::FeatureInvocationResult {
                invocation_id: invocation.invocation_id.clone(),
                identity: invocation.identity.clone(),
                status: protocol::FeatureInvocationStatus::Succeeded,
                message: "done".into(),
                context: Some("prepared recovery context".into()),
            })
        }
    }

    fn install_recovery_invocation<C: LlmClient + 'static, St: Store>(
        worker: &mut Worker<C, St>,
        calls: Arc<AtomicUsize>,
    ) {
        let feature = RecoveryInvocationFeature {
            calls,
            sink: worker.sink(),
        };
        assert!(
            !worker
                .install_features(FeatureRegistryBuilder::new().with_module(feature))
                .has_errors()
        );
    }

    #[derive(Clone)]
    struct ResidentInvocationClient {
        sink: Arc<Mutex<Option<SegmentLogSink>>>,
        requests: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmClient for ResidentInvocationClient {
        async fn stream(
            &self,
            request: agen::llm_client::Request,
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
            let entries = self
                .sink
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .subscribe_with_snapshot()
                .0;
            let preparation = entries
                .iter()
                .position(|entry| {
                    matches!(entry,
                        LogEntry::AnnotatedSystemItem { entry, .. }
                            if input_schema(entry) == Some("feature:invocation-result")
                    )
                })
                .expect("invocation result must be durable before the model request");
            let behavior = entries
                .iter()
                .position(|entry| {
                    matches!(entry,
                        LogEntry::AnnotatedSystemItem { entry, .. }
                            if input_schema(entry) == Some("subject:behavior")
                    )
                })
                .expect("Subject behavior refresh must be durable before the model request");
            assert!(preparation < behavior);
            let request = format!("{request:?}");
            assert!(request.contains("prepared recovery context"));
            assert!(request.contains("restore behavior"));
            assert!(request.contains("literal /prepare()"));
            self.requests.fetch_add(1, Ordering::SeqCst);
            Ok(Box::pin(futures::stream::iter([Ok(
                agen::llm_client::event::Event::Status(agen::llm_client::event::StatusEvent {
                    status: agen::llm_client::event::ResponseStatus::Completed,
                }),
            )])))
        }

        fn clone_boxed(&self) -> Box<dyn LlmClient> {
            Box::new(self.clone())
        }
    }

    #[tokio::test]
    async fn feature_invocation_and_subject_refresh_remain_append_only_across_restore_replay() {
        let runtime = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(runtime.path());
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("workspace");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut manifest = minimal_manifest();
        manifest.scope.allow[0].target = cwd.clone();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest
            .feature
            .subjektiv
            .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                workspace_id: "workspace-test".into(),

                language: "English".into(),
            })
            .unwrap();
        let client = ResidentInvocationClient {
            sink: Arc::default(),
            requests: Arc::default(),
        };
        let mut engine =
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(client.clone());
        engine.set_system_prompt("fixed initial system prompt");
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(dir.path().join("workers")).unwrap(),
        );
        let authority = WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone());
        let mut original = Worker::new(
            manifest.clone(),
            engine,
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            authority.clone(),
            Scope::writable(&cwd).unwrap(),
        )
        .await
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let behavior_md = Arc::new(Mutex::new("restore behavior".to_string()));
        let resident_source = || {
            crate::feature::builtin::memory::ordinary_subjektiv_resident_summary_source(
                &manifest,
                Some(crate::subjektiv::test_connection(Arc::new(
                    RestoreSubjectResidentClient {
                        availability: memory::backend::MemoryResidentSummaryAvailability::Ready,
                        content: Some("literal /prepare()".into()),
                        load_count: Arc::default(),
                        behavior_md: Some(behavior_md.clone()),
                    },
                ))),
                Arc::new(ArcSwap::from(PromptCatalog::builtins_only().unwrap())),
            )
            .unwrap()
            .unwrap()
        };
        original.attach_log_writer(Arc::new(original.log_writer_handle()));
        let source = resident_source();
        let report = original.install_features(
            FeatureRegistryBuilder::new()
                .with_module(RecoveryInvocationFeature {
                    calls: calls.clone(),
                    sink: original.sink(),
                })
                .with_module(
                    crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::new(
                        source.clone(),
                        original.prompts(),
                        "workspace-test",
                    ),
                ),
        );
        assert!(!report.has_errors(), "{report:?}");
        original.install_system_prompt_contribution(Some(source), None);
        original.set_system_prompt_template(
            SystemPromptTemplate::parse(
                "default",
                crate::prompt::source::PromptCatalogSource::builtins_only(),
            )
            .unwrap(),
        );
        original.materialize_durable_session_head().await.unwrap();
        *behavior_md.lock().unwrap() = "updated behavior".to_string();
        *client.sink.lock().unwrap() = Some(original.sink());
        let pending = original.pending_submission_handle();
        pending
            .accept_notification(
                "composed-notify".into(),
                "resident context notification".into(),
            )
            .unwrap();
        for (notification, extension) in pending.prepare_notification_batch() {
            original.pending_notifies.push_durable_notify(
                notification.message,
                notification.provenance,
                extension,
            );
        }
        let input = vec![recovery_invocation("composed")];
        pending
            .accept("composed-submit".into(), input.clone(), false)
            .unwrap();
        let Some(PendingActivation::Submission(submission)) =
            pending.prepare_next_activation(None).unwrap()
        else {
            panic!("expected submission")
        };
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let result = original
            .run_with_input_extensions_and_commit_hook(
                submission.input,
                vec![pending.activation_extension()],
                submission.provenance,
                crate::controller::input_committed_hook(
                    pending.clone(),
                    submission.submission_id,
                    sender,
                ),
            )
            .await
            .unwrap();
        // No Controller receiver was polled while the invocation/refresh ran.
        assert!(receiver.try_recv().is_ok());
        assert!(pending.state.lock().unwrap().activating.is_none());
        assert_eq!(
            client.requests.load(Ordering::SeqCst),
            1,
            "{result:?}; history={:?}",
            original.history()
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "resident text must not invoke a handler"
        );
        let history_before = original.history().to_vec();
        let session_id = original.session_id();
        let segment_id = original.segment_id();
        drop(original);

        let mut restored =
            Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session_id,
                segment_id,
                manifest.clone(),
                store,
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                authority,
                Some(Box::new(client.clone())),
            )
            .await
            .unwrap();
        restored.attach_log_writer(Arc::new(restored.log_writer_handle()));
        assert!(
            restored
                .pending_submission_handle()
                .prepare_next_activation(None)
                .unwrap()
                .is_none(),
            "notification/invocation checkpoints must not resurrect committed input after restore"
        );
        assert_eq!(
            restored
                .session
                .pending_activations
                .lock()
                .unwrap()
                .invocation_receipts
                .len(),
            1
        );
        assert_eq!(
            restored
                .session
                .pending_activations
                .lock()
                .unwrap()
                .notification_receipts
                .len(),
            1
        );
        let source = resident_source();
        let report = restored.install_features(
            FeatureRegistryBuilder::new()
                .with_module(RecoveryInvocationFeature {
                    calls: calls.clone(),
                    sink: restored.sink(),
                })
                .with_module(
                    crate::feature::builtin::memory::SubjektivResidentRestoreRefreshFeature::new(
                        source.clone(),
                        restored.prompts(),
                        "workspace-test",
                    ),
                ),
        );
        assert!(!report.has_errors(), "{report:?}");
        restored.install_system_prompt_contribution(Some(source), None);
        *client.sink.lock().unwrap() = Some(restored.sink());
        restored.run(input).await.unwrap();
        assert_eq!(client.requests.load(Ordering::SeqCst), 2);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "restore/replay must not repeat the business effect"
        );
        assert_eq!(&restored.history()[..history_before.len()], history_before);
        let entries = restored.sink().subscribe_with_snapshot().0;
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry,
                    LogEntry::AnnotatedSystemItem { entry, .. }
                        if input_schema(entry) == Some("feature:invocation-result")
                ))
                .count(),
            2 // Explicit replay displays the cached result; it does not repeat the effect.
        );
        assert!(entries.iter().any(|entry| matches!(entry,
            LogEntry::AnnotatedSystemItem { entry, .. }
                if input_schema(entry) == Some("memory:resident-summary")
        )));
    }

    #[tokio::test]
    async fn feature_invocation_restart_reuses_completed_results_and_fences_incomplete_starts() {
        let runtime = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(runtime.path());
        for id in ["completed", "incomplete"] {
            let dir = tempfile::tempdir().unwrap();
            let cwd = dir.path().join("workspace");
            std::fs::create_dir_all(&cwd).unwrap();
            let mut engine =
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient);
            engine.set_system_prompt("recovery test system prompt");
            let mut manifest = minimal_manifest();
            manifest.scope.allow[0].target = cwd.clone();
            let mut original = Worker::new(
                manifest,
                engine,
                session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
                WorkerWorkspaceContext::local_filesystem(None),
                WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
                Scope::writable(&cwd).unwrap(),
            )
            .await
            .unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            install_recovery_invocation(&mut original, calls.clone());
            let input = vec![recovery_invocation(id)];
            if id == "incomplete" {
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), original.run(input.clone()))
                        .await
                        .is_err()
                );
            } else {
                let _ = original.run(input.clone()).await;
                let entries = original.sink().subscribe_with_snapshot().0;
                assert!(entries.iter().any(|entry| match entry {
                    LogEntry::AnnotatedSystemItem {
                        entry, extensions, ..
                    } => {
                        input_schema(entry) == Some("feature:invocation-result")
                            && extensions.iter().any(|extension| {
                                extension.domain == SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN
                                    && serde_json::from_value::<PendingActivationState>(
                                        extension.payload.clone(),
                                    )
                                    .unwrap()
                                    .invocation_receipts
                                    .iter()
                                    .any(|receipt| receipt.result.is_some())
                            })
                    }
                    _ => false,
                }));
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let session = original.session_id();
            let segment = original.segment_id();
            let store = session_store::CombinedStore::new(
                original.store.clone(),
                session_store::FsWorkerStore::new(dir.path().join("workers")).unwrap(),
            );
            let manifest = original.manifest.clone();
            drop(original);
            let cwd = dir.path().join("workspace");
            let mut restored = Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session, segment, manifest, store,
                crate::prompt::source::PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::local_filesystem(None),
                WorkerFilesystemAuthority::local(cwd.clone(), cwd),
                Some(Box::new(NoopClient)),
            ).await.unwrap();
            install_recovery_invocation(&mut restored, calls.clone());
            if id == "incomplete" {
                assert!(restored.resume().await.is_err());
                assert!(
                    restored
                        .run_for_notification(protocol::InvokeKind::Notify)
                        .await
                        .is_err()
                );
                assert!(restored.run(input).await.is_err());
                let last = restored
                    .session
                    .pending_activations
                    .lock()
                    .unwrap()
                    .invocation_receipts[0]
                    .result
                    .clone()
                    .unwrap();
                assert_eq!(
                    last.status,
                    protocol::FeatureInvocationStatus::OutcomeUnknown
                );
            } else {
                let _ = restored.run(input).await;
                assert!(restored.ensure_invocation_preparation_complete().is_ok());
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "restore/replay must not repeat business execution"
            );
        }
    }

    #[tokio::test]
    async fn feature_invocation_rewind_atomic_replacement_preserves_recovery_on_disk_restart() {
        let runtime = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(runtime.path());
        for queued in [false, true] {
            for scenario in ["completed", "incomplete", "result-fault", "corrupt"] {
                for replacement_fault in [0, 1, 3, 4] {
                    let dir = tempfile::tempdir().unwrap();
                    let cwd = dir.path().join("workspace");
                    std::fs::create_dir_all(&cwd).unwrap();
                    let store = FailRunTerminalStore {
                        inner: session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
                        fail_run_terminal: Arc::new(AtomicBool::new(false)),
                        invocation_failure: Arc::new(AtomicUsize::new(0)),
                    };
                    let mut engine =
                        Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient);
                    engine.set_system_prompt("recovery rewind test system prompt");
                    let mut manifest = minimal_manifest();
                    manifest.scope.allow[0].target = cwd.clone();
                    let mut original = Worker::new(
                        manifest,
                        engine,
                        store.clone(),
                        WorkerWorkspaceContext::local_filesystem(None),
                        WorkerFilesystemAuthority::local(cwd.clone(), cwd.clone()),
                        Scope::writable(&cwd).unwrap(),
                    )
                    .await
                    .unwrap();
                    let calls = Arc::new(AtomicUsize::new(0));
                    install_recovery_invocation(&mut original, calls.clone());
                    let input = vec![recovery_invocation(scenario)];
                    if scenario == "corrupt" {
                        let _ = original.run(vec![Segment::text("rewind target")]).await;
                        let payload = serde_json::json!({"invocation_receipts": "corrupt"});
                        original
                            .commit_entry(LogEntry::Extension {
                                ts: segment_log::now_millis(),
                                domain: SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.into(),
                                payload: payload.clone(),
                            })
                            .unwrap();
                        original.session.restore_pending_activations(&[(
                            SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.into(),
                            payload,
                        )]);
                    } else {
                        if scenario == "result-fault" {
                            store.invocation_failure.store(2, Ordering::SeqCst);
                        }
                        let (run_input, extensions, provenance, activation) = if queued {
                            // Exercise the durable admission/activation path used
                            // by Controller, in addition to direct Worker.run.
                            let handle = original.pending_submission_handle();
                            let accepted = handle
                                .accept("request-1".into(), input.clone(), true)
                                .unwrap();
                            let pending = accepted.activation.unwrap();
                            let id = pending.submission_id.clone();
                            (
                                pending.input,
                                vec![handle.activation_extension()],
                                pending.provenance,
                                Some((handle, id)),
                            )
                        } else {
                            (
                                input.clone(),
                                Vec::new(),
                                WorkerHistoryProvenance::LegacyUnknown,
                                None,
                            )
                        };
                        let run = original.run_with_input_extensions_and_commit_hook(
                            run_input,
                            extensions,
                            provenance,
                            move || {
                                if let Some((handle, id)) = activation {
                                    handle.finish_activation(&id);
                                }
                            },
                        );
                        if scenario == "incomplete" {
                            assert!(
                                tokio::time::timeout(Duration::from_millis(20), run)
                                    .await
                                    .is_err()
                            );
                        } else {
                            let _ = run.await;
                        }
                        assert_eq!(calls.load(Ordering::SeqCst), 1);
                    }
                    let pending = original.session.pending_activations.lock().unwrap().clone();
                    if !queued {
                        assert!(pending.pending.is_empty());
                        assert!(pending.activating.is_none());
                        assert!(pending.receipts.is_empty());
                        assert!(pending.notification_receipts.is_empty());
                        assert!(pending.pending_notifications.is_empty());
                        assert!(pending.activating_notifications.is_empty());
                    }
                    let targets = rewind_test_inputs(&original);
                    let target = targets.last().unwrap();
                    let old = store
                        .read_all(original.session_id(), original.segment_id())
                        .unwrap();
                    let old_mirror = original.sink.subscribe_with_snapshot().0;
                    let old_count = original.segment_state.entries_written();
                    store
                        .invocation_failure
                        .store(replacement_fault, Ordering::SeqCst);
                    let applied = original.rewind_to(target.0.clone()).await;
                    let disk = store
                        .read_all(original.session_id(), original.segment_id())
                        .unwrap();
                    if replacement_fault == 3 {
                        assert!(applied.is_err());
                        assert_eq!(
                            serde_json::to_value(&disk).unwrap(),
                            serde_json::to_value(&old).unwrap()
                        );
                        assert_eq!(
                            serde_json::to_value(original.sink.subscribe_with_snapshot().0)
                                .unwrap(),
                            serde_json::to_value(old_mirror).unwrap()
                        );
                        assert_eq!(original.segment_state.entries_written(), old_count);
                    } else {
                        if replacement_fault <= 1 {
                            // An injected checkpoint-append fault is no longer
                            // relevant: rewind must never append after truncation.
                            assert_eq!(
                                store.invocation_failure.load(Ordering::SeqCst),
                                replacement_fault
                            );
                            let applied = applied.unwrap();
                            assert_eq!(applied.summary.truncated_to_entries, target.1);
                            assert_eq!(applied.summary.discarded_entries, old.len() - target.1);
                            assert_eq!(applied.entries.len(), target.1);
                            assert_eq!(original.segment_state.entries_written(), disk.len());
                            assert_eq!(
                                serde_json::to_value(original.sink.subscribe_with_snapshot().0)
                                    .unwrap(),
                                serde_json::to_value(&disk).unwrap()
                            );
                        } else {
                            assert!(applied.is_err());
                        }
                        assert_eq!(disk.len(), target.1 + 2);
                        assert_eq!(
                            serde_json::to_value(&disk[..target.1]).unwrap(),
                            serde_json::to_value(&old[..target.1]).unwrap()
                        );
                        let LogEntry::Extension {
                            domain, payload, ..
                        } = &disk[target.1]
                        else {
                            panic!("complete replacement must include recovery checkpoint")
                        };
                        assert_eq!(domain, SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN);
                        assert_eq!(*payload, serde_json::to_value(&pending).unwrap());
                        assert!(
                            matches!(disk.last().unwrap(), LogEntry::Extension { domain, payload, .. }
                            if domain == PROTECTED_INPUTS_DOMAIN && payload == &serde_json::json!({"version":1, "kinds":[]}))
                        );
                    }
                    let session = original.session_id();
                    let segment = original.segment_id();
                    let manifest = original.manifest.clone();
                    drop(original);
                    // Reopen from the path: no retained in-memory queue/Store state.
                    let reopened = session_store::CombinedStore::new(
                        session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
                        session_store::FsWorkerStore::new(dir.path().join("workers")).unwrap(),
                    );
                    let mut restored = Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                        session, segment, manifest, reopened,
                        crate::prompt::source::PromptCatalogSource::builtins_only(),
                        WorkerWorkspaceContext::local_filesystem(None),
                        WorkerFilesystemAuthority::local(cwd.clone(), cwd),
                        Some(Box::new(NoopClient)),
                    ).await.unwrap();
                    install_recovery_invocation(&mut restored, calls.clone());
                    let replay = restored.run(input).await;
                    if scenario == "completed" {
                        assert!(restored.ensure_invocation_preparation_complete().is_ok());
                    } else {
                        assert!(replay.is_err());
                        assert!(restored.resume().await.is_err());
                        assert!(
                            restored
                                .run_for_notification(protocol::InvokeKind::Notify)
                                .await
                                .is_err()
                        );
                    }
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        usize::from(scenario != "corrupt"),
                        "queued={queued} scenario={scenario} replacement_fault={replacement_fault}"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn feature_invocation_queue_receipts_prevent_activation_and_business_replay() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let calls = Arc::new(AtomicUsize::new(0));
        install_recovery_invocation(&mut worker, calls.clone());
        let input = vec![recovery_invocation("queued")];
        let handle = worker.pending_submission_handle();
        let accepted = handle
            .accept("request-1".into(), input.clone(), false)
            .unwrap();
        let repeated = handle
            .accept("request-1".into(), input.clone(), false)
            .unwrap();
        assert_eq!(accepted.submission_id, repeated.submission_id);
        assert_eq!(handle.snapshot().submissions.len(), 1);
        let PendingActivation::Submission(pending) =
            handle.prepare_next_activation(None).unwrap().unwrap();
        let id = pending.submission_id.clone();
        let extension = handle.activation_extension();
        let finished = handle.clone();
        let _ = worker
            .run_with_input_extensions_and_commit_hook(
                pending.input,
                vec![extension],
                pending.provenance,
                move || finished.finish_activation(&id),
            )
            .await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let replay = handle
            .accept("request-1".into(), input.clone(), false)
            .unwrap();
        assert!(replay.activation.is_none());
        assert!(handle.prepare_next_activation(None).unwrap().is_none());
        let _ = worker.run(input).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn feature_invocation_start_and_result_append_failures_fence_execution_and_replay() {
        for failure in [1, 2] {
            let dir = tempfile::tempdir().unwrap();
            let store = FailRunTerminalStore {
                inner: session_store::FsStore::new(dir.path().join("sessions")).unwrap(),
                fail_run_terminal: Arc::new(AtomicBool::new(false)),
                invocation_failure: Arc::new(AtomicUsize::new(failure)),
            };
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
            let calls = Arc::new(AtomicUsize::new(0));
            install_recovery_invocation(&mut worker, calls.clone());
            let input = vec![recovery_invocation("fault")];
            assert!(worker.run(input.clone()).await.is_err());
            assert_eq!(
                calls.load(Ordering::SeqCst),
                if failure == 1 { 0 } else { 1 }
            );
            assert!(worker.resume().await.is_err());
            if failure == 1 {
                // No start became durable and no effect ran; an explicit fresh
                // submission may execute after the append fault is corrected.
                let _ = worker.run(input).await;
            } else {
                // Effect ran but result did not become durable. Do not invoke
                // the handler a second time even when the Store is healthy.
                assert!(worker.run(input).await.is_err());
                let result = worker
                    .session
                    .pending_activations
                    .lock()
                    .unwrap()
                    .invocation_receipts[0]
                    .result
                    .clone()
                    .unwrap();
                assert_eq!(
                    result.status,
                    protocol::FeatureInvocationStatus::OutcomeUnknown
                );
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn feature_invocation_corrupt_checkpoint_fails_closed() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let calls = Arc::new(AtomicUsize::new(0));
        install_recovery_invocation(&mut worker, calls.clone());
        worker.session.restore_pending_activations(&[(
            SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.into(),
            serde_json::json!({"invocation_receipts": "corrupt"}),
        )]);
        assert!(worker.run(vec![recovery_invocation("new")]).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn feature_invocation_receipts_retain_digest_not_arguments_and_detect_payload_changes() {
        let Segment::FeatureInvoke { mut invocation } = recovery_invocation("large") else {
            unreachable!()
        };
        invocation.arguments = (0..32)
            .map(|index| protocol::InvocationArgumentValue {
                name: format!("arg{index}"),
                value: protocol::InvocationValue::String("private-argument".repeat(1024)),
            })
            .collect();
        let receipt = InvocationReceipt::new(&invocation, None);
        assert!(receipt.matches(&invocation));
        assert!(receipt.is_valid());
        let serialized = serde_json::to_vec(&receipt).unwrap();
        assert!(serialized.len() < 512);
        assert!(
            !String::from_utf8(serialized)
                .unwrap()
                .contains("private-argument")
        );
        let mut altered = invocation.clone();
        altered.arguments[0].value = protocol::InvocationValue::String("different".into());
        assert!(!receipt.matches(&altered));
        altered = invocation.clone();
        altered.name = "alias".into();
        assert!(!receipt.matches(&altered));
        altered = invocation.clone();
        altered.arguments.reverse();
        assert!(!receipt.matches(&altered));
        let state = PendingActivationState {
            invocation_receipts: vec![receipt; MAX_INVOCATION_RECEIPTS],
            ..PendingActivationState::default()
        };
        assert!(serde_json::to_vec(&state).unwrap().len() < 512 * MAX_INVOCATION_RECEIPTS);
    }

    #[test]
    fn feature_invocation_structurally_valid_corrupt_receipts_fail_closed_on_restore() {
        let Segment::FeatureInvoke { invocation } = recovery_invocation("corrupt") else {
            unreachable!()
        };
        let receipt = InvocationReceipt::new(&invocation, None);
        let valid = serde_json::to_value(PendingActivationState {
            invocation_receipts: vec![receipt],
            ..PendingActivationState::default()
        })
        .unwrap();
        let mut variants = Vec::new();
        let mut digest = valid.clone();
        digest["invocation_receipts"][0]["payload_digest"] = serde_json::json!("not-sha256");
        variants.push(digest);
        let mut duplicate = valid.clone();
        duplicate["invocation_receipts"]
            .as_array_mut()
            .unwrap()
            .push(valid["invocation_receipts"][0].clone());
        variants.push(duplicate);
        let mut identity = valid.clone();
        identity["invocation_receipts"][0]["result"] =
            serde_json::to_value(protocol::FeatureInvocationResult {
                invocation_id: "foreign".into(),
                identity: invocation.identity.clone(),
                status: protocol::FeatureInvocationStatus::Succeeded,
                message: "done".into(),
                context: None,
            })
            .unwrap();
        variants.push(identity);
        let mut oversized = valid.clone();
        oversized["invocation_receipts"][0]["result"] =
            serde_json::to_value(protocol::FeatureInvocationResult {
                invocation_id: invocation.invocation_id.clone(),
                identity: invocation.identity.clone(),
                status: protocol::FeatureInvocationStatus::Succeeded,
                message: "done".into(),
                context: Some("x".repeat(16385)),
            })
            .unwrap();
        variants.push(oversized);
        let mut old_shape = valid;
        old_shape["invocation_receipts"][0]["invocation"] =
            serde_json::to_value(invocation).unwrap();
        variants.push(old_shape);
        for payload in variants {
            let mut session = WorkerSession::new(session_store::new_session_id(), Vec::new());
            session.restore_pending_activations(&[(
                SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.into(),
                payload,
            )]);
            assert!(
                session
                    .pending_activations
                    .lock()
                    .unwrap()
                    .invocation_recovery_blocked
            );
        }
    }

    #[tokio::test]
    async fn feature_invocation_unknown_segments_rejected_at_queue_admission() {
        let (_dir, worker) = rewind_test_worker().await;
        let handle = worker.pending_submission_handle();
        for activate_now in [true, false] {
            let before = handle.persisted_entries_for_test().len();
            assert!(
                handle
                    .accept(
                        "unknown".into(),
                        vec![Segment::text("intent"), Segment::Unknown],
                        activate_now
                    )
                    .is_err()
            );
            assert_eq!(handle.persisted_entries_for_test().len(), before);
            assert!(handle.snapshot().submissions.is_empty());
        }
    }

    #[test]
    fn restoring_an_in_flight_activation_requeues_it_at_the_fifo_head() {
        let mut session = WorkerSession::new(session_store::new_session_id(), Vec::new());
        let state = PendingActivationState {
            next_activation_sequence: 3,
            invocation_receipts: Vec::new(),
            invocation_recovery_blocked: false,
            activating: Some(PendingSubmission {
                submission_request_id: "request-1".into(),
                source_namespace: "direct:test".into(),
                submission_id: "submission-1".into(),
                payload_digest: submission_payload_digest(&[Segment::text("first")]),
                accepted_at_ms: 1,
                activation_sequence: 0,
                provenance: WorkerHistoryProvenance::LegacyUnknown,
                was_queued: false,
                input: vec![Segment::text("first")],
            }),
            activating_notifications: VecDeque::from([PendingNotification {
                notification_request_id: "notification-1".into(),
                source_namespace: "account:account-1".into(),
                message: "deferred notice".into(),
                payload_digest: notification_payload_digest("deferred notice"),
                accepted_at_ms: 3,
                activation_sequence: 2,
                provenance: WorkerHistoryProvenance::HumanInput {
                    account_id: "account-1".into(),
                },
            }]),
            pending: VecDeque::from([PendingSubmission {
                submission_request_id: "request-2".into(),
                source_namespace: "direct:test".into(),
                submission_id: "submission-2".into(),
                payload_digest: submission_payload_digest(&[Segment::text("second")]),
                accepted_at_ms: 2,
                activation_sequence: 1,
                provenance: WorkerHistoryProvenance::LegacyUnknown,
                was_queued: true,
                input: vec![Segment::text("second")],
            }]),
            pending_notifications: VecDeque::new(),
            receipts: VecDeque::new(),
            notification_receipts: VecDeque::from([NotificationReceipt {
                notification_request_id: "notification-1".into(),
                source_namespace: "account:account-1".into(),
                payload_digest: notification_payload_digest("deferred notice"),
            }]),
        };
        let mut payload = serde_json::to_value(state).unwrap();
        let legacy_activating = payload["activating_notification"]
            .as_array_mut()
            .expect("new state serializes an activating notification array")
            .pop()
            .expect("one activating notification");
        payload["activating_notification"] = legacy_activating;
        session.restore_pending_activations(&[(
            SESSION_PENDING_ACTIVATIONS_EXTENSION_DOMAIN.into(),
            payload,
        )]);
        let state = session
            .pending_activations
            .lock()
            .expect("pending activation state poisoned");
        assert!(state.activating.is_none());
        assert_eq!(state.pending.len(), 2);
        assert_eq!(state.pending[0].submission_id, "submission-1");
        assert_eq!(state.pending[1].submission_id, "submission-2");
        assert!(state.activating_notifications.is_empty());
        assert_eq!(state.pending_notifications.len(), 1);
        assert!(matches!(
            state.pending_notifications[0].provenance,
            WorkerHistoryProvenance::HumanInput { ref account_id } if account_id == "account-1"
        ));
        assert_eq!(state.notification_receipts.len(), 1);
    }

    #[tokio::test]
    async fn restore_does_not_synthesize_compaction_lifecycle_history() {
        let (_dir, mut worker) = rewind_test_worker().await;
        let before = worker.sink.subscribe_with_snapshot().0;

        worker.recover_unfinished_compaction().await.unwrap();

        let after = worker.sink.subscribe_with_snapshot().0;
        assert_eq!(format!("{after:?}"), format!("{before:?}"));
        assert!(!after.iter().any(|entry| {
            matches!(
                entry,
                LogEntry::Extension { domain, .. }
                    if domain == "yoi.compaction"
            )
        }));
    }

    fn input_schema(entry: &session_store::LoggedSystemHistoryEntry) -> Option<&str> {
        entry
            .metadata
            .input_metadata
            .as_ref()?
            .feature_payload()
            .map(|payload| payload.schema())
    }

    fn startup_test_input() -> session_store::LoggedSystemHistoryEntry {
        use session_store::*;
        let payload = FeatureInputPayload::encode(
            "journal:startup",
            1,
            &crate::feature::builtin::journal::JournalStartupInput {
                snapshot: subjektiv::journal::startup::JournalStartupSnapshot {
                    subject_id: "subject".into(),
                    journals: vec![],
                    omitted: false,
                },
                snapshot_sha256: "snapshot-digest".into(),
                prompt_provenance: PromptRenderProvenance {
                    workspace_id: None,
                    source_digest: "source".into(),
                    projection_digest: "projection".into(),
                    logical_name: "internal.subjektiv_journal_startup".into(),
                },
            },
        )
        .unwrap();
        LoggedSystemHistoryEntry::from_input(
            LoggedSessionHistoryOrigin::Provided {
                producer: "arbitrary-context-provider".into(),
            },
            None,
            SystemInputEnvelope::new(
                "PRIOR_JOURNAL_REFERENCE",
                HistoryInputMetadata::new(
                    HistoryInputUsePolicy {
                        recording_material: InputEvidenceUse::Exclude,
                        memory_evidence: InputEvidenceUse::Exclude,
                        observation: InputObservationUse::Hidden,
                        compaction: InputCompactionUse::ReferenceInput,
                    },
                    Some(payload),
                    None,
                )
                .unwrap(),
            )
            .unwrap(),
        )
    }
    struct StartupTestSource {
        calls: Arc<AtomicUsize>,
        fail: Arc<AtomicBool>,
    }
    #[async_trait]
    impl SessionContextSource for StartupTestSource {
        async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err("startup read failed".into());
            }
            Ok(vec![startup_test_input()])
        }
    }

    #[tokio::test]
    async fn initial_system_messages_preserve_feature_metadata_without_payload_classification() {
        struct Source(Vec<session_store::LoggedSystemHistoryEntry>);
        #[async_trait]
        impl SessionContextSource for Source {
            async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
                Ok(self.0.clone())
            }
        }
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let expected = vec![
            session_store::LoggedSystemHistoryEntry {
                item: SystemItem::Notification {
                    message: "reference".into(),
                    body: "reference".into(),
                    prompt_provenance: None,
                },
                metadata: new_history_metadata(WorkerHistoryProvenance::JournalContext, None),
            },
            {
                let mut input = startup_test_input();
                let custom = new_history_metadata(
                    WorkerHistoryProvenance::BackendInstruction {
                        operation_id: Some("feature-selected-origin".into()),
                    },
                    Some(SessionHistoryDerivation {
                        sources: vec![SessionHistoryEntryId::new()],
                    }),
                );
                input.metadata.origin = custom.origin;
                input.metadata.derivation = custom.derivation;
                input
            },
        ];
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.install_session_context_source(Arc::new(Source(expected.clone())));
        worker.materialize_durable_session_head().await.unwrap();
        let log = store
            .read_all(worker.session_id(), worker.segment_id())
            .unwrap();
        let saved: Vec<_> = log
            .iter()
            .filter_map(|entry| match entry {
                LogEntry::AnnotatedSystemItem { entry, .. } => Some(entry),
                _ => None,
            })
            .collect();
        assert_eq!(
            serde_json::to_value(saved).unwrap(),
            serde_json::to_value(&expected).unwrap()
        );
        let replay =
            restore_history_entries(worker.session_id(), worker.segment_id(), &log).unwrap();
        assert_eq!(replay.len(), expected.len());
        for (entry, expected) in replay.iter().zip(&expected) {
            assert_eq!(entry.annotation, expected.metadata);
            assert_eq!(
                entry.item.as_text(),
                Some(expected.item.history_text().as_str())
            );
            assert!(matches!(
                entry.item,
                Item::Message {
                    role: agen::llm_client::types::Role::System,
                    ..
                }
            ));
        }
    }

    // Real FsStore atomic seed/replay and Core initial lifecycle. Only the model
    // boundary and returned Store response are replaced; no live Host is claimed.
    #[tokio::test]
    async fn initial_seed_write_failures_retry_exact_context_before_user() {
        for failure in [3, 4] {
            let temp = tempfile::tempdir().unwrap();
            let store = FailRunTerminalStore {
                inner: session_store::FsStore::new(temp.path()).unwrap(),
                fail_run_terminal: Arc::new(AtomicBool::new(false)),
                invocation_failure: Arc::new(AtomicUsize::new(failure)),
            };
            let mut worker = Worker::new(
                minimal_manifest(),
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
                store.clone(),
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            worker.install_session_context_source(Arc::new(StartupTestSource {
                calls: calls.clone(),
                fail: Arc::new(AtomicBool::new(false)),
            }));
            assert!(worker.run_text("must not be saved").await.is_err());
            assert!(worker.history().is_empty());
            assert_eq!(worker.segment_state.entries_written(), 0);
            worker.materialize_durable_session_head().await.unwrap();
            worker.materialize_durable_session_head().await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let entries = store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap();
            assert_eq!(entries.len(), 3);
            let replay =
                restore_history_entries(worker.session_id(), worker.segment_id(), &entries)
                    .unwrap();
            assert_eq!(replay.len(), 1);
            assert_eq!(replay[0].item.as_text(), Some("PRIOR_JOURNAL_REFERENCE"));
            assert_eq!(
                replay[0].annotation.origin,
                WorkerHistoryProvenance::Provided {
                    producer: "arbitrary-context-provider".into()
                }
            );
            let view = crate::session_capture::SessionCapture::from_complete_history(
                "session", "startup", replay,
            );
            let extract =
                crate::feature::builtin::memory_staging_output::render_extract_input(&view);
            assert!(!extract.contains("PRIOR_JOURNAL_REFERENCE"));
            let _ = worker.run_text("first accepted User").await;
            let entries = store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap();
            let journal = entries.iter().position(|entry| matches!(entry, LogEntry::AnnotatedSystemItem { entry, .. } if input_schema(entry) == Some("journal:startup"))).unwrap();
            let user = entries
                .iter()
                .position(|entry| matches!(entry, LogEntry::AnnotatedUserInput { .. }))
                .unwrap();
            assert!(journal < user);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn initial_read_error_is_not_initialized_empty_and_retries_before_input() {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let fail = Arc::new(AtomicBool::new(true));
        worker.install_session_context_source(Arc::new(StartupTestSource {
            calls: calls.clone(),
            fail: fail.clone(),
        }));
        assert!(worker.run_text("blocked User").await.is_err());
        assert!(
            !store
                .exists(worker.session_id(), worker.segment_id())
                .unwrap()
        );
        fail.store(false, Ordering::SeqCst);
        worker.materialize_durable_session_head().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(worker.history().len(), 1);
    }

    struct PreparedPromptSource {
        loads: Arc<AtomicUsize>,
        completions: Arc<AtomicUsize>,
    }
    #[async_trait::async_trait]
    impl SystemPromptContributionSource for PreparedPromptSource {
        async fn load(&self) -> SystemPromptContribution {
            panic!("initial load must use its prepared completion")
        }
        async fn load_initial(
            &self,
        ) -> (
            SystemPromptContribution,
            Option<PromptContributionCompletion>,
        ) {
            self.loads.fetch_add(1, Ordering::SeqCst);
            let completed = self.completions.clone();
            (
                SystemPromptContribution::Ready("PREPARED_RESIDENT_BODY".into()),
                Some(Box::new(move || {
                    completed.fetch_add(1, Ordering::SeqCst);
                })),
            )
        }
    }

    // Production initial-head preparation/create/reconciliation and real FsStore.
    // Only returned write failures and the Feature completion counter are injected.
    #[tokio::test]
    async fn initial_prompt_completion_waits_for_commit_and_reconciliation_and_never_replays_to_replacement()
     {
        for failure in [3, 4, 0] {
            for replace_source in [false, true] {
                let temp = tempfile::tempdir().unwrap();
                let store = FailRunTerminalStore {
                    inner: session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                    fail_run_terminal: Arc::new(AtomicBool::new(false)),
                    invocation_failure: Arc::new(AtomicUsize::new(failure)),
                };
                let mut worker = Worker::new(
                    minimal_manifest(),
                    Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
                    store.clone(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::None,
                    Scope::empty(),
                )
                .await
                .unwrap();
                worker.set_system_prompt_template(
                    SystemPromptTemplate::parse("default", PromptCatalogSource::builtins_only())
                        .unwrap(),
                );
                let loads = Arc::new(AtomicUsize::new(0));
                let completed = Arc::new(AtomicUsize::new(0));
                worker.install_system_prompt_contribution(
                    Some(Arc::new(PreparedPromptSource {
                        loads: loads.clone(),
                        completions: completed.clone(),
                    })),
                    None,
                );
                if failure == 0 {
                    let fail = Arc::new(AtomicBool::new(true));
                    worker.worker_metadata_writer = Some(Arc::new(move |_| {
                        if fail.swap(false, Ordering::SeqCst) {
                            Err(std::io::Error::other("initial active pointer write failed").into())
                        } else {
                            Ok(())
                        }
                    }));
                }
                assert!(worker.materialize_durable_session_head().await.is_err());
                assert_eq!(completed.load(Ordering::SeqCst), 0);
                let new_completed = Arc::new(AtomicUsize::new(0));
                let new_loads = Arc::new(AtomicUsize::new(0));
                if replace_source {
                    worker.install_system_prompt_contribution(
                        Some(Arc::new(PreparedPromptSource {
                            loads: new_loads.clone(),
                            completions: new_completed.clone(),
                        })),
                        None,
                    );
                }
                worker.materialize_durable_session_head().await.unwrap();
                worker.materialize_durable_session_head().await.unwrap();
                assert_eq!(loads.load(Ordering::SeqCst), 1);
                assert_eq!(
                    completed.load(Ordering::SeqCst),
                    usize::from(!replace_source)
                );
                assert_eq!(new_completed.load(Ordering::SeqCst), 0);
                assert_eq!(new_loads.load(Ordering::SeqCst), 0);
                let saved = store
                    .read_all(worker.session_id(), worker.segment_id())
                    .unwrap();
                assert!(
                    matches!(&saved[0], LogEntry::AnnotatedSegmentStart { system_prompt: Some(prompt), .. } if prompt.contains("PREPARED_RESIDENT_BODY"))
                );
            }
        }
    }

    #[tokio::test]
    async fn initial_metadata_failure_repairs_pointer_without_refetch_or_user_append() {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let mut worker = Worker::new(
            minimal_manifest(),
            Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient),
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        worker.install_session_context_source(Arc::new(StartupTestSource {
            calls: calls.clone(),
            fail: Arc::new(AtomicBool::new(false)),
        }));
        let fail = Arc::new(AtomicBool::new(true));
        worker.worker_metadata_writer = Some(Arc::new(move |_| {
            if fail.swap(false, Ordering::SeqCst) {
                Err(std::io::Error::other("metadata response lost").into())
            } else {
                Ok(())
            }
        }));
        assert!(worker.run_text("blocked User").await.is_err());
        assert_eq!(worker.segment_state.entries_written(), 0);
        assert_eq!(
            store
                .read_all(worker.session_id(), worker.segment_id())
                .unwrap()
                .len(),
            3
        );
        worker.materialize_durable_session_head().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(worker.segment_state.entries_written(), 3);
    }

    #[tokio::test]
    async fn pending_reopen_recovers_committed_seed_before_refetch_and_normal_restore_keeps_it() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let store = session_store::CombinedStore::new(
            FailRunTerminalStore {
                inner: session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                fail_run_terminal: Arc::new(AtomicBool::new(false)),
                invocation_failure: Arc::new(AtomicUsize::new(4)),
            },
            session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        let mut manifest = minimal_manifest();
        manifest.worker.name = format!("startup-reopen-{}", uuid::Uuid::now_v7());
        manifest.scope.allow.clear();
        let mut engine = Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(NoopClient);
        engine.set_system_prompt("persisted initial prompt");
        let mut worker = Worker::new(
            manifest.clone(),
            engine,
            store.clone(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            Scope::empty(),
        )
        .await
        .unwrap();
        worker.enable_worker_metadata_write_through().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        worker.install_session_context_source(Arc::new(StartupTestSource {
            calls: calls.clone(),
            fail: Arc::new(AtomicBool::new(false)),
        }));
        let (session, segment) = (worker.session_id(), worker.segment_id());
        assert!(worker.materialize_durable_session_head().await.is_err());
        assert!(
            store
                .read_by_name(&manifest.worker.name)
                .unwrap()
                .unwrap()
                .active
                .unwrap()
                .segment_id
                .is_none()
        );
        drop(worker);
        for _ in 0..2 {
            let mut restored = Worker::<Box<dyn LlmClient>, _>::restore_pending_from_worker_metadata_with_context_and_model_client(
                &manifest.worker.name, manifest.clone(), store.clone(),
                PromptCatalogSource::builtins_only(), WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None, Some(Box::new(NoopClient))).await.unwrap();
            restored.install_session_context_source(Arc::new(StartupTestSource {
                calls: calls.clone(),
                fail: Arc::new(AtomicBool::new(true)),
            }));
            restored.materialize_durable_session_head().await.unwrap();
            assert_eq!(
                (restored.session_id(), restored.segment_id()),
                (session, segment)
            );
            assert_eq!(
                restored.history()[0].as_text(),
                Some("PRIOR_JOURNAL_REFERENCE")
            );
            assert_eq!(
                store
                    .read_by_name(&manifest.worker.name)
                    .unwrap()
                    .unwrap()
                    .active
                    .unwrap()
                    .segment_id,
                Some(segment)
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
        assert_eq!(store.list_segments(session).unwrap(), vec![segment]);
        assert_eq!(store.read_all(session, segment).unwrap().len(), 3);
    }

    #[derive(Clone, Copy, Default)]
    enum BodyPublicationAuthorization {
        #[default]
        Allowed,
        Denied,
        Unavailable,
        WrongSession,
        WrongSubject,
        MissingHost,
    }

    #[derive(Default)]
    struct BodyPublicationHost {
        authorization: Mutex<BodyPublicationAuthorization>,
        calls: Mutex<Vec<(crate::subjektiv::SubjektivHostContext, bool)>>,
        startup_calls: AtomicUsize,
        startup_unavailable: AtomicBool,
    }

    impl crate::subjektiv::SubjektivHost for BodyPublicationHost {
        fn authorize_operation(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            operation: &str,
        ) -> Result<(), crate::subjektiv::SubjektivHostError> {
            match operation {
                "memory.resident.read"
                | "memory.resident.supply"
                | "journal.startup.read"
                | "journal.startup.supply" => Ok(()),
                _ => Err(crate::subjektiv::SubjektivHostError::PermissionDenied(
                    format!("publication fixture does not grant {operation}"),
                )),
            }
        }

        fn settings(&self) -> crate::subjektiv::SubjektivHostSettings {
            crate::subjektiv::SubjektivHostSettings {
                language: "English".into(),
            }
        }
        fn journal_startup_granted(&self) -> bool {
            true
        }
        fn journal_startup_context(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
        ) -> Result<
            subjektiv::journal::startup::JournalStartupSnapshot,
            crate::subjektiv::SubjektivHostError,
        > {
            self.startup_calls.fetch_add(1, Ordering::SeqCst);
            if self.startup_unavailable.load(Ordering::SeqCst) {
                return Err(crate::subjektiv::SubjektivHostError::Unavailable(
                    "startup fetch must not occur on normal restore".into(),
                ));
            }
            Ok(subjektiv::journal::startup::JournalStartupSnapshot {
                subject_id: "publication-subject".into(),
                journals: vec![],
                omitted: false,
            })
        }
        fn record_session(
            &self,
            context: &crate::subjektiv::SubjektivHostContext,
            create_if_missing: bool,
        ) -> Result<server_api::SubjektivRecordSessionResponse, crate::subjektiv::SubjektivHostError>
        {
            self.calls
                .lock()
                .unwrap()
                .push((context.clone(), create_if_missing));
            let mut output = server_api::SubjektivRecordSessionResponse {
                subject_id: "publication-subject".into(),
                session_id: context.session_id.clone(),
            };
            if !create_if_missing {
                match *self.authorization.lock().unwrap() {
                    BodyPublicationAuthorization::Allowed
                    | BodyPublicationAuthorization::MissingHost => {}
                    BodyPublicationAuthorization::Denied => {
                        return Err(crate::subjektiv::SubjektivHostError::Conflict {
                            code: "body_disconnected".into(),
                            message: "current body publication denied".into(),
                        });
                    }
                    BodyPublicationAuthorization::Unavailable => {
                        return Err(crate::subjektiv::SubjektivHostError::Unavailable(
                            "current authority unavailable".into(),
                        ));
                    }
                    BodyPublicationAuthorization::WrongSession => {
                        output.session_id = "other-session".into();
                    }
                    BodyPublicationAuthorization::WrongSubject => {
                        output.subject_id = "other-subject".into();
                    }
                }
            }
            Ok(output)
        }
        fn memory(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            operation: server_api::SubjektivMemoryBackendOperation,
        ) -> Result<server_api::SubjektivMemoryBackendResponse, crate::subjektiv::SubjektivHostError>
        {
            match operation {
                server_api::SubjektivMemoryBackendOperation::ResidentContext(_) => {
                    Ok(server_api::SubjektivMemoryBackendResponse::ResidentContext(
                        server_api::SubjektivResidentContextOutput {
                            behavior_md: "Publication test behavior".into(),
                            memory_surface: memory::backend::MemoryResidentSummaryOutput {
                                availability:
                                    memory::backend::MemoryResidentSummaryAvailability::Ungenerated,
                                content: None,
                            },
                        },
                    ))
                }
                _ => Err(crate::subjektiv::SubjektivHostError::Unavailable(
                    "not used".into(),
                )),
            }
        }
        fn session(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: server_api::SubjektivSessionBackendOperation,
        ) -> Result<server_api::SubjektivSessionBackendResponse, crate::subjektiv::SubjektivHostError>
        {
            Err(crate::subjektiv::SubjektivHostError::Unavailable(
                "not used".into(),
            ))
        }
        fn stage_candidate(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: memory::backend::MemoryStageCandidateOperation,
        ) -> Result<server_api::SubjektivStageCandidateResponse, crate::subjektiv::SubjektivHostError>
        {
            Err(crate::subjektiv::SubjektivHostError::Unavailable(
                "not used".into(),
            ))
        }
        fn request_consolidation(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: memory::backend::MemoryConsolidateStagingOperation,
        ) -> Result<memory::backend::MemoryConsolidationOutput, crate::subjektiv::SubjektivHostError>
        {
            Err(crate::subjektiv::SubjektivHostError::Unavailable(
                "not used".into(),
            ))
        }
    }

    type BodyPublicationStore =
        session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

    fn body_publication_bootstrap(
        temp: &tempfile::TempDir,
        manifest: WorkerManifest,
        store: BodyPublicationStore,
        host: Arc<BodyPublicationHost>,
        runtime_managed: bool,
    ) -> crate::WorkerBootstrap<BodyPublicationStore> {
        let layout = if runtime_managed {
            crate::WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: temp.path().join("runtime/runs/1"),
            }
        } else {
            crate::WorkerBootstrapLayout::Direct {
                runtime_base: temp.path().join("runtime"),
            }
        };
        crate::WorkerBootstrap::new(
            manifest,
            store,
            PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::no_workspace(),
            WorkerFilesystemAuthority::None,
            layout,
        )
        .with_model_client(NoopClient)
        .with_subjektiv_host(host)
    }

    fn body_publication_fixture(
        temp: &tempfile::TempDir,
    ) -> (WorkerManifest, BodyPublicationStore) {
        let mut manifest = minimal_manifest();
        manifest.worker.name = format!("body-publication-{}", uuid::Uuid::now_v7());
        manifest.scope.allow.clear();
        manifest.feature.subjektiv.profile.enabled = true;
        manifest.feature.subjektiv.profile.extraction.enabled = false;
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        (manifest, store)
    }

    async fn stop_body_publication_controller(controller: crate::BootstrappedWorker) {
        controller
            .handle
            .send(protocol::Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), controller.shutdown)
            .await
            .unwrap()
            .unwrap();
        controller.controller_task.await.unwrap();
    }

    // Production Direct/runtime-managed Controller bootstrap and FsStore; only
    // the Host boundary and model are replaced. Host-side races are not claimed.
    #[tokio::test]
    async fn confirmed_body_publication_denial_preserves_attribution_without_controller_initialization()
     {
        for runtime_managed in [false, true] {
            for authorization in [
                BodyPublicationAuthorization::Denied,
                BodyPublicationAuthorization::Unavailable,
                BodyPublicationAuthorization::WrongSession,
                BodyPublicationAuthorization::WrongSubject,
                BodyPublicationAuthorization::MissingHost,
            ] {
                let temp = tempfile::tempdir().unwrap();
                let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
                let (manifest, store) = body_publication_fixture(&temp);
                let host = Arc::new(BodyPublicationHost::default());
                let mut prepared = body_publication_bootstrap(
                    &temp,
                    manifest.clone(),
                    store.clone(),
                    host.clone(),
                    runtime_managed,
                )
                .prepare()
                .await
                .unwrap();
                let session = prepared.worker().session_id();
                let before = store.read_by_name(&manifest.worker.name).unwrap().unwrap();
                assert!(matches!(
                    before.subjektiv_session_attribution,
                    Some(SubjektivSessionAttributionState::Confirmed { .. })
                ));
                *host.authorization.lock().unwrap() = authorization;
                let missing_host =
                    matches!(authorization, BodyPublicationAuthorization::MissingHost);
                if missing_host {
                    prepared.worker_mut().bind_subjektiv_host(None);
                }
                let error = match prepared.start().await {
                    Err(crate::WorkerBootstrapError::Controller { source, .. }) => source,
                    Err(other) => panic!("expected publication rejection, got {other:?}"),
                    Ok(controller) => {
                        stop_body_publication_controller(controller).await;
                        panic!("denied body was exposed");
                    }
                };
                assert!(error.to_string().contains("publication_denied"));
                let after = store.read_by_name(&manifest.worker.name).unwrap().unwrap();
                assert_eq!(
                    before.subjektiv_session_attribution,
                    after.subjektiv_session_attribution
                );
                assert_eq!(before.active, after.active);
                assert!(store.list_segments(session).unwrap().is_empty());
                assert!(!temp.path().join("runtime").exists());
                assert_eq!(host.startup_calls.load(Ordering::SeqCst), 0);
                let calls = host.calls.lock().unwrap();
                assert_eq!(calls.len(), if missing_host { 1 } else { 2 });
                assert!(calls[0].1);
                if !missing_host {
                    assert!(!calls[1].1);
                    assert_eq!(calls[1].0.worker_id, manifest.worker.name);
                    assert_eq!(calls[1].0.session_id, session.to_string());
                }
            }
        }
    }

    #[tokio::test]
    async fn normal_restore_revalidates_current_body_authority_without_refetching_empty_startup() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let (manifest, store) = body_publication_fixture(&temp);
        let host = Arc::new(BodyPublicationHost::default());
        let prepared =
            body_publication_bootstrap(&temp, manifest.clone(), store.clone(), host.clone(), false)
                .prepare()
                .await
                .unwrap();
        let (session, segment) = (
            prepared.worker().session_id(),
            prepared.worker().segment_id(),
        );
        let controller = prepared.start().await.unwrap();
        let initial = store.read_all(session, segment).unwrap();
        let saved_startup = initial
            .iter()
            .filter_map(|entry| match entry {
                LogEntry::AnnotatedSystemItem { entry, .. }
                    if input_schema(entry) == Some("journal:startup") =>
                {
                    Some(entry.item.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(saved_startup.is_empty());
        stop_body_publication_controller(controller).await;
        assert_eq!(host.startup_calls.load(Ordering::SeqCst), 1);
        host.startup_unavailable.store(true, Ordering::SeqCst);
        for allowed in [true, false] {
            *host.authorization.lock().unwrap() = if allowed {
                BodyPublicationAuthorization::Allowed
            } else {
                BodyPublicationAuthorization::Denied
            };
            let calls_before = host.calls.lock().unwrap().len();
            let before = store.read_all(session, segment).unwrap();
            let attribution = store
                .read_by_name(&manifest.worker.name)
                .unwrap()
                .unwrap()
                .subjektiv_session_attribution;
            let prepared = body_publication_bootstrap(
                &temp,
                manifest.clone(),
                store.clone(),
                host.clone(),
                false,
            )
            .prepare_restored(&manifest.worker.name)
            .await
            .unwrap();
            assert_eq!(
                host.calls.lock().unwrap().len(),
                calls_before,
                "finalization of historical Confirmed attribution remains separate"
            );
            let result = prepared.start().await;
            let calls = host.calls.lock().unwrap().clone();
            assert_eq!(calls.len(), calls_before + 1);
            assert!(!calls.last().unwrap().1);
            assert_eq!(host.startup_calls.load(Ordering::SeqCst), 1);
            if allowed {
                let controller = result.unwrap();
                let entries = controller.handle.committed_entries();
                let saved = entries
                    .iter()
                    .filter_map(|entry| match entry {
                        LogEntry::AnnotatedSystemItem { entry, .. }
                            if input_schema(entry) == Some("journal:startup") =>
                        {
                            Some(&entry.item)
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    serde_json::to_value(saved).unwrap(),
                    serde_json::to_value(&saved_startup).unwrap()
                );
                stop_body_publication_controller(controller).await;
            } else {
                assert!(result.is_err());
                assert_eq!(
                    serde_json::to_value(store.read_all(session, segment).unwrap()).unwrap(),
                    serde_json::to_value(before).unwrap()
                );
            }
            assert_eq!(
                store
                    .read_by_name(&manifest.worker.name)
                    .unwrap()
                    .unwrap()
                    .subjektiv_session_attribution,
                attribution
            );
        }
    }

    #[test]
    fn system_input_completion_observes_durable_identity_under_publication_barrier() {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let session_id = SessionId::now_v7();
        let segment_id = SegmentId::now_v7();
        let start = LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: agen::llm_client::RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        };
        store
            .create_segment(session_id, segment_id, std::slice::from_ref(&start))
            .unwrap();
        let writer = LogWriterHandle {
            store: store.clone(),
            state: SegmentState::new(session_id, segment_id, 1),
            sink: SegmentLogSink::with_initial(vec![start]),
            in_flight: None,
        };
        let mut receipt = None;
        let committed = writer
            .commit_system_item_with_extensions_then(
                SystemItem::Notification {
                    message: "message".into(),
                    body: "exact committed system body".into(),
                    prompt_provenance: None,
                },
                vec![],
                None,
                &mut |history| {
                    assert!(writer.state.append_lock.try_lock().is_err());
                    let durable = store.read_all(session_id, segment_id).unwrap();
                    let Some(LogEntry::AnnotatedSystemItem { entry, .. }) = durable.last() else {
                        panic!("completion must follow actual storage append");
                    };
                    assert_eq!(entry.metadata, history.annotation);
                    assert_eq!(entry.item.to_history_item(), history.item);
                    assert_eq!(history.item.as_text(), Some("exact committed system body"));
                    receipt = Some(history.annotation.entry_id.clone());
                },
            )
            .unwrap();
        assert_eq!(receipt, Some(committed.annotation.entry_id));
        assert!(writer.state.append_lock.try_lock().is_ok());
    }

    // Production bootstrap, Controller registration, and durable stores; only the
    // Workspace transport and model are replaced. No live Server is needed.
    #[derive(Debug, Default)]
    struct FeatureAdmissionClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
        response_status: Mutex<Option<u16>>,
    }

    impl WorkspaceClient for FeatureAdmissionClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("feature-restore-workspace")
        }

        fn kind(&self) -> &str {
            "feature-admission-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            assert!(request.path.ends_with("/authorization/check"));
            self.requests.lock().unwrap().push(request);
            match *self.response_status.lock().unwrap() {
                Some(status) => Ok(WorkspaceResponse {
                    status,
                    body: if status == 200 {
                        r#"{"scope":null}"#.into()
                    } else {
                        r#"{"message":"current grant denied"}"#.into()
                    },
                }),
                None => Err(WorkspaceClientError::Unavailable("Host not serving".into())),
            }
        }
    }

    fn feature_restore_bootstrap(
        temp: &tempfile::TempDir,
        manifest: WorkerManifest,
        store: BodyPublicationStore,
        client: Arc<FeatureAdmissionClient>,
    ) -> crate::WorkerBootstrap<BodyPublicationStore> {
        crate::WorkerBootstrap::new(
            manifest,
            store,
            PromptCatalogSource::builtins_only(),
            WorkerWorkspaceContext::with_client(
                Some(WorkspaceId::new("feature-restore-workspace").unwrap()),
                client,
            ),
            WorkerFilesystemAuthority::None,
            crate::WorkerBootstrapLayout::RuntimeManagedRun {
                run_dir: temp
                    .path()
                    .join("runtime/runs")
                    .join(uuid::Uuid::now_v7().to_string()),
            },
        )
        .with_model_client(NoopClient)
    }

    fn feature_restore_fixture(temp: &tempfile::TempDir) -> (WorkerManifest, BodyPublicationStore) {
        let mut manifest = minimal_manifest();
        manifest.worker.name = format!("feature-restore-{}", uuid::Uuid::now_v7());
        manifest.scope.allow.clear();
        manifest.feature.task.enabled = true;
        manifest.feature.ticket.enabled = true;
        manifest.profile = Some(manifest::ProfileManifestSnapshot {
            source: manifest::ProfileSource::Registry {
                source: manifest::ProfileRegistrySource::Builtin,
                name: "default".into(),
                path: None,
                provenance: None,
            },
            profile: None,
        });
        let store = session_store::CombinedStore::new(
            session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
            session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
        );
        (manifest, store)
    }

    #[tokio::test]
    async fn new_worker_registers_local_features_without_operation_grants() {
        for status in [Some(403), None] {
            let temp = tempfile::tempdir().unwrap();
            let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
            let (manifest, store) = feature_restore_fixture(&temp);
            let client = Arc::new(FeatureAdmissionClient::default());
            *client.response_status.lock().unwrap() = status;
            let prepared =
                feature_restore_bootstrap(&temp, manifest.clone(), store.clone(), client.clone())
                    .prepare()
                    .await
                    .unwrap();
            assert!(store.read_by_name(&manifest.worker.name).unwrap().is_some());
            assert!(client.requests.lock().unwrap().is_empty());
            let controller = prepared.start().await.unwrap();
            assert!(client.requests.lock().unwrap().is_empty());
            let installed: WorkerManifest =
                toml::from_str(&controller.handle.shared_state.manifest_toml).unwrap();
            assert!(installed.feature.task.enabled);
            assert!(installed.feature.ticket.enabled);
            stop_body_publication_controller(controller).await;
        }
    }

    #[tokio::test]
    async fn new_worker_publication_does_not_reintroduce_feature_activation_gate() {
        let temp = tempfile::tempdir().unwrap();
        let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
        let (manifest, store) = feature_restore_fixture(&temp);
        let client = Arc::new(FeatureAdmissionClient::default());
        *client.response_status.lock().unwrap() = Some(200);
        let prepared = feature_restore_bootstrap(&temp, manifest, store, client.clone())
            .prepare()
            .await
            .unwrap();
        assert!(client.requests.lock().unwrap().is_empty());
        *client.response_status.lock().unwrap() = Some(403);
        let controller = prepared.start().await.unwrap();
        assert!(client.requests.lock().unwrap().is_empty());
        stop_body_publication_controller(controller).await;
    }

    #[tokio::test]
    async fn restore_preserves_saved_features_and_prompt_without_feature_readmission() {
        for materialized in [false, true] {
            for response_status in [Some(403), None] {
                let temp = tempfile::tempdir().unwrap();
                let _sandbox = worker_allocation::test_util::RuntimeDirSandbox::new(temp.path());
                let (manifest, store) = feature_restore_fixture(&temp);
                let client = Arc::new(FeatureAdmissionClient::default());
                *client.response_status.lock().unwrap() = Some(200);
                let prepared = feature_restore_bootstrap(
                    &temp,
                    manifest.clone(),
                    store.clone(),
                    client.clone(),
                )
                .prepare()
                .await
                .unwrap();
                let session = prepared.worker().session_id();
                let mut saved = Vec::new();
                let mut saved_prompt = None;
                let mut saved_segment = None;
                if materialized {
                    let segment = prepared.worker().segment_id();
                    let controller = prepared.start().await.unwrap();
                    saved = store.read_all(session, segment).unwrap();
                    saved_prompt = session_store::collect_state(&saved).system_prompt;
                    assert!(saved_prompt.is_some());
                    saved_segment = Some(segment);
                    stop_body_publication_controller(controller).await;
                } else {
                    drop(prepared);
                    assert!(store.list_segments(session).unwrap().is_empty());
                }
                client.requests.lock().unwrap().clear();
                *client.response_status.lock().unwrap() = response_status;
                let mut fallback = manifest.clone();
                fallback.feature.task.enabled = false;
                fallback.feature.ticket.enabled = false;
                fallback.feature.workdir_catalog.enabled = false;
                fallback.engine.instruction = "must not replace saved instructions".into();
                let restored =
                    feature_restore_bootstrap(&temp, fallback, store.clone(), client.clone())
                        .prepare_restored(&manifest.worker.name)
                        .await
                        .unwrap();
                assert_eq!(restored.worker().session_id(), session);
                assert_eq!(restored.worker().manifest().feature, manifest.feature);
                assert_eq!(
                    restored.worker().manifest().engine.instruction,
                    manifest.engine.instruction
                );
                if let Some(prompt) = &saved_prompt {
                    assert_eq!(
                        restored.worker().engine().get_system_prompt(),
                        Some(prompt.as_str())
                    );
                    assert_eq!(Some(restored.worker().segment_id()), saved_segment);
                }
                assert!(client.requests.lock().unwrap().is_empty());
                let controller = restored.start().await.unwrap();
                let installed: WorkerManifest =
                    toml::from_str(&controller.handle.shared_state.manifest_toml).unwrap();
                assert_eq!(installed.feature, manifest.feature);
                let committed = controller.handle.committed_entries();
                if materialized {
                    assert_eq!(
                        serde_json::to_value(&committed[..saved.len()]).unwrap(),
                        serde_json::to_value(&saved).unwrap(),
                    );
                    assert_eq!(
                        session_store::collect_state(&committed).system_prompt,
                        saved_prompt
                    );
                } else {
                    assert!(
                        session_store::collect_state(&committed)
                            .system_prompt
                            .is_some()
                    );
                }
                assert!(
                    client.requests.lock().unwrap().is_empty(),
                    "restore must not ask for Feature admission"
                );
                stop_body_publication_controller(controller).await;
            }
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

    mod composition_tests {
        include!("worker_composition_tests.rs");
    }

    // Real initial-seed/replay/fork/compaction and Memory source wiring. Only the
    // Host HTTP boundary and provider transport are fixtures; Host policy evaluation
    // itself is covered by workspace-server authorization tests, not asserted here.
    mod subject_job_input_tests {
        include!("worker_job_input_supply_tests.rs");
    }

    mod protected_input_tests {
        fn assert_supply_outcome(outcome: Result<WorkerRunResult, WorkerError>, allowed: bool) {
            if allowed {
                assert!(
                    matches!(outcome, Ok(WorkerRunResult::Finished)),
                    "{outcome:?}"
                );
            } else {
                assert!(
                    matches!(&outcome, Ok(WorkerRunResult::Interrupted { message, .. })
                    if message.contains("protected input supply rejected:")),
                    "{outcome:?}"
                );
            }
        }

        use super::*;
        use agen::llm_client::event::{Event, ResponseStatus, StatusEvent};
        type TestStore =
            session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>;

        #[derive(Debug)]
        struct Host {
            status: AtomicUsize,
            checks: AtomicUsize,
            reads: Arc<AtomicUsize>,
        }
        impl Default for Host {
            fn default() -> Self {
                Self {
                    status: AtomicUsize::new(200),
                    checks: AtomicUsize::new(0),
                    reads: Arc::default(),
                }
            }
        }
        impl WorkspaceClient for Host {
            fn workspace_id(&self) -> Option<&str> {
                Some("workspace-test")
            }
            fn kind(&self) -> &str {
                "protected-input-host"
            }
            fn is_available(&self) -> bool {
                true
            }
            fn execute(
                &self,
                request: WorkspaceRequest,
            ) -> Result<WorkspaceResponse, WorkspaceClientError> {
                if request.path.ends_with("/authorization/check") {
                    self.checks.fetch_add(1, Ordering::SeqCst);
                    let input: server_api::authorization::OperationAuthorizationRequest =
                        serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                    assert!(matches!(
                        input.operation.as_str(),
                        "memory.resident.read" | "memory.resident.supply"
                    ));
                    assert_eq!(
                        input.resource,
                        server_api::authorization::PermissionResource::Workspace {
                            workspace_id: "workspace-test".into()
                        }
                    );
                    return Ok(WorkspaceResponse {
                        status: self.status.load(Ordering::SeqCst) as u16,
                        body: if self.status.load(Ordering::SeqCst) == 200 {
                            serde_json::json!({"scope": input.resource}).to_string()
                        } else {
                            serde_json::json!({"message":"current permission revoked"}).to_string()
                        },
                    });
                }
                RestoreLegacyResidentClient {
                    content: "RETAINED_PROTECTED_INPUT".into(),
                    load_count: self.reads.clone(),
                }
                .execute(request)
            }
        }

        #[derive(Clone, Default)]
        struct Provider {
            calls: Arc<AtomicUsize>,
            compact_calls: Arc<AtomicUsize>,
            revoke_on_compact: Option<Arc<Host>>,
        }
        #[async_trait]
        impl LlmClient for Provider {
            async fn stream(
                &self,
                request: agen::llm_client::Request,
            ) -> Result<agen::llm_client::ResponseStream, agen::llm_client::ClientError>
            {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let events = if request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "write_summary")
                    && self.compact_calls.fetch_add(1, Ordering::SeqCst) == 0
                {
                    if let Some(host) = &self.revoke_on_compact {
                        host.status.store(403, Ordering::SeqCst);
                    }
                    vec![
                        Event::tool_use_start(0, "summary", "write_summary"),
                        Event::tool_input_delta(
                            0,
                            serde_json::json!({"text":"Retained protected summary"}).to_string(),
                        ),
                        Event::tool_use_stop(0),
                    ]
                } else {
                    vec![
                        Event::text_block_start(0),
                        Event::text_delta(0, "done"),
                        Event::text_block_stop(0, None),
                    ]
                };
                Ok(Box::pin(futures::stream::iter(
                    events
                        .into_iter()
                        .chain(std::iter::once(Event::Status(StatusEvent {
                            status: ResponseStatus::Completed,
                        })))
                        .map(Ok),
                )))
            }
            fn clone_boxed(&self) -> Box<dyn LlmClient> {
                Box::new(self.clone())
            }
        }

        fn store(temp: &tempfile::TempDir) -> TestStore {
            session_store::CombinedStore::new(
                session_store::FsStore::new(temp.path().join("sessions")).unwrap(),
                session_store::FsWorkerStore::new(temp.path().join("workers")).unwrap(),
            )
        }
        fn memory_manifest() -> WorkerManifest {
            let mut manifest = minimal_manifest();
            manifest.worker.name = format!("protected-input-{}", uuid::Uuid::now_v7());
            manifest.scope.allow.clear();
            manifest.feature.memory.profile.enabled = true;
            manifest
                .feature
                .memory
                .bind_workspace_settings(manifest::WorkspaceMemorySettingsSnapshot {
                    workspace_id: "workspace-test".into(),
                    language: "English".into(),
                })
                .unwrap();
            manifest
        }
        fn install_memory<C: LlmClient + 'static>(
            worker: &mut Worker<C, TestStore>,
            manifest: &WorkerManifest,
            host: Arc<Host>,
        ) {
            let plan = crate::feature::builtin::memory::MemoryFeatureInstallPlan::prepare(
                manifest,
                host,
                worker.prompts().load_full(),
            )
            .unwrap()
            .unwrap();
            let source = plan.resident_summary_source;
            assert!(
                !worker
                    .install_features(FeatureRegistryBuilder::new().with_module(plan.module))
                    .has_errors()
            );
            worker.install_system_prompt_contribution(source, None);
        }
        async fn boot(
            manifest: WorkerManifest,
            store: TestStore,
            provider: Provider,
            host: Arc<Host>,
        ) -> Worker<Provider, TestStore> {
            let mut worker = Worker::new(
                manifest.clone(),
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(provider),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            worker.set_system_prompt_template(
                SystemPromptTemplate::parse("default", PromptCatalogSource::builtins_only())
                    .unwrap(),
            );
            install_memory(&mut worker, &manifest, host);
            worker.materialize_durable_session_head().await.unwrap();
            worker
        }
        async fn restore(
            manifest: WorkerManifest,
            store: TestStore,
            session: SessionId,
            segment: SegmentId,
            provider: Provider,
        ) -> Worker<Box<dyn LlmClient>, TestStore> {
            Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(
                session,
                segment,
                manifest,
                store,
                PromptCatalogSource::builtins_only(),
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Some(Box::new(provider)),
            )
            .await
            .unwrap()
        }
        fn assert_protected(store: &TestStore, session: SessionId, segment: SegmentId) {
            let log = store.read_all(session, segment).unwrap();
            assert!(log.iter().any(|entry| matches!(entry, LogEntry::Extension { domain, payload, .. } if domain == PROTECTED_INPUTS_DOMAIN && payload["kinds"] == serde_json::json!(["workspace-memory"]))));
        }

        struct WrongSource;
        #[async_trait]
        impl SystemPromptContributionSource for WrongSource {
            fn protected_input_kind(&self) -> Option<&'static str> {
                Some("subject-resident")
            }
            async fn load(&self) -> SystemPromptContribution {
                panic!("restore must not reload")
            }
        }

        #[tokio::test]
        async fn cached_memory_restore_requires_matching_source_and_current_supply_authority() {
            for mode in ["removed", "different-kind", "revoked", "allowed"] {
                let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let temp = tempfile::tempdir().unwrap();
                let store = store(&temp);
                let manifest = memory_manifest();
                let host = Arc::new(Host::default());
                let original = boot(
                    manifest.clone(),
                    store.clone(),
                    Provider::default(),
                    host.clone(),
                )
                .await;
                assert_eq!(host.reads.load(Ordering::SeqCst), 1);
                let (session, segment) = (original.session_id(), original.segment_id());
                let prompt = original.engine().get_system_prompt().unwrap().to_string();
                assert!(prompt.contains("RETAINED_PROTECTED_INPUT"));
                assert_protected(&store, session, segment);
                drop(original);
                let mut replacement = manifest.clone();
                replacement.feature.memory = Default::default();
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let mut restored =
                    restore(replacement, store.clone(), session, segment, provider).await;
                match mode {
                    "different-kind" => restored
                        .install_system_prompt_contribution(Some(Arc::new(WrongSource)), None),
                    "revoked" | "allowed" => install_memory(&mut restored, &manifest, host.clone()),
                    _ => {}
                }
                if mode == "revoked" {
                    host.status.store(403, Ordering::SeqCst);
                }
                let checks = host.checks.load(Ordering::SeqCst);
                let outcome = restored.run_text("next cached supply").await;
                assert_supply_outcome(outcome, mode == "allowed");
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    usize::from(mode == "allowed"),
                    "{mode}"
                );
                assert_eq!(restored.engine().get_system_prompt(), Some(prompt.as_str()));
                assert_eq!(
                    host.reads.load(Ordering::SeqCst),
                    1,
                    "restore must not read again"
                );
                if mode == "revoked" {
                    assert!(host.checks.load(Ordering::SeqCst) > checks);
                }
            }
        }

        #[tokio::test]
        async fn provenance_survives_automatic_fork_and_compaction() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let manifest = memory_manifest();
            let host = Arc::new(Host::default());
            let mut worker = boot(manifest.clone(), store.clone(), Provider::default(), host).await;
            let session = worker.session_id();
            let first = worker.segment_id();
            store
                .append(
                    session,
                    first,
                    &LogEntry::Extension {
                        ts: 1,
                        domain: "test.concurrent-write".into(),
                        payload: serde_json::json!({}),
                    },
                )
                .unwrap();
            worker.run_text("fork retained input").await.unwrap();
            let fork = worker.segment_id();
            assert_ne!(fork, first);
            assert_protected(&store, session, fork);
            let compact = worker.compact(0).await.unwrap();
            assert_ne!(compact, fork);
            assert_protected(&store, session, compact);
            drop(worker);
            for segment in [fork, compact] {
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let mut local = manifest.clone();
                local.feature.memory = Default::default();
                let mut restored = restore(local, store.clone(), session, segment, provider).await;
                assert_supply_outcome(restored.run_text("must not leak").await, false);
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }

        #[tokio::test]
        async fn compactor_rechecks_supply_on_each_provider_request() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let manifest = memory_manifest();
            let host = Arc::new(Host::default());
            let provider = Provider {
                revoke_on_compact: Some(host.clone()),
                ..Default::default()
            };
            let compact_calls = provider.compact_calls.clone();
            let mut worker = boot(manifest, store.clone(), provider, host).await;
            worker.run_text("history for compaction").await.unwrap();
            let source = worker.segment_id();
            assert!(worker.compact(0).await.is_err());
            assert_eq!(
                compact_calls.load(Ordering::SeqCst),
                1,
                "revocation after first request must stop the next compactor request"
            );
            assert_eq!(worker.segment_id(), source);
        }

        struct RequestHook {
            append_journal: AtomicBool,
            revoke: Option<Arc<Host>>,
        }
        #[async_trait]
        impl Hook<PreLlmRequest> for RequestHook {
            async fn call(
                &self,
                context: &crate::hook::PreRequestContext,
            ) -> Result<crate::hook::HookPreRequestAction, crate::hook::HookError> {
                if self.append_journal.swap(false, Ordering::SeqCst) {
                    context
                        .system_items()
                        .unwrap()
                        .append_input(startup_test_input());
                }
                if let Some(host) = &self.revoke {
                    host.status.store(403, Ordering::SeqCst);
                }
                Ok(crate::hook::HookPreRequestAction::Continue)
            }
        }

        #[tokio::test]
        async fn request_hooks_cannot_supply_new_protected_input_without_its_source() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let mut manifest = minimal_manifest();
            manifest.scope.allow.clear();
            let provider = Provider::default();
            let calls = provider.calls.clone();
            let mut worker = Worker::new(
                manifest,
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(provider),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            worker.engine_mut().set_system_prompt("local only");
            worker.attach_log_writer(Arc::new(worker.log_writer_handle()));
            worker.add_pre_llm_request_hook(RequestHook {
                append_journal: AtomicBool::new(true),
                revoke: None,
            });
            assert_supply_outcome(worker.run_text("hook input must be gated").await, false);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(
                !worker
                    .history()
                    .iter()
                    .any(|item| item.as_text() == Some("PRIOR_JOURNAL_REFERENCE"))
            );
            // The denied hook input was never committed: it must not poison
            // the next local request with phantom retained provenance.
            assert_supply_outcome(worker.run_text("local retry").await, true);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn request_hooks_revoking_cached_input_stop_the_same_provider_request() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let host = Arc::new(Host::default());
            let provider = Provider::default();
            let calls = provider.calls.clone();
            let mut worker = boot(memory_manifest(), store, provider, host.clone()).await;
            worker.add_pre_llm_request_hook(RequestHook {
                append_journal: AtomicBool::new(false),
                revoke: Some(host),
            });
            assert_supply_outcome(worker.run_text("revoked during preparation").await, false);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn rewind_preserves_retained_prompt_provenance_after_source_removal() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let manifest = memory_manifest();
            let mut worker = boot(
                manifest.clone(),
                store.clone(),
                Provider::default(),
                Arc::new(Host::default()),
            )
            .await;
            assert_supply_outcome(worker.run_text("first").await, true);
            assert_supply_outcome(worker.run_text("rewind this").await, true);
            let target = rewind_test_inputs(&worker)[0].0.clone();
            worker.rewind_to(target).await.unwrap();
            let (session, segment) = (worker.session_id(), worker.segment_id());
            assert_protected(&store, session, segment);
            drop(worker);
            let mut local = manifest;
            local.feature.memory = Default::default();
            let provider = Provider::default();
            let calls = provider.calls.clone();
            let mut restored = restore(local, store, session, segment, provider).await;
            assert_supply_outcome(
                restored.run_text("retained prompt still protected").await,
                false,
            );
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn local_task_boot_and_restore_need_no_operation_authorization() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let mut manifest = minimal_manifest();
            manifest.scope.allow.clear();
            manifest.worker.name = format!("local-task-{}", uuid::Uuid::now_v7());
            manifest.feature.task.enabled = true;
            let host = Arc::new(Host::default());
            host.status.store(403, Ordering::SeqCst);
            let provider = Provider::default();
            let calls = provider.calls.clone();
            let mut worker = Worker::new(
                manifest.clone(),
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(provider.clone()),
                store.clone(),
                WorkerWorkspaceContext::with_client(
                    Some(WorkspaceId::new("workspace-test").unwrap()),
                    host.clone(),
                ),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            worker.engine_mut().set_system_prompt("local task only");
            worker.run_text("local task").await.unwrap();
            let (session, segment) = (worker.session_id(), worker.segment_id());
            drop(worker);
            let mut restored = restore(manifest, store, session, segment, provider).await;
            restored.run_text("local replay").await.unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            assert_eq!(host.checks.load(Ordering::SeqCst), 0);
        }

        #[tokio::test]
        async fn malformed_retention_extension_is_rejected_before_provider() {
            for payload in [
                serde_json::json!({"version":1,"kinds":["unknown-source"]}),
                serde_json::json!({"version":2,"kinds":[]}),
                serde_json::json!({"version":1,"kinds":[],"grant":true}),
            ] {
                let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let temp = tempfile::tempdir().unwrap();
                let store = store(&temp);
                let manifest = memory_manifest();
                let worker = boot(
                    manifest.clone(),
                    store.clone(),
                    Provider::default(),
                    Arc::new(Host::default()),
                )
                .await;
                let (session, segment) = (worker.session_id(), worker.segment_id());
                drop(worker);
                store
                    .append(
                        session,
                        segment,
                        &LogEntry::Extension {
                            ts: 1,
                            domain: PROTECTED_INPUTS_DOMAIN.into(),
                            payload,
                        },
                    )
                    .unwrap();
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let restored = Worker::<Box<dyn LlmClient>, _>::restore_from_manifest_with_context_and_model_client(session, segment, manifest, store, PromptCatalogSource::builtins_only(), WorkerWorkspaceContext::no_workspace(), WorkerFilesystemAuthority::None, Some(Box::new(provider))).await;
                assert!(restored.is_err());
                assert_eq!(calls.load(Ordering::SeqCst), 0);
            }
        }

        #[tokio::test]
        async fn legacy_manifest_retention_is_persisted_before_replacement_config() {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let manifest = memory_manifest();
            let original = boot(
                manifest.clone(),
                store.clone(),
                Provider::default(),
                Arc::new(Host::default()),
            )
            .await;
            let (session, segment) = (original.session_id(), original.segment_id());
            drop(original);
            let old = store.read_all(session, segment).unwrap().into_iter().filter(|entry| !matches!(entry, LogEntry::Extension { domain, .. } if domain == PROTECTED_INPUTS_DOMAIN)).collect::<Vec<_>>();
            store.create_segment(session, segment, &old).unwrap();
            let mut metadata = store
                .read_by_name(&manifest.worker.name)
                .unwrap()
                .unwrap_or_else(|| {
                    WorkerMetadata::new(
                        manifest.worker.name.clone(),
                        Some(WorkerActiveSegmentRef::active_segment(session, segment)),
                    )
                });
            metadata.resolved_manifest_snapshot =
                Some(manifest::write_persisted_worker_manifest_snapshot(&manifest).unwrap());
            WorkerMetadataStore::write(&store, &metadata).unwrap();
            let mut replacement = manifest.clone();
            replacement.feature.memory = Default::default();
            let restored = restore(
                replacement.clone(),
                store.clone(),
                session,
                segment,
                Provider::default(),
            )
            .await;
            assert_protected(&store, session, segment);
            drop(restored);
            let provider = Provider::default();
            let calls = provider.calls.clone();
            let mut again = restore(replacement, store, session, segment, provider).await;
            assert_supply_outcome(again.run_text("still protected").await, false);
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }

        #[derive(Debug)]
        struct JournalHostClient {
            allowed: Mutex<std::collections::BTreeSet<String>>,
            operations: Mutex<Vec<String>>,
            startup_protected: bool,
        }
        impl JournalHostClient {
            fn new(startup_protected: bool) -> Self {
                Self {
                    startup_protected,
                    operations: Mutex::default(),
                    allowed: Mutex::new(
                        [
                            "journal.startup.read",
                            "journal.startup.supply",
                            "journal.compaction.read",
                            "journal.compaction.supply",
                        ]
                        .into_iter()
                        .map(String::from)
                        .collect(),
                    ),
                }
            }
            fn supply_only(&self, operations: &[&str]) {
                *self.allowed.lock().unwrap() = operations.iter().map(|s| s.to_string()).collect();
                self.operations.lock().unwrap().clear();
            }
        }
        impl WorkspaceClient for JournalHostClient {
            fn workspace_id(&self) -> Option<&str> {
                Some("workspace-test")
            }
            fn kind(&self) -> &str {
                "journal-retained-http-fixture"
            }
            fn is_available(&self) -> bool {
                true
            }
            fn execute(
                &self,
                request: WorkspaceRequest,
            ) -> Result<WorkspaceResponse, WorkspaceClientError> {
                if request.path.ends_with("/authorization/bound-check") {
                    let input: server_api::authorization::BoundOperationAuthorizationRequest =
                        serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                    assert_eq!(
                        input.target,
                        server_api::authorization::BoundAuthorizationTarget::Subject {}
                    );
                    self.operations
                        .lock()
                        .unwrap()
                        .push(input.operation.clone());
                    let allowed = self.allowed.lock().unwrap().contains(&input.operation);
                    return Ok(WorkspaceResponse {
                        status: if allowed { 200 } else { 403 },
                        body: if allowed {
                            serde_json::json!({"scope": {"kind":"subject", "workspace_id":"workspace-test", "subject_id":"subject-fixture"}}).to_string()
                        } else {
                            serde_json::json!({"message":"current Journal operation denied"})
                                .to_string()
                        },
                    });
                }
                let startup = request.path.ends_with("/subjektiv/journal/startup");
                assert!(
                    startup || request.path.ends_with("/subjektiv/journal/compaction"),
                    "{}",
                    request.path
                );
                let snapshot = subjektiv::journal::startup::JournalStartupSnapshot {
                    subject_id: "subject-fixture".into(),
                    journals: vec![],
                    // Omitted startup evidence is not an unprotected empty snapshot.
                    omitted: !startup || self.startup_protected,
                };
                Ok(WorkspaceResponse {
                    status: 200,
                    body: serde_json::to_string(&snapshot).unwrap(),
                })
            }
        }
        fn install_journal<C: LlmClient + 'static>(
            worker: &mut Worker<C, TestStore>,
            client: Arc<JournalHostClient>,
        ) {
            let mut connection = crate::subjektiv::test_connection(client);
            connection.context.session_id = worker.session_id().to_string();
            worker.install_session_context_source(Arc::new(
                crate::feature::builtin::journal::JournalContextSource::new(
                    connection,
                    worker.prompts(),
                ),
            ));
        }
        async fn journal_worker(
            store: TestStore,
            client: Arc<JournalHostClient>,
        ) -> Worker<Provider, TestStore> {
            let mut manifest = minimal_manifest();
            manifest.scope.allow.clear();
            manifest.worker.name = format!("journal-provenance-{}", uuid::Uuid::now_v7());
            let mut worker = Worker::new(
                manifest,
                Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(Provider::default()),
                store,
                WorkerWorkspaceContext::no_workspace(),
                WorkerFilesystemAuthority::None,
                Scope::empty(),
            )
            .await
            .unwrap();
            worker.engine_mut().set_system_prompt("local instructions");
            install_journal(&mut worker, client);
            worker.materialize_durable_session_head().await.unwrap();
            worker
        }
        fn journal_kinds(
            store: &TestStore,
            session: SessionId,
            segment: SegmentId,
        ) -> serde_json::Value {
            store
                .read_all(session, segment)
                .unwrap()
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    LogEntry::Extension {
                        domain, payload, ..
                    } if domain == PROTECTED_INPUTS_DOMAIN => Some(payload["kinds"].clone()),
                    _ => None,
                })
                .expect("retention checkpoint")
        }

        // Real JournalContextSource -> BackendSubjektivHost -> bound HTTP seam,
        // real Session store and core request interception. Only HTTP responses
        // and the provider are fixtures, not the retained-supply authorizer.
        #[tokio::test]
        async fn journal_startup_subtype_restore_requires_only_startup_supply_and_rejects_removal()
        {
            let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let temp = tempfile::tempdir().unwrap();
            let store = store(&temp);
            let client = Arc::new(JournalHostClient::new(true));
            let original = journal_worker(store.clone(), client.clone()).await;
            let manifest = original.manifest().clone();
            let session = original.session_id();
            let segment = original.segment_id();
            assert_eq!(
                journal_kinds(&store, session, segment),
                serde_json::json!(["journal-startup"])
            );
            drop(original);
            for mode in ["matching", "removed", "revoked"] {
                client.supply_only(if mode == "revoked" {
                    &[]
                } else {
                    &["journal.startup.supply"]
                });
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let mut restored =
                    restore(manifest.clone(), store.clone(), session, segment, provider).await;
                if mode != "removed" {
                    install_journal(&mut restored, client.clone());
                }
                let outcome = restored.run_text("supply cached startup Journal").await;
                assert_supply_outcome(outcome, mode == "matching");
                assert_eq!(
                    calls.load(Ordering::SeqCst),
                    usize::from(mode == "matching")
                );
                let operations = client.operations.lock().unwrap();
                assert!(
                    !operations
                        .iter()
                        .any(|op| op == "journal.compaction.supply")
                );
                if mode != "removed" {
                    assert!(operations.iter().any(|op| op == "journal.startup.supply"));
                }
            }
        }

        #[tokio::test]
        async fn journal_subtypes_survive_fork_and_compacted_summary_without_overgranting() {
            for startup_protected in [false, true] {
                let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let temp = tempfile::tempdir().unwrap();
                let store = store(&temp);
                let client = Arc::new(JournalHostClient::new(startup_protected));
                let mut original = journal_worker(store.clone(), client.clone()).await;
                let manifest = original.manifest().clone();
                let session = original.session_id();
                let first = original.segment_id();
                store
                    .append(
                        session,
                        first,
                        &LogEntry::Extension {
                            ts: 1,
                            domain: "test.fork".into(),
                            payload: serde_json::json!({}),
                        },
                    )
                    .unwrap();
                original.run_text("fork Journal input").await.unwrap();
                let fork = original.segment_id();
                assert_ne!(fork, first);
                assert_eq!(
                    journal_kinds(&store, session, fork),
                    if startup_protected {
                        serde_json::json!(["journal-startup"])
                    } else {
                        serde_json::json!([])
                    }
                );
                let compact = original.compact(0).await.unwrap();
                assert_eq!(
                    journal_kinds(&store, session, compact),
                    if startup_protected {
                        serde_json::json!(["journal-startup", "journal-compaction"])
                    } else {
                        serde_json::json!(["journal-compaction"])
                    }
                );
                drop(original);
                for revoke_compaction in [false, true] {
                    let mut allowed = vec![];
                    if startup_protected {
                        allowed.push("journal.startup.supply");
                    }
                    if !revoke_compaction {
                        allowed.push("journal.compaction.supply");
                    }
                    client.supply_only(&allowed);
                    let provider = Provider::default();
                    let calls = provider.calls.clone();
                    let mut restored =
                        restore(manifest.clone(), store.clone(), session, compact, provider).await;
                    install_journal(&mut restored, client.clone());
                    let outcome = restored.run_text("supply compacted Journal").await;
                    assert_supply_outcome(outcome, !revoke_compaction);
                    assert_eq!(
                        calls.load(Ordering::SeqCst),
                        usize::from(!revoke_compaction)
                    );
                    let ops = client.operations.lock().unwrap();
                    assert!(ops.iter().any(|op| op == "journal.compaction.supply"));
                    assert_eq!(
                        ops.iter().any(|op| op == "journal.startup.supply"),
                        startup_protected
                    );
                }
            }
        }

        struct JournalPayloadSource(Vec<session_store::LoggedSystemHistoryEntry>);
        #[async_trait]
        impl SessionContextSource for JournalPayloadSource {
            fn protected_input_kind(&self) -> Option<&'static str> {
                Some("journal")
            }
            async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
                Ok(self.0.clone())
            }
        }
        fn journal_payload_item(
            payload: session_store::FeatureInputPayload,
        ) -> session_store::LoggedSystemHistoryEntry {
            let mut entry = startup_test_input();
            entry.metadata.origin = WorkerHistoryProvenance::Provided {
                producer: "subjektiv.journal".into(),
            };
            entry.metadata.input_metadata = Some(
                session_store::HistoryInputMetadata::new(
                    session_store::HistoryInputUsePolicy {
                        recording_material: session_store::InputEvidenceUse::Exclude,
                        memory_evidence: session_store::InputEvidenceUse::Exclude,
                        observation: session_store::InputObservationUse::Hidden,
                        compaction: session_store::InputCompactionUse::ReferenceInput,
                    },
                    Some(payload),
                    None,
                )
                .unwrap(),
            );
            entry
        }
        fn compaction_payload(status: &str) -> session_store::FeatureInputPayload {
            session_store::FeatureInputPayload::encode("journal:compaction", 1, &serde_json::json!({
                "status": status, "snapshot": null, "snapshot_sha256": null,
                "prompt_provenance": {"source_digest":"source", "projection_digest":"projection", "logical_name":"internal.subjektiv_journal_compaction"}
            })).unwrap()
        }
        #[tokio::test]
        async fn nonprotected_empty_journal_payloads_do_not_require_source_on_restore() {
            for status in ["disabled", "absent", "unavailable"] {
                let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let temp = tempfile::tempdir().unwrap();
                let store = store(&temp);
                let mut original =
                    journal_worker(store.clone(), Arc::new(JournalHostClient::new(false))).await;
                // Start a fresh head with the payload fixture as the only input source.
                let manifest = original.manifest().clone();
                drop(original);
                let provider = Provider::default();
                original = Worker::new(
                    manifest.clone(),
                    Engine::<_, Mutable, SessionHistoryMetadata>::new_annotated(provider),
                    store.clone(),
                    WorkerWorkspaceContext::no_workspace(),
                    WorkerFilesystemAuthority::None,
                    Scope::empty(),
                )
                .await
                .unwrap();
                original.engine_mut().set_system_prompt("local");
                original.install_session_context_source(Arc::new(JournalPayloadSource(vec![
                    journal_payload_item(compaction_payload(status)),
                ])));
                original.materialize_durable_session_head().await.unwrap();
                let session = original.session_id();
                let segment = original.segment_id();
                assert_eq!(
                    journal_kinds(&store, session, segment),
                    serde_json::json!([])
                );
                drop(original);
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let mut restored = restore(manifest, store, session, segment, provider).await;
                restored
                    .run_text("local empty Journal status")
                    .await
                    .unwrap();
                assert_eq!(calls.load(Ordering::SeqCst), 1);
            }
        }

        #[tokio::test]
        async fn legacy_or_malformed_journal_payload_requires_both_supply_operations() {
            for malformed in [false, true] {
                let _runtime_environment = worker_allocation::test_util::ENV_LOCK
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let temp = tempfile::tempdir().unwrap();
                let store = store(&temp);
                let client = Arc::new(JournalHostClient::new(false));
                let original = journal_worker(store.clone(), client.clone()).await;
                let manifest = original.manifest().clone();
                let session = original.session_id();
                let segment = original.segment_id();
                drop(original);
                let entry = if malformed {
                    journal_payload_item(
                        session_store::FeatureInputPayload::encode(
                            "journal:compaction",
                            1,
                            &serde_json::json!({"status":"disabled"}),
                        )
                        .unwrap(),
                    )
                } else {
                    let mut entry = startup_test_input();
                    entry.metadata.origin = WorkerHistoryProvenance::JournalContext;
                    // Legacy records have no subtype payload. Retaining the
                    // startup fixture's payload would correctly narrow it.
                    entry.metadata.input_metadata = Some(
                        session_store::HistoryInputMetadata::new(
                            entry.metadata.input_metadata.as_ref().unwrap().usage(),
                            None,
                            None,
                        )
                        .unwrap(),
                    );
                    entry
                };
                store
                    .append(
                        session,
                        segment,
                        &LogEntry::AnnotatedSystemItem {
                            ts: 1,
                            entry,
                            extensions: vec![],
                        },
                    )
                    .unwrap();
                client.supply_only(&["journal.startup.supply"]);
                let provider = Provider::default();
                let calls = provider.calls.clone();
                let mut restored = restore(manifest, store, session, segment, provider).await;
                install_journal(&mut restored, client.clone());
                assert_supply_outcome(
                    restored
                        .run_text("legacy source needs both authorities")
                        .await,
                    false,
                );
                assert_eq!(calls.load(Ordering::SeqCst), 0);
                assert_eq!(
                    *client.operations.lock().unwrap(),
                    vec!["journal.startup.supply", "journal.compaction.supply"]
                );
            }
        }
    }
}
