//! Resolved Feature data belongs to the create reservation, not caller intent.
//! NULL is retained legacy evidence, never an implicit empty connection set.
use super::*;

pub(super) const MIGRATION_NAME: &str = "Worker create resolved Feature initialization binding";
pub(super) const COLUMN_SCHEMA: &str = "ALTER TABLE worker_create_reservations ADD COLUMN feature_initialization_json TEXT
         CHECK (feature_initialization_json IS NULL OR
            (typeof(feature_initialization_json) = 'text' AND length(CAST(feature_initialization_json AS BLOB)) <= 131072
             AND json_valid(feature_initialization_json) AND json_type(feature_initialization_json) = 'array'));";

pub(super) fn migrate(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() || current_schema_version(conn)? != 141 {
        return Err(Error::Store(
            "Feature initialization migration requires schema 141".into(),
        ));
    }
    let tx = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Old requests cannot be reconstructed from their hashes. Preserve every old
    // hash and mark missing resolved data explicitly; owners must reconcile it.
    tx.execute_batch(COLUMN_SCHEMA)?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(142,?1)",
        [MIGRATION_NAME],
    )?;
    tx.commit()?;
    Ok(())
}

impl SqliteWorkspaceStore {
    /// Host-only saved evidence for replay before resolving current defaults.
    /// This neither authorizes creation nor checks caller input; the caller must
    /// validate the original operation and current owner/domain authority.
    pub(crate) fn saved_worker_feature_initialization(
        &self,
        workspace: &str,
        allocation: &str,
    ) -> Result<Option<FeatureInitializations>> {
        self.with_conn(|conn| read_saved(conn, workspace, allocation))
    }
}

pub(super) fn read_saved(
    conn: &Connection,
    workspace: &str,
    allocation: &str,
) -> Result<Option<FeatureInitializations>> {
    let saved = conn.query_row(
        "SELECT request_fingerprint,create_fingerprint,memory_language,feature_initialization_json,state,runtime_id,worker_id
         FROM worker_create_reservations WHERE workspace_id=?1 AND allocation_key=?2",
        params![workspace, allocation],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?, row.get::<_, String>(4)?,
            row.get::<_, String>(5)?, row.get::<_, String>(6)?)),
    ).optional()?;
    let Some((request, create, language, initialization, state, runtime, worker)) = saved else {
        return Ok(None);
    };
    if state == "removed" {
        return Err(Error::RepositoryConflict(
            "original Worker create allocation was terminally removed".into(),
        ));
    }
    let request = request
        .ok_or_else(|| Error::Store("original Worker create request fingerprint missing".into()))?;
    let memory = manifest::WorkspaceMemorySettingsSnapshot {
        workspace_id: workspace.into(),
        language: language
            .ok_or_else(|| Error::Store("original Worker create Memory snapshot missing".into()))?,
    };
    validate_workspace_memory_settings_snapshot(&memory, workspace)?;
    read_bound(
        conn,
        &RuntimeWorkerRef::new(runtime, worker),
        initialization.as_deref(),
        &request,
        &memory,
        &create,
    )
    .map(Some)
}

pub(super) fn read_bound(
    conn: &Connection,
    worker: &RuntimeWorkerRef,
    saved: Option<&str>,
    request_fingerprint: &str,
    memory: &manifest::WorkspaceMemorySettingsSnapshot,
    create_fingerprint: &str,
) -> Result<FeatureInitializations> {
    let saved = saved.ok_or_else(|| Error::RepositoryConflict(
        "worker_create_legacy_initialization_missing: original resolved Feature connections are unavailable; preserve the allocation and reconcile with its owner before replay; no replacement grant was issued".into(),
    ))?;
    let initialization: FeatureInitializations = serde_json::from_str(saved)
        .map_err(|_| Error::Store("saved Worker Feature initialization is invalid".into()))?;
    let canonical = serde_json::to_string(&initialization).map_err(|_| {
        Error::Store("saved Worker Feature initialization could not be encoded".into())
    })?;
    let base = bound_worker_create_fingerprint(request_fingerprint, memory, &canonical);
    // Plugin pinning seals an additional immutable layer on the same reservation.
    // Reuse that saved layer; never reselect a plan from current configuration.
    let bound = match crate::plugin_plan::worker_launch_plan(
        conn,
        &memory.workspace_id,
        &worker.runtime_id,
        &worker.worker_id,
    )? {
        Some(plan) => crate::plugin_plan::worker_launch_fingerprint(&base, &plan.fingerprint)?,
        None => base,
    };
    if bound != create_fingerprint {
        return Err(Error::RepositoryConflict(
            "saved Worker create fingerprint does not bind its resolved Feature initialization and Memory snapshot".into(),
        ));
    }
    Ok(initialization)
}

pub(super) fn require_same(
    saved: &FeatureInitializations,
    resolved: &FeatureInitializations,
) -> Result<()> {
    if saved != resolved {
        return Err(Error::RepositoryConflict(
            "Worker create allocation already has different resolved Feature initialization; reuse the saved resolution, not current defaults".into(),
        ));
    }
    Ok(())
}
