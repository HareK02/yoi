use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use protocol::{
    Segment, SessionContentPart, SessionEntryProvenance, SessionMessageRole, SessionSnapshot,
    SessionSnapshotEntry, SessionSnapshotEntryData, SessionToolAttachment,
};

use crate::{
    LogEntry, LoggedContentPart, LoggedHistoryEntry, LoggedItem, LoggedRole,
    LoggedSessionHistoryOrigin, SessionId, SystemItem,
};

/// Project a complete current-segment log. A valid segment always starts with
/// one of the two SegmentStart records; malformed partial input uses the nil
/// session only to keep the public failure projection deterministic.
pub fn project_current_session_snapshot(log: &[LogEntry]) -> SessionSnapshot {
    let session_id = log.iter().find_map(|entry| match entry {
        LogEntry::SegmentStart { session_id, .. }
        | LogEntry::AnnotatedSegmentStart { session_id, .. } => Some(*session_id),
        _ => None,
    });
    project_session_snapshot(session_id.unwrap_or_else(SessionId::nil), log)
}

/// Project the current durable segment into the only public session-history
/// representation. Append-log records remain an internal persistence format.
pub fn project_session_snapshot(session_id: SessionId, log: &[LogEntry]) -> SessionSnapshot {
    let mut session_key = session_id;
    let mut entries = Vec::new();

    for (log_index, record) in log.iter().enumerate() {
        match record {
            LogEntry::SegmentStart {
                ts,
                session_id,
                history,
                ..
            } => {
                session_key = *session_id;
                entries.clear();
                for (item_index, item) in history.iter().enumerate() {
                    if let Some(data) = project_item(item) {
                        entries.push(legacy_entry(&session_key, log_index, item_index, *ts, data));
                    }
                }
            }
            LogEntry::AnnotatedSegmentStart {
                ts,
                session_id,
                history,
                ..
            } => {
                session_key = *session_id;
                entries.clear();
                extend_history(&mut entries, history, None, *ts);
            }
            LogEntry::UserInput { ts, segments, .. } => entries.push(legacy_entry(
                &session_key,
                log_index,
                0,
                *ts,
                SessionSnapshotEntryData::UserInput {
                    segments: segments.clone(),
                },
            )),
            LogEntry::AnnotatedUserInput {
                ts,
                segments,
                history,
                ..
            } => extend_history(&mut entries, history, Some(segments), *ts),
            LogEntry::AssistantItem { ts, item } | LogEntry::ToolResult { ts, item } => {
                if let Some(data) = project_item(item) {
                    entries.push(legacy_entry(&session_key, log_index, 0, *ts, data));
                }
            }
            LogEntry::AnnotatedAssistantItem { ts, entry }
            | LogEntry::AnnotatedToolResult { ts, entry } => {
                if let Some(data) = project_item(&entry.item) {
                    entries.push(history_entry(entry, *ts, data));
                }
            }
            LogEntry::SystemItem { ts, item } => entries.push(system_entry(
                item,
                legacy_entry_id(&session_key, log_index, 0),
                *ts,
                SessionEntryProvenance::LegacyUnknown,
                Vec::new(),
            )),
            LogEntry::AnnotatedSystemItem { ts, entry } => entries.push(system_entry(
                &entry.item,
                entry.metadata.entry_id.0.clone(),
                *ts,
                provenance(&entry.metadata.origin),
                derivation_ids(entry),
            )),
            LogEntry::RunErrored { ts, message, .. } => entries.push(legacy_entry(
                &session_key,
                log_index,
                0,
                *ts,
                SessionSnapshotEntryData::RunError {
                    message: message.clone(),
                },
            )),
            // Run checkpoints, configuration, usage, and extension state are
            // controller/storage authority rather than committed conversation.
            LogEntry::Invoke { .. }
            | LogEntry::TurnEnd { .. }
            | LogEntry::RunCompleted { .. }
            | LogEntry::ActiveRunCheckpoint { .. }
            | LogEntry::PausedTurnAbandoned { .. }
            | LogEntry::ConfigChanged { .. }
            | LogEntry::LlmUsage { .. }
            | LogEntry::Extension { .. } => {}
        }
    }

    SessionSnapshot { entries }
}

fn extend_history(
    output: &mut Vec<SessionSnapshotEntry>,
    history: &[LoggedHistoryEntry],
    input_segments: Option<&Vec<Segment>>,
    timestamp: u64,
) {
    let mut attached_segments = false;
    for entry in history {
        let data = if !attached_segments
            && matches!(
                entry.metadata.origin,
                LoggedSessionHistoryOrigin::HumanInput { .. }
            )
            && input_segments.is_some()
        {
            attached_segments = true;
            SessionSnapshotEntryData::UserInput {
                segments: input_segments.cloned().unwrap_or_default(),
            }
        } else {
            let Some(data) = project_item(&entry.item) else {
                continue;
            };
            data
        };
        output.push(history_entry(entry, timestamp, data));
    }
}

fn history_entry(
    entry: &LoggedHistoryEntry,
    timestamp: u64,
    data: SessionSnapshotEntryData,
) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: entry.metadata.entry_id.0.clone(),
        timestamp,
        provenance: provenance(&entry.metadata.origin),
        derived_from: entry
            .metadata
            .derivation
            .as_ref()
            .map(|derivation| {
                derivation
                    .sources
                    .iter()
                    .map(|source| source.0.clone())
                    .collect()
            })
            .unwrap_or_default(),
        data,
    }
}

fn derivation_ids(entry: &crate::LoggedSystemHistoryEntry) -> Vec<String> {
    entry
        .metadata
        .derivation
        .as_ref()
        .map(|derivation| {
            derivation
                .sources
                .iter()
                .map(|source| source.0.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn legacy_entry(
    session_key: &SessionId,
    log_index: usize,
    item_index: usize,
    timestamp: u64,
    data: SessionSnapshotEntryData,
) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: legacy_entry_id(session_key, log_index, item_index),
        timestamp,
        provenance: SessionEntryProvenance::LegacyUnknown,
        derived_from: Vec::new(),
        data,
    }
}

fn legacy_entry_id(session_key: &SessionId, log_index: usize, item_index: usize) -> String {
    let mut identity = Vec::with_capacity(32);
    identity.extend_from_slice(session_key.as_bytes());
    identity.extend_from_slice(&(log_index as u64).to_be_bytes());
    identity.extend_from_slice(&(item_index as u64).to_be_bytes());
    format!("l-{}", URL_SAFE_NO_PAD.encode(identity))
}

fn provenance(origin: &LoggedSessionHistoryOrigin) -> SessionEntryProvenance {
    match origin {
        LoggedSessionHistoryOrigin::HumanInput { .. } => SessionEntryProvenance::HumanInput,
        LoggedSessionHistoryOrigin::WorkerInput { .. } => SessionEntryProvenance::WorkerInput,
        LoggedSessionHistoryOrigin::FlowInstruction { .. } => {
            SessionEntryProvenance::FlowInstruction
        }
        LoggedSessionHistoryOrigin::BackendInstruction { .. } => {
            SessionEntryProvenance::BackendInstruction
        }
        LoggedSessionHistoryOrigin::ModelOutput { .. } => SessionEntryProvenance::ModelOutput,
        LoggedSessionHistoryOrigin::ToolOutput { .. } => SessionEntryProvenance::ToolOutput,
        LoggedSessionHistoryOrigin::DerivedSummary => SessionEntryProvenance::DerivedSummary,
        LoggedSessionHistoryOrigin::LegacyUnknown => SessionEntryProvenance::LegacyUnknown,
    }
}

fn project_item(item: &LoggedItem) -> Option<SessionSnapshotEntryData> {
    match item {
        LoggedItem::Message { role, content } => {
            let role = match role {
                LoggedRole::User => SessionMessageRole::User,
                LoggedRole::Assistant => SessionMessageRole::Assistant,
                // System prompts and instruction history never cross the public
                // snapshot boundary. Typed SystemItems have separate records.
                LoggedRole::System => return None,
            };
            Some(SessionSnapshotEntryData::Message {
                role,
                content: content
                    .iter()
                    .map(|part| match part {
                        LoggedContentPart::Text { text } => {
                            SessionContentPart::Text { text: text.clone() }
                        }
                        LoggedContentPart::Refusal { refusal } => SessionContentPart::Refusal {
                            refusal: refusal.clone(),
                        },
                    })
                    .collect(),
            })
        }
        LoggedItem::ToolCall {
            call_id,
            name,
            arguments,
        } => Some(SessionSnapshotEntryData::ToolCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        LoggedItem::ToolResult {
            call_id,
            summary,
            content,
            is_error,
            attachments,
            ..
        } => Some(SessionSnapshotEntryData::ToolResult {
            call_id: call_id.clone(),
            summary: summary.clone(),
            content: content.clone(),
            is_error: *is_error,
            attachments: attachments
                .iter()
                .map(|attachment| match attachment {
                    crate::logged_item::LoggedAttachment::Image { mime_type, data } => {
                        SessionToolAttachment {
                            media_type: mime_type.clone(),
                            data_base64: BASE64.encode(data),
                        }
                    }
                })
                .collect(),
        }),
        // Hidden model reasoning is never observable.
        LoggedItem::Reasoning { .. } => None,
    }
}

fn system_entry(
    item: &SystemItem,
    entry_id: String,
    timestamp: u64,
    provenance: SessionEntryProvenance,
    derived_from: Vec<String>,
) -> SessionSnapshotEntry {
    let mut data = serde_json::to_value(item).ok();
    if let Some(serde_json::Value::Object(object)) = data.as_mut() {
        object.remove("prompt_provenance");
    }
    let item_kind = data
        .as_ref()
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("system_item")
        .to_owned();
    SessionSnapshotEntry {
        entry_id,
        timestamp,
        provenance,
        derived_from,
        data: SessionSnapshotEntryData::SystemItem {
            item_kind,
            content: item.history_text(),
            data,
        },
    }
}

#[cfg(test)]
mod tests {
    use agen::llm_client::RequestConfig;

    use super::*;
    use crate::{LoggedSessionHistoryEntryId, LoggedSessionHistoryMetadata, LoggedWorkerSubject};

    #[test]
    fn legacy_projection_is_stable_and_hides_reasoning_and_system_prompts() {
        let session_id = crate::new_session_id();
        let log = vec![LogEntry::SegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![
                LoggedItem::Message {
                    role: LoggedRole::System,
                    content: vec![LoggedContentPart::Text {
                        text: "secret prompt".into(),
                    }],
                },
                LoggedItem::Reasoning {
                    text: "secret reasoning".into(),
                    summary: Vec::new(),
                    encrypted_content: None,
                    signature: None,
                },
                LoggedItem::Message {
                    role: LoggedRole::Assistant,
                    content: vec![LoggedContentPart::Text {
                        text: "visible".into(),
                    }],
                },
            ],
            forked_from: None,
            compacted_from: None,
        }];

        let first = project_session_snapshot(session_id, &log);
        let second = project_session_snapshot(session_id, &log);
        assert_eq!(first, second);
        assert_eq!(first.entries.len(), 1);
        assert_eq!(first.entries[0].timestamp, 1);
        assert_eq!(
            first.entries[0].provenance,
            SessionEntryProvenance::LegacyUnknown
        );
        let json = serde_json::to_string(&first).unwrap();
        assert!(!json.contains("secret prompt"));
        assert!(!json.contains("secret reasoning"));
        assert!(json.contains("visible"));
    }

    #[test]
    fn annotated_projection_preserves_identity_and_provenance() {
        let session_id = crate::new_session_id();
        let metadata = LoggedSessionHistoryMetadata {
            entry_id: LoggedSessionHistoryEntryId::new(),
            origin: LoggedSessionHistoryOrigin::ModelOutput {
                worker: LoggedWorkerSubject {
                    workspace_id: None,
                    runtime_id: None,
                    worker_id: "worker".into(),
                },
            },
            derivation: None,
        };
        let expected_id = metadata.entry_id.0.clone();
        let log = vec![LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![LoggedHistoryEntry {
                item: LoggedItem::Message {
                    role: LoggedRole::Assistant,
                    content: vec![LoggedContentPart::Text { text: "ok".into() }],
                },
                metadata,
            }],
            forked_from: None,
            compacted_from: None,
        }];

        let snapshot = project_session_snapshot(session_id, &log);
        assert_eq!(snapshot.entries[0].entry_id, expected_id);
        assert_eq!(
            snapshot.entries[0].provenance,
            SessionEntryProvenance::ModelOutput
        );
    }
}
