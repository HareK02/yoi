use runtime_api::{
    WorkerDetail, WorkerLifecycleAck, WorkerLifecycleState, WorkerObservationState, WorkerSummary,
};

const WORKER_ID: &str = "01968f08-2122-7000-8000-000000000001";

fn worker_summary_wire() -> serde_json::Value {
    serde_json::json!({
        "worker_ref": {"worker_id": WORKER_ID},
        "worker_id": WORKER_ID,
        "lifecycle": "active",
        "observation": {"kind": "unavailable"},
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
    })
}

#[test]
fn worker_wire_lifecycle_is_explicit_and_independent_of_last_catalog_status() {
    let mut summary = worker_summary_wire();
    summary.as_object_mut().unwrap().remove("lifecycle");
    let ack = serde_json::json!({
        "worker_ref": {"worker_id": WORKER_ID},
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

#[test]
fn worker_wire_absent_observation_defaults_to_unavailable() {
    let mut wire = worker_summary_wire();
    wire.as_object_mut().unwrap().remove("observation");

    let summary: WorkerSummary = serde_json::from_value(wire.clone()).unwrap();
    let detail: WorkerDetail = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(summary.observation, WorkerObservationState::Unavailable);
    assert_eq!(detail.observation, WorkerObservationState::Unavailable);
    assert_eq!(summary.observation, WorkerObservationState::default());

    wire["observation"] = serde_json::json!({"kind": "unavailable"});
    assert_eq!(serde_json::to_value(&summary).unwrap(), wire);
    assert_eq!(serde_json::to_value(&detail).unwrap(), wire);
    assert_eq!(
        serde_json::from_value::<WorkerSummary>(wire).unwrap(),
        summary
    );
}

#[test]
fn worker_wire_current_observation_source_sequence_round_trips() {
    for state_sequence in [0, 42, u64::MAX] {
        let mut wire = worker_summary_wire();
        wire["observation"] =
            serde_json::json!({"kind": "current", "state_sequence": state_sequence});

        let summary: WorkerSummary = serde_json::from_value(wire.clone()).unwrap();
        let detail: WorkerDetail = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            summary.observation,
            WorkerObservationState::Current { state_sequence }
        );
        assert_eq!(detail.observation, summary.observation);
        assert_eq!(serde_json::to_value(summary).unwrap(), wire);
        assert_eq!(serde_json::to_value(detail).unwrap(), wire);
    }
}

#[test]
fn worker_wire_unknown_observation_kind_is_rejected() {
    let observation = serde_json::json!({"kind": "future", "state_sequence": 42});
    assert!(serde_json::from_value::<WorkerObservationState>(observation.clone()).is_err());

    let mut wire = worker_summary_wire();
    wire["observation"] = observation;
    assert!(serde_json::from_value::<WorkerSummary>(wire.clone()).is_err());
    assert!(serde_json::from_value::<WorkerDetail>(wire).is_err());
}
