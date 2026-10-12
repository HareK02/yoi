//! Request-only omission of details from old tool results.
//!
//! [`prunable_indices`] selects candidates while protecting a token budget at the
//! history tail; [`project`] removes detailed text and attachments. Summaries remain
//! visible to the model. Projection operates on a request-history clone, leaving
//! stored history and annotations unchanged.
//!
//! This module does not own token measurements. The host supplies prefix and savings
//! estimates; unavailable estimates or invalid boundaries produce no candidates.
//! The Engine also checks savings to avoid invalidating the cache for little benefit.
//!
//! Compaction, which stores a summary as a new segment, is a separate responsibility.
//! Omitting already-consumed images likewise affects only the request projection,
//! never the stored results.

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
/// Estimate the savings from removing details, not from dropping entire items.
/// Items and their summaries remain, so this callback must match the actual projection.
pub type SavingsEstimator = Box<dyn Fn(&[Item], &[usize]) -> u64 + Send + Sync>;

/// Result of one prune evaluation pass, surfaced to the optional
/// [`PruneObserver`] for instrumentation.
///
/// The Engine evaluates pruning once per LLM request and reports this value if
/// an observer is registered. It includes the decision and its inputs: candidate
/// count, estimated savings, and the start index of the protected suffix.
#[derive(Debug, Clone)]
pub struct PruneEvaluation {
    /// Length of `prunable_indices`; zero for `SkippedNoCandidates`.
    pub candidate_count: usize,
    /// Estimated token savings; zero when there are no candidates.
    pub estimated_savings: u64,
    /// First item in the suffix protected by the token budget.
    /// `None` when `NoData` estimates prevent determining the boundary.
    pub protected_start_index: Option<usize>,
    /// Evaluation outcome.
    pub decision: PruneDecision,
}

/// Outcome of one prune evaluation. Each variant is one branch of the
/// "fire vs skip" decision tree the Engine walks before each LLM request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PruneDecision {
    /// No projection: `prunable_indices` is empty.
    SkippedNoCandidates,
    /// No projection: candidates exist, but estimated savings are below `min_savings`.
    SkippedBelowMinSavings,
    /// Applied projection because candidates exist and savings meet `min_savings`.
    /// `pruned_count` counts items actually changed by `project()`;
    /// candidates that already lack details contribute zero.
    Fired { pruned_count: usize },
}

/// Optional observer invoked after each prune evaluation, regardless of
/// branch. Callers can install this to publish metrics or traces.
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

/// Remove detailed text and attachments from each `Item::ToolResult` at the given indices.
///
/// The mandatory summary remains. Returns the number of items that were actually
/// modified — results that already contain no detail are counted as 0. Intended
/// for use on a request-context clone (never on a persistent history).
pub fn project(items: &mut [Item], indices: &[usize]) -> usize {
    let mut count = 0;
    for &i in indices {
        if let Item::ToolResult {
            content,
            attachments,
            ..
        } = &mut items[i]
            && (content.is_some() || !attachments.is_empty())
        {
            *content = None;
            attachments.clear();
            count += 1;
        }
    }
    count
}

/// Remove binary attachments after the first model request that could consume them.
///
/// Attachments on tool results following the latest model output are fresh outputs for the
/// immediate follow-up request and remain visible. With early dispatch, later
/// text/calls in the same response have not consumed its tool results: use the
/// confirmed response boundary, not that response's last individual item.
/// Subsequent requests keep the durable tool summary and can call the originating
/// tool again when the image is still needed. Only the request-context clone is modified.
pub fn project_consumed_attachments(items: &mut [Item]) -> usize {
    let latest_model_output = crate::image_recovery::accepted_input_end(items.iter());
    let mut count = 0;
    for (index, item) in items.iter_mut().enumerate() {
        let is_fresh = latest_model_output.is_some_and(|boundary| index > boundary);
        if is_fresh {
            continue;
        }
        if let Item::ToolResult { attachments, .. } = item
            && !attachments.is_empty()
        {
            attachments.clear();
            count += 1;
        }
    }
    count
}

/// Indices of detailed `Item::ToolResult` values that lie before
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
                content,
                attachments,
                ..
            } if content.is_some() || !attachments.is_empty() => Some(i),
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
    fn project_drops_image_detail_but_keeps_summary_and_persistent_source() {
        let original = vec![Item::tool_result_item_with_attachments(
            "call_image",
            "Attached image/png image (12 bytes)",
            None,
            false,
            vec![crate::tool::Attachment::Image(
                crate::tool::ImageAttachment::new("image/png", b"image-body".to_vec()),
            )],
        )];
        let mut request_context = original.clone();

        let estimates = uniform_estimates(&original, 100);
        assert_eq!(prunable_indices(&original, 0, &estimates), vec![0]);

        assert_eq!(project(&mut request_context, &[0]), 1);
        assert!(matches!(
            &request_context[0],
            Item::ToolResult {
                summary,
                content: None,
                attachments,
                ..
            } if summary == "Attached image/png image (12 bytes)" && attachments.is_empty()
        ));
        assert!(matches!(
            &original[0],
            Item::ToolResult { attachments, .. } if attachments.len() == 1
        ));
    }

    fn image_result(call_id: &str, bytes: &[u8]) -> Item {
        Item::tool_result_item_with_attachments(
            call_id,
            "Attached image.png",
            None,
            false,
            vec![crate::tool::Attachment::Image(
                crate::tool::ImageAttachment::new("image/png", bytes.to_vec()),
            )],
        )
    }

    #[test]
    fn consumed_attachment_projection_keeps_only_fresh_tool_outputs() {
        let mut immediate = vec![
            Item::tool_call("image-1", "ViewImage", r#"{"path":"first.png"}"#),
            image_result("image-1", b"first"),
        ];
        assert_eq!(project_consumed_attachments(&mut immediate), 0);
        assert!(matches!(
            &immediate[1],
            Item::ToolResult { attachments, .. } if attachments.len() == 1
        ));

        let mut consumed = immediate.clone();
        consumed.push(Item::assistant_message("I inspected the image."));
        consumed.push(Item::user_message("continue"));
        assert_eq!(project_consumed_attachments(&mut consumed), 1);
        assert!(matches!(
            &consumed[1],
            Item::ToolResult { attachments, .. } if attachments.is_empty()
        ));

        consumed.push(Item::tool_call(
            "image-2",
            "ViewImage",
            r#"{"path":"second.png"}"#,
        ));
        consumed.push(image_result("image-2", b"second"));
        assert_eq!(project_consumed_attachments(&mut consumed), 0);
        assert!(matches!(
            consumed.last(),
            Some(Item::ToolResult { attachments, .. }) if attachments.len() == 1
        ));
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
        // Second pass: reusing the same prunable_indices result changes zero items.
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
