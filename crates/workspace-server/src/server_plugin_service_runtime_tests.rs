// Included in plugin_runtime_tests: real Ticket DB/outbox -> Feature HostReady
// task registry -> pinned artifact delivery -> authored Rust Service Component ->
// signed Remote/Embedded owner -> Ticket read/data/TLS receipts -> cursor commit.
// Worker allocation uses the existing isolated fixture; the pump/controller and
// all event/effect owners below are production adapters, not guest/owner doubles.
mod service_connection {
    use super::*;
    use rusqlite::OptionalExtension;
    use server_api::authorization::OperationAuthorizationRequest;
    use server_api::*;
    use std::time::Duration;

    const SERVICE: &str = "ticket_events";

    async fn fixture(peer: &Peer) -> Fixture {
        fixture_with_ticket_access(peer, true).await
    }

    async fn fixture_with_ticket_access(peer: &Peer, ticket_access: bool) -> Fixture {
        let mut binding = peer.binding();
        binding.methods = vec![PluginHttpMethod::Post];
        binding.path_prefixes = vec!["/events".into()];
        let mut f = Fixture::with_package(
            Some(binding),
            include_str!("../../plugin-runtime/tests/service-guest/plugin.toml").into(),
            include_bytes!("../tests/fixtures/ticket-service-v2.component.wasm"),
            SERVICE,
            Some(json!({
                "tools":[], "services":[SERVICE], "ticket_events":true,
                "logging":true, "ticket_read":ticket_access,
                "ticket_data_read":if ticket_access { json!([{"collection":"notes","schema_id":"note","schema_version":"1"}]) } else { json!([]) },
                "ticket_data_write":if ticket_access { json!([{"collection":"notes","schema_id":"note","schema_version":"1"}]) } else { json!([]) },
                "config":{"mode":"normal"}
            })),
        ).await;
        f.api.plugin_https_transport = Some(Arc::new(peer.transport()));
        allow_artifact_read(&f);
        f
    }

    #[tokio::test]
    async fn service_check_reuses_physical_subject_database_only_within_each_sync_scope() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let owner = f.owner(None).await;
        for _ in 0..2 {
            let before = f.api.subjektiv_registration.open_observation();
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::Prepare {
                        plan_id: f.plan.plan_id.clone(),
                        installation_id: f.installed.installation.installation_id.clone(),
                        service_name: SERVICE.into(),
                        operation: PluginServiceLifecycleOperation::Admit,
                    }
                )
                .await
                .unwrap(),
                PluginServiceResponse::Prepared
            );
            assert_check_physical_initializations(&f, before, 2);
        }
        assert_eq!(peer.connection_count(), 0);
        f.shutdown().await;
    }

    async fn request(
        owner: &dyn PluginHostOwner,
        request: PluginServiceRequest,
    ) -> std::result::Result<PluginServiceResponse, PluginHostError> {
        match owner.call(PluginHostRequest::Service { request }).await? {
            PluginHostResponse::Service { response } => Ok(response),
            other => panic!("unexpected owner response: {other:?}"),
        }
    }

    fn acquire_request(f: &Fixture) -> PluginServiceRequest {
        PluginServiceRequest::Acquire {
            plan_id: f.plan.plan_id.clone(),
            installation_id: f.installed.installation.installation_id.clone(),
            service_name: SERVICE.into(),
        }
    }

    fn state(f: &Fixture) -> Option<(String, i64, Option<String>, Option<String>)> {
        f.api.config_store.with_conn(|conn| {
            Ok(conn.query_row("SELECT consumer_id,cursor,pause_reason,lease_token FROM plugin_service_consumers WHERE plan_id=?1",[&f.plan.plan_id],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?)
        }).unwrap()
    }

    fn effect_states(f: &Fixture) -> Vec<(String, Option<String>)> {
        f.api.config_store.with_conn(|conn| {
            Ok(conn.prepare("SELECT state,json_extract(error_json,'$.code') FROM plugin_effect_receipts ORDER BY rowid")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?)
        }).unwrap()
    }

    async fn until(f: &Fixture, phase: &str, mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(45), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!(
            "Service {phase} did not converge: state={:?}, invocations={}, data_receipts={}, http_receipts={}, effect_states={:?}",
            state(f), f.count("plugin_invocations"), f.count("plugin_data_receipts"), f.count("plugin_effect_receipts"), effect_states(f),
        ));
    }

    async fn service_host(
        f: &Fixture,
        artifacts: Arc<dyn ArtifactSource>,
        owner: Arc<dyn PluginHostOwner>,
        incarnation: &str,
    ) -> worker::Worker<
        Model,
        session_store::CombinedStore<session_store::FsStore, session_store::FsWorkerStore>,
    > {
        let worker_ref = WorkerRef::try_from(&f.source).unwrap();
        let bridge = PinnedPluginToolRuntime::new(
            Arc::new(RuntimeProviders {
                worker: worker_ref.clone(),
                artifacts,
                owner,
            }),
            &f._directory.path().join("service-cache"),
        )
        .unwrap();
        let manifest = worker::WorkerManifest::from_toml(&format!(
            "[worker]\nname=\"{}\"\n[model]\nscheme=\"anthropic\"\nmodel_id=\"scripted\"\n[engine]\n[scope]\nallow=[]\n", f.source.worker_id)).unwrap();
        let mut host = worker::Worker::new(
            manifest,
            agen::Engine::new_annotated(Model {
                tool_name: "no-model-call".into(),
                requests: Arc::default(),
            })
            .system_prompt("No Submit is needed for host tasks."),
            session_store::CombinedStore::new(
                session_store::FsStore::new(
                    f._directory
                        .path()
                        .join(format!("service-history-{incarnation}")),
                )
                .unwrap(),
                session_store::FsWorkerStore::new(
                    f._directory
                        .path()
                        .join(format!("service-metadata-{incarnation}")),
                )
                .unwrap(),
            ),
            worker::WorkerWorkspaceContext::unavailable(
                Some(worker::WorkspaceId::new(TEST_WORKSPACE_ID).unwrap()),
                "unrelated operations are unbound",
            ),
            worker::WorkerFilesystemAuthority::None,
            manifest::Scope::empty(),
        )
        .await
        .unwrap();
        let feature = bridge
            .prepare(
                &worker_ref,
                &runtime_api::PluginExecutionSnapshot {
                    plan_id: f.plan.plan_id.clone(),
                    fingerprint: f.plan.fingerprint.clone(),
                    plan: f.plan.plan.clone(),
                },
                &host.session_id().to_string(),
            )
            .await
            .unwrap();
        let report =
            host.install_features(FeatureRegistryBuilder::new().with_module(feature.clone()));
        assert!(!report.has_errors(), "{}", report.error_message());
        host.bind_plugin_tools(feature);
        host
    }

    async fn connection(remote: bool) {
        let mut peer = Peer::new(HOST, vec![response(200, &[], b"accepted")]).await;
        let f = fixture(&peer).await;
        let (artifacts, delivery_server) = artifact_source(&f, remote).await;
        let (owner, owner_server) = if remote {
            let (owner, task) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
            (owner, Some(task))
        } else {
            (f.owner(Some(peer.transport())).await, None)
        };
        assert_eq!(
            request(
                owner.as_ref(),
                PluginServiceRequest::Prepare {
                    plan_id: f.plan.plan_id.clone(),
                    installation_id: f.installed.installation.installation_id.clone(),
                    service_name: SERVICE.into(),
                    operation: PluginServiceLifecycleOperation::Admit,
                }
            )
            .await
            .unwrap(),
            PluginServiceResponse::Prepared
        );
        assert!(
            state(&f).is_none(),
            "Prepare must not create consumer state"
        );
        let host = service_host(&f, artifacts, owner, "initial").await;
        assert!(
            state(&f).is_none(),
            "installation is not HostReady acquisition"
        );
        // Create and authorize the object before HostReady; its historical event
        // must not be backfilled. The later comment is the delivered commit.
        let backend = browser_ticket_backend(&f.api).unwrap();
        let mut input = ticket::NewTicket::new("Service connection");
        input.body = ticket::MarkdownText::new("real domain commit");
        let ticket = backend.create(input).unwrap();
        data::ticket_policy(&f, &[(ticket.id.as_str(), true, true)]);
        let (handle, shutdown) = worker::controller::WorkerController::spawn_runtime_managed(
            host,
            &f._directory.path().join("service-controller"),
        )
        .await
        .unwrap();
        until(&f, "HostReady acquisition", || state(&f).is_some()).await;
        let before = state(&f).unwrap().1;
        backend
            .add_event(
                ticket.id.clone().into(),
                ticket::NewTicketEvent::new(
                    ticket::TicketEventKind::Comment,
                    "committed after HostReady",
                ),
            )
            .unwrap();
        until(&f, "Component effects and cursor ack", || {
            state(&f).is_some_and(|(_, cursor, pause, _)| cursor > before && pause.is_none())
        })
        .await;
        let bytes = peer.observed().await;
        assert!(
            String::from_utf8(bytes)
                .unwrap()
                .starts_with("POST /events HTTP/1.1\r\n")
        );
        assert_eq!(peer.connection_count(), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        assert_eq!(f.count("plugin_data_receipts"), 1);
        f.api.config_store.with_conn(|conn| {
            let row:(i64,Option<String>,Option<String>)=conn.query_row("SELECT cursor,current_event_id,last_ack_event_id FROM plugin_service_consumers WHERE plan_id=?1",[&f.plan.plan_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            assert!(row.0>before);assert!(row.1.is_none());assert!(row.2.is_some());
            Ok(())
        }).unwrap();
        handle
            .send(protocol::Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(
            state(&f).unwrap().3.is_none(),
            "controller shutdown releases owner lease"
        );
        drop(owner_server);
        drop(delivery_server);
        f.shutdown().await;
    }

    // Fault only the transport boundary of the first cursor commit. All
    // admission, projection, read/data and HTTP requests reach the signed owner.
    struct FailFirstComplete {
        owner: Arc<dyn PluginHostOwner>,
        failed: std::sync::atomic::AtomicBool,
        deliveries: Mutex<Vec<(String, String, u32)>>,
        leases: Mutex<Vec<PluginServiceLease>>,
    }

    #[async_trait::async_trait]
    impl PluginHostOwner for FailFirstComplete {
        async fn call(
            &self,
            request: PluginHostRequest,
        ) -> std::result::Result<PluginHostResponse, PluginHostError> {
            if matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Complete {
                        failure: None,
                        result: PluginServiceEventResult::Ack,
                        ..
                    }
                }
            ) && !self.failed.swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                // Hold the pre-dispatch boundary until the test shuts down this
                // Controller. Cancellation, not a scheduling race with a new
                // observer tick, models losing this completion before commit.
                return std::future::pending().await;
            }
            let response = self.owner.call(request).await?;
            match &response {
                PluginHostResponse::Service {
                    response:
                        PluginServiceResponse::Delivery {
                            envelope,
                            invocation_id,
                            ..
                        },
                } => {
                    self.deliveries.lock().unwrap().push((
                        envelope.event_id.clone(),
                        invocation_id.clone(),
                        envelope.attempt,
                    ));
                }
                PluginHostResponse::Service {
                    response: PluginServiceResponse::Acquired { lease },
                } => {
                    self.leases.lock().unwrap().push(lease.clone());
                }
                _ => {}
            }
            Ok(response)
        }
    }

    struct HoldReleaseReplies {
        owner: Arc<dyn PluginHostOwner>,
        allow_reply: std::sync::atomic::AtomicBool,
        released: tokio::sync::Notify,
        polled: tokio::sync::Notify,
        lifecycle_logs: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl PluginHostOwner for HoldReleaseReplies {
        async fn call(
            &self,
            request: PluginHostRequest,
        ) -> std::result::Result<PluginHostResponse, PluginHostError> {
            let releasing = matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Release { .. }
                }
            );
            if matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Poll { .. }
                }
            ) {
                // Poll can only be sent after the driver retained Acquire's
                // response proof. A committed consumer row alone is too early.
                self.polled.notify_one();
            }
            if matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::LifecycleLog { .. }
                }
            ) {
                self.lifecycle_logs
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            let response = self.owner.call(request).await?;
            if releasing && !self.allow_reply.load(std::sync::atomic::Ordering::SeqCst) {
                // The real transaction committed. Only its reply is unavailable.
                self.released.notify_one();
                return Err(PluginHostError {
                    code: PluginHostErrorCode::Unavailable,
                    effect: PluginEffectObservation::NotDispatched,
                    diagnostic_id: String::new(),
                });
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn service_shutdown_retains_cleanup_until_lost_release_reply_is_reconciled() {
        for remote in [false, true] {
            let peer = Peer::new(HOST, vec![]).await;
            let f = fixture(&peer).await;
            let (artifacts, delivery_server) = artifact_source(&f, remote).await;
            let (owner, owner_server) = if remote {
                let (owner, task) =
                    remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                (owner, Some(task))
            } else {
                (f.owner(None).await, None)
            };
            let boundary = Arc::new(HoldReleaseReplies {
                owner: owner.clone(),
                allow_reply: std::sync::atomic::AtomicBool::new(false),
                released: tokio::sync::Notify::new(),
                polled: tokio::sync::Notify::new(),
                lifecycle_logs: std::sync::atomic::AtomicUsize::new(0),
            });
            let host =
                service_host(&f, artifacts, boundary.clone(), "release-reconciliation").await;
            let (handle, mut shutdown) =
                worker::controller::WorkerController::spawn_runtime_managed(
                    host,
                    &f._directory.path().join("release-controller"),
                )
                .await
                .unwrap();
            until(&f, "lease acquisition", || state(&f).is_some()).await;
            // Owner commit is not response delivery: shutting down before the
            // driver receives Acquire would test a different unknown-proof case,
            // not the retained-proof Release reconciliation contract below.
            tokio::time::timeout(Duration::from_secs(10), boundary.polled.notified())
                .await
                .expect("driver must retain its lease and begin polling before shutdown");
            let (consumer_id, cursor, _, token) = state(&f).unwrap();
            let lease = PluginServiceLease {
                consumer_id,
                lease_token: token.unwrap(),
            };
            let logs_before = boundary
                .lifecycle_logs
                .load(std::sync::atomic::Ordering::SeqCst);
            handle
                .send(protocol::Method::Shutdown {
                    command: protocol::WorkerCommandEnvelope::new(1),
                })
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(10), boundary.released.notified())
                .await
                .unwrap();
            assert!(
                matches!(
                    shutdown.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty)
                ),
                "unconfirmed cleanup must not acknowledge shutdown"
            );
            assert!(
                state(&f).unwrap().3.is_none(),
                "release really committed before reply loss"
            );
            assert_eq!(
                request(owner.as_ref(), PluginServiceRequest::Poll { lease })
                    .await
                    .unwrap_err()
                    .code,
                PluginHostErrorCode::Conflict,
                "old proof cannot admit late delivery"
            );
            boundary
                .allow_reply
                .store(true, std::sync::atomic::Ordering::SeqCst);
            tokio::time::timeout(Duration::from_secs(10), shutdown)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(state(&f).unwrap().1, cursor);
            assert_eq!(
                boundary
                    .lifecycle_logs
                    .load(std::sync::atomic::Ordering::SeqCst),
                logs_before + 1,
                "release reconciliation must not rerun on-stop"
            );
            assert_eq!(f.count("plugin_service_released_proofs"), 1);
            assert_eq!(f.count("plugin_invocations"), 0);
            assert_eq!(peer.connection_count(), 0);
            drop(delivery_server);
            drop(owner_server);
            f.shutdown().await;
        }
    }

    // Only the transport boundary is faulted: the real driver, signed owner,
    // event/receipt transactions and authorized Diagnostics remain connected.
    struct LoseLifecycleResponses {
        owner: Arc<dyn PluginHostOwner>,
        commit_completion: bool,
        completed: std::sync::atomic::AtomicBool,
        release_lost: std::sync::atomic::AtomicBool,
    }
    #[async_trait::async_trait]
    impl PluginHostOwner for LoseLifecycleResponses {
        async fn call(
            &self,
            request: PluginHostRequest,
        ) -> std::result::Result<PluginHostResponse, PluginHostError> {
            let unavailable = || PluginHostError {
                code: PluginHostErrorCode::Unavailable,
                effect: PluginEffectObservation::NotDispatched,
                diagnostic_id: String::new(),
            };
            let completing = matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Complete { .. }
                }
            );
            let releasing = matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Release { .. }
                }
            );
            if matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::LifecycleLog { .. }
                }
            ) && self.completed.load(std::sync::atomic::Ordering::SeqCst)
            {
                // The real stop export calls logging and returns a guest error
                // when that import fails. The driver must retain Guest, never
                // infer a reason from the guest's error message.
                return Err(unavailable());
            }
            if matches!(
                &request,
                PluginHostRequest::Service {
                    request: PluginServiceRequest::Diagnostics { .. }
                }
            ) && self.completed.load(std::sync::atomic::Ordering::SeqCst)
            {
                // Keep the old driver read-only/parked until it is dropped. The
                // fresh owner below observes recovery without this lost channel.
                return Err(unavailable());
            }
            if completing {
                self.completed
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                if !self.commit_completion {
                    return Err(unavailable());
                }
            }
            let response = self.owner.call(request).await?;
            if completing
                || (releasing
                    && !self
                        .release_lost
                        .swap(true, std::sync::atomic::Ordering::SeqCst))
            {
                return Err(unavailable());
            }
            Ok(response)
        }
    }

    #[tokio::test]
    async fn service_lifecycle_response_loss_survives_driver_loss_and_reopened_authorized_query() {
        for remote in [false, true] {
            for commit_completion in [false, true] {
                let mut peer = Peer::new(HOST, vec![response(200, &[], b"accepted")]).await;
                let f = fixture(&peer).await;
                let ticket = create_ticket(&f);
                let (artifacts, delivery_server) = artifact_source(&f, remote).await;
                let (owner, owner_server) = if remote {
                    let (owner, task) =
                        remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                    (owner, Some(task))
                } else {
                    (f.owner(Some(peer.transport())).await, None)
                };
                let boundary = Arc::new(LoseLifecycleResponses {
                    owner,
                    commit_completion,
                    completed: std::sync::atomic::AtomicBool::new(false),
                    release_lost: std::sync::atomic::AtomicBool::new(false),
                });
                let host = service_host(&f, artifacts, boundary.clone(), "durable-lifecycle").await;
                let (handle, shutdown) =
                    worker::controller::WorkerController::spawn_runtime_managed(
                        host,
                        &f._directory.path().join("durable-lifecycle-controller"),
                    )
                    .await
                    .unwrap();
                until(&f, "lifecycle consumer acquisition", || state(&f).is_some()).await;
                let (consumer, cursor, _, token) = state(&f).unwrap();
                let token = token.unwrap();
                comment(&f, &ticket);
                until(&f, "three independent owner lifecycle observations", || {
                    f.api.config_store.with_conn(|conn| {
                        let c = crate::plugin_service::load(conn, TEST_WORKSPACE_ID, &consumer)?;
                        let diagnostics = crate::plugin_service::diagnostics(conn, &c, chrono::Utc::now().timestamp_millis())?;
                        Ok(matches!(diagnostics.last_lifecycle, Some(PluginServiceLifecycleDiagnostic::Known { observations }) if observations.len() == 3))
                    }).unwrap()
                }).await;
                handle
                    .send(protocol::Method::Shutdown {
                        command: protocol::WorkerCommandEnvelope::new(1),
                    })
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(10), shutdown)
                    .await
                    .unwrap()
                    .unwrap();
                drop(handle);
                drop(boundary);
                // Drop the driver/controller and reopen the production catalog.
                // Query through a freshly authenticated owner, not driver memory.
                let mut recovered_api = f.api.clone();
                recovered_api.config_store = Arc::new(
                    SqliteWorkspaceStore::open_for_test(&f.api.config.database_path).unwrap(),
                );
                let recovered = super::plugin_host::EmbeddedPluginHostOwner::new(
                    Arc::new(recovered_api.clone()),
                    f.verified_context().await,
                    TEST_WORKSPACE_ID.into(),
                );
                let PluginServiceResponse::Diagnostics { diagnostics } = request(
                    &recovered,
                    PluginServiceRequest::Diagnostics {
                        consumer_id: consumer.clone(),
                    },
                )
                .await
                .unwrap() else {
                    panic!("expected authorized owner diagnostics");
                };
                assert!(!diagnostics.lease_active);
                assert_eq!(diagnostics.pause_reason, None);
                let Some(PluginServiceLifecycleDiagnostic::Known { observations }) =
                    &diagnostics.last_lifecycle
                else {
                    panic!("missing durable lifecycle evidence")
                };
                assert_eq!(observations.len(), 3);
                let hex: String = Sha256::digest(token.as_bytes())
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                let lease_id = format!("sha256:{hex}");
                assert!(observations.iter().all(|o| o.lease_id == lease_id));
                let causes: Vec<_> = observations
                    .iter()
                    .map(|o| match &o.failure {
                        PluginServiceFailureDiagnostic::Known { cause, explanation } => {
                            assert_eq!(explanation, &cause.explanation());
                            *cause
                        }
                        other => panic!("unreadable lifecycle cause: {other:?}"),
                    })
                    .collect();
                assert!(causes.contains(&PluginServiceFailure::CompletionUnconfirmed));
                assert!(causes.contains(&PluginServiceFailure::ReleaseUnconfirmed));
                assert!(causes.contains(&PluginServiceFailure::Stop {
                    reason: PluginServiceExecutionFailure::Guest
                }));
                let completion = observations
                    .iter()
                    .find(|o| o.invocation.is_some())
                    .unwrap()
                    .invocation
                    .as_ref()
                    .unwrap();
                if commit_completion {
                    assert!(
                        matches!(&diagnostics.last_outcome, Some(PluginServiceOutcomeDiagnostic::Known { event_id, invocation_id, attempt: 1, result: PluginServiceEventResult::Ack, failure: None })
                        if event_id == &completion.event_id && invocation_id == &completion.invocation_id)
                    );
                    assert!(state(&f).unwrap().1 > cursor);
                    assert_eq!(diagnostics.current_event_id, None);
                } else {
                    assert_eq!(
                        diagnostics.last_outcome, None,
                        "response loss must not fabricate a failed or committed invocation"
                    );
                    assert_eq!(state(&f).unwrap().1, cursor);
                    assert_eq!(
                        diagnostics.current_event_id.as_ref(),
                        Some(&completion.event_id)
                    );
                }
                let management = recovered_api
                    .plugin_service_diagnostics(
                        TEST_WORKSPACE_ID.into(),
                        test_owner_actor(),
                        consumer.clone(),
                    )
                    .await
                    .unwrap();
                assert_eq!(management, diagnostics);
                let serialized = serde_json::to_string(&diagnostics).unwrap();
                assert!(!serialized.contains(&token));
                assert!(!serialized.contains("private Service"));
                assert_eq!(f.count("plugin_invocations"), 1);
                assert_eq!(f.count("plugin_effect_receipts"), 1);
                assert_eq!(f.count("plugin_data_receipts"), 1);
                assert_eq!(
                    peer.connection_count(),
                    1,
                    "neither lifecycle recording nor query replays the effect"
                );
                // Current policy, not stored presentation, governs disclosure.
                f.revoke();
                assert_eq!(
                    request(
                        &recovered,
                        PluginServiceRequest::Diagnostics {
                            consumer_id: consumer
                        }
                    )
                    .await
                    .unwrap_err()
                    .code,
                    PluginHostErrorCode::Denied
                );
                drop(owner_server);
                drop(delivery_server);
                f.shutdown().await;
            }
        }
    }

    #[tokio::test]
    async fn service_lifecycle_observation_requires_same_worker_and_exact_proof_after_revocation() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let (owners, server) = owners(&f).await;
        let lease = acquire(owners[0].as_ref(), &f).await;
        let observation = PluginServiceLifecycleObservation::Stop {
            reason: PluginServiceExecutionFailure::HostDenied,
        };
        // A random UUID never reaches the consumer-owner check: the signed
        // request verifier first requires registered/reserved Worker membership.
        // Use a real owner reservation so Denied proves the immutable consumer
        // identity fence, rather than an earlier proof/transport rejection.
        let memory = f
            .api
            .config_store
            .get_workspace_memory_settings(TEST_WORKSPACE_ID)
            .unwrap();
        let foreign_reservation = f
            .api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                &f.source.runtime_id,
                "foreign-service-observer",
                "sha256:foreign-service-observer",
                None,
                &memory,
            )
            .unwrap();
        let (foreign, foreign_server) = remote_owner_for(
            &f,
            &foreign_reservation.worker_id.to_string(),
            TEST_WORKSPACE_ID,
        )
        .await;
        assert_eq!(
            request(
                foreign.as_ref(),
                PluginServiceRequest::ObserveLifecycle {
                    lease: lease.clone(),
                    observation: observation.clone()
                }
            )
            .await
            .unwrap_err()
            .code,
            PluginHostErrorCode::Denied
        );
        for owner in &owners {
            let fake = PluginServiceLease {
                consumer_id: lease.consumer_id.clone(),
                lease_token: "not-issued".into(),
            };
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::ObserveLifecycle {
                        lease: fake,
                        observation: observation.clone()
                    }
                )
                .await
                .unwrap_err()
                .code,
                PluginHostErrorCode::Conflict
            );
        }
        f.revoke();
        // Cleanup evidence may be recorded by its original authenticated owner
        // even though live grants correctly prohibit delivery and disclosure.
        for owner in &owners {
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::ObserveLifecycle {
                        lease: lease.clone(),
                        observation: observation.clone()
                    }
                )
                .await
                .unwrap(),
                PluginServiceResponse::LifecycleObserved
            );
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::Diagnostics {
                        consumer_id: lease.consumer_id.clone()
                    }
                )
                .await
                .unwrap_err()
                .code,
                PluginHostErrorCode::Denied
            );
        }
        request(
            owners[0].as_ref(),
            PluginServiceRequest::Release {
                lease: lease.clone(),
            },
        )
        .await
        .unwrap();
        request(
            owners[1].as_ref(),
            PluginServiceRequest::ObserveLifecycle {
                lease: lease.clone(),
                observation: PluginServiceLifecycleObservation::ReleaseUnconfirmed,
            },
        )
        .await
        .unwrap();
        let management = f
            .api
            .plugin_service_diagnostics(
                TEST_WORKSPACE_ID.into(),
                test_owner_actor(),
                lease.consumer_id,
            )
            .await
            .unwrap();
        assert!(
            matches!(management.last_lifecycle, Some(PluginServiceLifecycleDiagnostic::Known { observations }) if observations.len() == 2)
        );
        assert_eq!(management.last_outcome, None);
        assert!(!management.lease_active);
        assert_eq!(f.count("plugin_invocations"), 0);
        assert_eq!(peer.connection_count(), 0);
        drop(foreign_server);
        drop(server);
        f.shutdown().await;
    }

    async fn complete_failure_restore(remote: bool) {
        let mut peer = Peer::new(HOST, vec![response(200, &[], b"accepted")]).await;
        let f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let (artifacts, delivery_server) = artifact_source(&f, remote).await;
        let (owner, owner_server) = if remote {
            let (owner, task) = remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
            (owner, Some(task))
        } else {
            (f.owner(Some(peer.transport())).await, None)
        };
        let boundary = Arc::new(FailFirstComplete {
            owner,
            failed: std::sync::atomic::AtomicBool::new(false),
            deliveries: Mutex::new(vec![]),
            leases: Mutex::new(vec![]),
        });
        let host = service_host(&f, artifacts.clone(), boundary.clone(), "before-ack-loss").await;
        assert!(state(&f).is_none());
        let (handle, shutdown) = worker::controller::WorkerController::spawn_runtime_managed(
            host,
            &f._directory
                .path()
                .join("service-controller-before-ack-loss"),
        )
        .await
        .unwrap();
        until(&f, "HostReady acquisition", || state(&f).is_some()).await;
        let (consumer, before, _, _) = state(&f).unwrap();
        comment(&f, &ticket);
        until(&f, "first ack held before owner dispatch", || {
            boundary.failed.load(std::sync::atomic::Ordering::SeqCst)
        })
        .await;
        handle
            .send(protocol::Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(state(&f).unwrap().3.is_none());
        assert_eq!(
            state(&f).unwrap().1,
            before,
            "failed Complete was never dispatched"
        );
        assert_eq!(peer.connection_count(), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        assert_eq!(f.count("plugin_data_receipts"), 1);
        let first = boundary.deliveries.lock().unwrap()[0].clone();
        assert_eq!(
            boundary.deliveries.lock().unwrap().len(),
            1,
            "failed task cannot automatically resend"
        );
        assert_eq!(first.2, 1);
        let (effect_key, fingerprint, response_body): (String, String, Vec<u8>) = f.api.config_store.with_conn(|conn| {
            let row = conn.query_row("SELECT effect_key,fingerprint,response_body FROM plugin_effect_receipts WHERE invocation_id=?1 AND state='completed'", [&first.1], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(row)
        }).unwrap();
        assert_eq!(effect_key, "event-hook");
        assert_eq!(response_body, b"accepted");
        // Advance only the persisted retry eligibility boundary, not wall time,
        // event identity, invocation receipt, cursor or lease authority.
        f.api.config_store.with_immediate_transaction(|tx| {
            assert_eq!(tx.execute("UPDATE plugin_service_consumers SET next_attempt_ms=0 WHERE consumer_id=?1 AND lease_token IS NULL AND current_event_id=?2 AND invocation_id=?3", rusqlite::params![consumer, first.0, first.1])?, 1);
            Ok(())
        }).unwrap();
        let host = service_host(&f, artifacts, boundary.clone(), "after-ack-loss").await;
        let (handle, shutdown) = worker::controller::WorkerController::spawn_runtime_managed(
            host,
            &f._directory
                .path()
                .join("service-controller-after-ack-loss"),
        )
        .await
        .unwrap();
        until(&f, "restored receipt replay and cursor ack", || {
            state(&f).is_some_and(|(id, cursor, pause, _)| {
                id == consumer && cursor > before && pause.is_none()
            })
        })
        .await;
        let deliveries = boundary.deliveries.lock().unwrap().clone();
        assert_eq!(deliveries.len(), 2);
        assert_eq!(deliveries[1], (first.0.clone(), first.1.clone(), 2));
        let leases = boundary.leases.lock().unwrap().clone();
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].consumer_id, leases[1].consumer_id);
        assert_ne!(leases[0].lease_token, leases[1].lease_token);
        assert_eq!(
            peer.connection_count(),
            1,
            "replayed HTTP uses the completed receipt, not another request"
        );
        assert_eq!(f.count("plugin_invocations"), 1);
        assert_eq!(f.count("plugin_effect_receipts"), 1);
        assert_eq!(
            f.count("plugin_data_receipts"),
            1,
            "guest lookup recovers the original data mutation"
        );
        f.api.config_store.with_conn(|conn| {
            let saved: (String, Vec<u8>) = conn.query_row("SELECT fingerprint,response_body FROM plugin_effect_receipts WHERE invocation_id=?1 AND effect_key=?2", rusqlite::params![first.1, effect_key], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!(saved, (fingerprint, response_body));
            let (current, ack): (Option<String>, Option<String>) = conn.query_row("SELECT current_event_id,last_ack_event_id FROM plugin_service_consumers WHERE consumer_id=?1", [&consumer], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert!(current.is_none());
            assert_eq!(ack.as_deref(), Some(first.0.as_str()));
            Ok(())
        }).unwrap();
        assert!(
            String::from_utf8(peer.observed().await)
                .unwrap()
                .starts_with("POST /events HTTP/1.1\r\n")
        );
        handle
            .send(protocol::Method::Shutdown {
                command: protocol::WorkerCommandEnvelope::new(1),
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(10), shutdown)
            .await
            .unwrap()
            .unwrap();
        assert!(state(&f).unwrap().3.is_none());
        drop(owner_server);
        drop(delivery_server);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn embedded_service_complete_transport_loss_restores_same_receipts_without_duplicate_effects()
     {
        complete_failure_restore(false).await;
    }

    #[tokio::test]
    async fn remote_service_complete_transport_loss_restores_same_receipts_without_duplicate_effects()
     {
        complete_failure_restore(true).await;
    }

    async fn owners(f: &Fixture) -> ([Arc<dyn PluginHostOwner>; 2], ServerTask) {
        let embedded = f.owner(None).await;
        let (remote, server) = remote_owner_for(f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
        ([embedded, remote], server)
    }

    async fn acquire(owner: &dyn PluginHostOwner, f: &Fixture) -> PluginServiceLease {
        match request(owner, acquire_request(f)).await.unwrap() {
            PluginServiceResponse::Acquired { lease } => lease,
            other => panic!("expected acquired lease: {other:?}"),
        }
    }

    fn create_ticket(f: &Fixture) -> String {
        let mut input = ticket::NewTicket::new("private Service title");
        input.body = ticket::MarkdownText::new("private Service body");
        let ticket = browser_ticket_backend(&f.api)
            .unwrap()
            .create(input)
            .unwrap();
        data::ticket_policy(f, &[(ticket.id.as_str(), true, true)]);
        ticket.id.to_string()
    }

    fn comment(f: &Fixture, ticket: &str) {
        browser_ticket_backend(&f.api)
            .unwrap()
            .add_event(
                ticket.into(),
                ticket::NewTicketEvent::new(
                    ticket::TicketEventKind::Comment,
                    "private Service comment",
                ),
            )
            .unwrap();
    }

    async fn delivery(
        owner: &dyn PluginHostOwner,
        lease: &PluginServiceLease,
    ) -> (PluginTicketEventEnvelope, String) {
        match request(
            owner,
            PluginServiceRequest::Poll {
                lease: lease.clone(),
            },
        )
        .await
        .unwrap()
        {
            PluginServiceResponse::Delivery {
                envelope,
                invocation_id,
                ..
            } => (envelope, invocation_id),
            other => panic!("expected Service delivery: {other:?}"),
        }
    }

    fn host_request(
        lease: &PluginServiceLease,
        request: PluginInvocationRequest,
    ) -> PluginServiceRequest {
        PluginServiceRequest::Host {
            lease: lease.clone(),
            request: Box::new(request),
        }
    }

    async fn assert_late_operations_rejected(
        owners: &[Arc<dyn PluginHostOwner>],
        lease: &PluginServiceLease,
        ticket: &str,
        event: &str,
        invocation: &str,
        code: PluginHostErrorCode,
    ) {
        for owner in owners {
            let operations = [
                PluginInvocationRequest::ReadTicket {
                    invocation_id: invocation.into(),
                    ticket_id: ticket.into(),
                },
                PluginInvocationRequest::DataMutate {
                    invocation_id: invocation.into(),
                    ticket_id: ticket.into(),
                    address: PluginDataAddress {
                        collection: PluginDataCollection {
                            collection: "notes".into(),
                            schema_id: "note".into(),
                            schema_version: "1".into(),
                        },
                        key: "main".into(),
                    },
                    operation_key: "late-write".into(),
                    mutation: PluginDataMutation::Put {
                        expected: PluginDataExpectation::NeverWritten,
                        value_json: "1".into(),
                    },
                },
                PluginInvocationRequest::Send {
                    invocation_id: invocation.into(),
                    slot: "api".into(),
                    request: PluginHttpRequest {
                        method: PluginHttpMethod::Post,
                        path: "/events".into(),
                        query: vec![],
                        headers: vec![],
                        body: vec![],
                        timeout_ms: 1000,
                        effect_key: "late-send".into(),
                    },
                },
                PluginInvocationRequest::Authorize {
                    invocation_id: invocation.into(),
                    operation: PluginHostOperation::Publish,
                },
            ];
            for operation in operations {
                assert_eq!(
                    request(owner.as_ref(), host_request(lease, operation))
                        .await
                        .unwrap_err()
                        .code,
                    code
                );
            }
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::Complete {
                        failure: None,
                        lease: lease.clone(),
                        event_id: event.into(),
                        result: PluginServiceEventResult::Ack,
                    }
                )
                .await
                .unwrap_err()
                .code,
                code
            );
        }
    }

    fn assert_no_effects(f: &Fixture, peer: &Peer, cursor: i64) {
        assert_eq!(
            state(f).unwrap().1,
            cursor,
            "denial cannot ack or skip the event"
        );
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        assert_eq!(f.count("plugin_data_receipts"), 0);
        assert_eq!(f.count("plugin_data_cells"), 0);
        assert_eq!(peer.connection_count(), 0);
    }

    #[tokio::test]
    async fn service_artifact_revoke_rejects_late_read_data_https_ack_and_allows_release() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let (owners, server) = owners(&f).await;
        let lease = acquire(owners[0].as_ref(), &f).await;
        let cursor = state(&f).unwrap().1;
        comment(&f, &ticket);
        let (event, invocation) = delivery(owners[1].as_ref(), &lease).await;
        for owner in &owners {
            assert!(matches!(
                request(
                    owner.as_ref(),
                    host_request(
                        &lease,
                        PluginInvocationRequest::ReadTicket {
                            invocation_id: invocation.clone(),
                            ticket_id: ticket.clone(),
                        }
                    )
                )
                .await
                .unwrap(),
                PluginServiceResponse::Host { .. }
            ));
        }
        f.revoke();
        assert_late_operations_rejected(
            &owners,
            &lease,
            &ticket,
            &event.event_id,
            &invocation,
            PluginHostErrorCode::Denied,
        )
        .await;
        assert_eq!(state(&f).unwrap().2.as_deref(), Some("authorization"));
        for owner in &owners {
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::Poll {
                        lease: lease.clone()
                    }
                )
                .await
                .unwrap_err()
                .code,
                PluginHostErrorCode::Denied
            );
        }
        assert_no_effects(&f, &peer, cursor);
        assert_eq!(
            request(owners[1].as_ref(), PluginServiceRequest::Release { lease })
                .await
                .unwrap(),
            PluginServiceResponse::Released
        );
        assert!(state(&f).unwrap().3.is_none());
        drop(server);
        f.shutdown().await;
    }

    fn without_operation(
        mut assignment: ::feature_permissions::PermissionAssignment,
        operation: &str,
    ) -> ::feature_permissions::PermissionAssignment {
        use ::feature_permissions::{PermissionKey, PermissionReference};
        let forbidden = PermissionKey::parse(operation).unwrap();
        assert!(assignment.grants.iter().any(|reference| matches!(
            reference,
            PermissionReference::Namespace(namespace) if namespace.keys.contains(&forbidden)
        )));
        for reference in &mut assignment.grants {
            if let PermissionReference::Namespace(namespace) = reference {
                namespace.keys.retain(|key| key != &forbidden);
            }
        }
        assignment
    }

    #[tokio::test]
    async fn service_lifecycle_requires_exact_live_publish_and_log_operations_without_receipts() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let (owners, server) = owners(&f).await;
        let store = open_subjektiv_store(&f.api).unwrap();
        let policy = store.current_subject_policy(&f.subject).unwrap().unwrap();
        for forbidden in ["plugin.publish", "plugin.log"] {
            replace_config_plugin_subject_policy_as_owner(
                &f.api,
                &f.subject,
                without_operation(policy.execution.clone(), forbidden),
                policy.delegation.clone(),
            );
            for owner in &owners {
                for operation in [
                    PluginServiceLifecycleOperation::Admit,
                    PluginServiceLifecycleOperation::Publish,
                ] {
                    let result = request(
                        owner.as_ref(),
                        PluginServiceRequest::Prepare {
                            plan_id: f.plan.plan_id.clone(),
                            installation_id: f.installed.installation.installation_id.clone(),
                            service_name: SERVICE.into(),
                            operation,
                        },
                    )
                    .await;
                    if operation == PluginServiceLifecycleOperation::Publish
                        && forbidden == "plugin.publish"
                    {
                        assert_eq!(result.unwrap_err().code, PluginHostErrorCode::Denied);
                    } else {
                        assert_eq!(result.unwrap(), PluginServiceResponse::Prepared);
                    }
                }
                let result = request(
                    owner.as_ref(),
                    PluginServiceRequest::LifecycleLog {
                        plan_id: f.plan.plan_id.clone(),
                        installation_id: f.installed.installation.installation_id.clone(),
                        service_name: SERVICE.into(),
                        entry: PluginLogEntry {
                            level: PluginLogLevel::Info,
                            event_code: "lifecycle".into(),
                            message: "bounded lifecycle".into(),
                            fields: vec![],
                        },
                    },
                )
                .await;
                if forbidden == "plugin.log" {
                    assert_eq!(result.unwrap_err().code, PluginHostErrorCode::Denied);
                } else {
                    assert_eq!(result.unwrap(), PluginServiceResponse::Logged);
                }
            }
        }
        assert!(state(&f).is_none(), "lifecycle cannot create a consumer");
        assert_eq!(f.count("plugin_invocations"), 0);
        assert_eq!(f.count("plugin_effect_receipts"), 0);
        assert_eq!(peer.connection_count(), 0);
        drop(server);
        f.shutdown().await;
    }

    #[derive(Clone, Copy, Debug)]
    enum QueueChange {
        Unchanged,
        ServiceGrant,
        BodyOwnership,
        DelegationSource,
        Feature,
        RuntimeTrust(RuntimeTrustChange),
    }

    fn change_queued_authority(f: &mut Fixture, change: QueueChange) {
        match change {
            QueueChange::Unchanged => {}
            QueueChange::RuntimeTrust(change) => replace_runtime_trust(f, change),
            QueueChange::ServiceGrant => {
                let store = open_subjektiv_store(&f.api).unwrap();
                let previous = store.current_subject_policy(&f.subject).unwrap().unwrap();
                let ticket_read =
                    ::feature_permissions::PermissionKey::parse("ticket.read").unwrap();
                assert!(previous.execution.grants.iter().any(|reference| matches!(
                    reference,
                    ::feature_permissions::PermissionReference::Namespace(namespace)
                        if namespace.keys.contains(&ticket_read)
                )));
                replace_config_plugin_subject_policy_as_owner(
                    &f.api,
                    &f.subject,
                    without_operation(previous.execution, "plugin.service.consume"),
                    previous.delegation,
                );
            }
            QueueChange::BodyOwnership => {
                let lease = f
                    .api
                    .store
                    .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.source)
                    .unwrap()
                    .unwrap();
                // The fixture has no running Subject writer. These real release
                // operations remove ownership, not a fabricated policy failure.
                let binding = "fixture-service-writer-disconnected";
                let operation = "fixture-service-body-released";
                f.api
                    .config_store
                    .reserve_subject_release(TEST_WORKSPACE_ID, &lease, binding, operation)
                    .unwrap();
                f.api
                    .config_store
                    .confirm_subject_release_proof(
                        TEST_WORKSPACE_ID,
                        &lease,
                        binding,
                        operation,
                        false,
                    )
                    .unwrap();
                f.api
                    .config_store
                    .release_subject_singleton(TEST_WORKSPACE_ID, &lease, binding, operation)
                    .unwrap();
            }
            QueueChange::DelegationSource => {
                let store = open_subjektiv_store(&f.api).unwrap();
                let previous = store.current_subject_policy(&f.subject).unwrap().unwrap();
                let replacement = replace_config_plugin_subject_policy_as_owner(
                    &f.api,
                    &f.subject,
                    previous.execution,
                    previous.delegation,
                );
                assert_ne!(replacement.change_id, previous.change_id);
            }
            QueueChange::Feature => {
                f.api
                    .permission_registry
                    .write()
                    .unwrap()
                    .set_enabled(&worker::feature::FeatureId::builtin("plugin-host"), false)
                    .unwrap();
            }
        }
    }

    async fn queued_authority_change(change: QueueChange) {
        for remote in [false, true] {
            for boundary in [
                "acquire", "poll", "disclose", "complete", "consumer", "data", "mutate",
            ] {
                let peer = Peer::new(HOST, vec![]).await;
                let mut f = fixture(&peer).await;
                let ticket = create_ticket(&f);
                if matches!(change, QueueChange::DelegationSource) {
                    delegate_plugin_fixture_with_permissions(
                        &mut f,
                        ["ticket.read", "ticket.extension.write"]
                            .into_iter()
                            .map(|operation| ScopedPermission {
                                operation: operation.into(),
                                resource: PermissionResource::Ticket {
                                    workspace_id: TEST_WORKSPACE_ID.into(),
                                    ticket_id: ticket.clone(),
                                },
                            })
                            .collect(),
                    )
                    .await;
                }
                let (owner, server) = if remote {
                    let (owner, server) =
                        remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                    (owner, Some(server))
                } else {
                    (f.owner(None).await, None)
                };
                let (operation, cursor) = if boundary == "acquire" {
                    (acquire_request(&f), None)
                } else {
                    let lease = acquire(owner.as_ref(), &f).await;
                    let cursor = state(&f).unwrap().1;
                    comment(&f, &ticket);
                    let operation =
                        if matches!(boundary, "complete" | "consumer" | "data" | "mutate") {
                            let (event, invocation) = delivery(owner.as_ref(), &lease).await;
                            if boundary == "complete" {
                                PluginServiceRequest::Complete {
                                    lease,
                                    event_id: event.event_id,
                                    result: PluginServiceEventResult::Ack,
                                    failure: None,
                                }
                            } else if boundary == "mutate" {
                                host_request(
                                    &lease,
                                    PluginInvocationRequest::DataMutate {
                                        invocation_id: invocation,
                                        ticket_id: ticket.clone(),
                                        address: PluginDataAddress {
                                            collection: PluginDataCollection {
                                                collection: "notes".into(),
                                                schema_id: "note".into(),
                                                schema_version: "1".into(),
                                            },
                                            key: "queued".into(),
                                        },
                                        operation_key: "queued-write".into(),
                                        mutation: PluginDataMutation::Put {
                                            expected: PluginDataExpectation::NeverWritten,
                                            value_json: r#""queued""#.into(),
                                        },
                                    },
                                )
                            } else {
                                host_request(
                                    &lease,
                                    PluginInvocationRequest::ReadTicket {
                                        invocation_id: invocation,
                                        ticket_id: ticket.clone(),
                                    },
                                )
                            }
                        } else {
                            PluginServiceRequest::Poll { lease }
                        };
                    (operation, Some(cursor))
                };
                let (entered, reached) = tokio::sync::oneshot::channel();
                let (resume, resumed) = std::sync::mpsc::channel();
                let gate = Mutex::new(Some((entered, resumed)));
                let boundary_observed = if boundary == "mutate" {
                    "data"
                } else {
                    boundary
                };
                let _registration = WorkspaceApi::observe_service_boundary(
                    f.plan.plan_id.clone(),
                    Arc::new(move |stage| {
                        if stage == boundary_observed {
                            if let Some((entered, resumed)) = gate.lock().unwrap().take() {
                                let _ = entered.send(());
                                resumed
                                    .recv_timeout(Duration::from_secs(15))
                                    .expect("queued Service boundary was not released");
                            }
                        }
                    }),
                );
                let current_operation = operation.clone();
                let pending = tokio::spawn(async move { request(owner.as_ref(), operation).await });
                tokio::time::timeout(Duration::from_secs(15), reached)
                    .await
                    .expect("Service did not reach queued boundary")
                    .unwrap();
                change_queued_authority(&mut f, change);
                if matches!(change, QueueChange::ServiceGrant | QueueChange::Feature) {
                    // Ticket access is intentionally still valid: it cannot stand
                    // in for the callee's service namespace at disclosure.
                    subject_authorization::authorize(
                        &f.api,
                        &f.source,
                        &OperationAuthorizationRequest {
                            operation: "ticket.read".into(),
                            resource: PermissionResource::Ticket {
                                workspace_id: TEST_WORKSPACE_ID.into(),
                                ticket_id: ticket.clone(),
                            },
                        },
                    )
                    .unwrap();
                }
                resume.send(()).unwrap();
                let outcome = tokio::time::timeout(Duration::from_secs(15), pending)
                    .await
                    .expect("Service did not finish queued boundary")
                    .unwrap();
                if matches!(change, QueueChange::Unchanged) {
                    let response = outcome.unwrap();
                    assert!(
                        match boundary {
                            "acquire" => matches!(response, PluginServiceResponse::Acquired { .. }),
                            "complete" => matches!(response, PluginServiceResponse::Completed),
                            "consumer" | "data" | "mutate" =>
                                matches!(response, PluginServiceResponse::Host { .. }),
                            _ => matches!(response, PluginServiceResponse::Delivery { .. }),
                        },
                        "normal control failed at {boundary}, remote={remote}"
                    );
                    if boundary == "complete" {
                        assert!(
                            state(&f).unwrap().1 > cursor.unwrap(),
                            "authorized ack must persist"
                        );
                    }
                } else {
                    assert_eq!(
                        outcome.unwrap_err().code,
                        PluginHostErrorCode::Denied,
                        "{change:?} at {boundary}, remote={remote}"
                    );
                    if boundary == "poll" {
                        assert_eq!(
                            f.count("plugin_invocations"),
                            0,
                            "stale poll cannot stage a delivery"
                        );
                    }
                    if let Some(cursor) = cursor {
                        assert_no_effects(&f, &peer, cursor);
                        f.api.config_store.with_conn(|conn| {
                            let ack: Option<String> = conn.query_row(
                                "SELECT last_ack_event_id FROM plugin_service_consumers WHERE plan_id=?1",
                                [&f.plan.plan_id], |row| row.get(0),
                            )?;
                            assert!(ack.is_none(), "denial must not persist an acknowledgement");
                            Ok(())
                        }).unwrap();
                    } else {
                        assert!(
                            state(&f).is_none(),
                            "stale admission must not acquire a lease"
                        );
                    }
                }
                if matches!(change, QueueChange::RuntimeTrust(_)) {
                    if boundary == "disclose" {
                        // Poll staged the delivery before the binding changed.
                        // Make its retry due without altering authority, the
                        // pending event, invocation, acknowledgement or cursor.
                        f.api.config_store.with_immediate_transaction(|tx| {
                            assert_eq!(tx.execute(
                                "UPDATE plugin_service_consumers SET next_attempt_ms=0 WHERE plan_id=?1 AND current_event_id IS NOT NULL",
                                [&f.plan.plan_id],
                            )?, 1);
                            Ok(())
                        }).unwrap();
                    }
                    let (current, current_server) = if remote {
                        let (owner, server) =
                            remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                        (owner, Some(server))
                    } else {
                        (f.owner(None).await, None)
                    };
                    let response = request(current.as_ref(), current_operation).await
                        .unwrap_or_else(|error| panic!(
                            "current binding at {boundary}, remote={remote}, {change:?}: {error:?}"
                        ));
                    assert!(
                        match boundary {
                            "acquire" => matches!(response, PluginServiceResponse::Acquired { .. }),
                            "complete" => matches!(response, PluginServiceResponse::Completed),
                            "mutate" => matches!(response, PluginServiceResponse::Host { response }
                            if matches!(*response, PluginInvocationResponse::DataMutated { .. })),
                            "consumer" | "data" =>
                                matches!(response, PluginServiceResponse::Host { response }
                            if matches!(*response, PluginInvocationResponse::TicketRead { .. })),
                            _ => matches!(response, PluginServiceResponse::Delivery { .. }),
                        },
                        "current binding response at {boundary}, remote={remote}"
                    );
                    if boundary == "mutate" {
                        assert_eq!(f.count("plugin_data_receipts"), 1);
                        assert_eq!(f.count("plugin_data_cells"), 1);
                    }
                    drop(current_server);
                }
                assert_eq!(peer.connection_count(), 0);
                drop(server);
                drop(_registration);
                f.shutdown().await;
            }
        }
    }

    #[tokio::test]
    async fn service_queue_boundaries_preserve_authorized_embedded_and_signed_remote_calls() {
        queued_authority_change(QueueChange::Unchanged).await;
    }

    #[tokio::test]
    async fn service_consume_revoked_in_queue_denies_acquire_ack_and_disclosure_with_ticket_read() {
        queued_authority_change(QueueChange::ServiceGrant).await;
    }

    #[tokio::test]
    async fn service_body_released_in_queue_denies_acquire_ack_and_disclosure() {
        queued_authority_change(QueueChange::BodyOwnership).await;
    }

    #[tokio::test]
    async fn service_delegation_source_replaced_in_queue_denies_acquire_ack_and_disclosure() {
        queued_authority_change(QueueChange::DelegationSource).await;
    }

    #[tokio::test]
    async fn service_feature_disabled_in_queue_denies_acquire_ack_and_disclosure() {
        queued_authority_change(QueueChange::Feature).await;
    }

    #[tokio::test]
    async fn service_runtime_key_replaced_in_queue_denies_old_request_and_preserves_current_binding()
     {
        queued_authority_change(QueueChange::RuntimeTrust(RuntimeTrustChange::KeyRotation)).await;
    }

    #[tokio::test]
    async fn service_runtime_same_key_reactivated_in_queue_denies_old_binding_not_current_request()
    {
        queued_authority_change(QueueChange::RuntimeTrust(
            RuntimeTrustChange::ReactivateSameKey,
        ))
        .await;
    }

    #[tokio::test]
    async fn service_runtime_same_key_aba_in_queue_never_revives_the_authenticated_old_binding() {
        queued_authority_change(QueueChange::RuntimeTrust(RuntimeTrustChange::SameKeyAba)).await;
    }

    #[tokio::test]
    async fn service_ticket_read_revoked_before_poll_pauses_without_disclosing_or_advancing() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let (owners, server) = owners(&f).await;
        let lease = acquire(owners[0].as_ref(), &f).await;
        let cursor = state(&f).unwrap().1;
        comment(&f, &ticket);
        data::ticket_policy(&f, &[]);
        for owner in &owners {
            let PluginServiceResponse::Diagnostics { diagnostics } = request(
                owner.as_ref(),
                PluginServiceRequest::Poll {
                    lease: lease.clone(),
                },
            )
            .await
            .unwrap() else {
                panic!("revoked Ticket read must never disclose an envelope");
            };
            assert_eq!(
                diagnostics.pause_reason,
                Some(PluginServicePauseReason::Authorization)
            );
        }
        assert_no_effects(&f, &peer, cursor);
        request(owners[0].as_ref(), PluginServiceRequest::Release { lease })
            .await
            .unwrap();
        drop(server);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn service_plan_owner_unavailable_rejects_late_operations_without_effect_or_ack() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let (owners, server) = owners(&f).await;
        let lease = acquire(owners[0].as_ref(), &f).await;
        let cursor = state(&f).unwrap().1;
        comment(&f, &ticket);
        let (event, invocation) = delivery(owners[1].as_ref(), &lease).await;
        // Deterministic owner-storage failure, not a replacement admission owner.
        f.api.config_store.with_conn(|conn| {
            conn.execute_batch("ALTER TABLE plugin_execution_plans RENAME COLUMN plan_json TO unavailable_plan_json")?;
            Ok(())
        }).unwrap();
        assert_late_operations_rejected(
            &owners,
            &lease,
            &ticket,
            &event.event_id,
            &invocation,
            PluginHostErrorCode::Unavailable,
        )
        .await;
        for owner in &owners {
            assert_eq!(
                request(
                    owner.as_ref(),
                    PluginServiceRequest::Poll {
                        lease: lease.clone()
                    }
                )
                .await
                .unwrap_err()
                .code,
                PluginHostErrorCode::Unavailable
            );
        }
        assert_no_effects(&f, &peer, cursor);
        f.api.config_store.with_conn(|conn| {
            conn.execute_batch("ALTER TABLE plugin_execution_plans RENAME COLUMN unavailable_plan_json TO plan_json")?;
            Ok(())
        }).unwrap();
        assert_eq!(state(&f).unwrap().2.as_deref(), Some("authorization"));
        request(owners[1].as_ref(), PluginServiceRequest::Release { lease })
            .await
            .unwrap();
        drop(server);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn service_delegation_source_expiry_fences_child_but_not_parent_consumer() {
        let peer = Peer::new(HOST, vec![]).await;
        let mut f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let parent = f.owner(None).await;
        let parent_lease = acquire(parent.as_ref(), &f).await;
        let source = delegate_plugin_fixture_with_permissions(
            &mut f,
            ["ticket.read", "ticket.extension.write"]
                .into_iter()
                .map(|operation| ScopedPermission {
                    operation: operation.into(),
                    resource: PermissionResource::Ticket {
                        workspace_id: TEST_WORKSPACE_ID.into(),
                        ticket_id: ticket.clone(),
                    },
                })
                .collect(),
        )
        .await;
        let (owners, server) = owners(&f).await;
        let lease = acquire(owners[0].as_ref(), &f).await;
        let cursor = state(&f).unwrap().1;
        comment(&f, &ticket);
        let (event, invocation) = delivery(owners[1].as_ref(), &lease).await;
        assert!(matches!(
            request(
                owners[0].as_ref(),
                host_request(
                    &lease,
                    PluginInvocationRequest::ReadTicket {
                        invocation_id: invocation.clone(),
                        ticket_id: ticket.clone(),
                    }
                )
            )
            .await
            .unwrap(),
            PluginServiceResponse::Host { .. }
        ));
        // Delegation validity is source-identity based, not wall-clock based.
        // Even identical replacement policy expires this retained child grant.
        crate::server::plugin_host::with_authority_database(&f.api, || {
            subject_authorization::plugin_execution_identity(&f.api, &f.source).unwrap();
            let (_manager, independent) = authority_storage::independent_store(&f.api);
            let current = independent
                .current_subject_policy(&f.subject)
                .unwrap()
                .unwrap();
            assert_eq!(current.change_id, source.change_id);
            let mut independent_api = f.api.clone();
            independent_api.config_store =
                Arc::new(crate::SqliteWorkspaceStore::open(&f.api.config.database_path).unwrap());
            independent_api.subject_authority_store = Some(independent);
            let replacement = replace_config_plugin_subject_policy_as_owner(
                &independent_api,
                &f.subject,
                current.execution,
                current.delegation,
            );
            assert_ne!(replacement.change_id, source.change_id);
            assert!(subject_authorization::plugin_execution_identity(&f.api, &f.source).is_err());
            Ok(())
        })
        .unwrap();
        assert_late_operations_rejected(
            &owners,
            &lease,
            &ticket,
            &event.event_id,
            &invocation,
            PluginHostErrorCode::Denied,
        )
        .await;
        assert_eq!(state(&f).unwrap().2.as_deref(), Some("authorization"));
        assert_no_effects(&f, &peer, cursor);
        let (parent_event, _) = delivery(parent.as_ref(), &parent_lease).await;
        assert_eq!(parent_event.event_id, event.event_id);
        assert_eq!(
            request(
                parent.as_ref(),
                PluginServiceRequest::Complete {
                    failure: None,
                    lease: parent_lease.clone(),
                    event_id: parent_event.event_id,
                    result: PluginServiceEventResult::Ack,
                }
            )
            .await
            .unwrap(),
            PluginServiceResponse::Completed
        );
        f.api
            .config_store
            .with_conn(|conn| {
                let (cursor, pause): (i64, Option<String>) = conn.query_row(
                    "SELECT cursor,pause_reason FROM plugin_service_consumers WHERE consumer_id=?1",
                    [&parent_lease.consumer_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert!(cursor > 0);
                assert!(
                    pause.is_none(),
                    "child authorization failure cannot pause its parent"
                );
                Ok(())
            })
            .unwrap();
        request(owners[1].as_ref(), PluginServiceRequest::Release { lease })
            .await
            .unwrap();
        request(
            parent.as_ref(),
            PluginServiceRequest::Release {
                lease: parent_lease,
            },
        )
        .await
        .unwrap();
        drop(server);
        f.shutdown().await;
    }

    #[tokio::test]
    async fn service_deleted_ticket_metadata_never_grants_resolve_read_or_data_access() {
        for ticket_access in [false, true] {
            let peer = Peer::new(HOST, vec![]).await;
            let f = fixture_with_ticket_access(&peer, ticket_access).await;
            let ticket = create_ticket(&f);
            let (owners, server) = owners(&f).await;
            let lease = acquire(owners[0].as_ref(), &f).await;
            let cursor = state(&f).unwrap().1;
            let (deletion, digest) = f
                .api
                .config_store
                .with_immediate_transaction(|tx| {
                    let digest =
                        ticket::sqlite_ticket_content_digest(tx, TEST_WORKSPACE_ID, &ticket)?
                            .unwrap();
                    let event = ticket::service_outbox::record_ticket_deletion_in_transaction(
                        tx,
                        TEST_WORKSPACE_ID,
                        &ticket,
                        "delete-service-ticket",
                    )?;
                    assert_eq!(
                        tx.execute(
                            "DELETE FROM typed_tickets WHERE workspace_id=?1 AND ticket_id=?2",
                            rusqlite::params![TEST_WORKSPACE_ID, ticket]
                        )?,
                        1
                    );
                    Ok((event, digest))
                })
                .unwrap();
            let (event, invocation) = delivery(owners[1].as_ref(), &lease).await;
            assert_eq!(event.event_id, deletion);
            assert_eq!(event.event_type, PluginTicketEventType::Deleted);
            assert_eq!(event.ticket_id, ticket);
            assert_eq!(event.domain_event_ref, "delete-service-ticket");
            assert_eq!(event.content_digest, digest);
            let serialized = serde_json::to_string(&event).unwrap();
            for sensitive in ["private Service", "title", "body", "author", "attributes"] {
                assert!(
                    !serialized.contains(sensitive),
                    "deletion must expose metadata only"
                );
            }
            for owner in &owners {
                for operation in [
                    PluginInvocationRequest::ResolveTicket {
                        invocation_id: invocation.clone(),
                        ticket_ref: ticket.clone(),
                        access: PluginTicketAccess::Read,
                    },
                    PluginInvocationRequest::ReadTicket {
                        invocation_id: invocation.clone(),
                        ticket_id: ticket.clone(),
                    },
                    PluginInvocationRequest::DataGet {
                        invocation_id: invocation.clone(),
                        ticket_id: ticket.clone(),
                        address: PluginDataAddress {
                            collection: PluginDataCollection {
                                collection: "notes".into(),
                                schema_id: "note".into(),
                                schema_version: "1".into(),
                            },
                            key: "main".into(),
                        },
                    },
                ] {
                    let error = request(owner.as_ref(), host_request(&lease, operation))
                        .await
                        .unwrap_err();
                    assert_eq!(error.code, PluginHostErrorCode::NotFound);
                }
            }
            assert_no_effects(&f, &peer, cursor);
            assert_eq!(
                request(
                    owners[0].as_ref(),
                    PluginServiceRequest::Complete {
                        failure: None,
                        lease: lease.clone(),
                        event_id: deletion,
                        result: PluginServiceEventResult::Ack
                    }
                )
                .await
                .unwrap(),
                PluginServiceResponse::Completed
            );
            assert!(state(&f).unwrap().1 > cursor);
            request(owners[1].as_ref(), PluginServiceRequest::Release { lease })
                .await
                .unwrap();
            drop(server);
            f.shutdown().await;
        }
    }

    #[tokio::test]
    async fn embedded_service_real_ticket_commit_feature_pump_component_read_data_https_cursor() {
        connection(false).await;
    }
    #[tokio::test]
    async fn remote_service_real_ticket_commit_feature_pump_component_read_data_https_cursor() {
        connection(true).await;
    }
}
