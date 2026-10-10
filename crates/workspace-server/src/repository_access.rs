use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use config_source::ConfigSchemaContribution;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Deserialize;
use server_api::{
    CreateRepositorySshCredentialRequest, DeleteRepositorySshCredentialRequest,
    DeleteRepositorySshHostTrustRequest, GenerateRepositorySshCredentialRequest,
    PutRepositorySshHostTrustRequest, RepositoryAccessMode, RepositoryAccessProjection,
    RepositorySshAccessBinding, RepositorySshCredential, RepositorySshHostTrust,
    RepositorySshPublicKey, RotateRepositorySshCredentialRequest,
};
use sha2::{Digest, Sha256};
use ssh_key::private::Ed25519Keypair;
use ssh_key::{Algorithm, HashAlg, LineEnding, PrivateKey, PublicKey};

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
                credential = String;
                host_trust = String;
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
pub const WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID: &str = "workspace-default";
const WORKSPACE_DEFAULT_REPOSITORY_SSH_OPERATION_ID: &str = "workspace-default-repository-ssh-v1";
const WORKSPACE_DEFAULT_REPOSITORY_SSH_NAME: &str = "Workspace default SSH key";

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
    credential: String,
    host_trust: String,
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
        &state.projection_digest,
        &evaluation,
    )
}

fn validate_repository_access_source(
    repository_key: &str,
    source: &server_api::RepositorySource,
) -> Result<()> {
    if crate::repository_source::is_plain_http_repository_source(source) {
        return Err(Error::InvalidInput(format!(
            "repository_source_plain_http_unsupported: Repository `{repository_key}` uses unsupported plain HTTP; register an HTTPS or SSH source instead"
        )));
    }
    Ok(())
}

pub(crate) fn repository_ssh_endpoint(
    repository_key: &str,
    repository_uri: &str,
) -> Result<Option<(String, u16)>> {
    if !repository_uri.contains("://") {
        if let Some((identity, path)) = repository_uri.split_once(':')
            && !path.is_empty()
            && let Some((_, hostname)) = identity.rsplit_once('@')
            && !hostname.is_empty()
        {
            return Ok(Some((hostname.to_ascii_lowercase(), 22)));
        }
    }
    let parsed = url::Url::parse(repository_uri).map_err(|error| {
        Error::InvalidInput(format!(
            "Repository `{repository_key}` has invalid SSH URI: {error}"
        ))
    })?;
    if parsed.scheme() != "ssh" {
        return Ok(None);
    }
    if parsed.username().is_empty() || parsed.password().is_some() {
        return Err(Error::InvalidInput(format!(
            "Repository `{repository_key}` must use ssh://user@host[:port]/path without embedded credentials"
        )));
    }
    let hostname = parsed.host_str().ok_or_else(|| {
        Error::InvalidInput(format!(
            "Repository `{repository_key}` SSH URI has no hostname"
        ))
    })?;
    Ok(Some((
        hostname.to_ascii_lowercase(),
        parsed.port().unwrap_or(22),
    )))
}

fn project_repository_access_evaluation(
    store: &dyn ControlPlaneStore,
    secrets: &RepositorySecretService,
    workspace_id: &str,
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
    if !config.repository_access.is_empty() {
        secrets.ensure_workspace_default_credential(workspace_id)?;
    }
    let mut bindings = Vec::with_capacity(config.repository_access.len());
    for (repository_key, access) in config.repository_access {
        server_api::validate_repository_key(&repository_key)
            .map_err(|error| Error::InvalidInput(format!("invalid Repository key: {error}")))?;
        validate_identifier("credential_id", &access.ssh.credential)?;
        validate_identifier("host_trust_id", &access.ssh.host_trust)?;
        let repository = store
            .get_repository_by_key(workspace_id, &repository_key)?
            .ok_or_else(|| Error::InvalidInput(format!("unknown Repository `{repository_key}`")))?;
        validate_repository_access_source(&repository_key, &repository.source)?;
        if repository.source.kind != server_api::RepositorySourceKind::Ssh {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_key}` is not an ssh:// Repository"
            )));
        }
        let credential = secrets
            .get_credential(workspace_id, &access.ssh.credential, &[])?
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "unknown Repository SSH credential `{}`",
                    access.ssh.credential
                ))
            })?;
        if credential.status != "active" {
            return Err(Error::InvalidInput(format!(
                "Repository SSH credential `{}` is not active",
                access.ssh.credential
            )));
        }
        let host_trust = secrets
            .get_host_trust(workspace_id, &access.ssh.host_trust, &[])?
            .ok_or_else(|| {
                Error::InvalidInput(format!(
                    "unknown Repository SSH host trust `{}`",
                    access.ssh.host_trust
                ))
            })?;
        let (hostname, port) =
            repository_ssh_endpoint(repository_key.as_str(), &repository.source.uri)?.ok_or_else(
                || {
                    Error::InvalidInput(format!(
                        "Repository `{repository_key}` must use an SSH source"
                    ))
                },
            )?;
        if hostname != host_trust.hostname || port != host_trust.port {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_key}` SSH host does not match host trust `{}`",
                access.ssh.host_trust
            )));
        }
        bindings.push(RepositorySshAccessBinding {
            repository_key,
            credential_id: access.ssh.credential,
            host_trust_id: access.ssh.host_trust,
            access: access.ssh.access,
        });
    }
    bindings.sort_by(|left, right| left.repository_key.cmp(&right.repository_key));
    Ok(RepositoryAccessProjection {
        workspace_id: workspace_id.to_string(),
        projection_digest: projection_digest.to_string(),
        bindings,
    })
}

/// An immutable SSH materialization selection. Operation IDs name the actual
/// mutations that stored these envelopes/endpoints; fingerprints name key identity.
#[derive(Clone)]
pub struct LeasedRepositorySshAccess {
    pub credential_id: String,
    pub credential_fingerprint: String,
    pub host_trust_id: String,
    pub host_trust_fingerprint: String,
    pub private_key: zeroize::Zeroizing<String>,
    pub known_hosts_entry: String,
}

/// A durable acknowledgement, not a credential or a lease. A removed resource
/// remains recoverable here even when its original typed request cannot be proved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RepositorySecretOperationStatus {
    pub operation_id: String,
    pub state: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub result_operation_id: String,
    pub created_at: String,
    pub mutation_kind: Option<String>,
    pub legacy_precondition_proven: Option<bool>,
}

#[derive(Clone)]
pub struct RepositorySecretService {
    store: Arc<SqliteWorkspaceStore>,
    master_key: Option<Arc<[u8; MASTER_KEY_BYTES]>>,
}

impl RepositorySecretService {
    pub fn operation_status(
        &self,
        workspace_id: &str,
        operation_id: &str,
    ) -> Result<Option<RepositorySecretOperationStatus>> {
        let operation_id = validate_identifier("operation_id", operation_id)?;
        self.store.with_conn(|conn| {
            let Some((_,resource_kind,resource_id,result_operation_id))=read_operation(conn,workspace_id,&operation_id)? else { return Ok(None); };
            let created_at=conn.query_row("SELECT created_at FROM repository_secret_operations WHERE workspace_id=?1 AND operation_id=?2",params![workspace_id,operation_id],|r|r.get(0))?;
            let legacy: Option<(Option<String>,Option<i64>,Option<String>,Option<String>)>=conn.query_row(
                "SELECT mutation_kind,expected_revision,expected_operation_id,expected_key_fingerprint FROM repository_secret_legacy_receipts WHERE workspace_id=?1 AND operation_id=?2",
                params![workspace_id,operation_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
            ).optional()?;
            let (mutation_kind,legacy_precondition_proven)=if let Some((kind,counter,prior,key))=legacy {
                let proven=match kind.as_deref() {
                    Some("credential_created"|"host_trust_created") => counter==Some(0),
                    Some("credential_rotated"|"host_trust_rotated"|"credential_deleted"|"host_trust_deleted") => counter.is_some_and(|c|c>0) && prior.is_some_and(|p|!p.is_empty()) && key.is_some_and(|k|!k.is_empty()),
                    _ => false,
                };
                (kind,Some(proven))
            } else {
                let kind=conn.query_row("SELECT min(kind) FROM repository_secret_audit_events WHERE workspace_id=?1 AND operation_id=?2 HAVING count(*)=1",params![workspace_id,operation_id],|r|r.get::<_,Option<String>>(0)).optional()?.flatten();
                (kind,None)
            };
            Ok(Some(RepositorySecretOperationStatus { operation_id,state:"committed".into(),resource_kind,resource_id,result_operation_id,created_at,mutation_kind,legacy_precondition_proven }))
        })
    }

    pub fn open(store: Arc<SqliteWorkspaceStore>, database_path: &Path) -> Result<Self> {
        let key_path = master_key_path(database_path)?;
        let key = load_or_create_master_key(&key_path)?;
        Ok(Self {
            store,
            master_key: Some(Arc::new(key)),
        })
    }

    fn generated_ed25519_private_key(
        &self,
        workspace_id: &str,
        operation_id: &str,
        credential_id: &str,
        intent: &str,
    ) -> Result<String> {
        let master_key = self.master_key.as_ref().ok_or_else(|| {
            Error::Store("Repository secret encryption authority is unavailable".to_string())
        })?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, master_key.as_slice());
        let context = format!(
            "yoi/repository-ssh-key/v1\0{workspace_id}\0{operation_id}\0{credential_id}\0{intent}"
        );
        let seed = hmac::sign(&key, context.as_bytes());
        PrivateKey::from(Ed25519Keypair::from_seed(
            seed.as_ref().try_into().map_err(|_| {
                Error::Store("generated SSH Ed25519 seed had an invalid length".to_string())
            })?,
        ))
        .to_openssh(LineEnding::LF)
        .map(|key| key.to_string())
        .map_err(|err| Error::Store(format!("failed to encode generated SSH key: {err}")))
    }

    pub fn generate_credential(
        &self,
        workspace_id: &str,
        request: GenerateRepositorySshCredentialRequest,
        actor_account_id: &str,
    ) -> Result<RepositorySshCredential> {
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let credential_id = validate_identifier("credential_id", &request.credential_id)?;
        let name = normalize_name(&request.name)?;
        let private_key = self.generated_ed25519_private_key(
            workspace_id,
            &operation_id,
            &credential_id,
            &format!("create\0{name}"),
        )?;
        self.create_credential(
            workspace_id,
            CreateRepositorySshCredentialRequest {
                operation_id,
                credential_id,
                name,
                private_key,
                passphrase: None,
            },
            actor_account_id,
        )
    }

    pub fn ensure_workspace_default_credential(
        &self,
        workspace_id: &str,
    ) -> Result<RepositorySshCredential> {
        if let Some(credential) = self.store.with_conn(|conn| {
            read_credential(
                conn,
                workspace_id,
                WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
            )
        })? {
            return Ok(credential);
        }
        self.generate_credential(
            workspace_id,
            GenerateRepositorySshCredentialRequest {
                operation_id: WORKSPACE_DEFAULT_REPOSITORY_SSH_OPERATION_ID.to_string(),
                credential_id: WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID.to_string(),
                name: WORKSPACE_DEFAULT_REPOSITORY_SSH_NAME.to_string(),
            },
            "workspace-system",
        )
    }

    pub fn credential_public_key(
        &self,
        workspace_id: &str,
        credential_id: &str,
    ) -> Result<Option<RepositorySshPublicKey>> {
        let credential_id = validate_identifier("credential_id", credential_id)?;
        let Some((credential, operation_id, private_secret, passphrase_secret)) =
            self.store.with_conn(|conn| {
                let Some(credential) = read_credential(conn, workspace_id, &credential_id)? else {
                    return Ok(None);
                };
                let operation_id: String = conn.query_row(
                    "SELECT current_operation_id FROM repository_ssh_credentials WHERE workspace_id=?1 AND credential_id=?2",
                    params![workspace_id, credential_id], |row| row.get(0),
                )?;
                let private_secret = read_sealed_secret(
                    conn,
                    workspace_id,
                    &credential_id,
                    &operation_id,
                    "private_key",
                )?
                .ok_or_else(|| Error::Store("credential private key is missing".to_string()))?;
                let passphrase_secret = read_sealed_secret(
                    conn,
                    workspace_id,
                    &credential_id,
                    &operation_id,
                    "passphrase",
                )?;
                Ok(Some((credential, operation_id, private_secret, passphrase_secret)))
            })?
        else {
            return Ok(None);
        };
        let private_key = zeroize::Zeroizing::new(self.unseal(
            workspace_id,
            &credential_id,
            &operation_id,
            "private_key",
            private_secret,
        )?);
        let passphrase = passphrase_secret
            .map(|secret| {
                self.unseal(
                    workspace_id,
                    &credential_id,
                    &operation_id,
                    "passphrase",
                    secret,
                )
                .map(zeroize::Zeroizing::new)
            })
            .transpose()?;
        let private_key = std::str::from_utf8(private_key.as_slice())
            .map_err(|_| Error::Store("credential private key is not UTF-8".to_string()))?;
        let passphrase = passphrase
            .as_deref()
            .map(|value| std::str::from_utf8(value.as_slice()))
            .transpose()
            .map_err(|_| Error::Store("credential passphrase is not UTF-8".to_string()))?;
        let parsed = parse_private_key(private_key, passphrase).map_err(|err| {
            Error::Store(format!("stored credential private key is invalid: {err}"))
        })?;
        if parsed.fingerprint != credential.public_key_fingerprint {
            return Err(Error::RegistryInconsistency(
                "Repository SSH credential key identity mismatch".to_string(),
            ));
        }
        Ok(Some(RepositorySshPublicKey {
            credential_id,
            public_key_algorithm: parsed.algorithm,
            public_key_fingerprint: parsed.fingerprint,
            public_key: parsed.public_key,
        }))
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
            "",
            &request.private_key,
            request.passphrase.as_deref(),
        );
        let private_secret = self.seal(
            workspace_id,
            &credential_id,
            &operation_id,
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
                    &operation_id,
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
                LegacySecretRequest::Credential {
                    mutation_kind: "credential_created",
                    name: &name,
                    expected_key: None,
                    private_key: &request.private_key,
                    passphrase: request.passphrase.as_deref(),
                },
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
                    public_key_fingerprint, current_operation_id, status, created_at, rotated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, NULL)"#,
                params![
                    workspace_id,
                    credential_id,
                    name,
                    parsed.algorithm,
                    parsed.fingerprint,
                    operation_id,
                    now
                ],
            )?;
            tx.execute(
                r#"INSERT INTO repository_ssh_credential_keys (
                    workspace_id, credential_id, operation_id, public_key_algorithm,
                    public_key_fingerprint, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    workspace_id,
                    credential_id,
                    operation_id,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now
                ],
            )?;
            insert_secret(
                &tx,
                workspace_id,
                &credential_id,
                &operation_id,
                "private_key",
                &private_secret,
                &now,
            )?;
            if let Some(secret) = passphrase_secret.as_ref() {
                insert_secret(
                    &tx,
                    workspace_id,
                    &credential_id,
                    &operation_id,
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
                &operation_id,
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
                &operation_id,
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
        if credential_id == WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID {
            return Err(Error::WorkspaceConfigConflict(
                "Workspace default SSH credential is immutable".to_string(),
            ));
        }
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        let parsed = parse_private_key(&request.private_key, request.passphrase.as_deref())?;
        validate_key_fingerprint(&request.expected_public_key_fingerprint)?;
        let fingerprint = credential_fingerprint(
            "rotate",
            &credential_id,
            "",
            &request.expected_public_key_fingerprint,
            &request.private_key,
            request.passphrase.as_deref(),
        );
        let private_secret = self.seal(
            workspace_id,
            &credential_id,
            &operation_id,
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
                    &operation_id,
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
                LegacySecretRequest::Credential {
                    mutation_kind: "credential_rotated",
                    name: "",
                    expected_key: Some(&request.expected_public_key_fingerprint),
                    private_key: &request.private_key,
                    passphrase: request.passphrase.as_deref(),
                },
            )? {
                tx.commit()?;
                return Ok(replayed);
            }
            let current = read_credential(&tx, workspace_id, &credential_id)?
                .ok_or_else(|| Error::InvalidRecordId(credential_id.clone()))?;
            if current.public_key_fingerprint != request.expected_public_key_fingerprint
                || current.status != "active"
            {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` key fingerprint/status changed"
                )));
            }
            tx.execute(
                r#"INSERT INTO repository_ssh_credential_keys (
                    workspace_id, credential_id, operation_id, public_key_algorithm,
                    public_key_fingerprint, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
                params![
                    workspace_id,
                    credential_id,
                    &operation_id,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now
                ],
            )?;
            insert_secret(
                &tx,
                workspace_id,
                &credential_id,
                &operation_id,
                "private_key",
                &private_secret,
                &now,
            )?;
            if let Some(secret) = passphrase_secret.as_ref() {
                insert_secret(
                    &tx,
                    workspace_id,
                    &credential_id,
                    &operation_id,
                    "passphrase",
                    secret,
                    &now,
                )?;
            }
            let updated = tx.execute(
                r#"UPDATE repository_ssh_credentials
                   SET public_key_algorithm = ?4, public_key_fingerprint = ?5,
                       current_operation_id = ?3, rotated_at = ?6
                   WHERE workspace_id = ?1 AND credential_id = ?2
                     AND public_key_fingerprint = ?7 AND status = 'active'"#,
                params![
                    workspace_id,
                    credential_id,
                    &operation_id,
                    parsed.algorithm,
                    parsed.fingerprint,
                    now,
                    request.expected_public_key_fingerprint
                ],
            )?;
            if updated != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` key fingerprint changed"
                )));
            }
            insert_audit(
                &tx,
                workspace_id,
                "credential_rotated",
                &credential_id,
                &operation_id,
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
                &operation_id,
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
        if credential_id == WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID {
            return Err(Error::WorkspaceConfigConflict(
                "Workspace default SSH credential is immutable".to_string(),
            ));
        }
        let operation_id = validate_identifier("operation_id", &request.operation_id)?;
        validate_key_fingerprint(&request.expected_public_key_fingerprint)?;
        let references = credential_references(projection, &credential_id);
        if !references.is_empty() {
            return Err(Error::WorkspaceConfigConflict(format!(
                "credential `{credential_id}` is referenced by active Workspace config"
            )));
        }
        let fingerprint = simple_operation_fingerprint(
            "delete_credential",
            &credential_id,
            &request.expected_public_key_fingerprint,
        );
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if replay_deleted_operation(&tx, workspace_id, &operation_id, &fingerprint, "credential", &credential_id, LegacySecretRequest::Delete { mutation_kind: "credential_deleted", expected_key: &request.expected_public_key_fingerprint })? {
                tx.commit()?;
                return Ok(());
            }
            let current = read_credential(&tx, workspace_id, &credential_id)?
                .ok_or_else(|| Error::InvalidRecordId(credential_id.clone()))?;
            if current.public_key_fingerprint != request.expected_public_key_fingerprint {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` key fingerprint changed"
                )));
            }
            let retained_by_workdir_create: bool = tx.query_row(
                r#"SELECT EXISTS(
                       SELECT 1
                       FROM workdir_create_credential_retentions
                       WHERE workspace_id = ?1 AND credential_id = ?2
                   )"#,
                params![workspace_id, credential_id],
                |row| row.get(0),
            )?;
            if retained_by_workdir_create {
                return Err(Error::RepositoryConflict(format!(
                    "credential `{credential_id}` is retained by a retryable Workdir create operation"
                )));
            }
            insert_audit(&tx, workspace_id, "credential_deleted", &credential_id, &operation_id, actor_account_id, &now)?;
            let deleted = tx.execute(
                "DELETE FROM repository_ssh_credentials WHERE workspace_id = ?1 AND credential_id = ?2 AND public_key_fingerprint = ?3",
                params![workspace_id, credential_id, request.expected_public_key_fingerprint],
            )?;
            if deleted != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "credential `{credential_id}` key fingerprint changed"
                )));
            }
            insert_operation(&tx, workspace_id, &operation_id, &fingerprint, "credential", &credential_id, &operation_id, &now)?;
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
                          public_key_fingerprint, current_operation_id, status, created_at, rotated_at
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
        if let Some(expected) = request.expected_fingerprint.as_deref() {
            validate_key_fingerprint(expected)?;
        }
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
                LegacySecretRequest::Host {
                    hostname: &hostname,
                    port: request.port,
                    key_fingerprint: &parsed.fingerprint,
                    expected_key: request.expected_fingerprint.as_deref(),
                },
            )? {
                tx.commit()?;
                return Ok(replayed);
            }
            ensure_workspace_exists(&tx, workspace_id)?;
            let current = read_host_trust(&tx, workspace_id, &host_trust_id)?;
            match (current.as_ref(), request.expected_fingerprint.as_deref()) {
                (None, None) => {}
                (Some(current), Some(expected)) if current.fingerprint == expected => {}
                _ => {
                    return Err(Error::WorkspaceConfigConflict(format!(
                        "host trust `{host_trust_id}` create/update operation precondition failed"
                    )));
                }
            }
            if current.is_none() {
                tx.execute(
                    r#"INSERT INTO repository_ssh_host_trusts (
                        workspace_id, host_trust_id, hostname, port, key_algorithm,
                        host_key, fingerprint, current_operation_id, created_at, updated_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)"#,
                    params![
                        workspace_id,
                        host_trust_id,
                        hostname,
                        request.port,
                        parsed.algorithm,
                        parsed.canonical_key,
                        parsed.fingerprint,
                        &operation_id,
                        now
                    ],
                )?;
            } else {
                tx.execute(
                    r#"UPDATE repository_ssh_host_trusts
                       SET hostname = ?4, port = ?5, key_algorithm = ?6,
                           host_key = ?7, fingerprint = ?8,
                           current_operation_id = ?3, updated_at = ?9
                       WHERE workspace_id = ?1 AND host_trust_id = ?2
                         AND fingerprint = ?10"#,
                    params![
                        workspace_id,
                        host_trust_id,
                        &operation_id,
                        hostname,
                        request.port,
                        parsed.algorithm,
                        parsed.canonical_key,
                        parsed.fingerprint,
                        now,
                        request.expected_fingerprint
                    ],
                )?;
            }
            tx.execute(
                r#"INSERT INTO repository_ssh_host_trust_keys (
                    workspace_id, host_trust_id, operation_id, hostname, port,
                    key_algorithm, host_key, fingerprint, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"#,
                params![
                    workspace_id,
                    host_trust_id,
                    &operation_id,
                    hostname,
                    request.port,
                    parsed.algorithm,
                    parsed.canonical_key,
                    parsed.fingerprint,
                    now
                ],
            )?;
            let event = if current.is_none() {
                "host_trust_created"
            } else {
                "host_trust_rotated"
            };
            insert_audit(
                &tx,
                workspace_id,
                event,
                &host_trust_id,
                &operation_id,
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
                &operation_id,
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
        validate_key_fingerprint(&request.expected_fingerprint)?;
        if !host_trust_references(projection, &host_trust_id).is_empty() {
            return Err(Error::WorkspaceConfigConflict(format!(
                "host trust `{host_trust_id}` is referenced by active Workspace config"
            )));
        }
        let fingerprint = simple_operation_fingerprint(
            "delete_host_trust",
            &host_trust_id,
            &request.expected_fingerprint,
        );
        let now = now();
        self.store.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if replay_deleted_operation(&tx, workspace_id, &operation_id, &fingerprint, "host_trust", &host_trust_id, LegacySecretRequest::Delete { mutation_kind: "host_trust_deleted", expected_key: &request.expected_fingerprint })? {
                tx.commit()?;
                return Ok(());
            }
            let current = read_host_trust(&tx, workspace_id, &host_trust_id)?
                .ok_or_else(|| Error::InvalidRecordId(host_trust_id.clone()))?;
            if current.fingerprint != request.expected_fingerprint {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "host trust `{host_trust_id}` key fingerprint changed"
                )));
            }
            insert_audit(&tx, workspace_id, "host_trust_deleted", &host_trust_id, &operation_id, actor_account_id, &now)?;
            let deleted = tx.execute(
                "DELETE FROM repository_ssh_host_trusts WHERE workspace_id = ?1 AND host_trust_id = ?2 AND fingerprint = ?3",
                params![workspace_id, host_trust_id, request.expected_fingerprint],
            )?;
            if deleted != 1 {
                return Err(Error::WorkspaceConfigConflict(format!(
                    "host trust `{host_trust_id}` key fingerprint changed"
                )));
            }
            insert_operation(&tx, workspace_id, &operation_id, &fingerprint, "host_trust", &host_trust_id, &operation_id, &now)?;
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
                          host_key, fingerprint, current_operation_id, created_at, updated_at
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

    pub fn host_trusts_for_endpoint(
        &self,
        workspace_id: &str,
        hostname: &str,
        port: u16,
    ) -> Result<Vec<RepositorySshHostTrust>> {
        self.store.with_conn(|conn| {
            let mut statement = conn.prepare(
                r#"SELECT workspace_id, host_trust_id, hostname, port, key_algorithm,
                          host_key, fingerprint, current_operation_id, created_at, updated_at
                   FROM repository_ssh_host_trusts
                   WHERE workspace_id = ?1 AND lower(hostname) = lower(?2) AND port = ?3
                   ORDER BY host_trust_id"#,
            )?;
            statement
                .query_map(
                    params![workspace_id, hostname, i64::from(port)],
                    read_host_trust_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(Error::from)
        })
    }

    pub fn automatic_host_trust_id(hostname: &str, port: u16) -> String {
        let normalized = hostname
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                    character.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(96)
            .collect::<String>();
        format!("tofu-{normalized}-{port}")
    }

    pub fn default_ssh_binding_for_repository(
        &self,
        workspace_id: &str,
        repository_key: &str,
        repository_uri: &str,
    ) -> Result<Option<RepositorySshAccessBinding>> {
        let Some((hostname, port)) = repository_ssh_endpoint(repository_key, repository_uri)?
        else {
            return Ok(None);
        };
        let matches = self.host_trusts_for_endpoint(workspace_id, &hostname, port)?;
        let Some(host_trust) = matches.first() else {
            return Ok(None);
        };
        if matches.len() > 1 {
            return Err(Error::InvalidInput(format!(
                "Repository `{repository_key}` matches multiple SSH host trusts for {hostname}:{port}; configure an explicit Repository access binding"
            )));
        }
        self.ensure_workspace_default_credential(workspace_id)?;
        Ok(Some(RepositorySshAccessBinding {
            repository_key: repository_key.to_string(),
            credential_id: WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID.to_string(),
            host_trust_id: host_trust.host_trust_id.clone(),
            access: RepositoryAccessMode::ReadWrite,
        }))
    }

    pub fn lease_ssh_materialization_access(
        &self,
        workspace_id: &str,
        binding: &RepositorySshAccessBinding,
    ) -> Result<LeasedRepositorySshAccess> {
        // A live binding carries current endpoint authority. Do not resolve it by
        // fingerprint alone: a host key can legitimately have historical endpoints.
        let (credential_operation, host_operation) = self.store.with_conn(|conn| {
            let credential: String = conn.query_row(
                "SELECT current_operation_id FROM repository_ssh_credentials WHERE workspace_id=?1 AND credential_id=?2 AND status='active'",
                params![workspace_id, binding.credential_id], |row| row.get(0),
            ).optional()?.ok_or_else(|| Error::InvalidInput("Repository SSH credential is unavailable or inactive".into()))?;
            let host: String = conn.query_row(
                "SELECT current_operation_id FROM repository_ssh_host_trusts WHERE workspace_id=?1 AND host_trust_id=?2",
                params![workspace_id, binding.host_trust_id], |row| row.get(0),
            ).optional()?.ok_or_else(|| Error::InvalidInput("Repository SSH host trust is unavailable".into()))?;
            Ok((credential, host))
        })?;
        self.lease_ssh_materialization_access_operation(
            workspace_id,
            &binding.credential_id,
            &credential_operation,
            &binding.host_trust_id,
            &host_operation,
        )
    }

    pub fn lease_ssh_materialization_access_fingerprints(
        &self,
        workspace_id: &str,
        credential_id: &str,
        credential_fingerprint: &str,
        host_trust_id: &str,
        host_trust_fingerprint: &str,
    ) -> Result<LeasedRepositorySshAccess> {
        let (credential_operation, host_operation) = self.store.with_conn(|conn| {
            let credential_operation: String = conn.query_row(
                "SELECT k.operation_id FROM repository_ssh_credential_keys k JOIN repository_ssh_credentials c ON c.workspace_id=k.workspace_id AND c.credential_id=k.credential_id WHERE k.workspace_id=?1 AND k.credential_id=?2 AND k.public_key_fingerprint=?3 AND c.status='active' ORDER BY (k.operation_id=c.current_operation_id) DESC, k.created_at DESC, k.operation_id DESC LIMIT 1",
                params![workspace_id, credential_id, credential_fingerprint], |row| row.get(0),
            )?;
            let mut statement = conn.prepare("SELECT operation_id, hostname, port, host_key FROM repository_ssh_host_trust_keys WHERE workspace_id=?1 AND host_trust_id=?2 AND fingerprint=?3 ORDER BY created_at DESC, operation_id DESC")?;
            let matches = statement.query_map(params![workspace_id, host_trust_id, host_trust_fingerprint], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, u16>(2)?, row.get::<_, String>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let first = matches.first().ok_or_else(|| Error::RegistryInconsistency("Repository SSH host key is unavailable".into()))?;
            if matches.iter().any(|entry| (&entry.1, entry.2, &entry.3) != (&first.1, first.2, &first.3)) {
                return Err(Error::RegistryInconsistency("Repository SSH host key fingerprint has ambiguous endpoint evidence".into()));
            }
            Ok((credential_operation, first.0.clone()))
        })?;
        self.lease_ssh_materialization_access_operation(
            workspace_id,
            credential_id,
            &credential_operation,
            host_trust_id,
            &host_operation,
        )
    }

    fn lease_ssh_materialization_access_operation(
        &self,
        workspace_id: &str,
        credential_id: &str,
        credential_operation_id: &str,
        host_trust_id: &str,
        host_trust_operation_id: &str,
    ) -> Result<LeasedRepositorySshAccess> {
        let (private_key, passphrase, credential_fingerprint, hostname, port, host_key, host_fingerprint) = self.store.with_conn(|conn| {
            let active: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM repository_ssh_credentials WHERE workspace_id=?1 AND credential_id=?2 AND status='active')",
                params![workspace_id, credential_id], |row| row.get(0),
            )?;
            if !active { return Err(Error::InvalidInput("Repository SSH credential is unavailable or inactive".into())); }
            let credential_fingerprint: String = conn.query_row(
                "SELECT public_key_fingerprint FROM repository_ssh_credential_keys WHERE workspace_id = ?1 AND credential_id = ?2 AND operation_id = ?3",
                params![workspace_id, credential_id, credential_operation_id],
                |row| row.get(0),
            ).optional()?.ok_or_else(|| Error::RegistryInconsistency(
                "Repository SSH credential key identity is unavailable".to_string()
            ))?;
            let private_key = read_sealed_secret(
                conn,
                workspace_id,
                credential_id,
                credential_operation_id,
                "private_key",
            )?
            .ok_or_else(|| {
                Error::RegistryInconsistency(format!(
                    "Repository SSH credential `{credential_id}` operation_id {credential_operation_id} is unavailable"
                ))
            })?;
            let passphrase = read_sealed_secret(
                conn,
                workspace_id,
                credential_id,
                credential_operation_id,
                "passphrase",
            )?;
            let (hostname, port, host_key, host_fingerprint) = conn
                .query_row(
                    r#"SELECT v.hostname, v.port, v.host_key, v.fingerprint
                       FROM repository_ssh_host_trusts h
                       JOIN repository_ssh_host_trust_keys v
                         ON v.workspace_id = h.workspace_id
                        AND v.host_trust_id = h.host_trust_id
                       WHERE h.workspace_id = ?1 AND h.host_trust_id = ?2
                         AND v.operation_id = ?3"#,
                    params![workspace_id, host_trust_id, host_trust_operation_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)? as u16,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| {
                    Error::RegistryInconsistency(format!(
                        "Repository SSH host trust `{host_trust_id}` operation_id {host_trust_operation_id} is unavailable"
                    ))
                })?;
            Ok((private_key, passphrase, credential_fingerprint, hostname, port, host_key, host_fingerprint))
        })?;
        let private_key = self.unseal(
            workspace_id,
            credential_id,
            credential_operation_id,
            "private_key",
            private_key,
        )?;
        let passphrase = passphrase
            .map(|secret| {
                self.unseal(
                    workspace_id,
                    credential_id,
                    credential_operation_id,
                    "passphrase",
                    secret,
                )
            })
            .transpose()?;
        let private_key =
            zeroize::Zeroizing::new(String::from_utf8(private_key).map_err(|_| {
                Error::Store("Repository SSH private key plaintext is invalid".to_string())
            })?);
        let passphrase = passphrase
            .map(|value| {
                String::from_utf8(value)
                    .map(zeroize::Zeroizing::new)
                    .map_err(|_| {
                        Error::Store("Repository SSH passphrase plaintext is invalid".to_string())
                    })
            })
            .transpose()?;
        let key = PrivateKey::from_openssh(private_key.as_str()).map_err(|_| {
            Error::Store("Repository SSH private key plaintext is invalid".to_string())
        })?;
        let key = if key.is_encrypted() {
            key.decrypt(passphrase.as_deref().ok_or_else(|| {
                Error::Store("Repository SSH passphrase operation_id is unavailable".to_string())
            })?)
            .map_err(|_| Error::Store("Repository SSH private key decryption failed".to_string()))?
        } else {
            key
        };
        if key.public_key().fingerprint(HashAlg::Sha256).to_string() != credential_fingerprint
            || parse_host_key(&host_key)
                .map_err(|_| Error::Store("stored host key is invalid".to_string()))?
                .fingerprint
                != host_fingerprint
        {
            return Err(Error::RegistryInconsistency(
                "Repository SSH key identity mismatch".to_string(),
            ));
        }
        let private_key = key
            .to_openssh(LineEnding::LF)
            .map_err(|_| Error::Store("Repository SSH private key encoding failed".to_string()))?;
        let host = if port == 22 {
            hostname
        } else {
            format!("[{hostname}]:{port}")
        };
        Ok(LeasedRepositorySshAccess {
            credential_id: credential_id.to_string(),
            credential_fingerprint,
            host_trust_id: host_trust_id.to_string(),
            host_trust_fingerprint: host_fingerprint,
            private_key,
            known_hosts_entry: format!("{host} {host_key}\n"),
        })
    }

    fn unseal(
        &self,
        workspace_id: &str,
        credential_id: &str,
        operation_id: &str,
        purpose: &str,
        secret: SealedSecret,
    ) -> Result<Vec<u8>> {
        let master_key = self.master_key.as_ref().ok_or_else(|| {
            Error::Store("Repository secret encryption authority is unavailable".to_string())
        })?;
        let unbound = UnboundKey::new(&AES_256_GCM, master_key.as_slice())
            .map_err(|_| Error::Store("Repository secret encryption key is invalid".to_string()))?;
        let key = LessSafeKey::new(unbound);
        let mut plaintext = secret.ciphertext;
        let aad = secret_aad(workspace_id, credential_id, operation_id, purpose);
        let plaintext_len = key
            .open_in_place(
                Nonce::assume_unique_for_key(secret.nonce),
                Aad::from(aad.as_bytes()),
                &mut plaintext,
            )
            .map_err(|_| Error::Store("Repository secret decryption failed".to_string()))?
            .len();
        plaintext.truncate(plaintext_len);
        Ok(plaintext)
    }

    fn seal(
        &self,
        workspace_id: &str,
        credential_id: &str,
        operation_id: &str,
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
        let aad = secret_aad(workspace_id, credential_id, operation_id, purpose);
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
    public_key: String,
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
        public_key: public_key.to_openssh().map_err(|err| {
            Error::Store(format!("failed to encode Repository SSH public key: {err}"))
        })?,
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
    operation_id: &str,
    purpose: &str,
    secret: &SealedSecret,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO server_secret_objects (
            workspace_id, secret_id, operation_id, purpose, encryption_algorithm,
            nonce, ciphertext, created_at
        ) VALUES (?1, ?2, ?3, ?4, 'aes-256-gcm-v1', ?5, ?6, ?7)"#,
        params![
            workspace_id,
            credential_id,
            operation_id,
            purpose,
            secret.nonce.as_slice(),
            secret.ciphertext,
            created_at
        ],
    )?;
    Ok(())
}

fn read_sealed_secret(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    credential_id: &str,
    operation_id: &str,
    purpose: &str,
) -> Result<Option<SealedSecret>> {
    let row = conn
        .query_row(
            r#"SELECT encryption_algorithm, nonce, ciphertext
               FROM server_secret_objects
               WHERE workspace_id = ?1 AND secret_id = ?2
                 AND operation_id = ?3 AND purpose = ?4"#,
            params![workspace_id, credential_id, operation_id, purpose],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((algorithm, nonce, ciphertext)) = row else {
        return Ok(None);
    };
    if algorithm != "aes-256-gcm-v1" || nonce.len() != NONCE_BYTES {
        return Err(Error::RegistryInconsistency(
            "Repository secret envelope is invalid".to_string(),
        ));
    }
    let mut nonce_bytes = [0u8; NONCE_BYTES];
    nonce_bytes.copy_from_slice(&nonce);
    Ok(Some(SealedSecret {
        nonce: nonce_bytes,
        ciphertext,
    }))
}

// Complete caller typed input is hashed in the frozen encoding. Counters are
// evidence for an already committed receipt only, never live mutation CAS.
enum LegacySecretRequest<'a> {
    Credential {
        mutation_kind: &'a str,
        name: &'a str,
        expected_key: Option<&'a str>,
        private_key: &'a str,
        passphrase: Option<&'a str>,
    },
    Host {
        hostname: &'a str,
        port: u16,
        key_fingerprint: &'a str,
        expected_key: Option<&'a str>,
    },
    Delete {
        mutation_kind: &'a str,
        expected_key: &'a str,
    },
}

fn matches_legacy_secret_replay(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
    resource_id: &str,
    stored_fingerprint: &str,
    request: LegacySecretRequest<'_>,
) -> Result<bool> {
    let evidence: Option<(String,Option<String>,Option<i64>,Option<String>,Option<String>)> = conn.query_row(
        "SELECT request_fingerprint,mutation_kind,expected_revision,expected_operation_id,expected_key_fingerprint FROM repository_secret_legacy_receipts WHERE workspace_id=?1 AND operation_id=?2 AND resource_id=?3",
        params![workspace_id,operation_id,resource_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)),
    ).optional()?;
    let Some((original, Some(mutation_kind), Some(counter), prior_operation, prior_key)) = evidence
    else {
        return Ok(false);
    };
    let Ok(counter) = u64::try_from(counter) else {
        return Ok(false);
    };
    if original != stored_fingerprint {
        return Ok(false);
    }
    let (requested_kind, expected_key) = match &request {
        LegacySecretRequest::Credential {
            mutation_kind,
            expected_key,
            ..
        } => (*mutation_kind, *expected_key),
        LegacySecretRequest::Host { expected_key, .. } => (
            if expected_key.is_some() {
                "host_trust_rotated"
            } else {
                "host_trust_created"
            },
            *expected_key,
        ),
        LegacySecretRequest::Delete {
            mutation_kind,
            expected_key,
        } => (*mutation_kind, Some(*expected_key)),
    };
    if requested_kind != mutation_kind {
        return Ok(false);
    }
    match expected_key {
        None if counter == 0
            && matches!(
                mutation_kind.as_str(),
                "credential_created" | "host_trust_created"
            ) => {}
        Some(expected)
            if prior_key.as_deref() == Some(expected)
                && prior_operation.as_ref().is_some_and(|id| !id.is_empty()) => {}
        _ => return Ok(false),
    }
    let mut hasher = Sha256::new();
    match request {
        LegacySecretRequest::Credential {
            mutation_kind,
            name,
            private_key,
            passphrase,
            ..
        } => {
            hasher.update(b"yoi repository credential operation v1");
            hasher.update(if mutation_kind == "credential_created" {
                "create"
            } else {
                "rotate"
            });
            hasher.update(resource_id.as_bytes());
            hasher.update(name.as_bytes());
            hasher.update(counter.to_be_bytes());
            hasher.update(Sha256::digest(private_key.as_bytes()));
            if let Some(passphrase) = passphrase {
                hasher.update(Sha256::digest(passphrase.as_bytes()));
            }
        }
        LegacySecretRequest::Host {
            hostname,
            port,
            key_fingerprint,
            ..
        } => {
            hasher.update(b"yoi repository host trust operation v1");
            hasher.update(resource_id.as_bytes());
            hasher.update(hostname.as_bytes());
            hasher.update(port.to_be_bytes());
            hasher.update(key_fingerprint.as_bytes());
            hasher.update(counter.to_be_bytes());
        }
        LegacySecretRequest::Delete { mutation_kind, .. } => {
            hasher.update(b"yoi repository secret simple operation v1");
            hasher.update(if mutation_kind == "credential_deleted" {
                "delete_credential"
            } else {
                "delete_host_trust"
            });
            hasher.update(resource_id.as_bytes());
            hasher.update(counter.to_be_bytes());
        }
    }
    Ok(format!("sha256:{}", encode_hex(&hasher.finalize())) == original)
}

fn replay_credential_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    credential_id: &str,
    legacy_request: LegacySecretRequest<'_>,
) -> Result<Option<RepositorySshCredential>> {
    let operation = read_operation(tx, workspace_id, operation_id)?;
    let Some((stored_fingerprint, kind, resource_id, _)) = operation else {
        return Ok(None);
    };
    if kind != "credential"
        || resource_id != credential_id
        || (stored_fingerprint != fingerprint
            && !matches_legacy_secret_replay(
                tx,
                workspace_id,
                operation_id,
                credential_id,
                &stored_fingerprint,
                legacy_request,
            )?)
    {
        return Err(Error::WorkspaceConfigConflict(
            "Repository secret operation id was reused with different input or lacks legacy replay evidence; recover committed status with operation_status, never reexecute".to_string(),
        ));
    }
    read_credential(tx, workspace_id, credential_id)?
        .map(Some)
        .ok_or_else(|| {
            Error::RegistryInconsistency(
                "Repository secret operation is committed but its credential was removed; recover operation_status, never reexecute".to_string(),
            )
        })
}

fn replay_host_operation(
    tx: &rusqlite::Transaction<'_>,
    workspace_id: &str,
    operation_id: &str,
    fingerprint: &str,
    host_trust_id: &str,
    legacy_request: LegacySecretRequest<'_>,
) -> Result<Option<RepositorySshHostTrust>> {
    let operation = read_operation(tx, workspace_id, operation_id)?;
    let Some((stored_fingerprint, kind, resource_id, _)) = operation else {
        return Ok(None);
    };
    if kind != "host_trust"
        || resource_id != host_trust_id
        || (stored_fingerprint != fingerprint
            && !matches_legacy_secret_replay(
                tx,
                workspace_id,
                operation_id,
                host_trust_id,
                &stored_fingerprint,
                legacy_request,
            )?)
    {
        return Err(Error::WorkspaceConfigConflict(
            "Repository host trust operation id was reused with different input or lacks legacy replay evidence; recover committed status with operation_status, never reexecute".to_string(),
        ));
    }
    read_host_trust(tx, workspace_id, host_trust_id)?
        .map(Some)
        .ok_or_else(|| {
            Error::RegistryInconsistency(
                "Repository host trust operation is committed but its resource was removed; recover operation_status, never reexecute".to_string(),
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
    legacy_request: LegacySecretRequest<'_>,
) -> Result<bool> {
    let Some((stored_fingerprint, stored_kind, stored_resource, _)) =
        read_operation(tx, workspace_id, operation_id)?
    else {
        return Ok(false);
    };
    if stored_kind != kind
        || stored_resource != resource_id
        || (stored_fingerprint != fingerprint
            && !matches_legacy_secret_replay(
                tx,
                workspace_id,
                operation_id,
                resource_id,
                &stored_fingerprint,
                legacy_request,
            )?)
    {
        return Err(Error::WorkspaceConfigConflict(
            "Repository secret operation id was reused with different input or lacks legacy replay evidence; recover committed status with operation_status, never reexecute".to_string(),
        ));
    }
    Ok(true)
}

fn read_operation(
    conn: &rusqlite::Connection,
    workspace_id: &str,
    operation_id: &str,
) -> Result<Option<(String, String, String, String)>> {
    conn.query_row(
        r#"SELECT request_fingerprint, resource_kind, resource_id, result_operation_id
           FROM repository_secret_operations
           WHERE workspace_id = ?1 AND operation_id = ?2"#,
        params![workspace_id, operation_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
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
    result_operation_id: &str,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO repository_secret_operations (
            workspace_id, operation_id, request_fingerprint, resource_kind,
            resource_id, result_operation_id, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            workspace_id,
            operation_id,
            fingerprint,
            kind,
            resource_id,
            result_operation_id,
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
    operation_id: &str,
    actor_account_id: &str,
    created_at: &str,
) -> Result<()> {
    tx.execute(
        r#"INSERT INTO repository_secret_audit_events (
            workspace_id, event_id, kind, resource_id, operation_id,
            actor_account_id, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            workspace_id,
            format!("repo-secret-audit-{}", uuid::Uuid::now_v7()),
            kind,
            resource_id,
            operation_id,
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
                  public_key_fingerprint, current_operation_id, status, created_at, rotated_at
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
                  host_key, fingerprint, current_operation_id, created_at, updated_at
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
        .map(|binding| binding.repository_key.clone())
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
        .map(|binding| binding.repository_key.clone())
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

/// Frozen bridge for the parent's one-time legacy schema migration. Ciphertext
/// cannot be copied: its authenticated context included the old numeric identity.
/// Migration requires the existing master key and never creates a replacement.
#[allow(dead_code)]
pub(crate) fn migrate_legacy_repository_secret_envelope(
    database_path: &Path,
    workspace_id: &str,
    credential_id: &str,
    legacy_revision: u64,
    operation_id: &str,
    purpose: &str,
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    let master_bytes =
        zeroize::Zeroizing::new(std::fs::read(master_key_path(database_path)?).map_err(|_| {
            Error::Store("Repository secret migration master key is unavailable".to_string())
        })?);
    let master_key = zeroize::Zeroizing::new(master_key_from_bytes(&master_bytes)?);
    let unbound = UnboundKey::new(&AES_256_GCM, master_key.as_slice())
        .map_err(|_| Error::Store("Repository secret migration key is invalid".to_string()))?;
    let key = LessSafeKey::new(unbound);
    let old_nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| {
        Error::RegistryInconsistency("Repository secret migration nonce is invalid".to_string())
    })?;
    let old_aad = format!(
        "yoi/repository-secret/v1/{workspace_id}/{credential_id}/{legacy_revision}/{purpose}"
    );
    let mut plaintext = zeroize::Zeroizing::new(ciphertext.to_vec());
    let plaintext_len = key
        .open_in_place(
            Nonce::assume_unique_for_key(old_nonce),
            Aad::from(old_aad.as_bytes()),
            plaintext.as_mut_slice(),
        )
        .map_err(|_| Error::Store("Repository secret migration decryption failed".to_string()))?
        .len();
    plaintext.truncate(plaintext_len);
    let mut new_nonce = [0; NONCE_BYTES];
    SystemRandom::new().fill(&mut new_nonce).map_err(|_| {
        Error::Store("Repository secret migration nonce generation failed".to_string())
    })?;
    let new_aad = secret_aad(workspace_id, credential_id, operation_id, purpose);
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(new_nonce),
        Aad::from(new_aad.as_bytes()),
        &mut *plaintext,
    )
    .map_err(|_| Error::Store("Repository secret migration encryption failed".to_string()))?;
    Ok((new_nonce.to_vec(), plaintext.to_vec()))
}

fn master_key_from_bytes(bytes: &[u8]) -> Result<[u8; MASTER_KEY_BYTES]> {
    bytes
        .try_into()
        .map_err(|_| Error::Store("Repository secret master key has an invalid length".to_string()))
}

fn secret_aad(
    workspace_id: &str,
    credential_id: &str,
    operation_id: &str,
    purpose: &str,
) -> String {
    serde_json::to_string(&(
        "yoi/repository-secret/operation",
        workspace_id,
        credential_id,
        operation_id,
        purpose,
    ))
    .expect("string tuple is serializable")
}

fn validate_key_fingerprint(value: &str) -> Result<()> {
    value
        .parse::<ssh_key::Fingerprint>()
        .map_err(|_| Error::InvalidInput("invalid SSH key fingerprint".into()))?;
    Ok(())
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
    expected_key_fingerprint: &str,
    private_key: &str,
    passphrase: Option<&str>,
) -> String {
    operation_fingerprint(&(
        "credential",
        kind,
        credential_id,
        name,
        expected_key_fingerprint,
        encode_hex(&Sha256::digest(private_key.as_bytes())),
        passphrase.map(|value| encode_hex(&Sha256::digest(value.as_bytes()))),
    ))
}

fn host_operation_fingerprint(
    request: &PutRepositorySshHostTrustRequest,
    hostname: &str,
    key_fingerprint: &str,
) -> String {
    operation_fingerprint(&(
        "host_trust",
        request.host_trust_id.trim(),
        hostname,
        request.port,
        key_fingerprint,
        request.expected_fingerprint.as_deref(),
    ))
}

fn simple_operation_fingerprint(
    kind: &str,
    resource_id: &str,
    expected_key_fingerprint: &str,
) -> String {
    operation_fingerprint(&(kind, resource_id, expected_key_fingerprint))
}

fn operation_fingerprint(value: &impl serde::Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("operation identity is serializable");
    format!("sha256:{}", encode_hex(&Sha256::digest(bytes)))
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
    use server_api::{RepositoryObservedStatus, RepositorySource, RepositorySourceKind};
    use ssh_key::private::Ed25519Keypair;

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
                owner_account_id: "owner-account".to_string(),
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
    fn workspace_config_projection_rejects_legacy_plain_http_repository_source() {
        let source: RepositorySource = serde_json::from_value(serde_json::json!({
            "kind": "http",
            "uri": "http://git.example.test/team/project.git",
        }))
        .unwrap();

        let error = validate_repository_access_source("remote", &source).unwrap_err();
        assert!(error.to_string().contains("unsupported plain HTTP"));
        assert!(error.to_string().contains("HTTPS or SSH"));

        let mismatched = server_api::RepositorySource {
            kind: server_api::RepositorySourceKind::Https,
            uri: "http://git.example.test/team/project.git".to_string(),
        };
        let error = validate_repository_access_source("remote", &mismatched).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("repository_source_plain_http_unsupported")
        );
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
    fn workspace_default_credential_is_generated_once_and_immutable() {
        let (_dir, _store, service) = test_service();

        let created = service
            .ensure_workspace_default_credential("workspace-a")
            .unwrap();
        let replayed = service
            .ensure_workspace_default_credential("workspace-a")
            .unwrap();
        let public_key = service
            .credential_public_key(
                "workspace-a",
                WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
            )
            .unwrap()
            .unwrap();

        assert_eq!(created, replayed);
        assert_eq!(
            public_key.public_key_fingerprint,
            created.public_key_fingerprint
        );
        assert!(
            service
                .rotate_credential(
                    "workspace-a",
                    WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
                    RotateRepositorySshCredentialRequest {
                        operation_id: "rotate-default".to_string(),
                        expected_public_key_fingerprint: created.public_key_fingerprint.clone(),
                        private_key: test_private_key(12).0,
                        passphrase: None,
                    },
                    "owner-a",
                )
                .is_err()
        );
        assert!(
            service
                .delete_credential(
                    "workspace-a",
                    WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
                    DeleteRepositorySshCredentialRequest {
                        operation_id: "delete-default".to_string(),
                        expected_public_key_fingerprint: created.public_key_fingerprint.clone(),
                    },
                    "owner-a",
                    &RepositoryAccessProjection {
                        workspace_id: "workspace-a".to_string(),
                        projection_digest: "sha256:empty".to_string(),
                        bindings: Vec::new(),
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn generated_credential_is_replayable_and_exposes_only_its_public_key() {
        let (_dir, _store, service) = test_service();
        let request = GenerateRepositorySshCredentialRequest {
            operation_id: "generate-one".to_string(),
            credential_id: "workspace-key".to_string(),
            name: "Workspace key".to_string(),
        };

        let created = service
            .generate_credential("workspace-a", request.clone(), "owner-a")
            .unwrap();
        let replayed = service
            .generate_credential("workspace-a", request, "owner-a")
            .unwrap();
        let public_key = service
            .credential_public_key("workspace-a", "workspace-key")
            .unwrap()
            .unwrap();

        assert_eq!(replayed, created);
        assert_eq!(
            public_key.public_key_fingerprint,
            created.public_key_fingerprint
        );
        assert!(public_key.public_key.starts_with("ssh-ed25519 "));
        assert!(!public_key.public_key.contains("PRIVATE KEY"));
        assert!(
            service
                .credential_public_key("workspace-b", "workspace-key")
                .unwrap()
                .is_none()
        );
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
                    "SELECT nonce, ciphertext FROM server_secret_objects WHERE workspace_id = 'workspace-a' AND secret_id = 'deploy-main' AND operation_id = 'create-one' AND purpose = 'private_key'",
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
                    expected_public_key_fingerprint: created.public_key_fingerprint.clone(),
                    private_key: rotated_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        assert_ne!(
            rotated.public_key_fingerprint,
            created.public_key_fingerprint
        );
    }

    #[test]
    fn pinned_host_trust_has_stable_key_identity_across_operations() {
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
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();
        assert_eq!(created.hostname, "github.com");
        let updated = service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-update".to_string(),
                    host_trust_id: "github".to_string(),
                    hostname: "github.com".to_string(),
                    port: 22,
                    host_key: public_key,
                    expected_fingerprint: Some(created.fingerprint.clone()),
                },
                "owner-a",
            )
            .unwrap();
        assert_eq!(updated.fingerprint, created.fingerprint);
    }

    fn config_state(source: &str) -> WorkspaceConfigState {
        let path = VirtualPath::parse("main.dcdl").unwrap();
        let entry = ConfigEntry::new(path.clone(), ConfigContentType::Decodal, source).unwrap();
        let snapshot = ConfigTreeSnapshot::from_entries(vec![entry]).unwrap();
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
                repository_key: "remote".to_string(),
                kind: "git".to_string(),
                provider: Some("git".to_string()),
                source,
                default_ref: Some("main".to_string()),
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
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();
        let state = config_state(
            r#"{
                repository_access = {
                    remote = {
                        ssh = {
                            credential = "deploy";
                            host_trust = "example";
                            access = "read_only";
                        };
                    };
                };
            } as WorkspaceConfigSchema"#,
        );
        let projection =
            project_repository_access_state(&*store, &service, "workspace-a", &state).unwrap();
        assert_eq!(projection.bindings.len(), 1);
        assert_eq!(projection.bindings[0].repository_key, "remote");
        assert_eq!(
            projection.bindings[0].access,
            RepositoryAccessMode::ReadOnly
        );
        let lease = service
            .lease_ssh_materialization_access("workspace-a", &projection.bindings[0])
            .unwrap();
        assert!(lease.private_key.contains("BEGIN OPENSSH PRIVATE KEY"));
        assert!(
            lease
                .known_hosts_entry
                .starts_with("example.test ssh-ed25519 ")
        );
        let exact = service
            .lease_ssh_materialization_access_fingerprints(
                "workspace-a",
                "deploy",
                &lease.credential_fingerprint,
                "example",
                &lease.host_trust_fingerprint,
            )
            .unwrap();
        assert_eq!(exact.credential_fingerprint, lease.credential_fingerprint);
        assert_eq!(exact.host_trust_fingerprint, lease.host_trust_fingerprint);
        assert_eq!(exact.known_hosts_entry, lease.known_hosts_entry);

        let unknown = config_state(
            r#"{
                repository_access = {
                    remote = {
                        ssh = {
                            credential = "missing";
                            host_trust = "example";
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
    fn default_binding_is_read_write_for_unique_host_trust_and_ssh_sources() {
        let (_dir, _store, service) = test_service();
        assert!(
            service
                .default_ssh_binding_for_repository(
                    "workspace-a",
                    "main",
                    "git@example.test:org/main.git",
                )
                .unwrap()
                .is_none()
        );
        let (_, host_key) = test_private_key(10);
        service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-default".to_string(),
                    host_trust_id: "example".to_string(),
                    hostname: "example.test".to_string(),
                    port: 22,
                    host_key,
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();

        for uri in [
            "ssh://git@example.test/org/main.git",
            "git@example.test:org/main.git",
        ] {
            let binding = service
                .default_ssh_binding_for_repository("workspace-a", "main", uri)
                .unwrap()
                .unwrap();
            assert_eq!(
                binding.credential_id,
                WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID
            );
            assert_eq!(binding.host_trust_id, "example");
            assert_eq!(binding.access, RepositoryAccessMode::ReadWrite);
        }
        assert!(
            service
                .credential_public_key(
                    "workspace-a",
                    WORKSPACE_DEFAULT_REPOSITORY_SSH_CREDENTIAL_ID,
                )
                .unwrap()
                .is_some()
        );

        let (_, second_host_key) = test_private_key(11);
        service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "host-default-second".to_string(),
                    host_trust_id: "example-second".to_string(),
                    hostname: "example.test".to_string(),
                    port: 22,
                    host_key: second_host_key,
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();
        assert!(
            service
                .default_ssh_binding_for_repository(
                    "workspace-a",
                    "main",
                    "git@example.test:org/main.git",
                )
                .is_err()
        );
    }

    #[test]
    fn retryable_workdir_create_retains_candidate_key_until_success() {
        let (_dir, store, service) = test_service();
        let (private_key, _) = test_private_key(13);
        let credential = service
            .create_credential(
                "workspace-a",
                CreateRepositorySshCredentialRequest {
                    operation_id: "create-retained".to_string(),
                    credential_id: "retained-deploy".to_string(),
                    name: "Retained deploy".to_string(),
                    private_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        let operation = crate::store::WorkdirCreateOperationRecord {
            workspace_id: "workspace-a".to_string(),
            operation_id: "create-workdir-retained".to_string(),
            request_fingerprint: "sha256:request".to_string(),
            repository_id: "repo-a".to_string(),
            selector: Some("develop".to_string()),
            requested_runtime_id: Some("runtime-a".to_string()),
            resolved_runtime_id: "runtime-a".to_string(),
            config_projection_digest: "sha256:projection".to_string(),
            source_kind: Some("ssh".to_string()),
            source_uri: Some("ssh://git@example.test/org/main.git".to_string()),
            source_fingerprint: Some("sha256:source".to_string()),
            credential_id: None,
            credential_fingerprint: None,
            host_trust_id: None,
            host_trust_fingerprint: None,
            repository_access_mode: None,
            credential_candidates: Vec::new(),
            working_directory_id: "workdir-retained".to_string(),
            state: "pending".to_string(),
            failure: None,
            created_at: "2026-08-24T00:00:00Z".to_string(),
            updated_at: "2026-08-24T00:00:00Z".to_string(),
        };
        store.reserve_workdir_create_operation(&operation).unwrap();
        let candidates = vec![crate::store::WorkdirCreateCredentialCandidate {
            role: crate::store::WorkdirCreateCredentialCandidateRole::Primary,
            credential_id: "retained-deploy".to_string(),
            credential_fingerprint: credential.public_key_fingerprint.clone(),
        }];
        store
            .bind_workdir_create_repository_access(
                "workspace-a",
                "create-workdir-retained",
                "sha256:request",
                "retained-deploy",
                &credential.public_key_fingerprint,
                "host-a",
                "host-created",
                "read_only",
                &candidates,
                "2026-08-24T00:00:01Z",
            )
            .unwrap();
        let projection = RepositoryAccessProjection {
            workspace_id: "workspace-a".to_string(),
            projection_digest: "sha256:empty".to_string(),
            bindings: Vec::new(),
        };

        let retained = service
            .delete_credential(
                "workspace-a",
                "retained-deploy",
                DeleteRepositorySshCredentialRequest {
                    operation_id: "delete-retained".to_string(),
                    expected_public_key_fingerprint: credential.public_key_fingerprint.clone(),
                },
                "owner-a",
                &projection,
            )
            .unwrap_err();
        assert!(matches!(retained, Error::RepositoryConflict(_)));

        store
            .finish_workdir_create_operation(
                "workspace-a",
                "create-workdir-retained",
                "sha256:request",
                true,
                None,
                "2026-08-24T00:00:02Z",
            )
            .unwrap();
        service
            .delete_credential(
                "workspace-a",
                "retained-deploy",
                DeleteRepositorySshCredentialRequest {
                    operation_id: "delete-released".to_string(),
                    expected_public_key_fingerprint: credential.public_key_fingerprint.clone(),
                },
                "owner-a",
                &projection,
            )
            .unwrap();
    }

    fn seed_lease_binding(
        service: &RepositorySecretService,
    ) -> (
        RepositorySshCredential,
        RepositorySshHostTrust,
        RepositorySshAccessBinding,
    ) {
        let (private_key, host_key) = test_private_key(21);
        let credential = service
            .create_credential(
                "workspace-a",
                CreateRepositorySshCredentialRequest {
                    operation_id: "key-create".into(),
                    credential_id: "deploy".into(),
                    name: "Deploy".into(),
                    private_key,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        let host = service
            .put_host_trust(
                "workspace-a",
                PutRepositorySshHostTrustRequest {
                    operation_id: "trust-create".into(),
                    host_trust_id: "host".into(),
                    hostname: "old.example.test".into(),
                    port: 22,
                    host_key,
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();
        let binding = RepositorySshAccessBinding {
            repository_key: "remote".into(),
            credential_id: "deploy".into(),
            host_trust_id: "host".into(),
            access: RepositoryAccessMode::ReadOnly,
        };
        (credential, host, binding)
    }

    #[test]
    fn same_key_reseal_keeps_key_precondition_and_replay_does_not_revert_rotation() {
        let (_dir, store, service) = test_service();
        let (credential, _host, binding) = seed_lease_binding(&service);
        let reseal = RotateRepositorySshCredentialRequest {
            operation_id: "key-reseal".into(),
            expected_public_key_fingerprint: credential.public_key_fingerprint.clone(),
            private_key: test_private_key(21).0,
            passphrase: None,
        };
        let resealed = service
            .rotate_credential("workspace-a", "deploy", reseal.clone(), "owner-a")
            .unwrap();
        assert_eq!(
            resealed.public_key_fingerprint,
            credential.public_key_fingerprint
        );
        assert_eq!(
            service
                .rotate_credential("workspace-a", "deploy", reseal.clone(), "owner-a")
                .unwrap(),
            resealed
        );
        let changed = service
            .rotate_credential(
                "workspace-a",
                "deploy",
                RotateRepositorySshCredentialRequest {
                    operation_id: "key-change".into(),
                    expected_public_key_fingerprint: credential.public_key_fingerprint.clone(),
                    private_key: test_private_key(22).0,
                    passphrase: None,
                },
                "owner-a",
            )
            .unwrap();
        assert_ne!(
            changed.public_key_fingerprint,
            credential.public_key_fingerprint
        );
        let replayed = service
            .rotate_credential("workspace-a", "deploy", reseal.clone(), "owner-a")
            .unwrap();
        assert_eq!(
            replayed.public_key_fingerprint,
            changed.public_key_fingerprint
        );
        let stale = RotateRepositorySshCredentialRequest {
            operation_id: "key-stale".into(),
            ..reseal.clone()
        };
        assert!(matches!(
            service.rotate_credential("workspace-a", "deploy", stale, "owner-a"),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        let reused = RotateRepositorySshCredentialRequest {
            private_key: test_private_key(23).0,
            ..reseal
        };
        assert!(matches!(
            service.rotate_credential("workspace-a", "deploy", reused, "owner-a"),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        let historical = service
            .lease_ssh_materialization_access_operation(
                "workspace-a",
                "deploy",
                "key-create",
                "host",
                "trust-create",
            )
            .unwrap();
        assert_eq!(
            historical.credential_fingerprint,
            credential.public_key_fingerprint
        );
        assert_eq!(
            service
                .lease_ssh_materialization_access("workspace-a", &binding)
                .unwrap()
                .credential_fingerprint,
            changed.public_key_fingerprint
        );
        store.with_conn(|conn| {
            assert_eq!(conn.query_row("SELECT count(*) FROM repository_ssh_credential_keys WHERE workspace_id='workspace-a' AND credential_id='deploy'", [], |r| r.get::<_, i64>(0))?, 3);
            assert_eq!(conn.query_row("SELECT count(*) FROM repository_secret_operations WHERE operation_id='key-stale'", [], |r| r.get::<_, i64>(0))?, 0);
            assert_eq!(conn.query_row("SELECT count(*) FROM repository_secret_audit_events WHERE kind='credential_rotated'", [], |r| r.get::<_, i64>(0))?, 2);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn same_host_key_endpoint_change_allows_current_lease_but_rejects_ambiguous_history() {
        let (_dir, _store, service) = test_service();
        let (credential, host, binding) = seed_lease_binding(&service);
        let old_lease = service
            .lease_ssh_materialization_access("workspace-a", &binding)
            .unwrap();
        let request = PutRepositorySshHostTrustRequest {
            operation_id: "trust-move".into(),
            host_trust_id: "host".into(),
            hostname: "new.example.test".into(),
            port: 2222,
            host_key: test_private_key(21).1,
            expected_fingerprint: Some(host.fingerprint.clone()),
        };
        let changed = service
            .put_host_trust("workspace-a", request.clone(), "owner-a")
            .unwrap();
        assert_eq!(changed.fingerprint, host.fingerprint);
        assert_eq!(
            service
                .put_host_trust("workspace-a", request.clone(), "owner-a")
                .unwrap(),
            changed
        );
        let current = service
            .lease_ssh_materialization_access("workspace-a", &binding)
            .unwrap();
        assert!(
            current
                .known_hosts_entry
                .starts_with("[new.example.test]:2222 ")
        );
        let historical = service.lease_ssh_materialization_access_fingerprints(
            "workspace-a",
            "deploy",
            &credential.public_key_fingerprint,
            "host",
            &host.fingerprint,
        );
        assert!(
            matches!(historical, Err(Error::RegistryInconsistency(message)) if message.contains("ambiguous endpoint"))
        );
        let exact = service
            .lease_ssh_materialization_access_operation(
                "workspace-a",
                "deploy",
                "key-create",
                "host",
                "trust-create",
            )
            .unwrap();
        assert_eq!(exact.known_hosts_entry, old_lease.known_hosts_entry);
        let changed_input = PutRepositorySshHostTrustRequest {
            hostname: "other.example.test".into(),
            ..request
        };
        assert!(matches!(
            service.put_host_trust("workspace-a", changed_input, "owner-a"),
            Err(Error::WorkspaceConfigConflict(_))
        ));
        assert!(
            service
                .lease_ssh_materialization_access("workspace-b", &binding)
                .is_err()
        );
    }

    #[test]
    fn inactive_credential_cannot_be_leased_by_current_or_historical_identity() {
        let (_dir, store, service) = test_service();
        let (credential, host, binding) = seed_lease_binding(&service);
        store.with_conn(|conn| {
            conn.execute("UPDATE repository_ssh_credentials SET status='revoked' WHERE workspace_id='workspace-a' AND credential_id='deploy'", [])?;
            Ok(())
        }).unwrap();
        assert!(
            service
                .lease_ssh_materialization_access("workspace-a", &binding)
                .is_err()
        );
        assert!(
            service
                .lease_ssh_materialization_access_fingerprints(
                    "workspace-a",
                    "deploy",
                    &credential.public_key_fingerprint,
                    "host",
                    &host.fingerprint
                )
                .is_err()
        );
        assert!(
            service
                .lease_ssh_materialization_access_operation(
                    "workspace-a",
                    "deploy",
                    "key-create",
                    "host",
                    "trust-create"
                )
                .is_err()
        );
    }

    #[test]
    fn envelopes_authenticate_operation_workspace_resource_and_purpose() {
        let (_dir, _store, service) = test_service();
        for (workspace, credential, operation, purpose) in [
            ("workspace-b", "deploy", "create", "private_key"),
            ("workspace-a", "other", "create", "private_key"),
            ("workspace-a", "deploy", "other", "private_key"),
            ("workspace-a", "deploy", "create", "passphrase"),
        ] {
            let secret = service
                .seal("workspace-a", "deploy", "create", "private_key", b"secret")
                .unwrap();
            assert!(
                service
                    .unseal(workspace, credential, operation, purpose, secret)
                    .is_err()
            );
        }
        let secret = service
            .seal("workspace-a", "deploy", "create", "private_key", b"secret")
            .unwrap();
        assert_eq!(
            service
                .unseal("workspace-a", "deploy", "create", "private_key", secret)
                .unwrap(),
            b"secret"
        );
        assert_ne!(
            secret_aad("a/b", "c", "op", "private_key"),
            secret_aad("a", "b/c", "op", "private_key")
        );
    }

    #[test]
    fn operation_fingerprints_preserve_field_boundaries_and_preconditions() {
        assert_ne!(
            credential_fingerprint("create", "ab", "c", "", "key", None),
            credential_fingerprint("create", "a", "bc", "", "key", None)
        );
        assert_ne!(
            credential_fingerprint("rotate", "deploy", "", "first", "key", None),
            credential_fingerprint("rotate", "deploy", "", "second", "key", None)
        );
        assert_ne!(
            simple_operation_fingerprint("delete", "ab", "c"),
            simple_operation_fingerprint("delete", "a", "bc")
        );
    }

    #[test]
    fn frozen_legacy_envelope_migration_rebinds_authenticated_context() {
        let (dir, _store, service) = test_service();
        let master = service.master_key.as_ref().unwrap();
        let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, master.as_slice()).unwrap());
        let nonce = [42; NONCE_BYTES];
        let mut ciphertext = b"legacy-secret".to_vec();
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(b"yoi/repository-secret/v1/workspace-a/deploy/7/private_key"),
            &mut ciphertext,
        )
        .unwrap();
        let (new_nonce, ciphertext) = migrate_legacy_repository_secret_envelope(
            &dir.path().join("server.db"),
            "workspace-a",
            "deploy",
            7,
            "migrated-operation",
            "private_key",
            &nonce,
            &ciphertext,
        )
        .unwrap();
        let secret = SealedSecret {
            nonce: new_nonce.try_into().unwrap(),
            ciphertext,
        };
        assert_eq!(
            service
                .unseal(
                    "workspace-a",
                    "deploy",
                    "migrated-operation",
                    "private_key",
                    secret
                )
                .unwrap(),
            b"legacy-secret"
        );
    }

    #[test]
    fn delete_operations_are_replayable_but_stale_or_reused_input_conflicts() {
        let (_dir, _store, service) = test_service();
        let credential = service
            .generate_credential(
                "workspace-a",
                GenerateRepositorySshCredentialRequest {
                    operation_id: "create-deleted".into(),
                    credential_id: "deploy".into(),
                    name: "Deploy".into(),
                },
                "owner-a",
            )
            .unwrap();
        let projection = RepositoryAccessProjection {
            workspace_id: "workspace-a".into(),
            projection_digest: "sha256:empty".into(),
            bindings: Vec::new(),
        };
        let stale = DeleteRepositorySshCredentialRequest {
            operation_id: "stale-delete".into(),
            expected_public_key_fingerprint: parse_private_key(&test_private_key(91).0, None)
                .unwrap()
                .fingerprint,
        };
        assert!(matches!(
            service
                .delete_credential("workspace-a", "deploy", stale, "owner-a", &projection)
                .unwrap_err(),
            Error::WorkspaceConfigConflict(_)
        ));
        let request = DeleteRepositorySshCredentialRequest {
            operation_id: "delete-key".into(),
            expected_public_key_fingerprint: credential.public_key_fingerprint,
        };
        service
            .delete_credential(
                "workspace-a",
                "deploy",
                request.clone(),
                "owner-a",
                &projection,
            )
            .unwrap();
        service
            .delete_credential(
                "workspace-a",
                "deploy",
                request.clone(),
                "owner-a",
                &projection,
            )
            .unwrap();
        let changed = DeleteRepositorySshCredentialRequest {
            expected_public_key_fingerprint: parse_private_key(&test_private_key(92).0, None)
                .unwrap()
                .fingerprint,
            ..request
        };
        assert!(matches!(
            service
                .delete_credential("workspace-a", "deploy", changed, "owner-a", &projection)
                .unwrap_err(),
            Error::WorkspaceConfigConflict(_)
        ));
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
                    expected_fingerprint: None,
                },
                "owner-a",
            )
            .unwrap();
        let projection = RepositoryAccessProjection {
            workspace_id: "workspace-a".to_string(),
            projection_digest: "sha256:test".to_string(),
            bindings: vec![RepositorySshAccessBinding {
                repository_key: "main".to_string(),
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
                    expected_public_key_fingerprint: parse_private_key(
                        &test_private_key(10).0,
                        None,
                    )
                    .unwrap()
                    .fingerprint,
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
