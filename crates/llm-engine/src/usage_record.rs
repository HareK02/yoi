//! Per-LLM-request Usage measurement snapshot.
//!
//! 1 リクエストの送信時点での「ある history prefix 長で計測した占有量」を
//! 1 件分にまとめたもの。`UsageEvent` (provider stream イベント) を
//! 受けて呼び出し側 (typically Worker) が組み立て、永続化層
//! (session-store) に流したり、token accounting (`token_counter`) で
//! 履歴として参照したりする。

/// LLM リクエスト送信時点での占有量スナップショット。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageRecord {
    /// 送信時の history.len()
    pub history_len: usize,
    /// history[..history_len] の占有量（プロンプト全長、実測）
    pub input_total_tokens: u64,
    /// 上記のうちキャッシュから読み出された分
    pub cache_read_tokens: u64,
    /// 上記のうちこのリクエストでキャッシュに書かれた分
    pub cache_write_tokens: u64,
    /// このリクエストで生成された出力トークン数
    pub output_tokens: u64,
}
