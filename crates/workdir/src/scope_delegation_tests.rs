// Included in scope::tests; exercise real provider resolution through stacked brokers.

fn filesystem_scope(path: &str, permission: WorkdirToolScopePermission) -> WorkdirToolScope {
    let mut scope = request(path, permission);
    scope.command = false;
    scope
}

fn authorization(
    path: &str,
    permission: WorkdirToolScopePermission,
) -> WorkdirScopeAuthorizationRequest {
    WorkdirScopeAuthorizationRequest {
        rules: filesystem_scope("", permission).rules,
        path: fs_path(path),
        permission,
    }
}

#[tokio::test]
async fn stacked_brokers_delegate_all_filesystem_provider_kinds() {
    use crate::{BoundedReadLimits, ExternalWorkdirRoot};
    for kind in [
        "repository",
        "read-only-wrapper",
        "external-read",
        "external-write",
        "external-read-command",
        "external-all",
    ] {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("child")).unwrap();
        fs::create_dir(root.path().join("other")).unwrap();
        fs::write(root.path().join("child/input"), "visible").unwrap();
        fs::write(root.path().join("other/secret"), "hidden").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("../other", root.path().join("child/escape")).unwrap();
        let capabilities = match kind {
            "read-only-wrapper" | "external-read" => WorkdirSessionCapabilities::READ_ONLY,
            "external-write" => WorkdirSessionCapabilities::READ_WRITE,
            "external-read-command" => WorkdirSessionCapabilities::READ_ONLY
                .union(WorkdirSessionCapabilities::COMMAND_ONLY),
            _ => WorkdirSessionCapabilities::ALL,
        };
        let source: WorkdirSessionHandle = if kind.starts_with("external-") {
            Arc::new(
                LocalWorkdirSession::external_with_capabilities_pinned(
                    Workdir::new(kind),
                    ExternalWorkdirRoot::pin(root.path()).unwrap(),
                    BoundedReadLimits::new(4096, 1024).unwrap(),
                    capabilities,
                )
                .unwrap(),
            )
        } else {
            let source: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
                Scope::writable(root.path()).unwrap(),
                root.path().to_path_buf(),
            ));
            if kind == "read-only-wrapper" {
                Arc::new(ReadOnlyWorkdirSession::new(source))
            } else {
                source
            }
        };
        // Backend capability wrapper, attachment routing, then the Worker broker.
        let backend = WorkdirToolBroker::with_capabilities(source, capabilities);
        let router = crate::WorkdirSessionRouter::new();
        let routed = router
            .attach(
                crate::WorkdirAttachmentAlias::new("selected").unwrap(),
                backend.tool_session(),
            )
            .unwrap();
        let worker = WorkdirToolBroker::new(routed);
        let child = worker
            .scope(filesystem_scope("child", WorkdirToolScopePermission::Read))
            .await
            .unwrap();
        assert_eq!(
            child.read(read("input")).await.unwrap().bytes,
            b"visible",
            "{kind}"
        );
        assert!(child.write(write("denied", "no")).await.is_err(), "{kind}");
        #[cfg(unix)]
        assert!(child.read(read("escape/secret")).await.is_err(), "{kind}");
        child.close().await.unwrap();
        let writable = worker
            .scope(filesystem_scope("child", WorkdirToolScopePermission::Write))
            .await;
        if !capabilities.supports(WorkdirSessionCapability::Write) {
            assert!(writable.is_err(), "{kind}");
            continue;
        }
        let writable = writable.unwrap();
        writable.write(write("created", "child")).await.unwrap();
        assert!(
            worker.write(write("child/blocked", "no")).await.is_err(),
            "{kind}"
        );
        assert!(
            worker
                .scope(filesystem_scope("child", WorkdirToolScopePermission::Write))
                .await
                .is_err(),
            "{kind}"
        );
        // A non-overlapping write lease requires the second missing provider method.
        let sibling = worker
            .scope(filesystem_scope("other", WorkdirToolScopePermission::Write))
            .await
            .unwrap();
        sibling.write(write("created", "sibling")).await.unwrap();
        assert!(
            !writable
                .capabilities
                .supports(WorkdirSessionCapability::Command)
        );
        writable.close().await.unwrap();
        worker
            .write(write("child/released", "parent"))
            .await
            .unwrap();
        sibling.close().await.unwrap();
        let command_child = worker
            .scope(request("child", WorkdirToolScopePermission::Write))
            .await;
        if capabilities.supports(WorkdirSessionCapability::Command) {
            let command_child = command_child.unwrap();
            assert_eq!(
                run_command(&command_child.tool_session(), "printf delegated", kind)
                    .await
                    .content,
                "delegated"
            );
            command_child.close().await.unwrap();
        } else {
            assert!(command_child.is_err());
        }
    }
}

#[tokio::test]
async fn stacked_authorization_preserves_ceiling_scope_and_revocation() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("allowed/deep")).unwrap();
    let backend = session(root.path());
    let read_only = WorkdirToolBroker::with_capabilities(
        backend.tool_session(),
        WorkdirSessionCapabilities::READ_ONLY,
    );
    read_only
        .authorize_scope_path(authorization("allowed", WorkdirToolScopePermission::Read))
        .await
        .unwrap();
    assert!(
        read_only
            .authorize_scope_path(authorization("allowed", WorkdirToolScopePermission::Write))
            .await
            .is_err()
    );
    let command_only = WorkdirToolBroker::with_capabilities(
        backend.tool_session(),
        WorkdirSessionCapabilities::COMMAND_ONLY,
    );
    assert!(
        command_only
            .authorize_scope_path(authorization("allowed", WorkdirToolScopePermission::Read))
            .await
            .is_err()
    );
    assert!(
        command_only
            .scope(filesystem_scope(
                "allowed",
                WorkdirToolScopePermission::Read
            ))
            .await
            .is_err()
    );

    let mut limited = filesystem_scope("allowed", WorkdirToolScopePermission::Read);
    limited.rules[0].recursive = false;
    let parent = backend.scope(limited).await.unwrap();
    let stacked = WorkdirToolBroker::new(parent.tool_session());
    let mut valid = filesystem_scope("", WorkdirToolScopePermission::Read);
    valid.rules[0].recursive = false;
    let child = stacked.scope(valid.clone()).await.unwrap();
    assert!(
        stacked
            .scope(filesystem_scope("", WorkdirToolScopePermission::Read))
            .await
            .is_err()
    );
    let mut logical = valid.clone();
    logical.rules[0].symlink_policy = SymlinkPolicy::Logical;
    assert!(stacked.scope(logical).await.is_err());
    assert!(
        stacked
            .authorize_scope_path(authorization("deep/file", WorkdirToolScopePermission::Read))
            .await
            .is_err()
    );
    parent.close().await.unwrap();
    assert!(stacked.scope(valid).await.is_err());
    assert!(child.read(read("deep/file")).await.is_err());
    assert!(
        stacked
            .scope_rules_overlap(WorkdirScopeOverlapRequest {
                left: filesystem_scope("", WorkdirToolScopePermission::Read)
                    .rules
                    .remove(0),
                right: filesystem_scope("", WorkdirToolScopePermission::Read)
                    .rules
                    .remove(0),
            })
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stacked_scope_rebases_rules_paths_and_overlap_once() {
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("base/inner/one")).unwrap();
    fs::create_dir_all(root.path().join("base/inner/two")).unwrap();
    fs::write(root.path().join("base/inner/one/input"), "nested").unwrap();
    let backend = session(root.path());
    let first = backend
        .scope(filesystem_scope("base", WorkdirToolScopePermission::Write))
        .await
        .unwrap();
    let worker = WorkdirToolBroker::new(first.tool_session());
    let second = worker
        .scope(filesystem_scope("inner", WorkdirToolScopePermission::Write))
        .await
        .unwrap();
    let nested = WorkdirToolBroker::new(second.tool_session());
    let leaf = nested
        .scope(filesystem_scope("one", WorkdirToolScopePermission::Write))
        .await
        .unwrap();
    assert_eq!(leaf.read(read("input")).await.unwrap().bytes, b"nested");
    leaf.write(write("output", "yes")).await.unwrap();
    assert_eq!(
        fs::read(root.path().join("base/inner/one/output")).unwrap(),
        b"yes"
    );
    assert!(nested.write(write("one/blocked", "no")).await.is_err());
    let sibling = nested
        .scope(filesystem_scope("two", WorkdirToolScopePermission::Write))
        .await
        .unwrap();
    sibling.write(write("output", "sibling")).await.unwrap();
    assert!(
        nested
            .scope(filesystem_scope("one", WorkdirToolScopePermission::Write))
            .await
            .is_err()
    );
    leaf.close().await.unwrap();
    nested.write(write("one/released", "yes")).await.unwrap();
    sibling.close().await.unwrap();
    first.close().await.unwrap();
    assert!(
        nested
            .authorize_scope_path(authorization("one", WorkdirToolScopePermission::Read))
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn stacked_scope_keeps_resolved_symlink_and_lease_checks() {
    use std::os::unix::fs::symlink;
    let root = TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("base/target")).unwrap();
    fs::create_dir(root.path().join("secret")).unwrap();
    fs::write(root.path().join("secret/key"), "hidden").unwrap();
    symlink("target", root.path().join("base/alias-a")).unwrap();
    symlink("target", root.path().join("base/alias-b")).unwrap();
    symlink("../../secret", root.path().join("base/target/escape")).unwrap();
    let backend = session(root.path());
    let parent = backend
        .scope(filesystem_scope("base", WorkdirToolScopePermission::Write))
        .await
        .unwrap();
    let worker = WorkdirToolBroker::new(parent.tool_session());
    let first = worker
        .scope(filesystem_scope(
            "alias-a",
            WorkdirToolScopePermission::Write,
        ))
        .await
        .unwrap();
    first.write(write("ok", "allowed")).await.unwrap();
    assert!(first.read(read("escape/key")).await.is_err());
    assert!(
        worker
            .scope(filesystem_scope(
                "alias-a/escape",
                WorkdirToolScopePermission::Read
            ))
            .await
            .is_err()
    );
    assert!(
        worker
            .scope(filesystem_scope(
                "alias-b",
                WorkdirToolScopePermission::Write
            ))
            .await
            .is_err()
    );
    assert!(worker.write(write("target/blocked", "no")).await.is_err());
    // Direct authorization through the wrapper must not bypass its existing write lease.
    assert!(
        worker
            .authorize_scope_path(authorization(
                "target/blocked",
                WorkdirToolScopePermission::Write
            ))
            .await
            .is_err()
    );
    first.close().await.unwrap();
    worker
        .write(write("alias-b/released", "yes"))
        .await
        .unwrap();
}
