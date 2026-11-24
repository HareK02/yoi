use super::*;

#[cfg(feature = "fs-store")]
use crate::retention::{SubjectRemovalArchiveValidation, SubjectRemovalReceipt};

#[derive(PartialEq, Eq)]
enum SubjectOriginals {
    Present,
    Absent,
    #[cfg(feature = "fs-store")]
    PendingRemoval(WorkerRetentionExecutionRequest),
}

impl RuntimeState {
    // Canonical removal owns completion. A new missing catalog ID is not a
    // completed removal, and cannot be upgraded by an empty backend registry.
    fn confirmed_subject_delete_originals_absent(
        &mut self,
        request: &runtime_api::SubjectDeleteWorkerRequest,
    ) -> Result<SubjectOriginals, RuntimeError> {
        if self.workers.contains_key(&request.worker_id) {
            return Ok(SubjectOriginals::Present);
        }
        if self.unreconciled_worker_writes.contains(&request.worker_id) {
            return Err(RuntimeError::InvalidRequest(
                "Subject Worker write remains uncertain".into(),
            ));
        }
        #[cfg(feature = "fs-store")]
        {
            let fence = self
                .subject_delete_worker_fences
                .get(&request.worker_id)
                .filter(|fence| fence.request == *request)
                .ok_or_else(|| {
                    RuntimeError::InvalidRequest("Missing exact Subject fence".into())
                })?;
            let store = self.fs_store().ok_or_else(|| {
                RuntimeError::InvalidRequest("Subject original absence requires fs storage".into())
            })?;
            if !store.worker_originals_absent(request.worker_id)? {
                return Err(RuntimeError::InvalidRequest(
                    "Subject Worker original material remains".into(),
                ));
            }
            // Preserve confirmation-before-removal replay. A fence confirmed
            // through retention history must instead reread its original child
            // every time; its response is not a substitute for that receipt.
            let confirmed_before_removal = fence.canonical_removal.is_none()
                && fence.response.as_ref().is_some_and(|saved| {
                    saved.workspace_id == request.workspace_id
                        && saved.worker_id == request.worker_id
                        && saved.create_fingerprint == request.create_fingerprint
                        && saved.operation_id == request.operation_id
                        && saved.completed_removal.is_none()
                });
            let pending: Option<WorkerRetentionExecutionRequest> = fence
                .response
                .as_ref()
                .and_then(|response| response.pending_removal.as_ref())
                .map(|request| {
                    serde_json::from_value(
                        serde_json::to_value(request)
                            .map_err(|e| RuntimeError::InvalidRequest(e.to_string()))?,
                    )
                    .map_err(|e| RuntimeError::InvalidRequest(e.to_string()))
                })
                .transpose()?;
            let observed = FsWorkerRetentionProvider::new(store.runtime_dir())
                .subject_removal_if_present(
                    request,
                    self.runtime_identity.as_deref(),
                    fence
                        .canonical_removal
                        .as_ref()
                        .map(|child| (&child.request, child.receipt_sha256.as_str())),
                    pending.as_ref(),
                    SubjectRemovalArchiveValidation::RetainedMaterial,
                )?;
            let (removal, receipt_sha256) = match observed {
                Some(SubjectRemovalReceipt::Completed {
                    request,
                    receipt_sha256,
                }) => (request, receipt_sha256),
                Some(SubjectRemovalReceipt::Pending(request)) if confirmed_before_removal => {
                    return Ok(SubjectOriginals::PendingRemoval(request));
                }
                // Confirmed direct deletion needs no Runtime retention identity
                // when discovery proves that no child exists. Never skip discovery.
                None if confirmed_before_removal => return Ok(SubjectOriginals::Absent),
                _ => {
                    return Err(RuntimeError::InvalidRequest(
                        "Missing or mismatched completed canonical removal receipt".into(),
                    ));
                }
            };
            self.subject_delete_worker_fences
                .get_mut(&request.worker_id)
                .expect("exact Subject fence held under Worker lock")
                .canonical_removal = Some(SubjectCanonicalRemoval {
                request: removal,
                receipt_sha256,
            });
            // Bind the original child before external resource cleanup. Retry
            // also synchronizes a prior uncertain snapshot acknowledgement.
            self.persist_runtime_snapshot()?;
            return Ok(SubjectOriginals::Absent);
        }
        #[cfg(not(feature = "fs-store"))]
        Err(RuntimeError::InvalidRequest(
            "Subject Worker original material remains or cannot be inspected".into(),
        ))
    }
}

#[cfg(feature = "fs-store")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectCanonicalRemoval {
    request: WorkerRetentionExecutionRequest,
    receipt_sha256: String,
}

/// Permanent exclusion survives WorkerRemoval. Exact retry checks the original
/// removal identity, not a second historical execution shutdown proof.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubjectDeleteWorkerFence {
    pub(crate) request: runtime_api::SubjectDeleteWorkerRequest,
    pub(crate) response: Option<runtime_api::SubjectDeleteWorkerResponse>,
    // Child identity only; completion remains owned by the removal receipt.
    #[cfg(feature = "fs-store")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) canonical_removal: Option<SubjectCanonicalRemoval>,
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
        let fence = state
            .subject_delete_worker_fences
            .get(&deletion.worker_id)
            .filter(|fence| fence.request == *deletion && fence.response.is_some())
            .ok_or_else(|| {
                RuntimeError::InvalidRequest(
                    "Exact confirmed Subject deletion fence required".into(),
                )
            })?;
        if fence
            .response
            .as_ref()
            .and_then(|response| response.pending_removal.as_ref())
            .is_some_and(|pending| pending != removal)
        {
            return Err(RuntimeError::InvalidRequest(
                "Subject Job purge pending child mismatch".into(),
            ));
        }
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
            // Recheck generation and the parent-bound receipt digest immediately
            // before erasure; a past fence response is not canonical success.
            let provider = FsWorkerRetentionProvider::new(store.runtime_dir());
            let child = state
                .subject_delete_worker_fences
                .get(&deletion.worker_id)
                .and_then(|fence| fence.canonical_removal.as_ref());
            let observed = provider.subject_removal_if_present(
                deletion,
                Some(&removal.source_runtime_id),
                child.map(|child| (&child.request, child.receipt_sha256.as_str())),
                None,
                SubjectRemovalArchiveValidation::ExplicitPurgeIdentity,
            )?;
            let Some(SubjectRemovalReceipt::Completed {
                request: completed,
                receipt_sha256,
            }) = observed
            else {
                return Err(RuntimeError::InvalidRequest(
                    "Subject Job purge requires completed child".into(),
                ));
            };
            if completed != removal {
                return Err(RuntimeError::InvalidRequest(
                    "Subject Job purge canonical child mismatch".into(),
                ));
            }
            state
                .subject_delete_worker_fences
                .get_mut(&deletion.worker_id)
                .expect("confirmed Subject fence held under Worker lock")
                .canonical_removal = Some(SubjectCanonicalRemoval {
                request: completed,
                receipt_sha256,
            });
            state.persist_runtime_snapshot()?;
            // Worker -> Runtime -> retention -> Session is the canonical removal
            // lock order. The provider validates the receipt under its disk lock.
            provider.purge_subject_job_removal(&removal)?;
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
                // WorkerId identifies the allocation even when a settled legacy
                // aggregate lacks restore material. Compare every retained
                // fingerprint, but do not require or invent a restore request.
                if worker.workspace_id.as_deref() != Some(request.workspace_id.as_str())
                    || worker.request.as_ref().is_some_and(|create| {
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
                        #[cfg(feature = "fs-store")]
                        canonical_removal: None,
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
        let absent = {
            let mut state = self.lock()?;
            state.confirmed_subject_delete_originals_absent(&request)?
        };
        if absent != SubjectOriginals::Present {
            // Historical shutdown comes from completed removal or the previously
            // confirmed fence, never from a pending receipt. Check only current
            // resource owners and remaining factory cleanup here.
            backend
                .confirm_subject_delete_worker_absent(&request)
                .map_err(RuntimeError::InvalidRequest)?;
            let mut state = self.lock()?;
            if state.confirmed_subject_delete_originals_absent(&request)? != absent {
                return Err(RuntimeError::InvalidRequest(
                    "Subject originals changed during absence reconciliation".into(),
                ));
            }
            #[cfg(feature = "fs-store")]
            let completed_removal = state
                .subject_delete_worker_fences
                .get(&request.worker_id)
                .and_then(|fence| fence.canonical_removal.as_ref())
                .map(|child| {
                    serde_json::from_value(
                        serde_json::to_value(&child.request)
                            .map_err(|error| RuntimeError::InvalidRequest(error.to_string()))?,
                    )
                    .map_err(|error| RuntimeError::InvalidRequest(error.to_string()))
                })
                .transpose()?;
            #[cfg(not(feature = "fs-store"))]
            let completed_removal = None;
            #[cfg(feature = "fs-store")]
            let pending_removal = match &absent {
                SubjectOriginals::PendingRemoval(request) => Some(
                    serde_json::from_value(
                        serde_json::to_value(request)
                            .map_err(|e| RuntimeError::InvalidRequest(e.to_string()))?,
                    )
                    .map_err(|e| RuntimeError::InvalidRequest(e.to_string()))?,
                ),
                _ => None,
            };
            #[cfg(not(feature = "fs-store"))]
            let pending_removal = None;
            let response = runtime_api::SubjectDeleteWorkerResponse {
                workspace_id: request.workspace_id.clone(),
                worker_id: request.worker_id,
                create_fingerprint: request.create_fingerprint.clone(),
                operation_id: request.operation_id.clone(),
                originals_absent: absent == SubjectOriginals::Absent,
                completed_removal,
                pending_removal,
            };
            state
                .subject_delete_worker_fences
                .get_mut(&request.worker_id)
                .expect("confirmed fence retained under Worker lock")
                .response = Some(response.clone());
            state.persist_runtime_snapshot()?;
            return Ok(response);
        }
        let stop = {
            let mut state = self.lock()?;
            state.refresh_uncertain_worker_write(&worker_ref)?;
            let worker = state.worker(&worker_ref)?;
            if worker.is_settled_inactive() {
                // Both NotStarted and Stopped are authoritative. In particular,
                // NotStarted has no execution or Stop receipt to manufacture.
                None
            } else if let Some(pending) = worker.pending_stop() {
                Some((
                    WorkerExecutionStopRequest {
                        worker_ref: worker_ref.clone(),
                        operation_id: pending.operation_id,
                    },
                    worker.stop_termination().cloned(),
                ))
            } else {
                drop(state);
                let operation_id = WorkerLifecycleOperationId::new();
                self.begin_stop_operation(&worker_ref, operation_id)?;
                Some((
                    WorkerExecutionStopRequest {
                        worker_ref: worker_ref.clone(),
                        operation_id,
                    },
                    None,
                ))
            }
        };
        if let Some((stop, termination)) = stop {
            // This invocation owns the in-memory Stop. A later deletion retry
            // rechecks resources without resurrecting a saved Stop operation.
            let result = backend
                .subject_delete_worker_stop(&request, stop.clone(), termination.as_ref())
                .unwrap_or_else(|message| {
                    WorkerExecutionResult::errored(WorkerExecutionOperation::Stop, message)
                });
            self.commit_stopped_worker(&worker_ref, stop.operation_id, result)?;
        } else {
            // A settled lifecycle authorizes removal without Stop. Admission is
            // already fenced; reject a conflicting current candidate/resource
            // owner without interpreting an older Stop as its termination.
            let saved = self
                .lock()?
                .worker(&worker_ref)?
                .restore_guard
                .execution_termination
                .clone();
            backend
                .cleanup_stopped_worker(&worker_ref, saved.as_ref())
                .map_err(RuntimeError::InvalidRequest)?;
            backend
                .preflight_worker_removal(&worker_ref)
                .map_err(RuntimeError::InvalidRequest)?;
            // A prior Stop may have terminated execution but failed to save it.
            // Retry only the settled facts before confirming durable deletion;
            // never reconstruct a Stop or obtain another join receipt.
            let mut state = self.lock()?;
            let settled = state.worker(&worker_ref)?.clone();
            state.persist_worker_record_with_fence(&settled)?;
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
            completed_removal: None,
            pending_removal: None,
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
