use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fmt::Write as _;

use decodal::{Engine, LoadedSource, SourceLoader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const FLOW_SCHEMA_VERSION: u32 = 1;
pub const CANCELLED_STATE_ID: &str = "$cancelled";
pub const CANCEL_TRANSITION_ID: &str = "$cancel";
pub const CANCEL_CONDITION: &str = "An exceptional condition makes it impossible to continue the current instructions and reach a normal terminal state with the available tools, scope, and session context.";

const MAX_SOURCE_BYTES: usize = 256 * 1024;
const MAX_STATES: usize = 128;
const MAX_TRANSITIONS_PER_STATE: usize = 32;
const MAX_TEXT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StateId(String);

impl StateId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_identifier("state", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StateId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransitionId(String);

impl TransitionId {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        validate_identifier("transition", &value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TransitionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledFlowDefinition {
    pub schema_version: u32,
    pub name: String,
    pub initial: StateId,
    pub states: BTreeMap<StateId, CompiledState>,
    pub content_digest: String,
}

impl CompiledFlowDefinition {
    pub fn state(&self, state_id: &StateId) -> Option<&CompiledState> {
        self.states.get(state_id)
    }

    pub fn outgoing(&self, state_id: &StateId) -> Option<&[CompiledTransition]> {
        self.state(state_id)
            .map(|state| state.transitions.as_slice())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledState {
    pub id: StateId,
    pub instructions: String,
    pub terminal: bool,
    pub transitions: Vec<CompiledTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledTransition {
    pub id: TransitionId,
    pub target: StateId,
    pub condition: String,
    pub synthetic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowDiagnostic {
    pub code: String,
    pub path: String,
    pub message: String,
}

impl FlowDiagnostic {
    fn new(code: impl Into<String>, path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            path: path.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[error("Flow definition is invalid")]
pub struct FlowCompileError {
    pub diagnostics: Vec<FlowDiagnostic>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FlowSource {
    schema_version: u32,
    name: String,
    initial: String,
    states: BTreeMap<String, StateSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateSource {
    instructions: String,
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    transitions: BTreeMap<String, TransitionSource>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransitionSource {
    target: String,
    condition: String,
}

pub fn compile_flow_source(content: &str) -> Result<CompiledFlowDefinition, FlowCompileError> {
    if content.len() > MAX_SOURCE_BYTES {
        return Err(one_diagnostic(
            "source_too_large",
            "$",
            format!("Flow source exceeds {MAX_SOURCE_BYTES} bytes"),
        ));
    }

    let mut engine = Engine::new(RejectImports);
    let module = engine
        .add_root_source("flow.dcdl", "flow.dcdl", content)
        .map_err(|error| {
            one_diagnostic("dcdl_parse", "$", format!("DCDL parsing failed: {error:?}"))
        })?;
    let value = engine.eval_module(module).map_err(|error| {
        one_diagnostic(
            "dcdl_evaluation",
            "$",
            format!("DCDL evaluation failed: {error:?}"),
        )
    })?;
    let data = engine.materialize(&value).map_err(|error| {
        one_diagnostic(
            "dcdl_materialization",
            "$",
            format!("DCDL materialization failed: {error:?}"),
        )
    })?;
    let value = decodal_data_to_json(&data);
    let source = serde_json::from_value::<FlowSource>(value).map_err(|error| {
        one_diagnostic(
            "schema_decode",
            "$",
            format!("Flow source does not match schema: {error}"),
        )
    })?;
    compile_typed_source(source, content)
}

fn compile_typed_source(
    source: FlowSource,
    content: &str,
) -> Result<CompiledFlowDefinition, FlowCompileError> {
    let mut diagnostics = Vec::new();
    if source.schema_version != FLOW_SCHEMA_VERSION {
        diagnostics.push(FlowDiagnostic::new(
            "unsupported_schema_version",
            "schema_version",
            format!(
                "unsupported Flow schema version {}; expected {FLOW_SCHEMA_VERSION}",
                source.schema_version
            ),
        ));
    }
    if let Err(message) = validate_identifier("Flow", &source.name) {
        diagnostics.push(FlowDiagnostic::new("invalid_name", "name", message));
    }
    if source.states.is_empty() {
        diagnostics.push(FlowDiagnostic::new(
            "states_empty",
            "states",
            "Flow must declare at least one state",
        ));
    }
    if source.states.len() > MAX_STATES {
        diagnostics.push(FlowDiagnostic::new(
            "too_many_states",
            "states",
            format!("Flow declares more than {MAX_STATES} states"),
        ));
    }
    if source.initial == CANCELLED_STATE_ID {
        diagnostics.push(FlowDiagnostic::new(
            "reserved_state",
            "initial",
            format!("{CANCELLED_STATE_ID} is reserved by the Flow runtime"),
        ));
    }

    let initial = StateId::new(source.initial.clone()).unwrap_or_else(|message| {
        diagnostics.push(FlowDiagnostic::new("invalid_initial", "initial", message));
        StateId(source.initial.clone())
    });

    let declared_names = source.states.keys().cloned().collect::<BTreeSet<_>>();
    if !declared_names.contains(initial.as_str()) {
        diagnostics.push(FlowDiagnostic::new(
            "initial_not_found",
            "initial",
            format!("initial state {initial:?} is not declared"),
        ));
    }
    if declared_names.contains(CANCELLED_STATE_ID) {
        diagnostics.push(FlowDiagnostic::new(
            "reserved_state",
            format!("states.{CANCELLED_STATE_ID}"),
            format!("{CANCELLED_STATE_ID} is reserved by the Flow runtime"),
        ));
    }

    let mut states = BTreeMap::new();
    for (state_name, state_source) in source.states {
        let state_path = format!("states.{state_name}");
        let state_id = StateId::new(state_name.clone()).unwrap_or_else(|message| {
            diagnostics.push(FlowDiagnostic::new(
                "invalid_state_id",
                state_path.clone(),
                message,
            ));
            StateId(state_name.clone())
        });
        validate_text(
            &mut diagnostics,
            "instructions",
            &format!("{state_path}.instructions"),
            &state_source.instructions,
            state_source.terminal,
        );
        if state_source.terminal && !state_source.transitions.is_empty() {
            diagnostics.push(FlowDiagnostic::new(
                "terminal_has_transitions",
                format!("{state_path}.transitions"),
                "terminal states must not declare outgoing transitions",
            ));
        }
        if !state_source.terminal && state_source.transitions.is_empty() {
            diagnostics.push(FlowDiagnostic::new(
                "non_terminal_without_transition",
                format!("{state_path}.transitions"),
                "non-terminal states must declare at least one transition",
            ));
        }
        if state_source.transitions.len() > MAX_TRANSITIONS_PER_STATE {
            diagnostics.push(FlowDiagnostic::new(
                "too_many_transitions",
                format!("{state_path}.transitions"),
                format!("state declares more than {MAX_TRANSITIONS_PER_STATE} transitions"),
            ));
        }

        let mut transitions = Vec::new();
        for (transition_name, transition_source) in state_source.transitions {
            let transition_path = format!("{state_path}.transitions.{transition_name}");
            if transition_name == CANCEL_TRANSITION_ID || transition_name.starts_with('$') {
                diagnostics.push(FlowDiagnostic::new(
                    "reserved_transition",
                    transition_path.clone(),
                    "transition identifiers beginning with '$' are reserved by the Flow runtime",
                ));
            }
            let transition_id =
                TransitionId::new(transition_name.clone()).unwrap_or_else(|message| {
                    diagnostics.push(FlowDiagnostic::new(
                        "invalid_transition_id",
                        transition_path.clone(),
                        message,
                    ));
                    TransitionId(transition_name.clone())
                });
            if transition_source.target == CANCELLED_STATE_ID
                || transition_source.target.starts_with('$')
            {
                diagnostics.push(FlowDiagnostic::new(
                    "reserved_transition_target",
                    format!("{transition_path}.target"),
                    "definition authors cannot target runtime-reserved states",
                ));
            }
            let target = StateId::new(transition_source.target.clone()).unwrap_or_else(|message| {
                diagnostics.push(FlowDiagnostic::new(
                    "invalid_transition_target",
                    format!("{transition_path}.target"),
                    message,
                ));
                StateId(transition_source.target.clone())
            });
            if !declared_names.contains(target.as_str()) {
                diagnostics.push(FlowDiagnostic::new(
                    "transition_target_not_found",
                    format!("{transition_path}.target"),
                    format!("transition target {target:?} is not declared"),
                ));
            }
            validate_text(
                &mut diagnostics,
                "condition",
                &format!("{transition_path}.condition"),
                &transition_source.condition,
                false,
            );
            transitions.push(CompiledTransition {
                id: transition_id,
                target,
                condition: transition_source.condition,
                synthetic: false,
            });
        }
        if !state_source.terminal {
            transitions.push(CompiledTransition {
                id: TransitionId(CANCEL_TRANSITION_ID.to_string()),
                target: StateId(CANCELLED_STATE_ID.to_string()),
                condition: CANCEL_CONDITION.to_string(),
                synthetic: true,
            });
        }
        states.insert(
            state_id.clone(),
            CompiledState {
                id: state_id,
                instructions: state_source.instructions,
                terminal: state_source.terminal,
                transitions,
            },
        );
    }

    if diagnostics.is_empty() {
        validate_graph(&states, &initial, &mut diagnostics);
    }
    if !diagnostics.is_empty() {
        return Err(FlowCompileError { diagnostics });
    }

    states.insert(
        StateId(CANCELLED_STATE_ID.to_string()),
        CompiledState {
            id: StateId(CANCELLED_STATE_ID.to_string()),
            instructions: String::new(),
            terminal: true,
            transitions: Vec::new(),
        },
    );

    Ok(CompiledFlowDefinition {
        schema_version: source.schema_version,
        name: source.name,
        initial,
        states,
        content_digest: content_digest(content),
    })
}

fn validate_identifier(kind: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{kind} identifier must not be empty"));
    }
    if value.len() > 128 {
        return Err(format!("{kind} identifier exceeds 128 bytes"));
    }
    if value.starts_with('$') {
        return Err(format!("{kind} identifier beginning with '$' is reserved"));
    }
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!(
            "{kind} identifier must contain only ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn validate_text(
    diagnostics: &mut Vec<FlowDiagnostic>,
    field: &str,
    path: &str,
    value: &str,
    allow_empty: bool,
) {
    if !allow_empty && value.trim().is_empty() {
        diagnostics.push(FlowDiagnostic::new(
            format!("{field}_empty"),
            path,
            format!("{field} must not be empty"),
        ));
    }
    if value.len() > MAX_TEXT_BYTES {
        diagnostics.push(FlowDiagnostic::new(
            format!("{field}_too_large"),
            path,
            format!("{field} exceeds {MAX_TEXT_BYTES} bytes"),
        ));
    }
}

fn validate_graph(
    states: &BTreeMap<StateId, CompiledState>,
    initial: &StateId,
    diagnostics: &mut Vec<FlowDiagnostic>,
) {
    if !states.contains_key(initial) {
        return;
    }
    let mut reachable = BTreeSet::new();
    let mut pending = VecDeque::from([initial.clone()]);
    while let Some(current) = pending.pop_front() {
        if !reachable.insert(current.clone()) {
            continue;
        }
        if let Some(state) = states.get(&current) {
            for transition in state
                .transitions
                .iter()
                .filter(|transition| !transition.synthetic)
            {
                pending.push_back(transition.target.clone());
            }
        }
    }
    for state_id in states.keys() {
        if !reachable.contains(state_id) {
            diagnostics.push(FlowDiagnostic::new(
                "unreachable_state",
                format!("states.{state_id}"),
                format!("state {state_id:?} is unreachable from initial state {initial:?}"),
            ));
        }
    }

    let terminal_states = states
        .values()
        .filter(|state| state.terminal)
        .map(|state| state.id.clone())
        .collect::<BTreeSet<_>>();
    if terminal_states.is_empty() {
        diagnostics.push(FlowDiagnostic::new(
            "terminal_missing",
            "states",
            "Flow must declare at least one terminal state",
        ));
        return;
    }

    let mut reverse: BTreeMap<StateId, Vec<StateId>> = BTreeMap::new();
    for state in states.values() {
        for transition in state
            .transitions
            .iter()
            .filter(|transition| !transition.synthetic)
        {
            reverse
                .entry(transition.target.clone())
                .or_default()
                .push(state.id.clone());
        }
    }
    let mut can_reach_terminal = terminal_states.clone();
    let mut pending = terminal_states.into_iter().collect::<VecDeque<_>>();
    while let Some(current) = pending.pop_front() {
        for predecessor in reverse.get(&current).into_iter().flatten() {
            if can_reach_terminal.insert(predecessor.clone()) {
                pending.push_back(predecessor.clone());
            }
        }
    }
    for state_id in reachable {
        if !can_reach_terminal.contains(&state_id) {
            diagnostics.push(FlowDiagnostic::new(
                "terminal_unreachable",
                format!("states.{state_id}"),
                format!(
                    "state {state_id:?} cannot reach a user-declared terminal state; the graph contains a closed non-terminal path"
                ),
            ));
        }
    }
}

fn content_digest(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    let mut encoded = String::with_capacity(7 + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

struct RejectImports;

impl SourceLoader for RejectImports {
    fn load(
        &mut self,
        _current_key: Option<&str>,
        specifier: &str,
    ) -> decodal::Result<LoadedSource> {
        Err(decodal::Diagnostic::new(
            decodal::DiagnosticKind::Import,
            decodal::Span::default(),
            format!("Flow source imports are not enabled: {specifier}"),
        ))
    }
}

fn decodal_data_to_json(data: &decodal::Data) -> serde_json::Value {
    match data {
        decodal::Data::Bool(value) => serde_json::Value::Bool(*value),
        decodal::Data::Int(value) => serde_json::Value::Number(serde_json::Number::from(*value)),
        decodal::Data::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        decodal::Data::String(value) => serde_json::Value::String(value.clone()),
        decodal::Data::Array(values) => {
            serde_json::Value::Array(values.iter().map(decodal_data_to_json).collect())
        }
        decodal::Data::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|field| (field.name.clone(), decodal_data_to_json(&field.value)))
                .collect(),
        ),
    }
}

fn one_diagnostic(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> FlowCompileError {
    FlowCompileError {
        diagnostics: vec![FlowDiagnostic::new(code, path, message)],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_source() -> &'static str {
        r#"{
            schema_version = 1;
            name = "coder-review";
            initial = "code";
            states = {
                code = {
                    instructions = "Implement and validate the requested change.";
                    transitions = {
                        review = {
                            target = "review";
                            condition = "Implementation and validation evidence are present.";
                        };
                    };
                };
                review = {
                    instructions = "Read the independent review result.";
                    transitions = {
                        done = {
                            target = "done";
                            condition = "The independent reviewer approved the implementation.";
                        };
                        fix = {
                            target = "fix";
                            condition = "The independent reviewer requested concrete changes.";
                        };
                    };
                };
                fix = {
                    instructions = "Resolve every open review finding and validate the fixes.";
                    transitions = {
                        review = {
                            target = "review";
                            condition = "The requested changes are resolved and ready for re-review.";
                        };
                    };
                };
                done = {
                    instructions = "";
                    terminal = true;
                };
            };
        }"#
    }

    #[test]
    fn compiles_definition_and_injects_cancel_transition() {
        let definition = compile_flow_source(valid_source()).expect("valid Flow");
        assert_eq!(definition.name, "coder-review");
        assert!(definition.content_digest.starts_with("sha256:"));
        let code = definition
            .state(&StateId::new("code").unwrap())
            .expect("code state");
        assert_eq!(code.transitions.len(), 2);
        assert!(
            code.transitions
                .iter()
                .any(|transition| transition.id.as_str() == CANCEL_TRANSITION_ID
                    && transition.synthetic)
        );
        let cancelled = definition
            .state(&StateId(CANCELLED_STATE_ID.to_string()))
            .expect("synthetic cancelled state");
        assert!(cancelled.terminal);
        assert!(cancelled.instructions.is_empty());
    }

    #[test]
    fn rejects_unknown_structural_fields() {
        let source = valid_source().replace(
            "schema_version = 1;",
            "schema_version = 1; unexpected = true;",
        );
        let error = compile_flow_source(&source).unwrap_err();
        assert_eq!(error.diagnostics[0].code, "schema_decode");
        assert!(error.diagnostics[0].message.contains("unexpected"));
    }

    #[test]
    fn rejects_definition_authored_cancel_target() {
        let source = valid_source().replace("target = \"review\";", "target = \"$cancelled\";");
        let error = compile_flow_source(&source).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| { diagnostic.code == "reserved_transition_target" })
        );
    }

    #[test]
    fn rejects_unreachable_state() {
        let source = valid_source().replace(
            "done = {\n                    instructions = \"\";",
            "unused = { instructions = \"unused\"; terminal = true; };\n                done = {\n                    instructions = \"\";",
        );
        let error = compile_flow_source(&source).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "unreachable_state")
        );
    }

    #[test]
    fn rejects_closed_non_terminal_cycle_even_with_synthetic_cancel() {
        let source = r#"{
            schema_version = 1;
            name = "closed-cycle";
            initial = "a";
            states = {
                a = {
                    instructions = "a";
                    transitions = { to_b = { target = "b"; condition = "go b"; }; };
                };
                b = {
                    instructions = "b";
                    transitions = { to_a = { target = "a"; condition = "go a"; }; };
                };
                done = { instructions = ""; terminal = true; };
            };
        }"#;
        let error = compile_flow_source(source).unwrap_err();
        assert!(
            error
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "terminal_unreachable")
        );
    }
}
