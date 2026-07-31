use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use memory::backend::{
    MemoryBackendOperation, MemoryBackendOperationResult, MemoryStageCandidateOperation,
};
use memory::extract::{CandidateKind, ExtractedCandidate};
use memory::schema::SourceRef;
use schemars::JsonSchema;
use serde::Deserialize;
use uuid::Uuid;

use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule, ToolContribution,
    ToolDeclaration,
};
use crate::session_reference::{
    ReadDetail, ReadOptions, ReadSelector, ReferenceKind, SearchOptions, SessionReferenceView,
    ToolPart,
};
use crate::worker::WorkspaceClient;

const SEARCH_EVIDENCE_DESCRIPTION: &str = "Search the host-created session evidence index. Use this to find stable evidence ids before staging a memory candidate. Supports kind=user|assistant|system|tool and tool_part=input|output|both.";
const READ_EVIDENCE_DESCRIPTION: &str = "Read bounded session evidence by evidence_id or entry_range. Use compact mode for normal verification and full mode only when exact tool arguments or result content are necessary.";
const STAGE_CANDIDATE_DESCRIPTION: &str = "Stage one memory candidate as a flat staging record. The candidate must cite one or more evidence_ids returned by search_evidence/read_evidence.";
const FINISH_EXTRACTION_DESCRIPTION: &str = "Finish the extract worker run after all useful candidates have been staged, or state why no candidates were useful.";

#[derive(Clone)]
pub(crate) struct SessionExploreState {
    view: Arc<SessionReferenceView>,
    workspace_client: Arc<dyn WorkspaceClient>,
    source: SourceRef,
    extract_run_id: String,
    staged: Arc<Mutex<Vec<String>>>,
    finished: Arc<Mutex<Option<FinishExtractionParams>>>,
}

impl SessionExploreState {
    pub(crate) fn new(
        view: SessionReferenceView,
        workspace_client: Arc<dyn WorkspaceClient>,
        source: SourceRef,
    ) -> Self {
        Self {
            view: Arc::new(view),
            workspace_client,
            source,
            extract_run_id: Uuid::now_v7().to_string(),
            staged: Arc::new(Mutex::new(Vec::new())),
            finished: Arc::new(Mutex::new(None)),
        }
    }

    pub(crate) fn view(&self) -> &SessionReferenceView {
        &self.view
    }

    pub(crate) fn staged(&self) -> Vec<String> {
        self.staged
            .lock()
            .expect("session explore staged state poisoned")
            .clone()
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
            .lock()
            .expect("session explore finished state poisoned")
            .is_some()
    }
}

#[derive(Clone)]
pub(crate) struct SessionExploreFeature {
    state: SessionExploreState,
}

impl SessionExploreFeature {
    pub(crate) fn new(state: SessionExploreState) -> Self {
        Self { state }
    }
}

impl FeatureModule for SessionExploreFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("session-explore", "Session Explore")
            .with_description(
                "Host-bounded session evidence search/read tools plus explicit extraction staging.",
            )
            .with_tool(ToolDeclaration::new(
                "search_evidence",
                SEARCH_EVIDENCE_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(
                "read_evidence",
                READ_EVIDENCE_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(
                "stage_candidate",
                STAGE_CANDIDATE_DESCRIPTION,
            ))
            .with_tool(ToolDeclaration::new(
                "finish_extraction",
                FINISH_EXTRACTION_DESCRIPTION,
            ))
    }

    fn install(&self, context: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        context.tools().register(ToolContribution::new(
            "search_evidence",
            search_evidence_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "read_evidence",
            read_evidence_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "stage_candidate",
            stage_candidate_definition(self.state.clone()),
        ))?;
        context.tools().register(ToolContribution::new(
            "finish_extraction",
            finish_extraction_definition(self.state.clone()),
        ))?;
        Ok(())
    }
}

fn search_evidence_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(SearchEvidenceParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("search_evidence")
            .description(SEARCH_EVIDENCE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(SearchEvidenceTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn read_evidence_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ReadEvidenceParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("read_evidence")
            .description(READ_EVIDENCE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ReadEvidenceTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn stage_candidate_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(StageCandidateParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("stage_candidate")
            .description(STAGE_CANDIDATE_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(StageCandidateTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

fn finish_extraction_definition(state: SessionExploreState) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(FinishExtractionParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("finish_extraction")
            .description(FINISH_EXTRACTION_DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(FinishExtractionTool {
            state: state.clone(),
        });
        (meta, tool)
    })
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SearchEvidenceParams {
    /// Case-insensitive substring. Empty query lists bounded index entries matching filters.
    #[serde(default)]
    query: String,
    /// Optional evidence kind: user, assistant, system, tool.
    #[serde(default)]
    kind: Option<String>,
    /// Optional tool part filter for tool evidence: input, output, both.
    #[serde(default)]
    tool_part: Option<String>,
    /// Optional tool name filter for tool input entries.
    #[serde(default)]
    tool_name: Option<String>,
    /// 0-based session item offset to start from.
    #[serde(default)]
    offset: Option<usize>,
    /// Maximum number of hits.
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReadEvidenceParams {
    /// Evidence id returned by search_evidence, e.g. M0001, T0002i, T0003o.
    #[serde(default)]
    evidence_id: Option<String>,
    /// Inclusive 0-based session item range. Use when search returned an entry_range instead of a single id.
    #[serde(default)]
    entry_range: Option<[u64; 2]>,
    /// compact omits tool arguments/results; full includes them within host bounds.
    #[serde(default = "default_read_mode")]
    mode: String,
    /// Maximum entries to return.
    #[serde(default)]
    max_items: Option<usize>,
}

fn default_read_mode() -> String {
    "compact".to_string()
}

#[derive(Debug, Deserialize, JsonSchema)]
struct StageCandidateParams {
    kind: CandidateKind,
    claim: String,
    why_useful: String,
    #[serde(default)]
    staleness: Option<String>,
    #[serde(default)]
    evidence_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct FinishExtractionParams {
    staged_count: usize,
    #[serde(default)]
    no_candidates_reason: Option<String>,
}

struct SearchEvidenceTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for SearchEvidenceTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: SearchEvidenceParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid search_evidence input: {e}"))
        })?;
        let kind = params
            .kind
            .as_deref()
            .map(parse_reference_kind)
            .transpose()?;
        let tool_part = params
            .tool_part
            .as_deref()
            .map(parse_tool_part)
            .transpose()?;
        let hits = self.state.view.search(&SearchOptions {
            query: params.query,
            kind,
            tool_part,
            tool_name: params.tool_name,
            limit: params.limit,
            min_entry_index: params.offset.map(|offset| offset as u64),
        });
        let content = hits
            .iter()
            .map(|hit| {
                let part = hit
                    .tool_part
                    .map(|part| format!(" {part:?}"))
                    .unwrap_or_default();
                let tool = hit
                    .tool_name
                    .as_ref()
                    .map(|name| format!(" {name}"))
                    .unwrap_or_default();
                format!(
                    "[{} {}{}{} {:?}] {}\n{}",
                    hit.id,
                    hit.kind.as_str(),
                    part,
                    tool,
                    hit.entry_range,
                    hit.label,
                    hit.summary
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(ToolOutput {
            summary: format!("Found {} evidence hit(s).", hits.len()),
            content: (!content.is_empty()).then_some(content),
        })
    }
}

struct ReadEvidenceTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for ReadEvidenceTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: ReadEvidenceParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid read_evidence input: {e}")))?;
        let selector = match (params.evidence_id.as_deref(), params.entry_range) {
            (Some(id), None) => ReadSelector::Id(id),
            (None, Some(range)) => ReadSelector::EntryRange(range),
            _ => {
                return Err(ToolError::InvalidArgument(
                    "read_evidence requires exactly one of evidence_id or entry_range".to_string(),
                ));
            }
        };
        let detail = parse_read_detail(&params.mode)?;
        let read = self.state.view.read(
            selector,
            ReadOptions {
                include_tools: true,
                tool_part: ToolPart::Both,
                detail,
                max_items: params.max_items.unwrap_or(10),
                max_bytes: 16 * 1024,
            },
        );
        let content = read
            .entries
            .iter()
            .map(|entry| {
                let part = entry
                    .tool_part
                    .map(|part| format!(" {part:?}"))
                    .unwrap_or_default();
                let tool = entry
                    .tool_name
                    .as_ref()
                    .map(|name| format!(" {name}"))
                    .unwrap_or_default();
                format!(
                    "[{} {}{}{} {:?}] {}\n{}",
                    entry.id,
                    entry.kind.as_str(),
                    part,
                    tool,
                    entry.entry_range,
                    entry.label,
                    entry.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let summary = if read.truncated {
            format!(
                "Read {} evidence entrie(s); output truncated.",
                read.entries.len()
            )
        } else {
            format!("Read {} evidence entrie(s).", read.entries.len())
        };
        Ok(ToolOutput {
            summary,
            content: (!content.is_empty()).then_some(content),
        })
    }
}

struct StageCandidateTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for StageCandidateTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: StageCandidateParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid stage_candidate input: {e}"))
        })?;
        if params.evidence_ids.is_empty() {
            return Err(ToolError::InvalidArgument(
                "stage_candidate requires at least one evidence_id".to_string(),
            ));
        }
        let mut evidence = Vec::with_capacity(params.evidence_ids.len());
        let mut source_refs = Vec::with_capacity(params.evidence_ids.len());
        for id in &params.evidence_ids {
            let staging =
                self.state.view.staging_evidence_for(id).ok_or_else(|| {
                    ToolError::InvalidArgument(format!("unknown evidence_id {id:?}"))
                })?;
            let source_ref =
                self.state.view.source_ref_for(id).ok_or_else(|| {
                    ToolError::InvalidArgument(format!("unknown evidence_id {id:?}"))
                })?;
            evidence.push(staging);
            source_refs.push(source_ref);
        }
        let candidate = ExtractedCandidate {
            kind: params.kind,
            claim: params.claim,
            why_useful: params.why_useful,
            staleness: params.staleness,
            evidence_ids: params.evidence_ids,
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
            .map_err(|e| ToolError::ExecutionFailed(format!("write staging failed: {e}")))?;
        let ids = match result {
            MemoryBackendOperationResult::StagingWritten(output) if output.staging_count == 1 => {
                output.staging_ids
            }
            MemoryBackendOperationResult::StagingWritten(output) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "stage_candidate expected one staging record, backend wrote {}",
                    output.staging_count
                )));
            }
            other => {
                return Err(ToolError::ExecutionFailed(format!(
                    "unexpected memory backend result for stage_candidate: {other:?}"
                )));
            }
        };
        let id = ids.into_iter().next().ok_or_else(|| {
            ToolError::ExecutionFailed(
                "stage_candidate backend did not return a staging id".to_string(),
            )
        })?;
        self.state
            .staged
            .lock()
            .expect("session explore staged state poisoned")
            .push(id.clone());
        Ok(ToolOutput {
            summary: format!("Staged memory candidate {id}."),
            content: Some(format!("staging_id: {id}")),
        })
    }
}

struct FinishExtractionTool {
    state: SessionExploreState,
}

#[async_trait]
impl Tool for FinishExtractionTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: FinishExtractionParams = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid finish_extraction input: {e}"))
        })?;
        let actual = self
            .state
            .staged
            .lock()
            .expect("session explore staged state poisoned")
            .len();
        if params.staged_count != actual {
            return Err(ToolError::InvalidArgument(format!(
                "finish_extraction staged_count {} does not match actual staged count {actual}",
                params.staged_count
            )));
        }
        let reason = params.no_candidates_reason.clone();
        *self
            .state
            .finished
            .lock()
            .expect("session explore finished state poisoned") = Some(params);
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

fn parse_reference_kind(value: &str) -> Result<ReferenceKind, ToolError> {
    ReferenceKind::parse(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "invalid kind {value:?}; expected user, assistant/agent, system, or tool"
        ))
    })
}

fn parse_tool_part(value: &str) -> Result<ToolPart, ToolError> {
    ToolPart::parse(value).ok_or_else(|| {
        ToolError::InvalidArgument(format!(
            "invalid tool_part {value:?}; expected input, output, or both"
        ))
    })
}

fn parse_read_detail(value: &str) -> Result<ReadDetail, ToolError> {
    match value {
        "compact" => Ok(ReadDetail::Compact),
        "full" => Ok(ReadDetail::Full),
        other => Err(ToolError::InvalidArgument(format!(
            "invalid read mode {other:?}; expected compact or full"
        ))),
    }
}

pub(crate) fn render_extract_input(view: &SessionReferenceView) -> String {
    let mut out = String::new();
    out.push_str("# Session overview\n\n");
    if view.overview().is_empty() {
        out.push_str("No user/assistant overview entries are available.\n\n");
    } else {
        for item in view.overview() {
            out.push_str(&format!(
                "- [{} {} {:?}] {}\n  {}\n",
                item.id,
                item.kind.as_str(),
                item.entry_range,
                item.label,
                truncate_line(&item.text, 500)
            ));
        }
        out.push('\n');
    }

    out.push_str("# Initial evidence index\n\n");
    out.push_str(
        "Use search_evidence/read_evidence to inspect details. Cite only M/T evidence ids in stage_candidate.evidence_ids.\n\n",
    );
    let hits = view.search(&SearchOptions {
        query: String::new(),
        kind: None,
        tool_part: None,
        tool_name: None,
        limit: Some(50),
        min_entry_index: None,
    });
    for hit in hits {
        let part = hit
            .tool_part
            .map(|part| format!(" {part:?}"))
            .unwrap_or_default();
        let tool = hit
            .tool_name
            .as_ref()
            .map(|name| format!(" {name}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- [{} {}{}{} {:?}] {} — {}\n",
            hit.id,
            hit.kind.as_str(),
            part,
            tool,
            hit.entry_range,
            hit.label,
            hit.summary
        ));
    }
    out
}

fn truncate_line(text: &str, max_chars: usize) -> String {
    let normalized = text.replace('\n', " ");
    if normalized.chars().count() <= max_chars {
        normalized
    } else {
        let mut out = normalized.chars().take(max_chars).collect::<String>();
        out.push_str("…");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_engine::Item;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn stub_memory_backend_response(
        body: &'static str,
    ) -> (Arc<dyn WorkspaceClient>, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = Vec::new();
            let mut temp = [0_u8; 1024];
            let header_end = loop {
                let read = stream.read(&mut temp).unwrap();
                if read == 0 {
                    break buffer.len();
                }
                buffer.extend_from_slice(&temp[..read]);
                if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    break pos + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            while buffer.len() < header_end + content_length {
                let read = stream.read(&mut temp).unwrap();
                if read == 0 {
                    break;
                }
                buffer.extend_from_slice(&temp[..read]);
            }
            let request = String::from_utf8_lossy(&buffer).into_owned();
            tx.send(request).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (
            Arc::new(crate::worker::RuntimeWorkspaceHttpClient::new(
                "test-workspace",
                format!("http://{addr}"),
                "test-worker",
            )),
            rx,
        )
    }

    #[test]
    fn descriptor_declares_session_explore_tools() {
        let state = SessionExploreState::new(
            SessionReferenceView::new("segment-1", vec![Item::user_message("remember this")]),
            crate::worker::marker_workspace_client(None, "test-backend"),
            SourceRef {
                segment_id: "segment-1".to_string(),
                range: [0, 0],
            },
        );
        let descriptor = SessionExploreFeature::new(state).descriptor();
        assert_eq!(descriptor.id.as_str(), "builtin:session-explore");
        let names = descriptor
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "search_evidence",
                "read_evidence",
                "stage_candidate",
                "finish_extraction"
            ]
        );
    }

    #[test]
    fn render_extract_input_exposes_overview_and_evidence_ids() {
        let view = SessionReferenceView::new(
            "segment-1",
            vec![
                Item::user_message("user preference"),
                Item::assistant_message("assistant reply"),
                Item::tool_call("c1", "Read", "{}"),
                Item::tool_result_with_content("c1", "read ok", "file body"),
            ],
        );
        let input = render_extract_input(&view);
        assert!(input.contains("# Session overview"));
        assert!(input.contains("M0000"));
        assert!(input.contains("T0002i"));
        assert!(input.contains("T0003o"));
        assert!(input.contains("stage_candidate.evidence_ids"));
    }

    #[tokio::test]
    async fn stage_candidate_writes_staging_record_with_source_evidence() {
        let (client, request_rx) = stub_memory_backend_response(
            r#"{"status":"ok","result":{"kind":"staging_written","staging_count":1,"staging_ids":["00000000-0000-7000-8000-000000000001"]}}"#,
        );
        let state = SessionExploreState::new(
            SessionReferenceView::new("segment-1", vec![Item::user_message("durable decision")]),
            client,
            SourceRef {
                segment_id: "segment-1".to_string(),
                range: [0, 0],
            },
        );
        let tool = StageCandidateTool {
            state: state.clone(),
        };
        tool.execute(
            &serde_json::json!({
                "kind": "decision",
                "claim": "Use host-created evidence anchors for extract staging.",
                "why_useful": "Future consolidation can trust bounded source refs.",
                "evidence_ids": ["M0000"]
            })
            .to_string(),
            llm_engine::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();

        let staged = state.staged();
        assert_eq!(
            staged,
            vec!["00000000-0000-7000-8000-000000000001".to_string()]
        );
        let request = request_rx.recv().unwrap();
        assert!(request.contains("\"operation\":\"stage_candidate\""));
        assert!(request.contains("\"kind\":\"decision\""));
        assert!(request.contains("\"id\":\"M0000\""));
        assert!(request.contains("\"evidence_id\":\"M0000\""));
    }
}
