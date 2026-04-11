//! Conditional Prune algorithm for context window management.
//!
//! Removes `content` from old [`Item::ToolResult`] entries, leaving only
//! their `summary`. This reclaims tokens while preserving the "what
//! happened" trail.
//!
//! Pruning is **conditional**: it only fires when the estimated token
//! savings exceed [`PruneConfig::min_savings`], avoiding unnecessary
//! KV-cache invalidation.

use serde::{Deserialize, Serialize};

use crate::llm_client::types::Item;

/// Configuration for the Prune algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneConfig {
    /// Number of recent turns to protect from pruning.
    /// A "turn" starts at each user message.
    #[serde(default = "default_protected_turns")]
    pub protected_turns: usize,

    /// Minimum estimated token savings required to actually prune.
    /// If the prunable content is smaller than this, we skip to
    /// avoid pointless KV-cache invalidation.
    #[serde(default = "default_min_savings")]
    pub min_savings: usize,
}

fn default_protected_turns() -> usize {
    3
}
fn default_min_savings() -> usize {
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

/// Result of a prune operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneResult {
    /// Number of items whose `content` was set to `None`.
    pub pruned_count: usize,
    /// Estimated tokens reclaimed.
    pub estimated_savings: usize,
}

/// Estimate the token count of a string (rough: chars / 4).
fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
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

/// Conditionally prune old tool-result content from `items`.
///
/// Returns `None` if pruning was skipped (not enough savings or not
/// enough turns). Returns `Some(PruneResult)` if items were modified.
///
/// # Algorithm
///
/// 1. Identify turn boundaries (user-message positions).
/// 2. Compute the protection boundary: items before the last
///    `protected_turns` turns are candidates.
/// 3. Sum the estimated token savings from prunable `content` fields.
/// 4. If savings < `min_savings`, skip.
/// 5. Otherwise, set `content = None` on each candidate.
pub fn prune(items: &mut [Item], config: &PruneConfig) -> Option<PruneResult> {
    let turn_starts = find_turn_starts(items);

    // Not enough turns to have anything outside the protected window.
    if turn_starts.len() <= config.protected_turns {
        return None;
    }

    // Everything before this index is a prune candidate.
    let boundary = turn_starts[turn_starts.len() - config.protected_turns];

    // Collect prunable indices and total savings.
    let mut total_savings: usize = 0;
    let mut prunable: Vec<usize> = Vec::new();

    for (i, item) in items[..boundary].iter().enumerate() {
        if let Item::ToolResult {
            content: Some(c), ..
        } = item
        {
            total_savings += estimate_tokens(c);
            prunable.push(i);
        }
    }

    if prunable.is_empty() || total_savings < config.min_savings {
        return None;
    }

    // Apply: drop content, keep summary.
    for &i in &prunable {
        if let Item::ToolResult { content, .. } = &mut items[i] {
            *content = None;
        }
    }

    Some(PruneResult {
        pruned_count: prunable.len(),
        estimated_savings: total_savings,
    })
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
    fn no_prune_when_too_few_turns() {
        let mut items = make_history(&[
            ("turn1", vec![("summary1", Some("big content here"))]),
            ("turn2", vec![("summary2", Some("more content"))]),
        ]);
        let config = PruneConfig {
            protected_turns: 3,
            min_savings: 0,
        };
        assert!(prune(&mut items, &config).is_none());
    }

    #[test]
    fn no_prune_when_savings_below_threshold() {
        let mut items = make_history(&[
            ("turn1", vec![("s", Some("tiny"))]), // ~1 token
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        let config = PruneConfig {
            protected_turns: 2,
            min_savings: 9999,
        };
        assert!(prune(&mut items, &config).is_none());
    }

    #[test]
    fn prune_old_content() {
        // 4 turns. protected_turns=2 → turns 1-2 are candidates.
        let big = "x".repeat(4096 * 4); // ~4096 tokens
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep me"))]),
            ("turn4", vec![("s4", Some("keep me too"))]),
        ]);
        let config = PruneConfig {
            protected_turns: 2,
            min_savings: 1000,
        };

        let result = prune(&mut items, &config).expect("should prune");
        assert_eq!(result.pruned_count, 2);
        assert!(result.estimated_savings >= 8000);

        // Verify: pruned items have content=None, protected items keep content.
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
    fn idempotent() {
        let big = "x".repeat(4096 * 4);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        let config = PruneConfig {
            protected_turns: 2,
            min_savings: 100,
        };

        let first = prune(&mut items, &config).expect("first prune");
        assert_eq!(first.pruned_count, 1);

        // Second call: nothing left to prune.
        assert!(prune(&mut items, &config).is_none());
    }

    #[test]
    fn already_pruned_items_skipped() {
        // Items that already have content=None are not counted as savings.
        let mut items = make_history(&[
            ("turn1", vec![("s1", None)]), // already pruned
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        let config = PruneConfig {
            protected_turns: 2,
            min_savings: 0, // Even with threshold 0, no savings means no prune
        };

        assert!(prune(&mut items, &config).is_none());
    }

    #[test]
    fn protected_turns_boundary_exact() {
        // 3 turns with protected_turns=2:
        // Turn 1 content should be pruned, turns 2-3 protected.
        let big = "x".repeat(4096 * 4);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some("protected"))]),
            ("turn3", vec![("s3", Some("also protected"))]),
        ]);
        let config = PruneConfig {
            protected_turns: 2,
            min_savings: 100,
        };

        let result = prune(&mut items, &config).expect("should prune turn1");
        assert_eq!(result.pruned_count, 1);

        // Verify s1 pruned, s2 and s3 intact.
        for item in &items {
            if let Item::ToolResult {
                summary, content, ..
            } = item
            {
                match summary.as_str() {
                    "s1" => assert!(content.is_none()),
                    "s2" | "s3" => assert!(content.is_some()),
                    _ => {}
                }
            }
        }
    }
}
