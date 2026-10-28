//! Native input contracts own both WIP declarations and domain-command conversion.
//!
//! These shapes are authored for native operations, not derived from Tool schemas. WIP validates
//! the generated declaration before destination mapping; authorization and business constraints
//! remain with the domain handler. Conversion neither resolves objects nor grants access.
//!
//! WIP 0.2 represents unions as records with `$case` and an optional `value`, and has no nullable
//! type. Nullable inputs therefore expose an explicit `null | value(T)` union rather than an
//! opaque JSON escape hatch. Omission and explicit null remain distinct.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json};
use wip_protocol::{
    CallOperationRequest, Documentation, EnumCase, FieldDeclaration, INTERFACE_FORMAT_V1,
    InterfaceDescriptor, InterfaceReference, InterfaceTarget, OperationDeclaration,
    ParameterDeclaration, ProtocolError, ProtocolErrorCode, ReturnDeclaration, Target, TypeExpr,
    UnionCase, Value,
};

use crate::wip::WipOperationError;

#[derive(Clone, Debug)]
pub struct Contract {
    pub name: &'static str,
    pub summary: &'static str,
    pub parameters: Vec<Input>,
}

#[derive(Clone, Debug)]
pub struct Input {
    pub name: &'static str,
    pub required: bool,
    pub ty: Shape,
    pub destination: Destination,
}

#[derive(Clone, Copy, Debug, Default)]
pub enum Destination {
    #[default]
    Same,
    Rename(&'static str),
    /// Merge a converted object into its enclosing command, rejecting collisions.
    Flatten,
}

#[derive(Clone, Debug)]
pub enum Shape {
    String,
    Boolean,
    Integer,
    Enum(Vec<&'static str>),
    Nullable(Box<Shape>),
    List(Box<Shape>),
    Record(Vec<Input>),
    /// Record payloads merge into `{tag: case}`; other payloads use the `value` field.
    Union {
        tag: &'static str,
        cases: Vec<Case>,
    },
    /// Payload-free cases become strings; payload cases become `{case: payload}`.
    ExternalUnion {
        cases: Vec<Case>,
    },
    Reference(ReferenceKind),
}

#[derive(Clone, Debug)]
pub struct Case {
    pub name: &'static str,
    pub payload: Option<Shape>,
}

#[derive(Clone, Copy, Debug)]
pub enum ReferenceKind {
    Ticket,
    MergeRequest,
    Objective,
}

/// Exact collection routes allocated by the caller's current namespace registry.
/// There are deliberately no default paths or process-global namespace assumptions.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceRoots<'a> {
    pub ticket: &'a str,
    pub merge_request: &'a str,
    pub objective: &'a str,
}

impl Contract {
    pub fn declaration(&self) -> OperationDeclaration {
        OperationDeclaration {
            name: self.name.into(),
            documentation: Some(Documentation {
                summary: self.summary.into(),
                details: None,
            }),
            parameters: self
                .parameters
                .iter()
                .map(|input| ParameterDeclaration {
                    name: input.name.into(),
                    required: input.required,
                    documentation: input.ty.documentation(),
                    r#type: input.ty.declaration(),
                })
                .collect(),
            // Typed result Entries are owned separately from this input contract.
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Json,
            },
        }
    }

    pub fn convert(
        &self,
        arguments: &BTreeMap<String, Value>,
        roots: &ReferenceRoots<'_>,
    ) -> Result<Json, WipOperationError> {
        let descriptor = InterfaceDescriptor {
            format: INTERFACE_FORMAT_V1.into(),
            documentation: None,
            types: Vec::new(),
            operations: vec![self.declaration()],
        };
        descriptor.validate().map_err(invalid)?;
        // validate_call is the upstream type validator. These inert targets exist only to
        // supply its request envelope; no Host, observation, dispatch, or authority is involved.
        descriptor
            .validate_call(&CallOperationRequest {
                target: Target {
                    path: "/".into(),
                    validator: None,
                },
                interface: InterfaceTarget {
                    scope_ref: None,
                    reference: InterfaceReference {
                        scope: "/".into(),
                        name: "native.contract/v1".into(),
                    },
                    validator: None,
                },
                operation: self.name.into(),
                arguments: arguments.clone(),
            })
            .map_err(invalid)?;
        convert_record(&self.parameters, arguments, roots).map(Json::Object)
    }
}

impl Shape {
    fn documentation(&self) -> Option<Documentation> {
        let summary = match self {
            Self::Reference(ReferenceKind::Ticket) => {
                "Ticket Entry: an exact item path under the current Ticket collection"
            }
            Self::Reference(ReferenceKind::MergeRequest) => {
                "Merge Request Entry: an exact item path under the current Merge Request collection"
            }
            Self::Reference(ReferenceKind::Objective) => {
                "Objective Entry: an exact item path under the current Objective collection; leaf O- followed by decimal digits, or an 8–128 character ASCII alphanumeric canonical ID"
            }
            Self::Nullable(_) => {
                "Explicit null uses {$case: null}; a present value uses {$case: value, value: T}"
            }
            _ => return None,
        };
        Some(Documentation {
            summary: summary.into(),
            details: None,
        })
    }

    fn declaration(&self) -> TypeExpr {
        match self {
            Self::String => TypeExpr::String,
            Self::Boolean => TypeExpr::Boolean,
            Self::Integer => TypeExpr::Integer,
            Self::Enum(cases) => TypeExpr::Enum {
                cases: cases
                    .iter()
                    .map(|name| EnumCase {
                        name: (*name).into(),
                        documentation: None,
                    })
                    .collect(),
            },
            Self::Nullable(inner) => TypeExpr::Union {
                cases: vec![
                    UnionCase {
                        name: "null".into(),
                        documentation: None,
                        payload: None,
                    },
                    UnionCase {
                        name: "value".into(),
                        documentation: inner.documentation(),
                        payload: Some(inner.declaration()),
                    },
                ],
            },
            Self::List(items) => TypeExpr::List {
                items: Box::new(items.declaration()),
            },
            Self::Record(fields) => TypeExpr::Record {
                fields: fields
                    .iter()
                    .map(|input| FieldDeclaration {
                        name: input.name.into(),
                        required: input.required,
                        documentation: input.ty.documentation(),
                        r#type: input.ty.declaration(),
                    })
                    .collect(),
            },
            Self::Union { cases, .. } | Self::ExternalUnion { cases } => TypeExpr::Union {
                cases: cases
                    .iter()
                    .map(|case| UnionCase {
                        name: case.name.into(),
                        documentation: None,
                        payload: case.payload.as_ref().map(Self::declaration),
                    })
                    .collect(),
            },
            Self::Reference(_) => TypeExpr::Entry,
        }
    }

    fn convert(
        &self,
        value: &Value,
        roots: &ReferenceRoots<'_>,
    ) -> Result<Json, WipOperationError> {
        match (self, value) {
            (Self::String | Self::Enum(_), Value::String(value)) => Ok(Json::String(value.clone())),
            (Self::Boolean, Value::Boolean(value)) => Ok(Json::Bool(*value)),
            (Self::Integer, Value::Integer(value)) => Ok(Json::from(*value)),
            (Self::Reference(kind), Value::String(path)) => {
                kind.identity(path, roots).map(|id| Json::String(id.into()))
            }
            (Self::Record(fields), Value::Record(values)) => {
                convert_record(fields, values, roots).map(Json::Object)
            }
            (Self::List(item), Value::List(values)) => values
                .iter()
                .map(|value| item.convert(value, roots))
                .collect::<Result<Vec<_>, _>>()
                .map(Json::Array),
            (Self::Nullable(inner), Value::Record(fields)) => match union_case(fields)? {
                "null" => Ok(Json::Null),
                "value" => inner.convert(union_payload(fields)?, roots),
                _ => Err(invalid("unknown nullable case")),
            },
            (Self::ExternalUnion { cases }, Value::Record(fields)) => {
                let name = union_case(fields)?;
                let case = cases
                    .iter()
                    .find(|case| case.name == name)
                    .ok_or_else(|| invalid("unknown union case"))?;
                match &case.payload {
                    None => Ok(Json::String(name.into())),
                    Some(payload) => {
                        let converted = payload.convert(union_payload(fields)?, roots)?;
                        let mut output = Map::new();
                        insert(&mut output, name, converted)?;
                        Ok(Json::Object(output))
                    }
                }
            }
            (Self::Union { tag, cases }, Value::Record(fields)) => {
                let name = union_case(fields)?;
                let case = cases
                    .iter()
                    .find(|case| case.name == name)
                    .ok_or_else(|| invalid("unknown union case"))?;
                let mut output = Map::new();
                insert(&mut output, tag, Json::String(name.into()))?;
                if let Some(payload) = &case.payload {
                    let converted = payload.convert(union_payload(fields)?, roots)?;
                    if let Json::Object(fields) = converted {
                        for (name, value) in fields {
                            insert(&mut output, &name, value)?;
                        }
                    } else {
                        insert(&mut output, "value", converted)?;
                    }
                }
                Ok(Json::Object(output))
            }
            _ => Err(invalid("value does not match the declared native shape")),
        }
    }
}

fn union_case(fields: &BTreeMap<String, Value>) -> Result<&str, WipOperationError> {
    match fields.get("$case") {
        Some(Value::String(case)) => Ok(case),
        _ => Err(invalid("union requires a string $case")),
    }
}

fn union_payload(fields: &BTreeMap<String, Value>) -> Result<&Value, WipOperationError> {
    fields
        .get("value")
        .ok_or_else(|| invalid("union case requires a value payload"))
}

fn convert_record(
    fields: &[Input],
    values: &BTreeMap<String, Value>,
    roots: &ReferenceRoots<'_>,
) -> Result<Map<String, Json>, WipOperationError> {
    let mut output = Map::new();
    for field in fields {
        let Some(value) = values.get(field.name) else {
            continue;
        };
        let value = field.ty.convert(value, roots)?;
        match field.destination {
            Destination::Same => insert(&mut output, field.name, value)?,
            Destination::Rename(name) => insert(&mut output, name, value)?,
            Destination::Flatten => {
                let Json::Object(fields) = value else {
                    return Err(invalid(format!(
                        "flattened field `{}` must convert to an object",
                        field.name
                    )));
                };
                for (name, value) in fields {
                    insert(&mut output, &name, value)?;
                }
            }
        }
    }
    Ok(output)
}

fn insert(
    output: &mut Map<String, Json>,
    name: &str,
    value: Json,
) -> Result<(), WipOperationError> {
    if output.contains_key(name) {
        return Err(invalid(format!("destination collision at `{name}`")));
    }
    output.insert(name.into(), value);
    Ok(())
}

impl ReferenceKind {
    fn identity<'a>(
        self,
        path: &'a str,
        roots: &ReferenceRoots<'_>,
    ) -> Result<&'a str, WipOperationError> {
        let root = match self {
            Self::Ticket => roots.ticket,
            Self::MergeRequest => roots.merge_request,
            Self::Objective => roots.objective,
        };
        wip_protocol::validate_path(root).map_err(invalid)?;
        wip_protocol::validate_path(path).map_err(invalid)?;
        let leaf = path
            .strip_prefix(root)
            .and_then(|suffix| suffix.strip_prefix('/'))
            .filter(|leaf| !leaf.contains('/'))
            .ok_or_else(|| invalid("reference must name an item in its exact collection"))?;
        // Keep the dynamic resolvers' route-leaf grammar, without accepting bare identities,
        // encoded separators, descendants, prefix siblings, or cross-namespace references.
        let valid = match self {
            Self::Ticket | Self::Objective => {
                let prefix = if matches!(self, Self::Ticket) {
                    "T-"
                } else {
                    "O-"
                };
                leaf.strip_prefix(prefix).is_some_and(|sequence| {
                    !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
                }) || ((8..=128).contains(&leaf.len())
                    && leaf.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            }
            Self::MergeRequest => {
                !leaf.is_empty()
                    && leaf.len() <= 128
                    && leaf != "."
                    && leaf != ".."
                    && leaf.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
                    })
            }
        };
        if !valid {
            return Err(invalid("reference has an invalid route identity"));
        }
        Ok(leaf)
    }
}

fn invalid(message: impl std::fmt::Display) -> WipOperationError {
    WipOperationError::Protocol(ProtocolError {
        code: ProtocolErrorCode::InvalidArguments,
        message: format!("native contract: {message}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ROOTS: ReferenceRoots<'_> = ReferenceRoots {
        ticket: "/workspace/cases",
        merge_request: "/workspace/changes",
        objective: "/workspace/goals",
    };

    fn field(name: &'static str, required: bool, ty: Shape) -> Input {
        Input {
            name,
            required,
            ty,
            destination: Destination::Same,
        }
    }

    fn contract(parameters: Vec<Input>) -> Contract {
        Contract {
            name: "apply",
            summary: "Apply a native domain command",
            parameters,
        }
    }

    fn fields<const N: usize>(values: [(&str, Value); N]) -> BTreeMap<String, Value> {
        values
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect()
    }

    fn text(value: &str) -> Value {
        Value::String(value.into())
    }

    fn variant(case: &str, payload: Option<Value>) -> Value {
        let mut values = fields([("$case", text(case))]);
        if let Some(payload) = payload {
            values.insert("value".into(), payload);
        }
        Value::Record(values)
    }

    fn rejected(contract: &Contract, arguments: BTreeMap<String, Value>) {
        let error = contract.convert(&arguments, &ROOTS).unwrap_err();
        assert!(
            matches!(
                &error,
                WipOperationError::Protocol(ProtocolError {
                    code: ProtocolErrorCode::InvalidArguments,
                    message,
                }) if message.starts_with("native contract:")
            ),
            "expected a prefixed InvalidArguments error"
        );
    }

    fn failed(error: WipOperationError) -> Json {
        match error {
            WipOperationError::Protocol(error) => panic!("{}", error.message),
            _ => panic!("unexpected non-protocol failure"),
        }
    }

    #[test]
    fn union_record_payload_converts_to_tagged_command_and_maps_destinations() {
        let contract = contract(vec![Input {
            name: "principal",
            required: true,
            ty: Shape::Union {
                tag: "kind",
                cases: vec![
                    Case {
                        name: "user",
                        payload: Some(Shape::Record(vec![Input {
                            name: "account",
                            required: true,
                            ty: Shape::String,
                            destination: Destination::Rename("account_id"),
                        }])),
                    },
                    Case {
                        name: "system",
                        payload: None,
                    },
                ],
            },
            destination: Destination::Flatten,
        }]);
        assert!(matches!(
            contract.declaration().parameters[0].r#type,
            TypeExpr::Union { .. }
        ));
        let arguments = fields([(
            "principal",
            variant(
                "user",
                Some(Value::Record(fields([("account", text("A-1"))]))),
            ),
        )]);
        assert_eq!(
            contract.convert(&arguments, &ROOTS).unwrap_or_else(failed),
            json!({"kind": "user", "account_id": "A-1"})
        );
        assert_eq!(
            contract
                .convert(&fields([("principal", variant("system", None))]), &ROOTS)
                .unwrap_or_else(failed),
            json!({"kind": "system"})
        );
        for bad in [
            variant("unknown", None),
            variant("user", None),
            variant("user", Some(text("not a record"))),
            variant("system", Some(Value::Unit)),
            Value::Record(fields([
                ("$case", text("system")),
                ("extra", Value::Boolean(true)),
            ])),
        ] {
            rejected(&contract, fields([("principal", bad)]));
        }
    }

    #[test]
    fn enum_lists_and_nested_records_are_closed_before_mapping() {
        let contract = contract(vec![field(
            "filter",
            true,
            Shape::Record(vec![field(
                "states",
                true,
                Shape::List(Box::new(Shape::Enum(vec!["open", "closed"]))),
            )]),
        )]);
        let valid = Value::Record(fields([(
            "states",
            Value::List(vec![text("open"), text("closed")]),
        )]));
        assert_eq!(
            contract
                .convert(&fields([("filter", valid.clone())]), &ROOTS)
                .unwrap_or_else(failed),
            json!({"filter": {"states": ["open", "closed"]}})
        );
        for bad in [
            Value::Record(fields([("states", Value::List(vec![text("queued")]))])),
            Value::Record(fields([("states", Value::Unit)])),
            Value::Record(fields([])),
            Value::Record(fields([
                ("states", Value::List(vec![])),
                ("undeclared", text("hidden")),
            ])),
        ] {
            rejected(&contract, fields([("filter", bad)]));
        }
        rejected(
            &contract,
            fields([("filter", valid), ("extra", Value::Unit)]),
        );
    }

    #[test]
    fn optional_nullable_preserves_omission_null_and_present_value() {
        let contract = contract(vec![field(
            "note",
            false,
            Shape::Nullable(Box::new(Shape::String)),
        )]);
        let declaration = contract.declaration();
        assert!(!declaration.parameters[0].required);
        assert!(matches!(
            &declaration.parameters[0].r#type,
            TypeExpr::Union { cases } if cases[0].name == "null"
                && cases[0].payload.is_none()
                && cases[1].name == "value"
                && cases[1].payload == Some(TypeExpr::String)
        ));
        assert_eq!(
            contract.convert(&fields([]), &ROOTS).unwrap_or_else(failed),
            json!({})
        );
        for (value, expected) in [
            (variant("null", None), json!({"note": null})),
            (
                variant("value", Some(text("hello"))),
                json!({"note": "hello"}),
            ),
        ] {
            assert_eq!(
                contract
                    .convert(&fields([("note", value)]), &ROOTS)
                    .unwrap_or_else(failed),
                expected
            );
        }
        for bad in [
            Value::Unit,
            text("hello"),
            variant("null", Some(text("unexpected"))),
            variant("value", None),
            variant("value", Some(Value::Unit)),
        ] {
            rejected(&contract, fields([("note", bad)]));
        }
        let nonnullable = contract_with_string();
        rejected(&nonnullable, fields([("value", Value::Unit)]));
        rejected(&nonnullable, fields([]));
    }

    fn contract_with_string() -> Contract {
        contract(vec![field("value", true, Shape::String)])
    }

    #[test]
    fn scalar_union_payload_converts_without_losing_case() {
        let contract = contract(vec![field(
            "reason",
            true,
            Shape::Union {
                tag: "reason",
                cases: vec![Case {
                    name: "other",
                    payload: Some(Shape::String),
                }],
            },
        )]);
        assert_eq!(
            contract
                .convert(
                    &fields([("reason", variant("other", Some(text("explanation"))))]),
                    &ROOTS,
                )
                .unwrap_or_else(failed),
            json!({"reason": {"reason": "other", "value": "explanation"}})
        );
        rejected(
            &contract,
            fields([("reason", variant("other", Some(Value::Unit)))]),
        );
    }

    #[test]
    fn external_union_converts_to_typed_ticket_closure_reasons() {
        use ticket::TicketClosureReason;

        let contract = contract(vec![field(
            "reason",
            true,
            Shape::ExternalUnion {
                cases: vec![
                    Case {
                        name: "completed",
                        payload: None,
                    },
                    Case {
                        name: "not_planned",
                        payload: None,
                    },
                    Case {
                        name: "duplicate",
                        payload: None,
                    },
                    Case {
                        name: "unknown",
                        payload: None,
                    },
                    Case {
                        name: "other",
                        payload: Some(Shape::String),
                    },
                ],
            },
        )]);
        assert!(matches!(
            contract.declaration().parameters[0].r#type,
            TypeExpr::Union { .. }
        ));
        for (value, expected_json, expected_reason) in [
            (
                variant("completed", None),
                json!("completed"),
                TicketClosureReason::Completed,
            ),
            (
                variant("not_planned", None),
                json!("not_planned"),
                TicketClosureReason::NotPlanned,
            ),
            (
                variant("duplicate", None),
                json!("duplicate"),
                TicketClosureReason::Duplicate,
            ),
            (
                variant("unknown", None),
                json!("unknown"),
                TicketClosureReason::Unknown,
            ),
            (
                variant("other", Some(text("explanation"))),
                json!({"other": "explanation"}),
                TicketClosureReason::Other("explanation".into()),
            ),
        ] {
            let converted = contract
                .convert(&fields([("reason", value)]), &ROOTS)
                .unwrap_or_else(failed);
            assert_eq!(converted["reason"], expected_json);
            let reason: TicketClosureReason =
                serde_json::from_value(converted["reason"].clone()).unwrap();
            assert_eq!(reason, expected_reason);
        }
        for bad in [
            variant("unknown_case", None),
            variant("completed", Some(Value::Unit)),
            variant("other", None),
            variant("other", Some(Value::Boolean(true))),
            Value::Record(fields([
                ("$case", text("completed")),
                ("extra", text("hidden")),
            ])),
        ] {
            rejected(&contract, fields([("reason", bad)]));
        }
    }

    #[test]
    fn objective_entries_use_exact_configured_collection_and_route_leaf_grammar() {
        let contract = contract(vec![field(
            "linked",
            true,
            Shape::Reference(ReferenceKind::Objective),
        )]);
        let parameter = &contract.declaration().parameters[0];
        assert_eq!(parameter.r#type, TypeExpr::Entry);
        assert!(
            parameter
                .documentation
                .as_ref()
                .unwrap()
                .summary
                .contains("8–128")
        );
        for identity in ["O-42", "00001M60CK1PM"] {
            assert_eq!(
                contract
                    .convert(
                        &fields([("linked", text(&format!("{}/{identity}", ROOTS.objective)))]),
                        &ROOTS
                    )
                    .unwrap_or_else(failed),
                json!({"linked": identity})
            );
        }
        for bad in [
            "/workspace/cases/O-42",
            "/workspace/changes/O-42",
            "/other/goals/O-42",
            "/workspace/goals-extra/O-42",
            "/workspace/goals/O-42/child",
            "/workspace/goals/O-42/",
            "/workspace/goals//O-42",
            "/workspace/goals/%4f-42",
            "/workspace/goals/O-no",
            "/workspace/goals/T-42",
            "/workspace/goals/short",
            "/workspace/goals/..",
            "/workspace/goals",
            "O-42",
        ] {
            rejected(&contract, fields([("linked", text(bad))]));
        }
        let moved = ReferenceRoots {
            objective: "/moved/goals",
            ..ROOTS
        };
        assert!(
            contract
                .convert(&fields([("linked", text("/workspace/goals/O-42"))]), &moved)
                .is_err()
        );
        assert_eq!(
            contract
                .convert(&fields([("linked", text("/moved/goals/O-42"))]), &moved)
                .unwrap_or_else(failed),
            json!({"linked": "O-42"})
        );
    }

    #[test]
    fn entry_references_require_exact_caller_namespace_collection_and_valid_leaf() {
        let contract = contract(vec![
            field("ticket", true, Shape::Reference(ReferenceKind::Ticket)),
            field(
                "change",
                true,
                Shape::Reference(ReferenceKind::MergeRequest),
            ),
        ]);
        let arguments = fields([
            ("ticket", text("/workspace/cases/T-42")),
            ("change", text("/workspace/changes/MR-9")),
        ]);
        assert!(matches!(
            contract.declaration().parameters[0].r#type,
            TypeExpr::Entry
        ));
        assert_eq!(
            contract.convert(&arguments, &ROOTS).unwrap_or_else(failed),
            json!({"ticket": "T-42", "change": "MR-9"})
        );
        for (field, invalid_path) in [
            ("ticket", "/workspace/changes/T-42"),
            ("ticket", "/other/cases/T-42"),
            ("ticket", "/workspace/cases-extra/T-42"),
            ("ticket", "/workspace/cases/T-42/child"),
            ("ticket", "/workspace/cases/MR-9"),
            ("ticket", "/workspace/cases/T-no"),
            ("ticket", "/workspace/cases"),
            ("ticket", "T-42"),
            ("ticket", "/workspace/cases//T-42"),
            ("ticket", "/workspace/cases/T-42/"),
            ("ticket", "/workspace/cases/%54-42"),
            ("change", "/workspace/cases/MR-9"),
            ("change", "/workspace/changes/.."),
            ("change", "/workspace/changes/MR%2f9"),
            ("change", "/workspace/changes/MR-9?query"),
        ] {
            let mut invalid = arguments.clone();
            invalid.insert(field.into(), text(invalid_path));
            rejected(&contract, invalid);
        }
        let moved_roots = ReferenceRoots {
            ticket: "/moved/cases",
            merge_request: ROOTS.merge_request,
            objective: ROOTS.objective,
        };
        assert!(contract.convert(&arguments, &moved_roots).is_err());
        let mut moved = arguments;
        moved.insert("ticket".into(), text("/moved/cases/T-42"));
        assert_eq!(
            contract
                .convert(&moved, &moved_roots)
                .unwrap_or_else(failed)["ticket"],
            "T-42"
        );
    }

    #[test]
    fn scalar_validation_rejects_wrong_types_and_unsafe_integers() {
        for (shape, good, bad) in [
            (Shape::String, text("x"), Value::Integer(1)),
            (Shape::Boolean, Value::Boolean(true), text("true")),
            (Shape::Integer, Value::Integer(1), Value::Number(1.5)),
            (
                Shape::Integer,
                Value::Integer(0),
                Value::Integer(wip_protocol::MAX_SAFE_INTEGER + 1),
            ),
        ] {
            let contract = contract(vec![field("value", true, shape)]);
            assert!(contract.convert(&fields([("value", good)]), &ROOTS).is_ok());
            rejected(&contract, fields([("value", bad)]));
        }
    }

    #[test]
    fn destination_collisions_fail_instead_of_overwriting_command_fields() {
        let contract = contract(vec![
            field("id", true, Shape::String),
            Input {
                name: "details",
                required: true,
                ty: Shape::Record(vec![field("id", true, Shape::String)]),
                destination: Destination::Flatten,
            },
        ]);
        rejected(
            &contract,
            fields([
                ("id", text("outer")),
                ("details", Value::Record(fields([("id", text("inner"))]))),
            ]),
        );
        let tagged = contract_with_tag_collision();
        rejected(
            &tagged,
            fields([(
                "choice",
                variant("case", Some(Value::Record(fields([("kind", text("bad"))])))),
            )]),
        );
    }

    fn contract_with_tag_collision() -> Contract {
        contract(vec![field(
            "choice",
            true,
            Shape::Union {
                tag: "kind",
                cases: vec![Case {
                    name: "case",
                    payload: Some(Shape::Record(vec![field("kind", true, Shape::String)])),
                }],
            },
        )])
    }
}
