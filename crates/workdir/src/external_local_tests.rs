use crate::http::{
    WorkdirSessionOperation, WorkdirSessionOperationResult, dispatch_workdir_session_operation,
};
use crate::{
    BoundedReadLimits, CommandRequest, EditRequest, ExternalWorkdirRoot, LocalWorkdirSession,
    ReadRequest, Workdir, WorkdirError, WorkdirPath, WorkdirSession, WorkdirSessionCapabilities,
    WorkdirSessionCapability, WriteRequest,
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
            cwd: None,
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
        assert!(matches!(
            error,
            WorkdirError::SymlinkOutOfScope { .. } | WorkdirError::OutOfScope(_)
        ));
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

#[cfg(target_os = "linux")]
#[tokio::test]
async fn external_local_provider_keeps_pre_grant_root_when_path_changes_before_session_creation() {
    let parent = tempfile::tempdir().unwrap();
    let approved = parent.path().join("shared");
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("item.txt"), "approved").unwrap();

    // This is the CLI's approval/open boundary. Remote grant creation happens
    // only after this descriptor has been pinned.
    let pinned = ExternalWorkdirRoot::pin(&approved).unwrap();
    std::fs::rename(&approved, parent.path().join("approved-original")).unwrap();
    std::fs::create_dir(&approved).unwrap();
    std::fs::write(approved.join("item.txt"), "replacement").unwrap();

    let session = LocalWorkdirSession::external_read_only_pinned(
        Workdir::new("external-workdir-pre-grant-pin"),
        pinned,
        BoundedReadLimits::new(4096, 1024).unwrap(),
    )
    .unwrap();
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
