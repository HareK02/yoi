//! Pending-notification buffer for `Method::Notify`.
//!
//! Notifications are queued here by the Controller and drained by
//! `PodInterceptor::pre_llm_request` into the per-request context
//! (never into the Worker's persistent history). Each queued entry
//! becomes one `Item::system_message` in the outgoing request.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use llm_worker::Item;
use tracing::warn;

/// Maximum queued notifications. Oldest entries are dropped beyond this.
const CAPACITY: usize = 128;

/// One pending notification awaiting injection into the next LLM request.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub message: String,
}

/// Shared, mutex-guarded buffer of pending notifications.
///
/// Cloned between the Pod (producer) and PodInterceptor (consumer).
#[derive(Clone, Default)]
pub struct NotificationBuffer {
    inner: Arc<Mutex<VecDeque<PendingNotification>>>,
}

impl NotificationBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a notification onto the queue. If the queue is full, the
    /// oldest entry is dropped and a `tracing::warn` is emitted — the
    /// caller should never hit this in normal operation.
    pub fn push(&self, message: String) {
        let mut q = self.inner.lock().expect("notification buffer poisoned");
        if q.len() >= CAPACITY {
            let dropped = q.pop_front();
            warn!(
                capacity = CAPACITY,
                dropped_message = dropped.as_ref().map(|n| n.message.as_str()),
                "notification buffer overflow; dropped oldest"
            );
        }
        q.push_back(PendingNotification { message });
    }

    /// Remove and return all pending notifications in FIFO order.
    pub fn drain(&self) -> Vec<PendingNotification> {
        let mut q = self.inner.lock().expect("notification buffer poisoned");
        q.drain(..).collect()
    }

    /// Number of pending notifications. Primarily for tests.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("notification buffer poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Format a single pending notification into the `Item::system_message`
/// that gets injected into the per-request context.
pub(crate) fn format_notification(n: &PendingNotification) -> Item {
    let text = format!(
        "[Notification]\n{message}\n\n\
         This is a notification, not a blocking request. \
         If you are in the middle of a task, continue your current work \
         and address this at a natural stopping point.",
        message = n.message,
    );
    Item::system_message(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_preserves_order() {
        let buf = NotificationBuffer::new();
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
        let buf = NotificationBuffer::new();
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
    fn format_notification_includes_message_and_nonblocking_hint() {
        let n = PendingNotification {
            message: "hello".into(),
        };
        let item = format_notification(&n);
        let text = item.as_text().unwrap_or_default().to_string();
        assert!(text.contains("[Notification]"));
        assert!(text.contains("hello"));
        assert!(text.contains("not a blocking request"));
    }
}
