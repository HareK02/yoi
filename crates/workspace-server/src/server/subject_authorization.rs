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
    WorkerDelegationRequest, WorkerExecutionGrant, canonical_worker_operation,
};
use std::collections::BTreeSet;

fn denied(message: impl Into<String>) -> ApiError {
    Error::WorkspacePermissionDenied(message.into()).into()
}

/// Minimal shared authority for Worker control and removal. In particular this
/// must not retain WorkspaceApi or Runtime: the embedded removal dispatcher is
/// itself owned by Runtime. Policy, lineage and body ownership keep one checker.
#[derive(Clone)]
pub(super) struct WorkerAuthorityView {
    workspace_id: String,
    store: Arc<dyn ControlPlaneStore>,
    config_store: Arc<crate::SqliteWorkspaceStore>,
    feature_storage: crate::WorkspaceFeatureStorage,
    subjektiv_registration: crate::RegisteredFeature,
}

impl From<&WorkspaceApi> for WorkerAuthorityView {
    fn from(api: &WorkspaceApi) -> Self {
        Self {
            workspace_id: api.workspace_id().into(),
            store: api.store.clone(),
            config_store: api.config_store.clone(),
            feature_storage: api.feature_storage.clone(),
            subjektiv_registration: api.subjektiv_registration.clone(),
        }
    }
}

impl WorkerAuthorityView {
    fn subject_store(&self) -> ApiResult<crate::subjektiv::SubjektivStore> {
        crate::subjektiv::SubjektivStore::open(&self.feature_storage, &self.subjektiv_registration)
            .map_err(|error| Error::Store(error.to_string()).into())
    }

    fn require_not_deleted(&self, worker: &RuntimeWorkerRef) -> ApiResult<()> {
        let deleting = self.config_store.with_conn(|connection| {
            Ok(connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM subject_deletion_workers WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3)",
                rusqlite::params![self.workspace_id, worker.runtime_id, worker.worker_id],
                |row| row.get::<_, bool>(0),
            )?)
        })?;
        if deleting
            || self
                .subject_store()?
                .worker_subject_deleted(&worker.runtime_id, &worker.worker_id)
                .map_err(subjektiv_store_error)?
        {
            return Err(denied("Worker belonged to an explicitly deleted Subject"));
        }
        Ok(())
    }

    fn policy(&self, subject_id: &str) -> ApiResult<Option<SubjectPermissions>> {
        let store = self.subject_store()?;
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

    fn current_body_subject(&self, worker: &RuntimeWorkerRef) -> ApiResult<Option<String>> {
        let retained = self
            .store
            .retained_worker_singleton_lease(&self.workspace_id, worker)?;
        let Some(retained) = retained else {
            return Ok(None);
        };
        let Some(subject) = retained.key.strip_prefix(SUBJEKTIV_SINGLETON_PREFIX) else {
            return Ok(None);
        };
        // This also rejects a retained lease from a released/replaced body. Never
        // fall through to non-Subject compatibility after ownership has ended.
        let current = self
            .store
            .require_current_worker_singleton_owner(&self.workspace_id, worker)?
            .ok_or_else(|| denied("Subject body no longer owns its singleton"))?;
        if current.key != retained.key {
            return Err(denied("Subject body ownership changed"));
        }
        Ok(Some(subject.to_owned()))
    }

    fn live_child_grant(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> ApiResult<Option<WorkerExecutionGrant>> {
        let store = self.subject_store()?;
        let Some(grant) = store
            .worker_execution_grant(&worker.runtime_id, &worker.worker_id)
            .map_err(subjektiv_store_error)?
        else {
            return Ok(None);
        };
        let source = self
            .policy(&grant.subject_id)?
            .ok_or_else(|| denied("Subject permissions are unset"))?;
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
            let origin =
                RuntimeWorkerRef::new(&current.origin_runtime_id, &current.origin_worker_id);
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
                    if self.current_body_subject(&origin)?.as_deref()
                        != Some(grant.subject_id.as_str())
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

    fn subject_permissions(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> ApiResult<Option<Vec<ScopedPermission>>> {
        if let Some(grant) = self.live_child_grant(worker)? {
            return Ok(Some(grant.permissions));
        }
        let Some(subject) = self.current_body_subject(worker)? else {
            return Ok(None);
        };
        Ok(Some(
            self.policy(&subject)?
                .map(|policy| policy.execution)
                .unwrap_or_default(),
        ))
    }

    /// Saved individual-Worker permissions remain unchanged. Their canonical
    /// projection authorizes operation use only; without an explicit current
    /// WorkerOperation capability, the original exact target still restricts
    /// Backend control. Neither covers nor delegation ceilings are widened.
    fn authorize_control_use(
        &self,
        controller: &RuntimeWorkerRef,
        target: &RuntimeWorkerRef,
        permission: &str,
    ) -> ApiResult<()> {
        self.require_not_deleted(controller)?;
        if self
            .store
            .worker_registry_projection(&self.workspace_id, controller)?
            .is_some_and(|record| record.job.is_some())
        {
            return Err(denied("Job domain grants do not authorize Worker control"));
        }
        let Some(permissions) = self.subject_permissions(controller)? else {
            return Ok(()); // Preserve ordinary Workers' existing Backend contract.
        };
        let operation = if permission == "notify" {
            "worker.messaging".to_owned()
        } else {
            format!("worker.{permission}")
        };
        let requested = ScopedPermission {
            operation: operation.clone(),
            resource: PermissionResource::WorkerOperation {
                workspace_id: self.workspace_id.clone(),
            },
        };
        if permissions.iter().any(|granted| granted.covers(&requested)) {
            return Ok(());
        }
        if permissions.iter().any(|granted| {
            granted.validate().is_ok()
                && canonical_worker_operation(&granted.operation) == Some(operation.as_str())
                && matches!(&granted.resource, PermissionResource::Worker { runtime_id, worker_id }
                    if runtime_id == &target.runtime_id && worker_id == &target.worker_id)
        }) {
            return Ok(());
        }
        Err(denied(format!(
            "Worker operation {operation} is not granted for this target"
        )))
    }

    pub(super) fn authorize_control_permission(
        &self,
        controller: &RuntimeWorkerRef,
        target: &RuntimeWorkerRef,
        permission: &str,
    ) -> Result<WorkerControlGrantRecord> {
        self.authorize_control_use(controller, target, permission)
            .map_err(|error| error.error)?;
        let grant = self
            .store
            .get_active_worker_control_grant(&self.workspace_id, controller, target)?
            .ok_or_else(|| Error::UnknownWorker {
                worker: target.clone(),
            })?;
        if !grant
            .permissions
            .iter()
            .any(|candidate| candidate == permission)
        {
            return Err(Error::InvalidInput(format!(
                "worker control permission {permission} was not granted"
            )));
        }
        Ok(grant)
    }
}

fn policy(api: &WorkspaceApi, subject_id: &str) -> ApiResult<Option<SubjectPermissions>> {
    WorkerAuthorityView::from(api).policy(subject_id)
}

fn current_body_subject(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<String>> {
    WorkerAuthorityView::from(api).current_body_subject(worker)
}

fn live_child_grant(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<WorkerExecutionGrant>> {
    WorkerAuthorityView::from(api).live_child_grant(worker)
}

/// Job domain capabilities attenuate this policy; they are never non-Subject
/// compatibility. A missing frozen origin cannot mint new operation permissions.
fn job_permissions(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<Vec<ScopedPermission>>> {
    let Some(binding) = api
        .store
        .worker_registry_projection(api.workspace_id(), worker)?
        .and_then(|projection| projection.job)
    else {
        return Ok(None);
    };
    let job = api
        .store
        .get_backend_job(api.workspace_id(), &binding.job_id)?
        .ok_or_else(|| denied("Job authorization binding has no intent"))?;
    let subject_id = job
        .request
        .grants
        .subjektiv_consolidation
        .as_ref()
        .map(|grant| &grant.subject_id)
        .or_else(|| {
            job.request
                .grants
                .subjektiv_journal
                .as_ref()
                .map(|grant| &grant.subject_id)
        });
    let Some(subject_id) = subject_id else {
        // Generic Jobs have no Subject operation capability, even if their Profile
        // exposes a protected Feature or their source owns a Subject body.
        return Ok(Some(Vec::new()));
    };
    // This shared check validates current attempt, worker, input digest and deadline
    // for both Job kinds, independently of its optional consolidation payload.
    subjektiv_job::active_grant(api, api.workspace_id(), worker)?;
    let origin = job
        .request
        .source_worker
        .as_ref()
        .ok_or_else(|| denied("saved Job has no frozen operation-authority origin"))?;
    if current_body_subject(api, origin)?.as_deref() != Some(subject_id.as_str()) {
        return Err(denied("Job origin is no longer the current Subject body"));
    }
    let permissions = policy(api, subject_id)?
        .map(|policy| policy.execution)
        .unwrap_or_default();
    Ok(Some(permissions.into_iter().filter(|permission| {
        // Domain grants are immutable capabilities, not permission-policy snapshots.
        // Current policy can narrow them, but cannot turn a Journal Job into a
        // consolidator or confer other Subject operations on either Job kind.
        let within_job = if job.request.grants.subjektiv_consolidation.is_some() {
            matches!(permission.operation.as_str(),
                "memory.consolidation.supply" | "memory.staging.list" | "memory.staging.read"
                | "memory.read" | "memory.query" | "memory.resident.read" | "memory.resident.supply"
                | "memory.candidate.decide" | "memory.proposal.validate" | "memory.changes.list"
                | "memory.receipt.read" | "memory.surface.prepare" | "memory.surface.supply"
                | "memory.surface.publish" | "memory.surface.fail")
        } else {
            matches!(permission.operation.as_str(),
                "journal.job.input.read" | "journal.job.input.supply" | "journal.state.read"
                | "journal.text.read" | "journal.operation.query" | "journal.operation.list"
                | "journal.operation.coverage.read" | "journal.entry.create" | "journal.entry.fix"
                | "journal.entry.rewrite" | "journal.position.update" | "journal.unchanged.confirm"
                | "journal.checkpoint" | "journal.finish")
        };
        within_job && matches!(&permission.resource, PermissionResource::Subject {workspace_id, subject_id: granted}
            if workspace_id == api.workspace_id() && granted == subject_id)
    }).collect()))
}

/// Resolve only Host-held Job identity; authorization still validates the live
/// attempt, frozen origin and current policy before returning the resource.
pub(super) fn job_subject(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<String>> {
    let Some(binding) = api
        .store
        .worker_registry_projection(api.workspace_id(), worker)?
        .and_then(|projection| projection.job)
    else {
        return Ok(None);
    };
    let job = api
        .store
        .get_backend_job(api.workspace_id(), &binding.job_id)?
        .ok_or_else(|| denied("Job authorization binding has no intent"))?;
    Ok(job
        .request
        .grants
        .subjektiv_consolidation
        .map(|grant| grant.subject_id)
        .or_else(|| {
            job.request
                .grants
                .subjektiv_journal
                .map(|grant| grant.subject_id)
        }))
}

pub(super) fn authorize_job_input(
    api: &WorkspaceApi,
    request: &BackendJobRequest,
) -> ApiResult<()> {
    let binding = request
        .grants
        .subjektiv_consolidation
        .as_ref()
        .map(|grant| (&grant.subject_id, "memory.consolidation.supply"))
        .or_else(|| {
            request
                .grants
                .subjektiv_journal
                .as_ref()
                .map(|grant| (&grant.subject_id, "journal.job.input.supply"))
        });
    let Some((subject_id, operation)) = binding else {
        return Ok(());
    };
    let origin = request
        .source_worker
        .as_ref()
        .ok_or_else(|| denied("saved Job has no frozen operation-authority origin"))?;
    if current_body_subject(api, origin)?.as_deref() != Some(subject_id.as_str()) {
        return Err(denied("Job origin is no longer the current Subject body"));
    }
    authorize(
        api,
        origin,
        &OperationAuthorizationRequest {
            operation: operation.into(),
            resource: PermissionResource::Subject {
                workspace_id: api.workspace_id().into(),
                subject_id: subject_id.clone(),
            },
        },
    )
}

/// None is explicitly the preexisting non-Subject contract, not an all-powerful
/// grant. Existing Workdir/provider/Ticket checks still apply to those Workers.
pub(super) fn worker_permissions(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<Option<Vec<ScopedPermission>>> {
    let authority = WorkerAuthorityView::from(api);
    authority.require_not_deleted(worker)?;
    if let Some(permissions) = job_permissions(api, worker)? {
        return Ok(Some(permissions));
    }
    authority.subject_permissions(worker)
}

/// Host-only identity shared by plan creation and live Plugin operations. The
/// identity is an immutable ceiling, not a permission: recheck the live lineage
/// on every use, including byte delivery and cache hits.
pub(super) fn plugin_execution_identity(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<crate::plugin_plan::HostExecutionIdentity> {
    worker_permissions(api, worker)?;
    let (principal_id, delegation_id) = if let Some(grant) = live_child_grant(api, worker)? {
        (
            format!("subject:{}", grant.subject_id),
            Some(grant.source_change_id),
        )
    } else if let Some(subject) = current_body_subject(api, worker)? {
        (format!("subject:{subject}"), None)
    } else if let Some(subject) = job_subject(api, worker)? {
        (format!("subject:{subject}"), None)
    } else {
        (
            format!("worker:{}/{}", worker.runtime_id, worker.worker_id),
            None,
        )
    };
    Ok(crate::plugin_plan::HostExecutionIdentity {
        runtime_id: worker.runtime_id.clone(),
        worker_id: worker.worker_id.clone(),
        principal_id,
        delegation_id,
    })
}

pub(super) fn authorize(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: &OperationAuthorizationRequest,
) -> ApiResult<()> {
    authorize_scope(api, worker, request).map(|_| ())
}

fn resources_cover(
    api: &WorkspaceApi,
    granted: &PermissionResource,
    requested: &PermissionResource,
) -> ApiResult<bool> {
    if granted.covers(requested) {
        return Ok(true);
    }
    let (workspace_id, granted_id, requested_id, kind) = match (granted, requested) {
        (
            PermissionResource::Ticket {
                workspace_id: a,
                ticket_id: ai,
            },
            PermissionResource::Ticket {
                workspace_id: b,
                ticket_id: bi,
            },
        ) if a == b => (a, ai, bi, WorkspaceResourceKind::Ticket),
        (
            PermissionResource::Objective {
                workspace_id: a,
                objective_id: ai,
            },
            PermissionResource::Objective {
                workspace_id: b,
                objective_id: bi,
            },
        ) if a == b => (a, ai, bi, WorkspaceResourceKind::Objective),
        _ => return Ok(false),
    };
    // Resolve public aliases using identity metadata only. Never fetch Ticket,
    // Objective or MR bodies to answer an authorization query. Missing aliases
    // and unrelated resources have the same refusal.
    let granted_id = api
        .store
        .resolve_resource_reference(workspace_id, kind, granted_id)?;
    let requested_id = api
        .store
        .resolve_resource_reference(workspace_id, kind, requested_id)?;
    Ok(granted_id.is_some() && granted_id == requested_id)
}

pub(super) fn authorize_scope(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    request: &OperationAuthorizationRequest,
) -> ApiResult<Option<PermissionResource>> {
    request.validate().map_err(denied)?;
    if let Some(workspace_id) = request.resource.workspace_id() {
        if workspace_id != api.config.workspace_id {
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
    // Read-only migration of saved target-scoped permissions. Do not mutate the
    // assignment/grant or use this projection for delegation; Backend separately
    // checks the original exact target at every control effect.
    if matches!(&needed.resource, PermissionResource::WorkerOperation { workspace_id }
        if workspace_id == api.workspace_id())
        && permissions.iter().any(|permission| {
            permission.validate().is_ok()
                && canonical_worker_operation(&permission.operation)
                    == Some(needed.operation.as_str())
                && matches!(permission.resource, PermissionResource::Worker { .. })
        })
    {
        return Ok(Some(needed.resource));
    }
    for permission in &permissions {
        if permission.operation == needed.operation
            && resources_cover(api, &permission.resource, &needed.resource)?
        {
            return Ok(Some(if permission.resource.covers(&needed.resource) {
                permission.resource.clone()
            } else {
                // Alias resolution proved exact identity; return the spelling the
                // caller requested rather than a scope it cannot lexically match.
                needed.resource.clone()
            }));
        }
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
    // A deleted grant must never become an ordinary non-Subject delegation source.
    worker_permissions(api, source)?;
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
    open_subjektiv_store(api)?
        .bind_worker_execution_grant(&bound)
        .map_err(subjektiv_store_error)?;
    Ok(())
}

/// Persist only the confirmed execution delegation's reply direction, never an
/// inferred inverse of an ordinary A -> B control grant. A reservation is not a
/// Worker registry row: this runs after normal creation has committed that row.
/// Initial Runtime execution may already have started; replies are unavailable
/// until this write, and initial input is not queued or redispatched to hide that
/// window. Spawn replay repairs a failed write under the same allocation identity
/// without replacing the Worker or reissuing a revoked saved grant.
pub(super) fn persist_reply_grant(
    api: &WorkspaceApi,
    controller: &RuntimeWorkerRef,
    child: &RuntimeWorkerRef,
    allocation_key: &str,
) -> ApiResult<()> {
    let authority = WorkerAuthorityView::from(api);
    authority.require_not_deleted(child)?;
    let Some(bound) = authority.live_child_grant(child)? else {
        return Ok(()); // Ordinary Workers do not acquire reverse control.
    };
    let origin = RuntimeWorkerRef::new(&bound.origin_runtime_id, &bound.origin_worker_id);
    if &origin != controller {
        return Err(denied(
            "reply grant origin does not match authenticated spawn source",
        ));
    }
    if api
        .store
        .get_worker_registry(api.workspace_id(), child)?
        .is_none()
    {
        return Err(denied(
            "reply grant requires the committed Worker registry identity",
        ));
    }
    api.store
        .create_worker_control_grant(&WorkerControlGrantRecord {
            workspace_id: api.workspace_id().into(),
            grant_id: new_id("wcg"),
            controller: child.clone(),
            subject: origin,
            relation: "reply".into(),
            origin: "execution_grant".into(),
            permissions: vec!["notify".into()],
            operation_id: format!("worker-reply:{allocation_key}"),
            created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            revoked_at: None,
        })?;
    Ok(())
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

impl WorkspaceApi {
    /// Trusted Host plan creation uses the same identity as live Plugin operations.
    /// Principal and delegation are derived here, never accepted from public JSON.
    pub async fn plugin_execution_identity(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> crate::Result<crate::plugin_plan::HostExecutionIdentity> {
        self.store
            .get_workspace_runtime_binding(self.workspace_id(), &worker.runtime_id)
            .await?
            .filter(|binding| binding.revoked_at.is_none())
            .ok_or_else(|| {
                Error::WorkspacePermissionDenied("Plugin Runtime trust revoked".into())
            })?;
        let api = self.clone();
        let worker = worker.clone();
        self.host_executors
            .host()?
            .run(move || {
                let member = api.store.get_worker_registry(api.workspace_id(), &worker)?;
                let reserved = api
                    .store
                    .has_active_worker_create_reservation(api.workspace_id(), &worker)?;
                if member.is_none() && !reserved {
                    return Err(Error::WorkspacePermissionDenied(
                        "Plugin Worker is not registered".into(),
                    ));
                }
                api.store
                    .require_current_worker_singleton_owner(api.workspace_id(), &worker)?;
                let summary = api.runtime.worker(&worker).map_err(|_| {
                    Error::WorkspacePermissionDenied("Plugin Worker unavailable".into())
                })?;
                if summary.workspace.workspace_id.as_deref() != Some(api.workspace_id()) {
                    return Err(Error::WorkspacePermissionDenied(
                        "foreign Plugin Worker".into(),
                    ));
                }
                super::plugin_host::with_authority_database(&api, || {
                    plugin_execution_identity(&api, &worker).map_err(|error| error.error)
                })
            })
            .await
    }
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
