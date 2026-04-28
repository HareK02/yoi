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
        llm_worker::token_counter::total_tokens(self.history(), &usage)
    }

    /// 任意の history index 時点でのプロンプト全長推定。
    ///
    /// `total_tokens()` と同じ accounting を任意位置で評価する版。
    /// memory phase 1 trigger が
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
