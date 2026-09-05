use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use fs_operation::{
    EditRequest, EditResult, FsPath, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest,
    ListResult, ReadRequest, ReadResult, StatRequest, StatResult, WriteRequest, WriteResult,
};
use tokio::sync::broadcast;

use crate::{
    CommandEvent, CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest,
    CommandSnapshot, CommandStatus, Workdir, WorkdirError, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirSessionCapability, WorkdirSessionHandle,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirToolScopePermission {
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkdirToolScopeRule {
    pub target: FsPath,
    pub permission: WorkdirToolScopePermission,
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkdirToolScope {
    pub rules: Vec<WorkdirToolScopeRule>,
    pub cwd: FsPath,
    pub command: bool,
}

#[derive(Clone)]
pub struct WorkdirToolBroker {
    authority: Arc<ScopedWorkdirSession>,
    session: WorkdirSessionHandle,
    event_forwarder: Option<Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>>,
}

impl std::fmt::Debug for WorkdirToolBroker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkdirToolBroker")
            .field("workdir", self.session.workdir())
            .field("capabilities", &self.session.capabilities())
            .finish_non_exhaustive()
    }
}

impl WorkdirToolBroker {
    /// Own the parent Worker's active session and mediate every scoped child operation.
    pub fn new(source: WorkdirSessionHandle) -> Self {
        let capabilities = source.capabilities();
        let (command_events, _) = broadcast::channel(64);
        let authority = Arc::new(ScopedWorkdirSession {
            source,
            cwd: FsPath::new("").expect("empty Workdir path is valid"),
            scope: None,
            capabilities,
            validity: SessionValidity::root(),
            child_write_leases: Mutex::new(HashMap::new()),
            next_lease_id: AtomicU64::new(1),
            owned_commands: Arc::new(Mutex::new(HashSet::new())),
            command_events,
            closes_source: true,
        });
        Self {
            session: authority.clone(),
            authority,
            event_forwarder: None,
        }
    }

    /// Session used only by tools registered by the owning Worker.
    pub fn tool_session(&self) -> WorkdirSessionHandle {
        self.session.clone()
    }

    /// Create a revocable, attenuated tool route without delegating a provider session.
    pub async fn scope(
        &self,
        request: WorkdirToolScope,
    ) -> Result<WorkdirScopeLease, WorkdirError> {
        self.authority.scope(request).await
    }
}

impl std::ops::Deref for WorkdirToolBroker {
    type Target = WorkdirSessionHandle;

    fn deref(&self) -> &Self::Target {
        &self.session
    }
}

pub struct WorkdirScopeLease {
    broker: WorkdirToolBroker,
    pub capabilities: WorkdirSessionCapabilities,
    validity: Arc<SessionValidity>,
    cleanup_pending: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkdirScopeLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkdirScopeLease")
            .field("workdir", self.broker.session.workdir())
            .field("capabilities", &self.capabilities)
            .field("active", &self.is_active())
            .finish()
    }
}

impl WorkdirScopeLease {
    pub fn broker(&self) -> WorkdirToolBroker {
        self.broker.clone()
    }

    pub fn tool_session(&self) -> WorkdirSessionHandle {
        self.broker.tool_session()
    }

    pub async fn scope(
        &self,
        request: WorkdirToolScope,
    ) -> Result<WorkdirScopeLease, WorkdirError> {
        self.broker.scope(request).await
    }

    pub async fn close(&self) -> Result<(), WorkdirError> {
        self.validity.active.store(false, Ordering::Release);
        let command_ids = self
            .broker
            .authority
            .owned_commands
            .lock()
            .expect("scoped command set mutex poisoned")
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut first_error = None;
        for command_id in command_ids {
            let handle = CommandHandle(command_id.clone());
            let cancel = self
                .broker
                .authority
                .source
                .cancel_command(handle.clone())
                .await;
            let terminal = self
                .broker
                .authority
                .source
                .command_output(CommandOutputRequest {
                    handle,
                    cursor: 0,
                    limit: 1,
                    wait: true,
                })
                .await;
            match (cancel, terminal) {
                (_, Ok(_))
                | (Ok(()), Err(WorkdirError::UnknownCommand(_)))
                | (Err(WorkdirError::UnknownCommand(_)), Err(WorkdirError::UnknownCommand(_))) => {
                    self.broker
                        .authority
                        .owned_commands
                        .lock()
                        .expect("scoped command set mutex poisoned")
                        .remove(&command_id);
                }
                (Err(error), _) | (_, Err(error)) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        tokio::task::yield_now().await;
        self.finish_release();
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.validity.is_active()
    }

    /// Revoke a scope whose owner has already terminalized every tool call.
    /// Use [`Self::close`] when commands may still be live.
    pub fn revoke(&self) {
        self.finish_release();
    }

    fn finish_release(&self) {
        self.validity.active.store(false, Ordering::Release);
        self.cleanup_pending.store(false, Ordering::Release);
        if let Some(forwarder) = &self.broker.event_forwarder
            && let Some(handle) = forwarder
                .lock()
                .expect("scoped command forwarder mutex poisoned")
                .take()
        {
            handle.abort();
        }
    }
}

impl std::ops::Deref for WorkdirScopeLease {
    type Target = WorkdirSessionHandle;

    fn deref(&self) -> &Self::Target {
        &self.broker.session
    }
}

impl Drop for WorkdirScopeLease {
    fn drop(&mut self) {
        self.finish_release();
    }
}

#[derive(Debug)]
struct SessionValidity {
    active: AtomicBool,
    parent: Option<Arc<SessionValidity>>,
}

impl SessionValidity {
    fn root() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(true),
            parent: None,
        })
    }

    fn child(parent: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(true),
            parent: Some(parent),
        })
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
            && self.parent.as_ref().is_none_or(|parent| parent.is_active())
    }
}

#[derive(Clone, Debug)]
struct ActiveWriteLease {
    validity: Weak<SessionValidity>,
    cleanup_pending: Weak<AtomicBool>,
    rules: Vec<WorkdirToolScopeRule>,
}

struct ScopedWorkdirSession {
    source: WorkdirSessionHandle,
    cwd: FsPath,
    scope: Option<Vec<WorkdirToolScopeRule>>,
    capabilities: WorkdirSessionCapabilities,
    validity: Arc<SessionValidity>,
    child_write_leases: Mutex<HashMap<u64, ActiveWriteLease>>,
    next_lease_id: AtomicU64,
    owned_commands: Arc<Mutex<HashSet<String>>>,
    command_events: broadcast::Sender<CommandEvent>,
    closes_source: bool,
}

impl std::fmt::Debug for ScopedWorkdirSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedWorkdirSession")
            .field("workdir", &self.source.workdir())
            .field("scope", &self.scope)
            .field("capabilities", &self.capabilities)
            .field("active", &self.validity.is_active())
            .finish_non_exhaustive()
    }
}

impl ScopedWorkdirSession {
    fn ensure_active(&self) -> Result<(), WorkdirError> {
        if self.validity.is_active() {
            Ok(())
        } else {
            Err(WorkdirError::SessionClosed)
        }
    }

    fn ensure_capability(
        &self,
        required: WorkdirSessionCapability,
        operation: &'static str,
    ) -> Result<(), WorkdirError> {
        self.ensure_active()?;
        if self.capabilities.supports(required) {
            Ok(())
        } else {
            Err(WorkdirError::Denied(format!(
                "scoped Workdir tools do not permit {operation}"
            )))
        }
    }

    fn ensure_path(
        &self,
        path: &FsPath,
        permission: WorkdirToolScopePermission,
    ) -> Result<(), WorkdirError> {
        self.ensure_active()?;
        if let Some(scope) = &self.scope {
            if !scope
                .iter()
                .any(|rule| rule_allows_path(rule, path, permission))
            {
                return Err(WorkdirError::Denied(format!(
                    "logical workdir path `{path}` is outside the scoped {permission:?} scope"
                )));
            }
        }
        if permission == WorkdirToolScopePermission::Write {
            self.ensure_parent_write_available(path)?;
        }
        Ok(())
    }

    fn resolve_path(&self, path: &FsPath) -> Result<FsPath, WorkdirError> {
        if self.cwd.as_str().is_empty() {
            return Ok(path.clone());
        }
        let joined = Path::new(self.cwd.as_str()).join(path.as_str());
        let joined = joined.to_str().ok_or_else(|| {
            WorkdirError::Denied("logical Workdir path is not valid UTF-8".into())
        })?;
        FsPath::new(joined).map_err(|error| WorkdirError::Denied(error.to_string()))
    }

    fn ensure_read(
        &self,
        path: &FsPath,
        capability: WorkdirSessionCapability,
    ) -> Result<(), WorkdirError> {
        self.ensure_capability(capability, "read operations")?;
        self.ensure_path(path, WorkdirToolScopePermission::Read)
    }

    fn ensure_write(
        &self,
        path: &FsPath,
        capability: WorkdirSessionCapability,
    ) -> Result<(), WorkdirError> {
        self.ensure_capability(capability, "write operations")?;
        self.ensure_path(path, WorkdirToolScopePermission::Write)
    }

    fn ensure_command(&self) -> Result<(), WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command, "command execution")
    }

    fn ensure_owned_command(&self, handle: &CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_command()?;
        if self.scope.is_none()
            || self
                .owned_commands
                .lock()
                .expect("scoped command set mutex poisoned")
                .contains(&handle.0)
        {
            Ok(())
        } else {
            Err(WorkdirError::UnknownCommand(handle.0.clone()))
        }
    }

    fn ensure_parent_write_available(&self, path: &FsPath) -> Result<(), WorkdirError> {
        let mut leases = self
            .child_write_leases
            .lock()
            .expect("Workdir tool scope lease mutex poisoned");
        leases.retain(|_, lease| lease.validity.upgrade().is_some_and(|v| v.is_active()));
        if leases.values().any(|lease| {
            lease.rules.iter().any(|rule| {
                rule.permission == WorkdirToolScopePermission::Write
                    && rule_allows_path(rule, path, WorkdirToolScopePermission::Write)
            })
        }) {
            Err(WorkdirError::Denied(format!(
                "logical workdir path `{path}` is leased to child Workdir tools"
            )))
        } else {
            Ok(())
        }
    }

    async fn ensure_source_path_has_no_symlink(&self, path: &FsPath) -> Result<(), WorkdirError> {
        let mut current = String::new();
        for component in Path::new(path.as_str()).components() {
            let component = component.as_os_str().to_string_lossy();
            if component.is_empty() || component == "." {
                continue;
            }
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(&component);
            let current = FsPath::new(&current).map_err(|error| {
                WorkdirError::Denied(format!("invalid scoped Workdir path: {error}"))
            })?;
            match self.source.stat(StatRequest { path: current }).await {
                Ok(result) if result.kind == fs_operation::EntryKind::Symlink => {
                    return Err(WorkdirError::Denied(format!(
                        "scoped Workdir path `{path}` traverses a symlink"
                    )));
                }
                Ok(_) => {}
                Err(WorkdirError::NotFound(_)) => break,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn ensure_scope_targets_do_not_traverse_symlinks(
        &self,
        rules: &[WorkdirToolScopeRule],
    ) -> Result<(), WorkdirError> {
        for rule in rules {
            self.ensure_source_path_has_no_symlink(&rule.target).await?;
        }
        Ok(())
    }

    async fn resolve_operation_path(&self, path: &FsPath) -> Result<FsPath, WorkdirError> {
        self.ensure_active()?;
        let resolved = self.resolve_path(path)?;
        if self.scope.is_some() {
            self.ensure_source_path_has_no_symlink(&resolved).await?;
        }
        Ok(resolved)
    }

    fn validate_scope(
        &self,
        rules: &[WorkdirToolScopeRule],
        command: bool,
    ) -> Result<WorkdirSessionCapabilities, WorkdirError> {
        self.ensure_active()?;
        if rules.is_empty() {
            return Err(WorkdirError::Denied(
                "workdir tool scope requires at least one logical scope rule".into(),
            ));
        }
        let writable = rules
            .iter()
            .any(|rule| rule.permission == WorkdirToolScopePermission::Write);
        if !self.capabilities.supports(WorkdirSessionCapability::Read)
            || (writable
                && (!self.capabilities.supports(WorkdirSessionCapability::Write)
                    || !self.capabilities.supports(WorkdirSessionCapability::Edit)))
        {
            return Err(WorkdirError::Denied(
                "parent Workdir session cannot scope the requested capabilities".into(),
            ));
        }
        if command {
            if !writable {
                return Err(WorkdirError::Denied(
                    "command execution requires a writable scoped path".into(),
                ));
            }
            if !self
                .capabilities
                .supports(WorkdirSessionCapability::Command)
            {
                return Err(WorkdirError::Denied(
                    "parent Workdir session does not support Command".into(),
                ));
            }
        }
        for requested in rules {
            if let Some(scope) = &self.scope {
                if !scope
                    .iter()
                    .any(|parent| rule_contains_rule(parent, requested))
                {
                    return Err(WorkdirError::Denied(format!(
                        "logical workdir scope `{}` exceeds the parent tool scope",
                        requested.target
                    )));
                }
            }
        }
        let mut delegated = vec![WorkdirSessionCapability::Read];
        for capability in [
            WorkdirSessionCapability::Glob,
            WorkdirSessionCapability::Grep,
        ] {
            if self.capabilities.supports(capability) {
                delegated.push(capability);
            }
        }
        if writable {
            delegated.push(WorkdirSessionCapability::Write);
            delegated.push(WorkdirSessionCapability::Edit);
        }
        if command {
            delegated.push(WorkdirSessionCapability::Command);
        }
        Ok(WorkdirSessionCapabilities::from_capabilities(delegated))
    }

    async fn scope(
        self: &Arc<Self>,
        request: WorkdirToolScope,
    ) -> Result<WorkdirScopeLease, WorkdirError> {
        let capabilities = self.validate_scope(&request.rules, request.command)?;
        if !request
            .rules
            .iter()
            .any(|rule| rule_allows_path(rule, &request.cwd, WorkdirToolScopePermission::Read))
        {
            return Err(WorkdirError::Denied(format!(
                "scoped tool cwd `{}` is outside the readable scope",
                request.cwd
            )));
        }
        self.ensure_scope_targets_do_not_traverse_symlinks(&request.rules)
            .await?;
        let validity = SessionValidity::child(self.validity.clone());
        let cleanup_pending = Arc::new(AtomicBool::new(true));
        let id = self.next_lease_id.fetch_add(1, Ordering::Relaxed);
        if request
            .rules
            .iter()
            .any(|rule| rule.permission == WorkdirToolScopePermission::Write)
        {
            let mut leases = self
                .child_write_leases
                .lock()
                .expect("Workdir tool scope lease mutex poisoned");
            leases.retain(|_, lease| {
                lease
                    .validity
                    .upgrade()
                    .is_some_and(|validity| validity.is_active())
                    || lease
                        .cleanup_pending
                        .upgrade()
                        .is_some_and(|pending| pending.load(Ordering::Acquire))
            });
            let requested_write_rules = request
                .rules
                .iter()
                .filter(|rule| rule.permission == WorkdirToolScopePermission::Write);
            for requested in requested_write_rules {
                if leases.values().any(|lease| {
                    lease
                        .rules
                        .iter()
                        .any(|active| rules_overlap(active, requested))
                }) {
                    return Err(WorkdirError::Denied(format!(
                        "scoped write path `{}` overlaps an active child scope",
                        requested.target
                    )));
                }
            }
            leases.insert(
                id,
                ActiveWriteLease {
                    validity: Arc::downgrade(&validity),
                    cleanup_pending: Arc::downgrade(&cleanup_pending),
                    rules: request.rules.clone(),
                },
            );
        }
        let owned_commands = Arc::new(Mutex::new(HashSet::new()));
        let (command_events, _) = broadcast::channel(64);
        let event_forwarder = forward_owned_command_events(
            self.source.subscribe_command_events(),
            owned_commands.clone(),
            command_events.clone(),
        )
        .map(|handle| Arc::new(Mutex::new(Some(handle))));
        let child = Arc::new(ScopedWorkdirSession {
            source: self.source.clone(),
            cwd: request.cwd,
            scope: Some(request.rules),
            capabilities,
            validity: validity.clone(),
            child_write_leases: Mutex::new(HashMap::new()),
            next_lease_id: AtomicU64::new(1),
            owned_commands,
            command_events,
            closes_source: false,
        });
        let broker = WorkdirToolBroker {
            session: child.clone(),
            authority: child,
            event_forwarder,
        };
        Ok(WorkdirScopeLease {
            broker,
            capabilities,
            validity,
            cleanup_pending,
        })
    }
}

#[async_trait]
impl WorkdirSession for ScopedWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.source.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.capabilities
    }

    async fn stat(&self, mut request: StatRequest) -> Result<StatResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        request.path = path;
        self.source.stat(request).await
    }

    async fn read(&self, mut request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        request.path = path;
        self.source.read(request).await
    }

    async fn write(&self, mut request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_write(&path, WorkdirSessionCapability::Write)?;
        request.path = path;
        self.source.write(request).await
    }

    async fn edit(&self, mut request: EditRequest) -> Result<EditResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_write(&path, WorkdirSessionCapability::Edit)?;
        request.path = path;
        self.source.edit(request).await
    }

    async fn list(&self, mut request: ListRequest) -> Result<ListResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        request.path = path;
        self.source.list(request).await
    }

    async fn glob(&self, mut request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_read(&path, WorkdirSessionCapability::Glob)?;
        request.path = path;
        self.source.glob(request).await
    }

    async fn grep(&self, mut request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        let path = self.resolve_operation_path(&request.path).await?;
        self.ensure_read(&path, WorkdirSessionCapability::Grep)?;
        request.path = path;
        self.source.grep(request).await
    }

    async fn start_command(
        &self,
        mut request: CommandRequest,
    ) -> Result<CommandHandle, WorkdirError> {
        self.ensure_command()?;
        let tool_call_id = request.tool_call_id.clone();
        if self.scope.is_some() {
            request.cwd = Some(match request.cwd.as_ref() {
                Some(cwd) => self.resolve_path(cwd)?,
                None => self.cwd.clone(),
            });
        }
        let handle = self.source.start_command(request).await?;
        self.owned_commands
            .lock()
            .expect("scoped command set mutex poisoned")
            .insert(handle.0.clone());
        let _ = self.command_events.send(CommandEvent::Started {
            command_id: handle.0.clone(),
            tool_call_id,
            observed_at_ms: unix_timestamp_ms(),
        });
        Ok(handle)
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.ensure_owned_command(&handle)?;
        self.source.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.ensure_owned_command(&request.handle)?;
        let command_id = request.handle.0.clone();
        let output = self.source.command_output(request).await?;
        if !matches!(output.status, CommandStatus::Running) {
            self.owned_commands
                .lock()
                .expect("scoped command set mutex poisoned")
                .remove(&command_id);
        }
        Ok(output)
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_owned_command(&handle)?;
        self.source.cancel_command(handle).await
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        if !self
            .capabilities
            .supports(WorkdirSessionCapability::Command)
        {
            return None;
        }
        if self.scope.is_none() {
            self.source.subscribe_command_events()
        } else {
            Some(self.command_events.subscribe())
        }
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        if !self
            .capabilities
            .supports(WorkdirSessionCapability::Command)
        {
            return Vec::new();
        }
        if self.scope.is_none() {
            return self.source.command_snapshot();
        }
        let owned = self
            .owned_commands
            .lock()
            .expect("scoped command set mutex poisoned");
        self.source
            .command_snapshot()
            .into_iter()
            .filter(|snapshot| owned.contains(&snapshot.command_id))
            .collect()
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        self.validity.active.store(false, Ordering::Release);
        if self.closes_source {
            self.source.close().await
        } else {
            Ok(())
        }
    }
}

/// A fail-closed read-only view over an already scoped scoped tool route.
#[derive(Debug)]
pub struct ReadOnlyWorkdirSession {
    inner: WorkdirSessionHandle,
}

impl ReadOnlyWorkdirSession {
    pub fn new(inner: WorkdirSessionHandle) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl WorkdirSession for ReadOnlyWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.inner.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        WorkdirSessionCapabilities::READ_ONLY
    }

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.inner.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.inner.read(request).await
    }

    async fn write(&self, _request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn edit(&self, _request: EditRequest) -> Result<EditResult, WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.inner.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.inner.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.inner.grep(request).await
    }

    async fn start_command(&self, _request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn command_status(&self, _handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn command_output(
        &self,
        _request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn cancel_command(&self, _handle: CommandHandle) -> Result<(), WorkdirError> {
        Err(WorkdirError::Denied("read-only workdir session".into()))
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        self.inner.close().await
    }
}

fn forward_owned_command_events(
    receiver: Option<broadcast::Receiver<CommandEvent>>,
    owned_commands: Arc<Mutex<HashSet<String>>>,
    sender: broadcast::Sender<CommandEvent>,
) -> Option<tokio::task::JoinHandle<()>> {
    let mut receiver = receiver?;
    Some(tokio::spawn(async move {
        loop {
            let event = match receiver.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            };
            let command_id = match &event {
                CommandEvent::Started { .. } => continue,
                CommandEvent::Output { command_id, .. }
                | CommandEvent::Terminal { command_id, .. } => command_id.clone(),
            };
            let owned = owned_commands
                .lock()
                .expect("scoped command set mutex poisoned")
                .contains(&command_id);
            if owned {
                let _ = sender.send(event);
            }
        }
    }))
}

fn unix_timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn rules_overlap(left: &WorkdirToolScopeRule, right: &WorkdirToolScopeRule) -> bool {
    left.permission == WorkdirToolScopePermission::Write
        && right.permission == WorkdirToolScopePermission::Write
        && (rule_allows_path(left, &right.target, WorkdirToolScopePermission::Write)
            || rule_allows_path(right, &left.target, WorkdirToolScopePermission::Write))
}

fn rule_allows_path(
    rule: &WorkdirToolScopeRule,
    path: &FsPath,
    required: WorkdirToolScopePermission,
) -> bool {
    if required == WorkdirToolScopePermission::Write
        && rule.permission != WorkdirToolScopePermission::Write
    {
        return false;
    }
    path_in_rule(rule, path)
}

fn path_in_rule(rule: &WorkdirToolScopeRule, path: &FsPath) -> bool {
    let target = Path::new(rule.target.as_str());
    let path = Path::new(path.as_str());
    if path == target {
        return true;
    }
    let Ok(suffix) = path.strip_prefix(target) else {
        return false;
    };
    let depth = suffix.components().count();
    rule.recursive || depth <= 1
}

fn rule_contains_rule(parent: &WorkdirToolScopeRule, child: &WorkdirToolScopeRule) -> bool {
    if child.permission == WorkdirToolScopePermission::Write
        && parent.permission != WorkdirToolScopePermission::Write
    {
        return false;
    }
    if !path_in_rule(parent, &child.target) {
        return false;
    }
    if parent.recursive {
        return true;
    }
    !child.recursive && parent.target == child.target
}

#[cfg(test)]
mod tests {
    use std::fs;

    use manifest::{Permission, Scope, ScopeConfig, ScopeRule, SharedScope};
    use tempfile::TempDir;

    use super::*;
    use crate::LocalWorkdirSession;

    fn fs_path(path: &str) -> FsPath {
        FsPath::new(path).unwrap()
    }

    fn session(root: &Path) -> WorkdirToolBroker {
        let scope = SharedScope::new(
            Scope::from_config(&ScopeConfig {
                allow: vec![ScopeRule {
                    target: root.to_path_buf(),
                    permission: Permission::Write,
                    recursive: true,
                }],
                deny: Vec::new(),
            })
            .unwrap(),
        );
        WorkdirToolBroker::new(Arc::new(LocalWorkdirSession::materialized_bound(
            Workdir::new("delegation-test"),
            root.to_path_buf(),
            root.to_path_buf(),
            scope,
            WorkdirSessionCapabilities::ALL,
        )))
    }

    fn request(path: &str, permission: WorkdirToolScopePermission) -> WorkdirToolScope {
        WorkdirToolScope {
            rules: vec![WorkdirToolScopeRule {
                target: fs_path(path),
                permission,
                recursive: true,
            }],
            cwd: fs_path(path),
            command: permission == WorkdirToolScopePermission::Write,
        }
    }

    fn read(path: &str) -> ReadRequest {
        ReadRequest {
            path: fs_path(path),
            offset: 0,
            limit: 20,
            max_bytes: 1024,
        }
    }

    fn write(path: &str, content: &str) -> WriteRequest {
        WriteRequest {
            path: fs_path(path),
            content: content.as_bytes().to_vec(),
            expected_hash: None,
        }
    }

    async fn run_command(
        session: &WorkdirSessionHandle,
        command: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> CommandOutput {
        let handle = session
            .start_command(CommandRequest {
                command: command.into(),
                timeout_secs: 5,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: Some(tool_call_id.into()),
            })
            .await
            .unwrap();
        session
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1024,
                wait: true,
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn workdir_tool_broker_session_forwards_command_telemetry() {
        let root = TempDir::new().unwrap();
        let parent = session(root.path());
        let mut events = parent
            .subscribe_command_events()
            .expect("delegation wrapper must preserve command observation");
        let handle = parent
            .start_command(CommandRequest {
                command: "printf ready; sleep 0.2; printf done".into(),
                timeout_secs: 5,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: Some("tool-delegated".into()),
            })
            .await
            .unwrap();

        let first_output = loop {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
                .await
                .expect("delegated command telemetry should not stall")
                .unwrap();
            if let CommandEvent::Output { content, .. } = event {
                break content;
            }
        };
        assert_eq!(first_output, "ready");
        let snapshots = parent.command_snapshot();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].command_id, handle.0);
        assert_eq!(snapshots[0].status, CommandStatus::Running);
        assert_eq!(snapshots[0].stdout.content, "ready");

        let output = parent
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1024,
                wait: true,
            })
            .await
            .unwrap();
        assert_eq!(output.status, CommandStatus::Completed);
        assert_eq!(output.content, "readydone");
        assert!(parent.command_snapshot().is_empty());
    }

    #[tokio::test]
    async fn write_scope_without_command_grant_has_no_command_capability() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("work")).unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(WorkdirToolScope {
                rules: vec![WorkdirToolScopeRule {
                    target: fs_path("work"),
                    permission: WorkdirToolScopePermission::Write,
                    recursive: true,
                }],
                cwd: fs_path("work"),
                command: false,
            })
            .await
            .unwrap();

        assert!(child.capabilities.supports(WorkdirSessionCapability::Write));
        assert!(
            !child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        let error = child
            .start_command(CommandRequest {
                command: "pwd".into(),
                timeout_secs: 5,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(error, WorkdirError::Denied(_)));
    }

    #[tokio::test]
    async fn scoped_commands_use_child_cwd_and_do_not_leak_between_siblings() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("one")).unwrap();
        fs::create_dir_all(root.path().join("two")).unwrap();
        let parent = session(root.path());
        let first = parent
            .scope(request("one", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        let second = parent
            .scope(request("two", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        let mut first_events = first.subscribe_command_events().unwrap();
        let mut second_events = second.subscribe_command_events().unwrap();

        let handle = first
            .start_command(CommandRequest {
                command: "pwd; sleep 0.2".into(),
                timeout_secs: 5,
                output_limit: 4096,
                cwd: None,
                spill_dir: None,
                tool_call_id: Some("first-command".into()),
            })
            .await
            .unwrap();
        assert!(matches!(
            first_events.recv().await.unwrap(),
            CommandEvent::Started { .. }
        ));
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_millis(50), second_events.recv()).await,
            Err(_)
        ));
        assert!(matches!(
            second.command_status(handle.clone()).await,
            Err(WorkdirError::UnknownCommand(_))
        ));

        let output = first
            .command_output(CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 4096,
                wait: true,
            })
            .await
            .unwrap();
        let expected = root.path().join("one").to_string_lossy().into_owned();
        assert!(
            output
                .content
                .lines()
                .next()
                .is_some_and(|line| line == expected)
        );
    }

    #[test]
    fn non_recursive_rule_covers_target_and_direct_children_only() {
        let rule = WorkdirToolScopeRule {
            target: fs_path("docs"),
            permission: WorkdirToolScopePermission::Read,
            recursive: false,
        };
        assert!(path_in_rule(&rule, &fs_path("docs")));
        assert!(path_in_rule(&rule, &fs_path("docs/readme.md")));
        assert!(!path_in_rule(&rule, &fs_path("docs/guides/start.md")));
    }

    #[tokio::test]
    async fn read_only_delegation_allows_prefix_and_denies_mutation() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs")).unwrap();
        fs::create_dir_all(root.path().join("secret")).unwrap();
        fs::write(root.path().join("docs/readme.md"), "visible").unwrap();
        fs::write(root.path().join("secret/key"), "hidden").unwrap();
        let parent = session(root.path());

        let child = parent
            .scope(request("docs", WorkdirToolScopePermission::Read))
            .await
            .unwrap();
        assert_eq!(child.capabilities, WorkdirSessionCapabilities::READ_ONLY);
        assert_eq!(
            child.read(read("readme.md")).await.unwrap().bytes,
            b"visible"
        );
        assert!(matches!(
            child.write(write("new.md", "no")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(
            !child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        assert!(child.subscribe_command_events().is_none());
        assert!(child.command_snapshot().is_empty());
        assert!(matches!(
            child
                .start_command(CommandRequest {
                    command: "printf denied".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
                    cwd: None,
                    spill_dir: None,
                    tool_call_id: Some("read-only-command".into()),
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_scope_denies_read_through_symlink_outside_grant() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("granted")).unwrap();
        fs::create_dir_all(root.path().join("secret")).unwrap();
        fs::write(root.path().join("secret/key"), "hidden").unwrap();
        symlink("../secret/key", root.path().join("granted/link")).unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(request("granted", WorkdirToolScopePermission::Read))
            .await
            .unwrap();

        let result = child.read(read("link")).await;
        assert!(
            result.is_err(),
            "symlink read escaped provider scope: {result:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provider_scope_denies_write_through_symlink_outside_grant() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("granted")).unwrap();
        fs::create_dir_all(root.path().join("secret")).unwrap();
        symlink("../secret", root.path().join("granted/outside")).unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(request("granted", WorkdirToolScopePermission::Write))
            .await
            .unwrap();

        let result = child.write(write("outside/new", "forbidden")).await;
        assert!(
            result.is_err(),
            "symlink write escaped provider scope: {result:?}"
        );
        assert!(!root.path().join("secret/new").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_delegation_rejects_symlink_target_before_lease() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("granted")).unwrap();
        fs::create_dir_all(root.path().join("secret")).unwrap();
        symlink("../secret", root.path().join("granted/outside")).unwrap();
        let parent = session(root.path());

        assert!(matches!(
            parent
                .scope(request(
                    "granted/outside",
                    WorkdirToolScopePermission::Write
                ))
                .await,
            Err(WorkdirError::Denied(_))
        ));
        parent
            .write(write("secret/parent", "still-authoritative"))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn write_lease_keeps_typed_parent_writes_exclusive_without_blocking_commands() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("leased")).unwrap();
        fs::create_dir_all(root.path().join("other")).unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(request("leased", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        assert!(
            child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        let child_output =
            run_command(&child, "printf child-command", "delegated-child-command").await;
        assert_eq!(child_output.content, "child-command");
        let parent_output = run_command(
            &parent,
            "printf parent-write > leased/from-command; printf parent-command",
            "parent-command-during-child-write",
        )
        .await;
        assert_eq!(parent_output.status, CommandStatus::Completed);
        assert_eq!(parent_output.content, "parent-command");
        assert_eq!(
            fs::read_to_string(root.path().join("leased/from-command")).unwrap(),
            "parent-write"
        );

        assert!(matches!(
            parent.write(write("leased/file", "parent")).await,
            Err(WorkdirError::Denied(_))
        ));
        parent.write(write("other/file", "parent")).await.unwrap();
        child.write(write("file", "child")).await.unwrap();
        child.close().await.unwrap();
        assert!(matches!(
            child
                .start_command(CommandRequest {
                    command: "printf revoked".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
                    cwd: None,
                    spill_dir: None,
                    tool_call_id: Some("revoked-child-command".into()),
                })
                .await,
            Err(WorkdirError::SessionClosed)
        ));
        parent
            .write(write("leased/parent", "parent"))
            .await
            .unwrap();
        assert!(matches!(
            child.read(read("file")).await,
            Err(WorkdirError::SessionClosed)
        ));
    }

    #[tokio::test]
    async fn sibling_write_scopes_must_not_overlap() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("shared/one")).unwrap();
        fs::create_dir_all(root.path().join("other")).unwrap();
        let parent = session(root.path());
        let first = parent
            .scope(request("shared", WorkdirToolScopePermission::Write))
            .await
            .unwrap();

        assert!(matches!(
            parent
                .scope(request("shared/one", WorkdirToolScopePermission::Write))
                .await,
            Err(WorkdirError::Denied(_))
        ));
        let other = parent
            .scope(request("other", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        other.close().await.unwrap();
        first.close().await.unwrap();
    }

    #[tokio::test]
    async fn closing_scope_cancels_and_terminalizes_owned_commands() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("work")).unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(request("work", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        let mut events = child.subscribe_command_events().unwrap();
        let handle = child
            .start_command(CommandRequest {
                command: "sleep 30; printf leaked > marker".into(),
                timeout_secs: 60,
                output_limit: 1024,
                cwd: None,
                spill_dir: None,
                tool_call_id: Some("owned-command".into()),
            })
            .await
            .unwrap();
        assert!(matches!(
            events.recv().await.unwrap(),
            CommandEvent::Started { .. }
        ));

        child.close().await.unwrap();

        assert!(matches!(
            parent.command_status(handle).await,
            Ok(CommandStatus::Cancelled | CommandStatus::Completed | CommandStatus::Failed)
                | Err(WorkdirError::UnknownCommand(_))
        ));
        assert!(!root.path().join("work/marker").exists());
        let terminal = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                if let CommandEvent::Terminal { .. } = events.recv().await.unwrap() {
                    break;
                }
            }
        })
        .await;
        assert!(
            terminal.is_ok(),
            "scope close must publish terminal command telemetry"
        );
    }

    #[tokio::test]
    async fn nested_delegation_is_attenuated_and_parent_revocation_cascades() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs/sub")).unwrap();
        fs::create_dir_all(root.path().join("docs/peer")).unwrap();
        fs::write(root.path().join("docs/sub/a"), "a").unwrap();
        fs::write(root.path().join("docs/peer/b"), "b").unwrap();
        let root_session = session(root.path());
        let child = root_session
            .scope(request("docs", WorkdirToolScopePermission::Read))
            .await
            .unwrap();
        let nested = child
            .scope(request("docs/sub", WorkdirToolScopePermission::Read))
            .await
            .unwrap();

        nested.read(read("a")).await.unwrap();
        assert!(
            child
                .scope(request("other", WorkdirToolScopePermission::Read))
                .await
                .is_err()
        );
        assert!(
            child
                .scope(request("docs/sub", WorkdirToolScopePermission::Write))
                .await
                .is_err()
        );

        child.close().await.unwrap();
        assert!(matches!(
            nested.read(read("a")).await,
            Err(WorkdirError::SessionClosed)
        ));
    }

    #[tokio::test]
    async fn nested_write_leases_do_not_block_command_capable_ancestors() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs/sub")).unwrap();
        let root_session = session(root.path());
        let child = root_session
            .scope(request("docs", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        let nested = child
            .scope(request("docs/sub", WorkdirToolScopePermission::Write))
            .await
            .unwrap();

        for (session, label) in [
            (root_session.tool_session(), "root"),
            (child.tool_session(), "child"),
            (nested.tool_session(), "nested"),
        ] {
            let output = run_command(
                &session,
                format!("printf {label}"),
                format!("{label}-command-during-nested-write"),
            )
            .await;
            assert_eq!(output.status, CommandStatus::Completed);
            assert_eq!(output.content, label);
        }

        assert!(matches!(
            root_session.write(write("docs/root", "blocked")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(matches!(
            child.write(write("sub/child", "blocked")).await,
            Err(WorkdirError::Denied(_))
        ));
        nested.write(write("nested", "allowed")).await.unwrap();

        nested.close().await.unwrap();
        child.close().await.unwrap();
    }

    #[tokio::test]
    async fn closing_parent_invalidates_scoped_tools() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs")).unwrap();
        fs::write(root.path().join("docs/a"), "a").unwrap();
        let parent = session(root.path());
        let child = parent
            .scope(request("docs", WorkdirToolScopePermission::Read))
            .await
            .unwrap();

        parent.close().await.unwrap();
        assert!(matches!(
            parent
                .start_command(CommandRequest {
                    command: "printf closed".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
                    cwd: None,
                    spill_dir: None,
                    tool_call_id: Some("closed-parent-command".into()),
                })
                .await,
            Err(WorkdirError::SessionClosed)
        ));
        let child_result = child.read(read("a")).await;
        assert!(
            matches!(child_result, Err(WorkdirError::SessionClosed)),
            "child result after parent close: {child_result:?}"
        );
    }
}
