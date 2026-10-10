//! Host-scoped model tools for change-identified subjektiv Memory.
//!
//! Confirmed Memory is read-only here. Explicit remembering and change requests
//! become immutable staging candidates; T-670 remains the only authority that may
//! adopt a candidate as a confirmed change.

use std::collections::HashSet;
use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use memory::extract::CandidateKind;
use schemars::JsonSchema;
use serde::Deserialize;
use server_api::{
    SubjektivMemoryBackendOperation, SubjektivMemoryBackendResponse, SubjektivMemoryChangeIntent,
    SubjektivMemoryChangeProposal, SubjektivMemoryListChangesRequest, SubjektivMemoryQueryRequest,
    SubjektivMemoryReadRequest, SubjektivMemoryReceiptStatus, SubjektivMemoryReceiptStatusRequest,
    SubjektivMemoryStageExplicitRequest, SubjektivMemoryStageExplicitResponse,
    SubjektivMemoryValidateProposalRequest,
};

use crate::feature::builtin::memory_staging_output::{source_evidence_ref, staging_evidence};
use crate::feature::session::CommittedSessionCaptureHandle;
use crate::feature::{
    FeatureDescriptor, FeatureHookPoint, FeatureInstallContext, FeatureInstallError, FeatureModule,
    HookDeclaration, ToolContribution, ToolDeclaration,
};
use crate::hook::{
    BeforeSessionRewrite, BeforeSessionRewriteAction, BeforeSessionRewriteContext, Hook, HookError,
    HookErrorCategory, HookExecutionPolicy, HookFailurePolicy, HookPreRequestAction, PreLlmRequest,
    PreRequestContext, RunCommitted, RunCommittedContext,
};
use crate::session_capture::{SessionCapture, SessionEntryEvidence};
use crate::subjektiv::SubjektivHostConnection;
#[cfg(test)]
use crate::worker::WorkspaceClient;
#[cfg(test)]
use server_api::SubjektivMemoryBackendRequest;

const QUERY_TOOL: &str = "SubjektivMemoryQuery";
const READ_TOOL: &str = "SubjektivMemoryRead";
const LIST_CHANGES_TOOL: &str = "SubjektivMemoryListChanges";
const REMEMBER_TOOL: &str = "SubjektivMemoryRemember";
const PROPOSE_CHANGE_TOOL: &str = "SubjektivMemoryProposeChange";
const COMMIT_HOOK: &str = "stage-explicit-memory-after-commit";
const RETRY_HOOK: &str = "retry-committed-explicit-memory";
const REWRITE_HOOK: &str = "flush-explicit-memory-before-rewrite";
const MAX_EVIDENCE_REFS: usize = 10;

const QUERY_DESCRIPTION: &str = "Search the connected subject's current Memory changes. Omitted states means active only; results are bounded and cursors are bound to the subject, filters, order, and store snapshot.";
const READ_DESCRIPTION: &str = "Read one current or immutable historical Memory change for the connected subject, with line- and byte-bounded Markdown plus paged provenance anchors. Continue partial reads using the returned exact change, line offset, and byte offset.";
const LIST_CHANGES_DESCRIPTION: &str = "List immutable changes of one connected-subject Memory in newest-first change order using snapshot-bound pagination.";
const REMEMBER_DESCRIPTION: &str = "Explicitly stage a new Memory candidate for the connected subject. This never writes confirmed Memory. With no entry_refs, the committed tool call itself becomes model-origin evidence after the run commits and status is pending_commit.";
const PROPOSE_DESCRIPTION: &str = "Stage a typed correction, resolution, retraction, or reopen proposal for an exact current Memory change. This validates optimistic concurrency now but never creates a confirmed change.";

#[derive(Clone)]
pub(crate) struct SubjektivMemoryFeature {
    state: SubjektivMemoryState,
}

#[derive(Clone)]
struct SubjektivMemoryState {
    host: SubjektivHostConnection,
    capture: CommittedSessionCaptureHandle,
}

#[derive(Clone)]
struct PendingExplicit {
    receipt_id: String,
    kind: CandidateKind,
    claim: String,
    why_useful: String,
    staleness: Option<String>,
    proposal: Option<SubjektivMemoryChangeProposal>,
}

impl SubjektivMemoryFeature {
    pub(crate) fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        host: Option<SubjektivHostConnection>,
    ) -> std::io::Result<Option<Self>> {
        if !config.profile.enabled || host.is_none() {
            return Ok(None);
        }
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        let host = host.expect("checked explicit connection");
        Ok(Some(Self {
            state: SubjektivMemoryState { host, capture },
        }))
    }
}

impl FeatureModule for SubjektivMemoryFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv-memory-tools", "subjektiv Memory Tools")
            .with_description("Subject-scoped recall and staging-only explicit Memory tools.")
            .with_tool(ToolDeclaration::new(QUERY_TOOL, QUERY_DESCRIPTION))
            .with_tool(ToolDeclaration::new(READ_TOOL, READ_DESCRIPTION))
            .with_tool(ToolDeclaration::new(
                LIST_CHANGES_TOOL,
                LIST_CHANGES_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(REMEMBER_TOOL, REMEMBER_DESCRIPTION))
            .with_tool(ToolDeclaration::new(
                PROPOSE_CHANGE_TOOL,
                PROPOSE_DESCRIPTION,
            ))
            .with_hook(HookDeclaration::new(
                COMMIT_HOOK,
                FeatureHookPoint::RunCommitted,
            ))
            .with_hook(HookDeclaration::new(
                RETRY_HOOK,
                FeatureHookPoint::PreLlmRequest,
            ))
            .with_hook(HookDeclaration::new(
                REWRITE_HOOK,
                FeatureHookPoint::BeforeSessionRewrite,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for (name, description, operation) in [
            (QUERY_TOOL, QUERY_DESCRIPTION, ReadOperation::Query),
            (READ_TOOL, READ_DESCRIPTION, ReadOperation::Read),
            (
                LIST_CHANGES_TOOL,
                LIST_CHANGES_DESCRIPTION,
                ReadOperation::ListChanges,
            ),
        ] {
            context.tools().register(ToolContribution::new(
                name,
                read_tool_definition(name, description, self.state.clone(), operation),
            ))?;
        }
        context.tools().register(ToolContribution::new(
            REMEMBER_TOOL,
            explicit_tool_definition(
                REMEMBER_TOOL,
                REMEMBER_DESCRIPTION,
                self.state.clone(),
                ExplicitOperation::Remember,
            ),
        ))?;
        context.tools().register(ToolContribution::new(
            PROPOSE_CHANGE_TOOL,
            explicit_tool_definition(
                PROPOSE_CHANGE_TOOL,
                PROPOSE_DESCRIPTION,
                self.state.clone(),
                ExplicitOperation::Propose,
            ),
        ))?;
        context.hooks().add_run_committed(
            COMMIT_HOOK,
            HookExecutionPolicy::new(HookFailurePolicy::AttentionRequired, 30_000),
            PendingCommitHook {
                state: self.state.clone(),
            },
        )?;
        context.hooks().add_pre_request(
            RETRY_HOOK,
            PendingRetryHook {
                state: self.state.clone(),
            },
        )?;
        context.hooks().add_before_session_rewrite(
            REWRITE_HOOK,
            HookExecutionPolicy::new(HookFailurePolicy::FailClosed, 30_000),
            PendingRewriteHook {
                state: self.state.clone(),
            },
        )
    }
}

#[derive(Clone, Copy)]
enum ReadOperation {
    Query,
    Read,
    ListChanges,
}

fn read_tool_definition(
    name: &'static str,
    description: &'static str,
    state: SubjektivMemoryState,
    operation: ReadOperation,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = match operation {
            ReadOperation::Query => schema_for::<SubjektivMemoryQueryRequest>(),
            ReadOperation::Read => schema_for::<SubjektivMemoryReadRequest>(),
            ReadOperation::ListChanges => schema_for::<SubjektivMemoryListChangesRequest>(),
        };
        let tool: Arc<dyn Tool> = Arc::new(SubjektivReadTool {
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

struct SubjektivReadTool {
    state: SubjektivMemoryState,
    operation: ReadOperation,
}

#[async_trait]
impl Tool for SubjektivReadTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let operation = match self.operation {
            ReadOperation::Query => {
                SubjektivMemoryBackendOperation::Query(parse(input_json, QUERY_TOOL)?)
            }
            ReadOperation::Read => {
                SubjektivMemoryBackendOperation::Read(parse(input_json, READ_TOOL)?)
            }
            ReadOperation::ListChanges => {
                SubjektivMemoryBackendOperation::ListChanges(parse(input_json, LIST_CHANGES_TOOL)?)
            }
        };
        let response = match self.state.execute(operation) {
            Ok(response) => response,
            Err(error) => return structured_conflict_output(error),
        };
        let (summary, value) = match response {
            SubjektivMemoryBackendResponse::Query(value) => (
                format!("Found {} Memory item(s).", value.items.len()),
                serde_json::to_value(value),
            ),
            SubjektivMemoryBackendResponse::Read(value) => (
                format!(
                    "Read Memory {} change {}.",
                    value.memory_id, value.change_id
                ),
                serde_json::to_value(value),
            ),
            SubjektivMemoryBackendResponse::ListChanges(value) => (
                format!(
                    "Listed {} change(s) for Memory {}.",
                    value.items.len(),
                    value.memory_id
                ),
                serde_json::to_value(value),
            ),
            other => {
                return Err(ToolError::ExecutionFailed(format!(
                    "unexpected subjektiv backend response: {other:?}"
                )));
            }
        };
        let value = value.map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let output = json_output(summary, &value)?;
        if matches!(self.operation, ReadOperation::Read)
            && output.content.as_ref().is_some_and(|content| {
                content.len() > server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES
            })
        {
            return Err(ToolError::ExecutionFailed(
                "subjektiv Memory read exceeded its model-visible output budget".into(),
            ));
        }
        Ok(output)
    }
}

#[derive(Clone, Copy)]
enum ExplicitOperation {
    Remember,
    Propose,
}

fn explicit_tool_definition(
    name: &'static str,
    description: &'static str,
    state: SubjektivMemoryState,
    operation: ExplicitOperation,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = match operation {
            ExplicitOperation::Remember => schema_for::<RememberParams>(),
            ExplicitOperation::Propose => schema_for::<ProposeChangeParams>(),
        };
        let tool: Arc<dyn Tool> = Arc::new(SubjektivExplicitTool {
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

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RememberParams {
    kind: CandidateKind,
    claim: String,
    why_useful: String,
    #[serde(default)]
    staleness: Option<String>,
    #[serde(default)]
    entry_refs: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProposeChangeParams {
    memory_id: String,
    #[schemars(length(min = 1))]
    expected_change_id: String,
    intent: SubjektivMemoryChangeIntent,
    claim: String,
    why_useful: String,
    change_reason: String,
    #[serde(default)]
    staleness: Option<String>,
    #[serde(default)]
    entry_refs: Vec<String>,
}

struct SubjektivExplicitTool {
    state: SubjektivMemoryState,
    operation: ExplicitOperation,
}

#[async_trait]
impl Tool for SubjektivExplicitTool {
    async fn execute(
        &self,
        input_json: &str,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let call_id = required(&context.call_id, "tool call_id")?.to_string();
        let capture = self
            .state
            .capture
            .capture()
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        let receipt_id = receipt_id(&capture.session_id, &call_id)?;
        let (kind, claim, why_useful, staleness, entry_refs, proposal) = match self.operation {
            ExplicitOperation::Remember => {
                let params: RememberParams = parse(input_json, REMEMBER_TOOL)?;
                (
                    params.kind,
                    params.claim,
                    params.why_useful,
                    params.staleness,
                    params.entry_refs,
                    None,
                )
            }
            ExplicitOperation::Propose => {
                let params: ProposeChangeParams = parse(input_json, PROPOSE_CHANGE_TOOL)?;
                if params.expected_change_id.trim().is_empty() {
                    return Err(ToolError::InvalidArgument(
                        "expected_change_id must not be empty".into(),
                    ));
                }
                let validation =
                    match self
                        .state
                        .execute(SubjektivMemoryBackendOperation::ValidateProposal(
                            SubjektivMemoryValidateProposalRequest {
                                memory_id: params.memory_id.clone(),
                                expected_change_id: params.expected_change_id.clone(),
                                intent: params.intent,
                            },
                        )) {
                        Ok(response) => response,
                        Err(error) => return structured_conflict_output(error),
                    };
                let validated = match validation {
                    SubjektivMemoryBackendResponse::ProposalValidated(value) => value,
                    other => {
                        return Err(ToolError::ExecutionFailed(format!(
                            "unexpected proposal validation response: {other:?}"
                        )));
                    }
                };
                let proposal = SubjektivMemoryChangeProposal {
                    memory_id: params.memory_id,
                    expected_change_id: params.expected_change_id.clone(),
                    intent: params.intent,
                    change_reason: params.change_reason,
                };
                (
                    validated.kind,
                    params.claim,
                    params.why_useful,
                    params.staleness,
                    params.entry_refs,
                    Some(proposal),
                )
            }
        };
        validate_candidate_text(&claim, &why_useful)?;
        if entry_refs.len() > MAX_EVIDENCE_REFS {
            return Err(ToolError::InvalidArgument(format!(
                "entry_refs is limited to {MAX_EVIDENCE_REFS} committed references"
            )));
        }
        if proposal
            .as_ref()
            .is_some_and(|proposal| proposal.change_reason.trim().is_empty())
        {
            return Err(ToolError::InvalidArgument(
                "change_reason must not be empty".into(),
            ));
        }

        if entry_refs.is_empty() {
            if kind == CandidateKind::Preference {
                return Err(ToolError::InvalidArgument(
                    "preference candidates require committed HumanInput entry_refs; the model-authored tool call is not preference authority"
                        .into(),
                ));
            }
            let status = self.state.receipt_status(&receipt_id)?;
            if status.status == SubjektivMemoryReceiptStatus::Staged {
                let value = serde_json::to_value(status)
                    .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
                return json_output("Explicit Memory candidate is already staged.", &value);
            }
            let value = serde_json::json!({
                "receipt_id": receipt_id,
                "status": "pending_commit",
                "target": proposal.as_ref().map(|value| serde_json::json!({
                    "memory_id": value.memory_id,
                    "expected_change_id": value.expected_change_id,
                })),
                "intent": proposal.as_ref().map(|value| value.intent),
                "message": "Candidate is not confirmed Memory. It will be staged only after this tool call is durably committed; an uncommitted/cancelled operation is discarded.",
            });
            return json_output(
                "Accepted explicit Memory candidate pending Session commit.",
                &value,
            );
        }

        let entries = resolve_entry_refs(&capture, &entry_refs)?;
        ensure_preference_evidence(&kind, &entries)?;
        let response = match self.state.stage(
            &receipt_id,
            &capture.session_id,
            kind,
            claim,
            why_useful,
            staleness,
            entries,
            proposal,
        ) {
            Ok(response) => response,
            Err(error) => return structured_conflict_output(error),
        };
        staged_output(response)
    }
}

impl SubjektivMemoryState {
    fn execute(
        &self,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, ToolError> {
        self.host.memory(operation).map_err(Into::into)
    }

    fn backend_receipt_status(
        &self,
        expected_receipt_id: &str,
    ) -> Result<server_api::SubjektivMemoryReceiptStatusResponse, ToolError> {
        match self.execute(SubjektivMemoryBackendOperation::ReceiptStatus(
            SubjektivMemoryReceiptStatusRequest {
                receipt_id: expected_receipt_id.to_string(),
            },
        ))? {
            SubjektivMemoryBackendResponse::ReceiptStatus(value) => Ok(value),
            other => Err(ToolError::ExecutionFailed(format!(
                "unexpected receipt status response: {other:?}"
            ))),
        }
    }

    fn receipt_status(
        &self,
        expected_receipt_id: &str,
    ) -> Result<server_api::SubjektivMemoryReceiptStatusResponse, ToolError> {
        let mut status = self.backend_receipt_status(expected_receipt_id)?;
        if status.status == SubjektivMemoryReceiptStatus::Missing {
            let capture = self
                .capture
                .capture()
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
            let session_id = capture.session_id.clone();
            let view = SessionCapture::from_history_entries(capture.segment_id, capture.history);
            let committed = view
                .committed_pending_tool_calls(&[REMEMBER_TOOL, PROPOSE_CHANGE_TOOL])
                .into_iter()
                .any(|call| {
                    receipt_id(&session_id, &call.call_id)
                        .is_ok_and(|committed_receipt| committed_receipt == expected_receipt_id)
                });
            if committed {
                status.status = SubjektivMemoryReceiptStatus::PendingCommit;
            }
        }
        Ok(status)
    }

    #[allow(clippy::too_many_arguments)]
    fn stage(
        &self,
        receipt_id: &str,
        session_id: &str,
        kind: CandidateKind,
        claim: String,
        why_useful: String,
        staleness: Option<String>,
        entries: Vec<SessionEntryEvidence>,
        proposal: Option<SubjektivMemoryChangeProposal>,
    ) -> Result<SubjektivMemoryStageExplicitResponse, ToolError> {
        if entries.is_empty() {
            return Err(ToolError::ExecutionFailed(
                "explicit staging requires committed evidence".into(),
            ));
        }
        let evidence = entries.iter().map(staging_evidence).collect();
        let source_refs = entries.iter().map(source_evidence_ref).collect();
        let response = self.execute(SubjektivMemoryBackendOperation::StageExplicit(
            SubjektivMemoryStageExplicitRequest {
                receipt_id: receipt_id.to_string(),
                session_id: session_id.to_string(),
                kind,
                claim,
                why_useful,
                staleness,
                evidence,
                source_refs,
                proposal,
            },
        ))?;
        match response {
            SubjektivMemoryBackendResponse::Staged(value) => Ok(value),
            other => Err(ToolError::ExecutionFailed(format!(
                "unexpected explicit staging response: {other:?}"
            ))),
        }
    }
}

struct PendingCommitHook {
    state: SubjektivMemoryState,
}

#[async_trait]
impl Hook<RunCommitted> for PendingCommitHook {
    async fn call(&self, _input: &RunCommittedContext) -> Result<(), HookError> {
        replay_pending_committed_memory(&self.state)
    }
}

struct PendingRetryHook {
    state: SubjektivMemoryState,
}

#[async_trait]
impl Hook<PreLlmRequest> for PendingRetryHook {
    async fn call(&self, _input: &PreRequestContext) -> Result<HookPreRequestAction, HookError> {
        replay_pending_committed_memory_with_policy(&self.state, true)?;
        Ok(HookPreRequestAction::Continue)
    }
}

struct PendingRewriteHook {
    state: SubjektivMemoryState,
}

#[async_trait]
impl Hook<BeforeSessionRewrite> for PendingRewriteHook {
    async fn call(
        &self,
        _input: &BeforeSessionRewriteContext,
    ) -> Result<BeforeSessionRewriteAction, HookError> {
        replay_pending_committed_memory(&self.state)?;
        Ok(BeforeSessionRewriteAction::Continue)
    }
}

fn replay_pending_committed_memory(state: &SubjektivMemoryState) -> Result<(), HookError> {
    replay_pending_committed_memory_with_policy(state, false)
}
fn replay_pending_committed_memory_with_policy(
    state: &SubjektivMemoryState,
    allow_pending: bool,
) -> Result<(), HookError> {
    let capture = state
        .capture
        .capture()
        .map_err(|error| HookError::new(HookErrorCategory::Dependency, error.to_string()))?;
    let view =
        SessionCapture::from_history_entries(capture.segment_id.clone(), capture.history.clone());
    let mut failures = Vec::new();
    for call in view.committed_pending_tool_calls(&[REMEMBER_TOOL, PROPOSE_CHANGE_TOOL]) {
        let receipt_id = match receipt_id(&capture.session_id, &call.call_id) {
            Ok(value) => value,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        match state.backend_receipt_status(&receipt_id) {
            Ok(status) if status.status == SubjektivMemoryReceiptStatus::Staged => continue,
            Ok(_) => {}
            Err(error) => {
                failures.push(format!("{receipt_id}: {error}"));
                continue;
            }
        }
        let item =
            match pending_from_committed_call(state, &receipt_id, &call.name, &call.arguments) {
                Ok(Some(value)) => value,
                Ok(None) => continue,
                Err(error) => {
                    failures.push(format!("{receipt_id}: {error}"));
                    continue;
                }
            };
        if let Err(error) = state.stage(
            &item.receipt_id,
            &capture.session_id,
            item.kind,
            item.claim,
            item.why_useful,
            item.staleness,
            vec![call.evidence],
            item.proposal,
        ) {
            if !allow_pending
                || !matches!(&error,ToolError::StructuredConflict {code,..} if code=="pending_commit")
            {
                failures.push(format!("{receipt_id}: {error}"));
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(HookError::new(
            HookErrorCategory::Dependency,
            format!(
                "explicit Memory staging after commit failed: {}",
                failures.join("; ")
            ),
        ))
    }
}

fn pending_from_committed_call(
    state: &SubjektivMemoryState,
    receipt_id: &str,
    tool_name: &str,
    arguments: &str,
) -> Result<Option<PendingExplicit>, ToolError> {
    let (kind, claim, why_useful, staleness, entry_refs, proposal) = match tool_name {
        REMEMBER_TOOL => {
            let params: RememberParams = parse(arguments, REMEMBER_TOOL)?;
            (
                params.kind,
                params.claim,
                params.why_useful,
                params.staleness,
                params.entry_refs,
                None,
            )
        }
        PROPOSE_CHANGE_TOOL => {
            let params: ProposeChangeParams = parse(arguments, PROPOSE_CHANGE_TOOL)?;
            let response = state.execute(SubjektivMemoryBackendOperation::ValidateProposal(
                SubjektivMemoryValidateProposalRequest {
                    memory_id: params.memory_id.clone(),
                    expected_change_id: params.expected_change_id.clone(),
                    intent: params.intent,
                },
            ))?;
            let validated = match response {
                SubjektivMemoryBackendResponse::ProposalValidated(value) => value,
                other => {
                    return Err(ToolError::ExecutionFailed(format!(
                        "unexpected proposal validation response: {other:?}"
                    )));
                }
            };
            (
                validated.kind,
                params.claim,
                params.why_useful,
                params.staleness,
                params.entry_refs,
                Some(SubjektivMemoryChangeProposal {
                    memory_id: params.memory_id,
                    expected_change_id: params.expected_change_id.clone(),
                    intent: params.intent,
                    change_reason: params.change_reason,
                }),
            )
        }
        _ => return Ok(None),
    };
    if !entry_refs.is_empty() {
        return Ok(None);
    }
    validate_candidate_text(&claim, &why_useful)?;
    if kind == CandidateKind::Preference {
        return Err(ToolError::InvalidArgument(
            "preference candidates cannot use model-origin implicit evidence".into(),
        ));
    }
    Ok(Some(PendingExplicit {
        receipt_id: receipt_id.to_string(),
        kind,
        claim,
        why_useful,
        staleness,
        proposal,
    }))
}

fn resolve_entry_refs(
    capture: &crate::feature::session::CommittedSessionCapture,
    entry_refs: &[String],
) -> Result<Vec<SessionEntryEvidence>, ToolError> {
    let view =
        SessionCapture::from_history_entries(capture.segment_id.clone(), capture.history.clone());
    let mut seen = HashSet::new();
    entry_refs
        .iter()
        .map(|entry_ref| {
            if !seen.insert(entry_ref) {
                return Err(ToolError::InvalidArgument(format!(
                    "duplicate SessionEntryRef {entry_ref:?}"
                )));
            }
            view.evidence_for(entry_ref).ok_or_else(|| {
                ToolError::InvalidArgument(format!(
                    "unknown SessionEntryRef {entry_ref:?} for the committed Session"
                ))
            })
        })
        .collect()
}

fn ensure_preference_evidence(
    kind: &CandidateKind,
    entries: &[SessionEntryEvidence],
) -> Result<(), ToolError> {
    if kind == &CandidateKind::Preference
        && entries
            .iter()
            .any(|entry| !matches!(entry.origin, protocol::SessionEntryProvenance::HumanInput))
    {
        return Err(ToolError::InvalidArgument(
            "preference candidates require exclusively HumanInput evidence".into(),
        ));
    }
    Ok(())
}

fn receipt_id(session_id: &str, call_id: &str) -> Result<String, ToolError> {
    required(session_id, "Session id")?;
    required(call_id, "tool call_id")?;
    let value = format!("explicit:{session_id}:{call_id}");
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err(ToolError::InvalidArgument(
            "explicit Memory receipt identity is invalid".into(),
        ));
    }
    Ok(value)
}

fn validate_candidate_text(claim: &str, why_useful: &str) -> Result<(), ToolError> {
    required(claim, "claim")?;
    required(why_useful, "why_useful")?;
    Ok(())
}

fn required<'a>(value: &'a str, field: &str) -> Result<&'a str, ToolError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(ToolError::InvalidArgument(format!("{field} is invalid")))
    } else {
        Ok(value)
    }
}

fn parse<T: serde::de::DeserializeOwned>(input: &str, tool: &str) -> Result<T, ToolError> {
    serde_json::from_str(input)
        .map_err(|error| ToolError::InvalidArgument(format!("invalid {tool} input: {error}")))
}

fn schema_for<T: JsonSchema>() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(T)).unwrap_or_else(|_| serde_json::json!({}))
}

fn structured_conflict_output(error: ToolError) -> Result<ToolOutput, ToolError> {
    match error {
        ToolError::StructuredConflict { code, message } => json_output(
            format!("Subjektiv Memory operation rejected with {code}."),
            &serde_json::json!({
                "status": "error",
                "error": {
                    "code": code,
                    "message": message,
                },
            }),
        ),
        other => Err(other),
    }
}

fn staged_output(response: SubjektivMemoryStageExplicitResponse) -> Result<ToolOutput, ToolError> {
    if !matches!(response.status, SubjektivMemoryReceiptStatus::Staged) {
        return Err(ToolError::ExecutionFailed(
            "subjektiv backend returned a non-staged receipt".into(),
        ));
    }
    let value = serde_json::to_value(&response)
        .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
    json_output(
        format!(
            "Staged explicit Memory candidate {}.",
            response.candidate_id
        ),
        &value,
    )
}

fn json_output(
    summary: impl Into<String>,
    value: &serde_json::Value,
) -> Result<ToolOutput, ToolError> {
    Ok(ToolOutput {
        summary: summary.into(),
        content: Some(
            serde_json::to_string_pretty(value)
                .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?,
        ),
        attachments: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::session::{CommittedRunExit, CommittedSessionCapture};
    use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
    use std::collections::HashSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug)]
    struct TestWorkspaceClient;

    impl WorkspaceClient for TestWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            let request: SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let SubjektivMemoryBackendOperation::ReceiptStatus(input) = request.operation else {
                return Err(WorkspaceClientError::Request(
                    "test client only supports receipt lookup".into(),
                ));
            };
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&SubjektivMemoryBackendResponse::ReceiptStatus(
                    server_api::SubjektivMemoryReceiptStatusResponse {
                        receipt_id: input.receipt_id,
                        status: SubjektivMemoryReceiptStatus::Missing,
                        candidate_id: None,
                    },
                ))
                .unwrap(),
            })
        }
    }

    #[derive(Debug)]
    struct ConflictWorkspaceClient(&'static str);

    impl WorkspaceClient for ConflictWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "conflict-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            _request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            Ok(WorkspaceResponse {
                status: 409,
                body: serde_json::to_string(&server_api::RepositoryApiError::new(
                    409,
                    "Conflict",
                    format!("{}: retry with a fresh snapshot", self.0),
                    vec![server_api::Diagnostic {
                        code: self.0.to_string(),
                        severity: server_api::DiagnosticSeverity::Error,
                        message: "typed conflict".into(),
                    }],
                ))
                .unwrap(),
            })
        }
    }

    #[derive(Debug)]
    struct ReadWorkspaceClient(server_api::SubjektivMemoryReadResponse);

    impl WorkspaceClient for ReadWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "read-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            let request: SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            assert!(matches!(
                request.operation,
                SubjektivMemoryBackendOperation::Read(_)
            ));
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&SubjektivMemoryBackendResponse::Read(self.0.clone()))
                    .unwrap(),
            })
        }
    }

    #[derive(Debug, Default)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
        staged: Mutex<HashSet<String>>,
        fail_stages: AtomicUsize,
    }

    impl WorkspaceClient for RecordingWorkspaceClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-test")
        }

        fn kind(&self) -> &str {
            "recording-test"
        }

        fn is_available(&self) -> bool {
            true
        }

        fn execute(
            &self,
            request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            self.requests.lock().unwrap().push(request.clone());
            let request: SubjektivMemoryBackendRequest =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            let response = match request.operation {
                SubjektivMemoryBackendOperation::ReceiptStatus(input) => {
                    let staged = self.staged.lock().unwrap().contains(&input.receipt_id);
                    SubjektivMemoryBackendResponse::ReceiptStatus(
                        server_api::SubjektivMemoryReceiptStatusResponse {
                            receipt_id: input.receipt_id.clone(),
                            status: if staged {
                                SubjektivMemoryReceiptStatus::Staged
                            } else {
                                SubjektivMemoryReceiptStatus::Missing
                            },
                            candidate_id: staged.then_some(input.receipt_id),
                        },
                    )
                }
                SubjektivMemoryBackendOperation::StageExplicit(input) => {
                    if self
                        .fail_stages
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                            remaining.checked_sub(1)
                        })
                        .is_ok()
                    {
                        return Ok(WorkspaceResponse {
                            status: 503,
                            body: r#"{"error":"Unavailable","message":"retry","diagnostics":[]}"#
                                .into(),
                        });
                    }
                    self.staged.lock().unwrap().insert(input.receipt_id.clone());
                    SubjektivMemoryBackendResponse::Staged(SubjektivMemoryStageExplicitResponse {
                        candidate_id: input.receipt_id.clone(),
                        receipt_id: input.receipt_id,
                        status: SubjektivMemoryReceiptStatus::Staged,
                        target: None,
                        intent: None,
                    })
                }
                other => panic!("unexpected test operation: {other:?}"),
            };
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&response).unwrap(),
            })
        }
    }

    fn feature() -> SubjektivMemoryFeature {
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),

                entry_count: 0,
                run_exit: CommittedRunExit::Finished,
                history: Vec::new(),
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        SubjektivMemoryFeature {
            state: SubjektivMemoryState {
                host: crate::subjektiv::test_connection(Arc::new(TestWorkspaceClient)),
                capture,
            },
        }
    }

    #[test]
    fn descriptor_registers_exact_five_tools_and_commit_hook() {
        let descriptor = feature().descriptor();
        let names = descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                QUERY_TOOL,
                READ_TOOL,
                LIST_CHANGES_TOOL,
                REMEMBER_TOOL,
                PROPOSE_CHANGE_TOOL,
            ]
        );
        assert_eq!(descriptor.hooks.len(), 3);
        assert_eq!(descriptor.hooks[0].name, COMMIT_HOOK);
        assert_eq!(descriptor.hooks[0].point, FeatureHookPoint::RunCommitted);
        assert_eq!(descriptor.hooks[1].name, RETRY_HOOK);
        assert_eq!(descriptor.hooks[1].point, FeatureHookPoint::PreLlmRequest);
        assert_eq!(descriptor.hooks[2].name, REWRITE_HOOK);
        assert_eq!(
            descriptor.hooks[2].point,
            FeatureHookPoint::BeforeSessionRewrite
        );
    }

    #[test]
    fn model_schemas_exclude_host_scope_and_allow_optional_evidence_refs() {
        let remember = schema_for::<RememberParams>();
        let remember_text = serde_json::to_string(&remember).unwrap();
        assert!(!remember_text.contains("subject_id"));
        assert!(!remember_text.contains("session_id"));
        assert!(!remember_text.contains("origin"));
        assert!(
            !remember["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field == "entry_refs")
        );

        let propose = schema_for::<ProposeChangeParams>();
        let propose_text = serde_json::to_string(&propose).unwrap();
        assert!(!propose_text.contains("subject_id"));
        assert!(!propose_text.contains("\"kind\""));
        assert!(propose_text.contains("expected_change_id"));
        assert!(propose_text.contains("change_reason"));

        let parsed: RememberParams = serde_json::from_str(
            r#"{"kind":"lesson","claim":"Remember this","why_useful":"Later recall"}"#,
        )
        .unwrap();
        assert!(parsed.entry_refs.is_empty());
    }

    #[tokio::test]
    async fn read_tool_preserves_bounded_model_visible_json_with_escape_heavy_content() {
        let origin = || memory::schema::EvidenceOrigin {
            kind: memory::schema::EvidenceOriginKind::ModelOutput,
            account_id: Some("a".repeat(64)),
            workspace_id: Some("workspace-test".into()),
            runtime_id: Some("r".repeat(64)),
            worker_id: Some("w".repeat(64)),
            flow_selector: Some("f".repeat(64)),
            flow_definition_id: Some("d".repeat(64)),
            flow_definition_fingerprint: Some("fingerprint".into()),
        };
        let evidence = (0..MAX_EVIDENCE_REFS)
            .map(|index| memory::extract::StagingEvidence {
                id: format!("evidence-{index}"),
                kind: memory::schema::EvidenceKind::new("model_output"),
                entry_range: Some([index as u64, index as u64]),
                origin: Some(origin()),
                excerpt: Some("\\\u{0000}".repeat(32)),
                summary: Some("\\\u{0001}".repeat(32)),
            })
            .collect::<Vec<_>>();
        let source_refs = (0..MAX_EVIDENCE_REFS)
            .map(|index| memory::schema::SourceEvidenceRef {
                session_id: Some(format!("session-{index}")),
                segment_id: Some(format!("segment-{index}")),
                entry_range: Some([index as u64, index as u64]),
                evidence_id: Some(format!("evidence-{index}")),
                origin: Some(origin()),
                evidence_kind: Some(memory::schema::EvidenceKind::new("model_output")),
                label: Some("\\\u{0002}".repeat(32)),
                summary: Some("\\\u{0003}".repeat(32)),
            })
            .collect::<Vec<_>>();
        let response = server_api::SubjektivMemoryReadResponse {
            memory_id: "memory-1".into(),
            change_id: "change-1".into(),
            current_change_id: "change-1".into(),
            kind: CandidateKind::Lesson,
            state: server_api::SubjektivMemoryState::Active,
            claim: "Escape-aware read".into(),
            body_md: "\\\u{0000}\u{0001}".repeat(2_000),
            why_useful: "Proves the actual ToolOutput remains parseable".into(),
            staleness: None,
            change_reason: "test".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            body_offset: 0,
            body_byte_offset: 0,
            body_next_offset: Some(0),
            body_next_byte_offset: Some(6_000),
            body_truncated: true,
            source_candidate_ids: vec!["candidate-1".into()],
            source_candidates: vec![server_api::SubjektivMemoryEvidenceCandidate {
                candidate_id: "candidate-1".into(),
                evidence,
                evidence_total: MAX_EVIDENCE_REFS,
                evidence_truncated: false,
                source_refs,
                source_refs_total: MAX_EVIDENCE_REFS,
                source_refs_truncated: false,
            }],
            derived_from: Vec::new(),
            evidence_next_cursor: None,
            evidence_has_more: false,
        };
        let mut oversized_response = response.clone();
        oversized_response.body_md =
            "\\\u{0000}".repeat(server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES);
        let tool = SubjektivReadTool {
            state: SubjektivMemoryState {
                host: crate::subjektiv::test_connection(Arc::new(ReadWorkspaceClient(response))),
                capture: feature().state.capture,
            },
            operation: ReadOperation::Read,
        };
        let output = tool
            .execute(
                r#"{"memory_id":"memory-1"}"#,
                ToolExecutionContext::new("call-read", "batch-read", 0),
            )
            .await
            .unwrap();
        let content = output.content.unwrap();
        assert!(content.len() <= server_api::SUBJEKTIV_MEMORY_READ_MAX_TOOL_CONTENT_BYTES);
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["body_truncated"], true);
        assert_eq!(value["body_next_byte_offset"], 6_000);

        let oversized_tool = SubjektivReadTool {
            state: SubjektivMemoryState {
                host: crate::subjektiv::test_connection(Arc::new(ReadWorkspaceClient(
                    oversized_response,
                ))),
                capture: feature().state.capture,
            },
            operation: ReadOperation::Read,
        };
        let error = oversized_tool
            .execute(
                r#"{"memory_id":"memory-1"}"#,
                ToolExecutionContext::new("call-oversized", "batch-read", 1),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("model-visible output budget"));
    }

    #[tokio::test]
    async fn backend_conflict_discriminants_survive_as_structured_tool_output() {
        for code in ["change_conflict", "stale_cursor"] {
            let tool = SubjektivReadTool {
                state: SubjektivMemoryState {
                    host: crate::subjektiv::test_connection(Arc::new(ConflictWorkspaceClient(
                        code,
                    ))),
                    capture: feature().state.capture,
                },
                operation: ReadOperation::Query,
            };
            let output = tool
                .execute("{}", ToolExecutionContext::new("call", "batch", 0))
                .await
                .unwrap();
            let value: serde_json::Value =
                serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(value["status"], "error");
            assert_eq!(value["error"]["code"], code);
        }
    }

    #[test]
    fn receipt_identity_is_stable_and_rejects_invalid_values() {
        assert_eq!(
            receipt_id("session-1", "call-1").unwrap(),
            "explicit:session-1:call-1"
        );
        assert!(receipt_id("", "call-1").is_err());
        assert!(receipt_id("session-1", "bad\ncall").is_err());
    }

    #[tokio::test]
    async fn remember_without_refs_is_pending_until_commit_and_idempotent_per_call() {
        let feature = feature();
        let tool = SubjektivExplicitTool {
            state: feature.state.clone(),
            operation: ExplicitOperation::Remember,
        };
        let input =
            r#"{"kind":"lesson","claim":"Use fixed changes","why_useful":"Avoid mixed reads"}"#;
        let output = tool
            .execute(input, ToolExecutionContext::new("call-1", "batch-1", 0))
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value["status"], "pending_commit");
        assert_eq!(value["receipt_id"], "explicit:session-1:call-1");
        tool.execute(input, ToolExecutionContext::new("call-1", "batch-1", 0))
            .await
            .unwrap();

        let preference = tool
            .execute(
                r#"{"kind":"preference","claim":"I prefer X","why_useful":"Personalization"}"#,
                ToolExecutionContext::new("call-2", "batch-1", 1),
            )
            .await
            .unwrap_err();
        assert!(preference.to_string().contains("HumanInput"));
    }

    #[tokio::test]
    async fn cancelled_or_uncommitted_calls_are_not_replayed() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),

                entry_count: 3,
                run_exit: CommittedRunExit::Interrupted,
                history: vec![
                    crate::session_history::history_entry(
                        agen::Item::tool_call(
                            "call-uncommitted",
                            REMEMBER_TOOL,
                            r#"{"kind":"lesson","claim":"No result","why_useful":"Never stage"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ModelOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                    crate::session_history::history_entry(
                        agen::Item::tool_call(
                            "call-cancelled",
                            REMEMBER_TOOL,
                            r#"{"kind":"lesson","claim":"Cancelled","why_useful":"Never stage"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ModelOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                    crate::session_history::history_entry(
                        agen::Item::tool_result_item("call-cancelled", "cancelled", None, true),
                        crate::session_history::WorkerHistoryProvenance::ToolOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                ],
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        PendingCommitHook {
            state: SubjektivMemoryState {
                host: crate::subjektiv::test_connection(client.clone()),
                capture,
            },
        }
        .call(&RunCommittedContext {
            invocation: crate::hook::HookInvocationContext::default(),
            exit: crate::hook::RunCommittedExit::Interrupted,
            committed_history: crate::hook::HookHistoryRange::default(),
        })
        .await
        .unwrap();
        assert!(client.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_commit_hook_retries_from_durable_history() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        client.fail_stages.store(1, Ordering::SeqCst);
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),

                entry_count: 2,
                run_exit: CommittedRunExit::Finished,
                history: vec![
                    crate::session_history::history_entry(
                        agen::Item::tool_call(
                            "call-retry",
                            REMEMBER_TOOL,
                            r#"{"kind":"lesson","claim":"Retry","why_useful":"Recover after failure"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ModelOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                    crate::session_history::history_entry(
                        agen::Item::tool_result_with_content(
                            "call-retry",
                            "pending commit",
                            r#"{"status":"pending_commit"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ToolOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                ],
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        let hook = PendingCommitHook {
            state: SubjektivMemoryState {
                host: crate::subjektiv::test_connection(client.clone()),
                capture,
            },
        };
        let context = RunCommittedContext {
            invocation: crate::hook::HookInvocationContext::default(),
            exit: crate::hook::RunCommittedExit::Finished,
            committed_history: crate::hook::HookHistoryRange::default(),
        };
        assert!(hook.call(&context).await.is_err());
        hook.call(&context).await.unwrap();
        hook.call(&context).await.unwrap();
        assert!(
            client
                .staged
                .lock()
                .unwrap()
                .contains("explicit:session-1:call-retry")
        );
        let stage_attempts = client
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| {
                let request: SubjektivMemoryBackendRequest =
                    serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
                matches!(
                    request.operation,
                    SubjektivMemoryBackendOperation::StageExplicit(_)
                )
            })
            .count();
        assert_eq!(
            stage_attempts, 2,
            "a staged receipt must not be replayed again"
        );
    }

    #[tokio::test]
    async fn committed_tool_call_is_replayed_after_restart_with_host_evidence() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),

                entry_count: 2,
                run_exit: CommittedRunExit::Finished,
                history: vec![
                    crate::session_history::history_entry(
                        agen::Item::tool_call(
                            "call-commit",
                            REMEMBER_TOOL,
                            r#"{"kind":"lesson","claim":"Committed","why_useful":"Evidence"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ModelOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                    crate::session_history::history_entry(
                        agen::Item::tool_result_with_content(
                            "call-commit",
                            "pending commit",
                            r#"{"status":"pending_commit"}"#,
                        ),
                        crate::session_history::WorkerHistoryProvenance::ToolOutput {
                            worker: crate::session_history::worker_subject(Default::default()),
                        },
                    ),
                ],
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        let state = SubjektivMemoryState {
            host: crate::subjektiv::test_connection(client.clone()),
            capture,
        };
        assert_eq!(
            state
                .receipt_status("explicit:session-1:call-commit")
                .unwrap()
                .status,
            SubjektivMemoryReceiptStatus::PendingCommit
        );

        PendingRetryHook {
            state: state.clone(),
        }
        .call(&crate::hook::PreRequestContext::new(
            crate::hook::PreRequestInfo {
                item_count: 2,
                estimated_tokens: None,
                turn_index: 0,
                tool_calls_this_turn: 0,
            },
            None,
        ))
        .await
        .unwrap();
        let requests = client.requests.lock().unwrap();
        let request = requests
            .iter()
            .filter_map(|request| {
                serde_json::from_str::<SubjektivMemoryBackendRequest>(
                    request.body.as_deref().unwrap(),
                )
                .ok()
            })
            .find_map(|request| match request.operation {
                SubjektivMemoryBackendOperation::StageExplicit(staged) => Some(staged),
                _ => None,
            })
            .expect("restart replay must stage the committed receipt");
        assert_eq!(request.session_id, "session-1");
        assert_eq!(request.evidence.len(), 1);
        assert_eq!(
            request.evidence[0].origin.as_ref().unwrap().kind,
            memory::schema::EvidenceOriginKind::ModelOutput
        );
        assert_eq!(
            request.source_refs[0].evidence_id.as_deref(),
            Some(request.evidence[0].id.as_str())
        );
    }
}
