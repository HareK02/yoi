//! Runtime-owned byte cache, not a source of admission, configuration or grants.
//!
//! The caller supplies a private Runtime directory and an expectation from its
//! trusted Host plan, never a model-selected path or owner-supplied expectation.
//! Every resolution checks the live owner twice, including hits. The returned
//! bytes prove format/identity only; execution still requires current admission.
//! A process-released filesystem lock bounds all final and pending files together.
//! Only synced, completely verified files are renamed into the visible namespace;
//! abandoned staging files are discarded under that same lock after restart.

use async_trait::async_trait;
use manifest::plugin::{self, PluginPackageLimits};
use server_api::{PluginArtifact, PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

pub type ArtifactStream = Pin<Box<dyn AsyncRead + Send + Unpin>>;

#[async_trait]
pub trait ArtifactSource: Send + Sync {
    /// Stable authenticated Server authority, not a workspace or connection ID.
    fn server_authority_identity(&self) -> &str;
    async fn authorize(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata, DeliveryError>;
    async fn fetch(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<ArtifactStream, DeliveryError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    #[error("artifact delivery denied by current owner")]
    Denied,
    #[error("artifact owner unavailable: {0}")]
    Unavailable(String),
    #[error("invalid artifact delivery request")]
    InvalidRequest,
    #[error("artifact metadata differs from trusted Host plan")]
    MetadataMismatch,
    #[error("artifact exceeds delivery bound or declared size")]
    TooLarge,
    #[error("invalid artifact: {0}")]
    InvalidArtifact(String),
    #[error("cache directory must be private and Runtime-owned")]
    InvalidCacheRoot,
    #[error("invalid artifact cache limits")]
    InvalidLimits,
    #[error("artifact delivery deadline exceeded")]
    Timeout,
    #[error("artifact cache I/O failed: {0}")]
    Io(#[from] io::Error),
}

/// Exact immutable descriptor selected by the Host, not metadata learned from a
/// delivery response. Keeping the complete descriptor also binds Tool schemas,
/// connection declarations, requested (not effective) grants and schema identity.
#[derive(Clone, Debug)]
pub struct ArtifactExpectation {
    artifact: PluginArtifact,
}

impl ArtifactExpectation {
    pub fn new(host_plan_artifact: PluginArtifact) -> Self {
        Self {
            artifact: host_plan_artifact,
        }
    }

    pub fn descriptor(&self) -> &PluginArtifact {
        &self.artifact
    }
}

/// Bytes have no public constructor or mutable accessor, and contain no cached
/// owner authorization, Worker configuration, effective grants or pin ownership.
pub struct VerifiedArtifact {
    bytes: Vec<u8>,
}

impl std::fmt::Debug for VerifiedArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedArtifact")
            .field("byte_length", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

impl VerifiedArtifact {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Clone, Debug)]
pub struct CacheLimits {
    pub package: PluginPackageLimits,
    /// Includes the current staging download, not only published files.
    pub max_cache_bytes: u64,
    pub max_entries: usize,
    pub delivery_timeout: Duration,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            package: PluginPackageLimits::default(),
            max_cache_bytes: 256 * 1024 * 1024,
            max_entries: 32,
            delivery_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug)]
pub struct ArtifactCache {
    root: PathBuf,
    limits: CacheLimits,
}

impl ArtifactCache {
    /// The root belongs exclusively to the Runtime cache. Existing roots must
    /// already be private; this never fixes permissions on arbitrary directories.
    pub fn new(
        runtime_owned_root: impl AsRef<Path>,
        limits: CacheLimits,
    ) -> Result<Self, DeliveryError> {
        if limits.max_cache_bytes == 0
            || limits.max_entries == 0
            || limits.package.max_package_size_bytes == 0
            || limits.delivery_timeout.is_zero()
        {
            return Err(DeliveryError::InvalidLimits);
        }
        let root = runtime_owned_root.as_ref();
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root)?;
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(DeliveryError::InvalidCacheRoot);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(DeliveryError::InvalidCacheRoot);
            }
        }
        Ok(Self {
            root: fs::canonicalize(root)?,
            limits,
        })
    }

    pub async fn resolve(
        &self,
        source: &dyn ArtifactSource,
        trusted_workspace_id: &str,
        request: &PluginArtifactDeliveryRequest,
        expected: &ArtifactExpectation,
    ) -> Result<VerifiedArtifact, DeliveryError> {
        tokio::time::timeout(
            self.limits.delivery_timeout,
            self.resolve_inner(source, trusted_workspace_id, request, expected),
        )
        .await
        .map_err(|_| DeliveryError::Timeout)?
    }

    async fn resolve_inner(
        &self,
        source: &dyn ArtifactSource,
        workspace: &str,
        request: &PluginArtifactDeliveryRequest,
        expected: &ArtifactExpectation,
    ) -> Result<VerifiedArtifact, DeliveryError> {
        let authority = source.server_authority_identity().to_owned();
        let descriptor = &expected.artifact;
        if authority.is_empty()
            || workspace.is_empty()
            || request.plan_id.is_empty()
            || request.plan_fingerprint.is_empty()
            || request.installation_id.is_empty()
            || !is_digest(&request.artifact_digest)
            || request.artifact_digest != descriptor.artifact_digest
            || !is_digest(&descriptor.package_digest)
        {
            return Err(DeliveryError::InvalidRequest);
        }
        let size = u64::from(descriptor.artifact_size);
        if size == 0
            || size > self.limits.package.max_package_size_bytes
            || size > self.limits.max_cache_bytes
        {
            return Err(DeliveryError::TooLarge);
        }
        self.authorize(source, &authority, workspace, request, expected)
            .await?;

        // Lock ownership follows this future, including cancellation/deadline.
        let _lock = lock_cache(self.root.join(".cache.lock")).await?;
        let mut entries = self.inventory()?;
        let path = self.root.join(format!(
            "{}-{}.artifact",
            hash(authority.as_bytes()),
            &request.artifact_digest[7..]
        ));
        let bytes = match self.read_verified(&path, expected)? {
            Some(bytes) => {
                self.make_room(&mut entries, Some(&path), 0, 0)?;
                bytes
            }
            None => {
                // Discard a corrupt final file only while holding the cache lock;
                // no competing publisher can have repaired it since the read.
                if let Some(index) = entries.iter().position(|entry| entry.path == path) {
                    fs::remove_file(&path)?;
                    entries.remove(index);
                }
                self.make_room(&mut entries, None, size, 1)?;
                let mut stream = source.fetch(request).await?;
                if source.server_authority_identity() != authority {
                    return Err(DeliveryError::MetadataMismatch);
                }
                let pending = tempfile::Builder::new()
                    .prefix(".pending-")
                    .tempfile_in(&self.root)?;
                let mut file = tokio::fs::File::from_std(pending.reopen()?);
                let mut written = 0_u64;
                let mut buffer = [0_u8; 64 * 1024];
                loop {
                    // Read at most one byte beyond the declaration, even if the
                    // peer streams forever. Excess bytes are never written.
                    let remaining = (size - written + 1).min(buffer.len() as u64) as usize;
                    let n = stream.read(&mut buffer[..remaining]).await?;
                    if n == 0 {
                        break;
                    }
                    written += n as u64;
                    if written > size {
                        return Err(DeliveryError::TooLarge);
                    }
                    file.write_all(&buffer[..n]).await?;
                }
                if written != size {
                    return Err(DeliveryError::InvalidArtifact(
                        "truncated artifact stream".into(),
                    ));
                }
                file.flush().await?;
                file.sync_all().await?;
                drop(file);
                let bytes = self.read_exact_bounded(pending.path(), size)?;
                self.verify(&bytes, expected)?;
                // tempfile persist is an atomic same-filesystem rename. The global
                // lock prevents overwrite races; no final name ever exposes a prefix.
                pending.persist(&path).map_err(|error| error.error)?;
                sync_directory(&self.root)?;
                bytes
            }
        };
        // Neither disk presence nor authorization before transfer proves current
        // owner policy after a slow download, eviction wait or cached verification.
        self.authorize(source, &authority, workspace, request, expected)
            .await?;
        Ok(VerifiedArtifact { bytes })
    }

    async fn authorize(
        &self,
        source: &dyn ArtifactSource,
        authority: &str,
        workspace: &str,
        request: &PluginArtifactDeliveryRequest,
        expected: &ArtifactExpectation,
    ) -> Result<(), DeliveryError> {
        let metadata = source.authorize(request).await?;
        if metadata.artifact.revoked || expected.artifact.revoked {
            return Err(DeliveryError::Denied);
        }
        if source.server_authority_identity() != authority
            || metadata.workspace_id != workspace
            || metadata.request != *request
            || metadata.artifact != expected.artifact
        {
            return Err(DeliveryError::MetadataMismatch);
        }
        Ok(())
    }

    fn read_verified(
        &self,
        path: &Path,
        expected: &ArtifactExpectation,
    ) -> Result<Option<Vec<u8>>, DeliveryError> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DeliveryError::InvalidCacheRoot);
        }
        if metadata.len() != u64::from(expected.artifact.artifact_size) {
            return Ok(None);
        }
        let bytes = self.read_exact_bounded(path, metadata.len())?;
        match self.verify(&bytes, expected) {
            Ok(()) => Ok(Some(bytes)),
            Err(DeliveryError::InvalidArtifact(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn read_exact_bounded(&self, path: &Path, size: u64) -> Result<Vec<u8>, DeliveryError> {
        let mut bytes = Vec::new();
        File::open(path)?.take(size + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != size {
            return Err(DeliveryError::InvalidArtifact(
                "cached artifact size changed".into(),
            ));
        }
        Ok(bytes)
    }

    fn verify(&self, bytes: &[u8], expected: &ArtifactExpectation) -> Result<(), DeliveryError> {
        let d = &expected.artifact;
        let package = plugin::verify_plugin_artifact(
            bytes,
            &d.package_digest,
            &d.artifact_digest,
            u64::from(d.artifact_size),
            &self.limits.package,
        )
        .map_err(|error| DeliveryError::InvalidArtifact(error.message))?;
        let m = &package.package.manifest;
        let schema = plugin::plugin_config_schema_identity(m);
        if d.package_id != m.id
            || d.name != m.name
            || d.version != m.version
            || d.description != m.description
            || d.schema_version != m.schema_version
            || d.world != m.runtime.world
            || d.required_imports != m.runtime.required_imports
            || !same_json(&d.tools, &m.tools)?
            || !same_json(&d.services, &m.services)?
            || !same_json(&d.connections, &m.connections)?
            || !same_json(&d.requested_grants, &m.requested_grants)?
            || !same_json(&d.data_collections, &m.data_collections)?
            || d.config_schema.dcdl != m.config_schema
            || d.config_schema.digest != schema.digest
            || d.config_schema.identity != schema.identity
            || d.config_schema.version != schema.version
            || d.config_schema.namespace != schema.namespace
        {
            return Err(DeliveryError::MetadataMismatch);
        }
        Ok(())
    }

    fn inventory(&self) -> Result<Vec<Entry>, DeliveryError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().ok_or(DeliveryError::InvalidCacheRoot)?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(DeliveryError::InvalidCacheRoot);
            }
            if name == ".cache.lock" {
                continue;
            }
            if name.starts_with(".pending-") {
                fs::remove_file(entry.path())?;
                continue;
            }
            if !is_cache_filename(name) {
                return Err(DeliveryError::InvalidCacheRoot);
            }
            entries.push(Entry {
                path: entry.path(),
                size: metadata.len(),
                modified: metadata.modified()?,
            });
        }
        // Oldest publication first; hits do not need a second metadata ledger.
        entries.sort_by(|a, b| a.modified.cmp(&b.modified).then(a.path.cmp(&b.path)));
        Ok(entries)
    }

    fn make_room(
        &self,
        entries: &mut Vec<Entry>,
        keep: Option<&Path>,
        additional_bytes: u64,
        additional_entries: usize,
    ) -> Result<(), DeliveryError> {
        let mut bytes = entries.iter().try_fold(additional_bytes, |sum, entry| {
            sum.checked_add(entry.size).ok_or(DeliveryError::TooLarge)
        })?;
        let mut count = entries.len() + additional_entries;
        for entry in entries.iter() {
            if bytes <= self.limits.max_cache_bytes && count <= self.limits.max_entries {
                break;
            }
            if keep == Some(entry.path.as_path()) {
                continue;
            }
            fs::remove_file(&entry.path)?;
            bytes -= entry.size;
            count -= 1;
        }
        if bytes > self.limits.max_cache_bytes || count > self.limits.max_entries {
            return Err(DeliveryError::TooLarge);
        }
        Ok(())
    }
}

struct Entry {
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
}

async fn lock_cache(path: PathBuf) -> Result<File, DeliveryError> {
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (!metadata.is_file() || metadata.file_type().is_symlink())
    {
        return Err(DeliveryError::InvalidCacheRoot);
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    loop {
        if fs4::fs_std::FileExt::try_lock_exclusive(&file)? {
            return Ok(file);
        }
        // OS locks provide crash recovery; nonblocking acquisition keeps cancelled
        // waiters from occupying Tokio blocking threads after their caller exits.
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn sync_directory(root: &Path) -> Result<(), DeliveryError> {
    // Windows does not expose directory fsync through std::fs::File. Atomic
    // visibility still holds there; power-loss directory durability is Unix-only.
    #[cfg(unix)]
    File::open(root)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(is_hex)
}

fn is_cache_filename(value: &str) -> bool {
    value
        .strip_suffix(".artifact")
        .and_then(|value| value.split_once('-'))
        .is_some_and(|(authority, digest)| is_hex(authority) && is_hex(digest))
}

fn same_json<A: serde::Serialize, B: serde::Serialize>(
    a: &A,
    b: &B,
) -> Result<bool, DeliveryError> {
    let a = serde_json::to_value(a).map_err(|_| DeliveryError::MetadataMismatch)?;
    let b = serde_json::to_value(b).map_err(|_| DeliveryError::MetadataMismatch)?;
    Ok(a == b)
}

#[cfg(test)]
mod tests;
