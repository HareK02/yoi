//! Runtime request proof verification and secret-free rejection diagnostics.
//!
//! Unverified issuers only select public keys from current Workspace bindings. After matching
//! signature, Backend audience, Workspace, permission, method, path/query, body digest, and time,
//! verification consumes the JTI transactionally and checks Worker registration or creation
//! reservations and singleton ownership. One sampled clock serves time checks and JTI records;
//! the lower-level verifier owns tolerance and ordering. Diagnostics do not recompute rejection
//! with another clock. Trust reads, JTI consumption and catalog checks use bounded DB ownership;
//! capacity/authority failure is operational unavailability, not an invalid proof. Cancellation
//! after admission may still consume the JTI. Successful verification returns the exact binding
//! identity for transaction-local mutation fences, not a newly resolved authentication source.
//!
//! Rejection reasons expose only safe categories and verification stages. Claim-derived identities
//! use domain-separated hashes, omitted on decode failure. request_verified may still precede
//! replay/registration/ownership checks; it is not an authenticated principal. Diagnostics do not
//! infer key-mismatch causes or absence/revocation distinctions the store does not make.
//! Proofs, raw claims, keys, bodies, and store error text are not retained. Identity hashes are
//! not encryption and do not make guessable IDs secret.

use std::sync::Arc;

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
    /// The binding actually used for signature verification. Mutation boundaries
    /// compare this identity with current trust inside their write transaction.
    pub authentication_binding_id: String,
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
    /// Operational unavailability is not a proof or principal rejection.
    pub(crate) availability: bool,
}

impl RuntimeRequestProofError {
    pub fn is_availability_error(&self) -> bool {
        self.availability
    }

    fn unavailable(mut self, reason: &'static str) -> Self {
        self.reason = reason;
        self.availability = true;
        self
    }

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
            availability: false,
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

impl From<crate::Error> for RuntimeRequestProofError {
    fn from(error: crate::Error) -> Self {
        let reason = match error {
            crate::Error::DbCapacityExceeded => "db_capacity_exceeded",
            crate::Error::DbExecutorUnavailable => "db_executor_unavailable",
            crate::Error::DbOperationOutcomeUnknown => "db_operation_outcome_unknown",
            _ => "database_authority_unavailable",
        };
        Self::undecoded().unavailable(reason)
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
        Arc::clone(&api.store),
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
    store: Arc<dyn ControlPlaneStore>,
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
    store: Arc<dyn ControlPlaneStore>,
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
    let audience = remote_audience(config, workspace_id).map_err(|_| {
        diagnostic
            .clone()
            .unavailable("backend_audience_unavailable")
    })?;
    let trusted = store
        .get_workspace_runtime_binding(workspace_id, &unverified.iss)
        .await
        .map_err(|_| {
            diagnostic
                .clone()
                .unavailable("runtime_trust_authority_unavailable")
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
        .map_err(|_| {
            diagnostic
                .clone()
                .unavailable("replay_authority_unavailable")
        })?
    {
        return Err(diagnostic.reason("replay"));
    }
    if let Some(worker_id) = claims.worker_id.as_deref() {
        let worker = worker_runtime::identity::RuntimeWorkerRef {
            runtime_id: claims.iss.clone(),
            worker_id: worker_id.to_owned(),
        };
        let workspace_id = workspace_id.to_owned();
        let executor = store.db_executor();
        let stage_diagnostic = diagnostic.clone();
        // Only synchronous catalog authority lives in this closure. The trust
        // and JTI async wrappers above already offload through the executor.
        executor
            .run(move || {
                let member = store
                    .get_worker_registry(&workspace_id, &worker)
                    .map_err(|_| {
                        stage_diagnostic
                            .clone()
                            .unavailable("worker_catalog_authority_unavailable")
                    })?;
                let reserved = store
                    .has_active_worker_create_reservation(&workspace_id, &worker)
                    .map_err(|_| {
                        stage_diagnostic
                            .clone()
                            .unavailable("worker_reservation_authority_unavailable")
                    })?;
                if member.is_none() && !reserved {
                    return Err(stage_diagnostic.reason("worker_catalog_membership"));
                }
                store
                    .require_current_worker_singleton_owner(&workspace_id, &worker)
                    .map_err(|error| {
                        if matches!(
                            error,
                            crate::Error::DbCapacityExceeded
                                | crate::Error::DbExecutorUnavailable
                                | crate::Error::DbOperationOutcomeUnknown
                                | crate::Error::Sqlite(_)
                        ) {
                            stage_diagnostic
                                .clone()
                                .unavailable("worker_singleton_authority_unavailable")
                        } else {
                            stage_diagnostic
                                .clone()
                                .reason("worker_singleton_authority_denied")
                        }
                    })?;
                Ok(())
            })
            .await
            .map_err(|error: RuntimeRequestProofError| {
                if error.verification == "unverified" && error.is_availability_error() {
                    diagnostic.clone().unavailable(error.reason)
                } else {
                    error
                }
            })?;
    }
    Ok(VerifiedRuntimeRequestSource {
        token_id_hash: Some(diagnostic_id_hash(&claims.jti)),
        runtime_id: claims.iss,
        worker_id: claims.worker_id,
        authentication_binding_id: trusted.binding_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{
        AccountRecord, SqliteWorkspaceStore, WorkspaceRecord, WorkspaceRuntimeAuthenticationMode,
        WorkspaceRuntimeBinding,
    };
    use std::time::Duration;
    use worker_runtime::auth::{
        BACKEND_RESOURCE_FETCH_PERMISSION, RuntimeIdentityMaterial, RuntimeRequestSourceSigner,
        request_body_digest,
    };

    const WORKSPACE: &str = "019d0000-0000-7000-8000-0000000000aa";
    const NOW: i64 = 1_700_000_000;
    const PATH: &str = "/api/runtime/v1/resources/fetch";

    // rusqlite's busy handler accepts a function pointer, not a closure. Each
    // fixture owns a dedicated single-worker executor, so this thread-local
    // handshake is isolated from all parallel tests and product connections.
    thread_local! {
        static SQLITE_BUSY_HANDSHAKE: std::cell::RefCell<Option<(
            tokio::sync::oneshot::Sender<()>,
            std::sync::mpsc::Receiver<()>,
        )>> = const { std::cell::RefCell::new(None) };
    }

    fn acknowledge_sqlite_contention(_attempt: i32) -> bool {
        SQLITE_BUSY_HANDSHAKE.with(|slot| {
            let Some((blocked, retry)) = slot.borrow_mut().take() else {
                return false;
            };
            let _ = blocked.send(());
            // The test releases SQLite's writer lock before allowing retry.
            // Dropping the sender on panic also releases this worker promptly.
            retry.recv_timeout(Duration::from_secs(3)).is_ok()
        })
    }

    async fn fixture(
        root: &std::path::Path,
    ) -> (Arc<SqliteWorkspaceStore>, ServerConfig, String, String) {
        let db_path = root.join("auth.db");
        let executor = crate::db_executor::DbExecutor::new(crate::db_executor::DbExecutorLimits {
            workers: std::num::NonZeroUsize::new(1).unwrap(),
            queued: std::num::NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();
        let store = Arc::new(
            SqliteWorkspaceStore::open_for_test(&db_path)
                .unwrap()
                .with_db_executor(executor),
        );
        store
            .upsert_account(&AccountRecord {
                account_id: "account-auth".into(),
                kind: "user".into(),
                handle: "auth".into(),
                display_name: "Auth".into(),
                created_at: "1".into(),
                updated_at: "1".into(),
            })
            .unwrap();
        let workspace = WorkspaceRecord {
            workspace_id: WORKSPACE.into(),
            owner_account_id: "account-auth".into(),
            display_name: "Auth".into(),
            state: "active".into(),
            created_at: "1".into(),
            updated_at: "1".into(),
        };
        store.upsert_workspace(&workspace).await.unwrap();
        let config = ServerConfig {
            workspace_id: WORKSPACE.into(),
            workspace_display_name: "Auth".into(),
            workspace_created_at: "1".into(),
            workspace_execution_root: root.to_owned(),
            database_path: db_path,
            data_root: root.to_owned(),
            embedded_runtime_store_root: root.join("runtime"),
            static_assets_dir: None,
            auth: crate::server::AuthConfig::Passkey {
                rp_id: "localhost".into(),
                origin: "http://localhost".into(),
                public_base_url: "http://localhost".into(),
                cookie_name: "session".into(),
            },
            max_records: 10,
            repositories: vec![],
            runtime_event_sources: vec![],
            remote_runtime_sources: vec![],
            backend_base_url: Some("server-auth".into()),
            control_plane_endpoints: Default::default(),
        };
        let identity = RuntimeIdentityMaterial::generate("runtime-auth").unwrap();
        store
            .upsert_workspace_runtime_binding_record(
                WorkspaceRuntimeBinding {
                    workspace_id: WORKSPACE.into(),
                    runtime_id: "runtime-auth".into(),
                    display_name: "Auth".into(),
                    base_url: "http://127.0.0.1:1".into(),
                    public_key: identity.public_key.clone(),
                    public_key_fingerprint: String::new(),
                    binding_id: String::new(),
                    authentication_mode: WorkspaceRuntimeAuthenticationMode::LegacyServerIssuer,
                    created_at: "1".into(),
                    updated_at: "1".into(),
                    revoked_at: None,
                },
                false,
            )
            .await
            .unwrap();
        let binding = ControlPlaneStore::get_workspace_runtime_binding(
            store.as_ref(),
            WORKSPACE,
            "runtime-auth",
        )
        .await
        .unwrap()
        .unwrap();
        let proof = RuntimeRequestSourceSigner::from_identity(&identity)
            .issue(
                "server-auth",
                WORKSPACE,
                None,
                BACKEND_RESOURCE_FETCH_PERMISSION,
                "POST",
                PATH,
                b"{}",
                NOW,
                30,
            )
            .unwrap();
        (store, config, proof, binding.binding_id)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn sqlite_blocked_jti_keeps_timer_and_control_tasks_responsive_and_rejects_replay() {
        let root = tempfile::tempdir().unwrap();
        let (store, config, proof, binding_id) = fixture(root.path()).await;
        // A real independent SQLite writer prevents JTI insertion while trust
        // reads remain possible. Rollback on Drop also releases this on panic.
        let mut writer = rusqlite::Connection::open(&config.database_path).unwrap();
        let lock = writer
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let (blocked, contention_reached) = tokio::sync::oneshot::channel();
        let (retry, retry_rx) = std::sync::mpsc::channel();
        let setup_store = store.clone();
        store
            .db_executor()
            .run(move || -> crate::Result<()> {
                SQLITE_BUSY_HANDSHAKE.with(|slot| *slot.borrow_mut() = Some((blocked, retry_rx)));
                setup_store.with_conn(|conn| {
                    conn.busy_handler(Some(acknowledge_sqlite_contention))?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        let request_store = store.clone();
        let request_config = config.clone();
        let request_proof = proof.clone();
        let task = tokio::spawn(async move {
            verify_runtime_request_source_proof_with_clock(
                request_store,
                &request_config,
                &request_proof,
                WORKSPACE,
                BACKEND_RESOURCE_FETCH_PERMISSION,
                "POST",
                PATH,
                &request_body_digest(b"{}"),
                &|| NOW,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(1), contention_reached)
            .await
            .expect(
                "SQLite JTI persistence must report real writer contention without blocking Tokio",
            )
            .unwrap();
        // The busy callback acknowledges an actual SQLite write blocked by the
        // independent writer, then holds retry until explicitly released.
        assert!(
            !task.is_finished(),
            "JTI must remain pending while SQLite writer is locked"
        );
        let control = tokio::spawn(async { "control-responsive" });
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), control)
                .await
                .unwrap()
                .unwrap(),
            "control-responsive"
        );
        // This timer is the behavior being tested, not a guessed wait for SQL.
        tokio::time::timeout(
            Duration::from_secs(1),
            tokio::time::sleep(Duration::from_millis(1)),
        )
        .await
        .unwrap();
        assert!(
            !task.is_finished(),
            "timer must advance while JTI remains blocked"
        );
        lock.rollback().unwrap();
        retry.send(()).unwrap();
        let source = tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(source.runtime_id, "runtime-auth");
        assert_eq!(source.authentication_binding_id, binding_id);
        let replay = verify_runtime_request_source_proof_with_clock(
            store,
            &config,
            &proof,
            WORKSPACE,
            BACKEND_RESOURCE_FETCH_PERMISSION,
            "POST",
            PATH,
            &request_body_digest(b"{}"),
            &|| NOW,
        )
        .await
        .unwrap_err();
        assert_eq!(replay.reason, "replay");
        assert!(!replay.is_availability_error());
    }

    #[tokio::test]
    async fn saturated_database_is_availability_failure_not_invalid_proof() {
        let root = tempfile::tempdir().unwrap();
        let (store, config, proof, _) = fixture(root.path()).await;
        // Fill the isolated executor with a blocked job and queued jobs. No
        // scheduling sleeps: the worker explicitly acknowledges its start.
        let executor = store.db_executor();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let blocker_executor = executor.clone();
        let blocker = tokio::spawn(async move {
            blocker_executor
                .run(move || -> crate::Result<()> {
                    let _ = started.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), started_rx)
            .await
            .unwrap()
            .unwrap();
        let mut queued = Box::pin(executor.run(|| -> crate::Result<()> { Ok(()) }));
        assert!(futures::poll!(queued.as_mut()).is_pending());
        let outcome = tokio::time::timeout(
            Duration::from_secs(1),
            verify_runtime_request_source_proof_with_clock(
                store,
                &config,
                &proof,
                WORKSPACE,
                BACKEND_RESOURCE_FETCH_PERMISSION,
                "POST",
                PATH,
                &request_body_digest(b"{}"),
                &|| NOW,
            ),
        )
        .await;
        // Release before assertions so a failed contract never strands a worker.
        release.send(()).unwrap();
        blocker.await.unwrap().unwrap();
        queued.await.unwrap();
        let error = outcome
            .expect("capacity rejection must not wait for queued SQL")
            .unwrap_err();
        assert_eq!(error.reason, "runtime_trust_authority_unavailable");
        assert!(error.is_availability_error());
    }
}
