//! Pod-state-backed discovery and restore tools.
//!
//! This surface deliberately does not enumerate every Pod on the host. The
//! listing path starts from the caller's visibility set (the caller itself and
//! Pods it spawned according to durable Pod state) and only then reads each
//! Pod's own state. Name-targeted operations distinguish missing state from
//! state that exists but is outside that visibility set.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use client::PodRuntimeCommand;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::{Permission, ScopeRule};
use pod_store::{PodActiveSegmentRef, PodMetadata, PodMetadataStore, validate_pod_name};
use protocol::stream::JsonLineReader;
use protocol::{Event, Method, PodStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::{SegmentId, SessionId};
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::runtime::dir::SpawnedPodRecord;
use crate::runtime::pod_registry;
use crate::spawn::comm_tools::connect_and_send;
use crate::spawn::registry::SpawnedPodRegistry;

const PROBE_TIMEOUT: Duration = Duration::from_millis(500);
const RESTORE_START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub struct PodDiscovery<St> {
    store: St,
    self_pod_name: String,
    runtime_base: PathBuf,
    cwd: PathBuf,
    store_dir: Option<PathBuf>,
    spawned_registry: Arc<SpawnedPodRegistry>,
}

impl<St> PodDiscovery<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    pub fn new(
        store: St,
        self_pod_name: String,
        runtime_base: PathBuf,
        cwd: PathBuf,
        spawned_registry: Arc<SpawnedPodRegistry>,
    ) -> Self {
        let store_dir = store.root_dir();
        Self {
            store,
            self_pod_name,
            runtime_base,
            cwd,
            store_dir,
            spawned_registry,
        }
    }

    pub async fn list_visible(&self) -> Result<Vec<VisiblePodItem>, PodDiscoveryError> {
        let visibility = self.visibility().await?;
        let mut items = Vec::with_capacity(visibility.visible.len());
        for pod_name in visibility.visible.keys() {
            items.push(
                self.build_item_for_visible_name(pod_name, &visibility)
                    .await,
            );
        }
        Ok(items)
    }

    pub async fn inspect(&self, pod_name: &str) -> Result<PodDetail, PodDiscoveryError> {
        let visibility = self.visibility().await?;
        let known_names = self.store.list_names()?;
        let state_exists = known_names.iter().any(|n| n == pod_name);
        if !state_exists {
            return Err(PodDiscoveryError::StateMissing {
                pod_name: pod_name.to_string(),
            });
        }
        if !visibility.visible.contains_key(pod_name) {
            return Err(PodDiscoveryError::NotVisible {
                pod_name: pod_name.to_string(),
            });
        }

        match self.store.read_by_name(pod_name)? {
            Some(metadata) => Ok(self.detail_from_metadata(metadata, &visibility).await),
            None => Err(PodDiscoveryError::StateMissing {
                pod_name: pod_name.to_string(),
            }),
        }
    }

    pub async fn restore(&self, pod_name: &str) -> Result<RestoreResult, PodDiscoveryError> {
        match self.plan_restore(pod_name).await? {
            RestorePlan::AlreadyLive {
                pod_name,
                socket_path,
                status,
            } => Ok(RestoreResult::AlreadyLive {
                pod_name,
                socket_path,
                status,
            }),
            RestorePlan::Restore {
                pod_name,
                socket_path,
            } => {
                self.spawn_restore_process(&pod_name, &socket_path).await?;
                Ok(RestoreResult::Restored {
                    pod_name,
                    socket_path,
                })
            }
        }
    }

    pub async fn plan_restore(&self, pod_name: &str) -> Result<RestorePlan, PodDiscoveryError> {
        let detail = self.inspect(pod_name).await?;
        if detail.live.reachable {
            return Ok(RestorePlan::AlreadyLive {
                pod_name: pod_name.to_string(),
                socket_path: detail.live.socket_path,
                status: detail.live.status,
            });
        }

        let active = detail
            .active
            .ok_or_else(|| PodDiscoveryError::NotRestorable {
                pod_name: pod_name.to_string(),
                reason: "pod state has no active session".into(),
            })?;
        let segment_id = active
            .segment_id
            .ok_or_else(|| PodDiscoveryError::NotRestorable {
                pod_name: pod_name.to_string(),
                reason: "pod state has an active session but no active segment yet".into(),
            })?;

        if let Some(lock) = lookup_segment_lock(segment_id)? {
            let lock_live = probe_socket(&lock.socket).await;
            return if lock_live.reachable {
                Ok(RestorePlan::AlreadyLive {
                    pod_name: lock.pod_name,
                    socket_path: lock.socket,
                    status: lock_live.status,
                })
            } else {
                Err(PodDiscoveryError::LockConflict {
                    pod_name: pod_name.to_string(),
                    segment_id,
                    owner_pod: lock.pod_name,
                    socket_path: lock.socket,
                    pid: lock.pid,
                })
            };
        }

        Ok(RestorePlan::Restore {
            pod_name: pod_name.to_string(),
            socket_path: self.default_socket_path(pod_name),
        })
    }

    pub fn register_peer(
        &self,
        peer_name: &str,
    ) -> Result<PeerRegistrationResult, PodDiscoveryError> {
        validate_pod_name(peer_name)?;
        if peer_name == self.self_pod_name {
            return Err(PodDiscoveryError::SelfPeer {
                pod_name: peer_name.to_string(),
            });
        }
        let self_metadata = self
            .store
            .read_by_name(&self.self_pod_name)?
            .ok_or_else(|| PodDiscoveryError::StateMissing {
                pod_name: self.self_pod_name.clone(),
            })?;
        let prior_self_peers = self_metadata.peers.clone();
        if self.store.read_by_name(peer_name)?.is_none() {
            return Err(PodDiscoveryError::MissingPod {
                pod_name: peer_name.to_string(),
            });
        }

        self.store.add_peer(&self.self_pod_name, peer_name)?;
        if let Err(error) = self.store.add_peer(peer_name, &self.self_pod_name) {
            let _ = self.store.set_peers(&self.self_pod_name, prior_self_peers);
            return Err(PodDiscoveryError::PodStore(error));
        }

        Ok(PeerRegistrationResult {
            source: self.self_pod_name.clone(),
            peer: peer_name.to_string(),
        })
    }

    async fn visibility(&self) -> Result<VisibilitySet, PodDiscoveryError> {
        let mut visible = BTreeMap::new();
        let mut child_sockets = BTreeMap::new();
        let mut comm_registry = BTreeMap::new();
        visible.insert(self.self_pod_name.clone(), VisibilityReason::SelfPod);

        // Durable parent -> child state is the primary visibility source.
        if let Some(metadata) = self.store.read_by_name(&self.self_pod_name)? {
            for child in metadata.spawned_children {
                visible
                    .entry(child.pod_name.clone())
                    .or_insert(VisibilityReason::SpawnedChild);
                child_sockets.insert(child.pod_name.clone(), child.socket_path.clone());
                comm_registry.insert(child.pod_name.clone(), comm_info_from_spawned_child(&child));
            }
            for peer in metadata.peers {
                visible
                    .entry(peer.pod_name)
                    .or_insert(VisibilityReason::Peer);
            }
        }

        // The live in-memory registry covers just-spawned children even if a
        // state write failed after the process became reachable. It is an
        // additive visibility hint, not the source of Pod metadata.
        for record in self.spawned_registry.list().await {
            visible
                .entry(record.pod_name.clone())
                .or_insert(VisibilityReason::SpawnedChild);
            child_sockets.insert(record.pod_name.clone(), record.socket_path.clone());
            comm_registry.insert(
                record.pod_name.clone(),
                CommRegistryInfo::from_record(&record),
            );
        }

        Ok(VisibilitySet {
            visible,
            child_sockets,
            comm_registry,
        })
    }

    async fn build_item_for_visible_name(
        &self,
        pod_name: &str,
        visibility: &VisibilitySet,
    ) -> VisiblePodItem {
        let visibility_reason = visibility.reason_for(pod_name);
        match self.store.read_by_name(pod_name) {
            Ok(Some(metadata)) => {
                let detail = self.detail_from_metadata(metadata, visibility).await;
                VisiblePodItem {
                    pod_name: pod_name.to_string(),
                    visibility: visibility_reason,
                    state: PodStateStatus::Readable,
                    active: detail.active,
                    live: detail.live,
                    restore: detail.restore,
                    comm_registry: detail.comm_registry,
                    spawned_children: detail.spawned_children,
                    error: None,
                }
            }
            Ok(None) => VisiblePodItem {
                pod_name: pod_name.to_string(),
                visibility: visibility_reason,
                state: PodStateStatus::Missing,
                active: None,
                live: self.live_for_name(pod_name, None).await,
                restore: RestoreInfo::not_possible("pod state missing"),
                comm_registry: visibility.comm_info_for(pod_name),
                spawned_children: SpawnedChildrenSummary::default(),
                error: None,
            },
            Err(error) => VisiblePodItem {
                pod_name: pod_name.to_string(),
                visibility: visibility_reason,
                state: PodStateStatus::Corrupt,
                active: None,
                live: self.live_for_name(pod_name, None).await,
                restore: RestoreInfo::not_possible("pod state is unreadable"),
                comm_registry: visibility.comm_info_for(pod_name),
                spawned_children: SpawnedChildrenSummary::default(),
                error: Some(error.to_string()),
            },
        }
    }

    async fn detail_from_metadata(
        &self,
        metadata: PodMetadata,
        visibility: &VisibilitySet,
    ) -> PodDetail {
        let child_socket = visibility.child_socket_for(&metadata.pod_name);
        let live = self
            .live_for_name(&metadata.pod_name, child_socket.as_deref())
            .await;
        let restore = self
            .restore_info(&metadata.pod_name, metadata.active.as_ref())
            .await;
        let spawned_children = summarize_spawned_children(&metadata.spawned_children).await;
        PodDetail {
            pod_name: metadata.pod_name.clone(),
            visibility: visibility.reason_for(&metadata.pod_name),
            active: metadata.active.map(ActivePointer::from),
            live,
            restore,
            comm_registry: visibility.comm_info_for(&metadata.pod_name),
            spawned_children,
        }
    }

    async fn restore_info(
        &self,
        pod_name: &str,
        active: Option<&PodActiveSegmentRef>,
    ) -> RestoreInfo {
        let Some(active) = active else {
            return RestoreInfo::not_possible("pod state has no active session");
        };
        let Some(segment_id) = active.segment_id else {
            return RestoreInfo::not_possible("active segment is not known yet");
        };
        match lookup_segment_lock(segment_id) {
            Ok(Some(lock)) => RestoreInfo {
                possible: false,
                restore_name: Some(pod_name.to_string()),
                reason: Some(format!(
                    "segment is currently locked by `{}` (pid {})",
                    lock.pod_name, lock.pid
                )),
            },
            Ok(None) => RestoreInfo {
                possible: true,
                restore_name: Some(pod_name.to_string()),
                reason: None,
            },
            Err(error) => RestoreInfo {
                possible: false,
                restore_name: Some(pod_name.to_string()),
                reason: Some(format!("lock lookup failed: {error}")),
            },
        }
    }

    async fn live_for_name(&self, pod_name: &str, socket_override: Option<&Path>) -> LiveInfo {
        let socket_path = socket_override
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.default_socket_path(pod_name));
        probe_socket(&socket_path).await
    }

    fn default_socket_path(&self, pod_name: &str) -> PathBuf {
        self.runtime_base.join(pod_name).join("sock")
    }

    async fn spawn_restore_process(
        &self,
        pod_name: &str,
        socket_path: &Path,
    ) -> Result<(), PodDiscoveryError> {
        let runtime_command =
            PodRuntimeCommand::resolve().map_err(PodDiscoveryError::RestoreSpawn)?;
        let mut command = Command::new(runtime_command.program());
        command
            .args(runtime_command.prefix_args())
            .arg("--pod")
            .arg(pod_name)
            .arg("--require-pod-state")
            .current_dir(&self.cwd)
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
                .map_err(|source| PodDiscoveryError::RestoreLaunchFailed {
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
            if let Some(status) = child.try_wait().map_err(PodDiscoveryError::RestoreSpawn)? {
                return Err(PodDiscoveryError::RestoreExited { status });
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(PodDiscoveryError::RestoreTimeout);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityReason {
    SelfPod,
    SpawnedChild,
    Peer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PodStateStatus {
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

impl From<PodActiveSegmentRef> for ActivePointer {
    fn from(value: PodActiveSegmentRef) -> Self {
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
    pub status: Option<PodStatus>,
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

    fn from_record(record: &SpawnedPodRecord) -> Self {
        Self {
            registered: true,
            socket_path: Some(record.socket_path.clone()),
            scope_delegated: record.scope_delegated.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VisiblePodItem {
    pub pod_name: String,
    pub visibility: VisibilityReason,
    pub state: PodStateStatus,
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
pub struct PodDetail {
    pub pod_name: String,
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
        pod_name: String,
        socket_path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<PodStatus>,
    },
    Restore {
        pod_name: String,
        socket_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RestoreResult {
    AlreadyLive {
        pod_name: String,
        socket_path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<PodStatus>,
    },
    Restored {
        pod_name: String,
        socket_path: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerRegistrationResult {
    pub source: String,
    pub peer: String,
}

#[derive(Debug, thiserror::Error)]
pub enum PodDiscoveryError {
    #[error("pod state missing for `{pod_name}`")]
    StateMissing { pod_name: String },
    #[error("pod `{pod_name}` is not visible to this Pod")]
    NotVisible { pod_name: String },
    #[error("pod `{pod_name}` is not restorable: {reason}")]
    NotRestorable { pod_name: String, reason: String },
    #[error("pod `{pod_name}` cannot be registered as a peer of itself")]
    SelfPeer { pod_name: String },
    #[error("pod `{pod_name}` does not exist")]
    MissingPod { pod_name: String },
    #[error(
        "pod `{pod_name}` segment {segment_id} is locked by `{owner_pod}` pid {pid} at {socket_path}"
    )]
    LockConflict {
        pod_name: String,
        segment_id: SegmentId,
        owner_pod: String,
        socket_path: PathBuf,
        pid: u32,
    },
    #[error("session store error: {0}")]
    Store(#[from] session_store::StoreError),
    #[error("pod store error: {0}")]
    PodStore(#[from] pod_store::PodStoreError),
    #[error("scope lock error: {0}")]
    ScopeLock(#[from] pod_registry::ScopeLockError),
    #[error("failed to launch restore process: {0}")]
    RestoreSpawn(io::Error),
    #[error("failed to launch restore runtime command `{command}`: {source}")]
    RestoreLaunchFailed {
        command: PodRuntimeCommand,
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
    fn reason_for(&self, pod_name: &str) -> VisibilityReason {
        self.visible
            .get(pod_name)
            .cloned()
            .unwrap_or(VisibilityReason::SpawnedChild)
    }

    fn child_socket_for(&self, pod_name: &str) -> Option<PathBuf> {
        self.child_sockets.get(pod_name).cloned()
    }

    fn comm_info_for(&self, pod_name: &str) -> CommRegistryInfo {
        self.comm_registry
            .get(pod_name)
            .cloned()
            .unwrap_or_else(CommRegistryInfo::missing)
    }
}

fn comm_info_from_spawned_child(child: &pod_store::PodSpawnedChild) -> CommRegistryInfo {
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
    children: &[pod_store::PodSpawnedChild],
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
) -> Result<Option<pod_registry::SegmentLockInfo>, pod_registry::ScopeLockError> {
    pod_registry::lookup_segment(segment_id)
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PodNameInput {
    /// Pod name to restore.
    name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct SendToPeerPodInput {
    /// Target peer Pod name.
    name: String,
    /// Text delivered to the peer as a peer notification.
    message: String,
}

struct ListPodsTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for ListPodsTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        _input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let items = self
            .discovery
            .list_visible()
            .await
            .map_err(discovery_error_to_tool_error)?;
        let summary = format!("{} visible pod(s)", items.len());
        Ok(ToolOutput {
            summary,
            content: Some(json_content(&items)?),
        })
    }
}

struct RestorePodTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for RestorePodTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: PodNameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid RestorePod input: {e}")))?;
        let result = self
            .discovery
            .restore(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        let summary = match &result {
            RestoreResult::AlreadyLive { pod_name, .. } => {
                format!("pod `{pod_name}` is already live")
            }
            RestoreResult::Restored { pod_name, .. } => {
                format!("pod `{pod_name}` restored from pod state")
            }
        };
        Ok(ToolOutput {
            summary,
            content: Some(json_content(&result)?),
        })
    }
}

pub fn list_pods_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("ListPods")
            .description(
                "List Pods visible to this Pod from durable Pod state, peer metadata, and the spawned-child registry. This does not expose the host-wide Pod universe.",
            )
            .input_schema(serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }));
        let tool: Arc<dyn Tool> = Arc::new(ListPodsTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

pub fn restore_pod_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("RestorePod")
            .description(
                "Restore a visible stopped/restorable Pod, or report that a visible Pod is already live. Missing state is an error.",
            )
            .input_schema(serde_json::to_value(schemars::schema_for!(PodNameInput)).unwrap());
        let tool: Arc<dyn Tool> = Arc::new(RestorePodTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

const SEND_TO_PEER_POD_DESCRIPTION: &str = "Send a text message to a peer Pod made visible by explicit reciprocal peer metadata. The message is delivered as a peer notification through the target Pod's durable notification/history path. This does not grant delegated scope, create a spawned-child output cursor, imply parent ownership, or produce child completion notifications. Fails clearly if the target is not a visible live peer; it does not auto-restore stopped peers.";

struct SendToPeerPodTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for SendToPeerPodTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: SendToPeerPodInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid SendToPeerPod input: {e}")))?;
        let detail = self
            .discovery
            .inspect(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        if detail.visibility != VisibilityReason::Peer {
            return Err(ToolError::InvalidArgument(format!(
                "pod `{}` is visible as {:?}, not as a peer",
                input.name, detail.visibility
            )));
        }
        if !detail.live.reachable {
            return Err(ToolError::ExecutionFailed(format!(
                "peer pod `{}` is not live/reachable; restore it before sending",
                input.name
            )));
        }

        let message = format!(
            "[Peer message from `{}`]\n{}",
            self.discovery.self_pod_name, input.message
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

pub fn send_to_peer_pod_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("SendToPeerPod")
            .description(SEND_TO_PEER_POD_DESCRIPTION)
            .input_schema(serde_json::to_value(schemars::schema_for!(SendToPeerPodInput)).unwrap());
        let tool: Arc<dyn Tool> = Arc::new(SendToPeerPodTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

async fn send_peer_notify(socket_path: &Path, message: String) -> io::Result<()> {
    connect_and_send(
        socket_path,
        &Method::Notify {
            message,
            auto_run: true,
        },
    )
    .await
}

fn json_content<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::Internal(format!("serialize pod discovery output: {e}")))
}

fn discovery_error_to_tool_error(error: PodDiscoveryError) -> ToolError {
    match error {
        PodDiscoveryError::StateMissing { .. }
        | PodDiscoveryError::NotVisible { .. }
        | PodDiscoveryError::NotRestorable { .. }
        | PodDiscoveryError::SelfPeer { .. }
        | PodDiscoveryError::MissingPod { .. } => ToolError::InvalidArgument(error.to_string()),
        PodDiscoveryError::LockConflict { .. }
        | PodDiscoveryError::Store(_)
        | PodDiscoveryError::PodStore(_)
        | PodDiscoveryError::ScopeLock(_)
        | PodDiscoveryError::RestoreSpawn(_)
        | PodDiscoveryError::RestoreLaunchFailed { .. }
        | PodDiscoveryError::RestoreExited { .. }
        | PodDiscoveryError::RestoreTimeout => ToolError::ExecutionFailed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use manifest::{Permission, ScopeRule};
    use pod_store::{FsPodStore, PodSpawnedChild, PodSpawnedScopeRule, PodStoreError};
    use protocol::stream::JsonLineWriter;
    use protocol::{Alert, AlertLevel, AlertSource};
    use session_store::{new_segment_id, new_session_id};
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    use crate::runtime::dir::RuntimeDir;

    #[derive(Clone)]
    struct FailTargetPeerStore {
        inner: FsPodStore,
    }

    impl PodMetadataStore for FailTargetPeerStore {
        fn write(&self, metadata: &PodMetadata) -> Result<(), PodStoreError> {
            if metadata.pod_name == "target"
                && metadata.peers.iter().any(|peer| peer.pod_name == "source")
            {
                return Err(PodStoreError::Io(io::Error::other(
                    "injected target-side peer write failure",
                )));
            }
            self.inner.write(metadata)
        }

        fn read_by_name(&self, pod_name: &str) -> Result<Option<PodMetadata>, PodStoreError> {
            self.inner.read_by_name(pod_name)
        }

        fn list_names(&self) -> Result<Vec<String>, PodStoreError> {
            self.inner.list_names()
        }

        fn root_dir(&self) -> Option<PathBuf> {
            self.inner.root_dir()
        }

        fn delete_by_name(&self, pod_name: &str) -> Result<(), PodStoreError> {
            self.inner.delete_by_name(pod_name)
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test(flavor = "current_thread")]
    async fn state_backed_visibility_and_restore_planning() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        unsafe {
            std::env::set_var("YOI_RUNTIME_DIR", &runtime_base);
        }

        let store = FsPodStore::new(&store_dir).unwrap();
        let session_id = new_session_id();
        let active_child_segment = new_segment_id();
        let pending_session_id = new_session_id();
        let live_socket = runtime_base.join("child-live").join("sock");
        std::fs::create_dir_all(live_socket.parent().unwrap()).unwrap();
        let live_listener = spawn_snapshot_socket(&live_socket).await;

        let stale_socket = runtime_base.join("child-stale").join("sock");
        let pending_socket = runtime_base.join("child-pending").join("sock");
        let parent = PodMetadata {
            pod_name: "parent".into(),
            active: None,
            spawned_children: vec![
                child("child-live", &live_socket),
                child("child-stale", &stale_socket),
                child("child-pending", &pending_socket),
            ],
            reclaimed_children: Vec::new(),
            peers: vec![pod_store::PodPeer {
                pod_name: "peer".into(),
            }],
            resolved_manifest_snapshot: None,
        };
        store.write(&parent).unwrap();
        store
            .write(&PodMetadata {
                pod_name: "child-live".into(),
                active: Some(PodActiveSegmentRef::active_segment(
                    session_id,
                    active_child_segment,
                )),
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "child-stale".into(),
                active: Some(PodActiveSegmentRef::active_segment(
                    session_id,
                    active_child_segment,
                )),
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "child-pending".into(),
                active: Some(PodActiveSegmentRef::pending_segment(pending_session_id)),
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "hidden".into(),
                active: Some(PodActiveSegmentRef::active_segment(
                    session_id,
                    new_segment_id(),
                )),
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: Vec::new(),
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "peer".into(),
                active: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![pod_store::PodPeer {
                    pod_name: "parent".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();

        // RuntimeDir creates parent runtime files; discovery must still use
        // Pod state when spawned_pods.json is absent.
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "parent").await.unwrap());
        let runtime_file = runtime_dir.path().join("spawned_pods.json");
        assert!(!runtime_file.exists());
        let registry = SpawnedPodRegistry::new(runtime_dir);
        let discovery = PodDiscovery::new(
            store.clone(),
            "parent".into(),
            runtime_base.clone(),
            root.path().to_path_buf(),
            registry,
        );

        let list_tool_def = list_pods_tool(discovery.clone());
        let (list_meta, _) = list_tool_def();
        assert_eq!(list_meta.name, "ListPods");
        let restore_tool_def = restore_pod_tool(discovery.clone());
        let (restore_meta, _) = restore_tool_def();
        assert_eq!(restore_meta.name, "RestorePod");
        let send_peer_tool_def = send_to_peer_pod_tool(discovery.clone());
        let (send_peer_meta, _) = send_peer_tool_def();
        assert_eq!(send_peer_meta.name, "SendToPeerPod");

        let list = discovery.list_visible().await.unwrap();
        let names: Vec<_> = list.iter().map(|p| p.pod_name.as_str()).collect();
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
                .find(|p| p.pod_name == "peer")
                .unwrap()
                .visibility,
            VisibilityReason::Peer
        );
        assert_eq!(
            list.iter()
                .find(|p| p.pod_name == "child-live")
                .unwrap()
                .visibility,
            VisibilityReason::SpawnedChild
        );
        assert!(
            list.iter()
                .find(|p| p.pod_name == "child-live")
                .unwrap()
                .live
                .reachable
        );
        assert!(
            !list
                .iter()
                .find(|p| p.pod_name == "child-stale")
                .unwrap()
                .live
                .reachable
        );

        let pending = list.iter().find(|p| p.pod_name == "child-pending").unwrap();
        assert_eq!(pending.state, PodStateStatus::Readable);
        assert_eq!(
            pending.active.as_ref().unwrap().session_id,
            pending_session_id
        );
        assert_eq!(pending.active.as_ref().unwrap().segment_id, None);
        assert!(!pending.restore.possible);

        let hidden_err = discovery.inspect("hidden").await.unwrap_err();
        assert!(matches!(hidden_err, PodDiscoveryError::NotVisible { .. }));
        let missing_err = discovery.inspect("missing").await.unwrap_err();
        assert!(matches!(
            missing_err,
            PodDiscoveryError::StateMissing { .. }
        ));
        let hidden_restore_err = discovery.plan_restore("hidden").await.unwrap_err();
        assert!(matches!(
            hidden_restore_err,
            PodDiscoveryError::NotVisible { .. }
        ));

        let live_plan = discovery.plan_restore("child-live").await.unwrap();
        assert!(matches!(live_plan, RestorePlan::AlreadyLive { .. }));
        let restore_plan = discovery.plan_restore("child-stale").await.unwrap();
        assert!(matches!(restore_plan, RestorePlan::Restore { .. }));

        let lock_socket = runtime_base.join("lock-owner.sock");
        let _guard = pod_registry::install_top_level(
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
        assert!(matches!(locked_err, PodDiscoveryError::LockConflict { .. }));

        live_listener.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_peer_persists_reciprocal_metadata() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let store = FsPodStore::new(&store_dir).unwrap();
        store.write(&PodMetadata::new("source", None)).unwrap();
        store.write(&PodMetadata::new("target", None)).unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());

        let discovery = PodDiscovery::new(
            store.clone(),
            "source".into(),
            runtime_base.clone(),
            root.path().to_path_buf(),
            SpawnedPodRegistry::new(runtime_dir),
        );
        let result = discovery.register_peer("target").unwrap();
        assert_eq!(result.source, "source");
        assert_eq!(result.peer, "target");

        let source = store.read_by_name("source").unwrap().unwrap();
        let target = store.read_by_name("target").unwrap().unwrap();
        assert_eq!(source.peers[0].pod_name, "target");
        assert_eq!(target.peers[0].pod_name, "source");

        let list = discovery.list_visible().await.unwrap();
        assert_eq!(
            list.iter()
                .find(|item| item.pod_name == "target")
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
        let store = FsPodStore::new(&store_dir).unwrap();
        store.write(&PodMetadata::new("source", None)).unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = PodDiscovery::new(
            store,
            "source".into(),
            runtime_base,
            root.path().to_path_buf(),
            SpawnedPodRegistry::new(runtime_dir),
        );

        let self_err = discovery.register_peer("source").unwrap_err();
        assert!(matches!(self_err, PodDiscoveryError::SelfPeer { .. }));
        let missing_err = discovery.register_peer("missing").unwrap_err();
        assert!(matches!(missing_err, PodDiscoveryError::MissingPod { .. }));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn register_peer_target_failure_preserves_existing_source_peer() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        let inner = FsPodStore::new(&store_dir).unwrap();
        inner
            .write(&PodMetadata {
                pod_name: "source".into(),
                active: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![pod_store::PodPeer {
                    pod_name: "target".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        inner.write(&PodMetadata::new("target", None)).unwrap();
        let store = FailTargetPeerStore { inner };
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = PodDiscovery::new(
            store.clone(),
            "source".into(),
            runtime_base,
            root.path().to_path_buf(),
            SpawnedPodRegistry::new(runtime_dir),
        );

        let err = discovery.register_peer("target").unwrap_err();
        assert!(matches!(err, PodDiscoveryError::PodStore(_)));
        let source = store.read_by_name("source").unwrap().unwrap();
        assert_eq!(source.peers.len(), 1);
        assert_eq!(source.peers[0].pod_name, "target");
        let target = store.read_by_name("target").unwrap().unwrap();
        assert!(target.peers.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn send_to_peer_pod_delivers_notify_without_child_registry() {
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(runtime_base.join("target")).unwrap();
        let store = FsPodStore::new(&store_dir).unwrap();
        store
            .write(&PodMetadata {
                pod_name: "source".into(),
                active: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![pod_store::PodPeer {
                    pod_name: "target".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "target".into(),
                active: None,
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
                peers: vec![pod_store::PodPeer {
                    pod_name: "source".into(),
                }],
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        let runtime_dir = Arc::new(RuntimeDir::create(&runtime_base, "source").await.unwrap());
        let discovery = PodDiscovery::new(
            store,
            "source".into(),
            runtime_base.clone(),
            root.path().to_path_buf(),
            SpawnedPodRegistry::new(runtime_dir),
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
                        pod_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: PodStatus::Idle,
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
                    source: AlertSource::Pod,
                    message: "connect-time alert".into(),
                    timestamp_ms: 0,
                }))
                .await
                .unwrap();
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        pod_name: "target".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: PodStatus::Idle,
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

        let (_, tool) = send_to_peer_pod_tool(discovery)();
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
    async fn probe_socket_reads_status_after_replayed_alert() {
        let root = TempDir::new().unwrap();
        let socket = root.path().join("pod.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Alert(Alert {
                    level: AlertLevel::Warn,
                    source: AlertSource::Pod,
                    message: "replayed alert".into(),
                    timestamp_ms: 0,
                }))
                .await
                .unwrap();
            writer
                .write(&Event::Snapshot {
                    entries: Vec::new(),
                    greeting: protocol::Greeting {
                        pod_name: "alerted".into(),
                        cwd: "/tmp".into(),
                        provider: "test".into(),
                        model: "test".into(),
                        scope_summary: String::new(),
                        tools: Vec::new(),
                        context_window: 0,
                        context_tokens: 0,
                    },
                    status: PodStatus::Paused,
                })
                .await
                .unwrap();
        });

        let info = probe_socket(&socket).await;
        assert!(info.reachable);
        assert!(matches!(info.status, Some(PodStatus::Paused)));
        handle.await.unwrap();
    }

    fn child(name: &str, socket_path: &Path) -> PodSpawnedChild {
        PodSpawnedChild {
            pod_name: name.to_string(),
            socket_path: socket_path.to_path_buf(),
            scope_delegated: vec![PodSpawnedScopeRule {
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
                                pod_name: "child-live".into(),
                                cwd: "/tmp".into(),
                                provider: "test".into(),
                                model: "test".into(),
                                scope_summary: String::new(),
                                tools: Vec::new(),
                                context_window: 0,
                                context_tokens: 0,
                            },
                            status: PodStatus::Idle,
                        })
                        .await;
                });
            }
        })
    }
}
