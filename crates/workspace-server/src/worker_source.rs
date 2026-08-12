use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use worker_runtime::auth::{
    WorkerMutationActorKind, WorkerMutationOperation, WorkerMutationSourceClaims,
    WorkerMutationSourceExpectation, decode_worker_mutation_source_claims,
    verify_worker_mutation_source_proof,
};
use worker_runtime::worker_source::InProcessWorkerMutationProof;

use crate::hosts::RemoteRuntimeConfig;
use crate::server::WorkspaceApi;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresentedWorkerMutationSourceProof<'a> {
    Remote(&'a str),
    InProcess(InProcessWorkerMutationProof),
}

pub fn presented_worker_remove_source<'a>(
    headers: &'a HeaderMap,
    in_process: Option<InProcessWorkerMutationProof>,
) -> Result<PresentedWorkerMutationSourceProof<'a>, WorkerMutationSourceProofError> {
    if let Some(claims) = in_process {
        return Ok(PresentedWorkerMutationSourceProof::InProcess(claims));
    }
    headers
        .get(worker_runtime::auth::WORKER_MUTATION_SOURCE_PROOF_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .map(PresentedWorkerMutationSourceProof::Remote)
        .ok_or(WorkerMutationSourceProofError::Missing)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedWorkerMutationSource {
    pub runtime_id: String,
    pub worker_id: String,
    pub actor_kind: WorkerMutationActorKind,
    pub permission: String,
    pub jti: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerMutationSourceProofError {
    #[error("Worker mutation source proof is required")]
    Missing,
    #[error("Worker mutation source proof is invalid")]
    Invalid,
    #[error("Worker mutation source proof is not authorized for this Server")]
    WrongAudience,
    #[error("Worker mutation source proof is not authorized for this Workspace")]
    WrongWorkspace,
    #[error("Worker mutation source proof actor is not allowed")]
    WrongActor,
    #[error("Worker mutation source proof lacks `{0}` permission")]
    MissingPermission(String),
    #[error("Worker mutation source proof is expired")]
    Expired,
    #[error("Runtime trust is missing or revoked")]
    RevokedRuntimeTrust,
    #[error("Worker mutation source proof was already consumed")]
    Replay,
    #[error("source Worker is not a current member of this Workspace Runtime catalog")]
    WorkerCatalogMembership,
    #[error("source proof authority failed: {0}")]
    Authority(String),
}

pub async fn verify_worker_remove_source(
    api: &WorkspaceApi,
    proof: PresentedWorkerMutationSourceProof<'_>,
    target_runtime_id: &str,
    target_worker_id: &str,
) -> Result<VerifiedWorkerMutationSource, WorkerMutationSourceProofError> {
    verify_worker_remove_source_with(
        &api.config,
        &api.store,
        proof,
        target_runtime_id,
        target_worker_id,
    )
    .await
}

async fn verify_worker_remove_source_with(
    config: &crate::server::ServerConfig,
    store: &std::sync::Arc<dyn crate::store::ControlPlaneStore>,
    proof: PresentedWorkerMutationSourceProof<'_>,
    target_runtime_id: &str,
    target_worker_id: &str,
) -> Result<VerifiedWorkerMutationSource, WorkerMutationSourceProofError> {
    let required_permission = worker_runtime::auth::WORKER_REMOVE_PERMISSION;
    let now = unix_now_seconds();
    let claims = match proof {
        PresentedWorkerMutationSourceProof::Remote(token) => {
            let unverified = decode_worker_mutation_source_claims(token)
                .map_err(|_| WorkerMutationSourceProofError::Invalid)?;
            let audience = remote_audience(config, &unverified.iss)?;
            let trusted = store
                .get_trusted_runtime(&unverified.iss)
                .await
                .map_err(|error| WorkerMutationSourceProofError::Authority(error.to_string()))?
                .filter(|record| record.revoked_at.is_none())
                .ok_or(WorkerMutationSourceProofError::RevokedRuntimeTrust)?;
            let expected = WorkerMutationSourceExpectation {
                runtime_id: &unverified.iss,
                audience,
                workspace_id: &config.workspace_id,
                worker_id: None,
                actor_kind: WorkerMutationActorKind::Worker,
                operation: WorkerMutationOperation::WorkerRemove,
                target_runtime_id,
                target_worker_id,
                permission: required_permission,
            };
            verify_worker_mutation_source_proof(&trusted.public_key, token, &expected, now)
                .map_err(map_auth_error)?
        }
        PresentedWorkerMutationSourceProof::InProcess(proof) => {
            let claims = proof.into_claims();
            if config
                .remote_runtime_sources
                .iter()
                .any(|runtime| runtime.runtime_id == claims.iss)
            {
                return Err(WorkerMutationSourceProofError::Invalid);
            }
            validate_in_process_claims(
                &claims,
                &format!("embedded:{}", config.workspace_id),
                &config.workspace_id,
                target_runtime_id,
                target_worker_id,
                required_permission,
                now,
            )?;
            claims
        }
    };

    let worker = worker_runtime::identity::RuntimeWorkerRef {
        runtime_id: claims.iss.clone(),
        worker_id: claims.worker_id.clone(),
    };
    let member = store
        .get_worker_registry(&config.workspace_id, &worker)
        .map_err(|error| WorkerMutationSourceProofError::Authority(error.to_string()))?;
    if member.is_none() {
        return Err(WorkerMutationSourceProofError::WorkerCatalogMembership);
    }

    let consumed_at = chrono::Utc::now().to_rfc3339();
    let consumed = store
        .consume_worker_mutation_source_jti(&claims.iss, &claims.jti, claims.exp, now, &consumed_at)
        .await
        .map_err(|error| WorkerMutationSourceProofError::Authority(error.to_string()))?;
    if !consumed {
        return Err(WorkerMutationSourceProofError::Replay);
    }

    Ok(VerifiedWorkerMutationSource {
        runtime_id: claims.iss,
        worker_id: claims.worker_id,
        actor_kind: claims.actor_kind,
        permission: claims.permission,
        jti: claims.jti,
    })
}

pub(crate) trait VerifiedWorkerRemoveExecutor: Send + Sync {
    fn execute(
        &self,
        source: VerifiedWorkerMutationSource,
        target_runtime_id: &str,
        target_worker_id: &str,
        expected_worker_revision: &str,
        reason: &str,
    ) -> Result<worker::WorkspaceResponse, String>;
}

#[derive(Clone)]
pub(crate) struct EmbeddedServerWorkerMutationDispatcher {
    config: crate::server::ServerConfig,
    store: Arc<dyn crate::store::ControlPlaneStore>,
    executor: Arc<OnceLock<Arc<dyn VerifiedWorkerRemoveExecutor>>>,
}

impl EmbeddedServerWorkerMutationDispatcher {
    pub(crate) fn new(
        config: crate::server::ServerConfig,
        store: Arc<dyn crate::store::ControlPlaneStore>,
    ) -> Self {
        Self {
            config,
            store,
            executor: Arc::new(OnceLock::new()),
        }
    }

    pub(crate) fn install_executor(
        &self,
        executor: Arc<dyn VerifiedWorkerRemoveExecutor>,
    ) -> Result<(), &'static str> {
        self.executor
            .set(executor)
            .map_err(|_| "WorkerRemove executor is already installed")
    }
}

impl worker_runtime::worker_source::EmbeddedWorkerMutationDispatcher
    for EmbeddedServerWorkerMutationDispatcher
{
    fn execute_worker_remove(
        &self,
        proof: InProcessWorkerMutationProof,
        target_runtime_id: &str,
        target_worker_id: &str,
        expected_worker_revision: &str,
        reason: &str,
    ) -> Result<
        worker::WorkspaceResponse,
        worker_runtime::worker_source::RuntimeWorkerMutationForwardError,
    > {
        let source = futures::executor::block_on(verify_worker_remove_source_with(
            &self.config,
            &self.store,
            PresentedWorkerMutationSourceProof::InProcess(proof),
            target_runtime_id,
            target_worker_id,
        ))
        .map_err(|error| {
            worker_runtime::worker_source::RuntimeWorkerMutationForwardError::Embedded(
                error.to_string(),
            )
        })?;
        let executor = self.executor.get().ok_or_else(|| {
            worker_runtime::worker_source::RuntimeWorkerMutationForwardError::Embedded(
                "WorkerRemove executor is unavailable".to_string(),
            )
        })?;
        executor
            .execute(
                source,
                target_runtime_id,
                target_worker_id,
                expected_worker_revision,
                reason,
            )
            .map_err(worker_runtime::worker_source::RuntimeWorkerMutationForwardError::Embedded)
    }
}

fn remote_audience<'a>(
    config: &'a crate::server::ServerConfig,
    runtime_id: &str,
) -> Result<&'a str, WorkerMutationSourceProofError> {
    config
        .remote_runtime_sources
        .iter()
        .find(|runtime| runtime.runtime_id == runtime_id)
        .and_then(|runtime: &RemoteRuntimeConfig| runtime.auth.as_ref())
        .map(|auth| auth.server_id.as_str())
        .ok_or(WorkerMutationSourceProofError::RevokedRuntimeTrust)
}

fn validate_in_process_claims(
    claims: &WorkerMutationSourceClaims,
    audience: &str,
    workspace_id: &str,
    target_runtime_id: &str,
    target_worker_id: &str,
    permission: &str,
    now: u64,
) -> Result<(), WorkerMutationSourceProofError> {
    if claims.aud != audience {
        return Err(WorkerMutationSourceProofError::WrongAudience);
    }
    if claims.workspace_id != workspace_id {
        return Err(WorkerMutationSourceProofError::WrongWorkspace);
    }
    if claims.actor_kind != WorkerMutationActorKind::Worker {
        return Err(WorkerMutationSourceProofError::WrongActor);
    }
    if claims.operation != WorkerMutationOperation::WorkerRemove
        || claims.target_runtime_id != target_runtime_id
        || claims.target_worker_id != target_worker_id
    {
        return Err(WorkerMutationSourceProofError::Invalid);
    }
    if claims.permission != permission {
        return Err(WorkerMutationSourceProofError::MissingPermission(
            permission.to_string(),
        ));
    }
    if claims.exp <= now || claims.iat > now.saturating_add(60) || claims.jti.trim().is_empty() {
        return Err(WorkerMutationSourceProofError::Expired);
    }
    Ok(())
}

fn map_auth_error(error: worker_runtime::auth::RuntimeAuthError) -> WorkerMutationSourceProofError {
    use worker_runtime::auth::RuntimeAuthError;
    match error {
        RuntimeAuthError::WrongAudience { .. } => WorkerMutationSourceProofError::WrongAudience,
        RuntimeAuthError::WrongWorkspace { .. } => WorkerMutationSourceProofError::WrongWorkspace,
        RuntimeAuthError::WrongActorKind => WorkerMutationSourceProofError::WrongActor,
        RuntimeAuthError::WrongOperation | RuntimeAuthError::WrongMutationTarget => {
            WorkerMutationSourceProofError::Invalid
        }
        RuntimeAuthError::MissingPermission(permission) => {
            WorkerMutationSourceProofError::MissingPermission(permission)
        }
        RuntimeAuthError::Expired => WorkerMutationSourceProofError::Expired,
        _ => WorkerMutationSourceProofError::Invalid,
    }
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
