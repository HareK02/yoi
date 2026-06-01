//! Usage 履歴ベースのトークン会計（汎用部分）。
//!
//! `UsageRecord` の列（プロバイダ実測値）と現在の history から、
//! 任意の history index 時点のプロンプト全長トークン数を pure に計算する。
//!
//! # 方針
//!
//! - ローカルトークナイザは持たない。実測値があればそれを採用し、
//!   measurement 間はバイト数で按分、最新 measurement より先は byte/4
//!   fallback で外挿する
//! - 推定の出どころは [`EstimateSource`] で呼び出し側に明示する。
//!   課金判断には使えないが、compact / prune / memory extract trigger 等の
//!   閾値判定には十分な精度
//! - `records` は `history_len` 昇順を仮定する（呼び出し側がそのように積む）

use crate::{Item, UsageRecord};

/// 推定の出どころ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateSource {
    /// measurement の境界にちょうど一致（実測値そのもの）
    Measured,
    /// 連続する 2 つの measurement の間をバイト按分で計算
    Interpolated,
    /// 最後の measurement より新しい区間を byte/4 fallback で外挿
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

/// `items[..i]` までの累積バイト数（`prefix[i]`）を返す。長さは `items.len()+1`。
pub fn prefix_bytes(items: &[Item]) -> Vec<u64> {
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
pub fn item_bytes(item: &Item) -> u64 {
    serde_json::to_string(item)
        .map(|s| s.len() as u64)
        .unwrap_or(0)
}

/// `history[..index]` までのトークン数を推定する。
///
/// `prefix` は [`prefix_bytes`] で得た `history.len() + 1` 長の累積バイト列。
/// 呼び出し側が 1 度だけ計算して使い回すことで、線形探索や複数回の推定が
/// O(n) シリアライズで済む（内部で毎回再計算すると O(n²) になる）。
pub fn tokens_at(
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
            let delta_bytes = at_bytes.saturating_sub(lo_bytes);

            TokenEstimate {
                tokens: lo.input_total_tokens.saturating_add(delta_bytes / 4),
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

/// 現在の history 全体の推定トークン数。
pub fn total_tokens(history: &[Item], records: &[UsageRecord]) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history.len(), &prefix)
}

/// 任意の history index 時点でのプロンプト全長推定。
/// `history_len == 0` で 0 を返す。delta 計算 (extract trigger 等) で
/// `total_tokens_at(now) - total_tokens_at(pointer)` の形で使う。
pub fn total_tokens_at(
    history: &[Item],
    records: &[UsageRecord],
    history_len: usize,
) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history_len.min(history.len()), &prefix)
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
        let est = total_tokens(&history, &[]);
        assert_eq!(est.source, EstimateSource::NoData);
        assert!(est.tokens > 0);
    }

    #[test]
    fn total_measured_when_last_record_matches_history_len() {
        let history = vec![msg("a"), msg("b"), msg("c")];
        let records = vec![record(3, 120)];
        let est = total_tokens(&history, &records);
        assert_eq!(est.source, EstimateSource::Measured);
        assert_eq!(est.tokens, 120);
    }

    #[test]
    fn total_extrapolated_when_history_grew_past_last_measurement() {
        let history = vec![msg("a"), msg("b"), msg("c"), msg("d")];
        let records = vec![record(3, 100)];
        let est = total_tokens(&history, &records);
        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert!(est.tokens > 100);
    }

    #[test]
    fn extrapolation_after_single_measurement_uses_byte_fallback_not_total_prompt_rate() {
        let history = vec![msg("first"), msg(&"tool output ".repeat(400))];
        let records = vec![record(1, 11_124)];
        let prefix = prefix_bytes(&history);
        let delta_bytes = prefix[2].saturating_sub(prefix[1]);

        let est = total_tokens(&history, &records);

        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert_eq!(est.tokens, 11_124 + delta_bytes / 4);

        let old_projection =
            11_124 + (delta_bytes as u128 * 11_124_u128 / prefix[1] as u128) as u64;
        assert!(
            old_projection > est.tokens.saturating_mul(10),
            "old_projection={old_projection}, corrected={}",
            est.tokens
        );
    }

    #[test]
    fn extrapolation_after_multiple_measurements_uses_byte_fallback_for_unmeasured_delta() {
        let history = vec![
            msg("first"),
            msg(&"measured increment ".repeat(20)),
            msg(&"unmeasured increment ".repeat(30)),
        ];
        let records = vec![record(1, 10_000), record(2, 10_200)];
        let prefix = prefix_bytes(&history);
        let delta_bytes = prefix[3].saturating_sub(prefix[2]);

        let est = total_tokens(&history, &records);

        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert_eq!(est.tokens, 10_200 + delta_bytes / 4);
    }

    #[test]
    fn extrapolation_does_not_reuse_measured_rate_after_context_projection() {
        let compacted_span = msg("x");
        let projected = vec![
            msg("first"),
            msg("summary only"),
            compacted_span,
            msg("new user input"),
        ];
        let records = vec![record(1, 10_000), record(3, 30_000)];
        let prefix = prefix_bytes(&projected);
        let delta_bytes = prefix[4].saturating_sub(prefix[3]);

        let est = total_tokens(&projected, &records);

        assert_eq!(est.source, EstimateSource::Extrapolated);
        assert_eq!(est.tokens, 30_000 + delta_bytes / 4);
    }

    #[test]
    fn total_zero_history_is_zero() {
        let est = total_tokens(&[], &[]);
        assert_eq!(est.tokens, 0);
    }
}
