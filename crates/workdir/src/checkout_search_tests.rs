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
            after: None,
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
            after: None,
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
                after: None,
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
                    after: None,
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
                after: None,
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
    // Default resolved authorization must survive every wrapper.
    std::fs::create_dir(root.path().join("child/keep/deep")).unwrap();
    std::fs::write(
        root.path().join("child/keep/deep/secret"),
        "needle hidden nested",
    )
    .unwrap();
    let second = stacked
        .scope(scope(vec![rule("keep", false)], "keep"))
        .await
        .unwrap();
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
            after: None,
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
                after: None,
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
        after: None,
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
        after: None,
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

fn write_ignore_fixture(root: &std::path::Path, relative: &str, contents: &str) {
    let file = root.join(relative);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, contents).unwrap();
}

async fn verify_ignore_search_parity(session: &dyn WorkdirSession, base: &str, expected: &[&str]) {
    let request = GlobRequest {
        path: path(base),
        pattern: "**/*.txt".into(),
        limit: 100,
    };
    // On a Local session this is the actual ordinary WalkBuilder-backed path,
    // not a broker wrapper that would route both calls through checkout_search.
    let ordinary = session.glob(request.clone()).await.unwrap();
    assert_eq!(
        ordinary.paths,
        expected.iter().map(|value| path(value)).collect::<Vec<_>>(),
        "ordinary Glob fixture semantics at {base:?}"
    );
    assert!(!ordinary.truncated);
    let CheckoutSearchResult::Glob(native) = session
        .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(
            request,
        )))
        .await
        .unwrap()
    else {
        panic!("expected native Glob result")
    };
    assert_eq!(native, ordinary, "native Glob parity at {base:?}");

    // Content mode renders paths in sorted order, avoiding dependence on the
    // different walkers' visitation order. All fixture content has one match.
    let request = GrepRequest {
        path: path(base),
        ..grep(GrepOutputMode::Content)
    };
    let ordinary = session.grep(request.clone()).await.unwrap();
    assert_eq!(
        ordinary.paths,
        expected.iter().map(|value| path(value)).collect::<Vec<_>>(),
        "ordinary Grep fixture semantics at {base:?}"
    );
    assert_eq!(ordinary.matched_files, expected.len());
    assert_eq!(ordinary.match_count, expected.len());
    assert!(!ordinary.truncated);
    let CheckoutSearchResult::Grep(native) = session
        .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(
            request,
        )))
        .await
        .unwrap()
    else {
        panic!("expected native Grep result")
    };
    assert_eq!(native, ordinary, "native Grep parity at {base:?}");
}

#[tokio::test]
async fn checkout_search_local_glob_inherits_root_and_nested_ignore_rules() {
    // Exercise the two ignore sources independently so rule precedence cannot
    // conceal either missing source. A .git marker activates .gitignore rules.
    for ignore_name in [".ignore", ".gitignore"] {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join(".git")).unwrap();
        write_ignore_fixture(
            root.path(),
            ignore_name,
            "root-only.txt\n/root-anchored.txt\n*.drop.txt\n!keep.drop.txt\ninherited.txt\npruned/\n",
        );
        write_ignore_fixture(
            root.path(),
            &format!("nested/{ignore_name}"),
            "/nested-anchored.txt\nnested-only.txt\n!inherited.txt\nignored-dir/\n!ignored-dir/rescue.txt\n",
        );
        for relative in [
            "visible.txt",
            "root-only.txt",
            "root-anchored.txt",
            "discard.drop.txt",
            "keep.drop.txt",
            "inherited.txt",
            "pruned/rescue.txt",
            "nested/visible.txt",
            "nested/root-only.txt",
            "nested/root-anchored.txt",
            "nested/nested-only.txt",
            "nested/nested-anchored.txt",
            "nested/discard.drop.txt",
            "nested/keep.drop.txt",
            "nested/inherited.txt",
            "nested/pruned/rescue.txt",
            "nested/ignored-dir/rescue.txt",
            "nested/deep/nested-anchored.txt",
        ] {
            write_ignore_fixture(root.path(), relative, "needle fixture\n");
        }
        let source = LocalWorkdirSession::new(
            manifest::Scope::writable(root.path()).unwrap(),
            root.path().to_path_buf(),
        );
        verify_ignore_search_parity(
            &source,
            "",
            &[
                "keep.drop.txt",
                "nested/deep/nested-anchored.txt",
                "nested/inherited.txt",
                "nested/keep.drop.txt",
                "nested/root-anchored.txt",
                "nested/visible.txt",
                "visible.txt",
            ],
        )
        .await;
        // Starting explicitly below the provider root must still inherit the
        // root's rules and retain provider-relative (not base-relative) paths.
        verify_ignore_search_parity(
            &source,
            "nested",
            &[
                "nested/deep/nested-anchored.txt",
                "nested/inherited.txt",
                "nested/keep.drop.txt",
                "nested/root-anchored.txt",
                "nested/visible.txt",
            ],
        )
        .await;
        verify_ignore_search_parity(&source, "nested/deep", &["nested/deep/nested-anchored.txt"])
            .await;
    }
}

#[tokio::test]
async fn checkout_search_local_glob_preserves_ignore_source_precedence_across_levels() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    write_ignore_fixture(
        root.path(),
        ".ignore",
        "priority.txt\n!ignore-wins.txt\nroot-ignore.txt\n",
    );
    write_ignore_fixture(
        root.path(),
        ".gitignore",
        "ignore-wins.txt\nrevived-git.txt\n",
    );
    write_ignore_fixture(root.path(), "nested/.ignore", "!root-ignore.txt\n");
    write_ignore_fixture(
        root.path(),
        "nested/.gitignore",
        "!priority.txt\n!revived-git.txt\n",
    );
    for relative in [
        "priority.txt",
        "ignore-wins.txt",
        "root-ignore.txt",
        "revived-git.txt",
        "nested/priority.txt",
        "nested/ignore-wins.txt",
        "nested/root-ignore.txt",
        "nested/revived-git.txt",
    ] {
        write_ignore_fixture(root.path(), relative, "needle fixture\n");
    }
    let source = LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    );
    // .ignore outranks .gitignore even when the .gitignore is deeper. Within
    // the same source kind, a deeper negation can override an ancestor rule.
    verify_ignore_search_parity(
        &source,
        "",
        &[
            "ignore-wins.txt",
            "nested/ignore-wins.txt",
            "nested/revived-git.txt",
            "nested/root-ignore.txt",
        ],
    )
    .await;
    verify_ignore_search_parity(
        &source,
        "nested",
        &[
            "nested/ignore-wins.txt",
            "nested/revived-git.txt",
            "nested/root-ignore.txt",
        ],
    )
    .await;
}

#[tokio::test]
async fn checkout_search_scoped_broker_skips_denied_ancestor_ignore_files() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    for (relative, contents) in [
        (".ignore", "root-ignore-visible.txt\n"),
        (".gitignore", "root-git-visible.txt\n"),
        ("parent/.ignore", "parent-ignore-visible.txt\n"),
        ("parent/.gitignore", "parent-git-visible.txt\n"),
        ("parent/nested/.ignore", "allowed-ignore.txt\n"),
    ] {
        write_ignore_fixture(root.path(), relative, contents);
    }
    for name in [
        "root-ignore-visible.txt",
        "root-git-visible.txt",
        "parent-ignore-visible.txt",
        "parent-git-visible.txt",
        "allowed-ignore.txt",
    ] {
        write_ignore_fixture(
            root.path(),
            &format!("parent/nested/{name}"),
            "needle fixture\n",
        );
    }
    let source = Arc::new(LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    ));
    // Without a scope layer, ordinary Local traversal really does inherit
    // these ancestor rules. Each denied source suppresses a different file.
    assert!(
        source
            .glob(GlobRequest {
                path: path("parent/nested"),
                pattern: "**/*.txt".into(),
                limit: 100,
            })
            .await
            .unwrap()
            .paths
            .is_empty()
    );
    // Make accidental consumption observable even if a matcher were to drop
    // the valid patterns: these denied sparse sources exceed the shared Grep
    // source budget. The allowed subtree remains tiny.
    for relative in [
        ".ignore",
        ".gitignore",
        "parent/.ignore",
        "parent/.gitignore",
    ] {
        std::fs::OpenOptions::new()
            .write(true)
            .open(root.path().join(relative))
            .unwrap()
            .set_len(65 * 1024 * 1024)
            .unwrap();
    }
    let broker = WorkdirToolBroker::new(source.clone());
    let lease = broker
        .scope(scope(vec![rule("parent/nested", true)], "parent/nested"))
        .await
        .unwrap();
    // Broker .glob/.grep use the native path under a scope. Compare those
    // public entry points with explicit checkout_search and assert rebasing.
    verify_ignore_search_parity(
        &*lease.tool_session(),
        "",
        &[
            "parent-git-visible.txt",
            "parent-ignore-visible.txt",
            "root-git-visible.txt",
            "root-ignore-visible.txt",
        ],
    )
    .await;
    // Also exercise a Local native request with an unre-based provider root:
    // allowed descendants do not grant authority to ancestor ignore sources.
    let mut request = CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(GlobRequest {
        path: path("parent/nested"),
        pattern: "**/*.txt".into(),
        limit: 100,
    }));
    request.scope_layers = vec![vec![rule("parent/nested", true)]];
    let CheckoutSearchResult::Glob(result) = source.checkout_search(request).await.unwrap() else {
        panic!("expected native Glob result")
    };
    assert_eq!(
        result.paths,
        vec![
            path("parent/nested/parent-git-visible.txt"),
            path("parent/nested/parent-ignore-visible.txt"),
            path("parent/nested/root-git-visible.txt"),
            path("parent/nested/root-ignore-visible.txt"),
        ]
    );
    assert!(!result.truncated);
    lease.close().await.unwrap();
}

#[tokio::test]
async fn checkout_search_native_skips_symlinked_ignore_sources_without_outside_effects() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::create_dir(root.path().join("nested")).unwrap();
    // Following any of these sources would hide an otherwise visible file.
    // The native descriptor provider must safely skip rejected ignore opens,
    // including when the source belongs to an explicitly selected base.
    for (relative, hidden) in [
        (".ignore", "root-ignore-visible.txt"),
        (".gitignore", "root-git-visible.txt"),
        ("nested/.ignore", "nested-ignore-visible.txt"),
        ("nested/.gitignore", "nested-git-visible.txt"),
    ] {
        let target = outside.path().join(relative.replace('/', "-"));
        let contents = format!("{hidden}\n");
        std::fs::write(&target, &contents).unwrap();
        std::os::unix::fs::symlink(&target, root.path().join(relative)).unwrap();
        let visible = if relative.starts_with("nested/") {
            format!("nested/{hidden}")
        } else {
            hidden.to_string()
        };
        write_ignore_fixture(root.path(), &visible, "needle fixture\n");
    }
    write_ignore_fixture(outside.path(), "secret.txt", "needle outside\n");
    let source = LocalWorkdirSession::new(
        manifest::Scope::writable(root.path()).unwrap(),
        root.path().to_path_buf(),
    );
    for (base, expected) in [
        (
            "",
            vec![
                path("nested/nested-git-visible.txt"),
                path("nested/nested-ignore-visible.txt"),
                path("root-git-visible.txt"),
                path("root-ignore-visible.txt"),
            ],
        ),
        (
            "nested",
            vec![
                path("nested/nested-git-visible.txt"),
                path("nested/nested-ignore-visible.txt"),
            ],
        ),
    ] {
        let CheckoutSearchResult::Glob(result) = source
            .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Glob(
                GlobRequest {
                    path: path(base),
                    pattern: "**/*.txt".into(),
                    limit: 100,
                },
            )))
            .await
            .unwrap()
        else {
            panic!("expected native Glob result")
        };
        assert_eq!(result.paths, expected);
        assert!(!result.truncated);
        // Grep's existing descriptor contract rejects an unauthorized ignore
        // open, rather than ignoring its error as Glob does. Both behaviors
        // preserve confinement; do not relax Grep merely to match Glob.
        let error = source
            .checkout_search(CheckoutSearchRequest::new(CheckoutSearchOperation::Grep(
                GrepRequest {
                    path: path(base),
                    ..grep(GrepOutputMode::Content)
                },
            )))
            .await
            .unwrap_err();
        let classification = match &error {
            WorkdirError::DenialContext { source, .. } => source.as_ref(),
            error => error,
        };
        assert!(matches!(classification, WorkdirError::OutOfScope(_)));
        assert_ne!(
            error.denial_reason(),
            Some(crate::WorkdirDenialReason::OsPermissionDenied)
        );
    }
    for (relative, hidden) in [
        (".ignore", "root-ignore-visible.txt"),
        (".gitignore", "root-git-visible.txt"),
        ("nested/.ignore", "nested-ignore-visible.txt"),
        ("nested/.gitignore", "nested-git-visible.txt"),
    ] {
        assert!(
            std::fs::symlink_metadata(root.path().join(relative))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(outside.path().join(relative.replace('/', "-"))).unwrap(),
            format!("{hidden}\n")
        );
    }
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
        "needle outside\n"
    );
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
                after: None,
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
