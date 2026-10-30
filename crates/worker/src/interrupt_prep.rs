//! Pre-run cleanup that fires when a Worker transitions out of `Paused`
//! into a fresh turn via new user input.
//!
//! The previously in-flight turn is treated as finished. Any orphan
//! `Item::ToolCall` (tool_use emitted by the LLM but whose tool did not
//! run to completion before the pause) is closed with a synthetic
//! `Item::ToolResult` so the next request is wire-valid under providers
//! that require every `tool_use` to be followed by a matching
//! `tool_result` (Anthropic). A short system note is then inserted so
//! the LLM understands the prior work was cut short. Both side effects
//! happen at the front of `Worker::run` when
//! `worker.last_run_interrupted()` is set; see `Worker::apply_interrupt_prep`.

#[cfg(test)]
use crate::prompt::catalog::PromptCatalog;
use agen::{Item, ToolResultDisposition};

/// Build synthetic `Item::ToolResult` items for every unanswered
/// `Item::ToolCall` in `history`, preserving order.
pub(crate) fn orphan_tool_result_closures(history: &[Item], summary: &str) -> Vec<Item> {
    let mut pending: Vec<Option<&str>> = Vec::new();
    for item in history {
        match item {
            Item::ToolCall { call_id, .. } => pending.push(Some(call_id)),
            Item::ToolResult { call_id, .. } => {
                if let Some(index) = pending
                    .iter()
                    .position(|entry| *entry == Some(call_id.as_str()))
                {
                    pending[index] = None;
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for call_id in pending.into_iter().flatten() {
        out.push(Item::tool_result_item_with_disposition_and_attachments(
                    call_id,
                    summary,
                    Some(
                        "Execution ended before completion could be confirmed. Completion and side effects are unknown."
                            .to_string(),
                    ),
                    ToolResultDisposition::OutcomeUnknown,
                    Vec::new(),
                ));
    }
    out
}

/// Test-only helper to surface the canonical interrupt tool-result
/// summary without round-tripping through a Worker — used by tests in
/// this module that validate the closure logic.
#[cfg(test)]
fn interrupt_tool_result_summary() -> String {
    PromptCatalog::builtins_only()
        .unwrap()
        .interrupt_tool_result_summary()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answered_provider_id_does_not_hide_later_unanswered_logical_call() {
        let history = vec![
            Item::tool_call("reused", "plugin", "{}"),
            Item::tool_result("reused", "first finished"),
            Item::tool_call("reused", "plugin", "{}"),
        ];
        let closed = orphan_tool_result_closures(&history, "Interrupted");
        assert_eq!(closed.len(), 1);
        assert!(
            matches!(&closed[0], Item::ToolResult { call_id, disposition: ToolResultDisposition::OutcomeUnknown, .. } if call_id == "reused")
        );
    }

    #[test]
    fn no_orphans_returns_empty() {
        let history = vec![Item::user_message("hi"), Item::assistant_message("hello")];
        let summary = interrupt_tool_result_summary();
        assert!(orphan_tool_result_closures(&history, &summary).is_empty());
    }

    #[test]
    fn paired_call_and_result_is_not_orphan() {
        let history = vec![
            Item::tool_call("c1", "Read", "{}"),
            Item::tool_result("c1", "ok"),
        ];
        let summary = interrupt_tool_result_summary();
        assert!(orphan_tool_result_closures(&history, &summary).is_empty());
    }

    #[test]
    fn unanswered_call_becomes_closure() {
        let history = vec![Item::tool_call("c1", "Read", "{}")];
        let summary = interrupt_tool_result_summary();
        let out = orphan_tool_result_closures(&history, &summary);
        assert_eq!(out.len(), 1);
        match &out[0] {
            Item::ToolResult {
                call_id,
                summary: got,
                disposition,
                ..
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(got, &summary);
                assert_eq!(*disposition, ToolResultDisposition::OutcomeUnknown);
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
        let summary = interrupt_tool_result_summary();
        let out = orphan_tool_result_closures(&history, &summary);
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
