//! Internal Repository access errors retain their local source; wire details live in server-api.
use error_contract::{
    Browser, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, ProjectFor, Storage, Telemetry, Tool,
};
pub use server_api::repository_access_failure::{
    RepositoryAccessDetails, RepositoryAccessOperation, RepositoryAccessReason,
    RepositoryAccessVisibility,
};
/// The old local boundary error is retained, not used to infer public meaning.
/// Boxing breaks the recursive layout when the root Error owns this failure.
#[derive(Debug, thiserror::Error)]
#[error("Repository {} failed: {}", .details.operation.description(), .details.reason.explanation())]
pub struct RepositoryAccessFailure {
    details: RepositoryAccessDetails,
    #[source]
    source: Box<crate::Error>,
}

impl RepositoryAccessFailure {
    pub(crate) fn new(
        operation: RepositoryAccessOperation,
        reason: RepositoryAccessReason,
        source: crate::Error,
    ) -> Self {
        Self {
            details: RepositoryAccessDetails { operation, reason },
            source: Box::new(source),
        }
    }

    pub fn details(&self) -> &RepositoryAccessDetails {
        &self.details
    }

    pub fn source_error(&self) -> &crate::Error {
        &self.source
    }
}

impl FailureCause for RepositoryAccessFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for RepositoryAccessFailure {
            type Authorization = RepositoryAccessVisibility;
            type Details = RepositoryAccessDetails;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &RepositoryAccessVisibility) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                self.details.disclosure()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);
