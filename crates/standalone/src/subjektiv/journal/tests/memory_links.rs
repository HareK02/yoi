//! Connected standalone Host -> Subjektiv Memory/Journal -> real FsStore coverage.
//! Candidate and Journal decisions are explicit test inputs; no model, Runtime or live DB
//! is used. The parent Journal fixture owns isolated temporary storage and binding cleanup.
//! Selected by the ordinary standalone lib tests, with no additional feature or service.

use super::*;
use memory::{
    backend::MemoryStageCandidateOperation,
    extract::{CandidateKind, ExtractedCandidate, StagingEvidence},
    schema::{EvidenceKind, EvidenceOriginKind, SourceEvidenceRef, SourceRef},
};
use subjektiv::{
    CandidateDecision, CandidateDecisionRequest, MemoryChangeTarget, MemoryDraft, MemoryRecord,
    StagingResolution, SubjectStagingRecord,
    journal::memory_links::{
        JournalTargetAvailability, MemoryJournalLinks, MemoryJournalRequest, MemoryJournalResponse,
        MemoryJournalSelector, SourceConnection, UnconnectedReason,
    },
};

fn adopt_original(fixture: &Fixture) -> (MemoryRecord, SubjectStagingRecord, StagingResolution) {
    let evidence_index = session_store::public_index::read_fs_session_evidence_index(
        fixture.sessions.root_dir(),
        fixture.context.session_id.parse().unwrap(),
        session_store::SessionPublicIndexLimits {
            max_bytes: 8 * 1024 * 1024,
            max_entries: 20_000,
            max_segments: 64,
        },
    )
    .unwrap();
    let range = *evidence_index
        .ranges
        .get(&(fixture.segment.to_string(), "Efirst".into()))
        .unwrap()
        .first()
        .unwrap();
    let staged = fixture
        .host()
        .stage_candidate(
            &fixture.context,
            MemoryStageCandidateOperation {
                source: SourceRef {
                    segment_id: fixture.segment.to_string(),
                    range,
                },
                extract_run_id: "explicit-test-extraction-input".into(),
                candidate: ExtractedCandidate {
                    kind: CandidateKind::Preference,
                    claim: "Keep correction history".into(),
                    why_useful: "Preserves the reason for later decisions".into(),
                    staleness: None,
                    evidence_ids: vec!["Efirst".into()],
                },
                evidence: vec![StagingEvidence {
                    id: "Efirst".into(),
                    kind: EvidenceKind::new(EvidenceKind::MESSAGE),
                    entry_range: Some(range),
                    origin: None,
                    excerpt: None,
                    summary: None,
                }],
                source_refs: vec![SourceEvidenceRef {
                    session_id: Some(fixture.context.session_id.clone()),
                    segment_id: Some(fixture.segment.to_string()),
                    entry_range: Some(range),
                    evidence_id: Some("Efirst".into()),
                    ..Default::default()
                }],
            },
        )
        .unwrap();
    let store = &fixture.host().catalog.store;
    let subject = &fixture.connection.binding.subject_id;
    let candidate = store
        .staging_candidate(subject, &staged.staging_id)
        .unwrap()
        .unwrap();
    let receipt = store
        .decide_candidate(
            subject,
            CandidateDecisionRequest {
                request_id: "explicit-test-adoption-input".into(),
                candidate_id: staged.staging_id,
                reason: "Adopt the committed original evidence, not the Journal summary".into(),
                decision: CandidateDecision::Apply {
                    target: MemoryChangeTarget::Create,
                    draft: MemoryDraft::active(
                        CandidateKind::Preference,
                        "Keep correction history",
                        "Preserve earlier decisions and their corrections",
                        "Preserves the reason for later decisions",
                        "Adopt original HumanInput",
                    ),
                },
            },
        )
        .unwrap();
    (receipt.memory.unwrap(), candidate, receipt.resolution)
}

fn create_entry(adapter: &LocalJournalHost, state: JournalEntryState) -> JournalReceipt {
    save(
        adapter,
        "record-original-context",
        &adapter.generation.initial_journal.state_token,
        JournalAction::Create {
            title: "A proposal was corrected".into(),
            body: "Proposal A changed to B; only part was performed, outcome unconfirmed.".into(),
            ongoing: if state == JournalEntryState::Open {
                "Remaining work is not complete".into()
            } else {
                String::new()
            },
            state,
            source_end: (state == JournalEntryState::Fixed).then(|| {
                adapter
                    .generation
                    .selected_ranges
                    .last()
                    .unwrap()
                    .source
                    .pointer
                    .clone()
            }),
            correction_refs: vec![],
        },
        adapter.generation.selected_ranges.clone(),
    )
}

fn assert_originals_unchanged(
    fixture: &Fixture,
    memory: &MemoryRecord,
    candidate: &SubjectStagingRecord,
    resolution: &StagingResolution,
) {
    let catalog = StandaloneSubjects::open_existing(&fixture.host().catalog.state_root).unwrap();
    let store = &catalog.store;
    let subject = &fixture.connection.binding.subject_id;
    assert_eq!(
        serde_json::to_value(store.memory(subject, &memory.id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(memory).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(
            store
                .staging_candidate(subject, &candidate.id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(candidate).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(
            store
                .staging_resolution(subject, &candidate.id)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(resolution).unwrap(),
    );
    assert_eq!(store.list_memories(subject).unwrap().len(), 1);
}

fn selector(memory: &MemoryRecord, candidate: &SubjectStagingRecord) -> MemoryJournalSelector {
    MemoryJournalSelector {
        memory_id: memory.id.clone(),
        change_id: memory.change_id.clone(),
        candidate_id: candidate.id.clone(),
    }
}

fn links(
    fixture: &Fixture,
    selector: &MemoryJournalSelector,
    reconcile: bool,
) -> MemoryJournalLinks {
    let request = if reconcile {
        MemoryJournalRequest::Reconcile {
            selector: selector.clone(),
        }
    } else {
        MemoryJournalRequest::Links {
            selector: selector.clone(),
        }
    };
    let MemoryJournalResponse::Links(links) = fixture
        .host()
        .memory_journal(&fixture.context, request)
        .unwrap()
    else {
        panic!("expected Memory Journal links")
    };
    links
}

fn read_request(
    selector: &MemoryJournalSelector,
    journal: &str,
    entry: &str,
) -> MemoryJournalRequest {
    MemoryJournalRequest::Read {
        selector: selector.clone(),
        source_index: 0,
        journal_id: journal.into(),
        entry_id: entry.into(),
        expected_token: None,
        byte_offset: 0,
        max_bytes: MAX_JOURNAL_READ_BYTES,
    }
}

fn assert_link(links: &MemoryJournalLinks, journal: &str, entry: &str, reconsideration: bool) {
    assert_eq!(links.sources.len(), 1);
    assert_eq!(links.sources[0].source_index, 0);
    assert_eq!(
        links.sources[0].connection,
        SourceConnection::Linked {
            journal_id: journal.into(),
            entry_id: entry.into(),
            target: JournalTargetAvailability::Available,
            reconsideration,
        },
    );
}

#[test]
fn saved_link_reads_after_original_worker_and_session_deletion_from_replacement_host() {
    let mut fixture = Fixture::new("Please preserve the reasons for my corrected decisions");
    let (memory, candidate, resolution) = adopt_original(&fixture);
    let selector = selector(&memory, &candidate);
    assert_eq!(
        candidate.evidence[0].origin.as_ref().unwrap().kind,
        EvidenceOriginKind::HumanInput
    );
    assert_eq!(
        candidate.source_refs[0].origin,
        candidate.evidence[0].origin
    );
    assert!(
        candidate.evidence[0]
            .excerpt
            .as_ref()
            .unwrap()
            .contains("corrected decisions")
    );
    let (adapter, _, _) = fixture.adapter();
    let recorded = create_entry(&adapter, JournalEntryState::Fixed);
    let entry = recorded.entry_id.clone().unwrap();
    let journal = adapter.grant.journal_id.clone();
    let connected = links(&fixture, &selector, true);
    assert_link(&connected, &journal, &entry, false);
    let request = read_request(&selector, &journal, &entry);
    let before = fixture
        .host()
        .memory_journal(&fixture.context, request.clone())
        .unwrap();
    let positioned = save(
        &adapter,
        "position-original",
        &recorded.state_token,
        JournalAction::UpdatePosition {
            current_position: "Recorded correction; remaining outcome unconfirmed".into(),
        },
        vec![],
    );
    save(
        &adapter,
        "checkpoint-original",
        &positioned.state_token,
        JournalAction::Checkpoint {
            record_operation_ids: vec![recorded.request_id],
        },
        adapter.generation.selected_ranges.clone(),
    );
    adapter.finish().unwrap();
    fixture
        .host()
        .catalog
        .store
        .close_journal_generation(
            &adapter.context,
            &adapter.generation.id,
            &adapter.generation.job,
        )
        .unwrap();
    let state = fixture.host().catalog.state_root.clone();
    let subject = fixture.connection.binding.subject_id.clone();
    let original_session = fixture.context.session_id.clone();
    // This Host/storage suite has no Controller. Complete the explicit persisted
    // writer-release boundary before deletion; the sibling handoff suite exercises
    // the production Idle reservation + Host lifecycle that authorizes that release.
    fixture
        .host()
        .catalog
        .store
        .release_journal_writer(&fixture.host().writer)
        .unwrap();
    let (worker, workers) = fixture.stop_worker();
    workers.delete(worker).unwrap();
    assert!(matches!(
        workers.load(worker),
        Err(crate::store::StandaloneStoreError::WorkerNotFound(_))
    ));
    assert!(!fixture.sessions.root_dir().join(&original_session).exists());
    assert!(
        !retained_workers_root(&state)
            .join(worker.to_string())
            .exists()
    );

    let replacement_worker = protocol::WorkerId::now_v7();
    let mut replacement = StandaloneSubjects::open_existing(&state)
        .unwrap()
        .connect(&subject, replacement_worker)
        .unwrap();
    let replacement_context = SubjektivHostContext {
        worker_id: replacement_worker.to_string(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let MemoryJournalResponse::Links(saved) = replacement
        .host
        .memory_journal(
            &replacement_context,
            MemoryJournalRequest::Links {
                selector: selector.clone(),
            },
        )
        .unwrap()
    else {
        panic!("expected saved links")
    };
    assert_link(&saved, &journal, &entry, false);
    let after = replacement
        .host
        .memory_journal(&replacement_context, request)
        .unwrap();
    let MemoryJournalResponse::Read(read) = &after else {
        panic!("expected saved Journal text")
    };
    assert!(
        read.page
            .as_ref()
            .unwrap()
            .text
            .contains("outcome unconfirmed")
    );
    assert_eq!(
        read.source.original_source,
        subjektiv::journal::memory_links::OriginalSourceAvailability::Missing
    );
    // Reading the saved edge does not depend on a live Session fetch or a replacement Job.
    let MemoryJournalResponse::Read(before) = before else {
        panic!("expected Journal text")
    };
    assert_eq!(before.page, read.page);
    assert!(
        replacement.host.jobs().is_err(),
        "the reader has no model/Job executor"
    );
    assert_originals_unchanged(&fixture, &memory, &candidate, &resolution);
    replacement.close().unwrap();
}

#[test]
fn late_journal_fix_connects_existing_memory_without_using_checkpoint_as_event_end() {
    let fixture = Fixture::new("Keep the reasons behind corrected decisions");
    let (memory, candidate, resolution) = adopt_original(&fixture);
    let selector = selector(&memory, &candidate);
    assert_eq!(
        links(&fixture, &selector, false).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::NotReconciled
        }
    );
    assert_eq!(
        links(&fixture, &selector, true).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::SourceNotAdmitted
        }
    );
    let (adapter, _, _) = fixture.adapter();
    let recorded = create_entry(&adapter, JournalEntryState::Open);
    let entry = recorded.entry_id.clone().unwrap();
    let positioned = save(
        &adapter,
        "position-open",
        &recorded.state_token,
        JournalAction::UpdatePosition {
            current_position: "Work remains open; no fixed endpoint yet".into(),
        },
        vec![],
    );
    // Checkpoint a proper prefix, leaving source bytes for the later Fix.
    // A fully checkpointed generation cannot admit another activity write.
    let mut reflected = adapter.generation.selected_ranges.clone();
    reflected.last_mut().unwrap().bytes.end -= 1;
    let checkpoint = save(
        &adapter,
        "checkpoint-open",
        &positioned.state_token,
        JournalAction::Checkpoint {
            record_operation_ids: vec![recorded.request_id],
        },
        reflected.clone(),
    );
    let unconnected = links(&fixture, &selector, true);
    assert_eq!(
        unconnected.sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::JournalNotFixed
        }
    );
    assert_eq!(links(&fixture, &selector, true), unconnected);
    let state = adapter.state().unwrap();
    let open = state.current_open.unwrap();
    assert_eq!(open.id, entry);
    assert_eq!(open.state, JournalEntryState::Open);
    assert!(open.source_end.is_none());
    let last = reflected.last().unwrap();
    assert_eq!(state.current_journal.process_order, last.source.order);
    assert_eq!(state.current_journal.process_byte, last.bytes.end);
    save(
        &adapter,
        "fix-later",
        &checkpoint.state_token,
        JournalAction::Fix {
            entry_id: entry.clone(),
            source_end: adapter
                .generation
                .selected_ranges
                .last()
                .unwrap()
                .source
                .pointer
                .clone(),
        },
        adapter.generation.selected_ranges.clone(),
    );
    // A plain saved-state read must not silently perform reconciliation.
    assert_eq!(links(&fixture, &selector, false), unconnected);
    let connected = links(&fixture, &selector, true);
    assert_link(&connected, &adapter.grant.journal_id, &entry, false);
    assert_eq!(links(&fixture, &selector, true), connected);
    assert_eq!(links(&fixture, &selector, false), connected);
    assert_originals_unchanged(&fixture, &memory, &candidate, &resolution);
}

#[test]
fn host_rejects_foreign_subject_worker_context_and_closed_connection_for_all_link_operations() {
    let mut fixture = Fixture::new("Keep correction history");
    let (memory, candidate, resolution) = adopt_original(&fixture);
    let selector = selector(&memory, &candidate);
    let (adapter, _, _) = fixture.adapter();
    let recorded = create_entry(&adapter, JournalEntryState::Fixed);
    let journal = adapter.grant.journal_id.clone();
    let entry = recorded.entry_id.unwrap();
    assert_link(&links(&fixture, &selector, true), &journal, &entry, false);
    let requests = [
        MemoryJournalRequest::Reconcile {
            selector: selector.clone(),
        },
        MemoryJournalRequest::Links {
            selector: selector.clone(),
        },
        read_request(&selector, &journal, &entry),
        MemoryJournalRequest::Related {
            journal_id: journal,
            entry_id: entry,
            after: None,
            limit: 32,
        },
    ];
    let catalog = StandaloneSubjects::open_existing(&fixture.host().catalog.state_root).unwrap();
    let foreign_subject = catalog.create("different-reader", None).unwrap();
    let foreign_worker = protocol::WorkerId::now_v7();
    let mut foreign = catalog
        .connect(&foreign_subject.id, foreign_worker)
        .unwrap();
    let foreign_context = SubjektivHostContext {
        worker_id: foreign_worker.to_string(),
        session_id: session_store::SessionId::now_v7().to_string(),
    };
    let wrong_worker = SubjektivHostContext {
        worker_id: foreign_worker.to_string(),
        ..fixture.context.clone()
    };
    for request in &requests {
        assert!(
            foreign
                .host
                .memory_journal(&foreign_context, request.clone())
                .is_err()
        );
        assert!(
            fixture
                .host()
                .memory_journal(&wrong_worker, request.clone())
                .is_err()
        );
    }
    foreign.close().unwrap();
    fixture.connection.close().unwrap();
    for request in requests {
        assert!(
            fixture
                .host()
                .memory_journal(&fixture.context, request)
                .is_err()
        );
    }
    assert_originals_unchanged(&fixture, &memory, &candidate, &resolution);
}

#[test]
fn journal_correction_finds_related_memory_without_revising_or_retargeting_its_evidence() {
    let fixture = Fixture::new("Keep correction history");
    let (memory, candidate, resolution) = adopt_original(&fixture);
    let selector = selector(&memory, &candidate);
    fixture.append("correction", "The earlier result was not confirmed");
    let (adapter, _, _) = fixture.adapter();
    let ranges = &adapter.generation.selected_ranges;
    assert_eq!(ranges.len(), 2);
    let original = save(
        &adapter,
        "original-event",
        &adapter.generation.initial_journal.state_token,
        JournalAction::Create {
            title: "Original context".into(),
            body: "A change was proposed; execution remained uncertain".into(),
            ongoing: String::new(),
            state: JournalEntryState::Fixed,
            source_end: Some(ranges[0].source.pointer.clone()),
            correction_refs: vec![],
        },
        vec![ranges[0].clone()],
    );
    let entry = original.entry_id.unwrap();
    let journal = &adapter.grant.journal_id;
    assert_link(&links(&fixture, &selector, true), journal, &entry, false);
    let corrected = save(
        &adapter,
        "correction-event",
        &original.state_token,
        JournalAction::Create {
            title: "Correction".into(),
            body: "The earlier result was unconfirmed; reconsider its associated Memory".into(),
            ongoing: String::new(),
            state: JournalEntryState::Fixed,
            source_end: Some(ranges[1].source.pointer.clone()),
            correction_refs: vec![entry.clone()],
        },
        vec![ranges[1].clone()],
    );
    assert_link(&links(&fixture, &selector, false), journal, &entry, true);
    let correction_entry = corrected.entry_id.unwrap();
    for target in [&entry, &correction_entry] {
        let MemoryJournalResponse::Related(related) = fixture
            .host()
            .memory_journal(
                &fixture.context,
                MemoryJournalRequest::Related {
                    journal_id: journal.clone(),
                    entry_id: target.clone(),
                    after: None,
                    limit: 32,
                },
            )
            .unwrap()
        else {
            panic!("expected related Memory")
        };
        assert_eq!(related.items.len(), 1);
        assert_eq!(related.items[0].selector, selector);
        assert_eq!(related.items[0].source_index, 0);
        assert!(related.items[0].reconsideration);
        assert!(related.next.is_none());
    }
    let MemoryJournalResponse::Read(read) = fixture
        .host()
        .memory_journal(&fixture.context, read_request(&selector, journal, &entry))
        .unwrap()
    else {
        panic!("expected original context")
    };
    assert!(
        read.page
            .unwrap()
            .text
            .contains("execution remained uncertain")
    );
    assert_originals_unchanged(&fixture, &memory, &candidate, &resolution);
}
