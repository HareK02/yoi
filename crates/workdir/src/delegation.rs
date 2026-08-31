use std::collections::HashMap;
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
pub enum WorkdirDelegationPermission {
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkdirDelegationRule {
    pub target: FsPath,
    pub permission: WorkdirDelegationPermission,
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkdirDelegationRequest {
    pub rules: Vec<WorkdirDelegationRule>,
    pub cwd: FsPath,
}

pub struct WorkdirDelegation {
    pub scoped_session: WorkdirSessionHandle,
    pub capabilities: WorkdirSessionCapabilities,
    validity: Arc<SessionValidity>,
}

impl std::fmt::Debug for WorkdirDelegation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkdirDelegation")
            .field("workdir", &self.scoped_session.workdir())
            .field("capabilities", &self.capabilities)
            .field("active", &self.is_active())
            .finish()
    }
}

impl WorkdirDelegation {
    pub fn is_active(&self) -> bool {
        self.validity.is_active()
    }

    pub fn release(&self) {
        self.validity.active.store(false, Ordering::Release);
    }
}

impl Drop for WorkdirDelegation {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct AppliedWorkdirDelegation {
    pub scoped_session: WorkdirSessionHandle,
    _leases: Vec<WorkdirDelegation>,
}

impl std::fmt::Debug for AppliedWorkdirDelegation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppliedWorkdirDelegation")
            .field("workdir", self.scoped_session.workdir())
            .field("lease_count", &self._leases.len())
            .finish()
    }
}

pub async fn apply_delegation_chain(
    source: WorkdirSessionHandle,
    requests: impl IntoIterator<Item = WorkdirDelegationRequest>,
) -> Result<AppliedWorkdirDelegation, WorkdirError> {
    let mut current = source;
    let mut leases = Vec::new();
    for request in requests {
        let authority = if current.is_delegation_capable() {
            current.clone()
        } else {
            delegation_capable_session(current.clone())
        };
        let lease = authority.delegate(request).await?;
        current = lease.scoped_session.clone();
        leases.push(lease);
    }
    Ok(AppliedWorkdirDelegation {
        scoped_session: current,
        _leases: leases,
    })
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
    rules: Vec<WorkdirDelegationRule>,
}

struct DelegatingWorkdirSession {
    source: WorkdirSessionHandle,
    cwd: FsPath,
    scope: Option<Vec<WorkdirDelegationRule>>,
    capabilities: WorkdirSessionCapabilities,
    validity: Arc<SessionValidity>,
    child_write_leases: Mutex<HashMap<u64, ActiveWriteLease>>,
    next_lease_id: AtomicU64,
    closes_source: bool,
}

impl std::fmt::Debug for DelegatingWorkdirSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegatingWorkdirSession")
            .field("workdir", &self.source.workdir())
            .field("scope", &self.scope)
            .field("capabilities", &self.capabilities)
            .field("active", &self.validity.is_active())
            .finish_non_exhaustive()
    }
}

/// Wrap a provider session with logical-path delegation and parent write gates.
pub fn delegation_capable_session(source: WorkdirSessionHandle) -> WorkdirSessionHandle {
    let capabilities = source.capabilities();
    Arc::new(DelegatingWorkdirSession {
        source,
        cwd: FsPath::new("").expect("empty Workdir path is valid"),
        scope: None,
        capabilities,
        validity: SessionValidity::root(),
        child_write_leases: Mutex::new(HashMap::new()),
        next_lease_id: AtomicU64::new(1),
        closes_source: true,
    })
}

impl DelegatingWorkdirSession {
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
                "delegated workdir session does not permit {operation}"
            )))
        }
    }

    fn ensure_path(
        &self,
        path: &FsPath,
        permission: WorkdirDelegationPermission,
    ) -> Result<(), WorkdirError> {
        self.ensure_active()?;
        if let Some(scope) = &self.scope {
            if !scope
                .iter()
                .any(|rule| rule_allows_path(rule, path, permission))
            {
                return Err(WorkdirError::Denied(format!(
                    "logical workdir path `{path}` is outside the delegated {permission:?} scope"
                )));
            }
        }
        if permission == WorkdirDelegationPermission::Write {
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
        self.ensure_path(path, WorkdirDelegationPermission::Read)
    }

    fn ensure_write(
        &self,
        path: &FsPath,
        capability: WorkdirSessionCapability,
    ) -> Result<(), WorkdirError> {
        self.ensure_capability(capability, "write operations")?;
        self.ensure_path(path, WorkdirDelegationPermission::Write)
    }

    fn ensure_command(&self) -> Result<(), WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command, "command execution")
    }

    fn ensure_parent_write_available(&self, path: &FsPath) -> Result<(), WorkdirError> {
        let mut leases = self
            .child_write_leases
            .lock()
            .expect("workdir delegation lease mutex poisoned");
        leases.retain(|_, lease| lease.validity.upgrade().is_some_and(|v| v.is_active()));
        if leases.values().any(|lease| {
            lease.rules.iter().any(|rule| {
                rule.permission == WorkdirDelegationPermission::Write
                    && rule_allows_path(rule, path, WorkdirDelegationPermission::Write)
            })
        }) {
            Err(WorkdirError::Denied(format!(
                "logical workdir path `{path}` is leased to a child session"
            )))
        } else {
            Ok(())
        }
    }

    fn validate_delegation_rules(
        &self,
        rules: &[WorkdirDelegationRule],
    ) -> Result<WorkdirSessionCapabilities, WorkdirError> {
        self.ensure_active()?;
        if rules.is_empty() {
            return Err(WorkdirError::Denied(
                "workdir delegation requires at least one logical scope rule".into(),
            ));
        }
        let writable = rules
            .iter()
            .any(|rule| rule.permission == WorkdirDelegationPermission::Write);
        if !self.capabilities.supports(WorkdirSessionCapability::Read)
            || (writable
                && (!self.capabilities.supports(WorkdirSessionCapability::Write)
                    || !self.capabilities.supports(WorkdirSessionCapability::Edit)
                    || !self
                        .capabilities
                        .supports(WorkdirSessionCapability::Command)))
        {
            return Err(WorkdirError::Denied(
                "parent workdir session cannot delegate the requested capabilities".into(),
            ));
        }
        for requested in rules {
            if let Some(scope) = &self.scope {
                if !scope
                    .iter()
                    .any(|parent| rule_contains_rule(parent, requested))
                {
                    return Err(WorkdirError::Denied(format!(
                        "logical workdir scope `{}` exceeds the parent delegation",
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
            delegated.push(WorkdirSessionCapability::Command);
        }
        Ok(WorkdirSessionCapabilities::from_capabilities(delegated))
    }
}

#[async_trait]
impl WorkdirSession for DelegatingWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.source.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.capabilities
    }

    fn is_delegation_capable(&self) -> bool {
        true
    }

    fn transports_delegation_context(&self) -> bool {
        self.source.transports_delegation_context()
    }

    async fn capture_delegation_source(
        &self,
        request: &WorkdirDelegationRequest,
    ) -> Result<WorkdirSessionHandle, WorkdirError> {
        self.ensure_active()?;
        if self.scope.is_some() {
            return Err(WorkdirError::Denied(
                "scoped Workdir sessions cannot expose their provider source".into(),
            ));
        }
        self.source.capture_delegation_source(request).await
    }

    async fn delegate(
        &self,
        request: WorkdirDelegationRequest,
    ) -> Result<WorkdirDelegation, WorkdirError> {
        let capabilities = self.validate_delegation_rules(&request.rules)?;
        if !request
            .rules
            .iter()
            .any(|rule| rule_allows_path(rule, &request.cwd, WorkdirDelegationPermission::Read))
        {
            return Err(WorkdirError::Denied(format!(
                "delegated cwd `{}` is outside the delegated readable scope",
                request.cwd
            )));
        }
        let source = self.source.capture_delegation_source(&request).await?;
        let validity = SessionValidity::child(self.validity.clone());
        let id = self.next_lease_id.fetch_add(1, Ordering::Relaxed);
        if request
            .rules
            .iter()
            .any(|rule| rule.permission == WorkdirDelegationPermission::Write)
        {
            self.child_write_leases
                .lock()
                .expect("workdir delegation lease mutex poisoned")
                .insert(
                    id,
                    ActiveWriteLease {
                        validity: Arc::downgrade(&validity),
                        rules: request.rules.clone(),
                    },
                );
        }
        let child: WorkdirSessionHandle = Arc::new(DelegatingWorkdirSession {
            source,
            cwd: request.cwd,
            scope: Some(request.rules),
            capabilities,
            validity: validity.clone(),
            child_write_leases: Mutex::new(HashMap::new()),
            next_lease_id: AtomicU64::new(1),
            closes_source: false,
        });
        let scoped_session: WorkdirSessionHandle =
            if capabilities == WorkdirSessionCapabilities::READ_ONLY {
                Arc::new(ReadOnlyWorkdirSession::new(child))
            } else {
                child
            };
        Ok(WorkdirDelegation {
            scoped_session,
            capabilities,
            validity,
        })
    }

    async fn stat(&self, mut request: StatRequest) -> Result<StatResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.stat(request).await
    }

    async fn read(&self, mut request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.read(request).await
    }

    async fn write(&self, mut request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_write(&path, WorkdirSessionCapability::Write)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.write(request).await
    }

    async fn edit(&self, mut request: EditRequest) -> Result<EditResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_write(&path, WorkdirSessionCapability::Edit)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.edit(request).await
    }

    async fn list(&self, mut request: ListRequest) -> Result<ListResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_read(&path, WorkdirSessionCapability::Read)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.list(request).await
    }

    async fn glob(&self, mut request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_read(&path, WorkdirSessionCapability::Glob)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.glob(request).await
    }

    async fn grep(&self, mut request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        let path = self.resolve_path(&request.path)?;
        self.ensure_read(&path, WorkdirSessionCapability::Grep)?;
        if !self.source.transports_delegation_context() {
            request.path = path;
        }
        self.source.grep(request).await
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.ensure_command()?;
        self.source.start_command(request).await
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.ensure_command()?;
        self.source.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.ensure_command()?;
        self.source.command_output(request).await
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_command()?;
        self.source.cancel_command(handle).await
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        self.ensure_capability(WorkdirSessionCapability::Command, "command observation")
            .ok()?;
        self.source.subscribe_command_events()
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        if self
            .ensure_capability(WorkdirSessionCapability::Command, "command observation")
            .is_err()
        {
            return Vec::new();
        }
        self.source.command_snapshot()
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

/// A fail-closed read-only view over an already scoped delegated session.
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

    fn is_delegation_capable(&self) -> bool {
        true
    }

    fn transports_delegation_context(&self) -> bool {
        self.inner.transports_delegation_context()
    }

    async fn delegate(
        &self,
        request: WorkdirDelegationRequest,
    ) -> Result<WorkdirDelegation, WorkdirError> {
        if request
            .rules
            .iter()
            .any(|rule| rule.permission == WorkdirDelegationPermission::Write)
        {
            return Err(WorkdirError::Denied(
                "read-only workdir session cannot delegate write access".into(),
            ));
        }
        self.inner.delegate(request).await
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

fn rule_allows_path(
    rule: &WorkdirDelegationRule,
    path: &FsPath,
    required: WorkdirDelegationPermission,
) -> bool {
    if required == WorkdirDelegationPermission::Write
        && rule.permission != WorkdirDelegationPermission::Write
    {
        return false;
    }
    path_in_rule(rule, path)
}

fn path_in_rule(rule: &WorkdirDelegationRule, path: &FsPath) -> bool {
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

fn rule_contains_rule(parent: &WorkdirDelegationRule, child: &WorkdirDelegationRule) -> bool {
    if child.permission == WorkdirDelegationPermission::Write
        && parent.permission != WorkdirDelegationPermission::Write
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

    fn session(root: &Path) -> WorkdirSessionHandle {
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
        delegation_capable_session(Arc::new(LocalWorkdirSession::materialized_bound(
            Workdir::new("delegation-test"),
            root.to_path_buf(),
            root.to_path_buf(),
            scope,
            WorkdirSessionCapabilities::ALL,
        )))
    }

    fn request(path: &str, permission: WorkdirDelegationPermission) -> WorkdirDelegationRequest {
        WorkdirDelegationRequest {
            rules: vec![WorkdirDelegationRule {
                target: fs_path(path),
                permission,
                recursive: true,
            }],
            cwd: fs_path(path),
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
    async fn delegation_capable_session_forwards_command_telemetry() {
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

    #[test]
    fn non_recursive_rule_covers_target_and_direct_children_only() {
        let rule = WorkdirDelegationRule {
            target: fs_path("docs"),
            permission: WorkdirDelegationPermission::Read,
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
            .delegate(request("docs", WorkdirDelegationPermission::Read))
            .await
            .unwrap();
        assert_eq!(child.capabilities, WorkdirSessionCapabilities::READ_ONLY);
        assert_eq!(
            child
                .scoped_session
                .read(read("readme.md"))
                .await
                .unwrap()
                .bytes,
            b"visible"
        );
        assert!(matches!(
            child.scoped_session.write(write("new.md", "no")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(
            !child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        assert!(child.scoped_session.subscribe_command_events().is_none());
        assert!(child.scoped_session.command_snapshot().is_empty());
        assert!(matches!(
            child
                .scoped_session
                .start_command(CommandRequest {
                    command: "printf denied".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
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
            .delegate(request("granted", WorkdirDelegationPermission::Read))
            .await
            .unwrap();

        let result = child.scoped_session.read(read("link")).await;
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
            .delegate(request("granted", WorkdirDelegationPermission::Write))
            .await
            .unwrap();

        let result = child
            .scoped_session
            .write(write("outside/new", "forbidden"))
            .await;
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
                .delegate(request(
                    "granted/outside",
                    WorkdirDelegationPermission::Write
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
            .delegate(request("leased", WorkdirDelegationPermission::Write))
            .await
            .unwrap();
        assert!(
            child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        let child_output = run_command(
            &child.scoped_session,
            "printf child-command",
            "delegated-child-command",
        )
        .await;
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
        child
            .scoped_session
            .write(write("file", "child"))
            .await
            .unwrap();
        child.release();
        assert!(matches!(
            child
                .scoped_session
                .start_command(CommandRequest {
                    command: "printf revoked".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
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
            child.scoped_session.read(read("file")).await,
            Err(WorkdirError::SessionClosed)
        ));
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
            .delegate(request("docs", WorkdirDelegationPermission::Read))
            .await
            .unwrap();
        let nested = child
            .scoped_session
            .delegate(request("docs/sub", WorkdirDelegationPermission::Read))
            .await
            .unwrap();

        nested.scoped_session.read(read("a")).await.unwrap();
        assert!(
            child
                .scoped_session
                .delegate(request("other", WorkdirDelegationPermission::Read))
                .await
                .is_err()
        );
        assert!(
            child
                .scoped_session
                .delegate(request("docs/sub", WorkdirDelegationPermission::Write))
                .await
                .is_err()
        );

        child.release();
        assert!(matches!(
            nested.scoped_session.read(read("a")).await,
            Err(WorkdirError::SessionClosed)
        ));
    }

    #[tokio::test]
    async fn nested_write_leases_do_not_block_command_capable_ancestors() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs/sub")).unwrap();
        let root_session = session(root.path());
        let child = root_session
            .delegate(request("docs", WorkdirDelegationPermission::Write))
            .await
            .unwrap();
        let nested = child
            .scoped_session
            .delegate(request("docs/sub", WorkdirDelegationPermission::Write))
            .await
            .unwrap();

        for (session, label) in [
            (&root_session, "root"),
            (&child.scoped_session, "child"),
            (&nested.scoped_session, "nested"),
        ] {
            let output = run_command(
                session,
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
            child
                .scoped_session
                .write(write("sub/child", "blocked"))
                .await,
            Err(WorkdirError::Denied(_))
        ));
        nested
            .scoped_session
            .write(write("nested", "allowed"))
            .await
            .unwrap();

        nested.release();
        child.release();
    }

    #[tokio::test]
    async fn reapplied_write_delegation_chain_forwards_command_lifecycle() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("delegated")).unwrap();
        let applied = apply_delegation_chain(
            session(root.path()),
            [request("delegated", WorkdirDelegationPermission::Write)],
        )
        .await
        .unwrap();

        let output = run_command(
            &applied.scoped_session,
            "printf reapplied",
            "reapplied-command",
        )
        .await;
        assert_eq!(output.status, CommandStatus::Completed);
        assert_eq!(output.content, "reapplied");
    }

    #[tokio::test]
    async fn applied_chain_cannot_replace_outer_provider_attenuation() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("outer")).unwrap();
        fs::create_dir_all(root.path().join("outside")).unwrap();
        let result = apply_delegation_chain(
            Arc::new(LocalWorkdirSession::materialized_bound(
                Workdir::new("delegation-chain-test"),
                root.path().to_path_buf(),
                root.path().to_path_buf(),
                SharedScope::new(
                    Scope::from_config(&ScopeConfig {
                        allow: vec![ScopeRule {
                            target: root.path().to_path_buf(),
                            permission: Permission::Write,
                            recursive: true,
                        }],
                        deny: Vec::new(),
                    })
                    .unwrap(),
                ),
                WorkdirSessionCapabilities::ALL,
            )),
            [
                request("outer", WorkdirDelegationPermission::Read),
                request("outside", WorkdirDelegationPermission::Read),
            ],
        )
        .await;
        assert!(matches!(result, Err(WorkdirError::Denied(_))));
    }

    #[tokio::test]
    async fn closing_parent_invalidates_delegated_sessions() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("docs")).unwrap();
        fs::write(root.path().join("docs/a"), "a").unwrap();
        let parent = session(root.path());
        let child = parent
            .delegate(request("docs", WorkdirDelegationPermission::Read))
            .await
            .unwrap();

        parent.close().await.unwrap();
        assert!(matches!(
            parent
                .start_command(CommandRequest {
                    command: "printf closed".into(),
                    timeout_secs: 5,
                    output_limit: 1024,
                    spill_dir: None,
                    tool_call_id: Some("closed-parent-command".into()),
                })
                .await,
            Err(WorkdirError::SessionClosed)
        ));
        assert!(matches!(
            child.scoped_session.read(read("a")).await,
            Err(WorkdirError::SessionClosed)
        ));
    }
}
