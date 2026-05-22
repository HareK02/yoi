//! Compact / prune 専用のトークン会計補助。
//!
//! 汎用部分（`prefix_bytes`, `tokens_at`, `total_tokens`, `total_tokens_at`）は
//! [`llm_worker::token_counter`] にあり、`UsageRecord` の列と現在の history から
//! pure に推定する。本モジュールは compact / prune 固有のロジック
//! （`split_for_retained`, `savings_for_prune`）と、Pod 上の公開 API に
//! 限定する。
//!
//! # 方針
//!
//! - ローカルトークナイザは持たない。実測値があればそれを採用し、
//!   measurement 間はバイト数で按分、最新 measurement より先は最終 rate で外挿する
//! - 推定の出どころは [`EstimateSource`] で呼び出し側に明示する。
//!   課金判断には使えないが、compact / prune の閾値判定には十分な精度

use llm_worker::llm_client::client::LlmClient;
use llm_worker::token_counter::{item_bytes, prefix_bytes, tokens_at};
use llm_worker::{Item, UsageRecord};
use session_store::Store;

pub use llm_worker::token_counter::{EstimateSource, TokenEstimate};

use crate::Pod;

/// history を分割する位置。
///
/// `items[..index]` が捨てる／要約される側、`items[index..]` が残る側。
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
    let target = current.tokens - retained;

    // `tokens_at` が target 以上になる最小の idx を線形探索。
    // prefix を使い回すので 1 回の split 呼び出しあたり O(n) で済む
    // （内部で毎回再計算すると O(n²) になる）。将来ボトルネックになれば
    // record 境界で二分探索に置き換える。
    let mut chosen_source = current.source;
    let mut cut_index = history.len();
    for idx in 1..=history.len() {
        let est = tokens_at(history, records, idx, &prefix);
        if est.tokens >= target {
            chosen_source = est.source;
            cut_index = idx;
            break;
        }
    }
    SplitPoint {
        index: balance_to_pair_boundary(history, cut_index),
        source: chosen_source,
    }
}

/// `history[cut..]` が `ToolCall` / `ToolResult` のペア境界を尊重するよう
/// `cut` を後退させる。
///
/// LLM API は「`ToolResult` を送るならその `ToolCall` も同じ request に
/// 含まれていなければならない」というバリデーションを持つ。トークン数
/// だけで切った `cut` は並列 tool 呼び出しの途中に落ちうるので、retained
/// 側の先頭に対応 `ToolCall` を持たない `ToolResult`（orphan）が残ると
/// 次セッション初回 request が API バリデーションで弾かれる。
///
/// 対策は「retained に入る `ToolResult` について、対応 `ToolCall` も
/// retained に含まれる位置まで `cut` を引き下げる」こと。retained_tokens
/// 予算は超えうるが、ここでは直接 LLM に投げる訳ではなく次の
/// `pre_llm_request` で再評価されるだけなので safe。
///
/// アルゴリズム: history を末尾から走査し、retained 範囲内の `ToolResult`
/// に出会うたびに対応 `ToolCall` の位置で `cut` を min 更新する。`cut` が
/// 下がると以前は要約側だった位置が retained に入るので、後続走査で連鎖的
/// に正しい位置まで引き下がる。`ToolCall` の `call_id` はユニークなので
/// 事前にマップ化して O(n) で済ます。
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

/// 1 つの ToolResult 項目について、`content` を `None` に射影したとき
/// 減少するシリアライズ後バイト数。ToolResult 以外や既に content=None
/// の item は 0 を返す。
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

/// Prune 射影（`ToolResult.content = None`）で節約されるトークン数の推定。
///
/// `indices` は [`llm_worker::prune::prunable_indices`] が返す候補列を
/// 想定する。各候補の content バイト差分を合算し、usage 履歴由来の
/// tokens/byte レートでトークン数に換算する。範囲を「丸ごと drop」する
/// のではなく、item 自体（summary 等）は残したままの値を返す点が
/// `tokens_at` ベースの計算と異なる。
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

    // 最新の measurement を使って tokens/byte を求め、バイト差分を換算する。
    // 実測値そのものではなく比率しか使わないので、history_len と
    // record.history_len が一致しなくても rate は正しい。
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

// ── Pod に生やす公開 API ───────────────────────────────────────────────

impl<C: LlmClient, St: Store> Pod<C, St> {
    /// 現在の history 全体の推定トークン数。
    ///
    /// 最後の measurement と、その後に追加された未測定分のバイト按分／外挿。
    pub fn total_tokens(&self) -> TokenEstimate {
        let usage = self.usage_history();
        llm_worker::token_counter::total_tokens(self.history(), &usage)
    }

    /// 任意の history index 時点でのプロンプト全長推定。
    ///
    /// `total_tokens()` と同じ accounting を任意位置で評価する版。
    /// memory extract trigger が
    /// `total_tokens_at(now) - total_tokens_at(pointer)` で
    /// pointer 以降に増えたプロンプト長を測るのに使う。
    pub fn total_tokens_at(&self, history_len: usize) -> TokenEstimate {
        let usage = self.usage_history();
        llm_worker::token_counter::total_tokens_at(self.history(), &usage, history_len)
    }

    /// 末尾から `retained` トークン以上を残すための分割位置。
    ///
    /// `history[..cut.index]` が要約／破棄される側、`history[cut.index..]` が残る側。
    pub fn split_for_retained(&self, retained: u64) -> SplitPoint {
        let usage = self.usage_history();
        split_for_retained_impl(self.history(), &usage, retained)
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
    fn split_at_exact_measurement_boundary() {
        // 4 items。measurements: len=2 → 100, len=4 → 300。
        // retained=200 → target_drop = 100 → record[0] にぴったり一致 → index=2。
        let history = vec![msg("a"), msg("b"), msg("c"), msg("d")];
        let records = vec![record(2, 100), record(4, 300)];
        let cut = split_for_retained_impl(&history, &records, 200);
        assert_eq!(cut.index, 2);
        assert_eq!(cut.source, EstimateSource::Measured);
    }

    #[test]
    fn split_interpolated_between_measurements() {
        let history = vec![msg("aaaaaa"), msg("bbbbbb"), msg("cccccc"), msg("dddddd")];
        let records = vec![record(1, 50), record(4, 400)];
        let cut = split_for_retained_impl(&history, &records, 250);
        assert!(cut.index > 1 && cut.index <= 4);
        assert_eq!(cut.source, EstimateSource::Interpolated);
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
        // cut=5 → retained=[TR_b, TR_c]。TR_c の TC は idx=3、TR_b は idx=2 →
        // idx=2 まで後退。だが retained に TR_a (idx=4) が新たに入り、その TC_a
        // は idx=1 でまだ外 → 連鎖後退で最終的に idx=1。retained は
        // [TC_a, TC_b, TC_c, TR_a, TR_b, TR_c]。
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
        // [TC_a, TR_a, TC_b, TR_b]。cut=3 → retained=[TR_b] orphan。
        // TC_b は idx=2 → cut=2。retained=[TC_b, TR_b]。
        let history = vec![tc("a"), tr("a"), tc("b"), tr("b")];
        assert_eq!(balance_to_pair_boundary(&history, 3), 2);
    }

    #[test]
    fn balance_cascades_through_nested_pairs() {
        // [TC_a, TC_b, TR_b, TR_a, TC_c, TR_c]。cut=3 → retained=[TR_a, TC_c, TR_c]。
        // TR_a の TC は idx=0 → cut=0。retained=full。
        let history = vec![tc("a"), tc("b"), tr("b"), tr("a"), tc("c"), tr("c")];
        assert_eq!(balance_to_pair_boundary(&history, 3), 0);
    }

    #[test]
    fn balance_noop_when_cut_at_pair_boundary() {
        // [TC_a, TR_a, Msg, TC_b, TR_b]。cut=2 → retained=[Msg, TC_b, TR_b] balanced。
        let history = vec![tc("a"), tr("a"), msg("u"), tc("b"), tr("b")];
        assert_eq!(balance_to_pair_boundary(&history, 2), 2);
    }

    #[test]
    fn balance_handles_orphan_result_without_matching_call() {
        // ToolCall がそもそも存在しない ToolResult は触らない（壊れた history は
        // ここでは直しようがない）。cut=1 → そのまま 1 を返す。
        let history = vec![msg("u"), tr("zombie")];
        assert_eq!(balance_to_pair_boundary(&history, 1), 1);
    }

    #[test]
    fn balance_keeps_cut_when_call_is_inside_retained() {
        // [Msg, TC_a, TR_a]。cut=1 → retained=[TC_a, TR_a]。TR_a の call_pos=1 >= idx=1。OK。
        let history = vec![msg("u"), tc("a"), tr("a")];
        assert_eq!(balance_to_pair_boundary(&history, 1), 1);
    }

    #[test]
    fn split_for_retained_aligns_to_pair_boundary() {
        // 並列 TC*3 / TR*3 ターン後に Msg を 1 件足し、retained=Msg のサイズ相当に
        // 設定。トークン的には cut=末尾近くだが、orphan を避けるため TC 群の手前
        // まで後退するはず。
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
        // tail の item_bytes 相当のみ retain したい。
        let tail_tokens = item_bytes(&history[7]);
        let cut = split_for_retained_impl(&history, &records, tail_tokens);
        // token 単独だと cut は 7（tail のみ retained）になるが、retained 先頭が
        // Msg なら balance しなくて OK。balance helper の no-op を確認する意味も込めて
        // index == 7 を期待する。
        assert_eq!(cut.index, 7);

        // 逆に retained をやや増やしてトークン的に cut=6（TR_c のみ retained）に
        // させると、TR_c は orphan なので balance が 1 まで後退するはず。
        let big_retain = tail_tokens + item_bytes(&history[6]);
        let cut = split_for_retained_impl(&history, &records, big_retain);
        assert_eq!(cut.index, 1);
    }
}
