//! Authored Rust -> PDK -> validated Component -> fresh Wasmtime Store -> OwnerHost.
//! The deterministic owner replaces storage/network, not the execution adapter;
//! transaction/lease/live policy guarantees are tested by the domain owner suites.
use async_trait::async_trait;
use manifest::plugin::{self, PluginPackageLimits, PluginSourceKind};
use server_api::*;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    process::Command,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;
use yoi_plugin_runtime::{
    host::{CancellationToken, Grants, InvocationContext},
    owner::{OwnerHost, PluginHostOwner},
    *,
};

fn component() -> &'static [u8] {
    static COMPONENT: OnceLock<Vec<u8>> = OnceLock::new();
    COMPONENT.get_or_init(|| {
        let target =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/plugin-service-fixture/guest");
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
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/service-guest/Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .output()
            .unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            target.parent().unwrap().join("guest-build.log"),
            &output.stderr,
        )
        .unwrap();
        assert!(
            output.status.success(),
            "Service fixture requires wasm32 std + lld on PATH: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let core = fs::read(target.join("wasm32-unknown-unknown/release/service_test_guest.wasm"))
            .unwrap();
        let bytes = wit_component::ComponentEncoder::default()
            .module(&core)
            .unwrap()
            .validate(true)
            .encode()
            .unwrap();
        fs::write(
            target.parent().unwrap().join("service.component.wasm"),
            &bytes,
        )
        .unwrap();
        bytes
    })
}
const MANIFEST: &str = include_str!("service-guest/plugin.toml");
fn package(manifest: &str) -> Result<plugin::CanonicalPluginArtifact, plugin::PluginDiagnostic> {
    package_bytes(manifest, component())
}
fn package_bytes(
    manifest: &str,
    bytes: &[u8],
) -> Result<plugin::CanonicalPluginArtifact, plugin::PluginDiagnostic> {
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/plugin-service-fixture");
    fs::create_dir_all(&target).unwrap();
    let dir = tempfile::tempdir_in(target).unwrap();
    fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    fs::write(dir.path().join("plugin.component.wasm"), bytes).unwrap();
    let materialized = plugin::read_plugin_directory(
        dir.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )?;
    plugin::pack_plugin_package(&materialized, &PluginPackageLimits::default())
}
fn compile(ex: &Executor) -> CompiledService {
    let p = package(MANIFEST).unwrap();
    ex.compile_service(
        &p.bytes,
        &p.package_digest,
        &p.artifact_digest,
        p.artifact_size,
    )
    .unwrap()
}
#[derive(Clone, Copy, Default)]
enum Fault {
    #[default]
    None,
    HttpUnknown,
    DataUnknown,
    HttpWait,
    DataWait,
    LookupUnavailable,
    LookupExpired,
    ReadDenied,
}
#[derive(Default)]
struct State {
    calls: Vec<PluginHostRequest>,
    writes: usize,
    sends: usize,
    receipts: BTreeMap<String, PluginDataReceipt>,
    http: BTreeMap<String, PluginHttpResponse>,
}
struct Owner {
    state: Mutex<State>,
    fault: Fault,
    waiting: tokio::sync::Notify,
}
impl Owner {
    fn new(fault: Fault) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            fault,
            waiting: tokio::sync::Notify::new(),
        })
    }
}
fn rejected(code: PluginHostErrorCode, effect: PluginEffectObservation) -> PluginHostError {
    PluginHostError {
        code,
        effect,
        diagnostic_id: "test".into(),
    }
}
#[async_trait]
impl PluginHostOwner for Owner {
    async fn call(
        &self,
        request: PluginHostRequest,
    ) -> Result<PluginHostResponse, PluginHostError> {
        if (matches!(self.fault, Fault::HttpWait)
            && matches!(request, PluginHostRequest::Send { .. }))
            || (matches!(self.fault, Fault::DataWait)
                && matches!(request, PluginHostRequest::DataMutate { .. }))
        {
            self.waiting.notify_one();
            return std::future::pending().await;
        }
        let mut s = self.state.lock().unwrap();
        s.calls.push(request.clone());
        Ok(match request {
            PluginHostRequest::Begin { .. } => PluginHostResponse::Invocation {
                invocation_id: "stable-service-invocation".into(),
                expires_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64
                    + 60_000,
            },
            PluginHostRequest::Authorize { .. } => PluginHostResponse::Authorized,
            PluginHostRequest::Log { .. } => PluginHostResponse::Logged,
            PluginHostRequest::ResolveTicket { ticket_ref, .. } => {
                assert_eq!(ticket_ref, "T-796");
                PluginHostResponse::TicketResolved {
                    ticket_id: "canonical-ticket".into(),
                }
            }
            PluginHostRequest::ReadTicket { .. } => {
                if matches!(self.fault, Fault::ReadDenied) {
                    return Err(rejected(
                        PluginHostErrorCode::Denied,
                        PluginEffectObservation::NotDispatched,
                    ));
                }
                PluginHostResponse::TicketRead {
                    snapshot: PluginTicketSnapshot {
                        title: "note".into(),
                        case_state: PluginTicketCaseState::Open,
                        content_digest: "digest".into(),
                        body: "body".into(),
                        truncated: false,
                    },
                }
            }
            PluginHostRequest::DataLookupMutation { operation_key, .. } => {
                if matches!(self.fault, Fault::LookupUnavailable) {
                    return Err(rejected(
                        PluginHostErrorCode::Unavailable,
                        PluginEffectObservation::NotDispatched,
                    ));
                }
                PluginHostResponse::DataMutationLookup {
                    outcome: if matches!(self.fault, Fault::LookupExpired) {
                        PluginDataMutationLookup::Expired
                    } else {
                        s.receipts
                            .get(&operation_key)
                            .cloned()
                            .map(PluginDataMutationLookup::Committed)
                            .unwrap_or(PluginDataMutationLookup::NotRecorded)
                    },
                }
            }
            PluginHostRequest::DataGet { .. } => PluginHostResponse::DataCell {
                cell: PluginDataCell::NeverWritten,
            },
            PluginHostRequest::DataMutate { operation_key, .. } => {
                if matches!(self.fault, Fault::DataUnknown) {
                    return Err(rejected(
                        PluginHostErrorCode::Indeterminate,
                        PluginEffectObservation::Indeterminate,
                    ));
                }
                s.writes += 1;
                let receipt = PluginDataReceipt {
                    mutation_id: "write-1".into(),
                    result_digest: "result".into(),
                };
                assert!(s.receipts.insert(operation_key, receipt.clone()).is_none());
                PluginHostResponse::DataMutated { receipt }
            }
            PluginHostRequest::Open { .. } => PluginHostResponse::Opened,
            PluginHostRequest::Send { request, .. } => {
                if matches!(self.fault, Fault::HttpUnknown) {
                    return Err(rejected(
                        PluginHostErrorCode::Indeterminate,
                        PluginEffectObservation::Indeterminate,
                    ));
                }
                let response = if let Some(response) = s.http.get(&request.effect_key) {
                    response.clone()
                } else {
                    s.sends += 1;
                    let response = PluginHttpResponse {
                        status: 200,
                        headers: vec![],
                        body: b"ok".to_vec(),
                    };
                    s.http.insert(request.effect_key, response.clone());
                    response
                };
                PluginHostResponse::Response { response }
            }
            _ => {
                return Err(rejected(
                    PluginHostErrorCode::Unsupported,
                    PluginEffectObservation::NotDispatched,
                ));
            }
        })
    }
}
fn context(c: &CompiledService) -> InvocationContext {
    let namespace = ("notes".into(), "note".into(), "1".into());
    InvocationContext {
        workspace: "workspace".into(),
        installation: "installation".into(),
        plan: "plan".into(),
        subject: "worker".into(),
        invocation_id: "ignored-before-begin".into(),
        artifact: c.identity().clone(),
        tool_name: "ticket_events".into(),
        grants: Grants {
            logging: true,
            connection_slots: ["api".into()].into(),
            ticket_events: true,
            ticket_read: true,
            ticket_data_read: [namespace.clone()].into(),
            ticket_data_write: [namespace].into(),
        },
        deadline: Instant::now() + Duration::from_secs(10),
        cancellation: CancellationToken::new(),
    }
}
async fn host(c: &CompiledService, owner: Arc<Owner>) -> Arc<OwnerHost> {
    Arc::new(
        OwnerHost::begin(
            owner,
            context(c),
            PluginConsumer::Consumer {
                consumer_id: "consumer".into(),
                event_id: "event".into(),
                export_name: "ticket_events".into(),
            },
        )
        .await
        .unwrap(),
    )
}
fn envelope() -> PluginTicketEventEnvelope {
    PluginTicketEventEnvelope {
        event_id: "event".into(),
        event_type: PluginTicketEventType::Changed,
        version: 1,
        occurred_at: "2026-10-30T00:00:00Z".into(),
        ticket_id: "T-796".into(),
        domain_event_ref: "domain-event".into(),
        content_digest: "digest".into(),
        correlation_id: "correlation".into(),
        causation_id: None,
        attempt: 1,
    }
}
#[tokio::test]
async fn authored_service_recovers_with_receipt_lookup_before_rebuilding_a_mutation() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    let owner = Owner::new(Fault::None);
    let host = host(&c, owner.clone()).await;
    assert!(matches!(
        ex.service_start(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            r#"{"mode":"normal"}"#
        )
        .await
        .outcome,
        Ok(ServiceOutput::Started)
    ));
    for attempt in 1..=2 {
        let mut e = envelope();
        e.attempt = attempt;
        let result = ex
            .service_event(
                &c,
                host.context().clone(),
                host.clone(),
                "ticket_events",
                &e,
                r#"{"mode":"normal"}"#,
            )
            .await;
        assert!(
            matches!(
                result.outcome,
                Ok(ServiceOutput::Event(PluginServiceEventResult::Ack))
            ),
            "{result:?}"
        );
        assert!(matches!(
            result.effect,
            host::errors::EffectObservation::ResponseObserved
        ));
    }
    assert!(matches!(
        ex.service_stop(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            "normal"
        )
        .await
        .outcome,
        Ok(ServiceOutput::Stopped)
    ));
    let state = owner.state.lock().unwrap();
    assert_eq!(state.writes, 1);
    assert_eq!(state.sends, 1);
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|r| matches!(r, PluginHostRequest::DataGet { .. }))
            .count(),
        1
    );
    assert_eq!(
        state
            .calls
            .iter()
            .filter(|r| matches!(r, PluginHostRequest::DataLookupMutation { .. }))
            .count(),
        2
    );
}
#[tokio::test]
async fn lifecycle_hooks_allow_logging_but_never_open_resolve_read_data_or_http() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    let owner = Owner::new(Fault::None);
    let host = host(&c, owner.clone()).await;
    for mode in ["open", "resolve", "read", "data", "http"] {
        let config = format!(r#"{{"mode":"{mode}"}}"#);
        let start = ex
            .service_start(
                &c,
                host.context().clone(),
                host.clone(),
                "ticket_events",
                &config,
            )
            .await;
        let stop = ex
            .service_stop(
                &c,
                host.context().clone(),
                host.clone(),
                "ticket_events",
                mode,
            )
            .await;
        assert!(
            matches!(start.outcome, Err(ExecutionError::Trap(_))),
            "{mode}: {start:?}"
        );
        assert!(
            matches!(stop.outcome, Err(ExecutionError::Trap(_))),
            "{mode}: {stop:?}"
        );
    }
    let s = owner.state.lock().unwrap();
    assert!(s.calls.iter().all(|r| matches!(
        r,
        PluginHostRequest::Begin { .. } | PluginHostRequest::Authorize { .. }
    )));
}
#[tokio::test]
async fn guest_ack_cannot_hide_unknown_http_data_or_failed_receipt_lookup() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    for fault in [
        Fault::HttpUnknown,
        Fault::DataUnknown,
        Fault::LookupUnavailable,
        Fault::LookupExpired,
    ] {
        let host = host(&c, Owner::new(fault)).await;
        let result = ex
            .service_event(
                &c,
                host.context().clone(),
                host.clone(),
                "ticket_events",
                &envelope(),
                r#"{"mode":"swallow-unknown"}"#,
            )
            .await;
        assert!(
            matches!(
                result.outcome,
                Ok(ServiceOutput::Event(PluginServiceEventResult::Ack))
            ),
            "{result:?}"
        );
        assert!(
            matches!(
                result.effect,
                host::errors::EffectObservation::Indeterminate
            ),
            "{result:?}"
        );
    }
}
#[tokio::test]
async fn cancellation_during_effect_wait_preserves_unknown_observation() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    for fault in [Fault::HttpWait, Fault::DataWait] {
        let owner = Owner::new(fault);
        let host = host(&c, owner.clone()).await;
        let cancellation = host.context().cancellation.clone();
        let e = envelope();
        let (result, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                ex.service_event(
                    &c,
                    host.context().clone(),
                    host.clone(),
                    "ticket_events",
                    &e,
                    r#"{"mode":"normal"}"#
                ),
                async {
                    owner.waiting.notified().await;
                    cancellation.cancel();
                }
            )
        })
        .await
        .expect("effect was not dispatched");
        assert!(
            matches!(result.outcome, Err(ExecutionError::Cancelled)),
            "{result:?}"
        );
        assert!(
            matches!(
                result.effect,
                host::errors::EffectObservation::Indeterminate
            ),
            "{result:?}"
        );
    }
}
#[tokio::test]
async fn deleted_metadata_never_resolves_a_ticket_and_live_read_denial_remains_a_failure() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    let owner = Owner::new(Fault::ReadDenied);
    let host = host(&c, owner.clone()).await;
    let result = ex
        .service_event(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            &envelope(),
            r#"{"mode":"normal"}"#,
        )
        .await;
    assert!(matches!(
        result.outcome,
        Ok(ServiceOutput::Event(
            PluginServiceEventResult::RetryAfter { .. }
        ))
    ));
    let before = owner.state.lock().unwrap().calls.len();
    let mut e = envelope();
    e.event_type = PluginTicketEventType::Deleted;
    e.content_digest.clear();
    let result = ex
        .service_event(
            &c,
            host.context().clone(),
            host,
            "ticket_events",
            &e,
            r#"{"mode":"normal"}"#,
        )
        .await;
    assert!(matches!(
        result.outcome,
        Ok(ServiceOutput::Event(PluginServiceEventResult::Ack))
    ));
    assert!(
        owner.state.lock().unwrap().calls[before..]
            .iter()
            .all(|r| matches!(r, PluginHostRequest::Authorize { .. }))
    );
}
#[test]
fn service_manifest_rejects_old_world_subscription_and_grant_mismatches() {
    package(MANIFEST).unwrap();
    for bad in [
        MANIFEST.replace("@2.2.0", "@2.1.0"),
        MANIFEST.replace("ticket_changed", "arbitrary_event"),
        MANIFEST.replace("kind = \"ticket_events\"", "kind = \"ticket_read\""),
        MANIFEST.replace(
            "ticket_changed\", \"ticket_deleted",
            "ticket_changed\", \"ticket_changed",
        ),
        MANIFEST.replace("surfaces = [\"service\"]", "surfaces = [\"tool\"]"),
    ] {
        assert!(package(&bad).is_err());
    }
}

#[tokio::test]
async fn service_host_and_guest_limits_fail_before_publication() {
    let ex = Executor::new(Limits::default()).unwrap();
    let c = compile(&ex);
    let host = host(&c, Owner::new(Fault::None)).await;
    let invalid = ex
        .service_event(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            &envelope(),
            r#"{"mode":"oversize-retry"}"#,
        )
        .await;
    assert!(matches!(
        invalid.outcome,
        Err(ExecutionError::InvalidOutput(_))
    ));
    let mut ctx = host.context().clone();
    ctx.grants.ticket_events = false;
    let denied = ex
        .service_event(
            &c,
            ctx,
            host.clone(),
            "ticket_events",
            &envelope(),
            r#"{"mode":"normal"}"#,
        )
        .await;
    assert!(matches!(
        denied.outcome,
        Err(ExecutionError::InvalidInput(_))
    ));
    let mut e = envelope();
    e.version = 2;
    assert!(matches!(
        ex.service_event(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            &e,
            "{}"
        )
        .await
        .outcome,
        Err(ExecutionError::InvalidInput(_))
    ));
    e = envelope();
    e.correlation_id = "x".repeat(65536);
    assert!(matches!(
        ex.service_event(
            &c,
            host.context().clone(),
            host.clone(),
            "ticket_events",
            &e,
            "{}"
        )
        .await
        .outcome,
        Err(ExecutionError::InvalidInput(_))
    ));
    let ctx = host.context().clone();
    ctx.cancellation.cancel();
    assert!(matches!(
        ex.service_start(&c, ctx, host.clone(), "ticket_events", "{}")
            .await
            .outcome,
        Err(ExecutionError::Cancelled)
    ));
}

fn startup_component(logging: bool) -> Vec<u8> {
    let mut resolve = wit_parser::Resolve::default();
    let (package, _) = resolve
        .push_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/plugin/wit-service"))
        .unwrap();
    let world = resolve
        .select_world(&[package], Some("ticket-service-plugin"))
        .unwrap();
    let core = wit_component::dummy_module(&resolve, world, wit_parser::ManglingAndAbi::Standard32);
    let wat = wasmprinter::print_bytes(core).unwrap();
    let init = wat
        .lines()
        .find(|l| l.contains("(export \"cm32p2_initialize\""))
        .unwrap();
    let index = init
        .split("(func ")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let needle = wat
        .lines()
        .find(|l| l.contains(&format!("(func (;{index};)")))
        .unwrap();
    assert!(needle.trim_end().ends_with("))"), "{needle}");
    let import = wat
        .lines()
        .find(|l| {
            l.contains(if logging {
                "yoi:host/logging@2"
            } else {
                "yoi:host/connections@2"
            }) && l.contains(if logging { "\"log\"" } else { "\"open\"" })
        })
        .unwrap();
    let imported_index = import
        .split("(func (;")
        .nth(1)
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let body = if logging {
        "i32.const 0 ".repeat(7) + &format!("i32.const 128 call {imported_index}")
    } else {
        format!("i32.const 0 i32.const 0 i32.const 128 call {imported_index}")
    };
    let replacement = format!("{} {body})", needle.trim_end().strip_suffix(')').unwrap());
    let wat = wat
        .replace(needle, &replacement)
        .replace("(memory (;0;) 0)", "(memory (;0;) 1)");
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
#[tokio::test]
async fn component_start_sections_never_receive_a_service_export_context() {
    let ex = Executor::new(Limits::default()).unwrap();
    for logging in [false, true] {
        let p = package_bytes(MANIFEST, &startup_component(logging)).unwrap();
        let c = ex
            .compile_service(
                &p.bytes,
                &p.package_digest,
                &p.artifact_digest,
                p.artifact_size,
            )
            .unwrap();
        let owner = Owner::new(Fault::None);
        let host = host(&c, owner.clone()).await;
        let result = ex
            .service_start(
                &c,
                host.context().clone(),
                host.clone(),
                "ticket_events",
                r#"{"mode":"normal"}"#,
            )
            .await;
        assert!(
            matches!(result.outcome, Err(ExecutionError::Trap(_))),
            "{result:?}"
        );
        assert!(owner.state.lock().unwrap().calls.iter().all(|r| matches!(
            r,
            PluginHostRequest::Begin { .. } | PluginHostRequest::Authorize { .. }
        )));
    }
}

#[tokio::test]
async fn artifact_delivery_binds_service_names_and_subscriptions_to_component_package() {
    use delivery::{
        ArtifactCache, ArtifactExpectation, ArtifactSource, ArtifactStream, CacheLimits,
        DeliveryError,
    };

    struct Source {
        bytes: Vec<u8>,
        metadata: PluginArtifactDeliveryMetadata,
    }
    #[async_trait]
    impl ArtifactSource for Source {
        fn server_authority_identity(&self) -> &str {
            "service-test-authority"
        }
        async fn authorize(
            &self,
            _: &PluginArtifactDeliveryRequest,
        ) -> Result<PluginArtifactDeliveryMetadata, DeliveryError> {
            Ok(self.metadata.clone())
        }
        async fn fetch(
            &self,
            _: &PluginArtifactDeliveryRequest,
        ) -> Result<ArtifactStream, DeliveryError> {
            Ok(Box::pin(std::io::Cursor::new(self.bytes.clone())))
        }
    }
    fn metadata<T: serde::Serialize, U: serde::de::DeserializeOwned>(value: &T) -> U {
        serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap()
    }

    let artifact = package(MANIFEST).unwrap();
    let verified = plugin::verify_plugin_artifact(
        &artifact.bytes,
        &artifact.package_digest,
        &artifact.artifact_digest,
        artifact.artifact_size,
        &PluginPackageLimits::default(),
    )
    .unwrap();
    let m = &verified.package.manifest;
    let schema = plugin::plugin_config_schema_identity(m);
    let descriptor = PluginArtifact {
        package_id: m.id.clone(),
        name: m.name.clone(),
        version: m.version.clone(),
        description: m.description.clone(),
        package_digest: artifact.package_digest.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
        artifact_size: artifact.artifact_size.try_into().unwrap(),
        schema_version: m.schema_version,
        world: m.runtime.world.clone(),
        required_imports: m.runtime.required_imports.clone(),
        tools: metadata(&m.tools),
        services: metadata(&m.services),
        connections: metadata(&m.connections),
        requested_grants: metadata(&m.requested_grants),
        data_collections: metadata(&m.data_collections),
        config_schema: PluginSchemaContribution {
            identity: schema.identity,
            version: schema.version,
            digest: schema.digest,
            namespace: schema.namespace,
            dcdl: m.config_schema.clone(),
        },
        revoked: false,
        revocation_reason: None,
    };
    assert_eq!(
        descriptor.services[0].subscriptions,
        ["ticket_changed", "ticket_deleted"]
    );
    let request = PluginArtifactDeliveryRequest {
        plan_id: "service-plan".into(),
        plan_fingerprint: "service-plan-fingerprint".into(),
        installation_id: "installation".into(),
        artifact_digest: artifact.artifact_digest.clone(),
    };
    let directory = tempfile::tempdir().unwrap();
    let cache = ArtifactCache::new(directory.path().join("cache"), CacheLimits::default()).unwrap();
    let mut source = Source {
        bytes: artifact.bytes.clone(),
        metadata: PluginArtifactDeliveryMetadata {
            workspace_id: "workspace".into(),
            request: request.clone(),
            artifact: descriptor.clone(),
        },
    };
    let resolved = cache
        .resolve(
            &source,
            "workspace",
            &request,
            &ArtifactExpectation::new(descriptor.clone()),
        )
        .await
        .unwrap();
    assert_eq!(resolved.bytes(), artifact.bytes);

    let mut renamed = descriptor.clone();
    renamed.services[0].name = "different_service".into();
    let mut narrowed = descriptor.clone();
    narrowed.services[0].subscriptions = vec!["ticket_changed".into()];
    let mut unknown = descriptor.clone();
    unknown.services[0].subscriptions = vec!["arbitrary_event".into()];
    let mut missing = descriptor.clone();
    missing.services.clear();
    for tampered in [renamed, narrowed, unknown, missing] {
        // Source metadata and the trusted descriptor agree; only verification
        // against the actual package can detect this mismatch, including hits.
        source.metadata.artifact = tampered.clone();
        let expected = ArtifactExpectation::new(tampered);
        assert!(matches!(
            cache
                .resolve(&source, "workspace", &request, &expected)
                .await,
            Err(DeliveryError::MetadataMismatch)
        ));
    }
    source.metadata.artifact = descriptor.clone();
    assert_eq!(
        cache
            .resolve(
                &source,
                "workspace",
                &request,
                &ArtifactExpectation::new(descriptor)
            )
            .await
            .unwrap()
            .bytes(),
        artifact.bytes
    );
}
