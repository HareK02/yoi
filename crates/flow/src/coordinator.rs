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
    pub definition_digest: String,
    pub current_state: StateId,
    pub status: FlowInstanceStatus,
    pub active_attempt_id: Option<String>,
}

impl FlowInstance {
    pub fn start(
        instance_id: impl Into<String>,
        definition_id: impl Into<String>,
        definition: &CompiledFlowDefinition,
    ) -> Result<Self, FlowTransitionError> {
        let instance_id = instance_id.into();
        let definition_id = definition_id.into();
        ensure_non_empty("instance_id", &instance_id)
            .and_then(|_| ensure_non_empty("definition_id", &definition_id))
            .map_err(FlowTransitionError::InvalidRequest)?;
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
    pub definition_digest: String,
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
/// Workspace authority resolves the exact source content, but never mutates
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
        let instance =
            FlowInstance::start(instance_id, source.flow_id.clone(), &source.definition)?;
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
        let attempt_id = attempt_id.into();
        // The event log is the identity authority: an attempt ID is used once,
        // including failed/cancelled attempts. This prevents a delayed result
        // from an earlier visit to the same state from matching a later attempt.
        if self.events.iter().any(|event| {
            matches!(
                &event.event,
                FlowEventKind::TransitionRequested { attempt_id: used, .. } if used == &attempt_id
            )
        }) {
            return Err(FlowTransitionError::InvalidRequest(
                "transition attempt ID has already been used".to_string(),
            ));
        }
        let request = FlowTransitionRequest {
            attempt_id,
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
    #[error("Flow definition content does not match the instance's pinned digest")]
    DefinitionMismatch,
    #[error("invalid transition request: {0}")]
    InvalidRequest(String),
    #[error("Flow invariant failed: {0}")]
    Invariant(String),
}

fn begin_transition(
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
        definition_digest: instance.definition_digest.clone(),
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
            reason: attempt.reason.clone(),
            transitions: attempt.transitions.clone(),
        },
        FlowEventKind::TransitionVerifying {
            attempt_id: attempt.attempt_id.clone(),
        },
    ];
    Ok((attempt, events))
}

fn resolve_transition(
    instance: &mut FlowInstance,
    definition: &CompiledFlowDefinition,
    mut attempt: FlowTransitionAttempt,
    outcome: FlowVerifierOutcome,
) -> Result<FlowTransitionResolution, FlowTransitionError> {
    // Accept a result only for the currently reserved operation on this instance.
    // A completed/cancelled attempt has already released that reservation.
    if instance.instance_id != attempt.instance_id
        || instance.active_attempt_id.as_deref() != Some(attempt.attempt_id.as_str())
    {
        return Err(FlowTransitionError::InvalidRequest(
            "attempt is not the active/latest attempt for this Flow instance".to_string(),
        ));
    }
    if definition.content_digest != instance.definition_digest
        || attempt.definition_digest != instance.definition_digest
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
            if instance.status != FlowInstanceStatus::Active
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

    fn runtime_state() -> FlowRuntimeState {
        let definition = definition();
        FlowRuntimeState::start(
            &crate::ResolvedFlowSource {
                selector: "workspace:simple".parse().unwrap(),
                workspace_id: "workspace-1".into(),
                flow_id: "flow-1".into(),
                content_digest: definition.content_digest.clone(),
                definition,
            },
            "instance-1",
        )
        .unwrap()
        .0
    }

    #[test]
    fn cancelled_attempt_id_cannot_be_reused_even_after_restore() {
        let mut state = runtime_state();
        state.begin_or_recover_transition("first", "ready").unwrap();
        state
            .resolve_active_transition("first", FlowVerifierOutcome::Cancelled)
            .unwrap();
        let mut restored: FlowRuntimeState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        let before = restored.clone();
        assert!(matches!(
            restored.begin_or_recover_transition("first", "try again"),
            Err(FlowTransitionError::InvalidRequest(_))
        ));
        assert_eq!(restored, before);
        restored
            .begin_or_recover_transition("second", "try again")
            .unwrap();
        assert_eq!(
            restored.instance.active_attempt_id.as_deref(),
            Some("second")
        );
    }

    #[test]
    fn late_result_cannot_consume_a_new_attempt_in_the_same_state() {
        let mut state = runtime_state();
        state.begin_or_recover_transition("first", "ready").unwrap();
        state
            .resolve_active_transition("first", FlowVerifierOutcome::Cancelled)
            .unwrap();
        state
            .begin_or_recover_transition("second", "ready")
            .unwrap();
        let before = state.clone();
        assert!(matches!(
            state.resolve_active_transition(
                "first",
                FlowVerifierOutcome::Completed { results: vec![] }
            ),
            Err(FlowTransitionError::InvalidRequest(_))
        ));
        assert_eq!(state, before);
        state
            .resolve_active_transition("second", FlowVerifierOutcome::Cancelled)
            .unwrap();
        assert!(state.active_attempt.is_none());
    }

    #[test]
    fn source_content_is_checked_before_start_and_before_accepting_a_result() {
        let mut state = runtime_state();
        let mut changed = state.definition.clone();
        changed.content_digest = "different-content".into();
        assert_eq!(
            begin_transition(
                &mut state.instance,
                &changed,
                FlowTransitionRequest {
                    attempt_id: "first".into(),
                    reason: "ready".into(),
                }
            )
            .unwrap_err(),
            FlowTransitionError::DefinitionMismatch
        );
        assert!(state.instance.active_attempt_id.is_none());
        state.begin_or_recover_transition("first", "ready").unwrap();
        state.definition = changed;
        let before = state.clone();
        assert_eq!(
            state
                .resolve_active_transition("first", FlowVerifierOutcome::Cancelled)
                .unwrap_err(),
            FlowTransitionError::DefinitionMismatch
        );
        assert_eq!(state, before);
    }

    #[test]
    fn exactly_one_met_transition_enters_terminal_state() {
        let definition = definition();
        let mut instance = FlowInstance {
            instance_id: "instance-1".to_string(),
            definition_id: "definition-1".to_string(),
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
            status: FlowInstanceStatus::Active,
            active_attempt_id: None,
        };
        let attempt = begin(&mut instance);
        instance.current_state = StateId::new("done").unwrap();
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
            definition_digest: definition.content_digest.clone(),
            current_state: definition.initial.clone(),
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
