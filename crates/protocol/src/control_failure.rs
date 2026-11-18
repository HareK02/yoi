//! Runtime-control causes, independent of operation settlement and retry authority.
//!
//! Owners select a concrete variant before producing diagnostics. No constructor accepts
//! Display text, HTTP bodies, credentials, resource IDs, or arbitrary detail maps. This
//! deliberately small domain is not a replacement for Restore/Stop/command result types.
use error_contract::{Category, ContractError, FailureCause, FailureIdentity, WireDetails};
use serde::{Deserialize, Serialize};

// One table owns wire identity, category and explanation. Adding a reason does not require
// synchronizing a second decoder allowlist or audience-specific explanation templates.
macro_rules! control_causes {
    ($($variant:ident => ($reason:literal, $category:ident, $explanation:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
        #[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
        pub enum RuntimeControlCause { $($variant),+ }

        impl RuntimeControlCause {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub fn reason(self) -> &'static str {
                match self { $(Self::$variant => $reason),+ }
            }

            pub fn explanation(self) -> &'static str {
                match self { $(Self::$variant => $explanation),+ }
            }
        }

        impl FailureCause for RuntimeControlCause {
            fn identity(&self) -> FailureIdentity {
                FailureIdentity {
                    domain: "runtime_control",
                    reason: self.reason(),
                    category: match self { $(Self::$variant => Category::$category),+ },
                    details_version: 1,
                }
            }
            fn validate(&self) -> Result<(), ContractError> { Ok(()) }
        }
    };
}

control_causes! {
    RuntimeMissing => ("runtime_missing", Dependency, "The requested Runtime is not registered in this Workspace."),
    WorkerMissing => ("worker_missing", Dependency, "The Runtime did not find the requested Worker."),
    InvalidWorkerIdentifier => ("invalid_worker_identifier", InvalidInput, "The Worker identifier is invalid; lookup was not performed."),
    RuntimeStopped => ("runtime_stopped", Dependency, "The Runtime is stopped and cannot perform the requested operation."),
    ExecutionUnavailable => ("execution_unavailable", Dependency, "The Worker's execution endpoint is unavailable."),
    ExecutionFailed => ("execution_failed", Internal, "The Worker control operation failed; its retained domain result describes settlement and remaining work."),
    ExecutionRejected => ("execution_rejected", Conflict, "The execution owner rejected the operation; consult its operation result for settlement."),
    RestoreObservationStale => ("restore_observation_stale", Conflict, "The Restore observation is stale; the request was not accepted."),
    RestoreObservationConflict => ("restore_observation_conflict", Conflict, "The Restore observation conflicts with the current operation."),
    RestoreAdmissionBusy => ("restore_admission_busy", Conflict, "The Worker lifecycle is busy; Restore admission was not obtained."),
    StopObservationConflict => ("stop_observation_conflict", Conflict, "The Stop observation conflicts with the current lifecycle."),
    StopRequestConflict => ("stop_request_conflict", Conflict, "The Stop request identity conflicts with existing input or a pending Stop."),
    StopAdmissionBusy => ("stop_admission_busy", Conflict, "The Worker lifecycle is busy; Stop admission was not obtained."),
    CreateRequestConflict => ("create_request_conflict", Conflict, "The Worker create identity is already bound to different request input; reconcile the original request."),
    InvalidRequest => ("invalid_request", InvalidInput, "The Runtime request does not satisfy its input contract."),
    WorkspaceBindingMismatch => ("workspace_binding_mismatch", Authorization, "The Workspace binding does not match the Runtime owner."),
    ConfigurationUnavailable => ("configuration_unavailable", Dependency, "The required Runtime configuration is missing, invalid, or unsupported."),
    WorkingDirectoryFailed => ("working_directory_failed", Dependency, "The Runtime's working-directory operation failed."),
    PersistenceFailed => ("persistence_failed", Internal, "The Runtime could not persist the requested change."),
    PersistenceOutcomeUnknown => ("persistence_outcome_unknown", Unknown, "The Runtime could not confirm whether the storage change committed."),
    StoreUnavailable => ("store_unavailable", Internal, "The Runtime store is unavailable, missing required data, or corrupt."),
    RecordingSourceUnavailable => ("recording_source_unavailable", Dependency, "A required retained Journal source is unavailable; removal or release is not confirmed."),
    RecordingOwnershipUnknown => ("recording_ownership_unknown", Unknown, "The Runtime could not confirm ownership of a retained Journal source; removal or release is not confirmed."),
    RecordingExecutionUnconfirmed => ("recording_execution_unconfirmed", Unknown, "Required execution termination is unconfirmed; the Journal writer must be retained."),
    StateUnavailable => ("state_unavailable", Internal, "The Runtime state lock is unavailable."),
    AuthorizationFailed => ("authorization_failed", Authorization, "Authorization for the Runtime request failed."),
    ConnectionFailed => ("connection_failed", Dependency, "A connection to the Runtime could not be established."),
    TransportFailed => ("transport_failed", Unknown, "Runtime communication failed; no remote operation result was observed."),
    TimedOut => ("timed_out", Unknown, "The Runtime response timed out; no remote operation result was observed."),
    RequestEncodingFailed => ("request_encoding_failed", InvalidInput, "The Runtime request could not be encoded."),
    ClientSetupFailed => ("client_setup_failed", Internal, "The Runtime client could not be initialized; no request was issued."),
    ResponseTooLarge => ("response_too_large", Dependency, "The Runtime response exceeded the allowed size; its result could not be read."),
    InvalidResponse => ("invalid_response", Dependency, "The Runtime response did not satisfy the expected contract; its result could not be read."),
    ResultUnavailable => ("result_unavailable", Unknown, "The Worker control result could not be observed; no operation outcome is confirmed."),
    UnsupportedOperation => ("unsupported_operation", InvalidInput, "The Runtime does not support the requested Worker control operation."),
    RemoteRejected => ("remote_rejected", Dependency, "The Runtime returned a rejection without a supported control cause."),
}

impl WireDetails for RuntimeControlCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "runtime_control"
            && version == 1
            && Self::ALL.iter().any(|cause| cause.reason() == reason)
    }
}

// All audiences receive only the enum and its fixed explanation. Common context is NOT
// copied: even a caller-added operation_ref, diagnostic, cause link or correlation may
// contain private identifiers. Audience selection alone never grants disclosure rights.
// Resource-aware details, if added later, need an explicit current-visibility projector.
macro_rules! project_control {
    ($($audience:ty),+ $(,)?) => {$(
        impl error_contract::ProjectFor<$audience> for RuntimeControlCause {
            type Authorization = ();
            type Details = Self;
            fn disclose(
                &self,
                _: &error_contract::FailureFacts<Self>,
                _: &(),
            ) -> Result<error_contract::Disclosure<Self>, ContractError> {
                Ok(error_contract::Disclosure::new(*self, self.explanation()))
            }
        }
    )+};
}
project_control!(
    error_contract::Browser,
    error_contract::Tool,
    error_contract::InternalTransfer,
    error_contract::Storage,
    error_contract::Telemetry,
);

/// Evidence-bearing details within the common failure contract, not a new envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RuntimeControlFailure {
    pub cause: RuntimeControlCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<crate::WorkerCommandResult>,
}

impl FailureCause for RuntimeControlFailure {
    fn identity(&self) -> FailureIdentity {
        self.cause.identity()
    }
    fn validate(&self) -> Result<(), ContractError> {
        self.cause.validate()
    }
}
impl WireDetails for RuntimeControlFailure {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        RuntimeControlCause::supports(domain, reason, version)
    }
}
macro_rules! project_control_failure {
    ($($audience:ty),+ $(,)?) => {$(
        impl error_contract::ProjectFor<$audience> for RuntimeControlFailure {
            type Authorization = ();
            type Details = Self;
            fn disclose(&self, _: &error_contract::FailureFacts<Self>, _: &())
                -> Result<error_contract::Disclosure<Self>, ContractError> {
                Ok(error_contract::Disclosure::new(self.clone(), self.cause.explanation()))
            }
        }
    )+};
}
project_control_failure!(
    error_contract::Browser,
    error_contract::Tool,
    error_contract::InternalTransfer,
    error_contract::Storage,
    error_contract::Telemetry,
);

#[cfg(test)]
mod tests {
    use super::*;
    use error_contract::{Budget, DecodeFailure, FailureFacts, OperationContext, ProjectFor};

    fn roundtrip<A: error_contract::Audience>()
    where
        RuntimeControlCause: ProjectFor<A, Authorization = (), Details = RuntimeControlCause>,
    {
        for cause in RuntimeControlCause::ALL {
            let mut context = OperationContext::new("worker_lookup", "registry").unwrap();
            context.operation_ref = Some("private-worker-id".into());
            let mut facts = FailureFacts::new(*cause).unwrap();
            facts.with_context(context).unwrap();
            let projected = error_contract::project_failure::<A, _>(&facts, &()).unwrap();
            assert_eq!(projected.details(), cause);
            assert_eq!(projected.identity(), cause.identity());
            assert_eq!(projected.explanation(), cause.explanation());
            assert!(projected.context().operations.is_empty());
            let encoded =
                error_contract::encode_projected_failure(&projected, Budget::new(4096, 4096))
                    .unwrap();
            assert!(
                !std::str::from_utf8(encoded.as_bytes())
                    .unwrap()
                    .contains("private-worker-id")
            );
            match error_contract::decode_failure::<RuntimeControlCause>(
                encoded.as_bytes(),
                Budget::new(4096, 4096),
            ) {
                DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), cause),
                _ => panic!("known control cause did not round trip"),
            }
        }
    }

    #[test]
    fn all_control_causes_use_the_same_safe_contract_for_every_audience() {
        roundtrip::<error_contract::Browser>();
        roundtrip::<error_contract::Tool>();
        roundtrip::<error_contract::InternalTransfer>();
        roundtrip::<error_contract::Storage>();
        roundtrip::<error_contract::Telemetry>();
    }

    #[test]
    fn unsupported_reason_or_version_is_not_a_known_control_cause() {
        assert!(!RuntimeControlCause::supports(
            "runtime_control",
            "future_reason",
            1
        ));
        assert!(!RuntimeControlCause::supports(
            "runtime_control",
            "worker_missing",
            2
        ));
        assert!(serde_json::from_str::<RuntimeControlCause>("\"future_reason\"").is_err());
    }
    #[test]
    fn command_evidence_survives_tool_and_browser_failure_projection() {
        use crate::{
            WorkerCommandDelivery as D, WorkerCommandKind as K,
            WorkerCommandObservationFailure as F, WorkerCommandResult as R,
        };
        for (delivery, reason) in [
            (D::Enqueued, F::TimedOut),
            (D::Enqueued, F::Lagged),
            (D::Unknown, F::Closed),
            (D::Unknown, F::SendFailed),
        ] {
            let evidence = R::Unconfirmed {
                command: K::Compact,
                command_id: Some(42),
                delivery,
                reason,
            };
            let facts = FailureFacts::new(RuntimeControlFailure {
                cause: RuntimeControlCause::ResultUnavailable,
                command: Some(evidence.clone()),
            })
            .unwrap();
            let browser =
                error_contract::project_failure::<error_contract::Browser, _>(&facts, &()).unwrap();
            let tool =
                error_contract::project_failure::<error_contract::Tool, _>(&facts, &()).unwrap();
            let encoded =
                error_contract::encode_projected_failure(&tool, Budget::new(16384, 16384)).unwrap();
            match error_contract::decode_failure::<RuntimeControlFailure>(
                encoded.as_bytes(),
                Budget::new(16384, 16384),
            ) {
                DecodeFailure::Known(value) => {
                    assert_eq!(value.facts().cause().command, Some(evidence));
                    assert_eq!(value.facts().cause(), browser.details());
                }
                _ => panic!("command evidence lost"),
            }
        }
        assert!(serde_json::from_value::<R>(serde_json::json!({"kind":"completed"})).is_err());
    }
}
