//! Regression coverage for MR P1_CURRENT_ANCESTOR_SCOPE, through real adapters.
use super::*;

struct State {
    visible: bool,
    identity: Option<String>,
    name: String,
    generation: usize,
    target_validator: Option<Vec<u8>>,
    lookups: Vec<String>,
}

struct AncestorProvider {
    scope_path: String,
    target_path: String,
    state: Arc<Mutex<State>>,
    calls: Arc<AtomicUsize>,
    blocked: bool,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

impl AncestorProvider {
    fn scope_projection(&self, state: &State) -> WipProjection {
        let mut p = projection(
            &self.scope_path,
            &self.scope_path,
            &state.name,
            self.calls.clone(),
        );
        p.object.r#ref = state.identity.clone();
        self.version(p, state.generation)
    }

    fn version(&self, mut p: WipProjection, generation: usize) -> WipProjection {
        p.descriptor.documentation = Some(Documentation {
            summary: format!("Published generation {generation}"),
            details: None,
        });
        if self.blocked {
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
        }
        p
    }
}

#[async_trait]
impl WipSubtreeProvider for AncestorProvider {
    async fn publication(&self, path: &str) -> Result<Option<WipPublication>, ProtocolError> {
        // One lock captures target, scope lifetime, descriptor/validator and
        // handler. No separate scope lookup can splice new identity onto old data.
        let mut state = self.state.lock().unwrap();
        state.lookups.push(path.into());
        if path == self.scope_path {
            return if state.visible {
                WipPublication::self_scoped(self.scope_projection(&state)).map(Some)
            } else {
                Ok(None)
            };
        }
        if path == "/github" && self.scope_path != path {
            return if state.visible {
                WipPublication::self_scoped(projection(path, path, "root", self.calls.clone()))
                    .map(Some)
            } else {
                Ok(None)
            };
        }
        if path != self.target_path {
            return Ok(None);
        }
        let mut target = projection(path, &self.scope_path, &state.name, self.calls.clone());
        target.object.validator = state.target_validator.clone();
        let target = self.version(target, state.generation);
        // Retain a target artifact after deleting the scope to exercise Host
        // fail-closed behavior, not merely a provider that already hides orphans.
        let scope = state.visible.then(|| self.scope_projection(&state).object);
        Ok(Some(WipPublication {
            projection: target,
            scope,
        }))
    }

    async fn children(&self, path: &str) -> Result<Vec<String>, ProtocolError> {
        let state = self.state.lock().unwrap();
        if !state.visible {
            return Ok(vec![]);
        }
        if path == self.scope_path {
            return Ok(vec![self.target_path.clone()]);
        }
        if path == "/github" && path != self.scope_path {
            return Ok(vec![self.scope_path.clone()]);
        }
        Ok(vec![])
    }
}

fn fixture(scope: &str, with_ref: bool, blocked: bool) -> (WipRuntime, Arc<AncestorProvider>) {
    let provider = Arc::new(AncestorProvider {
        scope_path: scope.into(),
        target_path: if scope == "/" {
            "/item".into()
        } else {
            format!("{scope}/item")
        },
        state: Arc::new(Mutex::new(State {
            visible: true,
            identity: with_ref.then(|| "live-scope".into()),
            name: "read".into(),
            generation: 0,
            target_validator: None,
            lookups: vec![],
        })),
        calls: Arc::new(AtomicUsize::new(0)),
        blocked,
        started: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    });
    let mut mounts = WipMountRegistry::new();
    mounts.allocate_namespace("scope", "github").unwrap();
    let mount_root = if scope == "/" { "/" } else { "/github" };
    let mut placeholder = projection(mount_root, mount_root, "read", provider.calls.clone());
    placeholder.object.r#ref = Some("registration-placeholder".into());
    mounts.mount(placeholder).unwrap();
    mounts
        .mount_subtree(WipSubtreeMount {
            root: mount_root.into(),
            provider: provider.clone(),
        })
        .unwrap();
    (
        WipRuntime::from_mounts(mounts, "ancestor-test".into()).unwrap(),
        provider,
    )
}

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

#[tokio::test]
async fn live_root_and_intermediate_ancestors_use_one_current_publication_not_placeholder() {
    for scope in ["/", "/github", "/github/team"] {
        for with_ref in [true, false] {
            let (runtime, provider) = fixture(scope, with_ref, false);
            let selected = reference(scope, "read");
            let fetched = runtime.host.fetch_interface_live(&selected).await.unwrap();
            assert_eq!(fetched.scope_ref, with_ref.then(|| "live-scope".into()));
            runtime
                .inspect(provider.target_path.clone(), false)
                .await
                .unwrap();
            provider.state.lock().unwrap().lookups.clear();
            runtime
                .invoke(
                    provider.target_path.clone(),
                    selected,
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap();
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                provider.state.lock().unwrap().lookups,
                [provider.target_path.clone()]
            );
            let state = runtime.state.lock().unwrap();
            let call = state
                .client
                .call_history(&state.session)
                .unwrap()
                .back()
                .unwrap();
            assert_eq!(
                call.context.request().interface.scope_ref,
                fetched.scope_ref
            );
            drop(state);
            // A specified ref must still match even when the current scope has none.
            assert_eq!(
                rejected_code(
                    &runtime,
                    metadata_request(
                        &provider.target_path,
                        json!({"scope":scope,"name":"read"}),
                        None,
                        Some("wrong")
                    )
                )
                .await,
                "interface_mismatch"
            );
            assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn deletion_without_prior_scope_inspect_rejects_cached_invoke_and_orphan_observation() {
    for scope in ["/github", "/github/team"] {
        for with_ref in [true, false] {
            let (runtime, provider) = fixture(scope, with_ref, false);
            let selected = reference(scope, "read");
            runtime
                .inspect(provider.target_path.clone(), false)
                .await
                .unwrap();
            provider.state.lock().unwrap().visible = false;
            let failed = runtime
                .invoke(
                    provider.target_path.clone(),
                    selected.clone(),
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct(),
                )
                .await
                .unwrap_err();
            assert!(failed.to_string().contains("InterfaceMismatch"), "{failed}");
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
            assert!(
                runtime
                    .inspect(provider.target_path.clone(), true)
                    .await
                    .is_err()
            );
            assert!(
                runtime
                    .tree(provider.target_path.clone(), 0, true)
                    .await
                    .is_err()
            );
            assert_eq!(
                runtime
                    .host
                    .fetch_interface_live(&selected)
                    .await
                    .unwrap_err()
                    .code,
                ProtocolErrorCode::InterfaceNotFound
            );
            let root = runtime.tree("/".into(), 3, true).await.unwrap();
            assert!(!root.content.unwrap().contains("/github"));
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        }
    }
}

#[tokio::test]
async fn republishing_scope_never_restores_old_registration_even_without_refs() {
    for with_ref in [true, false] {
        let (runtime, provider) = fixture("/github", with_ref, false);
        let old = reference("/github", "read");
        runtime
            .inspect(provider.target_path.clone(), false)
            .await
            .unwrap();
        provider.state.lock().unwrap().visible = false;
        assert_eq!(
            runtime
                .host
                .fetch_interface_live(&old)
                .await
                .unwrap_err()
                .code,
            ProtocolErrorCode::InterfaceNotFound
        );
        {
            let mut state = provider.state.lock().unwrap();
            state.visible = true;
            state.identity = with_ref.then(|| "replacement".into());
            state.name = "new".into();
            state.generation = 1;
        }
        // The old root registration and cached old descriptor both still exist.
        assert_eq!(
            runtime
                .host
                .fetch_interface_live(&old)
                .await
                .unwrap_err()
                .code,
            ProtocolErrorCode::InterfaceNotFound
        );
        assert!(
            runtime
                .invoke(
                    provider.target_path.clone(),
                    old,
                    "read".into(),
                    json!({}),
                    ToolExecutionContext::direct()
                )
                .await
                .is_err()
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        runtime
            .inspect(provider.target_path.clone(), true)
            .await
            .unwrap();
        let new = reference("/github", "new");
        assert_eq!(
            runtime
                .host
                .fetch_interface_live(&new)
                .await
                .unwrap()
                .scope_ref,
            with_ref.then(|| "replacement".into())
        );
        runtime
            .invoke(
                provider.target_path.clone(),
                new,
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn missing_ancestor_preserves_wire_shape_target_and_object_validator_precedence() {
    let (runtime, provider) = fixture("/github", false, false);
    {
        let mut state = provider.state.lock().unwrap();
        state.visible = false;
        state.target_validator = Some(vec![1]);
    }
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request("/missing", json!("old"), None, None)
        )
        .await,
        "invalid_request"
    );
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                "/missing",
                json!({"scope":"/github","name":"read"}),
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
                &provider.target_path,
                json!({"scope":"/github","name":"read"}),
                Some("AA=="),
                None
            )
        )
        .await,
        "validator_mismatch"
    );
    assert_eq!(
        rejected_code(
            &runtime,
            metadata_request(
                &provider.target_path,
                json!({"scope":"/github","name":"read"}),
                Some("AQ=="),
                None
            )
        )
        .await,
        "interface_mismatch"
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ancestor_replacement_during_in_flight_call_keeps_the_original_descriptor_and_handler() {
    let (runtime, provider) = fixture("/github/team", true, true);
    let selected = reference("/github/team", "read");
    runtime
        .inspect(provider.target_path.clone(), false)
        .await
        .unwrap();
    let pending = runtime.invoke(
        provider.target_path.clone(),
        selected.clone(),
        "read".into(),
        json!({}),
        ToolExecutionContext::direct(),
    );
    let update = async {
        provider.started.notified().await;
        {
            let mut state = provider.state.lock().unwrap();
            state.identity = Some("replacement".into());
            state.generation = 1;
        }
        let updated = runtime
            .inspect(provider.target_path.clone(), true)
            .await
            .unwrap();
        assert!(updated.content.unwrap().contains("integer"));
        provider.release.notify_one();
    };
    let (old, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(pending, update)
    })
    .await
    .unwrap();
    assert_eq!(old.unwrap().content.as_deref(), Some("\"old result\""));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let state = runtime.state.lock().unwrap();
    let history = state.client.call_history(&state.session).unwrap();
    let old = history.back().unwrap();
    assert_eq!(
        old.context.request().interface.scope_ref.as_deref(),
        Some("live-scope")
    );
    assert_eq!(
        old.context.descriptor().operations[0].returns.r#type,
        TypeExpr::String
    );
    drop(state);
    let new = runtime
        .invoke(
            provider.target_path.clone(),
            selected,
            "read".into(),
            json!({}),
            ToolExecutionContext::direct(),
        )
        .await
        .unwrap();
    assert_eq!(new.content.as_deref(), Some("2"));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn same_name_scope_replacement_or_loss_of_ref_rejects_old_metadata_without_replay() {
    for identity in [Some("replacement"), None] {
        let (runtime, provider) = fixture("/github/team", true, false);
        let selected = reference("/github/team", "read");
        runtime
            .inspect(provider.target_path.clone(), false)
            .await
            .unwrap();
        {
            let mut state = provider.state.lock().unwrap();
            state.identity = identity.map(Into::into);
            if identity.is_some() {
                state.generation = 1;
            }
        }
        assert_eq!(
            rejected_code(
                &runtime,
                metadata_request(
                    &provider.target_path,
                    json!({"scope":"/github/team","name":"read"}),
                    None,
                    Some("live-scope")
                )
            )
            .await,
            "interface_mismatch"
        );
        let failed = runtime
            .invoke(
                provider.target_path.clone(),
                selected.clone(),
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap_err();
        assert!(failed.to_string().contains("InterfaceMismatch"), "{failed}");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        let fetched = runtime.host.fetch_interface_live(&selected).await.unwrap();
        assert_eq!(fetched.scope_ref.as_deref(), identity);
        assert_eq!(
            fetched.descriptor.documentation.unwrap().summary,
            if identity.is_some() {
                "Published generation 1"
            } else {
                "Published generation 0"
            }
        );
        runtime
            .inspect(provider.target_path.clone(), true)
            .await
            .unwrap();
        runtime
            .invoke(
                provider.target_path.clone(),
                selected,
                "read".into(),
                json!({}),
                ToolExecutionContext::direct(),
            )
            .await
            .unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        let state = runtime.state.lock().unwrap();
        let call = state
            .client
            .call_history(&state.session)
            .unwrap()
            .back()
            .unwrap();
        // Omitted scope_ref is never filled from the placeholder or other observations.
        assert_eq!(
            call.context.request().interface.scope_ref.as_deref(),
            identity
        );
    }
}
