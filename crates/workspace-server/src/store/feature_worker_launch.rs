//! Public Feature-owned create operations are aliases of an existing reservation.
//! This is frozen caller/configuration evidence, not an execution grant or a
//! second launch state machine. Manual singleton reuse keeps its original row.
use super::*;
use worker_runtime::config_bundle::ConfigBundle;

pub(super) const MIGRATION_NAME: &str = "Public Worker create operation reservation bindings";
pub(super) const SCHEMA: &str = "
CREATE TABLE worker_create_operation_bindings (
    workspace_id TEXT NOT NULL,
    operation_id TEXT NOT NULL CHECK (length(trim(operation_id)) > 0),
    caller_fingerprint TEXT NOT NULL CHECK (length(trim(caller_fingerprint)) > 0),
    allocation_key TEXT NOT NULL,
    profile TEXT NOT NULL CHECK (length(trim(profile)) > 0),
    config_bundle_json TEXT NOT NULL CHECK (json_valid(config_bundle_json) AND json_type(config_bundle_json) = 'object'),
    PRIMARY KEY (workspace_id, operation_id),
    FOREIGN KEY (workspace_id, allocation_key)
        REFERENCES worker_create_reservations(workspace_id, allocation_key)
);
CREATE INDEX worker_create_operation_allocation
    ON worker_create_operation_bindings(workspace_id, allocation_key);
";

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 142 {
        return Err(Error::Store(
            "Worker create operation binding migration requires schema 142".into(),
        ));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Caller hashes cannot reconstruct old public input. No legacy row is
    // rewritten and no alias or grant is inferred from existing allocations.
    tx.execute_batch(SCHEMA)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(143,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedFeatureWorkerLaunch {
    pub profile: String,
    pub bundle: ConfigBundle,
    pub initialization: FeatureInitializations,
}

struct SavedBinding {
    allocation: String,
    launch: SavedFeatureWorkerLaunch,
}

impl SqliteWorkspaceStore {
    /// Saved evidence only. The public caller still owns current authorization
    /// and must recheck its Feature/domain admission before using the result.
    pub(crate) fn saved_feature_worker_launch(
        &self,
        workspace: &str,
        operation: &str,
        caller_fingerprint: &str,
    ) -> Result<Option<SavedFeatureWorkerLaunch>> {
        self.with_conn(|conn| {
            // Keep the alias, allocation and optional plugin plan in one read
            // snapshot even when another Server uses a different connection.
            let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Deferred)?;
            let saved = read_saved(&tx, workspace, operation, caller_fingerprint)?;
            tx.commit()?;
            Ok(saved.map(|saved| saved.launch))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reserve_feature_worker_launch(
        &self,
        workspace: &str,
        runtime: &str,
        operation: &str,
        caller_fingerprint: &str,
        request_fingerprint: &str,
        singleton: Option<&str>,
        memory: &WorkspaceMemorySettingsRecord,
        profile: &str,
        bundle: &ConfigBundle,
        initialization: &FeatureInitializations,
    ) -> Result<WorkerCreateReservation> {
        if operation.trim().is_empty() || caller_fingerprint.trim().is_empty() {
            return Err(Error::InvalidInput(
                "Worker create operation and full caller fingerprint must be non-empty".into(),
            ));
        }
        if !valid_profile_bundle(workspace, profile, bundle) {
            return Err(Error::InvalidInput(
                "resolved Feature Worker Profile or bundle mismatch".into(),
            ));
        }
        let allocation = format!("manual:{operation}");
        let bundle_json = serde_json::to_string(bundle)
            .map_err(|e| Error::Store(format!("Feature Worker Profile bundle: {e}")))?;
        self.reserve_worker_create_with_binding(
            workspace, runtime, &allocation, request_fingerprint, singleton, memory, initialization,
            |tx| {
                // Double-check under the same IMMEDIATE transaction as reserve.
                // A prior public read cannot fence competing Server processes.
                if let Some(saved) = read_saved(tx, workspace, operation, caller_fingerprint)? {
                    if saved.launch.profile != profile || saved.launch.bundle != *bundle {
                        return Err(Error::RepositoryConflict(
                            "Worker create operation has different frozen Profile input; reread the saved launch".into(),
                        ));
                    }
                    feature_initialization::require_same(&saved.launch.initialization, initialization)?;
                    let identity: (String, Option<String>, Option<String>) = tx.query_row(
                        "SELECT runtime_id,request_fingerprint,singleton_key FROM worker_create_reservations WHERE workspace_id=?1 AND allocation_key=?2",
                        params![workspace, saved.allocation],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )?;
                    if identity.0 != runtime || identity.1.as_deref() != Some(request_fingerprint)
                        || identity.2.as_deref() != singleton
                    {
                        return Err(Error::RepositoryConflict(
                            "Worker create operation reused with different reservation input".into(),
                        ));
                    }
                }
                Ok(None)
            },
            |_, _| Ok(()),
            |tx, reservation| {
                // Singleton reuse may return a Worker belonging to another
                // manual operation. Bind the actual existing reservation PK,
                // never invent a new allocation or reference a core grant.
                let actual: String = tx.query_row(
                    "SELECT allocation_key FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3",
                    params![workspace, runtime, reservation.worker_id.to_string()],
                    |row| row.get(0),
                )?;
                let frozen: Option<(String, String)> = tx.query_row(
                    "SELECT profile,config_bundle_json FROM worker_create_operation_bindings WHERE workspace_id=?1 AND allocation_key=?2 LIMIT 1",
                    params![workspace, actual], |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional()?;
                if let Some((saved_profile, saved_bundle)) = frozen {
                    if saved_profile != profile || saved_bundle != bundle_json {
                        return Err(Error::RepositoryConflict(
                            "singleton reuse must preserve its original frozen Profile bundle".into(),
                        ));
                    }
                } else if actual != allocation {
                    return Err(Error::RepositoryConflict(
                        "worker_create_legacy_profile_missing: singleton has no original frozen Backend Profile evidence; preserve its allocation and reconcile before binding a public operation".into(),
                    ));
                }
                let existing: Option<(String, String, String, String)> = tx.query_row(
                    "SELECT allocation_key,caller_fingerprint,profile,config_bundle_json FROM worker_create_operation_bindings WHERE workspace_id=?1 AND operation_id=?2",
                    params![workspace, operation],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                ).optional()?;
                if let Some(existing) = existing {
                    if existing != (actual, caller_fingerprint.to_owned(), profile.to_owned(), bundle_json.clone()) {
                        return Err(Error::RepositoryConflict(
                            "Worker create operation already binds different full input or allocation".into(),
                        ));
                    }
                } else {
                    tx.execute(
                        "INSERT INTO worker_create_operation_bindings(workspace_id,operation_id,caller_fingerprint,allocation_key,profile,config_bundle_json) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![workspace, operation, caller_fingerprint, actual, profile, bundle_json],
                    )?;
                }
                Ok(())
            },
        )
    }
}

pub(super) fn allocation_for_operation(
    conn: &Connection,
    workspace: &str,
    operation_or_allocation: &str,
) -> Result<String> {
    let bound: Option<String> = conn.query_row(
        "SELECT allocation_key FROM worker_create_operation_bindings WHERE workspace_id=?1 AND operation_id=?2",
        params![workspace, operation_or_allocation], |row| row.get(0),
    ).optional()?;
    Ok(bound.unwrap_or_else(|| operation_or_allocation.to_owned()))
}

fn valid_profile_bundle(workspace: &str, profile: &str, bundle: &ConfigBundle) -> bool {
    !profile.is_empty()
        && profile.trim() == profile
        && !profile.chars().any(char::is_control)
        && bundle.metadata.workspace_id == workspace
        && bundle.metadata.digest == bundle.computed_digest()
}

fn read_saved(
    conn: &Connection,
    workspace: &str,
    operation: &str,
    caller_fingerprint: &str,
) -> Result<Option<SavedBinding>> {
    let saved: Option<(String, String, String, String)> = conn.query_row(
        "SELECT allocation_key,caller_fingerprint,profile,config_bundle_json FROM worker_create_operation_bindings WHERE workspace_id=?1 AND operation_id=?2",
        params![workspace, operation],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let Some((allocation, fingerprint, profile, bundle)) = saved else {
        return Ok(None);
    };
    if fingerprint != caller_fingerprint {
        return Err(Error::RepositoryConflict(
            "creation operation reused with different full input".into(),
        ));
    }
    let initialization = feature_initialization::read_saved(conn, workspace, &allocation)?
        .ok_or_else(|| Error::Store("saved Feature launch has no create reservation".into()))?;
    let bundle: ConfigBundle = serde_json::from_str(&bundle)
        .map_err(|e| Error::Store(format!("saved Feature launch Profile bundle: {e}")))?;
    if !valid_profile_bundle(workspace, &profile, &bundle) {
        return Err(Error::Store(
            "saved Feature launch Profile bundle does not match its workspace or digest".into(),
        ));
    }
    Ok(Some(SavedBinding {
        allocation,
        launch: SavedFeatureWorkerLaunch {
            profile,
            bundle,
            initialization,
        },
    }))
}
