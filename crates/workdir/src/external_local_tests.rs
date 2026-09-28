use crate::http::{
    WorkdirSessionOperation, WorkdirSessionOperationResult, dispatch_workdir_session_operation,
};
use crate::{
    BoundedReadLimits, CommandRequest, EditRequest, ExternalWorkdirRoot, GlobRequest,
    GrepOutputMode, GrepRequest, LocalWorkdirSession, ReadRequest, Workdir, WorkdirError,
    WorkdirPath, WorkdirSession, WorkdirSessionCapabilities, WorkdirSessionCapability,
    WriteRequest,
};

fn external_session(root: &tempfile::TempDir, limits: BoundedReadLimits) -> LocalWorkdirSession {
    LocalWorkdirSession::external_read_only(Workdir::new("external-workdir-1"), root.path(), limits)
        .unwrap()
}

#[tokio::test]
async fn external_local_provider_is_strictly_read_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("item.txt"), "original").unwrap();
    let session = external_session(&root, BoundedReadLimits::new(4096, 1024).unwrap());

    assert_eq!(
        session.capabilities(),
        WorkdirSessionCapabilities::READ_ONLY
    );

    let write = WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            content: b"changed".to_vec(),
            expected_hash: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        write,
        WorkdirError::Unsupported(WorkdirSessionCapability::Write)
    ));

    let edit = WorkdirSession::edit(
        &session,
        EditRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            old_string: "original".to_string(),
            new_string: "changed".to_string(),
            replace_all: false,
            expected_hash: [0; 32],
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        edit,
        WorkdirError::Unsupported(WorkdirSessionCapability::Edit)
    ));

    let command = WorkdirSession::start_command(
        &session,
        CommandRequest {
            command: "echo changed > item.txt".to_string(),
            timeout_secs: 1,
            output_limit: 1024,
            cwd: WorkdirPath::root(),
            spill_dir: None,
            tool_call_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        command,
        WorkdirError::Unsupported(WorkdirSessionCapability::Command)
    ));
    assert_eq!(
        std::fs::read_to_string(root.path().join("item.txt")).unwrap(),
        "original"
    );
}

#[tokio::test]
async fn external_local_provider_read_write_is_filesystem_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("item.txt"), "original").unwrap();
    let session = LocalWorkdirSession::external_read_write(
        Workdir::new("external-workdir-rw"),
        root.path(),
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap();

    assert_eq!(
        session.capabilities(),
        WorkdirSessionCapabilities::READ_WRITE
    );
    assert!(
        !session
            .capabilities()
            .supports(WorkdirSessionCapability::Command)
    );

    let observed = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap();
    WorkdirSession::edit(
        &session,
        EditRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            old_string: "original".to_string(),
            new_string: "changed".to_string(),
            replace_all: false,
            expected_hash: observed.content_hash,
        },
    )
    .await
    .unwrap();
    WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("created.txt").unwrap(),
            content: b"created".to_vec(),
            expected_hash: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.path().join("item.txt")).unwrap(),
        "changed"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("created.txt")).unwrap(),
        "created"
    );
    let command = WorkdirSession::start_command(
        &session,
        CommandRequest {
            command: "touch command-ran".to_string(),
            timeout_secs: 1,
            output_limit: 1024,
            cwd: WorkdirPath::root(),
            spill_dir: None,
            tool_call_id: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        command,
        WorkdirError::Unsupported(WorkdirSessionCapability::Command)
    ));
    assert!(!root.path().join("command-ran").exists());
}

#[tokio::test]
async fn common_dispatcher_preserves_operation_result_pairing_and_capabilities() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("item.txt"), "visible").unwrap();
    let session = external_session(&root, BoundedReadLimits::new(4096, 1024).unwrap());

    let result = dispatch_workdir_session_operation(
        &session,
        WorkdirSessionOperation::Read(ReadRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        WorkdirSessionOperationResult::Read(ref result) if result.bytes == b"visible"
    ));

    let error = dispatch_workdir_session_operation(
        &session,
        WorkdirSessionOperation::Write(WriteRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            content: b"blocked".to_vec(),
            expected_hash: None,
        }),
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        WorkdirError::Unsupported(WorkdirSessionCapability::Write)
    ));
}

#[tokio::test]
async fn external_local_provider_rejects_absolute_and_parent_paths() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    let session = external_session(&root, BoundedReadLimits::new(4096, 1024).unwrap());

    assert!(WorkdirPath::new("../outside.txt").is_err());
    let absolute = WorkdirPath::new_scoped(outside.path().to_string_lossy()).unwrap();
    let error = WorkdirSession::read(
        &session,
        ReadRequest {
            path: absolute,
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WorkdirError::InvalidPath(_)));
}

#[tokio::test]
async fn external_local_provider_enforces_source_and_response_bounds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("large.log"), vec![b'x'; 2048]).unwrap();
    std::fs::write(root.path().join("small.log"), b"0123456789\n").unwrap();
    let session = external_session(&root, BoundedReadLimits::new(1024, 4).unwrap());

    let error = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("large.log").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: usize::MAX,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, WorkdirError::InvalidArgument(_)));

    let bounded = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("small.log").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: usize::MAX,
        },
    )
    .await
    .unwrap();
    assert_eq!(bounded.bytes, b"0123");
    assert!(bounded.truncated);
}

#[cfg(unix)]
#[tokio::test]
async fn external_local_provider_rejects_symlink_roots_and_traversal() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    symlink(
        outside.path().join("secret.txt"),
        root.path().join("outside.txt"),
    )
    .unwrap();
    std::fs::write(root.path().join("inside.txt"), "inside").unwrap();
    symlink(
        root.path().join("inside.txt"),
        root.path().join("inside-link.txt"),
    )
    .unwrap();
    let session = external_session(&root, BoundedReadLimits::new(4096, 1024).unwrap());

    for path in ["outside.txt", "inside-link.txt"] {
        let error = WorkdirSession::read(
            &session,
            ReadRequest {
                path: WorkdirPath::new(path).unwrap(),
                offset: 0,
                limit: 10,
                max_bytes: 1024,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(
                error,
                WorkdirError::SymlinkOutOfScope { .. } | WorkdirError::OutOfScope(_)
            ),
            "unexpected error for {path}: {error:?}"
        );
    }

    let parent = tempfile::tempdir().unwrap();
    let root_link = parent.path().join("shared-root");
    symlink(root.path(), &root_link).unwrap();
    let error = LocalWorkdirSession::external_read_only(
        Workdir::new("external-workdir-2"),
        &root_link,
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(error, WorkdirError::Denied(_)));
}

#[cfg(unix)]
#[tokio::test]
async fn external_read_write_provider_rejects_symlink_mutation_escape() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
    symlink(
        outside.path().join("secret.txt"),
        root.path().join("outside.txt"),
    )
    .unwrap();
    let session = LocalWorkdirSession::external_read_write(
        Workdir::new("external-workdir-rw-symlink"),
        root.path(),
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap();

    let error = WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("outside.txt").unwrap(),
            content: b"escaped".to_vec(),
            expected_hash: None,
        },
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            error,
            WorkdirError::SymlinkOutOfScope { .. }
                | WorkdirError::OutOfScope(_)
                | WorkdirError::Denied(_)
        ),
        "unexpected error: {error:?}"
    );
    assert_eq!(
        std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(),
        "secret"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_grep_honors_gitignore_above_a_nested_search_root() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("nested")).unwrap();
    std::fs::write(root.path().join(".gitignore"), "nested/ignored.txt\n").unwrap();
    std::fs::write(root.path().join("nested/ignored.txt"), "needle ignored\n").unwrap();
    std::fs::write(root.path().join("nested/visible.txt"), "needle visible\n").unwrap();
    let session = external_session(&root, BoundedReadLimits::new(4096, 1024).unwrap());

    let result = WorkdirSession::grep(
        &session,
        GrepRequest {
            pattern: "needle".to_string(),
            path: WorkdirPath::new("nested").unwrap(),
            glob: Some("*.txt".to_string()),
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();

    assert!(result.output.contains("nested/visible.txt"));
    assert!(!result.output.contains("nested/ignored.txt"));

    std::fs::write(root.path().join(".gitignore"), "nested/\n").unwrap();
    let explicit_ignored_root = WorkdirSession::grep(
        &session,
        GrepRequest {
            pattern: "needle".to_string(),
            path: WorkdirPath::new("nested").unwrap(),
            glob: Some("*.txt".to_string()),
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert!(explicit_ignored_root.output.contains("nested/visible.txt"));
    assert!(explicit_ignored_root.output.contains("nested/ignored.txt"));

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("ignore-rules"), "nested/visible.txt\n").unwrap();
    std::fs::remove_file(root.path().join(".gitignore")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("ignore-rules"),
        root.path().join(".gitignore"),
    )
    .unwrap();
    let glob_with_symlinked_ignore = WorkdirSession::glob(
        &session,
        GlobRequest {
            pattern: "*.txt".to_string(),
            path: WorkdirPath::new("").unwrap(),
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert!(
        glob_with_symlinked_ignore
            .paths
            .iter()
            .any(|path| path.as_str() == "nested/visible.txt")
    );
    let symlink_error = WorkdirSession::grep(
        &session,
        GrepRequest {
            pattern: "needle".to_string(),
            path: WorkdirPath::new("nested").unwrap(),
            glob: Some("*.txt".to_string()),
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(symlink_error, WorkdirError::OutOfScope(_)));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_local_provider_keeps_pre_grant_root_when_path_changes_before_session_creation() {
    let parent = tempfile::tempdir().unwrap();
    let approved = parent.path().join("shared");
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("approved.txt"), "approved needle").unwrap();

    // This is the CLI's approval/open boundary. Remote grant creation happens
    // only after this descriptor has been pinned.
    let pinned = ExternalWorkdirRoot::pin(&approved).unwrap();
    std::fs::rename(&approved, parent.path().join("approved-original")).unwrap();
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("replacement.txt"), "replacement needle").unwrap();

    let session = LocalWorkdirSession::external_read_only_pinned(
        Workdir::new("external-workdir-pre-grant-pin"),
        pinned,
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap();
    let read = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("approved.txt").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap();
    assert_eq!(read.bytes, b"approved needle");

    let glob = WorkdirSession::glob(
        &session,
        GlobRequest {
            pattern: "*.txt".to_string(),
            path: WorkdirPath::new("").unwrap(),
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        glob.paths
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        vec!["approved.txt"]
    );

    let grep = WorkdirSession::grep(
        &session,
        GrepRequest {
            pattern: "needle".to_string(),
            path: WorkdirPath::new("").unwrap(),
            glob: Some("*.txt".to_string()),
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert!(grep.output.contains("approved.txt"));
    assert!(!grep.output.contains("replacement.txt"));

    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "secret needle").unwrap();
    std::fs::remove_file(approved.join("replacement.txt")).unwrap();
    std::fs::remove_dir(&approved).unwrap();
    std::os::unix::fs::symlink(outside.path(), &approved).unwrap();

    let glob_after_symlink_swap = WorkdirSession::glob(
        &session,
        GlobRequest {
            pattern: "*.txt".to_string(),
            path: WorkdirPath::new("").unwrap(),
            limit: 10,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        glob_after_symlink_swap
            .paths
            .iter()
            .map(|path| path.as_str())
            .collect::<Vec<_>>(),
        vec!["approved.txt"]
    );
    let grep_after_symlink_swap = WorkdirSession::grep(
        &session,
        GrepRequest {
            pattern: "needle".to_string(),
            path: WorkdirPath::new("").unwrap(),
            glob: Some("*.txt".to_string()),
            file_type: None,
            case_insensitive: false,
            before_context: 0,
            after_context: 0,
            multiline: false,
            output_mode: GrepOutputMode::FilesWithMatches,
            limit: 10,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert!(grep_after_symlink_swap.output.contains("approved.txt"));
    assert!(!grep_after_symlink_swap.output.contains("secret.txt"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_read_write_uses_pinned_root_for_conflict_and_commit() {
    let parent = tempfile::tempdir().unwrap();
    let approved = parent.path().join("shared");
    let approved_original = parent.path().join("approved-original");
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("item.txt"), "approved").unwrap();
    let session = LocalWorkdirSession::external_read_write(
        Workdir::new("external-workdir-pinned-rw"),
        &approved,
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();

    std::fs::rename(&approved, &approved_original).unwrap();
    std::fs::create_dir(&approved).unwrap();

    let conflict = WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            content: b"unexpected".to_vec(),
            expected_hash: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(conflict, WorkdirError::Conflict(_)));
    assert_eq!(
        std::fs::read_to_string(approved_original.join("item.txt")).unwrap(),
        "approved"
    );
    assert!(!approved.join("item.txt").exists());

    let observed = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap();
    WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            content: b"updated".to_vec(),
            expected_hash: Some(observed.content_hash),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(approved_original.join("item.txt")).unwrap(),
        "updated"
    );
    assert!(!approved.join("item.txt").exists());
}

#[tokio::test]
async fn external_read_write_rejects_oversized_preimages_and_edit_results_before_commit() {
    let root = tempfile::tempdir().unwrap();
    let limit = crate::external::MAX_EXTERNAL_WRITE_BYTES;
    let oversized_content = vec![b'x'; limit + 1];
    std::fs::write(root.path().join("oversized.txt"), &oversized_content).unwrap();
    std::fs::write(root.path().join("growth.txt"), vec![b'x'; limit]).unwrap();
    let high_replacement_content = vec![b'x'; crate::external::MAX_EXTERNAL_RESULT_ITEMS + 1];
    std::fs::write(
        root.path().join("high-replacements.txt"),
        &high_replacement_content,
    )
    .unwrap();
    let session = LocalWorkdirSession::external_read_write(
        Workdir::new("external-workdir-bounded-rw"),
        root.path(),
        BoundedReadLimits::EXTERNAL_DEFAULT,
    )
    .unwrap();

    let oversized_observed = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("oversized.txt").unwrap(),
            offset: 0,
            limit: 1,
            max_bytes: 1,
        },
    )
    .await
    .unwrap();
    let write_error = WorkdirSession::write(
        &session,
        WriteRequest {
            path: WorkdirPath::new("oversized.txt").unwrap(),
            content: b"small".to_vec(),
            expected_hash: Some(oversized_observed.content_hash),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(write_error, WorkdirError::InvalidArgument(_)));
    assert_eq!(
        std::fs::read(root.path().join("oversized.txt")).unwrap(),
        oversized_content
    );
    let edit_error = WorkdirSession::edit(
        &session,
        EditRequest {
            path: WorkdirPath::new("oversized.txt").unwrap(),
            old_string: "x".to_string(),
            new_string: "y".to_string(),
            replace_all: false,
            expected_hash: oversized_observed.content_hash,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(edit_error, WorkdirError::InvalidArgument(_)));
    assert_eq!(
        std::fs::read(root.path().join("oversized.txt")).unwrap(),
        oversized_content
    );

    let growth_observed = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("growth.txt").unwrap(),
            offset: 0,
            limit: 1,
            max_bytes: 1,
        },
    )
    .await
    .unwrap();
    let growth_error = WorkdirSession::edit(
        &session,
        EditRequest {
            path: WorkdirPath::new("growth.txt").unwrap(),
            old_string: "x".to_string(),
            new_string: "yy".to_string(),
            replace_all: true,
            expected_hash: growth_observed.content_hash,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(growth_error, WorkdirError::InvalidArgument(_)));
    assert_eq!(
        std::fs::read(root.path().join("growth.txt")).unwrap(),
        vec![b'x'; limit]
    );

    let high_replacement_observed = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("high-replacements.txt").unwrap(),
            offset: 0,
            limit: 1,
            max_bytes: 1,
        },
    )
    .await
    .unwrap();
    let high_replacement_error = WorkdirSession::edit(
        &session,
        EditRequest {
            path: WorkdirPath::new("high-replacements.txt").unwrap(),
            old_string: "x".to_string(),
            new_string: "y".to_string(),
            replace_all: true,
            expected_hash: high_replacement_observed.content_hash,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        high_replacement_error,
        WorkdirError::InvalidArgument(_)
    ));
    assert_eq!(
        std::fs::read(root.path().join("high-replacements.txt")).unwrap(),
        high_replacement_content
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_local_provider_pins_the_approved_root_across_directory_replacement() {
    let parent = tempfile::tempdir().unwrap();
    let approved = parent.path().join("shared");
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("item.txt"), "approved").unwrap();
    let session = LocalWorkdirSession::external_read_only(
        Workdir::new("external-workdir-pinned-root"),
        &approved,
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap();

    std::fs::rename(&approved, parent.path().join("approved-original")).unwrap();
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("item.txt"), "replacement").unwrap();

    let read = WorkdirSession::read(
        &session,
        ReadRequest {
            path: WorkdirPath::new("item.txt").unwrap(),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        },
    )
    .await
    .unwrap();
    assert_eq!(read.bytes, b"approved");
}
