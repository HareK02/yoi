//! Storage boundary for background failure evidence, not execution/retry authority.
//!
//! Only a Storage projection can create a write attachment. Loaded bytes are a
//! different type: decode validates the existing error-contract envelope, and
//! redisclosure always requires the domain's current authorization/projector.
//! Neither type accepts Display errors or reconstructs causes from legacy prose.
use error_contract::{
    Audience, Budget, ContractError, DecodeFailure, ProjectFor, ProjectedFailure, Storage,
    WireDetails, decode_failure, encode_projected_failure, project_failure,
};

/// A storage-safe writer value. Intentionally not deserializable: stored/received
/// audience labels cannot mint a fresh storage projection or disclosure capability.
#[derive(Clone, PartialEq, Eq)]
pub struct BackgroundFailure(String);

/// Untrusted retained evidence. Not serializable: query boundaries must publish a
/// fresh projection, not forward a historical explanation or unknown payload.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredFailure(String);

macro_rules! redacted_debug {
    ($name:ident) => {
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}
redacted_debug!(BackgroundFailure);
redacted_debug!(StoredFailure);

fn budget() -> Budget {
    Budget::new(
        error_contract::MAX_FAILURE_BYTES,
        error_contract::MAX_FAILURE_BYTES,
    )
}

impl BackgroundFailure {
    pub fn from_projected<D: WireDetails>(
        failure: &ProjectedFailure<Storage, D>,
    ) -> Result<Self, ContractError> {
        let encoded = encode_projected_failure(failure, budget())?;
        String::from_utf8(encoded.as_bytes().to_vec())
            .map(Self)
            .map_err(|_| ContractError::Encoding)
    }

    /// Bytes for the existing result transaction, never a public response encoder.
    pub fn as_storage_str(&self) -> &str {
        &self.0
    }

    /// Reproject only implemented startup domains. Unknown/corrupt causes are
    /// conversion failures, not permission to invent a generic startup refusal.
    pub fn from_startup_primary(
        primary: &crate::startup_failure::StartupFailureAttachment,
    ) -> Result<Self, ContractError> {
        use crate::{
            session_failure::SessionRegistrationCause, startup_failure::RuntimeStartupCause,
        };
        if let Ok(projected) = primary.project_for::<Storage, SessionRegistrationCause>(&()) {
            return Self::from_projected(&projected);
        }
        let projected = primary.project_for::<Storage, RuntimeStartupCause>(&())?;
        Self::from_projected(&projected)
    }
}

impl StoredFailure {
    /// Reader-only entrance. These bytes are not validated, serializable, or
    /// accepted by the writer APIs. Preserve corrupt/unsupported evidence locally.
    pub fn from_storage(raw: String) -> Self {
        Self(raw)
    }

    pub fn decode<D: WireDetails>(&self) -> DecodeFailure<D> {
        decode_failure(self.0.as_bytes(), budget())
    }

    pub fn read_optional<D: WireDetails>(
        stored: Option<&Self>,
    ) -> error_contract::compatibility::StoredFailureRead<D> {
        error_contract::compatibility::read_failure_attachment(
            stored.map(|value| value.0.as_bytes()),
            budget(),
        )
    }

    pub fn project_for<A: Audience, D: WireDetails + ProjectFor<A>>(
        &self,
        authorization: &D::Authorization,
    ) -> Result<ProjectedFailure<A, D::Details>, ContractError> {
        match self.decode::<D>() {
            DecodeFailure::Known(received) => project_failure(received.facts(), authorization),
            DecodeFailure::Unsupported(_) | DecodeFailure::Corrupt(_) => {
                Err(ContractError::ProjectionUnavailable)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_failure::{
        SessionFailureVisibility, SessionPrincipal, SessionRegistrationCause,
        session_registration_attachment,
    };

    #[test]
    fn storage_reprojects_known_startup_primary_with_current_visibility() {
        let primary = session_registration_attachment(SessionRegistrationCause::denied(
            SessionPrincipal::Subject,
        ))
        .unwrap();
        let writer = BackgroundFailure::from_startup_primary(&primary).unwrap();
        let stored = StoredFailure::from_storage(writer.as_storage_str().into());
        assert!(matches!(
            stored.decode::<SessionRegistrationCause>(),
            DecodeFailure::Known(_)
        ));
        assert!(
            stored
                .project_for::<error_contract::Browser, SessionRegistrationCause>(
                    &SessionFailureVisibility {
                        can_view_cause: false
                    },
                )
                .is_err()
        );
        let projected = stored
            .project_for::<error_contract::Tool, SessionRegistrationCause>(
                &SessionFailureVisibility {
                    can_view_cause: true,
                },
            )
            .unwrap();
        assert_eq!(
            projected.details().required_operation,
            crate::session_failure::SessionOperation::Record
        );
    }

    #[test]
    fn stored_startup_conversion_uses_domain_policy_not_received_explanation() {
        let primary = session_registration_attachment(SessionRegistrationCause::denied(
            SessionPrincipal::Subject,
        ))
        .unwrap();
        let mut retained = serde_json::to_value(&primary)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        retained = retained.replace(
            "Session registration permission denied.",
            "TEST_ONLY_SECRET /private/path raw_http_body",
        );
        let received = serde_json::from_value(serde_json::Value::String(retained)).unwrap();
        let writer = BackgroundFailure::from_startup_primary(&received).unwrap();
        assert!(!writer.as_storage_str().contains("TEST_ONLY_SECRET"));
        assert!(!writer.as_storage_str().contains("/private/path"));
        let corrupt = serde_json::from_value(serde_json::json!("not a typed envelope")).unwrap();
        assert!(BackgroundFailure::from_startup_primary(&corrupt).is_err());
        let facts =
            error_contract::FailureFacts::new(crate::startup_failure::RuntimeStartupCause {
                stage: crate::startup_failure::StartupStage::Controller,
                reason: crate::startup_failure::RuntimeStartupReason::StartupFailed,
            })
            .unwrap();
        let transfer = project_failure::<error_contract::InternalTransfer, _>(&facts, &()).unwrap();
        let primary =
            crate::startup_failure::StartupFailureAttachment::from_projected(&transfer).unwrap();
        let writer = BackgroundFailure::from_startup_primary(&primary).unwrap();
        assert!(matches!(
            StoredFailure::from_storage(writer.0)
                .decode::<crate::startup_failure::RuntimeStartupCause>(),
            DecodeFailure::Known(_)
        ));
    }

    #[test]
    fn absent_future_corrupt_and_oversized_evidence_are_distinct() {
        use error_contract::compatibility::StoredFailureRead;
        assert!(matches!(
            StoredFailure::read_optional::<SessionRegistrationCause>(None),
            StoredFailureRead::LegacyWithoutFacts
        ));
        let primary =
            session_registration_attachment(SessionRegistrationCause::transport()).unwrap();
        let writer = BackgroundFailure::from_startup_primary(&primary).unwrap();
        let future =
            StoredFailure::from_storage(writer.0.replace("\"version\":1", "\"version\":999"));
        assert!(matches!(
            future.decode::<SessionRegistrationCause>(),
            DecodeFailure::Unsupported(_)
        ));
        let corrupt = StoredFailure::from_storage("{ TEST_ONLY_SECRET /private/path".into());
        assert!(matches!(
            corrupt.decode::<SessionRegistrationCause>(),
            DecodeFailure::Corrupt(_)
        ));
        assert!(!format!("{corrupt:?}").contains("TEST_ONLY_SECRET"));
        let oversized =
            StoredFailure::from_storage(" ".repeat(error_contract::MAX_FAILURE_BYTES + 1));
        assert!(matches!(
            oversized.decode::<SessionRegistrationCause>(),
            DecodeFailure::Corrupt(ContractError::TooLarge)
        ));
        for stored in [future, corrupt, oversized] {
            assert!(
                stored
                    .project_for::<error_contract::Browser, SessionRegistrationCause>(
                        &SessionFailureVisibility {
                            can_view_cause: true
                        },
                    )
                    .is_err()
            );
        }
    }
}
