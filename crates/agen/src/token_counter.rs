//! Token estimates based on provider measurements and history growth.
//!
//! Uses the full input token count from [`UsageRecord`], interpolating between
//! measured boundaries by byte count and extrapolating beyond the latest boundary
//! from text bytes. Input totals include cache reads rather than inventing a
//! system/history breakdown.
//!
//! [`EstimateSource`] distinguishes measurements from estimates. Estimates inform
//! threshold decisions; they neither determine billing nor guarantee that a request
//! fits the provider's context limit. Attachment base64 is a storage representation,
//! not text input, and is excluded from byte counts. Provider measurements for
//! requests that actually sent images remain valid. The caller supplies records in
//! ascending `history_len` order.

use crate::{Item, UsageRecord};

/// Source of a token estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstimateSource {
    /// Exact match with a measurement boundary; uses the measured value.
    Measured,
    /// Interpolated between consecutive measurements in proportion to byte count.
    Interpolated,
    /// Extrapolated beyond the latest measurement using the bytes/4 fallback.
    Extrapolated,
    /// Byte-count fallback with no measurements available.
    NoData,
}

/// Estimated token count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenEstimate {
    pub tokens: u64,
    pub source: EstimateSource,
}

/// Returns cumulative bytes for `items[..i]` in `prefix[i]`, with length `items.len() + 1`.
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

/// Approximate text size of one Item, using its JSON serialization length.
///
/// Binary attachments are deliberately projected out. Their durable base64 encoding is a
/// storage representation, not provider text, so charging it at the text `bytes / 4` fallback
/// rate produces meaningless estimates. Exact provider usage measurements remain authoritative
/// when an attachment was actually sent.
pub fn item_bytes(item: &Item) -> u64 {
    let mut projected = item.clone();
    if let Item::ToolResult { attachments, .. } = &mut projected {
        attachments.clear();
    }
    serde_json::to_string(&projected)
        .map(|s| s.len() as u64)
        .unwrap_or(0)
}

/// Estimates the token count of `history[..index]`.
///
/// `prefix` contains `history.len() + 1` cumulative byte counts from [`prefix_bytes`].
/// Computing it once and reusing it keeps serialization for a scan or repeated
/// estimates O(n), rather than O(n²) from recomputing it for every estimate.
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

    // Exact match: reverse iteration selects the most recent record.
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

/// Estimated token count for the entire current history.
pub fn total_tokens(history: &[Item], records: &[UsageRecord]) -> TokenEstimate {
    let prefix = prefix_bytes(history);
    tokens_at(history, records, history.len(), &prefix)
}

/// Estimates the full prompt token count at an arbitrary history boundary.
/// Returns zero for `history_len == 0`. Differences such as
/// `total_tokens_at(now) - total_tokens_at(pointer)` can drive extraction triggers.
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
    fn fallback_does_not_count_durable_image_bytes_as_text_tokens() {
        let small = Item::tool_result_item_with_attachments(
            "call",
            "Attached image.png",
            None,
            false,
            vec![crate::tool::Attachment::Image(
                crate::tool::ImageAttachment::new("image/png", vec![1_u8; 8]),
            )],
        );
        let large = Item::tool_result_item_with_attachments(
            "call",
            "Attached image.png",
            None,
            false,
            vec![crate::tool::Attachment::Image(
                crate::tool::ImageAttachment::new("image/png", vec![1_u8; 1_000_000]),
            )],
        );

        assert_eq!(item_bytes(&small), item_bytes(&large));
        assert_eq!(total_tokens(&[small], &[]), total_tokens(&[large], &[]));
    }

    #[test]
    fn total_zero_history_is_zero() {
        let est = total_tokens(&[], &[]);
        assert_eq!(est.tokens, 0);
    }
}
