use worker_runtime::auth::{
    RuntimeAuthError, RuntimeRequestClaim, RuntimeRequestSourceClaims,
    RuntimeRequestSourceExpectation, decode_runtime_request_source_claims,
    verify_runtime_request_source,
};

use super::{remote_audience, unix_now_seconds};
use crate::server::{ServerConfig, WorkspaceApi};
use crate::store::ControlPlaneStore;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRuntimeRequestSource {
    pub runtime_id: String,
    pub worker_id: Option<String>,
    /// Existing proof JTI correlation, retained only as a fixed-length diagnostic fingerprint.
    pub token_id_hash: Option<String>,
}

/// Safe diagnostics only: never stores proof, raw claims, keys or store errors.
/// Verification is progress through proof checks, NOT an authenticated principal.
#[derive(Clone, Debug, thiserror::Error)]
#[error("invalid runtime request proof")]
pub struct RuntimeRequestProofError {
    pub(crate) reason: &'static str,
    pub(crate) verification: &'static str,
    pub(crate) runtime_id_hash: Option<String>,
    pub(crate) worker_id_hash: Option<String>,
    pub(crate) token_id_hash: Option<String>,
    pub(crate) iat: Option<i64>,
    pub(crate) exp: Option<i64>,
    pub(crate) now: Option<i64>,
    pub(crate) time_delta_seconds: Option<u64>,
}

impl RuntimeRequestProofError {
    fn undecoded() -> Self {
        Self {
            reason: "invalid",
            verification: "unverified",
            runtime_id_hash: None,
            worker_id_hash: None,
            token_id_hash: None,
            iat: None,
            exp: None,
            now: None,
            time_delta_seconds: None,
        }
    }

    fn decoded(claims: &RuntimeRequestSourceClaims) -> Self {
        Self {
            runtime_id_hash: Some(diagnostic_id_hash(&claims.iss)),
            worker_id_hash: claims.worker_id.as_deref().map(diagnostic_id_hash),
            token_id_hash: Some(diagnostic_id_hash(&claims.jti)),
            ..Self::undecoded()
        }
    }

    fn reason(mut self, reason: &'static str) -> Self {
        self.reason = reason;
        self
    }

    fn auth_error(mut self, error: RuntimeAuthError) -> Self {
        self.reason = match error {
            RuntimeAuthError::InvalidTokenFormat => "invalid_format",
            RuntimeAuthError::InvalidBase64(_) => "invalid_encoding",
            RuntimeAuthError::MalformedClaims(_) => "malformed_claims",
            RuntimeAuthError::InvalidPublicKeyFormat => "invalid_runtime_public_key",
            RuntimeAuthError::InvalidSignature => "invalid_signature",
            RuntimeAuthError::ClaimMismatch(claim) => {
                self.verification = "signature_verified";
                match claim {
                    RuntimeRequestClaim::Issuer => "issuer_mismatch",
                    RuntimeRequestClaim::Audience => "audience_mismatch",
                    RuntimeRequestClaim::Workspace => "workspace_mismatch",
                    RuntimeRequestClaim::Worker => "worker_mismatch",
                    RuntimeRequestClaim::Permission => "permission_mismatch",
                    RuntimeRequestClaim::Method => "method_mismatch",
                    RuntimeRequestClaim::Path => "path_mismatch",
                    RuntimeRequestClaim::BodyDigest => "body_digest_mismatch",
                }
            }
            RuntimeAuthError::RequestIssuedInFuture { iat, exp, now } => {
                self.time(iat, exp, now, iat.abs_diff(now));
                "issued_in_future"
            }
            RuntimeAuthError::RequestExpired { iat, exp, now } => {
                self.time(iat, exp, now, now.abs_diff(exp));
                "expired"
            }
            _ => "invalid",
        };
        self
    }

    fn time(&mut self, iat: i64, exp: i64, now: i64, delta: u64) {
        self.verification = "signature_verified";
        self.iat = Some(iat);
        self.exp = Some(exp);
        self.now = Some(now);
        self.time_delta_seconds = Some(delta);
    }
}

/// Domain-separated fixed-length fingerprints, not raw request-derived IDs.
/// Operators can correlate a known ID by applying this same function.
pub fn diagnostic_id_hash(id: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"yoi-runtime-proof-diagnostic-id-v1\0");
    hash.update(id.as_bytes());
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub async fn verify_runtime_request_source_proof(
    api: &WorkspaceApi,
    proof: &str,
    workspace_id: &str,
    permission: &str,
    method: &str,
    path: &str,
    body_digest: &str,
) -> Result<VerifiedRuntimeRequestSource, RuntimeRequestProofError> {
    verify_runtime_request_source_proof_with_store(
        api.store.as_ref(),
        &api.config,
        proof,
        workspace_id,
        permission,
        method,
        path,
        body_digest,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn verify_runtime_request_source_proof_with_store(
    store: &dyn ControlPlaneStore,
    config: &ServerConfig,
    proof: &str,
    workspace_id: &str,
    permission: &str,
    method: &str,
    path: &str,
    body_digest: &str,
) -> Result<VerifiedRuntimeRequestSource, RuntimeRequestProofError> {
    verify_runtime_request_source_proof_with_clock(
        store,
        config,
        proof,
        workspace_id,
        permission,
        method,
        path,
        body_digest,
        &|| i64::try_from(unix_now_seconds()).unwrap_or(i64::MAX),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn verify_runtime_request_source_proof_with_clock(
    store: &dyn ControlPlaneStore,
    config: &ServerConfig,
    proof: &str,
    workspace_id: &str,
    permission: &str,
    method: &str,
    path: &str,
    body_digest: &str,
    clock: &(dyn Fn() -> i64 + Send + Sync),
) -> Result<VerifiedRuntimeRequestSource, RuntimeRequestProofError> {
    let unverified = decode_runtime_request_source_claims(proof)
        .map_err(|error| RuntimeRequestProofError::undecoded().auth_error(error))?;
    let diagnostic = RuntimeRequestProofError::decoded(&unverified);
    let audience = remote_audience(config, workspace_id)
        .map_err(|_| diagnostic.clone().reason("backend_audience_unavailable"))?;
    let trusted = store
        .get_workspace_runtime_binding(workspace_id, &unverified.iss)
        .await
        .map_err(|_| {
            diagnostic
                .clone()
                .reason("runtime_trust_authority_unavailable")
        })?
        .filter(|record| record.revoked_at.is_none())
        .ok_or_else(|| {
            diagnostic
                .clone()
                .reason("runtime_trust_missing_or_revoked")
        })?;
    let expected = RuntimeRequestSourceExpectation {
        identity_id: &unverified.iss,
        audience,
        workspace_id,
        worker_id: unverified.worker_id.as_deref(),
        permission,
        method,
        path,
        body_digest,
        now_unix: clock(),
    };
    let claims =
        verify_runtime_request_source(proof, &trusted.public_key, &expected).map_err(|error| {
            // Proof encoding was already decoded successfully. A base64 error
            // at this stage is in the configured public key, not the proof.
            let error = match error {
                RuntimeAuthError::InvalidBase64(_) => RuntimeAuthError::InvalidPublicKeyFormat,
                other => other,
            };
            diagnostic.clone().auth_error(error)
        })?;
    let diagnostic = RuntimeRequestProofError {
        verification: "request_verified",
        ..diagnostic
    };
    let now_seconds = u64::try_from(expected.now_unix).unwrap_or(u64::MAX);
    let expires_at = u64::try_from(claims.exp).unwrap_or(0);
    let consumed_at = chrono::DateTime::from_timestamp(expected.now_unix, 0)
        .ok_or_else(|| diagnostic.clone().reason("verification_time_out_of_range"))?
        .to_rfc3339();
    if !store
        .consume_worker_mutation_source_jti(
            workspace_id,
            &claims.iss,
            &claims.jti,
            expires_at,
            now_seconds,
            &consumed_at,
        )
        .await
        .map_err(|_| diagnostic.clone().reason("replay_authority_unavailable"))?
    {
        return Err(diagnostic.reason("replay"));
    }
    if let Some(worker_id) = claims.worker_id.as_deref() {
        let worker = worker_runtime::identity::RuntimeWorkerRef {
            runtime_id: claims.iss.clone(),
            worker_id: worker_id.to_owned(),
        };
        let member = store
            .get_worker_registry(workspace_id, &worker)
            .map_err(|_| {
                diagnostic
                    .clone()
                    .reason("worker_catalog_authority_unavailable")
            })?;
        let reserved = store
            .has_active_worker_create_reservation(workspace_id, &worker)
            .map_err(|_| {
                diagnostic
                    .clone()
                    .reason("worker_reservation_authority_unavailable")
            })?;
        if member.is_none() && !reserved {
            return Err(diagnostic.reason("worker_catalog_membership"));
        }
        store
            .require_current_worker_singleton_owner(workspace_id, &worker)
            .map_err(|_| {
                diagnostic
                    .clone()
                    .reason("worker_singleton_authority_denied")
            })?;
    }
    Ok(VerifiedRuntimeRequestSource {
        token_id_hash: Some(diagnostic_id_hash(&claims.jti)),
        runtime_id: claims.iss,
        worker_id: claims.worker_id,
    })
}
