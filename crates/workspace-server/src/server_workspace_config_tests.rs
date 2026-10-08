// Included inside server::tests so the existing real Workspace/Runtime fixtures are reused.
mod workspace_config_integration {
    include!("server_workspace_config_wip_tests.rs");
    use super::*;
    use server_api::{
        WorkspaceConfigAccess as Access, WorkspaceConfigAttachRequest as Attach,
        WorkspaceConfigAttachment as Attachment, WorkspaceConfigCommitRequest as Commit,
        WorkspaceConfigFailureClassification as Classification,
        WorkspaceConfigGrantCreateRequest as Grant, WorkspaceConfigObserveRequest as Observe,
        WorkspaceConfigObserveResponse as Observed, WorkspaceConfigReadRequest as Read,
    };

    async fn grant(
        api: &WorkspaceApi,
        worker: &RuntimeWorkerRef,
        access: Access,
    ) -> server_api::WorkspaceConfigGrantResponse {
        workspace_config::create_grant(
            api,
            &test_owner_actor(),
            TEST_WORKSPACE_ID,
            Grant {
                runtime_id: worker.runtime_id.clone(),
                worker_id: worker.worker_id.clone(),
                access,
            },
        )
        .await
        .unwrap()
    }
    async fn attach(api: &WorkspaceApi, worker: &RuntimeWorkerRef) -> Attachment {
        workspace_config::attach(
            api,
            worker,
            Attach {
                alias: None,
                access: None,
            },
        )
        .await
        .unwrap()
    }
    async fn observe(
        api: &WorkspaceApi,
        worker: &RuntimeWorkerRef,
        connection_id: &str,
        paths: &[&str],
        depth: u32,
    ) -> Observed {
        workspace_config::observe(
            api,
            worker,
            Observe {
                connection_id: connection_id.into(),
                paths: paths.iter().map(|path| (*path).into()).collect(),
                depth,
            },
        )
        .await
        .unwrap()
    }
    fn commit_request(
        connection: &str,
        snapshot: &Observed,
        changes: Vec<server_api::ConfigTreeChange>,
    ) -> Commit {
        Commit {
            connection_id: connection.into(),
            validator: snapshot.validator.clone(),
            request: server_api::ConfigCommitRequest {
                base_revision: snapshot.revision,
                base_digest: snapshot.digest.clone(),
                changes,
                entrypoints: snapshot.entrypoints.clone(),
            },
        }
    }
    fn create(path: &str, content: &str) -> server_api::ConfigTreeChange {
        server_api::ConfigTreeChange::Create {
            path: path.into(),
            content_type: server_api::ConfigContentType::Text,
            content: content.into(),
        }
    }
    async fn second_worker(fixture: &ManualCoderAssignmentFixture) -> RuntimeWorkerRef {
        let mut summary = fixture.api.runtime.worker(&fixture.worker).unwrap();
        summary.worker.worker_id = Uuid::now_v7().to_string();
        summary.display_name = "Second config editor".into();
        summary.label = summary.display_name.clone();
        summary.workdir_attachments.clear();
        fixture
            .runtime
            .workers
            .lock()
            .unwrap()
            .push(summary.clone());
        sync_worker_observation(&fixture.api, &summary).unwrap();
        summary.worker
    }
    async fn signed_call(
        api: &WorkspaceApi,
        identity: &RuntimeIdentityMaterial,
        worker: Option<&str>,
        method: &str,
        path: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let bytes = if method == "GET" {
            vec![]
        } else {
            serde_json::to_vec(&body).unwrap()
        };
        let response = build_router(api.clone())
            .oneshot(runtime_source_request(
                identity, worker, method, path, bytes,
            ))
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned())),
        )
    }

    #[tokio::test]
    async fn workspace_config_signed_editor_routes_cannot_bypass_wip_authority() {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let api = &fixture.api;
        let initial = api.config_store.load_workspace_config(TEST_WORKSPACE_ID).unwrap().unwrap();
        let tree = format!("/api/w/{TEST_WORKSPACE_ID}/config/source-tree");
        let change = server_api::ConfigCommitRequest {
            base_revision: initial.snapshot.revision,
            base_digest: initial.snapshot.digest.clone(),
            changes: vec![create("notes/editor-only.txt", "must not persist from Worker")],
            entrypoints: vec!["main.dcdl".into()],
        };
        let routes = [
            ("GET", tree.clone()),
            ("GET", format!("{tree}/entries/main.dcdl")),
            ("GET", format!("{tree}/revisions/{}", initial.snapshot.revision)),
            ("POST", format!("{tree}/commit")),
            ("GET", format!("/api/w/{TEST_WORKSPACE_ID}/settings/profiles")),
        ];
        for state in ["ungranted", "read_only", "revoked", "detached", "read_write"] {
            match state {
                "read_only" => {
                    grant(api, &fixture.worker, Access::ReadOnly).await;
                    attach(api, &fixture.worker).await;
                }
                "revoked" => {
                    let current = api.store.current_workspace_config_grant(TEST_WORKSPACE_ID, &fixture.worker).unwrap().unwrap();
                    workspace_config::revoke_grant(api, &test_owner_actor(), TEST_WORKSPACE_ID,
                        &current.grant_id).await.unwrap();
                }
                "detached" => {
                    grant(api, &fixture.worker, Access::ReadWrite).await;
                    let attached = attach(api, &fixture.worker).await;
                    detach_current_worker_workdir(api, &fixture.worker, "workspace-config",
                        Some(&attached.connection_id)).await.unwrap();
                }
                "read_write" => { attach(api, &fixture.worker).await; }
                _ => {}
            }
            for (method, path) in &routes {
                let body = if *method == "POST" { serde_json::to_vec(&change).unwrap() } else { vec![] };
                for scoped in [false, true] {
                    let app = if scoped {
                        workspace_server_router(WorkspaceServerApi::new(api.config.clone(), api.store.clone()))
                    } else {
                        build_router(api.clone())
                    };
                    let response = app.oneshot(runtime_source_request(&identity,
                        Some(&fixture.worker.worker_id), method, path, body.clone())).await.unwrap();
                    assert_eq!(response.status(), StatusCode::FORBIDDEN,
                        "{state}: {method} {path}, scoped={scoped}");
                    let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
                    assert!(!String::from_utf8_lossy(&bytes).contains("must not persist"));
                }
            }
            let after = api.config_store.load_workspace_config(TEST_WORKSPACE_ID).unwrap().unwrap();
            assert_eq!(after.snapshot, initial.snapshot, "{state}: editor rejection must have no effect");
        }
        // A Runtime-only proof is not a user editor identity either.
        assert_eq!(signed_call(api, &identity, None, "GET", &tree, Value::Null).await.0,
            StatusCode::FORBIDDEN);
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(api, &fixture.worker, &attached.connection_id, &["main.dcdl"], 0).await;
        let node = observed.nodes.iter().find(|n| n.path == "main.dcdl").unwrap();
        assert!(workspace_config::read(api, &fixture.worker, Read {
            connection_id: attached.connection_id,
            path: "main.dcdl".into(), validator: node.validator.clone(),
        }).await.is_ok(), "the granted WIP path remains available");

        // Real API-token authentication still permits the normal UI editor.
        let token = seed_test_api_token(api.store.as_ref(), TEST_WORKSPACE_ID);
        for (method, path) in &routes {
            if *method == "GET" {
                request_json_authenticated(build_router(api.clone()), method, path, None,
                    &token, StatusCode::OK).await;
            }
        }
        request_json_authenticated(build_router(api.clone()), "POST", &format!("{tree}/commit"),
            Some(serde_json::to_value(change).unwrap()), &token, StatusCode::CREATED).await;
        let saved = api.config_store.load_workspace_config(TEST_WORKSPACE_ID).unwrap().unwrap();
        assert_eq!(saved.snapshot.revision, initial.snapshot.revision + 1);
        assert!(saved.snapshot.entries.keys().any(|p| p.as_str() == "notes/editor-only.txt"));
        // Prompt and Skill runtime consumption are separate read-only projections,
        // not authored-tree editor APIs; bootstrap must remain usable.
        assert_eq!(signed_call(api, &identity, Some(&fixture.worker.worker_id), "GET",
            &format!("/api/w/{TEST_WORKSPACE_ID}/config/projections/prompts"), Value::Null).await.0,
            StatusCode::OK);
        assert_eq!(signed_call(api, &identity, Some(&fixture.worker.worker_id), "GET",
            &format!("/api/w/{TEST_WORKSPACE_ID}/skills"), Value::Null).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn workspace_config_entrypoint_support_is_exact_and_canonical_mutations_stay_guarded() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["", "main.dcdl"],
            1,
        )
        .await;
        let main = observed
            .nodes
            .iter()
            .find(|node| node.path == "main.dcdl")
            .unwrap();
        assert_eq!(main.operations, ["read", "edit", "write"]);
        let before = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        for change in [
            server_api::ConfigTreeChange::Delete {
                path: "main.dcdl".into(),
                expected_digest: main.digest.clone().unwrap(),
            },
            server_api::ConfigTreeChange::Rename {
                from: "main.dcdl".into(),
                to: "renamed.dcdl".into(),
                expected_digest: main.digest.clone().unwrap(),
            },
        ] {
            let failure = workspace_config::commit(
                api,
                &fixture.worker,
                commit_request(&attached.connection_id, &observed, vec![change]),
            )
            .await
            .unwrap_err();
            assert_eq!(failure.classification, Classification::NotCommitted);
            assert_eq!(
                api.config_store
                    .load_workspace_config(TEST_WORKSPACE_ID)
                    .unwrap()
                    .unwrap(),
                before
            );
        }
        workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &attached.connection_id,
                &observed,
                vec![create("notes/deletable.md", "note")],
            ),
        )
        .await
        .unwrap();
        let note = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["notes/deletable.md"],
            0,
        )
        .await;
        assert_eq!(
            note.nodes[0].operations,
            ["read", "edit", "write", "delete"]
        );
    }

    #[tokio::test]
    async fn workspace_config_grants_owner_scope_and_signed_routes() {
        let mut fixture = manual_worker_assignment_fixture().await;
        let identity = RuntimeIdentityMaterial::generate(&fixture.worker.runtime_id).unwrap();
        configure_runtime_request_auth(&mut fixture.api, &identity, &fixture.worker.runtime_id);
        let api = &fixture.api;
        let base = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workspace-config");
        assert_eq!(
            signed_call(
                api,
                &identity,
                Some(&fixture.worker.worker_id),
                "GET",
                &base,
                Value::Null
            )
            .await
            .0,
            StatusCode::FORBIDDEN,
            "unattached GET must still require a grant"
        );
        assert_eq!(
            signed_call(
                api,
                &identity,
                Some(&fixture.worker.worker_id),
                "POST",
                &base,
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert!(
            api.store
                .current_workspace_config_grant(TEST_WORKSPACE_ID, &fixture.worker)
                .unwrap()
                .is_none(),
            "attach cannot create a grant"
        );
        let mut other_owner = test_owner_actor();
        other_owner.account_id = "not-owner".into();
        let request = Grant {
            runtime_id: fixture.worker.runtime_id.clone(),
            worker_id: fixture.worker.worker_id.clone(),
            access: Access::ReadWrite,
        };
        assert_eq!(
            workspace_config::create_grant(api, &other_owner, TEST_WORKSPACE_ID, request.clone())
                .await
                .unwrap_err()
                .status,
            403
        );
        assert!(
            workspace_config::create_grant(
                api,
                &test_owner_actor(),
                "foreign-workspace",
                request.clone()
            )
            .await
            .is_err()
        );
        // Exercise generated browser grant router with the authenticated owner extension.
        let path = format!("/api/w/{TEST_WORKSPACE_ID}/workspace-config-grants");
        let response = build_inner_router(api.clone())
            .layer(Extension(test_owner_actor()))
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(&path)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&request).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let granted: server_api::WorkspaceConfigGrantResponse = serde_json::from_slice(
            &to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            grant(api, &fixture.worker, Access::ReadWrite)
                .await
                .grant_id,
            granted.grant_id
        );
        let (status, current) = signed_call(
            api,
            &identity,
            Some(&fixture.worker.worker_id),
            "GET",
            &base,
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(current.is_null());
        let (status, current) = signed_call(
            api,
            &identity,
            Some(&fixture.worker.worker_id),
            "POST",
            &base,
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let attached: Attachment = serde_json::from_value(current).unwrap();
        let again: Attachment = serde_json::from_value(
            signed_call(
                api,
                &identity,
                Some(&fixture.worker.worker_id),
                "POST",
                &base,
                json!({}),
            )
            .await
            .1,
        )
        .unwrap();
        assert!(again.already_attached);
        assert_eq!(again.connection_id, attached.connection_id);
        assert_eq!(
            signed_call(api, &identity, None, "GET", &base, Value::Null)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        let mut forged = runtime_source_request(
            &identity,
            Some(&fixture.worker.worker_id),
            "GET",
            &base,
            vec![],
        );
        forged
            .headers_mut()
            .insert("x-yoi-worker-id", "forged-worker".parse().unwrap());
        assert_eq!(
            build_router(api.clone())
                .oneshot(forged)
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let foreign = base.replace(TEST_WORKSPACE_ID, "foreign-workspace");
        assert_ne!(
            signed_call(
                api,
                &identity,
                Some(&fixture.worker.worker_id),
                "GET",
                &foreign,
                Value::Null
            )
            .await
            .0,
            StatusCode::OK
        );
        // A Worker proof is not an owner grant capability.
        assert_ne!(
            signed_call(
                api,
                &identity,
                Some(&fixture.worker.worker_id),
                "POST",
                &path,
                serde_json::to_value(request).unwrap()
            )
            .await
            .0,
            StatusCode::OK
        );
        assert_eq!(
            workspace_config::attach(
                api,
                &fixture.worker,
                Attach {
                    alias: Some("other".into()),
                    access: None
                }
            )
            .await
            .unwrap_err()
            .status,
            400
        );
        let (status, metadata) = signed_call(
            api,
            &identity,
            Some(&fixture.worker.worker_id),
            "POST",
            &format!("{base}/observe"),
            json!({"connection_id":attached.connection_id,"paths":[""],"depth":1}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            metadata["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|node| node.get("content").is_none())
        );
        assert_eq!(metadata["entrypoints"], json!(["main.dcdl"]));
        assert!(
            api.runtime
                .worker(&fixture.worker)
                .unwrap()
                .workdir_attachments
                .is_empty(),
            "logical source never creates a Runtime filesystem attachment"
        );
        let runtime_logical = fixture
            .runtime
            .logical_attachments(&fixture.worker.worker_id);
        assert!(
            runtime_logical
                .iter()
                .all(|item| item.working_directory_id != attached.working_directory_id)
        );
        let catalog = list_current_worker_workdir_catalog_for_worker(
            api,
            &fixture.worker,
            Default::default(),
        )
        .unwrap();
        let logical = catalog
            .items
            .iter()
            .find(|item| item.working_directory_id == attached.working_directory_id)
            .unwrap();
        assert!(logical.cleanup_target.is_none());
        assert_eq!(
            logical.materializer_kind,
            server_api::WorkingDirectoryMaterializerKind::LogicalWorkspaceConfig
        );
        assert!(matches!(
            logical.source,
            server_api::WorkingDirectorySource::WorkspaceConfig { .. }
        ));
        assert!(
            api.store
                .delete_workdir_registry(TEST_WORKSPACE_ID, &attached.working_directory_id)
                .is_err()
        );
        let generic = format!("/api/w/{TEST_WORKSPACE_ID}/workers/self/workdir-attachments");
        assert_eq!(signed_call(api,&identity,Some(&fixture.worker.worker_id),"POST",&generic,json!({"alias":"workspace-config","working_directory_id":attached.working_directory_id})).await.0,StatusCode::FORBIDDEN);
        let link = api
            .store
            .list_worker_workdir_links(TEST_WORKSPACE_ID, &fixture.worker)
            .unwrap()
            .into_iter()
            .find(|link| link.alias == "workspace-config")
            .unwrap();
        assert!(
            !link
                .capabilities
                .supports(workdir::WorkdirSessionCapability::Command)
        );
        let lock = current_worker_session_lock(api, &fixture.worker);
        let _guard = lock.lock().await;
        assert!(
            open_current_worker_workdir_session_locked(api, &fixture.worker, &link)
                .await
                .is_err()
        );
        drop(_guard);
        assert!(
            cleanup_working_directory(
                api.clone(),
                ScopedWorkingDirectoryPath {
                    workspace_id: TEST_WORKSPACE_ID.into(),
                    working_directory_id: attached.working_directory_id.clone()
                },
                "owner".into(),
                WorkingDirectoryRemovalRequest {
                    reason: "unsupported logical cleanup".into()
                }
            )
            .await
            .is_err()
        );
        let revoke_path = format!(
            "/api/w/{TEST_WORKSPACE_ID}/workspace-config-grants/{}",
            granted.grant_id
        );
        let response = build_inner_router(api.clone())
            .layer(Extension(test_owner_actor()))
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(&revoke_path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            workspace_config::get(api, &fixture.worker)
                .await
                .unwrap_err()
                .status,
            403
        );
    }

    #[tokio::test]
    async fn workspace_config_readonly_revoke_and_conditional_lifetime_fences() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        let read_grant = grant(api, &fixture.worker, Access::ReadOnly).await;
        assert!(
            workspace_config::get(api, &fixture.worker)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            workspace_config::attach(
                api,
                &fixture.worker,
                Attach {
                    alias: None,
                    access: Some(Access::ReadWrite)
                }
            )
            .await
            .unwrap_err()
            .status,
            403
        );
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["", "main.dcdl"],
            1,
        )
        .await;
        assert!(
            observed
                .nodes
                .iter()
                .all(|node| node.operations.iter().all(|operation| operation == "read"))
        );
        let read_node = observed
            .nodes
            .iter()
            .find(|node| node.path == "main.dcdl")
            .unwrap();
        let read = Read {
            connection_id: attached.connection_id.clone(),
            path: "main.dcdl".into(),
            validator: read_node.validator.clone(),
        };
        assert!(
            !workspace_config::read(api, &fixture.worker, read.clone())
                .await
                .unwrap()
                .content
                .is_empty()
        );
        let denied = workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &attached.connection_id,
                &observed,
                vec![create("notes/no.md", "no")],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(denied.status, 403);
        assert_eq!(denied.classification, Classification::NotCommitted);
        let second = second_worker(&fixture).await;
        assert_eq!(
            workspace_config::read(api, &second, read.clone())
                .await
                .unwrap_err()
                .status,
            403
        );
        let second_grant = grant(api, &second, Access::ReadWrite).await;
        assert_ne!(
            second_grant.working_directory_id,
            read_grant.working_directory_id
        );
        assert_eq!(
            workspace_config::read(api, &second, read.clone())
                .await
                .unwrap_err()
                .code,
            "connection_changed"
        );
        let detached = detach_current_worker_workdir(
            api,
            &fixture.worker,
            "workspace-config",
            Some(&attached.connection_id),
        )
        .await
        .unwrap();
        assert!(!detached.attached);
        assert_eq!(
            workspace_config::read(api, &fixture.worker, read.clone())
                .await
                .unwrap_err()
                .code,
            "connection_changed"
        );
        let reattached = attach(api, &fixture.worker).await;
        assert_ne!(reattached.connection_id, attached.connection_id);
        assert!(
            detach_current_worker_workdir(
                api,
                &fixture.worker,
                "workspace-config",
                Some(&attached.connection_id)
            )
            .await
            .is_err()
        );
        assert_eq!(
            workspace_config::get(api, &fixture.worker)
                .await
                .unwrap()
                .unwrap()
                .connection_id,
            reattached.connection_id
        );
        workspace_config::revoke_grant(
            api,
            &test_owner_actor(),
            TEST_WORKSPACE_ID,
            &read_grant.grant_id,
        )
        .await
        .unwrap();
        assert_eq!(
            workspace_config::get(api, &fixture.worker)
                .await
                .unwrap_err()
                .status,
            403
        );
        assert_eq!(
            workspace_config::read(api, &fixture.worker, read)
                .await
                .unwrap_err()
                .status,
            403
        );
        assert_eq!(
            workspace_config::attach(
                api,
                &fixture.worker,
                Attach {
                    alias: None,
                    access: None
                }
            )
            .await
            .unwrap_err()
            .status,
            403
        );
        assert!(
            workspace_config::revoke_grant(
                api,
                &test_owner_actor(),
                "foreign-workspace",
                &second_grant.grant_id
            )
            .await
            .is_err()
        );
        assert!(
            api.store
                .current_workspace_config_grant(TEST_WORKSPACE_ID, &second)
                .unwrap()
                .is_some()
        );
        let writable = attach(api, &second).await;
        let restricted = workspace_config::attach(
            api,
            &second,
            Attach {
                alias: None,
                access: Some(Access::ReadOnly),
            },
        )
        .await
        .unwrap();
        assert_eq!(restricted.connection_id, writable.connection_id);
        assert_eq!(restricted.access, Access::ReadOnly);
        assert_eq!(
            attach(api, &second).await.access,
            Access::ReadOnly,
            "default repeat must honor current attenuation"
        );
        assert_eq!(
            workspace_config::attach(
                api,
                &second,
                Attach {
                    alias: None,
                    access: Some(Access::ReadWrite)
                }
            )
            .await
            .unwrap_err()
            .status,
            403
        );
    }

    #[tokio::test]
    async fn workspace_config_two_workers_ui_cas_and_identical_virtual_replacement() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        let second = second_worker(&fixture).await;
        grant(api, &fixture.worker, Access::ReadWrite).await;
        grant(api, &second, Access::ReadWrite).await;
        let a = attach(api, &fixture.worker).await;
        let b = attach(api, &second).await;
        let first = observe(api, &fixture.worker, &a.connection_id, &[""], 0).await;
        let other = observe(api, &second, &b.connection_id, &[""], 0).await;
        assert_eq!(first.revision, other.revision);
        assert_eq!(first.digest, other.digest);
        assert_ne!(first.validator, other.validator);
        workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &a.connection_id,
                &first,
                vec![create("notes/concurrent.md", "first")],
            ),
        )
        .await
        .unwrap();
        let rejected = workspace_config::commit(
            api,
            &second,
            commit_request(
                &b.connection_id,
                &other,
                vec![create("notes/second.md", "second")],
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(rejected.code, "stale_validator");
        assert_eq!(rejected.classification, Classification::NotCommitted);
        let fresh = observe(api, &second, &b.connection_id, &["notes/concurrent.md"], 0).await;
        let file = fresh.nodes.first().unwrap();
        let old_read = Read {
            connection_id: b.connection_id.clone(),
            path: file.path.clone(),
            validator: file.validator.clone(),
        };
        let canonical = config_commit_request_from_api(server_api::ConfigCommitRequest {
            base_revision: fresh.revision,
            base_digest: fresh.digest.clone(),
            entrypoints: fresh.entrypoints.clone(),
            changes: vec![server_api::ConfigTreeChange::Update {
                path: file.path.clone(),
                expected_digest: file.digest.clone().unwrap(),
                content: "first".into(),
            }],
        })
        .unwrap();
        // Same bytes, new virtual identity/revision via the ORIGINAL UI commit boundary.
        commit_workspace_config_tree(api, TEST_WORKSPACE_ID, &canonical).unwrap();
        let after = observe(api, &second, &b.connection_id, &["notes/concurrent.md"], 0).await;
        assert_eq!(after.digest, fresh.digest);
        assert!(after.revision > fresh.revision);
        assert_ne!(after.validator, fresh.validator);
        assert_eq!(
            workspace_config::read(api, &second, old_read)
                .await
                .unwrap_err()
                .code,
            "stale_validator"
        );
        assert_eq!(
            workspace_config::commit(
                api,
                &second,
                commit_request(
                    &b.connection_id,
                    &fresh,
                    vec![create("notes/stale.md", "stale")]
                )
            )
            .await
            .unwrap_err()
            .code,
            "stale_validator"
        );
        let delete = vec![server_api::ConfigTreeChange::Delete {
            path: file.path.clone(),
            expected_digest: file.digest.clone().unwrap(),
        }];
        workspace_config::commit(
            api,
            &second,
            commit_request(&b.connection_id, &after, delete),
        )
        .await
        .unwrap();
        let deleted = observe(api, &second, &b.connection_id, &[""], 0).await;
        workspace_config::commit(
            api,
            &second,
            commit_request(
                &b.connection_id,
                &deleted,
                vec![create(&file.path, "replacement")],
            ),
        )
        .await
        .unwrap();
        let state = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        assert_eq!(
            state
                .snapshot
                .get(&config_source::VirtualPath::parse("notes/concurrent.md").unwrap())
                .unwrap()
                .content,
            "replacement"
        );
        assert!(
            state
                .snapshot
                .get(&config_source::VirtualPath::parse("notes/second.md").unwrap())
                .is_none()
        );
        // Canonical immediate transaction still rejects a race after evaluation.
        let current = observe(api, &second, &b.connection_id, &[""], 0).await;
        let request = config_commit_request_from_api(
            commit_request(
                &b.connection_id,
                &current,
                vec![create("notes/raced.md", "race")],
            )
            .request,
        )
        .unwrap();
        let candidate = prepare_workspace_config_tree(api, TEST_WORKSPACE_ID, &request).unwrap();
        let ui = config_commit_request_from_api(
            commit_request(
                &b.connection_id,
                &current,
                vec![create("notes/ui.md", "ui")],
            )
            .request,
        )
        .unwrap();
        commit_workspace_config_tree(api, TEST_WORKSPACE_ID, &ui).unwrap();
        assert!(
            api.config_store
                .commit_evaluated_workspace_config(TEST_WORKSPACE_ID, &candidate)
                .is_err()
        );
    }

    #[tokio::test]
    async fn workspace_config_invalid_bounds_and_failed_save_do_not_corrupt_tree() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let before = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let observed = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["", "main.dcdl"],
            0,
        )
        .await;
        let main = observed
            .nodes
            .iter()
            .find(|node| node.path == "main.dcdl")
            .unwrap();
        for changes in [
            vec![server_api::ConfigTreeChange::Update {
                path: "main.dcdl".into(),
                expected_digest: main.digest.clone().unwrap(),
                content: "not valid Decodal".into(),
            }],
            vec![server_api::ConfigTreeChange::Delete {
                path: "main.dcdl".into(),
                expected_digest: main.digest.clone().unwrap(),
            }],
            vec![create("../escape", "no")],
            vec![create("bad/\nsecret", "no")],
            vec![create(&"x".repeat(513), "no")],
            vec![create("notes/large.md", &"x".repeat(256 * 1024 + 1))],
            (0..257)
                .map(|i| create(&format!("notes/{i}.md"), "no"))
                .collect(),
        ] {
            let error = workspace_config::commit(
                api,
                &fixture.worker,
                commit_request(&attached.connection_id, &observed, changes),
            )
            .await
            .unwrap_err();
            assert_eq!(error.classification, Classification::NotCommitted);
            assert_eq!(
                api.config_store
                    .load_workspace_config(TEST_WORKSPACE_ID)
                    .unwrap()
                    .unwrap(),
                before
            );
            assert!(!error.message.contains("Decodal\""));
        }
        for (paths, depth) in [
            (vec!["".to_string()], 9),
            (vec!["/host/secret".to_string()], 0),
            (vec!["".to_string(); 257], 0),
        ] {
            assert!(
                workspace_config::observe(
                    api,
                    &fixture.worker,
                    Observe {
                        connection_id: attached.connection_id.clone(),
                        paths,
                        depth
                    }
                )
                .await
                .is_err()
            );
        }
        let missing = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["notes/missing.md"],
            0,
        )
        .await;
        assert_eq!(
            missing.nodes[0].kind,
            server_api::WorkspaceConfigNodeKind::Missing
        );
        assert_eq!(missing.nodes[0].operations, ["create"]);
        api.config_store.with_conn(|conn| { conn.execute_batch("CREATE TRIGGER config_test_save_failure BEFORE INSERT ON workspace_config_entries WHEN NEW.path='notes/fail.md' BEGIN SELECT RAISE(ABORT,'private /host/path'); END;")?;Ok(()) }).unwrap();
        let error = workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &attached.connection_id,
                &observed,
                vec![create("notes/fail.md", "safe")],
            ),
        )
        .await
        .unwrap_err();
        // The API conservatively marks failures past the auxiliary-effects boundary unknown.
        assert_eq!(error.classification, Classification::Unknown);
        assert!(!error.message.contains("private"));
        assert_eq!(
            api.config_store
                .load_workspace_config(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap(),
            before
        );
        api.config_store
            .with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER config_test_save_failure;")?;
                Ok(())
            })
            .unwrap();
        workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &attached.connection_id,
                &observed,
                vec![create("notes/ok.md", "ok")],
            ),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn workspace_config_operation_and_revoke_share_the_detach_session_lock() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        let granted = grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(api, &fixture.worker, &attached.connection_id, &[""], 0).await;
        let lock = current_worker_session_lock(api, &fixture.worker);
        let held = lock.lock().await;
        let mut operation = Box::pin(workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(
                &attached.connection_id,
                &observed,
                vec![create("notes/locked.md", "no")],
            ),
        ));
        let owner = test_owner_actor();
        let mut revoke = Box::pin(workspace_config::revoke_grant(
            api,
            &owner,
            TEST_WORKSPACE_ID,
            &granted.grant_id,
        ));
        // Poll once, without sleeps/time-based ordering: both must be fenced by held lock.
        assert!(matches!(
            futures::poll!(operation.as_mut()),
            std::task::Poll::Pending
        ));
        assert!(matches!(
            futures::poll!(revoke.as_mut()),
            std::task::Poll::Pending
        ));
        drop(held);
        // Commit queued first, then revocation; subsequent calls cannot use its cached validator.
        operation.await.unwrap();
        revoke.await.unwrap();
        assert_eq!(
            workspace_config::commit(
                api,
                &fixture.worker,
                commit_request(
                    &attached.connection_id,
                    &observed,
                    vec![create("notes/after.md", "no")]
                )
            )
            .await
            .unwrap_err()
            .status,
            403
        );
    }
    #[tokio::test]
    async fn workspace_config_grant_restart_source_immutability_and_owner_acceptance_guard() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        let granted = grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let restarted = SqliteWorkspaceStore::open(&api.config.database_path).unwrap();
        assert_eq!(
            restarted
                .get_workspace_config_grant(TEST_WORKSPACE_ID, &granted.grant_id)
                .unwrap(),
            Some(granted.clone())
        );
        assert!(
            restarted
                .get_workspace_config_grant("foreign-workspace", &granted.grant_id)
                .unwrap()
                .is_none()
        );
        let record = api
            .store
            .get_workdir_registry(TEST_WORKSPACE_ID, &attached.working_directory_id)
            .unwrap()
            .unwrap();
        let mut forged = record.clone();
        forged.source = WorkdirRegistrySource::Repository {
            runtime_id: fixture.worker.runtime_id.clone(),
            repository_id: test_repository_id(api),
        };
        assert!(
            api.store.upsert_workdir_registry(&forged).is_err(),
            "Runtime observation must never replace a logical config source identity"
        );
        assert_eq!(
            api.store
                .get_workdir_registry(TEST_WORKSPACE_ID, &attached.working_directory_id)
                .unwrap(),
            Some(record)
        );
        let second = second_worker(&fixture).await;
        let requested = server_api::WorkspaceConfigGrantResponse {
            grant_id: Uuid::now_v7().to_string(),
            workspace_id: TEST_WORKSPACE_ID.into(),
            runtime_id: second.runtime_id.clone(),
            worker_id: second.worker_id.clone(),
            working_directory_id: Uuid::now_v7().to_string(),
            access: Access::ReadOnly,
            revoked: false,
        };
        assert!(
            api.store
                .create_workspace_config_grant(&requested, "not-owner")
                .is_err(),
            "storage transaction rechecks owner acceptance"
        );
        api.config_store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspaces SET state='deleting' WHERE workspace_id=?1",
                    [TEST_WORKSPACE_ID],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            api.store
                .create_workspace_config_grant(&requested, &test_owner_actor().account_id)
                .is_err(),
            "existing Workspace mutation lifecycle gate must also fence grants"
        );
        api.config_store
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE workspaces SET state='active' WHERE workspace_id=?1",
                    [TEST_WORKSPACE_ID],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(
            api.store
                .current_workspace_config_grant(TEST_WORKSPACE_ID, &second)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn workspace_config_auxiliary_secret_effects_are_unknown_not_false_rollback() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["", "main.dcdl"],
            0,
        )
        .await;
        let main = observed
            .nodes
            .iter()
            .find(|node| node.path == "main.dcdl")
            .unwrap();
        let before = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let count = || {
            api.config_store
                .with_conn(|conn| {
                    conn.query_row(
                        "SELECT COUNT(*) FROM repository_ssh_credentials WHERE workspace_id=?1",
                        [TEST_WORKSPACE_ID],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(Error::from)
                })
                .unwrap()
        };
        let before_secrets = count();
        let source = r#"{ repository_access = { missing_repository = { ssh = { credential = "workspace-default"; host_trust = "missing-host-trust"; access = "read_write"; }; }; }; } as WorkspaceConfigSchema"#;
        let change = server_api::ConfigTreeChange::Update {
            path: "main.dcdl".into(),
            expected_digest: main.digest.clone().unwrap(),
            content: source.into(),
        };
        let error = workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(&attached.connection_id, &observed, vec![change]),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.classification,
            Classification::Unknown,
            "repository projection can create a default credential before rejecting source/reference validation"
        );
        assert_eq!(
            api.config_store
                .load_workspace_config(TEST_WORKSPACE_ID)
                .unwrap()
                .unwrap(),
            before,
            "canonical config stays unchanged"
        );
        assert!(
            count() > before_secrets,
            "auxiliary credential side effect must be demonstrably durable; cannot label this NotCommitted"
        );
        assert!(!error.message.contains("host_trust") && !error.message.contains("private"));
    }
    #[tokio::test]
    async fn workspace_config_observe_node_limit_is_complete_or_error_not_silent_truncation() {
        let fixture = manual_worker_assignment_fixture().await;
        let api = &fixture.api;
        grant(api, &fixture.worker, Access::ReadWrite).await;
        let attached = attach(api, &fixture.worker).await;
        let observed = observe(api, &fixture.worker, &attached.connection_id, &[""], 0).await;
        let state = api
            .config_store
            .load_workspace_config(TEST_WORKSPACE_ID)
            .unwrap()
            .unwrap();
        let count = config_source::MAX_ENTRY_COUNT - state.snapshot.entries.len();
        let changes = (0..count)
            .map(|i| create(&format!("bounded/{i}.md"), "text"))
            .collect();
        workspace_config::commit(
            api,
            &fixture.worker,
            commit_request(&attached.connection_id, &observed, changes),
        )
        .await
        .unwrap();
        let overflow = workspace_config::observe(
            api,
            &fixture.worker,
            Observe {
                connection_id: attached.connection_id.clone(),
                paths: vec!["".into()],
                depth: 8,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(overflow.status, 413);
        let root = observe(api, &fixture.worker, &attached.connection_id, &[""], 0).await;
        assert_eq!(root.nodes.len(), 1);
        let directory = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["bounded"],
            1,
        )
        .await;
        assert_eq!(directory.nodes.len(), count + 1);
        assert!(directory.nodes.len() <= 256);
        let direct = observe(
            api,
            &fixture.worker,
            &attached.connection_id,
            &["bounded/0.md"],
            0,
        )
        .await;
        assert_eq!(
            direct.nodes[0],
            *directory
                .nodes
                .iter()
                .find(|node| node.path == "bounded/0.md")
                .unwrap(),
            "listing and direct lookup must expose identical metadata/auth/validator"
        );
    }
}
