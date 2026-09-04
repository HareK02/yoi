use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, TicketError, sqlite_err};

const MIGRATION_TABLE: &str = "ticket_schema_migrations";
const MAX_SCHEMA_DIAGNOSTICS: usize = 32;
const LATEST_SQLITE_TICKET_SCHEMA_VERSION: i64 = 6;

#[derive(Clone, Copy)]
struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const MIGRATIONS: &[Migration] = &[Migration {
    version: LATEST_SQLITE_TICKET_SCHEMA_VERSION,
    name: "ticket schema baseline",
    apply: create_latest_ticket_schema,
}];

#[derive(Clone, Copy)]
struct ExpectedColumn {
    name: &'static str,
    data_type: &'static str,
    not_null: bool,
    primary_key_position: i64,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ExpectedForeignKey {
    from: &'static str,
    target_table: &'static str,
    to: &'static str,
    on_delete: &'static str,
}

const OWNED_TABLES: &[&str] = &[
    "typed_tickets",
    "typed_ticket_labels",
    "typed_ticket_risk_flags",
    "typed_ticket_raw_frontmatter",
    "typed_ticket_events",
    "typed_ticket_event_references",
    "typed_ticket_event_attributes",
    "typed_ticket_relations",
    "typed_ticket_orchestration_plans",
    "typed_ticket_artifacts",
    "workspace_resource_keys",
];

const RESOURCE_KEY_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("resource_kind", "TEXT", true, 2),
    column("resource_id", "TEXT", true, 3),
    column("sequence", "INTEGER", true, 0),
    column("resource_key", "TEXT", true, 0),
    column("allocated_at", "TEXT", true, 0),
];

const RESOURCE_KEY_COUNTER_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("resource_kind", "TEXT", true, 2),
    column("next_sequence", "INTEGER", true, 0),
];

const MIGRATION_COLUMNS: &[ExpectedColumn] = &[
    column("version", "INTEGER", false, 1),
    column("name", "TEXT", true, 0),
    column("applied_at", "TEXT", true, 0),
];

const TICKET_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("slug", "TEXT", true, 0),
    column("title", "TEXT", true, 0),
    column("status", "TEXT", true, 0),
    column("kind", "TEXT", true, 0),
    column("priority", "TEXT", true, 0),
    column("body", "TEXT", true, 0),
    column("created_at", "TEXT", false, 0),
    column("updated_at", "TEXT", false, 0),
    column("assignee", "TEXT", false, 0),
    column("readiness", "TEXT", false, 0),
    column("workflow_state", "TEXT", true, 0),
    column("workflow_state_explicit", "INTEGER", true, 0),
    column("queued_by", "TEXT", false, 0),
    column("queued_at", "TEXT", false, 0),
    column("resolution", "TEXT", false, 0),
    column("repository_id", "TEXT", false, 0),
    column("ref_selector", "TEXT", false, 0),
];

const LABEL_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("ordinal", "INTEGER", true, 3),
    column("label", "TEXT", true, 0),
];

const RISK_FLAG_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("ordinal", "INTEGER", true, 3),
    column("risk_flag", "TEXT", true, 0),
];

const RAW_FRONTMATTER_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("key", "TEXT", true, 3),
    column("value", "TEXT", true, 0),
];

const EVENT_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("event_index", "INTEGER", true, 3),
    column("kind", "TEXT", true, 0),
    column("author", "TEXT", false, 0),
    column("at", "TEXT", false, 0),
    column("status", "TEXT", false, 0),
    column("from_state", "TEXT", false, 0),
    column("to_state", "TEXT", false, 0),
    column("reason", "TEXT", false, 0),
    column("state_field", "TEXT", false, 0),
    column("heading", "TEXT", false, 0),
    column("body", "TEXT", true, 0),
];

const EVENT_REFERENCE_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("event_index", "INTEGER", true, 3),
    column("ordinal", "INTEGER", true, 4),
    column("kind", "TEXT", true, 0),
    column("target", "TEXT", true, 0),
];

const EVENT_ATTRIBUTE_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("event_index", "INTEGER", true, 3),
    column("key", "TEXT", true, 4),
    column("value", "TEXT", true, 0),
];

const RELATION_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("kind", "TEXT", true, 3),
    column("target", "TEXT", true, 4),
    column("note", "TEXT", false, 0),
    column("author", "TEXT", true, 0),
    column("at", "TEXT", true, 0),
];

const ORCHESTRATION_PLAN_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("record_id", "TEXT", true, 3),
    column("kind", "TEXT", true, 0),
    column("related_ticket", "TEXT", false, 0),
    column("note", "TEXT", false, 0),
    column("accepted_summary", "TEXT", false, 0),
    column("accepted_branch", "TEXT", false, 0),
    column("accepted_worktree", "TEXT", false, 0),
    column("accepted_role_plan", "TEXT", false, 0),
    column("author", "TEXT", true, 0),
    column("at", "TEXT", true, 0),
];

const ARTIFACT_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("relative_path", "TEXT", true, 3),
    column("content", "BLOB", true, 0),
];

const TICKET_FOREIGN_KEYS: &[ExpectedForeignKey] = &[];
const CHILD_FOREIGN_KEYS: &[ExpectedForeignKey] = &[
    ExpectedForeignKey {
        from: "workspace_id",
        target_table: "typed_tickets",
        to: "workspace_id",
        on_delete: "CASCADE",
    },
    ExpectedForeignKey {
        from: "ticket_id",
        target_table: "typed_tickets",
        to: "ticket_id",
        on_delete: "CASCADE",
    },
];
const EVENT_CHILD_FOREIGN_KEYS: &[ExpectedForeignKey] = &[
    ExpectedForeignKey {
        from: "workspace_id",
        target_table: "typed_ticket_events",
        to: "workspace_id",
        on_delete: "CASCADE",
    },
    ExpectedForeignKey {
        from: "ticket_id",
        target_table: "typed_ticket_events",
        to: "ticket_id",
        on_delete: "CASCADE",
    },
    ExpectedForeignKey {
        from: "event_index",
        target_table: "typed_ticket_events",
        to: "event_index",
        on_delete: "CASCADE",
    },
];

const fn column(
    name: &'static str,
    data_type: &'static str,
    not_null: bool,
    primary_key_position: i64,
) -> ExpectedColumn {
    ExpectedColumn {
        name,
        data_type,
        not_null,
        primary_key_position,
    }
}

/// Creates and verifies the Ticket crate's latest SQLite schema.
///
/// This is a startup/standalone-open operation. Normal Ticket request handling must
/// use [`verify_sqlite_ticket_schema`] instead, so request paths never acquire DDL
/// authority.
pub fn migrate_sqlite_ticket_schema(connection: &Connection) -> Result<()> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(sqlite_err)?;
    connection
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(sqlite_err)?;

    let result = (|| {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS ticket_schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                );",
            )
            .map_err(sqlite_err)?;
        verify_table(connection, MIGRATION_TABLE, MIGRATION_COLUMNS, &[], false)?;

        let applied = load_applied_migrations(connection)?;
        if applied.is_empty() {
            let migration = MIGRATIONS
                .first()
                .ok_or_else(|| TicketError::Sqlite("Ticket migration catalog is empty".into()))?;
            (migration.apply)(connection)?;
            connection
                .execute(
                    "INSERT INTO ticket_schema_migrations (version, name, applied_at)
                     VALUES (?1, ?2, ?3)",
                    params![
                        migration.version,
                        migration.name,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )
                .map_err(sqlite_err)?;
        } else {
            validate_applied_migrations(&applied)?;
        }

        verify_sqlite_ticket_schema(connection)
    })();

    match result {
        Ok(()) => connection.execute_batch("COMMIT").map_err(sqlite_err),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Verifies the current Ticket-owned SQLite schema without executing DDL.
pub fn verify_sqlite_ticket_schema(connection: &Connection) -> Result<()> {
    let mut diagnostics = Vec::new();

    collect_table_diagnostics(
        connection,
        MIGRATION_TABLE,
        MIGRATION_COLUMNS,
        &[],
        &mut diagnostics,
    );

    match load_applied_migrations(connection) {
        Ok(applied) => {
            if let Err(error) = validate_applied_migrations(&applied) {
                push_diagnostic(&mut diagnostics, error.to_string());
            } else if applied.len() != MIGRATIONS.len() {
                push_diagnostic(
                    &mut diagnostics,
                    format!(
                        "Ticket schema is not current: found {} migration(s), expected {}",
                        applied.len(),
                        MIGRATIONS.len()
                    ),
                );
            }
        }
        Err(error) => push_diagnostic(&mut diagnostics, error.to_string()),
    }

    for (table, columns, foreign_keys) in [
        ("typed_tickets", TICKET_COLUMNS, TICKET_FOREIGN_KEYS),
        ("typed_ticket_labels", LABEL_COLUMNS, CHILD_FOREIGN_KEYS),
        (
            "typed_ticket_risk_flags",
            RISK_FLAG_COLUMNS,
            CHILD_FOREIGN_KEYS,
        ),
        (
            "typed_ticket_raw_frontmatter",
            RAW_FRONTMATTER_COLUMNS,
            CHILD_FOREIGN_KEYS,
        ),
        ("typed_ticket_events", EVENT_COLUMNS, CHILD_FOREIGN_KEYS),
        (
            "typed_ticket_event_references",
            EVENT_REFERENCE_COLUMNS,
            EVENT_CHILD_FOREIGN_KEYS,
        ),
        (
            "typed_ticket_event_attributes",
            EVENT_ATTRIBUTE_COLUMNS,
            EVENT_CHILD_FOREIGN_KEYS,
        ),
        (
            "typed_ticket_relations",
            RELATION_COLUMNS,
            CHILD_FOREIGN_KEYS,
        ),
        (
            "typed_ticket_orchestration_plans",
            ORCHESTRATION_PLAN_COLUMNS,
            CHILD_FOREIGN_KEYS,
        ),
        (
            "typed_ticket_artifacts",
            ARTIFACT_COLUMNS,
            CHILD_FOREIGN_KEYS,
        ),
    ] {
        collect_table_diagnostics(connection, table, columns, foreign_keys, &mut diagnostics);
    }

    collect_column_diagnostics(
        connection,
        "workspace_resource_keys",
        RESOURCE_KEY_COLUMNS,
        &mut diagnostics,
    );
    collect_column_diagnostics(
        connection,
        "workspace_resource_key_counters",
        RESOURCE_KEY_COUNTER_COLUMNS,
        &mut diagnostics,
    );
    collect_index_diagnostics(
        connection,
        "workspace_resource_keys",
        None,
        true,
        &["workspace_id", "resource_kind", "sequence"],
        &mut diagnostics,
    );
    collect_index_diagnostics(
        connection,
        "workspace_resource_keys",
        None,
        true,
        &["workspace_id", "resource_key"],
        &mut diagnostics,
    );
    collect_index_diagnostics(
        connection,
        "workspace_resource_keys",
        Some("idx_workspace_resource_keys_reverse"),
        false,
        &["workspace_id", "resource_kind", "resource_key"],
        &mut diagnostics,
    );
    for legacy_table in [
        "workspace_resource_human_keys",
        "workspace_resource_human_key_counters",
    ] {
        collect_absent_table_diagnostic(connection, legacy_table, &mut diagnostics);
    }

    for table in OWNED_TABLES {
        collect_foreign_key_check_diagnostics(connection, table, &mut diagnostics);
    }

    if diagnostics.is_empty() {
        Ok(())
    } else {
        let was_truncated = diagnostics.len() > MAX_SCHEMA_DIAGNOSTICS;
        diagnostics.truncate(MAX_SCHEMA_DIAGNOSTICS);
        let mut message = format!(
            "Ticket SQLite schema verification failed: {}",
            diagnostics.join("; ")
        );
        if was_truncated {
            message.push_str("; additional diagnostics omitted");
        }
        Err(TicketError::Sqlite(message))
    }
}

fn create_latest_ticket_schema(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(include_str!("latest_schema.sql"))
        .map_err(sqlite_err)
}

fn load_applied_migrations(connection: &Connection) -> Result<BTreeMap<i64, String>> {
    let mut statement = connection
        .prepare("SELECT version, name FROM ticket_schema_migrations ORDER BY version")
        .map_err(sqlite_err)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(sqlite_err)?;
    let mut applied = BTreeMap::new();
    for row in rows {
        let (version, name) = row.map_err(sqlite_err)?;
        if applied.insert(version, name).is_some() {
            return Err(TicketError::Sqlite(format!(
                "duplicate Ticket schema migration version {version}"
            )));
        }
    }
    Ok(applied)
}

fn validate_applied_migrations(applied: &BTreeMap<i64, String>) -> Result<()> {
    let expected = BTreeMap::from([(
        LATEST_SQLITE_TICKET_SCHEMA_VERSION,
        MIGRATIONS[0].name.to_string(),
    )]);
    if applied == &expected {
        Ok(())
    } else {
        Err(TicketError::Sqlite(format!(
            "Ticket schema migration history must contain only the canonical version {LATEST_SQLITE_TICKET_SCHEMA_VERSION} baseline marker"
        )))
    }
}

#[derive(Debug)]
struct ActualColumn {
    name: String,
    data_type: String,
    not_null: bool,
    primary_key_position: i64,
}

fn load_columns(connection: &Connection, table: &str) -> Result<Vec<ActualColumn>> {
    let exists = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![table],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite_err)?
        .is_some();
    if !exists {
        return Err(TicketError::Sqlite(format!(
            "required Ticket schema table {table:?} is missing"
        )));
    }

    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_err)?;
    let rows = statement
        .query_map([], |row| {
            Ok(ActualColumn {
                name: row.get(1)?,
                data_type: row.get::<_, String>(2)?.to_ascii_uppercase(),
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key_position: row.get(5)?,
            })
        })
        .map_err(sqlite_err)?;
    rows.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)
}

fn verify_table(
    connection: &Connection,
    table: &str,
    expected_columns: &[ExpectedColumn],
    expected_foreign_keys: &[ExpectedForeignKey],
    check_foreign_keys: bool,
) -> Result<()> {
    let mut diagnostics = Vec::new();
    collect_column_diagnostics(connection, table, expected_columns, &mut diagnostics);
    if check_foreign_keys {
        collect_foreign_key_diagnostics(connection, table, expected_foreign_keys, &mut diagnostics);
    }
    if diagnostics.is_empty() {
        Ok(())
    } else {
        Err(TicketError::Sqlite(diagnostics.join("; ")))
    }
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(sqlite_err)
}

fn collect_absent_table_diagnostic(
    connection: &Connection,
    table: &str,
    diagnostics: &mut Vec<String>,
) {
    match table_exists(connection, table) {
        Ok(false) => {}
        Ok(true) => diagnostics.push(format!("legacy table `{table}` is still present")),
        Err(error) => {
            diagnostics.push(format!("failed to inspect legacy table `{table}`: {error}"))
        }
    }
}

fn collect_index_diagnostics(
    connection: &Connection,
    table: &str,
    expected_name: Option<&str>,
    expected_unique: bool,
    expected_columns: &[&str],
    diagnostics: &mut Vec<String>,
) {
    let sql = format!("PRAGMA index_list({table})");
    let mut statement = match connection.prepare(&sql) {
        Ok(statement) => statement,
        Err(error) => {
            diagnostics.push(format!("failed to inspect indexes for `{table}`: {error}"));
            return;
        }
    };
    let rows = match statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? != 0))
    }) {
        Ok(rows) => rows,
        Err(error) => {
            diagnostics.push(format!("failed to read indexes for `{table}`: {error}"));
            return;
        }
    };
    let indexes = match rows.collect::<std::result::Result<Vec<_>, _>>() {
        Ok(indexes) => indexes,
        Err(error) => {
            diagnostics.push(format!("failed to decode indexes for `{table}`: {error}"));
            return;
        }
    };
    for (name, unique) in indexes {
        if expected_name.is_some_and(|expected| expected != name) || unique != expected_unique {
            continue;
        }
        let sql = format!("PRAGMA index_info({name})");
        let mut statement = match connection.prepare(&sql) {
            Ok(statement) => statement,
            Err(error) => {
                diagnostics.push(format!("failed to inspect index `{name}`: {error}"));
                return;
            }
        };
        let rows = match statement.query_map([], |row| row.get::<_, String>(2)) {
            Ok(rows) => rows,
            Err(error) => {
                diagnostics.push(format!("failed to read index `{name}`: {error}"));
                return;
            }
        };
        match rows.collect::<std::result::Result<Vec<_>, _>>() {
            Ok(columns) if columns == expected_columns => return,
            Ok(_) => {}
            Err(error) => {
                diagnostics.push(format!("failed to decode index `{name}`: {error}"));
                return;
            }
        }
    }
    let identity = expected_name
        .map(|name| format!("named `{name}`"))
        .unwrap_or_else(|| "unnamed".to_string());
    diagnostics.push(format!(
        "table `{table}` is missing {identity} {} index on ({})",
        if expected_unique {
            "unique"
        } else {
            "non-unique"
        },
        expected_columns.join(", ")
    ));
}

fn collect_table_diagnostics(
    connection: &Connection,
    table: &str,
    expected_columns: &[ExpectedColumn],
    expected_foreign_keys: &[ExpectedForeignKey],
    diagnostics: &mut Vec<String>,
) {
    collect_column_diagnostics(connection, table, expected_columns, diagnostics);
    if diagnostics.len() < MAX_SCHEMA_DIAGNOSTICS {
        collect_foreign_key_diagnostics(connection, table, expected_foreign_keys, diagnostics);
    }
}

fn collect_column_diagnostics(
    connection: &Connection,
    table: &str,
    expected: &[ExpectedColumn],
    diagnostics: &mut Vec<String>,
) {
    let actual = match load_columns(connection, table) {
        Ok(actual) => actual,
        Err(error) => {
            push_diagnostic(diagnostics, error.to_string());
            return;
        }
    };

    let expected_names = expected
        .iter()
        .map(|column| column.name)
        .collect::<BTreeSet<_>>();
    let actual_names = actual
        .iter()
        .map(|column| column.name.as_str())
        .collect::<BTreeSet<_>>();
    for missing in expected_names.difference(&actual_names) {
        push_diagnostic(
            diagnostics,
            format!("table {table:?} is missing column {missing:?}"),
        );
    }
    for unexpected in actual_names.difference(&expected_names) {
        push_diagnostic(
            diagnostics,
            format!("table {table:?} has unexpected column {unexpected:?}"),
        );
    }

    for expected in expected {
        let Some(actual) = actual.iter().find(|column| column.name == expected.name) else {
            continue;
        };
        if actual.data_type != expected.data_type {
            push_diagnostic(
                diagnostics,
                format!(
                    "table {table:?} column {:?} has type {:?}, expected {:?}",
                    expected.name, actual.data_type, expected.data_type
                ),
            );
        }
        if actual.not_null != expected.not_null {
            push_diagnostic(
                diagnostics,
                format!(
                    "table {table:?} column {:?} NOT NULL is {}, expected {}",
                    expected.name, actual.not_null, expected.not_null
                ),
            );
        }
        if actual.primary_key_position != expected.primary_key_position {
            push_diagnostic(
                diagnostics,
                format!(
                    "table {table:?} column {:?} primary-key position is {}, expected {}",
                    expected.name, actual.primary_key_position, expected.primary_key_position
                ),
            );
        }
    }
}

fn collect_foreign_key_diagnostics(
    connection: &Connection,
    table: &str,
    expected: &[ExpectedForeignKey],
    diagnostics: &mut Vec<String>,
) {
    let mut statement = match connection.prepare(&format!("PRAGMA foreign_key_list({table})")) {
        Ok(statement) => statement,
        Err(error) => {
            push_diagnostic(diagnostics, sqlite_err(error).to_string());
            return;
        }
    };
    let rows = match statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(3)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(6)?.to_ascii_uppercase(),
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => {
            push_diagnostic(diagnostics, sqlite_err(error).to_string());
            return;
        }
    };
    let mut actual = BTreeSet::new();
    for row in rows {
        match row {
            Ok(foreign_key) => {
                actual.insert(foreign_key);
            }
            Err(error) => push_diagnostic(diagnostics, sqlite_err(error).to_string()),
        }
    }
    let expected = expected
        .iter()
        .map(|foreign_key| {
            (
                foreign_key.from.to_string(),
                foreign_key.target_table.to_string(),
                foreign_key.to.to_string(),
                foreign_key.on_delete.to_string(),
            )
        })
        .collect::<BTreeSet<_>>();
    // The Ticket component owns its required foreign keys, while an integrated host may
    // strengthen Workspace/domain boundaries with additional references to host-owned
    // tables. Reject missing component constraints, but do not treat those host extensions
    // as Ticket schema drift.
    for missing in expected.difference(&actual) {
        push_diagnostic(
            diagnostics,
            format!("table {table:?} is missing foreign key {missing:?}"),
        );
    }
}

fn collect_foreign_key_check_diagnostics(
    connection: &Connection,
    table: &str,
    diagnostics: &mut Vec<String>,
) {
    let mut statement = match connection.prepare(&format!("PRAGMA foreign_key_check({table})")) {
        Ok(statement) => statement,
        Err(error) => {
            push_diagnostic(diagnostics, sqlite_err(error).to_string());
            return;
        }
    };
    let rows = match statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(error) => {
            push_diagnostic(diagnostics, sqlite_err(error).to_string());
            return;
        }
    };
    for row in rows {
        match row {
            Ok((table, row_id, parent, foreign_key_id)) => push_diagnostic(
                diagnostics,
                format!(
                    "foreign-key violation in table {table:?} row {row_id:?} referencing {parent:?} (foreign key {foreign_key_id})"
                ),
            ),
            Err(error) => push_diagnostic(diagnostics, sqlite_err(error).to_string()),
        }
    }
}

fn push_diagnostic(diagnostics: &mut Vec<String>, diagnostic: String) {
    if diagnostics.len() <= MAX_SCHEMA_DIAGNOSTICS {
        diagnostics.push(diagnostic);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::*;
    use crate::SqliteTicketBackend;

    #[test]
    fn migrates_fresh_database_to_current_ticket_schema() {
        let connection = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        verify_sqlite_ticket_schema(&connection).unwrap();

        let versions = load_applied_migrations(&connection).unwrap();
        assert_eq!(
            versions,
            BTreeMap::from([(
                LATEST_SQLITE_TICKET_SCHEMA_VERSION,
                "ticket schema baseline".to_string(),
            )])
        );

        migrate_sqlite_ticket_schema(&connection).unwrap();
        assert_eq!(load_applied_migrations(&connection).unwrap(), versions);
    }

    #[test]
    fn rejects_unknown_future_migration_history_without_changing_schema() {
        let connection = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO ticket_schema_migrations (version, name, applied_at)
                 VALUES (99, 'future', '2026-08-10T00:00:00Z')",
                [],
            )
            .unwrap();

        let error = migrate_sqlite_ticket_schema(&connection).unwrap_err();
        assert!(error.to_string().contains(
            "migration history must contain only the canonical version 6 baseline marker"
        ));
        assert_eq!(load_applied_migrations(&connection).unwrap().len(), 2);
    }

    #[test]
    fn rejects_legacy_migration_marker() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ticket_schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                 );
                 INSERT INTO ticket_schema_migrations (version, name, applied_at)
                 VALUES (6, 'rename_workspace_resource_keys', '2026-08-10T00:00:00Z');",
            )
            .unwrap();

        let error = migrate_sqlite_ticket_schema(&connection).unwrap_err();
        assert!(error.to_string().contains(
            "migration history must contain only the canonical version 6 baseline marker"
        ));
        assert!(!table_exists(&connection, "typed_tickets").unwrap());
    }

    #[test]
    fn verifier_rejects_resource_key_schema_drift() {
        for (drift, expected) in [
            (
                "DROP TABLE workspace_resource_key_counters",
                "workspace_resource_key_counters",
            ),
            (
                "ALTER TABLE workspace_resource_keys RENAME COLUMN resource_key TO human_key",
                "missing column \"resource_key\"",
            ),
            (
                "DROP INDEX idx_workspace_resource_keys_reverse",
                "idx_workspace_resource_keys_reverse",
            ),
            (
                "CREATE TABLE workspace_resource_human_keys (value TEXT)",
                "legacy table `workspace_resource_human_keys` is still present",
            ),
        ] {
            let connection = Connection::open_in_memory().unwrap();
            migrate_sqlite_ticket_schema(&connection).unwrap();
            connection.execute_batch(drift).unwrap();

            let error = verify_sqlite_ticket_schema(&connection).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} after {drift:?}, got {error}"
            );
        }
    }

    #[test]
    fn verified_backend_open_fails_on_drift_without_repairing_request_schema() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("tickets.db");
        SqliteTicketBackend::open(&database, "workspace-1").unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch("DROP TABLE typed_ticket_artifacts")
            .unwrap();

        let error = match SqliteTicketBackend::open_verified(&database, "workspace-1") {
            Ok(_) => panic!("drifted schema unexpectedly passed request-path verification"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("typed_ticket_artifacts"));
        let still_missing = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'typed_ticket_artifacts'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none();
        assert!(still_missing);
    }

    #[test]
    fn verification_does_not_claim_unrelated_foreign_key_authority() {
        let connection = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE unrelated_parent (id TEXT PRIMARY KEY);
                 CREATE TABLE unrelated_child (
                    id TEXT PRIMARY KEY,
                    parent_id TEXT NOT NULL REFERENCES unrelated_parent(id)
                 );
                 INSERT INTO unrelated_child (id, parent_id) VALUES ('child', 'missing');",
            )
            .unwrap();

        verify_sqlite_ticket_schema(&connection).unwrap();
    }

    #[test]
    fn concurrent_migrators_converge_on_one_version_history() {
        let directory = tempdir().unwrap();
        let database = directory.path().join("tickets.db");
        let barrier = Arc::new(Barrier::new(3));
        let mut joins = Vec::new();
        for _ in 0..2 {
            let database = database.clone();
            let barrier = barrier.clone();
            joins.push(thread::spawn(move || {
                let connection = Connection::open(database).unwrap();
                barrier.wait();
                migrate_sqlite_ticket_schema(&connection)
            }));
        }
        barrier.wait();
        for join in joins {
            join.join().unwrap().unwrap();
        }

        let connection = Connection::open(database).unwrap();
        verify_sqlite_ticket_schema(&connection).unwrap();
        assert_eq!(load_applied_migrations(&connection).unwrap().len(), 1);
    }
}
