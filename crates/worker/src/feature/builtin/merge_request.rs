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
    "MergeRequestRequestReview",
    "MergeRequestComplete",
];
pub const MERGE_REQUEST_REVIEW_TOOL_NAME: &str = "MergeRequestReviewSubmit";

#[derive(Clone, Copy)]
enum Kind {
    Show,
    Readiness,
    Open,
    RequestReview,
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
    selector_from: String,
    selector_to: String,
    base_commit: String,
    head_commit: String,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct RequestReviewInput {
    ticket: String,
    expected_head_commit: String,
    base_commit: String,
    head_commit: String,
    #[serde(default)]
    changed_paths: Vec<String>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct CompleteInput {
    ticket: String,
    operation_id: String,
    expected_head_commit: String,
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
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    message: String,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::Show => "MergeRequestShow",
            Self::Readiness => "MergeRequestReadinessCheck",
            Self::Open => "MergeRequestOpen",
            Self::RequestReview => "MergeRequestRequestReview",
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
            Self::RequestReview => json!(schemars::schema_for!(RequestReviewInput)),
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
                let value: ShowInput = parse(input)?;
                nonempty(&value.ticket)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request",
                        value.ticket
                    ),
                    None,
                )
            }
            Kind::Readiness => {
                let value: ShowInput = parse(input)?;
                nonempty(&value.ticket)?;
                (
                    WorkspaceRequestMethod::Get,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/readiness",
                        value.ticket
                    ),
                    None,
                )
            }
            Kind::Open => {
                let value: OpenInput = parse(input)?;
                nonempty(&value.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request",
                        value.ticket
                    ),
                    Some(json!({
                        "repository_id": value.repository_id,
                        "selector_from": value.selector_from,
                        "selector_to": value.selector_to,
                        "base_commit": value.base_commit,
                        "head_commit": value.head_commit,
                        "changed_paths": value.changed_paths,
                        "summary": value.summary,
                    })),
                )
            }
            Kind::RequestReview => {
                let value: RequestReviewInput = parse(input)?;
                nonempty(&value.ticket)?;
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/review-requests",
                        value.ticket
                    ),
                    Some(json!({
                        "expected_head_commit": value.expected_head_commit,
                        "base_commit": value.base_commit,
                        "head_commit": value.head_commit,
                        "changed_paths": value.changed_paths,
                        "summary": value.summary,
                    })),
                )
            }
            Kind::Complete => {
                let value: CompleteInput = parse(input)?;
                nonempty(&value.ticket)?;
                let strategy = match value.strategy {
                    MergeStrategyInput::FastForward => "fast_forward",
                    MergeStrategyInput::Merge => "merge",
                };
                let resolution = match value.resolution {
                    MergeResolutionInput::None => "none",
                    MergeResolutionInput::Clean => "clean",
                    MergeResolutionInput::ConflictsResolved => "conflicts_resolved",
                };
                (
                    WorkspaceRequestMethod::Post,
                    format!(
                        "/api/w/{workspace_id}/tickets/{}/merge-request/complete",
                        value.ticket
                    ),
                    Some(json!({
                        "operation_id": value.operation_id,
                        "expected_head_commit": value.expected_head_commit,
                        "target_commit": value.target_commit,
                        "source_commit": value.source_commit,
                        "result_commit": value.result_commit,
                        "strategy": strategy,
                        "resolution": resolution,
                    })),
                )
            }
            Kind::Review => {
                let value: ReviewInput = parse(input)?;
                let context = self.client.reviewer_context().ok_or_else(|| {
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
                    Some(json!({
                        "decision": match value.decision {
                            ReviewDecisionInput::Approve => "approve",
                            ReviewDecisionInput::RequestChanges => "request_changes",
                        },
                        "body": value.body,
                        "findings": value.findings.into_iter().map(|finding| json!({
                            "severity": finding.severity,
                            "path": finding.path,
                            "line": finding.line,
                            "message": finding.message,
                        })).collect::<Vec<_>>(),
                    })),
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
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
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
    serde_json::from_str(value).map_err(|error| ToolError::InvalidArgument(error.to_string()))
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
        definition(client.clone(), Kind::RequestReview),
        definition(client, Kind::Complete),
    ]
}

pub fn reviewer_tools(client: Arc<dyn WorkspaceClient>) -> Vec<ToolDefinition> {
    if client.reviewer_context().is_some() {
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
            "Read the authoritative Merge Request, selector pair, append-only thread, and current review status.",
        ),
        "MergeRequestReadinessCheck" => {
            Some("Check derived merge readiness for the current review request.")
        }
        "MergeRequestOpen" => Some(
            "Open a Merge Request with immutable source/target selectors and its first review request.",
        ),
        "MergeRequestRequestReview" => Some(
            "Append a RequestForReview event for new candidate evidence; prior approval cannot carry forward.",
        ),
        "MergeRequestComplete" => Some(
            "CAS-complete the approved current candidate with operation-id replay and crash fencing.",
        ),
        "MergeRequestReviewSubmit" => {
            Some("Submit the attested direct-child Reviewer result for the current candidate.")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_request_tool_contract_uses_selectors_and_commit_fences_without_revision_ids() {
        let open = serde_json::to_string(&schemars::schema_for!(OpenInput)).unwrap();
        let request = serde_json::to_string(&schemars::schema_for!(RequestReviewInput)).unwrap();
        let complete = serde_json::to_string(&schemars::schema_for!(CompleteInput)).unwrap();
        assert!(open.contains("selector_from"));
        assert!(open.contains("selector_to"));
        assert!(request.contains("expected_head_commit"));
        assert!(complete.contains("expected_head_commit"));
        for schema in [&open, &request, &complete] {
            assert!(!schema.contains("revision_id"));
            assert!(!schema.contains("attempt_id"));
            assert!(!schema.contains("head_tree"));
            assert!(!schema.contains("diff_digest"));
        }
        assert!(!MERGE_REQUEST_COMMON_TOOL_NAMES.contains(&"MergeRequestAddRevision"));
    }
}
