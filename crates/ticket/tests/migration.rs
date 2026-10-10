//! Adopted compatibility contract: canonical standalone schema7 six-state snapshots
//! migrate independently to schema9, preserving original evidence and routing handoff.
use rusqlite::{Connection, params};
use tempfile::TempDir;
use ticket::*;

fn schema7(connection: &Connection) {
    connection
        .execute_batch(include_str!("fixtures/schema7.sql"))
        .unwrap();
    connection.execute_batch("CREATE TABLE ticket_schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL);
        INSERT INTO ticket_schema_migrations VALUES (7, 'ticket schema baseline', 'legacy');").unwrap();
}

#[test]
fn standalone_schema7_migration_preserves_six_state_snapshot_and_evidence_without_success_inference()
 {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("tickets.db");
    let connection = Connection::open(&path).unwrap();
    schema7(&connection);
    connection
        .execute_batch("CREATE TABLE legacy_worker_status (ticket_id TEXT, status TEXT);")
        .unwrap();
    for state in [
        "planning",
        "ready",
        "queued",
        "inprogress",
        "done",
        "closed",
    ] {
        connection.execute("INSERT INTO typed_tickets (workspace_id, ticket_id, slug, title, status, kind, priority, body, workflow_state, workflow_state_explicit, assignee, queued_by, queued_at, resolution) VALUES ('workspace', ?1, ?1, ?1, 'open', 'task', 'P2', 'criteria', ?1, 1, 'retained-assignee', 'legacy-router', 'legacy-queue-time', 'Original decision text')", params![state]).unwrap();
        connection.execute("INSERT INTO typed_ticket_events (workspace_id, ticket_id, event_index, kind, from_state, to_state, reason, author, body) VALUES ('workspace', ?1, 0, 'state_changed', 'planning', ?1, 'original reason', 'legacy-actor', 'legacy body')", params![state]).unwrap();
        connection.execute("INSERT INTO typed_ticket_event_attributes VALUES ('workspace', ?1, 0, 'operation_key', 'legacy-receipt')", params![state]).unwrap();
        connection.execute("INSERT INTO typed_ticket_event_attributes VALUES ('workspace', ?1, 0, 'request_fingerprint', 'legacy-fingerprint')", params![state]).unwrap();
        // Actual pre-content receipt spelling: preserve it as historical input,
        // never promote an old counter fingerprint into a current content CAS.
        connection.execute("INSERT INTO typed_ticket_event_attributes VALUES ('workspace', ?1, 0, 'expected_content_digest', 'old-input')", params![state]).unwrap();
        connection.execute(
            "UPDATE typed_ticket_event_attributes SET key='expected_item_revision',value='T:0' WHERE ticket_id=?1 AND key='expected_content_digest'",
            [state],
        ).unwrap();
        connection.execute("INSERT INTO typed_ticket_event_references VALUES ('workspace', ?1, 0, 0, 'merge_request', 'MR-existing')", params![state]).unwrap();
        connection.execute("INSERT INTO typed_ticket_targets VALUES ('workspace', ?1, 0, 'main', 'develop', 'read_write')", params![state]).unwrap();
        connection
            .execute(
                "INSERT INTO legacy_worker_status VALUES (?1, 'released')",
                params![state],
            )
            .unwrap();
    }
    // Snapshot evidence before migration, including untyped `done` and existing MR references.
    let historical_events: Vec<_> = connection.prepare("SELECT ticket_id, from_state, to_state, reason, author, body FROM typed_ticket_events ORDER BY ticket_id").unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))).unwrap().collect::<std::result::Result<_, _>>().unwrap();
    let old_attributes: Vec<(String, String, String)> = connection
        .prepare(
            "SELECT ticket_id,key,value FROM typed_ticket_event_attributes ORDER BY ticket_id,key",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    migrate_sqlite_ticket_schema(&connection).unwrap();
    let preserved_attributes: Vec<(String, String, String)> = connection
        .prepare(
            "SELECT ticket_id,key,value FROM typed_ticket_event_attributes ORDER BY ticket_id,key",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(old_attributes, preserved_attributes);
    verify_sqlite_ticket_schema(&connection).unwrap();
    let marker: i64 = connection
        .query_row("SELECT version FROM ticket_schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(marker, 9);
    let after_events: Vec<_> = connection.prepare("SELECT ticket_id, from_state, to_state, reason, author, body FROM typed_ticket_events ORDER BY ticket_id").unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))).unwrap().collect::<std::result::Result<_, _>>().unwrap();
    assert_eq!(after_events, historical_events);
    let store = SqliteTicketBackend::open_verified(&path, "workspace").unwrap();
    for state in [
        "planning",
        "ready",
        "queued",
        "inprogress",
        "done",
        "closed",
    ] {
        let ticket = store.show(state.into()).unwrap();
        let expected_state = if matches!(state, "done" | "closed") {
            TicketState::Closed
        } else {
            TicketState::Open
        };
        assert_eq!(ticket.meta.workflow_state, expected_state);
        let old_key_request = TicketCaseUpdate {
            expected_content_digest: ticket_content_digest(&ticket),
            expected_state,
            expected_closure_operation_key: ticket.closure_operation_key.clone(),
            operation_key: "legacy-receipt".into(),
            action: if expected_state == TicketState::Closed {
                TicketCaseAction::CorrectClosure {
                    reason: TicketClosureReason::Unknown,
                }
            } else {
                TicketCaseAction::Close {
                    reason: TicketClosureReason::Unknown,
                }
            },
            explanation: "Cannot reinterpret a frozen operation receipt".into(),
            references: vec![],
            author: Some("human".into()),
        };
        assert!(matches!(
            store.update_case(state, old_key_request),
            Err(TicketError::OperationFingerprintMismatch { .. })
        ));
        assert_eq!(store.show(state.into()).unwrap(), ticket);
        let legacy = ticket.legacy_workflow_snapshot.as_ref().unwrap();
        assert_eq!(legacy.workflow_state, state);
        assert!(legacy.workflow_state_explicit);
        assert_eq!(legacy.status, "open");
        assert_eq!(legacy.queued_by.as_deref(), Some("legacy-router"));
        assert_eq!(legacy.queued_at.as_deref(), Some("legacy-queue-time"));
        assert_eq!(legacy.resolution.as_deref(), Some("Original decision text"));
        assert_eq!(ticket.meta.assignee.as_deref(), Some("retained-assignee"));
        assert_eq!(ticket.meta.targets[0].repository_key, "main");
        assert_eq!(ticket.events[0].from.as_deref(), Some("planning"));
        assert_eq!(ticket.events[0].to.as_deref(), Some(state));
        assert_eq!(ticket.events[0].references[0].target, "MR-existing");
        assert_eq!(
            ticket.events[0].attributes["request_fingerprint"],
            "legacy-fingerprint"
        );
        if expected_state == TicketState::Closed {
            let judgment = ticket.current_closure.as_ref().unwrap();
            assert_eq!(
                judgment.reason,
                TicketClosureReason::Unknown,
                "old done must never mean successful completion"
            );
            assert_eq!(judgment.explanation, "Original decision text");
            let reopened = store
                .update_case(
                    state,
                    TicketCaseUpdate {
                        expected_content_digest: ticket_content_digest(&ticket),
                        expected_state,
                        expected_closure_operation_key: ticket.closure_operation_key.clone(),
                        operation_key: "reopen-migrated".into(),
                        action: TicketCaseAction::Reopen,
                        explanation: "Reconsider the case".into(),
                        references: Vec::new(),
                        author: Some("human".into()),
                    },
                )
                .unwrap();
            assert!(reopened.current_closure.is_none());
            assert_eq!(reopened.case_history[0], ticket.case_history[0]);
            assert_eq!(
                reopened.legacy_workflow_snapshot,
                ticket.legacy_workflow_snapshot
            );
        } else {
            assert!(ticket.current_closure.is_none());
        }
    }
    let released: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM legacy_worker_status WHERE status = 'released'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(released, 6);
    assert!(
        connection
            .execute(
                "UPDATE typed_ticket_legacy_workflow_snapshot SET snapshot_json = '{}'",
                []
            )
            .is_err()
    );
    migrate_sqlite_ticket_schema(&connection).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM typed_ticket_legacy_workflow_snapshot",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        6
    );
}

#[test]
fn unknown_legacy_state_rolls_back_the_independent_case_migration() {
    let connection = Connection::open_in_memory().unwrap();
    schema7(&connection);
    connection.execute("INSERT INTO typed_tickets (workspace_id, ticket_id, slug, title, status, kind, priority, body, workflow_state, workflow_state_explicit) VALUES ('workspace', 'unknown', 'unknown', 'unknown', 'open', 'task', 'P2', 'body', 'future-state', 1)", []).unwrap();
    assert!(migrate_sqlite_ticket_schema(&connection).is_err());
    assert_eq!(
        connection
            .query_row("SELECT version FROM ticket_schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        7
    );
    assert_eq!(
        connection
            .query_row("SELECT workflow_state FROM typed_tickets", [], |row| row
                .get::<_, String>(
                0
            ))
            .unwrap(),
        "future-state"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'typed_ticket_case_history'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn schema8_tag_composition_is_rejected_until_explicitly_integrated() {
    let connection = Connection::open_in_memory().unwrap();
    schema7(&connection);
    connection
        .execute("UPDATE ticket_schema_migrations SET version = 8", [])
        .unwrap();
    assert!(migrate_sqlite_ticket_schema(&connection).is_err());
    assert_eq!(
        connection
            .query_row("SELECT version FROM ticket_schema_migrations", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        8
    );
}
