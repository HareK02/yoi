//! Ticket schema authority is the verified persisted shape, not a version marker alone.
//! Startup migrations compose case, tag, and assignment schemas without replacing applied
//! migrations; request paths verify only and never acquire DDL authority.
//!
//! Legacy planning/ready/queued/inprogress become open; done/closed become closed with
//! an explicit migration actor and unknown reason, never inferred success. Immutable
//! snapshots retain the old phase, flags, routing, and resolution for downstream handoff,
//! not a second current case authority. Existing tags, tombstones, memberships, receipts,
//! judgments, and audit snapshots survive composition and are not regenerated on rerun.
//! Assignment migration preserves identities and reconciled evidence; ambiguous or
//! historical relationships remain diagnostic unknown handling, not fresh execution input.
//! Migration never starts Workers or restores execution grants.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Result, TicketError, sqlite_err};

const MIGRATION_TABLE: &str = "ticket_schema_migrations";
const MAX_SCHEMA_DIAGNOSTICS: usize = 32;
const LATEST_SQLITE_TICKET_SCHEMA_VERSION: i64 = 15;
const BASELINE_MIGRATION_NAME: &str = "ticket schema baseline";

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
    "workspace_tags",
    "typed_ticket_tags",
    "workspace_tag_operations",
    "typed_tickets",
    "typed_ticket_targets",
    "typed_ticket_labels",
    "typed_ticket_risk_flags",
    "typed_ticket_raw_frontmatter",
    "typed_ticket_events",
    "typed_ticket_event_references",
    "typed_ticket_event_attributes",
    "typed_ticket_relations",
    "typed_ticket_orchestration_plans",
    "typed_ticket_artifacts",
    "typed_ticket_case_metadata",
    "typed_ticket_case_history",
    "typed_ticket_legacy_workflow_snapshot",
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
];

const TAG_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("tag_id", "TEXT", true, 2),
    column("name", "TEXT", true, 0),
    column("normalized_name", "TEXT", true, 0),
    column("last_operation_key", "TEXT", true, 0),
    column("deleted", "INTEGER", true, 0),
];
const TICKET_TAG_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("tag_id", "TEXT", true, 3),
];
const TAG_OPERATION_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("operation_key", "TEXT", true, 2),
    column("kind", "TEXT", true, 0),
    column("target_id", "TEXT", true, 0),
    column("input_json", "TEXT", true, 0),
    column("source_context_json", "TEXT", true, 0),
    column("before_json", "TEXT", true, 0),
    column("result_json", "TEXT", true, 0),
    column("at", "TEXT", true, 0),
];
const TICKET_TAG_FOREIGN_KEYS: &[ExpectedForeignKey] = &[
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
    ExpectedForeignKey {
        from: "workspace_id",
        target_table: "workspace_tags",
        to: "workspace_id",
        on_delete: "RESTRICT",
    },
    ExpectedForeignKey {
        from: "tag_id",
        target_table: "workspace_tags",
        to: "tag_id",
        on_delete: "RESTRICT",
    },
];

const TARGET_COLUMNS: &[ExpectedColumn] = &[
    column("workspace_id", "TEXT", true, 1),
    column("ticket_id", "TEXT", true, 2),
    column("ordinal", "INTEGER", true, 3),
    column("repository_key", "TEXT", true, 0),
    column("ref_selector", "TEXT", false, 0),
    column("access", "TEXT", true, 0),
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

    let result = migrate_sqlite_ticket_schema_in_transaction(connection);
    match result {
        Ok(()) => connection.execute_batch("COMMIT").map_err(sqlite_err),
        Err(error) => {
            let _ = connection.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

/// Startup-only historical composition targets retained by Workspace migrations.
/// These are frozen component boundaries, not selectable request schemas or a
/// way to restore legacy execution authority. Current stores must use the latest
/// composer and request paths must use read-only verification instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoricalTicketSchemaTarget {
    /// Two-state cases plus tags; no owning assignment tracking yet.
    Cases10,
    /// Owning assignment tracking, before canonical posts.
    Tracking11,
    /// Canonical posts, before content-change facts.
    Posts12,
    /// Content-change facts, before Subject responsibility constraints.
    Content13,
    /// Subject responsibility, before Service outbox facts.
    Subjects14,
}
impl HistoricalTicketSchemaTarget {
    fn shape(self) -> TicketSchemaShape {
        match self {
            Self::Cases10 => TicketSchemaShape::Composed10,
            Self::Tracking11 => TicketSchemaShape::Tracking11,
            Self::Posts12 => TicketSchemaShape::Posts12,
            Self::Content13 => TicketSchemaShape::ContentChanges13,
            Self::Subjects14 => TicketSchemaShape::Subjects14,
        }
    }
}

/// Compose one explicit retained host-migration boundary in the host's startup
/// transaction. This is NOT a request-path API or a downgrade/repair operation.
///
/// A recognized older preimage is verified before composition. Applied receipts
/// are retained; only missing migration receipts are appended. If the stored
/// version is already at or above the target, its ACTUAL version and complete
/// shape are verified without downgrading, relabelling, or replaying old steps.
/// Unknown history and weakened constraints fail closed. The component savepoint
/// rolls back failure without committing or discarding the caller's other work.
pub fn migrate_historical_sqlite_ticket_schema_in_transaction(
    connection: &Connection,
    target: HistoricalTicketSchemaTarget,
) -> Result<()> {
    migrate_ticket_schema_to_in_transaction(connection, target.shape())
}

/// Appends owning component migrations through the latest contract without
/// replacing applied receipts. Startup only; the host owns the transaction.
/// A savepoint rolls back this component on failure without committing host work.
/// Already-current schemas are verified, never repaired or backfilled.
pub fn migrate_sqlite_ticket_schema_in_transaction(connection: &Connection) -> Result<()> {
    migrate_ticket_schema_to_in_transaction(connection, TicketSchemaShape::Outbox15)
}

fn migrate_ticket_schema_to_in_transaction(
    connection: &Connection,
    target: TicketSchemaShape,
) -> Result<()> {
    if connection.is_autocommit() {
        return Err(TicketError::Sqlite(
            "Ticket component migration requires a host transaction".into(),
        ));
    }
    connection
        .execute_batch("SAVEPOINT ticket_component_composition")
        .map_err(sqlite_err)?;
    let result = (|| {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS ticket_schema_migrations (
            version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL
        );",
            )
            .map_err(sqlite_err)?;
        verify_table(connection, MIGRATION_TABLE, MIGRATION_COLUMNS, &[], false)?;
        let applied = load_applied_migrations(connection)?;
        if applied.is_empty() {
            connection
                .execute_batch(include_str!("latest_schema.sql"))
                .map_err(sqlite_err)?;
            for version in 11..=target.version() {
                apply_component_migration(connection, version)?;
            }
            connection
                .execute(
                    "INSERT INTO ticket_schema_migrations VALUES (?1, ?2, ?3)",
                    params![
                        target.version(),
                        BASELINE_MIGRATION_NAME,
                        chrono::Utc::now().to_rfc3339()
                    ],
                )
                .map_err(sqlite_err)?;
        } else {
            let mut version = recognized_schema_version(&applied)?;
            if version == 6 {
                // Retained v6 conversion establishes the verified v7 preimage.
                // Preserve its original baseline receipt instead of renumbering it.
                migrate_v6_to_v7(connection)?;
                append_component_receipt(connection, 7)?;
                version = 7;
            }
            let shape = TicketSchemaShape::from_version(version);
            verify_ticket_schema_shape(connection, shape)?;
            if version >= target.version() {
                return Ok(());
            }
            match shape {
                TicketSchemaShape::Legacy7 => {
                    add_tag_schema(connection)?;
                    migrate_legacy_cases(connection)?;
                }
                TicketSchemaShape::Tags8 => migrate_legacy_cases(connection)?,
                TicketSchemaShape::Cases9 => add_tag_schema(connection)?,
                TicketSchemaShape::Composed10
                | TicketSchemaShape::Tracking11
                | TicketSchemaShape::Posts12
                | TicketSchemaShape::ContentChanges13
                | TicketSchemaShape::Subjects14
                | TicketSchemaShape::Outbox15 => {}
            }
            if version < 10 {
                append_component_receipt(connection, 10)?;
                version = 10;
            }
            for next in version + 1..=target.version() {
                apply_component_migration(connection, next)?;
                append_component_receipt(connection, next)?;
            }
        }
        verify_ticket_schema_shape(connection, target)
    })();
    match result {
        Ok(()) => connection
            .execute_batch("RELEASE ticket_component_composition")
            .map_err(sqlite_err),
        Err(error) => {
            let _ = connection.execute_batch(
                "ROLLBACK TO ticket_component_composition; RELEASE ticket_component_composition",
            );
            Err(error)
        }
    }
}

#[derive(Clone, Copy)]
enum TicketSchemaShape {
    Legacy7,
    Tags8,
    Cases9,
    Composed10,
    Tracking11,
    Posts12,
    ContentChanges13,
    Subjects14,
    Outbox15,
}
impl TicketSchemaShape {
    fn from_version(version: i64) -> Self {
        match version {
            7 => Self::Legacy7,
            8 => Self::Tags8,
            9 => Self::Cases9,
            10 => Self::Composed10,
            11 => Self::Tracking11,
            12 => Self::Posts12,
            13 => Self::ContentChanges13,
            14 => Self::Subjects14,
            15 => Self::Outbox15,
            _ => unreachable!(),
        }
    }
    fn version(self) -> i64 {
        match self {
            Self::Legacy7 => 7,
            Self::Tags8 => 8,
            Self::Cases9 => 9,
            Self::Composed10 => 10,
            Self::Tracking11 => 11,
            Self::Posts12 => 12,
            Self::ContentChanges13 => 13,
            Self::Subjects14 => 14,
            Self::Outbox15 => 15,
        }
    }
    fn tags(self) -> bool {
        matches!(
            self,
            Self::Tags8
                | Self::Composed10
                | Self::Tracking11
                | Self::Posts12
                | Self::ContentChanges13
                | Self::Subjects14
                | Self::Outbox15
        )
    }
    fn cases(self) -> bool {
        matches!(
            self,
            Self::Cases9
                | Self::Composed10
                | Self::Tracking11
                | Self::Posts12
                | Self::ContentChanges13
                | Self::Subjects14
                | Self::Outbox15
        )
    }
}

/// Read-only verification for a retained Host composition boundary. The actual
/// supported shape must be at least the requested boundary; later component
/// receipts and their complete constraints are verified without relabelling.
pub fn verify_historical_sqlite_ticket_schema(
    connection: &Connection,
    target: HistoricalTicketSchemaTarget,
) -> Result<()> {
    let version = recognized_schema_version(&load_applied_migrations(connection)?)?;
    if version < target.shape().version() {
        return Err(TicketError::Sqlite(
            "Ticket schema precedes the required Host composition boundary".into(),
        ));
    }
    verify_ticket_schema_shape(connection, TicketSchemaShape::from_version(version))
}

/// Verifies the current Ticket-owned SQLite schema without executing DDL.
pub fn verify_sqlite_ticket_schema(connection: &Connection) -> Result<()> {
    verify_ticket_schema_shape(connection, TicketSchemaShape::Outbox15)
}

/// Retained Workspace schema-66 migration produces the pre-classification v7
/// Ticket shape. Validate that intermediate contract before the normal startup
/// path upgrades it; never require future tag tables inside that old transaction.
pub fn verify_pre_classification_ticket_schema(connection: &Connection) -> Result<()> {
    verify_ticket_schema_shape(connection, TicketSchemaShape::Legacy7)
}

/// Verifies retained six-state component 7 or tags-only component 8, without
/// treating historical workflow values as the current two-state case authority.
pub fn verify_legacy_sqlite_ticket_schema(connection: &Connection) -> Result<()> {
    let applied = load_applied_migrations(connection)?;
    let version = recognized_schema_version(&applied)?;
    if !matches!(version, 7 | 8) {
        return Err(TicketError::Sqlite(
            "legacy Ticket schema requires component 7 or 8".into(),
        ));
    }
    verify_ticket_schema_shape(connection, TicketSchemaShape::from_version(version))
}

fn verify_ticket_schema_shape(connection: &Connection, shape: TicketSchemaShape) -> Result<()> {
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
            let history = recognized_schema_version(&applied).and_then(|version| {
                if version == shape.version() {
                    Ok(())
                } else {
                    Err(TicketError::Sqlite(format!(
                        "Ticket schema migration history must match version {}",
                        shape.version()
                    )))
                }
            });
            if let Err(error) = history {
                push_diagnostic(&mut diagnostics, error.to_string());
            }
        }
        Err(error) => push_diagnostic(&mut diagnostics, error.to_string()),
    }

    for (table, columns, foreign_keys) in [
        ("workspace_tags", TAG_COLUMNS, TICKET_FOREIGN_KEYS),
        (
            "typed_ticket_tags",
            TICKET_TAG_COLUMNS,
            TICKET_TAG_FOREIGN_KEYS,
        ),
        (
            "workspace_tag_operations",
            TAG_OPERATION_COLUMNS,
            TICKET_FOREIGN_KEYS,
        ),
        ("typed_tickets", TICKET_COLUMNS, TICKET_FOREIGN_KEYS),
        ("typed_ticket_targets", TARGET_COLUMNS, CHILD_FOREIGN_KEYS),
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
        if !shape.tags()
            && matches!(
                table,
                "workspace_tags" | "typed_ticket_tags" | "workspace_tag_operations"
            )
        {
            continue;
        }
        let mut columns = columns.to_vec();
        if matches!(
            shape,
            TicketSchemaShape::Posts12
                | TicketSchemaShape::ContentChanges13
                | TicketSchemaShape::Subjects14
                | TicketSchemaShape::Outbox15
        ) {
            if table == "typed_tickets" {
                columns.retain(|c| c.name != "body");
                columns.push(column("created_by", "TEXT", false, 0));
            }
            if table == "typed_ticket_events" {
                columns.extend([
                    column("event_ref", "TEXT", true, 0),
                    column("edited_at", "TEXT", false, 0),
                    column("edited_by", "TEXT", false, 0),
                    column("archived", "INTEGER", true, 0),
                ]);
            }
        }
        collect_table_diagnostics(connection, table, &columns, foreign_keys, &mut diagnostics);
    }

    if shape.tags() {
        collect_index_diagnostics(
            connection,
            "workspace_tags",
            Some("workspace_tags_active_name"),
            true,
            &["workspace_id", "normalized_name"],
            &mut diagnostics,
        );
        collect_index_diagnostics(
            connection,
            "typed_ticket_tags",
            Some("typed_ticket_tags_workspace_tag"),
            false,
            &["workspace_id", "tag_id", "ticket_id"],
            &mut diagnostics,
        );
        let tag_name_index_sql: Option<String> = connection.query_row("SELECT sql FROM sqlite_master WHERE type='index' AND name='workspace_tags_active_name'", [], |row| row.get(0)).optional().map_err(sqlite_err)?;
        if !tag_name_index_sql.is_some_and(|sql| {
            sql.split_whitespace()
                .collect::<String>()
                .to_lowercase()
                .ends_with("wheredeleted=0")
        }) {
            push_diagnostic(
                &mut diagnostics,
                "workspace_tags_active_name must be partial WHERE deleted = 0".into(),
            );
        }
    }
    collect_index_diagnostics(
        connection,
        "typed_ticket_targets",
        None,
        true,
        &["workspace_id", "ticket_id", "repository_key"],
        &mut diagnostics,
    );
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

    if shape.cases() {
        for (table, columns) in [
            (
                "typed_ticket_case_metadata",
                vec![
                    column("workspace_id", "TEXT", true, 1),
                    column("ticket_id", "TEXT", true, 2),
                    column("closure_operation_key", "TEXT", false, 0),
                ],
            ),
            (
                "typed_ticket_case_history",
                vec![
                    column("workspace_id", "TEXT", true, 1),
                    column("ticket_id", "TEXT", true, 2),
                    column("ordinal", "INTEGER", true, 0),
                    column("operation_key", "TEXT", true, 3),
                    column("request_fingerprint", "TEXT", true, 0),
                    column("judgment_json", "TEXT", true, 0),
                ],
            ),
            (
                "typed_ticket_legacy_workflow_snapshot",
                vec![
                    column("workspace_id", "TEXT", true, 1),
                    column("ticket_id", "TEXT", true, 2),
                    column("snapshot_json", "TEXT", true, 0),
                ],
            ),
        ] {
            if let Err(error) = verify_table(connection, table, &columns, CHILD_FOREIGN_KEYS, false)
            {
                push_diagnostic(&mut diagnostics, error.to_string());
            }
        }
        collect_index_diagnostics(
            connection,
            "typed_ticket_case_history",
            None,
            true,
            &["workspace_id", "ticket_id", "ordinal"],
            &mut diagnostics,
        );
        for name in [
            "typed_ticket_case_history_no_update",
            "typed_ticket_case_history_no_delete",
            "typed_ticket_legacy_workflow_no_update",
            "typed_ticket_legacy_workflow_no_delete",
            "typed_ticket_case_state_insert",
            "typed_ticket_case_state_update",
        ] {
            let sql: Option<String> = connection
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()
                .map_err(sqlite_err)?;
            let canonical = include_str!("case_schema.sql")
                .split("CREATE TRIGGER ")
                .skip(1)
                .find(|ddl| ddl.starts_with(name))
                .expect("canonical case trigger")
                .split_once("END;")
                .expect("trigger terminator")
                .0;
            let canonical = format!("CREATE TRIGGER {canonical}END");
            let normalize = |sql: &str| {
                sql.trim_end_matches(';')
                    .split_whitespace()
                    .collect::<String>()
                    .to_lowercase()
            };
            match sql {
                None => push_diagnostic(
                    &mut diagnostics,
                    format!("missing Ticket case trigger {name}"),
                ),
                Some(sql) if normalize(&sql) != normalize(&canonical) => push_diagnostic(
                    &mut diagnostics,
                    format!("invalid Ticket case trigger {name}"),
                ),
                Some(_) => {}
            }
        }
    } else {
        for table in [
            "typed_ticket_case_metadata",
            "typed_ticket_case_history",
            "typed_ticket_legacy_workflow_snapshot",
        ] {
            collect_absent_table_diagnostic(connection, table, &mut diagnostics);
        }
    }
    if !shape.tags() {
        for table in [
            "workspace_tags",
            "typed_ticket_tags",
            "workspace_tag_operations",
        ] {
            collect_absent_table_diagnostic(connection, table, &mut diagnostics);
        }
    }
    let invalid_states: i64 = connection.query_row(
        if shape.cases() {
            "SELECT COUNT(*) FROM typed_tickets WHERE workflow_state NOT IN ('open','closed')"
        } else {
            "SELECT COUNT(*) FROM typed_tickets WHERE workflow_state NOT IN ('planning','ready','queued','inprogress','done','closed')"
        }, [], |row| row.get(0)).map_err(sqlite_err)?;
    if invalid_states != 0 {
        push_diagnostic(
            &mut diagnostics,
            format!(
                "component {} has {invalid_states} unsupported Ticket states",
                shape.version()
            ),
        );
    }
    for table in OWNED_TABLES {
        if !shape.tags()
            && matches!(
                *table,
                "workspace_tags" | "typed_ticket_tags" | "workspace_tag_operations"
            )
        {
            continue;
        }
        if !shape.cases()
            && matches!(
                *table,
                "typed_ticket_case_metadata"
                    | "typed_ticket_case_history"
                    | "typed_ticket_legacy_workflow_snapshot"
            )
        {
            continue;
        }
        collect_foreign_key_check_diagnostics(connection, table, &mut diagnostics);
    }

    if matches!(
        shape,
        TicketSchemaShape::Tracking11
            | TicketSchemaShape::Posts12
            | TicketSchemaShape::ContentChanges13
            | TicketSchemaShape::Subjects14
            | TicketSchemaShape::Outbox15
    ) {
        if let Err(error) = crate::assignment_tracking::verify_assignment_tracking_schema(
            connection,
            matches!(
                shape,
                TicketSchemaShape::Subjects14 | TicketSchemaShape::Outbox15
            ),
        ) {
            push_diagnostic(&mut diagnostics, error.to_string());
        }
    }
    if matches!(
        shape,
        TicketSchemaShape::Posts12
            | TicketSchemaShape::ContentChanges13
            | TicketSchemaShape::Subjects14
            | TicketSchemaShape::Outbox15
    ) {
        collect_index_diagnostics(
            connection,
            "typed_ticket_events",
            Some("typed_ticket_events_ref"),
            true,
            &["workspace_id", "ticket_id", "event_ref"],
            &mut diagnostics,
        );
    }
    if matches!(
        shape,
        TicketSchemaShape::ContentChanges13
            | TicketSchemaShape::Subjects14
            | TicketSchemaShape::Outbox15
    ) {
        collect_content_change_diagnostics(connection, &mut diagnostics)?;
    }
    if matches!(shape, TicketSchemaShape::Outbox15) {
        collect_service_outbox_diagnostics(connection, &mut diagnostics)?;
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

// Both fresh and retained composition use this single ordered step dispatcher.
fn apply_component_migration(connection: &Connection, version: i64) -> Result<()> {
    match version {
        11 => crate::assignment_tracking::migrate_assignment_tracking(connection),
        12 => migrate_posts(connection),
        13 => migrate_content_changes(connection),
        14 => crate::subject_assignment::migrate(connection),
        15 => connection
            .execute_batch(crate::service_outbox::OUTBOX_SCHEMA_SQL)
            .map_err(sqlite_err),
        _ => unreachable!("bounded component migration"),
    }
}

fn component_migration_name(version: i64) -> &'static str {
    match version {
        7 => "Ordered Ticket targets",
        10 => "Ticket case and tag composition",
        11 => "Ticket-owned assignment tracking",
        12 => "Canonical Ticket posts",
        13 => "Ticket content change facts",
        14 => "Subject Ticket responsibility",
        15 => "Ticket Service transactional outbox",
        _ => unreachable!("bounded component receipt"),
    }
}

fn append_component_receipt(connection: &Connection, version: i64) -> Result<()> {
    connection
        .execute(
            "INSERT INTO ticket_schema_migrations VALUES (?1, ?2, ?3)",
            params![
                version,
                component_migration_name(version),
                chrono::Utc::now().to_rfc3339()
            ],
        )
        .map_err(sqlite_err)?;
    Ok(())
}

fn migrate_content_changes(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(include_str!("content_changes.sql"))
        .map_err(sqlite_err)
}

fn collect_content_change_diagnostics(
    connection: &Connection,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    const TABLE: &str = "typed_ticket_content_changes";
    collect_table_diagnostics(
        connection,
        TABLE,
        &[
            column("change_id", "TEXT", true, 1),
            column("workspace_id", "TEXT", true, 0),
            column("ticket_id", "TEXT", true, 0),
            column("kind", "TEXT", true, 0),
            column("actor", "TEXT", false, 0),
            column("at", "TEXT", true, 0),
            column("title_changed", "INTEGER", true, 0),
            column("body_changed", "INTEGER", true, 0),
            column("targets_changed", "INTEGER", true, 0),
            column("source_event_ref", "TEXT", false, 0),
        ],
        CHILD_FOREIGN_KEYS,
        diagnostics,
    );
    // These new objects are wholly Ticket-owned. Compare their complete definitions,
    // including CHECK predicates and the composite FK, not just column affinities.
    // Additional host triggers are independent and remain permitted.
    let ddl = include_str!("content_changes.sql");
    let table_sql = format!(
        "CREATE TABLE {}",
        ddl.split_once("CREATE TABLE ")
            .expect("table DDL")
            .1
            .split_once(";\n")
            .expect("table terminator")
            .0
    );
    let trigger_sql = format!(
        "CREATE TRIGGER {}",
        ddl.split_once("CREATE TRIGGER ")
            .expect("trigger DDL")
            .1
            .trim()
    );
    for (kind, name, expected) in [
        ("table", TABLE, table_sql),
        (
            "trigger",
            "typed_ticket_content_changes_no_update",
            trigger_sql,
        ),
    ] {
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = ?1 AND name = ?2",
                params![kind, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;
        let normalize = |sql: &str| {
            sql.trim()
                .trim_end_matches(';')
                .split_whitespace()
                .collect::<String>()
        };
        if actual.as_deref().map(normalize) != Some(normalize(&expected)) {
            push_diagnostic(
                diagnostics,
                format!("invalid Ticket content change {kind} {name}"),
            );
        }
    }
    collect_foreign_key_check_diagnostics(connection, TABLE, diagnostics);
    Ok(())
}

fn collect_service_outbox_diagnostics(
    connection: &Connection,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    // Check the complete owned DDL, including AUTOINCREMENT, bounds, uniqueness
    // and absence of a cascading FK. Host-owned additional triggers are allowed.
    for expected in crate::service_outbox::OUTBOX_SCHEMA_SQL.split(';') {
        let expected = expected.trim();
        if expected.is_empty() {
            continue;
        }
        let words: Vec<_> = expected.split_whitespace().collect();
        let kind = words[1].to_ascii_lowercase();
        let name = words[2];
        let actual: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type=?1 AND name=?2",
                params![kind, name],
                |row| row.get(0),
            )
            .optional()
            .map_err(sqlite_err)?;
        let normalize = |sql: &str| sql.split_whitespace().collect::<String>();
        if actual.as_deref().map(normalize) != Some(normalize(expected)) {
            push_diagnostic(
                diagnostics,
                format!("invalid Ticket Service outbox {kind} {name}"),
            );
        }
    }
    Ok(())
}

fn migrate_posts(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(include_str!("post_migration.sql"))
        .map_err(sqlite_err)
}

/// Retained 7/8 case migration: leave tags and historical receipts untouched.
fn migrate_legacy_cases(connection: &Connection) -> Result<()> {
    let snapshots = {
        let mut stmt = connection.prepare("SELECT workspace_id, ticket_id, workflow_state, workflow_state_explicit, status, queued_by, queued_at, resolution FROM typed_tickets").map_err(sqlite_err)?;
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                crate::TicketLegacyWorkflowSnapshot {
                    workflow_state: row.get(2)?,
                    workflow_state_explicit: row.get::<_, i64>(3)? != 0,
                    status: row.get(4)?,
                    queued_by: row.get(5)?,
                    queued_at: row.get(6)?,
                    resolution: row.get(7)?,
                },
            ))
        })
        .map_err(sqlite_err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(sqlite_err)?
    };
    // Validate old authority before any writes; unknown states are not silently repaired.
    for (_, _, snapshot) in &snapshots {
        if !matches!(
            snapshot.workflow_state.as_str(),
            "planning" | "ready" | "queued" | "inprogress" | "done" | "closed"
        ) {
            return Err(TicketError::Sqlite(format!(
                "unsupported legacy Ticket workflow state {:?}",
                snapshot.workflow_state
            )));
        }
    }
    // State guards are installed only after all six-state rows have been converted.
    for (workspace_id, ticket_id, snapshot) in &snapshots {
        let state = if matches!(snapshot.workflow_state.as_str(), "done" | "closed") {
            "closed"
        } else {
            "open"
        };
        connection.execute("UPDATE typed_tickets SET workflow_state = ?3, status = ?3 WHERE workspace_id = ?1 AND ticket_id = ?2", params![workspace_id, ticket_id, state]).map_err(sqlite_err)?;
    }
    connection
        .execute_batch(include_str!("case_schema.sql"))
        .map_err(sqlite_err)?;
    for (workspace_id, ticket_id, snapshot) in snapshots {
        connection.execute("INSERT INTO typed_ticket_legacy_workflow_snapshot (workspace_id, ticket_id, snapshot_json) VALUES (?1, ?2, ?3)", params![workspace_id, ticket_id, serde_json::to_string(&snapshot).map_err(sqlite_err)?]).map_err(sqlite_err)?;
        if matches!(snapshot.workflow_state.as_str(), "done" | "closed") {
            let key = format!("migration-v10:{ticket_id}");
            let judgment = crate::TicketCaseHistoryEntry {
                operation_key: key.clone(),
                previous_closure_operation_key: None,
                // No typed evidence in schema7 establishes Completed. `done` is not success.
                action: crate::TicketCaseAction::Close {
                    reason: crate::TicketClosureReason::Unknown,
                },
                explanation: snapshot
                    .resolution
                    .clone()
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| {
                        format!(
                            "Migrated legacy {} state; closure reason is unknown.",
                            snapshot.workflow_state
                        )
                    }),
                references: Vec::new(),
                author: "ticket-schema-migration-v10".into(),
                at: chrono::Utc::now().to_rfc3339(),
            };
            connection.execute("INSERT INTO typed_ticket_case_history (workspace_id, ticket_id, ordinal, operation_key, request_fingerprint, judgment_json) VALUES (?1, ?2, 0, ?3, 'migration-v10', ?4)", params![workspace_id, ticket_id, key, serde_json::to_string(&judgment).map_err(sqlite_err)?]).map_err(sqlite_err)?;
            connection.execute("INSERT INTO typed_ticket_case_metadata (workspace_id, ticket_id, closure_operation_key) VALUES (?1, ?2, ?3)", params![workspace_id, ticket_id, key]).map_err(sqlite_err)?;
        }
    }
    Ok(())
}
fn add_tag_schema(connection: &Connection) -> Result<()> {
    // Use the exact canonical DDL rather than duplicating migration definitions.
    let (_, tag_schema) = include_str!("latest_schema.sql")
        .split_once("-- Workspace tag foundation (v8): shared by fresh schema and v7 upgrade.")
        .expect("canonical tag schema marker");
    connection.execute_batch(tag_schema).map_err(sqlite_err)
}

fn migrate_v6_to_v7(connection: &Connection) -> Result<()> {
    connection
        .execute_batch(
            "CREATE TABLE typed_ticket_targets (
                workspace_id TEXT NOT NULL,
                ticket_id TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                repository_key TEXT NOT NULL,
                ref_selector TEXT,
                access TEXT NOT NULL CHECK (access IN ('read_only', 'read_write')),
                PRIMARY KEY (workspace_id, ticket_id, ordinal),
                UNIQUE (workspace_id, ticket_id, repository_key),
                FOREIGN KEY (workspace_id, ticket_id)
                    REFERENCES typed_tickets(workspace_id, ticket_id) ON DELETE CASCADE
            );",
        )
        .map_err(sqlite_err)?;

    let legacy_targets = {
        let mut statement = connection
            .prepare(
                "SELECT workspace_id, ticket_id, repository_id, ref_selector
                 FROM typed_tickets
                 WHERE repository_id IS NOT NULL OR ref_selector IS NOT NULL
                 ORDER BY workspace_id, ticket_id",
            )
            .map_err(sqlite_err)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?
    };
    for (workspace_id, ticket_id, repository_key, ref_selector) in legacy_targets {
        let repository_key = repository_key.ok_or_else(|| {
            TicketError::Sqlite(format!(
                "legacy Ticket {ticket_id:?} in Workspace {workspace_id:?} has ref_selector without repository_id"
            ))
        })?;
        connection
            .execute(
                "INSERT INTO typed_ticket_targets
                 (workspace_id, ticket_id, ordinal, repository_key, ref_selector, access)
                 VALUES (?1, ?2, 0, ?3, ?4, 'read_write')",
                params![workspace_id, ticket_id, repository_key, ref_selector],
            )
            .map_err(sqlite_err)?;
    }

    let legacy_event_targets = {
        let mut statement = connection
            .prepare(
                "SELECT repository.workspace_id, repository.ticket_id, repository.event_index,
                        repository.value, selector.value
                 FROM typed_ticket_event_attributes AS repository
                 JOIN typed_ticket_event_attributes AS selector
                   ON selector.workspace_id = repository.workspace_id
                  AND selector.ticket_id = repository.ticket_id
                  AND selector.event_index = repository.event_index
                  AND selector.key = 'ref_selector'
                 WHERE repository.key = 'repository_id'
                 ORDER BY repository.workspace_id, repository.ticket_id, repository.event_index",
            )
            .map_err(sqlite_err)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)?
    };
    for (workspace_id, ticket_id, event_index, repository_key, ref_selector) in legacy_event_targets
    {
        let targets = serde_json::to_string(&[serde_json::json!({
            "repository_key": repository_key,
            "ref_selector": ref_selector,
            "access": "read_write",
        })])
        .map_err(sqlite_err)?;
        connection
            .execute(
                "INSERT OR IGNORE INTO typed_ticket_event_attributes
                 (workspace_id, ticket_id, event_index, key, value)
                 VALUES (?1, ?2, ?3, 'targets', ?4)",
                params![workspace_id, ticket_id, event_index, targets],
            )
            .map_err(sqlite_err)?;
        connection
            .execute(
                "INSERT OR IGNORE INTO typed_ticket_event_attributes
                 (workspace_id, ticket_id, event_index, key, value)
                 SELECT ?1, ?2, ?3, 'fingerprint_version', '1'
                 WHERE EXISTS (
                    SELECT 1 FROM typed_ticket_event_attributes
                    WHERE workspace_id = ?1 AND ticket_id = ?2 AND event_index = ?3
                      AND key = 'request_fingerprint'
                 )",
                params![workspace_id, ticket_id, event_index],
            )
            .map_err(sqlite_err)?;
    }
    connection
        .execute(
            "DELETE FROM typed_ticket_event_attributes AS legacy
             WHERE legacy.key IN ('repository_id', 'ref_selector')
               AND EXISTS (
                    SELECT 1 FROM typed_ticket_event_attributes AS canonical
                    WHERE canonical.workspace_id = legacy.workspace_id
                      AND canonical.ticket_id = legacy.ticket_id
                      AND canonical.event_index = legacy.event_index
                      AND canonical.key = 'targets'
               )",
            [],
        )
        .map_err(sqlite_err)?;

    connection
        .execute_batch(
            "ALTER TABLE typed_tickets DROP COLUMN repository_id;
             ALTER TABLE typed_tickets DROP COLUMN ref_selector;",
        )
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

fn recognized_schema_version(applied: &BTreeMap<i64, String>) -> Result<i64> {
    if let Some((&baseline, name)) = applied.first_key_value() {
        if (6..=LATEST_SQLITE_TICKET_SCHEMA_VERSION).contains(&baseline)
            && name == BASELINE_MIGRATION_NAME
        {
            let mut expected = BTreeMap::from([(baseline, name.clone())]);
            if applied == &expected {
                return Ok(baseline);
            }
            // Versions 8 and 9 were independent baseline generations, not
            // ordered upgrades. Both compose at10; v6 additionally needs v7.
            for version in [7, 10, 11, 12, 13, 14, 15] {
                if version <= baseline || (version == 7 && baseline != 6) {
                    continue;
                }
                expected.insert(version, component_migration_name(version).to_owned());
                if applied == &expected {
                    return Ok(version);
                }
            }
        }
    }
    Err(TicketError::Sqlite(format!(
        "Ticket schema migration history must contain a canonical baseline and ordered migrations through {LATEST_SQLITE_TICKET_SCHEMA_VERSION}"
    )))
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
    fn migrates_single_target_rows_to_ordered_read_write_targets() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!("../tests/fixtures/schema7.sql"))
            .unwrap();
        connection
            .execute_batch(
                "DROP TABLE typed_ticket_targets;
                 ALTER TABLE typed_tickets ADD COLUMN repository_id TEXT;
                 ALTER TABLE typed_tickets ADD COLUMN ref_selector TEXT;
                 CREATE TABLE ticket_schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at TEXT NOT NULL
                 );
                 INSERT INTO ticket_schema_migrations (version, name, applied_at)
                 VALUES (6, 'ticket schema baseline', '2026-09-23T00:00:00Z');
                 INSERT INTO typed_tickets (
                    workspace_id, ticket_id, slug, title, status, kind, priority, body,
                    workflow_state, workflow_state_explicit, repository_id, ref_selector
                 ) VALUES (
                    'workspace-1', 'ticket-1', 'ticket-1', 'Migrated', 'open', 'task', 'P2',
                    'body', 'ready', 1, 'main', 'develop'
                 );
                 INSERT INTO typed_ticket_events (
                    workspace_id, ticket_id, event_index, kind, body
                 ) VALUES (
                    'workspace-1', 'ticket-1', 0, 'state_changed', 'ready'
                 );
                 INSERT INTO typed_ticket_event_attributes (
                    workspace_id, ticket_id, event_index, key, value
                 ) VALUES
                    ('workspace-1', 'ticket-1', 0, 'repository_id', 'main'),
                    ('workspace-1', 'ticket-1', 0, 'ref_selector', 'develop'),
                    ('workspace-1', 'ticket-1', 0, 'request_fingerprint', 'legacy');",
            )
            .unwrap();

        migrate_sqlite_ticket_schema(&connection).unwrap();
        verify_sqlite_ticket_schema(&connection).unwrap();

        let target = connection
            .query_row(
                "SELECT ordinal, repository_key, ref_selector, access
                 FROM typed_ticket_targets
                 WHERE workspace_id = 'workspace-1' AND ticket_id = 'ticket-1'",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            target,
            (
                0,
                "main".to_owned(),
                Some("develop".to_owned()),
                "read_write".to_owned()
            )
        );
        let columns = load_columns(&connection, "typed_tickets").unwrap();
        assert!(
            columns
                .iter()
                .all(|column| { column.name != "repository_id" && column.name != "ref_selector" })
        );
        let encoded_targets: String = connection
            .query_row(
                "SELECT value FROM typed_ticket_event_attributes
                 WHERE workspace_id = 'workspace-1' AND ticket_id = 'ticket-1'
                   AND event_index = 0 AND key = 'targets'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&encoded_targets).unwrap(),
            serde_json::json!([{
                "repository_key": "main",
                "ref_selector": "develop",
                "access": "read_write"
            }])
        );
        let migrated_attributes = {
            let mut statement = connection
                .prepare(
                    "SELECT key, value FROM typed_ticket_event_attributes
                     WHERE workspace_id = 'workspace-1' AND ticket_id = 'ticket-1'
                       AND event_index = 0 ORDER BY key",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .unwrap()
                .collect::<std::result::Result<BTreeMap<_, _>, _>>()
                .unwrap()
        };
        assert_eq!(
            migrated_attributes
                .get("fingerprint_version")
                .map(String::as_str),
            Some("1")
        );
        assert!(!migrated_attributes.contains_key("repository_id"));
        assert!(!migrated_attributes.contains_key("ref_selector"));
        assert_eq!(
            load_applied_migrations(&connection).unwrap(),
            BTreeMap::from([
                (6, BASELINE_MIGRATION_NAME.to_owned()),
                (7, "Ordered Ticket targets".to_owned()),
                (10, "Ticket case and tag composition".to_owned()),
                (11, "Ticket-owned assignment tracking".to_owned()),
                (12, "Canonical Ticket posts".to_owned()),
                (13, "Ticket content change facts".to_owned()),
                (14, "Subject Ticket responsibility".to_owned()),
                (15, "Ticket Service transactional outbox".to_owned())
            ])
        );
    }

    #[test]
    fn migrates_v7_to_identical_fresh_schema_without_touching_legacy_labels() {
        let upgraded = Connection::open_in_memory().unwrap();
        let legacy_sql = include_str!("../tests/fixtures/schema7.sql");
        upgraded.execute_batch(legacy_sql).unwrap();
        upgraded.execute_batch(
            "CREATE TABLE ticket_schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL);
             INSERT INTO ticket_schema_migrations VALUES (7, 'ticket schema baseline', 'old');
             INSERT INTO typed_tickets (workspace_id,ticket_id,slug,title,status,kind,priority,body,workflow_state,workflow_state_explicit)
                VALUES ('ws','ticket','ticket','Title','open','task','P2','body','ready',1);
             INSERT INTO typed_ticket_labels VALUES ('ws','ticket',0,' Legacy Label ');"
        ).unwrap();
        migrate_sqlite_ticket_schema(&upgraded).unwrap();
        verify_sqlite_ticket_schema(&upgraded).unwrap();
        let fresh = Connection::open_in_memory().unwrap();
        migrate_sqlite_ticket_schema(&fresh).unwrap();
        let schema = |conn: &Connection| {
            let mut statement = conn.prepare("SELECT type,name,tbl_name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name").unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?
                            .map(|sql| sql.split_whitespace().collect::<String>()),
                    ))
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(schema(&upgraded), schema(&fresh));
        let label: String = upgraded
            .query_row("SELECT label FROM typed_ticket_labels", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(label, " Legacy Label ");
        let tags: i64 = upgraded
            .query_row("SELECT COUNT(*) FROM workspace_tags", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tags, 0); // Legacy labels are never promoted to catalog tags.
        migrate_sqlite_ticket_schema(&upgraded).unwrap();
        assert_eq!(schema(&upgraded), schema(&fresh));
    }

    #[test]
    fn tag_schema_verification_detects_drift_without_repair() {
        for (drift, expected) in [
            ("DROP TABLE typed_ticket_tags", "typed_ticket_tags"),
            (
                "ALTER TABLE workspace_tags RENAME COLUMN last_operation_key TO wrong_observed_key",
                "last_operation_key",
            ),
            (
                "ALTER TABLE workspace_tag_operations DROP COLUMN result_json",
                "result_json",
            ),
            (
                "DROP INDEX typed_ticket_tags_workspace_tag",
                "typed_ticket_tags_workspace_tag",
            ),
            (
                "DROP INDEX workspace_tags_active_name; CREATE UNIQUE INDEX workspace_tags_active_name ON workspace_tags(workspace_id, normalized_name)",
                "partial WHERE deleted = 0",
            ),
            (
                "DROP TABLE typed_ticket_tags; CREATE TABLE typed_ticket_tags (workspace_id TEXT NOT NULL,ticket_id TEXT NOT NULL,tag_id TEXT NOT NULL, PRIMARY KEY(workspace_id,ticket_id,tag_id)); CREATE INDEX typed_ticket_tags_workspace_tag ON typed_ticket_tags(workspace_id,tag_id,ticket_id)",
                "missing foreign key",
            ),
        ] {
            let connection = Connection::open_in_memory().unwrap();
            migrate_sqlite_ticket_schema(&connection).unwrap();
            connection.execute_batch(drift).unwrap();
            let error = verify_sqlite_ticket_schema(&connection).unwrap_err();
            assert!(
                error.to_string().contains(expected),
                "after {drift}: {error}"
            );
            assert!(migrate_sqlite_ticket_schema(&connection).is_err());
        }
    }

    #[test]
    fn tag_schema_rejects_cross_workspace_links_and_preserves_tombstones() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .unwrap();
        migrate_sqlite_ticket_schema(&connection).unwrap();
        connection.execute_batch(
            "INSERT INTO typed_tickets (workspace_id,ticket_id,slug,title,status,kind,priority,workflow_state,workflow_state_explicit)
             VALUES ('one','ticket','ticket','Title','open','task','P2','open',1);
             INSERT INTO workspace_tags VALUES ('two','tag','Name','name','create',0);"
        ).unwrap();
        assert!(
            connection
                .execute(
                    "INSERT INTO typed_ticket_tags VALUES ('one','ticket','tag')",
                    []
                )
                .is_err()
        );
        connection
            .execute(
                "INSERT INTO workspace_tags VALUES ('one','tag','Name','name','create',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO typed_ticket_tags VALUES ('one','ticket','tag')",
                [],
            )
            .unwrap();
        assert!(
            connection
                .execute("DELETE FROM workspace_tags WHERE workspace_id='one'", [])
                .is_err()
        );
        verify_sqlite_ticket_schema(&connection).unwrap();
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
            "migration history must contain a canonical baseline and ordered migrations through 15"
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
            "migration history must contain a canonical baseline and ordered migrations through 15"
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
