//! Worker operation admission based on Workspace registration.
//!
//! [`WorkspaceWorker`] fixes Runtime/Worker identity; it is not a lease guaranteeing a live
//! execution or authorization. Each operation rereads registration and revalidates the request's
//! Account or Worker control grant before delegating to Runtime. The controller determines actual
//! Idle/Busy state and input admission; catalog state alone cannot determine execution eligibility.
//!
//! # Caller and input boundaries
//!
//! The Server builds [`WorkerOperationContext`] from authentication results. Worker control locks
//! per grant and rechecks the same grant ID and permissions after waiting. Account requests reread
//! credentials, Account, and active Workspace, also checking browser session Origin. Test-only
//! Backend contexts do not exist in production.
//!
//! Worker control converts User input to UserIfIdle, never queuing Submit for a running Worker.
//! Notify requires separate permission. Normal input is denied to Backend Job Workers and Workers
//! with unsettled Ticket claims. Only Runtime Accepted Stop closes the current Workdir session
//! and synchronizes attachments. WorkerRemove is a separate contract: it verifies a target-bound
//! proof, then rechecks grants and retention in the removal service.
//!
//! # Connections and results
//!
//! Dedicated and multiplexed WS use the same [`WorkspaceWorkerMethodSender`]. Connections check
//! observe; method sends check send_input and browser context. Successful sends mean transport
//! channel enqueue, not controller acknowledgment or durable Submit/Notify receipts.
//! Actual results arrive through the event stream. Connections bind to one execution and do not
//! implicitly reconnect or Restore into the next execution with the same Worker ID.
//!
//! Unary input lacks the WS Submit replay-key contract; unknown delivery outcomes remain diagnostic.
//! Restore observation tokens, controller command IDs, Submit/Notify request IDs, and cleanup plan
//! digests have separate verifying owners, not a shared success criterion or replay contract.
//! Internal SubWorkers are parent Worker sessions, outside this Workspace registry handle.
use super::*;

/// Caller authority supplied by authenticated Server adapters, never public JSON/identity hints.
/// The unchecked Backend variant exists only for lower-level regression fixtures.
#[derive(Clone)]
pub(crate) enum WorkerOperationContext {
    #[cfg(test)]
    Backend,
    WorkerControl {
        controller: RuntimeWorkerRef,
    },
    Browser {
        headers: HeaderMap,
        source: protocol::AuthenticatedInputSource,
    },
}

impl WorkerOperationContext {
    fn bind_input(&self, request: &mut WorkerInputRequest) {
        // Public adapters cannot choose provenance or consume another input path's
        // receipt IDs. Bind these only after the current caller is authorized.
        request.source = match self {
            WorkerOperationContext::Browser { source, .. } => Some(source.clone()),
            WorkerOperationContext::WorkerControl { controller } => {
                Some(protocol::AuthenticatedInputSource::Worker {
                    runtime_id: controller.runtime_id.clone(),
                    worker_id: controller.worker_id.clone(),
                })
            }
            #[cfg(test)]
            WorkerOperationContext::Backend => request.source.clone(),
        };
        if let Some(source) = request.source.as_ref() {
            if let Some(client_id) = request.submission_request_id.take() {
                let path = match request.kind {
                    WorkerInputKind::Notify => "notify",
                    WorkerInputKind::User => "user",
                    WorkerInputKind::UserIfIdle => "user-if-idle",
                    WorkerInputKind::Compact => "compact",
                    WorkerInputKind::RegisterPeer => "register-peer",
                };
                // Fixed-size identity stays below the controller's request-ID ceiling
                // even for long Runtime/Account IDs. Tuple framing remains unambiguous.
                let canonical = serde_json::to_vec(&(source.namespace(), client_id))
                    .expect("input namespace contains only strings");
                let digest = sha2::Sha256::digest(canonical)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                request.submission_request_id = Some(format!("runtime-input:{path}:{digest}"));
            }
        }
    }

    pub(super) async fn authorize_workspace(&self, api: &WorkspaceApi) -> ApiResult<()> {
        if let Self::Browser { headers, source } = self {
            let actor = resolve_actor(&ServerAuthApi::from(api), headers)
                .await?
                .ok_or_else(|| {
                    Error::WorkspacePermissionDenied(
                        "Worker connection credentials were revoked".into(),
                    )
                })?;
            if matches!(actor.auth_method, ActorAuthMethod::BrowserSession) {
                let AuthConfig::Passkey { origin, .. } = &api.config.auth;
                if headers.get(ORIGIN).and_then(|value| value.to_str().ok())
                    != Some(origin.as_str())
                {
                    return Err(Error::WorkspacePermissionDenied(
                        "cross-origin Worker mutation denied".into(),
                    )
                    .into());
                }
            }
            if authenticated_browser_input_source(&actor) != *source {
                return Err(Error::WorkspacePermissionDenied(
                    "Worker connection Account changed".into(),
                )
                .into());
            }
            if !api
                .store
                .get_workspace(api.workspace_id())
                .await?
                .is_some_and(|workspace| workspace.state == "active")
            {
                return Err(Error::WorkspacePermissionDenied(
                    "Workspace is not accepting Worker mutations".into(),
                )
                .into());
            }
        }
        Ok(())
    }
    pub(super) async fn from_request_async(
        api: &WorkspaceApi,
        context: &server_api::ServerRequestContext,
    ) -> std::result::Result<Self, server_api::RepositoryApiError> {
        let owned_api = api.clone();
        let context = context.clone();
        api.store
            .db_executor()
            .run(move || Ok::<_, Error>(Self::from_request(&owned_api, &context)))
            .await
            .map_err(|error| ApiError::from(error).into_repository_api_error())?
    }

    pub(super) fn from_request(
        api: &WorkspaceApi,
        context: &server_api::ServerRequestContext,
    ) -> std::result::Result<Self, server_api::RepositoryApiError> {
        if let Some(source) = context.runtime_source.as_ref() {
            // Check hints against middleware's verified subject without replaying its proof
            // or requiring the target Runtime to be online just to represent identity.
            current_worker_contract_headers(context)?;
            let controller = RuntimeWorkerRef::new(
                &source.runtime_id,
                source
                    .worker_id
                    .as_deref()
                    .expect("source binding required a Worker identity"),
            );
            if api
                .store
                .get_worker_registry(api.workspace_id(), &controller)
                .map_err(|error| ApiError::from(error).into_repository_api_error())?
                .is_none()
            {
                return Err(ApiError::from(Error::UnknownWorker { worker: controller })
                    .into_repository_api_error());
            }
            return Ok(Self::WorkerControl { controller });
        }
        let actor = context.actor.as_ref().ok_or_else(|| {
            ApiError::from(Error::WorkspacePermissionDenied(
                "authenticated Worker operation source required".into(),
            ))
            .into_repository_api_error()
        })?;
        let headers = contract_request_headers(context)?;
        Ok(Self::Browser {
            headers,
            source: authenticated_browser_input_source(actor),
        })
    }
}

/// A registry identity, not an authorization lease or a snapshot of live execution.
#[derive(Clone)]
pub(crate) struct WorkspaceWorker<Services = WorkspaceApi> {
    api: Services,
    removal: WorkerRemovalService,
    identity: RuntimeWorkerRef,
}

// The removal-only specialization keeps the installed embedded dispatcher from
// retaining WorkspaceApi -> Runtime -> dispatcher -> WorkspaceApi as a strong cycle.
// Both specializations resolve the same registry identity and call the same remove method.
impl<Services> WorkspaceWorker<Services> {
    fn resolve_with_services(
        api: Services,
        removal: WorkerRemovalService,
        runtime_id: &str,
        reference: &str,
    ) -> Result<Self> {
        let direct = RuntimeWorkerRef::new(runtime_id, reference);
        let identity = if let Some(record) = removal
            .store
            .get_worker_registry(&removal.workspace_id, &direct)?
        {
            record.worker
        } else {
            let worker = removal
                .store
                .resolve_worker_resource_reference(&removal.workspace_id, reference)?
                .filter(|worker| worker.runtime_id == runtime_id)
                .ok_or_else(|| Error::UnknownWorker {
                    worker: direct.clone(),
                })?;
            removal
                .store
                .get_worker_registry(&removal.workspace_id, &worker)?
                .ok_or_else(|| Error::UnknownWorker { worker: direct })?
                .worker
        };
        Ok(Self {
            api,
            removal,
            identity,
        })
    }
    pub(super) fn identity(&self) -> &RuntimeWorkerRef {
        &self.identity
    }

    fn current_record(&self) -> ApiResult<WorkerRegistryRecord> {
        self.removal
            .store
            .get_worker_registry(&self.removal.workspace_id, &self.identity)?
            .ok_or_else(|| {
                Error::UnknownWorker {
                    worker: self.identity.clone(),
                }
                .into()
            })
    }

    pub(super) async fn remove(
        &self,
        source: crate::worker_source::VerifiedWorkerMutationSource,
        reason: &str,
    ) -> std::result::Result<worker::WorkspaceResponse, String> {
        let removal = self.removal.clone();
        let identity = self.identity.clone();
        self.removal
            .store
            .db_executor()
            .run(move || {
                removal
                    .store
                    .get_worker_registry(&removal.workspace_id, &identity)?
                    .ok_or_else(|| Error::UnknownWorker { worker: identity })
                    .map(drop)
            })
            .await
            .map_err(|error| error.to_string())?;
        self.removal
            .execute_async(
                source,
                &self.identity.runtime_id,
                &self.identity.worker_id,
                reason,
            )
            .await
    }
}

/// Server-owned adapter for Runtime's independent embedded Tool dispatcher.
/// The proof was target-bound and verified before this synchronous transport seam.
pub(super) struct EmbeddedWorkspaceWorkerRemoveExecutor {
    removal: WorkerRemovalService,
}
impl EmbeddedWorkspaceWorkerRemoveExecutor {
    pub(super) fn new(api: &WorkspaceApi) -> Self {
        Self {
            removal: WorkerRemovalService::new(api),
        }
    }
}
impl crate::worker_source::VerifiedWorkerRemoveExecutor for EmbeddedWorkspaceWorkerRemoveExecutor {
    fn execute(
        &self,
        source: crate::worker_source::VerifiedWorkerMutationSource,
        target_runtime_id: &str,
        target_worker_id: &str,
        reason: &str,
    ) -> std::result::Result<worker::WorkspaceResponse, String> {
        let worker = match WorkspaceWorker::resolve_with_services(
            (),
            self.removal.clone(),
            target_runtime_id,
            target_worker_id,
        ) {
            Ok(worker) => worker,
            Err(Error::UnknownWorker { .. }) => {
                return Ok(worker_remove_error_response(
                    StatusCode::NOT_FOUND,
                    "unknown_worker",
                    "The target Worker is not known in this Workspace",
                ));
            }
            Err(error) => return Err(error.to_string()),
        };
        let reason = reason.to_owned();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())?
                .block_on(worker.remove(source, &reason))
        })
        .join()
        .map_err(|_| "embedded WorkerRemove executor thread panicked".to_owned())?
    }
}

impl WorkspaceWorker {
    pub(crate) async fn resolve_async(
        api: &WorkspaceApi,
        runtime_id: &str,
        reference: &str,
    ) -> Result<Self> {
        let owned_api = api.clone();
        let runtime_id = runtime_id.to_owned();
        let reference = reference.to_owned();
        api.store
            .db_executor()
            .run(move || Self::resolve(&owned_api, &runtime_id, &reference))
            .await
    }

    pub(crate) fn resolve(api: &WorkspaceApi, runtime_id: &str, reference: &str) -> Result<Self> {
        Self::resolve_with_services(
            api.clone(),
            WorkerRemovalService::new(api),
            runtime_id,
            reference,
        )
    }

    pub(super) async fn authorize_operation(
        &self,
        context: &WorkerOperationContext,
        permission: &str,
    ) -> ApiResult<Option<tokio::sync::OwnedMutexGuard<()>>> {
        let worker = self.clone();
        let initial_context = context.clone();
        let permission = permission.to_owned();
        let initial_permission = permission.clone();
        let grant = self
            .api
            .store
            .db_executor()
            .run(move || -> ApiResult<Option<WorkerControlGrantRecord>> {
                worker.current_record()?;
                let WorkerOperationContext::WorkerControl { controller } = initial_context else {
                    return Ok(None);
                };
                if worker
                    .api
                    .store
                    .get_worker_registry(worker.api.workspace_id(), &controller)?
                    .is_none()
                {
                    return Err(Error::UnknownWorker { worker: controller }.into());
                }
                Ok(Some(authorize_known_worker_permission(
                    &worker.api,
                    worker.api.workspace_id(),
                    &controller,
                    &worker.identity,
                    &initial_permission,
                )?))
            })
            .await?;
        match context {
            #[cfg(test)]
            WorkerOperationContext::Backend => Ok(None),
            WorkerOperationContext::Browser { .. } => {
                context.authorize_workspace(&self.api).await?;
                Ok(None)
            }
            WorkerOperationContext::WorkerControl { controller } => {
                let grant = grant.expect("Worker control context resolved a grant");
                // Await admission outside the DB lane; the recheck owns the guard
                // until it finishes, even if its async waiter is cancelled.
                let guard = worker_control_lock(&self.api, &grant.grant_id)
                    .lock_owned()
                    .await;
                let worker = self.clone();
                let controller = controller.clone();
                self.api
                    .store
                    .db_executor()
                    .run(move || -> ApiResult<_> {
                        worker.current_record()?;
                        let current_grant = authorize_known_worker_permission(
                            &worker.api,
                            worker.api.workspace_id(),
                            &controller,
                            &worker.identity,
                            &permission,
                        )?;
                        if current_grant.grant_id != grant.grant_id {
                            return Err(Error::RepositoryConflict(
                                "Worker control grant changed while waiting for admission".into(),
                            )
                            .into());
                        }
                        Ok(Some(guard))
                    })
                    .await
            }
        }
    }

    pub(super) async fn restore(
        &self,
        context: &WorkerOperationContext,
        request: server_api::WorkerRestoreRequest,
    ) -> ApiResult<InternalWorkerRestoreResult> {
        let guard = self.authorize_operation(context, "restore").await?;
        let worker = self.clone();
        self.api
            .host_executors
            .host()?
            .run(move || {
                let _guard = guard;
                worker.restore_internal(request)
            })
            .await
    }

    /// Trusted Backend callers still supply an explicit observed intent. Runtime
    /// admission is shared with browser/Tool callers; this is not an unguarded launch.
    pub(super) fn restore_internal(
        &self,
        request: server_api::WorkerRestoreRequest,
    ) -> ApiResult<InternalWorkerRestoreResult> {
        self.current_record()?;
        self.api.restore_workspace_worker(&self.identity, request)
    }

    pub(super) async fn stop(
        &self,
        context: &WorkerOperationContext,
        request: WorkerLifecycleRequest,
    ) -> ApiResult<WorkerLifecycleResult> {
        let guard = self.authorize_operation(context, "stop").await?;
        let worker = self.clone();
        // Once admitted, Runtime stop, session close and linked Workdir
        // publication have one owner. Never hand an accepted stop's guards
        // back to its cancellable waiter or acquire this Host lane again.
        run_host_workflow(self.api.host_executors.host()?, async move {
            let _control = guard;
            let result = worker
                .api
                .runtime
                .stop_worker(&worker.identity, request)
                .map_err(|error| error.into_error())?;
            if result.state != InternalWorkerOperationState::Accepted {
                return Ok(result);
            }
            parse_runtime_worker_id_for_registry(&worker.identity.worker_id)?;
            let _session = current_worker_session_lock(&worker.api, &worker.identity)
                .lock_owned()
                .await;
            close_current_worker_session_locked(&worker.api, &worker.identity).await?;
            if let Some(record) = worker
                .api
                .store
                .get_worker_registry(worker.api.workspace_id(), &worker.identity)?
            {
                sync_linked_workdir_after_worker_stop(
                    &worker.api,
                    &worker.identity.runtime_id,
                    &record,
                )?;
            }
            Ok(result)
        })
        .await
    }

    pub(super) async fn cancel(
        &self,
        context: &WorkerOperationContext,
        request: WorkerLifecycleRequest,
    ) -> ApiResult<WorkerLifecycleResult> {
        let guard = self.authorize_operation(context, "cancel").await?;
        let worker = self.clone();
        self.api
            .host_executors
            .host()?
            .run(move || -> ApiResult<_> {
                let _guard = guard;
                Ok(worker
                    .api
                    .runtime
                    .cancel_worker(&worker.identity, request)
                    .map_err(|e| e.into_error())?)
            })
            .await
    }

    pub(super) async fn input(
        &self,
        context: &WorkerOperationContext,
        request: WorkerInputRequest,
    ) -> ApiResult<WorkerInputResult> {
        self.input_impl(context, request, None).await
    }

    pub(super) async fn input_current_subject(
        &self,
        context: &WorkerOperationContext,
        request: WorkerInputRequest,
        expected_subject: &str,
    ) -> ApiResult<WorkerInputResult> {
        self.input_impl(context, request, Some(expected_subject.to_owned()))
            .await
    }

    async fn input_impl(
        &self,
        context: &WorkerOperationContext,
        mut request: WorkerInputRequest,
        expected_subject: Option<String>,
    ) -> ApiResult<WorkerInputResult> {
        let permission = if request.kind == WorkerInputKind::Notify {
            "notify"
        } else {
            "send_input"
        };
        let guard = self.authorize_operation(context, permission).await?;
        let session_lock = current_worker_session_lock(&self.api, &self.identity);
        let session_guard = session_lock.lock_owned().await;
        self.ensure_interactive().await?;
        if matches!(context, WorkerOperationContext::WorkerControl { .. })
            && request.kind == WorkerInputKind::User
        {
            request.kind = WorkerInputKind::UserIfIdle;
        }
        context.bind_input(&mut request);
        let worker = self.clone();
        self.api
            .host_executors
            .host()?
            .run(move || -> ApiResult<_> {
                let (_control, _session) = (guard, session_guard);
                // Re-resolve while excluding Subject replacement through Runtime
                // admission. The DB read ends before Runtime I/O; a DB transaction
                // fence here would deadlock reentrant Host operations.
                let subject_gate = expected_subject
                    .as_deref()
                    .map(|subject| subjektiv_journal::lifecycle_lock(&worker.api, subject))
                    .transpose()?;
                let _subject_guard = subject_gate
                    .as_ref()
                    .map(|gate| {
                        gate.lock()
                            .map_err(|_| Error::Store("Subject lifecycle lock poisoned".into()))
                    })
                    .transpose()?;
                if let Some(subject) = expected_subject.as_deref() {
                    let key = crate::subjektiv::subject_worker_singleton_key(subject)
                        .map_err(|error| Error::InvalidInput(error.to_string()))?;
                    let owner = worker
                        .api
                        .store
                        .current_worker_singleton_owner(worker.api.workspace_id(), &key)?;
                    if !owner.is_some_and(|lease| lease.worker == worker.identity) {
                        return Err(Error::WorkspacePermissionDenied(
                            "chat input target is no longer the current Subject body".into(),
                        )
                        .into());
                    }
                }
                Ok(worker
                    .api
                    .runtime
                    .send_input(&worker.identity, request)
                    .map_err(|e| e.into_error())?)
            })
            .await
    }

    async fn ensure_interactive(&self) -> ApiResult<()> {
        let worker = self.clone();
        self.api
            .store
            .db_executor()
            .run(move || -> ApiResult<()> {
                if let Some(claim) = worker
                    .api
                    .store
                    .get_pending_ticket_assignment_operation_for_worker(
                        worker.api.workspace_id(),
                        &worker.identity,
                    )?
                {
                    return Err(Error::TicketAssignmentConflict(format!(
                        "Worker has pending Ticket claim {}; reconcile that exact operation before admitting input",
                        claim.operation_id,
                    )).into());
                }
                if worker
                    .api
                    .store
                    .worker_registry_projection(worker.api.workspace_id(), &worker.identity)?
                    .and_then(|projection| projection.job)
                    .is_some()
                {
                    return Err(Error::InvalidInput("Backend Job Workers accept only their Backend-owned immutable input; Console is read-only".into()).into());
                }
                Ok(())
            })
            .await
    }

    pub(super) async fn set_pinned(
        &self,
        context: &WorkerOperationContext,
        pinned: bool,
    ) -> ApiResult<WorkerRetentionResponse> {
        let guard = self.authorize_operation(context, "pin").await?;
        let worker = self.clone();
        self.api
            .store
            .db_executor()
            .run(move || -> ApiResult<WorkerRetentionResponse> {
                // Publication and its DB mutation are one owned stage. Accepted
                // work retains control admission through commit and publication.
                let _guard = guard;
                let retention_state = if pinned { "pinned" } else { "normal" };
                let changed = worker.api.worker_projection.publish_ordered(|| {
                    worker
                        .api
                        .store
                        .update_worker_retention(
                            worker.api.workspace_id(),
                            &worker.identity,
                            retention_state,
                            now_registry_timestamp().as_str(),
                        )
                        .map(|commit| (!commit.changes.is_empty(), commit))
                })?;
                if !changed {
                    return Err(cleanup_api_error(
                        &worker.identity.runtime_id,
                        "workspace_worker_retention_unknown_worker",
                        "Worker is not known to the Backend registry",
                    ));
                }
                Ok(WorkerRetentionResponse {
                    workspace_id: worker.api.workspace_id().to_owned(),
                    runtime_id: worker.identity.runtime_id.clone(),
                    worker_id: worker.identity.worker_id.clone(),
                    pinned,
                    retention_state: retention_state.into(),
                })
            })
            .await
    }

    /// The caller owns the full cleanup-plan digest check. The removal service
    /// rechecks live assignment/retention/use at its existing commit boundary.
    pub(super) async fn delete_from_plan(
        &self,
        context: &WorkerOperationContext,
        candidate: &CleanupWorkerCandidate,
    ) -> ApiResult<()> {
        let _guard = self.authorize_operation(context, "remove").await?;
        if candidate.runtime_id != self.identity.runtime_id
            || candidate.runtime_worker_id != self.identity.worker_id
        {
            return Err(Error::InvalidInput(
                "cleanup candidate does not address the resolved Worker".into(),
            )
            .into());
        }
        WorkerRemovalService::new(&self.api)
            .execute_cleanup_removal(candidate)
            .await
    }

    pub(crate) async fn connect_protocol(
        &self,
        context: &WorkerOperationContext,
    ) -> Result<WorkspaceWorkerConnection> {
        let _guard = self
            .authorize_operation(context, "observe")
            .await
            .map_err(|error| error.error)?;
        let source = match context {
            WorkerOperationContext::Browser { source, .. } => Some(source),
            _ => None,
        };
        let connection =
            connect_workspace_worker_protocol(&self.api, &self.identity, source).await?;
        Ok(WorkspaceWorkerConnection {
            sender: WorkspaceWorkerMethodSender::new(
                self.clone(),
                connection.methods,
                connection.termination,
            ),
            events: connection.events,
        })
    }
}

/// An execution connection is bound to exactly the identity which created it. No caller
/// can pair a different target handle with a subscription's raw sending capability.
#[derive(Clone)]
pub(crate) struct WorkspaceWorkerMethodSender {
    worker: WorkspaceWorker,
    methods: tokio::sync::mpsc::Sender<protocol::Method>,
    pub(super) termination: WorkerProtocolTermination,
}
pub(crate) struct WorkspaceWorkerConnection {
    pub(crate) sender: WorkspaceWorkerMethodSender,
    pub(crate) events: tokio::sync::mpsc::Receiver<protocol::Event>,
}
impl WorkspaceWorkerMethodSender {
    pub(crate) fn end_status(&self) -> tokio::sync::watch::Receiver<Option<WorkerProtocolEnd>> {
        self.termination.subscribe()
    }
    pub(super) fn new(
        worker: WorkspaceWorker,
        methods: tokio::sync::mpsc::Sender<protocol::Method>,
        termination: WorkerProtocolTermination,
    ) -> Self {
        Self {
            worker,
            methods,
            termination,
        }
    }
    pub(crate) async fn send(
        &self,
        context: &WorkerOperationContext,
        method: protocol::Method,
    ) -> Result<()> {
        match method {
            protocol::Method::Pause { command } => self.pause(context, command).await,
            protocol::Method::Resume { command } => self.resume(context, command).await,
            protocol::Method::Cancel { command } => self.cancel(context, command).await,
            protocol::Method::Compact { command } => self.compact(context, command).await,
            protocol::Method::Submit {
                submission_request_id,
                input,
            } => self.submit(context, submission_request_id, input).await,
            protocol::Method::Notify {
                notification_request_id,
                message,
            } => self.notify(context, notification_request_id, message).await,
            other => self.dispatch(context, other).await,
        }
    }
    async fn dispatch(
        &self,
        context: &WorkerOperationContext,
        method: protocol::Method,
    ) -> Result<()> {
        self.send_checked(context, method)
            .await
            .map_err(|error| error.error)
    }
    pub(crate) async fn pause(
        &self,
        context: &WorkerOperationContext,
        command: protocol::WorkerCommandEnvelope,
    ) -> Result<()> {
        self.dispatch(context, protocol::Method::Pause { command })
            .await
    }
    pub(crate) async fn resume(
        &self,
        context: &WorkerOperationContext,
        command: protocol::WorkerCommandEnvelope,
    ) -> Result<()> {
        self.dispatch(context, protocol::Method::Resume { command })
            .await
    }
    pub(crate) async fn cancel(
        &self,
        context: &WorkerOperationContext,
        command: protocol::WorkerCommandEnvelope,
    ) -> Result<()> {
        self.dispatch(context, protocol::Method::Cancel { command })
            .await
    }
    pub(crate) async fn compact(
        &self,
        context: &WorkerOperationContext,
        command: protocol::WorkerCommandEnvelope,
    ) -> Result<()> {
        self.dispatch(context, protocol::Method::Compact { command })
            .await
    }
    pub(crate) async fn submit(
        &self,
        context: &WorkerOperationContext,
        submission_request_id: String,
        input: Vec<protocol::Segment>,
    ) -> Result<()> {
        self.dispatch(
            context,
            protocol::Method::Submit {
                submission_request_id,
                input,
            },
        )
        .await
    }
    pub(crate) async fn notify(
        &self,
        context: &WorkerOperationContext,
        notification_request_id: String,
        message: String,
    ) -> Result<()> {
        self.dispatch(
            context,
            protocol::Method::Notify {
                notification_request_id,
                message,
            },
        )
        .await
    }

    async fn send_checked(
        &self,
        context: &WorkerOperationContext,
        method: protocol::Method,
    ) -> ApiResult<()> {
        let _guard = self
            .worker
            .authorize_operation(context, "send_input")
            .await?;
        let session_lock = current_worker_session_lock(&self.worker.api, &self.worker.identity);
        let _session_guard = session_lock.lock().await;
        self.worker.ensure_interactive().await?;
        let WorkerOperationContext::Browser { source, .. } = context else {
            return Err(Error::WorkspacePermissionDenied(
                "Browser Worker protocol context required".into(),
            )
            .into());
        };
        let method = authorize_browser_worker_method(method, source)
            .map_err(|message| Error::InvalidInput(message.into()))?;
        // Session exclusion fences attach/stop only through admission, never delivery.
        let result = if self.termination.reason().is_some() {
            Err(tokio::sync::mpsc::error::TrySendError::Closed(method))
        } else {
            self.methods.try_send(method)
        };
        result.map_err(|error| {
            let (code, message) = match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    self.termination.terminate(WorkerProtocolEnd::MethodOverflow);
                    ("worker_protocol_overflow", "Worker method queue overflow; this method was not admitted. Earlier pending methods may have unknown delivery; reconnect with a fresh snapshot, no automatic retry")
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) =>
                    ("worker_protocol_closed", "Worker connection closed; pending delivery may be unknown; no implicit Restore or retry was performed"),
            };
            Error::RuntimeOperationFailed {
                runtime_id: self.worker.identity.runtime_id.clone(),
                code: code.into(), message: message.into(),
            }.into()
        })
    }
}

#[cfg(test)]
mod input_binding_tests {
    use super::*;

    fn notify(client_id: &str) -> WorkerInputRequest {
        WorkerInputRequest {
            kind: WorkerInputKind::Notify,
            content: "message".into(),
            source: Some(protocol::AuthenticatedInputSource::Backend {
                operation_id: "forged-source".into(),
            }),
            submission_request_id: Some(client_id.into()),
            segments: None,
        }
    }

    #[test]
    fn generic_input_binds_caller_source_and_isolates_client_ids_from_chat_and_other_paths() {
        let browser = |id: &str| WorkerOperationContext::Browser {
            headers: HeaderMap::new(),
            source: protocol::AuthenticatedInputSource::Account {
                account_id: id.into(),
            },
        };
        let context = browser("account-a");
        let raw_id = "chat-message:stored-message-uuid";
        let mut request = notify(raw_id);
        context.bind_input(&mut request);
        assert_eq!(
            request.source,
            Some(protocol::AuthenticatedInputSource::Account {
                account_id: "account-a".into(),
            })
        );
        let bounded = request.submission_request_id.as_deref().unwrap();
        assert!(bounded.starts_with("runtime-input:notify:"));
        assert!(bounded.len() < 128);
        assert_ne!(bounded, raw_id);
        let mut retry = notify(raw_id);
        context.bind_input(&mut retry);
        assert_eq!(retry.submission_request_id, request.submission_request_id);
        let mut other = notify(raw_id);
        browser("account-b").bind_input(&mut other);
        assert_ne!(other.submission_request_id, request.submission_request_id);
        let mut submit = notify(raw_id);
        submit.kind = WorkerInputKind::User;
        context.bind_input(&mut submit);
        assert_ne!(submit.submission_request_id, request.submission_request_id);
        // JSON tuple framing keeps delimiter/quote characters unambiguous.
        let mut delimiter = notify("a\",\"b");
        context.bind_input(&mut delimiter);
        assert_ne!(
            delimiter.submission_request_id,
            request.submission_request_id
        );
        let mut no_client_id = notify(raw_id);
        no_client_id.submission_request_id = None;
        context.bind_input(&mut no_client_id);
        assert!(no_client_id.submission_request_id.is_none());
    }

    #[test]
    fn worker_control_provenance_is_the_authenticated_controller_not_a_body_or_source_claim() {
        let context = WorkerOperationContext::WorkerControl {
            controller: RuntimeWorkerRef::new("controller-runtime", "controller-worker"),
        };
        let mut request = notify("same-id");
        request.content = r#"{"source":{"kind":"account","account_id":"spoofed"}}"#.into();
        context.bind_input(&mut request);
        assert_eq!(
            request.source,
            Some(protocol::AuthenticatedInputSource::Worker {
                runtime_id: "controller-runtime".into(),
                worker_id: "controller-worker".into(),
            })
        );
        assert!(
            request
                .submission_request_id
                .as_deref()
                .unwrap()
                .starts_with("runtime-input:notify:")
        );
        assert!(request.submission_request_id.as_ref().unwrap().len() < 128);
    }
}
