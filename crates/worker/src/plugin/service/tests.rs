//! Real ServiceDriver/FeatureBackgroundTask + canonical authored Component tests.
//! Only the PluginHostOwner is scripted; its pause/recovery state is not evidence
//! of durable owner transactions (covered by the owner integration suites).
//! Run: cargo test -p worker --lib plugin::service::tests::

use super::*;
use crate::feature::background::FeatureBackgroundTaskRegistryBuilder;
use crate::hook::HookInvocationContext;
use manifest::plugin::{self, PluginPackageLimits, PluginSourceKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{Semaphore, oneshot};

const SERVICE: &str = "ticket_events";
const MANIFEST: &str = include_str!("../../../../plugin-runtime/tests/service-guest/plugin.toml");
const COMPONENT: &[u8] =
    include_bytes!("../../../../workspace-server/tests/fixtures/ticket-service-v2.component.wasm");

struct Fixture {
    executor: Arc<Executor>,
    compiled: Arc<CompiledService>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("plugin.toml"), MANIFEST).unwrap();
        std::fs::write(directory.path().join("plugin.component.wasm"), COMPONENT).unwrap();
        let limits = PluginPackageLimits::default();
        let package =
            plugin::read_plugin_directory(directory.path(), PluginSourceKind::Explicit, &limits)
                .unwrap();
        let artifact = plugin::pack_plugin_package(&package, &limits).unwrap();
        let executor = Arc::new(Executor::new(Limits::default()).unwrap());
        let compiled = Arc::new(
            executor
                .compile_service(
                    &artifact.bytes,
                    &artifact.package_digest,
                    &artifact.artifact_digest,
                    artifact.artifact_size,
                )
                .unwrap(),
        );
        Self { executor, compiled }
    }

    fn driver(&self, owner: Arc<Owner>) -> Arc<ServiceDriver> {
        let namespace = ("notes".into(), "note".into(), "1".into());
        Arc::new(ServiceDriver {
            executor: self.executor.clone(),
            compiled: self.compiled.clone(),
            workspace: "workspace".into(),
            installation: owner.installation.clone(),
            plan: "plan".into(),
            subject: "worker".into(),
            grants: Grants {
                logging: true,
                connection_slots: ["api".into()].into(),
                ticket_events: true,
                ticket_read: true,
                ticket_data_read: [namespace.clone()].into(),
                ticket_data_write: [namespace].into(),
            },
            owner,
            name: SERVICE.into(),
            config: r#"{"mode":"normal"}"#.into(),
            lease: Mutex::new(None),
            stop_started: Mutex::new(None),
            parked_consumer: Mutex::new(None),
            unconfirmed_indeterminate: Mutex::new(None),
            prepared_start: AtomicBool::new(false),
        })
    }
}

// Obtain a real registry-generated generation fence rather than bypassing the
// FeatureBackgroundTask boundary or adding a production-only context constructor.
struct CaptureContext(Mutex<Option<oneshot::Sender<BackgroundTaskContext>>>);

#[async_trait]
impl FeatureBackgroundTask for CaptureContext {
    async fn run(
        &self,
        context: BackgroundTaskContext,
        _: BackgroundTaskCancellation,
    ) -> Result<(), HookError> {
        self.0
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .send(context)
            .unwrap();
        Ok(())
    }
}

async fn context() -> BackgroundTaskContext {
    let (sender, receiver) = oneshot::channel();
    let feature = FeatureId::builtin("service-driver-test");
    let mut builder = FeatureBackgroundTaskRegistryBuilder::default();
    builder
        .register(
            feature.clone(),
            BackgroundTaskSpec::single_flight(
                BackgroundTaskDeclaration::worker_managed("capture", "capture test context"),
                Duration::from_secs(5),
            ),
            CaptureContext(Mutex::new(Some(sender))),
        )
        .unwrap();
    let registry = builder.build();
    registry
        .start(&feature, "capture", HookInvocationContext::default())
        .unwrap();
    let context = receiver.await.unwrap();
    registry.shutdown().await.unwrap();
    context.generation_fence.ensure_current().unwrap();
    context
}

#[derive(Clone, Copy)]
enum CompleteResponse {
    Lost,
    Unexpected,
    ConfirmIndeterminate,
}

#[derive(Default)]
struct State {
    requests: Vec<PluginServiceRequest>,
    lease: Option<PluginServiceLease>,
    acquisitions: usize,
    pause: Option<PluginServicePauseReason>,
}

struct Owner {
    installation: String,
    state: Mutex<State>,
    complete_response: CompleteResponse,
    gate_log: AtomicBool,
    deny_log: AtomicBool,
    deny_release: AtomicBool,
    deny_publish: AtomicBool,
    log_entered: Semaphore,
}

impl Owner {
    fn new(installation: &str, complete_response: CompleteResponse) -> Arc<Self> {
        Arc::new(Self {
            installation: installation.into(),
            state: Mutex::new(State::default()),
            complete_response,
            gate_log: AtomicBool::new(false),
            deny_log: AtomicBool::new(false),
            deny_release: AtomicBool::new(false),
            deny_publish: AtomicBool::new(false),
            log_entered: Semaphore::new(0),
        })
    }

    fn consumer(&self) -> String {
        format!("consumer:{}", self.installation)
    }

    fn requests(&self) -> Vec<PluginServiceRequest> {
        self.state.lock().unwrap().requests.clone()
    }

    fn diagnostics(&self) -> PluginServiceDiagnostics {
        let state = self.state.lock().unwrap();
        PluginServiceDiagnostics {
            consumer_id: self.consumer(),
            pause_reason: state.pause,
            queued: 1,
            attempt: 1,
            current_event_id: Some("event".into()),
            lease_active: state.lease.is_some(),
        }
    }
}

#[async_trait]
impl PluginHostOwner for Owner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        let PluginHostRequest::Service { request } = request else {
            panic!("Service driver must use only the leased/lifecycle channel: {request:?}");
        };
        self.state.lock().unwrap().requests.push(request.clone());
        let response = match request {
            PluginServiceRequest::Prepare {
                plan_id,
                installation_id,
                service_name,
                operation,
            } => {
                assert_eq!(plan_id, "plan");
                assert_eq!(installation_id, self.installation);
                assert_eq!(service_name, SERVICE);
                if operation == PluginServiceLifecycleOperation::Publish
                    && self.deny_publish.load(Ordering::SeqCst)
                {
                    return Err(unavailable());
                }
                PluginServiceResponse::Prepared
            }
            PluginServiceRequest::LifecycleLog { .. } => {
                if self.gate_log.load(Ordering::SeqCst) {
                    self.log_entered.add_permits(1);
                    return std::future::pending().await;
                }
                if self.deny_log.load(Ordering::SeqCst) {
                    return Err(unavailable());
                }
                PluginServiceResponse::Logged
            }
            PluginServiceRequest::Acquire {
                plan_id,
                installation_id,
                service_name,
            } => {
                assert_eq!(plan_id, "plan");
                assert_eq!(installation_id, self.installation);
                assert_eq!(service_name, SERVICE);
                let mut state = self.state.lock().unwrap();
                assert!(state.pause.is_none(), "driver cannot clear the owner pause");
                assert!(state.lease.is_none(), "driver must await inactive lease");
                state.acquisitions += 1;
                let lease = PluginServiceLease {
                    consumer_id: self.consumer(),
                    lease_token: format!("lease-{}", state.acquisitions),
                };
                state.lease = Some(lease.clone());
                PluginServiceResponse::Acquired { lease }
            }
            PluginServiceRequest::Poll { lease } => {
                assert_eq!(self.state.lock().unwrap().lease.as_ref(), Some(&lease));
                if self.state.lock().unwrap().pause.is_some() {
                    PluginServiceResponse::Diagnostics {
                        diagnostics: self.diagnostics(),
                    }
                } else {
                    PluginServiceResponse::Delivery {
                        envelope: PluginTicketEventEnvelope {
                            event_id: "event".into(),
                            event_type: PluginTicketEventType::Changed,
                            version: 1,
                            occurred_at: "2026-10-30T00:00:00Z".into(),
                            ticket_id: "T-796".into(),
                            domain_event_ref: "domain-event".into(),
                            content_digest: "digest".into(),
                            correlation_id: "correlation".into(),
                            causation_id: None,
                            attempt: 1,
                        },
                        invocation_id: "invocation".into(),
                        expires_at_ms: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as i64
                            + 60_000,
                    }
                }
            }
            PluginServiceRequest::Complete {
                lease,
                event_id,
                result,
            } => {
                let mut state = self.state.lock().unwrap();
                assert_eq!(state.lease.as_ref(), Some(&lease));
                assert_eq!(event_id, "event");
                assert_eq!(
                    result,
                    match self.complete_response {
                        CompleteResponse::ConfirmIndeterminate =>
                            PluginServiceEventResult::Indeterminate,
                        _ => PluginServiceEventResult::Ack,
                    }
                );
                // Model an owner decision committed before its response is lost.
                // Audited recovery itself is deliberately outside this unit test.
                state.pause = Some(PluginServicePauseReason::Indeterminate);
                match self.complete_response {
                    CompleteResponse::Lost => return Err(unavailable()),
                    CompleteResponse::Unexpected => PluginServiceResponse::Idle,
                    CompleteResponse::ConfirmIndeterminate => PluginServiceResponse::Completed,
                }
            }
            PluginServiceRequest::Release { lease } => {
                let mut state = self.state.lock().unwrap();
                assert_eq!(state.lease.as_ref(), Some(&lease));
                if self.deny_release.load(Ordering::SeqCst) {
                    return Err(unavailable());
                }
                state.lease = None;
                PluginServiceResponse::Released
            }
            PluginServiceRequest::Diagnostics { consumer_id } => {
                assert_eq!(consumer_id, self.consumer());
                PluginServiceResponse::Diagnostics {
                    diagnostics: self.diagnostics(),
                }
            }
            PluginServiceRequest::Host { lease, request } => {
                assert_eq!(self.state.lock().unwrap().lease.as_ref(), Some(&lease));
                let response = match *request {
                    PluginInvocationRequest::Authorize { invocation_id, .. } => {
                        assert_eq!(invocation_id, "invocation");
                        PluginInvocationResponse::Authorized
                    }
                    PluginInvocationRequest::ResolveTicket { ticket_ref, .. } => {
                        assert_eq!(ticket_ref, "T-796");
                        PluginInvocationResponse::TicketResolved {
                            ticket_id: "canonical-ticket".into(),
                        }
                    }
                    PluginInvocationRequest::ReadTicket { .. } => {
                        PluginInvocationResponse::TicketRead {
                            snapshot: PluginTicketSnapshot {
                                title: "note".into(),
                                case_state: PluginTicketCaseState::Open,
                                content_digest: "digest".into(),
                                body: "body".into(),
                                truncated: false,
                            },
                        }
                    }
                    PluginInvocationRequest::DataLookupMutation { .. } => {
                        PluginInvocationResponse::DataMutationLookup {
                            outcome: PluginDataMutationLookup::NotRecorded,
                        }
                    }
                    PluginInvocationRequest::DataGet { .. } => PluginInvocationResponse::DataCell {
                        cell: PluginDataCell::NeverWritten,
                    },
                    PluginInvocationRequest::DataMutate { .. } => {
                        PluginInvocationResponse::DataMutated {
                            receipt: PluginDataReceipt {
                                mutation_id: "mutation".into(),
                                result_digest: "result".into(),
                            },
                        }
                    }
                    PluginInvocationRequest::Open { .. } => PluginInvocationResponse::Opened,
                    PluginInvocationRequest::Send { .. } => PluginInvocationResponse::Response {
                        response: PluginHttpResponse {
                            status: 200,
                            headers: vec![],
                            body: b"ok".to_vec(),
                        },
                    },
                    other => panic!("unexpected guest Host request: {other:?}"),
                };
                PluginServiceResponse::Host {
                    response: Box::new(response),
                }
            }
        };
        Ok(PluginHostResponse::Service { response })
    }
}

async fn run(driver: &Arc<ServiceDriver>, context: &BackgroundTaskContext) {
    driver
        .run(context.clone(), BackgroundTaskCancellation::default())
        .await
        .expect("a parked observer must remain a successful HostReady run");
}

#[tokio::test]
async fn cancelled_inline_stop_retains_lease_for_protected_cleanup() {
    let fixture = Fixture::new();
    let owner = Owner::new("selected", CompleteResponse::Lost);
    let driver = fixture.driver(owner.clone());
    let context = context().await;
    run(&driver, &context).await;
    let lease = driver.lease.lock().unwrap().clone().unwrap();
    owner.state.lock().unwrap().pause = Some(PluginServicePauseReason::Authorization);
    owner.gate_log.store(true, Ordering::SeqCst);

    // Poll observes a pause and enters inline stopped(). Drop the *run future*
    // at a known guest logging await, just as HostReady cancellation does.
    {
        let pending = driver.run(context.clone(), BackgroundTaskCancellation::default());
        tokio::pin!(pending);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = &mut pending => panic!("stop must wait at LifecycleLog: {result:?}"),
                permit = owner.log_entered.acquire() => permit.unwrap().forget(),
            }
        })
        .await
        .expect("real guest on_stop must reach gated lifecycle logging");
    }
    assert_eq!(driver.lease.lock().unwrap().as_ref(), Some(&lease));
    assert_eq!(
        driver.parked_consumer.lock().unwrap().as_deref(),
        Some(lease.consumer_id.as_str())
    );
    assert!(
        !owner
            .requests()
            .iter()
            .any(|request| matches!(request, PluginServiceRequest::Release { .. }))
    );

    let logs_before_cleanup = owner
        .requests()
        .iter()
        .filter(|r| matches!(r, PluginServiceRequest::LifecycleLog { .. }))
        .count();
    owner.gate_log.store(false, Ordering::SeqCst);
    driver.stopped(context.clone()).await.unwrap();
    assert_eq!(
        owner
            .requests()
            .iter()
            .filter(|r| matches!(r, PluginServiceRequest::LifecycleLog { .. }))
            .count(),
        logs_before_cleanup,
        "interrupted on-stop is not invoked twice"
    );
    assert!(driver.lease.lock().unwrap().is_none());
    assert!(owner.state.lock().unwrap().lease.is_none());
    let releases = owner
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            PluginServiceRequest::Release { lease } => Some(lease),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(releases, [lease]);
    let requests = owner.requests();
    driver.stopped(context).await.unwrap();
    assert_eq!(
        owner.requests(),
        requests,
        "confirmed release is idempotent"
    );
}

#[tokio::test]
async fn failed_stop_export_still_releases_the_owner_lease() {
    let fixture = Fixture::new();
    let owner = Owner::new("selected", CompleteResponse::Lost);
    let driver = fixture.driver(owner.clone());
    let context = context().await;
    run(&driver, &context).await;
    let lease = driver.lease.lock().unwrap().clone().unwrap();
    owner.deny_log.store(true, Ordering::SeqCst);
    assert!(driver.stopped(context).await.is_err());
    assert!(driver.lease.lock().unwrap().is_none());
    assert!(owner.state.lock().unwrap().lease.is_none());
    assert_eq!(
        owner.requests().last(),
        Some(&PluginServiceRequest::Release { lease })
    );
}

#[tokio::test]
async fn unconfirmed_release_retains_the_exact_proof_until_retry_is_confirmed() {
    let fixture = Fixture::new();
    let owner = Owner::new("selected", CompleteResponse::Lost);
    let driver = fixture.driver(owner.clone());
    let context = context().await;
    run(&driver, &context).await;
    let lease = driver.lease.lock().unwrap().clone().unwrap();
    owner.deny_release.store(true, Ordering::SeqCst);
    assert!(driver.stopped(context.clone()).await.is_err());
    assert_eq!(driver.lease.lock().unwrap().as_ref(), Some(&lease));
    assert_eq!(owner.state.lock().unwrap().lease.as_ref(), Some(&lease));
    assert_eq!(
        owner.requests().last(),
        Some(&PluginServiceRequest::Release {
            lease: lease.clone(),
        })
    );
    owner.deny_release.store(false, Ordering::SeqCst);
    driver.stopped(context).await.unwrap();
    assert!(driver.lease.lock().unwrap().is_none());
    assert_eq!(
        owner.requests().last(),
        Some(&PluginServiceRequest::Release { lease })
    );
}

#[tokio::test]
async fn uncertain_complete_parks_without_retry_and_reacquires_only_recovered_consumer() {
    let fixture = Fixture::new();
    let context = context().await;
    for response in [CompleteResponse::Lost, CompleteResponse::Unexpected] {
        let owner = Owner::new("selected", response);
        let driver = fixture.driver(owner.clone());
        let other_owner = Owner::new("other", response);
        let other = fixture.driver(other_owner.clone());
        for (driver, owner) in [(&driver, &owner), (&other, &other_owner)] {
            run(driver, &context).await; // Acquire + real guest on_start.
            run(driver, &context).await; // Real guest effects + uncertain Complete.
            assert!(driver.lease.lock().unwrap().is_none());
            assert_eq!(
                driver.parked_consumer.lock().unwrap().as_deref(),
                Some(owner.consumer().as_str())
            );
            let requests = owner.requests();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| matches!(request,
                        PluginServiceRequest::Host { request, .. }
                            if matches!(request.as_ref(), PluginInvocationRequest::DataMutate { .. })
                    ))
                    .count(),
                1
            );
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| matches!(request,
                        PluginServiceRequest::Host { request, .. }
                            if matches!(request.as_ref(), PluginInvocationRequest::Send { .. })
                    ))
                    .count(),
                1
            );
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| matches!(request, PluginServiceRequest::Complete { .. }))
                    .count(),
                1
            );
            assert_eq!(
                owner.diagnostics().pause_reason,
                Some(PluginServicePauseReason::Indeterminate)
            );
            assert!(!owner.diagnostics().lease_active);
            let before = requests.len();
            run(driver, &context).await;
            assert_eq!(
                &owner.requests()[before..],
                &[PluginServiceRequest::Diagnostics {
                    consumer_id: owner.consumer(),
                }],
                "paused runs must not poll, re-admit the guest, repeat effects, or complete again"
            );
        }

        let other_before = other_owner.requests();
        let original_lease = owner
            .requests()
            .into_iter()
            .find_map(|request| match request {
                PluginServiceRequest::Release { lease } => Some(lease),
                _ => None,
            })
            .unwrap();
        // Script the result of external owner recovery, not an unaudited driver
        // resume. A cleared pause alone is insufficient while any lease is live.
        {
            let mut state = owner.state.lock().unwrap();
            state.pause = None;
            state.lease = Some(original_lease.clone());
        }
        let before = owner.requests().len();
        run(&driver, &context).await;
        assert_eq!(
            &owner.requests()[before..],
            &[PluginServiceRequest::Diagnostics {
                consumer_id: owner.consumer(),
            }]
        );
        assert!(driver.lease.lock().unwrap().is_none());
        owner.state.lock().unwrap().lease = None;
        run(&driver, &context).await;
        let reacquired = driver.lease.lock().unwrap().clone().unwrap();
        assert_eq!(reacquired.consumer_id, original_lease.consumer_id);
        assert_ne!(reacquired.lease_token, original_lease.lease_token);
        assert!(driver.parked_consumer.lock().unwrap().is_none());
        assert_eq!(owner.state.lock().unwrap().acquisitions, 2);
        assert_eq!(
            other_owner.requests(),
            other_before,
            "no global driver rearm"
        );
        let before = other_before.len();
        run(&other, &context).await;
        assert_eq!(
            &other_owner.requests()[before..],
            &[PluginServiceRequest::Diagnostics {
                consumer_id: other_owner.consumer(),
            }]
        );
        assert_eq!(other_owner.state.lock().unwrap().acquisitions, 1);
        assert!(other.lease.lock().unwrap().is_none());
        driver.stopped(context.clone()).await.unwrap();
    }
}

#[tokio::test]
async fn unconfirmed_indeterminate_control_reconciles_pause_without_redelivering_guest() {
    let fixture = Fixture::new();
    let owner = Owner::new("selected", CompleteResponse::ConfirmIndeterminate);
    let driver = fixture.driver(owner.clone());
    let context = context().await;
    *driver.parked_consumer.lock().unwrap() = Some(owner.consumer());
    *driver.unconfirmed_indeterminate.lock().unwrap() = Some("event".into());
    // A lost Complete(Indeterminate) reply does not prove a durable pause.
    // Script precisely that observation: same head, no pause, no active lease.
    run(&driver, &context).await;
    assert_eq!(
        owner.diagnostics().pause_reason,
        Some(PluginServicePauseReason::Indeterminate)
    );
    assert!(driver.unconfirmed_indeterminate.lock().unwrap().is_none());
    assert_eq!(
        driver.parked_consumer.lock().unwrap().as_ref(),
        Some(&owner.consumer())
    );
    assert!(driver.lease.lock().unwrap().is_none());
    assert!(!owner.requests().iter().any(|r| matches!(
        r,
        PluginServiceRequest::Poll { .. } | PluginServiceRequest::Host { .. }
    )));
    assert!(owner.requests().iter().any(|r| matches!(r, PluginServiceRequest::Complete { event_id, result: PluginServiceEventResult::Indeterminate, .. } if event_id == "event")));
    let before = owner.requests().len();
    run(&driver, &context).await;
    assert_eq!(
        &owner.requests()[before..],
        &[PluginServiceRequest::Diagnostics {
            consumer_id: owner.consumer()
        }]
    );
}

#[tokio::test]
async fn real_lifecycle_sends_exact_admit_and_publish_operations_and_honors_publish_denial() {
    let fixture = Fixture::new();
    let owner = Owner::new("selected", CompleteResponse::Lost);
    let driver = fixture.driver(owner.clone());
    driver.prepare_start().await.unwrap();
    let expected = vec![
        serde_json::json!({"kind":"prepare", "plan_id":"plan", "installation_id":"selected", "service_name":SERVICE, "operation":"admit"}),
        serde_json::json!({"kind":"prepare", "plan_id":"plan", "installation_id":"selected", "service_name":SERVICE, "operation":"publish"}),
    ];
    let prepare_wire = |requests: Vec<PluginServiceRequest>| {
        requests
            .into_iter()
            .filter(|request| matches!(request, PluginServiceRequest::Prepare { .. }))
            .map(|request| serde_json::to_value(request).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(prepare_wire(owner.requests()), expected);
    assert_eq!(
        owner.requests().len(),
        3,
        "Admit, guest lifecycle log, Publish"
    );
    assert!(owner.state.lock().unwrap().lease.is_none());
    owner.deny_publish.store(true, Ordering::SeqCst);
    let before = owner.requests().len();
    assert!(
        driver.prepare_start().await.is_err(),
        "Publish is not another Admit check"
    );
    assert_eq!(prepare_wire(owner.requests()[before..].to_vec()), expected);
}
