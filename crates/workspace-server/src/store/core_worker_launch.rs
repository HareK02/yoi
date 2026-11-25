//! The resolved bundle and grant bind atomically to the existing creation reservation.
use super::*;
use server_api::authorization::CoreWorkerExecutionGrant;

impl SqliteWorkspaceStore {
    pub(crate) fn saved_core_worker_launch(
        &self,
        workspace: &str,
        operation: &str,
        caller_fingerprint: &str,
    ) -> Result<
        Option<(
            CoreWorkerExecutionGrant,
            worker_runtime::config_bundle::ConfigBundle,
        )>,
    > {
        self.with_conn(|conn| {
            let row: Option<(String, String, String, String)> = conn.query_row(
                "SELECT caller_fingerprint,runtime_id,worker_id,config_bundle_json FROM core_worker_launch_bindings WHERE workspace_id=?1 AND operation_id=?2",
                params![workspace, operation], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).optional()?;
            let Some((fingerprint, runtime, worker, bundle)) = row else { return Ok(None); };
            if fingerprint != caller_fingerprint { return Err(Error::RepositoryConflict("creation operation reused with different full input".into())); }
            feature_initialization::read_saved(conn, workspace, operation)?
                .ok_or_else(|| Error::Store("saved launch has no create reservation".into()))?;
            let grant = crate::launch_policy::core_grant(conn, workspace, &runtime, &worker)?.ok_or_else(|| Error::Store("saved launch has no execution grant".into()))?;
            let bundle: worker_runtime::config_bundle::ConfigBundle = serde_json::from_str(&bundle).map_err(|e| Error::Store(format!("saved launch Profile bundle: {e}")))?;
            if grant.creation_operation_id != operation || grant.profile_digest != bundle.computed_digest()
                || bundle.metadata.workspace_id != workspace
            {
                return Err(Error::Store("saved launch Profile binding does not match its grant".into()));
            }
            Ok(Some((grant, bundle)))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reserve_worker_create_with_core(
        &self,
        workspace: &str,
        runtime: &str,
        operation: &str,
        request_fingerprint: &str,
        caller_fingerprint: &str,
        singleton: Option<&str>,
        draft: &CoreWorkerExecutionGrant,
        bundle: &worker_runtime::config_bundle::ConfigBundle,
    ) -> Result<WorkerCreateReservation> {
        self.reserve_worker_create_with_core_initialization(
            workspace,
            runtime,
            operation,
            request_fingerprint,
            caller_fingerprint,
            singleton,
            draft,
            bundle,
            &FeatureInitializations::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reserve_worker_create_with_core_initialization(
        &self,
        workspace: &str,
        runtime: &str,
        operation: &str,
        request_fingerprint: &str,
        caller_fingerprint: &str,
        singleton: Option<&str>,
        draft: &CoreWorkerExecutionGrant,
        bundle: &worker_runtime::config_bundle::ConfigBundle,
        feature_initialization: &FeatureInitializations,
    ) -> Result<WorkerCreateReservation> {
        if draft.workspace_id != workspace
            || draft.runtime_id != runtime
            || draft.creation_operation_id != operation
            || draft.profile_digest != bundle.computed_digest()
            || bundle.metadata.workspace_id != workspace
        {
            return Err(Error::InvalidInput(
                "resolved launch identity or Profile bundle mismatch".into(),
            ));
        }
        self.reserve_worker_create_with(workspace, runtime, operation, request_fingerprint, singleton, feature_initialization,
            |tx| {
                let existing: Option<(Option<String>, Option<String>, String)> = tx.query_row(
                    "SELECT b.caller_fingerprint,b.config_bundle_json,r.worker_id FROM worker_create_reservations r LEFT JOIN core_worker_launch_bindings b ON b.workspace_id=r.workspace_id AND b.operation_id=r.allocation_key WHERE r.workspace_id=?1 AND r.allocation_key=?2",
                    params![workspace, operation], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
                ).optional()?;
                if let Some((fingerprint, saved_bundle, worker)) = existing {
                    let saved_grant = crate::launch_policy::core_grant(tx, workspace, runtime, &worker)?;
                    let mut expected = draft.clone();
                    expected.worker_id = worker;
                    // Reservation replay compares raw saved authority, not its
                    // current effective permission projection.
                    if fingerprint.as_deref() != Some(caller_fingerprint)
                        || saved_bundle.as_deref() != Some(serde_json::to_string(bundle).map_err(|e| Error::Store(e.to_string()))?.as_str())
                        || saved_grant.as_ref() != Some(&expected)
                    {
                        return Err(Error::RepositoryConflict("Worker launch reservation already has different or missing resolved authority; reread the saved launch".into()));
                    }
                }
                Ok(None)
            }, |tx, reservation| {
                let mut grant = draft.clone();
                grant.worker_id = reservation.worker_id.to_string();
                crate::launch_policy::bind_core_grant(tx, &grant)?;
                tx.execute("INSERT INTO core_worker_launch_bindings(workspace_id,operation_id,caller_fingerprint,runtime_id,worker_id,config_bundle_json) VALUES(?1,?2,?3,?4,?5,?6)",
                    params![workspace, operation, caller_fingerprint, runtime, grant.worker_id, serde_json::to_string(bundle).map_err(|e| Error::Store(e.to_string()))?])?;
                Ok(())
            })
    }
}
