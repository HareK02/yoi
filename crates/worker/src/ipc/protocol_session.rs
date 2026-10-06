use protocol::{Alert, Event, Method};
use session_store::LogEntry;
use tokio::sync::broadcast;

use crate::controller::WorkerHandle;

/// Live channels and initial replay data for a Worker protocol session.
///
/// This is intentionally transport-agnostic: Unix JSONL sockets and Runtime
/// WebSocket transports should subscribe through this helper so they cannot
/// drift on which Worker/log events make up the protocol stream.
pub struct WorkerProtocolSessionStreams {
    pub snapshot_event: Event,
    pub alert_snapshot: Vec<Alert>,
    pub log_entries: broadcast::Receiver<LogEntry>,
    pub events: broadcast::Receiver<Event>,
}

fn subscribe_before_snapshot<Subscription, Snapshot>(
    subscribe: impl FnOnce() -> Subscription,
    snapshot: impl FnOnce() -> Snapshot,
) -> (Snapshot, Subscription) {
    // Subscription must exist before any snapshot field is read. A concurrent
    // state update can then appear in both lanes, but it can never be absent
    // from both the snapshot and the queued live stream.
    let subscription = subscribe();
    let snapshot = snapshot();
    (snapshot, subscription)
}

pub fn subscribe_worker_protocol_session(handle: &WorkerHandle) -> WorkerProtocolSessionStreams {
    let ((snapshot_event, log_entries), (alert_snapshot, events)) = subscribe_before_snapshot(
        || handle.alerter.subscribe_with_snapshot(),
        || handle.snapshot_event_with_entry_subscription(),
    );
    WorkerProtocolSessionStreams {
        snapshot_event,
        alert_snapshot,
        log_entries,
        events,
    }
}

pub fn live_log_entry_event(entry: LogEntry) -> Option<Event> {
    match entry {
        entry @ LogEntry::AnnotatedSegmentStart { .. } => {
            let session =
                session_store::public_snapshot::project_current_session_snapshot(&[entry]);
            Some(Event::SegmentRotated { session })
        }
        LogEntry::AnnotatedUserInput {
            ts,
            segments,
            history,
            extensions,
        } => {
            let projected = session_store::public_snapshot::project_current_session_snapshot(&[
                LogEntry::AnnotatedUserInput {
                    ts,
                    segments: segments.clone(),
                    history,
                    extensions,
                },
            ]);
            let entry_id = projected
                .entries
                .iter()
                .find(|entry| {
                    matches!(
                        entry.data,
                        protocol::SessionSnapshotEntryData::UserInput { .. }
                    )
                })
                .map(|entry| entry.entry_id.clone());
            Some(Event::UserMessage { entry_id, segments })
        }
        entry @ (LogEntry::AnnotatedAssistantItem { .. }
        | LogEntry::AnnotatedToolResult { .. }
        | LogEntry::ToolResultCorrected { .. }
        | LogEntry::RunYielded { .. }
        | LogEntry::RunResumed { .. }
        | LogEntry::RunCancelled { .. }
        | LogEntry::RunErrored { .. }) => {
            let mut projected =
                session_store::public_snapshot::project_current_session_snapshot(&[entry]);
            projected
                .entries
                .pop()
                .map(|entry| Event::SessionEntryCommitted { entry })
        }
        LogEntry::AnnotatedSystemItem { entry, .. } => {
            let entry_id = Some(entry.metadata.entry_id.0.clone());
            let value = serde_json::to_value(&entry.item).expect("SystemItem is Serialize");
            Some(Event::SystemItem {
                entry_id,
                item: value,
            })
        }
        LogEntry::Invoke { trigger, .. } => Some(Event::InvokeStart { kind: trigger }),
        other => {
            // `SegmentLogSink::is_live_relevant` keeps non-live-relevant
            // variants off the broadcast lane; reaching here means the two are
            // out of sync and we silently dropped a wire event. Log so a future
            // regression surfaces instead of vanishing.
            tracing::error!(
                entry_kind = ?std::mem::discriminant(&other),
                "session-log broadcast emitted a non-live-relevant entry; sink filter and protocol dispatch are out of sync"
            );
            None
        }
    }
}

/// Dispatch a client Method that has same-connection response semantics.
///
/// Methods returning `Some(Event)` are handled by the protocol session and must
/// be written back only to the requesting transport. Other methods are sent to
/// the Worker controller and their results appear through the normal protocol
/// event/log streams.
pub async fn dispatch_worker_protocol_method(
    handle: &WorkerHandle,
    method: Method,
) -> Option<Event> {
    match method {
        Method::ListCompletions {
            request_id,
            kind,
            prefix,
            context,
        } => {
            let entries = handle
                .completion_entries(kind, &prefix, context.as_ref())
                .await;
            Some(Event::Completions {
                request_id,
                kind,
                prefix,
                context,
                entries,
            })
        }
        method => {
            let _ = handle.send(method).await;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_store::Store;

    use crate::segment_log_sink::SegmentLogSink;
    use crate::worker::{LogWriterHandle, SegmentState};

    fn committed_entry(event: Event) -> protocol::SessionSnapshotEntry {
        match event {
            Event::SessionEntryCommitted { entry } => entry,
            other => panic!("expected SessionEntryCommitted, got {other:?}"),
        }
    }

    #[test]
    fn context_publication_between_subscription_and_snapshot_cannot_be_missed() {
        let (events, _) = broadcast::channel(4);
        let context_tokens = std::cell::Cell::new(20_u64);

        let (snapshot_tokens, mut receiver) = subscribe_before_snapshot(
            || events.subscribe(),
            || {
                context_tokens.set(25);
                events
                    .send(Event::ContextUsage {
                        usage: Some(protocol::ContextUsage {
                            tokens: 25,
                            source: protocol::ContextTokenSource::Estimated,
                        }),
                    })
                    .unwrap();
                context_tokens.get()
            },
        );

        assert_eq!(snapshot_tokens, 25);
        assert!(matches!(
            receiver.try_recv(),
            Ok(Event::ContextUsage {
                usage: Some(protocol::ContextUsage { tokens: 25, .. })
            })
        ));
    }

    #[test]
    fn persisted_run_transitions_keep_identity_across_live_and_snapshot_projection() {
        let temp = tempfile::tempdir().unwrap();
        let store = session_store::FsStore::new(temp.path()).unwrap();
        let session_id = session_store::new_session_id();
        let segment_id = session_store::new_segment_id();
        let start = LogEntry::AnnotatedSegmentStart {
            ts: 1,
            session_id,
            system_prompt: None,
            config: agen::llm_client::RequestConfig::default(),
            history: Vec::new(),
            forked_from: None,
            compacted_from: None,
        };
        store
            .create_segment(session_id, segment_id, std::slice::from_ref(&start))
            .unwrap();
        let sink = SegmentLogSink::with_initial(vec![start]);
        let writer = LogWriterHandle {
            store: store.clone(),
            state: SegmentState::new(session_id, segment_id, 1),
            sink: sink.clone(),
            in_flight: None,
        };
        let (_, mut receiver) = sink.subscribe_with_snapshot();
        let transitions = [
            LogEntry::RunYielded {
                ts: 10,
                entry_id: None,
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunResumed {
                ts: 10,
                entry_id: None,
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 2,
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "same compaction failure".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
            LogEntry::RunErrored {
                ts: 10,
                entry_id: None,
                interrupted: false,
                message: "same compaction failure".into(),
                failure: Some(protocol::RunFailureKind::Compaction),
            },
            LogEntry::RunCancelled {
                ts: 10,
                entry_id: None,
            },
        ];

        let mut live_ids = Vec::new();
        for transition in transitions {
            writer.append_entry(transition).unwrap();
            let committed = receiver.try_recv().expect("committed transition broadcast");
            live_ids.push(
                committed_entry(live_log_entry_event(committed).expect("live projection")).entry_id,
            );
        }
        assert_eq!(
            live_ids
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            5
        );

        let (mirror, _) = sink.subscribe_with_snapshot();
        let snapshot = session_store::public_snapshot::project_current_session_snapshot(&mirror);
        let snapshot_ids: Vec<_> = snapshot
            .entries
            .iter()
            .filter(|entry| {
                matches!(
                    entry.data,
                    protocol::SessionSnapshotEntryData::RunYielded { .. }
                        | protocol::SessionSnapshotEntryData::RunResumed { .. }
                        | protocol::SessionSnapshotEntryData::RunError { .. }
                        | protocol::SessionSnapshotEntryData::RunCancelled
                )
            })
            .map(|entry| entry.entry_id.clone())
            .collect();
        assert_eq!(snapshot_ids, live_ids);

        let persisted = store.read_all(session_id, segment_id).unwrap();
        let persisted_snapshot =
            session_store::public_snapshot::project_current_session_snapshot(&persisted);
        let persisted_ids: Vec<_> = persisted_snapshot
            .entries
            .iter()
            .filter(|entry| snapshot_ids.contains(&entry.entry_id))
            .map(|entry| entry.entry_id.clone())
            .collect();
        assert_eq!(persisted_ids, live_ids);
    }

    #[test]
    fn durable_run_transition_maps_to_session_entry_committed() {
        let event = live_log_entry_event(LogEntry::RunYielded {
            ts: 42,
            entry_id: Some(session_store::LoggedSessionHistoryEntryId::new()),
            reason: protocol::RunYieldReason::Compaction,
            active_run_turn_count: 3,
        })
        .expect("RunYielded must be live-relevant");

        match event {
            Event::SessionEntryCommitted { entry } => {
                assert_eq!(entry.timestamp, 42);
                assert!(matches!(
                    entry.data,
                    protocol::SessionSnapshotEntryData::RunYielded {
                        reason: protocol::RunYieldReason::Compaction,
                        active_run_turn_count: 3,
                    }
                ));
            }
            other => panic!("expected SessionEntryCommitted, got {other:?}"),
        }
    }

    #[test]
    fn system_item_log_entry_keeps_identity_in_live_event() {
        let entry_id = session_store::LoggedSessionHistoryEntryId::new();
        let event = live_log_entry_event(LogEntry::AnnotatedSystemItem {
            ts: session_store::segment_log::now_millis(),
            entry: session_store::LoggedSystemHistoryEntry {
                item: session_store::SystemItem::ResidentSummaryRefresh {
                    body: "refresh".to_string(),
                    prompt_provenance: None,
                },
                metadata: session_store::LoggedSessionHistoryMetadata {
                    entry_id: entry_id.clone(),
                    origin: session_store::LoggedSessionHistoryOrigin::BackendInstruction {
                        operation_id: None,
                    },
                    derivation: None,
                },
            },
            extensions: Vec::new(),
        })
        .expect("SystemItem must be live-relevant");

        match event {
            Event::SystemItem {
                entry_id: live_entry_id,
                item,
            } => {
                assert_eq!(live_entry_id.as_deref(), Some(entry_id.0.as_str()));
                assert_eq!(item["kind"], "resident_summary_refresh");
            }
            other => panic!("expected SystemItem, got {other:?}"),
        }
    }

    #[test]
    fn user_input_log_entry_maps_to_user_message_event() {
        let segments = vec![protocol::Segment::text("hello from log")];
        let event = live_log_entry_event(LogEntry::AnnotatedUserInput {
            ts: session_store::segment_log::now_millis(),
            extensions: vec![],
            history: vec![crate::session_history::test_logged_history_entry(
                agen::Item::user_message("hello from log"),
            )],
            segments: segments.clone(),
        })
        .expect("UserInput must be live-relevant");

        match event {
            Event::UserMessage {
                segments: echoed, ..
            } => assert_eq!(echoed, segments),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }
}
