//! Alternate staging handler -> native SQLite admission. Real canonical adoption,
//! body ownership, Session attribution and registry are used; Runtime execution is
//! the shared fixture. An observer pauses only after the handler's detached checks.
use super::*;
use std::time::Duration;

#[derive(Clone, Copy)]
enum Change {
    None,
    Policy,
    Feature,
    Body,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alternate_staging_unchanged_authority_persists_attributed_candidate() {
    staging_schedule(Change::None).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alternate_staging_policy_revoked_after_precheck_leaves_no_candidate() {
    staging_schedule(Change::Policy).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alternate_staging_feature_disabled_after_precheck_leaves_no_candidate() {
    staging_schedule(Change::Feature).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn alternate_staging_body_released_after_precheck_leaves_no_candidate() {
    staging_schedule(Change::Body).await;
}

async fn staging_schedule(change: Change) {
    let f = Fixture::new().await;
    f.adopt_policy(r#"{ sets = ["subject-default", "memory"]; }"#, "{}")
        .await;
    let store = open_subjektiv_store(&f.api).unwrap();
    let attribution = store
        .subject_session_attributions(&f.subject)
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let binding = f
        .api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &f.source.runtime_id)
        .await
        .unwrap()
        .unwrap();
    let context = server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: Some(server_api::ServerRuntimeSource {
            runtime_id: f.source.runtime_id.clone(),
            worker_id: Some(f.source.worker_id.clone()),
            authentication_binding_id: binding.binding_id,
        }),
        origin: None,
        transport_headers: vec![],
    };
    let request: server_api::SubjektivStageCandidateRequest = serde_json::from_value(json!({
        "session_id": attribution.session_id,
        "operation": {
            "source": { "segment_id": "staging-segment", "range": [0,0] },
            "extract_run_id": "staging-run",
            "candidate": { "kind": "decision", "claim": "retain native staging authority", "why_useful": "avoid stale writes", "staleness": null, "evidence_ids": ["E00000000"] },
            "evidence": [{ "id": "E00000000", "kind": "message", "entry_range": [0,0], "origin": { "kind": "human_input" }, "excerpt": "explicit decision", "summary": "decision" }],
            "source_refs": [{ "segment_id": "staging-segment", "entry_range": [0,0], "evidence_id": "E00000000", "evidence_kind": "message" }]
        }
    })).unwrap();
    let (ready, at_native) = tokio::sync::oneshot::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    let api = f.api.clone();
    let operation = tokio::spawn(async move {
        stage_subjektiv_candidate_before_native(
            api,
            ScopedWorkspacePath {
                workspace_id: TEST_WORKSPACE_ID.into(),
            },
            context,
            request,
            move || {
                ready.send(()).unwrap();
                proceed
                    .recv_timeout(Duration::from_secs(20))
                    .expect("native staging gate released");
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(20), at_native)
        .await
        .unwrap()
        .unwrap();
    match change {
        Change::None => {}
        Change::Policy => {
            f.adopt_policy(r#"{ sets = ["subject-default"]; exclude_namespaces = [{ namespace = "memory"; keys = ["memory.staging.write"]; }]; }"#, "{}")
                .await
        }
        Change::Feature => f
            .api
            .permission_registry
            .write()
            .unwrap()
            .set_enabled(&worker::feature::FeatureId::builtin("memory"), false)
            .unwrap(),
        Change::Body => f.release_source_lease(),
    }
    release.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(20), operation)
        .await
        .unwrap()
        .unwrap();
    if matches!(change, Change::None) {
        let Json(staged) = result.unwrap();
        let saved = open_subjektiv_store(&f.api)
            .unwrap()
            .staging_candidate(&f.subject, &staged.staging_id)
            .unwrap()
            .unwrap();
        assert_eq!(saved.subject_id, f.subject);
        assert_eq!(
            saved.source_refs[0].session_id.as_deref(),
            Some(attribution.session_id.as_str())
        );
    } else {
        let error = result.unwrap_err();
        assert!(
            matches!(error.error, Error::WorkspacePermissionDenied(_)),
            "native admission must reject current authority: {error:?}"
        );
    }
    let count: i64 = f
        .api
        .feature_storage
        .open(&f.api.subjektiv_registration)
        .unwrap()
        .with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT count(*) FROM staging_records WHERE subject_id=?1",
                [&f.subject],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(count, if matches!(change, Change::None) { 1 } else { 0 });
}
