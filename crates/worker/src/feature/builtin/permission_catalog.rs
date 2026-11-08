//! Trusted metadata composition of Feature-owned declarations, independent of
//! Tool enablement. The legacy resource editor is not an operation authority.

use super::super::{FeatureInstallError, FeatureRegistryBuilder};
use super::FeaturePermissionRegistry;
use crate::feature::builtin;
use feature_permissions::{PermissionHost, WorkspaceId};

pub fn builtin_permission_registry(
    workspace: WorkspaceId,
) -> Result<FeaturePermissionRegistry, FeatureInstallError> {
    let mut builder = FeatureRegistryBuilder::new();
    // This is module composition, not a namespace/resource ownership table.
    // Each participant supplies its own registered identity and declaration.
    for declaration in [
        &builtin::manage_workdir::permission_declaration::FILESYSTEM,
        &builtin::bash::permission_declaration::DECLARATION,
        &builtin::manage_workdir::permission_declaration::DECLARATION,
        &builtin::manage_worker::permission_declaration::DECLARATION,
        &builtin::manage_worker::permission_declaration::SUBWORKER,
        &builtin::ticket::permission_declaration::DECLARATION,
        &builtin::objective::permission_declaration::DECLARATION,
        &builtin::merge_request::permission_declaration::DECLARATION,
        &builtin::repository_permissions::DECLARATION,
        &builtin::drive::permission_declaration::DECLARATION,
        &builtin::workspace_config::permission_declaration::DECLARATION,
        &builtin::memory::permission_declaration::DECLARATION,
        &builtin::journal::permission_declaration::DECLARATION,
        &builtin::subjektiv_chat::permission_declaration::DECLARATION,
        &builtin::session_explore::permission_declaration::DECLARATION,
        &builtin::web::permission_declaration::DECLARATION,
        &builtin::notification_permissions::DECLARATION,
        &builtin::plugin_host_permissions::DECLARATION,
    ] {
        builder = builder.with_module(super::BuiltinPermissionFeature(declaration));
    }
    builder = builder.with_module(builtin::subject_permissions::SubjectPermissionsFeature);
    builder.permission_registry(workspace)
}

pub fn builtin_permission_host(
    workspace: WorkspaceId,
) -> Result<PermissionHost, FeatureInstallError> {
    Ok(builtin_permission_registry(workspace)?.into_host())
}

#[cfg(test)]
#[path = "permission_catalog_tests.rs"]
mod tests;
