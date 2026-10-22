//! Normal lib tests connecting live Host-grant checks to real provider traversal
//! and router cwd projection. Only the Host transport is replaced by the parent
//! module's grant fixture; no commands, network or ambient Workspace state are used.

use super::*;

#[tokio::test]
async fn authorization_wrapper_preserves_inner_environment_and_capability_ceiling() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    let provider = Arc::new(LocalWorkdirSession::materialized(
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        manifest::SharedScope::new(manifest::Scope::writable(root.path()).unwrap()),
        WorkdirSessionCapabilities::READ_ONLY,
    ));
    let broker = WorkdirToolBroker::at_cwd(provider, WorkdirPath::new("src").unwrap()).unwrap();
    let inner = broker.tool_session();
    let expected = inner.environment();
    let client = AuthorizationClient::new(vec![permission(READ, "src", true)]);
    let wrapped = authorized_workdir_session(inner, client.clone(), "attachment-id".into());
    assert_eq!(wrapped.environment(), expected);
    assert_eq!(wrapped.environment().cwd.as_str(), "src");
    assert_eq!(
        wrapped.environment().capabilities,
        WorkdirSessionCapabilities::READ_ONLY
    );
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "environment is metadata, not a cached grant decision"
    );
    wrapped.close().await.unwrap();
}

#[tokio::test]
async fn nonroot_routed_search_retains_host_grants_output_coordinates_and_cursors() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::create_dir(root.path().join("private")).unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.path().join("src").join(name), "needle public marker").unwrap();
    }
    std::fs::write(
        root.path().join("private/secret.txt"),
        "needle private marker",
    )
    .unwrap();
    let client = AuthorizationClient::new(vec![permission(READ, "src", true)]);
    let authorized = session(&root, client.clone());
    let router = workdir::WorkdirSessionRouter::new();
    let alias = workdir::WorkdirAttachmentAlias::new("checkout").unwrap();
    router
        .attach_at_cwd(alias.clone(), authorized, WorkdirPath::new("src").unwrap())
        .unwrap();
    let routed = router.resolve(Some("checkout")).unwrap().session;
    assert_eq!(routed.environment().cwd.as_str(), "src");
    let paths = routed
        .glob(GlobRequest {
            path: WorkdirPath::root(),
            pattern: "**/*.txt".into(),
            limit: 100,
        })
        .await
        .unwrap()
        .paths;
    assert_eq!(
        paths,
        vec![
            WorkdirPath::new("a.txt").unwrap(),
            WorkdirPath::new("b.txt").unwrap()
        ]
    );
    // Search output is reusable as a path without adding src/ at the caller.
    assert_eq!(
        routed.read(read(paths[0].as_str())).await.unwrap().bytes,
        b"needle public marker"
    );
    for output_mode in [
        workdir::GrepOutputMode::Content,
        workdir::GrepOutputMode::FilesWithMatches,
        workdir::GrepOutputMode::Count,
    ] {
        let result = routed
            .grep(GrepRequest {
                path: WorkdirPath::root(),
                pattern: "needle".into(),
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode,
                limit: 100,
                offset: 0,
            })
            .await
            .unwrap();
        let mut grep_paths = result.paths;
        grep_paths.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        assert_eq!(grep_paths, paths);
        assert!(result.output.contains("a.txt"));
        assert!(!result.output.contains("src/"));
        assert!(!result.output.contains("secret"));
    }
    let first = routed
        .list(ListRequest {
            path: WorkdirPath::root(),
            after: None,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(first.entries[0].path.as_str(), "a.txt");
    assert_eq!(first.next_after.as_ref().unwrap().path.as_str(), "a.txt");
    let second = routed
        .list(ListRequest {
            path: WorkdirPath::root(),
            after: first.next_after,
            limit: 1,
        })
        .await
        .unwrap();
    assert_eq!(second.entries[0].path.as_str(), "b.txt");
    assert!(second.next_after.is_none());

    // An existing caller ceiling must survive the extra Host layer. An empty
    // layer is invalid and cannot silently turn into the broader Host grant.
    let mut denied = CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(GlobRequest {
        path: WorkdirPath::root(),
        pattern: "**/*.txt".into(),
        limit: 100,
    }));
    denied.scope_layers.push(Vec::new());
    assert!(routed.checkout_search(denied).await.is_err());
    assert!(client.requests.lock().unwrap().iter().all(|request| {
        matches!(&request.resource, PermissionResource::Workdir { path, .. } if path == "src" || path == "src/a.txt")
    }));
    client.revoked.store(true, Ordering::SeqCst);
    assert!(
        routed
            .glob(GlobRequest {
                path: WorkdirPath::root(),
                pattern: "**/*.txt".into(),
                limit: 100,
            })
            .await
            .is_err()
    );
    router.detach(&alias).await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn scoped_checkout_search_preserves_caller_layer_and_rejects_resolved_escape() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
    std::fs::create_dir(root.path().join("private")).unwrap();
    std::fs::write(root.path().join("src/public.txt"), "needle public").unwrap();
    std::fs::write(root.path().join("src/nested/hidden.txt"), "needle hidden").unwrap();
    std::fs::write(root.path().join("private/secret.txt"), "needle secret").unwrap();
    std::os::unix::fs::symlink("../private", root.path().join("src/escape")).unwrap();
    std::os::unix::fs::symlink("../private/secret.txt", root.path().join("src/alias.txt")).unwrap();
    let client = AuthorizationClient::new(vec![permission(READ, "src", true)]);
    let authorized = session(&root, client);
    let request = CheckoutSearchRequest {
        operation: CheckoutSearchOperation::Glob(GlobRequest {
            path: WorkdirPath::new("src").unwrap(),
            pattern: "**/*.txt".into(),
            limit: 100,
        }),
        output_root: WorkdirPath::new("src").unwrap(),
        scope_layers: vec![vec![WorkdirToolScopeRule {
            target: WorkdirPath::new("src").unwrap(),
            recursive: false,
            permission: WorkdirToolScopePermission::Read,
            symlink_policy: manifest::SymlinkPolicy::Resolved,
        }]],
    };
    let result = authorized.checkout_search(request).await.unwrap();
    let CheckoutSearchResult::Glob(result) = result else {
        panic!("expected Glob result")
    };
    assert_eq!(result.paths, vec![WorkdirPath::new("public.txt").unwrap()]);
    assert!(
        authorized
            .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(
                GlobRequest {
                    path: WorkdirPath::new("private").unwrap(),
                    pattern: "**/*.txt".into(),
                    limit: 100,
                }
            )))
            .await
            .is_err()
    );
    authorized.close().await.unwrap();
}
