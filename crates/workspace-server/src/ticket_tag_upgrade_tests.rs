// Included by store::tests. Genuine retained six-state Workspace89/90 fixtures
// compose auth90, classification and cases; no lowered current-schema markers.

// Frozen classification schema from develop b1fe562e (Ticket component 8).
// Deliberate persisted compatibility input, independent of the latest baseline.
fn install_retained_ticket_tags8(conn: &Connection) {
    ticket::verify_pre_classification_ticket_schema(conn).unwrap();
    conn.execute_batch(r#"
        CREATE TABLE workspace_tags (
            workspace_id TEXT NOT NULL, tag_id TEXT NOT NULL, name TEXT NOT NULL,
            normalized_name TEXT NOT NULL, last_operation_key TEXT NOT NULL,
            deleted INTEGER NOT NULL CHECK (deleted IN (0,1)),
            PRIMARY KEY(workspace_id,tag_id));
        CREATE UNIQUE INDEX workspace_tags_active_name
            ON workspace_tags(workspace_id,normalized_name) WHERE deleted=0;
        CREATE TABLE typed_ticket_tags (
            workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, tag_id TEXT NOT NULL,
            PRIMARY KEY(workspace_id,ticket_id,tag_id),
            FOREIGN KEY(workspace_id,ticket_id) REFERENCES typed_tickets(workspace_id,ticket_id) ON DELETE CASCADE,
            FOREIGN KEY(workspace_id,tag_id) REFERENCES workspace_tags(workspace_id,tag_id) ON DELETE RESTRICT);
        CREATE INDEX typed_ticket_tags_workspace_tag ON typed_ticket_tags(workspace_id,tag_id,ticket_id);
        CREATE TABLE workspace_tag_operations (
            workspace_id TEXT NOT NULL, operation_key TEXT NOT NULL, kind TEXT NOT NULL,
            target_id TEXT NOT NULL, input_json TEXT NOT NULL, source_context_json TEXT NOT NULL,
            before_json TEXT NOT NULL, result_json TEXT NOT NULL, at TEXT NOT NULL,
            PRIMARY KEY(workspace_id,operation_key));
        UPDATE ticket_schema_migrations SET version=8;
    "#).unwrap();
}

#[derive(Clone)]
struct RetainedClassification {
    source: std::collections::BTreeMap<String, String>,
    request: ticket::TicketTagSetRequest,
    receipt: Vec<ticket::WorkspaceTag>,
    active_ids: Vec<String>,
    deleted_id: String,
}

// Seed actual v8 persisted boundaries rather than invoking a latest-only API on
// a legacy database. Full source context is audit, only authenticated identity
// belongs to the replay fingerprint. It is deliberately not the assigned Worker.
fn seed_retained_classification(
    conn: &Connection,
    workspace: &str,
    id: &str,
) -> RetainedClassification {
    let source = std::collections::BTreeMap::from([
        ("source_runtime_id".into(), "case-runtime".into()),
        ("source_worker_id".into(), "classifier".into()),
        ("assigned_runtime_id".into(), "case-runtime".into()),
        ("assigned_worker_id".into(), id.into()),
        ("source_actor_role".into(), "ticket_worker".into()),
        (
            "source_assignment_id".into(),
            format!("old-assignment-{id}"),
        ),
    ]);
    let identity = std::collections::BTreeMap::from([
        ("source_runtime_id", "case-runtime"),
        ("source_worker_id", "classifier"),
    ]);
    let mut receipt = Vec::new();
    for (suffix, name, deleted) in [
        ("ready", "ready", false),
        ("closed", "closed", false),
        ("old", "Bug", true),
        ("replacement", "Bug", false),
    ] {
        let tag = ticket::WorkspaceTag {
            workspace_id: workspace.into(),
            id: format!("{id}-{suffix}"),
            name: name.into(),
            last_operation_key: format!("{id}-{suffix}-create"),
            deleted: false,
        };
        let request = ticket::WorkspaceTagMutation {
            operation_key: tag.last_operation_key.clone(),
            action: ticket::WorkspaceTagAction::Create { name: name.into() },
        };
        conn.execute(
            "INSERT INTO workspace_tags VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                workspace,
                tag.id,
                name,
                name.to_lowercase(),
                if deleted {
                    format!("{id}-delete")
                } else {
                    tag.last_operation_key.clone()
                },
                deleted
            ],
        )
        .unwrap();
        let input =
            serde_json::to_string(&("mutate_workspace_tag", "", &request, &identity)).unwrap();
        conn.execute("INSERT INTO workspace_tag_operations VALUES(?1,?2,'create',?3,?4,?5,'null',?6,'original-time')",
            params![workspace, request.operation_key, tag.id, input, serde_json::to_string(&source).unwrap(), serde_json::to_string(&tag).unwrap()]).unwrap();
        if suffix != "replacement" {
            receipt.push(tag);
        }
    }
    receipt.sort_by(|a, b| a.id.cmp(&b.id));
    let request = ticket::TicketTagSetRequest {
        operation_key: format!("{id}-attach"),
        expected_tag_ids: vec![],
        tag_ids: receipt.iter().map(|tag| tag.id.clone()).collect(),
    };
    let selector: ticket::TicketIdOrSlug = id.into();
    let input =
        serde_json::to_string(&("set_ticket_tags", "", &(&selector, &request), &identity)).unwrap();
    conn.execute("INSERT INTO workspace_tag_operations VALUES(?1,?2,'set_ticket_tags',?3,?4,?5,'[]',?6,'original-time')",
        params![workspace, request.operation_key, id, input, serde_json::to_string(&source).unwrap(), serde_json::to_string(&receipt).unwrap()]).unwrap();
    let active: Vec<_> = receipt
        .iter()
        .filter(|tag| !tag.id.ends_with("-old"))
        .cloned()
        .collect();
    let old = receipt.iter().find(|tag| tag.id.ends_with("-old")).unwrap();
    let mut deleted = old.clone();
    deleted.deleted = true;
    deleted.last_operation_key = format!("{id}-delete");
    let delete_request = ticket::WorkspaceTagMutation {
        operation_key: deleted.last_operation_key.clone(),
        action: ticket::WorkspaceTagAction::Delete {
            tag_id: old.id.clone(),
            expected_last_operation_key: old.last_operation_key.clone(),
        },
    };
    let input =
        serde_json::to_string(&("mutate_workspace_tag", "", &delete_request, &identity)).unwrap();
    conn.execute("INSERT INTO workspace_tag_operations VALUES(?1,?2,'delete',?3,?4,?5,?6,?7,'original-time')",
        params![workspace, delete_request.operation_key, old.id, input, serde_json::to_string(&source).unwrap(), serde_json::to_string(old).unwrap(), serde_json::to_string(&deleted).unwrap()]).unwrap();
    for tag in &active {
        conn.execute(
            "INSERT INTO typed_ticket_tags VALUES(?1,?2,?3)",
            params![workspace, id, tag.id],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO typed_ticket_labels VALUES(?1,?2,0,'ready')",
        params![workspace, id],
    )
    .unwrap();
    let next: i64 = conn.query_row("SELECT COALESCE(MAX(event_index)+1,0) FROM typed_ticket_events WHERE workspace_id=?1 AND ticket_id=?2",params![workspace,id],|row|row.get(0)).unwrap();
    for (offset, key, before, after) in [
        (
            0,
            request.operation_key.as_str(),
            Vec::new(),
            receipt.clone(),
        ),
        (
            1,
            delete_request.operation_key.as_str(),
            receipt.clone(),
            active.clone(),
        ),
    ] {
        let body =
            serde_json::json!({"operation_key":key,"before":before,"after":after}).to_string();
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,body) VALUES(?1,?2,?3,'tags_changed','classifier','original-time',?4)",params![workspace,id,next+offset,body]).unwrap();
        for (key, value) in &source {
            conn.execute(
                "INSERT INTO typed_ticket_event_attributes VALUES(?1,?2,?3,?4,?5)",
                params![workspace, id, next + offset, key, value],
            )
            .unwrap();
        }
    }
    let deleted_id = old.id.clone();
    RetainedClassification {
        source,
        request,
        receipt,
        active_ids: active.iter().map(|tag| tag.id.clone()).collect(),
        deleted_id,
    }
}

fn classification_rows(conn: &Connection) -> Vec<Vec<Vec<rusqlite::types::Value>>> {
    [
        "workspace_tags",
        "typed_ticket_tags",
        "workspace_tag_operations",
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_labels",
    ]
    .map(|table| stored_fixture_rows(conn, table))
    .to_vec()
}

#[test]
fn workspace90_ticket7_and_tagged8_upgrade_to_cases10_preserving_history_and_auth() {
    for component in [7, 8] {
        assert_retained_classification_upgrade(90, component);
    }
}

#[test]
fn workspace89_auth_then_cases_preserve_tags_receipts_source_context_and_runtime_archives() {
    assert_retained_classification_upgrade(89, 8);
}

fn assert_retained_classification_upgrade(workspace_version: i64, component: i64) {
    use ticket::TicketBackend;
    let (_dir, path) = legacy_case_workspace(workspace_version);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    // Real six-state parent rows: neither a fresh open/closed schema nor API close.
    conn.execute_batch("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit,resolution,created_at,updated_at)
        VALUES('case-workspace','closed-legacy','closed-legacy','Retained decision','closed','task','P2','Original requirements','done',1,'Original closure text','1','2');
        INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,from_state,to_state,reason,state_field,body)
        VALUES('case-workspace','closed-legacy',0,'state_changed','original-author','1','inprogress','done','Original decision','state','Original thread');
        INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at)
        VALUES('case-workspace','case-runtime','recipient','Recipient','normal','1','1');
        INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,assigned_by,assigned_at)
        VALUES('case-workspace','closed-legacy','assignment','worker','worker','case-runtime','recipient','original-actor','1');
        INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,runtime_id,worker_id,updated_at)
        VALUES('case-workspace','closed-legacy','assignment','worker','worker','case-runtime','recipient','1');").unwrap();
    ticket::verify_pre_classification_ticket_schema(&conn).unwrap();
    let retained = if component == 8 {
        install_retained_ticket_tags8(&conn);
        Some(seed_retained_classification(
            &conn,
            "case-workspace",
            "closed-legacy",
        ))
    } else {
        conn.execute(
            "INSERT INTO typed_ticket_labels VALUES('case-workspace','closed-legacy',0,'ready')",
            [],
        )
        .unwrap();
        None
    };
    assert_eq!(
        conn.query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        component
    );
    assert!(!table_exists(&conn, "typed_ticket_case_history").unwrap());
    let preserved_tables = [
        "typed_ticket_events",
        "typed_ticket_event_attributes",
        "typed_ticket_labels",
        "ticket_current_worker_assignments",
        "ticket_worker_assignments",
        "workspace_signing_identities",
        "workspace_signing_identity_audit",
        "workspace_signing_identity_provisioning_operations",
        "workspace_runtime_binding_audit",
    ];
    let preserved = preserved_tables.map(|table| stored_fixture_rows(&conn, table));
    let tags_before = retained.as_ref().map(|_| classification_rows(&conn));
    let auth_before = fixture_auth_rows(&conn);
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store.with_conn(|conn| {
        assert_eq!(current_schema_version(conn)?,LATEST_SCHEMA_VERSION);
        assert_eq!(conn.query_row("SELECT MAX(version) FROM ticket_schema_migrations",[],|row|row.get::<_,i64>(0))?,14);
        assert_eq!(fixture_auth_rows(conn),auth_before);
        for (table, rows) in preserved_tables.iter().zip(&preserved) {
            let mut migrated = stored_fixture_rows(conn, table);
            if *table == "ticket_worker_assignments" {
                // Component 11 adds handling columns to the same identity row.
                // Compare the ten retained identity/principal/Assign columns,
                // and separately prove the historical Worker acquired no tracking.
                for row in &mut migrated { row.truncate(10); }
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM ticket_worker_assignments WHERE tracking_json IS NOT NULL", [], |row| row.get::<_,i64>(0))?, 0);
            }
            assert_eq!(&migrated, rows, "preserve {table}");
        }
        if let Some(rows)=&tags_before {assert_eq!(&classification_rows(conn),rows);}
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM ticket_assignment_work_releases WHERE assignment_id='assignment'",[],|row|row.get::<_,i64>(0))?,1);
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|row|row.get::<_,i64>(0))?,0);
        assert!(!column_exists(conn,"workspace_runtime_bindings","state")?);
        ticket::verify_sqlite_ticket_schema(conn).unwrap();
        Ok(())
    }).unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "case-workspace").unwrap();
    let before = backend.show("closed-legacy".into()).unwrap();
    assert_eq!(before.meta.workflow_state, ticket::TicketState::Closed);
    assert_eq!(
        before.current_closure.as_ref().unwrap().reason,
        ticket::TicketClosureReason::Unknown
    );
    assert_eq!(before.case_history.len(), 1);
    assert_eq!(
        before
            .legacy_workflow_snapshot
            .as_ref()
            .unwrap()
            .resolution
            .as_deref(),
        Some("Original closure text")
    );
    if let Some(retained) = retained {
        let mut changed_context = retained.source.clone();
        changed_context.insert("source_assignment_id".into(), "new-assignment".into());
        changed_context.insert("assigned_worker_id".into(), "new-recipient".into());
        let actor = backend
            .clone()
            .with_event_attributes(changed_context.clone());
        assert_eq!(
            actor
                .set_ticket_tags("closed-legacy".into(), retained.request.clone())
                .unwrap(),
            retained.receipt
        );
        changed_context.insert("source_worker_id".into(), "other-classifier".into());
        assert!(
            backend
                .clone()
                .with_event_attributes(changed_context)
                .set_ticket_tags("closed-legacy".into(), retained.request)
                .is_err()
        );
        store
            .with_conn(|conn| {
                assert_eq!(classification_rows(conn), tags_before.clone().unwrap());
                assert!(
                    conn.execute(
                        "INSERT INTO typed_ticket_tags VALUES('other','missing',?1)",
                        [&retained.active_ids[0]]
                    )
                    .is_err()
                );
                Ok(())
            })
            .unwrap();
        assert!(
            backend
                .set_ticket_tags(
                    "closed-legacy".into(),
                    ticket::TicketTagSetRequest {
                        operation_key: "deleted-reference".into(),
                        expected_tag_ids: retained.active_ids.clone(),
                        tag_ids: vec![retained.deleted_id]
                    }
                )
                .is_err()
        );
    }
    // Arbitrary lifecycle-looking tag names cannot alter a reasoned closure,
    // digest, assignment or prior immutable case history.
    let reasoned = update_test_case(
        &backend,
        "closed-legacy",
        "correct-reason",
        ticket::TicketCaseAction::CorrectClosure {
            reason: ticket::TicketClosureReason::NotPlanned,
        },
    );
    let tag = backend
        .mutate_workspace_tag(ticket::WorkspaceTagMutation {
            operation_key: "new-ready".into(),
            action: ticket::WorkspaceTagAction::Create {
                name: "ready after upgrade".into(),
            },
        })
        .unwrap();
    backend
        .set_ticket_tags(
            "closed-legacy".into(),
            ticket::TicketTagSetRequest {
                operation_key: "new-attach".into(),
                expected_tag_ids: reasoned.tags.iter().map(|tag| tag.id.clone()).collect(),
                tag_ids: vec![tag.id],
            },
        )
        .unwrap();
    let classified = backend.show("closed-legacy".into()).unwrap();
    assert_eq!(classified.meta, reasoned.meta);
    assert_eq!(classified.current_closure, reasoned.current_closure);
    assert_eq!(classified.case_history, reasoned.case_history);
    assert_eq!(
        ticket::ticket_content_digest(&classified),
        ticket::ticket_content_digest(&reasoned)
    );
    let tag_history: Vec<_> = classified
        .events
        .iter()
        .filter(|event| event.kind == ticket::TicketEventKind::TagsChanged)
        .cloned()
        .collect();
    let reopened = update_test_case(
        &backend,
        "closed-legacy",
        "reopen",
        ticket::TicketCaseAction::Reopen,
    );
    assert_eq!(reopened.tags, classified.tags);
    assert_eq!(
        reopened.case_history[..classified.case_history.len()],
        classified.case_history
    );
    assert_eq!(
        reopened
            .events
            .iter()
            .filter(|event| event.kind == ticket::TicketEventKind::TagsChanged)
            .cloned()
            .collect::<Vec<_>>(),
        tag_history
    );
    assert!(
        store
            .get_active_ticket_worker_assignment("case-workspace", "closed-legacy")
            .unwrap()
            .is_none()
    );
}

// Frozen cases-only component9 DDL from a558, not a stripped current DB.
const RETAINED_CASES9_SQL: &str = r#"
-- Ticket case authority. Legacy workflow data is a handoff snapshot, never current authority.
CREATE TABLE typed_ticket_case_metadata (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, closure_operation_key TEXT,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_case_history (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL,
    operation_key TEXT NOT NULL, request_fingerprint TEXT NOT NULL, judgment_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, operation_key),
    UNIQUE (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE typed_ticket_legacy_workflow_snapshot (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, snapshot_json TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TRIGGER typed_ticket_case_history_no_update BEFORE UPDATE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_case_history_no_delete BEFORE DELETE ON typed_ticket_case_history
BEGIN SELECT RAISE(ABORT, 'Ticket case history is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_update BEFORE UPDATE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_legacy_workflow_no_delete BEFORE DELETE ON typed_ticket_legacy_workflow_snapshot
BEGIN SELECT RAISE(ABORT, 'Ticket legacy workflow snapshot is immutable'); END;
CREATE TRIGGER typed_ticket_case_state_insert BEFORE INSERT ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
CREATE TRIGGER typed_ticket_case_state_update BEFORE UPDATE OF workflow_state ON typed_tickets
WHEN NEW.workflow_state NOT IN ('open', 'closed')
BEGIN SELECT RAISE(ABORT, 'Ticket case state must be open or closed'); END;
"#;

#[test]
fn workspace90_cases9_add_classification_without_reinterpreting_reasoned_history() {
    use ticket::TicketBackend;
    let (_dir, path) = legacy_case_workspace(90);
    let conn = Connection::open(&path).unwrap();
    configure_sqlite(&conn).unwrap();
    // Build a genuine cases-only boundary from retained7 and frozen9 DDL.
    // No latest schema is relabelled and no six-state row is called current.
    conn.execute_batch(RETAINED_CASES9_SQL).unwrap();
    conn.execute_batch("UPDATE ticket_schema_migrations SET version=9;
        INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit)
        VALUES('case-workspace','reasoned','reasoned','Existing judgment','closed','task','P2','Original requirements','closed',1);
        INSERT INTO typed_ticket_case_metadata VALUES('case-workspace','reasoned','original-closure');").unwrap();
    let judgment = ticket::TicketCaseHistoryEntry {
        operation_key: "original-closure".into(),
        previous_closure_operation_key: None,
        action: ticket::TicketCaseAction::Close {
            reason: ticket::TicketClosureReason::Completed,
        },
        explanation: "Original reasoned conclusion".into(),
        references: vec![ticket::TicketReference {
            kind: "commit".into(),
            target: "original-source".into(),
        }],
        author: "original-author".into(),
        at: "original-time".into(),
    };
    conn.execute("INSERT INTO typed_ticket_case_history VALUES('case-workspace','reasoned',0,'original-closure','original-fingerprint',?1)",
        [serde_json::to_string(&judgment).unwrap()]).unwrap();
    assert!(!table_exists(&conn, "workspace_tags").unwrap());
    let history_before = stored_fixture_rows(&conn, "typed_ticket_case_history");
    let metadata_before = stored_fixture_rows(&conn, "typed_ticket_case_metadata");
    let auth_before = fixture_auth_rows(&conn);
    drop(conn);
    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
    store
        .with_conn(|conn| {
            assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
            assert_eq!(
                conn.query_row(
                    "SELECT MAX(version) FROM ticket_schema_migrations",
                    [],
                    |row| row.get::<_, i64>(0)
                )?,
                14
            );
            assert_eq!(
                stored_fixture_rows(conn, "typed_ticket_case_history"),
                history_before
            );
            assert_eq!(
                stored_fixture_rows(conn, "typed_ticket_case_metadata"),
                metadata_before
            );
            assert_eq!(fixture_auth_rows(conn), auth_before);
            assert_eq!(
                conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get::<_, i64>(0)
                })?,
                0
            );
            ticket::verify_sqlite_ticket_schema(conn).unwrap();
            Ok(())
        })
        .unwrap();
    let backend = ticket::SqliteTicketBackend::open_verified(&path, "case-workspace").unwrap();
    let before = backend.show("reasoned".into()).unwrap();
    assert_eq!(before.case_history, vec![judgment]);
    assert_eq!(
        before.current_closure.as_ref().unwrap().reason,
        ticket::TicketClosureReason::Completed
    );
    for name in ["ready", "closed"] {
        let tag = backend
            .mutate_workspace_tag(ticket::WorkspaceTagMutation {
                operation_key: format!("create-{name}"),
                action: ticket::WorkspaceTagAction::Create { name: name.into() },
            })
            .unwrap();
        let current = backend.show("reasoned".into()).unwrap();
        let mut desired: Vec<_> = current.tags.iter().map(|tag| tag.id.clone()).collect();
        desired.push(tag.id);
        backend
            .set_ticket_tags(
                "reasoned".into(),
                ticket::TicketTagSetRequest {
                    operation_key: format!("attach-{name}"),
                    expected_tag_ids: current.tags.iter().map(|tag| tag.id.clone()).collect(),
                    tag_ids: desired,
                },
            )
            .unwrap();
    }
    let after = backend.show("reasoned".into()).unwrap();
    assert_eq!(after.meta, before.meta);
    assert_eq!(after.current_closure, before.current_closure);
    assert_eq!(after.case_history, before.case_history);
    assert_eq!(
        ticket::ticket_content_digest(&after),
        ticket::ticket_content_digest(&before)
    );
}
