use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{
    CallOperationRequest, FetchInterfaceRequest, FetchInterfaceResponse, InterfaceDescriptor,
    InterfaceReference, MAX_SAFE_INTEGER, MIN_SAFE_INTEGER, NameNamespace, Object,
    ObjectObservation, ObserveRequest, PathSegment, TypeDeclaration, TypeExpr, UNION_CASE_FIELD,
    UNION_VALUE_FIELD, ValidationError, ValidationErrorKind, Value, ValueKind,
};

pub(crate) fn validate_object(object: &Object) -> Result<(), ValidationError> {
    validate_object_name(&object.name, true)?;

    let mut seen = HashSet::new();
    for (index, interface) in object.interfaces.iter().enumerate() {
        interface.validate().map_err(|error| {
            error
                .prepend(PathSegment::Index(index))
                .prepend(PathSegment::Field("interfaces".into()))
        })?;
        if !seen.insert(interface) {
            return Err(
                ValidationError::new(ValidationErrorKind::DuplicateInterface {
                    reference: interface.clone(),
                })
                .prepend(PathSegment::Field("interfaces".into())),
            );
        }
    }
    Ok(())
}

fn validate_object_name(name: &str, allow_root: bool) -> Result<(), ValidationError> {
    let valid =
        (allow_root || !name.is_empty()) && !name.contains('/') && name != "." && name != "..";
    if valid {
        Ok(())
    } else {
        Err(ValidationError::new(
            ValidationErrorKind::InvalidObjectName {
                name: name.to_owned(),
            },
        ))
    }
}

pub(crate) fn validate_object_at_path(object: &Object, path: &str) -> Result<(), ValidationError> {
    validate_path(path)?;
    object
        .validate()
        .map_err(|error| error.prepend(PathSegment::Object(path.to_owned())))?;

    let expected = if path == "/" {
        ""
    } else {
        path.rsplit('/')
            .next()
            .expect("non-root path has a segment")
    };
    for (index, interface) in object.interfaces.iter().enumerate() {
        interface.validate_for_path(path).map_err(|error| {
            error
                .prepend(PathSegment::Index(index))
                .prepend(PathSegment::Field("interfaces".into()))
                .prepend(PathSegment::Object(path.to_owned()))
        })?;
    }
    if object.name == expected {
        Ok(())
    } else {
        Err(
            ValidationError::new(ValidationErrorKind::ObjectNameMismatch {
                expected: expected.to_owned(),
                actual: object.name.clone(),
            })
            .prepend(PathSegment::Object(path.to_owned())),
        )
    }
}

pub(crate) fn validate_object_consistency(
    left: &Object,
    right: &Object,
) -> Result<(), ValidationError> {
    left.validate()?;
    right.validate()?;
    if let (Some(left_ref), Some(right_ref)) = (&left.r#ref, &right.r#ref)
        && left_ref == right_ref
        && left.validator.is_some()
        && left.validator == right.validator
        && left != right
    {
        return Err(ValidationError::new(
            ValidationErrorKind::InconsistentObjectObservation {
                reference: left_ref.clone(),
            },
        ));
    }
    Ok(())
}

pub(crate) fn validate_path(path: &str) -> Result<(), ValidationError> {
    let valid = if path == "/" {
        true
    } else if let Some(remainder) = path.strip_prefix('/') {
        !remainder.is_empty()
            && remainder
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    } else {
        false
    };

    if valid {
        Ok(())
    } else {
        Err(ValidationError::new(ValidationErrorKind::InvalidPath {
            path: path.to_owned(),
        }))
    }
}

pub(crate) fn validate_interface_reference(
    reference: &InterfaceReference,
) -> Result<(), ValidationError> {
    validate_path(&reference.scope)
        .map_err(|error| error.prepend(PathSegment::Field("scope".into())))?;
    if reference.name.is_empty() {
        return Err(
            ValidationError::new(ValidationErrorKind::EmptyInterfaceName)
                .prepend(PathSegment::Field("name".into())),
        );
    }
    Ok(())
}

pub(crate) fn validate_interface_scope(
    reference: &InterfaceReference,
    path: &str,
) -> Result<(), ValidationError> {
    reference.validate()?;
    validate_path(path)?;
    let scope = reference.scope.as_str();
    if scope == "/"
        || path == scope
        || path
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
    {
        Ok(())
    } else {
        Err(ValidationError::new(
            ValidationErrorKind::InterfaceScopeMismatch {
                scope: reference.scope.clone(),
                path: path.to_owned(),
            },
        ))
    }
}

pub(crate) fn validate_call_shape(request: &CallOperationRequest) -> Result<(), ValidationError> {
    validate_path(&request.target.path)
        .map_err(|error| error.prepend(PathSegment::Field("target".into())))?;
    request.interface.reference.validate().map_err(|error| {
        error
            .prepend(PathSegment::Field("reference".into()))
            .prepend(PathSegment::Field("interface".into()))
    })
}

pub(crate) fn validate_descriptor(descriptor: &InterfaceDescriptor) -> Result<(), ValidationError> {
    let declarations = declaration_map(descriptor)?;

    for declaration in &descriptor.types {
        validate_type_expr(&declaration.definition, &declarations).map_err(|error| {
            error
                .prepend(PathSegment::Declaration(declaration.name.clone()))
                .prepend(PathSegment::Descriptor)
        })?;
    }

    let mut operation_names = HashSet::new();
    for operation in &descriptor.operations {
        if !operation_names.insert(operation.name.as_str()) {
            return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                namespace: NameNamespace::Operation,
                name: operation.name.clone(),
            })
            .prepend(PathSegment::Operation(operation.name.clone()))
            .prepend(PathSegment::Descriptor));
        }

        let mut parameter_names = HashSet::new();
        for parameter in &operation.parameters {
            if !parameter_names.insert(parameter.name.as_str()) {
                return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                    namespace: NameNamespace::Parameter,
                    name: parameter.name.clone(),
                })
                .prepend(PathSegment::Parameter(parameter.name.clone()))
                .prepend(PathSegment::Operation(operation.name.clone()))
                .prepend(PathSegment::Descriptor));
            }
            validate_type_expr(&parameter.r#type, &declarations).map_err(|error| {
                error
                    .prepend(PathSegment::Parameter(parameter.name.clone()))
                    .prepend(PathSegment::Operation(operation.name.clone()))
                    .prepend(PathSegment::Descriptor)
            })?;
        }
        validate_type_expr(&operation.returns.r#type, &declarations).map_err(|error| {
            error
                .prepend(PathSegment::Return)
                .prepend(PathSegment::Operation(operation.name.clone()))
                .prepend(PathSegment::Descriptor)
        })?;
    }

    validate_no_cycles(descriptor, &declarations)
}

fn declaration_map(
    descriptor: &InterfaceDescriptor,
) -> Result<HashMap<&str, &TypeDeclaration>, ValidationError> {
    let mut declarations = HashMap::new();
    for declaration in &descriptor.types {
        if declarations
            .insert(declaration.name.as_str(), declaration)
            .is_some()
        {
            return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                namespace: NameNamespace::TypeDeclaration,
                name: declaration.name.clone(),
            })
            .prepend(PathSegment::Declaration(declaration.name.clone()))
            .prepend(PathSegment::Descriptor));
        }
    }
    Ok(declarations)
}

fn validate_type_expr(
    r#type: &TypeExpr,
    declarations: &HashMap<&str, &TypeDeclaration>,
) -> Result<(), ValidationError> {
    match r#type {
        TypeExpr::Named { name } if !declarations.contains_key(name.as_str()) => {
            Err(ValidationError::new(ValidationErrorKind::UnknownType {
                name: name.clone(),
            }))
        }
        TypeExpr::Record { fields } => {
            let mut names = HashSet::new();
            for field in fields {
                if !names.insert(field.name.as_str()) {
                    return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                        namespace: NameNamespace::Field,
                        name: field.name.clone(),
                    })
                    .prepend(PathSegment::Field(field.name.clone())));
                }
                validate_type_expr(&field.r#type, declarations)
                    .map_err(|error| error.prepend(PathSegment::Field(field.name.clone())))?;
            }
            Ok(())
        }
        TypeExpr::List { items } => validate_type_expr(items, declarations),
        TypeExpr::Enum { cases } => validate_case_names(
            cases.iter().map(|case| case.name.as_str()),
            NameNamespace::EnumCase,
        ),
        TypeExpr::Union { cases } => {
            validate_case_names(
                cases.iter().map(|case| case.name.as_str()),
                NameNamespace::UnionCase,
            )?;
            for case in cases {
                if let Some(payload) = &case.payload {
                    validate_type_expr(payload, declarations)
                        .map_err(|error| error.prepend(PathSegment::Case(case.name.clone())))?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_case_names<'a>(
    names: impl Iterator<Item = &'a str>,
    namespace: NameNamespace,
) -> Result<(), ValidationError> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                namespace,
                name: name.to_owned(),
            })
            .prepend(PathSegment::Case(name.to_owned())));
        }
    }
    Ok(())
}

fn validate_no_cycles(
    descriptor: &InterfaceDescriptor,
    declarations: &HashMap<&str, &TypeDeclaration>,
) -> Result<(), ValidationError> {
    let mut state: HashMap<&str, VisitState> = HashMap::new();
    let mut stack = Vec::new();
    for declaration in &descriptor.types {
        visit_declaration(
            declaration.name.as_str(),
            declarations,
            &mut state,
            &mut stack,
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

fn visit_declaration<'a>(
    name: &'a str,
    declarations: &HashMap<&'a str, &'a TypeDeclaration>,
    state: &mut HashMap<&'a str, VisitState>,
    stack: &mut Vec<&'a str>,
) -> Result<(), ValidationError> {
    match state.get(name) {
        Some(VisitState::Visited) => return Ok(()),
        Some(VisitState::Visiting) => {
            let start = stack.iter().position(|entry| *entry == name).unwrap_or(0);
            let mut cycle: Vec<String> = stack[start..]
                .iter()
                .map(|entry| (*entry).to_owned())
                .collect();
            cycle.push(name.to_owned());
            return Err(
                ValidationError::new(ValidationErrorKind::RecursiveType { cycle })
                    .prepend(PathSegment::Declaration(name.to_owned()))
                    .prepend(PathSegment::Descriptor),
            );
        }
        None => {}
    }

    state.insert(name, VisitState::Visiting);
    stack.push(name);
    let mut references = Vec::new();
    collect_type_references(&declarations[name].definition, &mut references);
    for reference in references {
        visit_declaration(reference, declarations, state, stack)?;
    }
    stack.pop();
    state.insert(name, VisitState::Visited);
    Ok(())
}

fn collect_type_references<'a>(r#type: &'a TypeExpr, references: &mut Vec<&'a str>) {
    match r#type {
        TypeExpr::Named { name } => references.push(name),
        TypeExpr::Record { fields } => {
            for field in fields {
                collect_type_references(&field.r#type, references);
            }
        }
        TypeExpr::List { items } => collect_type_references(items, references),
        TypeExpr::Union { cases } => {
            for case in cases {
                if let Some(payload) = &case.payload {
                    collect_type_references(payload, references);
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn validate_root_value(
    descriptor: &InterfaceDescriptor,
    r#type: &TypeExpr,
    value: &Value,
) -> Result<(), ValidationError> {
    descriptor.validate()?;
    let declarations = declaration_map(descriptor)?;
    validate_type_expr(r#type, &declarations)
        .map_err(|error| error.prepend(PathSegment::Descriptor))?;
    validate_value(r#type, value, &declarations)
        .map_err(|error| error.prepend(PathSegment::Descriptor))
}

fn validate_value(
    r#type: &TypeExpr,
    value: &Value,
    declarations: &HashMap<&str, &TypeDeclaration>,
) -> Result<(), ValidationError> {
    match r#type {
        TypeExpr::Unit => expect_kind(value, ValueKind::Unit),
        TypeExpr::Boolean => expect_kind(value, ValueKind::Boolean),
        TypeExpr::Integer => validate_integer(value),
        TypeExpr::Number => validate_number(value),
        TypeExpr::String => expect_kind(value, ValueKind::String),
        TypeExpr::Bytes => expect_kind(value, ValueKind::Bytes),
        TypeExpr::Json => validate_json(value),
        TypeExpr::Entry => match value {
            Value::String(path) => validate_path(path),
            _ => expect_kind(value, ValueKind::String),
        },
        TypeExpr::Named { name } => {
            let Some(declaration) = declarations.get(name.as_str()) else {
                return Err(ValidationError::new(ValidationErrorKind::UnknownType {
                    name: name.clone(),
                }));
            };
            validate_value(&declaration.definition, value, declarations)
        }
        TypeExpr::Record { fields } => validate_record(fields, value, declarations),
        TypeExpr::List { items } => match value {
            Value::List(values) => {
                for (index, item) in values.iter().enumerate() {
                    validate_value(items, item, declarations)
                        .map_err(|error| error.prepend(PathSegment::Index(index)))?;
                }
                Ok(())
            }
            _ => expect_kind(value, ValueKind::List),
        },
        TypeExpr::Enum { cases } => match value {
            Value::String(case) if cases.iter().any(|declared| declared.name == *case) => Ok(()),
            Value::String(case) => {
                Err(
                    ValidationError::new(ValidationErrorKind::UnknownCase { name: case.clone() })
                        .prepend(PathSegment::Case(case.clone())),
                )
            }
            _ => expect_kind(value, ValueKind::String),
        },
        TypeExpr::Union { cases } => validate_union(cases, value, declarations),
    }
}

fn validate_integer(value: &Value) -> Result<(), ValidationError> {
    match value {
        Value::Integer(integer) if (MIN_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(integer) => {
            Ok(())
        }
        Value::Integer(integer) => Err(ValidationError::new(
            ValidationErrorKind::IntegerOutOfRange { value: *integer },
        )),
        _ => expect_kind(value, ValueKind::Integer),
    }
}

fn validate_number(value: &Value) -> Result<(), ValidationError> {
    match value {
        Value::Number(number) if number.is_finite() => Ok(()),
        Value::Number(_) => Err(ValidationError::new(ValidationErrorKind::NonFiniteNumber)),
        _ => expect_kind(value, ValueKind::Number),
    }
}

fn validate_record(
    fields: &[crate::FieldDeclaration],
    value: &Value,
    declarations: &HashMap<&str, &TypeDeclaration>,
) -> Result<(), ValidationError> {
    let Value::Record(values) = value else {
        return expect_kind(value, ValueKind::Record);
    };

    for (name, field_value) in values {
        let Some(field) = fields.iter().find(|field| field.name == *name) else {
            return Err(ValidationError::new(ValidationErrorKind::UnknownField {
                name: name.clone(),
            })
            .prepend(PathSegment::Field(name.clone())));
        };
        validate_value(&field.r#type, field_value, declarations)
            .map_err(|error| error.prepend(PathSegment::Field(name.clone())))?;
    }

    for field in fields {
        if field.required && !values.contains_key(&field.name) {
            return Err(ValidationError::new(ValidationErrorKind::MissingField {
                name: field.name.clone(),
            })
            .prepend(PathSegment::Field(field.name.clone())));
        }
    }
    Ok(())
}

fn validate_union(
    cases: &[crate::UnionCase],
    value: &Value,
    declarations: &HashMap<&str, &TypeDeclaration>,
) -> Result<(), ValidationError> {
    let Value::Record(fields) = value else {
        return expect_kind(value, ValueKind::Record);
    };

    for name in fields.keys() {
        if name != UNION_CASE_FIELD && name != UNION_VALUE_FIELD {
            return Err(ValidationError::new(ValidationErrorKind::UnknownField {
                name: name.clone(),
            })
            .prepend(PathSegment::Field(name.clone())));
        }
    }

    let Some(discriminator) = fields.get(UNION_CASE_FIELD) else {
        return Err(ValidationError::new(ValidationErrorKind::MissingField {
            name: UNION_CASE_FIELD.into(),
        })
        .prepend(PathSegment::Field(UNION_CASE_FIELD.into())));
    };
    let Value::String(case_name) = discriminator else {
        return expect_kind(discriminator, ValueKind::String)
            .map_err(|error| error.prepend(PathSegment::Field(UNION_CASE_FIELD.into())));
    };
    let Some(case) = cases.iter().find(|case| case.name == *case_name) else {
        return Err(ValidationError::new(ValidationErrorKind::UnknownCase {
            name: case_name.clone(),
        })
        .prepend(PathSegment::Case(case_name.clone())));
    };

    match (&case.payload, fields.get(UNION_VALUE_FIELD)) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(
            ValidationError::new(ValidationErrorKind::UnexpectedUnionPayload)
                .prepend(PathSegment::Field(UNION_VALUE_FIELD.into())),
        ),
        (Some(_), None) => Err(
            ValidationError::new(ValidationErrorKind::MissingUnionPayload)
                .prepend(PathSegment::Field(UNION_VALUE_FIELD.into())),
        ),
        (Some(payload), Some(payload_value)) => {
            validate_value(payload, payload_value, declarations)
                .map_err(|error| error.prepend(PathSegment::Field(UNION_VALUE_FIELD.into())))
        }
    }
}

fn expect_kind(value: &Value, expected: ValueKind) -> Result<(), ValidationError> {
    if value.kind() == expected {
        Ok(())
    } else {
        Err(ValidationError::new(ValidationErrorKind::TypeMismatch {
            expected,
            actual: value.kind(),
        }))
    }
}

fn validate_json(value: &Value) -> Result<(), ValidationError> {
    match value {
        Value::Integer(_) => validate_integer(value),
        Value::Number(_) => validate_number(value),
        Value::Bytes(_) => Err(ValidationError::new(ValidationErrorKind::BytesInJson)),
        Value::Record(fields) => {
            for (name, field) in fields {
                validate_json(field)
                    .map_err(|error| error.prepend(PathSegment::Field(name.clone())))?;
            }
            Ok(())
        }
        Value::List(items) => {
            for (index, item) in items.iter().enumerate() {
                validate_json(item).map_err(|error| error.prepend(PathSegment::Index(index)))?;
            }
            Ok(())
        }
        Value::Unit | Value::Boolean(_) | Value::String(_) => Ok(()),
    }
}

pub(crate) fn validate_arguments(
    descriptor: &InterfaceDescriptor,
    operation_name: &str,
    arguments: &BTreeMap<String, Value>,
) -> Result<(), ValidationError> {
    descriptor.validate()?;
    let operation = find_operation(descriptor, operation_name)?;
    let declarations = declaration_map(descriptor)?;

    for (name, value) in arguments {
        let Some(parameter) = operation
            .parameters
            .iter()
            .find(|parameter| parameter.name == *name)
        else {
            return Err(ValidationError::new(ValidationErrorKind::UnknownParameter {
                name: name.clone(),
            })
            .prepend(PathSegment::Parameter(name.clone()))
            .prepend(PathSegment::Operation(operation.name.clone()))
            .prepend(PathSegment::Descriptor));
        };
        validate_value(&parameter.r#type, value, &declarations).map_err(|error| {
            error
                .prepend(PathSegment::Parameter(name.clone()))
                .prepend(PathSegment::Operation(operation.name.clone()))
                .prepend(PathSegment::Descriptor)
        })?;
    }

    for parameter in &operation.parameters {
        if parameter.required && !arguments.contains_key(&parameter.name) {
            return Err(ValidationError::new(ValidationErrorKind::MissingParameter {
                name: parameter.name.clone(),
            })
            .prepend(PathSegment::Parameter(parameter.name.clone()))
            .prepend(PathSegment::Operation(operation.name.clone()))
            .prepend(PathSegment::Descriptor));
        }
    }
    Ok(())
}

pub(crate) fn validate_result(
    descriptor: &InterfaceDescriptor,
    operation_name: &str,
    result: &Value,
) -> Result<(), ValidationError> {
    descriptor.validate()?;
    let operation = find_operation(descriptor, operation_name)?;
    let declarations = declaration_map(descriptor)?;
    validate_value(&operation.returns.r#type, result, &declarations).map_err(|error| {
        error
            .prepend(PathSegment::Return)
            .prepend(PathSegment::Operation(operation.name.clone()))
            .prepend(PathSegment::Descriptor)
    })
}

pub(crate) fn validate_call(
    descriptor: &InterfaceDescriptor,
    request: &CallOperationRequest,
) -> Result<(), ValidationError> {
    request.validate()?;
    validate_arguments(descriptor, &request.operation, &request.arguments)
}

fn find_operation<'a>(
    descriptor: &'a InterfaceDescriptor,
    operation_name: &str,
) -> Result<&'a crate::OperationDeclaration, ValidationError> {
    descriptor
        .operations
        .iter()
        .find(|operation| operation.name == operation_name)
        .ok_or_else(|| {
            ValidationError::new(ValidationErrorKind::UnknownOperation {
                name: operation_name.to_owned(),
            })
            .prepend(PathSegment::Operation(operation_name.to_owned()))
            .prepend(PathSegment::Descriptor)
        })
}

pub(crate) fn validate_observe_response(
    response: &ObjectObservation,
    request: &ObserveRequest,
) -> Result<(), ValidationError> {
    request.validate()?;
    validate_observation_node(response, &request.path, 0, request.depth)
}

fn validate_observation_node(
    node: &ObjectObservation,
    path: &str,
    depth: u32,
    maximum_depth: u32,
) -> Result<(), ValidationError> {
    node.object.validate_at_path(path)?;

    if depth == maximum_depth {
        if node.children.is_some() {
            return Err(
                ValidationError::new(ValidationErrorKind::ChildrenAtDepthBoundary { depth })
                    .prepend(PathSegment::Object(path.to_owned())),
            );
        }
        return Ok(());
    }

    let children = node.children.as_ref().ok_or_else(|| {
        ValidationError::new(ValidationErrorKind::ChildrenMissingBeforeDepth { depth })
            .prepend(PathSegment::Object(path.to_owned()))
    })?;
    let mut names = HashSet::new();
    for child in children {
        validate_object_name(&child.object.name, false)
            .map_err(|error| error.prepend(PathSegment::Child(child.object.name.clone())))?;
        if !names.insert(child.object.name.as_str()) {
            return Err(ValidationError::new(ValidationErrorKind::DuplicateName {
                namespace: NameNamespace::ChildObject,
                name: child.object.name.clone(),
            })
            .prepend(PathSegment::Child(child.object.name.clone()))
            .prepend(PathSegment::Object(path.to_owned())));
        }
        let child_path = if path == "/" {
            format!("/{}", child.object.name)
        } else {
            format!("{path}/{}", child.object.name)
        };
        validate_observation_node(child, &child_path, depth + 1, maximum_depth)?;
    }
    Ok(())
}

pub(crate) fn validate_fetch_interface_response(
    response: &FetchInterfaceResponse,
    request: &FetchInterfaceRequest,
) -> Result<(), ValidationError> {
    request.validate()?;
    response.interface.validate()?;
    if response.interface != request.interface {
        return Err(
            ValidationError::new(ValidationErrorKind::InterfaceResponseMismatch {
                expected: Box::new(request.interface.clone()),
                actual: Box::new(response.interface.clone()),
            })
            .prepend(PathSegment::Field("interface".into())),
        );
    }
    response.descriptor.validate()
}
