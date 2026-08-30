//! Serializable history entries with restore-authoritative logical identity and origin.

use serde::{Deserialize, Serialize};

use crate::LoggedItem;

/// Stable logical identity of one model-visible history entry.
///
/// This value is generated at the trusted Worker session boundary and copied
/// unchanged across fork, rewind, compaction retention, restore, and reboot.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LoggedSessionHistoryEntryId(pub String);

impl LoggedSessionHistoryEntryId {
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}

impl Default for LoggedSessionHistoryEntryId {
    fn default() -> Self {
        Self::new()
    }
}

/// Bounded subject snapshot. It is evidence, not a live authorization handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedWorkerSubject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    pub worker_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LoggedSessionHistoryOrigin {
    HumanInput {
        account_id: String,
    },
    WorkerInput {
        actor: LoggedWorkerSubject,
    },
    FlowInstruction {
        selector: String,
        definition_id: String,
        definition_revision: u64,
        instance_id: String,
        state_id: String,
    },
    BackendInstruction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_id: Option<String>,
    },
    ModelOutput {
        worker: LoggedWorkerSubject,
    },
    ToolOutput {
        worker: LoggedWorkerSubject,
    },
    DerivedSummary,
    LegacyUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedHistoryDerivation {
    pub sources: Vec<LoggedSessionHistoryEntryId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoggedSessionHistoryMetadata {
    pub entry_id: LoggedSessionHistoryEntryId,
    pub origin: LoggedSessionHistoryOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation: Option<LoggedHistoryDerivation>,
}

impl LoggedSessionHistoryMetadata {
    pub fn legacy_unknown() -> Self {
        Self {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
        }
    }
}

/// Persisted item and metadata are one value so transforms cannot reorder or
/// truncate one without the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoggedHistoryEntry {
    pub item: LoggedItem,
    pub metadata: LoggedSessionHistoryMetadata,
}

/// Typed system-item history record. The typed system event remains available
/// to client replay while its model-visible projection carries the same stable
/// metadata used by live history.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LoggedSystemHistoryEntry {
    pub item: crate::SystemItem,
    pub metadata: LoggedSessionHistoryMetadata,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LoggedRole;
    use agen::llm_client::RequestConfig;

    #[test]
    fn logged_history_entry_round_trip_preserves_id_origin_and_derivation() {
        let source_id = LoggedSessionHistoryEntryId::new();
        let entry = LoggedHistoryEntry {
            item: LoggedItem::Message {
                role: LoggedRole::User,
                content: vec![crate::LoggedContentPart::Text {
                    text: "preference".into(),
                }],
            },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId::new(),
                origin: LoggedSessionHistoryOrigin::HumanInput {
                    account_id: "account-1".into(),
                },
                derivation: Some(LoggedHistoryDerivation {
                    sources: vec![source_id.clone()],
                }),
            },
        };
        let encoded = serde_json::to_vec(&entry).unwrap();
        let decoded: LoggedHistoryEntry = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, entry);
        assert_eq!(
            decoded.metadata.derivation.unwrap().sources,
            vec![source_id]
        );
    }

    #[test]
    fn annotated_segment_start_is_restore_visible_without_projecting_metadata() {
        let session_id = uuid::Uuid::now_v7();
        let history_entry = LoggedHistoryEntry {
            item: LoggedItem::Message {
                role: LoggedRole::Assistant,
                content: vec![crate::LoggedContentPart::Text {
                    text: "answer".into(),
                }],
            },
            metadata: LoggedSessionHistoryMetadata::legacy_unknown(),
        };
        let state = crate::collect_state(&[crate::LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![history_entry],
            forked_from: None,
            compacted_from: None,
        }]);
        assert_eq!(state.history[0].as_text(), Some("answer"));
    }
}
