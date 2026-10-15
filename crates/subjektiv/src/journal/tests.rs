//! Component tests through the Journal store boundary and production FeatureStorage migrations.
//! Only Host-attested Session fragments and Job bindings are supplied by the fixture; no Session
//! runtime or provider is started. Temporary storage owns cleanup. Receipt insertion faults are
//! deterministic SQLite triggers, not a claim of arbitrary process-crash durability.

use super::*;
use crate::SubjectRole;
use feature_storage::{FeatureStorage, ScopedFeatureStorage};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn stable_legacy_run_references_are_accepted_without_relaxing_character_or_size_limits() {
    let f = Fixture::new();
    let mut range = source_range("source-session", 2, 64);
    range.source.pointer.entry_ref =
        format!("El-run-{}-{}-123456789", Uuid::now_v7(), Uuid::now_v7());
    assert!(range.source.pointer.entry_ref.len() > 65);
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[range.clone()])
        .unwrap();
    assert_eq!(
        f.store()
            .read_journal_source(&f.host, &f.journal_id, 2)
            .unwrap()
            .source,
        range.source
    );
    let mut next = source_range("source-session", 3, 64);
    for reference in [
        format!("E{}", "a".repeat(129)),
        "Einvalid_ref".into(),
        "E".into(),
    ] {
        next.source.pointer.entry_ref = reference;
        assert!(matches!(
            f.store()
                .accept_journal_sources(&f.host, "Journal", &[next.clone()]),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            f.store().read_journal_source(&f.host, &f.journal_id, 3),
            Err(JournalError::NotFound(_))
        ));
    }
    next.source.pointer.entry_ref = format!("E{}", "a".repeat(128));
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[next])
        .unwrap();
}

#[test]
fn descriptor_only_job_cannot_record_unread_bytes_and_delivery_survives_reopen() {
    let mut f = Fixture::new();
    let text = "公開履歴から実際に供給された全文";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let binding = JobAttemptBinding {
        job_id: "descriptor-only".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), ranges.clone())
        .unwrap();
    let context =
        JournalJobContext::validated(f.store(), &host, &generation.id, binding.clone()).unwrap();
    let operation = JournalOperation {
        request_id: "save-only-after-reading".into(),
        expected_state: f.store().read_journal(&host, &id).unwrap().state_token,
        action: open_action("ongoing"),
        covered_ranges: ranges.clone(),
    };
    let execute = || transport::JournalJobRequest::Execute {
        operation: operation.clone(),
    };
    assert!(matches!(
        service::execute(f.store(), &context, execute(), |_| unreachable!()),
        Err(JournalError::Conflict(_))
    ));
    let first = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText {
            read: transport::JournalTextRead {
                target: transport::JournalTextTarget::Source {
                    pointer: ranges[0].source.pointer.clone(),
                },
                offset: 0,
                limit: 6,
                content_token: None,
            },
        },
        |_| Ok(text.into()),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(first) = first else {
        panic!("source page required")
    };
    assert_eq!(first.text, "公開");
    assert!(matches!(
        service::execute(f.store(), &context, execute(), |_| unreachable!()),
        Err(JournalError::Conflict(_))
    ));
    f.reopen();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let remainder = transport::JournalJobRequest::ReadText {
        read: transport::JournalTextRead {
            target: transport::JournalTextTarget::Source {
                pointer: ranges[0].source.pointer.clone(),
            },
            offset: first.next_byte.unwrap(),
            limit: MAX_JOURNAL_READ_BYTES,
            content_token: Some(first.content_token),
        },
    };
    // A failed source fetch must not turn allowed descriptors into delivered input.
    assert!(
        service::execute(f.store(), &context, remainder.clone(), |_| Err(
            JournalError::NotFound("original source missing".into())
        ))
        .is_err()
    );
    assert!(service::execute(f.store(), &context, execute(), |_| unreachable!()).is_err());
    service::execute(f.store(), &context, remainder, |_| Ok(text.into())).unwrap();
    let response = service::execute(f.store(), &context, execute(), |_| unreachable!()).unwrap();
    assert!(matches!(response, transport::JournalJobResponse::Saved(_)));
    assert_eq!(
        f.store()
            .list_journal_entries(&host, &id, None, 32)
            .unwrap()
            .items
            .len(),
        1
    );
    // Original bytes are not duplicated into the durable delivery ledger.
    f.store()
        .database
        .try_with_connection(|tx| {
            let delivered: String = tx.query_row(
                "SELECT ranges_json FROM journal_generation_deliveries WHERE generation_id=?1",
                [&generation.id],
                |r| r.get(0),
            )?;
            assert!(!delivered.contains(text));
            assert_eq!(
                serde_json::from_str::<Vec<JournalByteRange>>(&delivered).unwrap(),
                vec![ranges[0].bytes]
            );
            Ok::<_, JournalError>(())
        })
        .unwrap();
}

#[test]
fn oversized_source_response_does_not_attest_undelivered_bytes() {
    let f = Fixture::new();
    let text = "\u{1}".repeat(MAX_JOURNAL_READ_BYTES);
    let (host, id, ranges) = recording_sources(&f, &[&text]);
    let binding = JobAttemptBinding {
        job_id: "escaped-source".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), ranges.clone())
        .unwrap();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText {
                read: transport::JournalTextRead {
                    target: transport::JournalTextTarget::Source {
                        pointer: ranges[0].source.pointer.clone()
                    },
                    offset: 0,
                    limit: MAX_JOURNAL_READ_BYTES,
                    content_token: None,
                },
            },
            |_| Ok(text.clone())
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let operation = JournalOperation {
        request_id: "unread-create".into(),
        expected_state: f.store().read_journal(&host, &id).unwrap().state_token,
        action: open_action("must not save"),
        covered_ranges: ranges,
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::Execute { operation },
            |_| unreachable!()
        ),
        Err(JournalError::Conflict(_))
    ));
    f.store()
        .database
        .try_with_connection(|tx| {
            let count: u64 = tx.query_row(
                "SELECT COUNT(*) FROM journal_generation_deliveries WHERE generation_id=?1",
                [&generation.id],
                |r| r.get(0),
            )?;
            assert_eq!(count, 0);
            Ok::<_, JournalError>(())
        })
        .unwrap();
}

#[test]
fn job_state_keeps_initial_position_and_bounds_current_text_with_stale_continuation_rejection() {
    let f = Fixture::new();
    let (_, context) = f.generation("state-reading-job", f.ranges.clone());
    let current = "現在地".repeat(8192);
    f.store()
        .execute_journal(
            &context,
            f.operation(
                "position-one",
                JournalAction::UpdatePosition {
                    current_position: current.clone(),
                },
                vec![],
            ),
        )
        .unwrap();
    let state = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadState,
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::State(state) = state else {
        panic!("missing state")
    };
    assert_eq!(state.initial_journal.current_position_bytes, 0);
    assert_eq!(state.current_journal.current_position_bytes, current.len());
    assert!(serde_json::to_vec(&state).unwrap().len() < 4096);
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Position {
            snapshot: transport::JournalSnapshot::Current,
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let page = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read: read.clone() },
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(page) = page else {
        panic!("missing text")
    };
    assert!(page.text.len() <= 100);
    assert_eq!(page.text, current[..page.text.len()]);
    let next = page.next_byte.unwrap();
    f.store()
        .execute_journal(
            &context,
            f.operation(
                "position-two",
                JournalAction::UpdatePosition {
                    current_position: "変更".repeat(8192),
                },
                vec![],
            ),
        )
        .unwrap();
    let continued = transport::JournalTextRead {
        offset: next,
        content_token: Some(page.content_token),
        ..read
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText { read: continued },
            |_| unreachable!()
        ),
        Err(JournalError::Conflict(_))
    ));
    let initial = transport::JournalTextRead {
        target: transport::JournalTextTarget::Position {
            snapshot: transport::JournalSnapshot::Initial,
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let initial = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read: initial },
        |_| unreachable!(),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(initial) = initial else {
        panic!("missing initial text")
    };
    assert!(initial.text.is_empty());
    assert_eq!(initial.next_byte, None);
}

#[test]
fn bounded_material_selection_fetches_only_a_budgeted_fragment_of_a_large_source() {
    let f = Fixture::with_sizes(&[16 * 1024 * 1024]);
    let materials = recording::select_materials_bounded(
        f.store(),
        &f.host,
        &f.journal_id,
        1024,
        |source, start, limit| {
            assert_eq!(start, 0);
            assert!(limit < 1024);
            Ok(recording::JournalMaterial {
                range: JournalSourceRange {
                    source: source.clone(),
                    bytes: JournalByteRange {
                        start,
                        end: start + limit as u64,
                    },
                },
                text: "x".repeat(limit),
            })
        },
    )
    .unwrap();
    assert_eq!(materials.len(), 1);
    assert!(serde_json::to_vec(&materials).unwrap().len() <= 1024);
    assert!(materials[0].range.bytes.end < f.ranges[0].source.total_bytes);
    assert_eq!(f.state().0.process_order, 0);
}

#[test]
fn bounded_material_selection_rejects_wrong_identity_range_and_unusable_input_budget() {
    let f = Fixture::new();
    for defect in 0..3 {
        let result = recording::select_materials_bounded(
            f.store(),
            &f.host,
            &f.journal_id,
            4096,
            |source, start, _| {
                let mut material = recording::JournalMaterial {
                    range: JournalSourceRange {
                        source: source.clone(),
                        bytes: JournalByteRange {
                            start,
                            end: start + 4,
                        },
                    },
                    text: "text".into(),
                };
                match defect {
                    0 => material.range.source.content_sha256 = "0".repeat(64),
                    1 => material.range.bytes.start += 1,
                    _ => material.range.bytes.end += 1,
                }
                Ok(material)
            },
        );
        assert!(matches!(result, Err(JournalError::Conflict(_))));
    }
    assert!(matches!(
        recording::select_materials_bounded(
            f.store(),
            &f.host,
            &f.journal_id,
            16,
            |_, _, _| panic!("unusable budget must not fetch")
        ),
        Err(JournalError::InvalidInput(_))
    ));
}

#[test]
fn erasure_failure_rolls_back_tombstone_and_content_then_exact_retry_succeeds() {
    let f = Fixture::new();
    let (_, job) = f.generation("erasure-fault", f.ranges.clone());
    let operation = f.operation(
        "record-before-fault",
        open_action("preserved until erase commits"),
        f.ranges.clone(),
    );
    f.store().execute_journal(&job, operation).unwrap();
    f.store().database.try_with_connection::<_,JournalError>(|tx| {
        tx.execute_batch("CREATE TRIGGER erase_fault BEFORE DELETE ON journal_entries BEGIN SELECT RAISE(ABORT,'erase fault'); END;")?;
        Ok(())
    }).unwrap();
    assert!(
        f.store()
            .erase_journal_session(&f.host, "erasure-fault-request")
            .is_err()
    );
    assert!(f.store().journal_for_session(&f.host).unwrap().is_some());
    assert!(f.state().1.is_some());
    assert!(
        f.store()
            .query_journal_operation(&f.host, &f.journal_id, "record-before-fault")
            .unwrap()
            .is_some()
    );
    f.store()
        .database
        .try_with_connection::<_, JournalError>(|tx| {
            tx.execute_batch("DROP TRIGGER erase_fault;")?;
            Ok(())
        })
        .unwrap();
    f.store()
        .erase_journal_session(&f.host, "erasure-fault-request")
        .unwrap();
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn job_service_completion_requires_checkpoint_and_replays_saved_completion_after_new_generation() {
    let mut f = Fixture::new();
    let (generation, context) = f.generation("service-job", f.ranges.clone());
    for range in &f.ranges {
        service::execute_bounded(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText {
                read: transport::JournalTextRead {
                    target: transport::JournalTextTarget::Source {
                        pointer: range.source.pointer.clone(),
                    },
                    offset: 0,
                    limit: MAX_JOURNAL_READ_BYTES,
                    content_token: None,
                },
            },
            |source, start, limit| {
                let end = source.total_bytes.min(start + limit as u64);
                Ok(recording::JournalMaterial {
                    range: JournalSourceRange {
                        source: source.clone(),
                        bytes: JournalByteRange { start, end },
                    },
                    text: "x".repeat((end - start) as usize),
                })
            },
        )
        .unwrap();
    }
    let create = f.operation("service-create", open_action("ongoing"), f.ranges.clone());
    let response = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::Execute { operation: create },
        |_| panic!("record operation has no source fetch"),
    )
    .unwrap();
    let transport::JournalJobResponse::Saved(receipt) = response else {
        panic!("missing receipt")
    };
    assert!(matches!(
        service::finish(f.store(), &context),
        Err(JournalError::Conflict(_))
    ));
    service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::Execute {
            operation: JournalOperation {
                request_id: "service-checkpoint".into(),
                expected_state: receipt.state_token,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["service-create".into()],
                },
                covered_ranges: f.ranges.clone(),
            },
        },
        |_| unreachable!(),
    )
    .unwrap();
    let completion = service::finish(f.store(), &context).unwrap();
    assert_eq!(completion.generation_id, generation.id);
    assert_eq!((completion.process_order, completion.process_byte), (2, 0));
    assert!(
        f.store()
            .active_journal_generation(&f.host, &f.journal_id)
            .unwrap()
            .is_none()
    );
    f.reopen();
    assert!(
        JournalJobContext::validated(f.store(), &f.host, &generation.id, generation.job.clone())
            .is_err()
    );
    let context = JournalJobContext::validated_recovery(
        f.store(),
        &f.host,
        &generation.id,
        generation.job.clone(),
    )
    .unwrap();
    assert_eq!(service::finish(f.store(), &context).unwrap(), completion);
    let next_range = source_range("source-session", 2, 256);
    f.store()
        .accept_journal_sources(&f.host, "Journal", &[next_range.clone()])
        .unwrap();
    let (_, next) = f.generation("next-service-job", vec![next_range]);
    let update = f.operation(
        "later-position",
        JournalAction::UpdatePosition {
            current_position: "new generation".into(),
        },
        vec![],
    );
    f.store().execute_journal(&next, update.clone()).unwrap();
    assert_eq!(service::finish(f.store(), &context).unwrap(), completion);
    assert!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::Execute { operation: update },
            |_| unreachable!()
        )
        .is_err()
    );
}

#[test]
fn job_service_source_reads_are_generation_bound_and_stop_at_supplied_byte_end() {
    let f = Fixture::new();
    let text = "public source with tail";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let mut selected = ranges[0].clone();
    selected.bytes.end = 6;
    let binding = JobAttemptBinding {
        job_id: "bounded-service".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), vec![selected.clone()])
        .unwrap();
    let context = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Source {
            pointer: selected.source.pointer.clone(),
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    let response = service::execute(
        f.store(),
        &context,
        transport::JournalJobRequest::ReadText { read },
        |_| Ok(text.into()),
    )
    .unwrap();
    let transport::JournalJobResponse::Text(page) = response else {
        panic!("missing text page")
    };
    assert_eq!(page.text, "public");
    assert_eq!(page.next_byte, None);
    let read = transport::JournalTextRead {
        target: transport::JournalTextTarget::Source {
            pointer: f.ranges[0].source.pointer.clone(),
        },
        offset: 0,
        limit: 100,
        content_token: None,
    };
    assert!(matches!(
        service::execute(
            f.store(),
            &context,
            transport::JournalJobRequest::ReadText { read },
            |_| panic!("foreign source must not be fetched")
        ),
        Err(JournalError::PermissionDenied(_))
    ));
}

#[test]
fn explicit_erasure_remains_available_for_retired_subject_without_reopening_recording() {
    let f = Fixture::new();
    f.store().retire_subject(&f.host.subject_id).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .erase_journal_session(&f.host, "erase-retired")
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        f.store()
            .accept_journal_sources(&f.host, "late", &f.ranges)
            .is_err()
    );
}

#[test]
fn explicit_erasure_removes_saved_content_and_fences_late_admission_after_reopen() {
    let mut f = Fixture::new();
    let (generation, job) = f.generation("erase-job", f.ranges.clone());
    let operation = f.operation(
        "created-before-erasure",
        open_action("private content"),
        f.ranges.clone(),
    );
    f.store().execute_journal(&job, operation.clone()).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    assert!(!f.store().journal_session_erased(&f.host).unwrap());
    f.store()
        .erase_journal_session(&f.host, "erase-request")
        .unwrap();
    f.reopen();
    f.store()
        .erase_journal_session(&f.host, "erase-request")
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(f.store().journal_session_erased(&f.host).unwrap());
    let reader = JournalHostContext::validated_reader(f.store(), &f.host.subject_id).unwrap();
    assert!(matches!(
        f.store().journal_session_erased(&reader),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(
        f.store()
            .list_journals(&f.host, None, 32)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(matches!(
        f.store()
            .accept_journal_sources(&f.host, "late retry", &f.ranges),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(f.store().execute_journal(&job, operation).is_err());
    assert!(
        JournalJobContext::validated(f.store(), &f.host, &generation.id, generation.job).is_err()
    );
}

#[test]
fn explicit_erasure_before_admission_prevents_later_creation_and_cross_session_key_reuse() {
    let f = Fixture::new();
    let other = session_host(f.store(), &f.host.subject_id, "not-yet-admitted");
    f.store()
        .erase_journal_session(&other, "erase-unadmitted")
        .unwrap();
    assert!(matches!(
        f.store()
            .accept_journal_sources(&other, "late", &[source_range("not-yet-admitted", 0, 8)]),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        f.store().erase_journal_session(&f.host, "erase-unadmitted"),
        Err(JournalError::Conflict(_))
    ));
    assert!(f.store().read_journal(&f.host, &f.journal_id).is_ok());
}

#[test]
fn retention_release_requires_checkpoint_not_saved_entry_or_unavailability() {
    let f = Fixture::new();
    let (_, job) = f.generation("retained-job", f.ranges.clone());
    let operation = f.operation("record", open_action("ongoing activity"), f.ranges.clone());
    let receipt = f.store().execute_journal(&job, operation).unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .mark_journal_sources_unavailable(&f.host, &f.journal_id, &f.ranges)
        .unwrap();
    assert!(f.store().journal_retention_required(&f.host).unwrap());
    f.store()
        .mark_journal_sources_available(&f.host, &f.journal_id, &f.ranges)
        .unwrap();
    f.store()
        .checkpoint_journal(
            &job,
            JournalOperation {
                request_id: "retained-checkpoint".into(),
                expected_state: receipt.state_token,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["record".into()],
                },
                covered_ranges: f.ranges.clone(),
            },
        )
        .unwrap();
    assert!(!f.store().journal_retention_required(&f.host).unwrap());
    assert!(f.state().1.is_some());
}

fn recording_sources(
    f: &Fixture,
    texts: &[&str],
) -> (JournalHostContext, String, Vec<JournalSourceRange>) {
    let host = session_host(f.store(), &f.host.subject_id, "recording-session");
    let ranges = texts
        .iter()
        .enumerate()
        .map(|(order, text)| {
            let mut range = source_range("recording-session", order as u64, text.len() as u64);
            range.source.content_sha256 = Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            range
        })
        .collect::<Vec<_>>();
    let id = f
        .store()
        .accept_journal_sources(&host, "Recording", &ranges)
        .unwrap();
    (host, id, ranges)
}

#[test]
fn recording_materials_bound_escaped_utf8_without_advancing_reflection() {
    let f = Fixture::new();
    let text = "日本語\n\"\\".repeat(4000);
    let (host, id, ranges) = recording_sources(&f, &[&text]);
    let materials =
        recording::select_materials(f.store(), &host, &id, 1024, |_| Ok(text.clone())).unwrap();
    assert_eq!(materials.len(), 1);
    assert!(serde_json::to_vec(&materials).unwrap().len() <= 1024);
    let range = &materials[0].range;
    assert_eq!(range.bytes.start, 0);
    assert!(range.bytes.end < ranges[0].source.total_bytes);
    assert_eq!(materials[0].text, text[..range.bytes.end as usize]);
    let status = f.store().read_journal_source(&host, &id, 0).unwrap();
    assert_eq!(status.reflected_bytes, 0);
    assert_eq!(status.accepted, vec![ranges[0].bytes]);
}

#[test]
fn recording_materials_reject_changed_bytes_instead_of_using_preview_identity() {
    let f = Fixture::new();
    let (host, id, _) = recording_sources(&f, &["canonical public text"]);
    assert!(matches!(
        recording::select_materials(f.store(), &host, &id, 4096, |_| Ok("preview".into())),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(
        f.store()
            .read_journal_source(&host, &id, 0)
            .unwrap()
            .reflected_bytes,
        0
    );
}

#[test]
fn recording_materials_stop_at_unavailable_old_source_without_reading_newer_input() {
    let f = Fixture::new();
    let (host, id, ranges) = recording_sources(&f, &["old", "new"]);
    f.store()
        .mark_journal_sources_unavailable(&host, &id, &ranges[..1])
        .unwrap();
    let materials = recording::select_materials(f.store(), &host, &id, 4096, |_| {
        panic!("blocked prefix must not fetch newer input")
    })
    .unwrap();
    assert!(materials.is_empty());
}

#[test]
fn recording_materials_resume_exact_saved_byte_prefix_after_reopen() {
    let mut f = Fixture::new();
    let text = "first 日本語 second";
    let (host, id, ranges) = recording_sources(&f, &[text]);
    let selected =
        recording::select_materials(f.store(), &host, &id, 512, |_| Ok(text.into())).unwrap();
    let selected = vec![JournalSourceRange {
        bytes: JournalByteRange { start: 0, end: 6 },
        ..selected[0].range.clone()
    }];
    let binding = JobAttemptBinding {
        job_id: "recording-job".into(),
        attempt_id: "attempt".into(),
    };
    let generation = f
        .store()
        .prepare_journal_generation(&host, &id, binding.clone(), selected.clone())
        .unwrap();
    let job = JournalJobContext::validated(f.store(), &host, &generation.id, binding).unwrap();
    let state = f.store().read_journal(&host, &id).unwrap().state_token;
    let receipt = f
        .store()
        .execute_journal(
            &job,
            JournalOperation {
                request_id: "unchanged".into(),
                expected_state: state,
                action: JournalAction::ConfirmUnchanged,
                covered_ranges: selected.clone(),
            },
        )
        .unwrap();
    f.store()
        .checkpoint_journal(
            &job,
            JournalOperation {
                request_id: "checkpoint".into(),
                expected_state: receipt.state_token,
                action: JournalAction::Checkpoint {
                    record_operation_ids: vec!["unchanged".into()],
                },
                covered_ranges: selected,
            },
        )
        .unwrap();
    f.reopen();
    let remaining =
        recording::select_materials(f.store(), &host, &id, 4096, |_| Ok(text.into())).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0].range.bytes,
        JournalByteRange {
            start: 6,
            end: ranges[0].source.total_bytes
        }
    );
    assert_eq!(remaining[0].text, text[6..]);
}

struct Fixture {
    store: Option<SubjektivStore>,
    workspace: Option<ScopedFeatureStorage>,
    manager: Option<FeatureStorage>,
    temp: tempfile::TempDir,
    host: JournalHostContext,
    journal_id: String,
    ranges: Vec<JournalSourceRange>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_sizes(&[256, 256])
    }

    fn with_sizes(sizes: &[u64]) -> Self {
        Self::with_schema(sizes, false)
    }

    fn with_schema(sizes: &[u64], upgrade: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager.workspace("journal-tests").unwrap();
        let registration = if upgrade {
            workspace
                .register(feature_storage::FeatureRegistration::new(
                    crate::SUBJEKTIV_FEATURE_ID,
                    &crate::MIGRATIONS[..crate::MIGRATIONS
                        .iter()
                        .position(|migration| migration.version() == 8)
                        .expect("Journal introduction migration")],
                ))
                .unwrap()
        } else {
            SubjektivStore::register(&workspace).unwrap()
        };
        let store = SubjektivStore::open(&workspace, &registration).unwrap();
        let subject = store
            .create_subject_with_behavior(
                SubjectRole::new("companion").unwrap(),
                if upgrade {
                    "Preserve existing Subject behavior".into()
                } else {
                    String::new()
                },
            )
            .unwrap();
        let host = session_host(&store, &subject.id, "source-session");
        let (manager, workspace, store) = if upgrade {
            let has_journals: bool = store.database.try_with_connection::<_, JournalError>(|connection| {
                Ok(connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='journals')", [], |row| row.get(0))?)
            }).unwrap();
            assert!(!has_journals);
            drop(store);
            drop(workspace);
            manager.shutdown().unwrap();
            drop(manager);
            let manager = FeatureStorage::new(temp.path().join("storage"));
            let workspace = manager.workspace("journal-tests").unwrap();
            let registration = SubjektivStore::register(&workspace).unwrap();
            let store = SubjektivStore::open(&workspace, &registration).unwrap();
            assert_eq!(store.subject(&subject.id).unwrap(), Some(subject));
            assert_eq!(
                store.session_attribution("source-session").unwrap(),
                host.attribution
            );
            (manager, workspace, store)
        } else {
            (manager, workspace, store)
        };
        let ranges = sizes
            .iter()
            .enumerate()
            .map(|(order, size)| source_range("source-session", order as u64, *size))
            .collect::<Vec<_>>();
        let journal_id = store
            .accept_journal_sources(&host, "Development journal", &ranges)
            .unwrap();
        Self {
            store: Some(store),
            workspace: Some(workspace),
            manager: Some(manager),
            temp,
            host,
            journal_id,
            ranges,
        }
    }

    fn store(&self) -> &SubjektivStore {
        self.store.as_ref().unwrap()
    }

    fn reopen(&mut self) {
        self.store.take();
        self.workspace.take();
        self.manager.take().unwrap().shutdown().unwrap();
        let manager = FeatureStorage::new(self.temp.path().join("storage"));
        let workspace = manager.workspace("journal-tests").unwrap();
        let registration = SubjektivStore::register(&workspace).unwrap();
        self.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
        self.workspace = Some(workspace);
        self.manager = Some(manager);
    }

    fn state(&self) -> (Journal, Option<JournalEntry>) {
        self.store()
            .journal_resume_state(&self.host, &self.journal_id)
            .unwrap()
    }

    fn generation(
        &self,
        name: &str,
        ranges: Vec<JournalSourceRange>,
    ) -> (JournalGeneration, JournalJobContext) {
        let binding = JobAttemptBinding {
            job_id: name.into(),
            attempt_id: "attempt-1".into(),
        };
        let generation = self
            .store()
            .prepare_journal_generation(&self.host, &self.journal_id, binding.clone(), ranges)
            .unwrap();
        let context =
            JournalJobContext::validated(self.store(), &self.host, &generation.id, binding)
                .unwrap();
        (generation, context)
    }

    fn operation(
        &self,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalOperation {
        JournalOperation {
            request_id: id.into(),
            expected_state: self.state().0.state_token,
            action,
            covered_ranges: ranges,
        }
    }

    fn execute(
        &self,
        ctx: &JournalJobContext,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalReceipt {
        self.store()
            .execute_journal(ctx, self.operation(id, action, ranges))
            .unwrap()
    }

    fn close(&self, generation: &JournalGeneration) {
        self.store()
            .close_journal_generation(&self.host, &generation.id, &generation.job)
            .unwrap();
    }

    fn sources(&self) -> Vec<JournalSourceStatus> {
        self.store()
            .list_journal_sources(&self.host, &self.journal_id, None, MAX_JOURNAL_PAGE)
            .unwrap()
            .items
    }

    fn entries(&self) -> Vec<JournalEntrySummary> {
        self.store()
            .list_journal_entries(&self.host, &self.journal_id, None, MAX_JOURNAL_PAGE)
            .unwrap()
            .items
    }

    fn receipt(&self, id: &str) -> Option<JournalReceipt> {
        self.store()
            .query_journal_operation(&self.host, &self.journal_id, id)
            .unwrap()
    }

    fn sql(&self, sql: &str) -> JournalResult<()> {
        self.store().database.try_transaction(|tx| {
            tx.execute_batch(sql)?;
            Ok(())
        })
    }

    fn execute_and_reopen(
        &mut self,
        ctx: &JournalJobContext,
        id: &str,
        action: JournalAction,
        ranges: Vec<JournalSourceRange>,
    ) -> JournalReceipt {
        let operation = self.operation(id, action, ranges);
        let receipt = self
            .store()
            .execute_journal(ctx, operation.clone())
            .unwrap();
        let state = self.state();
        self.reopen();
        assert_eq!(self.state(), state);
        assert_eq!(self.receipt(id), Some(receipt.clone()));
        assert_eq!(
            self.store().execute_journal(ctx, operation).unwrap(),
            receipt
        );
        receipt
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        self.workspace.take();
        if let Some(manager) = self.manager.take() {
            let _ = manager.shutdown();
        }
    }
}

fn session_host(store: &SubjektivStore, subject: &str, session: &str) -> JournalHostContext {
    let attribution =
        SubjectSessionAttribution::new(subject, "runtime", "worker", session).unwrap();
    store
        .record_session_attribution(attribution.clone())
        .unwrap();
    JournalHostContext::validated_session(store, attribution).unwrap()
}

fn source_range(session: &str, order: u64, total_bytes: u64) -> JournalSourceRange {
    let digest = Sha256::digest(format!(
        "canonical-public-entry:{session}:{order}:{total_bytes}"
    ));
    JournalSourceRange {
        source: JournalSource {
            session_id: session.into(),
            pointer: JournalSourcePointer {
                segment_id: Uuid::now_v7(),
                entry_ref: format!("E{order}"),
            },
            order,
            content_sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            total_bytes,
        },
        bytes: JournalByteRange {
            start: 0,
            end: total_bytes,
        },
    }
}

fn fragment(range: &JournalSourceRange, start: u64, end: u64) -> JournalSourceRange {
    JournalSourceRange {
        source: range.source.clone(),
        bytes: JournalByteRange { start, end },
    }
}

fn open_action(body: &str) -> JournalAction {
    JournalAction::Create {
        title: "Investigation".into(),
        body: body.into(),
        ongoing: "Verify the next boundary".into(),
        state: JournalEntryState::Open,
        source_end: None,
        correction_refs: vec![],
    }
}

fn fixed_action(body: &str, range: &JournalSourceRange, corrections: Vec<String>) -> JournalAction {
    JournalAction::Create {
        title: "Verified outcome".into(),
        body: body.into(),
        ongoing: String::new(),
        state: JournalEntryState::Fixed,
        source_end: Some(range.source.pointer.clone()),
        correction_refs: corrections,
    }
}

fn checkpoint(ids: &[&str]) -> JournalAction {
    JournalAction::Checkpoint {
        record_operation_ids: ids.iter().map(|id| (*id).into()).collect(),
    }
}

#[test]
fn sequential_open_rewrite_fix_and_correction_preserve_fixed_history() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("sequence", fixture.ranges.clone());
    let first = fixture.execute(
        &ctx,
        "create",
        open_action("Initial finding"),
        vec![fixture.ranges[0].clone()],
    );
    let first_id = first.entry_id.unwrap();
    let before = fixture.state();
    let blocked = fixture.operation(
        "append-before-fix",
        fixed_action("Must not append", &fixture.ranges[1], vec![]),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, blocked),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.state(), before);
    assert!(fixture.receipt("append-before-fix").is_none());

    fixture.execute(
        &ctx,
        "rewrite",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Evidence checked".into(),
            body: "Revised finding".into(),
            ongoing: "One final check".into(),
        },
        vec![fixture.ranges[0].clone()],
    );
    let rewritten = fixture.state().1.unwrap();
    assert_eq!(rewritten.id, first_id);
    assert_eq!(rewritten.position, 0);
    assert_eq!(rewritten.body, "Revised finding");
    fixture.execute(
        &ctx,
        "fix",
        JournalAction::Fix {
            entry_id: first_id.clone(),
            source_end: fixture.ranges[0].source.pointer.clone(),
        },
        vec![fixture.ranges[0].clone()],
    );
    assert!(fixture.state().1.is_none());
    let fixed = fixture.entries()[0].clone();
    assert_eq!(fixed.state, JournalEntryState::Fixed);
    assert!(fixed.fixed_at.is_some());
    assert!(fixed.previous_fixed_end.is_none());
    assert!(fixed.source_available);
    assert_eq!(fixture.state().0.process_order, 0);

    let immutable = fixture.operation(
        "rewrite-fixed",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Tampered".into(),
            body: "Tampered".into(),
            ongoing: String::new(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, immutable),
        Err(JournalError::Conflict(_))
    ));
    let repeated_end = fixture.operation(
        "non-advancing-end",
        fixed_action(
            "Cannot reuse the earlier endpoint",
            &fixture.ranges[0],
            vec![],
        ),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, repeated_end),
        Err(JournalError::InvalidInput(_))
    ));
    let missing_correction = fixture.operation(
        "missing-correction",
        fixed_action(
            "No such target",
            &fixture.ranges[1],
            vec!["missing-entry".into()],
        ),
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, missing_correction),
        Err(JournalError::PermissionDenied(_))
    ));
    assert_eq!(fixture.entries(), vec![fixed.clone()]);
    assert!(fixture.receipt("missing-correction").is_none());
    let second = fixture.execute(
        &ctx,
        "correction",
        fixed_action(
            "The earlier finding needs qualification",
            &fixture.ranges[1],
            vec![first_id.clone()],
        ),
        vec![fixture.ranges[1].clone()],
    );
    let entries = fixture.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], fixed);
    assert_eq!(entries[1].id, second.entry_id.unwrap());
    assert_eq!(entries[1].position, 1);
    assert_eq!(entries[1].correction_refs, vec![first_id]);
    assert_eq!(entries[1].previous_fixed_end, fixed.source_end);
    assert_eq!(fixture.state().0.process_order, 0);
    let cp = fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["rewrite", "correction"]),
        fixture.ranges.clone(),
    );
    assert_eq!((cp.process_order, cp.process_byte), (2, 0));
}

#[test]
fn same_attempt_keeps_immutable_input_while_next_attempt_carries_current_state() {
    let fixture = Fixture::new();
    let selected = vec![fixture.ranges[0].clone()];
    let (generation, ctx) = fixture.generation("first-job", selected.clone());
    fixture.execute(
        &ctx,
        "create",
        open_action("Pending investigation"),
        selected.clone(),
    );
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "Investigating receipt recovery".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["create"]),
        selected.clone(),
    );
    let current = fixture.state();
    let new_source = source_range("source-session", 2, 400);
    assert_eq!(
        fixture
            .store()
            .accept_journal_sources(
                &fixture.host,
                "Ignored replacement title",
                &[new_source.clone()]
            )
            .unwrap(),
        fixture.journal_id
    );
    assert_eq!(fixture.state(), current);
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
    assert_eq!(
        fixture
            .store()
            .prepare_journal_generation(
                &fixture.host,
                &fixture.journal_id,
                generation.job.clone(),
                selected
            )
            .unwrap(),
        generation
    );
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            generation.job.clone(),
            fixture.ranges.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    let changed_input = fixture.operation(
        "outside-snapshot",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[1].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed_input),
        Err(JournalError::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            JobAttemptBinding {
                job_id: "another-job".into(),
                attempt_id: "attempt-1".into()
            },
            vec![fixture.ranges[1].clone()]
        ),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    let (next, _) = fixture.generation("next-job", vec![fixture.ranges[1].clone(), new_source]);
    assert_eq!(next.initial_journal, current.0);
    assert_eq!(next.initial_open, current.1);
    assert!(next.previous_fixed_end.is_none());
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
}

#[test]
fn lost_response_reopen_returns_exact_receipt_and_rejects_changed_retry() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("response-loss", fixture.ranges.clone());
    let operation = fixture.operation(
        "stable-request",
        open_action("Saved before response loss"),
        vec![fixture.ranges[0].clone()],
    );
    // The caller discards the successful response and recovers from the durable receipt.
    fixture
        .store()
        .execute_journal(&ctx, operation.clone())
        .unwrap();
    fixture.reopen();
    let recovered = fixture.receipt("stable-request").unwrap();
    let state = fixture.state();
    assert_eq!(
        fixture
            .store()
            .execute_journal(&ctx, operation.clone())
            .unwrap(),
        recovered
    );
    assert_eq!(fixture.state(), state);
    assert_eq!(fixture.entries().len(), 1);
    let mut changed = operation.clone();
    changed.action = open_action("Different body using the same request ID");
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    let mut changed = operation.clone();
    changed.expected_state = recovered.state_token.clone();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    let mut changed = operation.clone();
    changed.covered_ranges = vec![fixture.ranges[1].clone()];
    assert!(matches!(
        fixture.store().execute_journal(&ctx, changed),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    fixture.reopen();
    assert_eq!(
        fixture
            .store()
            .execute_journal(&ctx, operation.clone())
            .unwrap(),
        recovered
    );
    let (_, replacement) = fixture.generation("replacement-job", fixture.ranges.clone());
    assert!(matches!(
        fixture.store().execute_journal(&replacement, operation),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.receipt("stable-request"), Some(recovered));
    assert_eq!(fixture.state(), state);
}

#[test]
fn cancellation_preserves_open_and_receipts_but_fences_late_writes_and_checkpoint() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("cancelled", fixture.ranges.clone());
    let saved = fixture.execute(
        &ctx,
        "create",
        open_action("Saved but not reflected"),
        fixture.ranges.clone(),
    );
    let late = fixture.operation(
        "late-checkpoint",
        checkpoint(&["create"]),
        fixture.ranges.clone(),
    );
    let before = fixture.state();
    fixture.close(&generation);
    fixture.close(&generation);
    fixture.reopen();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late),
        Err(JournalError::Conflict(_))
    ));
    let late_rewrite = fixture.operation(
        "late-rewrite",
        JournalAction::Rewrite {
            entry_id: saved.entry_id.clone().unwrap(),
            title: "Late".into(),
            body: "Late".into(),
            ongoing: String::new(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late_rewrite),
        Err(JournalError::Conflict(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.receipt("create"), Some(saved));
    assert!(fixture.receipt("late-checkpoint").is_none());
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == 0)
    );
}

#[test]
fn stale_cas_is_rejected_even_after_position_returns_to_same_text() {
    let mut fixture = Fixture::new();
    let (_, ctx) = fixture.generation("causal-state", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "position-a",
        JournalAction::UpdatePosition {
            current_position: "A".into(),
        },
        vec![],
    );
    let at_a = fixture.state().0;
    let stale = fixture.operation(
        "stale-create",
        open_action("Must not apply against a later A"),
        vec![],
    );
    fixture.execute(
        &ctx,
        "position-b",
        JournalAction::UpdatePosition {
            current_position: "B".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "position-a-again",
        JournalAction::UpdatePosition {
            current_position: "A".into(),
        },
        vec![],
    );
    fixture.reopen();
    let current = fixture.state().0;
    assert_eq!(current.current_position, at_a.current_position);
    assert_ne!(current.state_token, at_a.state_token);
    assert!(matches!(
        fixture.store().execute_journal(&ctx, stale),
        Err(JournalError::Conflict(_))
    ));
    assert!(fixture.entries().is_empty());
    assert!(fixture.receipt("stale-create").is_none());
    assert_eq!(fixture.state().0, current);
}

#[test]
fn foreign_scope_subject_session_job_and_journal_cannot_mutate_or_read_receipts() {
    let fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("authorized-job", fixture.ranges.clone());
    let created = fixture.execute(&ctx, "create", open_action("Subject-owned record"), vec![]);
    let entry_id = created.entry_id.unwrap();
    let before = fixture.state();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("other").unwrap())
        .unwrap();
    let foreign_host = session_host(fixture.store(), &foreign_subject.id, "foreign-session");
    assert!(matches!(
        fixture
            .store()
            .read_journal(&foreign_host, &fixture.journal_id),
        Err(JournalError::NotFound(_))
    ));
    assert!(matches!(
        fixture
            .store()
            .query_journal_operation(&foreign_host, &fixture.journal_id, "create"),
        Err(JournalError::NotFound(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &foreign_host,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::NotFound(_))
    ));
    let other_session = session_host(fixture.store(), &fixture.host.subject_id, "other-session");
    assert!(matches!(
        fixture
            .store()
            .journal_resume_state(&other_session, &fixture.journal_id),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &other_session,
            &generation.id,
            generation.job.clone()
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    let wrong_attempt = JobAttemptBinding {
        job_id: generation.job.job_id.clone(),
        attempt_id: "attempt-2".into(),
    };
    assert!(matches!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            wrong_attempt.clone()
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        fixture
            .store()
            .close_journal_generation(&fixture.host, &generation.id, &wrong_attempt),
        Err(JournalError::PermissionDenied(_))
    ));

    let other_manager = FeatureStorage::new(fixture.temp.path().join("foreign-storage"));
    let other_workspace = other_manager.workspace("foreign-scope").unwrap();
    let registration = SubjektivStore::register(&other_workspace).unwrap();
    let other_store = SubjektivStore::open(&other_workspace, &registration).unwrap();
    assert!(matches!(
        other_store.read_journal(&fixture.host, &fixture.journal_id),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(matches!(
        other_store.execute_journal(
            &ctx,
            fixture.operation("wrong-scope", open_action("No"), vec![])
        ),
        Err(JournalError::PermissionDenied(_))
    ));
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    assert!(matches!(
        fixture
            .store()
            .accept_journal_sources(&reader, "No authority", &fixture.ranges),
        Err(JournalError::PermissionDenied(_))
    ));
    let mut forged_attribution = fixture.host.attribution.clone().unwrap();
    forged_attribution.worker_id = "different-worker".into();
    assert!(matches!(
        JournalHostContext::validated_session(fixture.store(), forged_attribution),
        Err(JournalError::PermissionDenied(_))
    ));

    let other_range = source_range("other-session", 0, 256);
    let other_id = fixture
        .store()
        .accept_journal_sources(&other_session, "Another journal", &[other_range.clone()])
        .unwrap();
    let other_generation = fixture
        .store()
        .prepare_journal_generation(
            &other_session,
            &other_id,
            JobAttemptBinding {
                job_id: "other-journal-job".into(),
                attempt_id: "attempt-1".into(),
            },
            vec![other_range],
        )
        .unwrap();
    let other_ctx = JournalJobContext::validated(
        fixture.store(),
        &other_session,
        &other_generation.id,
        other_generation.job,
    )
    .unwrap();
    let other_state = fixture
        .store()
        .journal_resume_state(&other_session, &other_id)
        .unwrap();
    let cross_journal = JournalOperation {
        request_id: "cross-journal-rewrite".into(),
        expected_state: other_state.0.state_token.clone(),
        action: JournalAction::Rewrite {
            entry_id,
            title: "No".into(),
            body: "No".into(),
            ongoing: "No".into(),
        },
        covered_ranges: vec![],
    };
    assert!(matches!(
        fixture.store().execute_journal(&other_ctx, cross_journal),
        Err(JournalError::NotFound(_))
    ));
    assert_eq!(
        fixture
            .store()
            .journal_resume_state(&other_session, &other_id)
            .unwrap(),
        other_state
    );
    assert_eq!(fixture.state(), before);
}

#[test]
fn corrections_allow_same_subject_other_journal_but_reject_foreign_subject_atomically() {
    let fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("first", fixture.ranges.clone());
    let target = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Original record", &fixture.ranges[0], vec![]),
            vec![],
        )
        .entry_id
        .unwrap();
    fixture.close(&generation);
    let same_subject = session_host(fixture.store(), &fixture.host.subject_id, "later-session");
    let later_source = source_range("later-session", 0, 128);
    let later_id = fixture
        .store()
        .accept_journal_sources(&same_subject, "Later journal", &[later_source.clone()])
        .unwrap();
    let later_generation = fixture
        .store()
        .prepare_journal_generation(
            &same_subject,
            &later_id,
            JobAttemptBinding {
                job_id: "later".into(),
                attempt_id: "one".into(),
            },
            vec![later_source.clone()],
        )
        .unwrap();
    let later_ctx = JournalJobContext::validated(
        fixture.store(),
        &same_subject,
        &later_generation.id,
        later_generation.job,
    )
    .unwrap();
    let operation = JournalOperation {
        request_id: "cross-journal-correction".into(),
        expected_state: later_generation.initial_journal.state_token,
        action: fixed_action(
            "Qualification from a later Session",
            &later_source,
            vec![target.clone()],
        ),
        covered_ranges: vec![later_source],
    };
    fixture
        .store()
        .execute_journal(&later_ctx, operation)
        .unwrap();
    let later_entries = fixture
        .store()
        .list_journal_entries(&same_subject, &later_id, None, 1)
        .unwrap();
    assert_eq!(later_entries.items[0].correction_refs, vec![target.clone()]);

    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(fixture.store(), &foreign_subject.id, "foreign-session");
    let foreign_source = source_range("foreign-session", 0, 128);
    let foreign_id = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Foreign", &[foreign_source.clone()])
        .unwrap();
    let foreign_generation = fixture
        .store()
        .prepare_journal_generation(
            &foreign_host,
            &foreign_id,
            JobAttemptBinding {
                job_id: "foreign".into(),
                attempt_id: "one".into(),
            },
            vec![foreign_source.clone()],
        )
        .unwrap();
    let foreign_ctx = JournalJobContext::validated(
        fixture.store(),
        &foreign_host,
        &foreign_generation.id,
        foreign_generation.job,
    )
    .unwrap();
    let rejected = JournalOperation {
        request_id: "foreign-correction".into(),
        expected_state: foreign_generation.initial_journal.state_token.clone(),
        action: fixed_action("Forbidden correction", &foreign_source, vec![target]),
        covered_ranges: vec![foreign_source],
    };
    assert!(matches!(
        fixture.store().execute_journal(&foreign_ctx, rejected),
        Err(JournalError::PermissionDenied(_))
    ));
    assert!(
        fixture
            .store()
            .list_journal_entries(&foreign_host, &foreign_id, None, 1)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        fixture
            .store()
            .query_journal_operation(&foreign_host, &foreign_id, "foreign-correction")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture
            .store()
            .read_journal(&foreign_host, &foreign_id)
            .unwrap()
            .state_token,
        foreign_generation.initial_journal.state_token
    );
}

#[test]
fn retired_subject_keeps_reader_access_but_rejects_existing_write_context() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("retired-subject", fixture.ranges.clone());
    let receipt = fixture.execute(&ctx, "saved", open_action("Historical record"), vec![]);
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let late = fixture.operation(
        "late",
        JournalAction::UpdatePosition {
            current_position: "No longer authorized".into(),
        },
        vec![],
    );
    fixture
        .store()
        .retire_subject(&fixture.host.subject_id)
        .unwrap();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, late),
        Err(JournalError::PermissionDenied(_))
    ));
    assert_eq!(
        fixture
            .store()
            .query_journal_operation(&reader, &fixture.journal_id, "saved")
            .unwrap(),
        Some(receipt)
    );
    assert_eq!(
        fixture
            .store()
            .list_journal_entries(&reader, &fixture.journal_id, None, 1)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn concurrent_creates_from_independent_connections_save_exactly_one_open_and_receipt() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("concurrent", fixture.ranges.clone());
    let manager = FeatureStorage::new(fixture.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    let second_store = SubjektivStore::open(&workspace, &registration).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let first_op = fixture.operation(
        "writer-a",
        open_action("A"),
        vec![fixture.ranges[0].clone()],
    );
    let second_op = fixture.operation(
        "writer-b",
        open_action("B"),
        vec![fixture.ranges[0].clone()],
    );
    let spawn = |store: SubjektivStore, operation: JournalOperation| {
        let barrier = barrier.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            barrier.wait();
            store.execute_journal(&ctx, operation)
        })
    };
    let first = spawn(fixture.store().clone(), first_op);
    let second = spawn(second_store, second_op);
    barrier.wait();
    let results = [first.join().unwrap(), second.join().unwrap()];
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(JournalError::Conflict(_))))
            .count(),
        1
    );
    let winner = results.into_iter().find_map(Result::ok).unwrap();
    assert_eq!(fixture.entries().len(), 1);
    assert_eq!(fixture.state().1.unwrap().id, winner.entry_id.unwrap());
    let receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(receipts.items.len(), 1);
    assert_eq!(receipts.items[0].request_id, winner.request_id);
    assert_eq!(fixture.state().0.process_order, 0);
    drop(workspace);
    manager.shutdown().unwrap();
}

#[test]
fn checkpoint_requires_saved_record_correspondence_and_never_position_only_evidence() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("checkpoint-evidence", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "A summary is not a receipt for source reflection".into(),
        },
        vec![],
    );
    let before = fixture.state();
    let position_only = fixture.operation(
        "position-checkpoint",
        checkpoint(&["position"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, position_only),
        Err(JournalError::InvalidInput(_))
    ));
    let missing = fixture.operation(
        "missing-checkpoint",
        checkpoint(&["missing-record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, missing),
        Err(JournalError::NotFound(_))
    ));
    let false_position = fixture.operation(
        "false-position",
        JournalAction::UpdatePosition {
            current_position: "Pretend processed".into(),
        },
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, false_position),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), before);
    fixture.execute(
        &ctx,
        "record",
        open_action("Only the first source is represented"),
        vec![fixture.ranges[0].clone()],
    );
    let after_record = fixture.state();
    let excess = fixture.operation(
        "excess-checkpoint",
        checkpoint(&["record"]),
        fixture.ranges.clone(),
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, excess),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), after_record);
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == 0)
    );
    fixture.execute(
        &ctx,
        "checked-unchanged",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[1].clone()],
    );
    let receipt = fixture.execute(
        &ctx,
        "valid-checkpoint",
        checkpoint(&["record", "checked-unchanged"]),
        fixture.ranges.clone(),
    );
    assert_eq!((receipt.process_order, receipt.process_byte), (2, 0));
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == source.source.total_bytes)
    );
    let before = fixture.state();
    let replay_prefix = fixture.operation(
        "rewind-checkpoint",
        checkpoint(&["record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, replay_prefix),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.state(), before);
}

#[test]
fn huge_source_partial_generations_checkpoint_only_read_prefix_and_fix_only_at_entry_end() {
    let mut fixture = Fixture::with_sizes(&[8 * 1024 * 1024]);
    let source = fixture.ranges[0].clone();
    let head = fragment(&source, 0, 16 * 1024);
    let rest = fragment(&source, head.bytes.end, source.source.total_bytes);
    let (first_generation, first_ctx) = fixture.generation("huge-head", vec![head.clone()]);
    let created = fixture.execute(
        &first_ctx,
        "partial-record",
        open_action("Only the initial fragment was checked"),
        vec![head.clone()],
    );
    let unread_end = fixture.operation(
        "premature-fix",
        JournalAction::Fix {
            entry_id: created.entry_id.clone().unwrap(),
            source_end: source.source.pointer.clone(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().execute_journal(&first_ctx, unread_end),
        Err(JournalError::InvalidInput(_))
    ));
    let cp = fixture.execute(
        &first_ctx,
        "partial-checkpoint",
        checkpoint(&["partial-record"]),
        vec![head.clone()],
    );
    assert_eq!((cp.process_order, cp.process_byte), (0, head.bytes.end));
    assert_eq!(fixture.sources()[0].reflected_bytes, head.bytes.end);
    fixture.close(&first_generation);
    fixture.reopen();
    let gap = fragment(&source, head.bytes.end + 1, source.source.total_bytes);
    let binding = JobAttemptBinding {
        job_id: "gap-job".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![gap]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let overlap = fragment(&source, head.bytes.end - 1, source.source.total_bytes);
    let binding = JobAttemptBinding {
        job_id: "overlap-job".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![overlap]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let (next, next_ctx) = fixture.generation("huge-tail", vec![rest.clone()]);
    assert_eq!(next.initial_journal.process_byte, head.bytes.end);
    assert_eq!(
        next.initial_open.as_ref().unwrap().id,
        created.entry_id.clone().unwrap()
    );
    fixture.execute(
        &next_ctx,
        "tail-record",
        JournalAction::Rewrite {
            entry_id: created.entry_id.clone().unwrap(),
            title: "Complete input checked".into(),
            body: "A bounded record of a huge source".into(),
            ongoing: "Ready to fix".into(),
        },
        vec![rest.clone()],
    );
    fixture.execute(
        &next_ctx,
        "fix",
        JournalAction::Fix {
            entry_id: created.entry_id.unwrap(),
            source_end: source.source.pointer.clone(),
        },
        vec![],
    );
    assert_eq!(fixture.state().0.process_byte, head.bytes.end);
    let cp = fixture.execute(
        &next_ctx,
        "tail-checkpoint",
        checkpoint(&["tail-record"]),
        vec![rest],
    );
    assert_eq!((cp.process_order, cp.process_byte), (1, 0));
    assert_eq!(
        fixture.sources()[0].reflected_bytes,
        source.source.total_bytes
    );
}

#[test]
fn accepted_overlap_merges_but_gaps_and_overlapping_selected_prefixes_cannot_skip_input() {
    let fixture = Fixture::new();
    let ranges = vec![
        fragment(&fixture.ranges[0], 0, 150),
        fragment(&fixture.ranges[0], 100, 256),
    ];
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "Same journal", &ranges)
        .unwrap();
    assert_eq!(
        fixture.sources()[0].accepted,
        vec![JournalByteRange { start: 0, end: 256 }]
    );
    let binding = JobAttemptBinding {
        job_id: "overlapping-selection".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            ranges
        ),
        Err(JournalError::InvalidInput(_))
    ));
    let binding = JobAttemptBinding {
        job_id: "missing-order-zero".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            vec![fixture.ranges[1].clone()]
        ),
        Err(JournalError::InvalidInput(_))
    ));

    let host = session_host(fixture.store(), &fixture.host.subject_id, "gapped-session");
    let full = source_range("gapped-session", 0, 1000);
    let head = fragment(&full, 0, 100);
    let tail = fragment(&full, 200, 1000);
    let id = fixture
        .store()
        .accept_journal_sources(&host, "Gapped admission", &[head.clone(), tail.clone()])
        .unwrap();
    let binding = JobAttemptBinding {
        job_id: "gapped-source".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture
            .store()
            .prepare_journal_generation(&host, &id, binding.clone(), vec![full.clone()]),
        Err(JournalError::InvalidInput(_))
    ));
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &host,
            &id,
            binding.clone(),
            vec![head.clone(), tail.clone()]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    fixture
        .store()
        .accept_journal_sources(&host, "Fill only the gap", &[fragment(&full, 100, 200)])
        .unwrap();
    let generation = fixture
        .store()
        .prepare_journal_generation(&host, &id, binding, vec![full])
        .unwrap();
    assert_eq!(generation.initial_journal.process_byte, 0);
    assert!(fixture.entries().is_empty());
}

#[test]
fn canonical_compaction_identity_is_idempotent_and_rewind_replacement_rolls_back_admission() {
    let fixture = Fixture::new();
    let before = fixture.state();
    let sources = fixture.sources();
    // A retained compaction copy supplies the original canonical identity, not a new source.
    assert_eq!(
        fixture
            .store()
            .accept_journal_sources(&fixture.host, "Compacted copy", &fixture.ranges)
            .unwrap(),
        fixture.journal_id
    );
    assert_eq!(fixture.sources(), sources);
    assert_eq!(fixture.state(), before);
    let original = fixture.ranges[0].clone();
    let mut replacements = Vec::new();
    let mut changed = original.clone();
    changed.source.content_sha256 = "f".repeat(64);
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.pointer.segment_id = Uuid::now_v7();
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.pointer.entry_ref = "Ereplacement".into();
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.total_bytes += 1;
    replacements.push(changed);
    let mut changed = original.clone();
    changed.source.order = 2;
    replacements.push(changed);
    for replacement in replacements {
        let new_source = source_range("source-session", 3, 64);
        let result = fixture.store().accept_journal_sources(
            &fixture.host,
            "Must roll back both",
            &[new_source, replacement],
        );
        assert!(
            matches!(result, Err(JournalError::Conflict(_))),
            "{result:?}"
        );
        assert_eq!(fixture.sources(), sources);
        assert_eq!(fixture.state(), before);
    }
    let wrong_session = source_range("another-session", 2, 100);
    assert!(matches!(
        fixture
            .store()
            .accept_journal_sources(&fixture.host, "No", &[wrong_session]),
        Err(JournalError::PermissionDenied(_))
    ));
    let mut derived_summary = source_range("source-session", 2, 100);
    derived_summary.source.pointer.entry_ref = "summary-2".into();
    assert!(matches!(
        fixture.store().accept_journal_sources(
            &fixture.host,
            "No derived source",
            &[derived_summary]
        ),
        Err(JournalError::InvalidInput(_))
    ));
    assert_eq!(fixture.sources(), sources);
}

#[test]
fn unavailable_source_preserves_records_and_receipts_without_advancing_processing() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("retention", fixture.ranges.clone());
    let record = fixture.execute(
        &ctx,
        "record",
        fixed_action("Retained outcome", &fixture.ranges[0], vec![]),
        vec![fixture.ranges[0].clone()],
    );
    let before = fixture.state();
    fixture
        .store()
        .mark_journal_sources_unavailable(
            &fixture.host,
            &fixture.journal_id,
            &[fixture.ranges[0].clone()],
        )
        .unwrap();
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.receipt("record"), Some(record));
    assert_eq!(
        fixture.entries()[0].source_end,
        Some(fixture.ranges[0].source.pointer.clone())
    );
    assert!(!fixture.entries()[0].source_available);
    assert_eq!(fixture.sources()[0].reflected_bytes, 0);
    let unread = fixture.operation(
        "unavailable-checkpoint",
        checkpoint(&["record"]),
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, unread),
        Err(JournalError::Conflict(_))
    ));
    let unavailable = fixture.operation(
        "unavailable-confirmation",
        JournalAction::ConfirmUnchanged,
        vec![fixture.ranges[0].clone()],
    );
    assert!(matches!(
        fixture.store().execute_journal(&ctx, unavailable),
        Err(JournalError::Conflict(_))
    ));
    fixture.close(&generation);
    let binding = JobAttemptBinding {
        job_id: "retention-replacement".into(),
        attempt_id: "one".into(),
    };
    assert!(matches!(
        fixture.store().prepare_journal_generation(
            &fixture.host,
            &fixture.journal_id,
            binding,
            fixture.ranges.clone()
        ),
        Err(JournalError::Conflict(_))
    ));
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let page = fixture
        .store()
        .read_journal_text(
            &reader,
            &fixture.journal_id,
            Some(fixture.entries()[0].id.as_str()),
            JournalTextField::Body,
            None,
            0,
            100,
        )
        .unwrap();
    assert_eq!(page.text, "Retained outcome");
    assert!(fixture.receipt("unavailable-checkpoint").is_none());
}

#[test]
fn unicode_and_escaped_long_text_pages_reconstruct_exactly_and_reject_stale_continuation() {
    let fixture = Fixture::new();
    let (_, ctx) = fixture.generation("text-pages", fixture.ranges.clone());
    let body = "界🙂\"\\\n\t".repeat(12_000);
    let ongoing = "次の確認🙂\n".repeat(3000);
    let action = JournalAction::Create {
        title: "Unicode".into(),
        body: body.clone(),
        ongoing: ongoing.clone(),
        state: JournalEntryState::Open,
        source_end: None,
        correction_refs: vec![],
    };
    let id = fixture
        .execute(&ctx, "create", action, vec![])
        .entry_id
        .unwrap();
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: body.clone(),
        },
        vec![],
    );
    for (field, entry, expected) in [
        (JournalTextField::Body, Some(id.as_str()), body.as_str()),
        (
            JournalTextField::Ongoing,
            Some(id.as_str()),
            ongoing.as_str(),
        ),
        (JournalTextField::CurrentPosition, None, body.as_str()),
    ] {
        let mut offset = 0;
        let mut token = None::<String>;
        let mut reconstructed = String::new();
        loop {
            let page = fixture
                .store()
                .read_journal_text(
                    &fixture.host,
                    &fixture.journal_id,
                    entry,
                    field,
                    token.as_deref(),
                    offset,
                    257,
                )
                .unwrap();
            assert!(page.text.len() <= 257);
            assert!(!page.text.is_empty());
            assert!(serde_json::to_vec(&page).unwrap().len() < MAX_JOURNAL_READ_BYTES);
            if let Some(token) = &token {
                assert_eq!(&page.content_token, token);
            }
            reconstructed.push_str(&page.text);
            token = Some(page.content_token);
            match page.next_byte {
                Some(next) => {
                    assert!(next > offset);
                    assert!(expected.is_char_boundary(next));
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(reconstructed, expected);
    }
    let first = fixture
        .store()
        .read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            None,
            0,
            257,
        )
        .unwrap();
    let position = fixture
        .store()
        .read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            None,
            JournalTextField::CurrentPosition,
            None,
            0,
            257,
        )
        .unwrap();
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            None,
            first.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            Some(&first.content_token),
            1,
            257
        ),
        Err(JournalError::InvalidInput(_))
    ));
    fixture.execute(
        &ctx,
        "rewrite",
        JournalAction::Rewrite {
            entry_id: id.clone(),
            title: "Changed".into(),
            body: "Different body".into(),
            ongoing: "Changed ongoing".into(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            Some(&id),
            JournalTextField::Body,
            Some(&first.content_token),
            first.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    fixture.execute(
        &ctx,
        "position-rewrite",
        JournalAction::UpdatePosition {
            current_position: "Different position".into(),
        },
        vec![],
    );
    assert!(matches!(
        fixture.store().read_journal_text(
            &fixture.host,
            &fixture.journal_id,
            None,
            JournalTextField::CurrentPosition,
            Some(&position.content_token),
            position.next_byte.unwrap(),
            257
        ),
        Err(JournalError::Conflict(_))
    ));
    for invalid_limit in [0, 3, MAX_JOURNAL_READ_BYTES + 1] {
        assert!(matches!(
            fixture.store().read_journal_text(
                &fixture.host,
                &fixture.journal_id,
                Some(&id),
                JournalTextField::Body,
                None,
                0,
                invalid_limit
            ),
            Err(JournalError::InvalidInput(_))
        ));
    }
    let summary = &fixture.entries()[0];
    let json = serde_json::to_value(summary).unwrap();
    assert!(json.get("body").is_none());
    assert!(json.get("ongoing").is_none());
}

#[test]
fn metadata_and_operation_keysets_do_not_skip_appends_or_sort_by_request_id() {
    let fixture = Fixture::with_sizes(&[100, 100, 100]);
    let (_, ctx) = fixture.generation("metadata-pages", fixture.ranges.clone());
    fixture.execute(
        &ctx,
        "z-request",
        fixed_action("First", &fixture.ranges[0], vec![]),
        vec![],
    );
    fixture.execute(
        &ctx,
        "a-request",
        fixed_action("Second", &fixture.ranges[1], vec![]),
        vec![],
    );
    let first = fixture
        .store()
        .list_journal_entries(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    let receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    assert_eq!(first.items[0].position, 0);
    assert_eq!(receipts.items[0].request_id, "z-request");
    fixture.execute(
        &ctx,
        "m-request",
        fixed_action("Appended between pages", &fixture.ranges[2], vec![]),
        vec![],
    );
    let second = fixture
        .store()
        .list_journal_entries(
            &fixture.host,
            &fixture.journal_id,
            Some(first.next.unwrap().parse().unwrap()),
            1,
        )
        .unwrap();
    let third = fixture
        .store()
        .list_journal_entries(
            &fixture.host,
            &fixture.journal_id,
            Some(second.next.unwrap().parse().unwrap()),
            1,
        )
        .unwrap();
    assert_eq!(second.items[0].position, 1);
    assert_eq!(third.items[0].position, 2);
    assert!(third.next.is_none());
    let second_receipt = fixture
        .store()
        .list_journal_operations(
            &fixture.host,
            &fixture.journal_id,
            receipts.next.as_deref(),
            1,
        )
        .unwrap();
    let third_receipt = fixture
        .store()
        .list_journal_operations(
            &fixture.host,
            &fixture.journal_id,
            second_receipt.next.as_deref(),
            1,
        )
        .unwrap();
    assert_eq!(second_receipt.items[0].request_id, "a-request");
    assert_eq!(third_receipt.items[0].request_id, "m-request");
    assert!(third_receipt.next.is_none());
    let source_page = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 1)
        .unwrap();
    let next_source = fixture
        .store()
        .list_journal_sources(
            &fixture.host,
            &fixture.journal_id,
            source_page.next.as_deref(),
            2,
        )
        .unwrap();
    assert_eq!(
        next_source
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    for limit in [0, MAX_JOURNAL_PAGE + 1] {
        assert!(matches!(
            fixture
                .store()
                .list_journal_entries(&fixture.host, &fixture.journal_id, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture.store().list_journal_operations(
                &fixture.host,
                &fixture.journal_id,
                None,
                limit
            ),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture
                .store()
                .list_journal_sources(&fixture.host, &fixture.journal_id, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
        assert!(matches!(
            fixture.store().list_journals(&fixture.host, None, limit),
            Err(JournalError::InvalidInput(_))
        ));
    }
}

#[test]
fn journal_metadata_pages_are_subject_scoped_and_reader_context_needs_no_session() {
    let fixture = Fixture::new();
    let later_host = session_host(
        fixture.store(),
        &fixture.host.subject_id,
        "metadata-later-session",
    );
    let later_source = source_range("metadata-later-session", 0, 100);
    let later_id = fixture
        .store()
        .accept_journal_sources(&later_host, "Later", &[later_source])
        .unwrap();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(
        fixture.store(),
        &foreign_subject.id,
        "metadata-foreign-session",
    );
    let foreign_source = source_range("metadata-foreign-session", 0, 100);
    let foreign_id = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Not visible", &[foreign_source])
        .unwrap();
    let reader =
        JournalHostContext::validated_reader(fixture.store(), &fixture.host.subject_id).unwrap();
    let first = fixture.store().list_journals(&reader, None, 1).unwrap();
    assert_eq!(first.items.len(), 1);
    let second = fixture
        .store()
        .list_journals(&reader, first.next.as_deref(), 1)
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert!(second.next.is_none());
    let mut expected = vec![fixture.journal_id.clone(), later_id];
    expected.sort();
    assert_eq!(
        vec![first.items[0].id.clone(), second.items[0].id.clone()],
        expected
    );
    assert!(
        first
            .items
            .iter()
            .chain(&second.items)
            .all(|item| item.subject_id == fixture.host.subject_id)
    );
    assert!(matches!(
        fixture.store().read_journal(&reader, &foreign_id),
        Err(JournalError::NotFound(_))
    ));
    let json = serde_json::to_value(&first.items[0]).unwrap();
    assert!(json.get("current_position").is_none());
    assert!(json.get("entries").is_none());
    assert!(json.get("body").is_none());
}

#[test]
fn backup_restore_preserves_generation_records_corrections_sources_position_and_receipts() {
    let mut fixture = Fixture::new();
    let (generation, ctx) = fixture.generation("backup", fixture.ranges.clone());
    let first = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Durable outcome", &fixture.ranges[0], vec![]),
            vec![fixture.ranges[0].clone()],
        )
        .entry_id
        .unwrap();
    let mut correction = open_action("Further qualification is ongoing");
    if let JournalAction::Create {
        correction_refs, ..
    } = &mut correction
    {
        correction_refs.push(first);
    }
    fixture.execute(
        &ctx,
        "correction",
        correction,
        vec![fixture.ranges[1].clone()],
    );
    fixture.execute(
        &ctx,
        "position",
        JournalAction::UpdatePosition {
            current_position: "Correction awaits verification".into(),
        },
        vec![],
    );
    fixture.execute(
        &ctx,
        "checkpoint",
        checkpoint(&["fixed"]),
        vec![fixture.ranges[0].clone()],
    );
    fixture
        .store()
        .mark_journal_sources_unavailable(
            &fixture.host,
            &fixture.journal_id,
            &[fragment(&fixture.ranges[0], 0, 10)],
        )
        .unwrap();
    let expected_state = fixture.state();
    let expected_entries = fixture.entries();
    let expected_sources = fixture.sources();
    let expected_receipts = fixture
        .store()
        .list_journal_operations(&fixture.host, &fixture.journal_id, None, 32)
        .unwrap();
    let snapshot = fixture.temp.path().join("snapshot");
    fixture
        .workspace
        .as_ref()
        .unwrap()
        .backup(&snapshot)
        .unwrap();
    fixture.execute(
        &ctx,
        "after-backup",
        JournalAction::UpdatePosition {
            current_position: "Not part of snapshot".into(),
        },
        vec![],
    );
    fixture.close(&generation);
    fixture.workspace.as_ref().unwrap().delete().unwrap();
    fixture.store.take();
    fixture.workspace.take();
    fixture.manager.take().unwrap().shutdown().unwrap();
    let manager = FeatureStorage::new(fixture.temp.path().join("storage"));
    let workspace = manager.workspace("journal-tests").unwrap();
    workspace.restore(&snapshot).unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    fixture.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
    fixture.workspace = Some(workspace);
    fixture.manager = Some(manager);
    assert_eq!(fixture.state(), expected_state);
    assert_eq!(fixture.entries(), expected_entries);
    assert_eq!(fixture.sources(), expected_sources);
    assert_eq!(
        fixture
            .store()
            .read_journal_generation(&fixture.host, &generation.id)
            .unwrap(),
        generation
    );
    assert_eq!(
        fixture
            .store()
            .list_journal_operations(&fixture.host, &fixture.journal_id, None, 32)
            .unwrap(),
        expected_receipts
    );
    assert!(fixture.receipt("after-backup").is_none());
    assert!(
        JournalJobContext::validated(
            fixture.store(),
            &fixture.host,
            &generation.id,
            generation.job
        )
        .is_ok()
    );
    fixture.reopen();
    assert_eq!(fixture.state(), expected_state);
    assert_eq!(fixture.entries(), expected_entries);
}

#[test]
fn migrated_schema_rejects_null_fixed_order_open_tail_mutation_and_receipt_tampering() {
    let mut fixture = Fixture::with_schema(&[256, 256], true);
    let (_, ctx) = fixture.generation("schema", fixture.ranges.clone());
    let fixed_id = fixture
        .execute(
            &ctx,
            "fixed",
            fixed_action("Immutable record", &fixture.ranges[0], vec![]),
            vec![],
        )
        .entry_id
        .unwrap();
    let open_id = fixture
        .execute(&ctx, "open", open_action("Mutable tail"), vec![])
        .entry_id
        .unwrap();
    let foreign_subject = fixture
        .store()
        .create_subject(SubjectRole::new("foreign").unwrap())
        .unwrap();
    let foreign_host = session_host(
        fixture.store(),
        &foreign_subject.id,
        "schema-foreign-session",
    );
    let foreign_source = source_range("schema-foreign-session", 0, 100);
    let foreign_journal = fixture
        .store()
        .accept_journal_sources(&foreign_host, "Foreign journal", &[foreign_source])
        .unwrap();
    let before = fixture.state();
    let entries = fixture.entries();
    let journal = &fixture.journal_id;
    let subject = &fixture.host.subject_id;
    let invalid_sql = [
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES('foreign-owner','{subject}','{foreign_journal}',0,'open','','','','now','now')"
        ),
        format!("UPDATE journal_entries SET body='changed' WHERE id='{fixed_id}'"),
        format!("UPDATE journal_entries SET position=10 WHERE id='{open_id}'"),
        format!("UPDATE journal_entries SET subject_id='missing-subject' WHERE id='{open_id}'"),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=NULL, fixed_at='now', ongoing='' WHERE id='{open_id}'"
        ),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=-1, fixed_at='now', ongoing='' WHERE id='{open_id}'"
        ),
        format!(
            "UPDATE journal_entries SET state='fixed', source_end_json='{{}}', source_order=1, fixed_at='now', ongoing='still open' WHERE id='{open_id}'"
        ),
        format!("UPDATE journal_entries SET source_end_json='{{}}' WHERE id='{open_id}'"),
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,created_at,updated_at) VALUES('second-open','{subject}','{journal}',2,'open','','','','now','now')"
        ),
        format!(
            "INSERT INTO journal_entries(id,subject_id,journal_id,position,state,title,body,ongoing,source_end_json,source_order,created_at,updated_at,fixed_at) VALUES('after-open','{subject}','{journal}',2,'fixed','','','','{{}}',1,'now','now','now')"
        ),
        format!(
            "UPDATE journal_operations SET receipt_json='{{}}' WHERE journal_id='{journal}' AND request_id='fixed'"
        ),
        format!(
            "DELETE FROM journal_operations WHERE journal_id='{journal}' AND request_id='fixed'"
        ),
        format!("DELETE FROM subjects WHERE subject_id='{subject}'"),
        format!(
            "INSERT INTO journal_corrections(subject_id,entry_id,target_id) VALUES('{subject}','{open_id}','{open_id}')"
        ),
    ];
    for sql in invalid_sql {
        assert!(
            matches!(fixture.sql(&sql), Err(JournalError::Storage(_))),
            "schema accepted {sql}"
        );
        assert_eq!(fixture.state(), before);
        assert_eq!(fixture.entries(), entries);
    }
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.entries(), entries);
    assert!(fixture.receipt("fixed").is_some());
    assert!(fixture.receipt("open").is_some());
}

#[test]
fn failure_before_receipt_rolls_back_record_and_checkpoint_then_exact_retry_succeeds() {
    let mut fixture = Fixture::new();
    let (_, ctx) = fixture.generation("transaction-fault", fixture.ranges.clone());
    fixture.sql("CREATE TRIGGER fail_test_receipt BEFORE INSERT ON journal_operations WHEN NEW.request_id IN ('fault-record','fault-checkpoint') BEGIN SELECT RAISE(ABORT, 'injected before receipt'); END;").unwrap();
    let record = fixture.operation(
        "fault-record",
        open_action("Must commit with its receipt"),
        fixture.ranges.clone(),
    );
    let before = fixture.state();
    let sources = fixture.sources();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, record.clone()),
        Err(JournalError::Storage(_))
    ));
    fixture.reopen();
    assert_eq!(fixture.state(), before);
    assert_eq!(fixture.sources(), sources);
    assert!(fixture.entries().is_empty());
    assert!(fixture.receipt("fault-record").is_none());
    fixture.sql("DROP TRIGGER fail_test_receipt;").unwrap();
    let saved = fixture
        .store()
        .execute_journal(&ctx, record.clone())
        .unwrap();
    assert_eq!(
        fixture.store().execute_journal(&ctx, record).unwrap(),
        saved
    );
    let cp = fixture.operation(
        "fault-checkpoint",
        checkpoint(&["fault-record"]),
        fixture.ranges.clone(),
    );
    fixture.sql("CREATE TRIGGER fail_test_receipt BEFORE INSERT ON journal_operations WHEN NEW.request_id='fault-checkpoint' BEGIN SELECT RAISE(ABORT, 'injected before receipt'); END;").unwrap();
    let before_checkpoint = fixture.state();
    assert!(matches!(
        fixture.store().execute_journal(&ctx, cp.clone()),
        Err(JournalError::Storage(_))
    ));
    fixture.reopen();
    assert_eq!(fixture.state(), before_checkpoint);
    assert_eq!(fixture.sources(), sources);
    assert_eq!(fixture.receipt("fault-record"), Some(saved));
    assert!(fixture.receipt("fault-checkpoint").is_none());
    fixture.sql("DROP TRIGGER fail_test_receipt;").unwrap();
    let reflected = fixture.store().execute_journal(&ctx, cp.clone()).unwrap();
    assert_eq!((reflected.process_order, reflected.process_byte), (2, 0));
    fixture.reopen();
    assert_eq!(
        fixture.store().execute_journal(&ctx, cp).unwrap(),
        reflected
    );
    assert_eq!(fixture.entries().len(), 1);
    assert!(
        fixture
            .sources()
            .iter()
            .all(|source| source.reflected_bytes == source.source.total_bytes)
    );
}

#[test]
fn per_operation_reopen_recovers_a_realistic_investigation_and_later_correction_without_job_result()
{
    let mut fixture = Fixture::new();
    let first_range = fixture.ranges[0].clone();
    let second_range = fixture.ranges[1].clone();
    let (first_generation, first_ctx) =
        fixture.generation("investigation-job", vec![first_range.clone()]);
    let created = fixture.execute_and_reopen(
        &first_ctx,
        "investigation-create",
        open_action("Observed a gap in checkpoint evidence; reproducing it"),
        vec![first_range.clone()],
    );
    let first_id = created.entry_id.unwrap();
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-rewrite",
        JournalAction::Rewrite {
            entry_id: first_id.clone(),
            title: "Checkpoint evidence verified".into(),
            body: "Position-only updates cannot prove source reflection; record receipts can."
                .into(),
            ongoing: "Verify reopen before fixing the outcome".into(),
        },
        vec![first_range.clone()],
    );
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-position",
        JournalAction::UpdatePosition {
            current_position: "Record saved; checkpoint not yet committed".into(),
        },
        vec![],
    );
    assert_eq!(fixture.state().0.process_order, 0);
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-fix",
        JournalAction::Fix {
            entry_id: first_id.clone(),
            source_end: first_range.source.pointer.clone(),
        },
        vec![],
    );
    assert!(fixture.state().1.is_none());
    assert_eq!(fixture.state().0.process_order, 0);
    let fixed_before_correction = fixture.entries()[0].clone();
    fixture.execute_and_reopen(
        &first_ctx,
        "investigation-checkpoint",
        checkpoint(&["investigation-rewrite"]),
        vec![first_range.clone()],
    );
    fixture.close(&first_generation);
    fixture.reopen();
    let (second_generation, second_ctx) =
        fixture.generation("followup-job", vec![second_range.clone()]);
    assert_eq!(
        second_generation.previous_fixed_end,
        Some(first_range.source.pointer)
    );
    assert_eq!(second_generation.initial_journal.process_order, 1);
    assert_eq!(
        second_generation.initial_journal.current_position,
        "Record saved; checkpoint not yet committed"
    );
    assert!(second_generation.initial_open.is_none());
    let mut correction =
        open_action("Clarify that explicit no-change receipts also count as evidence");
    if let JournalAction::Create {
        correction_refs, ..
    } = &mut correction
    {
        correction_refs.push(first_id);
    }
    let correction_id = fixture
        .execute_and_reopen(
            &second_ctx,
            "followup-create",
            correction,
            vec![second_range.clone()],
        )
        .entry_id
        .unwrap();
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-fix",
        JournalAction::Fix {
            entry_id: correction_id,
            source_end: second_range.source.pointer.clone(),
        },
        vec![],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-confirm",
        JournalAction::ConfirmUnchanged,
        vec![second_range.clone()],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-position",
        JournalAction::UpdatePosition {
            current_position: "Follow-up recorded; all admitted input has been checked".into(),
        },
        vec![],
    );
    fixture.execute_and_reopen(
        &second_ctx,
        "followup-checkpoint",
        checkpoint(&["followup-create", "followup-confirm"]),
        vec![second_range],
    );
    fixture.close(&second_generation);
    fixture.reopen();
    let entries = fixture.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0], fixed_before_correction);
    assert_eq!(entries[1].correction_refs, vec![entries[0].id.clone()]);
    assert!(
        entries
            .iter()
            .all(|entry| entry.state == JournalEntryState::Fixed)
    );
    assert_eq!(
        (
            fixture.state().0.process_order,
            fixture.state().0.process_byte
        ),
        (2, 0)
    );
    assert!(fixture.state().1.is_none());
}

#[test]
fn split_input_and_replacement_job_reconcile_old_record_coverage_without_duplicate_create() {
    let mut fixture = Fixture::with_sizes(&[8]);
    let whole = fixture.ranges[0].clone();
    let (generation, ctx) = fixture.generation(
        "split-job",
        vec![fragment(&whole, 0, 4), fragment(&whole, 4, 8)],
    );
    let saved = fixture.execute(
        &ctx,
        "record",
        fixed_action("Confirmed from both UTF-8 input fragments", &whole, vec![]),
        vec![whole.clone()],
    );
    fixture.close(&generation);
    fixture.reopen();
    assert!(
        fixture
            .store()
            .active_journal_generation(&fixture.host, &fixture.journal_id)
            .unwrap()
            .is_none()
    );
    let status = fixture
        .store()
        .read_journal_operation_status(&fixture.host, &fixture.journal_id, "record")
        .unwrap();
    assert_eq!(status.kind, JournalOperationKind::Create);
    assert_eq!(status.receipt, saved);
    assert_eq!(status.covered_range_count, 1);
    let coverage = fixture
        .store()
        .read_journal_operation_coverage(&fixture.host, &fixture.journal_id, "record", 0, 1)
        .unwrap();
    assert_eq!(coverage.items, vec![whole.clone()]);
    let (_, replacement) = fixture.generation("replacement-job", vec![whole.clone()]);
    let cp = fixture.operation("recover-reflection", checkpoint(&["record"]), vec![whole]);
    let result = fixture
        .store()
        .checkpoint_journal(&replacement, cp)
        .unwrap();
    assert_eq!((result.process_order, result.process_byte), (1, 0));
    assert_eq!(fixture.entries().len(), 1);
    assert_eq!(fixture.entries()[0].id, saved.entry_id.unwrap());
    assert!(fixture.state().1.is_none());
}

#[test]
fn serialized_metadata_and_text_budgets_preserve_continuation_and_availability_recovery() {
    let fixture = Fixture::with_sizes(&[8; 64]);
    let (_, ctx) = fixture.generation("large-pages", fixture.ranges.clone());
    for (i, r) in fixture.ranges.iter().enumerate() {
        let action = JournalAction::Create {
            title: "\u{0001}".repeat(1024),
            body: "\u{0001}".repeat(65536),
            ongoing: String::new(),
            state: JournalEntryState::Fixed,
            source_end: Some(r.source.pointer.clone()),
            correction_refs: vec![],
        };
        fixture.execute(&ctx, &format!("record-{i}"), action, vec![r.clone()]);
    }
    let mut after = None;
    let mut ids = Vec::new();
    loop {
        let page = fixture
            .store()
            .list_journal_entries(&fixture.host, &fixture.journal_id, after, 32)
            .unwrap();
        assert!(serde_json::to_string_pretty(&page).unwrap().len() <= MAX_JOURNAL_RESPONSE_BYTES);
        assert!(!page.items.is_empty());
        ids.extend(page.items.iter().map(|e| e.id.clone()));
        let Some(next) = page.next else { break };
        after = Some(next.parse().unwrap());
    }
    assert_eq!(ids.len(), 64);
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        64
    );
    let mut offset = 0;
    let mut token = None;
    let mut body = String::new();
    loop {
        let page = fixture
            .store()
            .read_journal_text(
                &fixture.host,
                &fixture.journal_id,
                Some(&ids[0]),
                JournalTextField::Body,
                token.as_deref(),
                offset,
                MAX_JOURNAL_READ_BYTES,
            )
            .unwrap();
        assert!(serde_json::to_string_pretty(&page).unwrap().len() <= MAX_JOURNAL_RESPONSE_BYTES);
        body.push_str(&page.text);
        token = Some(page.content_token);
        let Some(next) = page.next_byte else { break };
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(body, "\u{0001}".repeat(65536));
    fixture
        .store()
        .mark_journal_sources_unavailable(&fixture.host, &fixture.journal_id, &fixture.ranges[..1])
        .unwrap();
    assert!(!fixture.entries()[0].source_available);
    fixture
        .store()
        .mark_journal_sources_available(&fixture.host, &fixture.journal_id, &fixture.ranges[..1])
        .unwrap();
    assert!(fixture.entries()[0].source_available);
    assert_eq!(fixture.state().0.process_order, 0);
}

#[test]
fn journal_migration_failure_rolls_back_ddl_and_retries_without_changing_existing_subject() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let manager = FeatureStorage::new(&root);
    let scope = manager.scope("migration-rollback").unwrap();
    let registration = scope
        .register(feature_storage::FeatureRegistration::new(
            crate::SUBJEKTIV_FEATURE_ID,
            &crate::MIGRATIONS[..7],
        ))
        .unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    let subject = store
        .create_subject_with_behavior(
            SubjectRole::new("preserved").unwrap(),
            "Original behavior".into(),
        )
        .unwrap();
    store
        .database
        .try_transaction::<_, JournalError>(|tx| {
            tx.execute_batch("CREATE TABLE journal_entries(injected INTEGER)")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    drop(scope);
    manager.shutdown().unwrap();
    drop(manager);
    let manager = FeatureStorage::new(&root);
    let scope = manager.scope("migration-rollback").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    assert!(matches!(
        SubjektivStore::open(&scope, &registration),
        Err(crate::SubjektivError::Storage(
            feature_storage::FeatureStorageError::MigrationFailed { .. }
        ))
    ));
    drop(scope);
    manager.shutdown().unwrap();
    drop(manager);
    let manager = FeatureStorage::new(&root);
    let scope = manager.scope("migration-rollback").unwrap();
    let registration = scope
        .register(feature_storage::FeatureRegistration::new(
            crate::SUBJEKTIV_FEATURE_ID,
            &crate::MIGRATIONS[..7],
        ))
        .unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
    store
        .database
        .try_transaction::<_, JournalError>(|tx| {
            assert!(!tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='journals')",
                [],
                |r| r.get::<_, bool>(0)
            )?);
            tx.execute_batch("DROP TABLE journal_entries")?;
            Ok(())
        })
        .unwrap();
    drop(store);
    drop(scope);
    manager.shutdown().unwrap();
    drop(manager);
    let manager = FeatureStorage::new(&root);
    let scope = manager.scope("migration-rollback").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    assert_eq!(store.subject(&subject.id).unwrap(), Some(subject.clone()));
    let host = session_host(&store, &subject.id, "post-migration-session");
    let range = source_range("post-migration-session", 0, 8);
    let id = store
        .accept_journal_sources(&host, "Recovered migration", &[range])
        .unwrap();
    assert_eq!(
        store.read_journal(&host, &id).unwrap().source_session_id,
        "post-migration-session"
    );
}

#[test]
fn late_gap_admission_and_partial_updates_reject_stale_source_page_cursors() {
    let fixture = Fixture::with_sizes(&[8]);
    let second = source_range("source-session", 1, 8);
    let third = source_range("source-session", 2, 8);
    let fourth = source_range("source-session", 3, 8);
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &[third, fourth])
        .unwrap();
    let first = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    let cursor = first.next.unwrap();
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &[second])
        .unwrap();
    let next =
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, Some(&cursor), 2);
    assert!(
        matches!(next, Err(JournalError::Conflict(_))),
        "gap completion must not silently skip newly admitted order 1: {next:?}"
    );
    let fresh = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, None, 2)
        .unwrap();
    assert_eq!(
        fresh
            .items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![0, 1]
    );
    let next = fixture
        .store()
        .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
        .unwrap();
    assert_eq!(
        next.items
            .iter()
            .map(|s| s.source.order)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
    // Duplicate notifications do not change the content condition.
    fixture
        .store()
        .accept_journal_sources(&fixture.host, "", &fixture.ranges)
        .unwrap();
    assert!(
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
            .is_ok()
    );
    let selected = fixture.ranges[0].clone();
    let (_, job) = fixture.generation("reflect-during-list", vec![selected.clone()]);
    fixture.execute(
        &job,
        "no-change",
        JournalAction::ConfirmUnchanged,
        vec![selected.clone()],
    );
    assert!(
        fixture
            .store()
            .list_journal_sources(&fixture.host, &fixture.journal_id, fresh.next.as_deref(), 2)
            .is_ok()
    );
    fixture.execute(
        &job,
        "checkpoint",
        checkpoint(&["no-change"]),
        vec![selected],
    );
    assert!(matches!(
        fixture.store().list_journal_sources(
            &fixture.host,
            &fixture.journal_id,
            fresh.next.as_deref(),
            2
        ),
        Err(JournalError::Conflict(_))
    ));
    assert_eq!(
        fixture
            .store()
            .read_journal_source(&fixture.host, &fixture.journal_id, 0)
            .unwrap()
            .reflected_bytes,
        8
    );

    let host = session_host(fixture.store(), &fixture.host.subject_id, "partial-session");
    let range = source_range("partial-session", 0, 16);
    let later = source_range("partial-session", 1, 8);
    let id = fixture
        .store()
        .accept_journal_sources(&host, "Partial source", &[fragment(&range, 0, 4), later])
        .unwrap();
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .accept_journal_sources(&host, "", &[fragment(&range, 4, 8)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .mark_journal_sources_unavailable(&host, &id, &[fragment(&range, 0, 8)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    fixture
        .store()
        .mark_journal_sources_available(&host, &id, &[fragment(&range, 0, 4)])
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, page.next.as_deref(), 1),
        Err(JournalError::Conflict(_))
    ));
    let page = fixture
        .store()
        .list_journal_sources(&host, &id, None, 1)
        .unwrap();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &fixture.journal_id, page.next.as_deref(), 1),
        Err(JournalError::InvalidInput(_))
    ));
    let mut cursor: serde_json::Value =
        serde_json::from_str(page.next.as_deref().unwrap()).unwrap();
    cursor["unexpected_field"] = true.into();
    assert!(matches!(
        fixture
            .store()
            .list_journal_sources(&host, &id, Some(&cursor.to_string()), 1),
        Err(JournalError::InvalidInput(_))
    ));
}
