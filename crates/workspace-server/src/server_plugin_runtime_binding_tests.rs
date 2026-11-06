//! Signed Embedded/Remote requests cross real Host admission and SQLite receipt
//! creation. A deterministic executor gate replaces scheduling only; the shared
//! fixture substitutes the external TLS peer, not Runtime proof verification.
//! Registered in the normal `plugin_host_tests` lib suite (openssl required).
use super::*;
use std::time::Duration;

async fn queued_receipt_rejects_replaced_binding(boundary: &'static str) {
    for remote in [false, true] {
        for change in [
            RuntimeTrustChange::KeyRotation,
            RuntimeTrustChange::ReactivateSameKey,
            RuntimeTrustChange::SameKeyAba,
        ] {
            let peer = Peer::new(HOST, vec![response(200, &[], b"current binding")]).await;
            let mut f = Fixture::new(Some(peer_binding(&peer))).await;
            f.api.plugin_https_transport = Some(Arc::new(peer.transport()));
            let (owner, server) = if remote {
                let (owner, server) =
                    remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                (owner, Some(server))
            } else {
                (f.owner(Some(peer.transport())).await, None)
            };
            let begin = PluginHostRequest::Begin {
                plan_id: f.plan.plan_id.clone(),
                installation_id: f.installed.installation.installation_id.clone(),
                consumer: f.consumer("queued-binding"),
            };
            let operation = if boundary == "begin" {
                begin
            } else {
                let PluginHostResponse::Invocation { invocation_id, .. } =
                    owner.call(begin).await.unwrap()
                else {
                    panic!("authorized setup must create an invocation");
                };
                send(&invocation_id, PluginHttpMethod::Get, "queued-binding-effect")
            };
            let invocations = f.count("plugin_invocations");
            let (entered, reached) = tokio::sync::oneshot::channel();
            let (resume, resumed) = std::sync::mpsc::channel();
            let gate = Mutex::new(Some((entered, resumed)));
            let registration = WorkspaceApi::observe_service_boundary(
                f.plan.plan_id.clone(),
                Arc::new(move |stage| {
                    if stage == boundary {
                        if let Some((entered, resumed)) = gate.lock().unwrap().take() {
                            let _ = entered.send(());
                            resumed
                                .recv_timeout(Duration::from_secs(15))
                                .expect("queued receipt boundary was not released");
                        }
                    }
                }),
            );
            let current_operation = operation.clone();
            let pending = tokio::spawn(async move { owner.call(operation).await });
            tokio::time::timeout(Duration::from_secs(15), reached)
                .await
                .expect("request did not reach final receipt transaction")
                .unwrap();
            replace_runtime_trust(&mut f, change);
            resume.send(()).unwrap();
            let outcome = tokio::time::timeout(Duration::from_secs(15), pending)
                .await
                .expect("queued request did not finish")
                .unwrap();
            assert_eq!(
                outcome.unwrap_err().code,
                PluginHostErrorCode::Denied,
                "{change:?} at {boundary}, remote={remote}"
            );
            assert_eq!(f.count("plugin_invocations"), invocations);
            assert_eq!(f.count("plugin_effect_receipts"), 0);
            assert_eq!(peer.connection_count(), 0);

            let (current, current_server) = if remote {
                let (owner, server) =
                    remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                (owner, Some(server))
            } else {
                (f.owner(Some(peer.transport())).await, None)
            };
            let response = current
                .call(current_operation)
                .await
                .expect("current binding must retain the authorized operation");
            if boundary == "begin" {
                assert!(matches!(response, PluginHostResponse::Invocation { .. }));
                assert_eq!(f.count("plugin_invocations"), invocations + 1);
                assert_eq!(f.count("plugin_effect_receipts"), 0);
                assert_eq!(peer.connection_count(), 0);
            } else {
                assert!(matches!(response, PluginHostResponse::Response { .. }));
                assert_eq!(f.count("plugin_invocations"), invocations);
                assert_eq!(f.count("plugin_effect_receipts"), 1);
                assert_eq!(peer.connection_count(), 1);
            }
            drop(current_server);
            drop(server);
            drop(registration);
            f.shutdown().await;
        }
    }
}

#[tokio::test]
async fn runtime_binding_replaced_before_invocation_receipt_denies_old_request_not_current_binding() {
    queued_receipt_rejects_replaced_binding("begin").await;
}

#[tokio::test]
async fn runtime_binding_replaced_before_effect_receipt_denies_old_dispatch_not_current_binding() {
    queued_receipt_rejects_replaced_binding("effect").await;
}
