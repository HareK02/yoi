fn root_reference(name: &str) -> wip_protocol::InterfaceReference {
    wip_protocol::InterfaceReference {
        scope: "/".into(),
        name: name.into(),
    }
}

use std::collections::BTreeMap;

use wip_client::{
    CallOutcome, Client, ClientErrorKind, ClientLimits, Completion, DiagnosticKind,
    ObservationState, OutcomeUnknownReason, SecurityContext,
};
use wip_http::http::{Response, StatusCode};
use wip_http::{
    Limits, decode_call_operation_request, decode_fetch_interface_request,
    encode_fetch_interface_response, encode_observe_response, encode_protocol_error_response,
};
use wip_protocol::{
    CallOperationRequest, FetchInterfaceRequest, FetchInterfaceResponse, INTERFACE_FORMAT_V1,
    InterfaceDescriptor, InterfaceReference, InterfaceTarget, Object, ObjectObservation,
    ObserveRequest, OperationDeclaration, ParameterDeclaration, ProtocolError, ProtocolErrorCode,
    ProtocolInteraction, ReturnDeclaration, Target, TypeExpr, Value,
};

fn limits() -> Limits {
    Limits::new(64 * 1024, 64 * 1024, 64).unwrap()
}

fn setup() -> (Client, wip_client::SessionId) {
    let mut client = Client::new(ClientLimits::new(2, 16, 16, 16, 16).unwrap(), limits());
    let session = client
        .open_session("https://example.test/wip", SecurityContext::new("subject"))
        .unwrap();
    (client, session)
}

fn response(status: StatusCode, body: &[u8]) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .body(body.to_vec())
        .unwrap()
}

fn object() -> Object {
    Object {
        name: "item".into(),
        description: None,
        interfaces: vec![root_reference("math")],
        r#ref: None,
        validator: Some(b"object-v1".to_vec()),
    }
}

fn descriptor(format: &str) -> InterfaceDescriptor {
    InterfaceDescriptor {
        format: format.into(),
        documentation: None,
        types: vec![],
        operations: vec![OperationDeclaration {
            name: "calculate".into(),
            documentation: None,
            parameters: vec![ParameterDeclaration {
                name: "value".into(),
                required: true,
                documentation: None,
                r#type: TypeExpr::Integer,
            }],
            returns: ReturnDeclaration {
                documentation: None,
                r#type: TypeExpr::Integer,
            },
        }],
    }
}

fn reference(scope: &str, name: &str) -> InterfaceReference {
    InterfaceReference {
        scope: scope.into(),
        name: name.into(),
    }
}

fn observe(client: &mut Client, session: &wip_client::SessionId, path: &str, value: Object) {
    let request = client.refresh_object(session, path).unwrap();
    client
        .complete(
            request.id,
            encode_observe_response(
                &ObserveRequest {
                    path: path.into(),
                    depth: 0,
                },
                &ObjectObservation {
                    object: value,
                    children: None,
                },
                limits(),
            )
            .unwrap(),
        )
        .unwrap();
}

fn fetch(
    client: &mut Client,
    session: &wip_client::SessionId,
    value: FetchInterfaceResponse,
) -> Completion {
    let request = client
        .prepare_interface(session, value.interface.clone())
        .unwrap();
    let logical = FetchInterfaceRequest {
        interface: value.interface.clone(),
    };
    assert_eq!(
        decode_fetch_interface_request(request.request.body(), limits()).unwrap(),
        logical
    );
    client
        .complete(
            request.id,
            encode_fetch_interface_response(&logical, &value, limits()).unwrap(),
        )
        .unwrap()
}

#[test]
fn observe_accepts_same_name_interfaces_in_different_scopes() {
    let (mut client, session) = setup();
    let request = client.refresh_object(&session, "/item").unwrap();
    let completion = client
        .complete(
            request.id,
            response(
                StatusCode::OK,
                br#"{"object":{"name":"item","interfaces":[{"scope":"/","name":"math"},{"scope":"/item","name":"math"}]}}"#,
            ),
        )
        .unwrap();
    let expected = vec![root_reference("math"), reference("/item", "math")];
    assert!(matches!(
        completion,
        Completion::Object(observation)
            if observation.state == ObservationState::Fresh
                && observation.object.as_ref().unwrap().interfaces == expected
    ));
    let cached = client.object(&session, "/item").unwrap();
    assert_eq!(cached.state, ObservationState::Fresh);
    assert_eq!(cached.object.unwrap().interfaces, expected);
}

#[test]
fn same_name_different_scopes_have_independent_requests_and_cached_descriptors() {
    let (mut client, session) = setup();
    let root = root_reference("math");
    let scoped = reference("/area", "math");
    let root_request = client
        .ensure_interface(&session, root.clone())
        .unwrap()
        .unwrap();
    let scoped_request = client
        .ensure_interface(&session, scoped.clone())
        .unwrap()
        .unwrap();
    assert_ne!(root_request.id, scoped_request.id);
    assert_eq!(
        decode_fetch_interface_request(root_request.request.body(), limits()).unwrap(),
        FetchInterfaceRequest {
            interface: root.clone(),
        }
    );
    assert_eq!(
        decode_fetch_interface_request(scoped_request.request.body(), limits()).unwrap(),
        FetchInterfaceRequest {
            interface: scoped.clone(),
        }
    );
    for selected in [&root, &scoped] {
        assert!(
            client
                .ensure_interface(&session, selected.clone())
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(client.in_flight_len(), 2);

    // Reverse completion order must not supersede the other scope's request.
    for (request, selected, documentation, scope_ref, validator) in [
        (
            scoped_request,
            &scoped,
            "area math",
            Some("area-object"),
            Some(b"area-v1".to_vec()),
        ),
        (root_request, &root, "root math", None, None),
    ] {
        let mut expected_descriptor = descriptor(INTERFACE_FORMAT_V1);
        expected_descriptor.documentation = Some(wip_protocol::Documentation {
            summary: documentation.into(),
            details: None,
        });
        let value = FetchInterfaceResponse {
            interface: selected.clone(),
            scope_ref: scope_ref.map(str::to_owned),
            descriptor: expected_descriptor.clone(),
            validator: validator.clone(),
        };
        let completion = client
            .complete(
                request.id,
                encode_fetch_interface_response(
                    &FetchInterfaceRequest {
                        interface: selected.clone(),
                    },
                    &value,
                    limits(),
                )
                .unwrap(),
            )
            .unwrap();
        assert!(matches!(completion, Completion::Interface(_)));
        let cached = client.interface(&session, selected).unwrap();
        assert_eq!(cached.reference, *selected);
        assert_eq!(cached.descriptor, Some(expected_descriptor));
        assert_eq!(cached.scope_ref, value.scope_ref);
        assert_eq!(cached.validator, validator);
        assert_eq!(cached.state, ObservationState::Fresh);
    }
    for selected in [&root, &scoped] {
        assert!(
            client
                .ensure_interface(&session, selected.clone())
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(client.in_flight_len(), 0);
    assert_eq!(
        client
            .interface(&session, &root)
            .unwrap()
            .descriptor
            .as_ref()
            .unwrap()
            .documentation
            .as_ref()
            .map(|documentation| documentation.summary.as_str()),
        Some("root math")
    );
    assert_eq!(
        client
            .interface(&session, &scoped)
            .unwrap()
            .descriptor
            .as_ref()
            .unwrap()
            .documentation
            .as_ref()
            .map(|documentation| documentation.summary.as_str()),
        Some("area math")
    );
}

#[test]
fn interface_completion_retains_optional_scope_ref_independently_of_validator() {
    for scope_ref in [None, Some("opaque scope identity / not a path")] {
        for validator in [None, Some(b"interface-v1".to_vec())] {
            let (mut client, session) = setup();
            let selected = reference("/area", "math");
            let expected = wip_client::InterfaceObservation {
                reference: selected.clone(),
                descriptor: Some(descriptor(INTERFACE_FORMAT_V1)),
                scope_ref: scope_ref.map(str::to_owned),
                validator: validator.clone(),
                state: ObservationState::Fresh,
            };
            let completion = fetch(
                &mut client,
                &session,
                FetchInterfaceResponse {
                    interface: selected.clone(),
                    descriptor: expected.descriptor.clone().unwrap(),
                    scope_ref: expected.scope_ref.clone(),
                    validator,
                },
            );
            assert_eq!(completion, Completion::Interface(expected.clone()));
            assert_eq!(client.interface(&session, &selected), Some(&expected));
        }
    }
}

#[test]
fn call_membership_requires_the_complete_scope_and_name_pair() {
    let (mut client, session) = setup();
    let member = reference("/item", "math");
    let same_name = root_reference("math");
    let mut value = object();
    value.interfaces = vec![member.clone()];
    observe(&mut client, &session, "/item", value);
    for selected in [&member, &same_name] {
        fetch(
            &mut client,
            &session,
            FetchInterfaceResponse {
                interface: selected.clone(),
                descriptor: descriptor(INTERFACE_FORMAT_V1),
                scope_ref: None,
                validator: None,
            },
        );
    }
    let arguments = BTreeMap::from([("value".into(), Value::Integer(2))]);
    assert_eq!(
        client
            .prepare_call(
                &session,
                "/item",
                &same_name,
                "calculate",
                arguments.clone(),
            )
            .unwrap_err()
            .kind,
        ClientErrorKind::InvalidRequest
    );
    assert_eq!(client.in_flight_len(), 0);
    let call = client
        .prepare_call(&session, "/item", &member, "calculate", arguments)
        .unwrap();
    assert_eq!(
        decode_call_operation_request(
            call.request.body(),
            &descriptor(INTERFACE_FORMAT_V1),
            limits(),
        )
        .unwrap()
        .interface
        .reference,
        member
    );
}

#[test]
fn calls_copy_exact_observed_references_metadata_and_validators_for_root_ancestor_and_self_scopes()
{
    for (path, scope) in [
        ("/", "/"),
        ("/item", "/"),
        ("/item", "/item"),
        ("/area/item", "/area"),
        ("/area/item", "/area/item"),
    ] {
        for scope_ref in [None, Some("opaque scope identity / not a path")] {
            for interface_validator in [None, Some(b"interface-v1".to_vec())] {
                for object_validator in [None, Some(b"object-v1".to_vec())] {
                    let (mut client, session) = setup();
                    let selected = reference(scope, "math");
                    let expected_descriptor = descriptor(INTERFACE_FORMAT_V1);
                    let mut value = object();
                    value.name = path.rsplit('/').next().unwrap().into();
                    value.interfaces = vec![selected.clone()];
                    value.validator = object_validator.clone();
                    if path == scope {
                        value.r#ref = scope_ref.map(str::to_owned);
                    }
                    observe(&mut client, &session, path, value.clone());
                    fetch(
                        &mut client,
                        &session,
                        FetchInterfaceResponse {
                            interface: selected.clone(),
                            descriptor: expected_descriptor.clone(),
                            scope_ref: scope_ref.map(str::to_owned),
                            validator: interface_validator.clone(),
                        },
                    );
                    let expected_interface = client.interface(&session, &selected).unwrap().clone();
                    let expected_object = client.object(&session, path).unwrap();
                    let arguments = BTreeMap::from([("value".into(), Value::Integer(2))]);
                    let expected = CallOperationRequest {
                        target: Target {
                            path: path.into(),
                            validator: object_validator,
                        },
                        interface: InterfaceTarget {
                            reference: selected.clone(),
                            scope_ref: scope_ref.map(str::to_owned),
                            validator: interface_validator.clone(),
                        },
                        operation: "calculate".into(),
                        arguments: arguments.clone(),
                    };
                    let call = client
                        .prepare_call(&session, path, &selected, "calculate", arguments)
                        .unwrap();
                    assert_eq!(
                        decode_call_operation_request(
                            call.request.body(),
                            &expected_descriptor,
                            limits(),
                        )
                        .unwrap(),
                        expected
                    );
                    let context = client.pending_call_context(call.id).unwrap();
                    assert_eq!(context.request(), &expected);
                    assert_eq!(context.interface(), &expected_interface);
                    assert_eq!(context.object(), &expected_object);
                    assert_eq!(context.object().object.as_ref(), Some(&value));
                    assert_eq!(context.descriptor(), &expected_descriptor);
                    assert_eq!(context.operation(), &expected_descriptor.operations[0]);
                    assert_eq!(context.session(), &session);
                }
            }
        }
    }
}

#[test]
fn malformed_json_unknown_fields_duplicates_and_invalid_success_are_invalid_response() {
    let cases: &[&[u8]] = &[
        b"{not json",
        br#"{"name":"item","interfaces":[],"unknown":true}"#,
        br#"{"name":"item","interfaces":[{"scope":"/","name":"math"},{"scope":"/","name":"math"}]}"#,
        br#"{"name":"wrong","interfaces":[]}"#,
    ];
    for body in cases {
        let (mut client, session) = setup();
        let request = client.refresh_observed(&session, "/item", 0).unwrap();
        let error = client
            .complete(request.id, response(StatusCode::OK, body))
            .unwrap_err();
        assert_eq!(error.kind, ClientErrorKind::InvalidResponse);
        assert!(matches!(
            client.object(&session, "/item").unwrap().state,
            ObservationState::Error(_)
        ));
    }
}

#[test]
fn disallowed_code_and_status_success_contradictions_are_invalid_response() {
    let cases = [
        (
            StatusCode::NOT_FOUND,
            br#"{"error":{"code":"interface_not_found","message":"wrong"}}"#.as_slice(),
        ),
        (
            StatusCode::OK,
            br#"{"error":{"code":"not_found","message":"wrong status"}}"#.as_slice(),
        ),
        (
            StatusCode::CREATED,
            br#"{"name":"item","interfaces":[]}"#.as_slice(),
        ),
    ];
    for (status, body) in cases {
        let (mut client, session) = setup();
        let request = client.refresh_observed(&session, "/item", 0).unwrap();
        assert_eq!(
            client
                .complete(request.id, response(status, body))
                .unwrap_err()
                .kind,
            ClientErrorKind::InvalidResponse
        );
    }
}

#[test]
fn valid_error_envelope_is_authoritative_and_status_mismatch_is_diagnostic() {
    let (mut client, session) = setup();
    let request = client.refresh_observed(&session, "/item", 0).unwrap();
    let error = ProtocolError {
        code: ProtocolErrorCode::NotFound,
        message: "missing".into(),
    };
    let mut encoded =
        encode_protocol_error_response(ProtocolInteraction::Observe, &error, limits()).unwrap();
    *encoded.status_mut() = StatusCode::IM_A_TEAPOT;

    assert_eq!(
        client.complete(request.id, encoded).unwrap(),
        Completion::ProtocolFailure(error)
    );
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(
        client.diagnostics(&session).unwrap().back().unwrap().kind,
        DiagnosticKind::HttpStatusMismatch
    );
}

#[test]
fn resource_envelope_on_413_uses_code_and_records_canonical_422_mismatch() {
    let (mut client, session) = setup();
    let request = client.refresh_observed(&session, "/item", 0).unwrap();
    let error = ProtocolError {
        code: ProtocolErrorCode::ResourceLimitExceeded,
        message: "logical observation limit exceeded".into(),
    };
    let mut encoded =
        encode_protocol_error_response(ProtocolInteraction::Observe, &error, limits()).unwrap();
    *encoded.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;

    assert_eq!(
        client.complete(request.id, encoded).unwrap(),
        Completion::ProtocolFailure(error)
    );
    assert_eq!(
        client.diagnostics(&session).unwrap().back().unwrap().kind,
        DiagnosticKind::HttpStatusMismatch
    );
}

#[test]
fn non_success_without_valid_envelope_remains_transport_failure() {
    for status in [StatusCode::BAD_GATEWAY, StatusCode::PAYLOAD_TOO_LARGE] {
        let (mut client, session) = setup();
        let request = client.refresh_observed(&session, "/item", 0).unwrap();
        let error = client
            .complete(request.id, response(status, b"infrastructure failure"))
            .unwrap_err();
        assert_eq!(error.kind, ClientErrorKind::Transport);
    }
}

#[test]
fn unsupported_descriptor_format_is_distinct_from_invalid_response() {
    let (mut client, session) = setup();
    let request = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    let logical = FetchInterfaceRequest {
        interface: root_reference("math"),
    };
    let value = FetchInterfaceResponse {
        scope_ref: None,
        interface: root_reference("math"),
        descriptor: descriptor("future-format/2"),
        validator: Some(b"v2".to_vec()),
    };
    let encoded = encode_fetch_interface_response(&logical, &value, limits()).unwrap();
    let error = client.complete(request.id, encoded).unwrap_err();
    assert_eq!(error.kind, ClientErrorKind::UnsupportedDescriptorFormat);
}

#[test]
fn malformed_call_success_after_dispatch_is_outcome_unknown_and_stales_object() {
    let (mut client, session) = setup();
    let observation = client.refresh_observed(&session, "/item", 0).unwrap();
    client
        .complete(
            observation.id,
            encode_observe_response(
                &ObserveRequest {
                    path: "/item".into(),
                    depth: 0,
                },
                &ObjectObservation {
                    object: object(),
                    children: None,
                },
                limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let interface = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    client
        .complete(
            interface.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &FetchInterfaceResponse {
                    scope_ref: None,
                    interface: root_reference("math"),
                    descriptor: descriptor(INTERFACE_FORMAT_V1),
                    validator: None,
                },
                limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            BTreeMap::from([("value".into(), Value::Integer(2))]),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();

    let completion = client
        .complete(
            call.id,
            response(
                StatusCode::OK,
                br#"{"result":"not an integer","unknown":true}"#,
            ),
        )
        .unwrap();
    assert!(matches!(
        completion,
        Completion::Call(record)
            if matches!(
                &record.outcome,
                CallOutcome::Unknown {
                    reason: OutcomeUnknownReason::ResponseDecode,
                    failure: Some(failure),
                } if failure.kind == ClientErrorKind::InvalidResponse
            )
    ));
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(client.in_flight_len(), 0);
}

#[test]
fn plain_413_after_call_dispatch_is_unknown_transport_outcome() {
    let (mut client, session) = setup();
    let observation = client.refresh_observed(&session, "/item", 0).unwrap();
    client
        .complete(
            observation.id,
            encode_observe_response(
                &ObserveRequest {
                    path: "/item".into(),
                    depth: 0,
                },
                &ObjectObservation {
                    object: object(),
                    children: None,
                },
                limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let interface = client
        .prepare_interface(&session, root_reference("math"))
        .unwrap();
    client
        .complete(
            interface.id,
            encode_fetch_interface_response(
                &FetchInterfaceRequest {
                    interface: root_reference("math"),
                },
                &FetchInterfaceResponse {
                    scope_ref: None,
                    interface: root_reference("math"),
                    descriptor: descriptor(INTERFACE_FORMAT_V1),
                    validator: None,
                },
                limits(),
            )
            .unwrap(),
        )
        .unwrap();
    let call = client
        .prepare_call(
            &session,
            "/item",
            &root_reference("math"),
            "calculate",
            BTreeMap::from([("value".into(), Value::Integer(2))]),
        )
        .unwrap();
    client.mark_dispatched(call.id).unwrap();

    let completion = client
        .complete(
            call.id,
            Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Vec::<u8>::new())
                .unwrap(),
        )
        .unwrap();
    assert!(matches!(
        completion,
        Completion::Call(record)
            if matches!(
                &record.outcome,
                CallOutcome::Unknown {
                    reason: OutcomeUnknownReason::ResponseDecode,
                    failure: Some(failure),
                } if failure.kind == ClientErrorKind::Transport
            )
    ));
    assert_eq!(
        client.object(&session, "/item").unwrap().state,
        ObservationState::Stale
    );
    assert_eq!(client.in_flight_len(), 0);
}
