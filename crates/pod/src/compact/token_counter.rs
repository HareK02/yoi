//! Usage 履歴ベースのトークン会計。
//!
//! `UsageRecord` の列（プロバイダ実測値）と現在の history から、
//! 「末尾 N トークン残すための split 位置」「prune 射影で節約される
//! トークン数」などを pure に計算する。
//!
//! # 方針
//!
//! - ローカルトークナイザは持たない。実測値があればそれを採用し、
//!   measurement 間はバイト数で按分、最新 measurement より先は最終 rate で外挿する
//! - 推定の出どころは [`EstimateSource`] で呼び出し側に明示する。
//!   課金判断には使えないが、compact/prune の閾値判定には十分な精度
//! - `records` は `history_len` 昇順を仮定する（`collect_state` と
//!   `UsageTracker` がそのように積む）
//!
//! 公開 API は本ファイル内の `impl Pod` で [`Pod`](crate::Pod) のメソッドとして
//! 生やしている。pure な補助関数はこのモジュール内に private に閉じる。

use llm_worker::Item;
use llm_worker::llm_client::client::LlmClient;
use session_store::{Store, UsageRecord};

use crate::Pod;

/// 推定の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateSource {
    /// measurement の境界にちょうど一致（実測値そのもの）
    Measured,
    /// 連続する 2 つの measurement の間をバイト按分で計算
    Interpolated,
    /// 最後の measurement より新しい区間を最終 rate で外挿
    Extrapolated,
    /// measurement が 1 件も無く、バイト数のみのフォールバック
    NoData,
}

/// トークン数の推定値。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub source: EstimateSource,
}

/// history を分割する位置。
///
/// `items[..index]` が捨てる／要約される側、`items[index..]` が残る側。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SplitPoint {
    pub index: usize,
    pub source: EstimateSource,
}

/// `items[..i]` までの累積バイト数（`prefix[i]`）を返す。長さは `items.len()+1`。
fn prefix_bytes(items: &[Item]) -> Vec<u64> {
    let mut prefix = Vec::with_capacity(items.len() + 1);
    let mut acc: u64 = 0;
    prefix.push(0);
    for item in items {
        acc = acc.saturating_add(item_bytes(item));
        prefix.push(acc);
    }
    prefix
}

/// 1 Item の大きさ。JSON シリアライズ長を使う粗い近似。
/// トークン数との絶対変換ではなく区間の按分にしか使わないので、
/// プロバイダごとの overhead は比率でキャンセルされる。
fn item_bytes(item: &Item) -> u64 {
    serde_json::to_string(item)
        .map(|s| s.len() as u64)
        .unwrap_or(0)
}

/// `history[..index]` までのトークン数を推定する。
///
/// `prefix` は [`prefix_bytes`] で得た `history.len() + 1` 長の累積バイト列。
/// 呼び出し側が 1 度だけ計算して使い回すことで、線形探索や複数回の推定が
/// O(n) シリアライズで済む（内部で毎回再計算すると O(n²) になる）。
fn tokens_at(
    history: &[Item],
    records: &[UsageRecord],
    index: usize,
    prefix: &[u64],
) -> TokenEstimate {
    debug_assert!(index <= history.len());
    debug_assert_eq!(prefix.len(), history.len() + 1);

    if index == 0 {
        return TokenEstimate {
            tokens: 0,
            source: EstimateSource::Measured,
        };
    }

    if records.is_empty() {
        return TokenEstimate {
            tokens: prefix[index] / 4,
            source: EstimateSource::NoData,
        };
    }

    // exact match（rev 走査で一番新しい record を採用）
    if let Some(r) = records.iter().rev().find(|r| r.history_len == index) {
        return TokenEstimate {
            tokens: r.input_total_tokens,
            source: EstimateSource::Measured,
        };
    }

    let lower = records.iter().rev().find(|r| r.history_len < index);
    let upper = records.iter().find(|r| r.history_len > index);
    let cap = history.len();

    match (lower, upper) {
        (Some(lo), Some(up)) => {
            let lo_bytes = prefix[lo.history_len.min(cap)];
            let up_bytes = prefix[up.history_len.min(cap)];
            let at_bytes = prefix[index];
            let span_bytes = up_bytes.saturating_sub(lo_bytes);
            let span_tokens = up.input_total_tokens.saturating_sub(lo.input_total_tokens);
            if span_bytes == 0 || span_tokens == 0 {
                return TokenEstimate {
                    tokens: lo.input_total_tokens,
                    source: EstimateSource::Interpolated,
                };
            }
            let delta_bytes = at_bytes.saturating_sub(lo_bytes);
            let delta_tokens =
                (delta_bytes as u128 * span_tokens as u128 / span_bytes as u128) as u64;
            TokenEstimate {
                tokens: lo.input_total_tokens + delta_tokens,
                source: EstimateSource::Interpolated,
            }
        }
        (Some(lo), None) => {
            let lo_bytes = prefix[lo.history_len.min(cap)];
            let at_bytes = prefix[index];
            if lo_bytes == 0 || lo.input_total_tokens == 0 {
                return TokenEstimate {
                    tokens: lo.input_total_tokens,
                    source: EstimateSource::Extrapolated,
                };
            }
            let delta_bytes = at_bytes.saturating_sub(lo_bytes);
            let delta_tokens =
                (delta_bytes as u128 * lo.input_total_tokens as u128 / lo_bytes as u128) as u64;
            TokenEstimate {
                tokens: lo.input_total_tokens + delta_tokens,
                source: EstimateSource::Extrapolated,
            }
        }
        (None, Some(up)) => {
            let up_bytes = prefix[up.history_len.min(cap)];
            let at_bytes = prefix[index];
            if up_bytes == 0 {
                return TokenEstimate {
                    tokens: 0,
                    source: EstimateSource::Interpolated,
                };
            }
            let t = (at_bytes as u128 * up.input_total_tokens as u128 / up_bytes as u128) as u64;
            TokenEstimate {
                tokens: t,
                source: EstimateSource::Interpolated,
            }
        }
        (None, None) => unreachable!("records non-empty but neither lower nor upper matched"),
    }
}

pub(crate) fn total_tokens_impl(history: &[Item], records: &[UsageRecord]) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history.len(), &prefix)
}

/// 任意の history index 時点でのプロンプト全長推定。
/// `history_len == 0` で 0 を返す。delta 計算 (extract trigger 等) で
/// `total_tokens_at(now) - total_tokens_at(pointer)` の形で使う。
pub(crate) fn total_tokens_at_impl(
    history: &[Item],
    records: &[UsageRecord],
    history_len: usize,
) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history_len.min(history.len()), &prefix)
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
    for idx in 1..=history.len() {
        let est = tokens_at(history, records, idx, &prefix);
        if est.tokens >= target {
            chosen_source = est.source;
            return SplitPoint {
                index: idx,
                source: chosen_source,
            };
        }
    }
    SplitPoint {
        index: history.len(),
        source: chosen_source,
    }
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
        total_tokens_impl(self.history(), &usage)
    }

    /// 任意の history index 時点でのプロンプト全長推定。
    ///
    /// `total_tokens()` と同じ accounting を任意位置で評価する版。
    /// memory phase 1 trigger が
    /// `total_tokens_at(now) - total_tokens_at(pointer)` で
    /// pointer 以降に増えたプロンプト長を測るのに使う。
    pub fn total_tokens_at(&self, history_len: usize) -> TokenEstimate {
        let usage = self.usage_history();
        total_tokens_at_impl(self.history(), &usage, history_len)
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
    fn total_no_data_falls_back_to_byte_estimate() {
        let history = vec![msg("hello world")];
        let est = total_tokens_impl(&history, &[]);
        assert_eq!(est.source, EstimateSource::NoData);
        assert!(est.tokens > 0);
    }

    #[test]
    fn total_measured_when_last_record_matches_history_len() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        let records = vec![record(3, 120)];
        let est = total_tokens_impl(&history, &records);
        assert_eq!(est.source, EstimateSource::Measured);
        assert_eq!(est.tokens, 120);
    }

    #[test]
    fn total_extrapolated_when_history_grew_past_last_measurement() {
        let history = vec![msg("a"), msg("b"), msg("c"), msg("d")];
        let records = vec![record(3, 100)];
        let est = total_tokens_impl(&history, &records);
        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert!(est.tokens > 100);
    }

    #[test]
    fn total_zero_history_is_zero() {
        let est = total_tokens_impl(&[], &[]);
        assert_eq!(est.tokens, 0);
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
}
