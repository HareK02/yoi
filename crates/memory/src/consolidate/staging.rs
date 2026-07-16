//! `_staging/*.json` を列挙して [`StagingRecord`] に展開する読み込みヘルパー。
//!
//! consolidation 起動時のスナップショット（consumed ID list 確定）と、整理 step
//! が終わった後の cleanup の双方で使う。`.consolidation.lock` のような
//! 占有ファイルは UUIDv7 として parse できないので自然に除外される。
//!
//! [`StagingRecord`] のスキーマは extract が書き出す側 (`crate::extract`)
//! と単一の真実源 — ここでは読み出す側だけを担当する。

use std::path::PathBuf;

use uuid::Uuid;

use crate::extract::StagingRecord;
use crate::workspace::WorkspaceLayout;

/// staging に積まれている 1 件分のエントリ。`id` は UUIDv7 で、ファイル名
/// `<id>.json` を逆引きしたもの。
#[derive(Debug, Clone)]
pub struct StagingEntry {
    pub id: Uuid,
    pub path: PathBuf,
    pub record: StagingRecord,
    /// このファイルのバイト長。閾値判定 (`consolidation_threshold_bytes`)
    /// に使う。
    pub bytes: u64,
}

/// staging directory の検査結果。`entries` は current schema として読めた
/// staging のみで、`invalid_count` は `.json` だが staging として採用できなかった
/// ファイル数。
#[derive(Debug, Clone, Default)]
pub struct StagingEntriesSnapshot {
    pub entries: Vec<StagingEntry>,
    pub invalid_count: usize,
}

/// `<staging_dir>/*.json` を読んで UUIDv7 順に並べた [`StagingEntry`]
/// 配列を返す。staging_dir が存在しなければ空配列。読めないファイルや
/// JSON parse 失敗は `tracing::warn!` してスキップ（壊れた個別ファイルが
/// consolidation 全体を止めないように）。
pub fn list_staging_entries(layout: &WorkspaceLayout) -> Vec<StagingEntry> {
    list_staging_entries_snapshot(layout).entries
}

/// `<staging_dir>/*.json` を読んで valid staging と invalid staging 件数を返す。
/// invalid は自動 migration / 削除 / archive せず、観測可能にするための件数だけを
/// 呼び出し側へ渡す。
pub fn list_staging_entries_snapshot(layout: &WorkspaceLayout) -> StagingEntriesSnapshot {
    let dir = layout.staging_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return StagingEntriesSnapshot::default(),
    };

    let mut out: Vec<StagingEntry> = Vec::new();
    let mut invalid_count = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "json" {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => {
                invalid_count += 1;
                continue;
            }
        };
        let id = match Uuid::parse_str(stem) {
            Ok(u) => u,
            Err(e) => {
                invalid_count += 1;
                tracing::warn!(path = %path.display(), error = %e, "failed to parse staging entry id");
                continue;
            }
        };
        let bytes = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => 0,
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                invalid_count += 1;
                tracing::warn!(path = %path.display(), error = %e, "failed to read staging entry");
                continue;
            }
        };
        let record = match serde_json::from_str::<StagingRecord>(&raw) {
            Ok(r) => r,
            Err(e) => {
                invalid_count += 1;
                tracing::warn!(path = %path.display(), error = %e, "failed to parse staging entry");
                continue;
            }
        };
        out.push(StagingEntry {
            id,
            path,
            record,
            bytes,
        });
    }
    out.sort_by_key(|e| e.id);
    StagingEntriesSnapshot {
        entries: out,
        invalid_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractedPayload, write_staging};
    use crate::schema::SourceRef;

    fn empty_payload() -> ExtractedPayload {
        ExtractedPayload::default()
    }

    fn source(segment_id: &str, range: [u64; 2]) -> SourceRef {
        SourceRef {
            segment_id: segment_id.into(),
            range,
        }
    }

    #[test]
    fn lists_in_uuidv7_order() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());

        let (id1, _) = write_staging(&layout, source("s", [0, 1]), empty_payload()).unwrap();
        let (id2, _) = write_staging(&layout, source("s", [2, 3]), empty_payload()).unwrap();
        let (id3, _) = write_staging(&layout, source("s", [4, 5]), empty_payload()).unwrap();

        let entries = list_staging_entries(&layout);
        let ids: Vec<Uuid> = entries.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![id1, id2, id3]);
    }

    #[test]
    fn skips_lock_file_and_counts_invalid_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());
        let (_id, _) = write_staging(&layout, source("s", [0, 1]), empty_payload()).unwrap();

        // Drop a non-UUID json file, an unparsable UUID-named json file, a
        // legacy source UUID-named json file, and a bare lock file alongside.
        // Lock files are not `.json`; invalid `.json` files are surfaced
        // separately instead of being mistaken for an empty staging directory.
        // Legacy `source.session_id` staging remains readable for compatibility.
        std::fs::write(layout.staging_dir().join("not-a-uuid.json"), "{}").unwrap();
        let bad_id = Uuid::now_v7();
        std::fs::write(layout.staging_dir().join(format!("{bad_id}.json")), "{").unwrap();
        let old_schema_id = Uuid::now_v7();
        std::fs::write(
            layout.staging_dir().join(format!("{old_schema_id}.json")),
            serde_json::json!({
                "source": {
                    "session_id": "legacy-session",
                    "range": [0, 1]
                },
                "requests": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(layout.staging_dir().join(".consolidation.lock"), "{}").unwrap();

        let entries = list_staging_entries(&layout);
        assert_eq!(entries.len(), 2);

        let snapshot = list_staging_entries_snapshot(&layout);
        assert_eq!(snapshot.entries.len(), 2);
        assert_eq!(snapshot.invalid_count, 2);
        assert!(
            snapshot
                .entries
                .iter()
                .any(|entry| entry.record.source.segment_id == "legacy-session")
        );
    }

    #[test]
    fn missing_dir_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());
        // No staging dir at all.
        assert!(list_staging_entries(&layout).is_empty());
    }
}
