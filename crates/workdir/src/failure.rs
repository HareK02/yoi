//! Workdir-owned failure meaning and disclosure, independent of HTTP and Tool envelopes.
//!
//! The source error stays local. Only closed operation/classification/reason values
//! cross the codec. A current visibility decision is required even for received or
//! stored facts; an audience label is not authority to inspect a Workdir/provider.
//!
//! | Field | Authorized Browser / Tool / transfer / storage / telemetry |
//! | --- | --- |
//! | Operation, classification, observed denial/failure reason | Closed enum values |
//! | Explanation, stage | Local templates derived from those enums |
//! | Source text, paths, commands, URLs, credentials, received explanation/context | Never copied |
//!
//! Classification is retained separately from observed reason. In particular a
//! response-loss reason does not establish whether an operation was dispatched.
//! Audit receipts, cleanup results, request identity and reconciliation remain in
//! their existing domain results, not in this cause or a new retry state machine.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    WorkdirDenialReason, WorkdirError, WorkdirFailureReason,
    http::{WorkdirSessionOperation, WorkdirTransportError, WorkdirTransportErrorCode},
};

/// Only the operation kind is retained, never its arguments or target path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirFailureOperation {
    SessionOpen,
    SessionClose,
    AuthorizeScope,
    ScopeRulesOverlap,
    CheckoutSearch,
    CheckoutObserve,
    CheckoutExecute,
    Stat,
    Read,
    ReadBytes,
    Write,
    Edit,
    List,
    Glob,
    Grep,
    CommandStart,
    CommandStatus,
    CommandOutput,
    CommandCancel,
}

impl WorkdirFailureOperation {
    fn description(self) -> &'static str {
        match self {
            Self::SessionOpen => "session opening",
            Self::SessionClose => "session closing",
            Self::AuthorizeScope => "scope authorization",
            Self::ScopeRulesOverlap => "scope comparison",
            Self::CheckoutSearch => "checkout search",
            Self::CheckoutObserve => "checkout observation",
            Self::CheckoutExecute => "checkout update",
            Self::Stat => "entry inspection",
            Self::Read => "file read",
            Self::ReadBytes => "binary read",
            Self::Write => "file write",
            Self::Edit => "file edit",
            Self::List => "directory listing",
            Self::Glob => "path search",
            Self::Grep => "content search",
            Self::CommandStart => "command start",
            Self::CommandStatus => "command status lookup",
            Self::CommandOutput => "command output retrieval",
            Self::CommandCancel => "command cancellation",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionOpen => "session_open",
            Self::SessionClose => "session_close",
            Self::AuthorizeScope => "authorize_scope",
            Self::ScopeRulesOverlap => "scope_rules_overlap",
            Self::CheckoutSearch => "checkout_search",
            Self::CheckoutObserve => "checkout_observe",
            Self::CheckoutExecute => "checkout_execute",
            Self::Stat => "stat",
            Self::Read => "read",
            Self::ReadBytes => "read_bytes",
            Self::Write => "write",
            Self::Edit => "edit",
            Self::List => "list",
            Self::Glob => "glob",
            Self::Grep => "grep",
            Self::CommandStart => "command_start",
            Self::CommandStatus => "command_status",
            Self::CommandOutput => "command_output",
            Self::CommandCancel => "command_cancel",
        }
    }
}

impl From<&WorkdirSessionOperation> for WorkdirFailureOperation {
    fn from(operation: &WorkdirSessionOperation) -> Self {
        match operation {
            WorkdirSessionOperation::AuthorizeScope(_) => Self::AuthorizeScope,
            WorkdirSessionOperation::ScopeRulesOverlap(_) => Self::ScopeRulesOverlap,
            WorkdirSessionOperation::CheckoutSearch(_) => Self::CheckoutSearch,
            WorkdirSessionOperation::CheckoutObserve(_) => Self::CheckoutObserve,
            WorkdirSessionOperation::CheckoutExecute(_) => Self::CheckoutExecute,
            WorkdirSessionOperation::Stat(_) => Self::Stat,
            WorkdirSessionOperation::Read(_) => Self::Read,
            WorkdirSessionOperation::ReadBytes(_) => Self::ReadBytes,
            WorkdirSessionOperation::Write(_) => Self::Write,
            WorkdirSessionOperation::Edit(_) => Self::Edit,
            WorkdirSessionOperation::List(_) => Self::List,
            WorkdirSessionOperation::Glob(_) => Self::Glob,
            WorkdirSessionOperation::Grep(_) => Self::Grep,
            WorkdirSessionOperation::CommandStart(_) => Self::CommandStart,
            WorkdirSessionOperation::CommandStatus(_) => Self::CommandStatus,
            WorkdirSessionOperation::CommandOutput(_) => Self::CommandOutput,
            WorkdirSessionOperation::CommandCancel(_) => Self::CommandCancel,
        }
    }
}

/// Allowlisted observations, not a second operation result. Missing reasons stay
/// absent, including when an older provider supplied only a classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkdirFailureDetails {
    pub operation: WorkdirFailureOperation,
    pub classification: WorkdirTransportErrorCode,
    pub denial_reason: Option<WorkdirDenialReason>,
    pub failure_reason: Option<WorkdirFailureReason>,
}

impl WorkdirFailureDetails {
    fn reason(&self) -> &'static str {
        self.failure_reason
            .map(WorkdirFailureReason::as_str)
            .or_else(|| self.denial_reason.map(WorkdirDenialReason::as_str))
            .unwrap_or_else(|| self.classification.as_str())
    }

    fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let cause = match (self.failure_reason, self.denial_reason) {
            (Some(reason), _) => failure_explanation(reason),
            (None, Some(reason)) => denial_explanation(reason),
            (None, None) => classification_explanation(self.classification),
        };
        let impact = match self.classification {
            WorkdirTransportErrorCode::OutcomeUnknown | WorkdirTransportErrorCode::Transport => {
                format!(
                    "Workdir {} result could not be confirmed",
                    self.operation.description()
                )
            }
            WorkdirTransportErrorCode::NotDispatched => {
                format!(
                    "Workdir {} was not dispatched",
                    self.operation.description()
                )
            }
            _ => format!(
                "Workdir {} could not complete",
                self.operation.description()
            ),
        };
        let mut disclosure = Disclosure::new(*self, compose_explanation(cause, &impact)?);
        let stage = self
            .failure_reason
            .map(|reason| reason.stage().as_str())
            .unwrap_or("operation");
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), stage)?);
        Ok(disclosure)
    }
}

impl FailureCause for WorkdirFailureDetails {
    fn identity(&self) -> FailureIdentity {
        use WorkdirTransportErrorCode as Code;
        let category = match self.classification {
            Code::Denied | Code::OutOfScope | Code::SymlinkOutOfScope | Code::ReadOnly => {
                Category::Authorization
            }
            Code::Conflict => Category::Conflict,
            Code::OutcomeUnknown => Category::Unknown,
            Code::Unavailable | Code::Transport | Code::NotDispatched => Category::Dependency,
            Code::Io | Code::Internal => Category::Internal,
            Code::NotFound
            | Code::Unsupported
            | Code::InvalidRequest
            | Code::BrokenSymlink
            | Code::SymlinkTargetIsDirectory
            | Code::IsDirectory
            | Code::SymlinkDirectoryNotTraversed
            | Code::UnknownCommand => Category::InvalidInput,
        };
        FailureIdentity {
            domain: "workdir",
            reason: self.reason(),
            category,
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        // Fields are closed enums. Diagnostic wrappers deliberately do not
        // constrain the original classification or assert a dispatch outcome.
        Ok(())
    }
}

impl WireDetails for WorkdirFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        if domain != "workdir" || details_version != 1 {
            return false;
        }
        // Reuse the owning enums' wire vocabulary rather than maintaining a
        // second reason registry. This parses a reason identifier, never prose.
        let reason = serde_json::Value::String(reason.to_owned());
        serde_json::from_value::<WorkdirFailureReason>(reason.clone()).is_ok()
            || serde_json::from_value::<WorkdirDenialReason>(reason.clone()).is_ok()
            || serde_json::from_value::<WorkdirTransportErrorCode>(reason).is_ok()
    }
}

/// Owns the original source chain; it is intentionally not serializable.
#[derive(Debug, thiserror::Error)]
#[error("Workdir {operation:?} failed", operation = .details.operation)]
pub struct WorkdirFailure {
    details: WorkdirFailureDetails,
    #[source]
    source: WorkdirError,
}

impl WorkdirFailure {
    pub fn new(operation: WorkdirFailureOperation, source: WorkdirError) -> Self {
        let transport = WorkdirTransportError::from_workdir_error(&source);
        Self {
            details: WorkdirFailureDetails {
                operation,
                classification: transport.code,
                denial_reason: transport.denial_reason,
                failure_reason: transport.failure_reason,
            },
            source,
        }
    }

    /// Retain absence of legacy provider reasons before reconstructing the
    /// local error. Provider-authored messages are never used for projection.
    pub fn from_transport_error(
        operation: WorkdirFailureOperation,
        transport: WorkdirTransportError,
    ) -> Self {
        Self {
            details: WorkdirFailureDetails {
                operation,
                classification: transport.code,
                denial_reason: transport.denial_reason,
                failure_reason: transport.failure_reason,
            },
            source: transport.into_workdir_error(),
        }
    }

    pub fn details(&self) -> &WorkdirFailureDetails {
        &self.details
    }
}

impl FailureCause for WorkdirFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

/// Supplied by the current authenticated boundary after checking visibility of
/// the Workdir and its failure diagnostics. It is not a serialized capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkdirFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for WorkdirFailureDetails {
            type Authorization = WorkdirFailureVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &WorkdirFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                // Do not copy received explanation, arbitrary context or
                // correlations. Only closed local facts can be redisclosed.
                self.disclosure()
            }
        }
        impl ProjectFor<$audience> for WorkdirFailure {
            type Authorization = WorkdirFailureVisibility;
            type Details = WorkdirFailureDetails;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &WorkdirFailureVisibility,
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

fn failure_explanation(reason: WorkdirFailureReason) -> &'static str {
    use WorkdirFailureReason as Reason;
    match reason {
        Reason::SessionClosed => "the Workdir session is closed",
        Reason::ProviderUnavailable => "the Workdir provider is unavailable",
        Reason::ProviderGrantExpired => "the Workdir provider grant expired",
        Reason::ProviderAdmissionClosed => "the Workdir provider is no longer accepting operations",
        Reason::ProviderDispatchRejected => "the Workdir provider rejected dispatch",
        Reason::ProviderResponseLost => "the Workdir provider response was lost",
        Reason::ProviderOperationTimeout => {
            "the Workdir provider did not respond before the operation deadline"
        }
        Reason::CommandTerminalMissing => "the command ended without a terminal result",
        Reason::AuditStoreUnavailable => "Workdir audit persistence is unavailable",
        Reason::HttpTimeout => "the Workdir transport deadline expired",
        Reason::HttpConnect => "the Workdir transport connection failed",
        Reason::HttpTransfer => "the Workdir response transfer failed",
        Reason::HttpResponseInvalid => "the Workdir provider returned an invalid response",
        Reason::HttpResponseTooLarge => "the Workdir provider response exceeded its size limit",
        Reason::OperationResultMismatch => {
            "the Workdir response did not match the requested operation"
        }
        Reason::SessionIdentityMismatch => {
            "the opened Workdir session did not match the requested identity"
        }
        Reason::SessionOpenTaskFailed => "the Workdir session opening task failed",
        Reason::SessionOpenFailed => {
            "the Workdir session could not be opened; no more specific cause was reported"
        }
        Reason::AuthorizationMissing => "Workdir authorization is missing",
        Reason::AuthorizationStale => "Workdir authorization is no longer current",
        Reason::AuthorizationUnavailable => "Workdir authorization could not be prepared",
        Reason::SigningIdentityUnavailable => "the Workdir request signing identity is unavailable",
        Reason::AuthorizationSignFailed => "the Workdir request could not be signed",
        Reason::RuntimeStopped => "the Runtime is stopped",
        Reason::RuntimeStateUnavailable => "the Runtime state is unavailable",
        Reason::RuntimeBackendUnavailable => "the Runtime Workdir backend is unavailable",
        Reason::RuntimeOwnerMismatch => "the Runtime owner binding does not match",
        Reason::RuntimeInvalidRequest => "the Runtime rejected the Workdir request as invalid",
        Reason::WorkingDirectoryNotFound => "the Workdir was not found",
        Reason::WorkingDirectoryNotActive => "the Workdir is not active",
        Reason::WorkingDirectoryRecordInvalid => "the stored Workdir record is invalid",
        Reason::WorkingDirectoryAccessUnavailable => "access to the Workdir is unavailable",
    }
}

fn denial_explanation(reason: WorkdirDenialReason) -> &'static str {
    use WorkdirDenialReason as Reason;
    match reason {
        Reason::ScopeResolutionUnavailable => "the Workdir scope could not be resolved",
        Reason::ScopeComparisonUnavailable => "the Workdir scope could not be compared",
        Reason::ScopedCapabilityDenied => {
            "the scoped Workdir session does not permit this capability"
        }
        Reason::LogicalScopeExceeded => "the operation exceeds the logical Workdir scope",
        Reason::InvalidScopedPath => "the scoped Workdir path is invalid",
        Reason::ChildWriteLeaseConflict => "a delegated child holds a conflicting write lease",
        Reason::EmptyScope => "the delegated Workdir scope is empty",
        Reason::ParentCapabilityDenied => {
            "the parent Workdir session does not permit this capability"
        }
        Reason::CommandRequiresWritableScope => "commands require a writable delegated scope",
        Reason::ParentScopeExceeded => "the requested scope exceeds the parent Workdir scope",
        Reason::CwdOutsideReadableScope => {
            "the requested working directory is outside readable scope"
        }
        Reason::ReadOnlySession => "the Workdir session is read-only",
        Reason::ExternalRootSymlink => "the external Workdir root is a symbolic link",
        Reason::ExternalRootChanged => "the external Workdir root changed",
        Reason::ExternalRootMoved => "the external Workdir root moved",
        Reason::ExternalScopeSymlink => {
            "the external Workdir scope crosses a disallowed symbolic link"
        }
        Reason::AttachmentScopeExceeded => "the operation exceeds the Workdir attachment scope",
        Reason::DelegatedScopeExceeded => "the operation exceeds the delegated Workdir scope",
        Reason::CheckoutOutputRootExceeded => "the checkout output exceeds its permitted root",
        Reason::ProviderRootExceeded => "the operation exceeds the provider root",
        Reason::ProviderSymlinkDenied => "the provider does not permit this symbolic link",
        Reason::CheckoutTargetDenied => "the checkout target is not permitted",
        Reason::CheckoutCreateParentDenied => "the checkout parent does not permit creation",
        Reason::CheckoutSearchPathDenied => "the checkout search path is not permitted",
        Reason::CheckoutEnumerationDenied => "the checkout path cannot be enumerated in this scope",
        Reason::PathOutOfScope => "the requested path is outside Workdir scope",
        Reason::SymlinkTargetOutOfScope => "the symbolic link target is outside Workdir scope",
        Reason::PathReadOnly => "the requested Workdir path is read-only",
        Reason::OsPermissionDenied => "the operating system denied access",
        Reason::UnclassifiedPermissionDenied => {
            "access was denied; no more specific origin was reported"
        }
    }
}

fn classification_explanation(code: WorkdirTransportErrorCode) -> &'static str {
    use WorkdirTransportErrorCode as Code;
    match code {
        Code::NotFound => "the requested Workdir path was not found",
        Code::Conflict => "the Workdir target changed since it was observed",
        Code::OutcomeUnknown => "the operation outcome is unknown; effects may have occurred",
        Code::NotDispatched => "the operation was not dispatched",
        Code::Unsupported => "the Workdir session does not support this operation",
        Code::InvalidRequest => "the Workdir request is invalid",
        Code::Denied => "the Workdir operation was denied; no specific reason was reported",
        Code::OutOfScope => "the requested path is outside Workdir scope",
        Code::SymlinkOutOfScope => "the symbolic link target is outside Workdir scope",
        Code::BrokenSymlink => "the symbolic link target does not exist",
        Code::SymlinkTargetIsDirectory => "the symbolic link target is a directory",
        Code::ReadOnly => "the Workdir path is read-only",
        Code::IsDirectory => "the requested file is a directory",
        Code::SymlinkDirectoryNotTraversed => {
            "the operation does not follow symbolic link directories"
        }
        Code::UnknownCommand => "the requested Workdir command was not found",
        Code::Unavailable => "the Workdir session is unavailable; no specific reason was reported",
        Code::Io => "the Workdir input/output operation failed",
        Code::Transport => "the Workdir transport failed; the remote result was not confirmed",
        Code::Internal => "the Workdir operation failed; no specific reason was reported",
    }
}

#[cfg(test)]
mod tests;
