//! Component 14 owns Subject assignment table constraints and guards. It never
//! maps a legacy agent to a Subject, starts work, or rewrites historical receipts.
//! The host separately owns authenticated live Subject bindings, delivery and startup fences.
use rusqlite::{Connection, OptionalExtension, params};

const PRINCIPAL_TABLES: [&str; 3] = [
    "ticket_assignment_operations",
    "ticket_current_worker_assignments",
    "ticket_worker_assignments",
];
const OLD_KINDS: &str = "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent'))";
const SUBJECT_KINDS: &str =
    "CHECK(principal_kind IN ('user', 'worker', 'workspace_agent', 'subject'))";
pub(crate) const SUBJECT_GUARDS: &str = include_str!("subject_assignment_schema.sql");
const LEGACY_DIAGNOSTIC: &str = "Legacy Orchestrator identity retained without a trustworthy historical Subject mapping; nonoperational, explicit authorized Subject assignment required. Saved history and receipts do not grant execution authority.";

#[derive(Debug, thiserror::Error)]
enum MigrationError {
    #[error("{0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("{0}")]
    Contract(String),
}
type Result<T> = std::result::Result<T, MigrationError>;

/// Called only by the owning startup migration, within its savepoint. The caller
/// rolls back on error. No independent ledger, transaction, or authority fallback.
pub(crate) fn migrate(conn: &Connection) -> crate::Result<()> {
    let result = (|| -> Result<()> {
        if conn.is_autocommit() {
            return Err(MigrationError::Contract(
                "Subject assignment migration requires a caller-owned transaction".into(),
            ));
        }
        widen_principal_checks(conn)?;
        conn.execute_batch(SUBJECT_GUARDS)?;
        conn.execute(
            "UPDATE ticket_worker_assignments
            SET tracking_migration_diagnostic = CASE
              WHEN tracking_migration_diagnostic IS NULL OR tracking_migration_diagnostic=''
                THEN ?1 ELSE tracking_migration_diagnostic || char(10) || ?1 END
            WHERE role='orchestrator' AND principal_kind!='subject'",
            [LEGACY_DIAGNOSTIC],
        )?;
        verify_inner(conn)?;
        verify_integrity(conn)
    })();
    result.map_err(super::sqlite_err)
}

pub(crate) fn verify(conn: &Connection) -> crate::Result<()> {
    verify_inner(conn).map_err(super::sqlite_err)
}

fn verify_inner(conn: &Connection) -> Result<()> {
    require_schema_writes_closed(conn)?;
    let canonical = known_table_shape(conn, true)?;
    canonical.execute_batch(SUBJECT_GUARDS)?;
    let mut statement =
        canonical.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' ORDER BY name")?;
    for name in statement.query_map([], |row| row.get::<_, String>(0))? {
        require_same_sql(conn, &canonical, "trigger", &name?)?;
    }
    Ok(())
}

fn require_schema_writes_closed(conn: &Connection) -> Result<()> {
    for setting in ["writable_schema", "ignore_check_constraints"] {
        if conn.pragma_query_value(None, setting, |row| row.get::<_, bool>(0))? {
            return Err(MigrationError::Contract(format!(
                "Subject assignment schema requires {setting} OFF"
            )));
        }
    }
    Ok(())
}

fn schema_sql(conn: &Connection, kind: &str, name: &str) -> Result<String> {
    conn.query_row(
        "SELECT sql FROM main.sqlite_schema WHERE type=?1 AND name=?2",
        params![kind, name],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| MigrationError::Contract(format!("Missing Subject assignment {kind} {name}")))
}

fn require_same_sql(
    conn: &Connection,
    canonical: &Connection,
    kind: &str,
    name: &str,
) -> Result<()> {
    if normalize_sql(&schema_sql(conn, kind, name)?)
        != normalize_sql(&schema_sql(canonical, kind, name)?)
    {
        return Err(MigrationError::Contract(format!(
            "Subject assignment {kind} {name} differs from canonical schema"
        )));
    }
    Ok(())
}

/// Token boundaries and quoted literal bytes are significant. Only whitespace,
/// unquoted ASCII case and SQLite's ALTER-generated quoting of these three known
/// table identifiers are normalized. In particular, do not globally strip quotes,
/// comments, IF NOT EXISTS, whitespace in literals, or merge adjacent SQL tokens.
fn normalize_sql(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_whitespace() {
            continue;
        }
        if matches!(ch, '\'' | '"' | '`' | '[') {
            let end = if ch == '[' { ']' } else { ch };
            let mut token = String::from(ch);
            while let Some(next) = chars.next() {
                token.push(next);
                if next == end {
                    if chars.peek() == Some(&end) && end != ']' {
                        token.push(chars.next().expect("peeked quote"));
                    } else {
                        break;
                    }
                }
            }
            if ch == '"'
                && PRINCIPAL_TABLES
                    .iter()
                    .any(|table| token == format!("\"{table}\""))
            {
                token = token[1..token.len() - 1].to_owned();
            }
            tokens.push(token);
        } else if ch.is_alphanumeric() || matches!(ch, '_' | '$') {
            let mut token = ch.to_string();
            while chars
                .peek()
                .is_some_and(|next| next.is_alphanumeric() || matches!(*next, '_' | '$'))
            {
                token.push(chars.next().expect("peeked token"));
            }
            tokens.push(token.to_ascii_lowercase());
        } else {
            tokens.push(ch.to_string());
        }
    }
    tokens
}

/// Reconstruct both explicitly supported historical generations from the immutable
/// Ticket-owned core and tracking DDL. Only Workspace-parent FK presence differs.
fn canonical_tables(subject: bool, workspace_parent: bool) -> Result<Connection> {
    let canonical = Connection::open_in_memory()?;
    let historical = super::TICKET_ASSIGNMENT_SCHEMA_SQL;
    let mut sql = if workspace_parent {
        historical.to_owned()
    } else {
        historical.replace("FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE,", "")
            .replace(",\n            FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id) ON DELETE CASCADE\n", "\n")
    };
    if subject {
        sql = sql.replace(OLD_KINDS, SUBJECT_KINDS);
    }
    canonical.execute_batch(&sql)?;
    canonical.execute_batch(include_str!("assignment_tracking_schema.sql"))?;
    Ok(canonical)
}

/// Match a whole published three-table generation. The operation table selects
/// the known Workspace-FK variant; the other two must match that SAME variant.
/// Mixed principal generations or one-off dropped constraints are not accepted.
fn known_table_shape(conn: &Connection, subject: bool) -> Result<Connection> {
    let operation = normalize_sql(&schema_sql(conn, "table", PRINCIPAL_TABLES[0])?);
    for workspace_parent in [true, false] {
        let canonical = canonical_tables(subject, workspace_parent)?;
        if operation != normalize_sql(&schema_sql(&canonical, "table", PRINCIPAL_TABLES[0])?) {
            continue;
        }
        for table in PRINCIPAL_TABLES {
            if normalize_sql(&schema_sql(conn, "table", table)?)
                != normalize_sql(&schema_sql(&canonical, "table", table)?)
            {
                return Err(MigrationError::Contract(format!(
                    "Unsupported complete assignment table SQL in {table}; refusing unknown or mixed schema"
                )));
            }
        }
        return Ok(canonical);
    }
    Err(MigrationError::Contract(format!(
        "Unsupported complete assignment table SQL in {}; refusing unknown schema",
        PRINCIPAL_TABLES[0]
    )))
}

/// SQLite's documented CHECK-only schema-change procedure avoids rebuilding
/// immutable assignment parents and their many FK dependents. No columns,
/// rootpages, rowids, index definitions, or data bytes change. The *entire* known
/// old/new table SQL is preflighted before schema writes. The UPDATE targets are
/// exactly the three named main-schema table rows and use exact-SQL CAS.
///
/// A normal DROP/rebuild needs foreign_keys=OFF *before* the caller transaction:
/// defer_foreign_keys does not defer CASCADE actions or immutable child-delete
/// triggers. Renaming with FK enforcement rewrites inbound references. Rebuilding
/// the complete FK graph would include immutable tracking, receipts, work releases,
/// origins, deliveries, startup fences and historical implementation delegations.
/// That would require a different migration boundary and much broader data churn.
/// For this CHECK-only change under the agreed caller transaction, the documented
/// bounded schema edit is safer than an improvised graph rebuild. If table layout
/// ever changes, this helper must reject it; it is not a general migration engine.
fn widen_principal_checks(conn: &Connection) -> Result<()> {
    require_schema_writes_closed(conn)?;
    let old = known_table_shape(conn, false)?;
    let mut changes = Vec::new();
    for table in PRINCIPAL_TABLES {
        let sql = schema_sql(conn, "table", table)?;
        if sql.matches(OLD_KINDS).count() != 1 {
            return Err(MigrationError::Contract(format!(
                "Unsupported principal CHECK spelling in {table}"
            )));
        }
        changes.push((table, sql.replace(OLD_KINDS, SUBJECT_KINDS), sql));
    }
    // Do not silently repair same-named weakened principal triggers on upgrade.
    for name in [
        "typed_ticket_assignment_orchestrator_principal_insert",
        "typed_ticket_assignment_role_principal_insert",
    ] {
        require_same_sql(conn, &old, "trigger", name)?;
    }
    let cookie: i64 = conn.pragma_query_value(None, "schema_version", |row| row.get(0))?;
    if !(0..i64::from(i32::MAX)).contains(&cookie) {
        return Err(MigrationError::Contract(
            "SQLite schema cookie cannot be advanced".into(),
        ));
    }
    conn.pragma_update(None, "writable_schema", true)?;
    let change_result = (|| {
        let enabled: bool = conn.pragma_query_value(None, "writable_schema", |row| row.get(0))?;
        if !enabled {
            return Err(MigrationError::Contract(
                "SQLite refused the CHECK-only Subject schema migration".into(),
            ));
        }
        for (table, new_sql, old_sql) in changes {
            let changed = conn.execute(
                "UPDATE main.sqlite_schema SET sql=?2 WHERE type='table' AND name=?1 AND tbl_name=?1 AND sql=?3",
                params![table, new_sql, old_sql],
            )?;
            if changed != 1 {
                return Err(MigrationError::Contract(format!(
                    "Principal CHECK changed concurrently in {table}"
                )));
            }
        }
        // Engine schema cache generation, NOT the application migration ledger.
        conn.pragma_update(None, "schema_version", cookie + 1)?;
        Ok(())
    })();
    // writable_schema is connection state, not transactional: restore it even
    // on failure, before returning control to the caller's rollback path.
    let restore_result = conn.pragma_update(None, "writable_schema", false);
    restore_result?;
    require_schema_writes_closed(conn)?;
    change_result?;
    conn.flush_prepared_statement_cache();
    // Force a complete reparse with writable_schema OFF. Malformed schema must
    // fail here rather than leave an apparently successful migration receipt.
    for table in PRINCIPAL_TABLES {
        conn.prepare(&format!("SELECT * FROM {table} LIMIT 0"))?;
    }
    Ok(())
}

fn verify_integrity(conn: &Connection) -> Result<()> {
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(MigrationError::Contract(format!(
            "Subject assignment migration integrity check failed: {integrity}"
        )));
    }
    let violations: i64 =
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(MigrationError::Contract(
            "Subject assignment migration foreign-key violation".into(),
        ));
    }
    Ok(())
}
