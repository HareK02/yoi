//! Terminal-grant cleanup is a frozen child operation. Inspection never enters
//! this driver; recovery observes an admitted dispatch and never sends it again.
use super::*;
use crate::grant_cleanup_operation::{
    CleanupAdmission, CleanupRequest, CleanupRuntimeTarget, LoadedCleanupOperation,
};
use crate::hosts::{RuntimeOperationResponse, RuntimeOperationStatus};
use workdir::management_failure::WorkdirManagementReason as Reason;

type Guard = Option<Arc<tokio::sync::OwnedMutexGuard<()>>>;

async fn db<T: Send + 'static>(
    api: &WorkspaceApi,
    guard: &Guard,
    reason: Reason,
    f: impl FnOnce(WorkspaceApi) -> ApiResult<T> + Send + 'static,
) -> ApiResult<T> {
    let stage = api.clone();
    let held = guard.clone();
    api.store
        .db_executor()
        .run(move || {
            let _held = held;
            f(stage)
        })
        .await
        .map_err(|error| external_grant_failure::attributed(error.error, reason))
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

fn append(primary: &mut Option<ApiError>, error: ApiError) {
    *primary = Some(match primary.take() {
        Some(primary) => external_grant_failure::combine(primary, error),
        None => error,
    });
}

fn unavailable(
    worker: &RuntimeWorkerRef,
    cause: Option<protocol::control_failure::RuntimeControlCause>,
) -> ApiError {
    Error::WorkdirAttachmentSyncFailed {
        runtime_id: worker.runtime_id.clone(),
        observation: Box::new(RuntimeOperationResponse {
            status: RuntimeOperationStatus::Unknown,
            cause,
            worker: None,
            diagnostics: Vec::new(),
        }),
    }
    .into()
}

async fn retain_failure(
    api: &WorkspaceApi,
    guard: &Guard,
    owner: &mut LoadedCleanupOperation,
    error: ApiError,
    deduplicate: bool,
) -> ApiError {
    let saved = owner.operation().failures();
    let failures =
        external_grant_failure::stored_error(&error, Reason::GrantAttachmentReleaseFailed)
            .into_iter()
            .filter(|failure| {
                !deduplicate
                    || !saved.contains(&protocol::background_failure::StoredFailure::from_storage(
                        failure.as_storage_str().to_owned(),
                    ))
            })
            .collect::<Vec<_>>();
    if failures.is_empty() {
        return error;
    }
    let expected = owner.clone();
    match db(
        api,
        guard,
        Reason::GrantCleanupPersistenceFailed,
        move |api| {
            Ok(api
                .store
                .append_external_workdir_cleanup_failures(&expected, &failures, &now())?)
        },
    )
    .await
    {
        Ok(updated) => {
            *owner = updated;
            error
        }
        Err(followup) => external_grant_failure::combine(error, followup),
    }
}

pub(super) async fn execute(
    api: &WorkspaceApi,
    workdir_id: &str,
    locked: Option<(&RuntimeWorkerRef, &Arc<tokio::sync::OwnedMutexGuard<()>>)>,
    expected_target: Option<&WorkerWorkdirLinkRecord>,
    preceding: Option<ApiError>,
) -> ApiResult<()> {
    // The incoming fence/audit result outlives every fallible cleanup stage.
    // Persistence may fail, but cannot erase it from the live result.
    let result = execute_inner(api, workdir_id, locked, expected_target, preceding.as_ref()).await;
    match (result, preceding) {
        (Err(error), Some(preceding)) => Err(external_grant_failure::combine(error, preceding)),
        (Err(error), None) => Err(error),
        (Ok(()), Some(preceding)) => Err(preceding),
        (Ok(()), None) => Ok(()),
    }
}

async fn execute_inner(
    api: &WorkspaceApi,
    workdir_id: &str,
    locked: Option<(&RuntimeWorkerRef, &Arc<tokio::sync::OwnedMutexGuard<()>>)>,
    expected_target: Option<&WorkerWorkdirLinkRecord>,
    preceding: Option<&ApiError>,
) -> ApiResult<()> {
    let guard = locked.map(|(_, guard)| guard.clone());
    let expected_target = expected_target.cloned();
    let workdir = workdir_id.to_string();
    let (grant, target, state, owner, intent) =
        db(api, &guard, Reason::GrantCleanupQueryFailed, move |api| {
            let workdir = api
                .store
                .get_workdir_registry(&api.config.workspace_id, &workdir)?
                .ok_or_else(|| {
                    Error::RegistryInconsistency("External Workdir disappeared".into())
                })?;
            let grant_id = workdir.source.external_grant_id().ok_or_else(|| {
                Error::RegistryInconsistency("Workdir has no External grant".into())
            })?;
            let grant = api
                .store
                .get_external_workdir_grant(&api.config.workspace_id, grant_id)?
                .ok_or_else(|| Error::RegistryInconsistency("External grant disappeared".into()))?;
            if !matches!(grant.status.as_str(), "revoked" | "expired") {
                return Err(Error::WorkdirAttachmentConflict(
                    "External cleanup authority is not terminal".into(),
                )
                .into());
            }
            let target = api
                .store
                .latest_workdir_worker_link(&grant.workspace_id, &grant.workdir_id)?;
            let identity = |link: &WorkerWorkdirLinkRecord| {
                (
                    link.worker.clone(),
                    link.connection_id.clone(),
                    link.alias.clone(),
                )
            };
            if target.as_ref().map(identity) != expected_target.as_ref().map(identity) {
                return Err(Error::WorkdirAttachmentConflict(
                    "External cleanup connection changed".into(),
                )
                .into());
            }
            let (state, _) = api
                .store
                .external_workdir_cleanup_observation(&grant.workspace_id, grant_id)?;
            let owner = api
                .store
                .external_workdir_cleanup_operation(&grant.workspace_id, grant_id)?;
            let intent = api
                .store
                .external_workdir_cleanup_intent(&grant.workspace_id, grant_id)?;
            Ok((grant, target, state, owner, intent))
        })
        .await?;
    if state == "completed" || (owner.is_none() && intent.is_none()) {
        // A legacy row cannot acquire never-dispatched authority on inspection
        // or restart, even when the present attachment set happens to match.
        return Ok(());
    }
    let preceding_failures = preceding
        .map(|error| external_grant_failure::stored_error(error, Reason::GrantAuditFailed))
        .unwrap_or_default();
    let mut owner = if let Some(mut owner) = owner {
        if !preceding_failures.is_empty() {
            let expected = owner.clone();
            owner = db(
                api,
                &guard,
                Reason::GrantCleanupPersistenceFailed,
                move |api| {
                    Ok(api.store.append_external_workdir_cleanup_failures(
                        &expected,
                        &preceding_failures,
                        &now(),
                    )?)
                },
            )
            .await?;
        }
        owner
    } else {
        let mut intent = intent.ok_or_else(|| {
            Error::RegistryInconsistency("External cleanup intent disappeared".into())
        })?;
        if !preceding_failures.is_empty() {
            let expected = intent.clone();
            intent = db(
                api,
                &guard,
                Reason::GrantCleanupPersistenceFailed,
                move |api| {
                    Ok(api.store.append_external_workdir_cleanup_intent_failures(
                        &expected,
                        &preceding_failures,
                        &now(),
                    )?)
                },
            )
            .await?;
        }
        let frozen_target: ApiResult<_> = async {
            let Some(link) = &target else {
                return Ok(None);
            };
            let worker = link.worker.clone();
            let workdir = grant.workdir_id.clone();
            let attachments = db(api, &guard, Reason::GrantCleanupQueryFailed, move |api| {
                prepare_runtime_worker_workdir_attachments_excluding(&api, &worker, Some(&workdir))
            })
            .await?;
            let stage = api.clone();
            let worker = link.worker.clone();
            let desired = attachments.clone();
            let held = guard.clone();
            let observed = api
                .host_executors
                .host()?
                .run(move || {
                    let _held = held;
                    stage
                        .runtime
                        .observe_worker_workdir_attachments(&worker, desired)
                        .map_err(RuntimeRegistryError::into_error)
                        .map_err(ApiError::from)
                })
                .await?;
            let generation = observed
                .current_generation
                .filter(|generation| !generation.trim().is_empty())
                .ok_or_else(|| unavailable(&link.worker, observed.cause))?;
            Ok(Some(CleanupRuntimeTarget {
                worker: link.worker.clone(),
                connection_id: link.connection_id.clone(),
                alias: link.alias.clone(),
                expected_generation: generation,
                logical_workdir_attachments: attachments,
            }))
        }
        .await;
        let frozen_target = match frozen_target {
            Ok(target) => target,
            Err(error) => {
                let failures =
                    external_grant_failure::stored_error(&error, Reason::GrantCleanupQueryFailed);
                let retained = db(
                    api,
                    &guard,
                    Reason::GrantCleanupPersistenceFailed,
                    move |api| {
                        Ok(api.store.append_external_workdir_cleanup_intent_failures(
                            &intent,
                            &failures,
                            &now(),
                        )?)
                    },
                )
                .await;
                return Err(match retained {
                    Ok(_) => error,
                    Err(followup) => external_grant_failure::combine(error, followup),
                });
            }
        };
        let request = CleanupRequest {
            operation_id: Uuid::now_v7().to_string(),
            workspace_id: grant.workspace_id.clone(),
            grant_id: grant.grant_id.clone(),
            provider_instance_id: grant.provider_instance_id.clone(),
            grant_generation: grant.generation,
            working_directory_id: grant.workdir_id.clone(),
            target: frozen_target,
        };
        match db(
            api,
            &guard,
            Reason::GrantCleanupPersistenceFailed,
            move |api| {
                Ok(api
                    .store
                    .begin_external_workdir_cleanup_operation(&request, &[], &now())?)
            },
        )
        .await?
        {
            CleanupAdmission::Admitted(owner) | CleanupAdmission::Existing(owner) => owner,
        }
    };
    if owner.operation().settled {
        return Ok(());
    }
    let mut primary = None;
    let result: ApiResult<()> = async {
        if !owner.operation().release_confirmed {
            let expected = owner.clone();
            db(
                api,
                &guard,
                Reason::GrantAttachmentReleaseFailed,
                move |api| {
                    Ok(api
                        .store
                        .validate_external_workdir_cleanup_release(&expected)?)
                },
            )
            .await?;
            if let Some(target) = &owner.operation().request.target {
                close_current_worker_attachment_session_locked(api, &target.worker).await?;
                close_current_worker_command_sessions_for_alias_locked(
                    api,
                    &target.worker,
                    &target.alias,
                )
                .await?;
            }
            let expected = owner.clone();
            owner = db(
                api,
                &guard,
                Reason::GrantCleanupPersistenceFailed,
                move |api| {
                    Ok(api
                        .store
                        .confirm_external_workdir_cleanup_release(&expected, &now())?)
                },
            )
            .await?;
            #[cfg(test)]
            if owner.operation().request.target.is_some()
                && take_external_workdir_cleanup_failure(workdir_id)
            {
                return Err(Error::RuntimeOperationFailed {
                    runtime_id: "workspace-backend".into(),
                    code: "injected_external_workdir_cleanup_failure".into(),
                    message: "injected External Workdir attachment cleanup failure".into(),
                }
                .into());
            }
        }
        if let Some(target) = owner.operation().request.target.clone() {
            let request = owner.operation().request.runtime_request().ok_or_else(|| {
                Error::RegistryInconsistency("External cleanup request missing".into())
            })?;
            if !owner.operation().runtime_dispatched {
                let expected = owner.clone();
                owner = db(
                    api,
                    &guard,
                    Reason::GrantCleanupPersistenceFailed,
                    move |api| {
                        Ok(api
                            .store
                            .mark_external_workdir_cleanup_runtime_dispatched(&expected, &now())?)
                    },
                )
                .await?;
                #[cfg(test)]
                record_external_workdir_cleanup_reconciliation(workdir_id);
                let stage = api.clone();
                let worker = target.worker.clone();
                let body = request.clone();
                let held = guard.clone();
                let dispatched = api
                    .host_executors
                    .host()?
                    .run(move || {
                        let _held = held;
                        stage
                            .runtime
                            .apply_worker_workdir_attachments_operation(&worker, body)
                            .map_err(RuntimeRegistryError::into_error)
                            .map_err(ApiError::from)
                    })
                    .await;
                let response = match dispatched {
                    Ok(response) => response.response,
                    Err(error) => {
                        append(&mut primary, error);
                        RuntimeOperationResponse {
                            status: RuntimeOperationStatus::Unknown,
                            cause: None,
                            worker: None,
                            diagnostics: Vec::new(),
                        }
                    }
                };
                let status = response.status;
                let failures = if status != RuntimeOperationStatus::Accepted {
                    let error = ApiError::from(Error::WorkdirAttachmentSyncFailed {
                        runtime_id: target.worker.runtime_id.clone(),
                        observation: Box::new(response),
                    });
                    let failures = external_grant_failure::stored_error(
                        &error,
                        Reason::GrantAttachmentReleaseFailed,
                    );
                    append(&mut primary, error);
                    failures
                } else {
                    Vec::new()
                };
                let expected = owner.clone();
                owner = db(
                    api,
                    &guard,
                    Reason::GrantCleanupPersistenceFailed,
                    move |api| {
                        Ok(api
                            .store
                            .record_external_workdir_cleanup_runtime_observation(
                                &expected,
                                status,
                                &failures,
                                &now(),
                            )?)
                    },
                )
                .await?;
            }
            // A committed checkpoint, including a lost immediate result, permits
            // only this exact-operation query. Absent/conflict never authorize resend.
            let stage = api.clone();
            let worker = target.worker.clone();
            let held = guard.clone();
            let observed = api
                .host_executors
                .host()?
                .run(move || {
                    let _held = held;
                    stage
                        .runtime
                        .observe_worker_workdir_attachments_operation(&worker, request)
                        .map_err(RuntimeRegistryError::into_error)
                        .map_err(ApiError::from)
                })
                .await?;
            let proof = observed
                .observation
                .ok_or_else(|| unavailable(&target.worker, observed.cause))?;
            let expected = owner.clone();
            owner = db(
                api,
                &guard,
                Reason::GrantCleanupPersistenceFailed,
                move |api| {
                    api.store
                        .settle_external_workdir_cleanup_operation(&expected, Some(&proof), &now())
                        .map_err(|error| {
                            if matches!(error, Error::WorkdirAttachmentConflict(_)) {
                                external_grant_failure::attributed(
                                    error,
                                    Reason::GrantCleanupOutcomeUnknown,
                                )
                            } else {
                                ApiError::from(error)
                            }
                        })
                },
            )
            .await?;
            if let Err(error) = api.worker_projection.refresh_async(&target.worker).await {
                append(&mut primary, error.into());
            }
        } else {
            let expected = owner.clone();
            owner = db(
                api,
                &guard,
                Reason::GrantCleanupPersistenceFailed,
                move |api| {
                    Ok(api.store.settle_external_workdir_cleanup_operation(
                        &expected,
                        None,
                        &now(),
                    )?)
                },
            )
            .await?;
        }
        let grant_id = grant.grant_id.clone();
        if let Err(error) = db(api, &guard, Reason::GrantAuditFailed, move |api| {
            Ok(api.store.record_external_workdir_audit(
                &grant.workspace_id,
                "system",
                "workspace-backend",
                "external_workdir_cleanup_completed",
                &grant_id,
                None,
                "completed",
                "External Workdir cleanup confirmed",
                &now(),
            )?)
        })
        .await
        {
            append(&mut primary, error);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        append(
            &mut primary,
            external_grant_failure::attributed(error.error, Reason::GrantAttachmentReleaseFailed),
        );
    }
    match primary {
        Some(error) => {
            let mut error = retain_failure(api, &guard, &mut owner, error, true).await;
            let request = owner.operation().request.clone();
            if let Err(audit) = db(api, &guard, Reason::GrantAuditFailed, move |api| {
                Ok(api.store.record_external_workdir_audit(
                    &request.workspace_id,
                    "system",
                    "workspace-backend",
                    "external_workdir_cleanup_attention_required",
                    &request.grant_id,
                    Some(&request.operation_id),
                    "attention_required",
                    "External Workdir cleanup needs inspection",
                    &now(),
                )?)
            })
            .await
            {
                let audit = retain_failure(api, &guard, &mut owner, audit, false).await;
                error = external_grant_failure::combine(error, audit);
            }
            Err(error)
        }
        None => Ok(()),
    }
}
