use server_api::{SetTicketRoleAssignmentRequest, TicketAssignmentPrincipal, TicketQueryRequest};

#[test]
fn subject_assignment_and_query_use_the_same_exact_principal_identity() {
    for subject_id in ["subject-a", "subject-b"] {
        let principal = serde_json::json!({ "kind": "subject", "subject_id": subject_id });
        let assign: SetTicketRoleAssignmentRequest = serde_json::from_value(serde_json::json!({
            "operation_id": "assign-subject", "expected_assignment_id": null, "principal": principal
        }))
        .unwrap();
        assert!(
            matches!(assign.principal, TicketAssignmentPrincipal::Subject { subject_id: ref id } if id == subject_id)
        );
        let query: TicketQueryRequest = serde_json::from_value(serde_json::json!({
            "assigned_principal": principal
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(query.assigned_principal).unwrap(),
            principal
        );
        assert_eq!(serde_json::to_value(assign.principal).unwrap(), principal);
    }
}

#[test]
fn assignment_principal_keeps_legacy_audit_and_rejects_subject_identity_spoofing() {
    for principal in [
        serde_json::json!({ "kind": "user", "account_id": "alice" }),
        serde_json::json!({ "kind": "worker", "runtime_id": "runtime", "worker_id": "worker" }),
        serde_json::json!({ "kind": "workspace_agent", "agent_key": "workspace-orchestrator" }),
    ] {
        let retained: TicketAssignmentPrincipal =
            serde_json::from_value(principal.clone()).unwrap();
        assert_eq!(serde_json::to_value(retained).unwrap(), principal);
    }
    for malformed in [
        serde_json::json!({ "kind": "subject" }),
        serde_json::json!({ "kind": "subject", "subject_id": 1 }),
        serde_json::json!({ "kind": "subject", "subject_id": "a", "worker_id": "old-body" }),
        serde_json::json!({ "kind": "subject", "subject_id": "a", "agent_key": "workspace-orchestrator" }),
    ] {
        assert!(serde_json::from_value::<TicketAssignmentPrincipal>(malformed).is_err());
    }
}
