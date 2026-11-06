//! Connect generated Axum responses to generated Reqwest opt-in decoding over isolated loopback.
//! Domain disclosure is a fixture; HTTP encoding, bounded reads and metadata checks are production.
#![cfg(all(feature = "axum", feature = "reqwest", feature = "error-contract"))]

use std::{sync::Arc, time::Duration};

use api_macros::{
    HttpErrorResponse, api,
    reqwest::{ClientError, ClientFailure, HttpClientError, ReceivedResponse},
};
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use error_contract::{
    Browser, Budget, Category, ContractError, DecodeFailure, Disclosure, FailureCause,
    FailureFacts, FailureIdentity, ProjectFor, ProjectedFailure, WireDetails, project_failure,
};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

#[derive(Serialize, Deserialize)]
pub struct Payload {
    value: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Denied {
    required_operation: String,
}
impl FailureCause for Denied {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "subject",
            reason: "operation_denied",
            category: Category::Authorization,
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        if self.required_operation == "session.record" {
            Ok(())
        } else {
            Err(ContractError::InvalidDetails)
        }
    }
}
impl WireDetails for Denied {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "subject" && reason == "operation_denied" && version == 1
    }
}
impl ProjectFor<Browser> for Denied {
    type Authorization = ();
    type Details = Self;
    fn disclose(&self, _: &FailureFacts<Self>, _: &()) -> Result<Disclosure<Self>, ContractError> {
        Ok(Disclosure::new(
            self.clone(),
            "Subject cannot record a session.",
        ))
    }
}
struct Boundary;
impl api_macros::http::ApiBoundary<Denied> for Boundary {
    fn classify(&self, _: &Denied) -> error_contract::problem::ProblemClass {
        error_contract::problem::ProblemClass::authorization_denied()
    }
}
fn budget() -> Budget {
    Budget::new(4096, 4096)
}
fn projected() -> ProjectedFailure<Browser, Denied> {
    project_failure(
        &FailureFacts::new(Denied {
            required_operation: "session.record".into(),
        })
        .unwrap(),
        &(),
    )
    .unwrap()
}
fn encoded() -> HttpErrorResponse {
    api_macros::http::problem_response(&projected(), &Boundary, "urn:yoi:failure:local-1", budget())
        .unwrap()
        .into()
}

// Not Clone: the original must be retained without duplicating a domain error.
#[derive(Serialize)]
pub struct ServerError {
    legacy: &'static str,
    #[serde(skip)]
    mode: u8,
}
impl api_macros::HttpError for ServerError {
    fn status_code(&self) -> u16 {
        422
    }
    fn custom_response(&self) -> Option<HttpErrorResponse> {
        match self.mode {
            0 => Some(encoded()),
            1 => None,
            mode => Some(HttpErrorResponse {
                status: if mode == 2 { 200 } else { 403 },
                content_type: if mode == 3 {
                    "bad\ncontent-type"
                } else {
                    "application/problem+json"
                },
                body: b"private-hook-body".to_vec().into(),
            }),
        }
    }
}
impl api_macros::HttpRequestError for ServerError {
    fn from_request_rejection(_: u16, _: String) -> Self {
        Self {
            legacy: "never serialize",
            mode: 0,
        }
    }
}
#[api(axum)]
pub trait HookServer {
    #[get("/problem", status = 200, error_status = 422)]
    async fn problem(&self) -> Result<Payload, ServerError>;
    #[post("/reject", error_status = 422, normalize_body_errors = true)]
    async fn reject(&self, #[body] input: Payload) -> Result<Payload, ServerError>;
    #[post("/reject-binary", error_status = 422, normalize_body_errors = true)]
    async fn reject_binary(
        &self,
        #[binary] input: api_macros::BinaryBody,
    ) -> Result<Payload, ServerError>;
}
pub struct Service;
impl HookServer for Service {
    async fn problem(&self) -> Result<Payload, ServerError> {
        Err(ServerError {
            legacy: "never serialize",
            mode: 0,
        })
    }
    async fn reject(&self, input: Payload) -> Result<Payload, ServerError> {
        Ok(input)
    }
    async fn reject_binary(&self, _input: api_macros::BinaryBody) -> Result<Payload, ServerError> {
        Ok(Payload { value: "ok".into() })
    }
}

// Neither Deserialize nor HttpError: client hooks impose no server or serde error bounds.
pub struct DecodedError {
    operation: String,
}
impl HttpClientError for DecodedError {
    fn decode_error(response: &ReceivedResponse) -> Result<Self, ClientFailure> {
        let status = response.status().as_u16();
        match response.decode_problem::<Denied>(&Boundary, budget()) {
            DecodeFailure::Known(failure) => Ok(Self {
                operation: failure.facts().cause().required_operation.clone(),
            }),
            DecodeFailure::Unsupported(_) => {
                Err(ClientFailure::UnsupportedErrorResponse { status })
            }
            DecodeFailure::Corrupt(ContractError::TooLarge) => {
                Err(ClientFailure::ResponseTooLarge {
                    limit: budget().bytes(),
                })
            }
            DecodeFailure::Corrupt(_) => Err(ClientFailure::ErrorResponseDecode { status }),
        }
    }
}
#[api(reqwest)]
pub trait HookClient {
    #[get("/problem", status = 200, error_status = 422, client_error_hook = true)]
    async fn problem(&self) -> Result<Payload, DecodedError>;
    #[get(
        "/wire/{mode}",
        status = 200,
        error_status = 422,
        client_error_hook = true
    )]
    async fn wire(&self, mode: String) -> Result<Payload, DecodedError>;
    #[get("/legacy", error_status = 422)]
    async fn legacy(&self) -> Result<Payload, LegacyClientError>;
}

// A false method flag cannot disable a trait-wide opt-in; the selection is logical OR.
#[api(reqwest, client_error_hook)]
pub trait TraitHookClient {
    #[get("/problem", client_error_hook = false)]
    async fn problem(&self) -> Result<Payload, DecodedError>;
}

// Client-only legacy errors must still compile and decode without either hook trait.
#[derive(Deserialize)]
pub struct LegacyClientError {
    legacy: String,
}
#[api(reqwest)]
pub trait LegacyClient {
    #[get("/legacy", error_status = 422)]
    async fn legacy(&self) -> Result<Payload, LegacyClientError>;
}

#[tokio::test]
async fn generated_server_hook_overrides_wire_encoding_and_retains_arc_for_errors_and_rejections() {
    let app = HookServerAxum::router(Service).layer(axum::extract::DefaultBodyLimit::max(32));
    for request in [
        Request::builder()
            .uri("/problem")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method("POST")
            .uri("/reject")
            .header("content-type", "application/json")
            .body(Body::from("{"))
            .unwrap(),
        Request::builder()
            .method("POST")
            .uri("/reject-binary")
            .body(Body::from(vec![0; 64]))
            .unwrap(),
    ] {
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        assert_eq!(
            response
                .extensions()
                .get::<Arc<ServerError>>()
                .unwrap()
                .mode,
            0
        );
        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["status"], 403);
        assert_eq!(
            value["yoi_failure"]["details"]["required_operation"],
            "session.record"
        );
        assert!(!String::from_utf8_lossy(&bytes).contains("never serialize"));
    }
}

#[tokio::test]
async fn absent_hook_preserves_json_and_invalid_hook_never_falls_back_to_legacy() {
    let legacy = api_macros::axum::http_error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        ServerError {
            legacy: "legacy-value",
            mode: 1,
        },
    );
    assert_eq!(legacy.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(legacy.headers()["content-type"], "application/json");
    assert!(legacy.extensions().get::<Arc<ServerError>>().is_some());
    let body = to_bytes(legacy.into_body(), 4096).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["legacy"],
        "legacy-value"
    );
    for mode in [2, 3] {
        let response = api_macros::axum::http_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            ServerError {
                legacy: "private",
                mode,
            },
        );
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.extensions().get::<Arc<ServerError>>().is_none());
        assert!(
            to_bytes(response.into_body(), 4096)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn problem_conversion_falls_back_without_exceeding_byte_or_depth_budgets() {
    for (instance, budget) in [
        ("invalid URI", budget()),
        ("", Budget::new(256, 256)),
        ("", budget().with_depth(1, 1)),
    ] {
        let response =
            api_macros::http::problem_response(&projected(), &Boundary, instance, budget).unwrap();
        assert_eq!(response.status, 500);
        assert_eq!(
            response.body(),
            error_contract::problem::STATIC_PROBLEM_FALLBACK
        );
        assert!(response.body().len() <= budget.bytes());
    }
    assert!(matches!(
        api_macros::http::problem_response(&projected(), &Boundary, "", Budget::new(1, 1)),
        Err(ContractError::TooLarge)
    ));
}

async fn wire(axum::extract::Path(mode): axum::extract::Path<String>) -> axum::response::Response {
    let mut value: serde_json::Value = serde_json::from_slice(encoded().body.as_ref()).unwrap();
    let mut status = StatusCode::FORBIDDEN;
    let mut content_type = "application/problem+json; charset=utf-8";
    match mode.as_str() {
        "unknown" => value["yoi_failure"]["reason"] = "future_reason".into(),
        "version" => value["yoi_failure"]["version"] = 99.into(),
        "status" => status = StatusCode::CONFLICT,
        "content-type" => content_type = "application/json",
        "corrupt" => {
            return (
                status,
                [("content-type", content_type)],
                "{private-raw-body",
            )
                .into_response();
        }
        "large" => {
            return (status, [("content-type", content_type)], "x".repeat(8192)).into_response();
        }
        _ => {}
    }
    let mut response = (
        status,
        [("content-type", content_type)],
        serde_json::to_vec(&value).unwrap(),
    )
        .into_response();
    if mode == "duplicate" {
        response
            .headers_mut()
            .append("content-type", "application/json".parse().unwrap());
    }
    if mode == "missing" {
        response.headers_mut().remove("content-type");
    }
    response
}
struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn serve() -> (String, ServerTask) {
    let app = HookServerAxum::router(Service)
        .route("/wire/{mode}", get(wire))
        .route(
            "/legacy",
            get(|| async {
                api_macros::axum::http_error_response(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    ServerError {
                        legacy: "legacy-value",
                        mode: 1,
                    },
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = ServerTask(tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    }));
    (url, task)
}

#[tokio::test]
async fn generated_client_hook_uses_actual_metadata_and_preserves_unknown_corrupt_and_size_failures()
 {
    let (url, _server) = serve().await;
    let client = HookClientClient::builder(&url)
        .unwrap()
        .request_timeout(Duration::from_secs(5))
        .response_body_limit(16384)
        .build()
        .unwrap();
    match client.problem().await {
        Err(ClientError::Public { status, error }) => {
            assert_eq!(status.as_u16(), 403);
            assert_eq!(error.operation, "session.record");
        }
        _ => panic!("expected a typed problem"),
    }
    for mode in ["unknown", "version"] {
        assert!(matches!(
            client.wire(mode.into()).await,
            Err(ClientError::Failure(
                ClientFailure::UnsupportedErrorResponse { status: 403 }
            ))
        ));
    }
    for (mode, expected) in [
        ("status", 409),
        ("content-type", 403),
        ("corrupt", 403),
        ("duplicate", 403),
        ("missing", 403),
    ] {
        let result = client.wire(mode.into()).await;
        assert!(
            matches!(result, Err(ClientError::Failure(ClientFailure::ErrorResponseDecode { status })) if status == expected),
            "mode={mode}"
        );
    }
    // Contract budget and streaming transport budget are independent, bounded failures.
    assert!(matches!(
        client.wire("large".into()).await,
        Err(ClientError::Failure(ClientFailure::ResponseTooLarge {
            limit: 4096
        }))
    ));
    let small_client = HookClientClient::builder(&url)
        .unwrap()
        .request_timeout(Duration::from_secs(5))
        .response_body_limit(128)
        .build()
        .unwrap();
    assert!(matches!(
        small_client.problem().await,
        Err(ClientError::Failure(ClientFailure::ResponseTooLarge {
            limit: 128
        }))
    ));
    match client.legacy().await {
        Err(ClientError::Public { status, error }) => {
            assert_eq!(status.as_u16(), 422);
            assert_eq!(error.legacy, "legacy-value");
        }
        _ => panic!("unrelated error on the same trait must keep serde behavior"),
    }
    let trait_client = TraitHookClientClient::builder(&url)
        .unwrap()
        .request_timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    assert!(
        matches!(trait_client.problem().await, Err(ClientError::Public { error, .. }) if error.operation == "session.record")
    );
    let legacy = LegacyClientClient::builder(&url)
        .unwrap()
        .request_timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    match legacy.legacy().await {
        Err(ClientError::Public { status, error }) => {
            assert_eq!(status.as_u16(), 422);
            assert_eq!(error.legacy, "legacy-value");
        }
        _ => panic!("legacy client-only error must keep serde behavior"),
    }
}
