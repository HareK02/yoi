//! Pending-notify buffer for `Method::Notify`.
//!
//! Notify entries are queued here by the Controller and drained by
//! `PodInterceptor::pre_llm_request` into the per-request context
//! (never into the Worker's persistent history). Each queued entry
//! becomes one `Item::system_message` in the outgoing request.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use llm_worker::Item;
use tracing::warn;

use crate::prompt::catalog::{CatalogError, PromptCatalog};

/// Maximum queued notify entries. Oldest entries are dropped beyond this.
const CAPACITY: usize = 128;

/// One pending notify entry awaiting injection into the next LLM request.
#[derive(Debug, Clone)]
pub struct PendingNotify {
    pub message: String,
}

/// Shared, mutex-guarded buffer of pending notify entries.
///
/// Cloned between the Pod (producer) and PodInterceptor (consumer).
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
    pub fn push(&self, message: String) {
        let mut q = self.inner.lock().expect("notify buffer poisoned");
        if q.len() >= CAPACITY {
            let dropped = q.pop_front();
            warn!(
                capacity = CAPACITY,
                dropped_message = dropped.as_ref().map(|n| n.message.as_str()),
                "notify buffer overflow; dropped oldest"
            );
        }
        q.push_back(PendingNotify { message });
    }

    /// Remove and return all pending notify entries in FIFO order.
    pub fn drain(&self) -> Vec<PendingNotify> {
        let mut q = self.inner.lock().expect("notify buffer poisoned");
        q.drain(..).collect()
    }

    /// Number of pending notify entries. Primarily for tests.
    pub fn len(&self) -> usize {
        self.inner.lock().expect("notify buffer poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Format a single pending notify entry into the `Item::system_message`
/// that gets injected into the per-request context. The wrapper body
/// comes from `PodPrompt::NotifyWrapper` so the surrounding phrasing
/// can be customised via a prompt pack (translation, tone, ...).
pub(crate) fn format_notify(
    n: &PendingNotify,
    prompts: &PromptCatalog,
) -> Result<Item, CatalogError> {
    let text = prompts.notify_wrapper(&n.message)?;
    Ok(Item::system_message(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_preserves_order() {
        let buf = NotifyBuffer::new();
        buf.push("one".into());
        buf.push("two".into());
        let drained = buf.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "one");
        assert_eq!(drained[1].message, "two");
        assert!(buf.is_empty());
    }

    #[test]
    fn capacity_drops_oldest() {
        let buf = NotifyBuffer::new();
        for i in 0..(CAPACITY + 5) {
            buf.push(format!("msg{i}"));
        }
        let drained = buf.drain();
        assert_eq!(drained.len(), CAPACITY);
        // Oldest 5 were dropped; first retained is msg5.
        assert_eq!(drained[0].message, "msg5");
        assert_eq!(drained[CAPACITY - 1].message, format!("msg{}", CAPACITY + 4));
    }

    #[test]
    fn format_notify_includes_message_and_nonblocking_hint() {
        let n = PendingNotify {
            message: "hello".into(),
        };
        let catalog = PromptCatalog::builtins_only().unwrap();
        let item = format_notify(&n, &catalog).unwrap();
        let text = item.as_text().unwrap_or_default().to_string();
        assert!(text.contains("[Notification]"));
        assert!(text.contains("hello"));
        assert!(text.contains("not a blocking request"));
    }
}
