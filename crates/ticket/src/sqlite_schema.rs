use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, TicketError, sqlite_err};

const MIGRATION_TABLE: &str = "ticket_schema_migrations";
const MAX_SCHEMA_DIAGNOSTICS: usize = 32;
pub const LATEST_SQLITE_TICKET_SCHEMA_VERSION: i64 = 5;

#[derive(Clone, Copy)]
struct Migration {
    version: i64,
    name: &'static str,
    apply: fn(&Connection) -> Result<()>,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "create_typed_ticket_tables",
        apply: create_typed_ticket_tables,
    },
    Migration {
        version: 2,
        name: "add_ticket_repository_target",
        apply: add_ticket_repository_target,
    },
    Migration {
        version: 3,
        name: "convert_legacy_reviews_to_comments",
        apply: retire_legacy_ticket_review_events,
    },
    Migration {
        version: 4,
        name: "add_ticket_query_indexes",
        apply: add_ticket_query_indexes,
    },
    Migration {
        version: 5,
        name: "add_workspace_human_keys",
        apply: add_workspace_human_keys,
    },
];

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

/// Applies the Ticket crate's SQLite migrations and verifies the resulting schema.
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
        validate_applied_migrations(&applied)?;

        for migration in MIGRATIONS {
            if applied.contains_key(&migration.version) {
                continue;
            }
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

fn create_typed_ticket_tables(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            r#"
CREATE TABLE IF NOT EXISTS typed_tickets (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    slug TEXT NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    kind TEXT NOT NULL,
    priority TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT,
    updated_at TEXT,
    assignee TEXT,
    readiness TEXT,
    workflow_state TEXT NOT NULL,
    workflow_state_explicit INTEGER NOT NULL,
    queued_by TEXT,
    queued_at TEXT,
    resolution TEXT,
    PRIMARY KEY (workspace_id, ticket_id)
);
CREATE TABLE IF NOT EXISTS typed_ticket_labels (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, label TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_risk_flags (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, ordinal INTEGER NOT NULL, risk_flag TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, ordinal),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_raw_frontmatter (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, key),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_events (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    event_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    author TEXT,
    at TEXT,
    status TEXT,
    from_state TEXT,
    to_state TEXT,
    reason TEXT,
    state_field TEXT,
    heading TEXT,
    body TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_event_references (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, ordinal),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_event_attributes (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, event_index INTEGER NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, event_index, key),
    FOREIGN KEY (workspace_id, ticket_id, event_index) REFERENCES typed_ticket_events(workspace_id, ticket_id, event_index) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_relations (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, kind TEXT NOT NULL, target TEXT NOT NULL, note TEXT, author TEXT NOT NULL, at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, kind, target),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_orchestration_plans (
    workspace_id TEXT NOT NULL,
    ticket_id TEXT NOT NULL,
    record_id TEXT NOT NULL,
    kind TEXT NOT NULL,
    related_ticket TEXT,
    note TEXT,
    accepted_summary TEXT,
    accepted_branch TEXT,
    accepted_worktree TEXT,
    accepted_role_plan TEXT,
    author TEXT NOT NULL,
    at TEXT NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, record_id),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS typed_ticket_artifacts (
    workspace_id TEXT NOT NULL, ticket_id TEXT NOT NULL, relative_path TEXT NOT NULL, content BLOB NOT NULL,
    PRIMARY KEY (workspace_id, ticket_id, relative_path),
    FOREIGN KEY (workspace_id, ticket_id) REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
);
"#,
        )
        .map_err(sqlite_err)
}

fn add_ticket_repository_target(connection: &Connection) -> Result<()> {
    add_column_if_missing(connection, "typed_tickets", "repository_id", "TEXT")?;
    add_column_if_missing(connection, "typed_tickets", "ref_selector", "TEXT")
}

fn retire_legacy_ticket_review_events(connection: &Connection) -> Result<()> {
    // Historical prose remains visible for audit, but it is explicitly converted to a
    // non-authoritative comment. Approval authority now lives only in Merge Requests.
    connection
        .execute_batch(
            r#"
        INSERT OR REPLACE INTO typed_ticket_event_attributes
            (workspace_id, ticket_id, event_index, key, value)
        SELECT workspace_id, ticket_id, event_index, 'legacy_event_kind', 'review'
        FROM typed_ticket_events WHERE kind = 'review';
        UPDATE typed_ticket_events
        SET kind = 'comment', status = NULL, heading = 'Legacy review (non-authoritative)'
        WHERE kind = 'review';
        DELETE FROM typed_ticket_event_attributes
        WHERE key IN ('result', 'review_result', 'status')
          AND EXISTS (
            SELECT 1 FROM typed_ticket_events event
            WHERE event.workspace_id = typed_ticket_event_attributes.workspace_id
              AND event.ticket_id = typed_ticket_event_attributes.ticket_id
              AND event.event_index = typed_ticket_event_attributes.event_index
              AND event.heading = 'Legacy review (non-authoritative)'
          );
        "#,
        )
        .map_err(sqlite_err)
}

fn add_ticket_query_indexes(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            r#"
        CREATE INDEX IF NOT EXISTS typed_tickets_workspace_state_updated
            ON typed_tickets(workspace_id, workflow_state, updated_at DESC, ticket_id);
        CREATE INDEX IF NOT EXISTS typed_tickets_workspace_updated
            ON typed_tickets(workspace_id, updated_at DESC, ticket_id);
        CREATE INDEX IF NOT EXISTS typed_tickets_workspace_created
            ON typed_tickets(workspace_id, created_at DESC, ticket_id);
        CREATE INDEX IF NOT EXISTS typed_tickets_workspace_title
            ON typed_tickets(workspace_id, title COLLATE NOCASE, ticket_id);
        CREATE INDEX IF NOT EXISTS typed_ticket_events_workspace_kind_ticket
            ON typed_ticket_events(workspace_id, kind, ticket_id, event_index);
        CREATE INDEX IF NOT EXISTS typed_ticket_relations_workspace_source_kind
            ON typed_ticket_relations(workspace_id, ticket_id, kind, target);
        CREATE INDEX IF NOT EXISTS typed_ticket_relations_workspace_target_kind
            ON typed_ticket_relations(workspace_id, target, kind, ticket_id);
        "#,
        )
        .map_err(sqlite_err)
}

fn add_workspace_human_keys(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            r#"
        CREATE TABLE IF NOT EXISTS workspace_resource_human_keys (
            workspace_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
            resource_id TEXT NOT NULL,
            sequence INTEGER NOT NULL CHECK (sequence > 0),
            human_key TEXT NOT NULL,
            allocated_at TEXT NOT NULL,
            PRIMARY KEY (workspace_id, resource_kind, resource_id),
            UNIQUE (workspace_id, resource_kind, sequence),
            UNIQUE (workspace_id, human_key)
        );
        CREATE TABLE IF NOT EXISTS workspace_resource_human_key_counters (
            workspace_id TEXT NOT NULL,
            resource_kind TEXT NOT NULL CHECK (resource_kind IN ('ticket', 'objective', 'worker')),
            next_sequence INTEGER NOT NULL CHECK (next_sequence > 0),
            PRIMARY KEY (workspace_id, resource_kind)
        );

        INSERT OR IGNORE INTO workspace_resource_human_keys (
            workspace_id, resource_kind, resource_id, sequence, human_key, allocated_at
        )
        SELECT workspace_id,
               'ticket',
               ticket_id,
               ROW_NUMBER() OVER (
                   PARTITION BY workspace_id ORDER BY created_at ASC, ticket_id ASC
               ),
               'T-' || ROW_NUMBER() OVER (
                   PARTITION BY workspace_id ORDER BY created_at ASC, ticket_id ASC
               ),
               COALESCE(created_at, updated_at)
        FROM typed_tickets;

        INSERT INTO workspace_resource_human_key_counters (
            workspace_id, resource_kind, next_sequence
        )
        SELECT workspace_id, 'ticket', MAX(sequence) + 1
        FROM workspace_resource_human_keys
        WHERE resource_kind = 'ticket'
        GROUP BY workspace_id
        ON CONFLICT(workspace_id, resource_kind) DO UPDATE SET
            next_sequence = MAX(next_sequence, excluded.next_sequence);
        "#,
        )
        .map_err(sqlite_err)
}

fn add_column_if_missing(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<()> {
    let columns = load_columns(connection, table)?;
    if columns.iter().any(|found| found.name == column) {
        return Ok(());
    }
    connection
        .execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))
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
    for (&version, name) in applied {
        let Some(expected) = MIGRATIONS
            .iter()
            .find(|migration| migration.version == version)
        else {
            return Err(TicketError::Sqlite(format!(
                "unsupported Ticket schema migration version {version}; latest supported version is {LATEST_SQLITE_TICKET_SCHEMA_VERSION}"
            )));
        };
        if name != expected.name {
            return Err(TicketError::Sqlite(format!(
                "Ticket schema migration {version} is named {name:?}, expected {:?}",
                expected.name
            )));
        }
    }
    for migration in MIGRATIONS {
        if applied.keys().any(|version| *version > migration.version)
            && !applied.contains_key(&migration.version)
        {
            return Err(TicketError::Sqlite(format!(
                "Ticket schema migration history has a gap at version {}",
                migration.version
            )));
        }
    }
    Ok(())
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
        assert_eq!(versions.len(), 5);
        assert_eq!(
            versions.get(&LATEST_SQLITE_TICKET_SCHEMA_VERSION),
            Some(&"add_workspace_human_keys".to_string())
        );
    }

    #[test]
    fn adopts_existing_current_schema_without_losing_data() {
        let connection = Connection::open_in_memory().unwrap();
        create_typed_ticket_tables(&connection).unwrap();
        add_ticket_repository_target(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO typed_tickets (
                    workspace_id, ticket_id, slug, title, status, kind, priority, body,
                    workflow_state, workflow_state_explicit, repository_id, ref_selector
                 ) VALUES ('workspace-1', 'ticket-1', 'ticket-1', 'kept', 'open',
                    'task', 'medium', 'body', 'ready', 1, 'main', 'develop')",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO typed_ticket_events (
                    workspace_id, ticket_id, event_index, kind, author, at, heading, body
                 ) VALUES (
                    'workspace-1', 'ticket-1', 0, 'comment', 'hare',
                    '2026-08-10T00:00:00Z', 'Evidence', 'event kept'
                 );
                 INSERT INTO typed_ticket_event_references (
                    workspace_id, ticket_id, event_index, ordinal, kind, target
                 ) VALUES ('workspace-1', 'ticket-1', 0, 0, 'commit', 'abc123');
                 INSERT INTO typed_ticket_relations (
                    workspace_id, ticket_id, kind, target, note, author, at
                 ) VALUES (
                    'workspace-1', 'ticket-1', 'related', 'ticket-2', 'relation kept',
                    'hare', '2026-08-10T00:00:00Z'
                 );
                 INSERT INTO typed_ticket_orchestration_plans (
                    workspace_id, ticket_id, record_id, kind, note, author, at
                 ) VALUES (
                    'workspace-1', 'ticket-1', 'plan-1', 'waiting_capacity_note',
                    'plan kept', 'hare', '2026-08-10T00:00:00Z'
                 );
                 INSERT INTO typed_ticket_artifacts (
                    workspace_id, ticket_id, relative_path, content
                 ) VALUES ('workspace-1', 'ticket-1', 'evidence.txt', X'6b657074');",
            )
            .unwrap();

        migrate_sqlite_ticket_schema(&connection).unwrap();

        let row = connection
            .query_row(
                "SELECT title, repository_id, ref_selector FROM typed_tickets",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, ("kept".into(), "main".into(), "develop".into()));
        let preserved = connection
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM typed_ticket_events),
                    (SELECT COUNT(*) FROM typed_ticket_event_references),
                    (SELECT COUNT(*) FROM typed_ticket_relations),
                    (SELECT COUNT(*) FROM typed_ticket_orchestration_plans),
                    (SELECT COUNT(*) FROM typed_ticket_artifacts)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(preserved, (1, 1, 1, 1, 1));
    }

    #[test]
    fn v5_backfills_ticket_keys_by_creation_order_and_advances_counter() {
        let connection = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        connection.execute_batch(
            "DROP TABLE workspace_resource_human_key_counters;
             DROP TABLE workspace_resource_human_keys;
             DELETE FROM ticket_schema_migrations WHERE version = 5;
             INSERT INTO typed_tickets (
                 workspace_id, ticket_id, slug, title, status, kind, priority, body,
                 workflow_state, workflow_state_explicit, created_at, updated_at
             ) VALUES
                 ('workspace-1', 'later', 'later', 'Later', 'open', 'task', 'medium', '', 'ready', 1, '2026-01-02T00:00:00Z', '2026-01-02T00:00:00Z'),
                 ('workspace-1', 'earlier', 'earlier', 'Earlier', 'open', 'task', 'medium', '', 'ready', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');"
        ).unwrap();

        migrate_sqlite_ticket_schema(&connection).unwrap();
        let keys = connection
            .prepare(
                "SELECT resource_id, human_key FROM workspace_resource_human_keys
             WHERE workspace_id = 'workspace-1' AND resource_kind = 'ticket'
             ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(
            keys,
            vec![
                ("earlier".into(), "T-1".into()),
                ("later".into(), "T-2".into())
            ]
        );
        let next: i64 = connection
            .query_row(
                "SELECT next_sequence FROM workspace_resource_human_key_counters
             WHERE workspace_id = 'workspace-1' AND resource_kind = 'ticket'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(next, 3);
    }

    #[test]
    fn upgrades_legacy_schema_without_repository_target_columns() {
        let connection = Connection::open_in_memory().unwrap();
        create_typed_ticket_tables(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO typed_tickets (
                    workspace_id, ticket_id, slug, title, status, kind, priority, body,
                    workflow_state, workflow_state_explicit
                 ) VALUES ('workspace-1', 'ticket-1', 'ticket-1', 'legacy', 'open',
                    'task', 'medium', 'body', 'ready', 1)",
                [],
            )
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE ticket_schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                 );
                 INSERT INTO ticket_schema_migrations (version, name, applied_at)
                 VALUES (1, 'create_typed_ticket_tables', '2026-08-10T00:00:00Z');",
            )
            .unwrap();

        migrate_sqlite_ticket_schema(&connection).unwrap();
        verify_sqlite_ticket_schema(&connection).unwrap();

        let columns = load_columns(&connection, "typed_tickets").unwrap();
        assert!(columns.iter().any(|column| column.name == "repository_id"));
        assert!(columns.iter().any(|column| column.name == "ref_selector"));
        let title = connection
            .query_row("SELECT title FROM typed_tickets", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap();
        assert_eq!(title, "legacy");
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
        assert!(
            error
                .to_string()
                .contains("unsupported Ticket schema migration version 99")
        );
        assert_eq!(load_applied_migrations(&connection).unwrap().len(), 6);
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
    fn migration_rejects_constraint_drift_and_rolls_back_version_adoption() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE typed_tickets (
                    workspace_id TEXT NOT NULL,
                    ticket_id TEXT NOT NULL,
                    slug TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    body TEXT NOT NULL,
                    created_at TEXT,
                    updated_at TEXT,
                    assignee TEXT,
                    readiness TEXT,
                    workflow_state TEXT NOT NULL,
                    workflow_state_explicit INTEGER NOT NULL,
                    queued_by TEXT,
                    queued_at TEXT,
                    resolution TEXT,
                    PRIMARY KEY (ticket_id, workspace_id)
                );",
            )
            .unwrap();

        let error = migrate_sqlite_ticket_schema(&connection).unwrap_err();
        assert!(error.to_string().contains("primary-key position"));
        let migration_table_exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ticket_schema_migrations'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(!migration_table_exists);
    }

    #[test]
    fn legacy_review_upgrade_preserves_prose_as_non_authoritative_comment() {
        let connection = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        connection.execute("INSERT INTO typed_tickets (workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit) VALUES ('workspace-1','ticket-1','ticket-1','title','open','task','medium','body','inprogress',1)",[]).unwrap();
        connection.execute("INSERT INTO typed_ticket_events (workspace_id,ticket_id,event_index,kind,author,at,status,heading,body) VALUES ('workspace-1','ticket-1',0,'review','reviewer','2026-08-11T00:00:00Z','approve','Review','legacy evidence')",[]).unwrap();
        connection.execute("INSERT INTO typed_ticket_event_attributes (workspace_id,ticket_id,event_index,key,value) VALUES ('workspace-1','ticket-1',0,'result','approve')",[]).unwrap();
        connection
            .execute("DELETE FROM ticket_schema_migrations WHERE version>=3", [])
            .unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        let (kind,status,heading,body):(String,Option<String>,Option<String>,Option<String>)=connection.query_row("SELECT kind,status,heading,body FROM typed_ticket_events WHERE workspace_id='workspace-1' AND ticket_id='ticket-1' AND event_index=0",[],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).unwrap();
        assert_eq!(kind, "comment");
        assert_eq!(status, None);
        assert_eq!(
            heading.as_deref(),
            Some("Legacy review (non-authoritative)")
        );
        assert_eq!(body.as_deref(), Some("legacy evidence"));
        let attributes:i64=connection.query_row("SELECT COUNT(*) FROM typed_ticket_event_attributes WHERE workspace_id='workspace-1' AND ticket_id='ticket-1'",[],|row|row.get(0)).unwrap();
        assert_eq!(attributes, 1);
        let legacy:String=connection.query_row("SELECT value FROM typed_ticket_event_attributes WHERE workspace_id='workspace-1' AND ticket_id='ticket-1' AND key='legacy_event_kind'",[],|row|row.get(0)).unwrap();
        assert_eq!(legacy, "review");
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
        assert_eq!(load_applied_migrations(&connection).unwrap().len(), 5);
    }
}
