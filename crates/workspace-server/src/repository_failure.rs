//! Repository-owned failure meaning and disclosure, independent of transport.
//!
//! The original lookup error stays local, including identifiers, provider text,
//! selectors and commit details. Only closed operation/reason values cross the
//! shared codec. Every audience, including storage and telemetry, needs a current
//! visibility decision; received explanations and context are never forwarded.
//!
//! Category is presentation/aggregation metadata, not HTTP status, retry
//! permission or an operation outcome. Those decisions remain with the caller.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use serde::{Deserialize, Serialize};

use crate::repositories::RepositoryLookupError;

/// The caller supplies the attempted operation, never provider-authored prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryFailureOperation {
    Summary,
    SummaryByKey,
    RecentLog,
    ObserveMergeTarget,
    ObserveCommit,
    EnsureAncestor,
}

impl RepositoryFailureOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Summary => "summary",
            Self::SummaryByKey => "summary_by_key",
            Self::RecentLog => "recent_log",
            Self::ObserveMergeTarget => "observe_merge_target",
            Self::ObserveCommit => "observe_commit",
            Self::EnsureAncestor => "ensure_ancestor",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Summary => "summary lookup",
            Self::SummaryByKey => "summary lookup by key",
            Self::RecentLog => "recent commit log retrieval",
            Self::ObserveMergeTarget => "merge target observation",
            Self::ObserveCommit => "commit observation",
            Self::EnsureAncestor => "commit ancestry check",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryFailureReason {
    UnknownRepository,
    UnsupportedProvider,
    MissingDefaultSelector,
    InvalidSelector,
    CommitNotFound,
    InvalidCommitRelation,
    ProviderFailure,
}

impl RepositoryFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnknownRepository => "unknown_repository",
            Self::UnsupportedProvider => "unsupported_provider",
            Self::MissingDefaultSelector => "missing_default_selector",
            Self::InvalidSelector => "invalid_selector",
            Self::CommitNotFound => "commit_not_found",
            Self::InvalidCommitRelation => "invalid_commit_relation",
            Self::ProviderFailure => "provider_failure",
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Self::UnknownRepository => "the requested repository was not found in the registry",
            Self::UnsupportedProvider => {
                "the configured repository provider is not supported for this operation"
            }
            Self::MissingDefaultSelector => "no default selector is configured for the repository",
            Self::InvalidSelector => "the requested repository selector is invalid",
            Self::CommitNotFound => "the requested repository commit was not found",
            Self::InvalidCommitRelation => "the commits do not satisfy the required relationship",
            Self::ProviderFailure => {
                "the repository provider operation failed; no more specific typed cause was reported"
            }
        }
    }
}

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

/// No raw identifier, path, selector, provider or diagnostic text is a wire field.
/// Identity is derived from this reason, so a separate identity cannot contradict it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryFailureDetails {
    pub operation: RepositoryFailureOperation,
    pub reason: RepositoryFailureReason,
}

impl RepositoryFailureDetails {
    fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let impact = format!(
            "Repository {} could not complete",
            self.operation.description()
        );
        let mut disclosure = Disclosure::new(
            *self,
            compose_explanation(self.reason.explanation(), &impact)?,
        );
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), "operation")?);
        Ok(disclosure)
    }
}

impl FailureCause for RepositoryFailureDetails {
    fn identity(&self) -> FailureIdentity {
        let category = match self.reason {
            RepositoryFailureReason::UnsupportedProvider
            | RepositoryFailureReason::ProviderFailure => Category::Dependency,
            RepositoryFailureReason::UnknownRepository
            | RepositoryFailureReason::MissingDefaultSelector
            | RepositoryFailureReason::InvalidSelector
            | RepositoryFailureReason::CommitNotFound
            | RepositoryFailureReason::InvalidCommitRelation => Category::InvalidInput,
        };
        FailureIdentity {
            domain: "repository",
            reason: self.reason.as_str(),
            category,
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        // Both required fields have a closed, bounded vocabulary. There are no
        // optional prose fields to reinterpret as a more specific provider cause.
        Ok(())
    }
}

impl WireDetails for RepositoryFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "repository"
            && details_version == 1
            && serde_json::from_value::<RepositoryFailureReason>(serde_json::Value::String(
                reason.to_owned(),
            ))
            .is_ok()
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

/// Set by the authenticated boundary after checking current resource/diagnostic
/// visibility. It is not serializable and never inferred from the audience.
#[derive(Clone, Copy, Debug, Default)]
pub struct RepositoryFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for RepositoryFailureDetails {
            type Authorization = RepositoryFailureVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &RepositoryFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.disclosure()
            }
        }
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
