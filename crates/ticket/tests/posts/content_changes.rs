use super::*;
use ticket::{TicketBodyReplacement, TicketTarget, TicketTargetAccess, TicketTargetsEdit};

struct Fixture {
    _temp: TempDir,
    path: std::path::PathBuf,
    backend: SqliteTicketBackend,
    conn: Connection,
    id: String,
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("facts.db");
        let backend = SqliteTicketBackend::open(&path, "workspace").unwrap();
        let mut input = NewTicket::new("Original title");
        input.body = MarkdownText::new("Private original body");
        input.author = Some("creator".into());
        let id = backend.create(input).unwrap().id;
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        Self {
            _temp: temp,
            path,
            backend,
            conn,
            id,
        }
    }

    fn ticket(&self) -> ticket::Ticket {
        self.backend.show(self.id.clone().into()).unwrap()
    }

    fn edit(&self, edit: TicketItemEdit) -> ticket::Ticket {
        self.backend
            .edit_item(self.id.clone().into(), edit)
            .unwrap()
    }

    fn facts(&self) -> Vec<Fact> {
        self.conn.prepare("SELECT change_id,workspace_id,ticket_id,kind,actor,at,title_changed,body_changed,targets_changed,source_event_ref FROM typed_ticket_content_changes ORDER BY rowid").unwrap()
            .query_map([], |r| Ok(Fact {
                id: r.get(0)?, workspace: r.get(1)?, ticket: r.get(2)?, kind: r.get(3)?, actor: r.get(4)?, at: r.get(5)?,
                title: r.get(6)?, body: r.get(7)?, targets: r.get(8)?, source: r.get(9)?,
            })).unwrap().collect::<rusqlite::Result<_>>().unwrap()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Fact {
    id: String,
    workspace: String,
    ticket: String,
    kind: String,
    actor: Option<String>,
    at: String,
    title: bool,
    body: bool,
    targets: bool,
    source: Option<String>,
}

fn targets(names: &[&str]) -> TicketTargetsEdit {
    TicketTargetsEdit::Set {
        targets: names
            .iter()
            .map(|name| TicketTarget {
                repository_key: (*name).into(),
                ref_selector: None,
                access: TicketTargetAccess::ReadWrite,
            })
            .collect(),
    }
}

#[test]
fn item_edits_record_actual_fields_and_contemporaneous_actor_once_without_text() {
    let f = Fixture::new();
    assert!(f.facts().is_empty());
    let original = f.ticket();
    let edits = [
        (
            TicketItemEdit {
                title: Some("Changed title".into()),
                ..Default::default()
            },
            (true, false, false),
        ),
        (
            TicketItemEdit {
                targets: Some(targets(&["main", "other"])),
                ..Default::default()
            },
            (false, false, true),
        ),
        (
            TicketItemEdit {
                targets: Some(targets(&["other", "main"])),
                ..Default::default()
            },
            (false, false, true),
        ),
        (
            TicketItemEdit {
                title: Some("Combined title".into()),
                body: Some(MarkdownText::new("Private replacement body")),
                targets: Some(TicketTargetsEdit::Clear),
                ..Default::default()
            },
            (true, true, true),
        ),
        (
            TicketItemEdit {
                title: Some("Combined title".into()),
                body_replacement: Some(TicketBodyReplacement {
                    old_string: "replacement".into(),
                    new_string: "updated".into(),
                    replace_all: false,
                }),
                ..Default::default()
            },
            (false, true, false),
        ),
    ];
    for (index, (mut edit, flags)) in edits.into_iter().enumerate() {
        let actor = format!("editor-{index}");
        edit.author = Some(actor.clone());
        let current = f.edit(edit);
        let facts = f.facts();
        assert_eq!(facts.len(), index + 1);
        let fact = facts.last().unwrap();
        assert_eq!(fact.workspace, "workspace");
        assert_eq!(fact.ticket, f.id);
        assert_eq!(fact.kind, "content_edit");
        assert_eq!(fact.actor.as_deref(), Some(actor.as_str()));
        assert_eq!((fact.title, fact.body, fact.targets), flags);
        assert_eq!(
            fact.source.as_deref(),
            flags.1.then_some(original.events[0].event_ref.as_str())
        );
        assert_eq!(Some(fact.at.as_str()), current.meta.updated_at.as_deref());
        assert_eq!(current.events[0].author, original.events[0].author);
    }
    let facts = f.facts();
    assert_eq!(
        facts
            .iter()
            .map(|f| &f.id)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        facts.len()
    );
    // The schema itself excludes content snapshots, not just this particular INSERT.
    let columns: Vec<String> = f
        .conn
        .prepare("SELECT name FROM pragma_table_info('typed_ticket_content_changes')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        columns,
        [
            "change_id",
            "workspace_id",
            "ticket_id",
            "kind",
            "actor",
            "at",
            "title_changed",
            "body_changed",
            "targets_changed",
            "source_event_ref"
        ]
    );
    let reopened = SqliteTicketBackend::open(&f.path, "workspace").unwrap();
    assert_eq!(reopened.show(f.id.clone().into()).unwrap(), f.ticket());
    assert_eq!(f.facts(), facts);
    f.edit(TicketItemEdit {
        title: Some("Unknown local editor".into()),
        ..Default::default()
    });
    assert_eq!(f.facts().last().unwrap().actor, None);
}

#[test]
fn post_edits_distinguish_initial_and_discussion_and_preserve_prior_edit_actors() {
    let f = Fixture::new();
    f.backend
        .add_event(
            f.id.clone().into(),
            comment("Private discussion", "commenter"),
        )
        .unwrap();
    assert!(f.facts().is_empty());
    let before = f.ticket();
    for (index, original) in before.events.iter().enumerate() {
        let current = f
            .backend
            .edit_post(
                f.id.clone().into(),
                TicketPostEdit {
                    event_ref: original.event_ref.clone(),
                    expected_body: original.body.clone(),
                    body: MarkdownText::new(format!("Private edited post {index}")),
                    author: format!("post-editor-{index}"),
                },
            )
            .unwrap();
        let facts = f.facts();
        assert_eq!(facts.len(), index + 1);
        let fact = facts.last().unwrap();
        assert_eq!(
            fact.kind,
            if original.is_initial {
                "content_edit"
            } else {
                "comment_edit"
            }
        );
        assert_eq!((fact.title, fact.body, fact.targets), (false, true, false));
        assert_eq!(fact.source.as_ref(), Some(&original.event_ref));
        assert_eq!(
            fact.actor.as_deref(),
            Some(format!("post-editor-{index}").as_str())
        );
        assert_eq!(
            current.events[index].edited_at.as_deref(),
            Some(fact.at.as_str())
        );
    }
    let facts = f.facts();
    let post = &f.ticket().events[0];
    f.backend
        .edit_post(
            f.id.clone().into(),
            TicketPostEdit {
                event_ref: post.event_ref.clone(),
                expected_body: post.body.clone(),
                body: MarkdownText::new("Another private edit"),
                author: "later-editor".into(),
            },
        )
        .unwrap();
    assert_eq!(&f.facts()[..2], facts.as_slice());
}

#[test]
fn noops_preserve_metadata_and_do_not_bypass_validation_or_emit_facts() {
    let f = Fixture::new();
    f.edit(TicketItemEdit {
        targets: Some(targets(&["main"])),
        author: Some("first".into()),
        ..Default::default()
    });
    let before = f.ticket();
    let facts = f.facts();
    for edit in [
        TicketItemEdit {
            title: Some(before.meta.title.clone()),
            ..Default::default()
        },
        TicketItemEdit {
            body: Some(before.document.body.clone()),
            ..Default::default()
        },
        TicketItemEdit {
            targets: Some(targets(&["main"])),
            ..Default::default()
        },
        TicketItemEdit {
            title: Some(before.meta.title.clone()),
            body: Some(before.document.body.clone()),
            targets: Some(targets(&["main"])),
            ..Default::default()
        },
    ] {
        assert_eq!(
            f.edit(TicketItemEdit {
                author: Some("noop-editor".into()),
                ..edit
            }),
            before
        );
    }
    let post = &before.events[0];
    let edit = TicketPostEdit {
        event_ref: post.event_ref.clone(),
        expected_body: post.body.clone(),
        body: post.body.clone(),
        author: "noop-editor".into(),
    };
    assert_eq!(
        f.backend
            .edit_post(f.id.clone().into(), edit.clone())
            .unwrap(),
        before
    );
    assert!(
        f.backend
            .edit_post(
                f.id.clone().into(),
                TicketPostEdit {
                    expected_body: MarkdownText::new("stale"),
                    ..edit
                }
            )
            .is_err()
    );
    assert!(
        f.backend
            .edit_item(
                f.id.clone().into(),
                TicketItemEdit {
                    title: Some(before.meta.title.clone()),
                    author: Some(" ".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    assert_eq!(f.facts(), facts);
    assert_eq!(f.ticket(), before);
    f.edit(TicketItemEdit {
        targets: Some(TicketTargetsEdit::Clear),
        ..Default::default()
    });
    let empty = f.ticket();
    let facts = f.facts();
    assert_eq!(
        f.edit(TicketItemEdit {
            targets: Some(TicketTargetsEdit::Clear),
            ..Default::default()
        }),
        empty
    );
    assert_eq!(f.facts(), facts);
}

#[test]
fn failed_host_capture_rolls_back_edits_fact_and_trigger_writes_together() {
    let f = Fixture::new();
    f.conn.execute_batch("CREATE TABLE host_capture(change_id TEXT, actor TEXT); CREATE TRIGGER capture_fact AFTER INSERT ON typed_ticket_content_changes BEGIN INSERT INTO host_capture VALUES(NEW.change_id, NEW.actor); SELECT RAISE(ABORT, 'capture failed'); END;").unwrap();
    let before = f.ticket();
    assert!(
        f.backend
            .edit_item(
                f.id.clone().into(),
                TicketItemEdit {
                    title: Some("Changed title".into()),
                    body: Some(MarkdownText::new("Changed body")),
                    targets: Some(targets(&["main"])),
                    author: Some("item-editor".into()),
                    ..Default::default()
                }
            )
            .is_err()
    );
    let post = &before.events[0];
    assert!(
        f.backend
            .edit_post(
                f.id.clone().into(),
                TicketPostEdit {
                    event_ref: post.event_ref.clone(),
                    expected_body: post.body.clone(),
                    body: MarkdownText::new("Changed post"),
                    author: "post-editor".into(),
                }
            )
            .is_err()
    );
    assert_eq!(f.ticket(), before);
    assert!(f.facts().is_empty());
    assert_eq!(
        f.conn
            .query_row("SELECT count(*) FROM host_capture", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    f.conn.execute_batch("DROP TRIGGER capture_fact; CREATE TRIGGER capture_fact AFTER INSERT ON typed_ticket_content_changes BEGIN INSERT INTO host_capture VALUES(NEW.change_id, NEW.actor); END;").unwrap();
    f.edit(TicketItemEdit {
        title: Some("Successful title".into()),
        author: Some("actual-editor".into()),
        ..Default::default()
    });
    let fact = f.facts().remove(0);
    assert_eq!(
        f.conn
            .query_row("SELECT change_id, actor FROM host_capture", [], |r| Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?
            )))
            .unwrap(),
        (fact.id, "actual-editor".into())
    );
}

fn schema12(conn: &Connection) {
    super::schema11(conn);
    super::seed_legacy(conn, "retained");
    conn.execute_batch(include_str!("../../src/post_migration.sql"))
        .unwrap();
    conn.execute_batch("INSERT INTO ticket_schema_migrations VALUES(12,'Canonical Ticket posts','original-post-marker');").unwrap();
}

fn schema(conn: &Connection) -> Vec<(String, String, String)> {
    conn.prepare("SELECT type,name,sql FROM sqlite_master WHERE sql IS NOT NULL ORDER BY type,name")
        .unwrap()
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get::<_, String>(2)?
                    .split_whitespace()
                    .collect::<String>(),
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn content_change_upgrade_matches_fresh_schema_without_backfill_and_preserves_markers() {
    let upgraded = Connection::open_in_memory().unwrap();
    upgraded
        .execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);")
        .unwrap();
    schema12(&upgraded);
    migrate_sqlite_ticket_schema(&upgraded).unwrap();
    let fresh = Connection::open_in_memory().unwrap();
    fresh
        .execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY);")
        .unwrap();
    migrate_sqlite_ticket_schema(&fresh).unwrap();
    let upgraded_schema = schema(&upgraded);
    let fresh_schema = schema(&fresh);
    assert_eq!(upgraded_schema.len(), fresh_schema.len());
    for (upgraded_object, fresh_object) in upgraded_schema.iter().zip(&fresh_schema) {
        assert_eq!(upgraded_object, fresh_object);
    }
    assert_eq!(
        upgraded
            .query_row(
                "SELECT count(*) FROM typed_ticket_content_changes",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        upgraded
            .query_row(
                "SELECT body FROM typed_ticket_events WHERE event_index=-1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "Latest body"
    );
    assert_eq!(
        upgraded
            .query_row(
                "SELECT applied_at FROM ticket_schema_migrations WHERE version=12",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "original-post-marker"
    );
    let markers: Vec<(i64, String, String)> = upgraded
        .prepare("SELECT version,name,applied_at FROM ticket_schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    migrate_sqlite_ticket_schema(&upgraded).unwrap();
    let after: Vec<(i64, String, String)> = upgraded
        .prepare("SELECT version,name,applied_at FROM ticket_schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(markers, after);
}

#[test]
fn failed_content_change_migration_rolls_back_ddl_and_marker_without_committing_host() {
    let conn = Connection::open_in_memory().unwrap();
    schema12(&conn);
    conn.execute_batch("CREATE TABLE host_work(value TEXT); CREATE TRIGGER typed_ticket_content_changes_no_update BEFORE UPDATE ON host_work BEGIN SELECT RAISE(ABORT, 'collision'); END;").unwrap();
    let before = schema(&conn);
    conn.execute_batch("BEGIN IMMEDIATE; INSERT INTO host_work VALUES('pending');")
        .unwrap();
    assert!(migrate_sqlite_ticket_schema_in_transaction(&conn).is_err());
    assert!(!conn.is_autocommit());
    assert_eq!(schema(&conn), before);
    assert_eq!(
        conn.query_row(
            "SELECT max(version) FROM ticket_schema_migrations",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        12
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM host_work", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    conn.execute_batch("ROLLBACK; DROP TRIGGER typed_ticket_content_changes_no_update;")
        .unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    verify_sqlite_ticket_schema(&conn).unwrap();
}

#[test]
fn fact_constraints_reject_invalid_shapes_updates_and_cross_workspace_parent() {
    let f = Fixture::new();
    let insert = |id: &str,
                  workspace: &str,
                  kind: &str,
                  title: i64,
                  body: i64,
                  targets: i64,
                  source: Option<&str>| {
        f.conn.execute("INSERT INTO typed_ticket_content_changes VALUES(?1,?2,?3,?4,'actor','time',?5,?6,?7,?8)", params![id, workspace, f.id, kind, title, body, targets, source])
    };
    for (kind, title, body, targets, source) in [
        ("unknown", 1, 0, 0, None),
        ("content_edit", 0, 0, 0, None),
        ("content_edit", 2, 0, 0, None),
        ("content_edit", 0, 2, 0, Some("post")),
        ("content_edit", 0, 0, 2, None),
        ("content_edit", 1, 0, 0, Some("post")),
        ("content_edit", 0, 1, 0, None),
        ("content_edit", 0, 1, 0, Some(" ")),
        ("comment_edit", 1, 1, 0, Some("post")),
        ("comment_edit", 0, 1, 1, Some("post")),
    ] {
        assert!(insert("invalid", "workspace", kind, title, body, targets, source).is_err());
    }
    assert!(insert("foreign", "other-workspace", "content_edit", 1, 0, 0, None).is_err());
    insert("valid", "workspace", "content_edit", 1, 0, 0, None).unwrap();
    assert!(insert("valid", "workspace", "content_edit", 1, 0, 0, None).is_err());
    assert!(
        f.conn
            .execute(
                "UPDATE typed_ticket_content_changes SET actor='changed'",
                []
            )
            .is_err()
    );
    f.conn
        .execute("DELETE FROM typed_ticket_content_changes", [])
        .unwrap();
    assert!(f.facts().is_empty());
}

#[test]
fn verifier_rejects_weakened_fact_constraints_trigger_and_migration_history() {
    for (from, to) in [
        ("CHECK (kind IN ('content_edit', 'comment_edit'))", ""),
        ("CHECK (title_changed IN (0, 1))", ""),
        ("CHECK (body_changed IN (0, 1))", ""),
        ("CHECK (targets_changed IN (0, 1))", ""),
        (
            "CHECK (title_changed + body_changed + targets_changed > 0),",
            "",
        ),
        (
            "REFERENCES typed_tickets(workspace_id, ticket_id)",
            "REFERENCES typed_tickets(ticket_id, workspace_id)",
        ),
        ("ON DELETE CASCADE", "ON DELETE RESTRICT"),
        (
            "SELECT RAISE(ABORT, 'Ticket content change facts are immutable')",
            "SELECT 1",
        ),
    ] {
        let f = Fixture::new();
        f.conn
            .execute_batch("DROP TABLE typed_ticket_content_changes")
            .unwrap();
        let sql = include_str!("../../src/content_changes.sql").replace(from, to);
        f.conn.execute_batch(&sql).unwrap();
        assert!(verify_sqlite_ticket_schema(&f.conn).is_err(), "{from}");
        assert!(migrate_sqlite_ticket_schema(&f.conn).is_err(), "{from}");
    }
    for mutation in [
        "UPDATE ticket_schema_migrations SET name='unrecognized' WHERE version=14",
        "INSERT INTO ticket_schema_migrations VALUES(12,'Canonical Ticket posts','invalid extra history')",
        "UPDATE ticket_schema_migrations SET version=15 WHERE version=14",
    ] {
        let f = Fixture::new();
        f.conn.execute_batch(mutation).unwrap();
        assert!(verify_sqlite_ticket_schema(&f.conn).is_err());
        assert!(migrate_sqlite_ticket_schema(&f.conn).is_err());
    }
}
