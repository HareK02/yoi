//! Attachment compensation preserves the primary observation and the connection
//! lifetime it may undo. Unknown outcomes never authorize an inverse mutation.
use super::*;
use server_api::workdir_management_error::{
    WorkdirAttachmentFailureStage as Stage, WorkdirAttachmentRelatedFailure,
    WorkdirManagementCause as Cause,
};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation as Op, WorkdirManagementReason as Reason,
    WorkdirManagementVisibility,
};

impl Error {
    pub(super) fn attachment_outcome_unknown(&self) -> bool {
        match self {
            Error::DbOperationOutcomeUnknown => true,
            Error::WorkdirAttachmentSyncFailed { observation, .. } => {
                observation.status == crate::hosts::RuntimeOperationStatus::Unknown
            }
            Error::WorkdirSessionOpenFailed { transport, .. } => {
                transport.code == workdir::http::WorkdirTransportErrorCode::OutcomeUnknown
            }
            Error::WorkdirSessionCloseFailed { failure, .. } => {
                failure.details().iter().any(|cause| {
                    cause.classification == workdir::http::WorkdirTransportErrorCode::OutcomeUnknown
                })
            }
            _ => false,
        }
    }
}

fn source_cause(error: &Error, operation: Op) -> Option<Cause> {
    let reason = match error {
        Error::WorkdirAttachmentSyncFailed { observation, .. } => {
            return Some(match observation.cause {
                Some(cause) => Cause::Runtime(cause),
                None => Cause::Management(WorkdirManagementFailure {
                    operation,
                    reason: Reason::AttachmentRuntimeResultUnavailable,
                }),
            });
        }
        Error::WorkdirSessionOpenFailed { transport, .. } => {
            return Some(Cause::Session(workdir::failure::WorkdirFailureDetails {
                operation: workdir::failure::WorkdirFailureOperation::SessionOpen,
                classification: transport.code,
                denial_reason: transport.denial_reason,
                failure_reason: transport.failure_reason,
            }));
        }
        Error::WorkdirSessionCloseFailed { failure, .. } => {
            return failure.details().first().copied().map(Cause::Session);
        }
        Error::ExternalWorkdirObservationFailed {
            operation: source_operation,
            reason,
            source,
        } => {
            let nested = matches!(
                source.as_ref(),
                Error::ExternalWorkdirObservationFailed { .. }
                    | Error::ExternalWorkdirCleanupFailed { .. }
                    | Error::WorkdirAttachmentSyncFailed { .. }
                    | Error::WorkdirSessionOpenFailed { .. }
                    | Error::WorkdirSessionCloseFailed { .. }
            )
            .then(|| source_cause(source, operation))
            .flatten();
            return nested.or(Some(Cause::Management(WorkdirManagementFailure {
                operation: *source_operation,
                reason: *reason,
            })));
        }
        Error::ExternalWorkdirCleanupFailed { primary, .. } => {
            return source_cause(primary, operation);
        }
        Error::WorkdirOccupied { .. } => Some(Reason::Occupied),
        Error::WorkdirAttachmentConflict(_) => Some(Reason::AttachmentConflict),
        Error::DbOperationOutcomeUnknown => Some(Reason::AttachmentStateOutcomeUnknown),
        Error::Sqlite(_) | Error::DbCapacityExceeded | Error::DbExecutorUnavailable => {
            Some(Reason::AttachmentPersistenceFailed)
        }
        _ => None,
    };
    reason.map(|reason| Cause::Management(WorkdirManagementFailure { operation, reason }))
}

fn push_related_source(
    related: &mut Vec<WorkdirAttachmentRelatedFailure>,
    stage: Stage,
    error: &Error,
    operation: Op,
) {
    match error {
        Error::ExternalWorkdirCleanupFailed {
            primary,
            supplementary,
        } => {
            push_related_source(related, stage, primary, operation);
            for error in supplementary {
                push_related_source(related, stage, error, operation);
            }
        }
        Error::WorkdirSessionCloseFailed { failure, .. } => {
            for cause in failure.details() {
                related.push(related_failure(stage, 502, Some(Cause::Session(cause))));
            }
        }
        _ => related.push(related_failure(
            stage,
            api_error_status(error).as_u16(),
            source_cause(error, operation),
        )),
    }
}

fn push_supplementary(
    related: &mut Vec<WorkdirAttachmentRelatedFailure>,
    error: &Error,
    operation: Op,
) {
    if let Error::ExternalWorkdirCleanupFailed {
        primary,
        supplementary,
    } = error
    {
        push_supplementary(related, primary, operation);
        for error in supplementary {
            push_related_source(related, Stage::GrantCleanup, error, operation);
        }
    }
}

impl ApiError {
    pub(super) fn attachment_outcome_unknown(&self) -> bool {
        self.error.attachment_outcome_unknown()
    }

    fn attachment_cause(&self, operation: Op) -> Option<Cause> {
        let producer = self
            .diagnostics
            .iter()
            .filter_map(|d| d.workdir)
            .find(|cause| cause.operation == operation)
            .map(Cause::Management);
        // A legacy conflict class cannot replace the producer's more specific
        // typed reason. Native Runtime/session failures remain the primary cause.
        if matches!(
            self.error,
            Error::WorkdirAttachmentConflict(_) | Error::WorkdirOccupied { .. }
        ) {
            producer.or_else(|| source_cause(&self.error, operation))
        } else {
            source_cause(&self.error, operation).or(producer)
        }
    }

    pub(super) fn attachment_projection_failure(error: Error, operation: Op) -> Self {
        Self::workdir_management_source(error, operation, Reason::AttachmentProjectionFailed)
    }

    pub(super) fn into_workdir_attachment_api_error(
        self,
        operation: Op,
    ) -> server_api::WorkdirManagementApiError {
        use error_contract::{Browser, FailureFacts, project_failure};
        let visibility = WorkdirManagementVisibility {
            can_view_cause: true,
        };
        let primary = self.attachment_cause(operation).and_then(|cause| {
            FailureFacts::new(cause)
                .and_then(|facts| project_failure::<Browser, _>(&facts, &visibility))
                .ok()
        });
        let Some(primary) = primary else {
            return self.into_repository_api_error().into();
        };
        let mut related = Vec::new();
        // Session close aggregates retain every provider error, not only Error::source().
        if let Error::WorkdirSessionCloseFailed { failure, .. } = &self.error {
            for cause in failure.details().into_iter().skip(1) {
                related.push(related_failure(
                    Stage::SessionClose,
                    502,
                    Some(Cause::Session(cause)),
                ));
            }
        }
        push_supplementary(&mut related, &self.error, operation);
        for (stage, error) in &self.attachment_failures {
            if let Error::WorkdirSessionCloseFailed { failure, .. } = &error.error {
                for cause in failure.details() {
                    related.push(related_failure(*stage, 502, Some(Cause::Session(cause))));
                }
            } else {
                related.push(related_failure(
                    *stage,
                    api_error_status(&error.error).as_u16(),
                    error.attachment_cause(operation),
                ));
            }
        }
        // Runtime diagnostics may include mixed-version private prose. The projected
        // domain cause is the explanation; do not forward a second raw explanation.
        let legacy = server_api::RepositoryApiError::new(
            502,
            "workdir_attachment",
            primary.explanation(),
            Vec::new(),
        );
        server_api::WorkdirManagementApiError::projected_attachment(legacy, primary, related)
    }
}

fn related_failure(
    stage: Stage,
    http_status: u16,
    cause: Option<Cause>,
) -> WorkdirAttachmentRelatedFailure {
    let failure = cause.and_then(|cause| {
        error_contract::FailureFacts::new(cause)
            .and_then(|facts| {
                error_contract::project_failure::<error_contract::Browser, _>(
                    &facts,
                    &WorkdirManagementVisibility {
                        can_view_cause: true,
                    },
                )
            })
            .and_then(|projected| {
                protocol::startup_failure::StartupFailureAttachment::from_projected(&projected)
            })
            .ok()
    });
    WorkdirAttachmentRelatedFailure {
        stage,
        http_status,
        failure,
    }
}

pub(super) enum Compensation {
    DetachNewLink,
    RestorePreviousLink,
}

pub(super) async fn compensate_attachment(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
    link: &WorkerWorkdirLinkRecord,
    guard: &Arc<tokio::sync::OwnedMutexGuard<()>>,
    action: Compensation,
    synchronize_runtime: bool,
    refresh_session: bool,
    mut primary: ApiError,
) -> ApiError {
    if primary.attachment_outcome_unknown() {
        return primary;
    }
    let (stage, outcome) = match action {
        Compensation::DetachNewLink => {
            let link = link.clone();
            (
                Stage::DetachBackendLink,
                run_worker_workdir_db_stage_locked(api, guard, move |api| {
                    api.store
                        .detach_worker_workdir_connection(
                            &api.config.workspace_id,
                            &link.worker,
                            &link.alias,
                            &link.connection_id,
                            &Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
                        )
                        .map(|_| ())
                })
                .await,
            )
        }
        Compensation::RestorePreviousLink => (
            Stage::RestoreBackendLink,
            restore_worker_workdir_connection_stage_locked(api, link, guard).await,
        ),
    };
    if let Err(error) = outcome {
        primary.attachment_failures.push((stage, error.into()));
        // A failed CAS or an uncertain ledger update cannot authorize publishing
        // an assumed attachment set to Runtime or opening a replacement session.
        return primary;
    }
    if synchronize_runtime {
        if let Err(error) = sync_runtime_worker_workdir_attachments_locked(api, worker, guard).await
        {
            primary
                .attachment_failures
                .push((Stage::SyncRuntimeAttachments, error));
            return primary;
        }
    }
    if refresh_session {
        if let Err(error) = refresh_current_worker_session_locked(api, worker, guard).await {
            primary
                .attachment_failures
                .push((Stage::RefreshSession, error.into()));
        }
    }
    primary
}
