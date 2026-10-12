//! Prune integration — wires the Engine's prune projection to the Worker's
//! usage-history-backed token accounting.
//!
//! The Engine projects context itself, immediately after constructing `request_context` in
//! `worker.rs`. Since it has no usage history, a callback supplies the savings estimate used
//! to evaluate `min_savings`. This module provides `impl Worker` to assemble and install it.
//!
//! The same path installs a `PruneObserver` that adds `prune.fire` / `prune.skip` metrics to
//! `MetricsTracker` on each evaluation. On `Fired`, it also stashes a uuid in `UsageTracker`
//! to pair with the subsequent `LlmUsage` and emit `prune.post_request`.

use agen::Item;
use agen::llm_client::client::LlmClient;
use agen::prune::{PruneConfig, PruneDecision, PruneObserver, SavingsEstimator, TokenEstimator};
use session_metrics::Metric;
use session_store::Store;

use crate::Worker;
use crate::compact::token_counter::{
    EstimateSource, savings_for_prune_impl, token_estimates_for_prune_impl,
};

fn prune_config_from_compaction(compaction: &manifest::CompactionConfig) -> Option<PruneConfig> {
    compaction.prune_enabled.then_some(PruneConfig {
        protected_tokens: compaction.prune_protected_tokens,
        min_savings: compaction.prune_min_savings,
    })
}

impl<C: LlmClient + 'static, St: Store> Worker<C, St> {
    /// Enable prune projection on the underlying Engine.
    ///
    /// Registers the config and token/savings-estimator closures on the Engine.
    /// The estimators combine persisted [`Worker::usage_history_handle`] records
    /// with in-flight `UsageTracker` records so multi-request tool loops can
    /// prune before the surrounding Worker run finishes.
    ///
    /// Measurement-less estimates (before the first LLM call, or immediately
    /// after a compact) return `0` from the estimator, which naturally
    /// prevents the prune projection from firing until usage data exists.
    ///
    /// Also installs a [`PruneObserver`] that pushes `prune.fire` /
    /// `prune.skip` metrics into the shared `MetricsTracker`. On `Fired`
    /// the observer additionally stashes a fresh correlation_id in
    /// `UsageTracker` so the next `LlmUsage` can be paired with a
    /// `prune.post_request` metric carrying the same id.
    pub fn attach_prune(&mut self, config: PruneConfig) {
        let usage_history_for_tokens = self.usage_history_handle();
        let usage_tracker_for_tokens = self.usage_tracker_handle();
        let token_estimator: TokenEstimator = Box::new(move |history: &[Item]| {
            let mut snapshot = usage_history_for_tokens
                .lock()
                .expect("usage_history poisoned")
                .clone();
            snapshot.extend(usage_tracker_for_tokens.records());
            token_estimates_for_prune_impl(history, &snapshot)
        });

        let usage_history_for_savings = self.usage_history_handle();
        let usage_tracker_for_savings = self.usage_tracker_handle();
        let estimator: SavingsEstimator = Box::new(move |history: &[Item], indices| {
            let mut snapshot = usage_history_for_savings
                .lock()
                .expect("usage_history poisoned")
                .clone();
            snapshot.extend(usage_tracker_for_savings.records());
            let est = savings_for_prune_impl(history, &snapshot, indices);
            match est.source {
                EstimateSource::NoData => 0,
                _ => est.tokens,
            }
        });

        let metrics = self.metrics_tracker_handle();
        let usage_tracker = self.usage_tracker_handle();
        let observer: PruneObserver = Box::new(move |eval| match &eval.decision {
            PruneDecision::Fired { .. } => {
                let correlation_id = uuid::Uuid::now_v7().to_string();
                let mut metric = Metric::now("prune.fire")
                    .with_value(eval.estimated_savings as f64)
                    .with_correlation_id(&correlation_id)
                    .with_dimension("candidate_count", eval.candidate_count.to_string());
                if let Some(protected_start) = eval.protected_start_index {
                    metric =
                        metric.with_dimension("protected_start_index", protected_start.to_string());
                }
                metrics.push(metric);
                usage_tracker.note_correlation_id(correlation_id);
            }
            PruneDecision::SkippedNoCandidates => {
                metrics.push(Metric::now("prune.skip").with_dimension("reason", "no_candidates"));
            }
            PruneDecision::SkippedBelowMinSavings => {
                let mut metric = Metric::now("prune.skip")
                    .with_dimension("reason", "below_min_savings")
                    .with_dimension("candidate_count", eval.candidate_count.to_string())
                    .with_value(eval.estimated_savings as f64);
                if let Some(protected_start) = eval.protected_start_index {
                    metric =
                        metric.with_dimension("protected_start_index", protected_start.to_string());
                }
                metrics.push(metric);
            }
        });

        let worker = self.engine_mut();
        worker.set_prune_config(Some(config));
        worker.set_token_estimator(Some(token_estimator));
        worker.set_savings_estimator(Some(estimator));
        worker.set_prune_observer(Some(observer));
    }

    /// If the manifest enables prune under `[compaction]`, build a
    /// `PruneConfig` from its `prune_*` fields and call
    /// [`attach_prune`](Self::attach_prune). Otherwise no-op. Called from all
    /// Worker constructors.
    pub(crate) fn apply_prune_from_manifest(&mut self) {
        let Some(compaction) = self.manifest().compaction.as_ref() else {
            return;
        };
        let Some(config) = prune_config_from_compaction(compaction) else {
            return;
        };
        self.attach_prune(config);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_pruning_does_not_build_prune_config() {
        let compaction = manifest::CompactionConfig {
            prune_enabled: false,
            ..Default::default()
        };

        assert!(prune_config_from_compaction(&compaction).is_none());
    }

    #[test]
    fn enabled_compaction_preserves_prune_thresholds() {
        let compaction = manifest::CompactionConfig {
            prune_enabled: true,
            prune_protected_tokens: 1234,
            prune_min_savings: 5678,
            ..Default::default()
        };

        let config = prune_config_from_compaction(&compaction).unwrap();
        assert_eq!(config.protected_tokens, 1234);
        assert_eq!(config.min_savings, 5678);
    }
}
