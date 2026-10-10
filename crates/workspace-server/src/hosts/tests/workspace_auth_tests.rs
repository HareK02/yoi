use super::*;
use crate::store::{
    AccountRecord, SqliteWorkspaceStore, WorkspaceRecord, WorkspaceRuntimeBinding,
    WorkspaceSigningIdentityRecord,
};
use crate::workspace_signing_identity::{
    InMemoryWorkspaceSigningMaterialStore, WorkspaceSigningMaterialStore,
    WorkspaceSigningPrivateMaterial, public_key_fingerprint,
};
use worker_runtime::auth::RuntimeIdentityMaterial;
use worker_runtime::workspace_issuer::{
    InMemoryWorkspaceClaimReplayProtection, VerifiedWorkspaceCapability,
    WorkspaceCapabilityExpectation, WorkspaceCapabilityVerificationError,
    WorkspaceCapabilityVerifier, WorkspaceIssuerTrustRecord, WorkspaceIssuerTrustState,
};

const WORKSPACE: &str = "workspace-auth-test";
const RUNTIME: &str = "runtime-auth-test";
const BACKEND: &str = "https://backend.test";
const PATH: &str = "/v1/ping?probe=1";

struct Fixture {
    _temp: tempfile::TempDir,
    store: Arc<SqliteWorkspaceStore>,
    materials: Arc<InMemoryWorkspaceSigningMaterialStore>,
    service: WorkspaceSigningIdentityService,
    authorizer: WorkspaceRuntimeAuthorization,
    binding: WorkspaceRuntimeBinding,
    identity: WorkspaceSigningIdentityRecord,
}

impl Fixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteWorkspaceStore::open(temp.path().join("server.db")).unwrap());
        store
            .upsert_account(&AccountRecord {
                account_id: "owner".into(),
                kind: "user".into(),
                handle: "owner".into(),
                display_name: "Owner".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: WORKSPACE.into(),
                owner_account_id: "owner".into(),
                display_name: "Workspace".into(),
                state: "active".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .await
            .unwrap();
        let materials = Arc::new(InMemoryWorkspaceSigningMaterialStore::default());
        let service = WorkspaceSigningIdentityService::new(store.clone(), materials.clone());
        let identity = service.provision_existing(WORKSPACE, "owner").unwrap();
        let runtime_identity = RuntimeIdentityMaterial::generate(RUNTIME).unwrap();
        store
            .upsert_workspace_runtime_binding(
                WorkspaceRuntimeBinding {
                    workspace_id: WORKSPACE.into(),
                    runtime_id: RUNTIME.into(),
                    display_name: "Runtime".into(),
                    base_url: "https://runtime.test".into(),
                    public_key_fingerprint: public_key_fingerprint(&runtime_identity.public_key)
                        .unwrap(),
                    public_key: runtime_identity.public_key,
                    binding_id: String::new(),
                    authentication_mode: WorkspaceRuntimeAuthenticationMode::WorkspaceIdentity,
                    created_at: "1".into(),
                    updated_at: "1".into(),
                    revoked_at: None,
                },
                false,
            )
            .unwrap();
        let binding = store
            .get_workspace_runtime_binding(WORKSPACE, RUNTIME)
            .unwrap()
            .unwrap();
        let authorizer = WorkspaceRuntimeAuthorization::new(
            store.clone(),
            service.clone(),
            BACKEND,
            binding.clone(),
        );
        Self {
            _temp: temp,
            store,
            materials,
            service,
            authorizer,
            binding,
            identity,
        }
    }

    fn issue(&self) -> Result<String, RuntimeDiagnostic> {
        self.authorizer
            .issue("GET", PATH, RUNTIME_PING_PERMISSION, None, b"")
    }

    fn verifier(&self) -> WorkspaceCapabilityVerifier {
        WorkspaceCapabilityVerifier::new(
            vec![WorkspaceIssuerTrustRecord {
                workspace_id: WORKSPACE.into(),
                backend_url: BACKEND.into(),
                key_id: self.identity.key_id.clone(),
                algorithm: "ed25519".into(),
                public_key: self.identity.public_key.clone().unwrap(),
                public_key_fingerprint: self.identity.public_key_fingerprint.clone().unwrap(),
                trust_id: uuid::Uuid::now_v7().to_string(),
                state: WorkspaceIssuerTrustState::Active,
                registered_at_unix: Utc::now().timestamp(),
                updated_at_unix: Utc::now().timestamp(),
            }],
            Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
        )
        .unwrap()
    }

    // Fixture-only rotation: deliberately no production rotation API or live database.
    fn rotate_identity(&mut self) {
        let key_id = "WK-rotated";
        let material_ref = format!("workspace-signing/{WORKSPACE}/{key_id}");
        let material = WorkspaceSigningPrivateMaterial::generate(WORKSPACE, key_id).unwrap();
        let public_key = material.validate_and_public_key(WORKSPACE, key_id).unwrap();
        let fingerprint = public_key_fingerprint(&public_key).unwrap();
        self.materials
            .put_if_absent(&material_ref, &material)
            .unwrap();
        self.store.with_conn(|conn| {
            conn.execute("UPDATE workspace_signing_identities SET key_id = ?1, public_key = ?2, public_key_fingerprint = ?3, private_material_ref = ?4 WHERE workspace_id = ?5",
                rusqlite::params![key_id, public_key, fingerprint, material_ref, WORKSPACE])?;
            Ok(())
        }).unwrap();
        self.identity = self.service.get_validated(WORKSPACE).unwrap();
    }
}

fn verify(
    verifier: &WorkspaceCapabilityVerifier,
    token: &str,
) -> Result<VerifiedWorkspaceCapability, WorkspaceCapabilityVerificationError> {
    verifier.verify(
        token,
        &WorkspaceCapabilityExpectation {
            workspace_id: WORKSPACE,
            runtime_id: RUNTIME,
            worker_id: None,
            operation: RUNTIME_PING_PERMISSION,
            method: "GET",
            path_and_query: PATH,
            body_digest: &workspace_request_body_digest(b""),
            now_unix: Utc::now().timestamp(),
        },
    )
}

#[tokio::test]
async fn authorizer_token_authenticates_actual_request_without_prior_verification_record() {
    let fixture = Fixture::new().await;
    let token = fixture.issue().unwrap();
    let verifier = fixture.verifier();
    let authenticated = verify(&verifier, &token).unwrap();
    assert_eq!(authenticated.workspace_id, WORKSPACE);
    assert_eq!(authenticated.issuer_key_id, fixture.identity.key_id);
    assert_eq!(
        verify(&verifier, &token).unwrap_err(),
        WorkspaceCapabilityVerificationError::Replay
    );
    let token = fixture.issue().unwrap();
    let wrong_request = WorkspaceCapabilityExpectation {
        workspace_id: WORKSPACE,
        runtime_id: RUNTIME,
        worker_id: None,
        operation: RUNTIME_PING_PERMISSION,
        method: "GET",
        path_and_query: "/v1/ping?probe=2",
        body_digest: &workspace_request_body_digest(b""),
        now_unix: Utc::now().timestamp(),
    };
    assert_eq!(
        verifier.verify(&token, &wrong_request).unwrap_err(),
        WorkspaceCapabilityVerificationError::WrongPathAndQuery
    );
}

#[tokio::test]
async fn existing_authorizer_selects_rotated_workspace_key_and_new_runtime_trust() {
    let mut fixture = Fixture::new().await;
    let old_verifier = fixture.verifier();
    let old_token = fixture.issue().unwrap();
    fixture.rotate_identity();
    let new_token = fixture.issue().unwrap();
    let new_verifier = fixture.verifier();
    assert_eq!(
        verify(&new_verifier, &new_token).unwrap().issuer_key_id,
        "WK-rotated"
    );
    assert_eq!(
        verify(&new_verifier, &old_token).unwrap_err(),
        WorkspaceCapabilityVerificationError::WrongIssuerKey
    );
    assert_eq!(
        verify(&old_verifier, &new_token).unwrap_err(),
        WorkspaceCapabilityVerificationError::WrongIssuerKey
    );
    // Changing Workspace authority did not replace or copy metadata into the connection.
    assert_eq!(
        fixture
            .store
            .get_workspace_runtime_binding(WORKSPACE, RUNTIME)
            .unwrap()
            .unwrap(),
        fixture.binding
    );
}

#[tokio::test]
async fn replaced_or_revoked_connection_rejects_stale_authorizer() {
    for revoked in [false, true] {
        let fixture = Fixture::new().await;
        fixture.issue().unwrap();
        if revoked {
            fixture
                .store
                .revoke_workspace_runtime_binding(WORKSPACE, RUNTIME, "2")
                .unwrap();
        } else {
            let mut replacement = fixture.binding.clone();
            replacement.base_url = "https://replacement-runtime.test".into();
            fixture
                .store
                .upsert_workspace_runtime_binding(replacement, true)
                .unwrap();
        }
        assert_eq!(
            fixture.issue().unwrap_err().code,
            "workspace_runtime_authorization_stale"
        );
    }
}

#[tokio::test]
async fn missing_workspace_signing_material_rejects_authorization_without_fallback() {
    let fixture = Fixture::new().await;
    fixture
        .materials
        .delete(&fixture.identity.private_material_ref)
        .unwrap();
    let error = fixture.issue().unwrap_err();
    assert_eq!(error.code, "workspace_runtime_authorization_unavailable");
    assert!(
        error.message.contains("private material is missing"),
        "{}",
        error.message
    );
}

#[tokio::test]
async fn capability_claims_must_match_current_workspace_identity_before_signing() {
    let mut fixture = Fixture::new().await;
    let now = Utc::now().timestamp();
    let mut claims = WorkspaceCapabilityClaims {
        issuer: BACKEND.into(),
        issuer_workspace_id: WORKSPACE.into(),
        issuer_key_id: fixture.identity.key_id.clone(),
        issuer_public_key_fingerprint: fixture.identity.public_key_fingerprint.clone().unwrap(),
        runtime_id: RUNTIME.into(),
        worker_id: None,
        operation: RUNTIME_PING_PERMISSION.into(),
        method: "GET".into(),
        path_and_query: PATH.into(),
        body_digest: workspace_request_body_digest(b""),
        iat: now,
        exp: now + 60,
        jti: uuid::Uuid::now_v7().to_string(),
    };
    let token = fixture
        .service
        .issue_workspace_capability(WORKSPACE, &claims)
        .unwrap();
    verify(&fixture.verifier(), &token).unwrap();
    for field in ["workspace", "key", "fingerprint"] {
        let mut mismatched = claims.clone();
        match field {
            "workspace" => mismatched.issuer_workspace_id = "another-workspace".into(),
            "key" => mismatched.issuer_key_id = "WK-another".into(),
            "fingerprint" => {
                mismatched.issuer_public_key_fingerprint = format!("sha256:{}", "0".repeat(64))
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(fixture.service.issue_workspace_capability(WORKSPACE, &mismatched),
            Err(Error::WorkspaceSigningIdentity { ref code, .. }) if code == "workspace_capability_identity_mismatch"),
            "{field}"
        );
    }
    fixture.rotate_identity();
    assert!(
        matches!(fixture.service.issue_workspace_capability(WORKSPACE, &claims),
        Err(Error::WorkspaceSigningIdentity { ref code, .. }) if code == "workspace_capability_identity_mismatch")
    );
    claims.issuer_key_id = fixture.identity.key_id.clone();
    claims.issuer_public_key_fingerprint = fixture.identity.public_key_fingerprint.clone().unwrap();
    let token = fixture
        .service
        .issue_workspace_capability(WORKSPACE, &claims)
        .unwrap();
    verify(&fixture.verifier(), &token).unwrap();
}
