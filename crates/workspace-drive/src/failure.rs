//! Drive storage-domain cause adapter, independent of HTTP, Tool and mutation outcomes.
//!
//! `DriveFailure` retains the actual [`crate::Error`] and its source chain locally.
//! Only closed operation/reason values cross the shared `error-contract` codec.
//! The boundary must check **current** visibility of the Workspace Drive and its
//! failure diagnostics for every audience, including rereads of stored facts.
//!
//! | Information | Disclosure after current visibility check |
//! | --- | --- |
//! | Operation kind and observed top-level error variant | Closed enums below |
//! | Explanation and operation context | Locally generated static templates |
//! | Invalid-request prose, SQL, blob/storage subclasses, physical identity | Never copied |
//! | Paths, names, contents, credentials, actor/request/node IDs | Never copied |
//! | Received explanation, arbitrary context and correlations | Never forwarded |
//!
//! `Invalid` is used for both request and stored-data validity constraints; it
//! does not establish which field was invalid. `Conflict` similarly does not
//! distinguish CAS, name, nonempty-folder or request-identity conflicts. Do not
//! infer a more specific reason by parsing Display or inspecting an error string.
//! Internal storage subclasses remain available through the local source only.
//!
//! This adapter has no status, retry, dispatch, success or Unknown inference.
//! Mutation identity, CAS, receipts and reconciliation remain authoritative in
//! `Drive::mutate`, `Drive::request_status` and the existing caller domain results.
//! In particular these causes do not prove whether a remote mutation committed.
//! Server/Worker adapter causes and their shared boundary integration are separate.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Mutation};

/// Operation kind only: no arguments, physical identity or request identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveFailureOperation {
    Open,
    AttachWorkspaceAuthority,
    CheckAuthorized,
    Root,
    Metadata,
    List,
    RequestStatus,
    CreateFolder,
    CreateFile,
    Update,
    Relocate,
    Delete,
    Read,
    ReadText,
    Search,
    Collect,
    Fence,
    Purge,
}

impl DriveFailureOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::AttachWorkspaceAuthority => "attach_workspace_authority",
            Self::CheckAuthorized => "check_authorized",
            Self::Root => "root",
            Self::Metadata => "metadata",
            Self::List => "list",
            Self::RequestStatus => "request_status",
            Self::CreateFolder => "create_folder",
            Self::CreateFile => "create_file",
            Self::Update => "update",
            Self::Relocate => "relocate",
            Self::Delete => "delete",
            Self::Read => "read",
            Self::ReadText => "read_text",
            Self::Search => "search",
            Self::Collect => "collect",
            Self::Fence => "fence",
            Self::Purge => "purge",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Open => "storage opening",
            Self::AttachWorkspaceAuthority => "Workspace authority attachment",
            Self::CheckAuthorized => "authorization check",
            Self::Root => "root lookup",
            Self::Metadata => "node metadata lookup",
            Self::List => "directory listing",
            Self::RequestStatus => "mutation receipt lookup",
            Self::CreateFolder => "folder creation",
            Self::CreateFile => "file creation",
            Self::Update => "file update",
            Self::Relocate => "node relocation",
            Self::Delete => "node deletion",
            Self::Read => "binary read",
            Self::ReadText => "text read",
            Self::Search => "content search",
            Self::Collect => "unreferenced blob collection",
            Self::Fence => "deletion fencing",
            Self::Purge => "storage purge",
        }
    }
}

impl From<&Mutation> for DriveFailureOperation {
    fn from(mutation: &Mutation) -> Self {
        match mutation {
            Mutation::CreateFolder { .. } => Self::CreateFolder,
            Mutation::CreateFile { .. } => Self::CreateFile,
            Mutation::Update { .. } => Self::Update,
            Mutation::Relocate { .. } => Self::Relocate,
            Mutation::Delete { .. } => Self::Delete,
        }
    }
}

/// Safe meanings observed from the actual top-level storage error variants.
/// These are not transport failure reasons or an operation disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriveFailureReason {
    Invalid,
    NotFound,
    Conflict,
    Denied,
    Fenced,
    StorageFailure,
    MetadataFailure,
    BlobFailure,
    EncodingFailure,
}

impl DriveFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Denied => "denied",
            Self::Fenced => "fenced",
            Self::StorageFailure => "storage_failure",
            Self::MetadataFailure => "metadata_failure",
            Self::BlobFailure => "blob_failure",
            Self::EncodingFailure => "encoding_failure",
        }
    }

    fn category(self) -> Category {
        match self {
            Self::Invalid | Self::NotFound => Category::InvalidInput,
            Self::Conflict | Self::Fenced => Category::Conflict,
            Self::Denied => Category::Authorization,
            Self::StorageFailure
            | Self::MetadataFailure
            | Self::BlobFailure
            | Self::EncodingFailure => Category::Internal,
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Self::Invalid => "a Drive request or stored-data validity constraint was not satisfied",
            Self::NotFound => "the Drive node no longer exists",
            Self::Conflict => {
                "the last committed Drive mutation, name, folder contents or request identity conflicts with current state"
            }
            Self::Denied => "current authorization does not permit this Drive access",
            Self::Fenced => "the Workspace Drive is fenced for deletion",
            Self::StorageFailure => "the Drive feature-storage boundary reported a failure",
            Self::MetadataFailure => "the Drive metadata database reported a failure",
            Self::BlobFailure => "the Drive blob store reported a failure",
            Self::EncodingFailure => "the Drive result could not be encoded or decoded",
        }
    }
}

impl From<&Error> for DriveFailureReason {
    fn from(source: &Error) -> Self {
        // Deliberately exhaustive. New variants require a disclosure decision;
        // source Display and nested storage diagnostics are not a wire protocol.
        match source {
            Error::Invalid(_) => Self::Invalid,
            Error::NotFound => Self::NotFound,
            Error::Conflict => Self::Conflict,
            Error::Denied => Self::Denied,
            Error::Fenced => Self::Fenced,
            Error::Storage(_) => Self::StorageFailure,
            Error::Sqlite(_) => Self::MetadataFailure,
            Error::Blob(_) => Self::BlobFailure,
            Error::Encoding(_) => Self::EncodingFailure,
        }
    }
}

/// The entire wire allowlist. No opaque diagnostic or arbitrary JSON fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DriveFailureDetails {
    pub operation: DriveFailureOperation,
    pub reason: DriveFailureReason,
}

impl DriveFailureDetails {
    fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let impact = format!("Drive {} reported a failure", self.operation.description());
        let mut disclosure = Disclosure::new(
            *self,
            compose_explanation(self.reason.explanation(), &impact)?,
        );
        // Rebuild, rather than copying context from received/stored facts.
        // The operation kind does not establish a transaction/dispatch stage.
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), "operation")?);
        Ok(disclosure)
    }
}

impl FailureCause for DriveFailureDetails {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "drive",
            reason: self.reason.as_str(),
            category: self.reason.category(),
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        // Closed enums only. No assertion about operation disposition is made.
        Ok(())
    }
}

impl WireDetails for DriveFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "drive"
            && details_version == 1
            && serde_json::from_value::<DriveFailureReason>(serde_json::Value::String(
                reason.to_owned(),
            ))
            .is_ok()
    }
}

/// Local source owner, intentionally not serializable. Safe wire facts are
/// obtained only by projecting with a current visibility decision.
#[derive(Debug, thiserror::Error)]
#[error("Drive {operation:?} reported a failure", operation = .details.operation)]
pub struct DriveFailure {
    details: DriveFailureDetails,
    #[source]
    source: Error,
}

impl DriveFailure {
    pub fn new(operation: DriveFailureOperation, source: Error) -> Self {
        Self {
            details: DriveFailureDetails {
                operation,
                reason: DriveFailureReason::from(&source),
            },
            source,
        }
    }

    pub fn details(&self) -> &DriveFailureDetails {
        &self.details
    }

    /// The original typed source is local diagnostic evidence, not public prose.
    pub fn source_error(&self) -> &Error {
        &self.source
    }

    /// Recover the original domain error without changing its semantics.
    pub fn into_source(self) -> Error {
        self.source
    }
}

impl FailureCause for DriveFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

/// Supplied by the authenticated boundary after checking current Drive and
/// failure-diagnostic visibility. Not a serialized capability or cached grant.
/// Default is deny for *all* audiences, including storage and telemetry.
#[derive(Clone, Copy, Debug, Default)]
pub struct DriveFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for DriveFailureDetails {
            type Authorization = DriveFailureVisibility;
            type Details = Self;

            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &DriveFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.disclosure()
            }
        }

        impl ProjectFor<$audience> for DriveFailure {
            type Authorization = DriveFailureVisibility;
            type Details = DriveFailureDetails;

            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &DriveFailureVisibility,
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
