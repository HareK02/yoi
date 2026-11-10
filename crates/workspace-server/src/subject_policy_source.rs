//! Native Feature consumers read the same Server-owned Subject policy as Workers.
//! Reusable Workspace Sets remain a resolver input, not an individual policy writer.

use crate::SqliteWorkspaceStore;
use feature_permissions::PermissionHost;

pub(crate) struct CanonicalSubjectPolicySource {
    pub store: SqliteWorkspaceStore,
    pub host: PermissionHost,
}

impl subjektiv::CanonicalPermissionSource for CanonicalSubjectPolicySource {
    fn subject_policy_source(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::SubjectPolicySource>> {
        self.current_subject_policy(workspace, subject)
            .map(|source| source.map(server_api::authorization::SubjectPolicySource::Feature))
    }

    fn current_subject_policy(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        if self.host.catalog().workspace.as_str() != workspace {
            return Err(subjektiv::SubjektivError::PermissionTransferConflict);
        }
        self.store
            .current_subject_permission_policy(workspace, subject)
            .map_err(|error| {
                subjektiv::SubjektivError::InvalidRecord(format!(
                    "Subject permission policy unavailable: {error}"
                ))
            })
    }

    fn subject_permissions(
        &self,
        workspace: &str,
        subject: &str,
    ) -> subjektiv::Result<Option<server_api::authorization::SubjectPermissions>> {
        if self.host.catalog().workspace.as_str() != workspace {
            return Err(subjektiv::SubjektivError::PermissionTransferConflict);
        }
        self.store
            .adopted_subject_legacy_permissions(workspace, subject, &self.host)
            .map_err(|error| {
                subjektiv::SubjektivError::InvalidRecord(format!(
                    "Subject permission projection unavailable: {error}"
                ))
            })
    }
}
