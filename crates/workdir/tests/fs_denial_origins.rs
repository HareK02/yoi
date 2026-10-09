use std::path::Path;

use fs_operation::{AtomicWriteMode, CheckedTarget, FsAccessPolicy, FsDenialReason, FsError};
use workdir::{
    WorkdirDenialReason as Reason, WorkdirError,
    http::{WorkdirTransportError, WorkdirTransportErrorCode as Code},
};

fn assert_origin(source: std::io::Error, reason: Reason, message: &str) {
    assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(source.raw_os_error(), None);
    assert_eq!(source.to_string(), message);
    assert_transport_reason(source, reason);
}

fn assert_transport_reason(source: std::io::Error, reason: Reason) {
    let error = WorkdirError::from(FsError::io("/private-input/secret", source));
    assert!(matches!(&error, WorkdirError::Io { .. }));
    assert_eq!(error.denial_reason(), Some(reason));
    let transport = WorkdirTransportError::from_workdir_error(&error);
    assert_eq!(transport.code, Code::Denied);
    assert_eq!(transport.code.http_status(), 403);
    assert_eq!(transport.message, "Workdir operation was denied");
    let wire = serde_json::to_value(&transport).unwrap();
    assert_eq!(wire["denial_reason"], reason.as_str());
    assert!(!wire.to_string().contains("private-input"));
    assert!(!wire.to_string().contains("secret"));
    let decoded: WorkdirTransportError = serde_json::from_value(wire).unwrap();
    assert_eq!(
        WorkdirTransportError::from_workdir_error(&decoded.into_workdir_error()),
        transport
    );
}

#[test]
fn synthetic_fs_origins_survive_fs_workdir_and_transport_conversion() {
    for (origin, reason, message) in [
        (
            FsDenialReason::ProviderRootExceeded,
            Reason::ProviderRootExceeded,
            "path is outside provider root",
        ),
        (
            FsDenialReason::ProviderSymlinkDenied,
            Reason::ProviderSymlinkDenied,
            "symbolic links are not permitted by this provider",
        ),
        (
            FsDenialReason::CheckoutTargetDenied,
            Reason::CheckoutTargetDenied,
            "checkout target denied",
        ),
        (
            FsDenialReason::CheckoutCreateParentDenied,
            Reason::CheckoutCreateParentDenied,
            "Create parent denied",
        ),
    ] {
        assert_origin(origin.into_io_error(), reason, message);
    }
}

#[test]
fn custom_permission_denied_is_not_guessed_from_kind_or_message() {
    for message in [
        "Permission denied (os error 13)",
        "checkout target denied",
        "/private-input/secret",
    ] {
        assert_transport_reason(
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, message),
            Reason::UnclassifiedPermissionDenied,
        );
    }
}

#[cfg(unix)]
#[test]
fn raw_os_permission_denied_retains_errno_and_os_origin() {
    for errno in [libc::EACCES, libc::EPERM] {
        let source = std::io::Error::from_raw_os_error(errno);
        let error = WorkdirError::from(FsError::io("/private-input/secret", source));
        let WorkdirError::Io { source, .. } = &error else {
            panic!("classification changed")
        };
        assert_eq!(source.raw_os_error(), Some(errno));
        assert_transport_reason(
            std::io::Error::from_raw_os_error(errno),
            Reason::OsPermissionDenied,
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn actual_confined_open_and_write_refusals_keep_distinct_origins() {
    use fs_operation::{
        atomic_write_beneath_no_symlinks_at, open_beneath_no_symlinks, open_beneath_no_symlinks_at,
        open_root_no_symlinks,
    };
    let root = tempfile::tempdir().unwrap();
    let directory = open_root_no_symlinks(root.path()).unwrap();
    let outside = root.path().parent().unwrap().join("private-input");
    for source in [
        open_beneath_no_symlinks_at(&directory, &outside).unwrap_err(),
        open_beneath_no_symlinks(root.path(), &outside).unwrap_err(),
        atomic_write_beneath_no_symlinks_at(
            &directory,
            &outside,
            b"unused",
            AtomicWriteMode::CreateNew,
            || Ok(()),
        )
        .unwrap_err(),
        atomic_write_beneath_no_symlinks_at(
            &directory,
            Path::new("../private-input"),
            b"unused",
            AtomicWriteMode::CreateNew,
            || Ok(()),
        )
        .unwrap_err(),
    ] {
        assert_origin(
            source,
            Reason::ProviderRootExceeded,
            "path is outside provider root",
        );
    }
    std::fs::write(root.path().join("file"), "secret").unwrap();
    std::os::unix::fs::symlink("file", root.path().join("link")).unwrap();
    assert_origin(
        open_beneath_no_symlinks_at(&directory, Path::new("link")).unwrap_err(),
        Reason::ProviderSymlinkDenied,
        "symbolic links are not permitted by this provider",
    );
}

struct ExactWrite<'a>(&'a Path);
impl FsAccessPolicy for ExactWrite<'_> {
    fn is_readable(&self, _: &Path) -> bool {
        true
    }
    fn is_writable(&self, path: &Path) -> bool {
        path == self.0
    }
}

#[cfg(target_os = "linux")]
#[test]
fn actual_checked_target_and_create_parent_refusals_keep_distinct_origins() {
    let root = tempfile::tempdir().unwrap();
    let directory = fs_operation::open_root_no_symlinks(root.path()).unwrap();
    let destination = root.path().join("parent/file");
    let policy = ExactWrite(&destination);
    let source = CheckedTarget::pin(&directory, Path::new(""), root.path(), &policy, true)
        .err()
        .expect("target policy must reject");
    assert_origin(
        source,
        Reason::CheckoutTargetDenied,
        "checkout target denied",
    );
    let target =
        CheckedTarget::pin(&directory, Path::new(""), root.path(), &policy, false).unwrap();
    let create = target.create_access("parent/file".into(), destination.clone());
    let error = fs_operation::run_write(
        root.path(),
        fs_operation::WriteRequest {
            path: fs_operation::FsPath::new("parent/file").unwrap(),
            content: "unused".into(),
            expected_hash: None,
        },
        &create,
    )
    .unwrap_err();
    let FsError::Io { source, .. } = error else {
        panic!("expected parent policy denial")
    };
    assert_origin(
        source,
        Reason::CheckoutCreateParentDenied,
        "Create parent denied",
    );
    assert!(!root.path().join("parent").exists());
}

#[test]
fn every_transport_code_forwards_supplied_reason_without_reclassification() {
    // JSON is the provider boundary. An optional reason is independent of code/status.
    for code in [
        "not_found",
        "conflict",
        "outcome_unknown",
        "unsupported",
        "invalid_request",
        "denied",
        "out_of_scope",
        "symlink_out_of_scope",
        "broken_symlink",
        "symlink_target_is_directory",
        "read_only",
        "is_directory",
        "symlink_directory_not_traversed",
        "unknown_command",
        "unavailable",
        "io",
        "transport",
        "internal",
    ] {
        let decode = |reason| {
            serde_json::from_value::<WorkdirTransportError>(serde_json::json!({
                "code": code, "message": "private-input secret", "denial_reason": reason,
            }))
            .unwrap()
            .into_workdir_error()
        };
        let baseline_error = decode(None::<Reason>);
        let baseline = WorkdirTransportError::from_workdir_error(&baseline_error);
        let error = decode(Some(Reason::CheckoutTargetDenied));
        let forwarded = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(
            error.denial_reason(),
            Some(Reason::CheckoutTargetDenied),
            "{code}"
        );
        assert_eq!(forwarded.code, baseline.code, "{code}");
        assert_eq!(
            forwarded.code.http_status(),
            baseline.code.http_status(),
            "{code}"
        );
        assert_eq!(forwarded.message, baseline.message, "{code}");
        assert_eq!(error.to_string(), baseline_error.to_string(), "{code}");
        // Missing provider reasons retain the deliberate existing fallback: canonical
        // path classifications supply their known reason; all other codes stay unknown.
        assert_eq!(
            baseline_error.denial_reason(),
            match code {
                "out_of_scope" => Some(Reason::PathOutOfScope),
                "symlink_out_of_scope" => Some(Reason::SymlinkTargetOutOfScope),
                "read_only" => Some(Reason::PathReadOnly),
                _ => None,
            },
            "{code}"
        );
    }
}

#[test]
fn diagnostic_labels_match_allowlisted_wire_values() {
    for label in [
        "scope_resolution_unavailable",
        "scope_comparison_unavailable",
        "scoped_capability_denied",
        "logical_scope_exceeded",
        "invalid_scoped_path",
        "child_write_lease_conflict",
        "empty_scope",
        "parent_capability_denied",
        "command_requires_writable_scope",
        "parent_scope_exceeded",
        "cwd_outside_readable_scope",
        "read_only_session",
        "external_root_symlink",
        "external_root_changed",
        "external_root_moved",
        "external_scope_symlink",
        "attachment_scope_exceeded",
        "delegated_scope_exceeded",
        "checkout_output_root_exceeded",
        "provider_root_exceeded",
        "provider_symlink_denied",
        "checkout_target_denied",
        "checkout_create_parent_denied",
        "checkout_search_path_denied",
        "checkout_enumeration_denied",
        "path_out_of_scope",
        "symlink_target_out_of_scope",
        "path_read_only",
        "os_permission_denied",
        "unclassified_permission_denied",
    ] {
        let reason: Reason = serde_json::from_value(serde_json::json!(label)).unwrap();
        assert_eq!(reason.as_str(), label);
        assert_eq!(serde_json::to_value(reason).unwrap(), label);
    }
}
