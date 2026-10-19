//! Versioned decoder for Session schemas v1-4, including frozen Feature wire.
//!
//! These types are intentionally private to `session-store`. Current writers,
//! replay, and public projections use [`crate::LogEntry`] exclusively; only the
//! explicit Session schema migration is allowed to deserialize these shapes.

use agen::llm_client::types::RequestConfig;
use protocol::Segment;
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

use crate::{
    LogEntry, LoggedHistoryEntry, LoggedItem, LoggedSessionHistoryEntryId,
    LoggedSessionHistoryMetadata, LoggedSessionHistoryOrigin, LoggedSystemHistoryEntry, SegmentId,
    SegmentOrigin, SessionExtension, SessionId, SystemItem,
};

// Frozen v1-4 Feature wire. These types are deliberately not exported and are
// never used by LogEntry/SystemItem's current deserializers. Payload structs
// retain the owner's v1 shape rather than erasing typed fields into arbitrary JSON.
#[derive(Debug, Serialize, Deserialize)]
struct JournalStartupPayload {
    snapshot: serde_json::Value,
    snapshot_sha256: String,
    prompt_provenance: crate::PromptRenderProvenance,
}

#[derive(Debug, Serialize, Deserialize)]
struct ResidentSummaryPayload {
    #[serde(default)]
    prompt_provenance: Option<crate::PromptRenderProvenance>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SubjectBehaviorPayload {
    #[serde(default)]
    behavior_md: String,
    #[serde(default)]
    prompt_provenance: Option<crate::PromptRenderProvenance>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SkillActivationPayload {
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct InvocationResultPayload {
    result: protocol::FeatureInvocationResult,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LegacyFeatureSystemItem {
    JournalStartup {
        body: String,
        #[serde(flatten)]
        payload: JournalStartupPayload,
    },
    ResidentSummaryRefresh {
        body: String,
        #[serde(flatten)]
        payload: ResidentSummaryPayload,
    },
    SubjectBehaviorRefresh {
        body: String,
        #[serde(flatten)]
        payload: SubjectBehaviorPayload,
    },
    SkillActivation {
        body: String,
        #[serde(flatten)]
        payload: SkillActivationPayload,
    },
    FeatureInvocationResult {
        body: String,
        #[serde(flatten)]
        payload: InvocationResultPayload,
    },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacySystemItem {
    Feature(LegacyFeatureSystemItem),
    // Includes already-migrated Input records for interrupted migration retry.
    Core(SystemItem),
}

impl LegacySystemItem {
    fn into_entry(
        self,
        mut metadata: LoggedSessionHistoryMetadata,
    ) -> Result<LoggedSystemHistoryEntry, crate::SystemInputError> {
        use crate::InputCompactionUse::{ReferenceInput, Retain};
        let item = match self {
            Self::Core(item) => item,
            Self::Feature(feature) => {
                metadata.validate_system_input()?;
                if metadata.system_input.is_some() {
                    return Err(crate::SystemInputError::InconsistentRecord);
                }
                let (body, input_metadata) = match feature {
                    LegacyFeatureSystemItem::JournalStartup { body, payload } => {
                        migrated_feature_input(
                            body,
                            "journal_startup",
                            "journal:startup",
                            payload,
                            ReferenceInput,
                        )?
                    }
                    LegacyFeatureSystemItem::ResidentSummaryRefresh { body, payload } => {
                        migrated_feature_input(
                            body,
                            "resident_summary_refresh",
                            "memory:resident-summary",
                            payload,
                            Retain,
                        )?
                    }
                    LegacyFeatureSystemItem::SubjectBehaviorRefresh { body, payload } => {
                        migrated_feature_input(
                            body,
                            "subject_behavior_refresh",
                            "subject:behavior",
                            payload,
                            Retain,
                        )?
                    }
                    LegacyFeatureSystemItem::SkillActivation { body, payload } => {
                        migrated_feature_input(
                            body,
                            "skill_activation",
                            "skill:activation",
                            payload,
                            Retain,
                        )?
                    }
                    LegacyFeatureSystemItem::FeatureInvocationResult { body, payload } => {
                        migrated_feature_input(
                            body,
                            "feature_invocation_result",
                            "feature:invocation-result",
                            payload,
                            Retain,
                        )?
                    }
                };
                // Identity/origin/derivation are retained, never re-inferred.
                metadata.system_input = Some(input_metadata);
                SystemItem::Input { body }
            }
        };
        let entry = LoggedSystemHistoryEntry { item, metadata };
        entry.validate_system_input()?;
        Ok(entry)
    }
}

fn migrated_feature_input<T: Serialize>(
    body: String,
    label: &str,
    schema: &str,
    payload: T,
    compaction: crate::InputCompactionUse,
) -> Result<(String, crate::SystemInputMetadata), crate::SystemInputError> {
    let metadata = crate::SystemInputMetadata::new(
        migration_policy(compaction),
        Some(crate::FeatureInputPayload::encode(schema, 1, &payload)?),
        Some(crate::PublicSystemItem {
            kind: label.into(),
            body: body.clone(),
            presentation: crate::PublicSystemPresentation::Context,
        }),
    )?;
    Ok((body, metadata))
}

/// Annotated old wire used by v3/v4, and by already-converted files when an
/// older marker survived an interrupted migration. No ordinary decoder uses it.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LegacyAnnotatedLogEntry {
    AnnotatedSystemItem {
        ts: u64,
        entry: LegacySystemHistoryEntry,
        #[serde(default)]
        extensions: Vec<SessionExtension>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacySystemHistoryEntry {
    item: LegacySystemItem,
    metadata: LoggedSessionHistoryMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacyAnnotatedOrCurrent {
    Annotated(LegacyAnnotatedLogEntry),
    Current(LogEntry),
}

impl LegacyAnnotatedOrCurrent {
    fn into_current(self) -> Result<LogEntry, crate::SystemInputError> {
        match self {
            Self::Annotated(LegacyAnnotatedLogEntry::AnnotatedSystemItem {
                ts,
                entry,
                extensions,
            }) => Ok(LogEntry::AnnotatedSystemItem {
                ts,
                entry: entry.item.into_entry(entry.metadata)?,
                extensions,
            }),
            Self::Current(entry) => Ok(entry),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum LegacyHistoryLogEntry {
    SegmentStart {
        ts: u64,
        session_id: SessionId,
        system_prompt: Option<String>,
        config: RequestConfig,
        history: Vec<LoggedItem>,
        #[serde(default)]
        forked_from: Option<SegmentOrigin>,
        #[serde(default)]
        compacted_from: Option<SegmentOrigin>,
    },
    UserInput {
        ts: u64,
        segments: Vec<Segment>,
        #[serde(default)]
        extensions: Vec<SessionExtension>,
    },
    AssistantItem {
        ts: u64,
        item: LoggedItem,
    },
    ToolResult {
        ts: u64,
        item: LoggedItem,
    },
    SystemItem {
        ts: u64,
        item: LegacySystemItem,
        #[serde(default)]
        extensions: Vec<SessionExtension>,
    },
}

/// Schema-v1 decoder. Non-history records already had their current shape, so
/// they pass through `LogEntry`; legacy history records are converted below.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacySessionLogEntryV1 {
    History(LegacyHistoryLogEntry),
    Current(LegacyAnnotatedOrCurrent),
}

/// Schema v2 retained the v1 history shapes while adding non-history records.
/// Keep a distinct type so supported source versions remain explicit rather
/// than turning migration compatibility into the current `LogEntry` contract.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LegacySessionLogEntryV2 {
    History(LegacyHistoryLogEntry),
    Current(LegacyAnnotatedOrCurrent),
}

pub(crate) fn decode_entry(
    schema_version: u32,
    line: &str,
    session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
) -> Result<LogEntry, serde_json::Error> {
    let entry = match schema_version {
        1 => match serde_json::from_str::<LegacySessionLogEntryV1>(line)? {
            LegacySessionLogEntryV1::History(entry) => Entry::History(entry),
            LegacySessionLogEntryV1::Current(entry) => Entry::Current(entry),
        },
        2 => match serde_json::from_str::<LegacySessionLogEntryV2>(line)? {
            LegacySessionLogEntryV2::History(entry) => Entry::History(entry),
            LegacySessionLogEntryV2::Current(entry) => Entry::Current(entry),
        },
        3 | 4 => Entry::Current(serde_json::from_str(line)?),
        _ => unreachable!("legacy decoder called for unsupported schema {schema_version}"),
    };
    match entry {
        Entry::History(entry) => {
            canonicalize_history_entry(session_id, segment_id, line_index, entry)
        }
        Entry::Current(entry) => entry.into_current(),
    }
    .map_err(serde::de::Error::custom)
}

/// Schema 3 used DerivedSummary for Host compaction seeds. Only the trusted
/// creation boundary may identify one; arbitrary text and later seed items do not.
/// Include already-upgraded entries so an interrupted multi-file migration resumes.
pub(crate) fn compaction_summary_ids<'a>(
    entries: impl Iterator<Item = &'a LogEntry>,
) -> std::collections::HashSet<LoggedSessionHistoryEntryId> {
    entries
        .filter_map(|entry| {
            let LogEntry::AnnotatedSegmentStart {
                compacted_from: Some(_),
                forked_from: None,
                history,
                ..
            } = entry
            else {
                return None;
            };
            let first = history.first()?;
            (matches!(
                first.metadata.origin,
                LoggedSessionHistoryOrigin::DerivedSummary
                    | LoggedSessionHistoryOrigin::CompactionSummary
            ) && matches!(
                first.item,
                LoggedItem::Message {
                    role: crate::LoggedRole::System,
                    ..
                }
            ) && agen::Item::from(first.item.clone())
                .as_text()
                .is_some_and(|text| text.starts_with("[Compacted context summary]\n\n")))
            .then(|| first.metadata.entry_id.clone())
        })
        .collect()
}

pub(crate) fn migrate_compaction_origins(
    entries: &mut [LogEntry],
    summaries: &std::collections::HashSet<LoggedSessionHistoryEntryId>,
) {
    // Only inherited seed copies can share these identities. Appended input is
    // not a construction boundary and must never be reclassified from its text.
    for entry in entries {
        if let LogEntry::AnnotatedSegmentStart { history, .. } = entry {
            for entry in history {
                if summaries.contains(&entry.metadata.entry_id)
                    && entry.metadata.origin == LoggedSessionHistoryOrigin::DerivedSummary
                {
                    entry.metadata.origin = LoggedSessionHistoryOrigin::CompactionSummary;
                }
            }
        }
    }
}

/// Explicit v1-4 -> v5 boundary. No Feature inference belongs in ordinary
/// readers/restoration. Invalid or oversized payloads fail migration; the generic
/// payload/body/public limits are deliberately not relaxed for legacy data.
pub(crate) fn migrate_system_inputs(entry: &mut LogEntry) -> Result<(), crate::SystemInputError> {
    entry.validate_system_inputs()?;
    match entry {
        // Feature wire has already been converted by the explicit old-schema
        // decoder. Ordinary LogEntry can contain only generic Input/core events.
        LogEntry::AnnotatedSystemItem { entry, .. } => entry.validate_system_input()?,
        LogEntry::AnnotatedSegmentStart { history, .. }
        | LogEntry::AnnotatedUserInput { history, .. } => {
            for entry in history {
                migrate_flattened_system_input(entry)?;
            }
        }
        LogEntry::AnnotatedAssistantItem { entry, .. }
        | LogEntry::AnnotatedToolResult { entry, .. }
        | LogEntry::ToolResultCorrected { entry, .. } => {
            migrate_flattened_system_input(entry)?;
        }
        _ => {}
    }
    entry.validate_system_inputs()
}

fn migration_policy(compaction: crate::InputCompactionUse) -> crate::SystemInputUsePolicy {
    crate::SystemInputUsePolicy {
        recording_material: crate::InputEvidenceUse::Exclude,
        memory_evidence: crate::InputEvidenceUse::Exclude,
        observation: crate::InputObservationUse::Hidden,
        compaction,
    }
}

fn migrate_flattened_system_input(
    entry: &mut LoggedHistoryEntry,
) -> Result<(), crate::SystemInputError> {
    // Check before supplying legacy policy: Provided without metadata is a
    // malformed NEW record, not permission to synthesize a producer policy.
    entry.validate_system_input()?;
    if entry.metadata.system_input.is_none()
        && matches!(
            entry.item,
            LoggedItem::Message {
                role: crate::LoggedRole::System,
                ..
            }
        )
    {
        let compaction = match entry.metadata.origin {
            LoggedSessionHistoryOrigin::JournalContext => crate::InputCompactionUse::ReferenceInput,
            LoggedSessionHistoryOrigin::CompactionSummary => {
                crate::InputCompactionUse::OpeningSummary
            }
            _ => crate::InputCompactionUse::Retain,
        };
        // No Feature payload or public projection can be recovered from text.
        entry.metadata.system_input = Some(crate::SystemInputMetadata::new(
            migration_policy(compaction),
            None,
            None,
        )?);
    }
    entry.validate_system_input()
}

enum Entry {
    History(LegacyHistoryLogEntry),
    Current(LegacyAnnotatedOrCurrent),
}

fn legacy_metadata(
    segment_id: SegmentId,
    line_index: usize,
    item_index: usize,
) -> LoggedSessionHistoryMetadata {
    let mut identity = Vec::with_capacity(32);
    identity.extend_from_slice(segment_id.as_bytes());
    identity.extend_from_slice(&(line_index as u64).to_be_bytes());
    identity.extend_from_slice(&(item_index as u64).to_be_bytes());
    LoggedSessionHistoryMetadata {
        system_input: None,
        entry_id: LoggedSessionHistoryEntryId(format!(
            "l-{}",
            base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, identity)
        )),
        origin: LoggedSessionHistoryOrigin::LegacyUnknown,
        derivation: None,
    }
}

fn canonicalize_history_entry(
    _session_id: SessionId,
    segment_id: SegmentId,
    line_index: usize,
    entry: LegacyHistoryLogEntry,
) -> Result<LogEntry, crate::SystemInputError> {
    Ok(match entry {
        LegacyHistoryLogEntry::SegmentStart {
            ts,
            session_id,
            system_prompt,
            config,
            history,
            forked_from,
            compacted_from,
        } => LogEntry::AnnotatedSegmentStart {
            ts,
            session_id,
            system_prompt,
            config,
            history: history
                .into_iter()
                .enumerate()
                .map(|(item_index, item)| LoggedHistoryEntry {
                    item,
                    metadata: legacy_metadata(segment_id, line_index, item_index),
                })
                .collect(),
            forked_from,
            compacted_from,
        },
        LegacyHistoryLogEntry::UserInput {
            ts,
            segments,
            extensions,
        } => LogEntry::AnnotatedUserInput {
            ts,
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::from(agen::Item::user_message(Segment::flatten_to_text(
                    &segments,
                ))),
                metadata: legacy_metadata(segment_id, line_index, 0),
            }],
            segments,
            extensions,
        },
        LegacyHistoryLogEntry::AssistantItem { ts, item } => LogEntry::AnnotatedAssistantItem {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(segment_id, line_index, 0),
            },
        },
        LegacyHistoryLogEntry::ToolResult { ts, item } => LogEntry::AnnotatedToolResult {
            ts,
            entry: LoggedHistoryEntry {
                item,
                metadata: legacy_metadata(segment_id, line_index, 0),
            },
        },
        LegacyHistoryLogEntry::SystemItem {
            ts,
            item,
            extensions,
        } => LogEntry::AnnotatedSystemItem {
            ts,
            entry: item.into_entry(legacy_metadata(segment_id, line_index, 0))?,
            extensions,
        },
    })
}
