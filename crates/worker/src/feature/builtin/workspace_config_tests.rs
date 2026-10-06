//! Backend-independent preparation tests; not config persistence/E2E evidence.
use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use agen::tool::ToolExecutionContext;
use protocol::InvocationArgumentValue;
use wip_protocol::{ProtocolErrorCode, Value};

use crate::feature::{FeatureInvocationRegistry, FeatureRegistryBuilder};
use crate::hook::HookRegistryBuilder;
use crate::wip::{WipCallContext, WipMountRegistry, WipOperationError};
use crate::worker::{WorkspaceClientError, WorkspaceRequest, WorkspaceResponse};

#[derive(Debug)]
struct Client {
    workspace: Option<&'static str>,
    available: bool,
}
impl WorkspaceClient for Client {
    fn workspace_id(&self) -> Option<&str> {
        self.workspace
    }
    fn kind(&self) -> &str {
        "test"
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn execute(&self, _: WorkspaceRequest) -> Result<WorkspaceResponse, WorkspaceClientError> {
        panic!("scaffolding must not invent Backend endpoints");
    }
}

struct Backend {
    granted: AtomicBool,
    read_only: AtomicBool,
    unknown: AtomicBool,
    calls: AtomicUsize,
    keys: Mutex<Vec<String>>,
    active: Mutex<Option<ConfigAttachment>>,
}
impl Backend {
    fn new() -> Self {
        Self {
            granted: AtomicBool::new(true),
            read_only: AtomicBool::new(false),
            unknown: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            keys: Mutex::new(Vec::new()),
            active: Mutex::new(None),
        }
    }
}
#[async_trait]
impl WorkspaceConfigAttachmentBackend for Backend {
    fn is_available(&self) -> bool {
        self.granted.load(Ordering::SeqCst)
    }
    fn validate_access(&self, access: Option<ConfigAccess>) -> Result<(), ConfigAttachError> {
        if !self.granted.load(Ordering::SeqCst)
            || (self.read_only.load(Ordering::SeqCst) && access == Some(ConfigAccess::ReadWrite))
        {
            return Err(ConfigAttachError::Denied);
        }
        Ok(())
    }
    async fn attach(
        &self,
        key: &str,
        access: Option<ConfigAccess>,
    ) -> Result<ConfigAttachment, ConfigAttachError> {
        self.validate_access(access)?;
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.keys.lock().unwrap().push(key.into());
        if self.unknown.load(Ordering::SeqCst) {
            return Err(ConfigAttachError::OutcomeUnknown);
        }
        let mut active = self.active.lock().unwrap();
        let already_attached = active.is_some();
        let access = access.unwrap_or(if self.read_only.load(Ordering::SeqCst) {
            ConfigAccess::ReadOnly
        } else {
            ConfigAccess::ReadWrite
        });
        let result = ConfigAttachment {
            connection_id: "connection-1".into(),
            access,
            already_attached,
        };
        *active = Some(result.clone());
        Ok(result)
    }
}

fn feature(
    backend: Arc<Backend>,
    wip_mode: bool,
    workspace: Option<&'static str>,
) -> WorkspaceConfigFeature {
    WorkspaceConfigFeature::new(
        Arc::new(Client {
            workspace,
            available: true,
        }),
        backend,
        wip_mode,
    )
}
fn registry(feature: WorkspaceConfigFeature) -> FeatureInvocationRegistry {
    let mut tools = Vec::new();
    let report = FeatureRegistryBuilder::new()
        .with_module(feature)
        .install_into_pending(&mut tools, &mut HookRegistryBuilder::default());
    assert!(!report.has_errors());
    assert!(tools.is_empty(), "no config compatibility tools");
    report.chat_invocations
}
fn invocation(key: &str, access: Option<&str>) -> FeatureInvocation {
    FeatureInvocation {
        invocation_id: key.into(),
        identity: FeatureInvocationIdentity(INVOCATION_ID.into()),
        name: FEATURE_ID.into(),
        arguments: access
            .into_iter()
            .map(|value| InvocationArgumentValue {
                name: "access".into(),
                value: InvocationValue::String(value.into()),
            })
            .collect(),
    }
}
fn context() -> WipCallContext {
    WipCallContext {
        execution: ToolExecutionContext::direct(),
        security_context: "test-worker".into(),
    }
}

#[tokio::test]
async fn workspace_config_common_completion_selection_and_preparation_use_backend_ledger() {
    let backend = Arc::new(Backend::new());
    let registry = registry(feature(backend.clone(), true, Some("workspace-1")));
    let completion = registry.feature_completions("workspace-c");
    assert_eq!(completion.len(), 1);
    let descriptor = completion[0].invocation.as_ref().unwrap();
    assert_eq!(descriptor.identity.0, INVOCATION_ID);
    assert!(descriptor.client_adapter.is_none());
    assert_eq!(
        registry
            .argument_completions(&descriptor.identity, "access", "read_")
            .await
            .into_iter()
            .map(|entry| entry.value)
            .collect::<Vec<_>>(),
        ["read_only", "read_write"]
    );

    let first = registry
        .invoke(&invocation("selected-1", None))
        .await
        .unwrap();
    assert_eq!(first.status, FeatureInvocationStatus::Succeeded);
    assert!(first.message.contains("read_write"));
    assert!(first.context.unwrap().contains("WIP `/workspace-config`"));
    let second = registry
        .invoke(&invocation("selected-2", None))
        .await
        .unwrap();
    assert!(second.message.starts_with("Already attached"));
    assert_eq!(*backend.keys.lock().unwrap(), ["selected-1", "selected-2"]);
}

#[tokio::test]
async fn workspace_config_slash_and_wip_self_attach_share_handler_and_identity() {
    let backend = Arc::new(Backend::new());
    let feature = feature(backend.clone(), true, Some("workspace-1"));
    let slash = registry(feature.clone());
    slash.invoke(&invocation("slash-1", None)).await.unwrap();
    let projection = wip::attach_projection(&feature);
    assert_eq!(projection.route, CONTENT_ROOT);
    assert_eq!(projection.descriptor.operations.len(), 1);
    let result = projection
        .handler
        .call("attach", &BTreeMap::new(), context())
        .await;
    assert!(result.is_ok());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        backend
            .active
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .connection_id,
        "connection-1"
    );
    assert!(backend.keys.lock().unwrap()[1].starts_with("wip:"));
}

#[tokio::test]
async fn workspace_config_effective_access_and_explicit_read_write_do_not_silently_downgrade() {
    let backend = Arc::new(Backend::new());
    backend.read_only.store(true, Ordering::SeqCst);
    let feature = feature(backend.clone(), true, Some("workspace-1"));
    let registry = registry(feature.clone());
    let result = registry
        .invoke(&invocation("effective-1", None))
        .await
        .unwrap();
    assert!(result.message.contains("read_only"));
    assert!(
        registry
            .invoke(&invocation("write-1", Some("read_write")))
            .await
            .is_err()
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let args = BTreeMap::from([("access".into(), Value::String("read_write".into()))]);
    assert!(
        matches!(wip::attach_projection(&feature).handler.call("attach", &args, context()).await,
        Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::PermissionDenied)
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workspace_config_old_handler_and_replay_validation_recheck_revocation() {
    let backend = Arc::new(Backend::new());
    let feature = feature(backend.clone(), true, Some("workspace-1"));
    let old = wip::attach_projection(&feature);
    let registry = registry(feature);
    let request = invocation("replay-1", None);
    registry.invoke(&request).await.unwrap();
    backend.granted.store(false, Ordering::SeqCst);
    assert!(registry.feature_completions("").is_empty());
    assert!(registry.validate(&request).is_err());
    assert!(!old.handler.is_visible());
    assert!(
        matches!(old.handler.call("attach", &BTreeMap::new(), context()).await,
        Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::PermissionDenied)
    );
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workspace_config_unknown_attach_is_not_retried_and_never_reports_success() {
    let backend = Arc::new(Backend::new());
    backend.unknown.store(true, Ordering::SeqCst);
    let feature = feature(backend.clone(), true, Some("workspace-1"));
    let registry = registry(feature.clone());
    let error = registry
        .invoke(&invocation("unknown-1", None))
        .await
        .unwrap_err();
    assert!(error.outcome_unknown);
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(
        wip::attach_projection(&feature)
            .handler
            .call("attach", &BTreeMap::new(), context())
            .await,
        Err(WipOperationError::OutcomeUnknown(_))
    ));
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn workspace_config_non_wip_and_missing_workspace_have_no_fallback() {
    for (wip_mode, workspace) in [(false, Some("workspace-1")), (true, None), (true, Some(""))] {
        let backend = Arc::new(Backend::new());
        let feature = feature(backend.clone(), wip_mode, workspace);
        assert!(registry(feature.clone()).descriptors().is_empty());
        let mut mounts = WipMountRegistry::new();
        wip::mount_workspace_config_attach_wip(&mut mounts, &feature).unwrap();
        assert_eq!(mounts.routes().count(), 0);
        assert_eq!(
            feature.attach("not-enabled", None).await,
            Err(ConfigAttachError::Denied)
        );
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn workspace_config_rejects_unstructured_authority_and_invalid_access_before_attach() {
    let backend = Arc::new(Backend::new());
    let feature = feature(backend.clone(), true, Some("workspace-1"));
    let registry = registry(feature.clone());
    assert!(
        registry
            .invoke(&invocation("bad-1", Some("admin")))
            .await
            .is_err()
    );
    let projection = wip::attach_projection(&feature);
    let args = BTreeMap::from([("workspace_id".into(), Value::String("another".into()))]);
    assert!(
        matches!(projection.handler.call("attach", &args, context()).await,
        Err(WipOperationError::Protocol(error)) if error.code == ProtocolErrorCode::InvalidArguments)
    );
    let mut bad = invocation("bad-2", None);
    bad.arguments.push(InvocationArgumentValue {
        name: "access".into(),
        value: InvocationValue::String("true".into()),
    });
    assert!(registry.invoke(&bad).await.is_err());
    assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn workspace_config_controller_gate_requires_configured_workspace_wip_and_not_a_grant() {
    for enabled in [false, true] {
        for wip in [false, true] {
            for workspace in [None, Some(""), Some("workspace-1"), Some("bad\nworkspace")] {
                for available in [false, true] {
                    let client = Arc::new(Client {
                        workspace,
                        available,
                    });
                    let expected = enabled && wip && available && workspace == Some("workspace-1");
                    // Registration needs identity/mode, but performs no grant or
                    // filesystem mutation. Runtime authorization is still Backend's.
                    assert_eq!(
                        WorkspaceConfigFeature::configured(client, enabled, wip).is_some(),
                        expected
                    );
                }
            }
        }
    }
}
