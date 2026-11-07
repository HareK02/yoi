//! Signed router -> Memory/alternate staging -> native Feature SQLite regressions.
//!
//! Authentication, canonical policy adoption, body creation, Session registration,
//! registry, Host reservations, Job dispatch/attempt/batch grants and persistence
//! are real. Remote cases reuse Fixture's WorkdirlessFixtureRuntime; Embedded
//! cases create a real embedded Worker with the shared recording execution backend.
//! The backend records execution, not an external model or Runtime network call.
//! The remote body uses the shared workspace_read profile; the Embedded body
//! uses the maintained builtin:companion creation helper. Canonical execution is
//! subject-default + memory, with empty delegation. The unused Runtime field in
//! the Embedded policy-helper wrapper is not registered as its execution Runtime.
//! Neither fixture claims filesystem/OS isolation or exercises Workdir resources.
//! Evidence anchors reuse the maintained staging fixture shape; Session registration
//! and attribution are real, but these tests do not prove retained Session bytes.
//! Source profiles do not confer Job authority: only a normally dispatched Memory
//! consolidation Job consumes DecideCandidate. No context/grant/attempt is forged.
//!
//! The pre-effect schedule uses api.memory_before_effect(operation), after real
//! authentication/prechecks and before constructing the authenticated effect store.
//! This covers replacement while a caller is suspended before native admission;
//! it is not proof of a wait in an executor's internal queue. The native schedule
//! uses subject_effect_admission_observer(operation), with Feature held but before
//! registry/Host acquisition. Rotation never holds a Host lock while waiting.
//! ReceiptStatus separately checks the native protected-read fence. A further
//! executor schedule verifies a real signature before submitting StageExplicit
//! behind an occupied single-worker DB executor; it invokes the production handler
//! with that verified context, not the router (router wiring is covered separately).
//!
//! Ordinary registered parents run exact ignored children with a 60s kill/reap
//! watchdog (the maintained Journal pattern). Product gates have finite 10s waits;
//! no sleeps, environment behavior hooks, global environment mutation or unsafe.
//! These tests require the parent-owned module registration and memory_before_effect
//! hook; this file exposes no production helper and changes no existing fixture.
use super::*;
#[path = "session_registration_binding_tests.rs"]
mod registration;
use server_api::{
    SubjektivMemoryBackendOperation as Op, SubjektivMemoryBackendRequest as MemoryRequest,
    SubjektivMemoryBackendResponse as MemoryResponse,
};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);
const STAGE: &str = "memory.staging.write";
const DECIDE: &str = "memory.candidate.decide";
const RECEIPT_READ: &str = "memory.receipt.read";

#[derive(Clone, Copy, Debug)]
enum RuntimeKind {
    Embedded,
    Remote,
}
#[derive(Clone, Copy, Debug)]
enum Change {
    Unchanged,
    Rotate,
    ReactivateSameKey,
    AbaSameTrust,
}
#[derive(Clone, Copy, Debug)]
enum Boundary {
    BeforeEffect,
    Native,
}

struct BoundedChild(Child);
impl Drop for BoundedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn bounded(case: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let log_path = scratch.path().join("signed-memory.log");
    let log = fs::File::create(&log_path).unwrap();
    let module = module_path!().split_once("::").unwrap().1;
    let mut child = BoundedChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::{case}"),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
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
        // Exit polling only, never synchronization with a product effect.
        std::thread::park_timeout(Duration::from_millis(10));
    };
    let output = fs::read_to_string(log_path).unwrap();
    assert!(
        status.is_some(),
        "signed Memory deadline ({case}):\n{output}"
    );
    assert!(status.unwrap().success(), "{case}:\n{output}");
    assert!(
        output.contains("1 passed; 0 failed"),
        "exact registered child must actually run: {output}"
    );
}

// Independently named ordinary lib tests, not an ignored-only opt-in suite.
macro_rules! matrix_case {
    ($parent:ident, $child:ident, $runtime:ident, $change:ident) => {
        #[test]
        fn $parent() {
            bounded(stringify!($child));
        }
        #[test]
        #[ignore = "exact subprocess case run by bounded signed Memory parent"]
        fn $child() {
            run(RuntimeKind::$runtime, Change::$change);
        }
    };
}
matrix_case!(
    signed_memory_embedded_unchanged_commits,
    embedded_allowed_case,
    Embedded,
    Unchanged
);
matrix_case!(
    signed_memory_remote_unchanged_commits,
    remote_allowed_case,
    Remote,
    Unchanged
);
matrix_case!(
    signed_memory_embedded_rotation_fences_original_binding,
    embedded_rotated_case,
    Embedded,
    Rotate
);
matrix_case!(
    signed_memory_remote_rotation_fences_original_binding,
    remote_rotated_case,
    Remote,
    Rotate
);
matrix_case!(
    signed_memory_embedded_same_key_reactivation_fences_original_binding,
    embedded_reactivated_case,
    Embedded,
    ReactivateSameKey
);
matrix_case!(
    signed_memory_remote_same_key_reactivation_fences_original_binding,
    remote_reactivated_case,
    Remote,
    ReactivateSameKey
);
matrix_case!(
    signed_memory_embedded_aba_same_trust_fences_original_binding,
    embedded_aba_case,
    Embedded,
    AbaSameTrust
);
matrix_case!(
    signed_memory_remote_aba_same_trust_fences_original_binding,
    remote_aba_case,
    Remote,
    AbaSameTrust
);

#[test]
fn signed_memory_receipt_read_does_not_return_protected_dto_after_rotation() {
    bounded("receipt_read_case");
}
#[test]
#[ignore = "exact subprocess case run by bounded signed Memory parent"]
fn receipt_read_case() {
    with_runtime(async {
        for kind in [RuntimeKind::Embedded, RuntimeKind::Remote] {
            let mut f = SignedMemory::new(kind).await;
            let request = f.explicit("read-control");
            f.call_ok(&f.policy.source, &request).await;
            let read = Wire::memory(Op::ReceiptStatus(
                server_api::SubjektivMemoryReceiptStatusRequest {
                    receipt_id: "read-control".into(),
                },
            ));
            let before = f.snapshot();
            for boundary in [Boundary::BeforeEffect, Boundary::Native] {
                for change in [
                    Change::Unchanged,
                    Change::Rotate,
                    Change::ReactivateSameKey,
                    Change::AbaSameTrust,
                ] {
                    let body = schedule(&mut f, None, &read, change, boundary).await;
                    assert!(matches!(
                        serde_json::from_value::<MemoryResponse>(body).unwrap(),
                        MemoryResponse::ReceiptStatus(status)
                            if status.status == server_api::SubjektivMemoryReceiptStatus::Staged
                                && status.candidate_id.as_deref() == Some("read-control")
                    ));
                    assert_eq!(
                        f.snapshot(),
                        before,
                        "a current protected read is read-only"
                    );
                }
            }
            f.policy.api.stop_backend_job_recovery().await;
        }
    });
}

fn with_runtime(future: impl std::future::Future<Output = ()>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(future);
    runtime.shutdown_timeout(Duration::from_secs(2));
}
fn run(kind: RuntimeKind, change: Change) {
    with_runtime(async {
        let mut f = SignedMemory::new(kind).await;
        executor_queue(&mut f, change).await;
        for boundary in [Boundary::BeforeEffect, Boundary::Native] {
            let id = format!("explicit-{boundary:?}");
            let request = f.explicit(&id);
            let body = schedule(&mut f, None, &request, change, boundary).await;
            let MemoryResponse::Staged(staged) = serde_json::from_value(body.clone()).unwrap()
            else {
                panic!("StageExplicit must return its committed receipt: {body}");
            };
            assert_eq!(staged.receipt_id, id);
            assert_eq!(staged.candidate_id, id);
            assert_eq!(
                staged.status,
                server_api::SubjektivMemoryReceiptStatus::Staged
            );
            f.assert_candidate(&id);
            let saved = f.snapshot();
            assert_eq!(f.call_ok(&f.policy.source, &request).await, body);
            assert_eq!(f.snapshot(), saved, "same receipt ID replay is effect-free");

            let request = f.alternate(boundary);
            let before_count = f.staging_count();
            let body = schedule(&mut f, None, &request, change, boundary).await;
            let staged: server_api::SubjektivStageCandidateResponse =
                serde_json::from_value(body).unwrap();
            f.assert_candidate(&staged.staging_id);
            assert_eq!(f.staging_count(), before_count + 1);
            // Alternate staging has a server-generated ID, not an idempotent
            // request key. Retry preserves the exact input, not an invented ID.
        }
        if matches!(kind, RuntimeKind::Embedded) {
            job_matrix(&mut f, change).await;
        }
        f.policy.api.stop_backend_job_recovery().await;
    });
}

async fn executor_queue(f: &mut SignedMemory, change: Change) {
    let executor = crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
        workers: std::num::NonZeroUsize::new(1).unwrap(),
        ..Default::default()
    })
    .unwrap();
    let catalog = Arc::new(
        f.policy
            .api
            .config_store
            .as_ref()
            .clone()
            .with_db_executor(executor.clone()),
    );
    f.policy.api.store = catalog.clone();
    f.policy.api.config_store = catalog;
    let request = f.explicit("executor-queued-receipt");
    let signed = f.signed(&f.policy.source, &request);
    let path = signed.uri().path().to_string();
    let proof = signed.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
        .to_str()
        .unwrap();
    let verified = crate::worker_source::verify_runtime_request_source_proof(
        &f.policy.api,
        proof,
        TEST_WORKSPACE_ID,
        worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
        "POST",
        &path,
        &worker_runtime::auth::request_body_digest(&request.bytes),
    )
    .await
    .unwrap();
    let context = server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: Some(server_api::ServerRuntimeSource {
            runtime_id: verified.runtime_id,
            worker_id: verified.worker_id,
            authentication_binding_id: verified.authentication_binding_id,
        }),
        origin: None,
        transport_headers: vec![],
    };
    let before = f.snapshot();
    let original_worker = f.authority_projection(&f.policy.source);
    let (entered, started) = tokio::sync::oneshot::channel();
    let (resume, release) = mpsc::channel();
    let mut occupying = Box::pin(executor.run(move || -> crate::Result<()> {
        entered.send(()).unwrap();
        release
            .recv_timeout(WAIT)
            .expect("release owned executor blocker");
        Ok(())
    }));
    assert!(futures::poll!(occupying.as_mut()).is_pending());
    tokio::time::timeout(WAIT, started).await.unwrap().unwrap();
    let mut queued = Box::pin(scoped_subjektiv_memory_backend(
        State(f.policy.api.clone()),
        AxumPath(ScopedWorkspacePath {
            workspace_id: TEST_WORKSPACE_ID.into(),
        }),
        context,
        Json(serde_json::from_slice::<MemoryRequest>(&request.bytes).unwrap()),
    ));
    // The executor has exactly one occupied worker: this poll actually enqueues
    // the verified operation, rather than merely pausing before a submission.
    assert!(futures::poll!(queued.as_mut()).is_pending());
    f.replace_trust(change);
    resume.send(()).unwrap();
    tokio::time::timeout(WAIT, occupying)
        .await
        .unwrap()
        .unwrap();
    let result = tokio::time::timeout(WAIT, queued).await.unwrap();
    assert_eq!(f.authority_projection(&f.policy.source), original_worker);
    if matches!(change, Change::Unchanged) {
        let Json(MemoryResponse::Staged(saved)) = result.unwrap() else {
            panic!("queued positive must stage")
        };
        assert_eq!(saved.receipt_id, "executor-queued-receipt");
    } else {
        let error = result.unwrap_err();
        assert!(
            matches!(error.error, Error::WorkspacePermissionDenied(_)),
            "{error:?}"
        );
        assert!(
            error
                .error
                .to_string()
                .contains("authentication binding changed"),
            "{error:?}"
        );
        assert_eq!(
            f.snapshot(),
            before,
            "queued stale request cannot consume its receipt ID"
        );
        f.call_ok(&f.policy.source, &request).await;
    }
    f.assert_candidate("executor-queued-receipt");
}

struct SignedMemory {
    policy: Fixture,
    session: String,
}
impl SignedMemory {
    async fn new(kind: RuntimeKind) -> Self {
        let policy = match kind {
            RuntimeKind::Remote => Fixture::new().await,
            RuntimeKind::Embedded => {
                let dir = tempfile::tempdir().unwrap();
                let (mut api, _execution) = test_api_with_recording_backend(dir.path()).await;
                let subject = open_subjektiv_store(&api)
                    .unwrap()
                    .create_subject(crate::subjektiv::SubjectRole::new("signed-memory").unwrap())
                    .unwrap()
                    .id;
                // Supply the exercised capabilities before normal body creation:
                // its saved execution ceiling must not be widened by later policy
                // adoption. No body/registry/attribution rows are forged.
                super::super::subjektiv_jobs_tests::grant_memory_operations(
                    &api,
                    &subject,
                    &[
                        "session.record",
                        "memory.resident.read",
                        "memory.resident.supply",
                        "journal.startup.read",
                        "journal.startup.supply",
                        "memory.staging.write",
                        "memory.receipt.read",
                        "memory.consolidation.request",
                        "memory.consolidation.supply",
                        "memory.staging.list",
                        "memory.staging.read",
                        "memory.candidate.decide",
                        "memory.query",
                        "memory.read",
                        "memory.changes.list",
                        "memory.surface.prepare",
                        "memory.surface.supply",
                        "memory.surface.publish",
                        "memory.surface.fail",
                    ],
                );
                let source =
                    super::super::subjektiv_jobs_tests::attach_memory_job_source(&api, &subject)
                        .await;
                assert_eq!(source.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
                let identity =
                    Arc::new(RuntimeIdentityMaterial::generate(&source.runtime_id).unwrap());
                configure_runtime_request_auth(&mut api, &identity, &source.runtime_id);
                let owner =
                    seed_test_workspace_owner_token(api.store.as_ref(), "signed-memory-owner")
                        .await;
                Fixture {
                    api,
                    identity,
                    runtime: WorkdirlessFixtureRuntime::default(),
                    subject,
                    source,
                    owner,
                    _dir: dir,
                }
            }
        };
        policy
            .adopt_policy(r#"{ sets = ["subject-default", "memory"]; }"#, "{}")
            .await;
        let attributions = open_subjektiv_store(&policy.api)
            .unwrap()
            .subject_session_attributions(&policy.subject)
            .unwrap();
        let session = if let Some(attribution) = attributions.first() {
            assert_eq!(attribution.runtime_id, policy.source.runtime_id);
            assert_eq!(attribution.worker_id, policy.source.worker_id);
            attribution.session_id.clone()
        } else {
            let session = session_store::new_session_id().to_string();
            let request = runtime_source_request(
                &policy.identity,
                Some(&policy.source.worker_id),
                "POST",
                &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/sessions"),
                serde_json::to_vec(&json!({"session_id":session,"create_if_missing":true}))
                    .unwrap(),
            );
            let (status, body) = tokio::time::timeout(WAIT, response(&policy.api, request))
                .await
                .unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "real signed Session startup: {body}"
            );
            session
        };
        assert!(
            policy
                .api
                .store
                .list_worker_workdir_links(TEST_WORKSPACE_ID, &policy.source)
                .unwrap()
                .is_empty()
        );
        Self { policy, session }
    }
    fn explicit(&self, id: &str) -> Wire {
        Wire::memory(Op::StageExplicit(
            serde_json::from_value(json!({
                "receipt_id": id, "session_id": self.session, "kind": "decision",
                "claim": "Preserve original authentication binding",
                "why_useful": "Do not attribute a stale request to replacement trust",
                "evidence": evidence(), "source_refs": source_refs()
            }))
            .unwrap(),
        ))
    }
    fn alternate(&self, boundary: Boundary) -> Wire {
        let request: server_api::SubjektivStageCandidateRequest = serde_json::from_value(json!({
            "session_id": self.session,
            "operation": {
                "source": { "segment_id": "signed-memory-segment", "range": [0,0] },
                "extract_run_id": format!("alternate-{boundary:?}"),
                "candidate": { "kind": "decision", "claim": "Preserve original authentication binding", "why_useful": "No stale alternate write", "staleness": null, "evidence_ids": ["E00000000"] },
                "evidence": evidence(), "source_refs": source_refs()
            }
        })).unwrap();
        Wire {
            route: "staging",
            operation: STAGE,
            bytes: serde_json::to_vec(&request).unwrap(),
        }
    }
    fn signed(&self, worker: &RuntimeWorkerRef, request: &Wire) -> Request<Body> {
        assert_eq!(worker.runtime_id, self.policy.identity.identity_id);
        runtime_source_request(
            &self.policy.identity,
            Some(&worker.worker_id),
            "POST",
            &format!("/api/w/{TEST_WORKSPACE_ID}/subjektiv/{}", request.route),
            request.bytes.clone(),
        )
    }
    async fn call_ok(&self, worker: &RuntimeWorkerRef, request: &Wire) -> Value {
        let (status, body) = tokio::time::timeout(
            WAIT,
            response(&self.policy.api, self.signed(worker, request)),
        )
        .await
        .expect("signed router completion");
        assert_eq!(status, StatusCode::OK, "current binding control: {body}");
        body
    }
    fn replace_trust(&mut self, change: Change) {
        if matches!(change, Change::Unchanged) {
            return;
        }
        let catalog = &self.policy.api.config_store;
        let runtime = &self.policy.source.runtime_id;
        let original = catalog
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, runtime)
            .unwrap()
            .unwrap();
        let mut replacement = original.clone();
        match change {
            Change::Unchanged => unreachable!(),
            Change::Rotate => {
                self.policy.identity =
                    Arc::new(RuntimeIdentityMaterial::generate(runtime).unwrap());
                replacement.public_key = self.policy.identity.public_key.clone();
                replacement.public_key_fingerprint.clear();
            }
            Change::ReactivateSameKey => {
                assert!(
                    catalog
                        .revoke_workspace_runtime_binding(
                            TEST_WORKSPACE_ID,
                            runtime,
                            TEST_CREATED_AT
                        )
                        .unwrap()
                );
            }
            Change::AbaSameTrust => {
                replacement.display_name.push_str("-temporary");
                catalog
                    .upsert_workspace_runtime_binding(replacement, true)
                    .unwrap();
                replacement = original.clone();
            }
        }
        catalog
            .upsert_workspace_runtime_binding(replacement, true)
            .unwrap();
        let current = catalog
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, runtime)
            .unwrap()
            .unwrap();
        assert_ne!(current.binding_id, original.binding_id);
        assert!(current.revoked_at.is_none());
        assert_eq!(current.public_key, self.policy.identity.public_key);
        if !matches!(change, Change::Rotate) {
            assert_eq!(
                current.public_key, original.public_key,
                "same signing key, new binding"
            );
            assert_eq!(
                current.public_key_fingerprint,
                original.public_key_fingerprint
            );
        }
        if matches!(change, Change::AbaSameTrust) {
            assert_eq!(current.display_name, original.display_name);
            assert_eq!(current.base_url, original.base_url);
            assert_eq!(current.authentication_mode, original.authentication_mode);
        }
    }
    fn assert_candidate(&self, id: &str) {
        let store = open_subjektiv_store(&self.policy.api).unwrap();
        let candidate = store
            .staging_candidate(&self.policy.subject, id)
            .unwrap()
            .unwrap();
        assert_eq!(candidate.subject_id, self.policy.subject);
        assert!(!candidate.source_refs.is_empty());
        assert!(
            candidate
                .source_refs
                .iter()
                .all(|source| source.session_id.as_deref() == Some(self.session.as_str()))
        );
    }
    fn authority_projection(
        &self,
        worker: &RuntimeWorkerRef,
    ) -> Option<crate::store::WorkerRegistryProjectionRecord> {
        self.policy
            .api
            .store
            .worker_registry_projection(TEST_WORKSPACE_ID, worker)
            .unwrap()
            .map(|mut projection| {
                // Runtime liveness observations may refresh during real checks;
                // they are not ownership, resource, role or Job binding changes.
                projection.observation = None;
                projection
            })
    }
    fn staging_count(&self) -> i64 {
        self.policy
            .api
            .feature_storage
            .open(&self.policy.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT count(*) FROM staging_records WHERE subject_id=?1",
                    [&self.policy.subject],
                    |row| row.get(0),
                )?)
            })
            .unwrap()
    }
    fn snapshot(&self) -> Vec<(String, Vec<Vec<String>>)> {
        // Trusted post-call verification reopens real storage. It is not model
        // execution authority and runs only after the held request has completed.
        self.policy
            .api
            .feature_storage
            .open(&self.policy.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                let mut snapshot = Vec::new();
                for table in [
                    "subjects",
                    "subject_session_attributions",
                    "staging_records",
                    "staging_resolutions",
                    "staging_resolution_targets",
                    "candidate_decision_receipts",
                    "memory_records",
                    "memory_changes",
                    "memory_change_candidates",
                    "memory_change_derivations",
                    "surface_generation_state",
                    "surface_generation_runs",
                    "subject_permissions",
                ] {
                    let mut query = connection
                        .prepare(&format!("SELECT * FROM {table} WHERE subject_id=?1"))?;
                    let columns = query.column_count();
                    let mut rows = query
                        .query_map([&self.policy.subject], |row| {
                            (0..columns)
                                .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                                .collect::<rusqlite::Result<Vec<_>>>()
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows.sort();
                    snapshot.push((table.to_string(), rows));
                }
                Ok(snapshot)
            })
            .unwrap()
    }
}

fn evidence() -> Value {
    // The maintained staging fixtures model host-resolved public evidence. These
    // anchors are not proof of Session-file retention or an external model run.
    json!([{"id":"E00000000","kind":"message","entry_range":[0,0],
        "origin":{"kind":"human_input"},"excerpt":"explicit decision","summary":"decision"}])
}
fn source_refs() -> Value {
    json!([{"segment_id":"signed-memory-segment","entry_range":[0,0],
        "evidence_id":"E00000000","evidence_kind":"message"}])
}
struct Wire {
    route: &'static str,
    operation: &'static str,
    bytes: Vec<u8>,
}
impl Wire {
    fn memory(operation: Op) -> Self {
        let name = match &operation {
            Op::StageExplicit(_) => STAGE,
            Op::DecideCandidate(_) => DECIDE,
            Op::ReceiptStatus(_) => RECEIPT_READ,
            _ => panic!("add an explicit admission expectation for a new operation"),
        };
        Self {
            route: "memory",
            operation: name,
            bytes: serde_json::to_vec(&MemoryRequest { operation }).unwrap(),
        }
    }
}

struct Pause {
    reached: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
}
fn pause_hook(
    operation: &'static str,
) -> (Arc<dyn Fn(&str) + Send + Sync>, Pause, Arc<AtomicUsize>) {
    let (entered, reached) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let resumed = Mutex::new(resumed);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let hook = Arc::new(move |current: &str| {
        if current == operation && observed.fetch_add(1, Ordering::SeqCst) == 0 {
            entered.send(()).unwrap();
            resumed
                .lock()
                .unwrap()
                .recv_timeout(WAIT)
                .expect("release signed Memory boundary");
        }
    });
    (hook, Pause { reached, resume }, calls)
}
async fn schedule(
    f: &mut SignedMemory,
    worker: Option<&RuntimeWorkerRef>,
    request: &Wire,
    change: Change,
    boundary: Boundary,
) -> Value {
    let worker = worker.cloned().unwrap_or_else(|| f.policy.source.clone());
    let before = f.snapshot();
    let body_owner = f.authority_projection(&f.policy.source);
    let worker_owner = f.authority_projection(&worker);
    let (hook, pause, calls) = pause_hook(request.operation);
    match boundary {
        Boundary::BeforeEffect => f.policy.api.memory_before_effect = Some(hook),
        Boundary::Native => f.policy.api.subject_effect_admission_observer = Some(hook),
    }
    // Build exactly one B1 proof, then let the *real router* authenticate it.
    // The hook is unreachable until signature verification and prechecks finish.
    let signed = f.signed(&worker, request);
    let api = f.policy.api.clone();
    let pending = tokio::spawn(async move { response(&api, signed).await });
    let resume = tokio::task::spawn_blocking(move || {
        pause
            .reached
            .recv_timeout(WAIT)
            .expect("B1 must authenticate and reach held Memory boundary");
        pause.resume
    })
    .await
    .unwrap();
    f.replace_trust(change);
    resume.send(()).unwrap();
    let (status, body) = tokio::time::timeout(WAIT, pending)
        .await
        .expect("held signed request completes")
        .unwrap();
    f.policy.api.memory_before_effect = None;
    f.policy.api.subject_effect_admission_observer = None;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "one native mutation (or pre-effect) admission expected"
    );
    assert_eq!(f.authority_projection(&f.policy.source), body_owner);
    assert_eq!(f.authority_projection(&worker), worker_owner);
    if matches!(change, Change::Unchanged) {
        assert_eq!(status, StatusCode::OK, "unchanged {boundary:?}: {body}");
        return body;
    }
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "{change:?} {boundary:?}: {body}"
    );
    assert!(
        body.to_string().contains("authentication binding changed"),
        "must refuse original B1, not unrelated policy/Job/Session authority: {body}"
    );
    assert!(
        serde_json::from_value::<MemoryResponse>(body.clone()).is_err(),
        "no protected Memory DTO/receipt: {body}"
    );
    assert!(
        serde_json::from_value::<server_api::SubjektivStageCandidateResponse>(body).is_err(),
        "no alternate candidate DTO"
    );
    assert_eq!(
        f.snapshot(),
        before,
        "refusal changes no candidate, Memory, CAS, surface, Session, policy or request receipt"
    );
    // Fresh B2 proof, same Worker, exact original operation bytes/receipt/request
    // ID and (for a revise decision) same expected_change_id. Only trust changed.
    f.call_ok(&worker, request).await
}

fn decision(candidate: &str, request_id: &str, target: Value) -> Wire {
    Wire::memory(Op::DecideCandidate(serde_json::from_value(json!({
        "candidate_id": candidate, "request_id": request_id,
        "reason": "Retain confirmed binding discipline",
        "decision": { "kind": "apply", "target": target,
            "memory": { "kind": "decision", "state": "active",
                "claim": "Keep original authenticated authority", "body_md": "Only current requests may commit.",
                "why_useful": "Prevent stale authenticated effects", "change_reason": request_id }
        }
    })).unwrap()))
}
async fn job_matrix(f: &mut SignedMemory, change: Change) {
    // Stage through the signed body route, then dispatch the maintained real Job.
    // Do not grant consolidation operations to an arbitrary Subject Worker.
    for id in ["job-create", "job-revise-before", "job-revise-native"] {
        f.call_ok(&f.policy.source, &f.explicit(id)).await;
    }
    let started = tokio::time::timeout(
        WAIT,
        start_subjektiv_staging_consolidation(
            f.policy.api.clone(),
            &f.policy.subject,
            Some(f.policy.source.clone()),
            MemoryConsolidateStagingOperation { force: true },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(started.status, "started");
    let job = f
        .policy
        .api
        .store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(&f.policy.subject),
        )
        .unwrap()
        .unwrap();
    let attempt = f
        .policy
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap();
    let worker = attempt.worker.as_ref().unwrap().clone();
    assert_eq!(worker.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
    assert_ne!(worker, f.policy.source);
    assert!(
        f.policy
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_none()
    );
    let grant = job.request.grants.subjektiv_consolidation.as_ref().unwrap();
    assert_eq!(grant.subject_id, f.policy.subject);
    for id in ["job-create", "job-revise-before", "job-revise-native"] {
        assert!(grant.candidate_ids.iter().any(|candidate| candidate == id));
    }
    let create = decision("job-create", "job-create-request", json!({"kind":"create"}));
    let before_body_attempt = f.snapshot();
    let (status, body) = tokio::time::timeout(
        WAIT,
        response(&f.policy.api, f.signed(&f.policy.source, &create)),
    )
    .await
    .unwrap();
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a body is not a consolidation Job: {body}"
    );
    assert!(serde_json::from_value::<MemoryResponse>(body).is_err());
    assert_eq!(f.snapshot(), before_body_attempt);
    let body = f.call_ok(&worker, &create).await;
    let MemoryResponse::CandidateDecided(created) = serde_json::from_value(body).unwrap() else {
        panic!("a permitted real Job must apply Memory");
    };
    let memory_id = created.memory.unwrap().memory_id;
    for (boundary, candidate) in [
        (Boundary::BeforeEffect, "job-revise-before"),
        (Boundary::Native, "job-revise-native"),
    ] {
        let store = open_subjektiv_store(&f.policy.api).unwrap();
        let previous = store
            .memory(&f.policy.subject, &memory_id)
            .unwrap()
            .unwrap();
        let request_id = format!("{candidate}-request");
        let request = decision(
            candidate,
            &request_id,
            json!({
                "kind":"revise", "memory_id":memory_id, "expected_change_id":previous.change_id
            }),
        );
        let body = schedule(f, Some(&worker), &request, change, boundary).await;
        let MemoryResponse::CandidateDecided(receipt) =
            serde_json::from_value(body.clone()).unwrap()
        else {
            panic!("current permitted Job must return committed decision: {body}");
        };
        assert_eq!(receipt.request_id, request_id);
        assert_eq!(receipt.candidate_id, candidate);
        let reference = receipt.memory.unwrap();
        assert_eq!(reference.memory_id, memory_id);
        assert_ne!(reference.change_id, previous.change_id);
        assert_eq!(
            store
                .memory(&f.policy.subject, &memory_id)
                .unwrap()
                .unwrap()
                .change_id,
            reference.change_id
        );
        assert!(
            store
                .staging_resolution(&f.policy.subject, candidate)
                .unwrap()
                .is_some()
        );
        let saved = f.snapshot();
        assert_eq!(
            f.call_ok(&worker, &request).await,
            body,
            "exact Job request ID replay returns saved receipt"
        );
        assert_eq!(
            f.snapshot(),
            saved,
            "Job replay cannot append a second Memory change"
        );
        let current_job = f
            .policy
            .api
            .store
            .get_backend_job(TEST_WORKSPACE_ID, &job.request.job_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            current_job.request, job.request,
            "binding replacement must not change the Job grant/input"
        );
        let current_attempt = f
            .policy
            .api
            .store
            .get_backend_job_attempt(TEST_WORKSPACE_ID, &job.request.job_id, &attempt.attempt_id)
            .unwrap()
            .unwrap();
        assert_eq!(current_attempt.worker, attempt.worker);
        assert_eq!(current_attempt.state, attempt.state);
        assert_eq!(current_attempt.input_digest, attempt.input_digest);
        assert_eq!(current_attempt.runtime_run_id, attempt.runtime_run_id);
    }
}
