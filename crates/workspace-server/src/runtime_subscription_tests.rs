use super::*;
use protocol::subscription::{SubscriptionWorkerIds, SubscriptionWorkerState};
use worker_runtime::Runtime;
use worker_runtime::catalog::{
    CreateWorkerRequest, ProfileSelector, ProfileSourceArchiveHttpRef, ProfileSourceArchiveSource,
};
use worker_runtime::execution::{
    WorkerExecutionBackend, WorkerExecutionHandle, WorkerExecutionOperation, WorkerExecutionResult,
    WorkerExecutionRunState, WorkerExecutionSpawnRequest, WorkerExecutionSpawnResult,
};
use worker_runtime::profile_archive::{ProfileSourceArchiveRef, ProfileSourceGraphSummary};

#[derive(Debug)]
struct TestExecutionBackend;

impl WorkerExecutionBackend for TestExecutionBackend {
    fn backend_id(&self) -> &str {
        "runtime-subscription-test"
    }

    fn spawn_worker(&self, request: WorkerExecutionSpawnRequest) -> WorkerExecutionSpawnResult {
        WorkerExecutionSpawnResult::connected(
            WorkerExecutionHandle::new(request.worker_ref, self.backend_id()),
            WorkerExecutionRunState::Idle,
            None,
        )
    }

    fn dispatch_input(
        &self,
        _handle: &WorkerExecutionHandle,
        _input: worker_runtime::interaction::WorkerInput,
    ) -> WorkerExecutionResult {
        WorkerExecutionResult::accepted(
            WorkerExecutionOperation::Input,
            WorkerExecutionRunState::Busy,
        )
    }

    fn stop_worker(&self, _handle: &WorkerExecutionHandle) -> WorkerExecutionResult {
        WorkerExecutionResult::accepted(
            WorkerExecutionOperation::Stop,
            WorkerExecutionRunState::Stopped,
        )
    }
}

const TOKEN: &str = "runtime-subscription-test-token";

fn create_request(name: &str) -> CreateWorkerRequest {
    CreateWorkerRequest {
        idempotency_key: None,
        idempotency_fingerprint: None,
        profile: ProfileSelector::Builtin("builtin:companion".to_string()),
        display_name: Some(name.to_string()),
        config_bundle: None,
        profile_source: ProfileSourceArchiveSource::Http {
            location: ProfileSourceArchiveHttpRef {
                url: "http://127.0.0.1/profiles/test".to_string(),
                etag: None,
                archive: ProfileSourceArchiveRef {
                    id: "test-profile-source".to_string(),
                    digest: "test-digest".to_string(),
                    size_bytes: 0,
                    source_graph: ProfileSourceGraphSummary {
                        source_count: 0,
                        total_source_bytes: 0,
                        entrypoints: std::collections::BTreeMap::new(),
                        import_count: 0,
                    },
                },
            },
        },
        initial_input: None,
        working_directory_request: None,
        working_directory: None,
        worker_observation_enabled: false,
        worker_observation_grants: Vec::new(),
        workspace_api: None,
    }
}

async fn start_runtime_server(
    listener: tokio::net::TcpListener,
    runtime: Runtime,
) -> tokio::task::JoinHandle<()> {
    let router = worker_runtime::http_server::runtime_http_router(runtime, TOKEN.to_string());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    })
}

async fn fixture() -> (Runtime, RemoteRuntimeConfig, tokio::task::JoinHandle<()>) {
    let runtime = Runtime::with_execution_backend(
        worker_runtime::RuntimeOptions::default(),
        std::sync::Arc::new(TestExecutionBackend),
    )
    .unwrap();
    runtime
        .create_worker_scoped(
            &worker_runtime::RuntimeWorkspaceScope::new("local", "local-token"),
            create_request("fixture"),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = start_runtime_server(listener, runtime.clone()).await;
    let config = RemoteRuntimeConfig::new(
        "runtime-test",
        "Runtime test",
        format!("http://{address}"),
        Some(TOKEN.to_string()),
    );
    (runtime, config, task)
}

async fn next_event(subscription: &mut BrokerSubscription) -> BrokerSubscriptionEvent {
    tokio::time::timeout(Duration::from_secs(5), subscription.recv())
        .await
        .expect("subscription event timed out")
        .expect("subscription closed")
}

async fn next_snapshot(subscription: &mut BrokerSubscription) -> BrokerSubscriptionEvent {
    loop {
        let event = next_event(subscription).await;
        if matches!(event, BrokerSubscriptionEvent::Snapshot { .. }) {
            return event;
        }
    }
}

async fn wait_for_status(
    broker: &RuntimeSubscriptionBroker,
    runtime_id: &str,
    predicate: impl Fn(&RuntimeSubscriptionBrokerStatus) -> bool,
) -> RuntimeSubscriptionBrokerStatus {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(status) = broker.status(runtime_id) {
                if predicate(&status) {
                    return status;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("broker status timed out")
}

#[tokio::test]
async fn equal_downstream_selectors_share_one_upstream_subscription() {
    let (runtime, config, server) = fixture().await;
    let worker = runtime.list_workers().unwrap().remove(0);
    let broker = RuntimeSubscriptionBroker::new("local");
    broker.register_remote_runtime(config);
    let selector = EventSubscriptionSelector::WorkerLifecycle {
        worker_ids: SubscriptionWorkerIds::new([
            protocol::subscription::SubscriptionWorkerId::new(
                worker.worker_ref.worker_id.to_string(),
            )
            .unwrap(),
        ])
        .unwrap(),
    };
    let mut first = broker.subscribe("runtime-test", selector.clone()).unwrap();
    let mut second = broker.subscribe("runtime-test", selector.clone()).unwrap();
    assert!(matches!(
        next_snapshot(&mut first).await,
        BrokerSubscriptionEvent::Snapshot { .. }
    ));
    assert!(matches!(
        next_snapshot(&mut second).await,
        BrokerSubscriptionEvent::Snapshot { .. }
    ));
    let status = wait_for_status(&broker, "runtime-test", |status| {
        status.upstream_subscriptions == 1
    })
    .await;
    assert_eq!(status.desired_selectors, 1);

    runtime
        .observe_worker_event(
            &worker.worker_ref,
            protocol::Event::Status {
                status: protocol::WorkerStatus::Running,
            },
        )
        .unwrap();
    for subscription in [&mut first, &mut second] {
        assert!(matches!(
            next_event(subscription).await,
            BrokerSubscriptionEvent::Event {
                payload: SubscriptionEventPayload::WorkerUpserted { ref worker },
                ..
            } if worker.state == SubscriptionWorkerState::Running
        ));
    }
    let mut late = broker.subscribe("runtime-test", selector.clone()).unwrap();
    let BrokerSubscriptionEvent::Snapshot { snapshot, .. } = next_snapshot(&mut late).await else {
        panic!("expected cached snapshot for late subscriber");
    };
    assert!(matches!(
        snapshot,
        SubscriptionSnapshot::Workers { workers }
            if workers.iter().any(|worker| worker.state == SubscriptionWorkerState::Running)
    ));
    drop(late);

    drop(first);
    tokio::task::yield_now().await;
    assert_eq!(
        broker
            .status("runtime-test")
            .unwrap()
            .upstream_subscriptions,
        1
    );
    drop(second);
    wait_for_status(&broker, "runtime-test", |status| {
        status.upstream_subscriptions == 0 && status.desired_selectors == 0
    })
    .await;
    server.abort();
}

#[tokio::test]
async fn replacing_runtime_registration_fences_the_old_generation() {
    let (_runtime, config, server) = fixture().await;
    let broker = RuntimeSubscriptionBroker::new("local");
    let first_generation = broker.register_remote_runtime(config.clone());
    let mut old = broker
        .subscribe("runtime-test", EventSubscriptionSelector::RuntimeWorkers)
        .unwrap();
    assert!(matches!(
        next_snapshot(&mut old).await,
        BrokerSubscriptionEvent::Snapshot { .. }
    ));

    let second_generation = broker.register_remote_runtime(config);
    assert!(second_generation > first_generation);
    assert!(matches!(
        next_event(&mut old).await,
        BrokerSubscriptionEvent::Closed {
            connection_generation,
            ..
        } if connection_generation == first_generation
    ));
    let status = wait_for_status(&broker, "runtime-test", |status| {
        status.connection_generation == second_generation && status.connected
    })
    .await;
    assert_eq!(status.connection_generation, second_generation);
    server.abort();
}

#[tokio::test]
async fn reconnect_resubscribes_and_replaces_state_from_fresh_snapshot() {
    let (runtime, config, server) = fixture().await;
    let address = config
        .base_url
        .strip_prefix("http://")
        .unwrap()
        .parse::<std::net::SocketAddr>()
        .unwrap();
    server.abort();
    tokio::task::yield_now().await;

    let broker = RuntimeSubscriptionBroker::new("local");
    broker.register_remote_runtime(config);
    let mut subscription = broker
        .subscribe("runtime-test", EventSubscriptionSelector::RuntimeWorkers)
        .unwrap();
    assert!(matches!(
        next_event(&mut subscription).await,
        BrokerSubscriptionEvent::Disconnected { .. }
    ));

    runtime
        .create_worker_scoped(
            &worker_runtime::RuntimeWorkspaceScope::new("local", "local-token"),
            create_request("after-reconnect"),
        )
        .unwrap();
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let restarted = start_runtime_server(listener, runtime).await;
    let event = next_snapshot(&mut subscription).await;
    let BrokerSubscriptionEvent::Snapshot { snapshot, .. } = event else {
        panic!("expected fresh snapshot after reconnect");
    };
    let SubscriptionSnapshot::Workers { workers } = snapshot else {
        panic!("expected Worker snapshot");
    };
    assert_eq!(workers.len(), 2);
    restarted.abort();
}

#[tokio::test]
async fn embedded_runtime_uses_in_process_subscription_source() {
    let (runtime, _config, server) = fixture().await;
    let worker = runtime.list_workers().unwrap().remove(0);
    let broker = RuntimeSubscriptionBroker::new("local");
    broker.register_embedded_runtime("embedded-worker-runtime", runtime.clone());
    assert_eq!(broker.runtime_ids(), vec!["embedded-worker-runtime"]);
    let mut subscription = broker
        .subscribe(
            "embedded-worker-runtime",
            EventSubscriptionSelector::RuntimeWorkers,
        )
        .unwrap();
    let BrokerSubscriptionEvent::Snapshot { snapshot, .. } = next_snapshot(&mut subscription).await
    else {
        panic!("expected embedded snapshot");
    };
    let SubscriptionSnapshot::Workers { workers } = snapshot else {
        panic!("expected Worker snapshot");
    };
    assert_eq!(
        workers[0].runtime_id.as_deref(),
        Some("embedded-worker-runtime")
    );
    runtime
        .observe_worker_event(
            &worker.worker_ref,
            protocol::Event::Status {
                status: protocol::WorkerStatus::Running,
            },
        )
        .unwrap();
    assert!(matches!(next_event(&mut subscription).await,
        BrokerSubscriptionEvent::Event { payload: SubscriptionEventPayload::WorkerUpserted { worker }, .. }
        if worker.runtime_id.as_deref() == Some("embedded-worker-runtime") && worker.state == SubscriptionWorkerState::Running));
    let mut late = broker
        .subscribe(
            "embedded-worker-runtime",
            EventSubscriptionSelector::RuntimeWorkers,
        )
        .unwrap();
    let BrokerSubscriptionEvent::Snapshot { snapshot, .. } = next_snapshot(&mut late).await else {
        panic!("expected cached embedded snapshot for late subscriber");
    };
    assert!(matches!(
        snapshot,
        SubscriptionSnapshot::Workers { workers }
            if workers.iter().any(|worker| worker.state == SubscriptionWorkerState::Running)
    ));

    runtime
        .stop_worker(&worker.worker_ref, Some("done".to_string()))
        .unwrap();
    assert!(matches!(
        next_event(&mut subscription).await,
        BrokerSubscriptionEvent::Event {
            payload: SubscriptionEventPayload::WorkerUpserted { .. },
            ..
        }
    ));
    runtime.delete_worker(&worker.worker_ref).unwrap();
    assert!(matches!(
        next_event(&mut subscription).await,
        BrokerSubscriptionEvent::Event {
            payload: SubscriptionEventPayload::WorkerRemoved { .. },
            ..
        }
    ));
    let mut after_remove = broker
        .subscribe(
            "embedded-worker-runtime",
            EventSubscriptionSelector::RuntimeWorkers,
        )
        .unwrap();
    let BrokerSubscriptionEvent::Snapshot { snapshot, .. } = next_snapshot(&mut after_remove).await
    else {
        panic!("expected cached embedded snapshot after remove");
    };
    assert!(matches!(
        snapshot,
        SubscriptionSnapshot::Workers { workers }
            if workers.iter().all(|candidate| candidate.worker_id.as_str() != worker.worker_ref.worker_id.to_string())
    ));
    server.abort();
}
