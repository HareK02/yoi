//! Wire diagnostics must not change command disappearance or lease release.
//! Fake command handles only; no OS process is started, polled, or cancelled.
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use workdir::*;

#[derive(Debug)]
struct CommandFailureSession {
    local: LocalWorkdirSession,
    code: &'static str,
    depth: u8,
    cancel_ok: bool,
}
impl CommandFailureSession {
    fn new(root: &Path, code: &'static str, depth: u8, cancel_ok: bool) -> Arc<Self> {
        Arc::new(Self {
            local: LocalWorkdirSession::new(
                manifest::Scope::writable(root).unwrap(),
                root.to_path_buf(),
            ),
            code,
            depth,
            cancel_ok,
        })
    }
    fn failure(&self) -> WorkdirError {
        let mut error = serde_json::from_value::<http::WorkdirTransportError>(serde_json::json!({
            "code": self.code, "message": "safe fixture error",
            "denial_reason": (self.depth > 0).then_some(WorkdirDenialReason::ReadOnlySession),
        }))
        .unwrap()
        .into_workdir_error();
        if self.depth == 2 {
            error = WorkdirError::DenialContext {
                reason: WorkdirDenialReason::OsPermissionDenied,
                source: Box::new(error),
            };
        }
        error
    }
}
#[async_trait]
impl WorkdirSession for CommandFailureSession {
    fn workdir(&self) -> &Workdir {
        self.local.workdir()
    }
    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.local.capabilities()
    }
    async fn authorize_scope_path(
        &self,
        r: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        self.local.authorize_scope_path(r).await
    }
    async fn scope_rules_overlap(
        &self,
        r: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        self.local.scope_rules_overlap(r).await
    }
    async fn stat(&self, r: StatRequest) -> Result<StatResult, WorkdirError> {
        self.local.stat(r).await
    }
    async fn read(&self, r: ReadRequest) -> Result<ReadResult, WorkdirError> {
        self.local.read(r).await
    }
    async fn write(&self, r: WriteRequest) -> Result<WriteResult, WorkdirError> {
        self.local.write(r).await
    }
    async fn edit(&self, r: EditRequest) -> Result<EditResult, WorkdirError> {
        self.local.edit(r).await
    }
    async fn list(&self, r: ListRequest) -> Result<ListResult, WorkdirError> {
        self.local.list(r).await
    }
    async fn glob(&self, r: GlobRequest) -> Result<GlobResult, WorkdirError> {
        self.local.glob(r).await
    }
    async fn grep(&self, r: GrepRequest) -> Result<GrepResult, WorkdirError> {
        self.local.grep(r).await
    }
    async fn start_command(&self, _: CommandRequest) -> Result<CommandHandle, WorkdirError> {
        Ok(CommandHandle("fake-command".into()))
    }
    async fn command_status(&self, _: CommandHandle) -> Result<CommandStatus, WorkdirError> {
        Err(self.failure())
    }
    async fn command_output(&self, _: CommandOutputRequest) -> Result<CommandOutput, WorkdirError> {
        Err(self.failure())
    }
    async fn cancel_command(&self, _: CommandHandle) -> Result<(), WorkdirError> {
        if self.cancel_ok {
            Ok(())
        } else {
            Err(self.failure())
        }
    }
    async fn close(&self) -> Result<(), WorkdirError> {
        self.local.close().await
    }
}
fn command() -> CommandRequest {
    CommandRequest {
        command: "fake command, never executed".into(),
        timeout_secs: 1,
        output_limit: 100,
        cwd: WorkdirPath::root(),
        spill_dir: None,
        tool_call_id: None,
    }
}
fn scope() -> WorkdirToolScope {
    WorkdirToolScope {
        rules: vec![WorkdirToolScopeRule {
            target: WorkdirPath::root(),
            permission: WorkdirToolScopePermission::Write,
            recursive: true,
            symlink_policy: Default::default(),
        }],
        command: true,
        cwd: WorkdirPath::root(),
    }
}

#[tokio::test]
async fn command_diagnostics_preserve_detach_disappearance_and_busy_guard() {
    for code in ["unknown_command", "unavailable"] {
        for depth in [0, 1, 2] {
            let dir = TempDir::new().unwrap();
            let router = WorkdirSessionRouter::new();
            let alias = WorkdirAttachmentAlias::new("fixture").unwrap();
            router
                .attach(
                    alias.clone(),
                    CommandFailureSession::new(dir.path(), code, depth, false),
                )
                .unwrap();
            let session = router.resolve(Some("fixture")).unwrap().session;
            session.start_command(command()).await.unwrap();
            let result = router.detach(&alias).await;
            if code == "unknown_command" {
                result.unwrap_or_else(|error| panic!("depth {depth}: {error:?}"));
                assert!(router.resolve(Some("fixture")).is_err());
            } else {
                assert!(matches!(result, Err(WorkdirError::Conflict(_))));
                assert!(router.resolve(Some("fixture")).is_ok());
            }
        }
    }
}

#[tokio::test]
async fn command_diagnostics_preserve_scope_close_and_write_lease_guard() {
    for code in ["unknown_command", "unavailable"] {
        for depth in [0, 1, 2] {
            for cancel_ok in [false, true] {
                let dir = TempDir::new().unwrap();
                let broker = WorkdirToolBroker::new(CommandFailureSession::new(
                    dir.path(),
                    code,
                    depth,
                    cancel_ok,
                ));
                let child = broker.scope(scope()).await.unwrap();
                child.start_command(command()).await.unwrap();
                let result = child.close().await;
                if code == "unknown_command" {
                    result.unwrap_or_else(|error| {
                        panic!("depth {depth}, cancel_ok={cancel_ok}: {error:?}")
                    });
                    assert!(!child.is_active());
                    broker.scope(scope()).await.unwrap().close().await.unwrap();
                } else {
                    assert!(result.is_err());
                    // Closing revokes new work immediately, but failed cleanup
                    // must retain the write lease until ownership is resolved.
                    assert!(!child.is_active());
                    assert!(broker.scope(scope()).await.is_err());
                }
            }
        }
    }
}
