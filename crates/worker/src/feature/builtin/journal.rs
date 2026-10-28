//! Journal admission and attenuated sequential Job tools, independent of Memory extraction.
//!
//! Hooks request Host reconciliation, never infer source ranges from in-memory history.
//! Hosts retain exact operation inputs/receipts and validate live Job authority on every
//! operation. Successful operations are never rolled back by Job finalization.
//!
//! Compaction freezes a read-only Host snapshot independently of admission/Job progress.
//! Its rendered reference is shared by mandatory compactor input and the replacement seed;
//! no restore/turn hook refreshes it. Previous references are only historical compactor
//! material so fallback can recover information intentionally omitted from an earlier
//! complementary summary, without accumulating stale references in the next segment.

use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    BeforeSessionRewrite, BeforeSessionRewriteAction, BeforeSessionRewriteContext, Hook, HookError,
    HookErrorCategory, HookExecutionPolicy, HookPostToolAction, PostToolCall, RunCommitted,
    RunCommittedContext, SessionRewriteKind, ToolResultSummary, WorkerRestored,
    WorkerRestoredContext,
};
use crate::subjektiv::{SubjektivHostConnection, SubjektivHostError};
use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use subjektiv::journal::transport::*;
use subjektiv::journal::{
    JournalAction, JournalOperation, MAX_JOURNAL_PAGE, MAX_JOURNAL_READ_BYTES,
    MAX_JOURNAL_RESPONSE_BYTES,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JournalStartupInput {
    pub snapshot: subjektiv::journal::startup::JournalStartupSnapshot,
    pub snapshot_sha256: String,
    pub prompt_provenance: session_store::PromptRenderProvenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JournalReadStatus {
    Available,
    Absent,
    Disabled,
    Unavailable,
}
impl JournalReadStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Absent => "absent",
            Self::Disabled => "disabled",
            Self::Unavailable => "unavailable",
        }
    }
}

// Only bounded classifications cross into diagnostics. Host messages/codes and
// io::Error payloads can contain Journal text, credentials, proofs or host paths.
// These diagnostics belong to the Feature, not persisted/model-visible input.
enum JournalCompactionFailure {
    Connection(std::io::ErrorKind),
    SnapshotPermissionDenied,
    SnapshotConflict,
    SnapshotInvalidArgument,
    SnapshotUnavailable,
    Serialization,
    ByteBudget,
}

impl JournalCompactionFailure {
    fn snapshot(error: SubjektivHostError) -> Self {
        match error {
            SubjektivHostError::PermissionDenied(_) => Self::SnapshotPermissionDenied,
            SubjektivHostError::Conflict { .. } => Self::SnapshotConflict,
            SubjektivHostError::InvalidArgument(_) => Self::SnapshotInvalidArgument,
            SubjektivHostError::Unavailable(_) => Self::SnapshotUnavailable,
        }
    }

    fn stage(&self) -> &'static str {
        match self {
            Self::Connection(_) => "connection",
            Self::SnapshotPermissionDenied
            | Self::SnapshotConflict
            | Self::SnapshotInvalidArgument
            | Self::SnapshotUnavailable => "snapshot",
            Self::Serialization => "serialization",
            Self::ByteBudget => "byte_budget",
        }
    }

    fn reason(&self) -> &'static str {
        match self {
            Self::Connection(std::io::ErrorKind::PermissionDenied) => "permission_denied",
            Self::Connection(std::io::ErrorKind::InvalidInput) => "invalid_input",
            Self::Connection(std::io::ErrorKind::NotConnected) => "not_connected",
            Self::Connection(_) => "io_error",
            Self::SnapshotPermissionDenied => "permission_denied",
            Self::SnapshotConflict => "host_conflict",
            Self::SnapshotInvalidArgument => "host_invalid_argument",
            Self::SnapshotUnavailable => "host_unavailable",
            Self::Serialization => "json_serialization_failed",
            Self::ByteBudget => "startup_max_bytes_exceeded",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalCompactionInput {
    status: JournalReadStatus,
    snapshot: Option<subjektiv::journal::startup::JournalStartupSnapshot>,
    snapshot_sha256: Option<String>,
    prompt_provenance: session_store::PromptRenderProvenance,
}

fn journal_input(
    body: String,
    payload: session_store::FeatureInputPayload,
    kind: &str,
) -> Result<session_store::SystemInputEnvelope, String> {
    use session_store::*;
    let public = PublicSystemItem {
        kind: kind.into(),
        body: body.clone(),
        presentation: PublicSystemPresentation::Context,
    };
    let metadata = HistoryInputMetadata::new(
        HistoryInputUsePolicy {
            recording_material: InputEvidenceUse::Exclude,
            memory_evidence: InputEvidenceUse::Exclude,
            observation: InputObservationUse::Hidden,
            compaction: InputCompactionUse::ReferenceInput,
        },
        Some(payload),
        Some(public),
    )
    .map_err(|e| e.to_string())?;
    SystemInputEnvelope::new(body, metadata).map_err(|e| e.to_string())
}

/// Subjektiv owns startup projection; Core only commits its typed initial items.
pub(crate) struct JournalContextSource {
    host: SubjektivHostConnection,
    prompts: Arc<arc_swap::ArcSwap<crate::prompt::catalog::PromptCatalog>>,
    startup_supplied: AtomicBool,
    compaction_supplied: AtomicBool,
}
impl JournalContextSource {
    pub(crate) fn new(
        host: SubjektivHostConnection,
        prompts: Arc<arc_swap::ArcSwap<crate::prompt::catalog::PromptCatalog>>,
    ) -> Self {
        Self {
            host,
            prompts,
            startup_supplied: AtomicBool::new(false),
            compaction_supplied: AtomicBool::new(false),
        }
    }
}
#[async_trait]
impl crate::worker::SessionContextSource for JournalContextSource {
    fn protected_input_kind(&self) -> Option<&'static str> {
        Some("journal")
    }

    async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
        use sha2::{Digest, Sha256};
        if !self.host.host.journal_startup_granted() {
            return Ok(Vec::new());
        }
        self.host
            .authorize_operation("journal.startup.read")
            .map_err(|e| e.to_string())?;
        let snapshot = self
            .host
            .journal_startup_context()
            .map_err(|e| e.to_string())?;
        // A successful empty read contributes no model context. Keep an omitted
        // snapshot even when empty: truncation is not evidence of absence.
        let empty = snapshot.journals.is_empty() && !snapshot.omitted;
        // Hash the same canonical JSON value retained by SystemItem, so replay
        // can verify provenance without depending on domain struct field order.
        let snapshot_value = serde_json::to_value(&snapshot).map_err(|e| e.to_string())?;
        let snapshot_json = serde_json::to_string(&snapshot_value).map_err(|e| e.to_string())?;
        if snapshot_json.len() > subjektiv::journal::startup::STARTUP_MAX_BYTES {
            return Err("Host Journal startup snapshot exceeds independent byte budget".into());
        }
        if empty {
            return Ok(Vec::new());
        }
        let prompts = self.prompts.load();
        let body = prompts
            .subjektiv_journal_startup(&snapshot_json)
            .map_err(|e| e.to_string())?;
        if body.len() > subjektiv::journal::startup::STARTUP_MAX_BYTES + 4096 {
            return Err("Journal startup prompt exceeds rendered byte budget".into());
        }
        let projection = prompts.projection();
        let payload = session_store::FeatureInputPayload::encode(
            "journal:startup",
            1,
            &JournalStartupInput {
                snapshot,
                snapshot_sha256: Sha256::digest(snapshot_json.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                prompt_provenance: session_store::PromptRenderProvenance {
                    workspace_id: self.host.host.workspace_id(),
                    source_digest: projection.source_digest.clone(),
                    projection_digest: projection.catalog_digest.clone(),
                    logical_name: "internal.subjektiv_journal_startup".into(),
                },
            },
        )
        .map_err(|e| e.to_string())?;
        self.host
            .authorize_operation("journal.startup.supply")
            .map_err(|e| e.to_string())?;
        let input = journal_input(body, payload, "journal_startup")?;
        self.startup_supplied.store(true, Ordering::Release);
        Ok(vec![session_store::LoggedSystemHistoryEntry::from_input(
            session_store::LoggedSessionHistoryOrigin::Provided {
                producer: "subjektiv.journal".into(),
            },
            None,
            input,
        )])
    }

    async fn authorize_supply(&self) -> Result<(), String> {
        if self.startup_supplied.load(Ordering::Acquire) {
            self.host
                .authorize_operation("journal.startup.supply")
                .map_err(|error| error.to_string())?;
        }
        if self.compaction_supplied.load(Ordering::Acquire) {
            self.host
                .authorize_operation("journal.compaction.supply")
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn authorize_retained_supply(&self, kind: &str) -> Result<(), String> {
        let operations: &[&str] = match kind {
            "journal-startup" => &["journal.startup.supply"],
            "journal-compaction" => &["journal.compaction.supply"],
            // Legacy Journal context cannot distinguish its original source.
            // It may retain both kinds; it never obtains permission from flags
            // reset by restore or from today's Feature configuration.
            "journal" => &["journal.startup.supply", "journal.compaction.supply"],
            _ => return Err("unknown retained Journal input kind".into()),
        };
        for operation in operations {
            self.host
                .authorize_operation(operation)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn load_compaction_context(
        &self,
        session: session_store::SessionId,
    ) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
        // A changed Session must never reuse the old source's Host ownership.
        if self.host.context.session_id != session.to_string() {
            return Err("Journal context source belongs to a different Session".into());
        }
        if !self.host.host.journal_compaction_granted() {
            return Ok(Vec::new());
        }
        self.host
            .authorize_operation("journal.compaction.read")
            .map_err(|error| error.to_string())?;
        self.host
            .authorize_operation("journal.compaction.supply")
            .map_err(|error| error.to_string())?;
        let input =
            freeze_compaction_journal(Ok(Some(self.host.clone())), &self.prompts.load_full())?;
        self.host
            .authorize_operation("journal.compaction.supply")
            .map_err(|error| error.to_string())?;
        self.compaction_supplied.store(true, Ordering::Release);
        Ok(vec![session_store::LoggedSystemHistoryEntry::from_input(
            session_store::LoggedSessionHistoryOrigin::Provided {
                producer: "subjektiv.journal".into(),
            },
            None,
            input,
        )])
    }
}

/// Read once without waiting for recording. The returned body is immutable mandatory
/// compactor input and the exact replacement-segment reference, including fallback status.
/// Selection and authorization belong to the Host, not the core compaction engine.
pub(crate) fn freeze_compaction_journal(
    connection: std::io::Result<Option<SubjektivHostConnection>>,
    prompts: &crate::prompt::catalog::PromptCatalog,
) -> Result<session_store::SystemInputEnvelope, String> {
    use JournalReadStatus::*;
    let workspace_id = connection.as_ref().ok().and_then(|connection| {
        connection
            .as_ref()
            .and_then(|connection| connection.host.workspace_id())
    });
    let read = match connection {
        Ok(Some(connection)) if connection.host.journal_compaction_granted() => Some(
            connection
                .authorize_operation("journal.compaction.read")
                .and_then(|_| connection.journal_compaction_context())
                .and_then(|snapshot| {
                    connection.authorize_operation("journal.compaction.supply")?;
                    Ok(snapshot)
                })
                .map_err(JournalCompactionFailure::snapshot),
        ),
        Ok(_) => None,
        Err(error) => Some(Err(JournalCompactionFailure::Connection(error.kind()))),
    };
    let snapshot = read.as_ref().and_then(|read| read.as_ref().ok()).cloned();
    let prepared = match read {
        None => Ok((Disabled, String::new())),
        Some(Ok(snapshot)) => {
            let status = if snapshot.journals.is_empty() {
                Absent
            } else {
                Available
            };
            // The current snapshot has only derived, JSON-safe fields (strings,
            // integers, vectors, enums and a UUID). There is no known serialization
            // failure fixture; retain this guard if the domain type changes.
            serde_json::to_string(&snapshot)
                .map_err(|_| JournalCompactionFailure::Serialization)
                .and_then(|json| {
                    if json.len() <= subjektiv::journal::startup::STARTUP_MAX_BYTES {
                        Ok((status, json))
                    } else {
                        Err(JournalCompactionFailure::ByteBudget)
                    }
                })
        }
        Some(Err(failure)) => Err(failure),
    };
    let (status, json) = prepared.unwrap_or_else(|failure| {
        tracing::warn!(
            stage = failure.stage(),
            reason = failure.reason(),
            "Journal compaction snapshot unavailable; continuing without Journal"
        );
        (Unavailable, String::new())
    });
    use sha2::{Digest, Sha256};
    let snapshot_sha256: Option<String> = (!json.is_empty()).then(|| {
        Sha256::digest(json.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    });
    let body = prompts
        .subjektiv_journal_compaction(
            status.as_str(),
            &json,
            snapshot_sha256.as_deref(),
            workspace_id.as_deref(),
        )
        .map_err(|e| e.to_string())?;
    let payload = session_store::FeatureInputPayload::encode(
        "journal:compaction",
        1,
        &JournalCompactionInput {
            status,
            prompt_provenance: session_store::PromptRenderProvenance {
                workspace_id,
                source_digest: prompts.projection().source_digest.clone(),
                projection_digest: prompts.projection().catalog_digest.clone(),
                logical_name: "internal.subjektiv_journal_compaction".into(),
            },
            snapshot: if status == Unavailable {
                None
            } else {
                snapshot
            },
            snapshot_sha256,
        },
    )
    .map_err(|e| e.to_string())?;
    journal_input(body, payload, "journal_compaction")
}

#[derive(Clone)]
pub struct JournalRecordingFeature {
    host: SubjektivHostConnection,
}
impl JournalRecordingFeature {
    pub fn new(host: SubjektivHostConnection) -> std::io::Result<Self> {
        if !host.host.journal_recording_granted() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Journal recording requires an explicit Host grant",
            ));
        }
        Ok(Self { host })
    }
    fn request(&self, reason: JournalRecordingReason) -> Result<(), HookError> {
        self.host
            .authorize_operation("journal.recording.request")
            .and_then(|_| {
                self.host
                    .host
                    .request_journal_recording(&self.host.context, reason)
            })
            .map(|_| ())
            .map_err(|error| {
                let diagnostic = match error {
                    SubjektivHostError::PermissionDenied(_) => {
                        "Journal journal.recording.request permission denied; admission not started"
                    }
                    _ => "Journal admission unconfirmed; Host must reconcile saved public history",
                };
                HookError::new(HookErrorCategory::Dependency, diagnostic)
            })
    }
}
impl FeatureModule for JournalRecordingFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("journal-recording", "Journal recording admission")
            .with_hook(HookDeclaration::new(
                "record-task",
                FeatureHookPoint::PostToolCall,
            ))
            .with_hook(HookDeclaration::new(
                "record-run",
                FeatureHookPoint::RunCommitted,
            ))
            .with_hook(HookDeclaration::new(
                "record-compact",
                FeatureHookPoint::BeforeSessionRewrite,
            ))
            .with_hook(HookDeclaration::new(
                "record-restore",
                FeatureHookPoint::WorkerRestored,
            ))
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.hooks().add_post_tool_call(
            "record-task",
            HookExecutionPolicy::new(crate::hook::HookFailurePolicy::AttentionRequired, 30_000),
            self.clone(),
        )?;
        context.hooks().add_run_committed(
            "record-run",
            HookExecutionPolicy::new(crate::hook::HookFailurePolicy::AttentionRequired, 30_000),
            self.clone(),
        )?;
        context.hooks().add_before_session_rewrite(
            "record-compact",
            HookExecutionPolicy::fail_closed(),
            self.clone(),
        )?;
        context.hooks().add_worker_restored(
            "record-restore",
            HookExecutionPolicy::new(crate::hook::HookFailurePolicy::AttentionRequired, 30_000),
            self.clone(),
        )?;
        Ok(())
    }
}
#[async_trait]
impl Hook<PostToolCall> for JournalRecordingFeature {
    async fn call(&self, result: &ToolResultSummary) -> Result<HookPostToolAction, HookError> {
        if result.tool_name == "TaskUpdate"
            && !result.is_error
            && result.arguments.get("status").and_then(Value::as_str) == Some("completed")
        {
            self.request(JournalRecordingReason::TaskCompleted)?;
        }
        Ok(HookPostToolAction::Continue)
    }
}
#[async_trait]
impl Hook<RunCommitted> for JournalRecordingFeature {
    async fn call(&self, _: &RunCommittedContext) -> Result<(), HookError> {
        self.request(JournalRecordingReason::RunCommitted)
    }
}
#[async_trait]
impl Hook<BeforeSessionRewrite> for JournalRecordingFeature {
    async fn call(
        &self,
        context: &BeforeSessionRewriteContext,
    ) -> Result<BeforeSessionRewriteAction, HookError> {
        if context.kind == SessionRewriteKind::Compact {
            // Only admission/retention is awaited, not Journal generation.
            self.request(JournalRecordingReason::BeforeCompact)?;
        }
        Ok(BeforeSessionRewriteAction::Continue)
    }
}
#[async_trait]
impl Hook<WorkerRestored> for JournalRecordingFeature {
    async fn call(&self, _: &WorkerRestoredContext) -> Result<(), HookError> {
        self.request(JournalRecordingReason::WorkerRestored)
    }
}

/// A distinct Host capability, bound to one Journal/generation and frozen public input.
/// Profile names and main-Worker recording grants cannot construct this authority.
/// Implementations must preserve exact request_id/full input and reject stale leases.
pub trait JournalJobHost: Send + Sync + 'static {
    fn execute(&self, request: JournalJobRequest)
    -> Result<JournalJobResponse, SubjektivHostError>;
    /// Verify saved checkpoints and remaining input; model prose is not completion evidence.
    fn finish(&self) -> Result<JournalJobCompletion, SubjektivHostError>;
}

#[derive(Clone)]
pub struct JournalJobToolsFeature {
    host: Arc<dyn JournalJobHost>,
    serial: Arc<tokio::sync::Mutex<()>>,
}
impl JournalJobToolsFeature {
    pub fn new(host: Arc<dyn JournalJobHost>) -> Self {
        Self {
            host,
            serial: Arc::default(),
        }
    }
}
const TOOLS: &[(&str, &str)] = &[
    (
        "CreateJournalEntry",
        "Save a new open or fixed event. Existing open must first be fixed; fixed needs source_end. Retain exact request_id/full input on retry; carry returned state_token into the next new operation.",
    ),
    (
        "RewriteJournalEntry",
        "Rewrite only an open entry's title/body/ongoing. Never rewrite fixed history. Retain exact request_id/full input on retry.",
    ),
    (
        "FixJournalEntry",
        "Fix an open event with an allowed original source endpoint. Task/Job/turn/compaction completion is NOT evidence that an event ended.",
    ),
    (
        "UpdateJournalPosition",
        "Save a short current-position memo. This alone is never evidence for checkpointing source bytes.",
    ),
    (
        "ConfirmJournalUnchanged",
        "Save an explicit checked no-entry-change conclusion for actually read source bytes. This receipt can support a checkpoint.",
    ),
    (
        "CheckpointJournal",
        "Advance only the contiguous actually reflected source prefix backed by saved record/no-change request IDs. Never infer coverage from final prose or position updates.",
    ),
    (
        "ReadJournalState",
        "Read bounded initial/current Journal metadata and immutable selected_ranges descriptors, not source bodies or proof of delivery. Fetch every selected Source range with ReadJournalText before any record, position, no-change or checkpoint mutation.",
    ),
    (
        "QueryJournalOperation",
        "Reconcile a saved request_id after unknown outcome before any new create. Absence is not evidence of an execution failure.",
    ),
    (
        "ReadJournalOperationCoverage",
        "Read a bounded page of exact saved source ranges for a request_id. Reconcile old receipts against selected bytes before reusing them in a replacement Job checkpoint; a metadata count alone is not coverage evidence.",
    ),
    (
        "ListJournalOperations",
        "Page saved operation receipts for recovery without replaying record creation.",
    ),
    (
        "ReadJournalText",
        "Read a bounded UTF-8 chunk of Host-bound initial/current position/open text or source text addressed by a frozen Source descriptor. Job input contains no raw source bodies: fetch every selected Source byte range, continuing with next_byte and content_token, before any mutation. Only successful Source delivery counts; failed fetches and previews do not. Do not mix later input into this generation.",
    ),
];
impl FeatureModule for JournalJobToolsFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        TOOLS.iter().fold(
            FeatureDescriptor::builtin("journal-job-tools", "Journal Job tools")
                .with_description("Descriptor-only Journal Jobs fetch immutable selected Source bytes through bounded tools before sequential mutations; selection metadata is not delivery evidence."),
            |descriptor, (name, description)| {
                descriptor.with_tool(ToolDeclaration::new(*name, *description))
            },
        )
    }
    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for &(name, description) in TOOLS {
            let state = self.clone();
            let definition: ToolDefinition = Arc::new(move || {
                (
                    ToolMeta::new(name)
                        .description(description)
                        .input_schema(tool_schema(name)),
                    Arc::new(JournalTool {
                        name,
                        state: state.clone(),
                    }) as Arc<dyn Tool>,
                )
            });
            context
                .tools()
                .register(ToolContribution::new(name, definition))?;
        }
        Ok(())
    }
}
struct JournalTool {
    name: &'static str,
    state: JournalJobToolsFeature,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Query {
    request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    request_id: String,
    offset: usize,
    limit: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    after: Option<String>,
    limit: usize,
}
fn parse<T: serde::de::DeserializeOwned>(input: &str) -> Result<T, ToolError> {
    serde_json::from_str(input).map_err(|error| ToolError::InvalidArgument(error.to_string()))
}
#[async_trait]
impl Tool for JournalTool {
    async fn execute(&self, input: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        let request = match self.name {
            "ReadJournalState" => {
                let _: Empty = parse(input)?;
                JournalJobRequest::ReadState
            }
            "QueryJournalOperation" => {
                let query: Query = parse(input)?;
                JournalJobRequest::QueryOperation {
                    request_id: query.request_id,
                }
            }
            "ReadJournalOperationCoverage" => {
                let coverage: Coverage = parse(input)?;
                if !(1..=MAX_JOURNAL_PAGE).contains(&coverage.limit) {
                    return Err(ToolError::InvalidArgument(
                        "limit outside Journal page budget".into(),
                    ));
                }
                JournalJobRequest::ReadOperationCoverage {
                    request_id: coverage.request_id,
                    offset: coverage.offset,
                    limit: coverage.limit,
                }
            }
            "ListJournalOperations" => {
                let list: List = parse(input)?;
                if !(1..=MAX_JOURNAL_PAGE).contains(&list.limit) {
                    return Err(ToolError::InvalidArgument(
                        "limit outside Journal page budget".into(),
                    ));
                }
                JournalJobRequest::ListOperations {
                    after: list.after,
                    limit: list.limit,
                }
            }
            "ReadJournalText" => {
                let read: JournalTextRead = parse(input)?;
                // A selected Source range may begin at a nonzero byte. Only the
                // bound Host knows that first-chunk offset and enforces its token
                // condition; position/open reads always start at zero.
                if !(1..=MAX_JOURNAL_READ_BYTES).contains(&read.limit)
                    || (read.offset > 0
                        && read.content_token.is_none()
                        && !matches!(&read.target, JournalTextTarget::Source { .. }))
                {
                    return Err(ToolError::InvalidArgument(
                        "invalid bounded text read or missing continuation content_token".into(),
                    ));
                }
                JournalJobRequest::ReadText { read }
            }
            _ => {
                let operation: JournalOperation = parse(input)?;
                let correct = matches!(
                    (self.name, &operation.action),
                    ("CreateJournalEntry", JournalAction::Create { .. })
                        | ("RewriteJournalEntry", JournalAction::Rewrite { .. })
                        | ("FixJournalEntry", JournalAction::Fix { .. })
                        | (
                            "UpdateJournalPosition",
                            JournalAction::UpdatePosition { .. }
                        )
                        | ("ConfirmJournalUnchanged", JournalAction::ConfirmUnchanged)
                        | ("CheckpointJournal", JournalAction::Checkpoint { .. })
                );
                if !correct {
                    return Err(ToolError::InvalidArgument(
                        "action does not match Journal tool".into(),
                    ));
                }
                JournalJobRequest::Execute { operation }
            }
        };
        let _serial = self.state.serial.lock().await;
        let response = self.state.host.execute(request)?;
        let content = serde_json::to_string(&response)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if content.len() > MAX_JOURNAL_RESPONSE_BYTES {
            return Err(ToolError::ExecutionFailed(
                "Journal response exceeds bounded output; reconcile saved operation before retry"
                    .into(),
            ));
        }
        Ok(ToolOutput {
            summary: format!("{}: Host response", self.name),
            content: Some(content),
            attachments: vec![],
        })
    }
}

/// Checks Host-saved state at the existing Job result boundary, not a whole-Job write.
pub struct JournalJobFinalizer {
    host: Arc<dyn JournalJobHost>,
}
impl JournalJobFinalizer {
    pub fn new(host: Arc<dyn JournalJobHost>) -> Self {
        Self { host }
    }
}
#[async_trait]
impl crate::job::JobResultFinalizer for JournalJobFinalizer {
    fn validate_profile(&self, manifest: &manifest::WorkerManifest) -> Result<(), String> {
        // Host-selected source bytes must reach the model intact, not be silently
        // truncated by the generic Engine output limiter after domain delivery.
        if TOOLS.iter().any(|(name, _)| {
            manifest.engine.tool_output.limit_for(name) < MAX_JOURNAL_RESPONSE_BYTES
        }) {
            return Err(
                "Journal tool output limits must preserve the complete bounded Host response"
                    .into(),
            );
        }
        Ok(())
    }
    async fn finalize(
        &self,
        result: Value,
        _: manifest::WorkerManifest,
        _: Box<dyn agen::llm_client::LlmClient>,
        cancellation: crate::feature::background::BackgroundTaskCancellation,
    ) -> Result<Value, ToolError> {
        if cancellation.is_cancelled() {
            return Err(ToolError::ExecutionFailed("Journal Job cancelled".into()));
        }
        if result != json!({}) {
            return Err(ToolError::InvalidArgument(
                "Submit empty result {}; Journal changes must already be saved by sequential tools"
                    .into(),
            ));
        }
        let completion = self.host.finish()?;
        serde_json::to_value(completion)
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type":"object", "properties":properties, "required":required, "additionalProperties":false})
}
fn tool_schema(name: &str) -> Value {
    let string = json!({"type":"string"});
    let pointer = object(
        json!({"segment_id":{"type":"string","format":"uuid"},"entry_ref":string}),
        &["segment_id", "entry_ref"],
    );
    let range = object(
        json!({"source":object(json!({"session_id":string,"pointer":pointer,"order":{"type":"integer","minimum":0},"content_sha256":string,"total_bytes":{"type":"integer","minimum":1}}), &["session_id","pointer","order","content_sha256","total_bytes"]),"bytes":object(json!({"start":{"type":"integer","minimum":0},"end":{"type":"integer","minimum":1}}), &["start","end"])}),
        &["source", "bytes"],
    );
    match name {
        "ReadJournalState" => object(json!({}), &[]),
        "QueryJournalOperation" => object(json!({"request_id":string}), &["request_id"]),
        "ReadJournalOperationCoverage" => object(
            json!({"request_id":string,"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":MAX_JOURNAL_PAGE}}),
            &["request_id", "offset", "limit"],
        ),
        "ListJournalOperations" => object(
            json!({"after":{"type":["string","null"]},"limit":{"type":"integer","minimum":1,"maximum":MAX_JOURNAL_PAGE}}),
            &["limit"],
        ),
        "ReadJournalText" => {
            let snapshot = json!({"type":"string","enum":["initial","current"]});
            let targets = ["position", "open_body", "open_ongoing"].map(|target| {
                object(
                    json!({"target":{"const":target},"snapshot":snapshot}),
                    &["target", "snapshot"],
                )
            });
            object(
                json!({"target":{"oneOf":[targets[0],targets[1],targets[2],object(json!({"target":{"const":"source"},"pointer":pointer}), &["target","pointer"])]},"offset":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":MAX_JOURNAL_READ_BYTES},"content_token":{"type":["string","null"]}}),
                &["target", "offset", "limit"],
            )
        }
        _ => {
            let strings = json!({"type":"array","items":{"type":"string"}});
            let action = match name {
                "CreateJournalEntry" => object(
                    json!({"action":{"const":"create"},"title":string,"body":string,"ongoing":string,"state":{"enum":["open","fixed"]},"source_end":{"anyOf":[pointer,{"type":"null"}]},"correction_refs":strings}),
                    &[
                        "action",
                        "title",
                        "body",
                        "ongoing",
                        "state",
                        "correction_refs",
                    ],
                ),
                "RewriteJournalEntry" => object(
                    json!({"action":{"const":"rewrite"},"entry_id":string,"title":string,"body":string,"ongoing":string}),
                    &["action", "entry_id", "title", "body", "ongoing"],
                ),
                "FixJournalEntry" => object(
                    json!({"action":{"const":"fix"},"entry_id":string,"source_end":pointer}),
                    &["action", "entry_id", "source_end"],
                ),
                "UpdateJournalPosition" => object(
                    json!({"action":{"const":"update_position"},"current_position":string}),
                    &["action", "current_position"],
                ),
                "ConfirmJournalUnchanged" => {
                    object(json!({"action":{"const":"confirm_unchanged"}}), &["action"])
                }
                "CheckpointJournal" => object(
                    json!({"action":{"const":"checkpoint"},"record_operation_ids":strings}),
                    &["action", "record_operation_ids"],
                ),
                _ => unreachable!(),
            };
            object(
                json!({"request_id":string,"expected_state":string,"action":action,"covered_ranges":{"type":"array","items":range,"maxItems":64}}),
                &["request_id", "expected_state", "action", "covered_ranges"],
            )
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/common/diagnostics.rs"]
mod diagnostic_test_support;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use subjektiv::journal::JournalReceipt;

    #[test]
    fn compaction_connection_failure_keeps_safe_reason_without_exposing_error_or_changing_input() {
        let prompts = crate::prompt::catalog::PromptCatalog::builtins_only().unwrap();
        for (kind, reason) in [
            (std::io::ErrorKind::PermissionDenied, "permission_denied"),
            (std::io::ErrorKind::InvalidInput, "invalid_input"),
            (std::io::ErrorKind::NotConnected, "not_connected"),
            (std::io::ErrorKind::Other, "io_error"),
        ] {
            let logs = diagnostic_test_support::CapturedWarnings::default();
            let input = tracing::subscriber::with_default(logs.subscriber(), || {
                freeze_compaction_journal(
                    Err(std::io::Error::new(
                        kind,
                        "PRIVATE_JOURNAL_BODY /private/host/path proof=PRIVATE_PROOF credential=PRIVATE_CREDENTIAL raw_error=PRIVATE_ERROR",
                    )),
                    &prompts,
                )
                .unwrap()
            });
            let log = logs.text();
            assert!(log.contains("stage=\"connection\""), "{log}");
            assert!(log.contains(&format!("reason=\"{reason}\"")), "{log}");
            assert_eq!(log.matches("continuing without Journal").count(), 1);
            assert!(!log.contains("PRIVATE_"));
            assert!(!log.contains("/private/host/path"));
            assert!(input.body().contains("Journal read status: unavailable."));
            let persisted = serde_json::to_string(&input).unwrap();
            assert!(!persisted.contains("PRIVATE_"));
            assert!(!persisted.contains("/private/host/path"));
            assert!(!persisted.contains(reason));
            let payload: JournalCompactionInput = input
                .metadata()
                .feature_payload()
                .unwrap()
                .decode("journal:compaction", 1)
                .unwrap();
            assert!(payload.snapshot.is_none());
            assert!(payload.snapshot_sha256.is_none());
            assert_eq!(
                payload.prompt_provenance.source_digest,
                prompts.projection().source_digest
            );
            assert_eq!(
                payload.prompt_provenance.projection_digest,
                prompts.projection().catalog_digest
            );
        }
        let logs = diagnostic_test_support::CapturedWarnings::default();
        let input = tracing::subscriber::with_default(logs.subscriber(), || {
            freeze_compaction_journal(Ok(None), &prompts).unwrap()
        });
        assert!(input.body().contains("Journal read status: disabled."));
        assert!(logs.text().is_empty());
    }

    #[derive(Default)]
    struct ContextHost {
        allowed: Mutex<Vec<&'static str>>,
        reads: std::sync::atomic::AtomicUsize,
        requests: std::sync::atomic::AtomicUsize,
    }
    impl crate::subjektiv::SubjektivHost for ContextHost {
        fn settings(&self) -> crate::subjektiv::SubjektivHostSettings {
            crate::subjektiv::SubjektivHostSettings {
                language: "English".into(),
            }
        }
        fn authorize_operation(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            operation: &str,
        ) -> Result<(), SubjektivHostError> {
            if self.allowed.lock().unwrap().contains(&operation) {
                Ok(())
            } else {
                Err(SubjektivHostError::PermissionDenied(operation.into()))
            }
        }
        fn journal_startup_granted(&self) -> bool {
            true
        }
        fn journal_compaction_granted(&self) -> bool {
            true
        }
        fn journal_recording_granted(&self) -> bool {
            true
        }
        fn journal_startup_context(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
        ) -> Result<subjektiv::journal::startup::JournalStartupSnapshot, SubjektivHostError>
        {
            self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(subjektiv::journal::startup::JournalStartupSnapshot {
                subject_id: "subject".into(),
                journals: vec![],
                omitted: true,
            })
        }
        fn journal_compaction_context(
            &self,
            context: &crate::subjektiv::SubjektivHostContext,
        ) -> Result<subjektiv::journal::startup::JournalStartupSnapshot, SubjektivHostError>
        {
            self.journal_startup_context(context)
        }
        fn request_journal_recording(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: JournalRecordingReason,
        ) -> Result<JournalRecordingResponse, SubjektivHostError> {
            self.requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(JournalRecordingResponse::NoNewSources)
        }
        fn memory(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: server_api::SubjektivMemoryBackendOperation,
        ) -> Result<server_api::SubjektivMemoryBackendResponse, SubjektivHostError> {
            unreachable!()
        }
        fn session(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: server_api::SubjektivSessionBackendOperation,
        ) -> Result<server_api::SubjektivSessionBackendResponse, SubjektivHostError> {
            unreachable!()
        }
        fn stage_candidate(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: memory::backend::MemoryStageCandidateOperation,
        ) -> Result<server_api::SubjektivStageCandidateResponse, SubjektivHostError> {
            unreachable!()
        }
        fn record_session(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: bool,
        ) -> Result<server_api::SubjektivRecordSessionResponse, SubjektivHostError> {
            unreachable!()
        }
        fn request_consolidation(
            &self,
            _: &crate::subjektiv::SubjektivHostContext,
            _: memory::backend::MemoryConsolidateStagingOperation,
        ) -> Result<memory::backend::MemoryConsolidationOutput, SubjektivHostError> {
            unreachable!()
        }
    }
    fn context_connection(host: Arc<ContextHost>) -> SubjektivHostConnection {
        SubjektivHostConnection::new(
            host,
            crate::subjektiv::SubjektivHostContext {
                worker_id: "worker".into(),
                session_id: "session".into(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn startup_context_denial_never_reads_or_supplies_and_revocation_rechecks_supply() {
        use crate::worker::SessionContextSource;
        use std::sync::atomic::Ordering::SeqCst;
        let host = Arc::new(ContextHost::default());
        let source = JournalContextSource::new(
            context_connection(host.clone()),
            Arc::new(arc_swap::ArcSwap::from(
                crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
            )),
        );
        source.authorize_supply().await.unwrap();
        assert!(
            source
                .load()
                .await
                .unwrap_err()
                .contains("journal.startup.read")
        );
        assert_eq!(host.reads.load(SeqCst), 0);
        *host.allowed.lock().unwrap() = vec!["journal.startup.read", "journal.startup.supply"];
        assert_eq!(source.load().await.unwrap().len(), 1);
        source.authorize_supply().await.unwrap();
        assert_eq!(host.reads.load(SeqCst), 1);
        *host.allowed.lock().unwrap() = vec!["journal.startup.read"];
        assert!(
            source
                .authorize_supply()
                .await
                .unwrap_err()
                .contains("journal.startup.supply")
        );
        assert_eq!(host.reads.load(SeqCst), 1);
        assert!(
            source
                .load()
                .await
                .unwrap_err()
                .contains("journal.startup.supply")
        );
        assert_eq!(host.reads.load(SeqCst), 2);
    }

    #[tokio::test]
    async fn cached_compaction_supply_rechecks_without_reading_or_requiring_startup_grant() {
        use crate::worker::SessionContextSource;
        use std::sync::atomic::Ordering::SeqCst;
        let host = Arc::new(ContextHost::default());
        let session = session_store::new_session_id();
        let mut connection = context_connection(host.clone());
        connection.context.session_id = session.to_string();
        let source = JournalContextSource::new(
            connection,
            Arc::new(arc_swap::ArcSwap::from(
                crate::prompt::catalog::PromptCatalog::builtins_only().unwrap(),
            )),
        );
        assert!(source.load_compaction_context(session).await.is_err());
        assert_eq!(host.reads.load(SeqCst), 0);
        source.authorize_supply().await.unwrap();
        *host.allowed.lock().unwrap() =
            vec!["journal.compaction.read", "journal.compaction.supply"];
        assert_eq!(
            source.load_compaction_context(session).await.unwrap().len(),
            1
        );
        source.authorize_supply().await.unwrap();
        assert_eq!(host.reads.load(SeqCst), 1);
        *host.allowed.lock().unwrap() = vec!["journal.compaction.read"];
        assert!(
            source
                .authorize_supply()
                .await
                .unwrap_err()
                .contains("journal.compaction.supply")
        );
        assert_eq!(host.reads.load(SeqCst), 1);
    }

    #[test]
    fn compaction_and_recording_denial_has_zero_downstream_calls_and_revocation_is_live() {
        use std::sync::atomic::Ordering::SeqCst;
        let host = Arc::new(ContextHost::default());
        let connection = context_connection(host.clone());
        let prompts = crate::prompt::catalog::PromptCatalog::builtins_only().unwrap();
        let denied = freeze_compaction_journal(Ok(Some(connection.clone())), &prompts).unwrap();
        assert!(denied.body().contains("Journal read status: unavailable."));
        assert_eq!(host.reads.load(SeqCst), 0);
        let feature = JournalRecordingFeature::new(connection.clone()).unwrap();
        assert!(
            feature
                .request(JournalRecordingReason::RunCommitted)
                .is_err()
        );
        assert_eq!(host.requests.load(SeqCst), 0);
        *host.allowed.lock().unwrap() = vec![
            "journal.compaction.read",
            "journal.compaction.supply",
            "journal.recording.request",
        ];
        let allowed = freeze_compaction_journal(Ok(Some(connection.clone())), &prompts).unwrap();
        let payload: JournalCompactionInput = allowed
            .metadata()
            .feature_payload()
            .unwrap()
            .decode("journal:compaction", 1)
            .unwrap();
        assert!(payload.snapshot.is_some());
        feature
            .request(JournalRecordingReason::RunCommitted)
            .unwrap();
        assert_eq!(host.reads.load(SeqCst), 1);
        assert_eq!(host.requests.load(SeqCst), 1);
        *host.allowed.lock().unwrap() = vec!["journal.compaction.read"];
        let denied = freeze_compaction_journal(Ok(Some(connection)), &prompts).unwrap();
        let payload: JournalCompactionInput = denied
            .metadata()
            .feature_payload()
            .unwrap()
            .decode("journal:compaction", 1)
            .unwrap();
        assert!(payload.snapshot.is_none());
        assert_eq!(host.reads.load(SeqCst), 2);
        assert!(
            feature
                .request(JournalRecordingReason::RunCommitted)
                .is_err()
        );
        assert_eq!(host.requests.load(SeqCst), 1);
    }

    #[derive(Default)]
    struct Host {
        requests: Mutex<Vec<JournalJobRequest>>,
    }
    impl JournalJobHost for Host {
        fn execute(
            &self,
            request: JournalJobRequest,
        ) -> Result<JournalJobResponse, SubjektivHostError> {
            self.requests.lock().unwrap().push(request.clone());
            match request {
                JournalJobRequest::Execute { operation } => {
                    Ok(JournalJobResponse::Saved(JournalReceipt {
                        request_id: operation.request_id,
                        generation_id: "generation".into(),
                        entry_id: None,
                        state_token: "saved-token".into(),
                        process_order: 0,
                        process_byte: 0,
                    }))
                }
                _ => Ok(JournalJobResponse::Operation(None)),
            }
        }
        fn finish(&self) -> Result<JournalJobCompletion, SubjektivHostError> {
            Ok(JournalJobCompletion {
                generation_id: "generation".into(),
                state_token: "saved-token".into(),
                process_order: 1,
                process_byte: 0,
            })
        }
    }
    async fn call(
        host: Arc<Host>,
        name: &'static str,
        input: Value,
    ) -> Result<ToolOutput, ToolError> {
        JournalTool {
            name,
            state: JournalJobToolsFeature::new(host),
        }
        .execute(
            &input.to_string(),
            ToolExecutionContext::new("call", "batch", 0),
        )
        .await
    }
    #[tokio::test]
    async fn operation_retry_keeps_complete_original_input_and_exposes_saved_token() {
        let host = Arc::new(Host::default());
        let input = json!({"request_id":"stable-id","expected_state":"old-token", "action":{"action":"confirm_unchanged"},"covered_ranges":[]});
        for _ in 0..2 {
            let output = call(host.clone(), "ConfirmJournalUnchanged", input.clone())
                .await
                .unwrap();
            let output: Value = serde_json::from_str(output.content.as_ref().unwrap()).unwrap();
            assert_eq!(output["value"]["state_token"], "saved-token");
        }
        let requests = host.requests.lock().unwrap();
        assert_eq!(requests[0], requests[1]);
        let JournalJobRequest::Execute { operation } = &requests[0] else {
            panic!("expected operation")
        };
        assert_eq!(serde_json::to_value(operation).unwrap(), input);
    }
    #[tokio::test]
    async fn first_source_chunk_can_start_at_nonzero_selected_offset_without_a_token() {
        let host = Arc::new(Host::default());
        let input = json!({
            "target":{"target":"source","pointer":{"segment_id":uuid::Uuid::nil(),"entry_ref":"Eselected"}},
            "offset":32,"limit":16,"content_token":null
        });
        let schema = tool_schema("ReadJournalText");
        assert!(jsonschema::validator_for(&schema).unwrap().is_valid(&input));
        call(host.clone(), "ReadJournalText", input.clone())
            .await
            .unwrap();
        let requests = host.requests.lock().unwrap();
        let JournalJobRequest::ReadText { read } = &requests[0] else {
            panic!("expected Source read")
        };
        assert_eq!(serde_json::to_value(read).unwrap(), input);
        // Source membership, first offset and later token conditions remain Host checks.
    }
    #[tokio::test]
    async fn model_cannot_select_subject_or_cross_tool_action_or_unbounded_read() {
        let host = Arc::new(Host::default());
        for (name, input) in [
            ("ReadJournalState", json!({"subject_id":"foreign"})),
            (
                "FixJournalEntry",
                json!({"request_id":"id","expected_state":"state", "action":{"action":"confirm_unchanged"},"covered_ranges":[]}),
            ),
            (
                "ReadJournalText",
                json!({"target":{"target":"position","snapshot":"current"},"offset":0,"limit":MAX_JOURNAL_READ_BYTES+1}),
            ),
            (
                "ReadJournalText",
                json!({"target":{"target":"position","snapshot":"current"},"offset":1,"limit":10}),
            ),
            (
                "ReadJournalOperationCoverage",
                json!({"request_id":"old-record","offset":0,"limit":MAX_JOURNAL_PAGE+1}),
            ),
            (
                "ReadJournalOperationCoverage",
                json!({"request_id":"old-record","offset":0,"limit":1,"journal_id":"foreign"}),
            ),
            ("ListJournalOperations", json!({"limit":MAX_JOURNAL_PAGE+1})),
        ] {
            assert!(call(host.clone(), name, input).await.is_err(), "{name}");
        }
        assert!(host.requests.lock().unwrap().is_empty());
    }
    #[test]
    fn truncating_profile_output_budget_is_rejected_before_source_delivery() {
        use crate::job::JobResultFinalizer;
        let finalizer = JournalJobFinalizer::new(Arc::new(Host::default()));
        let mut manifest = manifest::WorkerManifest::from_toml("[worker]\nname='journal-budget'\n[model]\nscheme='anthropic'\nmodel_id='scripted'\n[engine]\n[scope]\nallow=[]").unwrap();
        assert!(finalizer.validate_profile(&manifest).is_ok());
        manifest
            .engine
            .tool_output
            .per_tool
            .insert("ReadJournalText".into(), MAX_JOURNAL_RESPONSE_BYTES - 1);
        assert!(finalizer.validate_profile(&manifest).is_err());
    }
    #[test]
    fn model_schemas_compile_and_constrain_action_and_scope() {
        for &(name, _) in TOOLS {
            let schema = tool_schema(name);
            let validator = jsonschema::validator_for(&schema).unwrap();
            assert!(
                !validator.is_valid(&json!({"subject_id":"foreign"})),
                "{name}"
            );
        }
        let schema = tool_schema("CheckpointJournal");
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(validator.is_valid(&json!({"request_id":"checkpoint", "expected_state":"saved-token", "action":{"action":"checkpoint","record_operation_ids":["saved-record"]},"covered_ranges":[]})));
        assert!(!validator.is_valid(&json!({"request_id":"checkpoint", "expected_state":"saved-token", "action":{"action":"confirm_unchanged"},"covered_ranges":[]})));
    }
}
