// Included in plugin_runtime_tests: real Ticket DB/outbox -> Feature HostReady
// task registry -> pinned artifact delivery -> authored Rust Service Component ->
// signed Remote/Embedded owner -> Ticket read/data/TLS receipts -> cursor commit.
// Worker allocation uses the existing isolated fixture; the pump/controller and
// all event/effect owners below are production adapters, not guest/owner doubles.
mod service_connection {
    use super::*;
    use rusqlite::OptionalExtension;
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

    #[tokio::test]
    async fn service_lifecycle_requires_exact_live_publish_and_log_operations_without_receipts() {
        let peer = Peer::new(HOST, vec![]).await;
        let f = fixture(&peer).await;
        let (owners, server) = owners(&f).await;
        let store = open_subjektiv_store(&f.api).unwrap();
        let permissions = store
            .subject_permissions(&f.subject)
            .unwrap()
            .unwrap()
            .execution;
        for forbidden in ["plugin.publish", "plugin.log"] {
            let previous = store.subject_permissions(&f.subject).unwrap().unwrap();
            store
                .set_subject_permissions(
                    &f.subject,
                    &SubjectPermissionsUpdate {
                        operation_id: format!("deny-{forbidden}"),
                        expected_change_id: Some(previous.change_id),
                        execution: permissions
                            .iter()
                            .filter(|p| p.operation != forbidden)
                            .cloned()
                            .collect(),
                        delegation: vec![],
                    },
                )
                .unwrap();
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
        use server_api::authorization::WorkerDelegationRequest;
        let peer = Peer::new(HOST, vec![]).await;
        let mut f = fixture(&peer).await;
        let ticket = create_ticket(&f);
        let parent = f.owner(None).await;
        let parent_lease = acquire(parent.as_ref(), &f).await;
        let store = open_subjektiv_store(&f.api).unwrap();
        let previous = store.subject_permissions(&f.subject).unwrap().unwrap();
        let source = store
            .set_subject_permissions(
                &f.subject,
                &SubjectPermissionsUpdate {
                    operation_id: "delegate-service".into(),
                    expected_change_id: Some(previous.change_id),
                    execution: previous.execution.clone(),
                    delegation: previous.execution.clone(),
                },
            )
            .unwrap();
        let grant = subject_authorization::prepare_delegation(
            &f.api,
            &f.source,
            Some(&WorkerDelegationRequest {
                permissions: source.execution.clone(),
                redelegation: vec![],
            }),
            &f.source.runtime_id,
            PROFILE,
            "service-profile-digest",
            None,
            false,
        )
        .unwrap()
        .unwrap();
        let memory = f
            .api
            .config_store
            .get_workspace_memory_settings(TEST_WORKSPACE_ID)
            .unwrap();
        let reserved = f
            .api
            .config_store
            .reserve_worker_create(
                TEST_WORKSPACE_ID,
                &f.source.runtime_id,
                "service-child",
                "sha256:service-child",
                None,
                &memory,
            )
            .unwrap();
        subject_authorization::bind_reserved_grant(&f.api, &grant, &reserved.worker_id.to_string())
            .unwrap();
        let child = RuntimeWorkerRef::new(&f.source.runtime_id, reserved.worker_id.to_string());
        let identity = f
            .api
            .plugin_reserved_execution_identity(&child.runtime_id, &reserved, Some(&grant))
            .unwrap();
        let mut summary = f.api.runtime.worker(&f.source).unwrap();
        summary.worker.worker_id = child.worker_id.clone();
        summary.singleton_key = None;
        f.runtime.workers.lock().unwrap().push(summary);
        assert_eq!(
            f.api.plugin_execution_identity(&child).await.unwrap(),
            identity
        );
        let config = f
            .api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        f.plan = f
            .api
            .config_store
            .with_immediate_transaction(|tx| {
                let plan = crate::plugin_settings::resolve_worker_plugin_plan(
                    tx,
                    TEST_WORKSPACE_ID,
                    &config.snapshot.digest,
                    PROFILE,
                    identity,
                )?;
                crate::plugin_plan::plugin_plan_create(
                    tx,
                    "fixture-host",
                    "service-child-plan",
                    &plan,
                )
            })
            .unwrap();
        f.api
            .config_store
            .plugin_plan_transition(
                TEST_WORKSPACE_ID,
                &f.plan.plan_id,
                PlanPinState::Pending,
                PlanPinState::Active,
            )
            .unwrap();
        f.source = child;
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
        store
            .set_subject_permissions(
                &f.subject,
                &SubjectPermissionsUpdate {
                    operation_id: "expire-service-source".into(),
                    expected_change_id: Some(source.change_id),
                    execution: source.execution,
                    delegation: source.delegation,
                },
            )
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
