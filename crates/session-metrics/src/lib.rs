//! Session metrics — generic ad-hoc measurement lane on top of
//! `LogEntry::Extension { domain: "metrics" }`.
//!
//! セッション中に積み上げて後で引きたい値（prune の発火頻度・Hook の実行
//! 時間・ツールリトライ回数 等）を session-log に乗せるための薄い層。
//! session-store は payload を不透明な `serde_json::Value` として扱うので、
//! このクレートは型と読み書きヘルパーだけを提供する。
//!
//! # 設計
//!
//! - 厳格な label set は持たない。次元は sparse な `BTreeMap<String,String>`、
//!   観測できない値は `None` で明示する
//! - 「後から埋まる値」（例: prune 発火直後の `cache_read_tokens`）は前 entry に
//!   書き戻さず、`correlation_id` を共有する別 metric として流す。集計は読み手で join
//! - 集計 / 可視化には [`read_session_metrics`] / [`read_segment_metrics`] /
//!   [`export_metrics_jsonl`] の明示的な metrics 専用経路を使う。通常の
//!   Session snapshot は `Extension` を公開しない。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use session_store::{
    LogEntry, SegmentId, SegmentOrigin, SessionId, Store, StoreError, save_extension, segment_log,
};

/// Domain tag used in `LogEntry::Extension` for all metrics records.
pub const DOMAIN: &str = "metrics";

/// 単発の計測値。`name` は `namespace.metric` 形式の自由文字列
/// （例: `"prune.fire"`、`"hook.duration"`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    /// `namespace.metric` 形式の名前。
    pub name: String,
    /// epoch ms。
    pub ts: u64,
    /// sparse な次元（label）。観測できないものはキー自体を入れない。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dimensions: BTreeMap<String, String>,
    /// 主スカラ値。dimension では表現したくない数値を載せる場所。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// 関連 metric を join するためのキー。同一 ID を持つ複数 metric は
    /// 「同じ事象を多面的に観測している」という意味付けで読まれる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl Metric {
    /// 最小コンストラクタ。`ts` は呼び出し時刻（epoch ms）で埋める。
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

/// `LogEntry::Extension { domain: "metrics", payload: <metric> }` を append する。
///
/// `save_extension` の薄い wrapper。書き込み失敗は呼び出し側に返す
/// （メトリクスのために本体処理を止めるかは呼び出し側の判断）。
pub fn record_metric(
    store: &impl Store,
    session_id: SessionId,
    segment_id: SegmentId,
    metric: &Metric,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(metric).expect("Metric serialization cannot fail");
    save_extension(store, session_id, segment_id, DOMAIN, payload)
}

/// `RestoredState.extensions` から metrics domain の payload を順に取り出し、
/// `Metric` 列に fold する。
///
/// schema 変更で deserialize できない payload は無視する（後方互換）。
pub fn metrics_from_extensions(extensions: &[(String, serde_json::Value)]) -> Vec<Metric> {
    extensions
        .iter()
        .filter(|(domain, _)| domain == DOMAIN)
        .filter_map(|(_, payload)| serde_json::from_value::<Metric>(payload.clone()).ok())
        .collect()
}

/// A metric together with its durable Session/Segment origin.
///
/// `compacted_from` is copied from the Segment start record so readers can
/// reconstruct compaction lineage without inferring relationships from metric
/// names or timestamps.
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

/// Read every metric for a Session across all of its Segments.
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
        // dimensions が空 / value/correlation_id が None の時は出力に含めない。
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
        // 将来 schema が変わって読めない payload も skip して落ちない。
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
