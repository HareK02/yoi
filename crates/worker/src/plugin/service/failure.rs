//! Explicit Service-owned mappings. Never inspect Display, guest code/message,
//! diagnostic id or source text. This module has no retry/lifecycle authority.
use super::*;
use yoi_plugin_runtime::{ExecutionError, ServiceInvocationResult};

#[derive(Clone, Debug, Default)]
pub(in crate::plugin) struct ServiceDriverObservations {
    pub event_id: Option<String>,
    pub invocation_id: Option<String>,
    pub attempt: Option<u32>,
    pub result: Option<PluginServiceEventResult>,
    pub invocation_cause: Option<PluginServiceFailure>,
    pub invocation_failure: Option<PluginServiceFailureAttachment>,
    pub attachment_unavailable: bool,
    pub completion: Option<PluginServiceFailure>,
    pub stop: Option<PluginServiceFailure>,
    pub release: Option<PluginServiceFailure>,
}

pub(super) fn execution_reason(error: &ExecutionError) -> PluginServiceExecutionFailure {
    use PluginServiceExecutionFailure as F;
    match error {
        ExecutionError::Artifact(_) => F::Artifact,
        ExecutionError::UnsupportedComponent(_) => F::UnsupportedComponent,
        ExecutionError::InvalidInput(_) => F::InvalidInput,
        ExecutionError::InvalidOutput(_) => F::InvalidOutput,
        ExecutionError::Cancelled => F::Cancelled,
        ExecutionError::Deadline => F::Deadline,
        ExecutionError::Host(error) => match error.code {
            errors::Code::Denied => F::HostDenied,
            errors::Code::InvalidInput => F::HostInvalidInput,
            errors::Code::NotFound => F::HostNotFound,
            errors::Code::Conflict => F::HostConflict,
            errors::Code::Unsupported => F::HostUnsupported,
            errors::Code::QuotaExceeded => F::HostQuotaExceeded,
            errors::Code::DeadlineExceeded => F::HostDeadlineExceeded,
            errors::Code::Cancelled => F::HostCancelled,
            errors::Code::Unavailable => F::HostUnavailable,
            errors::Code::Indeterminate => F::HostIndeterminate,
            errors::Code::ReceiptExpired => F::HostReceiptExpired,
        },
        ExecutionError::Guest { .. } => F::Guest,
        ExecutionError::Trap(_) => F::Trap,
    }
}

/// The pre-existing result selection is unchanged: unknown effects dominate;
/// execution failures still propose the same fixed RetryAfter(1000).
pub(super) fn event_outcome(
    output: &ServiceInvocationResult,
) -> (PluginServiceEventResult, Option<PluginServiceFailure>) {
    let cause = match &output.outcome {
        Err(error) => Some(PluginServiceFailure::Invocation {
            reason: execution_reason(error),
        }),
        Ok(ServiceOutput::Event(PluginServiceEventResult::RetryAfter { .. })) => {
            Some(PluginServiceFailure::Invocation {
                reason: PluginServiceExecutionFailure::GuestRetry,
            })
        }
        Ok(ServiceOutput::Event(PluginServiceEventResult::Reject)) => {
            Some(PluginServiceFailure::Invocation {
                reason: PluginServiceExecutionFailure::GuestRejected,
            })
        }
        Ok(ServiceOutput::Event(_)) => None,
        Ok(_) => Some(PluginServiceFailure::Invocation {
            reason: PluginServiceExecutionFailure::UnexpectedOutput,
        }),
    };
    if matches!(output.effect, errors::EffectObservation::Indeterminate) {
        return (
            PluginServiceEventResult::Indeterminate,
            cause.or(Some(PluginServiceFailure::EffectIndeterminate)),
        );
    }
    let result = match &output.outcome {
        Ok(ServiceOutput::Event(result)) => result.clone(),
        _ => PluginServiceEventResult::RetryAfter { delay_ms: 1_000 },
    };
    (result, cause)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn execution_sources_are_explicitly_mapped_and_never_serialized() {
        use PluginServiceExecutionFailure as F;
        let secret = "credential=/private/raw-source/body";
        let errors = [
            (ExecutionError::Artifact(secret.into()), F::Artifact),
            (
                ExecutionError::UnsupportedComponent(secret.into()),
                F::UnsupportedComponent,
            ),
            (ExecutionError::InvalidInput(secret.into()), F::InvalidInput),
            (
                ExecutionError::InvalidOutput(secret.into()),
                F::InvalidOutput,
            ),
            (ExecutionError::Cancelled, F::Cancelled),
            (ExecutionError::Deadline, F::Deadline),
            (
                ExecutionError::Host(errors::HostError {
                    code: errors::Code::Denied,
                    message: secret.into(),
                    diagnostic_id: secret.into(),
                    effect: errors::EffectObservation::NotDispatched,
                }),
                F::HostDenied,
            ),
            (
                ExecutionError::Guest {
                    code: secret.into(),
                    message: secret.into(),
                },
                F::Guest,
            ),
            (ExecutionError::Trap(secret.into()), F::Trap),
        ];
        for (error, reason) in errors {
            let output = ServiceInvocationResult {
                outcome: Err(error),
                effect: errors::EffectObservation::NotDispatched,
            };
            let (result, cause) = event_outcome(&output);
            assert_eq!(
                result,
                PluginServiceEventResult::RetryAfter { delay_ms: 1000 }
            );
            let expected = PluginServiceFailure::Invocation { reason };
            assert_eq!(cause, Some(expected));
            let attachment = PluginServiceFailureAttachment::from_cause(expected).unwrap();
            let wire = serde_json::to_string(&attachment).unwrap();
            assert!(!wire.contains(secret));
            assert!(!wire.contains("diagnostic_id"));
        }
    }
    #[test]
    fn unknown_effect_retains_execution_cause_without_changing_disposition() {
        let output = ServiceInvocationResult {
            outcome: Err(ExecutionError::Deadline),
            effect: errors::EffectObservation::Indeterminate,
        };
        assert_eq!(
            event_outcome(&output),
            (
                PluginServiceEventResult::Indeterminate,
                Some(PluginServiceFailure::Invocation {
                    reason: PluginServiceExecutionFailure::Deadline
                })
            )
        );
        let output = ServiceInvocationResult {
            outcome: Ok(ServiceOutput::Event(PluginServiceEventResult::RetryAfter {
                delay_ms: 42,
            })),
            effect: errors::EffectObservation::NotDispatched,
        };
        assert_eq!(
            event_outcome(&output),
            (
                PluginServiceEventResult::RetryAfter { delay_ms: 42 },
                Some(PluginServiceFailure::Invocation {
                    reason: PluginServiceExecutionFailure::GuestRetry
                })
            )
        );
    }
}
