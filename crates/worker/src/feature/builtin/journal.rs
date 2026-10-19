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
use std::sync::Arc;
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
}
impl JournalContextSource {
    pub(crate) fn new(
        host: SubjektivHostConnection,
        prompts: Arc<arc_swap::ArcSwap<crate::prompt::catalog::PromptCatalog>>,
    ) -> Self {
        Self { host, prompts }
    }
}
#[async_trait]
impl crate::worker::SessionContextSource for JournalContextSource {
    async fn load(&self) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
        use sha2::{Digest, Sha256};
        if !self.host.host.journal_startup_granted() {
            return Ok(Vec::new());
        }
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
        Ok(vec![session_store::LoggedSystemHistoryEntry::from_input(
            session_store::LoggedSessionHistoryOrigin::Provided {
                producer: "subjektiv.journal".into(),
            },
            None,
            journal_input(body, payload, "journal_startup")?,
        )])
    }

    async fn load_compaction_context(
        &self,
        session: session_store::SessionId,
    ) -> Result<Vec<session_store::LoggedSystemHistoryEntry>, String> {
        // A changed Session must never reuse the old source's Host ownership.
        let connection = if self.host.context.session_id == session.to_string() {
            Ok(Some(self.host.clone()))
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "context source belongs to a different Session",
            ))
        };
        let input = freeze_compaction_journal(connection, &self.prompts.load_full())?;
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
        Ok(Some(connection)) if connection.host.journal_compaction_granted() => {
            Some(connection.journal_compaction_context().map_err(|_| ()))
        }
        Ok(_) => None,
        Err(_) => Some(Err(())),
    };
    let snapshot = read.as_ref().and_then(|read| read.as_ref().ok()).cloned();
    let (status, json) = match read {
        None => (Disabled, String::new()),
        Some(Ok(snapshot)) => {
            let status = if snapshot.journals.is_empty() {
                Absent
            } else {
                Available
            };
            match serde_json::to_string(&snapshot) {
                Ok(json) if json.len() <= subjektiv::journal::startup::STARTUP_MAX_BYTES => {
                    (status, json)
                }
                _ => (Unavailable, String::new()),
            }
        }
        Some(Err(())) => (Unavailable, String::new()),
    };
    if status == Unavailable {
        tracing::warn!("Journal compaction snapshot unavailable; continuing without Journal");
    }
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
            .host
            .request_journal_recording(&self.host.context, reason)
            .map(|_| ())
            .map_err(|_| {
                HookError::new(
                    HookErrorCategory::Dependency,
                    "Journal admission unconfirmed; Host must reconcile saved public history",
                )
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
mod tests {
    use super::*;
    use std::sync::Mutex;
    use subjektiv::journal::JournalReceipt;

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
