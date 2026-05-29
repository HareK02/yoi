//! Pod-state-backed discovery and restore/attach tools.
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
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use pod_store::{PodActiveSegmentRef, PodMetadata, PodMetadataStore};
use protocol::stream::JsonLineReader;
use protocol::{Event, PodStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use session_store::{SegmentId, SessionId};
use tokio::net::UnixStream;
use tokio::process::Command;

use crate::runtime::pod_registry;
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

    pub async fn attach_or_restore(
        &self,
        pod_name: &str,
    ) -> Result<AttachRestoreResult, PodDiscoveryError> {
        match self.plan_attach_or_restore(pod_name).await? {
            AttachRestorePlan::Attach {
                pod_name,
                socket_path,
                status,
            } => Ok(AttachRestoreResult::Attached {
                pod_name,
                socket_path,
                status,
            }),
            AttachRestorePlan::Restore {
                pod_name,
                socket_path,
            } => {
                self.spawn_restore_process(&pod_name, &socket_path).await?;
                Ok(AttachRestoreResult::Restored {
                    pod_name,
                    socket_path,
                })
            }
        }
    }

    pub async fn plan_attach_or_restore(
        &self,
        pod_name: &str,
    ) -> Result<AttachRestorePlan, PodDiscoveryError> {
        let detail = self.inspect(pod_name).await?;
        if detail.live.reachable {
            return Ok(AttachRestorePlan::Attach {
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
                Ok(AttachRestorePlan::Attach {
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

        Ok(AttachRestorePlan::Restore {
            pod_name: pod_name.to_string(),
            socket_path: self.default_socket_path(pod_name),
        })
    }

    async fn visibility(&self) -> Result<VisibilitySet, PodDiscoveryError> {
        let mut visible = BTreeMap::new();
        let mut child_sockets = BTreeMap::new();
        visible.insert(self.self_pod_name.clone(), VisibilityReason::SelfPod);

        // Durable parent -> child state is the primary visibility source.
        if let Some(metadata) = self.store.read_by_name(&self.self_pod_name)? {
            for child in metadata.spawned_children {
                visible
                    .entry(child.pod_name.clone())
                    .or_insert(VisibilityReason::SpawnedChild);
                child_sockets.insert(child.pod_name, child.socket_path);
            }
        }

        // The live in-memory registry covers just-spawned children even if a
        // state write failed after the process became reachable. It is an
        // additive visibility hint, not the source of Pod metadata.
        for record in self.spawned_registry.list().await {
            visible
                .entry(record.pod_name.clone())
                .or_insert(VisibilityReason::SpawnedChild);
            child_sockets.insert(record.pod_name, record.socket_path);
        }

        Ok(VisibilitySet {
            visible,
            child_sockets,
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
        let mut command = Command::new(resolve_pod_command());
        command
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

        let mut child = command.spawn().map_err(PodDiscoveryError::RestoreSpawn)?;
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
pub struct VisiblePodItem {
    pub pod_name: String,
    pub visibility: VisibilityReason,
    pub state: PodStateStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<ActivePointer>,
    pub live: LiveInfo,
    pub restore: RestoreInfo,
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
    pub spawned_children: SpawnedChildrenSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AttachRestorePlan {
    Attach {
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
pub enum AttachRestoreResult {
    Attached {
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

#[derive(Debug, thiserror::Error)]
pub enum PodDiscoveryError {
    #[error("pod state missing for `{pod_name}`")]
    StateMissing { pod_name: String },
    #[error("pod `{pod_name}` is not visible to this Pod")]
    NotVisible { pod_name: String },
    #[error("pod `{pod_name}` is not restorable: {reason}")]
    NotRestorable { pod_name: String, reason: String },
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
    #[error("restore process exited before socket became reachable: {status}")]
    RestoreExited { status: std::process::ExitStatus },
    #[error("restore process did not become reachable before timeout")]
    RestoreTimeout,
}

struct VisibilitySet {
    visible: BTreeMap<String, VisibilityReason>,
    child_sockets: BTreeMap<String, PathBuf>,
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

fn resolve_pod_command() -> PathBuf {
    if let Ok(cmd) = std::env::var("INSOMNIA_POD_COMMAND")
        && !cmd.is_empty()
    {
        return PathBuf::from(cmd);
    }
    PathBuf::from("insomnia-pod")
}

#[derive(Debug, Deserialize, JsonSchema)]
struct PodNameInput {
    /// Pod name to inspect, attach, or restore.
    name: String,
}

struct ListVisiblePodsTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for ListVisiblePodsTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(&self, _input_json: &str) -> Result<ToolOutput, ToolError> {
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

struct InspectPodTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for InspectPodTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: PodNameInput = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid InspectPod input: {e}")))?;
        let detail = self
            .discovery
            .inspect(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        Ok(ToolOutput {
            summary: format!("pod `{}` inspected", detail.pod_name),
            content: Some(json_content(&detail)?),
        })
    }
}

struct AttachOrRestorePodTool<St> {
    discovery: PodDiscovery<St>,
}

#[async_trait]
impl<St> Tool for AttachOrRestorePodTool<St>
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let input: PodNameInput = serde_json::from_str(input_json).map_err(|e| {
            ToolError::InvalidArgument(format!("invalid AttachOrRestorePod input: {e}"))
        })?;
        let result = self
            .discovery
            .attach_or_restore(&input.name)
            .await
            .map_err(discovery_error_to_tool_error)?;
        let summary = match &result {
            AttachRestoreResult::Attached { pod_name, .. } => {
                format!("pod `{pod_name}` is live; attached to existing socket")
            }
            AttachRestoreResult::Restored { pod_name, .. } => {
                format!("pod `{pod_name}` restored from pod state")
            }
        };
        Ok(ToolOutput {
            summary,
            content: Some(json_content(&result)?),
        })
    }
}

pub fn list_visible_pods_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("ListVisiblePods")
            .description(
                "List Pod state entries visible to this Pod. This is state-backed and does not expose the host-wide Pod universe.",
            )
            .input_schema(serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }));
        let tool: Arc<dyn Tool> = Arc::new(ListVisiblePodsTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

pub fn inspect_pod_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("InspectPod")
            .description(
                "Inspect one visible Pod by name from durable Pod state, distinguishing missing state from not-visible Pods.",
            )
            .input_schema(serde_json::to_value(schemars::schema_for!(PodNameInput)).unwrap());
        let tool: Arc<dyn Tool> = Arc::new(InspectPodTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

pub fn attach_or_restore_pod_tool<St>(discovery: PodDiscovery<St>) -> ToolDefinition
where
    St: PodMetadataStore + Clone + Send + Sync + 'static,
{
    Arc::new(move || {
        let meta = ToolMeta::new("AttachOrRestorePod")
            .description(
                "Attach to a visible live Pod, or restore it from Pod state when no live socket is reachable. Missing state is an error.",
            )
            .input_schema(serde_json::to_value(schemars::schema_for!(PodNameInput)).unwrap());
        let tool: Arc<dyn Tool> = Arc::new(AttachOrRestorePodTool {
            discovery: discovery.clone(),
        });
        (meta, tool)
    })
}

fn json_content<T: Serialize>(value: &T) -> Result<String, ToolError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| ToolError::Internal(format!("serialize pod discovery output: {e}")))
}

fn discovery_error_to_tool_error(error: PodDiscoveryError) -> ToolError {
    match error {
        PodDiscoveryError::StateMissing { .. }
        | PodDiscoveryError::NotVisible { .. }
        | PodDiscoveryError::NotRestorable { .. } => ToolError::InvalidArgument(error.to_string()),
        PodDiscoveryError::LockConflict { .. }
        | PodDiscoveryError::Store(_)
        | PodDiscoveryError::PodStore(_)
        | PodDiscoveryError::ScopeLock(_)
        | PodDiscoveryError::RestoreSpawn(_)
        | PodDiscoveryError::RestoreExited { .. }
        | PodDiscoveryError::RestoreTimeout => ToolError::ExecutionFailed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use manifest::{Permission, ScopeRule};
    use pod_store::{FsPodStore, PodSpawnedChild, PodSpawnedScopeRule};
    use protocol::stream::JsonLineWriter;
    use protocol::{Alert, AlertLevel, AlertSource, Greeting};
    use session_store::{new_segment_id, new_session_id};
    use tempfile::TempDir;
    use tokio::net::UnixListener;

    use crate::runtime::dir::RuntimeDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test(flavor = "current_thread")]
    async fn state_backed_visibility_and_attach_restore_planning() {
        let _env = ENV_LOCK.lock().unwrap();
        let root = TempDir::new().unwrap();
        let store_dir = root.path().join("store");
        let runtime_base = root.path().join("runtime");
        std::fs::create_dir_all(&runtime_base).unwrap();
        unsafe {
            std::env::set_var("INSOMNIA_RUNTIME_DIR", &runtime_base);
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
                resolved_manifest_snapshot: None,
            })
            .unwrap();
        store
            .write(&PodMetadata {
                pod_name: "child-pending".into(),
                active: Some(PodActiveSegmentRef::pending_segment(pending_session_id)),
                spawned_children: Vec::new(),
                reclaimed_children: Vec::new(),
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

        let list = discovery.list_visible().await.unwrap();
        let names: Vec<_> = list.iter().map(|p| p.pod_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["child-live", "child-pending", "child-stale", "parent"]
        );
        assert!(!names.contains(&"hidden"));
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
        let hidden_restore_err = discovery
            .plan_attach_or_restore("hidden")
            .await
            .unwrap_err();
        assert!(matches!(
            hidden_restore_err,
            PodDiscoveryError::NotVisible { .. }
        ));

        let attach_plan = discovery
            .plan_attach_or_restore("child-live")
            .await
            .unwrap();
        assert!(matches!(attach_plan, AttachRestorePlan::Attach { .. }));
        let restore_plan = discovery
            .plan_attach_or_restore("child-stale")
            .await
            .unwrap();
        assert!(matches!(restore_plan, AttachRestorePlan::Restore { .. }));

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
        let locked_err = discovery
            .plan_attach_or_restore("child-stale")
            .await
            .unwrap_err();
        assert!(matches!(locked_err, PodDiscoveryError::LockConflict { .. }));

        live_listener.abort();
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
                    greeting: Greeting {
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
                            greeting: Greeting {
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
