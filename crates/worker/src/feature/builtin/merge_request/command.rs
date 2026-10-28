//! Shared typed commands, below the independent Tool and native input contracts.
//!
//! Native validation produces normalized command fields, not a Tool invocation. Both adapters
//! enter the same authorization and Workspace request boundary; neither adapter retries dispatch.

use super::*;

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(super) enum Command {
    Read {
        merge_request_id: String,
        after: Option<u64>,
        limit: Option<usize>,
    },
    Readiness {
        merge_request_id: String,
    },
    Open {
        ticket: String,
        repository_key: String,
        selector_from: String,
        selector_to: String,
        #[serde(default)]
        summary: String,
    },
    Complete {
        merge_request_id: String,
        operation_id: String,
        approval_event_id: String,
        target_ref_before: String,
        target_ref_after: String,
        strategy: MergeStrategyInput,
        resolution: MergeResolutionInput,
    },
    Review {
        decision: ReviewDecisionInput,
        #[serde(default)]
        body: String,
        #[serde(default)]
        findings: Vec<ReviewFindingInput>,
    },
}

impl Command {
    /// Tool-mode parsing remains governed by its own DTOs and schemars declarations.
    pub(super) fn from_tool(kind: Kind, input: &str) -> Result<Self, ToolError> {
        Ok(match kind {
            Kind::Show => {
                let v: ShowMergeRequestInput = parse(input)?;
                Self::Read {
                    merge_request_id: v.merge_request_id,
                    after: v.after,
                    limit: v.limit,
                }
            }
            Kind::Readiness => {
                let v: MergeRequestInput = parse(input)?;
                Self::Readiness {
                    merge_request_id: v.merge_request_id,
                }
            }
            Kind::Open => {
                let v: OpenMergeRequestInput = parse(input)?;
                Self::Open {
                    ticket: v.ticket,
                    repository_key: v.repository_key,
                    selector_from: v.selector_from,
                    selector_to: v.selector_to,
                    summary: v.summary,
                }
            }
            Kind::Complete => {
                let v: CompleteMergeRequestInput = parse(input)?;
                Self::Complete {
                    merge_request_id: v.merge_request_id,
                    operation_id: v.operation_id,
                    approval_event_id: v.approval_event_id,
                    target_ref_before: v.target_ref_before,
                    target_ref_after: v.target_ref_after,
                    strategy: v.strategy,
                    resolution: v.resolution,
                }
            }
            Kind::Review => {
                let v: ReviewMergeRequestInput = parse(input)?;
                Self::Review {
                    decision: v.decision,
                    body: v.body,
                    findings: v.findings,
                }
            }
        })
    }

    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Read { .. } => Kind::Show,
            Self::Readiness { .. } => Kind::Readiness,
            Self::Open { .. } => Kind::Open,
            Self::Complete { .. } => Kind::Complete,
            Self::Review { .. } => Kind::Review,
        }
    }
}

/// Ordinary Tool output serializes the shared domain value only at this presentation boundary.
pub(super) fn execute(
    client: Arc<dyn WorkspaceClient>,
    command: Command,
) -> Result<ToolOutput, ToolError> {
    let kind = command.kind();
    let value = execute_value(client, command)?;
    Ok(ToolOutput {
        summary: kind.name().into(),
        content: Some(value.to_string()),
        attachments: vec![],
    })
}

/// The single domain dispatch boundary. Decode the documented Workspace JSON response once;
/// native consumers never recover domain references by reparsing a formatted ToolOutput.
/// Backend authorization precedes every data request.
pub(super) fn execute_value(
    client: Arc<dyn WorkspaceClient>,
    command: Command,
) -> Result<Value, ToolError> {
    let executor = Executor { client };
    let ws = executor.client.workspace_id().ok_or_else(|| {
        ToolError::ExecutionFailed("Merge Request operations require Workspace identity".into())
    })?;
    let (method, path, body) = match command {
        Command::Read {
            merge_request_id,
            after,
            limit,
        } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            if limit.is_some_and(|limit| !(1..=200).contains(&limit)) {
                return Err(ToolError::InvalidArgument(
                    "limit must be between 1 and 200".into(),
                ));
            }
            return executor.show_merge_request(ws, &merge_request_id, after, limit);
        }
        Command::Readiness { merge_request_id } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            executor.authorize(
                "merge_request.readiness",
                PermissionResource::MergeRequest {
                    workspace_id: ws.into(),
                    merge_request_id: merge_request_id.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Get,
                format!(
                    "/api/w/{ws}/merge-requests/{}/readiness",
                    encode_path_segment(&merge_request_id)
                ),
                None,
            )
        }
        Command::Open {
            ticket,
            repository_key,
            selector_from,
            selector_to,
            summary,
        } => {
            nonempty(&ticket)?;
            executor.authorize(
                "merge_request.open",
                PermissionResource::Ticket {
                    workspace_id: ws.into(),
                    ticket_id: ticket.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Post,
                format!(
                    "/api/w/{ws}/tickets/{}/merge-request",
                    encode_path_segment(&ticket)
                ),
                Some(
                    json!({"repository_key":repository_key,"selector_from":selector_from,"selector_to":selector_to,"summary":summary}),
                ),
            )
        }
        Command::Complete {
            merge_request_id,
            operation_id,
            approval_event_id,
            target_ref_before,
            target_ref_after,
            strategy,
            resolution,
        } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            executor.authorize(
                "merge_request.complete",
                PermissionResource::MergeRequest {
                    workspace_id: ws.into(),
                    merge_request_id: merge_request_id.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Post,
                format!(
                    "/api/w/{ws}/merge-requests/{}/complete",
                    encode_path_segment(&merge_request_id)
                ),
                Some(
                    json!({"operation_id":operation_id,"approval_event_id":approval_event_id,"target_ref_before":target_ref_before,"target_ref_after":target_ref_after,"strategy":match strategy{MergeStrategyInput::FastForward=>"fast_forward",MergeStrategyInput::Merge=>"merge"},"resolution":match resolution{MergeResolutionInput::None=>"none",MergeResolutionInput::Clean=>"clean",MergeResolutionInput::ConflictsResolved=>"conflicts_resolved"}}),
                ),
            )
        }
        Command::Review {
            decision,
            body,
            findings,
        } => {
            let ctx = executor.client.reviewer_context().ok_or_else(|| {
                ToolError::ExecutionFailed(
                    "Review submit requires injected Reviewer capability".into(),
                )
            })?;
            executor.authorize(
                "merge_request.review",
                PermissionResource::MergeRequest {
                    workspace_id: ws.into(),
                    merge_request_id: ctx.merge_request_id.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Post,
                format!(
                    "/api/w/{ws}/merge-requests/{}/reviews",
                    encode_path_segment(&ctx.merge_request_id)
                ),
                Some(
                    json!({"decision":match decision{ReviewDecisionInput::Approve=>"approve",ReviewDecisionInput::RequestChanges=>"request_changes"},"body":body,"findings":findings.into_iter().map(|f|json!({"severity":f.severity,"code":f.code,"path":f.path,"line":f.line,"body":f.body})).collect::<Vec<_>>() }),
                ),
            )
        }
    };
    let req = match body {
        Some(v) => WorkspaceRequest::json(method, path, v.to_string()),
        None => WorkspaceRequest::get(path),
    };
    let res = executor
        .client
        .execute(req)
        .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
    if !res.is_success() {
        return Err(api_error("Merge Request API", &res));
    }
    response_value(&res.body)
}

struct Executor {
    client: Arc<dyn WorkspaceClient>,
}

impl Executor {
    fn authorize(&self, operation: &str, resource: PermissionResource) -> Result<(), ToolError> {
        self.client.authorize_operation(operation, &resource)
            .map_err(|error| {
                let denied = matches!(&error, crate::worker::WorkspaceClientError::Request(message)
                    if message.contains("HTTP status 403") || message.contains("HTTP status 401"));
                ToolError::ExecutionFailed(format!("Merge Request operation authorization failed before dispatch: {operation}; resource={resource:?}; denied={denied}"))
            })
    }
    fn show_merge_request(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
        after: Option<u64>,
        limit: Option<usize>,
    ) -> Result<Value, ToolError> {
        self.authorize(
            "merge_request.read",
            PermissionResource::MergeRequest {
                workspace_id: workspace_id.into(),
                merge_request_id: merge_request_id.into(),
            },
        )?;
        let merge_request_id = encode_path_segment(merge_request_id);
        let mut path = format!("/api/w/{workspace_id}/merge-requests/{merge_request_id}");
        let mut query = Vec::new();
        if let Some(after) = after {
            query.push(format!("after={after}"));
        }
        if let Some(limit) = limit {
            query.push(format!("limit={limit}"));
        }
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query.join("&"));
        }
        let response = self
            .client
            .execute(WorkspaceRequest::get(path))
            .map_err(|error| ToolError::ExecutionFailed(error.to_string()))?;
        if !response.is_success() {
            return Err(api_error("Merge Request API", &response));
        }
        response_value(&response.body)
    }
}

fn response_value(body: &str) -> Result<Value, ToolError> {
    serde_json::from_str(body).map_err(|_| {
        ToolError::ExecutionFailed("Merge Request API returned invalid JSON after dispatch".into())
    })
}
