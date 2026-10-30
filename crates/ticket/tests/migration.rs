//! Adopted compatibility contract: canonical standalone schema7 six-state snapshots
//! compose into schema10, preserving original evidence and routing handoff.
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
    let historical_events: Vec<_> = connection.prepare("SELECT ticket_id, from_state, to_state, reason, author, body FROM typed_ticket_events WHERE event_index >= 0 ORDER BY ticket_id").unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))).unwrap().collect::<std::result::Result<_, _>>().unwrap();
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
    assert_eq!(marker, 7);
    assert_eq!(
        connection
            .query_row(
                "SELECT applied_at FROM ticket_schema_migrations WHERE version=7",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "legacy"
    );
    let after_events: Vec<_> = connection.prepare("SELECT ticket_id, from_state, to_state, reason, author, body FROM typed_ticket_events WHERE event_index >= 0 ORDER BY ticket_id").unwrap().query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, String>(5)?))).unwrap().collect::<std::result::Result<_, _>>().unwrap();
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
        assert_eq!(ticket.events[1].from.as_deref(), Some("planning"));
        assert_eq!(ticket.events[1].to.as_deref(), Some(state));
        assert_eq!(ticket.events[1].references[0].target, "MR-existing");
        assert_eq!(
            ticket.events[1].attributes["request_fingerprint"],
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
fn falsely_numbered_schema8_without_tags_is_rejected_without_writes() {
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

// Read actual SQLite values, not merely counts: receipt JSON, original context,
// name snapshots and event/reference/attribute rows must survive byte-for-byte.
fn rows(connection: &Connection, table: &str) -> Vec<Vec<rusqlite::types::Value>> {
    // Compare the retained v7-v11 payload, excluding the new initial post and metadata.
    let modern: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_info('typed_tickets') WHERE name='created_by')", [], |r| r.get(0)).unwrap();
    let projection = match table {
        "typed_tickets" if modern => {
            "workspace_id,ticket_id,slug,title,status,kind,priority,(SELECT body FROM typed_ticket_events e WHERE e.workspace_id=typed_tickets.workspace_id AND e.ticket_id=typed_tickets.ticket_id AND e.event_index=-1),created_at,updated_at,assignee,readiness,workflow_state,workflow_state_explicit,queued_by,queued_at,resolution"
        }
        "typed_ticket_events" => {
            "workspace_id,ticket_id,event_index,kind,author,at,status,from_state,to_state,reason,state_field,heading,body"
        }
        _ => "*",
    };
    let filter = if table.starts_with("typed_ticket_event") {
        "WHERE event_index >= 0"
    } else {
        ""
    };
    let mut stmt = connection
        .prepare(&format!(
            "SELECT {projection} FROM {table} {filter} ORDER BY rowid"
        ))
        .unwrap();
    let count = stmt.column_count();
    stmt.query_map([], |row| (0..count).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap()
}

fn copy_tables(connection: &Connection, source: &std::path::Path, tables: &[&str]) {
    connection
        .execute(
            "ATTACH DATABASE ?1 AS fixture_source",
            [source.to_str().unwrap()],
        )
        .unwrap();
    for table in tables {
        let columns = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let selection = columns.iter().map(|column| {
            if *table == "typed_tickets" && column == "body" {
                "(SELECT body FROM fixture_source.typed_ticket_events e WHERE e.workspace_id=t.workspace_id AND e.ticket_id=t.ticket_id AND e.event_index=-1)".to_owned()
            } else { format!("t.{column}") }
        }).collect::<Vec<_>>().join(",");
        let filter = if table.starts_with("typed_ticket_event") {
            "WHERE t.event_index >= 0"
        } else {
            ""
        };
        connection.execute_batch(&format!("INSERT INTO {table} ({}) SELECT {selection} FROM fixture_source.{table} t {filter}", columns.join(","))).unwrap();
    }
    connection
        .execute_batch("DETACH DATABASE fixture_source")
        .unwrap();
}

fn mutate(backend: &SqliteTicketBackend, key: &str, action: WorkspaceTagAction) -> WorkspaceTag {
    backend
        .mutate_workspace_tag(WorkspaceTagMutation {
            operation_key: key.into(),
            action,
        })
        .unwrap()
}

fn case_request(ticket: &Ticket, key: &str, action: TicketCaseAction) -> TicketCaseUpdate {
    TicketCaseUpdate {
        expected_content_digest: ticket_content_digest(ticket),
        expected_state: ticket.meta.workflow_state,
        expected_closure_operation_key: ticket.closure_operation_key.clone(),
        operation_key: key.into(),
        action,
        explanation: "Retain this judgment".into(),
        references: vec![TicketReference {
            kind: "result".into(),
            target: "retained-result".into(),
        }],
        author: Some("authenticated-user".into()),
    }
}

#[test]
fn tagged_schema8_composes_cases_preserving_tombstones_membership_context_receipts_and_thread() {
    let temp = TempDir::new().unwrap();
    let source_path = temp.path().join("tag-operations.db");
    let source = SqliteTicketBackend::open(&source_path, "workspace")
        .unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_account_id".into(), "account".into()),
            ("source_user_id".into(), "user".into()),
            ("source_actor_role".into(), "human".into()),
        ]));
    let id = source
        .create(NewTicket::new("historical tagged ticket"))
        .unwrap()
        .id;
    // Generate real durable T-740 receipts/audit/event snapshots through its
    // unchanged classification boundary, then seed only those rows into frozen8.
    let create = WorkspaceTagMutation {
        operation_key: "create".into(),
        action: WorkspaceTagAction::Create {
            name: "open".into(),
        },
    };
    let original = source.mutate_workspace_tag(create.clone()).unwrap();
    let attach = TicketTagSetRequest {
        operation_key: "attach".into(),
        expected_tag_ids: vec![],
        tag_ids: vec![original.id.clone()],
    };
    let attach_result = source
        .set_ticket_tags(id.clone().into(), attach.clone())
        .unwrap();
    let renamed = mutate(
        &source,
        "rename",
        WorkspaceTagAction::Rename {
            tag_id: original.id.clone(),
            expected_last_operation_key: "create".into(),
            name: "closed".into(),
        },
    );
    let deleted = mutate(
        &source,
        "delete",
        WorkspaceTagAction::Delete {
            tag_id: renamed.id.clone(),
            expected_last_operation_key: "rename".into(),
        },
    );
    let recreated = mutate(
        &source,
        "recreate",
        WorkspaceTagAction::Create {
            name: "closed".into(),
        },
    );
    assert_ne!(recreated.id, deleted.id);
    source
        .set_ticket_tags(
            id.clone().into(),
            TicketTagSetRequest {
                operation_key: "attach-recreated".into(),
                expected_tag_ids: vec![],
                tag_ids: vec![recreated.id.clone()],
            },
        )
        .unwrap();

    let path = temp.path().join("retained8.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    schema7(&connection);
    connection
        .execute_batch(include_str!("fixtures/schema8_tags.sql"))
        .unwrap();
    connection
        .execute("UPDATE ticket_schema_migrations SET version=8", [])
        .unwrap();
    let preserved = [
        "workspace_tags",
        "typed_ticket_tags",
        "workspace_tag_operations",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_event_references",
    ];
    copy_tables(&connection, &source_path, &["typed_tickets"]);
    copy_tables(&connection, &source_path, &preserved);
    // Six-state retained data, not a current10 DB with its number changed.
    connection.execute("UPDATE typed_tickets SET workflow_state='done', status='done', resolution='Historical explanation', queued_by='old-router',queued_at='old-time'", []).unwrap();
    connection.execute("INSERT INTO typed_ticket_events (workspace_id,ticket_id,event_index,kind,author,body) VALUES ('workspace',?1,99,'close','old-user','Original close')", [&id]).unwrap();
    connection.execute("INSERT INTO typed_ticket_event_attributes VALUES ('workspace',?1,99,'operation_key','old-close-receipt')", [&id]).unwrap();
    connection.execute("INSERT INTO typed_ticket_event_attributes VALUES ('workspace',?1,99,'request_fingerprint','old-close-fingerprint')", [&id]).unwrap();
    let before: Vec<_> = preserved.iter().map(|t| rows(&connection, t)).collect();
    verify_legacy_sqlite_ticket_schema(&connection).unwrap();
    assert!(verify_sqlite_ticket_schema(&connection).is_err());
    assert!(verify_pre_classification_ticket_schema(&connection).is_err());
    migrate_sqlite_ticket_schema(&connection).unwrap();
    verify_sqlite_ticket_schema(&connection).unwrap();
    assert_eq!(
        rows(&connection, "ticket_schema_migrations")[0][0],
        rusqlite::types::Value::Integer(8)
    );
    for (table, expected) in preserved.iter().zip(&before) {
        assert_eq!(&rows(&connection, table), expected, "changed {table}");
    }
    let backend = SqliteTicketBackend::open_verified(&path, "workspace")
        .unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_account_id".into(), "account".into()),
            ("source_user_id".into(), "user".into()),
            ("source_actor_role".into(), "human".into()),
        ]));
    let ticket = backend.show(id.clone().into()).unwrap();
    assert_eq!(ticket.tags, vec![recreated]);
    assert_eq!(
        ticket_content_digest(&ticket),
        ticket_content_digest(&source.show(id.clone().into()).unwrap())
    );
    assert!(matches!(
        backend.update_case(
            &id,
            case_request(&ticket, "old-close-receipt", TicketCaseAction::Reopen)
        ),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(
        ticket.current_closure.as_ref().unwrap().reason,
        TicketClosureReason::Unknown
    );
    assert_eq!(
        ticket.current_closure.as_ref().unwrap().explanation,
        "Historical explanation"
    );
    assert_eq!(
        ticket
            .legacy_workflow_snapshot
            .as_ref()
            .unwrap()
            .workflow_state,
        "done"
    );
    assert_eq!(
        backend.mutate_workspace_tag(create.clone()).unwrap(),
        original
    );
    assert_eq!(
        backend
            .set_ticket_tags(id.clone().into(), attach.clone())
            .unwrap(),
        attach_result
    );
    let other = backend
        .clone()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_account_id".into(), "other-account".into()),
            ("source_user_id".into(), "user".into()),
        ]));
    assert!(matches!(
        other.mutate_workspace_tag(create),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert!(matches!(
        other.set_ticket_tags(id.clone().into(), attach),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert!(
        backend
            .set_ticket_tags(
                id.clone().into(),
                TicketTagSetRequest {
                    operation_key: "attach-deleted".into(),
                    expected_tag_ids: ticket.tags.iter().map(|t| t.id.clone()).collect(),
                    tag_ids: vec![deleted.id.clone()],
                }
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO typed_ticket_tags VALUES ('foreign',?1,?2)",
                params![id, deleted.id]
            )
            .is_err()
    );
    let judgments = rows(&connection, "typed_ticket_case_history");
    migrate_sqlite_ticket_schema(&connection).unwrap();
    assert_eq!(rows(&connection, "typed_ticket_case_history"), judgments);
    for (table, expected) in preserved.iter().zip(&before) {
        assert_eq!(
            &rows(&connection, table),
            expected,
            "replay changed {table}"
        );
    }
    let reopened = backend
        .update_case(
            &id,
            case_request(&ticket, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    assert_eq!(reopened.tags, ticket.tags);
    assert_eq!(reopened.case_history[0], ticket.case_history[0]);
    assert_eq!(
        ticket_content_digest(&reopened),
        ticket_content_digest(&ticket)
    );
    assert!(reopened.current_closure.is_none());
    let fk_failures: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(fk_failures, 0);
}

#[test]
fn independent_case_schema9_adds_only_tags_without_reapplying_judgments_or_receipts() {
    let temp = TempDir::new().unwrap();
    let source_path = temp.path().join("case-operations.db");
    let source = SqliteTicketBackend::open(&source_path, "workspace")
        .unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_runtime_id".into(), "runtime".into()),
            ("source_worker_id".into(), "worker".into()),
        ]));
    let id = source.create(NewTicket::new("existing case")).unwrap().id;
    let before = source.show(id.clone().into()).unwrap();
    let close = case_request(
        &before,
        "close",
        TicketCaseAction::Close {
            reason: TicketClosureReason::Duplicate,
        },
    );
    let closed = source.update_case(&id, close.clone()).unwrap();
    let corrected = source
        .update_case(
            &id,
            case_request(
                &closed,
                "correct",
                TicketCaseAction::CorrectClosure {
                    reason: TicketClosureReason::NotPlanned,
                },
            ),
        )
        .unwrap();
    let mut reopened = source
        .update_case(
            &id,
            case_request(&corrected, "reopen", TicketCaseAction::Reopen),
        )
        .unwrap();
    reopened.events[0]
        .attributes
        .retain(|key, _| matches!(key.as_str(), "event_id" | "event_sequence"));
    // The frozen SQLite loader published ticket_id:event_index identities.
    // These remain identical after migration, without replacing them with display ordinals.
    let path = temp.path().join("retained9.db");
    let connection = Connection::open(&path).unwrap();
    schema7(&connection);
    connection
        .execute_batch(include_str!("fixtures/schema9_cases.sql"))
        .unwrap();
    connection
        .execute("UPDATE ticket_schema_migrations SET version=9", [])
        .unwrap();
    let tables = [
        "typed_tickets",
        "typed_ticket_case_metadata",
        "typed_ticket_case_history",
        "typed_ticket_legacy_workflow_snapshot",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_event_references",
        "workspace_resource_keys",
        "workspace_resource_key_counters",
    ];
    copy_tables(&connection, &source_path, &tables);
    // A migrated closed case already judged Unknown by standalone9 must remain
    // exactly that judgment, with its original migration actor/key and handoff.
    connection.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit,resolution) VALUES ('workspace','old9','old9','Old migrated case','closed','task','P2','criteria','closed',1,'Original legacy close')", []).unwrap();
    let snapshot = TicketLegacyWorkflowSnapshot {
        workflow_state: "done".into(),
        workflow_state_explicit: true,
        status: "done".into(),
        queued_by: Some("original-router".into()),
        queued_at: Some("original-time".into()),
        resolution: Some("Original legacy close".into()),
    };
    let judgment = TicketCaseHistoryEntry {
        operation_key: "migration-v9:old9".into(),
        previous_closure_operation_key: None,
        action: TicketCaseAction::Close {
            reason: TicketClosureReason::Unknown,
        },
        explanation: "Original legacy close".into(),
        references: vec![],
        author: "ticket-schema-migration-v9".into(),
        at: "original-migration-time".into(),
    };
    connection
        .execute(
            "INSERT INTO typed_ticket_legacy_workflow_snapshot VALUES ('workspace','old9',?1)",
            [serde_json::to_string(&snapshot).unwrap()],
        )
        .unwrap();
    connection.execute("INSERT INTO typed_ticket_case_history VALUES ('workspace','old9',0,'migration-v9:old9','migration-v9',?1)", [serde_json::to_string(&judgment).unwrap()]).unwrap();
    connection.execute("INSERT INTO typed_ticket_case_metadata VALUES ('workspace','old9','migration-v9:old9')", []).unwrap();
    let original: Vec<_> = tables.iter().map(|t| rows(&connection, t)).collect();
    migrate_sqlite_ticket_schema(&connection).unwrap();
    migrate_sqlite_ticket_schema(&connection).unwrap();
    for (table, expected) in tables.iter().zip(&original) {
        assert_eq!(
            &rows(&connection, table),
            expected,
            "changed independent9 {table}"
        );
    }
    assert!(rows(&connection, "workspace_tags").is_empty());
    let backend = SqliteTicketBackend::open_verified(&path, "workspace")
        .unwrap()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_runtime_id".into(), "runtime".into()),
            ("source_worker_id".into(), "worker".into()),
        ]));
    let old9 = backend.show("old9".into()).unwrap();
    assert_eq!(old9.case_history, vec![judgment]);
    assert_eq!(old9.legacy_workflow_snapshot, Some(snapshot));
    assert_eq!(
        old9.current_closure.unwrap().reason,
        TicketClosureReason::Unknown
    );
    assert_eq!(backend.show(id.clone().into()).unwrap(), reopened);
    assert_eq!(backend.update_case(&id, close.clone()).unwrap(), reopened);
    let other = backend
        .clone()
        .with_event_attributes(std::collections::BTreeMap::from([
            ("source_runtime_id".into(), "runtime".into()),
            ("source_worker_id".into(), "other-worker".into()),
        ]));
    assert!(matches!(
        other.update_case(&id, close),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(rows(&connection, "typed_ticket_case_history"), original[2]);
}

#[test]
fn migration_inside_host_transaction_is_resumable_and_never_commits_host_writes() {
    let connection = Connection::open_in_memory().unwrap();
    schema7(&connection);
    verify_pre_classification_ticket_schema(&connection).unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&connection).is_err());
    connection.execute_batch("CREATE TABLE host_work (value TEXT); BEGIN IMMEDIATE; INSERT INTO host_work VALUES ('pending');").unwrap();
    migrate_sqlite_ticket_schema_in_transaction(&connection).unwrap();
    verify_sqlite_ticket_schema(&connection).unwrap();
    assert!(!connection.is_autocommit());
    connection.execute_batch("ROLLBACK").unwrap();
    verify_pre_classification_ticket_schema(&connection).unwrap();
    assert!(rows(&connection, "host_work").is_empty());
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    migrate_sqlite_ticket_schema_in_transaction(&connection).unwrap();
    migrate_sqlite_ticket_schema_in_transaction(&connection).unwrap();
    connection.execute_batch("COMMIT").unwrap();
    verify_sqlite_ticket_schema(&connection).unwrap();
}

#[test]
fn falsely_numbered_schema9_and_weakened_immutable_triggers_are_rejected_without_migration() {
    for drift in [
        "UPDATE ticket_schema_migrations SET version=9",
        "UPDATE ticket_schema_migrations SET version=9; DROP TRIGGER typed_ticket_case_history_no_update; CREATE TRIGGER typed_ticket_case_history_no_update BEFORE UPDATE ON typed_ticket_case_history BEGIN SELECT 1; END;",
    ] {
        let connection = Connection::open_in_memory().unwrap();
        schema7(&connection);
        if drift.contains("DROP TRIGGER") {
            connection
                .execute_batch(include_str!("fixtures/schema9_cases.sql"))
                .unwrap();
        }
        connection.execute_batch(drift).unwrap();
        let marker = rows(&connection, "ticket_schema_migrations");
        assert!(migrate_sqlite_ticket_schema(&connection).is_err());
        assert_eq!(rows(&connection, "ticket_schema_migrations"), marker);
        let tags: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='workspace_tags'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tags, 0);
    }
}

#[test]
fn failed_component_conversion_rolls_back_its_ddl_and_keeps_host_transaction_retryable() {
    let connection = Connection::open_in_memory().unwrap();
    schema7(&connection);
    connection.execute_batch("CREATE TABLE host_work (value TEXT);
        INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit)
        VALUES ('workspace','ticket','ticket','Case','open','task','P2','criteria','ready',1);
        CREATE TRIGGER host_abort_case_update BEFORE UPDATE OF workflow_state ON typed_tickets
        BEGIN SELECT RAISE(ABORT,'host failure'); END;
        BEGIN IMMEDIATE; INSERT INTO host_work VALUES ('retained host work');").unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&connection).is_err());
    assert!(!connection.is_autocommit());
    verify_pre_classification_ticket_schema(&connection).unwrap();
    assert_eq!(rows(&connection, "host_work").len(), 1);
    let tags: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='workspace_tags'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tags, 0, "DDL before failed state conversion must roll back");
    connection
        .execute_batch("DROP TRIGGER host_abort_case_update")
        .unwrap();
    migrate_sqlite_ticket_schema_in_transaction(&connection).unwrap();
    connection.execute_batch("COMMIT").unwrap();
    verify_sqlite_ticket_schema(&connection).unwrap();
    assert_eq!(rows(&connection, "host_work").len(), 1);
}

#[test]
fn retained_workspace_foreign_keys_survive_composition_without_rebuilding_ticket_or_tags() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    connection.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY); INSERT INTO workspaces VALUES ('workspace');").unwrap();
    // Ticket accepts stricter host-owned Workspace references and must preserve
    // them, not rebuild parents against its weaker standalone authority schema.
    let schema = include_str!("fixtures/schema7.sql");
    let (before, ticket) = schema.split_once("CREATE TABLE typed_tickets (").unwrap();
    let (ticket, after) = ticket
        .split_once("CREATE TABLE typed_ticket_targets (")
        .unwrap();
    let ticket = ticket.replace("PRIMARY KEY (workspace_id, ticket_id)",
        "PRIMARY KEY (workspace_id, ticket_id), FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE");
    connection
        .execute_batch(&format!(
            "{before}CREATE TABLE typed_tickets ({ticket}CREATE TABLE typed_ticket_targets ({after}"
        ))
        .unwrap();
    let tags = include_str!("fixtures/schema8_tags.sql")
        .replace("PRIMARY KEY (workspace_id, tag_id)", "PRIMARY KEY (workspace_id, tag_id), FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE")
        .replace("PRIMARY KEY (workspace_id, operation_key)", "PRIMARY KEY (workspace_id, operation_key), FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE");
    connection.execute_batch(&tags).unwrap();
    connection.execute_batch("CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL);
        INSERT INTO ticket_schema_migrations VALUES(8,'ticket schema baseline','retained');
        INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit)
        VALUES('workspace','ticket','ticket','Case','done','task','P2','criteria','done',1);
        INSERT INTO workspace_tags VALUES('workspace','tag','closed','closed','create',0);
        INSERT INTO typed_ticket_tags VALUES('workspace','ticket','tag');").unwrap();
    let signature = |table| {
        let mut stmt = connection
            .prepare(&format!(
                "SELECT * FROM pragma_foreign_key_list('{table}') ORDER BY id,seq"
            ))
            .unwrap();
        let count = stmt.column_count();
        stmt.query_map([], |row| {
            (0..count)
                .map(|i| row.get::<_, rusqlite::types::Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    };
    let tables = [
        "typed_tickets",
        "workspace_tags",
        "workspace_tag_operations",
        "typed_ticket_tags",
    ];
    let before: Vec<_> = tables.iter().map(|t| signature(t)).collect();
    verify_legacy_sqlite_ticket_schema(&connection).unwrap();
    migrate_sqlite_ticket_schema(&connection).unwrap();
    for (table, expected) in tables.iter().zip(before) {
        assert_eq!(signature(table), expected, "lost host FK on {table}");
    }
    assert!(
        connection
            .execute(
                "INSERT INTO workspace_tags VALUES('foreign','tag','open','open','create',0)",
                []
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO typed_ticket_tags VALUES('foreign','ticket','tag')",
                []
            )
            .is_err()
    );
    let failures: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(failures, 0);
}

fn compose_historical(connection: &Connection, target: HistoricalTicketSchemaTarget) {
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    migrate_historical_sqlite_ticket_schema_in_transaction(connection, target).unwrap();
    assert!(!connection.is_autocommit());
    connection.execute_batch("COMMIT").unwrap();
}

fn max_component(connection: &Connection) -> i64 {
    connection
        .query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn historical_composition_stops_at_each_explicit_boundary_and_preserves_applied_receipts() {
    use HistoricalTicketSchemaTarget::{Cases10, Content13, Posts12, Tracking11};
    // Both empty stores and real old schema7 preimages must stop at the chosen
    // contract, rather than installing the latest Subject schema prematurely.
    for retained in [false, true] {
        for (target, version) in [
            (Cases10, 10),
            (Tracking11, 11),
            (Posts12, 12),
            (Content13, 13),
        ] {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
            if retained {
                schema7(&conn);
            }
            assert!(migrate_historical_sqlite_ticket_schema_in_transaction(&conn, target).is_err());
            compose_historical(&conn, target);
            assert_eq!(max_component(&conn), version);
            let exists = |kind: &str, name: &str| -> bool {
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type=?1 AND name=?2)",
                    params![kind, name],
                    |r| r.get(0),
                )
                .unwrap()
            };
            assert_eq!(
                exists("table", "typed_ticket_assignment_tracking_history"),
                version >= 11
            );
            assert_eq!(exists("index", "typed_ticket_events_ref"), version >= 12);
            assert_eq!(
                exists("table", "typed_ticket_content_changes"),
                version >= 13
            );
            assert!(!exists("trigger", "ticket_subject_current_insert"));
            if retained {
                assert_eq!(
                    conn.query_row(
                        "SELECT name,applied_at FROM ticket_schema_migrations WHERE version=7",
                        [],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    )
                    .unwrap(),
                    ("ticket schema baseline".into(), "legacy".into())
                );
            }
            let receipts = rows(&conn, "ticket_schema_migrations");
            compose_historical(&conn, target);
            assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
            // Older host hooks verify the actual newer historical generation;
            // they neither replay older steps nor change its receipts.
            compose_historical(&conn, Cases10);
            assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
            assert!(verify_sqlite_ticket_schema(&conn).is_err());
        }
    }
    let conn = Connection::open_in_memory().unwrap();
    schema7(&conn);
    let mut prior = rows(&conn, "ticket_schema_migrations");
    for target in [Cases10, Tracking11, Posts12, Content13] {
        compose_historical(&conn, target);
        let after = rows(&conn, "ticket_schema_migrations");
        assert_eq!(&after[..prior.len()], prior.as_slice());
        prior = after;
    }
}

#[test]
fn historical_composition_verifies_actual_current_contract_without_downgrade_or_repair() {
    let conn = Connection::open_in_memory().unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    let receipts = rows(&conn, "ticket_schema_migrations");
    for target in [
        HistoricalTicketSchemaTarget::Cases10,
        HistoricalTicketSchemaTarget::Tracking11,
        HistoricalTicketSchemaTarget::Posts12,
        HistoricalTicketSchemaTarget::Content13,
    ] {
        compose_historical(&conn, target);
        assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
        verify_sqlite_ticket_schema(&conn).unwrap();
    }
    // A lower requested target cannot hide a weakened actual14 guard.
    conn.execute_batch("DROP TRIGGER ticket_subject_current_insert; CREATE TRIGGER ticket_subject_current_insert BEFORE INSERT ON ticket_current_worker_assignments BEGIN SELECT 1; END;").unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        migrate_historical_sqlite_ticket_schema_in_transaction(
            &conn,
            HistoricalTicketSchemaTarget::Cases10
        )
        .is_err()
    );
    assert!(!conn.is_autocommit());
    assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
    conn.execute_batch("ROLLBACK").unwrap();

    // A claimed13 marker over actual10 must also be rejected, not backfilled.
    let conn = Connection::open_in_memory().unwrap();
    compose_historical(&conn, HistoricalTicketSchemaTarget::Cases10);
    conn.execute("UPDATE ticket_schema_migrations SET version=13", [])
        .unwrap();
    let before = rows(&conn, "ticket_schema_migrations");
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        migrate_historical_sqlite_ticket_schema_in_transaction(
            &conn,
            HistoricalTicketSchemaTarget::Cases10
        )
        .is_err()
    );
    assert_eq!(rows(&conn, "ticket_schema_migrations"), before);
    conn.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn bounded_content13_reopens_after_failed_subject_upgrade_then_migrates_atomically_to_latest() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("retained-content13.db");
    let (receipts, history, old_table);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        compose_historical(&conn, HistoricalTicketSchemaTarget::Content13);
        // Actual retained13 accepts its historical stable-agent contract. It
        // has not been created at14 and then merely relabelled as old.
        conn.execute_batch("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('workspace','t','t','Title','open','task','P2','open',1);
            INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('workspace','t',-1,'t:-1','comment','Body');
            INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at) VALUES('workspace','t','old','orchestrator','workspace_agent','workspace-orchestrator','old-author','old-time');
            INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at) VALUES('workspace','t','old','orchestrator','workspace_agent','workspace-orchestrator','old-time');
            INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id) VALUES('workspace','t','orchestrator','old-assigned','assigned','old','old-author','old-time','old-op');
            CREATE TRIGGER host_reject_subject_migration BEFORE UPDATE OF tracking_migration_diagnostic ON ticket_worker_assignments BEGIN SELECT RAISE(ABORT,'forced post-widen failure'); END;").unwrap();
        receipts = rows(&conn, "ticket_schema_migrations");
        history = rows(&conn, "typed_ticket_assignment_tracking_history");
        old_table = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name='ticket_worker_assignments'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        conn.execute_batch("BEGIN IMMEDIATE; CREATE TABLE host_work(value TEXT); INSERT INTO host_work VALUES('keep');").unwrap();
        assert!(migrate_sqlite_ticket_schema_in_transaction(&conn).is_err());
        assert!(!conn.is_autocommit());
        assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
        assert_eq!(
            rows(&conn, "typed_ticket_assignment_tracking_history"),
            history
        );
        assert_eq!(
            conn.query_row(
                "SELECT sql FROM sqlite_schema WHERE name='ticket_worker_assignments'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            old_table
        );
        assert!(
            !conn
                .pragma_query_value(None, "writable_schema", |r| r.get::<_, bool>(0))
                .unwrap()
        );
        conn.execute_batch("COMMIT").unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    compose_historical(&conn, HistoricalTicketSchemaTarget::Content13);
    assert_eq!(max_component(&conn), 13);
    assert_eq!(rows(&conn, "ticket_schema_migrations"), receipts);
    assert_eq!(
        conn.query_row("SELECT value FROM host_work", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    conn.execute_batch("DROP TRIGGER host_reject_subject_migration")
        .unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(max_component(&conn), 15);
    assert_eq!(
        &rows(&conn, "ticket_schema_migrations")[..receipts.len()],
        receipts.as_slice()
    );
    assert_eq!(
        rows(&conn, "typed_ticket_assignment_tracking_history"),
        history
    );
    drop(conn);
    let conn = Connection::open(&path).unwrap();
    let applied14 = rows(&conn, "ticket_schema_migrations");
    compose_historical(&conn, HistoricalTicketSchemaTarget::Content13);
    assert_eq!(rows(&conn, "ticket_schema_migrations"), applied14);
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn service_outbox_upgrade_is_empty_preserves_old_facts_and_matches_fresh_schema() {
    let conn = Connection::open_in_memory().unwrap();
    compose_historical(&conn, HistoricalTicketSchemaTarget::Subjects14);
    conn.execute_batch("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('workspace','old','old','Private title','open','task','P2','open',1);
        INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body) VALUES('workspace','old',-1,'old:-1','comment','Private body');
        INSERT INTO typed_ticket_content_changes VALUES('old-change','workspace','old','content_edit','Private actor','old-time',1,0,0,NULL);").unwrap();
    let events = rows(&conn, "typed_ticket_events");
    let facts = rows(&conn, "typed_ticket_content_changes");
    let receipts = rows(&conn, "ticket_schema_migrations");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(rows(&conn, "typed_ticket_events"), events);
    assert_eq!(rows(&conn, "typed_ticket_content_changes"), facts);
    assert_eq!(
        &rows(&conn, "ticket_schema_migrations")[..receipts.len()],
        receipts.as_slice()
    );
    assert!(rows(&conn, "typed_ticket_service_outbox").is_empty());
    let fresh = Connection::open_in_memory().unwrap();
    migrate_sqlite_ticket_schema(&fresh).unwrap();
    let ddl = |db: &Connection| {
        db.prepare("SELECT type,name,sql FROM sqlite_master WHERE name LIKE 'typed_ticket_service_outbox%' ORDER BY name").unwrap()
        .query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    assert_eq!(ddl(&conn), ddl(&fresh));
    let all_receipts = rows(&conn, "ticket_schema_migrations");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(rows(&conn, "ticket_schema_migrations"), all_receipts);
    assert!(rows(&conn, "typed_ticket_service_outbox").is_empty());
    conn.execute_batch("DROP INDEX typed_ticket_service_outbox_workspace_sequence")
        .unwrap();
    assert!(
        verify_sqlite_ticket_schema(&conn)
            .unwrap_err()
            .to_string()
            .contains("Ticket Service outbox")
    );
    assert!(
        migrate_sqlite_ticket_schema(&conn).is_err(),
        "current schema must not repair missing objects"
    );
}

#[test]
fn failed_service_outbox_migration_rolls_back_component_and_retains_host_transaction() {
    let conn = Connection::open_in_memory().unwrap();
    compose_historical(&conn, HistoricalTicketSchemaTarget::Subjects14);
    let before = rows(&conn, "ticket_schema_migrations");
    conn.execute_batch(
        "CREATE TABLE typed_ticket_service_outbox_workspace_sequence(value TEXT);
        BEGIN IMMEDIATE; CREATE TABLE host_work(value TEXT); INSERT INTO host_work VALUES('keep');",
    )
    .unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&conn).is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(rows(&conn, "ticket_schema_migrations"), before);
    assert!(!conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='typed_ticket_service_outbox')", [], |r| r.get::<_,bool>(0)).unwrap());
    conn.execute_batch("COMMIT; DROP TABLE typed_ticket_service_outbox_workspace_sequence;")
        .unwrap();
    assert_eq!(
        conn.query_row("SELECT value FROM host_work", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
}
