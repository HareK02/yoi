//! Subject adapters derive targets from the ordinary singleton and dispatch ordinary
//! Worker lifecycle operations. Receipts are historical; observations are read separately.
use super::*;
use server_api::{
    SubjectBodyDisposition, SubjectBodyMutationResponse, SubjectBodyObservation, SubjectBodyRef,
    SubjectBodyResponse, SubjectBodyStartRequest, SubjectBodyStopRequest,
};

pub(super) fn api_error(
    error: ApiError,
    workspace_id: String,
    subject_id: String,
    request_id: String,
) -> server_api::SubjectBodyApiError {
    use server_api::SubjectBodyRequestResolution;
    let resolution = match &error.error {
        Error::SubjectBodySuperseded => SubjectBodyRequestResolution::Superseded,
        Error::WorkerLifecycleObservationStale => SubjectBodyRequestResolution::NotAccepted,
        _ => SubjectBodyRequestResolution::Unresolved,
    };
    let mut response = server_api::SubjectBodyApiError::from(error.into_repository_api_error());
    response.request = Some(server_api::SubjectBodyFailedRequest {
        workspace_id,
        subject_id,
        request_id,
        resolution,
    });
    response
}

fn diagnostic(code: &str, message: String) -> server_api::Diagnostic {
    server_api::Diagnostic {
        code: code.into(),
        severity: server_api::DiagnosticSeverity::Warning,
        message: sanitize_backend_error(&message),
    }
}

fn require_subject(
    api: &WorkspaceApi,
    subject: &str,
) -> ApiResult<crate::subjektiv::SubjectRecord> {
    let record = open_subjektiv_store(api)?
        .subject(subject)
        .map_err(subjektiv_store_error)?
        .ok_or_else(|| Error::SubjektivSubjectNotFound(subject.into()))?;
    if record.state != crate::subjektiv::SubjectState::Active
        || subjektiv_deletion::pending(api, subject)?
    {
        return Err(Error::WorkspacePermissionDenied(
            "Subject is deleted, being deleted or no longer active".into(),
        )
        .into());
    }
    Ok(record)
}

fn current(
    api: &WorkspaceApi,
    subject: &str,
) -> ApiResult<Option<crate::store::WorkerSingletonLease>> {
    require_subject(api, subject)?;
    let key = crate::subjektiv::subject_worker_singleton_key(subject)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    Ok(api
        .store
        .current_worker_singleton_owner(api.workspace_id(), &key)?)
}

pub(super) fn reference(lease: &crate::store::WorkerSingletonLease) -> SubjectBodyRef {
    SubjectBodyRef {
        runtime_id: lease.worker.runtime_id.clone(),
        worker_id: lease.worker.worker_id.clone(),
        generation: lease.generation.to_string(),
    }
}

/// Call while holding the existing Subject lifecycle gate through Runtime admission.
pub(super) fn require_expected(
    api: &WorkspaceApi,
    subject: &str,
    expected: Option<&SubjectBodyRef>,
) -> ApiResult<Option<crate::store::WorkerSingletonLease>> {
    let lease = current(api, subject)?;
    if lease.as_ref().map(reference).as_ref() != expected {
        return Err(Error::SubjectBodySuperseded.into());
    }
    Ok(lease)
}

pub(super) fn snapshot(api: &WorkspaceApi, subject: &str) -> ApiResult<SubjectBodyResponse> {
    let lease = current(api, subject)?;
    let observation = match lease.as_ref() {
        None => SubjectBodyObservation::Absent,
        Some(lease) => {
            let observe = || -> ApiResult<server_api::WorkerSummary> {
                let live = api
                    .runtime
                    .worker(&lease.worker)
                    .map_err(RuntimeRegistryError::into_error)?;
                let record = api
                    .store
                    .get_worker_registry(api.workspace_id(), &lease.worker)?
                    .ok_or_else(|| Error::UnknownWorker {
                        worker: lease.worker.clone(),
                    })?;
                let links = api
                    .store
                    .list_worker_workdir_links(api.workspace_id(), &lease.worker)?;
                let workdirs = api.store.list_workdir_registry(api.workspace_id(), 500)?;
                project_worker_registry_projection(api, Some(&live), &record, links, &workdirs)
            };
            match observe() {
                Ok(worker) => SubjectBodyObservation::Available { worker },
                Err(error) => SubjectBodyObservation::Unavailable {
                    diagnostics: vec![diagnostic(
                        "subject_body_observation_unavailable",
                        error.error.to_string(),
                    )],
                },
            }
        }
    };
    if current(api, subject)? != lease {
        return Err(Error::RepositoryConflict(
            "Subject body changed during observation; reload its current state".into(),
        )
        .into());
    }
    Ok(SubjectBodyResponse {
        workspace_id: api.workspace_id().into(),
        subject_id: subject.into(),
        current: lease.as_ref().map(reference),
        observation,
        can_start: false,
        can_stop: false,
    })
}

async fn account_context(
    api: &WorkspaceApi,
    context: &server_api::ServerRequestContext,
    workspace: &str,
) -> ApiResult<(String, WorkerOperationContext)> {
    let (account, authority) = read_context(api, context, workspace).await?;
    authority.authorize_workspace(api).await?;
    Ok((account, authority))
}

async fn read_context(
    api: &WorkspaceApi,
    context: &server_api::ServerRequestContext,
    workspace: &str,
) -> ApiResult<(String, WorkerOperationContext)> {
    validate_workspace_scope(api, workspace)?;
    let actor = subject_authorization::administration_actor(context)?;
    require_workspace_owner(api, workspace, actor, "Subject body lifecycle").await?;
    let authority = WorkerOperationContext::from_request_async(api, context)
        .await
        .map_err(|error| Error::WorkspacePermissionDenied(error.message))?;
    authority.authorize_workspace_read(api).await?;
    Ok((actor.account_id.clone(), authority))
}

pub(super) async fn get(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    subject: String,
) -> ApiResult<SubjectBodyResponse> {
    read_context(&api, &context, &workspace).await?;
    api.host_executors
        .host()?
        .run(move || {
            let mut body = snapshot(&api, &subject)?;
            body.can_start = true;
            body.can_stop = true;
            Ok(body)
        })
        .await
}

fn validate_request_id(id: &str) -> ApiResult<()> {
    if id.trim().is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "request_id must contain 1..128 bytes without control characters".into(),
        )
        .into());
    }
    Ok(())
}

fn operation_id(account: &str, workspace: &str, subject: &str, request: &str) -> String {
    let bytes = serde_json::to_vec(&(account, workspace, subject, request))
        .expect("lifecycle identity is text");
    let digest = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("subject-body-start:{digest}")
}

fn failure_diagnostics(code: &str, error: ApiError) -> Vec<server_api::Diagnostic> {
    let mut diagnostics = vec![diagnostic(code, error.error.to_string())];
    diagnostics.extend(error.diagnostics.into_iter().map(Into::into));
    diagnostics
}

async fn finish(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    subject: String,
    request_id: String,
    disposition: SubjectBodyDisposition,
    diagnostics: Vec<server_api::Diagnostic>,
) -> SubjectBodyMutationResponse {
    let body = get(api, context, workspace, subject).await.ok();
    SubjectBodyMutationResponse {
        request_id,
        disposition,
        diagnostics,
        body,
    }
}

fn stop_request(request: &SubjectBodyStopRequest) -> WorkerLifecycleRequest {
    WorkerLifecycleRequest {
        reason: Some("Stopped from the Subject screen; Subject data is retained".into()),
        ticket_assignment: None,
        stop_intent: Some(runtime_api::WorkerStopIntent {
            expected_observation_token: request.expected_observation_token.clone(),
            request_id: request.request_id.clone(),
        }),
    }
}

pub(super) async fn stop_outcome(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    subject: String,
    request: SubjectBodyStopRequest,
) -> ApiResult<server_api::SubjectBodyStopOutcomeResponse> {
    validate_request_id(&request.request_id)?;
    let (_, authority) = account_context(&api, &context, &workspace).await?;
    let lookup_api = api.clone();
    let lookup_subject = subject.clone();
    let expected = request.expected_current.clone();
    let lease = api
        .store
        .db_executor()
        .run(move || {
            require_expected(&lookup_api, &lookup_subject, Some(&expected))?
                .ok_or_else(|| ApiError::from(Error::SubjectBodySuperseded))
        })
        .await?;
    let worker =
        WorkspaceWorker::resolve_async(&api, &lease.worker.runtime_id, &lease.worker.worker_id)
            .await?;
    let result = worker
        .stop_outcome_current_subject(
            &authority,
            stop_request(&request),
            subject.clone(),
            request.expected_current,
        )
        .await?;
    let outcome = match result {
        runtime_api::WorkerStopOutcome::NotFound => server_api::SubjectBodyStopOutcome::NotFound,
        runtime_api::WorkerStopOutcome::Pending => server_api::SubjectBodyStopOutcome::Pending,
        runtime_api::WorkerStopOutcome::Stopped { .. } => {
            server_api::SubjectBodyStopOutcome::Stopped
        }
    };
    let body = get(api, context, workspace, subject).await.ok();
    Ok(server_api::SubjectBodyStopOutcomeResponse {
        request_id: request.request_id,
        outcome,
        body,
    })
}

pub(super) async fn stop(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    subject: String,
    request: SubjectBodyStopRequest,
) -> ApiResult<SubjectBodyMutationResponse> {
    validate_request_id(&request.request_id)?;
    let (_, authority) = account_context(&api, &context, &workspace).await?;
    let lookup_api = api.clone();
    let lookup_subject = subject.clone();
    let expected = request.expected_current.clone();
    let lease = api
        .store
        .db_executor()
        .run(move || {
            require_expected(&lookup_api, &lookup_subject, Some(&expected))?
                .ok_or_else(|| ApiError::from(Error::SubjectBodySuperseded))
        })
        .await?;
    let worker =
        WorkspaceWorker::resolve_async(&api, &lease.worker.runtime_id, &lease.worker.worker_id)
            .await?;
    let result = worker
        .stop_current_subject(
            &authority,
            stop_request(&request),
            subject.clone(),
            request.expected_current,
        )
        .await;
    let (disposition, diagnostics) = match result {
        Ok(result) => (
            if result.state == InternalWorkerOperationState::Accepted {
                SubjectBodyDisposition::Accepted
            } else {
                SubjectBodyDisposition::Unknown
            },
            result.diagnostics.into_iter().map(Into::into).collect(),
        ),
        Err(error)
            if matches!(
                error.error,
                Error::SubjectBodySuperseded
                    | Error::WorkerLifecycleObservationStale
                    | Error::RepositoryConflict(_)
                    | Error::WorkspacePermissionDenied(_)
                    | Error::RestoreObservationConflict
            ) =>
        {
            return Err(error);
        }
        // A post-dispatch failure may have followed a real stop. Keep the exact intent.
        Err(error) => (
            SubjectBodyDisposition::Unknown,
            failure_diagnostics("subject_body_stop_outcome_unknown", error),
        ),
    };
    Ok(finish(
        api,
        context,
        workspace,
        subject,
        request.request_id,
        disposition,
        diagnostics,
    )
    .await)
}

pub(super) async fn start(
    api: WorkspaceApi,
    context: server_api::ServerRequestContext,
    workspace: String,
    subject: String,
    request: SubjectBodyStartRequest,
) -> ApiResult<SubjectBodyMutationResponse> {
    validate_request_id(&request.request_id)?;
    let (account, authority) = account_context(&api, &context, &workspace).await?;
    if let Some(expected) = request.expected_current.as_ref() {
        let observed = get(
            api.clone(),
            context.clone(),
            workspace.clone(),
            subject.clone(),
        )
        .await?;
        if observed.current.as_ref() != Some(expected) {
            return Err(Error::SubjectBodySuperseded.into());
        }
        let SubjectBodyObservation::Available { .. } = observed.observation else {
            return Err(Error::RepositoryConflict(
                "Current body cannot be observed; do not create a replacement".into(),
            )
            .into());
        };
        let token = request.expected_observation_token.clone().ok_or_else(|| {
            Error::InvalidInput(
                "Starting an existing body requires its observed Runtime token".into(),
            )
        })?;
        // Even a live-body reuse checks Runtime's existing request owner first;
        // changed input cannot bypass a retained Restore receipt via a fresh GET.
        let worker =
            WorkspaceWorker::resolve_async(&api, &expected.runtime_id, &expected.worker_id).await?;
        let result = worker
            .restore_current_subject(
                &authority,
                server_api::WorkerRestoreRequest {
                    expected_observation_token: token,
                    request_id: operation_id(&account, &workspace, &subject, &request.request_id),
                },
                subject.clone(),
                expected.clone(),
            )
            .await;
        let (disposition, diagnostics) = match result {
            Ok(result) => (
                match result.state {
                    server_api::WorkerRestoreState::Accepted => SubjectBodyDisposition::Accepted,
                    server_api::WorkerRestoreState::Rejected
                    | server_api::WorkerRestoreState::RolledBack
                    | server_api::WorkerRestoreState::Superseded => {
                        SubjectBodyDisposition::Rejected
                    }
                    server_api::WorkerRestoreState::ReconciliationRequired => {
                        SubjectBodyDisposition::Unknown
                    }
                },
                result.diagnostics.into_iter().map(Into::into).collect(),
            ),
            Err(error)
                if matches!(
                    error.error,
                    Error::SubjectBodySuperseded
                        | Error::WorkerLifecycleObservationStale
                        | Error::RepositoryConflict(_)
                        | Error::WorkspacePermissionDenied(_)
                        | Error::RestoreObservationConflict
                ) =>
            {
                return Err(error);
            }
            Err(error) => (
                SubjectBodyDisposition::Unknown,
                failure_diagnostics("subject_body_restore_outcome_unknown", error),
            ),
        };
        return Ok(finish(
            api,
            context,
            workspace,
            subject,
            request.request_id,
            disposition,
            diagnostics,
        )
        .await);
    }
    if request.expected_observation_token.is_some() {
        return Err(
            Error::InvalidInput("An absent body has no Runtime observation token".into()).into(),
        );
    }
    let create_api = api.clone();
    let create_context = context.clone();
    let create_subject = subject.clone();
    let create_workspace = workspace.clone();
    let request_id = request.request_id.clone();
    let operation = operation_id(&account, &workspace, &subject, &request_id);
    let check_operation = operation.clone();
    let result = run_host_workflow(api.host_executors.host()?, async move {
        let subject_record = require_subject(&create_api, &create_subject)?;
        // An exact creation retry uses the same ordinary reservation, including its
        // original Runtime. It never changes an unknown request into a new launch.
        let reserved_runtime = create_api
            .config_store
            .reserved_worker_create_runtime(&create_workspace, &operation)?;
        if reserved_runtime.is_none() {
            require_expected(&create_api, &create_subject, None)?;
        }
        let runtime_id = match reserved_runtime {
            Some(runtime) => runtime,
            None => {
                let runtimes = create_api.runtime.list_runtimes(100);
                runtimes
                    .items
                    .iter()
                    .filter(|runtime| runtime.worker_creation_available)
                    .min_by_key(|runtime| {
                        (
                            runtime.runtime_id != EMBEDDED_WORKER_RUNTIME_ID,
                            runtime.runtime_id.clone(),
                        )
                    })
                    .map(|runtime| runtime.runtime_id.clone())
                    .ok_or_else(|| {
                        Error::Config("No Runtime is available to start this Subject".into())
                    })?
            }
        };
        let input = serde_json::to_vec(&(&account, &create_workspace, &create_subject, &request))
            .map_err(|error| Error::InvalidInput(error.to_string()))?;
        let fingerprint = Sha256::digest(input)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        create_workspace_worker_authenticated_on_host(
            create_api,
            contract_request_headers(&create_context)
                .map_err(|error| Error::WorkspacePermissionDenied(error.message))?,
            server_api::CreateWorkspaceWorkerRequest {
                runtime_id,
                display_name: subject_record.role.as_str().into(),
                singleton_key: None,
                profile: Some(server_api::authorization::SUBJECT_CONVERSATION_PROFILE.into()),
                ticket_assignment: None,
                initial_submit: vec![],
                workdir_attachments: vec![],
                feature_connections: server_api::WorkspaceWorkerFeatureConnectionsRequest {
                    subjektiv: Some(server_api::SubjektivWorkerConnectionRequest {
                        subject_id: create_subject,
                    }),
                },
                control_operation_id: None,
                execution_grant: None,
            },
            Some(WorkerControlOperation {
                operation_id: operation,
                input_fingerprint: format!("sha256:{fingerprint}"),
            }),
            Some(&create_context),
        )
        .await
    })
    .await;
    let (disposition, diagnostics) = match result {
        Ok(Json(result)) => (SubjectBodyDisposition::Accepted, result.diagnostics),
        Err(error)
            if matches!(
                error.error,
                Error::SubjectBodySuperseded
                    | Error::WorkerLifecycleObservationStale
                    | Error::RepositoryConflict(_)
                    | Error::WorkspacePermissionDenied(_)
                    | Error::SubjektivSubjectNotFound(_)
            ) =>
        {
            // A racing absent intent may lose after its initial observation but
            // before allocation. Only absence of its own reservation proves that
            // this screen's intent cannot affect the now-current body.
            if matches!(error.error, Error::RepositoryConflict(_)) {
                let lookup_api = api.clone();
                let lookup_workspace = workspace.clone();
                let lookup_subject = subject.clone();
                let superseded = api
                    .store
                    .db_executor()
                    .run(move || -> ApiResult<bool> {
                        Ok(lookup_api
                            .config_store
                            .reserved_worker_create_runtime(&lookup_workspace, &check_operation)?
                            .is_none()
                            && current(&lookup_api, &lookup_subject)?.is_some())
                    })
                    .await?;
                if superseded {
                    return Err(Error::SubjectBodySuperseded.into());
                }
            }
            return Err(error);
        }
        Err(error) => (
            SubjectBodyDisposition::Unknown,
            failure_diagnostics("subject_body_start_outcome_unknown", error),
        ),
    };
    Ok(finish(
        api,
        context,
        workspace,
        subject,
        request_id,
        disposition,
        diagnostics,
    )
    .await)
}
