//! Cache/real filesystem/shared Component package integration. Only owner policy
//! and transport are fixtures; canonical verification, streaming limits, locking,
//! eviction and publication use production code. Subprocess actors prove lock
//! convergence and process-exit recovery, not arbitrary power-loss durability.

use super::*;
use manifest::plugin::{CanonicalPluginArtifact, PluginSourceKind};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
};
use tokio::{io::ReadBuf, sync::Semaphore};

const MANIFEST: &str =
    include_str!("../../../../resources/plugin/templates/rust-component-tool/plugin.toml");
const COMPONENT: &[u8] =
    include_bytes!("../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm");

fn fixture() -> (CanonicalPluginArtifact, PluginArtifact) {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("plugin.toml"), MANIFEST).unwrap();
    fs::write(directory.path().join("plugin.component.wasm"), COMPONENT).unwrap();
    let limits = PluginPackageLimits::default();
    let package =
        plugin::read_plugin_directory(directory.path(), PluginSourceKind::Explicit, &limits)
            .unwrap();
    let artifact = plugin::pack_plugin_package(&package, &limits).unwrap();
    let m = package.package.manifest;
    let schema_digest = format!("sha256:{}", hash(m.config_schema.as_bytes()));
    let descriptor = PluginArtifact {
        package_id: m.id.clone(),
        name: m.name,
        version: m.version.clone(),
        description: m.description,
        package_digest: artifact.package_digest.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
        artifact_size: artifact.artifact_size.try_into().unwrap(),
        schema_version: m.schema_version,
        world: m.runtime.world,
        required_imports: m.runtime.required_imports,
        tools: serde_json::from_value(serde_json::to_value(m.tools).unwrap()).unwrap(),
        services: vec![],
        connections: vec![],
        requested_grants: vec![],
        data_collections: vec![],
        config_schema: server_api::PluginSchemaContribution {
            identity: format!("{}/config", m.id),
            version: m.version.clone(),
            namespace: format!("{}/config/{}/{}", m.id, m.version, schema_digest),
            digest: schema_digest,
            dcdl: m.config_schema,
        },
        revoked: false,
        revocation_reason: None,
    };
    (artifact, descriptor)
}

#[derive(Clone, Copy)]
enum Transfer {
    Complete,
    Short,
    Error,
    Pause,
    Crash,
}

struct Source {
    authority: String,
    metadata: Mutex<PluginArtifactDeliveryMetadata>,
    bytes: Vec<u8>,
    fetches: AtomicUsize,
    authorizations: AtomicUsize,
    deny_on: AtomicUsize,
    unavailable: AtomicUsize,
    transfer: Transfer,
    read_bytes: Arc<AtomicUsize>,
    paused: Arc<Semaphore>,
}

impl Source {
    fn new(artifact: &CanonicalPluginArtifact, descriptor: PluginArtifact) -> Self {
        Self {
            authority: "authenticated-server-a".into(),
            metadata: Mutex::new(PluginArtifactDeliveryMetadata {
                workspace_id: "workspace-a".into(),
                request: PluginArtifactDeliveryRequest {
                    plan_id: "plan-a".into(),
                    plan_fingerprint: "exact-plan-fingerprint".into(),
                    installation_id: "installation-a".into(),
                    artifact_digest: artifact.artifact_digest.clone(),
                },
                artifact: descriptor,
            }),
            bytes: artifact.bytes.clone(),
            fetches: AtomicUsize::new(0),
            authorizations: AtomicUsize::new(0),
            deny_on: AtomicUsize::new(0),
            unavailable: AtomicUsize::new(0),
            transfer: Transfer::Complete,
            read_bytes: Arc::new(AtomicUsize::new(0)),
            paused: Arc::new(Semaphore::new(0)),
        }
    }

    fn request(&self) -> PluginArtifactDeliveryRequest {
        self.metadata.lock().unwrap().request.clone()
    }

    fn expectation(&self) -> ArtifactExpectation {
        ArtifactExpectation::new(self.metadata.lock().unwrap().artifact.clone())
    }

    async fn resolve(&self, cache: &ArtifactCache) -> Result<VerifiedArtifact, DeliveryError> {
        cache
            .resolve(self, "workspace-a", &self.request(), &self.expectation())
            .await
    }
}

#[async_trait]
impl ArtifactSource for Source {
    fn server_authority_identity(&self) -> &str {
        &self.authority
    }

    async fn authorize(
        &self,
        _: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata, DeliveryError> {
        let call = self.authorizations.fetch_add(1, Ordering::SeqCst) + 1;
        if self.unavailable.load(Ordering::SeqCst) != 0 {
            return Err(DeliveryError::Unavailable("offline fixture".into()));
        }
        if call == self.deny_on.load(Ordering::SeqCst) {
            return Err(DeliveryError::Denied);
        }
        Ok(self.metadata.lock().unwrap().clone())
    }

    async fn fetch(
        &self,
        _: &PluginArtifactDeliveryRequest,
    ) -> Result<ArtifactStream, DeliveryError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        // A real bounded in-memory transport pipe fragments successful downloads.
        // Fault streams replace only that transport, not the filesystem/cache.
        if matches!(self.transfer, Transfer::Complete | Transfer::Short) {
            let (reader, mut writer) = tokio::io::duplex(127);
            let mut bytes = self.bytes.clone();
            if matches!(self.transfer, Transfer::Short) {
                bytes.truncate(bytes.len() / 2);
            }
            tokio::spawn(async move {
                for chunk in bytes.chunks(61) {
                    if writer.write_all(chunk).await.is_err() {
                        break;
                    }
                }
            });
            return Ok(Box::pin(CountingReader {
                reader,
                count: self.read_bytes.clone(),
            }));
        }
        Ok(Box::pin(FaultReader {
            bytes: self.bytes[..self.bytes.len() / 2].to_vec(),
            position: 0,
            transfer: self.transfer,
            paused: self.paused.clone(),
        }))
    }
}

struct CountingReader<R> {
    reader: R,
    count: Arc<AtomicUsize>,
}
impl<R: AsyncRead + Unpin> AsyncRead for CountingReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.reader).poll_read(cx, buffer);
        self.count
            .fetch_add(buffer.filled().len() - before, Ordering::SeqCst);
        result
    }
}

struct FaultReader {
    bytes: Vec<u8>,
    position: usize,
    transfer: Transfer,
    paused: Arc<Semaphore>,
}
impl AsyncRead for FaultReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.position < self.bytes.len() {
            let n = buffer.remaining().min(self.bytes.len() - self.position);
            buffer.put_slice(&self.bytes[self.position..self.position + n]);
            self.position += n;
            return Poll::Ready(Ok(()));
        }
        match self.transfer {
            Transfer::Error => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "interrupted fixture",
            ))),
            Transfer::Pause => {
                // Reaching this poll proves the cache consumed/wrote the prefix
                // and is awaiting further input, before any publication.
                self.paused.add_permits(1);
                Poll::Pending
            }
            Transfer::Crash => std::process::exit(73),
            _ => unreachable!(),
        }
    }
}

fn cache(directory: &tempfile::TempDir) -> ArtifactCache {
    ArtifactCache::new(directory.path().join("cache"), CacheLimits::default()).unwrap()
}

fn final_files(cache: &ArtifactCache) -> Vec<PathBuf> {
    fs::read_dir(&cache.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "artifact"))
        .collect()
}

fn pending_files(cache: &ArtifactCache) -> Vec<PathBuf> {
    fs::read_dir(&cache.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".pending-")
        })
        .collect()
}

#[tokio::test]
async fn streamed_download_and_restart_hit_return_only_verified_bytes() {
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let verified = source.resolve(&cache).await.unwrap();
    assert_eq!(verified.bytes(), artifact.bytes);
    assert_eq!(fs::read(&final_files(&cache)[0]).unwrap(), artifact.bytes);
    let restarted = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
    assert_eq!(
        source.resolve(&restarted).await.unwrap().bytes(),
        artifact.bytes
    );
    assert_eq!(source.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(source.authorizations.load(Ordering::SeqCst), 4);
    assert!(pending_files(&cache).is_empty());
}

#[tokio::test]
async fn cached_bytes_do_not_bypass_owner_outage_revocation_or_denial() {
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    source.resolve(&cache).await.unwrap();
    source.unavailable.store(1, Ordering::SeqCst);
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::Unavailable(_))
    ));
    source.unavailable.store(0, Ordering::SeqCst);
    source.deny_on.store(4, Ordering::SeqCst);
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::Denied)
    ));
    source.metadata.lock().unwrap().artifact.revoked = true;
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::Denied)
    ));
    assert_eq!(source.fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn final_authorization_denies_both_download_and_cache_hit() {
    for hit in [false, true] {
        let (artifact, descriptor) = fixture();
        let source = Source::new(&artifact, descriptor);
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        if hit {
            source.resolve(&cache).await.unwrap();
        }
        source
            .deny_on
            .store(if hit { 4 } else { 2 }, Ordering::SeqCst);
        assert!(matches!(
            source.resolve(&cache).await,
            Err(DeliveryError::Denied)
        ));
        assert_eq!(source.fetches.load(Ordering::SeqCst), 1);
        // A verified blob can remain after denial, but is not usable authority.
        assert_eq!(final_files(&cache).len(), 1);
    }
}

#[tokio::test]
async fn response_must_bind_trusted_workspace_and_every_exact_request_field() {
    for field in [
        "workspace",
        "plan",
        "fingerprint",
        "installation",
        "artifact",
    ] {
        let (artifact, descriptor) = fixture();
        let source = Source::new(&artifact, descriptor);
        let request = source.request();
        let expected = source.expectation();
        {
            let mut metadata = source.metadata.lock().unwrap();
            match field {
                "workspace" => metadata.workspace_id = "another-workspace".into(),
                "plan" => metadata.request.plan_id.push('x'),
                "fingerprint" => metadata.request.plan_fingerprint.push('x'),
                "installation" => metadata.request.installation_id.push('x'),
                "artifact" => metadata.request.artifact_digest.push('x'),
                _ => unreachable!(),
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        assert!(
            matches!(
                cache
                    .resolve(&source, "workspace-a", &request, &expected)
                    .await,
                Err(DeliveryError::MetadataMismatch)
            ),
            "{field}"
        );
        assert_eq!(source.fetches.load(Ordering::SeqCst), 0);
        assert!(final_files(&cache).is_empty());
    }
}

#[tokio::test]
async fn metadata_cannot_replace_the_host_plan_descriptor() {
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    let expected = source.expectation();
    source.metadata.lock().unwrap().artifact.version = "owner-selected-version".into();
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    assert!(matches!(
        cache
            .resolve(&source, "workspace-a", &source.request(), &expected)
            .await,
        Err(DeliveryError::MetadataMismatch)
    ));
    assert_eq!(source.fetches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn canonical_package_must_match_complete_descriptor_not_only_digests() {
    for field in [
        "id",
        "name",
        "version",
        "description",
        "api",
        "world",
        "imports",
        "tools",
        "connections",
        "grants",
        "data_collections",
        "schema_identity",
        "schema_version",
        "schema_digest",
        "schema_namespace",
        "schema_dcdl",
    ] {
        let (artifact, mut d) = fixture();
        match field {
            "id" => d.package_id.push('x'),
            "name" => d.name.push('x'),
            "version" => d.version.push('x'),
            "description" => d.description = Some("different".into()),
            "api" => d.schema_version += 1,
            "world" => d.world.push('x'),
            "imports" => d.required_imports.push("yoi:host/logging@2.0.0".into()),
            "tools" => d.tools[0].input_schema = serde_json::json!({"type":"string"}),
            "connections" => d.connections.push(server_api::PluginConnectionMetadata {
                name: "other".into(),
                methods: vec![],
                path_prefixes: vec![],
            }),
            "grants" => d
                .requested_grants
                .push(server_api::PluginGrantMetadata::Logging),
            "data_collections" => d
                .data_collections
                .push(server_api::PluginDataSchemaMetadata {
                    collection: "notes".into(),
                    schema_id: "note".into(),
                    schema_version: "1".into(),
                    schema: serde_json::json!({"type":"string"}),
                }),
            "schema_identity" => d.config_schema.identity.push('x'),
            "schema_version" => d.config_schema.version.push('x'),
            "schema_digest" => d.config_schema.digest.push('x'),
            "schema_namespace" => d.config_schema.namespace.push('x'),
            "schema_dcdl" => d.config_schema.dcdl = "{ different: String }".into(),
            _ => unreachable!(),
        }
        let source = Source::new(&artifact, d);
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        assert!(
            matches!(
                source.resolve(&cache).await,
                Err(DeliveryError::MetadataMismatch)
            ),
            "{field}"
        );
        assert!(final_files(&cache).is_empty(), "{field}");
        assert!(pending_files(&cache).is_empty(), "{field}");
    }
}

#[tokio::test]
async fn corrupted_or_wrong_size_cache_file_is_refetched_and_verified() {
    for wrong_size in [false, true] {
        let (artifact, descriptor) = fixture();
        let source = Source::new(&artifact, descriptor);
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        source.resolve(&cache).await.unwrap();
        let path = final_files(&cache).pop().unwrap();
        let mut corrupt = artifact.bytes.clone();
        if wrong_size {
            corrupt.truncate(7);
        } else {
            corrupt[0] ^= 1;
        }
        fs::write(&path, corrupt).unwrap();
        assert_eq!(
            source.resolve(&cache).await.unwrap().bytes(),
            artifact.bytes
        );
        assert_eq!(source.fetches.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(path).unwrap(), artifact.bytes);
    }
}

#[tokio::test]
async fn independent_package_and_artifact_digests_are_both_required() {
    for package_digest in [false, true] {
        let (artifact, mut descriptor) = fixture();
        if package_digest {
            descriptor.package_digest = format!("sha256:{}", "0".repeat(64));
        }
        let mut source = Source::new(&artifact, descriptor);
        if !package_digest {
            source.bytes[100] ^= 1;
        }
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        assert!(matches!(
            source.resolve(&cache).await,
            Err(DeliveryError::InvalidArtifact(_))
        ));
        assert!(final_files(&cache).is_empty());
    }
}

#[tokio::test]
async fn declared_size_is_checked_before_fetch_and_stream_is_read_only_one_byte_past_bound() {
    let (artifact, descriptor) = fixture();
    let mut source = Source::new(&artifact, descriptor);
    let directory = tempfile::tempdir().unwrap();
    let mut limits = CacheLimits::default();
    limits.package.max_package_size_bytes = artifact.artifact_size - 1;
    let cache = ArtifactCache::new(directory.path().join("cache"), limits).unwrap();
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::TooLarge)
    ));
    assert_eq!(source.fetches.load(Ordering::SeqCst), 0);
    source.bytes.extend_from_slice(&vec![0; 100_000]);
    let cache = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::TooLarge)
    ));
    assert_eq!(
        source.read_bytes.load(Ordering::SeqCst) as u64,
        artifact.artifact_size + 1
    );
    assert!(final_files(&cache).is_empty());
    assert!(pending_files(&cache).is_empty());
}

#[tokio::test]
async fn interrupted_and_failed_streams_never_publish_and_restart_can_retry() {
    for transfer in [Transfer::Short, Transfer::Error] {
        let (artifact, descriptor) = fixture();
        let mut source = Source::new(&artifact, descriptor);
        source.transfer = transfer;
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        let error = source.resolve(&cache).await.unwrap_err();
        assert!(matches!(
            error,
            DeliveryError::InvalidArtifact(_) | DeliveryError::Io(_)
        ));
        assert!(final_files(&cache).is_empty());
        assert!(pending_files(&cache).is_empty());
        source.transfer = Transfer::Complete;
        let restarted = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
        assert_eq!(
            source.resolve(&restarted).await.unwrap().bytes(),
            artifact.bytes
        );
    }
}

#[tokio::test]
async fn cancellation_drops_staging_without_exposing_a_partial_final_file() {
    let (artifact, descriptor) = fixture();
    let mut source = Source::new(&artifact, descriptor);
    source.transfer = Transfer::Pause;
    let source = Arc::new(source);
    let directory = tempfile::tempdir().unwrap();
    let cache = Arc::new(cache(&directory));
    let task = tokio::spawn({
        let source = source.clone();
        let cache = cache.clone();
        async move { source.resolve(&cache).await }
    });
    tokio::time::timeout(Duration::from_secs(5), source.paused.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(pending_files(&cache).len(), 1);
    assert!(final_files(&cache).is_empty());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(pending_files(&cache).is_empty());
    let retry = Source::new(&artifact, source.expectation().artifact);
    assert_eq!(retry.resolve(&cache).await.unwrap().bytes(), artifact.bytes);
}

#[tokio::test]
async fn stalled_transfer_deadline_releases_lock_and_staging() {
    let (artifact, descriptor) = fixture();
    let mut source = Source::new(&artifact, descriptor);
    source.transfer = Transfer::Pause;
    let directory = tempfile::tempdir().unwrap();
    let limits = CacheLimits {
        delivery_timeout: Duration::from_millis(100),
        ..CacheLimits::default()
    };
    let cache = ArtifactCache::new(directory.path().join("cache"), limits).unwrap();
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::Timeout)
    ));
    assert!(pending_files(&cache).is_empty());
    assert!(final_files(&cache).is_empty());
    source.transfer = Transfer::Complete;
    let restarted = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
    source.resolve(&restarted).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_instances_converge_on_one_verified_publication() {
    let (artifact, descriptor) = fixture();
    let source = Arc::new(Source::new(&artifact, descriptor));
    let directory = tempfile::tempdir().unwrap();
    let first = cache(&directory);
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let cache = ArtifactCache::new(&first.root, CacheLimits::default()).unwrap();
        let source = source.clone();
        tasks.push(tokio::spawn(async move { source.resolve(&cache).await }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap().unwrap().bytes(), artifact.bytes);
    }
    assert_eq!(source.fetches.load(Ordering::SeqCst), 1);
    assert_eq!(source.authorizations.load(Ordering::SeqCst), 16);
    assert_eq!(final_files(&first).len(), 1);
    assert!(pending_files(&first).is_empty());
}

#[tokio::test]
async fn restart_discards_stale_partial_even_when_it_contains_a_complete_valid_artifact() {
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    fs::write(
        cache.root.join(".pending-crashed-complete"),
        &artifact.bytes,
    )
    .unwrap();
    fs::write(
        cache.root.join(".pending-crashed-prefix"),
        &artifact.bytes[..99],
    )
    .unwrap();
    let restarted = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
    assert_eq!(
        source.resolve(&restarted).await.unwrap().bytes(),
        artifact.bytes
    );
    assert_eq!(source.fetches.load(Ordering::SeqCst), 1);
    assert!(pending_files(&cache).is_empty());
}

#[tokio::test]
async fn eviction_bounds_bytes_and_entry_count_independently_without_invalidating_returned_bytes() {
    let (artifact, descriptor) = fixture();
    for (max_cache_bytes, max_entries) in
        [(artifact.artifact_size, 8), (artifact.artifact_size * 8, 1)]
    {
        let source = Source::new(&artifact, descriptor.clone());
        let directory = tempfile::tempdir().unwrap();
        let limits = CacheLimits {
            max_cache_bytes,
            max_entries,
            ..CacheLimits::default()
        };
        let cache = ArtifactCache::new(directory.path().join("cache"), limits).unwrap();
        let held = source.resolve(&cache).await.unwrap();
        let mut other = Source::new(&artifact, descriptor.clone());
        other.authority = "server-b".into();
        other.resolve(&cache).await.unwrap();
        assert_eq!(final_files(&cache).len(), 1);
        assert_eq!(
            fs::metadata(&final_files(&cache)[0]).unwrap().len(),
            artifact.artifact_size
        );
        assert_eq!(held.bytes(), artifact.bytes);
        source.resolve(&cache).await.unwrap();
        assert_eq!(source.fetches.load(Ordering::SeqCst), 2);
        assert_eq!(final_files(&cache).len(), 1);
    }
}

#[tokio::test]
async fn hashed_authority_separates_servers_while_fresh_workspace_and_worker_owners_are_required() {
    let (artifact, descriptor) = fixture();
    let mut source = Source::new(&artifact, descriptor.clone());
    source.authority = "../../outside/opaque:server".into();
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    source.resolve(&cache).await.unwrap();
    let mut other = Source::new(&artifact, descriptor.clone());
    other.authority = "server-b".into();
    other.resolve(&cache).await.unwrap();
    assert_eq!(final_files(&cache).len(), 2);
    for path in final_files(&cache) {
        assert_eq!(path.parent().unwrap(), cache.root);
        assert!(is_cache_filename(
            path.file_name().unwrap().to_str().unwrap()
        ));
        assert!(
            !path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .contains("outside")
        );
    }
    let mut workspace_b = Source::new(&artifact, descriptor.clone());
    workspace_b.authority = source.authority.clone();
    workspace_b.metadata.lock().unwrap().workspace_id = "workspace-b".into();
    let request = workspace_b.request();
    let expected = workspace_b.expectation();
    assert_eq!(
        cache
            .resolve(&workspace_b, "workspace-b", &request, &expected)
            .await
            .unwrap()
            .bytes(),
        artifact.bytes
    );
    assert_eq!(workspace_b.fetches.load(Ordering::SeqCst), 0);
    assert_eq!(workspace_b.authorizations.load(Ordering::SeqCst), 2);
    let mut different_worker_owner = Source::new(&artifact, descriptor);
    different_worker_owner.authority = source.authority;
    different_worker_owner.deny_on.store(1, Ordering::SeqCst);
    assert!(matches!(
        different_worker_owner.resolve(&cache).await,
        Err(DeliveryError::Denied)
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn private_root_and_non_symlink_cache_entries_are_enforced() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let public = directory.path().join("public");
    fs::create_dir(&public).unwrap();
    fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        ArtifactCache::new(&public, CacheLimits::default()),
        Err(DeliveryError::InvalidCacheRoot)
    ));
    let link = directory.path().join("link");
    symlink(&public, &link).unwrap();
    assert!(matches!(
        ArtifactCache::new(&link, CacheLimits::default()),
        Err(DeliveryError::InvalidCacheRoot)
    ));
    let cache = cache(&directory);
    assert_eq!(
        fs::metadata(&cache.root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let sentinel = directory.path().join("sentinel");
    fs::write(&sentinel, b"do not change").unwrap();
    symlink(&sentinel, cache.root.join(".pending-symlink")).unwrap();
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    assert!(matches!(
        source.resolve(&cache).await,
        Err(DeliveryError::InvalidCacheRoot)
    ));
    assert_eq!(fs::read(sentinel).unwrap(), b"do not change");
}

// Deliberately corrupt the shared package's API without breaking ZIP integrity,
// so rejection must come from the common manifest verifier, not a bad checksum.
fn unsupported_api_bytes(bytes: &mut [u8]) {
    let header = bytes
        .windows(4)
        .position(|window| window == b"PK\x03\x04")
        .unwrap();
    let mut offset = header;
    let mut crc = None;
    while &bytes[offset..offset + 4] == b"PK\x03\x04" {
        let size = u32::from_le_bytes(bytes[offset + 18..offset + 22].try_into().unwrap()) as usize;
        let name_size =
            u16::from_le_bytes(bytes[offset + 26..offset + 28].try_into().unwrap()) as usize;
        let extra =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let start = offset + 30 + name_size + extra;
        if &bytes[offset + 30..offset + 30 + name_size] == b"plugin.toml" {
            let payload = &mut bytes[start..start + size];
            let index = payload
                .windows(b"schema_version = 2".len())
                .position(|window| window == b"schema_version = 2")
                .unwrap();
            payload[index + b"schema_version = ".len()] = b'9';
            let mut checksum = !0_u32;
            for byte in payload {
                checksum ^= u32::from(*byte);
                for _ in 0..8 {
                    checksum = (checksum >> 1) ^ (0xedb88320 & 0_u32.wrapping_sub(checksum & 1));
                }
            }
            let checksum = (!checksum).to_le_bytes();
            bytes[offset + 14..offset + 18].copy_from_slice(&checksum);
            crc = Some(checksum);
        }
        offset = start + size;
    }
    while &bytes[offset..offset + 4] == b"PK\x01\x02" {
        let name_size =
            u16::from_le_bytes(bytes[offset + 28..offset + 30].try_into().unwrap()) as usize;
        let extra =
            u16::from_le_bytes(bytes[offset + 30..offset + 32].try_into().unwrap()) as usize;
        let comment =
            u16::from_le_bytes(bytes[offset + 32..offset + 34].try_into().unwrap()) as usize;
        if &bytes[offset + 46..offset + 46 + name_size] == b"plugin.toml" {
            bytes[offset + 16..offset + 20].copy_from_slice(&crc.unwrap());
        }
        offset += 46 + name_size + extra + comment;
    }
}

#[tokio::test]
async fn unsupported_api_is_rejected_by_common_verifier_before_publication() {
    let (mut artifact, mut descriptor) = fixture();
    unsupported_api_bytes(&mut artifact.bytes);
    artifact.artifact_digest = format!("sha256:{}", hash(&artifact.bytes));
    descriptor.artifact_digest = artifact.artifact_digest.clone();
    descriptor.schema_version = 9;
    let source = Source::new(&artifact, descriptor);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let error = source.resolve(&cache).await.unwrap_err();
    assert!(
        matches!(error, DeliveryError::InvalidArtifact(ref message) if message.contains("unsupported schema")),
        "{error:?}"
    );
    assert!(final_files(&cache).is_empty());
}

struct Actor {
    child: Child,
    lines: std::sync::mpsc::Receiver<Option<String>>,
    stdout_thread: Option<std::thread::JoinHandle<()>>,
    stderr_thread: Option<std::thread::JoinHandle<String>>,
    text: String,
}
impl Drop for Actor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.stdout_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }
}
impl Actor {
    fn spawn(root: &Path, action: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "delivery::tests::process_cache_actor",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        serde_json::to_writer(child.stdin.take().unwrap(), &(root, action)).unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (sender, lines) = std::sync::mpsc::channel();
        let stdout_thread = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(Some(line.unwrap())).is_err() {
                    return;
                }
            }
            let _ = sender.send(None);
        });
        let stderr_thread = std::thread::spawn(move || {
            let mut text = String::new();
            stderr.read_to_string(&mut text).unwrap();
            text
        });
        Self {
            child,
            lines,
            stdout_thread: Some(stdout_thread),
            stderr_thread: Some(stderr_thread),
            text: String::new(),
        }
    }

    fn line(&mut self) -> Option<String> {
        let line = self
            .lines
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|error| panic!("child output deadline: {error}: {}", self.text));
        if let Some(line) = &line {
            self.text.push_str(line);
            self.text.push('\n');
        }
        line
    }

    fn ready(&mut self) {
        loop {
            let line = self.line().expect("child exited before readiness");
            if line.contains("cache-actor-ready") {
                break;
            }
        }
    }

    fn finish(&mut self) -> (std::process::ExitStatus, String) {
        while self.line().is_some() {}
        let status = self.child.wait().unwrap();
        self.text
            .push_str(&self.stderr_thread.take().unwrap().join().unwrap());
        (status, std::mem::take(&mut self.text))
    }
}

#[test]
#[ignore = "subprocess actor invoked by ordinary cache integration tests"]
fn process_cache_actor() {
    let (root, action): (PathBuf, String) = serde_json::from_reader(io::stdin()).unwrap();
    let (artifact, descriptor) = fixture();
    let mut source = Source::new(&artifact, descriptor);
    if action == "crash" {
        source.transfer = Transfer::Crash;
    }
    let cache = ArtifactCache::new(root, CacheLimits::default()).unwrap();
    if action == "resolve" {
        let probe = OpenOptions::new()
            .read(true)
            .write(true)
            .open(cache.root.join(".cache.lock"))
            .unwrap();
        assert!(!fs4::fs_std::FileExt::try_lock_exclusive(&probe).unwrap());
    }
    println!("cache-actor-ready");
    io::stdout().flush().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let verified = runtime.block_on(source.resolve(&cache)).unwrap();
    assert_eq!(verified.bytes(), artifact.bytes);
    println!(
        "cache-actor-fetches={}",
        source.fetches.load(Ordering::SeqCst)
    );
}

#[tokio::test]
async fn independent_processes_converge_after_contending_for_same_cache_lock() {
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let lock = lock_cache(cache.root.join(".cache.lock")).await.unwrap();
    let mut first = Actor::spawn(&cache.root, "resolve");
    let mut second = Actor::spawn(&cache.root, "resolve");
    first.ready();
    second.ready();
    drop(lock);
    let (a, a_text) = first.finish();
    let (b, b_text) = second.finish();
    assert!(a.success(), "{a_text}");
    assert!(b.success(), "{b_text}");
    assert_eq!(
        format!("{a_text}{b_text}")
            .matches("cache-actor-fetches=1")
            .count(),
        1
    );
    assert_eq!(
        format!("{a_text}{b_text}")
            .matches("cache-actor-fetches=0")
            .count(),
        1
    );
    assert_eq!(final_files(&cache).len(), 1);
    assert!(pending_files(&cache).is_empty());
}

#[tokio::test]
async fn process_exit_before_publish_leaves_no_final_bytes_and_restart_discards_partial() {
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let mut actor = Actor::spawn(&cache.root, "crash");
    actor.ready();
    let (status, text) = actor.finish();
    assert_eq!(status.code(), Some(73), "{text}");
    assert!(final_files(&cache).is_empty());
    assert_eq!(pending_files(&cache).len(), 1);
    let (artifact, descriptor) = fixture();
    let source = Source::new(&artifact, descriptor);
    let restarted = ArtifactCache::new(&cache.root, CacheLimits::default()).unwrap();
    assert_eq!(
        source.resolve(&restarted).await.unwrap().bytes(),
        artifact.bytes
    );
    assert!(pending_files(&cache).is_empty());
    assert_eq!(final_files(&cache).len(), 1);
}
