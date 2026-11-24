use super::*;
use serde_json::json;

fn checkpoint(payload: serde_json::Value) -> Vec<session_store::LogEntry> {
    vec![session_store::LogEntry::Extension {
        ts: 1,
        domain: "worker.pending_activations.v1".into(),
        payload,
    }]
}

fn empty_queue() -> serde_json::Value {
    json!({
        "activating": null,
        "activating_notification": [],
        "pending": [],
        "pending_notifications": [],
        "invocation_recovery_blocked": false,
    })
}

#[test]
fn saved_notifications_block_release_but_not_stopped_removal() {
    for (field, value) in [
        ("pending_notifications", json!([{"message": "unconsumed"}])),
        ("activating_notification", json!([{"message": "staged"}])),
        // The saved single-notification checkpoint is also still readable.
        ("activating_notification", json!({"message": "staged"})),
    ] {
        let mut queue = empty_queue();
        queue[field] = value;
        let entries = checkpoint(queue);
        assert!(ensure_inputs_settled(&entries, RecordingScopePurpose::Removal).is_ok());
        assert!(matches!(
            ensure_inputs_settled(&entries, RecordingScopePurpose::SubjectRelease),
            Err(RuntimeError::RecordingExecutionUnconfirmed { .. })
        ));
    }
    assert!(
        ensure_inputs_settled(
            &checkpoint(empty_queue()),
            RecordingScopePurpose::SubjectRelease,
        )
        .is_ok()
    );
}

#[test]
fn removal_still_requires_submit_and_invocation_settlement_and_readable_checkpoint() {
    for (field, value) in [
        ("activating", json!({"submission_id": "active"})),
        ("pending", json!([{"submission_id": "queued"}])),
        ("invocation_recovery_blocked", json!(true)),
        ("pending", json!("corrupt queue")),
    ] {
        let mut queue = empty_queue();
        queue["pending_notifications"] = json!([{"message": "unconsumed"}]);
        queue[field] = value;
        let entries = checkpoint(queue);
        for purpose in [
            RecordingScopePurpose::Removal,
            RecordingScopePurpose::SubjectRelease,
        ] {
            assert!(
                matches!(
                    ensure_inputs_settled(&entries, purpose),
                    Err(RuntimeError::RecordingExecutionUnconfirmed { .. })
                ),
                "must retain source for {field}"
            );
        }
    }
}
