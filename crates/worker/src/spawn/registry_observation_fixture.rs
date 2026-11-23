//! Cross-crate registry/consumer fixture, excluded from default product builds.
//! Both registry forwarders and the published projection are real. A substituted
//! transport receiver forces loss/closure or injects overlapping state snapshots.
//! Loss uses the session state owner; overlap controls independent source sequences.
use super::*;
use crate::internal_worker::{
    InternalWorkerSessionHandle, InternalWorkerSessionStatus,
    test_internal_worker_session_with_greeting,
};

#[derive(Clone, Copy, Debug)]
pub enum Loss {
    Lagged,
    Closed,
    Shutdown,
}

pub async fn exercise(
    root: &std::path::Path,
    service: bool,
    loss: Loss,
    consume: impl FnMut(Event),
) -> (InternalWorkerSnapshot, Option<InternalWorkerSnapshot>) {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        exercise_inner(root, service, loss, consume),
    )
    .await
    .expect("registry/consumer fixture deadline")
}

/// Deliver deterministic full-snapshot/live overlap through either production
/// registry forwarder. `root` must be an existing isolated fixture directory.
/// `consume` receives, in stream order, the paired initial child Snapshot, a
/// parent Snapshot containing Running descendants, the child's Idle(seq4) and
/// grandchild's Idle(seq31) completion events, and the overlapping old parent
/// Snapshot. Each callback runs after the registry has applied that event.
///
/// Returns the normalized reconnect projection after overlap, without injecting
/// Shutdown/loss into the consumer. Nested IDs are `nested-session` and
/// `grandchild-session`; their finished request IDs are `child-run` and
/// `grandchild-run`. The top-level session ID comes from the session fixture.
/// Transport/state delivery is injected; registry reduction and forwarding are
/// real. Consumers may connect their own production reducer via the callback.
pub async fn exercise_snapshot_overlap(
    root: &std::path::Path,
    service: bool,
    consume: impl FnMut(Event),
) -> InternalWorkerSnapshot {
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        exercise_snapshot_overlap_inner(root, service, consume),
    )
    .await
    .expect("registry snapshot overlap fixture deadline")
}

async fn exercise_snapshot_overlap_inner(
    root: &std::path::Path,
    service: bool,
    mut consume: impl FnMut(Event),
) -> InternalWorkerSnapshot {
    let RegistryFixture {
        registry,
        child_tx,
        mut parent_rx,
        ..
    } = setup(root, service).await;
    consume(parent_rx.recv().await.unwrap()); // paired initial snapshot
    let mut parent = registry.internal_worker_snapshots().pop().unwrap();
    parent.state.as_mut().unwrap().state_sequence = 12;
    let mut child = parent.clone();
    child.worker.session_id = "nested-session".into();
    child.worker.parent_session_id = Some(parent.worker.session_id.clone());
    child.status = protocol::WorkerStatus::Running;
    let mut running: protocol::WorkerStateSnapshot = protocol::WorkerStatus::Running.into();
    running.state_sequence = 3;
    child.state = Some(running);
    let mut grandchild = child.clone();
    grandchild.worker.session_id = "grandchild-session".into();
    grandchild.worker.parent_session_id = Some(child.worker.session_id.clone());
    grandchild.state.as_mut().unwrap().state_sequence = 30;
    child.internal_workers = vec![grandchild.clone()];
    parent.internal_workers = vec![child.clone()];
    let mut child_finished: protocol::WorkerStateSnapshot = protocol::WorkerStatus::Idle.into();
    child_finished.state_sequence = 4;
    child_finished.last_finished_submission_request_id = Some("child-run".into());
    let mut grandchild_finished = child_finished.clone();
    grandchild_finished.state_sequence = 31;
    grandchild_finished.last_finished_submission_request_id = Some("grandchild-run".into());
    let events = [
        internal_worker_live_snapshot(parent.clone()).unwrap(),
        Event::InternalWorker {
            worker: child.worker.clone(),
            event: Box::new(Event::WorkerState {
                snapshot: child_finished,
            }),
        },
        Event::InternalWorker {
            worker: child.worker,
            event: Box::new(Event::InternalWorker {
                worker: grandchild.worker,
                event: Box::new(Event::WorkerState {
                    snapshot: grandchild_finished,
                }),
            }),
        },
        internal_worker_live_snapshot(parent).unwrap(),
    ];
    for event in events {
        child_tx.send(event).unwrap();
        consume(parent_rx.recv().await.unwrap());
    }
    registry.internal_worker_snapshots().pop().unwrap()
}

struct RegistryFixture {
    registry: Arc<SpawnedWorkerRegistry>,
    source: InternalWorkerSessionHandle,
    child_tx: broadcast::Sender<Event>,
    parent_rx: broadcast::Receiver<Event>,
}

async fn setup(root: &std::path::Path, service: bool) -> RegistryFixture {
    let registry = SpawnedWorkerRegistry::new_internal(
        "parent".into(),
        SharedScope::new(manifest::Scope::empty()),
    );
    let (parent_tx, parent_rx) = broadcast::channel(64);
    registry.attach_parent_protocol(parent_tx, "parent-session".into());
    let (source, _source_tx) = test_internal_worker_session_with_greeting(
        InternalWorkerVisibility::ParentClient,
        Some(protocol::Greeting {
            worker_name: "child".into(),
            cwd: root.to_string_lossy().into(),
            provider: "fixture".into(),
            model: "none".into(),
            reasoning: None,
            scope_summary: String::new(),
            tools: vec![],
            context_window: 0,
            context_tokens: 0,
            context_usage: None,
        }),
    );
    let (child_tx, child_rx) = broadcast::channel(8);
    if service {
        let record = InternalServiceWorkerRecord::new("fixture", "child", source.clone());
        *record.published.events.lock().unwrap() = Some(child_rx);
        registry
            .service_records
            .lock()
            .unwrap()
            .push(record.clone());
        registry.start_service_protocol_forwarding(record);
    } else {
        let scope = SharedScope::new(
            manifest::Scope::from_config(&manifest::ScopeConfig {
                allow: vec![ScopeRule {
                    target: root.into(),
                    permission: Permission::Read,
                    recursive: true,
                    symlink_policy: Default::default(),
                }],
                deny: vec![],
            })
            .unwrap(),
        );
        let broker = workdir::WorkdirToolBroker::new(Arc::new(
            workdir::LocalWorkdirSession::materialized_bound(
                workdir::Workdir::new("observation-fixture"),
                root.into(),
                root.into(),
                scope,
                workdir::WorkdirSessionCapabilities::ALL,
            ),
        ));
        let lease = broker
            .scope(workdir::WorkdirToolScope {
                rules: vec![workdir::WorkdirToolScopeRule {
                    target: workdir::WorkdirPath::new("").unwrap(),
                    permission: workdir::WorkdirToolScopePermission::Read,
                    recursive: true,
                    symlink_policy: Default::default(),
                }],
                cwd: workdir::WorkdirPath::new("").unwrap(),
                command: false,
            })
            .await
            .unwrap();
        let record = InternalSpawnedWorkerRecord::new(
            "child".into(),
            vec![],
            Arc::new(
                WorkdirScopeLeaseSet::from_single(
                    workdir::WorkdirAttachmentAlias::new("main").unwrap(),
                    lease,
                )
                .unwrap(),
            ),
            #[cfg(test)]
            vec![],
            source.clone(),
            SpawnedWorkerRegistry::new_internal(
                "child".into(),
                SharedScope::new(manifest::Scope::empty()),
            ),
            None,
        );
        *record.published.events.lock().unwrap() = Some(child_rx);
        registry
            .internal_records
            .lock()
            .unwrap()
            .push(record.clone());
        registry.start_protocol_forwarding(record);
    }
    RegistryFixture {
        registry,
        source,
        child_tx,
        parent_rx,
    }
}

async fn exercise_inner(
    root: &std::path::Path,
    service: bool,
    loss: Loss,
    mut consume: impl FnMut(Event),
) -> (InternalWorkerSnapshot, Option<InternalWorkerSnapshot>) {
    let RegistryFixture {
        registry,
        source,
        child_tx,
        mut parent_rx,
    } = setup(root, service).await;
    consume(parent_rx.recv().await.unwrap());
    source.force_status(InternalWorkerSessionStatus::Running);
    child_tx
        .send(Event::WorkerState {
            snapshot: source.protocol_snapshot().state.unwrap(),
        })
        .unwrap();
    consume(parent_rx.recv().await.unwrap());
    child_tx
        .send(Event::Error {
            code: protocol::ErrorCode::Internal,
            message: "ordinary diagnostic".into(),
        })
        .unwrap();
    consume(parent_rx.recv().await.unwrap());
    assert!(registry.internal_worker_snapshots()[0].state.is_some());
    match loss {
        Loss::Lagged => {
            // No await: a current-thread executor cannot drain the receiver
            // between these sends, so overflow is deterministic.
            for _ in 0..32 {
                child_tx
                    .send(Event::TextDelta {
                        text: "overflow".into(),
                    })
                    .unwrap();
            }
        }
        Loss::Closed => drop(child_tx),
        Loss::Shutdown => {
            child_tx.send(Event::Shutdown).unwrap();
        }
    }
    let event = parent_rx.recv().await.unwrap();
    assert!(
        matches!(&event, Event::InternalWorkerObservationUnavailable { .. })
            || matches!(&event, Event::InternalWorker { event, .. } if matches!(**event, Event::Shutdown))
    );
    consume(event);
    while let Ok(event) = parent_rx.try_recv() {
        consume(event);
    }
    let lost = registry.internal_worker_snapshots().pop().unwrap();
    assert!(lost.state.is_none());
    let fresh = if matches!(loss, Loss::Shutdown) {
        None
    } else {
        // A new paired subscription is the recovery boundary, not an old
        // queued WorkerState or child Snapshot from the closed forwarder.
        let reconnect = SpawnedWorkerRegistry::new_internal(
            "parent".into(),
            SharedScope::new(manifest::Scope::empty()),
        );
        let (tx, _rx) = broadcast::channel(8);
        reconnect.attach_parent_protocol(tx, "parent-session".into());
        reconnect
            .attach_service(InternalServiceWorkerRecord::new("fixture", "child", source))
            .unwrap();
        Some(reconnect.internal_worker_snapshots().pop().unwrap())
    };
    (lost, fresh)
}
