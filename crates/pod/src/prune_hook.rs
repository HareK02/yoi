//! PruneHook — applies conditional pruning before each LLM request.
//!
//! Wraps the pure `prune` API from `llm-worker` as a [`Hook<PreLlmRequest>`].
//! `min_savings` の判定は usage 履歴ベースのトークン会計
//! ([`crate::token_counter::savings_for_drop_impl`]) で行う。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_worker::Item;
use llm_worker::interceptor::PreRequestAction;
use llm_worker::prune::{PruneConfig, apply_prune, prunable_indices};
use session_store::UsageRecord;
use tracing::debug;

use crate::hook::{Hook, PreLlmRequest};
use crate::token_counter::{EstimateSource, savings_for_drop_impl};

/// Hook that conditionally prunes old tool-result content before each
/// LLM request, reclaiming context-window tokens.
///
/// `usage_history` は [`crate::Pod::usage_history_handle`] から共有された
/// `Arc<Mutex<_>>`。リクエスト直前に snapshot を取って savings を見積もる。
pub struct PruneHook {
    config: PruneConfig,
    usage_history: Arc<Mutex<Vec<UsageRecord>>>,
}

impl PruneHook {
    pub fn new(config: PruneConfig, usage_history: Arc<Mutex<Vec<UsageRecord>>>) -> Self {
        Self {
            config,
            usage_history,
        }
    }
}

#[async_trait]
impl Hook<PreLlmRequest> for PruneHook {
    async fn call(&self, context: &mut Vec<Item>) -> PreRequestAction {
        let candidates = prunable_indices(context, self.config.protected_turns);
        if candidates.is_empty() {
            return PreRequestAction::Continue;
        }

        // 候補範囲のトークン節約量を usage 履歴ベースで見積もる。
        // content だけ削除する場合の上限値（範囲全体を消した場合の savings）として
        // 近似する。実際の content drop は items 数を変えないので、本来の savings
        // はこの値以下。閾値判定は上振れ方向＝「やや prune を発動しやすい」側で安全。
        let first = *candidates.first().unwrap();
        let last = *candidates.last().unwrap() + 1;
        let snapshot = self
            .usage_history
            .lock()
            .expect("usage_history poisoned")
            .clone();
        let savings = savings_for_drop_impl(context, &snapshot, first..last);

        // measurement が無い場合 (NoData) は判定材料がないので prune を見送る。
        // 最初の LLM call が走るまでは usage_history が空なのでこのパスを通る。
        if matches!(savings.source, EstimateSource::NoData) {
            return PreRequestAction::Continue;
        }

        if savings.tokens < self.config.min_savings {
            return PreRequestAction::Continue;
        }

        let result = apply_prune(context, &candidates);
        if result.pruned_count > 0 {
            debug!(
                pruned = result.pruned_count,
                estimated_savings_tokens = savings.tokens,
                source = ?savings.source,
                "Pruned old tool-result content"
            );
        }
        PreRequestAction::Continue
    }
}
