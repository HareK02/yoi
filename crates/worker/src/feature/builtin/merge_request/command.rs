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
        view: Option<MergeRequestReadView>,
        after: Option<u64>,
        limit: Option<usize>,
    },
    WorkerSet {
        merge_request_id: String,
        #[serde(flatten)]
        request: server_api::SetMergeRequestWorkerRequest,
    },
    Comment {
        merge_request_id: String,
        operation_id: String,
        expected_owner_operation_id: String,
        body: String,
    },
    Open {
        ticket: String,
        repository_key: String,
        selector_from: String,
        selector_to: String,
        #[serde(default)]
        summary: String,
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
                    view: v.view,
                    after: v.after,
                    limit: v.limit,
                }
            }
            Kind::WorkerSet => {
                let v: SetMergeRequestWorkerInput = parse(input)?;
                Self::WorkerSet {
                    merge_request_id: v.merge_request_id,
                    request: v.request,
                }
            }
            Kind::Comment => {
                let v: CommentMergeRequestInput = parse(input)?;
                Self::Comment {
                    merge_request_id: v.merge_request_id,
                    operation_id: v.operation_id,
                    expected_owner_operation_id: v.expected_owner_operation_id,
                    body: v.body,
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
            Self::WorkerSet { .. } => Kind::WorkerSet,
            Self::Comment { .. } => Kind::Comment,
            Self::Open { .. } => Kind::Open,
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
    let ownership_mutation = matches!(
        &command,
        Command::WorkerSet { .. } | Command::Comment { .. }
    );
    let executor = Executor { client };
    let ws = executor.client.workspace_id().ok_or_else(|| {
        ToolError::ExecutionFailed("Merge Request operations require Workspace identity".into())
    })?;
    let (method, path, body) = match command {
        Command::Read {
            merge_request_id,
            view,
            after,
            limit,
        } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            if limit.is_some_and(|limit| !(1..=200).contains(&limit)) {
                return Err(ToolError::InvalidArgument(
                    "limit must be between 1 and 200".into(),
                ));
            }
            let view = view.unwrap_or_else(|| {
                if after.is_some() || limit.is_some() {
                    MergeRequestReadView::Detail
                } else {
                    MergeRequestReadView::Summary
                }
            });
            if matches!(view, MergeRequestReadView::Summary) && (after.is_some() || limit.is_some())
            {
                return Err(ToolError::InvalidArgument(
                    "after/limit require view=detail".into(),
                ));
            }
            return executor.show_merge_request(ws, &merge_request_id, view, after, limit);
        }
        Command::WorkerSet {
            merge_request_id,
            request,
        } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            nonempty_named("operation_id", &request.operation_id)?;
            if let Some(expected) = &request.expected_operation_id {
                nonempty_named("expected_operation_id", expected)?;
            }
            if let Some(worker) = &request.worker {
                nonempty_named("runtime_id", &worker.runtime_id)?;
                nonempty_named("worker_id", &worker.worker_id)?;
            }
            executor.authorize(
                "merge_request.assign",
                PermissionResource::MergeRequest {
                    workspace_id: ws.into(),
                    merge_request_id: merge_request_id.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Put,
                format!(
                    "/api/w/{ws}/merge-requests/{}/worker",
                    encode_path_segment(&merge_request_id)
                ),
                Some(json!(request)),
            )
        }
        Command::Comment {
            merge_request_id,
            operation_id,
            expected_owner_operation_id,
            body,
        } => {
            nonempty_named("merge_request_id", &merge_request_id)?;
            nonempty_named("operation_id", &operation_id)?;
            nonempty_named("expected_owner_operation_id", &expected_owner_operation_id)?;
            if body.trim().is_empty() {
                return Err(ToolError::InvalidArgument("body must be nonempty".into()));
            }
            executor.authorize(
                "merge_request.comment",
                PermissionResource::MergeRequest {
                    workspace_id: ws.into(),
                    merge_request_id: merge_request_id.clone(),
                },
            )?;
            (
                WorkspaceRequestMethod::Post,
                format!(
                    "/api/w/{ws}/merge-requests/{}/comments",
                    encode_path_segment(&merge_request_id)
                ),
                Some(
                    json!({"operation_id":operation_id,"expected_owner_operation_id":expected_owner_operation_id,"body":body}),
                ),
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
    let res = executor.client.execute(req).map_err(|error| {
        if ownership_mutation {
            ownership_outcome_unknown(&error.to_string())
        } else {
            ToolError::ExecutionFailed(error.to_string())
        }
    })?;
    if !res.is_success() {
        let error = api_error("Merge Request API", &res);
        return Err(if ownership_mutation && res.status >= 500 {
            ownership_outcome_unknown(&error.to_string())
        } else {
            error
        });
    }
    response_value(&res.body).map_err(|error| {
        if ownership_mutation {
            ownership_outcome_unknown(&error.to_string())
        } else {
            error
        }
    })
}

fn ownership_outcome_unknown(detail: &str) -> ToolError {
    ToolError::ExecutionFailed(format!(
        "MR mutation outcome is unknown after dispatch: {detail}. Reconcile the saved assignment or comment; any exact retry must retain the original operation_id and full input. Do not automatically retry or mint a new operation_id."
    ))
}

struct Executor {
    client: Arc<dyn WorkspaceClient>,
}

impl Executor {
    fn authorize(&self, operation: &str, resource: PermissionResource) -> Result<(), ToolError> {
        self.client.authorize_operation(operation, &resource)
            .map_err(|error| {
                let denied = error.is_authorization_denial();
                ToolError::ExecutionFailed(format!("Merge Request operation authorization failed before dispatch: {operation}; resource={resource:?}; denied={denied}"))
            })
    }
    fn show_merge_request(
        &self,
        workspace_id: &str,
        merge_request_id: &str,
        view: MergeRequestReadView,
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
        let mut query = vec![format!(
            "view={}",
            match view {
                MergeRequestReadView::Summary => "summary",
                MergeRequestReadView::Detail => "detail",
            }
        )];
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
