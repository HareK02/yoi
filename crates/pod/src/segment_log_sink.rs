//! Pod-side session-log mirror + broadcast.
//!
//! Owns the in-memory `Vec<LogEntry>` mirror that backs `Event::Snapshot`
//! delivery to newly connected clients and the
//! `broadcast::Sender<LogEntry>` that fans out per-entry commits to
//! existing subscribers. Disk writes remain the responsibility of the
//! Pod (which still owns the `Store` handle); the sink stays focused on
//! the wire-side fan-out.
//!
//! Atomicity contract:
//!
//! 1. Pod writes the entry to disk via the `Store`.
//! 2. Pod calls [`SegmentLogSink::publish`] which acquires the mirror
//!    mutex, pushes the entry, and fires `broadcast::send` — all under
//!    the same critical section.
//!
//! [`SegmentLogSink::subscribe_with_snapshot`] takes the same mutex,
//! so the `(snapshot, receiver)` pair returned to a connecting client
//! splits the entry sequence cleanly: every entry shows up in exactly
//! one of `snapshot` or on `receiver`.
//!
//! Disk-write failures short-circuit before `publish`, so a failed
//! entry never appears in the mirror or on the broadcast.

use std::sync::{Arc, Mutex as StdMutex};

use session_store::LogEntry;
use tokio::sync::broadcast;

/// Broadcast capacity for the live receiver. Slow subscribers that
/// fall behind will see `RecvError::Lagged` and are expected to drop
/// the connection so that the next reconnect's `subscribe_with_snapshot`
/// re-seeds the prefix.
const BROADCAST_CAPACITY: usize = 256;

/// In-memory mirror + broadcast fan-out for the active session log.
///
/// Clone is cheap (`Arc` clone) — the Pod hands one to the IPC layer
/// for read-only `subscribe_with_snapshot` access and keeps one for
/// its own write path.
#[derive(Clone)]
pub struct SegmentLogSink {
    inner: Arc<SinkInner>,
}

struct SinkInner {
    /// Full session log mirror in commit order. Reset on session swap
    /// (compaction / fork) via [`SegmentLogSink::reset_with_initial`].
    mirror: StdMutex<Vec<LogEntry>>,
    /// Broadcast channel for live entry updates. The same `Sender`
    /// survives session swaps so existing subscribers keep their
    /// receiver — they observe the swap as a freshly broadcast
    /// `LogEntry::SegmentStart` and reset their view accordingly.
    broadcast_tx: broadcast::Sender<LogEntry>,
}

impl SegmentLogSink {
    /// Create a fresh sink with an empty mirror. Used before any entry
    /// has been written (deferred SegmentStart) or as a placeholder in
    /// tests.
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(SinkInner {
                mirror: StdMutex::new(Vec::new()),
                broadcast_tx,
            }),
        }
    }

    /// Create a sink seeded with a prefix of entries already on disk.
    /// Used by restore / fork-at-restore code paths that materialise
    /// the existing log before the sink starts taking new commits.
    pub fn with_initial(entries: Vec<LogEntry>) -> Self {
        let (broadcast_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            inner: Arc::new(SinkInner {
                mirror: StdMutex::new(entries),
                broadcast_tx,
            }),
        }
    }

    /// Push `entry` to the mirror; selectively broadcast it.
    ///
    /// MUST be called only after the Pod has successfully persisted the
    /// entry to the underlying `Store` — disk write is the gate. Failed
    /// disk writes must not call `publish`.
    ///
    /// Live broadcast fires for committed session-log entries that
    /// socket clients must see in log order:
    ///   - `LogEntry::SegmentStart` → `Event::SegmentRotated` on the wire.
    ///   - `LogEntry::UserInput`    → `Event::UserMessage`.
    ///   - `LogEntry::SystemItem`   → `Event::SystemItem`.
    ///   - `LogEntry::Invoke`       → `Event::InvokeStart`.
    /// Everything else (AssistantItem, ToolResult, TurnEnd,
    /// RunCompleted, RunErrored, PausedTurnAbandoned, LlmUsage, Extension,
    /// ConfigChanged) is reflected in the mirror so reconnect snapshots stay accurate,
    /// but is not sent live — the streaming events (TextDelta /
    /// ToolCallStart / ToolResult / TurnEnd / etc.) already provide
    /// that data, and re-broadcasting it as a typed entry would just
    /// double-render every block on the client side.
    pub fn publish(&self, entry: LogEntry) {
        let mut mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        mirror.push(entry.clone());
        if Self::is_live_relevant(&entry) {
            // SendError means there are zero subscribers; harmless. The
            // mirror lock is held across `send` so subscribers cannot
            // observe an inconsistent (snapshot, receiver) pair.
            let _ = self.inner.broadcast_tx.send(entry);
        }
    }

    /// `true` for entry kinds that the IPC layer forwards to clients
    /// as a typed live event.
    fn is_live_relevant(entry: &LogEntry) -> bool {
        matches!(
            entry,
            LogEntry::SegmentStart { .. }
                | LogEntry::UserInput { .. }
                | LogEntry::SystemItem { .. }
                | LogEntry::Invoke { .. }
        )
    }

    /// Atomically swap the mirror to `[initial]` and broadcast the new
    /// session-start entry. Used during compaction / fork: the new
    /// `LogEntry::SegmentStart` is the first entry of the replacement
    /// session, and existing subscribers transition by replaying it
    /// like any other live entry.
    ///
    /// Existing snapshot prefixes seen by old subscribers stay valid
    /// for the prior session; the new `SegmentStart` on the broadcast
    /// is the signal to reset their derived view.
    pub fn reset_with_initial(&self, initial: LogEntry) {
        let mut mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        mirror.clear();
        mirror.push(initial.clone());
        let _ = self.inner.broadcast_tx.send(initial);
    }

    /// Atomically swap the mirror to the supplied replacement-session prefix
    /// and broadcast the first entry as the live rotation signal. Entries after
    /// the first are already reflected in reconnect snapshots but are not
    /// broadcast live; this is intended for non-live extension state that must
    /// share the new segment prefix with SegmentStart.
    pub fn reset_with_initial_entries(&self, entries: Vec<LogEntry>) {
        let first = entries.first().cloned();
        let mut mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        *mirror = entries;
        if let Some(initial) = first {
            let _ = self.inner.broadcast_tx.send(initial);
        }
    }

    /// Replace the mirror with the supplied prefix without broadcasting.
    ///
    /// Used by restore paths that load a session's complete log into
    /// the mirror before any subscriber is connected. Callers that need
    /// to notify existing subscribers should use [`reset_with_initial`].
    pub fn replace_silent(&self, entries: Vec<LogEntry>) {
        let mut mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        *mirror = entries;
    }

    /// Truncate the mirror without broadcasting.
    pub fn truncate_silent(&self, entries_len: usize) {
        let mut mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        mirror.truncate(entries_len);
    }

    /// Atomically read the current mirror and subscribe to subsequent
    /// commits. The returned snapshot and receiver split the entry
    /// timeline into a duplicate-free, gap-free prefix/suffix pair.
    pub fn subscribe_with_snapshot(&self) -> (Vec<LogEntry>, broadcast::Receiver<LogEntry>) {
        let mirror = self
            .inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned");
        let snapshot = mirror.clone();
        let rx = self.inner.broadcast_tx.subscribe();
        (snapshot, rx)
    }

    /// Current entry count. Useful for tests / diagnostics.
    pub fn len(&self) -> usize {
        self.inner
            .mirror
            .lock()
            .expect("session log mirror mutex poisoned")
            .len()
    }

    /// Whether the mirror is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SegmentLogSink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_engine::llm_client::RequestConfig;
    use session_store::segment_log::now_millis;

    fn session_start() -> LogEntry {
        LogEntry::SegmentStart {
            ts: now_millis(),
            session_id: uuid::Uuid::nil(),
            system_prompt: None,
            config: RequestConfig::default(),
            history: vec![],
            forked_from: None,
            compacted_from: None,
        }
    }

    fn turn_end(n: usize) -> LogEntry {
        LogEntry::TurnEnd {
            ts: now_millis(),
            turn_count: n,
        }
    }

    fn user_input(text: &str) -> LogEntry {
        LogEntry::UserInput {
            ts: now_millis(),
            segments: vec![protocol::Segment::Text {
                content: text.to_owned(),
            }],
        }
    }

    #[test]
    fn publish_then_subscribe_returns_history_in_snapshot() {
        let sink = SegmentLogSink::new();
        sink.publish(session_start());
        sink.publish(turn_end(1));

        let (snapshot, mut rx) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(matches!(snapshot[0], LogEntry::SegmentStart { .. }));
        assert!(matches!(
            snapshot[1],
            LogEntry::TurnEnd { turn_count: 1, .. }
        ));
        assert!(rx.try_recv().is_err());
    }

    fn notification_entry(text: &str) -> LogEntry {
        LogEntry::SystemItem {
            ts: now_millis(),
            item: session_store::SystemItem::Notification {
                message: text.to_owned(),
                body: format!("[Notification] {text}"),
            },
        }
    }

    #[test]
    fn subscribe_then_publish_delivers_only_live_relevant_entries() {
        let sink = SegmentLogSink::new();
        sink.publish(session_start());

        let (snapshot, mut rx) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 1);

        // TurnEnd is mirror-only — no live broadcast.
        sink.publish(turn_end(1));
        assert!(rx.try_recv().is_err(), "TurnEnd must not be broadcast live");

        // UserInput is live-relevant because it is the persisted source
        // for Event::UserMessage.
        sink.publish(user_input("hi from log"));
        match rx.try_recv() {
            Ok(LogEntry::UserInput { segments, .. }) => {
                assert_eq!(segments.len(), 1);
            }
            other => panic!("expected UserInput, got {other:?}"),
        }

        // SystemItem is live-relevant.
        sink.publish(notification_entry("hi"));
        match rx.try_recv() {
            Ok(LogEntry::SystemItem { .. }) => {}
            other => panic!("expected SystemItem, got {other:?}"),
        }

        // Mirror still grew with all entries (snapshot completeness).
        let (after_snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(after_snapshot.len(), 4);
    }

    #[test]
    fn snapshot_and_live_never_overlap() {
        let sink = SegmentLogSink::new();
        sink.publish(session_start());
        let (snapshot, mut rx) = sink.subscribe_with_snapshot();
        sink.publish(notification_entry("post-snapshot"));

        assert_eq!(snapshot.len(), 1);
        match rx.try_recv() {
            Ok(LogEntry::SystemItem { .. }) => {}
            other => panic!("unexpected: {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn reset_with_initial_clears_and_broadcasts() {
        let sink = SegmentLogSink::new();
        sink.publish(session_start());
        sink.publish(turn_end(1));

        let (_pre_snapshot, mut rx) = sink.subscribe_with_snapshot();
        sink.reset_with_initial(session_start());

        match rx.try_recv() {
            Ok(LogEntry::SegmentStart { .. }) => {}
            other => panic!("expected SegmentStart broadcast, got {other:?}"),
        }

        let (post_snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(post_snapshot.len(), 1);
        assert!(matches!(post_snapshot[0], LogEntry::SegmentStart { .. }));
    }

    #[test]
    fn replace_silent_does_not_broadcast() {
        let sink = SegmentLogSink::new();
        sink.publish(session_start());
        let (_pre_snapshot, mut rx) = sink.subscribe_with_snapshot();

        sink.replace_silent(vec![session_start(), turn_end(1)]);

        // No broadcast fired.
        assert!(rx.try_recv().is_err());
        let (post_snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(post_snapshot.len(), 2);
    }

    #[test]
    fn with_initial_seeds_the_mirror() {
        let sink = SegmentLogSink::with_initial(vec![session_start(), turn_end(1)]);
        let (snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
