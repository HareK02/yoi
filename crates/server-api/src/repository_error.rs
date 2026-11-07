//! Repository lookup/registration publication uses the shared HTTP adapter.
use crate::{
    RepositoryApiError,
    repository_failure::{
        RepositoryFailureDetails, RepositoryFailureOperation, RepositoryFailureReason,
    },
};
use error_contract::problem::{ApiBoundary, HttpFailureStatus, ProblemClass};
use error_contract::{Browser, Budget, ProjectedFailure};

/// Preserve the existing caller-owned HTTP boundary, independently of category.
pub struct RepositoryBoundary;
impl ApiBoundary<RepositoryFailureDetails> for RepositoryBoundary {
    fn classify(&self, details: &RepositoryFailureDetails) -> ProblemClass {
        use RepositoryFailureReason::*;
        let status = if details.operation == RepositoryFailureOperation::ObserveMergeTarget {
            HttpFailureStatus::BadRequest
        } else {
            match details.reason {
                RegistrationIntentConflict => HttpFailureStatus::Conflict,
                UnknownRepository => HttpFailureStatus::NotFound,
                UnsupportedProvider => HttpFailureStatus::BadRequest,
                MissingDefaultSelector
                | InvalidSelector
                | CommitNotFound
                | InvalidCommitRelation
                | ProviderFailure => HttpFailureStatus::BadGateway,
            }
        };
        ProblemClass {
            status,
            type_uri: "urn:yoi:problem:repository",
            title: "Repository operation failed",
        }
    }
}

pub(crate) fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

impl RepositoryApiError {
    /// Received details are observations, not permission to re-encode for another audience.
    pub fn repository_cause(&self) -> Option<&RepositoryFailureDetails> {
        self.repository.as_ref()
    }

    /// Only a current Browser projection grants this local HTTP encoder capability.
    pub fn with_repository_projection(
        mut self,
        projected: ProjectedFailure<Browser, RepositoryFailureDetails>,
    ) -> Self {
        let response =
            api_macros::http::problem_response(&projected, &RepositoryBoundary, "", budget())
                .expect("fixed boundary budget accommodates the static fallback");
        self.repository = Some(*projected.details());
        self.status = response.status;
        self.message = projected.explanation().to_owned();
        self.repository_problem = Some(response.body().to_vec());
        self
    }
}
