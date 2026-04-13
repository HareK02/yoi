//! Prune integration — wires the Worker's prune projection to the Pod's
//! usage-history-backed token accounting.
//!
//! Worker 自身がコンテキスト射影を行う（`worker.rs` の `request_context` 構築
//! 直後）。Worker は usage 履歴を知らないので、`min_savings` 判定に使う savings
//! の見積もりはコールバックで外部から注入する。このモジュールはそのコールバック
//! を組み立てて Worker に差し込むための `impl Pod` を提供する。

use llm_worker::Item;
use llm_worker::llm_client::client::LlmClient;
use llm_worker::prune::{PruneConfig, SavingsEstimator};
use session_store::Store;

use crate::Pod;
use crate::token_counter::{EstimateSource, savings_for_prune_impl};

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
        let worker = self.worker_mut();
        worker.set_prune_config(Some(config));
        worker.set_savings_estimator(Some(estimator));
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
