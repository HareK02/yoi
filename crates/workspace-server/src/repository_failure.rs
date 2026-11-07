//! Repository lookup failures retain the concrete local source; wire details live in server-api.
use crate::repositories::RepositoryLookupError;
use error_contract::{
    Browser, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, ProjectFor, Storage, Telemetry, Tool,
};
pub use server_api::repository_failure::{
    RepositoryFailureDetails, RepositoryFailureOperation, RepositoryFailureReason,
    RepositoryFailureVisibility,
};
impl From<&RepositoryLookupError> for RepositoryFailureReason {
    fn from(source: &RepositoryLookupError) -> Self {
        match source {
            RepositoryLookupError::UnknownRepository { .. } => Self::UnknownRepository,
            RepositoryLookupError::UnsupportedProvider { .. } => Self::UnsupportedProvider,
            RepositoryLookupError::MissingDefaultSelector { .. } => Self::MissingDefaultSelector,
            RepositoryLookupError::InvalidSelector { .. } => Self::InvalidSelector,
            RepositoryLookupError::CommitNotFound { .. } => Self::CommitNotFound,
            RepositoryLookupError::InvalidCommitRelation { .. } => Self::InvalidCommitRelation,
            RepositoryLookupError::ProviderFailure { .. } => Self::ProviderFailure,
        }
    }
}

/// Retains the concrete original error as its source, without serializing it.
#[derive(Debug, thiserror::Error)]
#[error("Repository {} failed: {}", .details.operation.description(), .details.reason.explanation())]
pub struct RepositoryFailure {
    details: RepositoryFailureDetails,
    #[source]
    source: RepositoryLookupError,
}

impl RepositoryFailure {
    pub fn new(operation: RepositoryFailureOperation, source: RepositoryLookupError) -> Self {
        Self {
            details: RepositoryFailureDetails {
                operation,
                reason: RepositoryFailureReason::from(&source),
            },
            source,
        }
    }

    pub fn details(&self) -> &RepositoryFailureDetails {
        &self.details
    }

    pub fn source_error(&self) -> &RepositoryLookupError {
        &self.source
    }
}

impl FailureCause for RepositoryFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for RepositoryFailure {
            type Authorization = RepositoryFailureVisibility;
            type Details = RepositoryFailureDetails;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &RepositoryFailureVisibility,
            ) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.details.disclosure()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);

#[cfg(test)]
mod tests;
