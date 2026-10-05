#![cfg(target_os = "linux")]
use crate::*;
use std::sync::Arc;
fn path(value: &str) -> WorkdirPath {
    WorkdirPath::new(value).unwrap()
}
fn rule(value: &str, recursive: bool) -> WorkdirToolScopeRule {
    WorkdirToolScopeRule {
        target: path(value),
        recursive,
        permission: WorkdirToolScopePermission::Read,
        symlink_policy: Default::default(),
    }
}
fn scope(rules: Vec<WorkdirToolScopeRule>, cwd: &str) -> WorkdirToolScope {
    WorkdirToolScope {
        rules,
        cwd: path(cwd),
        command: false,
    }
}
fn grep(mode: GrepOutputMode) -> GrepRequest {
    GrepRequest {
        path: WorkdirPath::root(),
        pattern: "needle".into(),
        glob: None,
        file_type: None,
        case_insensitive: false,
        before_context: 0,
        after_context: 0,
        multiline: false,
        output_mode: mode,
        offset: 0,
        limit: 100,
    }
}
fn fixture() -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("child/keep")).unwrap();
    std::fs::create_dir_all(root.path().join("child/denied/deep")).unwrap();
    std::fs::write(root.path().join("child/a.txt"), "needle allowed direct").unwrap();
    std::fs::write(
        root.path().join("child/keep/yes.txt"),
        "needle allowed subtree",
    )
    .unwrap();
    std::fs::write(
        root.path().join("child/denied/deep/secret.txt"),
        "needle secret",
    )
    .unwrap();
    // Opening this out-of-scope source would exhaust the shared grep source bound.
    std::fs::File::create(root.path().join("child/denied/huge.txt"))
        .unwrap()
        .set_len(65 * 1024 * 1024)
        .unwrap();
    std::fs::write(root.path().join("outside.txt"), "needle outside").unwrap();
    root
}
async fn verify_sparse(session: &dyn WorkdirSession) {
    let listed = session
        .list(ListRequest {
            path: WorkdirPath::root(),
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(
        listed
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![path("denied"), path("keep"), path("a.txt")]
    );
    let empty = session
        .list(ListRequest {
            path: path("denied"),
            limit: 100,
        })
        .await
        .unwrap();
    assert!(empty.entries.is_empty());
    assert_eq!(empty.total_entries, 0);
    let globbed = session
        .glob(GlobRequest {
            path: WorkdirPath::root(),
            pattern: "**/*.txt".into(),
            limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(globbed.paths, vec![path("a.txt"), path("keep/yes.txt")]);
    for mode in [
        GrepOutputMode::Content,
        GrepOutputMode::Count,
        GrepOutputMode::FilesWithMatches,
    ] {
        let result = session.grep(grep(mode)).await.unwrap();
        let mut paths = result.paths;
        paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        assert_eq!(paths, vec![path("a.txt"), path("keep/yes.txt")]);
        assert!(!result.output.contains("child/"));
        assert!(!result.output.contains("secret"));
        assert!(!result.output.contains("outside"));
        assert_eq!(result.matched_files, 2);
    }
    let CheckoutSearchResult::List(native) = session
        .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::List(
            ListRequest {
                path: WorkdirPath::root(),
                limit: 100,
            },
        )))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(listed, native);
}
#[tokio::test]
async fn checkout_search_nested_sparse_nonrecursive_scopes_rebase_and_exclude_content() {
    let root = fixture();
    let source = Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    ));
    let parent = WorkdirToolBroker::new(source);
    let first = parent
        .scope(scope(vec![rule("child", true)], "child"))
        .await
        .unwrap();
    let nested = first
        .scope(scope(
            vec![rule("child", false), rule("child/keep", true)],
            "child",
        ))
        .await
        .unwrap();
    verify_sparse(&*nested.tool_session()).await;
    // Nested incoming layers cannot replace the session's own sparse layer.
    let mut request = CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(GlobRequest {
        path: WorkdirPath::root(),
        pattern: "**/*.txt".into(),
        limit: 100,
    }));
    request.scope_layers.push(vec![rule("", true)]);
    let CheckoutSearchResult::Glob(result) = nested
        .tool_session()
        .checkout_search(request)
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(result.paths, vec![path("a.txt"), path("keep/yes.txt")]);
    nested.close().await.unwrap();
    assert!(matches!(
        nested
            .tool_session()
            .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::List(
                ListRequest {
                    path: WorkdirPath::root(),
                    limit: 100
                }
            )))
            .await,
        Err(WorkdirError::SessionClosed)
    ));
}
#[tokio::test]
async fn checkout_search_composes_actual_nested_wrappers_and_output_root() {
    let root = fixture();
    let parent = WorkdirToolBroker::new(Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    )));
    let first = parent
        .scope(scope(vec![rule("child", true)], "child"))
        .await
        .unwrap();
    // Two broker wrappers, not only leases that share a source.
    let stacked = WorkdirToolBroker::new(first.tool_session());
    let mut request =
        CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(grep(GrepOutputMode::Content)));
    *request.operation.path_mut() = path("keep");
    request.output_root = path("keep");
    request.scope_layers = vec![vec![rule("keep", false)]];
    let CheckoutSearchResult::Grep(result) = stacked.checkout_search(request).await.unwrap() else {
        panic!()
    };
    assert_eq!(result.paths, vec![path("yes.txt")]);
    assert!(result.output.starts_with("yes.txt\n"));
    // The outer context is relative to the inner session; no duplicated child/.
    let CheckoutSearchResult::List(result) = stacked
        .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::List(
            ListRequest {
                path: path("keep"),
                limit: 100,
            },
        )))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(result.entries[0].path, path("keep/yes.txt"));
    // A second actual scoped wrapper has its own cwd and nonrecursive layer.
    // Logical rules avoid requesting resolved authorization from a wrapper
    // that conservatively does not expose host-side resolution authority.
    std::fs::create_dir(root.path().join("child/keep/deep")).unwrap();
    std::fs::write(
        root.path().join("child/keep/deep/secret"),
        "needle hidden nested",
    )
    .unwrap();
    let mut own_rule = rule("keep", false);
    own_rule.symlink_policy = manifest::SymlinkPolicy::Logical;
    let second = stacked.scope(scope(vec![own_rule], "keep")).await.unwrap();
    let result = second
        .tool_session()
        .grep(grep(GrepOutputMode::Content))
        .await
        .unwrap();
    assert_eq!(result.paths, vec![path("yes.txt")]);
    assert!(result.output.starts_with("yes.txt\n"));
    assert!(!result.output.contains("hidden nested"));
    let entries = second
        .tool_session()
        .list(ListRequest {
            path: WorkdirPath::root(),
            limit: 100,
        })
        .await
        .unwrap()
        .entries;
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec![path("deep"), path("yes.txt")]
    );
    assert!(
        second
            .tool_session()
            .list(ListRequest {
                path: path("deep"),
                limit: 100
            })
            .await
            .unwrap()
            .entries
            .is_empty()
    );
}
#[tokio::test]
async fn checkout_search_external_keeps_approved_root_and_sparse_authority() {
    let root = fixture();
    let external = LocalWorkdirSession::external_read_only(
        Workdir::new("external-search"),
        root.path(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();
    let broker = WorkdirToolBroker::new(Arc::new(external));
    let lease = broker
        .scope(scope(
            vec![rule("child", false), rule("child/keep", true)],
            "child",
        ))
        .await
        .unwrap();
    let parent = tempfile::tempdir().unwrap();
    let approved = parent.path().join("approved");
    std::fs::rename(root.path(), &approved).unwrap();
    std::fs::create_dir(root.path()).unwrap();
    std::fs::create_dir(root.path().join("child")).unwrap();
    std::fs::write(
        root.path().join("child/replacement.txt"),
        "needle replacement",
    )
    .unwrap();
    verify_sparse(&*lease.tool_session()).await;
}
#[tokio::test]
async fn checkout_search_scope_layers_intersect_before_list_and_grep() {
    let root = fixture();
    let source = LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    );
    let mut request = CheckoutSearchRequest::new(CheckoutSearchOperation::List(ListRequest {
        path: path("child"),
        limit: 100,
    }));
    request.output_root = path("child");
    request.scope_layers = vec![
        vec![rule("child", false), rule("child/keep", true)],
        vec![rule("child", false)],
    ];
    let CheckoutSearchResult::List(result) = source.checkout_search(request.clone()).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(result.entries.len(), 3);
    request.operation = CheckoutSearchOperation::Grep(GrepRequest {
        path: path("child"),
        ..grep(GrepOutputMode::Content)
    });
    let CheckoutSearchResult::Grep(result) = source.checkout_search(request).await.unwrap() else {
        panic!()
    };
    assert_eq!(result.paths, vec![path("a.txt")]);
    assert!(!result.output.contains("subtree"));
}
#[tokio::test]
async fn checkout_search_wire_dto_bounds_and_provider_capabilities() {
    use crate::external::{ExternalWorkdirOperation, ExternalWorkdirOperationResult};
    use crate::http::{WorkdirSessionOperation as Op, WorkdirSessionOperationResult as Output};
    let root = fixture();
    let source = LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    );
    let request = CheckoutSearchRequest::new(CheckoutSearchOperation::List(ListRequest {
        path: path("child/keep"),
        limit: 100,
    }));
    let wire = Op::CheckoutSearch(request.clone());
    assert!(ExternalWorkdirOperation::try_from(wire.clone()).is_ok());
    assert_eq!(
        serde_json::from_value::<Op>(serde_json::to_value(&wire).unwrap()).unwrap(),
        wire
    );
    let result = crate::http::dispatch_workdir_session_operation(&source, wire)
        .await
        .unwrap();
    assert!(matches!(
        &result,
        Output::CheckoutSearch(CheckoutSearchResult::List(_))
    ));
    assert!(ExternalWorkdirOperationResult::try_from(result.clone()).is_ok());
    assert_eq!(
        serde_json::from_value::<Output>(serde_json::to_value(&result).unwrap()).unwrap(),
        result
    );
    let mut bad = request.clone();
    bad.output_root = WorkdirPath::new_scoped("/host").unwrap();
    assert!(ExternalWorkdirOperation::try_from(Op::CheckoutSearch(bad)).is_err());
    let mut bad = request.clone();
    bad.scope_layers = vec![vec![rule("", true)]; 17];
    assert!(ExternalWorkdirOperation::try_from(Op::CheckoutSearch(bad)).is_err());
    let mut bad = request.clone();
    bad.scope_layers = vec![vec![rule("", true); 257]];
    assert!(ExternalWorkdirOperation::try_from(Op::CheckoutSearch(bad)).is_err());
    let mut bad = request.clone();
    bad.scope_layers = vec![vec![rule("", true); 65]];
    assert!(ExternalWorkdirOperation::try_from(Op::CheckoutSearch(bad)).is_err());
    let mut bad = request;
    bad.scope_layers = vec![Vec::new()];
    assert!(ExternalWorkdirOperation::try_from(Op::CheckoutSearch(bad)).is_err());
    let bad_result = Output::CheckoutSearch(CheckoutSearchResult::Glob(GlobResult {
        paths: vec![WorkdirPath::new_scoped("/host").unwrap()],
        truncated: false,
    }));
    assert!(ExternalWorkdirOperationResult::try_from(bad_result).is_err());
}

#[tokio::test]
async fn checkout_search_preserves_current_provider_scope_and_rejects_symlink_escape() {
    let root = fixture();
    let current = manifest::Scope::from_config(&manifest::ScopeConfig {
        allow: vec![manifest::ScopeRule {
            target: root.path().join("child"),
            permission: manifest::Permission::Read,
            recursive: false,
            symlink_policy: Default::default(),
        }],
        deny: Vec::new(),
    })
    .unwrap();
    let source = LocalWorkdirSession::new(current, root.path().to_path_buf());
    let mut request = CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(GrepRequest {
        path: path("child"),
        ..grep(GrepOutputMode::Content)
    }));
    request.output_root = path("child");
    // A broad caller layer cannot enlarge the provider's nonrecursive authority.
    request.scope_layers = vec![vec![rule("", true)]];
    let CheckoutSearchResult::Grep(result) = source.checkout_search(request).await.unwrap() else {
        panic!()
    };
    assert_eq!(result.paths, vec![path("a.txt")]);
    let CheckoutSearchResult::List(result) = source
        .checkout_search(CheckoutSearchRequest {
            operation: CheckoutSearchOperation::List(ListRequest {
                path: path("child/denied"),
                limit: 100,
            }),
            output_root: path("child"),
            scope_layers: Vec::new(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(result.entries.is_empty());
    std::os::unix::fs::symlink("../outside.txt", root.path().join("child/link.txt")).unwrap();
    let mut request = CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(GrepRequest {
        path: path("child/link.txt"),
        ..grep(GrepOutputMode::Content)
    }));
    request.output_root = path("child");
    assert!(source.checkout_search(request).await.is_err());
}
