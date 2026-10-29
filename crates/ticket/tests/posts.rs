//! Real SQLite component tests through TicketBackend, including durable edit facts.
//! Host capture is represented only by test triggers; Workspace routing is not tested here.

#[path = "posts/content_changes.rs"]
mod content_changes;

use rusqlite::{Connection, params};
use tempfile::TempDir;
use ticket::{
    MarkdownText, NewTicket, NewTicketEvent, SqliteTicketBackend, TicketBackend, TicketEventKind,
    TicketItemEdit, TicketPostEdit, migrate_sqlite_ticket_schema,
    migrate_sqlite_ticket_schema_in_transaction, ticket_content_digest,
    verify_sqlite_ticket_schema,
};

fn comment(body: &str, author: &str) -> NewTicketEvent {
    let mut event = NewTicketEvent::new(TicketEventKind::Comment, body);
    event.author = Some(author.into());
    event
}

#[test]
fn initial_and_discussion_comments_share_identity_authorship_and_cas_edits() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("posts.db");
    let backend = SqliteTicketBackend::open(&path, "workspace").unwrap();
    let mut input = NewTicket::new("Real first post");
    input.body = MarkdownText::new("First authored post");
    input.author = Some("creator".into());
    let (reference, created) = backend.create_with_snapshot(input).unwrap();
    assert_eq!(created.events.len(), 1);
    let initial = &created.events[0];
    assert_eq!(initial.kind, TicketEventKind::Comment);
    assert!(initial.is_initial);
    assert_eq!(initial.body, created.document.body);
    assert_eq!(initial.author.as_deref(), Some("creator"));
    assert_eq!(initial.at, created.meta.created_at);
    assert_eq!(created.meta.created_by.as_deref(), Some("creator"));
    assert!(initial.edited_at.is_none());
    backend
        .add_event(
            reference.id.clone().into(),
            comment("Discussion", "commenter"),
        )
        .unwrap();
    let before = backend.show(reference.id.clone().into()).unwrap();
    let ordinary = before.events[1].clone();
    assert!(!ordinary.is_initial);
    assert_eq!(ordinary.kind, initial.kind);
    let initial_time = initial.at.clone();
    for original in [ordinary, initial.clone()] {
        let edit = TicketPostEdit {
            event_ref: original.event_ref.clone(),
            expected_body: original.body.clone(),
            body: MarkdownText::new(format!("Edited {}", original.event_ref)),
            author: "editor".into(),
        };
        let (previous, current) = backend
            .edit_post_with_snapshots(reference.id.clone().into(), edit.clone())
            .unwrap();
        let post = current
            .events
            .iter()
            .find(|p| p.event_ref == original.event_ref)
            .unwrap();
        assert_eq!(post.author, original.author);
        assert_eq!(post.at, original.at);
        assert_eq!(post.edited_by.as_deref(), Some("editor"));
        assert!(post.edited_at.is_some());
        assert_eq!(current.meta.created_by.as_deref(), Some("creator"));
        assert_eq!(current.meta.created_at, initial_time);
        assert_eq!(current.events.len(), 2);
        if original.is_initial {
            assert_eq!(current.document.body, edit.body);
            assert_ne!(
                ticket_content_digest(&previous),
                ticket_content_digest(&current)
            );
        } else {
            assert_eq!(
                ticket_content_digest(&previous),
                ticket_content_digest(&current)
            );
        }
        assert!(
            backend
                .edit_post(reference.id.clone().into(), edit)
                .is_err()
        );
        assert_eq!(backend.show(reference.id.clone().into()).unwrap(), current);
    }
    let previous = backend.show(reference.id.clone().into()).unwrap();
    let edited = backend
        .edit_item(
            reference.id.clone().into(),
            TicketItemEdit {
                title: Some("New title".into()),
                body: Some(MarkdownText::new("Item authoring edits initial post")),
                author: Some("item-editor".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(edited.events.len(), previous.events.len());
    assert_eq!(edited.events[0].author, previous.events[0].author);
    assert_eq!(edited.events[0].at, previous.events[0].at);
    assert_eq!(edited.events[0].edited_by.as_deref(), Some("item-editor"));
    assert_eq!(edited.document.body, edited.events[0].body);
    assert!(
        edited
            .events
            .iter()
            .all(|e| !matches!(e.kind.as_str(), "create" | "item_edit"))
    );
    let reopened = SqliteTicketBackend::open(&path, "workspace").unwrap();
    assert_eq!(reopened.show(reference.id.into()).unwrap(), edited);
    let conn = Connection::open(path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM pragma_table_info('typed_tickets') WHERE name='body'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}

#[test]
fn editing_rejects_system_judgments_other_tickets_and_missing_identity() {
    let temp = TempDir::new().unwrap();
    let backend = SqliteTicketBackend::open(temp.path().join("posts.db"), "workspace").unwrap();
    let first = backend.create(NewTicket::new("first")).unwrap();
    let second = backend.create(NewTicket::new("second")).unwrap();
    let mut event = comment("System judgment", "system");
    event.kind = TicketEventKind::Decision;
    backend.add_event(first.id.clone().into(), event).unwrap();
    let ticket = backend.show(first.id.clone().into()).unwrap();
    let system = &ticket.events[1];
    let edit = TicketPostEdit {
        event_ref: system.event_ref.clone(),
        expected_body: system.body.clone(),
        body: MarkdownText::new("changed"),
        author: "editor".into(),
    };
    assert!(
        backend
            .edit_post(first.id.clone().into(), edit.clone())
            .is_err()
    );
    assert!(backend.edit_post(second.id.into(), edit).is_err());
    let initial = &ticket.events[0];
    assert!(
        backend
            .edit_post(
                first.id.clone().into(),
                TicketPostEdit {
                    event_ref: initial.event_ref.clone(),
                    expected_body: initial.body.clone(),
                    body: MarkdownText::new("changed"),
                    author: " ".into()
                }
            )
            .is_err()
    );
    assert_eq!(backend.show(first.id.into()).unwrap(), ticket);
}

fn schema11(conn: &Connection) {
    // Build the actual retained component shape, not a current schema with its marker changed.
    conn.execute_batch(include_str!("../src/latest_schema.sql"))
        .unwrap();
    conn.execute_batch(include_str!("../src/assignment_schema.sql"))
        .unwrap();
    conn.execute_batch(include_str!("../src/assignment_tracking_schema.sql"))
        .unwrap();
    conn.execute_batch("CREATE TABLE ticket_schema_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at TEXT NOT NULL); INSERT INTO ticket_schema_migrations VALUES(11,'ticket schema baseline','original-marker');").unwrap();
}

fn seed_legacy(conn: &Connection, id: &str) {
    conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,body,created_at,workflow_state,workflow_state_explicit) VALUES('workspace',?1,?1,'Legacy','open','task','P2','Latest body','ticket-time','open',1)", [id]).unwrap();
}

#[test]
fn schema12_migrates_body_authors_archived_history_and_stable_references_then_replays() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    schema11(&conn);
    for id in ["known", "unknown", "metadata"] {
        seed_legacy(&conn, id);
    }
    conn.execute_batch("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,body) VALUES
        ('workspace','known',0,'create','original-author','original-time','Created'),
        ('workspace','known',1,'comment','discussant','discussion-time','Discussion'),
        ('workspace','known',2,'item_edit','legacy-editor','edit-time','Body changed');
        INSERT INTO typed_ticket_event_attributes VALUES('workspace','known',1,'event_id','external-original-id');
        INSERT INTO typed_ticket_event_attributes VALUES('workspace','known',2,'body_edit','whole');
        INSERT INTO typed_ticket_event_references VALUES('workspace','known',2,0,'commit','original-evidence');
        INSERT INTO typed_ticket_raw_frontmatter VALUES('workspace','metadata','created_by','metadata-author');").unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
    let backend = SqliteTicketBackend::open_verified(&path, "workspace").unwrap();
    let known = backend.show("known".into()).unwrap();
    assert_eq!(known.meta.created_at.as_deref(), Some("ticket-time"));
    assert_eq!(known.meta.created_by.as_deref(), Some("original-author"));
    assert_eq!(known.events.len(), 2);
    assert_eq!(known.events[0].author.as_deref(), Some("original-author"));
    assert_eq!(known.events[0].at.as_deref(), Some("original-time"));
    assert_eq!(known.document.body.as_str(), "Latest body");
    assert_eq!(known.events[0].body, known.document.body);
    assert_eq!(known.events[1].event_ref, "known:1");
    assert_eq!(known.events[1].attributes["event_id"], "known:1");
    assert_eq!(conn.query_row("SELECT value FROM typed_ticket_event_attributes WHERE ticket_id='known' AND event_index=1 AND key='event_id'", [], |r| r.get::<_,String>(0)).unwrap(), "external-original-id");
    assert_eq!(known.events[1].attributes["event_sequence"], "1");
    let unknown = backend.show("unknown".into()).unwrap();
    assert_eq!(unknown.meta.created_by, None);
    assert_eq!(unknown.events[0].author, None);
    assert_eq!(unknown.events[0].at.as_deref(), Some("ticket-time"));
    assert_eq!(
        backend.show("metadata".into()).unwrap().events[0]
            .author
            .as_deref(),
        Some("metadata-author")
    );
    let history = || {
        conn.prepare("SELECT event_index,kind,author,at,body,archived FROM typed_ticket_events WHERE ticket_id='known' AND event_index>=0 ORDER BY event_index").unwrap().query_map([], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,bool>(5)?))).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    let original_history = history();
    assert_eq!(
        original_history[0],
        (
            0,
            "create".into(),
            "original-author".into(),
            "original-time".into(),
            "Created".into(),
            true
        )
    );
    assert_eq!(
        original_history[2],
        (
            2,
            "item_edit".into(),
            "legacy-editor".into(),
            "edit-time".into(),
            "Body changed".into(),
            true
        )
    );
    assert_eq!(conn.query_row("SELECT target FROM typed_ticket_event_references WHERE ticket_id='known' AND event_index=2", [], |r| r.get::<_,String>(0)).unwrap(), "original-evidence");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(history(), original_history);
    assert_eq!(backend.show("known".into()).unwrap(), known);
    assert_eq!(
        conn.query_row(
            "SELECT applied_at FROM ticket_schema_migrations WHERE version=11",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "original-marker"
    );
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        14
    );
    backend
        .add_event("known".into(), comment("Next", "author"))
        .unwrap();
    assert_eq!(
        backend
            .show("known".into())
            .unwrap()
            .events
            .last()
            .unwrap()
            .event_ref,
        "known:3"
    );
}

#[test]
fn schema12_reserved_index_collision_rolls_back_component_and_host_can_retry() {
    let conn = Connection::open_in_memory().unwrap();
    schema11(&conn);
    seed_legacy(&conn, "collision");
    conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body) VALUES('workspace','collision',-1,'comment','retained negative index')", []).unwrap();
    conn.execute_batch("CREATE TABLE host_work(value TEXT); BEGIN IMMEDIATE; INSERT INTO host_work VALUES('pending');").unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&conn).is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(
        conn.query_row(
            "SELECT body FROM typed_tickets WHERE ticket_id='collision'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "Latest body"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM pragma_table_info('typed_ticket_events') WHERE name='event_ref'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        11
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM host_work", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    // Remove the deliberately conflicting test row, then retry the unchanged migration.
    conn.execute(
        "DELETE FROM typed_ticket_events WHERE event_index=?1",
        params![-1],
    )
    .unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn migration_preserves_published_refs_before_archiving_even_with_sparse_indices() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("identities.db");
    let conn = Connection::open(&path).unwrap();
    schema11(&conn);
    seed_legacy(&conn, "sparse");
    conn.execute_batch("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body) VALUES
        ('workspace','sparse',3,'create','Created'),
        ('workspace','sparse',12,'item_edit','Edited'),
        ('workspace','sparse',90,'comment','Existing comment'),
        ('workspace','sparse',99,'comment','Explicit identity');
        INSERT INTO typed_ticket_event_attributes VALUES('workspace','sparse',99,'event_id','published-custom-id');").unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    let refs: Vec<(i64, String)> = conn.prepare("SELECT event_index,event_ref FROM typed_ticket_events WHERE ticket_id='sparse' ORDER BY event_index").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    assert_eq!(
        refs,
        vec![
            (-1, "sparse:-1".into()),
            (3, "sparse:3".into()),
            (12, "sparse:12".into()),
            (90, "sparse:90".into()),
            (99, "sparse:99".into())
        ]
    );
    let backend = SqliteTicketBackend::open_verified(&path, "workspace").unwrap();
    let ticket = backend.show("sparse".into()).unwrap();
    assert_eq!(ticket.events[1].event_ref, "sparse:90");
    assert_eq!(ticket.events[1].attributes["event_id"], "sparse:90");
    assert_eq!(ticket.events[1].attributes["event_sequence"], "90");
    let edited = backend
        .edit_post(
            "sparse".into(),
            TicketPostEdit {
                event_ref: "sparse:90".into(),
                expected_body: MarkdownText::new("Existing comment"),
                body: MarkdownText::new("Updated comment"),
                author: "editor".into(),
            },
        )
        .unwrap();
    assert_eq!(edited.events[1].event_ref, "sparse:90");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(backend.show("sparse".into()).unwrap(), edited);
}

#[test]
fn migration_preserves_raw_identity_attributes_without_rebinding_published_refs() {
    for event_ids in [["same", "same"], ["sequence:1", ""], ["collision:-1", ""]] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("identity-attributes.db");
        let conn = Connection::open(&path).unwrap();
        schema11(&conn);
        seed_legacy(&conn, "collision");
        for (index, event_id) in event_ids.iter().enumerate() {
            conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,body) VALUES('workspace','collision',?1,'comment','retained')", [index as i64]).unwrap();
            if !event_id.is_empty() {
                conn.execute("INSERT INTO typed_ticket_event_attributes VALUES('workspace','collision',?1,'event_id',?2)", params![index as i64,event_id]).unwrap();
            }
        }
        migrate_sqlite_ticket_schema(&conn).unwrap();
        let backend = SqliteTicketBackend::open_verified(&path, "workspace").unwrap();
        let migrated = backend.show("collision".into()).unwrap();
        assert_eq!(
            migrated
                .events
                .iter()
                .map(|e| e.event_ref.as_str())
                .collect::<Vec<_>>(),
            ["collision:-1", "collision:0", "collision:1"]
        );
        for (index, event_id) in event_ids.iter().enumerate() {
            let event = &migrated.events[index + 1];
            assert_eq!(event.attributes["event_id"], format!("collision:{index}"));
            if !event_id.is_empty() {
                assert_eq!(conn.query_row("SELECT value FROM typed_ticket_event_attributes WHERE ticket_id='collision' AND event_index=?1 AND key='event_id'", [index as i64], |r| r.get::<_, String>(0)).unwrap(), *event_id);
            }
        }
        migrate_sqlite_ticket_schema(&conn).unwrap();
        assert_eq!(backend.show("collision".into()).unwrap(), migrated);
    }
}

#[test]
fn migration_recovers_only_last_known_body_edit_and_never_invents_creation_author() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("edit-metadata.db");
    let conn = Connection::open(&path).unwrap();
    schema11(&conn);
    for id in [
        "known",
        "unknown-last-editor",
        "title-only",
        "empty-creator",
    ] {
        seed_legacy(&conn, id);
    }
    conn.execute_batch("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,kind,author,at,body) VALUES
        ('workspace','known',0,'create','creator','original-time','Created'),
        ('workspace','known',1,'item_edit','old-editor','old-time','old body edit'),
        ('workspace','known',5,'item_edit','body-editor','body-time','latest body edit'),
        ('workspace','known',6,'item_edit','title-editor','title-time','later title edit'),
        ('workspace','unknown-last-editor',0,'item_edit','old-editor','old-time','old body edit'),
        ('workspace','unknown-last-editor',1,'item_edit',NULL,NULL,'last body edit'),
        ('workspace','title-only',0,'item_edit','title-editor','title-time','not a body edit'),
        ('workspace','empty-creator',0,'create','  ','create-time','Created');
        INSERT INTO typed_ticket_event_attributes VALUES
        ('workspace','known',1,'changes','body'),
        ('workspace','known',5,'changes','title,body,targets'),
        ('workspace','known',6,'changes','title'),
        ('workspace','unknown-last-editor',0,'changes','body'),
        ('workspace','unknown-last-editor',1,'changes','body'),
        ('workspace','title-only',0,'changes','title,body_format');
        INSERT INTO typed_ticket_raw_frontmatter VALUES('workspace','empty-creator','created_by','');").unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    let backend = SqliteTicketBackend::open_verified(&path, "workspace").unwrap();
    let known = backend.show("known".into()).unwrap();
    let post = &known.events[0];
    assert_eq!(post.author.as_deref(), Some("creator"));
    assert_eq!(post.at.as_deref(), Some("original-time"));
    assert_eq!(post.edited_at.as_deref(), Some("body-time"));
    assert_eq!(post.edited_by.as_deref(), Some("body-editor"));
    assert_eq!(known.document.body.as_str(), "Latest body");
    for id in ["unknown-last-editor", "title-only", "empty-creator"] {
        let ticket = backend.show(id.into()).unwrap();
        assert_eq!(ticket.events[0].edited_at, None, "{id}");
        assert_eq!(ticket.events[0].edited_by, None, "{id}");
        assert_eq!(ticket.events[0].author, None, "{id}");
        assert_eq!(ticket.meta.created_by, None, "{id}");
        let json = serde_json::to_value(ticket).unwrap();
        assert!(json["meta"].as_object().unwrap().contains_key("created_by"));
        assert!(json["meta"]["created_by"].is_null());
        assert!(json["events"][0]["author"].is_null());
    }
    migrate_sqlite_ticket_schema(&conn).unwrap();
    assert_eq!(backend.show("known".into()).unwrap(), known);
    // Blank new creation identities use the same honest nullable contract.
    let mut input = NewTicket::new("Unknown creator");
    input.author = Some(" ".into());
    let (_, created) = backend.create_with_snapshot(input).unwrap();
    assert_eq!(created.meta.created_by, None);
    assert_eq!(created.events[0].author, None);
}

#[test]
fn edit_post_dispatch_round_trips_atomic_ticket_snapshots() {
    use ticket::{
        TicketBackendOperation, TicketBackendOperationResult, execute_ticket_backend_operation,
    };
    let temp = TempDir::new().unwrap();
    let backend = SqliteTicketBackend::open(temp.path().join("dispatch.db"), "workspace").unwrap();
    let (reference, before) = backend
        .create_with_snapshot(NewTicket::new("Dispatch"))
        .unwrap();
    let request = TicketBackendOperation::EditPost {
        id: reference.id.clone().into(),
        edit: TicketPostEdit {
            event_ref: before.events[0].event_ref.clone(),
            expected_body: before.document.body.clone(),
            body: MarkdownText::new("Rescoped requirements"),
            author: "caller".into(),
        },
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "edit_post");
    let decoded = serde_json::from_value(json).unwrap();
    let result = execute_ticket_backend_operation(&backend, decoded).unwrap();
    let wire = serde_json::to_value(&result).unwrap();
    assert_eq!(wire["result"], "ticket_snapshots");
    assert_eq!(
        wire["value"]["previous"]["document"]["body"],
        before.document.body.as_str()
    );
    assert_eq!(
        wire["value"]["current"]["document"]["body"],
        "Rescoped requirements"
    );
    let TicketBackendOperationResult::TicketSnapshots { previous, current } =
        serde_json::from_value(wire).unwrap()
    else {
        panic!("wrong result variant")
    };
    assert_eq!(previous, before);
    assert_eq!(backend.show(reference.id.into()).unwrap(), current);
    assert_ne!(
        ticket_content_digest(&previous),
        ticket_content_digest(&current)
    );
    assert!(execute_ticket_backend_operation(&backend, request).is_err());
}
