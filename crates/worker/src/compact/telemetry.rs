use std::collections::BTreeMap;
use std::time::Duration;

use agen::token_counter::EstimateSource;
use agen::usage_record::UsageRecord;
use session_metrics::Metric;
use session_store::{SegmentId, SessionId};

use super::usage_tracker::{PostRequestMetric, UsageSnapshot};

const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactMode {
    Manual,
    Automatic,
}

impl CompactMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Automatic => "automatic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactThresholdPolicy {
    Manual,
    PreRun,
    RequestThreshold,
}

impl CompactThresholdPolicy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::PreRun => "pre_run",
            Self::RequestThreshold => "request_threshold",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompactFailureCategory {
    Cancelled,
    SummaryMissing,
    SummaryTooLarge,
    ResultContextTooLarge,
    ActiveSegmentCommit,
    Storage,
    InternalWorker,
    Preparation,
    Other,
}

impl CompactFailureCategory {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::SummaryMissing => "summary_missing",
            Self::SummaryTooLarge => "summary_too_large",
            Self::ResultContextTooLarge => "result_context_too_large",
            Self::ActiveSegmentCommit => "active_segment_commit",
            Self::Storage => "storage",
            Self::InternalWorker => "internal_worker",
            Self::Preparation => "preparation",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompactAttempt {
    correlation_id: String,
    session_id: SessionId,
    source_segment_id: SegmentId,
    mode: CompactMode,
    threshold_policy: CompactThresholdPolicy,
    pre_context_tokens: u64,
    pre_context_source: EstimateSource,
    retained_token_budget: u64,
}

impl CompactAttempt {
    pub(crate) fn new(
        correlation_id: String,
        session_id: SessionId,
        source_segment_id: SegmentId,
        mode: CompactMode,
        threshold_policy: CompactThresholdPolicy,
        pre_context_tokens: u64,
        pre_context_source: EstimateSource,
        retained_token_budget: u64,
    ) -> Self {
        debug_assert!(uuid::Uuid::parse_str(&correlation_id).is_ok());
        Self {
            correlation_id,
            session_id,
            source_segment_id,
            mode,
            threshold_policy,
            pre_context_tokens,
            pre_context_source,
            retained_token_budget,
        }
    }

    pub(crate) fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    pub(crate) fn start_metric(&self) -> Metric {
        self.metric("compact.start")
            .with_value(safe_metric_number(self.pre_context_tokens))
            .with_dimension("occupancy_source", estimate_source(self.pre_context_source))
            .with_dimension(
                "retained_token_budget",
                self.retained_token_budget.to_string(),
            )
    }

    pub(crate) fn success_metrics(
        &self,
        result_segment_id: SegmentId,
        elapsed: Duration,
        stats: &CompactSuccessStats,
    ) -> Vec<Metric> {
        let dimensions = self.base_dimensions();
        let correlation_id = self.correlation_id.clone();
        let mut metrics = vec![
            metric_with_context("compact.finish", 1, &dimensions, &correlation_id)
                .with_dimension("outcome", "succeeded")
                .with_dimension("result_segment_id", result_segment_id.to_string())
                .with_dimension("retained_items", stats.retained_items.to_string())
                .with_dimension("summarized_items", stats.summarized_items.to_string()),
            metric_with_context(
                "compact.retained_tokens",
                stats.retained_tokens,
                &dimensions,
                &correlation_id,
            )
            .with_dimension("source", estimate_source(stats.retained_tokens_source)),
            metric_with_context(
                "compact.overview_tokens",
                stats.overview_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.summary_tokens",
                stats.summary_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.auto_read_tokens",
                stats.auto_read_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.result_context_tokens",
                stats.result_context_tokens,
                &dimensions,
                &correlation_id,
            )
            .with_dimension("source", estimate_source(stats.result_context_source)),
            metric_with_context(
                "compact.input_tokens",
                stats.usage.input_total_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.output_tokens",
                stats.usage.output_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.cache_read_tokens",
                stats.usage.cache_read_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.cache_creation_tokens",
                stats.usage.cache_write_tokens,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.requests",
                stats.requests,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context("compact.turns", stats.turns, &dimensions, &correlation_id),
            metric_with_context(
                "compact.tool_calls",
                stats.tool_calls,
                &dimensions,
                &correlation_id,
            ),
            metric_with_context(
                "compact.duration_ms",
                elapsed.as_millis().min(u128::from(MAX_SAFE_INTEGER)) as u64,
                &dimensions,
                &correlation_id,
            ),
        ];
        for metric in &mut metrics {
            metric
                .dimensions
                .insert("result_segment_id".into(), result_segment_id.to_string());
        }
        // Provider UsageEvent currently carries tokens but no price or cost. Keep
        // the field explicit and valueless rather than fabricating a zero cost.
        metrics.push(
            self.metric("compact.cost_usd")
                .with_dimension("status", "unavailable")
                .with_dimension("reason", "provider_usage_unpriced")
                .with_dimension("result_segment_id", result_segment_id.to_string()),
        );
        metrics
    }

    pub(crate) fn failure_metrics(
        &self,
        observed_segment_id: SegmentId,
        elapsed: Duration,
        category: CompactFailureCategory,
    ) -> [Metric; 2] {
        let outcome = if category == CompactFailureCategory::Cancelled {
            "cancelled"
        } else {
            "failed"
        };
        let outcome_metric = self
            .metric("compact.finish")
            .with_value(1.0)
            .with_dimension("outcome", outcome)
            .with_dimension("failure_category", category.as_str())
            .with_dimension("observed_segment_id", observed_segment_id.to_string());
        let duration_metric = self
            .metric("compact.duration_ms")
            .with_value(elapsed.as_millis().min(u128::from(MAX_SAFE_INTEGER)) as f64)
            .with_dimension("outcome", outcome)
            .with_dimension("observed_segment_id", observed_segment_id.to_string());
        [outcome_metric, duration_metric]
    }

    fn metric(&self, name: &'static str) -> Metric {
        let mut metric = Metric::now(name).with_correlation_id(&self.correlation_id);
        metric.dimensions = self.base_dimensions();
        metric
    }

    fn base_dimensions(&self) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("session_id".into(), self.session_id.to_string()),
            (
                "source_segment_id".into(),
                self.source_segment_id.to_string(),
            ),
            ("mode".into(), self.mode.as_str().into()),
            ("trigger".into(), self.threshold_policy.as_str().into()),
            (
                "threshold_policy".into(),
                self.threshold_policy.as_str().into(),
            ),
        ])
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CompactSuccessStats {
    pub(crate) retained_items: u64,
    pub(crate) summarized_items: u64,
    pub(crate) retained_tokens: u64,
    pub(crate) retained_tokens_source: EstimateSource,
    pub(crate) overview_tokens: u64,
    pub(crate) summary_tokens: u64,
    pub(crate) auto_read_tokens: u64,
    pub(crate) result_context_tokens: u64,
    pub(crate) result_context_source: EstimateSource,
    pub(crate) usage: UsageSnapshot,
    pub(crate) requests: u64,
    pub(crate) turns: u64,
    pub(crate) tool_calls: u64,
}

fn metric_with_context(
    name: &'static str,
    value: u64,
    dimensions: &BTreeMap<String, String>,
    correlation_id: &str,
) -> Metric {
    let mut metric = Metric::now(name)
        .with_value(safe_metric_number(value))
        .with_correlation_id(correlation_id);
    metric.dimensions = dimensions.clone();
    metric
}

pub(crate) fn new_compact_metric_correlation_id(lifecycle_id: &str) -> String {
    loop {
        let correlation_id = uuid::Uuid::now_v7().to_string();
        if correlation_id != lifecycle_id {
            return correlation_id;
        }
    }
}

pub(crate) fn correlated_post_request_metric(
    kind: PostRequestMetric,
    correlation_id: &str,
    record: &UsageRecord,
) -> Metric {
    let value = match kind {
        PostRequestMetric::Prune => record.cache_read_tokens,
        PostRequestMetric::Compaction => record.input_total_tokens,
    };
    Metric::now(kind.name())
        .with_correlation_id(correlation_id)
        .with_value(safe_metric_number(value))
        .with_dimension("history_len", record.history_len.to_string())
        .with_dimension("input_total_tokens", record.input_total_tokens.to_string())
        .with_dimension("cache_read_tokens", record.cache_read_tokens.to_string())
        .with_dimension("cache_write_tokens", record.cache_write_tokens.to_string())
        .with_dimension("output_tokens", record.output_tokens.to_string())
}

pub(crate) fn safe_metric_number(value: u64) -> f64 {
    value.min(MAX_SAFE_INTEGER) as f64
}

fn estimate_source(source: EstimateSource) -> &'static str {
    match source {
        EstimateSource::Measured => "provider",
        EstimateSource::Interpolated | EstimateSource::Extrapolated | EstimateSource::NoData => {
            "fallback"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_metrics_use_fixed_bounded_labels_and_safe_numbers() {
        let attempt = CompactAttempt::new(
            uuid::Uuid::now_v7().to_string(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            CompactMode::Automatic,
            CompactThresholdPolicy::RequestThreshold,
            u64::MAX,
            EstimateSource::Measured,
            500,
        );
        let start = attempt.start_metric();
        assert_eq!(start.name, "compact.start");
        assert_eq!(start.value, Some(MAX_SAFE_INTEGER as f64));
        assert_eq!(start.dimensions["mode"], "automatic");
        assert_eq!(start.dimensions["trigger"], "request_threshold");
        assert_eq!(start.dimensions["threshold_policy"], "request_threshold");
        assert_eq!(start.dimensions["occupancy_source"], "provider");
        assert!(start.correlation_id.is_some());
        assert!(start.dimensions.keys().all(|key| key.len() <= 32));
        assert!(start.dimensions.values().all(|value| value.len() <= 64));
    }

    #[test]
    fn occupancy_sources_match_the_public_provider_fallback_schema() {
        assert_eq!(estimate_source(EstimateSource::Measured), "provider");
        assert_eq!(estimate_source(EstimateSource::Interpolated), "fallback");
        assert_eq!(estimate_source(EstimateSource::Extrapolated), "fallback");
        assert_eq!(estimate_source(EstimateSource::NoData), "fallback");
    }

    #[test]
    fn metric_correlation_id_is_distinct_from_lifecycle_identity() {
        let lifecycle_id = uuid::Uuid::now_v7().to_string();
        let correlation_id = new_compact_metric_correlation_id(&lifecycle_id);
        assert_ne!(correlation_id, lifecycle_id);
        assert!(uuid::Uuid::parse_str(&correlation_id).is_ok());
    }

    #[test]
    fn post_request_metric_saturates_values_above_json_safe_integer() {
        let record = UsageRecord {
            history_len: 1,
            input_total_tokens: u64::MAX,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 1,
        };
        let metric = correlated_post_request_metric(
            PostRequestMetric::Compaction,
            "018f6f8a-9822-7b11-8b35-706f30313700",
            &record,
        );
        assert_eq!(metric.name, "compact.post_request");
        assert_eq!(metric.value, Some(MAX_SAFE_INTEGER as f64));
        assert_eq!(
            metric.dimensions["input_total_tokens"],
            u64::MAX.to_string()
        );
    }

    #[test]
    fn failure_metrics_never_include_error_text() {
        let attempt = CompactAttempt::new(
            uuid::Uuid::now_v7().to_string(),
            uuid::Uuid::now_v7(),
            uuid::Uuid::now_v7(),
            CompactMode::Manual,
            CompactThresholdPolicy::Manual,
            1,
            EstimateSource::NoData,
            1,
        );
        let [metric, duration] = attempt.failure_metrics(
            uuid::Uuid::now_v7(),
            Duration::from_millis(7),
            CompactFailureCategory::InternalWorker,
        );
        let encoded = serde_json::to_string(&metric).unwrap();
        assert_eq!(metric.value, Some(1.0));
        assert_eq!(metric.dimensions["outcome"], "failed");
        assert_eq!(duration.name, "compact.duration_ms");
        assert_eq!(duration.value, Some(7.0));
        assert!(encoded.contains("internal_worker"));
        assert!(!encoded.contains("error"));
        assert!(!encoded.contains("path"));
    }
}
