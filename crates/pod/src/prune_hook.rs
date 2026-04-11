//! PruneHook — applies conditional pruning before each LLM request.
//!
//! Wraps [`llm_worker::prune::prune()`] as a [`Hook<PreLlmRequest>`] so
//! that Pod can register it in the hook pipeline.

use async_trait::async_trait;
use llm_worker::interceptor::PreRequestAction;
use llm_worker::prune::{PruneConfig, prune};
use llm_worker::Item;
use tracing::debug;

use crate::hook::{Hook, PreLlmRequest};

/// Hook that conditionally prunes old tool-result content before each
/// LLM request, reclaiming context-window tokens.
pub struct PruneHook {
    config: PruneConfig,
}

impl PruneHook {
    pub fn new(config: PruneConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl Hook<PreLlmRequest> for PruneHook {
    async fn call(&self, context: &mut Vec<Item>) -> PreRequestAction {
        if let Some(result) = prune(context, &self.config) {
            debug!(
                pruned = result.pruned_count,
                estimated_savings = result.estimated_savings,
                "Pruned old tool-result content"
            );
        }
        PreRequestAction::Continue
    }
}
