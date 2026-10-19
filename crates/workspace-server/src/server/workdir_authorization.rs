//! Subject resource checks at the Workdir provider boundary, in addition to existing ceilings.
//!
//! Paths here are attachment-relative lexical resources. Providers still own resolved-path,
//! symlink, capability and checkout validation; this helper never substitutes for those checks.

use server_api::authorization::{PermissionResource, ScopedPermission};
use workdir::{
    WorkdirPath, WorkdirSessionCapabilities, WorkdirToolScopePermission,
    http::WorkdirSessionOperation,
};

use super::{ApiError, ApiResult, Error, RuntimeWorkerRef, WorkspaceApi};
use crate::store::WorkerWorkdirLinkRecord;

const READ: &str = "filesystem.read";
const WRITE: &str = "filesystem.write";
const COMMAND: &str = "bash.execute";

pub(super) fn authorize(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    link: &WorkerWorkdirLinkRecord,
    operation: &WorkdirSessionOperation,
) -> ApiResult<()> {
    // These are reconciliation/termination of an already registered effect, not execution.
    // Keep the existing worker/attachment/opaque-handle ownership checks on these routes.
    if is_command_reconciliation(operation) {
        return Ok(());
    }
    let permissions = super::subject_authorization::worker_permissions(api, worker)?;
    authorize_permissions(permissions.as_deref(), &link.workdir_id, operation)
}

fn is_command_reconciliation(operation: &WorkdirSessionOperation) -> bool {
    matches!(
        operation,
        WorkdirSessionOperation::CommandCancel(_) | WorkdirSessionOperation::CommandStatus(_)
    )
}

fn denied(message: &str) -> ApiError {
    Error::WorkspacePermissionDenied(format!("Workdir Subject authorization: {message}")).into()
}

fn resource(
    working_directory_id: &str,
    operation: &str,
    path: &WorkdirPath,
    recursive: bool,
) -> ApiResult<ScopedPermission> {
    // FsPath also accepts explicitly scoped absolute paths. Subject resources here are
    // attachment-relative only, even if the provider separately supports absolute grants.
    let normalized = WorkdirPath::new(path.as_str())
        .map_err(|_| denied("operation path must be attachment-relative without escape"))?;
    let permission = ScopedPermission {
        operation: operation.into(),
        resource: PermissionResource::Workdir {
            working_directory_id: working_directory_id.into(),
            path: if normalized.is_root() {
                ".".into()
            } else {
                normalized.as_str().into()
            },
            recursive,
        },
    };
    permission
        .validate()
        .map_err(|_| denied("invalid normalized operation resource"))?;
    Ok(permission)
}

fn require(
    permissions: &[ScopedPermission],
    working_directory_id: &str,
    operation: &str,
    path: &WorkdirPath,
    recursive: bool,
) -> ApiResult<()> {
    let requested = resource(working_directory_id, operation, path, recursive)?;
    if permissions.iter().any(|granted| granted.covers(&requested)) {
        Ok(())
    } else {
        Err(denied("operation or resource is outside the current grant"))
    }
}

fn authorize_permissions(
    permissions: Option<&[ScopedPermission]>,
    working_directory_id: &str,
    operation: &WorkdirSessionOperation,
) -> ApiResult<()> {
    if is_command_reconciliation(operation) {
        return Ok(());
    }
    let Some(permissions) = permissions else {
        // Non-Subject Workers retain the existing provider/Profile/attachment ceilings.
        return Ok(());
    };
    let check = |operation, path: &WorkdirPath, recursive| {
        require(
            permissions,
            working_directory_id,
            operation,
            path,
            recursive,
        )
    };
    match operation {
        WorkdirSessionOperation::Stat(request) => check(READ, &request.path, false),
        WorkdirSessionOperation::Read(request) => check(READ, &request.path, false),
        WorkdirSessionOperation::ReadBytes(request) => check(READ, &request.path, false),
        WorkdirSessionOperation::Write(request) => check(WRITE, &request.path, false),
        WorkdirSessionOperation::Edit(request) => check(WRITE, &request.path, false),
        WorkdirSessionOperation::List(request) => {
            // Directory enumeration accesses children, not just the directory inode.
            check(READ, &request.path, true)?;
            if let Some(cursor) = &request.after {
                check(READ, &cursor.path, false)?;
            }
            Ok(())
        }
        WorkdirSessionOperation::Glob(request) => check(READ, &request.path, true),
        WorkdirSessionOperation::Grep(request) => check(READ, &request.path, true),
        WorkdirSessionOperation::AuthorizeScope(request) => check(
            match request.permission {
                WorkdirToolScopePermission::Read => READ,
                WorkdirToolScopePermission::Write => WRITE,
            },
            &request.path,
            false,
        ),
        WorkdirSessionOperation::CommandStart(request) => {
            // Validating cwd is not a shell confinement mechanism. The command can cd elsewhere,
            // so a prefix grant (even one covering cwd recursively) is never enough.
            resource(working_directory_id, COMMAND, &request.cwd, false)?;
            if request.spill_dir.is_some() {
                // This is a provider-local PathBuf, not an attachment-relative WorkdirPath.
                // Do not authorize an unrepresentable write destination as attachment scope.
                return Err(denied(
                    "caller-selected provider-local command spill paths are unsupported",
                ));
            }
            check(COMMAND, &WorkdirPath::root(), true)
        }
        WorkdirSessionOperation::CommandOutput(_) => {
            // Output may contain anything the original shell read. Unlike status/cancel,
            // retrieving its contents requires a current whole-attachment command grant.
            check(COMMAND, &WorkdirPath::root(), true)
        }
        WorkdirSessionOperation::CommandCancel(_) | WorkdirSessionOperation::CommandStatus(_) => {
            Ok(())
        }
        // These contracts have additional targets, scopes or provider-resolved metadata.
        // Deny explicitly until all their accesses can be mapped to scoped resources.
        WorkdirSessionOperation::ScopeRulesOverlap(_)
        | WorkdirSessionOperation::CheckoutObserve(_)
        | WorkdirSessionOperation::CheckoutExecute(_)
        | WorkdirSessionOperation::CheckoutSearch(_) => {
            Err(denied("operation has no scoped Subject resource mapping"))
        }
    }
}

/// Attenuate coarse attachment capabilities, without converting a path grant into root scope.
/// Per-operation authorization above remains required, including for recursive searches.
pub(super) fn intersect_capabilities(
    permissions: &[ScopedPermission],
    working_directory_id: &str,
    ceiling: WorkdirSessionCapabilities,
) -> WorkdirSessionCapabilities {
    let has_path_grant = |operation: &str| {
        permissions.iter().any(|grant| {
            grant.operation == operation
                && grant.validate().is_ok()
                && matches!(&grant.resource, PermissionResource::Workdir {
                    working_directory_id: id, ..
                } if id == working_directory_id)
        })
    };
    let mut allowed = WorkdirSessionCapabilities::EMPTY;
    if has_path_grant(READ) {
        allowed = allowed.union(WorkdirSessionCapabilities::READ_ONLY);
    }
    if has_path_grant(WRITE) {
        allowed = allowed.union(WorkdirSessionCapabilities::WRITE_ONLY);
    }
    if require(
        permissions,
        working_directory_id,
        COMMAND,
        &WorkdirPath::root(),
        true,
    )
    .is_ok()
    {
        allowed = allowed.union(WorkdirSessionCapabilities::COMMAND_ONLY);
    }
    ceiling.intersection(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use workdir::{
        CommandHandle, CommandOutputRequest, CommandRequest, EditRequest, GlobRequest, GrepRequest,
        ListRequest, ReadBytesRequest, ReadRequest, StatRequest, WriteRequest,
    };

    fn path(value: &str) -> WorkdirPath {
        WorkdirPath::new(value).unwrap()
    }

    fn grant(operation: &str, target: &str, recursive: bool) -> ScopedPermission {
        resource("wd", operation, &path(target), recursive).unwrap()
    }

    fn allowed(grants: &[ScopedPermission], operation: &WorkdirSessionOperation) -> bool {
        authorize_permissions(Some(grants), "wd", operation).is_ok()
    }

    fn read(target: &str) -> WorkdirSessionOperation {
        WorkdirSessionOperation::Read(ReadRequest {
            path: path(target),
            offset: 0,
            limit: 10,
            max_bytes: 1024,
        })
    }

    fn command(cwd: &str) -> WorkdirSessionOperation {
        WorkdirSessionOperation::CommandStart(CommandRequest {
            command: "cd ..; cat secret".into(),
            timeout_secs: 1,
            output_limit: 1024,
            cwd: path(cwd),
            spill_dir: None,
            tool_call_id: None,
        })
    }

    #[test]
    fn real_filesystem_operations_require_exact_operation_and_attachment_scope() {
        let read_grant = grant(READ, "src", true);
        let write_grant = grant(WRITE, "src", true);
        let reads = [
            read("src/a"),
            WorkdirSessionOperation::Stat(StatRequest {
                path: path("src/a"),
            }),
            WorkdirSessionOperation::ReadBytes(ReadBytesRequest {
                path: path("src/a"),
                offset: 0,
                max_bytes: 10,
                expected_hash: None,
            }),
            WorkdirSessionOperation::List(ListRequest {
                path: path("src"),
                limit: 10,
                after: None,
            }),
            WorkdirSessionOperation::Glob(GlobRequest {
                pattern: "**/*".into(),
                path: path("src"),
                limit: 10,
            }),
            WorkdirSessionOperation::Grep(GrepRequest {
                pattern: "needle".into(),
                path: path("src"),
                glob: None,
                file_type: None,
                case_insensitive: false,
                before_context: 0,
                after_context: 0,
                multiline: false,
                output_mode: Default::default(),
                limit: 10,
                offset: 0,
            }),
        ];
        for operation in reads {
            assert!(
                allowed(std::slice::from_ref(&read_grant), &operation),
                "{operation:?}"
            );
            assert!(
                !allowed(std::slice::from_ref(&write_grant), &operation),
                "{operation:?}"
            );
            assert!(
                authorize_permissions(
                    Some(std::slice::from_ref(&read_grant)),
                    "other-wd",
                    &operation
                )
                .is_err()
            );
        }
        let writes = [
            WorkdirSessionOperation::Write(WriteRequest {
                path: path("src/a"),
                content: vec![],
                expected_hash: None,
            }),
            WorkdirSessionOperation::Edit(EditRequest {
                path: path("src/a"),
                old_string: "a".into(),
                new_string: "b".into(),
                replace_all: false,
                expected_hash: [0; 32],
            }),
        ];
        for operation in writes {
            assert!(allowed(std::slice::from_ref(&write_grant), &operation));
            assert!(!allowed(std::slice::from_ref(&read_grant), &operation));
        }
        assert!(!allowed(&[read_grant], &read("src-other/a")));
    }

    #[test]
    fn activation_workspace_and_parent_operation_names_do_not_grant_file_access() {
        for permission in [
            ScopedPermission {
                operation: "filesystem".into(),
                resource: PermissionResource::Feature,
            },
            grant("filesystem", ".", true),
            grant("filesystem.read.extra", ".", true),
            ScopedPermission {
                operation: READ.into(),
                resource: PermissionResource::Workspace {
                    workspace_id: "ws".into(),
                },
            },
        ] {
            assert!(!allowed(&[permission], &read("src/a")));
        }
        assert!(!allowed(&[], &read("src/a")));
        assert!(authorize_permissions(None, "wd", &read("src/a")).is_ok());
    }

    #[test]
    fn nonrecursive_grant_cannot_enumerate_descendants_or_search() {
        let grants = [grant(READ, "src", false)];
        assert!(allowed(&grants, &read("src")));
        assert!(!allowed(&grants, &read("src/a")));
        assert!(!allowed(
            &grants,
            &WorkdirSessionOperation::List(ListRequest {
                path: path("src"),
                limit: 10,
                after: None
            })
        ));
        assert!(!allowed(
            &grants,
            &WorkdirSessionOperation::Glob(GlobRequest {
                pattern: "a".into(),
                path: path("src"),
                limit: 10
            })
        ));
    }

    #[test]
    fn wire_paths_are_normalized_and_escape_is_denied_before_provider_dispatch() {
        let grants = [grant(READ, "src", true)];
        for bad in [
            "/src/a",
            "../src/a",
            "src/../secret",
            "src\\a",
            "C:/src/a",
            "src/\u{0}a",
        ] {
            let operation = serde_json::from_value::<WorkdirSessionOperation>(json!({
                "operation": "stat", "request": {"path": bad}
            }));
            // Some escapes are already rejected by FsPath's wire boundary. Absolute
            // provider paths and malformed resource names must also fail this boundary.
            if let Ok(operation) = operation {
                assert!(!allowed(&grants, &operation), "{bad:?}");
            }
        }
        let operation: WorkdirSessionOperation = serde_json::from_value(json!({
            "operation": "stat", "request": {"path": "./src//a"}
        }))
        .unwrap();
        assert!(allowed(&grants, &operation));
    }

    #[test]
    fn scope_probes_and_list_cursors_cannot_access_outside_the_granted_resource() {
        let grants = [grant(READ, "src", true)];
        for (target, permission, expected) in [
            ("src/a", WorkdirToolScopePermission::Read, true),
            ("private/a", WorkdirToolScopePermission::Read, false),
            ("src/a", WorkdirToolScopePermission::Write, false),
        ] {
            let operation = WorkdirSessionOperation::AuthorizeScope(
                workdir::WorkdirScopeAuthorizationRequest {
                    rules: vec![],
                    path: path(target),
                    permission,
                },
            );
            assert_eq!(allowed(&grants, &operation), expected);
        }
        let list = |cursor| {
            WorkdirSessionOperation::List(ListRequest {
                path: path("src"),
                limit: 10,
                after: Some(workdir::ListCursor {
                    kind: workdir::EntryKind::File,
                    path: path(cursor),
                }),
            })
        };
        assert!(allowed(&grants, &list("src/a")));
        assert!(!allowed(&grants, &list("private/a")));
    }

    #[test]
    fn arbitrary_shell_requires_recursive_attachment_root_not_cwd_scope() {
        assert!(!allowed(&[grant(COMMAND, "src", true)], &command("src")));
        assert!(!allowed(&[grant(COMMAND, ".", false)], &command("src")));
        assert!(!allowed(&[grant(WRITE, ".", true)], &command("src")));
        assert!(allowed(&[grant(COMMAND, ".", true)], &command("src")));
        let WorkdirSessionOperation::CommandStart(mut request) = command("src") else {
            unreachable!()
        };
        request.spill_dir = Some("/provider/private".into());
        assert!(!allowed(
            &[grant(COMMAND, ".", true)],
            &WorkdirSessionOperation::CommandStart(request)
        ));
    }

    #[test]
    fn revocation_denies_new_execute_and_output_but_preserves_cancel_and_status() {
        let handle = CommandHandle("owned-opaque-handle".into());
        assert!(allowed(
            &[],
            &WorkdirSessionOperation::CommandCancel(handle.clone())
        ));
        assert!(allowed(
            &[],
            &WorkdirSessionOperation::CommandStatus(handle.clone())
        ));
        assert!(!allowed(&[], &command(".")));
        let output = WorkdirSessionOperation::CommandOutput(CommandOutputRequest {
            handle,
            cursor: 0,
            limit: 10,
            wait: false,
        });
        assert!(!allowed(&[], &output));
        assert!(allowed(&[grant(COMMAND, ".", true)], &output));
    }

    #[test]
    fn unsupported_checkout_and_scope_comparison_fail_closed_for_scoped_workers() {
        let grants = [
            grant(READ, ".", true),
            grant(WRITE, ".", true),
            grant(COMMAND, ".", true),
        ];
        let operations = [
            serde_json::from_value::<WorkdirSessionOperation>(json!({
                "operation": "scope_rules_overlap",
                "request": {
                    "left": {"target": "src", "permission": "read", "recursive": true},
                    "right": {"target": "private", "permission": "read", "recursive": true}
                }
            }))
            .unwrap(),
            WorkdirSessionOperation::CheckoutObserve(path("src")),
            WorkdirSessionOperation::CheckoutExecute(workdir::CheckoutRequest {
                target: path("src"),
                validator: vec![],
                operation: workdir::CheckoutOperation::Create {
                    path: path("outside/a"),
                    content: vec![],
                },
            }),
            WorkdirSessionOperation::CheckoutSearch(workdir::CheckoutSearchRequest::new(
                workdir::CheckoutSearchOperation::List(ListRequest {
                    path: path("src"),
                    limit: 10,
                    after: None,
                }),
            )),
        ];
        for operation in operations {
            assert!(!allowed(&grants, &operation));
            assert!(authorize_permissions(None, "wd", &operation).is_ok());
        }
    }

    #[test]
    fn readonly_delegation_attenuates_edit_capable_profile_without_losing_narrow_read_scope() {
        let grants = [grant(READ, "src", true)];
        assert_eq!(
            intersect_capabilities(&grants, "wd", WorkdirSessionCapabilities::ALL),
            WorkdirSessionCapabilities::READ_ONLY
        );
        assert_eq!(
            intersect_capabilities(&grants, "other", WorkdirSessionCapabilities::ALL),
            WorkdirSessionCapabilities::EMPTY
        );
        assert_eq!(
            intersect_capabilities(
                &[grant(WRITE, ".", true)],
                "wd",
                WorkdirSessionCapabilities::READ_ONLY
            ),
            WorkdirSessionCapabilities::EMPTY
        );
        assert_eq!(
            intersect_capabilities(
                &[grant(COMMAND, "src", true)],
                "wd",
                WorkdirSessionCapabilities::ALL
            ),
            WorkdirSessionCapabilities::EMPTY
        );
        assert_eq!(
            intersect_capabilities(
                &[grant(COMMAND, ".", true)],
                "wd",
                WorkdirSessionCapabilities::READ_WRITE
            ),
            WorkdirSessionCapabilities::EMPTY
        );
    }
}
