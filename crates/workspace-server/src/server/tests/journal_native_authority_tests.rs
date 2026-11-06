//! Signed Runtime router -> Journal Job service -> native Feature SQLite admission.
//! Real request proofs, canonical config adoption, registry, Host Job/attempt,
//! writer/generation and receipt persistence are connected. The shared Journal
//! fixture substitutes Runtime execution, not authentication or authorization;
//! canonical source bytes come from its isolated real Session files. Ordinary
//! workspace-server lib tests run exact child cases with a kill/reap watchdog.
use super::*;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};
use subjektiv::journal::transport::{JournalJobRequest, JournalJobResponse};
use subjektiv::journal::*;

const WAIT: Duration = Duration::from_secs(10);
const CREATE: &str = "journal.entry.create";
// The first three admissions protect active_journal_generation,
// JournalJobContext::validated and execute_snapshot's generation/delivery read.
// The fourth enters execute_journal's actual mutation transaction. The fifth
// (success only) is execute_snapshot's final protected check. Keeping this
// explicit makes a scheduling change fail, rather than silently test a pre-read.
const CREATE_MUTATION_ADMISSION: usize = 4;

struct JournalChild(Child);
impl Drop for JournalChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn bounded(case: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let log_path = scratch.path().join("signed-journal.log");
    let log = fs::File::create(&log_path).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut child = JournalChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::{case}"),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            // Allocation root only; no environment hook controls product behavior.
            .env("TMPDIR", scratch.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
            break None;
        }
        // Poll child exit, not product completion inferred from elapsed time.
        std::thread::park_timeout(Duration::from_millis(10));
    };
    let output = fs::read_to_string(log_path).unwrap();
    assert!(
        status.is_some(),
        "signed Journal deadline ({case}):\n{output}"
    );
    assert!(status.unwrap().success(), "{case}:\n{output}");
    assert!(
        output.contains("1 passed; 0 failed"),
        "exact child must execute: {output}"
    );
}

#[test]
fn signed_journal_unchanged_authority_commits_entry_and_exact_receipt() {
    bounded("signed_journal_allowed_case");
}
#[test]
fn signed_journal_policy_revoked_after_prechecks_leaves_no_effect_or_receipt() {
    bounded("signed_journal_policy_revoked_case");
}
#[test]
fn signed_journal_feature_removed_at_native_mutation_leaves_no_effect_or_receipt() {
    bounded("signed_journal_feature_removed_case");
}
#[test]
fn signed_journal_current_authority_does_not_bypass_job_generation_or_cas() {
    bounded("signed_journal_domain_fences_case");
}

#[test]
#[ignore = "exact subprocess case run by bounded signed Journal parent"]
fn signed_journal_allowed_case() {
    run(Change::Unchanged);
}
#[test]
#[ignore = "exact subprocess case run by bounded signed Journal parent"]
fn signed_journal_policy_revoked_case() {
    run(Change::RevokePolicy);
}
#[test]
#[ignore = "exact subprocess case run by bounded signed Journal parent"]
fn signed_journal_feature_removed_case() {
    run(Change::RemoveFeature);
}
#[test]
#[ignore = "exact subprocess case run by bounded signed Journal parent"]
fn signed_journal_domain_fences_case() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(domain_fences());
    runtime.shutdown_timeout(Duration::from_secs(2));
}

#[derive(Clone, Copy)]
enum Change {
    Unchanged,
    RevokePolicy,
    RemoveFeature,
}
fn run(change: Change) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(schedule(change));
    runtime.shutdown_timeout(Duration::from_secs(2));
}

struct RemoteJournal {
    // Reuse Fixture's real owner/config-adoption helper with the dispatched
    // Journal fixture's Subject/API. Its WorkdirlessFixtureRuntime field is not
    // installed or used: execution belongs to model_supply_job_fixture.
    policy: Fixture,
    reservation: BackendJobReservation,
    worker: RuntimeWorkerRef,
}
impl RemoteJournal {
    async fn new() -> Self {
        let (dir, mut api, reservation) =
            super::super::subjektiv_journal_tests::model_supply_job_fixture(false).await;
        // Concurrent management is part of this schedule. Keep the same real
        // catalog/connection, but give its FIFO executor two finite workers so
        // the paused HTTP Job does not queue the policy owner behind itself.
        let catalog = Arc::new(
            api.config_store.as_ref().clone().with_db_executor(
                crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
                    workers: std::num::NonZeroUsize::new(2).unwrap(),
                    ..Default::default()
                })
                .unwrap(),
            ),
        );
        api.config_store = catalog.clone();
        api.store = catalog;
        let worker = reservation.attempt.worker.clone().unwrap();
        let grant = reservation
            .job
            .request
            .grants
            .subjektiv_journal
            .as_ref()
            .unwrap();
        let subject = grant.subject_id.clone();
        let source = reservation.job.request.source_worker.clone().unwrap();
        let identity = Arc::new(RuntimeIdentityMaterial::generate(&worker.runtime_id).unwrap());
        configure_runtime_request_auth(&mut api, &identity, &worker.runtime_id);
        let owner =
            seed_test_workspace_owner_token(api.store.as_ref(), "signed-journal-policy-owner")
                .await;
        let policy = Fixture {
            api,
            identity,
            runtime: WorkdirlessFixtureRuntime::default(),
            subject,
            source,
            owner,
            _dir: dir,
        };
        policy
            .adopt_policy(r#"{ sets = ["subject-default"]; }"#, "{}")
            .await;
        Self {
            policy,
            reservation,
            worker,
        }
    }
    fn grant(&self) -> &crate::backend_job::SubjektivJournalGrant {
        self.reservation
            .job
            .request
            .grants
            .subjektiv_journal
            .as_ref()
            .unwrap()
    }
    fn host(&self, store: &crate::subjektiv::SubjektivStore) -> JournalHostContext {
        subjektiv_journal::host(&self.policy.api, store, self.grant()).unwrap()
    }
    fn create(&self, request_id: &str) -> JournalOperation {
        let store = open_subjektiv_store(&self.policy.api).unwrap();
        let journal = store
            .read_journal(&self.host(&store), &self.grant().journal_id)
            .unwrap();
        JournalOperation {
            request_id: request_id.into(),
            expected_state: journal.state_token,
            action: JournalAction::Create {
                title: "signed native entry".into(),
                body: "An authenticated Job recorded public input; work remains pending.".into(),
                ongoing: "awaiting next action".into(),
                state: JournalEntryState::Open,
                source_end: None,
                correction_refs: vec![],
            },
            covered_ranges: self.grant().selected_ranges.clone(),
        }
    }
    fn signed(&self, worker: &RuntimeWorkerRef, operation: &JournalOperation) -> Request<Body> {
        runtime_source_request(
            &self.policy.identity,
            Some(&worker.worker_id),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/journal/job"),
            serde_json::to_vec(&JournalJobRequest::Execute {
                operation: operation.clone(),
            })
            .unwrap(),
        )
    }
    async fn execute(&self, operation: &JournalOperation) -> (StatusCode, Value) {
        response(&self.policy.api, self.signed(&self.worker, operation)).await
    }
    fn snapshot(&self) -> Value {
        // Reopen the real repository, without a model execution grant. These
        // trusted post-call reads do not restore revoked execution authority.
        let store = open_subjektiv_store(&self.policy.api).unwrap();
        let host = self.host(&store);
        let id = &self.grant().journal_id;
        let operations = store
            .list_journal_operations(&host, id, None, MAX_JOURNAL_PAGE)
            .unwrap();
        let sources = store
            .list_journal_sources(&host, id, None, MAX_JOURNAL_PAGE)
            .unwrap();
        let entries = store
            .list_journal_entries(&host, id, None, MAX_JOURNAL_PAGE)
            .unwrap();
        assert!(operations.next.is_none() && sources.next.is_none() && entries.next.is_none());
        let writer = JournalWriterBinding {
            subject_id: self.grant().subject_id.clone(),
            runtime_id: self.policy.source.runtime_id.clone(),
            worker_id: self.policy.source.worker_id.clone(),
            binding_id: self.grant().binding_id.clone(),
        };
        assert!(store.journal_writer_active(&writer).unwrap());
        assert!(
            store
                .journal_session_writer_matches(&writer, &self.grant().session_id)
                .unwrap()
        );
        json!({"journal": store.journal_resume_state(&host, id).unwrap(),
            "writer": writer,
            "generation": store.active_journal_generation(&host, id).unwrap(),
            "operations": operations, "sources": sources, "entries": entries})
    }
    fn assert_no_receipt(&self, request_id: &str) {
        let store = open_subjektiv_store(&self.policy.api).unwrap();
        assert!(
            store
                .query_journal_operation(&self.host(&store), &self.grant().journal_id, request_id)
                .unwrap()
                .is_none()
        );
    }
}

// Dropping the controller on panic disconnects the receiver, releasing the hook.
// Every hook also has a finite wait; the outer process contains native deadlocks.
struct Pause {
    reached: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
}
fn pause_hook(
    operation: &'static str,
    occurrence: usize,
) -> (Arc<dyn Fn(&str) + Send + Sync>, Pause, Arc<AtomicUsize>) {
    let (entered, reached) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let resumed = Mutex::new(resumed);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let hook = Arc::new(move |current: &str| {
        if current == operation && observed.fetch_add(1, Ordering::SeqCst) + 1 == occurrence {
            entered.send(()).unwrap();
            resumed
                .lock()
                .unwrap()
                .recv_timeout(WAIT)
                .expect("release signed Journal admission");
        }
    });
    (hook, Pause { reached, resume }, calls)
}

async fn schedule(change: Change) {
    let mut f = RemoteJournal::new().await;
    let operation = f.create("signed-native-create");
    let before = f.snapshot();
    let (hook, pause, counts) = if matches!(change, Change::RevokePolicy) {
        let (hook, pause, counts) = pause_hook(CREATE, 1);
        f.policy.api.journal_before_effect = Some(hook.clone());
        (hook, pause, counts)
    } else {
        let (hook, pause, counts) = pause_hook(CREATE, CREATE_MUTATION_ADMISSION);
        f.policy.api.subject_effect_admission_observer = Some(hook.clone());
        (hook, pause, counts)
    };
    let _hook = hook;
    let api = f.policy.api.clone();
    let request = f.signed(&f.worker, &operation);
    let pending = tokio::spawn(async move { response(&api, request).await });
    let resume = tokio::task::spawn_blocking(move || {
        pause
            .reached
            .recv_timeout(WAIT)
            .expect("signed request must reach scheduled authority boundary");
        pause.resume
    })
    .await
    .unwrap();
    match change {
        Change::Unchanged => {}
        Change::RevokePolicy => {
            // The Feature transaction has not begun. Real configuration adoption
            // is allowed to acquire it and commits before the request resumes.
            // Keep input.supply and all other set operations; revoke only create.
            tokio::time::timeout(WAIT, f.policy.adopt_policy(
                r#"{ sets = ["subject-default"]; exclude_namespaces = [{ namespace = "journal"; keys = ["journal.entry.create"]; }]; }"#, "{}"))
                .await.expect("canonical policy adoption before native admission");
        }
        Change::RemoveFeature => {
            // The native Feature transaction is held, before registry/Host
            // acquisition. Do not access Feature storage from this thread.
            // Registry removal is the real Feature-owner transition, not a DB
            // policy edit that would be excluded by the held Feature lock.
            f.policy
                .api
                .permission_registry
                .write()
                .unwrap()
                .remove(&worker::feature::FeatureId::builtin("journal-recording"))
                .unwrap();
        }
    }
    resume.send(()).unwrap();
    let (status, body) = tokio::time::timeout(WAIT, pending)
        .await
        .expect("signed Journal request completion")
        .unwrap();
    let admissions = counts.load(Ordering::SeqCst);
    f.policy.api.journal_before_effect = None;
    f.policy.api.subject_effect_admission_observer = None;
    if matches!(change, Change::Unchanged) {
        assert_eq!(
            admissions,
            CREATE_MUTATION_ADMISSION + 1,
            "mutation and final protected read were reached"
        );
        assert_eq!(status, StatusCode::OK, "unchanged signed request: {body}");
        let JournalJobResponse::Saved(receipt) = serde_json::from_value(body.clone()).unwrap()
        else {
            panic!("signed create must return its committed receipt: {body}");
        };
        let store = open_subjektiv_store(&f.policy.api).unwrap();
        let host = f.host(&store);
        assert_eq!(
            store
                .query_journal_operation(&host, &f.grant().journal_id, &operation.request_id)
                .unwrap(),
            Some(receipt.clone())
        );
        let entries = store
            .list_journal_entries(&host, &f.grant().journal_id, None, MAX_JOURNAL_PAGE)
            .unwrap();
        assert_eq!(entries.items.len(), 1);
        assert_ne!(f.snapshot(), before);
        let saved = f.snapshot();
        let (status, replay) = f.execute(&operation).await;
        assert_eq!(status, StatusCode::OK, "exact retry: {replay}");
        assert_eq!(replay, body);
        assert_eq!(
            f.snapshot(),
            saved,
            "exact replay must not duplicate entry/receipt or advance the cursor"
        );
    } else {
        assert_eq!(
            admissions,
            if matches!(change, Change::RevokePolicy) {
                1
            } else {
                CREATE_MUTATION_ADMISSION
            }
        );
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "live authority refusal: {body}"
        );
        assert_eq!(
            f.snapshot(),
            before,
            "refusal must preserve Journal/open entry, cursor, sources, generation and receipts"
        );
        f.assert_no_receipt(&operation.request_id);
    }
}

async fn domain_fences() {
    let f = RemoteJournal::new().await;
    let operation = f.create("signed-fenced-create");
    let before = f.snapshot();
    let (status, body) = response(&f.policy.api, f.signed(&f.policy.source, &operation)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "body is not the bound Journal Job: {body}"
    );
    assert_eq!(f.snapshot(), before);
    f.assert_no_receipt(&operation.request_id);

    let (status, body) = f.execute(&operation).await;
    assert_eq!(status, StatusCode::OK, "control create: {body}");
    let saved = f.snapshot();
    let mut stale = operation.clone();
    stale.request_id = "signed-stale-cas".into();
    stale.action = JournalAction::UpdatePosition {
        current_position: "must not persist".into(),
    };
    let (status, body) = f.execute(&stale).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "current permission must not bypass stale CAS: {body}"
    );
    f.assert_no_receipt(&stale.request_id);
    assert_eq!(f.snapshot(), saved);

    let store = open_subjektiv_store(&f.policy.api).unwrap();
    let host = f.host(&store);
    let generation = store
        .active_journal_generation(&host, &f.grant().journal_id)
        .unwrap()
        .unwrap();
    store
        .close_journal_generation(&host, &generation.id, &generation.job)
        .unwrap();
    let closed = f.snapshot();
    let next = f.create("signed-closed-generation");
    let (status, body) = f.execute(&next).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "permission cannot revive closed generation: {body}"
    );
    f.assert_no_receipt(&next.request_id);
    assert_eq!(f.snapshot(), closed);
}
