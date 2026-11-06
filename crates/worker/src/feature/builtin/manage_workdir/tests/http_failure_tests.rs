//! The registered consumer retains received cause and independent audit outcome.
//! Local route tests use the production lifecycle Tool and router; only the
//! Workspace transport and the provider's terminal close boundary are substituted.
use super::*;
use workdir::failure::{WorkdirFailureDetails, WorkdirFailureOperation, WorkdirFailureVisibility};
use workdir::http::{
    WorkdirAuditFailure, WorkdirAuditFailureReason, WorkdirAuditOperationOutcome,
    WorkdirAuditStage, WorkdirTransportError, WorkdirTransportErrorCode as Code,
};

#[derive(Debug)]
struct FailingCloseSession {
    inner: WorkdirSessionHandle,
    calls: Arc<AtomicUsize>,
    private_message: &'static str,
}

#[async_trait]
impl WorkdirSession for FailingCloseSession {
    fn workdir(&self) -> &Workdir {
        self.inner.workdir()
    }
    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.inner.capabilities()
    }
    async fn stat(&self, request: StatRequest) -> Result<workdir::StatResult, WorkdirError> {
        self.inner.stat(request).await
    }
    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.inner.read(request).await
    }
    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.inner.write(request).await
    }
    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        self.inner.edit(request).await
    }
    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.inner.list(request).await
    }
    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.inner.glob(request).await
    }
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.inner.grep(request).await
    }
    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.inner.start_command(request).await
    }
    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.inner.command_status(handle).await
    }
    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.inner.command_output(request).await
    }
    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.inner.cancel_command(handle).await
    }
    async fn close(&self) -> Result<(), WorkdirError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(WorkdirError::OutcomeUnknown(self.private_message.into())
            .with_failure_reason(workdir::WorkdirFailureReason::ProviderResponseLost))
    }
}

#[tokio::test]
async fn registered_detach_keeps_backend_success_and_typed_close_unknown_without_resending() {
    for private_message in ["SECRET /private/path", "confirmed safe to repeat detach"] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
            "workspace_id":"workspace/test", "alias":"checkout", "working_directory_id":"wd",
            "capabilities":{"bits":63}, "attached":false
        }))]));
        let router = Arc::new(workdir::WorkdirSessionRouter::new());
        let calls = Arc::new(AtomicUsize::new(0));
        router
            .attach(
                workdir::WorkdirAttachmentAlias::new("checkout").unwrap(),
                Arc::new(FailingCloseSession {
                    inner: WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout"),
                    calls: calls.clone(),
                    private_message,
                }),
            )
            .unwrap();
        let (_, tool) = workdir_tool(
            DETACH_TOOL,
            DETACH_DESCRIPTION,
            detach_schema(),
            WorkspaceHttpWorkdirBackend::new(client.clone()).with_session_router(router.clone()),
            WorkdirOperation::Detach,
        )();
        let output = tool
            .execute(r#"{"alias":"checkout"}"#, ToolExecutionContext::default())
            .await
            .unwrap();
        assert!(output.summary.contains("Backend detach succeeded"));
        assert!(output.summary.contains("Do not repeat Backend detach"));
        assert!(!output.summary.contains(private_message));
        let result: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(result["attached"], false);
        assert_eq!(result["working_directory_id"], "wd");
        let attachment: protocol::startup_failure::StartupFailureAttachment =
            serde_json::from_value(result["cleanup_failures"][0].clone()).unwrap();
        let cause = attachment
            .project_for::<error_contract::Tool, WorkdirFailureDetails>(&WorkdirFailureVisibility {
                can_view_cause: true,
            })
            .unwrap();
        assert_eq!(
            cause.details().operation,
            WorkdirFailureOperation::SessionClose
        );
        assert_eq!(cause.details().classification, Code::OutcomeUnknown);
        assert_eq!(
            cause.details().failure_reason,
            Some(workdir::WorkdirFailureReason::ProviderResponseLost)
        );
        assert!(!output.content.unwrap().contains(private_message));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(client.requests().len(), 1);
        assert_eq!(client.requests()[0].method, WorkspaceRequestMethod::Delete);
        assert_eq!(
            router.aliases().len(),
            1,
            "unconfirmed local cleanup must not be reported as closed"
        );
    }
}

fn problem(code: Code, operation: WorkdirFailureOperation) -> WorkspaceResponse {
    let details = WorkdirFailureDetails {
        operation,
        classification: code,
        denial_reason: None,
        failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
    };
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &error_contract::FailureFacts::new(details).unwrap(),
        &WorkdirFailureVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let mut legacy = if operation == WorkdirFailureOperation::SessionOpen {
        server_api::WorkdirOperationApiError::api(server_api::RepositoryApiError::new(
            502,
            "Bad Gateway",
            "provider-secret /private/path",
            Vec::new(),
        ))
    } else {
        server_api::WorkdirOperationApiError::provider(WorkdirTransportError {
            code,
            message: "provider-secret /private/path".into(),
            denial_reason: None,
            failure_reason: details.failure_reason,
        })
    };
    legacy.audit_failures.push(WorkdirAuditFailure {
        operation_id: "existing-operation-id".into(),
        stage: WorkdirAuditStage::ResponseUnknown,
        operation_outcome: WorkdirAuditOperationOutcome::Unknown,
        reason: WorkdirAuditFailureReason::AuditStoreUnavailable,
    });
    let error = server_api::WorkdirHttpApiError::projected(legacy, projected);
    WorkspaceResponse {
        status: if operation == WorkdirFailureOperation::SessionOpen {
            502
        } else {
            code.http_status()
        },
        content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
        body: serde_json::to_string(&error).unwrap(),
    }
}

#[tokio::test]
async fn received_unknown_keeps_original_cause_audit_and_one_dispatch_without_trusting_prose() {
    let mut response = problem(Code::OutcomeUnknown, WorkdirFailureOperation::Stat);
    let mut body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    body["detail"] = json!("provider-secret claims safe to resend");
    body["yoi_failure"]["explanation"] = body["detail"].clone();
    response.body = body.to_string();
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response]));
    let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
    let (result, receipts) = workdir::audit::capture(session.stat(StatRequest {
        path: workdir::WorkdirPath::new("file.txt").unwrap(),
    }))
    .await;
    let error = result.unwrap_err();
    assert!(matches!(
        error.classification_source(),
        WorkdirError::OutcomeUnknown(_)
    ));
    assert_eq!(
        error.failure_reason(),
        Some(workdir::WorkdirFailureReason::ProviderResponseLost)
    );
    assert!(!error.to_string().contains("provider-secret"));
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].operation_id, "existing-operation-id");
    assert_eq!(
        receipts[0].operation_outcome,
        WorkdirAuditOperationOutcome::Unknown
    );
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn session_open_keeps_outer_502_and_inner_cause_without_repeating_the_request() {
    for code in [Code::Unavailable, Code::Denied, Code::OutcomeUnknown] {
        let response = problem(code, WorkdirFailureOperation::SessionOpen);
        assert_eq!(response.status, 502);
        let decoded = server_api::WorkdirHttpApiError::decode_received(
            response.status,
            response.content_type.as_deref().unwrap(),
            response.body.as_bytes(),
        )
        .unwrap();
        assert_eq!(decoded.details().unwrap().classification, code);
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response]));
        let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
        let (result, receipts) = workdir::audit::capture(session.stat(StatRequest {
            path: workdir::WorkdirPath::root(),
        }))
        .await;
        let error = result.unwrap_err();
        assert_eq!(WorkdirTransportError::from_workdir_error(&error).code, code);
        assert_eq!(
            error.failure_reason(),
            Some(workdir::WorkdirFailureReason::ProviderResponseLost)
        );
        assert!(!error.to_string().contains("provider-secret"));
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0].operation_outcome,
            WorkdirAuditOperationOutcome::Unknown
        );
        assert_eq!(client.requests().len(), 1);
    }
}

#[tokio::test]
async fn wrong_operation_or_http_status_is_not_an_authenticated_operation_result() {
    for mismatch in [true, false] {
        let mut response = problem(
            Code::OutcomeUnknown,
            if mismatch {
                WorkdirFailureOperation::Write
            } else {
                WorkdirFailureOperation::Stat
            },
        );
        if !mismatch {
            response.status = 403;
        }
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response]));
        let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
        let (result, receipts) = workdir::audit::capture(session.stat(StatRequest {
            path: workdir::WorkdirPath::new("file.txt").unwrap(),
        }))
        .await;
        let error = result.unwrap_err();
        assert!(matches!(
            error.classification_source(),
            WorkdirError::Transport(_)
        ));
        assert_eq!(
            error.failure_reason(),
            Some(if mismatch {
                workdir::WorkdirFailureReason::OperationResultMismatch
            } else {
                workdir::WorkdirFailureReason::HttpResponseInvalid
            })
        );
        assert!(receipts.is_empty());
        assert_eq!(client.requests().len(), 1);
    }
}

#[tokio::test]
async fn missing_media_type_does_not_select_legacy_codec_or_infer_a_rejection_from_body() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
        status: 403,
        content_type: None,
        body: json!({"code":"denied", "message":"provider-secret confirmed no effects"})
            .to_string(),
    }]));
    let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
    let error = session
        .stat(StatRequest {
            path: workdir::WorkdirPath::root(),
        })
        .await
        .unwrap_err();
    assert!(matches!(
        error.classification_source(),
        WorkdirError::Transport(_)
    ));
    assert_eq!(
        error.failure_reason(),
        Some(workdir::WorkdirFailureReason::HttpResponseInvalid)
    );
    assert!(!error.to_string().contains("provider-secret"));
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn registered_read_reprojects_received_http_cause_without_provider_text_or_new_requests() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![problem(
        Code::Denied,
        WorkdirFailureOperation::Read,
    )]));
    let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
    let (_, tool) = tools::read_tool(session, tools::Tracker::new())();
    let error = tool
        .execute(
            r#"{"file_path":"file.txt"}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap_err();
    let ToolError::ExecutionFailure(output) = error else {
        panic!("missing typed Tool cause")
    };
    let content = output.content.as_deref().unwrap();
    let (primary, audit) = content.rsplit_once("\n\nWorkdir audit report:\n").unwrap();
    let report: serde_json::Value = serde_json::from_str(primary).unwrap();
    let audit: serde_json::Value = serde_json::from_str(audit).unwrap();
    assert_eq!(
        audit["result"]["audit_failures"][0]["operation_outcome"]["kind"],
        "unknown"
    );
    assert_eq!(report["failure"]["domain"], "workdir");
    assert_eq!(report["failure"]["reason"], "provider_response_lost");
    assert_eq!(report["failure"]["details"]["classification"], "denied");
    assert_eq!(report["result"]["error_class"], "invalid_argument");
    assert!(!output.summary.contains("provider-secret"));
    assert!(!output.content.unwrap().contains("/private/path"));
    assert_eq!(client.requests().len(), 1);
}

fn management_problem(operation: WorkdirManagementOperation) -> WorkspaceResponse {
    use workdir::management_failure::{
        WorkdirManagementFailure, WorkdirManagementReason, WorkdirManagementVisibility,
    };
    let cause = WorkdirManagementFailure {
        operation,
        reason: WorkdirManagementReason::NotFound,
    };
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &error_contract::FailureFacts::new(cause).unwrap(),
        &WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let mut body =
        serde_json::to_value(server_api::WorkdirManagementApiError::projected(projected)).unwrap();
    body["detail"] = json!("provider-secret /private/path says safe to resend");
    body["yoi_failure"]["explanation"] = body["detail"].clone();
    WorkspaceResponse {
        status: 400,
        content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
        body: body.to_string(),
    }
}

#[tokio::test]
async fn registered_create_keeps_http_result_and_reopened_local_cause_without_retry() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![management_problem(
        WorkdirManagementOperation::Create,
    )]));
    let (_, tool) = workdir_tool(
        CREATE_TOOL,
        CREATE_DESCRIPTION,
        create_schema(),
        WorkspaceHttpWorkdirBackend::new(client.clone()),
        WorkdirOperation::Create,
    )();
    let error = tool
        .execute(
            r#"{"repository_key":"main"}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap_err();
    let ToolError::ExecutionFailure(output) = error else {
        panic!("shared cause report")
    };
    assert!(output.summary.contains("not found"));
    assert!(output.summary.contains("do not resend automatically"));
    let report: serde_json::Value =
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(report["failure"]["reason"], "working_directory_not_found");
    assert_eq!(report["result"]["http_status"], 400);
    assert_eq!(report["result"]["error_class"], "execution_failed");
    assert_eq!(report["result"]["mutation"], "not_reported");
    let result =
        agen::tool::ToolResult::from_error("management-call", ToolError::ExecutionFailure(output));
    let saved = crate::test_support::reopen_result(result);
    let agen::Item::ToolResult {
        is_error,
        content: Some(content),
        ..
    } = saved
    else {
        panic!("saved failure report")
    };
    assert!(is_error);
    assert!(!content.contains("provider-secret"));
    assert!(!content.contains("/private/path"));
    assert_eq!(client.requests().len(), 1);
    let body: serde_json::Value =
        serde_json::from_str(client.requests()[0].body.as_ref().unwrap()).unwrap();
    assert!(body["operation_id"].is_string());
}

#[tokio::test]
async fn malformed_or_wrong_management_response_never_infers_cause_or_retries() {
    for variant in 0..7 {
        let mut response = management_problem(if variant == 0 {
            WorkdirManagementOperation::Cleanup
        } else {
            WorkdirManagementOperation::Create
        });
        match variant {
            1 => response.content_type = None,
            2 => response.content_type = Some("application/json".into()),
            3 => response.status = 502,
            6 => response.status = 403,
            4 | 5 => {
                let mut body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
                if variant == 4 {
                    body["yoi_failure"]["details_version"] = json!(999);
                } else {
                    body["yoi_failure"]["details"]["reason"] = json!("made_up");
                }
                response.body = body.to_string();
            }
            _ => {}
        }
        let client = Arc::new(RecordingWorkspaceClient::new(vec![response]));
        let tool = WorkspaceHttpWorkdirTool {
            backend: WorkspaceHttpWorkdirBackend::new(client.clone()),
            operation: WorkdirOperation::Create,
        };
        let result = tool
            .execute_lifecycle(
                r#"{"repository_key":"main"}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(result.source, ToolError::ExecutionFailed(_)));
        assert!(result.report.is_none());
        assert!(!result.to_string().contains("provider-secret"));
        assert_eq!(client.requests().len(), 1);
    }
}

#[tokio::test]
async fn registered_inventory_retains_success_and_grant_targets_with_fresh_diagnostic_projection() {
    use error_contract::{Browser, FailureFacts, OperationContext, project_failure};
    use workdir::management_failure::{
        WorkdirManagementFailure, WorkdirManagementReason, WorkdirManagementVisibility,
    };
    let cause = WorkdirManagementFailure {
        operation: WorkdirManagementOperation::List,
        reason: WorkdirManagementReason::LegacyExternalGrantPermissions,
    };
    let mut facts = FailureFacts::new(cause).unwrap();
    for (operation, id) in [
        ("workdir.inventory", "wd-omitted"),
        ("workdir.grant", "grant-legacy"),
    ] {
        let mut context = OperationContext::new(operation, "legacy_permissions").unwrap();
        context.operation_ref = Some(id.into());
        facts.with_context(context).unwrap();
    }
    let browser = project_failure::<Browser, _>(
        &facts,
        &WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let failure =
        protocol::startup_failure::StartupFailureAttachment::from_projected(&browser).unwrap();
    let mut untrusted: serde_json::Value =
        serde_json::from_str(serde_json::to_value(&failure).unwrap().as_str().unwrap()).unwrap();
    untrusted["explanation"] = json!("provider-secret /private/path");
    let mut wrong_version = untrusted.clone();
    wrong_version["details_version"] = json!(999);
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(json!({
        "workspace_id":"workspace/test", "items":[],
        "diagnostics":[
            {"code":"opaque_arbitrary_code", "severity":"error", "message":"provider-secret /private/path", "failure":untrusted.to_string()},
            {"code":"working_directory_create_failed", "severity":"error", "message":"provider-secret /private/path"},
            {"code":"working_directory_create_failed", "severity":"error", "message":"provider-secret /private/path", "failure":wrong_version.to_string()}
        ]
    }))]));
    let (_, tool) = workdir_tool(
        LIST_TOOL,
        LIST_DESCRIPTION,
        list_schema(),
        WorkspaceHttpWorkdirBackend::new(client.clone()),
        WorkdirOperation::List,
    )();
    let output = tool
        .execute("{}", ToolExecutionContext::default())
        .await
        .unwrap();
    assert!(output.summary.contains("wd-omitted"));
    assert!(output.summary.contains("grant-legacy"));
    assert!(output.summary.contains("revoke"));
    assert!(!output.summary.contains("provider-secret"));
    let result: serde_json::Value =
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert!(result["items"].as_array().unwrap().is_empty());
    assert_eq!(
        result["diagnostics"][0]["code"],
        "external_workdir_legacy_permissions"
    );
    assert_eq!(result["diagnostics"][0]["severity"], "error");
    assert_eq!(
        result["diagnostics"][1]["code"],
        "workdir_diagnostic_unavailable"
    );
    assert!(result["diagnostics"][1].get("failure").is_none());
    assert!(result["diagnostics"][2].get("failure").is_none());
    assert!(!result.to_string().contains("provider-secret"));
    assert!(!result.to_string().contains("/private/path"));
    let saved = crate::test_support::reopen_result(agen::tool::ToolResult::from_output(
        "list-call",
        output,
    ));
    let agen::Item::ToolResult {
        is_error, summary, ..
    } = saved
    else {
        panic!("saved inventory result")
    };
    assert!(!is_error);
    assert!(summary.contains("grant-legacy"));
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn registered_attach_preserves_occupied_conflict_and_cause_without_local_route_or_retry() {
    use workdir::management_failure::{
        WorkdirManagementFailure, WorkdirManagementReason, WorkdirManagementVisibility,
    };
    let cause = WorkdirManagementFailure {
        operation: WorkdirManagementOperation::Attach,
        reason: WorkdirManagementReason::Occupied,
    };
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &error_contract::FailureFacts::new(cause).unwrap(),
        &WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let client = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
        status: 409,
        content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
        body: serde_json::to_string(&server_api::WorkdirManagementApiError::projected(projected))
            .unwrap(),
    }]));
    let backend = WorkspaceHttpWorkdirBackend::new(client.clone());
    let error = backend
        .attach(WorkdirAttachInput {
            alias: "checkout".into(),
            working_directory_id: "occupied".into(),
        })
        .unwrap_err();
    assert!(
        matches!(error.source, ToolError::StructuredConflict { ref code, .. } if code == "workdir_occupied")
    );
    let output = error.report.unwrap();
    let report: serde_json::Value =
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(report["result"]["error_class"], "structured_conflict");
    assert_eq!(report["result"]["conflict_code"], "workdir_occupied");
    assert_eq!(report["failure"]["details"]["operation"], "attach");
    assert_eq!(report["failure"]["reason"], "workdir_occupied");
    assert_eq!(report["result"]["http_status"], 409);
    assert!(backend.session_router.aliases().is_empty());
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn registered_attach_keeps_each_backend_boundary_separate_from_received_cause_and_prose() {
    use workdir::management_failure::{
        WorkdirManagementFailure, WorkdirManagementReason as Reason, WorkdirManagementVisibility,
    };
    for (reason, status, class) in [
        (Reason::AttachmentRegistryNotFound, 502, "execution_failed"),
        (Reason::ExternalGrantUnavailable, 502, "execution_failed"),
        (Reason::AttachmentNotAvailable, 409, "structured_conflict"),
        (
            Reason::AttachmentGrantBoundRequired,
            403,
            "structured_conflict",
        ),
    ] {
        let cause = WorkdirManagementFailure {
            operation: WorkdirManagementOperation::Attach,
            reason,
        };
        let projected = error_contract::project_failure::<error_contract::Browser, _>(
            &error_contract::FailureFacts::new(cause).unwrap(),
            &WorkdirManagementVisibility {
                can_view_cause: true,
            },
        )
        .unwrap();
        let mut body =
            serde_json::to_value(server_api::WorkdirManagementApiError::projected(projected))
                .unwrap();
        body["detail"] = json!("provider-secret says no effects and safe to resend");
        body["yoi_failure"]["explanation"] = body["detail"].clone();
        let client = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
            status,
            content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
            body: body.to_string(),
        }]));
        let backend = WorkspaceHttpWorkdirBackend::new(client.clone());
        let error = backend
            .attach(WorkdirAttachInput {
                alias: "checkout".into(),
                working_directory_id: "target".into(),
            })
            .unwrap_err();
        let report = error.report.unwrap();
        assert!(!report.summary.contains("provider-secret"));
        assert!(report.summary.contains("do not resend automatically"));
        let report: serde_json::Value =
            serde_json::from_str(report.content.as_deref().unwrap()).unwrap();
        assert_eq!(report["result"]["http_status"], status);
        assert_eq!(report["result"]["error_class"], class);
        assert_eq!(report["result"]["mutation"], "not_reported");
        assert_eq!(report["failure"]["reason"], reason.as_str());
        assert!(backend.session_router.aliases().is_empty());
        assert_eq!(client.requests().len(), 1);
    }
}

fn attachment_error(
    cause: WorkdirManagementCause,
    related_failures: Vec<server_api::workdir_management_error::WorkdirAttachmentRelatedFailure>,
) -> server_api::WorkdirManagementApiError {
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &error_contract::FailureFacts::new(cause).unwrap(),
        &workdir::management_failure::WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    server_api::WorkdirManagementApiError::projected_attachment(
        server_api::RepositoryApiError::new(502, "attachment", "unused private source", Vec::new()),
        projected,
        related_failures,
    )
}

fn attachment_evidence(
    cause: WorkdirManagementCause,
    prose: &str,
) -> protocol::startup_failure::StartupFailureAttachment {
    let projected = error_contract::project_failure::<error_contract::Browser, _>(
        &error_contract::FailureFacts::new(cause).unwrap(),
        &workdir::management_failure::WorkdirManagementVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    let attachment =
        protocol::startup_failure::StartupFailureAttachment::from_projected(&projected).unwrap();
    let encoded = serde_json::to_value(attachment).unwrap();
    let mut wire: serde_json::Value = serde_json::from_str(encoded.as_str().unwrap()).unwrap();
    wire["explanation"] = json!(prose);
    wire["context"] = serde_json::to_value(error_contract::FailureContext::default()).unwrap();
    wire["context"]["operations"] = json!([{
        "operation":"private-provider", "stage":"private-stage", "operation_ref":"provider-secret"
    }]);
    serde_json::from_value(json!(wire.to_string())).unwrap()
}

// Only the HTTP endpoint/Host authorization are substituted. The shared HTTP encoder,
// generated ServerApiClient, Worker receive decoder, lifecycle Tool/router and saved result
// are real. This fixture does not claim to exercise Backend attachment ownership/persistence.
#[derive(Debug)]
struct AttachmentHttpClient {
    base_url: String,
    authorization: RecordingWorkspaceClient,
    requests: Mutex<Vec<WorkspaceRequest>>,
}

impl WorkspaceClient for AttachmentHttpClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace/test")
    }
    fn kind(&self) -> &str {
        "attachment-http-fixture"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        request: WorkspaceRequest,
    ) -> Result<WorkspaceResponse, WorkspaceClientError> {
        if request.path.ends_with("/authorization/check")
            || request.path.ends_with("/authorization/bound-check")
        {
            return self.authorization.execute(request);
        }
        self.requests.lock().unwrap().push(request.clone());
        let base_url = self.base_url.clone();
        std::thread::spawn(move || {
            let send = || -> Result<WorkspaceResponse, reqwest::Error> {
                let client = reqwest::blocking::Client::builder()
                    .timeout(std::time::Duration::from_secs(5))
                    .build()?;
                let method = match request.method {
                    WorkspaceRequestMethod::Post => reqwest::Method::POST,
                    WorkspaceRequestMethod::Delete => reqwest::Method::DELETE,
                    _ => panic!("unexpected fixture operation"),
                };
                let mut builder = client.request(method, format!("{base_url}{}", request.path));
                if let Some(body) = request.body {
                    builder = builder
                        .header("content-type", "application/json")
                        .body(body);
                }
                let response = builder.send()?;
                Ok(WorkspaceResponse {
                    status: response.status().as_u16(),
                    content_type: response
                        .headers()
                        .get("content-type")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned),
                    body: response.text()?,
                })
            };
            send().map_err(|error| WorkspaceClientError::Request(error.to_string()))
        })
        .join()
        .expect("bounded HTTP fixture thread")
    }
}

#[derive(Clone, Copy, Debug)]
enum AttachmentHttpFault {
    None,
    UnknownRelatedStage,
    WrongRelatedFailureType,
    MalformedRelatedList,
    MixedRelatedRows,
    RelatedUnavailable,
    CorruptPrimary,
    WrongHttpStatus,
}

impl AttachmentHttpFault {
    fn apply(self, wire: &mut serde_json::Value, status: &mut axum::http::StatusCode) {
        match self {
            Self::None => {}
            Self::UnknownRelatedStage => {
                wire["related_failures"][0]["stage"] = json!("future-private-stage");
            }
            Self::WrongRelatedFailureType => {
                wire["related_failures"][0]["failure"] = json!({});
            }
            Self::MalformedRelatedList => {
                wire["related_failures"] = json!({"provider-secret":"/private/path"});
            }
            Self::MixedRelatedRows => {
                let mut future = wire["related_failures"][0].clone();
                future["stage"] = json!("future-private-stage");
                let mut malformed = wire["related_failures"][0].clone();
                malformed["failure"] = json!({});
                let rows = wire["related_failures"].as_array_mut().unwrap();
                rows.push(future);
                rows.push(malformed);
            }
            Self::RelatedUnavailable => {
                wire["related_failures_unavailable"] = json!(true);
            }
            Self::CorruptPrimary => {
                wire["yoi_failure"]["details"] = json!({"provider-secret":"/private/path"});
                wire["related_failures"] = json!({});
            }
            Self::WrongHttpStatus => {
                *status = axum::http::StatusCode::FORBIDDEN;
                wire["related_failures"] = json!({});
            }
        }
    }
}

struct AttachmentHttpFixture {
    generated: server_api::ServerApiClient,
    workspace: Arc<AttachmentHttpClient>,
    calls: Arc<AtomicUsize>,
    // JoinSet aborts the isolated server on drop, including assertion failure.
    _tasks: tokio::task::JoinSet<()>,
}

impl AttachmentHttpFixture {
    async fn new(
        cause: WorkdirManagementCause,
        related: Vec<server_api::workdir_management_error::WorkdirAttachmentRelatedFailure>,
        prose: &'static str,
    ) -> Self {
        Self::with_fault(cause, related, prose, AttachmentHttpFault::None).await
    }

    async fn with_fault(
        cause: WorkdirManagementCause,
        related: Vec<server_api::workdir_management_error::WorkdirAttachmentRelatedFailure>,
        prose: &'static str,
        fault: AttachmentHttpFault,
    ) -> Self {
        use axum::{body::Body, http::StatusCode, response::Response, routing::post};
        let calls = Arc::new(AtomicUsize::new(0));
        let handler_calls = calls.clone();
        let handler = move || {
            let error = attachment_error(cause, related.clone());
            let calls = handler_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let response =
                    server_api::server_support::http_error_response(StatusCode::BAD_GATEWAY, error);
                let (mut parts, body) = response.into_parts();
                let bytes = axum::body::to_bytes(body, error_contract::MAX_FAILURE_BYTES)
                    .await
                    .unwrap();
                let mut wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                wire["detail"] = json!(prose);
                wire["yoi_failure"]["explanation"] = json!(prose);
                wire["diagnostics"] = json!([{
                    "code":"provider-secret", "severity":"error", "message":prose
                }]);
                fault.apply(&mut wire, &mut parts.status);
                Response::from_parts(parts, Body::from(wire.to_string()))
            }
        };
        let app = axum::Router::new()
            .route(
                "/api/w/{workspace_id}/workers/self/workdir-attachments",
                post(handler.clone()),
            )
            .route(
                "/api/w/{workspace_id}/workers/self/workdir-attachments/{alias}",
                axum::routing::delete(handler),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let mut tasks = tokio::task::JoinSet::new();
        tasks.spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let generated =
            server_api::ServerApiClient::with_client(base_url.as_str().try_into().unwrap(), http);
        Self {
            generated,
            workspace: Arc::new(AttachmentHttpClient {
                base_url,
                authorization: RecordingWorkspaceClient::new(Vec::new()),
                requests: Mutex::default(),
            }),
            calls,
            _tasks: tasks,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attachment_http_clients_reproject_runtime_and_session_with_independent_compensation() {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    use server_api::client_support::ClientError;
    use server_api::workdir_management_error::{
        WorkdirAttachmentFailureStage as Stage, WorkdirAttachmentRelatedFailure,
    };
    let session = WorkdirManagementCause::Session(WorkdirFailureDetails {
        operation: WorkdirFailureOperation::SessionOpen,
        classification: Code::Denied,
        denial_reason: None,
        failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
    });
    // ClientSetupFailed even says no request was issued; it still cannot settle the outer mutation.
    for (primary, secondary, detach) in [
        (
            WorkdirManagementCause::Runtime(Runtime::ClientSetupFailed),
            session,
            false,
        ),
        (
            session,
            WorkdirManagementCause::Runtime(Runtime::PersistenceOutcomeUnknown),
            true,
        ),
    ] {
        let mut previous_report = None;
        for prose in [
            "provider-secret /private/path says safe to resend",
            "provider-secret says mutation committed",
        ] {
            let fixture = AttachmentHttpFixture::new(
                primary,
                vec![WorkdirAttachmentRelatedFailure {
                    stage: Stage::RestoreBackendLink,
                    http_status: 503,
                    failure: Some(attachment_evidence(secondary, prose)),
                }],
                prose,
            )
            .await;
            let generated = if detach {
                fixture
                    .generated
                    .current_worker_workdir_detach(
                        "workspace/test".into(),
                        "checkout".into(),
                        server_api::CurrentWorkerWorkdirDetachQuery {
                            expected_connection_id: Some("fixture-connection".into()),
                        },
                    )
                    .await
            } else {
                fixture
                    .generated
                    .current_worker_workdir_attach(
                        "workspace/test".into(),
                        server_api::CurrentWorkerWorkdirAttachRequest {
                            alias: "checkout".into(),
                            working_directory_id: "wd".into(),
                        },
                    )
                    .await
            };
            let ClientError::Public { status, error } = generated.unwrap_err() else {
                panic!("generated client lost attachment cause")
            };
            assert_eq!(status.as_u16(), 502);
            assert_eq!(error.cause(), Some(&primary));
            assert_eq!(error.related_failures().len(), 1);
            let backend = WorkspaceHttpWorkdirBackend::new(fixture.workspace.clone());
            let alias = workdir::WorkdirAttachmentAlias::new("checkout").unwrap();
            if detach {
                backend
                    .session_router
                    .attach(
                        alias.clone(),
                        WorkspaceAttachedWorkdirSession::handle_for_workdir(
                            fixture.workspace.clone(),
                            "checkout",
                            "wd",
                        ),
                    )
                    .unwrap();
            }
            let (_, tool) = if detach {
                workdir_tool(
                    DETACH_TOOL,
                    DETACH_DESCRIPTION,
                    detach_schema(),
                    backend.clone(),
                    WorkdirOperation::Detach,
                )()
            } else {
                workdir_tool(
                    ATTACH_TOOL,
                    ATTACH_DESCRIPTION,
                    attach_schema(),
                    backend.clone(),
                    WorkdirOperation::Attach,
                )()
            };
            let error = tool
                .execute(
                    if detach {
                        r#"{"alias":"checkout"}"#
                    } else {
                        r#"{"alias":"checkout","working_directory_id":"wd"}"#
                    },
                    ToolExecutionContext::default(),
                )
                .await
                .unwrap_err();
            let ToolError::ExecutionFailure(output) = error else {
                panic!("missing fresh Tool report")
            };
            assert!(output.summary.contains("do not resend automatically"));
            assert!(!output.summary.contains("provider-secret"));
            let report: serde_json::Value =
                serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            assert_eq!(report["boundary"], "observation");
            assert_eq!(report["result"]["http_status"], 502);
            assert_eq!(report["result"]["error_class"], "execution_failed");
            assert_eq!(report["result"]["mutation"], "not_reported");
            assert_eq!(
                report["failure"]["details"],
                serde_json::to_value(primary).unwrap(),
            );
            assert_eq!(
                report["result"]["related_failures"][0]["stage"],
                "restore_backend_link",
            );
            assert_eq!(report["result"]["related_failures"][0]["http_status"], 503);
            let secondary_attachment: protocol::startup_failure::StartupFailureAttachment =
                serde_json::from_value(report["result"]["related_failures"][0]["failure"].clone())
                    .unwrap();
            let error_contract::DecodeFailure::Known(received) =
                secondary_attachment.decode::<WorkdirManagementCause>()
            else {
                panic!("secondary cause lost")
            };
            assert_eq!(received.facts().cause(), &secondary);
            assert!(received.facts().context().operations.is_empty());
            let visibility = workdir::management_failure::WorkdirManagementVisibility {
                can_view_cause: true,
            };
            let fresh_primary = error_contract::project_failure::<error_contract::Tool, _>(
                &error_contract::FailureFacts::new(primary).unwrap(),
                &visibility,
            )
            .unwrap();
            assert_eq!(
                report["failure"]["explanation"],
                fresh_primary.explanation()
            );
            let fresh_secondary = error_contract::project_failure::<error_contract::Tool, _>(
                &error_contract::FailureFacts::new(secondary).unwrap(),
                &visibility,
            )
            .unwrap();
            assert_eq!(
                secondary_attachment,
                protocol::startup_failure::StartupFailureAttachment::from_projected(
                    &fresh_secondary
                )
                .unwrap(),
            );
            if let Some(previous) = &previous_report {
                assert_eq!(previous, &report);
            }
            previous_report = Some(report);
            let saved = crate::test_support::reopen_result(agen::tool::ToolResult::from_error(
                "attachment-call",
                ToolError::ExecutionFailure(output),
            ));
            let agen::Item::ToolResult {
                is_error,
                summary,
                content: Some(content),
                ..
            } = saved
            else {
                panic!("saved attachment result")
            };
            assert!(is_error);
            for secret in [
                "provider-secret",
                "/private/path",
                "private-provider",
                "private-stage",
            ] {
                assert!(!content.contains(secret));
                assert!(!summary.contains(secret));
            }
            assert_eq!(fixture.workspace.requests.lock().unwrap().len(), 1);
            assert_eq!(
                fixture.calls.load(Ordering::SeqCst),
                2,
                "one generated observation and one Tool dispatch",
            );
            if detach {
                assert_eq!(backend.session_router.aliases().len(), 1);
                // Failed HTTP detach cancels route exclusion; the same alias remains usable.
                backend.session_router.begin_detach(&alias).await.unwrap();
                backend.session_router.cancel_detach(&alias);
                let requests = fixture.workspace.requests.lock().unwrap();
                assert!(
                    requests[0]
                        .path
                        .ends_with("?expected_connection_id=fixture-connection")
                );
            } else {
                assert!(backend.session_router.aliases().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn unavailable_compensation_attachments_preserve_primary_and_stage_without_fabricating_causes()
 {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    use server_api::workdir_management_error::{
        WorkdirAttachmentFailureStage as Stage, WorkdirAttachmentRelatedFailure,
    };
    use workdir::management_failure::{WorkdirManagementFailure, WorkdirManagementReason};
    let primary = WorkdirManagementCause::Runtime(Runtime::TransportFailed);
    let secondary = WorkdirManagementCause::Session(WorkdirFailureDetails {
        operation: WorkdirFailureOperation::SessionClose,
        classification: Code::OutcomeUnknown,
        denial_reason: None,
        failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
    });
    let management = WorkdirManagementCause::Management(WorkdirManagementFailure {
        operation: WorkdirManagementOperation::Detach,
        reason: WorkdirManagementReason::AttachmentProjectionFailed,
    });
    let valid = attachment_evidence(secondary, "provider-secret /private/path");
    let encoded = serde_json::to_value(&valid).unwrap();
    let mut unsupported: serde_json::Value =
        serde_json::from_str(encoded.as_str().unwrap()).unwrap();
    unsupported["details_version"] = json!(999);
    let corrupt = serde_json::from_value(json!("{invalid provider-secret /private/path")).unwrap();
    let unsupported = serde_json::from_value(json!(unsupported.to_string())).unwrap();
    let related = [
        Some(valid),
        Some(attachment_evidence(
            management,
            "provider-secret /private/path",
        )),
        None,
        Some(corrupt),
        Some(unsupported),
    ]
    .into_iter()
    .map(|failure| WorkdirAttachmentRelatedFailure {
        stage: Stage::SessionClose,
        http_status: 409,
        failure,
    })
    .collect();
    let body = serde_json::to_string(&attachment_error(primary, related)).unwrap();
    let client = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
        status: 502,
        content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
        body,
    }]));
    let (_, tool) = workdir_tool(
        ATTACH_TOOL,
        ATTACH_DESCRIPTION,
        attach_schema(),
        WorkspaceHttpWorkdirBackend::new(client.clone()),
        WorkdirOperation::Attach,
    )();
    let error = tool
        .execute(
            r#"{"alias":"checkout","working_directory_id":"wd"}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap_err();
    let ToolError::ExecutionFailure(output) = error else {
        panic!("primary lost with optional evidence")
    };
    let report: serde_json::Value =
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        report["failure"]["details"],
        serde_json::to_value(primary).unwrap(),
    );
    assert_eq!(report["result"]["http_status"], 502);
    assert_eq!(report["result"]["mutation"], "not_reported");
    let related = report["result"]["related_failures"].as_array().unwrap();
    assert_eq!(related.len(), 5);
    for (index, related) in related.iter().enumerate() {
        assert_eq!(related["stage"], "session_close");
        assert_eq!(related["http_status"], 409);
        assert_eq!(related.get("failure").is_some(), index < 2);
    }
    for (related, cause) in related.iter().zip([secondary, management]) {
        let attachment: protocol::startup_failure::StartupFailureAttachment =
            serde_json::from_value(related["failure"].clone()).unwrap();
        let error_contract::DecodeFailure::Known(received) =
            attachment.decode::<WorkdirManagementCause>()
        else {
            panic!("supported compensation lost")
        };
        assert_eq!(received.facts().cause(), &cause);
    }
    assert!(!report.to_string().contains("provider-secret"));
    assert!(!report.to_string().contains("/private/path"));
    assert_eq!(client.requests().len(), 1);
}

#[tokio::test]
async fn attachment_prerequisite_cause_cannot_authenticate_create_or_a_changed_http_status() {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    for (operation, status) in [
        (WorkdirOperation::Create, 502),
        (WorkdirOperation::Attach, 403),
    ] {
        let client = Arc::new(RecordingWorkspaceClient::new(vec![WorkspaceResponse {
            status,
            content_type: Some(error_contract::problem::PROBLEM_CONTENT_TYPE.into()),
            body: serde_json::to_string(&attachment_error(
                WorkdirManagementCause::Runtime(Runtime::ClientSetupFailed),
                Vec::new(),
            ))
            .unwrap(),
        }]));
        let tool = WorkspaceHttpWorkdirTool {
            backend: WorkspaceHttpWorkdirBackend::new(client.clone()),
            operation,
        };
        let error = tool
            .execute_lifecycle(
                match operation {
                    WorkdirOperation::Create => r#"{"repository_key":"main"}"#,
                    _ => r#"{"alias":"checkout","working_directory_id":"wd"}"#,
                },
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error.source, ToolError::ExecutionFailed(_)));
        assert!(error.report.is_none());
        assert!(tool.backend.session_router.aliases().is_empty());
        assert_eq!(client.requests().len(), 1);
    }
}

#[test]
fn compensation_output_budget_does_not_displace_the_primary_failure() {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    use server_api::workdir_management_error::{
        WorkdirAttachmentFailureStage as Stage, WorkdirAttachmentRelatedFailure,
    };
    let primary = WorkdirManagementCause::Runtime(Runtime::TransportFailed);
    let related = WorkdirAttachmentRelatedFailure {
        stage: Stage::RestoreBackendLink,
        http_status: 503,
        failure: Some(attachment_evidence(primary, "provider-secret")),
    };
    let error = LifecycleError::received(
        lifecycle_http_source(502, None),
        502,
        primary,
        &vec![related; 512],
        false,
    );
    let output = error.report.unwrap();
    let report: serde_json::Value =
        serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        report["failure"]["details"],
        serde_json::to_value(primary).unwrap(),
    );
    assert_eq!(report["result"]["mutation"], "not_reported");
    assert_eq!(report["result"]["related_failures_unavailable"], true);
    assert_eq!(report["result"]["related_failures"], json!([]));
    assert!(!report.to_string().contains("provider-secret"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn optional_http_metadata_loss_survives_generated_client_tool_and_saved_reopen() {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    use server_api::client_support::ClientError;
    use server_api::workdir_management_error::{
        WorkdirAttachmentFailureStage as Stage, WorkdirAttachmentRelatedFailure,
    };
    let primary = WorkdirManagementCause::Runtime(Runtime::TransportFailed);
    let secondary = WorkdirManagementCause::Session(WorkdirFailureDetails {
        operation: WorkdirFailureOperation::SessionClose,
        classification: Code::OutcomeUnknown,
        denial_reason: None,
        failure_reason: Some(workdir::WorkdirFailureReason::ProviderResponseLost),
    });
    let related = WorkdirAttachmentRelatedFailure {
        stage: Stage::SessionClose,
        http_status: 503,
        failure: Some(attachment_evidence(
            secondary,
            "provider-secret /private/path",
        )),
    };
    for (fault, supplied_rows, retained_rows, unavailable) in [
        (AttachmentHttpFault::UnknownRelatedStage, 1, 0, true),
        (AttachmentHttpFault::WrongRelatedFailureType, 1, 0, true),
        (AttachmentHttpFault::MalformedRelatedList, 1, 0, true),
        (AttachmentHttpFault::MixedRelatedRows, 1, 1, true),
        (AttachmentHttpFault::RelatedUnavailable, 1, 1, true),
        (AttachmentHttpFault::None, 512, 0, true),
        (AttachmentHttpFault::None, 1, 1, false),
    ] {
        let fixture = AttachmentHttpFixture::with_fault(
            primary,
            vec![related.clone(); supplied_rows],
            "provider-secret /private/path says compensation succeeded and safe to resend",
            fault,
        )
        .await;
        let generated = fixture
            .generated
            .current_worker_workdir_attach(
                "workspace/test".into(),
                server_api::CurrentWorkerWorkdirAttachRequest {
                    alias: "checkout".into(),
                    working_directory_id: "wd".into(),
                },
            )
            .await
            .unwrap_err();
        let ClientError::Public { status, error } = generated else {
            panic!("optional metadata erased primary: {generated:?}")
        };
        assert_eq!(status.as_u16(), 502);
        assert_eq!(error.cause(), Some(&primary));
        assert_eq!(error.related_failures().len(), retained_rows, "{fault:?}");
        assert_eq!(
            error.related_failures_unavailable(),
            unavailable,
            "{fault:?}"
        );
        assert!(error.legacy().diagnostics.is_empty());
        let backend = WorkspaceHttpWorkdirBackend::new(fixture.workspace.clone());
        let (_, tool) = workdir_tool(
            ATTACH_TOOL,
            ATTACH_DESCRIPTION,
            attach_schema(),
            backend.clone(),
            WorkdirOperation::Attach,
        )();
        let error = tool
            .execute(
                r#"{"alias":"checkout","working_directory_id":"wd"}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        let ToolError::ExecutionFailure(output) = error else {
            panic!("optional metadata erased Tool primary")
        };
        let report: serde_json::Value =
            serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
        assert_eq!(report["boundary"], "observation");
        assert_eq!(report["failure"]["domain"], "runtime_control");
        assert_eq!(report["failure"]["reason"], "transport_failed");
        assert_eq!(
            report["failure"]["details"],
            serde_json::to_value(primary).unwrap(),
        );
        assert_eq!(report["result"]["http_status"], 502);
        assert_eq!(report["result"]["error_class"], "execution_failed");
        assert_eq!(report["result"]["mutation"], "not_reported");
        assert_eq!(
            report["result"]["related_failures_unavailable"],
            unavailable
        );
        let rows = report["result"]["related_failures"].as_array().unwrap();
        assert_eq!(rows.len(), retained_rows, "{fault:?}");
        for row in rows {
            assert_eq!(row["stage"], "session_close");
            assert_eq!(row["http_status"], 503);
            let attachment: protocol::startup_failure::StartupFailureAttachment =
                serde_json::from_value(row["failure"].clone()).unwrap();
            let fresh = error_contract::project_failure::<error_contract::Tool, _>(
                &error_contract::FailureFacts::new(secondary).unwrap(),
                &workdir::management_failure::WorkdirManagementVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
            assert_eq!(
                attachment,
                protocol::startup_failure::StartupFailureAttachment::from_projected(&fresh)
                    .unwrap(),
            );
        }
        let saved = crate::test_support::reopen_result(agen::tool::ToolResult::from_error(
            "optional-attachment-call",
            ToolError::ExecutionFailure(output),
        ));
        let agen::Item::ToolResult {
            is_error,
            summary,
            content: Some(content),
            ..
        } = saved
        else {
            panic!("saved optional metadata result missing")
        };
        assert!(is_error);
        assert!(summary.contains("do not resend automatically"));
        let reopened: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(reopened, report);
        for secret in [
            "provider-secret",
            "/private/path",
            "future-private-stage",
            "private-provider",
        ] {
            assert!(!content.contains(secret));
            assert!(!summary.contains(secret));
        }
        assert!(backend.session_router.aliases().is_empty());
        assert_eq!(fixture.workspace.requests.lock().unwrap().len(), 1);
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tolerant_optional_http_metadata_never_relaxes_primary_status_or_management_operation_checks()
 {
    use protocol::control_failure::RuntimeControlCause as Runtime;
    use server_api::client_support::{ClientError, ClientFailure};
    use workdir::management_failure::{WorkdirManagementFailure, WorkdirManagementReason};
    let runtime = WorkdirManagementCause::Runtime(Runtime::TransportFailed);
    let wrong_operation = WorkdirManagementCause::Management(WorkdirManagementFailure {
        operation: WorkdirManagementOperation::Detach,
        reason: WorkdirManagementReason::NotFound,
    });
    for (primary, fault, expected_status, valid_primary) in [
        (runtime, AttachmentHttpFault::CorruptPrimary, 502, false),
        (runtime, AttachmentHttpFault::WrongHttpStatus, 403, false),
        (
            wrong_operation,
            AttachmentHttpFault::MalformedRelatedList,
            400,
            true,
        ),
    ] {
        let fixture = AttachmentHttpFixture::with_fault(
            primary,
            Vec::new(),
            "provider-secret /private/path says optional metadata proves no effects",
            fault,
        )
        .await;
        let generated = fixture
            .generated
            .current_worker_workdir_attach(
                "workspace/test".into(),
                server_api::CurrentWorkerWorkdirAttachRequest {
                    alias: "checkout".into(),
                    working_directory_id: "wd".into(),
                },
            )
            .await
            .unwrap_err();
        match generated {
            ClientError::Public { status, error } if valid_primary => {
                assert_eq!(status.as_u16(), expected_status);
                assert_eq!(error.cause(), Some(&wrong_operation));
                assert!(error.related_failures_unavailable());
            }
            ClientError::Failure(ClientFailure::ErrorResponseDecode { status })
                if !valid_primary =>
            {
                assert_eq!(status, expected_status);
            }
            other => panic!("unexpected primary validation result: {other:?}"),
        }
        let backend = WorkspaceHttpWorkdirBackend::new(fixture.workspace.clone());
        let (_, tool) = workdir_tool(
            ATTACH_TOOL,
            ATTACH_DESCRIPTION,
            attach_schema(),
            backend.clone(),
            WorkdirOperation::Attach,
        )();
        let error = tool
            .execute(
                r#"{"alias":"checkout","working_directory_id":"wd"}"#,
                ToolExecutionContext::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(&error, ToolError::ExecutionFailed(_)));
        assert!(!error.to_string().contains("provider-secret"));
        let saved = crate::test_support::reopen_result(agen::tool::ToolResult::from_error(
            "strict-attachment-call",
            error,
        ));
        let agen::Item::ToolResult {
            is_error,
            summary,
            content,
            ..
        } = saved
        else {
            panic!("saved strict rejection missing")
        };
        assert!(is_error);
        assert!(summary.contains("execution outcome is not established"));
        assert!(!summary.contains("provider-secret"));
        assert!(content.is_none());
        assert!(backend.session_router.aliases().is_empty());
        assert_eq!(fixture.workspace.requests.lock().unwrap().len(), 1);
        assert_eq!(fixture.calls.load(Ordering::SeqCst), 2);
    }
}
