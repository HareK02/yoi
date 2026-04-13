//! Conditional Prune algorithm for context window management.
//!
//! Removes `content` from old [`Item::ToolResult`] entries, leaving only
//! their `summary`. This reclaims tokens while preserving the "what
//! happened" trail.
//!
//! このモジュールは pure な「候補抽出」と「適用」だけを提供する。
//! `min_savings` 判定や savings 推定はこの crate には置かず、上位層
//! （`pod::prune_hook` など）が usage 履歴ベースのトークン会計と組み合わせて行う。

use serde::{Deserialize, Serialize};

use crate::llm_client::types::Item;

/// Configuration for the Prune algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneConfig {
    /// Number of recent turns to protect from pruning.
    /// A "turn" starts at each user message.
    #[serde(default = "default_protected_turns")]
    pub protected_turns: usize,

    /// Minimum token savings required to actually prune. If the prunable
    /// content is smaller than this, the caller should skip to avoid
    /// pointless KV-cache invalidation. The unit is tokens; the caller
    /// is responsible for measuring savings via a usage-history-aware
    /// estimator and comparing against this threshold.
    #[serde(default = "default_min_savings")]
    pub min_savings: u64,
}

fn default_protected_turns() -> usize {
    3
}
fn default_min_savings() -> u64 {
    4096
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            protected_turns: default_protected_turns(),
            min_savings: default_min_savings(),
        }
    }
}

/// Result of [`apply_prune`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneResult {
    /// Number of items whose `content` was set to `None`.
    pub pruned_count: usize,
}

/// Find indices where each "turn" begins.
///
/// A turn starts at every user message. Returns the indices of those
/// user messages in ascending order.
fn find_turn_starts(items: &[Item]) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.is_user_message())
        .map(|(i, _)| i)
        .collect()
}

/// Indices of `Item::ToolResult { content: Some(_), .. }` that lie outside
/// the last `protected_turns` turns. Pure: does not mutate `items`.
///
/// Returns an empty vector when there are too few turns or no prunable
/// candidates.
pub fn prunable_indices(items: &[Item], protected_turns: usize) -> Vec<usize> {
    let turn_starts = find_turn_starts(items);
    if turn_starts.len() <= protected_turns {
        return Vec::new();
    }
    let boundary = turn_starts[turn_starts.len() - protected_turns];
    items[..boundary]
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::ToolResult {
                content: Some(_), ..
            } => Some(i),
            _ => None,
        })
        .collect()
}

/// Set `content = None` on each item at `indices`. Returns the number
/// of items that were actually modified (already-pruned items are
/// counted as 0).
pub fn apply_prune(items: &mut [Item], indices: &[usize]) -> PruneResult {
    let mut count = 0;
    for &i in indices {
        if let Item::ToolResult { content, .. } = &mut items[i] {
            if content.is_some() {
                *content = None;
                count += 1;
            }
        }
    }
    PruneResult {
        pruned_count: count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a history with interleaved user messages and tool results.
    fn make_history(turns: &[(&str, Vec<(&str, Option<&str>)>)]) -> Vec<Item> {
        let mut items = Vec::new();
        for (user_msg, tool_results) in turns {
            items.push(Item::user_message(*user_msg));
            items.push(Item::assistant_message("ok"));
            for (i, (summary, content)) in tool_results.iter().enumerate() {
                let call_id = format!("call_{}", items.len() + i);
                items.push(Item::tool_call(&call_id, "some_tool", "{}"));
                match content {
                    Some(c) => items.push(Item::tool_result_with_content(&call_id, *summary, *c)),
                    None => items.push(Item::tool_result(&call_id, *summary)),
                }
            }
        }
        items
    }

    #[test]
    fn no_candidates_when_too_few_turns() {
        let items = make_history(&[
            ("turn1", vec![("summary1", Some("big content here"))]),
            ("turn2", vec![("summary2", Some("more content"))]),
        ]);
        assert!(prunable_indices(&items, 3).is_empty());
    }

    #[test]
    fn candidates_in_unprotected_turns() {
        let big = "x".repeat(4096 * 4);
        let items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep me"))]),
            ("turn4", vec![("s4", Some("keep me too"))]),
        ]);
        let candidates = prunable_indices(&items, 2);
        assert_eq!(candidates.len(), 2);
        // 候補は turn1 と turn2 の ToolResult のみ
        for &i in &candidates {
            if let Item::ToolResult { summary, .. } = &items[i] {
                assert!(summary == "s1" || summary == "s2");
            } else {
                panic!("non tool-result selected");
            }
        }
    }

    #[test]
    fn apply_drops_content_only() {
        let big = "x".repeat(64);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep me"))]),
            ("turn4", vec![("s4", Some("keep me too"))]),
        ]);
        let candidates = prunable_indices(&items, 2);
        let result = apply_prune(&mut items, &candidates);
        assert_eq!(result.pruned_count, 2);

        for item in &items {
            if let Item::ToolResult {
                summary, content, ..
            } = item
            {
                if summary == "s1" || summary == "s2" {
                    assert!(content.is_none(), "old content should be pruned");
                } else {
                    assert!(content.is_some(), "protected content should remain");
                }
            }
        }
    }

    #[test]
    fn apply_is_idempotent() {
        let big = "x".repeat(64);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        let first_indices = prunable_indices(&items, 2);
        assert_eq!(apply_prune(&mut items, &first_indices).pruned_count, 1);

        // 2 周目: 候補は (まだ) いるかもしれないが、すでに content=None なので
        // apply_prune は 0 件と数える。
        let second_indices = prunable_indices(&items, 2);
        assert!(second_indices.is_empty());
    }

    #[test]
    fn already_pruned_items_excluded_from_candidates() {
        let items = make_history(&[
            ("turn1", vec![("s1", None)]), // already pruned (content=None)
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        assert!(prunable_indices(&items, 2).is_empty());
    }

    #[test]
    fn protected_turns_boundary_exact() {
        // 3 turns with protected_turns=2: only turn 1 is a candidate.
        let big = "x".repeat(64);
        let items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some("protected"))]),
            ("turn3", vec![("s3", Some("also protected"))]),
        ]);
        let candidates = prunable_indices(&items, 2);
        assert_eq!(candidates.len(), 1);
        if let Item::ToolResult { summary, .. } = &items[candidates[0]] {
            assert_eq!(summary, "s1");
        } else {
            panic!("expected ToolResult at candidate index");
        }
    }
}
