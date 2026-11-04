//! Physical compatibility with published Subjektiv stores, not adoption of domain authority.
//!
//! FeatureStorage calls `validate` in its migration transaction, before applying pending common
//! migrations. Published receipts and complete SQLite shapes must agree. Recognized extensions
//! stay physically intact: this module never installs a permission policy, transforms Job states,
//! or advances Journal scopes on the target. Those transitions belong to their domain owners.

use crate::{MIGRATIONS, SUBJEKTIV_FEATURE_ID};
use feature_storage::FeatureStorageError;
use rusqlite::{Connection, Transaction, params};
use std::sync::OnceLock;

const HISTORY_TABLE: &str = "__yoi_feature_schema_migrations";
const COMMON_COUNT: usize = 17;
const PERMISSION_18: &str =
    "seal legacy permission authority and retain creation enrollment defaults";
const CONSOLIDATION_18: &str = "retain irreversible consolidation write bindings";
const FIXED_18: &str = "fixed Journal removal scopes";
const CANCELLATION_19: &str = "retain exact Subject release scope cancellations";
const CONVERGENCE_20: &str = "converge published Journal scope and consolidation write histories";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Common(usize),
    Permission18,
    Consolidation18,
    Fixed18,
    Cancellation19,
    // Equal final shapes do not make their receipt histories interchangeable.
    Consolidation20,
    Fixed20,
    Cancellation20,
}

fn incompatible(detail: impl Into<String>) -> FeatureStorageError {
    FeatureStorageError::MigrationHistory {
        feature: SUBJEKTIV_FEATURE_ID.into(),
        detail: detail.into(),
    }
}

fn recognize(applied: &[(u32, String)]) -> feature_storage::Result<Format> {
    let count = applied.len().min(COMMON_COUNT);
    if !applied[..count]
        .iter()
        .zip(&MIGRATIONS[..count])
        .all(|((version, name), expected)| {
            *version == expected.version() && name == expected.name()
        })
    {
        return Err(incompatible(
            "receipt prefix differs from published Subjektiv 1..17",
        ));
    }
    if applied.len() <= COMMON_COUNT {
        return Ok(Format::Common(count));
    }
    let tail: Vec<_> = applied[COMMON_COUNT..]
        .iter()
        .map(|(version, name)| (*version, name.as_str()))
        .collect();
    match tail.as_slice() {
        [(18, PERMISSION_18)] => Ok(Format::Permission18),
        [(18, CONSOLIDATION_18)] => Ok(Format::Consolidation18),
        [(18, FIXED_18)] => Ok(Format::Fixed18),
        [(18, FIXED_18), (19, CANCELLATION_19)] => Ok(Format::Cancellation19),
        [(18, CONSOLIDATION_18), (20, CONVERGENCE_20)] => Ok(Format::Consolidation20),
        [(18, FIXED_18), (20, CONVERGENCE_20)] => Ok(Format::Fixed20),
        [(18, FIXED_18), (19, CANCELLATION_19), (20, CONVERGENCE_20)] => Ok(Format::Cancellation20),
        _ => Err(incompatible(
            "unknown, missing or mixed published Subjektiv receipt tail",
        )),
    }
}

/// Validates physical compatibility only. The caller owns identity/adoption and the transaction.
/// Never relabel/delete original receipts, fill historical gaps, or append a domain receipt here.
/// After success the engine may apply only the pending registered common migrations (1..17).
pub(super) fn validate(
    tx: &Transaction<'_>,
    applied: &[(u32, String)],
) -> feature_storage::Result<()> {
    let format = recognize(applied)?;
    let expected = reference_shape(format)?;
    let actual = domain_shape(tx)?;
    if actual != expected {
        let difference = actual
            .iter()
            .find(|object| !expected.contains(object))
            .or_else(|| expected.iter().find(|object| !actual.contains(object)));
        return Err(incompatible(format!(
            "published receipts disagree with SQLite shape at {}",
            difference.map_or("unknown object", |object| object.1.as_str())
        )));
    }
    Ok(())
}

// Compare tables, indexes, views, triggers and their full SQL (including constraints), not
// merely column presence. Exact physical-owner metadata is excluded, never a wildcard prefix.
type SchemaObject = (String, String, String, Option<String>);

fn domain_shape(connection: &Connection) -> feature_storage::Result<Vec<SchemaObject>> {
    let mut statement = connection.prepare(
        "SELECT type,name,tbl_name,sql FROM sqlite_schema
         WHERE name NOT GLOB 'sqlite_*'
           AND name NOT IN ('__yoi_feature_schema_migrations', '__yoi_storage_identity', '__yoi_storage_origins', '__yoi_feature_database_identity', '__yoi_feature_provisioning_receipt')
         ORDER BY type,name",
    )?;
    let shape = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(shape)
}

fn reference_shape(format: Format) -> feature_storage::Result<Vec<SchemaObject>> {
    // Cache reference shapes only; every target is reread under its caller's transaction.
    static SHAPES: [OnceLock<Vec<SchemaObject>>; 25] = [const { OnceLock::new() }; 25];
    let index = match format {
        Format::Common(count) => count,
        Format::Permission18 => 18,
        Format::Consolidation18 => 19,
        Format::Fixed18 => 20,
        Format::Cancellation19 => 21,
        Format::Consolidation20 => 22,
        Format::Fixed20 => 23,
        Format::Cancellation20 => 24,
    };
    if let Some(shape) = SHAPES[index].get() {
        return Ok(shape.clone());
    }
    let mut reference = Connection::open_in_memory()?;
    reference.execute_batch("PRAGMA foreign_keys=ON;")?;
    let tx = reference.transaction()?;
    tx.execute_batch(
        "CREATE TABLE __yoi_feature_schema_migrations (
            version INTEGER PRIMARY KEY CHECK(version>0), name TEXT NOT NULL,
            applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);",
    )?;
    let common = match format {
        Format::Common(count) => count,
        _ => COMMON_COUNT,
    };
    for migration in &MIGRATIONS[..common] {
        migration.apply(&tx)?;
        tx.execute(
            &format!("INSERT INTO {HISTORY_TABLE}(version,name) VALUES(?1,?2)"),
            params![migration.version(), migration.name()],
        )?;
    }
    // These frozen DDL fragments run ONLY on the disposable reference, never on a target.
    // Preserve ALTER/rebuild provenance: sqlite_schema text can differ for equal columns.
    match format {
        Format::Common(_) => {}
        Format::Permission18 => {
            tx.execute_batch(PERMISSION_SEAL_SQL)?;
            for table in PERMISSION_FENCE_TABLES {
                for action in ["INSERT", "UPDATE"] {
                    tx.execute_batch(&format!(
                        "CREATE TRIGGER permission_sealed_{table}_{action} BEFORE {action} ON {table}
                 WHEN EXISTS(SELECT 1 FROM permission_authority_seal)
                 BEGIN SELECT RAISE(ABORT, 'legacy permission authority is sealed'); END;"
                    ))?;
                }
                tx.execute_batch(&format!(
                    "CREATE TRIGGER permission_sealed_{table}_DELETE BEFORE DELETE ON {table}
             WHEN EXISTS(SELECT 1 FROM permission_authority_seal)
               AND NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
             BEGIN SELECT RAISE(ABORT, 'legacy permission authority is sealed'); END;"
                ))?;
            }
        }
        Format::Consolidation18 => tx.execute_batch(CONSOLIDATION_SQL)?,
        Format::Fixed18 => tx.execute_batch(FIXED_SCOPE_SQL)?,
        Format::Cancellation19 => {
            tx.execute_batch(FIXED_SCOPE_SQL)?;
            tx.execute_batch(CANCELLATION_SQL)?;
        }
        Format::Consolidation20 | Format::Fixed20 | Format::Cancellation20 => {
            tx.execute_batch(FIXED_SCOPE_SQL)?;
            tx.execute_batch(CANCELLATION_SQL)?;
            tx.execute_batch(CONSOLIDATION_SQL)?;
        }
    }
    let shape = domain_shape(&tx)?;
    let _ = SHAPES[index].set(shape.clone());
    Ok(shape)
}

const PERMISSION_FENCE_TABLES: [&str; 4] = [
    "subject_permissions",
    "subject_permission_changes",
    "subject_permission_receipts",
    "worker_execution_grants",
];

// Frozen physical shape evidence, not target migrations. Permission SQL is from
// 9edebfa5936f2411adb6affffc793d3e93cc21bd:crates/subjektiv/src/permissions/authority.rs.
// Journal/consolidation SQL is from 7b39a44ca698c474287e5511f674f2e8baae4c03:
// crates/subjektiv/src/{consolidation_write_bindings.sql,journal/final_scope.sql,
// journal/final_scope/cancellation.sql}. Receipt provenance follows that ref's recognizer;
// no permission-seal -> 20 history is published there.

const PERMISSION_SEAL_SQL: &str = r#"CREATE TABLE permission_authority_seal (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            transfer_id TEXT NOT NULL CHECK(length(transfer_id)>0),
            export_json TEXT NOT NULL
        );
        CREATE TRIGGER permission_authority_seal_immutable BEFORE UPDATE ON permission_authority_seal
            BEGIN SELECT RAISE(ABORT, 'permission authority seal is immutable'); END;
        CREATE TRIGGER permission_authority_seal_retained BEFORE DELETE ON permission_authority_seal
            BEGIN SELECT RAISE(ABORT, 'permission authority seal is permanent'); END;
        ALTER TABLE subject_creation_receipts ADD COLUMN permission_enrollment_json TEXT;
        CREATE TRIGGER subject_creation_receipts_immutable BEFORE UPDATE ON subject_creation_receipts
            BEGIN SELECT RAISE(ABORT, 'Subject creation receipts are immutable'); END;
        CREATE TRIGGER subject_creation_receipts_retained BEFORE DELETE ON subject_creation_receipts
            BEGIN SELECT RAISE(ABORT, 'Subject creation receipts are retained'); END;"#;

const CONSOLIDATION_SQL: &str = r#"-- Domain write fence only: no Job runner state and no Subject content.
-- Deliberately no Subject foreign key: erasure must retain this identity fence.
CREATE TABLE consolidation_write_bindings (
    scope_id TEXT NOT NULL,
    job_id TEXT NOT NULL,
    attempt_id TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    runtime_id TEXT NOT NULL,
    worker_id TEXT NOT NULL,
    input_digest TEXT NOT NULL,
    grant_digest TEXT NOT NULL,
    deadline_at TEXT NOT NULL,
    revoked_at TEXT,
    PRIMARY KEY (scope_id, job_id, attempt_id),
    CHECK (revoked_at IS NULL OR length(revoked_at) > 0)
);

-- BEFORE INSERT also catches INSERT OR REPLACE, even with recursive_triggers off.
CREATE TRIGGER consolidation_write_binding_no_replace
BEFORE INSERT ON consolidation_write_bindings
WHEN EXISTS (SELECT 1 FROM consolidation_write_bindings
             WHERE scope_id = NEW.scope_id AND job_id = NEW.job_id AND attempt_id = NEW.attempt_id)
BEGIN
    SELECT RAISE(ABORT, 'consolidation write binding already exists');
END;

CREATE TRIGGER consolidation_write_binding_immutable
BEFORE UPDATE ON consolidation_write_bindings
WHEN NEW.scope_id IS NOT OLD.scope_id
  OR NEW.job_id IS NOT OLD.job_id OR NEW.attempt_id IS NOT OLD.attempt_id
  OR NEW.subject_id IS NOT OLD.subject_id OR NEW.runtime_id IS NOT OLD.runtime_id
  OR NEW.worker_id IS NOT OLD.worker_id OR NEW.input_digest IS NOT OLD.input_digest
  OR NEW.grant_digest IS NOT OLD.grant_digest OR NEW.deadline_at IS NOT OLD.deadline_at
  OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NOT OLD.revoked_at)
BEGIN
    SELECT RAISE(ABORT, 'consolidation write binding is immutable or revoked');
END;

CREATE TRIGGER consolidation_write_binding_no_delete
BEFORE DELETE ON consolidation_write_bindings
BEGIN
    SELECT RAISE(ABORT, 'consolidation write binding must be retained');
END;
"#;

const FIXED_SCOPE_SQL: &str = r#"-- Fixed removal scope metadata only; no source bodies.
-- No FK to live Subject/writer: retained identity evidence cannot block explicit deletion.
-- Every API rechecks the live Subject, exact binding and permanent erase fences.
CREATE TABLE journal_final_scopes (
    request_id TEXT PRIMARY KEY,
    removal_operation_id TEXT NOT NULL UNIQUE,
    binding_id TEXT NOT NULL UNIQUE,
    owner_json TEXT NOT NULL,
    member_digest TEXT NOT NULL,
    member_count INTEGER NOT NULL CHECK(member_count >= 0),
    collected_count INTEGER NOT NULL DEFAULT 0 CHECK(collected_count >= 0 AND collected_count <= member_count),
    after_session_id TEXT,
    scope_digest TEXT
);
CREATE TRIGGER journal_final_scopes_guard BEFORE UPDATE ON journal_final_scopes
WHEN OLD.scope_digest IS NOT NULL OR NEW.request_id != OLD.request_id
 OR NEW.removal_operation_id != OLD.removal_operation_id OR NEW.binding_id != OLD.binding_id
 OR NEW.owner_json != OLD.owner_json OR NEW.member_digest != OLD.member_digest
 OR NEW.member_count != OLD.member_count OR NEW.collected_count < OLD.collected_count
 OR (NEW.scope_digest IS NOT NULL AND NEW.collected_count != NEW.member_count)
BEGIN SELECT RAISE(ABORT, 'fixed Journal scope owner and frozen header are immutable'); END;
CREATE TRIGGER journal_final_scopes_no_delete BEFORE DELETE ON journal_final_scopes
BEGIN SELECT RAISE(ABORT, 'fixed Journal scopes are retained'); END;
-- One normalized permanent metadata row per collected Session. No parallel full-scope JSON.
CREATE TABLE journal_final_scope_sessions (
    request_id TEXT NOT NULL REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    session_id TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    PRIMARY KEY(request_id,session_id)
);
CREATE TRIGGER journal_final_scope_sessions_frozen_insert BEFORE INSERT ON journal_final_scope_sessions
WHEN (SELECT scope_digest FROM journal_final_scopes WHERE request_id=NEW.request_id) IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'frozen Session set is immutable'); END;
CREATE TRIGGER journal_final_scope_sessions_no_update BEFORE UPDATE ON journal_final_scope_sessions
BEGIN SELECT RAISE(ABORT, 'fixed Session metadata is immutable'); END;
CREATE TRIGGER journal_final_scope_sessions_no_delete BEFORE DELETE ON journal_final_scope_sessions
BEGIN SELECT RAISE(ABORT, 'fixed Session metadata is retained'); END;
-- Page receipts contain identities only; replay metadata is read from normalized rows.
CREATE TABLE journal_final_scope_pages (
    request_id TEXT NOT NULL REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    after_session_id TEXT NOT NULL,
    through_session_id TEXT NOT NULL,
    page_digest TEXT NOT NULL,
    PRIMARY KEY(request_id,after_session_id)
);
CREATE TRIGGER journal_final_scope_pages_no_update BEFORE UPDATE ON journal_final_scope_pages
BEGIN SELECT RAISE(ABORT, 'scope page receipts are immutable'); END;
CREATE TRIGGER journal_final_scope_pages_no_delete BEFORE DELETE ON journal_final_scope_pages
BEGIN SELECT RAISE(ABORT, 'scope page receipts are retained'); END;
CREATE TABLE journal_scope_release_receipts (
    request_id TEXT PRIMARY KEY REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    receipt_json TEXT NOT NULL
);
CREATE TRIGGER journal_scope_release_no_update BEFORE UPDATE ON journal_scope_release_receipts
BEGIN SELECT RAISE(ABORT, 'Journal release receipts are immutable'); END;
CREATE TRIGGER journal_scope_release_no_delete BEFORE DELETE ON journal_scope_release_receipts
BEGIN SELECT RAISE(ABORT, 'Journal release receipts are retained'); END;
"#;

const CANCELLATION_SQL: &str = r#"-- Migration 19 replaces only migration-18's unconditional binding UNIQUE with a live-owner
-- constraint. Transaction-local copies preserve every column/row. Published 18 is unchanged.
-- No Journal history, body input or checkpoint is removed or rewritten.
CREATE TEMP TABLE scope_cancel_copy_journal_final_scopes AS SELECT * FROM journal_final_scopes;
CREATE TEMP TABLE scope_cancel_copy_journal_final_scope_sessions AS SELECT * FROM journal_final_scope_sessions;
CREATE TEMP TABLE scope_cancel_copy_journal_final_scope_pages AS SELECT * FROM journal_final_scope_pages;
CREATE TEMP TABLE scope_cancel_copy_journal_scope_release_receipts AS SELECT * FROM journal_scope_release_receipts;
DROP TABLE journal_scope_release_receipts;
DROP TABLE journal_final_scope_pages;
DROP TABLE journal_final_scope_sessions;
DROP TABLE journal_final_scopes;
CREATE TABLE journal_final_scopes (
    request_id TEXT PRIMARY KEY,
    removal_operation_id TEXT NOT NULL UNIQUE,
    binding_id TEXT NOT NULL,
    owner_json TEXT NOT NULL,
    member_digest TEXT NOT NULL,
    member_count INTEGER NOT NULL CHECK(member_count >= 0),
    collected_count INTEGER NOT NULL DEFAULT 0 CHECK(collected_count >= 0 AND collected_count <= member_count),
    after_session_id TEXT,
    scope_digest TEXT
);
CREATE TABLE journal_final_scope_sessions (
    request_id TEXT NOT NULL REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    session_id TEXT NOT NULL,
    metadata_json TEXT NOT NULL,
    PRIMARY KEY(request_id,session_id)
);
CREATE TABLE journal_final_scope_pages (
    request_id TEXT NOT NULL REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    after_session_id TEXT NOT NULL,
    through_session_id TEXT NOT NULL,
    page_digest TEXT NOT NULL,
    PRIMARY KEY(request_id,after_session_id)
);
CREATE TABLE journal_scope_release_receipts (
    request_id TEXT PRIMARY KEY REFERENCES journal_final_scopes(request_id) ON DELETE RESTRICT,
    receipt_json TEXT NOT NULL
);
INSERT INTO journal_final_scopes SELECT * FROM scope_cancel_copy_journal_final_scopes;
DROP TABLE scope_cancel_copy_journal_final_scopes;
INSERT INTO journal_final_scope_sessions SELECT * FROM scope_cancel_copy_journal_final_scope_sessions;
DROP TABLE scope_cancel_copy_journal_final_scope_sessions;
INSERT INTO journal_final_scope_pages SELECT * FROM scope_cancel_copy_journal_final_scope_pages;
DROP TABLE scope_cancel_copy_journal_final_scope_pages;
INSERT INTO journal_scope_release_receipts SELECT * FROM scope_cancel_copy_journal_scope_release_receipts;
DROP TABLE scope_cancel_copy_journal_scope_release_receipts;
CREATE TRIGGER journal_final_scopes_guard BEFORE UPDATE ON journal_final_scopes
WHEN OLD.scope_digest IS NOT NULL OR NEW.request_id != OLD.request_id
 OR NEW.removal_operation_id != OLD.removal_operation_id OR NEW.binding_id != OLD.binding_id
 OR NEW.owner_json != OLD.owner_json OR NEW.member_digest != OLD.member_digest
 OR NEW.member_count != OLD.member_count OR NEW.collected_count < OLD.collected_count
 OR (NEW.scope_digest IS NOT NULL AND NEW.collected_count != NEW.member_count)
BEGIN SELECT RAISE(ABORT, 'fixed Journal scope owner and frozen header are immutable'); END;
CREATE TRIGGER journal_final_scopes_no_delete BEFORE DELETE ON journal_final_scopes
BEGIN SELECT RAISE(ABORT, 'fixed Journal scopes are retained'); END;
CREATE TRIGGER journal_final_scope_sessions_frozen_insert BEFORE INSERT ON journal_final_scope_sessions
WHEN (SELECT scope_digest FROM journal_final_scopes WHERE request_id=NEW.request_id) IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'frozen Session set is immutable'); END;
CREATE TRIGGER journal_final_scope_sessions_no_update BEFORE UPDATE ON journal_final_scope_sessions
BEGIN SELECT RAISE(ABORT, 'fixed Session metadata is immutable'); END;
CREATE TRIGGER journal_final_scope_sessions_no_delete BEFORE DELETE ON journal_final_scope_sessions
BEGIN SELECT RAISE(ABORT, 'fixed Session metadata is retained'); END;
CREATE TRIGGER journal_final_scope_pages_no_update BEFORE UPDATE ON journal_final_scope_pages
BEGIN SELECT RAISE(ABORT, 'scope page receipts are immutable'); END;
CREATE TRIGGER journal_final_scope_pages_no_delete BEFORE DELETE ON journal_final_scope_pages
BEGIN SELECT RAISE(ABORT, 'scope page receipts are retained'); END;
CREATE TRIGGER journal_scope_release_no_update BEFORE UPDATE ON journal_scope_release_receipts
BEGIN SELECT RAISE(ABORT, 'Journal release receipts are immutable'); END;
CREATE TRIGGER journal_scope_release_no_delete BEFORE DELETE ON journal_scope_release_receipts
BEGIN SELECT RAISE(ABORT, 'Journal release receipts are retained'); END;
-- One immutable cancellation fact with its parent identity, including before collection.
CREATE TABLE journal_scope_cancellations (
    request_id TEXT PRIMARY KEY,
    owner_json TEXT NOT NULL
);
CREATE TRIGGER journal_scope_cancellations_no_update BEFORE UPDATE ON journal_scope_cancellations
BEGIN SELECT RAISE(ABORT, 'scope cancellation is immutable'); END;
CREATE TRIGGER journal_scope_cancellations_no_delete BEFORE DELETE ON journal_scope_cancellations
BEGIN SELECT RAISE(ABORT, 'scope cancellation is retained'); END;
CREATE TRIGGER journal_scope_cancellation_guard BEFORE INSERT ON journal_scope_cancellations
WHEN NEW.request_id != 'subject-release:' || json_extract(NEW.owner_json,'$.removal_operation_id')
 OR NEW.request_id != json_extract(NEW.owner_json,'$.request_id')
 OR EXISTS(SELECT 1 FROM journal_scope_release_receipts WHERE request_id=NEW.request_id)
 OR EXISTS(SELECT 1 FROM journal_final_scopes s WHERE s.request_id=NEW.request_id AND s.owner_json!=NEW.owner_json)
BEGIN SELECT RAISE(ABORT, 'only exact unreleased Subject release scope can be cancelled'); END;
CREATE TRIGGER journal_final_scopes_live_owner BEFORE INSERT ON journal_final_scopes
WHEN EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=NEW.request_id)
 OR EXISTS(SELECT 1 FROM journal_final_scopes s WHERE s.binding_id=NEW.binding_id
     AND NOT EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=s.request_id))
BEGIN SELECT RAISE(ABORT, 'fixed scope writer is occupied or request cancelled'); END;
CREATE TRIGGER journal_scope_release_not_cancelled BEFORE INSERT ON journal_scope_release_receipts
WHEN EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=NEW.request_id)
BEGIN SELECT RAISE(ABORT, 'cancelled scope cannot release'); END;
CREATE TRIGGER journal_final_scope_cancelled_update BEFORE UPDATE ON journal_final_scopes
WHEN EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=OLD.request_id)
BEGIN SELECT RAISE(ABORT, 'cancelled scope cannot continue'); END;
CREATE TRIGGER journal_final_scope_cancelled_session BEFORE INSERT ON journal_final_scope_sessions
WHEN EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=NEW.request_id)
BEGIN SELECT RAISE(ABORT, 'cancelled scope cannot continue'); END;
CREATE TRIGGER journal_final_scope_cancelled_page BEFORE INSERT ON journal_final_scope_pages
WHEN EXISTS(SELECT 1 FROM journal_scope_cancellations c WHERE c.request_id=NEW.request_id)
BEGIN SELECT RAISE(ABORT, 'cancelled scope cannot continue'); END;
"#;

#[cfg(test)]
#[path = "storage_format_tests.rs"]
mod tests;
