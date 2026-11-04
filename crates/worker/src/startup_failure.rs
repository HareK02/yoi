//! Typed startup diagnostics across Worker ownership wrappers. Domain causes are
//! projected at their origin; fallback Runtime facts are selected from variants,
//! never inferred from Display or legacy attribution diagnostics.
use crate::WorkerError;
use error_contract::{FailureFacts, InternalTransfer, OperationContext, project_failure};
use protocol::startup_failure::{
    RuntimeStartupCause, RuntimeStartupReason, StartupFailureAttachment, StartupSecondaryFailure,
    StartupStage,
};

impl WorkerError {
    pub fn session_registration_cause(
        &self,
    ) -> Option<protocol::session_failure::SessionRegistrationCause> {
        match self {
            Self::SessionRegistration { source } => Some(source.session_registration_cause()),
            Self::StartupFailure { source, .. }
            | Self::ConstructionCleanup { source, .. }
            | Self::RestoreActivationEffects { source } => source.session_registration_cause(),
            _ => None,
        }
    }

    pub fn with_startup_failure(self, failure: StartupFailureAttachment) -> Self {
        Self::StartupFailure {
            source: Box::new(self),
            failure,
            secondary: Vec::new(),
            metadata_save: None,
        }
    }

    pub fn startup_failure(&self) -> Option<&StartupFailureAttachment> {
        match self {
            Self::StartupFailure { failure, .. } => Some(failure),
            Self::ConstructionCleanup { source, .. }
            | Self::RestoreActivationEffects { source } => source.startup_failure(),
            _ => None,
        }
    }

    pub fn startup_secondary_failures(&self) -> &[StartupSecondaryFailure] {
        match self {
            Self::StartupFailure { secondary, .. } => secondary,
            Self::ConstructionCleanup { source, .. }
            | Self::RestoreActivationEffects { source } => source.startup_secondary_failures(),
            _ => &[],
        }
    }

    pub fn startup_metadata_save(
        &self,
    ) -> Option<protocol::startup_failure::StartupPersistenceOutcome> {
        match self {
            Self::StartupFailure { metadata_save, .. } => *metadata_save,
            Self::ConstructionCleanup { source, .. }
            | Self::RestoreActivationEffects { source } => source.startup_metadata_save(),
            _ => None,
        }
    }

    /// Preserve the original attachment when an outer resource owner adds context.
    pub fn startup_failure_at(
        &self,
        stage: StartupStage,
    ) -> Result<StartupFailureAttachment, error_contract::ContractError> {
        if let Some(failure) = self.startup_failure() {
            return Ok(failure.clone());
        }
        if let Some(cause) = self.session_registration_cause() {
            return protocol::session_failure::session_registration_attachment(cause);
        }
        let reason = match self {
            Self::ConstructionCleanup { source, .. }
            | Self::RestoreActivationEffects { source } => return source.startup_failure_at(stage),
            Self::SegmentEmpty { .. } => RuntimeStartupReason::SegmentEmpty,
            Self::SegmentSystemPromptMissing { .. } => RuntimeStartupReason::SystemPromptMissing,
            Self::WorkerMetadataPending { .. } => RuntimeStartupReason::MetadataPending,
            Self::WorkerMetadataMissing { .. } => RuntimeStartupReason::MetadataMissing,
            Self::WorkerMetadataInactive { .. } => RuntimeStartupReason::MetadataInactive,
            Self::WorkerMetadataManifestSnapshotMissing { .. } => {
                RuntimeStartupReason::ManifestMissing
            }
            Self::ScopeLock(_) => RuntimeStartupReason::AllocationFailed,
            Self::Store(_) => RuntimeStartupReason::SessionReadFailed,
            Self::WorkerStore(_) => RuntimeStartupReason::MetadataReadFailed,
            Self::Provider(_) => RuntimeStartupReason::ProviderFailed,
            Self::SubjektivSessionAttribution { .. } => RuntimeStartupReason::AttributionFailed,
            Self::FeatureInstall(_) | Self::FeatureLifecycle(_) => {
                RuntimeStartupReason::FeatureFailed
            }
            _ => RuntimeStartupReason::StartupFailed,
        };
        runtime_startup_attachment(RuntimeStartupCause { stage, reason })
    }
}

/// The local fallback accepts only enums, so provider strings and internal paths
/// cannot accidentally enter storage or public cause details.
pub fn runtime_startup_attachment(
    cause: RuntimeStartupCause,
) -> Result<StartupFailureAttachment, error_contract::ContractError> {
    let mut facts = FailureFacts::new(cause)?;
    facts.with_context(OperationContext::new("worker_startup", "runtime")?)?;
    let projected = project_failure::<InternalTransfer, _>(&facts, &())?;
    StartupFailureAttachment::from_projected(&projected)
}

/// The internal HTTP/Host transfer admits only implemented startup domains.
/// Unknown/corrupt envelopes cannot be blindly forwarded or classified from text.
pub fn reproject_internal_startup_failure(
    failure: &StartupFailureAttachment,
) -> Result<StartupFailureAttachment, error_contract::ContractError> {
    if let Ok(projected) = failure
        .project_for::<InternalTransfer, protocol::session_failure::SessionRegistrationCause>(&())
    {
        return StartupFailureAttachment::from_projected(&projected);
    }
    let projected = failure.project_for::<InternalTransfer, RuntimeStartupCause>(&())?;
    StartupFailureAttachment::from_projected(&projected)
}

/// Dispatch by the domain attested in process, not by parsing attachment fields.
pub(crate) fn persist_binding_startup_failure<S: session_store::WorkerMetadataStore>(
    store: &S,
    expected: &session_store::worker_metadata::StartupFailureExpectation,
    source: WorkerError,
) -> WorkerError {
    if source.session_registration_cause().is_some() {
        persist_startup_failure::<_, protocol::session_failure::SessionRegistrationCause>(
            store,
            expected,
            source,
            StartupStage::Binding,
            &(),
        )
    } else {
        persist_startup_failure::<_, RuntimeStartupCause>(
            store,
            expected,
            source,
            StartupStage::Binding,
            &(),
        )
    }
}

/// An attribution-state write failure supplements the registration decision. It
/// neither establishes the write's absence nor replaces the primary Error source.
pub(crate) fn retain_attribution_save_failure(
    source: WorkerError,
    saved: Result<(), WorkerError>,
) -> WorkerError {
    use protocol::startup_failure::{StartupFailurePhase, StartupPersistenceOutcome};
    let Err(error) = saved else {
        return source;
    };
    let outcome = match error {
        WorkerError::SubjektivSessionAttribution {
            state: "attribution_conflict",
            ..
        } => StartupPersistenceOutcome::Conflict,
        WorkerError::WorkerStore(_) => StartupPersistenceOutcome::OutcomeUnknown,
        _ => StartupPersistenceOutcome::NotSaved,
    };
    let failure = source
        .startup_failure_at(StartupStage::Binding)
        .expect("fixed Session cause fits the shared codec budget");
    WorkerError::StartupFailure {
        source: Box::new(source),
        failure,
        secondary: vec![StartupSecondaryFailure::Persistence {
            phase: StartupFailurePhase::MetadataSave,
            outcome,
        }],
        metadata_save: None,
    }
}

/// Bind a caller-owned cause to the exact pre-observed Session/attribution. Saving
/// never replaces the original error, changes ownership, or issues retry authority.
pub fn persist_startup_failure<S, D>(
    store: &S,
    expected: &session_store::worker_metadata::StartupFailureExpectation,
    source: WorkerError,
    stage: StartupStage,
    authorization: &D::Authorization,
) -> WorkerError
where
    S: session_store::WorkerMetadataStore,
    D: error_contract::WireDetails + error_contract::ProjectFor<error_contract::Storage>,
{
    use protocol::startup_failure::{StartupFailurePhase, StartupPersistenceOutcome};
    use session_store::worker_metadata::StartupFailureSaveResult;
    let failure = source
        .startup_failure_at(stage)
        .expect("fixed Worker startup projection fits the error-contract budget");
    let storage = failure
        .project_for::<error_contract::Storage, D>(authorization)
        .and_then(|projected| {
            protocol::startup_failure::StoredStartupFailureAttachment::from_projected(&projected)
        });
    let outcome = match storage
        .as_ref()
        .map(|stored| store.compare_and_swap_startup_failure(expected, stored))
    {
        Ok(StartupFailureSaveResult::Persisted) => StartupPersistenceOutcome::Persisted,
        Ok(StartupFailureSaveResult::Conflict) => StartupPersistenceOutcome::Conflict,
        Ok(StartupFailureSaveResult::NotSaved(_)) => StartupPersistenceOutcome::NotSaved,
        Ok(StartupFailureSaveResult::OutcomeUnknown(_)) => {
            StartupPersistenceOutcome::OutcomeUnknown
        }
        Err(_) => StartupPersistenceOutcome::NotSaved,
    };
    let mut secondary = source.startup_secondary_failures().to_vec();
    if storage.is_err() {
        secondary.push(StartupSecondaryFailure::ProjectionUnavailable {
            phase: StartupFailurePhase::MetadataSave,
        });
    }
    if outcome != StartupPersistenceOutcome::Persisted {
        secondary.push(StartupSecondaryFailure::Persistence {
            phase: StartupFailurePhase::MetadataSave,
            outcome,
        });
    }
    WorkerError::StartupFailure {
        source: Box::new(source),
        failure,
        secondary,
        metadata_save: Some(outcome),
    }
}

#[cfg(test)]
mod tests {
    //! Connect Worker error retention, projection and metadata CAS to the real filesystem store.
    //! One wrapper deliberately loses the save response after the real CAS commits; it does not
    //! replace persistence or claim a real OS crash. Run in the normal worker lib test target.
    use super::*;
    use protocol::startup_failure::{
        StartupFailurePhase, StartupPersistenceOutcome, StoredStartupFailureAttachment,
    };
    use session_store::worker_metadata::{
        StartupFailureExpectation, StartupFailureRead, StartupFailureSaveResult,
    };
    use session_store::{
        FsWorkerStore, WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore,
        WorkerStoreError,
    };

    struct LostSaveResponse(FsWorkerStore);
    impl WorkerMetadataStore for LostSaveResponse {
        fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
            self.0.write(metadata)
        }
        fn read_by_name(&self, name: &str) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
            self.0.read_by_name(name)
        }
        fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
            self.0.list_names()
        }
        fn delete_by_name(&self, name: &str) -> Result<(), WorkerStoreError> {
            self.0.delete_by_name(name)
        }
        fn compare_and_swap_startup_failure(
            &self,
            expected: &StartupFailureExpectation,
            failure: &StoredStartupFailureAttachment,
        ) -> StartupFailureSaveResult {
            match self.0.compare_and_swap_startup_failure(expected, failure) {
                StartupFailureSaveResult::Persisted => StartupFailureSaveResult::OutcomeUnknown(
                    WorkerStoreError::Io(std::io::Error::other("TEST_ONLY lost response")),
                ),
                other => other,
            }
        }
    }

    fn metadata() -> WorkerMetadata {
        WorkerMetadata::new(
            "startup",
            Some(WorkerActiveSegmentRef::pending_segment(
                session_store::new_session_id(),
            )),
        )
        .with_workspace_id("workspace")
    }

    fn error() -> WorkerError {
        WorkerError::FeatureInstall(
            "TEST_ONLY_CREDENTIAL /private/internal/path raw_http_body".into(),
        )
    }

    #[test]
    fn startup_error_save_reopens_same_cause_without_replacing_primary_or_disclosing_source() {
        let dir = tempfile::tempdir().unwrap();
        let store = FsWorkerStore::new(dir.path()).unwrap();
        let original = metadata();
        store.write(&original).unwrap();
        let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
        let source = error();
        let primary = source.startup_failure_at(StartupStage::Feature).unwrap();
        let retained = persist_startup_failure::<_, RuntimeStartupCause>(
            &store,
            &expected,
            source,
            StartupStage::Feature,
            &(),
        );
        assert_eq!(retained.startup_failure(), Some(&primary));
        assert_eq!(
            retained.startup_metadata_save(),
            Some(StartupPersistenceOutcome::Persisted)
        );
        let reopened = FsWorkerStore::new(dir.path())
            .unwrap()
            .read_by_name("startup")
            .unwrap()
            .unwrap();
        assert!(matches!(
            reopened.decode_startup_failure::<RuntimeStartupCause>(),
            StartupFailureRead::Present(error_contract::DecodeFailure::Known(_))
        ));
        for secret in [
            "TEST_ONLY_CREDENTIAL",
            "/private/internal/path",
            "raw_http_body",
        ] {
            assert!(
                !serde_json::to_string(&reopened.startup_failure)
                    .unwrap()
                    .contains(secret)
            );
        }
        assert_eq!(reopened.active, original.active);
        assert_eq!(
            reopened.subjektiv_session_attribution,
            original.subjektiv_session_attribution
        );
    }

    #[test]
    fn startup_error_unknown_save_keeps_primary_and_requires_reconciliation_of_real_commit() {
        let dir = tempfile::tempdir().unwrap();
        let store = LostSaveResponse(FsWorkerStore::new(dir.path()).unwrap());
        let original = metadata();
        store.write(&original).unwrap();
        let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
        let source = error();
        let primary = source.startup_failure_at(StartupStage::Feature).unwrap();
        let retained = persist_startup_failure::<_, RuntimeStartupCause>(
            &store,
            &expected,
            source,
            StartupStage::Feature,
            &(),
        );
        assert_eq!(retained.startup_failure(), Some(&primary));
        assert_eq!(
            retained.startup_metadata_save(),
            Some(StartupPersistenceOutcome::OutcomeUnknown)
        );
        assert!(retained.startup_secondary_failures().contains(
            &StartupSecondaryFailure::Persistence {
                phase: StartupFailurePhase::MetadataSave,
                outcome: StartupPersistenceOutcome::OutcomeUnknown,
            }
        ));
        let reopened = FsWorkerStore::new(dir.path())
            .unwrap()
            .read_by_name("startup")
            .unwrap()
            .unwrap();
        assert!(matches!(
            reopened.decode_startup_failure::<RuntimeStartupCause>(),
            StartupFailureRead::Present(error_contract::DecodeFailure::Known(_))
        ));
        // The original observation must not be retried as a fresh write after an unknown response.
        assert!(matches!(
            store.compare_and_swap_startup_failure(
                &expected,
                &StoredStartupFailureAttachment::from_projected(
                    &primary
                        .project_for::<error_contract::Storage, RuntimeStartupCause>(&())
                        .unwrap()
                )
                .unwrap()
            ),
            StartupFailureSaveResult::Conflict
        ));
    }

    #[test]
    fn startup_error_save_failure_and_conflict_preserve_original_primary() {
        for conflict in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let store = FsWorkerStore::new(dir.path()).unwrap();
            let original = metadata();
            store.write(&original).unwrap();
            let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
            if conflict {
                store
                    .set_active(
                        "startup",
                        Some(WorkerActiveSegmentRef::pending_segment(
                            session_store::new_session_id(),
                        )),
                        None,
                    )
                    .unwrap();
            } else {
                let path = dir.path().join("startup/metadata.json");
                std::fs::remove_file(&path).unwrap();
                std::fs::create_dir(&path).unwrap(); // Real read failure before a save can be attempted.
            }
            let source = error();
            let primary = source.startup_failure_at(StartupStage::Feature).unwrap();
            let retained = persist_startup_failure::<_, RuntimeStartupCause>(
                &store,
                &expected,
                source,
                StartupStage::Feature,
                &(),
            );
            assert_eq!(retained.startup_failure(), Some(&primary));
            assert_eq!(
                retained.startup_metadata_save(),
                Some(if conflict {
                    StartupPersistenceOutcome::Conflict
                } else {
                    StartupPersistenceOutcome::NotSaved
                })
            );
        }
    }
}
