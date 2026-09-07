use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{ED25519, Ed25519KeyPair, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use workspace_api::WorkspacePublicIdentityBundle;

use crate::auth::{RuntimeAuthContext, RuntimeAuthError, RuntimeHttpAuthConfig};

const WORKSPACE_TOKEN_PREFIX: &str = "yoi-workspace-v1";
const WORKSPACE_SIGNING_INPUT_PREFIX: &str = "yoi.workspace.capability.v1.";
const WORKSPACE_SIGNING_ALGORITHM: &str = "ed25519";
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 256;
const MAX_ISSUER_BYTES: usize = 2 * 1024;
const MAX_OPERATION_BYTES: usize = 128;
const MAX_REPLAY_ENTRIES: usize = 65_536;
const MAX_TOKEN_LIFETIME_SECONDS: i64 = 300;
const MAX_CLOCK_SKEW_SECONDS: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceIssuerTrustState {
    Active,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceIssuerTrustRecord {
    pub workspace_id: String,
    pub backend_url: String,
    pub key_id: String,
    pub algorithm: String,
    pub public_key: String,
    pub public_key_fingerprint: String,
    pub identity_revision: u64,
    pub trust_generation: u64,
    pub state: WorkspaceIssuerTrustState,
    pub registered_at_unix: i64,
    pub updated_at_unix: i64,
}

impl WorkspaceIssuerTrustRecord {
    pub fn from_bundle(
        bundle: WorkspacePublicIdentityBundle,
        trust_generation: u64,
        now_unix: i64,
    ) -> Result<Self, WorkspaceIssuerTrustError> {
        validate_bundle(&bundle)?;
        if trust_generation == 0 {
            return Err(WorkspaceIssuerTrustError::InvalidTrustGeneration);
        }
        Ok(Self {
            workspace_id: bundle.workspace_id,
            backend_url: bundle.backend_url,
            key_id: bundle.key_id,
            algorithm: bundle.algorithm,
            public_key: bundle.public_key,
            public_key_fingerprint: bundle.public_key_fingerprint,
            identity_revision: bundle.revision,
            trust_generation,
            state: WorkspaceIssuerTrustState::Active,
            registered_at_unix: now_unix,
            updated_at_unix: now_unix,
        })
    }

    pub fn public_bundle(&self) -> WorkspacePublicIdentityBundle {
        WorkspacePublicIdentityBundle {
            workspace_id: self.workspace_id.clone(),
            backend_url: self.backend_url.clone(),
            key_id: self.key_id.clone(),
            algorithm: self.algorithm.clone(),
            public_key: self.public_key.clone(),
            public_key_fingerprint: self.public_key_fingerprint.clone(),
            revision: self.identity_revision,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkspaceIssuerTrustError {
    #[error("Workspace issuer trust already exists for `{0}`")]
    AlreadyExists(String),
    #[error("Workspace issuer trust is not registered for `{0}`")]
    NotFound(String),
    #[error("Workspace signing algorithm is not supported")]
    UnsupportedAlgorithm,
    #[error("Workspace signing public identity backend URL is invalid")]
    InvalidBackendUrl,
    #[error("Workspace signing public identity contains an invalid identifier")]
    InvalidIdentifier,
    #[error("Workspace signing public key is invalid")]
    InvalidPublicKey,
    #[error("Workspace signing public key fingerprint does not match the public key")]
    PublicKeyFingerprintMismatch,
    #[error("Workspace signing identity revision must be greater than zero")]
    InvalidIdentityRevision,
    #[error("Workspace issuer trust generation must be greater than zero")]
    InvalidTrustGeneration,
    #[error("Workspace signing identity replacement revision is stale")]
    StaleIdentityRevision,
    #[error("Workspace issuer trust generation overflow")]
    TrustGenerationOverflow,
}

pub fn validate_workspace_issuer_trust_records(
    records: &[WorkspaceIssuerTrustRecord],
) -> Result<(), WorkspaceCapabilityVerificationError> {
    let mut seen = std::collections::HashSet::new();
    for record in records {
        validate_trust_record(record)?;
        if !seen.insert(record.workspace_id.clone()) {
            return Err(WorkspaceCapabilityVerificationError::DuplicateWorkspaceTrust);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceIssuerTrustMutation {
    Added,
    Replaced,
    Revoked,
    Unchanged,
}

pub fn add_workspace_issuer_trust(
    records: &mut Vec<WorkspaceIssuerTrustRecord>,
    bundle: WorkspacePublicIdentityBundle,
    now_unix: i64,
) -> Result<(WorkspaceIssuerTrustMutation, WorkspaceIssuerTrustRecord), WorkspaceIssuerTrustError> {
    validate_bundle(&bundle)?;
    if let Some(existing) = records
        .iter()
        .find(|record| record.workspace_id == bundle.workspace_id)
    {
        if existing.state == WorkspaceIssuerTrustState::Active && existing.public_bundle() == bundle
        {
            return Ok((WorkspaceIssuerTrustMutation::Unchanged, existing.clone()));
        }
        return Err(WorkspaceIssuerTrustError::AlreadyExists(
            bundle.workspace_id,
        ));
    }
    let record = WorkspaceIssuerTrustRecord::from_bundle(bundle, 1, now_unix)?;
    records.push(record.clone());
    records.sort_by(|left, right| left.workspace_id.cmp(&right.workspace_id));
    Ok((WorkspaceIssuerTrustMutation::Added, record))
}

pub fn replace_workspace_issuer_trust(
    records: &mut [WorkspaceIssuerTrustRecord],
    bundle: WorkspacePublicIdentityBundle,
    now_unix: i64,
) -> Result<(WorkspaceIssuerTrustMutation, WorkspaceIssuerTrustRecord), WorkspaceIssuerTrustError> {
    validate_bundle(&bundle)?;
    let record = records
        .iter_mut()
        .find(|record| record.workspace_id == bundle.workspace_id)
        .ok_or_else(|| WorkspaceIssuerTrustError::NotFound(bundle.workspace_id.clone()))?;
    if record.state == WorkspaceIssuerTrustState::Active && record.public_bundle() == bundle {
        return Ok((WorkspaceIssuerTrustMutation::Unchanged, record.clone()));
    }
    if bundle.revision < record.identity_revision {
        return Err(WorkspaceIssuerTrustError::StaleIdentityRevision);
    }
    if bundle.revision == record.identity_revision
        && (bundle.key_id != record.key_id
            || bundle.public_key != record.public_key
            || bundle.public_key_fingerprint != record.public_key_fingerprint)
    {
        return Err(WorkspaceIssuerTrustError::StaleIdentityRevision);
    }
    let generation = record
        .trust_generation
        .checked_add(1)
        .ok_or(WorkspaceIssuerTrustError::TrustGenerationOverflow)?;
    let registered_at_unix = record.registered_at_unix;
    *record = WorkspaceIssuerTrustRecord::from_bundle(bundle, generation, now_unix)?;
    record.registered_at_unix = registered_at_unix;
    Ok((WorkspaceIssuerTrustMutation::Replaced, record.clone()))
}

pub fn revoke_workspace_issuer_trust(
    records: &mut [WorkspaceIssuerTrustRecord],
    workspace_id: &str,
    now_unix: i64,
) -> Result<(WorkspaceIssuerTrustMutation, WorkspaceIssuerTrustRecord), WorkspaceIssuerTrustError> {
    validate_id(workspace_id)?;
    let record = records
        .iter_mut()
        .find(|record| record.workspace_id == workspace_id)
        .ok_or_else(|| WorkspaceIssuerTrustError::NotFound(workspace_id.to_string()))?;
    if record.state == WorkspaceIssuerTrustState::Revoked {
        return Ok((WorkspaceIssuerTrustMutation::Unchanged, record.clone()));
    }
    record.trust_generation = record
        .trust_generation
        .checked_add(1)
        .ok_or(WorkspaceIssuerTrustError::TrustGenerationOverflow)?;
    record.state = WorkspaceIssuerTrustState::Revoked;
    record.updated_at_unix = now_unix;
    Ok((WorkspaceIssuerTrustMutation::Revoked, record.clone()))
}

fn validate_bundle(
    bundle: &WorkspacePublicIdentityBundle,
) -> Result<(), WorkspaceIssuerTrustError> {
    validate_id(&bundle.workspace_id)?;
    validate_id(&bundle.key_id)?;
    if bundle.backend_url.len() > MAX_ISSUER_BYTES
        || bundle.backend_url.trim() != bundle.backend_url
        || bundle.backend_url.chars().any(char::is_control)
    {
        return Err(WorkspaceIssuerTrustError::InvalidBackendUrl);
    }
    let backend_url = url::Url::parse(&bundle.backend_url)
        .map_err(|_| WorkspaceIssuerTrustError::InvalidBackendUrl)?;
    if !matches!(backend_url.scheme(), "http" | "https")
        || backend_url.host_str().is_none()
        || !backend_url.username().is_empty()
        || backend_url.password().is_some()
        || backend_url.query().is_some()
        || backend_url.fragment().is_some()
    {
        return Err(WorkspaceIssuerTrustError::InvalidBackendUrl);
    }
    if bundle.algorithm != WORKSPACE_SIGNING_ALGORITHM {
        return Err(WorkspaceIssuerTrustError::UnsupportedAlgorithm);
    }
    if bundle.revision == 0 {
        return Err(WorkspaceIssuerTrustError::InvalidIdentityRevision);
    }
    let public_key = crate::auth::decode_public_key(&bundle.public_key)
        .map_err(|_| WorkspaceIssuerTrustError::InvalidPublicKey)?;
    let fingerprint = format!("sha256:{}", hex_lower(&Sha256::digest(&public_key)));
    if fingerprint != bundle.public_key_fingerprint {
        return Err(WorkspaceIssuerTrustError::PublicKeyFingerprintMismatch);
    }
    Ok(())
}

fn validate_id(value: &str) -> Result<(), WorkspaceIssuerTrustError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(WorkspaceIssuerTrustError::InvalidIdentifier);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCapabilityClaims {
    pub issuer: String,
    pub issuer_workspace_id: String,
    pub issuer_key_id: String,
    pub issuer_identity_revision: u64,
    pub trust_generation: u64,
    pub binding_revision: u64,
    pub runtime_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    pub operation: String,
    pub body_digest: String,
    pub iat: i64,
    pub exp: i64,
    pub jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCapabilityExpectation<'a> {
    pub workspace_id: &'a str,
    pub binding_revision: u64,
    pub runtime_id: &'a str,
    pub worker_id: Option<&'a str>,
    pub operation: &'a str,
    pub body_digest: &'a str,
    pub now_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedWorkspaceCapability {
    pub issuer: String,
    pub workspace_id: String,
    pub issuer_key_id: String,
    pub issuer_identity_revision: u64,
    pub trust_generation: u64,
    pub binding_revision: u64,
    pub runtime_id: String,
    pub worker_id: Option<String>,
    pub operation: String,
    pub token_id: String,
    pub expires_at: i64,
}

pub trait WorkspaceClaimReplayProtection: Send + Sync {
    fn consume_once(
        &self,
        workspace_id: &str,
        trust_generation: u64,
        token_id: &str,
        expires_at: i64,
        now_unix: i64,
    ) -> Result<bool, WorkspaceCapabilityVerificationError>;
}

#[derive(Default)]
pub struct InMemoryWorkspaceClaimReplayProtection {
    consumed: Mutex<HashMap<(String, u64, String), i64>>,
}

impl WorkspaceClaimReplayProtection for InMemoryWorkspaceClaimReplayProtection {
    fn consume_once(
        &self,
        workspace_id: &str,
        trust_generation: u64,
        token_id: &str,
        expires_at: i64,
        now_unix: i64,
    ) -> Result<bool, WorkspaceCapabilityVerificationError> {
        let mut consumed = self
            .consumed
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        consumed.retain(|_, expiry| *expiry > now_unix);
        let key = (
            workspace_id.to_string(),
            trust_generation,
            token_id.to_string(),
        );
        if consumed.contains_key(&key) {
            return Ok(false);
        }
        if consumed.len() >= MAX_REPLAY_ENTRIES {
            return Err(WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable);
        }
        consumed.insert(key, expires_at);
        Ok(true)
    }
}

#[derive(Clone)]
pub struct WorkspaceCapabilityVerifier {
    records: Arc<[WorkspaceIssuerTrustRecord]>,
    replay: Arc<dyn WorkspaceClaimReplayProtection>,
}

impl fmt::Debug for WorkspaceCapabilityVerifier {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceCapabilityVerifier")
            .field("trusted_workspace_count", &self.records.len())
            .finish_non_exhaustive()
    }
}

impl WorkspaceCapabilityVerifier {
    pub fn new(
        records: Vec<WorkspaceIssuerTrustRecord>,
        replay: Arc<dyn WorkspaceClaimReplayProtection>,
    ) -> Result<Self, WorkspaceCapabilityVerificationError> {
        if records.is_empty() {
            return Err(WorkspaceCapabilityVerificationError::TrustAuthorityMissing);
        }
        validate_workspace_issuer_trust_records(&records)?;
        Ok(Self {
            records: records.into(),
            replay,
        })
    }

    pub fn verify(
        &self,
        token: &str,
        expected: &WorkspaceCapabilityExpectation<'_>,
    ) -> Result<VerifiedWorkspaceCapability, WorkspaceCapabilityVerificationError> {
        if token.len() > MAX_TOKEN_BYTES {
            return Err(WorkspaceCapabilityVerificationError::MalformedToken);
        }
        let (payload, signature) = split_workspace_token(token)?;
        let claims_json = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| WorkspaceCapabilityVerificationError::MalformedToken)?;
        let claims: WorkspaceCapabilityClaims = serde_json::from_slice(&claims_json)
            .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?;
        validate_claim_shape(&claims)?;

        if claims.issuer_workspace_id != expected.workspace_id {
            return Err(WorkspaceCapabilityVerificationError::WrongWorkspace);
        }
        let record = self
            .records
            .iter()
            .find(|record| record.workspace_id == claims.issuer_workspace_id)
            .ok_or(WorkspaceCapabilityVerificationError::UnknownWorkspaceIssuer)?;
        if record.state != WorkspaceIssuerTrustState::Active {
            return Err(WorkspaceCapabilityVerificationError::IssuerRevoked);
        }
        if record.backend_url != claims.issuer {
            return Err(WorkspaceCapabilityVerificationError::WrongIssuer);
        }
        if record.trust_generation != claims.trust_generation {
            return Err(WorkspaceCapabilityVerificationError::StaleTrustGeneration);
        }
        if record.key_id != claims.issuer_key_id {
            return Err(WorkspaceCapabilityVerificationError::WrongIssuerKey);
        }
        if record.identity_revision != claims.issuer_identity_revision {
            return Err(WorkspaceCapabilityVerificationError::StaleIdentityRevision);
        }

        let public_key = crate::auth::decode_public_key(&record.public_key)
            .map_err(|_| WorkspaceCapabilityVerificationError::TrustRecordCorrupt)?;
        let signing_input = format!("{WORKSPACE_SIGNING_INPUT_PREFIX}{payload}");
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(signing_input.as_bytes(), &signature)
            .map_err(|_| WorkspaceCapabilityVerificationError::InvalidSignature)?;

        if claims.runtime_id != expected.runtime_id {
            return Err(WorkspaceCapabilityVerificationError::WrongRuntime);
        }
        if claims.binding_revision != expected.binding_revision {
            return Err(WorkspaceCapabilityVerificationError::StaleBindingRevision);
        }
        if claims.worker_id.as_deref() != expected.worker_id {
            return Err(WorkspaceCapabilityVerificationError::WrongWorker);
        }
        if claims.operation != expected.operation {
            return Err(WorkspaceCapabilityVerificationError::WrongOperation);
        }
        if claims.body_digest != expected.body_digest {
            return Err(WorkspaceCapabilityVerificationError::WrongBodyDigest);
        }
        if claims.exp <= expected.now_unix {
            return Err(WorkspaceCapabilityVerificationError::Expired);
        }
        if claims.iat > expected.now_unix.saturating_add(MAX_CLOCK_SKEW_SECONDS) {
            return Err(WorkspaceCapabilityVerificationError::IssuedInFuture);
        }
        if claims.exp <= claims.iat
            || claims.exp.saturating_sub(claims.iat) > MAX_TOKEN_LIFETIME_SECONDS
        {
            return Err(WorkspaceCapabilityVerificationError::InvalidLifetime);
        }
        if !self.replay.consume_once(
            &claims.issuer_workspace_id,
            claims.trust_generation,
            &claims.jti,
            claims.exp,
            expected.now_unix,
        )? {
            return Err(WorkspaceCapabilityVerificationError::Replay);
        }

        Ok(VerifiedWorkspaceCapability {
            issuer: claims.issuer,
            workspace_id: claims.issuer_workspace_id,
            issuer_key_id: claims.issuer_key_id,
            issuer_identity_revision: claims.issuer_identity_revision,
            trust_generation: claims.trust_generation,
            binding_revision: claims.binding_revision,
            runtime_id: claims.runtime_id,
            worker_id: claims.worker_id,
            operation: claims.operation,
            token_id: claims.jti,
            expires_at: claims.exp,
        })
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkspaceCapabilityVerificationError {
    #[error("Workspace issuer trust authority is not configured")]
    TrustAuthorityMissing,
    #[error("Workspace issuer trust contains duplicate Workspace identities")]
    DuplicateWorkspaceTrust,
    #[error("Workspace issuer trust record is corrupt")]
    TrustRecordCorrupt,
    #[error("Workspace capability token is malformed")]
    MalformedToken,
    #[error("Workspace capability claims are malformed")]
    MalformedClaims,
    #[error("Workspace capability claim identifier is invalid")]
    InvalidIdentifier,
    #[error("Workspace capability claim body digest is invalid")]
    InvalidBodyDigest,
    #[error("Workspace capability issuer is not trusted")]
    UnknownWorkspaceIssuer,
    #[error("Workspace capability issuer does not match trust")]
    WrongIssuer,
    #[error("Workspace capability issuer trust is revoked")]
    IssuerRevoked,
    #[error("Workspace capability trust generation is stale")]
    StaleTrustGeneration,
    #[error("Workspace capability issuer key does not match trust")]
    WrongIssuerKey,
    #[error("Workspace capability identity revision is stale")]
    StaleIdentityRevision,
    #[error("Workspace capability signature is invalid")]
    InvalidSignature,
    #[error("Workspace capability targets another Workspace")]
    WrongWorkspace,
    #[error("Workspace capability targets another Runtime")]
    WrongRuntime,
    #[error("Workspace capability binding revision is stale")]
    StaleBindingRevision,
    #[error("Workspace capability targets another Worker")]
    WrongWorker,
    #[error("Workspace capability does not authorize this operation")]
    WrongOperation,
    #[error("Workspace capability does not bind this request body")]
    WrongBodyDigest,
    #[error("Workspace capability has expired")]
    Expired,
    #[error("Workspace capability was issued in the future")]
    IssuedInFuture,
    #[error("Workspace capability lifetime is invalid")]
    InvalidLifetime,
    #[error("Workspace capability was already consumed")]
    Replay,
    #[error("Workspace capability replay authority is unavailable")]
    ReplayAuthorityUnavailable,
}

pub fn issue_workspace_capability_token(
    signing_key: &Ed25519KeyPair,
    claims: &WorkspaceCapabilityClaims,
) -> Result<String, WorkspaceCapabilityVerificationError> {
    validate_claim_shape(claims)?;
    let payload = serde_json::to_vec(claims)
        .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?;
    let payload = URL_SAFE_NO_PAD.encode(payload);
    let signing_input = format!("{WORKSPACE_SIGNING_INPUT_PREFIX}{payload}");
    let signature = signing_key.sign(signing_input.as_bytes());
    Ok(format!(
        "{WORKSPACE_TOKEN_PREFIX}.{payload}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

fn split_workspace_token(
    token: &str,
) -> Result<(&str, Vec<u8>), WorkspaceCapabilityVerificationError> {
    let mut parts = token.split('.');
    if parts.next() != Some(WORKSPACE_TOKEN_PREFIX) {
        return Err(WorkspaceCapabilityVerificationError::MalformedToken);
    }
    let payload = parts
        .next()
        .ok_or(WorkspaceCapabilityVerificationError::MalformedToken)?;
    let signature = parts
        .next()
        .ok_or(WorkspaceCapabilityVerificationError::MalformedToken)?;
    if parts.next().is_some() || payload.is_empty() || signature.is_empty() {
        return Err(WorkspaceCapabilityVerificationError::MalformedToken);
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| WorkspaceCapabilityVerificationError::MalformedToken)?;
    Ok((payload, signature))
}

fn validate_trust_record(
    record: &WorkspaceIssuerTrustRecord,
) -> Result<(), WorkspaceCapabilityVerificationError> {
    validate_bundle(&WorkspacePublicIdentityBundle {
        workspace_id: record.workspace_id.clone(),
        backend_url: record.backend_url.clone(),
        key_id: record.key_id.clone(),
        algorithm: record.algorithm.clone(),
        public_key: record.public_key.clone(),
        public_key_fingerprint: record.public_key_fingerprint.clone(),
        revision: record.identity_revision,
    })
    .map_err(|_| WorkspaceCapabilityVerificationError::TrustRecordCorrupt)?;
    if record.trust_generation == 0 {
        return Err(WorkspaceCapabilityVerificationError::TrustRecordCorrupt);
    }
    Ok(())
}

fn validate_claim_shape(
    claims: &WorkspaceCapabilityClaims,
) -> Result<(), WorkspaceCapabilityVerificationError> {
    if claims.issuer.len() > MAX_ISSUER_BYTES
        || claims.issuer.trim() != claims.issuer
        || claims.issuer.chars().any(char::is_control)
    {
        return Err(WorkspaceCapabilityVerificationError::WrongIssuer);
    }
    let issuer = url::Url::parse(&claims.issuer)
        .map_err(|_| WorkspaceCapabilityVerificationError::WrongIssuer)?;
    if !matches!(issuer.scheme(), "http" | "https")
        || issuer.host_str().is_none()
        || !issuer.username().is_empty()
        || issuer.password().is_some()
        || issuer.query().is_some()
        || issuer.fragment().is_some()
    {
        return Err(WorkspaceCapabilityVerificationError::WrongIssuer);
    }
    for value in [
        claims.issuer_workspace_id.as_str(),
        claims.issuer_key_id.as_str(),
        claims.runtime_id.as_str(),
        claims.jti.as_str(),
    ] {
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || value.trim() != value
            || value.chars().any(char::is_control)
        {
            return Err(WorkspaceCapabilityVerificationError::InvalidIdentifier);
        }
    }
    if let Some(worker_id) = &claims.worker_id {
        if worker_id.is_empty()
            || worker_id.len() > MAX_ID_BYTES
            || worker_id.trim() != worker_id
            || worker_id.chars().any(char::is_control)
        {
            return Err(WorkspaceCapabilityVerificationError::InvalidIdentifier);
        }
    }
    if claims.issuer_identity_revision == 0
        || claims.trust_generation == 0
        || claims.binding_revision == 0
    {
        return Err(WorkspaceCapabilityVerificationError::MalformedClaims);
    }
    if claims.operation.is_empty()
        || claims.operation.len() > MAX_OPERATION_BYTES
        || claims.operation.trim() != claims.operation
        || claims.operation.chars().any(char::is_control)
    {
        return Err(WorkspaceCapabilityVerificationError::MalformedClaims);
    }
    if !is_sha256_digest(&claims.body_digest) {
        return Err(WorkspaceCapabilityVerificationError::InvalidBodyDigest);
    }
    Ok(())
}

fn is_sha256_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn workspace_request_body_digest(body: &[u8]) -> String {
    format!("sha256:{}", hex_lower(&Sha256::digest(body)))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub enum RuntimeCapabilityVerifier {
    LegacyServer(RuntimeHttpAuthConfig),
    WorkspaceIssuer(WorkspaceCapabilityVerifier),
}

pub enum RuntimeCapabilityVerification<'a> {
    LegacyServer {
        required_permission: Option<&'a str>,
        now_seconds: u64,
    },
    WorkspaceIssuer(WorkspaceCapabilityExpectation<'a>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedRuntimeCapability {
    LegacyServer(RuntimeAuthContext),
    WorkspaceIssuer(VerifiedWorkspaceCapability),
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeCapabilityVerificationError {
    #[error("Runtime capability verifier mode does not match the verification request")]
    ModeMismatch,
    #[error(transparent)]
    LegacyServer(#[from] RuntimeAuthError),
    #[error(transparent)]
    WorkspaceIssuer(#[from] WorkspaceCapabilityVerificationError),
}

impl RuntimeCapabilityVerifier {
    pub fn verify(
        &self,
        token: &str,
        verification: RuntimeCapabilityVerification<'_>,
    ) -> Result<VerifiedRuntimeCapability, RuntimeCapabilityVerificationError> {
        match (self, verification) {
            (
                Self::LegacyServer(config),
                RuntimeCapabilityVerification::LegacyServer {
                    required_permission,
                    now_seconds,
                },
            ) => crate::auth::verify_capability_token(
                config,
                token,
                required_permission,
                now_seconds,
            )
            .map(VerifiedRuntimeCapability::LegacyServer)
            .map_err(Into::into),
            (
                Self::WorkspaceIssuer(verifier),
                RuntimeCapabilityVerification::WorkspaceIssuer(expectation),
            ) => verifier
                .verify(token, &expectation)
                .map(VerifiedRuntimeCapability::WorkspaceIssuer)
                .map_err(Into::into),
            _ => Err(RuntimeCapabilityVerificationError::ModeMismatch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::RuntimeIdentityMaterial;

    fn identity(
        workspace_id: &str,
        key_id: &str,
        revision: u64,
    ) -> (Ed25519KeyPair, WorkspacePublicIdentityBundle) {
        let material = RuntimeIdentityMaterial::generate(key_id).unwrap();
        let signing_key = material.signing_key().unwrap();
        let public_key_fingerprint = format!(
            "sha256:{}",
            hex_lower(&Sha256::digest(
                crate::auth::decode_public_key(&material.public_key).unwrap()
            ))
        );
        (
            signing_key,
            WorkspacePublicIdentityBundle {
                workspace_id: workspace_id.to_string(),
                backend_url: "https://backend.example.test".to_string(),
                key_id: key_id.to_string(),
                algorithm: WORKSPACE_SIGNING_ALGORITHM.to_string(),
                public_key: material.public_key,
                public_key_fingerprint,
                revision,
            },
        )
    }

    fn claims(record: &WorkspaceIssuerTrustRecord, jti: &str) -> WorkspaceCapabilityClaims {
        WorkspaceCapabilityClaims {
            issuer: record.backend_url.clone(),
            issuer_workspace_id: record.workspace_id.clone(),
            issuer_key_id: record.key_id.clone(),
            issuer_identity_revision: record.identity_revision,
            trust_generation: record.trust_generation,
            binding_revision: 7,
            runtime_id: "runtime-1".to_string(),
            worker_id: Some("worker-1".to_string()),
            operation: "POST /v1/workers/worker-1/submit".to_string(),
            body_digest: workspace_request_body_digest(br#"{"content":"hello"}"#),
            iat: 1_000,
            exp: 1_060,
            jti: jti.to_string(),
        }
    }

    fn expectation<'a>(body_digest: &'a str) -> WorkspaceCapabilityExpectation<'a> {
        WorkspaceCapabilityExpectation {
            workspace_id: "workspace-1",
            binding_revision: 7,
            runtime_id: "runtime-1",
            worker_id: Some("worker-1"),
            operation: "POST /v1/workers/worker-1/submit",
            body_digest,
            now_unix: 1_001,
        }
    }

    #[test]
    fn trust_mutations_are_idempotent_generation_fenced_and_reject_stale_keys() {
        let (_, bundle_v1) = identity("workspace-1", "WK-1", 1);
        let mut records = Vec::new();
        let (mutation, first) =
            add_workspace_issuer_trust(&mut records, bundle_v1.clone(), 10).unwrap();
        assert_eq!(mutation, WorkspaceIssuerTrustMutation::Added);
        assert_eq!(first.trust_generation, 1);
        let (mutation, replay) =
            add_workspace_issuer_trust(&mut records, bundle_v1.clone(), 11).unwrap();
        assert_eq!(mutation, WorkspaceIssuerTrustMutation::Unchanged);
        assert_eq!(replay, first);

        let (_, stale_other_key) = identity("workspace-1", "WK-stale", 1);
        assert_eq!(
            replace_workspace_issuer_trust(&mut records, stale_other_key, 12).unwrap_err(),
            WorkspaceIssuerTrustError::StaleIdentityRevision
        );
        let (_, bundle_v2) = identity("workspace-1", "WK-2", 2);
        let (mutation, replaced) =
            replace_workspace_issuer_trust(&mut records, bundle_v2, 13).unwrap();
        assert_eq!(mutation, WorkspaceIssuerTrustMutation::Replaced);
        assert_eq!(replaced.trust_generation, 2);
        assert_eq!(replaced.registered_at_unix, 10);
        let (_, revoked) = revoke_workspace_issuer_trust(&mut records, "workspace-1", 14).unwrap();
        assert_eq!(revoked.trust_generation, 3);
        assert_eq!(revoked.state, WorkspaceIssuerTrustState::Revoked);
        let (mutation, same) =
            revoke_workspace_issuer_trust(&mut records, "workspace-1", 15).unwrap();
        assert_eq!(mutation, WorkspaceIssuerTrustMutation::Unchanged);
        assert_eq!(same, revoked);
    }

    #[test]
    fn verifier_binds_workspace_key_generation_runtime_worker_operation_and_body() {
        let (key_1, bundle_1) = identity("workspace-1", "WK-1", 1);
        let (key_2, bundle_2) = identity("workspace-2", "WK-2", 1);
        let record_1 = WorkspaceIssuerTrustRecord::from_bundle(bundle_1, 1, 1).unwrap();
        let record_2 = WorkspaceIssuerTrustRecord::from_bundle(bundle_2, 1, 1).unwrap();
        let verifier = WorkspaceCapabilityVerifier::new(
            vec![record_1.clone(), record_2.clone()],
            Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
        )
        .unwrap();
        let expected_body = workspace_request_body_digest(br#"{"content":"hello"}"#);
        let token =
            issue_workspace_capability_token(&key_1, &claims(&record_1, "token-ok")).unwrap();
        assert_eq!(
            verifier
                .verify(&token, &expectation(&expected_body))
                .unwrap()
                .workspace_id,
            "workspace-1"
        );

        let workspace_2_token =
            issue_workspace_capability_token(&key_2, &claims(&record_2, "workspace-2")).unwrap();
        assert_eq!(
            verifier
                .verify(&workspace_2_token, &expectation(&expected_body))
                .unwrap_err(),
            WorkspaceCapabilityVerificationError::WrongWorkspace
        );
        let workspace_2_expectation = WorkspaceCapabilityExpectation {
            workspace_id: "workspace-2",
            binding_revision: 7,
            runtime_id: "runtime-1",
            worker_id: Some("worker-1"),
            operation: "POST /v1/workers/worker-1/submit",
            body_digest: &expected_body,
            now_unix: 1_001,
        };
        assert_eq!(
            verifier
                .verify(&workspace_2_token, &workspace_2_expectation)
                .unwrap()
                .workspace_id,
            "workspace-2"
        );

        let wrong_key_token =
            issue_workspace_capability_token(&key_2, &claims(&record_1, "wrong-key")).unwrap();
        assert_eq!(
            verifier
                .verify(&wrong_key_token, &expectation(&expected_body))
                .unwrap_err(),
            WorkspaceCapabilityVerificationError::InvalidSignature
        );

        type ClaimsMutation = fn(&mut WorkspaceCapabilityClaims);
        for (name, mutate, expected) in [
            (
                "runtime",
                (|claims: &mut WorkspaceCapabilityClaims| {
                    claims.runtime_id = "runtime-2".to_string()
                }) as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongRuntime,
            ),
            (
                "worker",
                (|claims: &mut WorkspaceCapabilityClaims| {
                    claims.worker_id = Some("worker-2".to_string())
                }) as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongWorker,
            ),
            (
                "binding",
                (|claims: &mut WorkspaceCapabilityClaims| claims.binding_revision = 8)
                    as ClaimsMutation,
                WorkspaceCapabilityVerificationError::StaleBindingRevision,
            ),
            (
                "issuer",
                (|claims: &mut WorkspaceCapabilityClaims| {
                    claims.issuer = "https://other-backend.example.test".to_string()
                }) as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongIssuer,
            ),
            (
                "operation",
                (|claims: &mut WorkspaceCapabilityClaims| {
                    claims.operation = "DELETE /v1/workers/worker-1".to_string()
                }) as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongOperation,
            ),
        ] {
            let mut changed = claims(&record_1, name);
            mutate(&mut changed);
            let token = issue_workspace_capability_token(&key_1, &changed).unwrap();
            assert_eq!(
                verifier
                    .verify(&token, &expectation(&expected_body))
                    .unwrap_err(),
                expected
            );
        }
        let token =
            issue_workspace_capability_token(&key_1, &claims(&record_1, "wrong-body")).unwrap();
        let wrong_body = workspace_request_body_digest(b"different");
        assert_eq!(
            verifier
                .verify(&token, &expectation(&wrong_body))
                .unwrap_err(),
            WorkspaceCapabilityVerificationError::WrongBodyDigest
        );
    }

    #[test]
    fn verifier_rejects_replay_revocation_stale_generation_expiry_and_future_claims() {
        let (key, bundle) = identity("workspace-1", "WK-1", 1);
        let record = WorkspaceIssuerTrustRecord::from_bundle(bundle, 2, 1).unwrap();
        let replay = Arc::new(InMemoryWorkspaceClaimReplayProtection::default());
        let verifier = WorkspaceCapabilityVerifier::new(vec![record.clone()], replay).unwrap();
        let body = workspace_request_body_digest(br#"{"content":"hello"}"#);
        let token = issue_workspace_capability_token(&key, &claims(&record, "replay")).unwrap();
        verifier.verify(&token, &expectation(&body)).unwrap();
        assert_eq!(
            verifier.verify(&token, &expectation(&body)).unwrap_err(),
            WorkspaceCapabilityVerificationError::Replay
        );

        let mut stale = claims(&record, "stale");
        stale.trust_generation = 1;
        let token = issue_workspace_capability_token(&key, &stale).unwrap();
        assert_eq!(
            verifier.verify(&token, &expectation(&body)).unwrap_err(),
            WorkspaceCapabilityVerificationError::StaleTrustGeneration
        );

        let mut expired = claims(&record, "expired");
        expired.iat = 900;
        expired.exp = 950;
        let token = issue_workspace_capability_token(&key, &expired).unwrap();
        assert_eq!(
            verifier.verify(&token, &expectation(&body)).unwrap_err(),
            WorkspaceCapabilityVerificationError::Expired
        );

        let mut future = claims(&record, "future");
        future.iat = 1_100;
        future.exp = 1_160;
        let token = issue_workspace_capability_token(&key, &future).unwrap();
        assert_eq!(
            verifier.verify(&token, &expectation(&body)).unwrap_err(),
            WorkspaceCapabilityVerificationError::IssuedInFuture
        );

        let mut revoked = record;
        revoked.state = WorkspaceIssuerTrustState::Revoked;
        revoked.trust_generation += 1;
        let revoked_verifier = WorkspaceCapabilityVerifier::new(
            vec![revoked.clone()],
            Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
        )
        .unwrap();
        let token = issue_workspace_capability_token(&key, &claims(&revoked, "revoked")).unwrap();
        assert_eq!(
            revoked_verifier
                .verify(&token, &expectation(&body))
                .unwrap_err(),
            WorkspaceCapabilityVerificationError::IssuerRevoked
        );
    }

    #[test]
    fn verifier_selection_never_falls_back_between_legacy_and_workspace_authority() {
        assert_eq!(
            WorkspaceCapabilityVerifier::new(
                Vec::new(),
                Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
            )
            .unwrap_err(),
            WorkspaceCapabilityVerificationError::TrustAuthorityMissing
        );
        let (key, bundle) = identity("workspace-1", "WK-1", 1);
        let record = WorkspaceIssuerTrustRecord::from_bundle(bundle, 1, 1).unwrap();
        let workspace = RuntimeCapabilityVerifier::WorkspaceIssuer(
            WorkspaceCapabilityVerifier::new(
                vec![record.clone()],
                Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
            )
            .unwrap(),
        );
        let token = issue_workspace_capability_token(&key, &claims(&record, "typed-mode")).unwrap();
        assert!(matches!(
            workspace.verify(
                &token,
                RuntimeCapabilityVerification::LegacyServer {
                    required_permission: None,
                    now_seconds: 1_001,
                },
            ),
            Err(RuntimeCapabilityVerificationError::ModeMismatch)
        ));
        assert!(matches!(
            RuntimeCapabilityVerifier::WorkspaceIssuer(
                WorkspaceCapabilityVerifier::new(
                    vec![record],
                    Arc::new(InMemoryWorkspaceClaimReplayProtection::default()),
                )
                .unwrap(),
            )
            .verify(
                &token,
                RuntimeCapabilityVerification::WorkspaceIssuer(expectation(
                    &workspace_request_body_digest(br#"{"content":"hello"}"#),
                )),
            ),
            Ok(VerifiedRuntimeCapability::WorkspaceIssuer(_))
        ));
    }
}
