//! Local Workdir cleanup causes retain all provider errors, independently of the
//! operation that requested cleanup. Failure never proves that a session closed.
use std::fmt;
use workdir::failure::{WorkdirFailureDetails, WorkdirFailureOperation};
use workdir::http::{WorkdirTransportError, WorkdirTransportErrorCode};
use workdir::{WorkdirError, WorkdirFailureReason};

#[derive(Debug)]
pub enum WorkdirCleanupFailure {
    RegistryUnavailable,
    SessionCloses(Vec<WorkdirError>),
    AttachmentProjection(Box<crate::Error>),
}

impl fmt::Display for WorkdirCleanupFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Presentation comes from the domain projector, never provider Display.
        f.write_str("Workdir cleanup could not be confirmed")
    }
}
impl std::error::Error for WorkdirCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::RegistryUnavailable => None,
            Self::SessionCloses(errors) => {
                errors.first().map(|error| error as &dyn std::error::Error)
            }
            Self::AttachmentProjection(error) => Some(error.as_ref()),
        }
    }
}

impl WorkdirCleanupFailure {
    pub fn session_errors(&self) -> &[WorkdirError] {
        match self {
            Self::SessionCloses(errors) => errors,
            _ => &[],
        }
    }

    pub fn details(&self) -> Vec<WorkdirFailureDetails> {
        match self {
            Self::SessionCloses(errors) => errors
                .iter()
                .map(|error| {
                    let transport = WorkdirTransportError::from_workdir_error(error);
                    WorkdirFailureDetails {
                        operation: WorkdirFailureOperation::SessionClose,
                        classification: transport.code,
                        denial_reason: transport.denial_reason,
                        failure_reason: transport.failure_reason,
                    }
                })
                .collect(),
            Self::RegistryUnavailable => vec![WorkdirFailureDetails {
                operation: WorkdirFailureOperation::SessionClose,
                classification: WorkdirTransportErrorCode::Internal,
                denial_reason: None,
                failure_reason: Some(WorkdirFailureReason::SessionRegistryUnavailable),
            }],
            Self::AttachmentProjection(_) => vec![WorkdirFailureDetails {
                operation: WorkdirFailureOperation::AttachmentProjection,
                classification: WorkdirTransportErrorCode::Internal,
                denial_reason: None,
                failure_reason: Some(WorkdirFailureReason::AttachmentProjectionUnavailable),
            }],
        }
    }
}
