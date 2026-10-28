//! Actual Component -> Wasmtime -> async Host adapter integration, no network.
//! T-791 binaries exercise the unmodified PDK; the adversarial Rust guest is built
//! once with wasm32-unknown-unknown std and offline Cargo, as in the PDK suite.
//! Provider authority/HTTP are substituted here, not the executor or WIT boundary.

use async_trait::async_trait;
use manifest::plugin::{self, CanonicalPluginArtifact, PluginPackageLimits, PluginSourceKind};
use std::{
    any::Any,
    collections::BTreeSet,
    fs,
    path::Path,
    process::Command,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};
use yoi_plugin_runtime::{
    host::{errors, http, logging, *},
    *,
};

const PURE: &str =
    include_str!("../../../resources/plugin/templates/rust-component-tool/plugin.toml");
const HTTPS: &str =
    include_str!("../../../resources/plugin/templates/rust-component-https/plugin.toml");
const PURE_COMPONENT: &[u8] =
    include_bytes!("../../plugin-pdk/tests/fixtures/tool-v2.component.wasm");
const HTTPS_COMPONENT: &[u8] =
    include_bytes!("../../plugin-pdk/tests/fixtures/https-tool-v2.component.wasm");

fn artifact(
    manifest: &str,
    bytes: &[u8],
) -> Result<CanonicalPluginArtifact, plugin::PluginDiagnostic> {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    fs::write(dir.path().join("plugin.component.wasm"), bytes).unwrap();
    let package = plugin::read_plugin_directory(
        dir.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )?;
    plugin::pack_plugin_package(&package, &PluginPackageLimits::default())
}
fn compile(executor: &Executor, artifact: &CanonicalPluginArtifact) -> CompiledTool {
    executor
        .compile(
            &artifact.bytes,
            &artifact.package_digest,
            &artifact.artifact_digest,
            artifact.artifact_size,
        )
        .unwrap()
}
fn guest() -> &'static [u8] {
    static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
    COMPONENT.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let target = root.join("target/plugin-executor-guest");
        let output = Command::new(env!("CARGO"))
            .args([
                "build",
                "--locked",
                "--offline",
                "--release",
                "--target",
                "wasm32-unknown-unknown",
                "--manifest-path",
            ])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guest/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let core = fs::read(target.join("wasm32-unknown-unknown/release/executor_test_guest.wasm"))
            .unwrap();
        wit_component::ComponentEncoder::default()
            .module(&core)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap()
    })
}
fn guest_manifest() -> String {
    HTTPS.replace("https-tool-plugin@", "https-logged-tool-plugin@")
        .replace("\"yoi:host/http@2.0.0\"]", "\"yoi:host/http@2.0.0\", \"yoi:host/logging@2.0.0\"]")
        .replace("requested_grants = [", "requested_grants = [{ kind = \"logging\" }, ")
        .replace("input_schema = { type = \"object\", properties = {}, additionalProperties = false }", "input_schema = { type = \"string\" }")
        .replace("output_schema = { type = \"object\", properties = { status = { type = \"integer\" }, body_bytes = { type = \"integer\" } }, required = [\"status\", \"body_bytes\"], additionalProperties = false }", "output_schema = { type = \"object\", properties = { calls = { type = \"integer\" } }, required = [\"calls\"], additionalProperties = false }")
        .replace("\"failed\"]", "\"failed\", \"typed\", \"host\"]")
}
fn context(component: &CompiledTool, id: &str) -> InvocationContext {
    InvocationContext {
        workspace: "workspace-a".into(),
        installation: "installed-a".into(),
        plan: "exact-plan".into(),
        subject: "worker-a".into(),
        invocation_id: id.into(),
        artifact: component.identity().clone(),
        tool_name: "example_status".into(),
        grants: Grants {
            logging: true,
            connection_slots: BTreeSet::from(["api".into()]),
        },
        deadline: Instant::now() + Duration::from_secs(10),
        cancellation: CancellationToken::new(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Behavior {
    Respond,
    Wait,
    CancelAtResponse,
    DenyAdmission,
    DenySend,
    DenyPublish,
}
struct TestHost {
    behavior: Behavior,
    entered: Semaphore,
    release: Semaphore,
    active: Arc<AtomicUsize>,
    resources: Arc<AtomicUsize>,
    sends: AtomicUsize,
    opens: AtomicUsize,
    logs: Mutex<Vec<String>>,
    admissions: AtomicUsize,
    publications: AtomicUsize,
    tokens: Mutex<Vec<CancellationToken>>,
}
impl TestHost {
    fn new(behavior: Behavior) -> Arc<Self> {
        Arc::new(Self {
            behavior,
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            active: Arc::default(),
            resources: Arc::default(),
            sends: AtomicUsize::new(0),
            opens: AtomicUsize::new(0),
            logs: Mutex::default(),
            admissions: AtomicUsize::new(0),
            publications: AtomicUsize::new(0),
            tokens: Mutex::default(),
        })
    }
    fn released(&self) {
        assert_eq!(self.active.load(Ordering::SeqCst), 0);
        assert_eq!(self.resources.load(Ordering::SeqCst), 0);
        assert!(
            self.tokens
                .lock()
                .unwrap()
                .iter()
                .all(CancellationToken::is_cancelled)
        );
    }
}
struct Counter(Arc<AtomicUsize>);
impl Counter {
    fn new(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(counter.clone())
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Binding {
    id: String,
    _counter: Counter,
}
fn denied() -> errors::HostError {
    errors::HostError {
        code: errors::Code::Denied,
        message: "denied".into(),
        diagnostic_id: "test-only".into(),
        effect: errors::EffectObservation::NotDispatched,
    }
}
#[async_trait]
impl Host for TestHost {
    async fn authorize(
        &self,
        context: &InvocationContext,
        operation: Operation<'_>,
    ) -> Result<(), errors::HostError> {
        let deny = match operation {
            Operation::Admit => {
                self.tokens
                    .lock()
                    .unwrap()
                    .push(context.cancellation.clone());
                self.admissions.fetch_add(1, Ordering::SeqCst);
                self.behavior == Behavior::DenyAdmission
            }
            Operation::Publish => {
                self.publications.fetch_add(1, Ordering::SeqCst);
                self.behavior == Behavior::DenyPublish
            }
            Operation::Send { .. } => self.behavior == Behavior::DenySend,
            _ => false,
        };
        if deny { Err(denied()) } else { Ok(()) }
    }
    async fn open(
        &self,
        context: &InvocationContext,
        _: &str,
    ) -> Result<Arc<dyn Any + Send + Sync>, errors::HostError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(Binding {
            id: context.invocation_id.clone(),
            _counter: Counter::new(&self.resources),
        }))
    }
    async fn send(
        &self,
        context: &InvocationContext,
        binding: &(dyn Any + Send + Sync),
        request: http::Request,
    ) -> Result<http::Response, errors::HostError> {
        assert_eq!(
            binding.downcast_ref::<Binding>().unwrap().id,
            context.invocation_id
        );
        assert_eq!(request.path, "/status");
        assert!(request.timeout_ms <= 15_000);
        let _active = Counter::new(&self.active);
        self.sends.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        if self.behavior == Behavior::Wait {
            self.release.acquire().await.unwrap().forget();
        }
        if self.behavior == Behavior::CancelAtResponse {
            context.cancellation.cancel();
        }
        Ok(http::Response {
            status: 201,
            headers: vec![],
            body: context.invocation_id.as_bytes().to_vec(),
        })
    }
    async fn log(
        &self,
        context: &InvocationContext,
        entry: logging::Entry,
    ) -> Result<(), errors::HostError> {
        assert_eq!(entry.event_code, "count");
        self.logs
            .lock()
            .unwrap()
            .push(context.invocation_id.clone());
        self.entered.add_permits(1);
        Ok(())
    }
}
fn executor(limits: Limits) -> Executor {
    Executor::new(limits).unwrap()
}
fn adversary(ex: &Executor) -> CompiledTool {
    compile(ex, &artifact(&guest_manifest(), guest()).unwrap())
}
async fn call(
    ex: &Executor,
    component: &CompiledTool,
    host: Arc<TestHost>,
    mode: &str,
) -> Result<InvocationResult, ExecutionError> {
    ex.invoke(
        component,
        context(component, mode),
        host,
        "example_status",
        &format!("\"{mode}\""),
        "{}",
    )
    .await
}

#[tokio::test]
async fn pdk_pure_tool_preserves_typed_results_and_requires_live_admission() {
    let ex = executor(Limits::default());
    let component = compile(&ex, &artifact(PURE, PURE_COMPONENT).unwrap());
    let host = TestHost::new(Behavior::Respond);
    let result = ex
        .invoke(
            &component,
            echo_context(&component, "pure"),
            host.clone(),
            "example_echo",
            r#"{"text":"hello"}"#,
            "{}",
        )
        .await
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_str(result.output.structured_json.as_ref().unwrap()).unwrap();
    assert_eq!(value["text"], "hello");
    assert_eq!(host.admissions.load(Ordering::SeqCst), 1);
    assert_eq!(host.publications.load(Ordering::SeqCst), 1);
    let error = ex
        .invoke(
            &component,
            echo_context(&component, "empty"),
            host.clone(),
            "example_echo",
            r#"{"text":""}"#,
            "{}",
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ExecutionError::Guest { code, .. } if code == "invalid_input"));
    for behavior in [Behavior::DenyAdmission, Behavior::DenyPublish] {
        assert!(matches!(
            ex.invoke(
                &component,
                echo_context(&component, "denied"),
                TestHost::new(behavior),
                "example_echo",
                r#"{"text":"x"}"#,
                "{}"
            )
            .await,
            Err(ExecutionError::Host(_))
        ));
    }
}

#[tokio::test]
async fn pdk_https_uses_only_bound_host_capability_and_drops_it() {
    let ex = executor(Limits::default());
    let component = compile(&ex, &artifact(HTTPS, HTTPS_COMPONENT).unwrap());
    let host = TestHost::new(Behavior::Respond);
    let result = ex
        .invoke(
            &component,
            context(&component, "https"),
            host.clone(),
            "example_status",
            "{}",
            "{}",
        )
        .await
        .unwrap();
    let value: serde_json::Value =
        serde_json::from_str(result.output.structured_json.as_ref().unwrap()).unwrap();
    assert_eq!(value["status"], 201);
    assert_eq!(host.sends.load(Ordering::SeqCst), 1);
    host.released();
    let denied = TestHost::new(Behavior::DenySend);
    assert!(
        ex.invoke(
            &component,
            context(&component, "no-send"),
            denied.clone(),
            "example_status",
            "{}",
            "{}"
        )
        .await
        .is_err()
    );
    assert_eq!(denied.sends.load(Ordering::SeqCst), 0);
    denied.released();
}

#[tokio::test]
async fn fresh_store_isolates_memory_and_logs_are_bounded_not_model_output() {
    let ex = executor(Limits {
        log_bytes: 40,
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Respond);
    for _ in 0..2 {
        let result = call(&ex, &component, host.clone(), "logging")
            .await
            .unwrap();
        assert_eq!(
            result.output.structured_json.as_deref(),
            Some("{\"calls\":1}")
        );
        assert_eq!(result.dropped_logs, 2);
        assert_eq!(result.output.summary, "logging");
    }
    assert_eq!(host.logs.lock().unwrap().len(), 2);
    for (mode, code) in [("typed", "typed"), ("unknown", "plugin-failed")] {
        assert!(
            matches!(call(&ex, &component, host.clone(), mode).await, Err(ExecutionError::Guest { code: actual, .. }) if actual == code)
        );
    }
}

#[tokio::test]
async fn undeclared_tool_and_invalid_input_are_rejected_before_admission() {
    let ex = executor(Limits::default());
    let component = compile(&ex, &artifact(PURE, PURE_COMPONENT).unwrap());
    let host = TestHost::new(Behavior::Respond);
    for (tool, input) in [
        ("missing", "{}"),
        ("example_echo", "{}"),
        ("example_echo", "not-json"),
        ("example_echo", r#"{"text":3}"#),
    ] {
        assert!(matches!(
            ex.invoke(
                &component,
                echo_context(&component, "bad"),
                host.clone(),
                tool,
                input,
                "{}"
            )
            .await,
            Err(ExecutionError::InvalidInput(_))
        ));
    }
    assert_eq!(host.admissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_schema_output_and_oversized_results_never_publish_success() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    for mode in ["bad-json", "bad-schema", "output", "error-output"] {
        assert!(matches!(
            call(&ex, &component, TestHost::new(Behavior::Respond), mode).await,
            Err(ExecutionError::InvalidOutput(_))
        ));
    }
}

#[tokio::test]
async fn trap_fuel_memory_and_resource_exhaustion_leave_next_call_fresh() {
    let ex = executor(Limits {
        memory_bytes: 4 * 1024 * 1024,
        resources: 2,
        fuel: 200_000,
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Respond);
    for mode in ["trap", "loop", "memory", "resources"] {
        assert!(
            matches!(
                call(&ex, &component, host.clone(), mode).await,
                Err(ExecutionError::Trap(_))
            ),
            "{mode}"
        );
        host.released();
        assert_eq!(
            call(&ex, &component, host.clone(), "fresh")
                .await
                .unwrap()
                .output
                .structured_json
                .as_deref(),
            Some("{\"calls\":1}")
        );
    }
}

#[tokio::test]
async fn stale_and_foreign_resource_representations_cannot_dispatch_http() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    for mode in ["stale", "foreign"] {
        let host = TestHost::new(Behavior::Respond);
        assert!(
            matches!(
                call(&ex, &component, host.clone(), mode).await,
                Err(ExecutionError::Trap(_))
            ),
            "{mode}"
        );
        assert_eq!(host.sends.load(Ordering::SeqCst), 0);
        host.released();
    }
}

#[tokio::test]
async fn cancellation_during_host_wait_drops_future_and_call_resources() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Wait);
    let ctx = context(&component, "cancel-wait");
    let cancellation = ctx.cancellation.clone();
    let invocation = ex.invoke(
        &component,
        ctx,
        host.clone(),
        "example_status",
        "\"http\"",
        "{}",
    );
    let cancellation_task = async {
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        cancellation.cancel();
    };
    let (result, ()) = tokio::join!(invocation, cancellation_task);
    assert!(matches!(result, Err(ExecutionError::Cancelled)));
    host.released();
    host.release.add_permits(1);
    assert_eq!(host.publications.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn host_wait_uses_common_wall_deadline_and_drops_resources() {
    let ex = executor(Limits {
        wall_time: Duration::from_secs(1),
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Wait);
    let invocation = call(&ex, &component, host.clone(), "http");
    let clock = async {
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        tokio::time::advance(Duration::from_secs(2)).await;
    };
    let (result, ()) = tokio::join!(invocation, clock);
    assert!(matches!(result, Err(ExecutionError::Deadline)));
    host.released();
}

#[tokio::test]
async fn cancelled_late_response_is_not_published() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::CancelAtResponse);
    assert!(matches!(
        call(&ex, &component, host.clone(), "http").await,
        Err(ExecutionError::Cancelled)
    ));
    assert_eq!(host.publications.load(Ordering::SeqCst), 0);
    host.released();
}

#[tokio::test]
async fn concurrent_invocations_never_mix_identity_memory_or_capabilities() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Wait);
    let a = ex.invoke(
        &component,
        context(&component, "a"),
        host.clone(),
        "example_status",
        "\"http\"",
        "{}",
    );
    let b = ex.invoke(
        &component,
        context(&component, "b"),
        host.clone(),
        "example_status",
        "\"http\"",
        "{}",
    );
    let release = async {
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        host.release.add_permits(2);
    };
    let (a, b, ()) = tokio::join!(a, b, release);
    assert_eq!(a.unwrap().output.summary, "a");
    assert_eq!(b.unwrap().output.summary, "b");
    host.released();
}

#[test]
fn wrong_import_or_interface_version_is_rejected_by_shared_validator() {
    assert!(artifact(PURE, HTTPS_COMPONENT).is_err());
    assert!(artifact(&HTTPS.replace("2.0.0", "1.0.0"), HTTPS_COMPONENT).is_err());
    let ex = executor(Limits::default());
    let package = artifact(PURE, PURE_COMPONENT).unwrap();
    assert!(matches!(
        ex.compile(
            &package.bytes,
            &package.artifact_digest,
            &package.package_digest,
            package.artifact_size
        ),
        Err(ExecutionError::Artifact(_))
    ));
}

fn echo_context(component: &CompiledTool, id: &str) -> InvocationContext {
    let mut c = context(component, id);
    c.tool_name = "example_echo".into();
    c
}

fn generated_component(transform: impl FnOnce(String) -> String) -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::default();
    let package = resolve
        .push_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/plugin/wit"))
        .unwrap()
        .0;
    let world = resolve
        .select_world(&[package], Some("https-logged-tool-plugin"))
        .unwrap();
    let bytes =
        wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
    let wat = transform(wasmprinter::print_bytes(bytes).unwrap());
    let mut core = wat::parse_str(wat).unwrap();
    wit_component::embed_component_metadata(
        &mut core,
        &resolve,
        world,
        wit_component::StringEncoding::UTF8,
    )
    .unwrap();
    wit_component::ComponentEncoder::default()
        .module(&core)
        .unwrap()
        .validate(true)
        .encode()
        .unwrap()
}
fn startup_component(logging: bool) -> Vec<u8> {
    generated_component(|wat| {
        // Mutate only a generated initializer, retaining exact canonical types.
        // These calls run during instantiation, not the Tool export.
        let body = if logging {
            "i32.const 0 ".repeat(7) + "i32.const 128 call 3"
        } else {
            "i32.const 0 i32.const 0 i32.const 128 call 0".into()
        };
        let needle = "(func (;7;) (type 6))";
        assert!(wat.contains(needle));
        wat.replace("(memory (;0;) 0)", "(memory (;0;) 1)")
            .replace(needle, &format!("(func (;7;) (type 6) {body})"))
    })
}

#[tokio::test]
async fn instantiate_without_export_context_rejects_connections_and_logging() {
    let ex = executor(Limits::default());
    for logging in [false, true] {
        let component = compile(
            &ex,
            &artifact(&guest_manifest(), &startup_component(logging)).unwrap(),
        );
        let host = TestHost::new(Behavior::Respond);
        let error = call(&ex, &component, host.clone(), "fresh")
            .await
            .unwrap_err();
        assert!(matches!(error, ExecutionError::Trap(_)));
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
        assert_eq!(host.sends.load(Ordering::SeqCst), 0);
        assert!(host.logs.lock().unwrap().is_empty());
        host.released();
    }
}

#[tokio::test]
async fn host_context_cannot_admit_another_artifact_or_tool() {
    let ex = executor(Limits::default());
    let component = compile(&ex, &artifact(PURE, PURE_COMPONENT).unwrap());
    let host = TestHost::new(Behavior::Respond);
    for changed_artifact in [false, true] {
        let mut context = echo_context(&component, "bad-bound-context");
        if changed_artifact {
            context.artifact.package_digest.push('x');
        } else {
            context.tool_name = "different-tool".into();
        }
        assert!(matches!(
            ex.invoke(
                &component,
                context,
                host.clone(),
                "example_echo",
                r#"{"text":"x"}"#,
                "{}"
            )
            .await,
            Err(ExecutionError::InvalidInput(_))
        ));
    }
    assert_eq!(host.admissions.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn initial_memory_table_and_instance_limits_apply_at_instantiation() {
    for limits in [
        Limits {
            memory_bytes: 0,
            ..Limits::default()
        },
        Limits {
            table_elements: 0,
            ..Limits::default()
        },
        Limits {
            instances: 0,
            ..Limits::default()
        },
        Limits {
            memories: 0,
            ..Limits::default()
        },
        Limits {
            tables: 0,
            ..Limits::default()
        },
    ] {
        let ex = executor(limits);
        let component = adversary(&ex);
        assert!(matches!(
            call(&ex, &component, TestHost::new(Behavior::Respond), "fresh").await,
            Err(ExecutionError::Trap(_))
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn running_cpu_loop_obeys_wall_deadline_without_consuming_all_fuel() {
    let ex = executor(Limits {
        wall_time: Duration::from_secs(1),
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Respond);
    let invocation = call(&ex, &component, host.clone(), "wall-loop");
    let clock = async {
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        tokio::time::advance(Duration::from_secs(2)).await;
    };
    let (result, ()) = tokio::join!(invocation, clock);
    assert!(matches!(result, Err(ExecutionError::Deadline)));
    assert_eq!(host.logs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn missing_grants_and_host_operation_ceiling_never_dispatch() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    for mode in ["logging", "http"] {
        let host = TestHost::new(Behavior::Respond);
        let mut context = context(&component, mode);
        context.grants = Grants::default();
        assert!(matches!(
            ex.invoke(
                &component,
                context,
                host.clone(),
                "example_status",
                &format!("\"{mode}\""),
                "{}"
            )
            .await,
            Err(ExecutionError::Guest { .. })
        ));
        assert_eq!(host.opens.load(Ordering::SeqCst), 0);
        assert!(host.logs.lock().unwrap().is_empty());
    }
    let ex = executor(Limits {
        host_operations: 1,
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Respond);
    assert!(call(&ex, &component, host.clone(), "http").await.is_err());
    assert_eq!(host.opens.load(Ordering::SeqCst), 1);
    assert_eq!(host.sends.load(Ordering::SeqCst), 0);
    host.released();
}

#[tokio::test]
async fn oversized_input_and_config_are_rejected_before_admission() {
    let ex = executor(Limits {
        payload_bytes: 32,
        config_bytes: 16,
        ..Limits::default()
    });
    let component = compile(&ex, &artifact(PURE, PURE_COMPONENT).unwrap());
    let host = TestHost::new(Behavior::Respond);
    for (input, config) in [
        (format!(r#"{{"text":"{}"}}"#, "x".repeat(32)), "{}".into()),
        (
            r#"{"text":"ok"}"#.into(),
            format!(r#"{{"x":"{}"}}"#, "x".repeat(16)),
        ),
    ] {
        assert!(matches!(
            ex.invoke(
                &component,
                echo_context(&component, "size"),
                host.clone(),
                "example_echo",
                &input,
                &config
            )
            .await,
            Err(ExecutionError::InvalidInput(_))
        ));
    }
    assert_eq!(host.admissions.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn http_wait_has_its_own_shorter_deadline_without_extending_the_call() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Wait);
    let mut context = context(&component, "request-timeout");
    context.deadline = Instant::now() + Duration::from_secs(30);
    let invocation = ex.invoke(
        &component,
        context,
        host.clone(),
        "example_status",
        "\"http\"",
        "{}",
    );
    let clock = async {
        tokio::time::timeout(Duration::from_secs(5), host.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        tokio::time::advance(Duration::from_secs(16)).await;
    };
    let (result, ()) = tokio::join!(invocation, clock);
    assert!(
        matches!(&result, Err(ExecutionError::Guest { code, message }) if code == "host" && message == "deadline-exceeded:indeterminate"),
        "{result:?}"
    );
    host.released();
}

#[tokio::test]
async fn bounded_http_response_is_not_truncated_into_success() {
    let ex = executor(Limits {
        http_body_bytes: 1,
        ..Limits::default()
    });
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Respond);
    let result = call(&ex, &component, host.clone(), "http").await;
    assert!(
        matches!(&result, Err(ExecutionError::Guest { code, message }) if code == "host" && message == "quota-exceeded:response-observed"),
        "{result:?}"
    );
    assert_eq!(host.sends.load(Ordering::SeqCst), 1);
    host.released();
}

#[tokio::test]
async fn dropping_invoke_cancels_only_its_child_lifetime_and_reclaims_store() {
    let ex = executor(Limits::default());
    let component = adversary(&ex);
    let host = TestHost::new(Behavior::Wait);
    let context = context(&component, "dropped");
    let parent_token = context.cancellation.clone();
    let mut invocation = Box::pin(ex.invoke(
        &component,
        context,
        host.clone(),
        "example_status",
        "\"http\"",
        "{}",
    ));
    tokio::select! {
        result = &mut invocation => panic!("unexpected completion: {result:?}"),
        entered = tokio::time::timeout(Duration::from_secs(5), host.entered.acquire()) => entered.unwrap().unwrap().forget(),
    }
    drop(invocation);
    assert!(!parent_token.is_cancelled());
    host.released();
}

#[tokio::test]
async fn multiple_memories_and_tables_cannot_multiply_store_budgets() {
    for memory in [false, true] {
        let ex = executor(Limits {
            memory_bytes: 4 * 1024 * 1024,
            table_elements: 100,
            ..Limits::default()
        });
        let bytes = generated_component(|wat| {
            wat.replace(
                "(memory (;0;) 0)",
                if memory {
                    "(memory (;0;) 40) (memory 40)"
                } else {
                    "(memory (;0;) 0) (table 80 funcref) (table 80 funcref)"
                },
            )
        });
        let component = compile(&ex, &artifact(&guest_manifest(), &bytes).unwrap());
        let error = call(&ex, &component, TestHost::new(Behavior::Respond), "fresh")
            .await
            .unwrap_err();
        let expected = if memory {
            "aggregate memory budget exceeded"
        } else {
            "aggregate table budget exceeded"
        };
        assert!(
            matches!(&error, ExecutionError::Trap(message) if message.contains(expected)),
            "{error:?}"
        );
    }
}

#[test]
fn operational_limits_cannot_raise_the_supported_ceiling() {
    assert!(
        Executor::new(Limits {
            wall_time: Duration::MAX,
            ..Limits::default()
        })
        .is_err()
    );
}

fn private_resource_component(nested_depth: usize, allocate: bool) -> Vec<u8> {
    let mut payload = "(type $private (resource (rep i32)))".to_string();
    if allocate {
        // The i32 drop discards a representation, not the live resource handle.
        // A core start creates handles during Component instantiation without
        // changing the T-791 fixture's public imports or exports.
        payload.push_str(&format!(
            r#"
            (core func $new (canon resource.new $private))
            (core module $allocate
                (import "r" "new" (func $new (param i32) (result i32)))
                (func $start {})
                (start $start)
            )
            (core instance $resource-funcs (export "new" (func $new)))
            (core instance $allocated (instantiate $allocate (with "r" (instance $resource-funcs))))
        "#,
            "i32.const 7 call $new drop ".repeat(64)
        ));
    }
    for _ in 0..nested_depth {
        payload = format!("(component $nested {payload}) (instance (instantiate $nested))");
    }
    let wat = wasmprinter::print_bytes(PURE_COMPONENT).unwrap();
    let end = wat.rfind(')').unwrap();
    wat::parse_str(format!("{}{}{}", &wat[..end], payload, &wat[end..])).unwrap()
}

#[tokio::test]
async fn private_and_nested_component_resources_fail_closed_before_instantiation() {
    for resources in [0, Limits::default().resources] {
        let ex = executor(Limits {
            resources,
            ..Limits::default()
        });
        let fresh = compile(&ex, &artifact(PURE, PURE_COMPONENT).unwrap());
        let host = TestHost::new(Behavior::Respond);
        for nested_depth in 0..=2 {
            for allocate in [false, true] {
                let bytes = private_resource_component(nested_depth, allocate);
                // The canonical package/ABI is valid. Reject only the unsupported
                // execution policy, after verification and before a Store exists.
                let package = artifact(PURE, &bytes).unwrap();
                let error = ex
                    .compile(
                        &package.bytes,
                        &package.package_digest,
                        &package.artifact_digest,
                        package.artifact_size,
                    )
                    .err()
                    .expect("private resources must be rejected");
                assert!(
                    matches!(&error, ExecutionError::UnsupportedComponent(message) if message.contains("guest-defined Component resources")),
                    "depth={nested_depth}, resources={resources}, allocate={allocate}: {error:?}"
                );
                host.released();
            }
        }
        assert_eq!(host.admissions.load(Ordering::SeqCst), 0);
        let result = ex
            .invoke(
                &fresh,
                echo_context(&fresh, "fresh-after-rejection"),
                host.clone(),
                "example_echo",
                r#"{"text":"fresh"}"#,
                "{}",
            )
            .await
            .unwrap();
        let structured: serde_json::Value =
            serde_json::from_str(result.output.structured_json.as_deref().unwrap()).unwrap();
        assert_eq!(structured["text"], "fresh");
        host.released();
    }
}
