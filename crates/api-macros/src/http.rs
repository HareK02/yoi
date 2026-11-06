//! Shared HTTP error-contract adapter, independent of Axum and Reqwest.
//!
//! Only Browser-projected failures can be encoded here. Receiving a valid failure does not
//! grant permission to forward it: domains must reproject with current authorization.

pub use error_contract::problem::{ApiBoundary, ProblemResponse};
use error_contract::{
    Browser, Budget, ContractError, DecodeFailure, ProjectedFailure, WireDetails,
};

/// Encode a projected failure or a static, non-domain 500 fallback under the same budget.
///
/// `safe_instance` must be selected by the caller's disclosure policy (for example a local
/// opaque occurrence URI), never copied from a request URI containing credentials or queries.
/// URI syntax validation is not disclosure authorization. If even the static fallback exceeds
/// the budget, return an error instead of truncating JSON or silently exceeding the limit.
pub fn problem_response<D: WireDetails>(
    projected: &ProjectedFailure<Browser, D>,
    boundary: &impl ApiBoundary<D>,
    safe_instance: &str,
    budget: Budget,
) -> Result<ProblemResponse, ContractError> {
    error_contract::problem::to_problem_details(projected, boundary, safe_instance, budget)
        .or_else(|_| error_contract::problem::problem_fallback(budget))
}

impl From<ProblemResponse> for crate::HttpErrorResponse {
    fn from(response: ProblemResponse) -> Self {
        Self {
            status: response.status,
            content_type: response.content_type,
            body: response.body().to_vec().into(),
        }
    }
}

/// Normalize a bounded HTTP error using actual transport metadata, not a body-claimed status.
///
/// Unsupported schema/reason, corrupt input, and `Corrupt(TooLarge)` remain distinct. None of
/// these results, including a known cause, establish remote execution or retry permission.
/// Streaming callers must also bound collection before calling this byte-slice adapter.
pub fn decode_problem_response<D: WireDetails>(
    input: &[u8],
    actual_status: u16,
    content_type: &str,
    boundary: &impl ApiBoundary<D>,
    budget: Budget,
) -> DecodeFailure<D> {
    error_contract::problem::decode_problem_details(
        input,
        actual_status,
        content_type,
        boundary,
        budget,
    )
}
