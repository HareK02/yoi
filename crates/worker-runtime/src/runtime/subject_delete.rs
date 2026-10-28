use super::*;

/// Permanent exclusion survives WorkerRemoval. A receipt never substitutes for a
/// fresh execution reconciliation on exact retry, including after Runtime reopen.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectDeleteWorkerFence {
    pub(crate) request: runtime_api::SubjectDeleteWorkerRequest,
    pub(crate) response: Option<runtime_api::SubjectDeleteWorkerResponse>,
}

impl Runtime {
    /// Erase only copies bound to an exact, completed canonical removal. The
    /// Subject fence already excludes execution; this does not mint erase authority
    /// for an arbitrary Worker or reinterpret the historical retention result.
    pub fn purge_subject_job_removal_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: runtime_api::SubjectJobRemovalPurgeRequest,
    ) -> Result<runtime_api::SubjectJobRemovalPurgeResponse, RuntimeError> {
        let deletion = &request.deletion;
        let removal = &request.removal;
        if deletion.workspace_id != scope.workspace_id
            || removal.workspace_id != deletion.workspace_id
            || removal.worker_id != deletion.worker_id
        {
            return Err(RuntimeError::InvalidRequest(
                "Subject Job removal scope mismatch".into(),
            ));
        }
        let lease = self.worker_operation_lock(deletion.worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let mut state = self.lock()?;
        state.ensure_running()?;
        state.ensure_workspace_owner(scope, false)?;
        if state.runtime_identity.as_deref() != Some(removal.source_runtime_id.as_str()) {
            return Err(RuntimeError::InvalidRequest(
                "Subject Job removal Runtime identity mismatch".into(),
            ));
        }
        state
            .subject_delete_worker_fences
            .get(&deletion.worker_id)
            .filter(|fence| fence.request == *deletion && fence.response.is_some())
            .ok_or_else(|| {
                RuntimeError::InvalidRequest(
                    "Exact confirmed Subject deletion fence required".into(),
                )
            })?;
        if state.workers.contains_key(&deletion.worker_id)
            || state
                .unreconciled_worker_writes
                .contains(&deletion.worker_id)
        {
            return Err(RuntimeError::InvalidRequest(
                "Subject Job canonical removal must be reconciled first".into(),
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            let store = state.fs_store().ok_or_else(|| {
                RuntimeError::InvalidRequest("Subject Job purge requires filesystem storage".into())
            })?;
            let removal: WorkerRetentionExecutionRequest = serde_json::from_value(
                serde_json::to_value(removal)
                    .map_err(|error| RuntimeError::InvalidRequest(error.to_string()))?,
            )
            .map_err(|error| RuntimeError::InvalidRequest(error.to_string()))?;
            // Worker -> Runtime -> retention -> Session is the canonical removal
            // lock order. The provider validates the receipt under its disk lock.
            FsWorkerRetentionProvider::new(store.runtime_dir())
                .purge_subject_job_removal(&removal)?;
            Ok(runtime_api::SubjectJobRemovalPurgeResponse {
                workspace_id: removal.workspace_id,
                worker_id: removal.worker_id,
                operation_id: removal.operation_id,
                input_fingerprint: removal.input_fingerprint,
            })
        }
        #[cfg(not(feature = "fs-store"))]
        Err(RuntimeError::InvalidRequest(
            "Subject Job purge requires filesystem storage".into(),
        ))
    }

    pub fn subject_delete_worker_scoped(
        &self,
        scope: &RuntimeWorkspaceScope,
        request: runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<runtime_api::SubjectDeleteWorkerResponse, RuntimeError> {
        if request.workspace_id != scope.workspace_id
            || request.create_fingerprint.trim().is_empty()
            || request.operation_id.trim().is_empty()
            || request.create_fingerprint.len() > 512
            || request.operation_id.len() > 512
        {
            return Err(RuntimeError::InvalidRequest(
                "invalid Subject delete Worker identity".into(),
            ));
        }
        let lease = self.worker_operation_lock(request.worker_id)?;
        let _operation = lease.lock().map_err(|_| RuntimeError::StatePoisoned)?;
        let worker_ref = WorkerRef::new(request.worker_id);
        let backend = {
            let mut state = self.lock()?;
            state.ensure_running()?;
            if state.backend == RuntimeBackendKind::Memory {
                return Err(RuntimeError::InvalidRequest(
                    "Subject delete requires durable storage".into(),
                ));
            }
            state.ensure_workspace_owner(scope, false)?;
            if state.workers.contains_key(&request.worker_id) {
                state.refresh_uncertain_worker_write(&worker_ref)?;
                let worker = state.worker(&worker_ref)?;
                if worker.workspace_id.as_deref() != Some(request.workspace_id.as_str())
                    || worker.request.as_ref().is_none_or(|create| {
                        create.create_fingerprint != request.create_fingerprint
                    })
                {
                    return Err(RuntimeError::InvalidRequest(
                        "Subject delete Worker identity mismatch".into(),
                    ));
                }
            }
            if let Some(recovery) = state.subject_create_recovery_fences.get(&request.worker_id)
                && (recovery.request.workspace_id != request.workspace_id
                    || recovery.request.create_fingerprint != request.create_fingerprint)
            {
                return Err(RuntimeError::InvalidRequest(
                    "Subject recovery identity mismatch".into(),
                ));
            }
            if state.subject_delete_worker_fences.values().any(|fence| {
                fence.request.workspace_id == request.workspace_id
                    && fence.request.operation_id == request.operation_id
                    && fence.request.worker_id != request.worker_id
            }) {
                return Err(RuntimeError::InvalidRequest(
                    "Subject delete operation reused for another Worker".into(),
                ));
            }
            state.execution_backend.clone()
        };
        // Unpublished execution identity is checked without holding the catalog
        // mutex. The Worker operation lock excludes Runtime create/restore.
        if let Some(backend) = backend.as_ref() {
            backend
                .preflight_subject_delete_worker(&request)
                .map_err(RuntimeError::InvalidRequest)?;
        }
        {
            let mut state = self.lock()?;
            state.ensure_workspace_owner(scope, false)?;
            if state.subject_delete_worker_fences.values().any(|fence| {
                fence.request.workspace_id == request.workspace_id
                    && fence.request.operation_id == request.operation_id
                    && fence.request.worker_id != request.worker_id
            }) {
                return Err(RuntimeError::InvalidRequest(
                    "Subject delete operation reused for another Worker".into(),
                ));
            }
            if let Some(saved) = state.subject_delete_worker_fences.get(&request.worker_id) {
                if saved.request != request {
                    return Err(RuntimeError::InvalidRequest(
                        "different Subject delete Worker request retained".into(),
                    ));
                }
            } else {
                state.ensure_workspace_owner(scope, true)?;
                state.subject_delete_worker_fences.insert(
                    request.worker_id,
                    SubjectDeleteWorkerFence {
                        request: request.clone(),
                        response: None,
                    },
                );
            }
            // Keep exclusion on persistence uncertainty; never start execution
            // cleanup until this durable write has been confirmed.
            state.persist_runtime_snapshot()?;
        }
        let backend = backend.ok_or_else(|| {
            RuntimeError::InvalidRequest(
                "Subject Worker shutdown cannot be confirmed without execution backend".into(),
            )
        })?;
        backend
            .subject_delete_worker(&request)
            .map_err(RuntimeError::InvalidRequest)?;
        let mut state = self.lock()?;
        if let Some(mut worker) = state.workers.get(&request.worker_id).cloned() {
            let restore_receipt =
                RuntimeWorkerRestoreResult::superseded_by_stop(worker.pending_restore);
            worker.pending_stop = None;
            worker.pending_restore = None;
            if let Some(context) = worker.restore_candidate_context.take() {
                context.discard_candidate();
            }
            worker.restore_guard.rotate();
            worker.restore_guard.finish(restore_receipt);
            worker.status = WorkerStatus::Stopped;
            worker.worker_state = None;
            worker.restore_intent = WorkerRestoreIntent::Explicit;
            worker.internal_workers.clear();
            worker.execution_bound = false;
            state.persist_worker_record(&worker)?;
            state.workers.insert(request.worker_id, worker);
            state.publish_worker_upsert(request.worker_id)?;
        }
        // This is storage-owner evidence under the same lock that permanently
        // excludes create/restore, not an inference from catalog projection.
        #[cfg(feature = "fs-store")]
        let originals_absent = !state.workers.contains_key(&request.worker_id)
            && state
                .fs_store()
                .ok_or_else(|| {
                    RuntimeError::InvalidRequest(
                        "Subject original absence requires fs storage".into(),
                    )
                })?
                .worker_originals_absent(request.worker_id)?;
        #[cfg(not(feature = "fs-store"))]
        let originals_absent = false;
        let response = runtime_api::SubjectDeleteWorkerResponse {
            workspace_id: request.workspace_id.clone(),
            worker_id: request.worker_id,
            create_fingerprint: request.create_fingerprint.clone(),
            operation_id: request.operation_id.clone(),
            originals_absent,
        };
        state
            .subject_delete_worker_fences
            .get_mut(&request.worker_id)
            .expect("delete fence retained under Worker operation lock")
            .response = Some(response.clone());
        state.persist_runtime_snapshot()?;
        Ok(response)
    }
}
