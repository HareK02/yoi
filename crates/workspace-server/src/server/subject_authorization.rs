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

/// Policy representation is chosen by the adopted authority, never by whether a
/// legacy read happens to succeed. An empty/missing canonical policy stays empty.
enum CurrentSubjectPolicy {
    Legacy(SubjectPermissions),
    Canonical(server_api::authorization::CurrentSubjectPolicy),
}

impl CurrentSubjectPolicy {
    fn change_id(&self) -> &str {
        match self {
            Self::Legacy(policy) => &policy.change_id,
            Self::Canonical(policy) => &policy.change_id,
        }
    }

    fn allows(
        &self,
        authority: &WorkerAuthorityView,
        needed: &ScopedPermission,
        delegation: bool,
        subject_id: &str,
    ) -> ApiResult<bool> {
        self.allows_with(
            authority,
            needed,
            delegation,
            subject_id,
            |operation, assignment| current_key_allowed(authority, operation, assignment),
        )
    }

    fn allows_in_registry(
        &self,
        authority: &WorkerAuthorityView,
        needed: &ScopedPermission,
        delegation: bool,
        subject_id: &str,
        registry: &worker::feature::permissions::FeaturePermissionRegistry,
    ) -> ApiResult<bool> {
        self.allows_with(
            authority,
            needed,
            delegation,
            subject_id,
            |operation, assignment| {
                key_allowed_in_registry(authority, registry, operation, assignment)
            },
        )
    }

    fn allows_with(
        &self,
        authority: &WorkerAuthorityView,
        needed: &ScopedPermission,
        delegation: bool,
        subject_id: &str,
        key_allowed: impl Fn(
            &str,
            Option<&feature_permissions::PermissionAssignment>,
        ) -> ApiResult<bool>,
    ) -> ApiResult<bool> {
        let constraints = match self {
            Self::Legacy(policy) => {
                if !key_allowed(&needed.operation, None)? {
                    return Ok(false);
                }
                Some(policy)
            }
            Self::Canonical(policy) => {
                let assignment = if delegation {
                    &policy.delegation
                } else {
                    &policy.execution
                };
                if !key_allowed(&needed.operation, Some(assignment))? {
                    return Ok(false);
                }
                if policy.legacy_constraints.is_none()
                    && matches!(&needed.resource, PermissionResource::Subject { subject_id: requested, .. } if requested != subject_id)
                {
                    return Ok(false);
                }
                policy.legacy_constraints.as_ref()
            }
        };
        let Some(constraints) = constraints else {
            // The Feature key permits operation use. Its caller still owns the
            // domain checks (assignment, provider scope, attempt/writer, CAS).
            return Ok(true);
        };
        let lane = if delegation {
            &constraints.delegation
        } else {
            &constraints.execution
        };
        for granted in lane {
            if granted.validate().is_ok()
                && canonical_worker_operation(&granted.operation).unwrap_or(&granted.operation)
                    == canonical_worker_operation(&needed.operation).unwrap_or(&needed.operation)
                && authority.resources_cover(&granted.resource, &needed.resource)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn legacy_read(&self, authority: &WorkerAuthorityView) -> ApiResult<SubjectPermissions> {
        match self {
            Self::Legacy(policy) => {
                let filter = |lane: &[ScopedPermission]| -> ApiResult<Vec<ScopedPermission>> {
                    let mut output = Vec::new();
                    for permission in lane {
                        if current_key_allowed(authority, &permission.operation, None)? {
                            output.push(permission.clone());
                        }
                    }
                    Ok(output)
                };
                Ok(SubjectPermissions {
                    change_id: policy.change_id.clone(),
                    execution: filter(&policy.execution)?,
                    delegation: filter(&policy.delegation)?,
                })
            }
            Self::Canonical(policy) => {
                authority.refresh_permission_registry()?;
                let registry = authority
                    .permission_registry
                    .read()
                    .map_err(|_| denied("current Feature permission registry is unavailable"))?;
                let filter = |assignment: &feature_permissions::PermissionAssignment,
                              constraints: &[ScopedPermission]|
                 -> ApiResult<Vec<ScopedPermission>> {
                    let resolved = registry.host().resolve(Some(assignment));
                    let mut represented = BTreeSet::new();
                    let mut output = Vec::new();
                    for permission in constraints {
                        let operation = canonical_worker_operation(&permission.operation)
                            .unwrap_or(&permission.operation);
                        if let Ok(key) = feature_permissions::PermissionKey::parse(operation) {
                            if resolved.projection().allowed.contains_key(&key) {
                                represented.insert(key);
                                output.push(permission.clone());
                            }
                        }
                    }
                    if resolved
                        .projection()
                        .allowed
                        .keys()
                        .any(|key| !represented.contains(key))
                    {
                        return Err(denied(
                            "current key policy requires the per-operation consumer; no legacy all-target fallback is available",
                        ));
                    }
                    Ok(output)
                };
                let constraints = policy.legacy_constraints.as_ref();
                Ok(SubjectPermissions {
                    change_id: policy.change_id.clone(),
                    execution: filter(
                        &policy.execution,
                        constraints
                            .map(|policy| policy.execution.as_slice())
                            .unwrap_or_default(),
                    )?,
                    delegation: filter(
                        &policy.delegation,
                        constraints
                            .map(|policy| policy.delegation.as_slice())
                            .unwrap_or_default(),
                    )?,
                })
            }
        }
    }
}

/// Legacy vectors keep exact domain limits but cannot keep a removed/disabled
/// Feature key alive. Scalar activation names remain the legacy-only exception
/// (including MCP); they never imply any dotted operation.
fn current_key_allowed(
    authority: &WorkerAuthorityView,
    operation: &str,
    assignment: Option<&feature_permissions::PermissionAssignment>,
) -> ApiResult<bool> {
    let operation = canonical_worker_operation(operation).unwrap_or(operation);
    if !operation.contains('.') {
        return Ok(assignment.is_none());
    }
    let key = feature_permissions::PermissionKey::parse(operation).map_err(denied)?;
    authority.refresh_permission_registry()?;
    let registry = authority
        .permission_registry
        .read()
        .map_err(|_| denied("current Feature permission registry is unavailable"))?;
    key_allowed_in_registry(authority, &registry, operation, assignment)
}

fn key_allowed_in_registry(
    authority: &WorkerAuthorityView,
    registry: &worker::feature::permissions::FeaturePermissionRegistry,
    operation: &str,
    assignment: Option<&feature_permissions::PermissionAssignment>,
) -> ApiResult<bool> {
    let operation = canonical_worker_operation(operation).unwrap_or(operation);
    if !operation.contains('.') {
        return Ok(assignment.is_none());
    }
    let key = feature_permissions::PermissionKey::parse(operation).map_err(denied)?;
    let catalog = registry.host().catalog();
    if catalog.workspace.as_str() != authority.workspace_id {
        return Err(denied(
            "Feature permission registry belongs to another Workspace",
        ));
    }
    let namespace_id = key.namespace();
    let Some(owner) = catalog
        .namespaces
        .iter()
        .find(|entry| entry.namespace == namespace_id)
    else {
        return Ok(false);
    };
    // Trusted registration metadata chooses the callee owner, never wire claims.
    let feature = worker::feature::FeatureId::new(owner.feature.as_str())
        .map_err(|error| denied(error.to_string()))?;
    let Some(namespace) = registry.namespace_handle(&feature, &namespace_id) else {
        return Ok(false);
    };
    let exact = feature_permissions::PermissionAssignment {
        grants: vec![feature_permissions::PermissionReference::Namespace(
            feature_permissions::NamespacePermissions {
                namespace: namespace_id,
                keys: vec![key.clone()],
            },
        )],
        ..Default::default()
    };
    let resolved = registry.host().resolve(Some(assignment.unwrap_or(&exact)));
    registry
        .host()
        .allows(namespace, &resolved, &key)
        .map_err(|error| denied(error.to_string()))
}

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
    plugin_artifact_root: PathBuf,
    permission_registry:
        Arc<std::sync::RwLock<worker::feature::permissions::FeaturePermissionRegistry>>,
}

impl From<&WorkspaceApi> for WorkerAuthorityView {
    fn from(api: &WorkspaceApi) -> Self {
        Self {
            workspace_id: api.workspace_id().into(),
            store: api.store.clone(),
            config_store: api.config_store.clone(),
            feature_storage: api.feature_storage.clone(),
            subjektiv_registration: api.subjektiv_registration.clone(),
            plugin_artifact_root: ServerConfig::server_data_root_for_data_dir(
                &api.config.data_root,
            )
            .join("plugin-artifacts"),
            permission_registry: api.permission_registry.clone(),
        }
    }
}

impl WorkerAuthorityView {
    fn refresh_permission_registry(&self) -> ApiResult<()> {
        let mut registry = self
            .permission_registry
            .write()
            .map_err(|_| denied("current Feature permission registry is unavailable"))?;
        self.config_store.sync_plugin_permission_registry(
            &self.plugin_artifact_root,
            &self.workspace_id,
            &mut registry,
        )?;
        Ok(())
    }

    fn subject_store(&self) -> ApiResult<crate::subjektiv::SubjektivStore> {
        crate::subjektiv::SubjektivStore::open(&self.feature_storage, &self.subjektiv_registration)
            .map_err(|error| subjektiv_store_error(error).into())
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

    fn policy(&self, subject_id: &str) -> ApiResult<Option<CurrentSubjectPolicy>> {
        let store = self.subject_store()?;
        let subject = store
            .subject(subject_id)
            .map_err(subjektiv_store_error)?
            .ok_or_else(|| denied("Subject permission source no longer exists"))?;
        if subject.state != crate::subjektiv::SubjectState::Active {
            return Err(denied("Subject permission source is retired"));
        }
        let canonical = self.config_store.with_conn(|connection| {
            crate::config_adoption::canonical(connection, &self.workspace_id)
        })?;
        if canonical {
            return Ok(self
                .config_store
                .adopted_subject_policy(&self.workspace_id, subject_id)?
                .map(|policy| {
                    CurrentSubjectPolicy::Canonical(
                        server_api::authorization::CurrentSubjectPolicy {
                            change_id: policy.change_id,
                            execution: policy.source.execution.assignment(),
                            delegation: policy.source.delegation.assignment(),
                            legacy_constraints: policy.legacy_constraints,
                        },
                    )
                }));
        }
        Ok(store
            .subject_policy_source(subject_id)
            .map_err(subjektiv_store_error)?
            .map(|source| match source {
                server_api::authorization::SubjectPolicySource::Legacy(policy) => {
                    CurrentSubjectPolicy::Legacy(policy)
                }
                server_api::authorization::SubjectPolicySource::Feature(policy) => {
                    CurrentSubjectPolicy::Canonical(policy)
                }
            }))
    }

    fn resources_cover(
        &self,
        granted: &PermissionResource,
        requested: &PermissionResource,
    ) -> ApiResult<bool> {
        resources_cover_store(self.store.as_ref(), granted, requested)
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
        if source.change_id() != grant.source_change_id {
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
                || current.source_change_id != source.change_id()
            {
                return Err(denied("delegated execution source changed"));
            }
            for permission in current
                .permissions
                .iter()
                .chain(current.redelegation.iter())
            {
                if !source.allows(self, permission, true, &grant.subject_id)? {
                    return Err(denied(
                        "delegated execution exceeds the current Subject ceiling",
                    ));
                }
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
        Ok(Some(match self.policy(&subject)? {
            Some(policy) => policy.legacy_read(self)?.execution,
            None => Vec::new(),
        }))
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
        let permissions = if let Some(grant) = self.live_child_grant(controller)? {
            grant.permissions
        } else if let Some(subject) = self.current_body_subject(controller)? {
            if let Some(policy) = self.policy(&subject)? {
                if policy.allows(self, &requested, false, &subject)?
                    || policy.allows(
                        self,
                        &ScopedPermission {
                            operation: operation.clone(),
                            resource: PermissionResource::Worker {
                                runtime_id: target.runtime_id.clone(),
                                worker_id: target.worker_id.clone(),
                            },
                        },
                        false,
                        &subject,
                    )?
                {
                    return Ok(());
                }
            }
            return Err(denied(format!(
                "Worker operation {operation} is not granted for this target"
            )));
        } else {
            return Ok(()); // Preserve ordinary Workers' existing Backend contract.
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

fn policy(api: &WorkspaceApi, subject_id: &str) -> ApiResult<Option<CurrentSubjectPolicy>> {
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
    job_permissions_with_policy(api, worker, |subject| policy(api, subject), None)
}

fn job_permissions_with_policy(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    current_policy: impl FnOnce(&str) -> ApiResult<Option<CurrentSubjectPolicy>>,
    registry: Option<&worker::feature::permissions::FeaturePermissionRegistry>,
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
    let Some(policy) = current_policy(subject_id)? else {
        return Ok(Some(Vec::new()));
    };
    // Domain grants are immutable capabilities, not policy snapshots. Resolve
    // only this Job kind's operations against current policy, never a whole Set.
    let operations: &[&str] = if job.request.grants.subjektiv_consolidation.is_some() {
        &[
            "memory.consolidation.supply",
            "memory.staging.list",
            "memory.staging.read",
            "memory.read",
            "memory.query",
            "memory.resident.read",
            "memory.resident.supply",
            "memory.candidate.decide",
            "memory.proposal.validate",
            "memory.changes.list",
            "memory.receipt.read",
            "memory.surface.prepare",
            "memory.surface.supply",
            "memory.surface.publish",
            "memory.surface.fail",
        ]
    } else {
        &[
            "journal.job.input.read",
            "journal.job.input.supply",
            "journal.state.read",
            "journal.text.read",
            "journal.operation.query",
            "journal.operation.list",
            "journal.operation.coverage.read",
            "journal.entry.create",
            "journal.entry.fix",
            "journal.entry.rewrite",
            "journal.position.update",
            "journal.unchanged.confirm",
            "journal.checkpoint",
            "journal.finish",
        ]
    };
    let authority = WorkerAuthorityView::from(api);
    let mut permissions = Vec::new();
    for operation in operations {
        let permission = ScopedPermission {
            operation: (*operation).into(),
            resource: PermissionResource::Subject {
                workspace_id: api.workspace_id().into(),
                subject_id: subject_id.clone(),
            },
        };
        if match registry {
            Some(registry) => {
                policy.allows_in_registry(&authority, &permission, false, subject_id, registry)?
            }
            None => policy.allows(&authority, &permission, false, subject_id)?,
        } {
            permissions.push(permission);
        }
    }
    Ok(Some(permissions))
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

pub(super) fn has_subject_authority(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<bool> {
    validate_worker(api, worker)?;
    let authority = WorkerAuthorityView::from(api);
    Ok(api
        .store
        .worker_registry_projection(api.workspace_id(), worker)?
        .is_some_and(|projection| projection.job.is_some())
        || authority.live_child_grant(worker)?.is_some()
        || authority.current_body_subject(worker)?.is_some())
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

fn validate_worker(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> ApiResult<()> {
    let authority = WorkerAuthorityView::from(api);
    authority.require_not_deleted(worker)?;
    if job_permissions(api, worker)?.is_none() && authority.live_child_grant(worker)?.is_none() {
        if let Some(subject) = authority.current_body_subject(worker)? {
            authority.policy(&subject)?;
        }
    }
    Ok(())
}

/// Host-only identity shared by plan creation and live Plugin operations. The
/// identity is an immutable ceiling, not a permission: recheck the live lineage
/// on every use, including byte delivery and cache hits.
pub(super) fn plugin_execution_identity(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<crate::plugin_plan::HostExecutionIdentity> {
    validate_worker(api, worker)?;
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
    resources_cover_store(api.store.as_ref(), granted, requested)
}

fn resources_cover_store(
    store: &dyn ControlPlaneStore,
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
    let granted_id = store.resolve_resource_reference(workspace_id, kind, granted_id)?;
    let requested_id = store.resolve_resource_reference(workspace_id, kind, requested_id)?;
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
    let authority = WorkerAuthorityView::from(api);
    authority.require_not_deleted(worker)?;
    let permissions = if let Some(permissions) = job_permissions(api, worker)? {
        permissions
    } else if let Some(grant) = authority.live_child_grant(worker)? {
        grant.permissions
    } else if let Some(subject) = authority.current_body_subject(worker)? {
        let Some(policy) = authority.policy(&subject)? else {
            return Err(denied("Subject permissions are unset"));
        };
        if matches!(policy, CurrentSubjectPolicy::Canonical(_)) {
            if policy.allows(&authority, &needed, false, &subject)? {
                // A key does not issue a broader resource capability. Return the
                // concrete operation context, leaving domain/provider checks intact.
                return Ok(Some(needed.resource));
            }
            if matches!(&needed.resource, PermissionResource::WorkerOperation { .. }) {
                if let CurrentSubjectPolicy::Canonical(ref canonical) = policy {
                    if let Some(constraints) = &canonical.legacy_constraints {
                        for permission in &constraints.execution {
                            if canonical_worker_operation(&permission.operation)
                                == Some(needed.operation.as_str())
                                && matches!(permission.resource, PermissionResource::Worker { .. })
                                && policy.allows(&authority, permission, false, &subject)?
                            {
                                return Ok(Some(needed.resource));
                            }
                        }
                    }
                }
            }
            return Err(denied(format!(
                "operation `{}` is not granted for the requested resource",
                request.operation
            )));
        }
        policy.legacy_read(&authority)?.execution
    } else {
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
    validate_worker(api, source)?;
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
    enum Ceiling {
        Scoped(Vec<ScopedPermission>),
        Subject(CurrentSubjectPolicy),
    }
    let (subject_id, origin_runtime_id, origin_worker_id, source_change_id, ceiling) = match child {
        Some(grant) => (
            grant.subject_id,
            source.runtime_id.clone(),
            source.worker_id.clone(),
            grant.source_change_id,
            Ceiling::Scoped(grant.redelegation),
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
                source_policy.change_id().to_owned(),
                Ceiling::Subject(source_policy),
            )
        }
    };
    let authority = WorkerAuthorityView::from(api);
    for permission in requested
        .permissions
        .iter()
        .chain(requested.redelegation.iter())
    {
        let allowed = match &ceiling {
            Ceiling::Scoped(ceiling) => ceiling.iter().any(|allowed| allowed.covers(permission)),
            Ceiling::Subject(policy) => policy.allows(&authority, permission, true, &subject_id)?,
        };
        if !allowed {
            return Err(denied(
                "requested delegation exceeds explicit operations, resources or redelegation ceiling",
            ));
        }
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

pub(super) fn revalidate_execution_grant(
    api: &WorkspaceApi,
    grant: &WorkerExecutionGrant,
) -> ApiResult<()> {
    let origin = RuntimeWorkerRef::new(&grant.origin_runtime_id, &grant.origin_worker_id);
    let requested = WorkerDelegationRequest {
        permissions: grant.permissions.clone(),
        redelegation: grant.redelegation.clone(),
    };
    let mut current = prepare_delegation(
        api,
        &origin,
        Some(&requested),
        &grant.runtime_id,
        &grant.profile,
        &grant.profile_digest,
        None,
        false,
    )?
    .ok_or_else(|| denied("execution grant origin lost its Subject authority"))?;
    current.worker_id = grant.worker_id.clone();
    if &current != grant {
        return Err(denied(
            "execution grant source or immutable delegation changed before use",
        ));
    }
    Ok(())
}

pub(super) fn bind_reserved_grant(
    api: &WorkspaceApi,
    grant: &WorkerExecutionGrant,
    worker_id: &str,
) -> ApiResult<()> {
    let mut bound = grant.clone();
    bound.worker_id = worker_id.to_owned();
    revalidate_execution_grant(api, &bound)?;
    // Opening the store connects its canonical provider. Do this before taking
    // the registry guard, avoiding a recursive read when a writer is queued.
    let store = open_subjektiv_store(api)?;
    let registry = api
        .permission_registry
        .read()
        .map_err(|_| denied("current Feature permission registry is unavailable"))?;
    store
        .bind_worker_execution_grant_with_host(&bound, registry.host())
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
                    stop_intent: None,
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

struct MemoryEffectAuthority {
    api: WorkspaceApi,
    worker: RuntimeWorkerRef,
    subject: String,
    operation: String,
    #[cfg(test)]
    after_reservation: Option<Arc<dyn Fn() + Send + Sync>>,
}
impl subjektiv::effect_admission::MemoryEffectAuthority for MemoryEffectAuthority {
    fn enter<'a>(
        &'a self,
        admission: &subjektiv::effect_admission::MemoryEffectAdmission<'_>,
    ) -> subjektiv::Result<Box<dyn 'a + subjektiv::effect_admission::MemoryEffectLease>> {
        let reject = |error: ApiError| match error.error {
            Error::WorkspacePermissionDenied(message) | Error::RepositoryConflict(message) => {
                subjektiv::SubjektivError::AuthorizationDenied(message)
            }
            error => subjektiv::SubjektivError::Storage(
                feature_storage::FeatureStorageError::operation(error.to_string()),
            ),
        };
        // Admission order: Feature DB -> catalog registry -> Host writer. Resolve
        // through that already held transaction, never a recursive catalog writer.
        let mut registry = self.api.permission_registry.write().map_err(|_| {
            subjektiv::SubjektivError::AuthorizationDenied("Feature registry is unavailable".into())
        })?;
        let catalog = Arc::new(
            self.api
                .config_store
                .memory_authority_reader()
                .map_err(|error| reject(error.into()))?,
        );
        #[cfg(test)]
        if let Some(observe) = &self.after_reservation {
            observe();
        }
        // All Host reads, including the canonical provider and nested Job/owner
        // checks, must share the reserved connection rather than the ordinary one.
        let mut api = self.api.clone();
        api.config_store = catalog.clone();
        api.store = catalog.clone();
        let authority = WorkerAuthorityView::from(&api);
        catalog
            .with_conn(|connection| {
                catalog.sync_plugin_permission_registry_in_connection(
                    connection,
                    &authority.plugin_artifact_root,
                    api.workspace_id(),
                    &mut registry,
                )
            })
            .map_err(|error| reject(error.into()))?;
        let source = admission
            .current_policy_with_canonical_source(
                &self.subject,
                Arc::new(crate::config_adoption::CanonicalConfigurationSource {
                    store: catalog.as_ref().clone(),
                    host: registry.host().clone(),
                }),
            )?
            .ok_or_else(|| {
                subjektiv::SubjektivError::AuthorizationDenied("Subject policy is unset".into())
            })?;
        let requested = ScopedPermission {
            operation: self.operation.clone(),
            resource: PermissionResource::Subject {
                workspace_id: self.api.workspace_id().into(),
                subject_id: self.subject.clone(),
            },
        };
        let permissions = job_permissions_with_policy(
            &api,
            &self.worker,
            |subject| {
                if subject != self.subject {
                    return Err(denied("Memory Job Subject changed"));
                }
                Ok(Some(CurrentSubjectPolicy::Canonical(source.clone())))
            },
            Some(&registry),
        )
        .map_err(reject)?;
        if let Some(permissions) = permissions {
            if !permissions
                .iter()
                .any(|permission| permission.covers(&requested))
            {
                return Err(subjektiv::SubjektivError::AuthorizationDenied(
                    "Memory operation is outside the live Job grant".into(),
                ));
            }
        } else {
            if authority
                .current_body_subject(&self.worker)
                .map_err(reject)?
                .as_deref()
                != Some(self.subject.as_str())
            {
                let owner = api
                    .store
                    .require_current_worker_singleton_owner(api.workspace_id(), &self.worker)
                    .map_err(|error| {
                        subjektiv::SubjektivError::AuthorizationDenied(error.to_string())
                    })?;
                if owner.is_none_or(|owner| {
                    owner.key
                        != format!("{SUBJEKTIV_CONSOLIDATION_SINGLETON_PREFIX}{}", self.subject)
                }) {
                    return Err(subjektiv::SubjektivError::AuthorizationDenied(
                        "Memory Worker ownership changed".into(),
                    ));
                }
            }
            if !CurrentSubjectPolicy::Canonical(source)
                .allows_in_registry(&authority, &requested, false, &self.subject, &registry)
                .map_err(reject)?
            {
                return Err(subjektiv::SubjektivError::AuthorizationDenied(
                    "Current policy denies the Memory operation".into(),
                ));
            }
        }
        Ok(Box::new((catalog, registry)))
    }
}

pub(super) fn memory_effect_store(
    api: &WorkspaceApi,
    store: &subjektiv::SubjektivStore,
    worker: &RuntimeWorkerRef,
    subject: &str,
    operation: &str,
) -> subjektiv::SubjektivStore {
    store.with_memory_effect_authority(Arc::new(MemoryEffectAuthority {
        api: api.clone(),
        worker: worker.clone(),
        subject: subject.into(),
        operation: operation.into(),
        #[cfg(test)]
        after_reservation: None,
    }))
}

#[cfg(test)]
pub(super) fn memory_effect_store_after_reservation(
    api: &WorkspaceApi,
    store: &subjektiv::SubjektivStore,
    worker: &RuntimeWorkerRef,
    subject: &str,
    operation: &str,
    observe: Arc<dyn Fn() + Send + Sync>,
) -> subjektiv::SubjektivStore {
    store.with_memory_effect_authority(Arc::new(MemoryEffectAuthority {
        api: api.clone(),
        worker: worker.clone(),
        subject: subject.into(),
        operation: operation.into(),
        after_reservation: Some(observe),
    }))
}
