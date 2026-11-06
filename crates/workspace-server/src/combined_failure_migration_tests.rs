//! Owner-level regression tests for the two published, overlapping failure tails.
//! Registered by store.rs as a child module. Fixtures compose actual pre-124
//! physical schemas: a fresh baseline is never made by deleting old receipts.
use super::*;
use rusqlite::{params, types::Value};
use std::collections::BTreeMap;
use std::path::Path;

// Frozen published identities/DDL, independent of the planner's current choice.
const OLD_JOB_NAME: &str = "Backend Job bounded structured failure evidence";
const OLD_SERVICE_NAME: &str = "Plugin Service retained outcomes and causes";
const OLD_JOB_DDL: &str = "
ALTER TABLE backend_jobs ADD COLUMN failure_cause TEXT
    CHECK(failure_cause IS NULL OR length(CAST(failure_cause AS BLOB))<=32768);
ALTER TABLE backend_job_attempts ADD COLUMN failure_cause TEXT
    CHECK(failure_cause IS NULL OR length(CAST(failure_cause AS BLOB))<=32768);
ALTER TABLE backend_job_attempts ADD COLUMN worker_cleanup_failure_cause TEXT
    CHECK(worker_cleanup_failure_cause IS NULL OR length(CAST(worker_cleanup_failure_cause AS BLOB))<=32768);
";
const OLD_SERVICE_DDL: &str =
    "ALTER TABLE plugin_service_consumers ADD COLUMN last_outcome_json TEXT;";
const STARTUP_DDL: &str = include_str!("worker_create_failure_schema.sql");
const FAILURE_COLUMNS: &[(&str, &str)] = &[
    ("worker_create_reservations", "startup_failure_json"),
    ("backend_jobs", "failure_cause"),
    ("backend_job_attempts", "failure_cause"),
    ("backend_job_attempts", "worker_cleanup_failure_cause"),
    ("plugin_service_consumers", "last_outcome_json"),
];

#[derive(Clone, Copy, Debug)]
enum Tail {
    Canonical,
    Published,
}

impl Tail {
    fn steps(self) -> [(i64, &'static str, &'static str); 4] {
        match self {
            Self::Canonical => [
                (124, WORKER_CREATE_FAILURE_MIGRATION_NAME, STARTUP_DDL),
                (125, OLD_JOB_NAME, OLD_JOB_DDL),
                (126, OLD_SERVICE_NAME, OLD_SERVICE_DDL),
                (
                    127,
                    journal_removal_store::MIGRATION_NAME,
                    journal_removal_store::SCHEMA_SQL,
                ),
            ],
            Self::Published => [
                (124, OLD_JOB_NAME, OLD_JOB_DDL),
                (125, OLD_SERVICE_NAME, OLD_SERVICE_DDL),
                (126, RETAINED_STARTUP_MIGRATION_NAME, STARTUP_DDL),
                (
                    127,
                    journal_removal_store::MIGRATION_NAME,
                    journal_removal_store::SCHEMA_SQL,
                ),
            ],
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Origin {
    Fresh,
    Ordered,
    // A genuine fresh 124 baseline followed by the next published receipt(s).
    Fresh124Prefix,
}

fn retained_step(conn: &Connection, version: i64, name: &str, ddl: &str) {
    let tx = conn.unchecked_transaction().unwrap();
    tx.execute_batch(ddl).unwrap();
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name,applied_at) VALUES(?1,?2,?3)",
        params![
            version,
            name,
            format!("saved receipt {version}: not migration time")
        ],
    )
    .unwrap();
    tx.commit().unwrap();
}

fn fixture(path: &Path, tail: Tail, current: i64, origin: Origin) {
    assert!((123..=127).contains(&current));
    let conn = match origin {
        Origin::Ordered => {
            tests::prepare_retained_schema(path, 123);
            Connection::open(path).unwrap()
        }
        Origin::Fresh | Origin::Fresh124Prefix => {
            let conn = Connection::open(path).unwrap();
            configure_sqlite(&conn).unwrap();
            ticket::migrate_sqlite_ticket_schema(&conn).unwrap();
            merge_request::migrate(&conn).unwrap();
            // Same rebuild window as the fresh owner: execution-123 rebuilds
            // parent tables before installing their final guards.
            conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;")
                .unwrap();
            let baseline = match origin {
                Origin::Fresh124Prefix => {
                    assert!(current >= 124);
                    124
                }
                _ => current,
            };
            // Match the fresh owner's composition only through execution-123,
            // then install the exact historical additive DDL, with no ledger
            // writes until the actual single baseline is recorded.
            let tx = conn.unchecked_transaction().unwrap();
            create_workspace_schema_through_v113(&tx).unwrap();
            tx.execute_batch(content_history_store::SCHEMA).unwrap();
            tx.execute_batch(crate::config_adoption::SCHEMA).unwrap();
            backend_job_upgrade::install_fresh(&tx).unwrap();
            for (version, _, ddl) in tail.steps() {
                if version <= baseline {
                    tx.execute_batch(ddl).unwrap();
                }
            }
            tx.execute(
                "INSERT INTO __yoi_schema_migrations(version,name,applied_at) VALUES(?1,?2,?3)",
                params![
                    baseline,
                    SCHEMA_BASELINE_NAME,
                    "saved fresh baseline timestamp"
                ],
            )
            .unwrap();
            tx.commit().unwrap();
            conn.execute_batch("PRAGMA legacy_alter_table=OFF; PRAGMA foreign_keys=ON;")
                .unwrap();
            conn
        }
    };
    configure_sqlite(&conn).unwrap();
    let baseline = current_schema_version(&conn).unwrap();
    for (version, name, ddl) in tail.steps() {
        if version > baseline && version <= current {
            retained_step(&conn, version, name, ddl);
        }
    }
    assert_eq!(current_schema_version(&conn).unwrap(), current);
    seed_rows(&conn);
}

fn seed_rows(conn: &Connection) {
    conn.execute_batch(r#"
INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
    VALUES('combined-owner','user','combined-owner','Owner','created','updated');
INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
    VALUES('combined-workspace','combined-owner','Saved workspace','active','created','updated');
INSERT INTO worker_create_reservations(workspace_id,allocation_key,worker_id,runtime_id,create_fingerprint,state,created_at,updated_at)
    VALUES('combined-workspace','saved-allocation','01900000-0000-7000-8000-000000000001','saved-runtime','saved-fingerprint','reserved','created','updated');
INSERT INTO backend_jobs(workspace_id,job_id,purpose,input_digest,input_ref,request_json,intent_fingerprint,state,current_attempt,result_json,result_digest,failure_category,failure_detail,created_at,updated_at,completed_at,input_disposition)
    VALUES('combined-workspace','saved-job','journal','digest','saved://input','{ "input": null }','fingerprint','completed',1,'{ "saved": true }','result-digest','old-job-cause','old job detail','created','updated','completed','erased');
INSERT INTO backend_job_attempts(workspace_id,job_id,attempt_id,attempt,input_digest,state,deadline_at,result_json,result_digest,failure_category,failure_detail,created_at,updated_at,completed_at)
    VALUES('combined-workspace','saved-job','saved-attempt',1,'digest','completed','deadline','{ "saved_attempt": true }','attempt-result-digest','old-attempt-cause','old attempt detail','created','updated','completed');
INSERT INTO plugin_installations VALUES('combined-workspace','installation','alias','package','installed',0);
"#).unwrap();
    let fingerprint = format!("sha256:{}", "a".repeat(64));
    conn.execute("INSERT INTO plugin_execution_plans VALUES('combined-workspace','plan','owner','plan',?1,'{}','active')", [&fingerprint]).unwrap();
    conn.execute_batch(r#"
INSERT INTO plugin_service_consumers(workspace_id,consumer_id,runtime_id,worker_id,installation_id,plan_id,service_name,subscriptions_json,cursor,drain_through,lease_token,lease_until_ms,last_ack_event_id)
    VALUES('combined-workspace','saved-consumer','saved-runtime','saved-worker','installation','plan','watch','["changed"]',7,11,'saved-lease',12345,'saved-event');
INSERT INTO plugin_service_released_proofs(workspace_id,consumer_id,lease_token,released_at_ms)
    VALUES('combined-workspace','saved-consumer','old-lease',123);
"#).unwrap();
    // Retain raw bytes, including future envelopes and whitespace. Migration
    // neither decodes them nor replaces them with a re-encoded known variant.
    for &(table, column) in FAILURE_COLUMNS {
        if column_exists(conn, table, column).unwrap() {
            conn.execute(
                &format!("UPDATE {table} SET {column}=?1"),
                [format!(" {{ \"version\": 999, \"saved\": \"{column}\" }} ")],
            )
            .unwrap();
        }
    }
    assert!(rows(conn, "PRAGMA foreign_key_check").is_empty());
}

fn rows(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = conn.prepare(sql).unwrap();
    let count = stmt.column_count();
    stmt.query_map([], |row| {
        (0..count)
            .map(|i| row.get(i))
            .collect::<rusqlite::Result<Vec<Value>>>()
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

type Columns = BTreeMap<String, Vec<String>>;
type SavedRows = BTreeMap<String, Vec<String>>;

fn all_columns(conn: &Connection) -> Columns {
    rows(conn, "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .into_iter()
        .map(|row| {
            let Value::Text(table) = &row[0] else { panic!("table name is text") };
            (table.clone(), table_columns(conn, table).unwrap())
        })
        .collect()
}

fn values(conn: &Connection, columns: &Columns) -> SavedRows {
    columns
        .iter()
        .map(|(table, columns)| {
            let columns = columns
                .iter()
                .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",");
            let mut saved = rows(conn, &format!("SELECT {columns} FROM \"{table}\""))
                .into_iter()
                .map(|row| format!("{row:?}"))
                .collect::<Vec<_>>();
            saved.sort();
            (table.clone(), saved)
        })
        .collect()
}

fn schema(conn: &Connection) -> Vec<Vec<Value>> {
    rows(
        conn,
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
    )
}

fn receipts(conn: &Connection) -> Vec<Vec<Value>> {
    rows(
        conn,
        "SELECT version,name,applied_at FROM __yoi_schema_migrations ORDER BY version",
    )
}

fn assert_old_receipts(conn: &Connection, saved: &[Vec<Value>]) {
    let actual = receipts(conn);
    assert_eq!(
        &actual[..saved.len()],
        saved,
        "existing receipts, including applied_at, must not change"
    );
}

// SQLite retains source indentation and may quote identifiers after ALTER.
// Ignore those spelling differences, but retain every byte inside SQL literals.
fn program_signature(sql: &str) -> String {
    let mut literal = false;
    let mut normalized = String::new();
    for ch in sql.chars() {
        if ch == '\'' {
            literal = !literal;
            normalized.push(ch);
        } else if literal {
            normalized.push(ch);
        } else if !ch.is_whitespace() && ch != '"' && ch != '`' {
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    normalized
}

// Column order can differ across additive histories, but types, defaults, keys,
// foreign keys, indexes and trigger programs must match the fresh owner. Bounds
// are exercised separately against populated rows (not inferred from SQL text).
fn failure_signature(conn: &Connection) -> Vec<Vec<Value>> {
    let mut signature = Vec::new();
    for table in [
        "worker_create_reservations",
        "backend_jobs",
        "backend_job_attempts",
        "plugin_service_consumers",
        "worker_removal_operations",
        "workspace_configuration_publication_observations",
        "external_workdir_connection_observations",
    ] {
        signature.extend(rows(conn, &format!("SELECT '{table}',name,type,\"notnull\",dflt_value,pk FROM pragma_table_info('{table}') ORDER BY name")));
        signature.extend(rows(
            conn,
            &format!("SELECT '{table}',* FROM pragma_foreign_key_list('{table}') ORDER BY id,seq"),
        ));
        let mut programs = rows(
            conn,
            &format!(
                "SELECT type,name,sql FROM sqlite_schema WHERE tbl_name='{table}' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name"
            ),
        );
        for program in &mut programs {
            let Value::Text(sql) = &program[2] else {
                panic!("stored program SQL is text")
            };
            program[2] = Value::Text(program_signature(sql));
        }
        signature.extend(programs);
    }
    signature
}

fn assert_bounds_and_restore(conn: &Connection) {
    for &(table, column) in FAILURE_COLUMNS {
        assert!(column_exists(conn, table, column).unwrap());
        let original = rows(conn, &format!("SELECT {column} FROM {table}"));
        assert_eq!(
            original.len(),
            1,
            "{table}: bounds must exercise a real row"
        );
        let sql = format!("UPDATE {table} SET {column}=?1");
        conn.execute(&sql, [Option::<String>::None]).unwrap();
        // Service's published DDL is nullable TEXT, not a SQL byte CHECK.
        if column != "last_outcome_json" {
            for accepted in ["x".repeat(32768), "é".repeat(16384)] {
                assert_eq!(conn.execute(&sql, [accepted]).unwrap(), 1);
            }
            for rejected in ["x".repeat(32769), "é".repeat(16385)] {
                assert!(
                    conn.execute(&sql, [rejected]).is_err(),
                    "{table}.{column} must bound UTF-8 bytes"
                );
            }
            if column == "startup_failure_json" {
                assert!(conn.execute(&sql, [Value::Blob(vec![0])]).is_err());
            }
        }
        conn.execute(&sql, [&original[0][0]]).unwrap();
    }
}

fn assert_plan(plan: &WorkspaceSchemaMigrationPlan, tail: Tail, current: i64) {
    // Schema 123 has no published failure-tail identity yet.
    let tail = if current < 124 { Tail::Canonical } else { tail };
    assert_eq!(plan.current_schema_version, current);
    assert_eq!(plan.target_schema_version, LATEST_SCHEMA_VERSION);
    assert_eq!(
        plan.migrations
            .iter()
            .map(|step| (step.version, step.name.as_str()))
            .collect::<Vec<_>>(),
        tail.steps()
            .into_iter()
            .chain(std::iter::once((
                128,
                DOMAIN_FAILURE_OBSERVATIONS_MIGRATION_NAME,
                ""
            )))
            .filter(|(version, _, _)| *version > current)
            .map(|(version, name, _)| (version, name))
            .collect::<Vec<_>>()
    );
}

#[test]
fn combined_failure_fresh_and_ordered_published_histories_plan_apply_and_reopen() {
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let expected = fresh.with_conn(|conn| Ok(failure_signature(conn))).unwrap();
    for tail in [Tail::Canonical, Tail::Published] {
        for origin in [Origin::Fresh, Origin::Ordered, Origin::Fresh124Prefix] {
            for current in 123..=127 {
                if matches!(origin, Origin::Fresh124Prefix) && current < 124 {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("combined.sqlite");
                fixture(&path, tail, current, origin);
                let conn = Connection::open(&path).unwrap();
                let before_schema = schema(&conn);
                let mut old_columns = all_columns(&conn);
                let before_all = values(&conn, &old_columns);
                let old_receipts = receipts(&conn);
                let old_failure_columns = FAILURE_COLUMNS
                    .iter()
                    .filter(|(table, column)| column_exists(&conn, table, column).unwrap())
                    .copied()
                    .collect::<Vec<_>>();
                let plan = SqliteWorkspaceStore::migration_plan(&path)
                    .unwrap_or_else(|error| panic!("{tail:?} {origin:?} {current}: {error}"));
                assert_plan(&plan, tail, current);
                assert_eq!(schema(&conn), before_schema, "dry run changed source DDL");
                assert_eq!(
                    values(&conn, &old_columns),
                    before_all,
                    "dry run changed source data/receipts"
                );
                old_columns.remove("__yoi_schema_migrations");
                let retained = values(&conn, &old_columns);
                drop(conn);

                let applied = SqliteWorkspaceStore::migrate_database(&path).unwrap();
                assert_plan(&applied, tail, current);
                for _ in 0..2 {
                    let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
                    store
                        .with_conn(|conn| {
                            assert_eq!(
                                current_schema_version(conn).unwrap(),
                                LATEST_SCHEMA_VERSION
                            );
                            assert_old_receipts(conn, &old_receipts);
                            assert_eq!(
                                values(conn, &old_columns),
                                retained,
                                "{tail:?} {origin:?} {current}: retained column data changed"
                            );
                            assert_eq!(
                                failure_signature(conn),
                                expected,
                                "{tail:?} {origin:?} {current}: schema differs from fresh owner"
                            );
                            for &(table, column) in FAILURE_COLUMNS {
                                if !old_failure_columns.contains(&(table, column)) {
                                    assert_eq!(
                                        rows(conn, &format!("SELECT {column} FROM {table}")),
                                        vec![vec![Value::Null]]
                                    );
                                }
                            }
                            assert_bounds_and_restore(conn);
                            assert_eq!(values(conn, &old_columns), retained);
                            assert!(rows(conn, "PRAGMA foreign_key_check").is_empty());
                            Ok(())
                        })
                        .unwrap();
                }
                let conn = Connection::open(&path).unwrap();
                assert!(
                    rows(
                        &conn,
                        "SELECT * FROM external_workdir_connection_observations"
                    )
                    .is_empty()
                );
                let final_receipts = receipts(&conn);
                assert_eq!(
                    final_receipts.len(),
                    old_receipts.len() + plan.migrations.len()
                );
                assert_eq!(
                    final_receipts[old_receipts.len()..]
                        .iter()
                        .map(|row| (row[0].clone(), row[1].clone()))
                        .collect::<Vec<_>>(),
                    plan.migrations
                        .iter()
                        .map(|step| (Value::Integer(step.version), Value::Text(step.name.clone())))
                        .collect::<Vec<_>>(),
                    "apply must append exactly the planned identities"
                );
                drop(conn);
                let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
                assert_plan(&plan, tail, LATEST_SCHEMA_VERSION);
                assert_eq!(receipts(&Connection::open(&path).unwrap()), final_receipts);
            }
        }
    }
}

#[test]
fn combined_failure_receipt_126_abort_rolls_back_ddl_and_data_then_retries_once() {
    for tail in [Tail::Canonical, Tail::Published] {
        for origin in [Origin::Fresh, Origin::Ordered, Origin::Fresh124Prefix] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("rollback.sqlite");
            fixture(&path, tail, 125, origin);
            let conn = Connection::open(&path).unwrap();
            configure_sqlite(&conn).unwrap();
            let missing = match tail {
                Tail::Canonical => ("plugin_service_consumers", "last_outcome_json"),
                Tail::Published => ("worker_create_reservations", "startup_failure_json"),
            };
            assert!(!column_exists(&conn, missing.0, missing.1).unwrap());
            conn.execute_batch("CREATE TRIGGER interrupt_combined_126 BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=126 BEGIN SELECT RAISE(ABORT,'injected combined failure receipt interruption'); END;").unwrap();
            let before_schema = schema(&conn);
            let columns = all_columns(&conn);
            let before = values(&conn, &columns);
            let saved_receipts = receipts(&conn);
            // Public preflight also executes a private-copy rehearsal. Its
            // failure must leave the retained source entirely untouched.
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(values(&conn, &columns), before);
            assert!(apply_migrations(&conn).is_err());
            assert_eq!(current_schema_version(&conn).unwrap(), 125);
            assert!(!column_exists(&conn, missing.0, missing.1).unwrap());
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(values(&conn, &columns), before);
            conn.execute_batch("DROP TRIGGER interrupt_combined_126")
                .unwrap();
            apply_migrations(&conn).unwrap();
            assert_eq!(
                current_schema_version(&conn).unwrap(),
                LATEST_SCHEMA_VERSION
            );
            assert!(column_exists(&conn, missing.0, missing.1).unwrap());
            assert_old_receipts(&conn, &saved_receipts);
            let after = receipts(&conn);
            assert_eq!(after.len(), saved_receipts.len() + 3);
            assert_eq!(
                after[saved_receipts.len()][1],
                Value::Text(tail.steps()[2].1.into())
            );
            apply_migrations(&conn).unwrap();
            assert_eq!(
                receipts(&conn),
                after,
                "retry recorded the final receipt twice"
            );
            drop(conn);
            let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            store
                .with_conn(|conn| {
                    assert_eq!(receipts(conn), after);
                    assert_bounds_and_restore(conn);
                    Ok(())
                })
                .unwrap();
        }
    }
}

fn assert_rejected_without_writes(path: &Path, context: &str) {
    let conn = Connection::open(path).unwrap();
    configure_sqlite(&conn).unwrap();
    let before_schema = schema(&conn);
    let columns = all_columns(&conn);
    let before = values(&conn, &columns);
    let before_changes = conn.total_changes();
    assert!(
        workspace_schema_migration_plan(&conn).is_err(),
        "{context}: planner accepted malformed history"
    );
    assert!(
        SqliteWorkspaceStore::migration_plan(path).is_err(),
        "{context}: public dry run accepted malformed history"
    );
    assert!(
        apply_migrations(&conn).is_err(),
        "{context}: apply accepted malformed history"
    );
    assert_eq!(
        conn.total_changes(),
        before_changes,
        "{context}: runner wrote before rejecting"
    );
    assert_eq!(
        schema(&conn),
        before_schema,
        "{context}: rejection repaired DDL"
    );
    assert_eq!(
        values(&conn, &columns),
        before,
        "{context}: rejection changed rows/receipts"
    );
    drop(conn);
    assert!(
        SqliteWorkspaceStore::open_for_test(path).is_err(),
        "{context}: owner open accepted malformed history"
    );
    let conn = Connection::open(path).unwrap();
    assert_eq!(schema(&conn), before_schema);
    assert_eq!(values(&conn, &columns), before);
}

#[test]
fn combined_failure_malformed_mixed_gap_and_extra_shapes_fail_closed() {
    for tail in [Tail::Canonical, Tail::Published] {
        for (current, damage) in [
            (
                124,
                "UPDATE __yoi_schema_migrations SET name='not a published failure migration' WHERE version=124",
            ),
            (
                125,
                "UPDATE __yoi_schema_migrations SET name='wrong 125 identity' WHERE version=125",
            ),
            (125, "DELETE FROM __yoi_schema_migrations WHERE version=124"),
            (
                125,
                "INSERT INTO __yoi_schema_migrations(version,name,applied_at) VALUES(127,'invented future step','saved future timestamp')",
            ),
            (
                125,
                "ALTER TABLE backend_jobs ADD COLUMN unexpected_failure_tail TEXT",
            ),
            (
                125,
                "CREATE TRIGGER unexpected_job_tail AFTER UPDATE ON backend_jobs BEGIN SELECT 1; END;",
            ),
            (
                125,
                "CREATE TRIGGER unexpected_service_tail AFTER UPDATE ON plugin_service_consumers BEGIN SELECT 1; END;",
            ),
            (
                125,
                "ALTER TABLE backend_job_attempts DROP COLUMN worker_cleanup_failure_cause",
            ),
            (
                125,
                "ALTER TABLE backend_jobs DROP COLUMN failure_cause; ALTER TABLE backend_jobs ADD COLUMN failure_cause TEXT",
            ),
            (
                125,
                "ALTER TABLE backend_job_attempts DROP COLUMN failure_cause; ALTER TABLE backend_job_attempts ADD COLUMN failure_cause TEXT CHECK(failure_cause IS NULL OR length(failure_cause)<=32768)",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("invalid.sqlite");
            fixture(&path, tail, current, Origin::Ordered);
            Connection::open(&path)
                .unwrap()
                .execute_batch(damage)
                .unwrap();
            assert_rejected_without_writes(&path, &format!("{tail:?} {current}: {damage}"));
        }
        // Same version numbers cannot license swapping the two real identities.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mixed.sqlite");
        fixture(&path, tail, 125, Origin::Ordered);
        let conn = Connection::open(&path).unwrap();
        let other = match tail {
            Tail::Canonical => OLD_SERVICE_NAME,
            Tail::Published => OLD_JOB_NAME,
        };
        conn.execute(
            "UPDATE __yoi_schema_migrations SET name=?1 WHERE version=125",
            [other],
        )
        .unwrap();
        drop(conn);
        assert_rejected_without_writes(&path, &format!("{tail:?}: mixed real identities"));
    }
}

#[test]
fn combined_failure_premature_columns_and_counterfeit_startup_check_are_not_receipts() {
    for (tail, current, damage) in [
        (Tail::Canonical, 123, STARTUP_DDL),
        (Tail::Canonical, 123, OLD_JOB_DDL),
        (Tail::Canonical, 124, OLD_SERVICE_DDL),
        (Tail::Published, 124, STARTUP_DDL),
        (Tail::Published, 124, OLD_SERVICE_DDL),
        (
            Tail::Canonical,
            126,
            "CREATE TRIGGER unexpected_startup_tail AFTER UPDATE ON worker_create_reservations BEGIN SELECT 1; END;",
        ),
        (
            Tail::Published,
            126,
            "ALTER TABLE plugin_service_consumers DROP COLUMN last_outcome_json; ALTER TABLE plugin_service_consumers ADD COLUMN last_outcome_json BLOB",
        ),
        (
            Tail::Canonical,
            126,
            "ALTER TABLE worker_create_reservations DROP COLUMN startup_failure_json; ALTER TABLE worker_create_reservations ADD COLUMN startup_failure_json TEXT /* CHECK (startup_failure_json IS NULL OR (typeof(startup_failure_json) = 'text' AND length(CAST(startup_failure_json AS BLOB)) <= 32768)) */ CHECK(startup_failure_json IS NULL OR 1)",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("counterfeit.sqlite");
        fixture(&path, tail, current, Origin::Fresh);
        Connection::open(&path)
            .unwrap()
            .execute_batch(damage)
            .unwrap();
        assert_rejected_without_writes(&path, &format!("{tail:?} {current}: {damage}"));
    }
}

// Independently exported published inputs, loaded before any current producer or
// registry. This branch has Journal-124, not Startup-124 or Job-124, and cannot
// acquire their numbered receipts by relabeling its history.
const JOURNAL_HISTORIES: [&str; 2] = [
    include_str!("fixtures/published_journal124_ordered.sql"),
    include_str!("fixtures/published_journal124_fresh.sql"),
];

fn published_journal(path: &Path, sql: &str) -> Connection {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(sql).unwrap();
    conn.execute_batch(include_str!("fixtures/published_journal124_values.sql"))
        .unwrap();
    conn
}

fn assert_journal_bridge_plan(plan: &WorkspaceSchemaMigrationPlan) {
    assert_eq!(plan.current_schema_version, 124);
    assert_eq!(plan.target_schema_version, LATEST_SCHEMA_VERSION);
    assert_eq!(
        plan.migrations
            .iter()
            .map(|step| (step.version, step.name.as_str()))
            .collect::<Vec<_>>(),
        [
            (127, journal_removal_store::RETAINED_BRIDGE_NAME),
            (128, DOMAIN_FAILURE_OBSERVATIONS_MIGRATION_NAME)
        ]
    );
}

#[test]
fn journal_bridge_preserves_both_published_histories_and_matches_fresh_through_owner_runner() {
    let fresh = SqliteWorkspaceStore::in_memory_for_test().unwrap();
    let expected = fresh.with_conn(|conn| Ok(failure_signature(conn))).unwrap();
    for sql in JOURNAL_HISTORIES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("published-journal.sqlite");
        let conn = published_journal(&path, sql);
        let before_schema = schema(&conn);
        let mut old_columns = all_columns(&conn);
        let before = values(&conn, &old_columns);
        let old_receipts = receipts(&conn);
        let plan = SqliteWorkspaceStore::migration_plan(&path).unwrap();
        assert_journal_bridge_plan(&plan);
        assert_eq!(
            schema(&conn),
            before_schema,
            "private-copy dry run must not touch source DDL"
        );
        assert_eq!(
            values(&conn, &old_columns),
            before,
            "dry run must retain every old value and applied_at"
        );
        old_columns.remove("__yoi_schema_migrations");
        let retained = values(&conn, &old_columns);
        drop(conn);
        let applied = SqliteWorkspaceStore::migrate_database(&path).unwrap();
        assert_journal_bridge_plan(&applied);
        for _ in 0..2 {
            let store = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            store.with_conn(|conn| {
                assert_eq!(current_schema_version(conn)?, LATEST_SCHEMA_VERSION);
                assert_old_receipts(conn, &old_receipts);
                assert_eq!(receipts(conn).len(), old_receipts.len()+2, "no empty intermediate receipts");
                assert_eq!(values(conn, &old_columns), retained, "all old columns and values must remain exact");
                assert_eq!(failure_signature(conn), expected, "all changed tables, keys and programs match fresh composition");
                storage_formats::verify_history(conn, LATEST_SCHEMA_VERSION)?;
                assert_eq!(rows(conn, "SELECT j.state,a.state,j.failure_cause FROM backend_jobs j JOIN backend_job_attempts a USING(workspace_id,job_id) WHERE j.job_id='after-retirement'"),
                    vec![vec![Value::Text("pending".into()),Value::Text("reserved".into()),Value::Null]], "Job-123 must not sweep a post-retirement Job again");
                assert!(rows(conn, "PRAGMA foreign_key_check").is_empty());
                // Exercise the new bounded Job columns against a real preserved
                // Job, then roll the probes back without changing its state.
                conn.execute_batch("SAVEPOINT journal_bounds")?;
                for column in ["failure_cause"] {
                    let update = format!("UPDATE backend_jobs SET {column}=?1 WHERE job_id='after-retirement'");
                    conn.execute(&update,["é".repeat(16384)])?;
                    assert!(conn.execute(&update,["é".repeat(16385)]).is_err());
                }
                conn.execute_batch("ROLLBACK TO journal_bounds; RELEASE journal_bounds")?;
                assert_eq!(values(conn, &old_columns), retained);
                Ok(())
            }).unwrap();
        }
    }
}

#[test]
fn journal_bridge_failure_rolls_back_all_extensions_and_receipt_and_retries_same_history() {
    for sql in JOURNAL_HISTORIES {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal-rollback.sqlite");
        let conn = published_journal(&path, sql);
        conn.execute_batch("CREATE TRIGGER reject_journal_bridge_receipt BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=127 BEGIN SELECT RAISE(ABORT,'injected Journal bridge receipt failure'); END;").unwrap();
        let before_schema = schema(&conn);
        let columns = all_columns(&conn);
        let before = values(&conn, &columns);
        for result in [
            SqliteWorkspaceStore::migration_plan(&path),
            SqliteWorkspaceStore::migrate_database(&path),
        ] {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("injected Journal bridge receipt failure")
            );
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(values(&conn, &columns), before);
        }
        assert!(
            !column_exists(&conn, "worker_create_reservations", "startup_failure_json").unwrap()
        );
        assert!(!column_exists(&conn, "backend_jobs", "failure_cause").unwrap());
        assert!(!column_exists(&conn, "plugin_service_consumers", "last_outcome_json").unwrap());
        conn.execute_batch("DROP TRIGGER reject_journal_bridge_receipt")
            .unwrap();
        let plan = SqliteWorkspaceStore::migrate_database(&path).unwrap();
        assert_journal_bridge_plan(&plan);
        storage_formats::verify_history(&conn, LATEST_SCHEMA_VERSION).unwrap();
        assert_eq!(
            current_schema_version(&conn).unwrap(),
            LATEST_SCHEMA_VERSION
        );
    }
}

#[test]
fn journal_bridge_rejects_mixed_prefix_receipts_and_shape_without_repair() {
    for sql in JOURNAL_HISTORIES {
        // Deliberately corrupt only private negative fixtures. The production
        // runner never UPDATEs, deletes or fabricates an old ledger row.
        for corrupt in [
            "UPDATE __yoi_schema_migrations SET name='Worker create coordinator returned startup failure' WHERE version=124",
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(125,'Backend Job bounded structured failure evidence')",
            "INSERT INTO __yoi_schema_migrations(version,name) VALUES(127,'Journal frozen scope on Worker removal intents')",
            "ALTER TABLE backend_jobs ADD COLUMN failure_cause TEXT",
            "DROP TRIGGER worker_removal_journal_immutable",
            "UPDATE __yoi_schema_migrations SET name='unknown baseline' WHERE version=(SELECT min(version) FROM __yoi_schema_migrations)",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("journal-invalid.sqlite");
            let conn = published_journal(&path, sql);
            conn.execute_batch(corrupt).unwrap();
            let before_schema = schema(&conn);
            let columns = all_columns(&conn);
            let before = values(&conn, &columns);
            assert!(
                SqliteWorkspaceStore::migration_plan(&path).is_err(),
                "{corrupt}"
            );
            assert!(
                SqliteWorkspaceStore::migrate_database(&path).is_err(),
                "{corrupt}"
            );
            assert!(
                SqliteWorkspaceStore::open_for_test(&path).is_err(),
                "{corrupt}"
            );
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(values(&conn, &columns), before);
        }
    }
}

#[test]
fn domain_observation_receipt_abort_rolls_back_both_schemas_and_retries_without_changing_adoption_data()
 {
    for tail in [Tail::Canonical, Tail::Published] {
        for origin in [Origin::Fresh, Origin::Ordered, Origin::Fresh124Prefix] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("publication-rollback.sqlite");
            fixture(&path, tail, 126, origin);
            let conn = Connection::open(&path).unwrap();
            configure_sqlite(&conn).unwrap();
            journal_removal_store::migrate_to_v127(&conn).unwrap();
            conn.execute_batch("CREATE TRIGGER interrupt_publication_receipt BEFORE INSERT ON __yoi_schema_migrations WHEN NEW.version=128 BEGIN SELECT RAISE(ABORT,'injected publication receipt interruption'); END;").unwrap();
            let before_schema = schema(&conn);
            let columns = all_columns(&conn);
            let before = values(&conn, &columns);
            let retained_receipts = receipts(&conn);
            assert!(SqliteWorkspaceStore::migration_plan(&path).is_err());
            assert!(apply_migrations(&conn).is_err());
            assert_eq!(current_schema_version(&conn).unwrap(), 127);
            assert_eq!(schema(&conn), before_schema);
            assert_eq!(values(&conn, &columns), before);
            conn.execute_batch("DROP TRIGGER interrupt_publication_receipt")
                .unwrap();
            apply_migrations(&conn).unwrap();
            assert_eq!(
                current_schema_version(&conn).unwrap(),
                LATEST_SCHEMA_VERSION
            );
            let mut retained_columns = columns.clone();
            retained_columns.remove("__yoi_schema_migrations");
            assert_eq!(
                values(&conn, &retained_columns),
                values_from_before(&before, &retained_columns)
            );
            assert_old_receipts(&conn, &retained_receipts);
            assert_eq!(receipts(&conn).len(), retained_receipts.len() + 1);
            assert!(
                rows(
                    &conn,
                    "SELECT * FROM workspace_configuration_publication_observations"
                )
                .is_empty()
            );
            assert!(
                rows(
                    &conn,
                    "SELECT * FROM external_workdir_connection_observations"
                )
                .is_empty()
            );
            let final_receipts = receipts(&conn);
            apply_migrations(&conn).unwrap();
            assert_eq!(receipts(&conn), final_receipts);
            drop(conn);
            let reopened = SqliteWorkspaceStore::open_for_test(&path).unwrap();
            reopened
                .with_conn(|conn| {
                    assert_eq!(receipts(conn), final_receipts);
                    assert_eq!(
                        values(conn, &retained_columns),
                        values_from_before(&before, &retained_columns)
                    );
                    Ok(())
                })
                .unwrap();
        }
    }
}

fn values_from_before(saved: &SavedRows, columns: &Columns) -> SavedRows {
    columns
        .keys()
        .map(|table| (table.clone(), saved[table].clone()))
        .collect()
}
