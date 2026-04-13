//! Prune — context projection for old tool-result content.
//!
//! LLM 送信時のコンテキストから古い [`Item::ToolResult`] の `content` を
//! 省略して、コンテキスト窓のトークンを回収する。`summary` は残すので
//! 「何が起きたか」の痕跡は保たれる。
//!
//! # 設計方針
//!
//! Prune は **コンテキスト射影** であり、history の変換ではない。
//! この crate が提供するのは pure な候補抽出 [`prunable_indices`] のみで、
//! 射影の適用は上位層（`pod::prune_hook` 等）が LLM に送る一時コンテキスト
//! に対してだけ行う。Worker の永続履歴は決して変更されない。
//!
//! `min_savings` 判定や savings 推定もこの crate には置かず、上位層が
//! usage 履歴ベースのトークン会計と組み合わせて行う。

use serde::{Deserialize, Serialize};

use crate::llm_client::types::Item;

/// Callback that estimates the token savings for projecting the
/// `ToolResult.content` out of `history[i]` for each `i` in `indices`.
///
/// Injected into [`crate::Worker`] via `set_savings_estimator` so the
/// Worker can make `min_savings` decisions without knowing about usage
/// measurement sources. Return `0` to signal "no data / refuse to prune".
///
/// 推定対象は「drop する範囲全体」ではなく「content を None にする差分」
/// であることに注意。item 自体（summary 等）は残るので、この callback は
/// 実際の projection と一致する savings を返す必要がある。
pub type SavingsEstimator = Box<dyn Fn(&[Item], &[usize]) -> u64 + Send + Sync>;

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

/// Set `content = None` on each `Item::ToolResult` at the given indices.
///
/// Returns the number of items that were actually modified — items that
/// are already content-less are counted as 0. Intended for use on a
/// request-context clone (never on a persistent history).
pub fn project(items: &mut [Item], indices: &[usize]) -> usize {
    let mut count = 0;
    for &i in indices {
        if let Item::ToolResult { content, .. } = &mut items[i] {
            if content.is_some() {
                *content = None;
                count += 1;
            }
        }
    }
    count
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
    fn project_drops_content_and_counts_modifications() {
        let big = "x".repeat(64);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep me"))]),
            ("turn4", vec![("s4", Some("keep me too"))]),
        ]);
        let candidates = prunable_indices(&items, 2);
        let count = project(&mut items, &candidates);
        assert_eq!(count, 2);

        for item in &items {
            if let Item::ToolResult {
                summary, content, ..
            } = item
            {
                if summary == "s1" || summary == "s2" {
                    assert!(content.is_none(), "old content should be projected out");
                } else {
                    assert!(content.is_some(), "protected content should remain");
                }
            }
        }
    }

    #[test]
    fn project_skips_already_pruned_items() {
        // indices points at an item whose content is already None.
        // project() should count it as 0 modifications.
        let mut items = make_history(&[
            ("turn1", vec![("s1", None)]),
            ("turn2", vec![("s2", Some("hello"))]),
        ]);
        // Manually target s1 (index 3) even though it's already None.
        let target = items
            .iter()
            .position(|it| matches!(it, Item::ToolResult { summary, .. } if summary == "s1"))
            .unwrap();
        let count = project(&mut items, &[target]);
        assert_eq!(count, 0);
    }

    #[test]
    fn project_is_idempotent() {
        let big = "x".repeat(64);
        let mut items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![]),
            ("turn3", vec![]),
            ("turn4", vec![]),
        ]);
        let candidates = prunable_indices(&items, 2);
        assert_eq!(project(&mut items, &candidates), 1);
        // 2 周目: 候補は一度の prunable_indices 結果を使い回しても 0 件。
        assert_eq!(project(&mut items, &candidates), 0);
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
