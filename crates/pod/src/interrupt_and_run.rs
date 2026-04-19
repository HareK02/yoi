//! Transition from `Paused` to a fresh turn via user input.
//!
//! The previously in-flight turn is treated as finished. Any orphan
//! `Item::ToolCall` (tool_use emitted by the LLM but whose tool did not
//! run to completion before the pause) is closed with a synthetic
//! `Item::ToolResult` so the next request is wire-valid under providers
//! that require every `tool_use` to be followed by a matching
//! `tool_result` (Anthropic). A short system note is then inserted so
//! the LLM understands the prior work was cut short, and finally the
//! user's new input is appended via `worker.run(input)`.

use llm_worker::Item;
use llm_worker::llm_client::client::LlmClient;
use session_store::Store;

use crate::pod::{Pod, PodError, PodRunResult};

const INTERRUPT_TOOL_RESULT_SUMMARY: &str = "[Interrupted by user]";
const INTERRUPT_SYSTEM_NOTE: &str =
    "[The previous turn was interrupted by the user. The user's next request follows.]";

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// Close out the current (paused) turn and start a new one with `input`.
    ///
    /// Invoked by the controller when a `Method::Run` arrives while the
    /// Pod is `Paused`. See module docs for the wire-compatibility
    /// rationale around synthetic tool results.
    pub async fn interrupt_and_run(
        &mut self,
        input: impl Into<String>,
    ) -> Result<PodRunResult, PodError> {
        let closures: Vec<Item> = orphan_tool_result_closures(self.worker().history());
        if !closures.is_empty() {
            self.worker_mut().extend_history(closures);
        }
        self.worker_mut()
            .push_item(Item::system_message(INTERRUPT_SYSTEM_NOTE));
        self.run(input).await
    }
}

/// Build synthetic `Item::ToolResult` items for every unanswered
/// `Item::ToolCall` in `history`, preserving order.
fn orphan_tool_result_closures(history: &[Item]) -> Vec<Item> {
    let mut answered: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for item in history {
        if let Item::ToolResult { call_id, .. } = item {
            answered.insert(call_id.as_str());
        }
    }
    let mut out = Vec::new();
    for item in history {
        if let Item::ToolCall { call_id, .. } = item {
            if !answered.contains(call_id.as_str()) {
                out.push(Item::tool_result(
                    call_id.clone(),
                    INTERRUPT_TOOL_RESULT_SUMMARY,
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_orphans_returns_empty() {
        let history = vec![
            Item::user_message("hi"),
            Item::assistant_message("hello"),
        ];
        assert!(orphan_tool_result_closures(&history).is_empty());
    }

    #[test]
    fn paired_call_and_result_is_not_orphan() {
        let history = vec![
            Item::tool_call("c1", "Read", "{}"),
            Item::tool_result("c1", "ok"),
        ];
        assert!(orphan_tool_result_closures(&history).is_empty());
    }

    #[test]
    fn unanswered_call_becomes_closure() {
        let history = vec![Item::tool_call("c1", "Read", "{}")];
        let out = orphan_tool_result_closures(&history);
        assert_eq!(out.len(), 1);
        match &out[0] {
            Item::ToolResult {
                call_id, summary, ..
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(summary, INTERRUPT_TOOL_RESULT_SUMMARY);
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn multiple_orphans_are_closed_in_order() {
        let history = vec![
            Item::tool_call("c1", "Read", "{}"),
            Item::tool_call("c2", "Write", "{}"),
            Item::tool_result("c1", "ok"),
            Item::tool_call("c3", "Grep", "{}"),
        ];
        let out = orphan_tool_result_closures(&history);
        let ids: Vec<&str> = out
            .iter()
            .map(|i| match i {
                Item::ToolResult { call_id, .. } => call_id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(ids, vec!["c2", "c3"]);
    }
}
