use std::sync::Arc;

use agen::{HistoryEntry, UsageRecord};
use serde_json::Value;

use crate::session_history::SessionHistoryMetadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommittedRunExit {
    Finished,
    NonFinal,
    Interrupted,
}

/// Immutable projection of one durably committed session-log location.
///
/// Feature code receives this value only after the host has committed the
/// terminal run record. The projection deliberately carries annotated history
/// rather than the public flattened transcript so provenance-sensitive
/// features can construct their own bounded views.
#[derive(Clone)]
pub(crate) struct CommittedSessionCapture {
    pub(crate) session_id: String,
    pub(crate) segment_id: String,
    /// Monotonic committed-log revision for the captured Segment.
    pub(crate) session_revision: u64,
    pub(crate) entry_count: usize,
    pub(crate) run_exit: CommittedRunExit,
    pub(crate) history: Vec<HistoryEntry<SessionHistoryMetadata>>,
    pub(crate) usage_history: Vec<UsageRecord>,
    pub(crate) extensions: Vec<(String, Value)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommittedSessionLocation {
    pub(crate) session_id: String,
    pub(crate) segment_id: String,
    /// Monotonic committed-log revision for the captured Segment.
    pub(crate) session_revision: u64,
    pub(crate) entry_count: usize,
}

impl CommittedSessionCapture {
    pub(crate) fn location(&self) -> CommittedSessionLocation {
        CommittedSessionLocation {
            session_id: self.session_id.clone(),
            segment_id: self.segment_id.clone(),
            session_revision: self.session_revision,
            entry_count: self.entry_count,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum FeatureSessionError {
    #[error("read committed session failed: {0}")]
    Capture(String),
    #[error("append session extension failed: {0}")]
    Extension(String),
}

#[derive(Clone)]
pub(crate) struct CommittedSessionCaptureHandle {
    capture: Arc<
        dyn Fn() -> Result<CommittedSessionCapture, FeatureSessionError> + Send + Sync + 'static,
    >,
}

impl CommittedSessionCaptureHandle {
    pub(crate) fn new(
        capture: impl Fn() -> Result<CommittedSessionCapture, FeatureSessionError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            capture: Arc::new(capture),
        }
    }

    pub(crate) fn capture(&self) -> Result<CommittedSessionCapture, FeatureSessionError> {
        (self.capture)()
    }
}

#[derive(Clone)]
pub(crate) struct SessionExtensionHandle {
    append: Arc<
        dyn Fn(&CommittedSessionLocation, &str, Value) -> Result<bool, FeatureSessionError>
            + Send
            + Sync
            + 'static,
    >,
}

impl SessionExtensionHandle {
    pub(crate) fn new(
        append: impl Fn(&CommittedSessionLocation, &str, Value) -> Result<bool, FeatureSessionError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            append: Arc::new(append),
        }
    }

    /// Appends an extension only while the committed session is still at the
    /// exact location captured by the feature. `Ok(false)` is a stale-write
    /// fence, not an I/O failure.
    pub(crate) fn append_if_current(
        &self,
        expected: &CommittedSessionLocation,
        domain: &str,
        payload: Value,
    ) -> Result<bool, FeatureSessionError> {
        (self.append)(expected, domain, payload)
    }
}
