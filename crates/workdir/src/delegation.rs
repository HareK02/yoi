use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_trait::async_trait;
use fs_operation::{
    EditRequest, EditResult, FsPath, GlobRequest, GlobResult, GrepRequest, GrepResult, ListRequest,
    ListResult, ReadRequest, ReadResult, StatRequest, StatResult, WriteRequest, WriteResult,
};

use crate::{
    CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest, CommandStatus, Workdir,
    WorkdirError, WorkdirSession, WorkdirSessionCapabilities, WorkdirSessionCapability,
    WorkdirSessionHandle,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkdirDelegationPermission {
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkdirDelegationRule {
    pub target: FsPath,
    pub permission: WorkdirDelegationPermission,
    pub recursive: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

    fn ensure_command(&self, starting: bool) -> Result<(), WorkdirError> {
        self.ensure_capability(WorkdirSessionCapability::Command, "command execution")?;
        if starting && self.has_active_write_lease() {
            return Err(WorkdirError::Denied(
                "command execution is denied while a child holds a write delegation".into(),
            ));
        }
        Ok(())
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

    fn has_active_write_lease(&self) -> bool {
        let mut leases = self
            .child_write_leases
            .lock()
            .expect("workdir delegation lease mutex poisoned");
        leases.retain(|_, lease| lease.validity.upgrade().is_some_and(|v| v.is_active()));
        leases.values().any(|lease| {
            lease
                .rules
                .iter()
                .any(|rule| rule.permission == WorkdirDelegationPermission::Write)
        })
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
                    || !self.capabilities.supports(WorkdirSessionCapability::Edit)))
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

    async fn capture_delegation_source(&self) -> Result<WorkdirSessionHandle, WorkdirError> {
        self.ensure_active()?;
        if self.scope.is_some() {
            return Err(WorkdirError::Denied(
                "scoped Workdir sessions cannot expose their provider source".into(),
            ));
        }
        self.source.capture_delegation_source().await
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
        let source = self.source.capture_delegation_source().await?;
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

    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError> {
        self.ensure_read(&request.path, WorkdirSessionCapability::Read)?;
        self.source.stat(request).await
    }

    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.ensure_read(&request.path, WorkdirSessionCapability::Read)?;
        self.source.read(request).await
    }

    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.ensure_write(&request.path, WorkdirSessionCapability::Write)?;
        self.source.write(request).await
    }

    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError> {
        self.ensure_write(&request.path, WorkdirSessionCapability::Edit)?;
        self.source.edit(request).await
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        self.ensure_read(&request.path, WorkdirSessionCapability::Read)?;
        self.source.list(request).await
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.ensure_read(&request.path, WorkdirSessionCapability::Glob)?;
        self.source.glob(request).await
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.ensure_read(&request.path, WorkdirSessionCapability::Grep)?;
        self.source.grep(request).await
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        self.ensure_command(true)?;
        self.source.start_command(request).await
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        self.ensure_command(false)?;
        self.source.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.ensure_command(false)?;
        self.source.command_output(request).await
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.ensure_command(false)?;
        self.source.cancel_command(handle).await
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
    parent.recursive || !child.recursive
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
    async fn read_only_delegation_allows_prefix_and_denies_siblings_and_mutation() {
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
                .read(read("docs/readme.md"))
                .await
                .unwrap()
                .bytes,
            b"visible"
        );
        assert!(matches!(
            child.scoped_session.read(read("secret/key")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(matches!(
            child.scoped_session.write(write("docs/new.md", "no")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(
            !child
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
    }

    #[tokio::test]
    async fn write_lease_blocks_parent_region_until_release() {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("leased")).unwrap();
        fs::create_dir_all(root.path().join("other")).unwrap();
        let parent = session(root.path());
        let child = parent
            .delegate(request("leased", WorkdirDelegationPermission::Write))
            .await
            .unwrap();

        assert!(matches!(
            parent.write(write("leased/file", "parent")).await,
            Err(WorkdirError::Denied(_))
        ));
        parent.write(write("other/file", "parent")).await.unwrap();
        child
            .scoped_session
            .write(write("leased/file", "child"))
            .await
            .unwrap();
        child.release();
        parent
            .write(write("leased/parent", "parent"))
            .await
            .unwrap();
        assert!(matches!(
            child.scoped_session.read(read("leased/file")).await,
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

        nested
            .scoped_session
            .read(read("docs/sub/a"))
            .await
            .unwrap();
        assert!(matches!(
            nested.scoped_session.read(read("docs/peer/b")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(
            child
                .scoped_session
                .delegate(request("docs/sub", WorkdirDelegationPermission::Write))
                .await
                .is_err()
        );

        child.release();
        assert!(matches!(
            nested.scoped_session.read(read("docs/sub/a")).await,
            Err(WorkdirError::SessionClosed)
        ));
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
            child.scoped_session.read(read("docs/a")).await,
            Err(WorkdirError::SessionClosed)
        ));
    }
}
