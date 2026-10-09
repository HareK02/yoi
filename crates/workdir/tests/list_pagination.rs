#![cfg(target_os = "linux")]
use std::sync::Arc;
use workdir::external::{
    ExternalWorkdirOperation, ExternalWorkdirOperationResult, MAX_EXTERNAL_RESULT_ITEMS,
};
use workdir::http::{WorkdirSessionOperation as Op, WorkdirSessionOperationResult as Output};
use workdir::*;

fn path(value: &str) -> WorkdirPath {
    WorkdirPath::new(value).unwrap()
}
fn rule(value: &str, recursive: bool) -> WorkdirToolScopeRule {
    WorkdirToolScopeRule {
        target: path(value),
        recursive,
        permission: WorkdirToolScopePermission::Read,
        symlink_policy: manifest::SymlinkPolicy::Logical,
    }
}
fn scope(rules: Vec<WorkdirToolScopeRule>, cwd: &str) -> WorkdirToolScope {
    WorkdirToolScope {
        rules,
        cwd: path(cwd),
        command: false,
    }
}
fn request(value: &str, limit: usize, after: Option<ListCursor>) -> ListRequest {
    ListRequest {
        path: path(value),
        limit,
        after,
    }
}
fn names(result: &ListResult) -> Vec<&str> {
    result
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect()
}

#[tokio::test]
async fn list_scoped_external_pages_rebase_cursor_and_preserve_sparse_authority() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("child/deep")).unwrap();
    for name in ["child/a", "child/z", "child/deep/secret", "outside"] {
        std::fs::write(root.path().join(name), b"x").unwrap();
    }
    let source = LocalWorkdirSession::external_read_only(
        Workdir::new("pagination"),
        root.path(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();
    let broker = WorkdirToolBroker::new(Arc::new(source));
    let lease = broker
        .scope(scope(vec![rule("child", false)], "child"))
        .await
        .unwrap();
    // Two actual wrappers must not duplicate child/ in the continuation.
    let stacked = WorkdirToolBroker::new(lease.tool_session());
    let first = stacked.list(request("", 1, None)).await.unwrap();
    assert_eq!(names(&first), ["deep"]);
    assert_eq!(first.next_after.as_ref().unwrap().path, path("deep"));
    assert_eq!(
        first.next_after.as_ref().unwrap().kind,
        EntryKind::Directory
    );
    assert_eq!(first.total_entries, 3);
    assert!(
        stacked
            .list(request("deep", 1, None))
            .await
            .unwrap()
            .entries
            .is_empty()
    );
    let second = stacked
        .list(request("", 1, first.next_after))
        .await
        .unwrap();
    assert_eq!(names(&second), ["a"]);
    std::fs::remove_file(root.path().join("child/a")).unwrap();
    std::fs::write(root.path().join("child/b"), b"xx").unwrap();
    let third = stacked
        .list(request("", 10, second.next_after))
        .await
        .unwrap();
    assert_eq!(names(&third), ["b", "z"]);
    assert!(!third.truncated);
    assert!(third.next_after.is_none());
    assert_eq!(third.total_entries, 3);
    lease.close().await.unwrap();
    assert!(matches!(
        stacked.list(request("", 1, None)).await,
        Err(WorkdirError::SessionClosed)
    ));
}

#[tokio::test]
async fn list_checkout_search_cursor_uses_output_root_coordinates_across_nested_wrappers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("child/keep")).unwrap();
    for name in ["a", "b", "c"] {
        std::fs::write(root.path().join("child/keep").join(name), b"x").unwrap();
    }
    let source = LocalWorkdirSession::external_read_only(
        Workdir::new("rebased-pagination"),
        root.path(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();
    let broker = WorkdirToolBroker::new(Arc::new(source));
    let lease = broker
        .scope(scope(vec![rule("child", true)], "child"))
        .await
        .unwrap();
    let stacked = WorkdirToolBroker::new(lease.tool_session());
    let mut search =
        CheckoutSearchRequest::new(CheckoutSearchOperation::List(request("keep", 1, None)));
    search.output_root = path("keep");
    let CheckoutSearchResult::List(first) = stacked.checkout_search(search.clone()).await.unwrap()
    else {
        panic!()
    };
    assert_eq!(names(&first), ["a"]);
    if let CheckoutSearchOperation::List(list) = &mut search.operation {
        list.after = first.next_after;
    }
    let CheckoutSearchResult::List(second) = stacked.checkout_search(search).await.unwrap() else {
        panic!()
    };
    assert_eq!(names(&second), ["b"]);
    assert_eq!(second.next_after.unwrap().path, path("b"));
}

#[tokio::test]
async fn list_read_capability_suffices_without_glob_and_missing_read_is_denied() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a"), b"x").unwrap();
    let source = LocalWorkdirSession::materialized(
        root.path().to_path_buf(),
        root.path().to_path_buf(),
        manifest::SharedScope::new(manifest::Scope::writable(root.path()).unwrap()),
        WorkdirSessionCapabilities::from_capabilities([WorkdirSessionCapability::Read]),
    );
    let broker = WorkdirToolBroker::new(Arc::new(source));
    let lease = broker.scope(scope(vec![rule("", true)], "")).await.unwrap();
    assert_eq!(
        names(
            &lease
                .tool_session()
                .list(request("", 1, None))
                .await
                .unwrap()
        ),
        ["a"]
    );
    let denied = LocalWorkdirSession::external_with_capabilities_pinned(
        Workdir::new("no-read"),
        ExternalWorkdirRoot::pin(root.path()).unwrap(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
        WorkdirSessionCapabilities::COMMAND_ONLY,
    )
    .unwrap();
    assert!(matches!(
        denied.list(request("", 1, None)).await,
        Err(WorkdirError::Unsupported(WorkdirSessionCapability::Read))
    ));
}

#[tokio::test]
async fn list_external_wire_preserves_continuation_and_rejects_unbounded_cursor() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("child")).unwrap();
    for name in ["a", "b"] {
        std::fs::write(root.path().join("child").join(name), b"x").unwrap();
    }
    let source = LocalWorkdirSession::external_read_only(
        Workdir::new("wire-list"),
        root.path(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();
    let first = source.list(request("child", 1, None)).await.unwrap();
    let wire = serde_json::to_value(
        ExternalWorkdirOperationResult::try_from(Output::List(first.clone())).unwrap(),
    )
    .unwrap();
    assert_eq!(wire["result"]["next_after"]["path"], "child/a");
    let decoded: ExternalWorkdirOperationResult = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(decoded.into_inner(), Output::List(first.clone()));
    let operation =
        ExternalWorkdirOperation::try_from(Op::List(request("child", 1, first.next_after)))
            .unwrap();
    let decoded: ExternalWorkdirOperation =
        serde_json::from_value(serde_json::to_value(operation).unwrap()).unwrap();
    let Output::List(next) = dispatch_workdir_session_operation(&source, decoded.into_inner())
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(names(&next), ["child/b"]);
    assert!(next.next_after.is_none());
    for bad_path in ["/host".to_owned(), "x".repeat(4097), "outside/a".to_owned()] {
        let mut bad = serde_json::to_value(Op::List(request("child", 1, None))).unwrap();
        bad["request"]["after"] = serde_json::json!({"kind": "file", "path": bad_path});
        assert!(serde_json::from_value::<ExternalWorkdirOperation>(bad).is_err());
        let mut bad_result = wire.clone();
        bad_result["result"]["next_after"]["path"] = serde_json::json!(bad_path);
        assert!(serde_json::from_value::<ExternalWorkdirOperationResult>(bad_result).is_err());
    }
    assert!(
        ExternalWorkdirOperation::try_from(Op::List(request(
            "child",
            MAX_EXTERNAL_RESULT_ITEMS + 1,
            None
        )))
        .is_err()
    );
    // Search cursors use result coordinates, not provider path coordinates.
    let mut search = CheckoutSearchRequest::new(CheckoutSearchOperation::List(request(
        "child",
        1,
        Some(ListCursor {
            kind: EntryKind::File,
            path: path("a"),
        }),
    )));
    search.output_root = path("child");
    let bounded = ExternalWorkdirOperation::try_from(Op::CheckoutSearch(search)).unwrap();
    let Output::CheckoutSearch(CheckoutSearchResult::List(next)) =
        dispatch_workdir_session_operation(&source, bounded.into_inner())
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(names(&next), ["b"]);
}

#[test]
fn list_external_result_counts_cursor_toward_aggregate_path_bound() {
    let entries: Vec<_> = (0..252)
        .map(|n| ListEntry {
            path: path(&format!("{n:04}{}", "x".repeat(4092))),
            kind: EntryKind::File,
            size: 1,
        })
        .collect();
    let after = ListCursor {
        kind: EntryKind::File,
        path: entries.last().unwrap().path.clone(),
    };
    let result = ListResult {
        entries,
        total_entries: 253,
        total_bytes: 253,
        truncated: true,
        next_after: None,
    };
    let mut wire = serde_json::to_value(Output::List(result)).unwrap();
    assert!(serde_json::from_value::<ExternalWorkdirOperationResult>(wire.clone()).is_ok());
    wire["result"]["next_after"] = serde_json::to_value(after).unwrap();
    assert!(serde_json::from_value::<ExternalWorkdirOperationResult>(wire).is_err());
}
