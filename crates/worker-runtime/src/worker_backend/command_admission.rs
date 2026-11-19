//! Bounded observation of the actual Controller's admission, not operation completion.

use super::*;
use protocol::{
    WorkerCommandDelivery, WorkerCommandDisposition, WorkerCommandKind,
    WorkerCommandObservationFailure, WorkerCommandResult,
};

pub(super) fn controller_command_identity(method: &Method) -> Option<(u64, WorkerCommandKind)> {
    let (envelope, kind) = match method {
        Method::Cancel { command } => (command, WorkerCommandKind::Cancel),
        Method::Compact { command } => (command, WorkerCommandKind::Compact),
        Method::Pause { command } => (command, WorkerCommandKind::Pause),
        Method::Resume { command } => (command, WorkerCommandKind::Resume),
        Method::Shutdown { command } => (command, WorkerCommandKind::Shutdown),
        _ => return None,
    };
    Some((envelope.command_id, kind))
}

pub(super) async fn observe_command<Fut>(
    send: Fut,
    command: WorkerCommandKind,
    command_id: Option<u64>,
    timeout: Duration,
) -> WorkerCommandResult
where
    Fut: Future<
        Output = Result<protocol::WorkerCommandAcknowledgement, worker::WorkerCommandSendError>,
    >,
{
    match tokio::time::timeout(timeout, send).await {
        Ok(Ok(acknowledgement)) => WorkerCommandResult::Acknowledged { acknowledgement },
        Ok(Err(worker::WorkerCommandSendError::NotDelivered {
            command,
            command_id,
            ..
        })) => WorkerCommandResult::NotDelivered {
            command,
            command_id,
        },
        Ok(Err(worker::WorkerCommandSendError::AdmissionUnknown {
            command,
            command_id,
        })) => WorkerCommandResult::Unconfirmed {
            command,
            command_id,
            delivery: WorkerCommandDelivery::Enqueued,
            reason: WorkerCommandObservationFailure::Closed,
        },
        Ok(Err(worker::WorkerCommandSendError::NotACommand)) => WorkerCommandResult::NotDelivered {
            command,
            command_id,
        },
        Err(_) => WorkerCommandResult::Unconfirmed {
            command,
            command_id,
            // Cancellation can race enqueue or Controller dequeue. Without a
            // receipt neither delivery nor a generated Host ID can be inferred.
            delivery: WorkerCommandDelivery::Unknown,
            reason: WorkerCommandObservationFailure::TimedOut,
        },
    }
}

pub(super) fn command_execution_result(
    operation: WorkerExecutionOperation,
    evidence: WorkerCommandResult,
) -> WorkerExecutionResult {
    let mut result = match &evidence {
        WorkerCommandResult::Acknowledged { acknowledgement }
            if acknowledgement.disposition == WorkerCommandDisposition::Accepted =>
        {
            WorkerExecutionResult::accepted(operation)
        }
        WorkerCommandResult::Acknowledged { acknowledgement } => WorkerExecutionResult::rejected(
            operation,
            format!(
                "Controller command {} {:?} rejected: {:?}",
                acknowledgement.command_id, acknowledgement.command, acknowledgement.disposition
            ),
        ),
        WorkerCommandResult::NotDelivered { .. } => WorkerExecutionResult::rejected(
            operation,
            "command was not delivered to the Controller",
        ),
        WorkerCommandResult::Unconfirmed { .. } => WorkerExecutionResult::errored(
            operation,
            "Controller admission is unconfirmed; do not resend as a fresh command. Exact retry requires the original ID on the same endpoint within its retained admission window",
        ),
    };
    // Acknowledgement snapshots are evidence for this command only. State
    // observation remains owned by the Controller stream, not this result path.
    result.command = Some(evidence);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn admission_timeout_and_endpoint_loss_are_not_rejections() {
        let timed_out = observe_command(
            std::future::pending(),
            WorkerCommandKind::Cancel,
            None,
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(
            timed_out,
            WorkerCommandResult::Unconfirmed {
                delivery: WorkerCommandDelivery::Unknown,
                reason: WorkerCommandObservationFailure::TimedOut,
                ..
            }
        ));
        let closed = observe_command(
            async {
                Err(worker::WorkerCommandSendError::AdmissionUnknown {
                    command: WorkerCommandKind::Compact,
                    command_id: Some(39),
                })
            },
            WorkerCommandKind::Compact,
            Some(39),
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(
            closed,
            WorkerCommandResult::Unconfirmed {
                command_id: Some(39),
                delivery: WorkerCommandDelivery::Enqueued,
                reason: WorkerCommandObservationFailure::Closed,
                ..
            }
        ));
        assert_eq!(
            command_execution_result(WorkerExecutionOperation::Cancel, timed_out).outcome,
            crate::execution::WorkerExecutionOutcome::Errored
        );
        assert_eq!(
            command_execution_result(WorkerExecutionOperation::Input, closed).outcome,
            crate::execution::WorkerExecutionOutcome::Errored
        );
    }

    #[test]
    fn controller_dispositions_survive_without_becoming_live_snapshot_writes() {
        for disposition in [
            WorkerCommandDisposition::Accepted,
            WorkerCommandDisposition::StaleCommandId,
            WorkerCommandDisposition::Conflict,
            WorkerCommandDisposition::InvalidState,
        ] {
            let acknowledgement = protocol::WorkerCommandAcknowledgement {
                command_id: 17,
                command: WorkerCommandKind::Cancel,
                disposition,
                state: protocol::WorkerStateSnapshot::initial(),
            };
            let evidence = WorkerCommandResult::Acknowledged { acknowledgement };
            let result =
                command_execution_result(WorkerExecutionOperation::Cancel, evidence.clone());
            assert_eq!(
                result.is_accepted(),
                disposition == WorkerCommandDisposition::Accepted
            );
            assert_eq!(result.command, Some(evidence));
            assert!(result.worker_state.is_none());
        }
    }
}
