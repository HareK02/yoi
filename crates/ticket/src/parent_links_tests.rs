//! Real SQLite component tests. Authentication and public adapters are outside this
//! suite; it exercises the owning storage transaction and rereads durable effects.

use super::*;
use tempfile::tempdir;

fn create(backend: &SqliteTicketBackend, title: &str) -> String {
    backend.create(NewTicket::new(title)).unwrap().id
}

fn observe(backend: &SqliteTicketBackend, child: &str) -> TicketParentObservation {
    backend
        .parent_link(TicketIdOrSlug::Id(child.into()))
        .unwrap()
        .observation
}

fn request(
    backend: &SqliteTicketBackend,
    child: &str,
    parent: Option<&str>,
    key: &str,
) -> TicketParentSetRequest {
    TicketParentSetRequest {
        expected: observe(backend, child),
        parent: parent.map(str::to_owned),
        operation_key: key.into(),
    }
}

fn children(backend: &SqliteTicketBackend, parent: &str) -> Vec<String> {
    backend
        .children_page(parent.to_owned().into(), TicketChildrenRequest::default())
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.id)
        .collect()
}

fn counts(backend: &SqliteTicketBackend, child: &str) -> (i64, i64, i64) {
    let conn = backend.open_connection().unwrap();
    let count = |table: &str, column: &str| {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE workspace_id=?1 AND {column}=?2"),
            params![backend.workspace_id, child],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
    };
    (
        count("typed_ticket_parent_operations", "ticket_id"),
        count("typed_ticket_events", "ticket_id"),
        count("typed_ticket_service_outbox", "ticket_id"),
    )
}

#[test]
fn reparent_and_unlink_persist_one_forward_authority_without_changing_requirements() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let c = create(&backend, "C");
    let d = create(&backend, "D");
    let before = backend.show(b.clone().into()).unwrap();
    backend
        .set_parent_link(b.clone().into(), request(&backend, &b, Some(&a), "b-a"))
        .unwrap();
    backend
        .set_parent_link(c.clone().into(), request(&backend, &c, Some(&b), "c-b"))
        .unwrap();
    assert_eq!(children(&backend, &a), vec![b.clone()]);
    assert_eq!(children(&backend, &b), vec![c.clone()]);
    backend
        .set_parent_link(b.clone().into(), request(&backend, &b, Some(&d), "b-d"))
        .unwrap();
    assert!(children(&backend, &a).is_empty());
    assert_eq!(children(&backend, &d), vec![b.clone()]);
    let saved = backend
        .set_parent_link(b.clone().into(), request(&backend, &b, None, "unlink"))
        .unwrap();
    assert_eq!(saved.observation.parent_ticket_id, None);
    assert_eq!(
        saved.observation.last_operation_key.as_deref(),
        Some("unlink")
    );
    let reopened = SqliteTicketBackend::open_verified(backend.db_path(), "ws").unwrap();
    assert_eq!(observe(&reopened, &b), saved.observation);
    assert!(children(&reopened, &d).is_empty());
    assert_eq!(children(&reopened, &b), vec![c]);
    let after = reopened.show(b.into()).unwrap();
    assert_eq!(before.document, after.document);
    assert_eq!(before.meta.workflow_state, after.meta.workflow_state);
    assert_eq!(before.meta.targets, after.meta.targets);
    assert_eq!(before.relations, after.relations);
    assert_eq!(before.current_closure, after.current_closure);
}

#[test]
fn parent_cas_rejects_aba_and_replay_returns_historical_receipt_without_new_effects() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let first = request(&backend, &b, Some(&a), "first");
    let saved = backend
        .set_parent_link(b.clone().into(), first.clone())
        .unwrap();
    let stale = request(&backend, &b, None, "stale");
    backend
        .set_parent_link(b.clone().into(), request(&backend, &b, None, "unlink"))
        .unwrap();
    backend
        .set_parent_link(b.clone().into(), request(&backend, &b, Some(&a), "again"))
        .unwrap();
    let before = counts(&backend, &b);
    assert!(matches!(
        backend.set_parent_link(b.clone().into(), stale),
        Err(TicketError::Conflict(_))
    ));
    // Simulate losing the first response and retrying after subsequent changes.
    assert_eq!(
        backend
            .set_parent_link(b.clone().into(), first.clone())
            .unwrap(),
        saved
    );
    assert_eq!(
        observe(&backend, &b).last_operation_key.as_deref(),
        Some("again")
    );
    let changed_input = TicketParentSetRequest {
        parent: None,
        ..first.clone()
    };
    assert!(matches!(
        backend.set_parent_link(b.clone().into(), changed_input),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    let other_actor = backend.clone().with_event_attributes(BTreeMap::from([(
        "source_worker_id".into(),
        "other".into(),
    )]));
    assert!(matches!(
        other_actor.set_parent_link(b.clone().into(), first),
        Err(TicketError::OperationFingerprintMismatch { .. })
    ));
    assert_eq!(counts(&backend, &b), before);
}

#[test]
fn invalid_parent_rejects_self_missing_foreign_and_indirect_cycles_atomically() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("parents.db");
    let backend = SqliteTicketBackend::open(&path, "ws").unwrap();
    let foreign = SqliteTicketBackend::open(&path, "foreign").unwrap();
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let c = create(&backend, "C");
    let foreign_id = create(&foreign, "Foreign");
    backend
        .set_parent_link(b.clone().into(), request(&backend, &b, Some(&a), "b-a"))
        .unwrap();
    backend
        .set_parent_link(c.clone().into(), request(&backend, &c, Some(&b), "c-b"))
        .unwrap();
    let before = counts(&backend, &a);
    for (i, invalid) in [a.as_str(), "missing", foreign_id.as_str(), c.as_str()]
        .into_iter()
        .enumerate()
    {
        assert!(
            backend
                .set_parent_link(
                    a.clone().into(),
                    request(&backend, &a, Some(invalid), &format!("invalid-{i}"))
                )
                .is_err()
        );
        assert_eq!(observe(&backend, &a), TicketParentObservation::default());
        assert_eq!(counts(&backend, &a), before);
    }
}

#[test]
fn deep_ancestor_walk_does_not_treat_a_depth_cutoff_as_acyclic() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let root = create(&backend, "Root");
    // A large persisted ancestry fixture avoids thousands of unrelated creation
    // transactions. The mutation and entire ancestor validation remain production.
    backend.with_write(|conn| {
        let mut previous = root.clone();
        for depth in 0..1200 {
            let id = format!("deep-{depth:04}");
            conn.execute(
                "INSERT INTO typed_tickets (workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit)
                 VALUES ('ws',?1,?1,'Deep','open','task','P2','open',1)", [&id],
            ).map_err(sqlite_err)?;
            conn.execute(
                "INSERT INTO typed_ticket_parent_links VALUES ('ws',?1,?2,?1)", params![id, previous],
            ).map_err(sqlite_err)?;
            previous = id;
        }
        Ok(())
    }).unwrap();
    assert!(matches!(
        backend.set_parent_link(
            root.clone().into(),
            request(&backend, &root, Some("deep-1199"), "cycle")
        ),
        Err(TicketError::Conflict(_))
    ));
    assert_eq!(observe(&backend, &root), TicketParentObservation::default());
}

#[test]
fn reciprocal_parallel_reparents_cannot_both_commit() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let tasks = [(a.clone(), b.clone(), "a-b"), (b.clone(), a.clone(), "b-a")].map(
        |(child, parent, key)| {
            let backend = backend.clone();
            let barrier = barrier.clone();
            let input = request(&backend, &child, Some(&parent), key);
            std::thread::spawn(move || {
                barrier.wait();
                backend.set_parent_link(child.into(), input)
            })
        },
    );
    let results = tasks.map(|task| task.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let links = [observe(&backend, &a), observe(&backend, &b)];
    assert_eq!(
        links
            .iter()
            .filter(|link| link.parent_ticket_id.is_some())
            .count(),
        1
    );
}

#[test]
fn audit_outbox_failure_rolls_back_link_and_receipt_together() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let before = counts(&backend, &b);
    let failing = backend.clone().with_mutation_hook(Arc::new(|_, _| {
        Err(TicketError::Conflict("injected domain hook failure".into()))
    }));
    let input = request(&backend, &b, Some(&a), "link");
    assert!(
        failing
            .set_parent_link(b.clone().into(), input.clone())
            .is_err()
    );
    assert_eq!(observe(&backend, &b), TicketParentObservation::default());
    assert_eq!(counts(&backend, &b), before);
    backend
        .set_parent_link(b.clone().into(), input.clone())
        .unwrap();
    let after = counts(&backend, &b);
    assert_eq!(after, (before.0 + 1, before.1 + 1, before.2 + 1));
    backend.set_parent_link(b.clone().into(), input).unwrap();
    assert_eq!(counts(&backend, &b), after);
}

#[test]
fn children_keyset_pages_are_bounded_sorted_and_not_duplicated() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let parent = create(&backend, "Parent");
    let mut expected = Vec::new();
    for i in 0..5 {
        let child = create(&backend, &format!("Child {i}"));
        backend
            .set_parent_link(
                child.clone().into(),
                request(&backend, &child, Some(&parent), &format!("link-{i}")),
            )
            .unwrap();
        expected.push(child);
    }
    expected.sort();
    let mut cursor = None;
    let mut actual = Vec::new();
    loop {
        let page = backend
            .children_page(
                parent.clone().into(),
                TicketChildrenRequest {
                    cursor,
                    limit: Some(2),
                },
            )
            .unwrap();
        assert!(page.items.len() <= 2);
        assert_eq!(page.has_more, page.next_cursor.is_some());
        actual.extend(page.items.into_iter().map(|item| item.id));
        cursor = page.next_cursor;
        if !page.has_more {
            break;
        }
    }
    assert_eq!(actual, expected);
    for limit in [0, 101] {
        assert!(
            backend
                .children_page(
                    parent.clone().into(),
                    TicketChildrenRequest {
                        cursor: None,
                        limit: Some(limit)
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn parent_wire_contract_requires_explicit_nullable_observation_and_desired_parent() {
    for input in [
        r#"{"expected":{},"parent":null,"operation_key":"key"}"#,
        r#"{"expected":{"parent_ticket_id":null},"parent":null,"operation_key":"key"}"#,
        r#"{"expected":{"parent_ticket_id":null,"last_operation_key":null},"operation_key":"key"}"#,
    ] {
        assert!(serde_json::from_str::<TicketParentSetRequest>(input).is_err());
    }
    let input = r#"{"expected":{"parent_ticket_id":null,"last_operation_key":null},"parent":null,"operation_key":"key"}"#;
    assert!(serde_json::from_str::<TicketParentSetRequest>(input).is_ok());
}

fn outbox_schema_preimage() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    migrate_historical_sqlite_ticket_schema_in_transaction(
        &conn,
        HistoricalTicketSchemaTarget::Outbox15,
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    conn
}

#[test]
fn parent_migration_preserves_existing_relations_and_receipts_and_defaults_to_no_parent() {
    let conn = outbox_schema_preimage();
    conn.execute_batch(
        "INSERT INTO typed_tickets (workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit)
         VALUES ('ws','a','a','A','open','task','P2','open',1), ('ws','b','b','B','open','task','P2','open',1);
         INSERT INTO typed_ticket_relations VALUES ('ws','a','related','b','kept','actor','at');",
    ).unwrap();
    let old_receipt: (String, String) = conn
        .query_row(
            "SELECT name,applied_at FROM ticket_schema_migrations WHERE version=15",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
    let receipt: (String, String) = conn
        .query_row(
            "SELECT name,applied_at FROM ticket_schema_migrations WHERE version=15",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, old_receipt);
    let parent_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM typed_ticket_parent_links",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_count, 0);
    let relation: (String, String) = conn
        .query_row(
            "SELECT kind,note FROM typed_ticket_relations WHERE ticket_id='a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(relation, ("related".into(), "kept".into()));
    let fresh = Connection::open_in_memory().unwrap();
    migrate_sqlite_ticket_schema(&fresh).unwrap();
    let shape = |db: &Connection| {
        let mut stmt = db.prepare("SELECT name,sql FROM sqlite_master WHERE name LIKE 'typed_ticket_parent_%' AND sql IS NOT NULL ORDER BY name").unwrap();
        stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
    };
    assert_eq!(shape(&conn), shape(&fresh));
}

#[test]
fn parent_migration_failure_rolls_back_ddl_and_retains_original_schema_receipt() {
    let conn = outbox_schema_preimage();
    conn.execute_batch("CREATE TABLE typed_ticket_parent_operations (unexpected TEXT)")
        .unwrap();
    assert!(migrate_sqlite_ticket_schema(&conn).is_err());
    let parent_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='typed_ticket_parent_links'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(parent_tables, 0);
    let version: i64 = conn
        .query_row(
            "SELECT MAX(version) FROM ticket_schema_migrations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, 15);
    verify_historical_sqlite_ticket_schema(&conn, HistoricalTicketSchemaTarget::Outbox15).unwrap();
}

#[test]
fn simultaneous_changes_to_one_child_cannot_overwrite_the_same_observation() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let child = create(&backend, "Child");
    let a = create(&backend, "A");
    let b = create(&backend, "B");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let tasks = [(a, "a"), (b, "b")].map(|(parent, key)| {
        let input = request(&backend, &child, Some(&parent), key);
        let backend = backend.clone();
        let child = child.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            backend.set_parent_link(child.into(), input)
        })
    });
    let results = tasks.map(|task| task.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let winner = results
        .into_iter()
        .find_map(std::result::Result::ok)
        .unwrap();
    assert_eq!(observe(&backend, &child), winner.observation);
}

#[test]
fn children_cursor_rejects_other_parent_workspace_and_malformed_input() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("parents.db");
    let backend = SqliteTicketBackend::open(&path, "ws").unwrap();
    let parent = create(&backend, "Parent");
    let other = create(&backend, "Other parent");
    for i in 0..2 {
        let child = create(&backend, &format!("Child {i}"));
        backend
            .set_parent_link(
                child.clone().into(),
                request(&backend, &child, Some(&parent), &format!("link-{i}")),
            )
            .unwrap();
    }
    let cursor = backend
        .children_page(
            parent.clone().into(),
            TicketChildrenRequest {
                cursor: None,
                limit: Some(1),
            },
        )
        .unwrap()
        .next_cursor
        .unwrap();
    let foreign = SqliteTicketBackend::open(&path, "foreign").unwrap();
    let foreign_parent = create(&foreign, "Foreign parent");
    for (store, target) in [(&backend, other), (&foreign, foreign_parent)] {
        assert!(
            store
                .children_page(
                    target.into(),
                    TicketChildrenRequest {
                        cursor: Some(cursor.clone()),
                        limit: Some(1)
                    }
                )
                .is_err()
        );
    }
    for invalid in ["not-json".to_owned(), "x".repeat(4097)] {
        assert!(
            backend
                .children_page(
                    parent.clone().into(),
                    TicketChildrenRequest {
                        cursor: Some(invalid),
                        limit: Some(1)
                    }
                )
                .is_err()
        );
    }
}

#[test]
fn closed_child_projection_preserves_reason_and_does_not_close_parent_or_remove_dependencies() {
    let dir = tempdir().unwrap();
    let backend = SqliteTicketBackend::open(dir.path().join("parents.db"), "ws").unwrap();
    let parent = create(&backend, "Parent");
    let child = create(&backend, "Child");
    backend
        .add_ticket_relation(
            child.clone().into(),
            NewTicketRelation {
                kind: TicketRelationKind::DependsOn,
                target: parent.clone(),
                note: None,
                author: None,
            },
        )
        .unwrap();
    let before = backend.show(child.clone().into()).unwrap();
    backend
        .set_parent_link(
            child.clone().into(),
            request(&backend, &child, Some(&parent), "link"),
        )
        .unwrap();
    backend
        .update_case(
            &child,
            TicketCaseUpdate {
                expected_content_digest: ticket_content_digest(&before),
                expected_state: TicketState::Open,
                expected_closure_operation_key: None,
                operation_key: "not-planned".into(),
                action: TicketCaseAction::Close {
                    reason: TicketClosureReason::NotPlanned,
                },
                explanation: "Not proceeding".into(),
                references: vec![],
                author: Some("case-actor".into()),
            },
        )
        .unwrap();
    let page = backend
        .children_page(parent.clone().into(), TicketChildrenRequest::default())
        .unwrap();
    assert_eq!(page.items[0].state, TicketState::Closed);
    assert_eq!(
        page.items[0].closure_reason,
        Some(TicketClosureReason::NotPlanned)
    );
    let after = backend.show(child.into()).unwrap();
    assert_eq!(after.relations, before.relations);
    assert_eq!(after.meta.targets, before.meta.targets);
    assert_eq!(
        backend.show(parent.into()).unwrap().meta.workflow_state,
        TicketState::Open
    );
}
