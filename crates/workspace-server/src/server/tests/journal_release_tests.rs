//! Real Controller release and saved-scope recording use the removal suite's
//! isolated Server/Runtime fixture; only external model responses are scripted.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_release_recording_transfers_to_later_removal_without_losing_scope() {
    let f = idle_source().await;
    f.seed_retained(vec![RemovalFixture::history(
        "release-before-remove",
        "original retained across release and later removal",
    )]);
    release(&f, "release-before-ordinary-remove").await;
    f.wait_for_input().await;
    let released = f
        .api()
        .config_store
        .get_release_journal_recording(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    assert!(released.released && !released.recording.completed);
    RemovalFixture::stop_body(f.api(), &f.source).await;
    let response = f.remove().await;
    assert_eq!(
        response.status,
        StatusCode::OK.as_u16(),
        "{}",
        response.body
    );
    f.wait_removed().await;
    let removed = f
        .api()
        .store
        .get_journal_removal(&f.prepared())
        .unwrap()
        .unwrap();
    assert_eq!(
        removed.metadata.members.as_ref(),
        Some(&released.recording.members)
    );
    assert!(
        !removed
            .metadata
            .retained_recording
            .as_ref()
            .unwrap()
            .completed
    );
    f.model.set(ModelAction::Record);
    f.wait_recorded().await;
    wait_recorded(&f).await;
    assert!(!f.root().exists());
}

async fn idle_source() -> RemovalFixture {
    let f = RemovalFixture::with_live_source(true).await;
    f.model.source_release.send_replace(true);
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let summary = f.api().runtime.worker(&f.source).unwrap();
            if !f.model.source_requests.lock().unwrap().is_empty()
                && summary
                    .worker_state
                    .as_ref()
                    .is_some_and(|state| state.state == protocol::WorkerState::Idle)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("original Controller finishes its turn before release");
    f
}

async fn release(f: &RemovalFixture, operation: &str) -> runtime_api::SubjectReleaseResponse {
    let api = f.api().clone();
    let input = serde_json::from_value(json!({
        "worker": f.source, "binding_id": "", "operation_id": operation,
    }))
    .unwrap();
    tokio::task::spawn_blocking(move || subjektiv_journal::release_subject(&api, input))
        .await
        .unwrap()
        .unwrap()
}

async fn wait_recorded(f: &RemovalFixture) {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let progress = f
                .api()
                .config_store
                .get_release_journal_recording(TEST_WORKSPACE_ID, &f.source)
                .unwrap()
                .unwrap();
            if progress.recording.completed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("released fixed scope reaches every checkpoint and closes Jobs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn explicit_release_hands_off_unadmitted_multisession_history_without_waiting_for_jobs() {
    let f = idle_source().await;
    let retained = f.seed_retained(vec![
        RemovalFixture::history("release-retained-first", "retained original before release"),
        RemovalFixture::history("release-unadmitted-tail", &"未投入の原文".repeat(16000)),
    ]);
    let response = release(&f, "release-pending-recording").await;
    assert_eq!(
        response.state,
        runtime_api::SubjectReleaseState::Disconnected
    );
    let saved = f
        .api()
        .config_store
        .get_release_journal_recording(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    assert!(saved.released && !saved.recording.completed);
    assert_eq!(saved.recording.members.len(), 2);
    assert!(
        saved
            .recording
            .members
            .iter()
            .any(|m| m.session_id == retained.session_id && m.descriptors.len() >= 3)
    );
    assert!(f.root().exists(), "release is not removal");
    assert!(
        f.api()
            .config_store
            .recover_worker_removal_execution(TEST_WORKSPACE_ID, &f.source)
            .unwrap()
            .is_none()
    );
    let store = open_subjektiv_store(f.api()).unwrap();
    assert!(
        store
            .journal_worker_was_released(
                &f.attr.subject_id,
                &f.source.runtime_id,
                &f.source.worker_id
            )
            .unwrap()
    );
    let next = RemovalFixture::create_body(f.api(), &f.attr.subject_id, false).await;
    f.wait_for_input().await;
    let observed = browser_recording(&f).await;
    assert_eq!(observed["producer"], "released");
    assert_eq!(observed["completed"], false);
    assert_eq!(
        release(&f, "release-pending-recording").await.state,
        runtime_api::SubjectReleaseState::Disconnected
    );
    let key = crate::subjektiv::subject_worker_singleton_key(&f.attr.subject_id).unwrap();
    assert_eq!(
        f.api()
            .store
            .current_worker_singleton_owner(TEST_WORKSPACE_ID, &key)
            .unwrap()
            .unwrap()
            .worker,
        next
    );
    f.model.set(ModelAction::Record);
    wait_recorded(&f).await;
    assert_eq!(
        browser_recording(&f).await,
        json!({
            "producer": "released", "completed": true, "cause": null
        })
    );
    let current = f
        .api()
        .config_store
        .get_release_journal_recording(TEST_WORKSPACE_ID, &f.source)
        .unwrap()
        .unwrap();
    assert_eq!(current.recording.members, saved.recording.members);
    for member in &current.recording.members {
        let attr = store
            .session_attribution(&member.session_id)
            .unwrap()
            .unwrap();
        let host = subjektiv::journal::JournalHostContext::validated_session(&store, attr).unwrap();
        let id = store.journal_for_session(&host).unwrap().unwrap();
        let journal = store.read_journal(&host, &id).unwrap();
        assert_eq!(
            (journal.process_order, journal.process_byte),
            (member.end.canonical_order, member.end.byte_offset)
        );
    }
    RemovalFixture::stop_body(f.api(), &next).await;
    RemovalFixture::stop_body(f.api(), &f.source).await;
}
