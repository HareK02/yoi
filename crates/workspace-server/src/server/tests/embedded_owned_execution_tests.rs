use super::*;
use crate::db_executor::{
    DbExecutor, DbExecutorLimits, DbOperationLifetime, with_operation_lifetime,
};
use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};

fn owner() -> DbExecutor {
    DbExecutor::new(DbExecutorLimits {
        workers: NonZeroUsize::new(1).unwrap(),
        queued: NonZeroUsize::new(1).unwrap(),
    })
    .unwrap()
}
async fn observed<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .expect("owned protocol stage did not progress")
}
struct Release(Option<mpsc::Sender<()>>);
impl Release {
    fn release(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.release();
    }
}
struct Lifetime(Arc<AtomicUsize>);
struct Retained(Arc<AtomicUsize>);
impl Drop for Retained {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl DbOperationLifetime for Lifetime {
    fn retain(&self) -> Box<dyn Send> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::new(Retained(self.0.clone()))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_embedded_endpoint_capture_keeps_actual_host_budget_off_tokio() {
    let (runtime, worker, execution) = runtime_with_worker_backend();
    let (started, held) = tokio::sync::oneshot::channel();
    let (tx, rx) = mpsc::channel();
    let mut release = Release(Some(tx));
    let thread = std::thread::spawn(move || {
        let _guard = execution.contexts.lock().unwrap();
        let _ = started.send(());
        let _ = rx.recv_timeout(Duration::from_secs(5));
    });
    observed(held).await.unwrap();
    let executor = owner();
    let leases = Arc::new(AtomicUsize::new(0));
    let mut capture = Box::pin(with_operation_lifetime(
        Arc::new(Lifetime(leases.clone())),
        connect_embedded_worker_protocol(
            embedded_protocol_test_source(&runtime, &worker),
            embedded_protocol_test_scope(),
            executor.clone(),
        ),
    ));
    assert!(
        futures::poll!(&mut capture).is_pending(),
        "endpoint Runtime lock must not run on Tokio"
    );
    observed(tokio::time::sleep(Duration::from_millis(1))).await;
    assert_eq!(
        observed(tokio::spawn(async { "control" })).await.unwrap(),
        "control"
    );
    drop(capture);
    assert_eq!(leases.load(Ordering::SeqCst), 1);
    executor.dispatch(|| {}).unwrap(); // only the one queued slot is left
    let error = executor.run(|| Ok::<_, Error>(())).await.unwrap_err();
    assert!(matches!(error, Error::DbCapacityExceeded));
    release.release();
    thread.join().unwrap();
    observed(executor.drain()).await.unwrap();
    assert_eq!(leases.load(Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_embedded_shutdown_commits_fs_receipt_after_the_pump_disappears() {
    let dir = tempfile::tempdir().unwrap();
    let execution = Arc::new(DeterministicExecutionBackend::default());
    let options = worker_runtime::fs_store::FsRuntimeStoreOptions {
        root: dir.path().join("runtime"),
        runtime_id: "owned-protocol".into(),
        display_name: None,
    };
    let runtime = worker_runtime::Runtime::with_fs_store_and_execution_backend(
        options.clone(),
        execution.clone(),
    )
    .unwrap();
    runtime.store_config_bundle(runtime_test_bundle()).unwrap();
    let worker = runtime
        .create_worker_scoped(&embedded_protocol_test_scope(), runtime_create_request())
        .unwrap()
        .worker_ref;
    let executor = owner();
    let mut connection = connect_embedded_worker_protocol(
        embedded_protocol_test_source(&runtime, &worker),
        embedded_protocol_test_scope(),
        executor.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(
        observed(connection.events.recv()).await.unwrap(),
        protocol::Event::Snapshot { .. }
    ));
    let (entered, held) = tokio::sync::oneshot::channel();
    let (tx, rx) = mpsc::channel();
    let mut release = Release(Some(tx));
    let endpoint = execution
        .contexts
        .lock()
        .unwrap()
        .get(&worker)
        .unwrap()
        .clone();
    let gate_finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished = gate_finished.clone();
    *endpoint.dispatch_hook.lock().unwrap() = Some(Box::new(move || {
        let _ = entered.send(());
        let _ = rx.recv_timeout(Duration::from_secs(5));
        finished.store(true, Ordering::SeqCst);
    }));
    connection
        .methods
        .send(protocol::Method::Shutdown {
            command: protocol::WorkerCommandEnvelope::new(700),
        })
        .await
        .unwrap();
    observed(held).await.unwrap(); // actual Runtime method, after its durable stop admission
    observed(tokio::time::sleep(Duration::from_millis(1))).await;
    assert_eq!(
        observed(tokio::spawn(async { "control" })).await.unwrap(),
        "control"
    );
    assert!(
        !gate_finished.load(Ordering::SeqCst),
        "watchdog bailout is not Tokio progress"
    );
    connection
        .termination
        .terminate(WorkerProtocolEnd::Disconnected);
    drop(connection);
    executor.dispatch(|| {}).unwrap();
    assert!(matches!(
        executor.run(|| Ok::<_, Error>(())).await.unwrap_err(),
        Error::DbCapacityExceeded
    ));
    release.release();
    observed(executor.drain()).await.unwrap();
    assert_eq!(
        runtime.worker_detail(&worker).unwrap().status,
        worker_runtime::catalog::WorkerStatus::Stopped
    );
    // Read the actual fs-store aggregate written by the accepted method. The
    // deterministic execution fixture has no restorable Profile archive, so
    // restarting it would test archive recovery rather than this commit owner.
    let record: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            options
                .root
                .join("workers")
                .join(worker.worker_id.to_string())
                .join("worker.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["status"], "stopped",
        "cancelled pump lost its fs-store commit: {record}"
    );
    assert_ne!(
        record["execution_state"]["state"],
        "reconciliation_required"
    );
}
