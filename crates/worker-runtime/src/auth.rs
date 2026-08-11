use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

const PUBLIC_KEY_PREFIX: &str = "yoi-ed25519-pub:v1:";
const PRIVATE_KEY_PREFIX: &str = "yoi-ed25519-pkcs8:v1:";
const TOKEN_PREFIX: &str = "yoi-cap-v1";
const SIGNING_INPUT_PREFIX: &str = "yoi-cap-v1.";
pub const WORKER_MUTATION_SOURCE_PROOF_HEADER: &str = "x-yoi-worker-mutation-proof";
const WORKER_MUTATION_SOURCE_PROOF_PREFIX: &str = "yoi-worker-source-v1";
const WORKER_MUTATION_SOURCE_SIGNING_INPUT_PREFIX: &str = "yoi-worker-source-v1.";
pub const WORKER_REMOVE_PERMISSION: &str = "workspace:worker-remove";

#[derive(Debug, thiserror::Error)]
pub enum RuntimeAuthError {
    #[error("invalid Ed25519 public key format")]
    InvalidPublicKeyFormat,
    #[error("invalid Ed25519 private key format")]
    InvalidPrivateKeyFormat,
    #[error("invalid base64url key material: {0}")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("invalid Ed25519 private key")]
    InvalidPrivateKey,
    #[error("failed to generate Ed25519 keypair")]
    KeyGeneration,
    #[error("failed to generate random token id")]
    Random,
    #[error("invalid capability token format")]
    InvalidTokenFormat,
    #[error("malformed capability token claims: {0}")]
    MalformedClaims(#[from] serde_json::Error),
    #[error("unknown token issuer `{0}`")]
    UnknownIssuer(String),
    #[error("invalid token signature")]
    InvalidSignature,
    #[error("token audience `{actual}` does not match runtime `{expected}`")]
    WrongAudience { expected: String, actual: String },
    #[error("capability token is expired")]
    Expired,
    #[error("capability token is missing workspace scope")]
    MissingWorkspaceScope,
    #[error("capability token is missing required permission `{0}`")]
    MissingPermission(String),
    #[error("source proof workspace `{actual}` does not match `{expected}`")]
    WrongWorkspace { expected: String, actual: String },
    #[error("source proof Worker `{actual}` does not match `{expected}`")]
    WrongWorker { expected: String, actual: String },
    #[error("source proof actor kind is not allowed")]
    WrongActorKind,
    #[error("source proof operation is not allowed")]
    WrongOperation,
    #[error("source proof mutation target does not match the request")]
    WrongMutationTarget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentityMaterial {
    pub identity_id: String,
    pub public_key: String,
    pub private_key: String,
}

impl RuntimeIdentityMaterial {
    pub fn generate(identity_id: impl Into<String>) -> Result<Self, RuntimeAuthError> {
        let rng = SystemRandom::new();
        let pkcs8 =
            Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| RuntimeAuthError::KeyGeneration)?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| RuntimeAuthError::InvalidPrivateKey)?;
        Ok(Self {
            identity_id: identity_id.into(),
            public_key: encode_public_key(pair.public_key().as_ref()),
            private_key: encode_private_key(pkcs8.as_ref()),
        })
    }

    pub fn signing_key(&self) -> Result<Ed25519KeyPair, RuntimeAuthError> {
        let private = decode_private_key(&self.private_key)?;
        Ed25519KeyPair::from_pkcs8(&private).map_err(|_| RuntimeAuthError::InvalidPrivateKey)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedServerKey {
    pub server_id: String,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHttpAuthConfig {
    pub runtime_id: String,
    #[serde(default)]
    pub trusted_servers: Vec<TrustedServerKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAuthContext {
    pub server_id: String,
    pub workspace_id: String,
    pub permissions: Vec<String>,
    pub token_id: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityClaims {
    pub iss: String,
    pub aud: String,
    pub workspace_id: String,
    pub permissions: Vec<String>,
    pub exp: u64,
    pub jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapabilityTokenSigner {
    server_id: String,
    private_key: String,
}

impl CapabilityTokenSigner {
    pub fn new(server_id: impl Into<String>, private_key: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            private_key: private_key.into(),
        }
    }

    pub fn server_id(&self) -> &str {
        &self.server_id
    }

    pub fn sign(&self, claims: &CapabilityClaims) -> Result<String, RuntimeAuthError> {
        if claims.iss != self.server_id {
            return Err(RuntimeAuthError::UnknownIssuer(claims.iss.clone()));
        }
        let private = decode_private_key(&self.private_key)?;
        let pair = Ed25519KeyPair::from_pkcs8(&private)
            .map_err(|_| RuntimeAuthError::InvalidPrivateKey)?;
        let payload = serde_json::to_vec(claims)?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{SIGNING_INPUT_PREFIX}{payload}");
        let signature = pair.sign(signing_input.as_bytes());
        Ok(format!(
            "{TOKEN_PREFIX}.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }
}

pub fn capability_claims(
    server_id: impl Into<String>,
    runtime_id: impl Into<String>,
    workspace_id: impl Into<String>,
    permissions: Vec<String>,
    ttl_seconds: u64,
) -> Result<CapabilityClaims, RuntimeAuthError> {
    let exp = unix_now_seconds().saturating_add(ttl_seconds);
    Ok(CapabilityClaims {
        iss: server_id.into(),
        aud: runtime_id.into(),
        workspace_id: workspace_id.into(),
        permissions,
        exp,
        jti: new_token_id()?,
    })
}

pub fn verify_capability_token(
    config: &RuntimeHttpAuthConfig,
    token: &str,
    required_permission: Option<&str>,
    now_seconds: u64,
) -> Result<RuntimeAuthContext, RuntimeAuthError> {
    let (payload, signature) = split_token(token)?;
    let claims_json = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: CapabilityClaims = serde_json::from_slice(&claims_json)?;
    let Some(server) = config
        .trusted_servers
        .iter()
        .find(|server| server.server_id == claims.iss)
    else {
        return Err(RuntimeAuthError::UnknownIssuer(claims.iss));
    };
    let public_key = decode_public_key(&server.public_key)?;
    let signing_input = format!("{SIGNING_INPUT_PREFIX}{payload}");
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| RuntimeAuthError::InvalidSignature)?;

    if claims.aud != config.runtime_id {
        return Err(RuntimeAuthError::WrongAudience {
            expected: config.runtime_id.clone(),
            actual: claims.aud,
        });
    }
    if claims.exp < now_seconds {
        return Err(RuntimeAuthError::Expired);
    }
    if claims.workspace_id.trim().is_empty() {
        return Err(RuntimeAuthError::MissingWorkspaceScope);
    }
    if let Some(required) = required_permission {
        if !claims
            .permissions
            .iter()
            .any(|permission| permission == required)
        {
            return Err(RuntimeAuthError::MissingPermission(required.to_string()));
        }
    }
    Ok(RuntimeAuthContext {
        server_id: claims.iss,
        workspace_id: claims.workspace_id,
        permissions: claims.permissions,
        token_id: claims.jti,
        expires_at: claims.exp,
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerMutationSourceClaims {
    pub iss: String,
    pub aud: String,
    pub workspace_id: String,
    pub worker_id: String,
    pub actor_kind: WorkerMutationActorKind,
    pub operation: WorkerMutationOperation,
    pub target_runtime_id: String,
    pub target_worker_id: String,
    pub permission: String,
    pub iat: u64,
    pub exp: u64,
    pub jti: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMutationActorKind {
    Worker,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerMutationOperation {
    WorkerRemove,
}

#[derive(Clone)]
pub struct RuntimeWorkerMutationSourceSigner {
    runtime_id: String,
    private_key: String,
}

impl fmt::Debug for RuntimeWorkerMutationSourceSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeWorkerMutationSourceSigner")
            .field("runtime_id", &self.runtime_id)
            .field("private_key", &"[redacted]")
            .finish()
    }
}

impl RuntimeWorkerMutationSourceSigner {
    pub fn from_identity(identity: &RuntimeIdentityMaterial) -> Self {
        Self {
            runtime_id: identity.identity_id.clone(),
            private_key: identity.private_key.clone(),
        }
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn issue_worker_remove(
        &self,
        audience: impl Into<String>,
        workspace_id: impl Into<String>,
        source_worker_id: impl Into<String>,
        target_runtime_id: impl Into<String>,
        target_worker_id: impl Into<String>,
        ttl_seconds: u64,
    ) -> Result<String, RuntimeAuthError> {
        let issued_at = unix_now_seconds();
        let claims = WorkerMutationSourceClaims {
            iss: self.runtime_id.clone(),
            aud: audience.into(),
            workspace_id: workspace_id.into(),
            worker_id: source_worker_id.into(),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: target_runtime_id.into(),
            target_worker_id: target_worker_id.into(),
            permission: WORKER_REMOVE_PERMISSION.to_string(),
            iat: issued_at,
            exp: issued_at.saturating_add(ttl_seconds),
            jti: new_token_id()?,
        };
        self.sign(&claims)
    }

    pub fn sign(&self, claims: &WorkerMutationSourceClaims) -> Result<String, RuntimeAuthError> {
        if claims.iss != self.runtime_id {
            return Err(RuntimeAuthError::UnknownIssuer(claims.iss.clone()));
        }
        let private = decode_private_key(&self.private_key)?;
        let pair = Ed25519KeyPair::from_pkcs8(&private)
            .map_err(|_| RuntimeAuthError::InvalidPrivateKey)?;
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims)?);
        let signing_input = format!("{WORKER_MUTATION_SOURCE_SIGNING_INPUT_PREFIX}{payload}");
        let signature = pair.sign(signing_input.as_bytes());
        Ok(format!(
            "{WORKER_MUTATION_SOURCE_PROOF_PREFIX}.{payload}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerMutationSourceExpectation<'a> {
    pub runtime_id: &'a str,
    pub audience: &'a str,
    pub workspace_id: &'a str,
    pub worker_id: Option<&'a str>,
    pub actor_kind: WorkerMutationActorKind,
    pub operation: WorkerMutationOperation,
    pub target_runtime_id: &'a str,
    pub target_worker_id: &'a str,
    pub permission: &'a str,
}

pub fn decode_worker_mutation_source_claims(
    token: &str,
) -> Result<WorkerMutationSourceClaims, RuntimeAuthError> {
    let (payload, _) = split_worker_mutation_source_proof(token)?;
    Ok(serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload)?)?)
}

pub fn verify_worker_mutation_source_proof(
    trusted_runtime_public_key: &str,
    token: &str,
    expected: &WorkerMutationSourceExpectation<'_>,
    now_seconds: u64,
) -> Result<WorkerMutationSourceClaims, RuntimeAuthError> {
    let (payload, signature) = split_worker_mutation_source_proof(token)?;
    let claims_json = URL_SAFE_NO_PAD.decode(payload)?;
    let claims: WorkerMutationSourceClaims = serde_json::from_slice(&claims_json)?;
    let public_key = decode_public_key(trusted_runtime_public_key)?;
    let signing_input = format!("{WORKER_MUTATION_SOURCE_SIGNING_INPUT_PREFIX}{payload}");
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(signing_input.as_bytes(), &signature)
        .map_err(|_| RuntimeAuthError::InvalidSignature)?;

    if claims.iss != expected.runtime_id {
        return Err(RuntimeAuthError::UnknownIssuer(claims.iss));
    }
    if claims.aud != expected.audience {
        return Err(RuntimeAuthError::WrongAudience {
            expected: expected.audience.to_string(),
            actual: claims.aud,
        });
    }
    if claims.exp <= now_seconds || claims.iat > now_seconds.saturating_add(60) {
        return Err(RuntimeAuthError::Expired);
    }
    if claims.workspace_id != expected.workspace_id {
        return Err(RuntimeAuthError::WrongWorkspace {
            expected: expected.workspace_id.to_string(),
            actual: claims.workspace_id,
        });
    }
    if let Some(worker_id) = expected.worker_id {
        if claims.worker_id != worker_id {
            return Err(RuntimeAuthError::WrongWorker {
                expected: worker_id.to_string(),
                actual: claims.worker_id,
            });
        }
    }
    if claims.actor_kind != expected.actor_kind {
        return Err(RuntimeAuthError::WrongActorKind);
    }
    if claims.operation != expected.operation {
        return Err(RuntimeAuthError::WrongOperation);
    }
    if claims.target_runtime_id != expected.target_runtime_id
        || claims.target_worker_id != expected.target_worker_id
    {
        return Err(RuntimeAuthError::WrongMutationTarget);
    }
    if claims.permission != expected.permission {
        return Err(RuntimeAuthError::MissingPermission(
            expected.permission.to_string(),
        ));
    }
    if claims.jti.trim().is_empty() {
        return Err(RuntimeAuthError::InvalidTokenFormat);
    }
    Ok(claims)
}

fn split_worker_mutation_source_proof(token: &str) -> Result<(&str, Vec<u8>), RuntimeAuthError> {
    let mut parts = token.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(prefix), Some(payload), Some(signature), None)
            if prefix == WORKER_MUTATION_SOURCE_PROOF_PREFIX =>
        {
            Ok((payload, URL_SAFE_NO_PAD.decode(signature)?))
        }
        _ => Err(RuntimeAuthError::InvalidTokenFormat),
    }
}

fn split_token(token: &str) -> Result<(&str, Vec<u8>), RuntimeAuthError> {
    let mut parts = token.split('.');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(prefix), Some(payload), Some(signature), None) if prefix == TOKEN_PREFIX => {
            Ok((payload, URL_SAFE_NO_PAD.decode(signature)?))
        }
        _ => Err(RuntimeAuthError::InvalidTokenFormat),
    }
}

pub fn encode_public_key(bytes: &[u8]) -> String {
    format!("{PUBLIC_KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_public_key(value: &str) -> Result<Vec<u8>, RuntimeAuthError> {
    let Some(encoded) = value.strip_prefix(PUBLIC_KEY_PREFIX) else {
        return Err(RuntimeAuthError::InvalidPublicKeyFormat);
    };
    let bytes = URL_SAFE_NO_PAD.decode(encoded)?;
    if bytes.len() != 32 {
        return Err(RuntimeAuthError::InvalidPublicKeyFormat);
    }
    Ok(bytes)
}

pub fn encode_private_key(bytes: &[u8]) -> String {
    format!("{PRIVATE_KEY_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

pub fn decode_private_key(value: &str) -> Result<Vec<u8>, RuntimeAuthError> {
    let Some(encoded) = value.strip_prefix(PRIVATE_KEY_PREFIX) else {
        return Err(RuntimeAuthError::InvalidPrivateKeyFormat);
    };
    Ok(URL_SAFE_NO_PAD.decode(encoded)?)
}

pub fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn new_token_id() -> Result<String, RuntimeAuthError> {
    let rng = SystemRandom::new();
    let mut bytes = [0_u8; 16];
    rng.fill(&mut bytes).map_err(|_| RuntimeAuthError::Random)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

impl fmt::Display for RuntimeAuthContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "server={} workspace={} permissions={} exp={}",
            self.server_id,
            self.workspace_id,
            self.permissions.join(","),
            self.expires_at
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_mutation_source_proof_binds_all_source_authority_claims() {
        let runtime = RuntimeIdentityMaterial::generate("runtime-main").unwrap();
        let signer = RuntimeWorkerMutationSourceSigner::from_identity(&runtime);
        let claims = WorkerMutationSourceClaims {
            iss: "runtime-main".to_string(),
            aud: "server-main".to_string(),
            workspace_id: "workspace-a".to_string(),
            worker_id: "worker-7".to_string(),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-target".to_string(),
            target_worker_id: "worker-target".to_string(),
            permission: WORKER_REMOVE_PERMISSION.to_string(),
            iat: 90,
            exp: 100,
            jti: "source-proof-1".to_string(),
        };
        let token = signer.sign(&claims).unwrap();
        let expected = WorkerMutationSourceExpectation {
            runtime_id: "runtime-main",
            audience: "server-main",
            workspace_id: "workspace-a",
            worker_id: Some("worker-7"),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-target",
            target_worker_id: "worker-target",
            permission: WORKER_REMOVE_PERMISSION,
        };

        assert_eq!(
            verify_worker_mutation_source_proof(&runtime.public_key, &token, &expected, 99)
                .unwrap(),
            claims
        );

        let wrong_worker = WorkerMutationSourceExpectation {
            worker_id: Some("worker-8"),
            ..expected.clone()
        };
        assert!(matches!(
            verify_worker_mutation_source_proof(&runtime.public_key, &token, &wrong_worker, 99),
            Err(RuntimeAuthError::WrongWorker { .. })
        ));
        let wrong_scope = WorkerMutationSourceExpectation {
            workspace_id: "workspace-b",
            ..expected.clone()
        };
        assert!(matches!(
            verify_worker_mutation_source_proof(&runtime.public_key, &token, &wrong_scope, 99),
            Err(RuntimeAuthError::WrongWorkspace { .. })
        ));
        let wrong_audience = WorkerMutationSourceExpectation {
            audience: "server-other",
            ..expected.clone()
        };
        assert!(matches!(
            verify_worker_mutation_source_proof(&runtime.public_key, &token, &wrong_audience, 99),
            Err(RuntimeAuthError::WrongAudience { .. })
        ));
        assert!(matches!(
            verify_worker_mutation_source_proof(&runtime.public_key, &token, &expected, 101),
            Err(RuntimeAuthError::Expired)
        ));
    }

    #[test]
    fn worker_mutation_source_proof_rejects_spoofed_runtime_signature() {
        let trusted = RuntimeIdentityMaterial::generate("runtime-main").unwrap();
        let spoofed = RuntimeIdentityMaterial::generate("runtime-main").unwrap();
        let claims = WorkerMutationSourceClaims {
            iss: "runtime-main".to_string(),
            aud: "server-main".to_string(),
            workspace_id: "workspace-a".to_string(),
            worker_id: "worker-7".to_string(),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-target".to_string(),
            target_worker_id: "worker-target".to_string(),
            permission: WORKER_REMOVE_PERMISSION.to_string(),
            iat: 90,
            exp: 100,
            jti: "source-proof-2".to_string(),
        };
        let token = RuntimeWorkerMutationSourceSigner::from_identity(&spoofed)
            .sign(&claims)
            .unwrap();
        let expected = WorkerMutationSourceExpectation {
            runtime_id: "runtime-main",
            audience: "server-main",
            workspace_id: "workspace-a",
            worker_id: Some("worker-7"),
            actor_kind: WorkerMutationActorKind::Worker,
            operation: WorkerMutationOperation::WorkerRemove,
            target_runtime_id: "runtime-target",
            target_worker_id: "worker-target",
            permission: WORKER_REMOVE_PERMISSION,
        };

        assert!(matches!(
            verify_worker_mutation_source_proof(&trusted.public_key, &token, &expected, 99),
            Err(RuntimeAuthError::InvalidSignature)
        ));
    }

    #[test]
    fn capability_token_verifies_signature_audience_expiry_and_permission() {
        let server = RuntimeIdentityMaterial::generate("server-main").unwrap();
        let signer = CapabilityTokenSigner::new(&server.identity_id, &server.private_key);
        let claims = CapabilityClaims {
            iss: "server-main".to_string(),
            aud: "runtime-main".to_string(),
            workspace_id: "workspace-a".to_string(),
            permissions: vec!["workers:list".to_string()],
            exp: 100,
            jti: "token-1".to_string(),
        };
        let token = signer.sign(&claims).unwrap();
        let auth = RuntimeHttpAuthConfig {
            runtime_id: "runtime-main".to_string(),
            trusted_servers: vec![TrustedServerKey {
                server_id: "server-main".to_string(),
                public_key: server.public_key.clone(),
                display_name: None,
            }],
        };

        let context = verify_capability_token(&auth, &token, Some("workers:list"), 99).unwrap();
        assert_eq!(context.workspace_id, "workspace-a");
        assert!(matches!(
            verify_capability_token(&auth, &token, Some("workers:create"), 99),
            Err(RuntimeAuthError::MissingPermission(permission)) if permission == "workers:create"
        ));
        assert!(matches!(
            verify_capability_token(&auth, &token, Some("workers:list"), 101),
            Err(RuntimeAuthError::Expired)
        ));
        let wrong_audience = RuntimeHttpAuthConfig {
            runtime_id: "other-runtime".to_string(),
            trusted_servers: auth.trusted_servers.clone(),
        };
        assert!(matches!(
            verify_capability_token(&wrong_audience, &token, Some("workers:list"), 99),
            Err(RuntimeAuthError::WrongAudience { .. })
        ));
    }
}
