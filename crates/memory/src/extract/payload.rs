//! extract 抽出の出力 schema。
//!
//! LLM は [`ExtractedPayload`] そのもの（record-level source 抜き）を返し、Worker 側
//! ラッパーが [`StagingRecord`] に組み立てて staging へ書き出す。
//! source は機械付与する契約 (`docs/plan/memory.md` §Extract)。

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::schema::{SourceEvidenceRef, SourceRef};

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
    /// Host-resolved anchors backing this individual claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub source_refs: Vec<SourceEvidenceRef>,
}

/// 議論したこと（トピック + 論点）。結論が出ていなくてもよい。
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscussionEntry {
    /// 議論の主題。
    pub topic: String,
    /// 主題の中で挙がった論点 / 観点。
    pub points: Vec<String>,
    /// Host-resolved anchors backing this individual claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub source_refs: Vec<SourceEvidenceRef>,
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
    /// Host-resolved anchors backing this individual claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub source_refs: Vec<SourceEvidenceRef>,
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
    /// Host-resolved anchors backing this individual claim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(skip)]
    pub source_refs: Vec<SourceEvidenceRef>,
}

/// staging に書き出される 1 ファイル分のレコード。
///
/// `source` は Worker 側ラッパーが segment_id と log entry range を
/// 機械付与する。LLM はこのフィールドを見ない / 推論しない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagingRecord {
    pub source: SourceRef,
    #[serde(flatten)]
    pub payload: ExtractedPayload,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{EvidenceKind, SourceEvidenceRef};

    fn record_source() -> serde_json::Value {
        serde_json::json!({
            "source": {
                "segment_id": "seg-old",
                "range": [1, 5]
            }
        })
    }

    #[test]
    fn old_staging_json_without_entry_source_refs_deserializes() {
        let mut raw = record_source();
        raw["decisions"] = serde_json::json!([
            {
                "options": ["keep", "drop"],
                "chosen": "keep",
                "rationale": "compatible"
            }
        ]);
        raw["discussions"] = serde_json::json!([
            {
                "topic": "compatibility",
                "points": ["missing source_refs should default empty"]
            }
        ]);
        raw["attempts"] = serde_json::json!([
            {
                "action": "deserialize old staging",
                "result": "ok",
                "succeeded": true
            }
        ]);
        raw["requests"] = serde_json::json!([
            {
                "intent": "preserve old JSON",
                "summary": "old payload has no entry anchors"
            }
        ]);

        let record: StagingRecord = serde_json::from_value(raw).unwrap();

        assert_eq!(record.source.segment_id, "seg-old");
        assert!(record.payload.decisions[0].source_refs.is_empty());
        assert!(record.payload.discussions[0].source_refs.is_empty());
        assert!(record.payload.attempts[0].source_refs.is_empty());
        assert!(record.payload.requests[0].source_refs.is_empty());
        let serialized = serde_json::to_string(&record).unwrap();
        assert!(!serialized.contains("\"source_refs\""));
    }

    #[test]
    fn new_staging_json_roundtrips_entry_source_refs() {
        let evidence = SourceEvidenceRef {
            session_id: Some("session-1".into()),
            segment_id: Some("segment-1".into()),
            entry_range: Some([10, 12]),
            evidence_id: Some("ev-1".into()),
            evidence_kind: Some(EvidenceKind::new(EvidenceKind::TOOL_RESULT)),
            label: Some("cargo test result".into()),
            summary: Some("bounded host summary".into()),
        };
        let record = StagingRecord {
            source: SourceRef {
                segment_id: "segment-record".into(),
                range: [0, 20],
            },
            payload: ExtractedPayload {
                decisions: vec![DecisionEntry {
                    options: vec!["a".into(), "b".into()],
                    chosen: "a".into(),
                    rationale: "evidence-backed".into(),
                    source_refs: vec![evidence.clone()],
                }],
                discussions: vec![DiscussionEntry {
                    topic: "anchor shape".into(),
                    points: vec!["entry refs roundtrip".into()],
                    source_refs: vec![SourceEvidenceRef {
                        evidence_kind: Some(EvidenceKind::new(EvidenceKind::MESSAGE)),
                        ..Default::default()
                    }],
                }],
                attempts: vec![AttemptEntry {
                    action: "serialize".into(),
                    result: "contains source refs".into(),
                    succeeded: true,
                    source_refs: vec![SourceEvidenceRef {
                        evidence_kind: Some(EvidenceKind::new(EvidenceKind::FILE_REF)),
                        evidence_id: Some("file:crates/memory/src/extract/payload.rs".into()),
                        ..Default::default()
                    }],
                }],
                requests: vec![RequestEntry {
                    intent: "keep anchors".into(),
                    target: Some("memory staging".into()),
                    summary: "entry-level anchors survive".into(),
                    source_refs: vec![SourceEvidenceRef {
                        evidence_kind: Some(EvidenceKind::new(EvidenceKind::TICKET_REF)),
                        evidence_id: Some("00001KXNYXNM6".into()),
                        ..Default::default()
                    }],
                }],
            },
        };

        let json = serde_json::to_string_pretty(&record).unwrap();
        assert!(json.contains("source_refs"));
        assert!(json.contains("tool_result"));
        assert!(json.contains("entry_range"));
        let parsed: StagingRecord = serde_json::from_str(&json).unwrap();

        let source_ref = &parsed.payload.decisions[0].source_refs[0];
        assert_eq!(source_ref.session_id.as_deref(), Some("session-1"));
        assert_eq!(source_ref.segment_id.as_deref(), Some("segment-1"));
        assert_eq!(source_ref.entry_range, Some([10, 12]));
        assert_eq!(source_ref.evidence_id.as_deref(), Some("ev-1"));
        assert_eq!(
            source_ref.evidence_kind.as_ref().map(EvidenceKind::as_str),
            Some(EvidenceKind::TOOL_RESULT)
        );
        assert_eq!(source_ref.label.as_deref(), Some("cargo test result"));
        assert_eq!(source_ref.summary.as_deref(), Some("bounded host summary"));
        assert_eq!(
            parsed.payload.discussions[0].source_refs[0]
                .evidence_kind
                .as_ref()
                .map(EvidenceKind::as_str),
            Some(EvidenceKind::MESSAGE)
        );
        assert_eq!(
            parsed.payload.attempts[0].source_refs[0]
                .evidence_kind
                .as_ref()
                .map(EvidenceKind::as_str),
            Some(EvidenceKind::FILE_REF)
        );
        assert_eq!(
            parsed.payload.requests[0].source_refs[0]
                .evidence_kind
                .as_ref()
                .map(EvidenceKind::as_str),
            Some(EvidenceKind::TICKET_REF)
        );
    }
}
