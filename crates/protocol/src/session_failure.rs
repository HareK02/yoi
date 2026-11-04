//! Typed session-registration failures, not lifecycle or retry authority.
//!
//! Causes contain only fixed enum values. Stored or transferred attachments are
//! observations, never permission to disclose: Browser/Tool projections require
//! the caller's current visibility decision and fail closed without it.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Tool, WireDetails, project_failure,
};
use serde::{Deserialize, Serialize};

use crate::startup_failure::StartupFailureAttachment;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum SessionRegistrationReason {
    PermissionDenied,
    AttributionMissing,
    BindingMismatch,
    Transport,
    OutcomeUnknown,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum SessionPrincipal {
    Operator,
    Subject,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum SessionOperation {
    #[serde(rename = "session.record")]
    Record,
}

/// No account/session identifiers, paths, or free-form diagnostics are retained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct SessionRegistrationCause {
    pub reason: SessionRegistrationReason,
    pub principal: SessionPrincipal,
    pub required_operation: SessionOperation,
}

impl SessionRegistrationCause {
    pub const fn denied(principal: SessionPrincipal) -> Self {
        Self::new(SessionRegistrationReason::PermissionDenied, principal)
    }

    pub const fn attribution_missing() -> Self {
        Self::new(
            SessionRegistrationReason::AttributionMissing,
            SessionPrincipal::Subject,
        )
    }

    pub const fn binding_mismatch() -> Self {
        Self::new(
            SessionRegistrationReason::BindingMismatch,
            SessionPrincipal::Subject,
        )
    }

    pub const fn transport() -> Self {
        Self::new(
            SessionRegistrationReason::Transport,
            SessionPrincipal::Unknown,
        )
    }

    pub const fn outcome_unknown() -> Self {
        Self::new(
            SessionRegistrationReason::OutcomeUnknown,
            SessionPrincipal::Unknown,
        )
    }

    pub const fn invalid_response() -> Self {
        Self::new(
            SessionRegistrationReason::InvalidResponse,
            SessionPrincipal::Unknown,
        )
    }

    const fn new(reason: SessionRegistrationReason, principal: SessionPrincipal) -> Self {
        Self {
            reason,
            principal,
            required_operation: SessionOperation::Record,
        }
    }

    fn explanation(&self) -> &'static str {
        match self.reason {
            SessionRegistrationReason::PermissionDenied => {
                "Session registration permission denied."
            }
            SessionRegistrationReason::AttributionMissing => {
                "Session registration requires Subject attribution."
            }
            SessionRegistrationReason::BindingMismatch => {
                "Session registration Subject binding does not match."
            }
            SessionRegistrationReason::Transport => "Session registration transport failed.",
            SessionRegistrationReason::OutcomeUnknown => "Session registration outcome is unknown.",
            SessionRegistrationReason::InvalidResponse => {
                "Session registration returned an invalid response."
            }
        }
    }

    fn disclosure(&self, facts: &FailureFacts<Self>) -> Disclosure<Self> {
        let mut disclosure = Disclosure::new(*self, self.explanation());
        for context in &facts.context().operations {
            if matches!(
                (context.operation.as_str(), context.stage.as_str()),
                ("session.record", "authorization" | "registration")
                    | ("worker_startup", "binding" | "runtime")
            ) {
                let mut context = context.clone();
                context.operation_ref = None;
                disclosure.context.operations.push(context);
            }
        }
        disclosure
    }
}

impl FailureCause for SessionRegistrationCause {
    fn identity(&self) -> FailureIdentity {
        let (reason, category) = match self.reason {
            SessionRegistrationReason::PermissionDenied => {
                ("permission_denied", Category::Authorization)
            }
            SessionRegistrationReason::AttributionMissing => {
                ("attribution_missing", Category::Authorization)
            }
            SessionRegistrationReason::BindingMismatch => {
                ("binding_mismatch", Category::Authorization)
            }
            SessionRegistrationReason::Transport => ("transport", Category::Dependency),
            SessionRegistrationReason::OutcomeUnknown => ("outcome_unknown", Category::Unknown),
            SessionRegistrationReason::InvalidResponse => ("invalid_response", Category::Internal),
        };
        FailureIdentity {
            domain: "session_registration",
            reason,
            category,
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        // All fields are closed enums; arbitrary diagnostic data cannot enter.
        Ok(())
    }
}

impl WireDetails for SessionRegistrationCause {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "session_registration"
            && version == 1
            && matches!(
                reason,
                "permission_denied"
                    | "attribution_missing"
                    | "binding_mismatch"
                    | "transport"
                    | "outcome_unknown"
                    | "invalid_response"
            )
    }
}

impl std::fmt::Display for SessionRegistrationCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.explanation())
    }
}

impl std::error::Error for SessionRegistrationCause {}

/// A current caller-supplied visibility decision, not a serialized capability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionFailureVisibility {
    pub can_view_cause: bool,
}

impl ProjectFor<InternalTransfer> for SessionRegistrationCause {
    type Authorization = ();
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self>, ContractError> {
        Ok(self.disclosure(facts))
    }
}

impl ProjectFor<Storage> for SessionRegistrationCause {
    type Authorization = ();
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        _: &(),
    ) -> Result<Disclosure<Self>, ContractError> {
        Ok(self.disclosure(facts))
    }
}

impl ProjectFor<Browser> for SessionRegistrationCause {
    type Authorization = SessionFailureVisibility;
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        authorization: &SessionFailureVisibility,
    ) -> Result<Disclosure<Self>, ContractError> {
        if !authorization.can_view_cause {
            // Redaction must not substitute a different reason/category identity.
            return Err(ContractError::InvalidDetails);
        }
        Ok(self.disclosure(facts))
    }
}

impl ProjectFor<Tool> for SessionRegistrationCause {
    type Authorization = SessionFailureVisibility;
    type Details = Self;

    fn disclose(
        &self,
        facts: &FailureFacts<Self>,
        authorization: &SessionFailureVisibility,
    ) -> Result<Disclosure<Self>, ContractError> {
        if !authorization.can_view_cause {
            return Err(ContractError::InvalidDetails);
        }
        Ok(self.disclosure(facts))
    }
}

/// Attach fixed registration/binding observations using internal-transfer policy.
/// This does not assert registration rollback, retry safety, or startup ownership.
pub fn session_registration_attachment(
    cause: SessionRegistrationCause,
) -> Result<StartupFailureAttachment, ContractError> {
    let mut facts = FailureFacts::new(cause)?;
    facts.with_context(OperationContext::new("session.record", "registration")?)?;
    facts.with_context(OperationContext::new("worker_startup", "binding")?)?;
    let projected = project_failure::<InternalTransfer, _>(&facts, &())?;
    StartupFailureAttachment::from_projected(&projected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup_failure::StoredStartupFailureAttachment;
    use error_contract::{
        Audience, Budget, CausalLink, CorrelationRef, DecodeFailure, ProjectedFailure,
        decode_failure, encode_projected_failure,
    };
    use serde_json::json;

    fn budget() -> Budget {
        Budget::new(
            error_contract::MAX_FAILURE_BYTES,
            error_contract::MAX_FAILURE_BYTES,
        )
    }

    fn cases() -> [(
        SessionRegistrationCause,
        &'static str,
        Category,
        &'static str,
    ); 6] {
        [
            (
                SessionRegistrationCause::denied(SessionPrincipal::Operator),
                "permission_denied",
                Category::Authorization,
                "Session registration permission denied.",
            ),
            (
                SessionRegistrationCause::attribution_missing(),
                "attribution_missing",
                Category::Authorization,
                "Session registration requires Subject attribution.",
            ),
            (
                SessionRegistrationCause::binding_mismatch(),
                "binding_mismatch",
                Category::Authorization,
                "Session registration Subject binding does not match.",
            ),
            (
                SessionRegistrationCause::transport(),
                "transport",
                Category::Dependency,
                "Session registration transport failed.",
            ),
            (
                SessionRegistrationCause::outcome_unknown(),
                "outcome_unknown",
                Category::Unknown,
                "Session registration outcome is unknown.",
            ),
            (
                SessionRegistrationCause::invalid_response(),
                "invalid_response",
                Category::Internal,
                "Session registration returned an invalid response.",
            ),
        ]
    }

    #[test]
    fn constructors_serde_identity_and_fixed_explanations() {
        for (cause, reason, category, explanation) in cases() {
            let principal = match cause.reason {
                SessionRegistrationReason::PermissionDenied => "operator",
                SessionRegistrationReason::AttributionMissing
                | SessionRegistrationReason::BindingMismatch => "subject",
                _ => "unknown",
            };
            let value = serde_json::to_value(cause).unwrap();
            assert_eq!(
                value,
                json!({
                    "reason": reason,
                    "principal": principal,
                    "required_operation": "session.record"
                })
            );
            assert_eq!(
                serde_json::from_value::<SessionRegistrationCause>(value).unwrap(),
                cause
            );
            assert_eq!(cause.required_operation, SessionOperation::Record);
            assert_eq!(
                cause.identity(),
                FailureIdentity {
                    domain: "session_registration",
                    reason,
                    category,
                    details_version: 1,
                }
            );
            assert!(SessionRegistrationCause::supports(
                "session_registration",
                reason,
                1
            ));
            assert_eq!(cause.to_string(), explanation);
            assert!(std::error::Error::source(&cause).is_none());
            let facts = FailureFacts::new(cause).unwrap();
            let projected = project_failure::<InternalTransfer, _>(&facts, &()).unwrap();
            assert_eq!(projected.explanation(), explanation);
            let encoded = encode_projected_failure(&projected, budget()).unwrap();
            match decode_failure::<SessionRegistrationCause>(encoded.as_bytes(), budget()) {
                DecodeFailure::Known(received) => assert_eq!(received.facts().cause(), &cause),
                _ => panic!("expected known session-registration failure"),
            }
        }
        for (principal, wire) in [
            (SessionPrincipal::Operator, "operator"),
            (SessionPrincipal::Subject, "subject"),
            (SessionPrincipal::Unknown, "unknown"),
        ] {
            let cause = SessionRegistrationCause::denied(principal);
            assert_eq!(cause.principal, principal);
            assert_eq!(
                serde_json::to_value(cause).unwrap()["principal"],
                json!(wire)
            );
        }
        assert!(!SessionRegistrationCause::supports(
            "worker_startup",
            "transport",
            1
        ));
        assert!(!SessionRegistrationCause::supports(
            "session_registration",
            "future",
            1
        ));
        assert!(!SessionRegistrationCause::supports(
            "session_registration",
            "transport",
            2
        ));
    }

    #[test]
    fn serde_rejects_untyped_diagnostic_fields_and_unknown_enum_values() {
        let valid = serde_json::to_value(SessionRegistrationCause::attribution_missing()).unwrap();
        for (field, value) in [
            ("reason", json!("raw_error")),
            ("principal", json!("account-secret")),
            ("required_operation", json!("session.delete")),
            ("diagnostic", json!("/private/path TEST_ONLY_SECRET")),
        ] {
            let mut changed = valid.clone();
            changed[field] = value;
            assert!(serde_json::from_value::<SessionRegistrationCause>(changed).is_err());
        }
        for field in ["reason", "principal", "required_operation"] {
            let mut changed = valid.clone();
            changed.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<SessionRegistrationCause>(changed).is_err());
        }
    }

    #[test]
    fn codec_distinguishes_unsupported_and_corrupt_and_replaces_received_prose() {
        let facts = FailureFacts::new(SessionRegistrationCause::transport()).unwrap();
        let projected = project_failure::<InternalTransfer, _>(&facts, &()).unwrap();
        let encoded = encode_projected_failure(&projected, budget()).unwrap();
        let original: serde_json::Value = serde_json::from_slice(encoded.as_bytes()).unwrap();
        for (field, value) in [
            ("domain", json!("future_domain")),
            ("reason", json!("future_reason")),
            ("details_version", json!(2)),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert!(matches!(
                decode_failure::<SessionRegistrationCause>(
                    &serde_json::to_vec(&changed).unwrap(),
                    budget()
                ),
                DecodeFailure::Unsupported(_)
            ));
        }
        for (field, value) in [
            ("reason", json!("permission_denied")),
            ("category", json!("authorization")),
        ] {
            let mut changed = original.clone();
            changed[field] = value;
            assert!(matches!(
                decode_failure::<SessionRegistrationCause>(
                    &serde_json::to_vec(&changed).unwrap(),
                    budget()
                ),
                DecodeFailure::Corrupt(ContractError::InvalidDetails)
            ));
        }
        let mut changed = original;
        changed["explanation"] = json!("TEST_ONLY_SECRET received prose");
        changed["context"]["diagnostics"] = json!([
            {"code": "TEST_ONLY_SECRET", "explanation": "TEST_ONLY_SECRET diagnostic"}
        ]);
        let bytes = serde_json::to_vec(&changed).unwrap();
        let DecodeFailure::Known(received) =
            decode_failure::<SessionRegistrationCause>(&bytes, budget())
        else {
            panic!("expected known failure with untrusted supplementary fields");
        };
        let browser = project_failure::<Browser, _>(
            received.facts(),
            &SessionFailureVisibility {
                can_view_cause: true,
            },
        )
        .unwrap();
        assert_eq!(
            browser.explanation(),
            "Session registration transport failed."
        );
        assert!(browser.context().diagnostics.is_empty());
        let encoded = encode_projected_failure(&browser, budget()).unwrap();
        assert!(
            !std::str::from_utf8(encoded.as_bytes())
                .unwrap()
                .contains("TEST_ONLY_SECRET")
        );
    }

    #[test]
    fn attachment_storage_and_current_visibility_preserve_identity_without_granting_access() {
        let allowed = SessionFailureVisibility {
            can_view_cause: true,
        };
        let denied = SessionFailureVisibility::default();
        for (cause, _, _, _) in cases() {
            let attachment = session_registration_attachment(cause).unwrap();
            match attachment.decode::<SessionRegistrationCause>() {
                DecodeFailure::Known(received) => {
                    assert_eq!(received.facts().cause(), &cause);
                    assert_eq!(
                        received.facts().context().operations,
                        vec![
                            OperationContext::new("session.record", "registration").unwrap(),
                            OperationContext::new("worker_startup", "binding").unwrap(),
                        ]
                    );
                }
                _ => panic!("expected typed startup attachment"),
            }
            let storage = attachment
                .project_for::<Storage, SessionRegistrationCause>(&())
                .unwrap();
            let stored = StoredStartupFailureAttachment::from_projected(&storage).unwrap();
            let loaded: StoredStartupFailureAttachment =
                serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
            let browser = loaded
                .project_for::<Browser, SessionRegistrationCause>(&allowed)
                .unwrap();
            let tool = loaded
                .project_for::<Tool, SessionRegistrationCause>(&allowed)
                .unwrap();
            assert_eq!(browser.details(), &cause);
            assert_eq!(tool.details(), &cause);
            assert_eq!(browser.identity(), cause.identity());
            assert_eq!(tool.identity(), cause.identity());
            assert!(matches!(
                loaded.project_for::<Browser, SessionRegistrationCause>(&denied),
                Err(ContractError::InvalidDetails)
            ));
            assert!(matches!(
                loaded.project_for::<Tool, SessionRegistrationCause>(&denied),
                Err(ContractError::InvalidDetails)
            ));
            // Even a previous public projection cannot serve as current permission.
            let previously_public = StartupFailureAttachment::from_projected(&browser).unwrap();
            assert!(matches!(
                previously_public.project_for::<Browser, SessionRegistrationCause>(&denied),
                Err(ContractError::InvalidDetails)
            ));
            assert!(matches!(
                previously_public.project_for::<Tool, SessionRegistrationCause>(&denied),
                Err(ContractError::InvalidDetails)
            ));
        }
    }

    fn assert_redacted<A: Audience>(projected: &ProjectedFailure<A, SessionRegistrationCause>) {
        assert_eq!(
            projected.context().operations,
            vec![
                OperationContext::new("session.record", "authorization").unwrap(),
                OperationContext::new("session.record", "registration").unwrap(),
                OperationContext::new("worker_startup", "binding").unwrap(),
                OperationContext::new("worker_startup", "runtime").unwrap(),
            ]
        );
        assert!(projected.context().correlation.is_none());
        assert!(projected.context().causes.is_empty());
        assert!(projected.context().diagnostics.is_empty());
        let encoded = encode_projected_failure(projected, budget()).unwrap();
        let wire = std::str::from_utf8(encoded.as_bytes()).unwrap();
        for secret in ["TEST_ONLY_SECRET", "/private/path", "raw_stage"] {
            assert!(!wire.contains(secret));
        }
    }

    #[test]
    fn every_audience_allowlists_operation_stage_pairs_and_drops_references() {
        let mut facts = FailureFacts::new(SessionRegistrationCause::binding_mismatch()).unwrap();
        for (operation, stage) in [
            ("session.record", "authorization"),
            ("session.record", "registration"),
            ("worker_startup", "binding"),
            ("worker_startup", "runtime"),
            ("session.record", "binding"),
            ("worker_startup", "authorization"),
            ("/private/path", "registration"),
            ("session.record", "raw_stage"),
        ] {
            let mut context = OperationContext::new(operation, stage).unwrap();
            context.operation_ref = Some("TEST_ONLY_SECRET operation reference".into());
            facts.with_context(context).unwrap();
        }
        facts
            .set_correlation(CorrelationRef::new("TEST_ONLY_SECRET correlation").unwrap())
            .unwrap();
        facts
            .with_causal_link(CausalLink {
                domain: "TEST_ONLY_SECRET domain".into(),
                reason: "TEST_ONLY_SECRET reason".into(),
                correlation: CorrelationRef::new("TEST_ONLY_SECRET causal reference").unwrap(),
            })
            .unwrap();
        let allowed = SessionFailureVisibility {
            can_view_cause: true,
        };
        assert_redacted(&project_failure::<InternalTransfer, _>(&facts, &()).unwrap());
        assert_redacted(&project_failure::<Storage, _>(&facts, &()).unwrap());
        assert_redacted(&project_failure::<Browser, _>(&facts, &allowed).unwrap());
        assert_redacted(&project_failure::<Tool, _>(&facts, &allowed).unwrap());
        // Projection selects output without mutating the owner's facts.
        assert_eq!(facts.context().operations.len(), 8);
        assert!(
            facts
                .context()
                .operations
                .iter()
                .all(|context| context.operation_ref.is_some())
        );
    }
}
