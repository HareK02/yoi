//! Token-accounting helpers specific to compact / prune.
//!
//! Generic functions (`prefix_bytes`, `tokens_at`, `total_tokens`, `total_tokens_at`) live in
//! [`agen::token_counter`] and compute pure estimates from `UsageRecord` sequences and current
//! history. This module is limited to compact / prune logic (`split_for_retained`,
//! `savings_for_prune`) and public Worker APIs.
//!
//! # Policy
//!
//! - No local tokenizer. Use measurements when available, interpolate between them by byte count,
//!   and extrapolate beyond the latest measurement at byte/4.
//! - Compact's retained split does not treat request-time pruned/projected `UsageRecord`s as a
//!   monotonic series of persisted history prefixes. It distributes current prompt-occupancy
//!   estimates over raw serialized bytes and chooses the cut by persisted tail size.
//! - [`EstimateSource`] exposes estimate provenance to callers. Accuracy is sufficient for
//!   compact / prune thresholds, not billing decisions.

use agen::llm_client::client::LlmClient;
use agen::token_counter::{item_bytes, prefix_bytes, tokens_at};
use agen::{Item, UsageRecord};
use session_store::Store;

pub use agen::token_counter::{EstimateSource, TokenEstimate};

use crate::Worker;

/// Position at which to split history.
///
/// `items[..index]` is discarded/summarized; `items[index..]` is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitPoint {
    pub index: usize,
    pub source: EstimateSource,
}

fn split_for_retained_impl(history: &[Item], records: &[UsageRecord], retained: u64) -> SplitPoint {
    let prefix = prefix_bytes(history);
    let current = tokens_at(history, records, history.len(), &prefix);
    if current.tokens <= retained {
        return SplitPoint {
            index: 0,
            source: current.source,
        };
    }

    let cut_index = split_index_by_retained_bytes(&prefix, current.tokens, retained);
    SplitPoint {
        index: balance_to_pair_boundary(history, cut_index),
        source: current.source,
    }
}

fn split_index_by_retained_bytes(prefix: &[u64], total_tokens: u64, retained_tokens: u64) -> usize {
    debug_assert!(!prefix.is_empty());

    let len = prefix.len() - 1;
    if len == 0 {
        return 0;
    }
    if retained_tokens == 0 {
        return len;
    }

    let total_bytes = *prefix.last().unwrap_or(&0);
    if total_bytes == 0 || total_tokens == 0 {
        return 0;
    }

    let raw_fallback_tokens = ceil_div_u128(total_bytes as u128, 4) as u64;
    let rate_tokens = total_tokens.max(raw_fallback_tokens);
    let target_retained_bytes = ceil_div_u128(
        retained_tokens as u128 * total_bytes as u128,
        rate_tokens as u128,
    )
    .min(total_bytes as u128) as u64;

    // Drop as many complete Items as possible while keeping the raw persisted
    // suffix at or above the retained budget. This is monotonic in serialized
    // history size and intentionally does not inspect per-history_len
    // UsageRecords: request-time usage can move up and down after pruning /
    // projection, so it is not a valid prefix series for retained split. The
    // byte/4 fallback is kept as a lower bound for raw persisted size so a
    // heavily-pruned request measurement cannot justify retaining megabytes of
    // history.
    let mut cut = 0;
    for (idx, bytes_before) in prefix.iter().enumerate().take(len + 1) {
        let suffix_bytes = total_bytes.saturating_sub(*bytes_before);
        if suffix_bytes >= target_retained_bytes {
            cut = idx;
        } else {
            break;
        }
    }
    cut
}

fn ceil_div_u128(n: u128, d: u128) -> u128 {
    debug_assert!(d > 0);
    if n == 0 { 0 } else { ((n - 1) / d) + 1 }
}

/// Move `cut` backward so `history[cut..]` respects `ToolCall` / `ToolResult` pair boundaries.
///
/// LLM APIs require each `ToolResult`'s `ToolCall` in the same request. Token-only cuts can fall
/// inside parallel tool calls, leaving an orphan `ToolResult` at the retained head and causing
/// API validation to reject the next session's first request.
///
/// Lower `cut` until every retained `ToolResult`'s matching `ToolCall` is also retained. This may
/// exceed the retained_tokens budget, but is safe: nothing is sent directly to the LLM here;
/// the next `pre_llm_request` reevaluates it.
///
/// Scan history backward, lowering `cut` to each retained `ToolResult`'s matching `ToolCall` position.
/// Lowering the cut retains positions previously on the summary side; the remaining scan cascades
/// backward to the correct boundary. Unique `ToolCall` `call_id`s allow a precomputed map for O(n).
fn balance_to_pair_boundary(history: &[Item], cut: usize) -> usize {
    let mut idx = cut.min(history.len());
    if idx == 0 {
        return 0;
    }

    let call_positions: std::collections::HashMap<&str, usize> = history
        .iter()
        .enumerate()
        .filter_map(|(i, item)| match item {
            Item::ToolCall { call_id, .. } => Some((call_id.as_str(), i)),
            _ => None,
        })
        .collect();

    let mut k = history.len();
    while k > 0 {
        k -= 1;
        if k >= idx {
            if let Item::ToolResult { call_id, .. } = &history[k] {
                if let Some(&call_pos) = call_positions.get(call_id.as_str()) {
                    if call_pos < idx {
                        idx = call_pos;
                    }
                }
            }
        }
    }
    idx
}

/// Serialized bytes saved by projecting one ToolResult's `content` to `None`.
/// Returns 0 for non-ToolResult items or items already having content=None.
fn tool_result_content_bytes(item: &Item) -> u64 {
    if !matches!(
        item,
        Item::ToolResult {
            content: Some(_),
            ..
        }
    ) {
        return 0;
    }
    let mut cleared = item.clone();
    if let Item::ToolResult { content, .. } = &mut cleared {
        *content = None;
    }
    item_bytes(item).saturating_sub(item_bytes(&cleared))
}

/// Prefix-boundary token estimates used by Prune to find its protected suffix.
///
/// Returns `history.len() + 1` entries where entry `i` estimates
/// `history[..i]`. This shares the same [`tokens_at`] accounting as compact's
/// retained-tail split and prune's savings estimate.
pub(crate) fn token_estimates_for_prune_impl(
    history: &[Item],
    records: &[UsageRecord],
) -> Vec<TokenEstimate> {
    let prefix = prefix_bytes(history);
    (0..=history.len())
        .map(|idx| tokens_at(history, records, idx, &prefix))
        .collect()
}

/// Estimate token savings from prune projection (`ToolResult.content = None`).
///
/// `indices` is expected to contain candidates from [`agen::prune::prunable_indices`]. Sum their
/// content-byte differences and convert using the usage-history tokens/byte rate. Unlike
/// `tokens_at`-based calculations, this retains the items themselves (summaries, etc.) rather
/// than dropping the entire range.
pub(crate) fn savings_for_prune_impl(
    history: &[Item],
    records: &[UsageRecord],
    indices: &[usize],
) -> TokenEstimate {
    let removed_bytes: u64 = indices
        .iter()
        .filter_map(|&i| history.get(i))
        .map(tool_result_content_bytes)
        .sum();

    if removed_bytes == 0 {
        return TokenEstimate {
            tokens: 0,
            source: EstimateSource::Measured,
        };
    }

    if records.is_empty() {
        return TokenEstimate {
            tokens: removed_bytes / 4,
            source: EstimateSource::NoData,
        };
    }

    // Convert byte differences using the latest measurement's tokens/byte rate.
    // Only the ratio, not the measurement itself, is used, so the rate remains
    // valid even when history_len differs from record.history_len.
    let prefix = prefix_bytes(history);
    let last = records.last().expect("records non-empty");
    let ref_bytes = prefix[last.history_len.min(history.len())];
    if ref_bytes == 0 || last.input_total_tokens == 0 {
        return TokenEstimate {
            tokens: 0,
            source: EstimateSource::Extrapolated,
        };
    }
    let tokens =
        (removed_bytes as u128 * last.input_total_tokens as u128 / ref_bytes as u128) as u64;
    let source = if last.history_len == history.len() {
        EstimateSource::Measured
    } else {
        EstimateSource::Extrapolated
    };
    TokenEstimate { tokens, source }
}

// ── Public Worker APIs ───────────────────────────────────────────────

impl<C: LlmClient + 'static, St: Store> Worker<C, St> {
    /// Estimated tokens for the entire current history.
    ///
    /// Uses the last measurement plus byte/4 extrapolation for subsequent unmeasured additions.
    pub fn total_tokens(&self) -> TokenEstimate {
        let usage = self.usage_history();
        agen::token_counter::total_tokens(&self.history(), &usage)
    }

    /// Estimate full prompt length at any history index.
    ///
    /// Evaluates the same accounting as `total_tokens()` at an arbitrary position.
    /// Memory extraction triggers use `total_tokens_at(now) - total_tokens_at(pointer)`
    /// to measure prompt growth since the pointer.
    pub fn total_tokens_at(&self, history_len: usize) -> TokenEstimate {
        let usage = self.usage_history();
        agen::token_counter::total_tokens_at(&self.history(), &usage, history_len)
    }

    /// Split position retaining at least `retained` tokens from the tail.
    ///
    /// `history[..cut.index]` is summarized/discarded; `history[cut.index..]` is retained.
    pub fn split_for_retained(&self, retained: u64) -> SplitPoint {
        let usage = self.usage_history();
        split_for_retained_impl(&self.history(), &usage, retained)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> Item {
        Item::user_message(text)
    }

    fn record(history_len: usize, tokens: u64) -> UsageRecord {
        UsageRecord {
            history_len,
            input_total_tokens: tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
        }
    }

    #[test]
    fn split_returns_zero_when_current_below_retained() {
        let history = vec![msg("a"), msg("b")];
        let records = vec![record(2, 50)];
        let cut = split_for_retained_impl(&history, &records, 1000);
        assert_eq!(cut.index, 0);
    }

    #[test]
    fn split_uses_current_occupancy_as_raw_byte_rate() {
        // Compact retained split does not treat the intermediate record at
        // len=2 as a raw prefix boundary. It uses the current occupancy
        // estimate (len=4 → 300) as a serialized-byte rate and keeps the
        // smallest item-granular suffix whose raw size covers retained=200.
        let history = vec![msg("a"), msg("b"), msg("c"), msg("d")];
        let records = vec![record(2, 100), record(4, 300)];
        let cut = split_for_retained_impl(&history, &records, 200);
        assert_eq!(cut.index, 1);
        assert_eq!(cut.source, EstimateSource::Measured);
    }

    #[test]
    fn split_does_not_use_non_current_measurements_as_cut_boundaries() {
        let history = vec![msg("aaaaaa"), msg("bbbbbb"), msg("cccccc"), msg("dddddd")];
        let records = vec![record(1, 50), record(4, 400)];
        let cut = split_for_retained_impl(&history, &records, 250);
        assert_eq!(cut.index, 1);
        assert_eq!(cut.source, EstimateSource::Measured);
    }

    #[test]
    fn split_ignores_non_monotonic_usage_spike_for_retained_tail() {
        let history: Vec<Item> = (0..20)
            .map(|idx| msg(&format!("message-{idx}-{}", "x".repeat(100))))
            .collect();
        let records = vec![
            record(2, 900), // request-time spike after pruning/projection
            record(20, 1000),
        ];
        let cut = split_for_retained_impl(&history, &records, 100);

        // The old prefix-crossing logic picked index 2 because 900 >=
        // 1000-100, retaining almost the whole persisted history. The compact
        // split must instead use raw suffix size and keep only the tail needed
        // for the retained budget.
        assert!(cut.index > 10, "cut.index = {}", cut.index);
        assert_eq!(cut.source, EstimateSource::Measured);
    }

    #[test]
    fn retained_split_does_not_treat_image_base64_as_text_tokens() {
        fn history_with_image(bytes: usize) -> Vec<Item> {
            vec![
                msg(&"older text ".repeat(2_000)),
                Item::tool_call("image", "ViewImage", r#"{"path":"screen.png"}"#),
                Item::tool_result_item_with_attachments(
                    "image",
                    "Attached image screen.png",
                    None,
                    false,
                    vec![agen::tool::Attachment::Image(
                        agen::tool::ImageAttachment::new("image/png", vec![7_u8; bytes]),
                    )],
                ),
            ]
        }

        let small = history_with_image(8);
        let large = history_with_image(1_000_000);

        assert_eq!(
            split_for_retained_impl(&small, &[], 1_000),
            split_for_retained_impl(&large, &[], 1_000)
        );
    }

    #[test]
    fn split_all_when_retained_zero() {
        let history = vec![msg("a"), msg("b")];
        let records = vec![record(2, 100)];
        let cut = split_for_retained_impl(&history, &records, 0);
        assert_eq!(cut.index, 2);
    }

    fn tool_result_with(summary: &str, content: Option<&str>) -> Item {
        match content {
            Some(c) => Item::tool_result_with_content("call", summary, c),
            None => Item::tool_result("call", summary),
        }
    }

    #[test]
    fn token_estimates_for_prune_returns_every_prefix_boundary() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        let estimates = token_estimates_for_prune_impl(&history, &[record(3, 300)]);
        assert_eq!(estimates.len(), history.len() + 1);
        assert_eq!(estimates[0].tokens, 0);
        assert_eq!(estimates[3].tokens, 300);
        assert_eq!(estimates[3].source, EstimateSource::Measured);
    }

    #[test]
    fn token_estimates_for_prune_propagates_no_data() {
        let history = vec![msg("a"), msg("b")];
        let estimates = token_estimates_for_prune_impl(&history, &[]);
        assert_eq!(estimates.len(), history.len() + 1);
        assert_eq!(estimates[0].source, EstimateSource::Measured);
        assert_eq!(estimates[1].source, EstimateSource::NoData);
        assert_eq!(estimates[2].source, EstimateSource::NoData);
    }

    #[test]
    fn savings_for_prune_skips_non_toolresult_indices() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        // indices point at plain messages, not ToolResult → 0 savings.
        let est = savings_for_prune_impl(&history, &[record(3, 300)], &[0, 1, 2]);
        assert_eq!(est.tokens, 0);
    }

    #[test]
    fn savings_for_prune_skips_content_none_items() {
        let history = vec![
            msg("user"),
            tool_result_with("s1", None),
            tool_result_with("s2", None),
        ];
        let est = savings_for_prune_impl(&history, &[record(3, 300)], &[1, 2]);
        assert_eq!(est.tokens, 0);
    }

    #[test]
    fn savings_for_prune_counts_only_content_delta() {
        // 1 item with big content vs the same structure without content.
        let big = "x".repeat(400);
        let history = vec![
            msg("user"),
            tool_result_with("summary", Some(&big)),
            msg("tail"),
        ];
        // 1 record at end so rate = tokens / total_bytes
        let total_bytes: u64 = history.iter().map(item_bytes).sum();
        let records = vec![record(history.len(), total_bytes)]; // rate = 1 tok/byte
        let est = savings_for_prune_impl(&history, &records, &[1]);
        // saved bytes ≈ size of the big content payload; with rate=1 it
        // should be close to 400 and far from the full item bytes.
        let full_item_bytes = item_bytes(&history[1]);
        assert!(est.tokens > 0);
        assert!(est.tokens < full_item_bytes);
        assert!(est.tokens >= 400);
        assert_eq!(est.source, EstimateSource::Measured);
    }

    #[test]
    fn savings_for_prune_no_records_falls_back_to_bytes() {
        let history = vec![msg("u"), tool_result_with("s", Some("hello world"))];
        let est = savings_for_prune_impl(&history, &[], &[1]);
        assert_eq!(est.source, EstimateSource::NoData);
        assert!(est.tokens > 0);
    }

    #[test]
    fn savings_for_prune_extrapolated_when_history_grew_past_measurement() {
        let big = "x".repeat(200);
        let history = vec![
            msg("u1"),
            tool_result_with("s", Some(&big)),
            msg("u2"), // added after the last measurement
        ];
        let records = vec![record(2, 100)];
        let est = savings_for_prune_impl(&history, &records, &[1]);
        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert!(est.tokens > 0);
    }

    #[test]
    fn savings_for_prune_empty_indices_is_zero() {
        let history = vec![msg("a")];
        let est = savings_for_prune_impl(&history, &[record(1, 100)], &[]);
        assert_eq!(est.tokens, 0);
    }

    #[test]
    fn savings_for_prune_ignores_out_of_range_indices() {
        let history = vec![msg("a")];
        let est = savings_for_prune_impl(&history, &[record(1, 100)], &[99]);
        assert_eq!(est.tokens, 0);
    }

    fn tc(call_id: &str) -> Item {
        Item::tool_call(call_id, "Read", "{}")
    }

    fn tr(call_id: &str) -> Item {
        Item::tool_result(call_id, "summary")
    }

    #[test]
    fn balance_noop_on_clean_message_boundary() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        assert_eq!(balance_to_pair_boundary(&history, 2), 2);
        assert_eq!(balance_to_pair_boundary(&history, 0), 0);
        assert_eq!(balance_to_pair_boundary(&history, 3), 3);
    }

    #[test]
    fn balance_retreats_from_inside_parallel_tool_results() {
        // [Msg, TC_a, TC_b, TC_c, TR_a, TR_b, TR_c]
        // cut=5 -> retained=[TR_b, TR_c]. TR_c's TC is at idx=3, TR_b's at idx=2,
        // so retreat to idx=2. This now retains TR_a (idx=4), whose TC_a at idx=1
        // is still outside. Cascading retreat ends at idx=1, retaining
        // [TC_a, TC_b, TC_c, TR_a, TR_b, TR_c].
        let history = vec![
            msg("u"),
            tc("a"),
            tc("b"),
            tc("c"),
            tr("a"),
            tr("b"),
            tr("c"),
        ];
        assert_eq!(balance_to_pair_boundary(&history, 5), 1);
    }

    #[test]
    fn balance_retreats_between_call_and_result() {
        // [TC_a, TR_a, TC_b, TR_b]. cut=3 -> retained=[TR_b], an orphan.
        // TC_b is at idx=2 -> cut=2, retained=[TC_b, TR_b].
        let history = vec![tc("a"), tr("a"), tc("b"), tr("b")];
        assert_eq!(balance_to_pair_boundary(&history, 3), 2);
    }

    #[test]
    fn balance_cascades_through_nested_pairs() {
        // [TC_a, TC_b, TR_b, TR_a, TC_c, TR_c]. cut=3 -> retained=[TR_a, TC_c, TR_c].
        // TR_a's TC is at idx=0 -> cut=0, retaining everything.
        let history = vec![tc("a"), tc("b"), tr("b"), tr("a"), tc("c"), tr("c")];
        assert_eq!(balance_to_pair_boundary(&history, 3), 0);
    }

    #[test]
    fn balance_noop_when_cut_at_pair_boundary() {
        // [TC_a, TR_a, Msg, TC_b, TR_b]. cut=2 -> retained=[Msg, TC_b, TR_b], balanced.
        let history = vec![tc("a"), tr("a"), msg("u"), tc("b"), tr("b")];
        assert_eq!(balance_to_pair_boundary(&history, 2), 2);
    }

    #[test]
    fn balance_handles_orphan_result_without_matching_call() {
        // Leave ToolResults with no matching ToolCall untouched; broken history cannot
        // be repaired here. cut=1 -> return 1 unchanged.
        let history = vec![msg("u"), tr("zombie")];
        assert_eq!(balance_to_pair_boundary(&history, 1), 1);
    }

    #[test]
    fn balance_keeps_cut_when_call_is_inside_retained() {
        // [Msg, TC_a, TR_a]. cut=1 -> retained=[TC_a, TR_a]. TR_a's call_pos=1 >= idx=1: OK.
        let history = vec![msg("u"), tc("a"), tr("a")];
        assert_eq!(balance_to_pair_boundary(&history, 1), 1);
    }

    #[test]
    fn split_for_retained_aligns_to_pair_boundary() {
        // Add one Msg after a parallel TC*3 / TR*3 turn and set retained to its size.
        // The token-only cut is near the tail, but should retreat before the TC group
        // if needed to avoid orphans.
        let history = vec![
            msg("user"),
            tc("a"),
            tc("b"),
            tc("c"),
            tr("a"),
            tr("b"),
            tr("c"),
            msg("tail"),
        ];
        let total_bytes: u64 = history.iter().map(item_bytes).sum();
        let records = vec![record(history.len(), total_bytes)]; // rate = 1 tok/byte
        // Retain only the tail's item_bytes equivalent.
        let tail_tokens = item_bytes(&history[7]);
        let cut = split_for_retained_impl(&history, &records, tail_tokens);
        // Tokens alone give cut=7 (only the tail retained). A retained head of Msg needs
        // no balancing. Expect index == 7, also checking the balance helper's no-op.
        assert_eq!(cut.index, 7);

        // Slightly increasing retained gives token-only cut=6 (TR_c alone from the tool turn).
        // TR_c is an orphan, so balancing should retreat to 1.
        let big_retain = tail_tokens + item_bytes(&history[6]);
        let cut = split_for_retained_impl(&history, &records, big_retain);
        assert_eq!(cut.index, 1);
    }
}
