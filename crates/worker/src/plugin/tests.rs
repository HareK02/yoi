//! Component coverage through canonical T-791 packing, real ArtifactCache/Executor,
//! Feature installation and the Engine Tool registry. Only ArtifactSource and
//! PluginHostOwner are scripted: this is not a full Server/HTTPS vertical test,
//! nor proof of owner-side durable storage. Cancellation checks the exact owner
//! Cancel request and acknowledgement, not an inferred effect rollback.
//! Run with `cargo test -p worker --lib plugin::tests::`; no guest build is needed.

use super::*;
use crate::feature::FeatureRegistryBuilder;
use crate::hook::{HookInvocationContext, HookRegistry, HookRegistryBuilder};
use agen::Engine;
use agen::llm_client::client::LlmClient;
use agen::llm_client::{ClientError, Request, ResponseStream};
use agen::state::Mutable;
use manifest::plugin::{self, PluginPackageLimits, PluginSourceKind};
use serde_json::json;
use server_api::{
    PluginArtifact, PluginArtifactDeliveryMetadata, PluginArtifactDeliveryRequest, PluginConsumer,
    PluginEffectObservation, PluginHostError, PluginHostErrorCode, PluginHostOperation,
    PluginHostRequest, PluginHostResponse, PluginSchemaContribution,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use yoi_plugin_runtime::delivery::{ArtifactStream, CacheLimits, DeliveryError};

const MANIFEST: &str =
    include_str!("../../../../resources/plugin/templates/rust-component-tool/plugin.toml");
const COMPONENT: &[u8] =
    include_bytes!("../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm");
const TOOL_A: &str = "installation_a__example_echo";
const TOOL_C: &str = "installation_c__example_echo";
const INPUT: &str = r#"{"text":"hello from a Component"}"#;

struct Source {
    artifact: PluginArtifact,
    bytes: Vec<u8>,
    fetches: AtomicUsize,
    authorizations: AtomicUsize,
}

#[async_trait]
impl ArtifactSource for Source {
    fn server_authority_identity(&self) -> &str {
        "component-fixture-owner"
    }

    async fn authorize(
        &self,
        request: &PluginArtifactDeliveryRequest,
    ) -> Result<PluginArtifactDeliveryMetadata, DeliveryError> {
        self.authorizations.fetch_add(1, Ordering::SeqCst);
        assert_eq!(request.artifact_digest, self.artifact.artifact_digest);
        Ok(PluginArtifactDeliveryMetadata {
            workspace_id: "workspace-a".into(),
            request: request.clone(),
            artifact: self.artifact.clone(),
        })
    }

    async fn fetch(
        &self,
        _: &PluginArtifactDeliveryRequest,
    ) -> Result<ArtifactStream, DeliveryError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(std::io::Cursor::new(self.bytes.clone())))
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    source: Source,
    cache: ArtifactCache,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("plugin.toml"), MANIFEST).unwrap();
        std::fs::write(directory.path().join("plugin.component.wasm"), COMPONENT).unwrap();
        let limits = PluginPackageLimits::default();
        let package =
            plugin::read_plugin_directory(directory.path(), PluginSourceKind::Explicit, &limits)
                .unwrap();
        let packed = plugin::pack_plugin_package(&package, &limits).unwrap();
        let m = package.package.manifest;
        let schema_hex: String = Sha256::digest(m.config_schema.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let schema_digest = format!("sha256:{schema_hex}");
        let artifact = PluginArtifact {
            package_id: m.id.clone(),
            name: m.name,
            version: m.version.clone(),
            description: m.description,
            package_digest: packed.package_digest,
            artifact_digest: packed.artifact_digest,
            artifact_size: packed.artifact_size.try_into().unwrap(),
            schema_version: m.schema_version,
            world: m.runtime.world,
            required_imports: m.runtime.required_imports,
            tools: serde_json::from_value(serde_json::to_value(m.tools).unwrap()).unwrap(),
            connections: vec![],
            requested_grants: vec![],
            data_collections: vec![],
            config_schema: PluginSchemaContribution {
                identity: format!("{}/config", m.id),
                version: m.version.clone(),
                namespace: format!("{}/config/{}/{}", m.id, m.version, schema_digest),
                digest: schema_digest,
                dcdl: m.config_schema,
            },
            revoked: false,
            revocation_reason: None,
        };
        let cache =
            ArtifactCache::new(directory.path().join("cache"), CacheLimits::default()).unwrap();
        Self {
            _directory: directory,
            source: Source {
                artifact,
                bytes: packed.bytes,
                fetches: AtomicUsize::new(0),
                authorizations: AtomicUsize::new(0),
            },
            cache,
        }
    }

    async fn prepare(
        &self,
        installations: &[&str],
        owner: Arc<Owner>,
        session: &str,
    ) -> PluginToolsFeature {
        let pins = installations
            .iter()
            .map(|installation| PluginToolPin {
                workspace_id: "workspace-a".into(),
                installation_id: (*installation).into(),
                plan_id: "persisted-plan".into(),
                subject: "worker-a".into(),
                request: PluginArtifactDeliveryRequest {
                    plan_id: "persisted-plan".into(),
                    plan_fingerprint: "persisted-fingerprint".into(),
                    installation_id: (*installation).into(),
                    artifact_digest: self.source.artifact.artifact_digest.clone(),
                },
                expectation: ArtifactExpectation::new(self.source.artifact.clone()),
                tools: vec!["example_echo".into()],
                config: json!({}),
                grants: Grants::default(),
            })
            .collect();
        PluginToolsFeature::prepare(
            pins,
            &self.source,
            &self.cache,
            owner,
            session,
            Limits::default(),
        )
        .await
        .unwrap()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Behavior {
    Allow,
    DenyBegin,
    DenyAdmit,
    DenyLatePublish,
    HoldAdmit,
}

struct Owner {
    behavior: Behavior,
    requests: Mutex<Vec<PluginHostRequest>>,
    entered: Semaphore,
}

impl Owner {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            requests: Mutex::new(Vec::new()),
            entered: Semaphore::new(0),
        })
    }

    fn requests(&self) -> Vec<PluginHostRequest> {
        self.requests.lock().unwrap().clone()
    }

    fn consumers(&self) -> Vec<PluginConsumer> {
        self.requests()
            .into_iter()
            .filter_map(|request| match request {
                PluginHostRequest::Begin { consumer, .. } => Some(consumer),
                _ => None,
            })
            .collect()
    }

    fn publications(&self) -> usize {
        self.requests()
            .iter()
            .filter(|request| {
                matches!(
                    request,
                    PluginHostRequest::Authorize {
                        operation: PluginHostOperation::Publish,
                        ..
                    }
                )
            })
            .count()
    }
}

fn denied() -> PluginHostError {
    PluginHostError {
        code: PluginHostErrorCode::Denied,
        effect: PluginEffectObservation::NotDispatched,
        diagnostic_id: "component-fixture-denial".into(),
    }
}

#[async_trait]
impl PluginHostOwner for Owner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        self.requests.lock().unwrap().push(request.clone());
        match request {
            PluginHostRequest::Begin {
                plan_id,
                installation_id,
                consumer:
                    PluginConsumer::ToolCall {
                        tool_call_id,
                        tool_name,
                    },
            } => {
                assert_eq!(plan_id, "persisted-plan");
                assert!(matches!(
                    installation_id.as_str(),
                    "installation_a" | "installation_c"
                ));
                assert_eq!(tool_name, "example_echo");
                let invocation_id = format!("owner-{installation_id}-{tool_call_id}");
                // This fixture scripts owner rejection of an already cancelled
                // consumer; the production owner persistence belongs to its suite.
                let cancelled = self.requests().iter().any(|request| {
                    matches!(request, PluginHostRequest::Cancel { invocation_id: id } if id == &invocation_id)
                });
                if self.behavior == Behavior::DenyBegin || cancelled {
                    return Err(denied());
                }
                Ok(PluginHostResponse::Invocation {
                    invocation_id,
                    expires_at_ms: (SystemTime::now().duration_since(UNIX_EPOCH).unwrap()
                        + Duration::from_secs(60))
                    .as_millis()
                    .try_into()
                    .unwrap(),
                })
            }
            PluginHostRequest::Authorize { operation, .. } => {
                if operation == PluginHostOperation::Admit {
                    if self.behavior == Behavior::DenyAdmit {
                        return Err(denied());
                    }
                    if self.behavior == Behavior::HoldAdmit {
                        self.entered.add_permits(1);
                        std::future::pending::<()>().await;
                    }
                }
                // Executor publication succeeds; only the adapter's final
                // publication fence is denied after the guest has returned.
                if operation == PluginHostOperation::Publish
                    && self.behavior == Behavior::DenyLatePublish
                    && self.publications() == 2
                {
                    return Err(denied());
                }
                Ok(PluginHostResponse::Authorized)
            }
            PluginHostRequest::Cancel { .. } => Ok(PluginHostResponse::Authorized),
            other => panic!("pure Component must not request Host effects: {other:?}"),
        }
    }
}

#[derive(Clone)]
struct NoLlm;

#[async_trait]
impl LlmClient for NoLlm {
    async fn stream(&self, _: Request) -> Result<ResponseStream, ClientError> {
        panic!("tools are driven through the Engine registry, without an LLM")
    }

    fn clone_boxed(&self) -> Box<dyn LlmClient> {
        Box::new(self.clone())
    }
}

fn install(feature: PluginToolsFeature) -> (Engine<NoLlm, Mutable, ()>, HookRegistry) {
    let mut engine = Engine::new_annotated(NoLlm);
    let mut hooks = HookRegistryBuilder::new();
    let mut registry = FeatureRegistryBuilder::new();
    registry.add_module(feature);
    let report = registry.install_into_engine(&mut engine, &mut hooks);
    assert!(!report.has_errors(), "{}", report.error_message());
    engine.tool_server_handle().flush_pending();
    (engine, hooks.build())
}

fn context(call: &str, batch: &str) -> ToolExecutionContext {
    ToolExecutionContext::new(call, batch, 0)
        .with_logical_call_id(format!("persisted-entry-{call}"))
}

fn stopping() -> WorkerStoppingContext {
    WorkerStoppingContext {
        invocation: HookInvocationContext {
            workspace_id: Some("workspace-a".into()),
            worker_id: "worker-a".into(),
            session_id: "session-a".into(),
            ..Default::default()
        },
        reason: "component test stop".into(),
    }
}

#[tokio::test]
async fn selected_installations_publish_only_namespaced_tools_and_manifest_schema() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::Allow);
    // The same package is available to A/B/C, but only persisted A/C pins are selected.
    let feature = fixture
        .prepare(
            &["installation_a", "installation_c"],
            owner.clone(),
            "session-a",
        )
        .await;
    let (engine, _) = install(feature);
    let tools = engine.tool_server_handle();
    let definitions = tools.tool_definitions_sorted();
    assert_eq!(
        definitions
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        [TOOL_A, TOOL_C]
    );
    for (installation, name) in [("installation_a", TOOL_A), ("installation_c", TOOL_C)] {
        let (meta, _) = tools.get_tool(name).unwrap();
        assert_eq!(
            meta.input_schema,
            fixture.source.artifact.tools[0].input_schema
        );
        assert_eq!(meta.input_schema["required"], json!(["text"]));
        assert_eq!(meta.input_schema["additionalProperties"], false);
        let origin = meta.origin.unwrap();
        assert_eq!(origin.kind, "plugin");
        assert_eq!(origin.plugin_ref, installation);
        assert_eq!(origin.plugin_id, fixture.source.artifact.package_id);
        assert_eq!(origin.digest, fixture.source.artifact.package_digest);
        assert_eq!(origin.surface, "tool");
    }
    for absent in ["example_echo", "installation_b__example_echo"] {
        assert!(tools.get_tool(absent).is_none());
        assert!(
            tools
                .call_tool(absent, INPUT, context("missing", "batch"))
                .await
                .is_err()
        );
    }
    assert!(
        owner.requests().is_empty(),
        "registration must not admit a call"
    );
    assert_eq!(fixture.source.fetches.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn pure_component_returns_normal_tool_output_and_structured_json() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::Allow);
    let feature = fixture
        .prepare(&["installation_a"], owner.clone(), "session-a")
        .await;
    let (engine, _) = install(feature);
    let tools = engine.tool_server_handle();
    let output = tools
        .call_tool(TOOL_A, INPUT, context("call-echo", "batch-a"))
        .await
        .unwrap();
    assert_eq!(output.summary, "example_echo echoed text");
    assert!(output.attachments.is_empty());
    let output: Value = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
    assert_eq!(
        output["structured"],
        json!({"tool":"example_echo", "text":"hello from a Component"})
    );
    assert!(output["content"].is_null());
    assert_eq!(
        owner.publications(),
        2,
        "executor and adapter each fence publication"
    );
    assert!(
        tools
            .call_tool(TOOL_A, r#"{"text":""}"#, context("empty", "batch-a"))
            .await
            .is_err()
    );
    assert_eq!(
        owner.publications(),
        3,
        "guest error passes the executor fence but never publishes through the adapter"
    );
}

#[tokio::test]
async fn pure_component_owner_denial_applies_to_cold_and_cached_artifacts() {
    for behavior in [Behavior::DenyBegin, Behavior::DenyAdmit] {
        let fixture = Fixture::new();
        for session in ["cold", "cache-hit"] {
            let authorizations = fixture.source.authorizations.load(Ordering::SeqCst);
            let owner = Owner::new(behavior);
            let feature = fixture
                .prepare(&["installation_a"], owner.clone(), session)
                .await;
            let (engine, _) = install(feature);
            let result = engine
                .tool_server_handle()
                .call_tool(TOOL_A, INPUT, context("denied", "batch"))
                .await;
            assert!(
                result.is_err(),
                "{session}: pure code is not an execution lease"
            );
            assert_eq!(owner.consumers().len(), 1);
            assert_eq!(owner.publications(), 0);
            let admits = owner
                .requests()
                .iter()
                .filter(|request| {
                    matches!(
                        request,
                        PluginHostRequest::Authorize {
                            operation: PluginHostOperation::Admit,
                            ..
                        }
                    )
                })
                .count();
            assert_eq!(admits, usize::from(behavior == Behavior::DenyAdmit));
            assert_eq!(
                fixture.source.fetches.load(Ordering::SeqCst),
                1,
                "{session}: second preparation must use the verified cache entry"
            );
            assert!(fixture.source.authorizations.load(Ordering::SeqCst) > authorizations);
        }
    }
}

#[tokio::test]
async fn adapter_late_publish_denial_withholds_successful_guest_output() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::DenyLatePublish);
    let feature = fixture
        .prepare(&["installation_a"], owner.clone(), "session-a")
        .await;
    let (engine, _) = install(feature);
    let error = engine
        .tool_server_handle()
        .call_tool(TOOL_A, INPUT, context("late", "batch"))
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Plugin result publication refused")
    );
    assert_eq!(
        owner.publications(),
        2,
        "guest/executor fence passed before adapter denial"
    );
    assert_eq!(
        owner.consumers().len(),
        1,
        "publication refusal must not retry the call"
    );
}

#[tokio::test]
async fn consumer_identity_uses_host_entry_not_provider_call_or_live_batch() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::Allow);
    // Re-preparation simulates a restored adapter, not just reuse of a live Tool.
    for (session, call, logical, batch) in [
        (
            "session-a",
            "provider-call",
            "host-entry-a",
            "batch-before-restore",
        ),
        (
            "session-a",
            "provider-call",
            "host-entry-a",
            "batch-after-restore",
        ),
        (
            "session-b",
            "provider-call",
            "host-entry-a",
            "batch-after-restore",
        ),
        (
            "session-a",
            "provider-call",
            "host-entry-b",
            "batch-after-restore",
        ),
        (
            "session-a",
            "different-provider-call",
            "host-entry-a",
            "different-batch",
        ),
    ] {
        let feature = fixture
            .prepare(&["installation_a"], owner.clone(), session)
            .await;
        let (engine, _) = install(feature);
        engine
            .tool_server_handle()
            .call_tool(
                TOOL_A,
                INPUT,
                ToolExecutionContext::new(call, batch, 0).with_logical_call_id(logical),
            )
            .await
            .unwrap();
    }
    let consumers = owner.consumers();
    assert_eq!(consumers[0], consumers[1]);
    assert_ne!(consumers[0], consumers[2]);
    assert_ne!(consumers[0], consumers[3]);
    assert_eq!(consumers[0], consumers[4]);
}

#[tokio::test]
async fn missing_or_empty_logical_identity_refuses_owner_admission() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::Allow);
    let feature = fixture
        .prepare(&["installation_a"], owner.clone(), "session-a")
        .await;
    let (engine, _) = install(feature);
    let tools = engine.tool_server_handle();
    for context in [
        ToolExecutionContext::direct(),
        ToolExecutionContext::new("provider-call", "batch", 0),
        ToolExecutionContext::new("provider-call", "batch", 0).with_logical_call_id(""),
    ] {
        let error = tools.call_tool(TOOL_A, INPUT, context).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("committed logical Tool-call identity")
        );
    }
    assert!(
        owner.requests().is_empty(),
        "identity rejection must precede Begin"
    );
}

struct FailInstall;

impl FeatureModule for FailInstall {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin("late-failure", "Injected install failure")
    }

    fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        Err(FeatureInstallError::Install(
            "injected after Plugin staging".into(),
        ))
    }
}

#[tokio::test]
async fn late_install_failure_rolls_back_plugin_tools_and_stop_hook_only() {
    let fixture = Fixture::new();
    let owner = Owner::new(Behavior::Allow);
    let mut engine: Engine<NoLlm, Mutable, ()> = Engine::new_annotated(NoLlm);
    let mut hooks = HookRegistryBuilder::new();
    let mut existing = FeatureRegistryBuilder::new();
    existing.add_module(
        fixture
            .prepare(&["installation_c"], owner.clone(), "existing")
            .await,
    );
    let report = existing.install_into_engine(&mut engine, &mut hooks);
    assert!(!report.has_errors(), "{}", report.error_message());
    engine.tool_server_handle().flush_pending();

    let candidate = fixture
        .prepare(&["installation_a"], owner.clone(), "candidate")
        .await;
    let mut registry = FeatureRegistryBuilder::new();
    registry.add_module(candidate.clone());
    registry.add_module(FailInstall);
    let report = registry.install_into_engine(&mut engine, &mut hooks);
    assert!(report.has_errors());
    assert!(
        report.plan_error.is_none(),
        "failure must happen during installation"
    );
    assert!(
        report
            .reports
            .iter()
            .all(|report| !report.installed && report.installed_tools.is_empty())
    );
    let tools = engine.tool_server_handle();
    tools.flush_pending();
    assert!(tools.get_tool(TOOL_C).is_some());
    assert!(tools.get_tool(TOOL_A).is_none());
    assert!(owner.requests().is_empty());

    hooks.build().on_worker_stopping(&stopping()).await.unwrap();
    assert!(
        tools
            .call_tool(TOOL_C, INPUT, context("existing", "batch"))
            .await
            .is_err(),
        "preexisting stop hook must survive rollback"
    );
    let (retry, _) = install(candidate);
    retry
        .tool_server_handle()
        .call_tool(TOOL_A, INPUT, context("retry", "batch"))
        .await
        .unwrap();
    // A leaked staged stop hook would have stopped the candidate before this call.
    assert_eq!(owner.consumers().len(), 1);
}

#[tokio::test]
async fn active_cancel_and_stopping_send_owner_cancel_and_refuse_reentry() {
    for stop in [false, true] {
        let fixture = Fixture::new();
        let owner = Owner::new(Behavior::HoldAdmit);
        let feature = fixture
            .prepare(&["installation_a"], owner.clone(), "session-a")
            .await;
        let (engine, hooks) = install(feature);
        let tools = engine.tool_server_handle();
        let (_, tool) = tools.get_tool(TOOL_A).unwrap();
        let call_context = context("active", "batch-a");
        let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                tools.call_tool(TOOL_A, INPUT, call_context.clone()),
                async {
                    owner.entered.acquire().await.unwrap().forget();
                    if stop {
                        hooks.on_worker_stopping(&stopping()).await.unwrap();
                    } else {
                        tool.cancel_execution(&call_context).await.unwrap();
                    }
                }
            )
        })
        .await
        .expect("cancellation must settle the active owner wait");
        assert!(result.is_err());
        let requests = owner.requests();
        let cancels = requests
            .iter()
            .filter_map(|request| match request {
                PluginHostRequest::Cancel { invocation_id } => Some(invocation_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            cancels.len(),
            1,
            "local cancellation must also reach the owner"
        );
        let consumers = owner.consumers();
        let PluginConsumer::ToolCall { tool_call_id, .. } = &consumers[0] else {
            panic!("expected a Tool-call consumer")
        };
        assert_eq!(cancels[0], &format!("owner-installation_a-{tool_call_id}"));
        assert_eq!(owner.publications(), 0);

        let begins = consumers.len();
        // A fresh batch cannot revive the cancelled stable consumer. The fake
        // owner denies it; stopping instead fences all new calls locally.
        let retry = tokio::time::timeout(
            Duration::from_secs(5),
            tools.call_tool(TOOL_A, INPUT, context("active", "batch-b")),
        )
        .await
        .expect("cancelled consumer must not reenter admission");
        assert!(retry.is_err());
        assert_eq!(owner.consumers().len(), begins + usize::from(!stop));
        if stop {
            assert!(
                tools
                    .call_tool(TOOL_A, INPUT, context("new-call", "batch-b"))
                    .await
                    .is_err()
            );
            assert_eq!(
                owner.consumers().len(),
                begins,
                "stopped Tool must not ask Begin"
            );
        }
    }
}
