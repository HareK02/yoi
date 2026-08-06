//! Worker-state-backed discovery and restore tools.
//!
//! This surface deliberately does not enumerate every Worker on the host. The
//! listing path starts from the caller's visibility set (the caller itself and
//! Workers it spawned according to durable Worker state) and only then reads each
//! Worker's own state. Name-targeted operations distinguish missing state from
//! state that exists but is outside that visibility set.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client::WorkerRuntimeCommand;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{Permission, ScopeRule};
use protocol::stream::JsonLineReader;
use protocol::{Event, Method, WorkerStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::{SegmentId, SessionId};
use session_store::{
    WorkerActiveSegmentRef, WorkerMetadata, WorkerMetadataStore, validate_worker_name,
};
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::runtime::worker_allocation;
use crate::spawn::comm_tools::connect_and_send;
use crate::spawn::registry::SpawnedWorkerRegistry;

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const RESTORE_START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct WorkerDiscovery<St> {
    store: St,
    self_worker_name: String,
    runtime_base: PathBuf,
    cwd: Option<PathBuf>,
    store_dir: Option<PathBuf>,
}

impl<St> WorkerDiscovery<St>
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    pub fn new(
        store: St,
        self_worker_name: String,
        runtime_base: PathBuf,
        cwd: Option<PathBuf>,
        _spawned_registry: Arc<SpawnedWorkerRegistry>,
    ) -> Self {
        let store_dir = store.root_dir();
        Self {
            store,
            self_worker_name,
            runtime_base,
            cwd,
            store_dir,
        }
    }

    pub async fn list_visible(&self) -> Result<Vec<VisibleWorkerItem>, WorkerDiscoveryError> {
        let visibility = self.visibility().await?;
        let mut items = Vec::with_capacity(visibility.visible.len());
        for worker_name in visibility.visible.keys() {
            items.push(
                self.build_item_for_visible_name(worker_name, &visibility)
                    .await,
            );
        }
        Ok(items)
    }

    pub async fn inspect(&self, worker_name: &str) -> Result<WorkerDetail, WorkerDiscoveryError> {
        let visibility = self.visibility().await?;
        let known_names = self.store.list_names()?;
        let state_exists = known_names.iter().any(|n| n == worker_name);
        if !state_exists {
            return Err(WorkerDiscoveryError::StateMissing {
                worker_name: worker_name.to_string(),
            });
        }
        if !visibility.visible.contains_key(worker_name) {
            return Err(WorkerDiscoveryError::NotVisible {
                worker_name: worker_name.to_string(),
            });
        }

        match self.store.read_by_name(worker_name)? {
            Some(metadata) => Ok(self.detail_from_metadata(metadata, &visibility).await),
            None => Err(WorkerDiscoveryError::StateMissing {
                worker_name: worker_name.to_string(),
            }),
        }
    }

    pub async fn restore(&self, worker_name: &str) -> Result<RestoreResult, WorkerDiscoveryError> {
        match self.plan_restore(worker_name).await? {
            RestorePlan::AlreadyLive {
                worker_name,
                socket_path,
                status,
            } => Ok(RestoreResult::AlreadyLive {
                worker_name,
                socket_path,
                status,
            }),
            RestorePlan::Restore {
                worker_name,
                socket_path,
            } => {
                self.spawn_restore_process(&worker_name, &socket_path)
                    .await?;
                Ok(RestoreResult::Restored {
                    worker_name,
                    socket_path,
                })
            }
        }
    }

    pub async fn plan_restore(
        &self,
        worker_name: &str,
    ) -> Result<RestorePlan, WorkerDiscoveryError> {
        let detail = self.inspect(worker_name).await?;
        if detail.live.reachable {
            return Ok(RestorePlan::AlreadyLive {
                worker_name: worker_name.to_string(),
                socket_path: detail.live.socket_path,
                status: detail.live.status,
            });
        }

        let active = detail
            .active
            .ok_or_else(|| WorkerDiscoveryError::NotRestorable {
                worker_name: worker_name.to_string(),
                reason: "worker state has no active session".into(),
            })?;
        let segment_id = active
            .segment_id
            .ok_or_else(|| WorkerDiscoveryError::NotRestorable {
                worker_name: worker_name.to_string(),
                reason: "worker state has an active session but no active segment yet".into(),
            })?;

        if let Some(lock) = lookup_segment_lock(segment_id)? {
            let lock_live = probe_socket(&lock.socket).await;
            return if lock_live.reachable {
                Ok(RestorePlan::AlreadyLive {
                    worker_name: lock.worker_name,
                    socket_path: lock.socket,
                    status: lock_live.status,
                })
            } else {
                Err(WorkerDiscoveryError::LockConflict {
                    worker_name: worker_name.to_string(),
                    segment_id,
                    owner_worker: lock.worker_name,
                    socket_path: lock.socket,
                    pid: lock.pid,
                })
            };
        }

        Ok(RestorePlan::Restore {
            worker_name: worker_name.to_string(),
            socket_path: self.default_socket_path(worker_name),
        })
    }

    pub fn register_peer(
        &self,
        peer_name: &str,
    ) -> Result<PeerRegistrationResult, WorkerDiscoveryError> {
        self.ensure_existing_peer(peer_name)?
            .ok_or_else(|| WorkerDiscoveryError::MissingWorker {
                worker_name: peer_name.to_string(),
            })
    }

    pub fn ensure_existing_peer(
        &self,
        peer_name: &str,
    ) -> Result<Option<PeerRegistrationResult>, WorkerDiscoveryError> {
        validate_worker_name(peer_name)?;
        if peer_name == self.self_worker_name {
            return Err(WorkerDiscoveryError::SelfPeer {
                worker_name: peer_name.to_string(),
            });
        }
        let self_metadata = self
            .store
            .read_by_name(&self.self_worker_name)?
            .ok_or_else(|| WorkerDiscoveryError::StateMissing {
                worker_name: self.self_worker_name.clone(),
            })?;
        let prior_self_peers = self_metadata.peers.clone();
        if self.store.read_by_name(peer_name)?.is_none() {
            return Ok(None);
        }

        self.store.add_peer(&self.self_worker_name, peer_name)?;
        if let Err(error) = self.store.add_peer(peer_name, &self.self_worker_name) {
            let _ = self
                .store
                .set_peers(&self.self_worker_name, prior_self_peers);
            return Err(WorkerDiscoveryError::WorkerStore(error));
        }

        Ok(Some(PeerRegistrationResult {
            source: self.self_worker_name.clone(),
            peer: peer_name.to_string(),
        }))
    }

    async fn visibility(&self) -> Result<VisibilitySet, WorkerDiscoveryError> {
        let mut visible = BTreeMap::new();
        let mut child_sockets = BTreeMap::new();
        let mut comm_registry = BTreeMap::new();
        visible.insert(self.self_worker_name.clone(), VisibilityReason::SelfWorker);

        // Durable parent -> child state is the primary visibility source.
        if let Some(metadata) = self.store.read_by_name(&self.self_worker_name)? {
            for child in metadata.spawned_children {
                visible
                    .entry(child.worker_name.clone())
                    .or_insert(VisibilityReason::SpawnedChild);
                child_sockets.insert(child.worker_name.clone(), child.socket_path.clone());
                comm_registry.insert(
                    child.worker_name.clone(),
                    comm_info_from_spawned_child(&child),
                );
            }
            for peer in metadata.peers {
                visible
                    .entry(peer.worker_name)
                    .or_insert(VisibilityReason::Peer);
            }
        }

        Ok(VisibilitySet {
            visible,
            child_sockets,
            comm_registry,
        })
    }

    async fn build_item_for_visible_name(
        &self,
        worker_name: &str,
        visibility: &VisibilitySet,
    ) -> VisibleWorkerItem {
        let visibility_reason = visibility.reason_for(worker_name);
        match self.store.read_by_name(worker_name) {
            Ok(Some(metadata)) => {
                let detail = self.detail_from_metadata(metadata, visibility).await;
                VisibleWorkerItem {
                    worker_name: worker_name.to_string(),
                    visibility: visibility_reason,
                    state: WorkerStateStatus::Readable,
                    active: detail.active,
                    live: detail.live,
                    restore: detail.restore,
                    comm_registry: detail.comm_registry,
                    spawned_children: detail.spawned_children,
                    error: None,
                }
            }
            Ok(None) => VisibleWorkerItem {
                worker_name: worker_name.to_string(),
                visibility: visibility_reason,
                state: WorkerStateStatus::Missing,
                active: None,
                live: self.live_for_name(worker_name, None).await,
                restore: RestoreInfo::not_possible("worker state missing"),
                comm_registry: visibility.comm_info_for(worker_name),
                spawned_children: SpawnedChildrenSummary::default(),
                error: None,
            },
            Err(error) => VisibleWorkerItem {
                worker_name: worker_name.to_string(),
                visibility: visibility_reason,
                state: WorkerStateStatus::Corrupt,
                active: None,
                live: self.live_for_name(worker_name, None).await,
                restore: RestoreInfo::not_possible("worker state is unreadable"),
                comm_registry: visibility.comm_info_for(worker_name),
                spawned_children: SpawnedChildrenSummary::default(),
                error: Some(error.to_string()),
            },
        }
    }

    async fn detail_from_metadata(
        &self,
        metadata: WorkerMetadata,
        visibility: &VisibilitySet,
    ) -> WorkerDetail {
        let child_socket = visibility.child_socket_for(&metadata.worker_name);
        let live = self
            .live_for_name(&metadata.worker_name, child_socket.as_deref())
            .await;
        let restore = self
            .restore_info(&metadata.worker_name, metadata.active.as_ref())
            .await;
        let spawned_children = summarize_spawned_children(&metadata.spawned_children).await;
        WorkerDetail {
            worker_name: metadata.worker_name.clone(),
            visibility: visibility.reason_for(&metadata.worker_name),
            active: metadata.active.map(ActivePointer::from),
            live,
            restore,
            comm_registry: visibility.comm_info_for(&metadata.worker_name),
            spawned_children,
        }
    }

    async fn restore_info(
        &self,
        worker_name: &str,
        active: Option<&WorkerActiveSegmentRef>,
    ) -> RestoreInfo {
        let Some(active) = active else {
            return RestoreInfo::not_possible("worker state has no active session");
        };
        let Some(segment_id) = active.segment_id else {
            return RestoreInfo::not_possible("active segment is not known yet");
        };
        match lookup_segment_lock(segment_id) {
            Ok(Some(lock)) => RestoreInfo {
                possible: false,
                restore_name: Some(worker_name.to_string()),
                reason: Some(format!(
                    "segment is currently locked by `{}` (pid {})",
                    lock.worker_name, lock.pid
                )),
            },
            Ok(None) => RestoreInfo {
                possible: true,
                restore_name: Some(worker_name.to_string()),
                reason: None,
            },
            Err(error) => RestoreInfo {
                possible: false,
                restore_name: Some(worker_name.to_string()),
                reason: Some(format!("lock lookup failed: {error}")),
            },
        }
    }

    pub async fn send_weak_notify_to_live_peer(
        &self,
        peer_name: &str,
        message: String,
    ) -> WeakNotifyDelivery {
        let detail = match self.inspect(peer_name).await {
            Ok(detail) => detail,
            Err(
                WorkerDiscoveryError::StateMissing { .. }
                | WorkerDiscoveryError::MissingWorker { .. },
            ) => {
                return WeakNotifyDelivery::SkippedMissing;
            }
            Err(WorkerDiscoveryError::NotVisible { .. }) => {
                return WeakNotifyDelivery::SkippedNotVisible;
            }
            Err(error) => {
                return WeakNotifyDelivery::SendFailed {
                    error: error.to_string(),
                };
            }
        };
        if detail.visibility != VisibilityReason::Peer {
            return WeakNotifyDelivery::SkippedNotPeer {
                visibility: detail.visibility,
            };
        }
        if !detail.live.reachable {
            return WeakNotifyDelivery::SkippedNotLive {
                reason: detail.live.error,
            };
        }
        match send_notify(&detail.live.socket_path, message, false).await {
            Ok(()) => WeakNotifyDelivery::Delivered,
            Err(error) => WeakNotifyDelivery::SendFailed {
                error: error.to_string(),
            },
        }
    }

    async fn live_for_name(&self, worker_name: &str, socket_override: Option<&Path>) -> LiveInfo {
        let socket_path = socket_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.default_socket_path(worker_name));
        probe_socket(&socket_path).await
    }

    fn default_socket_path(&self, worker_name: &str) -> PathBuf {
        self.runtime_base.join(worker_name).join("sock")
    }

    async fn spawn_restore_process(
        &self,
        worker_name: &str,
        socket_path: &Path,
    ) -> Result<(), WorkerDiscoveryError> {
        let runtime_command =
            WorkerRuntimeCommand::resolve().map_err(WorkerDiscoveryError::RestoreSpawn)?;
        let Some(cwd) = &self.cwd else {
            return Err(WorkerDiscoveryError::NotRestorable {
                worker_name: worker_name.to_string(),
                reason: "restore requires local Worker filesystem authority".into(),
            });
        };
        let mut command = Command::new(runtime_command.program());
        command
            .args(runtime_command.prefix_args())
            .arg("--worker")
            .arg(worker_name)
            .arg("--require-worker-state")
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        if let Some(store_dir) = &self.store_dir {
            command.arg("--store").arg(store_dir);
        }

        let mut child =
            command
                .spawn()
                .map_err(|source| WorkerDiscoveryError::RestoreLaunchFailed {
                    command: runtime_command.clone(),
                    source,
                })?;
        let deadline = tokio::time::Instant::now() + RESTORE_START_TIMEOUT;
        loop {
            if probe_socket(socket_path).await.reachable {
                tokio::spawn(async move {
                    let _ = child.wait().await;
                });
                return Ok(());
            }
            if let Some(status) = child
                .try_wait()
                .map_err(WorkerDiscoveryError::RestoreSpawn)?
            {
                return Err(WorkerDiscoveryError::RestoreExited { status });
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(WorkerDiscoveryError::RestoreTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityReason {
    SelfWorker,
    SpawnedChild,
    Peer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStateStatus {
    Readable,
    Missing,
    Corrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivePointer {
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<SegmentId>,
}

impl From<WorkerActiveSegmentRef> for ActivePointer {
    fn from(value: WorkerActiveSegmentRef) -> Self {
        Self {
            session_id: value.session_id,
            segment_id: value.segment_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveInfo {
    pub socket_path: PathBuf,
    pub reachable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<WorkerStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RestoreInfo {
    pub possible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RestoreInfo {
    fn not_possible(reason: impl Into<String>) -> Self {
        Self {
            possible: false,
            restore_name: None,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpawnedChildrenSummary {
    pub count: usize,
    pub reachable: usize,
    pub unreachable: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommRegistryInfo {
    pub registered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope_delegated: Vec<ScopeRule>,
}

impl CommRegistryInfo {
    fn missing() -> Self {
        Self {
            registered: false,
            socket_path: None,
            scope_delegated: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VisibleWorkerItem {
    pub worker_name: String,
    pub visibility: VisibilityReason,
    pub state: WorkerStateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActivePointer>,
    pub live: LiveInfo,
    pub restore: RestoreInfo,
    pub comm_registry: CommRegistryInfo,
    pub spawned_children: SpawnedChildrenSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerDetail {
    pub worker_name: String,
    pub visibility: VisibilityReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActivePointer>,
    pub live: LiveInfo,
    pub restore: RestoreInfo,
    pub comm_registry: CommRegistryInfo,
    pub spawned_children: SpawnedChildrenSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RestorePlan {
    AlreadyLive {
        worker_name: String,
        socket_path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<WorkerStatus>,
    },
    Restore {
        worker_name: String,
        socket_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RestoreResult {
    AlreadyLive {
        worker_name: String,
        socket_path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<WorkerStatus>,
    },
    Restored {
        worker_name: String,
        socket_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRegistrationResult {
    pub source: String,
    pub peer: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeakNotifyDelivery {
    Delivered,
    SkippedMissing,
    SkippedNotVisible,
    SkippedNotPeer { visibility: VisibilityReason },
    SkippedNotLive { reason: Option<String> },
    SendFailed { error: String },
}

impl WeakNotifyDelivery {
    pub fn delivered(&self) -> bool {
        matches!(self, WeakNotifyDelivery::Delivered)
    }
}

impl fmt::Display for WeakNotifyDelivery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WeakNotifyDelivery::Delivered => write!(f, "delivered"),
            WeakNotifyDelivery::SkippedMissing => {
                write!(f, "skipped: target worker metadata is missing")
            }
            WeakNotifyDelivery::SkippedNotVisible => {
                write!(f, "skipped: target worker is not visible")
            }
            WeakNotifyDelivery::SkippedNotPeer { visibility } => {
                write!(
                    f,
                    "skipped: target worker is visible as {visibility:?}, not peer"
                )
            }
            WeakNotifyDelivery::SkippedNotLive { reason } => {
                if let Some(reason) = reason {
                    write!(f, "skipped: target peer is not live/reachable ({reason})")
                } else {
                    write!(f, "skipped: target peer is not live/reachable")
                }
            }
            WeakNotifyDelivery::SendFailed { error } => write!(f, "send failed: {error}"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkerDiscoveryError {
    #[error("worker state missing for `{worker_name}`")]
    StateMissing { worker_name: String },
    #[error("worker `{worker_name}` is not visible to this Worker")]
    NotVisible { worker_name: String },
    #[error("worker `{worker_name}` is not restorable: {reason}")]
    NotRestorable { worker_name: String, reason: String },
    #[error("worker `{worker_name}` cannot be registered as a peer of itself")]
    SelfPeer { worker_name: String },
    #[error("worker `{worker_name}` does not exist")]
    MissingWorker { worker_name: String },
    #[error(
        "worker `{worker_name}` segment {segment_id} is locked by `{owner_worker}` pid {pid} at {socket_path}"
    )]
    LockConflict {
        worker_name: String,
        segment_id: SegmentId,
        owner_worker: String,
        socket_path: PathBuf,
        pid: u32,
    },
    #[error("session store error: {0}")]
    Store(#[from] session_store::StoreError),
    #[error("worker store error: {0}")]
    WorkerStore(#[from] session_store::WorkerStoreError),
    #[error("scope lock error: {0}")]
    ScopeLock(#[from] worker_allocation::ScopeLockError),
    #[error("failed to launch restore process: {0}")]
    RestoreSpawn(io::Error),
    #[error("failed to launch restore runtime command `{command}`: {source}")]
    RestoreLaunchFailed {
        command: WorkerRuntimeCommand,
        #[source]
        source: io::Error,
    },
    #[error("restore process exited before socket became reachable: {status}")]
    RestoreExited { status: std::process::ExitStatus },
    #[error("restore process did not become reachable before timeout")]
    RestoreTimeout,
}

struct VisibilitySet {
    visible: BTreeMap<String, VisibilityReason>,
    child_sockets: BTreeMap<String, PathBuf>,
    comm_registry: BTreeMap<String, CommRegistryInfo>,
}

impl VisibilitySet {
    fn reason_for(&self, worker_name: &str) -> VisibilityReason {
        self.visible
            .get(worker_name)
            .cloned()
            .unwrap_or(VisibilityReason::SpawnedChild)
    }

    fn child_socket_for(&self, worker_name: &str) -> Option<PathBuf> {
        self.child_sockets.get(worker_name).cloned()
    }

    fn comm_info_for(&self, worker_name: &str) -> CommRegistryInfo {
        self.comm_registry
            .get(worker_name)
            .cloned()
            .unwrap_or_else(CommRegistryInfo::missing)
    }
}

fn comm_info_from_spawned_child(child: &session_store::WorkerSpawnedChild) -> CommRegistryInfo {
    let scope_delegated = child
        .scope_delegated
        .iter()
        .filter_map(|rule| {
            let permission = match rule.permission.as_str() {
                "read" => Permission::Read,
                "write" => Permission::Write,
                _ => return None,
            };
            Some(ScopeRule {
                target: rule.target.clone(),
                permission,
                recursive: rule.recursive,
            })
        })
        .collect();
    CommRegistryInfo {
        registered: true,
        socket_path: Some(child.socket_path.clone()),
        scope_delegated,
    }
}

async fn summarize_spawned_children(
    children: &[session_store::WorkerSpawnedChild],
) -> SpawnedChildrenSummary {
    let mut summary = SpawnedChildrenSummary {
        count: children.len(),
        ..Default::default()
    };
    for child in children {
        if probe_socket(&child.socket_path).await.reachable {
            summary.reachable += 1;
        } else {
            summary.unreachable += 1;
        }
    }
    summary
}

async fn probe_socket(socket_path: &Path) -> LiveInfo {
    match tokio::time::timeout(PROBE_TIMEOUT, UnixStream::connect(socket_path)).await {
        Ok(Ok(stream)) => {
            let (r, _w) = stream.into_split();
            let mut reader = JsonLineReader::new(r);
            let mut status = None;
            loop {
                match tokio::time::timeout(PROBE_TIMEOUT, reader.next::<Event>()).await {
                    Ok(Ok(Some(Event::Snapshot {
                        status: snapshot_status,
                        ..
                    }))) => {
                        status = Some(snapshot_status);
                        break;
                    }
                    Ok(Ok(Some(Event::Alert(_)))) => continue,
                    Ok(Ok(Some(_))) | Ok(Ok(None)) | Ok(Err(_)) | Err(_) => break,
                }
            }
            LiveInfo {
                socket_path: socket_path.to_path_buf(),
                reachable: true,
                status,
                error: None,
            }
        }
        Ok(Err(error)) => LiveInfo {
            socket_path: socket_path.to_path_buf(),
            reachable: false,
            status: None,
            error: Some(error.to_string()),
        },
        Err(_) => LiveInfo {
            socket_path: socket_path.to_path_buf(),
            reachable: false,
            status: None,
            error: Some("connect timed out".into()),
        },
    }
}

fn lookup_segment_lock(
    segment_id: SegmentId,
) -> Result<Option<worker_allocation::SegmentLockInfo>, worker_allocation::ScopeLockError> {
    worker_allocation::lookup_segment(segment_id)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct WorkerNameInput {
    /// Worker name to restore.
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SendToPeerWorkerInput {
    /// Target peer Worker name.
    name: String,
    /// Text delivered to the peer as a peer notification.
    message: String,
}

struct ListWorkersTool<St> {
    discovery: WorkerDiscovery<St>,
}

#[async_trait]
impl<St> Tool for ListWorkersTool<St>
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let items = self
            .discovery
            .list_visible()
            .await
            .map_err(discovery_error_to_tool_error)?;
        let summary = format!("{} visible worker(s)", items.len());
        Ok(ToolOutput {
            summary,
            content: Some(json_content(&items)?),
        })
    }
}

struct RestoreWorkerTool<St> {
    discovery: WorkerDiscovery<St>,
}

#[async_trait]
impl<St> Tool for RestoreWorkerTool<St>
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: WorkerNameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid RestoreWorker input: {e}")))?;
        let result = self
            .discovery
            .restore(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        let summary = match &result {
            RestoreResult::AlreadyLive { worker_name, .. } => {
                format!("worker `{worker_name}` is already live")
            }
            RestoreResult::Restored { worker_name, .. } => {
                format!("worker `{worker_name}` restored from worker state")
            }
        };
        Ok(ToolOutput {
            summary,
            content: Some(json_content(&result)?),
        })
    }
}

pub fn list_workers_tool<St>(discovery: WorkerDiscovery<St>) -> ToolDefinition
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("ListWorkers")
            .description(
                "List Workers visible to this Worker from durable Worker state, peer metadata, and the spawned-child registry. This does not expose the host-wide Worker universe.",
            )
            .input_schema(serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }));
        let tool: Arc<dyn Tool> = Arc::new(ListWorkersTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

pub fn restore_worker_tool<St>(discovery: WorkerDiscovery<St>) -> ToolDefinition
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("RestoreWorker")
            .description(
                "Restore a visible stopped/restorable Worker, or report that a visible Worker is already live. Missing state is an error.",
            )
            .input_schema(serde_json::to_value(schemars::schema_for!(WorkerNameInput)).unwrap());
        let tool: Arc<dyn Tool> = Arc::new(RestoreWorkerTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

const SEND_TO_PEER_POD_DESCRIPTION: &str = "Send a text message to a peer Worker made visible by explicit reciprocal peer metadata. The message is delivered as a peer notification through the target Worker's durable notification/history path. This does not grant delegated scope, create a spawned-child output cursor, imply parent ownership, or produce child completion notifications. Fails clearly if the target is not a visible live peer; it does not auto-restore stopped peers.";

struct SendToPeerWorkerTool<St> {
    discovery: WorkerDiscovery<St>,
}

#[async_trait]
impl<St> Tool for SendToPeerWorkerTool<St>
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SendToPeerWorkerInput = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid SendToPeerWorker input: {e}"))
        })?;
        let detail = self
            .discovery
            .inspect(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        if detail.visibility != VisibilityReason::Peer {
            return Err(ToolError::InvalidArgument(format!(
                "worker `{}` is visible as {:?}, not as a peer",
                input.name, detail.visibility
            )));
        }
        if !detail.live.reachable {
            return Err(ToolError::ExecutionFailed(format!(
                "peer worker `{}` is not live/reachable; restore it before sending",
                input.name
            )));
        }

        let message = format!(
            "[Peer message from `{}`]\n{}",
            self.discovery.self_worker_name, input.message
        );
        send_peer_notify(&detail.live.socket_path, message)
            .await
            .map_err(|error| {
                ToolError::ExecutionFailed(format!("send to peer `{}`: {error}", input.name))
            })?;

        Ok(ToolOutput {
            summary: format!("sent peer message to `{}`", input.name),
            content: None,
        })
    }
}

pub fn send_to_peer_worker_tool<St>(discovery: WorkerDiscovery<St>) -> ToolDefinition
where
    St: WorkerMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("SendToPeerWorker")
            .description(SEND_TO_PEER_POD_DESCRIPTION)
            .input_schema(
                serde_json::to_value(schemars::schema_for!(SendToPeerWorkerInput)).unwrap(),
            );
        let tool: Arc<dyn Tool> = Arc::new(SendToPeerWorkerTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

async fn send_peer_notify(socket_path: &Path, message: String) -> io::Result<()> {
    send_notify(socket_path, message, true).await
}

async fn send_notify(socket_path: &Path, message: String, auto_run: bool) -> io::Result<()> {
    connect_and_send(socket_path, &Method::Notify { message, auto_run }).await
}

fn json_content<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::Internal(format!("serialize worker discovery output: {e}")))
}

fn discovery_error_to_tool_error(error: WorkerDiscoveryError) -> ToolError {
    match error {
        WorkerDiscoveryError::StateMissing { .. }
        | WorkerDiscoveryError::NotVisible { .. }
        | WorkerDiscoveryError::NotRestorable { .. }
        | WorkerDiscoveryError::SelfPeer { .. }
        | WorkerDiscoveryError::MissingWorker { .. } => {
            ToolError::InvalidArgument(error.to_string())
        }
        WorkerDiscoveryError::LockConflict { .. }
        | WorkerDiscoveryError::Store(_)
        | WorkerDiscoveryError::WorkerStore(_)
        | WorkerDiscoveryError::ScopeLock(_)
        | WorkerDiscoveryError::RestoreSpawn(_)
        | WorkerDiscoveryError::RestoreLaunchFailed { .. }
        | WorkerDiscoveryError::RestoreExited { .. }
        | WorkerDiscoveryError::RestoreTimeout => ToolError::ExecutionFailed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{Permission, ScopeRule};
    use protocol::stream::JsonLineWriter;
    use protocol::{Alert, AlertLevel, AlertSource};
    use session_store::{
        FsWorkerStore, WorkerSpawnedChild, WorkerSpawnedScopeRule, WorkerStoreError,
    };
    use session_store::{new_segment_id, new_session_id};
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    use crate::runtime::dir::RuntimeDir;
    use crate::runtime::worker_allocation::test_util::RuntimeDirSandbox;

    #[derive(Clone)]
    struct FailTargetPeerStore {
        inner: FsWorkerStore,
    }

    impl WorkerMetadataStore for FailTargetPeerStore {
        fn write(&self, metadata: &WorkerMetadata) -> Result<(), WorkerStoreError> {
            if metadata.worker_name == "target"
                && metadata
                    .peers
                    .iter()
                    .any(|peer| peer.worker_name == "source")
            {
                return Err(WorkerStoreError::Io(io::Error::other(
                    "injected target-side peer write failure",
                )));
            }
            self.inner.write(metadata)
        }

        fn read_by_name(
            &self,
            worker_name: &str,
        ) -> Result<Option<WorkerMetadata>, WorkerStoreError> {
            self.inner.read_by_name(worker_name)
        }

        fn list_names(&self) -> Result<Vec<String>, WorkerStoreError> {
            self.inner.list_names()
        }

        fn root_dir(&self) -> Option<PathBuf> {
            self.inner.root_dir()
        }

        fn delete_by_name(&self, worker_name: &str) -> Result<(), WorkerStoreError> {
            self.inner.delete_by_name(worker_name)
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn state_backed_visibility_and_restore_planning() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let _runtime_sandbox = RuntimeDirSandbox::new(&runtime_base);

        let store = FsWorkerStore::new(&store_dir).unwrap();
        let session_id = new_session_id();
        let active_child_segment = new_segment_id();
        let pending_session_id = new_session_id();
        let live_socket = runtime_base.join("child-live").join("sock");
        std::fs::create_dir_all(live_socket.parent().unwrap()).unwrap();
        let live_listener = spawn_snapshot_socket(&live_socket).await;

        let stale_socket = runtime_base.join("child-stale").join("sock");
        let pending_socket = runtime_base.join("child-pending").join("sock");
        let parent = WorkerMetadata {
            worker_name: "parent".into(),
            active: None,
            workspace_root: None,
            workspace_id: None,
            spawned_children: vec![
                child("child-live", &live_socket),
                child("child-stale", &stale_socket),
                child("child-pending", &pending_socket),
            ],
            reclaimed_children: Vec::new(),
            peers: vec![session_store::WorkerPeer {
                worker_name: "peer".into(),
            }],
            resolved_manifest_snapshot: None,
        };
        store.write(&parent).unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "child-live".into(),
                active: Some(WorkerActiveSegmentRef::active_segment(
                    session_id,
                    active_child_segment,
                )),
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "child-stale".into(),
                active: Some(WorkerActiveSegmentRef::active_segment(
                    session_id,
                    active_child_segment,
                )),
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "child-pending".into(),
                active: Some(WorkerActiveSegmentRef::pending_segment(pending_session_id)),
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "hidden".into(),
                active: Some(WorkerActiveSegmentRef::active_segment(
                    session_id,
                    new_segment_id(),
                )),
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "peer".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "parent".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();

        // RuntimeDir creates parent runtime files; discovery must still use
        // Worker state when spawned_workers.json is absent.
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "parent").await.unwrap());
        let runtime_file = runtime_dir.path().join("spawned_workers.json");
        assert!(!runtime_file.exists());
        let registry = SpawnedWorkerRegistry::new(runtime_dir);
        let discovery = WorkerDiscovery::new(
            store.clone(),
            "parent".into(),
            runtime_base.clone(),
            Some(root.path().to_path_buf()),
            registry,
        );

        let list_tool_def = list_workers_tool(discovery.clone());
        let (list_meta, _) = list_tool_def();
        assert_eq!(list_meta.name, "ListWorkers");
        let restore_tool_def = restore_worker_tool(discovery.clone());
        let (restore_meta, _) = restore_tool_def();
        assert_eq!(restore_meta.name, "RestoreWorker");
        let send_peer_tool_def = send_to_peer_worker_tool(discovery.clone());
        let (send_peer_meta, _) = send_peer_tool_def();
        assert_eq!(send_peer_meta.name, "SendToPeerWorker");

        let list = discovery.list_visible().await.unwrap();
        let names: Vec<_> = list.iter().map(|p| p.worker_name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "child-live",
                "child-pending",
                "child-stale",
                "parent",
                "peer"
            ]
        );
        assert!(!names.contains(&"hidden"));
        assert_eq!(
            list.iter()
                .find(|p| p.worker_name == "peer")
                .unwrap()
                .visibility,
            VisibilityReason::Peer
        );
        assert_eq!(
            list.iter()
                .find(|p| p.worker_name == "child-live")
                .unwrap()
                .visibility,
            VisibilityReason::SpawnedChild
        );
        assert!(
            list.iter()
                .find(|p| p.worker_name == "child-live")
                .unwrap()
                .live
                .reachable
        );
        assert!(
            !list
                .iter()
                .find(|p| p.worker_name == "child-stale")
                .unwrap()
                .live
                .reachable
        );

        let pending = list
            .iter()
            .find(|p| p.worker_name == "child-pending")
            .unwrap();
        assert_eq!(pending.state, WorkerStateStatus::Readable);
        assert_eq!(
            pending.active.as_ref().unwrap().session_id,
            pending_session_id
        );
        assert_eq!(pending.active.as_ref().unwrap().segment_id, None);
        assert!(!pending.restore.possible);

        let hidden_err = discovery.inspect("hidden").await.unwrap_err();
        assert!(matches!(
            hidden_err,
            WorkerDiscoveryError::NotVisible { .. }
        ));
        let missing_err = discovery.inspect("missing").await.unwrap_err();
        assert!(matches!(
            missing_err,
            WorkerDiscoveryError::StateMissing { .. }
        ));
        let hidden_restore_err = discovery.plan_restore("hidden").await.unwrap_err();
        assert!(matches!(
            hidden_restore_err,
            WorkerDiscoveryError::NotVisible { .. }
        ));

        let live_plan = discovery.plan_restore("child-live").await.unwrap();
        assert!(matches!(live_plan, RestorePlan::AlreadyLive { .. }));
        let restore_plan = discovery.plan_restore("child-stale").await.unwrap();
        assert!(matches!(restore_plan, RestorePlan::Restore { .. }));

        let lock_socket = runtime_base.join("lock-owner.sock");
        let _guard = worker_allocation::install_top_level(
            "lock-owner".into(),
            std::process::id(),
            lock_socket.clone(),
            vec![ScopeRule {
                target: root.path().to_path_buf(),
                permission: Permission::Read,
                recursive: true,
            }],
            active_child_segment,
        )
        .unwrap();
        let locked_err = discovery.plan_restore("child-stale").await.unwrap_err();
        assert!(matches!(
            locked_err,
            WorkerDiscoveryError::LockConflict { .. }
        ));

        live_listener.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_peer_persists_reciprocal_metadata() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let store = FsWorkerStore::new(&store_dir).unwrap();
        store.write(&WorkerMetadata::new("source", None)).unwrap();
        store.write(&WorkerMetadata::new("target", None)).unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());

        let discovery = WorkerDiscovery::new(
            store.clone(),
            "source".into(),
            runtime_base.clone(),
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );
        let result = discovery.register_peer("target").unwrap();
        assert_eq!(result.source, "source");
        assert_eq!(result.peer, "target");

        let source = store.read_by_name("source").unwrap().unwrap();
        let target = store.read_by_name("target").unwrap().unwrap();
        assert_eq!(source.peers[0].worker_name, "target");
        assert_eq!(target.peers[0].worker_name, "source");

        let list = discovery.list_visible().await.unwrap();
        assert_eq!(
            list.iter()
                .find(|item| item.worker_name == "target")
                .unwrap()
                .visibility,
            VisibilityReason::Peer
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_peer_rejects_self_and_missing_target() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let store = FsWorkerStore::new(&store_dir).unwrap();
        store.write(&WorkerMetadata::new("source", None)).unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = WorkerDiscovery::new(
            store,
            "source".into(),
            runtime_base,
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );

        let self_err = discovery.register_peer("source").unwrap_err();
        assert!(matches!(self_err, WorkerDiscoveryError::SelfPeer { .. }));
        let missing_err = discovery.register_peer("missing").unwrap_err();
        assert!(matches!(
            missing_err,
            WorkerDiscoveryError::MissingWorker { .. }
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_peer_target_failure_preserves_existing_source_peer() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let inner = FsWorkerStore::new(&store_dir).unwrap();
        inner
            .write(&WorkerMetadata {
                worker_name: "source".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "target".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        inner.write(&WorkerMetadata::new("target", None)).unwrap();
        let store = FailTargetPeerStore { inner };
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = WorkerDiscovery::new(
            store.clone(),
            "source".into(),
            runtime_base,
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );

        let err = discovery.register_peer("target").unwrap_err();
        assert!(matches!(err, WorkerDiscoveryError::WorkerStore(_)));
        let source = store.read_by_name("source").unwrap().unwrap();
        assert_eq!(source.peers.len(), 1);
        assert_eq!(source.peers[0].worker_name, "target");
        let target = store.read_by_name("target").unwrap().unwrap();
        assert!(target.peers.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_to_peer_worker_delivers_notify_without_child_registry() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(runtime_base.join("target")).unwrap();
        let store = FsWorkerStore::new(&store_dir).unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "source".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "target".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "target".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "source".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = WorkerDiscovery::new(
            store,
            "source".into(),
            runtime_base.clone(),
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );

        let socket = runtime_base.join("target").join("sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let target = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (reader_half, writer_half) = stream.into_split();
            let mut reader = JsonLineReader::new(reader_half);
            let mut writer = JsonLineWriter::new(writer_half);
            writer
                .write(&Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message: "connect-time alert".into(),
                    timestamp_ms: 0,
                }))
                .await
                .unwrap();
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();
            let method = reader.next::<Method>().await.unwrap().unwrap();
            if let Method::Notify { message, auto_run } = method {
                assert!(auto_run);
                tx.send(message).await.unwrap();
            } else {
                panic!("expected Notify, got {method:?}");
            }
        });

        let (_, tool) = send_to_peer_worker_tool(discovery)();
        let output = tool
            .execute(r#"{"name":"target","message":"hello"}"#, Default::default())
            .await
            .unwrap();
        assert_eq!(output.summary, "sent peer message to `target`");
        let message = rx.recv().await.unwrap();
        assert_eq!(message, "[Peer message from `source`]\nhello");
        target.await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn weak_notify_to_live_peer_uses_notify_without_auto_run_and_noops_when_missing() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(runtime_base.join("target")).unwrap();
        let store = FsWorkerStore::new(&store_dir).unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "source".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "target".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&WorkerMetadata {
                worker_name: "target".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![session_store::WorkerPeer {
                    worker_name: "source".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = WorkerDiscovery::new(
            store,
            "source".into(),
            runtime_base.clone(),
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );

        let socket = runtime_base.join("target").join("sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let target = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();

            let (stream, _) = listener.accept().await.unwrap();
            let (reader_half, writer_half) = stream.into_split();
            let mut reader = JsonLineReader::new(reader_half);
            let mut writer = JsonLineWriter::new(writer_half);
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Idle,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();
            let method = reader.next::<Method>().await.unwrap().unwrap();
            if let Method::Notify { message, auto_run } = method {
                assert!(!auto_run);
                tx.send(message).await.unwrap();
            } else {
                panic!("expected Notify, got {method:?}");
            }
        });

        assert_eq!(
            discovery
                .send_weak_notify_to_live_peer("target", "weak event".into())
                .await,
            WeakNotifyDelivery::Delivered
        );
        assert_eq!(rx.recv().await.unwrap(), "weak event");
        target.await.unwrap();

        assert_eq!(
            discovery
                .send_weak_notify_to_live_peer("missing", "no-op".into())
                .await,
            WeakNotifyDelivery::SkippedMissing
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn weak_notify_does_not_send_to_spawned_child_visibility() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(runtime_base.join("target")).unwrap();
        let store = FsWorkerStore::new(&store_dir).unwrap();
        let socket = runtime_base.join("target").join("sock");
        store
            .write(&WorkerMetadata {
                worker_name: "source".into(),
                active: None,
                workspace_root: None,
                workspace_id: None,
                spawned_children: vec![child("target", &socket)],
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store.write(&WorkerMetadata::new("target", None)).unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = WorkerDiscovery::new(
            store,
            "source".into(),
            runtime_base,
            Some(root.path().to_path_buf()),
            SpawnedWorkerRegistry::new(runtime_dir),
        );

        assert_eq!(
            discovery
                .send_weak_notify_to_live_peer("target", "must not send".into())
                .await,
            WeakNotifyDelivery::SkippedNotPeer {
                visibility: VisibilityReason::SpawnedChild
            }
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn probe_socket_reads_status_after_replayed_alert() {
        let root = TempDir::new().unwrap();
        let socket = root.path().join("worker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Worker,
                    message: "replayed alert".into(),
                    timestamp_ms: 0,
                }))
                .await
                .unwrap();
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        worker_name: "alerted".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: WorkerStatus::Paused,
                    in_flight: Default::default(),
                })
                .await
                .unwrap();
        });

        let info = probe_socket(&socket).await;
        assert!(info.reachable);
        assert!(matches!(info.status, Some(WorkerStatus::Paused)));
        handle.await.unwrap();
    }

    fn child(name: &str, socket_path: &Path) -> WorkerSpawnedChild {
        WorkerSpawnedChild {
            worker_name: name.to_string(),
            socket_path: socket_path.to_path_buf(),
            scope_delegated: vec![WorkerSpawnedScopeRule {
                target: PathBuf::from("/tmp"),
                permission: "read".into(),
                recursive: true,
            }],
            callback_address: PathBuf::from("/tmp/parent.sock"),
        }
    }

    async fn spawn_snapshot_socket(socket_path: &Path) -> tokio::task::JoinHandle<()> {
        let _ = std::fs::remove_file(socket_path);
        let listener = UnixListener::bind(socket_path).unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut writer = JsonLineWriter::new(stream);
                    let _ = writer
                        .write(&Event::Snapshot {
                            entries: Vec::new(),
                            greeting: protocol::Greeting {
                                worker_name: "child-live".into(),
                                cwd: "/tmp".into(),
                                provider: "test".into(),
                                model: "test".into(),
                                scope_summary: String::new(),
                                tools: Vec::new(),
                                context_window: 0,
                                context_tokens: 0,
                            },
                            status: WorkerStatus::Idle,
                            in_flight: Default::default(),
                        })
                        .await;
                });
            }
        })
    }
}
