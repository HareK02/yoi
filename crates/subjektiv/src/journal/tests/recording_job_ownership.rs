//! Store-boundary tests: Host grants are supplied, not fabricated Runtime proof.
//! The active generation, immutable attribution and erasure fences use real SQLite.
use super::*;

#[test]
fn recording_job_survives_source_body_release_without_borrowing_successor_authority() {
    let mut f = Fixture::new();
    let original_host = f.host.clone();
    let (old_generation, old_job) = f.generation("old-job", f.ranges.clone());
    f.store()
        .close_journal_generation(&original_host, &old_generation.id, &old_generation.job)
        .unwrap();
    f.store()
        .release_removed_journal_body(&original_host)
        .unwrap();
    let successor = f
        .store()
        .claim_journal_writer(&original_host.subject_id, "runtime", "successor")
        .unwrap();
    assert!(f.store().journal_writer_active(&successor).unwrap());
    assert!(
        f.store()
            .journal_resume_state(&original_host, &f.journal_id)
            .is_err()
    );

    let binding = JobAttemptBinding {
        job_id: "recording-job".into(),
        attempt_id: "attempt-1".into(),
    };
    f.host = JournalHostContext::validated_recording_job(
        f.store(),
        original_host.attribution.clone().unwrap(),
        &f.journal_id,
        binding.clone(),
    )
    .unwrap();
    assert!(
        f.host.writer_binding_id().is_none(),
        "recording never borrows the successor's body binding"
    );
    let generation = f
        .store()
        .prepare_journal_generation(&f.host, &f.journal_id, binding.clone(), f.ranges.clone())
        .unwrap();
    let context =
        JournalJobContext::validated(f.store(), &f.host, &generation.id, binding.clone()).unwrap();
    f.reopen();
    let receipt = f.execute(
        &context,
        "record-after-release",
        open_action("Retained original"),
        vec![],
    );
    assert!(receipt.entry_id.is_some());
    assert!(f.store().journal_writer_active(&successor).unwrap());
    assert!(
        f.store()
            .execute_journal(
                &old_job,
                f.operation("late-old-attempt", open_action("Must reject"), vec![])
            )
            .is_err()
    );

    for wrong in [
        JobAttemptBinding {
            job_id: binding.job_id.clone(),
            attempt_id: "attempt-2".into(),
        },
        JobAttemptBinding {
            job_id: "other-job".into(),
            attempt_id: binding.attempt_id.clone(),
        },
    ] {
        assert!(matches!(
            f.store().prepare_journal_generation(
                &f.host,
                &f.journal_id,
                wrong.clone(),
                f.ranges.clone()
            ),
            Err(JournalError::PermissionDenied(_))
        ));
        assert!(matches!(
            JournalJobContext::validated(f.store(), &f.host, &generation.id, wrong),
            Err(JournalError::PermissionDenied(_))
        ));
    }
    f.store()
        .close_journal_generation(&f.host, &generation.id, &binding)
        .unwrap();
    assert!(
        f.store()
            .execute_journal(
                &context,
                f.operation("late-closed-generation", open_action("Must reject"), vec![])
            )
            .is_err()
    );
}

#[test]
fn retained_handoff_releases_body_without_closing_the_recording_job() {
    let mut f = Fixture::new();
    let body = f.host.clone();
    let attr = body.attribution.clone().unwrap();
    let binding = JobAttemptBinding {
        job_id: "retained-job".into(),
        attempt_id: "attempt-1".into(),
    };
    f.host = JournalHostContext::validated_recording_job(
        f.store(),
        attr.clone(),
        &f.journal_id,
        binding.clone(),
    )
    .unwrap();
    let generation = f
        .store()
        .prepare_journal_generation(&f.host, &f.journal_id, binding.clone(), f.ranges.clone())
        .unwrap();
    let job = JournalJobContext::validated(f.store(), &f.host, &generation.id, binding).unwrap();
    let body_binding = JournalWriterBinding {
        subject_id: attr.subject_id.clone(),
        runtime_id: attr.runtime_id.clone(),
        worker_id: attr.worker_id.clone(),
        binding_id: body.writer_binding_id().unwrap().into(),
    };
    assert!(f.store().release_journal_writer(&body_binding).is_err());
    f.store()
        .release_retained_journal_writer(&body_binding)
        .unwrap();
    f.store()
        .release_retained_journal_writer(&body_binding)
        .unwrap();
    let successor = f
        .store()
        .claim_journal_writer(&attr.subject_id, "runtime", "next-body")
        .unwrap();
    f.reopen();
    assert!(f.store().journal_writer_active(&successor).unwrap());
    assert_eq!(
        f.store()
            .active_journal_generation(&f.host, &f.journal_id)
            .unwrap()
            .unwrap()
            .id,
        generation.id
    );
    assert!(
        f.execute(
            &job,
            "retained-write",
            open_action("Recording after body release"),
            vec![]
        )
        .entry_id
        .is_some()
    );
    assert!(
        f.store()
            .journal_resume_state(&body, &f.journal_id)
            .is_err()
    );
}

#[test]
fn retained_source_admission_is_not_job_authority_and_cannot_revive_erased_session() {
    let f = Fixture::new();
    let attr = f.host.attribution.clone().unwrap();
    let source = JournalHostContext::validated_recording_source(f.store(), attr.clone()).unwrap();
    assert_eq!(
        f.store()
            .accept_journal_sources(&source, "Retained originals", &f.ranges)
            .unwrap(),
        f.journal_id
    );
    assert!(
        f.store()
            .prepare_journal_generation(
                &source,
                &f.journal_id,
                JobAttemptBinding {
                    job_id: "not-authorized".into(),
                    attempt_id: "attempt".into()
                },
                f.ranges.clone()
            )
            .is_err()
    );
    f.store()
        .erase_journal_session(&f.host, "erase-retained-input")
        .unwrap();
    assert!(
        f.store()
            .accept_journal_sources(&source, "Must not regenerate", &f.ranges)
            .is_err()
    );
    assert!(JournalHostContext::validated_recording_source(f.store(), attr).is_err());
}

#[test]
fn recording_job_context_is_bound_to_one_journal_and_permanent_erasure() {
    let mut f = Fixture::new();
    let binding = JobAttemptBinding {
        job_id: "recording-job".into(),
        attempt_id: "attempt-1".into(),
    };
    let original_host = f.host.clone();
    let other_host = session_host(f.store(), &original_host.subject_id, "other-session");
    let other_ranges = vec![source_range("other-session", 0, 256)];
    let other_id = f
        .store()
        .accept_journal_sources(&other_host, "Other Journal", &other_ranges)
        .unwrap();
    assert!(
        JournalHostContext::validated_recording_job(
            f.store(),
            original_host.attribution.clone().unwrap(),
            &other_id,
            binding.clone()
        )
        .is_err()
    );
    f.host = JournalHostContext::validated_recording_job(
        f.store(),
        original_host.attribution.clone().unwrap(),
        &f.journal_id,
        binding.clone(),
    )
    .unwrap();
    assert!(matches!(
        f.store()
            .prepare_journal_generation(&f.host, &other_id, binding.clone(), other_ranges),
        Err(JournalError::PermissionDenied(_))
    ));
    let generation = f
        .store()
        .prepare_journal_generation(&f.host, &f.journal_id, binding.clone(), f.ranges.clone())
        .unwrap();
    let context =
        JournalJobContext::validated(f.store(), &f.host, &generation.id, binding.clone()).unwrap();
    let late = f.operation("late-erased-generation", open_action("Must reject"), vec![]);
    f.store()
        .erase_journal_session(&original_host, "explicit-erase")
        .unwrap();
    assert!(f.store().execute_journal(&context, late).is_err());
    assert!(
        JournalHostContext::validated_recording_job(
            f.store(),
            original_host.attribution.clone().unwrap(),
            &f.journal_id,
            binding
        )
        .is_err()
    );
    assert!(f.store().read_journal(&other_host, &other_id).is_ok());
}
