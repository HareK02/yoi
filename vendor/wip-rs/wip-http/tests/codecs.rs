use std::collections::BTreeMap;

use wip_http::http::StatusCode;
use wip_http::wip_protocol::{
    CallOperationRequest, CallOperationResponse, Documentation, EnumCase, FetchInterfaceRequest,
    FetchInterfaceResponse, FieldDeclaration, INTERFACE_FORMAT_V1, InterfaceDescriptor,
    InterfaceReference, InterfaceTarget, MAX_SAFE_INTEGER, Object, ObjectObservation,
    ObserveRequest, OperationDeclaration, ParameterDeclaration, ReturnDeclaration, Target,
    TypeDeclaration, TypeExpr, UnionCase, Value,
};
use wip_http::{
    ClientResponseError, CodecError, DecodedResponse, Endpoint, InvalidResponseKind, Limits,
    decode_call_operation_metadata, decode_call_operation_request, decode_call_operation_response,
    decode_fetch_interface_request, decode_fetch_interface_response, decode_observe_request,
    decode_observe_response, encode_call_operation_request, encode_call_operation_response,
    encode_fetch_interface_request, encode_fetch_interface_response, encode_observe_request,
    encode_observe_response,
};

fn limits() -> Limits {
    Limits::new(1_000_000, 1_000_000, 100).unwrap()
}

fn endpoint() -> Endpoint {
    Endpoint::parse("https://example.test/wip").unwrap()
}

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

fn object(name: &str, validator: &[u8]) -> Object {
    Object {
        name: name.into(),
        description: Some("description".into()),
        interfaces: vec![reference("/", "interface-v1")],
        r#ref: Some(format!("object:{name}")),
        validator: Some(validator.to_vec()),
    }
}

fn descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: Some(Documentation {
            summary: "Codec fixture".into(),
            details: None,
        }),
        types: vec![TypeDeclaration {
            name: "Pair".into(),
            documentation: None,
            definition: TypeExpr::Record {
                fields: vec![
                    FieldDeclaration {
                        name: "required".into(),
                        required: true,
                        documentation: None,
                        r#type: TypeExpr::String,
                    },
                    FieldDeclaration {
                        name: "optional".into(),
                        required: false,
                        documentation: None,
                        r#type: TypeExpr::String,
                    },
                    FieldDeclaration {
                        name: "optional_json".into(),
                        required: false,
                        documentation: None,
                        r#type: TypeExpr::Json,
                    },
                ],
            },
        }],
        operations: vec![OperationDeclaration {
            name: "everything".into(),
            documentation: None,
            parameters: vec![
                parameter("unit", TypeExpr::Unit),
                parameter("boolean", TypeExpr::Boolean),
                parameter("integer", TypeExpr::Integer),
                parameter("number", TypeExpr::Number),
                parameter("string", TypeExpr::String),
                parameter("bytes", TypeExpr::Bytes),
                parameter("entry", TypeExpr::Entry),
                parameter(
                    "list",
                    TypeExpr::List {
                        items: Box::new(TypeExpr::Integer),
                    },
                ),
                parameter(
                    "record",
                    TypeExpr::Named {
                        name: "Pair".into(),
                    },
                ),
                parameter(
                    "enum",
                    TypeExpr::Enum {
                        cases: vec![EnumCase {
                            name: "ready".into(),
                            documentation: None,
                        }],
                    },
                ),
                parameter(
                    "union",
                    TypeExpr::Union {
                        cases: vec![
                            UnionCase {
                                name: "empty".into(),
                                documentation: None,
                                payload: None,
                            },
                            UnionCase {
                                name: "found".into(),
                                documentation: None,
                                payload: Some(TypeExpr::Entry),
                            },
                        ],
                    },
                ),
                parameter("json", TypeExpr::Json),
            ],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Bytes,
            },
        }],
    }
}

fn parameter(name: &str, r#type: TypeExpr) -> ParameterDeclaration {
    ParameterDeclaration {
        name: name.into(),
        required: true,
        documentation: None,
        r#type,
    }
}

fn record(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    Value::Record(
        fields
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect(),
    )
}

fn call_request() -> CallOperationRequest {
    CallOperationRequest {
        target: Target {
            path: "/items/123".into(),
            validator: Some(vec![0, 1, 2, 255]),
        },
        interface: InterfaceTarget {
            reference: reference("/", "interface-v1"),
            scope_ref: None,
            validator: Some(vec![9, 8, 7]),
        },
        operation: "everything".into(),
        arguments: BTreeMap::from([
            ("unit".into(), Value::Unit),
            ("boolean".into(), Value::Boolean(true)),
            ("integer".into(), Value::Integer(MAX_SAFE_INTEGER)),
            ("number".into(), Value::Number(1.5)),
            ("string".into(), Value::String("text".into())),
            ("bytes".into(), Value::Bytes(vec![0, 1, 2, 255])),
            ("entry".into(), Value::String("/items/123".into())),
            (
                "list".into(),
                Value::List(vec![Value::Integer(1), Value::Integer(2)]),
            ),
            (
                "record".into(),
                record([("required", Value::String("present".into()))]),
            ),
            ("enum".into(), Value::String("ready".into())),
            (
                "union".into(),
                record([
                    ("$case", Value::String("found".into())),
                    ("value", Value::String("/items/123".into())),
                ]),
            ),
            (
                "json".into(),
                record([
                    ("null", Value::Unit),
                    ("number", Value::Integer(2)),
                    ("path_like", Value::String("/not-an-entry".into())),
                ]),
            ),
        ]),
    }
}

#[test]
fn every_single_interaction_request_accepts_json() {
    let observe = encode_observe_request(
        &endpoint(),
        &ObserveRequest {
            path: "/".into(),
            depth: 0,
        },
        limits(),
    )
    .unwrap();
    let fetch_interface = encode_fetch_interface_request(
        &endpoint(),
        &FetchInterfaceRequest {
            interface: reference("/", "interface-v1"),
        },
        limits(),
    )
    .unwrap();
    let call = encode_call_operation_request(&endpoint(), &call_request(), &descriptor(), limits())
        .unwrap();

    for request in [observe, fetch_interface, call] {
        assert_eq!(request.headers()["accept"], "application/json");
    }
}

#[test]
fn depth_zero_observe_round_trips_direct_body_and_validator() {
    let request = ObserveRequest {
        path: "/items/123".into(),
        depth: 0,
    };
    let encoded_request = encode_observe_request(&endpoint(), &request, limits()).unwrap();
    assert_eq!(encoded_request.uri().path(), "/wip/v1/observe");
    assert_eq!(
        decode_observe_request(encoded_request.body(), limits()).unwrap(),
        request
    );

    let expected = ObjectObservation {
        object: object("123", &[0, 1, 2, 255]),
        children: None,
    };
    let encoded = encode_observe_response(&request, &expected, limits()).unwrap();
    assert_eq!(encoded.status(), StatusCode::OK);
    assert!(
        !std::str::from_utf8(encoded.body())
            .unwrap()
            .contains("children")
    );
    assert_eq!(
        decode_observe_response(&request, &encoded, limits()).unwrap(),
        DecodedResponse::Success(expected)
    );
    assert!(encoded.headers().get("etag").is_none());
}

#[test]
fn observe_and_interface_round_trip_nested_logical_models() {
    let observe_request = ObserveRequest {
        path: "/items".into(),
        depth: 1,
    };
    let encoded_request = encode_observe_request(&endpoint(), &observe_request, limits()).unwrap();
    assert_eq!(
        decode_observe_request(encoded_request.body(), limits()).unwrap(),
        observe_request
    );
    let observation = ObjectObservation {
        object: object("items", b"root-validator"),
        children: Some(vec![ObjectObservation {
            object: object("123", b"child-validator"),
            children: None,
        }]),
    };
    let encoded = encode_observe_response(&observe_request, &observation, limits()).unwrap();
    assert_eq!(
        decode_observe_response(&observe_request, &encoded, limits()).unwrap(),
        DecodedResponse::Success(observation)
    );

    let empty = ObjectObservation {
        object: object("items", b"root-validator"),
        children: Some(vec![]),
    };
    let encoded_empty = encode_observe_response(&observe_request, &empty, limits()).unwrap();
    assert!(
        std::str::from_utf8(encoded_empty.body())
            .unwrap()
            .contains("\"children\":[]")
    );

    let interface_request = FetchInterfaceRequest {
        interface: reference("/", "interface-v1"),
    };
    let encoded_request =
        encode_fetch_interface_request(&endpoint(), &interface_request, limits()).unwrap();
    assert_eq!(
        decode_fetch_interface_request(encoded_request.body(), limits()).unwrap(),
        interface_request
    );
    let interface = FetchInterfaceResponse {
        interface: interface_request.interface.clone(),
        scope_ref: None,
        descriptor: descriptor(),
        validator: Some(vec![4, 5, 6, 255]),
    };
    let encoded =
        encode_fetch_interface_response(&interface_request, &interface, limits()).unwrap();
    assert_eq!(
        decode_fetch_interface_response(&interface_request, &encoded, limits()).unwrap(),
        DecodedResponse::Success(interface)
    );
}

#[test]
fn descriptor_guided_values_and_validators_round_trip_both_directions() {
    let descriptor = descriptor();
    let request = call_request();
    let encoded =
        encode_call_operation_request(&endpoint(), &request, &descriptor, limits()).unwrap();
    let body = std::str::from_utf8(encoded.body()).unwrap();
    assert!(body.contains("AAEC/w=="));
    let metadata = decode_call_operation_metadata(encoded.body(), limits()).unwrap();
    assert_eq!(metadata.target, request.target);
    assert_eq!(metadata.interface, request.interface);
    assert_eq!(metadata.operation, request.operation);
    assert_eq!(
        metadata
            .decode_request(encoded.body(), &descriptor, limits())
            .unwrap(),
        request
    );
    let mut changed_route = request.clone();
    changed_route.target.path = "/items/other".into();
    let changed =
        encode_call_operation_request(&endpoint(), &changed_route, &descriptor, limits()).unwrap();
    assert!(
        metadata
            .decode_request(changed.body(), &descriptor, limits())
            .is_err()
    );

    let mut null_json_request = request.clone();
    null_json_request
        .arguments
        .insert("json".into(), Value::Unit);
    null_json_request.arguments.insert(
        "record".into(),
        record([
            ("required", Value::String("present".into())),
            ("optional_json", Value::Unit),
        ]),
    );
    let null_json =
        encode_call_operation_request(&endpoint(), &null_json_request, &descriptor, limits())
            .unwrap();
    assert_eq!(
        decode_call_operation_request(null_json.body(), &descriptor, limits()).unwrap(),
        null_json_request
    );

    let expected = CallOperationResponse {
        result: Value::Bytes(vec![255, 0, 1]),
        validator: Some(vec![3, 2, 1, 0]),
    };
    let encoded =
        encode_call_operation_response(&request, &descriptor, &expected, limits()).unwrap();
    assert!(
        std::str::from_utf8(encoded.body())
            .unwrap()
            .contains("/wAB")
    );
    assert_eq!(
        decode_call_operation_response(&request, &descriptor, &encoded, limits()).unwrap(),
        DecodedResponse::Success(expected)
    );
}

#[test]
fn malformed_unknown_null_and_noncanonical_base64_are_rejected() {
    let limits = limits();
    assert!(decode_observe_request(br#"{"path":"/","depth":0,"extra":true}"#, limits).is_err());
    assert!(
        decode_observe_request(br#"{"path":"relative","path":"/","depth":0}"#, limits).is_err()
    );

    let request = call_request();
    let descriptor = descriptor();
    let body = serde_json::to_vec(&serde_json::json!({
        "target": {"path": "/items/123", "validator": null},
        "interface": {"reference": {"scope": "/", "name": "interface-v1"}},
        "operation": "everything",
        "arguments": {}
    }))
    .unwrap();
    assert!(decode_call_operation_request(&body, &descriptor, limits).is_err());

    let mut json: serde_json::Value = serde_json::from_slice(
        encode_call_operation_request(&endpoint(), &request, &descriptor, limits)
            .unwrap()
            .body(),
    )
    .unwrap();
    json["arguments"]["bytes"] = serde_json::Value::String("AAEC_w==".into());
    assert!(
        decode_call_operation_request(&serde_json::to_vec(&json).unwrap(), &descriptor, limits)
            .is_err()
    );
}

#[test]
fn caller_limits_remain_local_codec_failures() {
    let tiny = Limits::new(8, 8, 2).unwrap();
    let too_large = decode_observe_request(br#"{"path":"/long","depth":0}"#, tiny).unwrap_err();
    assert!(matches!(&too_large, CodecError::BodyTooLarge { .. }));

    let nested = br#"{"path":[[["/"]]],"depth":0}"#;
    let nesting_limited = Limits::new(1024, 1024, 2).unwrap();
    let too_deep = decode_observe_request(nested, nesting_limited).unwrap_err();
    assert!(matches!(&too_deep, CodecError::NestingTooDeep { .. }));

    let outbound_limited = Limits::new(1_000_000, 1_000_000, 1).unwrap();
    assert!(
        encode_call_operation_request(
            &endpoint(),
            &call_request(),
            &descriptor(),
            outbound_limited,
        )
        .is_err()
    );
    assert!(
        encode_observe_response(
            &ObserveRequest {
                path: "/item".into(),
                depth: 0,
            },
            &ObjectObservation {
                object: object("item", b"validator"),
                children: None,
            },
            outbound_limited,
        )
        .is_err()
    );
}

#[test]
fn unsupported_descriptor_format_is_client_local() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "interface-v1"),
    };
    let mut interface = FetchInterfaceResponse {
        interface: request.interface.clone(),
        scope_ref: None,
        descriptor: descriptor(),
        validator: None,
    };
    interface.descriptor.format = "future-format".into();
    let response = encode_fetch_interface_response(&request, &interface, limits()).unwrap();
    let error = decode_fetch_interface_response(&request, &response, limits()).unwrap_err();
    assert!(matches!(
        error,
        ClientResponseError::UnsupportedDescriptorFormat { .. }
    ));

    let future_shape = wip_http::http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8")
        .body(
            br#"{"interface":{"scope":"/","name":"interface-v1"},"descriptor":{"format":"wip-interface/2","future_shape":true}}"#
                .to_vec(),
        )
        .unwrap();
    assert!(matches!(
        decode_fetch_interface_response(&request, &future_shape, limits()).unwrap_err(),
        ClientResponseError::UnsupportedDescriptorFormat { format }
            if format == "wip-interface/2"
    ));

    for malformed in [
        br#"{"interface":{"scope":"/","name":"wrong"},"descriptor":{"format":"wip-interface/2"}}"#.as_slice(),
        br#"{"interface":{"scope":"/","name":"interface-v1"},"descriptor":{"format":"wip-interface/2"},"validator":null}"#
            .as_slice(),
    ] {
        let response = wip_http::http::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "application/json; charset=utf-8")
            .body(malformed.to_vec())
            .unwrap();
        assert!(matches!(
            decode_fetch_interface_response(&request, &response, limits()).unwrap_err(),
            ClientResponseError::InvalidResponse {
                kind: InvalidResponseKind::InvalidSuccessBody,
                ..
            }
        ));
    }
}

#[test]
fn invalid_success_is_classified_as_invalid_response() {
    let response = wip_http::http::Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8")
        .body(br#"{"object":{"name":"wrong","interfaces":[]}}"#.to_vec())
        .unwrap();
    let error = decode_observe_response(
        &ObserveRequest {
            path: "/expected".into(),
            depth: 0,
        },
        &response,
        limits(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::InvalidSuccessBody,
            ..
        }
    ));
}
