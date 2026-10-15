//! User-facing alert channel for Worker → client.
//!
//! Separate from `tracing` (which is for developer logs). Alerts
//! are short human-readable messages the Worker layer wants a client to
//! see — for example "compaction failed", "tool-call early dispatch deferred".
//!
//! Each alert is broadcast on the shared `Event` channel and
//! also appended to an in-memory buffer so that clients connecting
//! after the fact can replay the bounded recent prefix from the same live Worker.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use protocol::{Alert, AlertLevel, AlertPosition, AlertSource, Event};

use crate::{in_flight::InFlightEvents, segment_log_sink::SegmentLogSink};

/// Upper bound on buffered alerts. When exceeded, the oldest
/// entries are discarded so a long-running session cannot leak
/// memory through a pathological loop of recurring alerts
/// (e.g. compaction failing every turn).
const MAX_BUFFERED_ALERTS: usize = 512;

#[derive(Clone)]
pub struct Alerter {
    inner: Arc<Inner>,
}

/// A queued identified Alert is no longer retained by this live Worker.
/// Transports must disconnect/resnapshot instead of forwarding its stale anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("alert is no longer retained; reconnect for a fresh snapshot")]
pub struct AlertReplayRequired;

struct Inner {
    working_event_tx: broadcast::Sender<Event>,
    buffer: Mutex<VecDeque<BufferedAlert>>,
    position_source: Mutex<Option<PositionSource>>,
}

#[derive(Clone)]
struct PositionSource {
    sink: SegmentLogSink,
    in_flight: Option<InFlightEvents>,
}

#[derive(Clone)]
struct BufferedAlert {
    alert: Alert,
    position: Option<Arc<Mutex<AlertPosition>>>,
}

impl BufferedAlert {
    fn snapshot(&self) -> Alert {
        let mut alert = self.alert.clone();
        alert.position = self.position.as_ref().map(|position| {
            position
                .lock()
                .expect("alert position mutex poisoned")
                .clone()
        });
        alert
    }
}

impl PositionSource {
    fn capture(&self) -> Arc<Mutex<AlertPosition>> {
        // Same order as commit/snapshot: in-flight -> sink. Never hold the
        // alerter buffer lock while acquiring either of these locks.
        let mut in_flight = self.in_flight.as_ref().map(InFlightEvents::snapshot_guard);
        in_flight
            .as_mut()
            .and_then(|guard| guard.capture_alert_position())
            .unwrap_or_else(|| Arc::new(Mutex::new(self.sink.alert_position())))
    }
}

impl Alerter {
    pub fn new(working_event_tx: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Inner {
                working_event_tx,
                buffer: Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_ALERTS)),
                position_source: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn attach_position_source(
        &self,
        sink: SegmentLogSink,
        in_flight: Option<InFlightEvents>,
    ) {
        *self
            .inner
            .position_source
            .lock()
            .expect("alert source mutex poisoned") = Some(PositionSource { sink, in_flight });
    }

    /// Record and broadcast an alert.
    ///
    /// The broadcast may have no subscribers (e.g. during Worker
    /// construction before any client has connected); the buffer
    /// guarantees the message is still delivered once a client
    /// attaches.
    ///
    /// The buffer mutex is held across `broadcast::send` to make
    /// `subscribe_with_snapshot` race-free — a client that snapshots
    /// the buffer while holding the same lock sees every alert
    /// exactly once: older ones from the snapshot, newer ones from
    /// the freshly-subscribed receiver.
    pub fn alert(&self, level: AlertLevel, source: AlertSource, message: String) {
        self.alert_at(level, source, message, now_ms());
    }

    fn alert_at(&self, level: AlertLevel, source: AlertSource, message: String, timestamp_ms: i64) {
        let source_handle = self
            .inner
            .position_source
            .lock()
            .expect("alert source mutex poisoned")
            .clone();
        let position = source_handle.map(|source| source.capture());
        let alert = Alert {
            id: Some(uuid::Uuid::now_v7().to_string()),
            position: None,
            level,
            source,
            message,
            timestamp_ms,
        };
        if let Ok(mut buf) = self.inner.buffer.lock() {
            if buf.len() >= MAX_BUFFERED_ALERTS {
                buf.pop_front();
            }
            let buffered = BufferedAlert { alert, position };
            let event = Event::Alert(buffered.snapshot());
            buf.push_back(buffered);
            let _ = self.inner.working_event_tx.send(event);
        }
    }

    /// Refresh a queued Alert immediately before transport forwarding.
    ///
    /// The independent log lane may have already delivered the text commit, so
    /// the broadcast's copied in-flight anchor is not authoritative anymore.
    /// Missing identified alerts require reconnect/resnapshot; legacy alerts
    /// without IDs and non-Alert events pass through unchanged.
    pub fn freshen_event(&self, event: Event) -> Result<Event, AlertReplayRequired> {
        let Event::Alert(ref alert) = event else {
            return Ok(event);
        };
        let Some(id) = alert.id.as_ref() else {
            return Ok(event);
        };
        let buffer = self
            .inner
            .buffer
            .lock()
            .expect("alerter buffer mutex poisoned");
        buffer
            .iter()
            .find(|buffered| buffered.alert.id.as_ref() == Some(id))
            .map(|buffered| Event::Alert(buffered.snapshot()))
            .ok_or(AlertReplayRequired)
    }

    /// Subscribe and atomically snapshot the current buffer.
    ///
    /// The returned snapshot contains alerts emitted before
    /// this call; the receiver will deliver alerts emitted
    /// after. An alert cannot appear in both.
    pub fn subscribe_with_snapshot(&self) -> (Vec<Alert>, broadcast::Receiver<Event>) {
        let (snapshot, rx) = self.subscribe_with_deferred_snapshot();
        (snapshot(), rx)
    }

    // Resolve positions after the session snapshot has been captured. Otherwise
    // a text commit between the two snapshots can leave only its old block ID
    // in replay, although that block is already absent from the session snapshot.
    pub(crate) fn subscribe_with_deferred_snapshot(
        &self,
    ) -> (impl FnOnce() -> Vec<Alert>, broadcast::Receiver<Event>) {
        let buf = self
            .inner
            .buffer
            .lock()
            .expect("alerter buffer mutex poisoned");
        let rx = self.inner.working_event_tx.subscribe();
        let snapshot: Vec<BufferedAlert> = buf.iter().cloned().collect();
        (
            move || snapshot.iter().map(BufferedAlert::snapshot).collect(),
            rx,
        )
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positioned_alerter() -> (Alerter, SegmentLogSink, InFlightEvents) {
        let (tx, _) = broadcast::channel(1024);
        let in_flight = InFlightEvents::new(tx.clone());
        let sink = SegmentLogSink::new();
        let alerter = Alerter::new(tx);
        alerter.attach_position_source(sink.clone(), Some(in_flight.clone()));
        (alerter, sink, in_flight)
    }

    #[test]
    fn queued_alert_uses_committed_anchor_when_log_lane_was_delivered_first() {
        let (alerter, sink, in_flight) = positioned_alerter();
        let text = in_flight.start_text_block();
        in_flight.text_delta(text, "finished text".into());
        in_flight.text_done(text, "finished text".into());
        let (_, mut events) = alerter.subscribe_with_snapshot();
        let (_, mut log_entries) = sink.subscribe_with_snapshot();
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "queued".into());
        let queued = events.try_recv().unwrap();
        let Event::Alert(ref original) = queued else {
            panic!("expected alert")
        };
        assert!(matches!(
            original.position,
            Some(AlertPosition::AfterInFlight { .. })
        ));
        let original_id = original.id.clone();
        let item: session_store::LoggedItem = agen::Item::assistant_message("finished text").into();
        let entry = crate::session_history::test_logged_history_entry(item.clone());
        let entry_id = entry.metadata.entry_id.0.clone();
        in_flight.clear_for_committed_item_then(&item, Some(&entry_id), || {
            sink.publish(session_store::LogEntry::AnnotatedAssistantItem { ts: 42, entry });
        });
        let committed =
            crate::ipc::protocol_session::live_log_entry_event(log_entries.try_recv().unwrap())
                .unwrap();
        assert!(
            matches!(committed, Event::SessionEntryCommitted { entry } if entry.entry_id == entry_id)
        );
        let Event::Alert(forwarded) = alerter.freshen_event(queued).unwrap() else {
            panic!("expected alert")
        };
        assert_eq!(forwarded.id, original_id);
        assert_eq!(
            forwarded.position,
            Some(AlertPosition::AfterEntry { entry_id })
        );
        let replay = alerter.subscribe_with_snapshot().0;
        assert_eq!(
            serde_json::to_value(forwarded).unwrap(),
            serde_json::to_value(&replay[0]).unwrap()
        );
    }

    #[test]
    fn evicted_or_unknown_alert_ids_require_replay_but_legacy_events_pass_through() {
        let (alerter, _, _) = positioned_alerter();
        let (_, mut events) = alerter.subscribe_with_snapshot();
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "old".into());
        let queued = events.try_recv().unwrap();
        for _ in 0..MAX_BUFFERED_ALERTS {
            alerter.alert(AlertLevel::Warn, AlertSource::Worker, "later".into());
        }
        assert!(matches!(
            alerter.freshen_event(queued),
            Err(AlertReplayRequired)
        ));
        let unknown = Event::Alert(Alert {
            id: Some("not-retained".into()),
            position: Some(AlertPosition::AfterInFlight { block_id: 1 }),
            level: AlertLevel::Warn,
            source: AlertSource::Worker,
            message: "unknown".into(),
            timestamp_ms: 42,
        });
        assert!(matches!(
            alerter.freshen_event(unknown),
            Err(AlertReplayRequired)
        ));
        let legacy = Event::Alert(Alert {
            id: None,
            position: None,
            level: AlertLevel::Warn,
            source: AlertSource::Worker,
            message: "legacy".into(),
            timestamp_ms: 42,
        });
        let expected = serde_json::to_value(&legacy).unwrap();
        assert_eq!(
            serde_json::to_value(alerter.freshen_event(legacy).unwrap()).unwrap(),
            expected
        );
        assert!(matches!(
            alerter.freshen_event(Event::Shutdown),
            Ok(Event::Shutdown)
        ));
    }

    #[test]
    fn rowless_run_transitions_preserve_visible_alert_anchor_live_and_on_replay() {
        use session_store::LogEntry;
        let (alerter, sink, _) = positioned_alerter();
        let entry = crate::session_history::test_logged_history_entry(
            agen::Item::assistant_message("visible"),
        );
        let expected = Some(AlertPosition::AfterEntry {
            entry_id: entry.metadata.entry_id.0.clone(),
        });
        sink.publish(LogEntry::AnnotatedAssistantItem { ts: 1, entry });
        let (_, mut events) = alerter.subscribe_with_snapshot();
        let transitions = [
            LogEntry::RunCancelled {
                ts: 2,
                entry_id: None,
            },
            LogEntry::RunYielded {
                ts: 3,
                entry_id: None,
                reason: protocol::RunYieldReason::Compaction,
                active_run_turn_count: 1,
            },
            LogEntry::RunResumed {
                ts: 4,
                entry_id: None,
                source: protocol::RunResumeSource::Compaction,
                active_run_turn_count: 1,
            },
        ];
        for mut transition in transitions {
            transition.ensure_session_entry_id();
            sink.publish(transition);
            alerter.alert(
                AlertLevel::Warn,
                AlertSource::Worker,
                "after transition".into(),
            );
            let Event::Alert(live) = alerter.freshen_event(events.try_recv().unwrap()).unwrap()
            else {
                panic!("expected alert")
            };
            assert_eq!(live.position, expected);
        }
        let replay = alerter.subscribe_with_snapshot().0;
        assert_eq!(replay.len(), 3);
        assert!(replay.iter().all(|alert| alert.position == expected));
        let (entries, _) = sink.subscribe_with_snapshot();
        // Rebuilding the sink cache from retained entries must use the same rule.
        sink.replace_silent(entries);
        alerter.alert(
            AlertLevel::Warn,
            AlertSource::Worker,
            "after cache rebuild".into(),
        );
        assert_eq!(
            alerter.subscribe_with_snapshot().0.last().unwrap().position,
            expected
        );
    }

    #[test]
    fn identical_alerts_in_same_millisecond_keep_distinct_ids_across_replay() {
        let (alerter, _, _) = positioned_alerter();
        let (_, mut rx) = alerter.subscribe_with_snapshot();
        for _ in 0..2 {
            alerter.alert_at(AlertLevel::Warn, AlertSource::Worker, "same".into(), 42);
        }
        let (snapshot, _) = alerter.subscribe_with_snapshot();
        assert_ne!(snapshot[0].id, snapshot[1].id);
        for expected in &snapshot {
            let Event::Alert(live) = rx.try_recv().unwrap() else {
                panic!("expected alert")
            };
            assert!(live.id.is_some());
            assert_eq!(live.id, expected.id);
            assert_eq!(live.timestamp_ms, 42);
            assert_eq!(live.position, Some(AlertPosition::Start));
            assert_eq!(live.position, expected.position);
        }
        let (replay, _) = alerter.subscribe_with_snapshot();
        assert_eq!(
            serde_json::to_value(snapshot).unwrap(),
            serde_json::to_value(replay).unwrap()
        );
    }

    #[test]
    fn tool_call_anchor_survives_commit_and_new_text() {
        let (alerter, sink, in_flight) = positioned_alerter();
        in_flight.tool_call_start("call-1".into(), "Read".into());
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "tool alert".into());
        let item: session_store::LoggedItem = agen::Item::tool_call("call-1", "Read", "{}").into();
        let entry = session_store::LogEntry::AnnotatedAssistantItem {
            ts: 1,
            entry: crate::session_history::test_logged_history_entry(item.clone()),
        };
        in_flight.clear_for_committed_item_then(&item, Some("entry-tool"), || sink.publish(entry));
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "after commit".into());
        let text = in_flight.start_text_block();
        in_flight.text_delta(text, "next".into());
        let (snapshot, _) = alerter.subscribe_with_snapshot();
        for alert in snapshot {
            assert_eq!(
                alert.position,
                Some(AlertPosition::AfterToolCall {
                    call_id: "call-1".into()
                })
            );
        }
    }

    #[test]
    fn eviction_drops_position_ownership_and_clear_does_not_retarget_old_alerts() {
        let (alerter, _, in_flight) = positioned_alerter();
        let text = in_flight.start_text_block();
        in_flight.text_delta(text, "discarded".into());
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "old".into());
        let weak = {
            let buffer = alerter.inner.buffer.lock().unwrap();
            Arc::downgrade(buffer[0].position.as_ref().unwrap())
        };
        for _ in 0..MAX_BUFFERED_ALERTS {
            alerter.alert(AlertLevel::Warn, AlertSource::Worker, "new".into());
        }
        assert!(weak.upgrade().is_none());
        let (before, _) = alerter.subscribe_with_snapshot();
        in_flight.clear();
        let next = in_flight.start_text_block();
        in_flight.text_delta(next, "discarded".into());
        let item = session_store::LoggedItem::Message {
            role: session_store::LoggedRole::Assistant,
            content: vec![session_store::LoggedContentPart::Text {
                text: "discarded".into(),
            }],
        };
        in_flight.clear_for_committed_item_then(&item, Some("unrelated-entry"), || ());
        let (after, _) = alerter.subscribe_with_snapshot();
        assert_eq!(before.len(), MAX_BUFFERED_ALERTS);
        assert_eq!(
            serde_json::to_value(before).unwrap(),
            serde_json::to_value(after).unwrap()
        );
    }

    #[test]
    fn snapshot_does_not_acquire_in_flight_lock_under_buffer_lock() {
        let (alerter, _, in_flight) = positioned_alerter();
        alerter.alert(AlertLevel::Warn, AlertSource::Worker, "before".into());
        let guard = in_flight.snapshot_guard();
        let cloned = alerter.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (snapshot, _) = cloned.subscribe_with_snapshot();
            tx.send(snapshot).unwrap();
        });
        let result = rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(guard);
        thread.join().unwrap();
        assert_eq!(result.unwrap().len(), 1);
    }

    #[test]
    fn alert_broadcasts_to_existing_subscriber() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let alerter = Alerter::new(tx);
        let (_snapshot, mut rx) = alerter.subscribe_with_snapshot();

        alerter.alert(
            AlertLevel::Warn,
            AlertSource::Compactor,
            "test message".into(),
        );

        match rx.try_recv() {
            Ok(Event::Alert(a)) => assert_eq!(a.message, "test message"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn late_subscriber_sees_earlier_alerts_via_snapshot() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let alerter = Alerter::new(tx);

        alerter.alert(AlertLevel::Error, AlertSource::Worker, "first".into());
        alerter.alert(AlertLevel::Warn, AlertSource::AgentsMd, "second".into());

        let (snapshot, mut rx) = alerter.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].message, "first");
        assert_eq!(snapshot[1].message, "second");
        assert!(rx.try_recv().is_err()); // nothing pending on the receiver
    }

    #[test]
    fn buffer_discards_oldest_past_cap() {
        let (tx, _keep) = broadcast::channel::<Event>(1024);
        let alerter = Alerter::new(tx);

        for i in 0..(MAX_BUFFERED_ALERTS + 50) {
            alerter.alert(AlertLevel::Warn, AlertSource::Engine, format!("msg-{i}"));
        }

        let (snapshot, _rx) = alerter.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), MAX_BUFFERED_ALERTS);
        // First 50 were evicted; the oldest remaining is msg-50.
        assert_eq!(snapshot.first().unwrap().message, "msg-50");
        let last = format!("msg-{}", MAX_BUFFERED_ALERTS + 49);
        assert_eq!(snapshot.last().unwrap().message, last);
    }

    #[test]
    fn subscribe_snapshot_and_live_do_not_overlap() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let alerter = Alerter::new(tx);

        alerter.alert(AlertLevel::Warn, AlertSource::Engine, "historic".into());
        let (snapshot, mut rx) = alerter.subscribe_with_snapshot();
        alerter.alert(AlertLevel::Error, AlertSource::Engine, "live".into());

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message, "historic");
        match rx.try_recv() {
            Ok(Event::Alert(a)) => assert_eq!(a.message, "live"),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }
}
