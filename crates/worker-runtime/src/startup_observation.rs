//! The Runtime creation responsibility sink records only projected typed fields.
//! Source/provider strings are never safe merely because a blacklist did not match.
//! Wrappers/projectors do not write events; a failing sink cannot change the result.
use crate::{error::RuntimeError, identity::WorkerId};

pub(crate) fn write_create_failure(
    worker_id: WorkerId,
    workspace_id: Option<&str>,
    error: &RuntimeError,
    elapsed: std::time::Duration,
) {
    let failure = error.create_failure();
    let projected = failure
        .as_ref()
        .and_then(|failure| failure.telemetry().ok());
    let fields = projected.as_ref().map(error_contract::observation_fields);
    let operation = failure.as_ref().map(|failure| failure.operation());
    tracing::error!(
        target: "yoi::worker_create",
        event = "worker_create_failed",
        component = "runtime",
        workspace_id = workspace_id.unwrap_or(""),
        worker_id = %worker_id,
        domain = fields.as_ref().map_or("startup_boundary", |fields| fields.domain),
        reason = fields.as_ref().map_or("observation_unavailable", |fields| fields.reason),
        operation = fields.as_ref().and_then(|fields| fields.operation),
        stage = fields.as_ref().and_then(|fields| fields.stage),
        correlation = operation.and_then(|operation| operation.correlation.as_ref()).map(|value| value.as_str()),
        acceptance = ?operation.map(|operation| operation.acceptance),
        execution = ?operation.map(|operation| operation.execution),
        finalize = ?operation.and_then(|operation| operation.finalize),
        persistence = ?failure.as_ref().and_then(|failure| failure.startup().persistence),
        resources = ?failure.as_ref().and_then(|failure| failure.startup().resources),
        duration_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        "Worker creation returned a typed failure"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CreateWorkerRequest, ProfileSelector, ProfileSourceArchiveSource};
    use crate::execution::{
        WorkerExecutionBackend, WorkerExecutionOperation, WorkerExecutionResult,
        WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
    };
    use crate::profile_archive::{ProfileSourceArchive, ProfileSourceArchiveInput};
    use crate::{Runtime, RuntimeOptions};
    use runtime_api::startup::*;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::prelude::*;

    const CANARIES: &[&str] = &[
        "aQ7xV2zN9mL5",
        "/private/startup/provider",
        "Bearer q3fV8aJ6",
        "token=K7sM2pQ4",
    ];
    struct FailingBackend;
    impl WorkerExecutionBackend for FailingBackend {
        fn backend_id(&self) -> &str {
            "log-owner-fixture"
        }
        fn dispatch_input(
            &self,
            _: &crate::identity::WorkerRef,
            _: crate::interaction::WorkerInput,
        ) -> WorkerExecutionResult {
            panic!("rejected creation never dispatches input")
        }
        fn spawn_worker(&self, _: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
            let result =
                WorkerExecutionResult::errored(WorkerExecutionOperation::Spawn, CANARIES.join(" "))
                    .with_startup_failure(
                        worker::startup_failure::runtime_startup_attachment(
                            runtime_api::RuntimeStartupCause {
                                stage: runtime_api::StartupStage::Controller,
                                reason: runtime_api::RuntimeStartupReason::StartupFailed,
                            },
                        )
                        .unwrap(),
                    );
            WorkerExecutionSpawnResult::Errored(result)
        }
    }
    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
    struct Fields(BTreeMap<String, String>);
    impl tracing::field::Visit for Fields {
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            self.0.insert(field.name().into(), value.into());
        }
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().into(), format!("{value:?}"));
        }
    }
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Events {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() == "yoi::worker_create" {
                let mut fields = Fields(BTreeMap::new());
                event.record(&mut fields);
                self.0.lock().unwrap().push(fields.0);
            }
        }
    }
    fn request() -> CreateWorkerRequest {
        let archive = ProfileSourceArchive::build(ProfileSourceArchiveInput {
            id: "log-profile".into(),
            entrypoints: BTreeMap::from([("builtin:coder".into(), "profiles/coder.dcdl".into())]),
            imports: BTreeMap::new(),
            sources: BTreeMap::from([("profiles/coder.dcdl".into(), "{}".into())]),
        })
        .unwrap();
        CreateWorkerRequest {
            worker_id: WorkerId::now_v7(),
            create_fingerprint: "log-create".into(),
            profile: ProfileSelector::Builtin("builtin:coder".into()),
            display_name: None,
            profile_source: ProfileSourceArchiveSource::Embedded { archive },
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
        }
    }
    #[test]
    fn real_runtime_creation_sink_omits_unlabelled_provider_canaries_and_matches_typed_result() {
        let runtime =
            Runtime::with_execution_backend(RuntimeOptions::default(), Arc::new(FailingBackend))
                .unwrap();
        runtime
            .bind_runtime_identity("runtime-log-fixture")
            .unwrap();
        let request = request();
        let worker_id = request.worker_id;
        let events = Events::default();
        let subscriber = tracing_subscriber::registry().with(events.clone());
        let error =
            tracing::subscriber::with_default(subscriber, || runtime.create_worker(request))
                .unwrap_err();
        let failure = error
            .create_failure()
            .expect("creation owner must retain a typed cause");
        let wire = failure
            .problem(
                StartupBoundary::Runtime,
                &SessionFailureVisibility {
                    can_view_cause: true,
                },
                "",
            )
            .unwrap();
        let encoded = failure.encode_storage().unwrap();
        let DecodeStoredWorkerCreateFailure::Known(saved) =
            decode_stored_worker_create_failure(&encoded)
        else {
            panic!("known storage");
        };
        assert_eq!(saved.public_facts(), failure.public_facts());
        let captured = events.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let event = &captured[0];
        let projection = failure.telemetry().unwrap();
        let fields = error_contract::observation_fields(&projection);
        assert_eq!(event["domain"], fields.domain);
        assert_eq!(event["reason"], fields.reason);
        assert_eq!(
            event["correlation"],
            StartupCorrelation::for_worker("runtime-log-fixture", &worker_id.to_string()).as_str()
        );
        for canary in CANARIES {
            assert!(!format!("{captured:?}").contains(canary));
            assert!(!String::from_utf8_lossy(wire.body()).contains(canary));
            assert!(!encoded.contains(canary));
        }
        assert!(!event.contains_key("execution_failure_message"));
    }
    #[test]
    fn unsupported_failure_logs_only_fixed_fallback_not_source_text() {
        let error = RuntimeError::InvalidRequest(CANARIES.join(" "));
        let events = Events::default();
        let subscriber = tracing_subscriber::registry().with(events.clone());
        tracing::subscriber::with_default(subscriber, || {
            write_create_failure(WorkerId::now_v7(), None, &error, std::time::Duration::ZERO)
        });
        let captured = events.0.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0]["reason"], "observation_unavailable");
        for canary in CANARIES {
            assert!(!format!("{captured:?}").contains(canary));
        }
    }
}
