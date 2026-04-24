//! User-facing notification channel for Pod → client.
//!
//! Separate from `tracing` (which is for developer logs). Notifications
//! are short human-readable messages the Pod layer wants a client to
//! see — for example "compaction failed", "tool output truncated".
//!
//! Each notification is broadcast on the shared `Event` channel and
//! also appended to an in-memory buffer so that clients connecting
//! after the fact still see everything emitted during the session.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use protocol::{Event, Notification, NotificationLevel, NotificationSource};

/// Upper bound on buffered notifications. When exceeded, the oldest
/// entries are discarded so a long-running session cannot leak
/// memory through a pathological loop of recurring notifications
/// (e.g. compaction failing every turn).
const MAX_BUFFERED_NOTIFICATIONS: usize = 512;

#[derive(Clone)]
pub struct Notifier {
    inner: Arc<Inner>,
}

struct Inner {
    event_tx: broadcast::Sender<Event>,
    buffer: Mutex<VecDeque<Notification>>,
}

impl Notifier {
    pub fn new(event_tx: broadcast::Sender<Event>) -> Self {
        Self {
            inner: Arc::new(Inner {
                event_tx,
                buffer: Mutex::new(VecDeque::with_capacity(MAX_BUFFERED_NOTIFICATIONS)),
            }),
        }
    }

    /// Record and broadcast a notification.
    ///
    /// The broadcast may have no subscribers (e.g. during Pod
    /// construction before any client has connected); the buffer
    /// guarantees the message is still delivered once a client
    /// attaches.
    ///
    /// The buffer mutex is held across `broadcast::send` to make
    /// `subscribe_with_snapshot` race-free — a client that snapshots
    /// the buffer while holding the same lock sees every notification
    /// exactly once: older ones from the snapshot, newer ones from
    /// the freshly-subscribed receiver.
    pub fn notify(&self, level: NotificationLevel, source: NotificationSource, message: String) {
        let notification = Notification {
            level,
            source,
            message,
            timestamp_ms: now_ms(),
        };
        if let Ok(mut buf) = self.inner.buffer.lock() {
            if buf.len() >= MAX_BUFFERED_NOTIFICATIONS {
                buf.pop_front();
            }
            buf.push_back(notification.clone());
            let _ = self
                .inner
                .event_tx
                .send(Event::Notification(notification));
        }
    }

    /// Subscribe and atomically snapshot the current buffer.
    ///
    /// The returned snapshot contains notifications emitted before
    /// this call; the receiver will deliver notifications emitted
    /// after. A notification cannot appear in both.
    pub fn subscribe_with_snapshot(&self) -> (Vec<Notification>, broadcast::Receiver<Event>) {
        let buf = self
            .inner
            .buffer
            .lock()
            .expect("notifier buffer mutex poisoned");
        let rx = self.inner.event_tx.subscribe();
        let snapshot: Vec<Notification> = buf.iter().cloned().collect();
        (snapshot, rx)
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

    #[test]
    fn notify_broadcasts_to_existing_subscriber() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let notifier = Notifier::new(tx);
        let (_snapshot, mut rx) = notifier.subscribe_with_snapshot();

        notifier.notify(
            NotificationLevel::Warn,
            NotificationSource::Compactor,
            "test message".into(),
        );

        match rx.try_recv() {
            Ok(Event::Notification(n)) => assert_eq!(n.message, "test message"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn late_subscriber_sees_earlier_notifications_via_snapshot() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let notifier = Notifier::new(tx);

        notifier.notify(
            NotificationLevel::Error,
            NotificationSource::Pod,
            "first".into(),
        );
        notifier.notify(
            NotificationLevel::Warn,
            NotificationSource::AgentsMd,
            "second".into(),
        );

        let (snapshot, mut rx) = notifier.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), 2);
        assert_eq!(snapshot[0].message, "first");
        assert_eq!(snapshot[1].message, "second");
        assert!(rx.try_recv().is_err()); // nothing pending on the receiver
    }

    #[test]
    fn buffer_discards_oldest_past_cap() {
        let (tx, _keep) = broadcast::channel::<Event>(1024);
        let notifier = Notifier::new(tx);

        for i in 0..(MAX_BUFFERED_NOTIFICATIONS + 50) {
            notifier.notify(
                NotificationLevel::Warn,
                NotificationSource::Worker,
                format!("msg-{i}"),
            );
        }

        let (snapshot, _rx) = notifier.subscribe_with_snapshot();
        assert_eq!(snapshot.len(), MAX_BUFFERED_NOTIFICATIONS);
        // First 50 were evicted; the oldest remaining is msg-50.
        assert_eq!(snapshot.first().unwrap().message, "msg-50");
        let last = format!("msg-{}", MAX_BUFFERED_NOTIFICATIONS + 49);
        assert_eq!(snapshot.last().unwrap().message, last);
    }

    #[test]
    fn subscribe_snapshot_and_live_do_not_overlap() {
        let (tx, _keep) = broadcast::channel::<Event>(8);
        let notifier = Notifier::new(tx);

        notifier.notify(
            NotificationLevel::Warn,
            NotificationSource::Worker,
            "historic".into(),
        );
        let (snapshot, mut rx) = notifier.subscribe_with_snapshot();
        notifier.notify(
            NotificationLevel::Error,
            NotificationSource::Worker,
            "live".into(),
        );

        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].message, "historic");
        match rx.try_recv() {
            Ok(Event::Notification(n)) => assert_eq!(n.message, "live"),
            other => panic!("unexpected: {other:?}"),
        }
        assert!(rx.try_recv().is_err());
    }
}
