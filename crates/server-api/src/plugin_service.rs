//! Host-owned Ticket Service delivery. Lease proofs fence a driver, not authorize it.
//! All requests still require the authenticated Worker and current retained plan.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

impl api_macros::openapi::OpenApiSchema for PluginServiceRecoveryRequest {}
impl api_macros::openapi::OpenApiSchema for PluginServiceDiagnostics {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginServiceLease {
    pub consumer_id: String,
    pub lease_token: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginTicketEventType {
    Changed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginTicketEventEnvelope {
    pub event_id: String,
    pub event_type: PluginTicketEventType,
    pub version: u32,
    pub occurred_at: String,
    pub ticket_id: String,
    pub domain_event_ref: String,
    pub content_digest: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceEventResult {
    Ack,
    RetryAfter {
        delay_ms: u32,
    },
    Reject,
    /// Host-observed unknown effect; never automatically redelivered.
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginServicePauseReason {
    Rejected,
    AttemptsExhausted,
    QueueFull,
    RetentionGap,
    Indeterminate,
    Authorization,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginServiceDiagnostics {
    pub consumer_id: String,
    pub pause_reason: Option<PluginServicePauseReason>,
    pub queued: u32,
    pub attempt: u32,
    pub current_event_id: Option<String>,
    pub lease_active: bool,
    /// Last owner-committed invocation observation, not a retry or recovery grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<PluginServiceOutcomeDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginServiceLifecycleOperation {
    Admit,
    Publish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceRequest {
    /// Live preflight for a reserved or running Worker. Does not create consumer
    /// state, acquire a lease, or admit event effects.
    Prepare {
        plan_id: String,
        installation_id: String,
        service_name: String,
        operation: PluginServiceLifecycleOperation,
    },
    /// Lifecycle logging uses live policy but never an event invocation/lease.
    LifecycleLog {
        plan_id: String,
        installation_id: String,
        service_name: String,
        entry: crate::PluginLogEntry,
    },
    /// Creates a consumer at the current tail, or restores the exact retained one.
    Acquire {
        plan_id: String,
        installation_id: String,
        service_name: String,
    },
    Poll {
        lease: PluginServiceLease,
    },
    Complete {
        lease: PluginServiceLease,
        event_id: String,
        result: PluginServiceEventResult,
        /// Optional reader-first cause attachment. Never changes the disposition.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        failure: Option<PluginServiceFailureAttachment>,
    },
    Release {
        lease: PluginServiceLease,
    },
    Diagnostics {
        consumer_id: String,
    },
    /// The only path for Service event Host operations. Nested Service requests,
    /// Tool admission and cancellation are not accepted here.
    Host {
        lease: PluginServiceLease,
        request: Box<crate::PluginInvocationRequest>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(
    tag = "kind",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PluginServiceResponse {
    Prepared,
    Logged,
    Acquired {
        lease: PluginServiceLease,
    },
    Idle,
    Delivery {
        envelope: PluginTicketEventEnvelope,
        invocation_id: String,
        #[schemars(range(min = 0, max = 9_007_199_254_740_991_i64))]
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        expires_at_ms: i64,
    },
    Completed,
    Released,
    Diagnostics {
        diagnostics: PluginServiceDiagnostics,
    },
    Host {
        response: Box<crate::PluginInvocationResponse>,
    },
}

/// Audited recovery is an explicit Workspace management operation, not a guest import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceRecoveryAction {
    Resume,
    Skip {
        event_id: String,
    },
    /// A retention gap cannot be skipped by guessing a missing event identity.
    AcceptGap,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PluginServiceRecoveryRequest {
    pub consumer_id: String,
    pub expected_pause: PluginServicePauseReason,
    pub action: PluginServiceRecoveryAction,
    pub operation_key: String,
    pub reason: String,
}

/// Closed Service-owned cause vocabulary. No guest code/message, source chain,
/// resource identity, path or credential is admitted into this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceFailure {
    Invocation {
        reason: PluginServiceExecutionFailure,
    },
    EffectIndeterminate,
    /// Observation of the Complete boundary, NOT proof that its transaction failed.
    CompletionUnconfirmed,
    Stop {
        reason: PluginServiceExecutionFailure,
    },
    ReleaseUnconfirmed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PluginServiceExecutionFailure {
    Artifact,
    UnsupportedComponent,
    InvalidInput,
    InvalidOutput,
    Cancelled,
    Deadline,
    HostDenied,
    HostInvalidInput,
    HostNotFound,
    HostConflict,
    HostUnsupported,
    HostQuotaExceeded,
    HostDeadlineExceeded,
    HostCancelled,
    HostUnavailable,
    HostIndeterminate,
    HostReceiptExpired,
    Guest,
    Trap,
    UnexpectedOutput,
    GuestRetry,
    GuestRejected,
}

impl PluginServiceExecutionFailure {
    fn detail(&self) -> &'static str {
        match self {
            Self::Artifact => "the artifact was invalid",
            Self::UnsupportedComponent => "the Component execution policy was unsupported",
            Self::InvalidInput => "the invocation input was invalid",
            Self::InvalidOutput => "the returned output was invalid",
            Self::Cancelled => "the invocation was cancelled",
            Self::Deadline => "the invocation deadline was exceeded",
            Self::HostDenied => "the Host operation was denied",
            Self::HostInvalidInput => "the Host operation input was invalid",
            Self::HostNotFound => "the Host resource was not found",
            Self::HostConflict => "the Host operation conflicted with current state",
            Self::HostUnsupported => "the Host operation was unsupported",
            Self::HostQuotaExceeded => "the Host operation quota was exceeded",
            Self::HostDeadlineExceeded => "the Host operation deadline was exceeded",
            Self::HostCancelled => "the Host operation was cancelled",
            Self::HostUnavailable => "the Host operation was unavailable",
            Self::HostIndeterminate => "the Host operation result is unknown",
            Self::HostReceiptExpired => "the Host receipt expired; this does not permit replay",
            Self::Guest => "guest execution failed",
            Self::Trap => "the Component trapped or exceeded a resource limit",
            Self::UnexpectedOutput => "an unexpected lifecycle output was returned",
            Self::GuestRetry => "the guest requested a nonterminal retry",
            Self::GuestRejected => "the guest rejected the delivery",
        }
    }
}
impl PluginServiceFailure {
    pub fn explanation(&self) -> String {
        match self {
            Self::EffectIndeterminate => "Service effect outcome is unknown; owner reconciliation is required.".into(),
            Self::CompletionUnconfirmed => "Service completion persistence was not confirmed; the invocation outcome is unchanged.".into(),
            Self::ReleaseUnconfirmed => "Service lease release was not confirmed; this is a cleanup observation.".into(),
            Self::Stop { reason } => format!("Service stop export failed: {}. The event outcome is unchanged.", reason.detail()),
            Self::Invocation { reason } => format!("Service event invocation: {}.", reason.detail()),
        }
    }
}
impl error_contract::FailureCause for PluginServiceFailure {
    fn identity(&self) -> error_contract::FailureIdentity {
        use error_contract::Category;
        let (reason, category) = match self {
            Self::Invocation { .. } => ("invocation_failed", Category::Dependency),
            Self::EffectIndeterminate => ("effect_indeterminate", Category::Unknown),
            Self::CompletionUnconfirmed => ("completion_unconfirmed", Category::Unknown),
            Self::Stop { .. } => ("stop_failed", Category::Dependency),
            Self::ReleaseUnconfirmed => ("release_unconfirmed", Category::Unknown),
        };
        error_contract::FailureIdentity {
            domain: "plugin_service",
            reason,
            category,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), error_contract::ContractError> {
        Ok(())
    }
}
impl error_contract::WireDetails for PluginServiceFailure {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "plugin_service"
            && version == 1
            && matches!(
                reason,
                "invocation_failed"
                    | "effect_indeterminate"
                    | "completion_unconfirmed"
                    | "stop_failed"
                    | "release_unconfirmed"
            )
    }
}

/// Boundary assertion after current Service/management authorization. The fixed
/// vocabulary has no resource-dependent fields. Stored audience labels are ignored.
pub struct PluginServiceFailureAccess(());
impl PluginServiceFailureAccess {
    pub fn authorized_service_observer() -> Self {
        Self(())
    }
}
macro_rules! service_failure_projection {
    ($($audience:ty),* $(,)?) => {$(
        impl error_contract::ProjectFor<$audience> for PluginServiceFailure {
            type Authorization = PluginServiceFailureAccess;
            type Details = Self;
            fn disclose(&self, _: &error_contract::FailureFacts<Self>, _: &Self::Authorization)
                -> Result<error_contract::Disclosure<Self>, error_contract::ContractError>
            {
                // Rebuild all metadata; never publish received context or explanation.
                Ok(error_contract::Disclosure::new(*self, self.explanation()))
            }
        }
    )*};
}
service_failure_projection!(
    error_contract::Storage,
    error_contract::InternalTransfer,
    error_contract::Browser,
    error_contract::Tool,
    error_contract::Telemetry
);

/// Minimal storage attachment, distinct from startup metadata. Only a Storage
/// projection can construct writer bytes. Deserialize alone grants no disclosure.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct PluginServiceFailureAttachment(String);
impl std::fmt::Debug for PluginServiceFailureAttachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginServiceFailureAttachment")
            .finish_non_exhaustive()
    }
}
impl PluginServiceFailureAttachment {
    pub fn from_projected(
        failure: &error_contract::ProjectedFailure<error_contract::Storage, PluginServiceFailure>,
    ) -> Result<Self, error_contract::ContractError> {
        let bytes = error_contract::encode_projected_failure(failure, Self::budget())?;
        String::from_utf8(bytes.as_bytes().to_vec())
            .map(Self)
            .map_err(|_| error_contract::ContractError::Encoding)
    }
    pub fn decode(&self) -> error_contract::DecodeFailure<PluginServiceFailure> {
        error_contract::decode_failure(self.0.as_bytes(), Self::budget())
    }
    pub fn from_cause(cause: PluginServiceFailure) -> Result<Self, error_contract::ContractError> {
        let facts = error_contract::FailureFacts::new(cause)?;
        Self::from_projected(&error_contract::project_failure::<
            error_contract::Storage,
            _,
        >(
            &facts,
            &PluginServiceFailureAccess::authorized_service_observer(),
        )?)
    }
    /// Normalizes an incoming attachment for owner storage; rejects unknown/corrupt
    /// writer input without classifying it as execution failure.
    pub fn for_storage(&self) -> Result<Self, error_contract::ContractError> {
        match self.decode() {
            error_contract::DecodeFailure::Known(received) => {
                Self::from_projected(&error_contract::project_failure::<
                    error_contract::Storage,
                    _,
                >(
                    received.facts(),
                    &PluginServiceFailureAccess::authorized_service_observer(),
                )?)
            }
            _ => Err(error_contract::ContractError::ProjectionUnavailable),
        }
    }
    pub fn diagnostic(
        &self,
        access: &PluginServiceFailureAccess,
    ) -> PluginServiceFailureDiagnostic {
        match self.decode() {
            error_contract::DecodeFailure::Known(received) => {
                match error_contract::project_failure::<error_contract::Browser, _>(
                    received.facts(),
                    access,
                ) {
                    Ok(projected) => PluginServiceFailureDiagnostic::Known {
                        cause: *projected.details(),
                        explanation: projected.explanation().into(),
                    },
                    Err(_) => PluginServiceFailureDiagnostic::Corrupt,
                }
            }
            error_contract::DecodeFailure::Unsupported(_) => {
                PluginServiceFailureDiagnostic::Unsupported
            }
            error_contract::DecodeFailure::Corrupt(_) => PluginServiceFailureDiagnostic::Corrupt,
        }
    }
    fn budget() -> error_contract::Budget {
        error_contract::Budget::new(4096, 4096)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceFailureDiagnostic {
    Known {
        cause: PluginServiceFailure,
        explanation: String,
    },
    Unsupported,
    Corrupt,
}

/// Existing invocation identity/outcome with a subordinate optional cause. No
/// lease credential is stored here and none of these fields authorizes replay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PluginServiceOutcomeDiagnostic {
    Known {
        event_id: String,
        invocation_id: String,
        attempt: u32,
        result: PluginServiceEventResult,
        failure: Option<PluginServiceFailureDiagnostic>,
    },
    Unsupported,
    Corrupt,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn complete_and_diagnostics_read_legacy_without_inventing_causes() {
        let request: PluginServiceRequest = serde_json::from_value(json!({
            "kind":"complete", "lease":{"consumer_id":"consumer","lease_token":"lease"},
            "event_id":"event", "result":{"kind":"retry_after","delay_ms":1000}
        }))
        .unwrap();
        assert!(matches!(
            request,
            PluginServiceRequest::Complete { failure: None, .. }
        ));
        let diagnostic: PluginServiceDiagnostics = serde_json::from_value(json!({
            "consumer_id":"consumer", "pause_reason":null, "queued":1, "attempt":1,
            "current_event_id":"event", "lease_active":true
        }))
        .unwrap();
        assert_eq!(diagnostic.last_outcome, None);
    }

    #[test]
    fn safe_service_attachment_rejects_raw_details_and_reprojects_received_presentation() {
        let cause = PluginServiceFailure::Invocation {
            reason: PluginServiceExecutionFailure::Deadline,
        };
        let attachment = PluginServiceFailureAttachment::from_cause(cause).unwrap();
        let mut envelope: serde_json::Value = serde_json::from_str(&attachment.0).unwrap();
        envelope["explanation"] = json!("raw credential path /private/source");
        envelope["audience"] = json!("storage");
        let received = PluginServiceFailureAttachment(envelope.to_string());
        assert_eq!(
            received.diagnostic(&PluginServiceFailureAccess::authorized_service_observer()),
            PluginServiceFailureDiagnostic::Known {
                cause,
                explanation: cause.explanation().into()
            }
        );
        envelope["details"]["raw_error"] = json!("credential");
        assert!(matches!(
            PluginServiceFailureAttachment(envelope.to_string()).decode(),
            error_contract::DecodeFailure::Corrupt(_)
        ));
        envelope["details"]
            .as_object_mut()
            .unwrap()
            .remove("raw_error");
        envelope["details"]["reason"] = json!("future_reason");
        assert!(matches!(
            PluginServiceFailureAttachment(envelope.to_string()).decode(),
            error_contract::DecodeFailure::Corrupt(_)
        ));
        assert!(!format!("{received:?}").contains("credential"));
    }

    #[test]
    fn prepare_is_lease_free_and_has_no_consumer_or_event_identity() {
        let request = PluginServiceRequest::Prepare {
            plan_id: "plan".into(),
            installation_id: "installation".into(),
            service_name: "watch".into(),
            operation: PluginServiceLifecycleOperation::Publish,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            json!({"kind":"prepare","plan_id":"plan","installation_id":"installation","service_name":"watch","operation":"publish"})
        );
        assert_eq!(
            serde_json::from_value::<PluginServiceRequest>(value).unwrap(),
            request
        );
        assert!(serde_json::from_value::<PluginServiceRequest>(json!({"kind":"prepare","plan_id":"plan","installation_id":"installation","service_name":"watch","lease":{"consumer_id":"consumer","lease_token":"token"}})).is_err());
    }

    #[test]
    fn leased_host_wire_type_cannot_nest_service_or_admit_tool_calls() {
        for nested in [
            json!({"kind":"service","request":{"kind":"diagnostics","consumer_id":"other"}}),
            json!({"kind":"begin","plan_id":"p","installation_id":"i","consumer":{"kind":"tool_call","tool_call_id":"call","tool_name":"tool"}}),
            json!({"kind":"cancel","invocation_id":"invocation"}),
        ] {
            assert!(serde_json::from_value::<PluginServiceRequest>(json!({"kind":"host","lease":{"consumer_id":"consumer","lease_token":"token"},"request":nested})).is_err());
        }
        // This must stay a finite schema for OpenAPI/TypeScript generation too.
        let schema = schemars::schema_for!(PluginServiceRequest);
        assert!(
            !serde_json::to_string(&schema)
                .unwrap()
                .contains("PluginHostRequest")
        );
    }

    #[test]
    fn recovery_is_typed_management_input_not_a_guest_service_request() {
        let recovery = json!({"consumer_id":"consumer","expected_pause":"indeterminate","action":{"kind":"skip","event_id":"event"},"operation_key":"reviewed-skip","reason":"effect independently reconciled"});
        assert!(serde_json::from_value::<PluginServiceRecoveryRequest>(recovery.clone()).is_ok());
        assert!(serde_json::from_value::<PluginServiceRequest>(recovery).is_err());
    }
}
