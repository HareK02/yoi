use crate::feature::ToolDefinition;
use crate::worker::{WorkspaceClient, WorkspaceRequest, WorkspaceRequestMethod};
use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolError, ToolExecutionContext, ToolMeta, ToolOutput};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

pub const MERGE_REQUEST_COMMON_TOOL_NAMES: &[&str] = &[
    "MergeRequestShow",
    "MergeRequestReadinessCheck",
    "MergeRequestOpen",
    "MergeRequestAddRevision",
    "MergeRequestRecordMergeResult",
    "MergeRequestComplete",
];
pub const MERGE_REQUEST_REVIEW_TOOL_NAME: &str = "MergeRequestReviewSubmit";
#[derive(Clone, Copy)]
enum Kind {
    Show,
    Readiness,
    Open,
    AddRevision,
    RecordMergeResult,
    Complete,
    Review,
}
#[derive(Clone)]
struct MergeRequestTool {
    client: Arc<dyn WorkspaceClient>,
    kind: Kind,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ShowInput {
    ticket: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct OpenInput {
    ticket: String,
    repository_id: String,
    revision_id: String,
    base_commit: String,
    head_commit: String,
    diff_digest: String,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    summary: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct AddRevisionInput {
    ticket: String,
    expected_current_revision_id: String,
    revision_id: String,
    base_commit: String,
    head_commit: String,
    diff_digest: String,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    summary: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct RecordMergeResultInput {
    ticket: String,
    expected_current_revision_id: String,
    operation_id: String,
    target_commit: String,
    source_commit: String,
    result_commit: String,
    strategy: MergeStrategyInput,
    resolution: MergeResolutionInput,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MergeStrategyInput {
    FastForward,
    Merge,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MergeResolutionInput {
    None,
    Clean,
    ConflictsResolved,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteInput {
    ticket: String,
    operation_id: String,
    expected_revision_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ReviewInput {
    decision: ReviewDecisionInput,
    #[serde(default)]
    body: String,
    #[serde(default)]
    findings: Vec<ReviewFindingInput>,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReviewDecisionInput {
    Approve,
    RequestChanges,
}
#[derive(Debug, Deserialize, JsonSchema)]
struct ReviewFindingInput {
    severity: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u64>,
    body: String,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "MergeRequestShow",
            Self::Readiness => "MergeRequestReadinessCheck",
            Self::Open => "MergeRequestOpen",
            Self::AddRevision => "MergeRequestAddRevision",
            Self::RecordMergeResult => "MergeRequestRecordMergeResult",
            Self::Complete => "MergeRequestComplete",
            Self::Review => "MergeRequestReviewSubmit",
        }
    }
    fn description(self) -> &'static str {
        description(self.name()).unwrap_or("Merge Request operation.")
    }
    fn schema(self) -> serde_json::Value {
        match self {
            Self::Show | Self::Readiness => json!(schemars::schema_for!(ShowInput)),
            Self::Open => json!(schemars::schema_for!(OpenInput)),
            Self::AddRevision => json!(schemars::schema_for!(AddRevisionInput)),
            Self::RecordMergeResult => json!(schemars::schema_for!(RecordMergeResultInput)),
            Self::Complete => json!(schemars::schema_for!(CompleteInput)),
            Self::Review => json!(schemars::schema_for!(ReviewInput)),
        }
    }
}

#[async_trait]
impl Tool for MergeRequestTool {
    async fn execute(
        &self,
        input: &str,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let workspace_id = self.client.workspace_id().ok_or_else(|| {
            ToolError::ExecutionFailed("Merge Request tools require Workspace identity".into())
        })?;
        let (method, path, body) = match self.kind {
            Kind::Show => {
                let v: ShowInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!("/api/w/{workspace_id}/tickets/{}/merge-request", v.ticket),
                    None,
                )
            }
            Kind::Readiness => {
                let v: ShowInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/readiness",
                        v.ticket
                    ),
                    None,
                )
            }
            Kind::Open => {
                let v: OpenInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!("/api/w/{workspace_id}/tickets/{}/merge-request", v.ticket),
                    Some(
                        json!({"repository_id":v.repository_id,"revision_id":v.revision_id,"base_commit":v.base_commit,"head_commit":v.head_commit,"diff_digest":v.diff_digest,"changed_paths":v.changed_paths,"summary":v.summary}),
                    ),
                )
            }
            Kind::AddRevision => {
                let v: AddRevisionInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/revisions",
                        v.ticket
                    ),
                    Some(
                        json!({"expected_current_revision_id":v.expected_current_revision_id,"revision_id":v.revision_id,"base_commit":v.base_commit,"head_commit":v.head_commit,"diff_digest":v.diff_digest,"changed_paths":v.changed_paths,"summary":v.summary}),
                    ),
                )
            }
            Kind::RecordMergeResult => {
                let v: RecordMergeResultInput = parse(input)?;
                nonempty(&v.ticket)?;
                let strategy = match v.strategy {
                    MergeStrategyInput::FastForward => "fast_forward",
                    MergeStrategyInput::Merge => "merge",
                };
                let resolution = match v.resolution {
                    MergeResolutionInput::None => "none",
                    MergeResolutionInput::Clean => "clean",
                    MergeResolutionInput::ConflictsResolved => "conflicts_resolved",
                };
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/merge-results",
                        v.ticket
                    ),
                    Some(
                        json!({"expected_current_revision_id":v.expected_current_revision_id,"operation_id":v.operation_id,"target_commit":v.target_commit,"source_commit":v.source_commit,"result_commit":v.result_commit,"strategy":strategy,"resolution":resolution}),
                    ),
                )
            }
            Kind::Complete => {
                let v: CompleteInput = parse(input)?;
                nonempty(&v.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/complete",
                        v.ticket
                    ),
                    Some(
                        json!({"operation_id":v.operation_id,"expected_revision_id":v.expected_revision_id}),
                    ),
                )
            }
            Kind::Review => {
                let v: ReviewInput = parse(input)?;
                let context = self.client.reviewer_attempt_context().ok_or_else(|| {
                    ToolError::ExecutionFailed(
                        "MergeRequestReviewSubmit is available only to an attested Reviewer child"
                            .into(),
                    )
                })?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/reviews",
                        context.ticket_id
                    ),
                    Some(
                        json!({"decision":match v.decision{ReviewDecisionInput::Approve=>"approve",ReviewDecisionInput::RequestChanges=>"request_changes"},"body":v.body,"findings":v.findings.into_iter().map(|f|json!({"severity":f.severity,"code":f.code,"path":f.path,"line":f.line,"body":f.body})).collect::<Vec<_>>() }),
                    ),
                )
            }
        };
        let request = match body {
            Some(body) => WorkspaceRequest::json(method, path, body.to_string()),
            None => WorkspaceRequest::get(path),
        };
        let response = self
            .client
            .execute(request)
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
        if !response.is_success() {
            return Err(ToolError::ExecutionFailed(format!(
                "Merge Request API returned HTTP {}: {}",
                response.status, response.body
            )));
        }
        Ok(ToolOutput {
            summary: self.kind.name().to_string(),
            content: Some(response.body),
            attachments: Vec::new(),
        })
    }
}
fn parse<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, ToolError> {
    serde_json::from_str(value).map_err(|e| ToolError::InvalidArgument(e.to_string()))
}
fn nonempty(value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        Err(ToolError::InvalidArgument(
            "ticket must not be empty".into(),
        ))
    } else {
        Ok(())
    }
}
fn definition(client: Arc<dyn WorkspaceClient>, kind: Kind) -> ToolDefinition {
    Arc::new(move || {
        let meta = ToolMeta::new(kind.name())
            .description(kind.description())
            .input_schema(kind.schema());
        let tool: Arc<dyn Tool> = Arc::new(MergeRequestTool {
            client: client.clone(),
            kind,
        });
        (meta, tool)
    })
}
pub fn common_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    vec![
        definition(client.clone(), Kind::Show),
        definition(client.clone(), Kind::Readiness),
        definition(client.clone(), Kind::Open),
        definition(client.clone(), Kind::AddRevision),
        definition(client.clone(), Kind::RecordMergeResult),
        definition(client, Kind::Complete),
    ]
}
pub fn reviewer_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    if client.reviewer_attempt_context().is_some() {
        vec![
            definition(client.clone(), Kind::Show),
            definition(client, Kind::Review),
        ]
    } else {
        Vec::new()
    }
}
pub fn description(name: &str) -> Option<&'static str> {
    match name {
        "MergeRequestShow" => Some(
            "Read the authoritative Merge Request, immutable current revision, and structured review status.",
        ),
        "MergeRequestReadinessCheck" => {
            Some("Check derived merge readiness for the current immutable revision.")
        }
        "MergeRequestOpen" => {
            Some("Open an immutable Merge Request revision for the current assigned Coder.")
        }
        "MergeRequestAddRevision" => {
            Some("Append an immutable revision; prior approval cannot carry to the new revision.")
        }
        "MergeRequestRecordMergeResult" => Some(
            "Record validated immutable integration evidence for the current source revision and target tip.",
        ),
        "MergeRequestComplete" => {
            Some("CAS-complete an approved revision with operation-id replay and crash fencing.")
        }
        "MergeRequestReviewSubmit" => Some(
            "Submit the attested direct-child Reviewer result bound to its immutable revision.",
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_request_tool_contract_omits_tree_hashes_and_exposes_merge_result() {
        let open = serde_json::to_string(&schemars::schema_for!(OpenInput)).unwrap();
        let add = serde_json::to_string(&schemars::schema_for!(AddRevisionInput)).unwrap();
        let result = serde_json::to_string(&schemars::schema_for!(RecordMergeResultInput)).unwrap();
        assert!(!open.contains("head_tree"));
        assert!(!add.contains("head_tree"));
        assert!(result.contains("fast_forward"));
        assert!(result.contains("conflicts_resolved"));
        assert!(MERGE_REQUEST_COMMON_TOOL_NAMES.contains(&"MergeRequestRecordMergeResult"));
    }
}
