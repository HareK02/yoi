//! Saved-material component tests use the production FsStore/WorkerSessionStore
//! and temporary roots. No model, runtime, domain recorder or live data is used.
use super::*;
use crate::{
    FsStore, LoggedContentPart, LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, Store,
};

fn entry(id: &str, item: LoggedItem) -> LoggedHistoryEntry {
    LoggedHistoryEntry {
        item,
        metadata: LoggedSessionHistoryMetadata {
            input_metadata: None,
            entry_id: LoggedSessionHistoryEntryId(id.into()),
            origin: LoggedSessionHistoryOrigin::LocalHumanInput,
            derivation: None,
        },
    }
}
fn message(id: &str, role: LoggedRole, text: &str) -> LoggedHistoryEntry {
    entry(
        id,
        LoggedItem::Message {
            role,
            content: vec![LoggedContentPart::Text { text: text.into() }],
        },
    )
}
fn system_context(
    id: &str,
    text: &str,
    compaction: crate::InputCompactionUse,
) -> LoggedHistoryEntry {
    let mut entry = message(id, LoggedRole::System, text);
    entry.metadata.input_metadata = Some(
        crate::HistoryInputMetadata::new(
            crate::HistoryInputUsePolicy {
                recording_material: crate::InputEvidenceUse::Exclude,
                memory_evidence: crate::InputEvidenceUse::Exclude,
                observation: crate::InputObservationUse::Hidden,
                compaction,
            },
            None,
            None,
        )
        .unwrap(),
    );
    entry
}
fn seed(session: SessionId, history: Vec<LoggedHistoryEntry>) -> LogEntry {
    LogEntry::AnnotatedSegmentStart {
        ts: 1,
        session_id: session,
        system_prompt: Some("private prompt".into()),
        config: agen::llm_client::RequestConfig::default(),
        history,
        forked_from: None,
        compacted_from: None,
    }
}
fn invoke() -> LogEntry {
    LogEntry::Invoke {
        ts: 2,
        trigger: protocol::InvokeKind::UserSend,
    }
}
fn input(id: &str) -> LogEntry {
    LogEntry::AnnotatedUserInput {
        ts: 3,
        segments: vec![],
        extensions: vec![],
        history: vec![message(id, LoggedRole::User, "request")],
    }
}
fn output(id: &str, text: &str) -> LogEntry {
    LogEntry::AnnotatedAssistantItem {
        ts: 4,
        entry: message(id, LoggedRole::Assistant, text),
    }
}
fn cancelled() -> LogEntry {
    LogEntry::RunCancelled {
        ts: 5,
        entry_id: None,
    }
}
fn read(root: &Path, session: SessionId, segment: SegmentId) -> SessionPublicMaterialReader {
    SessionPublicMaterialReader::open_fs(root, session, segment, PublicMaterialLimits::default())
        .unwrap()
}
fn ids(reader: &SessionPublicMaterialReader) -> Vec<String> {
    reader
        .descriptors()
        .map(|d| d.identity.entry_ref.clone())
        .collect()
}

#[test]
fn external_erasure_fences_worker_reader_and_publisher_before_physical_delete_finishes() {
    use crate::retention::{SessionDeletionIntent, SessionRetentionStore};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("worker/session");
    let retention = SessionRetentionStore::new(temp.path().join("outside-aggregate"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let store = WorkerSessionStore::new_with_retention(&root, retention.clone()).unwrap();
    let records = [
        seed(session, vec![]),
        invoke(),
        input("u"),
        output("a", "saved answer"),
        cancelled(),
    ];
    store.create_segment(session, segment, &records).unwrap();
    let reader = SessionPublicMaterialReader::open_worker_with_retention(
        &root,
        segment,
        Default::default(),
        retention.clone(),
    )
    .unwrap();
    let descriptor = reader.descriptors().next().unwrap().clone();
    assert!(
        !reader
            .read_content(&descriptor, 0, 64)
            .unwrap()
            .content
            .is_empty()
    );
    // The fence precedes deletion and survives an I/O failure that leaves all original bytes.
    let error = retention.delete(session, SessionDeletionIntent::ErasePermanently, || {
        Err::<(), _>(crate::StoreError::Io(std::io::Error::other(
            "injected removal failure",
        )))
    });
    assert!(error.is_err());
    assert!(
        root.join("segments")
            .join(format!("{segment}.jsonl"))
            .exists()
    );
    assert!(matches!(
        reader.read_content(&descriptor, 0, 64),
        Err(PublicMaterialError::Erased)
    ));
    assert!(matches!(
        store.append(session, segment, &output("late", "must not survive")),
        Err(crate::StoreError::SessionErased(_))
    ));
    assert!(matches!(
        store.create_segment(session, crate::new_segment_id(), &records),
        Err(crate::StoreError::SessionErased(_))
    ));
    assert!(matches!(
        WorkerSessionStore::new_with_retention(&root, retention.clone()),
        Err(crate::StoreError::SessionErased(_))
    ));
    assert!(matches!(
        SessionPublicMaterialReader::open_worker_with_retention(
            &root,
            segment,
            Default::default(),
            retention.clone()
        ),
        Err(PublicMaterialError::Erased)
    ));
    retention
        .delete(session, SessionDeletionIntent::ErasePermanently, || {
            std::fs::remove_dir_all(&root)?;
            Ok(())
        })
        .unwrap();
    // A retained publisher cannot reconstruct an erased Session even with its old manifest ID.
    assert!(matches!(
        store.create_segment(session, segment, &records),
        Err(crate::StoreError::SessionErased(_))
    ));
    assert!(!root.exists());
}

#[test]
fn external_fence_survives_fs_relocation_and_rejects_old_and_retained_publishers() {
    use crate::retention::{SessionDeletionIntent, SessionRetentionPin, SessionRetentionStore};
    let temp = tempfile::tempdir().unwrap();
    let live = temp.path().join("live");
    let retained = temp.path().join("retained");
    let retention = SessionRetentionStore::new(temp.path().join("outside-workers"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let original = FsStore::new_with_retention(&live, retention.clone()).unwrap();
    original
        .create_segment(
            session,
            segment,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                output("a", "answer"),
                cancelled(),
            ],
        )
        .unwrap();
    original
        .pin_session(session, SessionRetentionPin(session))
        .unwrap();
    assert!(matches!(
        original.delete_session(session),
        Err(crate::StoreError::SessionPinned(_))
    ));
    {
        let _guard = retention.lock_live(session).unwrap();
        std::fs::rename(&live, &retained).unwrap();
    }
    let archived = FsStore::new_with_retention(&retained, retention.clone()).unwrap();
    let reader = SessionPublicMaterialReader::open_fs_with_retention(
        &retained,
        session,
        segment,
        Default::default(),
        retention.clone(),
    )
    .unwrap();
    let descriptor = reader.descriptors().next().unwrap().clone();
    assert!(
        !reader
            .read_content(&descriptor, 0, 64)
            .unwrap()
            .content
            .is_empty()
    );
    archived
        .delete_session_with_intent(session, SessionDeletionIntent::ErasePermanently)
        .unwrap();
    assert!(matches!(
        reader.read_content(&descriptor, 0, 64),
        Err(PublicMaterialError::Erased)
    ));
    for store in [&original, &archived] {
        assert!(matches!(
            store.append(session, segment, &output("late", "private")),
            Err(crate::StoreError::SessionErased(_))
        ));
        assert!(matches!(
            store.pin_session(session, SessionRetentionPin(session)),
            Err(crate::StoreError::SessionErased(_))
        ));
    }
    assert!(!live.join(session.to_string()).exists());
    assert!(!retained.join(session.to_string()).exists());
    assert!(retention.ensure_live(session).is_err());
}

#[test]
fn late_run_boundary_is_new_material_without_changing_previously_supplied_bodies() {
    for (terminal, expected) in [
        (cancelled(), PublicRunBoundary::Cancelled),
        (
            LogEntry::RunErrored {
                ts: 5,
                entry_id: None,
                interrupted: true,
                message: "SECRET_PROVIDER_DETAIL".into(),
                failure: None,
            },
            PublicRunBoundary::Errored,
        ),
        (
            LogEntry::RunCompleted {
                entry_id: None,
                ts: 5,
                interrupted: false,
                result: agen::EngineResult::Finished,
                active_run_turn_count: None,
            },
            PublicRunBoundary::Completed {
                outcome: PublicRunOutcome::Finished,
                interrupted: false,
            },
        ),
        (
            LogEntry::RunCompleted {
                entry_id: None,
                ts: 5,
                interrupted: true,
                result: agen::EngineResult::LimitReached,
                active_run_turn_count: None,
            },
            PublicRunBoundary::Completed {
                outcome: PublicRunOutcome::LimitReached,
                interrupted: true,
            },
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        store
            .create_segment(
                session,
                segment,
                &[
                    seed(session, vec![]),
                    invoke(),
                    input("u"),
                    output("a", "partial action"),
                ],
            )
            .unwrap();
        let before: Vec<_> = read(temp.path(), session, segment)
            .descriptors()
            .cloned()
            .collect();
        assert_eq!(before.len(), 2);
        // A Task/compaction admission may already have reflected every body at this point.
        store.append(session, segment, &terminal).unwrap();
        let saved = read(temp.path(), session, segment);
        let after: Vec<_> = saved.descriptors().cloned().collect();
        assert_eq!(&after[..before.len()], &before);
        assert_eq!(after.len(), before.len() + 1);
        let boundary = after.last().unwrap();
        assert_eq!(boundary.origin, PublicMaterialOrigin::HostRunBoundary);
        let content = saved.read_content(boundary, 0, 4096).unwrap().content;
        let value: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["kind"], "run_boundary");
        assert_eq!(value["boundary"], serde_json::to_value(expected).unwrap());
        assert!(!content.contains("SECRET_PROVIDER_DETAIL"));
        assert!(!content.contains("private prompt"));
        assert_eq!(
            saved.descriptors().cloned().collect::<Vec<_>>(),
            read(temp.path(), session, segment)
                .descriptors()
                .cloned()
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn yield_resume_cancel_keep_original_boundary_identity_across_compaction() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let compact = crate::new_segment_id();
    let kept = message("a", LoggedRole::Assistant, "partial action");
    store
        .create_segment(
            session,
            root,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                LogEntry::AnnotatedAssistantItem {
                    ts: 4,
                    entry: kept.clone(),
                },
                LogEntry::RunYielded {
                    ts: 5,
                    entry_id: Some(LoggedSessionHistoryEntryId("yield".into())),
                    reason: protocol::RunYieldReason::Compaction,
                    active_run_turn_count: 1,
                },
            ],
        )
        .unwrap();
    let original: Vec<_> = read(temp.path(), session, root)
        .descriptors()
        .cloned()
        .collect();
    let mut summary = system_context(
        "summary",
        "must not replace originals",
        crate::InputCompactionUse::OpeningSummary,
    );
    summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
    summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
        sources: vec![
            LoggedSessionHistoryEntryId("u".into()),
            LoggedSessionHistoryEntryId("a".into()),
        ],
    });
    let mut start = seed(session, vec![summary, kept]);
    if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
        *compacted_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 0,
        });
    }
    store
        .create_segment(
            session,
            compact,
            &[
                start,
                LogEntry::RunResumed {
                    ts: 6,
                    entry_id: Some(LoggedSessionHistoryEntryId("resume".into())),
                    source: protocol::RunResumeSource::Compaction,
                    active_run_turn_count: 1,
                },
                cancelled(),
            ],
        )
        .unwrap();
    let saved = read(temp.path(), session, compact);
    assert_eq!(
        &saved.descriptors().cloned().collect::<Vec<_>>()[..original.len()],
        &original
    );
    assert_eq!(saved.descriptors().len(), original.len() + 2);
    let boundary_ids: Vec<_> = saved
        .descriptors()
        .filter(|d| d.origin == PublicMaterialOrigin::HostRunBoundary)
        .map(|d| d.identity.entry_ref.clone())
        .collect();
    assert_eq!(&boundary_ids[..2], ["Eyield", "Eresume"]);
    assert_eq!(boundary_ids.len(), 3);
}

#[test]
fn active_rollback_eligible_input_is_excluded_until_ai_materializes() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![]), invoke(), input("user")],
        )
        .unwrap();
    assert!(ids(&read(temp.path(), session, segment)).is_empty());
    store
        .append(session, segment, &output("assistant", "answer"))
        .unwrap();
    assert_eq!(
        ids(&read(temp.path(), session, segment)),
        ["Euser", "Eassistant"]
    );
    // Later cancellation retains the exact body identities and adds one Host boundary.
    store.append(session, segment, &cancelled()).unwrap();
    let saved = read(temp.path(), session, segment);
    assert_eq!(&ids(&saved)[..2], ["Euser", "Eassistant"]);
    assert_eq!(saved.descriptors().len(), 3);
    assert_eq!(
        saved.descriptors().last().unwrap().origin,
        PublicMaterialOrigin::HostRunBoundary
    );
    let evidence =
        crate::public_index::read_fs_session_public_index(temp.path(), session, Default::default())
            .unwrap();
    assert_eq!(evidence.entry_count, 0);
}

#[test]
fn terminal_error_and_cancel_recover_persisted_prefix_without_success_inference() {
    for terminal in [
        cancelled(),
        LogEntry::RunErrored {
            ts: 5,
            entry_id: None,
            interrupted: true,
            message: "failure".into(),
            failure: None,
        },
    ] {
        let temp = tempfile::tempdir().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        store
            .create_segment(
                session,
                segment,
                &[seed(session, vec![]), invoke(), input("request"), terminal],
            )
            .unwrap();
        let saved = read(temp.path(), session, segment);
        assert_eq!(&ids(&saved)[..1], ["Erequest"]);
        assert_eq!(saved.descriptors().len(), 2);
        assert_eq!(
            saved.descriptors().last().unwrap().origin,
            PublicMaterialOrigin::HostRunBoundary
        );
    }
}

#[test]
fn utf8_windows_reconstruct_large_original_and_verify_full_digest() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let text = "漢字🙂\n".repeat(20_000);
    store
        .create_segment(
            session,
            segment,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                output("a", &text),
            ],
        )
        .unwrap();
    let reader = read(temp.path(), session, segment);
    let descriptor = reader.descriptors().nth(1).unwrap();
    let mut offset = 0;
    let mut combined = String::new();
    // Deliberately split multibyte characters, without quadratically hashing
    // the large entry in tiny windows in the test.
    while offset < descriptor.total_bytes {
        let part = reader.read_content(descriptor, offset, 8191).unwrap();
        assert!(part.content.len() <= 8191);
        assert!(part.next_offset > offset);
        combined.push_str(&part.content);
        offset = part.next_offset;
    }
    assert_eq!(
        hex_digest(&Sha256::digest(combined.as_bytes())),
        descriptor.sha256
    );
    assert_eq!(combined.len() as u64, descriptor.total_bytes);
    let body: serde_json::Value = serde_json::from_str(&combined).unwrap();
    assert_eq!(body["kind"], "message");
    assert_eq!(body["role"], "assistant");
    assert_eq!(body["content"][0]["text"], text);
    let at = combined.find('漢').unwrap();
    assert!(matches!(
        reader.read_content(descriptor, at as u64 + 1, 20),
        Err(PublicMaterialError::InvalidOffset)
    ));
}

#[test]
fn identical_empty_entries_have_distinct_identity_and_nonempty_framing() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    store
        .create_segment(
            session,
            segment,
            &[seed(
                session,
                vec![
                    message("a", LoggedRole::User, ""),
                    message("b", LoggedRole::User, ""),
                ],
            )],
        )
        .unwrap();
    let reader = read(temp.path(), session, segment);
    let d: Vec<_> = reader.descriptors().collect();
    assert_eq!(d[0].sha256, d[1].sha256);
    assert_ne!(d[0].identity, d[1].identity);
    assert!(d[0].total_bytes > 0);
    assert_eq!(d[1].canonical_order, 1);
}

#[test]
fn reasoning_proves_rollback_safety_but_private_and_derived_items_are_not_materials() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let mut derived = system_context("derived", "summary", crate::InputCompactionUse::Retain);
    derived.metadata.origin = LoggedSessionHistoryOrigin::DerivedSummary;
    let mut journal = system_context(
        "journal",
        "frozen Journal reference",
        crate::InputCompactionUse::ReferenceInput,
    );
    journal.metadata.origin = LoggedSessionHistoryOrigin::JournalContext;
    let mut summary = system_context(
        "compaction-summary",
        "complementary summary",
        crate::InputCompactionUse::OpeningSummary,
    );
    summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
    let reasoning = entry(
        "reasoning",
        LoggedItem::Reasoning {
            text: "private reasoning".into(),
            summary: vec![],
            encrypted_content: None,
            signature: None,
        },
    );
    store
        .create_segment(
            session,
            segment,
            &[
                seed(
                    session,
                    vec![
                        derived,
                        journal,
                        summary,
                        message("system", LoggedRole::System, "secret"),
                    ],
                ),
                LogEntry::AnnotatedSystemItem {
                    ts: 1,
                    entry: crate::LoggedSystemHistoryEntry {
                        item: crate::SystemItem::Input {
                            body: "previous Journal must not become new recording material".into(),
                        },
                        metadata: LoggedSessionHistoryMetadata {
                            input_metadata: Some(
                                crate::HistoryInputMetadata::new(
                                    crate::HistoryInputUsePolicy {
                                        recording_material: crate::InputEvidenceUse::Exclude,
                                        memory_evidence: crate::InputEvidenceUse::Exclude,
                                        observation: crate::InputObservationUse::Hidden,
                                        compaction: crate::InputCompactionUse::ReferenceInput,
                                    },
                                    None,
                                    None,
                                )
                                .unwrap(),
                            ),
                            entry_id: LoggedSessionHistoryEntryId("startup-journal".into()),
                            origin: LoggedSessionHistoryOrigin::DerivedSummary,
                            derivation: None,
                        },
                    },
                    extensions: vec![],
                },
                invoke(),
                input("u"),
                LogEntry::AnnotatedAssistantItem {
                    ts: 4,
                    entry: reasoning,
                },
            ],
        )
        .unwrap();
    assert_eq!(ids(&read(temp.path(), session, segment)), ["Eu"]);
}

#[test]
fn fork_clips_parent_suffix_and_maps_inherited_copy_to_original_segment() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let fork = crate::new_segment_id();
    let original = message("original", LoggedRole::User, "request");
    store
        .create_segment(
            session,
            root,
            &[
                seed(session, vec![original.clone()]),
                LogEntry::TurnEnd {
                    ts: 2,
                    turn_count: 1,
                },
                output("abandoned", "later"),
            ],
        )
        .unwrap();
    let mut start = seed(session, vec![original]);
    if let LogEntry::AnnotatedSegmentStart { forked_from, .. } = &mut start {
        *forked_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 1,
        });
    }
    store
        .create_segment(
            session,
            fork,
            &[start, invoke(), input("next"), output("result", "result")],
        )
        .unwrap();
    let reader = read(temp.path(), session, fork);
    assert_eq!(ids(&reader), ["Eoriginal", "Enext", "Eresult"]);
    let d = reader.descriptors().next().unwrap();
    assert_eq!(d.identity.segment_id, root);
    assert!(
        reader
            .read_content(d, 0, 1000)
            .unwrap()
            .content
            .contains("request")
    );
    // Archival source is not deleted or rewritten when selecting the fork.
    assert_eq!(
        ids(&read(temp.path(), session, root)),
        ["Eoriginal", "Eabandoned"]
    );
}

#[test]
fn schema_v3_compaction_migrates_persistently_and_strict_reader_keeps_original_ids() {
    for standalone in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let session = crate::new_session_id();
        let root = crate::new_segment_id();
        let compact = crate::new_segment_id();
        let mut summary = message(
            "summary",
            LoggedRole::System,
            "[Compacted context summary]\n\nold summary",
        );
        summary.metadata.origin = LoggedSessionHistoryOrigin::DerivedSummary;
        summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
            sources: vec![LoggedSessionHistoryEntryId("u".into())],
        });
        let mut start = seed(session, vec![summary.clone()]);
        if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
            *compacted_from = Some(SegmentOrigin {
                segment_id: root,
                at_turn_index: 0,
            });
        }
        let records = [
            start,
            output("new", "new"),
            cancelled(),
            LogEntry::RunCompleted {
                ts: 6,
                entry_id: None,
                interrupted: false,
                result: agen::EngineResult::Finished,
                active_run_turn_count: None,
            },
            LogEntry::PausedTurnAbandoned {
                ts: 7,
                entry_id: None,
            },
        ];
        let session_root = if standalone {
            temp.path().join(session.to_string())
        } else {
            temp.path().to_path_buf()
        };
        let paths = if standalone {
            session_root.clone()
        } else {
            session_root.join("segments")
        };
        std::fs::create_dir_all(&paths).unwrap();
        for (id, entries) in [
            (
                root,
                vec![seed(
                    session,
                    vec![message("u", LoggedRole::User, "request")],
                )],
            ),
            (compact, records.to_vec()),
        ] {
            let bytes: String = entries
                .iter()
                .map(|entry| format!("{}\n", serde_json::to_string(entry).unwrap()))
                .collect();
            std::fs::write(paths.join(format!("{id}.jsonl")), bytes).unwrap();
        }
        if !standalone {
            std::fs::write(
                session_root.join("session.json"),
                serde_json::to_vec(
                    &serde_json::json!({"schema_version": 3, "session_id": session}),
                )
                .unwrap(),
            )
            .unwrap();
        }
        let open = || {
            if standalone {
                SessionPublicMaterialReader::open_fs(
                    temp.path(),
                    session,
                    compact,
                    Default::default(),
                )
            } else {
                SessionPublicMaterialReader::open_worker(&session_root, compact, Default::default())
            }
        };
        assert!(
            open().is_err(),
            "strict reader must not reinterpret old provenance"
        );
        let migrate = || {
            if standalone {
                FsStore::new(temp.path()).unwrap();
            } else {
                WorkerSessionStore::new(&session_root).unwrap();
            }
        };
        migrate();
        let reader = open().unwrap();
        assert_eq!(&ids(&reader)[..2], ["Eu", "Enew"]);
        let path = paths.join(format!("{compact}.jsonl"));
        let bytes = std::fs::read(&path).unwrap();
        let entries: Vec<LogEntry> = String::from_utf8(bytes.clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let LogEntry::AnnotatedSegmentStart { history, .. } = &entries[0] else {
            panic!()
        };
        summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
        summary.metadata.input_metadata = Some(
            crate::HistoryInputMetadata::new(
                crate::HistoryInputUsePolicy {
                    recording_material: crate::InputEvidenceUse::Exclude,
                    memory_evidence: crate::InputEvidenceUse::Exclude,
                    observation: crate::InputObservationUse::Hidden,
                    compaction: crate::InputCompactionUse::OpeningSummary,
                },
                None,
                None,
            )
            .unwrap(),
        );
        assert_eq!(history, &[summary]);
        // Run-transition ids synthesized from byte offsets must be frozen before
        // the changed summary record shifts every subsequent record offset.
        let mut old_offset = 0;
        for (index, original) in records.iter().enumerate() {
            if index >= 2 {
                let mut expected = original.clone();
                expected.ensure_legacy_session_entry_id(session, compact, old_offset);
                assert_eq!(
                    serde_json::to_value(&entries[index]).unwrap(),
                    serde_json::to_value(expected).unwrap()
                );
                assert_eq!(
                    ids(&reader)[index],
                    format!("El-run-{session}-{compact}-{old_offset}")
                );
            }
            old_offset += serde_json::to_vec(original).unwrap().len() as u64 + 1;
        }
        migrate();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn midturn_compaction_uses_exact_source_ids_and_preserves_discarded_originals() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let compact = crate::new_segment_id();
    let kept = message("kept", LoggedRole::Assistant, "answer");
    store
        .create_segment(
            session,
            root,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                LogEntry::AnnotatedAssistantItem {
                    ts: 4,
                    entry: kept.clone(),
                },
                output("after-cut", "unrelated later append"),
            ],
        )
        .unwrap();
    let mut summary = system_context(
        "summary",
        "never use as original",
        crate::InputCompactionUse::OpeningSummary,
    );
    summary.metadata.origin = LoggedSessionHistoryOrigin::Provided {
        producer: "renamed-summary-owner".into(),
    };
    summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
        sources: vec![
            LoggedSessionHistoryEntryId("u".into()),
            LoggedSessionHistoryEntryId("kept".into()),
        ],
    });
    let mut start = seed(session, vec![summary, kept]);
    if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
        *compacted_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 0,
        });
    }
    store
        .create_segment(session, compact, &[start, output("new", "new")])
        .unwrap();
    let retention = crate::retention::fs_retention(temp.path());
    let guard = retention.lock_live(session).unwrap();
    let captured = crate::capture::capture_recording_fs_guarded(
        temp.path(),
        session,
        compact,
        Default::default(),
        Default::default(),
        &guard,
    )
    .unwrap();
    drop(guard);
    let saved_bytes = captured.encode(Default::default()).unwrap();
    let reader = read(temp.path(), session, compact);
    assert_eq!(ids(&reader), ["Eu", "Ekept", "Enew"]);
    for (entry, descriptor) in captured.entries.iter().zip(reader.descriptors()) {
        assert_eq!(captured.recording_text(entry.order).unwrap().0, descriptor);
    }
    store
        .append(session, compact, &output("after-snapshot", "next Job only"))
        .unwrap();
    assert_eq!(captured.encode(Default::default()).unwrap(), saved_bytes);
    let d: Vec<_> = reader.descriptors().collect();
    assert_eq!(d[0].identity.segment_id, root);
    assert_eq!(d[1].identity.segment_id, root);
    std::fs::remove_file(
        temp.path()
            .join(session.to_string())
            .join(format!("{root}.jsonl")),
    )
    .unwrap();
    assert!(
        SessionPublicMaterialReader::open_fs(temp.path(), session, compact, Default::default())
            .is_err()
    );
    assert!(reader.read_content(d[0], 0, 4096).is_err());
    assert_eq!(
        captured
            .recording_slice(Default::default(), 100, 8192)
            .unwrap()
            .capture
            .entries
            .len(),
        3
    );
    assert_eq!(captured.encode(Default::default()).unwrap(), saved_bytes);
}

#[test]
fn missing_ancestor_and_modified_inherited_identity_are_explicit_errors() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let fork = crate::new_segment_id();
    store
        .create_segment(
            session,
            root,
            &[seed(
                session,
                vec![message("u", LoggedRole::User, "original")],
            )],
        )
        .unwrap();
    let mut start = seed(session, vec![message("u", LoggedRole::User, "different")]);
    if let LogEntry::AnnotatedSegmentStart { forked_from, .. } = &mut start {
        *forked_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 0,
        });
    }
    store.create_segment(session, fork, &[start]).unwrap();
    assert!(matches!(
        SessionPublicMaterialReader::open_fs(temp.path(), session, fork, Default::default()),
        Err(PublicMaterialError::Inconsistent)
    ));
}

#[test]
fn long_run_has_record_ceiling_not_run_ceiling_and_oversize_record_fails_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let mut records = vec![seed(session, vec![]), invoke(), input("u")];
    for i in 0..100 {
        records.push(output(&format!("a-{i}"), &"x".repeat(1000)));
    }
    store.create_segment(session, segment, &records).unwrap();
    let limits = PublicMaterialLimits {
        max_record_bytes: 4096,
        ..Default::default()
    };
    let reader =
        SessionPublicMaterialReader::open_fs(temp.path(), session, segment, limits).unwrap();
    assert_eq!(reader.descriptors().len(), 101);
    store
        .append(session, segment, &output("huge", &"x".repeat(5000)))
        .unwrap();
    assert!(matches!(
        SessionPublicMaterialReader::open_fs(temp.path(), session, segment, limits),
        Err(PublicMaterialError::ResourceLimit)
    ));
}

#[test]
fn worker_and_fs_layouts_produce_same_canonical_materials_and_tool_framing() {
    let temp = tempfile::tempdir().unwrap();
    let worker = WorkerSessionStore::new(temp.path().join("worker")).unwrap();
    let fs = FsStore::new(temp.path().join("fs")).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let records = [
        seed(session, vec![]),
        invoke(),
        LogEntry::AnnotatedAssistantItem {
            ts: 3,
            entry: entry(
                "call",
                LoggedItem::ToolCall {
                    call_id: "c".into(),
                    name: "TaskUpdate".into(),
                    arguments: "{\"status\":\"completed\"}".into(),
                    call_index: None,
                    execution_id: None,
                    status: None,
                },
            ),
        },
        LogEntry::AnnotatedToolResult {
            ts: 4,
            entry: entry(
                "result",
                LoggedItem::ToolResult {
                    call_id: "c".into(),
                    summary: "preview".into(),
                    content: Some("whole original".into()),
                    attachments: vec![],
                    disposition: Default::default(),
                    is_error: false,
                },
            ),
        },
    ];
    worker.create_segment(session, segment, &records).unwrap();
    fs.create_segment(session, segment, &records).unwrap();
    let a = SessionPublicMaterialReader::open_worker(
        &temp.path().join("worker"),
        segment,
        Default::default(),
    )
    .unwrap();
    let b = read(&temp.path().join("fs"), session, segment);
    assert_eq!(
        a.descriptors().collect::<Vec<_>>(),
        b.descriptors().collect::<Vec<_>>()
    );
    let d = a.descriptors().nth(1).unwrap();
    let content = a.read_content(d, 0, 1000).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content.content).unwrap();
    assert_eq!(json["summary"], "preview");
    assert_eq!(json["content"], "whole original");
}

#[test]
fn replaced_source_is_rejected_by_window_digest_check() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![message("u", LoggedRole::User, "old")])],
        )
        .unwrap();
    let reader = read(temp.path(), session, segment);
    let d = reader.descriptors().next().unwrap();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![message("u", LoggedRole::User, "new")])],
        )
        .unwrap();
    assert!(matches!(
        reader.read_content(d, 0, 4),
        Err(PublicMaterialError::Changed)
    ));
}

#[test]
fn zero_turn_fork_from_seed_and_consecutive_compactions_preserve_original_identity() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let original = message("original", LoggedRole::User, "original");
    store
        .create_segment(session, root, &[seed(session, vec![original.clone()])])
        .unwrap();
    let fork = crate::new_segment_id();
    let mut start = seed(session, vec![original.clone()]);
    if let LogEntry::AnnotatedSegmentStart { forked_from, .. } = &mut start {
        *forked_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 0,
        });
    }
    store.create_segment(session, fork, &[start]).unwrap();
    let mut parent = fork;
    let mut source_ids = vec!["original".to_string()];
    for index in 0..3 {
        let compact = crate::new_segment_id();
        let summary_id = format!("summary-{index}");
        let mut summary = system_context(
            &summary_id,
            "not original",
            crate::InputCompactionUse::OpeningSummary,
        );
        summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
        summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
            sources: source_ids
                .iter()
                .cloned()
                .map(LoggedSessionHistoryEntryId)
                .collect(),
        });
        let mut start = seed(session, vec![summary, original.clone()]);
        if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
            *compacted_from = Some(SegmentOrigin {
                segment_id: parent,
                at_turn_index: 0,
            });
        }
        store.create_segment(session, compact, &[start]).unwrap();
        let reader = read(temp.path(), session, compact);
        assert_eq!(ids(&reader), ["Eoriginal"]);
        let d = reader.descriptors().next().unwrap();
        assert_eq!(d.identity.segment_id, root);
        assert!(
            reader
                .read_content(d, 0, 4096)
                .unwrap()
                .content
                .contains("original")
        );
        source_ids = vec![summary_id, "original".into()];
        parent = compact;
    }
}

#[test]
fn framed_empty_body_is_canonical_and_erasure_fence_prevents_late_reads() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![message("empty", LoggedRole::User, "")])],
        )
        .unwrap();
    let reader = read(temp.path(), session, segment);
    let descriptor = reader.descriptors().next().unwrap();
    let content = reader.read_content(descriptor, 0, 1000).unwrap();
    assert_eq!(
        content.content,
        r#"{"kind":"message","origin":{"kind":"local_human_input"},"role":"user","content":[{"kind":"text","text":""}]}"#
    );
    // Fence remains effective if physical deletion failed and bytes still exist.
    let result: Result<(), crate::StoreError> = store.retention().delete(
        session,
        crate::retention::SessionDeletionIntent::ErasePermanently,
        || Err(std::io::Error::other("injected failure").into()),
    );
    assert!(result.is_err());
    assert!(matches!(
        reader.read_content(descriptor, 0, 4),
        Err(PublicMaterialError::Erased)
    ));
    assert!(matches!(
        SessionPublicMaterialReader::open_fs(temp.path(), session, segment, Default::default()),
        Err(PublicMaterialError::Erased)
    ));
}

#[test]
fn unterminated_tail_is_not_committed_material_and_observation_preserves_log_atime() {
    use std::io::Write as _;
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![message("u", LoggedRole::User, "saved")])],
        )
        .unwrap();
    let path = temp
        .path()
        .join(session.to_string())
        .join(format!("{segment}.jsonl"));
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"{\"kind\":\"annotated_assistant_item\",\"unfinished\":")
        .unwrap();
    let atime = filetime::FileTime::from_unix_time(123, 0);
    filetime::set_file_atime(&path, atime).unwrap();
    let reader = read(temp.path(), session, segment);
    assert_eq!(ids(&reader), ["Eu"]);
    let descriptor = reader.descriptors().next().unwrap();
    reader.read_content(descriptor, 0, 4096).unwrap();
    assert_eq!(
        filetime::FileTime::from_last_access_time(&std::fs::metadata(path).unwrap()),
        atime
    );
}

#[test]
fn correction_preserves_original_identity_across_compaction_without_summary_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let compact = crate::new_segment_id();
    let original = entry(
        "result",
        LoggedItem::ToolResult {
            call_id: "c".into(),
            summary: "display".into(),
            content: Some("original full".into()),
            attachments: vec![],
            disposition: Default::default(),
            is_error: false,
        },
    );
    let corrected = entry(
        "result",
        LoggedItem::ToolResult {
            call_id: "c".into(),
            summary: "replacement".into(),
            content: Some("corrected full".into()),
            attachments: vec![],
            disposition: Default::default(),
            is_error: true,
        },
    );
    store
        .create_segment(
            session,
            root,
            &[
                seed(session, vec![original]),
                LogEntry::ToolResultCorrected {
                    ts: 2,
                    entry: corrected.clone(),
                },
            ],
        )
        .unwrap();
    let mut summary = system_context(
        "summary",
        "not original",
        crate::InputCompactionUse::OpeningSummary,
    );
    summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
    summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
        sources: vec![LoggedSessionHistoryEntryId("result".into())],
    });
    let mut start = seed(session, vec![summary, corrected]);
    if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
        *compacted_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 0,
        });
    }
    store.create_segment(session, compact, &[start]).unwrap();
    let reader = read(temp.path(), session, compact);
    let d = reader.descriptors().next().unwrap();
    assert_eq!(d.identity.segment_id, root);
    assert_eq!(d.identity.entry_ref, "Eresult");
    let body = reader.read_content(d, 0, 4096).unwrap();
    assert!(body.content.contains("corrected full"));
    assert!(!body.content.contains("original full"));
}

#[test]
fn compaction_without_exact_derivation_or_with_invalid_turn_is_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    store
        .create_segment(
            session,
            root,
            &[seed(
                session,
                vec![message("u", LoggedRole::User, "original")],
            )],
        )
        .unwrap();
    for (derivation, turn) in [
        (None, 0),
        (
            Some(crate::LoggedHistoryDerivation {
                sources: vec![LoggedSessionHistoryEntryId("u".into())],
            }),
            999,
        ),
    ] {
        let segment = crate::new_segment_id();
        let mut summary = system_context(
            "summary",
            "not original",
            crate::InputCompactionUse::OpeningSummary,
        );
        summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
        summary.metadata.derivation = derivation;
        let mut start = seed(session, vec![summary]);
        if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
            *compacted_from = Some(SegmentOrigin {
                segment_id: root,
                at_turn_index: turn,
            });
        }
        store.create_segment(session, segment, &[start]).unwrap();
        assert!(matches!(
            SessionPublicMaterialReader::open_fs(temp.path(), session, segment, Default::default()),
            Err(PublicMaterialError::Inconsistent)
        ));
    }
}

#[test]
fn generic_system_material_uses_explicit_evidence_purpose_not_producer_or_body() {
    use crate::{
        FeatureInputPayload, HistoryInputMetadata, HistoryInputUsePolicy, InputCompactionUse,
        InputEvidenceUse, InputObservationUse, LoggedSystemHistoryEntry, SystemInputEnvelope,
    };
    for worker_layout in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        let fork = crate::new_segment_id();
        let make =
            |producer: &str, recording_material, memory_evidence, body: &str, public: bool| {
                LoggedSystemHistoryEntry::from_input(
                    LoggedSessionHistoryOrigin::Provided {
                        producer: producer.into(),
                    },
                    None,
                    SystemInputEnvelope::new(
                        format!("private model text: {body}"),
                        HistoryInputMetadata::new(
                            HistoryInputUsePolicy {
                                recording_material,
                                memory_evidence,
                                observation: InputObservationUse::Hidden,
                                compaction: InputCompactionUse::Retain,
                            },
                            Some(
                                FeatureInputPayload::encode(
                                    "unknown:future",
                                    99,
                                    &serde_json::json!({"private": "not-public"}),
                                )
                                .unwrap(),
                            ),
                            public.then(|| crate::PublicSystemItem {
                                kind: "context".into(),
                                body: body.into(),
                                presentation: crate::PublicSystemPresentation::Context,
                            }),
                        )
                        .unwrap(),
                    )
                    .unwrap(),
                )
            };
        let inputs = [
            make(
                "same-owner",
                InputEvidenceUse::Include,
                InputEvidenceUse::Exclude,
                "do not extract this; original material",
                true,
            ),
            make(
                "same-owner",
                InputEvidenceUse::Exclude,
                InputEvidenceUse::Include,
                "memory-only original",
                true,
            ),
            make(
                "renamed-owner",
                InputEvidenceUse::Exclude,
                InputEvidenceUse::Exclude,
                "reference without warning text",
                true,
            ),
            make(
                "private-only",
                InputEvidenceUse::Include,
                InputEvidenceUse::Include,
                "must not publish without projection",
                false,
            ),
        ];
        let root = if worker_layout {
            temp.path().join("worker")
        } else {
            temp.path().join(session.to_string())
        };
        let store: Box<dyn Store> = if worker_layout {
            Box::new(WorkerSessionStore::new(&root).unwrap())
        } else {
            Box::new(FsStore::new(temp.path()).unwrap())
        };
        store
            .create_segment(session, segment, &[seed(session, vec![])])
            .unwrap();
        for entry in &inputs {
            store
                .append(
                    session,
                    segment,
                    &LogEntry::AnnotatedSystemItem {
                        ts: 2,
                        entry: entry.clone(),
                        extensions: vec![],
                    },
                )
                .unwrap();
        }
        store
            .append(
                session,
                segment,
                &LogEntry::TurnEnd {
                    ts: 3,
                    turn_count: 1,
                },
            )
            .unwrap();
        let mut fork_seed = seed(
            session,
            inputs
                .iter()
                .map(LoggedSystemHistoryEntry::to_history_entry)
                .collect(),
        );
        if let LogEntry::AnnotatedSegmentStart { forked_from, .. } = &mut fork_seed {
            *forked_from = Some(crate::SegmentOrigin {
                segment_id: segment,
                at_turn_index: 1,
            });
        }
        store.create_segment(session, fork, &[fork_seed]).unwrap();
        let open = |active, purpose| {
            if worker_layout {
                SessionPublicMaterialReader::open_worker_for_use(
                    &root,
                    active,
                    Default::default(),
                    crate::retention::fs_retention(temp.path()),
                    purpose,
                )
                .unwrap()
            } else {
                SessionPublicMaterialReader::open_fs_for_use(
                    temp.path(),
                    session,
                    active,
                    Default::default(),
                    crate::retention::fs_retention(temp.path()),
                    purpose,
                )
                .unwrap()
            }
        };
        for (purpose, selected, expected_text) in [
            (
                PublicMaterialUse::Recording,
                0,
                "do not extract this; original material",
            ),
            (PublicMaterialUse::MemoryEvidence, 1, "memory-only original"),
        ] {
            let original = open(segment, purpose);
            let inherited = open(fork, purpose);
            assert_eq!(
                ids(&original),
                vec![format!("E{}", inputs[selected].metadata.entry_id.0)]
            );
            assert_eq!(ids(&inherited), ids(&original));
            let descriptor = inherited.descriptors().next().unwrap();
            let body = inherited
                .read_content(descriptor, 0, 16 * 1024)
                .unwrap()
                .content;
            let value: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(value["role"], "system");
            assert_eq!(value["content"][0]["text"], expected_text);
            assert!(!body.contains("not-public"));
            assert!(!body.contains("private model text"));
            assert!(!body.contains("unknown:future"));
            if purpose == PublicMaterialUse::Recording {
                let retention = crate::retention::fs_retention(temp.path());
                let guard = retention.lock_live(session).unwrap();
                let capture = if worker_layout {
                    crate::capture::capture_recording_worker_guarded(
                        &root,
                        session,
                        fork,
                        Default::default(),
                        Default::default(),
                        &guard,
                    )
                } else {
                    crate::capture::capture_recording_fs_guarded(
                        temp.path(),
                        session,
                        fork,
                        Default::default(),
                        Default::default(),
                        &guard,
                    )
                }
                .unwrap();
                assert_eq!(capture.entries.len(), 1);
                let (saved, _, text) = capture.recording_text(0).unwrap();
                assert_eq!(saved, descriptor);
                assert_eq!(text, body);
                let view =
                    crate::capture::SessionCapture::try_from_dto(capture, Default::default())
                        .unwrap();
                assert!(view.search(&Default::default()).is_empty());
                assert!(view.overview().is_empty());
            }
        }
    }
}

#[test]
fn recording_capture_slices_canonical_utf8_with_metadata_and_legacy_descriptors() {
    use crate::capture::*;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("session");
    let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let store = WorkerSessionStore::new_with_retention(&root, retention.clone()).unwrap();
    let mut records = vec![seed(session, vec![]), invoke(), input("u")];
    for i in 0..150 {
        records.push(output(&format!("a-{i}"), "saved text"));
    }
    records.push(LogEntry::AnnotatedToolResult {
        ts: 5,
        entry: entry(
            "tool",
            LoggedItem::ToolResult {
                call_id: "call".into(),
                summary: "summary".into(),
                content: Some("漢字🍎\n\"".repeat(6000)),
                disposition: agen::tool::ToolResultDisposition::default(),
                is_error: false,
                attachments: vec![],
            },
        ),
    });
    records.push(cancelled());
    store.create_segment(session, segment, &records).unwrap();
    let guard = retention.lock_live(session).unwrap();
    let capture = capture_recording_worker_guarded(
        &root,
        session,
        segment,
        Default::default(),
        Default::default(),
        &guard,
    )
    .unwrap();
    drop(guard);
    assert_eq!(capture.entries.len(), 153);
    let reader = SessionPublicMaterialReader::open_worker_with_retention(
        &root,
        segment,
        Default::default(),
        retention.clone(),
    )
    .unwrap();
    let descriptors: Vec<_> = reader.descriptors().cloned().collect();
    for (entry, descriptor) in capture.entries.iter().zip(&descriptors) {
        let (saved, offset, text) = capture.recording_text(entry.order).unwrap();
        assert_eq!(saved, descriptor);
        assert_eq!(offset, 0);
        assert_eq!(
            reader
                .read_content(descriptor, 0, text.len().max(4))
                .unwrap()
                .content,
            text
        );
    }
    let bytes = capture.encode(Default::default()).unwrap();
    let restored = CaptureDto::decode(&bytes, Default::default()).unwrap();
    let mut position = RecordingPosition::default();
    let mut combined = vec![String::new(); descriptors.len()];
    let mut slices = 0;
    loop {
        let slice = restored.recording_slice(position, 29, 4096).unwrap();
        assert!(serde_json::to_vec(&slice).unwrap().len() <= 4096);
        assert!(!slice.capture.entries.is_empty());
        slice.capture.validate(Default::default()).unwrap();
        for entry in &slice.capture.entries {
            let (descriptor, offset, text) = slice.capture.recording_text(entry.order).unwrap();
            let body = &mut combined[entry.order as usize];
            assert_eq!(offset, body.len() as u64);
            assert_eq!(descriptor, &descriptors[entry.order as usize]);
            body.push_str(text);
        }
        slices += 1;
        let Some(next) = slice.next else {
            break;
        };
        assert_ne!(next, position);
        position = next;
    }
    assert!(slices > 10);
    for (order, text) in combined.iter().enumerate() {
        assert_eq!(text, capture.recording_text(order as u64).unwrap().2);
    }
    assert!(matches!(
        capture.entries.last().unwrap().body,
        CaptureBody::CanonicalPublic {
            boundary: Some(PublicRunBoundary::Cancelled),
            ..
        }
    ));
    let mut corrupt = restored.clone();
    if let CaptureBody::CanonicalPublic { text, .. } = &mut corrupt.entries[0].body {
        text.replace_range(0..1, "[");
    }
    assert!(corrupt.validate(Default::default()).is_err());
    let mut unknown: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    unknown["entries"][0]["body"]["descriptor"]["unexpected"] = true.into();
    assert!(
        CaptureDto::decode(&serde_json::to_vec(&unknown).unwrap(), Default::default()).is_err()
    );
    let mut missing = restored.clone();
    missing.entries.remove(1);
    assert!(matches!(
        missing.recording_slice(Default::default(), 3, 8192),
        Err(CaptureError::InvalidRange)
    ));
    let tool_order = 151;
    let tool_text = capture.recording_text(tool_order).unwrap().2;
    let bad = tool_text.find('漢').unwrap() as u64 + 1;
    assert!(matches!(
        capture.recording_slice(
            RecordingPosition {
                canonical_order: tool_order,
                byte_offset: bad
            },
            1,
            4096
        ),
        Err(CaptureError::InvalidRange)
    ));
    assert!(matches!(
        capture.recording_slice(Default::default(), 1, 10),
        Err(CaptureError::BudgetTooSmall)
    ));
    // The full-value ceiling does not constrain bounded construction of a long Tool result.
    let guard = retention.lock_live(session).unwrap();
    assert!(matches!(
        capture_recording_worker_guarded(
            &root,
            session,
            segment,
            Default::default(),
            CaptureLimits {
                max_bytes: 2048,
                ..Default::default()
            },
            &guard
        ),
        Err(PublicMaterialError::ResourceLimit)
    ));
    let bounded = capture_recording_slice_worker_guarded(
        &root,
        session,
        segment,
        Default::default(),
        RecordingPosition {
            canonical_order: tool_order,
            byte_offset: 0,
        },
        1,
        4096,
        &guard,
    )
    .unwrap();
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 4096);
    assert!(bounded.next.is_some());
}

#[test]
fn recording_capture_fence_blocks_publication_and_value_survives_later_append_and_erasure() {
    use crate::capture::*;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("session");
    let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let store = WorkerSessionStore::new_with_retention(&root, retention.clone()).unwrap();
    store
        .create_segment(
            session,
            segment,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                output("a", "saved"),
            ],
        )
        .unwrap();
    let guard = retention.lock_live(session).unwrap();
    let (started, receiver) = std::sync::mpsc::channel();
    let (done, completed) = std::sync::mpsc::channel();
    let writer = store.clone();
    let thread = std::thread::spawn(move || {
        started.send(()).unwrap();
        writer
            .append(session, segment, &output("later", "not in snapshot"))
            .unwrap();
        done.send(()).unwrap();
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let capture = capture_recording_worker_guarded(
        &root,
        session,
        segment,
        Default::default(),
        Default::default(),
        &guard,
    )
    .unwrap();
    assert!(completed.try_recv().is_err());
    let bytes = capture.encode(Default::default()).unwrap();
    drop(guard);
    completed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    thread.join().unwrap();
    assert_eq!(capture.encode(Default::default()).unwrap(), bytes);
    assert_eq!(capture.entries.len(), 2);
    retention
        .delete(
            session,
            crate::retention::SessionDeletionIntent::ErasePermanently,
            || {
                std::fs::remove_dir_all(&root)?;
                Ok(())
            },
        )
        .unwrap();
    assert!(matches!(
        capture_recording_worker(
            &root,
            session,
            segment,
            &retention,
            Default::default(),
            Default::default()
        ),
        Err(PublicMaterialError::Erased)
    ));
    // Pure reads do not authorize adoption after erase; Host consumer revocation is separate.
    assert_eq!(
        capture
            .recording_slice(Default::default(), 2, 8192)
            .unwrap()
            .capture
            .entries
            .len(),
        2
    );
}

#[test]
fn recording_capture_rejects_foreign_guard_and_storage_or_body_allocation_excess() {
    use crate::capture::*;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("session");
    let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let store = WorkerSessionStore::new_with_retention(&root, retention.clone()).unwrap();
    store
        .create_segment(
            session,
            segment,
            &[seed(
                session,
                vec![message("original", LoggedRole::User, "text")],
            )],
        )
        .unwrap();
    let wrong_guard = retention.lock_live(crate::new_session_id()).unwrap();
    assert!(matches!(
        capture_recording_worker_guarded(
            &root,
            session,
            segment,
            Default::default(),
            Default::default(),
            &wrong_guard
        ),
        Err(PublicMaterialError::Inconsistent)
    ));
    let guard = retention.lock_live(session).unwrap();
    let limits = PublicMaterialLimits {
        max_storage_bytes: 1,
        ..Default::default()
    };
    assert!(matches!(
        capture_recording_worker_guarded(
            &root,
            session,
            segment,
            limits,
            Default::default(),
            &guard
        ),
        Err(PublicMaterialError::ResourceLimit)
    ));
    let limits = CaptureLimits {
        max_text_bytes: 1,
        ..Default::default()
    };
    assert!(matches!(
        capture_recording_worker_guarded(
            &root,
            session,
            segment,
            Default::default(),
            limits,
            &guard
        ),
        Err(PublicMaterialError::ResourceLimit)
    ));
}

#[test]
fn fixed_replay_ends_do_not_refresh_for_appends_between_replay_and_body_snapshot() {
    use crate::capture::*;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("session");
    let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
    let session = crate::new_session_id();
    let segment = crate::new_segment_id();
    let store = WorkerSessionStore::new_with_retention(&root, retention.clone()).unwrap();
    store
        .create_segment(
            session,
            segment,
            &[
                seed(session, vec![]),
                invoke(),
                input("u"),
                output("a", "saved"),
            ],
        )
        .unwrap();
    let guard = retention.lock_live(session).unwrap();
    let replay = CommittedPublicReplay::open_worker_guarded(
        &root,
        segment,
        Default::default(),
        None,
        PublicMaterialUse::Recording,
    )
    .unwrap();
    let before = replay
        .recording_slice(Default::default(), 100, 8192)
        .unwrap();
    drop(guard);
    // Production keeps its fence through construction. Releasing it deliberately
    // here also verifies that the lower-level replay never refreshes file ends.
    store
        .append(session, segment, &output("later", "not in fixed files"))
        .unwrap();
    let other = crate::new_segment_id();
    store
        .create_segment(
            session,
            other,
            &[
                seed(session, vec![]),
                output("other", "different active segment"),
            ],
        )
        .unwrap();
    let after = replay
        .recording_slice(RecordingPosition::default(), 100, 8192)
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.capture.entries.len(), 2);
}

#[test]
fn recording_source_coordinates_count_hidden_rows_and_preserve_originals_across_compaction() {
    use crate::capture::*;
    let temp = tempfile::tempdir().unwrap();
    let retention = crate::retention::fs_retention(temp.path());
    let store = FsStore::new_with_retention(temp.path(), retention.clone()).unwrap();
    let session = crate::new_session_id();
    let root = crate::new_segment_id();
    let compact = crate::new_segment_id();
    let hidden =
        |id: &str| system_context(id, "private omitted row", crate::InputCompactionUse::Retain);
    let root_user = message("root-user", LoggedRole::User, "first public row");
    let tool = entry(
        "root-tool",
        LoggedItem::ToolResult {
            call_id: "call".into(),
            summary: "original".into(),
            content: Some("full original".into()),
            disposition: Default::default(),
            is_error: false,
            attachments: vec![],
        },
    );
    let mut corrected = tool.clone();
    if let LoggedItem::ToolResult { content, .. } = &mut corrected.item {
        *content = Some("corrected full original".into());
    }
    let source_ids = [
        "hidden-0",
        "root-user",
        "hidden-2",
        "hidden-3",
        "batch-user",
        "reasoning",
        "root-tool",
        "root-answer",
    ];
    store
        .create_segment(
            session,
            root,
            &[
                seed(
                    session,
                    vec![hidden("hidden-0"), root_user.clone(), hidden("hidden-2")],
                ),
                invoke(),
                LogEntry::AnnotatedUserInput {
                    ts: 2,
                    segments: vec![],
                    extensions: vec![],
                    history: vec![
                        hidden("hidden-3"),
                        message("batch-user", LoggedRole::User, "second public row"),
                    ],
                },
                LogEntry::AnnotatedAssistantItem {
                    ts: 3,
                    entry: entry(
                        "reasoning",
                        LoggedItem::Reasoning {
                            text: "private reasoning".into(),
                            summary: vec![],
                            encrypted_content: None,
                            signature: None,
                        },
                    ),
                },
                LogEntry::AnnotatedToolResult {
                    ts: 4,
                    entry: tool.clone(),
                },
                LogEntry::RunYielded {
                    ts: 6,
                    entry_id: Some(LoggedSessionHistoryEntryId("yield-gap".into())),
                    reason: protocol::RunYieldReason::Compaction,
                    active_run_turn_count: 1,
                },
                LogEntry::RunResumed {
                    ts: 7,
                    entry_id: Some(LoggedSessionHistoryEntryId("resume-gap".into())),
                    source: protocol::RunResumeSource::Compaction,
                    active_run_turn_count: 1,
                },
                output("root-answer", "public answer"),
                LogEntry::TurnEnd {
                    ts: 8,
                    turn_count: 1,
                },
            ],
        )
        .unwrap();
    let mut summary = system_context(
        "summary",
        "not an original",
        crate::InputCompactionUse::OpeningSummary,
    );
    summary.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
    summary.metadata.derivation = Some(crate::LoggedHistoryDerivation {
        sources: source_ids
            .iter()
            .map(|id| LoggedSessionHistoryEntryId((*id).into()))
            .collect(),
    });
    let mut start = seed(
        session,
        vec![summary, root_user, tool, hidden("child-hidden-seed")],
    );
    if let LogEntry::AnnotatedSegmentStart { compacted_from, .. } = &mut start {
        *compacted_from = Some(SegmentOrigin {
            segment_id: root,
            at_turn_index: 1,
        });
    }
    store
        .create_segment(
            session,
            compact,
            &[
                start,
                LogEntry::ToolResultCorrected {
                    ts: 9,
                    entry: corrected,
                },
                output("child-answer", "new child original"),
                LogEntry::AnnotatedUserInput {
                    ts: 10,
                    segments: vec![],
                    extensions: vec![],
                    history: vec![
                        hidden("child-hidden-input"),
                        message("child-user", LoggedRole::User, "new child user"),
                    ],
                },
                cancelled(),
            ],
        )
        .unwrap();
    let guard = retention.lock_live(session).unwrap();
    let capture = capture_recording_fs_guarded(
        temp.path(),
        session,
        compact,
        Default::default(),
        Default::default(),
        &guard,
    )
    .unwrap();
    let catalog = recording_catalog_fs_guarded(
        temp.path(),
        session,
        compact,
        Default::default(),
        &guard,
        |descriptors| descriptors.to_vec(),
    )
    .unwrap();
    assert_eq!(
        catalog,
        capture
            .entries
            .iter()
            .map(|entry| capture.recording_text(entry.order).unwrap().0.clone())
            .collect::<Vec<_>>()
    );
    drop(guard);
    let expected = [
        ("Eroot-user", root, 1),
        ("Ebatch-user", root, 4),
        ("Eroot-tool", root, 6),
        ("Eyield-gap", root, 7),
        ("Eresume-gap", root, 7),
        ("Eroot-answer", root, 7),
        ("Echild-answer", compact, 4),
        ("Echild-user", compact, 6),
    ];
    for ((reference, segment, index), entry) in expected.iter().zip(&capture.entries) {
        assert_eq!(format!("E{}", entry.entry_id), *reference);
        assert_eq!(entry.source.segment_id, segment.to_string());
        assert_eq!(entry.source.entry_index, *index);
    }
    assert_eq!(capture.entries.len(), expected.len() + 1); // final cancelled boundary at child gap 7
    assert_eq!(capture.entries.last().unwrap().source.entry_index, 7);
    assert!(
        capture
            .recording_text(2)
            .unwrap()
            .2
            .contains("corrected full original")
    );
    let reader = read(temp.path(), session, compact);
    for (entry, descriptor) in capture.entries.iter().zip(reader.descriptors()) {
        assert_eq!(capture.recording_text(entry.order).unwrap().0, descriptor);
    }
    let decoded = CaptureDto::decode(
        &capture.encode(Default::default()).unwrap(),
        Default::default(),
    )
    .unwrap();
    assert_eq!(decoded, capture);
    let slice = decoded
        .recording_slice(
            RecordingPosition {
                canonical_order: 2,
                byte_offset: 0,
            },
            5,
            16 * 1024,
        )
        .unwrap();
    assert_eq!(
        slice
            .capture
            .entries
            .iter()
            .map(|e| e.source.entry_index)
            .collect::<Vec<_>>(),
        vec![6, 7, 7, 7, 4]
    );
    slice.capture.validate(Default::default()).unwrap();
    let mut alias = capture.clone();
    alias.entries[1].source.entry_index = 1;
    assert!(matches!(
        alias.validate(Default::default()),
        Err(CaptureError::Invalid("duplicate source position"))
    ));
    let mut reverse = capture.clone();
    reverse.entries[2].source.entry_index = 2;
    assert!(matches!(
        reverse.validate(Default::default()),
        Err(CaptureError::Invalid("recording source position order"))
    ));
    let mut reversed_gap = capture;
    reversed_gap.entries[3].source.entry_index = 5;
    assert!(matches!(
        reversed_gap.validate(Default::default()),
        Err(CaptureError::Invalid("recording source position order"))
    ));
}

#[test]
fn recording_catalog_guarded_is_descriptor_only_beyond_body_capture_budget_in_both_layouts() {
    use crate::capture::*;
    for worker_layout in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        let mut records = vec![seed(
            session,
            vec![
                system_context("hidden", "private", crate::InputCompactionUse::Retain),
                message("u", LoggedRole::User, "request"),
            ],
        )];
        records.push(invoke());
        for i in 0..130 {
            records.push(output(&format!("a-{i}"), &"漢字🍎\n\"".repeat(400)));
        }
        records.push(cancelled());
        if worker_layout {
            WorkerSessionStore::new_with_retention(&root, retention.clone())
                .unwrap()
                .create_segment(session, segment, &records)
                .unwrap();
        } else {
            FsStore::new_with_retention(&root, retention.clone())
                .unwrap()
                .create_segment(session, segment, &records)
                .unwrap();
        }
        let guard = retention.lock_live(session).unwrap();
        let inspect = |descriptors: &[PublicMaterialDescriptor]| descriptors.to_vec();
        let catalog = if worker_layout {
            recording_catalog_worker_guarded(
                &root,
                session,
                segment,
                Default::default(),
                &guard,
                inspect,
            )
        } else {
            recording_catalog_fs_guarded(
                &root,
                session,
                segment,
                Default::default(),
                &guard,
                inspect,
            )
        }
        .unwrap();
        assert_eq!(catalog.len(), 132); // user, 130 answers, typed Host boundary
        assert_eq!(catalog[0].identity.entry_ref, "Eu");
        assert_eq!(catalog[131].origin, PublicMaterialOrigin::HostRunBoundary);
        assert!(catalog.iter().map(|d| d.total_bytes).sum::<u64>() > 128 * 1024);
        assert!(
            catalog
                .iter()
                .enumerate()
                .all(|(i, d)| d.canonical_order == i as u64)
        );
        // This is an admission scan, not a full body DTO with a huge transient ceiling.
        let small = CaptureLimits {
            max_bytes: 8192,
            ..Default::default()
        };
        let full = if worker_layout {
            capture_recording_worker_guarded(
                &root,
                session,
                segment,
                Default::default(),
                small,
                &guard,
            )
        } else {
            capture_recording_fs_guarded(&root, session, segment, Default::default(), small, &guard)
        };
        assert!(matches!(full, Err(PublicMaterialError::ResourceLimit)));
        // Descriptor identities/order/digests are exactly those of the bounded body path.
        for order in [0, 1, 130, 131] {
            let start = RecordingPosition {
                canonical_order: order,
                byte_offset: 0,
            };
            let slice = if worker_layout {
                capture_recording_slice_worker_guarded(
                    &root,
                    session,
                    segment,
                    Default::default(),
                    start,
                    1,
                    4096,
                    &guard,
                )
            } else {
                capture_recording_slice_fs_guarded(
                    &root,
                    session,
                    segment,
                    Default::default(),
                    start,
                    1,
                    4096,
                    &guard,
                )
            }
            .unwrap();
            assert_eq!(
                slice.capture.recording_text(order).unwrap().0,
                &catalog[order as usize]
            );
            assert!(serde_json::to_vec(&slice).unwrap().len() <= 4096);
        }
    }
}

#[test]
fn recording_catalog_callback_keeps_same_external_guard_and_persisted_selector() {
    use crate::capture::*;
    use crate::{
        WorkerActiveSegmentRef, WorkerAggregateStore, WorkerMetadata, WorkerMetadataStore,
    };
    for worker_layout in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let control = temp.path().join("control");
        let retention = crate::retention::SessionRetentionStore::new(&control);
        let session = crate::new_session_id();
        let selected = crate::new_segment_id();
        let unrelated_newer = crate::new_segment_id();
        for (segment, id) in [(selected, "selected"), (unrelated_newer, "not-active")] {
            let records = [seed(session, vec![message(id, LoggedRole::User, id)])];
            if worker_layout {
                WorkerSessionStore::new_with_retention(&root, retention.clone())
                    .unwrap()
                    .create_segment(session, segment, &records)
                    .unwrap();
            } else {
                FsStore::new_with_retention(&root, retention.clone())
                    .unwrap()
                    .create_segment(session, segment, &records)
                    .unwrap();
            }
        }
        let metadata_root = temp.path().join("worker-metadata");
        let metadata = WorkerAggregateStore::new(&metadata_root, "test-worker").unwrap();
        let guard = retention.lock_live(session).unwrap();
        metadata
            .write(&WorkerMetadata::new(
                "test-worker",
                Some(WorkerActiveSegmentRef::active_segment(session, selected)),
            ))
            .unwrap();
        // Simulate with_recording_boundary: persisted pointer resolution and descriptor
        // mapping share exactly the same guard. Neither call selects the newest file.
        let active = WorkerAggregateStore::open_read_only(&metadata_root, "test-worker")
            .unwrap()
            .read_read_only()
            .unwrap()
            .unwrap()
            .active
            .unwrap();
        assert_eq!(active.session_id, session);
        let inspect = |descriptors: &[PublicMaterialDescriptor]| -> Result<Vec<_>, &'static str> {
            // Direct non-blocking check of the exact external fence, during callback.
            // Publishers, active-pointer changes and erasure all use this same lock.
            let probe = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(control.join(format!("{session}.lock")))
                .unwrap();
            assert!(!fs4::fs_std::FileExt::try_lock_exclusive(&probe).unwrap());
            assert_eq!(descriptors.len(), 1);
            assert_eq!(descriptors[0].identity.segment_id, selected);
            assert_eq!(descriptors[0].identity.entry_ref, "Eselected");
            Ok(descriptors.to_vec())
        };
        let catalog = if worker_layout {
            recording_catalog_worker_guarded(
                &root,
                session,
                active.segment_id.unwrap(),
                Default::default(),
                &guard,
                inspect,
            )
        } else {
            recording_catalog_fs_guarded(
                &root,
                session,
                active.segment_id.unwrap(),
                Default::default(),
                &guard,
                inspect,
            )
        }
        .unwrap()
        .unwrap();
        let callback_error = if worker_layout {
            recording_catalog_worker_guarded(
                &root,
                session,
                selected,
                Default::default(),
                &guard,
                |_| Err::<(), _>("host-error"),
            )
        } else {
            recording_catalog_fs_guarded(
                &root,
                session,
                selected,
                Default::default(),
                &guard,
                |_| Err::<(), _>("host-error"),
            )
        };
        assert_eq!(callback_error.unwrap(), Err("host-error"));
        // The API borrows, not replaces/drops, the caller's guard.
        let probe = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(control.join(format!("{session}.lock")))
            .unwrap();
        assert!(!fs4::fs_std::FileExt::try_lock_exclusive(&probe).unwrap());
        drop(guard);
        assert!(fs4::fs_std::FileExt::try_lock_exclusive(&probe).unwrap());
        fs4::fs_std::FileExt::unlock(&probe).unwrap();
        retention
            .delete(
                session,
                crate::retention::SessionDeletionIntent::ErasePermanently,
                || {
                    std::fs::remove_dir_all(&root)?;
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(
            retention.lock_live(session),
            Err(crate::StoreError::SessionErased(_))
        ));
        // Descriptor results contain no source body and do not authorize post-erase reads/adoption.
        assert_eq!(catalog[0].identity.entry_ref, "Eselected");
    }
}

#[test]
fn recording_catalog_errors_do_not_invoke_callback_or_relax_resource_and_identity_checks() {
    use crate::capture::*;
    for worker_layout in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("source");
        let retention = crate::retention::SessionRetentionStore::new(temp.path().join("control"));
        let session = crate::new_session_id();
        let segment = crate::new_segment_id();
        let records = [seed(
            session,
            vec![message("u", LoggedRole::User, "request")],
        )];
        if worker_layout {
            WorkerSessionStore::new_with_retention(&root, retention.clone())
                .unwrap()
                .create_segment(session, segment, &records)
                .unwrap();
        } else {
            FsStore::new_with_retention(&root, retention.clone())
                .unwrap()
                .create_segment(session, segment, &records)
                .unwrap();
        }
        let guard = retention.lock_live(session).unwrap();
        let other_session = crate::new_session_id();
        let other_guard = retention.lock_live(other_session).unwrap();
        let open = |session, segment, limits, guard| {
            if worker_layout {
                recording_catalog_worker_guarded(&root, session, segment, limits, guard, |_| {
                    panic!("callback must not run for rejected catalog")
                })
            } else {
                recording_catalog_fs_guarded(&root, session, segment, limits, guard, |_| {
                    panic!("callback must not run for rejected catalog")
                })
            }
        };
        assert!(matches!(
            open(session, segment, Default::default(), &other_guard),
            Err(PublicMaterialError::Inconsistent)
        ));
        for limits in [
            PublicMaterialLimits {
                max_storage_bytes: 1,
                ..Default::default()
            },
            PublicMaterialLimits {
                max_record_bytes: 1,
                ..Default::default()
            },
            PublicMaterialLimits {
                max_entries: 0,
                ..Default::default()
            },
            PublicMaterialLimits {
                max_segments: 0,
                ..Default::default()
            },
        ] {
            assert!(matches!(
                open(session, segment, limits, &guard),
                Err(PublicMaterialError::ResourceLimit)
            ));
        }
        assert!(open(session, crate::new_segment_id(), Default::default(), &guard).is_err());
        // Even a guard matching a claimed Session cannot bless another stored Session.
        if worker_layout {
            assert!(matches!(
                open(other_session, segment, Default::default(), &other_guard),
                Err(PublicMaterialError::Inconsistent)
            ));
        } else {
            let impostor = root.join(other_session.to_string());
            std::fs::create_dir(&impostor).unwrap();
            std::fs::copy(
                root.join(session.to_string())
                    .join(format!("{segment}.jsonl")),
                impostor.join(format!("{segment}.jsonl")),
            )
            .unwrap();
            assert!(matches!(
                open(other_session, segment, Default::default(), &other_guard),
                Err(PublicMaterialError::Inconsistent)
            ));
        }
    }
}
