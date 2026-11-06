//! Workdir Tool decoration contracts. Registered production session/Tool/history
//! integration is covered by worker's manage_workdir audit_tool_tests.
use super::*;
use agen::tool::{Attachment, ImageAttachment, ToolResult, ToolResultDisposition};
use workdir::http::{WorkdirAuditFailureReason, WorkdirAuditOperationOutcome, WorkdirAuditStage};

fn receipt(outcome: WorkdirAuditOperationOutcome) -> WorkdirAuditFailure {
    WorkdirAuditFailure {
        operation_id: "/private/provider/token=secret".into(),
        stage: WorkdirAuditStage::ResponseReceived,
        reason: WorkdirAuditFailureReason::AuditStoreUnavailable,
        operation_outcome: outcome,
    }
}
fn original() -> ToolOutput {
    ToolOutput {
        summary: "original result".into(),
        content: Some("original content".into()),
        attachments: vec![Attachment::Image(ImageAttachment::new(
            "image/png",
            vec![1, 2, 3],
        ))],
    }
}

#[test]
fn over_budget_secondary_report_keeps_primary_content_attachments_and_success() {
    let before = original();
    let output = finish(
        Ok(before.clone()),
        &[receipt(WorkdirAuditOperationOutcome::Succeeded)],
        1,
    )
    .unwrap();
    assert_eq!(output.content, before.content);
    assert_eq!(output.attachments, before.attachments);
    assert!(output.summary.starts_with(&before.summary));
    assert!(output.summary.contains("do not resend"));
    assert!(output.summary.contains("details unavailable"));
    let result = ToolResult::from_output("call", output);
    assert_eq!(result.disposition, ToolResultDisposition::Success);
    assert!(!result.is_error);
}

#[test]
fn secondary_audit_cannot_override_error_cancellation_or_interruption() {
    for (error, disposition, class) in [
        (
            ToolError::InvalidArgument("original refusal".into()),
            ToolResultDisposition::Error,
            "invalid_argument",
        ),
        (
            ToolError::ExecutionFailed("original unknown; inspect before retry".into()),
            ToolResultDisposition::Error,
            "execution_failed",
        ),
        (
            ToolError::StructuredConflict {
                code: "checkout_stale".into(),
                message: "original conflict".into(),
            },
            ToolResultDisposition::Error,
            "structured_conflict",
        ),
        (
            ToolError::Cancelled(original()),
            ToolResultDisposition::Cancelled,
            "cancelled",
        ),
        (
            ToolError::Interrupted(original()),
            ToolResultDisposition::Interrupted,
            "interrupted",
        ),
    ] {
        let error = finish(
            Err(error),
            &[receipt(WorkdirAuditOperationOutcome::Unknown)],
            65536,
        )
        .unwrap_err();
        let result = ToolResult::from_error("call", error);
        assert_eq!(result.disposition, disposition);
        assert!(result.is_error);
        assert!(result.summary.contains("original"));
        assert!(
            result
                .summary
                .contains("inspect effects before considering a retry")
        );
        let (_, report) = result
            .content
            .as_deref()
            .unwrap()
            .rsplit_once("\n\nWorkdir audit report:\n")
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(report).unwrap();
        assert_eq!(report["result"]["primary_error_class"], class);
        assert_eq!(
            report["result"]["audit_failures"][0]["operation_outcome"]["kind"],
            "unknown"
        );
        if class == "structured_conflict" {
            assert_eq!(report["result"]["primary_conflict_code"], "checkout_stale");
        }
    }
}

struct InterleavedTool {
    outcome: WorkdirAuditOperationOutcome,
    result: &'static str,
}
#[async_trait]
impl Tool for InterleavedTool {
    async fn execute(&self, _: &str, _: ToolExecutionContext) -> Result<ToolOutput, ToolError> {
        workdir::audit::notify(receipt(self.outcome));
        tokio::task::yield_now().await;
        Ok(ToolOutput {
            summary: self.result.into(),
            content: None,
            attachments: Vec::new(),
        })
    }
}

#[tokio::test]
async fn interleaved_calls_keep_their_own_receipts_and_never_disclose_received_identity() {
    let left = with_receipts(Arc::new(InterleavedTool {
        outcome: WorkdirAuditOperationOutcome::Succeeded,
        result: "left-result",
    }));
    let right = with_receipts(Arc::new(InterleavedTool {
        outcome: WorkdirAuditOperationOutcome::Unknown,
        result: "right-result",
    }));
    let (left, right) = tokio::join!(
        left.execute("{}", ToolExecutionContext::default()),
        right.execute("{}", ToolExecutionContext::default())
    );
    for (output, label, expected) in [
        (left.unwrap(), "left-result", "succeeded"),
        (right.unwrap(), "right-result", "unknown"),
    ] {
        assert!(output.summary.starts_with(label));
        let (_, report) = output
            .content
            .as_deref()
            .unwrap()
            .rsplit_once("\n\nWorkdir audit report:\n")
            .unwrap();
        let report: serde_json::Value = serde_json::from_str(report).unwrap();
        assert_eq!(
            report["result"]["audit_failures"].as_array().unwrap().len(),
            1
        );
        assert_eq!(
            report["result"]["audit_failures"][0]["operation_outcome"]["kind"],
            expected
        );
        assert!(
            !serde_json::to_string(&output)
                .unwrap()
                .contains("token=secret")
        );
    }
}
