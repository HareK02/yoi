//! Session-record judgment projection. Only explicit domain decisions issue causes;
//! status codes, diagnostic prose and arbitrary backend errors are not classifiers.
use super::*;
use protocol::session_failure::{SessionPrincipal, SessionRegistrationCause};

#[derive(Clone, Copy)]
pub(super) enum Judgment {
    Other,
    SessionRecord,
}

impl Judgment {
    pub(super) fn error(self, error: Error, cause: SessionRegistrationCause) -> ApiError {
        self.attach(error.into(), cause)
    }

    fn attach(self, mut error: ApiError, cause: SessionRegistrationCause) -> ApiError {
        if matches!(self, Self::SessionRecord) && error.session_registration_cause.is_none() {
            error.session_registration_cause = Some(cause);
        }
        error
    }

    pub(super) fn permission_rejection(
        self,
        error: ApiError,
        principal: SessionPrincipal,
    ) -> ApiError {
        if matches!(&error.error, Error::WorkspacePermissionDenied(_)) {
            self.attach(error, SessionRegistrationCause::denied(principal))
        } else {
            error
        }
    }

    pub(super) fn binding_rejection(self, error: ApiError) -> ApiError {
        if matches!(
            &error.error,
            Error::WorkspacePermissionDenied(_) | Error::SubjectBodySuperseded
        ) {
            self.attach(error, SessionRegistrationCause::binding_mismatch())
        } else {
            error
        }
    }
}

pub(super) fn bind_host_session(
    api: &WorkspaceApi,
    attribution: crate::subjektiv::SubjectSessionAttribution,
) -> ApiResult<()> {
    subjektiv_journal::bind_host_session_for_registration(api, attribution)
        .map_err(|error| Judgment::SessionRecord.binding_rejection(error))
}

pub(super) fn journal_error(error: subjektiv::journal::JournalError) -> ApiError {
    let cause = match &error {
        subjektiv::journal::JournalError::PermissionDenied(_) => {
            SessionRegistrationCause::binding_mismatch()
        }
        _ => SessionRegistrationCause::outcome_unknown(),
    };
    let mut failure = Judgment::SessionRecord.error(Error::Store(cause.to_string()), cause);
    failure.session_registration_source = Some(Box::new(error));
    failure
}

pub(super) fn domain_error(error: crate::subjektiv::SubjektivError) -> ApiError {
    let cause = match &error {
        crate::subjektiv::SubjektivError::SessionSubjectConflict { .. }
        | crate::subjektiv::SubjektivError::SessionAttributionConflict { .. } => {
            SessionRegistrationCause::binding_mismatch()
        }
        _ => SessionRegistrationCause::outcome_unknown(),
    };
    let mut failure = Judgment::SessionRecord.error(Error::InvalidInput(cause.to_string()), cause);
    failure.session_registration_source = Some(Box::new(error));
    failure
}

pub(super) fn execution_error(error: ApiError) -> ApiError {
    // This typed executor result means the accepted callback's outcome cannot
    // be established. It is not an authorization denial or proof of rollback.
    if matches!(&error.error, Error::DbOperationOutcomeUnknown) {
        Judgment::SessionRecord.attach(error, SessionRegistrationCause::outcome_unknown())
    } else {
        error
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.session_registration_source.as_deref() {
            Some(source) => Some(source),
            None => Some(&self.error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use api_macros::HttpError;
    use protocol::session_failure::SessionRegistrationReason;

    #[test]
    fn permission_projection_keeps_principal_and_original_error() {
        for principal in [SessionPrincipal::Operator, SessionPrincipal::Subject] {
            let error = Judgment::SessionRecord.permission_rejection(
                Error::WorkspacePermissionDenied("wording is not classification input".into())
                    .into(),
                principal,
            );
            let cause = error.session_registration_cause.as_ref().unwrap();
            assert_eq!(cause.reason, SessionRegistrationReason::PermissionDenied);
            assert_eq!(cause.principal, principal);
            assert_eq!(api_error_status(&error.error), StatusCode::FORBIDDEN);
            assert!(
                std::error::Error::source(&error)
                    .unwrap()
                    .downcast_ref::<Error>()
                    .is_some()
            );
        }
    }

    #[test]
    fn unrelated_operations_and_non_denial_errors_stay_unclassified() {
        let unrelated = Judgment::Other.permission_rejection(
            Error::WorkspacePermissionDenied("session.record permission denied".into()).into(),
            SessionPrincipal::Subject,
        );
        assert!(unrelated.session_registration_cause.is_none());
        let backend = Judgment::SessionRecord.permission_rejection(
            Error::Store("session.record permission denied".into()).into(),
            SessionPrincipal::Subject,
        );
        assert!(backend.session_registration_cause.is_none());
        assert!(backend.into_repository_api_error().failure.is_none());
    }

    #[test]
    fn known_causes_are_not_replaced_by_outer_projection() {
        for cause in [
            SessionRegistrationCause::attribution_missing(),
            SessionRegistrationCause::binding_mismatch(),
        ] {
            let error = Judgment::SessionRecord.error(
                Error::WorkspacePermissionDenied("private attribution diagnostic".into()),
                cause,
            );
            let before = serde_json::to_value(error.session_registration_cause.as_ref()).unwrap();
            let error =
                Judgment::SessionRecord.permission_rejection(error, SessionPrincipal::Subject);
            assert_eq!(
                before,
                serde_json::to_value(error.session_registration_cause.as_ref()).unwrap()
            );
        }
    }

    #[test]
    fn unknown_executor_outcome_is_not_permission_denied() {
        let error = execution_error(Error::DbOperationOutcomeUnknown.into());
        assert_eq!(
            error.session_registration_cause.unwrap().reason,
            SessionRegistrationReason::OutcomeUnknown
        );
        assert!(
            execution_error(Error::DbExecutorUnavailable.into())
                .session_registration_cause
                .is_none()
        );
    }

    #[test]
    fn all_enum_only_causes_satisfy_the_transfer_contract() {
        for cause in [
            SessionRegistrationCause::denied(SessionPrincipal::Operator),
            SessionRegistrationCause::denied(SessionPrincipal::Subject),
            SessionRegistrationCause::denied(SessionPrincipal::Unknown),
            SessionRegistrationCause::attribution_missing(),
            SessionRegistrationCause::binding_mismatch(),
            SessionRegistrationCause::transport(),
            SessionRegistrationCause::outcome_unknown(),
            SessionRegistrationCause::invalid_response(),
        ] {
            protocol::session_failure::session_registration_attachment(cause).unwrap();
        }
    }

    #[test]
    fn internal_transfer_contains_no_original_error_text() {
        let error = Judgment::SessionRecord
            .error(
                Error::WorkspacePermissionDenied("secret-subject-id /private/path raw-body".into()),
                SessionRegistrationCause::denied(SessionPrincipal::Subject),
            )
            .into_repository_api_error();
        assert_eq!(error.status_code(), StatusCode::FORBIDDEN.as_u16());
        let attachment = serde_json::to_string(error.failure.as_ref().unwrap()).unwrap();
        for private in ["secret-subject-id", "/private/path", "raw-body"] {
            assert!(!attachment.contains(private));
        }
    }
}
