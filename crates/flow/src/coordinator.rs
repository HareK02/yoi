use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    CANCELLED_STATE_ID, CompiledFlowDefinition, CompiledTransition, StateId, TransitionId,
};

const MAX_REASON_BYTES: usize = 16 * 1024;
const MAX_RATIONALE_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowInstanceStatus {
    Active,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowInstance {
    pub instance_id: String,
    pub definition_id: String,
    pub definition_revision: u64,
    pub definition_digest: String,
    pub current_state: StateId,
    pub state_revision: u64,
    pub status: FlowInstanceStatus,
    pub active_attempt_id: Option<String>,
}

impl FlowInstance {
    pub fn start(
        instance_id: impl Into<String>,
        definition_id: impl Into<String>,
        definition_revision: u64,
        definition: &CompiledFlowDefinition,
    ) -> Result<Self, FlowTransitionError> {
        let instance_id = instance_id.into();
        let definition_id = definition_id.into();
        ensure_non_empty("instance_id", &instance_id)
            .and_then(|_| ensure_non_empty("definition_id", &definition_id))
            .map_err(FlowTransitionError::InvalidRequest)?;
        if definition_revision == 0 {
            return Err(FlowTransitionError::InvalidRequest(
                "definition_revision must be positive".to_string(),
            ));
        }
        let initial_state = definition.state(&definition.initial).ok_or_else(|| {
            FlowTransitionError::Invariant("compiled initial state is missing".to_string())
        })?;
        let status = if initial_state.terminal {
            FlowInstanceStatus::Completed
        } else {
            FlowInstanceStatus::Active
        };
        Ok(Self {
            instance_id,
            definition_id,
            definition_revision,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status,
            active_attempt_id: None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTransitionRequest {
    pub attempt_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTransitionAttempt {
    pub attempt_id: String,
    pub instance_id: String,
    pub definition_revision: u64,
    pub definition_digest: String,
    pub checked_state_revision: u64,
    pub from_state: StateId,
    pub reason: String,
    pub transitions: Vec<TransitionCheckSnapshot>,
    pub status: FlowAttemptStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionCheckSnapshot {
    pub transition_id: TransitionId,
    pub target: StateId,
    pub condition: String,
    pub synthetic: bool,
}

impl From<&CompiledTransition> for TransitionCheckSnapshot {
    fn from(transition: &CompiledTransition) -> Self {
        Self {
            transition_id: transition.id.clone(),
            target: transition.target.clone(),
            condition: transition.condition.clone(),
            synthetic: transition.synthetic,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowAttemptStatus {
    Verifying,
    Entered,
    Rejected,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionVerdict {
    Met,
    NotMet,
    Indeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionConditionResult {
    pub transition_id: TransitionId,
    pub verdict: ConditionVerdict,
    pub rationale: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FlowVerifierOutcome {
    Completed {
        results: Vec<TransitionConditionResult>,
    },
    Cancelled,
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTransitionResolution {
    pub attempt: FlowTransitionAttempt,
    pub entered_state: Option<StateId>,
    pub state_instructions: Option<String>,
    pub rejection: Option<FlowTransitionRejection>,
    pub events: Vec<FlowEventKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTransitionRejection {
    pub code: FlowRejectionCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowRejectionCode {
    NoConditionMet,
    MultipleConditionsMet,
    Indeterminate,
    InvalidVerifierOutput,
    VerifierCancelled,
    VerifierFailed,
    StaleState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowEventKind {
    TransitionRequested {
        attempt_id: String,
        state_id: StateId,
        state_revision: u64,
        reason: String,
        transitions: Vec<TransitionCheckSnapshot>,
    },
    TransitionVerifying {
        attempt_id: String,
    },
    TransitionCheckPassed {
        attempt_id: String,
        transition_id: TransitionId,
        results: Vec<TransitionConditionResult>,
    },
    TransitionCheckRejected {
        attempt_id: String,
        rejection: FlowTransitionRejection,
        results: Vec<TransitionConditionResult>,
    },
    TransitionVerificationCancelled {
        attempt_id: String,
    },
    TransitionVerificationFailed {
        attempt_id: String,
        message: String,
    },
    StateEntered {
        attempt_id: String,
        state_id: StateId,
        state_revision: u64,
        status: FlowInstanceStatus,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRuntimeEvent {
    pub sequence: u64,
    pub event: FlowEventKind,
}

/// Durable Flow authority owned by one Runtime Worker.
///
/// Workspace authority resolves and revisions the source, but never mutates
/// this value. Runtime persists the complete snapshot with the Worker session
/// and replaces it only after the corresponding session-log write succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRuntimeState {
    pub definition: CompiledFlowDefinition,
    pub instance: FlowInstance,
    pub active_attempt: Option<FlowTransitionAttempt>,
    pub events: Vec<FlowRuntimeEvent>,
}

impl FlowRuntimeState {
    pub fn start(
        source: &crate::ResolvedFlowSource,
        instance_id: impl Into<String>,
    ) -> Result<(Self, String), FlowTransitionError> {
        if source.content_digest != source.definition.content_digest {
            return Err(FlowTransitionError::InvalidRequest(
                "resolved Flow source digest does not match compiled definition".to_string(),
            ));
        }
        if source.selector.slug() != source.definition.name {
            return Err(FlowTransitionError::InvalidRequest(
                "resolved Flow selector does not match compiled definition name".to_string(),
            ));
        }
        let instance = FlowInstance::start(
            instance_id,
            source.flow_id.clone(),
            source.revision,
            &source.definition,
        )?;
        let initial = source
            .definition
            .state(&source.definition.initial)
            .ok_or_else(|| {
                FlowTransitionError::Invariant("compiled initial state is missing".to_string())
            })?;
        let event = FlowRuntimeEvent {
            sequence: 0,
            event: FlowEventKind::StateEntered {
                attempt_id: String::new(),
                state_id: instance.current_state.clone(),
                state_revision: instance.state_revision,
                status: instance.status,
            },
        };
        Ok((
            Self {
                definition: source.definition.clone(),
                instance,
                active_attempt: None,
                events: vec![event],
            },
            initial.instructions.clone(),
        ))
    }

    /// Begin a new attempt, or return the persisted active attempt after a
    /// Runtime/Worker restore. A recovered attempt is never rewritten.
    pub fn begin_or_recover_transition(
        &mut self,
        attempt_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Result<FlowTransitionAttempt, FlowTransitionError> {
        if let Some(attempt) = &self.active_attempt {
            return Ok(attempt.clone());
        }
        let request = FlowTransitionRequest {
            attempt_id: attempt_id.into(),
            reason: reason.into(),
        };
        let (attempt, events) = begin_transition(&mut self.instance, &self.definition, request)?;
        self.append_events(events)?;
        self.active_attempt = Some(attempt.clone());
        Ok(attempt)
    }

    pub fn resolve_active_transition(
        &mut self,
        attempt_id: &str,
        outcome: FlowVerifierOutcome,
    ) -> Result<FlowTransitionResolution, FlowTransitionError> {
        let attempt = self.active_attempt.clone().ok_or_else(|| {
            FlowTransitionError::InvalidRequest("Flow has no active transition attempt".to_string())
        })?;
        if attempt.attempt_id != attempt_id {
            return Err(FlowTransitionError::InvalidRequest(
                "attempt is not the active/latest attempt for this Flow instance".to_string(),
            ));
        }
        let resolution =
            resolve_transition(&mut self.instance, &self.definition, attempt, outcome)?;
        self.append_events(resolution.events.clone())?;
        self.active_attempt = None;
        Ok(resolution)
    }

    fn append_events(&mut self, events: Vec<FlowEventKind>) -> Result<(), FlowTransitionError> {
        for event in events {
            let sequence = u64::try_from(self.events.len()).map_err(|_| {
                FlowTransitionError::Invariant("Flow event sequence overflowed".to_string())
            })?;
            self.events.push(FlowRuntimeEvent { sequence, event });
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum FlowTransitionError {
    #[error("Flow instance is not active")]
    NotActive,
    #[error("another transition attempt is already active")]
    AttemptInProgress,
    #[error("Flow definition does not match the instance's pinned revision")]
    DefinitionMismatch,
    #[error("invalid transition request: {0}")]
    InvalidRequest(String),
    #[error("Flow invariant failed: {0}")]
    Invariant(String),
}

pub fn begin_transition(
    instance: &mut FlowInstance,
    definition: &CompiledFlowDefinition,
    request: FlowTransitionRequest,
) -> Result<(FlowTransitionAttempt, Vec<FlowEventKind>), FlowTransitionError> {
    if instance.status != FlowInstanceStatus::Active {
        return Err(FlowTransitionError::NotActive);
    }
    if instance.active_attempt_id.is_some() {
        return Err(FlowTransitionError::AttemptInProgress);
    }
    if definition.content_digest != instance.definition_digest {
        return Err(FlowTransitionError::DefinitionMismatch);
    }
    ensure_non_empty("attempt_id", &request.attempt_id)
        .map_err(FlowTransitionError::InvalidRequest)?;
    validate_reason(&request.reason)?;
    let state = definition.state(&instance.current_state).ok_or_else(|| {
        FlowTransitionError::Invariant(format!(
            "current state {:?} is missing from the pinned definition",
            instance.current_state
        ))
    })?;
    if state.terminal || state.transitions.is_empty() {
        return Err(FlowTransitionError::Invariant(
            "active instance points at a state without transitions".to_string(),
        ));
    }

    let transitions = state
        .transitions
        .iter()
        .map(TransitionCheckSnapshot::from)
        .collect::<Vec<_>>();
    let attempt = FlowTransitionAttempt {
        attempt_id: request.attempt_id,
        instance_id: instance.instance_id.clone(),
        definition_revision: instance.definition_revision,
        definition_digest: instance.definition_digest.clone(),
        checked_state_revision: instance.state_revision,
        from_state: instance.current_state.clone(),
        reason: request.reason,
        transitions,
        status: FlowAttemptStatus::Verifying,
    };
    instance.active_attempt_id = Some(attempt.attempt_id.clone());
    let events = vec![
        FlowEventKind::TransitionRequested {
            attempt_id: attempt.attempt_id.clone(),
            state_id: attempt.from_state.clone(),
            state_revision: attempt.checked_state_revision,
            reason: attempt.reason.clone(),
            transitions: attempt.transitions.clone(),
        },
        FlowEventKind::TransitionVerifying {
            attempt_id: attempt.attempt_id.clone(),
        },
    ];
    Ok((attempt, events))
}

pub fn resolve_transition(
    instance: &mut FlowInstance,
    definition: &CompiledFlowDefinition,
    mut attempt: FlowTransitionAttempt,
    outcome: FlowVerifierOutcome,
) -> Result<FlowTransitionResolution, FlowTransitionError> {
    if instance.active_attempt_id.as_deref() != Some(attempt.attempt_id.as_str()) {
        return Err(FlowTransitionError::InvalidRequest(
            "attempt is not the active/latest attempt for this Flow instance".to_string(),
        ));
    }
    if definition.content_digest != instance.definition_digest
        || attempt.definition_digest != instance.definition_digest
        || attempt.definition_revision != instance.definition_revision
    {
        return Err(FlowTransitionError::DefinitionMismatch);
    }

    let mut events = Vec::new();
    let resolution = match outcome {
        FlowVerifierOutcome::Cancelled => {
            attempt.status = FlowAttemptStatus::Cancelled;
            events.push(FlowEventKind::TransitionVerificationCancelled {
                attempt_id: attempt.attempt_id.clone(),
            });
            rejected_resolution(
                attempt,
                FlowRejectionCode::VerifierCancelled,
                "Flow verifier was cancelled before producing a complete verdict",
                Vec::new(),
                events,
            )
        }
        FlowVerifierOutcome::Failed { message } => {
            let message = bounded_message(message);
            attempt.status = FlowAttemptStatus::Failed;
            events.push(FlowEventKind::TransitionVerificationFailed {
                attempt_id: attempt.attempt_id.clone(),
                message: message.clone(),
            });
            rejected_resolution(
                attempt,
                FlowRejectionCode::VerifierFailed,
                format!("Flow verifier failed: {message}"),
                Vec::new(),
                events,
            )
        }
        FlowVerifierOutcome::Completed { results } => {
            if instance.state_revision != attempt.checked_state_revision
                || instance.current_state != attempt.from_state
            {
                attempt.status = FlowAttemptStatus::Rejected;
                let rejection = FlowTransitionRejection {
                    code: FlowRejectionCode::StaleState,
                    message: "Flow state changed after verification began".to_string(),
                };
                events.push(FlowEventKind::TransitionCheckRejected {
                    attempt_id: attempt.attempt_id.clone(),
                    rejection: rejection.clone(),
                    results,
                });
                FlowTransitionResolution {
                    attempt,
                    entered_state: None,
                    state_instructions: None,
                    rejection: Some(rejection),
                    events,
                }
            } else {
                resolve_completed_results(instance, definition, attempt, results, events)?
            }
        }
    };
    instance.active_attempt_id = None;
    Ok(resolution)
}

fn resolve_completed_results(
    instance: &mut FlowInstance,
    definition: &CompiledFlowDefinition,
    mut attempt: FlowTransitionAttempt,
    results: Vec<TransitionConditionResult>,
    mut events: Vec<FlowEventKind>,
) -> Result<FlowTransitionResolution, FlowTransitionError> {
    let expected = attempt
        .transitions
        .iter()
        .map(|transition| transition.transition_id.clone())
        .collect::<BTreeSet<_>>();
    let mut by_id = BTreeMap::new();
    let mut invalid_reason = None;
    for result in &results {
        if result.rationale.len() > MAX_RATIONALE_BYTES {
            invalid_reason = Some(format!(
                "rationale for transition {:?} exceeds {MAX_RATIONALE_BYTES} bytes",
                result.transition_id
            ));
            break;
        }
        if by_id
            .insert(result.transition_id.clone(), result.verdict)
            .is_some()
        {
            invalid_reason = Some(format!(
                "verifier returned transition {:?} more than once",
                result.transition_id
            ));
            break;
        }
    }
    let actual = by_id.keys().cloned().collect::<BTreeSet<_>>();
    if invalid_reason.is_none() && actual != expected {
        invalid_reason = Some(
            "verifier must return exactly one result for every captured outgoing transition"
                .to_string(),
        );
    }
    if let Some(message) = invalid_reason {
        attempt.status = FlowAttemptStatus::Rejected;
        let rejection = FlowTransitionRejection {
            code: FlowRejectionCode::InvalidVerifierOutput,
            message,
        };
        events.push(FlowEventKind::TransitionCheckRejected {
            attempt_id: attempt.attempt_id.clone(),
            rejection: rejection.clone(),
            results,
        });
        return Ok(FlowTransitionResolution {
            attempt,
            entered_state: None,
            state_instructions: None,
            rejection: Some(rejection),
            events,
        });
    }

    let met = results
        .iter()
        .filter(|result| result.verdict == ConditionVerdict::Met)
        .collect::<Vec<_>>();
    let rejection = if met.is_empty() {
        let indeterminate = results
            .iter()
            .any(|result| result.verdict == ConditionVerdict::Indeterminate);
        Some(FlowTransitionRejection {
            code: if indeterminate {
                FlowRejectionCode::Indeterminate
            } else {
                FlowRejectionCode::NoConditionMet
            },
            message: if indeterminate {
                "No transition condition was met and at least one condition could not be determined"
                    .to_string()
            } else {
                "No outgoing transition condition was met".to_string()
            },
        })
    } else if met.len() > 1 {
        Some(FlowTransitionRejection {
            code: FlowRejectionCode::MultipleConditionsMet,
            message: "More than one outgoing transition condition was met".to_string(),
        })
    } else {
        None
    };
    if let Some(rejection) = rejection {
        attempt.status = FlowAttemptStatus::Rejected;
        events.push(FlowEventKind::TransitionCheckRejected {
            attempt_id: attempt.attempt_id.clone(),
            rejection: rejection.clone(),
            results,
        });
        return Ok(FlowTransitionResolution {
            attempt,
            entered_state: None,
            state_instructions: None,
            rejection: Some(rejection),
            events,
        });
    }

    let selected = met[0];
    let transition = attempt
        .transitions
        .iter()
        .find(|transition| transition.transition_id == selected.transition_id)
        .ok_or_else(|| {
            FlowTransitionError::Invariant(
                "selected transition is missing from the captured attempt".to_string(),
            )
        })?;
    let target_state = definition.state(&transition.target).ok_or_else(|| {
        FlowTransitionError::Invariant(format!(
            "target state {:?} is missing from the pinned definition",
            transition.target
        ))
    })?;
    attempt.status = FlowAttemptStatus::Entered;
    events.push(FlowEventKind::TransitionCheckPassed {
        attempt_id: attempt.attempt_id.clone(),
        transition_id: transition.transition_id.clone(),
        results,
    });
    instance.current_state = transition.target.clone();
    instance.state_revision = instance.state_revision.checked_add(1).ok_or_else(|| {
        FlowTransitionError::Invariant("Flow state revision overflowed".to_string())
    })?;
    instance.status = if transition.target.as_str() == CANCELLED_STATE_ID {
        FlowInstanceStatus::Cancelled
    } else if target_state.terminal {
        FlowInstanceStatus::Completed
    } else {
        FlowInstanceStatus::Active
    };
    events.push(FlowEventKind::StateEntered {
        attempt_id: attempt.attempt_id.clone(),
        state_id: instance.current_state.clone(),
        state_revision: instance.state_revision,
        status: instance.status,
    });
    Ok(FlowTransitionResolution {
        attempt,
        entered_state: Some(instance.current_state.clone()),
        state_instructions: Some(target_state.instructions.clone()),
        rejection: None,
        events,
    })
}

fn rejected_resolution(
    attempt: FlowTransitionAttempt,
    code: FlowRejectionCode,
    message: impl Into<String>,
    results: Vec<TransitionConditionResult>,
    mut events: Vec<FlowEventKind>,
) -> FlowTransitionResolution {
    let rejection = FlowTransitionRejection {
        code,
        message: message.into(),
    };
    events.push(FlowEventKind::TransitionCheckRejected {
        attempt_id: attempt.attempt_id.clone(),
        rejection: rejection.clone(),
        results,
    });
    FlowTransitionResolution {
        attempt,
        entered_state: None,
        state_instructions: None,
        rejection: Some(rejection),
        events,
    }
}

fn validate_reason(reason: &str) -> Result<(), FlowTransitionError> {
    if reason.trim().is_empty() {
        return Err(FlowTransitionError::InvalidRequest(
            "reason must not be empty".to_string(),
        ));
    }
    if reason.len() > MAX_REASON_BYTES {
        return Err(FlowTransitionError::InvalidRequest(format!(
            "reason exceeds {MAX_REASON_BYTES} bytes"
        )));
    }
    Ok(())
}

fn ensure_non_empty(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(())
    }
}

fn bounded_message(message: String) -> String {
    if message.len() <= MAX_RATIONALE_BYTES {
        return message;
    }
    let mut end = MAX_RATIONALE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &message[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_flow_source;

    fn definition() -> CompiledFlowDefinition {
        compile_flow_source(
            r#"{
                schema_version = 1;
                name = "simple";
                initial = "work";
                states = {
                    work = {
                        instructions = "Do the work.";
                        transitions = {
                            done = {
                                target = "done";
                                condition = "The requested work and validation are complete.";
                            };
                        };
                    };
                    done = { instructions = ""; terminal = true; };
                };
            }"#,
        )
        .unwrap()
    }

    fn begin(instance: &mut FlowInstance) -> FlowTransitionAttempt {
        let (attempt, events) = begin_transition(
            instance,
            &definition(),
            FlowTransitionRequest {
                attempt_id: "attempt-1".to_string(),
                reason: "Implementation and tests are complete.".to_string(),
            },
        )
        .unwrap();
        assert!(matches!(
            events.as_slice(),
            [
                FlowEventKind::TransitionRequested { .. },
                FlowEventKind::TransitionVerifying { .. }
            ]
        ));
        attempt
    }

    #[test]
    fn exactly_one_met_transition_enters_terminal_state() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let results = attempt
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: if transition.transition_id.as_str() == "done" {
                    ConditionVerdict::Met
                } else {
                    ConditionVerdict::NotMet
                },
                rationale: "bounded rationale".to_string(),
            })
            .collect();
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed { results },
        )
        .unwrap();
        assert_eq!(instance.current_state.as_str(), "done");
        assert_eq!(instance.state_revision, 1);
        assert_eq!(instance.status, FlowInstanceStatus::Completed);
        assert!(resolution.rejection.is_none());
        assert!(matches!(
            resolution.events.as_slice(),
            [
                FlowEventKind::TransitionCheckPassed { .. },
                FlowEventKind::StateEntered { .. }
            ]
        ));
    }

    #[test]
    fn cancellation_requires_synthetic_condition_to_be_met() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let results = attempt
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: if transition.synthetic {
                    ConditionVerdict::Met
                } else {
                    ConditionVerdict::NotMet
                },
                rationale: "the required authority is unavailable".to_string(),
            })
            .collect();
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed { results },
        )
        .unwrap();
        assert_eq!(instance.current_state.as_str(), CANCELLED_STATE_ID);
        assert_eq!(instance.status, FlowInstanceStatus::Cancelled);
        assert!(resolution.rejection.is_none());
    }

    #[test]
    fn no_met_condition_keeps_state_unchanged() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let results = attempt
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: ConditionVerdict::NotMet,
                rationale: "not enough evidence".to_string(),
            })
            .collect();
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed { results },
        )
        .unwrap();
        assert_eq!(instance.current_state.as_str(), "work");
        assert_eq!(instance.state_revision, 0);
        assert_eq!(instance.active_attempt_id, None);
        assert_eq!(
            resolution.rejection.unwrap().code,
            FlowRejectionCode::NoConditionMet
        );
    }

    #[test]
    fn duplicate_or_missing_verdict_is_invalid_output() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let one_result = vec![TransitionConditionResult {
            transition_id: attempt.transitions[0].transition_id.clone(),
            verdict: ConditionVerdict::Met,
            rationale: "done".to_string(),
        }];
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed {
                results: one_result,
            },
        )
        .unwrap();
        assert_eq!(
            resolution.rejection.unwrap().code,
            FlowRejectionCode::InvalidVerifierOutput
        );
        assert_eq!(instance.current_state.as_str(), "work");
    }

    #[test]
    fn stale_result_cannot_enter_state() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        instance.state_revision = 1;
        let results = attempt
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: ConditionVerdict::NotMet,
                rationale: "not met".to_string(),
            })
            .collect();
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed { results },
        )
        .unwrap();
        assert_eq!(
            resolution.rejection.unwrap().code,
            FlowRejectionCode::StaleState
        );
    }

    #[test]
    fn multiple_met_conditions_reject_without_state_entry() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let results = attempt
            .transitions
            .iter()
            .map(|transition| TransitionConditionResult {
                transition_id: transition.transition_id.clone(),
                verdict: ConditionVerdict::Met,
                rationale: "claimed met".to_string(),
            })
            .collect();
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Completed { results },
        )
        .unwrap();
        assert_eq!(
            resolution.rejection.unwrap().code,
            FlowRejectionCode::MultipleConditionsMet
        );
        assert_eq!(instance.current_state.as_str(), "work");
        assert_eq!(instance.state_revision, 0);
        assert!(
            resolution
                .events
                .iter()
                .all(|event| !matches!(event, FlowEventKind::StateEntered { .. }))
        );
    }

    #[test]
    fn verifier_cancellation_is_terminal_for_attempt_but_not_flow() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        let resolution = resolve_transition(
            &mut instance,
            &definition,
            attempt,
            FlowVerifierOutcome::Cancelled,
        )
        .unwrap();
        assert_eq!(resolution.attempt.status, FlowAttemptStatus::Cancelled);
        assert_eq!(instance.status, FlowInstanceStatus::Active);
        assert_eq!(instance.current_state.as_str(), "work");
        assert_eq!(instance.active_attempt_id, None);
        assert!(
            resolution
                .events
                .iter()
                .all(|event| !matches!(event, FlowEventKind::StateEntered { .. }))
        );
    }

    #[test]
    fn overlapping_attempt_is_rejected_before_verifier() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_revision: 1,
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            state_revision: 0,
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let _ = begin(&mut instance);
        let overlap = begin_transition(
            &mut instance,
            &definition,
            FlowTransitionRequest {
                attempt_id: "attempt-2".to_string(),
                reason: "retry".to_string(),
            },
        )
        .unwrap_err();
        assert_eq!(overlap, FlowTransitionError::AttemptInProgress);
    }
}
