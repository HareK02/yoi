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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
