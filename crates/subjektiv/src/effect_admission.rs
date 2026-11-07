//! Current Host authority retained through native Subject effects and protected reads.
use super::*;
use std::sync::Arc;

/// Trusted Host check, never an input DTO. Returned Host/catalog leases are held
/// through the native transaction; callbacks must use this view, not reenter the store.
pub trait SubjectEffectAuthority: Send + Sync {
    fn enter<'a>(
        &'a self,
        admission: &SubjectEffectAdmission<'_>,
    ) -> Result<Box<dyn 'a + SubjectEffectLease>>;
}

pub trait SubjectEffectLease {}
impl<T> SubjectEffectLease for T {}

pub struct SubjectEffectAdmission<'a> {
    store: &'a SubjektivStore,
    connection: &'a Connection,
}
impl SubjectEffectAdmission<'_> {
    pub fn current_policy(
        &self,
        subject: &str,
    ) -> Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        require_active_subject(self.connection, subject)?;
        self.store
            .current_policy_in_connection(self.connection, subject)
    }

    /// Use the Host source bound to this admission's authority reservation. The
    /// store's ordinary provider may reenter a connection held by a waiting writer.
    pub fn current_policy_with_canonical_source(
        &self,
        subject: &str,
        canonical_source: Arc<dyn CanonicalPermissionSource>,
    ) -> Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        let store = self
            .store
            .clone()
            .with_canonical_permission_source(canonical_source);
        SubjectEffectAdmission {
            store: &store,
            connection: self.connection,
        }
        .current_policy(subject)
    }
    pub fn writer_active(&self, binding: &journal::JournalWriterBinding) -> Result<bool> {
        require_active_subject(self.connection, &binding.subject_id)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE binding_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4 AND active=1)", params![binding.binding_id,binding.subject_id,binding.runtime_id,binding.worker_id], |row| row.get(0))?)
    }
}
impl SubjektivStore {
    /// A per-call Host adapter for Memory/Journal operations. Unrelated trusted
    /// repository/recovery calls remain unchanged; no cached grant is persisted.
    pub fn with_subject_effect_authority(
        &self,
        authority: Arc<dyn SubjectEffectAuthority>,
    ) -> Self {
        let mut store = self.clone();
        store.effect_authority = Some(authority);
        store
    }

    fn admit_effect(
        &self,
        connection: &Connection,
    ) -> Result<Option<Box<dyn '_ + SubjectEffectLease>>> {
        self.effect_authority
            .as_ref()
            .map(|authority| {
                authority.enter(&SubjectEffectAdmission {
                    store: self,
                    connection,
                })
            })
            .transpose()
    }

    pub(crate) fn memory_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.database
            .try_transaction_admitted(|connection| self.admit_effect(connection), operation)
    }

    /// Signed Memory reads retain the same native authority as mutations while
    /// reading protected data. Unbound repository/recovery reads stay unchanged.
    pub(crate) fn memory_connection<T>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        if self.effect_authority.is_some() {
            self.memory_transaction(|connection| read(connection))
        } else {
            self.database.try_with_connection(read)
        }
    }

    pub(crate) fn journal_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> journal::JournalResult<T>,
    ) -> journal::JournalResult<T> {
        self.database.try_transaction_admitted(
            |connection| {
                self.admit_effect(connection)
                    .map_err(journal_authority_error)
            },
            operation,
        )
    }

    /// Model-facing Journal reads wait for the native connection too. Bind their
    /// authority inside admission, not before that wait. Unbound Host recovery is
    /// a repository read and does not acquire a new execution permission.
    pub(crate) fn journal_connection<T>(
        &self,
        read: impl FnOnce(&Connection) -> journal::JournalResult<T>,
    ) -> journal::JournalResult<T> {
        if self.effect_authority.is_some() {
            self.journal_transaction(|connection| read(connection))
        } else {
            self.database.try_with_connection(read)
        }
    }
}

fn journal_authority_error(error: SubjektivError) -> journal::JournalError {
    match error {
        SubjektivError::AuthorizationDenied(message) => {
            journal::JournalError::PermissionDenied(message)
        }
        error => journal::JournalError::Subject(error),
    }
}
