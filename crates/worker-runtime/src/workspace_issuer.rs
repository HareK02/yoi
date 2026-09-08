use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::Ed25519KeyPair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use workspace_api::WorkspacePublicIdentityBundle;

use crate::auth::RuntimeAuthError;

const WORKSPACE_TOKEN_PREFIX: &str = "yoi-workspace-v1";
const WORKSPACE_SIGNING_INPUT_PREFIX: &str = "yoi.workspace.capability.v1.";
const WORKSPACE_SIGNING_ALGORITHM: &str = "ed25519";
const MAX_TOKEN_BYTES: usize = 16 * 1024;
const MAX_ID_BYTES: usize = 256;
const MAX_ISSUER_BYTES: usize = 2 * 1024;
const MAX_OPERATION_BYTES: usize = 128;
const MAX_PATH_AND_QUERY_BYTES: usize = 4 * 1024;
pub const MAX_WORKSPACE_ISSUER_TRUST_RECORDS: usize = 4_096;
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
    #[error("Workspace issuer trust record limit was reached")]
    TrustRecordLimitExceeded,
    #[error("Workspace issuer trust generation overflow")]
    TrustGenerationOverflow,
}

pub fn validate_workspace_issuer_trust_records(
    records: &[WorkspaceIssuerTrustRecord],
) -> Result<(), WorkspaceCapabilityVerificationError> {
    if records.len() > MAX_WORKSPACE_ISSUER_TRUST_RECORDS {
        return Err(WorkspaceCapabilityVerificationError::TrustRecordLimitExceeded);
    }
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
    if records.len() >= MAX_WORKSPACE_ISSUER_TRUST_RECORDS {
        return Err(WorkspaceIssuerTrustError::TrustRecordLimitExceeded);
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
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(WorkspaceIssuerTrustError::InvalidIdentifier);
    }
    Ok(())
}

pub const WORKSPACE_VERIFICATION_CHALLENGE_PATH: &str =
    "/v1/workspace-runtime-verification/challenge";
pub const WORKSPACE_VERIFICATION_ACK_PATH: &str =
    "/v1/workspace-runtime-verification/acknowledgement";
pub const WORKSPACE_VERIFICATION_OPERATION: &str = "workspace.runtime.verify";
const RUNTIME_VERIFICATION_TOKEN_PREFIX: &str = "yoi-runtime-verification-v1";
const RUNTIME_VERIFICATION_SIGNING_INPUT_PREFIX: &str = "yoi.runtime.verification.v1.";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeVerificationChallenge {
    pub challenge_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub binding_revision: u64,
    pub workspace_key_id: String,
    pub workspace_identity_revision: u64,
    pub workspace_trust_generation: u64,
    pub runtime_public_key_fingerprint: String,
    pub runtime_identity_revision: u64,
    pub workspace_nonce: String,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeVerificationResponse {
    pub challenge_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub binding_revision: u64,
    pub workspace_key_id: String,
    pub workspace_identity_revision: u64,
    pub workspace_trust_generation: u64,
    pub runtime_public_key_fingerprint: String,
    pub runtime_identity_revision: u64,
    pub workspace_nonce: String,
    pub runtime_nonce: String,
    pub expires_at: i64,
    pub response_proof: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeVerificationAcknowledgement {
    pub challenge_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub binding_revision: u64,
    pub workspace_key_id: String,
    pub workspace_identity_revision: u64,
    pub workspace_trust_generation: u64,
    pub runtime_public_key_fingerprint: String,
    pub runtime_identity_revision: u64,
    pub workspace_nonce: String,
    pub runtime_nonce: String,
    pub response_digest: String,
    pub response: WorkspaceRuntimeVerificationResponse,
    pub expires_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeVerificationReceipt {
    pub challenge_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub binding_revision: u64,
    pub accepted_at: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRuntimeVerificationRecord {
    pub workspace_id: String,
    pub runtime_id: String,
    pub binding_revision: u64,
    pub workspace_key_id: String,
    pub workspace_identity_revision: u64,
    pub workspace_trust_generation: u64,
    pub runtime_public_key_fingerprint: String,
    pub runtime_identity_revision: u64,
    pub verified_at: i64,
}

pub trait WorkspaceRuntimeVerificationAuthority: fmt::Debug + Send + Sync {
    fn get(
        &self,
        workspace_id: &str,
        runtime_id: &str,
    ) -> Result<Option<WorkspaceRuntimeVerificationRecord>, WorkspaceCapabilityVerificationError>;
    fn record(
        &self,
        record: WorkspaceRuntimeVerificationRecord,
    ) -> Result<(), WorkspaceCapabilityVerificationError>;
}

#[derive(Debug, Default)]
pub struct InMemoryWorkspaceRuntimeVerificationAuthority {
    records: Mutex<HashMap<(String, String), WorkspaceRuntimeVerificationRecord>>,
}

impl WorkspaceRuntimeVerificationAuthority for InMemoryWorkspaceRuntimeVerificationAuthority {
    fn get(
        &self,
        workspace_id: &str,
        runtime_id: &str,
    ) -> Result<Option<WorkspaceRuntimeVerificationRecord>, WorkspaceCapabilityVerificationError>
    {
        Ok(self
            .records
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?
            .get(&(workspace_id.to_string(), runtime_id.to_string()))
            .cloned())
    }

    fn record(
        &self,
        record: WorkspaceRuntimeVerificationRecord,
    ) -> Result<(), WorkspaceCapabilityVerificationError> {
        validate_verification_record(&record)?;
        self.records
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?
            .insert(
                (record.workspace_id.clone(), record.runtime_id.clone()),
                record,
            );
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FileWorkspaceRuntimeVerificationAuthority {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRuntimeVerificationDocument {
    version: u32,
    records: Vec<WorkspaceRuntimeVerificationRecord>,
}

impl FileWorkspaceRuntimeVerificationAuthority {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn read(
        &self,
    ) -> Result<WorkspaceRuntimeVerificationDocument, WorkspaceCapabilityVerificationError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let document: WorkspaceRuntimeVerificationDocument = serde_json::from_slice(&bytes)
                    .map_err(|_| {
                        WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable
                    })?;
                if document.version != 1
                    || document.records.len() > MAX_WORKSPACE_ISSUER_TRUST_RECORDS
                {
                    return Err(
                        WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable,
                    );
                }
                let mut identities = HashSet::new();
                for record in &document.records {
                    validate_verification_record(record)?;
                    if !identities
                        .insert((record.workspace_id.as_str(), record.runtime_id.as_str()))
                    {
                        return Err(
                            WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable,
                        );
                    }
                }
                Ok(document)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(WorkspaceRuntimeVerificationDocument {
                    version: 1,
                    records: Vec::new(),
                })
            }
            Err(_) => Err(WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable),
        }
    }

    fn write(
        &self,
        document: &WorkspaceRuntimeVerificationDocument,
    ) -> Result<(), WorkspaceCapabilityVerificationError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?;
        let temporary = parent.join(format!(
            ".workspace-runtime-verification-{}.tmp",
            uuid::Uuid::now_v7()
        ));
        let bytes = serde_json::to_vec(document)
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?;
        fs::write(&temporary, bytes)
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).map_err(|_| {
                WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable
            })?;
        }
        fs::rename(&temporary, &self.path).map_err(|_| {
            let _ = fs::remove_file(&temporary);
            WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable
        })?;
        Ok(())
    }
}

impl WorkspaceRuntimeVerificationAuthority for FileWorkspaceRuntimeVerificationAuthority {
    fn get(
        &self,
        workspace_id: &str,
        runtime_id: &str,
    ) -> Result<Option<WorkspaceRuntimeVerificationRecord>, WorkspaceCapabilityVerificationError>
    {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?;
        Ok(self
            .read()?
            .records
            .into_iter()
            .find(|record| record.workspace_id == workspace_id && record.runtime_id == runtime_id))
    }

    fn record(
        &self,
        record: WorkspaceRuntimeVerificationRecord,
    ) -> Result<(), WorkspaceCapabilityVerificationError> {
        validate_verification_record(&record)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable)?;
        let mut document = self.read()?;
        document.records.retain(|current| {
            current.workspace_id != record.workspace_id || current.runtime_id != record.runtime_id
        });
        document.records.push(record);
        self.write(&document)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RuntimeVerificationResponseClaims {
    response: WorkspaceRuntimeVerificationResponseUnsigned,
    method: String,
    path_and_query: String,
    body_digest: String,
    iat: i64,
    exp: i64,
    jti: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRuntimeVerificationResponseUnsigned {
    challenge_id: String,
    workspace_id: String,
    runtime_id: String,
    binding_revision: u64,
    workspace_key_id: String,
    workspace_identity_revision: u64,
    workspace_trust_generation: u64,
    runtime_public_key_fingerprint: String,
    runtime_identity_revision: u64,
    workspace_nonce: String,
    runtime_nonce: String,
    expires_at: i64,
}

impl WorkspaceRuntimeVerificationResponse {
    fn unsigned(&self) -> WorkspaceRuntimeVerificationResponseUnsigned {
        WorkspaceRuntimeVerificationResponseUnsigned {
            challenge_id: self.challenge_id.clone(),
            workspace_id: self.workspace_id.clone(),
            runtime_id: self.runtime_id.clone(),
            binding_revision: self.binding_revision,
            workspace_key_id: self.workspace_key_id.clone(),
            workspace_identity_revision: self.workspace_identity_revision,
            workspace_trust_generation: self.workspace_trust_generation,
            runtime_public_key_fingerprint: self.runtime_public_key_fingerprint.clone(),
            runtime_identity_revision: self.runtime_identity_revision,
            workspace_nonce: self.workspace_nonce.clone(),
            runtime_nonce: self.runtime_nonce.clone(),
            expires_at: self.expires_at,
        }
    }
}

#[derive(Clone)]
pub struct RuntimeVerificationSigner {
    runtime_id: String,
    public_key: String,
    public_key_fingerprint: String,
    signing_key: Arc<Ed25519KeyPair>,
}

impl fmt::Debug for RuntimeVerificationSigner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeVerificationSigner")
            .field("runtime_id", &self.runtime_id)
            .field("public_key_fingerprint", &self.public_key_fingerprint)
            .finish_non_exhaustive()
    }
}

impl RuntimeVerificationSigner {
    pub fn from_identity(
        identity: &crate::auth::RuntimeIdentityMaterial,
    ) -> Result<Self, WorkspaceCapabilityVerificationError> {
        let public_key = crate::auth::decode_public_key(&identity.public_key)
            .map_err(|_| WorkspaceCapabilityVerificationError::TrustRecordCorrupt)?;
        let fingerprint = format!("sha256:{}", hex_lower(&Sha256::digest(public_key)));
        Ok(Self {
            runtime_id: identity.identity_id.clone(),
            public_key: identity.public_key.clone(),
            public_key_fingerprint: fingerprint,
            signing_key: Arc::new(
                identity
                    .signing_key()
                    .map_err(|_| WorkspaceCapabilityVerificationError::TrustRecordCorrupt)?,
            ),
        })
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn public_key_fingerprint(&self) -> &str {
        &self.public_key_fingerprint
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    pub fn sign_response(
        &self,
        challenge: &WorkspaceRuntimeVerificationChallenge,
        runtime_nonce: String,
        now_unix: i64,
    ) -> Result<WorkspaceRuntimeVerificationResponse, WorkspaceCapabilityVerificationError> {
        validate_verification_challenge(challenge)?;
        if challenge.runtime_id != self.runtime_id
            || challenge.runtime_public_key_fingerprint != self.public_key_fingerprint
            || challenge.runtime_identity_revision == 0
            || challenge.expires_at <= now_unix
        {
            return Err(WorkspaceCapabilityVerificationError::VerificationChallengeMismatch);
        }
        let unsigned = WorkspaceRuntimeVerificationResponseUnsigned {
            challenge_id: challenge.challenge_id.clone(),
            workspace_id: challenge.workspace_id.clone(),
            runtime_id: challenge.runtime_id.clone(),
            binding_revision: challenge.binding_revision,
            workspace_key_id: challenge.workspace_key_id.clone(),
            workspace_identity_revision: challenge.workspace_identity_revision,
            workspace_trust_generation: challenge.workspace_trust_generation,
            runtime_public_key_fingerprint: challenge.runtime_public_key_fingerprint.clone(),
            runtime_identity_revision: challenge.runtime_identity_revision,
            workspace_nonce: challenge.workspace_nonce.clone(),
            runtime_nonce,
            expires_at: challenge.expires_at,
        };
        let claims = RuntimeVerificationResponseClaims {
            response: unsigned.clone(),
            method: "POST".to_string(),
            path_and_query: WORKSPACE_VERIFICATION_CHALLENGE_PATH.to_string(),
            body_digest: workspace_request_body_digest(
                &serde_json::to_vec(challenge)
                    .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?,
            ),
            iat: now_unix,
            exp: challenge.expires_at,
            jti: uuid::Uuid::now_v7().to_string(),
        };
        let proof = crate::auth::sign_json_token(
            RUNTIME_VERIFICATION_TOKEN_PREFIX,
            RUNTIME_VERIFICATION_SIGNING_INPUT_PREFIX,
            self.signing_key.as_ref(),
            &claims,
        )
        .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?;
        Ok(WorkspaceRuntimeVerificationResponse {
            challenge_id: unsigned.challenge_id,
            workspace_id: unsigned.workspace_id,
            runtime_id: unsigned.runtime_id,
            binding_revision: unsigned.binding_revision,
            workspace_key_id: unsigned.workspace_key_id,
            workspace_identity_revision: unsigned.workspace_identity_revision,
            workspace_trust_generation: unsigned.workspace_trust_generation,
            runtime_public_key_fingerprint: unsigned.runtime_public_key_fingerprint,
            runtime_identity_revision: unsigned.runtime_identity_revision,
            workspace_nonce: unsigned.workspace_nonce,
            runtime_nonce: unsigned.runtime_nonce,
            expires_at: unsigned.expires_at,
            response_proof: proof,
        })
    }
}

pub fn verify_runtime_verification_response(
    response: &WorkspaceRuntimeVerificationResponse,
    challenge: &WorkspaceRuntimeVerificationChallenge,
    runtime_public_key: &str,
    now_unix: i64,
) -> Result<(), WorkspaceCapabilityVerificationError> {
    validate_verification_challenge(challenge)?;
    let signed = crate::auth::decode_signed_json_token::<RuntimeVerificationResponseClaims>(
        &response.response_proof,
        RUNTIME_VERIFICATION_TOKEN_PREFIX,
    )
    .map_err(|_| WorkspaceCapabilityVerificationError::MalformedToken)?;
    crate::auth::verify_signed_json_token(
        RUNTIME_VERIFICATION_SIGNING_INPUT_PREFIX,
        &signed.payload,
        &signed.signature,
        runtime_public_key,
    )
    .map_err(|_| WorkspaceCapabilityVerificationError::InvalidSignature)?;
    let expected_body_digest = workspace_request_body_digest(
        &serde_json::to_vec(challenge)
            .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?,
    );
    if signed.claims.response != response.unsigned()
        || signed.claims.method != "POST"
        || signed.claims.path_and_query != WORKSPACE_VERIFICATION_CHALLENGE_PATH
        || signed.claims.body_digest != expected_body_digest
        || signed.claims.exp != response.expires_at
        || signed.claims.iat > now_unix.saturating_add(MAX_CLOCK_SKEW_SECONDS)
        || signed.claims.exp <= now_unix
        || response.challenge_id != challenge.challenge_id
        || response.workspace_id != challenge.workspace_id
        || response.runtime_id != challenge.runtime_id
        || response.binding_revision != challenge.binding_revision
        || response.workspace_key_id != challenge.workspace_key_id
        || response.workspace_identity_revision != challenge.workspace_identity_revision
        || response.workspace_trust_generation != challenge.workspace_trust_generation
        || response.runtime_public_key_fingerprint != challenge.runtime_public_key_fingerprint
        || response.runtime_identity_revision != challenge.runtime_identity_revision
        || response.workspace_nonce != challenge.workspace_nonce
        || response.runtime_nonce.is_empty()
    {
        return Err(WorkspaceCapabilityVerificationError::VerificationChallengeMismatch);
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
    pub method: String,
    pub path_and_query: String,
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
    pub method: &'a str,
    pub path_and_query: &'a str,
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
    pub method: String,
    pub path_and_query: String,
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

#[derive(Clone, Debug)]
pub struct FileWorkspaceClaimReplayProtection {
    path: PathBuf,
    lock: Arc<Mutex<()>>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceClaimReplayDocument {
    version: u32,
    entries: Vec<WorkspaceClaimReplayEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceClaimReplayEntry {
    workspace_id: String,
    trust_generation: u64,
    token_id: String,
    expires_at: i64,
}

impl FileWorkspaceClaimReplayProtection {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn read(&self) -> Result<WorkspaceClaimReplayDocument, WorkspaceCapabilityVerificationError> {
        match fs::read(&self.path) {
            Ok(bytes) => {
                let document: WorkspaceClaimReplayDocument = serde_json::from_slice(&bytes)
                    .map_err(|_| {
                        WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable
                    })?;
                if document.version != 1 || document.entries.len() > MAX_REPLAY_ENTRIES {
                    return Err(WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable);
                }
                Ok(document)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(WorkspaceClaimReplayDocument {
                    version: 1,
                    entries: Vec::new(),
                })
            }
            Err(_) => Err(WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable),
        }
    }

    fn write(
        &self,
        document: &WorkspaceClaimReplayDocument,
    ) -> Result<(), WorkspaceCapabilityVerificationError> {
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        let bytes = serde_json::to_vec(document)
            .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        let temporary = parent.join(format!(
            ".workspace-claim-replay-{}.tmp",
            uuid::Uuid::now_v7()
        ));
        fs::write(&temporary, bytes)
            .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
                .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        }
        fs::rename(&temporary, &self.path).map_err(|_| {
            let _ = fs::remove_file(&temporary);
            WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable
        })?;
        Ok(())
    }
}

impl WorkspaceClaimReplayProtection for FileWorkspaceClaimReplayProtection {
    fn consume_once(
        &self,
        workspace_id: &str,
        trust_generation: u64,
        token_id: &str,
        expires_at: i64,
        now_unix: i64,
    ) -> Result<bool, WorkspaceCapabilityVerificationError> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable)?;
        let mut document = self.read()?;
        document.entries.retain(|entry| entry.expires_at > now_unix);
        if document.entries.iter().any(|entry| {
            entry.workspace_id == workspace_id
                && entry.trust_generation == trust_generation
                && entry.token_id == token_id
        }) {
            return Ok(false);
        }
        if document.entries.len() >= MAX_REPLAY_ENTRIES {
            return Err(WorkspaceCapabilityVerificationError::ReplayAuthorityUnavailable);
        }
        document.entries.push(WorkspaceClaimReplayEntry {
            workspace_id: workspace_id.to_string(),
            trust_generation,
            token_id: token_id.to_string(),
            expires_at,
        });
        self.write(&document)?;
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

    pub fn has_active_workspace_issuer(&self, workspace_id: &str) -> bool {
        self.records.iter().any(|record| {
            record.workspace_id == workspace_id && record.state == WorkspaceIssuerTrustState::Active
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
        let signed = crate::auth::decode_signed_json_token::<WorkspaceCapabilityClaims>(
            token,
            WORKSPACE_TOKEN_PREFIX,
        )
        .map_err(|_| WorkspaceCapabilityVerificationError::MalformedToken)?;
        let claims = signed.claims;
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

        crate::auth::verify_signed_json_token(
            WORKSPACE_SIGNING_INPUT_PREFIX,
            &signed.payload,
            &signed.signature,
            &record.public_key,
        )
        .map_err(|error| match error {
            RuntimeAuthError::InvalidSignature => {
                WorkspaceCapabilityVerificationError::InvalidSignature
            }
            _ => WorkspaceCapabilityVerificationError::TrustRecordCorrupt,
        })?;

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
        if claims.method != expected.method {
            return Err(WorkspaceCapabilityVerificationError::WrongMethod);
        }
        if claims.path_and_query != expected.path_and_query {
            return Err(WorkspaceCapabilityVerificationError::WrongPathAndQuery);
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
            method: claims.method,
            path_and_query: claims.path_and_query,
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
    #[error("Workspace issuer trust contains too many records")]
    TrustRecordLimitExceeded,
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
    #[error("Workspace capability does not bind this HTTP method")]
    WrongMethod,
    #[error("Workspace capability does not bind this path and query")]
    WrongPathAndQuery,
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
    #[error("Workspace Runtime verification authority is unavailable")]
    VerificationAuthorityUnavailable,
    #[error(
        "Workspace Runtime verification challenge or response does not match current authority"
    )]
    VerificationChallengeMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceCapabilitySigningInput {
    payload: String,
    bytes: Vec<u8>,
}

impl WorkspaceCapabilitySigningInput {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub fn workspace_capability_signing_input(
    claims: &WorkspaceCapabilityClaims,
) -> Result<WorkspaceCapabilitySigningInput, WorkspaceCapabilityVerificationError> {
    validate_claim_shape(claims)?;
    let encoded = serde_json::to_vec(claims)
        .map_err(|_| WorkspaceCapabilityVerificationError::MalformedClaims)?;
    let payload = URL_SAFE_NO_PAD.encode(encoded);
    let bytes = format!("{WORKSPACE_SIGNING_INPUT_PREFIX}{payload}").into_bytes();
    Ok(WorkspaceCapabilitySigningInput { payload, bytes })
}

pub fn assemble_workspace_capability_token(
    input: WorkspaceCapabilitySigningInput,
    signature: &[u8],
) -> Result<String, WorkspaceCapabilityVerificationError> {
    if signature.len() != 64 {
        return Err(WorkspaceCapabilityVerificationError::InvalidSignature);
    }
    Ok(format!(
        "{WORKSPACE_TOKEN_PREFIX}.{}.{}",
        input.payload,
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

pub fn inspect_workspace_capability_claims(
    token: &str,
) -> Result<WorkspaceCapabilityClaims, WorkspaceCapabilityVerificationError> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(WorkspaceCapabilityVerificationError::MalformedToken);
    }
    let signed = crate::auth::decode_signed_json_token::<WorkspaceCapabilityClaims>(
        token,
        WORKSPACE_TOKEN_PREFIX,
    )
    .map_err(|_| WorkspaceCapabilityVerificationError::MalformedToken)?;
    validate_claim_shape(&signed.claims)?;
    Ok(signed.claims)
}

pub fn issue_workspace_capability_token(
    signing_key: &Ed25519KeyPair,
    claims: &WorkspaceCapabilityClaims,
) -> Result<String, WorkspaceCapabilityVerificationError> {
    let input = workspace_capability_signing_input(claims)?;
    let signature = signing_key.sign(input.bytes());
    assemble_workspace_capability_token(input, signature.as_ref())
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

fn validate_verification_record(
    record: &WorkspaceRuntimeVerificationRecord,
) -> Result<(), WorkspaceCapabilityVerificationError> {
    for value in [
        record.workspace_id.as_str(),
        record.runtime_id.as_str(),
        record.workspace_key_id.as_str(),
    ] {
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || value.trim() != value
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable);
        }
    }
    if record.binding_revision == 0
        || record.workspace_identity_revision == 0
        || record.workspace_trust_generation == 0
        || record.runtime_identity_revision == 0
        || record.verified_at <= 0
        || record.runtime_public_key_fingerprint.len() > 128
        || !record.runtime_public_key_fingerprint.starts_with("sha256:")
    {
        return Err(WorkspaceCapabilityVerificationError::VerificationAuthorityUnavailable);
    }
    Ok(())
}

fn validate_verification_challenge(
    challenge: &WorkspaceRuntimeVerificationChallenge,
) -> Result<(), WorkspaceCapabilityVerificationError> {
    for value in [
        challenge.challenge_id.as_str(),
        challenge.workspace_id.as_str(),
        challenge.runtime_id.as_str(),
        challenge.workspace_key_id.as_str(),
        challenge.workspace_nonce.as_str(),
    ] {
        if value.is_empty()
            || value.len() > MAX_ID_BYTES
            || value.trim() != value
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(WorkspaceCapabilityVerificationError::VerificationChallengeMismatch);
        }
    }
    if challenge.binding_revision == 0
        || challenge.workspace_identity_revision == 0
        || challenge.workspace_trust_generation == 0
        || challenge.runtime_identity_revision == 0
        || challenge.runtime_public_key_fingerprint.len() > 128
        || !challenge
            .runtime_public_key_fingerprint
            .starts_with("sha256:")
    {
        return Err(WorkspaceCapabilityVerificationError::VerificationChallengeMismatch);
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
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(WorkspaceCapabilityVerificationError::InvalidIdentifier);
        }
    }
    if let Some(worker_id) = &claims.worker_id {
        if worker_id.is_empty()
            || worker_id.len() > MAX_ID_BYTES
            || worker_id.trim() != worker_id
            || !worker_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
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
    if !matches!(claims.method.as_str(), "GET" | "POST" | "DELETE") {
        return Err(WorkspaceCapabilityVerificationError::MalformedClaims);
    }
    if claims.path_and_query.is_empty()
        || claims.path_and_query.len() > MAX_PATH_AND_QUERY_BYTES
        || !claims.path_and_query.starts_with('/')
        || claims.path_and_query.contains('#')
        || claims.path_and_query.chars().any(char::is_control)
    {
        return Err(WorkspaceCapabilityVerificationError::MalformedClaims);
    }
    if !is_sha256_digest(&claims.body_digest) {
        return Err(WorkspaceCapabilityVerificationError::InvalidBodyDigest);
    }
    Ok(())
}

fn is_sha256_digest(value: &str) -> bool {
    crate::auth::is_request_body_digest(value)
}

pub fn workspace_request_body_digest(body: &[u8]) -> String {
    crate::auth::request_body_digest(body)
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
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
            operation: "worker.submit".to_string(),
            method: "POST".to_string(),
            path_and_query: "/v1/workers/worker-1/submit".to_string(),
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
            operation: "worker.submit",
            method: "POST",
            path_and_query: "/v1/workers/worker-1/submit",
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

        let oversized_records =
            vec![first.clone(); MAX_WORKSPACE_ISSUER_TRUST_RECORDS.saturating_add(1)];
        assert_eq!(
            validate_workspace_issuer_trust_records(&oversized_records).unwrap_err(),
            WorkspaceCapabilityVerificationError::TrustRecordLimitExceeded
        );

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
            operation: "worker.submit",
            method: "POST",
            path_and_query: "/v1/workers/worker-1/submit",
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
                "method",
                (|claims: &mut WorkspaceCapabilityClaims| claims.method = "DELETE".to_string())
                    as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongMethod,
            ),
            (
                "path_and_query",
                (|claims: &mut WorkspaceCapabilityClaims| {
                    claims.path_and_query = "/v1/workers/worker-1/submit?retry=1".to_string()
                }) as ClaimsMutation,
                WorkspaceCapabilityVerificationError::WrongPathAndQuery,
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
    fn file_replay_protection_survives_reconstruction() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("workspace-replay.json");
        let first = FileWorkspaceClaimReplayProtection::new(&path);
        assert!(
            first
                .consume_once("workspace-a", 3, "token-a", 1_000, 100)
                .unwrap()
        );
        let restored = FileWorkspaceClaimReplayProtection::new(&path);
        assert!(
            !restored
                .consume_once("workspace-a", 3, "token-a", 1_000, 101)
                .unwrap()
        );
        assert!(
            restored
                .consume_once("workspace-a", 4, "token-a", 1_000, 101)
                .unwrap()
        );
    }

    #[test]
    fn file_runtime_verification_authority_survives_reconstruction() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("workspace-verifications.json");
        let authority = FileWorkspaceRuntimeVerificationAuthority::new(&path);
        let record = WorkspaceRuntimeVerificationRecord {
            workspace_id: "workspace-a".to_string(),
            runtime_id: "runtime-a".to_string(),
            binding_revision: 7,
            workspace_key_id: "WK-a".to_string(),
            workspace_identity_revision: 2,
            workspace_trust_generation: 3,
            runtime_public_key_fingerprint: "sha256:runtime".to_string(),
            runtime_identity_revision: 1,
            verified_at: 100,
        };
        authority.record(record.clone()).unwrap();
        let restored = FileWorkspaceRuntimeVerificationAuthority::new(&path);
        assert_eq!(
            restored.get("workspace-a", "runtime-a").unwrap(),
            Some(record)
        );
    }

    #[test]
    fn verification_response_binds_the_exact_challenge_and_runtime_identity() {
        let runtime_identity = RuntimeIdentityMaterial::generate("runtime-1").unwrap();
        let signer = RuntimeVerificationSigner::from_identity(&runtime_identity).unwrap();
        let public_key_fingerprint = format!(
            "sha256:{}",
            hex_lower(&Sha256::digest(
                crate::auth::decode_public_key(&runtime_identity.public_key).unwrap()
            ))
        );
        let challenge = WorkspaceRuntimeVerificationChallenge {
            challenge_id: "challenge-1".to_string(),
            workspace_id: "workspace-a".to_string(),
            runtime_id: "runtime-1".to_string(),
            binding_revision: 7,
            workspace_key_id: "WK-1".to_string(),
            workspace_identity_revision: 2,
            workspace_trust_generation: 3,
            runtime_public_key_fingerprint: public_key_fingerprint,
            runtime_identity_revision: 1,
            workspace_nonce: "workspace-nonce".to_string(),
            expires_at: 1_100,
        };
        let response = signer
            .sign_response(&challenge, "runtime-nonce".to_string(), 1_000)
            .unwrap();
        verify_runtime_verification_response(
            &response,
            &challenge,
            &runtime_identity.public_key,
            1_001,
        )
        .unwrap();

        let mut tampered = response.clone();
        tampered.binding_revision += 1;
        assert_eq!(
            verify_runtime_verification_response(
                &tampered,
                &challenge,
                &runtime_identity.public_key,
                1_001,
            ),
            Err(WorkspaceCapabilityVerificationError::VerificationChallengeMismatch)
        );
    }
}
