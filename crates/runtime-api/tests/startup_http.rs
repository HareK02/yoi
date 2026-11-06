//! Actual generated RuntimeApi client error hook with bounded HTTP metadata/bytes.
use api_macros::HttpError as _;
use error_contract::{DecodeFailure, FailureCause};
use protocol::session_failure::{SessionPrincipal, SessionRegistrationCause};
use runtime_api::startup::*;
use runtime_api::{RuntimeApiClient, RuntimeApiError, client_support, server_support};
use serde_json::{Value, json};

fn failure() -> WorkerCreateFailure {
    WorkerCreateFailure::from_cause(
        StartupCause::Session(SessionRegistrationCause::denied(SessionPrincipal::Subject)),
        StartupOperationFacts {
            acceptance: CreationAcceptance::Accepted,
            execution: ExecutionOutcome::Errored,
            finalize: Some(FinalizeFacts {
                stage: FinalizeStage::TicketAssignmentBind,
                compensation: CompensationOutcome::Unknown,
            }),
            backend_compensation: Some(CompensationOutcome::Residual),
            correlation: Some(StartupCorrelation::new("backend-local-812").unwrap()),
        },
    )
    .unwrap()
}
fn cause(error: &RuntimeApiError) -> StartupCause {
    match error
        .create_failure()
        .unwrap()
        .startup()
        .primary
        .decode::<StartupCause>()
    {
        DecodeFailure::Known(received) => received.facts().cause().clone(),
        _ => panic!("expected locally typed cause"),
    }
}
fn assert_unknown(error: &RuntimeApiError, reason: StartupBoundaryReason) {
    let operation = error.create_failure().unwrap().operation();
    assert_eq!(operation.acceptance, CreationAcceptance::Unknown);
    assert_eq!(operation.execution, ExecutionOutcome::Unknown);
    assert_eq!(operation.finalize, None);
    assert_eq!(operation.backend_compensation, None);
    assert_eq!(
        cause(error),
        StartupCause::Boundary(StartupBoundaryCause { reason })
    );
}

async fn receive(status: u16, types: Vec<&'static str>, body: Vec<u8>) -> RuntimeApiError {
    use server_support::framework::{Body, Response, Router, get};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/v1/workers/{worker_id}",
        get(move || {
            let types = types.clone();
            let body = body.clone();
            async move {
                let mut response = Response::builder().status(status);
                for content_type in types {
                    response = response.header("content-type", content_type);
                }
                response.body(Body::from(body)).unwrap()
            }
        }),
    );
    let server = tokio::spawn(async move {
        server_support::framework::serve(listener, router)
            .await
            .unwrap();
    });
    let client = RuntimeApiClient::builder(&format!("http://{address}"))
        .unwrap()
        .request_timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap();
    let result = client.get_worker("test".into()).await;
    server.abort();
    match result {
        Err(client_support::ClientError::Public {
            status: actual,
            error,
        }) => {
            assert_eq!(actual.as_u16(), status);
            assert_eq!(error.status(), status);
            error
        }
        _ => panic!("expected normalized public error"),
    }
}

#[test]
fn runtime_error_projects_public_schema_fresh_and_keeps_local_facts_off_serde() {
    let failure = failure();
    let error = RuntimeApiError::new(500, "PRIVATE_CODE", "PRIVATE_MESSAGE")
        .with_create_failure(failure.clone());
    let legacy = serde_json::to_value(&error).unwrap();
    assert!(legacy.get("create_failure").is_none());
    let reparsed: RuntimeApiError = serde_json::from_value(legacy).unwrap();
    assert!(reparsed.create_failure().is_none());
    let response = error.custom_response().unwrap();
    assert_eq!(response.status, 403); // runtime boundary is not Browser's 409
    assert_eq!(response.content_type, PROBLEM_CONTENT_TYPE);
    let text = std::str::from_utf8(response.body.as_ref()).unwrap();
    assert!(!text.contains("PRIVATE_CODE"));
    assert!(!text.contains("PRIVATE_MESSAGE"));
    let decoded = match decode_startup_problem(
        response.body.as_ref(),
        response.status,
        response.content_type,
        StartupBoundary::Runtime,
    ) {
        DecodeStartupProblem::Known(failure) => failure,
        _ => panic!("expected known public-safe Runtime failure"),
    };
    assert_eq!(decoded.public_facts(), failure.public_facts());
    assert_eq!(error.into_create_failure(), Some(failure));
    let ordinary = RuntimeApiError::new(404, "not_found", "not found");
    assert!(ordinary.custom_response().is_none());
}

#[test]
fn invalid_projection_uses_static_fallback_not_legacy_raw_serialization() {
    let (mut startup, operation) = failure().into_parts();
    startup.primary = serde_json::from_value(json!("RAW_PRIVATE_CREDENTIAL")).unwrap();
    let error = RuntimeApiError::new(500, "PRIVATE_CODE", "PRIVATE_MESSAGE")
        .with_create_failure(WorkerCreateFailure::new(startup, operation).unwrap());
    let response = error.custom_response().unwrap();
    assert_eq!(response.status, 500);
    assert_eq!(
        response.body.as_ref(),
        error_contract::problem::STATIC_PROBLEM_FALLBACK
    );
    let mut legacy = RuntimeApiError::new(500, "PRIVATE_CODE", "PRIVATE_MESSAGE");
    legacy.error.startup = error
        .create_failure()
        .map(|failure| failure.startup().clone());
    assert_eq!(
        legacy.custom_response().unwrap().body.as_ref(),
        error_contract::problem::STATIC_PROBLEM_FALLBACK
    );
}

#[tokio::test]
async fn generated_client_normalizes_known_problem_and_actual_metadata() {
    let original = failure();
    let response = RuntimeApiError::new(500, "PRIVATE_CODE", "PRIVATE_MESSAGE")
        .with_create_failure(original.clone())
        .custom_response()
        .unwrap();
    let mut value: Value = serde_json::from_slice(response.body.as_ref()).unwrap();
    value["detail"] = json!("WIRE_SECRET_EXPLANATION");
    value["yoi_failure"]["explanation"] = json!("WIRE_SECRET_EXPLANATION");
    let error = receive(
        response.status,
        vec![PROBLEM_CONTENT_TYPE],
        serde_json::to_vec(&value).unwrap(),
    )
    .await;
    assert_eq!(
        error.create_failure().unwrap().public_facts(),
        original.public_facts()
    );
    assert_eq!(
        cause(&error).identity(),
        StartupCause::Session(SessionRegistrationCause::denied(SessionPrincipal::Subject))
            .identity()
    );
    assert!(!error.error.message.contains("WIRE_SECRET"));
    let browser = error
        .create_failure()
        .unwrap()
        .problem(
            StartupBoundary::Browser,
            &SessionFailureVisibility {
                can_view_cause: true,
            },
            "",
        )
        .unwrap();
    assert_eq!(browser.status, 409);
    let wrong_status = receive(
        409,
        vec![PROBLEM_CONTENT_TYPE],
        serde_json::to_vec(&value).unwrap(),
    )
    .await;
    assert_unknown(&wrong_status, StartupBoundaryReason::InvalidResponse);
}

#[tokio::test]
async fn unknown_versions_reasons_and_invalid_responses_remain_local_unknown_observations() {
    let response = failure()
        .problem(
            StartupBoundary::Runtime,
            &SessionFailureVisibility {
                can_view_cause: true,
            },
            "",
        )
        .unwrap();
    let original: Value = serde_json::from_slice(response.body()).unwrap();
    for (field, replacement, reason) in [
        (
            "version",
            json!(99),
            StartupBoundaryReason::UnsupportedVersion,
        ),
        (
            "details_version",
            json!(99),
            StartupBoundaryReason::UnsupportedVersion,
        ),
        (
            "reason",
            json!("future_reason"),
            StartupBoundaryReason::UnsupportedReason,
        ),
        (
            "category",
            json!("invalid_category"),
            StartupBoundaryReason::InvalidResponse,
        ),
    ] {
        let mut value = original.clone();
        value["yoi_failure"][field] = replacement;
        let error = receive(
            403,
            vec![PROBLEM_CONTENT_TYPE],
            serde_json::to_vec(&value).unwrap(),
        )
        .await;
        assert_unknown(&error, reason);
    }
    for content_types in [
        vec![],
        vec!["text/plain"],
        vec![PROBLEM_CONTENT_TYPE, "application/json"],
    ] {
        let error = receive(403, content_types, response.body().to_vec()).await;
        assert_unknown(&error, StartupBoundaryReason::InvalidResponse);
    }
    let error = receive(
        502,
        vec![PROBLEM_CONTENT_TYPE],
        vec![b'x'; MAX_STARTUP_PROBLEM_BYTES + 1],
    )
    .await;
    assert_unknown(&error, StartupBoundaryReason::TooLarge);
    let error = receive(
        502,
        vec![PROBLEM_CONTENT_TYPE],
        b"not JSON PRIVATE_SECRET".to_vec(),
    )
    .await;
    assert_unknown(&error, StartupBoundaryReason::InvalidResponse);
    assert!(!error.error.message.contains("PRIVATE_SECRET"));
}

#[tokio::test]
async fn legacy_errors_are_bounded_and_startup_attachments_reprojected_without_outcome_inference() {
    let ordinary = RuntimeApiError::new(404, "old_not_found", "old not found");
    let error = receive(
        404,
        vec!["application/json"],
        serde_json::to_vec(&ordinary).unwrap(),
    )
    .await;
    assert_eq!(error.error, ordinary.error);
    assert!(error.create_failure().is_none());
    let original = failure();
    let mut legacy = RuntimeApiError::new(502, "OLD_PRIVATE_CODE", "OLD_PRIVATE_MESSAGE");
    legacy.error.startup = Some(original.startup().clone());
    let error = receive(
        502,
        vec!["application/json; charset=utf-8"],
        serde_json::to_vec(&legacy).unwrap(),
    )
    .await;
    assert_eq!(
        cause(&error).identity(),
        cause(&RuntimeApiError::new(500, "unused", "unused").with_create_failure(original.clone()))
            .identity()
    );
    let failure = error.create_failure().unwrap();
    assert_eq!(failure.operation().acceptance, CreationAcceptance::Unknown);
    assert_eq!(failure.operation().execution, ExecutionOutcome::Unknown);
    assert_eq!(failure.startup().secondary, original.startup().secondary);
    assert!(!error.error.message.contains("OLD_PRIVATE"));
    let mut value = serde_json::to_value(&legacy).unwrap();
    value["error"]["startup"]["primary"] = json!("PRIVATE_MALFORMED_ATTACHMENT");
    let malformed = receive(
        502,
        vec!["application/json"],
        serde_json::to_vec(&value).unwrap(),
    )
    .await;
    assert_unknown(&malformed, StartupBoundaryReason::InvalidResponse);
    let invalid = receive(
        502,
        vec!["application/json"],
        b"{invalid PRIVATE_SECRET".to_vec(),
    )
    .await;
    assert_unknown(&invalid, StartupBoundaryReason::Decode);
    let oversized = receive(
        502,
        vec!["application/json"],
        vec![b'x'; MAX_STARTUP_PROBLEM_BYTES + 1],
    )
    .await;
    assert_unknown(&oversized, StartupBoundaryReason::TooLarge);
}
