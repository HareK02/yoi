//! Approval drives publication without a Subject stamp or a model follow-up.
//! Claims and Runtime receipts separate new effects from recovery. Background
//! rounds are bounded and target exclusion is owned by the MR transaction, not
//! by a schedule, Worker liveness, or a process-local lock.
use super::*;
use merge_request::{Publication, PublicationBinding, PublicationOutcome};
use worker_runtime::catalog::{
    RepositoryPublicationBinding, RepositoryPublicationOutcome, RepositoryPublicationReceipt,
    RepositoryPublicationRequest,
};

pub(super) fn public(publication: Publication) -> server_api::MergeRequestPublication {
    let b = publication.binding;
    server_api::MergeRequestPublication {
        operation_id: b.operation_id,
        merge_request_id: b.merge_request_id,
        approval_event_id: b.approval_event_id,
        source_ref: b.source,
        target_selector: b.target,
        target_ref_before: b.target_before,
        outcome: match publication.outcome {
            PublicationOutcome::Unconfirmed {} => {
                server_api::MergeRequestPublicationOutcome::Unconfirmed {}
            }
            PublicationOutcome::NotPublished { reason } => {
                server_api::MergeRequestPublicationOutcome::NotPublished { reason }
            }
            PublicationOutcome::Published {
                target_after,
                observed_at,
            } => server_api::MergeRequestPublicationOutcome::Published {
                target_after,
                observed_at,
            },
        },
    }
}

fn runtime_binding(b: &PublicationBinding) -> ApiResult<RepositoryPublicationBinding> {
    Ok(RepositoryPublicationBinding {
        workspace_id: b.workspace_id.clone(),
        merge_request_id: b.merge_request_id.clone(),
        operation_id: b.operation_id.clone(),
        repository_id: b.repository_id.clone(),
        repository_source_fingerprint: b.repository_source_fingerprint.clone(),
        target: b.target.clone(),
        source: b.source.clone(),
        approval_event_id: b.approval_event_id.clone(),
        owner_operation_id: b.owner_operation_id.clone(),
        worker: RuntimeWorkerRef::new(&b.worker.runtime_id, &b.worker.worker_id),
        working_directory_id: b.working_directory_id.clone(),
        connection_id: b.connection_id.clone(),
        alias: workdir::WorkdirAttachmentAlias::new(b.alias.clone())
            .map_err(|e| Error::InvalidInput(e.to_string()))?,
        target_before: b.target_before.clone(),
    })
}

fn request(
    api: &WorkspaceApi,
    binding: &PublicationBinding,
) -> ApiResult<RepositoryPublicationRequest> {
    let repository = api.require_configured_workspace_repository(&binding.repository_id)?;
    if repository.source_fingerprint != binding.repository_source_fingerprint {
        return Err(Error::WorkspacePermissionDenied(
            "publication Repository source changed".into(),
        )
        .into());
    }
    let projection = active_repository_access_projection(api, api.workspace_id())?;
    let mut request = working_directory_request_from_repository(&repository, Some(&binding.target));
    authorize_repository_materialization(
        api,
        &binding.worker.runtime_id,
        &binding.operation_id,
        &projection,
        &mut request,
    )?;
    Ok(RepositoryPublicationRequest {
        binding: runtime_binding(binding)?,
        repository: request.repository,
        materialization: request.materialization.ok_or_else(|| {
            Error::WorkspacePermissionDenied("publication Repository authority unavailable".into())
        })?,
    })
}

fn authorize_dispatch(api: &WorkspaceApi, b: &PublicationBinding) -> ApiResult<()> {
    let worker = RuntimeWorkerRef::new(&b.worker.runtime_id, &b.worker.worker_id);
    require_mr_operation(api, &worker, &b.merge_request_id, "merge_request.publish")?;
    let live = api
        .store
        .list_worker_workdir_links(api.workspace_id(), &worker)?
        .into_iter()
        .find(|l| {
            l.unlinked_at.is_none()
                && l.workdir_id == b.working_directory_id
                && l.alias == b.alias
                && l.connection_id == b.connection_id
        })
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("publication Workdir connection changed".into())
        })?;
    let record = api
        .store
        .get_workdir_registry(api.workspace_id(), &b.working_directory_id)?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied("publication Workdir is unavailable".into())
        })?;
    if !matches!(&record.source, WorkdirRegistrySource::Repository { runtime_id, repository_id } if runtime_id == &worker.runtime_id && repository_id == &b.repository_id)
    {
        return Err(Error::WorkspacePermissionDenied(
            "publication Workdir Repository mismatch".into(),
        )
        .into());
    }
    let capabilities = live
        .capabilities
        .intersection(workdir_source_capabilities(api, &record)?);
    if capabilities.intersection(workdir::WorkdirSessionCapabilities::COMMAND_ONLY)
        != workdir::WorkdirSessionCapabilities::COMMAND_ONLY
    {
        return Err(Error::WorkspacePermissionDenied(
            "publication requires command-capable Workdir".into(),
        )
        .into());
    }
    for operation in ["bash.execute", "filesystem.read", "filesystem.write"] {
        subject_authorization::authorize_scope(
            api,
            &worker,
            &server_api::authorization::OperationAuthorizationRequest {
                operation: operation.into(),
                resource: server_api::authorization::PermissionResource::Workdir {
                    working_directory_id: b.working_directory_id.clone(),
                    path: ".".into(),
                    recursive: true,
                },
            },
        )?
        .ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "publication requires explicit Workdir authority".into(),
            )
        })?;
    }
    Ok(())
}

pub(super) fn accept_receipt(
    api: &WorkspaceApi,
    saved: &Publication,
    receipt: RepositoryPublicationReceipt,
) -> ApiResult<Publication> {
    if receipt.binding != runtime_binding(&saved.binding)? {
        return Err(Error::WorkspacePermissionDenied(
            "Runtime publication receipt binding mismatch".into(),
        )
        .into());
    }
    let outcome = match receipt.outcome {
        RepositoryPublicationOutcome::Unknown {} => return Ok(saved.clone()),
        RepositoryPublicationOutcome::NotPublished { reason } => {
            PublicationOutcome::NotPublished { reason }
        }
        RepositoryPublicationOutcome::Published {
            target_after,
            observed_at_epoch_seconds,
        } => {
            let observed = chrono::DateTime::<Utc>::from_timestamp(
                i64::try_from(observed_at_epoch_seconds).map_err(|_| {
                    Error::InvalidInput("invalid publication observation time".into())
                })?,
                0,
            )
            .ok_or_else(|| Error::InvalidInput("invalid publication observation time".into()))?;
            PublicationOutcome::Published {
                target_after,
                observed_at: observed.to_rfc3339(),
            }
        }
    };
    let store = merge_request_store(api, api.workspace_id())?;
    let saved = store.record_publication_outcome(&saved.binding, outcome)?;
    if matches!(saved.outcome, PublicationOutcome::Published { .. }) {
        // Publication remains recorded even if cancellation/rescope/revocation
        // now prevents concluding the current MR.
        if let Err(error) =
            store.complete_publication(api.workspace_id(), &saved.binding.operation_id, Utc::now())
        {
            tracing::debug!(operation_id=%saved.binding.operation_id, %error, "publication recorded; MR completion deferred");
        }
    }
    Ok(saved)
}

pub(super) fn reconcile(api: &WorkspaceApi, saved: Publication) -> ApiResult<Publication> {
    match saved.outcome {
        PublicationOutcome::Unconfirmed {} => {
            // Read Runtime-owned immutable evidence before acquiring any current
            // Repository access. Removing credentials/configuration or stopping
            // the former owner cannot erase an already acknowledged publication.
            let receipt = api
                .runtime
                .repository_publication_receipt(
                    &saved.binding.worker.runtime_id,
                    runtime_binding(&saved.binding)?,
                )
                .map_err(RuntimeRegistryError::into_error)?;
            let Some(receipt) = receipt else {
                // Absence cannot fence a delayed original dispatch. Never free
                // the target claim or send a new push from this observation.
                return Ok(saved);
            };
            let unresolved = matches!(receipt.outcome, RepositoryPublicationOutcome::Unknown {});
            let saved = accept_receipt(api, &saved, receipt)?;
            if !unresolved {
                return Ok(saved);
            }
            let request = match request(api, &saved.binding) {
                Ok(request) => request,
                Err(error) => {
                    tracing::debug!(operation_id=%saved.binding.operation_id, %error, "publication remains unknown; current provider observation unavailable");
                    return Ok(saved);
                }
            };
            let receipt = api
                .runtime
                .reconcile_repository_publication(&saved.binding.worker.runtime_id, request)
                .map_err(RuntimeRegistryError::into_error)?;
            accept_receipt(api, &saved, receipt)
        }
        PublicationOutcome::Published { .. } => {
            let store = merge_request_store(api, api.workspace_id())?;
            let _ = store.complete_publication(
                api.workspace_id(),
                &saved.binding.operation_id,
                Utc::now(),
            );
            Ok(saved)
        }
        PublicationOutcome::NotPublished { .. } => Ok(saved),
    }
}

pub(super) fn publish(
    api: &WorkspaceApi,
    mr_id: &str,
    operation_id: String,
    approval_event_id: String,
) -> ApiResult<Publication> {
    if operation_id.trim().is_empty() || operation_id.len() > 200 {
        return Err(Error::InvalidInput(
            "publication operation_id must be nonempty and at most 200 bytes".into(),
        )
        .into());
    }
    let store = merge_request_store(api, api.workspace_id())?;
    if let Some(saved) = store.publication(api.workspace_id(), &operation_id)? {
        if saved.binding.merge_request_id != mr_id
            || saved.binding.approval_event_id != approval_event_id
        {
            return Err(merge_request::MergeRequestError::Conflict(
                "publication request fingerprint mismatch".into(),
            )
            .into());
        }
        return reconcile(api, saved);
    }
    let mr = store.get_by_id(api.workspace_id(), mr_id)?;
    let (owner, worker) = current_mr_worker(&store, api.workspace_id(), mr_id)?;
    require_mr_operation(api, &worker, mr_id, "merge_request.publish")?;
    let approval = mr
        .thread
        .iter()
        .find_map(|event| match event {
            merge_request::MergeRequestThreadEvent::Review(review)
                if review.event_id == approval_event_id =>
            {
                Some(review)
            }
            _ => None,
        })
        .ok_or_else(|| Error::InvalidInput("publication approval was not found".into()))?;
    let selector = mr
        .selector_from
        .as_deref()
        .ok_or_else(|| Error::InvalidInput("MR source requires repair".into()))?;
    let observed = observe_published_source_ref(
        api,
        api.workspace_id(),
        &worker.runtime_id,
        &mr.repository_id,
        selector,
    )?;
    if observed.resolved_ref != approval.subject_ref {
        return Err(merge_request::MergeRequestError::NotReady(
            "MR source differs from the requested approval".into(),
        )
        .into());
    }
    let runtime_worker = api
        .runtime
        .worker(&worker)
        .map_err(RuntimeRegistryError::into_error)?;
    let mut selected = None;
    for link in api
        .store
        .list_worker_workdir_links(api.workspace_id(), &worker)?
        .into_iter()
        .filter(|l| l.unlinked_at.is_none())
    {
        if !runtime_worker.workdir_attachments.iter().any(|a| {
            a.alias.as_str() == link.alias
                && a.working_directory.summary.working_directory_id == link.workdir_id
        }) {
            continue;
        }
        let Some(status) = api
            .runtime
            .working_directory(&worker.runtime_id, &link.workdir_id)
            .map_err(RuntimeRegistryError::into_error)?
            .working_directory
        else {
            continue;
        };
        if validate_assigned_workdir_source(
            &status.summary,
            &mr.repository_id,
            selector,
            &approval.subject_ref,
        )
        .is_ok()
        {
            selected = Some(link);
            break;
        }
    }
    let link = selected.ok_or_else(|| {
        merge_request::MergeRequestError::NotReady(
            "no clean exact-source MR Workdir is currently available".into(),
        )
    })?;
    let target = observe_published_merge_ref(
        api,
        api.workspace_id(),
        &worker.runtime_id,
        &mr.repository_id,
        &mr.selector_to,
    )?;
    let binding = PublicationBinding {
        workspace_id: api.workspace_id().into(),
        merge_request_id: mr_id.into(),
        operation_id,
        repository_id: mr.repository_id.clone(),
        repository_source_fingerprint: observed.source_fingerprint,
        target: target.selector,
        source: approval.subject_ref.clone(),
        approval_event_id: approval.event_id.clone(),
        owner_operation_id: owner.operation_id,
        worker: merge_request::WorkerIdentity {
            runtime_id: worker.runtime_id.clone(),
            worker_id: worker.worker_id.clone(),
        },
        working_directory_id: link.workdir_id,
        connection_id: link.connection_id,
        alias: link.alias,
        target_before: target.resolved_ref,
    };
    let runtime_request = request(api, &binding)?;
    let (saved, acquired) = store.claim_publication(binding.clone(), Utc::now(), |_| {
        authorize_dispatch(api, &binding)
            .map_err(|e| merge_request::MergeRequestError::Unauthorized(e.error.to_string()))
    })?;
    if !acquired {
        return reconcile(api, saved);
    }
    // Only this claim owner can dispatch. Replays and recovery always take the
    // receipt-only path, including after a timeout or owner/policy change.
    if let Err(error) = authorize_dispatch(api, &binding) {
        store.record_publication_outcome(
            &binding,
            PublicationOutcome::NotPublished {
                reason: "authorization changed before Runtime dispatch".into(),
            },
        )?;
        return Err(error);
    }
    let receipt = api
        .runtime
        .publish_repository_source(&worker.runtime_id, runtime_request)
        .map_err(RuntimeRegistryError::into_error)?;
    accept_receipt(api, &saved, receipt)
}

pub(super) fn advance(api: &WorkspaceApi, mr_id: &str) -> ApiResult<()> {
    let store = merge_request_store(api, api.workspace_id())?;
    let saved = store.publications(api.workspace_id(), mr_id)?;
    for publication in saved
        .iter()
        .filter(|p| !matches!(p.outcome, PublicationOutcome::NotPublished { .. }))
    {
        reconcile(api, publication.clone())?;
    }
    let mr = store.get_by_id(api.workspace_id(), mr_id)?;
    if !matches!(
        mr.state,
        merge_request::MergeRequestState::Integration { .. }
    ) {
        return Ok(());
    }
    let Some(approval) = mr.thread.iter().rev().find_map(|e| match e {
        merge_request::MergeRequestThreadEvent::Review(r)
            if r.decision == merge_request::ReviewDecision::Approve
                && mr
                    .effective_review(&r.subject_ref)
                    .is_some_and(|current| current.event_id == r.event_id) =>
        {
            Some(r)
        }
        _ => None,
    }) else {
        return Ok(());
    };
    if saved.iter().any(|p| {
        p.binding.approval_event_id == approval.event_id
            && !matches!(p.outcome, PublicationOutcome::NotPublished { .. })
    }) {
        return Ok(());
    }
    publish(
        api,
        mr_id,
        format!("mr-publication-{}", Uuid::now_v7()),
        approval.event_id.clone(),
    )?;
    Ok(())
}

pub(super) async fn recovery_loop(api: WorkspaceApi) {
    let mut timer = tokio::time::interval(std::time::Duration::from_secs(30));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cursor = None;
    loop {
        tokio::select! { _ = timer.tick() => {}, _ = api.mr_publication_wake.notified() => { cursor = None; } }
        let stage = api.clone();
        let after = cursor.clone();
        let page = match api.host_executors.host() {
            Ok(executor) => {
                executor
                    .run(move || -> ApiResult<_> {
                        Ok(merge_request_store(&stage, stage.workspace_id())?.list(
                            stage.workspace_id(),
                            &merge_request::MergeRequestListQuery {
                                cursor: after,
                                limit: 16,
                                ..Default::default()
                            },
                        )?)
                    })
                    .await
            }
            Err(error) => Err(error.into()),
        };
        let Ok(page) = page else {
            continue;
        };
        cursor = page.next_cursor;
        let mut attempts = tokio::task::JoinSet::new();
        for mr in page.items {
            let stage = api.clone();
            attempts.spawn(async move {
                if let Ok(executor) = stage.host_executors.host() {
                    let _ = executor
                        .run(move || advance(&stage, &mr.merge_request_id))
                        .await;
                }
            });
        }
        while attempts.join_next().await.is_some() {}
    }
}
