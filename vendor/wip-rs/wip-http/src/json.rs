use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value as Json};
use wip_protocol::{
    CallOperationRequest, CallOperationResponse, Documentation, EnumCase, FetchInterfaceRequest,
    FetchInterfaceResponse, FieldDeclaration, INTERFACE_FORMAT_V1, InterfaceDescriptor,
    InterfaceReference, InterfaceTarget, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER, Object,
    ObjectObservation, ObserveRequest, ObserveResponse, OperationDeclaration, ParameterDeclaration,
    ReturnDeclaration, Target, TypeDeclaration, TypeExpr, UnionCase, Value,
};

use crate::{BodyKind, CodecError, Limits};

struct StrictJson(Json);

impl<'de> Deserialize<'de> for StrictJson {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate object members")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::Null))
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Number::from_f64(value)
            .map(Json::Number)
            .map(StrictJson)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::String(value.into())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJson(Json::String(value)))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(StrictJson(value)) = sequence.next_element()? {
            values.push(value);
        }
        Ok(StrictJson(Json::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(name) = object.next_key::<String>()? {
            if values.contains_key(&name) {
                return Err(de::Error::custom(format!(
                    "duplicate JSON object member `{name}`"
                )));
            }
            let StrictJson(value) = object.next_value()?;
            values.insert(name, value);
        }
        Ok(StrictJson(Json::Object(values)))
    }
}

pub(crate) fn parse(body: &[u8], kind: BodyKind, limits: Limits) -> Result<Json, CodecError> {
    let maximum = match kind {
        BodyKind::Request => limits.max_request_bytes,
        BodyKind::Response => limits.max_response_bytes,
    };
    if body.len() > maximum {
        return Err(CodecError::BodyTooLarge {
            actual: body.len(),
            maximum,
        });
    }
    let StrictJson(value) = serde_json::from_slice(body).map_err(CodecError::InvalidJson)?;
    check_nesting(&value, 0, limits.max_nesting)?;
    Ok(value)
}

pub(crate) fn serialize(
    value: &Json,
    kind: BodyKind,
    limits: Limits,
) -> Result<Vec<u8>, CodecError> {
    check_nesting(value, 0, limits.max_nesting)?;
    let body = serde_json::to_vec(value).expect("serde_json::Value serialization cannot fail");
    let maximum = match kind {
        BodyKind::Request => limits.max_request_bytes,
        BodyKind::Response => limits.max_response_bytes,
    };
    if body.len() > maximum {
        return Err(CodecError::BodyTooLarge {
            actual: body.len(),
            maximum,
        });
    }
    Ok(body)
}

fn check_nesting(value: &Json, depth: usize, maximum: usize) -> Result<(), CodecError> {
    match value {
        Json::Array(items) => {
            let next = depth.saturating_add(1);
            if next > maximum {
                return Err(CodecError::NestingTooDeep { maximum });
            }
            for item in items {
                check_nesting(item, next, maximum)?;
            }
        }
        Json::Object(fields) => {
            let next = depth.saturating_add(1);
            if next > maximum {
                return Err(CodecError::NestingTooDeep { maximum });
            }
            for item in fields.values() {
                check_nesting(item, next, maximum)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn object<'a>(value: &'a Json, field: &str) -> Result<&'a Map<String, Json>, CodecError> {
    value
        .as_object()
        .ok_or_else(|| invalid(field, "expected object"))
}

fn exact_object<'a>(
    value: &'a Json,
    field: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Json>, CodecError> {
    let object = object(value, field)?;
    for name in object.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(CodecError::UnknownField {
                field: qualified(field, name),
            });
        }
    }
    Ok(object)
}

fn required<'a>(object: &'a Map<String, Json>, name: &str) -> Result<&'a Json, CodecError> {
    object.get(name).ok_or_else(|| CodecError::MissingField {
        field: name.to_owned(),
    })
}

fn optional<'a>(object: &'a Map<String, Json>, name: &str) -> Result<Option<&'a Json>, CodecError> {
    match object.get(name) {
        Some(Json::Null) => Err(CodecError::NullOptionalField {
            field: name.to_owned(),
        }),
        value => Ok(value),
    }
}

fn string(value: &Json, field: &str) -> Result<String, CodecError> {
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid(field, "expected string"))
}

fn boolean(value: &Json, field: &str) -> Result<bool, CodecError> {
    value
        .as_bool()
        .ok_or_else(|| invalid(field, "expected boolean"))
}

fn u32_number(value: &Json, field: &str) -> Result<u32, CodecError> {
    let number = value
        .as_u64()
        .ok_or_else(|| invalid(field, "expected unsigned integer"))?;
    u32::try_from(number).map_err(|_| invalid(field, "integer exceeds u32"))
}

fn array<'a>(value: &'a Json, field: &str) -> Result<&'a [Json], CodecError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| invalid(field, "expected array"))
}

fn invalid(field: &str, reason: &str) -> CodecError {
    CodecError::InvalidField {
        field: field.to_owned(),
        reason: reason.to_owned(),
    }
}

fn qualified(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}

fn base64_decode(value: &Json, field: &str) -> Result<Vec<u8>, CodecError> {
    let encoded = value
        .as_str()
        .ok_or_else(|| invalid(field, "expected base64 string"))?;
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|_| CodecError::InvalidBase64 {
            field: field.to_owned(),
        })?;
    if STANDARD.encode(&bytes) != encoded {
        return Err(CodecError::InvalidBase64 {
            field: field.to_owned(),
        });
    }
    Ok(bytes)
}

fn base64_encode(bytes: &[u8]) -> Json {
    Json::String(STANDARD.encode(bytes))
}

fn insert_optional_string(map: &mut Map<String, Json>, name: &str, value: &Option<String>) {
    if let Some(value) = value {
        map.insert(name.into(), Json::String(value.clone()));
    }
}

fn insert_optional_bytes(map: &mut Map<String, Json>, name: &str, value: &Option<Vec<u8>>) {
    if let Some(value) = value {
        map.insert(name.into(), base64_encode(value));
    }
}

pub(crate) fn observe_request_to_json(request: &ObserveRequest) -> Json {
    serde_json::json!({"path": request.path, "depth": request.depth})
}

pub(crate) fn observe_request_from_json(value: &Json) -> Result<ObserveRequest, CodecError> {
    let fields = exact_object(value, "request", &["path", "depth"])?;
    let request = ObserveRequest {
        path: string(required(fields, "path")?, "path")?,
        depth: u32_number(required(fields, "depth")?, "depth")?,
    };
    request.validate()?;
    Ok(request)
}

fn interface_reference_to_json(reference: &InterfaceReference) -> Json {
    serde_json::json!({"scope": reference.scope, "name": reference.name})
}

fn interface_reference_from_json(
    value: &Json,
    field: &str,
) -> Result<InterfaceReference, CodecError> {
    let fields = exact_object(value, field, &["scope", "name"])?;
    let reference = InterfaceReference {
        scope: string(required(fields, "scope")?, &qualified(field, "scope"))?,
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
    };
    reference.validate()?;
    Ok(reference)
}

pub(crate) fn fetch_interface_request_to_json(request: &FetchInterfaceRequest) -> Json {
    serde_json::json!({"interface": interface_reference_to_json(&request.interface)})
}

pub(crate) fn fetch_interface_request_from_json(
    value: &Json,
) -> Result<FetchInterfaceRequest, CodecError> {
    let fields = exact_object(value, "request", &["interface"])?;
    Ok(FetchInterfaceRequest {
        interface: interface_reference_from_json(required(fields, "interface")?, "interface")?,
    })
}

pub(crate) fn object_to_json(object: &Object) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(object.name.clone()));
    insert_optional_string(&mut fields, "description", &object.description);
    fields.insert(
        "interfaces".into(),
        Json::Array(
            object
                .interfaces
                .iter()
                .map(interface_reference_to_json)
                .collect(),
        ),
    );
    insert_optional_string(&mut fields, "ref", &object.r#ref);
    insert_optional_bytes(&mut fields, "validator", &object.validator);
    Json::Object(fields)
}

pub(crate) fn object_from_json(value: &Json, field: &str) -> Result<Object, CodecError> {
    let fields = exact_object(
        value,
        field,
        &["name", "description", "interfaces", "ref", "validator"],
    )?;
    let interfaces = array(required(fields, "interfaces")?, "interfaces")?
        .iter()
        .enumerate()
        .map(|(index, value)| interface_reference_from_json(value, &format!("interfaces[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let object = Object {
        name: string(required(fields, "name")?, "name")?,
        description: optional(fields, "description")?
            .map(|value| string(value, "description"))
            .transpose()?,
        interfaces,
        r#ref: optional(fields, "ref")?
            .map(|value| string(value, "ref"))
            .transpose()?,
        validator: optional(fields, "validator")?
            .map(|value| base64_decode(value, "validator"))
            .transpose()?,
    };
    object.validate()?;
    Ok(object)
}

fn observation_to_json(node: &ObjectObservation) -> Json {
    let mut fields = Map::new();
    fields.insert("object".into(), object_to_json(&node.object));
    if let Some(children) = &node.children {
        fields.insert(
            "children".into(),
            Json::Array(children.iter().map(observation_to_json).collect()),
        );
    }
    Json::Object(fields)
}

fn observation_from_json(value: &Json, field: &str) -> Result<ObjectObservation, CodecError> {
    let fields = exact_object(value, field, &["object", "children"])?;
    let children = optional(fields, "children")?
        .map(|value| {
            array(value, &qualified(field, "children"))?
                .iter()
                .enumerate()
                .map(|(index, child)| {
                    observation_from_json(child, &format!("{field}.children[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(ObjectObservation {
        object: object_from_json(required(fields, "object")?, &qualified(field, "object"))?,
        children,
    })
}

pub(crate) fn observe_response_to_json(response: &ObserveResponse) -> Json {
    observation_to_json(response)
}

pub(crate) fn observe_response_from_json(value: &Json) -> Result<ObserveResponse, CodecError> {
    observation_from_json(value, "response")
}

fn documentation_to_json(documentation: &Documentation) -> Json {
    let mut fields = Map::new();
    fields.insert(
        "summary".into(),
        Json::String(documentation.summary.clone()),
    );
    insert_optional_string(&mut fields, "details", &documentation.details);
    Json::Object(fields)
}

fn documentation_from_json(value: &Json, field: &str) -> Result<Documentation, CodecError> {
    let fields = exact_object(value, field, &["summary", "details"])?;
    Ok(Documentation {
        summary: string(required(fields, "summary")?, &qualified(field, "summary"))?,
        details: optional(fields, "details")?
            .map(|value| string(value, &qualified(field, "details")))
            .transpose()?,
    })
}

fn type_expr_to_json(r#type: &TypeExpr) -> Json {
    let mut fields = Map::new();
    let kind = match r#type {
        TypeExpr::Unit => "unit",
        TypeExpr::Boolean => "boolean",
        TypeExpr::Integer => "integer",
        TypeExpr::Number => "number",
        TypeExpr::String => "string",
        TypeExpr::Bytes => "bytes",
        TypeExpr::Json => "json",
        TypeExpr::Entry => "entry",
        TypeExpr::Named { name } => {
            fields.insert("name".into(), Json::String(name.clone()));
            "named"
        }
        TypeExpr::Record {
            fields: record_fields,
        } => {
            fields.insert(
                "fields".into(),
                Json::Array(record_fields.iter().map(field_to_json).collect()),
            );
            "record"
        }
        TypeExpr::List { items } => {
            fields.insert("items".into(), type_expr_to_json(items));
            "list"
        }
        TypeExpr::Enum { cases } => {
            fields.insert(
                "cases".into(),
                Json::Array(cases.iter().map(enum_case_to_json).collect()),
            );
            "enum"
        }
        TypeExpr::Union { cases } => {
            fields.insert(
                "cases".into(),
                Json::Array(cases.iter().map(union_case_to_json).collect()),
            );
            "union"
        }
    };
    fields.insert("kind".into(), Json::String(kind.into()));
    Json::Object(fields)
}

fn type_expr_from_json(value: &Json, field: &str) -> Result<TypeExpr, CodecError> {
    let fields = object(value, field)?;
    let kind = string(required(fields, "kind")?, &qualified(field, "kind"))?;
    let allowed: &[&str] = match kind.as_str() {
        "unit" | "boolean" | "integer" | "number" | "string" | "bytes" | "json" | "entry" => {
            &["kind"]
        }
        "named" => &["kind", "name"],
        "record" => &["kind", "fields"],
        "list" => &["kind", "items"],
        "enum" | "union" => &["kind", "cases"],
        _ => return Err(invalid(&qualified(field, "kind"), "unknown type kind")),
    };
    exact_object(value, field, allowed)?;
    Ok(match kind.as_str() {
        "unit" => TypeExpr::Unit,
        "boolean" => TypeExpr::Boolean,
        "integer" => TypeExpr::Integer,
        "number" => TypeExpr::Number,
        "string" => TypeExpr::String,
        "bytes" => TypeExpr::Bytes,
        "json" => TypeExpr::Json,
        "entry" => TypeExpr::Entry,
        "named" => TypeExpr::Named {
            name: string(required(fields, "name")?, &qualified(field, "name"))?,
        },
        "record" => TypeExpr::Record {
            fields: array(required(fields, "fields")?, &qualified(field, "fields"))?
                .iter()
                .enumerate()
                .map(|(index, value)| field_from_json(value, &format!("{field}.fields[{index}]")))
                .collect::<Result<Vec<_>, _>>()?,
        },
        "list" => TypeExpr::List {
            items: Box::new(type_expr_from_json(
                required(fields, "items")?,
                &qualified(field, "items"),
            )?),
        },
        "enum" => TypeExpr::Enum {
            cases: array(required(fields, "cases")?, &qualified(field, "cases"))?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    enum_case_from_json(value, &format!("{field}.cases[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        },
        "union" => TypeExpr::Union {
            cases: array(required(fields, "cases")?, &qualified(field, "cases"))?
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    union_case_from_json(value, &format!("{field}.cases[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        },
        _ => unreachable!("kind checked above"),
    })
}

fn field_to_json(field: &FieldDeclaration) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(field.name.clone()));
    fields.insert("required".into(), Json::Bool(field.required));
    if let Some(documentation) = &field.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    fields.insert("type".into(), type_expr_to_json(&field.r#type));
    Json::Object(fields)
}

fn field_from_json(value: &Json, field: &str) -> Result<FieldDeclaration, CodecError> {
    let fields = exact_object(value, field, &["name", "required", "documentation", "type"])?;
    Ok(FieldDeclaration {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        required: boolean(required(fields, "required")?, &qualified(field, "required"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        r#type: type_expr_from_json(required(fields, "type")?, &qualified(field, "type"))?,
    })
}

fn enum_case_to_json(case: &EnumCase) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(case.name.clone()));
    if let Some(documentation) = &case.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    Json::Object(fields)
}

fn enum_case_from_json(value: &Json, field: &str) -> Result<EnumCase, CodecError> {
    let fields = exact_object(value, field, &["name", "documentation"])?;
    Ok(EnumCase {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
    })
}

fn union_case_to_json(case: &UnionCase) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(case.name.clone()));
    if let Some(documentation) = &case.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    if let Some(payload) = &case.payload {
        fields.insert("payload".into(), type_expr_to_json(payload));
    }
    Json::Object(fields)
}

fn union_case_from_json(value: &Json, field: &str) -> Result<UnionCase, CodecError> {
    let fields = exact_object(value, field, &["name", "documentation", "payload"])?;
    Ok(UnionCase {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        payload: optional(fields, "payload")?
            .map(|value| type_expr_from_json(value, &qualified(field, "payload")))
            .transpose()?,
    })
}

fn declaration_to_json(declaration: &TypeDeclaration) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(declaration.name.clone()));
    if let Some(documentation) = &declaration.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    fields.insert(
        "definition".into(),
        type_expr_to_json(&declaration.definition),
    );
    Json::Object(fields)
}

fn declaration_from_json(value: &Json, field: &str) -> Result<TypeDeclaration, CodecError> {
    let fields = exact_object(value, field, &["name", "documentation", "definition"])?;
    Ok(TypeDeclaration {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        definition: type_expr_from_json(
            required(fields, "definition")?,
            &qualified(field, "definition"),
        )?,
    })
}

fn parameter_to_json(parameter: &ParameterDeclaration) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(parameter.name.clone()));
    fields.insert("required".into(), Json::Bool(parameter.required));
    if let Some(documentation) = &parameter.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    fields.insert("type".into(), type_expr_to_json(&parameter.r#type));
    Json::Object(fields)
}

fn parameter_from_json(value: &Json, field: &str) -> Result<ParameterDeclaration, CodecError> {
    let fields = exact_object(value, field, &["name", "required", "documentation", "type"])?;
    Ok(ParameterDeclaration {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        required: boolean(required(fields, "required")?, &qualified(field, "required"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        r#type: type_expr_from_json(required(fields, "type")?, &qualified(field, "type"))?,
    })
}

fn operation_to_json(operation: &OperationDeclaration) -> Json {
    let mut fields = Map::new();
    fields.insert("name".into(), Json::String(operation.name.clone()));
    if let Some(documentation) = &operation.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    fields.insert(
        "parameters".into(),
        Json::Array(operation.parameters.iter().map(parameter_to_json).collect()),
    );
    let mut returns = Map::new();
    if let Some(documentation) = &operation.returns.documentation {
        returns.insert("documentation".into(), documentation_to_json(documentation));
    }
    returns.insert("type".into(), type_expr_to_json(&operation.returns.r#type));
    fields.insert("returns".into(), Json::Object(returns));
    Json::Object(fields)
}

fn operation_from_json(value: &Json, field: &str) -> Result<OperationDeclaration, CodecError> {
    let fields = exact_object(
        value,
        field,
        &["name", "documentation", "parameters", "returns"],
    )?;
    let returns_field = qualified(field, "returns");
    let returns = exact_object(
        required(fields, "returns")?,
        &returns_field,
        &["documentation", "type"],
    )?;
    Ok(OperationDeclaration {
        name: string(required(fields, "name")?, &qualified(field, "name"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        parameters: array(
            required(fields, "parameters")?,
            &qualified(field, "parameters"),
        )?
        .iter()
        .enumerate()
        .map(|(index, value)| parameter_from_json(value, &format!("{field}.parameters[{index}]")))
        .collect::<Result<Vec<_>, _>>()?,
        returns: ReturnDeclaration {
            documentation: optional(returns, "documentation")?
                .map(|value| {
                    documentation_from_json(value, &qualified(&returns_field, "documentation"))
                })
                .transpose()?,
            r#type: type_expr_from_json(
                required(returns, "type")?,
                &qualified(&returns_field, "type"),
            )?,
        },
    })
}

pub(crate) fn descriptor_to_json(descriptor: &InterfaceDescriptor) -> Json {
    let mut fields = Map::new();
    fields.insert("format".into(), Json::String(descriptor.format.clone()));
    if let Some(documentation) = &descriptor.documentation {
        fields.insert("documentation".into(), documentation_to_json(documentation));
    }
    fields.insert(
        "types".into(),
        Json::Array(descriptor.types.iter().map(declaration_to_json).collect()),
    );
    fields.insert(
        "operations".into(),
        Json::Array(
            descriptor
                .operations
                .iter()
                .map(operation_to_json)
                .collect(),
        ),
    );
    Json::Object(fields)
}

pub(crate) fn descriptor_from_json(
    value: &Json,
    field: &str,
) -> Result<InterfaceDescriptor, CodecError> {
    let fields = exact_object(
        value,
        field,
        &["format", "documentation", "types", "operations"],
    )?;
    let descriptor = InterfaceDescriptor {
        format: string(required(fields, "format")?, &qualified(field, "format"))?,
        documentation: optional(fields, "documentation")?
            .map(|value| documentation_from_json(value, &qualified(field, "documentation")))
            .transpose()?,
        types: array(required(fields, "types")?, &qualified(field, "types"))?
            .iter()
            .enumerate()
            .map(|(index, value)| declaration_from_json(value, &format!("{field}.types[{index}]")))
            .collect::<Result<Vec<_>, _>>()?,
        operations: array(
            required(fields, "operations")?,
            &qualified(field, "operations"),
        )?
        .iter()
        .enumerate()
        .map(|(index, value)| operation_from_json(value, &format!("{field}.operations[{index}]")))
        .collect::<Result<Vec<_>, _>>()?,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

pub(crate) fn fetch_interface_response_to_json(response: &FetchInterfaceResponse) -> Json {
    let mut fields = Map::new();
    fields.insert(
        "interface".into(),
        interface_reference_to_json(&response.interface),
    );
    insert_optional_string(&mut fields, "scope_ref", &response.scope_ref);
    fields.insert(
        "descriptor".into(),
        descriptor_to_json(&response.descriptor),
    );
    insert_optional_bytes(&mut fields, "validator", &response.validator);
    Json::Object(fields)
}

pub(crate) fn fetch_interface_response_from_json(
    value: &Json,
) -> Result<FetchInterfaceResponse, CodecError> {
    let fields = exact_object(
        value,
        "response",
        &["interface", "scope_ref", "descriptor", "validator"],
    )?;
    let interface = interface_reference_from_json(required(fields, "interface")?, "interface")?;
    let scope_ref = optional(fields, "scope_ref")?
        .map(|value| string(value, "scope_ref"))
        .transpose()?;
    let validator = optional(fields, "validator")?
        .map(|value| base64_decode(value, "validator"))
        .transpose()?;
    let descriptor_value = required(fields, "descriptor")?;
    let descriptor_fields = object(descriptor_value, "descriptor")?;
    let format = string(required(descriptor_fields, "format")?, "descriptor.format")?;
    let descriptor = if format == INTERFACE_FORMAT_V1 {
        descriptor_from_json(descriptor_value, "descriptor")?
    } else {
        // Preserve only the format discriminator. The client classifies this
        // response after validating common response fields and request
        // correspondence, without interpreting an unknown descriptor schema.
        InterfaceDescriptor {
            format,
            documentation: None,
            types: Vec::new(),
            operations: Vec::new(),
        }
    };
    Ok(FetchInterfaceResponse {
        interface,
        scope_ref,
        descriptor,
        validator,
    })
}

fn declaration<'a>(descriptor: &'a InterfaceDescriptor, name: &str) -> Option<&'a TypeExpr> {
    descriptor
        .types
        .iter()
        .find(|declaration| declaration.name == name)
        .map(|declaration| &declaration.definition)
}

fn type_accepts_json_null(r#type: &TypeExpr, descriptor: &InterfaceDescriptor) -> bool {
    match r#type {
        TypeExpr::Unit | TypeExpr::Json => true,
        TypeExpr::Named { name } => declaration(descriptor, name)
            .is_some_and(|definition| type_accepts_json_null(definition, descriptor)),
        _ => false,
    }
}

pub(crate) fn protocol_value_to_json(
    value: &Value,
    r#type: &TypeExpr,
    descriptor: &InterfaceDescriptor,
    field: &str,
) -> Result<Json, CodecError> {
    descriptor.validate_value(r#type, value)?;
    value_to_json_unchecked(value, r#type, descriptor, field)
}

fn value_to_json_unchecked(
    value: &Value,
    r#type: &TypeExpr,
    descriptor: &InterfaceDescriptor,
    field: &str,
) -> Result<Json, CodecError> {
    if let TypeExpr::Named { name } = r#type {
        let definition =
            declaration(descriptor, name).ok_or_else(|| invalid(field, "unknown named type"))?;
        return value_to_json_unchecked(value, definition, descriptor, field);
    }
    Ok(match (r#type, value) {
        (TypeExpr::Unit, Value::Unit) => Json::Null,
        (TypeExpr::Boolean, Value::Boolean(value)) => Json::Bool(*value),
        (TypeExpr::Integer, Value::Integer(value)) => Json::Number(Number::from(*value)),
        (TypeExpr::Number, Value::Number(value)) => Json::Number(
            Number::from_f64(*value).ok_or_else(|| invalid(field, "non-finite number"))?,
        ),
        (TypeExpr::String | TypeExpr::Entry | TypeExpr::Enum { .. }, Value::String(value)) => {
            Json::String(value.clone())
        }
        (TypeExpr::Bytes, Value::Bytes(value)) => base64_encode(value),
        (TypeExpr::Json, value) => json_value_to_json(value, field)?,
        (TypeExpr::List { items }, Value::List(values)) => Json::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    value_to_json_unchecked(value, items, descriptor, &format!("{field}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        (TypeExpr::Record { fields }, Value::Record(values)) => {
            let mut object = Map::new();
            for declaration in fields {
                if let Some(value) = values.get(&declaration.name) {
                    object.insert(
                        declaration.name.clone(),
                        value_to_json_unchecked(
                            value,
                            &declaration.r#type,
                            descriptor,
                            &qualified(field, &declaration.name),
                        )?,
                    );
                }
            }
            Json::Object(object)
        }
        (TypeExpr::Union { cases }, Value::Record(values)) => {
            let Value::String(case_name) = &values["$case"] else {
                return Err(invalid(field, "invalid union discriminator"));
            };
            let case = cases
                .iter()
                .find(|case| case.name == *case_name)
                .ok_or_else(|| invalid(field, "unknown union case"))?;
            let mut object = Map::new();
            object.insert("$case".into(), Json::String(case_name.clone()));
            if let Some(payload_type) = &case.payload {
                object.insert(
                    "value".into(),
                    value_to_json_unchecked(
                        &values["value"],
                        payload_type,
                        descriptor,
                        &qualified(field, "value"),
                    )?,
                );
            }
            Json::Object(object)
        }
        _ => return Err(invalid(field, "value does not match descriptor")),
    })
}

fn json_value_to_json(value: &Value, field: &str) -> Result<Json, CodecError> {
    Ok(match value {
        Value::Unit => Json::Null,
        Value::Boolean(value) => Json::Bool(*value),
        Value::Integer(value) if (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(value) => {
            Json::Number(Number::from(*value))
        }
        Value::Integer(_) => return Err(invalid(field, "integer outside safe range")),
        Value::Number(value) => Json::Number(
            Number::from_f64(*value).ok_or_else(|| invalid(field, "non-finite number"))?,
        ),
        Value::String(value) => Json::String(value.clone()),
        Value::Bytes(_) => return Err(invalid(field, "bytes are not valid under Json")),
        Value::Record(values) => Json::Object(
            values
                .iter()
                .map(|(name, value)| {
                    Ok((
                        name.clone(),
                        json_value_to_json(value, &qualified(field, name))?,
                    ))
                })
                .collect::<Result<Map<_, _>, CodecError>>()?,
        ),
        Value::List(values) => Json::Array(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| json_value_to_json(value, &format!("{field}[{index}]")))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    })
}

pub(crate) fn protocol_value_from_json(
    value: &Json,
    r#type: &TypeExpr,
    descriptor: &InterfaceDescriptor,
    field: &str,
) -> Result<Value, CodecError> {
    let value = value_from_json_unchecked(value, r#type, descriptor, field)?;
    descriptor.validate_value(r#type, &value)?;
    Ok(value)
}

fn value_from_json_unchecked(
    value: &Json,
    r#type: &TypeExpr,
    descriptor: &InterfaceDescriptor,
    field: &str,
) -> Result<Value, CodecError> {
    if let TypeExpr::Named { name } = r#type {
        let definition =
            declaration(descriptor, name).ok_or_else(|| invalid(field, "unknown named type"))?;
        return value_from_json_unchecked(value, definition, descriptor, field);
    }
    match r#type {
        TypeExpr::Unit => match value {
            Json::Null => Ok(Value::Unit),
            _ => Err(invalid(field, "expected null for Unit")),
        },
        TypeExpr::Boolean => Ok(Value::Boolean(boolean(value, field)?)),
        TypeExpr::Integer => {
            let integer = value
                .as_i64()
                .filter(|value| (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(value))
                .ok_or_else(|| invalid(field, "expected safe integer"))?;
            Ok(Value::Integer(integer))
        }
        TypeExpr::Number => {
            let number = value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| invalid(field, "expected finite number"))?;
            Ok(Value::Number(number))
        }
        TypeExpr::String => Ok(Value::String(string(value, field)?)),
        TypeExpr::Bytes => Ok(Value::Bytes(base64_decode(value, field)?)),
        TypeExpr::Entry => {
            let path = string(value, field)?;
            wip_protocol::validate_path(&path)?;
            Ok(Value::String(path))
        }
        TypeExpr::Json => json_value_from_json(value, field),
        TypeExpr::Enum { cases } => {
            let case = string(value, field)?;
            if !cases.iter().any(|declared| declared.name == case) {
                return Err(invalid(field, "unknown enum case"));
            }
            Ok(Value::String(case))
        }
        TypeExpr::List { items } => Ok(Value::List(
            array(value, field)?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    value_from_json_unchecked(item, items, descriptor, &format!("{field}[{index}]"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        TypeExpr::Record { fields } => {
            let object = object(value, field)?;
            let declared: BTreeSet<&str> = fields.iter().map(|field| field.name.as_str()).collect();
            for name in object.keys() {
                if !declared.contains(name.as_str()) {
                    return Err(CodecError::UnknownField {
                        field: qualified(field, name),
                    });
                }
            }
            let mut record = BTreeMap::new();
            for declaration in fields {
                match object.get(&declaration.name) {
                    Some(Json::Null)
                        if !type_accepts_json_null(&declaration.r#type, descriptor) =>
                    {
                        return Err(CodecError::NullOptionalField {
                            field: qualified(field, &declaration.name),
                        });
                    }
                    Some(value) => {
                        record.insert(
                            declaration.name.clone(),
                            value_from_json_unchecked(
                                value,
                                &declaration.r#type,
                                descriptor,
                                &qualified(field, &declaration.name),
                            )?,
                        );
                    }
                    None if declaration.required => {
                        return Err(CodecError::MissingField {
                            field: qualified(field, &declaration.name),
                        });
                    }
                    None => {}
                }
            }
            Ok(Value::Record(record))
        }
        TypeExpr::Union { cases } => {
            let object = exact_object(value, field, &["$case", "value"])?;
            let case_name = string(required(object, "$case")?, &qualified(field, "$case"))?;
            let case = cases
                .iter()
                .find(|case| case.name == case_name)
                .ok_or_else(|| invalid(field, "unknown union case"))?;
            let mut record = BTreeMap::new();
            record.insert("$case".into(), Value::String(case_name));
            match (&case.payload, object.get("value")) {
                (Some(_), None) => {
                    return Err(CodecError::MissingField {
                        field: qualified(field, "value"),
                    });
                }
                (None, Some(_)) => return Err(invalid(field, "payload-free union has value")),
                (Some(payload), Some(value)) => {
                    record.insert(
                        "value".into(),
                        value_from_json_unchecked(
                            value,
                            payload,
                            descriptor,
                            &qualified(field, "value"),
                        )?,
                    );
                }
                (None, None) => {}
            }
            Ok(Value::Record(record))
        }
        TypeExpr::Named { .. } => unreachable!("named handled above"),
    }
}

fn json_value_from_json(value: &Json, field: &str) -> Result<Value, CodecError> {
    match value {
        Json::Null => Ok(Value::Unit),
        Json::Bool(value) => Ok(Value::Boolean(*value)),
        Json::Number(value) => {
            if let Some(integer) = value.as_i64()
                && (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&integer)
            {
                return Ok(Value::Integer(integer));
            }
            let number = value
                .as_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| invalid(field, "number is not finite or interoperable"))?;
            Ok(Value::Number(number))
        }
        Json::String(value) => Ok(Value::String(value.clone())),
        Json::Array(values) => Ok(Value::List(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| json_value_from_json(value, &format!("{field}[{index}]")))
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Json::Object(values) => Ok(Value::Record(
            values
                .iter()
                .map(|(name, value)| {
                    Ok((
                        name.clone(),
                        json_value_from_json(value, &qualified(field, name))?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, CodecError>>()?,
        )),
    }
}

fn target_to_json(target: &Target) -> Json {
    let mut fields = Map::new();
    fields.insert("path".into(), Json::String(target.path.clone()));
    insert_optional_bytes(&mut fields, "validator", &target.validator);
    Json::Object(fields)
}

fn target_from_json(value: &Json, field: &str) -> Result<Target, CodecError> {
    let fields = exact_object(value, field, &["path", "validator"])?;
    Ok(Target {
        path: string(required(fields, "path")?, &qualified(field, "path"))?,
        validator: optional(fields, "validator")?
            .map(|value| base64_decode(value, &qualified(field, "validator")))
            .transpose()?,
    })
}

fn interface_target_to_json(target: &InterfaceTarget) -> Json {
    let mut fields = Map::new();
    fields.insert(
        "reference".into(),
        interface_reference_to_json(&target.reference),
    );
    insert_optional_string(&mut fields, "scope_ref", &target.scope_ref);
    insert_optional_bytes(&mut fields, "validator", &target.validator);
    Json::Object(fields)
}

fn interface_target_from_json(value: &Json, field: &str) -> Result<InterfaceTarget, CodecError> {
    let fields = exact_object(value, field, &["reference", "scope_ref", "validator"])?;
    Ok(InterfaceTarget {
        reference: interface_reference_from_json(
            required(fields, "reference")?,
            &qualified(field, "reference"),
        )?,
        scope_ref: optional(fields, "scope_ref")?
            .map(|value| string(value, &qualified(field, "scope_ref")))
            .transpose()?,
        validator: optional(fields, "validator")?
            .map(|value| base64_decode(value, &qualified(field, "validator")))
            .transpose()?,
    })
}

pub(crate) fn call_request_metadata_from_json(
    value: &Json,
) -> Result<(Target, InterfaceTarget, String), CodecError> {
    let fields = exact_object(
        value,
        "request",
        &["target", "interface", "operation", "arguments"],
    )?;
    object(required(fields, "arguments")?, "arguments")?;
    let target = target_from_json(required(fields, "target")?, "target")?;
    wip_protocol::validate_path(&target.path)?;
    Ok((
        target,
        interface_target_from_json(required(fields, "interface")?, "interface")?,
        string(required(fields, "operation")?, "operation")?,
    ))
}

pub(crate) fn call_request_to_json(
    request: &CallOperationRequest,
    descriptor: &InterfaceDescriptor,
) -> Result<Json, CodecError> {
    request.validate_with(descriptor)?;
    let operation = descriptor
        .operations
        .iter()
        .find(|operation| operation.name == request.operation)
        .expect("validated operation exists");
    let mut arguments = Map::new();
    for parameter in &operation.parameters {
        if let Some(value) = request.arguments.get(&parameter.name) {
            arguments.insert(
                parameter.name.clone(),
                protocol_value_to_json(value, &parameter.r#type, descriptor, &parameter.name)?,
            );
        }
    }
    Ok(serde_json::json!({
        "target": target_to_json(&request.target),
        "interface": interface_target_to_json(&request.interface),
        "operation": request.operation,
        "arguments": arguments,
    }))
}

pub(crate) fn call_request_from_json(
    value: &Json,
    descriptor: &InterfaceDescriptor,
) -> Result<CallOperationRequest, CodecError> {
    descriptor.validate()?;
    let fields = exact_object(
        value,
        "request",
        &["target", "interface", "operation", "arguments"],
    )?;
    let operation_name = string(required(fields, "operation")?, "operation")?;
    let operation = descriptor
        .operations
        .iter()
        .find(|operation| operation.name == operation_name)
        .ok_or_else(|| invalid("operation", "operation is not declared"))?;
    let argument_fields = exact_object(
        required(fields, "arguments")?,
        "arguments",
        &operation
            .parameters
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<Vec<_>>(),
    )?;
    let mut arguments = BTreeMap::new();
    for parameter in &operation.parameters {
        match argument_fields.get(&parameter.name) {
            Some(Json::Null) if !type_accepts_json_null(&parameter.r#type, descriptor) => {
                return Err(CodecError::NullOptionalField {
                    field: qualified("arguments", &parameter.name),
                });
            }
            Some(value) => {
                arguments.insert(
                    parameter.name.clone(),
                    protocol_value_from_json(
                        value,
                        &parameter.r#type,
                        descriptor,
                        &qualified("arguments", &parameter.name),
                    )?,
                );
            }
            None if parameter.required => {
                return Err(CodecError::MissingField {
                    field: qualified("arguments", &parameter.name),
                });
            }
            None => {}
        }
    }
    let request = CallOperationRequest {
        target: target_from_json(required(fields, "target")?, "target")?,
        interface: interface_target_from_json(required(fields, "interface")?, "interface")?,
        operation: operation_name,
        arguments,
    };
    request.validate_with(descriptor)?;
    Ok(request)
}

pub(crate) fn call_response_to_json(
    response: &CallOperationResponse,
    descriptor: &InterfaceDescriptor,
    operation_name: &str,
) -> Result<Json, CodecError> {
    response.validate_for(descriptor, operation_name)?;
    let operation = descriptor
        .operations
        .iter()
        .find(|operation| operation.name == operation_name)
        .expect("validated operation exists");
    let mut fields = Map::new();
    fields.insert(
        "result".into(),
        protocol_value_to_json(
            &response.result,
            &operation.returns.r#type,
            descriptor,
            "result",
        )?,
    );
    insert_optional_bytes(&mut fields, "validator", &response.validator);
    Ok(Json::Object(fields))
}

pub(crate) fn call_response_from_json(
    value: &Json,
    descriptor: &InterfaceDescriptor,
    operation_name: &str,
) -> Result<CallOperationResponse, CodecError> {
    descriptor.validate()?;
    let operation = descriptor
        .operations
        .iter()
        .find(|operation| operation.name == operation_name)
        .ok_or_else(|| invalid("operation", "operation is not declared"))?;
    let fields = exact_object(value, "response", &["result", "validator"])?;
    let response = CallOperationResponse {
        result: protocol_value_from_json(
            required(fields, "result")?,
            &operation.returns.r#type,
            descriptor,
            "result",
        )?,
        validator: optional(fields, "validator")?
            .map(|value| base64_decode(value, "validator"))
            .transpose()?,
    };
    response.validate_for(descriptor, operation_name)?;
    Ok(response)
}
