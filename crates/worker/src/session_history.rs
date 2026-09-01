//! Restore-authoritative metadata for model-visible Worker history.
//!
//! Agen transports this annotation without interpreting it. Session Log v2
//! stores each item and metadata in one typed record; legacy records are
//! retained only as explicit `LegacyUnknown` entries.

use agen::{HistoryEntry, Item};
use session_store::{
    LogEntry, LoggedHistoryDerivation, LoggedHistoryEntry, LoggedSessionHistoryEntryId,
    LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, LoggedWorkerSubject, SegmentId,
    SessionId,
};

pub type SessionHistoryEntryId = LoggedSessionHistoryEntryId;
pub type SessionHistoryMetadata = LoggedSessionHistoryMetadata;
pub type WorkerHistoryProvenance = LoggedSessionHistoryOrigin;
pub type SessionHistoryDerivation = LoggedHistoryDerivation;
pub type WorkerSubjectSnapshot = LoggedWorkerSubject;

#[cfg(test)]
pub(crate) fn test_logged_history_entry(item: impl Into<Item>) -> LoggedHistoryEntry {
    LoggedHistoryEntry {
        item: session_store::LoggedItem::from(item.into()),
        metadata: LoggedSessionHistoryMetadata {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
        },
    }
}

#[cfg(test)]
pub(crate) fn test_logged_system_entry(
    item: session_store::SystemItem,
) -> session_store::LoggedSystemHistoryEntry {
    session_store::LoggedSystemHistoryEntry {
        item,
        metadata: LoggedSessionHistoryMetadata {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
        },
    }
}

pub(crate) fn worker_subject(session_id: SessionId) -> WorkerSubjectSnapshot {
    WorkerSubjectSnapshot {
        workspace_id: None,
        runtime_id: None,
        worker_id: session_id.to_string(),
    }
}

pub(crate) fn metadata(
    origin: WorkerHistoryProvenance,
    derivation: Option<SessionHistoryDerivation>,
) -> SessionHistoryMetadata {
    SessionHistoryMetadata {
        entry_id: SessionHistoryEntryId::new(),
        origin,
        derivation,
    }
}

#[cfg(test)]
pub(crate) fn history_entry(
    item: Item,
    origin: WorkerHistoryProvenance,
) -> HistoryEntry<SessionHistoryMetadata> {
    HistoryEntry::new(item, metadata(origin, None))
}

pub(crate) fn history_entry_with_id(
    item: Item,
    entry_id: SessionHistoryEntryId,
    origin: WorkerHistoryProvenance,
) -> HistoryEntry<SessionHistoryMetadata> {
    HistoryEntry::new(
        item,
        SessionHistoryMetadata {
            entry_id,
            origin,
            derivation: None,
        },
    )
}

pub(crate) fn to_logged_history_entry(
    entry: &HistoryEntry<SessionHistoryMetadata>,
) -> LoggedHistoryEntry {
    LoggedHistoryEntry {
        item: entry.item.clone().into(),
        metadata: entry.annotation.clone(),
    }
}

fn from_logged(entry: &LoggedHistoryEntry) -> HistoryEntry<SessionHistoryMetadata> {
    HistoryEntry::new(Item::from(entry.item.clone()), entry.metadata.clone())
}

/// Rebuild typed Worker history directly from the append-only Session Log.
/// Missing legacy metadata is never inferred from role or plaintext.
pub(crate) fn restore_history_entries(
    _session_id: SessionId,
    _segment_id: SegmentId,
    entries: &[LogEntry],
) -> Result<Vec<HistoryEntry<SessionHistoryMetadata>>, String> {
    let mut history = Vec::new();
    for entry in entries {
        match entry {
            LogEntry::AnnotatedSegmentStart { history: seed, .. } => {
                history = seed.iter().map(from_logged).collect();
            }
            LogEntry::AnnotatedUserInput { history: input, .. } => {
                history.extend(input.iter().map(from_logged))
            }
            LogEntry::AnnotatedAssistantItem { entry, .. }
            | LogEntry::AnnotatedToolResult { entry, .. } => history.push(from_logged(entry)),
            LogEntry::AnnotatedSystemItem { entry, .. } => history.push(HistoryEntry::new(
                entry.item.to_history_item(),
                entry.metadata.clone(),
            )),
            _ => {}
        }
    }
    Ok(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agen::llm_client::RequestConfig;
    use protocol::Segment;
    use session_store::LogEntry;

    #[test]
    fn typed_flow_and_unknown_caller_input_round_trip_without_role_inference() {
        let session_id = SessionId::now_v7();
        let projected = vec![
            history_entry(
                Item::user_message("flow instructions"),
                WorkerHistoryProvenance::FlowInstruction {
                    selector: "builtin:coder-review".to_string(),
                    definition_id: "coder-review".to_string(),
                    definition_revision: 7,
                    instance_id: "flow-instance".to_string(),
                    state_id: "implement".to_string(),
                },
            ),
            history_entry(
                Item::user_message("implement"),
                WorkerHistoryProvenance::LegacyUnknown,
            ),
        ];
        let entries = vec![
            LogEntry::AnnotatedSegmentStart {
                ts: 0,
                session_id,
                system_prompt: None,
                config: RequestConfig::default(),
                history: Vec::new(),
                forked_from: None,
                compacted_from: None,
            },
            LogEntry::AnnotatedUserInput {
                ts: 1,
                segments: vec![
                    Segment::Flow {
                        selector: "builtin:coder-review".to_string(),
                    },
                    Segment::text("implement"),
                ],
                extensions: Vec::new(),
                history: projected.iter().map(to_logged_history_entry).collect(),
            },
        ];
        let restored = restore_history_entries(session_id, SegmentId::now_v7(), &entries).unwrap();
        assert_eq!(restored, projected);
    }

    #[test]
    fn annotated_restore_preserves_logical_ids_across_reboot() {
        let session_id = SessionId::now_v7();
        let entry = history_entry(
            Item::assistant_message("persisted"),
            WorkerHistoryProvenance::ModelOutput {
                worker: worker_subject(session_id),
            },
        );
        let log = vec![LogEntry::AnnotatedSegmentStart {
            ts: 0,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![to_logged_history_entry(&entry)],
            forked_from: None,
            compacted_from: None,
        }];
        let first = restore_history_entries(session_id, SegmentId::now_v7(), &log).unwrap();
        let second = restore_history_entries(session_id, SegmentId::now_v7(), &log).unwrap();
        assert_eq!(first[0].annotation.entry_id, entry.annotation.entry_id);
        assert_eq!(second[0].annotation.entry_id, entry.annotation.entry_id);
    }

    #[test]
    fn compacted_derivation_uses_stable_logical_entry_ids() {
        let source = history_entry(
            Item::user_message("source"),
            WorkerHistoryProvenance::LegacyUnknown,
        );
        let summary = HistoryEntry::new(
            Item::system_message("summary"),
            metadata(
                WorkerHistoryProvenance::DerivedSummary,
                Some(SessionHistoryDerivation {
                    sources: vec![source.annotation.entry_id.clone()],
                }),
            ),
        );
        assert_eq!(
            summary.annotation.derivation.unwrap().sources,
            vec![source.annotation.entry_id]
        );
    }
}
