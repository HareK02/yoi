//! Refusal reasons are selected by providers, not parsed from their messages.
use std::sync::Arc;

use manifest::{Permission, Scope, ScopeConfig, ScopeRule, SharedScope, SymlinkPolicy};
use workdir::{
    LocalWorkdirSession, ReadOnlyWorkdirSession, StatRequest, Workdir,
    WorkdirDenialReason as Reason, WorkdirPath, WorkdirScopeAuthorizationRequest, WorkdirSession,
    WorkdirSessionCapabilities, WorkdirToolBroker, WorkdirToolScope,
    WorkdirToolScopePermission as ToolPermission, WorkdirToolScopeRule, WriteRequest,
    http::{WorkdirTransportError, WorkdirTransportErrorCode as Code},
};

fn path(value: &str) -> WorkdirPath {
    WorkdirPath::new(value).unwrap()
}
fn rule(target: &str, permission: ToolPermission) -> WorkdirToolScopeRule {
    WorkdirToolScopeRule {
        target: path(target),
        permission,
        recursive: true,
        symlink_policy: SymlinkPolicy::Resolved,
    }
}
fn scoped(target: &str, permission: ToolPermission) -> WorkdirToolScope {
    WorkdirToolScope {
        rules: vec![rule(target, permission)],
        cwd: path(target),
        command: false,
    }
}
fn assert_reason(error: workdir::WorkdirError, reason: Reason) {
    assert_eq!(error.denial_reason(), Some(reason), "{error}");
    let transport = WorkdirTransportError::from_workdir_error(&error);
    assert_eq!(transport.code.http_status(), 403);
    assert_eq!(transport.denial_reason, Some(reason));
    let wire = serde_json::to_string(&transport).unwrap();
    assert!(!wire.contains("private-input"));
    let decoded: WorkdirTransportError = serde_json::from_str(&wire).unwrap();
    assert_eq!(
        WorkdirTransportError::from_workdir_error(&decoded.into_workdir_error()),
        transport
    );
}

#[tokio::test]
async fn local_authorize_scope_refusals_distinguish_attachment_and_delegation() {
    let root = tempfile::tempdir().unwrap();
    let session = LocalWorkdirSession::materialized_bound(
        Workdir::new("denials"),
        root.path().to_owned(),
        root.path().to_owned(),
        SharedScope::new(
            Scope::from_config(&ScopeConfig {
                allow: vec![ScopeRule {
                    target: root.path().join("allowed"),
                    permission: Permission::Write,
                    recursive: true,
                    symlink_policy: SymlinkPolicy::Resolved,
                }],
                deny: vec![],
            })
            .unwrap(),
        ),
        WorkdirSessionCapabilities::ALL,
    );
    for (rules, requested, reason) in [
        (
            vec![rule("", ToolPermission::Read)],
            "private-input",
            Reason::AttachmentScopeExceeded,
        ),
        (
            vec![rule("allowed/child", ToolPermission::Read)],
            "allowed/private-input",
            Reason::DelegatedScopeExceeded,
        ),
    ] {
        let error = session
            .authorize_scope_path(WorkdirScopeAuthorizationRequest {
                rules,
                path: path(requested),
                permission: ToolPermission::Read,
            })
            .await
            .unwrap_err();
        assert_reason(error, reason);
    }
}

#[tokio::test]
async fn broker_scope_refusal_matrix_preserves_distinct_reasons() {
    let root = tempfile::tempdir().unwrap();
    let local = Arc::new(LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().to_owned(),
    ));
    let parent = WorkdirToolBroker::new(local);
    let mut empty = scoped("", ToolPermission::Read);
    empty.rules.clear();
    let mut command = scoped("", ToolPermission::Read);
    command.command = true;
    let mut cwd = scoped("allowed", ToolPermission::Read);
    cwd.cwd = path("private-input");
    for (request, reason) in [
        (empty, Reason::EmptyScope),
        (command, Reason::CommandRequiresWritableScope),
        (cwd, Reason::CwdOutsideReadableScope),
    ] {
        assert_reason(parent.scope(request).await.unwrap_err(), reason);
    }
    let mut child_request = scoped("allowed", ToolPermission::Write);
    child_request.rules[0].recursive = false;
    let child = parent.scope(child_request).await.unwrap();
    assert_reason(
        child
            .scope(scoped("private-input", ToolPermission::Read))
            .await
            .unwrap_err(),
        Reason::ParentScopeExceeded,
    );
    assert_reason(
        parent
            .scope(scoped("allowed", ToolPermission::Write))
            .await
            .unwrap_err(),
        Reason::ChildWriteLeaseConflict,
    );
    assert_reason(
        parent
            .write(WriteRequest {
                path: path("allowed/private-input"),
                content: "unused".into(),
                expected_hash: None,
            })
            .await
            .unwrap_err(),
        Reason::ChildWriteLeaseConflict,
    );
    assert_reason(
        child
            .stat(StatRequest {
                path: path("private-input/nested"),
            })
            .await
            .unwrap_err(),
        Reason::DelegatedScopeExceeded,
    );
}

#[tokio::test]
async fn read_only_and_scoped_capability_refusals_do_not_collapse() {
    let root = tempfile::tempdir().unwrap();
    let local = Arc::new(LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().to_owned(),
    ));
    let parent = WorkdirToolBroker::new(local.clone());
    let child = parent
        .scope(scoped("", ToolPermission::Read))
        .await
        .unwrap();
    let request = WriteRequest {
        path: path("private-input"),
        content: "unused".into(),
        expected_hash: None,
    };
    assert_reason(
        child
            .start_command(workdir::CommandRequest {
                command: "must-not-run".into(),
                timeout_secs: 1,
                output_limit: 1,
                cwd: WorkdirPath::root(),
                spill_dir: None,
                tool_call_id: None,
            })
            .await
            .unwrap_err(),
        Reason::ScopedCapabilityDenied,
    );
    let readonly = ReadOnlyWorkdirSession::new(local);
    assert_reason(
        readonly.write(request).await.unwrap_err(),
        Reason::ReadOnlySession,
    );
    let error = parent
        .checkout_search(workdir::CheckoutSearchRequest {
            operation: workdir::CheckoutSearchOperation::List(workdir::ListRequest {
                path: path("private-input"),
                limit: 10,
                after: None,
            }),
            output_root: path("allowed"),
            scope_layers: vec![],
        })
        .await
        .unwrap_err();
    assert_reason(error, Reason::CheckoutOutputRootExceeded);
}

#[test]
fn denial_wire_is_allowlisted_and_old_missing_reason_remains_unknown() {
    let old: WorkdirTransportError =
        serde_json::from_str(r#"{"code":"denied","message":"private-input secret"}"#).unwrap();
    let error = old.into_workdir_error();
    assert_eq!(error.denial_reason(), None);
    let forwarded = WorkdirTransportError::from_workdir_error(&error);
    assert_eq!(forwarded.code, Code::Denied);
    assert_eq!(forwarded.message, "Workdir operation was denied");
    assert_eq!(
        serde_json::to_value(forwarded).unwrap(),
        serde_json::json!({"code":"denied", "message":"Workdir operation was denied"})
    );
    for reason in ["/host/private-input", "secret", &"x".repeat(8192)] {
        let input =
            serde_json::json!({"code":"denied", "message":"ignored", "denial_reason":reason});
        assert!(serde_json::from_value::<WorkdirTransportError>(input).is_err());
    }
}

// A provider with filesystem methods, but no provider-side scope resolver.
#[derive(Debug)]
struct UnresolvedProvider(LocalWorkdirSession);
#[async_trait::async_trait]
impl WorkdirSession for UnresolvedProvider {
    fn workdir(&self) -> &Workdir {
        self.0.workdir()
    }
    fn capabilities(&self) -> WorkdirSessionCapabilities {
        self.0.capabilities()
    }
    async fn stat(&self, r: StatRequest) -> Result<workdir::StatResult, workdir::WorkdirError> {
        self.0.stat(r).await
    }
    async fn read(
        &self,
        r: workdir::ReadRequest,
    ) -> Result<workdir::ReadResult, workdir::WorkdirError> {
        self.0.read(r).await
    }
    async fn write(&self, r: WriteRequest) -> Result<workdir::WriteResult, workdir::WorkdirError> {
        self.0.write(r).await
    }
    async fn edit(
        &self,
        r: workdir::EditRequest,
    ) -> Result<workdir::EditResult, workdir::WorkdirError> {
        self.0.edit(r).await
    }
    async fn list(
        &self,
        r: workdir::ListRequest,
    ) -> Result<workdir::ListResult, workdir::WorkdirError> {
        self.0.list(r).await
    }
    async fn glob(
        &self,
        r: workdir::GlobRequest,
    ) -> Result<workdir::GlobResult, workdir::WorkdirError> {
        self.0.glob(r).await
    }
    async fn grep(
        &self,
        r: workdir::GrepRequest,
    ) -> Result<workdir::GrepResult, workdir::WorkdirError> {
        self.0.grep(r).await
    }
    async fn start_command(
        &self,
        r: workdir::CommandRequest,
    ) -> Result<workdir::CommandHandle, workdir::WorkdirError> {
        self.0.start_command(r).await
    }
    async fn command_status(
        &self,
        h: workdir::CommandHandle,
    ) -> Result<workdir::CommandStatus, workdir::WorkdirError> {
        self.0.command_status(h).await
    }
    async fn command_output(
        &self,
        r: workdir::CommandOutputRequest,
    ) -> Result<workdir::CommandOutput, workdir::WorkdirError> {
        self.0.command_output(r).await
    }
    async fn cancel_command(&self, h: workdir::CommandHandle) -> Result<(), workdir::WorkdirError> {
        self.0.cancel_command(h).await
    }
    async fn close(&self) -> Result<(), workdir::WorkdirError> {
        self.0.close().await
    }
}

#[tokio::test]
async fn providers_without_scope_resolution_fail_closed_with_specific_reasons() {
    let root = tempfile::tempdir().unwrap();
    let provider = UnresolvedProvider(LocalWorkdirSession::new(
        Scope::writable(root.path()).unwrap(),
        root.path().to_owned(),
    ));
    let rules = vec![rule("", ToolPermission::Read)];
    assert_reason(
        provider
            .authorize_scope_path(WorkdirScopeAuthorizationRequest {
                rules: rules.clone(),
                path: path("private-input"),
                permission: ToolPermission::Read,
            })
            .await
            .unwrap_err(),
        Reason::ScopeResolutionUnavailable,
    );
    assert_reason(
        provider
            .scope_rules_overlap(workdir::WorkdirScopeOverlapRequest {
                left: rules[0].clone(),
                right: rules[0].clone(),
            })
            .await
            .unwrap_err(),
        Reason::ScopeComparisonUnavailable,
    );
    let mut logical = rules;
    logical[0].symlink_policy = SymlinkPolicy::Logical;
    provider
        .authorize_scope_path(WorkdirScopeAuthorizationRequest {
            rules: logical,
            path: path("private-input"),
            permission: ToolPermission::Read,
        })
        .await
        .unwrap();
}
