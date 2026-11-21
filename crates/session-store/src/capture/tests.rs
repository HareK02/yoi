use super::*;
use protocol::{RunFailureKind, SessionSnapshotEntry};

fn run_error(id: &str, message: &str) -> SessionSnapshotEntry {
    SessionSnapshotEntry {
        entry_id: id.into(),
        timestamp: 42,
        provenance: SessionEntryProvenance::BackendInstruction,
        derived_from: Vec::new(),
        use_policy: None,
        data: SessionSnapshotEntryData::RunError {
            message: message.into(),
            failure: Some(RunFailureKind::Engine),
        },
    }
}

fn snapshot(entries: Vec<SessionSnapshotEntry>) -> SessionSnapshot {
    SessionSnapshot {
        entries,
        pending_submissions: Default::default(),
    }
}

#[test]
fn snapshot_run_error_is_readable_with_stable_source_but_not_memory_or_recording() {
    let error = run_error("saved-error", "SSE overload: retry later");
    let capture =
        SessionCapture::from_session_snapshot("session", "segment", snapshot(vec![error.clone()]));
    let hits = capture
        .search_checked(&SearchOptions {
            query: "OVERLOAD".into(),
            kind: ReferenceKind::parse("run_error"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id.as_str(), "Esaved-error");
    assert_eq!(hits[0].kind.as_str(), "run_error");
    assert_eq!(hits[0].origin, error.provenance);
    assert_eq!(capture.overview()[0].id, hits[0].id);
    for kind in [
        ReferenceKind::User,
        ReferenceKind::Assistant,
        ReferenceKind::System,
        ReferenceKind::Tool,
    ] {
        assert!(
            capture
                .search(&SearchOptions {
                    kind: Some(kind),
                    ..Default::default()
                })
                .is_empty()
        );
    }
    let scoped = capture.scoped_ref(&hits[0].id).unwrap();
    let entry = capture
        .resolve_ref(&scoped, CapturePurpose::Observation)
        .unwrap();
    assert_eq!(entry.source.segment_id, "segment");
    assert_eq!(entry.source.entry_index, 0);
    assert_eq!(entry.source.provenance, error.provenance);
    assert!(capture.evidence_for(hits[0].id.as_str()).is_none());
    for purpose in [CapturePurpose::MemoryEvidence, CapturePurpose::Recording] {
        assert!(matches!(
            capture.resolve_ref(&scoped, purpose),
            Err(CaptureError::PolicyDenied)
        ));
        assert!(
            capture
                .slice(purpose, None, 10, 4096)
                .unwrap()
                .slice()
                .items
                .is_empty()
        );
    }
    let restored = SessionCapture::decode(
        &capture.encode(CaptureLimits::default()).unwrap(),
        CaptureLimits::default(),
    )
    .unwrap();
    assert_eq!(capture.to_dto(), restored.to_dto());
    let appended = SessionCapture::from_session_snapshot(
        "session",
        "segment",
        snapshot(vec![error, run_error("next-error", "other")]),
    );
    for view in [&capture, &restored, &appended] {
        for detail in [ReadDetail::Compact, ReadDetail::Full] {
            let read = view.read(
                ReadSelector::Id(hits[0].id.as_str()),
                ReadOptions {
                    detail,
                    ..Default::default()
                },
            );
            assert!(!read.truncated);
            assert_eq!(read.entries.len(), 1);
            assert_eq!(read.entries[0].kind, ReferenceKind::RunError);
            assert!(read.entries[0].text.contains("RunError"));
            assert!(read.entries[0].text.contains("Engine"));
            assert!(read.entries[0].text.contains("SSE overload: retry later"));
        }
    }
}

#[test]
fn run_error_previews_reads_and_search_pages_are_bounded() {
    let message = format!("{} late overload", "過負荷".repeat(500));
    let capture = SessionCapture::from_session_snapshot(
        "session",
        "segment",
        snapshot(
            (0..60)
                .map(|i| run_error(&format!("error-{i}"), &message))
                .collect(),
        ),
    );
    assert_eq!(capture.overview().len(), 60);
    assert!(
        capture
            .overview()
            .iter()
            .all(|entry| entry.text.chars().count() < 260)
    );
    let hits = capture.search(&SearchOptions {
        query: "late overload".into(),
        limit: Some(usize::MAX),
        ..Default::default()
    });
    assert_eq!(hits.len(), MAX_SEARCH_LIMIT);
    assert!(hits.iter().all(|hit| hit.summary.chars().count() < 260));
    assert_eq!(
        capture
            .search(&SearchOptions {
                offset: MAX_SEARCH_LIMIT,
                ..Default::default()
            })
            .len(),
        10
    );
    for detail in [ReadDetail::Compact, ReadDetail::Full] {
        let read = capture.read(
            ReadSelector::Id("Eerror-0"),
            ReadOptions {
                detail,
                max_bytes: 128,
                ..Default::default()
            },
        );
        assert!(read.truncated);
        assert!(read.entries[0].text.len() <= 128);
        assert!(read.entries[0].text.ends_with("… [truncated]"));
    }
}

#[test]
fn run_error_observation_policy_is_respected_and_evidence_policy_cannot_be_elevated() {
    let mut error = run_error("error", "hidden diagnostic");
    error.use_policy = Some(protocol::SessionEntryUsePolicy {
        observation_readable: false,
        memory_evidence: true,
    });
    let capture =
        SessionCapture::from_session_snapshot("session", "segment", snapshot(vec![error]));
    assert!(capture.overview().is_empty());
    assert!(capture.search(&SearchOptions::default()).is_empty());
    assert!(
        capture
            .read(ReadSelector::Id("Eerror"), ReadOptions::default())
            .entries
            .is_empty()
    );
    assert!(capture.evidence_for("Eerror").is_none());

    let capture = SessionCapture::from_session_snapshot(
        "session",
        "segment",
        snapshot(vec![run_error("error", "diagnostic")]),
    );
    for purpose in [CapturePurpose::MemoryEvidence, CapturePurpose::Recording] {
        let mut dto = capture.to_dto();
        match purpose {
            CapturePurpose::MemoryEvidence => dto.entries[0].policy.memory_evidence = true,
            CapturePurpose::Recording => dto.entries[0].policy.recording_material = true,
            CapturePurpose::Observation => unreachable!(),
        }
        assert!(matches!(
            SessionCapture::try_from_dto(dto, CaptureLimits::default()),
            Err(CaptureError::Invalid(_))
        ));
    }
}
