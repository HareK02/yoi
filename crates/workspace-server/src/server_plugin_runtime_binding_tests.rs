//! Signed Embedded/Remote requests cross real Host admission and SQLite receipt
//! creation. A deterministic executor gate replaces scheduling only; the shared
//! fixture substitutes the external TLS peer, not Runtime proof verification.
//! Registered in the normal `plugin_host_tests` lib suite (openssl required).
use super::*;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum ReceiptChange {
    RuntimeTrust(RuntimeTrustChange),
    Unchanged,
    Policy,
    Feature,
    Body,
    Parent,
}

fn change_receipt_authority(f: &mut Fixture, change: ReceiptChange, boundary: &str) {
    match change {
        ReceiptChange::RuntimeTrust(change) => replace_runtime_trust(f, change),
        ReceiptChange::Unchanged => {}
        ReceiptChange::Policy | ReceiptChange::Parent => {
            let store = open_subjektiv_store(&f.api).unwrap();
            let previous = store.current_subject_policy(&f.subject).unwrap().unwrap();
            let forbidden = feature_permissions::PermissionKey::parse(if boundary == "begin" {
                "plugin.admit"
            } else {
                "plugin.http.send"
            })
            .unwrap();
            let mut execution = previous.execution;
            if matches!(change, ReceiptChange::Policy) {
                assert!(execution.grants.iter().any(|reference| matches!(
                    reference,
                    feature_permissions::PermissionReference::Namespace(namespace)
                        if namespace.keys.contains(&forbidden)
                )));
                for reference in &mut execution.grants {
                    if let feature_permissions::PermissionReference::Namespace(namespace) =
                        reference
                    {
                        namespace.keys.retain(|key| key != &forbidden);
                    }
                }
            }
            replace_config_plugin_subject_policy_as_owner(
                &f.api,
                &f.subject,
                execution,
                previous.delegation,
            );
        }
        ReceiptChange::Feature => {
            f.api
                .permission_registry
                .write()
                .unwrap()
                .set_enabled(&worker::feature::FeatureId::builtin("plugin-host"), false)
                .unwrap();
        }
        ReceiptChange::Body => {
            let lease = f
                .api
                .store
                .require_current_worker_singleton_owner(TEST_WORKSPACE_ID, &f.source)
                .unwrap()
                .unwrap();
            // This fixture substitutes Worker execution and has no live Subject
            // writer. Release real catalog ownership, not a fake policy result.
            let binding = "fixture-tool-writer-disconnected";
            let operation = "fixture-tool-body-released";
            f.api
                .config_store
                .reserve_subject_release(TEST_WORKSPACE_ID, &lease, binding, operation)
                .unwrap();
            f.api
                .config_store
                .confirm_subject_release_proof(TEST_WORKSPACE_ID, &lease, binding, operation, false)
                .unwrap();
            f.api
                .config_store
                .release_subject_singleton(TEST_WORKSPACE_ID, &lease, binding, operation)
                .unwrap();
        }
    }
}

fn assert_receipt_success(
    f: &Fixture,
    peer: &Peer,
    boundary: &str,
    invocations: i64,
    response: PluginHostResponse,
) {
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
}

async fn queued_receipt_authority(boundary: &'static str, changes: &[ReceiptChange]) {
    for remote in [false, true] {
        for &change in changes {
            let peer = Peer::new(HOST, vec![response(200, &[], b"current authority")]).await;
            let mut f = Fixture::new(Some(peer_binding(&peer))).await;
            if matches!(change, ReceiptChange::Parent) {
                delegate_plugin_fixture(&mut f).await;
            }
            let original_binding = f
                .api
                .config_store
                .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &f.source.runtime_id)
                .unwrap()
                .unwrap()
                .binding_id;
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
                consumer: f.consumer("queued-receipt"),
            };
            let operation = if boundary == "begin" {
                begin
            } else {
                let PluginHostResponse::Invocation { invocation_id, .. } =
                    owner.call(begin).await.unwrap()
                else {
                    panic!("authorized setup must create an invocation");
                };
                send(
                    &invocation_id,
                    PluginHttpMethod::Get,
                    "queued-receipt-effect",
                )
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
            change_receipt_authority(&mut f, change, boundary);
            if !matches!(change, ReceiptChange::RuntimeTrust(_)) {
                assert_eq!(
                    f.api
                        .config_store
                        .get_workspace_runtime_binding(TEST_WORKSPACE_ID, &f.source.runtime_id,)
                        .unwrap()
                        .unwrap()
                        .binding_id,
                    original_binding,
                    "policy/Feature/body/parent schedules must retain the authenticated binding"
                );
            }
            resume.send(()).unwrap();
            let outcome = tokio::time::timeout(Duration::from_secs(15), pending)
                .await
                .expect("queued request did not finish")
                .unwrap();
            if matches!(change, ReceiptChange::Unchanged) {
                assert_receipt_success(&f, &peer, boundary, invocations, outcome.unwrap());
            } else {
                assert_eq!(
                    outcome.unwrap_err().code,
                    PluginHostErrorCode::Denied,
                    "{change:?} at {boundary}, remote={remote}"
                );
                assert_eq!(f.count("plugin_invocations"), invocations);
                assert_eq!(f.count("plugin_effect_receipts"), 0);
                assert_eq!(peer.connection_count(), 0);
                // Regranting a root's operation is a new current permission
                // decision, not receipt replay. The denied request must not have
                // consumed this same durable invocation/effect key.
                if matches!(change, ReceiptChange::Policy) {
                    set_policy(
                        &f.api,
                        &f.subject,
                        &f.installed.installation.installation_id,
                        true,
                    );
                }
                if matches!(
                    change,
                    ReceiptChange::RuntimeTrust(_) | ReceiptChange::Policy
                ) {
                    let (current, current_server) = if remote {
                        let (owner, server) =
                            remote_owner_for(&f, &f.source.worker_id, TEST_WORKSPACE_ID).await;
                        (owner, Some(server))
                    } else {
                        (f.owner(Some(peer.transport())).await, None)
                    };
                    let response = current.call(current_operation).await
                        .unwrap_or_else(|error| panic!("current authority {change:?} at {boundary}, remote={remote}: {error:?}"));
                    assert_receipt_success(&f, &peer, boundary, invocations, response);
                    drop(current_server);
                }
            }
            drop(server);
            drop(registration);
            f.shutdown().await;
        }
    }
}

async fn queued_receipt_rejects_replaced_binding(boundary: &'static str) {
    queued_receipt_authority(
        boundary,
        &[
            ReceiptChange::RuntimeTrust(RuntimeTrustChange::KeyRotation),
            ReceiptChange::RuntimeTrust(RuntimeTrustChange::ReactivateSameKey),
            ReceiptChange::RuntimeTrust(RuntimeTrustChange::SameKeyAba),
        ],
    )
    .await;
}

#[tokio::test]
async fn runtime_binding_replaced_before_invocation_receipt_denies_old_request_not_current_binding()
{
    queued_receipt_rejects_replaced_binding("begin").await;
}

#[tokio::test]
async fn runtime_binding_replaced_before_effect_receipt_denies_old_dispatch_not_current_binding() {
    queued_receipt_rejects_replaced_binding("effect").await;
}

#[tokio::test]
async fn unchanged_binding_authority_changed_before_invocation_receipt_denies_stale_insert() {
    queued_receipt_authority(
        "begin",
        &[
            ReceiptChange::Unchanged,
            ReceiptChange::Policy,
            ReceiptChange::Feature,
            ReceiptChange::Body,
            ReceiptChange::Parent,
        ],
    )
    .await;
}

#[tokio::test]
async fn unchanged_binding_authority_changed_before_effect_receipt_never_consumes_stale_key() {
    queued_receipt_authority(
        "effect",
        &[
            ReceiptChange::Unchanged,
            ReceiptChange::Policy,
            ReceiptChange::Feature,
            ReceiptChange::Body,
            ReceiptChange::Parent,
        ],
    )
    .await;
}
