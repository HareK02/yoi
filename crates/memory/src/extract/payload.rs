//! extract 抽出の出力 schema。
//!
//! LLM は [`ExtractedPayload`] そのもの（source 抜き）を返し、Pod 側
//! ラッパーが [`StagingRecord`] に組み立てて staging へ書き出す。
//! source は機械付与する契約 (`docs/plan/memory.md` §Extract)。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::schema::SourceRef;

/// LLM が返す活動ログ候補の集合。すべて optional（空配列は許容）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedPayload {
    #[serde(default)]
    pub decisions: Vec<DecisionEntry>,
    #[serde(default)]
    pub discussions: Vec<DiscussionEntry>,
    #[serde(default)]
    pub attempts: Vec<AttemptEntry>,
    #[serde(default)]
    pub requests: Vec<RequestEntry>,
}

impl ExtractedPayload {
    /// すべての配列が空であれば true。空ペイロードは
    /// "Nothing to save" 扱いで staging への書き込みを省いてよい。
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
            && self.discussions.is_empty()
            && self.attempts.is_empty()
            && self.requests.is_empty()
    }
}

/// 判断したこと（選択肢 + 選んだ + 根拠）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DecisionEntry {
    /// 検討された選択肢の列挙。
    pub options: Vec<String>,
    /// 採用された選択肢。
    pub chosen: String,
    /// 採用理由 / 根拠。
    pub rationale: String,
}

/// 議論したこと（トピック + 論点）。結論が出ていなくてもよい。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscussionEntry {
    /// 議論の主題。
    pub topic: String,
    /// 主題の中で挙がった論点 / 観点。
    pub points: Vec<String>,
}

/// 試したこと（試行 + 結果 + 成否）。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttemptEntry {
    /// 何を試したか。
    pub action: String,
    /// 試した結果。
    pub result: String,
    /// 試行が目的に対して成功したか。失敗 / 部分成功も含めて bool で表現する。
    pub succeeded: bool,
}

/// ユーザー submit の構造化要約。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestEntry {
    /// ユーザーの意図 / ゴール。
    pub intent: String,
    /// 対象ファイル / モジュール / 機能（任意）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// 一文サマリ。
    pub summary: String,
}

/// staging に書き出される 1 ファイル分のレコード。
///
/// `source` は Pod 側ラッパーが session_id と log entry range を
/// 機械付与する。LLM はこのフィールドを見ない / 推論しない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingRecord {
    pub source: SourceRef,
    #[serde(flatten)]
    pub payload: ExtractedPayload,
}
