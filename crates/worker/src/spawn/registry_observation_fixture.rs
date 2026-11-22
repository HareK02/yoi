//! Cross-crate registry/consumer fixture, excluded from default product builds.
//! The session state owner and both registry forwarders are real; only their
//! transport receiver is substituted to deterministically force loss/closure.
use super::*;
use crate::internal_worker::{
    InternalWorkerSessionStatus, test_internal_worker_session_with_greeting,
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

async fn exercise_inner(
    root: &std::path::Path,
    service: bool,
    loss: Loss,
    mut consume: impl FnMut(Event),
) -> (InternalWorkerSnapshot, Option<InternalWorkerSnapshot>) {
    let registry = SpawnedWorkerRegistry::new_internal(
        "parent".into(),
        SharedScope::new(manifest::Scope::empty()),
    );
    let (parent_tx, mut parent_rx) = broadcast::channel(64);
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
