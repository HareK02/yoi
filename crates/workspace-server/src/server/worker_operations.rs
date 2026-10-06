//! Workspace identity-bound operation reception. Runtime/controller remain execution authority.
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
        self.current_record()
            .map_err(|error| error.error.to_string())?;
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
        self.current_record()?;
        match context {
            #[cfg(test)]
            WorkerOperationContext::Backend => Ok(None),
            WorkerOperationContext::Browser { .. } => {
                context.authorize_workspace(&self.api).await?;
                Ok(None)
            }
            WorkerOperationContext::WorkerControl { controller } => {
                if self
                    .api
                    .store
                    .get_worker_registry(self.api.workspace_id(), controller)?
                    .is_none()
                {
                    return Err(Error::UnknownWorker {
                        worker: controller.clone(),
                    }
                    .into());
                }
                let grant = authorize_known_worker_permission(
                    &self.api,
                    self.api.workspace_id(),
                    controller,
                    &self.identity,
                    permission,
                )?;
                let guard = worker_control_lock(&self.api, &grant.grant_id)
                    .lock_owned()
                    .await;
                self.current_record()?;
                let current_grant = authorize_known_worker_permission(
                    &self.api,
                    self.api.workspace_id(),
                    controller,
                    &self.identity,
                    permission,
                )?;
                if current_grant.grant_id != grant.grant_id {
                    return Err(Error::RepositoryConflict(
                        "Worker control grant changed while waiting for admission".into(),
                    )
                    .into());
                }
                Ok(Some(guard))
            }
        }
    }

    pub(super) async fn restore(
        &self,
        context: &WorkerOperationContext,
        request: server_api::WorkerRestoreRequest,
    ) -> ApiResult<InternalWorkerRestoreResult> {
        let _guard = self.authorize_operation(context, "restore").await?;
        self.restore_internal(request)
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
        let _guard = self.authorize_operation(context, "stop").await?;
        let result = self
            .api
            .runtime
            .stop_worker(&self.identity, request)
            .map_err(|e| e.into_error())?;
        if result.state != InternalWorkerOperationState::Accepted {
            return Ok(result);
        }
        parse_runtime_worker_id_for_registry(&self.identity.worker_id)?;
        let session_lock = current_worker_session_lock(&self.api, &self.identity);
        let _session_guard = session_lock.lock().await;
        close_current_worker_session_locked(&self.api, &self.identity).await?;
        if let Some(record) = self
            .api
            .store
            .get_worker_registry(self.api.workspace_id(), &self.identity)?
        {
            sync_linked_workdir_after_worker_stop(&self.api, &self.identity.runtime_id, &record)?;
        }
        Ok(result)
    }

    pub(super) async fn cancel(
        &self,
        context: &WorkerOperationContext,
        request: WorkerLifecycleRequest,
    ) -> ApiResult<WorkerLifecycleResult> {
        let _guard = self.authorize_operation(context, "cancel").await?;
        Ok(self
            .api
            .runtime
            .cancel_worker(&self.identity, request)
            .map_err(|e| e.into_error())?)
    }

    pub(super) async fn input(
        &self,
        context: &WorkerOperationContext,
        mut request: WorkerInputRequest,
    ) -> ApiResult<WorkerInputResult> {
        let permission = if request.kind == WorkerInputKind::Notify {
            "notify"
        } else {
            "send_input"
        };
        let _guard = self.authorize_operation(context, permission).await?;
        self.ensure_interactive()?;
        if matches!(context, WorkerOperationContext::WorkerControl { .. })
            && request.kind == WorkerInputKind::User
        {
            request.kind = WorkerInputKind::UserIfIdle;
        }
        Ok(self
            .api
            .runtime
            .send_input(&self.identity, request)
            .map_err(|e| e.into_error())?)
    }

    fn ensure_interactive(&self) -> ApiResult<()> {
        if self
            .api
            .store
            .worker_registry_projection(self.api.workspace_id(), &self.identity)?
            .and_then(|projection| projection.job)
            .is_some()
        {
            return Err(Error::InvalidInput("Backend Job Workers accept only their Backend-owned immutable input; Console is read-only".into()).into());
        }
        Ok(())
    }

    pub(super) async fn set_pinned(
        &self,
        context: &WorkerOperationContext,
        pinned: bool,
    ) -> ApiResult<WorkerRetentionResponse> {
        let _guard = self.authorize_operation(context, "pin").await?;
        let retention_state = if pinned { "pinned" } else { "normal" };
        let changed = self.api.worker_projection.publish_ordered(|| {
            self.api
                .store
                .update_worker_retention(
                    self.api.workspace_id(),
                    &self.identity,
                    retention_state,
                    now_registry_timestamp().as_str(),
                )
                .map(|commit| (!commit.changes.is_empty(), commit))
        })?;
        if !changed {
            return Err(cleanup_api_error(
                &self.identity.runtime_id,
                "workspace_worker_retention_unknown_worker",
                "Worker is not known to the Backend registry",
            ));
        }
        Ok(WorkerRetentionResponse {
            workspace_id: self.api.workspace_id().to_owned(),
            runtime_id: self.identity.runtime_id.clone(),
            worker_id: self.identity.worker_id.clone(),
            pinned,
            retention_state: retention_state.into(),
        })
    }

    /// The caller owns the full cleanup-plan revision/digest check. The removal service
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
            sender: WorkspaceWorkerMethodSender {
                worker: self.clone(),
                methods: connection.methods,
            },
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
}
pub(crate) struct WorkspaceWorkerConnection {
    pub(crate) sender: WorkspaceWorkerMethodSender,
    pub(crate) events: tokio::sync::mpsc::Receiver<protocol::Event>,
}
impl WorkspaceWorkerMethodSender {
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
        self.worker.ensure_interactive()?;
        let WorkerOperationContext::Browser { source, .. } = context else {
            return Err(Error::WorkspacePermissionDenied(
                "Browser Worker protocol context required".into(),
            )
            .into());
        };
        let method = authorize_browser_worker_method(method, source)
            .map_err(|message| Error::InvalidInput(message.into()))?;
        self.methods.send(method).await.map_err(|_| {
            Error::RuntimeOperationFailed {
                runtime_id: self.worker.identity.runtime_id.clone(),
                code: "worker_protocol_closed".into(),
                message:
                    "Worker execution connection closed; no implicit Restore or retry was performed"
                        .into(),
            }
            .into()
        })
    }
}
