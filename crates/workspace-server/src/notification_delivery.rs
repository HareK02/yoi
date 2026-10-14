//! Backend delivery consumes committed routing decisions, never event-kind policy.
//!
//! A mutation callback is only a low-latency wakeup. The Workspace hook also runs
//! bounded recovery rounds independently of mutations and Runtime subscriptions.
//! Requests are pinned before network I/O; acceptance is not model processing.
//!
//! Routing boundary map:
//! | Source / trigger | Owner | Delivery / recovery |
//! | --- | --- | --- |
//! | Assign, Answer, content and post edits, case, tags, tracking, release | Ticket transaction captures facts; `notification_routing` decides both recipients and non-notification reasons | Saved assignment/Worker requests and ACKs; mutation wakeup and periodic rounds |
//! | Workspace startup, Orchestrator create/restore/replacement, idle | Wakeup only; never a new request or a second policy | Same saved pending decisions |
//! | MR/handoff and Backend Job | Separate adapters to be connected by follow-up work | Not inferred from comments or display state |
//! | Explicit WorkerNotify/WorkerSendInput | Authenticated caller communication | Not automatic business routing |
//! | Internal SubWorker result and Web subscription | Parent/child execution and UI synchronization respectively | Remain separate; no Ticket policy in Runtime |
//!
//! Runtime receipt retention is finite. This layer preserves request identity and
//! bytes across retries; it does not claim unbounded exactly-once model execution.
use super::*;
use crate::store::{
    NotificationDeliveryKey, NotificationFailureCode, TicketWorkerNotification,
    TicketWorkerNotificationTarget,
};

#[derive(Default)]
struct NotificationDeliveryActivity {
    stopped: bool,
    in_flight: usize,
}

#[derive(Default)]
pub(super) struct NotificationDeliveryLifecycle {
    state: Mutex<NotificationDeliveryActivity>,
    drained: tokio::sync::Notify,
}
pub(super) struct NotificationDeliveryPermit(Arc<NotificationDeliveryLifecycle>);
impl NotificationDeliveryLifecycle {
    pub(super) fn begin(self: &Arc<Self>) -> Option<NotificationDeliveryPermit> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.stopped {
            return None;
        }
        state.in_flight += 1;
        Some(NotificationDeliveryPermit(self.clone()))
    }
    pub(super) fn stopped(&self) -> bool {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).stopped
    }
    pub(super) async fn stop_and_drain(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).stopped = true;
        loop {
            let drained = self.drained.notified();
            tokio::pin!(drained);
            drained.as_mut().enable();
            if self
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .in_flight
                == 0
            {
                return;
            }
            drained.await;
        }
    }
}
impl Drop for NotificationDeliveryPermit {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.in_flight -= 1;
        if state.in_flight == 0 {
            self.0.drained.notify_waiters();
        }
    }
}

pub(super) fn notification_failure(
    result: &std::result::Result<WorkerInputResult, RuntimeRegistryError>,
) -> Option<NotificationFailureCode> {
    match result {
        Err(_) => Some(NotificationFailureCode::AcceptanceUnknown),
        Ok(result) if result.disposition == WorkerInputDisposition::Unknown => {
            Some(NotificationFailureCode::AcceptanceUnknown)
        }
        Ok(result)
            if result.state == InternalWorkerOperationState::Accepted
                && result.disposition == WorkerInputDisposition::Accepted =>
        {
            None
        }
        Ok(result) if result.state == InternalWorkerOperationState::Unsupported => {
            Some(NotificationFailureCode::RuntimeUnsupported)
        }
        Ok(_) => Some(NotificationFailureCode::RuntimeRejected),
    }
}

pub(super) fn assignment_delivery_key(
    input: &crate::store::TicketAssignmentDelivery,
) -> NotificationDeliveryKey {
    NotificationDeliveryKey::AssignmentInput {
        assignment_id: input.assignment_id.clone(),
        input_event_id: input.input_event_id.clone(),
    }
}

pub(super) fn record_notification_failure(
    api: &WorkspaceApi,
    key: &NotificationDeliveryKey,
    code: NotificationFailureCode,
) {
    if let Err(error) =
        api.store
            .record_notification_delivery_failure(&api.config.workspace_id, key, code)
    {
        // Do not turn failure to save diagnostics into a successful suppression.
        tracing::warn!(%error, "notification failure could not be persisted");
    }
}

pub(super) fn notification_target(
    api: &WorkspaceApi,
    input: &TicketWorkerNotification,
) -> std::result::Result<TicketWorkerNotificationTarget, NotificationFailureCode> {
    let worker = api
        .runtime
        .worker(&input.worker)
        .map_err(|_| NotificationFailureCode::TargetUnavailable)?;
    if worker.workspace.workspace_id.as_deref() != Some(&api.config.workspace_id) {
        return Err(NotificationFailureCode::WorkspaceMismatch);
    }
    if !matches!(worker.state.as_str(), "idle" | "running" | "paused") {
        return Err(NotificationFailureCode::TargetStopped);
    }
    let binding = api
        .config_store
        .get_workspace_runtime_binding(&api.config.workspace_id, &input.worker.runtime_id)
        .map_err(|_| NotificationFailureCode::SourceLookupFailed)?
        .ok_or(NotificationFailureCode::TargetUnavailable)?;
    if binding.revoked_at.is_some() {
        return Err(NotificationFailureCode::BindingRevoked);
    }
    Ok(TicketWorkerNotificationTarget {
        worker: input.worker.clone(),
        authentication_binding_id: binding.binding_id,
    })
}

pub(super) fn dispatch_ticket_worker_notifications(api: &WorkspaceApi) {
    let Some(_permit) = api.notification_delivery_lifecycle.begin() else {
        return;
    };
    let inputs = match api
        .store
        .list_pending_ticket_worker_notifications(&api.config.workspace_id, 20)
    {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(%error, "notification scan failed; committed sources remain recoverable");
            return;
        }
    };
    for input in inputs {
        if api.notification_delivery_lifecycle.stopped() {
            break;
        }
        let key = NotificationDeliveryKey::WorkerNotification {
            delivery_id: input.delivery_id.clone(),
        };
        let target = match notification_target(api, &input) {
            Ok(target) => target,
            Err(code) => {
                record_notification_failure(api, &key, code);
                continue;
            }
        };
        let resource_key = match api.store.resource_key(
            &api.config.workspace_id,
            WorkspaceResourceKind::Ticket,
            &input.ticket_id,
        ) {
            Ok(Some(key)) if canonical_ticket_resource_key(&key).is_some() => key,
            _ => {
                record_notification_failure(api, &key, NotificationFailureCode::SourceLookupFailed);
                continue;
            }
        };
        // An attention notification grants no new assignment or execution authority.
        // The immutable source reference lets the recipient reread authoritative facts.
        let content = format!(
            "Ticket {resource_key} has committed activity (input {}). Reread the current Ticket and assignment before acting. This notification does not grant or extend execution authority.",
            input.input_event_id,
        );
        let content = match api.store.prepare_ticket_worker_notification_content(
            &api.config.workspace_id,
            &input.delivery_id,
            &content,
            &target,
        ) {
            Ok(content) => content,
            Err(error) => {
                let code = match error {
                    Error::TicketAssignmentConflict(_) => {
                        NotificationFailureCode::AssignmentChanged
                    }
                    _ => NotificationFailureCode::PreparationFailed,
                };
                record_notification_failure(api, &key, code);
                continue;
            }
        };
        let result = api.runtime.send_input(
            &input.worker,
            WorkerInputRequest {
                kind: WorkerInputKind::Notify,
                content,
                submission_request_id: Some(input.request_id.clone()),
                segments: None,
            },
        );
        if let Some(failure) = notification_failure(&result) {
            record_notification_failure(api, &key, failure);
        } else if api
            .store
            .ack_ticket_worker_notification(&api.config.workspace_id, &input.delivery_id, &target)
            .is_err()
        {
            record_notification_failure(api, &key, NotificationFailureCode::AcknowledgementFailed);
        }
        // One recipient's failure never prevents the other recipients in this round.
    }
}

pub(super) fn orchestrator_target_failure(
    api: &WorkspaceApi,
    worker: &RuntimeWorkerRef,
) -> NotificationFailureCode {
    match api.runtime.worker(worker) {
        Err(_) => return NotificationFailureCode::TargetUnavailable,
        Ok(summary)
            if summary.workspace.workspace_id.as_deref() != Some(&api.config.workspace_id) =>
        {
            return NotificationFailureCode::WorkspaceMismatch;
        }
        Ok(summary) if !matches!(summary.state.as_str(), "idle" | "running" | "paused") => {
            return NotificationFailureCode::TargetStopped;
        }
        _ => {}
    }
    match api
        .config_store
        .get_workspace_runtime_binding(&api.config.workspace_id, &worker.runtime_id)
    {
        Err(_) => NotificationFailureCode::SourceLookupFailed,
        Ok(None) => NotificationFailureCode::TargetUnavailable,
        Ok(Some(binding)) if binding.revoked_at.is_some() => {
            NotificationFailureCode::BindingRevoked
        }
        _ => NotificationFailureCode::AssignmentChanged,
    }
}

pub(super) fn recover_ticket_notifications(api: &WorkspaceApi) {
    let Some(_permit) = api.notification_delivery_lifecycle.begin() else {
        return;
    };
    dispatch_ticket_worker_notifications(api);
    if let Some(orchestrator) = find_workspace_orchestrator(api) {
        dispatch_orchestrator_assignment_attention(api, &orchestrator.worker);
    } else {
        // Still reconcile committed sources and explain target absence even when
        // there is no Runtime Worker. Do not start or restore one as a side effect.
        match api
            .store
            .list_pending_ticket_assignment_deliveries(&api.config.workspace_id, 20)
        {
            Ok(inputs) => {
                for input in inputs {
                    record_notification_failure(
                        api,
                        &assignment_delivery_key(&input),
                        NotificationFailureCode::TargetUnavailable,
                    );
                }
            }
            Err(error) => tracing::warn!(%error, "assignment notification scan failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_unknown_disposition_is_not_a_confirmed_rejection() {
        for (state, disposition, expected) in [
            (
                InternalWorkerOperationState::Rejected,
                WorkerInputDisposition::Unknown,
                Some(NotificationFailureCode::AcceptanceUnknown),
            ),
            (
                InternalWorkerOperationState::Rejected,
                WorkerInputDisposition::Rejected,
                Some(NotificationFailureCode::RuntimeRejected),
            ),
            (
                InternalWorkerOperationState::Accepted,
                WorkerInputDisposition::Accepted,
                None,
            ),
        ] {
            let result = Ok(WorkerInputResult {
                state,
                disposition,
                worker: RuntimeWorkerRef::new("runtime", "worker"),
                runtime_run_id: None,
                diagnostics: Vec::new(),
            });
            assert_eq!(notification_failure(&result), expected);
        }
    }

    #[tokio::test]
    async fn shutdown_fences_new_rounds_and_drains_the_synchronous_io_owner() {
        let lifecycle = Arc::new(NotificationDeliveryLifecycle::default());
        let admitted = lifecycle.begin().unwrap();
        let stopped = lifecycle.stop_and_drain();
        tokio::pin!(stopped);
        // Poll shutdown while a synchronous owner still holds its permit. This
        // models an aborted async waiter without pretending its I/O was cancelled.
        assert!(futures::poll!(stopped.as_mut()).is_pending());
        assert!(lifecycle.begin().is_none());
        drop(admitted);
        tokio::time::timeout(std::time::Duration::from_secs(1), stopped)
            .await
            .unwrap();
        assert!(lifecycle.begin().is_none());
    }
}
