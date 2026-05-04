//! Prune integration — wires the Worker's prune projection to the Pod's
//! usage-history-backed token accounting.
//!
//! Worker 自身がコンテキスト射影を行う（`worker.rs` の `request_context` 構築
//! 直後）。Worker は usage 履歴を知らないので、`min_savings` 判定に使う savings
//! の見積もりはコールバックで外部から注入する。このモジュールはそのコールバック
//! を組み立てて Worker に差し込むための `impl Pod` を提供する。
//!
//! 同じ経路で `PruneObserver` も install し、評価のたびに `prune.fire` /
//! `prune.skip` metric を `MetricsTracker` に積む。`Fired` 時は uuid を
//! `UsageTracker` にも stash しておき、後続の `LlmUsage` と組で
//! `prune.post_request` を吐けるようにする。

use llm_worker::Item;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::prune::{PruneConfig, PruneDecision, PruneObserver, SavingsEstimator};
use session_metrics::Metric;
use session_store::Store;

use crate::Pod;
use crate::compact::token_counter::{EstimateSource, savings_for_prune_impl};

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// Enable prune projection on the underlying Worker.
    ///
    /// Registers the config and a savings-estimator closure on the Worker.
    /// The estimator captures a shared handle to [`Pod::usage_history_handle`]
    /// so that every LLM request sees the latest measurements.
    ///
    /// Measurement-less estimates (before the first LLM call, or immediately
    /// after a compact) return `0` from the estimator, which naturally
    /// prevents the prune projection from firing until usage data exists.
    ///
    /// Also installs a [`PruneObserver`] that pushes `prune.fire` /
    /// `prune.skip` metrics into the shared [`MetricsTracker`]. On `Fired`
    /// the observer additionally stashes a fresh correlation_id in
    /// [`UsageTracker`] so the next `LlmUsage` can be paired with a
    /// `prune.post_request` metric carrying the same id.
    pub fn attach_prune(&mut self, config: PruneConfig) {
        let usage = self.usage_history_handle();
        let estimator: SavingsEstimator = Box::new(move |history: &[Item], indices| {
            let snapshot = usage.lock().expect("usage_history poisoned").clone();
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
                if let Some(border) = eval.border_turn {
                    metric = metric.with_dimension("border_turn", border.to_string());
                }
                metrics.push(metric);
                usage_tracker.note_correlation_id(correlation_id);
            }
            PruneDecision::SkippedNoCandidates => {
                metrics.push(Metric::now("prune.skip").with_dimension("reason", "no_candidates"));
            }
            PruneDecision::SkippedBelowMinSavings => {
                metrics.push(
                    Metric::now("prune.skip")
                        .with_dimension("reason", "below_min_savings")
                        .with_dimension("candidate_count", eval.candidate_count.to_string())
                        .with_value(eval.estimated_savings as f64),
                );
            }
        });

        let worker = self.worker_mut();
        worker.set_prune_config(Some(config));
        worker.set_savings_estimator(Some(estimator));
        worker.set_prune_observer(Some(observer));
    }

    /// If the manifest has a `[compaction]` section, build a `PruneConfig`
    /// from its `prune_*` fields and call [`attach_prune`](Self::attach_prune).
    /// Otherwise no-op. Called from all Pod constructors so prune is
    /// active whenever the manifest asks for it.
    pub(crate) fn apply_prune_from_manifest(&mut self) {
        let Some(compaction) = self.manifest().compaction.as_ref() else {
            return;
        };
        let config = PruneConfig {
            protected_turns: compaction.prune_protected_turns,
            min_savings: compaction.prune_min_savings,
        };
        self.attach_prune(config);
    }
}
