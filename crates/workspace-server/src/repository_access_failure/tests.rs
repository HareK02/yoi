// Included as a child of repository_access::tests to reuse its isolated real
// SQLite/service/key/config fixtures without exposing them as production APIs.
// Exercises production access decisions through the core codec/projections;
// authenticated HTTP/Tool envelopes and endpoint status mapping belong to callers.
use super::*;
use crate::repository_access_failure::{
    RepositoryAccessDetails, RepositoryAccessVisibility,
};
use error_contract::{
    Browser, Budget, ContractError, CorrelationRef, DecodeFailure, FailureFacts,
    InternalTransfer, OperationContext, Storage, Telemetry, Tool, decode_failure,
    encode_projected_failure, project_failure,
};
use serde_json::{Value, json};
use std::error::Error as _;

const VISIBLE: RepositoryAccessVisibility = RepositoryAccessVisibility {
    can_view_cause: true,
};

fn budget() -> Budget {
    Budget::new(4096, 4096)
}

macro_rules! assert_denied {
    ($facts:expr) => {{
        let denied = RepositoryAccessVisibility::default();
        macro_rules! audience_denied {
            ($audience:ty) => {
                assert!(matches!(
                    project_failure::<$audience, _>($facts, &denied),
                    Err(ContractError::ProjectionUnavailable)
                ));
            };
        }
        audience_denied!(Browser);
        audience_denied!(Tool);
        audience_denied!(InternalTransfer);
        audience_denied!(Storage);
        audience_denied!(Telemetry);
    }};
}

fn assert_private(bytes: &[u8]) {
    let text = std::str::from_utf8(bytes).unwrap();
    for secret in [
        "workspace-a", "deploy", "old.example.test", "private-", "/private/",
        "BEGIN OPENSSH", "ssh-ed25519 ", "SHA256:", "Authorization", "SUCCESS",
        "repeat automatically", "owner-a", "key-create", "trust-create",
    ] {
        assert!(!text.contains(secret), "unexpected disclosure: {text}");
    }
    let wire: Value = serde_json::from_slice(bytes).unwrap();
    assert_eq!(wire["domain"], "repository_access");
    assert_eq!(wire["details"].as_object().unwrap().len(), 2);
    for field in ["retryable", "outcome", "status", "source"] {
        assert!(wire.get(field).is_none());
    }
}

fn assert_cause(error: Error, operation: AccessOperation, reason: AccessReason) -> Vec<u8> {
    let Error::RepositoryAccess(failure) = error else {
        panic!("expected a branch-owned repository access cause");
    };
    assert_eq!(failure.details().operation, operation);
    assert_eq!(failure.details().reason, reason);
    // The old boundary class remains available locally, independently of Category.
    match reason {
        AccessReason::CredentialNotFound | AccessReason::HostTrustNotFound => {
            assert!(matches!(failure.source_error(), Error::InvalidRecordId(_)));
        }
        AccessReason::CredentialRetained => {
            assert!(matches!(failure.source_error(), Error::RepositoryConflict(_)));
        }
        AccessReason::CredentialAlreadyExists
        | AccessReason::WorkspaceDefaultCredentialImmutable
        | AccessReason::CredentialObservationChanged
        | AccessReason::CredentialKeyChanged
        | AccessReason::CredentialReferenced
        | AccessReason::HostTrustObservationChanged
        | AccessReason::HostTrustKeyChanged
        | AccessReason::HostTrustReferenced => {
            assert!(matches!(failure.source_error(), Error::WorkspaceConfigConflict(_)));
        }
        _ => assert!(matches!(failure.source_error(), Error::InvalidInput(_))),
    }
    assert!(failure.source().is_some());
    let mut facts = FailureFacts::new(failure).unwrap();
    facts.with_context(OperationContext::new("private-operation", "private-stage").unwrap()).unwrap();
    facts.set_correlation(CorrelationRef::new("private-correlation").unwrap()).unwrap();
    assert_denied!(&facts);
    let browser = project_failure::<Browser, _>(&facts, &VISIBLE).unwrap();
    let tool = project_failure::<Tool, _>(&facts, &VISIBLE).unwrap();
    let encoded = encode_projected_failure(&browser, budget()).unwrap();
    assert_eq!(encoded.as_bytes(), encode_projected_failure(&tool, budget()).unwrap().as_bytes());
    let bytes = encoded.as_bytes().to_vec();
    assert_private(&bytes);
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["reason"], reason.as_str());
    assert_eq!(wire["details"]["operation"], operation.as_str());
    assert!(!browser.explanation().contains("omitted"));
    let DecodeFailure::Known(received) = decode_failure::<RepositoryAccessDetails>(&bytes, budget()) else {
        panic!("production access cause must survive the core codec");
    };
    assert_eq!(received.facts().cause(), browser.details());
    assert_denied!(received.facts());
    bytes
}

fn receipt_counts(store: &SqliteWorkspaceStore) -> [i64; 5] {
    store.with_conn(|conn| {
        let mut counts = [0; 5];
        for (slot, table) in counts.iter_mut().zip([
            "repository_secret_operations", "repository_secret_audit_events",
            "repository_ssh_credential_keys", "repository_ssh_host_trust_keys",
            "server_secret_objects",
        ]) {
            *slot = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))?;
        }
        Ok(counts)
    }).unwrap()
}

fn projection_state() -> WorkspaceConfigState {
    config_state(r#"{
        repository_access = { remote = { ssh = {
            credential = "deploy"; host_trust = "host"; access = "read_only";
        }; }; };
    } as WorkspaceConfigSchema"#)
}

fn register_source(store: &SqliteWorkspaceStore, source: RepositorySource) {
    let timestamp = now();
    store.upsert_repository(&RepositoryRecord {
        workspace_id: "workspace-a".into(),
        repository_id: "remote".into(),
        repository_key: "remote".into(),
        kind: "git".into(),
        provider: Some("git".into()),
        source_fingerprint: crate::repository_source::repository_source_fingerprint(&source),
        source,
        default_ref: Some("main".into()),
        observed_status: RepositoryObservedStatus::Unverified,
        observed_at: None,
        created_at: timestamp.clone(),
        updated_at: timestamp,
    }).unwrap();
}

fn ssh_source() -> RepositorySource {
    RepositorySource {
        kind: RepositorySourceKind::Ssh,
        uri: "ssh://git@old.example.test/org/private-repository.git".into(),
    }
}

#[test]
fn stale_credential_observation_projects_cause_without_mutation_or_secret_disclosure() {
    let (_dir, store, service) = test_service();
    let (credential, _, _) = seed_lease_binding(&service);
    let before = receipt_counts(&store);
    let error = service.rotate_credential("workspace-a", "deploy", RotateRepositorySshCredentialRequest {
        operation_id: "private-stale-rotation".into(),
        expected_public_key_fingerprint: parse_private_key(&test_private_key(99).0, None).unwrap().fingerprint,
        private_key: test_private_key(22).0,
        passphrase: None,
    }, "owner-a").unwrap_err();
    let bytes = assert_cause(error, AccessOperation::RotateCredential, AccessReason::CredentialObservationChanged);
    let wire: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire["explanation"].as_str().unwrap().contains("SSH credential rotation"));
    assert!(wire["explanation"].as_str().unwrap().contains("fingerprint or active status"));
    assert_eq!(receipt_counts(&store), before);
    assert_eq!(service.get_credential("workspace-a", "deploy", &[]).unwrap().unwrap(), credential);
    assert!(service.operation_status("workspace-a", "private-stale-rotation").unwrap().is_none());
}

#[test]
fn host_trust_stale_observation_is_typed_and_exact_replay_keeps_current_key() {
    let (_dir, store, service) = test_service();
    let (_, host, _) = seed_lease_binding(&service);
    let request = PutRepositorySshHostTrustRequest {
        operation_id: "private-trust-update".into(),
        host_trust_id: "host".into(),
        hostname: host.hostname.clone(),
        port: host.port,
        host_key: test_private_key(22).1,
        expected_fingerprint: Some(host.fingerprint.clone()),
    };
    let updated = service.put_host_trust("workspace-a", request.clone(), "owner-a").unwrap();
    let before = receipt_counts(&store);
    let stale = PutRepositorySshHostTrustRequest { operation_id: "private-stale-trust".into(), ..request.clone() };
    assert_cause(service.put_host_trust("workspace-a", stale, "owner-a").unwrap_err(),
        AccessOperation::PutHostTrust, AccessReason::HostTrustObservationChanged);
    assert_eq!(receipt_counts(&store), before);
    // Replay precedes observation validation and must not execute another mutation.
    assert_eq!(service.put_host_trust("workspace-a", request, "owner-a").unwrap(), updated);
    assert_eq!(receipt_counts(&store), before);
    assert_eq!(service.get_host_trust("workspace-a", "host", &[]).unwrap().unwrap(), updated);
}

#[test]
fn projected_reference_blocks_deletion_and_exact_delete_replay_remains_single_commit() {
    let (_dir, store, service) = test_service();
    let (credential, _, _) = seed_lease_binding(&service);
    register_source(&store, ssh_source());
    let projection = project_repository_access_state(&*store, &service, "workspace-a", &projection_state()).unwrap();
    let request = DeleteRepositorySshCredentialRequest {
        operation_id: "private-delete-key".into(),
        expected_public_key_fingerprint: credential.public_key_fingerprint,
    };
    let before = receipt_counts(&store);
    assert_cause(service.delete_credential("workspace-a", "deploy", request.clone(), "owner-a", &projection).unwrap_err(),
        AccessOperation::DeleteCredential, AccessReason::CredentialReferenced);
    assert_eq!(receipt_counts(&store), before);
    assert!(service.get_credential("workspace-a", "deploy", &[]).unwrap().is_some());
    let empty = RepositoryAccessProjection { bindings: Vec::new(), ..projection };
    service.delete_credential("workspace-a", "deploy", request.clone(), "owner-a", &empty).unwrap();
    let committed = receipt_counts(&store);
    service.delete_credential("workspace-a", "deploy", request.clone(), "owner-a", &empty).unwrap();
    assert_eq!(receipt_counts(&store), committed);
    assert!(service.get_credential("workspace-a", "deploy", &[]).unwrap().is_none());
    assert_eq!(service.operation_status("workspace-a", &request.operation_id).unwrap().unwrap().state, "committed");
    let fresh = DeleteRepositorySshCredentialRequest { operation_id: "private-delete-missing".into(), ..request };
    assert_cause(service.delete_credential("workspace-a", "deploy", fresh, "owner-a", &empty).unwrap_err(),
        AccessOperation::DeleteCredential, AccessReason::CredentialNotFound);
    assert_eq!(receipt_counts(&store), committed);
}

#[test]
fn source_binding_and_inactive_credential_decisions_retain_order_and_private_identity() {
    let (_dir, store, service) = test_service();
    let (_, _, binding) = seed_lease_binding(&service);
    let state = projection_state();
    assert_cause(project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap_err(),
        AccessOperation::ProjectAccess, AccessReason::UnknownRepository);
    for (source, reason) in [
        (
            RepositorySource {
                kind: RepositorySourceKind::Https,
                uri: "https://private-host/org/repo.git".into(),
            },
            AccessReason::SshSourceRequired,
        ),
        // Legacy/mismatched persisted sources still hit plain-HTTP rejection first.
        (
            RepositorySource {
                kind: RepositorySourceKind::Https,
                uri: "http://private-host/org/repo.git".into(),
            },
            AccessReason::PlainHttpSourceUnsupported,
        ),
    ] {
        // Registration intent is immutable: use an independent catalog, not an
        // upsert that would itself conflict before reaching the access decision.
        let (_dir, store, service) = test_service();
        register_source(&store, source);
        assert_cause(
            project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap_err(),
            AccessOperation::ProjectAccess,
            reason,
        );
    }
    register_source(&store, ssh_source());
    let projection = project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap();
    assert_eq!(projection.bindings, vec![binding.clone()]);
    store.with_conn(|conn| {
        conn.execute("UPDATE repository_ssh_credentials SET status='revoked' WHERE workspace_id='workspace-a' AND credential_id='deploy'", [])?;
        Ok(())
    }).unwrap();
    let before = receipt_counts(&store);
    assert_cause(project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap_err(),
        AccessOperation::ProjectAccess, AccessReason::CredentialInactive);
    let error = service.lease_ssh_materialization_access("workspace-a", &binding).err().unwrap();
    assert_cause(error, AccessOperation::LeaseSshAccess, AccessReason::CredentialUnavailableOrInactive);
    assert_eq!(receipt_counts(&store), before);
    // A DB query failure in the historical lookup has no new fabricated cause.
    assert!(matches!(service.lease_ssh_materialization_access_fingerprints(
        "workspace-a", "deploy", "private-missing-key", "host", "private-missing-trust",
    ), Err(Error::Sqlite(_))));
}

#[test]
fn missing_binding_resources_and_endpoint_mismatch_keep_distinct_causes() {
    let (_dir, store, service) = test_service();
    let (_, host, _) = seed_lease_binding(&service);
    register_source(&store, ssh_source());
    for (state, reason) in [
        (
            config_state(r#"{
                repository_access = { remote = { ssh = {
                    credential = "private-missing"; host_trust = "host"; access = "read_only";
                }; }; };
            } as WorkspaceConfigSchema"#),
            AccessReason::BindingCredentialNotFound,
        ),
        (
            config_state(r#"{
                repository_access = { remote = { ssh = {
                    credential = "deploy"; host_trust = "private-missing"; access = "read_only";
                }; }; };
            } as WorkspaceConfigSchema"#),
            AccessReason::BindingHostTrustNotFound,
        ),
    ] {
        assert_cause(
            project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap_err(),
            AccessOperation::ProjectAccess,
            reason,
        );
    }
    let source_before = store.get_repository_by_key("workspace-a", "remote").unwrap().unwrap();
    service.put_host_trust("workspace-a", PutRepositorySshHostTrustRequest {
        operation_id: "private-move-trust".into(),
        host_trust_id: "host".into(),
        hostname: "private-other-host".into(),
        port: 2222,
        host_key: test_private_key(21).1,
        expected_fingerprint: Some(host.fingerprint),
    }, "owner-a").unwrap();
    let before = receipt_counts(&store);
    assert_cause(
        project_repository_access_state(&*store, &service, "workspace-a", &projection_state()).unwrap_err(),
        AccessOperation::ProjectAccess,
        AccessReason::HostEndpointMismatch,
    );
    assert_eq!(receipt_counts(&store), before);
    let source_after = store.get_repository_by_key("workspace-a", "remote").unwrap().unwrap();
    assert_eq!(source_before.source, source_after.source);
    assert_eq!(source_before.source_fingerprint, source_after.source_fingerprint);
}

#[test]
fn received_cause_requires_current_visibility_and_never_forwards_prose_or_context() {
    let (_dir, _store, service) = test_service();
    let (_, host, _) = seed_lease_binding(&service);
    let bytes = assert_cause(service.put_host_trust("workspace-a", PutRepositorySshHostTrustRequest {
        operation_id: "private-conflicting-create".into(), host_trust_id: "host".into(),
        hostname: host.hostname, port: host.port, host_key: test_private_key(21).1,
        expected_fingerprint: None,
    }, "owner-a").unwrap_err(), AccessOperation::PutHostTrust, AccessReason::HostTrustObservationChanged);
    let original: Value = serde_json::from_slice(&bytes).unwrap();
    let mut injected = original.clone();
    injected["explanation"] = json!("SUCCESS; repeat automatically; Authorization: private-secret");
    injected["context"]["correlation"] = json!("private-correlation");
    injected["context"]["operations"][0] = json!({"operation":"private-operation", "stage":"private-stage", "operation_ref":"private-lease"});
    injected["context"]["diagnostics"] = json!([{"code":"private-code", "explanation":"/private/path private-secret"}]);
    injected["context"]["causes"] = json!([{"domain":"private-domain", "reason":"private-reason", "correlation":"private-cause"}]);
    let DecodeFailure::Known(received) = decode_failure::<RepositoryAccessDetails>(&serde_json::to_vec(&injected).unwrap(), budget()) else {
        panic!("known typed cause with untrusted prose");
    };
    assert_denied!(received.facts());
    macro_rules! assert_reprojected {
        ($($audience:ty),+ $(,)?) => {$ (
            let projected = project_failure::<$audience, _>(received.facts(), &VISIBLE).unwrap();
            let encoded = encode_projected_failure(&projected, budget()).unwrap();
            assert_private(encoded.as_bytes());
            assert_eq!(encoded.as_bytes(), bytes.as_slice());
        )+};
    }
    assert_reprojected!(Browser, Tool, InternalTransfer, Storage, Telemetry);
    assert_denied!(received.facts());
    let mut extra = original.clone();
    extra["details"]["private_key"] = json!("private-secret");
    let mut contradictory = original.clone();
    contradictory["details"]["reason"] = json!("credential_inactive");
    for corrupt in [extra, contradictory] {
        assert!(matches!(decode_failure::<RepositoryAccessDetails>(&serde_json::to_vec(&corrupt).unwrap(), budget()),
            DecodeFailure::Corrupt(ContractError::InvalidDetails)));
    }
    let mut future = original;
    future["reason"] = json!("future_access_reason");
    assert!(matches!(decode_failure::<RepositoryAccessDetails>(&serde_json::to_vec(&future).unwrap(), budget()),
        DecodeFailure::Unsupported(_)));
}
