//! `_staging/*.json` を列挙して [`StagingRecord`] に展開する読み込みヘルパー。
//!
//! Phase 2 起動時のスナップショット（consumed ID list 確定）と、整理 phase
//! が終わった後の cleanup の双方で使う。`.consolidation.lock` のような
//! 占有ファイルは UUIDv7 として parse できないので自然に除外される。
//!
//! [`StagingRecord`] のスキーマは Phase 1 が書き出す側 (`crate::extract`)
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

/// `<staging_dir>/*.json` を読んで UUIDv7 順に並べた [`StagingEntry`]
/// 配列を返す。staging_dir が存在しなければ空配列。読めないファイルや
/// JSON parse 失敗は `tracing::warn!` してスキップ（壊れた個別ファイルが
/// Phase 2 全体を止めないように）。
pub fn list_staging_entries(layout: &WorkspaceLayout) -> Vec<StagingEntry> {
    let dir = layout.staging_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(it) => it,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<StagingEntry> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "json" {
            continue;
        }
        let id = match Uuid::parse_str(stem) {
            Ok(u) => u,
            Err(_) => continue,
        };
        let bytes = match std::fs::metadata(&path) {
            Ok(m) => m.len(),
            Err(_) => 0,
        };
        let raw = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to read staging entry");
                continue;
            }
        };
        let record = match serde_json::from_str::<StagingRecord>(&raw) {
            Ok(r) => r,
            Err(e) => {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractedPayload, write_staging};
    use crate::schema::SourceRef;

    fn empty_payload() -> ExtractedPayload {
        ExtractedPayload::default()
    }

    fn source(session_id: &str, range: [u64; 2]) -> SourceRef {
        SourceRef {
            session_id: session_id.into(),
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
    fn skips_lock_file_and_garbage() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());
        let (_id, _) = write_staging(&layout, source("s", [0, 1]), empty_payload()).unwrap();

        // Drop a non-UUID json file and a bare lock file alongside.
        std::fs::write(layout.staging_dir().join("not-a-uuid.json"), "{}").unwrap();
        std::fs::write(layout.staging_dir().join(".consolidation.lock"), "{}").unwrap();

        let entries = list_staging_entries(&layout);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn missing_dir_returns_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());
        // No staging dir at all.
        assert!(list_staging_entries(&layout).is_empty());
    }
}
