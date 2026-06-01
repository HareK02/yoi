//! `<workspace>/.yoi/memory/_staging/<id>.json` への書き出しヘルパー。
//!
//! 1 件 1 ファイル、UUIDv7 命名（短命なので衝突回避と順序を兼ねる）。
//! `source` を機械付与した [`StagingRecord`] 形式で保存する。

use std::fs;
use std::path::PathBuf;

use uuid::Uuid;

use crate::extract::payload::{ExtractedPayload, StagingRecord};
use crate::schema::SourceRef;
use crate::workspace::WorkspaceLayout;

/// staging 書き出し時のエラー。
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("failed to create staging dir {}: {source}", .path.display())]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write staging file {}: {source}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to serialize staging record: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// `payload` を `source` で wrap して staging に書き出す。
///
/// 戻り値は割り当てられた staging file の (id, path)。`payload` が
/// 完全に空の場合は呼び出し側が事前に `is_empty()` で skip 推奨だが、
/// この関数は空でも正規に書き出す（仕様 §Extract で空配列許容と
/// 明記されており、書く / 書かないの判断は呼び出し側に委ねる）。
pub fn write_staging(
    layout: &WorkspaceLayout,
    source: SourceRef,
    payload: ExtractedPayload,
) -> Result<(Uuid, PathBuf), StagingError> {
    let staging_dir = layout.staging_dir();
    fs::create_dir_all(&staging_dir).map_err(|source| StagingError::CreateDir {
        path: staging_dir.clone(),
        source,
    })?;

    let id = Uuid::now_v7();
    let path = staging_dir.join(format!("{id}.json"));
    let record = StagingRecord { source, payload };
    let json = serde_json::to_string_pretty(&record)?;
    fs::write(&path, json).map_err(|source| StagingError::Write {
        path: path.clone(),
        source,
    })?;
    Ok((id, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extract::payload::{DecisionEntry, ExtractedPayload};

    #[test]
    fn writes_record_with_machine_attached_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());

        let source = SourceRef {
            segment_id: "sess-1".into(),
            range: [3, 7],
        };
        let payload = ExtractedPayload {
            decisions: vec![DecisionEntry {
                options: vec!["a".into(), "b".into()],
                chosen: "a".into(),
                rationale: "shorter".into(),
            }],
            ..Default::default()
        };
        let (id, path) = write_staging(&layout, source.clone(), payload).unwrap();
        assert_eq!(path.parent().unwrap(), layout.staging_dir());
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(&id.to_string())
        );

        let written: StagingRecord =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written.source.segment_id, "sess-1");
        assert_eq!(written.source.range, [3, 7]);
        assert_eq!(written.payload.decisions.len(), 1);
    }

    #[test]
    fn empty_payload_is_written_verbatim() {
        let tmp = tempfile::TempDir::new().unwrap();
        let layout = WorkspaceLayout::new(tmp.path().to_path_buf());
        let source = SourceRef {
            segment_id: "sess".into(),
            range: [0, 0],
        };
        let (_, path) = write_staging(&layout, source, ExtractedPayload::default()).unwrap();
        let written: StagingRecord =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(written.payload.is_empty());
    }
}
