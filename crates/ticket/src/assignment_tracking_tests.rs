use super::*;

fn db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    seed_ticket(&conn, "w", "t");
    conn
}

fn seed_ticket(conn: &Connection, workspace: &str, ticket: &str) {
    let legacy: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('typed_tickets') WHERE name='body')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    if legacy {
        conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit) VALUES (?1,?2,?2,'Title','open','task','P2','body','open',1)",params![workspace,ticket]).unwrap();
    } else {
        conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES (?1,?2,?2,'Title','open','task','P2','open',1)",params![workspace,ticket]).unwrap();
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES (?1,?2,-1,?2 || ':-1','comment','body')",params![workspace,ticket]).unwrap();
    }
}

fn assign(conn: &Connection, id: &str, event: &str) {
    assign_principal(conn, id, event, "subject", "subject-a");
}

// Historical schema fixture only: never infer an authority fallback from DB shape.
fn assign_legacy(conn: &Connection, id: &str, event: &str) {
    assign_principal(conn, id, event, "workspace_agent", "workspace-orchestrator");
}

fn assign_principal(conn: &Connection, id: &str, event: &str, kind: &str, principal: &str) {
    let previous: Option<String> = conn.query_row("SELECT assignment_id FROM ticket_current_worker_assignments WHERE workspace_id='w' AND ticket_id='t' AND role='orchestrator'",[],|r|r.get(0)).optional().unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES ('w','t',?1,'orchestrator',?2,?3,'author','2026-10-10T00:00:00Z')",params![id,kind,principal]).unwrap();
    conn.execute("DELETE FROM ticket_current_worker_assignments WHERE workspace_id='w' AND ticket_id='t' AND role='orchestrator'",[]).unwrap();
    conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES ('w','t',?1,'orchestrator',?2,?3,'now')",params![id,kind,principal]).unwrap();
    conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,previous_assignment_id,actor,created_at,operation_id) VALUES ('w','t','orchestrator',?2,?4,?1,?3,'author','now',?2)",params![id,event,previous,if previous.is_some(){"reassigned"}else{"assigned"}]).unwrap();
    conn.execute_batch("COMMIT").unwrap();
}

fn source() -> TicketAssignmentTrackingSourceContext {
    TicketAssignmentTrackingSourceContext {
        actor: "orchestrator".into(),
        runtime_id: "runtime".into(),
        worker_id: "worker".into(),
        subject_id: "subject-a".into(),
        authentication_binding_id: "binding".into(),
    }
}
fn update(
    id: &str,
    event: &str,
    key: &str,
    status: TicketAssignmentTrackingStatus,
) -> TicketAssignmentTrackingUpdate {
    TicketAssignmentTrackingUpdate {
        assignment_id: id.into(),
        expected_last_event_id: event.into(),
        operation_key: key.into(),
        tracking: TicketAssignmentTracking {
            status,
            waiting_reason: None,
            progress: Some("Considering request".into()),
            next_action: None,
            result: Some("Returned alternatives".into()),
        },
    }
}
fn apply(
    conn: &Connection,
    input: &TicketAssignmentTrackingUpdate,
    source: &TicketAssignmentTrackingSourceContext,
) -> Result<TicketAssignmentTrackingProjection> {
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let result =
        sqlite_update_ticket_assignment_tracking_in_transaction(conn, "w", "t", input, source);
    conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
        .unwrap();
    result
}

#[test]
fn assignment_initializes_waiting_and_audit_atomically() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let projection = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    assert!(projection.current);
    assert_eq!(projection.last_event_id.as_deref(), Some("assigned-a"));
    assert_eq!(
        projection.tracking.unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
    let history = sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].event_id, "assigned-a");
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES ('w','t','rolled-back','orchestrator','subject','subject-a','author','now')",[]).unwrap();
    conn.execute_batch("ROLLBACK").unwrap();
    assert!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "rolled-back")
            .unwrap()
            .is_none()
    );
}

#[test]
fn direct_done_close_reopen_and_removal_keep_last_result_and_exact_replay() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let input = update(
        "a",
        "assigned-a",
        "done",
        TicketAssignmentTrackingStatus::Done,
    );
    let result = apply(&conn, &input, &source()).unwrap();
    assert_eq!(
        conn.query_row("SELECT workflow_state FROM typed_tickets", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "open"
    );
    conn.execute("UPDATE typed_tickets SET workflow_state='closed'", [])
        .unwrap();
    let input2 = update(
        "a",
        result.last_event_id.as_deref().unwrap(),
        "correct-after-close",
        TicketAssignmentTrackingStatus::Waiting,
    );
    let corrected = apply(&conn, &input2, &source()).unwrap();
    assert_eq!(
        conn.query_row("SELECT workflow_state FROM typed_tickets", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "closed"
    );
    conn.execute("UPDATE typed_tickets SET workflow_state='open'", [])
        .unwrap();
    assert_eq!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap(),
        corrected
    );
    assign(&conn, "b", "assigned-b");
    assert_eq!(apply(&conn, &input, &source()).unwrap(), result);
    assert_eq!(apply(&conn, &input2, &source()).unwrap(), corrected);
    assert!(
        apply(
            &conn,
            &update(
                "a",
                corrected.last_event_id.as_deref().unwrap(),
                "old-new",
                TicketAssignmentTrackingStatus::Done
            ),
            &source()
        )
        .is_err()
    );
    let old = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    assert!(!old.current);
    assert_eq!(old.tracking, corrected.tracking);
    let b = sqlite_ticket_assignment_tracking(&conn, "w", "t", "b")
        .unwrap()
        .unwrap();
    assert_eq!(
        b.tracking.unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
    conn.execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    assert!(
        apply(
            &conn,
            &update(
                "b",
                "assigned-b",
                "removed-new",
                TicketAssignmentTrackingStatus::Done
            ),
            &source()
        )
        .is_err()
    );
    assert!(
        sqlite_current_ticket_assignment_tracking(&conn, "w", "t")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a")
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn tracking_cas_and_receipts_bind_full_payload_and_authenticated_source() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let input = update(
        "a",
        "assigned-a",
        "update",
        TicketAssignmentTrackingStatus::Inprogress,
    );
    let result = apply(&conn, &input, &source()).unwrap();
    assert_eq!(apply(&conn, &input, &source()).unwrap(), result);
    let mut changed = input.clone();
    changed.tracking.result = Some("Different result".into());
    assert!(apply(&conn, &changed, &source()).is_err());
    changed = input.clone();
    changed.operation_key = "fresh-stale".into();
    assert!(apply(&conn, &changed, &source()).is_err());
    for change in 0..5 {
        let mut sender = source();
        match change {
            0 => sender.actor = "other".into(),
            1 => sender.runtime_id = "other".into(),
            2 => sender.worker_id = "replacement".into(),
            3 => sender.authentication_binding_id = "replacement-binding".into(),
            _ => sender.subject_id = "subject-b".into(),
        };
        assert!(apply(&conn, &input, &sender).is_err());
    }
    let mut other = source();
    other.worker_id = "replacement".into();
    // Backend authenticated the replacement as the current Subject body. A new
    // operation can proceed on the same assignment without starting a new row.
    let next = update(
        "a",
        result.last_event_id.as_deref().unwrap(),
        "replacement-update",
        TicketAssignmentTrackingStatus::Done,
    );
    apply(&conn, &next, &other).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert!(
        sqlite_ticket_assignment_tracking(&conn, "other", "t", "a")
            .unwrap()
            .is_none()
    );
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        sqlite_update_ticket_assignment_tracking_in_transaction(
            &conn,
            "other",
            "t",
            &input,
            &source()
        )
        .is_err()
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert!(
        conn.execute(
            "UPDATE typed_ticket_assignment_tracking_history SET at='tamper'",
            []
        )
        .is_err()
    );
    assert!(
        conn.execute(
            "DELETE FROM typed_ticket_assignment_tracking_operations",
            []
        )
        .is_err()
    );
}

#[test]
fn tracking_reopens_database_without_restarting_or_reassigning_workers() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ticket.db");
    {
        let conn = Connection::open(&path).unwrap();
        migrate_sqlite_ticket_schema(&conn).unwrap();
        seed_ticket(&conn, "w", "t");
        assign(&conn, "a", "assigned-a");
        apply(
            &conn,
            &update(
                "a",
                "assigned-a",
                "done",
                TicketAssignmentTrackingStatus::Done,
            ),
            &source(),
        )
        .unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(
        sqlite_current_ticket_assignment_tracking(&conn, "w", "t").unwrap()[0]
            .tracking
            .as_ref()
            .unwrap()
            .status,
        TicketAssignmentTrackingStatus::Done
    );
    assert_eq!(
        sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a")
            .unwrap()
            .len(),
        2
    );
}

fn legacy(original: &str, fenced: bool, later_case: bool, reassigned: bool) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!("latest_schema.sql"))
        .unwrap();
    conn.execute_batch("CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL); INSERT INTO ticket_schema_migrations VALUES(10,'ticket schema baseline','original-time');").unwrap();
    install_assignment_core(&conn).unwrap();
    seed_ticket(&conn, "w", "t");
    assign_legacy(&conn, "a", "assigned-a");
    let snapshot = TicketLegacyWorkflowSnapshot {
        workflow_state: original.into(),
        workflow_state_explicit: true,
        status: original.into(),
        queued_by: Some("author".into()),
        queued_at: Some("original-queue-time".into()),
        resolution: None,
    };
    if original != "no-snapshot" {
        conn.execute(
            "INSERT INTO typed_ticket_legacy_workflow_snapshot VALUES('w','t',?1)",
            [serde_json::to_string(&snapshot).unwrap()],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,to_state,at,body) VALUES('w','t',1,'state_changed','queued','original-queue-time','Queue')",[]).unwrap();
    if fenced {
        conn.execute("INSERT INTO typed_ticket_event_attributes VALUES('w','t',1,'orchestrator_assignment_id','a')",[]).unwrap();
    }
    if later_case {
        conn.execute("INSERT INTO typed_ticket_case_history VALUES('w','t',0,'closed-later','case-fingerprint','{}')",[]).unwrap();
    }
    if reassigned {
        assign_legacy(&conn, "b", "assigned-b");
    }
    conn
}

#[test]
fn migration_preserves_original_records_and_only_reconciles_current_fenced_delegation() {
    for (original, fenced, later_case, reassigned, expected) in [
        (
            "queued",
            true,
            false,
            false,
            Some(TicketAssignmentTrackingStatus::Waiting),
        ),
        (
            "inprogress",
            true,
            false,
            false,
            Some(TicketAssignmentTrackingStatus::Inprogress),
        ),
        ("queued", false, false, false, None),
        ("queued", true, true, false, None),
        ("inprogress", true, false, true, None),
        ("ready", true, false, false, None),
        ("done", true, false, false, None),
        ("closed", true, false, false, None),
    ] {
        let conn = legacy(original, fenced, later_case, reassigned);
        let original_snapshot: String = conn
            .query_row(
                "SELECT snapshot_json FROM typed_ticket_legacy_workflow_snapshot",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let original_event: String = conn
            .query_row(
                "SELECT event_id FROM ticket_worker_assignment_events ORDER BY event_id LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        migrate_sqlite_ticket_schema(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT applied_at FROM ticket_schema_migrations WHERE version=10",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "original-time"
        );
        assert_eq!(
            conn.query_row(
                "SELECT MAX(version) FROM ticket_schema_migrations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            15
        );
        let value = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap();
        assert_eq!(
            value.tracking.as_ref().map(|t| t.status),
            expected,
            "{original}/{fenced}/{later_case}/{reassigned}"
        );
        assert!(value.migration_diagnostic.is_some());
        assert_eq!(
            conn.query_row(
                "SELECT snapshot_json FROM typed_ticket_legacy_workflow_snapshot",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            original_snapshot
        );
        assert_eq!(
            conn.query_row(
                "SELECT event_id FROM ticket_worker_assignment_events ORDER BY event_id LIMIT 1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            original_event
        );
        let history = sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a").unwrap();
        migrate_sqlite_ticket_schema(&conn).unwrap();
        assert_eq!(
            sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a").unwrap(),
            history
        );
        if reassigned {
            assert!(
                sqlite_ticket_assignment_tracking(&conn, "w", "t", "b")
                    .unwrap()
                    .unwrap()
                    .tracking
                    .is_none()
            );
        }
    }
}

#[test]
fn tracking_schema_rejects_drift_and_duplicate_current_singleton_roles() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    conn.execute("INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','t','b','orchestrator','subject','subject-a','author','now')",[]).unwrap();
    assert!(conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w','t','b','orchestrator','subject','subject-a','now')",[]).is_err());
    conn.execute_batch("DROP TRIGGER typed_ticket_assignment_tracking_history_no_update; CREATE TRIGGER typed_ticket_assignment_tracking_history_no_update BEFORE UPDATE ON typed_ticket_assignment_tracking_history BEGIN SELECT 1; END;").unwrap();
    assert!(
        verify_sqlite_ticket_schema(&conn)
            .unwrap_err()
            .to_string()
            .contains("differs from canonical schema")
    );
    assert!(migrate_sqlite_ticket_schema(&conn).is_err());
}

#[test]
fn tracking_input_wire_contract_rejects_unknown_fields_and_statuses() {
    let input = update(
        "a",
        "event",
        "operation",
        TicketAssignmentTrackingStatus::Inprogress,
    );
    let mut value = serde_json::to_value(&input).unwrap();
    assert_eq!(value["tracking"]["status"], "inprogress");
    value["claimed_role"] = serde_json::json!("orchestrator");
    assert!(serde_json::from_value::<TicketAssignmentTrackingUpdate>(value).is_err());
    let mut value = serde_json::to_value(&input).unwrap();
    value["tracking"]["status"] = serde_json::json!("running");
    assert!(serde_json::from_value::<TicketAssignmentTrackingUpdate>(value).is_err());
}

#[test]
fn standalone_assignment_identity_audit_and_terminal_receipts_are_immutable_but_pending_claims_can_finish()
 {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint) VALUES('w','assigned-op','assign','t','orchestrator','subject','subject-a','a','original-time','original-fingerprint')",[]).unwrap();
    for sql in [
        "UPDATE ticket_worker_assignments SET assignment_id='changed' WHERE assignment_id='a'",
        "UPDATE ticket_worker_assignments SET assigned_by='forged' WHERE assignment_id='a'",
        "UPDATE ticket_worker_assignments SET assigned_at='forged-time' WHERE assignment_id='a'",
        "UPDATE ticket_worker_assignments SET role='owner',principal_kind='user',principal_id='user' WHERE assignment_id='a'",
        "DELETE FROM ticket_worker_assignments WHERE assignment_id='a'",
        "UPDATE ticket_worker_assignment_events SET actor='forged'",
        "UPDATE ticket_worker_assignment_events SET event_id='forged'",
        "DELETE FROM ticket_worker_assignment_events",
        "UPDATE ticket_assignment_operations SET request_fingerprint='forged' WHERE operation_id='assigned-op'",
        "UPDATE ticket_assignment_operations SET created_at='forged' WHERE operation_id='assigned-op'",
        "DELETE FROM ticket_assignment_operations WHERE operation_id='assigned-op'",
        "UPDATE ticket_current_worker_assignments SET principal_id='another-agent'",
        "UPDATE ticket_worker_assignments SET tracking_json='{}' WHERE assignment_id='a'",
    ] {
        assert!(
            conn.execute(sql, []).is_err(),
            "deny direct mutation: {sql}"
        );
    }
    // The immutable original principal also fences insertion of a mismatched
    // current row, even in a standalone Ticket store without host constraints.
    conn.execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    assert!(conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w','t','a','orchestrator','subject','subject-b','now')",[]).is_err());
    conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,runtime_id,worker_id,created_at,request_fingerprint,claim_state) VALUES('w','pending','assign','t','worker','worker','runtime','pending-worker','claim-time','claim-fingerprint','pending')",[]).unwrap();
    conn.execute("UPDATE ticket_assignment_operations SET worker_id='resolved-worker',binding_recovery_json='{}' WHERE operation_id='pending'",[]).unwrap();
    conn.execute("UPDATE ticket_assignment_operations SET claim_state='failed',failure_reason='Admission failed' WHERE operation_id='pending'",[]).unwrap();
    assert!(conn.execute("UPDATE ticket_assignment_operations SET failure_reason='rewrite' WHERE operation_id='pending'",[]).is_err());
    assert!(
        conn.execute(
            "DELETE FROM ticket_assignment_operations WHERE operation_id='pending'",
            []
        )
        .is_err()
    );
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn release_projection_preserves_actor_time_reason_and_optional_legacy_tracking() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let result = apply(
        &conn,
        &update(
            "a",
            "assigned-a",
            "result",
            TicketAssignmentTrackingStatus::Done,
        ),
        &source(),
    )
    .unwrap();
    assign(&conn, "b", "assigned-b");
    let old = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    let release = old.release.as_ref().unwrap();
    assert_eq!(release.action, TicketAssignmentReleaseAction::Replaced);
    assert_eq!(release.replacement_assignment_id.as_deref(), Some("b"));
    assert_eq!(release.last_tracking, result.tracking);
    assert_eq!(release.actor, "author");
    assert_eq!(release.event_id, "assigned-b");
    conn.execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    conn.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,previous_assignment_id,actor,created_at,operation_id,reason) VALUES('w','t','orchestrator','unassign-b','unassigned','b','human','original-unassign-time','unassign-op','Withdrawn')",[]).unwrap();
    let removed = sqlite_ticket_assignment_tracking(&conn, "w", "t", "b")
        .unwrap()
        .unwrap();
    let release = removed.release.unwrap();
    assert_eq!(release.action, TicketAssignmentReleaseAction::Unassigned);
    assert_eq!(release.at, "original-unassign-time");
    assert_eq!(release.reason.as_deref(), Some("Withdrawn"));
    assert_eq!(
        release.last_tracking.as_ref().unwrap().status,
        TicketAssignmentTrackingStatus::Waiting
    );
    assert!(conn.execute("UPDATE ticket_worker_assignments SET tracking_json='{\"status\":\"done\"}' WHERE assignment_id='b'",[]).is_err());
    assert!(conn.execute("INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('w','t','b','orchestrator','subject','subject-a','now')",[]).is_err());
    let legacy = legacy("done", true, false, false);
    migrate_sqlite_ticket_schema(&legacy).unwrap();
    legacy
        .execute("DELETE FROM ticket_current_worker_assignments", [])
        .unwrap();
    legacy.execute("INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,previous_assignment_id,actor,created_at,operation_id) VALUES('w','t','orchestrator','unassign-legacy','unassigned','a','human','legacy-unassign-time','legacy-unassign-op')",[]).unwrap();
    let original = sqlite_ticket_assignment_tracking(&legacy, "w", "t", "a")
        .unwrap()
        .unwrap();
    assert!(original.tracking.is_none());
    assert!(original.release.unwrap().last_tracking.is_none());
}

fn routing_queue(conn: &Connection, index: i64, fence: &str) {
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,from_state,to_state,at,state_field,reason,body) VALUES('w','t',?1,'state_changed','open','open','typed-queue-time','routing','queued','Explicit component-10 Queue')",[index]).unwrap();
    conn.execute("INSERT INTO typed_ticket_event_attributes VALUES('w','t',?1,'orchestrator_assignment_id',?2)",params![index,fence]).unwrap();
}

#[test]
fn migration_recognizes_typed_case_neutral_queue_and_never_revives_an_older_mismatched_queue() {
    for original in ["ready", "no-snapshot"] {
        let conn = legacy(original, true, false, false);
        routing_queue(&conn, 2, "a");
        migrate_sqlite_ticket_schema(&conn).unwrap();
        let assignment = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap();
        assert_eq!(
            assignment.tracking.as_ref().unwrap().status,
            TicketAssignmentTrackingStatus::Waiting
        );
        assert_eq!(assignment.updated_at.as_deref(), Some("typed-queue-time"));
    }
    let conn = legacy("queued", true, false, false);
    routing_queue(&conn, 2, "mismatched-assignment");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap()
            .tracking
            .is_none()
    );
    let conn = legacy("no-snapshot", true, false, false);
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap()
            .tracking
            .is_none()
    );
}

#[test]
fn migration_accepts_new_explicit_queue_after_ordered_case_judgments_but_not_after_a_later_case() {
    for queue_after_case in [true, false] {
        let conn = legacy("queued", true, false, false);
        for (ordinal, index, action, state) in [
            (
                0,
                2,
                TicketCaseAction::Close {
                    reason: TicketClosureReason::Unknown,
                },
                "closed",
            ),
            (1, 3, TicketCaseAction::Reopen, "open"),
        ] {
            let key = format!("case-{ordinal}");
            let judgment = TicketCaseHistoryEntry {
                operation_key: key.clone(),
                previous_closure_operation_key: if ordinal == 1 {
                    Some("case-0".into())
                } else {
                    None
                },
                action,
                explanation: "Original case decision".into(),
                references: vec![],
                author: "human".into(),
                at: format!("case-time-{ordinal}"),
            };
            conn.execute("INSERT INTO typed_ticket_case_history VALUES('w','t',?1,?2,'original-fingerprint',?3)",params![ordinal,key,serde_json::to_string(&judgment).unwrap()]).unwrap();
            conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,state_field,to_state,at,body) VALUES('w','t',?1,'state_changed','case',?2,?3,'Original case decision')",params![index,state,judgment.at]).unwrap();
            conn.execute(
                "INSERT INTO typed_ticket_event_attributes VALUES('w','t',?1,'operation_key',?2)",
                params![index, key],
            )
            .unwrap();
        }
        if queue_after_case {
            routing_queue(&conn, 4, "a");
        }
        let original_history: i64 = conn
            .query_row("SELECT COUNT(*) FROM typed_ticket_case_history", [], |r| {
                r.get(0)
            })
            .unwrap();
        migrate_sqlite_ticket_schema(&conn).unwrap();
        let assignment = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap();
        assert_eq!(assignment.tracking.is_some(), queue_after_case);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM typed_ticket_case_history", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
            original_history
        );
    }
}

#[test]
fn standalone_assignment_constraints_reject_cross_ticket_principals_or_orphan_audit_and_operations()
{
    let conn = db();
    assign(&conn, "a", "assigned-a");
    for sql in [
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('other','t','cross-workspace','orchestrator','subject','subject-a','author','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','t','bad-orchestrator','orchestrator','user','user','author','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,assigned_by,assigned_at) VALUES('w','t','bad-owner','owner','worker','r','worker','author','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','t','bad-worker','worker','subject','subject-a','author','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,assigned_by,assigned_at) VALUES('w','t','missing-worker-identity','worker','worker','author','now')",
        "INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at) VALUES('other','t','orchestrator','cross-audit','assigned','a','author','now')",
        "INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at) VALUES('w','missing','orchestrator','cross-ticket-audit','assigned','a','author','now')",
        "INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,created_at) VALUES('other','orphan-operation','assign','t','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at,tracking_json) VALUES('w','t','bad-status','orchestrator','subject','subject-a','author','now','{\"status\":\"running\"}')",
    ] {
        assert!(conn.execute(sql, []).is_err(), "reject {sql}");
    }
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM ticket_worker_assignment_events",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn schema_verification_does_not_normalize_away_case_sensitive_authority_literals() {
    let conn = db();
    let sql:String=conn.query_row("SELECT sql FROM sqlite_schema WHERE name='typed_ticket_assignment_orchestrator_principal_insert'",[],|r|r.get(0)).unwrap();
    conn.execute_batch("DROP TRIGGER typed_ticket_assignment_orchestrator_principal_insert")
        .unwrap();
    conn.execute_batch(&sql.replace("NEW.role='orchestrator'", "NEW.role='Orchestrator'"))
        .unwrap();
    assert!(
        verify_sqlite_ticket_schema(&conn)
            .unwrap_err()
            .to_string()
            .contains("differs from canonical schema")
    );
}

#[test]
fn assignment_answer_commits_one_audited_input_and_exact_receipt_without_tracking_mutation() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let answer = TicketAssignmentAnswer {
        assignment_id: "a".into(),
        expected_last_event_id: "assigned-a".into(),
        operation_key: "answer-a".into(),
        body: "Continue with B".into(),
    };
    let source = serde_json::json!({"kind":"account","account_id":"account","user_id":"human"});
    let apply = |answer: &TicketAssignmentAnswer| {
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        let result = sqlite_answer_ticket_assignment_in_transaction(
            &conn, "w", "t", answer, &source, "human",
        );
        conn.execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" })
            .unwrap();
        result
    };
    let result = apply(&answer).unwrap();
    assert_eq!(result.input_event_id, "ticket-event:0");
    assert_eq!(apply(&answer).unwrap(), result);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM typed_ticket_events WHERE event_index >= 0",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT body FROM typed_ticket_events WHERE event_index >= 0",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        answer.body
    );
    assert_eq!(conn.query_row("SELECT value FROM typed_ticket_event_attributes WHERE key='orchestrator_assignment_id'",[],|r| r.get::<_,String>(0)).unwrap(),"a");
    assert_eq!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap()
            .last_event_id
            .as_deref(),
        Some("assigned-a")
    );
    let mut altered = answer.clone();
    altered.body = "Changed".into();
    assert!(apply(&altered).is_err());
    assign(&conn, "b", "assigned-b");
    assert_eq!(apply(&answer).unwrap(), result);
    altered.operation_key = "fresh-stale".into();
    assert!(apply(&altered).is_err());
}

#[test]
fn assignment_answer_rejects_unbounded_body_and_malformed_identities_without_appending_input() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let original = TicketAssignmentAnswer {
        assignment_id: "a".into(),
        expected_last_event_id: "assigned-a".into(),
        operation_key: "answer-a".into(),
        body: "Answer".into(),
    };
    for field in [
        "body-large",
        "body-nul",
        "body-empty",
        "identity-control",
        "event-empty",
    ] {
        let mut answer = original.clone();
        match field {
            "body-large" => answer.body = "a".repeat(16385),
            "body-nul" => answer.body = "answer\0hidden".into(),
            "body-empty" => answer.body = "   ".into(),
            "identity-control" => answer.operation_key = "key\n".into(),
            _ => answer.expected_last_event_id = "".into(),
        }
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(
            sqlite_answer_ticket_assignment_in_transaction(
                &conn,
                "w",
                "t",
                &answer,
                &serde_json::json!({"actor":"source"}),
                "human"
            )
            .is_err(),
            "{field}"
        );
        conn.execute_batch("ROLLBACK").unwrap();
    }
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM typed_ticket_events WHERE event_index >= 0",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

// Frozen component13 composition, not a latest-schema fixture relabelled as old.
fn legacy_v13() -> Connection {
    let conn = legacy("no-snapshot", false, false, false);
    migrate_assignment_tracking(&conn).unwrap();
    conn.execute_batch(include_str!("post_migration.sql"))
        .unwrap();
    conn.execute_batch(include_str!("content_changes.sql"))
        .unwrap();
    conn.execute_batch(
        "INSERT INTO ticket_schema_migrations VALUES
        (11,'Ticket-owned assignment tracking','v11-time'),
        (12,'Canonical Ticket posts','v12-time'),
        (13,'Ticket content change facts','v13-time');",
    )
    .unwrap();
    conn
}

#[test]
fn subject_migration_preserves_legacy_audit_and_receipts_without_inventing_subject_authority() {
    let conn = legacy_v13();
    let tracking = r#"{"status":"done","waiting_reason":null,"progress":null,"next_action":null,"result":"Historical result"}"#;
    let old_source = r#"{"actor":"old-actor","runtime_id":"old-runtime","worker_id":"old-worker","agent_key":"workspace-orchestrator","authentication_binding_id":"old-binding"}"#;
    conn.execute("UPDATE ticket_worker_assignments SET tracking_json=?1,last_tracking_event_id='old-event',tracking_updated_at='old-time' WHERE assignment_id='a'",[tracking]).unwrap();
    conn.execute("INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','t','a','old-event','old-op',?1,?2,'old-time')",params![tracking,old_source]).unwrap();
    let before = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    let history = sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a").unwrap();
    // Pre-Subject saved results remain byte-identical and are not new requests.
    let mut old_input = update(
        "a",
        "assigned-a",
        "old-op",
        TicketAssignmentTrackingStatus::Done,
    );
    old_input.tracking = before.tracking.clone().unwrap();
    conn.execute(
        "INSERT INTO typed_ticket_assignment_tracking_operations VALUES('w','old-op','t',?1,?2,?3)",
        params![
            serde_json::to_string(&("t", &old_input)).unwrap(),
            old_source,
            serde_json::to_string(&before).unwrap()
        ],
    )
    .unwrap();
    conn.execute("INSERT INTO ticket_assignment_operations(workspace_id,operation_id,action,ticket_id,role,principal_kind,principal_id,assignment_id,created_at,request_fingerprint) VALUES('w','legacy-assign','assign','t','orchestrator','workspace_agent','workspace-orchestrator','a','old-time','old-fingerprint')",[]).unwrap();
    let receipts = |db: &Connection| -> (String, String) {
        db.query_row("SELECT source_context_json,result_json FROM typed_ticket_assignment_tracking_operations WHERE operation_key='old-op'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap()
    };
    let saved_receipt = receipts(&conn);
    migrate_sqlite_ticket_schema(&conn).unwrap();
    let after = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    assert_eq!(
        after.principal,
        TicketAssignmentPrincipalSnapshot::WorkspaceAgent {
            agent_key: "workspace-orchestrator".into()
        }
    );
    assert_eq!(after.tracking, before.tracking);
    assert_eq!(after.last_event_id, before.last_event_id);
    assert_eq!(after.updated_at, before.updated_at);
    assert!(after.current);
    assert!(
        after
            .migration_diagnostic
            .as_ref()
            .unwrap()
            .contains("nonoperational")
    );
    assert_eq!(
        sqlite_ticket_assignment_tracking_history(&conn, "w", "t", "a").unwrap(),
        history
    );
    assert_eq!(
        history.last().unwrap().source_context["agent_key"],
        "workspace-orchestrator"
    );
    assert_eq!(receipts(&conn), saved_receipt);
    assert_eq!(conn.query_row("SELECT principal_kind FROM ticket_assignment_operations WHERE operation_id='legacy-assign'",[],|r|r.get::<_,String>(0)).unwrap(),"workspace_agent");
    // Neither a random Subject nor spelling the old singleton as a Subject owns it.
    for subject in ["subject-a", "workspace-orchestrator"] {
        let mut sender = source();
        sender.actor = "old-actor".into();
        sender.runtime_id = "old-runtime".into();
        sender.worker_id = "old-worker".into();
        sender.authentication_binding_id = "old-binding".into();
        sender.subject_id = subject.into();
        assert!(
            apply(
                &conn,
                &update(
                    "a",
                    "old-event",
                    "new-op",
                    TicketAssignmentTrackingStatus::Inprogress
                ),
                &sender
            )
            .is_err()
        );
        // Exact historical input and concrete sender still cannot reinterpret
        // old agent_key JSON as a new Subject-authenticated receipt.
        assert!(apply(&conn, &old_input, &sender).is_err());
    }
    for sql in [
        "UPDATE ticket_worker_assignments SET tracking_updated_at='new-time' WHERE assignment_id='a'",
        "INSERT INTO typed_ticket_assignment_tracking_history VALUES('w','t','a','new-event','new-op','{\"status\":\"done\"}','{}','now')",
        "INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('w','t','legacy-new','orchestrator','workspace_agent','workspace-orchestrator','actor','now')",
    ] {
        assert!(conn.execute(sql, []).is_err(), "reject {sql}");
    }
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(
        sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap(),
        after
    );
    // Explicit reissue is a new Subject identity, not a conversion of old audit.
    assign(&conn, "subject-reissue", "subject-reissue-event");
    apply(
        &conn,
        &update(
            "subject-reissue",
            "subject-reissue-event",
            "subject-update",
            TicketAssignmentTrackingStatus::Done,
        ),
        &source(),
    )
    .unwrap();
    assert_eq!(receipts(&conn), saved_receipt);
    assert!(
        !sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
            .unwrap()
            .unwrap()
            .current
    );
    let wire = serde_json::to_value(source()).unwrap();
    assert_eq!(wire["subject_id"], "subject-a");
    assert!(wire.get("agent_key").is_none());
    // Old serialized principal snapshots and audit JSON are still readable as old types.
    let old: TicketAssignmentTrackingProjection = serde_json::from_str(&saved_receipt.1).unwrap();
    assert_eq!(old.principal, before.principal);
}

#[test]
fn subject_owner_is_required_for_new_operations_and_worker_replacement_is_not_reassignment() {
    let conn = db();
    assign(&conn, "a", "assigned-a");
    let input = update(
        "a",
        "assigned-a",
        "fresh",
        TicketAssignmentTrackingStatus::Inprogress,
    );
    let mut wrong_subject = source();
    wrong_subject.subject_id = "subject-b".into();
    assert!(apply(&conn, &input, &wrong_subject).is_err());
    let projection = sqlite_ticket_assignment_tracking(&conn, "w", "t", "a")
        .unwrap()
        .unwrap();
    assert_eq!(
        projection.principal,
        TicketAssignmentPrincipalSnapshot::Subject {
            subject_id: "subject-a".into()
        }
    );
    let mut replacement = source();
    replacement.worker_id = "new-body".into();
    replacement.authentication_binding_id = "new-binding".into();
    let result = apply(&conn, &input, &replacement).unwrap();
    assert_eq!(result.assignment_id, "a");
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM typed_ticket_assignment_tracking_operations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn failed_subject_migration_rolls_back_schema_and_receipt_but_preserves_host_transaction() {
    let conn = legacy_v13();
    let original: String = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='ticket_worker_assignments'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch("CREATE TRIGGER host_reject_subject_migration BEFORE UPDATE OF tracking_migration_diagnostic ON ticket_worker_assignments BEGIN SELECT RAISE(ABORT,'simulated failure after CHECK widening'); END;").unwrap();
    conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE host_pending(value TEXT); INSERT INTO host_pending VALUES('keep');").unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&conn).is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(
        conn.query_row("SELECT value FROM host_pending", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        conn.query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        13
    );
    assert_eq!(
        conn.query_row(
            "SELECT sql FROM sqlite_schema WHERE name='ticket_worker_assignments'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        original
    );
    assert!(
        !conn
            .pragma_query_value(None, "writable_schema", |r| r.get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='ticket_subject_current_insert'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TRIGGER host_reject_subject_migration;")
        .unwrap();
    migrate_sqlite_ticket_schema_in_transaction(&conn).unwrap();
    conn.execute_batch("COMMIT").unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn subject_generation_is_ledger_selected_and_never_repaired_by_a_marker_trigger() {
    let conn = legacy_v13();
    conn.execute_batch("CREATE TRIGGER ticket_subject_current_insert BEFORE INSERT ON ticket_current_worker_assignments BEGIN SELECT 1; END;").unwrap();
    assert!(migrate_sqlite_ticket_schema(&conn).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        13
    );
    let conn = db();
    conn.execute_batch("DROP TRIGGER ticket_subject_current_insert; CREATE TRIGGER ticket_subject_current_insert BEFORE INSERT ON ticket_current_worker_assignments BEGIN SELECT 1; END;").unwrap();
    assert!(verify_sqlite_ticket_schema(&conn).is_err());
    assert!(migrate_sqlite_ticket_schema(&conn).is_err());
}
