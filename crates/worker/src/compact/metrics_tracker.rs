//! Sync buffer for `session_metrics::Metric` values queued from inside
//! Engine callbacks (which run synchronously and cannot themselves
//! perform `async` store writes).
//!
//! Worker drains this buffer in `persist_turn` and writes each metric via
//! `session_metrics::record_metric`, alongside the regular `LlmUsage`
//! entries.

use std::sync::Mutex;

use session_metrics::Metric;

pub(crate) struct MetricsTracker {
    pending: Mutex<Vec<Metric>>,
}

impl MetricsTracker {
    pub(crate) fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Queue a metric for the next `persist_turn` flush.
    pub(crate) fn push(&self, metric: Metric) {
        self.pending.lock().unwrap().push(metric);
    }

    /// Drain all queued metrics. Called by Worker after a run completes.
    pub(crate) fn drain(&self) -> Vec<Metric> {
        std::mem::take(&mut *self.pending.lock().unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_drain_returns_in_order_and_clears() {
        let t = MetricsTracker::new();
        t.push(Metric::now("a"));
        t.push(Metric::now("b"));
        let drained = t.drain();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].name, "a");
        assert_eq!(drained[1].name, "b");
        assert!(t.drain().is_empty());
    }
}
