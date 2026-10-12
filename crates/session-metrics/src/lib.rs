#![doc = include_str!("../README.md")]

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use session_store::{
    LogEntry, SegmentId, SegmentOrigin, SessionId, Store, StoreError, save_extension, segment_log,
};

/// Domain tag used in `LogEntry::Extension` for all metrics records.
pub const DOMAIN: &str = "metrics";

/// A single measurement appended to a Session log.
///
/// Stored as an extension payload and read separately from ordinary conversation
/// snapshots. Labels are unrestricted; unobservable dimensions are omitted and
/// missing values use `None`. Values learned later are appended as separate
/// measurements with the same `correlation_id`, not written back to old records.
/// This allows more measurements without changing conversation bodies or existing records.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    /// Name in `namespace.metric` form.
    pub name: String,
    /// Epoch milliseconds.
    pub ts: u64,
    /// Sparse dimensions (labels). Omit keys for unobservable dimensions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dimensions: BTreeMap<String, String>,
    /// Primary scalar value for numbers not represented as dimensions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// Key for joining related metrics. Metrics with the same ID represent
    /// different observations of the same event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl Metric {
    /// Minimal constructor; sets `ts` to the call time in epoch milliseconds.
    pub fn now(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ts: segment_log::now_millis(),
            dimensions: BTreeMap::new(),
            value: None,
            correlation_id: None,
        }
    }

    pub fn with_dimension(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.dimensions.insert(key.into(), value.into());
        self
    }

    pub fn with_value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }
}

/// Appends `LogEntry::Extension { domain: "metrics", payload: <metric> }`.
///
/// Thin wrapper around `save_extension`. Returns write failures to the caller,
/// who decides whether a metrics failure should stop the main operation.
pub fn record_metric(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    metric: &Metric,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(metric).expect("Metric serialization cannot fail");
    save_extension(store, session_id, segment_id, DOMAIN, payload)
}

/// Collects metrics-domain payloads from `RestoredState.extensions` in order
/// into a sequence of `Metric` values.
///
/// Skips payloads that cannot deserialize after schema changes for backward compatibility.
pub fn metrics_from_extensions(extensions: &[(String, serde_json::Value)]) -> Vec<Metric> {
    extensions
        .iter()
        .filter(|(domain, _)| domain == DOMAIN)
        .filter_map(|(_, payload)| serde_json::from_value::<Metric>(payload.clone()).ok())
        .collect()
}

/// A measurement with its storage location and Segment lineage.
///
/// Takes `compacted_from` from the Segment's initial record rather than inferring
/// compaction order from names or timestamps. Use `correlation_id` to relate the
/// same attempt, and `compacted_from` to relate source and resulting Segments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocatedMetric {
    pub session_id: SessionId,
    pub segment_id: SegmentId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compacted_from: Option<SegmentOrigin>,
    pub log_index: usize,
    pub metric: Metric,
}

#[derive(Debug)]
pub enum SessionMetricsError {
    Store(StoreError),
    MissingSegmentStart {
        segment_id: SegmentId,
    },
    SessionMismatch {
        requested: SessionId,
        observed: SessionId,
        segment_id: SegmentId,
    },
    Encode(serde_json::Error),
}

impl std::fmt::Display for SessionMetricsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Store(error) => write!(formatter, "session metrics store error: {error}"),
            Self::MissingSegmentStart { segment_id } => {
                write!(formatter, "segment {segment_id} has no start record")
            }
            Self::SessionMismatch {
                requested,
                observed,
                segment_id,
            } => write!(
                formatter,
                "segment {segment_id} belongs to session {observed}, not {requested}"
            ),
            Self::Encode(error) => write!(formatter, "session metrics encode error: {error}"),
        }
    }
}

impl std::error::Error for SessionMetricsError {}

impl From<StoreError> for SessionMetricsError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<serde_json::Error> for SessionMetricsError {
    fn from(error: serde_json::Error) -> Self {
        Self::Encode(error)
    }
}

/// Read metrics from one exact Segment.
///
/// This is an explicit metrics-only surface. It validates the Segment's
/// durable start record and retains the log position of each metric.
pub fn read_segment_metrics(
    store: &dyn Store,
    session_id: SessionId,
    segment_id: SegmentId,
) -> Result<Vec<LocatedMetric>, SessionMetricsError> {
    let entries = store.read_all(session_id, segment_id)?;
    let (observed_session_id, compacted_from) = entries
        .iter()
        .find_map(|entry| match entry {
            LogEntry::AnnotatedSegmentStart {
                session_id,
                compacted_from,
                ..
            } => Some((*session_id, compacted_from.clone())),
            _ => None,
        })
        .ok_or(SessionMetricsError::MissingSegmentStart { segment_id })?;
    if observed_session_id != session_id {
        return Err(SessionMetricsError::SessionMismatch {
            requested: session_id,
            observed: observed_session_id,
            segment_id,
        });
    }

    Ok(entries
        .iter()
        .enumerate()
        .filter_map(|(log_index, entry)| match entry {
            LogEntry::Extension {
                domain, payload, ..
            } if domain == DOMAIN => {
                serde_json::from_value::<Metric>(payload.clone())
                    .ok()
                    .map(|metric| LocatedMetric {
                        session_id,
                        segment_id,
                        compacted_from: compacted_from.clone(),
                        log_index,
                        metric,
                    })
            }
            _ => None,
        })
        .collect())
}

/// Reads measurements from all Segments with lineage.
///
/// Sorts by timestamp, measurement phase, Segment, and log position for reproducible
/// exports under the same conditions. This display order does not prove causality;
/// use `correlation_id` and `compacted_from` for relationships. Rejects Segments
/// without initial records and Session mismatches.
pub fn read_session_metrics(
    store: &dyn Store,
    session_id: SessionId,
) -> Result<Vec<LocatedMetric>, SessionMetricsError> {
    let mut metrics = Vec::new();
    for segment_id in store.list_segments(session_id)? {
        metrics.extend(read_segment_metrics(store, session_id, segment_id)?);
    }
    metrics.sort_by(|left, right| {
        (
            left.metric.ts,
            metric_phase_order(&left.metric.name),
            left.segment_id,
            left.log_index,
        )
            .cmp(&(
                right.metric.ts,
                metric_phase_order(&right.metric.name),
                right.segment_id,
                right.log_index,
            ))
    });
    Ok(metrics)
}

/// Serialize located metrics as newline-delimited JSON for an explicit export.
pub fn export_metrics_jsonl(metrics: &[LocatedMetric]) -> Result<String, SessionMetricsError> {
    let mut output = String::new();
    for metric in metrics {
        output.push_str(&serde_json::to_string(metric)?);
        output.push('\n');
    }
    Ok(output)
}

fn metric_phase_order(name: &str) -> u8 {
    match name {
        "compact.start" => 0,
        "compact.finish" => 2,
        "compact.post_request" => 3,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_round_trip_via_json() {
        let metric = Metric::now("prune.fire")
            .with_dimension("protected_start_index", "3")
            .with_dimension("candidate_count", "2")
            .with_value(4096.0)
            .with_correlation_id("abc-123");
        let json = serde_json::to_string(&metric).unwrap();
        let parsed: Metric = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, metric);
    }

    #[test]
    fn metric_serializes_minimal_form_compactly() {
        // Omit empty dimensions and None values for value/correlation_id.
        let metric = Metric {
            name: "x".into(),
            ts: 1,
            dimensions: BTreeMap::new(),
            value: None,
            correlation_id: None,
        };
        let json = serde_json::to_string(&metric).unwrap();
        assert!(!json.contains("dimensions"));
        assert!(!json.contains("value"));
        assert!(!json.contains("correlation_id"));
    }

    #[test]
    fn fold_skips_other_domains() {
        let extensions = vec![
            (
                "memory.extract".into(),
                serde_json::json!({ "processed_through_entry": 7 }),
            ),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("a")).unwrap(),
            ),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("b")).unwrap(),
            ),
        ];
        let metrics = metrics_from_extensions(&extensions);
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "a");
        assert_eq!(metrics[1].name, "b");
    }

    #[test]
    fn explicit_reader_and_export_preserve_compaction_lineage() {
        use session_store::FsStore;

        let temp = tempfile::tempdir().unwrap();
        let store = FsStore::new(temp.path()).unwrap();
        let session_id = SessionId::parse_str("018f6f8a-9822-7b11-8b35-706f30313701").unwrap();
        let source_segment_id =
            SegmentId::parse_str("018f6f8a-9822-7b11-8b35-706f30313702").unwrap();
        let result_segment_id =
            SegmentId::parse_str("018f6f8a-9822-7b11-8b35-706f30313703").unwrap();
        let correlation_id = "018f6f8a-9822-7b11-8b35-706f30313700";

        store
            .create_segment(
                session_id,
                source_segment_id,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 1,
                    session_id,
                    system_prompt: None,
                    config: Default::default(),
                    history: Vec::new(),
                    forked_from: None,
                    compacted_from: None,
                }],
            )
            .unwrap();
        let mut start = Metric::now("compact.start").with_correlation_id(correlation_id);
        start.ts = 10;
        record_metric(&store, session_id, source_segment_id, &start).unwrap();

        let origin = SegmentOrigin {
            segment_id: source_segment_id,
            at_turn_index: 0,
        };
        store
            .create_segment(
                session_id,
                result_segment_id,
                &[LogEntry::AnnotatedSegmentStart {
                    ts: 2,
                    session_id,
                    system_prompt: None,
                    config: Default::default(),
                    history: Vec::new(),
                    forked_from: None,
                    compacted_from: Some(origin.clone()),
                }],
            )
            .unwrap();
        let mut finish = Metric::now("compact.finish").with_correlation_id(correlation_id);
        finish.ts = 10;
        record_metric(&store, session_id, result_segment_id, &finish).unwrap();
        let mut post = Metric::now("compact.post_request").with_correlation_id(correlation_id);
        post.ts = 11;
        record_metric(&store, session_id, result_segment_id, &post).unwrap();

        let source_metrics = read_segment_metrics(&store, session_id, source_segment_id).unwrap();
        assert_eq!(source_metrics.len(), 1);
        assert_eq!(source_metrics[0].compacted_from, None);

        let metrics = read_session_metrics(&store, session_id).unwrap();
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0].metric.name, "compact.start");
        let finish = metrics
            .iter()
            .find(|record| record.metric.name == "compact.finish")
            .unwrap();
        assert_eq!(finish.segment_id, result_segment_id);
        assert_eq!(finish.compacted_from, Some(origin));
        assert!(
            metrics
                .iter()
                .all(|record| { record.metric.correlation_id.as_deref() == Some(correlation_id) })
        );

        let exported = export_metrics_jsonl(&metrics).unwrap();
        let ordinary_snapshot = session_store::public_snapshot::project_current_session_snapshot(
            &store.read_all(session_id, result_segment_id).unwrap(),
        );
        let ordinary_json = serde_json::to_string(&ordinary_snapshot).unwrap();
        assert!(!ordinary_json.contains("compact.finish"));
        assert!(!ordinary_json.contains("compact.post_request"));
        let decoded = exported
            .lines()
            .map(|line| serde_json::from_str::<LocatedMetric>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(decoded, metrics);

        let reopened = FsStore::new(temp.path()).unwrap();
        let restored = read_session_metrics(&reopened, session_id).unwrap();
        assert_eq!(restored, metrics);
    }

    #[test]
    fn fold_skips_undeserializable_payloads() {
        // Skip unreadable payloads from future schema changes without failing.
        let extensions = vec![
            (DOMAIN.into(), serde_json::json!({ "garbage": true })),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("ok")).unwrap(),
            ),
        ];
        let metrics = metrics_from_extensions(&extensions);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "ok");
    }
}
