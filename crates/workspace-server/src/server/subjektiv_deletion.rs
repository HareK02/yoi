//! Explicit Subject destruction is independent of ordinary Worker retention. The domain
//! fence commits before external effects, remains after failures, and rejects cached grants.
//! Worker originals are archived; Job execution copies are purged. Existing archives, Workdirs,
//! Tickets and other Subjects are not erased. Archive contents can include previously injected
//! Memory: their independent retention/access policy still applies. This is logical content
//! deletion, not a promise to erase backups, external providers or already exported material.
//!
//! Child removal/create-fence receipts, not acceptance, Idle or missing projections, prove
//! quiescence. A retry carries the same scope/Subject/operation; no new child identity is issued.
//! The domain receipt is authoritative completion. Backend inventories retain only identifiers
//! needed for cleanup and fail-closed authorization, never a copy of Subject content.
use super::*;
use rusqlite::params;

pub(super) fn job_subject(request: &BackendJobRequest) -> Option<&str> {
    request
        .grants
        .subjektiv_journal
        .as_ref()
        .map(|g| g.subject_id.as_str())
        .or_else(|| {
            request
                .grants
                .subjektiv_consolidation
                .as_ref()
                .map(|g| g.subject_id.as_str())
        })
}

pub(super) fn require_available(api: &WorkspaceApi, subject: &str) -> Result<()> {
    let store = open_subjektiv_store(api).map_err(|error| error.error)?;
    if store
        .subject(subject)
        .map_err(|e| Error::Store(e.to_string()))?
        .is_none()
    {
        return Err(Error::SubjektivSubjectNotFound(subject.into()));
    }
    Ok(())
}

pub(super) fn pending(api: &WorkspaceApi, subject: &str) -> Result<bool> {
    api.config_store.with_conn(|connection| {
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM subject_deletions WHERE workspace_id=?1 AND subject_id=?2)",
            params![api.workspace_id(), subject], |row| row.get(0),
        )?)
    })
}

/// This is internal cleanup evidence, never a user-supplied removal flag. It exists only
/// after the independently durable domain fence and exact-target inventory commit.
pub(super) fn deleting_worker(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Result<bool> {
    api.config_store.with_conn(|connection| {
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM subject_deletion_workers WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3)",
            params![api.workspace_id(), worker.runtime_id, worker.worker_id], |row| row.get(0),
        )?)
    })
}

pub(super) async fn delete(
    api: WorkspaceApi,
    workspace: String,
    subject: String,
    request: server_api::SubjektivSubjectDeleteRequest,
) -> ApiResult<server_api::SubjektivSubjectDeleteResponse> {
    let handle = tokio::runtime::Handle::current();
    api.host_executors
        .host()?
        .run(move || {
            validate_workspace_scope(&api, &workspace)?;
            handle.block_on(resume(&api, &subject, &request.operation_id))
        })
        .await
}

pub(super) async fn recover(api: &WorkspaceApi) -> Result<()> {
    let pending = open_subjektiv_store(api)
        .map_err(|error| error.error)?
        .pending_subject_deletions()
        .map_err(|e| Error::Store(e.to_string()))?;
    for deletion in pending {
        if let Err(error) = resume(api, &deletion.subject_id, &deletion.operation_id).await {
            tracing::warn!(%error.error, subject_id=%deletion.subject_id,
                "Subject deletion restart recovery remains incomplete; same request can resume");
        }
    }
    Ok(())
}

async fn resume(
    api: &WorkspaceApi,
    subject: &str,
    operation: &str,
) -> ApiResult<server_api::SubjektivSubjectDeleteResponse> {
    let store = open_subjektiv_store(api)?;
    let gate = subjektiv_journal::lifecycle_lock(api, subject)?;
    let receipt = {
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Subject lifecycle lock poisoned".into()))?;
        let receipt = store
            .begin_subject_deletion(subject, operation)
            .map_err(subjektiv_store_error)?;
        if !receipt.completed {
            let delegated = store
                .deletion_subject_workers(subject)
                .map_err(subjektiv_store_error)?
                .into_iter()
                .map(|(runtime, worker)| RuntimeWorkerRef::new(runtime, worker))
                .collect::<Vec<_>>();
            api.config_store.prepare_subject_deletion(
                api.workspace_id(),
                subject,
                operation,
                &delegated,
            )?;
        }
        receipt
    };
    let response = |completed| server_api::SubjektivSubjectDeleteResponse {
        subject_id: subject.into(),
        operation_id: operation.into(),
        completed,
    };
    if receipt.completed {
        return Ok(response(true));
    }
    // Never hold the Host callback gate across a Runtime call. A create/restore can be
    // waiting for the callback that now has to reject this permanently fenced Subject.
    let workers =
        api.config_store
            .list_subject_deletion_workers(api.workspace_id(), subject, operation)?;
    for (worker, _is_job) in workers {
        if let Err(error) = remove_worker(api, subject, operation, &worker).await {
            tracing::warn!(%error.error, subject_id=subject, operation_id=operation, "Subject deletion remains incomplete; exact retry required");
            return Ok(response(false));
        }
    }
    {
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Subject lifecycle lock poisoned".into()))?;
        api.config_store
            .erase_subject_deleted_jobs(api.workspace_id(), subject, operation)?;
        // Finish Backend cleanup before the authoritative domain receipt. If domain
        // purge fails, its separately committed fence and pending operation survive.
        // No retry can report domain completion while occupancy/Job cleanup is unproven.
        api.config_store
            .finish_subject_deletion_record(api.workspace_id(), subject, operation)?;
        store
            .finish_subject_deletion(subject, operation)
            .map_err(subjektiv_store_error)?;
    }
    Ok(response(true))
}

async fn remove_worker(
    api: &WorkspaceApi,
    subject: &str,
    operation: &str,
    worker: &RuntimeWorkerRef,
) -> ApiResult<()> {
    let request = api.config_store.subject_deletion_worker_request(
        api.workspace_id(),
        subject,
        operation,
        worker,
    )?;
    let fence = api
        .runtime
        .subject_delete_worker(&worker.runtime_id, request.clone())
        .map_err(RuntimeRegistryError::into_error)?;
    if fence.workspace_id != request.workspace_id
        || fence.worker_id != request.worker_id
        || fence.create_fingerprint != request.create_fingerprint
        || fence.operation_id != request.operation_id
    {
        return Err(Error::RepositoryConflict(
            "Subject Worker deletion fence receipt mismatch".into(),
        )
        .into());
    }
    if let Some(reconciliation) = api
        .config_store
        .subject_deletion_removal(api.workspace_id(), worker)?
    {
        if reconciliation.completed {
            return Ok(());
        }
        // First reconcile the original canonical result, without changing any old
        // disposition, fingerprint or success snapshot. A lost Runtime response or
        // Backend commit remains unknown until that same child succeeds.
        execute_removal(api, operation, worker).await?;
        let result = api
            .runtime
            .execute_worker_retention(worker, reconciliation.request.clone())
            .map_err(RuntimeRegistryError::into_error)?;
        let purge = if reconciliation.is_job {
            Some(
                api.runtime
                    .purge_subject_job_removal(
                        &worker.runtime_id,
                        request,
                        reconciliation.request.clone(),
                    )
                    .map_err(RuntimeRegistryError::into_error)?,
            )
        } else {
            None
        };
        // The extra physical Job erasure is a distinct receipt. An old Archive
        // success is never counted as a Purge, nor pinned as an older success later.
        api.config_store.confirm_subject_deletion_removal(
            api.workspace_id(),
            worker,
            &reconciliation.request,
            &result,
            purge.as_ref(),
        )?;
        return Ok(());
    }
    // Replay the canonical child even when physical removal already happened:
    // its retained receipt can finish the Backend catalog midpoint. The Runtime
    // fence above also prevents an older create from arriving after that removal.
    if api
        .config_store
        .recover_worker_removal_execution(api.workspace_id(), worker)
        .map_err(|error| Error::Store(error.to_string()))?
        .is_some()
    {
        return execute_removal(api, operation, worker).await;
    }
    match api.runtime.worker(worker) {
        Ok(detail) => {
            if !detail.state.eq_ignore_ascii_case("stopped") {
                let stopped = api
                    .runtime
                    .stop_worker(
                        worker,
                        WorkerLifecycleRequest {
                            reason: Some(format!("Explicit Subject deletion {operation}")),
                            ticket_assignment: None,
                        },
                    )
                    .map_err(RuntimeRegistryError::into_error)?;
                if stopped.state != InternalWorkerOperationState::Accepted {
                    return Err(Error::RepositoryConflict(
                        "Subject Worker stop was not accepted".into(),
                    )
                    .into());
                }
            }
            let actual = api
                .runtime
                .worker(worker)
                .map_err(RuntimeRegistryError::into_error)?;
            if !actual.state.eq_ignore_ascii_case("stopped") {
                return Err(
                    Error::RepositoryConflict("Subject Worker stop is unconfirmed".into()).into(),
                );
            }
            if api
                .store
                .get_worker_registry(api.workspace_id(), worker)?
                .is_none()
            {
                // The Runtime fence authenticated this exact reserved fingerprint. A create
                // can have reached Runtime before its Backend publication failed. Publish
                // only its stopped record so the normal archive/Purge owner can remove it.
                record_worker_summary(
                    api,
                    &actual,
                    &actual.display_name,
                    actual.profile.clone(),
                    WorkerRegistryDisplayNamePolicy::PreserveExisting,
                )?;
            }
        }
        Err(RuntimeRegistryError::UnknownWorker { .. }) => {
            if api
                .store
                .get_worker_registry(api.workspace_id(), worker)?
                .is_some()
            {
                return Err(Error::RepositoryConflict(
                    "Runtime object missing before canonical original archival".into(),
                )
                .into());
            }
            api.config_store.confirm_subject_deletion_absence(
                api.workspace_id(),
                subject,
                operation,
                worker,
                &fence,
            )?;
            return Ok(());
        }
        Err(error) => return Err(error.into_error().into()),
    }
    execute_removal(api, operation, worker).await
}

async fn execute_removal(
    api: &WorkspaceApi,
    operation: &str,
    worker: &RuntimeWorkerRef,
) -> ApiResult<()> {
    let response = WorkerRemovalService::new(api)
        .execute_target_removal_on_host(
            api.runtime.as_ref(),
            worker,
            &format!("Explicit Subject deletion {operation}"),
            None,
        )
        .await
        .map_err(Error::Store)?;
    if response.status != StatusCode::OK.as_u16() {
        return Err(Error::RepositoryConflict(
            "Subject Worker archival/Purge remains unconfirmed".into(),
        )
        .into());
    }
    Ok(())
}
