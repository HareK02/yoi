use wip_http::http::{Response, StatusCode};
use wip_http::wip_protocol::{
    ObserveRequest, ProtocolError, ProtocolErrorCode, ProtocolInteraction,
};
use wip_http::{
    ClientResponseError, DecodedResponse, InvalidResponseKind, Limits, TransportFailureKind,
    decode_observe_response, encode_protocol_error_response, error_code_name, is_json_content_type,
    status_for_error,
};

fn limits() -> Limits {
    Limits::new(64 * 1024, 64 * 1024, 32).unwrap()
}

fn request() -> ObserveRequest {
    ObserveRequest {
        path: "/".into(),
        depth: 0,
    }
}

fn response(status: StatusCode, body: impl Into<Vec<u8>>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .body(body.into())
        .unwrap()
}

#[test]
fn content_type_accepts_only_utf8_json_syntax() {
    assert!(is_json_content_type("application/json"));
    assert!(is_json_content_type("Application/JSON; Charset=\"UTF-8\""));
    assert!(!is_json_content_type("application/json; charset=utf-16"));
    assert!(!is_json_content_type("application/json; charset=\"utf-8"));
    assert!(!is_json_content_type("application/json; charset=utf-8\""));
    assert!(!is_json_content_type("application/json; profile=other"));
}

#[test]
fn every_protocol_code_has_stable_name_and_status() {
    let expected = [
        (
            ProtocolErrorCode::InvalidRequest,
            "invalid_request",
            StatusCode::BAD_REQUEST,
        ),
        (
            ProtocolErrorCode::NotFound,
            "not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            ProtocolErrorCode::InterfaceNotFound,
            "interface_not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            ProtocolErrorCode::InterfaceMismatch,
            "interface_mismatch",
            StatusCode::CONFLICT,
        ),
        (
            ProtocolErrorCode::OperationNotFound,
            "operation_not_found",
            StatusCode::NOT_FOUND,
        ),
        (
            ProtocolErrorCode::InvalidArguments,
            "invalid_arguments",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ProtocolErrorCode::ValidatorRequired,
            "validator_required",
            StatusCode::PRECONDITION_REQUIRED,
        ),
        (
            ProtocolErrorCode::ValidatorMismatch,
            "validator_mismatch",
            StatusCode::PRECONDITION_FAILED,
        ),
        (
            ProtocolErrorCode::InterfaceValidatorRequired,
            "interface_validator_required",
            StatusCode::PRECONDITION_REQUIRED,
        ),
        (
            ProtocolErrorCode::InterfaceValidatorMismatch,
            "interface_validator_mismatch",
            StatusCode::PRECONDITION_FAILED,
        ),
        (
            ProtocolErrorCode::PermissionDenied,
            "permission_denied",
            StatusCode::FORBIDDEN,
        ),
        (
            ProtocolErrorCode::ResourceLimitExceeded,
            "resource_limit_exceeded",
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            ProtocolErrorCode::Internal,
            "internal",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            ProtocolErrorCode::OperationOutcomeUnknown,
            "operation_outcome_unknown",
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ];
    assert_eq!(expected.len(), ProtocolErrorCode::ALL.len());
    for (code, name, status) in expected {
        assert_eq!(error_code_name(code), name);
        assert_eq!(status_for_error(code), status);
    }
}

#[test]
fn logical_resource_limit_uses_only_422_and_bare_413_is_transport_failure() {
    let error = ProtocolError {
        code: ProtocolErrorCode::ResourceLimitExceeded,
        message: "logical observation limit exceeded".into(),
    };
    let encoded =
        encode_protocol_error_response(ProtocolInteraction::Observe, &error, limits()).unwrap();
    assert_eq!(encoded.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        decode_observe_response(&request(), &encoded, limits()).unwrap(),
        DecodedResponse::ProtocolFailure {
            error: error.clone(),
            status_mismatch: None,
        }
    );

    let mut noncanonical = encoded;
    *noncanonical.status_mut() = StatusCode::PAYLOAD_TOO_LARGE;
    assert_eq!(
        decode_observe_response(&request(), &noncanonical, limits()).unwrap(),
        DecodedResponse::ProtocolFailure {
            error,
            status_mismatch: Some(wip_http::StatusMismatch {
                actual: StatusCode::PAYLOAD_TOO_LARGE,
                expected: StatusCode::UNPROCESSABLE_ENTITY,
            }),
        }
    );

    let bare_413 = Response::builder()
        .status(StatusCode::PAYLOAD_TOO_LARGE)
        .body(Vec::<u8>::new())
        .unwrap();
    assert!(matches!(
        decode_observe_response(&request(), &bare_413, limits()).unwrap_err(),
        ClientResponseError::TransportBinding(failure)
            if failure.status == StatusCode::PAYLOAD_TOO_LARGE
                && failure.kind == TransportFailureKind::HttpStatus
    ));
}

#[test]
fn rate_limit_status_without_protocol_envelope_stays_transport_failure() {
    let response = Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .body(b"rate limited".to_vec())
        .unwrap();
    assert!(matches!(
        decode_observe_response(&request(), &response, limits()).unwrap_err(),
        ClientResponseError::TransportBinding(failure)
            if failure.status == StatusCode::TOO_MANY_REQUESTS
                && failure.kind == TransportFailureKind::HttpStatus
    ));
}

#[test]
fn unsupported_response_content_coding_is_transport_failure() {
    let mut encoded = encode_protocol_error_response(
        ProtocolInteraction::Observe,
        &ProtocolError {
            code: ProtocolErrorCode::NotFound,
            message: "missing".into(),
        },
        limits(),
    )
    .unwrap();
    encoded
        .headers_mut()
        .insert("content-encoding", "gzip".parse().unwrap());
    assert!(matches!(
        decode_observe_response(&request(), &encoded, limits()).unwrap_err(),
        ClientResponseError::TransportBinding(failure)
            if failure.status == StatusCode::NOT_FOUND
                && failure.kind == TransportFailureKind::UnsupportedContentCoding
    ));

    let success = response(StatusCode::OK, br#"{"object":{"name":"","interfaces":[]}}"#);
    let mut identity = success.clone();
    identity
        .headers_mut()
        .insert("content-encoding", "identity".parse().unwrap());
    assert!(matches!(
        decode_observe_response(&request(), &identity, limits()).unwrap(),
        DecodedResponse::Success(_)
    ));

    let mut unsupported = success;
    unsupported
        .headers_mut()
        .insert("content-encoding", "br".parse().unwrap());
    assert!(matches!(
        decode_observe_response(&request(), &unsupported, limits()).unwrap_err(),
        ClientResponseError::TransportBinding(failure)
            if failure.status == StatusCode::OK
                && failure.kind == TransportFailureKind::UnsupportedContentCoding
    ));
}

#[test]
fn valid_success_body_requires_exactly_http_200() {
    let success_body = br#"{"object":{"name":"","interfaces":[]}}"#;
    for status in [StatusCode::CREATED, StatusCode::ACCEPTED] {
        assert!(matches!(
            decode_observe_response(&request(), &response(status, success_body), limits())
                .unwrap_err(),
            ClientResponseError::InvalidResponse {
                kind: InvalidResponseKind::NonCanonicalSuccessStatus,
                ..
            }
        ));
    }
}

#[test]
fn server_encoder_rejects_codes_disallowed_for_the_interaction() {
    let error = ProtocolError {
        code: ProtocolErrorCode::InterfaceNotFound,
        message: "not valid for observe".into(),
    };
    assert!(
        encode_protocol_error_response(ProtocolInteraction::Observe, &error, limits()).is_err()
    );
}

#[test]
fn valid_envelope_code_is_authoritative_when_status_mismatches() {
    let error = ProtocolError {
        code: ProtocolErrorCode::NotFound,
        message: "missing".into(),
    };
    let mut encoded =
        encode_protocol_error_response(ProtocolInteraction::Observe, &error, limits()).unwrap();
    *encoded.status_mut() = StatusCode::IM_A_TEAPOT;

    assert_eq!(
        decode_observe_response(&request(), &encoded, limits()).unwrap(),
        DecodedResponse::ProtocolFailure {
            error,
            status_mismatch: Some(wip_http::StatusMismatch {
                actual: StatusCode::IM_A_TEAPOT,
                expected: StatusCode::NOT_FOUND,
            }),
        }
    );
}

#[test]
fn envelope_and_status_contradictions_are_invalid_responses() {
    let error_body = br#"{"error":{"code":"not_found","message":"missing"}}"#;
    let on_success =
        decode_observe_response(&request(), &response(StatusCode::OK, error_body), limits())
            .unwrap_err();
    assert!(matches!(
        on_success,
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::ErrorEnvelopeOnSuccessStatus,
            ..
        }
    ));

    let success_body = br#"{"object":{"name":"","interfaces":[]}}"#;
    let on_error = decode_observe_response(
        &request(),
        &response(StatusCode::BAD_GATEWAY, success_body),
        limits(),
    )
    .unwrap_err();
    assert!(matches!(
        on_error,
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::SuccessBodyOnErrorStatus,
            ..
        }
    ));
}

#[test]
fn disallowed_protocol_code_is_invalid_instead_of_reclassified() {
    let body = br#"{"error":{"code":"interface_not_found","message":"wrong interaction"}}"#;
    let error =
        decode_observe_response(&request(), &response(StatusCode::NOT_FOUND, body), limits())
            .unwrap_err();
    assert!(matches!(
        error,
        ClientResponseError::InvalidResponse {
            kind: InvalidResponseKind::DisallowedProtocolCode,
            ..
        }
    ));
}

#[test]
fn proxy_authentication_and_endpoint_failures_stay_transport_failures() {
    let cases = [
        (
            StatusCode::BAD_GATEWAY,
            TransportFailureKind::HttpStatus,
            b"proxy failure".as_slice(),
        ),
        (
            StatusCode::UNAUTHORIZED,
            TransportFailureKind::AuthenticationRequired,
            b"login required".as_slice(),
        ),
        (
            StatusCode::FORBIDDEN,
            TransportFailureKind::EndpointForbidden,
            b"denied".as_slice(),
        ),
    ];
    for (status, expected_kind, body) in cases {
        let response = Response::builder()
            .status(status)
            .body(body.to_vec())
            .unwrap();
        let error = decode_observe_response(&request(), &response, limits()).unwrap_err();
        assert!(matches!(
            error,
            ClientResponseError::TransportBinding(failure)
                if failure.status == status && failure.kind == expected_kind
        ));
    }
}

#[test]
fn malformed_non_success_envelope_is_transport_binding_failure() {
    let malformed = response(
        StatusCode::BAD_GATEWAY,
        br#"{"error":{"code":"not_found","message":null}}"#,
    );
    assert!(matches!(
        decode_observe_response(&request(), &malformed, limits()).unwrap_err(),
        ClientResponseError::TransportBinding(_)
    ));
}
