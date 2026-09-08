use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use std::sync::Arc;

use chrono::{SecondsFormat, Utc};
use ring::signature::KeyPair;
use serde::{Deserialize, Serialize};
use worker_runtime::auth::{RuntimeIdentityMaterial, encode_public_key};
use worker_runtime::workspace_issuer::{
    WorkspaceCapabilityClaims, WorkspaceCapabilityVerificationError,
    assemble_workspace_capability_token, workspace_capability_signing_input,
};
use zeroize::Zeroize;

use crate::store::{
    ControlPlaneStore, WorkspaceSigningIdentityActivation,
    WorkspaceSigningIdentityProvisioningOperation, WorkspaceSigningIdentityRecord,
};
use crate::{Error, Result};

pub const WORKSPACE_SIGNING_ALGORITHM: &str = "ed25519";
pub const WORKSPACE_SIGNING_IDENTITY_REVISION: u64 = 1;
const MATERIAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSigningPrivateMaterial {
    version: u32,
    workspace_id: String,
    key_id: String,
    revision: u64,
    private_key: String,
}

impl fmt::Debug for WorkspaceSigningPrivateMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkspaceSigningPrivateMaterial")
            .field("version", &self.version)
            .field("workspace_id", &self.workspace_id)
            .field("key_id", &self.key_id)
            .field("revision", &self.revision)
            .field("private_key", &"[REDACTED]")
            .finish()
    }
}

impl Drop for WorkspaceSigningPrivateMaterial {
    fn drop(&mut self) {
        self.private_key.zeroize();
    }
}

impl WorkspaceSigningPrivateMaterial {
    pub fn generate(workspace_id: &str, key_id: &str) -> Result<Self> {
        let material = RuntimeIdentityMaterial::generate(key_id.to_string()).map_err(|error| {
            identity_error(
                "workspace_signing_identity_generation_failed",
                format!("failed to generate Workspace signing identity: {error}"),
            )
        })?;
        Ok(Self {
            version: MATERIAL_SCHEMA_VERSION,
            workspace_id: workspace_id.to_string(),
            key_id: key_id.to_string(),
            revision: WORKSPACE_SIGNING_IDENTITY_REVISION,
            private_key: material.private_key,
        })
    }

    fn signing_key(
        &self,
        expected_workspace_id: &str,
        expected_key_id: &str,
        expected_revision: u64,
    ) -> Result<ring::signature::Ed25519KeyPair> {
        if self.version != MATERIAL_SCHEMA_VERSION
            || self.workspace_id != expected_workspace_id
            || self.key_id != expected_key_id
            || self.revision != expected_revision
        {
            return Err(identity_error(
                "workspace_signing_identity_material_mismatch",
                "Workspace signing private material does not match its persisted metadata",
            ));
        }
        RuntimeIdentityMaterial {
            identity_id: self.key_id.clone(),
            public_key: String::new(),
            private_key: self.private_key.clone(),
        }
        .signing_key()
        .map_err(|_| {
            identity_error(
                "workspace_signing_identity_material_corrupt",
                "Workspace signing private material is corrupt",
            )
        })
    }

    pub fn validate_and_public_key(
        &self,
        expected_workspace_id: &str,
        expected_key_id: &str,
        expected_revision: u64,
    ) -> Result<String> {
        let signing_key =
            self.signing_key(expected_workspace_id, expected_key_id, expected_revision)?;
        Ok(encode_public_key(signing_key.public_key().as_ref()))
    }
}

pub trait WorkspaceSigningMaterialStore: Send + Sync {
    fn load(&self, material_ref: &str) -> Result<Option<WorkspaceSigningPrivateMaterial>>;
    fn put_if_absent(
        &self,
        material_ref: &str,
        material: &WorkspaceSigningPrivateMaterial,
    ) -> Result<WorkspaceSigningPrivateMaterial>;
    fn delete(&self, material_ref: &str) -> Result<()>;
}

#[derive(Clone)]
pub struct WorkspaceSigningIdentityService {
    store: Arc<dyn ControlPlaneStore>,
    materials: Arc<dyn WorkspaceSigningMaterialStore>,
}

impl WorkspaceSigningIdentityService {
    pub fn new(
        store: Arc<dyn ControlPlaneStore>,
        materials: Arc<dyn WorkspaceSigningMaterialStore>,
    ) -> Self {
        Self { store, materials }
    }

    pub fn prepare_workspace_creation(
        &self,
        workspace_create_operation_key: &str,
        request_fingerprint: &str,
        proposed_workspace_id: &str,
        actor_account_id: &str,
    ) -> Result<(WorkspaceSigningIdentityActivation, String)> {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let proposed_key_id = format!("WK-{}", uuid::Uuid::now_v7().simple());
        let proposed_material_ref = format!("workspace-signing/{proposed_workspace_id}/ed25519-v1");
        let operation_key = format!("workspace-create:{workspace_create_operation_key}");
        let operation = self.store.reserve_workspace_signing_identity_provisioning(
            &WorkspaceSigningIdentityProvisioningOperation {
                operation_key: operation_key.clone(),
                request_fingerprint: request_fingerprint.to_string(),
                operation_kind: "workspace_create".to_string(),
                workspace_id: proposed_workspace_id.to_string(),
                key_id: proposed_key_id,
                private_material_ref: proposed_material_ref,
                revision: WORKSPACE_SIGNING_IDENTITY_REVISION,
                actor_account_id: actor_account_id.to_string(),
                state: "pending".to_string(),
                created_at: now,
                completed_at: None,
            },
        )?;
        let activation = self.prepare_material(&operation)?;
        Ok((activation, operation.operation_key))
    }

    pub fn provision_existing(
        &self,
        workspace_id: &str,
        actor_account_id: &str,
    ) -> Result<WorkspaceSigningIdentityRecord> {
        let identity = self
            .store
            .get_workspace_signing_identity(workspace_id)?
            .ok_or_else(|| {
                identity_error(
                    "workspace_signing_identity_metadata_missing",
                    "Workspace signing identity metadata is missing",
                )
            })?;
        if identity.state == "active" {
            self.validate_active_material(&identity)?;
            return Ok(identity);
        }
        if identity.state != "pending_provisioning" {
            return Err(identity_error(
                "workspace_signing_identity_state_invalid",
                "Workspace signing identity state is invalid",
            ));
        }
        let operation_key = format!(
            "existing-workspace:{workspace_id}:revision-{}",
            identity.revision
        );
        let request_fingerprint =
            provisioning_fingerprint(workspace_id, &identity.key_id, identity.revision);
        let operation = self.store.reserve_workspace_signing_identity_provisioning(
            &WorkspaceSigningIdentityProvisioningOperation {
                operation_key: operation_key.clone(),
                request_fingerprint,
                operation_kind: "existing_workspace".to_string(),
                workspace_id: workspace_id.to_string(),
                key_id: identity.key_id.clone(),
                private_material_ref: identity.private_material_ref.clone(),
                revision: identity.revision,
                actor_account_id: actor_account_id.to_string(),
                state: "pending".to_string(),
                created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                completed_at: None,
            },
        )?;
        let activation = self.prepare_material(&operation)?;
        self.store.activate_workspace_signing_identity(
            &activation,
            &operation.operation_key,
            &operation.actor_account_id,
        )
    }

    pub fn get_validated(&self, workspace_id: &str) -> Result<WorkspaceSigningIdentityRecord> {
        let identity = self
            .store
            .get_workspace_signing_identity(workspace_id)?
            .ok_or_else(|| {
                identity_error(
                    "workspace_signing_identity_metadata_missing",
                    "Workspace signing identity metadata is missing",
                )
            })?;
        if identity.state == "active" {
            self.validate_active_material(&identity)?;
        }
        Ok(identity)
    }

    pub fn sign(&self, workspace_id: &str, payload: &[u8]) -> Result<Vec<u8>> {
        let identity = self.get_validated(workspace_id)?;
        if identity.state != "active" {
            return Err(identity_error(
                "workspace_signing_identity_not_provisioned",
                "Workspace signing identity is not provisioned",
            ));
        }
        let material = self
            .materials
            .load(&identity.private_material_ref)?
            .ok_or_else(|| {
                identity_error(
                    "workspace_signing_identity_material_missing",
                    "Workspace signing private material is missing",
                )
            })?;
        let signing_key =
            material.signing_key(workspace_id, &identity.key_id, identity.revision)?;
        Ok(signing_key.sign(payload).as_ref().to_vec())
    }

    pub fn issue_workspace_capability(
        &self,
        workspace_id: &str,
        claims: &WorkspaceCapabilityClaims,
    ) -> Result<String> {
        let input = workspace_capability_signing_input(claims).map_err(capability_error)?;
        let signature = self.sign(workspace_id, input.bytes())?;
        assemble_workspace_capability_token(input, &signature).map_err(capability_error)
    }

    pub fn delete_material(&self, workspace_id: &str) -> Result<()> {
        if let Some(identity) = self.store.get_workspace_signing_identity(workspace_id)? {
            self.materials.delete(&identity.private_material_ref)?;
        }
        Ok(())
    }

    fn prepare_material(
        &self,
        operation: &WorkspaceSigningIdentityProvisioningOperation,
    ) -> Result<WorkspaceSigningIdentityActivation> {
        let material = match self.materials.load(&operation.private_material_ref)? {
            Some(material) => material,
            None => {
                let generated = WorkspaceSigningPrivateMaterial::generate(
                    &operation.workspace_id,
                    &operation.key_id,
                )?;
                self.materials
                    .put_if_absent(&operation.private_material_ref, &generated)?
            }
        };
        let public_key = material.validate_and_public_key(
            &operation.workspace_id,
            &operation.key_id,
            operation.revision,
        )?;
        let public_key_fingerprint = public_key_fingerprint(&public_key)?;
        Ok(WorkspaceSigningIdentityActivation {
            workspace_id: operation.workspace_id.clone(),
            key_id: operation.key_id.clone(),
            public_key,
            public_key_fingerprint,
            private_material_ref: operation.private_material_ref.clone(),
            revision: operation.revision,
            provisioned_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        })
    }

    fn validate_active_material(&self, identity: &WorkspaceSigningIdentityRecord) -> Result<()> {
        let material = self
            .materials
            .load(&identity.private_material_ref)?
            .ok_or_else(|| {
                identity_error(
                    "workspace_signing_identity_material_missing",
                    "Workspace signing private material is missing",
                )
            })?;
        let public_key = material.validate_and_public_key(
            &identity.workspace_id,
            &identity.key_id,
            identity.revision,
        )?;
        let fingerprint = public_key_fingerprint(&public_key)?;
        if identity.public_key.as_deref() != Some(public_key.as_str())
            || identity.public_key_fingerprint.as_deref() != Some(fingerprint.as_str())
        {
            return Err(identity_error(
                "workspace_signing_identity_material_mismatch",
                "Workspace signing private material does not match public metadata",
            ));
        }
        Ok(())
    }
}

fn provisioning_fingerprint(workspace_id: &str, key_id: &str, revision: u64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(workspace_id.as_bytes());
    hasher.update([0]);
    hasher.update(key_id.as_bytes());
    hasher.update([0]);
    hasher.update(revision.to_be_bytes());
    format!("sha256:{}", hex_lower(&hasher.finalize()))
}

#[derive(Default)]
pub struct InMemoryWorkspaceSigningMaterialStore {
    materials: std::sync::Mutex<std::collections::HashMap<String, WorkspaceSigningPrivateMaterial>>,
}

impl WorkspaceSigningMaterialStore for InMemoryWorkspaceSigningMaterialStore {
    fn load(&self, material_ref: &str) -> Result<Option<WorkspaceSigningPrivateMaterial>> {
        Ok(self
            .materials
            .lock()
            .expect("identity material store lock")
            .get(material_ref)
            .cloned())
    }

    fn put_if_absent(
        &self,
        material_ref: &str,
        material: &WorkspaceSigningPrivateMaterial,
    ) -> Result<WorkspaceSigningPrivateMaterial> {
        let mut materials = self.materials.lock().expect("identity material store lock");
        Ok(materials
            .entry(material_ref.to_string())
            .or_insert_with(|| material.clone())
            .clone())
    }

    fn delete(&self, material_ref: &str) -> Result<()> {
        self.materials
            .lock()
            .expect("identity material store lock")
            .remove(material_ref);
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct FsWorkspaceSigningMaterialStore {
    root: PathBuf,
}

impl FsWorkspaceSigningMaterialStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn material_path(&self, material_ref: &str) -> Result<PathBuf> {
        let relative = Path::new(material_ref);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative.components().any(|component| {
                !matches!(component, Component::Normal(_))
                    || component.as_os_str().to_string_lossy().starts_with('.')
            })
        {
            return Err(identity_error(
                "workspace_signing_identity_material_ref_invalid",
                "Workspace signing private material reference is invalid",
            ));
        }
        Ok(self.root.join(relative).with_extension("json"))
    }
}

impl WorkspaceSigningMaterialStore for FsWorkspaceSigningMaterialStore {
    fn load(&self, material_ref: &str) -> Result<Option<WorkspaceSigningPrivateMaterial>> {
        let path = self.material_path(material_ref)?;
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(material_io_error("read", error)),
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|_| {
            identity_error(
                "workspace_signing_identity_material_corrupt",
                "Workspace signing private material is corrupt",
            )
        })
    }

    fn put_if_absent(
        &self,
        material_ref: &str,
        material: &WorkspaceSigningPrivateMaterial,
    ) -> Result<WorkspaceSigningPrivateMaterial> {
        let path = self.material_path(material_ref)?;
        let parent = path.parent().ok_or_else(|| {
            identity_error(
                "workspace_signing_identity_material_ref_invalid",
                "Workspace signing private material reference has no parent",
            )
        })?;
        ensure_private_tree(&self.root, parent)?;

        let mut bytes = serde_json::to_vec(material).map_err(|_| {
            identity_error(
                "workspace_signing_identity_material_encode_failed",
                "Workspace signing private material could not be encoded",
            )
        })?;
        let temporary = parent.join(format!(
            ".workspace-signing-{}.tmp",
            uuid::Uuid::now_v7().simple()
        ));
        let write_result = (|| -> Result<()> {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temporary)
                .map_err(|error| material_io_error("create", error))?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|error| material_io_error("write", error))?;
            match fs::hard_link(&temporary, &path) {
                Ok(()) => sync_directory(parent),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(material_io_error("publish", error)),
            }
        })();
        bytes.zeroize();
        let cleanup_result = match fs::remove_file(&temporary) {
            Ok(()) => sync_directory(parent),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(material_io_error("remove temporary", error)),
        };
        write_result?;
        cleanup_result?;
        self.load(material_ref)?.ok_or_else(|| {
            identity_error(
                "workspace_signing_identity_material_missing",
                "Workspace signing private material is missing after publication",
            )
        })
    }

    fn delete(&self, material_ref: &str) -> Result<()> {
        let path = self.material_path(material_ref)?;
        match fs::remove_file(&path) {
            Ok(()) => {
                let parent = path.parent().ok_or_else(|| {
                    identity_error(
                        "workspace_signing_identity_material_ref_invalid",
                        "Workspace signing private material reference has no parent",
                    )
                })?;
                sync_directory(parent)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(material_io_error("delete", error)),
        }
    }
}

fn ensure_private_tree(root: &Path, leaf: &Path) -> Result<()> {
    ensure_private_directory(root)?;
    if let Some(parent) = root.parent() {
        sync_directory(parent)?;
    }
    let relative = leaf.strip_prefix(root).map_err(|_| {
        identity_error(
            "workspace_signing_identity_material_ref_invalid",
            "Workspace signing private material path escapes its authority root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let parent = current.clone();
        current.push(component);
        ensure_private_directory(&current)?;
        sync_directory(&parent)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| material_io_error("synchronize directory", error))
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Err(identity_error(
        "workspace_signing_identity_durable_publish_unsupported",
        "Workspace signing private material durable publication is unsupported on this platform",
    ))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|error| material_io_error("create directory", error))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| material_io_error("set directory permissions", error))?;
    }
    Ok(())
}

pub fn workspace_signing_material_root(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("workspace-signing-identities")
}

pub fn public_key_fingerprint(public_key: &str) -> Result<String> {
    let bytes = worker_runtime::auth::decode_public_key(public_key).map_err(|_| {
        identity_error(
            "workspace_signing_identity_public_key_invalid",
            "Workspace signing public key is invalid",
        )
    })?;
    use sha2::{Digest, Sha256};
    Ok(format!("sha256:{}", hex_lower(&Sha256::digest(bytes))))
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

fn material_io_error(action: &str, error: std::io::Error) -> Error {
    identity_error(
        "workspace_signing_identity_material_io_failed",
        format!("failed to {action} Workspace signing private material: {error}"),
    )
}

fn capability_error(error: WorkspaceCapabilityVerificationError) -> Error {
    identity_error(
        "workspace_capability_issuance_failed",
        format!("failed to issue Workspace capability: {error}"),
    )
}

pub fn identity_error(code: impl Into<String>, message: impl Into<String>) -> Error {
    Error::WorkspaceSigningIdentity {
        code: code.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    struct FailFirstMaterialWrite {
        inner: Arc<InMemoryWorkspaceSigningMaterialStore>,
        fail: AtomicBool,
    }

    impl WorkspaceSigningMaterialStore for FailFirstMaterialWrite {
        fn load(&self, material_ref: &str) -> Result<Option<WorkspaceSigningPrivateMaterial>> {
            self.inner.load(material_ref)
        }

        fn put_if_absent(
            &self,
            material_ref: &str,
            material: &WorkspaceSigningPrivateMaterial,
        ) -> Result<WorkspaceSigningPrivateMaterial> {
            if self.fail.swap(false, Ordering::SeqCst) {
                return Err(identity_error(
                    "workspace_signing_identity_material_io_failed",
                    "injected private material write failure",
                ));
            }
            self.inner.put_if_absent(material_ref, material)
        }

        fn delete(&self, material_ref: &str) -> Result<()> {
            self.inner.delete(material_ref)
        }
    }

    #[tokio::test]
    async fn existing_workspace_provisioning_is_audited_idempotent_and_fails_closed_when_missing() {
        use crate::store::{AccountRecord, SqliteWorkspaceStore, WorkspaceRecord};

        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("server.db");
        let store = Arc::new(SqliteWorkspaceStore::open(&database_path).unwrap());
        store
            .upsert_account(&AccountRecord {
                account_id: "account-1".to_string(),
                kind: "user".to_string(),
                handle: "owner".to_string(),
                display_name: "Owner".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .unwrap();
        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-1".to_string(),
                owner_account_id: "account-1".to_string(),
                display_name: "Workspace".to_string(),
                state: "active".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .await
            .unwrap();
        let materials = Arc::new(InMemoryWorkspaceSigningMaterialStore::default());
        let failing_service = WorkspaceSigningIdentityService::new(
            store.clone(),
            Arc::new(FailFirstMaterialWrite {
                inner: materials.clone(),
                fail: AtomicBool::new(true),
            }),
        );
        assert_eq!(
            failing_service.get_validated("workspace-1").unwrap().state,
            "pending_provisioning"
        );
        let error = failing_service
            .provision_existing("workspace-1", "account-1")
            .unwrap_err();
        assert!(matches!(
            error,
            Error::WorkspaceSigningIdentity { ref code, .. }
                if code == "workspace_signing_identity_material_io_failed"
        ));
        assert_eq!(
            store
                .get_workspace_signing_identity("workspace-1")
                .unwrap()
                .unwrap()
                .state,
            "pending_provisioning"
        );

        drop(failing_service);
        drop(store);
        let store = Arc::new(SqliteWorkspaceStore::open(&database_path).unwrap());
        let service = WorkspaceSigningIdentityService::new(store.clone(), materials.clone());
        let provisioned = service
            .provision_existing("workspace-1", "account-1")
            .unwrap();
        assert_eq!(provisioned.state, "active");
        assert!(provisioned.public_key.is_some());
        let payload = b"Workspace authority proof";
        let signature = service.sign("workspace-1", payload).unwrap();
        let public_key =
            worker_runtime::auth::decode_public_key(provisioned.public_key.as_deref().unwrap())
                .unwrap();
        ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, public_key)
            .verify(payload, &signature)
            .unwrap();
        assert_eq!(
            service
                .provision_existing("workspace-1", "account-1")
                .unwrap(),
            provisioned
        );
        store
            .with_conn(|conn| {
                assert_eq!(
                    conn.query_row(
                        "SELECT COUNT(*) FROM workspace_signing_identity_audit WHERE workspace_id = 'workspace-1'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    1
                );
                Ok(())
            })
            .unwrap();

        store
            .upsert_workspace(&WorkspaceRecord {
                workspace_id: "workspace-2".to_string(),
                owner_account_id: "account-1".to_string(),
                display_name: "Workspace 2".to_string(),
                state: "active".to_string(),
                created_at: "1".to_string(),
                updated_at: "1".to_string(),
            })
            .await
            .unwrap();
        let pending = store
            .get_workspace_signing_identity("workspace-2")
            .unwrap()
            .unwrap();
        let operation = store
            .reserve_workspace_signing_identity_provisioning(
                &WorkspaceSigningIdentityProvisioningOperation {
                    operation_key: "existing-workspace:workspace-2:revision-1".to_string(),
                    request_fingerprint: provisioning_fingerprint(
                        "workspace-2",
                        &pending.key_id,
                        pending.revision,
                    ),
                    operation_kind: "existing_workspace".to_string(),
                    workspace_id: "workspace-2".to_string(),
                    key_id: pending.key_id.clone(),
                    private_material_ref: pending.private_material_ref.clone(),
                    revision: pending.revision,
                    actor_account_id: "account-1".to_string(),
                    state: "pending".to_string(),
                    created_at: "1".to_string(),
                    completed_at: None,
                },
            )
            .unwrap();
        let activation = service.prepare_material(&operation).unwrap();
        store
            .with_conn(|conn| {
                conn.execute_batch(
                    r#"CREATE TRIGGER fail_workspace_signing_identity_audit
                       BEFORE INSERT ON workspace_signing_identity_audit
                       BEGIN SELECT RAISE(ABORT, 'injected audit failure'); END;"#,
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            store
                .activate_workspace_signing_identity(
                    &activation,
                    &operation.operation_key,
                    "account-1",
                )
                .is_err()
        );
        store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER fail_workspace_signing_identity_audit;")?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .get_workspace_signing_identity("workspace-2")
                .unwrap()
                .unwrap()
                .state,
            "pending_provisioning"
        );
        drop(service);
        drop(store);
        let store = Arc::new(SqliteWorkspaceStore::open(&database_path).unwrap());
        let restarted = WorkspaceSigningIdentityService::new(store.clone(), materials.clone());
        let recovered = restarted
            .provision_existing("workspace-2", "account-1")
            .unwrap();
        assert_eq!(recovered.key_id, activation.key_id);
        assert_eq!(
            recovered.public_key_fingerprint.as_deref(),
            Some(activation.public_key_fingerprint.as_str())
        );

        materials.delete(&provisioned.private_material_ref).unwrap();
        let error = restarted.get_validated("workspace-1").unwrap_err();
        assert!(matches!(
            error,
            Error::WorkspaceSigningIdentity { ref code, .. }
                if code == "workspace_signing_identity_material_missing"
        ));
    }

    #[test]
    fn file_store_round_trips_private_material_without_overwrite() {
        let temp = tempfile::tempdir().unwrap();
        let store = FsWorkspaceSigningMaterialStore::new(temp.path().join("identities"));
        let first = WorkspaceSigningPrivateMaterial::generate("ws-1", "WK-1").unwrap();
        let first_public = first.validate_and_public_key("ws-1", "WK-1", 1).unwrap();
        let persisted = store.put_if_absent("ws-1/ed25519-v1", &first).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(temp.path().join("identities"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(store.material_path("ws-1/ed25519-v1").unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            persisted
                .validate_and_public_key("ws-1", "WK-1", 1)
                .unwrap(),
            first_public
        );

        let second = WorkspaceSigningPrivateMaterial::generate("ws-1", "WK-1").unwrap();
        let persisted = store.put_if_absent("ws-1/ed25519-v1", &second).unwrap();
        assert_eq!(
            persisted
                .validate_and_public_key("ws-1", "WK-1", 1)
                .unwrap(),
            first_public
        );
    }

    #[test]
    fn corrupt_and_cross_workspace_material_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let store = FsWorkspaceSigningMaterialStore::new(temp.path().join("identities"));
        let material = WorkspaceSigningPrivateMaterial::generate("ws-1", "WK-1").unwrap();
        store.put_if_absent("ws-1/ed25519-v1", &material).unwrap();
        let loaded = store.load("ws-1/ed25519-v1").unwrap().unwrap();
        assert!(loaded.validate_and_public_key("ws-2", "WK-1", 1).is_err());

        fs::write(store.material_path("ws-1/ed25519-v1").unwrap(), b"not json").unwrap();
        assert!(store.load("ws-1/ed25519-v1").is_err());
    }
}
