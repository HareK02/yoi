// Included in the delivery suite; the Runtime execution boundary is scripted.
// These cases prove Server reservation/pin atomicity, not Component execution.
#[tokio::test]
async fn worker_create_seals_plan_and_replay_retains_exact_snapshot_after_revoke() {
    let f = DeliveryFixture::new(false).await;
    let runtime = WorkdirlessFixtureRuntime::default();
    f.api.runtime.register_or_replace(runtime.clone());
    let worker = delivery_worker_profile(&f.api, None, "project:delivery").await;
    let saved = f
        .api
        .config_store
        .with_conn(|conn| {
            crate::plugin_plan::worker_launch_plan(
                conn,
                TEST_WORKSPACE_ID,
                &worker.runtime_id,
                &worker.worker_id,
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(saved.pin_state, PlanPinState::Active);
    assert_eq!(saved.plan.plugins(), f.saved.plan.plugins());
    assert_eq!(saved.plan.execution().worker_id, worker.worker_id);
    let (allocation, request_fingerprint): (String, String) = f.api.config_store.with_conn(|conn| {
        Ok(conn.query_row("SELECT allocation_key,request_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND worker_id=?2", rusqlite::params![TEST_WORKSPACE_ID, worker.worker_id], |row| Ok((row.get(0)?,row.get(1)?)))?)
    }).unwrap();
    let settings = f
        .api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = f
        .api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            &worker.runtime_id,
            &allocation,
            &request_fingerprint,
            None,
            &settings,
        )
        .unwrap();
    assert!(
        f.api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                &worker.runtime_id,
                &allocation,
                "different-plan",
                None,
                &settings
            )
            .is_err()
    );
    let request = runtime.spawn_requests().pop().unwrap();
    f.api
        .config_store
        .plugin_revoke(
            TEST_WORKSPACE_ID,
            "delivery-host",
            &f.request.installation_id,
            &PluginRevokeRequest {
                expected_last_operation: f.installed.operation_id.clone(),
                operation_key: "revoke-launch".into(),
                artifact_digest: f.request.artifact_digest.clone(),
                reason: "withdraw".into(),
            },
        )
        .unwrap();
    let binding = f
        .api
        .plugin_launch_binding(&worker.runtime_id, &request, &reservation)
        .unwrap();
    let replay = binding.plugin_execution.unwrap();
    assert_eq!(replay.plan_id, saved.plan_id);
    assert_eq!(replay.fingerprint, saved.fingerprint);
    assert_eq!(replay.plan, saved.plan);
    let local = f.api.plugin_artifact_source(worker.clone()).await.unwrap();
    let pin = &replay.plan.plugins()[0];
    assert!(
        local
            .authorize(&PluginArtifactDeliveryRequest {
                plan_id: replay.plan_id.clone(),
                plan_fingerprint: replay.fingerprint,
                installation_id: pin.installation_id().into(),
                artifact_digest: pin.artifact_digest().into()
            })
            .await
            .is_err()
    );
    f.api
        .store
        .delete_worker_registry(TEST_WORKSPACE_ID, &worker)
        .unwrap();
    assert_eq!(
        f.api
            .config_store
            .plugin_plan_load(TEST_WORKSPACE_ID, &saved.plan_id)
            .unwrap()
            .pin_state,
        PlanPinState::Released
    );
}

#[tokio::test]
async fn rejected_worker_create_releases_sealed_plugin_pins_without_registry_publication() {
    let f = DeliveryFixture::new(false).await;
    let runtime = WorkdirlessFixtureRuntime::default();
    runtime.reject_next_spawn();
    f.api.runtime.register_or_replace(runtime);
    assert!(
        create_workspace_worker(
            State(f.api.clone()),
            HeaderMap::new(),
            Json(delivery_create_request(None, "project:delivery"))
        )
        .await
        .is_err()
    );
    let plans: Vec<(String,String)> = f.api.config_store.with_conn(|conn| {
        let mut statement = conn.prepare("SELECT plan_id,pin_state FROM plugin_execution_plans WHERE workspace_id=?1 AND actor=?2")?;
        Ok(statement.query_map(rusqlite::params![TEST_WORKSPACE_ID, crate::plugin_plan::WORKER_LAUNCH_ACTOR], |row| Ok((row.get(0)?,row.get(1)?)))?.collect::<std::result::Result<Vec<_>,_>>()?)
    }).unwrap();
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].1, "released");
    let saved = f
        .api
        .config_store
        .plugin_plan_load(TEST_WORKSPACE_ID, &plans[0].0)
        .unwrap();
    let worker = RuntimeWorkerRef::new(
        saved.plan.execution().runtime_id.clone(),
        saved.plan.execution().worker_id.clone(),
    );
    assert!(
        f.api
            .store
            .get_worker_registry(TEST_WORKSPACE_ID, &worker)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn empty_plugin_selection_is_sealed_and_retained_on_exact_replay() {
    let f = DeliveryFixture::new(false).await;
    let runtime = WorkdirlessFixtureRuntime::default();
    f.api.runtime.register_or_replace(runtime.clone());
    let worker = delivery_worker_profile(&f.api, None, "builtin:companion").await;
    let selected = f
        .api
        .config_store
        .with_conn(|conn| {
            crate::plugin_plan::worker_launch_selection(
                conn,
                TEST_WORKSPACE_ID,
                &worker.runtime_id,
                &worker.worker_id,
            )
        })
        .unwrap();
    assert!(matches!(selected, Some(None)));
    let (allocation, request_fingerprint): (String, String) = f.api.config_store.with_conn(|conn| {
        Ok(conn.query_row("SELECT allocation_key,request_fingerprint FROM worker_create_reservations WHERE workspace_id=?1 AND worker_id=?2", rusqlite::params![TEST_WORKSPACE_ID, worker.worker_id], |row| Ok((row.get(0)?,row.get(1)?)))?)
    }).unwrap();
    let settings = f
        .api
        .config_store
        .get_workspace_memory_settings(TEST_WORKSPACE_ID)
        .unwrap();
    let reservation = f
        .api
        .config_store
        .reserve_worker_create(
            TEST_WORKSPACE_ID,
            &worker.runtime_id,
            &allocation,
            &request_fingerprint,
            None,
            &settings,
        )
        .unwrap();
    let request = runtime.spawn_requests().pop().unwrap();
    let replay = f
        .api
        .plugin_launch_binding(&worker.runtime_id, &request, &reservation)
        .unwrap();
    assert!(replay.plugin_execution.is_none());
    assert_eq!(replay.create_fingerprint, reservation.create_fingerprint);
}
