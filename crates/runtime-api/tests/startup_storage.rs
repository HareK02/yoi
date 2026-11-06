//! Storage codecs use the production attachment projector, never an HTTP body.
use error_contract::{Budget, ContractError, DecodeFailure, validate_json_input};
use protocol::session_failure::{SessionPrincipal, SessionRegistrationCause};
use runtime_api::startup::*;
use runtime_api::{
    RuntimeStartupCause, RuntimeStartupReason, StartupCleanup, StartupCleanupFailure,
    StartupFailureAttachment, StartupIoKind, StartupPersistenceOutcome, StartupResourceFacts,
    StartupResources, StartupSecondaryFailure, StartupStage, WorkerStartupFailure,
};
use serde_json::{Value, json};

fn operation() -> StartupOperationFacts {
    StartupOperationFacts {
        acceptance: CreationAcceptance::Accepted,
        execution: ExecutionOutcome::Errored,
        finalize: Some(FinalizeFacts {
            stage: FinalizeStage::Publication,
            compensation: CompensationOutcome::Residual,
        }),
        backend_compensation: Some(CompensationOutcome::Unknown),
        correlation: Some(StartupCorrelation::new("storage-test-812").unwrap()),
    }
}

fn failure(cause: StartupCause) -> WorkerCreateFailure {
    let base = WorkerCreateFailure::from_cause(cause, operation()).unwrap();
    let (mut startup, operation) = base.into_parts();
    startup.secondary.push(StartupSecondaryFailure::Cleanup {
        failure: StartupCleanupFailure::Workdir,
    });
    startup.resources = Some(StartupResourceFacts {
        resources: StartupResources::WorkerStartup,
        cleanup: StartupCleanup::Failed(StartupCleanupFailure::Workdir),
        durable_effects_possible: true,
    });
    // Session metadata write and Backend result write deliberately disagree.
    startup.persistence = Some(StartupPersistenceOutcome::NotSaved);
    startup.io_kind = Some(StartupIoKind::PermissionDenied);
    WorkerCreateFailure::new(startup, operation)
        .unwrap()
        .with_backend_result_persistence(Some(StartupPersistenceOutcome::Persisted))
}

fn session() -> StartupCause {
    StartupCause::Session(SessionRegistrationCause::denied(SessionPrincipal::Subject))
}

fn known(input: &str) -> WorkerCreateFailure {
    match decode_stored_worker_create_failure(input) {
        DecodeStoredWorkerCreateFailure::Known(failure) => failure,
        other => panic!("expected known storage snapshot, got {other:?}"),
    }
}

fn change_primary(value: &mut Value, change: impl FnOnce(&mut Value)) {
    let mut primary: Value = serde_json::from_str(value["primary"].as_str().unwrap()).unwrap();
    change(&mut primary);
    value["primary"] = Value::String(serde_json::to_string(&primary).unwrap());
}

#[test]
fn storage_semantic_roundtrip_covers_domains_and_independent_facts() {
    for cause in [
        session(),
        StartupCause::Runtime(RuntimeStartupCause {
            stage: StartupStage::Feature,
            reason: RuntimeStartupReason::FeatureFailed,
        }),
        StartupCause::Boundary(StartupBoundaryCause {
            reason: StartupBoundaryReason::FinalizeFailed,
        }),
    ] {
        let original = failure(cause.clone());
        let encoded = original.encode_storage().unwrap();
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["version"], 1);
        assert!(value.get("status").is_none());
        assert!(value.get("yoi_failure").is_none());
        let loaded = known(&encoded);
        assert_eq!(loaded.public_facts(), original.public_facts());
        assert_eq!(loaded.operation(), original.operation());
        assert_eq!(
            loaded.startup().persistence,
            Some(StartupPersistenceOutcome::NotSaved)
        );
        assert_eq!(
            loaded.backend_result_persistence(),
            Some(StartupPersistenceOutcome::Persisted)
        );
        match loaded.startup().primary.decode::<StartupCause>() {
            DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), &cause),
            _ => panic!("primary cause lost"),
        }
        assert_eq!(loaded.encode_storage().unwrap(), encoded);
    }
}

#[test]
fn encoding_uses_current_marker_without_replacing_primary_or_inferring_persistence() {
    let original = failure(session());
    for marker in [
        None,
        Some(StartupPersistenceOutcome::Persisted),
        Some(StartupPersistenceOutcome::NotSaved),
        Some(StartupPersistenceOutcome::Conflict),
        Some(StartupPersistenceOutcome::OutcomeUnknown),
    ] {
        let updated = original.clone().with_backend_result_persistence(marker);
        assert_eq!(updated.startup(), original.startup());
        assert_eq!(updated.operation(), original.operation());
        let loaded = known(&updated.encode_storage().unwrap());
        assert_eq!(loaded.backend_result_persistence(), marker);
        assert_eq!(loaded.startup().persistence, original.startup().persistence);
        assert_eq!(loaded.public_facts(), updated.public_facts());
    }
}

#[test]
fn legacy_absent_marker_is_none_in_storage_and_public_reader() {
    let original = failure(session()).with_backend_result_persistence(None);
    let mut value: Value = serde_json::from_str(&original.encode_storage().unwrap()).unwrap();
    value["facts"]
        .as_object_mut()
        .unwrap()
        .remove("backend_result_persistence");
    assert_eq!(known(&value.to_string()).backend_result_persistence(), None);
    let response = original
        .problem(
            StartupBoundary::Browser,
            &SessionFailureVisibility {
                can_view_cause: true,
            },
            "",
        )
        .unwrap();
    let public: Value = serde_json::from_slice(response.body()).unwrap();
    assert!(
        public["yoi_startup"]
            .get("backend_result_persistence")
            .is_none()
    );
    match decode_startup_problem(
        response.body(),
        response.status,
        response.content_type,
        StartupBoundary::Browser,
    ) {
        DecodeStartupProblem::Known(failure) => {
            assert_eq!(failure.backend_result_persistence(), None)
        }
        other => panic!("legacy public response failed: {other:?}"),
    }
}

#[test]
fn public_receiver_restores_backend_marker_and_current_visibility_is_required() {
    let original = failure(session());
    let loaded = known(&original.encode_storage().unwrap());
    assert!(
        loaded
            .problem(
                StartupBoundary::Browser,
                &SessionFailureVisibility {
                    can_view_cause: false
                },
                ""
            )
            .is_err()
    );
    let response = loaded
        .problem(
            StartupBoundary::Browser,
            &SessionFailureVisibility {
                can_view_cause: true,
            },
            "",
        )
        .unwrap();
    let received = match decode_startup_problem(
        response.body(),
        response.status,
        response.content_type,
        StartupBoundary::Browser,
    ) {
        DecodeStartupProblem::Known(failure) => failure,
        other => panic!("public response failed: {other:?}"),
    };
    assert_eq!(received.public_facts(), original.public_facts());
    let mut updated = operation();
    updated.acceptance = CreationAcceptance::Unknown;
    let received = received.with_operation(updated.clone());
    assert_eq!(received.operation(), &updated);
    assert_eq!(
        received.backend_result_persistence(),
        Some(StartupPersistenceOutcome::Persisted)
    );
    // The receiving Backend clears remote evidence before its own write.
    assert_eq!(
        received
            .with_backend_result_persistence(None)
            .backend_result_persistence(),
        None
    );
}

#[test]
fn unknown_versions_and_primary_identities_are_unsupported() {
    let base: Value = serde_json::from_str(&failure(session()).encode_storage().unwrap()).unwrap();
    for mutate in [
        |v: &mut Value| v["version"] = json!(2),
        |v: &mut Value| v["facts"]["version"] = json!(2),
        |v: &mut Value| change_primary(v, |p| p["version"] = json!(2)),
        |v: &mut Value| change_primary(v, |p| p["reason"] = json!("future_reason")),
        |v: &mut Value| {
            change_primary(v, |p| {
                p["required_extensions"] = json!(["future_extension"])
            })
        },
    ] {
        let mut value = base.clone();
        mutate(&mut value);
        assert_eq!(
            decode_stored_worker_create_failure(&value.to_string()),
            DecodeStoredWorkerCreateFailure::Unsupported
        );
    }
}

#[test]
fn malformed_and_duplicate_known_records_are_corrupt() {
    let base: Value = serde_json::from_str(&failure(session()).encode_storage().unwrap()).unwrap();
    for input in [
        "".to_owned(),
        "null".to_owned(),
        "{}".to_owned(),
        "{".to_owned(),
        base.to_string()
            .replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
    ] {
        assert!(matches!(
            decode_stored_worker_create_failure(&input),
            DecodeStoredWorkerCreateFailure::Corrupt(_)
        ));
    }
    for mutate in [
        |v: &mut Value| v["version"] = json!(0),
        |v: &mut Value| v["primary"] = json!("{broken"),
        |v: &mut Value| v["facts"]["backend_result_persistence"] = json!("invented"),
        |v: &mut Value| {
            v["facts"]["secondary"] = json!(vec![json!({"kind":"cleanup","failure":"workdir"}); 9])
        },
        |v: &mut Value| change_primary(v, |p| p["category"] = json!("internal")),
    ] {
        let mut value = base.clone();
        mutate(&mut value);
        assert!(matches!(
            decode_stored_worker_create_failure(&value.to_string()),
            DecodeStoredWorkerCreateFailure::Corrupt(_)
        ));
    }
}

#[test]
fn complete_input_and_embedded_attachment_are_bounded_before_parsing() {
    let oversized = " ".repeat(32 * 1024 + 1);
    assert_eq!(
        decode_stored_worker_create_failure(&oversized),
        DecodeStoredWorkerCreateFailure::Corrupt(ContractError::TooLarge)
    );
    let deep = format!("{}0{}", "[".repeat(17), "]".repeat(17));
    assert_eq!(
        decode_stored_worker_create_failure(&deep),
        DecodeStoredWorkerCreateFailure::Corrupt(ContractError::TooDeep)
    );
    // A supported-looking prefix must not bypass the whole-input guard.
    let deep = format!("{{\"version\":2,\"extra\":{deep}}}");
    assert_eq!(
        decode_stored_worker_create_failure(&deep),
        DecodeStoredWorkerCreateFailure::Corrupt(ContractError::TooDeep)
    );
    let mut value: Value =
        serde_json::from_str(&failure(session()).encode_storage().unwrap()).unwrap();
    change_primary(&mut value, |p| {
        p["extra"] =
            serde_json::from_str(&format!("{}0{}", "[".repeat(17), "]".repeat(17))).unwrap()
    });
    assert_eq!(
        decode_stored_worker_create_failure(&value.to_string()),
        DecodeStoredWorkerCreateFailure::Corrupt(ContractError::TooDeep)
    );
    // Shared wrapper is a bounds check, not a grammar validator or encoder.
    assert_eq!(validate_json_input(b"[", Budget::new(1024, 1024)), Ok(()));
}

#[test]
fn storage_encoder_and_reader_strip_untrusted_context_and_explanations() {
    let original = failure(session());
    let mut value: Value = serde_json::from_str(&original.encode_storage().unwrap()).unwrap();
    change_primary(&mut value, |p| {
        p["explanation"] = json!("TEST_ONLY_SECRET_RAW_HTTP");
        p["context"]["correlation"] = json!("private-worker-id");
        p["context"]["operations"] = json!([{
            "operation":"session.record", "stage":"binding", "operation_ref":"TEST_ONLY_SECRET_RAW_HTTP"
        }, {
            "operation":"/private/path", "stage":"http-body", "operation_ref":null
        }]);
    });
    // Simulate a received attachment without granting a projection capability.
    let primary: StartupFailureAttachment =
        serde_json::from_value(value["primary"].clone()).unwrap();
    let (startup, operation) = original.clone().into_parts();
    let incoming =
        WorkerCreateFailure::new(WorkerStartupFailure { primary, ..startup }, operation).unwrap();
    for encoded in [
        incoming.encode_storage().unwrap(),
        known(&value.to_string()).encode_storage().unwrap(),
    ] {
        for secret in [
            "TEST_ONLY_SECRET_RAW_HTTP",
            "private-worker-id",
            "/private/path",
            "http-body",
        ] {
            assert!(
                !encoded.contains(secret),
                "unexpected secret in stored snapshot"
            );
        }
        let loaded = known(&encoded);
        let response = loaded
            .problem(
                StartupBoundary::Browser,
                &SessionFailureVisibility {
                    can_view_cause: true,
                },
                "",
            )
            .unwrap();
        assert!(!String::from_utf8_lossy(response.body()).contains("TEST_ONLY_SECRET"));
    }
}
