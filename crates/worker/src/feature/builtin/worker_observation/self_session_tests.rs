//! Real FsStore -> local lineage capture -> observation tools; no Workspace or
//! remote provider is allowed to service self reads.
use super::*;
use crate::session_capture::SearchOptions;
use crate::session_history::{WorkerHistoryProvenance, history_entry, to_logged_history_entry};
use agen::Item;
use session_store::{FsStore, LoggedHistoryEntry, SegmentOrigin};

fn message(text: &str) -> LoggedHistoryEntry {
    to_logged_history_entry(&history_entry(
        Item::user_message(text),
        WorkerHistoryProvenance::LocalHumanInput,
    ))
}

fn seed(
    session_id: SessionId,
    history: Vec<LoggedHistoryEntry>,
    compacted_from: Option<SegmentOrigin>,
    forked_from: Option<SegmentOrigin>,
) -> LogEntry {
    LogEntry::AnnotatedSegmentStart {
        ts: 0,
        session_id,
        history,
        system_prompt: Some("PRIVATE_SYSTEM_PROMPT".into()),
        config: Default::default(),
        compacted_from,
        forked_from,
    }
}

fn summary(sources: Vec<session_store::LoggedSessionHistoryEntryId>) -> LoggedHistoryEntry {
    let input = session_store::SystemInputEnvelope::new(
        "PRIVATE_COMPACTION_SUMMARY",
        session_store::HistoryInputMetadata::new(
            session_store::HistoryInputUsePolicy {
                recording_material: session_store::InputEvidenceUse::Exclude,
                memory_evidence: session_store::InputEvidenceUse::Exclude,
                observation: session_store::InputObservationUse::Hidden,
                compaction: session_store::InputCompactionUse::OpeningSummary,
            },
            None,
            None,
        )
        .unwrap(),
    )
    .unwrap();
    session_store::LoggedSystemHistoryEntry::from_input(
        WorkerHistoryProvenance::Provided {
            producer: "worker.compaction".into(),
        },
        Some(session_store::LoggedHistoryDerivation { sources }),
        input,
    )
    .to_history_entry()
}

#[test]
fn local_capture_keeps_original_refs_and_source_through_compaction_and_fork() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = SessionId::now_v7();
    let original = SegmentId::now_v7();
    let compacted = SegmentId::now_v7();
    let forked = SegmentId::now_v7();
    let first = message("original evidence 日本語");
    store
        .create_segment(
            session,
            original,
            &[seed(session, vec![first.clone()], None, None)],
        )
        .unwrap();
    let before = capture(&store, session, original).unwrap();
    let reference = before.overview()[0].id.clone();
    // This late parent tail and a sibling are outside the adopted source prefix.
    store
        .append(
            session,
            original,
            &LogEntry::AnnotatedUserInput {
                ts: 1,
                history: vec![message("UNADOPTED_TAIL")],
                segments: vec![],
                extensions: vec![],
            },
        )
        .unwrap();
    store
        .create_segment(
            session,
            SegmentId::now_v7(),
            &[seed(session, vec![message("SIBLING_SECRET")], None, None)],
        )
        .unwrap();
    let opening = summary(vec![first.metadata.entry_id.clone()]);
    let retained = message("after compaction");
    store
        .create_segment(
            session,
            compacted,
            &[seed(
                session,
                vec![opening.clone(), retained.clone()],
                Some(SegmentOrigin {
                    segment_id: original,
                    at_turn_index: 0,
                }),
                None,
            )],
        )
        .unwrap();
    store
        .create_segment(
            session,
            forked,
            &[seed(
                session,
                vec![opening, retained],
                None,
                Some(SegmentOrigin {
                    segment_id: compacted,
                    at_turn_index: 0,
                }),
            )],
        )
        .unwrap();
    let view = capture(&store, session, forked).unwrap();
    let evidence = view.evidence_for(reference.as_str()).unwrap();
    assert_eq!(evidence.segment_id, original.to_string());
    assert!(evidence.excerpt.contains("original evidence 日本語"));
    assert_eq!(
        view.search(&SearchOptions {
            query: "after compaction".into(),
            ..Default::default()
        })
        .len(),
        1
    );
    for hidden in [
        "UNADOPTED_TAIL",
        "SIBLING_SECRET",
        "PRIVATE_SYSTEM_PROMPT",
        "PRIVATE_COMPACTION_SUMMARY",
    ] {
        assert!(
            view.search(&SearchOptions {
                query: hidden.into(),
                ..Default::default()
            })
            .is_empty(),
            "{hidden}"
        );
    }
    store
        .append(
            session,
            forked,
            &LogEntry::AnnotatedUserInput {
                ts: 2,
                history: vec![message("fresh append")],
                segments: vec![],
                extensions: vec![],
            },
        )
        .unwrap();
    let fresh = capture(&store, session, forked).unwrap();
    assert!(
        fresh
            .evidence_for(reference.as_str())
            .unwrap()
            .excerpt
            .contains("original evidence")
    );
    assert_eq!(
        fresh
            .search(&SearchOptions {
                query: "fresh append".into(),
                ..Default::default()
            })
            .len(),
        1
    );
}

#[tokio::test]
async fn self_tools_read_latest_local_log_and_reject_selectors_without_remote_access() {
    use crate::feature::builtin::worker_observation::{
        overview_definition, read_definition, search_definition,
    };
    use crate::feature::contracts::{
        WorkerObservationError, WorkerObservationProvider, WorkerObservationSubject,
        WorkerObservationSubjectRef, WorkerSessionCapture,
    };
    struct NoRemote;
    #[async_trait::async_trait]
    impl WorkerObservationProvider for NoRemote {
        async fn list_worker_sessions(
            &self,
        ) -> Result<Vec<WorkerObservationSubject>, WorkerObservationError> {
            panic!("self must not discover Workers")
        }
        async fn capture_worker_session(
            &self,
            _: &WorkerObservationSubjectRef,
        ) -> Result<WorkerSessionCapture, WorkerObservationError> {
            panic!("self must not contact a remote provider")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let store = FsStore::new(root.path()).unwrap();
    let session = SessionId::now_v7();
    let segment = SegmentId::now_v7();
    store
        .create_segment(
            session,
            segment,
            &[seed(session, vec![message("local proof")], None, None)],
        )
        .unwrap();
    let bound = store.clone();
    let reader = SelfSessionReader::new(move || capture(&bound, session, segment));
    let remote = Arc::new(NoRemote);
    let (_, overview) = overview_definition(remote.clone(), Some(reader.clone()))();
    let output = overview
        .execute(
            r#"{"target":{"kind":"self"}}"#,
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    let page: serde_json::Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(page["target"]["kind"], "self");
    let entry_ref = page["entries"][0]["entry_ref"].as_str().unwrap();
    let read = read_definition(remote.clone(), Some(reader.clone()))().1;
    let result = read
        .execute(
            &serde_json::json!({"target":{"kind":"self"},"entry_ref":entry_ref,"mode":"full"})
                .to_string(),
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert!(result.content.unwrap().contains("local proof"));
    store
        .append(
            session,
            segment,
            &LogEntry::AnnotatedUserInput {
                ts: 1,
                history: vec![message("new proof")],
                segments: vec![],
                extensions: vec![],
            },
        )
        .unwrap();
    let search = search_definition(remote.clone(), Some(reader))().1;
    let result = search
        .execute(
            r#"{"target":{"kind":"self"},"query":"new proof"}"#,
            agen::tool::ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert!(result.content.unwrap().contains("new proof"));
    for invalid in [
        r#"{"subject":{"kind":"self"}}"#,
        r#"{"subject":{"kind":"self"},"target":{"kind":"self"}}"#,
        r#"{"target":{"kind":"self","worker_id":"other"}}"#,
        r#"{"target":{"kind":"self"},"session_id":"other"}"#,
    ] {
        assert!(
            overview
                .execute(invalid, agen::tool::ToolExecutionContext::direct())
                .await
                .is_err()
        );
    }
    let disabled = overview_definition(remote, None)().1;
    assert!(
        disabled
            .execute(
                r#"{"target":{"kind":"self"}}"#,
                agen::tool::ToolExecutionContext::direct()
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("disabled")
    );
}

#[test]
fn adopted_trailing_correction_replaces_result_but_later_branch_correction_is_excluded() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = SessionId::now_v7();
    let parent = SegmentId::now_v7();
    let active = SegmentId::now_v7();
    let original = to_logged_history_entry(&history_entry(
        Item::tool_result("call", "obsolete result"),
        WorkerHistoryProvenance::LegacyUnknown,
    ));
    let mut corrected = original.clone();
    corrected.item = Item::tool_result("call", "corrected evidence").into();
    store
        .create_segment(
            session,
            parent,
            &[
                seed(session, vec![original.clone()], None, None),
                LogEntry::ToolResultCorrected {
                    ts: 1,
                    entry: corrected,
                },
            ],
        )
        .unwrap();
    store
        .create_segment(
            session,
            active,
            &[seed(
                session,
                vec![summary(vec![original.metadata.entry_id.clone()])],
                Some(SegmentOrigin {
                    segment_id: parent,
                    at_turn_index: 0,
                }),
                None,
            )],
        )
        .unwrap();
    let mut late = original;
    late.item = Item::tool_result("call", "unadopted replacement").into();
    store
        .append(
            session,
            parent,
            &LogEntry::AnnotatedAssistantItem {
                ts: 2,
                entry: message("later branch"),
            },
        )
        .unwrap();
    store
        .append(
            session,
            parent,
            &LogEntry::ToolResultCorrected { ts: 3, entry: late },
        )
        .unwrap();
    let view = capture(&store, session, active).unwrap();
    assert_eq!(
        view.search(&SearchOptions {
            query: "corrected evidence".into(),
            ..Default::default()
        })
        .len(),
        1
    );
    for excluded in ["obsolete result", "unadopted replacement", "later branch"] {
        assert!(
            view.search(&SearchOptions {
                query: excluded.into(),
                ..Default::default()
            })
            .is_empty()
        );
    }
}

#[test]
fn inconsistent_or_missing_ancestry_fails_without_scanning_other_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let store = FsStore::new(temp.path()).unwrap();
    let session = SessionId::now_v7();
    let parent = SegmentId::now_v7();
    let active = SegmentId::now_v7();
    let original = message("original");
    store
        .create_segment(
            session,
            parent,
            &[seed(session, vec![original], None, None)],
        )
        .unwrap();
    for bad in [SegmentId::now_v7(), parent, active] {
        store
            .create_segment(
                session,
                active,
                &[seed(
                    session,
                    vec![message("mismatched fork")],
                    None,
                    Some(SegmentOrigin {
                        segment_id: bad,
                        at_turn_index: 0,
                    }),
                )],
            )
            .unwrap();
        assert!(capture(&store, session, active).is_err());
    }
    let foreign = SessionId::now_v7();
    store
        .create_segment(
            session,
            active,
            &[seed(foreign, vec![message("foreign")], None, None)],
        )
        .unwrap();
    assert!(capture(&store, session, active).is_err());
}

#[test]
fn adopted_prefix_matches_logical_identities_but_rejects_conflicting_duplicates() {
    let session = SessionId::now_v7();
    let first = message("first adopted proof");
    let second = message("second adopted proof");
    let suffix = message("unadopted proof");
    let expected = vec![
        first.metadata.entry_id.0.clone(),
        second.metadata.entry_id.0.clone(),
    ];
    for batch in [false, true] {
        let log = if batch {
            vec![seed(
                session,
                vec![first.clone(), first.clone(), second.clone(), suffix.clone()],
                None,
                None,
            )]
        } else {
            vec![
                seed(session, vec![first.clone()], None, None),
                LogEntry::AnnotatedAssistantItem {
                    ts: 1,
                    entry: first.clone(),
                },
                LogEntry::AnnotatedAssistantItem {
                    ts: 2,
                    entry: second.clone(),
                },
                LogEntry::AnnotatedAssistantItem {
                    ts: 3,
                    entry: suffix.clone(),
                },
            ]
        };
        let prefix = adopted_prefix(log, &expected).unwrap();
        let restored =
            crate::session_history::restore_history_entries(session, SegmentId::now_v7(), &prefix)
                .unwrap();
        assert_eq!(restored.len(), 3);
        assert_eq!(restored[2].annotation.entry_id, second.metadata.entry_id);
        assert!(
            !restored
                .iter()
                .any(|entry| entry.annotation.entry_id == suffix.metadata.entry_id)
        );
    }
    let mut conflicting = first.clone();
    conflicting.item = Item::user_message("changed body without a correction").into();
    let error = adopted_prefix(
        vec![seed(session, vec![first, conflicting, second], None, None)],
        &expected,
    )
    .unwrap_err();
    assert!(error.contains("conflicting content"), "{error}");
}
