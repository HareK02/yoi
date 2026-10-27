//! Local sessions retaining scope and provider resources.
//!
//! Scope determines read/write bounds; cwd is the reference for searches and operations.
//! Clones share resources, but Tools own per-attachment read history. Checked observe/execute/search
//! and regular trait Read/Write/Edit share a nonqueueing session lock, serializing validator
//! comparison through I/O. This does not exclude other sessions/processes or external editors.
//!
//! Checkout uses Linux openat2 and /proc/self/fd, rejecting symlinks, special files, mount crossings,
//! absolute/traversal paths, and missing primitives. It does not implicitly expand regular Tools'
//! explicit symlink policies. Hashes bounded sources and uses shared fs-operation engines for
//! editing and temporary-file saves. Cooperative deadlines cannot forcibly interrupt kernel I/O,
//! so they provide no hard wall-time guarantee for hung storage.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
#[cfg(test)]
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use manifest::{Permission, Scope, ScopeConfig, ScopeRule, SharedScope, SymlinkPolicy};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

use crate::{
    BoundedReadLimits, CommandEvent, CommandHandle, CommandOutput, CommandOutputRequest,
    CommandRequest, CommandSnapshot, CommandStatus, CommandStream, CommandStreamSlice, EditRequest,
    EditResult, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest, ListResult,
    ReadBytesRequest, ReadBytesResult, ReadRequest, ReadResult, StatRequest, StatResult, Workdir,
    WorkdirError, WorkdirPath, WorkdirScopeAuthorizationRequest, WorkdirScopeOverlapRequest,
    WorkdirSession, WorkdirSessionCapabilities, WorkdirSessionCapability,
    WorkdirToolScopePermission, WriteRequest, WriteResult,
};
#[cfg(test)]
use crate::{EntryKind, WriteOutcome};

const COMMAND_EVENT_CHANNEL_CAPACITY: usize = 256;
const COMMAND_EVENT_CHUNK_BYTES: usize = 8 * 1024;
const COMMAND_SNAPSHOT_STREAM_BYTES: usize = 32 * 1024;

fn command_observed_at_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

mod command_transport;
use command_transport::LocalCommand;

#[derive(Debug, Clone)]
struct CommandTelemetry {
    inner: Arc<CommandTelemetryInner>,
}

#[derive(Debug)]
struct CommandTelemetryInner {
    snapshots: StdMutex<HashMap<String, CommandSnapshot>>,
    events: broadcast::Sender<CommandEvent>,
}

impl CommandTelemetry {
    fn new() -> Self {
        let (events, _) = broadcast::channel(COMMAND_EVENT_CHANNEL_CAPACITY);
        Self {
            inner: Arc::new(CommandTelemetryInner {
                snapshots: StdMutex::new(HashMap::new()),
                events,
            }),
        }
    }

    fn subscribe(&self) -> broadcast::Receiver<CommandEvent> {
        self.inner.events.subscribe()
    }

    fn snapshot(&self) -> Vec<CommandSnapshot> {
        let mut snapshots = self
            .inner
            .snapshots
            .lock()
            .expect("command telemetry mutex poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        snapshots.sort_by(|left, right| left.command_id.cmp(&right.command_id));
        snapshots
    }

    fn started(&self, command_id: &str, tool_call_id: Option<String>) {
        let observed_at_ms = command_observed_at_ms();
        self.inner
            .snapshots
            .lock()
            .expect("command telemetry mutex poisoned")
            .insert(
                command_id.to_string(),
                CommandSnapshot {
                    command_id: command_id.to_string(),
                    tool_call_id: tool_call_id.clone(),
                    status: CommandStatus::Running,
                    started_at_ms: observed_at_ms,
                    observed_at_ms,
                    last_output_at_ms: None,
                    stdout: CommandStreamSlice::default(),
                    stderr: CommandStreamSlice::default(),
                    exit_code: None,
                },
            );
        let _ = self.inner.events.send(CommandEvent::Started {
            command_id: command_id.to_string(),
            tool_call_id,
            observed_at_ms,
        });
    }

    fn output(&self, command_id: &str, stream: CommandStream, start_offset: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let end_offset = start_offset.saturating_add(bytes.len() as u64);
        let content = String::from_utf8_lossy(bytes).into_owned();
        let observed_at_ms = command_observed_at_ms();
        if let Some(snapshot) = self
            .inner
            .snapshots
            .lock()
            .expect("command telemetry mutex poisoned")
            .get_mut(command_id)
        {
            snapshot.observed_at_ms = observed_at_ms;
            snapshot.last_output_at_ms = Some(observed_at_ms);
            let target = match stream {
                CommandStream::Stdout => &mut snapshot.stdout,
                CommandStream::Stderr => &mut snapshot.stderr,
            };
            target.end_offset = end_offset;
            target.content.push_str(&content);
            if target.content.len() > COMMAND_SNAPSHOT_STREAM_BYTES {
                let mut cut = target.content.len() - COMMAND_SNAPSHOT_STREAM_BYTES;
                while cut < target.content.len() && !target.content.is_char_boundary(cut) {
                    cut += 1;
                }
                target.content.drain(..cut);
                target.start_offset = end_offset.saturating_sub(target.content.len() as u64);
                target.truncated = true;
            }
        }
        let _ = self.inner.events.send(CommandEvent::Output {
            command_id: command_id.to_string(),
            stream,
            start_offset,
            end_offset,
            content,
            observed_at_ms,
        });
    }

    fn terminal(&self, command_id: &str, status: CommandStatus, exit_code: Option<i32>) {
        let observed_at_ms = command_observed_at_ms();
        let (stdout_end_offset, stderr_end_offset) = if let Some(snapshot) = self
            .inner
            .snapshots
            .lock()
            .expect("command telemetry mutex poisoned")
            .get_mut(command_id)
        {
            snapshot.status = status;
            snapshot.exit_code = exit_code;
            snapshot.observed_at_ms = observed_at_ms;
            (snapshot.stdout.end_offset, snapshot.stderr.end_offset)
        } else {
            (0, 0)
        };
        let _ = self.inner.events.send(CommandEvent::Terminal {
            command_id: command_id.to_string(),
            status,
            exit_code,
            stdout_end_offset,
            stderr_end_offset,
            observed_at_ms,
        });
    }

    fn remove(&self, command_id: &str) {
        self.inner
            .snapshots
            .lock()
            .expect("command telemetry mutex poisoned")
            .remove(command_id);
    }
}

#[derive(Debug, Clone)]
struct ScopeAccess {
    scope: Arc<Scope>,
    root: PathBuf,
    pinned_root: Option<Arc<std::fs::File>>,
    reject_symlinks: bool,
    operation_guard: Option<OperationGuard>,
}

/// Policy refusals carried through FsAccessPolicy's I/O interface are not OS
/// PermissionDenied. This marker contains only fixed text and a closed reason.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
struct PolicyPermissionDenied {
    reason: crate::WorkdirDenialReason,
    message: &'static str,
}

fn policy_permission_denied(
    reason: crate::WorkdirDenialReason,
    message: &'static str,
) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        PolicyPermissionDenied { reason, message },
    )
}

pub(crate) fn io_denial_reason(error: &std::io::Error) -> crate::WorkdirDenialReason {
    use crate::WorkdirDenialReason as Reason;
    if let Some(source) = error.get_ref() {
        if let Some(denial) = source.downcast_ref::<PolicyPermissionDenied>() {
            return denial.reason;
        }
        if let Some(denial) = source.downcast_ref::<fs_operation::FsDenialReason>() {
            return match denial {
                fs_operation::FsDenialReason::ProviderRootExceeded => Reason::ProviderRootExceeded,
                fs_operation::FsDenialReason::ProviderSymlinkDenied => {
                    Reason::ProviderSymlinkDenied
                }
                fs_operation::FsDenialReason::CheckoutTargetDenied => Reason::CheckoutTargetDenied,
                fs_operation::FsDenialReason::CheckoutCreateParentDenied => {
                    Reason::CheckoutCreateParentDenied
                }
            };
        }
    }
    if error.raw_os_error().is_some() {
        Reason::OsPermissionDenied
    } else {
        Reason::UnclassifiedPermissionDenied
    }
}

impl ScopeAccess {
    fn new(
        scope: Arc<Scope>,
        root: &Path,
        pinned_root: Option<Arc<std::fs::File>>,
        reject_symlinks: bool,
        operation_guard: Option<OperationGuard>,
    ) -> Self {
        Self {
            scope,
            root: root.to_path_buf(),
            pinned_root,
            reject_symlinks,
            operation_guard,
        }
    }

    fn open_confined(&self, path: &Path) -> std::io::Result<std::fs::File> {
        let root = self.pinned_root.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "External Workdir root handle is unavailable",
            )
        })?;
        let relative = path.strip_prefix(&self.root).map_err(|_| {
            policy_permission_denied(
                crate::WorkdirDenialReason::ProviderRootExceeded,
                "path is outside provider root",
            )
        })?;
        fs_operation::open_beneath_no_symlinks_at(root, relative)
    }

    fn path_is_confined(&self, logical: &Path, resolved: &Path) -> bool {
        !self.reject_symlinks
            || (logical.starts_with(&self.root) && resolved.starts_with(&self.root))
    }
}

impl fs_operation::FsAccessPolicy for ScopeAccess {
    fn is_readable(&self, path: &Path) -> bool {
        self.scope.is_readable(path)
    }

    fn is_writable(&self, path: &Path) -> bool {
        self.scope.is_writable(path)
    }

    fn check_cancelled(&self) -> std::io::Result<()> {
        let Some(guard) = &self.operation_guard else {
            return Ok(());
        };
        if guard.cancelled.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "External Workdir operation was cancelled",
            ));
        }
        if Instant::now() >= guard.deadline {
            guard.cancelled.store(true, Ordering::Release);
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "External Workdir operation exceeded its provider deadline",
            ));
        }
        Ok(())
    }

    fn resolve_access_path(&self, logical: &Path) -> std::io::Result<PathBuf> {
        if !self.reject_symlinks {
            return fs_operation::resolve_access_path(logical);
        }
        if logical.starts_with(&self.root) {
            Ok(logical.to_path_buf())
        } else {
            Err(policy_permission_denied(
                crate::WorkdirDenialReason::ProviderRootExceeded,
                "path is outside provider root",
            ))
        }
    }

    fn open_traversal_root(
        &self,
        _logical: &Path,
        resolved: &Path,
    ) -> std::io::Result<Option<fs_operation::FsTraversalRoot>> {
        if !self.reject_symlinks {
            return Ok(None);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;

            let directory = self.open_confined(resolved)?;
            if !directory.metadata()?.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "path is not a directory",
                ));
            }
            let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
            return Ok(Some(fs_operation::FsTraversalRoot::new_at(
                path,
                directory,
                resolved.to_path_buf(),
            )));
        }
        #[cfg(not(target_os = "linux"))]
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "descriptor-rooted traversal is unavailable on this platform",
        ))
    }

    fn open_read_file(&self, _logical: &Path, resolved: &Path) -> std::io::Result<std::fs::File> {
        if self.reject_symlinks {
            self.open_confined(resolved)
        } else {
            std::fs::File::open(resolved)
        }
    }

    fn read_metadata(
        &self,
        _logical: &Path,
        resolved: &Path,
    ) -> std::io::Result<std::fs::Metadata> {
        if self.reject_symlinks {
            self.open_confined(resolved)?.metadata()
        } else {
            std::fs::metadata(resolved)
        }
    }

    fn max_write_bytes(&self) -> Option<usize> {
        self.reject_symlinks
            .then_some(crate::external::MAX_EXTERNAL_WRITE_BYTES)
    }

    fn max_edit_replacements(&self) -> Option<usize> {
        self.reject_symlinks
            .then_some(crate::external::MAX_EXTERNAL_RESULT_ITEMS)
    }

    fn atomic_write_file(
        &self,
        _logical: &Path,
        resolved: &Path,
        content: &[u8],
        mode: fs_operation::AtomicWriteMode,
    ) -> std::io::Result<()> {
        if !self.reject_symlinks {
            use std::io::Write;

            let parent = resolved.parent().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "file path has no parent")
            })?;
            std::fs::create_dir_all(parent)?;
            let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
            temporary.write_all(content)?;
            temporary.flush()?;
            temporary.as_file().sync_all()?;
            self.check_cancelled()?;
            match mode {
                fs_operation::AtomicWriteMode::CreateNew => temporary
                    .persist_noclobber(resolved)
                    .map_err(|error| error.error)?,
                fs_operation::AtomicWriteMode::Replace => {
                    temporary.persist(resolved).map_err(|error| error.error)?
                }
            };
            return Ok(());
        }
        let root = self.pinned_root.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "External Workdir root handle is unavailable",
            )
        })?;
        let relative = resolved.strip_prefix(&self.root).map_err(|_| {
            policy_permission_denied(
                crate::WorkdirDenialReason::ProviderRootExceeded,
                "path is outside provider root",
            )
        })?;
        fs_operation::atomic_write_beneath_no_symlinks_at(root, relative, content, mode, || {
            self.check_cancelled()
        })
    }

    fn open_read_dir(&self, _logical: &Path, resolved: &Path) -> std::io::Result<std::fs::ReadDir> {
        if !self.reject_symlinks {
            return std::fs::read_dir(resolved);
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;

            let directory = self.open_confined(resolved)?;
            if !directory.metadata()?.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    "path is not a directory",
                ));
            }
            return std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        }
        #[cfg(not(target_os = "linux"))]
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "root-confined directory enumeration is unavailable on this platform",
        ))
    }

    fn is_readable_paths(&self, logical: &Path, resolved: &Path) -> bool {
        self.path_is_confined(logical, resolved)
            && matches!(
                self.scope.permission_at_paths(logical, resolved),
                Some(Permission::Read | Permission::Write)
            )
    }

    fn is_writable_paths(&self, logical: &Path, resolved: &Path) -> bool {
        self.path_is_confined(logical, resolved)
            && self.scope.permission_at_paths(logical, resolved) == Some(Permission::Write)
    }
}

/// A read-only, conjunctively narrowed descriptor policy for native searches.
#[derive(Debug, Clone)]
struct SearchAccess {
    inner: ScopeAccess,
    layers: Vec<Vec<crate::WorkdirToolScopeRule>>,
    output_root: PathBuf,
    source_allows: Arc<Vec<manifest::ScopeRule>>,
}
impl SearchAccess {
    fn logical_path(&self, path: &Path) -> Option<WorkdirPath> {
        WorkdirPath::new(path.strip_prefix(&self.inner.root).ok()?.to_str()?).ok()
    }
    fn read_allowed(&self, logical: &Path, resolved: &Path) -> bool {
        use fs_operation::FsAccessPolicy;
        let Some(path) = self.logical_path(logical) else {
            return false;
        };
        logical.starts_with(&self.output_root)
            && self.inner.is_readable_paths(logical, resolved)
            && self.layers.iter().all(|layer| {
                layer.iter().any(|rule| {
                    crate::scope::rule_allows_path(rule, &path, WorkdirToolScopePermission::Read)
                })
            })
    }
    fn enumeration_allowed(&self, logical: &Path, resolved: &Path) -> bool {
        self.read_allowed(logical, resolved)
            && self.source_allows.iter().any(|rule| {
                if rule.recursive {
                    logical.starts_with(&rule.target) || rule.target.starts_with(logical)
                } else {
                    logical == rule.target || rule.target.starts_with(logical)
                }
            })
            && self.layers.iter().all(|layer| {
                layer.iter().any(|rule| {
                    let target = self.inner.root.join(rule.target.as_str());
                    if rule.recursive {
                        logical.starts_with(&target) || target.starts_with(logical)
                    } else {
                        logical == target || target.starts_with(logical)
                    }
                })
            })
    }
    fn require_read(&self, logical: &Path, resolved: &Path) -> std::io::Result<()> {
        if self.read_allowed(logical, resolved) {
            Ok(())
        } else {
            Err(policy_permission_denied(
                crate::WorkdirDenialReason::CheckoutSearchPathDenied,
                "checkout search path denied",
            ))
        }
    }
}
impl fs_operation::FsAccessPolicy for SearchAccess {
    fn is_readable(&self, path: &Path) -> bool {
        self.read_allowed(path, path)
    }
    fn is_writable(&self, _: &Path) -> bool {
        false
    }
    fn is_readable_paths(&self, logical: &Path, resolved: &Path) -> bool {
        self.read_allowed(logical, resolved)
    }
    fn can_enumerate_directory(&self, logical: &Path, resolved: &Path) -> bool {
        self.enumeration_allowed(logical, resolved)
    }
    fn traversal_filter(&self) -> Option<fs_operation::FsTraversalFilter> {
        let policy = self.clone();
        Some(Arc::new(move |path, directory| {
            if directory {
                policy.enumeration_allowed(path, path)
            } else {
                policy.read_allowed(path, path)
            }
        }))
    }
    fn check_cancelled(&self) -> std::io::Result<()> {
        self.inner.check_cancelled()
    }
    fn resolve_access_path(&self, logical: &Path) -> std::io::Result<PathBuf> {
        self.inner.resolve_access_path(logical)
    }
    fn open_traversal_root(
        &self,
        logical: &Path,
        resolved: &Path,
    ) -> std::io::Result<Option<fs_operation::FsTraversalRoot>> {
        self.require_read(logical, resolved)?;
        self.inner.open_traversal_root(logical, resolved)
    }
    fn open_read_file(&self, logical: &Path, resolved: &Path) -> std::io::Result<std::fs::File> {
        self.require_read(logical, resolved)?;
        self.inner.open_read_file(logical, resolved)
    }
    fn read_metadata(&self, logical: &Path, resolved: &Path) -> std::io::Result<std::fs::Metadata> {
        self.require_read(logical, resolved)?;
        self.inner.read_metadata(logical, resolved)
    }
    fn open_read_dir(&self, logical: &Path, resolved: &Path) -> std::io::Result<std::fs::ReadDir> {
        if !self.enumeration_allowed(logical, resolved) {
            return Err(policy_permission_denied(
                crate::WorkdirDenialReason::CheckoutEnumerationDenied,
                "checkout directory enumeration denied",
            ));
        }
        self.inner.open_read_dir(logical, resolved)
    }
}

fn path_sets_overlap(
    left: &Path,
    left_recursive: bool,
    right: &Path,
    right_recursive: bool,
) -> bool {
    match (left_recursive, right_recursive) {
        (true, true) => left.starts_with(right) || right.starts_with(left),
        (true, false) => {
            right.starts_with(left)
                || left == right
                || left.parent().is_some_and(|parent| parent == right)
        }
        (false, true) => {
            left.starts_with(right)
                || left == right
                || right.parent().is_some_and(|parent| parent == left)
        }
        (false, false) => {
            left == right
                || left.parent().is_some_and(|parent| parent == right)
                || right.parent().is_some_and(|parent| parent == left)
        }
    }
}

fn rule_targets(
    root: &Path,
    rule: &crate::WorkdirToolScopeRule,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let logical = root.join(rule.target.as_str());
    let resolved = fs_operation::resolve_access_path(&logical)?;
    Ok((logical, resolved))
}

#[derive(Debug)]
struct LocalWorkdirSessionInner {
    workdir: Workdir,
    root: PathBuf,
    pinned_root: Option<Arc<std::fs::File>>,
    scope: SharedScope,
    cwd: PathBuf,
    capabilities: WorkdirSessionCapabilities,
    read_limits: Option<BoundedReadLimits>,
    reject_symlinks: bool,
    closed: AtomicBool,
    close_lock: Mutex<()>,
    checkout_lock: StdMutex<()>,
    next_command_id: AtomicU64,
    commands: Mutex<HashMap<String, LocalCommand>>,
    command_telemetry: CommandTelemetry,
    command_environment: BTreeMap<String, String>,
    resources: StdMutex<Vec<Arc<dyn WorkdirSessionResource>>>,
}

impl Drop for LocalWorkdirSessionInner {
    fn drop(&mut self) {
        if let Ok(mut commands) = self.commands.try_lock() {
            for (_, command) in commands.drain() {
                command.task.abort();
            }
        }
    }
}

pub trait WorkdirSessionResource: Debug + Send + Sync {}
impl<T> WorkdirSessionResource for T where T: Debug + Send + Sync {}

/// External provider root with identity checked and retained before approval.
///
/// Compares selected-path metadata with the opened descriptor, rejecting renames/replacements
/// during acquisition. Sessions from this root use the descriptor as filesystem authority;
/// replacing the ambient pathname after approval does not transfer authority to another tree.
#[derive(Debug, Clone)]
pub struct ExternalWorkdirRoot {
    canonical_path: PathBuf,
    handle: Arc<std::fs::File>,
}

impl ExternalWorkdirRoot {
    pub fn pin(directory: impl AsRef<Path>) -> Result<Self, WorkdirError> {
        let directory = directory.as_ref();
        if !directory.is_absolute() {
            return Err(WorkdirError::InvalidArgument(
                "External Workdir directory must be absolute".to_string(),
            ));
        }
        let selected_metadata = std::fs::symlink_metadata(directory)
            .map_err(|error| WorkdirError::io(Path::new("<external-root>"), error))?;
        if selected_metadata.file_type().is_symlink() {
            return Err(WorkdirError::denied(
                crate::WorkdirDenialReason::ExternalRootSymlink,
                "External Workdir root must not be a symbolic link".to_string(),
            ));
        }
        if !selected_metadata.is_dir() {
            return Err(WorkdirError::InvalidArgument(
                "External Workdir root must be a directory".to_string(),
            ));
        }

        let handle = Arc::new(
            fs_operation::open_root_no_symlinks(directory)
                .map_err(|error| WorkdirError::io(Path::new("<external-root>"), error))?,
        );
        let opened_metadata = handle
            .metadata()
            .map_err(|error| WorkdirError::io(Path::new("<external-root>"), error))?;
        if !opened_metadata.is_dir() {
            return Err(WorkdirError::InvalidArgument(
                "External Workdir root must be a directory".to_string(),
            ));
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;

            if (selected_metadata.dev(), selected_metadata.ino())
                != (opened_metadata.dev(), opened_metadata.ino())
            {
                return Err(WorkdirError::denied(
                    crate::WorkdirDenialReason::ExternalRootChanged,
                    "External Workdir root changed while it was being pinned".to_string(),
                ));
            }
        }

        #[cfg(target_os = "linux")]
        let canonical_path = {
            use std::os::fd::AsRawFd;

            let path = std::fs::read_link(format!("/proc/self/fd/{}", handle.as_raw_fd()))
                .map_err(|error| WorkdirError::io(Path::new("<external-root>"), error))?;
            if !path.is_absolute() || path.to_string_lossy().ends_with(" (deleted)") {
                return Err(WorkdirError::denied(
                    crate::WorkdirDenialReason::ExternalRootMoved,
                    "External Workdir root moved while it was being pinned".to_string(),
                ));
            }
            path
        };
        #[cfg(not(target_os = "linux"))]
        let canonical_path = directory.to_path_buf();

        Ok(Self {
            canonical_path,
            handle,
        })
    }

    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }
}

#[derive(Debug, Clone)]
struct OperationGuard {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}

/// Scope-aware filesystem handle. Clone-cheap (`Arc` inside).
///
/// The wrapped [`SharedScope`] is shared with every clone of this
/// `LocalWorkdirSession` and with whoever else holds the same `SharedScope`
/// handle (typically the owning Worker). Mutations to that `SharedScope`
/// propagate atomically; the next permission check inside any
/// `LocalWorkdirSession` reads the new view.
#[derive(Debug, Clone)]
pub struct LocalWorkdirSession {
    inner: Arc<LocalWorkdirSessionInner>,
    operation_guard: Option<OperationGuard>,
}

/// First symlink encountered while resolving a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkInfo {
    /// The symlink path as it appears in the original path chain.
    pub link_path: PathBuf,
    /// The symlink target resolved relative to the symlink's parent when the
    /// link stores a relative target.
    pub target_path: PathBuf,
    /// Best-effort resolved form of the full requested path after replacing
    /// the symlink component with its target and rejoining any remaining tail.
    /// Existing targets are canonicalized; broken targets are left absolute.
    pub resolved_path: PathBuf,
    /// Whether the symlink target itself exists. A missing target is a broken
    /// symlink even when the symlink lives inside an allowed scope.
    pub target_exists: bool,
}

fn local_workdir_identity(root: &Path) -> Workdir {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let digest = Sha256::digest(canonical.to_string_lossy().as_bytes());
    let digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Workdir::new(format!("local-{digest}"))
}

impl LocalWorkdirSession {
    /// Create a new [`LocalWorkdirSession`] wrapping `scope` and `cwd` in a fresh
    /// [`SharedScope`]. Use [`LocalWorkdirSession::with_shared_scope`] when you
    /// need the resulting `LocalWorkdirSession` to share scope state with another
    /// holder of the `SharedScope` (typically the Worker).
    pub fn new(scope: Scope, cwd: PathBuf) -> Self {
        Self::materialized(
            cwd.clone(),
            cwd,
            SharedScope::new(scope),
            WorkdirSessionCapabilities::ALL,
        )
    }

    pub fn with_shared_scope(scope: SharedScope, cwd: PathBuf) -> Self {
        Self::materialized(cwd.clone(), cwd, scope, WorkdirSessionCapabilities::ALL)
    }

    /// Construct a standalone local session with a deterministic identity
    /// derived from the canonical materialization root.
    pub fn materialized(
        root: PathBuf,
        cwd: PathBuf,
        scope: SharedScope,
        capabilities: WorkdirSessionCapabilities,
    ) -> Self {
        let workdir = local_workdir_identity(&root);
        Self::materialized_bound(workdir, root, cwd, scope, capabilities)
    }

    /// Open a local session for an authority-assigned persistent Workdir.
    pub fn materialized_bound(
        workdir: Workdir,
        root: PathBuf,
        cwd: PathBuf,
        scope: SharedScope,
        capabilities: WorkdirSessionCapabilities,
    ) -> Self {
        Self::materialized_bound_with_environment(
            workdir,
            root,
            cwd,
            scope,
            capabilities,
            BTreeMap::new(),
            Vec::new(),
        )
    }

    pub fn materialized_bound_with_environment(
        workdir: Workdir,
        root: PathBuf,
        cwd: PathBuf,
        scope: SharedScope,
        capabilities: WorkdirSessionCapabilities,
        command_environment: BTreeMap<String, String>,
        resources: Vec<Arc<dyn WorkdirSessionResource>>,
    ) -> Self {
        Self::materialized_bound_with_policy(
            workdir,
            root,
            cwd,
            scope,
            capabilities,
            command_environment,
            resources,
            None,
            None,
            false,
        )
    }

    /// Construct a strictly read-only provider session for an
    /// authority-assigned Workdir and an operator-selected local directory.
    pub fn external_read_only(
        workdir: Workdir,
        directory: impl AsRef<Path>,
        read_limits: BoundedReadLimits,
    ) -> Result<Self, WorkdirError> {
        let root = ExternalWorkdirRoot::pin(directory)?;
        Self::external_read_only_pinned(workdir, root, read_limits)
    }

    /// Construct a filesystem read/write provider session without command
    /// authority for an operator-selected local directory.
    pub fn external_read_write(
        workdir: Workdir,
        directory: impl AsRef<Path>,
        read_limits: BoundedReadLimits,
    ) -> Result<Self, WorkdirError> {
        let root = ExternalWorkdirRoot::pin(directory)?;
        Self::external_read_write_pinned(workdir, root, read_limits)
    }

    /// Construct an External provider session with an exact operator-approved
    /// READ/WRITE/COMMAND category set. WRITE requires READ so callers can
    /// satisfy the read-before-write contract; COMMAND remains independent.
    pub fn external_with_capabilities_pinned(
        workdir: Workdir,
        pinned: ExternalWorkdirRoot,
        read_limits: BoundedReadLimits,
        capabilities: WorkdirSessionCapabilities,
    ) -> Result<Self, WorkdirError> {
        let read = WorkdirSessionCapabilities::READ_ONLY;
        let read_write = WorkdirSessionCapabilities::READ_WRITE;
        let command = WorkdirSessionCapabilities::COMMAND_ONLY;
        if ![
            read,
            read_write,
            command,
            read.union(command),
            read_write.union(command),
        ]
        .contains(&capabilities)
        {
            return Err(WorkdirError::InvalidArgument(
                "External Workdir capabilities must contain at least one of READ or COMMAND, and WRITE requires READ"
                    .to_string(),
            ));
        }
        let permission = if capabilities.supports(WorkdirSessionCapability::Write) {
            Permission::Write
        } else {
            // Command execution still needs provider-internal read access to
            // validate and enter its cwd. This scope is not an exposed READ grant.
            Permission::Read
        };
        Self::external_pinned(workdir, pinned, read_limits, permission, capabilities)
    }

    /// Construct a read-only provider session from a root pinned before remote
    /// grant creation.
    pub fn external_read_only_pinned(
        workdir: Workdir,
        pinned: ExternalWorkdirRoot,
        read_limits: BoundedReadLimits,
    ) -> Result<Self, WorkdirError> {
        Self::external_with_capabilities_pinned(
            workdir,
            pinned,
            read_limits,
            WorkdirSessionCapabilities::READ_ONLY,
        )
    }

    /// Construct a filesystem read/write provider session from a root pinned
    /// before remote grant creation. Command execution remains unavailable.
    pub fn external_read_write_pinned(
        workdir: Workdir,
        pinned: ExternalWorkdirRoot,
        read_limits: BoundedReadLimits,
    ) -> Result<Self, WorkdirError> {
        Self::external_with_capabilities_pinned(
            workdir,
            pinned,
            read_limits,
            WorkdirSessionCapabilities::READ_WRITE,
        )
    }

    /// The selected directory is atomically validated and pinned for the
    /// session lifetime. A symlink selected as the root is rejected, and all
    /// operation paths are confined below the pinned root without following
    /// symbolic links. Read source and response sizes are provider-bounded.
    fn external_pinned(
        workdir: Workdir,
        pinned: ExternalWorkdirRoot,
        read_limits: BoundedReadLimits,
        permission: Permission,
        capabilities: WorkdirSessionCapabilities,
    ) -> Result<Self, WorkdirError> {
        read_limits.validate().map_err(WorkdirError::from)?;
        let root = pinned.canonical_path;
        let scope = Scope::from_config(&ScopeConfig {
            allow: vec![ScopeRule {
                target: root.clone(),
                permission,
                recursive: true,
                symlink_policy: SymlinkPolicy::Resolved,
            }],
            deny: Vec::new(),
        })
        .map_err(|_| {
            WorkdirError::InvalidArgument(
                "External Workdir filesystem scope could not be established".to_string(),
            )
        })?;
        let scope = SharedScope::new(scope);
        Ok(Self::materialized_bound_with_policy(
            workdir,
            root.clone(),
            root,
            scope,
            capabilities,
            BTreeMap::new(),
            Vec::new(),
            Some(pinned.handle),
            Some(read_limits),
            true,
        ))
    }

    /// Return an operation-local clone that cooperatively stops filesystem
    /// work when cancelled or when its provider-side deadline expires.
    pub fn with_operation_guard(&self, cancelled: Arc<AtomicBool>, deadline: Instant) -> Self {
        Self {
            inner: self.inner.clone(),
            operation_guard: Some(OperationGuard {
                cancelled,
                deadline,
            }),
        }
    }

    fn materialized_bound_with_policy(
        workdir: Workdir,
        root: PathBuf,
        cwd: PathBuf,
        scope: SharedScope,
        capabilities: WorkdirSessionCapabilities,
        command_environment: BTreeMap<String, String>,
        resources: Vec<Arc<dyn WorkdirSessionResource>>,
        pinned_root: Option<Arc<std::fs::File>>,
        read_limits: Option<BoundedReadLimits>,
        reject_symlinks: bool,
    ) -> Self {
        Self {
            inner: Arc::new(LocalWorkdirSessionInner {
                workdir,
                root,
                pinned_root,
                scope,
                cwd,
                capabilities,
                read_limits,
                reject_symlinks,
                closed: AtomicBool::new(false),
                close_lock: Mutex::new(()),
                checkout_lock: StdMutex::new(()),
                next_command_id: AtomicU64::new(1),
                commands: Mutex::new(HashMap::new()),
                command_telemetry: CommandTelemetry::new(),
                command_environment,
                resources: StdMutex::new(resources),
            }),
            operation_guard: None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.inner.root
    }

    /// Snapshot the current scope. Cheap; the returned `Arc<Scope>` is
    /// a coherent point-in-time view that subsequent mutations do not
    /// affect.
    pub fn scope(&self) -> Arc<Scope> {
        self.inner.scope.snapshot()
    }

    /// Shared scope handle backing this `LocalWorkdirSession`. Cloning it lets a
    /// caller (usually the Worker) hold the same view and push updates
    /// that are immediately reflected in subsequent permission checks.
    pub fn shared_scope(&self) -> &SharedScope {
        &self.inner.scope
    }

    /// The Worker's working directory. Glob/Grep default their search base
    /// to this path when callers omit an explicit `path` parameter.
    pub fn cwd(&self) -> &Path {
        &self.inner.cwd
    }

    // =========================================================================
    // Read — scope-checked against readability
    // =========================================================================

    /// Read the full contents of `path` as raw bytes.
    ///
    /// Follows symlinks. Rejects directories, relative paths, paths not
    /// readable by the scope, and missing files.
    #[cfg(test)]
    pub(crate) fn read_bytes(&self, path: &Path) -> Result<Vec<u8>, WorkdirError> {
        if !path.is_absolute() {
            return Err(WorkdirError::RelativePath(path.to_path_buf()));
        }
        let symlink = first_symlink(path);
        if let Some(info) = symlink.as_ref()
            && !info.target_exists
        {
            return Err(broken_symlink_error(path, info));
        }
        let scope = self.inner.scope.load();
        if !scope.is_readable(path) {
            return Err(symlink_out_of_scope_or_plain(
                path,
                symlink.as_ref(),
                "read",
                &scope,
            ));
        }
        let meta = std::fs::metadata(path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => WorkdirError::NotFound(path.to_path_buf()),
            _ => WorkdirError::io(path, e),
        })?;
        if meta.is_dir() {
            return Err(if let Some(info) = symlink.as_ref() {
                WorkdirError::SymlinkTargetIsDirectory {
                    path: path.to_path_buf(),
                    target: info.resolved_path.clone(),
                }
            } else {
                WorkdirError::IsDirectory(path.to_path_buf())
            });
        }
        std::fs::read(path).map_err(|e| WorkdirError::io(path, e))
    }

    // =========================================================================
    // Write — scope-checked, atomic
    // =========================================================================

    /// Atomically write `content` to `path`, creating or overwriting it.
    ///
    /// - `path` must be absolute and writable under the scope.
    /// - Paths that are readable but not writable return [`WorkdirError::ReadOnly`].
    /// - Paths outside the scope entirely return [`WorkdirError::OutOfScope`].
    /// - Missing parent directories are created.
    /// - The actual write uses a sibling tempfile + `persist`, so the
    ///   target file transitions atomically between states.
    ///
    /// This method does **not** consult tool-specific read history.
    #[cfg(test)]
    pub(crate) fn write(&self, path: &Path, content: &[u8]) -> Result<WriteOutcome, WorkdirError> {
        if !path.is_absolute() {
            return Err(WorkdirError::RelativePath(path.to_path_buf()));
        }
        let symlink = first_symlink(path);
        let scope = self.inner.scope.load();
        if !scope.is_writable(path) {
            return Err(if scope.is_readable(path) {
                WorkdirError::ReadOnly(path.to_path_buf())
            } else {
                symlink_out_of_scope_or_plain(path, symlink.as_ref(), "write", &scope)
            });
        }
        drop(scope);

        if let Some(info) = symlink.as_ref() {
            if !info.target_exists {
                return Err(broken_symlink_error(path, info));
            }
        }

        // Reject existing directory targets.
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_dir() => {
                return Err(if let Some(info) = symlink.as_ref() {
                    WorkdirError::SymlinkTargetIsDirectory {
                        path: path.to_path_buf(),
                        target: info.resolved_path.clone(),
                    }
                } else {
                    WorkdirError::IsDirectory(path.to_path_buf())
                });
            }
            _ => {}
        }

        let existed = path.exists();
        let write_target = if existed {
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
        } else {
            path.to_path_buf()
        };

        let parent = write_target.parent().ok_or_else(|| {
            WorkdirError::InvalidArgument(format!(
                "path has no parent directory: {}",
                write_target.display()
            ))
        })?;
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| WorkdirError::io(parent, e))?;
        }

        let tmp_parent: &Path = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        let mut tmp = tempfile::NamedTempFile::new_in(tmp_parent)
            .map_err(|e| WorkdirError::io(tmp_parent, e))?;
        tmp.write_all(content)
            .map_err(|e| WorkdirError::io(&write_target, e))?;
        tmp.as_file()
            .sync_all()
            .map_err(|e| WorkdirError::io(&write_target, e))?;
        tmp.persist(&write_target)
            .map_err(|e| WorkdirError::io(&write_target, e.error))?;

        Ok(WriteOutcome {
            bytes_written: content.len(),
            created: !existed,
        })
    }

    fn ensure_open(&self) -> Result<(), WorkdirError> {
        if self.inner.closed.load(Ordering::Acquire) {
            Err(WorkdirError::Unavailable(format!(
                "Workdir session for {} is closed",
                self.inner.workdir.id()
            ))
            .with_failure_reason(crate::WorkdirFailureReason::SessionClosed))
        } else {
            Ok(())
        }
    }

    fn ensure_capability(&self, capability: WorkdirSessionCapability) -> Result<(), WorkdirError> {
        self.ensure_open()?;
        if self.inner.capabilities.supports(capability) {
            Ok(())
        } else {
            Err(WorkdirError::Unsupported(capability))
        }
    }

    fn resolve(&self, path: &WorkdirPath) -> PathBuf {
        if path.is_root() {
            self.inner.root.clone()
        } else {
            self.inner.root.join(path.as_str())
        }
    }

    fn validate_operation_path(&self, path: &WorkdirPath) -> Result<(), WorkdirError> {
        if !self.inner.reject_symlinks {
            return Ok(());
        }
        let path = Path::new(path.as_str());
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(WorkdirError::InvalidPath(
                "External Workdir paths must be root-relative".to_string(),
            ));
        }
        Ok(())
    }

    fn checkout_access(&self) -> ScopeAccess {
        let guard = self
            .operation_guard
            .clone()
            .unwrap_or_else(|| OperationGuard {
                cancelled: Arc::new(AtomicBool::new(false)),
                deadline: Instant::now() + Duration::from_secs(30),
            });
        ScopeAccess::new(
            self.inner.scope.snapshot(),
            &self.inner.root,
            self.inner.pinned_root.clone(),
            self.inner.reject_symlinks,
            Some(guard),
        )
    }

    fn checkout_root(&self) -> Result<Arc<std::fs::File>, WorkdirError> {
        match &self.inner.pinned_root {
            Some(root) => Ok(root.clone()),
            None => fs_operation::open_root_no_symlinks(&self.inner.root)
                .map(Arc::new)
                .map_err(|e| WorkdirError::io(Path::new("<checkout-root>"), e)),
        }
    }

    fn checkout_observation(
        &self,
        path: WorkdirPath,
        pinned: &fs_operation::CheckedTarget<'_>,
    ) -> Result<crate::CheckoutObservation, WorkdirError> {
        let metadata = pinned
            .metadata()
            .map_err(|e| WorkdirError::io(Path::new(path.as_str()), e))?;
        if fs_operation::identity_validator(&metadata).map_err(|e| checkout_io(&path, e))?
            != pinned.validator()
        {
            return Err(WorkdirError::Conflict(
                "checkout changed during observation".into(),
            ));
        }
        let access = self.scope_access();
        let absolute = self.resolve(&path);
        let capabilities = if manifest::Scope::is_writable(&access.scope, &absolute) {
            self.capabilities()
                .intersection(WorkdirSessionCapabilities::READ_WRITE)
        } else {
            self.capabilities()
                .intersection(WorkdirSessionCapabilities::READ_ONLY)
        };
        Ok(crate::CheckoutObservation {
            path,
            kind: if metadata.is_dir() {
                crate::EntryKind::Directory
            } else {
                crate::EntryKind::File
            },
            size: metadata.len(),
            validator: pinned.validator().to_vec(),
            capabilities,
        })
    }

    fn scope_access(&self) -> ScopeAccess {
        ScopeAccess::new(
            self.inner.scope.snapshot(),
            &self.inner.root,
            self.inner.pinned_root.clone(),
            self.inner.reject_symlinks,
            self.operation_guard.clone(),
        )
    }
}

#[async_trait]
impl WorkdirSession for LocalWorkdirSession {
    fn workdir(&self) -> &Workdir {
        &self.inner.workdir
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.inner.capabilities
    }

    fn environment(&self) -> crate::WorkdirSessionEnvironment {
        let scope = self.inner.scope.snapshot();
        let project = |rule: manifest::ScopeRule| -> Option<crate::WorkdirToolScopeRule> {
            let target = if self.inner.root.starts_with(&rule.target) && rule.recursive {
                WorkdirPath::root()
            } else {
                let path = rule
                    .target
                    .strip_prefix(&self.inner.root)
                    .unwrap_or(&rule.target);
                WorkdirPath::new_scoped(path.to_str()?).ok()?
            };
            Some(crate::WorkdirToolScopeRule {
                target,
                permission: match rule.permission {
                    Permission::Read => WorkdirToolScopePermission::Read,
                    Permission::Write => WorkdirToolScopePermission::Write,
                },
                recursive: rule.recursive,
                symlink_policy: rule.symlink_policy,
            })
        };
        crate::WorkdirSessionEnvironment {
            cwd: WorkdirPath::root(),
            scope: Some(
                scope
                    .allow_rules()
                    .into_iter()
                    .filter_map(project)
                    .collect(),
            ),
            capabilities: self.capabilities(),
            restrictions: scope
                .deny_rules()
                .into_iter()
                .map(|rule| {
                    project(rule)
                        .map(|rule| format!("Deny: {rule:?}"))
                        .unwrap_or_else(|| {
                            "Unrepresentable provider deny rule remains enforced".into()
                        })
                })
                .collect(),
        }
    }

    async fn authorize_scope_path(
        &self,
        request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        self.ensure_open()?;
        self.validate_operation_path(&request.path)?;
        let logical = self.inner.root.join(request.path.as_str());
        if self.inner.reject_symlinks && fs_operation::first_symlink(&logical).is_some() {
            return Err(WorkdirError::denied(
                crate::WorkdirDenialReason::ExternalScopeSymlink,
                "External Workdir scope cannot traverse symbolic links".to_string(),
            ));
        }
        let resolved = fs_operation::resolve_access_path(&logical)
            .map_err(|error| WorkdirError::io(&logical, error))?;
        let parent_permission = self
            .inner
            .scope
            .load()
            .permission_at_paths(&logical, &resolved);
        let parent_allows = match request.permission {
            WorkdirToolScopePermission::Read => matches!(
                parent_permission,
                Some(Permission::Read | Permission::Write)
            ),
            WorkdirToolScopePermission::Write => parent_permission == Some(Permission::Write),
        };
        if !parent_allows {
            return Err(WorkdirError::denied(
                crate::WorkdirDenialReason::AttachmentScopeExceeded,
                format!(
                    "Workdir path `{}` exceeds the provider attachment scope",
                    request.path
                ),
            ));
        }
        let allowed = request.rules.iter().any(|rule| {
            if request.permission == WorkdirToolScopePermission::Write
                && rule.permission != WorkdirToolScopePermission::Write
            {
                return false;
            }
            let logical_target = self.inner.root.join(rule.target.as_str());
            let (candidate, target) = match rule.symlink_policy {
                SymlinkPolicy::Logical => (logical.as_path(), logical_target),
                SymlinkPolicy::Resolved => {
                    let Ok(target) = fs_operation::resolve_access_path(&logical_target) else {
                        return false;
                    };
                    (resolved.as_path(), target)
                }
            };
            if rule.recursive {
                candidate.starts_with(target)
            } else {
                candidate == target || candidate.parent() == Some(target.as_path())
            }
        });
        if allowed {
            Ok(())
        } else {
            Err(WorkdirError::denied(
                crate::WorkdirDenialReason::DelegatedScopeExceeded,
                format!(
                    "Workdir path `{}` is outside the provider-resolved delegated scope",
                    request.path
                ),
            ))
        }
    }

    async fn scope_rules_overlap(
        &self,
        request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        self.ensure_open()?;
        let (left_logical, left_resolved) = rule_targets(&self.inner.root, &request.left)
            .map_err(|error| WorkdirError::io(&self.inner.root, error))?;
        let (right_logical, right_resolved) = rule_targets(&self.inner.root, &request.right)
            .map_err(|error| WorkdirError::io(&self.inner.root, error))?;
        Ok(path_sets_overlap(
            &left_logical,
            request.left.recursive,
            &right_logical,
            request.right.recursive,
        ) || path_sets_overlap(
            &left_resolved,
            request.left.recursive,
            &right_resolved,
            request.right.recursive,
        ))
    }

    async fn checkout_search(
        &self,
        mut request: crate::CheckoutSearchRequest,
    ) -> Result<crate::CheckoutSearchResult, WorkdirError> {
        use crate::{CheckoutSearchOperation as Op, CheckoutSearchResult as Output};
        self.ensure_capability(request.operation.capability())?;
        request.validate()?;
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("checkout provider busy".into()))?;
        let provider_root = self.checkout_root()?;
        let mut inner = self.checkout_access();
        inner.pinned_root = Some(provider_root);
        inner.reject_symlinks = true;
        let output_root = self.resolve(&request.output_root);
        let logical = request.operation.path().clone();
        let relative = Path::new(logical.as_str())
            .strip_prefix(request.output_root.as_str())
            .map_err(|_| {
                WorkdirError::denied(
                    crate::WorkdirDenialReason::CheckoutOutputRootExceeded,
                    "checkout search escaped output root",
                )
            })?;
        *request.operation.path_mut() = WorkdirPath::new(relative.to_str().ok_or_else(|| {
            WorkdirError::InvalidPath("checkout search path is not UTF8".into())
        })?)?;
        let source_allows = Arc::new(inner.scope.allow_rules());
        let access = SearchAccess {
            inner,
            layers: request.scope_layers,
            output_root: output_root.clone(),
            source_allows,
        };
        let result = match request.operation {
            Op::List(r) => fs_operation::run_list(&output_root, r, &access).map(Output::List),
            Op::Glob(r) => {
                fs_operation::run_glob(&output_root, &self.resolve(&logical), r, &access)
                    .map(Output::Glob)
            }
            Op::Grep(r) => fs_operation::run_grep(&output_root, self.resolve(&logical), r, &access)
                .map(Output::Grep),
        };
        result
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn checkout_observe(
        &self,
        path: WorkdirPath,
    ) -> Result<crate::CheckoutObservation, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        crate::checkout::validate_path(&path)?;
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("checkout provider busy".into()))?;
        let root = self.checkout_root().map_err(|error| match error {
            WorkdirError::Io { source, .. } => checkout_observe_io(&path, source),
            error => error,
        })?;
        let access = self.checkout_access();
        let pinned = fs_operation::CheckedTarget::pin(
            &root,
            Path::new(path.as_str()),
            &self.resolve(&path),
            &access,
            false,
        )
        .map_err(|e| checkout_observe_io(&path, e))?;
        self.checkout_observation(path.clone(), &pinned)
            .map_err(|error| match error {
                WorkdirError::Io { source, .. } => checkout_observe_io(&path, source),
                error => error,
            })
    }

    async fn checkout_execute(
        &self,
        request: crate::CheckoutRequest,
    ) -> Result<crate::CheckoutResult, WorkdirError> {
        use crate::{CheckoutOperation as Op, CheckoutOutput as Output};
        self.ensure_capability(request.operation.capability())?;
        crate::checkout::validate_path(&request.target)?;
        if request.validator.is_empty() || request.validator.len() > 256 {
            return Err(WorkdirError::InvalidArgument(
                "invalid checkout validator".into(),
            ));
        }
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("checkout provider busy".into()))?;
        let path = request.target;
        let root = self
            .checkout_root()
            .map_err(|error| checkout_execute_error(&path, error))?;
        let access = self.checkout_access();
        let mut pinned = fs_operation::CheckedTarget::pin(
            &root,
            Path::new(path.as_str()),
            &self.resolve(&path),
            &access,
            request.operation.capability() != WorkdirSessionCapability::Read,
        )
        .map_err(|e| checkout_io(&path, e))?;
        if pinned.validator() != request.validator {
            return Err(WorkdirError::Conflict(
                "checkout observation changed; observe and read again".into(),
            ));
        }
        let is_create = matches!(&request.operation, Op::Create { .. });
        let output = match request.operation {
            Op::ReadBytes { offset, max_bytes } => fs_operation::run_read_bytes_bounded(
                &self.inner.root,
                ReadBytesRequest {
                    path: path.clone(),
                    offset,
                    max_bytes,
                    expected_hash: None,
                },
                &pinned,
                self.inner
                    .read_limits
                    .unwrap_or(BoundedReadLimits::EXTERNAL_DEFAULT),
            )
            .map(Output::ReadBytes),
            Op::Read {
                offset,
                limit,
                max_bytes,
            } => {
                if limit > 1_000_000
                    || max_bytes > BoundedReadLimits::EXTERNAL_DEFAULT.max_response_bytes
                {
                    return Err(WorkdirError::InvalidArgument(
                        "checkout Read exceeds provider bounds".into(),
                    ));
                }
                fs_operation::run_read_bounded(
                    &self.inner.root,
                    ReadRequest {
                        path: path.clone(),
                        offset,
                        limit,
                        max_bytes,
                    },
                    &pinned,
                    self.inner
                        .read_limits
                        .unwrap_or(BoundedReadLimits::EXTERNAL_DEFAULT),
                )
                .map(Output::Read)
            }
            Op::Write {
                content,
                expected_hash,
            } => fs_operation::run_write(
                &self.inner.root,
                WriteRequest {
                    path: path.clone(),
                    content,
                    expected_hash: Some(expected_hash),
                },
                &pinned,
            )
            .map(Output::Write),
            Op::Edit {
                old_string,
                new_string,
                replace_all,
                expected_hash,
            } => {
                if old_string.len().saturating_add(new_string.len()) > 8 * 1024 * 1024 {
                    return Err(WorkdirError::InvalidArgument(
                        "checkout Edit exceeds provider bounds".into(),
                    ));
                }
                fs_operation::run_edit(
                    &self.inner.root,
                    EditRequest {
                        path: path.clone(),
                        old_string,
                        new_string,
                        replace_all,
                        expected_hash,
                    },
                    &pinned,
                )
                .map(Output::Edit)
            }
            Op::Create {
                path: destination,
                content,
            } => {
                let suffix = crate::checkout::create_relative_path(&path, &destination)?;
                let create = pinned.create_access(suffix, self.resolve(&destination));
                fs_operation::run_write(
                    &self.inner.root,
                    WriteRequest {
                        path: destination,
                        content,
                        expected_hash: None,
                    },
                    &create,
                )
                .map(Output::Write)
            }
        };
        let output = output.map_err(|error| {
            if pinned.effects_possible() {
                WorkdirError::OutcomeUnknown(
                    "checkout save or parent creation may have effects; inspect before retry"
                        .into(),
                )
            } else {
                checkout_execute_error(&path, WorkdirError::from(error))
            }
        })?;
        if matches!(&output, Output::Read(_) | Output::ReadBytes(_)) {
            pinned.confirm().map_err(|e| checkout_io(&path, e))?;
            return Ok(crate::CheckoutResult {
                observation: self
                    .checkout_observation(path.clone(), &pinned)
                    .map_err(|error| checkout_execute_error(&path, error))?,
                output,
            });
        }
        if is_create {
            pinned.refresh_directory_observation().map_err(|_| {
                WorkdirError::OutcomeUnknown(
                    "checkout Create may have effects but parent observation failed".into(),
                )
            })?;
            let observation = self.checkout_observation(path, &pinned).map_err(|_| {
                WorkdirError::OutcomeUnknown(
                    "checkout Create may have effects but parent observation failed".into(),
                )
            })?;
            return Ok(crate::CheckoutResult {
                observation,
                output,
            });
        }
        let after = fs_operation::CheckedTarget::pin(
            &root,
            Path::new(path.as_str()),
            &self.resolve(&path),
            &access,
            true,
        )
        .map_err(|_| {
            WorkdirError::OutcomeUnknown("checkout saved but post-save observation failed".into())
        })?;
        let observation = self.checkout_observation(path, &after).map_err(|_| {
            WorkdirError::OutcomeUnknown("checkout saved but post-save observation failed".into())
        })?;
        Ok(crate::CheckoutResult {
            observation,
            output,
        })
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_stat(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("filesystem provider busy".into()))?;
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        match self.inner.read_limits {
            Some(limits) => {
                fs_operation::run_read_bounded(&self.inner.root, request, &access, limits)
            }
            None => fs_operation::run_read(&self.inner.root, request, &access),
        }
        .map_err(WorkdirError::from)
        .map_err(|error| sanitize_error(error, &logical))
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<ReadBytesResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        match self.inner.read_limits {
            Some(limits) => {
                fs_operation::run_read_bytes_bounded(&self.inner.root, request, &access, limits)
            }
            None => fs_operation::run_read_bytes(&self.inner.root, request, &access),
        }
        .map_err(WorkdirError::from)
        .map_err(|error| sanitize_error(error, &logical))
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("filesystem provider busy".into()))?;
        self.ensure_capability(WorkdirSessionCapability::Write)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_write(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        let _serial = self
            .inner
            .checkout_lock
            .try_lock()
            .map_err(|_| WorkdirError::Unavailable("filesystem provider busy".into()))?;
        self.ensure_capability(WorkdirSessionCapability::Edit)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_edit(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_list(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Glob)?;
        let logical = request.path.clone();
        let base = self.resolve(&request.path);
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_glob(&self.inner.root, &base, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Grep)?;
        let base = self.resolve(&request.path);
        let logical = request.path.clone();
        self.validate_operation_path(&request.path)?;
        let access = self.scope_access();
        fs_operation::run_grep(&self.inner.root, base, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        request.validate()?;
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        // Command admission, resource capture and registration must be atomic
        // with terminal close, which drains commands and releases resources.
        let _admission = self.inner.close_lock.lock().await;
        self.ensure_open()?;
        let cwd = self.resolve(&request.cwd);
        let scope = self.inner.scope.snapshot();
        if !scope.is_readable(&cwd)
            || !std::fs::metadata(&cwd).is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(WorkdirError::OutOfScope(cwd));
        }
        let id = self.inner.next_command_id.fetch_add(1, Ordering::Relaxed);
        let handle = CommandHandle(format!("command-{id}"));
        let mut commands = self.inner.commands.lock().await;
        // Retain bounded retry tombstones, evicting only fully delivered output.
        if commands.len() >= command_transport::MAX_COMMANDS {
            let mut evict = None;
            for (id, command) in commands.iter() {
                if command.task.is_finished()
                    && command
                        .transport
                        .try_lock()
                        .is_ok_and(|state| state.terminal_delivered())
                {
                    evict = Some(id.clone());
                    break;
                }
            }
            if let Some(id) = evict {
                commands.remove(&id);
                self.inner.command_telemetry.remove(&id);
            } else {
                return Err(WorkdirError::Unavailable(
                    "command transport capacity reached; drain existing commands".into(),
                ));
            }
        }
        let resources = self
            .inner
            .resources
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let command = command_transport::start(
            cwd,
            request,
            handle.0.clone(),
            self.inner.command_telemetry.clone(),
            self.inner.command_environment.clone(),
            resources,
        );
        commands.insert(handle.0.clone(), command);
        Ok(handle)
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        let commands = self.inner.commands.lock().await;
        let command = commands
            .get(&handle.0)
            .ok_or_else(|| WorkdirError::UnknownCommand(handle.0.clone()))?;
        Ok(command
            .status
            .borrow()
            .as_ref()
            .map_or(CommandStatus::Running, |terminal| terminal.status))
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        self.ensure_open()?;
        let transport = self
            .inner
            .commands
            .lock()
            .await
            .get(&request.handle.0)
            .ok_or_else(|| WorkdirError::UnknownCommand(request.handle.0.clone()))?
            .transport
            .clone();
        let result = transport.lock().await.read(&request).await;
        self.ensure_open()?;
        if result.as_ref().is_ok_and(|page| page.terminal.is_some()) {
            self.inner.command_telemetry.remove(&request.handle.0);
        }
        result
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        let cancel = {
            let commands = self.inner.commands.lock().await;
            let command = commands
                .get(&handle.0)
                .ok_or_else(|| WorkdirError::UnknownCommand(handle.0.clone()))?;
            Some(command.cancel.clone())
        };
        if let Some(cancel) = cancel {
            let _ = cancel.send(true);
        }
        Ok(())
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        self.inner
            .capabilities
            .supports(WorkdirSessionCapability::Command)
            .then(|| self.inner.command_telemetry.subscribe())
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        if self
            .inner
            .capabilities
            .supports(WorkdirSessionCapability::Command)
        {
            self.inner.command_telemetry.snapshot()
        } else {
            Vec::new()
        }
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        let _close_guard = self.inner.close_lock.lock().await;
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let commands = {
            let mut commands = self.inner.commands.lock().await;
            commands
                .drain()
                .map(|(_, command)| command)
                .collect::<Vec<_>>()
        };
        for command in commands {
            let _ = command.cancel.send(true);
            command.transport.lock().await.close();
            let _ = command.task.await;
        }
        if let Ok(mut resources) = self.inner.resources.lock() {
            resources.clear();
        }
        Ok(())
    }
}

// Discovery of an absent route is not a stale-validator execution attempt.
fn checkout_observe_io(path: &WorkdirPath, error: std::io::Error) -> WorkdirError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    ) {
        WorkdirError::NotFound(PathBuf::from(path.as_str()))
    } else {
        checkout_io(path, error)
    }
}

// All pre-effect execution stages preserve the same stale-name classification,
// including FsError::NotFound produced by the shared engine after pinning.
fn checkout_execute_error(path: &WorkdirPath, error: WorkdirError) -> WorkdirError {
    match error {
        WorkdirError::NotFound(_) => {
            WorkdirError::Conflict("checkout observation changed; observe and read again".into())
        }
        WorkdirError::Io { source, .. } => checkout_io(path, source),
        error => sanitize_error(error, path),
    }
}

fn checkout_io(path: &WorkdirPath, error: std::io::Error) -> WorkdirError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::NotFound
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::AlreadyExists
    ) {
        WorkdirError::Conflict("checkout observation changed; observe and read again".into())
    } else {
        sanitize_error(WorkdirError::io(Path::new(path.as_str()), error), path)
    }
}

fn sanitize_error(error: WorkdirError, logical: &WorkdirPath) -> WorkdirError {
    let path = PathBuf::from(logical.as_str());
    match error {
        WorkdirError::RelativePath(_) => WorkdirError::InvalidPath(logical.to_string()),
        WorkdirError::OutOfScope(_) => WorkdirError::OutOfScope(path),
        WorkdirError::SymlinkOutOfScope {
            required_permission,
            ..
        } => WorkdirError::SymlinkOutOfScope {
            path,
            target: PathBuf::from("<provider-internal target>"),
            required_permission,
        },
        WorkdirError::BrokenSymlink { .. } => WorkdirError::BrokenSymlink {
            path: path.clone(),
            link: path,
            target: PathBuf::from("<provider-internal target>"),
        },
        WorkdirError::SymlinkTargetIsDirectory { .. } => WorkdirError::SymlinkTargetIsDirectory {
            path,
            target: PathBuf::from("<provider-internal target>"),
        },
        WorkdirError::SymlinkDirectoryNotTraversed { tool, .. } => {
            WorkdirError::SymlinkDirectoryNotTraversed {
                tool,
                path,
                target: PathBuf::from("<provider-internal target>"),
            }
        }
        WorkdirError::ReadOnly(_) => WorkdirError::ReadOnly(path),
        WorkdirError::IsDirectory(_) => WorkdirError::IsDirectory(path),
        WorkdirError::NotFound(_) => WorkdirError::NotFound(path),
        WorkdirError::Io { source, .. }
            if source.kind() == std::io::ErrorKind::PermissionDenied =>
        {
            WorkdirError::DenialContext {
                reason: io_denial_reason(&source),
                source: Box::new(WorkdirError::OutOfScope(path)),
            }
        }
        WorkdirError::Io { source, .. } => WorkdirError::Unavailable(format!(
            "I/O operation failed for {logical}: {}",
            source.kind()
        )),
        other => other,
    }
}

#[derive(Debug)]
struct CommandProcessGroupGuard {
    process_group_id: Option<u32>,
}

impl CommandProcessGroupGuard {
    fn new(process_group_id: Option<u32>) -> Self {
        Self { process_group_id }
    }

    fn terminate(&mut self) {
        let Some(process_group_id) = self.process_group_id.take() else {
            return;
        };
        terminate_process_group(process_group_id);
    }
}

impl Drop for CommandProcessGroupGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(unix)]
fn terminate_process_group(process_group_id: u32) {
    let Ok(process_group_id) = i32::try_from(process_group_id) else {
        return;
    };
    // The command is spawned as its own process-group leader below. A negative
    // pid addresses that exact group, including descendants that outlive bash.
    unsafe {
        libc::kill(-process_group_id, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_process_group_id: u32) {}

#[derive(Debug, Default)]
struct CommandOutputDecoder {
    read_offset: u64,
    emitted_offset: u64,
    pending: Vec<u8>,
}

fn publish_decoded_output(
    decoder: &mut CommandOutputDecoder,
    telemetry: &CommandTelemetry,
    command_id: &str,
    stream: CommandStream,
    flush: bool,
) {
    let prefix_len = if flush {
        decoder.pending.len()
    } else {
        stable_utf8_prefix_len(&decoder.pending)
    };
    if prefix_len == 0 {
        return;
    }
    telemetry.output(
        command_id,
        stream,
        decoder.emitted_offset,
        &decoder.pending[..prefix_len],
    );
    decoder.emitted_offset = decoder.emitted_offset.saturating_add(prefix_len as u64);
    decoder.pending.drain(..prefix_len);
}

/// Return the byte prefix that can be decoded now without replacing a valid
/// UTF-8 scalar whose remaining bytes may arrive in a later pipe read. Definite
/// invalid sequences remain in the prefix and are rendered lossily, preserving
/// the existing arbitrary-byte output behavior.
fn stable_utf8_prefix_len(bytes: &[u8]) -> usize {
    let mut inspected = 0;
    while inspected < bytes.len() {
        match std::str::from_utf8(&bytes[inspected..]) {
            Ok(_) => return bytes.len(),
            Err(error) => {
                inspected += error.valid_up_to();
                match error.error_len() {
                    Some(invalid_len) => inspected += invalid_len,
                    None => return inspected,
                }
            }
        }
    }
    inspected
}

/// Return the first symlink component in `path`, if one exists.
///
/// The function only inspects existing path components. It intentionally uses
/// `symlink_metadata` so the symlink itself can be diagnosed before any later
/// `metadata` call follows it and collapses the reason into `NotFound` or
/// `OutOfScope`.
pub fn first_symlink(path: &Path) -> Option<SymlinkInfo> {
    if !path.is_absolute() {
        return None;
    }

    let mut cur = PathBuf::new();
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        cur.push(component.as_os_str());
        let meta = std::fs::symlink_metadata(&cur).ok()?;
        if !meta.file_type().is_symlink() {
            continue;
        }

        let raw_target = std::fs::read_link(&cur).ok()?;
        let target_path = if raw_target.is_absolute() {
            raw_target
        } else {
            cur.parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(raw_target)
        };
        let target_exists = target_path.exists();
        let mut resolved_path = target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.clone());
        for remaining in components {
            resolved_path.push(remaining.as_os_str());
        }

        return Some(SymlinkInfo {
            link_path: cur,
            target_path,
            resolved_path,
            target_exists,
        });
    }

    None
}

pub fn direct_symlink(path: &Path) -> Option<SymlinkInfo> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        first_symlink(path)
    } else {
        None
    }
}

#[cfg(test)]
fn symlink_out_of_scope_or_plain(
    path: &Path,
    symlink: Option<&SymlinkInfo>,
    required_permission: &'static str,
    scope: &Scope,
) -> WorkdirError {
    if let Some(info) = symlink {
        let link_parent_readable = info
            .link_path
            .parent()
            .map(|parent| scope.is_readable(parent))
            .unwrap_or(false);
        if info.target_exists && link_parent_readable {
            return WorkdirError::SymlinkOutOfScope {
                path: path.to_path_buf(),
                target: info.resolved_path.clone(),
                required_permission,
            };
        }
    }
    WorkdirError::OutOfScope(path.to_path_buf())
}

#[cfg(test)]
fn broken_symlink_error(path: &Path, info: &SymlinkInfo) -> WorkdirError {
    WorkdirError::BrokenSymlink {
        path: path.to_path_buf(),
        link: info.link_path.clone(),
        target: info.target_path.clone(),
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::{Permission, ScopeConfig, ScopeRule};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn checkout_permission_mapping_distinguishes_provider_policy_from_os_without_reclassifying() {
        use crate::WorkdirDenialReason as Reason;
        use crate::http::{WorkdirTransportError, WorkdirTransportErrorCode as Code};
        use fs_operation::FsAccessPolicy;
        let root = tempfile::tempdir().unwrap();
        let access = ScopeAccess::new(
            Arc::new(Scope::writable(root.path()).unwrap()),
            root.path(),
            None,
            true,
            None,
        );
        // Exercise the actual policy boundary, not a string-based stand-in.
        let policy = access
            .resolve_access_path(Path::new("/outside/private-secret"))
            .unwrap_err();
        let os = std::io::Error::from_raw_os_error(libc::EACCES);
        for (source, reason) in [
            (policy, Reason::ProviderRootExceeded),
            (os, Reason::OsPermissionDenied),
        ] {
            let error = sanitize_error(
                WorkdirError::io(Path::new("/host/private-secret"), source),
                &WorkdirPath::new("file").unwrap(),
            );
            assert_eq!(error.denial_reason(), Some(reason));
            assert_eq!(error.to_string(), "path is outside allowed scope: file");
            let transport = WorkdirTransportError::from_workdir_error(&error);
            assert_eq!(transport.code, Code::OutOfScope);
            assert_eq!(transport.message, "Workdir path is out of scope");
            assert_eq!(transport.denial_reason, Some(reason));
            assert_eq!(
                WorkdirTransportError::from_workdir_error(&transport.clone().into_workdir_error()),
                transport
            );
            assert!(
                !serde_json::to_string(&transport)
                    .unwrap()
                    .contains("private-secret")
            );
        }
        // Ordinary OS I/O remains the existing generic denied/403 classification.
        let error = WorkdirError::io(
            Path::new("/host/private-secret"),
            std::io::Error::from_raw_os_error(libc::EACCES),
        );
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(transport.code, Code::Denied);
        assert_eq!(transport.denial_reason, Some(Reason::OsPermissionDenied));
    }

    #[test]
    fn checkout_error_mapping_distinguishes_absence_from_stale_execution_at_all_boundaries() {
        let path = WorkdirPath::new("missing").unwrap();
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::NotADirectory,
        ] {
            assert!(
                matches!(checkout_observe_io(&path, std::io::Error::from(kind)), WorkdirError::NotFound(p) if p == Path::new("missing"))
            );
            assert!(matches!(
                checkout_io(&path, std::io::Error::from(kind)),
                WorkdirError::Conflict(_)
            ));
            assert!(matches!(
                checkout_execute_error(
                    &path,
                    WorkdirError::io(Path::new("<provider-root>"), std::io::Error::from(kind))
                ),
                WorkdirError::Conflict(_)
            ));
        }
        let shared_error =
            WorkdirError::from(fs_operation::FsError::NotFound(PathBuf::from("missing")));
        assert!(matches!(
            checkout_execute_error(&path, shared_error),
            WorkdirError::Conflict(_)
        ));
        assert!(matches!(
            checkout_observe_io(&path, std::io::Error::from(std::io::ErrorKind::WouldBlock)),
            WorkdirError::Conflict(_)
        ));
        assert!(matches!(
            checkout_execute_error(
                &path,
                WorkdirError::OutcomeUnknown("possible save effects".into())
            ),
            WorkdirError::OutcomeUnknown(_)
        ));
    }

    fn make_fs(dir: &TempDir) -> LocalWorkdirSession {
        LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        )
    }

    fn make_logical_fs(dir: &TempDir) -> LocalWorkdirSession {
        LocalWorkdirSession::new(
            Scope::from_config(&ScopeConfig {
                allow: vec![ScopeRule {
                    target: dir.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: SymlinkPolicy::Logical,
                }],
                deny: Vec::new(),
            })
            .unwrap(),
            dir.path().to_path_buf(),
        )
    }

    #[tokio::test]
    async fn logical_provider_operations_cover_read_write_edit_stat_and_list() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let path = WorkdirPath::new("notes/item.txt").unwrap();

        let written = WorkdirSession::write(
            &workdir,
            WriteRequest {
                path: path.clone(),
                content: b"alpha\nbeta\n".to_vec(),
                expected_hash: None,
            },
        )
        .await
        .unwrap();
        assert!(written.created);

        let read = WorkdirSession::read(
            &workdir,
            ReadRequest {
                path: path.clone(),
                offset: 0,
                limit: 20,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        assert_eq!(read.bytes, b"alpha\nbeta\n");
        assert!(!read.truncated);

        let bounded = WorkdirSession::read(
            &workdir,
            ReadRequest {
                path: path.clone(),
                offset: 0,
                limit: 20,
                max_bytes: 6,
            },
        )
        .await
        .unwrap();
        assert_eq!(bounded.bytes, b"alpha\n");
        assert!(bounded.truncated);
        assert_eq!(bounded.content_hash, read.content_hash);

        let edited = WorkdirSession::edit(
            &workdir,
            EditRequest {
                path: path.clone(),
                old_string: "beta".to_owned(),
                new_string: "gamma".to_owned(),
                replace_all: false,
                expected_hash: read.content_hash,
            },
        )
        .await
        .unwrap();
        assert_eq!(edited.replacements, 1);

        let stat = WorkdirSession::stat(&workdir, StatRequest { path: path.clone() })
            .await
            .unwrap();
        assert_eq!(stat.path, path);
        assert_eq!(stat.kind, EntryKind::File);

        let listed = WorkdirSession::list(
            &workdir,
            ListRequest {
                after: None,
                path: WorkdirPath::new("notes").unwrap(),
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(listed.total_entries, 1);
        assert_eq!(listed.entries[0].path.as_str(), "notes/item.txt");

        let error = WorkdirSession::edit(
            &workdir,
            EditRequest {
                path: path.clone(),
                old_string: "gamma".to_owned(),
                new_string: "delta".to_owned(),
                replace_all: false,
                expected_hash: read.content_hash,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The target file's content or existence changed since it was last observed; read the file again before retrying: notes/item.txt"
        );

        std::fs::remove_file(dir.path().join("notes/item.txt")).unwrap();
        let error = WorkdirSession::write(
            &workdir,
            WriteRequest {
                path,
                content: b"replacement".to_vec(),
                expected_hash: Some(edited.content_hash),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The target file's content or existence changed since it was last observed; read the file again before retrying: notes/item.txt"
        );
    }

    #[tokio::test]
    async fn write_conflicts_when_observed_absence_becomes_a_file() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let path = WorkdirPath::new("race.txt").unwrap();

        let error = WorkdirSession::stat(&workdir, StatRequest { path: path.clone() })
            .await
            .unwrap_err();
        assert!(matches!(error, WorkdirError::NotFound(_)));

        std::fs::write(dir.path().join("race.txt"), "created externally").unwrap();
        let error = WorkdirSession::write(
            &workdir,
            WriteRequest {
                path,
                content: b"worker content".to_vec(),
                expected_hash: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The target file's content or existence changed since it was last observed; read the file again before retrying: race.txt"
        );
    }

    #[tokio::test]
    async fn close_is_terminal_for_one_session_without_deleting_workdir_identity() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("item.txt"), "persisted").unwrap();
        let workdir = Workdir::new("working-directory-42");
        let scope = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let session = LocalWorkdirSession::materialized_bound(
            workdir.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            scope.clone(),
            WorkdirSessionCapabilities::ALL,
        );
        assert_eq!(session.workdir(), &workdir);

        let command = WorkdirSession::start_command(
            &session,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "sleep 30".to_owned(),
                timeout_secs: 60,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let waiting_session = session.clone();
        let waiting_command = command.clone();
        let waiter = tokio::spawn(async move {
            crate::command_test_support::CollectCommand::collect_command_output(
                &waiting_session,
                CommandOutputRequest {
                    handle: waiting_command,
                    cursor: 0,
                    limit: 1024,
                    wait: true,
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        WorkdirSession::close(&session).await.unwrap();
        WorkdirSession::close(&session).await.unwrap();
        let waiter_error = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("close should wake command output waiters")
            .unwrap()
            .unwrap_err();
        assert!(matches!(
            waiter_error.classification_source(),
            WorkdirError::Unavailable(_)
        ));
        assert_eq!(
            waiter_error.failure_reason(),
            Some(crate::WorkdirFailureReason::SessionClosed)
        );

        assert!(matches!(
            WorkdirSession::command_status(&session, command).await,
            Err(ref error) if matches!(error.classification_source(), WorkdirError::Unavailable(_))
        ));
        assert!(matches!(
            WorkdirSession::read(
                &session,
                ReadRequest {
                    path: WorkdirPath::new("item.txt").unwrap(),
                    offset: 0,
                    limit: 10,
                    max_bytes: 1024,
                },
            )
            .await,
            Err(ref error) if matches!(error.classification_source(), WorkdirError::Unavailable(_))
        ));

        let restored = LocalWorkdirSession::materialized_bound(
            workdir.clone(),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            scope,
            WorkdirSessionCapabilities::ALL,
        );
        assert_eq!(restored.workdir(), &workdir);
        let read = WorkdirSession::read(
            &restored,
            ReadRequest {
                path: WorkdirPath::new("item.txt").unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        assert_eq!(read.bytes, b"persisted");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolved_provider_scope_rejects_read_and_write_through_outside_alias() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, "secret").unwrap();
        symlink(&target, root.path().join("alias.txt")).unwrap();
        symlink(outside.path(), root.path().join("alias-dir")).unwrap();
        let workdir = make_fs(&root);

        assert!(matches!(
            WorkdirSession::read(
                &workdir,
                ReadRequest {
                    path: WorkdirPath::new("alias.txt").unwrap(),
                    offset: 0,
                    limit: 10,
                    max_bytes: 1024,
                }
            )
            .await,
            Err(WorkdirError::SymlinkOutOfScope { .. })
        ));
        assert!(matches!(
            WorkdirSession::write(
                &workdir,
                WriteRequest {
                    path: WorkdirPath::new("alias.txt").unwrap(),
                    content: b"changed".to_vec(),
                    expected_hash: None,
                }
            )
            .await,
            Err(WorkdirError::SymlinkOutOfScope { .. })
        ));
        assert_eq!(fs::read_to_string(target).unwrap(), "secret");
        assert!(matches!(
            WorkdirSession::write(
                &workdir,
                WriteRequest {
                    path: WorkdirPath::new("alias-dir/new.txt").unwrap(),
                    content: b"new".to_vec(),
                    expected_hash: None,
                }
            )
            .await,
            Err(WorkdirError::ReadOnly(_))
        ));
        assert!(!outside.path().join("new.txt").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resolved_deny_blocks_missing_write_through_logical_alias() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), root.path().join("alias")).unwrap();
        let workdir = LocalWorkdirSession::new(
            Scope::from_config(&ScopeConfig {
                allow: vec![ScopeRule {
                    target: root.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: SymlinkPolicy::Logical,
                }],
                deny: vec![ScopeRule {
                    target: outside.path().join("blocked.txt"),
                    permission: Permission::Read,
                    recursive: false,
                    symlink_policy: SymlinkPolicy::Logical,
                }],
            })
            .unwrap(),
            root.path().to_path_buf(),
        );

        assert!(matches!(
            WorkdirSession::write(
                &workdir,
                WriteRequest {
                    path: WorkdirPath::new("alias/blocked.txt").unwrap(),
                    content: b"blocked".to_vec(),
                    expected_hash: None,
                }
            )
            .await,
            Err(WorkdirError::ReadOnly(_))
        ));
        assert!(!outside.path().join("blocked.txt").exists());
    }

    #[tokio::test]
    async fn capability_boundary_rejects_direct_unsupported_operation() {
        let dir = TempDir::new().unwrap();
        let workdir = LocalWorkdirSession::materialized(
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            SharedScope::new(Scope::writable(dir.path()).unwrap()),
            WorkdirSessionCapabilities::READ_ONLY,
        );

        assert_eq!(workdir.root(), dir.path());
        assert_eq!(workdir.cwd(), dir.path());
        let error = WorkdirSession::write(
            &workdir,
            WriteRequest {
                path: WorkdirPath::new("blocked.txt").unwrap(),
                content: b"blocked".to_vec(),
                expected_hash: None,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(
            error,
            WorkdirError::Unsupported(WorkdirSessionCapability::Write)
        ));
        assert!(!dir.path().join("blocked.txt").exists());
    }

    // -------------------------------------------------------------------------
    // read_bytes
    // -------------------------------------------------------------------------

    #[test]
    fn read_bytes_returns_content() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("a.txt");
        fs::write(&file, b"abc").unwrap();
        assert_eq!(fs.read_bytes(&file).unwrap(), b"abc");
    }

    #[test]
    fn read_bytes_rejects_relative() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(Path::new("rel.txt")).unwrap_err();
        assert!(matches!(err, WorkdirError::RelativePath(_)));
    }

    #[test]
    fn read_bytes_rejects_directory() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(dir.path()).unwrap_err();
        assert!(matches!(err, WorkdirError::IsDirectory(_)));
    }

    #[test]
    fn read_bytes_rejects_missing() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.read_bytes(&dir.path().join("nope.txt")).unwrap_err();
        assert!(matches!(err, WorkdirError::NotFound(_)));
    }

    #[test]
    fn read_bytes_rejects_paths_outside_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("x.txt");
        fs::write(&outside_file, b"hi").unwrap();

        let scoped = make_fs(&dir);
        let err = scoped.read_bytes(&outside_file).unwrap_err();
        assert!(matches!(err, WorkdirError::OutOfScope(_)));
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_reports_broken_symlink_target() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let link = dir.path().join("external-project");
        let target = dir.path().join("missing-target");
        symlink(&target, &link).unwrap();

        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                WorkdirError::BrokenSymlink { ref path, link: ref err_link, target: ref err_target }
                    if path == &link && err_link == &link && err_target == &target
            ),
            "expected broken symlink diagnostic, got {err:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_allows_logical_symlink_path_with_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_logical_fs(&dir);
        assert_eq!(fs.read_bytes(&link).unwrap(), b"secret");
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_allows_symlink_file_when_target_is_inside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, b"visible").unwrap();
        let link = dir.path().join("link.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        assert_eq!(fs.read_bytes(&link).unwrap(), b"visible");
    }

    #[cfg(unix)]
    #[test]
    fn read_bytes_reports_symlink_to_directory_as_wrong_file_type() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let target_dir = dir.path().join("target-dir");
        fs::create_dir(&target_dir).unwrap();
        let link = dir.path().join("dir-link");
        symlink(&target_dir, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                WorkdirError::SymlinkTargetIsDirectory { ref path, ref target }
                    if path == &link && target == &target_dir.canonicalize().unwrap()
            ),
            "expected symlink directory type diagnostic, got {err:?}"
        );
    }

    // -------------------------------------------------------------------------
    // write
    // -------------------------------------------------------------------------

    #[test]
    fn write_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("new.txt");
        let out = fs.write(&file, b"hello").unwrap();
        assert!(out.created);
        assert_eq!(out.bytes_written, 5);
        assert_eq!(fs::read(&file).unwrap(), b"hello");
    }

    #[test]
    fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let file = dir.path().join("a.txt");
        fs::write(&file, b"old").unwrap();
        let out = fs.write(&file, b"new").unwrap();
        assert!(!out.created);
        assert_eq!(fs::read(&file).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn write_existing_symlink_file_updates_in_scope_target() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let target = dir.path().join("target.txt");
        fs::write(&target, b"old").unwrap();
        let link = dir.path().join("link.txt");
        symlink(&target, &link).unwrap();

        let out = fs.write(&link, b"new").unwrap();
        assert!(!out.created);
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_allows_logical_symlink_path_with_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_logical_fs(&dir);
        fs.write(&link, b"new").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn write_rejects_out_of_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(&outside.path().join("x"), b"x").unwrap_err();
        assert!(matches!(err, WorkdirError::OutOfScope(_)));
    }

    #[test]
    fn write_rejects_readonly_path() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
                symlink_policy: Default::default(),
            }],
            deny: vec![ScopeRule {
                target: sub.clone(),
                permission: Permission::Write,
                recursive: true,
                symlink_policy: Default::default(),
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let scoped = LocalWorkdirSession::new(scope, dir.path().to_path_buf());
        let err = scoped.write(&sub.join("locked.txt"), b"x").unwrap_err();
        assert!(
            matches!(err, WorkdirError::ReadOnly(_)),
            "expected ReadOnly, got {err:?}"
        );
    }

    #[test]
    fn write_rejects_relative_path() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(Path::new("rel.txt"), b"x").unwrap_err();
        assert!(matches!(err, WorkdirError::RelativePath(_)));
    }

    #[test]
    fn write_creates_missing_parents_inside_scope() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let nested = dir.path().join("a/b/c/deep.txt");
        fs.write(&nested, b"x").unwrap();
        assert_eq!(fs::read(&nested).unwrap(), b"x");
    }

    #[test]
    fn write_rejects_directory_target() {
        let dir = TempDir::new().unwrap();
        let fs = make_fs(&dir);
        let err = fs.write(dir.path(), b"x").unwrap_err();
        assert!(matches!(err, WorkdirError::IsDirectory(_)));
    }

    // -------------------------------------------------------------------------
    // Dynamic scope: SharedScope mutations propagate into LocalWorkdirSession decisions
    // -------------------------------------------------------------------------

    #[test]
    fn add_allow_rule_through_shared_scope_grows_readable_set() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let extra = TempDir::new().unwrap();
        let extra_file = extra.path().join("x.txt");
        fs::write(&extra_file, b"hi").unwrap();

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs = LocalWorkdirSession::with_shared_scope(shared.clone(), dir.path().to_path_buf());

        // Before: extra is out of scope.
        let err = fs.read_bytes(&extra_file).unwrap_err();
        assert!(matches!(err, WorkdirError::OutOfScope(_)));

        // Push an allow(Read) rule.
        shared
            .update(|cur| {
                cur.with_added_allow_rules([ScopeRule {
                    target: extra.path().to_path_buf(),
                    permission: Permission::Read,
                    recursive: true,
                    symlink_policy: Default::default(),
                }])
            })
            .unwrap();

        // After: read goes through.
        assert_eq!(fs.read_bytes(&extra_file).unwrap(), b"hi");
        // But write still fails — allow only granted Read.
        let err = fs.write(&extra.path().join("y.txt"), b"x").unwrap_err();
        assert!(
            matches!(err, WorkdirError::ReadOnly(_)),
            "expected ReadOnly, got {err:?}"
        );
    }

    #[test]
    fn revoke_write_through_shared_scope_blocks_subsequent_writes() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let target = sub.join("a.txt");

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs = LocalWorkdirSession::with_shared_scope(shared.clone(), dir.path().to_path_buf());

        // Write succeeds initially.
        fs.write(&target, b"first").unwrap();

        // Revoke Write on `sub` (push a deny(Write) rule).
        shared
            .update(|cur| {
                cur.with_added_deny_rules([ScopeRule {
                    target: sub.clone(),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: Default::default(),
                }])
            })
            .unwrap();

        // Subsequent write fails with ReadOnly — Read is preserved.
        let err = fs.write(&target, b"second").unwrap_err();
        assert!(
            matches!(err, WorkdirError::ReadOnly(_)),
            "expected ReadOnly after revoke, got {err:?}"
        );
        // Read still works.
        assert_eq!(fs.read_bytes(&target).unwrap(), b"first");
    }

    #[test]
    fn shared_scope_changes_propagate_across_clones() {
        use manifest::SharedScope;

        let dir = TempDir::new().unwrap();
        let target = dir.path().join("a.txt");

        let shared = SharedScope::new(Scope::writable(dir.path()).unwrap());
        let fs1 = LocalWorkdirSession::with_shared_scope(shared.clone(), dir.path().to_path_buf());
        let fs2 = fs1.clone();

        // fs1 writes; both clones see the file.
        fs1.write(&target, b"hi").unwrap();
        assert_eq!(fs2.read_bytes(&target).unwrap(), b"hi");

        // Revoke write through the original handle.
        shared
            .update(|cur| {
                cur.with_added_deny_rules([ScopeRule {
                    target: dir.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: Default::default(),
                }])
            })
            .unwrap();

        // Both clones reject writes now — they share the same SharedScope.
        assert!(matches!(
            fs1.write(&target, b"x").unwrap_err(),
            WorkdirError::ReadOnly(_)
        ));
        assert!(matches!(
            fs2.write(&target, b"x").unwrap_err(),
            WorkdirError::ReadOnly(_)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_uses_explicit_logical_policy_through_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("worker.json"), "scope-needle\n").unwrap();
        symlink(outside.path(), dir.path().join("yoi.local")).unwrap();
        let workdir = make_logical_fs(&dir);

        let read = WorkdirSession::read(
            &workdir,
            ReadRequest {
                path: WorkdirPath::new("yoi.local/worker.json").unwrap(),
                offset: 0,
                limit: 100,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        assert_eq!(read.bytes, b"scope-needle\n");
        let list = WorkdirSession::list(
            &workdir,
            ListRequest {
                after: None,
                path: WorkdirPath::new("yoi.local").unwrap(),
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            list.entries[0].path,
            WorkdirPath::new("yoi.local/worker.json").unwrap()
        );
        let glob = WorkdirSession::glob(
            &workdir,
            GlobRequest {
                pattern: "**/*.json".into(),
                path: WorkdirPath::new("yoi.local").unwrap(),
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            glob.paths,
            [WorkdirPath::new("yoi.local/worker.json").unwrap()]
        );
        let grep = WorkdirSession::grep(
            &workdir,
            GrepRequest {
                pattern: "scope-needle".into(),
                path: WorkdirPath::new("yoi.local").unwrap(),
                glob: Some("*.json".into()),
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: crate::GrepOutputMode::Content,
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(grep.match_count, 1);
        assert!(grep.output.contains("yoi.local/worker.json"));
        assert!(
            !workdir
                .scope()
                .is_readable(&outside.path().join("worker.json"))
        );
    }

    #[tokio::test]
    async fn provider_executes_glob_grep_and_command_at_the_materialization() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main() { /* NEEDLE */ }\n",
        )
        .unwrap();
        let workdir = make_fs(&dir);
        let glob = WorkdirSession::glob(
            &workdir,
            GlobRequest {
                pattern: "**/*.rs".into(),
                path: WorkdirPath::root(),
                limit: 10,
            },
        )
        .await
        .unwrap();
        assert_eq!(glob.paths, [WorkdirPath::new("src/main.rs").unwrap()]);
        let grep = WorkdirSession::grep(
            &workdir,
            GrepRequest {
                pattern: "NEEDLE".into(),
                path: WorkdirPath::new("src/main.rs").unwrap(),
                glob: Some("src/*.rs".into()),
                file_type: Some("rust".into()),
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: crate::GrepOutputMode::Content,
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(grep.match_count, 1);
        assert!(grep.output.contains("src/main.rs"));
        assert!(!grep.output.contains(dir.path().to_string_lossy().as_ref()));
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "pwd && printf provider-command".into(),
                timeout_secs: 5,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let output = crate::command_test_support::CollectCommand::collect_command_output(
            &workdir,
            CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 4096,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.exit_code, Some(0));
        assert!(output.content.contains("provider-command"));
        assert!(
            output
                .content
                .contains(dir.path().to_string_lossy().as_ref())
        );
    }

    #[tokio::test]
    async fn explicitly_scoped_absolute_artifact_can_be_read_and_grepped() {
        let dir = TempDir::new().unwrap();
        let spill = TempDir::new().unwrap();
        let artifact = spill.path().join("bash-output.log");
        std::fs::write(&artifact, "first\nFINAL-NEEDLE\nlast\n").unwrap();
        let scope = Scope::from_config(&ScopeConfig {
            allow: vec![
                ScopeRule {
                    target: dir.path().to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: Default::default(),
                },
                ScopeRule {
                    target: spill.path().to_path_buf(),
                    permission: Permission::Read,
                    recursive: true,
                    symlink_policy: Default::default(),
                },
            ],
            deny: Vec::new(),
        })
        .unwrap();
        let workdir = LocalWorkdirSession::new(scope, dir.path().to_path_buf());
        let artifact_path = WorkdirPath::new_scoped(artifact.to_string_lossy()).unwrap();

        let read = WorkdirSession::read(
            &workdir,
            ReadRequest {
                path: artifact_path.clone(),
                offset: 1,
                limit: 1,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap();
        assert_eq!(String::from_utf8(read.bytes).unwrap(), "FINAL-NEEDLE\n");

        let grep = WorkdirSession::grep(
            &workdir,
            GrepRequest {
                pattern: "FINAL-NEEDLE".into(),
                path: artifact_path,
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: crate::GrepOutputMode::Content,
                limit: 10,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(grep.match_count, 1);
        assert!(grep.output.contains("FINAL-NEEDLE"));
    }

    #[tokio::test]
    async fn command_rejects_explicit_cwd_outside_attachment_scope() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let cwd = WorkdirPath::new_scoped(outside.path().display().to_string()).unwrap();

        let error = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "printf hidden".into(),
                timeout_secs: 5,

                cwd,

                tool_call_id: None,
            },
        )
        .await
        .unwrap_err();

        assert!(matches!(error, WorkdirError::OutOfScope(path) if path == outside.path()));
    }

    #[tokio::test]
    async fn provider_streams_bounded_command_lifecycle_and_distinct_output() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let mut events = WorkdirSession::subscribe_command_events(&workdir)
            .expect("local command observation must be available");
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "printf ready; printf warning >&2; sleep 0.2; printf done".into(),
                timeout_secs: 5,

                cwd: WorkdirPath::root(),

                tool_call_id: Some("tool-7".into()),
            },
        )
        .await
        .unwrap();

        let mut stdout = String::new();
        let mut stdout_chunks = 0;
        let mut stderr = String::new();
        let mut terminal = None;
        while terminal.is_none() {
            let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("command telemetry should not stall")
                .unwrap();
            match event {
                CommandEvent::Started {
                    command_id,
                    tool_call_id,
                    ..
                } => {
                    assert_eq!(command_id, handle.0);
                    assert_eq!(tool_call_id.as_deref(), Some("tool-7"));
                }
                CommandEvent::Output {
                    command_id,
                    stream,
                    content,
                    ..
                } => {
                    assert_eq!(command_id, handle.0);
                    match stream {
                        CommandStream::Stdout => {
                            stdout_chunks += 1;
                            stdout.push_str(&content);
                        }
                        CommandStream::Stderr => stderr.push_str(&content),
                    }
                }
                CommandEvent::Terminal {
                    command_id,
                    status,
                    exit_code,
                    stdout_end_offset,
                    stderr_end_offset,
                    observed_at_ms,
                } => {
                    assert_eq!(command_id, handle.0);
                    terminal = Some((
                        status,
                        exit_code,
                        stdout_end_offset,
                        stderr_end_offset,
                        observed_at_ms,
                    ));
                }
            }
        }
        let (status, exit_code, stdout_end_offset, stderr_end_offset, observed_at_ms) =
            terminal.unwrap();
        assert_eq!(status, CommandStatus::Completed);
        assert_eq!(exit_code, Some(0));
        assert_eq!(stdout_end_offset, "readydone".len() as u64);
        assert_eq!(stderr_end_offset, "warning".len() as u64);
        assert!(observed_at_ms > 0);
        assert!(
            stdout_chunks >= 2,
            "long-running output should stream incrementally"
        );
        assert_eq!(stdout, "readydone");
        assert_eq!(stderr, "warning");
        let snapshot = WorkdirSession::command_snapshot(&workdir);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, CommandStatus::Completed);
        assert_eq!(snapshot[0].stdout.content, "readydone");
        assert_eq!(snapshot[0].stderr.content, "warning");

        let output = crate::command_test_support::CollectCommand::collect_command_output(
            &workdir,
            CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1024,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.status, CommandStatus::Completed);
        assert!(WorkdirSession::command_snapshot(&workdir).is_empty());
    }

    #[tokio::test]
    async fn provider_distinguishes_timed_out_terminal_state() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let mut events = WorkdirSession::subscribe_command_events(&workdir).unwrap();
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "sleep 30".into(),
                timeout_secs: 1,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let output = crate::command_test_support::CollectCommand::collect_command_output(
            &workdir,
            CommandOutputRequest {
                handle: handle.clone(),
                cursor: 0,
                limit: 1024,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.status, CommandStatus::TimedOut);
        assert!(output.timed_out);

        let mut terminal = None;
        while let Ok(event) = events.try_recv() {
            if let CommandEvent::Terminal {
                command_id,
                status,
                exit_code,
                ..
            } = event
            {
                terminal = Some((command_id, status, exit_code));
            }
        }
        assert_eq!(terminal, Some((handle.0, CommandStatus::TimedOut, None)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn command_admission_serializes_with_close_before_capturing_resources() {
        let dir = TempDir::new().unwrap();
        let session = Arc::new(make_fs(&dir));
        let held = session.inner.close_lock.lock().await;
        let (closing, close_entered) = tokio::sync::oneshot::channel();
        let close_session = session.clone();
        let close = tokio::spawn(async move {
            closing.send(()).unwrap();
            WorkdirSession::close(close_session.as_ref()).await
        });
        close_entered.await.unwrap();
        let (starting, start_entered) = tokio::sync::oneshot::channel();
        let start_session = session.clone();
        let start = tokio::spawn(async move {
            starting.send(()).unwrap();
            WorkdirSession::start_command(
                start_session.as_ref(),
                CommandRequest {
                    transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                    command: "true".into(),
                    timeout_secs: 5,

                    cwd: WorkdirPath::root(),

                    tool_call_id: None,
                },
            )
            .await
        });
        start_entered.await.unwrap();
        assert!(
            !start.is_finished(),
            "command started outside terminal-close admission"
        );
        drop(held);
        tokio::time::timeout(Duration::from_secs(5), close)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), start)
                .await
                .unwrap()
                .unwrap(),
            Err(ref error) if matches!(error.classification_source(), WorkdirError::Unavailable(message) if message.contains("is closed"))
        ));
        assert!(
            session.command_snapshot().is_empty(),
            "closed admission launched a command"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_drop_retains_resources_until_command_task_cancellation_completes() {
        #[derive(Debug)]
        struct Resource(Option<tokio::sync::oneshot::Sender<()>>);
        impl Drop for Resource {
            fn drop(&mut self) {
                let _ = self.0.take().unwrap().send(());
            }
        }
        let dir = TempDir::new().unwrap();
        let (released, mut release) = tokio::sync::oneshot::channel();
        let session = LocalWorkdirSession::materialized_bound_with_environment(
            Workdir::new("command-resource-session"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            SharedScope::new(Scope::writable(dir.path()).unwrap()),
            WorkdirSessionCapabilities::ALL,
            BTreeMap::new(),
            vec![Arc::new(Resource(Some(released)))],
        );
        WorkdirSession::start_command(
            &session,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "true".into(),
                timeout_secs: 5,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        // The sole executor has not polled the command task, so dropping the
        // session schedules its abort but cannot yet complete cancellation.
        drop(session);
        assert!(
            matches!(
                release.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "resources released before the aborted command future was dropped"
        );
        tokio::time::timeout(Duration::from_secs(5), release)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn closing_session_releases_runtime_resources() {
        #[derive(Debug)]
        struct Resource(Arc<AtomicBool>);
        impl Drop for Resource {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }

        let dir = TempDir::new().unwrap();
        let released = Arc::new(AtomicBool::new(false));
        let session = LocalWorkdirSession::materialized_bound_with_environment(
            Workdir::new("resource-session"),
            dir.path().to_path_buf(),
            dir.path().to_path_buf(),
            SharedScope::new(Scope::writable(dir.path()).unwrap()),
            WorkdirSessionCapabilities::ALL,
            BTreeMap::from([("SSH_AUTH_SOCK".to_string(), "test-socket".to_string())]),
            vec![Arc::new(Resource(released.clone()))],
        );
        WorkdirSession::close(&session).await.unwrap();
        assert!(released.load(Ordering::Acquire));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_kills_command_process_group_descendants() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "(sleep 0.4; echo leaked > child-marker) & echo ready > child-ready; wait"
                    .into(),
                timeout_secs: 60,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        for _ in 0..100 {
            if dir.path().join("child-ready").exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(dir.path().join("child-ready").exists());

        WorkdirSession::cancel_command(&workdir, handle.clone())
            .await
            .unwrap();
        let output = crate::command_test_support::CollectCommand::collect_command_output(
            &workdir,
            CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1024,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(output.status, CommandStatus::Cancelled);
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !dir.path().join("child-marker").exists(),
            "cancelled command descendant survived its process group"
        );
    }

    #[tokio::test]
    async fn provider_cancels_active_command() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                transport_byte_budget: crate::MAX_COMMAND_TRANSPORT_BYTES,
                command: "sleep 30".into(),
                timeout_secs: 60,

                cwd: WorkdirPath::root(),

                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            WorkdirSession::command_status(&workdir, handle.clone())
                .await
                .unwrap(),
            CommandStatus::Running
        );
        let waiting_workdir = workdir.clone();
        let waiting_handle = handle.clone();
        let waiter = tokio::spawn(async move {
            crate::command_test_support::CollectCommand::collect_command_output(
                &waiting_workdir,
                CommandOutputRequest {
                    handle: waiting_handle,
                    cursor: 0,
                    limit: 1024,
                    wait: true,
                },
            )
            .await
        });
        tokio::task::yield_now().await;
        WorkdirSession::cancel_command(&workdir, handle.clone())
            .await
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("cancel should wake command output waiters")
            .unwrap()
            .unwrap();
        assert_eq!(output.status, CommandStatus::Cancelled);
        assert_eq!(
            WorkdirSession::command_status(&workdir, handle)
                .await
                .unwrap(),
            CommandStatus::Cancelled
        );
    }
}
