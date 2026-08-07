use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use memory::backend::{
    MemoryBackendOperation, MemoryBackendOperationResult, MemoryStageCandidateOperation,
};
use memory::extract::{CandidateKind, ExtractedCandidate, StagingEvidence};
use memory::schema::{EvidenceKind, SourceEvidenceRef, SourceRef};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::session_capture::{
    ReferenceKind, SearchOptions, SessionCapture, SessionEntryEvidence, ToolPart,
};
use crate::worker::WorkspaceClient;

use super::memory::WorkspaceMemoryBackendError;

const STAGE_DESCRIPTION: &str = "Stage one durable Memory candidate using SessionEntryRef values from the co-installed session-explore capture.";
const FINISH_DESCRIPTION: &str =
    "Finish Memory extraction after validating the number of candidates staged during this run.";

#[derive(Clone)]
pub(crate) struct MemoryExtractState {
    view: Arc<SessionCapture>,
    workspace_client: Arc<dyn WorkspaceClient>,
    source: SourceRef,
    extract_run_id: String,
    staged: Arc<Mutex<Vec<String>>>,
    finished: Arc<Mutex<Option<FinishMemoryExtractionParams>>>,
}

impl MemoryExtractState {
    pub(crate) fn new(
        view: SessionCapture,
        workspace_client: Arc<dyn WorkspaceClient>,
        source: SourceRef,
        extract_run_id: String,
    ) -> Self {
        Self {
            view: Arc::new(view),
            workspace_client,
            source,
            extract_run_id,
            staged: Arc::new(Mutex::new(Vec::new())),
            finished: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn staged(&self) -> Vec<String> {
        self.staged
            .lock()
            .expect("memory extract staged state poisoned")
            .clone()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
            .lock()
            .expect("memory extract finished state poisoned")
            .is_some()
    }
}

#[derive(Clone)]
pub(crate) struct MemoryExtractFeature {
    state: MemoryExtractState,
}

impl MemoryExtractFeature {
    pub(crate) fn new(state: MemoryExtractState) -> Self {
        Self { state }
    }
}

impl FeatureModule for MemoryExtractFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("memory-extract", "Memory Extract")
            .with_description(
                "Memory staging and extraction completion, independent from session exploration.",
            )
            .with_tool(ToolDeclaration::new(
                "StageMemoryCandidate",
                STAGE_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(
                "FinishMemoryExtraction",
                FINISH_DESCRIPTION,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            "StageMemoryCandidate",
            stage_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "FinishMemoryExtraction",
            finish_definition(self.state.clone()),
        ))?;
        Ok(())
    }
}

fn stage_definition(state: MemoryExtractState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(StageMemoryCandidateParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("StageMemoryCandidate")
            .description(STAGE_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(StageMemoryCandidateTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn finish_definition(state: MemoryExtractState) -> ToolDefinition {
    Arc::new(move || {
        let schema = serde_json::to_value(schemars::schema_for!(FinishMemoryExtractionParams))
            .unwrap_or_else(|_| serde_json::json!({}));
        let meta = ToolMeta::new("FinishMemoryExtraction")
            .description(FINISH_DESCRIPTION)
            .input_schema(schema);
        let tool: Arc<dyn Tool> = Arc::new(FinishMemoryExtractionTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StageMemoryCandidateParams {
    kind: CandidateKind,
    claim: String,
    why_useful: String,
    #[serde(default)]
    staleness: Option<String>,
    entry_refs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishMemoryExtractionParams {
    staged_count: usize,
    #[serde(default)]
    no_candidates_reason: Option<String>,
}

struct StageMemoryCandidateTool {
    state: MemoryExtractState,
}

#[async_trait]
impl Tool for StageMemoryCandidateTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: StageMemoryCandidateParams =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid StageMemoryCandidate input: {error}"))
            })?;
        if params.entry_refs.is_empty() {
            return Err(ToolError::InvalidArgument(
                "StageMemoryCandidate requires at least one entry_ref".to_string(),
            ));
        }
        let mut evidence = Vec::with_capacity(params.entry_refs.len());
        let mut source_refs = Vec::with_capacity(params.entry_refs.len());
        for entry_ref in &params.entry_refs {
            let projection = self.state.view.evidence_for(entry_ref).ok_or_else(|| {
                ToolError::InvalidArgument(format!(
                    "unknown SessionEntryRef {entry_ref:?} for this extraction capture"
                ))
            })?;
            evidence.push(staging_evidence(&projection));
            source_refs.push(source_evidence_ref(&projection));
        }
        let candidate = ExtractedCandidate {
            kind: params.kind,
            claim: params.claim,
            why_useful: params.why_useful,
            staleness: params.staleness,
            evidence_ids: params.entry_refs,
        };
        let result = self
            .state
            .workspace_client
            .execute_memory_backend_operation(MemoryBackendOperation::StageCandidate(
                MemoryStageCandidateOperation {
                    source: self.state.source.clone(),
                    extract_run_id: self.state.extract_run_id.clone(),
                    candidate,
                    evidence,
                    source_refs,
                },
            ))
            .await
            .map_err(map_memory_stage_error)?;
        let staging_ids = match result {
            MemoryBackendOperationResult::StagingWritten(output) if output.staging_count == 1 => {
                output.staging_ids
            }
            MemoryBackendOperationResult::StagingWritten(output) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "StageMemoryCandidate expected one staging record, backend wrote {}",
                    output.staging_count
                )));
            }
            other => {
                return Err(ToolError::ExecutionFailed(format!(
                    "unexpected Memory backend result for StageMemoryCandidate: {other:?}"
                )));
            }
        };
        let staging_id = staging_ids.into_iter().next().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "StageMemoryCandidate backend did not return a staging id".to_string(),
            )
        })?;
        self.state
            .staged
            .lock()
            .expect("memory extract staged state poisoned")
            .push(staging_id.clone());
        Ok(ToolOutput {
            summary: format!("Staged Memory candidate {staging_id}."),
            content: Some(format!("staging_id: {staging_id}")),
        })
    }
}

struct FinishMemoryExtractionTool {
    state: MemoryExtractState,
}

#[async_trait]
impl Tool for FinishMemoryExtractionTool {
    async fn execute(
        &self,
        input_json: &str,
        _context: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: FinishMemoryExtractionParams =
            serde_json::from_str(input_json).map_err(|error| {
                ToolError::InvalidArgument(format!("invalid FinishMemoryExtraction input: {error}"))
            })?;
        let actual = self
            .state
            .staged
            .lock()
            .expect("memory extract staged state poisoned")
            .len();
        if params.staged_count != actual {
            return Err(ToolError::InvalidArgument(format!(
                "FinishMemoryExtraction staged_count {} does not match actual staged count {actual}",
                params.staged_count
            )));
        }
        let reason = params.no_candidates_reason.clone();
        *self
            .state
            .finished
            .lock()
            .expect("memory extract finished state poisoned") = Some(params);
        Ok(ToolOutput {
            summary: reason
                .map(|reason| {
                    format!("Finished extraction with {actual} staged candidate(s): {reason}")
                })
                .unwrap_or_else(|| {
                    format!("Finished extraction with {actual} staged candidate(s).")
                }),
            content: None,
        })
    }
}

fn map_memory_stage_error(error: WorkspaceMemoryBackendError) -> ToolError {
    match error {
        WorkspaceMemoryBackendError::Backend(message) => ToolError::InvalidArgument(message),
        WorkspaceMemoryBackendError::Http { status, body }
            if matches!(
                status,
                reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::UNPROCESSABLE_ENTITY
            ) =>
        {
            ToolError::InvalidArgument(body)
        }
        error => ToolError::ExecutionFailed(format!("write Memory staging failed: {error}")),
    }
}

fn evidence_kind(entry: &SessionEntryEvidence) -> EvidenceKind {
    match (entry.kind, entry.tool_part) {
        (ReferenceKind::Tool, Some(ToolPart::Input)) => EvidenceKind::new(EvidenceKind::TOOL_CALL),
        (ReferenceKind::Tool, _) => EvidenceKind::new(EvidenceKind::TOOL_RESULT),
        _ => EvidenceKind::new(EvidenceKind::MESSAGE),
    }
}

fn staging_evidence(entry: &SessionEntryEvidence) -> StagingEvidence {
    StagingEvidence {
        id: entry.entry_ref.to_string(),
        kind: evidence_kind(entry),
        entry_range: Some(entry.entry_range),
        excerpt: Some(entry.excerpt.clone()),
        summary: Some(entry.summary.clone()),
    }
}

fn source_evidence_ref(entry: &SessionEntryEvidence) -> SourceEvidenceRef {
    SourceEvidenceRef {
        segment_id: Some(entry.segment_id.clone()),
        entry_range: Some(entry.entry_range),
        evidence_id: Some(entry.entry_ref.to_string()),
        evidence_kind: Some(evidence_kind(entry)),
        label: Some(entry.label.clone()),
        summary: Some(entry.summary.clone()),
        ..Default::default()
    }
}

pub(crate) fn render_extract_input(view: &SessionCapture) -> String {
    let mut output = String::from("# Session overview\n\n");
    if view.overview().is_empty() {
        output.push_str("No user/assistant overview entries are available.\n\n");
    } else {
        for item in view.overview() {
            output.push_str(&format!(
                "- [{} {}] {}\n  {}\n  intervening_entries: {}\n",
                item.id,
                item.kind.as_str(),
                item.label,
                truncate_line(&item.text, 500),
                item.intervening_entries,
            ));
        }
        output.push('\n');
    }
    output.push_str("# Initial session entry index\n\n");
    output.push_str("Use ShowOverview, SearchEntries, and ReadEntry to inspect details. Cite only SessionEntryRef values in StageMemoryCandidate.entry_refs.\n\n");
    let hits = view.search(&SearchOptions {
        query: String::new(),
        kind: None,
        tool_part: None,
        tool_name: None,
        limit: Some(50),
        min_entry_index: None,
        from: None,
        through: None,
        offset: 0,
    });
    for hit in hits {
        output.push_str(&format!(
            "- [{} {}] {} — {}\n",
            hit.id,
            hit.kind.as_str(),
            hit.label,
            hit.summary
        ));
    }
    output
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let normalized = text.replace('\n', " ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        let mut output = normalized.chars().take(max_chars).collect::<String>();
        output.push('…');
        output
    }
}

#[cfg(test)]
mod tests {
    use llm_engine::Item;

    use super::*;

    fn state() -> MemoryExtractState {
        MemoryExtractState::new(
            SessionCapture::new("segment-1", vec![Item::user_message("durable decision")]),
            crate::worker::marker_workspace_client(None, "test-backend"),
            SourceRef {
                segment_id: "segment-1".to_string(),
                range: [0, 0],
            },
            "run-1".to_string(),
        )
    }

    #[test]
    fn memory_extract_declares_only_memory_mutation_tools() {
        let descriptor = MemoryExtractFeature::new(state()).descriptor();
        assert_eq!(descriptor.id.as_str(), "builtin:memory-extract");
        assert_eq!(
            descriptor
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["StageMemoryCandidate", "FinishMemoryExtraction"]
        );
    }

    #[test]
    fn render_input_uses_session_entry_refs_and_new_tool_names() {
        let view = SessionCapture::new(
            "segment-1",
            vec![
                Item::user_message("preference"),
                Item::tool_call("call-1", "Read", "{}"),
            ],
        );
        let input = render_extract_input(&view);
        assert!(input.contains("E00000000"));
        assert!(input.contains("E00000001"));
        assert!(input.contains("StageMemoryCandidate.entry_refs"));
    }

    #[test]
    fn backend_input_failures_remain_invalid_argument_tool_errors() {
        let backend = map_memory_stage_error(WorkspaceMemoryBackendError::Backend(
            "invalid candidate".to_string(),
        ));
        assert!(matches!(backend, ToolError::InvalidArgument(_)));
        let http = map_memory_stage_error(WorkspaceMemoryBackendError::Http {
            status: reqwest::StatusCode::UNPROCESSABLE_ENTITY,
            body: "invalid candidate".to_string(),
        });
        assert!(matches!(http, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn stage_rejects_entry_ref_outside_capture_before_backend_mutation() {
        let tool = StageMemoryCandidateTool { state: state() };
        let error = tool
            .execute(
                r#"{"kind":"decision","claim":"claim","why_useful":"useful","entry_refs":["E00000009"]}"#,
                llm_engine::tool::ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(format!("{error:?}").contains("unknown SessionEntryRef"));
    }
}
