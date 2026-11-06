//! Local configuration causes retain the original tree error; wire details live in server-api.
use config_source::ConfigTreeError;
use error_contract::{
    Browser, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, ProjectFor, Storage, Telemetry, Tool,
};
pub use server_api::configuration_failure::{
    ConfigurationFailureDetails, ConfigurationFailureOperation, ConfigurationFailureReason,
    ConfigurationFailureVisibility,
};

fn tree_reason(source: &ConfigTreeError) -> ConfigurationFailureReason {
    use ConfigurationFailureReason::*;
    // Match variants explicitly; never parse Display or publish their payloads.
    match source {
        ConfigTreeError::InvalidPath(_) => InvalidPath,
        ConfigTreeError::InvalidImport(_) => InvalidImport,
        ConfigTreeError::ImportEscape(_) => ImportEscape,
        ConfigTreeError::AlreadyExists(_) => AlreadyExists,
        ConfigTreeError::NotFound(_) => NotFound,
        ConfigTreeError::EntryConflict(_) => EntryConflict,
        ConfigTreeError::EntryDigestMismatch(_) => EntryDigestMismatch,
        ConfigTreeError::EntryPathMismatch(_) => EntryPathMismatch,
        ConfigTreeError::TreeDigestMismatch => TreeDigestMismatch,
        ConfigTreeError::PathChangedMoreThanOnce(_) => PathChangedMoreThanOnce,
        ConfigTreeError::DuplicatePath => DuplicatePath,
        ConfigTreeError::DuplicateSchemaProvider(_) => DuplicateSchemaProvider,
        ConfigTreeError::DuplicateSchemaNamespace(_) => DuplicateSchemaNamespace,
        ConfigTreeError::InvalidSchemaContribution(_) => InvalidSchemaContribution,
        ConfigTreeError::LimitExceeded(_) => LimitExceeded,
    }
}
/// Local, nonserializable owner of the original typed tree error, when present.
/// Display is also safe while callers still expose it through legacy API errors.
#[derive(Debug, thiserror::Error)]
#[error("Configuration {} could not complete: {}", .details.operation.description(), .details.reason.explanation())]
pub struct ConfigurationFailure {
    details: ConfigurationFailureDetails,
    #[source]
    source: Option<ConfigTreeError>,
}

impl ConfigurationFailure {
    /// Assign a known reason at the branch deciding it, not from an error string.
    pub fn new(
        operation: ConfigurationFailureOperation,
        reason: ConfigurationFailureReason,
    ) -> Self {
        Self {
            details: ConfigurationFailureDetails { operation, reason },
            source: None,
        }
    }

    pub fn from_tree(operation: ConfigurationFailureOperation, source: ConfigTreeError) -> Self {
        Self {
            details: ConfigurationFailureDetails {
                operation,
                reason: tree_reason(&source),
            },
            source: Some(source),
        }
    }

    pub(crate) fn with_operation(mut self, operation: ConfigurationFailureOperation) -> Self {
        self.details.operation = operation;
        self
    }

    pub fn details(&self) -> &ConfigurationFailureDetails {
        &self.details
    }

    pub fn source_error(&self) -> Option<&ConfigTreeError> {
        self.source.as_ref()
    }
}

// Callers may consume the original failure in FailureFacts::new, or copy its safe
// details into facts while retaining the source in their existing error/result.
impl FailureCause for ConfigurationFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for ConfigurationFailure {
            type Authorization = ConfigurationFailureVisibility;
            type Details = ConfigurationFailureDetails;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &ConfigurationFailureVisibility,
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
