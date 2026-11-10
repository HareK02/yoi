//! Signed request verifier/router -> production chat -> native Feature SQLite regressions.
//! Normal workspace-server lib tests (`chat_tests::signed_binding` filter).
//! Real policy adoption, current body/singleton ownership and persisted chat/CAS
//! are connected. Remote uses the maintained WorkdirlessFixtureRuntime; Embedded
//! uses normal body creation with the recording execution backend, not a model.
//! Its unused Fixture Runtime field is not registered as the Embedded Runtime.
//! Ordinary lib tests run exact child cases with a kill/reap watchdog. No live
//! external Runtime, model, Workdir isolation or executed fault probe is claimed.
//!
//! The native schedule requires the parent-owned per-invocation
//! `chat::worker_operation_before_host` hook: same arguments as before_feature,
//! but its observer runs INSIDE with_chat_transaction, after the IMMEDIATE
//! transaction exists and BEFORE registry/Host acquisition. A preflight pause
//! cannot substitute for that observer. Production authorization is not replaced.
use super::*;
use server_api::{
    ChatWorkerRequest, ChatWorkerResponse, ServerRequestContext, ServerRuntimeSource,
};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);
const PROTECTED: &str = "protected chat body: original signed binding";

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
    ExecutorQueue,
    Lifecycle,
    BeforeFeature,
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
    let log_path = scratch.path().join("signed-chat.log");
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
            // Child allocation root only; no product behavior/environment hook.
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
        // Only poll child exit, never use time to infer a product effect.
        std::thread::park_timeout(Duration::from_millis(10));
    };
    let output = fs::read_to_string(log_path).unwrap();
    assert!(status.is_some(), "signed chat deadline ({case}):\n{output}");
    assert!(status.unwrap().success(), "{case}:\n{output}");
    assert!(
        output.contains("1 passed; 0 failed"),
        "exact registered child must actually execute: {output}"
    );
}

macro_rules! binding_case {
    ($parent:ident, $child:ident, $runtime:ident, $change:ident) => {
        #[test]
        fn $parent() {
            bounded(stringify!($child));
        }
        #[test]
        #[ignore = "exact subprocess case run by bounded signed chat parent"]
        fn $child() {
            run(RuntimeKind::$runtime, Change::$change);
        }
    };
}
binding_case!(
    signed_chat_embedded_unchanged_binding_reads_and_sends,
    embedded_unchanged_case,
    Embedded,
    Unchanged
);
binding_case!(
    signed_chat_remote_unchanged_binding_reads_and_sends,
    remote_unchanged_case,
    Remote,
    Unchanged
);
binding_case!(
    signed_chat_embedded_rotation_denies_held_send_and_read,
    embedded_rotation_case,
    Embedded,
    Rotate
);
binding_case!(
    signed_chat_remote_rotation_denies_held_send_and_read,
    remote_rotation_case,
    Remote,
    Rotate
);
binding_case!(
    signed_chat_embedded_same_key_reactivation_denies_held_send_and_read,
    embedded_reactivation_case,
    Embedded,
    ReactivateSameKey
);
binding_case!(
    signed_chat_remote_same_key_reactivation_denies_held_send_and_read,
    remote_reactivation_case,
    Remote,
    ReactivateSameKey
);
binding_case!(
    signed_chat_embedded_binding_aba_denies_held_send_and_read,
    embedded_aba_case,
    Embedded,
    AbaSameTrust
);
binding_case!(
    signed_chat_remote_binding_aba_denies_held_send_and_read,
    remote_aba_case,
    Remote,
    AbaSameTrust
);

fn run(kind: RuntimeKind, change: Change) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let mut f = SignedChat::new(kind).await;
        for boundary in [
            Boundary::ExecutorQueue,
            Boundary::Lifecycle,
            Boundary::BeforeFeature,
            Boundary::Native,
        ] {
            for is_read in [false, true] {
                exercise(&mut f, change, boundary, is_read).await;
            }
        }
        f.policy.api.stop_backend_job_recovery().await;
    });
    runtime.shutdown_timeout(Duration::from_secs(2));
}

struct SignedChat {
    policy: Fixture,
}
impl SignedChat {
    async fn new(kind: RuntimeKind) -> Self {
        let mut policy = match kind {
            RuntimeKind::Remote => Fixture::new().await,
            RuntimeKind::Embedded => {
                let dir = tempfile::tempdir().unwrap();
                let (mut api, _execution) = test_api_with_recording_backend(dir.path()).await;
                let subject = open_subjektiv_store(&api)
                    .unwrap()
                    .create_subject(crate::subjektiv::SubjectRole::new("signed-chat").unwrap())
                    .unwrap()
                    .id;
                // Normal body creation must capture a sufficient execution ceiling.
                // These startup operations match the maintained Embedded fixture;
                // no registry, singleton, Session or authenticated context is forged.
                super::super::super::subjektiv_jobs_tests::grant_memory_operations(
                    &api,
                    &subject,
                    &[
                        "session.record",
                        "memory.resident.read",
                        "memory.resident.supply",
                        "journal.startup.read",
                        "journal.startup.supply",
                        "chat.read",
                        "chat.send",
                    ],
                );
                let source = super::super::super::subjektiv_jobs_tests::attach_memory_job_source(
                    &api, &subject,
                )
                .await;
                assert_eq!(source.runtime_id, EMBEDDED_WORKER_RUNTIME_ID);
                let identity =
                    Arc::new(RuntimeIdentityMaterial::generate(&source.runtime_id).unwrap());
                configure_runtime_request_auth(&mut api, &identity, &source.runtime_id);
                let owner =
                    seed_test_workspace_owner_token(api.store.as_ref(), "signed-chat-owner").await;
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
            .adopt_policy(r#"{ sets = ["conversation", "work-coordination"]; }"#, "{}")
            .await;
        assert_eq!(
            policy.source.runtime_id == EMBEDDED_WORKER_RUNTIME_ID,
            matches!(kind, RuntimeKind::Embedded)
        );
        let lease = policy
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &policy.source)
            .unwrap()
            .expect("normally created source owns the current Subject body");
        assert_eq!(lease.worker, policy.source);
        assert_eq!(
            lease.key,
            crate::subjektiv::subject_worker_singleton_key(&policy.subject).unwrap()
        );
        // Chat uses the Feature owner, not the pure DB owner. Inject one worker
        // only after normal fixture setup; its real queue is controlled below.
        let executor = crate::db_executor::DbExecutor::new(Default::default()).unwrap();
        policy.api.host_executors = HostExecutors {
            feature: Arc::new(std::sync::OnceLock::from(Some(executor))),
            ..policy.api.host_executors.clone()
        };
        Self { policy }
    }

    fn signed(&self, bytes: &[u8]) -> Request<Body> {
        runtime_source_request(
            &self.policy.identity,
            Some(&self.policy.source.worker_id),
            "POST",
            &chat_worker_path(),
            bytes.to_vec(),
        )
    }

    async fn verified(&self, bytes: &[u8]) -> ServerRequestContext {
        let signed = self.signed(bytes);
        let proof = signed.headers()[worker_runtime::auth::RUNTIME_REQUEST_SOURCE_PROOF_HEADER]
            .to_str()
            .unwrap();
        let verified = tokio::time::timeout(
            WAIT,
            crate::worker_source::verify_runtime_request_source_proof(
                &self.policy.api,
                proof,
                TEST_WORKSPACE_ID,
                worker_runtime::auth::WORKSPACE_REQUEST_PERMISSION,
                "POST",
                signed.uri().path(),
                &worker_runtime::auth::request_body_digest(bytes),
            ),
        )
        .await
        .expect("B1 proof verification deadline")
        .expect("genuine B1 proof must verify before any hold");
        assert_eq!(verified.runtime_id, self.policy.source.runtime_id);
        assert_eq!(
            verified.worker_id.as_deref(),
            Some(self.policy.source.worker_id.as_str())
        );
        // Only the verifier's frozen original source flows to the handler.
        // Never fill authentication_binding_id from a new current lookup.
        ServerRequestContext {
            actor: None,
            worker_source: None,
            runtime_source: Some(ServerRuntimeSource {
                runtime_id: verified.runtime_id,
                worker_id: verified.worker_id,
                authentication_binding_id: verified.authentication_binding_id,
            }),
            origin: None,
            transport_headers: vec![],
        }
    }

    async fn call_ok(&self, bytes: &[u8]) -> Value {
        let (status, body) =
            tokio::time::timeout(WAIT, response(&self.policy.api, self.signed(bytes)))
                .await
                .expect("fresh signed router deadline");
        assert_eq!(status, StatusCode::OK, "current binding control: {body}");
        body
    }

    fn replace_trust(&mut self, change: Change) {
        let catalog = &self.policy.api.config_store;
        let runtime = &self.policy.source.runtime_id;
        let original = catalog
            .get_workspace_runtime_binding(TEST_WORKSPACE_ID, runtime)
            .unwrap()
            .unwrap();
        if matches!(change, Change::Unchanged) {
            assert!(original.revoked_at.is_none());
            return;
        }
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
            assert_eq!(current.public_key, original.public_key);
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

    fn ownership(&self) -> crate::store::WorkerRegistryProjectionRecord {
        let mut projection = self
            .policy
            .api
            .store
            .worker_registry_projection(TEST_WORKSPACE_ID, &self.policy.source)
            .unwrap()
            .unwrap();
        // Only liveness observations may refresh; ownership and body do not change.
        projection.observation = None;
        projection
    }

    fn native_probe(&self) -> rusqlite::Connection {
        // Resolve the actual managed file before the request holds it. This
        // independent connection probes SQLite's writer fence, not a mock flag.
        let path: String = self
            .policy
            .api
            .feature_storage
            .open(&self.policy.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT file FROM pragma_database_list WHERE name='main'",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        let probe = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        probe.busy_timeout(Duration::ZERO).unwrap();
        probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
        probe
    }

    fn snapshot(&self) -> Vec<(String, Vec<Vec<String>>)> {
        // Trusted post-call inspection, never authority for the pending request.
        // Operation keys live on chat_messages; include complete rows, input CAS
        // and reference state so a refusal cannot hide a consumed key or counter.
        self.policy
            .api
            .feature_storage
            .open(&self.policy.api.subjektiv_registration)
            .unwrap()
            .with_connection(|connection| {
                let mut snapshot = Vec::new();
                for table in [
                    "chat_conversations",
                    "chat_messages",
                    "chat_conversation_references",
                ] {
                    let mut query = connection.prepare(&format!("SELECT * FROM {table}"))?;
                    let columns = query.column_count();
                    let mut rows = query
                        .query_map([], |row| {
                            (0..columns)
                                .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                                .collect::<rusqlite::Result<Vec<_>>>()
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows.sort();
                    snapshot.push((table.into(), rows));
                }
                Ok(snapshot)
            })
            .unwrap()
    }
}

// Direct-handler cases still check the typed production rejection. Router cases
// additionally assert the actual HTTP status and absence of a protected DTO.
fn handler_outcome(result: ApiResult<ChatWorkerResponse>) -> (StatusCode, Value) {
    match result {
        Ok(body) => (StatusCode::OK, serde_json::to_value(body).unwrap()),
        Err(error) => {
            assert!(
                matches!(error.error, Error::WorkspacePermissionDenied(_)),
                "not a binding authorization denial: {error:?}"
            );
            (
                StatusCode::FORBIDDEN,
                json!({"error":error.error.to_string()}),
            )
        }
    }
}

async fn executor_queue(f: &mut SignedChat, change: Change, bytes: &[u8]) -> (StatusCode, Value) {
    let context = f.verified(bytes).await;
    let executor = f.policy.api.host_executors.feature().unwrap();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (resume, release) = mpsc::channel();
    let mut occupying = Box::pin(executor.run(move || -> crate::Result<()> {
        entered.send(()).unwrap();
        release
            .recv_timeout(WAIT)
            .expect("release real Feature executor blocker");
        Ok(())
    }));
    assert!(futures::poll!(occupying.as_mut()).is_pending());
    tokio::time::timeout(WAIT, started).await.unwrap().unwrap();
    let mut queued = Box::pin(crate::server::chat::worker_operation(
        f.policy.api.clone(),
        context,
        TEST_WORKSPACE_ID.into(),
        serde_json::from_slice(bytes).unwrap(),
    ));
    // worker_operation immediately submits its whole closure to this one-worker
    // Feature lane. One poll enqueues B1 behind the occupied worker; no preflight
    // callback stands in for actual executor admission.
    assert!(futures::poll!(queued.as_mut()).is_pending());
    f.replace_trust(change);
    resume.send(()).unwrap();
    tokio::time::timeout(WAIT, occupying)
        .await
        .unwrap()
        .unwrap();
    handler_outcome(tokio::time::timeout(WAIT, queued).await.unwrap())
}

async fn lifecycle(f: &mut SignedChat, change: Change, bytes: &[u8]) -> (StatusCode, Value) {
    let signed = f.signed(bytes);
    let gate = subjektiv_journal::lifecycle_lock(&f.policy.api, &f.policy.subject).unwrap();
    let held = gate.lock().unwrap();
    let reached = crate::server::chat::observe_next_admission(&f.policy.subject);
    let api = f.policy.api.clone();
    let pending = tokio::spawn(async move { response(&api, signed).await });
    tokio::task::spawn_blocking(move || {
        reached
            .recv_timeout(WAIT)
            .expect("real router B1 reached held lifecycle gate")
    })
    .await
    .unwrap();
    assert!(
        !pending.is_finished(),
        "lifecycle gate must hold the request"
    );
    f.replace_trust(change);
    drop(held);
    tokio::time::timeout(WAIT, pending).await.unwrap().unwrap()
}

async fn observed_boundary(
    f: &mut SignedChat,
    change: Change,
    boundary: Boundary,
    bytes: &[u8],
) -> (StatusCode, Value) {
    let native_probe = matches!(boundary, Boundary::Native).then(|| f.native_probe());
    let context = f.verified(bytes).await;
    let (entered, reached) = mpsc::channel();
    let (resume, resumed) = mpsc::channel();
    let resumed = Mutex::new(resumed);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        assert_eq!(observed.fetch_add(1, Ordering::SeqCst), 0);
        entered.send(()).unwrap();
        resumed
            .lock()
            .unwrap()
            .recv_timeout(WAIT)
            .expect("release held chat boundary");
    });
    let api = f.policy.api.clone();
    let request: ChatWorkerRequest = serde_json::from_slice(bytes).unwrap();
    let pending = tokio::spawn(async move {
        match boundary {
            Boundary::BeforeFeature => {
                crate::server::chat::worker_operation_before_feature(
                    api,
                    context,
                    TEST_WORKSPACE_ID.into(),
                    request,
                    hook,
                )
                .await
            }
            Boundary::Native => {
                // Parent-owned test-only hook after the native IMMEDIATE
                // transaction is acquired, before registry/Host entry.
                crate::server::chat::worker_operation_before_host(
                    api,
                    context,
                    TEST_WORKSPACE_ID.into(),
                    request,
                    hook,
                )
                .await
            }
            _ => unreachable!(),
        }
    });
    tokio::task::spawn_blocking(move || {
        reached
            .recv_timeout(WAIT)
            .expect("verified B1 reached exact chat boundary")
    })
    .await
    .unwrap();
    assert!(!pending.is_finished(), "observer must hold the request");
    if let Some(probe) = &native_probe {
        let blocked = probe
            .execute_batch("BEGIN IMMEDIATE;")
            .expect_err("native hook must hold the real IMMEDIATE Feature transaction");
        assert_eq!(
            blocked.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DatabaseBusy),
            "must be a held SQLite writer, not a simulated preflight: {blocked}"
        );
    }
    // Neither observer holds a Host transaction/registry guard: trust replacement
    // can commit while the native Feature transaction remains held in Native.
    f.replace_trust(change);
    resume.send(()).unwrap();
    let result = tokio::time::timeout(WAIT, pending).await.unwrap().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    if let Some(probe) = native_probe {
        probe.execute_batch("BEGIN IMMEDIATE; ROLLBACK;").unwrap();
    }
    handler_outcome(result)
}

async fn exercise(f: &mut SignedChat, change: Change, boundary: Boundary, is_read: bool) {
    let id = format!("{boundary:?}-{is_read}");
    let c = conversation(&f.policy, &f.policy.subject, &format!("conversation-{id}")).await;
    let input = user(&f.policy, &c.id, &format!("user-{id}"), PROTECTED).await;
    let operation_id = format!("send-{id}");
    let operation = if is_read {
        read(&c.id)
    } else {
        send(&c.id, &operation_id, Some(&input.message.id))
    };
    let bytes = serde_json::to_vec(&json!({"operation":operation})).unwrap();
    let before = f.snapshot();
    let body_owner = f.ownership();
    let lease = f
        .policy
        .api
        .store
        .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.policy.source)
        .unwrap()
        .unwrap();
    let (status, body) = match boundary {
        Boundary::ExecutorQueue => executor_queue(f, change, &bytes).await,
        Boundary::Lifecycle => lifecycle(f, change, &bytes).await,
        Boundary::BeforeFeature | Boundary::Native => {
            observed_boundary(f, change, boundary, &bytes).await
        }
    };
    assert_eq!(
        f.ownership(),
        body_owner,
        "trust changes must not replace the body"
    );
    assert_eq!(
        f.policy
            .api
            .store
            .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.policy.source)
            .unwrap()
            .unwrap(),
        lease,
        "same current Embedded/Remote singleton, generation and Worker"
    );
    let current = if matches!(change, Change::Unchanged) {
        assert_eq!(status, StatusCode::OK, "unchanged {boundary:?}: {body}");
        body
    } else {
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{change:?} {boundary:?}: {body}"
        );
        assert!(
            body.to_string().contains("authentication binding changed"),
            "must reject original B1, not policy, ownership or input CAS: {body}"
        );
        assert!(serde_json::from_value::<ChatWorkerResponse>(body.clone()).is_err());
        assert!(
            !body.to_string().contains(PROTECTED),
            "no protected chat body: {body}"
        );
        assert_eq!(
            f.snapshot(),
            before,
            "denied read/send changes no chat row or operation key"
        );
        // Fresh B2 proof only. Exact original bytes retain operation ID and the
        // observed user-message CAS; B1 must not consume either before refusing.
        f.call_ok(&bytes).await
    };
    let result: ChatWorkerResponse = serde_json::from_value(current.clone()).unwrap();
    let persisted = page(&f.policy, &c.id).await;
    assert_eq!(
        persisted.conversation.last_user_message_id,
        Some(input.message.id.clone())
    );
    if is_read {
        let ChatWorkerResponse::Messages { page: protected } = result else {
            panic!("current read must return a protected message page");
        };
        assert_eq!(protected, persisted);
        assert_eq!(protected.messages, vec![input.message]);
        assert_eq!(f.snapshot(), before, "valid read is read-only");
    } else {
        let ChatWorkerResponse::Sent { message } = result else {
            panic!("current send must return its persisted message");
        };
        assert_eq!(message.operation_id, operation_id);
        assert_eq!(message.last_user_message_id, Some(input.message.id.clone()));
        assert_eq!(
            message.sender,
            server_api::ChatMessageSender::Subject {
                subject_id: f.policy.subject.clone(),
            }
        );
        assert_eq!(persisted.messages, vec![input.message, message]);
        assert_ne!(f.snapshot(), before, "valid send must really persist");
    }
    let saved = f.snapshot();
    assert_eq!(
        f.call_ok(&bytes).await,
        current,
        "exact signed replay returns the saved result"
    );
    assert_eq!(
        f.snapshot(),
        saved,
        "same operation/CAS replay has no second effect"
    );
}
