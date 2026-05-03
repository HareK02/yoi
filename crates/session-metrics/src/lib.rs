//! Session metrics — generic ad-hoc measurement lane on top of
//! `LogEntry::Extension { domain: "metrics" }`.
//!
//! セッション中に積み上げて後で引きたい値（prune の発火頻度・Hook の実行
//! 時間・ツールリトライ回数 等）を session-log に乗せるための薄い層。
//! session-store は payload を不透明な `serde_json::Value` として扱うので、
//! このクレートは型と読み書きヘルパーだけを提供する。
//!
//! # 設計
//!
//! - 厳格な label set は持たない。次元は sparse な `BTreeMap<String,String>`、
//!   観測できない値は `None` で明示する
//! - 「後から埋まる値」（例: prune 発火直後の `cache_read_tokens`）は前 entry に
//!   書き戻さず、`correlation_id` を共有する別 metric として流す。集計は読み手で join
//! - 集計 / 可視化 API はこのクレートには無い。session-log を読めば取り出せる、
//!   までが到達点

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use session_store::{EntryHash, SessionId, Store, StoreError, save_extension, session_log};

/// Domain tag used in `LogEntry::Extension` for all metrics records.
pub const DOMAIN: &str = "metrics";

/// 単発の計測値。`name` は `namespace.metric` 形式の自由文字列
/// （例: `"prune.fire"`、`"hook.duration"`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metric {
    /// `namespace.metric` 形式の名前。
    pub name: String,
    /// epoch ms。
    pub ts: u64,
    /// sparse な次元（label）。観測できないものはキー自体を入れない。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dimensions: BTreeMap<String, String>,
    /// 主スカラ値。dimension では表現したくない数値を載せる場所。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// 関連 metric を join するためのキー。同一 ID を持つ複数 metric は
    /// 「同じ事象を多面的に観測している」という意味付けで読まれる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
}

impl Metric {
    /// 最小コンストラクタ。`ts` は呼び出し時刻（epoch ms）で埋める。
    pub fn now(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ts: session_log::now_millis(),
            dimensions: BTreeMap::new(),
            value: None,
            correlation_id: None,
        }
    }

    pub fn with_dimension(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.dimensions.insert(key.into(), value.into());
        self
    }

    pub fn with_value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }

    pub fn with_correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }
}

/// `LogEntry::Extension { domain: "metrics", payload: <metric> }` を append する。
///
/// `save_extension` の薄い wrapper。書き込み失敗は呼び出し側に返す
/// （メトリクスのために本体処理を止めるかは呼び出し側の判断）。
pub async fn record_metric(
    store: &impl Store,
    session_id: SessionId,
    head_hash: &mut Option<EntryHash>,
    metric: &Metric,
) -> Result<(), StoreError> {
    let payload = serde_json::to_value(metric).expect("Metric serialization cannot fail");
    save_extension(store, session_id, head_hash, DOMAIN, payload).await
}

/// `RestoredState.extensions` から metrics domain の payload を順に取り出し、
/// `Metric` 列に fold する。
///
/// schema 変更で deserialize できない payload は無視する（後方互換）。
pub fn metrics_from_extensions(
    extensions: &[(String, serde_json::Value)],
) -> Vec<Metric> {
    extensions
        .iter()
        .filter(|(domain, _)| domain == DOMAIN)
        .filter_map(|(_, payload)| serde_json::from_value::<Metric>(payload.clone()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_round_trip_via_json() {
        let metric = Metric::now("prune.fire")
            .with_dimension("border_turn", "3")
            .with_dimension("candidate_count", "2")
            .with_value(4096.0)
            .with_correlation_id("abc-123");
        let json = serde_json::to_string(&metric).unwrap();
        let parsed: Metric = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, metric);
    }

    #[test]
    fn metric_serializes_minimal_form_compactly() {
        // dimensions が空 / value/correlation_id が None の時は出力に含めない。
        let metric = Metric {
            name: "x".into(),
            ts: 1,
            dimensions: BTreeMap::new(),
            value: None,
            correlation_id: None,
        };
        let json = serde_json::to_string(&metric).unwrap();
        assert!(!json.contains("dimensions"));
        assert!(!json.contains("value"));
        assert!(!json.contains("correlation_id"));
    }

    #[test]
    fn fold_skips_other_domains() {
        let extensions = vec![
            (
                "memory.extract".into(),
                serde_json::json!({ "processed_through_entry": 7 }),
            ),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("a")).unwrap(),
            ),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("b")).unwrap(),
            ),
        ];
        let metrics = metrics_from_extensions(&extensions);
        assert_eq!(metrics.len(), 2);
        assert_eq!(metrics[0].name, "a");
        assert_eq!(metrics[1].name, "b");
    }

    #[test]
    fn fold_skips_undeserializable_payloads() {
        // 将来 schema が変わって読めない payload も skip して落ちない。
        let extensions = vec![
            (DOMAIN.into(), serde_json::json!({ "garbage": true })),
            (
                DOMAIN.into(),
                serde_json::to_value(Metric::now("ok")).unwrap(),
            ),
        ];
        let metrics = metrics_from_extensions(&extensions);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].name, "ok");
    }
}
