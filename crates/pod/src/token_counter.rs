//! Usage 履歴ベースのトークン会計。
//!
//! `UsageRecord` の列（プロバイダ実測値）と現在の history から、
//! 「末尾 N トークン残すための split 位置」「指定範囲を drop したときの
//! 節約トークン数」などを pure に計算する。
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

use std::ops::Range;

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

impl EstimateSource {
    fn rank(self) -> u8 {
        match self {
            Self::Measured => 0,
            Self::Interpolated => 1,
            Self::Extrapolated => 2,
            Self::NoData => 3,
        }
    }

    /// 複数の推定を合成するときは一番「粗い」ものに揃える。
    fn worst(self, other: Self) -> Self {
        if self.rank() >= other.rank() {
            self
        } else {
            other
        }
    }
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

fn total_tokens_impl(history: &[Item], records: &[UsageRecord]) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history.len(), &prefix)
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

pub(crate) fn savings_for_drop_impl(
    history: &[Item],
    records: &[UsageRecord],
    range: Range<usize>,
) -> TokenEstimate {
    if range.start >= range.end || range.end > history.len() {
        return TokenEstimate {
            tokens: 0,
            source: EstimateSource::Measured,
        };
    }
    let prefix = prefix_bytes(history);
    let s = tokens_at(history, records, range.start, &prefix);
    let e = tokens_at(history, records, range.end, &prefix);
    TokenEstimate {
        tokens: e.tokens.saturating_sub(s.tokens),
        source: s.source.worst(e.source),
    }
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

    /// 末尾から `retained` トークン以上を残すための分割位置。
    ///
    /// `history[..cut.index]` が要約／破棄される側、`history[cut.index..]` が残る側。
    pub fn split_for_retained(&self, retained: u64) -> SplitPoint {
        let usage = self.usage_history();
        split_for_retained_impl(self.history(), &usage, retained)
    }

    /// 指定範囲を drop したときの節約トークン数の推定。
    pub fn savings_for_drop(&self, range: Range<usize>) -> TokenEstimate {
        let usage = self.usage_history();
        savings_for_drop_impl(self.history(), &usage, range)
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

    #[test]
    fn savings_for_drop_uses_measurement_difference() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        let records = vec![record(1, 100), record(3, 300)];
        let est = savings_for_drop_impl(&history, &records, 1..3);
        assert_eq!(est.tokens, 200);
        assert_eq!(est.source, EstimateSource::Measured);
    }

    #[test]
    fn savings_for_drop_empty_range_is_zero() {
        let history = vec![msg("a"), msg("b")];
        let records = vec![record(2, 100)];
        let est = savings_for_drop_impl(&history, &records, 1..1);
        assert_eq!(est.tokens, 0);
    }

    #[test]
    fn savings_for_drop_interpolates_inside_measurement_span() {
        // len=4 → 400 のみ。range 1..3 は原点 0 と upper=4 の間で按分。
        let history = vec![msg("aa"), msg("aa"), msg("aa"), msg("aa")];
        let records = vec![record(4, 400)];
        let est = savings_for_drop_impl(&history, &records, 1..3);
        assert_eq!(est.source, EstimateSource::Interpolated);
        assert!(est.tokens > 0 && est.tokens < 400);
    }

    #[test]
    fn savings_for_drop_no_records_falls_back_to_bytes() {
        let history = vec![msg("hello hello"), msg("world world")];
        let est = savings_for_drop_impl(&history, &[], 0..1);
        assert_eq!(est.source, EstimateSource::NoData);
        assert!(est.tokens > 0);
    }

    #[test]
    fn savings_for_drop_out_of_range_is_zero() {
        let history = vec![msg("a")];
        let records = vec![record(1, 100)];
        let est = savings_for_drop_impl(&history, &records, 0..5);
        assert_eq!(est.tokens, 0);
    }
}
