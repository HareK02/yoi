//! Grant-owned observations reuse the shared storage codec. Cleanup failures do
//! not undo a terminal authority fence or authorize another grant mutation.
use super::*;
use error_contract::{Browser, FailureFacts, Storage, project_failure};
use protocol::background_failure::{BackgroundFailure, StoredFailure};
use protocol::startup_failure::StartupFailureAttachment;
use workdir::failure::{WorkdirFailureDetails, WorkdirFailureVisibility};
use workdir::management_failure::{
    WorkdirManagementFailure, WorkdirManagementOperation as Op, WorkdirManagementReason as Reason,
    WorkdirManagementVisibility,
};

fn management(operation: Op, reason: Reason) -> WorkdirManagementFailure {
    WorkdirManagementFailure { operation, reason }
}

fn storage_details(error: &Error, fallback: Reason, operation: Op) -> Vec<BackgroundFailure> {
    match error {
        Error::ExternalWorkdirConnectionSource(source) => {
            storage_details(source, fallback, operation)
        }
        Error::ExternalWorkdirObservationFailed {
            operation,
            reason,
            source,
        } => storage_details(source, *reason, *operation),
        Error::ExternalWorkdirCleanupFailed {
            primary,
            supplementary,
        } => {
            let mut failures = storage_details(primary, fallback, operation);
            for error in supplementary {
                failures.extend(storage_details(error, fallback, operation));
            }
            failures
        }
        Error::WorkdirAttachmentSyncFailed { observation, .. } => {
            if let Some(cause) = observation.cause {
                FailureFacts::new(cause)
                    .and_then(|facts| project_failure::<Storage, _>(&facts, &()))
                    .and_then(|projected| BackgroundFailure::from_projected(&projected))
                    .into_iter()
                    .collect()
            } else {
                FailureFacts::new(management(
                    operation,
                    Reason::AttachmentRuntimeResultUnavailable,
                ))
                .and_then(|facts| {
                    project_failure::<Storage, _>(
                        &facts,
                        &WorkdirManagementVisibility {
                            can_view_cause: true,
                        },
                    )
                })
                .and_then(|projected| BackgroundFailure::from_projected(&projected))
                .into_iter()
                .collect()
            }
        }
        Error::WorkdirSessionCloseFailed { failure, .. } => failure
            .details()
            .into_iter()
            .filter_map(|details| {
                FailureFacts::new(details)
                    .and_then(|facts| {
                        project_failure::<Storage, _>(
                            &facts,
                            &WorkdirFailureVisibility {
                                can_view_cause: true,
                            },
                        )
                    })
                    .and_then(|projected| BackgroundFailure::from_projected(&projected))
                    .ok()
            })
            .collect(),
        _ => {
            let reason = if matches!(error, Error::DbOperationOutcomeUnknown) {
                if operation == Op::GrantConnect {
                    Reason::GrantStateOutcomeUnknown
                } else {
                    Reason::GrantCleanupOutcomeUnknown
                }
            } else if let Error::ExternalWorkdirConnectionClosed { reason } = error {
                *reason
            } else {
                fallback
            };
            FailureFacts::new(management(operation, reason))
                .and_then(|facts| {
                    project_failure::<Storage, _>(
                        &facts,
                        &WorkdirManagementVisibility {
                            can_view_cause: true,
                        },
                    )
                })
                .and_then(|projected| BackgroundFailure::from_projected(&projected))
                .into_iter()
                .collect()
        }
    }
}

pub(super) fn stored_error(error: &ApiError, fallback: Reason) -> Vec<BackgroundFailure> {
    storage_details(&error.error, fallback, Op::GrantCleanup)
}

pub(super) fn publish(stored: &StoredFailure) -> Option<StartupFailureAttachment> {
    if let Ok(projected) =
        stored.project_for::<Browser, WorkdirFailureDetails>(&WorkdirFailureVisibility {
            can_view_cause: true,
        })
    {
        return StartupFailureAttachment::from_projected(&projected).ok();
    }
    if let Ok(projected) =
        stored.project_for::<Browser, protocol::control_failure::RuntimeControlCause>(&())
    {
        return StartupFailureAttachment::from_projected(&projected).ok();
    }
    stored
        .project_for::<Browser, WorkdirManagementFailure>(&WorkdirManagementVisibility {
            can_view_cause: true,
        })
        .and_then(|projected| StartupFailureAttachment::from_projected(&projected))
        .ok()
}

pub(super) fn public_error(error: &ApiError, fallback: Reason) -> Vec<StartupFailureAttachment> {
    stored_error(error, fallback)
        .iter()
        .filter_map(|failure| {
            publish(&StoredFailure::from_storage(
                failure.as_storage_str().into(),
            ))
        })
        .collect()
}

pub(super) fn observation(
    api: &WorkspaceApi,
    grant: &ExternalWorkdirGrantRecord,
) -> (String, Vec<StartupFailureAttachment>) {
    match api
        .store
        .external_workdir_cleanup_observation(&grant.workspace_id, &grant.grant_id)
    {
        Ok((state, failures)) => (state, failures.iter().filter_map(publish).collect()),
        Err(error) => (
            "unconfirmed".into(),
            public_error(&error.into(), Reason::GrantCleanupQueryFailed),
        ),
    }
}

pub(super) fn append_live(response: &mut ExternalWorkdirGrantResponse, error: &ApiError) {
    let live = public_error(error, Reason::GrantAttachmentReleaseFailed);
    for (index, failure) in live.iter().enumerate() {
        let observed_count = live[..=index]
            .iter()
            .filter(|value| *value == failure)
            .count();
        if response
            .cleanup_failures
            .iter()
            .filter(|value| *value == failure)
            .count()
            < observed_count
        {
            response.cleanup_failures.push(failure.clone());
        }
    }
}

pub(super) fn combine(primary: ApiError, followup: ApiError) -> ApiError {
    let mut result = ApiError::from(Error::ExternalWorkdirCleanupFailed {
        primary: Box::new(primary.error),
        supplementary: vec![followup.error],
    });
    result.diagnostics = primary.diagnostics;
    result.diagnostics.extend(followup.diagnostics);
    result
}

pub(super) fn attributed(error: Error, reason: Reason) -> ApiError {
    ApiError::workdir_management_source(
        Error::ExternalWorkdirObservationFailed {
            operation: Op::GrantCleanup,
            reason,
            source: Box::new(error),
        },
        Op::GrantCleanup,
        reason,
    )
}

pub(super) fn connection_error(error: Error, reason: Reason) -> ApiError {
    ApiError::workdir_management_source(
        Error::ExternalWorkdirObservationFailed {
            operation: Op::GrantConnect,
            reason,
            source: Box::new(error),
        },
        Op::GrantConnect,
        reason,
    )
}

pub(super) fn connection_stored(error: &ApiError) -> Vec<BackgroundFailure> {
    storage_details(
        &error.error,
        Reason::GrantConnectionStateFailed,
        Op::GrantConnect,
    )
}

pub(super) fn telemetry(error: &ApiError) {
    log_stored(connection_stored(error));
}

pub(super) fn cleanup_telemetry(error: &ApiError) {
    log_stored(stored_error(error, Reason::GrantAttachmentReleaseFailed));
}

pub(super) fn outcome_unknown(error: &ApiError) -> bool {
    fn unknown(error: &Error) -> bool {
        match error {
            Error::DbOperationOutcomeUnknown => true,
            Error::ExternalWorkdirObservationFailed { source, .. } => unknown(source),
            Error::ExternalWorkdirConnectionSource(source) => unknown(source),
            Error::ExternalWorkdirCleanupFailed {
                primary,
                supplementary,
            } => unknown(primary) || supplementary.iter().any(unknown),
            _ => error.attachment_outcome_unknown(),
        }
    }
    unknown(&error.error)
}

fn log_stored(failures: Vec<BackgroundFailure>) {
    for failure in failures {
        let stored = StoredFailure::from_storage(failure.as_storage_str().into());
        if let Ok(projected) = stored.project_for::<error_contract::Telemetry, WorkdirManagementFailure>(
            &WorkdirManagementVisibility { can_view_cause: true },
        ) {
            tracing::error!(operation = ?projected.details().operation, reason = %projected.details().reason,
                explanation = projected.explanation(), "External Workdir grant observation");
        } else if let Ok(projected) = stored.project_for::<error_contract::Telemetry, WorkdirFailureDetails>(
            &WorkdirFailureVisibility { can_view_cause: true },
        ) {
            tracing::error!(explanation = projected.explanation(), "External Workdir connection cleanup observation");
        } else if let Ok(projected) = stored.project_for::<error_contract::Telemetry, protocol::control_failure::RuntimeControlCause>(&()) {
            tracing::error!(reason = projected.details().reason(), explanation = projected.explanation(), "External Workdir Runtime cleanup observation");
        }
    }
}

/// Save evidence once, retaining the original source and any observation failure
/// independently. Failure here never permits replay of an authority mutation.
pub(super) fn save_connection(
    api: &WorkspaceApi,
    grant: &ExternalWorkdirGrantRecord,
    generation: u64,
    reason: server_api::ExternalWorkdirConnectionClosedReason,
    mut primary: Option<ApiError>,
) -> Option<ApiError> {
    let failures = primary.as_ref().map(connection_stored).unwrap_or_default();
    let saved = encode(&failures).and_then(|raw| {
        api.store.record_external_workdir_connection_observation(
            grant,
            generation,
            reason,
            raw.as_deref(),
            &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
        )
    });
    if let Err(error) = saved {
        let followup = connection_error(error, Reason::GrantConnectionPersistenceFailed);
        primary = Some(match primary {
            Some(error) => combine(error, followup),
            None => followup,
        });
    }
    if let Some(error) = &primary {
        telemetry(error);
    }
    primary
}

pub(super) fn registration_failed(
    api: &WorkspaceApi,
    grant: &ExternalWorkdirGrantRecord,
    connection: &ExternalProviderConnection,
    source: Arc<Error>,
) -> Option<ApiError> {
    api.external_workdir_providers
        .lock()
        .expect("External Workdir provider registry poisoned")
        .disconnect(&connection.grant_id, connection.generation);
    let mut primary = connection_error(
        Error::ExternalWorkdirConnectionSource(source),
        Reason::GrantConnectionTransportFailed,
    );
    if let Err(error) = api.store.update_external_workdir_grant_state(
        &grant.workspace_id,
        &grant.grant_id,
        &connection.provider_instance_id,
        connection.generation,
        "offline",
        &Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true),
    ) {
        primary = combine(
            primary,
            connection_error(error, Reason::GrantConnectionStateFailed),
        );
    }
    save_connection(
        api,
        grant,
        connection.generation,
        server_api::ExternalWorkdirConnectionClosedReason::RegistrationSendFailed,
        Some(primary),
    )
}

pub(super) async fn observe_connection(
    api: &WorkspaceApi,
    grant: &ExternalWorkdirGrantRecord,
    generation: u64,
    reason: server_api::ExternalWorkdirConnectionClosedReason,
    primary: ApiError,
) {
    let stage_api = api.clone();
    let grant = grant.clone();
    // Log the typed primary before admission; executor rejection must not hide it.
    telemetry(&primary);
    if let Err(error) = api
        .store
        .db_executor()
        .run(move || -> Result<()> {
            let _retained = save_connection(&stage_api, &grant, generation, reason, Some(primary));
            Ok(())
        })
        .await
    {
        telemetry(&connection_error(
            error,
            Reason::GrantConnectionPersistenceFailed,
        ));
    }
}

pub(super) fn connection_observations(
    api: &WorkspaceApi,
    grant: &ExternalWorkdirGrantRecord,
) -> Vec<server_api::ExternalWorkdirConnectionObservation> {
    match api
        .store
        .external_workdir_connection_observations(&grant.workspace_id, &grant.grant_id)
    {
        Ok(observations) => observations
            .into_iter()
            .filter(|observation| observation.grant_generation == grant.generation)
            .map(
                |observation| server_api::ExternalWorkdirConnectionObservation {
                    grant_generation: observation.grant_generation,
                    connection_generation: observation.connection_generation,
                    closed_reason: Some(observation.closed_reason),
                    failures: observation.failures.iter().filter_map(publish).collect(),
                },
            )
            .collect(),
        Err(error) => vec![server_api::ExternalWorkdirConnectionObservation {
            grant_generation: grant.generation,
            connection_generation: grant.generation,
            closed_reason: None,
            failures: connection_stored(&connection_error(
                error,
                Reason::GrantConnectionQueryFailed,
            ))
            .iter()
            .filter_map(|failure| {
                publish(&StoredFailure::from_storage(
                    failure.as_storage_str().into(),
                ))
            })
            .collect(),
        }],
    }
}

pub(super) fn encode(failures: &[BackgroundFailure]) -> Result<Option<String>> {
    if failures.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(
        &failures
            .iter()
            .map(BackgroundFailure::as_storage_str)
            .collect::<Vec<_>>(),
    )
    .map(Some)
    .map_err(|_| Error::RegistryInconsistency("grant cleanup evidence encoding failed".into()))
}
