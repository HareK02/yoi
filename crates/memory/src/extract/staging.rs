//! extract staging writer.
//!
//! Staging is flat: one file is one candidate and one consolidation decision
//! unit. The transitional extract tool still submits `ExtractedPayload` with
//! `candidates[]`; this writer expands it into one [`StagingRecord`] per
//! candidate.

use std::fs;
use std::io;
use std::path::PathBuf;

use uuid::Uuid;

use crate::extract::payload::{ExtractedPayload, StagingRecord};
use crate::schema::SourceRef;
use crate::workspace::WorkspaceLayout;

/// Filesystem result for a single staged candidate.
#[derive(Debug, Clone)]
pub struct StagingWriteResult {
    pub id: Uuid,
    pub path: PathBuf,
}

/// Write one flat staging JSON file per extracted candidate.
///
/// Returns an empty vector when `payload` has no candidates.
pub fn write_staging(
    layout: &WorkspaceLayout,
    source: SourceRef,
    payload: ExtractedPayload,
) -> io::Result<Vec<StagingWriteResult>> {
    if payload.candidates.is_empty() {
        return Ok(Vec::new());
    }

    let dir = layout.staging_dir();
    fs::create_dir_all(&dir)?;
    let extract_run_id = Uuid::now_v7().to_string();
    let mut written = Vec::with_capacity(payload.candidates.len());

    for candidate in payload.candidates {
        let id = Uuid::now_v7();
        let record = StagingRecord::from_candidate(
            id.to_string(),
            extract_run_id.clone(),
            source.clone(),
            candidate,
            Vec::new(),
            Vec::new(),
        );
        let path = dir.join(format!("{}.json", id));
        let bytes = serde_json::to_vec_pretty(&record).map_err(io::Error::other)?;
        fs::write(&path, bytes)?;
        written.push(StagingWriteResult { id, path });
    }

    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::extract::payload::{CandidateKind, ExtractedCandidate};

    fn layout() -> WorkspaceLayout {
        let dir = tempfile::tempdir().unwrap();
        // leak tempdir for the duration of the test process; sufficient for unit tests
        let path = dir.keep();
        WorkspaceLayout::new(path)
    }

    fn source() -> SourceRef {
        SourceRef {
            segment_id: "segment-1".into(),
            range: [1, 3],
        }
    }

    fn candidate(kind: CandidateKind, claim: &str) -> ExtractedCandidate {
        ExtractedCandidate {
            kind,
            claim: claim.into(),
            why_useful: "useful for future consolidation".into(),
            staleness: None,
            evidence_ids: Vec::new(),
        }
    }

    #[test]
    fn writes_one_file_per_candidate() {
        let layout = layout();
        let payload = ExtractedPayload {
            candidates: vec![
                candidate(CandidateKind::Preference, "Prefer implementation tickets"),
                candidate(CandidateKind::Decision, "Use flat staging records"),
            ],
        };
        let results = write_staging(&layout, source(), payload).unwrap();
        assert_eq!(results.len(), 2);
        assert_ne!(results[0].id, results[1].id);

        let first = fs::read_to_string(&results[0].path).unwrap();
        let second = fs::read_to_string(&results[1].path).unwrap();
        let first_record: StagingRecord = serde_json::from_str(&first).unwrap();
        let second_record: StagingRecord = serde_json::from_str(&second).unwrap();

        assert_eq!(first_record.kind, CandidateKind::Preference);
        assert_eq!(second_record.kind, CandidateKind::Decision);
        assert_eq!(first_record.extract_run_id, second_record.extract_run_id);
        assert_eq!(first_record.source.segment_id, "segment-1");
    }

    #[test]
    fn empty_payload_writes_nothing() {
        let layout = layout();
        let results = write_staging(&layout, source(), ExtractedPayload::default()).unwrap();
        assert!(results.is_empty());
        assert!(!layout.staging_dir().exists());
    }
}
