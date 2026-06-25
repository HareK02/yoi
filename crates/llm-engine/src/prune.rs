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
//! 射影の適用は上位層（`worker::prune_hook` 等）が LLM に送る一時コンテキスト
//! に対してだけ行う。Engine の永続履歴は決して変更されない。
//!
//! 保護境界は末尾 token budget で決めるが、この crate は usage 履歴を
//! 所有しない。prefix ごとの token 推定値と savings 推定は上位層から
//! callback で注入される。

use serde::{Deserialize, Serialize};

use crate::llm_client::types::Item;
use crate::token_counter::{EstimateSource, TokenEstimate};

/// Callback that returns token estimates for every prefix boundary of the
/// supplied request history.
///
/// The returned slice must have `history.len() + 1` entries where entry `i`
/// estimates the token count of `history[..i]`. Returning a malformed vector,
/// or estimates whose source is [`EstimateSource::NoData`], makes prune treat
/// the request as having no candidates.
pub type TokenEstimator = Box<dyn Fn(&[Item]) -> Vec<TokenEstimate> + Send + Sync>;

/// Callback that estimates the token savings for projecting the
/// `ToolResult.content` out of `history[i]` for each `i` in `indices`.
///
/// Injected into [`crate::Engine`] via `set_savings_estimator` so the
/// Engine can make `min_savings` decisions without knowing about usage
/// measurement sources. Return `0` to signal "no data / refuse to prune".
///
/// 推定対象は「drop する範囲全体」ではなく「content を None にする差分」
/// であることに注意。item 自体（summary 等）は残るので、この callback は
/// 実際の projection と一致する savings を返す必要がある。
pub type SavingsEstimator = Box<dyn Fn(&[Item], &[usize]) -> u64 + Send + Sync>;

/// Result of one prune evaluation pass, surfaced to the optional
/// [`PruneObserver`] for instrumentation.
///
/// Engine は LLM リクエストごとに 1 回 prune の評価をし、その結果を
/// （observer が登録されていれば）この値で通知する。fire/skip の判定
/// 結果と、判定材料になった候補数 / 推定 savings / 保護領域の先頭 index を持つ。
#[derive(Debug, Clone)]
pub struct PruneEvaluation {
    /// `prunable_indices` の長さ。`Skipped::NoCandidates` の時は 0。
    pub candidate_count: usize,
    /// 推定された savings (tokens)。`NoCandidates` の時は 0。
    pub estimated_savings: u64,
    /// Token budget で保護される suffix の先頭 item index。
    /// usage 推定が `NoData` で境界が決まらない場合は `None`。
    pub protected_start_index: Option<usize>,
    /// 判定結果。
    pub decision: PruneDecision,
}

/// Outcome of one prune evaluation. Each variant is one branch of the
/// "fire vs skip" decision tree the Engine walks before each LLM request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneDecision {
    /// `prunable_indices` が空 → 何もしない。
    SkippedNoCandidates,
    /// 候補はあったが推定 savings が `min_savings` 未満 → 何もしない。
    SkippedBelowMinSavings,
    /// 候補があり savings >= min_savings → projection を適用した。
    /// `pruned_count` は `project()` が実際に書き換えた item 数
    /// （既に content=None だった候補は 0 計上）。
    Fired { pruned_count: usize },
}

/// Optional observer invoked after each prune evaluation, regardless of
/// branch. Worker 等の上位層が install して metrics を発行する。
pub type PruneObserver = Box<dyn Fn(&PruneEvaluation) + Send + Sync>;

/// Configuration for the Prune algorithm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneConfig {
    /// Token budget at the history tail protected from pruning.
    #[serde(default = "default_protected_tokens")]
    pub protected_tokens: u64,

    /// Minimum token savings required to actually prune. If the prunable
    /// content is smaller than this, the caller should skip to avoid
    /// pointless KV-cache invalidation. The unit is tokens; the caller
    /// is responsible for measuring savings via a usage-history-aware
    /// estimator and comparing against this threshold.
    #[serde(default = "default_min_savings")]
    pub min_savings: u64,
}

fn default_protected_tokens() -> u64 {
    8000
}
fn default_min_savings() -> u64 {
    4096
}

impl Default for PruneConfig {
    fn default() -> Self {
        Self {
            protected_tokens: default_protected_tokens(),
            min_savings: default_min_savings(),
        }
    }
}

/// Set `content = None` on each `Item::ToolResult` at the given indices.
///
/// Returns the number of items that were actually modified — items that
/// are already content-less are counted as 0. Intended for use on a
/// request-context clone (never on a persistent history).
pub fn project(items: &mut [Item], indices: &[usize]) -> usize {
    let mut count = 0;
    for &i in indices {
        if let Item::ToolResult { content, .. } = &mut items[i]
            && content.is_some()
        {
            *content = None;
            count += 1;
        }
    }
    count
}

/// Indices of `Item::ToolResult { content: Some(_), .. }` that lie before
/// the suffix protected by `protected_tokens`. Pure: does not mutate `items`.
///
/// Returns an empty vector when token estimates are unavailable (`NoData`) or
/// no prunable candidates exist.
pub fn prunable_indices(
    items: &[Item],
    protected_tokens: u64,
    token_estimates: &[TokenEstimate],
) -> Vec<usize> {
    evaluate_candidates(items, protected_tokens, token_estimates).0
}

/// Same as [`prunable_indices`] but also returns the start index of the
/// protected suffix. `None` means the token boundary could not be determined
/// (currently because usage estimates were `NoData` or malformed).
pub fn evaluate_candidates(
    items: &[Item],
    protected_tokens: u64,
    token_estimates: &[TokenEstimate],
) -> (Vec<usize>, Option<usize>) {
    let Some(protected_start) = protected_start_index(items, protected_tokens, token_estimates)
    else {
        return (Vec::new(), None);
    };

    let candidates = items[..protected_start]
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::ToolResult {
                content: Some(_), ..
            } => Some(i),
            _ => None,
        })
        .collect();
    (candidates, Some(protected_start))
}

fn protected_start_index(
    items: &[Item],
    protected_tokens: u64,
    token_estimates: &[TokenEstimate],
) -> Option<usize> {
    if token_estimates.len() != items.len() + 1 {
        return None;
    }
    let total = token_estimates[items.len()];
    if total.source == EstimateSource::NoData {
        return None;
    }
    if protected_tokens == 0 {
        return Some(items.len());
    }

    let mut protected_start = items.len();
    for idx in (0..items.len()).rev() {
        let prefix = token_estimates[idx];
        if prefix.source == EstimateSource::NoData {
            return None;
        }
        protected_start = idx;
        let tail_tokens = total.tokens.saturating_sub(prefix.tokens);
        if tail_tokens >= protected_tokens {
            break;
        }
    }
    Some(protected_start)
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

    fn measured_prefix(tokens: &[u64]) -> Vec<TokenEstimate> {
        tokens
            .iter()
            .copied()
            .map(|tokens| TokenEstimate {
                tokens,
                source: EstimateSource::Measured,
            })
            .collect()
    }

    fn uniform_estimates(items: &[Item], item_tokens: u64) -> Vec<TokenEstimate> {
        let mut tokens = Vec::with_capacity(items.len() + 1);
        for i in 0..=items.len() {
            tokens.push(i as u64 * item_tokens);
        }
        measured_prefix(&tokens)
    }

    fn estimates_from_item_tokens(item_tokens: &[u64]) -> Vec<TokenEstimate> {
        let mut prefix = Vec::with_capacity(item_tokens.len() + 1);
        let mut acc = 0;
        prefix.push(acc);
        for tokens in item_tokens {
            acc += tokens;
            prefix.push(acc);
        }
        measured_prefix(&prefix)
    }

    fn no_data_estimates(items: &[Item]) -> Vec<TokenEstimate> {
        (0..=items.len())
            .map(|i| TokenEstimate {
                tokens: i as u64,
                source: if i == 0 {
                    EstimateSource::Measured
                } else {
                    EstimateSource::NoData
                },
            })
            .collect()
    }

    #[test]
    fn no_candidates_when_estimate_has_no_data() {
        let items = make_history(&[("turn1", vec![("summary1", Some("big content here"))])]);
        let estimates = no_data_estimates(&items);
        let (candidates, protected_start) = evaluate_candidates(&items, 10, &estimates);
        assert!(candidates.is_empty());
        assert_eq!(protected_start, None);
    }

    #[test]
    fn no_candidates_when_history_fits_in_protected_tokens() {
        let items = make_history(&[
            ("turn1", vec![("summary1", Some("big content here"))]),
            ("turn2", vec![("summary2", Some("more content"))]),
        ]);
        let estimates = uniform_estimates(&items, 10);
        assert!(prunable_indices(&items, 10_000, &estimates).is_empty());
    }

    #[test]
    fn candidates_before_token_protected_suffix() {
        let big = "x".repeat(4096 * 4);
        let items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep me"))]),
            ("turn4", vec![("s4", Some("keep me too"))]),
        ]);
        let estimates = uniform_estimates(&items, 10);
        let candidates = prunable_indices(&items, 80, &estimates);
        assert_eq!(candidates.len(), 2);
        // suffix budget 80 tokens protects turn3+turn4 (8 items), so only s1/s2 are candidates.
        for &i in &candidates {
            if let Item::ToolResult { summary, .. } = &items[i] {
                assert!(summary == "s1" || summary == "s2");
            } else {
                panic!("non tool-result selected");
            }
        }
    }

    #[test]
    fn single_long_task_gets_candidates_without_multiple_user_turns() {
        let big = "x".repeat(4096 * 8);
        let items = make_history(&[(
            "one long task",
            vec![
                ("s1", Some(&big)),
                ("s2", Some(&big)),
                ("s3", Some(&big)),
                ("s4", Some(&big)),
            ],
        )]);
        // user + assistant are cheap; every ToolCall is cheap; every ToolResult is heavy.
        let item_tokens = vec![1, 1, 1, 5_000, 1, 5_000, 1, 5_000, 1, 5_000];
        let estimates = estimates_from_item_tokens(&item_tokens);

        let (candidates, protected_start) = evaluate_candidates(&items, 8_000, &estimates);

        assert_eq!(protected_start, Some(7));
        assert_eq!(candidates.len(), 2);
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
        let estimates = uniform_estimates(&items, 10);
        assert!(prunable_indices(&items, 20, &estimates).is_empty());
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
        let estimates = uniform_estimates(&items, 10);
        let candidates = prunable_indices(&items, 80, &estimates);
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
        // Manually target s1 even though it's already None.
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
        let estimates = uniform_estimates(&items, 10);
        let candidates = prunable_indices(&items, 20, &estimates);
        assert_eq!(project(&mut items, &candidates), 1);
        // 2 周目: 候補は一度の prunable_indices 結果を使い回しても 0 件。
        assert_eq!(project(&mut items, &candidates), 0);
    }

    #[test]
    fn evaluate_candidates_returns_protected_start_index() {
        let big = "x".repeat(64);
        let items = make_history(&[
            ("turn1", vec![("s1", Some(&big))]),
            ("turn2", vec![("s2", Some(&big))]),
            ("turn3", vec![("s3", Some("keep"))]),
            ("turn4", vec![("s4", Some("keep too"))]),
        ]);
        let estimates = uniform_estimates(&items, 10);
        let (candidates, protected_start) = evaluate_candidates(&items, 80, &estimates);
        assert_eq!(candidates.len(), 2);
        // protected_tokens=80 → protected suffix is turn3+turn4, starting at index 8.
        assert_eq!(protected_start, Some(8));
    }

    #[test]
    fn evaluate_candidates_reports_zero_start_when_everything_is_protected() {
        let items = make_history(&[("only", vec![("s", Some("x"))])]);
        let estimates = uniform_estimates(&items, 10);
        let (candidates, protected_start) = evaluate_candidates(&items, 10_000, &estimates);
        assert!(candidates.is_empty());
        assert_eq!(protected_start, Some(0));
    }

    #[test]
    fn zero_protected_tokens_allows_all_tool_results_as_candidates() {
        let big = "x".repeat(64);
        let items = make_history(&[("turn1", vec![("s1", Some(&big)), ("s2", Some(&big))])]);
        let estimates = uniform_estimates(&items, 10);
        let (candidates, protected_start) = evaluate_candidates(&items, 0, &estimates);
        assert_eq!(protected_start, Some(items.len()));
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn malformed_estimate_vector_is_treated_as_no_boundary() {
        let items = make_history(&[("turn1", vec![("s1", Some("x"))])]);
        let (candidates, protected_start) = evaluate_candidates(&items, 10, &[]);
        assert!(candidates.is_empty());
        assert_eq!(protected_start, None);
    }
}
