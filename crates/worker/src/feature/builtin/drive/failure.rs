//! Worker Drive-client cause projection, not mutation disposition or recovery policy.
//!
//! This domain is `workspace_drive_client`, distinct from storage's `drive` domain
//! and source/schema. The actual [`DriveError`] remains the local source, including
//! authorization's `denied` flag and an unknown outcome's original request identity.
//! Its Display is **not** a public formatter and is never parsed by this adapter.
//!
//! | Information | Disclosure after a current visibility check |
//! | --- | --- |
//! | Attempted operation and observed typed variant | Closed enums below |
//! | Explanation and operation context | Locally regenerated templates |
//! | Original prose, permission resource, paths, provider/status bodies | Never copied |
//! | Request identity, received context/prose/correlations | Never copied |
//!
//! `Unavailable` and `OutcomeUnknown` contain no more detailed typed cause. The
//! diagnostic sink may distinguish failures, but logs/strings/status are not inputs
//! to this mapping. `Invalid` and `Conflict` also do not identify a specific field
//! or conflict subtype. Do not manufacture such distinctions from their Display.
//!
//! Category/reason/context do not establish dispatch, commitment, retry safety or
//! reconciliation results. Recover the original error with `source_error` or
//! `into_source` and follow the existing caller result and original-request receipt
//! protocol. This adapter does not authorize, call, retry or reconcile an operation.
//! Shared Tool/Job/Service adapters belong to T-814; domain caller connections live here.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use server_api::DriveMutation;

use super::backend::DriveError;

/// Caller-selected operation kind only; no arguments, resource or request identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveClientFailureOperation {
    Workspace,
    Capabilities,
    Root,
    Metadata,
    List,
    Search,
    Read,
    Bytes,
    CreateFolder,
    CreateText,
    UpdateText,
    Relocate,
    Delete,
    Upload,
    RequestStatus,
}

impl DriveClientFailureOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Capabilities => "capabilities",
            Self::Root => "root",
            Self::Metadata => "metadata",
            Self::List => "list",
            Self::Search => "search",
            Self::Read => "read",
            Self::Bytes => "bytes",
            Self::CreateFolder => "create_folder",
            Self::CreateText => "create_text",
            Self::UpdateText => "update_text",
            Self::Relocate => "relocate",
            Self::Delete => "delete",
            Self::Upload => "upload",
            Self::RequestStatus => "request_status",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace access check",
            Self::Capabilities => "capability lookup",
            Self::Root => "root lookup",
            Self::Metadata => "entry metadata lookup",
            Self::List => "directory listing",
            Self::Search => "content search",
            Self::Read => "text read",
            Self::Bytes => "binary read",
            Self::CreateFolder => "folder creation",
            Self::CreateText => "text file creation",
            Self::UpdateText => "text file update",
            Self::Relocate => "entry relocation",
            Self::Delete => "entry deletion",
            Self::Upload => "file upload",
            Self::RequestStatus => "mutation receipt lookup",
        }
    }
}

impl From<&DriveMutation> for DriveClientFailureOperation {
    fn from(mutation: &DriveMutation) -> Self {
        match mutation {
            DriveMutation::CreateFolder { .. } => Self::CreateFolder,
            DriveMutation::CreateText { .. } => Self::CreateText,
            DriveMutation::UpdateText { .. } => Self::UpdateText,
            DriveMutation::Relocate { .. } => Self::Relocate,
            DriveMutation::Delete { .. } => Self::Delete,
        }
    }
}

/// Observed existing variant (and authorization's existing denial flag), not a
/// second result state machine. No transport/diagnostic subreason is inferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveClientFailureReason {
    AuthorizationDenied,
    AuthorizationFailed,
    Denied,
    NotFound,
    Conflict,
    Invalid,
    Limit,
    Unavailable,
    OutcomeUnknown,
}

impl DriveClientFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AuthorizationDenied => "authorization_denied",
            Self::AuthorizationFailed => "authorization_failed",
            Self::Denied => "denied",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
            Self::Limit => "limit",
            Self::Unavailable => "unavailable",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    fn category(self) -> Category {
        match self {
            Self::AuthorizationDenied | Self::AuthorizationFailed | Self::Denied => {
                Category::Authorization
            }
            Self::NotFound | Self::Invalid | Self::Limit => Category::InvalidInput,
            Self::Conflict => Category::Conflict,
            Self::Unavailable => Category::Dependency,
            Self::OutcomeUnknown => Category::Unknown,
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Self::AuthorizationDenied => {
                "the client's operation authorization check reported a denial"
            }
            Self::AuthorizationFailed => {
                "the client's operation authorization check failed without a confirmed denial"
            }
            Self::Denied => "the Drive client reported that access was denied",
            Self::NotFound => "the Drive client reported that the requested resource was not found",
            Self::Conflict => {
                "the Drive client reported a content, location or name conflict; no more specific typed cause was retained"
            }
            Self::Invalid => {
                "the Drive client reported invalid arguments; no more specific typed cause was retained"
            }
            Self::Limit => {
                "the Drive client reported that a request or response limit was exceeded"
            }
            Self::Unavailable => {
                "the Drive client reported storage or transport unavailable; no more specific typed cause was retained"
            }
            Self::OutcomeUnknown => {
                "the Drive client reported an unknown mutation outcome; the existing original-request receipt protocol remains authoritative"
            }
        }
    }
}

impl From<&DriveError> for DriveClientFailureReason {
    fn from(source: &DriveError) -> Self {
        // Exhaustive so a new variant needs an explicit disclosure decision.
        // Never inspect source prose, permission resources or diagnostic logs.
        match source {
            DriveError::Authorization { denied: true, .. } => Self::AuthorizationDenied,
            DriveError::Authorization { denied: false, .. } => Self::AuthorizationFailed,
            DriveError::Denied => Self::Denied,
            DriveError::NotFound => Self::NotFound,
            DriveError::Conflict => Self::Conflict,
            DriveError::Invalid(_) => Self::Invalid,
            DriveError::Limit => Self::Limit,
            DriveError::Unavailable => Self::Unavailable,
            DriveError::OutcomeUnknown(_) => Self::OutcomeUnknown,
            DriveError::Remote { result, .. } => Self::from(result.as_ref()),
        }
    }
}

/// Entire wire allowlist. No arbitrary JSON, prose or operation outcome fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriveClientFailureDetails {
    pub operation: DriveClientFailureOperation,
    pub reason: DriveClientFailureReason,
    /// Closed received facts only; reprojected for the current audience, never prose/context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<server_api::DriveHttpFailureDetails>,
}

impl DriveClientFailureDetails {
    fn disclosure<A: error_contract::Audience>(&self) -> Result<Disclosure<Self>, ContractError>
    where
        server_api::DriveHttpFailureDetails: ProjectFor<
                A,
                Authorization = server_api::DriveHttpFailureVisibility,
                Details = server_api::DriveHttpFailureDetails,
            >,
    {
        let impact = format!(
            "Drive client {} reported a failure",
            self.operation.description()
        );
        let cause = match self.server {
            Some(server) => error_contract::project_failure::<A, _>(
                &FailureFacts::new(server)?,
                &server_api::DriveHttpFailureVisibility {
                    can_view_result: true,
                    can_view_storage_cause: true,
                },
            )?
            .explanation()
            .to_owned(),
            None => self.reason.explanation().to_owned(),
        };
        let mut disclosure = Disclosure::new(*self, compose_explanation(&cause, &impact)?);
        // Rebuilt from the closed operation; this is not a dispatch/commit stage.
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), "operation")?);
        Ok(disclosure)
    }
}

impl FailureCause for DriveClientFailureDetails {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "workspace_drive_client",
            reason: self.reason.as_str(),
            category: self.reason.category(),
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        if let Some(server) = self.server {
            server.validate()?;
        }
        // The cause and the caller's result are independent observations.
        Ok(())
    }
}

impl WireDetails for DriveClientFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "workspace_drive_client"
            && details_version == 1
            && serde_json::from_value::<DriveClientFailureReason>(serde_json::Value::String(
                reason.to_owned(),
            ))
            .is_ok()
    }
}

/// Local source owner, deliberately not serializable. Project with current
/// visibility to obtain encodable facts; never use the original source Display.
#[derive(Debug, thiserror::Error)]
#[error("Drive client {operation:?} reported a failure", operation = .details.operation)]
pub struct DriveClientFailure {
    details: DriveClientFailureDetails,
    #[source]
    source: DriveError,
}

impl DriveClientFailure {
    pub fn new(operation: DriveClientFailureOperation, source: DriveError) -> Self {
        Self {
            details: DriveClientFailureDetails {
                operation,
                reason: DriveClientFailureReason::from(&source),
                server: source.remote_details(),
            },
            source,
        }
    }

    pub fn details(&self) -> &DriveClientFailureDetails {
        &self.details
    }

    /// Original typed evidence, including `denied` and unknown request identity.
    pub fn source_error(&self) -> &DriveError {
        &self.source
    }

    /// Recover the unchanged caller error, not a reconstructed error from wire facts.
    pub fn into_source(self) -> DriveError {
        self.source
    }
}

impl FailureCause for DriveClientFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

/// Boundary-supplied current Drive/resource AND diagnostic visibility, including
/// on every reread. The registered Tool may disclose its own current attempt
/// through its explicit enum-only public-error policy, not another resource
/// observation or a stored error. Not a cached grant or serialized capability. All audiences
/// default to denial, including Storage and Telemetry.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriveClientFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for DriveClientFailureDetails {
            type Authorization = DriveClientFailureVisibility;
            type Details = Self;

            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &DriveClientFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.disclosure::<$audience>()
            }
        }

        impl ProjectFor<$audience> for DriveClientFailure {
            type Authorization = DriveClientFailureVisibility;
            type Details = DriveClientFailureDetails;

            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &DriveClientFailureVisibility,
            ) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.details.disclosure::<$audience>()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);

#[cfg(test)]
mod tests;
