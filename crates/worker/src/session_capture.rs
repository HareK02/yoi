//! Host consumers share the pure immutable Session capture implementation.
//! Live authorization, append fences, and recapture remain with the calling Host.

pub use session_store::capture::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_history::WorkerHistoryProvenance;
    use agen::Item;
    use protocol::{SessionEntryProvenance, SessionSnapshot};

    #[test]
    fn flow_user_role_is_excluded_while_explicit_human_origin_remains_evidence() {
        let entries = vec![
            crate::session_history::history_entry(
                Item::user_message("trusted flow instruction"),
                WorkerHistoryProvenance::FlowInstruction {
                    selector: "builtin:coder-review".into(),
                    definition_id: "coder-review".into(),
                    definition_digest: Some("sha256:flow-source".into()),
                    instance_id: "instance".into(),
                    state_id: "implement".into(),
                },
            ),
            crate::session_history::history_entry(
                Item::user_message("remember my preference"),
                WorkerHistoryProvenance::HumanInput {
                    account_id: "account-1".into(),
                },
            ),
        ];
        let capture = SessionCapture::from_history_entries("session", "segment", entries);
        let overview = capture.overview();
        assert_eq!(overview.len(), 1);
        assert!(matches!(
            overview[0].origin,
            SessionEntryProvenance::HumanInput
        ));
        let evidence = capture.evidence_for(overview[0].id.as_str()).unwrap();
        assert!(evidence.excerpt.ends_with("remember my preference"));
        assert!(matches!(
            evidence.origin,
            SessionEntryProvenance::HumanInput
        ));
    }

    #[test]
    fn stable_logical_ref_survives_retention_and_restore_projection() {
        let retained = crate::session_history::history_entry(
            Item::assistant_message("retained"),
            WorkerHistoryProvenance::ModelOutput {
                worker: crate::session_history::worker_subject(Default::default()),
            },
        );
        let expected_ref = SessionEntryRef::from_history_entry_id(&retained.annotation.entry_id);
        let before = SessionCapture::from_history_entries("session", "old", vec![retained.clone()]);
        let after = SessionCapture::from_history_entries("session", "new", vec![retained]);
        assert_eq!(before.overview()[0].id, expected_ref);
        assert_eq!(after.overview()[0].id, expected_ref);
        assert_eq!(
            after.evidence_for(expected_ref.as_str()).unwrap().entry_ref,
            expected_ref
        );
    }

    #[test]
    fn host_summary_remains_private_to_compaction_not_observation_or_memory_evidence() {
        let entries = [
            (
                Item::system_message("host-summary-secret"),
                WorkerHistoryProvenance::CompactionSummary,
            ),
            (
                Item::system_message("other-derived-secret"),
                WorkerHistoryProvenance::DerivedSummary,
            ),
            (
                // Exclusion is driven by origin, not role or instructions in the body.
                Item::user_message("prior-journal-secret"),
                WorkerHistoryProvenance::JournalContext,
            ),
            (
                Item::system_message("raw-system-secret"),
                WorkerHistoryProvenance::LegacyUnknown,
            ),
            (
                Item::reasoning("reasoning-secret"),
                WorkerHistoryProvenance::LegacyUnknown,
            ),
        ]
        .into_iter()
        .map(|(item, origin)| crate::session_history::history_entry(item, origin))
        .collect::<Vec<_>>();
        let refs = entries
            .iter()
            .map(|entry| SessionEntryRef::from_history_entry_id(&entry.annotation.entry_id))
            .collect::<Vec<_>>();
        let capture = SessionCapture::from_history_entries("session", "segment", entries);
        assert!(capture.overview().is_empty());
        assert!(
            capture
                .search(&SearchOptions {
                    query: "secret".into(),
                    ..SearchOptions::default()
                })
                .is_empty()
        );
        assert!(
            capture
                .read(ReadSelector::EntryRange([0, 4]), ReadOptions::default())
                .entries
                .is_empty()
        );
        for reference in refs {
            assert!(capture.evidence_for(reference.as_str()).is_none());
            assert!(
                capture
                    .read(ReadSelector::Id(reference.as_str()), ReadOptions::default())
                    .entries
                    .is_empty()
            );
        }
    }

    #[test]
    fn generic_public_input_observation_and_memory_policies_survive_store_restore_independently() {
        use session_store::{
            FeatureInputPayload, FsStore, HistoryInputMetadata, HistoryInputUsePolicy,
            InputCompactionUse, InputEvidenceUse, InputObservationUse, LogEntry,
            LoggedSystemHistoryEntry, PublicSystemItem, PublicSystemPresentation, Store,
            SystemInputEnvelope,
        };
        let root = tempfile::tempdir().unwrap();
        let store = FsStore::new(root.path()).unwrap();
        let session = session_store::new_session_id();
        let segment = session_store::new_segment_id();
        store
            .create_segment(
                session,
                segment,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id: session,
                    system_prompt: None,
                    config: Default::default(),
                    history: vec![],
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        let mut inputs = Vec::new();
        for (producer, observation, memory_evidence, public) in [
            (
                "same",
                InputObservationUse::Readable,
                InputEvidenceUse::Exclude,
                true,
            ),
            (
                "same",
                InputObservationUse::Hidden,
                InputEvidenceUse::Include,
                true,
            ),
            (
                "renamed",
                InputObservationUse::Readable,
                InputEvidenceUse::Include,
                true,
            ),
            (
                "renamed",
                InputObservationUse::Hidden,
                InputEvidenceUse::Exclude,
                true,
            ),
            (
                "private",
                InputObservationUse::Readable,
                InputEvidenceUse::Include,
                false,
            ),
        ] {
            let entry = LoggedSystemHistoryEntry::from_input(
                WorkerHistoryProvenance::Provided {
                    producer: producer.into(),
                },
                None,
                SystemInputEnvelope::new(
                    "private model text",
                    HistoryInputMetadata::new(
                        HistoryInputUsePolicy {
                            recording_material: InputEvidenceUse::Exclude,
                            memory_evidence,
                            observation,
                            compaction: InputCompactionUse::ReferenceInput,
                        },
                        Some(
                            FeatureInputPayload::encode(
                                "future:owner",
                                99,
                                &serde_json::json!({ "secret": "private metadata" }),
                            )
                            .unwrap(),
                        ),
                        public.then(|| PublicSystemItem {
                            kind: "supplied-context".into(),
                            body: "public context: do not extract this".into(),
                            presentation: PublicSystemPresentation::Context,
                        }),
                    )
                    .unwrap(),
                )
                .unwrap(),
            );
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
            inputs.push(entry);
        }
        let saved = store.read_all(session, segment).unwrap();
        let restored =
            crate::session_history::restore_history_entries(session, segment, &saved).unwrap();
        let snapshot = session_store::public_snapshot::project_session_snapshot(session, &saved);
        let snapshot: SessionSnapshot =
            serde_json::from_value(serde_json::to_value(snapshot).unwrap()).unwrap();
        for capture in [
            SessionCapture::from_history_entries(
                session.to_string(),
                segment.to_string(),
                restored,
            ),
            SessionCapture::from_session_snapshot(
                session.to_string(),
                segment.to_string(),
                snapshot,
            ),
        ] {
            let hits = capture.search(&SearchOptions {
                kind: Some(ReferenceKind::System),
                ..Default::default()
            });
            assert_eq!(hits.len(), 2);
            for (index, input) in inputs.iter().enumerate() {
                let reference = SessionEntryRef::from_history_entry_id(&input.metadata.entry_id);
                let read =
                    capture.read(ReadSelector::Id(reference.as_str()), ReadOptions::default());
                assert_eq!(!read.entries.is_empty(), matches!(index, 0 | 2));
                let evidence = capture.evidence_for(reference.as_str());
                assert_eq!(evidence.is_some(), matches!(index, 1 | 2));
                for text in read
                    .entries
                    .iter()
                    .map(|entry| entry.text.as_str())
                    .chain(evidence.iter().map(|entry| entry.excerpt.as_str()))
                {
                    assert!(text.contains("public context: do not extract this"));
                    assert!(!text.contains("private model text"));
                    assert!(!text.contains("private metadata"));
                    assert!(!text.contains("future:owner"));
                }
            }
        }
    }

    // Real FsStore seed/fork/reopen -> ordinary history replay, public snapshot,
    // capture and both evidence consumers; no Feature or Tool execution occurs.
    #[test]
    fn tool_policy_axes_survive_restore_and_fork_without_exposing_derived_context() {
        use session_store::public_material::{
            PublicMaterialLimits, PublicMaterialUse, SessionPublicMaterialReader,
        };
        use session_store::*;
        for recording in [InputEvidenceUse::Include, InputEvidenceUse::Exclude] {
            for memory in [InputEvidenceUse::Include, InputEvidenceUse::Exclude] {
                for observation in [InputObservationUse::Readable, InputObservationUse::Hidden] {
                    let root = tempfile::tempdir().unwrap();
                    let store = FsStore::new(root.path()).unwrap();
                    let session = new_session_id();
                    let first = new_segment_id();
                    let fork = new_segment_id();
                    let items = vec![
                        Item::tool_call("original", "ArbitraryTool", "{}"),
                        Item::tool_result("original", "original result"),
                        Item::tool_call("derived", "ArbitraryTool", "PRIVATE_SYNTHETIC_CONTEXT"),
                        Item::tool_result("derived", "PRIVATE_SYNTHETIC_RESULT"),
                    ];
                    let mut history: Vec<_> = items
                        .iter()
                        .cloned()
                        .map(|item| LoggedHistoryEntry {
                            item: LoggedItem::from(item),
                            metadata: LoggedSessionHistoryMetadata::legacy_unknown(),
                        })
                        .collect();
                    let sources = history[..2]
                        .iter()
                        .map(|entry| entry.metadata.entry_id.clone())
                        .collect::<Vec<_>>();
                    for entry in &mut history[..2] {
                        entry.metadata.input_metadata = Some(
                            HistoryInputMetadata::new(
                                HistoryInputUsePolicy {
                                    recording_material: recording,
                                    memory_evidence: memory,
                                    observation,
                                    compaction: InputCompactionUse::Retain,
                                },
                                None,
                                None,
                            )
                            .unwrap(),
                        );
                    }
                    for entry in &mut history[2..] {
                        entry.metadata.derivation = Some(LoggedHistoryDerivation {
                            sources: sources.clone(),
                        });
                        entry.metadata.input_metadata = Some(
                            HistoryInputMetadata::new(
                                HistoryInputUsePolicy {
                                    recording_material: InputEvidenceUse::Exclude,
                                    memory_evidence: InputEvidenceUse::Exclude,
                                    observation: InputObservationUse::Hidden,
                                    compaction: InputCompactionUse::Retain,
                                },
                                None,
                                None,
                            )
                            .unwrap(),
                        );
                    }
                    for segment in [first, fork] {
                        store
                            .create_segment(
                                session,
                                segment,
                                &[LogEntry::AnnotatedSegmentStart {
                                    ts: 1,
                                    session_id: session,
                                    system_prompt: None,
                                    config: Default::default(),
                                    history: history.clone(),
                                    forked_from: (segment == fork).then_some(SegmentOrigin {
                                        segment_id: first,
                                        at_turn_index: 0,
                                    }),
                                    compacted_from: None,
                                }],
                            )
                            .unwrap();
                    }
                    let reopened = FsStore::new(root.path()).unwrap();
                    let saved = reopened.read_all(session, fork).unwrap();
                    let restored =
                        crate::session_history::restore_history_entries(session, fork, &saved)
                            .unwrap();
                    for (index, entry) in restored.iter().enumerate() {
                        assert_eq!(entry.item, items[index]);
                        assert_eq!(entry.annotation, history[index].metadata);
                    }
                    let capture = SessionCapture::from_history_entries(
                        session.to_string(),
                        fork.to_string(),
                        restored,
                    );
                    // Private derived content must be absent even from the saved
                    // body representation, not merely hidden by the UI index.
                    let encoded = serde_json::to_string(&capture.to_dto()).unwrap();
                    assert!(!encoded.contains("PRIVATE_SYNTHETIC"));
                    let snapshot = public_snapshot::project_session_snapshot(session, &saved);
                    assert!(
                        !serde_json::to_string(&snapshot)
                            .unwrap()
                            .contains("PRIVATE_SYNTHETIC")
                    );
                    for capture in [
                        capture,
                        SessionCapture::from_session_snapshot(
                            session.to_string(),
                            fork.to_string(),
                            snapshot,
                        ),
                    ] {
                        assert_eq!(
                            capture
                                .search(&SearchOptions {
                                    kind: Some(ReferenceKind::Tool),
                                    ..Default::default()
                                })
                                .len(),
                            if observation == InputObservationUse::Readable {
                                2
                            } else {
                                0
                            }
                        );
                        for (index, entry) in history.iter().enumerate() {
                            let reference =
                                SessionEntryRef::from_history_entry_id(&entry.metadata.entry_id);
                            assert_eq!(
                                !capture
                                    .read(
                                        ReadSelector::Id(reference.as_str()),
                                        ReadOptions::default()
                                    )
                                    .entries
                                    .is_empty(),
                                index < 2 && observation == InputObservationUse::Readable
                            );
                            assert_eq!(
                                capture.evidence_for(reference.as_str()).is_some(),
                                index < 2 && memory == InputEvidenceUse::Include
                            );
                        }
                    }
                    for purpose in [
                        PublicMaterialUse::Recording,
                        PublicMaterialUse::MemoryEvidence,
                    ] {
                        let reader = SessionPublicMaterialReader::open_fs_for_use(
                            root.path(),
                            session,
                            fork,
                            PublicMaterialLimits::default(),
                            reopened.retention(),
                            purpose,
                        )
                        .unwrap();
                        let ids = reader
                            .descriptors()
                            .map(|entry| entry.identity.entry_ref.clone())
                            .collect::<Vec<_>>();
                        let included = match purpose {
                            PublicMaterialUse::Recording => recording,
                            PublicMaterialUse::MemoryEvidence => memory,
                        } == InputEvidenceUse::Include;
                        assert_eq!(
                            ids,
                            sources
                                .iter()
                                .filter(|_| included)
                                .map(|id| format!("E{}", id.0))
                                .collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn overview_contains_user_and_assistant_only() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::system_message("sys"),
                Item::user_message("hello"),
                Item::assistant_message("progress"),
                Item::tool_call("c1", "Read", "{\"file\":\"x\"}"),
                Item::tool_result("c1", "read ok"),
            ],
        );

        let overview = view.overview();
        assert_eq!(overview.len(), 2);
        assert_eq!(overview[0].kind, ReferenceKind::User);
        assert_eq!(overview[1].kind, ReferenceKind::Assistant);
        assert!(overview[1].text.contains("progress"));
    }

    #[test]
    fn search_filters_tool_input_and_output() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::tool_call("c1", "Read", "{\"file\":\"Cargo.toml\"}"),
                Item::tool_result_with_content("c1", "read ok", "package metadata"),
            ],
        );

        let input_hits = view.search(&SearchOptions {
            query: "Cargo.toml".into(),
            kind: Some(ReferenceKind::Tool),
            tool_part: Some(ToolPart::Input),
            tool_name: Some("Read".into()),
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(input_hits.len(), 1);
        assert_eq!(input_hits[0].tool_part, Some(ToolPart::Input));

        let output_hits = view.search(&SearchOptions {
            query: "package metadata".into(),
            kind: Some(ReferenceKind::Tool),
            tool_part: Some(ToolPart::Output),
            tool_name: None,
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(output_hits.len(), 1);
        assert_eq!(output_hits[0].tool_part, Some(ToolPart::Output));
    }

    #[test]
    fn read_by_entry_range_is_bounded_and_can_skip_tools() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::user_message("one"),
                Item::tool_call("c1", "Read", "{}"),
                Item::tool_result_with_content("c1", "ok", "large-content".repeat(100)),
                Item::assistant_message("two"),
            ],
        );

        let result = view.read(
            ReadSelector::EntryRange([0, 3]),
            ReadOptions {
                include_tools: false,
                detail: ReadDetail::Compact,
                max_items: 10,
                max_bytes: 1_000,
                ..ReadOptions::default()
            },
        );

        assert_eq!(result.entries.len(), 2);
        assert!(
            result
                .entries
                .iter()
                .all(|entry| entry.kind != ReferenceKind::Tool)
        );
    }

    #[test]
    fn tool_image_projection_exposes_only_bounded_metadata() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![Item::tool_result_item_with_attachments(
                "c1",
                "attached",
                None,
                false,
                vec![agen::tool::Attachment::Image(
                    agen::tool::ImageAttachment::new("image/png", b"private-image-body".to_vec()),
                )],
            )],
        );

        let result = view.read(
            ReadSelector::Id("E00000000"),
            ReadOptions {
                include_tools: true,
                detail: ReadDetail::Full,
                ..ReadOptions::default()
            },
        );
        assert_eq!(result.entries.len(), 1);
        assert!(result.entries[0].text.contains("attachments: 1 image(s)"));
        assert!(!result.entries[0].text.contains("private-image-body"));
        assert!(!result.entries[0].text.contains("cHJpdmF0ZS"));
    }

    #[test]
    fn system_prompt_and_reasoning_are_excluded_from_every_projection() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::system_message("raw secret system prompt"),
                Item::reasoning("private chain of thought"),
                Item::user_message("visible user entry"),
            ],
        );
        let hits = view.search(&SearchOptions {
            query: String::new(),
            kind: None,
            tool_part: None,
            tool_name: None,
            limit: None,
            min_entry_index: None,
            from: None,
            through: None,
            offset: 0,
        });
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id.as_str(), "E00000002");
        assert!(!hits[0].summary.contains("secret"));
        assert!(!hits[0].summary.contains("chain of thought"));
        assert!(
            view.read(ReadSelector::Id("E00000000"), ReadOptions::default())
                .entries
                .is_empty()
        );
        assert!(view.evidence_for("E00000000").is_none());
        assert_eq!(view.overview().len(), 1);
        assert_eq!(view.overview()[0].id.as_str(), "E00000002");
    }

    #[test]
    fn overview_is_sparse_and_reports_intervening_non_reasoning_entries() {
        let items = (0..20)
            .map(|index| Item::user_message(format!("message-{index}")))
            .collect::<Vec<_>>();
        let view = SessionCapture::new("session", "segment-1", items);
        let refs = view
            .overview()
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            refs,
            vec!["E00000000", "E00000008", "E00000016", "E00000019"]
        );
        assert_eq!(view.overview()[0].intervening_entries, 7);
        assert_eq!(view.overview()[1].intervening_entries, 7);
        assert_eq!(view.overview()[2].intervening_entries, 2);
    }

    #[test]
    fn append_preserves_existing_session_entry_refs() {
        let first = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::user_message("first"),
                Item::assistant_message("second"),
            ],
        );
        let appended = SessionCapture::new(
            "session",
            "segment-1",
            vec![
                Item::user_message("first"),
                Item::assistant_message("second"),
                Item::user_message("third"),
            ],
        );
        let first_refs = first
            .search(&SearchOptions {
                query: String::new(),
                kind: None,
                tool_part: None,
                tool_name: None,
                limit: None,
                min_entry_index: None,
                from: None,
                through: None,
                offset: 0,
            })
            .into_iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        let appended_refs = appended
            .search(&SearchOptions {
                query: String::new(),
                kind: None,
                tool_part: None,
                tool_name: None,
                limit: None,
                min_entry_index: None,
                from: None,
                through: None,
                offset: 0,
            })
            .into_iter()
            .take(2)
            .map(|entry| entry.id)
            .collect::<Vec<_>>();
        assert_eq!(first_refs, appended_refs);
    }

    #[test]
    fn evidence_projection_uses_entry_range_and_session_entry_ref() {
        let view = SessionCapture::new("session", "segment-1", vec![Item::user_message("hello")]);
        let source = view.evidence_for("E00000000").unwrap();
        assert_eq!(source.segment_id, "segment-1");
        assert_eq!(source.entry_range, [0, 0]);
        assert_eq!(source.entry_ref.as_str(), "E00000000");
    }

    #[test]
    fn committed_tool_call_can_be_resolved_by_provider_call_id() {
        let view = SessionCapture::new(
            "session",
            "segment-1",
            vec![Item::tool_call(
                "call-remember",
                "SubjektivMemoryRemember",
                r#"{"claim":"remember this"}"#,
            )],
        );
        let evidence = view.evidence_for_tool_call("call-remember").unwrap();
        assert_eq!(evidence.entry_ref.as_str(), "E00000000");
        assert_eq!(evidence.tool_part, Some(ToolPart::Input));
        assert!(view.evidence_for_tool_call("missing").is_none());
    }
    // Real Store append and Segment replacement -> Worker replay -> the common
    // fixed value used by Memory and compaction exploration. No live I/O remains
    // available after the owned temporary source is removed.
    #[test]
    fn frozen_memory_and_exploration_survive_append_compaction_and_source_removal() {
        use crate::feature::builtin::memory_staging_output::render_extract_input;
        use crate::feature::builtin::session_explore::SessionExploreState;
        use session_store::{FsStore, LogEntry, SegmentOrigin, Store};
        let root = tempfile::tempdir().unwrap();
        let store = FsStore::new(root.path()).unwrap();
        let session = session_store::new_session_id();
        let first = session_store::new_segment_id();
        let next = session_store::new_segment_id();
        let original = crate::session_history::history_entry(
            Item::user_message("fixed 日本語 intent"),
            WorkerHistoryProvenance::LocalHumanInput,
        );
        let seed = |history, compacted_from| LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id: session,
            system_prompt: Some("PRIVATE_SYSTEM".into()),
            config: Default::default(),
            history,
            forked_from: None,
            compacted_from,
        };
        store
            .create_segment(
                session,
                first,
                &[seed(
                    vec![crate::session_history::to_logged_history_entry(&original)],
                    None,
                )],
            )
            .unwrap();
        let log = store.read_all(session, first).unwrap();
        let restored =
            crate::session_history::restore_history_entries(session, first, &log).unwrap();
        let fixed =
            SessionCapture::from_history_entries(session.to_string(), first.to_string(), restored);
        let overview = fixed.overview()[0].clone();
        let evidence = fixed.evidence_for(overview.id.as_str()).unwrap();
        let extract = render_extract_input(&fixed);
        let encoded = fixed.encode(CaptureLimits::default()).unwrap();
        let state = SessionExploreState::new(fixed.clone());
        store
            .append(
                session,
                first,
                &LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: crate::session_history::to_logged_history_entry(
                        &crate::session_history::history_entry(
                            Item::assistant_message("later append"),
                            WorkerHistoryProvenance::LegacyUnknown,
                        ),
                    ),
                },
            )
            .unwrap();
        let summary = crate::session_history::history_entry(
            Item::system_message("PRIVATE_COMPACTED_SUMMARY"),
            WorkerHistoryProvenance::DerivedSummary,
        );
        store
            .create_segment(
                session,
                next,
                &[seed(
                    vec![crate::session_history::to_logged_history_entry(&summary)],
                    Some(SegmentOrigin {
                        segment_id: first,
                        at_turn_index: 0,
                    }),
                )],
            )
            .unwrap();
        drop(store);
        drop(root);
        assert_eq!(fixed.encode(CaptureLimits::default()).unwrap(), encoded);
        assert_eq!(render_extract_input(state.view()), extract);
        assert_eq!(state.view().overview()[0].id, overview.id);
        assert_eq!(state.view().overview()[0].text, overview.text);
        let retained = state.view().evidence_for(overview.id.as_str()).unwrap();
        assert_eq!(retained.entry_range, evidence.entry_range);
        assert_eq!(retained.excerpt, evidence.excerpt);
        assert!(
            state
                .view()
                .search(&SearchOptions {
                    query: "later".into(),
                    ..Default::default()
                })
                .is_empty()
        );
        let restored = SessionCapture::decode(&encoded, CaptureLimits::default()).unwrap();
        let scoped = fixed.scoped_ref(&overview.id).unwrap();
        assert_eq!(
            restored
                .resolve_ref(&scoped, CapturePurpose::MemoryEvidence)
                .unwrap()
                .entry_id,
            original.annotation.entry_id.0
        );
        assert!(!String::from_utf8(encoded).unwrap().contains("PRIVATE_"));
    }
}
