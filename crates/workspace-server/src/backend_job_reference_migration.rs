//! Managed migration-owner continuation across the catalog and Subject database.
//! Schema130 captures exact removed Job identities; only managed open drains them.
//! Feature commit precedes catalog completion. No distributed transaction, Job
//! completion, writer release, or ordinary runtime legacy recovery is implied.
use super::{Connection, Error, Result, SqliteWorkspaceStore};
use crate::subjektiv::SubjektivStore;
use feature_storage::{DatabaseIdentity, FeatureStorage};
use rusqlite::OptionalExtension;
use std::path::Path;

pub(super) const SCHEMA: &str = r#"
CREATE TABLE backend_job_reference_migration_targets (
    workspace_id TEXT PRIMARY KEY NOT NULL,
    physical_identity TEXT CHECK (physical_identity IS NULL OR length(physical_identity)>0)
);
CREATE TABLE backend_job_reference_migration_jobs (
    workspace_id TEXT NOT NULL REFERENCES backend_job_reference_migration_targets(workspace_id) ON DELETE RESTRICT,
    job_id TEXT NOT NULL,
    PRIMARY KEY(workspace_id,job_id)
);
"#;

/// Must run inside schema130's destructive catalog transaction, before removal.
pub(super) fn capture(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    conn.execute_batch(
        "INSERT INTO backend_job_reference_migration_targets(workspace_id)
         SELECT DISTINCT j.workspace_id FROM backend_jobs j
         JOIN temp.__yoi_obsolete_jobs old USING(workspace_id,job_id);
         INSERT INTO backend_job_reference_migration_jobs(workspace_id,job_id)
         SELECT j.workspace_id,j.job_id FROM backend_jobs j
         JOIN temp.__yoi_obsolete_jobs old USING(workspace_id,job_id);",
    )?;
    Ok(())
}

/// The caller has admitted the managed catalog but has not exposed a usable store.
/// Batches bound memory/Feature transaction size; open drains all pending work.
pub(super) fn drain(
    store: &SqliteWorkspaceStore,
    owner: &FeatureStorage,
    server_path: &Path,
) -> Result<()> {
    // Registration belongs to the manager, not a workspace or a batch.
    let mut registration = None;
    loop {
        let target: Option<(String, Option<String>)> = store.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT workspace_id,physical_identity FROM backend_job_reference_migration_targets ORDER BY workspace_id LIMIT 1",
                [], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?)
        })?;
        let Some((workspace, mut physical)) = target else {
            return Ok(());
        };
        let scope = owner.scope(&workspace)?;
        let registration = match &registration {
            Some(registration) => registration,
            None => registration.insert(SubjektivStore::register(&scope).map_err(subject_error)?),
        };
        if physical.is_none() {
            // This checks the catalog owner and persisted legacy attribution. It
            // also rejects a missing file with an existing ownership receipt.
            store.adopt_workspace_feature_storage(&scope, &registration, "store_scope")?;
            // Optional Features never provisioned have no references to retire.
            // Path inspection is only an absence check, never DB open authority.
            // Identity-bearing access below uses the storage owner's APIs only.
            let path = server_path
                .parent()
                .ok_or_else(|| Error::Store("Server database has no parent".into()))?
                .join("feature-storage")
                .join(&workspace)
                .join("features/subjektiv.sqlite");
            if !path
                .try_exists()
                .map_err(|error| Error::Store(error.to_string()))?
            {
                scope.verify_owned_if_present(&registration)?;
                store.with_immediate_transaction(|tx| {
                    // Do not erase a concurrently pinned existent target.
                    let pinned: Option<String> = tx.query_row(
                        "SELECT physical_identity FROM backend_job_reference_migration_targets WHERE workspace_id=?1", [&workspace], |row| row.get(0))?;
                    if pinned.is_some() { return Err(Error::Store("Job reference migration target was concurrently pinned".into())); }
                    tx.execute("DELETE FROM backend_job_reference_migration_jobs WHERE workspace_id=?1", [&workspace])?;
                    tx.execute("DELETE FROM backend_job_reference_migration_targets WHERE workspace_id=?1", [&workspace])?;
                    Ok(())
                })?;
                continue;
            }
            let observed = scope.existing_identity(&registration)?;
            physical = Some(store.with_immediate_transaction(|tx| {
                tx.execute("UPDATE backend_job_reference_migration_targets SET physical_identity=?2 WHERE workspace_id=?1 AND physical_identity IS NULL", [&workspace, &observed.physical])?;
                let pinned: String = tx.query_row("SELECT physical_identity FROM backend_job_reference_migration_targets WHERE workspace_id=?1", [&workspace], |row| row.get(0))?;
                if pinned != observed.physical {
                    return Err(Error::Store("Job reference migration physical identity changed".into()));
                }
                Ok(pinned)
            })?);
        }
        let expected = DatabaseIdentity {
            scope: workspace.clone(),
            feature: registration.feature_id().to_owned(),
            physical: physical.expect("migration target pinned before effects"),
        };
        if scope.existing_identity(&registration)? != expected {
            return Err(Error::Store(
                "Job reference migration physical identity changed".into(),
            ));
        }
        let database = scope.open_existing(&registration, &expected)?;
        let subject = SubjektivStore::from_database(database).map_err(subject_error)?;
        let jobs = store.with_conn(|conn| {
            Ok(conn.prepare("SELECT job_id FROM backend_job_reference_migration_jobs WHERE workspace_id=?1 ORDER BY job_id LIMIT 128")?
                .query_map([&workspace], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        subject
            .retire_migrated_job_references(&jobs)
            .map_err(subject_error)?;
        #[cfg(test)]
        if INTERRUPT_AFTER_FEATURE_COMMIT.with(|flag| flag.replace(false)) {
            return Err(Error::Store(
                "injected interruption after Feature commit".into(),
            ));
        }
        store.with_immediate_transaction(|tx| {
            let pinned: String = tx.query_row("SELECT physical_identity FROM backend_job_reference_migration_targets WHERE workspace_id=?1", [&workspace], |row| row.get(0))?;
            if pinned != expected.physical {
                return Err(Error::Store("Job reference migration completion identity changed".into()));
            }
            for job in &jobs {
                tx.execute("DELETE FROM backend_job_reference_migration_jobs WHERE workspace_id=?1 AND job_id=?2", [&workspace, job])?;
            }
            tx.execute("DELETE FROM backend_job_reference_migration_targets WHERE workspace_id=?1 AND NOT EXISTS(SELECT 1 FROM backend_job_reference_migration_jobs WHERE workspace_id=?1)", [&workspace])?;
            Ok(())
        })?;
    }
}

fn subject_error(error: impl std::fmt::Display) -> Error {
    Error::Store(format!("Job reference migration: {error}"))
}

#[cfg(test)]
thread_local! {
    static INTERRUPT_AFTER_FEATURE_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
#[path = "backend_job_reference_migration_tests.rs"]
mod tests;
