//! Typed Worker config-client causes captured before the WIP/attachment boundary.
//!
//! The original WIP error remains local and unchanged; it is not Debug/Error and is
//! never formatted or serialized here. Cause/category are not commit or retry policy.
//! Visibility must be supplied from current authority on every projection, including
//! rereads. No source prose, resource, path, identity or received context is disclosed.
//! T814 owns shared result-envelope transport/persistence, not this cause supplier.
use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, ProjectFor, Storage, Telemetry, Tool, WireDetails,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::wip::WipOperationError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFailureOperation {
    Current,
    Attach,
    Observe,
    Read,
    Commit,
    Post,
}
impl ConfigFailureOperation {
    pub(super) fn from_suffix(suffix: &str) -> Self {
        match suffix {
            "" => Self::Attach,
            "/observe" => Self::Observe,
            "/read" => Self::Read,
            "/commit" => Self::Commit,
            _ => Self::Post,
        }
    }
    pub(super) fn authorization(self) -> &'static str {
        match self {
            Self::Current => "workspace_config.current",
            Self::Attach => "workspace_config.attach",
            Self::Observe => "workspace_config.observe",
            Self::Read => "workspace_config.read",
            Self::Commit => "workspace_config.commit",
            Self::Post => "workspace_config.post",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Current => "Workspace config current attachment lookup",
            Self::Attach => "Workspace config attachment",
            Self::Observe => "Workspace config tree observation",
            Self::Read => "Workspace config source read",
            Self::Commit => "Workspace config source commit",
            Self::Post => "Workspace config request",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFailureStage {
    Request,
    Response,
    Completion,
    Reconcile,
    Attach,
}

macro_rules! reasons {
    ($($variant:ident => ($name:literal, $category:ident, $explanation:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum ConfigFailureReason { $($variant),+ }
        impl ConfigFailureReason {
            fn name(self) -> &'static str { match self { $(Self::$variant => $name),+ } }
            fn category(self) -> Category { match self { $(Self::$variant => Category::$category),+ } }
            fn explanation(self) -> &'static str { match self { $(Self::$variant => $explanation),+ } }
        }
    };
}
reasons! {
    AuthorizationDenied => ("authorization_denied", Authorization, "the pre-dispatch Host authorization response confirmed access denial"),
    AuthorizationFailed => ("authorization_failed", Dependency, "the pre-dispatch authorization check failed without a confirmed denial"),
    InvalidRequest => ("invalid_request", InvalidInput, "the operation request is invalid"),
    Deadline => ("deadline", Dependency, "the local wait deadline elapsed; this does not establish whether effects committed"),
    TaskJoin => ("task_join", Internal, "the blocking transport task did not complete normally"),
    TransportUnavailable => ("transport_unavailable", Dependency, "the Workspace client transport was unavailable"),
    InvalidPath => ("invalid_path", InvalidInput, "the Workspace client rejected the request path"),
    Communication => ("communication", Dependency, "the Workspace client reported a communication failure"),
    Decode => ("decode", Internal, "the request or response could not be encoded or decoded at the recorded stage"),
    RequestSize => ("request_size", InvalidInput, "the encoded request exceeded the size limit"),
    ResponseSize => ("response_size", Dependency, "the response exceeded the size limit"),
    Identity => ("identity", Authorization, "the required Workspace or attachment identity was missing or inconsistent"),
    BackendRejected => ("backend_rejected", Dependency, "the Backend explicitly reported that the request was not committed"),
    BackendUnknown => ("backend_unknown", Unknown, "the Backend explicitly reported an unknown outcome"),
    UnrecognizedResponse => ("unrecognized_response", Dependency, "the Backend returned an unrecognized failure response"),
}

/// Closed allowlist, deliberately not an operation result or a recovery instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFailureDetails {
    pub operation: ConfigFailureOperation,
    pub stage: ConfigFailureStage,
    pub reason: ConfigFailureReason,
}
impl ConfigFailureDetails {
    fn disclosure(self) -> Disclosure<Self> {
        Disclosure::new(
            self,
            format!(
                "{} failed: {}",
                self.operation.description(),
                self.reason.explanation()
            ),
        )
    }
}
impl FailureCause for ConfigFailureDetails {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "workspace_config_client",
            reason: self.reason.name(),
            category: self.reason.category(),
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WireDetails for ConfigFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "workspace_config_client"
            && details_version == 1
            && serde_json::from_value::<ConfigFailureReason>(serde_json::Value::String(
                reason.into(),
            ))
            .is_ok()
    }
}

/// No shared WIP trait impls or string reconstruction are needed to retain the source.
pub struct ConfigFailure {
    details: ConfigFailureDetails,
    source: WipOperationError,
}
impl ConfigFailure {
    pub(super) fn new(
        operation: ConfigFailureOperation,
        stage: ConfigFailureStage,
        reason: ConfigFailureReason,
        source: WipOperationError,
    ) -> Self {
        Self {
            details: ConfigFailureDetails {
                operation,
                stage,
                reason,
            },
            source,
        }
    }
    pub fn details(&self) -> &ConfigFailureDetails {
        &self.details
    }
    pub fn source_error(&self) -> &WipOperationError {
        &self.source
    }
    pub fn into_source(self) -> WipOperationError {
        self.source
    }
}
impl std::fmt::Debug for ConfigFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigFailure")
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}
impl FailureCause for ConfigFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

/// A boundary's current cause-visibility decision, not an authorization capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigFailureVisibility {
    pub can_view_cause: bool,
}
macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for ConfigFailure {
            type Authorization = ConfigFailureVisibility;
            type Details = ConfigFailureDetails;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &Self::Authorization) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                Ok(self.details.disclosure())
            }
        }
        impl ProjectFor<$audience> for ConfigFailureDetails {
            type Authorization = ConfigFailureVisibility;
            type Details = Self;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &Self::Authorization) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                Ok((*self).disclosure())
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);

#[cfg(test)]
pub(super) mod tests;
