//! Journal recovery connects the real local Host, Journal store and temporary Job SQLite
//! stores. Persisted prior-owner rows are injected without spawning a Worker; they cannot
//! supply cleanup proof. Scripted owned-Worker shutdown is covered in the parent suite.

use super::*;

#[test]
fn restart_reconciles_complete_checkpoint_but_retains_partial_input_and_all_unproven_cleanup() {
    for checkpoint_complete in [false, true] {
        let fixture = Fixture::new("restart uses only this frozen body");
        let (adapter, _, original) = fixture.adapter();
        let initial = adapter.state().unwrap();
        let saved = save(
            &adapter,
            "restart-record",
            &initial.current_journal.state_token,
            JournalAction::Create {
                title: "saved once".into(),
                body: "retained operation".into(),
                ongoing: "ongoing".into(),
                state: JournalEntryState::Open,
                source_end: None,
                correction_refs: vec![],
            },
            initial.selected_ranges.clone(),
        );
        if checkpoint_complete {
            let position = save(
                &adapter,
                "restart-position",
                &saved.state_token,
                JournalAction::UpdatePosition {
                    current_position: "selected input reflected".into(),
                },
                vec![],
            );
            save(
                &adapter,
                "restart-checkpoint",
                &position.state_token,
                JournalAction::Checkpoint {
                    record_operation_ids: vec!["restart-record".into()],
                },
                initial.selected_ranges,
            );
        }
        let path = fixture._temp.path().join("restart-job.sqlite3");
        let mut prior = crate::job_store::JobStore::open(&path).unwrap();
        prior.bind_host_lease("prior-host").unwrap();
        let reserved = prior
            .reserve_granted(original.request.clone(), original.domain_grant.clone())
            .unwrap();
        let dispatched = prior
            .dispatch(&original.request.job_id, &reserved.attempt.attempt_id, 8)
            .unwrap();
        drop(prior);
        let mut next = crate::job_store::JobStore::open(&path).unwrap();
        next.bind_host_lease("replacement-host").unwrap();
        let failed = next.reconcile_host_restart().unwrap().pop().unwrap();
        assert_eq!(failed.state, JobState::Failed);
        assert_eq!(failed.executions, dispatched.executions);
        assert!(!failed.version_stopped);
        drop(next);
        let jobs = crate::jobs::StandaloneJobs::open(&path, fixture._temp.path().into()).unwrap();
        // A missing live Session makes any source re-fetch an observable failure.
        let source = fixture
            .sessions
            .root_dir()
            .join(&fixture.context.session_id);
        let unavailable = fixture._temp.path().join("unavailable-live-session");
        fs::rename(&source, &unavailable).unwrap();
        let error = fixture
            .host()
            .recover_journal_job(&jobs, &failed)
            .unwrap_err();
        let settled = jobs.get(&original.request.job_id).unwrap();
        if checkpoint_complete {
            assert!(
                matches!(error, SubjektivHostError::Conflict {ref code, ..} if code == "journal_cleanup_unconfirmed")
            );
            assert_eq!(settled.state, JobState::Completed);
            let completion: JournalJobCompletion =
                serde_json::from_value(settled.result.clone().unwrap()).unwrap();
            assert_eq!(completion.generation_id, adapter.generation.id);
            assert!(settled.acknowledged);
        } else {
            assert!(
                matches!(error, SubjektivHostError::Conflict {ref code, ..} if code == "journal_execution_unreleased")
            );
            assert_eq!(settled, failed);
            assert!(settled.result.is_none());
        }
        assert_eq!(settled.attempt.attempt_id, reserved.attempt.attempt_id);
        assert_eq!(settled.attempt.number, 1);
        assert_eq!(settled.request, original.request);
        assert_eq!(settled.input_disposition, JobInputDisposition::Available);
        assert_eq!(settled.executions, dispatched.executions);
        assert!(!settled.executions_released());
        assert!(jobs.pending().unwrap().is_empty());
        assert!(
            jobs.resume_granted(&settled.request.job_id, &settled.attempt.attempt_id)
                .is_err()
        );
        let entries = fixture
            .host()
            .catalog
            .store
            .list_journal_entries(&adapter.context, &adapter.grant.journal_id, None, 32)
            .unwrap();
        assert_eq!(entries.items.len(), 1);
        assert_eq!(entries.items[0].id, saved.entry_id.unwrap());
        fs::rename(&unavailable, &source).unwrap();
    }
}

#[test]
fn current_schema_reopen_preserves_unconfirmed_execution_and_blocks_journal_resume() {
    let fixture = Fixture::new("saved input must not be recaptured");
    let (adapter, _, original) = fixture.adapter();
    let state = adapter.state().unwrap();
    let saved = save(
        &adapter,
        "saved-before-owner-loss",
        &state.current_journal.state_token,
        JournalAction::Create {
            title: "partial operation".into(),
            body: "durable record without checkpoint".into(),
            ongoing: "unconfirmed execution".into(),
            state: JournalEntryState::Open,
            source_end: None,
            correction_refs: vec![],
        },
        state.selected_ranges,
    );
    let source = fixture
        .sessions
        .root_dir()
        .join(&fixture.context.session_id);
    let denied = fixture._temp.path().join("unavailable-recovery-source");
    fs::rename(&source, &denied).unwrap();
    for fail_attempt in [false, true] {
        let path = fixture._temp.path().join(if fail_attempt {
            "prior-failed.sqlite3"
        } else {
            "prior-dispatched.sqlite3"
        });
        let mut prior = crate::job_store::JobStore::open(&path).unwrap();
        let reserved = prior
            .reserve_granted(original.request.clone(), original.domain_grant.clone())
            .unwrap();
        let mut retained = prior
            .dispatch(&original.request.job_id, &reserved.attempt.attempt_id, 8)
            .unwrap();
        if fail_attempt {
            retained = prior
                .finish(
                    &original.request.job_id,
                    &reserved.attempt.attempt_id,
                    JobAttemptState::Failed,
                    Some("prior execution failed without cleanup evidence"),
                )
                .unwrap();
        }
        drop(prior);
        let reopened =
            crate::jobs::StandaloneJobs::open(&path, fixture._temp.path().into()).unwrap();
        assert_eq!(reopened.get(&original.request.job_id).unwrap(), retained);
        assert!(!retained.version_stopped);
        assert!(!retained.executions_released());
        assert_eq!(retained.executions.len(), 1);
        assert!(!retained.executions[0].writes_revoked);
        assert!(retained.executions[0].cleanup_receipt.is_none());
        let error = match fixture.host().recover_journal_job(&reopened, &retained) {
            Err(error) => error,
            Ok(_) => panic!("unconfirmed prior execution must block Journal recovery"),
        };
        assert!(matches!(
            error,
            SubjektivHostError::Conflict { ref code, .. }
                if code == "journal_execution_unreleased"
        ));
        assert!(reopened.retry(&original.request.job_id).is_err());
        assert!(
            reopened
                .reconcile_granted_attempt(
                    &original.request.job_id,
                    &retained.attempt.attempt_id,
                    "domain-receipts-alone-are-not-cleanup",
                )
                .is_err()
        );
        assert!(
            reopened
                .resume_granted(&original.request.job_id, &retained.attempt.attempt_id)
                .is_err()
        );
        let mut replacement = original.request.clone();
        replacement.job_id = format!("replacement-{fail_attempt}");
        assert!(
            reopened
                .request_granted(replacement, original.domain_grant.clone().unwrap())
                .is_err(),
            "unconfirmed execution retains the serialization fence"
        );
        assert_eq!(reopened.get(&original.request.job_id).unwrap(), retained);
        assert_eq!(
            reopened
                .list_session_snapshots(&fixture.context.session_id, None, 100)
                .unwrap()
                .len(),
            1
        );
        assert!(matches!(
            reopened.try_shutdown(),
            Err(crate::jobs::JobServiceError::ShutdownIncomplete)
        ));
        // Only prior-owner DB rows exist here: dropping this observer is not cleanup proof.
        drop(reopened);
        let reread = crate::job_store::JobStore::open(&path).unwrap();
        assert_eq!(reread.get(&original.request.job_id).unwrap(), retained);
    }
    fs::rename(&denied, &source).unwrap();
    let entries = fixture
        .host()
        .catalog
        .store
        .list_journal_entries(&adapter.context, &adapter.grant.journal_id, None, 32)
        .unwrap();
    assert_eq!(entries.items.len(), 1);
    assert_eq!(entries.items[0].id, saved.entry_id.unwrap());
    assert_eq!(
        fixture
            .host()
            .catalog
            .store
            .active_journal_generation(&adapter.context, &adapter.grant.journal_id)
            .unwrap()
            .unwrap(),
        adapter.generation
    );
}

/// Minimal schema-five storage graph for the Journal cutover boundary. Legacy state
/// spellings belong to this persisted input fixture, never to the current Rust enums.
pub(super) fn seed_schema_five_journal(
    path: &std::path::Path,
    request: &job::JobRequest,
    grant: &LocalJournalGrant,
) {
    let mut conn = rusqlite::Connection::open(path).unwrap();
    feature_storage::configure_connection(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(
        "CREATE TABLE job_intents (
            job_id TEXT PRIMARY KEY NOT NULL,
            request_json TEXT NOT NULL,
            fingerprint TEXT NOT NULL,
            serialization_key TEXT,
            max_concurrent INTEGER NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('pending','completed','failed','unknown','cancelled')),
            current_attempt INTEGER NOT NULL,
            acknowledged INTEGER NOT NULL DEFAULT 0,
            input_disposition TEXT NOT NULL DEFAULT 'available',
            FOREIGN KEY(job_id,current_attempt) REFERENCES job_attempts(job_id,number)
                DEFERRABLE INITIALLY DEFERRED
         );
         CREATE TABLE job_attempts (
            job_id TEXT NOT NULL REFERENCES job_intents(job_id),
            number INTEGER NOT NULL,
            attempt_id TEXT NOT NULL UNIQUE,
            input_digest TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('reserved','dispatched','completed','failed','unknown','cancelled')),
            result_json TEXT,
            result_digest TEXT,
            failure TEXT,
            PRIMARY KEY(job_id,number)
         );
         CREATE UNIQUE INDEX job_serialization_singleflight ON job_intents(serialization_key)
            WHERE serialization_key IS NOT NULL AND state='pending';
         CREATE TABLE job_domain_grants (
            job_id TEXT PRIMARY KEY NOT NULL REFERENCES job_intents(job_id),
            grant_json TEXT NOT NULL
         );
         CREATE TABLE job_domain_reconciliations (
            job_id TEXT NOT NULL REFERENCES job_intents(job_id),
            attempt_id TEXT NOT NULL,
            domain_receipt TEXT NOT NULL,
            PRIMARY KEY(job_id,attempt_id)
         );",
    )
    .unwrap();
    tx.execute(
        "INSERT INTO job_intents(job_id,request_json,fingerprint,serialization_key,max_concurrent,state,current_attempt)
         VALUES(?1,?2,?3,?4,?5,'pending',1)",
        rusqlite::params![
            request.job_id,
            serde_json::to_string(request).unwrap(),
            request.fingerprint().unwrap(),
            request.serialization_key,
            request.limits.max_concurrent_jobs,
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO job_attempts(job_id,number,attempt_id,input_digest,state)
         VALUES(?1,1,?2,?3,'reserved')",
        rusqlite::params![
            request.job_id,
            job::attempt_id(&request.job_id, 1),
            request.input_digest().unwrap(),
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO job_domain_grants(job_id,grant_json) VALUES(?1,?2)",
        rusqlite::params![request.job_id, serde_json::to_string(grant).unwrap()],
    )
    .unwrap();
    tx.pragma_update(None, "user_version", 5).unwrap();
    tx.commit().unwrap();
}
