//! Native Memory admission versus production chat, grant binding and enrollment.
//! Real Feature SQLite, registry, canonical adoption, body/Job ownership and Host fences are
//! connected; Fixture replaces Runtime execution only. Authentication transport
//! is outside this component test: chat receives an already-authenticated context.
//! The two explicit observers schedule requests without replacing authorization.
//! Ordinary parent tests invoke exact ignored child cases in the same libtest
//! executable, so an ABBA regression can be killed and reaped with its temp root.
use super::*;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PHASE_BOUND: Duration = Duration::from_secs(10);

struct LockOrderChild(Child);
impl Drop for LockOrderChild {
    fn drop(&mut self) {
        // Also reap on assertion/panic. A native std::Mutex cycle cannot be
        // cancelled by aborting a Tokio task or timing out a thread join.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn bounded_case(case: &str) {
    let scratch = tempfile::tempdir().unwrap();
    let log_path = scratch.path().join("native-lock-order.log");
    let log = fs::File::create(&log_path).unwrap();
    // libtest names omit the crate component from module_path!().
    let module = module_path!().split_once("::").unwrap().1;
    let mut child = LockOrderChild(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                &format!("{module}::{case}"),
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            // This is only a child allocation root, not a product/test hook.
            // The parent's RAII directory owns fixture files even after SIGKILL.
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
        // Poll the actual child exit status, never infer product completion from
        // elapsed time. The short park only avoids spinning the watchdog thread.
        std::thread::park_timeout(Duration::from_millis(10));
    };
    let output = fs::read_to_string(&log_path).unwrap();
    assert!(
        status.is_some(),
        "native lock-order deadline ({case}):\n{output}"
    );
    assert!(status.unwrap().success(), "{case}:\n{output}");
    assert!(
        output.contains("1 passed; 0 failed"),
        "exact child must actually execute, not match zero tests: {output}"
    );
}

#[test]
fn native_memory_admission_and_chat_send_complete_without_registry_inversion() {
    bounded_case("native_memory_chat_allowed_case");
}

#[test]
fn native_memory_admission_and_chat_feature_removal_deny_without_append() {
    bounded_case("native_memory_chat_feature_removed_case");
}

#[test]
#[ignore = "exact subprocess case; run by the bounded native Memory/chat parent test"]
fn native_memory_chat_allowed_case() {
    native_memory_chat_case(false);
}

#[test]
#[ignore = "exact subprocess case; run by the bounded native Memory/chat parent test"]
fn native_memory_chat_feature_removed_case() {
    native_memory_chat_case(true);
}

fn native_memory_chat_case(remove_chat: bool) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(memory_chat_schedule(remove_chat));
    runtime.shutdown_timeout(Duration::from_secs(2));
}

async fn memory_chat_schedule(remove_chat: bool) {
    let f = Fixture::new().await;
    f.adopt_policy(r#"{ sets = ["subject-default", "memory"]; }"#, "{}")
        .await;
    // Exercise the real dispatch/attempt-derived Memory Job grant rather than a
    // second fake registry-only consumer or an unbound consolidation authority.
    f.api
        .runtime
        .register_or_replace(HeldAuthorizationJobRuntime(
            WorkdirlessFixtureRuntime::default(),
            worker_runtime::Runtime::new_memory(),
        ));
    let dispatched =
        subjektiv_job::dispatch(&f.api, &f.subject, Some(f.source.clone()), 0, 0).unwrap();
    assert_eq!(dispatched.status, "started");
    let job = f
        .api
        .store
        .find_active_backend_job_for_resource(
            TEST_WORKSPACE_ID,
            &crate::backend_job::subjektiv_serialization_key(&f.subject),
        )
        .unwrap()
        .unwrap();
    let memory_worker = f
        .api
        .store
        .get_backend_job_attempt(
            TEST_WORKSPACE_ID,
            &job.request.job_id,
            &crate::backend_job::attempt_id(&job.request.job_id, job.current_attempt),
        )
        .unwrap()
        .unwrap()
        .worker
        .unwrap();
    let store = open_subjektiv_store(&f.api).unwrap();
    let conversation = store
        .chat_create_conversation(
            "native-lock-order-account",
            &server_api::ChatCreateConversationRequest {
                operation_id: "native-lock-order-conversation".into(),
                subject_id: f.subject.clone(),
            },
        )
        .unwrap();
    let context = subjektiv::api::HostOperationContext::validated_consolidation(
        &store,
        &f.subject,
        Some(vec![]),
        None,
    )
    .unwrap();
    let binding = f
        .api
        .store
        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &f.source.runtime_id)
        .await
        .unwrap()
        .unwrap();
    let chat_context = server_api::ServerRequestContext {
        actor: None,
        worker_source: None,
        runtime_source: Some(server_api::ServerRuntimeSource {
            runtime_id: f.source.runtime_id.clone(),
            worker_id: Some(f.source.worker_id.clone()),
            authentication_binding_id: binding.binding_id,
        }),
        origin: None,
        transport_headers: vec![],
    };
    let request = server_api::ChatWorkerRequest {
        operation: server_api::ChatWorkerOperation::Send {
            request: serde_json::from_value(json!({
                "conversation_id": conversation.id,
                "operation_id": "native-lock-order-send",
                "text": "persisted after native Memory admission",
                "expected_last_user_message_id": null,
            }))
            .unwrap(),
        },
    };
    let (start_memory, memory_start) = std::sync::mpsc::channel();
    let (admitted, memory_admitted) = std::sync::mpsc::channel();
    let (consumer_at_wait, wait_for_consumer) = std::sync::mpsc::channel();
    let wait_for_consumer = Mutex::new(wait_for_consumer);
    let effect_store = subject_authorization::memory_effect_store_before_registry(
        &f.api,
        &store,
        &memory_worker,
        &f.subject,
        "memory.surface.prepare",
        Arc::new(move || {
            // This runs AFTER the real native Feature transaction is acquired,
            // BEFORE Memory requests registry write or a Host reservation.
            admitted.send(()).unwrap();
            wait_for_consumer
                .lock()
                .unwrap()
                .recv_timeout(PHASE_BOUND)
                .expect("production chat must reach its Feature wait edge");
        }),
    );
    let (memory_finished, memory_result) = tokio::sync::oneshot::channel();
    let memory = std::thread::spawn(move || {
        memory_start
            .recv_timeout(PHASE_BOUND)
            .expect("chat prechecks finished");
        let result = subjektiv::api::execute(
            &effect_store,
            &context,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                serde_json::from_value(json!({})).unwrap(),
            ),
        );
        let _ = memory_finished.send(result);
    });
    let memory_admitted = Mutex::new(memory_admitted);
    let registry = f.api.permission_registry.clone();
    let result = tokio::time::timeout(
        PHASE_BOUND,
        crate::server::chat::worker_operation_before_feature(
            f.api.clone(),
            chat_context,
            TEST_WORKSPACE_ID.into(),
            request,
            Arc::new(move || {
                // Chat has completed its real scope/store/body prechecks, so
                // Memory cannot accidentally block an earlier preparatory read.
                start_memory.send(()).unwrap();
                memory_admitted
                    .lock()
                    .unwrap()
                    .recv_timeout(PHASE_BOUND)
                    .expect("Memory must own native Feature before chat waits");
                if remove_chat {
                    registry
                        .write()
                        .unwrap()
                        .set_enabled(
                            &worker::feature::FeatureId::builtin("subjektiv-chat"),
                            false,
                        )
                        .unwrap();
                }
                // Returning goes directly into production with_chat_transaction.
                // With the old pre-acquired registry read, Memory's write and
                // chat's Feature acquisition form ABBA. No synthetic lock path
                // authorizes or performs either effect.
                consumer_at_wait.send(()).unwrap();
            }),
        ),
    )
    .await
    .expect("production chat must finish after native Memory admission");
    let memory_result = tokio::time::timeout(PHASE_BOUND, memory_result)
        .await
        .expect("native Memory completion deadline")
        .unwrap();
    // Join only after the completion event; a regression is recovered by killing
    // the child process, never an unbounded join in the ordinary test process.
    memory.join().unwrap();
    let server_api::SubjektivMemoryBackendResponse::SurfacePrepared(prepared) =
        memory_result.unwrap()
    else {
        panic!("Memory must return its admitted surface generation");
    };
    let page = open_subjektiv_store(&f.api)
        .unwrap()
        .chat_messages_for_account(
            "native-lock-order-account",
            &conversation.id,
            &Default::default(),
        )
        .unwrap();
    if remove_chat {
        assert!(
            matches!(
                result,
                Err(ApiError {
                    error: Error::WorkspacePermissionDenied(_),
                    ..
                })
            ),
            "removed chat Feature must deny at admission: {result:?}"
        );
        assert!(
            page.messages.is_empty(),
            "denial must not append a chat message"
        );
    } else {
        let server_api::ChatWorkerResponse::Sent { message } = result.unwrap() else {
            panic!("Send must return its committed message");
        };
        assert_eq!(message.operation_id, "native-lock-order-send");
        assert_eq!(page.messages, vec![message]);
    }
    f.api
        .feature_storage
        .open(&f.api.subjektiv_registration)
        .unwrap()
        .with_connection(|connection| {
            let count: i64 = connection.query_row(
                "SELECT count(*) FROM surface_generation_runs WHERE subject_id=?1",
                [&f.subject],
                |row| row.get(0),
            )?;
            assert_eq!(count, 1, "exactly one admitted Memory effect persists");
            let fingerprint: String = connection.query_row(
                "SELECT memory_fingerprint FROM surface_generation_runs WHERE subject_id=?1 AND generation_id=?2",
                rusqlite::params![f.subject, prepared.generation_id],
                |row| row.get(0),
            )?;
            assert_eq!(fingerprint, prepared.memory_fingerprint);
            Ok(())
        })
        .unwrap();
}

#[test]
fn native_memory_and_grant_binding_commit_without_registry_inversion() {
    bounded_case("native_memory_binding_allowed_case");
}

#[test]
fn native_memory_and_grant_binding_recheck_removed_feature_without_binding() {
    bounded_case("native_memory_binding_removed_case");
}

#[test]
fn native_memory_and_subject_enrollment_commit_without_registry_inversion() {
    bounded_case("native_memory_enrollment_case");
}

#[derive(Clone, Copy)]
enum AuthorityConsumer {
    Binding { remove_ticket: bool },
    Enrollment,
}

#[test]
#[ignore = "exact subprocess case; run by the bounded native Memory/binding parent test"]
fn native_memory_binding_allowed_case() {
    native_memory_authority_case(AuthorityConsumer::Binding {
        remove_ticket: false,
    });
}

#[test]
#[ignore = "exact subprocess case; run by the bounded native Memory/binding parent test"]
fn native_memory_binding_removed_case() {
    native_memory_authority_case(AuthorityConsumer::Binding {
        remove_ticket: true,
    });
}

#[test]
#[ignore = "exact subprocess case; run by the bounded native Memory/enrollment parent test"]
fn native_memory_enrollment_case() {
    native_memory_authority_case(AuthorityConsumer::Enrollment);
}

fn native_memory_authority_case(consumer: AuthorityConsumer) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(memory_authority_schedule(consumer));
    runtime.shutdown_timeout(Duration::from_secs(2));
}

async fn memory_authority_schedule(consumer: AuthorityConsumer) {
    let f = Fixture::new().await;
    f.adopt_policy(
        r#"{ sets = ["memory"]; }"#,
        r#"{ namespaces = [{ namespace = "ticket"; keys = ["ticket.create"]; }]; }"#,
    )
    .await;
    let store = open_subjektiv_store(&f.api).unwrap();
    let owner = f
        .api
        .store
        .get_workspace(TEST_WORKSPACE_ID)
        .await
        .unwrap()
        .unwrap()
        .owner_account_id;
    let enrollment = store
        .create_subject_idempotent(
            crate::subjektiv::SubjectRole::new("native-enrollment").unwrap(),
            "enrollment behavior".into(),
            "native-enrollment-creation",
        )
        .unwrap();
    let schema = f.api.config_schema_registry.compose().unwrap();
    let grant = subject_authorization::prepare_delegation(
        &f.api,
        &f.source,
        Some(&server_api::authorization::WorkerDelegationRequest {
            permissions: vec![create_permission()],
            redelegation: vec![],
        }),
        &f.source.runtime_id,
        "native-binding-profile",
        "native-binding-profile-digest",
        None,
        false,
    )
    .unwrap()
    .unwrap();
    let context = subjektiv::api::HostOperationContext::validated_consolidation(
        &store,
        &f.subject,
        Some(vec![]),
        None,
    )
    .unwrap();
    let (start_memory, memory_start) = std::sync::mpsc::channel();
    let (admitted, memory_admitted) = std::sync::mpsc::channel();
    let (consumer_at_wait, wait_for_consumer) = std::sync::mpsc::channel();
    let wait_for_consumer = Mutex::new(wait_for_consumer);
    let effect_store = subject_authorization::memory_effect_store_before_registry(
        &f.api,
        &store,
        &f.source,
        &f.subject,
        "memory.surface.prepare",
        Arc::new(move || {
            // Actual native Feature connection + IMMEDIATE transaction are held.
            admitted.send(()).unwrap();
            wait_for_consumer
                .lock()
                .unwrap()
                .recv_timeout(PHASE_BOUND)
                .expect("binding/enrollment must reach its Feature wait edge");
        }),
    );
    let (memory_finished, memory_result) = std::sync::mpsc::channel();
    let memory = std::thread::spawn(move || {
        memory_start
            .recv_timeout(PHASE_BOUND)
            .expect("consumer prechecks finished");
        let result = subjektiv::api::execute(
            &effect_store,
            &context,
            server_api::SubjektivMemoryBackendOperation::PrepareSurface(
                serde_json::from_value(json!({})).unwrap(),
            ),
        );
        memory_finished.send(result).unwrap();
    });
    // This observer is immediately before the real consumer's native writer;
    // no earlier Feature read can accidentally consume the forced wait edge.
    let before_feature = || {
        start_memory.send(()).unwrap();
        memory_admitted
            .recv_timeout(PHASE_BOUND)
            .expect("Memory owns native Feature");
        if matches!(
            consumer,
            AuthorityConsumer::Binding {
                remove_ticket: true
            }
        ) {
            f.api
                .permission_registry
                .write()
                .unwrap()
                .set_enabled(&worker::feature::FeatureId::builtin("ticket"), false)
                .unwrap();
        }
        consumer_at_wait.send(()).unwrap();
    };
    match consumer {
        AuthorityConsumer::Binding { remove_ticket } => {
            let result = subject_authorization::bind_reserved_grant_before_feature(
                &f.api,
                &grant,
                "native-bound-child",
                before_feature,
            );
            let saved = store
                .worker_execution_grant(&grant.runtime_id, "native-bound-child")
                .unwrap();
            if remove_ticket {
                assert!(
                    result.is_err(),
                    "removed catalog key must deny issuance: {result:?}"
                );
                assert!(saved.is_none(), "denial must not persist a grant");
            } else {
                result.unwrap();
                let mut expected = grant.clone();
                expected.worker_id = "native-bound-child".into();
                assert_eq!(saved, Some(expected));
                // The same authenticated binding path must recover immutable evidence.
                subject_authorization::bind_reserved_grant(&f.api, &grant, "native-bound-child")
                    .unwrap();
            }
        }
        AuthorityConsumer::Enrollment => {
            let result = f
                .api
                .config_store
                .enroll_created_subject_observed(
                    TEST_WORKSPACE_ID,
                    &owner,
                    &store,
                    &enrollment.id,
                    schema.clone(),
                    || {
                        let registry = f.api.permission_registry.read().unwrap();
                        let host = registry.host().clone();
                        Ok((registry, host))
                    },
                    before_feature,
                )
                .unwrap()
                .expect("post-seal enrollment");
            assert!(result.source_stored && result.policy_committed);
            let policy = f
                .api
                .config_store
                .adopted_subject_policy(TEST_WORKSPACE_ID, &enrollment.id)
                .unwrap()
                .expect("enrollment persisted canonical policy");
            assert!(policy.current().legacy_constraints.is_none());
            let replay = f
                .api
                .config_store
                .enroll_created_subject(
                    TEST_WORKSPACE_ID,
                    &owner,
                    &store,
                    &enrollment.id,
                    schema,
                    || {
                        let registry = f.api.permission_registry.read().unwrap();
                        let host = registry.host().clone();
                        Ok((registry, host))
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(
                replay, result,
                "same enrollment returns its adoption receipt"
            );
            assert_eq!(
                f.api
                    .config_store
                    .configuration_adoption_result(
                        TEST_WORKSPACE_ID,
                        &owner,
                        &format!("subject-enrollment:{}", enrollment.id),
                    )
                    .unwrap(),
                Some(result)
            );
        }
    }
    let memory_result = memory_result
        .recv_timeout(PHASE_BOUND)
        .expect("native Memory completion");
    memory.join().unwrap();
    let server_api::SubjektivMemoryBackendResponse::SurfacePrepared(prepared) =
        memory_result.unwrap()
    else {
        panic!("native Memory must return its committed generation");
    };
    f.api.feature_storage.open(&f.api.subjektiv_registration).unwrap().with_connection(|connection| {
        let count: i64 = connection.query_row(
            "SELECT count(*) FROM surface_generation_runs WHERE subject_id=?1", [&f.subject], |row| row.get(0),
        )?;
        assert_eq!(count, 1, "exactly one Memory effect persisted");
        let fingerprint: String = connection.query_row(
            "SELECT memory_fingerprint FROM surface_generation_runs WHERE subject_id=?1 AND generation_id=?2",
            rusqlite::params![f.subject, prepared.generation_id], |row| row.get(0),
        )?;
        assert_eq!(fingerprint, prepared.memory_fingerprint);
        Ok(())
    }).unwrap();
}
