use std::collections::BTreeMap;

use serde_json::{Value as Json, json};
use wip_http::http::{Response, StatusCode};
use wip_http::wip_protocol::{
    CallOperationRequest, FetchInterfaceRequest, FetchInterfaceResponse, INTERFACE_FORMAT_V1,
    InterfaceDescriptor, InterfaceReference, InterfaceTarget, Object, ObjectObservation,
    ObserveRequest, OperationDeclaration, ProtocolError, ProtocolErrorCode, ProtocolInteraction,
    ReturnDeclaration, Target, TypeExpr,
};
use wip_http::{
    ClientResponseError, CodecError, DecodedResponse, Endpoint, InvalidResponseKind, Limits,
    TransportFailureKind, decode_call_operation_metadata, decode_call_operation_request,
    decode_call_operation_response, decode_fetch_interface_request,
    decode_fetch_interface_response, decode_observe_response, encode_call_operation_request,
    encode_fetch_interface_request, encode_fetch_interface_response, encode_observe_response,
    encode_protocol_error_response,
};

fn limits() -> Limits {
    Limits::new(64 * 1024, 64 * 1024, 64).unwrap()
}

fn endpoint() -> Endpoint {
    Endpoint::parse("https://example.test/deployment/prefix").unwrap()
}

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

fn descriptor() -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: INTERFACE_FORMAT_V1.into(),
        documentation: None,
        types: vec![],
        operations: vec![OperationDeclaration {
            name: "run#::操作".into(),
            documentation: None,
            parameters: vec![],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Unit,
            },
        }],
    }
}

fn call(reference: InterfaceReference, scope_ref: Option<String>) -> CallOperationRequest {
    CallOperationRequest {
        target: Target {
            path: "/世界::#/item".into(),
            validator: Some(vec![0, 255]),
        },
        interface: InterfaceTarget {
            reference,
            scope_ref,
            validator: Some(vec![1, 255]),
        },
        operation: "run#::操作".into(),
        arguments: BTreeMap::new(),
    }
}

fn observation(references: Vec<InterfaceReference>) -> ObjectObservation {
    ObjectObservation {
        object: Object {
            name: "item".into(),
            description: None,
            interfaces: references,
            r#ref: None,
            validator: None,
        },
        children: None,
    }
}

fn observe_request() -> ObserveRequest {
    ObserveRequest {
        path: "/世界::#/item".into(),
        depth: 0,
    }
}

fn response(status: StatusCode, value: Json) -> Response<Vec<u8>> {
    raw_response(status, &serde_json::to_vec(&value).unwrap())
}

fn raw_response(status: StatusCode, body: &[u8]) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(body.to_vec())
        .unwrap()
}

fn fetch_json(interface: Json) -> Json {
    json!({
        "interface": interface,
        "descriptor": {"format": INTERFACE_FORMAT_V1, "types": [], "operations": []}
    })
}

fn call_json(reference: Json) -> Json {
    json!({
        "target": {"path": "/世界::#/item"},
        "interface": {"reference": reference},
        "operation": "run#::操作",
        "arguments": {}
    })
}

fn invalid_success<T: std::fmt::Debug>(result: Result<T, ClientResponseError>) {
    assert!(matches!(
        result.unwrap_err(),
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::InvalidSuccessBody,
            ..
        }
    ));
}

#[test]
fn exact_structured_reference_is_encoded_and_round_tripped_in_every_position() {
    let interface_ref = reference("/世界::#", "é::接口#read/with\"quote\\slash");
    let expected = json!({"scope": interface_ref.scope, "name": interface_ref.name});
    let fetch_request = FetchInterfaceRequest {
        interface: interface_ref.clone(),
    };
    let encoded = encode_fetch_interface_request(&endpoint(), &fetch_request, limits()).unwrap();
    assert_eq!(
        encoded.uri().path(),
        "/deployment/prefix/v1/fetch_interface"
    );
    assert_eq!(
        serde_json::from_slice::<Json>(encoded.body()).unwrap(),
        json!({"interface": expected})
    );
    assert_eq!(
        decode_fetch_interface_request(encoded.body(), limits()).unwrap(),
        fetch_request
    );

    let fetch_response = FetchInterfaceResponse {
        interface: interface_ref.clone(),
        scope_ref: Some("opaque:世界::# not base64!".into()),
        descriptor: descriptor(),
        validator: Some(vec![1, 255]),
    };
    let encoded =
        encode_fetch_interface_response(&fetch_request, &fetch_response, limits()).unwrap();
    let body: Json = serde_json::from_slice(encoded.body()).unwrap();
    assert_eq!(body["interface"], expected);
    assert_eq!(body["scope_ref"], "opaque:世界::# not base64!");
    assert_eq!(body["validator"], "Af8=");
    assert_eq!(
        decode_fetch_interface_response(&fetch_request, &encoded, limits()).unwrap(),
        DecodedResponse::Success(fetch_response)
    );

    let observation = observation(vec![interface_ref.clone(), reference("/", "root")]);
    let encoded = encode_observe_response(&observe_request(), &observation, limits()).unwrap();
    let body: Json = serde_json::from_slice(encoded.body()).unwrap();
    assert_eq!(
        body["object"]["interfaces"],
        json!([expected, {"scope": "/", "name": "root"}])
    );
    assert_eq!(
        decode_observe_response(&observe_request(), &encoded, limits()).unwrap(),
        DecodedResponse::Success(observation)
    );

    let request = call(interface_ref, Some("opaque:世界::# not base64!".into()));
    let encoded =
        encode_call_operation_request(&endpoint(), &request, &descriptor(), limits()).unwrap();
    let body: Json = serde_json::from_slice(encoded.body()).unwrap();
    assert_eq!(body["interface"]["reference"], expected);
    assert_eq!(body["interface"]["scope_ref"], "opaque:世界::# not base64!");
    assert_eq!(body["interface"]["validator"], "Af8=");
    assert_eq!(body["target"]["validator"], "AP8=");
    assert_eq!(body["operation"], "run#::操作");
    assert_eq!(
        decode_call_operation_request(encoded.body(), &descriptor(), limits()).unwrap(),
        request
    );
    assert_eq!(
        decode_call_operation_metadata(encoded.body(), limits())
            .unwrap()
            .interface,
        request.interface
    );
}

#[test]
fn malformed_reference_corpus_is_rejected_in_all_positions() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "interface-v1"),
    };
    for bad in [
        json!("interface-v1"),
        json!("/::interface-v1"),
        json!("interface-v1#run"),
        Json::Null,
        json!(false),
        json!(1),
        json!([]),
        json!({}),
        json!({"name": "interface-v1"}),
        json!({"scope": "/"}),
        json!({"scope": null, "name": "interface-v1"}),
        json!({"scope": 1, "name": "interface-v1"}),
        json!({"scope": "/", "name": null}),
        json!({"scope": "/", "name": []}),
        json!({"scope": "/", "name": ""}),
        json!({"scope": "/", "name": "interface-v1", "extra": true}),
        json!({"scope": "/", "name": "interface-v1", "scope_ref": "identity"}),
        json!({"Scope": "/", "name": "interface-v1"}),
        json!({"scope": "", "name": "interface-v1"}),
        json!({"scope": "relative", "name": "interface-v1"}),
        json!({"scope": ".", "name": "interface-v1"}),
        json!({"scope": "..", "name": "interface-v1"}),
        json!({"scope": "/a/", "name": "interface-v1"}),
        json!({"scope": "//a", "name": "interface-v1"}),
        json!({"scope": "/a//b", "name": "interface-v1"}),
        json!({"scope": "/a/./b", "name": "interface-v1"}),
        json!({"scope": "/a/../b", "name": "interface-v1"}),
    ] {
        let body = serde_json::to_vec(&json!({"interface": bad})).unwrap();
        assert!(
            decode_fetch_interface_request(&body, limits()).is_err(),
            "{bad}"
        );
        let body = serde_json::to_vec(&call_json(bad.clone())).unwrap();
        assert!(
            decode_call_operation_metadata(&body, limits()).is_err(),
            "{bad}"
        );
        assert!(
            decode_call_operation_request(&body, &descriptor(), limits()).is_err(),
            "{bad}"
        );
        invalid_success(decode_fetch_interface_response(
            &request,
            &response(StatusCode::OK, fetch_json(bad.clone())),
            limits(),
        ));
        let mut future = fetch_json(bad.clone());
        future["descriptor"] = json!({"format": "future-format", "future": true});
        invalid_success(decode_fetch_interface_response(
            &request,
            &response(StatusCode::OK, future),
            limits(),
        ));
        invalid_success(decode_observe_response(
            &observe_request(),
            &response(
                StatusCode::OK,
                json!({"object": {"name": "item", "interfaces": [bad]}}),
            ),
            limits(),
        ));
    }
}

#[test]
fn duplicate_reference_members_are_rejected_even_when_escaped_or_equal() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "x"),
    };
    for bad in [
        r#"{"scope":"/","scope":"/","name":"x"}"#,
        r#"{"scope":"/","name":"x","name":"x"}"#,
        r#"{"scope":"/","name":"x","na\u006de":"x"}"#,
    ] {
        let fetch = format!(r#"{{"interface":{bad}}}"#);
        assert!(matches!(
            decode_fetch_interface_request(fetch.as_bytes(), limits()).unwrap_err(),
            CodecError::InvalidJson(_)
        ));
        let call = format!(
            r#"{{"target":{{"path":"/"}},"interface":{{"reference":{bad}}},"operation":"run","arguments":{{}}}}"#
        );
        assert!(matches!(
            decode_call_operation_metadata(call.as_bytes(), limits()).unwrap_err(),
            CodecError::InvalidJson(_)
        ));
        assert!(matches!(
            decode_call_operation_request(call.as_bytes(), &descriptor(), limits()).unwrap_err(),
            CodecError::InvalidJson(_)
        ));
        let fetch_response = format!(
            r#"{{"interface":{bad},"descriptor":{{"format":"wip-interface/1","types":[],"operations":[]}}}}"#
        );
        invalid_success(decode_fetch_interface_response(
            &request,
            &raw_response(StatusCode::OK, fetch_response.as_bytes()),
            limits(),
        ));
        let observe = format!(r#"{{"object":{{"name":"item","interfaces":[{bad}]}}}}"#);
        invalid_success(decode_observe_response(
            &observe_request(),
            &raw_response(StatusCode::OK, observe.as_bytes()),
            limits(),
        ));
    }
}

#[test]
fn member_order_and_json_escapes_do_not_change_reference_identity() {
    let expected = reference("/世界::#", "é::接口#");
    let raw_reference = r#"{"name":"\u00e9::\u63a5\u53e3#","scope":"\/\u4e16\u754c::#"}"#;
    let fetch = format!(r#"{{"interface":{raw_reference}}}"#);
    let request = decode_fetch_interface_request(fetch.as_bytes(), limits()).unwrap();
    assert_eq!(request.interface, expected);
    let fetch_response = format!(
        r#"{{"descriptor":{{"operations":[],"types":[],"format":"wip-interface/1"}},"interface":{raw_reference}}}"#
    );
    assert!(
        matches!(decode_fetch_interface_response(&request, &raw_response(StatusCode::OK, fetch_response.as_bytes()), limits()).unwrap(), DecodedResponse::Success(value) if value.interface == expected)
    );
    let call = format!(
        r#"{{"arguments":{{}},"operation":"run#::操作","interface":{{"reference":{raw_reference}}},"target":{{"path":"/世界::#/item"}}}}"#
    );
    assert_eq!(
        decode_call_operation_metadata(call.as_bytes(), limits())
            .unwrap()
            .interface
            .reference,
        expected
    );
    assert_eq!(
        decode_call_operation_request(call.as_bytes(), &descriptor(), limits())
            .unwrap()
            .interface
            .reference,
        expected
    );
    let observe = format!(r#"{{"object":{{"interfaces":[{raw_reference}],"name":"item"}}}}"#);
    assert!(
        matches!(decode_observe_response(&observe_request(), &raw_response(StatusCode::OK, observe.as_bytes()), limits()).unwrap(), DecodedResponse::Success(value) if value.object.interfaces == vec![expected])
    );
}

#[test]
fn reference_comparison_is_exact_without_normalization_case_or_url_decoding() {
    for different in [
        reference("/", "e\u{301}"),
        reference("/", "É"),
        reference("/", "%C3%A9"),
        reference("/other", "é"),
    ] {
        let request = FetchInterfaceRequest {
            interface: reference("/", "é"),
        };
        invalid_success(decode_fetch_interface_response(
            &request,
            &response(
                StatusCode::OK,
                fetch_json(json!({"scope": different.scope, "name": different.name})),
            ),
            limits(),
        ));
    }
    let request = FetchInterfaceRequest {
        interface: reference("/%2E", "é"),
    };
    let encoded = encode_fetch_interface_request(&endpoint(), &request, limits()).unwrap();
    assert_eq!(
        decode_fetch_interface_request(encoded.body(), limits()).unwrap(),
        request
    );
    let request = FetchInterfaceRequest {
        interface: reference("/", "."),
    };
    assert!(encode_fetch_interface_request(&endpoint(), &request, limits()).is_ok());
}

#[test]
fn request_encoding_rejects_invalid_reference_shape_without_needing_a_fetch_target() {
    for bad in [
        reference("relative", "x"),
        reference("", "x"),
        reference("/a/../b", "x"),
        reference("/", ""),
    ] {
        assert!(
            encode_fetch_interface_request(
                &endpoint(),
                &FetchInterfaceRequest {
                    interface: bad.clone()
                },
                limits()
            )
            .is_err()
        );
        assert!(
            encode_call_operation_request(&endpoint(), &call(bad, None), &descriptor(), limits())
                .is_err()
        );
    }
    let request = FetchInterfaceRequest {
        interface: reference("/unrelated/scope", "x"),
    };
    let encoded = encode_fetch_interface_request(&endpoint(), &request, limits()).unwrap();
    assert_eq!(
        decode_fetch_interface_request(encoded.body(), limits()).unwrap(),
        request
    );
}

#[test]
fn call_metadata_leaves_valid_out_of_scope_references_and_identity_to_host_resolution() {
    let request = call(
        reference("/elsewhere", "x"),
        Some("not-current-identity".into()),
    );
    let encoded =
        encode_call_operation_request(&endpoint(), &request, &descriptor(), limits()).unwrap();
    let metadata = decode_call_operation_metadata(encoded.body(), limits()).unwrap();
    assert_eq!(metadata.interface, request.interface);
    assert_eq!(
        metadata
            .decode_request(encoded.body(), &descriptor(), limits())
            .unwrap(),
        request
    );

    // Neither an unknown operation nor descriptor-bound argument values are
    // resolved by the metadata boundary before Host precondition checks.
    let mut body: Json = serde_json::from_slice(encoded.body()).unwrap();
    body["operation"] = json!("missing");
    body["arguments"] = json!({"not_declared": [null, {"anything": true}]});
    let metadata =
        decode_call_operation_metadata(&serde_json::to_vec(&body).unwrap(), limits()).unwrap();
    assert_eq!(metadata.operation, "missing");
    assert_eq!(metadata.interface, request.interface);
}

#[test]
fn observation_scope_validation_uses_each_nodes_path_and_structural_duplicates() {
    let request = ObserveRequest {
        path: "/a".into(),
        depth: 1,
    };
    let valid = json!({
        "object": {"name": "a", "interfaces": [{"scope": "/", "name": "x"}, {"scope": "/a", "name": "x"}]},
        "children": [{"object": {"name": "b", "interfaces": [{"scope": "/a/b", "name": "x"}]}}]
    });
    assert!(
        decode_observe_response(&request, &response(StatusCode::OK, valid.clone()), limits())
            .is_ok()
    );
    for scope in ["/ab", "/elsewhere", "/a/b/c"] {
        let mut bad = valid.clone();
        bad["children"][0]["object"]["interfaces"][0]["scope"] = json!(scope);
        invalid_success(decode_observe_response(
            &request,
            &response(StatusCode::OK, bad),
            limits(),
        ));
    }
    let mut duplicate = valid;
    duplicate["object"]["interfaces"] =
        json!([{"scope": "/a", "name": "x"}, {"name": "x", "scope": "/a"}]);
    invalid_success(decode_observe_response(
        &request,
        &response(StatusCode::OK, duplicate),
        limits(),
    ));
    let root_request = ObserveRequest {
        path: "/".into(),
        depth: 0,
    };
    for (scope, valid) in [("/", true), ("/a", false)] {
        let result = decode_observe_response(
            &root_request,
            &response(
                StatusCode::OK,
                json!({"object": {"name": "", "interfaces": [{"scope": scope, "name": "x"}]}}),
            ),
            limits(),
        );
        if valid {
            assert!(result.is_ok());
        } else {
            invalid_success(result);
        }
    }
}

#[test]
fn scope_ref_omission_and_opaque_strings_round_trip_in_both_positions() {
    for scope_ref in [
        None,
        Some(String::new()),
        Some("opaque::# 世界 ! not base64 /+=".into()),
        Some("\"\\\n\u{0}".into()),
    ] {
        let request = FetchInterfaceRequest {
            interface: reference("/", "x"),
        };
        let expected = FetchInterfaceResponse {
            interface: request.interface.clone(),
            scope_ref: scope_ref.clone(),
            descriptor: descriptor(),
            validator: None,
        };
        let encoded = encode_fetch_interface_response(&request, &expected, limits()).unwrap();
        let body: Json = serde_json::from_slice(encoded.body()).unwrap();
        assert_eq!(
            body.get("scope_ref"),
            scope_ref
                .as_ref()
                .map(|value| Json::String(value.clone()))
                .as_ref()
        );
        assert!(body.get("validator").is_none());
        assert_eq!(
            decode_fetch_interface_response(&request, &encoded, limits()).unwrap(),
            DecodedResponse::Success(expected)
        );
        let call = call(request.interface, scope_ref);
        let encoded =
            encode_call_operation_request(&endpoint(), &call, &descriptor(), limits()).unwrap();
        let body: Json = serde_json::from_slice(encoded.body()).unwrap();
        assert_eq!(
            body["interface"].get("scope_ref"),
            call.interface
                .scope_ref
                .as_ref()
                .map(|value| Json::String(value.clone()))
                .as_ref()
        );
        assert_eq!(
            decode_call_operation_metadata(encoded.body(), limits())
                .unwrap()
                .interface,
            call.interface
        );
        assert_eq!(
            decode_call_operation_request(encoded.body(), &descriptor(), limits()).unwrap(),
            call
        );
    }
}

#[test]
fn scope_ref_null_and_every_nonstring_type_are_rejected_before_descriptor_resolution() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "x"),
    };
    for bad in [Json::Null, json!(true), json!(0), json!([]), json!({})] {
        let mut fetch = fetch_json(json!({"scope": "/", "name": "x"}));
        fetch["scope_ref"] = bad.clone();
        invalid_success(decode_fetch_interface_response(
            &request,
            &response(StatusCode::OK, fetch.clone()),
            limits(),
        ));
        // Unknown format does not mask malformed common response fields.
        fetch["descriptor"] = json!({"format": "future-format", "future": true});
        invalid_success(decode_fetch_interface_response(
            &request,
            &response(StatusCode::OK, fetch),
            limits(),
        ));
        let mut call = call_json(json!({"scope": "/", "name": "x"}));
        call["interface"]["scope_ref"] = bad.clone();
        let body = serde_json::to_vec(&call).unwrap();
        let error = decode_call_operation_metadata(&body, limits()).unwrap_err();
        if bad.is_null() {
            assert!(matches!(error, CodecError::NullOptionalField { .. }));
        } else {
            assert!(matches!(error, CodecError::InvalidField { .. }));
        }
        assert!(decode_call_operation_request(&body, &descriptor(), limits()).is_err());
    }
}

#[test]
fn scope_ref_is_only_allowed_on_fetch_responses_and_call_interface_targets() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "x"),
    };
    let reference = json!({"scope": "/", "name": "x"});
    let fetch_request = json!({"interface": reference, "scope_ref": "identity"});
    assert!(
        decode_fetch_interface_request(&serde_json::to_vec(&fetch_request).unwrap(), limits())
            .is_err()
    );

    let mut call = call_json(reference.clone());
    call["scope_ref"] = json!("identity");
    assert!(decode_call_operation_metadata(&serde_json::to_vec(&call).unwrap(), limits()).is_err());
    let mut call = call_json(reference);
    call["target"]["scope_ref"] = json!("identity");
    assert!(decode_call_operation_metadata(&serde_json::to_vec(&call).unwrap(), limits()).is_err());

    let duplicate = br#"{"interface":{"scope":"/","name":"x"},"scope_ref":"identity","scope_ref":"identity","descriptor":{"format":"wip-interface/1","types":[],"operations":[]}}"#;
    invalid_success(decode_fetch_interface_response(
        &request,
        &raw_response(StatusCode::OK, duplicate),
        limits(),
    ));
    let duplicate = br#"{"target":{"path":"/"},"interface":{"reference":{"scope":"/","name":"x"},"scope_ref":"identity","scope_ref":"identity"},"operation":"run#::\u64cd\u4f5c","arguments":{}}"#;
    assert!(matches!(
        decode_call_operation_metadata(duplicate, limits()).unwrap_err(),
        CodecError::InvalidJson(_)
    ));
    assert!(matches!(
        decode_call_operation_request(duplicate, &descriptor(), limits()).unwrap_err(),
        CodecError::InvalidJson(_)
    ));
}

#[test]
fn metadata_correspondence_includes_reference_and_scope_ref_but_not_wire_order() {
    let request = call(reference("/", "x"), Some("identity".into()));
    let encoded =
        encode_call_operation_request(&endpoint(), &request, &descriptor(), limits()).unwrap();
    let metadata = decode_call_operation_metadata(encoded.body(), limits()).unwrap();
    for (field, changed) in [
        ("scope_ref", json!("other")),
        ("reference", json!({"scope": "/", "name": "other"})),
    ] {
        let mut body: Json = serde_json::from_slice(encoded.body()).unwrap();
        body["interface"][field] = changed;
        assert!(
            matches!(metadata.decode_request(&serde_json::to_vec(&body).unwrap(), &descriptor(), limits()).unwrap_err(), CodecError::InvalidField { field, .. } if field == "request")
        );
    }
    let mut body: Json = serde_json::from_slice(encoded.body()).unwrap();
    body["interface"]
        .as_object_mut()
        .unwrap()
        .remove("scope_ref");
    assert!(
        metadata
            .decode_request(&serde_json::to_vec(&body).unwrap(), &descriptor(), limits())
            .is_err()
    );
}

#[test]
fn fetch_classification_preserves_transport_status_and_error_envelope_semantics() {
    let request = FetchInterfaceRequest {
        interface: reference("/", "x"),
    };
    let valid = fetch_json(json!({"scope": "/", "name": "x"}));
    for (status, kind) in [
        (
            StatusCode::CREATED,
            InvalidResponseKind::NonCanonicalSuccessStatus,
        ),
        (
            StatusCode::BAD_GATEWAY,
            InvalidResponseKind::SuccessBodyOnErrorStatus,
        ),
    ] {
        assert!(
            matches!(decode_fetch_interface_response(&request, &response(status, valid.clone()), limits()).unwrap_err(), ClientResponseError::InvalidResponse { kind: actual, .. } if actual == kind)
        );
    }
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::BAD_GATEWAY,
    ] {
        let malformed = response(status, fetch_json(json!("legacy")));
        assert!(
            matches!(decode_fetch_interface_response(&request, &malformed, limits()).unwrap_err(), ClientResponseError::TransportBinding(failure) if failure.status == status)
        );
    }
    let error = ProtocolError {
        code: ProtocolErrorCode::InterfaceNotFound,
        message: "unpublished".into(),
    };
    let mut encoded =
        encode_protocol_error_response(ProtocolInteraction::FetchInterface, &error, limits())
            .unwrap();
    assert_eq!(
        decode_fetch_interface_response(&request, &encoded, limits()).unwrap(),
        DecodedResponse::ProtocolFailure {
            error: error.clone(),
            status_mismatch: None
        }
    );
    *encoded.status_mut() = StatusCode::BAD_GATEWAY;
    assert!(
        matches!(decode_fetch_interface_response(&request, &encoded, limits()).unwrap(), DecodedResponse::ProtocolFailure { error: actual, status_mismatch: Some(_) } if actual == error)
    );
    *encoded.status_mut() = StatusCode::OK;
    assert!(matches!(
        decode_fetch_interface_response(&request, &encoded, limits()).unwrap_err(),
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::ErrorEnvelopeOnSuccessStatus,
            ..
        }
    ));
    let disallowed = response(
        StatusCode::CONFLICT,
        json!({"error": {"code": "interface_mismatch", "message": "wrong interaction"}}),
    );
    assert!(matches!(
        decode_fetch_interface_response(&request, &disallowed, limits()).unwrap_err(),
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::DisallowedProtocolCode,
            ..
        }
    ));
    let mut coded = response(StatusCode::OK, valid);
    coded
        .headers_mut()
        .insert("content-encoding", "gzip".parse().unwrap());
    assert!(
        matches!(decode_fetch_interface_response(&request, &coded, limits()).unwrap_err(), ClientResponseError::TransportBinding(failure) if failure.kind == TransportFailureKind::UnsupportedContentCoding)
    );
}

#[test]
fn call_interface_mismatch_envelope_remains_protocol_failure_with_structured_metadata() {
    let request = call(reference("/unrelated", "x"), Some("stale identity".into()));
    let error = ProtocolError {
        code: ProtocolErrorCode::InterfaceMismatch,
        message: "interface precondition".into(),
    };
    let encoded =
        encode_protocol_error_response(ProtocolInteraction::CallOperation, &error, limits())
            .unwrap();
    assert_eq!(
        decode_call_operation_response(&request, &descriptor(), &encoded, limits()).unwrap(),
        DecodedResponse::ProtocolFailure {
            error,
            status_mismatch: None
        }
    );
}
