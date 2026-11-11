//! Parent broker + live authorization adapter + real Local provider regression.
//! The Host fixture returns only the requested resource, as current core grants do.
//! No model, production Workspace or service restart is involved.
use super::*;

fn client() -> Arc<AuthorizationClient> {
    Arc::new(AuthorizationClient {
        legacy: false,
        requested_scope_only: true,
        revoked: AtomicBool::new(false),
        requests: Mutex::new(Vec::new()),
        permissions: vec![
            permission(READ, ".", true),
            permission(WRITE, "src", true),
            permission(COMMAND, ".", true),
        ],
    })
}

fn rule(
    path: &str,
    permission: WorkdirToolScopePermission,
    recursive: bool,
) -> WorkdirToolScopeRule {
    WorkdirToolScopeRule {
        target: WorkdirPath::new(path).unwrap(),
        permission,
        recursive,
        symlink_policy: manifest::SymlinkPolicy::Resolved,
    }
}

fn scope(rules: Vec<WorkdirToolScopeRule>, cwd: &str, command: bool) -> WorkdirToolScope {
    WorkdirToolScope {
        rules,
        cwd: WorkdirPath::new(cwd).unwrap(),
        command,
    }
}

fn write(path: &str, content: &str) -> WriteRequest {
    WriteRequest {
        path: WorkdirPath::new(path).unwrap(),
        content: content.as_bytes().to_vec(),
        expected_hash: None,
    }
}

#[tokio::test]
async fn recursive_mixed_scope_creation_never_requests_a_host_grant_and_effects_stay_live() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
    std::fs::write(root.path().join("src/note"), "original").unwrap();
    let client = client();
    let broker = WorkdirToolBroker::new(session(&root, client.clone()));
    let parent = broker.tool_session();
    let request = scope(
        vec![
            rule(".", WorkdirToolScopePermission::Read, true),
            rule("src", WorkdirToolScopePermission::Write, true),
        ],
        "src",
        true,
    );
    let lease = broker.scope(request.clone()).await.unwrap();
    assert!(
        client.requests.lock().unwrap().is_empty(),
        "scope creation is not a Host grant"
    );
    let child = lease.tool_session();
    let original = child.read(read("note")).await.unwrap();
    assert_eq!(original.bytes, b"original");
    child
        .edit(EditRequest {
            path: WorkdirPath::new("note").unwrap(),
            expected_hash: original.content_hash,
            old_string: "original".into(),
            new_string: "edited".into(),
            replace_all: false,
        })
        .await
        .unwrap();
    child.write(write("nested/new", "new")).await.unwrap();
    let paths = child
        .glob(GlobRequest {
            path: WorkdirPath::root(),
            pattern: "**/*".into(),
            limit: 100,
        })
        .await
        .unwrap()
        .paths;
    assert!(paths.contains(&WorkdirPath::new("nested/new").unwrap()));
    let grep = child
        .grep(GrepRequest {
            path: WorkdirPath::root(),
            pattern: "edited".into(),
            glob: None,
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: workdir::GrepOutputMode::Content,
            limit: 100,
            offset: 0,
        })
        .await
        .unwrap();
    assert!(grep.output.contains("edited"));

    // Structural overlap checks must work with request-only Host scopes too.
    assert!(broker.scope(request).await.is_err());
    assert!(parent.write(write("src/parent", "denied")).await.is_err());
    assert!(!root.path().join("src/parent").exists());
    // A second attachment identity does not inherit this one's live grant.
    let wrong = authorized_workdir_session(
        Arc::new(LocalWorkdirSession::new(
            manifest::Scope::writable(root.path()).unwrap(),
            root.path().into(),
        )),
        client.clone(),
        "another-attachment".into(),
    );
    assert!(wrong.read(read("src/note")).await.is_err());
    wrong.close().await.unwrap();

    client.revoked.store(true, Ordering::SeqCst);
    assert!(child.read(read("note")).await.is_err());
    assert!(child.write(write("revoked", "denied")).await.is_err());
    assert!(
        child
            .start_command(command("printf denied > revoked-command"))
            .await
            .is_err()
    );
    assert!(!root.path().join("src/revoked").exists());
    assert!(!root.path().join("src/revoked-command").exists());
    lease.close().await.unwrap();
    client.revoked.store(false, Ordering::SeqCst);
    assert!(child.write(write("late", "denied")).await.is_err());
    parent.write(write("src/reclaimed", "ok")).await.unwrap();
    parent.close().await.unwrap();
}

#[tokio::test]
async fn nested_scope_cannot_expand_permissions_recursion_or_command() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
    std::fs::write(root.path().join("src/note"), "readable").unwrap();
    let client = client();
    let broker = WorkdirToolBroker::new(session(&root, client.clone()));
    let lease = broker
        .scope(scope(
            vec![rule("src", WorkdirToolScopePermission::Read, false)],
            "src",
            false,
        ))
        .await
        .unwrap();
    for request in [
        scope(
            vec![rule("src", WorkdirToolScopePermission::Read, true)],
            "src",
            false,
        ),
        scope(
            vec![rule("src", WorkdirToolScopePermission::Write, false)],
            "src",
            false,
        ),
        scope(
            vec![rule(".", WorkdirToolScopePermission::Read, true)],
            ".",
            false,
        ),
        scope(
            vec![rule("src", WorkdirToolScopePermission::Read, false)],
            "src",
            true,
        ),
    ] {
        assert!(lease.broker().scope(request).await.is_err());
    }
    assert!(client.requests.lock().unwrap().is_empty());
    let child = lease.tool_session();
    assert_eq!(child.read(read("note")).await.unwrap().bytes, b"readable");
    assert!(child.read(read("nested/file")).await.is_err());
    assert!(child.write(write("note", "denied")).await.is_err());
    assert!(child.start_command(command("true")).await.is_err());
    lease.close().await.unwrap();
    broker.tool_session().close().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn structural_queries_retain_provider_symlink_checks_and_write_lease_alias_conflicts() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::create_dir(root.path().join("private")).unwrap();
    std::fs::write(root.path().join("private/secret"), "secret").unwrap();
    std::os::unix::fs::symlink("../private", root.path().join("src/escape")).unwrap();
    std::os::unix::fs::symlink("src", root.path().join("alias")).unwrap();
    let client = client();
    let broker = WorkdirToolBroker::new(session(&root, client.clone()));
    let lease = broker
        .scope(scope(
            vec![rule("src", WorkdirToolScopePermission::Write, true)],
            "src",
            false,
        ))
        .await
        .unwrap();
    assert!(
        broker
            .scope(scope(
                vec![rule("alias", WorkdirToolScopePermission::Write, true)],
                "alias",
                false
            ))
            .await
            .is_err()
    );
    assert!(client.requests.lock().unwrap().is_empty());
    assert!(
        lease
            .tool_session()
            .read(read("escape/secret"))
            .await
            .is_err()
    );
    assert!(
        lease
            .tool_session()
            .write(write("escape/new", "denied"))
            .await
            .is_err()
    );
    assert!(!root.path().join("private/new").exists());
    lease.close().await.unwrap();
    broker.tool_session().close().await.unwrap();
}

#[tokio::test]
async fn child_commands_keep_origin_session_and_cleanup_does_not_cancel_parent() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    let client = client();
    let broker = WorkdirToolBroker::new(session(&root, client.clone()));
    let parent = broker.tool_session();
    let lease = broker
        .scope(scope(
            vec![rule("src", WorkdirToolScopePermission::Write, true)],
            "src",
            true,
        ))
        .await
        .unwrap();
    let child = lease.tool_session();
    let parent_handle = parent
        .start_command(command("exec tail -f /dev/null"))
        .await
        .unwrap();
    let child_handle = child
        .start_command(command("printf child-output"))
        .await
        .unwrap();
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut cursor = 0;
        let mut bytes = Vec::new();
        loop {
            let output = child
                .command_output(CommandOutputRequest {
                    handle: child_handle.clone(),
                    cursor,
                    limit: 4096,
                    wait: true,
                })
                .await
                .unwrap();
            cursor = output.next_cursor;
            for chunk in output.chunks {
                bytes.extend(chunk.bytes);
            }
            if output.terminal.is_some() {
                break bytes;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(bytes, b"child-output");
    assert!(child.cancel_command(parent_handle.clone()).await.is_err());
    let running = child
        .start_command(command("exec tail -f /dev/null"))
        .await
        .unwrap();
    client.revoked.store(true, Ordering::SeqCst);
    assert!(
        child
            .command_output(CommandOutputRequest {
                handle: running.clone(),
                cursor: 0,
                limit: 4096,
                wait: false,
            })
            .await
            .is_err()
    );
    lease.close().await.unwrap();
    assert_ne!(
        parent.command_status(running).await.unwrap(),
        CommandStatus::Running
    );
    assert_eq!(
        parent.command_status(parent_handle.clone()).await.unwrap(),
        CommandStatus::Running
    );
    parent.cancel_command(parent_handle).await.unwrap();
    parent.close().await.unwrap();
}
