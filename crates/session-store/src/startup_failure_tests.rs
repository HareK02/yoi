//! Component coverage of startup attachments through real FsWorkerStore files,
//! reopen and process-local CAS. Only the directory-sync IO boundary is replaced
//! for the post-rename failure case; these tests do not claim OS-crash durability.
//! Selected by the normal session-store lib test target.

use super::*;
use error_contract::{FailureFacts, OperationContext, Storage, project_failure};
use protocol::startup_failure::{RuntimeStartupCause, RuntimeStartupReason, StartupStage};

fn attachment(stage: StartupStage) -> StoredStartupFailureAttachment {
    let mut facts = FailureFacts::new(RuntimeStartupCause {
        stage,
        reason: RuntimeStartupReason::StartupFailed,
    })
    .unwrap();
    facts
        .with_context(OperationContext {
            operation: "worker_startup".into(),
            stage: "runtime".into(),
            operation_ref: None,
        })
        .unwrap();
    let projected = project_failure::<Storage, _>(&facts, &()).unwrap();
    StoredStartupFailureAttachment::from_projected(&projected).unwrap()
}

fn metadata() -> WorkerMetadata {
    let session_id = crate::new_session_id();
    let mut metadata = WorkerMetadata::new(
        "agent",
        Some(WorkerActiveSegmentRef::pending_segment(session_id)),
    )
    .with_workspace_id("workspace-one");
    metadata.subjektiv_session_attribution = Some(SubjektivSessionAttributionState::Failed {
        session_id,
        diagnostic: "legacy diagnostic is not a typed cause".into(),
    });
    metadata.resolved_manifest_snapshot = Some(serde_json::json!({"retained": true}));
    metadata.peers.push(WorkerPeer {
        worker_name: "peer".into(),
    });
    metadata
}

fn bound_failure(
    metadata: &WorkerMetadata,
    attachment: &StoredStartupFailureAttachment,
) -> SessionStartupFailure {
    SessionStartupFailure::from_expected(
        &StartupFailureExpectation::from_metadata(metadata).unwrap(),
        attachment,
    )
}

fn assert_stored_cause(metadata: &WorkerMetadata, stage: StartupStage) {
    match metadata.decode_startup_failure::<RuntimeStartupCause>() {
        StartupFailureRead::Present(DecodeFailure::Known(received)) => {
            assert_eq!(received.facts().cause().stage, stage);
            assert_eq!(
                received.facts().cause().reason,
                RuntimeStartupReason::StartupFailed
            );
            assert_eq!(received.facts().context().operations.len(), 1);
            assert_eq!(
                received.facts().context().operations[0].operation,
                "worker_startup"
            );
        }
        _ => panic!("expected a known typed startup cause"),
    }
}

#[test]
fn startup_failure_cas_reopens_typed_cause_and_preserves_unrelated_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let cause = attachment(StartupStage::Binding);
    let received = cause.clone();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &cause),
        StartupFailureSaveResult::Persisted
    ));
    assert_eq!(cause, received);
    drop(store);

    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    let restored = reopened.read_by_name("agent").unwrap().unwrap();
    assert_stored_cause(&restored, StartupStage::Binding);
    let mut wanted = original;
    wanted.startup_failure = Some(SessionStartupFailure::from_expected(&expected, &received));
    assert_eq!(restored, wanted);
}

#[test]
fn startup_failure_wrapper_fingerprints_attribution_without_copying_raw_diagnostics() {
    const CREDENTIAL: &str = "credential=TEST_ONLY_METADATA_SENTINEL";
    const PRIVATE_PATH: &str = "/private/test-only-attribution/session.json";
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let mut original = metadata();
    original.subjektiv_session_attribution = Some(SubjektivSessionAttributionState::Failed {
        session_id: original.active.as_ref().unwrap().session_id,
        diagnostic: format!("{CREDENTIAL} {PRIVATE_PATH}"),
    });
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Persisted
    ));
    drop(store);

    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    let restored = reopened.read_by_name("agent").unwrap().unwrap();
    assert_eq!(
        restored.subjektiv_session_attribution,
        original.subjektiv_session_attribution
    );
    // The original field still retains the diagnostic: absence from the wrapper
    // must not be explained by removing or redacting the source metadata instead.
    let retained_attribution =
        serde_json::to_string(&restored.subjektiv_session_attribution).unwrap();
    assert!(retained_attribution.contains(CREDENTIAL));
    assert!(retained_attribution.contains(PRIVATE_PATH));
    let wrapper = restored.startup_failure.as_ref().unwrap();
    let wrapper_json = serde_json::to_string(wrapper).unwrap();
    assert!(!wrapper_json.contains(CREDENTIAL));
    assert!(!wrapper_json.contains(PRIVATE_PATH));
    let wrapper_value = serde_json::to_value(wrapper).unwrap();
    assert!(wrapper_value.get("attribution").is_none());
    let expected_fingerprint: [u8; 32] = Sha256::digest(retained_attribution.as_bytes()).into();
    assert_eq!(bound(wrapper).attribution_fingerprint, expected_fingerprint);
    assert_stored_cause(&restored, StartupStage::Binding);
}

#[test]
fn startup_failure_cas_rejects_changed_session_owner_attribution_and_cause_without_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let cause = attachment(StartupStage::Binding);
    let path = temp.path().join("agent/metadata.json");

    // Each row changes one compared fact. Worker-name mismatch is an on-disk
    // identity mismatch, not a write to a different key.
    let changed = [
        {
            let mut value = original.clone();
            value.active = Some(WorkerActiveSegmentRef::pending_segment(
                crate::new_session_id(),
            ));
            value
        },
        {
            let mut value = original.clone();
            value.workspace_id = Some("new-owner".into());
            value
        },
        {
            let mut value = original.clone();
            value.worker_name = "new-worker".into();
            value
        },
        {
            let mut value = original.clone();
            value.subjektiv_session_attribution =
                Some(SubjektivSessionAttributionState::Confirmed {
                    session_id: expected.session_id,
                    subject_id: "new-subject".into(),
                });
            value
        },
        {
            let mut value = original.clone();
            value.subjektiv_session_attribution = None;
            value
        },
        {
            let mut value = original.clone();
            value.subjektiv_session_attribution = Some(SubjektivSessionAttributionState::Failed {
                session_id: expected.session_id,
                diagnostic: "new attribution observation".into(),
            });
            value
        },
        {
            let mut value = original.clone();
            value.startup_failure = Some(bound_failure(&value, &attachment(StartupStage::Feature)));
            value
        },
        {
            let mut value = original.clone();
            value.active = None;
            value
        },
    ];
    for value in changed {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Retain noncanonical whitespace to detect even a no-op rewrite.
        let bytes = format!("  {}\n", serde_json::to_string(&value).unwrap());
        fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            store.compare_and_swap_startup_failure(&expected, &cause),
            StartupFailureSaveResult::Conflict
        ));
        assert_eq!(fs::read(&path).unwrap(), bytes.as_bytes());
        assert_eq!(store.read_by_name("agent").unwrap().unwrap(), value);
    }
}

#[test]
fn startup_failure_replacement_requires_the_observed_existing_attachment() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let mut original = metadata();
    original.startup_failure = Some(bound_failure(&original, &attachment(StartupStage::Feature)));
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let mut stale = expected.clone();
    stale.startup_failure = None;
    let cause = attachment(StartupStage::Binding);
    assert!(matches!(
        store.compare_and_swap_startup_failure(&stale, &cause),
        StartupFailureSaveResult::Conflict
    ));
    assert_eq!(store.read_by_name("agent").unwrap().unwrap(), original);
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &cause),
        StartupFailureSaveResult::Persisted
    ));
    assert_stored_cause(
        &store.read_by_name("agent").unwrap().unwrap(),
        StartupStage::Binding,
    );
}

#[test]
fn startup_failure_cas_missing_metadata_does_not_create_a_worker() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&metadata()).unwrap();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Conflict
    ));
    assert!(store.read_by_name("agent").unwrap().is_none());
    assert!(!temp.path().join("agent").exists());
}

#[test]
fn startup_failure_cas_allows_segment_movement_within_expected_session() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let active =
        WorkerActiveSegmentRef::active_segment(expected.session_id, crate::new_segment_id());
    store
        .set_active("agent", Some(active.clone()), None)
        .unwrap();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Persisted
    ));
    let restored = store.read_by_name("agent").unwrap().unwrap();
    assert_eq!(restored.active, Some(active));
    assert_eq!(
        restored.subjektiv_session_attribution,
        original.subjektiv_session_attribution
    );
    // A later Segment switch is not a Session/ownership change either.
    store
        .set_active(
            "agent",
            Some(WorkerActiveSegmentRef::active_segment(
                expected.session_id,
                crate::new_segment_id(),
            )),
            None,
        )
        .unwrap();
    assert_stored_cause(
        &store.read_by_name("agent").unwrap().unwrap(),
        StartupStage::Binding,
    );
}

#[test]
fn startup_failure_saved_binding_rejects_session_owner_and_attribution_changes_after_reopen() {
    for case in 0..7 {
        let temp = tempfile::tempdir().unwrap();
        let store = FsWorkerStore::new(temp.path()).unwrap();
        let original = metadata();
        store.write(&original).unwrap();
        let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
        let cause = attachment(StartupStage::Binding);
        assert!(matches!(
            store.compare_and_swap_startup_failure(&expected, &cause),
            StartupFailureSaveResult::Persisted
        ));
        let saved = store
            .read_by_name("agent")
            .unwrap()
            .unwrap()
            .startup_failure;
        store
            .update_by_name("agent", |value| match case {
                0 => {
                    value.active = Some(WorkerActiveSegmentRef::pending_segment(
                        crate::new_session_id(),
                    ))
                }
                1 => value.workspace_id = Some("new-workspace-owner".into()),
                2 => {
                    value.subjektiv_session_attribution =
                        Some(SubjektivSessionAttributionState::Confirmed {
                            session_id: expected.session_id,
                            subject_id: "new-subject-owner".into(),
                        })
                }
                3 => {
                    value.subjektiv_session_attribution =
                        Some(SubjektivSessionAttributionState::Failed {
                            session_id: expected.session_id,
                            diagnostic: "changed attribution observation".into(),
                        })
                }
                4 => value.active = None,
                5 => value.workspace_id = None,
                6 => value.subjektiv_session_attribution = None,
                _ => unreachable!(),
            })
            .unwrap();
        drop(store);

        let reopened = FsWorkerStore::new(temp.path()).unwrap();
        let restored = reopened.read_by_name("agent").unwrap().unwrap();
        // State changes retain the old cause as evidence; they do not rebind it
        // to the new Session/owner and do not turn a mismatch into absence.
        assert_eq!(restored.startup_failure, saved);
        let path = temp.path().join("agent/metadata.json");
        let bytes_before_read = fs::read(&path).unwrap();
        match restored.decode_startup_failure::<RuntimeStartupCause>() {
            StartupFailureRead::Mismatch(mismatch) => assert_eq!(
                mismatch,
                StartupFailureBindingMismatch {
                    worker_name: false,
                    session_id: matches!(case, 0 | 4),
                    workspace_id: matches!(case, 1 | 5),
                    attribution: matches!(case, 2 | 3 | 6),
                }
            ),
            _ => panic!("saved cause must not decode for a changed binding (case {case})"),
        }
        assert_eq!(fs::read(&path).unwrap(), bytes_before_read);
        assert!(matches!(
            reopened.compare_and_swap_startup_failure(&expected, &cause),
            StartupFailureSaveResult::Conflict
        ));
    }
}

#[test]
fn startup_failure_copy_to_another_worker_keeps_original_binding() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Persisted
    ));
    let mut copied = store.read_by_name("agent").unwrap().unwrap();
    copied.worker_name = "replacement".into();
    store.write(&copied).unwrap();
    drop(store);
    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    let restored = reopened.read_by_name("replacement").unwrap().unwrap();
    assert!(matches!(
        restored.decode_startup_failure::<RuntimeStartupCause>(),
        StartupFailureRead::Mismatch(StartupFailureBindingMismatch {
            worker_name: true,
            session_id: false,
            workspace_id: false,
            attribution: false,
        })
    ));
    assert_stored_cause(
        &reopened.read_by_name("agent").unwrap().unwrap(),
        StartupStage::Binding,
    );
}

#[test]
fn startup_failure_cas_compares_existing_binding_even_when_attachment_is_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let mut original = metadata();
    original.startup_failure = Some(bound_failure(&original, &attachment(StartupStage::Feature)));
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let mut stale = expected.clone();
    bound_mut(stale.startup_failure.as_mut().unwrap()).session_id = crate::new_session_id();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&stale, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Conflict
    ));
    assert_eq!(store.read_by_name("agent").unwrap().unwrap(), original);
    let mut changed_fingerprint = expected.clone();
    bound_mut(changed_fingerprint.startup_failure.as_mut().unwrap()).attribution_fingerprint[0] ^=
        1;
    assert!(matches!(
        store.compare_and_swap_startup_failure(
            &changed_fingerprint,
            &attachment(StartupStage::Binding)
        ),
        StartupFailureSaveResult::Conflict
    ));
    assert_eq!(store.read_by_name("agent").unwrap().unwrap(), original);
}

#[test]
fn startup_failure_bound_to_absent_optional_owner_fields_reopens_as_present() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let mut original = metadata();
    original.workspace_id = None;
    original.subjektiv_session_attribution = None;
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Binding)),
        StartupFailureSaveResult::Persisted
    ));
    drop(store);
    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    let restored = reopened.read_by_name("agent").unwrap().unwrap();
    assert_stored_cause(&restored, StartupStage::Binding);
    let expected_none_fingerprint: [u8; 32] = Sha256::digest(b"null").into();
    assert_eq!(
        bound(restored.startup_failure.as_ref().unwrap()).attribution_fingerprint,
        expected_none_fingerprint
    );
}

#[test]
fn startup_failure_cas_independently_opened_stores_have_one_concurrent_winner() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let handles = [StartupStage::Binding, StartupStage::Feature].map(|stage| {
        let store = FsWorkerStore::new(temp.path()).unwrap();
        let expected = expected.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            (
                stage,
                store.compare_and_swap_startup_failure(&expected, &attachment(stage)),
            )
        })
    });
    barrier.wait();
    let mut winner = None;
    let mut conflicts = 0;
    for handle in handles {
        let (stage, result) = handle.join().unwrap();
        match result {
            StartupFailureSaveResult::Persisted => {
                assert!(winner.replace(stage).is_none());
            }
            StartupFailureSaveResult::Conflict => conflicts += 1,
            other => panic!("unexpected persistence result: {other:?}"),
        }
    }
    assert_eq!(conflicts, 1);
    drop(store);
    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    assert_stored_cause(
        &reopened.read_by_name("agent").unwrap().unwrap(),
        winner.unwrap(),
    );
}

#[test]
fn startup_failure_legacy_missing_unknown_and_corrupt_survive_reopen_with_attribution() {
    let temp = tempfile::tempdir().unwrap();
    let original = metadata();
    let mut future: serde_json::Value = serde_json::from_str(
        serde_json::to_value(attachment(StartupStage::Binding))
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    future["version"] = serde_json::json!(65535);
    let fixtures = [
        None,
        Some(future.to_string()),
        Some("not an envelope".to_owned()),
    ];

    for (case, encoded) in fixtures.into_iter().enumerate() {
        let mut fixture = serde_json::to_value(&original).unwrap();
        assert!(fixture.get("startup_failure").is_none());
        if let Some(encoded) = encoded {
            // The new field is always a binding wrapper. Unknown/corrupt fixtures
            // change only its opaque codec payload, not the surrounding binding.
            let mut bound =
                serde_json::to_value(bound_failure(&original, &attachment(StartupStage::Binding)))
                    .unwrap();
            bound["attachment"] = serde_json::Value::String(encoded);
            fixture["startup_failure"] = bound;
        }
        let path = temp.path().join("agent/metadata.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(&fixture).unwrap()).unwrap();
        let reopened = FsWorkerStore::new(temp.path()).unwrap();
        let restored = reopened.read_by_name("agent").unwrap().unwrap();
        match (
            case,
            restored.decode_startup_failure::<RuntimeStartupCause>(),
        ) {
            (0, StartupFailureRead::Missing)
            | (1, StartupFailureRead::Present(DecodeFailure::Unsupported(_)))
            | (2, StartupFailureRead::Present(DecodeFailure::Corrupt(_))) => {}
            _ => panic!("legacy absence, unknown and corrupt must be distinguishable"),
        }
        assert_eq!(
            restored.subjektiv_session_attribution,
            original.subjektiv_session_attribution
        );
        assert_eq!(restored.active, original.active);
        assert_eq!(restored.workspace_id, original.workspace_id);
        if case != 0 {
            reopened
                .update_by_name("agent", |value| {
                    value.workspace_id = Some("new-owner".into())
                })
                .unwrap();
            let changed = reopened.read_by_name("agent").unwrap().unwrap();
            assert_eq!(changed.startup_failure, restored.startup_failure);
            assert!(matches!(
                changed.decode_startup_failure::<RuntimeStartupCause>(),
                StartupFailureRead::Mismatch(StartupFailureBindingMismatch {
                    workspace_id: true,
                    ..
                })
            ));
        }
    }
}

#[test]
fn startup_failure_save_read_error_is_not_saved_and_preserves_received_cause() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let path = temp.path().join("agent/metadata.json");
    fs::write(&path, b"invalid metadata").unwrap();
    let cause = attachment(StartupStage::Binding);
    let received = cause.clone();
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &cause),
        StartupFailureSaveResult::NotSaved(WorkerStoreError::Serde(_))
    ));
    assert_eq!(cause, received);
    assert_eq!(fs::read(path).unwrap(), b"invalid metadata");
}

#[test]
fn startup_failure_rename_error_is_not_saved_but_post_rename_sync_error_is_unknown() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let cause = attachment(StartupStage::Binding);
    let received = cause.clone();

    // Real IO rename failure before publication. The destination is an isolated
    // directory rather than metadata.json, so no retained metadata is destroyed.
    let blocked_path = temp.path().join("blocked/metadata.json");
    fs::create_dir_all(&blocked_path).unwrap();
    let result =
        save_startup_failure_locked(&store, &store.update_lock, &expected, &cause, |value| {
            write_metadata_atomically(value, &blocked_path)
        });
    assert!(matches!(
        result,
        StartupFailureSaveResult::NotSaved(WorkerStoreError::Io(_))
    ));
    assert_eq!(store.read_by_name("agent").unwrap().unwrap(), original);
    assert_eq!(
        fs::read_dir(blocked_path.parent().unwrap())
            .unwrap()
            .count(),
        1
    );

    // Production serialization + file sync + rename, replacing only directory
    // sync with an IO failure. Reopen sees publication, but durability is unknown.
    let path = store.metadata_path("agent").unwrap();
    let result =
        save_startup_failure_locked(&store, &store.update_lock, &expected, &cause, |value| {
            write_metadata_with_directory_sync(value, &path, |_| {
                Err(std::io::Error::other("directory sync unavailable"))
            })
        });
    assert!(matches!(
        result,
        StartupFailureSaveResult::OutcomeUnknown(WorkerStoreError::Io(_))
    ));
    assert_eq!(cause, received);
    drop(store);
    let reopened = FsWorkerStore::new(temp.path()).unwrap();
    let restored = reopened.read_by_name("agent").unwrap().unwrap();
    assert_stored_cause(&restored, StartupStage::Binding);
    let mut wanted = original;
    wanted.startup_failure = Some(SessionStartupFailure::from_expected(&expected, &received));
    assert_eq!(restored, wanted);
    // An uncertain attempt is not replayed against its stale expected cause.
    assert!(matches!(
        reopened.compare_and_swap_startup_failure(&expected, &cause),
        StartupFailureSaveResult::Conflict
    ));
}

#[test]
fn startup_failure_aggregate_and_combined_stores_use_atomic_cas() {
    let temp = tempfile::tempdir().unwrap();
    let aggregate = WorkerAggregateStore::new(temp.path().join("aggregate"), "agent").unwrap();
    let original = metadata();
    aggregate.write(&original).unwrap();
    let expected = StartupFailureExpectation::from_metadata(&original).unwrap();
    let store = CombinedStore::new((), aggregate);
    let cause = attachment(StartupStage::Binding);
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &cause),
        StartupFailureSaveResult::Persisted
    ));
    assert!(matches!(
        store.compare_and_swap_startup_failure(&expected, &attachment(StartupStage::Feature)),
        StartupFailureSaveResult::Conflict
    ));
    let reopened =
        WorkerAggregateStore::open_read_only(temp.path().join("aggregate"), "agent").unwrap();
    assert_stored_cause(
        &reopened.read_read_only().unwrap().unwrap(),
        StartupStage::Binding,
    );
}

fn bound(failure: &SessionStartupFailure) -> &StartupFailureBinding {
    match failure {
        SessionStartupFailure::Bound(binding) => binding,
        _ => panic!("expected current binding"),
    }
}
fn bound_mut(failure: &mut SessionStartupFailure) -> &mut StartupFailureBinding {
    match failure {
        SessionStartupFailure::Bound(binding) => binding,
        _ => panic!("expected current binding"),
    }
}

#[test]
fn startup_binding_unknown_version_and_malformed_value_do_not_destroy_lifecycle_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsWorkerStore::new(temp.path()).unwrap();
    let original = metadata();
    store.write(&original).unwrap();
    let mut wire = serde_json::to_value(&original).unwrap();
    for (value, unsupported) in [
        (
            serde_json::json!({"version": 900, "future_binding": true}),
            true,
        ),
        (
            serde_json::json!({"version": 1, "attachment": {"wrong_type": true}}),
            false,
        ),
        (serde_json::json!("invalid binding"), false),
        (serde_json::json!([1, 2]), false),
    ] {
        wire["startup_failure"] = value.clone();
        std::fs::write(
            temp.path().join("agent/metadata.json"),
            serde_json::to_vec(&wire).unwrap(),
        )
        .unwrap();
        let reopened = FsWorkerStore::new(temp.path())
            .unwrap()
            .read_by_name("agent")
            .unwrap()
            .unwrap();
        assert_eq!(reopened.active, original.active);
        assert_eq!(
            reopened.subjektiv_session_attribution,
            original.subjektiv_session_attribution
        );
        assert_eq!(
            reopened.resolved_manifest_snapshot,
            original.resolved_manifest_snapshot
        );
        match reopened.decode_startup_failure::<RuntimeStartupCause>() {
            StartupFailureRead::UnsupportedBindingVersion(900) if unsupported => {}
            StartupFailureRead::CorruptBinding if !unsupported => {}
            _ => panic!("binding diagnosis lost"),
        }
        assert_eq!(
            serde_json::to_value(&reopened).unwrap()["startup_failure"],
            value
        );
    }
}
