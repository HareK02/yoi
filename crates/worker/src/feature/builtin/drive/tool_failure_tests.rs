//! Registered Tool -> production DriveBackend -> shared result -> actual Session log/replay.
//! Only the Workspace transport is replaced. Mutation persistence remains covered by
//! the existing real-HTTP/SQLite suites; these tests assert no extra dispatch here.
use super::*;
use crate::feature::builtin::drive::tool_failure::{
    DriveToolFailureResult, DriveToolMutationObservation,
};
use agen::{Item, ToolResultDisposition, tool::ToolResult};
use error_contract::{Budget, DecodeFailure};

use crate::test_support::reopen_result;

async fn registered_failure(
    feature: &DriveFeature,
    name: &str,
    input: Value,
    context: ToolExecutionContext,
) -> ToolResult {
    let (_, definition) = feature
        .tools()
        .into_iter()
        .find(|(registered, _)| *registered == name)
        .unwrap();
    let (_, tool) = definition();
    let error = tool.execute(&input.to_string(), context).await.unwrap_err();
    ToolResult::from_error("registered-drive-call", error)
}

#[tokio::test]
async fn registered_drive_unknown_keeps_original_receipt_in_reopened_model_history_without_resend()
{
    let old = entry("2", "1");
    let client = Client::new(Vec::new());
    let feature = feature(client.clone());
    feature.observe(old.clone()).unwrap();
    let context = ToolExecutionContext::new("registered-write", "batch", 0);
    let request_id = feature.request_id(&context);
    client.responses.lock().unwrap().extend([
        Err(WorkspaceClientError::Request(
            "private-provider-body".into(),
        )),
        Ok(response(DriveRequestStatusResponse {
            request_id: request_id.clone(),
            state: DriveRequestState::Uncommitted,
            response: None,
        })),
    ]);
    let result = registered_failure(
        &feature,
        "DriveWrite",
        json!({"entry":old.entry,"content":"new"}),
        context,
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.disposition, ToolResultDisposition::Error);
    assert!(result.summary.contains(&request_id));
    assert!(result.summary.contains("do not resend"));
    let history = reopen_result(result);
    let mut request_history = vec![history.clone()];
    assert_eq!(agen::prune::project(&mut request_history, &[0]), 1);
    assert!(
        matches!(&request_history[0], Item::ToolResult { summary, content: None, is_error: true, disposition: ToolResultDisposition::Error, .. } if summary.contains(&request_id) && summary.contains("do not resend"))
    );
    assert!(
        matches!(
            &history,
            Item::ToolResult {
                content: Some(_),
                ..
            }
        ),
        "request pruning must not mutate restored canonical history"
    );
    let Item::ToolResult {
        summary,
        content: Some(content),
        disposition,
        is_error,
        ..
    } = history
    else {
        panic!("lost the structured failure on Session replay")
    };
    assert!(is_error);
    assert_eq!(disposition, ToolResultDisposition::Error);
    assert!(summary.contains(&request_id));
    assert!(!summary.contains("private-provider-body"));
    let report: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(report["boundary"], "observation");
    let result: DriveToolFailureResult = serde_json::from_value(report["result"].clone()).unwrap();
    assert_eq!(
        result.mutation,
        DriveToolMutationObservation::OutcomeUnknown {
            request_id: request_id.clone()
        }
    );
    let cause = error_contract::decode_failure::<failure::DriveClientFailureDetails>(
        &serde_json::to_vec(&report["failure"]).unwrap(),
        Budget::new(65536, 65536),
    );
    let DecodeFailure::Known(cause) = cause else {
        panic!("typed cause missing")
    };
    assert_eq!(
        cause.facts().cause().reason,
        failure::DriveClientFailureReason::OutcomeUnknown
    );
    assert_eq!(
        cause.facts().cause().operation,
        failure::DriveClientFailureOperation::UpdateText
    );
    let requests = client.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|r| r.method == crate::worker::WorkspaceRequestMethod::Post)
            .count(),
        1
    );
    assert!(
        requests[1]
            .path
            .ends_with(&format!("/requests/{request_id}"))
    );
}

#[tokio::test]
async fn registered_drive_denial_discloses_only_current_attempt_cause_and_has_no_data_dispatch() {
    let client = Client::new(Vec::new());
    client
        .auth_denied
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let result = registered_failure(
        &feature(client.clone()),
        "DriveRoot",
        json!({}),
        ToolExecutionContext::direct(),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.disposition, ToolResultDisposition::Error);
    assert!(
        result
            .summary
            .contains("authorization check reported a denial")
    );
    let wire = serde_json::to_string(&result).unwrap();
    assert!(!wire.contains("resource="));
    assert!(!wire.contains("workspace\""));
    let report: Value = serde_json::from_str(result.content.as_deref().unwrap()).unwrap();
    assert_eq!(report["failure"]["reason"], "authorization_denied");
    assert_eq!(report["result"]["mutation"]["kind"], "not_reported");
    assert!(client.requests.lock().unwrap().is_empty());
}

#[test]
fn drive_tool_public_control_fields_do_not_derive_from_source_prose() {
    use crate::feature::builtin::drive::tool_failure::tool_error;
    for prose in ["private/path and token", "SUCCESS; automatically resend"] {
        let result = ToolResult::from_error(
            "call",
            tool_error(
                failure::DriveClientFailureOperation::Read,
                DriveError::Invalid(prose.into()),
                "",
            ),
        );
        let report: Value = serde_json::from_str(result.content.as_deref().unwrap()).unwrap();
        assert_eq!(report["result"]["error_class"], "invalid_argument");
        assert_eq!(report["result"]["mutation"]["kind"], "not_reported");
        assert_eq!(report["failure"]["reason"], "invalid");
        assert!(!serde_json::to_string(&result).unwrap().contains(prose));
        assert_eq!(result.disposition, ToolResultDisposition::Error);
    }
    let conflict = ToolResult::from_error(
        "call",
        tool_error(
            failure::DriveClientFailureOperation::UpdateText,
            DriveError::Conflict,
            "",
        ),
    );
    let report: Value = serde_json::from_str(conflict.content.as_deref().unwrap()).unwrap();
    assert_eq!(report["result"]["error_class"], "structured_conflict");
    assert_eq!(report["result"]["code"], "drive_conflict");
    assert_eq!(report["result"]["mutation"]["kind"], "not_reported");
    let invalid_identity = ToolResult::from_error(
        "call",
        tool_error(
            failure::DriveClientFailureOperation::UpdateText,
            DriveError::OutcomeUnknown("private/path\nsecret".into()),
            "host-request",
        ),
    );
    assert!(invalid_identity.summary.contains("outcome unknown"));
    assert!(!invalid_identity.summary.contains("private/path"));
}
