use std::collections::BTreeMap;

use wip_protocol::{
    CallOperationRequest, CallOperationResponse, EnumCase, FetchInterfaceRequest,
    FetchInterfaceResponse, FieldDeclaration, INTERFACE_FORMAT_V1, InterfaceDescriptor,
    InterfaceReference, InterfaceTarget, MAX_SAFE_INTEGER, NameNamespace, Object,
    ObjectObservation, ObserveRequest, OperationDeclaration, ParameterDeclaration, PathSegment,
    ProtocolError, ProtocolErrorCode, ProtocolInteraction, ProtocolResult, ReturnDeclaration,
    Target, TypeDeclaration, TypeExpr, UnionCase, ValidationErrorKind, Value, ValueKind,
};

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

fn record(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Record(
        entries
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
    )
}

fn field(name: &str, r#type: TypeExpr, required: bool) -> FieldDeclaration {
    FieldDeclaration {
        name: name.into(),
        required,
        documentation: None,
        r#type,
    }
}

fn descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![
            TypeDeclaration {
                name: "Color".into(),
                documentation: None,
                definition: TypeExpr::Enum {
                    cases: vec![
                        EnumCase {
                            name: "red".into(),
                            documentation: None,
                        },
                        EnumCase {
                            name: "blue".into(),
                            documentation: None,
                        },
                    ],
                },
            },
            TypeDeclaration {
                name: "Selection".into(),
                documentation: None,
                definition: TypeExpr::Union {
                    cases: vec![
                        UnionCase {
                            name: "all".into(),
                            documentation: None,
                            payload: None,
                        },
                        UnionCase {
                            name: "one".into(),
                            documentation: None,
                            payload: Some(TypeExpr::Entry),
                        },
                    ],
                },
            },
            TypeDeclaration {
                name: "Payload".into(),
                documentation: None,
                definition: TypeExpr::Record {
                    fields: vec![
                        field("title", TypeExpr::String, true),
                        field(
                            "color",
                            TypeExpr::Named {
                                name: "Color".into(),
                            },
                            false,
                        ),
                        field("metadata", TypeExpr::Json, false),
                        field(
                            "entries",
                            TypeExpr::List {
                                items: Box::new(TypeExpr::Entry),
                            },
                            false,
                        ),
                    ],
                },
            },
        ],
        operations: vec![
            OperationDeclaration {
                name: "publish".into(),
                documentation: None,
                parameters: vec![
                    ParameterDeclaration {
                        name: "payload".into(),
                        required: true,
                        documentation: None,
                        r#type: TypeExpr::Named {
                            name: "Payload".into(),
                        },
                    },
                    ParameterDeclaration {
                        name: "selection".into(),
                        required: false,
                        documentation: None,
                        r#type: TypeExpr::Named {
                            name: "Selection".into(),
                        },
                    },
                ],
                returns: ReturnDeclaration {
                    documentation: None,
                    r#type: TypeExpr::Boolean,
                },
            },
            OperationDeclaration {
                name: "refresh".into(),
                documentation: None,
                parameters: vec![],
                returns: ReturnDeclaration {
                    documentation: None,
                    r#type: TypeExpr::Unit,
                },
            },
        ],
    }
}

fn object(name: &str) -> Object {
    Object {
        name: name.into(),
        description: Some(format!("Object {name}")),
        interfaces: vec![reference("/", "catalog-v1"), reference("/", "search-v2")],
        r#ref: Some("opaque-object-ref".into()),
        validator: Some(vec![1, 2, 3]),
    }
}

#[test]
fn object_fields_have_canonical_identity_and_ordered_set_semantics() {
    let object = object("article");
    object.validate().unwrap();
    object.validate_at_path("/articles/article").unwrap();
    assert_eq!(
        object.interfaces,
        [reference("/", "catalog-v1"), reference("/", "search-v2")]
    );

    let duplicate = Object {
        interfaces: vec![reference("/", "catalog-v1"), reference("/", "catalog-v1")],
        ..object.clone()
    };
    assert_eq!(
        duplicate.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateInterface {
            reference: reference("/", "catalog-v1")
        }
    );

    for invalid in ["a/b", ".", ".."] {
        assert!(matches!(
            Object {
                name: invalid.into(),
                ..object.clone()
            }
            .validate()
            .unwrap_err()
            .kind,
            ValidationErrorKind::InvalidObjectName { .. }
        ));
    }

    let root = Object {
        name: String::new(),
        ..object.clone()
    };
    root.validate_at_path("/").unwrap();
    assert!(matches!(
        object.validate_at_path("/articles/other").unwrap_err().kind,
        ValidationErrorKind::ObjectNameMismatch { .. }
    ));
}

#[test]
fn ref_and_validator_consistency_is_independent_from_path() {
    let first = object("article");
    let same = first.clone();
    first.validate_consistency_with(&same).unwrap();

    let changed_state = Object {
        description: Some("changed".into()),
        validator: Some(vec![4]),
        ..first.clone()
    };
    first.validate_consistency_with(&changed_state).unwrap();

    let inconsistent = Object {
        description: Some("different representation".into()),
        ..first.clone()
    };
    assert_eq!(
        first
            .validate_consistency_with(&inconsistent)
            .unwrap_err()
            .kind,
        ValidationErrorKind::InconsistentObjectObservation {
            reference: "opaque-object-ref".into()
        }
    );

    let validator_free = Object {
        validator: None,
        ..first.clone()
    };
    let validator_free_change = Object {
        description: Some("latest observation".into()),
        validator: None,
        ..first.clone()
    };
    validator_free
        .validate_consistency_with(&validator_free_change)
        .unwrap();

    let unrelated = Object {
        r#ref: Some("another-ref".into()),
        ..inconsistent
    };
    first.validate_consistency_with(&unrelated).unwrap();
}

#[test]
fn descriptor_supports_every_canonical_type_expression() {
    let descriptor = descriptor();
    descriptor.validate().unwrap();

    let all = [
        TypeExpr::Unit,
        TypeExpr::Boolean,
        TypeExpr::Integer,
        TypeExpr::Number,
        TypeExpr::String,
        TypeExpr::Bytes,
        TypeExpr::Json,
        TypeExpr::Entry,
        TypeExpr::Named {
            name: "Payload".into(),
        },
        TypeExpr::Record {
            fields: vec![field("ok", TypeExpr::Boolean, true)],
        },
        TypeExpr::List {
            items: Box::new(TypeExpr::String),
        },
        TypeExpr::Enum {
            cases: vec![EnumCase {
                name: "yes".into(),
                documentation: None,
            }],
        },
        TypeExpr::Union {
            cases: vec![UnionCase {
                name: "some".into(),
                documentation: None,
                payload: Some(TypeExpr::Integer),
            }],
        },
    ];
    assert_eq!(all.len(), 13);
}

#[test]
fn descriptor_rejects_duplicate_names_missing_references_and_cycles() {
    let mut duplicate = descriptor();
    duplicate.types.push(duplicate.types[0].clone());
    assert_eq!(
        duplicate.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::TypeDeclaration,
            name: "Color".into(),
        }
    );

    let mut missing = descriptor();
    missing.types[2].definition = TypeExpr::List {
        items: Box::new(TypeExpr::Named {
            name: "Absent".into(),
        }),
    };
    let error = missing.validate().unwrap_err();
    assert_eq!(
        error.kind,
        ValidationErrorKind::UnknownType {
            name: "Absent".into()
        }
    );
    assert_eq!(
        error.path,
        vec![
            PathSegment::Descriptor,
            PathSegment::Declaration("Payload".into())
        ]
    );

    let cyclic = InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![
            TypeDeclaration {
                name: "A".into(),
                documentation: None,
                definition: TypeExpr::Record {
                    fields: vec![field("b", TypeExpr::Named { name: "B".into() }, true)],
                },
            },
            TypeDeclaration {
                name: "B".into(),
                documentation: None,
                definition: TypeExpr::Union {
                    cases: vec![UnionCase {
                        name: "a".into(),
                        documentation: None,
                        payload: Some(TypeExpr::List {
                            items: Box::new(TypeExpr::Named { name: "A".into() }),
                        }),
                    }],
                },
            },
        ],
        operations: vec![],
    };
    assert_eq!(
        cyclic.validate().unwrap_err().kind,
        ValidationErrorKind::RecursiveType {
            cycle: vec!["A".into(), "B".into(), "A".into()]
        }
    );
}

#[test]
fn inline_record_enum_union_and_operation_names_are_unique() {
    let mut duplicate_field = descriptor();
    let TypeExpr::Record { fields } = &mut duplicate_field.types[2].definition else {
        unreachable!()
    };
    fields.push(field("title", TypeExpr::Integer, false));
    assert!(matches!(
        duplicate_field.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::Field,
            ..
        }
    ));

    let mut duplicate_enum_case = descriptor();
    let TypeExpr::Enum { cases } = &mut duplicate_enum_case.types[0].definition else {
        unreachable!()
    };
    cases.push(cases[0].clone());
    assert!(matches!(
        duplicate_enum_case.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::EnumCase,
            ..
        }
    ));

    let mut duplicate_operation = descriptor();
    let duplicate = duplicate_operation.operations[0].clone();
    duplicate_operation.operations.push(duplicate);
    assert!(matches!(
        duplicate_operation.validate().unwrap_err().kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::Operation,
            ..
        }
    ));
}

#[test]
fn raw_primitives_records_and_lists_validate_without_field_order_semantics() {
    let descriptor = descriptor();
    for (r#type, value) in [
        (TypeExpr::Unit, Value::Unit),
        (TypeExpr::Boolean, Value::Boolean(true)),
        (TypeExpr::Integer, Value::Integer(MAX_SAFE_INTEGER)),
        (TypeExpr::Number, Value::Number(1.5)),
        (TypeExpr::String, Value::String("hello".into())),
        (TypeExpr::Bytes, Value::Bytes(vec![0, 255])),
        (
            TypeExpr::List {
                items: Box::new(TypeExpr::Boolean),
            },
            Value::List(vec![Value::Boolean(false), Value::Boolean(true)]),
        ),
    ] {
        descriptor.validate_value(&r#type, &value).unwrap();
    }

    let schema = TypeExpr::Record {
        fields: vec![
            field("first", TypeExpr::String, true),
            field("second", TypeExpr::Integer, true),
        ],
    };
    let first_order = record([
        ("first", Value::String("x".into())),
        ("second", Value::Integer(2)),
    ]);
    let reverse_order = Value::Record(BTreeMap::from([
        ("second".into(), Value::Integer(2)),
        ("first".into(), Value::String("x".into())),
    ]));
    descriptor.validate_value(&schema, &first_order).unwrap();
    descriptor.validate_value(&schema, &reverse_order).unwrap();
}

#[test]
fn raw_numeric_record_and_list_failures_are_rejected() {
    let descriptor = descriptor();
    assert_eq!(
        descriptor
            .validate_value(&TypeExpr::Integer, &Value::Integer(MAX_SAFE_INTEGER + 1))
            .unwrap_err()
            .kind,
        ValidationErrorKind::IntegerOutOfRange {
            value: MAX_SAFE_INTEGER + 1
        }
    );
    assert_eq!(
        descriptor
            .validate_value(&TypeExpr::Number, &Value::Number(f64::NAN))
            .unwrap_err()
            .kind,
        ValidationErrorKind::NonFiniteNumber
    );
    assert_eq!(
        descriptor
            .validate_value(
                &TypeExpr::List {
                    items: Box::new(TypeExpr::Boolean)
                },
                &Value::List(vec![Value::Integer(1)])
            )
            .unwrap_err()
            .kind,
        ValidationErrorKind::TypeMismatch {
            expected: ValueKind::Boolean,
            actual: ValueKind::Integer,
        }
    );
}

#[test]
fn entry_enum_and_union_are_descriptor_interpretations_of_raw_values() {
    let descriptor = descriptor();
    descriptor
        .validate_value(&TypeExpr::Entry, &Value::String("/articles/one".into()))
        .unwrap();
    assert!(matches!(
        descriptor
            .validate_value(&TypeExpr::Entry, &Value::String("articles//one".into()))
            .unwrap_err()
            .kind,
        ValidationErrorKind::InvalidPath { .. }
    ));

    descriptor
        .validate_value(
            &TypeExpr::Named {
                name: "Color".into(),
            },
            &Value::String("red".into()),
        )
        .unwrap();
    assert!(matches!(
        descriptor
            .validate_value(
                &TypeExpr::Named {
                    name: "Color".into()
                },
                &Value::String("green".into())
            )
            .unwrap_err()
            .kind,
        ValidationErrorKind::UnknownCase { .. }
    ));

    let selection = TypeExpr::Named {
        name: "Selection".into(),
    };
    descriptor
        .validate_value(
            &selection,
            &record([("$case", Value::String("all".into()))]),
        )
        .unwrap();
    descriptor
        .validate_value(
            &selection,
            &record([
                ("$case", Value::String("one".into())),
                ("value", Value::String("/items/1".into())),
            ]),
        )
        .unwrap();

    assert_eq!(
        descriptor
            .validate_value(
                &selection,
                &record([
                    ("$case", Value::String("all".into())),
                    ("value", Value::Unit)
                ])
            )
            .unwrap_err()
            .kind,
        ValidationErrorKind::UnexpectedUnionPayload
    );
    assert_eq!(
        descriptor
            .validate_value(
                &selection,
                &record([("$case", Value::String("one".into()))])
            )
            .unwrap_err()
            .kind,
        ValidationErrorKind::MissingUnionPayload
    );
}

#[test]
fn json_uses_the_normal_value_tree_and_rejects_bytes() {
    let descriptor = descriptor();
    let json = record([
        ("null", Value::Unit),
        ("boolean", Value::Boolean(true)),
        ("integer", Value::Integer(42)),
        ("number", Value::Number(2.5)),
        ("string", Value::String("text".into())),
        (
            "array",
            Value::List(vec![Value::Unit, record([("nested", Value::Unit)])]),
        ),
    ]);
    descriptor.validate_value(&TypeExpr::Json, &json).unwrap();

    let error = descriptor
        .validate_value(
            &TypeExpr::Json,
            &record([("nested", Value::List(vec![Value::Bytes(vec![1])]))]),
        )
        .unwrap_err();
    assert_eq!(error.kind, ValidationErrorKind::BytesInJson);
    assert_eq!(
        error.path,
        vec![
            PathSegment::Descriptor,
            PathSegment::Field("nested".into()),
            PathSegment::Index(0),
        ]
    );
}

#[test]
fn calls_validate_canonical_paths_named_arguments_and_unit_results() {
    let descriptor = descriptor();
    let payload = record([("title", Value::String("hello".into()))]);
    let request = CallOperationRequest {
        target: Target {
            path: "/articles/one".into(),
            validator: Some(vec![1]),
        },
        interface: InterfaceTarget {
            scope_ref: None,
            reference: reference("/", "catalog-v1"),
            validator: Some(vec![2]),
        },
        operation: "publish".into(),
        arguments: BTreeMap::from([("payload".into(), payload)]),
    };
    request.validate_with(&descriptor).unwrap();

    CallOperationResponse {
        result: Value::Boolean(true),
        validator: Some(vec![3]),
    }
    .validate_for(&descriptor, "publish")
    .unwrap();
    descriptor.validate_result("refresh", &Value::Unit).unwrap();

    let mut missing = request.clone();
    missing.arguments.clear();
    assert_eq!(
        missing.validate_with(&descriptor).unwrap_err().kind,
        ValidationErrorKind::MissingParameter {
            name: "payload".into()
        }
    );

    let mut unknown = request.clone();
    unknown.arguments.insert("extra".into(), Value::Unit);
    assert_eq!(
        unknown.validate_with(&descriptor).unwrap_err().kind,
        ValidationErrorKind::UnknownParameter {
            name: "extra".into()
        }
    );

    let mut bad_path = request;
    bad_path.target.path = "relative".into();
    assert!(matches!(
        bad_path.validate_with(&descriptor).unwrap_err().kind,
        ValidationErrorKind::InvalidPath { .. }
    ));
}

#[test]
fn observe_and_fetch_interface_dtos_validate_request_response_correspondence() {
    let observe = ObserveRequest {
        path: "/articles/one".into(),
        depth: 0,
    };
    observe.validate().unwrap();
    observe
        .validate_response(&ObjectObservation {
            object: object("one"),
            children: None,
        })
        .unwrap();
    assert!(
        observe
            .validate_response(&ObjectObservation {
                object: object("two"),
                children: None,
            })
            .is_err()
    );

    for invalid in [
        "",
        "articles",
        "//articles",
        "/articles/",
        "/./articles",
        "/a/../b",
    ] {
        assert!(matches!(
            ObserveRequest {
                path: invalid.into(),
                depth: 0,
            }
            .validate()
            .unwrap_err()
            .kind,
            ValidationErrorKind::InvalidPath { .. }
        ));
    }

    let request = FetchInterfaceRequest {
        interface: reference("/", "catalog-v1"),
    };
    let response = FetchInterfaceResponse {
        scope_ref: None,
        interface: reference("/", "catalog-v1"),
        descriptor: descriptor(),
        validator: Some(vec![9]),
    };
    response.validate_for(&request).unwrap();
    assert!(matches!(
        FetchInterfaceResponse {
            interface: reference("/", "other"),
            ..response
        }
        .validate_for(&request)
        .unwrap_err()
        .kind,
        ValidationErrorKind::InterfaceResponseMismatch { .. }
    ));
}

#[test]
fn observe_response_validates_complete_children_and_depth_boundary() {
    let request = ObserveRequest {
        path: "/articles".into(),
        depth: 1,
    };
    let leaf = |name: &str| ObjectObservation {
        object: object(name),
        children: None,
    };
    let response = ObjectObservation {
        object: object("articles"),
        children: Some(vec![leaf("one"), leaf("two")]),
    };
    request.validate_response(&response).unwrap();

    let empty = ObjectObservation {
        object: object("articles"),
        children: Some(vec![]),
    };
    request.validate_response(&empty).unwrap();

    let missing = ObjectObservation {
        object: object("articles"),
        children: None,
    };
    assert!(matches!(
        request.validate_response(&missing).unwrap_err().kind,
        ValidationErrorKind::ChildrenMissingBeforeDepth { depth: 0 }
    ));

    let boundary = ObserveRequest {
        depth: 0,
        ..request.clone()
    };
    assert!(matches!(
        boundary.validate_response(&response).unwrap_err().kind,
        ValidationErrorKind::ChildrenAtDepthBoundary { depth: 0 }
    ));

    let duplicate = ObjectObservation {
        object: object("articles"),
        children: Some(vec![leaf("one"), leaf("one")]),
    };
    assert!(matches!(
        request.validate_response(&duplicate).unwrap_err().kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::ChildObject,
            ..
        }
    ));
}

#[test]
fn protocol_result_exposes_the_exact_canonical_wire_error_codes() {
    assert_eq!(
        ProtocolErrorCode::ALL,
        [
            ProtocolErrorCode::InvalidRequest,
            ProtocolErrorCode::NotFound,
            ProtocolErrorCode::InterfaceNotFound,
            ProtocolErrorCode::InterfaceMismatch,
            ProtocolErrorCode::OperationNotFound,
            ProtocolErrorCode::InvalidArguments,
            ProtocolErrorCode::ValidatorRequired,
            ProtocolErrorCode::ValidatorMismatch,
            ProtocolErrorCode::InterfaceValidatorRequired,
            ProtocolErrorCode::InterfaceValidatorMismatch,
            ProtocolErrorCode::PermissionDenied,
            ProtocolErrorCode::ResourceLimitExceeded,
            ProtocolErrorCode::Internal,
            ProtocolErrorCode::OperationOutcomeUnknown,
        ]
    );

    let failure: ProtocolResult<()> = ProtocolResult::Failure(ProtocolError {
        code: ProtocolErrorCode::InvalidArguments,
        message: "payload.title is required".into(),
    });
    assert!(matches!(failure, ProtocolResult::Failure(_)));
    assert_eq!(
        ProtocolResult::Success(Value::Unit),
        ProtocolResult::Success(Value::Unit)
    );
}

fn assert_allowed_codes(interaction: ProtocolInteraction, expected: &[ProtocolErrorCode]) {
    let actual: Vec<_> = ProtocolErrorCode::ALL
        .into_iter()
        .filter(|code| code.is_allowed_for(interaction))
        .collect();
    assert_eq!(actual, expected);

    for code in ProtocolErrorCode::ALL {
        assert_eq!(
            code.is_allowed_for(interaction),
            expected.contains(&code),
            "unexpected {code:?} validity for {interaction:?}"
        );
    }
}

#[test]
fn observe_error_codes_are_exhaustive() {
    assert_allowed_codes(
        ProtocolInteraction::Observe,
        &[
            ProtocolErrorCode::InvalidRequest,
            ProtocolErrorCode::NotFound,
            ProtocolErrorCode::ResourceLimitExceeded,
            ProtocolErrorCode::Internal,
        ],
    );
}

#[test]
fn fetch_interface_error_codes_are_exhaustive() {
    assert_allowed_codes(
        ProtocolInteraction::FetchInterface,
        &[
            ProtocolErrorCode::InvalidRequest,
            ProtocolErrorCode::InterfaceNotFound,
            ProtocolErrorCode::ResourceLimitExceeded,
            ProtocolErrorCode::Internal,
        ],
    );
}

#[test]
fn call_operation_error_codes_are_exhaustive() {
    assert_allowed_codes(
        ProtocolInteraction::CallOperation,
        &[
            ProtocolErrorCode::InvalidRequest,
            ProtocolErrorCode::NotFound,
            ProtocolErrorCode::InterfaceMismatch,
            ProtocolErrorCode::OperationNotFound,
            ProtocolErrorCode::InvalidArguments,
            ProtocolErrorCode::ValidatorRequired,
            ProtocolErrorCode::ValidatorMismatch,
            ProtocolErrorCode::InterfaceValidatorRequired,
            ProtocolErrorCode::InterfaceValidatorMismatch,
            ProtocolErrorCode::PermissionDenied,
            ProtocolErrorCode::ResourceLimitExceeded,
            ProtocolErrorCode::Internal,
            ProtocolErrorCode::OperationOutcomeUnknown,
        ],
    );
}

#[test]
fn invalid_success_payloads_remain_client_local_validation_failures() {
    let observe = ObserveRequest {
        path: "/articles/one".into(),
        depth: 0,
    };
    assert!(matches!(
        observe
            .validate_response(&ObjectObservation {
                object: object("other"),
                children: None,
            })
            .unwrap_err()
            .kind,
        ValidationErrorKind::ObjectNameMismatch { .. }
    ));

    let interface_request = FetchInterfaceRequest {
        interface: reference("/", "catalog-v1"),
    };
    let mut invalid_descriptor = descriptor();
    invalid_descriptor
        .operations
        .push(invalid_descriptor.operations[0].clone());
    assert!(matches!(
        FetchInterfaceResponse {
            interface: interface_request.interface.clone(),
            scope_ref: None,
            descriptor: invalid_descriptor,
            validator: None,
        }
        .validate_for(&interface_request)
        .unwrap_err()
        .kind,
        ValidationErrorKind::DuplicateName {
            namespace: NameNamespace::Operation,
            ..
        }
    ));

    assert!(matches!(
        CallOperationResponse {
            result: Value::String("not a boolean".into()),
            validator: None,
        }
        .validate_for(&descriptor(), "publish")
        .unwrap_err()
        .kind,
        ValidationErrorKind::TypeMismatch {
            expected: ValueKind::Boolean,
            actual: ValueKind::String,
        }
    ));
}
