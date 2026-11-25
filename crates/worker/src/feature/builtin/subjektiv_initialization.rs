//! Subjektiv owns launch meaning, policy binding and live Host identity checks.
//! Saved data is a stable expectation, never authority to reconnect a Subject.

use crate::feature::{
    FeatureDescriptor,
    initialization::{FeatureInitializationAdapter, InitializationContext, InitializationPhase},
};
use crate::{Worker, WorkerError};
mod attribution;
use agen::llm_client::client::LlmClient;
pub use attribution::SubjektivSessionAttributionLifecycle;
use protocol::feature_initialization::FeatureInitializations;
use session_store::{Store, SubjektivSessionAttributionState, WorkerMetadata, WorkerMetadataStore};
use subjektiv_api::worker_initialization::{
    FEATURE_ID, ResolvedWorkerConnection, VERSION, WorkerConnectionKind,
};

pub(crate) struct SubjektivInitialization;

fn connection(data: &FeatureInitializations) -> Result<Option<ResolvedWorkerConnection>, String> {
    data.get(FEATURE_ID)
        .map(|entry| {
            let resolved: ResolvedWorkerConnection = entry.decode(FEATURE_ID, VERSION)?;
            resolved.validate()?;
            Ok(resolved)
        })
        .transpose()
}

fn validate_binding(
    context: &InitializationContext<'_>,
) -> Result<Option<ResolvedWorkerConnection>, String> {
    let resolved = connection(context.data)?;
    let job = context.backend_job;
    if job.is_some_and(|j| j.subjektiv_consolidation) && resolved.is_none() {
        return Err("consolidation Job requires its trusted Subjektiv initialization".into());
    }
    if let Some(resolved) = &resolved {
        if context.workspace_id != Some(resolved.workspace_id.as_str()) {
            return Err("Subjektiv initialization does not match Workspace API scope".into());
        }
        match resolved.kind {
            WorkerConnectionKind::SubjectBody if job.is_some() => {
                return Err("Backend Job cannot use a Subject body connection".into());
            }
            WorkerConnectionKind::MemoryConsolidation
                if !job.is_some_and(|j| j.subjektiv_consolidation) =>
            {
                return Err(
                    "consolidation connection requires its independent Backend Job binding".into(),
                );
            }
            _ => {}
        }
        let snapshot = context
            .memory_settings
            .ok_or("subject-attached Worker is missing its trusted settings snapshot")?;
        if snapshot.workspace_id != resolved.workspace_id {
            return Err("Subjektiv settings Workspace mismatch".into());
        }
    }
    if resolved.is_none() && context.memory_settings.is_some() {
        return Err("Memory settings require an explicit Subjektiv connection".into());
    }
    Ok(resolved)
}

fn validate_job_policy(
    manifest: &manifest::WorkerManifest,
    context: &InitializationContext<'_>,
) -> Result<(), String> {
    if let Some(binding) = context.backend_job {
        let policy = &manifest.feature.subjektiv.profile;
        if policy.consolidation_tools != binding.subjektiv_consolidation
            || ((binding.subjektiv_consolidation || binding.subjektiv_journal)
                && policy.extraction.enabled)
        {
            return Err("Backend Job tools must match their independent trusted grants and extraction must be disabled".into());
        }
    }
    Ok(())
}

impl<C, St> FeatureInitializationAdapter<C, St> for SubjektivInitialization
where
    C: LlmClient + Clone + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("subjektiv", "Subjektiv initialization")
    }
    fn validate_transport(&self, context: &InitializationContext<'_>) -> Result<(), String> {
        if let Some(binding) = context.backend_job {
            binding.validate().map_err(str::to_owned)?;
            if context.workspace_id.is_none() {
                return Err("Backend Job capability requires Workspace authority".into());
            }
        }
        validate_binding(context).map(|_| ())
    }
    fn bind_manifest(
        &self,
        manifest: &mut manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        validate_job_policy(manifest, context)?;
        if validate_binding(context)?.is_some() {
            if !manifest.feature.subjektiv.profile.enabled {
                return Err(
                    "subject-attached Worker profile does not enable subjektiv policy".into(),
                );
            }
            manifest
                .feature
                .subjektiv
                .bind_workspace_settings(
                    context.memory_settings.expect("validated snapshot").clone(),
                )
                .map_err(str::to_owned)?;
        }
        Ok(())
    }
    fn validate_manifest(
        &self,
        manifest: &manifest::WorkerManifest,
        context: &InitializationContext<'_>,
    ) -> Result<(), String> {
        validate_job_policy(manifest, context)?;
        let resolved = validate_binding(context)?;
        manifest
            .feature
            .subjektiv
            .validate_execution()
            .map_err(str::to_owned)?;
        let saved = manifest.feature.subjektiv.workspace_settings();
        if resolved.is_some() {
            if !manifest.feature.subjektiv.profile.enabled {
                return Err("subject-attached Worker restored with a Profile that disables subjektiv policy".into());
            }
            if saved.as_ref() != context.memory_settings {
                return Err("Workspace Worker subjektiv settings snapshot mismatch".into());
            }
        } else if saved.is_some() {
            return Err(
                "ordinary Workspace Worker restored with an unauthorized subjektiv attachment"
                    .into(),
            );
        }
        Ok(())
    }
    fn validate_saved_state(&self, worker: &Worker<C, St>) -> Result<(), WorkerError> {
        worker.validate_saved_subjektiv_attribution()
    }
    fn prepare_host(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
    ) -> Result<(), WorkerError> {
        let Some(resolved) = connection(data).map_err(WorkerError::FeatureLifecycle)? else {
            return Ok(());
        };
        let client = worker.workspace_client_handle();
        if client.workspace_id() != Some(resolved.workspace_id.as_str()) || !client.is_available() {
            return Err(WorkerError::SubjektivSessionAttribution {
                state: "binding_mismatch",
                message: "Subjektiv initialization has no matching current Host".into(),
            });
        }
        verify_backend_connection(client.as_ref(), &resolved)?;
        if let Some(metadata) = worker
            .store()
            .read_by_name(&worker.manifest().worker.name)?
            && let Some(SubjektivSessionAttributionState::Confirmed { subject_id, .. }) =
                metadata.subjektiv_session_attribution
            && resolved.kind == WorkerConnectionKind::SubjectBody
            && resolved.subject_id.as_deref() != Some(subject_id.as_str())
        {
            return Err(binding_mismatch(&resolved));
        }
        worker
            .subjektiv_host_connection()
            .map_err(|e| WorkerError::FeatureLifecycle(e.to_string()))?
            .ok_or_else(|| {
                WorkerError::FeatureLifecycle(
                    "Subjektiv initialization requires its Host capability".into(),
                )
            })?;
        Ok(())
    }
    fn prepare_for_effects(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
        phase: InitializationPhase,
    ) -> Result<(), WorkerError> {
        if phase == InitializationPhase::New
            && connection(data)
                .map_err(WorkerError::FeatureLifecycle)?
                .is_some_and(|connection| {
                    connection.kind == WorkerConnectionKind::MemoryConsolidation
                })
        {
            // New Job allocation is not yet an accepted Backend attempt. Only
            // construct the local capability here; do not ask the active-attempt
            // endpoint to turn a create reservation into execution authority.
            // The Backend binds the accepted identity before supplying input;
            // the Job input hook and retained-input guard then reauthorize every
            // protected model dispatch. Restore takes the current-Host path.
            let resolved = connection(data)
                .map_err(WorkerError::FeatureLifecycle)?
                .expect("checked consolidation connection");
            let client = worker.workspace_client_handle();
            if client.workspace_id() != Some(resolved.workspace_id.as_str())
                || !client.is_available()
            {
                return Err(binding_mismatch(&resolved));
            }
            worker
                .subjektiv_host_connection()
                .map_err(|error| WorkerError::FeatureLifecycle(error.to_string()))?
                .ok_or_else(|| {
                    WorkerError::FeatureLifecycle(
                        "Subjektiv initialization requires its Host capability".into(),
                    )
                })?;
            return Ok(());
        }
        if phase == InitializationPhase::Restored {
            if let Some(resolved) = connection(data).map_err(WorkerError::FeatureLifecycle)?
                && resolved.kind == WorkerConnectionKind::SubjectBody
            {
                // A confirmed body reconstructs saved state, not current Host
                // authority. Recheck local identity at effects; a changed or
                // unconfirmed attribution must not become implicit repair.
                // Recovery and Jobs retain live verification, and protected
                // reads and supplies authorize against the Host per call.
                worker.validate_saved_subjektiv_attribution()?;
                let metadata = worker
                    .store()
                    .read_by_name(&worker.manifest().worker.name)?;
                return match metadata.and_then(|metadata| metadata.subjektiv_session_attribution) {
                    Some(SubjektivSessionAttributionState::Confirmed { subject_id, .. })
                        if resolved.subject_id.as_deref() == Some(subject_id.as_str()) =>
                    {
                        Ok(())
                    }
                    _ => Err(binding_mismatch(&resolved)),
                };
            }
            return self.prepare_host(worker, data);
        }
        // Observe the same Worker/Session before the Host boundary. Never adopt a
        // replacement owner or a newly read attribution after a failed call.
        let observed = worker
            .store()
            .read_by_name(&worker.manifest().worker.name)?;
        let expected = observed
            .as_ref()
            .filter(|metadata| {
                metadata.worker_name == worker.manifest().worker.name
                    && metadata.workspace_id.as_deref()
                        == worker.workspace_id().map(|id| id.as_str())
                    && metadata
                        .active
                        .as_ref()
                        .is_some_and(|active| active.session_id == worker.session_id())
            })
            .and_then(session_store::worker_metadata::StartupFailureExpectation::from_metadata);
        self.prepare_host(worker, data)
            .map_err(|source| match expected {
                Some(expected) => crate::startup_failure::persist_binding_startup_failure(
                    worker.store(),
                    &expected,
                    source,
                ),
                None => source,
            })
    }
    fn apply(
        &self,
        worker: &Worker<C, St>,
        data: &FeatureInitializations,
        phase: InitializationPhase,
    ) -> Result<(), WorkerError> {
        let resolved = connection(data).map_err(WorkerError::FeatureLifecycle)?;
        worker.finalize_subjektiv_connection(resolved.as_ref(), phase)
    }
    fn requires_source_repair(
        &self,
        metadata: &WorkerMetadata,
        data: &FeatureInitializations,
    ) -> Result<bool, String> {
        Ok(
            connection(data)?.is_some_and(|c| c.kind == WorkerConnectionKind::SubjectBody)
                && !matches!(
                    metadata.subjektiv_session_attribution,
                    Some(SubjektivSessionAttributionState::Confirmed { .. })
                ),
        )
    }
}

pub(crate) fn host_connection<C, St>(
    worker: &Worker<C, St>,
) -> std::io::Result<Option<crate::subjektiv::SubjektivHostConnection>>
where
    C: LlmClient + 'static,
    St: Store,
{
    use crate::subjektiv::{BackendSubjektivHost, SubjektivHostConnection, SubjektivHostContext};
    let config = &worker.manifest().feature.subjektiv;
    config.validate_execution().map_err(std::io::Error::other)?;
    if !config.profile.enabled {
        return Ok(None);
    }
    let host = if let Some(host) = &worker.explicit_subjektiv_host() {
        if config.workspace_settings.is_some()
            || worker.workspace_id().is_some()
            || worker.workspace_client().workspace_id().is_some()
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "explicit local subjektiv Host cannot replace trusted Backend Workspace authority",
            ));
        }
        Some(host.clone())
    } else {
        BackendSubjektivHost::from_resolved_config(config, worker.workspace_client_handle())?
    };
    host.map(|host| {
        SubjektivHostConnection::new(
            host,
            SubjektivHostContext {
                worker_id: worker.manifest().worker.name.clone(),
                session_id: worker.session_id().to_string(),
            },
        )
    })
    .transpose()
}

fn binding_mismatch(expected: &ResolvedWorkerConnection) -> WorkerError {
    if expected.kind == WorkerConnectionKind::SubjectBody {
        WorkerError::SessionRegistration {
            source: crate::subjektiv::SubjektivHostError::SessionRegistration {
                cause: protocol::session_failure::SessionRegistrationCause::binding_mismatch(),
                source: None,
            },
        }
    } else {
        WorkerError::FeatureLifecycle(
            "saved consolidation connection does not match the current Host Job".into(),
        )
    }
}

// This owner-only read does not register a Session or authorize session.record.
// The latter keeps its existing typed refusal and durable failure receipt path.
fn verify_backend_connection(
    client: &dyn crate::WorkspaceClient,
    expected: &ResolvedWorkerConnection,
) -> Result<(), WorkerError> {
    use crate::subjektiv::SubjektivHostError;
    use crate::{WorkspaceClientError, WorkspaceRequest};
    use protocol::session_failure::SessionRegistrationCause;
    let transport_error = |error: WorkspaceClientError| {
        if expected.kind == WorkerConnectionKind::SubjectBody {
            let cause = match &error {
                WorkspaceClientError::Unavailable(_) => SessionRegistrationCause::transport(),
                WorkspaceClientError::InvalidPath(_) => {
                    SessionRegistrationCause::invalid_response()
                }
                _ => SessionRegistrationCause::outcome_unknown(),
            };
            WorkerError::SessionRegistration {
                source: SubjektivHostError::session_registration(cause, error),
            }
        } else {
            WorkerError::FeatureLifecycle(
                "consolidation Host verification refused or unavailable".into(),
            )
        }
    };
    let request = WorkspaceRequest::get(format!(
        "/api/w/{}/subjektiv/worker-connection",
        crate::worker::encode_workspace_path_segment(&expected.workspace_id)
    ));
    let execute = || client.execute_with_timeout(request, std::time::Duration::from_secs(10));
    let response = if tokio::runtime::Handle::try_current().is_ok() {
        std::thread::scope(|scope| scope.spawn(execute).join()).map_err(|_| {
            transport_error(WorkspaceClientError::Request(
                "Subjektiv initialization verification thread panicked".into(),
            ))
        })?
    } else {
        execute()
    }
    .map_err(transport_error)?;
    if !response.is_success() || response.body.len() > 16 * 1024 {
        return Err(transport_error(WorkspaceClientError::Request(
            "Subjektiv initialization verification refused or invalid".into(),
        )));
    }
    let actual: ResolvedWorkerConnection = serde_json::from_str(&response.body).map_err(|_| {
        transport_error(WorkspaceClientError::Request(
            "invalid Subjektiv initialization verification response".into(),
        ))
    })?;
    actual.validate().map_err(|_| binding_mismatch(expected))?;
    if actual != *expected {
        return Err(binding_mismatch(expected));
    }
    Ok(())
}

pub(crate) fn bind_explicit_host<C, St>(
    worker: &mut Worker<C, St>,
    host: Option<std::sync::Arc<dyn crate::subjektiv::SubjektivHost>>,
    phase: InitializationPhase,
) -> Result<(), WorkerError>
where
    C: LlmClient + 'static,
    St: Store + WorkerMetadataStore + Clone + 'static,
{
    let explicit = host.is_some();
    worker.bind_subjektiv_host(host);
    if explicit
        && worker.manifest().feature.subjektiv.profile.enabled
        && !worker
            .manifest()
            .feature
            .subjektiv
            .profile
            .consolidation_tools
    {
        let lifecycle = match phase {
            InitializationPhase::New => SubjektivSessionAttributionLifecycle::NewSession,
            InitializationPhase::Restored | InitializationPhase::Recovery => {
                SubjektivSessionAttributionLifecycle::RestoredSession
            }
        };
        worker.finalize_subjektiv_session_attribution(true, lifecycle)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::WorkerManifest;
    #[test]
    fn backend_job_profile_policy_matches_trusted_grant_without_implicit_changes() {
        let mut manifest = WorkerManifest::from_toml(
            r#"
            [worker]
            name = "job-policy-test"
            pwd = "./"
            [model]
            scheme = "anthropic"
            model_id = "chosen-model"
            auth = { kind = "none" }
            [engine]
            instruction = "default"
            [[scope.allow]]
            target = "./"
            permission = "read"
        "#,
        )
        .unwrap();
        let data = FeatureInitializations::default();
        for (granted, journal) in [(false, false), (true, false), (false, true)] {
            let binding = crate::BackendJobExecutionBinding {
                job_id: "job-1".into(),
                attempt_id: "attempt-1".into(),
                input_digest: None,
                subjektiv_consolidation: granted,
                subjektiv_journal: journal,
            };
            let context = InitializationContext {
                data: &data,
                workspace_id: None,
                memory_settings: None,
                backend_job: Some(&binding),
            };
            for tools in [false, true] {
                for extraction in [false, true] {
                    manifest.feature.subjektiv.profile.consolidation_tools = tools;
                    manifest.feature.subjektiv.profile.extraction.enabled = extraction;
                    let expected = tools == granted && (!(granted || journal) || !extraction);
                    let before = serde_json::to_value(&manifest).unwrap();
                    assert_eq!(validate_job_policy(&manifest, &context).is_ok(), expected);
                    assert_eq!(serde_json::to_value(&manifest).unwrap(), before);
                }
            }
        }
    }
    #[derive(Debug)]
    struct CurrentConnection {
        response: crate::WorkspaceResponse,
        calls: std::sync::atomic::AtomicUsize,
    }
    impl crate::WorkspaceClient for CurrentConnection {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace-a")
        }
        fn kind(&self) -> &str {
            "current-connection-test"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(
            &self,
            request: crate::WorkspaceRequest,
        ) -> Result<crate::WorkspaceResponse, crate::WorkspaceClientError> {
            assert_eq!(
                request.path,
                "/api/w/workspace-a/subjektiv/worker-connection"
            );
            assert_eq!(request.method, crate::WorkspaceRequestMethod::Get);
            assert!(
                request.body.is_none(),
                "the caller must not choose its Host identity"
            );
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.response.clone())
        }
    }

    fn resolved(subject: Option<&str>, kind: WorkerConnectionKind) -> ResolvedWorkerConnection {
        ResolvedWorkerConnection {
            workspace_id: "workspace-a".into(),
            subject_id: subject.map(str::to_owned),
            kind,
        }
    }

    #[test]
    fn fresh_host_must_match_the_saved_workspace_subject_and_connection_kind() {
        let body = resolved(Some("subject-a"), WorkerConnectionKind::SubjectBody);
        let job = resolved(Some("subject-a"), WorkerConnectionKind::MemoryConsolidation);
        for expected in [&body, &job] {
            let mut wrong_workspace = expected.clone();
            wrong_workspace.workspace_id = "workspace-b".into();
            let mut wrong_subject = expected.clone();
            wrong_subject.subject_id = Some("subject-b".into());
            let mut wrong_kind = expected.clone();
            wrong_kind.kind = if expected.kind == WorkerConnectionKind::SubjectBody {
                WorkerConnectionKind::MemoryConsolidation
            } else {
                WorkerConnectionKind::SubjectBody
            };
            let mut missing_subject = expected.clone();
            missing_subject.subject_id = None;
            for actual in [
                expected.clone(),
                wrong_workspace,
                wrong_subject,
                wrong_kind,
                missing_subject,
            ] {
                let host = CurrentConnection {
                    response: crate::WorkspaceResponse {
                        status: 200,
                        content_type: None,
                        body: serde_json::to_string(&actual).unwrap(),
                    },
                    calls: Default::default(),
                };
                assert_eq!(
                    verify_backend_connection(&host, expected).is_ok(),
                    actual == *expected
                );
                assert_eq!(host.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            }
        }
    }

    #[test]
    fn revoked_or_malformed_host_response_cannot_fall_back_to_an_unattached_worker() {
        let expected = resolved(Some("subject-a"), WorkerConnectionKind::SubjectBody);
        for (status, body) in [
            (403, "{}".into()),
            (200, "{}".into()),
            (200, "not json".into()),
            (200, "x".repeat(16 * 1024 + 1)),
        ] {
            let host = CurrentConnection {
                response: crate::WorkspaceResponse {
                    status,
                    content_type: None,
                    body,
                },
                calls: Default::default(),
            };
            assert!(verify_backend_connection(&host, &expected).is_err());
        }
    }

    #[test]
    fn ordinary_job_has_no_subjektiv_initialization_or_subject_fallback() {
        let binding = crate::BackendJobExecutionBinding {
            job_id: "ordinary-job".into(),
            attempt_id: "attempt".into(),
            input_digest: Some("input".into()),
            subjektiv_consolidation: false,
            subjektiv_journal: false,
        };
        let empty = FeatureInitializations::default();
        let mut context = InitializationContext {
            data: &empty,
            workspace_id: Some("workspace-a"),
            memory_settings: None,
            backend_job: Some(&binding),
        };
        assert_eq!(validate_binding(&context).unwrap(), None);
        let granted = crate::BackendJobExecutionBinding {
            subjektiv_consolidation: true,
            ..binding.clone()
        };
        context.backend_job = Some(&granted);
        assert!(validate_binding(&context).is_err());
    }

    #[test]
    fn memory_settings_without_a_subject_connection_are_rejected() {
        let data = FeatureInitializations::default();
        let settings = manifest::SubjektivMemorySettingsSnapshot {
            workspace_id: "workspace-a".into(),
            language: "English".into(),
        };
        let context = InitializationContext {
            data: &data,
            workspace_id: Some("workspace-a"),
            memory_settings: Some(&settings),
            backend_job: None,
        };
        assert!(
            validate_binding(&context)
                .unwrap_err()
                .contains("explicit Subjektiv connection")
        );
    }

    #[test]
    fn consolidation_payload_requires_the_independent_job_binding_and_never_body_identity() {
        use protocol::feature_initialization::FeatureInitialization;
        let binding = crate::BackendJobExecutionBinding {
            job_id: "job".into(),
            attempt_id: "attempt".into(),
            input_digest: Some("input".into()),
            subjektiv_consolidation: true,
            subjektiv_journal: false,
        };
        let settings = manifest::SubjektivMemorySettingsSnapshot {
            workspace_id: "workspace-a".into(),
            language: "English".into(),
        };
        for connection in [
            resolved(None, WorkerConnectionKind::MemoryConsolidation),
            resolved(Some("subject-a"), WorkerConnectionKind::MemoryConsolidation),
        ] {
            let data = FeatureInitializations::new(vec![
                FeatureInitialization::encode(FEATURE_ID, VERSION, &connection).unwrap(),
            ])
            .unwrap();
            let mut context = InitializationContext {
                data: &data,
                workspace_id: Some("workspace-a"),
                memory_settings: Some(&settings),
                backend_job: Some(&binding),
            };
            assert_eq!(
                validate_binding(&context).is_ok(),
                connection.subject_id.is_some()
            );
            context.backend_job = None;
            assert!(validate_binding(&context).is_err());
            context.backend_job = Some(&binding);
            context.workspace_id = Some("workspace-b");
            assert!(validate_binding(&context).is_err());
        }
        let data = FeatureInitializations::new(vec![
            FeatureInitialization::encode(
                FEATURE_ID,
                VERSION,
                &resolved(Some("subject-a"), WorkerConnectionKind::SubjectBody),
            )
            .unwrap(),
        ])
        .unwrap();
        assert!(
            validate_binding(&InitializationContext {
                data: &data,
                workspace_id: Some("workspace-a"),
                memory_settings: Some(&settings),
                backend_job: Some(&binding)
            })
            .is_err()
        );
    }
}
