use runtime_api::{WorkerDetail, WorkerLifecycleAck, WorkerLifecycleState, WorkerSummary};

#[test]
fn worker_wire_lifecycle_is_explicit_and_independent_of_last_catalog_status() {
    let worker_id = "01968f08-2122-7000-8000-000000000001";
    let summary = serde_json::json!({
        "worker_ref": {"worker_id": worker_id},
        "worker_id": worker_id,
        "status": "running",
        "execution_metadata_available": true,
        "profile": {"kind": "builtin", "value": "default"},
        "profile_source": {
            "id": "profile", "digest": "digest", "size_bytes": 0,
            "source_graph": {
                "source_count": 0, "total_source_bytes": 0,
                "entrypoints": {}, "import_count": 0
            }
        }
    });
    let ack = serde_json::json!({
        "worker_ref": {"worker_id": worker_id},
        "status": "running", "replayed": true
    });
    assert!(serde_json::from_value::<WorkerSummary>(summary.clone()).is_err());
    assert!(serde_json::from_value::<WorkerDetail>(summary.clone()).is_err());
    assert!(serde_json::from_value::<WorkerLifecycleAck>(ack.clone()).is_err());

    for (lifecycle, wire) in [
        (WorkerLifecycleState::NotStarted, "not_started"),
        (WorkerLifecycleState::AwaitingRestore, "awaiting_restore"),
        (WorkerLifecycleState::Active, "active"),
        (WorkerLifecycleState::Stopped, "stopped"),
        (WorkerLifecycleState::Creating, "creating"),
        (WorkerLifecycleState::Restoring, "restoring"),
        (WorkerLifecycleState::Stopping, "stopping"),
        (WorkerLifecycleState::RecoveryRequired, "recovery_required"),
    ] {
        assert_eq!(serde_json::to_value(lifecycle).unwrap(), wire);
        let mut summary = summary.clone();
        summary["lifecycle"] = wire.into();
        let parsed: WorkerDetail = serde_json::from_value(summary.clone()).unwrap();
        assert_eq!(parsed.lifecycle, lifecycle);
        assert_eq!(serde_json::to_value(parsed).unwrap(), summary);
        let mut ack = ack.clone();
        ack["lifecycle"] = wire.into();
        let parsed: WorkerLifecycleAck = serde_json::from_value(ack.clone()).unwrap();
        assert_eq!(parsed.lifecycle, lifecycle);
        assert_eq!(serde_json::to_value(parsed).unwrap(), ack);
    }

    let mut invalid = summary;
    invalid["lifecycle"] = "running".into();
    assert!(serde_json::from_value::<WorkerSummary>(invalid).is_err());
}
