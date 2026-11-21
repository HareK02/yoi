//! Each approval makes one publication attempt. Failures return through ordinary
//! MR comments and notifications, not a receipt or automatic retry loop.
use super::*;
use merge_request::PublicationBinding;
use worker_runtime::catalog::{
    RepositoryPublicationBinding, RepositoryPublicationOutcome, RepositoryPublicationRequest,
    RepositoryPublicationResult,
};

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

pub(super) fn publish(
    api: &WorkspaceApi,
    mr_id: &str,
    operation_id: String,
    approval_event_id: String,
) -> ApiResult<()> {
    if operation_id.trim().is_empty() || operation_id.len() > 200 {
        return Err(Error::InvalidInput(
            "publication operation_id must be nonempty and at most 200 bytes".into(),
        )
        .into());
    }
    let store = merge_request_store(api, api.workspace_id())?;
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
    let _target = TargetUse::acquire(api.workspace_id(), &binding.repository_id, &binding.target)?;
    let digest = store.validate_publication(&binding, |_| {
        authorize_dispatch(api, &binding)
            .map_err(|e| merge_request::MergeRequestError::Unauthorized(e.error.to_string()))
    })?;
    let result: RepositoryPublicationResult = api.runtime
        .publish_repository_source(&worker.runtime_id, runtime_request)
        .map_err(|_| Error::InvalidInput("Git publication response was not received; success or non-publication cannot be confirmed. Inspect the remote before deciding the next action.".into()))?;
    if result.binding != runtime_binding(&binding)? {
        return Err(Error::InvalidInput(
            "Git publication result binding mismatch; outcome cannot be confirmed".into(),
        )
        .into());
    }
    match result.outcome {
        RepositoryPublicationOutcome::Published { target_after, .. } if target_after == binding.source => {
            store.record_confirmed_publication(&binding, &digest, Utc::now())?;
            Ok(())
        }
        RepositoryPublicationOutcome::NotPublished { reason } => Err(Error::InvalidInput(reason).into()),
        _ => Err(Error::InvalidInput("Git publication outcome cannot be confirmed; MR remains incomplete. Inspect the remote before deciding the next action.".into()).into()),
    }
}

/// Manual recovery records only a provider-confirmed result. It deliberately does
/// not use Workdir commands, publication dispatch, or the old Subject-only gate.
pub(super) fn complete(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    mr_id: &str,
    request: server_api::CompleteMergeRequestRequest,
) -> ApiResult<merge_request::MergeEvent> {
    for value in [
        &request.operation_id,
        &request.expected_owner_operation_id,
        &request.approval_event_id,
        &request.expected_source_ref,
    ] {
        if value.trim().is_empty() || value.len() > 200 || value.chars().any(char::is_control) {
            return Err(Error::InvalidInput(
                "completion expectations must be nonempty, bounded identifiers".into(),
            )
            .into());
        }
    }
    require_mr_operation(api, worker, mr_id, "merge_request.complete")?;
    require_mr_operation(api, worker, mr_id, "merge_request.read")?;
    let store = merge_request_store(api, api.workspace_id())?;
    let mr = store.get_by_id(api.workspace_id(), mr_id)?;
    let admit = |conn: &rusqlite::Connection, current: &merge_request::MergeRequest| {
        require_mr_operation(api, worker, mr_id, "merge_request.complete")
            .and_then(|_| require_mr_operation(api, worker, mr_id, "merge_request.read"))
            .map_err(|e| merge_request::MergeRequestError::Unauthorized(e.error.to_string()))?;
        if !current.current_worker().is_some_and(|owner| {
            owner.operation_id == request.expected_owner_operation_id
                && owner.worker.as_ref().is_some_and(|w| {
                    w.runtime_id == worker.runtime_id && w.worker_id == worker.worker_id
                })
        }) {
            return Err(merge_request::MergeRequestError::Unauthorized(
                "completion requires the current MR Worker and observed ownership operation".into(),
            ));
        }
        if !current.state.is_completed() {
            validate_completion_source(conn, current, &request.expected_source_ref)?;
        }
        Ok(())
    };
    // Check ownership before any provider access as well as under the writer lock.
    let (owner, assigned) = current_mr_worker(&store, api.workspace_id(), mr_id)?;
    if assigned != *worker || owner.operation_id != request.expected_owner_operation_id {
        return Err(
            Error::WorkspacePermissionDenied("completion MR ownership changed".into()).into(),
        );
    }
    let approval = mr
        .thread
        .iter()
        .find_map(|event| match event {
            merge_request::MergeRequestThreadEvent::Review(review)
                if review.event_id == request.approval_event_id =>
            {
                Some(review)
            }
            _ => None,
        })
        .ok_or_else(|| {
            Error::InvalidInput("completion approval was not found on this MR".into())
        })?;
    let replay = mr.thread.iter().find_map(|event| match event {
        merge_request::MergeRequestThreadEvent::Merge(event)
            if event.operation_id == request.operation_id =>
        {
            Some(event)
        }
        _ => None,
    });
    let (target_before, target_after) = if let Some(event) = replay {
        (
            event.target_ref_before.clone(),
            event.target_ref_after.clone(),
        )
    } else {
        if !matches!(
            mr.state,
            merge_request::MergeRequestState::Integration { .. }
        ) {
            return Err(merge_request::MergeRequestError::NotReady(
                "MR is not in integration".into(),
            )
            .into());
        }
        let selector = mr
            .selector_from
            .as_deref()
            .ok_or_else(|| Error::InvalidInput("MR source requires repair".into()))?;
        let source = observe_published_source_ref(
            api,
            api.workspace_id(),
            &worker.runtime_id,
            &mr.repository_id,
            selector,
        )?;
        let target = observe_published_ref_with_ancestor(
            api,
            api.workspace_id(),
            &worker.runtime_id,
            &mr.repository_id,
            &mr.selector_to,
            Some(&request.expected_source_ref),
        )?;
        if source.resolved_ref != request.expected_source_ref
            || target.contained_commit.as_deref() != Some(request.expected_source_ref.as_str())
            || target.repository_id != mr.repository_id
            || source.source_fingerprint != target.source_fingerprint
        {
            return Err(merge_request::MergeRequestError::NotReady(
                "provider did not confirm that the MR target contains the approved source".into(),
            )
            .into());
        }
        (target.resolved_ref.clone(), target.resolved_ref)
    };
    store
        .record_verified_completion(
            &merge_request::ConfirmedMergeRequestPublication {
                workspace_id: api.workspace_id().into(),
                merge_request_id: mr_id.into(),
                operation_id: request.operation_id.clone(),
                repository_id: mr.repository_id.clone(),
                target: mr.selector_to.clone(),
                source: request.expected_source_ref.clone(),
                approval_event_id: request.approval_event_id.clone(),
                owner_operation_id: request.expected_owner_operation_id.clone(),
                worker: merge_request::WorkerIdentity {
                    runtime_id: worker.runtime_id.clone(),
                    worker_id: worker.worker_id.clone(),
                },
                target_before,
                target_after,
            },
            &approval.ticket_content_digest,
            Utc::now(),
            admit,
        )
        .map_err(Error::from)
        .map_err(Into::into)
}

// The provider and SQLite cannot commit together. Fence newer source evidence
// already known to Backend under the same writer transaction as completion.
pub(super) fn validate_completion_source(
    conn: &rusqlite::Connection,
    mr: &merge_request::MergeRequest,
    expected: &str,
) -> std::result::Result<(), merge_request::MergeRequestError> {
    use rusqlite::OptionalExtension;
    let current: Option<String> = conn.query_row(
        "SELECT subject_ref FROM merge_request_source_observations WHERE workspace_id=?1 AND merge_request_id=?2 ORDER BY sequence DESC LIMIT 1",
        rusqlite::params![mr.workspace_id, mr.merge_request_id],
        |row| row.get(0),
    ).optional()?;
    if current.as_deref() != Some(expected) {
        return Err(merge_request::MergeRequestError::NotReady(
            "MR source evidence changed during completion verification".into(),
        ));
    }
    Ok(())
}

// A guard exists only while the attempt runs; response loss never retains it.
// Normal non-force Git updates also protect against external publishers.
struct TargetUse((String, String, String));
static TARGETS: std::sync::Mutex<Option<std::collections::HashSet<(String, String, String)>>> =
    std::sync::Mutex::new(None);
impl TargetUse {
    fn acquire(workspace: &str, repository: &str, target: &str) -> ApiResult<Self> {
        let key = (workspace.into(), repository.into(), target.into());
        let mut targets = TARGETS
            .lock()
            .map_err(|_| Error::InvalidInput("publication safety lock unavailable".into()))?;
        if !targets
            .get_or_insert_with(Default::default)
            .insert(key.clone())
        {
            return Err(Error::InvalidInput("another publication is currently using this Repository target; inspect the current MR and remote before retrying".into()).into());
        }
        Ok(Self(key))
    }
}
impl Drop for TargetUse {
    fn drop(&mut self) {
        if let Ok(mut targets) = TARGETS.lock() {
            if let Some(targets) = targets.as_mut() {
                targets.remove(&self.0);
            }
        }
    }
}

pub(super) fn advance(api: &WorkspaceApi, mr_id: &str) -> ApiResult<()> {
    let store = merge_request_store(api, api.workspace_id())?;
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
    if let Err(error) = publish(
        api,
        mr_id,
        format!("mr-publication-{}", Uuid::now_v7()),
        approval.event_id.clone(),
    ) {
        let body = format!(
            "Automatic Git publication did not complete the MR: {}\n\nNo automatic retry is scheduled. Reread the MR, current authority and remote target; the Subject decides the next action through the ordinary workflow.",
            error.error
        );
        store.record_publication_problem(api.workspace_id(), mr_id, &body, Utc::now())?;
    }
    Ok(())
}
