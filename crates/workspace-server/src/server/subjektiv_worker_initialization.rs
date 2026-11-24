//! Subjektiv owns Subject identity, attachment authority and lifecycle admission.
//! Common create transport sees only Feature records and opaque singleton leases.
use super::feature_initialization::{
    BackendFeatureInitializationAdapter, ConnectionCaller, ConnectionResolution, CreationAuthority,
    ReservedConnection, initialization_service,
};
use super::*;
use protocol::feature_initialization::{FeatureInitialization, FeatureInitializations};
use subjektiv_api::worker_initialization::{
    FEATURE_ID, ResolvedWorkerConnection, VERSION, WorkerConnectionKind, WorkerConnectionRequest,
};
use worker::feature::{FeatureDescriptor, ServiceDeclaration};

/// Inspect only the authenticated caller's current connection. This does not
/// authorize Session recording or supply protected data; their existing typed
/// operation boundaries still run after the binding has been compared.
pub(super) fn current_connection(
    api: &WorkspaceApi,
    workspace: &str,
    context: &server_api::ServerRequestContext,
) -> ApiResult<ResolvedWorkerConnection> {
    validate_workspace_scope(api, workspace)?;
    let worker = subject_authorization::source_from_context(context)?;
    if let Some(grant) = subjektiv_job::active_grant(api, workspace, &worker)? {
        authorize_subjektiv_operation(
            api,
            &worker,
            workspace,
            &grant.subject_id,
            "memory.consolidation.supply",
        )?;
        return Ok(ResolvedWorkerConnection {
            workspace_id: workspace.into(),
            subject_id: Some(grant.subject_id),
            kind: WorkerConnectionKind::MemoryConsolidation,
        });
    }
    if api
        .store
        .worker_registry_projection(workspace, &worker)?
        .is_some_and(|projection| projection.job.is_some())
    {
        return Err(Error::WorkspacePermissionDenied(
            "Job has no current Subjektiv connection".into(),
        )
        .into());
    }
    let scope = subjektiv_worker_scope_from_store(api, workspace, context)?;
    if !matches!(scope.authority, SubjektivStoredScopeAuthority::Subject) {
        return Err(Error::WorkspacePermissionDenied(
            "legacy consolidator has no versioned connection".into(),
        )
        .into());
    }
    Ok(ResolvedWorkerConnection {
        workspace_id: workspace.into(),
        subject_id: Some(scope.subject_id),
        kind: WorkerConnectionKind::SubjectBody,
    })
}

pub(super) struct SubjektivBackendAdapter;

pub(crate) fn subject_connection(subject_id: String) -> Result<FeatureInitializations> {
    let record =
        FeatureInitialization::encode(FEATURE_ID, VERSION, &WorkerConnectionRequest { subject_id })
            .map_err(Error::InvalidInput)?;
    FeatureInitializations::new(vec![record]).map_err(Error::InvalidInput)
}

pub(super) fn consolidation_connection(
    workspace: &str,
    subject: String,
) -> Result<FeatureInitializations> {
    resolved_connection(
        workspace,
        Some(subject),
        WorkerConnectionKind::MemoryConsolidation,
    )
}
#[cfg(test)]
pub(crate) fn fixture_subject_initialization(
    workspace: &str,
    subject: &str,
) -> Result<FeatureInitializations> {
    resolved_connection(
        workspace,
        Some(subject.into()),
        WorkerConnectionKind::SubjectBody,
    )
}

fn resolved_connection(
    workspace: &str,
    subject: Option<String>,
    kind: WorkerConnectionKind,
) -> Result<FeatureInitializations> {
    let record = FeatureInitialization::encode(
        FEATURE_ID,
        VERSION,
        &ResolvedWorkerConnection {
            workspace_id: workspace.into(),
            subject_id: subject,
            kind,
        },
    )
    .map_err(Error::InvalidInput)?;
    FeatureInitializations::new(vec![record]).map_err(Error::InvalidInput)
}

fn decode(
    record: &FeatureInitialization,
    api: &WorkspaceApi,
) -> ApiResult<ResolvedWorkerConnection> {
    let connection: ResolvedWorkerConnection = record
        .decode(FEATURE_ID, VERSION)
        .map_err(Error::InvalidInput)?;
    connection.validate().map_err(Error::InvalidInput)?;
    if connection.workspace_id != api.workspace_id() {
        return Err(Error::WorkspacePermissionDenied(
            "Subjektiv initialization belongs to another Workspace".into(),
        )
        .into());
    }
    match connection.kind {
        WorkerConnectionKind::SubjectBody
            if connection
                .subject_id
                .as_deref()
                .is_none_or(|id| id.trim().is_empty()) =>
        {
            return Err(Error::InvalidInput(
                "Subject body initialization requires a Subject identity".into(),
            )
            .into());
        }
        _ => {}
    }
    Ok(connection)
}

impl BackendFeatureInitializationAdapter for SubjektivBackendAdapter {
    fn feature_id(&self) -> &'static str {
        FEATURE_ID
    }
    fn version(&self) -> u32 {
        VERSION
    }
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(
            "subjektiv-backend-initialization",
            "Subjektiv Backend initialization",
        )
        .with_provided_service(ServiceDeclaration::new(
            initialization_service(FEATURE_ID),
            VERSION.to_string(),
            "Host-resolved Subjektiv connection",
        ))
    }

    fn authorize(
        &self,
        caller: &ConnectionCaller<'_>,
        record: &FeatureInitialization,
    ) -> ApiResult<CreationAuthority> {
        let request: WorkerConnectionRequest = record
            .decode(FEATURE_ID, VERSION)
            .map_err(Error::InvalidInput)?;
        request.validate().map_err(Error::InvalidInput)?;
        if caller.source.is_some() {
            return Err(Error::WorkspacePermissionDenied(
                "delegated Workers cannot acquire Subject body identity".into(),
            )
            .into());
        }
        if caller.singleton_key.is_some() {
            return Err(Error::InvalidInput(
                "singleton_key must be omitted when a subjektiv subject connection is requested"
                    .into(),
            )
            .into());
        }
        if !matches!(
            caller.authorization,
            server_api::launch_policy::WorkerCreationAuthorization::Assignment { assignment: None }
        ) {
            return Err(Error::WorkspacePermissionDenied(
                "Subject body attachment has dedicated authority, not an ordinary initial grant"
                    .into(),
            )
            .into());
        }
        let account = caller.account.ok_or_else(|| {
            Error::WorkspacePermissionDenied(
                "Subject attachment requires the Workspace owner".into(),
            )
        })?;
        caller.api.config_store.with_conn(|connection| {
            crate::launch_policy::require_owner(connection, caller.api.workspace_id(), account)
        })?;
        Ok(CreationAuthority::FeatureOwned)
    }

    fn resolve(
        &self,
        api: &WorkspaceApi,
        profile: &manifest::WorkerManifest,
        record: &FeatureInitialization,
    ) -> ApiResult<ConnectionResolution> {
        let connection: WorkerConnectionRequest = record
            .decode(FEATURE_ID, VERSION)
            .map_err(Error::InvalidInput)?;
        let attachment = open_subjektiv_store(api)?
            .authorize_worker_attachment(&connection.subject_id, profile)
            .map_err(subjektiv_store_error)?;
        let records = resolved_connection(
            api.workspace_id(),
            Some(connection.subject_id),
            WorkerConnectionKind::SubjectBody,
        )?;
        Ok(ConnectionResolution {
            initialization: records
                .iter()
                .next()
                .expect("one encoded connection")
                .clone(),
            singleton_key: Some(attachment.singleton_key),
        })
    }

    fn validate_resolved(
        &self,
        api: &WorkspaceApi,
        request: &WorkerSpawnRequest,
        record: &FeatureInitialization,
    ) -> ApiResult<CreationAuthority> {
        let connection = decode(record, api)?;
        let bundle = request.resolved_config_bundle.as_ref().ok_or_else(|| {
            Error::Config("Subjektiv initialization requires the resolved Profile bundle".into())
        })?;
        let profile = match &request.profile {
            ProfileSelector::Builtin(name) | ProfileSelector::Named(name) => name,
        };
        let manifest =
            crate::profile_settings::resolve_profile_manifest_from_config_bundle(bundle, profile)?;
        if !manifest.feature.subjektiv.profile.enabled {
            return Err(subjektiv_store_error(subjektiv::SubjektivError::ProfileDisabled).into());
        }
        match connection.kind {
            WorkerConnectionKind::SubjectBody => {
                let subject = connection
                    .subject_id
                    .as_deref()
                    .expect("validated Subject identity");
                let attachment = open_subjektiv_store(api)?
                    .authorize_worker_attachment(subject, &manifest)
                    .map_err(subjektiv_store_error)?;
                if request.singleton_key.as_deref() != Some(attachment.singleton_key.as_str()) {
                    return Err(Error::WorkspacePermissionDenied(
                        "Subjektiv initialization singleton identity mismatch".into(),
                    )
                    .into());
                }
                let requested = request.feature_connections.get(FEATURE_ID).ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Subject initialization has no authenticated connection intent".into(),
                    )
                })?;
                let requested: WorkerConnectionRequest = requested
                    .decode(FEATURE_ID, VERSION)
                    .map_err(Error::InvalidInput)?;
                if requested.subject_id != subject {
                    return Err(Error::WorkspacePermissionDenied(
                        "Subject request and resolved identity differ".into(),
                    )
                    .into());
                }
                Ok(CreationAuthority::FeatureOwned)
            }
            WorkerConnectionKind::MemoryConsolidation => {
                if request.singleton_key.is_some()
                    || !matches!(
                        &request.intent,
                        WorkerSpawnIntent::BackendJob {
                            subjektiv_consolidation: true,
                            ..
                        }
                    )
                {
                    return Err(Error::WorkspacePermissionDenied("Memory consolidation initialization requires the existing dedicated Job authority".into()).into());
                }
                if let WorkerSpawnIntent::BackendJob {
                    job_id,
                    input_digest,
                    ..
                } = &request.intent
                {
                    let job = api
                        .store
                        .get_backend_job(api.workspace_id(), job_id)?
                        .ok_or_else(|| {
                            Error::WorkspacePermissionDenied(
                                "Subjektiv Job intent is missing".into(),
                            )
                        })?;
                    let job_request = job.load_executable_request()?;
                    let grant = job_request
                        .grants
                        .subjektiv_consolidation
                        .as_ref()
                        .ok_or_else(|| {
                            Error::WorkspacePermissionDenied(
                                "Subjektiv Job grant is missing".into(),
                            )
                        })?;
                    if connection.subject_id.as_deref() != Some(grant.subject_id.as_str())
                        || job_request.input_digest()? != *input_digest
                    {
                        return Err(Error::WorkspacePermissionDenied(
                            "Subjektiv connection differs from its frozen Job intent".into(),
                        )
                        .into());
                    }
                }
                Ok(CreationAuthority::Ordinary)
            }
        }
    }

    fn admit_reservation(
        &self,
        api: &WorkspaceApi,
        record: &FeatureInitialization,
        reserve: &mut dyn FnMut() -> ApiResult<crate::store::WorkerCreateReservation>,
    ) -> ApiResult<crate::store::WorkerCreateReservation> {
        let connection = decode(record, api)?;
        if connection.kind != WorkerConnectionKind::SubjectBody {
            return reserve();
        }
        let Some(subject) = connection.subject_id.as_deref() else {
            return reserve();
        };
        subjektiv_journal::recover_failed_subject_create(api, subject)?;
        let gate = subjektiv_journal::lifecycle_lock(api, subject)?;
        let _guard = gate
            .lock()
            .map_err(|_| Error::Store("Journal lifecycle lock poisoned".into()))?;
        subjektiv_deletion::require_available(api, subject)?;
        subjektiv_journal::check_start_allowed(api, subject)?;
        reserve()
    }

    fn reserved(
        &self,
        context: &ReservedConnection<'_>,
        record: &FeatureInitialization,
    ) -> ApiResult<()> {
        let connection = decode(record, context.api)?;
        if connection.kind == WorkerConnectionKind::SubjectBody
            && let Some(subject) = connection.subject_id.as_deref()
        {
            let attachment_key =
                format!("{}{}", subjektiv::SUBJECT_WORKER_SINGLETON_PREFIX, subject);
            if !context
                .reservation
                .singleton
                .as_ref()
                .is_some_and(|lease| lease.worker == *context.worker && lease.key == attachment_key)
            {
                return Err(Error::WorkspacePermissionDenied(
                    "reserved Subjektiv connection does not own its Subject singleton lease".into(),
                )
                .into());
            }
            // The saved connection, stable operation and reserved Worker identity
            // are the Service handoff boundary. Reservation alone grants nothing.
        }
        Ok(())
    }

    fn accepted(
        &self,
        api: &WorkspaceApi,
        record: &FeatureInitialization,
        result: WorkerSpawnResult,
    ) -> ApiResult<WorkerSpawnResult> {
        let connection = decode(record, api)?;
        if connection.kind != WorkerConnectionKind::SubjectBody {
            return Ok(result);
        }
        let result = restore_reused_subject_worker(api, result)?;
        if result.state == InternalWorkerOperationState::Accepted {
            if let Some(worker) = result.worker.as_ref() {
                dispatch_orchestrator_assignment_attention(api, &worker.worker);
            }
        }
        Ok(result)
    }
}
