// Local effects tests share the real metadata/Session fixture in worker::tests.
// Runtime entry and zero-HTTP coverage lives in worker-runtime's connected tests.

#[tokio::test]
async fn restored_attribution_effects_never_repair_incomplete_saved_state() {
    use crate::feature::initialization::InitializationPhase;
    use subjektiv_api::worker_initialization::{ResolvedWorkerConnection, WorkerConnectionKind};
    for state in ["missing", "legacy_unknown", "outcome_unknown", "failed"] {
        let client = Arc::new(AttributionLifecycleClient::new([]));
        let (worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
        let session_id = worker.session_id();
        let diagnostic = "saved incomplete attribution".to_string();
        let attribution = match state {
            "missing" => None,
            "legacy_unknown" => Some(SubjektivSessionAttributionState::LegacyUnknown {
                session_id,
                diagnostic,
            }),
            "outcome_unknown" => Some(SubjektivSessionAttributionState::OutcomeUnknown {
                session_id,
                diagnostic,
            }),
            "failed" => Some(SubjektivSessionAttributionState::Failed {
                session_id,
                diagnostic,
            }),
            _ => unreachable!(),
        };
        store
            .update_by_name(&worker.manifest().worker.name, |metadata| {
                metadata.subjektiv_session_attribution = attribution;
            })
            .unwrap();
        let before = store
            .read_by_name(&worker.manifest().worker.name)
            .unwrap()
            .unwrap();
        let connection = ResolvedWorkerConnection {
            workspace_id: "workspace-test".into(),
            subject_id: Some("subject-test".into()),
            kind: WorkerConnectionKind::SubjectBody,
        };
        // Exercise the effects guard independently of candidate preparation:
        // attribution could have changed since that earlier local check.
        assert!(
            worker
                .finalize_subjektiv_connection(Some(&connection), InitializationPhase::Restored,)
                .is_err(),
            "{state}"
        );
        assert!(
            worker
                .finalize_restored_subjektiv_session_attribution(true)
                .is_err(),
            "{state}"
        );
        assert!(client.requests.lock().unwrap().is_empty(), "{state}");
        assert_eq!(
            store
                .read_by_name(&worker.manifest().worker.name)
                .unwrap()
                .unwrap(),
            before
        );
    }
}

#[tokio::test]
async fn restored_attribution_reinstates_local_journal_pin_without_host_requests() {
    use crate::feature::initialization::InitializationPhase;
    use subjektiv_api::worker_initialization::{ResolvedWorkerConnection, WorkerConnectionKind};
    let client = Arc::new(AttributionLifecycleClient::new([]));
    let (mut worker, store, _temp) = attribution_lifecycle_worker(client.clone()).await;
    worker.manifest.feature.subjektiv.profile.enabled = true;
    worker
        .manifest
        .feature
        .subjektiv
        .bind_workspace_settings(manifest::SubjektivMemorySettingsSnapshot {
            workspace_id: "workspace-test".into(),
            language: "English".into(),
        })
        .unwrap();
    worker
        .finalize_subjektiv_session_attribution(
            true,
            SubjektivSessionAttributionLifecycle::NewSession,
        )
        .unwrap();
    let session = worker.session_id();
    let pin =
        session_store::retention::SessionRetentionPin(subjektiv::journal::source_retention_pin(
            "workspace-test",
            "subject-test",
            &session.to_string(),
        ));
    store
        .session_store
        .release_session_pin(session, pin)
        .unwrap();
    assert!(
        store
            .session_store
            .retention()
            .pins(session)
            .unwrap()
            .is_empty()
    );
    let connection = ResolvedWorkerConnection {
        workspace_id: "workspace-test".into(),
        subject_id: Some("subject-test".into()),
        kind: WorkerConnectionKind::SubjectBody,
    };
    for _ in 0..2 {
        worker
            .finalize_subjektiv_connection(Some(&connection), InitializationPhase::Restored)
            .unwrap();
        assert_eq!(
            store.session_store.retention().pins(session).unwrap(),
            vec![pin]
        );
    }
    assert_eq!(
        client.requests.lock().unwrap().len(),
        1,
        "only initial registration may fetch"
    );
}
