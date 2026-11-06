//! Drive HTTP result facts do not infer mutation disposition from storage causes.
//! Only closed enums cross this boundary; received prose and identifiers are never copied.
use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    OperationContext, ProjectFor, WireDetails, compose_explanation, project_failure,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use workspace_drive::failure::{DriveFailureDetails, DriveFailureVisibility};

use crate::{DriveApiError, DriveApiErrorCode, DriveFailureClassification};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveHttpOperation {
    Request,
    Root,
    Capabilities,
    Metadata,
    List,
    Search,
    ReadText,
    ReadChunk,
    Download,
    Mutate,
    Upload,
    RequestStatus,
    GrantCreate,
    GrantRevoke,
    GrantList,
}
impl DriveHttpOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Root => "root",
            Self::Capabilities => "capabilities",
            Self::Metadata => "metadata",
            Self::List => "list",
            Self::Search => "search",
            Self::ReadText => "read_text",
            Self::ReadChunk => "read_chunk",
            Self::Download => "download",
            Self::Mutate => "mutate",
            Self::Upload => "upload",
            Self::RequestStatus => "request_status",
            Self::GrantCreate => "grant_create",
            Self::GrantRevoke => "grant_revoke",
            Self::GrantList => "grant_list",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriveHttpFailureDetails {
    pub operation: DriveHttpOperation,
    pub code: DriveApiErrorCode,
    pub classification: DriveFailureClassification,
    /// Present only for a real storage-domain source after current cause visibility checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<DriveFailureDetails>,
}
impl DriveHttpFailureDetails {
    pub fn result(operation: DriveHttpOperation, result: &DriveApiError) -> Self {
        Self {
            operation,
            code: result.code,
            classification: result.classification,
            storage: None,
        }
    }
    fn reason(&self) -> &'static str {
        match self.code {
            DriveApiErrorCode::Denied => "denied",
            DriveApiErrorCode::NotFound => "not_found",
            DriveApiErrorCode::Conflict => "conflict",
            DriveApiErrorCode::Invalid => "invalid",
            DriveApiErrorCode::Limit => "limit",
            DriveApiErrorCode::StorageUnavailable => "storage_unavailable",
            DriveApiErrorCode::OutcomeUnknown => "outcome_unknown",
        }
    }
}
impl FailureCause for DriveHttpFailureDetails {
    fn identity(&self) -> FailureIdentity {
        use DriveApiErrorCode::*;
        FailureIdentity {
            domain: "drive_http",
            reason: self.reason(),
            category: match self.code {
                Denied => Category::Authorization,
                NotFound | Invalid | Limit => Category::InvalidInput,
                Conflict => Category::Conflict,
                StorageUnavailable | OutcomeUnknown => Category::Internal,
            },
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        if let Some(storage) = self.storage {
            storage.validate()?;
        }
        Ok(())
    }
}
impl WireDetails for DriveHttpFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "drive_http"
            && details_version == 1
            && matches!(
                reason,
                "denied"
                    | "not_found"
                    | "conflict"
                    | "invalid"
                    | "limit"
                    | "storage_unavailable"
                    | "outcome_unknown"
            )
    }
}

/// The boundary may disclose result facts without authorizing storage diagnostics.
/// Default denies both; a received payload is not evidence of current visibility.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriveHttpFailureVisibility {
    pub can_view_result: bool,
    pub can_view_storage_cause: bool,
}
impl DriveHttpFailureDetails {
    fn disclosure<A: error_contract::Audience>(
        &self,
        visibility: &DriveHttpFailureVisibility,
    ) -> Result<Disclosure<Self>, ContractError>
    where
        DriveFailureDetails:
            ProjectFor<A, Authorization = DriveFailureVisibility, Details = DriveFailureDetails>,
    {
        if !visibility.can_view_result {
            return Err(ContractError::ProjectionUnavailable);
        }
        let mut details = *self;
        let mut explanation = DriveApiError::new(self.code).message;
        if let Some(storage) = self.storage {
            if visibility.can_view_storage_cause {
                let facts = FailureFacts::new(storage)?;
                let projected = project_failure::<A, _>(
                    &facts,
                    &DriveFailureVisibility {
                        can_view_cause: true,
                    },
                )?;
                explanation = compose_explanation(projected.explanation(), &explanation)?;
            } else {
                details.storage = None;
            }
        }
        let mut disclosure = Disclosure::new(
            details,
            compose_explanation(
                &explanation,
                &format!("Drive {} could not complete", self.operation.as_str()),
            )?,
        );
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), "operation")?);
        Ok(disclosure)
    }
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for DriveHttpFailureDetails {
            type Authorization = DriveHttpFailureVisibility;
            type Details = Self;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &Self::Authorization)
                -> Result<Disclosure<Self>, ContractError>
            {
                self.disclosure::<$audience>(visibility)
            }
        }
    )+};
}
project_for!(
    Browser,
    error_contract::Tool,
    error_contract::InternalTransfer,
    error_contract::Storage,
    error_contract::Telemetry
);
