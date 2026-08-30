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

pub fn subscribe_worker_protocol_session(handle: &WorkerHandle) -> WorkerProtocolSessionStreams {
    let (snapshot_event, log_entries) = handle.snapshot_event_with_entry_subscription();
    let (alert_snapshot, events) = handle.alerter.subscribe_with_snapshot();
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
        LogEntry::AnnotatedUserInput { segments, .. } => Some(Event::UserMessage { segments }),
        LogEntry::AnnotatedSystemItem { entry, .. } => {
            let value = serde_json::to_value(&entry.item).expect("SystemItem is Serialize");
            Some(Event::SystemItem { item: value })
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
        Method::ListCompletions { kind, prefix } => {
            let entries = handle.completion_entries(kind, &prefix).await;
            Some(Event::Completions { kind, entries })
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
            Event::UserMessage { segments: echoed } => assert_eq!(echoed, segments),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }
}
