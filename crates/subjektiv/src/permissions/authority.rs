//! Native lifecycle fencing and an explicit Host-owned permission source.
use super::*;
use rusqlite::TransactionBehavior;
use std::sync::Arc;

pub trait CanonicalPermissionSource: Send + Sync {
    fn subject_policy_source(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<server_api::authorization::SubjectPolicySource>> {
        self.subject_permissions(workspace_id, subject_id)
            .map(|source| source.map(server_api::authorization::SubjectPolicySource::Legacy))
    }

    fn current_subject_policy(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        self.subject_policy_source(workspace_id, subject_id)?
            .map(|source| match source {
                server_api::authorization::SubjectPolicySource::Feature(source) => Ok(source),
                server_api::authorization::SubjectPolicySource::Legacy(source) => {
                    server_api::authorization::CurrentSubjectPolicy::from_legacy(source)
                }
            })
            .transpose()
            .map_err(SubjektivError::InvalidRecord)
    }

    fn subject_permissions(
        &self,
        workspace_id: &str,
        subject_id: &str,
    ) -> Result<Option<SubjectPermissions>>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseIdentity {
    pub scope_id: String,
    pub feature_id: String,
    pub physical_id: String,
}

impl From<&feature_storage::DatabaseIdentity> for DatabaseIdentity {
    fn from(identity: &feature_storage::DatabaseIdentity) -> Self {
        Self {
            scope_id: identity.scope.clone(),
            feature_id: identity.feature.clone(),
            physical_id: identity.physical.clone(),
        }
    }
}

impl DatabaseIdentity {
    fn storage_identity(&self) -> feature_storage::DatabaseIdentity {
        feature_storage::DatabaseIdentity {
            scope: self.scope_id.clone(),
            feature: self.feature_id.clone(),
            physical: self.physical_id.clone(),
        }
    }
}

pub struct PermissionAuthorityGuard<'connection> {
    transaction: Transaction<'connection>,
}
impl PermissionAuthorityGuard<'_> {
    pub fn require_subject(&self, subject: &str) -> Result<()> {
        validate_authorization_identifier(subject).map_err(SubjektivError::InvalidRecord)?;
        require_subject_in_connection(&self.transaction, subject)?;
        Ok(())
    }
    pub fn subject_permission_enrollment(
        &self,
        subject: &str,
    ) -> Result<Option<SubjectPolicySource>> {
        self.require_subject(subject)?;
        let raw: Option<String> = self
            .transaction
            .query_row(
                "SELECT policy_json FROM subject_permission_enrollment_sources WHERE subject_id=?1",
                [subject],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
            .transpose()
    }
}
impl SubjektivStore {
    pub fn with_permission_authority_guard<T>(
        &self,
        expected: &DatabaseIdentity,
        action: impl FnOnce(&PermissionAuthorityGuard<'_>) -> Result<T>,
    ) -> Result<T> {
        if self.database.identity() != &expected.storage_identity() {
            return Err(SubjektivError::PermissionAuthorityConflict);
        }
        self.database.try_with_connection(|connection| {
            let transaction =
                Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
            action(&PermissionAuthorityGuard { transaction })
        })
    }
    pub fn with_canonical_permission_source(
        mut self,
        source: Arc<dyn CanonicalPermissionSource>,
    ) -> Self {
        self.canonical_permission_source = Some(source);
        self
    }
    pub fn database_identity(&self) -> Result<DatabaseIdentity> {
        self.database
            .try_with_connection(|_| Ok(DatabaseIdentity::from(self.database.identity())))
    }
    pub(super) fn current_permission_source(
        &self,
        connection: &Connection,
        subject: &str,
    ) -> Result<Option<SubjectPermissions>> {
        match &self.canonical_permission_source {
            Some(provider) => provider.subject_permissions(self.scope_id(), subject),
            None => source_in_connection(connection, subject),
        }
    }
}
