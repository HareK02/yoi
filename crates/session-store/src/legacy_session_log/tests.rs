use super::*;

mod v5;

fn seed(
    origin: LoggedSessionHistoryOrigin,
    compacted: bool,
    role: crate::LoggedRole,
    text: &str,
) -> LogEntry {
    LogEntry::AnnotatedSegmentStart {
        ts: 1,
        session_id: SessionId::now_v7(),
        system_prompt: None,
        config: Default::default(),
        forked_from: None,
        compacted_from: compacted.then(|| SegmentOrigin {
            segment_id: SegmentId::now_v7(),
            at_turn_index: 0,
        }),
        history: vec![LoggedHistoryEntry {
            item: LoggedItem::Message {
                role,
                content: vec![crate::LoggedContentPart::Text { text: text.into() }],
            },
            metadata: LoggedSessionHistoryMetadata {
                system_input: None,
                entry_id: LoggedSessionHistoryEntryId("summary".into()),
                origin,
                derivation: Some(crate::LoggedHistoryDerivation {
                    sources: vec![LoggedSessionHistoryEntryId("source".into())],
                }),
            },
        }],
    }
}

#[test]
fn only_trusted_compaction_seed_is_migrated_and_retries_update_inherited_copies() {
    let text = "[Compacted context summary]\n\nsummary";
    for (compacted, role, text) in [
        (false, crate::LoggedRole::System, text),
        (true, crate::LoggedRole::User, text),
        (
            true,
            crate::LoggedRole::System,
            "[Session TaskStore snapshot] tasks",
        ),
    ] {
        let entry = seed(
            LoggedSessionHistoryOrigin::DerivedSummary,
            compacted,
            role,
            text,
        );
        assert!(compaction_summary_ids([&entry].into_iter()).is_empty());
    }
    for origin in [
        LoggedSessionHistoryOrigin::DerivedSummary,
        LoggedSessionHistoryOrigin::CompactionSummary,
    ] {
        let creation = seed(origin, true, crate::LoggedRole::System, text);
        let inherited = seed(
            LoggedSessionHistoryOrigin::DerivedSummary,
            false,
            crate::LoggedRole::System,
            text,
        );
        let LogEntry::AnnotatedSegmentStart { history, .. } = &inherited else {
            panic!()
        };
        let mut pasted = history[0].clone();
        pasted.metadata.entry_id = LoggedSessionHistoryEntryId("pasted".into());
        let appended = LogEntry::AnnotatedUserInput {
            ts: 2,
            history: vec![pasted.clone()],
            segments: vec![],
            extensions: vec![],
        };
        let mut entries = vec![creation, inherited, appended];
        let ids = compaction_summary_ids(entries.iter());
        migrate_compaction_origins(&mut entries, &ids);
        for entry in &entries[..2] {
            let LogEntry::AnnotatedSegmentStart { history, .. } = entry else {
                panic!()
            };
            assert_eq!(
                history[0].metadata.origin,
                LoggedSessionHistoryOrigin::CompactionSummary
            );
            assert_eq!(history[0].metadata.entry_id.0, "summary");
            assert_eq!(
                history[0].metadata.derivation.as_ref().unwrap().sources[0].0,
                "source"
            );
        }
        let LogEntry::AnnotatedUserInput { history, .. } = &entries[2] else {
            panic!()
        };
        assert_eq!(history, &[pasted]);
        let once = serde_json::to_vec(&entries).unwrap();
        migrate_compaction_origins(&mut entries, &ids);
        assert_eq!(serde_json::to_vec(&entries).unwrap(), once);
    }
}
