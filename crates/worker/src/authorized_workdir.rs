//! Live Host authorization for trusted Features using Runtime-local Workdir sessions.
//!
//! Point access uses provider-resolved leases; traversals use provider scope layers.
//! This adapter adds no Tool executor or shell sandbox and retains provider ceilings.

use std::{future::Future, path::Path, sync::Arc};

use async_trait::async_trait;
use server_api::authorization::PermissionResource;
use tokio::sync::{OnceCell, broadcast};
use workdir::{
    CheckoutObservation, CheckoutOperation, CheckoutRequest, CheckoutResult,
    CheckoutSearchOperation, CheckoutSearchRequest, CheckoutSearchResult, CommandEvent,
    CommandHandle, CommandOutput, CommandOutputRequest, CommandRequest, CommandSnapshot,
    CommandStatus, EditRequest, EditResult, GlobRequest, GlobResult, GrepRequest, GrepResult,
    ListRequest, ListResult, ReadBytesRequest, ReadBytesResult, ReadRequest, ReadResult,
    StatRequest, StatResult, Workdir, WorkdirError, WorkdirPath, WorkdirScopeAuthorizationRequest,
    WorkdirScopeOverlapRequest, WorkdirSession, WorkdirSessionCapabilities, WorkdirSessionHandle,
    WorkdirToolBroker, WorkdirToolScope, WorkdirToolScopePermission, WorkdirToolScopeRule,
    WriteRequest, WriteResult,
};

use crate::WorkspaceClient;

const READ: &str = "filesystem.read";
const WRITE: &str = "filesystem.write";
const COMMAND: &str = "bash.execute";

/// Bind the authority-assigned attachment identity, never a local provider ID or alias.
/// Only explicitly standalone clients bypass Host authorization; an unavailable bound
/// transport must return an authorization error instead of becoming standalone.
pub fn authorized_workdir_session(
    session: WorkdirSessionHandle,
    client: Arc<dyn WorkspaceClient>,
    working_directory_id: String,
) -> WorkdirSessionHandle {
    if !client.is_available() && client.workspace_id().is_none() && client.kind() == "unavailable" {
        return session;
    }
    Arc::new(AuthorizedWorkdirSession {
        session,
        client,
        working_directory_id,
        scope_broker: OnceCell::new(),
    })
}

#[derive(Debug)]
struct AuthorizedWorkdirSession {
    session: WorkdirSessionHandle,
    client: Arc<dyn WorkspaceClient>,
    working_directory_id: String,
    scope_broker: OnceCell<WorkdirToolBroker>,
}

impl AuthorizedWorkdirSession {
    fn resource(
        &self,
        path: &WorkdirPath,
        recursive: bool,
    ) -> Result<PermissionResource, WorkdirError> {
        let normalized = WorkdirPath::new(path.as_str()).map_err(|_| {
            WorkdirError::Denied("Host authorization requires attachment-relative paths".into())
        })?;
        let resource = PermissionResource::Workdir {
            working_directory_id: self.working_directory_id.clone(),
            path: normalized.to_string(),
            recursive,
        };
        resource.validate().map_err(|error| {
            WorkdirError::Denied(format!("invalid Workdir authorization resource: {error}").into())
        })?;
        Ok(resource)
    }

    fn authorize(
        &self,
        operation: &str,
        path: &WorkdirPath,
        recursive: bool,
    ) -> Result<(), WorkdirError> {
        self.client
            .authorize_operation(operation, &self.resource(path, recursive)?)
            .map_err(authorization_error)
    }

    fn authorized_scope(
        &self,
        operation: &str,
        path: &WorkdirPath,
        recursive: bool,
    ) -> Result<Option<WorkdirToolScopeRule>, WorkdirError> {
        let requested = self.resource(path, recursive)?;
        let Some(granted) = self
            .client
            .authorize_operation_scope(operation, &requested)
            .map_err(authorization_error)?
        else {
            // Only the Host may choose the explicit non-Subject legacy contract.
            return Ok(None);
        };
        if !granted.covers(&requested) {
            return Err(WorkdirError::Denied(
                "Host returned a scope that does not cover the operation resource".into(),
            ));
        }
        let PermissionResource::Workdir {
            path, recursive, ..
        } = granted
        else {
            return Err(WorkdirError::Denied(
                "Host returned a non-Workdir scope".into(),
            ));
        };
        Ok(Some(WorkdirToolScopeRule {
            target: WorkdirPath::new(path)?,
            recursive,
            permission: match operation {
                READ => WorkdirToolScopePermission::Read,
                WRITE => WorkdirToolScopePermission::Write,
                _ => {
                    return Err(WorkdirError::Denied(
                        "operation has no filesystem scope mapping".into(),
                    ));
                }
            },
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        }))
    }

    async fn with_authorized<T, F, Fut>(
        &self,
        operation: &str,
        path: &WorkdirPath,
        recursive: bool,
        effect: F,
    ) -> Result<T, WorkdirError>
    where
        F: FnOnce(WorkdirSessionHandle, WorkdirPath, Option<WorkdirToolScopeRule>) -> Fut,
        Fut: Future<Output = Result<T, WorkdirError>>,
    {
        let Some(rule) = self.authorized_scope(operation, path, recursive)? else {
            return effect(self.session.clone(), WorkdirPath::root(), None).await;
        };
        let broker = self
            .scope_broker
            .get_or_init(|| async { WorkdirToolBroker::new(self.session.clone()) })
            .await;
        // Resolve against the granted prefix, not the requested path: a symlink
        // must not turn its target into the root of its own authorization.
        let lease = broker
            .scope(WorkdirToolScope {
                rules: vec![rule.clone()],
                cwd: rule.target.clone(),
                command: false,
            })
            .await?;
        let result = effect(lease.tool_session(), rule.target.clone(), Some(rule)).await;
        // Close only the attenuated lease. Source hash history, scope, command
        // ownership and attachment lifetime survive separate operations.
        match (result, lease.close().await) {
            (result, Ok(())) => result,
            (Ok(_), Err(cleanup)) => Err(WorkdirError::OutcomeUnknown(format!(
                "operation completed but its authorization lease cleanup failed: {cleanup}"
            ))),
            (Err(operation), Err(cleanup)) => Err(WorkdirError::OutcomeUnknown(format!(
                "operation failed: {operation}; authorization lease cleanup also failed: {cleanup}"
            ))),
        }
    }

    fn require_legacy(&self, operation: &str) -> Result<(), WorkdirError> {
        if self
            .authorized_scope(operation, &WorkdirPath::root(), true)?
            .is_some()
        {
            return Err(WorkdirError::Denied(
                "operation has no complete scoped resource mapping".into(),
            ));
        }
        Ok(())
    }

    async fn authorized_search(
        &self,
        operation: CheckoutSearchOperation,
    ) -> Result<CheckoutSearchResult, WorkdirError> {
        let path = operation.path().clone();
        self.with_authorized(READ, &path, true, |scoped, root, rule| async move {
            let Some(rule) = rule else {
                return match operation {
                    CheckoutSearchOperation::List(request) => self
                        .session
                        .list(request)
                        .await
                        .map(CheckoutSearchResult::List),
                    CheckoutSearchOperation::Glob(request) => self
                        .session
                        .glob(request)
                        .await
                        .map(CheckoutSearchResult::Glob),
                    CheckoutSearchOperation::Grep(request) => self
                        .session
                        .grep(request)
                        .await
                        .map(CheckoutSearchResult::Grep),
                };
            };
            self.search_with_scope(scoped, root, rule, CheckoutSearchRequest::new(operation))
                .await
        })
        .await
    }

    async fn search_with_scope(
        &self,
        scoped: WorkdirSessionHandle,
        root: WorkdirPath,
        rule: WorkdirToolScopeRule,
        mut request: CheckoutSearchRequest,
    ) -> Result<CheckoutSearchResult, WorkdirError> {
        scoped
            .authorize_scope_path(WorkdirScopeAuthorizationRequest {
                rules: vec![WorkdirToolScopeRule {
                    target: WorkdirPath::root(),
                    ..rule.clone()
                }],
                path: relative_path(&root, request.operation.path())?,
                permission: WorkdirToolScopePermission::Read,
            })
            .await?;
        // Preserve the caller's coordinate root, cursors and all existing
        // attenuation layers. The provider applies the Host ceiling during
        // traversal, not by filtering already-rendered filenames afterward.
        request.scope_layers.push(vec![rule]);
        self.session.checkout_search(request).await
    }

    fn authorize_command(&self) -> Result<(), WorkdirError> {
        // cwd is not a sandbox: arbitrary shell can access the entire attachment.
        self.authorize(COMMAND, &WorkdirPath::root(), true)
    }
}

fn authorization_error(error: crate::WorkspaceClientError) -> WorkdirError {
    WorkdirError::Denied(format!("Workdir Host authorization failed: {error}").into())
}

fn relative_path(root: &WorkdirPath, path: &WorkdirPath) -> Result<WorkdirPath, WorkdirError> {
    let relative = Path::new(path.as_str())
        .strip_prefix(root.as_str())
        .map_err(|_| {
            WorkdirError::Denied("operation path exceeds the granted provider scope".into())
        })?;
    WorkdirPath::new(
        relative
            .to_str()
            .ok_or_else(|| WorkdirError::InvalidPath("non-UTF8 scoped path".into()))?,
    )
    .map_err(WorkdirError::from)
}

#[async_trait]
impl WorkdirSession for AuthorizedWorkdirSession {
    fn workdir(&self) -> &Workdir {
        self.session.workdir()
    }

    fn capabilities(&self) -> WorkdirSessionCapabilities {
        // This is the provider ceiling, not a cached live authorization decision.
        self.session.capabilities()
    }

    fn environment(&self) -> workdir::WorkdirSessionEnvironment {
        let mut environment = self.session.environment();
        environment.capabilities = environment.capabilities.intersection(self.capabilities());
        environment
    }

    async fn authorize_scope_path(
        &self,
        mut request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(
            match request.permission {
                WorkdirToolScopePermission::Read => READ,
                WorkdirToolScopePermission::Write => WRITE,
            },
            &path,
            false,
            |session, root, _| async move {
                request.path = relative_path(&root, &request.path)?;
                for rule in &mut request.rules {
                    rule.target = relative_path(&root, &rule.target)?;
                }
                session.authorize_scope_path(request).await
            },
        )
        .await
    }

    async fn scope_rules_overlap(
        &self,
        request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        self.require_legacy(READ)?;
        self.session.scope_rules_overlap(request).await
    }

    async fn checkout_search(
        &self,
        request: CheckoutSearchRequest,
    ) -> Result<CheckoutSearchResult, WorkdirError> {
        let path = request.operation.path().clone();
        self.with_authorized(READ, &path, true, |scoped, root, rule| async move {
            match rule {
                Some(rule) => self.search_with_scope(scoped, root, rule, request).await,
                None => self.session.checkout_search(request).await,
            }
        })
        .await
    }

    async fn checkout_observe(
        &self,
        path: WorkdirPath,
    ) -> Result<CheckoutObservation, WorkdirError> {
        self.require_legacy(READ)?;
        self.session.checkout_observe(path).await
    }

    async fn checkout_execute(
        &self,
        request: CheckoutRequest,
    ) -> Result<CheckoutResult, WorkdirError> {
        self.require_legacy(match &request.operation {
            CheckoutOperation::Read { .. } | CheckoutOperation::ReadBytes { .. } => READ,
            CheckoutOperation::Write { .. }
            | CheckoutOperation::Edit { .. }
            | CheckoutOperation::Create { .. } => WRITE,
        })?;
        self.session.checkout_execute(request).await
    }

    async fn stat(&self, mut request: StatRequest) -> Result<StatResult, WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(READ, &path, false, |session, root, _| async move {
            let original = request.path.clone();
            request.path = relative_path(&root, &request.path)?;
            let mut result = session.stat(request).await?;
            result.path = original;
            Ok(result)
        })
        .await
    }

    async fn read(&self, mut request: ReadRequest) -> Result<ReadResult, WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(READ, &path, false, |session, root, _| async move {
            let original = request.path.clone();
            request.path = relative_path(&root, &request.path)?;
            let mut result = session.read(request).await?;
            result.path = original;
            Ok(result)
        })
        .await
    }

    async fn read_bytes(
        &self,
        mut request: ReadBytesRequest,
    ) -> Result<ReadBytesResult, WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(READ, &path, false, |session, root, _| async move {
            let original = request.path.clone();
            request.path = relative_path(&root, &request.path)?;
            let mut result = session.read_bytes(request).await?;
            result.path = original;
            Ok(result)
        })
        .await
    }

    async fn write(&self, mut request: WriteRequest) -> Result<WriteResult, WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(WRITE, &path, false, |session, root, _| async move {
            request.path = relative_path(&root, &request.path)?;
            session.write(request).await
        })
        .await
    }

    async fn edit(&self, mut request: EditRequest) -> Result<EditResult, WorkdirError> {
        let path = request.path.clone();
        self.with_authorized(WRITE, &path, false, |session, root, _| async move {
            request.path = relative_path(&root, &request.path)?;
            session.edit(request).await
        })
        .await
    }

    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError> {
        match self
            .authorized_search(CheckoutSearchOperation::List(request))
            .await?
        {
            CheckoutSearchResult::List(result) => Ok(result),
            _ => Err(WorkdirError::Unavailable(
                "mismatched scoped List result".into(),
            )),
        }
    }

    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError> {
        match self
            .authorized_search(CheckoutSearchOperation::Glob(request))
            .await?
        {
            CheckoutSearchResult::Glob(result) => Ok(result),
            _ => Err(WorkdirError::Unavailable(
                "mismatched scoped Glob result".into(),
            )),
        }
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError> {
        match self
            .authorized_search(CheckoutSearchOperation::Grep(request))
            .await?
        {
            CheckoutSearchResult::Grep(result) => Ok(result),
            _ => Err(WorkdirError::Unavailable(
                "mismatched scoped Grep result".into(),
            )),
        }
    }

    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        // Validate coordinates without treating cwd as the command's access scope.
        WorkdirPath::new(request.cwd.as_str()).map_err(WorkdirError::from)?;
        self.authorize_command()?;
        self.session.start_command(request).await
    }

    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        // Reconciliation of an owned effect is not new execution authority.
        self.session.command_status(handle).await
    }

    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError> {
        self.authorize_command()?;
        self.session.command_output(request).await
    }

    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError> {
        self.session.cancel_command(handle).await
    }

    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let mut source = self.session.subscribe_command_events()?;
        let (sender, receiver) = broadcast::channel(64);
        let client = self.client.clone();
        let resource = PermissionResource::Workdir {
            working_directory_id: self.working_directory_id.clone(),
            path: ".".into(),
            recursive: true,
        };
        // A subscription must not freeze the grant at subscription time. Check
        // each new output access; terminal/status events remain usable for cleanup.
        runtime.spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = sender.closed() => break,
                    event = source.recv() => match event {
                        Ok(event) => event,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                };
                if matches!(event, CommandEvent::Output { .. })
                    && client.authorize_operation(COMMAND, &resource).is_err()
                {
                    continue;
                }
                if sender.send(event).is_err() {
                    break;
                }
            }
        });
        Some(receiver)
    }

    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        if self.authorize_command().is_err() {
            return Vec::new();
        }
        self.session.command_snapshot()
    }

    async fn close(&self) -> Result<(), WorkdirError> {
        self.session.close().await
    }
}

#[cfg(test)]
mod tests {
    //! Component tests connect this adapter to the real LocalWorkdirSession.
    //! Only the Host authorization transport/policy is replaced; these are not E2E tests.

    mod environment;

    use super::*;
    use crate::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};
    use server_api::authorization::ScopedPermission;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    use tempfile::TempDir;
    use workdir::LocalWorkdirSession;

    #[derive(Debug)]
    struct AuthorizationClient {
        legacy: bool,
        revoked: AtomicBool,
        requests: Mutex<Vec<ScopedPermission>>,
        permissions: Vec<ScopedPermission>,
    }

    impl AuthorizationClient {
        fn new(permissions: Vec<ScopedPermission>) -> Arc<Self> {
            Arc::new(Self {
                legacy: false,
                revoked: AtomicBool::new(false),
                requests: Mutex::new(Vec::new()),
                permissions,
            })
        }
    }

    impl WorkspaceClient for AuthorizationClient {
        fn workspace_id(&self) -> Option<&str> {
            Some("workspace")
        }
        fn kind(&self) -> &str {
            "mock-bound"
        }
        fn is_available(&self) -> bool {
            true
        }
        fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
            panic!("the authorization client must not execute protected effects")
        }
        fn authorize_operation(
            &self,
            operation: &str,
            resource: &PermissionResource,
        ) -> Result<(), WorkspaceClientError> {
            self.authorize_operation_scope(operation, resource)
                .map(|_| ())
        }

        fn authorize_operation_scope(
            &self,
            operation: &str,
            resource: &PermissionResource,
        ) -> Result<Option<PermissionResource>, WorkspaceClientError> {
            let request = ScopedPermission {
                operation: operation.into(),
                resource: resource.clone(),
            };
            self.requests.lock().unwrap().push(request.clone());
            if !self.revoked.load(Ordering::SeqCst) {
                if self.legacy {
                    return Ok(None);
                }
                if let Some(granted) = self
                    .permissions
                    .iter()
                    .find(|permission| permission.covers(&request))
                {
                    return Ok(Some(granted.resource.clone()));
                }
            }
            Err(WorkspaceClientError::Request(
                "denied by current grant".into(),
            ))
        }
    }

    fn permission(operation: &str, path: &str, recursive: bool) -> ScopedPermission {
        ScopedPermission {
            operation: operation.into(),
            resource: PermissionResource::Workdir {
                working_directory_id: "attachment-id".into(),
                path: path.into(),
                recursive,
            },
        }
    }

    fn session(root: &TempDir, client: Arc<dyn WorkspaceClient>) -> WorkdirSessionHandle {
        authorized_workdir_session(
            Arc::new(LocalWorkdirSession::new(
                manifest::Scope::writable(root.path()).unwrap(),
                root.path().to_path_buf(),
            )),
            client,
            "attachment-id".into(),
        )
    }

    fn read(path: &str) -> ReadRequest {
        ReadRequest {
            path: WorkdirPath::new(path).unwrap(),
            offset: 0,
            limit: 100,
            max_bytes: 4096,
        }
    }

    fn command(command: &str) -> CommandRequest {
        CommandRequest {
            command: command.into(),
            timeout_secs: 10,
            transport_byte_budget: 4096,
            cwd: WorkdirPath::root(),
            tool_call_id: None,
        }
    }

    #[tokio::test]
    async fn writes_check_current_exact_resource_and_denial_has_no_filesystem_effect() {
        let root = tempfile::tempdir().unwrap();
        let client = AuthorizationClient::new(vec![permission(WRITE, "allowed.txt", false)]);
        let session = session(&root, client.clone());
        session
            .write(WriteRequest {
                path: WorkdirPath::new("./allowed.txt").unwrap(),
                content: b"allowed".to_vec(),
                expected_hash: None,
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("allowed.txt")).unwrap(),
            b"allowed"
        );
        let denied = session
            .write(WriteRequest {
                path: WorkdirPath::new("outside/new.txt").unwrap(),
                content: b"denied".to_vec(),
                expected_hash: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(denied, WorkdirError::Denied(_)));
        assert!(!root.path().join("outside").exists());
        client.revoked.store(true, Ordering::SeqCst);
        let denied = session
            .write(WriteRequest {
                path: WorkdirPath::new("allowed.txt").unwrap(),
                content: b"revoked".to_vec(),
                expected_hash: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(denied, WorkdirError::Denied(_)));
        assert_eq!(
            std::fs::read(root.path().join("allowed.txt")).unwrap(),
            b"allowed"
        );
        assert_eq!(
            *client.requests.lock().unwrap(),
            vec![
                permission(WRITE, "allowed.txt", false),
                permission(WRITE, "outside/new.txt", false),
                permission(WRITE, "allowed.txt", false),
            ]
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn read_revocation_is_checked_again_on_the_same_local_session() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("note"), "private").unwrap();
        let client = AuthorizationClient::new(vec![permission(READ, "note", false)]);
        let session = session(&root, client.clone());
        assert_eq!(session.read(read("note")).await.unwrap().bytes, b"private");
        client.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            session.read(read("note")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert_eq!(client.requests.lock().unwrap().len(), 2);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn narrow_command_grant_does_not_treat_cwd_as_a_sandbox() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let client = AuthorizationClient::new(vec![permission(COMMAND, "src", true)]);
        let session = session(&root, client.clone());
        let mut request = command("printf denied > escaped");
        request.cwd = WorkdirPath::new("src").unwrap();
        assert!(matches!(
            session.start_command(request).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(!root.path().join("src/escaped").exists());
        assert_eq!(
            *client.requests.lock().unwrap(),
            vec![permission(COMMAND, ".", true)]
        );
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn revoked_commands_cannot_start_or_read_output_but_status_and_cancel_remain_owned() {
        let root = tempfile::tempdir().unwrap();
        let client = AuthorizationClient::new(vec![permission(COMMAND, ".", true)]);
        let session = session(&root, client.clone());
        let handle = session
            .start_command(command("exec tail -f /dev/null"))
            .await
            .unwrap();
        client.revoked.store(true, Ordering::SeqCst);
        assert!(matches!(
            session.start_command(command("printf bad > escaped")).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(!root.path().join("escaped").exists());
        assert!(matches!(
            session
                .command_output(CommandOutputRequest {
                    handle: handle.clone(),
                    cursor: 0,
                    limit: 4096,
                    wait: false,
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(session.command_snapshot().is_empty());
        let checks_before_cleanup = client.requests.lock().unwrap().len();
        assert_eq!(
            session.command_status(handle.clone()).await.unwrap(),
            CommandStatus::Running
        );
        assert!(matches!(
            session
                .cancel_command(CommandHandle("not-owned".into()))
                .await,
            Err(WorkdirError::UnknownCommand(_))
        ));
        session.cancel_command(handle.clone()).await.unwrap();
        session.close().await.unwrap();
        assert_eq!(client.requests.lock().unwrap().len(), checks_before_cleanup);
    }

    #[tokio::test]
    async fn authorization_success_does_not_expand_provider_scope() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "original").unwrap();
        let client = AuthorizationClient::new(vec![permission(WRITE, ".", true)]);
        let local = LocalWorkdirSession::new(
            manifest::Scope::from_config(&manifest::ScopeConfig {
                allow: vec![manifest::ScopeRule {
                    target: root.path().to_path_buf(),
                    permission: manifest::Permission::Read,
                    recursive: true,
                    symlink_policy: manifest::SymlinkPolicy::Resolved,
                }],
                deny: Vec::new(),
            })
            .unwrap(),
            root.path().to_path_buf(),
        );
        let session =
            authorized_workdir_session(Arc::new(local), client.clone(), "attachment-id".into());
        assert!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("file").unwrap(),
                    content: b"bad".to_vec(),
                    expected_hash: None,
                })
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"original"
        );
        assert_eq!(client.requests.lock().unwrap().len(), 1);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn unmappable_absolute_paths_are_denied_before_local_effects() {
        let root = tempfile::tempdir().unwrap();
        let client = AuthorizationClient::new(vec![
            permission(WRITE, ".", true),
            permission(COMMAND, ".", true),
        ]);
        let session = session(&root, client.clone());
        assert!(matches!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new_scoped(root.path().join("absolute").to_string_lossy())
                        .unwrap(),
                    content: b"bad".to_vec(),
                    expected_hash: None,
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(!root.path().join("absolute").exists());
        assert!(client.requests.lock().unwrap().is_empty());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn scoped_unmapped_checkout_and_overlap_are_denied_even_with_root_grants() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), "original").unwrap();
        let client = AuthorizationClient::new(vec![
            permission(READ, ".", true),
            permission(WRITE, ".", true),
        ]);
        let session = session(&root, client);
        assert!(matches!(
            session.checkout_observe(WorkdirPath::root()).await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: WorkdirPath::new("file").unwrap(),
                    validator: vec![1],
                    operation: CheckoutOperation::Write {
                        content: b"bad".to_vec(),
                        expected_hash: [0; 32]
                    },
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(matches!(
            session
                .checkout_execute(CheckoutRequest {
                    target: WorkdirPath::root(),
                    validator: vec![1],
                    operation: CheckoutOperation::Create {
                        path: WorkdirPath::new("nested/new").unwrap(),
                        content: b"bad".to_vec()
                    },
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        let rule = WorkdirToolScopeRule {
            target: WorkdirPath::root(),
            permission: WorkdirToolScopePermission::Read,
            recursive: true,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        };
        assert!(matches!(
            session
                .scope_rules_overlap(WorkdirScopeOverlapRequest {
                    left: rule.clone(),
                    right: rule
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        assert_eq!(
            std::fs::read(root.path().join("file")).unwrap(),
            b"original"
        );
        assert!(!root.path().join("nested").exists());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn explicit_legacy_authorization_forwards_checkout_operations() {
        let root = tempfile::tempdir().unwrap();
        let client = Arc::new(AuthorizationClient {
            legacy: true,
            revoked: AtomicBool::new(false),
            requests: Mutex::new(Vec::new()),
            permissions: Vec::new(),
        });
        let session = session(&root, client.clone());
        let parent = session.checkout_observe(WorkdirPath::root()).await.unwrap();
        session
            .checkout_execute(CheckoutRequest {
                target: parent.path,
                validator: parent.validator,
                operation: CheckoutOperation::Create {
                    path: WorkdirPath::new("nested/new").unwrap(),
                    content: b"legacy".to_vec(),
                },
            })
            .await
            .unwrap();
        assert_eq!(
            session.read(read("nested/new")).await.unwrap().bytes,
            b"legacy"
        );
        assert_eq!(client.requests.lock().unwrap().len(), 3);
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn scoped_read_then_write_keeps_provider_hash_state_and_attachment_paths() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/file"), "original").unwrap();
        let client = AuthorizationClient::new(vec![
            permission(READ, "src", true),
            permission(WRITE, "src", true),
        ]);
        let session = session(&root, client);
        let original = session.read(read("src/file")).await.unwrap();
        assert_eq!(original.path.as_str(), "src/file");
        session
            .write(WriteRequest {
                path: WorkdirPath::new("src/file").unwrap(),
                content: b"replacement".to_vec(),
                expected_hash: Some(original.content_hash),
            })
            .await
            .unwrap();
        let replacement = session.read(read("src/file")).await.unwrap();
        assert_eq!(replacement.bytes, b"replacement");
        assert!(matches!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("src/file").unwrap(),
                    content: b"stale".to_vec(),
                    expected_hash: Some(original.content_hash),
                })
                .await,
            Err(WorkdirError::Conflict(_))
        ));
        session
            .edit(EditRequest {
                path: WorkdirPath::new("src/file").unwrap(),
                old_string: "replacement".into(),
                new_string: "edited".into(),
                replace_all: false,
                expected_hash: replacement.content_hash,
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("src/file")).unwrap(),
            b"edited"
        );
        session.close().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resolved_subtree_grants_reject_symlink_read_and_write_escape() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::create_dir(root.path().join("private")).unwrap();
        std::fs::write(root.path().join("private/secret"), "private marker").unwrap();
        std::os::unix::fs::symlink("../private", root.path().join("src/link")).unwrap();
        let client = AuthorizationClient::new(vec![
            permission(READ, "src", true),
            permission(WRITE, "src", true),
        ]);
        let session = session(&root, client);
        assert!(session.read(read("src/link/secret")).await.is_err());
        assert!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("src/link/new").unwrap(),
                    content: b"bad".to_vec(),
                    expected_hash: None,
                })
                .await
                .is_err()
        );
        assert!(!root.path().join("private/new").exists());
        assert_eq!(
            std::fs::read(root.path().join("private/secret")).unwrap(),
            b"private marker"
        );
        session.close().await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn provider_scoped_recursive_traversal_does_not_leak_symlink_targets() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::create_dir(root.path().join("private")).unwrap();
        std::fs::write(root.path().join("src/public.txt"), "needle public marker").unwrap();
        std::fs::write(
            root.path().join("private/secret.txt"),
            "needle private marker",
        )
        .unwrap();
        std::os::unix::fs::symlink("../private", root.path().join("src/link")).unwrap();
        std::os::unix::fs::symlink("../private/secret.txt", root.path().join("src/alias.txt"))
            .unwrap();
        let client = AuthorizationClient::new(vec![permission(READ, "src", true)]);
        let session = session(&root, client);
        let result = session
            .glob(GlobRequest {
                path: WorkdirPath::new("src").unwrap(),
                pattern: "**/*.txt".into(),
                limit: 100,
            })
            .await
            .unwrap();
        assert_eq!(
            result.paths,
            vec![WorkdirPath::new("src/public.txt").unwrap()]
        );
        for output_mode in [
            workdir::GrepOutputMode::Content,
            workdir::GrepOutputMode::FilesWithMatches,
            workdir::GrepOutputMode::Count,
        ] {
            let result = session
                .grep(GrepRequest {
                    path: WorkdirPath::new("src").unwrap(),
                    pattern: "needle".into(),
                    glob: None,
                    file_type: None,
                    case_insensitive: false,
                    before_context: 0,
                    after_context: 0,
                    multiline: false,
                    output_mode,
                    limit: 100,
                    offset: 0,
                })
                .await
                .unwrap();
            assert_eq!(
                result.paths,
                vec![WorkdirPath::new("src/public.txt").unwrap()]
            );
            assert!(!result.output.contains("private"));
            assert!(!result.output.contains("secret"));
            assert!(!result.output.contains("alias.txt"));
            assert!(result.output.contains("src/public.txt"));
            assert_eq!(result.matched_files, 1);
        }
        // List may reject a symlinked entry instead of skipping it. Either behavior
        // must avoid disclosing target metadata; successful entries stay scoped.
        if let Ok(listed) = session
            .list(ListRequest {
                path: WorkdirPath::new("src").unwrap(),
                after: None,
                limit: 100,
            })
            .await
        {
            assert!(
                listed
                    .entries
                    .iter()
                    .all(|entry| !entry.path.as_str().contains("alias.txt")
                        && !entry.path.as_str().contains("link"))
            );
        }
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn unavailable_workspace_bound_client_does_not_fall_back_to_local_authority() {
        let root = tempfile::tempdir().unwrap();
        let workspace_id = crate::WorkspaceId::new("workspace").unwrap();
        let session = session(
            &root,
            crate::unavailable_workspace_client(Some(&workspace_id), "transport unavailable"),
        );
        assert!(matches!(
            session
                .write(WriteRequest {
                    path: WorkdirPath::new("denied").unwrap(),
                    content: b"bad".to_vec(),
                    expected_hash: None,
                })
                .await,
            Err(WorkdirError::Denied(_))
        ));
        assert!(!root.path().join("denied").exists());
        session.close().await.unwrap();
    }

    #[tokio::test]
    async fn standalone_unavailable_client_preserves_local_compatibility() {
        let root = tempfile::tempdir().unwrap();
        let session = session(
            &root,
            crate::unavailable_workspace_client(None, "standalone test"),
        );
        session
            .write(WriteRequest {
                path: WorkdirPath::new("local").unwrap(),
                content: b"standalone".to_vec(),
                expected_hash: None,
            })
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("local")).unwrap(),
            b"standalone"
        );
        session.close().await.unwrap();
    }
}
