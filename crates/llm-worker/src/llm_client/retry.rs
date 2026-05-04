//! HTTP transient エラー向けリトライポリシー。
//!
//! `transport.rs` の HTTP 送信〜ステータスチェック区間で `is_retryable`
//! が true を返した失敗をリトライする際に、待ち時間と打ち切り条件を
//! 提供する。SSE 読み出し開始後の失敗は対象外。

use std::time::Duration;

/// 指数バックオフ + ジッター + 累積タイムアウトを表すポリシー。
///
/// `Default` は llm-worker 全体の固定値を返す。manifest 経由の上書きが
/// 必要になったら拡張する（現状は不要 → `tickets/llm-worker-transient-retry.md`）。
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// 指数の基準値。`base * 2^attempt` を `cap` で頭打ちにした上限から
    /// フルジッターで実際の wait を抽選する。
    pub base: Duration,
    /// 1 回あたりの wait の上限。
    pub cap: Duration,
    /// 試行の合計回数（初回 + リトライ）。`1` ならリトライしない。
    pub max_attempts: u32,
    /// 初回送信開始からの累積タイムアウト。これを超える wait は打ち切る。
    pub total_timeout: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(500),
            cap: Duration::from_secs(10),
            max_attempts: 4,
            total_timeout: Duration::from_secs(30),
        }
    }
}

impl RetryPolicy {
    /// `attempt` 回目の失敗（0-indexed）後に待つ時間を返す。
    /// `Retry-After` で上書きしたい場合は呼び出さず、その値をそのまま使う。
    pub fn backoff(&self, attempt: u32) -> Duration {
        let shift = attempt.min(20);
        let base_nanos = self.base.as_nanos() as u64;
        let exp_nanos = base_nanos.saturating_mul(1u64 << shift);
        let cap_nanos = self.cap.as_nanos() as u64;
        let upper = exp_nanos.min(cap_nanos);
        Duration::from_nanos(jitter_nanos(upper))
    }
}

/// `[0, max_nanos]` から擬似乱数的に 1 つ取り出す。`SystemTime` の
/// 下位ビットを splitmix64 で攪拌するだけの軽量実装で、暗号的乱数性は
/// 持たないがフルジッターのぶつかり回避には十分。
fn jitter_nanos(max_nanos: u64) -> u64 {
    if max_nanos == 0 {
        return 0;
    }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x % (max_nanos + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_values() {
        let p = RetryPolicy::default();
        assert_eq!(p.base, Duration::from_millis(500));
        assert_eq!(p.cap, Duration::from_secs(10));
        assert_eq!(p.max_attempts, 4);
        assert_eq!(p.total_timeout, Duration::from_secs(30));
    }

    #[test]
    fn backoff_respects_cap() {
        let p = RetryPolicy::default();
        for attempt in 0..30u32 {
            assert!(
                p.backoff(attempt) <= p.cap,
                "attempt {attempt} exceeded cap",
            );
        }
    }

    #[test]
    fn backoff_zero_when_base_zero() {
        let p = RetryPolicy {
            base: Duration::ZERO,
            cap: Duration::from_secs(10),
            max_attempts: 4,
            total_timeout: Duration::from_secs(30),
        };
        for attempt in 0..5 {
            assert_eq!(p.backoff(attempt), Duration::ZERO);
        }
    }
}
