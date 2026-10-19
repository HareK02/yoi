//! Host binding for cooperative Feature authorization. Profile configuration requests
//! capabilities; it never issues them. Non-Subject Workers retain their existing
//! Backend/provider contracts. Subject bodies with no assignment deny by default.
//!
//! Bindings here are execution grants, not another Worker registry. Current body
//! ownership remains the singleton store. Replacing a permission assignment fences
//! all children issued from it; restoring a Worker cannot refresh that assignment.
//! Checks refuse new effects; they do not undo effects already committed. Trusted
//! Features must call the operation boundary themselves, including alternate surfaces.
//!
//! Subject delegation uses the normal Worker control lifecycle, which binds stable
//! operation identity to the authenticated origin and recovers control-grant writes.
//! Generic Worker creation rejects Subject delegation before reservation, even if
//! the payload supplies an operation ID; its manual allocations are not replay-safe.

use super::*;
use server_api::authorization::{
    OperationAuthorizationRequest, PermissionResource, ScopedPermission, SubjectPermissions,
    WorkerDelegationRequest, WorkerExecutionGrant,
};
use std::collections::BTreeSet;

fn denied(message: impl Into<String>) -> ApiError {
    Error::WorkspacePermissionDenied(message.into()).into()
}

fn policy(api: &WorkspaceApi, subject_id: &str) -> ApiResult<Option<SubjectPermissions>> {
    let store = open_subjektiv_store(api)?;
    let subject = store
        .subject(subject_id)
        .map_err(subjektiv_store_error)?
        .ok_or_else(|| denied("Subject permission source no longer exists"))?;
    if subject.state != crate::subjektiv::SubjectState::Active {
        return Err(denied("Subject permission source is retired"));
    }
    Ok(store
        .subject_permissions(subject_id)
        .map_err(subjektiv_store_error)?)
}

fn current_body_subject(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<String>> {
    let retained = api
        .store
        .retained_worker_singleton_lease(&api.config.workspace_id, worker)?;
    let Some(retained) = retained else {
        return Ok(None);
    };
    let Some(subject) = retained.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX) else {
        return Ok(None);
    };
    // This also rejects a retained lease from a released/replaced body. Never
    // fall through to non-Subject compatibility after ownership has ended.
    let current = api
        .store
        .require_current_worker_singleton_owner(&api.config.workspace_id, worker)?
        .ok_or_else(|| denied("Subject body no longer owns its singleton"))?;
    if current.key != retained.key {
        return Err(denied("Subject body ownership changed"));
    }
    Ok(Some(subject.to_owned()))
}

fn live_child_grant(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<WorkerExecutionGrant>> {
    let store = open_subjektiv_store(api)?;
    let Some(grant) = store
        .worker_execution_grant(&worker.runtime_id, &worker.worker_id)
        .map_err(subjektiv_store_error)?
    else {
        return Ok(None);
    };
    let source =
        policy(api, &grant.subject_id)?.ok_or_else(|| denied("Subject permissions are unset"))?;
    if source.change_id != grant.source_change_id {
        return Err(denied(
            "delegated execution grant was revoked by permission replacement",
        ));
    }
    let mut current = grant.clone();
    let mut visited = HashSet::new();
    for _ in 0..64 {
        if !visited.insert((current.runtime_id.clone(), current.worker_id.clone())) {
            return Err(denied("cyclic execution grant lineage"));
        }
        if current.subject_id != grant.subject_id
            || current.source_change_id != source.change_id
            || current
                .permissions
                .iter()
                .chain(current.redelegation.iter())
                .any(|permission| {
                    !source
                        .delegation
                        .iter()
                        .any(|ceiling| ceiling.covers(permission))
                })
        {
            return Err(denied(
                "delegated execution exceeds the current Subject ceiling",
            ));
        }
        let origin = RuntimeWorkerRef::new(&current.origin_runtime_id, &current.origin_worker_id);
        match store
            .worker_execution_grant(&origin.runtime_id, &origin.worker_id)
            .map_err(subjektiv_store_error)?
        {
            Some(parent) => {
                if current
                    .permissions
                    .iter()
                    .chain(current.redelegation.iter())
                    .any(|permission| {
                        !parent
                            .redelegation
                            .iter()
                            .any(|ceiling| ceiling.covers(permission))
                    })
                {
                    return Err(denied(
                        "execution grant exceeds its immediate parent's redelegation ceiling",
                    ));
                }
                current = parent;
            }
            None => {
                if current_body_subject(api, &origin)?.as_deref() != Some(grant.subject_id.as_str())
                {
                    return Err(denied(
                        "delegation origin is no longer the current Subject body",
                    ));
                }
                return Ok(Some(grant));
            }
        }
    }
    Err(denied(
        "execution grant lineage exceeds the bounded delegation depth",
    ))
}

/// None is explicitly the preexisting non-Subject contract, not an all-powerful
/// grant. Existing Workdir/provider/Ticket checks still apply to those Workers.
pub(super) fn worker_permissions(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<Vec<ScopedPermission>>> {
    if let Some(grant) = live_child_grant(api, worker)? {
        return Ok(Some(grant.permissions));
    }
    let Some(subject) = current_body_subject(api, worker)? else {
        return Ok(None);
    };
    Ok(Some(
        policy(api, &subject)?
            .map(|policy| policy.execution)
            .unwrap_or_default(),
    ))
}

pub(super) fn authorize(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: &OperationAuthorizationRequest,
) -> ApiResult<()> {
    authorize_scope(api, worker, request).map(|_| ())
}

pub(super) fn authorize_scope(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: &OperationAuthorizationRequest,
) -> ApiResult<Option<PermissionResource>> {
    request.validate().map_err(denied)?;
    if let PermissionResource::Workspace { workspace_id } = &request.resource {
        if workspace_id != &api.config.workspace_id {
            return Err(denied("resource belongs to another Workspace"));
        }
    }
    let needed = ScopedPermission {
        operation: request.operation.clone(),
        resource: request.resource.clone(),
    };
    let Some(permissions) = worker_permissions(api, worker)? else {
        return Ok(None);
    };
    if let Some(permission) = permissions
        .iter()
        .find(|permission| permission.covers(&needed))
    {
        return Ok(Some(permission.resource.clone()));
    }
    Err(denied(format!(
        "operation `{}` is not granted for the requested resource",
        request.operation
    )))
}

pub(super) fn source_from_context(
    context: &server_api::ServerRequestContext,
) -> ApiResult<RuntimeWorkerRef> {
    let source = context
        .runtime_source
        .as_ref()
        .ok_or_else(|| denied("authorization requires a verified Runtime source"))?;
    let worker_id = source
        .worker_id
        .as_deref()
        .ok_or_else(|| denied("authorization requires Runtime-bound Worker identity"))?;
    Ok(RuntimeWorkerRef::new(&source.runtime_id, worker_id))
}

/// Resolves authority from the authenticated origin, independently of its enabled
/// Feature/tool set. A model cannot nominate a Subject or inherit body identity.
pub(super) fn prepare_delegation(
    api: &WorkspaceApi,
    source: &RuntimeWorkerRef,
    requested: Option<&WorkerDelegationRequest>,
    runtime_id: &str,
    profile: &str,
    profile_digest: &str,
    singleton_key: Option<&str>,
    subject_connection: bool,
) -> ApiResult<Option<WorkerExecutionGrant>> {
    let child = live_child_grant(api, source)?;
    let body = if child.is_none() {
        current_body_subject(api, source)?
    } else {
        None
    };
    if child.is_none() && body.is_none() {
        if requested.is_some() {
            return Err(denied(
                "only an authenticated Subject delegation source can issue an execution grant",
            ));
        }
        return Ok(None);
    }
    if singleton_key.is_some() || subject_connection {
        return Err(denied(
            "delegated Workers cannot acquire a Subject body or singleton identity",
        ));
    }
    let requested = requested.ok_or_else(|| {
        denied("Subject delegation requires explicit least-authority permissions")
    })?;
    requested.validate().map_err(denied)?;
    let (subject_id, origin_runtime_id, origin_worker_id, source_change_id, ceiling) = match child {
        Some(grant) => (
            grant.subject_id,
            source.runtime_id.clone(),
            source.worker_id.clone(),
            grant.source_change_id,
            grant.redelegation,
        ),
        None => {
            let subject = body.expect("checked Subject body");
            let source_policy = policy(api, &subject)?.ok_or_else(|| {
                denied(
                    "Subject permissions are unset; an authorized administrator must assign them",
                )
            })?;
            (
                subject,
                source.runtime_id.clone(),
                source.worker_id.clone(),
                source_policy.change_id,
                source_policy.delegation,
            )
        }
    };
    if requested
        .permissions
        .iter()
        .chain(requested.redelegation.iter())
        .any(|permission| !ceiling.iter().any(|allowed| allowed.covers(permission)))
    {
        return Err(denied(
            "requested delegation exceeds explicit operations, resources or redelegation ceiling",
        ));
    }
    if requested.redelegation.iter().any(|permission| {
        !requested
            .permissions
            .iter()
            .any(|allowed| allowed.covers(permission))
    }) {
        return Err(denied(
            "redelegation must be within this Worker's effective grant",
        ));
    }
    Ok(Some(WorkerExecutionGrant {
        subject_id,
        origin_runtime_id,
        origin_worker_id,
        source_change_id,
        runtime_id: runtime_id.to_owned(),
        worker_id: String::new(),
        profile: profile.to_owned(),
        profile_digest: profile_digest.to_owned(),
        permissions: requested.permissions.clone(),
        redelegation: requested.redelegation.clone(),
    }))
}

pub(super) fn bind_reserved_grant(
    api: &WorkspaceApi,
    grant: &WorkerExecutionGrant,
    worker_id: &str,
) -> ApiResult<()> {
    let mut bound = grant.clone();
    bound.worker_id = worker_id.to_owned();
    Ok(open_subjektiv_store(api)?
        .bind_worker_execution_grant(&bound)
        .map_err(subjektiv_store_error)?)
}

/// Host-owned binding for an already authenticated Backend operation. No model
/// input can choose the captured Worker or turn a shared client into a grant.
#[derive(Clone)]
struct BoundOperationClient {
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
}
impl std::fmt::Debug for BoundOperationClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundOperationClient")
            .field("worker", &self.worker)
            .finish_non_exhaustive()
    }
}
impl worker::WorkspaceClient for BoundOperationClient {
    fn workspace_id(&self) -> Option<&str> {
        Some(&self.api.config.workspace_id)
    }
    fn kind(&self) -> &str {
        "backend-bound-operation"
    }
    fn is_available(&self) -> bool {
        true
    }
    fn execute(
        &self,
        _: worker::WorkspaceRequest,
    ) -> std::result::Result<worker::WorkspaceResponse, worker::WorkspaceClientError> {
        Err(worker::WorkspaceClientError::Unavailable(
            "this binding provides authorization only".into(),
        ))
    }
    fn authorize_operation(
        &self,
        operation: &str,
        resource: &PermissionResource,
    ) -> std::result::Result<(), worker::WorkspaceClientError> {
        authorize(
            &self.api,
            &self.worker,
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: resource.clone(),
            },
        )
        .map_err(|error| worker::WorkspaceClientError::Request(error.error.to_string()))
    }
    fn authorize_operation_scope(
        &self,
        operation: &str,
        resource: &PermissionResource,
    ) -> std::result::Result<Option<PermissionResource>, worker::WorkspaceClientError> {
        authorize_scope(
            &self.api,
            &self.worker,
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: resource.clone(),
            },
        )
        .map_err(|error| worker::WorkspaceClientError::Request(error.error.to_string()))
    }
}

pub(super) fn bind_operation_client(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> Arc<dyn worker::WorkspaceClient> {
    Arc::new(BoundOperationClient {
        api: api.clone(),
        worker: worker.clone(),
    })
}

pub(super) async fn interrupt_subject_workers(
    api: &WorkspaceApi,
    subject_id: &str,
) -> Vec<server_api::SubjectPermissionInterruption> {
    let mut workers = BTreeSet::new();
    let collected = (|| -> ApiResult<()> {
        for grant in open_subjektiv_store(api)?
            .list_subject_worker_execution_grants(subject_id)
            .map_err(subjektiv_store_error)?
        {
            workers.insert((grant.runtime_id, grant.worker_id));
        }
        let key = crate::subjektiv::subject_worker_singleton_key(subject_id)
            .map_err(subjektiv_store_error)?;
        if let Some(body) = api
            .store
            .current_worker_singleton_owner(&api.config.workspace_id, &key)?
        {
            workers.insert((body.worker.runtime_id, body.worker.worker_id));
        }
        Ok(())
    })();
    if let Err(error) = collected {
        return vec![server_api::SubjectPermissionInterruption {
            runtime_id: String::new(),
            worker_id: String::new(),
            cancellation_accepted: false,
            backend_command_sessions_closed: false,
            diagnostics: vec![format!(
                "permissions committed; interruption targets could not be fully collected: {}",
                error.error
            )],
        }];
    }
    let mut results = Vec::new();
    for (runtime_id, worker_id) in workers {
        let worker = RuntimeWorkerRef::new(&runtime_id, &worker_id);
        let mut evidence = server_api::SubjectPermissionInterruption {
            runtime_id,
            worker_id,
            cancellation_accepted: false,
            backend_command_sessions_closed: false,
            diagnostics: Vec::new(),
        };
        let cancellation_api = api.clone();
        let cancellation_worker = worker.clone();
        let cancellation = match api.host_executors.host() {
            Ok(host) => host.run(move || -> ApiResult<_> {
                Ok(cancellation_api.runtime.cancel_worker(&cancellation_worker, WorkerLifecycleRequest {
                    reason: Some("Subject permission assignment changed; new operations are fenced".into()), ticket_assignment: None,
                }).map_err(RuntimeRegistryError::into_error)?)
            }).await,
            Err(error) => Err(error.into()),
        };
        match cancellation {
            Ok(result) => {
                evidence.cancellation_accepted =
                    result.state == InternalWorkerOperationState::Accepted;
                evidence.diagnostics.extend(
                    result
                        .diagnostics
                        .into_iter()
                        .map(|diagnostic| diagnostic.message),
                );
            }
            Err(error) => evidence.diagnostics.push(format!(
                "Runtime cancellation acceptance unknown or rejected: {}",
                error.error
            )),
        }
        match close_worker_workdir_sessions(&api.workdir_sessions, &worker).await {
            Ok(()) => evidence.backend_command_sessions_closed = true,
            Err(error) => evidence
                .diagnostics
                .push(format!("Backend command cleanup not confirmed: {error}")),
        }
        results.push(evidence);
    }
    results
}

pub(super) fn administration_actor(
    context: &server_api::ServerRequestContext,
) -> ApiResult<&RequestActor> {
    if context.runtime_source.is_some() || context.worker_source.is_some() {
        return Err(denied(
            "Subject permission administration is not a Worker capability",
        ));
    }
    context
        .actor
        .as_ref()
        .ok_or_else(|| denied("Subject permission administration requires an authenticated owner"))
}
