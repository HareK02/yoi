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
//! | Backend Job result | Result transaction captures source; checker supplies domain facts; common routing records result/suppression/failure | Fixed Worker request bytes, binding, ACK and legacy receipt; same recovery rounds |
//! | MR review/request/revoke/cancel/merge and typed implementation result | Atomic business source capture; central policy | Same durable routing and periodic recovery; never inferred from comments |
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
    fixed_worker_target(api, &input.worker)
}

fn fixed_worker_target(
    api: &WorkspaceApi,
    recipient: &RuntimeWorkerRef,
) -> std::result::Result<TicketWorkerNotificationTarget, NotificationFailureCode> {
    let worker = api
        .runtime
        .worker(recipient)
        .map_err(|_| NotificationFailureCode::TargetUnavailable)?;
    if worker.workspace.workspace_id.as_deref() != Some(&api.config.workspace_id) {
        return Err(NotificationFailureCode::WorkspaceMismatch);
    }
    if !matches!(worker.state.as_str(), "idle" | "running" | "paused") {
        return Err(NotificationFailureCode::TargetStopped);
    }
    let binding = api
        .config_store
        .get_workspace_runtime_binding(&api.config.workspace_id, &recipient.runtime_id)
        .map_err(|_| NotificationFailureCode::SourceLookupFailed)?
        .ok_or(NotificationFailureCode::TargetUnavailable)?;
    if binding.revoked_at.is_some() {
        return Err(NotificationFailureCode::BindingRevoked);
    }
    Ok(TicketWorkerNotificationTarget {
        worker: recipient.clone(),
        authentication_binding_id: binding.binding_id,
    })
}

fn authorize_worker_attention(
    api: &WorkspaceApi,
    input: &TicketWorkerNotification,
) -> ApiResult<()> {
    subject_authorization::authorize(
        api,
        &input.worker,
        &server_api::authorization::OperationAuthorizationRequest {
            operation: "ticket.read".into(),
            resource: server_api::authorization::PermissionResource::Ticket {
                workspace_id: api.workspace_id().into(),
                ticket_id: input.ticket_id.clone(),
            },
        },
    )
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
        if authorize_worker_attention(api, &input).is_err() {
            record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
            continue;
        }
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
        if authorize_worker_attention(api, &input).is_err() {
            record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
            continue;
        }
        match api.store.prepare_ticket_worker_notification_content(
            &api.config.workspace_id,
            &input.delivery_id,
            &content,
            &target,
        ) {
            Ok(_) => (),
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
        let content = match api.store.admit_ticket_worker_notification_send(
            api.workspace_id(),
            &input.delivery_id,
            &target,
        ) {
            Ok(Some(content)) => content,
            Ok(None) => continue,
            Err(_) => {
                record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
                continue;
            }
        };
        if authorize_worker_attention(api, &input).is_err() {
            record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
            continue;
        }
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
    recover_job_notifications(api);
    dispatch_ticket_worker_notifications(api);
    // Resolve each current assignment independently. A missing body is not an implicit restore.
    let inputs = match api
        .store
        .list_pending_ticket_assignment_deliveries(api.workspace_id(), 100)
    {
        Ok(inputs) => inputs,
        Err(error) => {
            tracing::warn!(%error, "assignment notification scan failed");
            return;
        }
    };
    let mut recipients = std::collections::BTreeSet::new();
    for input in inputs {
        match assigned_subject_body(api, &input.ticket_id, &input.assignment_id) {
            Ok(Some(worker)) => {
                recipients.insert((worker.runtime_id, worker.worker_id));
            }
            _ => record_notification_failure(
                api,
                &assignment_delivery_key(&input),
                NotificationFailureCode::TargetUnavailable,
            ),
        }
    }
    for (runtime_id, worker_id) in recipients {
        dispatch_orchestrator_assignment_attention(
            api,
            &RuntimeWorkerRef::new(runtime_id, worker_id),
        );
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

/// Recover notification sources, not Job execution. This same bounded round runs
/// on startup and periodically; no Worker restoration or assignment is implicit.
pub(super) fn recover_job_notifications(api: &WorkspaceApi) {
    let Some(_permit) = api.notification_delivery_lifecycle.begin() else {
        return;
    };
    let sources = match api
        .store
        .list_pending_job_notifications(&api.config.workspace_id, 20)
    {
        Ok(sources) => sources,
        Err(error) => {
            tracing::warn!(%error,"Job notification scan failed");
            return;
        }
    };
    for source in sources {
        if api.notification_delivery_lifecycle.stopped() {
            break;
        }
        let acceptance = (|| -> Result<BackendJobResultAcceptance> {
            let job = api
                .store
                .get_backend_job(&api.config.workspace_id, &source.job_id)?
                .ok_or_else(|| Error::Store("Job notification result disappeared".into()))?;
            let attempt = api
                .store
                .get_backend_job_attempt(
                    &api.config.workspace_id,
                    &source.job_id,
                    &source.attempt_id,
                )?
                .ok_or_else(|| Error::Store("Job notification attempt disappeared".into()))?;
            Ok(BackendJobResultAcceptance {
                job,
                attempt,
                replayed: true,
            })
        })();
        match acceptance {
            Ok(acceptance) => deliver_job_result(api, &acceptance),
            Err(error) => tracing::warn!(%error,"Job notification source lookup failed"),
        }
    }
}

fn job_result_facts(
    api: &WorkspaceApi,
    acceptance: &BackendJobResultAcceptance,
) -> crate::store::JobNotificationFacts {
    use crate::store::JobNotificationFacts as F;
    let content = if acceptance.job.request.purpose == ticket_item_checker::PURPOSE {
        let input = match ticket_item_checker::parse_input(&acceptance.job.request.input) {
            Ok(input) => input,
            Err(_) => return F::Failed(NotificationFailureCode::InvalidResult),
        };
        let ticket = match browser_ticket_backend(api)
            .and_then(|backend| backend.show(input.ticket_id.into()).map_err(Error::from))
        {
            Ok(ticket) => ticket,
            Err(Error::Ticket(ticket::TicketError::NotFound(_))) => {
                return F::Failed(NotificationFailureCode::SourceNotFound);
            }
            Err(_) => return F::Failed(NotificationFailureCode::SourceLookupFailed),
        };
        match ticket_item_checker::result_facts(acceptance, &ticket) {
            Ok(ticket_item_checker::TicketItemCheckFacts::Findings(content)) => content,
            Ok(ticket_item_checker::TicketItemCheckFacts::NoFindings) => {
                return F::NoFindings;
            }
            Ok(ticket_item_checker::TicketItemCheckFacts::Stale) => return F::StaleContent,
            Err(_) => return F::Failed(NotificationFailureCode::InvalidResult),
        }
    } else {
        format!(
            "Backend Job `{}` completed attempt `{}` for `{}`. Durable result digest: {}.",
            acceptance.job.request.job_id,
            acceptance.attempt.attempt_id,
            acceptance.job.request.purpose,
            acceptance
                .job
                .result_digest
                .as_deref()
                .unwrap_or("unavailable")
        )
    };
    if content.len() > MAX_JOB_DELIVERY_BYTES {
        F::Failed(NotificationFailureCode::ContentTooLarge)
    } else {
        F::Result(content)
    }
}

pub(super) fn deliver_job_result(api: &WorkspaceApi, acceptance: &BackendJobResultAcceptance) {
    use crate::store::JobNotificationFacts;
    // A recovered/captured result is not delivery authority. Serialize even the
    // Runtime send with deletion, so no prepared bytes escape after its fence.
    let subject = subjektiv_deletion::job_subject(&acceptance.job.request);
    let gate = match subject
        .map(|subject| subjektiv_journal::lifecycle_lock(api, subject))
        .transpose()
    {
        Ok(gate) => gate,
        Err(error) => {
            tracing::warn!(%error, "Subject Job notification gate unavailable");
            return;
        }
    };
    let _guard = match gate.as_ref().map(|gate| gate.lock()).transpose() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(subject) = subject
        && subjektiv_deletion::require_available(api, subject).is_err()
    {
        return;
    }
    let Some(_permit) = api.notification_delivery_lifecycle.begin() else {
        return;
    };
    let source = match api.store.job_notification_source(
        &api.config.workspace_id,
        &acceptance.job.request.job_id,
        &acceptance.attempt.attempt_id,
    ) {
        Ok(source) => source,
        Err(error) => {
            tracing::warn!(%error,"Job notification source lookup failed");
            return;
        }
    };
    let workspace = &api.config.workspace_id;
    let key = NotificationDeliveryKey::WorkerNotification {
        delivery_id: source.delivery_id.clone(),
    };
    // The prepared body wins over all subsequent rendering and content changes.
    let request = match api
        .store
        .job_notification_request(workspace, &source.delivery_id)
    {
        Ok(request) => request,
        Err(error) => {
            tracing::warn!(%error,"Job notification request lookup failed");
            return;
        }
    };
    let facts = match &request {
        Some((content, _)) => JobNotificationFacts::Result(content.clone()),
        None => job_result_facts(api, acceptance),
    };
    match api.store.route_job_notification(workspace, &source, &facts) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            tracing::warn!(%error,"Job notification routing failed; committed source retained");
            return;
        }
    }
    let request = match api
        .store
        .job_notification_request(workspace, &source.delivery_id)
    {
        Ok(Some(request)) => request,
        _ => {
            record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
            return;
        }
    };
    let (content, target) = request;
    if acceptance.job.request.notification_target.as_ref() != Some(&target) {
        record_notification_failure(api, &key, NotificationFailureCode::PreparationFailed);
        return;
    }
    let authorized = match fixed_worker_target(api, &target) {
        Ok(target) => target,
        Err(code) => {
            record_notification_failure(api, &key, code);
            return;
        }
    };
    match api
        .store
        .authorize_job_notification(workspace, &source.delivery_id, &authorized)
    {
        Ok(None) => {}
        Ok(Some(code)) => {
            record_notification_failure(api, &key, code);
            return;
        }
        Err(_) => {
            record_notification_failure(api, &key, NotificationFailureCode::SourceLookupFailed);
            return;
        }
    }
    let result = api.runtime.send_input(
        &target,
        WorkerInputRequest {
            kind: WorkerInputKind::Notify,
            content,
            submission_request_id: Some(source.delivery_id.clone()),
            segments: None,
        },
    );
    if let Some(failure) = notification_failure(&result) {
        record_notification_failure(api, &key, failure);
    } else if api
        .store
        .ack_job_notification(workspace, &source.delivery_id, &target)
        .is_err()
    {
        record_notification_failure(api, &key, NotificationFailureCode::AcknowledgementFailed);
    }
}
