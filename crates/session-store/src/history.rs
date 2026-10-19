//! Serializable history entries with restore-authoritative logical identity and origin.

use serde::{Deserialize, Serialize};

use crate::LoggedItem;

/// Stable logical identity of one durable public Session entry.
///
/// Model-visible history entries and typed Run transitions share this identity
/// space so every projection can reconcile the same committed record.
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
    /// Human input verified by the standalone Host, without a Backend Account.
    LocalHumanInput,
    HumanInput {
        account_id: String,
    },
    WorkerInput {
        actor: LoggedWorkerSubject,
    },
    FlowInstruction {
        selector: String,
        definition_id: String,
        /// Exact compiled source used for these instructions; absent in older logs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        definition_digest: Option<String>,
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
    /// Descriptive producer identity; use policies live in separate metadata.
    Provided {
        producer: String,
    },
    /// Host-produced opening context summary; not auto-read or TaskStore material.
    CompactionSummary,
    /// Frozen Host Journal reference, never independent observation evidence.
    JournalContext,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_input: Option<crate::SystemInputMetadata>,
}

impl LoggedSessionHistoryMetadata {
    fn validate_system_input(&self) -> Result<(), crate::SystemInputError> {
        if matches!(self.origin, LoggedSessionHistoryOrigin::Provided { .. })
            && self.system_input.is_none()
        {
            return Err(crate::SystemInputError::MissingMetadata);
        }
        Ok(())
    }

    pub fn legacy_unknown() -> Self {
        Self {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::LegacyUnknown,
            derivation: None,
            system_input: None,
        }
    }
}

/// Persisted item and metadata are one value so transforms cannot reorder or
/// truncate one without the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "HistoryEntryWire")]
pub struct LoggedHistoryEntry {
    pub item: LoggedItem,
    pub metadata: LoggedSessionHistoryMetadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryEntryWire {
    item: LoggedItem,
    metadata: LoggedSessionHistoryMetadata,
}

impl TryFrom<HistoryEntryWire> for LoggedHistoryEntry {
    type Error = crate::SystemInputError;

    fn try_from(wire: HistoryEntryWire) -> Result<Self, Self::Error> {
        let entry = Self {
            item: wire.item,
            metadata: wire.metadata,
        };
        entry.validate_system_input()?;
        Ok(entry)
    }
}

impl LoggedHistoryEntry {
    pub fn validate_system_input(&self) -> Result<(), crate::SystemInputError> {
        self.metadata.validate_system_input()?;
        if self.metadata.system_input.is_some() {
            let LoggedItem::Message {
                role: crate::LoggedRole::System,
                content,
            } = &self.item
            else {
                return Err(crate::SystemInputError::InconsistentRecord);
            };
            let [crate::LoggedContentPart::Text { text }] = content.as_slice() else {
                return Err(crate::SystemInputError::InconsistentRecord);
            };
            if text.len() > crate::system_input::MAX_SYSTEM_INPUT_BYTES {
                return Err(crate::SystemInputError::ResourceLimit);
            }
        }
        Ok(())
    }
}

/// Typed system-item history record. The typed system event remains available
/// to client replay while its model-visible projection carries the same stable
/// metadata used by live history.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(try_from = "SystemEntryWire")]
pub struct LoggedSystemHistoryEntry {
    pub item: crate::SystemItem,
    pub metadata: LoggedSessionHistoryMetadata,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemEntryWire {
    item: crate::SystemItem,
    metadata: LoggedSessionHistoryMetadata,
}

impl TryFrom<SystemEntryWire> for LoggedSystemHistoryEntry {
    type Error = crate::SystemInputError;

    fn try_from(wire: SystemEntryWire) -> Result<Self, Self::Error> {
        let entry = Self {
            item: wire.item,
            metadata: wire.metadata,
        };
        entry.validate_system_input()?;
        Ok(entry)
    }
}

impl LoggedSystemHistoryEntry {
    pub fn from_input(
        origin: LoggedSessionHistoryOrigin,
        derivation: Option<LoggedHistoryDerivation>,
        input: crate::SystemInputEnvelope,
    ) -> Self {
        let (body, system_input) = input.into_parts();
        Self {
            item: crate::SystemItem::Input { body },
            metadata: LoggedSessionHistoryMetadata {
                entry_id: LoggedSessionHistoryEntryId::new(),
                origin,
                derivation,
                system_input: Some(system_input),
            },
        }
    }

    pub fn validate_system_input(&self) -> Result<(), crate::SystemInputError> {
        self.metadata.validate_system_input()?;
        match (&self.item, &self.metadata.system_input) {
            (crate::SystemItem::Input { body }, Some(_)) => {
                if body.len() > crate::system_input::MAX_SYSTEM_INPUT_BYTES {
                    return Err(crate::SystemInputError::ResourceLimit);
                }
            }
            (crate::SystemItem::Input { .. }, None) => {
                return Err(crate::SystemInputError::MissingMetadata);
            }
            (_, Some(_)) => return Err(crate::SystemInputError::InconsistentRecord),
            (_, None) => {}
        }
        Ok(())
    }

    pub fn to_history_entry(&self) -> LoggedHistoryEntry {
        LoggedHistoryEntry {
            item: self.item.to_history_item().into(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn public_item(&self) -> crate::PublicSystemItem {
        if matches!(self.item, crate::SystemItem::Input { .. }) {
            return self
                .metadata
                .system_input
                .as_ref()
                .and_then(crate::SystemInputMetadata::public_item)
                .cloned()
                .unwrap_or_else(|| self.item.public_item());
        }
        self.item.public_item()
    }
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
                system_input: None,
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
    fn external_system_records_reject_missing_policy_role_mismatch_and_overlimit_body() {
        let valid = serde_json::json!({
            "item": { "kind": "input", "body": "context" },
            "metadata": {
                "entry_id": "stable-entry", "origin": { "kind": "provided", "producer": "owner" },
                "system_input": { "usage": {
                    "recording_material": "exclude", "memory_evidence": "exclude",
                    "observation": "hidden", "compaction": "reference_input"
                } }
            }
        });
        let restored: LoggedSystemHistoryEntry = serde_json::from_value(valid.clone()).unwrap();
        let flat = serde_json::to_value(restored.to_history_entry()).unwrap();
        assert!(serde_json::from_value::<LoggedHistoryEntry>(flat.clone()).is_ok());
        for generic in [true, false] {
            let wire = if generic { &valid } else { &flat };
            let decode = |wire| {
                if generic {
                    serde_json::from_value::<LoggedSystemHistoryEntry>(wire).is_err()
                } else {
                    serde_json::from_value::<LoggedHistoryEntry>(wire).is_err()
                }
            };
            let mut missing = wire.clone();
            missing["metadata"]
                .as_object_mut()
                .unwrap()
                .remove("system_input");
            assert!(decode(missing));
            let mut absent_policy = wire.clone();
            absent_policy["metadata"]["system_input"]
                .as_object_mut()
                .unwrap()
                .remove("usage");
            assert!(decode(absent_policy));
            let mut invalid_role = wire.clone();
            invalid_role["item"] =
                serde_json::to_value(LoggedItem::from(agen::Item::user_message("not system")))
                    .unwrap();
            assert!(decode(invalid_role));
            let mut oversized = wire.clone();
            if generic {
                oversized["item"]["body"] = "x"
                    .repeat(crate::system_input::MAX_SYSTEM_INPUT_BYTES + 1)
                    .into();
            } else {
                oversized["item"]["content"][0]["text"] = "x"
                    .repeat(crate::system_input::MAX_SYSTEM_INPUT_BYTES + 1)
                    .into();
            }
            assert!(decode(oversized));
        }
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
