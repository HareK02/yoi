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
    pub(super) fn from_bootstrap(stage: RuntimeRestoreStage, error: &WorkerBootstrapError) -> Self {
        match error {
            WorkerBootstrapError::Worker(source) => Self::from_worker(stage, source),
            WorkerBootstrapError::Controller { source, .. } => {
                let mut failure = Self::started(
                    RuntimeRestoreStage::Controller,
                    "Worker controller startup failed",
                );
                failure.io_kind = Some(source.kind());
                failure
            }
        }
    }
}

impl From<session_store::WorkerStoreError> for RuntimeRestoreFailure {
    fn from(error: session_store::WorkerStoreError) -> Self {
        Self::from_worker(
            RuntimeRestoreStage::Metadata,
            &WorkerError::WorkerStore(error),
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
