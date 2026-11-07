use super::*;
use crate::*;
use rusqlite::types::Value;
use std::collections::BTreeMap;

fn old_database(conn: &Connection, count: usize) {
    conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE")
        .unwrap();
    migrate_historical_sqlite_ticket_schema_in_transaction(
        conn,
        HistoricalTicketSchemaTarget::ParentLinks16,
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    for workspace in ["w", "other"] {
        for ticket in ["t", "dependency"] {
            conn.execute("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit,created_by) VALUES (?1,?2,?2,'Original title','open','task','P2','open',1,'original-author')",params![workspace,ticket]).unwrap();
            conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body,author,at) VALUES (?1,?2,-1,?2||':-1','comment','Original requirements and decisions','original-author','original-time')",params![workspace,ticket]).unwrap();
        }
        conn.execute("INSERT INTO typed_ticket_events(workspace_id,ticket_id,event_index,event_ref,kind,body,author,at) VALUES (?1,'t',7,'t:7','comment','Original discussion','comment-author','comment-time')",[workspace]).unwrap();
        conn.execute(
            "INSERT INTO typed_ticket_event_attributes VALUES (?1,'t',7,'context','retained')",
            [workspace],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO typed_ticket_event_references VALUES (?1,'t',7,0,'ticket','dependency')",
            [workspace],
        )
        .unwrap();
        conn.execute("INSERT INTO typed_ticket_relations(workspace_id,ticket_id,kind,target,note,author,at) VALUES (?1,'t','depends_on','dependency','original dependency','author','time')",[workspace]).unwrap();
    }
    conn.execute_batch("BEGIN IMMEDIATE;
        INSERT INTO ticket_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,assigned_by,assigned_at)
          VALUES ('w','t','assignment','orchestrator','subject','subject-a','author','assigned-time');
        INSERT INTO ticket_current_worker_assignments(workspace_id,ticket_id,assignment_id,role,principal_kind,principal_id,updated_at)
          VALUES ('w','t','assignment','orchestrator','subject','subject-a','assigned-time');
        INSERT INTO ticket_worker_assignment_events(workspace_id,ticket_id,role,event_id,action,assignment_id,actor,created_at,operation_id)
          VALUES ('w','t','orchestrator','assigned-event','assigned','assignment','author','assigned-time','assign-key');
        COMMIT;").unwrap();
    let answer = TicketAssignmentAnswer {
        assignment_id: "assignment".into(),
        expected_last_event_id: "assigned-event".into(),
        operation_key: "answer-key".into(),
        body: "Existing continuing input".into(),
    };
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    sqlite_answer_ticket_assignment_in_transaction(
        conn,
        "w",
        "t",
        &answer,
        &serde_json::json!({"kind":"account","account_id":"human","user_id":"human"}),
        "human",
    )
    .unwrap();
    conn.execute_batch("COMMIT").unwrap();
    for index in 0..count {
        let workspace = if index % 2 == 0 { "w" } else { "other" };
        let kind = [
            "before",
            "after",
            "blocked_by",
            "blocks",
            "conflicts_with",
            "do_not_parallelize",
            "waiting_capacity_note",
            "accepted_plan",
            "unknown-retained-kind",
        ][index % 9];
        conn.execute("INSERT INTO typed_ticket_orchestration_plans VALUES (?1,'t',?2,?3,?4,?5,?6,?7,?8,?9,'old-author','old-time')",params![workspace,format!("old-{index:04}"),kind,Some("dependency"),Some("固有決定\n``` and Unicode 🐇"),if index%2==0{Some("unique requirement")}else{None},Some("work/old"),Some("old workdir"),Some("Never treat as active approval")]).unwrap();
    }
}

fn rows(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .unwrap();
    let columns = statement.column_count();
    statement
        .query_map([], |row| (0..columns).map(|i| row.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn unaffected(conn: &Connection) -> BTreeMap<String, Vec<Vec<Value>>> {
    let mut statement = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT IN ('ticket_schema_migrations','typed_ticket_orchestration_plans','typed_ticket_events') ORDER BY name").unwrap();
    let names = statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let values = rows(conn, &name);
            (name, values)
        })
        .collect()
}

fn no_plan_storage(conn: &Connection) {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='typed_ticket_orchestration_plans')",[],|r|r.get(0)).unwrap();
    assert!(!exists);
    verify_sqlite_ticket_schema(conn).unwrap();
}

#[test]
fn retirement_preserves_every_row_across_workspaces_without_turning_memos_into_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.db");
    let conn = Connection::open(&path).unwrap();
    // Larger than the former public query limit, includes all kinds and an unknown kind.
    old_database(&conn, 535);
    let original_rows = rows(&conn, "typed_ticket_orchestration_plans");
    let original_events = rows(&conn, "typed_ticket_events");
    let before = unaffected(&conn);
    let history = rows(&conn, "ticket_schema_migrations");
    migrate_sqlite_ticket_schema(&conn).unwrap();
    no_plan_storage(&conn);
    assert_eq!(unaffected(&conn), before);
    assert_eq!(
        &rows(&conn, "ticket_schema_migrations")[..history.len()],
        history
    );
    let events = rows(&conn, "typed_ticket_events");
    assert_eq!(&events[..original_events.len()], original_events);
    assert_eq!(events.len(), original_events.len() + 535);
    let mut statement=conn.prepare("SELECT workspace_id,ticket_id,body,author,at FROM typed_ticket_events WHERE heading='Historical memo from retired storage'").unwrap();
    let mut migrated = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .unwrap()
        .map(|row| {
            let (workspace, ticket, body, author, at) = row.unwrap();
            assert!(body.contains("not a current assignment, approval"));
            assert_eq!(author, "migration:ticket-plan-retirement");
            assert_eq!(at, "old-time");
            let json = body
                .split_once("```json\n")
                .unwrap()
                .1
                .strip_suffix("\n```")
                .unwrap();
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            assert_eq!(value["workspace_id"], workspace);
            assert_eq!(value["ticket_id"], ticket);
            (value["record_id"].as_str().unwrap().to_string(), value)
        })
        .collect::<BTreeMap<_, _>>();
    for row in original_rows {
        let fields = [
            "workspace_id",
            "ticket_id",
            "record_id",
            "kind",
            "related_ticket",
            "note",
            "accepted_summary",
            "accepted_branch",
            "accepted_worktree",
            "accepted_role_plan",
            "author",
            "at",
        ];
        let expected = fields
            .into_iter()
            .zip(row)
            .map(|(key, value)| {
                (
                    key.into(),
                    match value {
                        Value::Null => serde_json::Value::Null,
                        Value::Text(s) => serde_json::Value::String(s),
                        _ => panic!("unexpected fixture type"),
                    },
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let id = expected["record_id"].as_str().unwrap();
        assert_eq!(
            migrated.remove(id).unwrap(),
            serde_json::Value::Object(expected)
        );
    }
    assert!(migrated.is_empty());
    drop(statement);
    drop(conn);
    let reopened = Connection::open(path).unwrap();
    migrate_sqlite_ticket_schema(&reopened).unwrap();
    no_plan_storage(&reopened);
    assert_eq!(rows(&reopened, "typed_ticket_events"), events);
    assert_eq!(unaffected(&reopened), before);
}

#[test]
fn empty_old_database_and_fresh_database_retire_storage_and_reopen_without_extra_posts() {
    for old in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tickets.db");
        let conn = Connection::open(&path).unwrap();
        if old {
            old_database(&conn, 0);
        }
        let original = if old {
            rows(&conn, "typed_ticket_events")
        } else {
            Vec::new()
        };
        migrate_sqlite_ticket_schema(&conn).unwrap();
        no_plan_storage(&conn);
        assert_eq!(rows(&conn, "typed_ticket_events"), original);
        drop(conn);
        let conn = Connection::open(path).unwrap();
        migrate_sqlite_ticket_schema(&conn).unwrap();
        no_plan_storage(&conn);
        assert_eq!(rows(&conn, "typed_ticket_events"), original);
    }
}

#[test]
fn failed_preservation_rolls_back_all_comments_table_drop_and_receipt_then_can_retry() {
    let conn = Connection::open_in_memory().unwrap();
    old_database(&conn, 3);
    let plans = rows(&conn, "typed_ticket_orchestration_plans");
    let events = rows(&conn, "typed_ticket_events");
    let history = rows(&conn, "ticket_schema_migrations");
    conn.execute_batch("CREATE TRIGGER reject_second_preserved_memo BEFORE INSERT ON typed_ticket_events WHEN NEW.heading='Historical memo from retired storage' AND EXISTS(SELECT 1 FROM typed_ticket_events WHERE heading='Historical memo from retired storage') BEGIN SELECT RAISE(ABORT,'injected preservation failure'); END;").unwrap();
    assert!(
        migrate_sqlite_ticket_schema(&conn)
            .unwrap_err()
            .to_string()
            .contains("injected preservation failure")
    );
    assert_eq!(rows(&conn, "typed_ticket_orchestration_plans"), plans);
    assert_eq!(rows(&conn, "typed_ticket_events"), events);
    assert_eq!(rows(&conn, "ticket_schema_migrations"), history);
    conn.execute_batch("DROP TRIGGER reject_second_preserved_memo")
        .unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    no_plan_storage(&conn);
    assert_eq!(rows(&conn, "typed_ticket_events").len(), events.len() + 3);
}

#[test]
fn retirement_keeps_maximum_legacy_text_and_json_escaping_without_truncation() {
    let conn = Connection::open_in_memory().unwrap();
    old_database(&conn, 1);
    let note = "line\n".repeat(3276);
    let summary = "\u{1}".repeat(16 * 1024);
    let roles = "\\\"".repeat(8 * 1024);
    conn.execute(
        "UPDATE typed_ticket_orchestration_plans SET note=?1, accepted_summary=?2, accepted_role_plan=?3",
        params![note, summary, roles],
    ).unwrap();
    migrate_sqlite_ticket_schema(&conn).unwrap();
    let body: String = conn.query_row(
        "SELECT body FROM typed_ticket_events WHERE heading='Historical memo from retired storage'",
        [], |r| r.get(0),
    ).unwrap();
    // Fits the existing public full-post read, not just the bounded thread preview.
    assert!(body.len() > 64 * 1024);
    assert!(body.len() < 1024 * 1024);
    let encoded = body
        .split_once("```json\n")
        .unwrap()
        .1
        .strip_suffix("\n```")
        .unwrap();
    let original: serde_json::Value = serde_json::from_str(encoded).unwrap();
    assert_eq!(original["note"], note);
    assert_eq!(original["accepted_summary"], summary);
    assert_eq!(original["accepted_role_plan"], roles);
    no_plan_storage(&conn);
}
