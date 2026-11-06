//! Registered Read -> WorkspaceAttachedWorkdirSession -> shared audit projection ->
//! real Session append/reopen/prune. Only Workspace transport is substituted;
//! audit persistence and write counts are covered by the Server SQLite/HTTP suite.
use super::*;
use agen::{Item, ToolResultDisposition, tool::ToolResult};
use error_contract::{Browser, Budget, DecodeFailure, FailureFacts, project_failure};
use workdir::{
    WorkdirSession,
    audit::failure::WorkdirAuditFailureDetails,
    failure::WorkdirFailureVisibility,
    http::{
        WorkdirAuditFailure, WorkdirAuditFailureReason, WorkdirAuditOperationOutcome,
        WorkdirAuditStage,
    },
};

#[tokio::test]
async fn registered_read_audit_warning_retains_success_content_and_reopened_model_cause() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("sample.txt"), "original file content\n").unwrap();
    let local = workdir::LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().into(),
    );
    let read = local
        .read(workdir::ReadRequest {
            path: workdir::WorkdirPath::new("sample.txt").unwrap(),
            offset: 0,
            limit: 20,
            max_bytes: 4096,
        })
        .await
        .unwrap();
    let receipt = WorkdirAuditFailure {
        operation_id: "/private/audit/provider/token=secret".into(),
        stage: WorkdirAuditStage::ResponseReceived,
        operation_outcome: WorkdirAuditOperationOutcome::Succeeded,
        reason: WorkdirAuditFailureReason::AuditStoreUnavailable,
    };
    let client = Arc::new(RecordingWorkspaceClient::new(vec![response(
        serde_json::to_value(workdir::http::WorkdirSessionOperationResponse {
            result: WorkdirSessionOperationResult::Read(read),
            audit_failures: vec![receipt.clone()],
        })
        .unwrap(),
    )]));
    let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
    let (_, tool) = tools::read_tool(session, tools::Tracker::new())();
    let output = tool
        .execute(
            r#"{"file_path":"sample.txt"}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap();
    assert!(output.content.as_deref().unwrap().starts_with("     1"));
    assert!(
        output
            .content
            .as_deref()
            .unwrap()
            .contains("original file content")
    );
    assert!(output.summary.contains("do not resend"));
    let result = ToolResult::from_output("read-audit-call", output);
    assert!(!result.is_error);
    assert_eq!(result.disposition, ToolResultDisposition::Success);
    let history = crate::test_support::reopen_result(result);
    let Item::ToolResult {
        summary,
        content: Some(content),
        disposition,
        is_error,
        ..
    } = &history
    else {
        panic!("retained Tool result")
    };
    assert!(!is_error);
    assert_eq!(*disposition, ToolResultDisposition::Success);
    let (_, report) = content.rsplit_once("\n\nWorkdir audit report:\n").unwrap();
    let report: serde_json::Value = serde_json::from_str(report).unwrap();
    assert_eq!(
        report["result"]["audit_failures"][0]["operation_outcome"]["kind"],
        "succeeded"
    );
    assert!(report["result"]["primary_error_class"].is_null());
    let DecodeFailure::Known(received) = error_contract::decode_failure::<WorkdirAuditFailureDetails>(
        &serde_json::to_vec(&report["failure"]).unwrap(),
        Budget::new(65536, 65536),
    ) else {
        panic!("typed secondary cause on replay")
    };
    let browser = project_failure::<Browser, _>(
        &FailureFacts::new(WorkdirAuditFailureDetails::from(&receipt)).unwrap(),
        &WorkdirFailureVisibility {
            can_view_cause: true,
        },
    )
    .unwrap();
    assert_eq!(received.facts().cause(), browser.details());
    assert!(summary.contains(browser.explanation()));
    let wire = serde_json::to_string(&history).unwrap();
    assert!(!wire.contains("token=secret"));
    assert!(!wire.contains("/private/audit"));
    let mut request_history = vec![history.clone()];
    assert_eq!(agen::prune::project(&mut request_history, &[0]), 1);
    assert!(
        matches!(&request_history[0], Item::ToolResult { summary, content: None, is_error: false, disposition: ToolResultDisposition::Success, .. } if summary.contains("audit persistence is unavailable") && summary.contains("do not resend"))
    );
    assert_eq!(
        client.requests().len(),
        1,
        "audit loss must not resend the operation"
    );
}

#[tokio::test]
async fn registered_write_unknown_and_audit_warning_survive_reopen_without_second_write() {
    let client = Arc::new(RecordingWorkspaceClient::new(vec![
        WorkspaceResponse {
            status: 404,
            body: json!({"code":"not_found", "message":"safe remote classification"}).to_string(),
        },
        WorkspaceResponse {
            status: 500,
            body: json!({
                "code":"outcome_unknown", "message":"private-provider-body",
                "audit_failures":[{
                    "operation_id":"/private/audit/token=secret", "stage":"response_unknown",
                    "reason":"audit_store_unavailable", "operation_outcome":{"kind":"unknown"}
                }]
            })
            .to_string(),
        },
    ]));
    let session = WorkspaceAttachedWorkdirSession::handle(client.clone(), "checkout");
    let (_, tool) = tools::write_tool(session, tools::Tracker::new())();
    let error = tool
        .execute(
            r#"{"file_path":"new.txt","content":"new content"}"#,
            ToolExecutionContext::default(),
        )
        .await
        .unwrap_err();
    let result = ToolResult::from_error("write-audit-call", error);
    assert!(result.is_error);
    assert_eq!(result.disposition, ToolResultDisposition::Error);
    let history = crate::test_support::reopen_result(result);
    let Item::ToolResult {
        summary,
        content: Some(content),
        disposition: ToolResultDisposition::Error,
        is_error: true,
        ..
    } = &history
    else {
        panic!("original write failure and secondary cause must survive")
    };
    assert!(summary.contains("outcome is unknown"));
    assert!(summary.contains("inspect effects before considering a retry"));
    let (_, report) = content.rsplit_once("\n\nWorkdir audit report:\n").unwrap();
    let report: serde_json::Value = serde_json::from_str(report).unwrap();
    assert_eq!(report["result"]["primary_error_class"], "execution_failed");
    assert_eq!(
        report["result"]["audit_failures"][0]["operation_outcome"]["kind"],
        "unknown"
    );
    let wire = serde_json::to_string(&history).unwrap();
    for secret in ["private-provider-body", "/private/audit", "token=secret"] {
        assert!(!wire.contains(secret));
    }
    let mut request_history = vec![history];
    agen::prune::project(&mut request_history, &[0]);
    assert!(
        matches!(&request_history[0], Item::ToolResult { summary, content: None, disposition: ToolResultDisposition::Error, is_error: true, .. } if summary.contains("outcome is unknown") && summary.contains("inspect effects"))
    );
    let operations = client
        .requests()
        .into_iter()
        .map(|request| {
            let body: serde_json::Value =
                serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
            body["operation"]["operation"].as_str().unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        operations,
        ["stat", "write"],
        "neither unknown nor audit loss may resend"
    );
}
