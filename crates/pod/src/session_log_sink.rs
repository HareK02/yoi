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
//! 2. Pod calls [`SessionLogSink::publish`] which acquires the mirror
//!    mutex, pushes the entry, and fires `broadcast::send` — all under
//!    the same critical section.
//!
//! [`SessionLogSink::subscribe_with_snapshot`] takes the same mutex,
//! so the `(snapshot, receiver)` pair returned to a connecting client
//! splits the entry sequence cleanly: every entry shows up in exactly
//! one of `snapshot` or on `receiver`.
//!
//! Disk-write failures short-circuit before `publish`, so a failed
//! entry never appears in the mirror or on the broadcast.

use std::sync::{Arc, Mutex as StdMutex};

use parking_lot::{Mutex, MutexGuard};
use session_store::{
    EntryHash, HashedEntry, LogEntry, SessionId, SessionStartState, Store, StoreError, session_log,
};
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
pub struct SessionLogSink {
    inner: Arc<SinkInner>,
}

struct SinkInner {
    /// Full session log mirror in commit order. Reset on session swap
    /// (compaction / fork) via [`SessionLogSink::reset_with_initial`].
    mirror: StdMutex<Vec<LogEntry>>,
    /// Broadcast channel for live entry updates. The same `Sender`
    /// survives session swaps so existing subscribers keep their
    /// receiver — they observe the swap as a freshly broadcast
    /// `LogEntry::SessionStart` and reset their view accordingly.
    broadcast_tx: broadcast::Sender<LogEntry>,
}

impl SessionLogSink {
    /// Create a fresh sink with an empty mirror. Used before any entry
    /// has been written (deferred SessionStart) or as a placeholder in
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
    /// Live broadcast fires only for entries that the streaming-event
    /// lane does not cover:
    ///   - `LogEntry::SessionStart` → `Event::SessionRotated` on the wire.
    ///   - `LogEntry::SystemItem`   → `Event::SystemItem`.
    /// Everything else (AssistantItem, ToolResult, UserInput, TurnEnd,
    /// RunCompleted, RunErrored, LlmUsage, Extension, ConfigChanged) is
    /// reflected in the mirror so reconnect snapshots stay accurate,
    /// but is not sent live — the streaming events (TextDelta /
    /// ToolCallStart / ToolResult / UserMessage / TurnEnd / etc.)
    /// already provide that data, and re-broadcasting it as a typed
    /// entry would just double-render every block on the client side.
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
            LogEntry::SessionStart { .. } | LogEntry::SystemItem { .. }
        )
    }

    /// Atomically swap the mirror to `[initial]` and broadcast the new
    /// session-start entry. Used during compaction / fork: the new
    /// `LogEntry::SessionStart` is the first entry of the replacement
    /// session, and existing subscribers transition by replaying it
    /// like any other live entry.
    ///
    /// Existing snapshot prefixes seen by old subscribers stay valid
    /// for the prior session; the new `SessionStart` on the broadcast
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

impl Default for SessionLogSink {
    fn default() -> Self {
        Self::new()
    }
}

/// Active session head for the Pod's persistent log: session id +
/// last-committed entry hash. Bundled with the store + sink in a
/// `SessionLogWriter` so the worker callback / interceptor can share
/// one cheap `Clone` handle for direct sync appends.
#[derive(Debug, Clone)]
pub struct SessionHeadState {
    pub session_id: SessionId,
    pub head_hash: Option<EntryHash>,
}

/// Pod-side session-log writer.
///
/// Bundles the (1) persistent store, (2) the in-memory session-head
/// state (id + hash), and (3) the broadcast sink. `append_entry`
/// chains the hash on disk, advances the head, then publishes the
/// entry through the sink — under a single sync mutex so two writers
/// cannot interleave the chain.
///
/// All append paths run synchronously: a local-fs `<1 KiB` JSONL line
/// completes well below a millisecond, and going through an async
/// `tokio::fs` ferry would re-introduce the `LogCommand` / drain task
/// we removed. `parking_lot::Mutex` is safe to hold across the disk
/// write since the lock is never crossed by an `.await`.
///
/// `Clone` is a cheap `Arc` clone. The Pod keeps one writer for its
/// inline commits (UserInput, TurnEnd, Usage, RunCompleted/Errored,
/// scope snapshots, metrics) and hands clones to every other commit
/// site (worker callback, interceptor).
pub struct SessionLogWriter<St> {
    inner: Arc<WriterInner<St>>,
}

struct WriterInner<St> {
    store: St,
    head: Mutex<SessionHeadState>,
    sink: SessionLogSink,
}

impl<St> Clone for SessionLogWriter<St> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<St> SessionLogWriter<St>
where
    St: Store + Clone,
{
    /// Create a writer for a fresh Pod with no entries on disk yet.
    /// `head_hash` is `None` until the first `append_entry` (typically
    /// the deferred `SessionStart` written by `ensure_session_head`).
    pub fn new(store: St, session_id: SessionId) -> Self {
        Self {
            inner: Arc::new(WriterInner {
                store,
                head: Mutex::new(SessionHeadState {
                    session_id,
                    head_hash: None,
                }),
                sink: SessionLogSink::new(),
            }),
        }
    }

    /// Create a writer seeded with a session already on disk. The
    /// mirror is populated with `mirror` (typically loaded via
    /// `Store::read_all`), and `head_hash` should be the hash of the
    /// last entry.
    pub fn restored(
        store: St,
        session_id: SessionId,
        head_hash: Option<EntryHash>,
        mirror: Vec<LogEntry>,
    ) -> Self {
        Self {
            inner: Arc::new(WriterInner {
                store,
                head: Mutex::new(SessionHeadState {
                    session_id,
                    head_hash,
                }),
                sink: SessionLogSink::with_initial(mirror),
            }),
        }
    }

    /// Append `entry` to the log: disk write → in-memory mirror push →
    /// broadcast — atomic w.r.t. `subscribe_with_snapshot` callers.
    pub fn append_entry(&self, entry: LogEntry) -> Result<EntryHash, StoreError> {
        let mut head = self.inner.head.lock();
        let hash = session_store::append_entry_with_hash(
            &self.inner.store,
            head.session_id,
            &mut head.head_hash,
            entry.clone(),
        )?;
        self.inner.sink.publish(entry);
        Ok(hash)
    }

    /// Atomically swap to a new compacted session.
    ///
    /// Creates the new session on disk with `initial` as its
    /// `SessionStart`, advances the head, and resets the sink mirror
    /// to `[initial]` while broadcasting the entry. Existing
    /// subscribers observe the swap as a freshly broadcast
    /// `SessionStart` (with `compacted_from` set), which is their
    /// signal to reset their derived view.
    pub fn swap_session(
        &self,
        new_session_id: SessionId,
        initial: LogEntry,
    ) -> Result<EntryHash, StoreError> {
        let hash = session_log::compute_hash(None, &initial);
        let hashed = HashedEntry {
            hash: hash.clone(),
            prev_hash: None,
            entry: initial.clone(),
        };
        self.inner.store.create_session(new_session_id, &[hashed])?;
        let mut head = self.inner.head.lock();
        head.session_id = new_session_id;
        head.head_hash = Some(hash.clone());
        self.inner.sink.reset_with_initial(initial);
        Ok(hash)
    }

    /// If the store's head no longer matches our cached head, mint a
    /// fresh session that forks from the current state and switch to
    /// it. Returns `true` when a fork happened.
    pub fn ensure_head_or_fork(&self, state: SessionStartState<'_>) -> Result<bool, StoreError> {
        let mut head = self.inner.head.lock();
        let store_head = self.inner.store.read_head_hash(head.session_id)?;
        if store_head == head.head_hash {
            return Ok(false);
        }
        let fork_id = session_store::new_session_id();
        let entry = LogEntry::SessionStart {
            ts: session_log::now_millis(),
            system_prompt: state.system_prompt.map(String::from),
            config: state.config.clone(),
            history: session_store::to_logged(state.history),
            forked_from: None,
            compacted_from: None,
        };
        let hash = session_log::compute_hash(None, &entry);
        let hashed = HashedEntry {
            hash: hash.clone(),
            prev_hash: None,
            entry: entry.clone(),
        };
        self.inner.store.create_session(fork_id, &[hashed])?;
        head.session_id = fork_id;
        head.head_hash = Some(hash);
        self.inner.sink.reset_with_initial(entry);
        Ok(true)
    }

    /// Cloneable handle to the broadcast sink. Used by the IPC layer
    /// for `subscribe_with_snapshot` and by tests that just want the
    /// non-write side.
    pub fn sink(&self) -> SessionLogSink {
        self.inner.sink.clone()
    }

    /// Underlying store handle. Direct access is preserved for callers
    /// that read state (`read_all`, `read_head_hash`) without going
    /// through the writer's hash chain.
    pub fn store(&self) -> &St {
        &self.inner.store
    }

    /// Cheap snapshot of the current session id.
    pub fn current_session_id(&self) -> SessionId {
        self.inner.head.lock().session_id
    }

    /// Cheap snapshot of the current head hash.
    pub fn current_head_hash(&self) -> Option<EntryHash> {
        self.inner.head.lock().head_hash.clone()
    }

    /// Direct lock on the head. Used by paths that need to coordinate
    /// custom writes with the hash chain.
    pub fn lock_head(&self) -> MutexGuard<'_, SessionHeadState> {
        self.inner.head.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_worker::llm_client::RequestConfig;
    use session_store::session_log::now_millis;

    fn session_start() -> LogEntry {
        LogEntry::SessionStart {
            ts: now_millis(),
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

    #[test]
    fn publish_then_subscribe_returns_history_in_snapshot() {
        let sink = SessionLogSink::new();
        sink.publish(session_start());
        sink.publish(turn_end(1));

        let (snapshot, mut rx) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert!(matches!(snapshot[0], LogEntry::SessionStart { .. }));
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
        let sink = SessionLogSink::new();
        sink.publish(session_start());

        let (snapshot, mut rx) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 1);

        // TurnEnd is mirror-only — no live broadcast.
        sink.publish(turn_end(1));
        assert!(rx.try_recv().is_err(), "TurnEnd must not be broadcast live");

        // SystemItem is live-relevant.
        sink.publish(notification_entry("hi"));
        match rx.try_recv() {
            Ok(LogEntry::SystemItem { .. }) => {}
            other => panic!("expected SystemItem, got {other:?}"),
        }

        // Mirror still grew with both entries (snapshot completeness).
        let (after_snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(after_snapshot.len(), 3);
    }

    #[test]
    fn snapshot_and_live_never_overlap() {
        let sink = SessionLogSink::new();
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
        let sink = SessionLogSink::new();
        sink.publish(session_start());
        sink.publish(turn_end(1));

        let (_pre_snapshot, mut rx) = sink.subscribe_with_snapshot();
        sink.reset_with_initial(session_start());

        match rx.try_recv() {
            Ok(LogEntry::SessionStart { .. }) => {}
            other => panic!("expected SessionStart broadcast, got {other:?}"),
        }

        let (post_snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(post_snapshot.len(), 1);
        assert!(matches!(post_snapshot[0], LogEntry::SessionStart { .. }));
    }

    #[test]
    fn replace_silent_does_not_broadcast() {
        let sink = SessionLogSink::new();
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
        let sink = SessionLogSink::with_initial(vec![session_start(), turn_end(1)]);
        let (snapshot, _) = sink.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
    }
}
