use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use config_source::ConfigSchemaContribution;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use ssh_key::{Algorithm, HashAlg, PrivateKey, PublicKey};
use workspace_api::{
    CreateRepositorySshCredentialRequest, DeleteRepositorySshCredentialRequest,
    DeleteRepositorySshHostTrustRequest, PutRepositorySshHostTrustRequest, RepositoryAccessMode,
    RepositoryAccessProjection, RepositorySshAccessBinding, RepositorySshCredential,
    RepositorySshHostTrust, RotateRepositorySshCredentialRequest,
};

use crate::config_source::{
    EvaluatedConfigCandidate, WorkspaceConfigSchemaProvider, WorkspaceConfigState,
    evaluate_workspace_config_state,
};
use crate::store::{ControlPlaneStore, SqliteWorkspaceStore};
use crate::{Error, Result};

const REPOSITORY_ACCESS_SCHEMA_SOURCE: &str = r#"{
    repository_access = {
        ...{
            ssh = {
                credential_id = String;
                host_trust_id = String;
                access = String;
            };
        }
    } default {};
}"#;
const MAX_SECRET_BYTES: usize = 256 * 1024;
const MAX_NAME_BYTES: usize = 200;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;

#[derive(Debug, Default)]
pub struct RepositoryAccessConfigSchemaProvider;

impl WorkspaceConfigSchemaProvider for RepositoryAccessConfigSchemaProvider {
    fn contribution(&self) -> Result<ConfigSchemaContribution> {
        ConfigSchemaContribution::new(
            "builtin:repository-access",
            "repository_access",
            "1",
            REPOSITORY_ACCESS_SCHEMA_SOURCE,
        )
        .map_err(|error| Error::Config(error.to_string()))
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualWorkspaceConfig {
    #[serde(default)]
    repository_access: BTreeMap<String, VirtualRepositoryAccess>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualRepositoryAccess {
    ssh: VirtualRepositorySshAccess,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VirtualRepositorySshAccess {
    credential_id: String,
    host_trust_id: String,
    access: RepositoryAccessMode,
}

pub fn project_repository_access_candidate(
    store: &dyn ControlPlaneStore,
    secrets: &RepositorySecretService,
    workspace_id: &str,
    candidate: &EvaluatedConfigCandidate,
) -> Result<RepositoryAccessProjection> {
    project_repository_access_evaluation(
        store,
        secrets,
        workspace_id,
        candidate.base_revision + 1,
        &candidate.evaluation.projection_digest,
        &candidate.evaluation,
    )
}

pub fn project_repository_access_state(
    store: &dyn ControlPlaneStore,
    secrets: &RepositorySecretService,
    workspace_id: &str,
    state: &WorkspaceConfigState,
) -> Result<RepositoryAccessProjection> {
    let has_schema = state
        .contract
        .schema_bundle
        .contributions
        .iter()
        .any(|entry| entry.provider_id == "builtin:repository-access");
    if !has_schema {
        return Ok(RepositoryAccessProjection {
            workspace_id: workspace_id.to_string(),
            config_revision: state.snapshot.revision,
            projection_digest: state.projection_digest.clone(),
            bindings: Vec::new(),
        });
    }
    let evaluation = evaluate_workspace_config_state(state, state.contract.schema_bundle.clone())?;
    if evaluation.projection_digest != state.projection_digest {
        return Err(Error::RegistryInconsistency(format!(
            "Repository access projection digest mismatch for Workspace {workspace_id}"
        )));
    }
    project_repository_access_evaluation(
        store,
        secrets,
        workspace_id,
        state.snapshot.revision,
        &state.projection_digest,
        &evaluation,
    )
}

fn project_repository_access_evaluation(
    store: &dyn ControlPlaneStore,
    secrets: &RepositorySecretService,
    workspace_id: &str,
    config_revision: u64,
    projection_digest: &str,
    evaluation: &config_source::EvaluationResult,
) -> Result<RepositoryAccessProjection> {
    let projection = evaluation.projections.first().ok_or_else(|| {
        Error::InvalidInput("Workspace config produced no active projection".to_string())
    })?;
    let config: VirtualWorkspaceConfig = serde_json::from_value(projection.data_json.clone())
        .map_err(|error| {
            Error::InvalidInput(format!("invalid Repository access config: {error}"))
        })?;
    let mut bindings = Vec::with_capacity(config.repository_access.len());
    for (repository_id, access) in config.repository_access {
        validate_identifier("repository_id", &repository_id)?;
        validate_identifier("credential_id", &access.ssh.credential_id)?;
        validate_identifier("host_trust_id", &access.ssh.host_trust_id)?;
        let repository = store
            .get_repository(workspace_id, &repository_id)?
            .ok_or_else(|| Error::InvalidInput(format!("unknown Repository `{repository_id}`")))?;
        if repository.source.kind != workspace_api::RepositorySourceKind::Ssh {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_id}` is not an ssh:// Repository"
            )));
        }
        let credential = secrets
            .get_credential(workspace_id, &access.ssh.credential_id, &[])?
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "unknown Repository SSH credential `{}`",
                    access.ssh.credential_id
                ))
            })?;
        if credential.status != "active" {
            return Err(Error::InvalidInput(format!(
                "Repository SSH credential `{}` is not active",
                access.ssh.credential_id
            )));
        }
        let host_trust = secrets
            .get_host_trust(workspace_id, &access.ssh.host_trust_id, &[])?
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "unknown Repository SSH host trust `{}`",
                    access.ssh.host_trust_id
                ))
            })?;
        let uri = url::Url::parse(&repository.source.uri).map_err(|_| {
            Error::InvalidInput(format!(
                "Repository `{repository_id}` has an invalid SSH URI"
            ))
        })?;
        if uri.scheme() != "ssh" || uri.username().is_empty() || uri.password().is_some() {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_id}` must use ssh://user@host[:port]/path without credentials"
            )));
        }
        let hostname = uri.host_str().ok_or_else(|| {
            Error::InvalidInput(format!(
                "Repository `{repository_id}` SSH URI has no hostname"
            ))
        })?;
        let port = uri.port().unwrap_or(22);
        if hostname != host_trust.hostname || port != host_trust.port {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_id}` SSH host does not match host trust `{}`",
                access.ssh.host_trust_id
            )));
        }
        bindings.push(RepositorySshAccessBinding {
            repository_id,
            credential_id: access.ssh.credential_id,
            host_trust_id: access.ssh.host_trust_id,
            access: access.ssh.access,
        });
    }
    bindings.sort_by(|left, right| left.repository_id.cmp(&right.repository_id));
    Ok(RepositoryAccessProjection {
        workspace_id: workspace_id.to_string(),
        config_revision,
        projection_digest: projection_digest.to_string(),
        bindings,
    })
}

#[derive(Clone)]
pub struct RepositorySecretService {
    store: Arc<SqliteWorkspaceStore>,
    master_key: Option<Arc<[u8; MASTER_KEY_BYTES]>>,
}

impl RepositorySecretService {
    pub fn open(store: Arc<SqliteWorkspaceStore>, database_path: &Path) -> Result<Self> {
        let key_path = master_key_path(database_path)?;
        let key = load_or_create_master_key(&key_path)?;
        Ok(Self {
            store,
            master_key: Some(Arc::new(key)),
        })
    }

    pub fn create_credential(
        &self,
        workspace_id: &str,
        request: CreateRepositorySshCredentialRequest,
        actor_account_id: &str,
    ) -> Result<RepositorySshCredential> {
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let credential_id = validate_identifier("credential_id", &request.credential_id)?;
        let name = normalize_name(&request.name)?;
        let parsed = parse_private_key(&request.private_key, request.passphrase.as_deref())?;
        let fingerprint = credential_fingerprint(
            "create",
            &credential_id,
            &name,
            0,
            &request.private_key,
            request.passphrase.as_deref(),
        );
        let private_secret = self.seal(
            workspace_id,
            &credential_id,
            1,
            "private_key",
            request.private_key.as_bytes(),
        )?;
        let passphrase_secret = request
            .passphrase
            .as_deref()
            .map(|value| {
                self.seal(
                    workspace_id,
                    &credential_id,
                    1,
                    "passphrase",
                    value.as_bytes(),
                )
            })
            .transpose()?;
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(replayed) = replay_credential_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                &credential_id,
            )? {
                tx.commit()?;
                return Ok(replayed);
            }
            ensure_workspace_exists(&tx, workspace_id)?;
            if credential_row_exists(&tx, workspace_id, &credential_id)? {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "Repository SSH credential `{credential_id}` already exists"
                )));
            }
            tx.execute(
                r#"INSERT INTO repository_ssh_credentials (
                    workspace_id, credential_id, name, public_key_algorithm,
                    public_key_fingerprint, current_revision, status, created_at, rotated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 1, 'active', ?6, NULL)"#,
                params![
                    workspace_id,
                    credential_id,
                    name,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now
                ],
            )?;
            tx.execute(
                r#"INSERT INTO repository_ssh_credential_revisions (
                    workspace_id, credential_id, revision, public_key_algorithm,
                    public_key_fingerprint, created_at
                ) VALUES (?1, ?2, 1, ?3, ?4, ?5)"#,
                params![
                    workspace_id,
                    credential_id,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now
                ],
            )?;
            insert_secret(
                &tx,
                workspace_id,
                &credential_id,
                1,
                "private_key",
                &private_secret,
                &now,
            )?;
            if let Some(secret) = passphrase_secret.as_ref() {
                insert_secret(
                    &tx,
                    workspace_id,
                    &credential_id,
                    1,
                    "passphrase",
                    secret,
                    &now,
                )?;
            }
            insert_audit(
                &tx,
                workspace_id,
                "credential_created",
                &credential_id,
                1,
                actor_account_id,
                &now,
            )?;
            insert_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                "credential",
                &credential_id,
                1,
                &now,
            )?;
            let record = read_credential(&tx, workspace_id, &credential_id)?.ok_or_else(|| {
                Error::RegistryInconsistency("created credential could not be reloaded".to_string())
            })?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn rotate_credential(
        &self,
        workspace_id: &str,
        credential_id: &str,
        request: RotateRepositorySshCredentialRequest,
        actor_account_id: &str,
    ) -> Result<RepositorySshCredential> {
        let credential_id = validate_identifier("credential_id", credential_id)?;
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let parsed = parse_private_key(&request.private_key, request.passphrase.as_deref())?;
        let next_revision = request
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::InvalidInput("credential revision overflow".to_string()))?;
        let fingerprint = credential_fingerprint(
            "rotate",
            &credential_id,
            "",
            request.expected_revision,
            &request.private_key,
            request.passphrase.as_deref(),
        );
        let private_secret = self.seal(
            workspace_id,
            &credential_id,
            next_revision,
            "private_key",
            request.private_key.as_bytes(),
        )?;
        let passphrase_secret = request
            .passphrase
            .as_deref()
            .map(|value| {
                self.seal(
                    workspace_id,
                    &credential_id,
                    next_revision,
                    "passphrase",
                    value.as_bytes(),
                )
            })
            .transpose()?;
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(replayed) = replay_credential_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                &credential_id,
            )? {
                tx.commit()?;
                return Ok(replayed);
            }
            let current = read_credential(&tx, workspace_id, &credential_id)?
                .ok_or_else(|| Error::InvalidRecordId(credential_id.clone()))?;
            if current.current_revision != request.expected_revision || current.status != "active" {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` revision/status changed"
                )));
            }
            tx.execute(
                r#"INSERT INTO repository_ssh_credential_revisions (
                    workspace_id, credential_id, revision, public_key_algorithm,
                    public_key_fingerprint, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    workspace_id,
                    credential_id,
                    next_revision,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now
                ],
            )?;
            insert_secret(
                &tx,
                workspace_id,
                &credential_id,
                next_revision,
                "private_key",
                &private_secret,
                &now,
            )?;
            if let Some(secret) = passphrase_secret.as_ref() {
                insert_secret(
                    &tx,
                    workspace_id,
                    &credential_id,
                    next_revision,
                    "passphrase",
                    secret,
                    &now,
                )?;
            }
            let updated = tx.execute(
                r#"UPDATE repository_ssh_credentials
                   SET public_key_algorithm = ?4, public_key_fingerprint = ?5,
                       current_revision = ?3, rotated_at = ?6
                   WHERE workspace_id = ?1 AND credential_id = ?2
                     AND current_revision = ?7 AND status = 'active'"#,
                params![
                    workspace_id,
                    credential_id,
                    next_revision,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now,
                    request.expected_revision
                ],
            )?;
            if updated != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` revision changed"
                )));
            }
            insert_audit(
                &tx,
                workspace_id,
                "credential_rotated",
                &credential_id,
                next_revision,
                actor_account_id,
                &now,
            )?;
            insert_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                "credential",
                &credential_id,
                next_revision,
                &now,
            )?;
            let record = read_credential(&tx, workspace_id, &credential_id)?.ok_or_else(|| {
                Error::RegistryInconsistency("rotated credential could not be reloaded".to_string())
            })?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn delete_credential(
        &self,
        workspace_id: &str,
        credential_id: &str,
        request: DeleteRepositorySshCredentialRequest,
        actor_account_id: &str,
        projection: &RepositoryAccessProjection,
    ) -> Result<()> {
        let credential_id = validate_identifier("credential_id", credential_id)?;
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let references = credential_references(projection, &credential_id);
        if !references.is_empty() {
            return Err(Error::WorkspaceConfigConflict(format!(
                "credential `{credential_id}` is referenced by active Workspace config"
            )));
        }
        let fingerprint = simple_operation_fingerprint(
            "delete_credential",
            &credential_id,
            request.expected_revision,
        );
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if replay_deleted_operation(&tx, workspace_id, &operation_id, &fingerprint, "credential", &credential_id)? {
                tx.commit()?;
                return Ok(());
            }
            let current = read_credential(&tx, workspace_id, &credential_id)?
                .ok_or_else(|| Error::InvalidRecordId(credential_id.clone()))?;
            if current.current_revision != request.expected_revision {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` revision changed"
                )));
            }
            insert_audit(&tx, workspace_id, "credential_deleted", &credential_id, current.current_revision, actor_account_id, &now)?;
            let deleted = tx.execute(
                "DELETE FROM repository_ssh_credentials WHERE workspace_id = ?1 AND credential_id = ?2 AND current_revision = ?3",
                params![workspace_id, credential_id, request.expected_revision],
            )?;
            if deleted != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` revision changed"
                )));
            }
            insert_operation(&tx, workspace_id, &operation_id, &fingerprint, "credential", &credential_id, request.expected_revision, &now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn list_credentials(
        &self,
        workspace_id: &str,
        projection: &RepositoryAccessProjection,
    ) -> Result<Vec<RepositorySshCredential>> {
        self.store.with_conn(|conn| {
            let mut statement = conn.prepare(
                r#"SELECT workspace_id, credential_id, name, public_key_algorithm,
                          public_key_fingerprint, current_revision, status, created_at, rotated_at
                   FROM repository_ssh_credentials WHERE workspace_id = ?1
                   ORDER BY credential_id"#,
            )?;
            statement
                .query_map([workspace_id], read_credential_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .map(|mut record| {
                    record.referenced_repositories =
                        credential_references(projection, &record.credential_id);
                    Ok(record)
                })
                .collect()
        })
    }

    pub fn get_credential(
        &self,
        workspace_id: &str,
        credential_id: &str,
        references: &[String],
    ) -> Result<Option<RepositorySshCredential>> {
        self.store.with_conn(|conn| {
            let mut record = read_credential(conn, workspace_id, credential_id)?;
            if let Some(record) = record.as_mut() {
                record.referenced_repositories = references.to_vec();
            }
            Ok(record)
        })
    }

    pub fn put_host_trust(
        &self,
        workspace_id: &str,
        request: PutRepositorySshHostTrustRequest,
        actor_account_id: &str,
    ) -> Result<RepositorySshHostTrust> {
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let host_trust_id = validate_identifier("host_trust_id", &request.host_trust_id)?;
        let hostname = normalize_hostname(&request.hostname)?;
        if request.port == 0 {
            return Err(Error::InvalidInput(
                "SSH host trust port must be non-zero".to_string(),
            ));
        }
        let parsed = parse_host_key(&request.host_key)?;
        let fingerprint = host_operation_fingerprint(&request, &hostname, &parsed.fingerprint);
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(replayed) = replay_host_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                &host_trust_id,
            )? {
                tx.commit()?;
                return Ok(replayed);
            }
            ensure_workspace_exists(&tx, workspace_id)?;
            let current = read_host_trust(&tx, workspace_id, &host_trust_id)?;
            let next_revision = match (current.as_ref(), request.expected_revision) {
                (None, None) => 1,
                (Some(current), Some(expected)) if current.current_revision == expected => {
                    expected.checked_add(1).ok_or_else(|| {
                        Error::InvalidInput("host trust revision overflow".to_string())
                    })?
                }
                (None, Some(_)) | (Some(_), None) => {
                    return Err(Error::WorkspaceConfigConflict(format!(
                        "host trust `{host_trust_id}` create/update precondition failed"
                    )));
                }
                (Some(_), Some(_)) => {
                    return Err(Error::WorkspaceConfigConflict(format!(
                        "host trust `{host_trust_id}` revision changed"
                    )));
                }
            };
            if current.is_none() {
                tx.execute(
                    r#"INSERT INTO repository_ssh_host_trusts (
                        workspace_id, host_trust_id, hostname, port, key_algorithm,
                        host_key, fingerprint, current_revision, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)"#,
                    params![
                        workspace_id,
                        host_trust_id,
                        hostname,
                        request.port,
                        parsed.algorithm,
                        parsed.canonical_key,
                        parsed.fingerprint,
                        next_revision,
                        now
                    ],
                )?;
            } else {
                tx.execute(
                    r#"UPDATE repository_ssh_host_trusts
                       SET hostname = ?4, port = ?5, key_algorithm = ?6,
                           host_key = ?7, fingerprint = ?8,
                           current_revision = ?3, updated_at = ?9
                       WHERE workspace_id = ?1 AND host_trust_id = ?2
                         AND current_revision = ?10"#,
                    params![
                        workspace_id,
                        host_trust_id,
                        next_revision,
                        hostname,
                        request.port,
                        parsed.algorithm,
                        parsed.canonical_key,
                        parsed.fingerprint,
                        now,
                        request.expected_revision
                    ],
                )?;
            }
            tx.execute(
                r#"INSERT INTO repository_ssh_host_trust_revisions (
                    workspace_id, host_trust_id, revision, hostname, port,
                    key_algorithm, host_key, fingerprint, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    workspace_id,
                    host_trust_id,
                    next_revision,
                    hostname,
                    request.port,
                    parsed.algorithm,
                    parsed.canonical_key,
                    parsed.fingerprint,
                    now
                ],
            )?;
            let event = if next_revision == 1 {
                "host_trust_created"
            } else {
                "host_trust_rotated"
            };
            insert_audit(
                &tx,
                workspace_id,
                event,
                &host_trust_id,
                next_revision,
                actor_account_id,
                &now,
            )?;
            insert_operation(
                &tx,
                workspace_id,
                &operation_id,
                &fingerprint,
                "host_trust",
                &host_trust_id,
                next_revision,
                &now,
            )?;
            let record = read_host_trust(&tx, workspace_id, &host_trust_id)?.ok_or_else(|| {
                Error::RegistryInconsistency("host trust could not be reloaded".to_string())
            })?;
            tx.commit()?;
            Ok(record)
        })
    }

    pub fn delete_host_trust(
        &self,
        workspace_id: &str,
        host_trust_id: &str,
        request: DeleteRepositorySshHostTrustRequest,
        actor_account_id: &str,
        projection: &RepositoryAccessProjection,
    ) -> Result<()> {
        let host_trust_id = validate_identifier("host_trust_id", host_trust_id)?;
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        if !host_trust_references(projection, &host_trust_id).is_empty() {
            return Err(Error::WorkspaceConfigConflict(format!(
                "host trust `{host_trust_id}` is referenced by active Workspace config"
            )));
        }
        let fingerprint = simple_operation_fingerprint(
            "delete_host_trust",
            &host_trust_id,
            request.expected_revision,
        );
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if replay_deleted_operation(&tx, workspace_id, &operation_id, &fingerprint, "host_trust", &host_trust_id)? {
                tx.commit()?;
                return Ok(());
            }
            let current = read_host_trust(&tx, workspace_id, &host_trust_id)?
                .ok_or_else(|| Error::InvalidRecordId(host_trust_id.clone()))?;
            if current.current_revision != request.expected_revision {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "host trust `{host_trust_id}` revision changed"
                )));
            }
            insert_audit(&tx, workspace_id, "host_trust_deleted", &host_trust_id, current.current_revision, actor_account_id, &now)?;
            let deleted = tx.execute(
                "DELETE FROM repository_ssh_host_trusts WHERE workspace_id = ?1 AND host_trust_id = ?2 AND current_revision = ?3",
                params![workspace_id, host_trust_id, request.expected_revision],
            )?;
            if deleted != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "host trust `{host_trust_id}` revision changed"
                )));
            }
            insert_operation(&tx, workspace_id, &operation_id, &fingerprint, "host_trust", &host_trust_id, request.expected_revision, &now)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn list_host_trusts(
        &self,
        workspace_id: &str,
        projection: &RepositoryAccessProjection,
    ) -> Result<Vec<RepositorySshHostTrust>> {
        self.store.with_conn(|conn| {
            let mut statement = conn.prepare(
                r#"SELECT workspace_id, host_trust_id, hostname, port, key_algorithm,
                          host_key, fingerprint, current_revision, created_at, updated_at
                   FROM repository_ssh_host_trusts WHERE workspace_id = ?1
                   ORDER BY host_trust_id"#,
            )?;
            statement
                .query_map([workspace_id], read_host_trust_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?
                .into_iter()
                .map(|mut record| {
                    record.referenced_repositories =
                        host_trust_references(projection, &record.host_trust_id);
                    Ok(record)
                })
                .collect()
        })
    }

    pub fn get_host_trust(
        &self,
        workspace_id: &str,
        host_trust_id: &str,
        references: &[String],
    ) -> Result<Option<RepositorySshHostTrust>> {
        self.store.with_conn(|conn| {
            let mut record = read_host_trust(conn, workspace_id, host_trust_id)?;
            if let Some(record) = record.as_mut() {
                record.referenced_repositories = references.to_vec();
            }
            Ok(record)
        })
    }

    fn seal(
        &self,
        workspace_id: &str,
        credential_id: &str,
        revision: u64,
        purpose: &str,
        plaintext: &[u8],
    ) -> Result<SealedSecret> {
        if plaintext.is_empty() || plaintext.len() > MAX_SECRET_BYTES {
            return Err(Error::InvalidInput(
                "Repository SSH secret input is empty or too large".to_string(),
            ));
        }
        let key = self.master_key.as_ref().ok_or_else(|| {
            Error::Store("Repository secret encryption authority is unavailable".to_string())
        })?;
        let unbound = UnboundKey::new(&AES_256_GCM, key.as_slice())
            .map_err(|_| Error::Store("Repository secret encryption key is invalid".to_string()))?;
        let key = LessSafeKey::new(unbound);
        let mut nonce = [0u8; NONCE_BYTES];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| Error::Store("Repository secret nonce generation failed".to_string()))?;
        let mut ciphertext = plaintext.to_vec();
        let aad = secret_aad(workspace_id, credential_id, revision, purpose);
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_bytes()),
            &mut ciphertext,
        )
        .map_err(|_| Error::Store("Repository secret encryption failed".to_string()))?;
        Ok(SealedSecret { nonce, ciphertext })
    }
}

#[derive(Debug)]
struct ParsedKey {
    algorithm: String,
    fingerprint: String,
}

fn parse_private_key(private_key: &str, passphrase: Option<&str>) -> Result<ParsedKey> {
    if private_key.is_empty() || private_key.len() > MAX_SECRET_BYTES {
        return Err(Error::InvalidInput(
            "Repository SSH private key is empty or too large".to_string(),
        ));
    }
    let key = PrivateKey::from_openssh(private_key)
        .map_err(|_| Error::InvalidInput("Repository SSH private key is malformed".to_string()))?;
    let key = if key.is_encrypted() {
        let passphrase = passphrase.ok_or_else(|| {
            Error::InvalidInput(
                "encrypted Repository SSH private key requires a passphrase".to_string(),
            )
        })?;
        key.decrypt(passphrase).map_err(|_| {
            Error::InvalidInput("Repository SSH private key passphrase is invalid".to_string())
        })?
    } else {
        if passphrase.is_some() {
            return Err(Error::InvalidInput(
                "passphrase was supplied for an unencrypted Repository SSH private key".to_string(),
            ));
        }
        key
    };
    if key.algorithm() != Algorithm::Ed25519 {
        return Err(Error::InvalidInput(
            "only ssh-ed25519 Repository private keys are supported".to_string(),
        ));
    }
    let public_key = key.public_key();
    Ok(ParsedKey {
        algorithm: public_key.algorithm().to_string(),
        fingerprint: public_key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

struct ParsedHostKey {
    algorithm: String,
    canonical_key: String,
    fingerprint: String,
}

fn parse_host_key(host_key: &str) -> Result<ParsedHostKey> {
    if host_key.is_empty() || host_key.len() > MAX_SECRET_BYTES {
        return Err(Error::InvalidInput(
            "SSH host key is empty or too large".to_string(),
        ));
    }
    let key = PublicKey::from_openssh(host_key)
        .map_err(|_| Error::InvalidInput("SSH host key is malformed".to_string()))?;
    if key.algorithm() != Algorithm::Ed25519 {
        return Err(Error::InvalidInput(
            "only ssh-ed25519 host keys are supported".to_string(),
        ));
    }
    Ok(ParsedHostKey {
        algorithm: key.algorithm().to_string(),
        canonical_key: key
            .to_openssh()
            .map_err(|_| Error::InvalidInput("SSH host key cannot be encoded".to_string()))?,
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    })
}

#[derive(Debug)]
struct SealedSecret {
    nonce: [u8; NONCE_BYTES],
    ciphertext: Vec<u8>,
}

fn insert_secret(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    credential_id: &str,
    revision: u64,
    purpose: &str,
    secret: &SealedSecret,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO server_secret_versions (
            workspace_id, secret_id, revision, purpose, encryption_algorithm,
            nonce, ciphertext, created_at
        ) VALUES (?1, ?2, ?3, ?4, 'aes-256-gcm-v1', ?5, ?6, ?7)"#,
        params![
            workspace_id,
            credential_id,
            revision,
            purpose,
            secret.nonce.as_slice(),
            secret.ciphertext,
            created_at
        ],
    )?;
    Ok(())
}

fn replay_credential_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    credential_id: &str,
) -> Result<Option<RepositorySshCredential>> {
    let operation = read_operation(tx, workspace_id, operation_id)?;
    let Some((stored_fingerprint, kind, resource_id, _)) = operation else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != "credential" || resource_id != credential_id {
        return Err(Error::WorkspaceConfigConflict(
            "Repository secret operation id was reused with different input".to_string(),
        ));
    }
    read_credential(tx, workspace_id, credential_id)?
        .map(Some)
        .ok_or_else(|| {
            Error::RegistryInconsistency(
                "Repository secret operation replay points to a missing credential".to_string(),
            )
        })
}

fn replay_host_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    host_trust_id: &str,
) -> Result<Option<RepositorySshHostTrust>> {
    let operation = read_operation(tx, workspace_id, operation_id)?;
    let Some((stored_fingerprint, kind, resource_id, _)) = operation else {
        return Ok(None);
    };
    if stored_fingerprint != fingerprint || kind != "host_trust" || resource_id != host_trust_id {
        return Err(Error::WorkspaceConfigConflict(
            "Repository host trust operation id was reused with different input".to_string(),
        ));
    }
    read_host_trust(tx, workspace_id, host_trust_id)?
        .map(Some)
        .ok_or_else(|| {
            Error::RegistryInconsistency(
                "Repository host trust operation replay points to a missing record".to_string(),
            )
        })
}

fn replay_deleted_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    kind: &str,
    resource_id: &str,
) -> Result<bool> {
    let Some((stored_fingerprint, stored_kind, stored_resource, _)) =
        read_operation(tx, workspace_id, operation_id)?
    else {
        return Ok(false);
    };
    if stored_fingerprint != fingerprint || stored_kind != kind || stored_resource != resource_id {
        return Err(Error::WorkspaceConfigConflict(
            "Repository secret operation id was reused with different input".to_string(),
        ));
    }
    Ok(true)
}

fn read_operation(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<(String, String, String, u64)>> {
    conn.query_row(
        r#"SELECT request_fingerprint, resource_kind, resource_id, result_revision
           FROM repository_secret_operations
           WHERE workspace_id = ?1 AND operation_id = ?2"#,
        params![workspace_id, operation_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get::<_, i64>(3)? as u64,
            ))
        },
    )
    .optional()
    .map_err(Into::into)
}

fn insert_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    kind: &str,
    resource_id: &str,
    revision: u64,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO repository_secret_operations (
            workspace_id, operation_id, request_fingerprint, resource_kind,
            resource_id, result_revision, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            workspace_id,
            operation_id,
            fingerprint,
            kind,
            resource_id,
            revision,
            created_at
        ],
    )?;
    Ok(())
}

fn insert_audit(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    kind: &str,
    resource_id: &str,
    revision: u64,
    actor_account_id: &str,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO repository_secret_audit_events (
            workspace_id, event_id, kind, resource_id, revision,
            actor_account_id, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            workspace_id,
            format!("repo-secret-audit-{}", uuid::Uuid::now_v7()),
            kind,
            resource_id,
            revision,
            actor_account_id,
            created_at
        ],
    )?;
    Ok(())
}

fn ensure_workspace_exists(conn: &rusqlite::Connection, workspace_id: &str) -> Result<()> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id = ?1)",
        [workspace_id],
        |row| row.get(0),
    )?;
    if !exists {
        return Err(Error::WorkspaceIdMismatch);
    }
    Ok(())
}

fn credential_row_exists(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    credential_id: &str,
) -> Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM repository_ssh_credentials WHERE workspace_id = ?1 AND credential_id = ?2)",
        params![workspace_id, credential_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn read_credential(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    credential_id: &str,
) -> Result<Option<RepositorySshCredential>> {
    conn.query_row(
        r#"SELECT workspace_id, credential_id, name, public_key_algorithm,
                  public_key_fingerprint, current_revision, status, created_at, rotated_at
           FROM repository_ssh_credentials
           WHERE workspace_id = ?1 AND credential_id = ?2"#,
        params![workspace_id, credential_id],
        read_credential_row,
    )
    .optional()
    .map_err(Into::into)
}

fn read_credential_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositorySshCredential> {
    Ok(RepositorySshCredential {
        workspace_id: row.get(0)?,
        credential_id: row.get(1)?,
        name: row.get(2)?,
        public_key_algorithm: row.get(3)?,
        public_key_fingerprint: row.get(4)?,
        current_revision: row.get::<_, i64>(5)? as u64,
        status: row.get(6)?,
        created_at: row.get(7)?,
        rotated_at: row.get(8)?,
        referenced_repositories: Vec::new(),
    })
}

fn read_host_trust(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    host_trust_id: &str,
) -> Result<Option<RepositorySshHostTrust>> {
    conn.query_row(
        r#"SELECT workspace_id, host_trust_id, hostname, port, key_algorithm,
                  host_key, fingerprint, current_revision, created_at, updated_at
           FROM repository_ssh_host_trusts
           WHERE workspace_id = ?1 AND host_trust_id = ?2"#,
        params![workspace_id, host_trust_id],
        read_host_trust_row,
    )
    .optional()
    .map_err(Into::into)
}

fn read_host_trust_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RepositorySshHostTrust> {
    Ok(RepositorySshHostTrust {
        workspace_id: row.get(0)?,
        host_trust_id: row.get(1)?,
        hostname: row.get(2)?,
        port: row.get::<_, i64>(3)? as u16,
        key_algorithm: row.get(4)?,
        host_key: row.get(5)?,
        fingerprint: row.get(6)?,
        current_revision: row.get::<_, i64>(7)? as u64,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        referenced_repositories: Vec::new(),
    })
}

fn credential_references(
    projection: &RepositoryAccessProjection,
    credential_id: &str,
) -> Vec<String> {
    projection
        .bindings
        .iter()
        .filter(|binding| binding.credential_id == credential_id)
        .map(|binding| binding.repository_id.clone())
        .collect()
}

fn host_trust_references(
    projection: &RepositoryAccessProjection,
    host_trust_id: &str,
) -> Vec<String> {
    projection
        .bindings
        .iter()
        .filter(|binding| binding.host_trust_id == host_trust_id)
        .map(|binding| binding.repository_id.clone())
        .collect()
}

fn master_key_path(database_path: &Path) -> Result<PathBuf> {
    let parent = database_path.parent().ok_or_else(|| {
        Error::Config("Server database has no parent for secret master key".to_string())
    })?;
    Ok(parent.join("repository-secrets.master-key"))
}

fn load_or_create_master_key(path: &Path) -> Result<[u8; MASTER_KEY_BYTES]> {
    match std::fs::read(path) {
        Ok(bytes) => return master_key_from_bytes(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(Error::Store(
                "Repository secret master key could not be read".to_string(),
            ));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| {
            Error::Store("Repository secret master key directory could not be created".to_string())
        })?;
    }
    let mut key = [0u8; MASTER_KEY_BYTES];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| Error::Store("Repository secret master key generation failed".to_string()))?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&key)
                .and_then(|_| file.sync_all())
                .map_err(|_| {
                    Error::Store("Repository secret master key could not be persisted".to_string())
                })?;
            Ok(key)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let bytes = std::fs::read(path).map_err(|_| {
                Error::Store("Repository secret master key race could not be resolved".to_string())
            })?;
            master_key_from_bytes(&bytes)
        }
        Err(_) => Err(Error::Store(
            "Repository secret master key could not be created".to_string(),
        )),
    }
}

fn master_key_from_bytes(bytes: &[u8]) -> Result<[u8; MASTER_KEY_BYTES]> {
    bytes
        .try_into()
        .map_err(|_| Error::Store("Repository secret master key has an invalid length".to_string()))
}

fn secret_aad(workspace_id: &str, credential_id: &str, revision: u64, purpose: &str) -> String {
    format!("yoi/repository-secret/v1/{workspace_id}/{credential_id}/{revision}/{purpose}")
}

fn validate_identifier(field: &str, value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > MAX_IDENTIFIER_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(Error::InvalidInput(format!(
            "{field} must be 1-{MAX_IDENTIFIER_BYTES} ASCII identifier characters"
        )));
    }
    Ok(value.to_string())
}

fn normalize_name(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_NAME_BYTES || value.chars().any(char::is_control) {
        return Err(Error::InvalidInput(format!(
            "credential name must be 1-{MAX_NAME_BYTES} non-control bytes"
        )));
    }
    Ok(value.to_string())
}

fn normalize_hostname(value: &str) -> Result<String> {
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 253
        || value.starts_with('-')
        || value.ends_with('-')
        || value.split('.').any(|label| {
            label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(Error::InvalidInput(
            "SSH host trust hostname is invalid".to_string(),
        ));
    }
    Ok(value)
}

fn credential_fingerprint(
    kind: &str,
    credential_id: &str,
    name: &str,
    expected_revision: u64,
    private_key: &str,
    passphrase: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi repository credential operation v1");
    hasher.update(kind.as_bytes());
    hasher.update(credential_id.as_bytes());
    hasher.update(name.as_bytes());
    hasher.update(expected_revision.to_be_bytes());
    hasher.update(Sha256::digest(private_key.as_bytes()));
    hasher.update(
        passphrase
            .map(|value| Sha256::digest(value.as_bytes()).to_vec())
            .unwrap_or_default(),
    );
    format!("sha256:{}", encode_hex(&hasher.finalize()))
}

fn host_operation_fingerprint(
    request: &PutRepositorySshHostTrustRequest,
    hostname: &str,
    key_fingerprint: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi repository host trust operation v1");
    hasher.update(request.host_trust_id.as_bytes());
    hasher.update(hostname.as_bytes());
    hasher.update(request.port.to_be_bytes());
    hasher.update(key_fingerprint.as_bytes());
    hasher.update(request.expected_revision.unwrap_or(0).to_be_bytes());
    format!("sha256:{}", encode_hex(&hasher.finalize()))
}

fn simple_operation_fingerprint(kind: &str, resource_id: &str, revision: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"yoi repository secret simple operation v1");
    hasher.update(kind.as_bytes());
    hasher.update(resource_id.as_bytes());
    hasher.update(revision.to_be_bytes());
    format!("sha256:{}", encode_hex(&hasher.finalize()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn credential_reference_set(projection: &RepositoryAccessProjection) -> BTreeSet<String> {
    projection
        .bindings
        .iter()
        .map(|binding| binding.credential_id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{RepositoryRecord, WorkspaceRecord};
    use config_source::{
        ConfigContentType, ConfigEntry, ConfigTreeSnapshot, DEFAULT_IMPORT_POLICY_VERSION,
        DEFAULT_SCHEMA_VERSION, SnapshotEnvironment, ToolchainContract, VirtualPath,
        WorkspaceConfigSchemaBundle,
    };
    use ssh_key::private::Ed25519Keypair;
    use workspace_api::{RepositoryObservedStatus, RepositorySource, RepositorySourceKind};

    fn test_private_key(seed: u8) -> (String, String) {
        let key = PrivateKey::from(Ed25519Keypair::from_seed(&[seed; 32]));
        let private = key.to_openssh(ssh_key::LineEnding::LF).unwrap().to_string();
        let public = key.public_key().to_openssh().unwrap();
        (private, public)
    }

    fn test_service() -> (
        tempfile::TempDir,
        Arc<SqliteWorkspaceStore>,
        RepositorySecretService,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("server.db");
        let store = Arc::new(SqliteWorkspaceStore::open(&database).unwrap());
        let timestamp = now();
        for workspace_id in ["workspace-a", "workspace-b"] {
            futures::executor::block_on(store.upsert_workspace(&WorkspaceRecord {
                workspace_id: workspace_id.to_string(),
                owner_account_id: None,
                display_name: workspace_id.to_string(),
                state: "active".to_string(),
                created_at: timestamp.clone(),
                updated_at: timestamp.clone(),
            }))
            .unwrap();
        }
        let service = RepositorySecretService::open(store.clone(), &database).unwrap();
        (dir, store, service)
    }

    #[test]
    fn schema_accepts_repository_keyed_ssh_access() {
        let contribution = RepositoryAccessConfigSchemaProvider.contribution().unwrap();
        assert_eq!(contribution.namespace, "repository_access");
        assert!(contribution.source.contains("...{"));
        assert!(!contribution.source.contains("private_key"));
        assert!(!contribution.source.contains("secret_ref"));
    }

    #[test]
    fn master_key_is_external_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("server.db");
        let first = load_or_create_master_key(&master_key_path(&database).unwrap()).unwrap();
        let second = load_or_create_master_key(&master_key_path(&database).unwrap()).unwrap();
        assert_eq!(first, second);
        assert!(!database.exists());
    }

    #[test]
    fn invalid_private_key_error_never_echoes_input() {
        let secret = "DO NOT ECHO THIS PRIVATE KEY";
        let error = parse_private_key(secret, None).unwrap_err().to_string();
        assert!(!error.contains(secret));
    }

    #[test]
    fn credential_create_rotate_replay_and_cross_workspace_scope_keep_secrets_write_only() {
        let (_dir, store, service) = test_service();
        let (private_key, _) = test_private_key(7);
        let request = CreateRepositorySshCredentialRequest {
            operation_id: "create-one".to_string(),
            credential_id: "deploy-main".to_string(),
            name: "Main deploy key".to_string(),
            private_key: private_key.clone(),
            passphrase: None,
        };
        let created = service
            .create_credential("workspace-a", request.clone(), "owner-a")
            .unwrap();
        let replayed = service
            .create_credential("workspace-a", request, "owner-a")
            .unwrap();
        assert_eq!(created, replayed);
        assert_eq!(created.current_revision, 1);
        assert!(
            service
                .get_credential("workspace-b", "deploy-main", &[])
                .unwrap()
                .is_none()
        );

        let serialized = serde_json::to_string(&created).unwrap();
        assert!(!serialized.contains("private_key"));
        assert!(!serialized.contains("secret_ref"));
        assert!(!serialized.contains("BEGIN OPENSSH"));
        store
            .with_conn(|conn| {
                let (nonce, ciphertext): (Vec<u8>, Vec<u8>) = conn.query_row(
                    "SELECT nonce, ciphertext FROM server_secret_versions WHERE workspace_id = 'workspace-a' AND secret_id = 'deploy-main' AND revision = 1 AND purpose = 'private_key'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                assert_eq!(nonce.len(), NONCE_BYTES);
                assert!(!ciphertext.windows(private_key.len()).any(|window| window == private_key.as_bytes()));
                Ok(())
            })
            .unwrap();

        let (rotated_key, _) = test_private_key(8);
        let rotated = service
            .rotate_credential(
                "workspace-a",
                "deploy-main",
                RotateRepositorySshCredentialRequest {
                    operation_id: "rotate-one".to_string(),
                    expected_revision: 1,
                    private_key: rotated_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        assert_eq!(rotated.current_revision, 2);
        assert_ne!(
            rotated.public_key_fingerprint,
            created.public_key_fingerprint
        );
    }

    #[test]
    fn pinned_host_trust_has_stable_fingerprint_and_revision() {
        let (_dir, _store, service) = test_service();
        let (_, public_key) = test_private_key(9);
        let created = service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-create".to_string(),
                    host_trust_id: "github".to_string(),
                    hostname: "GitHub.COM.".to_string(),
                    port: 22,
                    host_key: public_key.clone(),
                    expected_revision: None,
                },
                "owner-a",
            )
            .unwrap();
        assert_eq!(created.hostname, "github.com");
        assert_eq!(created.current_revision, 1);
        let updated = service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-update".to_string(),
                    host_trust_id: "github".to_string(),
                    hostname: "github.com".to_string(),
                    port: 22,
                    host_key: public_key,
                    expected_revision: Some(1),
                },
                "owner-a",
            )
            .unwrap();
        assert_eq!(updated.current_revision, 2);
        assert_eq!(updated.fingerprint, created.fingerprint);
    }

    fn config_state(source: &str) -> WorkspaceConfigState {
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let entry = ConfigEntry::new(path.clone(), ConfigContentType::Decodal, source).unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries(1, vec![entry]).unwrap();
        let schema_bundle = WorkspaceConfigSchemaBundle::compose(vec![
            RepositoryAccessConfigSchemaProvider.contribution().unwrap(),
        ])
        .unwrap();
        let contract = ToolchainContract::with_schema_bundle(
            DEFAULT_SCHEMA_VERSION,
            vec![path],
            DEFAULT_IMPORT_POLICY_VERSION,
            schema_bundle,
        );
        let evaluation = SnapshotEnvironment::new(snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap();
        WorkspaceConfigState {
            snapshot,
            contract,
            projection_digest: evaluation.projection_digest,
        }
    }

    #[test]
    fn workspace_config_projection_resolves_scoped_records_and_exact_host() {
        let (_dir, store, service) = test_service();
        let timestamp = now();
        let source = RepositorySource {
            kind: RepositorySourceKind::Ssh,
            uri: "ssh://git@example.test/org/repo.git".to_string(),
        };
        let source_fingerprint = crate::repository_source::repository_source_fingerprint(&source);
        store
            .upsert_repository(&RepositoryRecord {
                workspace_id: "workspace-a".to_string(),
                repository_id: "remote".to_string(),
                name: "Remote".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source,
                default_ref: Some("main".to_string()),
                source_revision: 1,
                source_fingerprint,
                observed_status: RepositoryObservedStatus::Unverified,
                observed_at: None,
                created_at: timestamp.clone(),
                updated_at: timestamp,
            })
            .unwrap();
        assert_eq!(
            store
                .get_repository("workspace-a", "remote")
                .unwrap()
                .unwrap()
                .source
                .kind,
            RepositorySourceKind::Ssh
        );
        let (private_key, public_key) = test_private_key(11);
        service
            .create_credential(
                "workspace-a",
                CreateRepositorySshCredentialRequest {
                    operation_id: "config-credential".to_string(),
                    credential_id: "deploy".to_string(),
                    name: "Deploy".to_string(),
                    private_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "config-host".to_string(),
                    host_trust_id: "example".to_string(),
                    hostname: "example.test".to_string(),
                    port: 22,
                    host_key: public_key,
                    expected_revision: None,
                },
                "owner-a",
            )
            .unwrap();
        let state = config_state(
            r#"{
                repository_access = {
                    remote = {
                        ssh = {
                            credential_id = "deploy";
                            host_trust_id = "example";
                            access = "read_only";
                        };
                    };
                };
            } as WorkspaceConfigSchema"#,
        );
        let projection =
            project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap();
        assert_eq!(projection.bindings.len(), 1);
        assert_eq!(projection.bindings[0].repository_id, "remote");
        assert_eq!(
            projection.bindings[0].access,
            RepositoryAccessMode::ReadOnly
        );

        let unknown = config_state(
            r#"{
                repository_access = {
                    remote = {
                        ssh = {
                            credential_id = "missing";
                            host_trust_id = "example";
                            access = "read_only";
                        };
                    };
                };
            } as WorkspaceConfigSchema"#,
        );
        let error = project_repository_access_state(&*store, &service, "workspace-a", &unknown)
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown Repository SSH credential")
        );
    }

    #[test]
    fn referenced_resources_cannot_be_deleted() {
        let (_dir, _store, service) = test_service();
        let (private_key, public_key) = test_private_key(10);
        service
            .create_credential(
                "workspace-a",
                CreateRepositorySshCredentialRequest {
                    operation_id: "create-ref".to_string(),
                    credential_id: "deploy".to_string(),
                    name: "Deploy".to_string(),
                    private_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-ref".to_string(),
                    host_trust_id: "host".to_string(),
                    hostname: "example.test".to_string(),
                    port: 22,
                    host_key: public_key,
                    expected_revision: None,
                },
                "owner-a",
            )
            .unwrap();
        let projection = RepositoryAccessProjection {
            workspace_id: "workspace-a".to_string(),
            config_revision: 3,
            projection_digest: "sha256:test".to_string(),
            bindings: vec![RepositorySshAccessBinding {
                repository_id: "main".to_string(),
                credential_id: "deploy".to_string(),
                host_trust_id: "host".to_string(),
                access: RepositoryAccessMode::ReadOnly,
            }],
        };
        let error = service
            .delete_credential(
                "workspace-a",
                "deploy",
                DeleteRepositorySshCredentialRequest {
                    operation_id: "delete-ref".to_string(),
                    expected_revision: 1,
                },
                "owner-a",
                &projection,
            )
            .unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
        assert!(
            service
                .get_credential("workspace-a", "deploy", &[])
                .unwrap()
                .is_some()
        );
    }
}
