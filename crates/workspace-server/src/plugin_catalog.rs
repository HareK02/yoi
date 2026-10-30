//! Workspace catalog authority. The authenticated service checks current read/manage
//! permission before entering these methods; neither a digest nor an actor string is
//! an authorization credential. Blob publication, retention and GC share SQLite's
//! immediate write transaction, including across independent Server connections.
use crate::{Error, Result, store::SqliteWorkspaceStore};
use chrono::{Duration, Utc};
use manifest::plugin::{self, PluginPackageLimits, PluginSourceKind};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use server_api::*;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PluginCatalogError {
    #[error("invalid Plugin catalog request")]
    Invalid,
    #[error("Plugin catalog resource not found")]
    NotFound,
    #[error("Plugin catalog operation conflicts with observed state")]
    Conflict,
    #[error("Plugin installation has retained references")]
    Referenced,
    #[error("Plugin package exceeds size limits")]
    TooLarge,
    #[error("Plugin storage quota exceeded")]
    Quota,
    #[error("Plugin artifact storage unavailable")]
    Unavailable,
}
impl PluginCatalogError {
    pub fn status(self) -> u16 {
        match self {
            Self::Invalid => 400,
            Self::NotFound => 404,
            Self::Conflict | Self::Referenced => 409,
            Self::TooLarge => 413,
            Self::Quota => 429,
            Self::Unavailable => 503,
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "plugin_invalid",
            Self::NotFound => "plugin_not_found",
            Self::Conflict => "plugin_conflict",
            Self::Referenced => "plugin_referenced",
            Self::TooLarge => "plugin_too_large",
            Self::Quota => "plugin_quota",
            Self::Unavailable => "plugin_unavailable",
        }
    }
}

const WORKSPACE_BYTES: i64 = 1024 * 1024 * 1024;
const ACTOR_UPLOAD_BYTES: i64 = 256 * 1024 * 1024;
const MAX_UPLOADS: i64 = 128;
const MAX_ACTOR_UPLOADS: i64 = 32;
const MAX_INSTALLATIONS: i64 = 128;
const MAX_ARTIFACTS: i64 = 64;

pub(crate) const MIGRATION_NAME: &str =
    "Workspace Plugin catalog and immutable artifact references";
pub(crate) fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row(
        "SELECT max(version) FROM __yoi_schema_migrations",
        [],
        |r| r.get(0),
    )?;
    if version != 105 {
        return Err(Error::Store(
            "Plugin catalog migration requires schema 105".into(),
        ));
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch(include_str!("plugin_catalog_schema.sql"))?;
    tx.execute(
        "INSERT INTO __yoi_schema_migrations(version,name) VALUES(106,?1)",
        [MIGRATION_NAME],
    )?;
    verify(&tx)?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn verify(conn: &Connection) -> Result<()> {
    verify_catalog_schema(conn, false)
}
/// Latest-schema verification after the separately registered typed-data
/// migration. Historical migration preconditions continue to use `verify`.
pub(crate) fn verify_with_data(conn: &Connection) -> Result<()> {
    verify_catalog_schema(conn, true)
}
fn verify_catalog_schema(conn: &Connection, with_data: bool) -> Result<()> {
    let expected = Connection::open_in_memory()?;
    expected.execute_batch(include_str!("plugin_catalog_schema.sql"))?;
    if with_data {
        expected.execute_batch(crate::plugin_data::REFERENCE_MIGRATION_SQL)?;
    }
    let objects = expected.prepare("SELECT name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND tbl_name LIKE 'plugin_%' ORDER BY name")?
        .query_map([], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (name, sql) in objects {
        let actual = conn
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name=?1",
                [&name],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        if actual.as_deref() != Some(sql.as_str()) {
            return Err(Error::Store(
                "Plugin catalog storage schema has drifted".into(),
            ));
        }
    }
    Ok(())
}
fn fail<T>(error: PluginCatalogError) -> Result<T> {
    Err(Error::PluginCatalog(error))
}
fn encode<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|_| PluginCatalogError::Unavailable.into())
}
fn decode<T: DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value).map_err(|_| PluginCatalogError::Unavailable.into())
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7())
}
fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(hex_digest)
}
fn hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn workspace_exists(conn: &Connection, workspace: &str) -> Result<()> {
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspaces WHERE workspace_id=?1)",
        [workspace],
        |r| r.get::<_, bool>(0),
    )? {
        return fail(PluginCatalogError::NotFound);
    }
    Ok(())
}
fn validate_identity(actor: &str, key: &str) -> Result<()> {
    if !bounded(actor, 512) || !bounded(key, 128) {
        return fail(PluginCatalogError::Invalid);
    }
    Ok(())
}
fn storage<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|_| PluginCatalogError::Unavailable.into())
}
fn blob_path(root: &Path, artifact: &str) -> Result<PathBuf> {
    if !digest(artifact) {
        return fail(PluginCatalogError::Invalid);
    }
    Ok(root.join("blobs").join(&artifact[7..]))
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub(crate) fn read_blob(root: &Path, artifact: &str, size: u64) -> Result<Vec<u8>> {
    let path = blob_path(root, artifact)?;
    let metadata = storage(fs::symlink_metadata(&path))?;
    if !metadata.is_file()
        || metadata.len() != size
        || size > PluginPackageLimits::default().max_package_size_bytes
    {
        return fail(PluginCatalogError::Unavailable);
    }
    let file = storage(fs::File::open(path))?;
    let mut bytes = Vec::new();
    storage(file.take(size + 1).read_to_end(&mut bytes))?;
    if bytes.len() as u64 != size || format!("sha256:{}", hash(&bytes)) != artifact {
        return fail(PluginCatalogError::Unavailable);
    }
    Ok(bytes)
}
// Caller holds SQLite write ownership until the upload reference commits. GC
// cannot unlink a candidate between publication and the first durable reference.
fn publish_blob(root: &Path, artifact: &str, bytes: &[u8]) -> Result<()> {
    let path = blob_path(root, artifact)?;
    storage(fs::create_dir_all(root.join("blobs")))?;
    if path.exists() {
        read_blob(root, artifact, bytes.len() as u64)?;
        return Ok(());
    }
    let pending = PendingFile(root.join("blobs").join(id(".pending")));
    let mut file = storage(
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&pending.0),
    )?;
    storage(file.write_all(bytes))?;
    storage(file.sync_all())?;
    match fs::hard_link(&pending.0, &path) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            read_blob(root, artifact, bytes.len() as u64)?;
        }
        Err(_) => return fail(PluginCatalogError::Unavailable),
    }
    storage(fs::File::open(root.join("blobs")))?
        .sync_all()
        .map_err(|_| Error::PluginCatalog(PluginCatalogError::Unavailable))?;
    Ok(())
}
fn artifact_metadata(
    package: &plugin::MaterializedPluginPackage,
    canonical: &plugin::CanonicalPluginArtifact,
) -> Result<PluginArtifact> {
    let m = &package.package.manifest;
    let schema = plugin::plugin_config_schema_identity(m);
    Ok(PluginArtifact {
        package_id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        description: m.description.clone(),
        package_digest: canonical.package_digest.clone(),
        artifact_digest: canonical.artifact_digest.clone(),
        artifact_size: u32::try_from(canonical.artifact_size)
            .map_err(|_| PluginCatalogError::TooLarge)?,
        schema_version: m.schema_version,
        world: m.runtime.world.clone(),
        required_imports: m.runtime.required_imports.clone(),
        tools: m
            .tools
            .iter()
            .map(|t| PluginToolMetadata {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
                output_schema: t.output_schema.clone(),
                error_codes: t.error_codes.clone(),
                external_write: t.external_write,
            })
            .collect(),
        connections: decode(&encode(&m.connections)?)?,
        requested_grants: decode(&encode(&m.requested_grants)?)?,
        data_collections: decode(&encode(&m.data_collections)?)?,
        config_schema: PluginSchemaContribution {
            identity: schema.identity,
            version: schema.version,
            namespace: schema.namespace,
            digest: schema.digest,
            dcdl: m.config_schema.clone(),
        },
        revoked: false,
        revocation_reason: None,
    })
}
fn load_artifact(
    conn: &Connection,
    workspace: &str,
    installation: &str,
    artifact: &str,
) -> Result<PluginArtifact> {
    let row=conn.query_row("SELECT metadata_json,revoked,revocation_reason FROM plugin_artifacts WHERE workspace_id=?1 AND installation_id=?2 AND artifact_digest=?3",params![workspace,installation,artifact],|r|Ok((r.get::<_,String>(0)?,r.get::<_,bool>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
    let Some((json, revoked, reason)) = row else {
        return fail(PluginCatalogError::NotFound);
    };
    let mut result: PluginArtifact = decode(&json)?;
    result.revoked = revoked;
    result.revocation_reason = reason;
    Ok(result)
}
fn load_installation(
    conn: &Connection,
    workspace: &str,
    installation: &str,
) -> Result<PluginInstallation> {
    let row=conn.query_row("SELECT alias,package_id,last_operation,uninstalled FROM plugin_installations WHERE workspace_id=?1 AND installation_id=?2",params![workspace,installation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,bool>(3)?))).optional()?;
    let Some((alias, package_id, last_operation, uninstalled)) = row else {
        return fail(PluginCatalogError::NotFound);
    };
    let digests=conn.prepare("SELECT artifact_digest FROM plugin_artifacts WHERE workspace_id=?1 AND installation_id=?2 ORDER BY artifact_digest")?.query_map(params![workspace,installation],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let artifacts = digests
        .iter()
        .map(|d| load_artifact(conn, workspace, installation, d))
        .collect::<Result<Vec<_>>>()?;
    Ok(PluginInstallation {
        installation_id: installation.into(),
        alias,
        package_id,
        last_operation,
        uninstalled,
        artifacts,
    })
}
fn compare_installation(
    conn: &Connection,
    workspace: &str,
    installation: &str,
    expected: &str,
) -> Result<PluginInstallation> {
    let current = load_installation(conn, workspace, installation)?;
    if current.uninstalled {
        return fail(PluginCatalogError::NotFound);
    }
    if current.last_operation != expected {
        return fail(PluginCatalogError::Conflict);
    }
    Ok(current)
}
fn load_upload(
    conn: &Connection,
    workspace: &str,
    actor: &str,
    upload: &str,
) -> Result<(PluginArtifact, i64)> {
    let row=conn.query_row("SELECT metadata_json,expires_at FROM plugin_uploads WHERE workspace_id=?1 AND actor=?2 AND upload_id=?3",params![workspace,actor,upload],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?))).optional()?;
    let Some((json, expires)) = row else {
        return fail(PluginCatalogError::NotFound);
    };
    Ok((decode(&json)?, expires))
}
fn kind_name(kind: PluginOperationKind) -> &'static str {
    match kind {
        PluginOperationKind::Install => "install",
        PluginOperationKind::AddArtifact => "add_artifact",
        PluginOperationKind::Revoke => "revoke",
        PluginOperationKind::Uninstall => "uninstall",
    }
}
fn replay(
    conn: &Connection,
    workspace: &str,
    actor: &str,
    kind: PluginOperationKind,
    key: &str,
    fingerprint: &str,
) -> Result<Option<PluginOperationResponse>> {
    let saved=conn.query_row("SELECT fingerprint,result_json FROM plugin_operations WHERE workspace_id=?1 AND actor=?2 AND kind=?3 AND operation_key=?4",params![workspace,actor,kind_name(kind),key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?;
    if let Some((old, json)) = saved {
        if old != fingerprint {
            return fail(PluginCatalogError::Conflict);
        }
        return Ok(Some(decode(&json)?));
    }
    Ok(None)
}
fn save_operation(
    tx: &Transaction<'_>,
    workspace: &str,
    actor: &str,
    key: &str,
    fingerprint: &str,
    kind: PluginOperationKind,
    operation: &str,
    installation: &str,
) -> Result<PluginOperationResponse> {
    tx.execute("UPDATE plugin_installations SET last_operation=?3 WHERE workspace_id=?1 AND installation_id=?2",params![workspace,installation,operation])?;
    let result = PluginOperationResponse {
        operation_id: operation.into(),
        kind,
        installation: load_installation(tx, workspace, installation)?,
    };
    tx.execute("INSERT INTO plugin_operations(workspace_id,operation_id,actor,kind,operation_key,fingerprint,result_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![workspace,operation,actor,kind_name(kind),key,fingerprint,encode(&result)?])?;
    Ok(result)
}
// Collection/schema-id/version is a durable namespace within one installation.
// Revocation does not erase that contract: a later artifact must use a new schema
// version instead of reinterpreting retained cells under a changed schema.
fn canonical_schema(value: &serde_json::Value) -> Result<String> {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let ordered: std::collections::BTreeMap<_, _> = map
                    .iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(sorted).collect())
            }
            value => value.clone(),
        }
    }
    encode(&sorted(value))
}
fn compare_data_schemas(
    tx: &Transaction<'_>,
    workspace: &str,
    installation: &str,
    artifact: &PluginArtifact,
) -> Result<()> {
    let mut incoming = std::collections::BTreeMap::new();
    for schema in &artifact.data_collections {
        let address = (
            schema.collection.clone(),
            schema.schema_id.clone(),
            schema.schema_version.clone(),
        );
        let canonical = canonical_schema(&schema.schema)?;
        if let Some(previous) = incoming.insert(address, canonical.clone()) {
            if previous != canonical {
                return fail(PluginCatalogError::Conflict);
            }
        }
    }
    if incoming.is_empty() {
        return Ok(());
    }
    // No revoked filter: the schema identity outlives execution authority.
    let mut statement = tx.prepare(
        "SELECT metadata_json FROM plugin_artifacts WHERE workspace_id=?1 AND installation_id=?2",
    )?;
    let mut rows = statement.query(params![workspace, installation])?;
    while let Some(row) = rows.next()? {
        let registered: PluginArtifact = decode(&row.get::<_, String>(0)?)?;
        for schema in registered.data_collections {
            let address = (
                schema.collection.clone(),
                schema.schema_id.clone(),
                schema.schema_version.clone(),
            );
            if let Some(candidate) = incoming.get(&address) {
                if *candidate != canonical_schema(&schema.schema)? {
                    return fail(PluginCatalogError::Conflict);
                }
            }
        }
    }
    Ok(())
}
fn add_artifact(
    tx: &Transaction<'_>,
    workspace: &str,
    installation: &str,
    artifact: &PluginArtifact,
) -> Result<()> {
    compare_data_schemas(tx, workspace, installation, artifact)?;
    let existing=tx.query_row("SELECT artifact_digest,package_digest,revoked FROM plugin_artifacts WHERE workspace_id=?1 AND installation_id=?2 AND version=?3",params![workspace,installation,artifact.version],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,bool>(2)?))).optional()?;
    if let Some((old, package, revoked)) = existing {
        if old != artifact.artifact_digest || package != artifact.package_digest || revoked {
            return fail(PluginCatalogError::Conflict);
        }
        return Ok(());
    }
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM plugin_artifacts WHERE workspace_id=?1 AND installation_id=?2",
        params![workspace, installation],
        |r| r.get(0),
    )?;
    if count >= MAX_ARTIFACTS {
        return fail(PluginCatalogError::Quota);
    }
    check_workspace_bytes(tx, workspace, artifact.artifact_size as i64)?;
    tx.execute("INSERT INTO plugin_artifacts(workspace_id,installation_id,artifact_digest,package_digest,version,artifact_size,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![workspace,installation,artifact.artifact_digest,artifact.package_digest,artifact.version,artifact.artifact_size as i64,encode(artifact)?])?;
    Ok(())
}
fn check_workspace_bytes(conn: &Connection, workspace: &str, additional: i64) -> Result<()> {
    let bytes:i64=conn.query_row("SELECT (SELECT coalesce(sum(a.artifact_size),0) FROM plugin_artifacts a JOIN plugin_installations i USING(workspace_id,installation_id) WHERE a.workspace_id=?1 AND i.uninstalled=0) + (SELECT coalesce(sum(artifact_size),0) FROM plugin_uploads WHERE workspace_id=?1 AND expires_at>?2)",params![workspace,Utc::now().timestamp()],|r|r.get(0))?;
    if bytes + additional > WORKSPACE_BYTES {
        return fail(PluginCatalogError::Quota);
    }
    Ok(())
}

impl SqliteWorkspaceStore {
    pub fn plugin_upload(
        &self,
        root: &Path,
        workspace: &str,
        actor: &str,
        bytes: &[u8],
    ) -> Result<PluginUploadResponse> {
        if !bounded(actor, 512) {
            return fail(PluginCatalogError::Invalid);
        }
        let limits = PluginPackageLimits::default();
        if bytes.len() as u64 > limits.max_package_size_bytes {
            return fail(PluginCatalogError::TooLarge);
        }
        let package = plugin::read_plugin_package_bytes(bytes, PluginSourceKind::Explicit, &limits)
            .map_err(|e| {
                if e.kind == plugin::PluginDiagnosticKind::Bounds {
                    PluginCatalogError::TooLarge
                } else {
                    PluginCatalogError::Invalid
                }
            })?;
        let canonical = plugin::pack_plugin_package(&package, &limits)
            .map_err(|_| PluginCatalogError::Invalid)?;
        let artifact = artifact_metadata(&package, &canonical)?;
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            workspace_exists(&tx,workspace)?;
            let now=Utc::now();
            let (count,own,own_bytes):(i64,i64,i64)=tx.query_row("SELECT count(*),coalesce(sum(actor=?2),0),coalesce(sum(CASE WHEN actor=?2 THEN artifact_size ELSE 0 END),0) FROM plugin_uploads WHERE workspace_id=?1 AND expires_at>?3",params![workspace,actor,now.timestamp()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            if count>=MAX_UPLOADS || own>=MAX_ACTOR_UPLOADS || own_bytes+canonical.artifact_size as i64>ACTOR_UPLOAD_BYTES {return fail(PluginCatalogError::Quota);}
            check_workspace_bytes(&tx,workspace,canonical.artifact_size as i64)?;
            publish_blob(root,&canonical.artifact_digest,&canonical.bytes)?;
            let upload_id=id("pu"); let expires=now+Duration::hours(24);
            tx.execute("INSERT INTO plugin_uploads(workspace_id,upload_id,actor,expires_at,artifact_digest,artifact_size,metadata_json) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![workspace,upload_id,actor,expires.timestamp(),artifact.artifact_digest,artifact.artifact_size as i64,encode(&artifact)?])?;
            tx.commit()?;
            Ok(PluginUploadResponse{upload_id,expires_at:expires.to_rfc3339_opts(chrono::SecondsFormat::Secs,true),artifact:artifact.clone()})
        })
    }
    pub fn plugin_install(
        &self,
        workspace: &str,
        actor: &str,
        request: &PluginInstallRequest,
    ) -> Result<PluginOperationResponse> {
        validate_identity(actor, &request.operation_key)?;
        if !bounded(&request.alias, 128) || request.alias.trim() != request.alias {
            return fail(PluginCatalogError::Invalid);
        }
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (artifact,expires)=load_upload(&tx,workspace,actor,&request.upload_id)?;
            let fingerprint=hash(encode(&(request,&artifact.package_digest,&artifact.artifact_digest))?.as_bytes());
            if let Some(saved)=replay(&tx,workspace,actor,PluginOperationKind::Install,&request.operation_key,&fingerprint)? {return Ok(saved);}
            if expires<=Utc::now().timestamp() {return fail(PluginCatalogError::NotFound);}
            let (count,alias):(i64,bool)=tx.query_row("SELECT count(*),coalesce(max(alias=?2),0) FROM plugin_installations WHERE workspace_id=?1 AND uninstalled=0",params![workspace,request.alias],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if alias {return fail(PluginCatalogError::Conflict);}
            if count>=MAX_INSTALLATIONS {return fail(PluginCatalogError::Quota);}
            let installation=id("pi"); let operation=id("po");
            tx.execute("INSERT INTO plugin_installations(workspace_id,installation_id,alias,package_id,last_operation) VALUES(?1,?2,?3,?4,?5)",params![workspace,installation,request.alias,artifact.package_id,operation])?;
            add_artifact(&tx,workspace,&installation,&artifact)?;
            let result=save_operation(&tx,workspace,actor,&request.operation_key,&fingerprint,PluginOperationKind::Install,&operation,&installation)?;
            tx.commit()?; Ok(result)
        })
    }
    pub fn plugin_add_artifact(
        &self,
        workspace: &str,
        actor: &str,
        installation: &str,
        request: &PluginAddArtifactRequest,
    ) -> Result<PluginOperationResponse> {
        validate_identity(actor, &request.operation_key)?;
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (artifact, expires) = load_upload(&tx, workspace, actor, &request.upload_id)?;
            let fingerprint = hash(
                encode(&(
                    installation,
                    request,
                    &artifact.package_digest,
                    &artifact.artifact_digest,
                ))?
                .as_bytes(),
            );
            if let Some(saved) = replay(
                &tx,
                workspace,
                actor,
                PluginOperationKind::AddArtifact,
                &request.operation_key,
                &fingerprint,
            )? {
                return Ok(saved);
            }
            if expires <= Utc::now().timestamp() {
                return fail(PluginCatalogError::NotFound);
            }
            let current = compare_installation(
                &tx,
                workspace,
                installation,
                &request.expected_last_operation,
            )?;
            if current.package_id != artifact.package_id {
                return fail(PluginCatalogError::Conflict);
            }
            add_artifact(&tx, workspace, installation, &artifact)?;
            let result = save_operation(
                &tx,
                workspace,
                actor,
                &request.operation_key,
                &fingerprint,
                PluginOperationKind::AddArtifact,
                &id("po"),
                installation,
            )?;
            tx.commit()?;
            Ok(result)
        })
    }
    pub fn plugin_revoke(
        &self,
        workspace: &str,
        actor: &str,
        installation: &str,
        request: &PluginRevokeRequest,
    ) -> Result<PluginOperationResponse> {
        validate_identity(actor, &request.operation_key)?;
        if !digest(&request.artifact_digest) || !bounded(&request.reason, 1024) {
            return fail(PluginCatalogError::Invalid);
        }
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let fingerprint=hash(encode(&(installation,request))?.as_bytes());
            if let Some(saved)=replay(&tx,workspace,actor,PluginOperationKind::Revoke,&request.operation_key,&fingerprint)? {return Ok(saved);}
            compare_installation(&tx,workspace,installation,&request.expected_last_operation)?;
            let artifact=load_artifact(&tx,workspace,installation,&request.artifact_digest)?;
            if artifact.revoked {return fail(PluginCatalogError::Conflict);}
            tx.execute("UPDATE plugin_artifacts SET revoked=1,revocation_reason=?4 WHERE workspace_id=?1 AND installation_id=?2 AND artifact_digest=?3",params![workspace,installation,request.artifact_digest,request.reason])?;
            let result=save_operation(&tx,workspace,actor,&request.operation_key,&fingerprint,PluginOperationKind::Revoke,&id("po"),installation)?;
            tx.commit()?; Ok(result)
        })
    }
    pub fn plugin_uninstall(
        &self,
        workspace: &str,
        actor: &str,
        installation: &str,
        request: &PluginUninstallRequest,
    ) -> Result<PluginOperationResponse> {
        validate_identity(actor, &request.operation_key)?;
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let fingerprint=hash(encode(&(installation,request))?.as_bytes());
            if let Some(saved)=replay(&tx,workspace,actor,PluginOperationKind::Uninstall,&request.operation_key,&fingerprint)? {return Ok(saved);}
            compare_installation(&tx,workspace,installation,&request.expected_last_operation)?;
            let referenced:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM plugin_references WHERE workspace_id=?1 AND installation_id=?2)",params![workspace,installation],|r|r.get(0))?;
            if referenced {return fail(PluginCatalogError::Referenced);}
            tx.execute("UPDATE plugin_installations SET uninstalled=1 WHERE workspace_id=?1 AND installation_id=?2",params![workspace,installation])?;
            let result=save_operation(&tx,workspace,actor,&request.operation_key,&fingerprint,PluginOperationKind::Uninstall,&id("po"),installation)?;
            tx.commit()?; Ok(result)
        })
    }
    pub fn plugin_show(&self, workspace: &str, installation: &str) -> Result<PluginInstallation> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let result = load_installation(&tx, workspace, installation)?;
            tx.commit()?;
            Ok(result)
        })
    }
    pub fn plugin_operation(
        &self,
        workspace: &str,
        operation: &str,
    ) -> Result<PluginOperationResponse> {
        self.with_conn(|conn| {
            let json=conn.query_row("SELECT result_json FROM plugin_operations WHERE workspace_id=?1 AND operation_id=?2",params![workspace,operation],|r|r.get::<_,String>(0)).optional()?;
            match json {Some(json)=>decode(&json),None=>fail(PluginCatalogError::NotFound)}
        })
    }
    pub fn plugin_list(
        &self,
        workspace: &str,
        request: &PluginListRequest,
    ) -> Result<PluginListResponse> {
        let limit = request.limit.unwrap_or(50);
        if limit == 0 || limit > 100 {
            return fail(PluginCatalogError::Invalid);
        }
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let conn = &tx;
            workspace_exists(conn,workspace)?;
            let after=if let Some(cursor)=&request.cursor {
                // Server generated operation identity is an opaque cursor scoped by
                // a persisted membership check, never interpreted as an arbitrary offset.
                let value=conn.query_row("SELECT json_extract(result_json,'$.installation.installation_id') FROM plugin_operations WHERE workspace_id=?1 AND operation_id=?2",params![workspace,cursor],|r|r.get::<_,String>(0)).optional()?;
                value.ok_or(PluginCatalogError::Invalid)?
            } else {String::new()};
            let mut installations=conn.prepare("SELECT installation_id,alias,package_id,last_operation,uninstalled FROM plugin_installations WHERE workspace_id=?1 AND installation_id>?2 ORDER BY installation_id LIMIT ?3")?.query_map(params![workspace,after,limit+1],|r|Ok(PluginInstallationSummary{installation_id:r.get(0)?,alias:r.get(1)?,package_id:r.get(2)?,last_operation:r.get(3)?,uninstalled:r.get(4)?}))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let more=installations.len()>limit as usize;
            installations.truncate(limit as usize);
            let next_cursor=if more {installations.last().map(|i|i.last_operation.clone())} else {None};
            tx.commit()?;
            Ok(PluginListResponse{installations,next_cursor})
        })
    }
    pub fn plugin_artifact_bytes(
        &self,
        root: &Path,
        workspace: &str,
        installation: &str,
        artifact_digest: &str,
    ) -> Result<Vec<u8>> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = load_installation(&tx, workspace, installation)?;
            if current.uninstalled {
                return fail(PluginCatalogError::NotFound);
            }
            let artifact = load_artifact(&tx, workspace, installation, artifact_digest)?;
            if artifact.revoked {
                return fail(PluginCatalogError::Conflict);
            }
            let bytes = read_blob(root, artifact_digest, u64::from(artifact.artifact_size))?;
            tx.commit()?;
            Ok(bytes)
        })
    }
    pub fn plugin_gc(&self, root: &Path) -> Result<u64> {
        self.with_conn_mut(|conn| {
            let tx=conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let retained=tx.prepare("SELECT a.artifact_digest FROM plugin_artifacts a JOIN plugin_installations i USING(workspace_id,installation_id) WHERE i.uninstalled=0 UNION SELECT artifact_digest FROM plugin_uploads WHERE expires_at>?1")?.query_map([Utc::now().timestamp()],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<BTreeSet<_>>>()?;
            let entries=match fs::read_dir(root.join("blobs")) {Ok(entries)=>entries,Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(0),Err(_)=>return fail(PluginCatalogError::Unavailable)};
            let mut removed=0;
            for entry in entries {
                let entry=storage(entry)?; let name=entry.file_name(); let Some(name)=name.to_str() else {continue;};
                if (hex_digest(name) || name.starts_with(".pending-")) && !retained.contains(&format!("sha256:{name}")) {
                    storage(fs::remove_file(entry.path()))?; removed+=1;
                }
            }
            storage(fs::File::open(root.join("blobs")))?.sync_all().map_err(|_|Error::PluginCatalog(PluginCatalogError::Unavailable))?;
            tx.commit()?; Ok(removed)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginReferenceKind {
    Config,
    Pending,
    Active,
    Restorable,
    /// Retained typed extension data, including delete tombstones.
    Data,
}
impl PluginReferenceKind {
    fn name(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Restorable => "restorable",
            Self::Data => "data",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginReference {
    pub kind: PluginReferenceKind,
    pub owner: String,
    pub installation_id: String,
    pub expected_last_operation: String,
    pub package_digest: String,
    pub artifact_digest: String,
}
/// Called inside the config/plan owner's transaction. The same write transaction
/// must commit that owner's content CAS and its reference changes. This function
/// does not persist a second copy of config or policy.
pub fn plugin_artifact_admit(
    tx: &Transaction<'_>,
    workspace: &str,
    installation: &str,
    expected_last_operation: &str,
    package_digest: &str,
    artifact_digest: &str,
) -> Result<PluginArtifact> {
    compare_installation(tx, workspace, installation, expected_last_operation)?;
    let artifact = load_artifact(tx, workspace, installation, artifact_digest)?;
    if artifact.revoked || artifact.package_digest != package_digest {
        return fail(PluginCatalogError::Conflict);
    }
    Ok(artifact)
}
pub fn plugin_reference_put(
    tx: &Transaction<'_>,
    workspace: &str,
    reference: &PluginReference,
) -> Result<()> {
    if !bounded(&reference.owner, 512) {
        return fail(PluginCatalogError::Invalid);
    }
    plugin_artifact_admit(
        tx,
        workspace,
        &reference.installation_id,
        &reference.expected_last_operation,
        &reference.package_digest,
        &reference.artifact_digest,
    )?;
    tx.execute("INSERT INTO plugin_references(workspace_id,kind,owner,installation_id,artifact_digest) VALUES(?1,?2,?3,?4,?5) ON CONFLICT DO NOTHING",params![workspace,reference.kind.name(),reference.owner,reference.installation_id,reference.artifact_digest])?;
    Ok(())
}
pub fn plugin_reference_remove(
    tx: &Transaction<'_>,
    workspace: &str,
    kind: PluginReferenceKind,
    owner: &str,
    installation: &str,
    artifact_digest: &str,
) -> Result<()> {
    tx.execute("DELETE FROM plugin_references WHERE workspace_id=?1 AND kind=?2 AND owner=?3 AND installation_id=?4 AND artifact_digest=?5",params![workspace,kind.name(),owner,installation,artifact_digest])?;
    Ok(())
}

#[cfg(test)]
#[path = "plugin_data/catalog_tests.rs"]
mod data_schema_tests;

#[cfg(test)]
mod tests;
