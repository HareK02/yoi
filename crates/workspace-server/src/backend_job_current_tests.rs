//! Synthetic retained databases only. No live Runtime or Feature store is opened.
use super::*;
use crate::store;
use rusqlite::{params, types::Value};
use std::path::Path;

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = conn.prepare(sql).unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |row| {
            (0..count)
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn advance(conn: &Connection, target: i64) {
    let current = store::current_schema_version(conn).unwrap();
    for migration in store::MIGRATIONS
        .iter()
        .filter(|m| m.version > current && m.version <= target)
    {
        (migration.apply)(conn).unwrap();
    }
    assert_eq!(store::current_schema_version(conn).unwrap(), target);
}

fn workspace(conn: &Connection) {
    conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('owner','user','owner','Owner','1','1');
        INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('w','owner','Workspace','active','1','1'),('other','owner','Other','active','1','1');").unwrap();
}

fn job(conn: &Connection, workspace: &str, id: &str, state: &str) {
    job_with_request(conn, workspace, id, state, |_| {});
}

fn job_with_request(
    conn: &Connection,
    workspace: &str,
    id: &str,
    state: &str,
    change: impl FnOnce(&mut serde_json::Value),
) {
    let legacy = store::column_exists(conn, "backend_jobs", "input_revision").unwrap();
    let input_column = if legacy {
        "input_revision"
    } else {
        "input_digest"
    };
    let mut request = serde_json::json!({
        "job_id":id,"purpose":"fixture","input_ref":"saved://input",
        "input":{"body":"retained input"},"instruction":"use saved input",
        "profile":"builtin:backend-job","limits":{
            "max_concurrent_jobs":1,"timeout_seconds":60,"max_result_bytes":4096,"max_attempts":3
        }
    });
    if legacy {
        request["input_revision"] = "old-token".into();
    }
    change(&mut request);
    conn.execute(&format!("INSERT INTO backend_jobs(workspace_id,job_id,purpose,{input_column},input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,created_at,updated_at,completed_at)
        VALUES(?1,?2,'fixture','input-digest','saved://input',?3,'intent',?4,1,?5,?6,'1','2',?7)"),
        params![workspace,id,request.to_string(),state,
            (state == "completed").then_some("{\"saved\":true}"),
            (state == "completed").then_some("result-digest"),
            (state != "pending").then_some("2")]).unwrap();
    let attempt_state = if state == "pending" {
        "reserved"
    } else {
        state
    };
    conn.execute(&format!("INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,{input_column},state,deadline_at,result_json,result_digest,created_at,updated_at,completed_at)
        VALUES(?1,?2,?2,1,'input-digest',?3,'deadline',?4,?5,'1','2',?6)"),
        params![workspace,id,attempt_state,(state == "completed").then_some("{\"saved\":true}"),
            (state == "completed").then_some("result-digest"),(state != "pending").then_some("2")]).unwrap();
}

fn notification(conn: &Connection, workspace: &str, job: &str, routed: bool) {
    let id = format!("arbitrary-notification:{job}");
    let facts = serde_json::json!({"backend_job":{"job_id":job,"attempt_id":job,"delivery_id":id},"result_digest":"result-digest"});
    conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES(?1,NULL,?2,'backend_job','','backend_job_result',?3)",params![workspace,id,facts.to_string()]).unwrap();
    let source = conn.last_insert_rowid();
    conn.execute("INSERT INTO notification_source_preparation_failures(source_sequence,code) VALUES(?1,'source_lookup_failed')",[source]).unwrap();
    conn.execute(
        "INSERT INTO notification_source_offers VALUES(?1,1)",
        [source],
    )
    .unwrap();
    if !routed {
        return;
    }
    conn.execute("INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) VALUES(?1,?2,?3,'deliver','result','backend_job')",params![source,workspace,id]).unwrap();
    conn.execute(
        "INSERT INTO notification_requests VALUES(?1,?2,'immutable notification','{}')",
        params![workspace, id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notification_job_targets VALUES(?1,?2,'{}')",
        params![workspace, id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notification_acknowledgements VALUES(?1,?2,'{}')",
        params![workspace, id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO notification_failures(workspace_id,delivery_id,code) VALUES(?1,?2,'unknown')",
        params![workspace, id],
    )
    .unwrap();
}

fn legacy_delivery(conn: &Connection, workspace: &str, job: &str) {
    let id = format!("arbitrary-notification:{job}");
    conn.execute("INSERT INTO backend_job_deliveries(workspace_id,delivery_id,job_id,attempt_id,target_runtime_id,target_worker_id,state,created_at,updated_at) VALUES(?1,?2,?3,?3,'runtime','worker','unknown','1','2')",params![workspace,id,job]).unwrap();
}

fn fixture(path: &Path) -> Connection {
    store::tests::prepare_retained_schema(path, 122);
    let conn = Connection::open(path).unwrap();
    store::configure_sqlite(&conn).unwrap();
    workspace(&conn);
    job(&conn, "w", "old", "completed");
    job(&conn, "w", "unrouted", "completed");
    job(&conn, "w", "unfinished", "pending");
    notification(&conn, "w", "old", true);
    legacy_delivery(&conn, "w", "old");
    conn.execute("INSERT INTO notification_job_legacy_receipts VALUES('w','arbitrary-notification:old','{}',1)",[]).unwrap();
    notification(&conn, "w", "unrouted", false);
    advance(&conn, 129);
    job(&conn, "w", "current", "completed");
    job(&conn, "w", "pending", "pending");
    job(&conn, "other", "old", "completed");
    job(&conn, "w", "invalidated", "completed");
    notification(&conn, "w", "current", true);
    notification(&conn, "other", "old", true);
    notification(&conn, "w", "invalidated", true);
    conn.execute_batch("UPDATE backend_jobs SET request_json=json_set(request_json,'$.input',json('null')),input_disposition='invalidated' WHERE workspace_id='w' AND job_id='invalidated';
        INSERT INTO backend_job_execution_fences VALUES('w','old','old','{}','{}','{}','{}','fence-time');
        INSERT INTO backend_job_recovery_probes VALUES('w','old','old',1);
        INSERT INTO backend_job_reconciliation VALUES('w','pending','pending','cause','detail','inspect','now');
        INSERT INTO workspace_memory_documents VALUES('w','# Retained Memory output','created','updated');
        INSERT INTO subject_deletions(workspace_id,subject_id,operation_id) VALUES('w','deleted-subject','deletion-operation');
        INSERT INTO subject_deletion_jobs VALUES('w','deleted-subject','old');
        INSERT INTO subject_deletion_allocations VALUES('w','deleted-subject','backend-job:old');").unwrap();
    store::storage_formats::verify_history(&conn, 129).unwrap();
    conn
}

fn dump(conn: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    let tables = rows(
        conn,
        "SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name",
    );
    tables
        .into_iter()
        .map(|entry| {
            let Value::Text(table) = &entry[0] else {
                panic!("table name")
            };
            let sql = format!(
                "SELECT * FROM \"{}\" ORDER BY rowid",
                table.replace('"', "\"\"")
            );
            // Some shared component tables are WITHOUT ROWID; sort debug rows instead.
            let sql = if conn.prepare(&sql).is_ok() {
                sql
            } else {
                format!("SELECT * FROM \"{}\"", table.replace('"', "\"\""))
            };
            (table.clone(), rows(conn, &sql))
        })
        .collect()
}

fn job_graph(conn: &Connection) -> Vec<(String, Vec<Vec<Value>>)> {
    dump(conn)
        .into_iter()
        .filter(|(table, _)| {
            (table.starts_with("backend_job")
                && !table.starts_with("backend_job_reference_migration_"))
                || table.starts_with("notification_")
                || table.starts_with("subject_deletion")
                || table == "workspace_memory_documents"
        })
        .collect()
}

fn schema(conn: &Connection) -> Vec<Vec<Value>> {
    rows(
        conn,
        "SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name",
    )
}

fn notification_guards(conn: &Connection) -> Vec<Vec<Value>> {
    rows(
        conn,
        "SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND (name LIKE 'notification_%' OR name LIKE 'subject_deleted_notification_%') AND name NOT LIKE 'notification_job_legacy_%' ORDER BY name",
    )
}

#[test]
fn boundary_removes_only_obsolete_job_graph_and_preserves_content_and_current_rows() {
    let dir = tempfile::tempdir().unwrap();
    let conn = fixture(&dir.path().join("server.sqlite"));
    let current = rows(
        &conn,
        "SELECT * FROM backend_jobs WHERE job_id IN ('current','pending') OR workspace_id='other' ORDER BY workspace_id,job_id",
    );
    let current_attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts WHERE job_id IN ('current','pending') OR workspace_id='other' ORDER BY workspace_id,job_id",
    );
    let content = rows(&conn, "SELECT * FROM workspace_memory_documents");
    let fences = rows(&conn, "SELECT * FROM subject_deletion_jobs");
    let allocation_fences = rows(&conn, "SELECT * FROM subject_deletion_allocations");
    let guards = notification_guards(&conn);
    let notification_rows: Vec<_> = ["notification_sources","notification_decisions","notification_requests","notification_acknowledgements","notification_job_targets","notification_failures"].into_iter().map(|table| {
        let predicate = if table == "notification_sources" { "input_event_id" } else { "delivery_id" };
        (table, rows(&conn,&format!("SELECT * FROM {table} WHERE workspace_id='other' OR {predicate}='arbitrary-notification:current' ORDER BY 1,2")))
    }).collect();
    migrate(&conn).unwrap();
    verify(&conn).unwrap();
    assert_eq!(store::current_schema_version(&conn).unwrap(), 130);
    assert_eq!(
        rows(
            &conn,
            "SELECT workspace_id,job_id FROM backend_job_reference_migration_jobs ORDER BY 1,2"
        ),
        ["invalidated", "old", "unfinished", "unrouted"]
            .into_iter()
            .map(|job| vec![Value::Text("w".into()), Value::Text(job.into())])
            .collect::<Vec<_>>()
    );
    assert_eq!(
        current,
        rows(
            &conn,
            "SELECT * FROM backend_jobs ORDER BY workspace_id,job_id"
        )
    );
    assert_eq!(
        current_attempts,
        rows(
            &conn,
            "SELECT * FROM backend_job_attempts ORDER BY workspace_id,job_id"
        )
    );
    assert_eq!(
        content,
        rows(&conn, "SELECT * FROM workspace_memory_documents")
    );
    assert_eq!(fences, rows(&conn, "SELECT * FROM subject_deletion_jobs"));
    assert_eq!(
        allocation_fences,
        rows(&conn, "SELECT * FROM subject_deletion_allocations")
    );
    assert_eq!(guards, notification_guards(&conn));
    for (table, expected) in notification_rows {
        assert_eq!(
            expected,
            rows(&conn, &format!("SELECT * FROM {table} ORDER BY 1,2")),
            "{table}"
        );
    }
    for table in [
        "backend_job_execution_fences",
        "backend_job_recovery_probes",
    ] {
        assert!(rows(&conn, &format!("SELECT * FROM {table}")).is_empty());
    }
    assert_eq!(
        rows(&conn, "SELECT job_id FROM backend_job_reconciliation"),
        vec![vec![Value::Text("pending".into())]]
    );
    assert_eq!(
        rows(
            &conn,
            "SELECT count(*) FROM notification_source_preparation_failures"
        ),
        vec![vec![Value::Integer(2)]]
    );
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM notification_source_offers"),
        vec![vec![Value::Integer(2)]]
    );
    for table in ["backend_job_deliveries", "notification_job_legacy_receipts"] {
        assert!(!store::table_exists(&conn, table).unwrap());
    }
    assert!(
        conn.execute(
            "DELETE FROM notification_requests WHERE workspace_id='w'",
            []
        )
        .is_err()
    );
    assert!(conn.execute("UPDATE backend_jobs SET input_disposition='invalidated',request_json=json_set(request_json,'$.input',json('null')) WHERE job_id='current'",[]).is_err());
    assert!(conn.execute("INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES('w',NULL,'stale','backend_job','','backend_job_result','{\"backend_job\":{\"job_id\":\"old\",\"attempt_id\":\"old\"},\"result_digest\":\"result-digest\"}')",[]).is_err());
    assert_eq!(
        rows(&conn, "PRAGMA foreign_keys"),
        vec![vec![Value::Integer(1)]]
    );
}

#[test]
fn unmarked_legacy_deliveries_and_receipts_remove_owners_without_fabricating_ack() {
    let dir = tempfile::tempdir().unwrap();
    let conn = fixture(&dir.path().join("server.sqlite"));
    // These all have current descriptors and no upgrade marker. Legacy delivery
    // rows alone, imported receipts alone, and unrouted sources must all retire.
    for id in ["delivery-only", "receipt-only", "unrouted-delivery"] {
        job(&conn, "w", id, "completed");
        notification(&conn, "w", id, false);
        if id != "receipt-only" {
            legacy_delivery(&conn, "w", id);
        }
    }
    conn.execute_batch(r#"UPDATE backend_job_deliveries SET state='completed',delivered_at='accepted' WHERE job_id='delivery-only';
        INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose)
        SELECT sequence,workspace_id,'not-the-source-id','deliver','result','backend_job' FROM notification_sources WHERE input_event_id='arbitrary-notification:receipt-only';
        INSERT INTO notification_job_legacy_receipts VALUES('w','not-the-source-id','{"retained":true}',1);"#).unwrap();
    let acks = rows(
        &conn,
        "SELECT * FROM notification_acknowledgements WHERE delivery_id='arbitrary-notification:current' OR workspace_id='other' ORDER BY 1,2",
    );
    migrate(&conn).unwrap();
    assert!(rows(&conn,"SELECT job_id FROM backend_jobs WHERE job_id IN ('delivery-only','receipt-only','unrouted-delivery')").is_empty());
    assert_eq!(
        acks,
        rows(
            &conn,
            "SELECT * FROM notification_acknowledgements ORDER BY 1,2"
        )
    );
    assert!(rows(&conn,"SELECT input_event_id FROM notification_sources WHERE input_event_id IN ('arbitrary-notification:delivery-only','arbitrary-notification:receipt-only','arbitrary-notification:unrouted-delivery')").is_empty());
    for name in OBSOLETE_TABLES.iter().chain(OBSOLETE_LIVE_TRIGGERS) {
        assert!(
            rows(
                &conn,
                &format!("SELECT name FROM sqlite_schema WHERE name='{name}'")
            )
            .is_empty()
        );
    }
}

#[test]
fn unrelated_ticket_notifications_and_current_execution_fences_are_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let conn = fixture(&dir.path().join("server.sqlite"));
    conn.execute_batch("INSERT INTO typed_tickets(workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit) VALUES('w','ticket','ticket','Ticket','open','task','P2','open',1);
        INSERT INTO notification_sources(workspace_id,ticket_id,input_event_id,role,assignment_id,kind,facts_json) VALUES('w','ticket','unrelated-event','orchestrator','','fixture','{}');
        INSERT INTO notification_decisions(source_sequence,workspace_id,delivery_id,disposition,reason,purpose) VALUES(last_insert_rowid(),'w','unrelated-delivery','deliver','result','fixture');
        INSERT INTO notification_requests VALUES('w','unrelated-delivery','Unrelated content','{}');
        INSERT INTO notification_acknowledgements VALUES('w','unrelated-delivery','{}');
        INSERT INTO backend_job_execution_fences VALUES('w','pending','pending','{}','{}','{}','{}','current-fence');
        INSERT INTO backend_job_recovery_probes VALUES('w','pending','pending',7);").unwrap();
    let queries = [
        "SELECT * FROM notification_sources WHERE role='orchestrator'",
        "SELECT * FROM notification_decisions WHERE delivery_id='unrelated-delivery'",
        "SELECT * FROM notification_requests WHERE delivery_id='unrelated-delivery'",
        "SELECT * FROM notification_acknowledgements WHERE delivery_id='unrelated-delivery'",
        "SELECT * FROM backend_job_execution_fences WHERE job_id='pending'",
        "SELECT * FROM backend_job_recovery_probes WHERE job_id='pending'",
    ];
    let before: Vec<_> = queries.iter().map(|sql| rows(&conn, sql)).collect();
    migrate(&conn).unwrap();
    let after: Vec<_> = queries.iter().map(|sql| rows(&conn, sql)).collect();
    assert_eq!(before, after);
}

#[test]
fn descriptor_classification_preserves_current_grants_and_retired_input_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    store::tests::prepare_retained_schema(&path, 129);
    let conn = Connection::open(path).unwrap();
    store::configure_sqlite(&conn).unwrap();
    workspace(&conn);
    let journal = serde_json::json!({
        "binding_id":"source-lifetime", "subject_id":"subject", "session_id":"session",
        "journal_id":"journal", "selected_ranges":[]
    });
    job_with_request(&conn, "w", "journal-current", "completed", |r| {
        r["grants"] = serde_json::json!({"subjektiv_journal":journal});
    });
    job_with_request(&conn, "w", "journal-old", "completed", |r| {
        let mut old = journal.clone();
        old.as_object_mut().unwrap().remove("binding_id");
        r["grants"] = serde_json::json!({"subjektiv_journal":old});
    });
    job_with_request(&conn, "w", "memory-current", "completed", |r| {
        r["grants"] = serde_json::json!({"subjektiv_consolidation":{"subject_id":"subject","candidate_ids":[]}});
    });
    job_with_request(&conn, "w", "old-descriptor", "completed", |r| {
        r.as_object_mut().unwrap().remove("instruction");
    });
    job_with_request(&conn, "w", "unknown-grant", "completed", |r| {
        r["grants"] = serde_json::json!({"retired_capability":{}});
    });
    for id in ["reclaimed", "erased"] {
        job(&conn, "w", id, "completed");
        conn.execute("UPDATE backend_jobs SET input_disposition=?1,request_json=json_set(request_json,'$.input',json('null')) WHERE job_id=?1",[id]).unwrap();
    }
    for id in [
        "journal-current",
        "journal-old",
        "memory-current",
        "old-descriptor",
        "unknown-grant",
    ] {
        notification(&conn, "w", id, true);
    }
    let current = rows(
        &conn,
        "SELECT * FROM backend_jobs WHERE job_id IN ('journal-current','memory-current','reclaimed','erased') ORDER BY job_id",
    );
    let attempts = rows(
        &conn,
        "SELECT * FROM backend_job_attempts WHERE job_id IN ('journal-current','memory-current','reclaimed','erased') ORDER BY job_id",
    );
    migrate(&conn).unwrap();
    assert_eq!(
        current,
        rows(&conn, "SELECT * FROM backend_jobs ORDER BY job_id")
    );
    assert_eq!(
        attempts,
        rows(&conn, "SELECT * FROM backend_job_attempts ORDER BY job_id")
    );
    assert_eq!(
        rows(&conn, "SELECT count(*) FROM notification_sources"),
        vec![vec![Value::Integer(2)]]
    );
}

#[test]
fn receipt_failure_restores_all_rows_schema_guards_and_foreign_key_mode() {
    for foreign_keys in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let conn = fixture(&dir.path().join("server.sqlite"));
        conn.execute_batch(if foreign_keys {
            "PRAGMA foreign_keys=ON"
        } else {
            "PRAGMA foreign_keys=OFF"
        })
        .unwrap();
        conn.execute_batch("CREATE TEMP TRIGGER fail_current_receipt BEFORE INSERT ON main.__yoi_schema_migrations WHEN NEW.version=130 BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;").unwrap();
        let saved = dump(&conn);
        let saved_schema = schema(&conn);
        assert!(
            migrate(&conn)
                .unwrap_err()
                .to_string()
                .contains("injected receipt failure")
        );
        assert_eq!(saved, dump(&conn));
        assert_eq!(saved_schema, schema(&conn));
        assert_eq!(
            rows(&conn, "PRAGMA foreign_keys"),
            vec![vec![Value::Integer(i64::from(foreign_keys))]]
        );
        assert!(
            rows(
                &conn,
                "SELECT name FROM sqlite_temp_schema WHERE type='table' AND name LIKE '__yoi_%'"
            )
            .is_empty()
        );
        conn.execute_batch("DROP TRIGGER fail_current_receipt")
            .unwrap();
        migrate(&conn).unwrap();
    }
}

#[test]
fn deletion_failure_rolls_back_already_removed_notifications_and_guard_changes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = fixture(&dir.path().join("server.sqlite"));
    conn.execute_batch("CREATE TEMP TRIGGER fail_obsolete_attempt_delete BEFORE DELETE ON main.backend_job_attempts WHEN OLD.job_id='old' BEGIN SELECT RAISE(ABORT,'injected delete failure'); END;").unwrap();
    let saved = dump(&conn);
    let saved_schema = schema(&conn);
    assert!(
        migrate(&conn)
            .unwrap_err()
            .to_string()
            .contains("injected delete failure")
    );
    assert_eq!(saved, dump(&conn));
    assert_eq!(saved_schema, schema(&conn));
    assert_eq!(
        rows(&conn, "PRAGMA foreign_keys"),
        vec![vec![Value::Integer(1)]]
    );
    conn.execute_batch("DROP TRIGGER fail_obsolete_attempt_delete")
        .unwrap();
    migrate(&conn).unwrap();
}

#[test]
fn migration_rejects_wrong_boundary_and_current_reopen_does_not_sweep_new_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    let conn = fixture(&path);
    let saved = dump(&conn);
    {
        let tx = conn.unchecked_transaction().unwrap();
        assert!(migrate(&tx).is_err());
    }
    assert_eq!(saved, dump(&conn));
    migrate(&conn).unwrap();
    advance(&conn, store::LATEST_SCHEMA_VERSION);
    job(&conn, "w", "new-after-upgrade", "pending");
    let saved = job_graph(&conn);
    let history_query =
        "SELECT version,name,applied_at FROM __yoi_schema_migrations ORDER BY version";
    let old_history = rows(&conn, history_query);
    assert!(migrate(&conn).is_err());
    assert_eq!(saved, job_graph(&conn));
    assert_eq!(old_history, rows(&conn, history_query));
    drop(conn);
    drop(store::SqliteWorkspaceStore::open_for_test(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(saved, job_graph(&conn));
    let current_history = rows(&conn, history_query);
    assert_eq!(&current_history[..old_history.len()], old_history);
    let additions = &current_history[old_history.len()..];
    assert_eq!(
        additions
            .iter()
            .map(|row| row[..2].to_vec())
            .collect::<Vec<_>>(),
        store::MIGRATIONS
            .iter()
            .filter(|step| step.version > store::LATEST_SCHEMA_VERSION)
            .map(|step| vec![Value::Integer(step.version), Value::Text(step.name.into())])
            .collect::<Vec<_>>(),
        "owner open appends only the later schema receipts, without rewriting Job history"
    );
    assert!(
        additions
            .iter()
            .all(|row| matches!(&row[2], Value::Text(at) if !at.is_empty()))
    );
    assert_eq!(
        store::current_schema_version(&conn).unwrap(),
        store::LATEST_SCHEMA_VERSION
    );
    store::storage_formats::verify_history(&conn, store::LATEST_SCHEMA_VERSION).unwrap();
    drop(conn);
    drop(store::SqliteWorkspaceStore::open_for_test(&path).unwrap());
    let conn = Connection::open(path).unwrap();
    assert_eq!(saved, job_graph(&conn));
    assert_eq!(current_history, rows(&conn, history_query));
}

#[test]
fn pre_content_captures_and_old_request_aliases_are_boundary_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    store::tests::prepare_retained_schema(&path, 85);
    let conn = Connection::open(path).unwrap();
    store::configure_sqlite(&conn).unwrap();
    workspace(&conn);
    job(&conn, "w", "pre-content", "completed");
    advance(&conn, 129);
    assert!(!rows(&conn, "SELECT * FROM backend_job_pre_content_history").is_empty());
    assert!(
        !rows(
            &conn,
            "SELECT * FROM backend_job_attempt_pre_content_history"
        )
        .is_empty()
    );
    // A post-cutover malformed old envelope must not become a current request by
    // alias normalization. Insert it directly in this synthetic database only.
    conn.execute_batch("INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,created_at,updated_at) VALUES('w','old-alias','fixture','d','saved://input','{\"input_revision\":\"old-token\",\"input\":null}','old-intent','pending',1,'1','2');").unwrap();
    migrate(&conn).unwrap();
    assert!(rows(&conn, "SELECT * FROM backend_jobs").is_empty());
    assert!(rows(&conn, "SELECT * FROM backend_job_attempts").is_empty());
    assert!(rows(&conn, "SELECT * FROM notification_sources").is_empty());
    verify(&conn).unwrap();
}

#[test]
fn fresh_and_upgraded_current_job_schema_have_same_columns_constraints_and_programs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("server.sqlite");
    store::tests::prepare_retained_schema(&path, 129);
    let upgraded = Connection::open(path).unwrap();
    store::configure_sqlite(&upgraded).unwrap();
    migrate(&upgraded).unwrap();
    let fresh = Connection::open_in_memory().unwrap();
    store::prepare_connection(&fresh).unwrap();
    fn signature(conn: &Connection) -> Vec<Vec<Value>> {
        let tables = rows(
            conn,
            "SELECT name FROM sqlite_schema WHERE type='table' AND (name LIKE 'backend_job%' OR name LIKE 'notification_%') ORDER BY name",
        );
        let mut result = tables.clone();
        for table in tables {
            let Value::Text(name) = &table[0] else {
                panic!("table name")
            };
            for query in [
                format!(
                    "SELECT name,type,\"notnull\",dflt_value,pk FROM pragma_table_info('{name}') ORDER BY name"
                ),
                format!(
                    "SELECT \"table\",\"from\",\"to\",on_update,on_delete,match,seq FROM pragma_foreign_key_list('{name}') ORDER BY \"table\",\"from\",\"to\",seq"
                ),
                format!(
                    "SELECT i.\"unique\",i.partial,(SELECT group_concat(name,',') FROM (SELECT name FROM pragma_index_info(i.name) ORDER BY seqno)) FROM pragma_index_list('{name}') i ORDER BY 1,2,3"
                ),
            ] {
                result.extend(rows(conn, &query));
            }
        }
        result.extend(rows(conn,"SELECT type,name,replace(replace(replace(sql,char(10),''),char(13),''),' ','') FROM sqlite_schema WHERE name LIKE 'backend_job%' AND type IN ('trigger','index') AND sql IS NOT NULL ORDER BY type,name"));
        // Full tokenized CHECK/trigger comparison is also required by the actual
        // recognizer, not just this pragma signature.
        store::storage_formats::verify_history(conn, store::current_schema_version(conn).unwrap())
            .unwrap();
        result
    }
    assert_eq!(signature(&upgraded), signature(&fresh));
}

#[test]
fn current_receipt_does_not_accept_reintroduced_legacy_objects_or_weakened_guards() {
    for damage in [
        "CREATE TABLE backend_job_deliveries(workspace_id TEXT,delivery_id TEXT)",
        "CREATE TABLE notification_job_legacy_receipts(workspace_id TEXT,delivery_id TEXT)",
        "CREATE TABLE backend_job_upgrade_history(workspace_id TEXT,job_id TEXT)",
        "CREATE TRIGGER backend_job_reconciliation_no_legacy BEFORE INSERT ON backend_job_reconciliation BEGIN SELECT 1; END",
        "DROP TRIGGER backend_job_input_retirement",
        "DROP TABLE backend_job_reference_migration_jobs",
        "ALTER TABLE backend_job_reference_migration_targets ADD COLUMN unexpected TEXT",
        "UPDATE __yoi_schema_migrations SET name='counterfeit' WHERE version=130",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.sqlite");
        store::tests::prepare_retained_schema(&path, 129);
        let conn = Connection::open(path).unwrap();
        store::configure_sqlite(&conn).unwrap();
        migrate(&conn).unwrap();
        // Exercise the real schema130 receipt, not a fresh latest baseline
        // without that row (where the counterfeit UPDATE would do nothing).
        store::storage_formats::verify_history(&conn, 130).unwrap();
        conn.execute_batch(damage).unwrap();
        let before = schema(&conn);
        assert!(
            store::storage_formats::verify_history(&conn, 130).is_err(),
            "{damage}"
        );
        assert_eq!(before, schema(&conn));
    }
}
