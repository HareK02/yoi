//! Trusted Host operation context, never deserialized from model input.
use super::{OperationError, OperationResult, subjektiv_store_error};
pub use crate::journal::{JournalHostContext, JournalJobContext};
use crate::{SubjectSessionAttribution, SubjektivStore};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JobAttemptBinding {
    pub job_id: String,
    pub attempt_id: String,
}

#[derive(Debug, Clone)]
enum Authority {
    Body {
        attribution: Option<SubjectSessionAttribution>,
    },
    Consolidation {
        candidate_ids: Option<Vec<String>>,
        job: Option<JobAttemptBinding>,
    },
}

/// Operation context constructed by a Host after checking external authentication, leases, delegation, and Session evidence.
/// Constructors validate only store domain bindings, not external execution authority.
/// Not a request DTO; models never select the Subject or Job batch.
#[derive(Debug, Clone)]
pub struct HostOperationContext {
    scope_id: String,
    subject_id: String,
    authority: Authority,
}

impl HostOperationContext {
    /// Pins the Host-verified execution context for the Subject body.
    /// Supplied attribution must match immutable store history. The Host resolves and validates
    /// explicit staging evidence against committed public Sessions; neither this constructor nor operations read Session files.
    pub fn validated_body(
        store: &SubjektivStore,
        subject_id: &str,
        attribution: Option<SubjectSessionAttribution>,
    ) -> OperationResult<Self> {
        Self::require_subject(store, subject_id)?;
        if let Some(record) = &attribution {
            if record.subject_id != subject_id
                || store
                    .session_attribution(&record.session_id)
                    .map_err(subjektiv_store_error)?
                    .as_ref()
                    != Some(record)
            {
                return Err(OperationError::PermissionDenied(
                    "Session attribution does not match the Host subject".into(),
                ));
            }
        }
        Ok(Self {
            scope_id: store.scope_id().into(),
            subject_id: subject_id.into(),
            authority: Authority::Body { attribution },
        })
    }

    /// Pins Host-verified consolidation authority and, when needed, the current Job attempt.
    /// `Some(candidate_ids)` is an immutable allowed set, including empty batches; candidates outside it cannot be operated on.
    /// `None` is for non-Job Host authority, never issued to a restricted Job.
    /// This context cannot perform Subject-body-only operations.
    pub fn validated_consolidation(
        store: &SubjektivStore,
        subject_id: &str,
        candidate_ids: Option<Vec<String>>,
        job: Option<JobAttemptBinding>,
    ) -> OperationResult<Self> {
        Self::require_subject(store, subject_id)?;
        if let Some(ids) = &candidate_ids {
            for id in ids {
                if store
                    .staging_candidate(subject_id, id)
                    .map_err(subjektiv_store_error)?
                    .is_none()
                {
                    return Err(OperationError::InvalidInput(format!(
                        "candidate_not_found: {id}"
                    )));
                }
            }
        }
        if job.as_ref().is_some_and(|binding| {
            binding.job_id.trim().is_empty() || binding.attempt_id.trim().is_empty()
        }) {
            return Err(OperationError::InvalidInput(
                "Job attempt binding must not be empty".into(),
            ));
        }
        Ok(Self {
            scope_id: store.scope_id().into(),
            subject_id: subject_id.into(),
            authority: Authority::Consolidation { candidate_ids, job },
        })
    }

    fn require_subject(store: &SubjektivStore, id: &str) -> OperationResult<()> {
        store
            .subject(id)
            .map_err(subjektiv_store_error)?
            .ok_or_else(|| OperationError::SubjectNotFound(id.into()))?;
        Ok(())
    }

    pub fn subject_id(&self) -> &str {
        &self.subject_id
    }

    pub(super) fn require_store(&self, store: &SubjektivStore) -> OperationResult<()> {
        if self.scope_id != store.scope_id() {
            return Err(OperationError::PermissionDenied(
                "Host operation context belongs to another storage scope".into(),
            ));
        }
        Self::require_subject(store, &self.subject_id)?;
        Ok(())
    }

    pub(super) fn body(&self) -> OperationResult<Option<&SubjectSessionAttribution>> {
        match &self.authority {
            Authority::Body { attribution } => Ok(attribution.as_ref()),
            _ => Err(OperationError::PermissionDenied(
                "operation requires Subject body authority".into(),
            )),
        }
    }

    pub(super) fn consolidation(
        &self,
    ) -> OperationResult<(Option<&[String]>, Option<&JobAttemptBinding>)> {
        match &self.authority {
            Authority::Consolidation { candidate_ids, job } => {
                Ok((candidate_ids.as_deref(), job.as_ref()))
            }
            _ => Err(OperationError::PermissionDenied(
                "operation requires consolidation authority".into(),
            )),
        }
    }

    pub(super) fn candidate(&self, id: &str) -> OperationResult<()> {
        let (ids, _) = self.consolidation()?;
        if ids.is_some_and(|ids| !ids.iter().any(|allowed| allowed == id)) {
            return Err(OperationError::PermissionDenied(
                "candidate is outside the admitted Job batch".into(),
            ));
        }
        Ok(())
    }
}
