//! Configuration-owned publication continuation. SQL observations do not schedule
//! work, amend adoption, or turn an unconfirmed notification into a retry.
use super::*;
use crate::config_publication::{PublicationIdentity, PublicationRecord, envelope};
use error_contract::{Browser, FailureFacts, Storage, project_failure};
use protocol::background_failure::BackgroundFailure;
use protocol::control_failure::RuntimeControlCause;
use server_api::{
    ConfigurationPublicationFailure as PublicFailure,
    ConfigurationPublicationObservation as PublicObservation,
    ConfigurationPublicationOutcome as Outcome, ConfigurationPublicationResponse,
    ConfigurationPublicationStage as Stage,
};

fn storage_error() -> Error {
    Error::RegistryInconsistency("configuration publication cause projection unavailable".into())
}
fn config_cause(cause: ConfigurationFailure) -> Result<BackgroundFailure> {
    let facts = FailureFacts::new(cause).map_err(|_| storage_error())?;
    let projected = project_failure::<Storage, _>(
        &facts,
        &server_api::configuration_failure::ConfigurationFailureVisibility {
            can_view_cause: true,
        },
    )
    .map_err(|_| storage_error())?;
    BackgroundFailure::from_projected(&projected).map_err(|_| storage_error())
}
fn runtime_cause(cause: RuntimeControlCause) -> Result<BackgroundFailure> {
    let facts = FailureFacts::new(cause).map_err(|_| storage_error())?;
    let projected = project_failure::<Storage, _>(&facts, &()).map_err(|_| storage_error())?;
    BackgroundFailure::from_projected(&projected).map_err(|_| storage_error())
}
fn record(
    api: &WorkspaceApi,
    identity: &PublicationIdentity,
    stage: Stage,
    runtime_id: Option<&str>,
    projection_digest: Option<&str>,
    cause: Result<Option<BackgroundFailure>>,
) {
    let stored = cause.and_then(|cause| {
        api.config_store.record_configuration_publication(
            identity,
            stage,
            runtime_id,
            projection_digest,
            if cause.is_some() {
                Outcome::Unconfirmed
            } else {
                Outcome::Observed
            },
            cause.as_ref(),
        )
    });
    if stored.is_err() {
        // An evidence write failure cannot undo adoption or justify another send.
        // Do not print database/provider errors or captured configuration content.
        tracing::warn!(operation_id = %identity.receipt.operation_id, ?stage,
            "Configuration publication observation could not be retained; consult the adoption receipt separately");
    }
}

pub(super) fn dispatch_publication(
    api: &WorkspaceApi,
    account: String,
    identity: PublicationIdentity,
) {
    let publication_api = api.clone();
    let continuation_identity = identity.clone();
    let admission = api.host_executors.host().and_then(|executor| {
        executor.dispatch(move || {
            let identity = continuation_identity;
            let api = publication_api;
            // An old receipt is historical evidence, not a grant to publish old source
            // after an owner change or a newer adoption while this job was queued.
            let projection = (|| {
                let current = api
                    .config_store
                    .configuration_current(&api.config.workspace_id, &account)?;
                if current.history_head.as_deref() != Some(identity.receipt.history_head.as_str())
                    || current.state != identity.receipt.state
                {
                    return Err(ConfigurationFailure::new(
                        FailureOperation::Publication,
                        FailureReason::EditingHeadChanged,
                    )
                    .into());
                }
                api.prompt_projection_cache
                    .resolve(&api.config.workspace_id, &current.state)
            })();
            match projection {
                Ok(projection) => {
                    record(
                        &api,
                        &identity,
                        Stage::Projection,
                        None,
                        Some(&projection.projection_digest),
                        Ok(None),
                    );
                    // Every Runtime result is retained, independently of presentation limits.
                    // A timeout/Unknown is evidence only: no resend or new operation is created.
                    for observation in api
                        .runtime
                        .observe_workspace_prompt_projection((*projection).clone())
                    {
                        record(
                            &api,
                            &identity,
                            Stage::Runtime,
                            Some(&observation.runtime_id),
                            Some(&projection.projection_digest),
                            observation.result.err().map(runtime_cause).transpose(),
                        );
                    }
                }
                Err(error) => {
                    let cause = match error {
                        Error::Configuration(cause) => {
                            cause.with_operation(FailureOperation::Publication)
                        }
                        error => ConfigurationFailure::from_publication(
                            FailureReason::PublicationProjectionUnavailable,
                            error,
                        ),
                    };
                    record(
                        &api,
                        &identity,
                        Stage::Projection,
                        None,
                        None,
                        config_cause(cause).map(Some),
                    );
                }
            }
        })
    });
    // DbExecutor::dispatch explicitly guarantees Err is not admitted. Success is
    // queue acceptance only; completion may race this independent observation.
    let cause = admission
        .err()
        .map(|error| {
            config_cause(ConfigurationFailure::from_publication(
                FailureReason::PublicationDispatchUnavailable,
                error,
            ))
        })
        .transpose();
    record(api, &identity, Stage::Dispatch, None, None, cause);
}

pub(super) fn publication_to_api(
    record: PublicationRecord,
) -> Result<ConfigurationPublicationResponse> {
    let visibility = server_api::configuration_failure::ConfigurationFailureVisibility {
        can_view_cause: true,
    };
    let observations = record
        .observations
        .into_iter()
        .map(|observation| {
            // Only reached after owner_operation AND the store's current owner check.
            // Stored explanation, context and audience labels never authorize disclosure.
            let failure = observation
                .failure
                .as_ref()
                .map(|stored| {
                    if let Ok(projected) = stored
                        .project_for::<Browser, server_api::configuration_failure::ConfigurationFailureDetails>(
                            &visibility,
                        )
                    {
                        return envelope(&projected)
                            .map(|failure| PublicFailure::Configuration { failure });
                    }
                    if let Ok(projected) = stored.project_for::<Browser, RuntimeControlCause>(&()) {
                        return envelope(&projected)
                            .map(|failure| PublicFailure::Runtime { failure });
                    }
                    Ok(PublicFailure::Unavailable)
                })
                .transpose()?;
            Ok(PublicObservation {
                stage: observation.stage,
                runtime_id: observation.runtime_id,
                projection_digest: observation.projection_digest,
                outcome: observation.outcome,
                failure,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let receipt = record.identity.receipt;
    Ok(ConfigurationPublicationResponse {
        workspace_id: receipt.workspace_id,
        operation_id: receipt.operation_id,
        request_fingerprint: record.identity.request_fingerprint,
        history_head: receipt.history_head,
        source_digest: receipt.state.snapshot.digest,
        observations,
    })
}
