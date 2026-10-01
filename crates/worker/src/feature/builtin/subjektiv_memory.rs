//! Host-scoped model tools for revisioned subjektiv Memory.
//!
//! Confirmed Memory is read-only here. Explicit remembering and revision requests
//! become immutable staging candidates; T-670 remains the only authority that may
//! adopt a candidate as a confirmed revision.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use agen::tool::{Tool, ToolDefinition, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use async_trait::async_trait;
use memory::extract::CandidateKind;
use schemars::JsonSchema;
use serde::Deserialize;
use server_api::{
    SubjektivMemoryBackendOperation, SubjektivMemoryBackendRequest, SubjektivMemoryBackendResponse,
    SubjektivMemoryListRevisionsRequest, SubjektivMemoryQueryRequest, SubjektivMemoryReadRequest,
    SubjektivMemoryReceiptStatus, SubjektivMemoryRevisionIntent, SubjektivMemoryRevisionProposal,
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
    Hook, HookError, HookErrorCategory, HookExecutionPolicy, HookFailurePolicy, RunCommitted,
    RunCommittedContext,
};
use crate::session_capture::{SessionCapture, SessionEntryEvidence};
use crate::worker::{WorkspaceClient, WorkspaceServerOperation};

const QUERY_TOOL: &str = "SubjektivMemoryQuery";
const READ_TOOL: &str = "SubjektivMemoryRead";
const LIST_REVISIONS_TOOL: &str = "SubjektivMemoryListRevisions";
const REMEMBER_TOOL: &str = "SubjektivMemoryRemember";
const PROPOSE_REVISION_TOOL: &str = "SubjektivMemoryProposeRevision";
const COMMIT_HOOK: &str = "stage-explicit-memory-after-commit";

const QUERY_DESCRIPTION: &str = "Search the connected subject's current Memory revisions. Omitted states means active only; results are bounded and cursors are bound to the subject, filters, order, and store snapshot.";
const READ_DESCRIPTION: &str = "Read one current or immutable historical Memory revision for the connected subject, with line-bounded Markdown and paged provenance anchors. Continue partial reads using the returned exact revision.";
const LIST_REVISIONS_DESCRIPTION: &str = "List immutable revisions of one connected-subject Memory in descending revision order using snapshot-bound pagination.";
const REMEMBER_DESCRIPTION: &str = "Explicitly stage a new Memory candidate for the connected subject. This never writes confirmed Memory. With no entry_refs, the committed tool call itself becomes model-origin evidence after the run commits and status is pending_commit.";
const PROPOSE_DESCRIPTION: &str = "Stage a typed correction, resolution, retraction, or reopen proposal for an exact current Memory revision. This validates optimistic concurrency now but never creates a confirmed revision.";

#[derive(Clone)]
pub(crate) struct SubjektivMemoryFeature {
    state: SubjektivMemoryState,
}

#[derive(Clone)]
struct SubjektivMemoryState {
    client: Arc<dyn WorkspaceClient>,
    capture: CommittedSessionCaptureHandle,
    pending: Arc<Mutex<HashMap<String, PendingExplicit>>>,
}

#[derive(Clone)]
struct PendingExplicit {
    receipt_id: String,
    call_id: String,
    kind: CandidateKind,
    claim: String,
    why_useful: String,
    staleness: Option<String>,
    proposal: Option<SubjektivMemoryRevisionProposal>,
}

impl SubjektivMemoryFeature {
    pub(crate) fn from_resolved_config(
        config: &manifest::ResolvedSubjektivFeatureConfig,
        capture: CommittedSessionCaptureHandle,
        client: Arc<dyn WorkspaceClient>,
    ) -> std::io::Result<Option<Self>> {
        if !config.profile.enabled {
            return Ok(None);
        }
        config
            .validate_execution()
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        if !client.is_available() || client.workspace_id().is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "subjektiv Memory tools require Backend Workspace API authority",
            ));
        }
        Ok(Some(Self {
            state: SubjektivMemoryState {
                client,
                capture,
                pending: Arc::new(Mutex::new(HashMap::new())),
            },
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
                LIST_REVISIONS_TOOL,
                LIST_REVISIONS_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(REMEMBER_TOOL, REMEMBER_DESCRIPTION))
            .with_tool(ToolDeclaration::new(
                PROPOSE_REVISION_TOOL,
                PROPOSE_DESCRIPTION,
            ))
            .with_hook(HookDeclaration::new(
                COMMIT_HOOK,
                FeatureHookPoint::RunCommitted,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        for (name, description, operation) in [
            (QUERY_TOOL, QUERY_DESCRIPTION, ReadOperation::Query),
            (READ_TOOL, READ_DESCRIPTION, ReadOperation::Read),
            (
                LIST_REVISIONS_TOOL,
                LIST_REVISIONS_DESCRIPTION,
                ReadOperation::ListRevisions,
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
            PROPOSE_REVISION_TOOL,
            explicit_tool_definition(
                PROPOSE_REVISION_TOOL,
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
        )
    }
}

#[derive(Clone, Copy)]
enum ReadOperation {
    Query,
    Read,
    ListRevisions,
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
            ReadOperation::ListRevisions => schema_for::<SubjektivMemoryListRevisionsRequest>(),
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
            ReadOperation::ListRevisions => SubjektivMemoryBackendOperation::ListRevisions(parse(
                input_json,
                LIST_REVISIONS_TOOL,
            )?),
        };
        let response = self.state.execute(operation)?;
        let (summary, value) = match response {
            SubjektivMemoryBackendResponse::Query(value) => (
                format!("Found {} Memory item(s).", value.items.len()),
                serde_json::to_value(value),
            ),
            SubjektivMemoryBackendResponse::Read(value) => (
                format!(
                    "Read Memory {} revision {}.",
                    value.memory_id, value.revision
                ),
                serde_json::to_value(value),
            ),
            SubjektivMemoryBackendResponse::ListRevisions(value) => (
                format!(
                    "Listed {} revision(s) for Memory {}.",
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
        Ok(json_output(summary, &value)?)
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
            ExplicitOperation::Propose => schema_for::<ProposeRevisionParams>(),
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
struct ProposeRevisionParams {
    memory_id: String,
    #[schemars(range(min = 1, max = 9_007_199_254_740_991_u64))]
    expected_revision: u64,
    intent: SubjektivMemoryRevisionIntent,
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
                let params: ProposeRevisionParams = parse(input_json, PROPOSE_REVISION_TOOL)?;
                if params.expected_revision == 0 {
                    return Err(ToolError::InvalidArgument(
                        "expected_revision must be a positive integer".into(),
                    ));
                }
                let validation =
                    self.state
                        .execute(SubjektivMemoryBackendOperation::ValidateProposal(
                            SubjektivMemoryValidateProposalRequest {
                                memory_id: params.memory_id.clone(),
                                expected_revision: params.expected_revision,
                                intent: params.intent,
                            },
                        ))?;
                let validated = match validation {
                    SubjektivMemoryBackendResponse::ProposalValidated(value) => value,
                    other => {
                        return Err(ToolError::ExecutionFailed(format!(
                            "unexpected proposal validation response: {other:?}"
                        )));
                    }
                };
                let proposal = SubjektivMemoryRevisionProposal {
                    memory_id: params.memory_id,
                    expected_revision: params.expected_revision,
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
            let pending = PendingExplicit {
                receipt_id: receipt_id.clone(),
                call_id,
                kind,
                claim,
                why_useful,
                staleness,
                proposal: proposal.clone(),
            };
            let mut queue = self.state.pending.lock().map_err(|_| {
                ToolError::ExecutionFailed("subjektiv pending queue poisoned".into())
            })?;
            match queue.get(&receipt_id) {
                Some(existing) if !pending_equivalent(existing, &pending) => {
                    return Err(ToolError::InvalidArgument(
                        "this tool operation was retried with different content".into(),
                    ));
                }
                Some(_) => {}
                None => {
                    queue.insert(receipt_id.clone(), pending);
                }
            }
            let value = serde_json::json!({
                "receipt_id": receipt_id,
                "status": "pending_commit",
                "target": proposal.as_ref().map(|value| serde_json::json!({
                    "memory_id": value.memory_id,
                    "expected_revision": value.expected_revision,
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
        let response = self.state.stage(
            &receipt_id,
            &capture.session_id,
            kind,
            claim,
            why_useful,
            staleness,
            entries,
            proposal,
        )?;
        staged_output(response)
    }
}

impl SubjektivMemoryState {
    fn execute(
        &self,
        operation: SubjektivMemoryBackendOperation,
    ) -> Result<SubjektivMemoryBackendResponse, ToolError> {
        let response = self
            .client
            .execute_server_operation(WorkspaceServerOperation::SubjektivMemory(
                SubjektivMemoryBackendRequest { operation },
            ))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            let message = format!(
                "subjektiv Memory backend returned HTTP {}: {}",
                response.status, response.body
            );
            return if matches!(response.status, 400 | 404 | 409 | 422) {
                Err(ToolError::InvalidArgument(message))
            } else {
                Err(ToolError::ExecutionFailed(message))
            };
        }
        serde_json::from_str(&response.body).map_err(|error| {
            ToolError::ExecutionFailed(format!("decode subjektiv response: {error}"))
        })
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
        proposal: Option<SubjektivMemoryRevisionProposal>,
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
        let capture =
            self.state.capture.capture().map_err(|error| {
                HookError::new(HookErrorCategory::Dependency, error.to_string())
            })?;
        let view = SessionCapture::from_history_entries(
            capture.segment_id.clone(),
            capture.history.clone(),
        );
        let pending = self
            .state
            .pending
            .lock()
            .map_err(|_| HookError::new(HookErrorCategory::Internal, "pending queue poisoned"))?
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut completed = Vec::new();
        let mut failures = Vec::new();
        for item in pending {
            let Some(evidence) = view.evidence_for_tool_call(&item.call_id) else {
                // The operation belongs to an uncommitted or different Session.
                continue;
            };
            match self.state.stage(
                &item.receipt_id,
                &capture.session_id,
                item.kind.clone(),
                item.claim,
                item.why_useful,
                item.staleness,
                vec![evidence],
                item.proposal,
            ) {
                Ok(_) => completed.push(item.receipt_id),
                Err(error) => failures.push(error.to_string()),
            }
        }
        if !completed.is_empty() {
            let mut queue = self.state.pending.lock().map_err(|_| {
                HookError::new(HookErrorCategory::Internal, "pending queue poisoned")
            })?;
            for receipt_id in completed {
                queue.remove(&receipt_id);
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

fn pending_equivalent(left: &PendingExplicit, right: &PendingExplicit) -> bool {
    left.receipt_id == right.receipt_id
        && left.call_id == right.call_id
        && left.kind == right.kind
        && left.claim == right.claim
        && left.why_useful == right.why_useful
        && left.staleness == right.staleness
        && left.proposal == right.proposal
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
            _request: WorkspaceRequest,
        ) -> Result<WorkspaceResponse, WorkspaceClientError> {
            Err(WorkspaceClientError::Request(
                "test client has no backend".into(),
            ))
        }
    }

    #[derive(Debug, Default)]
    struct RecordingWorkspaceClient {
        requests: Mutex<Vec<WorkspaceRequest>>,
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
            self.requests.lock().unwrap().push(request);
            Ok(WorkspaceResponse {
                status: 200,
                body: serde_json::to_string(&SubjektivMemoryBackendResponse::Staged(
                    SubjektivMemoryStageExplicitResponse {
                        candidate_id: "explicit:session-1:call-commit".into(),
                        receipt_id: "explicit:session-1:call-commit".into(),
                        status: SubjektivMemoryReceiptStatus::Staged,
                        target: None,
                        intent: None,
                    },
                ))
                .unwrap(),
            })
        }
    }

    fn feature() -> SubjektivMemoryFeature {
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),
                session_revision: 1,
                entry_count: 0,
                run_exit: CommittedRunExit::Finished,
                history: Vec::new(),
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        SubjektivMemoryFeature {
            state: SubjektivMemoryState {
                client: Arc::new(TestWorkspaceClient),
                capture,
                pending: Arc::new(Mutex::new(HashMap::new())),
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
                LIST_REVISIONS_TOOL,
                REMEMBER_TOOL,
                PROPOSE_REVISION_TOOL,
            ]
        );
        assert_eq!(descriptor.hooks.len(), 1);
        assert_eq!(descriptor.hooks[0].name, COMMIT_HOOK);
        assert_eq!(descriptor.hooks[0].point, FeatureHookPoint::RunCommitted);
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

        let propose = schema_for::<ProposeRevisionParams>();
        let propose_text = serde_json::to_string(&propose).unwrap();
        assert!(!propose_text.contains("subject_id"));
        assert!(!propose_text.contains("\"kind\""));
        assert!(propose_text.contains("expected_revision"));
        assert!(propose_text.contains("change_reason"));

        let parsed: RememberParams = serde_json::from_str(
            r#"{"kind":"lesson","claim":"Remember this","why_useful":"Later recall"}"#,
        )
        .unwrap();
        assert!(parsed.entry_refs.is_empty());
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
            r#"{"kind":"lesson","claim":"Use fixed revisions","why_useful":"Avoid mixed reads"}"#;
        let output = tool
            .execute(input, ToolExecutionContext::new("call-1", "batch-1", 0))
            .await
            .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(value["status"], "pending_commit");
        assert_eq!(value["receipt_id"], "explicit:session-1:call-1");
        assert_eq!(feature.state.pending.lock().unwrap().len(), 1);

        tool.execute(input, ToolExecutionContext::new("call-1", "batch-1", 0))
            .await
            .unwrap();
        assert_eq!(feature.state.pending.lock().unwrap().len(), 1);

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
    async fn committed_tool_call_stages_pending_candidate_with_host_evidence() {
        let client = Arc::new(RecordingWorkspaceClient::default());
        let capture = CommittedSessionCaptureHandle::new(|| {
            Ok(CommittedSessionCapture {
                session_id: "session-1".into(),
                segment_id: "segment-1".into(),
                session_revision: 2,
                entry_count: 1,
                run_exit: CommittedRunExit::Finished,
                history: vec![crate::session_history::history_entry(
                    agen::Item::tool_call(
                        "call-commit",
                        REMEMBER_TOOL,
                        r#"{"kind":"lesson","claim":"Committed","why_useful":"Evidence"}"#,
                    ),
                    crate::session_history::WorkerHistoryProvenance::ModelOutput {
                        worker: crate::session_history::worker_subject(Default::default()),
                    },
                )],
                usage_history: Vec::new(),
                extensions: Vec::new(),
            })
        });
        let state = SubjektivMemoryState {
            client: client.clone(),
            capture,
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        let tool = SubjektivExplicitTool {
            state: state.clone(),
            operation: ExplicitOperation::Remember,
        };
        tool.execute(
            r#"{"kind":"lesson","claim":"Committed","why_useful":"Evidence"}"#,
            ToolExecutionContext::new("call-commit", "batch-1", 0),
        )
        .await
        .unwrap();
        assert_eq!(state.pending.lock().unwrap().len(), 1);

        PendingCommitHook {
            state: state.clone(),
        }
        .call(&RunCommittedContext {
            invocation: crate::hook::HookInvocationContext::default(),
            exit: crate::hook::RunCommittedExit::Finished,
            committed_history: crate::hook::HookHistoryRange::default(),
        })
        .await
        .unwrap();
        assert!(state.pending.lock().unwrap().is_empty());
        let requests = client.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let body = requests[0].body.as_deref().unwrap();
        let request: SubjektivMemoryBackendRequest = serde_json::from_str(body).unwrap();
        let SubjektivMemoryBackendOperation::StageExplicit(staged) = request.operation else {
            panic!("expected explicit stage request");
        };
        assert_eq!(staged.session_id, "session-1");
        assert_eq!(staged.evidence.len(), 1);
        assert_eq!(
            staged.evidence[0].origin.as_ref().unwrap().kind,
            memory::schema::EvidenceOriginKind::ModelOutput
        );
        assert_eq!(
            staged.source_refs[0].evidence_id.as_deref(),
            Some(staged.evidence[0].id.as_str())
        );
    }
}
