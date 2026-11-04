//! Conversion preserves owner-selected facts. The string-only legacy seams carry
//! no domain verdict; they retain their diagnostic privately and receive a coarse
//! Runtime fallback, never a cause guessed from that diagnostic.
use super::*;

impl From<RuntimeRestoreStage> for protocol::startup_failure::StartupStage {
    fn from(stage: RuntimeRestoreStage) -> Self {
        match stage {
            RuntimeRestoreStage::Preflight => Self::Preflight,
            RuntimeRestoreStage::Metadata => Self::Metadata,
            RuntimeRestoreStage::Reconstruction => Self::Reconstruction,
            RuntimeRestoreStage::Recovery => Self::Recovery,
            RuntimeRestoreStage::Binding => Self::Binding,
            RuntimeRestoreStage::Feature => Self::Feature,
            RuntimeRestoreStage::ActivationEffects => Self::ActivationEffects,
            RuntimeRestoreStage::Controller => Self::Controller,
        }
    }
}

impl From<RuntimeRestoreCleanupFailure> for protocol::startup_failure::StartupCleanupFailure {
    fn from(failure: RuntimeRestoreCleanupFailure) -> Self {
        match failure {
            RuntimeRestoreCleanupFailure::Ownership => Self::Ownership,
            RuntimeRestoreCleanupFailure::Workdir => Self::Workdir,
            RuntimeRestoreCleanupFailure::Artifacts => Self::Artifacts,
            RuntimeRestoreCleanupFailure::Allocation => Self::Allocation,
            RuntimeRestoreCleanupFailure::Feature => Self::Feature,
            RuntimeRestoreCleanupFailure::Unsupported => Self::Unsupported,
        }
    }
}

impl RuntimeRestoreFailure {
    pub(super) fn from_bootstrap(stage: RuntimeRestoreStage, error: WorkerBootstrapError) -> Self {
        let mut failure = match &error {
            WorkerBootstrapError::Worker(source) => Self::from_worker_ref(stage, source),
            WorkerBootstrapError::Controller { source, .. } => {
                let mut failure = Self::started(
                    RuntimeRestoreStage::Controller,
                    "Worker controller startup failed",
                );
                failure.io_kind = Some(source.kind());
                failure
            }
        };
        failure.source = Some(Arc::new(error));
        failure
    }
}

impl From<session_store::WorkerStoreError> for RuntimeRestoreFailure {
    fn from(error: session_store::WorkerStoreError) -> Self {
        Self::from_worker(
            RuntimeRestoreStage::Metadata,
            WorkerError::WorkerStore(error),
        )
    }
}

impl From<String> for RuntimeRestoreFailure {
    fn from(diagnostic: String) -> Self {
        let mut failure =
            Self::unproven(RuntimeRestoreStage::Reconstruction, "Worker factory failed");
        failure.legacy_diagnostic = Some(diagnostic);
        failure
    }
}
impl From<&'static str> for RuntimeRestoreFailure {
    fn from(diagnostic: &'static str) -> Self {
        diagnostic.to_string().into()
    }
}

impl PendingRuntimeRestore {
    /// Adapter waiting/cleanup errors supplement, rather than stringify and replace,
    /// any primary result already received from this exact operation's owner.
    pub(super) fn failure_result(&self, message: String) -> WorkerExecutionResult {
        let failure = self
            .result
            .lock()
            .ok()
            .and_then(|saved| {
                saved
                    .as_ref()
                    .and_then(|result| result.as_ref().err())
                    .cloned()
            })
            .or_else(|| {
                self.activated.lock().ok().and_then(|saved| {
                    saved
                        .as_ref()
                        .and_then(|result| result.as_ref().err())
                        .cloned()
                })
            });
        let mut result = failure
            .map(|failure| failure.execution_result(WorkerExecutionOperation::Restore))
            .unwrap_or_else(|| {
                WorkerExecutionResult::errored(WorkerExecutionOperation::Restore, message.clone())
            });
        result.message = Some(message);
        result
    }
}

pub(super) fn finalize_startup_binding<St>(
    worker: &Worker<Box<dyn agen::llm_client::LlmClient>, St>,
    attached: bool,
) -> Result<(), RuntimeRestoreFailure>
where
    St: Store + WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    worker
        .finalize_subjektiv_session_attribution_retaining_startup_failure(
            attached,
            SubjektivSessionAttributionLifecycle::NewSession,
        )
        .map_err(|error| RuntimeRestoreFailure::from_worker(RuntimeRestoreStage::Binding, error))
}

#[cfg(test)]
mod tests {
    //! Connect the typed factory seam to the real Tokio adapter and protocol codec.
    //! The external startup action is a deterministic failure, not a real domain
    //! authorization decision. Resource disposition remains the existing backend result.
    use super::*;
    use crate::execution::WorkerExecutionContext;
    use crate::identity::WorkerId;
    use protocol::startup_failure::*;

    struct TypedFailingFactory;
    fn primary() -> StartupFailureAttachment {
        worker::startup_failure::runtime_startup_attachment(RuntimeStartupCause {
            stage: StartupStage::Feature,
            reason: RuntimeStartupReason::FeatureFailed,
        })
        .unwrap()
    }
    fn failure() -> RuntimeRestoreFailure {
        let mut failure = RuntimeRestoreFailure::unproven(
            RuntimeRestoreStage::Controller,
            "factory failed after receiving primary",
        )
        .with_startup_failure(primary());
        failure.cleanup = RuntimeRestoreCleanup::Failed(RuntimeRestoreCleanupFailure::Workdir);
        failure.cleanup_failure = Some(RuntimeRestoreCleanupFailure::Workdir);
        failure.io_kind = Some(std::io::ErrorKind::PermissionDenied);
        failure
    }
    #[async_trait]
    impl RuntimeWorkerFactory for TypedFailingFactory {
        async fn spawn_controller(
            &self,
            _: WorkerExecutionSpawnRequest,
        ) -> Result<RuntimeWorkerController, RuntimeRestoreFailure> {
            Err(failure())
        }
        async fn preflight_restore(
            &self,
            _: &WorkerExecutionRestoreRequest,
        ) -> Result<(), RuntimeRestoreFailure> {
            Err(failure())
        }
        async fn prepare_restore(
            &self,
            _: WorkerExecutionRestoreRequest,
        ) -> Result<Box<dyn RuntimeRestoreCandidate>, RuntimeRestoreFailure> {
            panic!("typed preflight rejection must not prepare live work")
        }
    }

    fn request() -> WorkerExecutionSpawnRequest {
        let worker_ref = WorkerRef::new(WorkerId::now_v7());
        // No factory reads this archive: it is explicit launch input, not a retained approval.
        let archive = crate::profile_archive::ProfileSourceArchive::build(
            crate::profile_archive::ProfileSourceArchiveInput {
                id: "profile-source-archive:typed-startup".into(),
                entrypoints: BTreeMap::from([(
                    "builtin:companion".into(),
                    "profiles/default.dcdl".into(),
                )]),
                imports: BTreeMap::new(),
                sources: BTreeMap::from([("profiles/default.dcdl".into(), "{}".into())]),
            },
        )
        .unwrap();
        let request = crate::catalog::CreateWorkerRequest {
            worker_id: worker_ref.worker_id,
            create_fingerprint: "typed-startup-failure".into(),
            profile: crate::catalog::ProfileSelector::Builtin("builtin:companion".into()),
            display_name: None,
            profile_source: crate::catalog::ProfileSourceArchiveSource::Embedded { archive },
            config_bundle: None,
            initial_input: None,
            workdir_attachment_requests: Vec::new(),
            workdir_attachments: Vec::new(),
            worker_observation_enabled: false,
            worker_observation_grants: Vec::new(),
            workspace_api: None,
            memory_settings: None,
            plugin_execution: None,
            subjektiv_attached: false,
            backend_job: None,
        };
        #[cfg(feature = "ws-server")]
        let context = WorkerExecutionContext::new(
            worker_ref.clone(),
            Arc::new(|_, _| unreachable!("failed startup must not publish a Controller")),
        );
        #[cfg(not(feature = "ws-server"))]
        let context = WorkerExecutionContext::new(worker_ref.clone());
        WorkerExecutionSpawnRequest {
            worker_ref,
            request,
            workspace_scope: None,
            context,
            workdir_attachments: BTreeMap::new(),
            config_bundle: None,
        }
    }

    fn assert_primary(result: &WorkerExecutionResult) {
        assert_eq!(result.startup_failure.as_ref(), Some(&primary()));
        let Some(error_contract::DecodeFailure::Known(received)) =
            result.startup_failure::<RuntimeStartupCause>()
        else {
            panic!("typed primary missing")
        };
        assert_eq!(received.facts().cause().stage, StartupStage::Feature);
        assert_eq!(
            received.facts().cause().reason,
            RuntimeStartupReason::FeatureFailed
        );
        assert_eq!(
            received.facts().context().operations[0].operation,
            "worker_startup"
        );
        assert!(
            result
                .secondary_failures
                .contains(&StartupSecondaryFailure::Cleanup {
                    failure: StartupCleanupFailure::Workdir
                })
        );
        assert_eq!(
            result.startup_io_kind,
            Some(StartupIoKind::PermissionDenied)
        );
        assert!(!result.is_accepted());
    }

    #[test]
    fn typed_factory_failure_survives_adapter_and_protocol_without_changing_disposition() {
        let backend = WorkerRuntimeExecutionBackend::new(TypedFailingFactory).unwrap();
        let WorkerExecutionSpawnResult::Errored(result) = backend.spawn_worker(request()) else {
            panic!("cause attachment must not reclassify the existing spawn result")
        };
        assert_primary(&result);
        let transferred: WorkerExecutionResult =
            serde_json::from_slice(&serde_json::to_vec(&result).unwrap()).unwrap();
        assert_eq!(transferred, result);
        assert_primary(&transferred);
        assert_eq!(
            transferred.startup_resources.unwrap().cleanup,
            StartupCleanup::Failed(StartupCleanupFailure::Workdir)
        );
    }

    #[test]
    fn typed_preflight_failure_survives_adapter_without_starting_live_preparation() {
        let backend = WorkerRuntimeExecutionBackend::new(TypedFailingFactory).unwrap();
        let spawn = request();
        let request = WorkerExecutionRestoreRequest {
            source_repair_possible: false,
            operation_id: crate::execution::WorkerLifecycleOperationId::new(),
            worker_ref: spawn.worker_ref,
            request: spawn.request,
            workspace_scope: None,
            context: spawn.context,
            previous_workdir_attachments: Vec::new(),
            logical_workdir_attachments: Vec::new(),
            workdir_attachments: BTreeMap::new(),
            deferred_workdir_sessions: BTreeMap::new(),
            config_bundle: None,
        };
        let result = backend.preflight_restore(&request).unwrap_err();
        assert_eq!(
            result.outcome,
            crate::execution::WorkerExecutionOutcome::Rejected
        );
        assert_primary(&result);
        let failure = backend
            .run_on_adapter_runtime(async move {
                Ok(prepare_restore_operation(&TypedFailingFactory, request)
                    .await
                    .err()
                    .unwrap())
            })
            .unwrap();
        assert_eq!(failure.startup_failure(), &primary());
    }
}
