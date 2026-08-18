//! Pending-notify buffer for `Method::Notify` and `Method::WorkerEvent`.
//!
//! Entries are queued here by the Controller (on receipt of the
//! corresponding IPC method) and drained by
//! `WorkerInterceptor::pending_history_appends`, which the Engine calls
//! at the head of each turn loop iteration. The drain renders each
//! pending entry into a typed `SystemItem` (with the `notify_wrapper`
//! prompt applied), commits a `LogEntry::SystemItem` per entry through
//! the session-log sink, and returns the corresponding
//! `Item::system_message`s for the worker to append to its
//! persistent history.
//!
//! This is the **single lane** for "system messages produced by Worker
//! state that should land in the next LLM request": Notify,
//! agent-visible WorkerEvent variants, and any future `<system-reminder>`
//! injection all ride this queue.
//! Per `tickets/notify-history-persist.md` and `AGENTS.md` (LLM
//! context の加工原則), there is **no** "transient, history-skipping"
//! lane — everything injected into a request is also committed to
//! history so any LLM reaction has a visible trigger across turns,
//! resume, and compaction, and so the Anthropic prompt cache prefix
//! stays stable across requests.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use protocol::WorkerEvent;
use session_store::SystemItem;
use tracing::warn;

use crate::prompt::catalog::{CatalogError, PromptCatalog};

/// Maximum queued pending entries. Oldest entries are dropped beyond this.
const CAPACITY: usize = 128;

/// One pending entry awaiting drain into the next LLM request.
///
/// The buffer keeps the raw input shape so the drain step can decide
/// the right `SystemItem` kind (and apply `notify_wrapper` to the
/// rendered body) at the moment of commit, when the prompt catalog
/// is available.
#[derive(Debug, Clone)]
pub enum PendingNotify {
    Notify { message: String, auto_run: bool },
    WorkerEvent { event: WorkerEvent },
}

/// Shared, mutex-guarded buffer of pending entries.
///
/// Cloned between the Worker (producer) and WorkerInterceptor (consumer).
#[derive(Clone, Default)]
pub struct NotifyBuffer {
    inner: Arc<Mutex<VecDeque<PendingNotify>>>,
}

impl NotifyBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a notify entry onto the queue. If the queue is full, the
    /// oldest entry is dropped and a `tracing::warn` is emitted — the
    /// caller should never hit this in normal operation.
    pub fn push_notify(&self, message: String, auto_run: bool) {
        self.push_entry(PendingNotify::Notify { message, auto_run });
    }

    /// Push a typed worker-event entry onto the queue.
    pub fn push_worker_event(&self, event: WorkerEvent) {
        self.push_entry(PendingNotify::WorkerEvent { event });
    }

    fn push_entry(&self, entry: PendingNotify) {
        let mut q = self.inner.lock().expect("notify buffer poisoned");
        if q.len() >= CAPACITY {
            let dropped = q.pop_front();
            warn!(
                capacity = CAPACITY,
                dropped = ?dropped,
                "notify buffer overflow; dropped oldest"
            );
        }
        q.push_back(entry);
    }

    /// Remove and return all pending entries in FIFO order.
    pub fn drain(&self) -> Vec<PendingNotify> {
        let mut q = self.inner.lock().expect("notify buffer poisoned");
        q.drain(..).collect()
    }

    /// Whether an undrained `Method::Notify { auto_run: true }` remains.
    pub fn has_auto_run_pending(&self) -> bool {
        self.inner
            .lock()
            .expect("notify buffer poisoned")
            .iter()
            .any(|entry| matches!(entry, PendingNotify::Notify { auto_run: true, .. }))
    }

    /// Number of pending entries. Primarily for tests.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("notify buffer poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Render one pending entry into a typed `SystemItem`. The
/// `notify_wrapper` prompt produces the LLM-context body for both
/// `Notify` (raw message) and `WorkerEvent` (rendered event line).
#[cfg(test)]
pub(crate) fn build_system_item(
    entry: &PendingNotify,
    prompts: &PromptCatalog,
) -> Result<SystemItem, CatalogError> {
    build_system_item_with_provenance(entry, prompts, None)
}

pub(crate) fn build_system_item_with_provenance(
    entry: &PendingNotify,
    prompts: &PromptCatalog,
    prompt_provenance: Option<session_store::PromptRenderProvenance>,
) -> Result<SystemItem, CatalogError> {
    match entry {
        PendingNotify::Notify { message, .. } => {
            let body = prompts.notify_wrapper(message)?;
            Ok(SystemItem::Notification {
                message: message.clone(),
                body,
                prompt_provenance,
            })
        }
        PendingNotify::WorkerEvent { event } => {
            let rendered = session_store::render_worker_event(event);
            let body = prompts.notify_wrapper(&rendered)?;
            Ok(SystemItem::WorkerEvent {
                event: event.clone(),
                body,
                prompt_provenance,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_preserves_order() {
        let buf = NotifyBuffer::new();
        buf.push_notify("one".into(), false);
        assert!(!buf.has_auto_run_pending());
        buf.push_notify("two".into(), true);
        assert!(buf.has_auto_run_pending());
        let drained = buf.drain();
        assert!(!buf.has_auto_run_pending());
        assert_eq!(drained.len(), 2);
        match &drained[0] {
            PendingNotify::Notify { message, .. } => assert_eq!(message, "one"),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(buf.is_empty());
    }

    #[test]
    fn capacity_drops_oldest() {
        let buf = NotifyBuffer::new();
        for i in 0..(CAPACITY + 5) {
            buf.push_notify(format!("msg{i}"), false);
        }
        let drained = buf.drain();
        assert_eq!(drained.len(), CAPACITY);
        match &drained[0] {
            PendingNotify::Notify { message, .. } => assert_eq!(message, "msg5"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn build_system_item_for_notify_carries_wrapper_body() {
        let entry = PendingNotify::Notify {
            message: "hello".into(),
            auto_run: false,
        };
        let catalog = PromptCatalog::builtins_only().unwrap();
        let item = build_system_item(&entry, &catalog).unwrap();
        match item {
            SystemItem::Notification { message, body, .. } => {
                assert_eq!(message, "hello");
                assert!(body.contains("[Notification]"));
                assert!(body.contains("hello"));
                assert!(body.contains("not a blocking request"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn build_system_item_for_worker_event_wraps_rendered_event_text() {
        let entry = PendingNotify::WorkerEvent {
            event: WorkerEvent::TurnEnded {
                worker_name: "child".into(),
            },
        };
        let catalog = PromptCatalog::builtins_only().unwrap();
        let item = build_system_item(&entry, &catalog).unwrap();
        match item {
            SystemItem::WorkerEvent { event, body, .. } => {
                assert!(
                    matches!(event, WorkerEvent::TurnEnded { ref worker_name } if worker_name == "child")
                );
                assert!(body.contains("[Notification]"));
                assert!(body.contains("`child`"));
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}
