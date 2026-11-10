//! Ordinary Worker authority shares the operation boundary with Subject and legacy grants.
//! Policy grants retain references and an issuance ceiling, not resource capabilities.
use super::*;
use feature_permissions::PermissionKey;
use server_api::authorization::{
    CoreWorkerExecutionGrant, CoreWorkerGrantParent, CoreWorkerGrantSource,
    PermissionReferenceEncoding,
};
use server_api::launch_policy::{WorkerLaunchPolicyState, launch_policy_usage_key};

impl WorkerAuthorityView {
    fn core_grant(&self, worker: &RuntimeWorkerRef) -> ApiResult<Option<CoreWorkerExecutionGrant>> {
        self.config_store
            .core_worker_execution_grant(&self.workspace_id, &worker.runtime_id, &worker.worker_id)
            .map_err(Into::into)
    }

    pub(super) fn core_key_allowed(
        &self,
        grant: &CoreWorkerExecutionGrant,
        operation: &str,
    ) -> ApiResult<bool> {
        let key = PermissionKey::parse(operation).map_err(denied)?;
        Ok(grant.bounds_operation(&key)
            && current_key_allowed(self, operation, Some(&grant.effective_assignment()))?)
    }

    fn validate_core_lineage(&self, grant: &CoreWorkerExecutionGrant) -> ApiResult<()> {
        grant.validate().map_err(denied)?;
        if grant.workspace_id != self.workspace_id {
            return Err(denied("execution grant belongs to another Workspace"));
        }
        self.config_store.with_conn(|connection| {
            crate::launch_policy::validate_policy_lineage(connection, grant)?;
            Ok(())
        })?;
        let mut current = grant.clone();
        let mut visited = HashSet::new();
        for _ in 0..64 {
            if !visited.insert(current.grant_id.clone()) {
                return Err(denied("cyclic core execution grant lineage"));
            }
            self.require_not_deleted(&RuntimeWorkerRef::new(
                &current.runtime_id,
                &current.worker_id,
            ))?;
            let removed = self.config_store.with_conn(|connection| {
                Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM worker_create_reservations WHERE workspace_id=?1 AND runtime_id=?2 AND worker_id=?3 AND state='removed')", rusqlite::params![self.workspace_id,current.runtime_id,current.worker_id], |row| row.get::<_,bool>(0))?)
            })?;
            if removed {
                return Err(denied("execution grant Worker was removed"));
            }
            match &current.source {
                CoreWorkerGrantSource::Human { .. } => return Ok(()),
                CoreWorkerGrantSource::LaunchPolicy {
                    policy_id,
                    policy_revision,
                    parent,
                } => {
                    let policy = self
                        .config_store
                        .worker_launch_policy(&self.workspace_id, policy_id)?
                        .ok_or_else(|| denied("launch policy source no longer exists"))?;
                    if policy.state != WorkerLaunchPolicyState::Active
                        || policy.authorization_revision != *policy_revision
                    {
                        return Err(denied("launch policy authorization revision is revoked"));
                    }
                    if current.effective_assignment() != policy.effective_child_permissions()
                        || !policy.allowed_profiles.contains(&current.profile)
                    {
                        return Err(denied(
                            "saved launch grant does not match its policy revision",
                        ));
                    }
                    let usage = launch_policy_usage_key(policy_id).map_err(denied)?;
                    let resource = PermissionResource::Workspace {
                        workspace_id: self.workspace_id.clone(),
                    };
                    match parent {
                        CoreWorkerGrantParent::SubjectBody {
                            subject_id,
                            source_change_id,
                            runtime_id,
                            worker_id,
                        } => {
                            let origin = RuntimeWorkerRef::new(runtime_id, worker_id);
                            if self.current_body_subject(&origin)?.as_deref() != Some(subject_id) {
                                return Err(denied("launch origin lost Subject body ownership"));
                            }
                            let source = self
                                .policy(subject_id)?
                                .ok_or_else(|| denied("Subject launch authority is unset"))?;
                            if source.change_id() != source_change_id {
                                return Err(denied("Subject launch authority was replaced"));
                            }
                            for operation in ["worker.create", usage.as_str()] {
                                if !source.allows(
                                    self,
                                    &ScopedPermission {
                                        operation: operation.into(),
                                        resource: resource.clone(),
                                    },
                                    false,
                                    subject_id,
                                )? {
                                    return Err(denied(
                                        "Subject no longer has launch policy use permission",
                                    ));
                                }
                            }
                            return Ok(());
                        }
                        CoreWorkerGrantParent::LegacySubjectGrant {
                            subject_id,
                            source_change_id,
                            runtime_id,
                            worker_id,
                        } => {
                            let origin = RuntimeWorkerRef::new(runtime_id, worker_id);
                            let legacy = self
                                .live_child_grant(&origin)?
                                .ok_or_else(|| denied("legacy launch origin lost its grant"))?;
                            if legacy.subject_id != *subject_id
                                || legacy.source_change_id != *source_change_id
                            {
                                return Err(denied("legacy launch origin was replaced"));
                            }
                            for operation in ["worker.create", usage.as_str()] {
                                if !legacy.permissions.iter().any(|p| {
                                    p.operation == operation && p.resource.covers(&resource)
                                }) {
                                    return Err(denied(
                                        "legacy origin does not permit launch policy use",
                                    ));
                                }
                            }
                            return Ok(());
                        }
                        CoreWorkerGrantParent::WorkerGrant {
                            grant_id,
                            runtime_id,
                            worker_id,
                        } => {
                            let parent = self
                                .core_grant(&RuntimeWorkerRef::new(runtime_id, worker_id))?
                                .ok_or_else(|| denied("parent execution grant is missing"))?;
                            if parent.grant_id != *grant_id {
                                return Err(denied("parent execution grant identity changed"));
                            }
                            for operation in ["worker.create", usage.as_str()] {
                                if !self.core_key_allowed(&parent, operation)? {
                                    return Err(denied(
                                        "parent no longer has launch policy use permission",
                                    ));
                                }
                            }
                            current = parent;
                        }
                    }
                }
            }
        }
        Err(denied("core execution grant lineage exceeds bounded depth"))
    }

    pub(super) fn live_core_grant(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> ApiResult<Option<CoreWorkerExecutionGrant>> {
        let Some(grant) = self.core_grant(worker)? else {
            return Ok(None);
        };
        self.validate_core_lineage(&grant)?;
        Ok(Some(grant))
    }
}

pub(in crate::server) fn validate_bound_worker(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> ApiResult<()> {
    let authority = WorkerAuthorityView::from(api);
    authority.require_not_deleted(worker)?;
    if authority.live_core_grant(worker)?.is_some()
        || authority.live_child_grant(worker)?.is_some()
        || job_permissions(api, worker)?.is_some()
    {
        return Ok(());
    }
    if let Some(subject) = authority.current_body_subject(worker)? {
        authority.policy(&subject)?;
        return Ok(());
    }
    Err(denied(
        "Worker execution authority is unbound; legacy ordinary Workers require explicit migration",
    ))
}

pub(in crate::server) fn prepare_core_policy_grant(
    api: &WorkspaceApi,
    source: &RuntimeWorkerRef,
    policy_id: &str,
    policy_revision: u64,
    runtime_id: &str,
    profile: &str,
    profile_digest: &str,
    operation_id: &str,
) -> ApiResult<CoreWorkerExecutionGrant> {
    server_api::launch_policy::validate_canonical_profile(profile).map_err(denied)?;
    validate_bound_worker(api, source)?;
    let authority = WorkerAuthorityView::from(api);
    let resource = PermissionResource::Workspace {
        workspace_id: api.workspace_id().into(),
    };
    let usage = launch_policy_usage_key(policy_id).map_err(denied)?;
    for operation in ["worker.create", usage.as_str()] {
        authorize(
            api,
            source,
            &OperationAuthorizationRequest {
                operation: operation.into(),
                resource: resource.clone(),
            },
        )?;
    }
    let policy = api
        .config_store
        .worker_launch_policy(api.workspace_id(), policy_id)?
        .ok_or_else(|| denied("unknown launch policy"))?;
    if policy.state != WorkerLaunchPolicyState::Active
        || policy.authorization_revision != policy_revision
    {
        return Err(denied("stale or inactive launch policy revision"));
    }
    if !policy
        .allowed_profiles
        .iter()
        .any(|allowed| allowed == profile)
    {
        return Err(denied(
            "Profile is not explicitly allowed by this launch policy",
        ));
    }
    let parent = if let Some(grant) = authority.live_core_grant(source)? {
        CoreWorkerGrantParent::WorkerGrant {
            grant_id: grant.grant_id,
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
        }
    } else if let Some(grant) = authority.live_child_grant(source)? {
        CoreWorkerGrantParent::LegacySubjectGrant {
            subject_id: grant.subject_id,
            source_change_id: grant.source_change_id,
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
        }
    } else {
        let subject_id = authority
            .current_body_subject(source)?
            .ok_or_else(|| denied("launch origin has no issuable authority"))?;
        let source_policy = authority
            .policy(&subject_id)?
            .ok_or_else(|| denied("Subject launch authority is unset"))?;
        CoreWorkerGrantParent::SubjectBody {
            source_change_id: source_policy.change_id().into(),
            subject_id,
            runtime_id: source.runtime_id.clone(),
            worker_id: source.worker_id.clone(),
        }
    };
    authority.refresh_permission_registry()?;
    let registry = authority
        .permission_registry
        .read()
        .map_err(|_| denied("permission registry unavailable"))?;
    let assignment = policy.effective_child_permissions();
    let host = authority
        .config_store
        .configured_permission_host(&authority.workspace_id, registry.host())?;
    let resolved = host.resolve(Some(&assignment));
    if resolved.projection().diagnostics.iter().any(|d| {
        matches!(
            d,
            feature_permissions::PermissionDiagnostic::UnknownKey(_)
                | feature_permissions::PermissionDiagnostic::UnknownNamespace(_)
                | feature_permissions::PermissionDiagnostic::UnknownSet(_)
        )
    }) {
        return Err(denied(
            "launch policy contains unknown permission references",
        ));
    }
    let ceiling = resolved.projection().allowed.keys().cloned().collect();
    Ok(CoreWorkerExecutionGrant {
        grant_id: new_id("weg"),
        workspace_id: api.workspace_id().into(),
        creation_operation_id: operation_id.into(),
        runtime_id: runtime_id.into(),
        worker_id: String::new(),
        profile: profile.into(),
        profile_digest: profile_digest.into(),
        assignment,
        reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        issuance_ceiling: Some(ceiling),
        source: CoreWorkerGrantSource::LaunchPolicy {
            policy_id: policy_id.into(),
            policy_revision,
            parent,
        },
    })
}

pub(in crate::server) fn revalidate_core_grant(
    api: &WorkspaceApi,
    grant: &CoreWorkerExecutionGrant,
) -> ApiResult<()> {
    WorkerAuthorityView::from(api).validate_core_lineage(grant)
}
