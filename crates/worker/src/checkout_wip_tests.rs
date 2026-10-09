//! Exercises native checkout interactions through the real WIP Client/HTTP codec.
use super::*;
use tempfile::TempDir;
use workdir::{
    LocalWorkdirSession, WorkdirAttachmentAlias, WorkdirSessionCapabilities, WorkdirSessionHandle,
    WorkdirSessionRouter,
};

fn session(dir: &TempDir, read_only: bool) -> WorkdirSessionHandle {
    let caps = if read_only {
        WorkdirSessionCapabilities::READ_ONLY
    } else {
        WorkdirSessionCapabilities::READ_WRITE
    };
    Arc::new(LocalWorkdirSession::materialized(
        dir.path().to_path_buf(),
        dir.path().to_path_buf(),
        manifest::SharedScope::new(manifest::Scope::writable(dir.path()).unwrap()),
        caps,
    ))
}
fn runtime(
    router: Arc<WorkdirSessionRouter>,
    permissions: Option<ToolPermissionConfig>,
) -> WipRuntime {
    let mut registry = WipMountRegistry::new();
    crate::checkout::mount_checkouts(&mut registry, router, tools::Tracker::new(), permissions)
        .unwrap();
    WipRuntime::new(
        WipHost::new(registry),
        SecurityContext::new("checkout-test-worker"),
        0,
    )
    .unwrap()
}
async fn interface(r: &WipRuntime, path: &str) -> wip_protocol::InterfaceReference {
    r.tree(path.into(), 0, true).await.unwrap();
    let p = r.host.projection_live(path).await.unwrap().unwrap();
    r.prepare_call_observations(path.into(), true)
        .await
        .unwrap();
    p.interface
}
async fn call(
    r: &WipRuntime,
    path: &str,
    operation: &str,
    args: Json,
) -> Result<ToolOutput, ToolError> {
    let i = interface(r, path).await;
    r.call(path.into(), i, operation.into(), args, Default::default())
        .await
}

#[tokio::test]
async fn checkout_wip_search_preserves_root_and_nested_ignore_rules_and_links() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    for (name, content) in [
        (".ignore", "root-denied.txt\npriority.txt\n"),
        (".gitignore", "git-denied.txt\n"),
        ("nested/.gitignore", "!priority.txt\nnested-denied.txt\n"),
    ] {
        std::fs::write(dir.path().join(name), content).unwrap();
    }
    for name in [
        "visible.txt",
        "root-denied.txt",
        "git-denied.txt",
        "nested/visible.txt",
        "nested/root-denied.txt",
        "nested/git-denied.txt",
        "nested/priority.txt",
        "nested/nested-denied.txt",
    ] {
        std::fs::write(dir.path().join(name), "needle fixture\n").unwrap();
    }
    let source = session(&dir, false);
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(WorkdirAttachmentAlias::new("main").unwrap(), source.clone())
        .unwrap();
    let r = runtime(router, None);
    for base in ["", "nested"] {
        let ordinary = source
            .glob(workdir::GlobRequest {
                path: workdir::WorkdirPath::new(base).unwrap(),
                pattern: "**/*.txt".into(),
                limit: 100,
            })
            .await
            .unwrap();
        let expected = ordinary
            .paths
            .iter()
            .map(|path| format!("/checkouts/main/{}", path.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), if base.is_empty() { 2 } else { 1 });
        let route = if base.is_empty() {
            "/checkouts/main"
        } else {
            "/checkouts/main/nested"
        };
        for (operation, args) in [
            ("glob", json!({"pattern":"**/*.txt"})),
            (
                "grep",
                json!({"pattern":"needle","output_mode":"files_with_matches"}),
            ),
        ] {
            let output = call(&r, route, operation, args).await.unwrap();
            let result: Json = serde_json::from_str(output.content.as_deref().unwrap()).unwrap();
            let mut paths = result["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["entry"].as_str().unwrap().to_string())
                .collect::<Vec<_>>();
            // Grep retains traversal order; Glob sorts its result paths.
            paths.sort();
            assert_eq!(paths, expected, "{operation} at {route}");
            for path in paths {
                assert!(r.host.projection_live(&path).await.unwrap().is_some());
            }
            assert!(!output.content.unwrap().contains("denied.txt"));
        }
    }
}

#[tokio::test]
async fn checkout_wip_native_roundtrip_search_read_edit_write_create() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("src/deep")).unwrap();
    std::fs::write(
        dir.path().join("src/deep/a.txt"),
        "first\nneedle needle\nlast\n",
    )
    .unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, false),
        )
        .unwrap();
    let r = runtime(router, None);
    let root = r.host.observe_live("/", 2).await.unwrap();
    assert_eq!(root.children.unwrap()[0].object.name, "checkouts");
    let file = "/checkouts/main/src/deep/a.txt";
    let glob = call(&r, "/checkouts/main", "glob", json!({"pattern":"**/*.txt"}))
        .await
        .unwrap();
    assert!(glob.content.unwrap().contains(file));
    let grep = call(
        &r,
        "/checkouts/main/src",
        "grep",
        json!({"pattern":"needle","output_mode":"files_with_matches"}),
    )
    .await
    .unwrap();
    let grep_result: Json = serde_json::from_str(grep.content.as_deref().unwrap()).unwrap();
    assert_eq!(grep_result["items"][0]["entry"], file);
    let followed = grep_result["items"][0]["entry"].as_str().unwrap();
    assert_eq!(followed, file);
    assert!(
        call(
            &r,
            file,
            "edit",
            json!({"old_string":"needle","new_string":"done"})
        )
        .await
        .is_err()
    );
    let read = call(&r, file, "read", json!({"offset":1,"limit":1}))
        .await
        .unwrap();
    assert!(read.content.unwrap().contains("needle needle"));
    assert!(
        call(
            &r,
            file,
            "edit",
            json!({"old_string":"needle","new_string":"done"})
        )
        .await
        .is_err()
    );
    call(
        &r,
        file,
        "edit",
        json!({"old_string":"needle","new_string":"done","replace_all":true}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/deep/a.txt")).unwrap(),
        "first\ndone done\nlast\n"
    );
    call(&r, file, "write", json!({"content":"saved\n"}))
        .await
        .unwrap();
    let reread = call(&r, file, "read", json!({})).await.unwrap();
    assert!(reread.content.unwrap().contains("saved"));
    call(
        &r,
        "/checkouts/main/src",
        "create_file",
        json!({"path":"new/deeper.txt","content":"created\n"}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("src/new/deeper.txt")).unwrap(),
        "created\n"
    );
    assert!(
        call(
            &r,
            "/checkouts/main/src",
            "create_file",
            json!({"path":"new/deeper.txt","content":"overwrite"})
        )
        .await
        .is_err()
    );
    call(&r, "/checkouts/main/src/new/deeper.txt", "read", json!({}))
        .await
        .unwrap();
}

#[tokio::test]
async fn checkout_wip_multiple_aliases_read_only_and_escape_rejection() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    std::fs::write(a.path().join("a.txt"), "A").unwrap();
    std::fs::write(b.path().join("b.txt"), "B").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("a").unwrap(),
            session(&a, false),
        )
        .unwrap();
    router
        .attach(WorkdirAttachmentAlias::new("b").unwrap(), session(&b, true))
        .unwrap();
    let r = runtime(router, None);
    let observed = r.host.observe_live("/checkouts", 2).await.unwrap();
    assert_eq!(observed.children.unwrap().len(), 2);
    let bfile = r
        .host
        .projection_live("/checkouts/b/b.txt")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        bfile
            .descriptor
            .operations
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>(),
        vec!["read"]
    );
    assert!(
        r.host
            .projection_live("/checkouts/b/a.txt")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        r.host
            .projection_live("/checkouts/a/../b/b.txt")
            .await
            .is_err()
    );
    assert!(
        call(
            &r,
            "/checkouts/a",
            "glob",
            json!({"pattern":"*","path":"../b"})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &r,
            "/checkouts/a",
            "create_file",
            json!({"path":"/outside.txt","content":"bad"})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &r,
            "/checkouts/a/a.txt",
            "read",
            json!({"target_workdir":"b"})
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &r,
            "/checkouts/a/a.txt",
            "read",
            json!({"file_path":"b.txt"})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn checkout_wip_detach_alias_reuse_and_restore_fence_old_references() {
    let a = TempDir::new().unwrap();
    let b = TempDir::new().unwrap();
    std::fs::write(a.path().join("file.txt"), "same").unwrap();
    std::fs::write(b.path().join("file.txt"), "same").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    let alias = WorkdirAttachmentAlias::new("main").unwrap();
    router.attach(alias.clone(), session(&a, false)).unwrap();
    let r = runtime(router.clone(), None);
    let path = "/checkouts/main/file.txt";
    call(&r, path, "read", json!({})).await.unwrap();
    let old = r.host.projection_live(path).await.unwrap().unwrap();
    router.detach(&alias).await.unwrap();
    assert!(r.host.projection_live(path).await.unwrap().is_none());
    assert!(r.host.fetch_interface_live(&old.interface).await.is_err());
    router.attach(alias, session(&b, false)).unwrap();
    let new = r.host.projection_live(path).await.unwrap().unwrap();
    assert_ne!(old.interface, new.interface);
    assert_ne!(old.object.validator, new.object.validator);
    assert!(r.host.fetch_interface_live(&old.interface).await.is_err());
    assert!(
        call(
            &r,
            path,
            "write",
            json!({"content":"must read new attachment"})
        )
        .await
        .is_err()
    );
    let restored = runtime(router, None);
    let next = restored.host.projection_live(path).await.unwrap().unwrap();
    assert_ne!(new.interface, next.interface);
    assert_ne!(new.object.validator, next.object.validator);
    assert!(
        restored
            .host
            .fetch_interface_live(&new.interface)
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(b.path().join("file.txt")).unwrap(),
        "same"
    );
}

#[tokio::test]
async fn checkout_wip_provider_validator_rejects_same_content_inode_substitution() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.txt"), "same").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, false),
        )
        .unwrap();
    let r = runtime(router, None);
    let path = "/checkouts/main/file.txt";
    call(&r, path, "read", json!({})).await.unwrap();
    let p = r.host.projection_live(path).await.unwrap().unwrap();
    std::fs::write(dir.path().join("replacement"), "same").unwrap();
    std::fs::rename(dir.path().join("replacement"), dir.path().join("file.txt")).unwrap();
    // Call the captured handler directly: provider must reject even if Host validation was earlier.
    let out = p
        .handler
        .call(
            "write",
            &BTreeMap::from([("content".into(), Value::String("bad".into()))]),
            WipCallContext {
                execution: Default::default(),
                security_context: "checkout-test-worker".into(),
            },
        )
        .await;
    assert!(matches!(
        out,
        Err(WipOperationError::Protocol(ProtocolError {
            code: ProtocolErrorCode::ValidatorMismatch,
            ..
        }))
    ));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "same"
    );
}

#[tokio::test]
async fn checkout_wip_policy_is_not_feature_authority_and_compatibility_is_replaced() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.txt"), "secret").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, false),
        )
        .unwrap();
    let permissions = ToolPermissionConfig {
        default_action: ToolPermissionAction::Deny,
        rules: Vec::new(),
    };
    let r = runtime(router.clone(), Some(permissions));
    assert!(
        r.host
            .projection_live("/checkouts/main/file.txt")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        r.host
            .observe_live("/checkouts", 1)
            .await
            .unwrap()
            .children
            .unwrap()
            .is_empty()
    );
    let mut registry = WipMountRegistry::new();
    crate::checkout::mount_checkouts(&mut registry, router.clone(), tools::Tracker::new(), None)
        .unwrap();
    for def in tools::routed_builtin_tools(router, tools::Tracker::new(), dir.path().join("bash")) {
        let (meta, tool) = def();
        let p = compatibility_projection(meta, tool, None).unwrap();
        registry.mount(p).unwrap();
    }
    for tool in ["Read", "Edit", "Write", "Glob", "Grep"] {
        assert!(!registry.routes().any(|p| p == format!("/tools/{tool}")));
    }
    assert!(registry.routes().any(|p| p == "/tools/Bash"));
}

#[tokio::test]
async fn checkout_wip_bounded_observation_and_deep_subtree_collision() {
    let dir = TempDir::new().unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, false),
        )
        .unwrap();
    let mut registry = WipMountRegistry::new();
    crate::checkout::mount_checkouts(&mut registry, router, tools::Tracker::new(), None).unwrap();
    let placeholder = registry.mounts["/checkouts"].resolved();
    let mut collision = placeholder.clone();
    collision.route = "/checkouts/main/a".into();
    collision.object.name = "a".into();
    assert!(matches!(
        registry.mount(collision),
        Err(WipMountError::RouteCollision { .. })
    ));
    let host = WipHost::new(registry);
    assert_eq!(
        host.observe_live("/checkouts", 33).await.unwrap_err().code,
        ProtocolErrorCode::ResourceLimitExceeded
    );
}

#[tokio::test]
async fn checkout_wip_post_operation_validators_support_cached_chains() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("file.txt"), "old\n").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, false),
        )
        .unwrap();
    let r = runtime(router, None);
    let path = "/checkouts/main/file.txt";
    let i = interface(&r, path).await;
    // No rediscovery or Interface refresh between any of these operations.
    for (op, args) in [
        ("read", json!({})),
        ("edit", json!({"old_string":"old","new_string":"edited"})),
        ("write", json!({"content":"saved\n"})),
        ("read", json!({})),
    ] {
        r.call(path.into(), i.clone(), op.into(), args, Default::default())
            .await
            .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "saved\n"
    );
    let parent = "/checkouts/main";
    let i = interface(&r, parent).await;
    for name in ["one.txt", "nested/two.txt"] {
        r.call(
            parent.into(),
            i.clone(),
            "create_file".into(),
            json!({"path":name,"content":"new"}),
            Default::default(),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/two.txt")).unwrap(),
        "new"
    );
}

#[tokio::test]
async fn checkout_wip_scoped_child_search_and_revoke_cannot_expose_siblings() {
    use workdir::{
        WorkdirPath, WorkdirToolBroker, WorkdirToolScope, WorkdirToolScopePermission as Permission,
        WorkdirToolScopeRule,
    };
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("child")).unwrap();
    std::fs::write(dir.path().join("child/a.txt"), "visible needle").unwrap();
    std::fs::write(dir.path().join("private-sibling-name.txt"), "hidden needle").unwrap();
    let broker = Arc::new(WorkdirToolBroker::new(session(&dir, false)));
    let lease = broker
        .scope(WorkdirToolScope {
            rules: vec![WorkdirToolScopeRule {
                target: WorkdirPath::new("child").unwrap(),
                permission: Permission::Write,
                recursive: true,
                symlink_policy: Default::default(),
            }],
            cwd: WorkdirPath::new("child").unwrap(),
            command: false,
        })
        .await
        .unwrap();
    let parent_router = Arc::new(WorkdirSessionRouter::new());
    parent_router
        .attach(
            WorkdirAttachmentAlias::new("parent").unwrap(),
            broker.tool_session(),
        )
        .unwrap();
    let parent = runtime(parent_router, None);
    let parent_file = parent
        .host
        .projection_live("/checkouts/parent/child/a.txt")
        .await
        .unwrap()
        .unwrap();
    assert!(
        !parent_file
            .descriptor
            .operations
            .iter()
            .any(|op| op.name == "write" || op.name == "edit")
    );
    let child_router = Arc::new(WorkdirSessionRouter::new());
    child_router
        .attach(
            WorkdirAttachmentAlias::new("child").unwrap(),
            lease.tool_session(),
        )
        .unwrap();
    let child = runtime(child_router, None);
    let observed = child
        .host
        .observe_live("/checkouts/child", 2)
        .await
        .unwrap();
    assert!(observed.children.unwrap().is_empty());
    for (op, args) in [
        ("list", json!({"limit":1})),
        ("glob", json!({"pattern":"**/*"})),
        (
            "grep",
            json!({"pattern":"needle","output_mode":"files_with_matches"}),
        ),
    ] {
        let output = call(&child, "/checkouts/child", op, args)
            .await
            .unwrap()
            .content
            .unwrap();
        assert!(output.contains("/checkouts/child/a.txt"));
        assert!(!output.contains("private-sibling-name"));
        assert!(!output.contains("hidden needle"));
    }
    assert!(
        child
            .host
            .projection_live("/checkouts/child/private-sibling-name.txt")
            .await
            .unwrap()
            .is_none()
    );
    let file = "/checkouts/child/a.txt";
    let i = interface(&child, file).await;
    child
        .call(
            file.into(),
            i.clone(),
            "read".into(),
            json!({}),
            Default::default(),
        )
        .await
        .unwrap();
    child
        .call(
            file.into(),
            i.clone(),
            "write".into(),
            json!({"content":"changed"}),
            Default::default(),
        )
        .await
        .unwrap();
    lease.close().await.unwrap();
    assert!(child.host.projection_live(file).await.unwrap().is_none());
    assert!(
        child
            .call(
                file.into(),
                i,
                "write".into(),
                json!({"content":"after revoke"}),
                Default::default()
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("child/a.txt")).unwrap(),
        "changed"
    );
}

pub(super) fn json_content(output: ToolOutput) -> Json {
    serde_json::from_str(output.content.as_deref().unwrap()).unwrap()
}
pub(super) async fn model_operation(
    r: Arc<WipRuntime>,
    name: &str,
    args: Json,
) -> Result<ToolOutput, ToolError> {
    let (_, tool) = wip_tool_definitions(r)
        .into_iter()
        .map(|definition| definition())
        .find(|(meta, _)| meta.name == name)
        .unwrap();
    tool.execute(&args.to_string(), Default::default()).await
}
pub(super) async fn model_inspect(r: Arc<WipRuntime>, path: &str) -> Json {
    json_content(
        model_operation(r, "Inspect", json!({"path":path}))
            .await
            .unwrap(),
    )
}
pub(super) async fn model_invoke(
    r: Arc<WipRuntime>,
    inspected: &Json,
    operation: &str,
    arguments: Json,
) -> Result<ToolOutput, ToolError> {
    model_operation(r, "Invoke", json!({"path":inspected["path"], "interface":inspected["interfaces"][0]["reference"], "operation":operation, "arguments":arguments})).await
}

#[tokio::test]
async fn checkout_depth_boundaries_are_independent_of_empty_small_or_large_contents() {
    for count in [0, 2, 1100] {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
        for n in 0..count {
            std::fs::write(dir.path().join(format!("file-{n}")), "fixture").unwrap();
        }
        let router = Arc::new(WorkdirSessionRouter::new());
        router
            .attach(
                WorkdirAttachmentAlias::new("main").unwrap(),
                session(&dir, true),
            )
            .unwrap();
        let r = runtime(router, None);
        let one = r.host.observe_live("/checkouts", 1).await.unwrap();
        assert!(one.children.unwrap()[0].children.is_none());
        for depth in [2, 32] {
            let observation = r.host.observe_live("/checkouts", depth).await.unwrap();
            let entrances = observation.children.unwrap();
            assert_eq!(entrances.len(), 1);
            assert_eq!(entrances[0].children, Some(vec![]));
        }
        let world = r.host.observe_live("/", 32).await.unwrap();
        assert_eq!(
            world.children.unwrap()[0].children.as_ref().unwrap()[0].children,
            Some(vec![])
        );
        for path in ["/checkouts/main", "/checkouts/main/nested/deep"] {
            assert!(
                r.host
                    .observe_live(path, 0)
                    .await
                    .unwrap()
                    .children
                    .is_none()
            );
            assert_eq!(
                r.host.observe_live(path, 1).await.unwrap().children,
                Some(vec![])
            );
        }
    }
}

#[tokio::test]
async fn checkout_model_list_glob_grep_entries_are_lazy_and_never_become_tree_edges() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("empty")).unwrap();
    std::fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    std::fs::write(dir.path().join("nested/deep/a.txt"), "needle\n").unwrap();
    std::fs::write(dir.path().join("z.txt"), "needle\n").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, true),
        )
        .unwrap();
    let r = Arc::new(runtime(router, None));
    let entrance = model_inspect(r.clone(), "/checkouts/main").await;
    assert_eq!(entrance["path"], "/checkouts/main");
    assert_eq!(
        entrance["interfaces"][0]["reference"]["scope"],
        "/checkouts/main"
    );
    let projection = r
        .host
        .projection_live("/checkouts/main")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        entrance["object_signature"],
        wip_text_view::render_object(&projection.object).unwrap()
    );
    assert!(entrance["interfaces"][0].get("signature").is_none());
    let definition = model_inspect(
        r.clone(),
        entrance["interfaces"][0]["path"].as_str().unwrap(),
    )
    .await;
    assert_eq!(
        definition["interface_signature"],
        wip_text_view::render_interface(&wip_text_view::Interface {
            reference: &projection.interface,
            descriptor: &projection.descriptor
        })
        .unwrap()
    );
    assert!(
        definition["interface_signature"]
            .as_str()
            .unwrap()
            .contains("entry")
    );
    let mut after = None;
    let mut items = vec![];
    loop {
        let mut args = json!({"limit":1});
        if let Some(cursor) = after {
            args["after"] = cursor;
        }
        let page = json_content(
            model_invoke(r.clone(), &entrance, "list", args)
                .await
                .unwrap(),
        );
        assert_eq!(page["items"].as_array().unwrap().len(), 1);
        items.push(page["items"][0].clone());
        if page["truncated"] == false {
            assert!(page.get("after").is_none());
            break;
        }
        after = Some(page["after"].clone());
    }
    assert_eq!(
        items
            .iter()
            .map(|item| item["entry"].clone())
            .collect::<Vec<_>>(),
        vec![
            json!("/checkouts/main/empty"),
            json!("/checkouts/main/nested"),
            json!("/checkouts/main/z.txt")
        ]
    );
    assert_eq!(items[0]["kind"], "directory");
    assert_eq!(items[2]["kind"], "file");
    let empty = model_inspect(r.clone(), items[0]["entry"].as_str().unwrap()).await;
    assert_eq!(
        json_content(
            model_invoke(r.clone(), &empty, "list", json!({}))
                .await
                .unwrap()
        )["items"],
        json!([])
    );
    let deep = model_inspect(r.clone(), "/checkouts/main/nested/deep").await;
    assert_eq!(
        deep["interfaces"][0]["reference"]["scope"],
        "/checkouts/main"
    );
    for (operation, args) in [
        ("list", json!({})),
        ("glob", json!({"pattern":"*.txt"})),
        (
            "grep",
            json!({"pattern":"needle", "output_mode":"files_with_matches"}),
        ),
    ] {
        let found = json_content(
            model_invoke(r.clone(), &deep, operation, args)
                .await
                .unwrap(),
        );
        let path = found["items"][0]["entry"].as_str().unwrap();
        assert_eq!(path, "/checkouts/main/nested/deep/a.txt");
        // No result-wide eager acquisition by Host or Client.
        if operation == "list" {
            let state = r.state.lock().unwrap();
            assert!(state.client.object(&state.session, path).is_none());
        }
        let inspected = model_inspect(r.clone(), path).await;
        let read = json_content(
            model_invoke(r.clone(), &inspected, "read", json!({}))
                .await
                .unwrap(),
        );
        assert!(read["content"].as_str().unwrap().contains("needle"));
    }
    let tree = json_content(
        model_operation(r.clone(), "Tree", json!({"path":"/checkouts", "depth":8}))
            .await
            .unwrap(),
    );
    assert_eq!(tree["tree"]["children"][0]["children"], json!([]));
    std::fs::remove_file(dir.path().join("nested/deep/a.txt")).unwrap();
    assert!(
        model_operation(
            r,
            "Inspect",
            json!({"path":"/checkouts/main/nested/deep/a.txt", "refresh":true})
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn checkout_list_permission_cannot_be_bypassed_with_glob_or_command_identity() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "fixture").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    router
        .attach(
            WorkdirAttachmentAlias::new("main").unwrap(),
            session(&dir, true),
        )
        .unwrap();
    let r = runtime(
        router,
        Some(ToolPermissionConfig {
            default_action: ToolPermissionAction::Allow,
            rules: vec![manifest::ToolPermissionRule {
                tool: "List".into(),
                pattern: "*".into(),
                action: ToolPermissionAction::Deny,
            }],
        }),
    );
    let p = r
        .host
        .projection_live("/checkouts/main")
        .await
        .unwrap()
        .unwrap();
    assert!(!p.descriptor.operations.iter().any(|op| op.name == "list"));
    assert!(p.descriptor.operations.iter().any(|op| op.name == "glob"));
    let denied = p
        .handler
        .call(
            "list",
            &BTreeMap::new(),
            WipCallContext {
                execution: Default::default(),
                security_context: "fixture".into(),
            },
        )
        .await;
    assert!(
        matches!(denied, Err(WipOperationError::Protocol(e)) if e.code == ProtocolErrorCode::PermissionDenied)
    );
}

#[tokio::test]
async fn checkout_ancestor_scope_rejects_siblings_prefixes_refs_and_old_connection_before_read() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "same\n").unwrap();
    let router = Arc::new(WorkdirSessionRouter::new());
    let main = WorkdirAttachmentAlias::new("main").unwrap();
    router.attach(main.clone(), session(&dir, true)).unwrap();
    router
        .attach(
            WorkdirAttachmentAlias::new("main-extra").unwrap(),
            session(&dir, true),
        )
        .unwrap();
    let r = runtime(router.clone(), None);
    let path = "/checkouts/main/a.txt";
    let p = r.host.projection_live(path).await.unwrap().unwrap();
    let fetched = r.host.fetch_interface_live(&p.interface).await.unwrap();
    assert!(
        fetched.scope_ref.is_none(),
        "ref-less publication is legitimate"
    );
    // Ordinary content changes do not terminate the entrance's Interface lifetime.
    std::fs::write(dir.path().join("new.txt"), "fixture").unwrap();
    assert_eq!(
        r.host
            .fetch_interface_live(&p.interface)
            .await
            .unwrap()
            .interface,
        p.interface
    );
    let request = |reference, scope_ref| CallOperationRequest {
        target: wip_protocol::Target {
            path: path.into(),
            validator: None,
        },
        interface: wip_protocol::InterfaceTarget {
            reference,
            scope_ref,
            validator: None,
        },
        operation: "read".into(),
        arguments: BTreeMap::new(),
    };
    for scope in ["/checkouts/main-extra", "/checkouts/main/a"] {
        let mut wrong = p.interface.clone();
        wrong.scope = scope.into();
        assert!(r.host.fetch_interface_live(&wrong).await.is_err());
        let forged = request(wrong, None);
        let result = r
            .host
            .call(
                forged,
                WipCallContext {
                    execution: Default::default(),
                    security_context: "fixture".into(),
                },
            )
            .await;
        assert!(
            matches!(result,Err(WipOperationError::Protocol(e)) if e.code == ProtocolErrorCode::InterfaceMismatch)
        );
    }
    let forged = request(p.interface.clone(), Some("old".into()));
    assert!(
        matches!(r.host.call(forged, WipCallContext { execution:Default::default(),security_context:"fixture".into() }).await, Err(WipOperationError::Protocol(e)) if e.code == ProtocolErrorCode::InterfaceMismatch)
    );
    router.detach(&main).await.unwrap();
    router.attach(main, session(&dir, true)).unwrap();
    let forged = request(p.interface.clone(), None);
    assert!(
        matches!(r.host.call(forged, WipCallContext { execution:Default::default(),security_context:"fixture".into() }).await, Err(WipOperationError::Protocol(e)) if e.code == ProtocolErrorCode::InterfaceMismatch)
    );
}
