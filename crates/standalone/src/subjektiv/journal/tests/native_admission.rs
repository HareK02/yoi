//! LocalJournalHost -> service -> real native Feature transaction -> real Local authority.
//! Only the outer Job attempt owner and (where disclosed) external canonical policy owner
//! are substituted. The per-instance observer wraps, never replaces, the real authority.
//! Ordinary standalone lib tests; isolated FsStore/Feature DB/Job store, no model/network.
use super::*;
use crate::subjektiv::authority::{MutablePolicy, bind_canonical_policy};
use ::feature_permissions::{
    NamespaceId, NamespacePermissions, PermissionKey, PermissionReference,
};
use std::sync::{atomic::AtomicUsize, mpsc};
use std::time::Duration;
use worker::feature::FeatureId;

const WAIT: Duration = Duration::from_secs(5);

fn canonical_fixture() -> (Fixture, Arc<MutablePolicy>) {
    let mut source = None;
    let fixture =
        Fixture::with_policy("record only under current authority", |catalog, subject| {
            source = Some(bind_canonical_policy(catalog, subject));
        });
    (fixture, source.unwrap())
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Unchanged,
    ExcludeOperation,
    RemoveFeature,
}
impl Change {
    fn apply(self, fixture: &Fixture, source: &MutablePolicy, operation: &str) {
        match self {
            Self::Unchanged => {}
            // Revoke just the requested operation. In particular, retaining input.supply
            // must not authorize a later entry/position/checkpoint/finish operation.
            Self::ExcludeOperation => {
                source
                    .current
                    .lock()
                    .unwrap()
                    .as_mut()
                    .unwrap()
                    .execution
                    .exclusions
                    .push(PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse("journal").unwrap(),
                        keys: vec![PermissionKey::parse(operation).unwrap()],
                    }));
            }
            Self::RemoveFeature => fixture
                .host()
                .catalog
                .permissions
                .lock()
                .unwrap()
                .remove(&FeatureId::builtin("journal-recording"))
                .unwrap(),
        }
    }
    fn denied(self) -> bool {
        !matches!(self, Self::Unchanged)
    }
}

// No database calls in this observer or the controlling thread while held: native
// admission owns the Feature connection/IMMEDIATE transaction before invoking it.
fn hold_admission(
    adapter: &mut LocalJournalHost,
    operation: &'static str,
    occurrence: usize,
) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered, waiting) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let resumed = Mutex::new(resumed);
    let calls = AtomicUsize::new(0);
    adapter.admission_observer = Some(Arc::new(move |current| {
        if current == operation && calls.fetch_add(1, Ordering::AcqRel) + 1 == occurrence {
            entered
                .send(())
                .map_err(|e| subjektiv::SubjektivError::AuthorizationDenied(e.to_string()))?;
            resumed.lock().unwrap().recv_timeout(WAIT).map_err(|e| {
                subjektiv::SubjektivError::AuthorizationDenied(format!(
                    "native admission resume: {e}"
                ))
            })?;
        }
        Ok(())
    }));
    (waiting, resume)
}

fn while_admitted<T: Send>(
    waiting: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
    call: impl FnOnce() -> T + Send,
    change: impl FnOnce(),
) -> T {
    std::thread::scope(|scope| {
        let call = scope.spawn(call);
        waiting
            .recv_timeout(WAIT)
            .expect("real native admission was not reached");
        change();
        resume.send(()).unwrap();
        call.join().unwrap()
    })
}

#[derive(Debug, PartialEq, Eq)]
struct DurableState {
    journal: Journal,
    open: Option<JournalEntry>,
    operations: Vec<JournalReceipt>,
    sources: Vec<JournalSourceStatus>,
    active: Option<JournalGeneration>,
}
fn durable_state(adapter: &LocalJournalHost) -> DurableState {
    // Trusted, unbound Host reads after the held call has completed. These are not
    // a new model request and remain available to assert refusal left no effects.
    let store = &adapter.host.catalog.store;
    let host = &adapter.context;
    let id = &adapter.grant.journal_id;
    let (journal, open) = store.journal_resume_state(host, id).unwrap();
    let operations = store
        .list_journal_operations(host, id, None, MAX_JOURNAL_PAGE)
        .unwrap();
    let sources = store
        .list_journal_sources(host, id, None, MAX_JOURNAL_PAGE)
        .unwrap();
    assert!(operations.next.is_none());
    assert!(sources.next.is_none());
    DurableState {
        journal,
        open,
        operations: operations.items,
        sources: sources.items,
        active: store.active_journal_generation(host, id).unwrap(),
    }
}

fn create() -> JournalAction {
    JournalAction::Create {
        title: "Admitted entry".into(),
        body: "Retain only effects authorized at native admission".into(),
        ongoing: "unfinished".into(),
        state: JournalEntryState::Open,
        source_end: None,
        correction_refs: vec![],
    }
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Create,
    Rewrite,
    Fix,
    Position,
    ConfirmUnchanged,
    Checkpoint,
}
impl Mutation {
    const ALL: [Self; 6] = [
        Self::Create,
        Self::Rewrite,
        Self::Fix,
        Self::Position,
        Self::ConfirmUnchanged,
        Self::Checkpoint,
    ];
    fn operation(self) -> &'static str {
        match self {
            Self::Create => "journal.entry.create",
            Self::Rewrite => "journal.entry.rewrite",
            Self::Fix => "journal.entry.fix",
            Self::Position => "journal.position.update",
            Self::ConfirmUnchanged => "journal.unchanged.confirm",
            Self::Checkpoint => "journal.checkpoint",
        }
    }
    fn prepare(self, adapter: &LocalJournalHost) -> JournalOperation {
        let ranges = adapter.grant.ranges.clone();
        let mut expected = adapter.generation.initial_journal.state_token.clone();
        let action = match self {
            Self::Create => create(),
            Self::ConfirmUnchanged => JournalAction::ConfirmUnchanged,
            Self::Position => JournalAction::UpdatePosition {
                current_position: "new position".into(),
            },
            Self::Rewrite | Self::Fix => {
                let created = save(
                    adapter,
                    "prepare-create",
                    &expected,
                    create(),
                    ranges.clone(),
                );
                expected = created.state_token;
                if matches!(self, Self::Rewrite) {
                    JournalAction::Rewrite {
                        entry_id: created.entry_id.unwrap(),
                        title: "rewritten".into(),
                        body: "new body".into(),
                        ongoing: "new ongoing".into(),
                    }
                } else {
                    JournalAction::Fix {
                        entry_id: created.entry_id.unwrap(),
                        source_end: ranges.last().unwrap().source.pointer.clone(),
                    }
                }
            }
            Self::Checkpoint => {
                let record = save(
                    adapter,
                    "prepare-record",
                    &expected,
                    JournalAction::ConfirmUnchanged,
                    ranges.clone(),
                );
                let position = save(
                    adapter,
                    "prepare-position",
                    &record.state_token,
                    JournalAction::UpdatePosition {
                        current_position: "prepared position".into(),
                    },
                    vec![],
                );
                expected = position.state_token;
                JournalAction::Checkpoint {
                    record_operation_ids: vec!["prepare-record".into()],
                }
            }
        };
        JournalOperation {
            request_id: "held-operation".into(),
            expected_state: expected,
            action,
            covered_ranges: if matches!(self, Self::Position) {
                vec![]
            } else {
                ranges
            },
        }
    }
}

fn mutations(change: Change) {
    for mutation in Mutation::ALL {
        let (fixture, source) = canonical_fixture();
        let (mut adapter, _, _) = fixture.adapter();
        let operation = mutation.prepare(&adapter);
        let before = durable_state(&adapter);
        // execute_snapshot first validates the bound generation/delivery under a native
        // read admission. The NEXT admission for the same operation is execute_journal's
        // actual mutation transaction (api::execute/checkpoint_journal are direct forwards).
        // Supply has a separate authority/key and cannot satisfy this observer.
        let (waiting, resume) = hold_admission(&mut adapter, mutation.operation(), 2);
        let result = while_admitted(
            waiting,
            resume,
            || {
                adapter.execute(JournalJobRequest::Execute {
                    operation: operation.clone(),
                })
            },
            || change.apply(&fixture, &source, mutation.operation()),
        );
        let after = durable_state(&adapter);
        if change.denied() {
            assert!(
                matches!(result, Err(SubjektivHostError::PermissionDenied(_))),
                "{mutation:?}/{change:?}: {result:?}"
            );
            assert_eq!(
                after, before,
                "{mutation:?}/{change:?}: no content/position/reflection/operation receipt may commit"
            );
            if matches!(change, Change::ExcludeOperation) {
                assert!(
                    adapter
                        .host
                        .authorize_current("journal.job.input.supply")
                        .is_ok(),
                    "the delivery key remains allowed and cannot substitute for the revoked operation"
                );
            }
            assert!(
                adapter
                    .host
                    .catalog
                    .store
                    .query_journal_operation(
                        &adapter.context,
                        &adapter.grant.journal_id,
                        &operation.request_id
                    )
                    .unwrap()
                    .is_none()
            );
        } else {
            let JournalJobResponse::Saved(receipt) = result.unwrap() else {
                panic!("expected saved operation")
            };
            assert_eq!(
                after.operations.len(),
                before.operations.len() + 1,
                "{mutation:?}"
            );
            assert_eq!(after.operations.last(), Some(&receipt));
            assert_ne!(after.journal.state_token, before.journal.state_token);
            match mutation {
                Mutation::Create | Mutation::Rewrite | Mutation::Fix => {
                    assert_ne!(after.open, before.open)
                }
                Mutation::Position => assert_eq!(after.journal.current_position, "new position"),
                Mutation::ConfirmUnchanged => assert_eq!(after.sources, before.sources),
                Mutation::Checkpoint => assert!(
                    after
                        .sources
                        .iter()
                        .all(|s| s.reflected_bytes == s.source.total_bytes)
                ),
            }
        }
    }
}

#[test]
fn local_journal_native_mutations_unchanged_authority_commits_exact_receipts() {
    mutations(Change::Unchanged);
}
#[test]
fn local_journal_native_mutations_operation_revocation_has_no_effect_or_receipt() {
    mutations(Change::ExcludeOperation);
}
#[test]
fn local_journal_native_mutations_feature_removal_has_no_effect_or_receipt() {
    mutations(Change::RemoveFeature);
}

#[test]
fn local_journal_native_legacy_policy_connection_is_not_reentered() {
    // Production Local source: legacy policy is in the SAME admitted Feature DB.
    // No canonical provider substitution. A bounded observer release followed by a
    // finite outer test runner timeout detects recursive connection acquisition.
    let fixture = Fixture::new("production Local policy in admitted Feature database");
    let (mut adapter, _, _) = fixture.adapter();
    let operation = Mutation::Create.prepare(&adapter);
    let (waiting, resume) = hold_admission(&mut adapter, "journal.entry.create", 2);
    let result = while_admitted(
        waiting,
        resume,
        || adapter.execute(JournalJobRequest::Execute { operation }),
        || {},
    );
    assert!(
        matches!(result, Ok(JournalJobResponse::Saved(_))),
        "{result:?}"
    );
    assert_eq!(durable_state(&adapter).operations.len(), 1);
}

#[test]
fn local_journal_native_delivery_uses_supply_authority_and_denial_saves_no_delivery() {
    for change in [
        Change::Unchanged,
        Change::ExcludeOperation,
        Change::RemoveFeature,
    ] {
        let (fixture, source) = canonical_fixture();
        let (mut adapter, _, _) = fixture.adapter();
        let before = durable_state(&adapter);
        let operation = Mutation::Create.prepare(&adapter);
        let (waiting, resume) = hold_admission(&mut adapter, "journal.job.input.supply", 1);
        let result = while_admitted(
            waiting,
            resume,
            || adapter.execute(JournalJobRequest::Execute { operation }),
            || change.apply(&fixture, &source, "journal.job.input.supply"),
        );
        let delivered = subjektiv::journal::service::execute_snapshot(
            &adapter.host.catalog.store,
            &adapter.job_context,
            &adapter.input_digest,
            JournalJobRequest::ReadState,
        );
        if change.denied() {
            assert!(
                matches!(result, Err(SubjektivHostError::PermissionDenied(_))),
                "{change:?}: {result:?}"
            );
            assert_eq!(durable_state(&adapter), before);
            assert!(
                matches!(delivered, Err(JournalError::Conflict(ref reason)) if reason.contains("delivery")),
                "{delivered:?}"
            );
        } else {
            assert!(
                matches!(result, Ok(JournalJobResponse::Saved(_))),
                "{result:?}"
            );
            assert!(delivered.is_ok(), "{delivered:?}");
        }
    }
}

#[test]
fn local_journal_native_finish_rechecks_before_saving_completion() {
    for change in [
        Change::Unchanged,
        Change::ExcludeOperation,
        Change::RemoveFeature,
    ] {
        let (fixture, source) = canonical_fixture();
        let (mut adapter, _, _) = fixture.adapter();
        let checkpoint = Mutation::Checkpoint.prepare(&adapter);
        adapter
            .execute(JournalJobRequest::Execute {
                operation: checkpoint,
            })
            .unwrap();
        let before = durable_state(&adapter);
        assert!(before.active.is_some());
        let (waiting, resume) = hold_admission(&mut adapter, "journal.finish", 1);
        let result = while_admitted(
            waiting,
            resume,
            || adapter.finish(),
            || change.apply(&fixture, &source, "journal.finish"),
        );
        let after = durable_state(&adapter);
        if change.denied() {
            assert!(
                matches!(result, Err(SubjektivHostError::PermissionDenied(_))),
                "{change:?}: {result:?}"
            );
            assert_eq!(
                after, before,
                "completion must not be saved under stale authority"
            );
        } else {
            let completed = result.unwrap();
            assert_eq!(completed.state_token, before.journal.state_token);
            assert!(after.active.is_none());
            assert_eq!(
                adapter.finish().unwrap(),
                completed,
                "unchanged exact completion replay"
            );
        }
    }
}

#[test]
fn local_journal_native_read_rechecks_current_authority_before_response() {
    for change in [
        Change::Unchanged,
        Change::ExcludeOperation,
        Change::RemoveFeature,
    ] {
        let (fixture, source) = canonical_fixture();
        let (mut adapter, _, _) = fixture.adapter();
        let before = durable_state(&adapter);
        // First callback validates delivery/generation; second protects state contents.
        let (waiting, resume) = hold_admission(&mut adapter, "journal.state.read", 2);
        let result = while_admitted(
            waiting,
            resume,
            || adapter.execute(JournalJobRequest::ReadState),
            || change.apply(&fixture, &source, "journal.state.read"),
        );
        assert_eq!(durable_state(&adapter), before);
        if change.denied() {
            assert!(
                matches!(result, Err(SubjektivHostError::PermissionDenied(_))),
                "{change:?}: {result:?}"
            );
        } else {
            assert!(
                matches!(result, Ok(JournalJobResponse::State(_))),
                "{result:?}"
            );
        }
    }
}

#[test]
fn local_journal_native_attempt_revocation_prevents_effect_and_receipt() {
    let fixture = Fixture::new("the current attempt remains an independent limit");
    let (mut adapter, fence, _) = fixture.adapter();
    let operation = Mutation::Create.prepare(&adapter);
    let before = durable_state(&adapter);
    let (waiting, resume) = hold_admission(&mut adapter, "journal.entry.create", 2);
    let result = while_admitted(
        waiting,
        resume,
        || adapter.execute(JournalJobRequest::Execute { operation }),
        || fence.0.store(false, Ordering::Release),
    );
    assert!(
        matches!(result, Err(SubjektivHostError::PermissionDenied(_))),
        "{result:?}"
    );
    assert_eq!(durable_state(&adapter), before);
}
