//! Closed audit facts are a secondary cause, never the operation's primary failure.
//!
//! Received operation IDs and prose are not disclosed. Correlation and recovery
//! continue through the original Backend receipt; this projection carries only
//! observed stage, reason and independent outcome, without inventing a source.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    failure::WorkdirFailureVisibility,
    http::{
        WorkdirAuditFailure, WorkdirAuditFailureReason, WorkdirAuditOperationOutcome,
        WorkdirAuditStage,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkdirAuditFailureDetails {
    pub stage: WorkdirAuditStage,
    pub reason: WorkdirAuditFailureReason,
    pub operation_outcome: WorkdirAuditOperationOutcome,
}

impl From<&WorkdirAuditFailure> for WorkdirAuditFailureDetails {
    fn from(receipt: &WorkdirAuditFailure) -> Self {
        Self {
            stage: receipt.stage,
            reason: receipt.reason,
            operation_outcome: receipt.operation_outcome,
        }
    }
}

impl FailureCause for WorkdirAuditFailureDetails {
    fn identity(&self) -> FailureIdentity {
        let reason = match self.reason {
            WorkdirAuditFailureReason::AuditStoreUnavailable => "audit_store_unavailable",
        };
        FailureIdentity {
            domain: "workdir_audit",
            reason,
            category: Category::Internal,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl WireDetails for WorkdirAuditFailureDetails {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "workdir_audit" && reason == "audit_store_unavailable" && version == 1
    }
}

impl WorkdirAuditFailureDetails {
    fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let cause = match self.reason {
            WorkdirAuditFailureReason::AuditStoreUnavailable => {
                "Workdir audit persistence is unavailable"
            }
        };
        let impact = match self.operation_outcome {
            WorkdirAuditOperationOutcome::Succeeded => {
                "the original operation succeeded, but its audit receipt was not persisted; do not resend the operation for audit loss"
            }
            WorkdirAuditOperationOutcome::Failed { .. } => {
                "the original operation failed independently, and its audit receipt was not persisted; audit loss does not authorize a resend"
            }
            WorkdirAuditOperationOutcome::Unknown => {
                "the original operation outcome is unknown, and its audit receipt was not persisted; inspect effects before considering a retry"
            }
            WorkdirAuditOperationOutcome::NotDispatched => {
                "the original operation was not dispatched, and its audit receipt was not persisted"
            }
        };
        let stage = match self.stage {
            WorkdirAuditStage::BeforeDispatch => "before_dispatch",
            WorkdirAuditStage::DispatchRejected => "dispatch_rejected",
            WorkdirAuditStage::ResponseReceived => "response_received",
            WorkdirAuditStage::ResponseUnknown => "response_unknown",
        };
        let mut disclosure = Disclosure::new(*self, compose_explanation(cause, impact)?);
        disclosure
            .context
            .operations
            .push(OperationContext::new("audit", stage)?);
        Ok(disclosure)
    }
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for WorkdirAuditFailureDetails {
            type Authorization = WorkdirFailureVisibility;
            type Details = Self;
            fn disclose(&self, _: &FailureFacts<Self>, visibility: &WorkdirFailureVisibility) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                self.disclosure()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);

#[cfg(test)]
mod tests {
    use super::*;
    use error_contract::{
        Budget, DecodeFailure, decode_failure, encode_projected_failure, project_failure,
    };

    #[test]
    fn received_audit_facts_keep_success_and_unknown_without_disclosing_received_identity() {
        for outcome in [
            WorkdirAuditOperationOutcome::Succeeded,
            WorkdirAuditOperationOutcome::Unknown,
        ] {
            let receipt: WorkdirAuditFailure = serde_json::from_value(serde_json::json!({
                "operation_id":"/private/provider/token=secret", "stage":"response_received",
                "reason":"audit_store_unavailable", "operation_outcome":outcome
            }))
            .unwrap();
            let facts = FailureFacts::new(WorkdirAuditFailureDetails::from(&receipt)).unwrap();
            let visibility = WorkdirFailureVisibility {
                can_view_cause: true,
            };
            let browser = project_failure::<Browser, _>(&facts, &visibility).unwrap();
            let tool = project_failure::<Tool, _>(&facts, &visibility).unwrap();
            assert_eq!(browser.details(), tool.details());
            assert_eq!(browser.explanation(), tool.explanation());
            let encoded = encode_projected_failure(&tool, Budget::new(65536, 65536)).unwrap();
            assert!(!String::from_utf8_lossy(encoded.as_bytes()).contains("secret"));
            let DecodeFailure::Known(received) = decode_failure::<WorkdirAuditFailureDetails>(
                encoded.as_bytes(),
                Budget::new(65536, 65536),
            ) else {
                panic!("audit cause must decode")
            };
            assert_eq!(received.facts().cause().operation_outcome, outcome);
            assert!(
                project_failure::<Tool, _>(received.facts(), &WorkdirFailureVisibility::default())
                    .is_err()
            );
        }
    }
}
