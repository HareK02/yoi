//! Scope publication and precondition tests through the actual Yoi HTTP adapter.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize};

struct ReadHandler(Arc<AtomicUsize>);
#[async_trait]
impl WipOperationHandler for ReadHandler {
    async fn call(
        &self,
        _: &str,
        _: &BTreeMap<String, Value>,
        _: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(WipOperationOutput::native(Value::String("value".into())))
    }
}

fn projection(path: &str, scope: &str, name: &str, calls: Arc<AtomicUsize>) -> WipProjection {
    let interface = InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    };
    WipProjection {
        route: path.into(),
        capability: "scope:object".into(),
        kind: WipProjectionKind::Native,
        object: Object {
            name: path.rsplit('/').next().unwrap().into(),
            description: None,
            interfaces: vec![interface.clone()],
            r#ref: Some(format!("object:{path}")),
            validator: None,
        },
        interface,
        descriptor: InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: None,
            types: vec![],
            operations: vec![OperationDeclaration {
                name: "read".into(),
                documentation: None,
                parameters: vec![],
                returns: ReturnDeclaration {
                    documentation: None,
                    r#type: TypeExpr::String,
                },
            }],
        },
        interface_validator: None,
        handler: Arc::new(ReadHandler(calls)),
    }
}

fn metadata_request(
    path: &str,
    reference: Json,
    object_validator: Option<&str>,
    scope_ref: Option<&str>,
) -> Request<Vec<u8>> {
    let mut target = json!({"path": path});
    if let Some(v) = object_validator {
        target["validator"] = json!(v);
    }
    let mut interface = json!({"reference": reference});
    if let Some(v) = scope_ref {
        interface["scope_ref"] = json!(v);
    }
    Request::builder()
        .body(
            serde_json::to_vec(
                &json!({"target":target,"interface":interface,"operation":"read","arguments":{}}),
            )
            .unwrap(),
        )
        .unwrap()
}

async fn rejected_code(runtime: &WipRuntime, request: Request<Vec<u8>>) -> String {
    let (response, passthrough, outcome) = runtime
        .dispatch_call(&request, ToolExecutionContext::direct())
        .await
        .unwrap();
    assert!(passthrough.is_none());
    assert_eq!(outcome, WipAuditOutcome::Rejected);
    let json: Json = serde_json::from_slice(response.body()).unwrap();
    json["error"]["code"].as_str().unwrap().into()
}

#[tokio::test]
async fn adapter_preconditions_preserve_shape_target_validator_and_scope_precedence() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    let mut p = projection("/github/item", "/github/item", "read", calls.clone());
    p.object.validator = Some(vec![1]);
    registry.mount(p).unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request("/missing", json!("legacy"), None, None)
        )
        .await,
        "invalid_request"
    );
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                "/missing",
                json!({"scope":"/sibling","name":"read"}),
                Some("AA=="),
                None
            )
        )
        .await,
        "not_found"
    );
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                "/github/item",
                json!({"scope":"/sibling","name":"read"}),
                Some("AA=="),
                None
            )
        )
        .await,
        "validator_mismatch"
    );
    for scope in ["/sibling", "/github/item/child", "/github-extra"] {
        assert_eq!(
            rejected_code(
                &runtime,
                metadata_request(
                    "/github/item",
                    json!({"scope":scope,"name":"read"}),
                    Some("AQ=="),
                    None
                )
            )
            .await,
            "interface_mismatch"
        );
    }
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                "/github/item",
                json!({"scope":"/github/item","name":"read"}),
                Some("AQ=="),
                Some("old")
            )
        )
        .await,
        "interface_mismatch"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn actual_provider_root_and_self_scope_inspect_invoke_preserve_optional_refs() {
    for path in ["/", "/github"] {
        for with_ref in [true, false] {
            let calls = Arc::new(AtomicUsize::new(0));
            let mut registry = WipMountRegistry::new();
            registry.allocate_namespace("scope", "github").unwrap();
            let mut p = projection(path, path, "read", calls.clone());
            if !with_ref {
                p.object.r#ref = None;
            }
            let reference = p.interface.clone();
            registry.mount(p).unwrap();
            let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
            let fetched = runtime.host.fetch_interface_live(&reference).await.unwrap();
            assert_eq!(
                fetched.scope_ref,
                with_ref.then(|| format!("object:{path}"))
            );
            runtime.inspect(path.into(), false).await.unwrap();
            runtime
                .invoke(
                    path.into(),
                    reference.clone(),
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            let state = runtime.state.lock().unwrap();
            let history = state.client.call_history(&state.session).unwrap();
            assert_eq!(
                history
                    .back()
                    .unwrap()
                    .context
                    .request()
                    .interface
                    .scope_ref,
                fetched.scope_ref
            );
            drop(state);
            if !with_ref {
                assert_eq!(
                    rejected_code(
                        &runtime,
                        metadata_request(
                            path,
                            json!({"scope":path,"name":"read"}),
                            None,
                            Some("invented")
                        )
                    )
                    .await,
                    "interface_mismatch"
                );
            }
        }
    }
}

#[tokio::test]
async fn multiple_interfaces_same_operation_are_inspected_and_dispatched_without_flattening() {
    let a = Arc::new(AtomicUsize::new(0));
    let b = Arc::new(AtomicUsize::new(0));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    let owner = projection("/github", "/", "same", a.clone());
    let other = projection("/github", "/github", "same", b.clone());
    let refs = [owner.interface.clone(), other.interface.clone()];
    registry.mount(owner).unwrap();
    registry.mount_interface(other).unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    let inspected = runtime.inspect("/github".into(), false).await.unwrap();
    let inspected: Json = serde_json::from_str(inspected.content.as_ref().unwrap()).unwrap();
    assert_eq!(inspected["interfaces"].as_array().unwrap().len(), 2);
    for reference in refs {
        runtime
            .invoke(
                "/github".into(),
                reference,
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
    }
    assert_eq!(a.load(Ordering::SeqCst), 1);
    assert_eq!(b.load(Ordering::SeqCst), 1);
}

struct Publication {
    visible: Arc<AtomicBool>,
    identity: Arc<Mutex<Option<String>>>,
    validator: Arc<Mutex<Option<Vec<u8>>>>,
    calls: Arc<AtomicUsize>,
}
impl WipDynamicItemResolver for Publication {
    fn contextual_interface(&self) -> bool {
        true
    }
    fn resolve(&self, item: &str) -> Option<WipDynamicItem> {
        if item != "item" || !self.visible.load(Ordering::SeqCst) {
            return None;
        }
        let mut p = projection("/github/item", "/", "read", self.calls.clone());
        p.object.r#ref = self.identity.lock().unwrap().clone();
        p.object.validator = self.validator.lock().unwrap().clone();
        Some(WipDynamicItem {
            object: p.object,
            handler: p.handler,
        })
    }
}

#[tokio::test]
async fn current_self_scope_fetch_does_not_attach_a_new_ref_to_an_old_target_snapshot() {
    let calls = Arc::new(AtomicUsize::new(0));
    let visible = Arc::new(AtomicBool::new(true));
    let identity = Arc::new(Mutex::new(Some("first".into())));
    let validator = Arc::new(Mutex::new(None));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    registry
        .mount(projection("/github", "/", "collection", calls.clone()))
        .unwrap();
    let p = projection("/github/item", "/", "read", calls.clone());
    registry
        .mount_dynamic(WipDynamicMount {
            collection_route: "/github".into(),
            capability: "scope:item".into(),
            interface: p.interface,
            descriptor: p.descriptor,
            interface_validator: None,
            resolver: Arc::new(Publication {
                visible: visible.clone(),
                identity: identity.clone(),
                validator: validator.clone(),
                calls: calls.clone(),
            }),
        })
        .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    let reference = contextual_reference("read", "/github/item");
    runtime.inspect("/github/item".into(), false).await.unwrap();
    assert_eq!(
        runtime
            .host
            .fetch_interface_live(&reference)
            .await
            .unwrap()
            .scope_ref
            .as_deref(),
        Some("first")
    );
    // Ordinary content-state updates do not change the scope Object identity.
    *validator.lock().unwrap() = Some(vec![2]);
    assert_eq!(
        runtime
            .host
            .fetch_interface_live(&reference)
            .await
            .unwrap()
            .scope_ref
            .as_deref(),
        Some("first")
    );
    *validator.lock().unwrap() = None;
    *identity.lock().unwrap() = Some("second".into());
    let error = runtime
        .invoke(
            "/github/item".into(),
            reference.clone(),
            "read".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("InterfaceMismatch"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // Both target and Interface are invalidated; retrievals are not reexecution.
    let state = runtime.state.lock().unwrap();
    assert_eq!(
        state
            .client
            .object(&state.session, "/github/item")
            .unwrap()
            .state,
        ObservationState::Stale
    );
    assert_eq!(
        state
            .client
            .interface(&state.session, &reference)
            .unwrap()
            .state,
        ObservationState::Stale
    );
    drop(state);
    runtime
        .invoke(
            "/github/item".into(),
            reference.clone(),
            "read".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    visible.store(false, Ordering::SeqCst);
    assert!(runtime.host.fetch_interface_live(&reference).await.is_err());
    assert!(runtime.inspect("/github/item".into(), true).await.is_err());
    let state = runtime.state.lock().unwrap();
    assert!(state.client.interface(&state.session, &reference).is_none());
}

#[tokio::test]
async fn supplied_validators_require_current_values_and_required_object_validator_is_late() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    registry
        .mount(projection("/github", "/github", "read", calls.clone()))
        .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    let reference = json!({"scope":"/github","name":"read"});
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request("/github", reference.clone(), Some("AQ=="), None)
        )
        .await,
        "validator_mismatch"
    );
    let mut request = metadata_request("/github", reference, None, None);
    let mut body: Json = serde_json::from_slice(request.body()).unwrap();
    body["interface"]["validator"] = json!("AQ==");
    *request.body_mut() = serde_json::to_vec(&body).unwrap();
    assert_eq!(
        rejected_code(&runtime, request).await,
        "interface_validator_mismatch"
    );
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    let mut p = projection("/github", "/github", "read", calls.clone());
    p.object.validator = Some(vec![1]);
    registry.mount(p).unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                "/github",
                json!({"scope":"/wrong","name":"read"}),
                None,
                None
            )
        )
        .await,
        "interface_mismatch"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct ChangingProvider {
    generation: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}
struct FrozenResultHandler {
    generation: usize,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    calls: Arc<AtomicUsize>,
}
#[async_trait]
impl WipOperationHandler for FrozenResultHandler {
    async fn call(
        &self,
        _: &str,
        _: &BTreeMap<String, Value>,
        _: WipCallContext,
    ) -> Result<WipOperationOutput, WipOperationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.generation == 0 {
            self.started.notify_one();
            self.release.notified().await;
            Ok(WipOperationOutput::native(Value::String(
                "old result".into(),
            )))
        } else {
            Ok(WipOperationOutput::native(Value::Integer(2)))
        }
    }
}
#[async_trait]
impl WipSubtreeProvider for ChangingProvider {
    async fn projection(&self, path: &str) -> Result<Option<WipProjection>, ProtocolError> {
        if path != "/github" {
            return Ok(None);
        }
        let generation = self.generation.load(Ordering::SeqCst);
        let mut p = projection(path, path, "read", self.calls.clone());
        p.interface_validator = Some(vec![generation as u8]);
        if generation != 0 {
            p.descriptor.operations[0].returns.r#type = TypeExpr::Integer;
        }
        p.handler = Arc::new(FrozenResultHandler {
            generation,
            started: self.started.clone(),
            release: self.release.clone(),
            calls: self.calls.clone(),
        });
        Ok(Some(p))
    }
    async fn children(&self, _: &str) -> Result<Vec<String>, ProtocolError> {
        Ok(vec![])
    }
}

#[tokio::test]
async fn in_flight_result_uses_frozen_host_and_client_descriptors_after_refresh() {
    let generation = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut registry = WipMountRegistry::new();
    registry.allocate_namespace("scope", "github").unwrap();
    registry
        .mount(projection("/github", "/github", "read", calls.clone()))
        .unwrap();
    registry
        .mount_subtree(WipSubtreeMount {
            root: "/github".into(),
            provider: Arc::new(ChangingProvider {
                generation: generation.clone(),
                started: started.clone(),
                release: release.clone(),
                calls: calls.clone(),
            }),
        })
        .unwrap();
    let runtime = WipRuntime::from_mounts(registry, "scope-test".into()).unwrap();
    runtime.inspect("/github".into(), false).await.unwrap();
    let pending = runtime.invoke(
        "/github".into(),
        contextual_reference("read", "/github"),
        "read".into(),
        json!({}),
        ToolExecutionContext::direct(),
    );
    let update = async {
        started.notified().await;
        generation.store(1, Ordering::SeqCst);
        let output = runtime.inspect("/github".into(), true).await.unwrap();
        assert!(output.content.unwrap().contains("integer"));
        release.notify_one();
    };
    let (old_result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(pending, update)
    })
    .await
    .unwrap();
    assert_eq!(
        old_result.unwrap().content.as_deref(),
        Some("\"old result\"")
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let state = runtime.state.lock().unwrap();
    let record = state
        .client
        .call_history(&state.session)
        .unwrap()
        .back()
        .unwrap();
    assert_eq!(
        record.context.descriptor().operations[0].returns.r#type,
        TypeExpr::String
    );
    assert_eq!(
        state
            .client
            .interface(&state.session, &contextual_reference("read", "/github"))
            .unwrap()
            .descriptor
            .as_ref()
            .unwrap()
            .operations[0]
            .returns
            .r#type,
        TypeExpr::Integer
    );
}
