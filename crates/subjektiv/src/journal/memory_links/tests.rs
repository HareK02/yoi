//! Component coverage through real FeatureStorage migrations, staging/application and Journal APIs.
//! Only extraction's Host-resolved evidence metadata and Journal source bytes/Job binding are fixture
//! inputs. No history provider or Worker is present: direct reads cannot fetch/restore either.
//! Host lifecycle/transport integrations are tested by their owning suites, not simulated here.

use super::*;
use crate::journal::*;
use crate::{MemoryChangeTarget, MemoryDraft, SubjectRole, SubjectSessionAttribution};
use feature_storage::{FeatureStorage, ScopedFeatureStorage};
use uuid::Uuid;

struct Fixture {
    store: Option<SubjektivStore>,
    workspace: Option<ScopedFeatureStorage>,
    manager: Option<FeatureStorage>,
    temp: tempfile::TempDir,
    host: JournalHostContext,
    journal_id: String,
    ranges: Vec<JournalSourceRange>,
}
impl Fixture {
    fn new() -> Self {
        Self::with_migrations(crate::MIGRATIONS)
    }
    fn with_migrations(migrations: &'static [feature_storage::FeatureMigration]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let manager = FeatureStorage::new(temp.path().join("storage"));
        let workspace = manager
            .workspace(&format!("memory-journal-test-{}", Uuid::now_v7()))
            .unwrap();
        let registration = workspace
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                migrations,
            ))
            .unwrap();
        let store = SubjektivStore::open(&workspace, &registration).unwrap();
        let subject = store
            .create_subject(SubjectRole::new("companion").unwrap())
            .unwrap();
        let attr =
            SubjectSessionAttribution::new(&subject.id, "runtime", "worker", "session").unwrap();
        store.record_session_attribution(attr.clone()).unwrap();
        let binding = store
            .claim_journal_writer(&subject.id, "runtime", "worker")
            .unwrap();
        let host = store.bind_journal_session(&binding, attr).unwrap();
        // Canonical order crosses two Segments; legacy numeric ranges below intentionally do not
        // agree with this order. Stable E identities, not display positions, are the join contract.
        let segments = [Uuid::now_v7(), Uuid::now_v7()];
        let ranges = (0..4)
            .map(|order| JournalSourceRange {
                source: JournalSource {
                    session_id: "session".into(),
                    pointer: JournalSourcePointer {
                        segment_id: segments[(order / 2) as usize],
                        entry_ref: format!("Eanchor-{order}"),
                    },
                    order,
                    content_sha256: "a".repeat(64),
                    total_bytes: 32,
                },
                bytes: JournalByteRange { start: 0, end: 32 },
            })
            .collect::<Vec<_>>();
        let journal_id = store
            .accept_journal_sources(&host, "Activity", &ranges)
            .unwrap();
        Self {
            store: Some(store),
            workspace: Some(workspace),
            manager: Some(manager),
            temp,
            host,
            journal_id,
            ranges,
        }
    }
    fn store(&self) -> &SubjektivStore {
        self.store.as_ref().unwrap()
    }
    fn reopen(&mut self) {
        self.store.take();
        self.workspace.take();
        self.manager.take().unwrap().shutdown().unwrap();
        let manager = FeatureStorage::new(self.temp.path().join("storage"));
        let workspace = manager.workspace(&self.host.scope_id).unwrap();
        let registration = SubjektivStore::register(&workspace).unwrap();
        self.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
        self.workspace = Some(workspace);
        self.manager = Some(manager);
        assert_eq!(self.store().scope_id(), self.host.scope_id);
        assert_eq!(
            self.store().session_attribution("session").unwrap(),
            self.host.attribution
        );
    }
    fn candidate(&self, id: &str, orders: &[usize]) -> SubjectStagingRecord {
        // Reuse the existing Memory suite's valid extraction fixture, including HumanInput origin.
        let mut record =
            crate::tests::candidate(&self.host.subject_id, id, self.store().workspace_id());
        let evidence = record.evidence[0].clone();
        let source = record.source_refs[0].clone();
        record.evidence = orders
            .iter()
            .map(|order| {
                let mut e = evidence.clone();
                e.id = self.ranges[*order].source.pointer.entry_ref.clone();
                e.entry_range = Some([100 + *order as u64, 100 + *order as u64]);
                e
            })
            .collect();
        record.source_refs = orders
            .iter()
            .enumerate()
            .map(|(i, order)| {
                let mut s = source.clone();
                s.session_id = Some("session".into());
                s.segment_id = Some(self.ranges[*order].source.pointer.segment_id.to_string());
                s.evidence_id = Some(record.evidence[i].id.clone());
                s.entry_range = record.evidence[i].entry_range;
                s
            })
            .collect();
        record.source.segment_id = self.ranges[0].source.pointer.segment_id.to_string();
        record
    }
    fn stage(&self, id: &str, orders: &[usize]) -> SubjectStagingRecord {
        self.store()
            .stage_candidate(self.candidate(id, orders))
            .unwrap()
    }
    fn apply(&self, record: &SubjectStagingRecord) -> MemoryJournalSelector {
        let draft = MemoryDraft::active(
            record.kind.clone(),
            record.claim.clone(),
            "Retain original evidence.",
            record.why_useful.clone(),
            "Reviewed original extraction.",
        );
        let (memory, _) = self
            .store()
            .apply_candidate(
                &self.host.subject_id,
                &record.id,
                MemoryChangeTarget::Create,
                draft,
                "adopt original extraction",
            )
            .unwrap();
        MemoryJournalSelector {
            memory_id: memory.id,
            change_id: memory.change_id,
            candidate_id: record.id.clone(),
        }
    }
    fn action(&self, id: &str, action: JournalAction) -> JournalReceipt {
        let binding = crate::api::JobAttemptBinding {
            job_id: "journal-recording".into(),
            attempt_id: "attempt".into(),
        };
        let generation = self
            .store()
            .prepare_journal_generation(
                &self.host,
                &self.journal_id,
                binding.clone(),
                self.ranges.clone(),
            )
            .unwrap();
        let context =
            JournalJobContext::validated(self.store(), &self.host, &generation.id, binding)
                .unwrap();
        let receipt = self
            .store()
            .execute_journal(
                &context,
                JournalOperation {
                    request_id: id.into(),
                    expected_state: self
                        .store()
                        .read_journal(&self.host, &self.journal_id)
                        .unwrap()
                        .state_token,
                    covered_ranges: match action {
                        JournalAction::UpdatePosition { .. } => vec![],
                        // Leave the final source pending so a subsequent Fix is a valid
                        // activity write, while the staged anchor is already checkpointed.
                        JournalAction::Checkpoint { .. } => self.ranges[..3].to_vec(),
                        _ => self.ranges.clone(),
                    },
                    action,
                },
            )
            .unwrap();
        receipt
    }
    fn fixed(&self, id: &str, end: usize, corrections: Vec<String>) -> String {
        self.action(
            id,
            JournalAction::Create {
                title: id.into(),
                body: format!("Journal context for {id}"),
                ongoing: String::new(),
                state: JournalEntryState::Fixed,
                source_end: Some(self.ranges[end].source.pointer.clone()),
                correction_refs: corrections,
            },
        )
        .entry_id
        .unwrap()
    }
    fn links(&self, selector: &MemoryJournalSelector, reconcile: bool) -> MemoryJournalLinks {
        let request = if reconcile {
            MemoryJournalRequest::Reconcile {
                selector: selector.clone(),
            }
        } else {
            MemoryJournalRequest::Links {
                selector: selector.clone(),
            }
        };
        let MemoryJournalResponse::Links(links) = self
            .store()
            .execute_memory_journal(&self.host, request)
            .unwrap()
        else {
            panic!("links response")
        };
        links
    }
    fn read(
        &self,
        selector: &MemoryJournalSelector,
        index: usize,
        entry: &str,
    ) -> MemoryJournalRead {
        let MemoryJournalResponse::Read(read) = self
            .store()
            .execute_memory_journal(
                &self.host,
                MemoryJournalRequest::Read {
                    selector: selector.clone(),
                    source_index: index,
                    journal_id: self.journal_id.clone(),
                    entry_id: entry.into(),
                    expected_token: None,
                    byte_offset: 0,
                    max_bytes: 4096,
                },
            )
            .unwrap()
        else {
            panic!("read response")
        };
        read
    }
    fn related(
        &self,
        entry: &str,
        after: Option<RelatedMemoryCursor>,
        limit: usize,
    ) -> RelatedMemories {
        let MemoryJournalResponse::Related(page) = self
            .store()
            .execute_memory_journal(
                &self.host,
                MemoryJournalRequest::Related {
                    journal_id: self.journal_id.clone(),
                    entry_id: entry.into(),
                    after,
                    limit,
                },
            )
            .unwrap()
        else {
            panic!("related response")
        };
        page
    }
    fn snapshot(&self) -> Vec<String> {
        self.store()
            .database
            .try_with_connection(|tx| {
                let mut values = Vec::new();
                for table in ["staging_records", "memory_changes", "memory_records"] {
                    let mut stmt = tx.prepare(&format!(
                        "SELECT record_json FROM {table} ORDER BY record_json"
                    ))?;
                    values.extend(
                        stmt.query_map([], |r| r.get::<_, String>(0))?
                            .collect::<Result<Vec<_>, _>>()?,
                    );
                }
                Ok::<_, JournalError>(values)
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.store.take();
        self.workspace.take();
        if let Some(manager) = self.manager.take() {
            let _ = manager.shutdown();
        }
    }
}

fn linked(source: &MemoryJournalSourceLink) -> (&str, &str, JournalTargetAvailability, bool) {
    let SourceConnection::Linked {
        journal_id,
        entry_id,
        target,
        reconsideration,
    } = &source.connection
    else {
        panic!("expected linked source: {source:?}")
    };
    (journal_id, entry_id, *target, *reconsideration)
}

#[test]
fn existing_applied_candidate_links_preserve_original_evidence_and_memory_history() {
    let mut f = Fixture::new();
    let staged = f.stage("existing", &[0]);
    let selector = f.apply(&staged);
    let before = f.snapshot();
    let entry = f.fixed("first", 1, vec![]);
    let initial = f.links(&selector, false);
    assert!(matches!(
        initial.sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::NotReconciled
        }
    ));
    let reconciled = f.links(&selector, true);
    assert_eq!(linked(&reconciled.sources[0]).1, entry);
    assert_eq!(f.snapshot(), before);
    f.reopen();
    assert_eq!(f.links(&selector, false), reconciled);
    assert_eq!(f.links(&selector, true), reconciled);
    let retained = f
        .store()
        .staging_candidate(&f.host.subject_id, &staged.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(retained).unwrap(),
        serde_json::to_value(staged).unwrap()
    );
    assert_eq!(f.snapshot(), before);
}

#[test]
fn missing_original_history_does_not_prevent_saved_link_body_read_after_reopen() {
    let mut f = Fixture::new();
    let selector = f.apply(&f.stage("saved", &[0]));
    let entry = f.fixed("first", 1, vec![]);
    f.links(&selector, true);
    // This is the durable Host availability attestation. The fixture contains no Session runtime
    // or history backend at all; no read below has an opportunity to fetch original history.
    f.store()
        .mark_journal_sources_unavailable(&f.host, &f.journal_id, &[f.ranges[0].clone()])
        .unwrap();
    f.reopen();
    let read = f.read(&selector, 0, &entry);
    assert_eq!(read.page.unwrap().text, "Journal context for first");
    assert_eq!(
        read.source.original_source,
        OriginalSourceAvailability::Missing
    );
    assert_eq!(linked(&read.source).2, JournalTargetAvailability::Available);
}

#[test]
fn original_source_metadata_failure_keeps_saved_journal_readable_and_does_not_claim_absence() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("metadata", &[0]));
    let entry = f.fixed("first", 1, vec![]);
    f.links(&selector, true);
    for (observation, expected) in [
        (None, OriginalSourceAvailability::Unknown),
        (Some(true), OriginalSourceAvailability::Unknown),
        (Some(false), OriginalSourceAvailability::Missing),
    ] {
        let mut calls = Vec::new();
        let response = f
            .store()
            .execute_memory_journal_with_source_presence(
                &f.host,
                MemoryJournalRequest::Read {
                    selector: selector.clone(),
                    source_index: 0,
                    journal_id: f.journal_id.clone(),
                    entry_id: entry.clone(),
                    expected_token: None,
                    byte_offset: 0,
                    max_bytes: 4096,
                },
                |session| {
                    calls.push(session.to_owned());
                    observation
                },
            )
            .unwrap();
        let MemoryJournalResponse::Read(read) = response else {
            panic!("read required")
        };
        assert_eq!(calls, [f.ranges[0].source.session_id.clone()]);
        assert_eq!(read.source.original_source, expected);
        assert_eq!(read.page.unwrap().text, "Journal context for first");
    }
}

#[test]
fn legacy_segment_entry_links_reuse_saved_session_for_presence_without_mutating_provenance() {
    let mut f = Fixture::new();
    let mut candidate = f.candidate("legacy-presence", &[0, 1]);
    for source in &mut candidate.source_refs {
        source.session_id = None;
    }
    let candidate = f.store().stage_candidate(candidate).unwrap();
    let selector = f.apply(&candidate);
    let entry = f.fixed("first", 1, vec![]);
    f.links(&selector, true);
    let before = f.snapshot();
    f.reopen();
    for request in [
        MemoryJournalRequest::Links {
            selector: selector.clone(),
        },
        MemoryJournalRequest::Read {
            selector: selector.clone(),
            source_index: 1,
            journal_id: f.journal_id.clone(),
            entry_id: entry.clone(),
            expected_token: None,
            byte_offset: 0,
            max_bytes: 4096,
        },
    ] {
        let mut calls = Vec::new();
        let response = f
            .store()
            .execute_memory_journal_with_source_presence(&f.host, request, |session| {
                calls.push(session.to_owned());
                Some(false)
            })
            .unwrap();
        assert_eq!(calls, ["session"]);
        match response {
            MemoryJournalResponse::Links(page) => {
                assert_eq!(page.sources.len(), 2);
                assert!(
                    page.sources
                        .iter()
                        .all(|s| s.original_source == OriginalSourceAvailability::Missing)
                );
            }
            MemoryJournalResponse::Read(read) => {
                assert_eq!(
                    read.source.original_source,
                    OriginalSourceAvailability::Missing
                );
                assert_eq!(read.page.unwrap().text, "Journal context for first");
            }
            _ => panic!("saved provenance result required"),
        }
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn multiple_segment_anchors_map_to_fixed_intervals_and_subject_crossing_is_denied() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("multi", &[0, 1, 2, 3]));
    let first = f.fixed("first", 1, vec![]);
    let second = f.fixed("second", 3, vec![]);
    let links = f.links(&selector, true);
    assert_eq!(
        links
            .sources
            .iter()
            .map(|s| linked(s).1)
            .collect::<Vec<_>>(),
        vec![
            first.as_str(),
            first.as_str(),
            second.as_str(),
            second.as_str()
        ]
    );
    let page = f.related(&first, None, 1);
    assert_eq!(page.items.len(), 1);
    let next = f.related(&first, page.next, 1);
    assert_eq!(next.items.len(), 1);
    assert!(next.next.is_none());
    assert_ne!(page.items[0].source_index, next.items[0].source_index);
    let other = f
        .store()
        .create_subject(SubjectRole::new("other").unwrap())
        .unwrap();
    let host = JournalHostContext::validated_reader(f.store(), &other.id).unwrap();
    for request in [
        MemoryJournalRequest::Links {
            selector: selector.clone(),
        },
        MemoryJournalRequest::Reconcile {
            selector: selector.clone(),
        },
        MemoryJournalRequest::Read {
            selector: selector.clone(),
            source_index: 0,
            journal_id: f.journal_id.clone(),
            entry_id: first.clone(),
            expected_token: None,
            byte_offset: 0,
            max_bytes: 100,
        },
        MemoryJournalRequest::Related {
            journal_id: f.journal_id.clone(),
            entry_id: first.clone(),
            after: None,
            limit: 1,
        },
    ] {
        assert!(f.store().execute_memory_journal(&host, request).is_err());
    }
    let mut wrong = selector.clone();
    wrong.change_id = "not-the-adopting-change".into();
    assert!(
        f.store()
            .execute_memory_journal(&f.host, MemoryJournalRequest::Reconcile { selector: wrong })
            .is_err()
    );
    assert!(
        f.store()
            .execute_memory_journal(
                &f.host,
                MemoryJournalRequest::Read {
                    selector,
                    source_index: 0,
                    journal_id: f.journal_id.clone(),
                    entry_id: second,
                    expected_token: None,
                    byte_offset: 0,
                    max_bytes: 100
                }
            )
            .is_err()
    );
}

#[test]
fn delayed_fixed_endpoint_connects_only_on_explicit_reconcile_without_using_checkpoint() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("delayed", &[2]));
    assert!(matches!(
        f.links(&selector, true).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::JournalNotFixed
        }
    ));
    let open = f
        .action(
            "open",
            JournalAction::Create {
                title: "In progress".into(),
                body: "Not final".into(),
                ongoing: "Still working".into(),
                state: JournalEntryState::Open,
                source_end: None,
                correction_refs: vec![],
            },
        )
        .entry_id
        .unwrap();
    f.action(
        "position-before-checkpoint",
        JournalAction::UpdatePosition {
            current_position: "Still working; no final endpoint is available".into(),
        },
    );
    f.action(
        "checkpoint",
        JournalAction::Checkpoint {
            record_operation_ids: vec!["open".into()],
        },
    );
    assert!(matches!(
        f.links(&selector, true).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::JournalNotFixed
        }
    ));
    assert_eq!(
        f.store()
            .list_journal_entries(&f.host, &f.journal_id, None, 32)
            .unwrap()
            .items[0]
            .state,
        JournalEntryState::Open
    );
    f.action(
        "fix",
        JournalAction::Fix {
            entry_id: open.clone(),
            source_end: f.ranges[3].source.pointer.clone(),
        },
    );
    assert!(matches!(
        f.links(&selector, false).sources[0].connection,
        SourceConnection::Unconnected { .. }
    ));
    let connected = f.links(&selector, true);
    assert_eq!(linked(&connected.sources[0]).1, open);
    assert_eq!(f.links(&selector, true), connected);
    assert_eq!(
        f.store()
            .list_journal_entries(&f.host, &f.journal_id, None, 32)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn legacy_numeric_range_is_not_guessed_and_scope_context_cannot_cross_stores() {
    let f = Fixture::new();
    let mut record =
        crate::tests::candidate(&f.host.subject_id, "legacy", f.store().workspace_id());
    record.source_refs[0].session_id = Some("session".into());
    record.source_refs[0].segment_id = Some(f.ranges[0].source.pointer.segment_id.to_string());
    record.source_refs[0].evidence_id = None;
    record.source_refs[0].entry_range = Some([0, 0]);
    let record = f.store().stage_candidate(record).unwrap();
    let selector = f.apply(&record);
    f.fixed("first", 1, vec![]);
    let before = f.snapshot();
    assert!(matches!(
        f.links(&selector, true).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::InsufficientSource
        }
    ));
    assert_eq!(before, f.snapshot());
    let other = Fixture::new();
    assert!(
        other
            .store()
            .execute_memory_journal(&f.host, MemoryJournalRequest::Links { selector })
            .is_err()
    );
}

#[test]
fn correction_identifies_reconsideration_without_mutation_and_erasure_retains_reference_identity() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("corrected", &[0]));
    let original = f.fixed("original", 1, vec![]);
    f.links(&selector, true);
    let before = f.snapshot();
    let correction = f.fixed("correction", 3, vec![original.clone()]);
    let related = f.related(&correction, None, 32);
    assert_eq!(related.items[0].selector, selector);
    assert!(related.items[0].reconsideration);
    assert!(linked(&f.links(&selector, false).sources[0]).3);
    assert_eq!(before, f.snapshot());
    f.store()
        .erase_journal_session(&f.host, "explicit-forgetting")
        .unwrap();
    let erased = f.read(&selector, 0, &original);
    assert!(erased.page.is_none());
    assert_eq!(linked(&erased.source).2, JournalTargetAvailability::Erased);
    let again = f.links(&selector, true);
    assert_eq!(linked(&again.sources[0]).1, original);
    assert_eq!(
        linked(&again.sources[0]).2,
        JournalTargetAvailability::Erased
    );
    assert_eq!(f.related(&original, None, 32).items[0].selector, selector);
    assert_eq!(before, f.snapshot());
}

#[test]
fn malformed_wire_selectors_and_page_limits_are_rejected_without_side_effects() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("wire", &[0]));
    assert!(
        serde_json::from_value::<MemoryJournalRequest>(serde_json::json!({
        "action":"reconcile","selector":selector,"subject_id":"another-subject"}))
        .is_err()
    );
    let first = f.fixed("first", 1, vec![]);
    f.links(&selector, true);
    for limit in [0, MAX_JOURNAL_PAGE + 1] {
        assert!(
            f.store()
                .execute_memory_journal(
                    &f.host,
                    MemoryJournalRequest::Related {
                        journal_id: f.journal_id.clone(),
                        entry_id: first.clone(),
                        after: None,
                        limit
                    }
                )
                .is_err()
        );
    }
    let response = f.store().execute_memory_journal(
        &f.host,
        MemoryJournalRequest::Read {
            selector,
            source_index: 0,
            journal_id: f.journal_id.clone(),
            entry_id: first,
            expected_token: Some("stale".into()),
            byte_offset: 1,
            max_bytes: 32,
        },
    );
    assert!(matches!(response, Err(JournalError::Conflict(_))));
}

#[test]
fn missing_target_never_retargets_saved_edge_to_a_later_event() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("unavailable", &[0]));
    let first = f.fixed("first", 1, vec![]);
    f.links(&selector, true);
    let later = f.fixed("later", 3, vec![]);
    // Simulate unavailable storage rather than explicit forgetting. This must remain distinct
    // from the erasure tombstone, and a subsequent reconciliation must not choose `later`.
    f.store()
        .database
        .try_transaction(|tx| {
            tx.execute("DELETE FROM journal_entries WHERE id=?1", [&first])?;
            Ok::<_, JournalError>(())
        })
        .unwrap();
    let read = f.read(&selector, 0, &first);
    assert!(read.page.is_none());
    assert_eq!(
        linked(&read.source).2,
        JournalTargetAvailability::Unavailable
    );
    let saved = f.links(&selector, true);
    assert_eq!(linked(&saved.sources[0]).1, first);
    assert_ne!(linked(&saved.sources[0]).1, later);
    assert_eq!(f.related(&first, None, 32).items[0].selector, selector);
}

#[test]
fn ambiguous_saved_anchor_does_not_choose_between_attributed_sessions() {
    let f = Fixture::new();
    f.fixed("first", 1, vec![]);
    // Two Sessions of the same current body may contain an identical saved anchor.
    // Bind both through the live writer; cross-body ownership is not part of this ambiguity.
    let attr =
        SubjectSessionAttribution::new(&f.host.subject_id, "runtime", "worker", "other-session")
            .unwrap();
    f.store().record_session_attribution(attr.clone()).unwrap();
    let binding = f
        .store()
        .claim_journal_writer(&f.host.subject_id, "runtime", "worker")
        .unwrap();
    let other_host = f.store().bind_journal_session(&binding, attr).unwrap();
    let mut range = f.ranges[0].clone();
    range.source.session_id = "other-session".into();
    f.store()
        .accept_journal_sources(&other_host, "Other", &[range])
        .unwrap();
    let mut candidate =
        crate::tests::candidate(&f.host.subject_id, "ambiguous", f.store().workspace_id());
    candidate.evidence[0].id = f.ranges[0].source.pointer.entry_ref.clone();
    candidate.source_refs[0].evidence_id = Some(candidate.evidence[0].id.clone());
    candidate.source_refs[0].segment_id = Some(f.ranges[0].source.pointer.segment_id.to_string());
    candidate.source_refs[0].session_id = None;
    let staged = f.store().stage_candidate(candidate).unwrap();
    let selector = f.apply(&staged);
    assert!(matches!(
        f.links(&selector, true).sources[0].connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::AmbiguousSource
        }
    ));
}

#[test]
fn reconcile_transaction_failure_rolls_back_all_sources_and_connected_edges_are_retained() {
    let f = Fixture::new();
    let selector = f.apply(&f.stage("atomic", &[0, 2]));
    f.fixed("first", 1, vec![]);
    f.fixed("second", 3, vec![]);
    let before = f.snapshot();
    f.store()
        .database
        .try_transaction(|tx| {
            tx.execute_batch(
                "CREATE TRIGGER reject_second_link BEFORE INSERT ON memory_journal_links
            WHEN NEW.source_index=1 BEGIN SELECT RAISE(ABORT, 'injected link fault'); END;",
            )?;
            Ok::<_, JournalError>(())
        })
        .unwrap();
    assert!(
        f.store()
            .execute_memory_journal(
                &f.host,
                MemoryJournalRequest::Reconcile {
                    selector: selector.clone()
                }
            )
            .is_err()
    );
    assert!(f.links(&selector, false).sources.iter().all(|s| matches!(
        s.connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::NotReconciled
        }
    )));
    assert_eq!(before, f.snapshot());
    f.store()
        .database
        .try_transaction(|tx| {
            tx.execute_batch("DROP TRIGGER reject_second_link")?;
            Ok::<_, JournalError>(())
        })
        .unwrap();
    let links = f.links(&selector, true);
    f.store()
        .database
        .try_with_connection(|tx| {
            assert!(
                tx.execute("UPDATE memory_journal_links SET entry_id='replacement'", [])
                    .is_err()
            );
            assert!(tx.execute("DELETE FROM memory_journal_links", []).is_err());
            Ok::<_, JournalError>(())
        })
        .unwrap();
    assert_eq!(f.links(&selector, false), links);
    assert_eq!(before, f.snapshot());
}

#[test]
fn historical_candidates_page_all_immutable_sources_without_reextracting_or_losing_late_indices() {
    let mut f = Fixture::new();
    for order in 4..12 {
        let mut range = f.ranges[3].clone();
        range.source.order = order;
        range.source.pointer.entry_ref = format!("Eanchor-{order}");
        f.ranges.push(range);
    }
    f.store()
        .accept_journal_sources(&f.host, "Activity", &f.ranges)
        .unwrap();
    let staged = f.candidate("historical-many", &(0..12).collect::<Vec<_>>());
    // Seed a historical saved record directly: new staging admission is capped at ten, but
    // reading/adopting a preserved older record is not. This does not bypass link authorization.
    f.store()
        .database
        .try_transaction(|tx| {
            tx.execute(
                "INSERT INTO staging_records(subject_id,candidate_id,kind,record_json,created_at)
            VALUES(?1,?2,'constraint',?3,?4)",
                params![
                    staged.subject_id,
                    staged.id,
                    serde_json::to_string(&staged)?,
                    staged.created_at
                ],
            )?;
            Ok::<_, JournalError>(())
        })
        .unwrap();
    let selector = f.apply(&staged);
    let before = f.snapshot();
    let entry = f.fixed("all-anchors", 11, vec![]);
    let first = f.links(&selector, true);
    assert_eq!(first.total_sources, 12);
    assert_eq!(first.sources.len(), MAX_STAGING_ANCHORS);
    assert_eq!(first.next_source_index, Some(10));
    let MemoryJournalResponse::Links(unreconciled) = f
        .store()
        .execute_memory_journal(
            &f.host,
            MemoryJournalRequest::LinksPage {
                selector: selector.clone(),
                source_offset: 10,
                limit: 2,
            },
        )
        .unwrap()
    else {
        panic!("links required")
    };
    assert!(unreconciled.sources.iter().all(|s| matches!(
        s.connection,
        SourceConnection::Unconnected {
            reason: UnconnectedReason::NotReconciled
        }
    )));
    let MemoryJournalResponse::Links(last) = f
        .store()
        .execute_memory_journal(
            &f.host,
            MemoryJournalRequest::ReconcilePage {
                selector: selector.clone(),
                source_offset: 10,
                limit: 2,
            },
        )
        .unwrap()
    else {
        panic!("links required")
    };
    assert_eq!(
        last.sources
            .iter()
            .map(|s| s.source_index)
            .collect::<Vec<_>>(),
        vec![10, 11]
    );
    assert!(last.next_source_index.is_none());
    assert_eq!(linked(&last.sources[1]).1, entry);
    assert!(f.read(&selector, 11, &entry).page.is_some());
    let page = f.related(&entry, None, 11);
    assert_eq!(page.next.as_ref().unwrap().source_index, 10);
    assert_eq!(f.related(&entry, page.next, 11).items[0].source_index, 11);
    f.reopen();
    assert_eq!(f.snapshot(), before);
    for (offset, limit) in [(0, 0), (0, MAX_STAGING_ANCHORS + 1), (13, 1)] {
        assert!(
            f.store()
                .execute_memory_journal(
                    &f.host,
                    MemoryJournalRequest::ReconcilePage {
                        selector: selector.clone(),
                        source_offset: offset,
                        limit
                    }
                )
                .is_err()
        );
    }
    let MemoryJournalResponse::Links(saved) = f
        .store()
        .execute_memory_journal(
            &f.host,
            MemoryJournalRequest::LinksPage {
                selector,
                source_offset: 10,
                limit: 2,
            },
        )
        .unwrap()
    else {
        panic!("links required")
    };
    assert_eq!(saved, last);
}

fn before_links() -> &'static [feature_storage::FeatureMigration] {
    &crate::MIGRATIONS[..crate::MIGRATIONS
        .iter()
        .position(|m| m.version() == 11)
        .unwrap()]
}

fn schema_objects(store: &SubjektivStore) -> Vec<(String, String, Option<String>)> {
    store.database.try_with_connection(|tx| {
        let mut stmt=tx.prepare("SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name")?;
        Ok::<_,JournalError>(stmt.query_map([], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?
            .collect::<Result<Vec<_>,_>>()?)
    }).unwrap()
}

#[test]
fn populated_pre_link_schema_upgrade_preserves_history_and_matches_fresh_schema() {
    let mut old = Fixture::with_migrations(before_links());
    let staged = old.stage("pre-link-candidate", &[0, 2]);
    let selector = old.apply(&staged);
    let first = old.fixed("old-first", 1, vec![]);
    old.fixed("old-second", 3, vec![]);
    let before = old.snapshot();
    let prior_entries = old
        .store()
        .list_journal_entries(&old.host, &old.journal_id, None, 32)
        .unwrap();
    assert!(
        !schema_objects(old.store())
            .iter()
            .any(|(_, name, _)| name == "memory_journal_links")
    );
    old.reopen();
    assert_eq!(old.snapshot(), before);
    assert_eq!(
        old.store()
            .list_journal_entries(&old.host, &old.journal_id, None, 32)
            .unwrap(),
        prior_entries
    );
    assert_eq!(linked(&old.links(&selector, true).sources[0]).1, first);
    assert_eq!(old.snapshot(), before);
    let fresh = Fixture::new();
    assert_eq!(schema_objects(old.store()), schema_objects(fresh.store()));
    // Verify the introduced constraints, not merely migration history numbers.
    old.store()
        .database
        .try_with_connection(|tx| {
            assert!(
                tx.execute(
                    "INSERT INTO memory_journal_links(subject_id,candidate_id,source_index,reason)
            VALUES(?1,?2,-1,'insufficient_source')",
                    params![old.host.subject_id, staged.id]
                )
                .is_err()
            );
            assert!(
                tx.execute(
                    "INSERT INTO memory_journal_links(subject_id,candidate_id,source_index,reason)
            VALUES(?1,'absent-candidate',0,'insufficient_source')",
                    [&old.host.subject_id]
                )
                .is_err()
            );
            assert!(
                tx.execute("UPDATE memory_journal_links SET entry_id='retarget'", [])
                    .is_err()
            );
            Ok::<_, JournalError>(())
        })
        .unwrap();
}

#[test]
fn oldest_supported_schema_upgrade_matches_fresh_and_preserves_staged_original_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let subject = "oldest-subject";
    let candidate = crate::tests::candidate(subject, "oldest-candidate", "oldest-scope");
    let raw = serde_json::to_string(&candidate).unwrap();
    {
        let manager = FeatureStorage::new(&root);
        let workspace = manager.workspace("oldest-scope").unwrap();
        let registration = workspace
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                &crate::MIGRATIONS[..1],
            ))
            .unwrap();
        let store = SubjektivStore::open(&workspace, &registration).unwrap();
        let timestamp = crate::now();
        let subject_json = serde_json::json!({"schema_version":crate::SUBJEKTIV_SCHEMA_VERSION,
            "id":subject,"role":"companion","state":"active","store_revision":0,
            "created_at":timestamp,"updated_at":timestamp});
        store.database.try_transaction(|tx| {
            tx.execute("INSERT INTO subjects(subject_id,role,state,store_revision,record_json,created_at,updated_at)
                VALUES(?1,'companion','active',0,?2,?3,?3)",params![subject,subject_json.to_string(),timestamp])?;
            tx.execute("INSERT INTO staging_records(subject_id,candidate_id,kind,record_json,created_at)
                VALUES(?1,?2,'constraint',?3,?4)",params![subject,candidate.id,raw,timestamp])?;
            Ok::<_,JournalError>(())
        }).unwrap();
        drop(store);
        drop(workspace);
        manager.shutdown().unwrap();
    }
    let manager = FeatureStorage::new(&root);
    let workspace = manager.workspace("oldest-scope").unwrap();
    let registration = SubjektivStore::register(&workspace).unwrap();
    let store = SubjektivStore::open(&workspace, &registration).unwrap();
    let preserved = store
        .staging_candidate(subject, &candidate.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(preserved).unwrap(),
        serde_json::to_value(candidate).unwrap()
    );
    assert_eq!(store.subject(subject).unwrap().unwrap().id, subject);
    let fresh = Fixture::new();
    assert_eq!(schema_objects(&store), schema_objects(fresh.store()));
    drop(store);
    drop(workspace);
    manager.shutdown().unwrap();
}

fn fail_after_link_schema(tx: &Transaction<'_>) -> feature_storage::Result<()> {
    super::migrate(tx)?;
    Err(feature_storage::FeatureStorageError::operation(
        "injected after link DDL",
    ))
}

#[test]
fn failed_link_migration_rolls_back_ddl_and_preserves_existing_data_for_retry() {
    static FAILED_PLAN: std::sync::LazyLock<Vec<feature_storage::FeatureMigration>> =
        std::sync::LazyLock::new(|| {
            crate::MIGRATIONS
                .iter()
                .map(|migration| {
                    if migration.version() == 11 {
                        feature_storage::FeatureMigration::new(
                            11,
                            migration.name(),
                            fail_after_link_schema,
                        )
                    } else {
                        *migration
                    }
                })
                .collect()
        });
    let mut f = Fixture::with_migrations(before_links());
    let selector = f.apply(&f.stage("rollback-candidate", &[0]));
    let entry = f.fixed("before-failed-upgrade", 1, vec![]);
    let before = f.snapshot();
    let before_schema = schema_objects(f.store());
    f.store.take();
    f.workspace.take();
    f.manager.take().unwrap().shutdown().unwrap();
    {
        let manager = FeatureStorage::new(f.temp.path().join("storage"));
        let workspace = manager.workspace(&f.host.scope_id).unwrap();
        let registration = workspace
            .register(feature_storage::FeatureRegistration::new(
                crate::SUBJEKTIV_FEATURE_ID,
                FAILED_PLAN.as_slice(),
            ))
            .unwrap();
        assert!(matches!(
            SubjektivStore::open(&workspace, &registration),
            Err(crate::SubjektivError::Storage(
                feature_storage::FeatureStorageError::MigrationFailed { version: 11, .. }
            ))
        ));
        drop(workspace);
        manager.shutdown().unwrap();
    }
    let manager = FeatureStorage::new(f.temp.path().join("storage"));
    let workspace = manager.workspace(&f.host.scope_id).unwrap();
    let registration = workspace
        .register(feature_storage::FeatureRegistration::new(
            crate::SUBJEKTIV_FEATURE_ID,
            before_links(),
        ))
        .unwrap();
    f.store = Some(SubjektivStore::open(&workspace, &registration).unwrap());
    f.workspace = Some(workspace);
    f.manager = Some(manager);
    assert_eq!(f.snapshot(), before);
    assert_eq!(schema_objects(f.store()), before_schema);
    f.reopen();
    assert_eq!(linked(&f.links(&selector, true).sources[0]).1, entry);
    assert_eq!(f.snapshot(), before);
}

#[test]
fn subject_deletion_denies_all_memory_journal_context_and_reconcile_entry_points() {
    let f = Fixture::new();
    let staged = f.stage("candidate", &[0]);
    let selector = f.apply(&staged);
    let entry = f.fixed("entry", 0, vec![]);
    f.links(&selector, true);
    f.store()
        .begin_subject_deletion(&f.host.subject_id, "delete")
        .unwrap();
    for request in [
        MemoryJournalRequest::Links {
            selector: selector.clone(),
        },
        MemoryJournalRequest::Reconcile {
            selector: selector.clone(),
        },
        MemoryJournalRequest::LinksPage {
            selector: selector.clone(),
            source_offset: 0,
            limit: 1,
        },
        MemoryJournalRequest::ReconcilePage {
            selector: selector.clone(),
            source_offset: 0,
            limit: 1,
        },
        MemoryJournalRequest::Read {
            selector,
            source_index: 0,
            journal_id: f.journal_id.clone(),
            entry_id: entry.clone(),
            expected_token: None,
            byte_offset: 0,
            max_bytes: 8,
        },
        MemoryJournalRequest::Related {
            journal_id: f.journal_id.clone(),
            entry_id: entry,
            after: None,
            limit: 1,
        },
    ] {
        assert!(matches!(
            f.store().execute_memory_journal(&f.host, request),
            Err(JournalError::Subject(
                crate::SubjektivError::SubjectNotFound(_)
            ))
        ));
    }
    f.store()
        .finish_subject_deletion(&f.host.subject_id, "delete")
        .unwrap();
    f.store()
        .database
        .try_with_connection::<_, JournalError>(|c| {
            assert_eq!(
                c.query_row("SELECT COUNT(*) FROM memory_journal_links", [], |r| r
                    .get::<_, i64>(0))?,
                0
            );
            Ok(())
        })
        .unwrap();
}
