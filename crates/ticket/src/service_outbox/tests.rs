//! Real backend operations against isolated SQLite storage; no guest or delivery
//! driver is needed to prove the domain transaction's durable outbox contract.

use super::*;
use crate::*;
use std::sync::Arc;
use tempfile::TempDir;

struct Fixture {
    _temp: TempDir,
    backend: SqliteTicketBackend,
    conn: Connection,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("tickets.db");
        let backend = SqliteTicketBackend::open(&path, "workspace").unwrap();
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        Self {
            _temp: temp,
            backend,
            conn,
        }
    }

    fn ticket(&self) -> Ticket {
        let mut input = NewTicket::new("Private title");
        input.body = "Private body".into();
        input.author = Some("Private actor".into());
        self.backend.create_with_snapshot(input).unwrap().1
    }

    fn rows(&self) -> Vec<Row> {
        self.conn.prepare("SELECT sequence,event_id,workspace_id,ticket_id,event_type,occurred_at,domain_event_ref,content_digest,correlation_id,causation_id FROM typed_ticket_service_outbox ORDER BY sequence").unwrap()
            .query_map([], |row| Ok(Row {
                sequence: row.get(0)?, id: row.get(1)?, workspace: row.get(2)?, ticket: row.get(3)?,
                kind: row.get(4)?, at: row.get(5)?, domain_ref: row.get(6)?, digest: row.get(7)?,
                correlation: row.get(8)?, causation: row.get(9)?,
            })).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    sequence: i64,
    id: String,
    workspace: String,
    ticket: String,
    kind: String,
    at: String,
    domain_ref: String,
    digest: String,
    correlation: String,
    causation: String,
}

#[test]
fn create_append_edits_and_case_emit_actual_digest_and_exact_domain_identity() {
    let f = Fixture::new();
    let initial = f.ticket();
    let id = initial.meta.id.clone();
    let created = f.rows().pop().unwrap();
    assert_eq!(created.domain_ref, initial.events[0].event_ref);
    assert_eq!(created.digest, ticket_content_digest(&initial));
    assert_eq!(created.at, initial.events[0].at.as_deref().unwrap());
    assert_eq!(
        (
            created.workspace.as_str(),
            created.ticket.as_str(),
            created.kind.as_str()
        ),
        ("workspace", id.as_str(), "changed")
    );
    assert_eq!(created.correlation, created.id);
    assert_eq!(created.causation, created.domain_ref);

    f.backend
        .add_event(
            id.clone().into(),
            NewTicketEvent::new(TicketEventKind::Comment, "Private comment"),
        )
        .unwrap();
    let appended = f.backend.show(id.clone().into()).unwrap();
    assert_eq!(
        f.rows()[1].domain_ref,
        appended.events.last().unwrap().event_ref
    );
    assert_eq!(f.rows()[1].digest, ticket_content_digest(&initial));

    let edited = f
        .backend
        .edit_item(
            id.clone().into(),
            TicketItemEdit {
                title: Some("Changed private title".into()),
                body: Some("Changed private body".into()),
                targets: Some(TicketTargetsEdit::Set {
                    targets: vec![TicketTarget {
                        repository_key: "main".into(),
                        ref_selector: Some("develop".into()),
                        access: TicketTargetAccess::ReadWrite,
                    }],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let change_id: String = f
        .conn
        .query_row(
            "SELECT change_id FROM typed_ticket_content_changes ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(f.rows()[2].domain_ref, change_id);
    assert_eq!(f.rows()[2].digest, ticket_content_digest(&edited));
    assert_ne!(f.rows()[2].digest, created.digest);

    let post = edited.events.last().unwrap();
    f.backend
        .edit_post_with_snapshots(
            id.clone().into(),
            TicketPostEdit {
                event_ref: post.event_ref.clone(),
                expected_body: post.body.clone(),
                body: "Edited private comment".into(),
                author: "Private editor".into(),
            },
        )
        .unwrap();
    let change_id: String = f
        .conn
        .query_row(
            "SELECT change_id FROM typed_ticket_content_changes ORDER BY rowid DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(f.rows()[3].domain_ref, change_id);
    assert_eq!(f.rows()[3].digest, ticket_content_digest(&edited));

    let request = TicketCaseUpdate {
        expected_content_digest: ticket_content_digest(&edited),
        expected_state: TicketState::Open,
        expected_closure_operation_key: None,
        operation_key: "close-once".into(),
        action: TicketCaseAction::Close {
            reason: TicketClosureReason::Completed,
        },
        explanation: "Private judgment".into(),
        references: vec![],
        author: Some("Private judge".into()),
    };
    let closed = f.backend.update_case(&id, request.clone()).unwrap();
    assert_eq!(
        f.rows()[4].domain_ref,
        closed.events.last().unwrap().event_ref
    );
    assert_eq!(f.rows()[4].digest, ticket_content_digest(&edited));
    let before = f.rows();
    f.backend.update_case(&id, request).unwrap();
    f.backend
        .edit_item(
            id.into(),
            TicketItemEdit {
                title: Some(edited.meta.title),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        f.rows(),
        before,
        "replay and no-op must not publish new events"
    );
    assert!(
        before
            .windows(2)
            .all(|rows| rows[0].sequence < rows[1].sequence && rows[0].id != rows[1].id)
    );
    let columns: Vec<String> = f
        .conn
        .prepare("PRAGMA table_info(typed_ticket_service_outbox)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "sequence",
            "event_id",
            "workspace_id",
            "ticket_id",
            "event_type",
            "occurred_at",
            "domain_event_ref",
            "content_digest",
            "correlation_id",
            "causation_id"
        ]
    );
}

#[test]
fn domain_or_outbox_failure_rolls_back_both_sides_of_real_backend_mutations() {
    let f = Fixture::new();
    let ticket = f.ticket();
    let before = f.rows();
    let failing = f.backend.clone().with_mutation_hook(Arc::new(|_, _| {
        Err(TicketError::Conflict("injected after outbox insert".into()))
    }));
    assert!(
        failing
            .add_event(
                ticket.meta.id.clone().into(),
                NewTicketEvent::new(TicketEventKind::Comment, "Must roll back")
            )
            .is_err()
    );
    assert!(failing.create(NewTicket::new("Must not exist")).is_err());
    assert_eq!(f.rows(), before);
    assert_eq!(
        f.backend
            .show(ticket.meta.id.clone().into())
            .unwrap()
            .events,
        ticket.events
    );
    assert_eq!(f.backend.list(TicketListQuery::default()).unwrap().len(), 1);

    f.conn.execute_batch("CREATE TRIGGER reject_outbox BEFORE INSERT ON typed_ticket_service_outbox BEGIN SELECT RAISE(ABORT,'injected outbox failure'); END;").unwrap();
    assert!(
        f.backend
            .edit_item(
                ticket.meta.id.clone().into(),
                TicketItemEdit {
                    title: Some("Rejected edit".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(
        f.backend
            .show(ticket.meta.id.clone().into())
            .unwrap()
            .meta
            .title,
        ticket.meta.title
    );
    assert_eq!(
        f.conn
            .query_row(
                "SELECT count(*) FROM typed_ticket_content_changes",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(f.rows(), before);
}

#[test]
fn deletion_captures_live_digest_and_retains_safe_metadata_only_on_commit() {
    let f = Fixture::new();
    let ticket = f.ticket();
    let id = &ticket.meta.id;
    assert!(
        record_ticket_deletion_in_transaction(&f.conn, "workspace", id, "delete-operation")
            .is_err()
    );
    let before = f.rows();
    f.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    record_ticket_deletion_in_transaction(&f.conn, "workspace", id, "delete-operation").unwrap();
    f.conn
        .execute(
            "DELETE FROM typed_tickets WHERE workspace_id='workspace' AND ticket_id=?1",
            [id],
        )
        .unwrap();
    f.conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(f.rows(), before);
    f.backend.show(id.clone().into()).unwrap();
    f.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let event_id =
        record_ticket_deletion_in_transaction(&f.conn, "workspace", id, "delete-operation")
            .unwrap();
    f.conn
        .execute(
            "DELETE FROM typed_tickets WHERE workspace_id='workspace' AND ticket_id=?1",
            [id],
        )
        .unwrap();
    f.conn.execute_batch("COMMIT").unwrap();
    let row = f.rows().pop().unwrap();
    assert_eq!(row.id, event_id);
    assert_eq!(row.kind, "deleted");
    assert_eq!(row.domain_ref, "delete-operation");
    assert_eq!(row.digest, ticket_content_digest(&ticket));
    assert!(f.backend.show(id.clone().into()).is_err());
    f.conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        record_ticket_deletion_in_transaction(&f.conn, "workspace", id, "missing-delete").is_err()
    );
    f.conn.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn initial_post_edit_uses_edit_fact_and_rollback_removes_edit_and_outbox() {
    let f = Fixture::new();
    let ticket = f.ticket();
    let edit = TicketPostEdit {
        event_ref: ticket.events[0].event_ref.clone(),
        expected_body: ticket.document.body.clone(),
        body: "Edited initial requirements".into(),
        author: "Private editor".into(),
    };
    let (_, current) = f
        .backend
        .edit_post_with_snapshots(ticket.meta.id.clone().into(), edit)
        .unwrap();
    let (change_id, at): (String, String) = f
        .conn
        .query_row(
            "SELECT change_id,at FROM typed_ticket_content_changes",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let row = f.rows().pop().unwrap();
    assert_eq!(row.domain_ref, change_id);
    assert_eq!(row.at, at);
    assert_eq!(row.digest, ticket_content_digest(&current));
    assert_ne!(row.digest, ticket_content_digest(&ticket));
    let before = f.rows();
    f.conn.execute_batch("CREATE TRIGGER reject_outbox BEFORE INSERT ON typed_ticket_service_outbox BEGIN SELECT RAISE(ABORT,'outbox rejected'); END;").unwrap();
    assert!(
        f.backend
            .edit_post_with_snapshots(
                ticket.meta.id.clone().into(),
                TicketPostEdit {
                    event_ref: current.events[0].event_ref.clone(),
                    expected_body: current.document.body.clone(),
                    body: "Must roll back".into(),
                    author: "Private editor".into(),
                }
            )
            .is_err()
    );
    assert_eq!(f.backend.show(ticket.meta.id.into()).unwrap(), current);
    assert_eq!(f.rows(), before);
    assert_eq!(
        f.conn
            .query_row(
                "SELECT count(*) FROM typed_ticket_content_changes",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn outbox_enforces_typed_bounds_unique_identity_and_no_sequence_reuse_after_retention() {
    let f = Fixture::new();
    let ticket = f.ticket();
    let first = f.rows().pop().unwrap();
    for mutation in [
        "event_type='arbitrary-name'",
        "workspace_id=''",
        "ticket_id=replace(hex(zeroblob(257)),'0','x')",
        "domain_event_ref=replace(hex(zeroblob(513)),'0','x')",
        "occurred_at=''",
        "content_digest='dummy'",
        "correlation_id=''",
        "causation_id=''",
        "event_id='not-hex'",
    ] {
        assert!(
            f.conn
                .execute(
                    &format!("UPDATE typed_ticket_service_outbox SET {mutation}"),
                    []
                )
                .is_err(),
            "{mutation}"
        );
    }
    assert!(f.conn.execute("INSERT INTO typed_ticket_service_outbox SELECT NULL,event_id,workspace_id,ticket_id,event_type,occurred_at,domain_event_ref,content_digest,correlation_id,causation_id FROM typed_ticket_service_outbox", []).is_err());
    f.conn
        .execute("DELETE FROM typed_ticket_service_outbox", [])
        .unwrap();
    f.backend
        .add_event(
            ticket.meta.id.into(),
            NewTicketEvent::new(TicketEventKind::Comment, "After retention"),
        )
        .unwrap();
    let next = f.rows().pop().unwrap();
    assert!(next.sequence > first.sequence);
    assert_ne!(next.id, first.id);
}
