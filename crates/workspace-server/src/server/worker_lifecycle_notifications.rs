//! Recover execution-owned facts into the existing notification ledger. Subscription
//! updates only wake this bounded recovery; neither Idle nor a lost connection is
//! interpreted as successful work or confirmed Stop.
//!
//! Recovery requires retained Runtime/Session facts. Process loss before a terminal
//! commit cannot be reconstructed as a successful run; an unavailable source stays
//! unknown, and later sweeps may recover only facts that actually committed. Once
//! captured, the common ledger retains delivery evidence independently of Session
//! retention. A reference is not a retention pin and can become unavailable.
//!
//! Ordinary spawners remain fixed recipients. Subject spawners resolve their current
//! body and recheck its own observe rights, without transferring the former body's
//! grants or implicitly restoring a stopped body. After request preparation, uncertain
//! acceptance is pinned to that exact recipient: replacement does not justify sending
//! the same fact to another body. Deferred failures remain visible in diagnostics.
use super::*;
use crate::store::{WorkerLifecycleEvent, WorkerLifecycleOutcome, WorkerLifecycleScan};

fn timestamp(ms: u64) -> Result<String> {
    i64::try_from(ms)
        .ok()
        .and_then(chrono::DateTime::<Utc>::from_timestamp_millis)
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| Error::Store("Invalid committed Worker event time".into()))
}

fn capture_scan(api: &WorkspaceApi, scan: &WorkerLifecycleScan) -> Result<()> {
    use session_store::public_material::{PublicRunBoundary as B, PublicRunOutcome as O};
    let request = runtime_api::WorkerActivityRequest {
        workspace_id: api.workspace_id().into(),
        worker_id: scan.grant.subject.worker_id.clone(),
        run_cursor: scan
            .run_cursor
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| Error::Store(e.to_string()))?,
        stop_after: scan.stop_after.clone(),
    };
    let response = api
        .runtime
        .worker_activity(&scan.grant.subject.runtime_id, request)
        .map_err(|_| Error::Store("Worker activity source is unavailable".into()))?;
    let mut next_run = scan.run_cursor.clone();
    match response.run {
        runtime_api::RunTerminalAvailability::Page { page } => {
            for fact in page.facts {
                let outcome = match fact.boundary {
                    B::Completed {
                        interrupted: true, ..
                    }
                    | B::Abandoned => WorkerLifecycleOutcome::Interrupted,
                    B::Completed {
                        outcome: O::Finished,
                        ..
                    } => WorkerLifecycleOutcome::Completed,
                    B::Completed {
                        outcome: O::LimitReached,
                        ..
                    } => WorkerLifecycleOutcome::Limited,
                    B::Cancelled => WorkerLifecycleOutcome::Cancelled,
                    B::Errored => WorkerLifecycleOutcome::Errored,
                    _ => continue,
                };
                api.store.capture_worker_lifecycle(
                    api.workspace_id(),
                    &scan.grant.grant_id,
                    &WorkerLifecycleEvent {
                        event_id: format!("run:{}:{}", fact.session_id, fact.entry_ref),
                        occurred_at: timestamp(fact.occurred_at_ms)?,
                        outcome,
                        session_entry_ref: Some(fact.entry_ref),
                        run_id: None,
                    },
                )?;
            }
            next_run = page
                .next_cursor
                .map(|c| serde_json::to_string(&c))
                .transpose()
                .map_err(|e| Error::Store(e.to_string()))?;
        }
        runtime_api::RunTerminalAvailability::NotMaterialized => {
            next_run = None;
        }
        runtime_api::RunTerminalAvailability::Unavailable { reason } => {
            // A changed/removed Session cannot reuse an old physical cursor. Start
            // a new sweep, never claim that an unavailable source had no failures.
            if matches!(
                reason,
                runtime_api::SessionPublicUnavailableReason::InvalidCursor
                    | runtime_api::SessionPublicUnavailableReason::RetentionMissing
            ) {
                next_run = None;
            }
            tracing::warn!(worker_id=%scan.grant.subject.worker_id, ?reason, "Worker terminal recovery source unavailable");
        }
    }
    for fact in response.stops {
        api.store.capture_worker_lifecycle(
            api.workspace_id(),
            &scan.grant.grant_id,
            &WorkerLifecycleEvent {
                event_id: format!("stop:{}", fact.operation_id),
                occurred_at: timestamp(fact.occurred_at_ms)?,
                outcome: WorkerLifecycleOutcome::Stopped,
                session_entry_ref: None,
                run_id: None,
            },
        )?;
    }
    // Each source commits before progress can advance. A crash before this CAS
    // replays sources under their stable identities, not a new notification ID.
    api.store.checkpoint_worker_lifecycle(
        api.workspace_id(),
        scan,
        next_run.as_deref(),
        response.next_stop.as_deref(),
    )
}

fn recipient(
    api: &WorkspaceApi,
    source: &crate::store::WorkerLifecycleNotification,
) -> Result<RuntimeWorkerRef> {
    let parent = if let Some(lease) = api
        .store
        .retained_worker_singleton_lease(api.workspace_id(), &source.parent)?
    {
        if lease
            .key
            .starts_with(crate::subjektiv::SUBJECT_WORKER_SINGLETON_PREFIX)
        {
            api.store
                .current_worker_singleton_owner(api.workspace_id(), &lease.key)?
                .ok_or_else(|| Error::Store("Spawner Subject has no current body".into()))?
                .worker
        } else {
            source.parent.clone()
        }
    } else {
        source.parent.clone()
    };
    // Body replacement does not transfer a target grant. A current body may
    // receive only if its own current operation and target rights allow it.
    authorize_known_worker_permission(api, api.workspace_id(), &parent, &source.child, "observe")?;
    Ok(parent)
}

pub(super) fn recover(api: &WorkspaceApi) {
    let Some(_permit) = api.notification_delivery_lifecycle.begin() else {
        return;
    };
    match api.store.lifecycle_scan_targets(api.workspace_id(), 8) {
        Ok(scans) => {
            for scan in scans {
                if api.notification_delivery_lifecycle.stopped() {
                    return;
                }
                if let Err(error) = capture_scan(api, &scan) {
                    tracing::warn!(grant_id=%scan.grant.grant_id,%error,"Worker activity recovery remains pending");
                }
            }
        }
        Err(error) => tracing::warn!(%error,"Worker activity scan unavailable"),
    }
    let pending = match api.store.pending_worker_lifecycle(api.workspace_id(), 20) {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(%error,"Worker activity delivery scan unavailable");
            return;
        }
    };
    for source in pending {
        if api.notification_delivery_lifecycle.stopped() {
            return;
        }
        let key = crate::store::NotificationDeliveryKey::WorkerNotification {
            delivery_id: source.delivery_id.clone(),
        };
        let parent = match recipient(api, &source) {
            Ok(parent) => parent,
            Err(_) => {
                record_notification_failure(
                    api,
                    &key,
                    crate::store::NotificationFailureCode::BindingRevoked,
                );
                continue;
            }
        };
        let target = match notification_delivery::fixed_worker_target(api, &parent) {
            Ok(target) => target,
            Err(code) => {
                record_notification_failure(api, &key, code);
                continue;
            }
        };
        let content = format!(
            "Worker {}/{} committed activity {:?} at {} (event {}; Session entry {}). This is an execution fact, not proof that the requested work succeeded. Inspect the committed Session and current task before deciding the result. It does not grant authority or change Ticket/MR/assignment state.",
            source.child.runtime_id,
            source.child.worker_id,
            source.event.outcome,
            source.event.occurred_at,
            source.event.event_id,
            source
                .event
                .session_entry_ref
                .as_deref()
                .unwrap_or("not present"),
        );
        let content =
            match api
                .store
                .prepare_worker_lifecycle(api.workspace_id(), &source, &content, &target)
            {
                Ok(content) => content,
                Err(_) => {
                    record_notification_failure(
                        api,
                        &key,
                        crate::store::NotificationFailureCode::PreparationFailed,
                    );
                    continue;
                }
            };
        if recipient(api, &source).ok().as_ref() != Some(&target.worker) {
            record_notification_failure(
                api,
                &key,
                crate::store::NotificationFailureCode::BindingRevoked,
            );
            continue;
        }
        let result = api.runtime.send_input(
            &target.worker,
            WorkerInputRequest {
                source: None,
                kind: WorkerInputKind::Notify,
                content,
                submission_request_id: Some(source.delivery_id.clone()),
                segments: None,
            },
        );
        if let Some(code) = notification_delivery::notification_failure(&result) {
            record_notification_failure(api, &key, code);
        } else if api
            .store
            .ack_worker_lifecycle(api.workspace_id(), &source.delivery_id, &target)
            .is_err()
        {
            record_notification_failure(
                api,
                &key,
                crate::store::NotificationFailureCode::AcknowledgementFailed,
            );
        }
    }
}
