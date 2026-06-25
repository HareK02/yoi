//! `LogEntry::Extension { domain: "memory.extract", payload }` の payload 形式と
//! restore 時の fold ヘルパー。memory crate がドメインを所有するので、
//! session-store / Worker は payload 構造を知らない。

use serde::{Deserialize, Serialize};

use super::EXTRACT_DOMAIN;

/// extract 完了境界の永続化 payload。session log の Extension entry
/// として 1 回ずつ書かれ、最新の 1 件が現行 pointer として有効になる。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractPointerPayload {
    /// 直近 extract が処理した最後の session-store LogEntry の index。
    /// 次回の `source.range.start` はこの値 + 1。
    pub processed_through_entry: usize,
    /// 直近 extract 時点の `history.len()`。次回入力は
    /// `history[processed_through_history_len..]` を切り出す。
    pub processed_through_history_len: usize,
    /// 書き出した staging file の UUIDv7 文字列。LLM が空 payload を返した
    /// 場合は staging file を作らず空文字列で記録する（pointer は前進する）。
    pub staging_id: String,
}

/// `RestoredState.extensions` から最新の extract pointer を取り出す。
/// 未抽出セッションでは `None`。
pub fn fold_pointer(extensions: &[(String, serde_json::Value)]) -> Option<ExtractPointerPayload> {
    extensions
        .iter()
        .rev()
        .find(|(domain, _)| domain == EXTRACT_DOMAIN)
        .and_then(|(_, value)| serde_json::from_value(value.clone()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_returns_latest_when_multiple_present() {
        let exts = vec![
            (
                EXTRACT_DOMAIN.to_string(),
                serde_json::json!({
                    "processed_through_entry": 5,
                    "processed_through_history_len": 4,
                    "staging_id": "old"
                }),
            ),
            ("other.domain".to_string(), serde_json::json!({ "x": 1 })),
            (
                EXTRACT_DOMAIN.to_string(),
                serde_json::json!({
                    "processed_through_entry": 11,
                    "processed_through_history_len": 8,
                    "staging_id": "new"
                }),
            ),
        ];
        let p = fold_pointer(&exts).unwrap();
        assert_eq!(p.processed_through_entry, 11);
        assert_eq!(p.processed_through_history_len, 8);
        assert_eq!(p.staging_id, "new");
    }

    #[test]
    fn fold_returns_none_when_absent() {
        let exts = vec![("other.domain".to_string(), serde_json::json!({ "x": 1 }))];
        assert!(fold_pointer(&exts).is_none());
    }

    #[test]
    fn fold_skips_malformed_entries() {
        let exts = vec![(
            EXTRACT_DOMAIN.to_string(),
            serde_json::json!({ "wrong_shape": true }),
        )];
        // 現状は最新を取り出して JSON 不一致なら None。古いものに fallback
        // しないのは、壊れた最新を黙って無視すると意図しない再抽出を招くため。
        assert!(fold_pointer(&exts).is_none());
    }
}
