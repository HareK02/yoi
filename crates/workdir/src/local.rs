//! Scope-aware filesystem primitive.
//!
//! `LocalWorkdirSession` is the write/read gate layered on top of a [`manifest::Scope`]
//! and a Worker's working directory. The scope decides which paths are
//! readable and writable; the cwd is carried alongside for convenience
//! (Glob/Grep default their search base to it).
//!
//! `LocalWorkdirSession` is cheap to clone (`Arc` inside). Tool-specific session
//! state, such as read-before-edit tracking, remains owned by the tool layer.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
#[cfg(test)]
use std::io::Write as _;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use manifest::{Permission, Scope, ScopeConfig, ScopeRule, SharedScope};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::sync::{Mutex, broadcast, watch};
use tokio::task::JoinHandle;

use crate::{
    CommandEvent, CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest,
    CommandSnapshot, CommandStatus, CommandStream, CommandStreamSlice, EditRequest, EditResult,
    GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest, ListResult, ReadRequest,
    ReadResult, StatRequest, StatResult, Workdir, WorkdirDelegationPermission,
    WorkdirDelegationRequest, WorkdirError, WorkdirPath, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionCapability, WorkdirSessionHandle, WriteRequest,
    WriteResult,
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

#[derive(Debug)]
enum LocalCommand {
    Running {
        task: JoinHandle<Result<CommandOutput, WorkdirError>>,
        completion: watch::Receiver<bool>,
        cancel: watch::Sender<bool>,
    },
    Completed(CommandOutput),
}

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

#[derive(Debug)]
struct ScopeAccess(Arc<Scope>);

impl fs_operation::FsAccessPolicy for ScopeAccess {
    fn is_readable(&self, path: &Path) -> bool {
        self.0.is_readable(path)
    }

    fn is_writable(&self, path: &Path) -> bool {
        self.0.is_writable(path)
    }
}

#[derive(Debug)]
struct LocalWorkdirSessionInner {
    workdir: Workdir,
    root: PathBuf,
    scope: SharedScope,
    cwd: PathBuf,
    capabilities: WorkdirSessionCapabilities,
    closed: AtomicBool,
    close_lock: Mutex<()>,
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
                if let LocalCommand::Running { task, .. } = command {
                    task.abort();
                }
            }
        }
    }
}

pub trait WorkdirSessionResource: Debug + Send + Sync {}
impl<T> WorkdirSessionResource for T where T: Debug + Send + Sync {}

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
        Self {
            inner: Arc::new(LocalWorkdirSessionInner {
                workdir,
                root,
                scope,
                cwd,
                capabilities,
                closed: AtomicBool::new(false),
                close_lock: Mutex::new(()),
                next_command_id: AtomicU64::new(1),
                commands: Mutex::new(HashMap::new()),
                command_telemetry: CommandTelemetry::new(),
                command_environment,
                resources: StdMutex::new(resources),
            }),
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
        let scope = self.inner.scope.load();
        if !scope.is_readable(path) {
            return Err(symlink_out_of_scope_or_plain(
                path,
                symlink.as_ref(),
                "read",
                &scope,
            ));
        }
        if let Some(info) = symlink.as_ref() {
            if !info.target_exists {
                return Err(broken_symlink_error(path, info));
            }
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
            )))
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
}

#[async_trait]
impl WorkdirSession for LocalWorkdirSession {
    fn workdir(&self) -> &Workdir {
        &self.inner.workdir
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.inner.capabilities
    }

    async fn capture_delegation_source(
        &self,
        request: &WorkdirDelegationRequest,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        let host_rules = request
            .rules
            .iter()
            .map(|rule| ScopeRule {
                target: self.inner.root.join(rule.target.as_str()),
                permission: match rule.permission {
                    WorkdirDelegationPermission::Read => Permission::Read,
                    WorkdirDelegationPermission::Write => Permission::Write,
                },
                recursive: rule.recursive,
            })
            .collect::<Vec<_>>();
        for (logical, host) in request.rules.iter().zip(&host_rules) {
            if logical.permission == WorkdirDelegationPermission::Write {
                let resolved = Scope::resolved_target(host)
                    .map_err(|error| WorkdirError::Denied(error.to_string()))?;
                if resolved != host.target {
                    return Err(WorkdirError::Denied(format!(
                        "write delegation target `{}` traverses a symlink",
                        logical.target
                    )));
                }
            }
        }
        let parent_scope = self.inner.scope.snapshot();
        for rule in &host_rules {
            if !parent_scope
                .allows_rule(rule)
                .map_err(|error| WorkdirError::Denied(error.to_string()))?
            {
                return Err(WorkdirError::Denied(format!(
                    "delegated provider scope `{}` exceeds the parent session",
                    rule.target.display()
                )));
            }
        }
        let child_scope = Scope::from_config(&ScopeConfig {
            allow: host_rules,
            deny: Vec::new(),
        })
        .map_err(|error| WorkdirError::Denied(error.to_string()))?;
        let child_cwd = self.inner.root.join(request.cwd.as_str());
        if !child_scope.is_readable(&child_cwd)
            || !std::fs::metadata(&child_cwd).is_ok_and(|metadata| metadata.is_dir())
        {
            return Err(WorkdirError::Denied(format!(
                "delegated cwd `{}` is not a readable Workdir directory",
                request.cwd
            )));
        }
        Ok(Arc::new(LocalWorkdirSession::materialized_bound(
            self.inner.workdir.clone(),
            self.inner.root.clone(),
            self.inner.root.clone(),
            SharedScope::new(child_scope),
            self.inner.capabilities,
        )))
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_stat(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_read(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Write)?;
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_write(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Edit)?;
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_edit(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Read)?;
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_list(&self.inner.root, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Glob)?;
        let logical = request.path.clone();
        let base = self.resolve(&request.path);
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_glob(&self.inner.root, &base, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Grep)?;
        let base = self.resolve(&request.path);
        let logical = request.path.clone();
        let access = ScopeAccess(self.inner.scope.snapshot());
        fs_operation::run_grep(&self.inner.root, base, request, &access)
            .map_err(WorkdirError::from)
            .map_err(|error| sanitize_error(error, &logical))
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        self.ensure_open()?;
        let id = self.inner.next_command_id.fetch_add(1, Ordering::Relaxed);
        let handle = CommandHandle(format!("command-{id}"));
        let cwd = self.inner.cwd.clone();
        let (completion_tx, completion) = watch::channel(false);
        let command_id = handle.0.clone();
        let telemetry = self.inner.command_telemetry.clone();
        let command_environment = self.inner.command_environment.clone();
        let (cancel, cancel_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let output = run_command(
                cwd,
                request,
                command_id,
                telemetry,
                command_environment,
                cancel_rx,
            )
            .await;
            let _ = completion_tx.send(true);
            output
        });
        let mut commands = self.inner.commands.lock().await;
        if let Err(error) = self.ensure_open() {
            let _ = cancel.send(true);
            task.abort();
            return Err(error);
        }
        commands.insert(
            handle.0.clone(),
            LocalCommand::Running {
                task,
                completion,
                cancel,
            },
        );
        Ok(handle)
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        let commands = self.inner.commands.lock().await;
        let command = commands
            .get(&handle.0)
            .ok_or_else(|| WorkdirError::UnknownCommand(handle.0.clone()))?;
        Ok(match command {
            LocalCommand::Running { task, .. } if !task.is_finished() => CommandStatus::Running,
            LocalCommand::Running { .. } => self
                .inner
                .command_telemetry
                .snapshot()
                .into_iter()
                .find(|snapshot| snapshot.command_id == handle.0)
                .map_or(CommandStatus::Completed, |snapshot| snapshot.status),
            LocalCommand::Completed(output) => output.status,
        })
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        let command = loop {
            self.ensure_open()?;
            let mut commands = self.inner.commands.lock().await;
            let Some(command) = commands.get(&request.handle.0) else {
                return Err(WorkdirError::UnknownCommand(request.handle.0.clone()));
            };
            let completion = match command {
                LocalCommand::Running { completion, .. }
                    if !*completion.borrow() && completion.has_changed().is_ok() =>
                {
                    Some(completion.clone())
                }
                _ => None,
            };
            if let Some(mut completion) = completion {
                if !request.wait {
                    return Ok(CommandOutput {
                        status: CommandStatus::Running,
                        exit_code: None,
                        timed_out: false,
                        content: String::new(),
                        next_cursor: None,
                        truncated: false,
                    });
                }
                drop(commands);
                let _ = completion.changed().await;
                continue;
            }
            if !request.wait
                && matches!(command, LocalCommand::Running { task, .. } if !task.is_finished())
            {
                return Ok(CommandOutput {
                    status: CommandStatus::Running,
                    exit_code: None,
                    timed_out: false,
                    content: String::new(),
                    next_cursor: None,
                    truncated: false,
                });
            }
            break commands
                .remove(&request.handle.0)
                .expect("command checked above");
        };

        let output = match command {
            LocalCommand::Running { task, .. } => task
                .await
                .map_err(|error| WorkdirError::Unavailable(error.to_string()))??,
            LocalCommand::Completed(output) => output,
        };
        let page = command_output_page(&output, request.cursor, request.limit);
        if page.next_cursor.is_some() {
            let mut commands = self.inner.commands.lock().await;
            if !self.inner.closed.load(Ordering::Acquire) {
                commands.insert(request.handle.0, LocalCommand::Completed(output));
            }
        } else {
            self.inner.command_telemetry.remove(&request.handle.0);
        }
        Ok(page)
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command)?;
        let cancel = {
            let commands = self.inner.commands.lock().await;
            let command = commands
                .get(&handle.0)
                .ok_or_else(|| WorkdirError::UnknownCommand(handle.0.clone()))?;
            match command {
                LocalCommand::Running { task, cancel, .. } if !task.is_finished() => {
                    Some(cancel.clone())
                }
                _ => None,
            }
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
            match command {
                LocalCommand::Running { task, cancel, .. } => {
                    let _ = cancel.send(true);
                    let _ = task.await;
                }
                LocalCommand::Completed(_) => {}
            }
        }
        if let Ok(mut resources) = self.inner.resources.lock() {
            resources.clear();
        }
        Ok(())
    }
}

fn command_output_page(output: &CommandOutput, cursor: usize, limit: usize) -> CommandOutput {
    let total_chars = output.content.chars().count();
    let start = cursor.min(total_chars);
    let content = output
        .content
        .chars()
        .skip(start)
        .take(limit.max(1))
        .collect::<String>();
    let end = start + content.chars().count();
    CommandOutput {
        status: output.status,
        exit_code: output.exit_code,
        timed_out: output.timed_out,
        content,
        next_cursor: (end < total_chars).then_some(end),
        truncated: output.truncated || end < total_chars,
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
        WorkdirError::Io { source, .. } => WorkdirError::Unavailable(format!(
            "I/O operation failed for {logical}: {}",
            source.kind()
        )),
        other => other,
    }
}

async fn run_command(
    cwd: PathBuf,
    request: CommandRequest,
    command_id: String,
    telemetry: CommandTelemetry,
    command_environment: BTreeMap<String, String>,
    mut cancel: watch::Receiver<bool>,
) -> Result<CommandOutput, WorkdirError> {
    let stdout = tempfile::NamedTempFile::new().map_err(|error| WorkdirError::io(&cwd, error))?;
    let stderr = tempfile::NamedTempFile::new().map_err(|error| WorkdirError::io(&cwd, error))?;
    let stdout_path = stdout.into_temp_path();
    let stderr_path = stderr.into_temp_path();
    let stdout_file = std::fs::File::create(&stdout_path)
        .map_err(|error| WorkdirError::io(&stdout_path, error))?;
    let stderr_file = std::fs::File::create(&stderr_path)
        .map_err(|error| WorkdirError::io(&stderr_path, error))?;

    telemetry.started(&command_id, request.tool_call_id.clone());
    let mut child = match Command::new("bash")
        .arg("-c")
        .arg(&request.command)
        .current_dir(&cwd)
        .envs(command_environment)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            telemetry.terminal(&command_id, CommandStatus::Failed, None);
            return Err(WorkdirError::io(&cwd, error));
        }
    };

    let mut stdout_reader =
        std::fs::File::open(&stdout_path).map_err(|error| WorkdirError::io(&stdout_path, error))?;
    let mut stderr_reader =
        std::fs::File::open(&stderr_path).map_err(|error| WorkdirError::io(&stderr_path, error))?;
    let mut stdout_decoder = CommandOutputDecoder::default();
    let mut stderr_decoder = CommandOutputDecoder::default();
    let mut timeout = Box::pin(tokio::time::sleep(Duration::from_secs(
        request.timeout_secs.max(1),
    )));
    let mut interval = tokio::time::interval(Duration::from_millis(50));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;

    let (status, exit_code) = loop {
        tokio::select! {
            exit = child.wait() => {
                let exit = exit.map_err(|error| WorkdirError::io(&cwd, error))?;
                break (
                    if exit.success() { CommandStatus::Completed } else { CommandStatus::Failed },
                    exit.code(),
                );
            }
            _ = &mut timeout => {
                let _ = child.start_kill();
                let exit_code = child.wait().await.ok().and_then(|status| status.code());
                break (CommandStatus::TimedOut, exit_code);
            }
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    let _ = child.start_kill();
                    let exit_code = child.wait().await.ok().and_then(|status| status.code());
                    break (CommandStatus::Cancelled, exit_code);
                }
            }
            _ = interval.tick() => {
                publish_available_output(
                    &mut stdout_reader,
                    &mut stdout_decoder,
                    &telemetry,
                    &command_id,
                    CommandStream::Stdout,
                    &stdout_path,
                    false,
                )?;
                publish_available_output(
                    &mut stderr_reader,
                    &mut stderr_decoder,
                    &telemetry,
                    &command_id,
                    CommandStream::Stderr,
                    &stderr_path,
                    false,
                )?;
            }
        }
    };

    publish_available_output(
        &mut stdout_reader,
        &mut stdout_decoder,
        &telemetry,
        &command_id,
        CommandStream::Stdout,
        &stdout_path,
        true,
    )?;
    publish_available_output(
        &mut stderr_reader,
        &mut stderr_decoder,
        &telemetry,
        &command_id,
        CommandStream::Stderr,
        &stderr_path,
        true,
    )?;
    telemetry.terminal(&command_id, status, exit_code);

    let (content, truncated) =
        read_command_output_files(&stdout_path, &stderr_path, request.output_limit.max(1))?;
    Ok(CommandOutput {
        status,
        exit_code,
        timed_out: status == CommandStatus::TimedOut,
        content,
        next_cursor: None,
        truncated,
    })
}

#[derive(Debug, Default)]
struct CommandOutputDecoder {
    read_offset: u64,
    emitted_offset: u64,
    pending: Vec<u8>,
}

fn publish_available_output(
    file: &mut std::fs::File,
    decoder: &mut CommandOutputDecoder,
    telemetry: &CommandTelemetry,
    command_id: &str,
    stream: CommandStream,
    path: &Path,
    flush: bool,
) -> Result<(), WorkdirError> {
    file.seek(SeekFrom::Start(decoder.read_offset))
        .map_err(|error| WorkdirError::io(path, error))?;
    loop {
        let mut buffer = vec![0; COMMAND_EVENT_CHUNK_BYTES];
        let read = file
            .read(&mut buffer)
            .map_err(|error| WorkdirError::io(path, error))?;
        if read == 0 {
            publish_decoded_output(decoder, telemetry, command_id, stream, flush);
            return Ok(());
        }
        decoder.pending.extend_from_slice(&buffer[..read]);
        decoder.read_offset = decoder.read_offset.saturating_add(read as u64);
        publish_decoded_output(decoder, telemetry, command_id, stream, false);
        if read < COMMAND_EVENT_CHUNK_BYTES {
            if flush {
                publish_decoded_output(decoder, telemetry, command_id, stream, true);
            }
            return Ok(());
        }
    }
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
/// UTF-8 scalar whose remaining bytes may arrive in a later file read. Definite
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

fn read_command_output_files(
    stdout_path: &Path,
    stderr_path: &Path,
    limit: usize,
) -> Result<(String, bool), WorkdirError> {
    let stdout_size = std::fs::metadata(stdout_path)
        .map_err(|error| WorkdirError::io(stdout_path, error))?
        .len() as usize;
    let stderr_size = std::fs::metadata(stderr_path)
        .map_err(|error| WorkdirError::io(stderr_path, error))?
        .len() as usize;
    let total = stdout_size.saturating_add(stderr_size);
    let stderr_budget = stderr_size.min(limit / 2);
    let stdout_budget = stdout_size.min(limit.saturating_sub(stderr_budget));
    let remaining = limit.saturating_sub(stdout_budget + stderr_budget);
    let stdout_budget = (stdout_budget + remaining.min(stdout_size - stdout_budget)).min(limit);
    let stderr_budget = limit.saturating_sub(stdout_budget).min(stderr_size);

    let stdout = read_tail(stdout_path, stdout_budget)?;
    let stderr = read_tail(stderr_path, stderr_budget)?;
    let mut content = String::new();
    if !stdout.is_empty() {
        content.push_str(&String::from_utf8_lossy(&stdout));
    }
    if !stderr.is_empty() {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(&String::from_utf8_lossy(&stderr));
    }
    Ok((content, total > limit))
}

fn read_tail(path: &Path, limit: usize) -> Result<Vec<u8>, WorkdirError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut file = std::fs::File::open(path).map_err(|error| WorkdirError::io(path, error))?;
    let len = file
        .metadata()
        .map_err(|error| WorkdirError::io(path, error))?
        .len();
    let start = len.saturating_sub(limit as u64);
    file.seek(SeekFrom::Start(start))
        .map_err(|error| WorkdirError::io(path, error))?;
    let mut bytes = Vec::with_capacity((len - start) as usize);
    file.read_to_end(&mut bytes)
        .map_err(|error| WorkdirError::io(path, error))?;
    Ok(bytes)
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

    fn make_fs(dir: &TempDir) -> LocalWorkdirSession {
        LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
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
        assert!(matches!(error, WorkdirError::Conflict(_)));

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
        assert!(matches!(error, WorkdirError::Conflict(_)));
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
                command: "sleep 30".to_owned(),
                timeout_secs: 60,
                output_limit: 1024,
                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let waiting_session = session.clone();
        let waiting_command = command.clone();
        let waiter = tokio::spawn(async move {
            WorkdirSession::command_output(
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
        assert!(matches!(waiter_error, WorkdirError::Unavailable(_)));

        assert!(matches!(
            WorkdirSession::command_status(&session, command).await,
            Err(WorkdirError::Unavailable(_))
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
            Err(WorkdirError::Unavailable(_))
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
    fn read_bytes_reports_symlink_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.read_bytes(&link).unwrap_err();
        assert!(
            matches!(
                err,
                WorkdirError::SymlinkOutOfScope { ref path, target: ref err_target, required_permission: "read" }
                    if path == &link && err_target == &target.canonicalize().unwrap()
            ),
            "expected symlink out-of-scope diagnostic, got {err:?}"
        );
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
    fn write_reports_symlink_target_outside_scope() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("target.txt");
        fs::write(&target, b"secret").unwrap();
        let link = dir.path().join("outside-repo.txt");
        symlink(&target, &link).unwrap();

        let fs = make_fs(&dir);
        let err = fs.write(&link, b"new").unwrap_err();
        assert!(
            matches!(
                err,
                WorkdirError::SymlinkOutOfScope { ref path, target: ref err_target, required_permission: "write" }
                    if path == &link && err_target == &target.canonicalize().unwrap()
            ),
            "expected write symlink out-of-scope diagnostic, got {err:?}"
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
            }],
            deny: vec![ScopeRule {
                target: sub.clone(),
                permission: Permission::Write,
                recursive: true,
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
        assert!(grep.output.contains("src/main.rs"));
        assert!(!grep.output.contains(dir.path().to_string_lossy().as_ref()));
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                command: "pwd && printf provider-command".into(),
                timeout_secs: 5,
                output_limit: 4096,
                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let output = WorkdirSession::command_output(
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
    async fn completed_command_output_can_be_read_in_bounded_unicode_pages() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                command: "printf 'aéz'".into(),
                timeout_secs: 5,
                output_limit: 1024,
                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let first = WorkdirSession::command_output(
            &workdir,
            CommandOutputRequest {
                handle: handle.clone(),
                cursor: 0,
                limit: 2,
                wait: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(first.content, "aé");
        assert_eq!(first.next_cursor, Some(2));

        let second = WorkdirSession::command_output(
            &workdir,
            CommandOutputRequest {
                handle: handle.clone(),
                cursor: first.next_cursor.unwrap(),
                limit: 2,
                wait: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(second.content, "z");
        assert_eq!(second.next_cursor, None);
        assert!(matches!(
            WorkdirSession::command_status(&workdir, handle).await,
            Err(WorkdirError::UnknownCommand(_))
        ));
    }

    #[test]
    fn command_output_decoder_preserves_utf8_split_across_file_reads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("command.out");
        let mut first_write = vec![b'a'; COMMAND_EVENT_CHUNK_BYTES - 1];
        first_write.push(0xe2);
        std::fs::write(&path, first_write).unwrap();

        let telemetry = CommandTelemetry::new();
        let mut events = telemetry.subscribe();
        telemetry.started("command-utf8", None);
        let mut decoder = CommandOutputDecoder::default();
        let mut reader = std::fs::File::open(&path).unwrap();
        publish_available_output(
            &mut reader,
            &mut decoder,
            &telemetry,
            "command-utf8",
            CommandStream::Stdout,
            &path,
            false,
        )
        .unwrap();
        assert_eq!(decoder.pending, vec![0xe2]);

        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writer.write_all(&[0x82, 0xac]).unwrap();
        writer.flush().unwrap();
        publish_available_output(
            &mut reader,
            &mut decoder,
            &telemetry,
            "command-utf8",
            CommandStream::Stdout,
            &path,
            false,
        )
        .unwrap();

        let output = std::iter::from_fn(|| events.try_recv().ok())
            .filter_map(|event| match event {
                CommandEvent::Output {
                    stream: CommandStream::Stdout,
                    content,
                    ..
                } => Some(content),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(output.len(), COMMAND_EVENT_CHUNK_BYTES - 1 + "€".len());
        assert!(output.ends_with('€'));
        assert!(!output.contains('\u{fffd}'));
        assert!(decoder.pending.is_empty());
    }

    #[tokio::test]
    async fn command_output_does_not_rewait_before_join_handle_finishes() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = CommandHandle("command-completion-race".into());
        let (completion_tx, completion) = watch::channel(false);
        let completion_observer = completion_tx.clone();
        let (cancel, _cancel_rx) = watch::channel(false);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
        let (completion_sent_tx, completion_sent_rx) = tokio::sync::oneshot::channel::<()>();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            start_rx.await.unwrap();
            completion_tx.send(true).unwrap();
            completion_sent_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(CommandOutput {
                status: CommandStatus::Completed,
                exit_code: Some(0),
                timed_out: false,
                content: "done".into(),
                next_cursor: None,
                truncated: false,
            })
        });
        workdir.inner.commands.lock().await.insert(
            handle.0.clone(),
            LocalCommand::Running {
                task,
                completion,
                cancel,
            },
        );

        let waiting_workdir = workdir.clone();
        let waiting_handle = handle.clone();
        let waiter = tokio::spawn(async move {
            WorkdirSession::command_output(
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
        for _ in 0..100 {
            if completion_observer.receiver_count() >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(
            completion_observer.receiver_count(),
            2,
            "waiter must subscribe before completion is published"
        );

        start_tx.send(()).unwrap();
        completion_sent_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert!(
            !waiter.is_finished(),
            "command_output should await the task after observing completion state"
        );
        release_tx.send(()).unwrap();

        let output = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("command output must not wait for a second completion notification")
            .unwrap()
            .unwrap();
        assert_eq!(output.status, CommandStatus::Completed);
        assert_eq!(output.content, "done");
    }

    #[tokio::test]
    async fn command_output_does_not_wait_forever_when_completion_sender_drops() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = CommandHandle("command-completion-drop".into());
        let (completion_tx, completion) = watch::channel(false);
        let (cancel, _cancel_rx) = watch::channel(false);
        let task: JoinHandle<Result<CommandOutput, WorkdirError>> = tokio::spawn(async move {
            drop(completion_tx);
            panic!("simulated command task panic");
        });
        workdir.inner.commands.lock().await.insert(
            handle.0.clone(),
            LocalCommand::Running {
                task,
                completion,
                cancel,
            },
        );

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            WorkdirSession::command_output(
                &workdir,
                CommandOutputRequest {
                    handle,
                    cursor: 0,
                    limit: 1024,
                    wait: true,
                },
            ),
        )
        .await
        .expect("closed completion channel must wake the waiter");
        assert!(matches!(result, Err(WorkdirError::Unavailable(_))));
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
                command: "printf ready; printf warning >&2; sleep 0.2; printf done".into(),
                timeout_secs: 5,
                output_limit: 1024,
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

        let output = WorkdirSession::command_output(
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
                command: "sleep 30".into(),
                timeout_secs: 1,
                output_limit: 1024,
                tool_call_id: None,
            },
        )
        .await
        .unwrap();
        let output = WorkdirSession::command_output(
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

    #[tokio::test]
    async fn provider_cancels_active_command() {
        let dir = TempDir::new().unwrap();
        let workdir = make_fs(&dir);
        let handle = WorkdirSession::start_command(
            &workdir,
            CommandRequest {
                command: "sleep 30".into(),
                timeout_secs: 60,
                output_limit: 1024,
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
            WorkdirSession::command_output(
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
        assert!(matches!(
            WorkdirSession::command_status(&workdir, handle).await,
            Err(WorkdirError::UnknownCommand(_))
        ));
    }
}
