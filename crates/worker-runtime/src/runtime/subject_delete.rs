use super::*;

impl RuntimeState {
    // Absence is usable only after this exact Subject operation confirmed the
    // original scope and cleanup. A new missing catalog ID is never proof.
    fn confirmed_subject_delete_originals_absent(
        &self,
        request: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<bool, RuntimeError> {
        if self.workers.contains_key(&request.worker_id) {
            return Ok(false);
        }
        if self.unreconciled_worker_writes.contains(&request.worker_id) {
            return Err(RuntimeError::InvalidRequest(
                "Subject Worker write remains uncertain".into(),
            ));
        }
        self.subject_delete_worker_fences
            .get(&request.worker_id)
            .filter(|fence| fence.request == *request)
            .and_then(|fence| fence.response.as_ref())
            .filter(|saved| {
                saved.workspace_id == request.workspace_id
                    && saved.worker_id == request.worker_id
                    && saved.create_fingerprint == request.create_fingerprint
                    && saved.operation_id == request.operation_id
            })
            .ok_or_else(|| {
                RuntimeError::InvalidRequest("Missing original confirmed Subject shutdown".into())
            })?;
        #[cfg(feature = "fs-store")]
        if self
            .fs_store()
            .ok_or_else(|| {
                RuntimeError::InvalidRequest("Subject original absence requires fs storage".into())
            })?
            .worker_originals_absent(request.worker_id)?
        {
            return Ok(true);
        }
        Err(RuntimeError::InvalidRequest(
            "Subject Worker original material remains or cannot be inspected".into(),
        ))
    }
}

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
            .fence_subject_delete_worker(&request)
            .map_err(RuntimeError::InvalidRequest)?;
        if self
            .lock()?
            .confirmed_subject_delete_originals_absent(&request)?
        {
            // The durable confirmed fence owns the historical shutdown proof;
            // this separately checks current adapter resources after removal.
            backend
                .confirm_subject_delete_worker_absent(&request)
                .map_err(RuntimeError::InvalidRequest)?;
            let mut state = self.lock()?;
            if !state.confirmed_subject_delete_originals_absent(&request)? {
                return Err(RuntimeError::InvalidRequest(
                    "Subject originals changed during absence reconciliation".into(),
                ));
            }
            let response = runtime_api::SubjectDeleteWorkerResponse {
                workspace_id: request.workspace_id.clone(),
                worker_id: request.worker_id,
                create_fingerprint: request.create_fingerprint.clone(),
                operation_id: request.operation_id.clone(),
                originals_absent: true,
            };
            state
                .subject_delete_worker_fences
                .get_mut(&request.worker_id)
                .expect("confirmed fence retained under Worker lock")
                .response = Some(response.clone());
            state.persist_runtime_snapshot()?;
            return Ok(response);
        }
        let (stop, canonical, pending_stop) = {
            let mut state = self.lock()?;
            // A prior uncertain Stop write must be synchronized before it can
            // supply evidence or choose the durable child for this retry.
            state.refresh_uncertain_worker_write(&worker_ref)?;
            let worker = state.worker(&worker_ref)?;
            if let Some(pending) = worker.pending_stop {
                (
                    WorkerExecutionStopRequest {
                        worker_ref: worker_ref.clone(),
                        operation_id: pending.operation_id,
                    },
                    worker.restore_guard.execution_termination.clone(),
                    true,
                )
            } else if let Some(saved) = worker.restore_guard.execution_termination.clone() {
                if worker.status != WorkerStatus::Stopped || worker.pending_restore.is_some() {
                    return Err(RuntimeError::InvalidRequest(
                        "Subject canonical Stop is not settled".into(),
                    ));
                }
                (
                    WorkerExecutionStopRequest {
                        worker_ref: worker_ref.clone(),
                        operation_id: saved.stop_operation_id,
                    },
                    Some(saved),
                    false,
                )
            } else {
                drop(state);
                // Never call begin on an existing canonical receipt: it clears
                // the old proof. No-record/never-started cases cannot get here
                // through an empty backend map as evidence of historical shutdown.
                let operation_id = WorkerLifecycleOperationId::new();
                self.begin_stop_operation(&worker_ref, operation_id)?;
                (
                    WorkerExecutionStopRequest {
                        worker_ref: worker_ref.clone(),
                        operation_id,
                    },
                    None,
                    true,
                )
            }
        };
        let result = backend
            .subject_delete_worker_stop(&request, stop.clone(), canonical.as_ref())
            .map_err(RuntimeError::InvalidRequest)?;
        if let Some(saved) = canonical {
            if !result.is_accepted()
                || result.operation != WorkerExecutionOperation::Stop
                || result.execution_id != Some(saved.execution_id)
                || result.termination.as_ref() != Some(&saved)
                || !saved.confirms_scope_shutdown(&worker_ref, stop.operation_id)
            {
                return Err(RuntimeError::InvalidRequest(
                    "Subject canonical Stop was not exactly reconciled".into(),
                ));
            }
        }
        if pending_stop {
            // Settled A is read-only; pending B must settle through the ordinary
            // validator only after cleanup reconciles its original whole receipt.
            self.commit_stopped_worker(&worker_ref, stop.operation_id, result)?;
        }
        let mut state = self.lock()?;
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
