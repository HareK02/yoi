//! Host revalidation inside native Memory transaction admission, not after effects.
use super::*;
use std::sync::Arc;

/// Trusted Host check, never an input DTO. Returned Host/catalog leases are held
/// through the native transaction; callbacks must use this view, not reenter the store.
pub trait MemoryEffectAuthority: Send + Sync {
    fn enter<'a>(
        &'a self,
        admission: &MemoryEffectAdmission<'_>,
    ) -> Result<Box<dyn 'a + MemoryEffectLease>>;
}

pub trait MemoryEffectLease {}
impl<T> MemoryEffectLease for T {}

pub struct MemoryEffectAdmission<'a> {
    store: &'a SubjektivStore,
    connection: &'a Connection,
}
impl MemoryEffectAdmission<'_> {
    pub fn current_policy(
        &self,
        subject: &str,
    ) -> Result<Option<server_api::authorization::CurrentSubjectPolicy>> {
        require_active_subject(self.connection, subject)?;
        self.store
            .current_policy_in_connection(self.connection, subject)
    }
    pub fn writer_active(&self, binding: &journal::JournalWriterBinding) -> Result<bool> {
        require_active_subject(self.connection, &binding.subject_id)?;
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM journal_writer_bindings WHERE binding_id=?1 AND subject_id=?2 AND runtime_id=?3 AND worker_id=?4 AND active=1)", params![binding.binding_id,binding.subject_id,binding.runtime_id,binding.worker_id], |row| row.get(0))?)
    }
}
impl SubjektivStore {
    /// A per-call Host adapter. The original store and all unrelated callers are
    /// unchanged; no cached authorization is persisted or used as a permission.
    pub fn with_memory_effect_authority(&self, authority: Arc<dyn MemoryEffectAuthority>) -> Self {
        let mut store = self.clone();
        store.memory_effect_authority = Some(authority);
        store
    }
    pub(crate) fn memory_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        self.database.try_transaction_admitted(
            |connection| {
                self.memory_effect_authority
                    .as_ref()
                    .map(|authority| {
                        authority.enter(&MemoryEffectAdmission {
                            store: self,
                            connection,
                        })
                    })
                    .transpose()
            },
            operation,
        )
    }
}
