//! Real standalone Subject/Controller/Journal Job/initial input connection. Only external
//! model streams are scripted. The scenario checks persisted data and ordering, not whether
//! a real model understands the correction or can autonomously continue the work.

use super::*;
use crate::{StandaloneHost, StandaloneLaunchConfig, StandaloneSubjectDisconnectError};
use agen::llm_client::types::Item;

fn launch(root: &Path, state: &Path, subject: &str, name: &str) -> crate::ResolvedStandaloneLaunch {
    StandaloneLaunchConfig::new(
        root,
        state,
        manifest::ProfileSelector::source_named(
            manifest::ProfileRegistrySource::Builtin,
            "standalone-subjektiv",
        ),
        name,
    )
    .with_subject(subject.into())
    .resolve()
    .unwrap()
}

#[tokio::test]
async fn recorded_idle_body_handoff_preserves_open_after_source_deletion_and_seeds_next_body_once()
{
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let catalog = StandaloneSubjects::open(&state).unwrap();
        let subject = catalog.create("continuity", Some("BEHAVIOR_ORIGINAL_UNCHANGED")).unwrap();
        let model = DialogAndJournal { journal: JournalScript { gate_first: true, ..Default::default() }, ..Default::default() };
        let mut first = StandaloneHost::start_with_model_client(launch(temp.path(), &state, &subject.id, "first"), model.clone()).await.unwrap();
        let old_worker = first.worker_id();
        let old_session = first.record().active_session_id;
        let old_writer = catalog.store.claim_journal_writer(&subject.id, &catalog.scope_id, &old_worker.to_string()).unwrap();
        submit_dialog(&first, "A was proposed, user corrected its constraint; B selected because A is unusable. One step performed; tests not performed; external outcome Unknown. Work is unfinished.").await;
        model.journal.entered.notified().await;
        assert!(matches!(first.disconnect_subject().await, Err(StandaloneSubjectDisconnectError::Incomplete(_))));
        assert!(catalog.store.journal_writer_active(&old_writer).unwrap());
        assert!(StandaloneHost::start_with_model_client(launch(temp.path(), &state, &subject.id, "premature"), DialogAndJournal::default()).await.is_err());
        let context = JournalHostContext::validated_session(&catalog.store, catalog.store.session_attribution(&old_session.to_string()).unwrap().unwrap()).unwrap();
        let id = catalog.store.journal_for_session(&context).unwrap().unwrap();
        let generation = catalog.store.active_journal_generation(&context, &id).unwrap().unwrap();
        model.journal.release.notify_one();
        first.jobs().wait(&generation.job.job_id).await.unwrap();
        // Observe the authoritative gate after cleanup; yielding is cooperative scheduling,
        // never elapsed time used as proof of recording completion.
        loop {
            match first.disconnect_subject().await {
                Ok(()) => break,
                Err(StandaloneSubjectDisconnectError::Incomplete(message)) if message.contains("dispatcher") || message.contains("cleanup") => tokio::task::yield_now().await,
                Err(error) => panic!("unexpected handoff blocker: {error}"),
            }
        }
        assert!(!catalog.store.journal_writer_active(&old_writer).unwrap());
        assert!(catalog.store.accept_journal_sources(&context, "late old activity", &[generation.selected_ranges[0].clone()]).is_err());
        first.shutdown().await.unwrap();
        let workers = StandaloneWorkerStore::open_existing(&state).unwrap();
        workers.delete(old_worker).unwrap();
        assert!(!workers.sessions_dir(old_worker).exists());
        let reader = JournalHostContext::validated_reader(&catalog.store, &subject.id).unwrap();
        let historical = catalog.store.journal_startup_snapshot(&reader, Some(&old_session.to_string())).unwrap();
        assert_eq!(historical.journals[0].entries[0].state, JournalEntryState::Open);
        assert!(historical.journals[0].entries[0].ongoing.contains("unknown"));
        assert!(catalog.store.list_memories(&subject.id).unwrap().is_empty());

        let second_model = DialogAndJournal::default();
        let second = StandaloneHost::start_with_model_client(launch(temp.path(), &state, &subject.id, "second"), second_model.clone()).await.unwrap();
        submit_dialog(&second, "Continue B; describe what is still unperformed and unconfirmed.").await;
        let requests = second_model.dialog_requests.lock().unwrap();
        let initial = &requests[0];
        let context_index = initial.items.iter().position(|item| item.as_text().is_some_and(|text| text.contains("Proposed A; user corrected it to B"))).expect("saved Journal visible to first model request");
        let user_index = initial.items.iter().position(|item| matches!(item, Item::Message { role: agen::llm_client::types::Role::User, .. })).unwrap();
        assert!(context_index < user_index);
        assert_eq!(initial.items.iter().filter(|item| item.as_text().is_some_and(|text| text.contains("Proposed A; user corrected it to B"))).count(), 1);
        drop(requests);
        let session = second.record().active_session_id;
        let sessions = session_store::FsStore::new_with_retention(workers.sessions_dir(second.worker_id()), crate::store::session_retention(&state)).unwrap();
        let pointer = session_store::FsWorkerStore::new(workers.worker_metadata_dir(second.worker_id())).unwrap().read_by_name(&second.record().storage_key).unwrap().unwrap().active.unwrap();
        let entries = sessions.read_all(session, pointer.segment_id.unwrap()).unwrap();
        let journal_items = entries.iter().filter(|entry| matches!(entry, session_store::LogEntry::AnnotatedSystemItem { entry, .. } if entry.metadata.input_metadata.as_ref().and_then(|input| input.feature_payload()).is_some_and(|payload| payload.schema() == "journal:startup"))).count();
        assert_eq!(journal_items, 1);
        assert_eq!(catalog.store.subject(&subject.id).unwrap().unwrap().behavior_md, "BEHAVIOR_ORIGINAL_UNCHANGED");
        second.shutdown().await.unwrap();
    }).await.expect("bounded complete Subject handoff connection");
}

#[tokio::test]
async fn saved_public_history_without_admission_notification_blocks_release_until_actual_recording()
{
    tokio::time::timeout(std::time::Duration::from_secs(60), async {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().join("state");
        let catalog = StandaloneSubjects::open(&state).unwrap();
        let subject = catalog.create("saved-before-notified", None).unwrap();
        let model = DialogAndJournal {
            journal: JournalScript {
                gate_first: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host = StandaloneHost::start_with_model_client(
            launch(temp.path(), &state, &subject.id, "gap"),
            model.clone(),
        )
        .await
        .unwrap();
        let workers = StandaloneWorkerStore::open_existing(&state).unwrap();
        let record = host.record().clone();
        let writer = catalog
            .store
            .claim_journal_writer(
                &subject.id,
                &catalog.scope_id,
                &host.worker_id().to_string(),
            )
            .unwrap();
        let sessions = session_store::FsStore::new_with_retention(
            workers.sessions_dir(host.worker_id()),
            crate::store::session_retention(&state),
        )
        .unwrap();
        // The production saved-history boundary, with its admission callback deliberately
        // omitted. The real Host release must discover these bytes, not trust Job status.
        sessions
            .append(
                record.active_session_id,
                record.active_segment_id.unwrap(),
                &session_store::LogEntry::AnnotatedUserInput {
                    ts: 3,
                    segments: vec![],
                    extensions: vec![],
                    history: vec![message(
                        "missed-admission",
                        "A corrected to B, partially performed, outcome Unknown",
                    )],
                },
            )
            .unwrap();
        assert!(host.disconnect_subject().await.is_err());
        assert!(catalog.store.journal_writer_active(&writer).unwrap());
        model.journal.entered.notified().await;
        let context = JournalHostContext::validated_session(
            &catalog.store,
            catalog
                .store
                .session_attribution(&record.active_session_id.to_string())
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let id = catalog
            .store
            .journal_for_session(&context)
            .unwrap()
            .unwrap();
        let generation = catalog
            .store
            .active_journal_generation(&context, &id)
            .unwrap()
            .unwrap();
        model.journal.release.notify_one();
        host.jobs().wait(&generation.job.job_id).await.unwrap();
        loop {
            match host.disconnect_subject().await {
                Ok(()) => break,
                Err(StandaloneSubjectDisconnectError::Incomplete(message))
                    if message.contains("dispatcher") || message.contains("cleanup") =>
                {
                    tokio::task::yield_now().await
                }
                Err(error) => {
                    panic!("unexpected release blocker after saved-history recording: {error}")
                }
            }
        }
        assert!(!catalog.store.journal_writer_active(&writer).unwrap());
        let snapshot = catalog
            .store
            .journal_startup_snapshot(
                &JournalHostContext::validated_reader(&catalog.store, &subject.id).unwrap(),
                None,
            )
            .unwrap();
        assert!(!snapshot.journals[0].current_position.is_empty());
        assert_eq!(
            snapshot.journals[0].entries[0].state,
            JournalEntryState::Open
        );
        host.shutdown().await.unwrap();
    })
    .await
    .expect("saved-history/notification-gap handoff is bounded");
}

#[tokio::test]
async fn shutdown_and_normal_deletion_cannot_release_subject_occupancy() {
    let temp = tempfile::tempdir().unwrap();
    let state = temp.path().join("state");
    let catalog = StandaloneSubjects::open(&state).unwrap();
    let subject = catalog.create("retained-owner", None).unwrap();
    let host = StandaloneHost::start_with_model_client(
        launch(temp.path(), &state, &subject.id, "retained"),
        DialogAndJournal::default(),
    )
    .await
    .unwrap();
    let worker = host.worker_id();
    host.shutdown().await.unwrap();
    let workers = StandaloneWorkerStore::open_existing(&state).unwrap();
    assert!(workers.delete(worker).is_err());
    assert!(
        StandaloneHost::start_with_model_client(
            launch(temp.path(), &state, &subject.id, "not-an-owner"),
            DialogAndJournal::default()
        )
        .await
        .is_err()
    );
    let mut restored =
        StandaloneHost::restore_with_model_client(state, worker, DialogAndJournal::default())
            .await
            .unwrap();
    restored.disconnect_subject().await.unwrap();
    restored.shutdown().await.unwrap();
}
