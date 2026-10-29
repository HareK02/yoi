//! Workspace composition of Ticket-owned Subject responsibility (component 14).
//!
//! Ticket alone owns principal tables/CHECKs, domain assignment guards and the
//! component migration ledger. This adapter composes that migration in the host
//! transaction, then installs only Workspace delivery/start guards. It never edits
//! sqlite_schema, rebuilds Ticket tables, or reinstalls Ticket-owned triggers.
//!
//! There is no trustworthy conversion of `workspace-orchestrator` to a Subject.
//! A current `subjektiv:<id>` body or child execution grant cannot attest ownership
//! of a historical stable-agent assignment. Original principals/current pointers,
//! tracking, inputs, results, receipt fingerprints and source JSON stay evidence,
//! not execution authority. Newly authorized Subject assignment is explicit.
//!
//! No Runtime/FeatureStorage calls, routing, grants, restore or receipt replay
//! occur here. The caller owns the surrounding transaction and Workspace version;
//! on any error it must roll back the whole transaction, including Ticket work.
use super::*;

pub(super) const LEGACY_DIAGNOSTIC: &str = "Legacy Orchestrator identity retained without a trustworthy historical Subject mapping; nonoperational, explicit authorized Subject assignment required. Saved history and receipts do not grant execution authority.";

const HOST_GUARDS: [&str; 2] = [
    "ticket_legacy_orchestrator_delivery_no_insert",
    "ticket_legacy_orchestrator_start_no_insert",
];

/// Parent may already have composed component 14 in this same transaction; the
/// Ticket entrypoint then verifies its current schema without applying it again.
/// Host guard installation is a single migration, not an operational repair API.
pub(super) fn install(conn: &Connection) -> Result<()> {
    if conn.is_autocommit() {
        return Err(Error::Store(
            "Subject assignment migration requires a caller-owned transaction".into(),
        ));
    }
    ticket::migrate_sqlite_ticket_schema_in_transaction(conn).map_err(|error| {
        Error::Store(format!(
            "Ticket Subject component composition failed: {error}"
        ))
    })?;
    conn.execute_batch(HOST_GUARDS_SQL)?;
    // Component 14 currently supplies this same diagnostic. Ensure it exists for
    // retained legacy rows without duplicating its line or touching their CAS,
    // tracking values, historical source JSON, events, inputs or receipts.
    conn.execute(
        "UPDATE ticket_worker_assignments
         SET tracking_migration_diagnostic = CASE
           WHEN tracking_migration_diagnostic IS NULL OR tracking_migration_diagnostic=''
             THEN ?1 ELSE tracking_migration_diagnostic || char(10) || ?1 END
         WHERE role='orchestrator' AND principal_kind!='subject'
           AND instr(char(10) || COALESCE(tracking_migration_diagnostic,'') || char(10),
                     char(10) || ?1 || char(10))=0",
        [LEGACY_DIAGNOSTIC],
    )?;
    verify(conn)
}

/// Read-only verification delegates the owning table/domain-guard contract to
/// Ticket. Workspace verifies only its two complete trigger definitions/targets,
/// followed by host database integrity and FK validation. No repair is performed.
pub(super) fn verify(conn: &Connection) -> Result<()> {
    ticket::verify_sqlite_ticket_schema(conn).map_err(|error| {
        Error::Store(format!(
            "Ticket Subject schema verification failed: {error}"
        ))
    })?;
    let canonical = Connection::open_in_memory()?;
    canonical.execute_batch("CREATE TABLE ticket_assignment_deliveries(workspace_id TEXT,assignment_id TEXT);
        CREATE TABLE ticket_assignment_operation_fences(workspace_id TEXT,orchestrator_assignment_id TEXT);")?;
    canonical.execute_batch(HOST_GUARDS_SQL)?;
    for name in HOST_GUARDS {
        let expected = trigger_definition(&canonical, name)?;
        let actual = trigger_definition(conn, name)?;
        if actual != expected {
            return Err(Error::Store(format!(
                "Workspace Subject guard {name} differs from canonical SQL or target"
            )));
        }
    }
    let integrity: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(Error::Store(format!(
            "Workspace Subject migration integrity check failed: {integrity}"
        )));
    }
    let violations: i64 =
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })?;
    if violations != 0 {
        return Err(Error::Store(
            "Workspace Subject migration foreign-key violation".into(),
        ));
    }
    Ok(())
}

fn trigger_definition(conn: &Connection, name: &str) -> Result<(String, String)> {
    conn.query_row(
        "SELECT tbl_name,sql FROM main.sqlite_schema WHERE type='trigger' AND name=?1",
        [name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()?
    .ok_or_else(|| Error::Store(format!("Missing Workspace Subject guard {name}")))
}

const HOST_GUARDS_SQL: &str = r#"
CREATE TRIGGER ticket_legacy_orchestrator_delivery_no_insert
BEFORE INSERT ON ticket_assignment_deliveries
WHEN EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.assignment_id
 AND a.role='orchestrator' AND a.principal_kind!='subject')
BEGIN SELECT RAISE(ABORT,'Legacy Orchestrator responsibility is nonoperational'); END;
CREATE TRIGGER ticket_legacy_orchestrator_start_no_insert
BEFORE INSERT ON ticket_assignment_operation_fences
WHEN NOT EXISTS(SELECT 1 FROM ticket_worker_assignments a
 WHERE a.workspace_id=NEW.workspace_id AND a.assignment_id=NEW.orchestrator_assignment_id
 AND a.role='orchestrator' AND a.principal_kind='subject')
BEGIN SELECT RAISE(ABORT,'Ticket startup fence requires Subject responsibility'); END;
"#;
