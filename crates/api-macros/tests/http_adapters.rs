#![cfg(all(feature = "reqwest", feature = "axum"))]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use api_macros::{api, reqwest as api_reqwest};
use serde::{Deserialize, Serialize};

pub struct NonSerdeClientFrame;
pub struct NonSerdeServerFrame;

#[api(reqwest, axum)]
pub trait WebSocketOnlyApi {
    #[websocket(
        "/events/{stream_id}",
        operation_id = "events.websocket",
        method = GET,
        client_to_server = NonSerdeClientFrame,
        server_to_client = NonSerdeServerFrame,
        path_parameters = [stream_id: u64]
    )]
    type Events;
}

#[derive(Clone)]
pub struct WebSocketOnlyService;
impl WebSocketOnlyApi for WebSocketOnlyService {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CreateWidget {
    name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Lookup {
    mode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Widget {
    id: u64,
    name: String,
    mode: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PublicError {
    message: String,
    #[serde(skip)]
    status: u16,
}

impl api_macros::HttpError for PublicError {
    fn status_code(&self) -> u16 {
        self.status
    }
}

impl api_macros::HttpRequestError for PublicError {
    fn from_request_rejection(status: u16, message: String) -> Self {
        Self { message, status }
    }
}

#[api(reqwest, axum)]
pub trait WidgetApi {
    #[post(
        "/widgets/{widget_id}",
        operation_id = "widgets.create",
        status = 201,
        error_status = 422
    )]
    async fn create(
        &self,
        #[path] widget_id: u64,
        #[query] lookup: Lookup,
        #[header("authorization")] authorization: String,
        #[body] request: CreateWidget,
    ) -> Result<Widget, PublicError>;

    #[delete("/widgets/{widget_id}", operation_id = "widgets.delete", status = 204)]
    async fn delete(&self, #[path] widget_id: u64) -> ();
}

#[derive(Clone)]
pub struct WidgetService;

impl WidgetApi for WidgetService {
    async fn create(
        &self,
        widget_id: u64,
        lookup: Lookup,
        authorization: String,
        request: CreateWidget,
    ) -> Result<Widget, PublicError> {
        assert_eq!(authorization, "Bearer top-secret");
        if request.name == "bad" {
            return Err(PublicError {
                message: "sensitive-public-detail".to_owned(),
                status: 409,
            });
        }
        Ok(Widget {
            id: widget_id,
            name: request.name,
            mode: lookup.mode,
        })
    }

    async fn delete(&self, _widget_id: u64) {}
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BinaryReceipt {
    bytes: Vec<u8>,
    content_type: String,
}

#[api(reqwest, axum)]
pub trait BinaryApi {
    #[put(
        "/binary",
        operation_id = "binary.put",
        status = 200,
        error_status = 400,
        additional_error_statuses = [413],
        normalize_body_errors = true
    )]
    async fn upload(
        &self,
        #[header("content-type")] content_type: String,
        #[binary] body: api_macros::BinaryBody,
    ) -> Result<BinaryReceipt, PublicError>;
}

#[derive(Clone)]
pub struct BinaryService;

impl BinaryApi for BinaryService {
    async fn upload(
        &self,
        content_type: String,
        body: api_macros::BinaryBody,
    ) -> Result<BinaryReceipt, PublicError> {
        Ok(BinaryReceipt {
            bytes: body.as_ref().to_vec(),
            content_type,
        })
    }
}

#[api(reqwest, axum)]
pub trait ResponseApi {
    #[get(
        "/conditional",
        operation_id = "responses.conditional",
        responses = [
            (status = 200, body = Widget, headers = [("etag", String), ("cache-control", String)]),
            (status = 304, headers = [("etag", String), ("cache-control", String)])
        ]
    )]
    async fn conditional(&self, #[query] lookup: Lookup) -> response_api_responses::Conditional;

    #[post(
        "/cookie",
        operation_id = "responses.cookie",
        responses = [
            (status = 200, body = Widget, headers = [("set-cookie", String)])
        ]
    )]
    async fn cookie(&self) -> response_api_responses::Cookie;
}

#[derive(Clone)]
pub struct ResponseService;

impl ResponseApi for ResponseService {
    async fn conditional(&self, lookup: Lookup) -> response_api_responses::Conditional {
        if lookup.mode == "cached" {
            response_api_responses::Conditional::Status304 {
                header_etag: "\"widget-v1\"".to_owned(),
                header_cache_control: "no-cache".to_owned(),
            }
        } else {
            response_api_responses::Conditional::Status200 {
                body: Widget {
                    id: 42,
                    name: "conditional".to_owned(),
                    mode: lookup.mode,
                },
                header_etag: "\"widget-v1\"".to_owned(),
                header_cache_control: "no-cache".to_owned(),
            }
        }
    }

    async fn cookie(&self) -> response_api_responses::Cookie {
        response_api_responses::Cookie::Status200 {
            body: Widget {
                id: 7,
                name: "cookie".to_owned(),
                mode: "session".to_owned(),
            },
            header_set_cookie: "session=top-secret; HttpOnly; SameSite=Lax".to_owned(),
        }
    }
}

#[api(reqwest)]
pub trait ResponseFailureApi {
    #[get(
        "/missing-header",
        responses = [(status = 200, headers = [("x-count", u32)])]
    )]
    async fn missing_header(&self) -> response_failure_api_responses::MissingHeader;

    #[get(
        "/invalid-header",
        responses = [(status = 200, headers = [("x-count", u32)])]
    )]
    async fn invalid_header(&self) -> response_failure_api_responses::InvalidHeader;

    #[get(
        "/duplicate-header",
        responses = [(status = 200, headers = [("x-count", u32)])]
    )]
    async fn duplicate_header(&self) -> response_failure_api_responses::DuplicateHeader;
}

#[api(reqwest)]
pub trait FailureApi {
    #[get("/malformed", operation_id = "failure.malformed", status = 200)]
    async fn malformed(&self) -> Widget;

    #[get("/oversized", operation_id = "failure.oversized", status = 200)]
    async fn oversized(&self) -> Widget;

    #[get("/slow", operation_id = "failure.slow", status = 200)]
    async fn slow(&self) -> Widget;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthorizedRequest {
    method: String,
    path_and_query: String,
    body: Vec<u8>,
}

#[derive(Clone)]
pub struct RecordingAuthorizer {
    request: Arc<Mutex<Option<AuthorizedRequest>>>,
    credential: String,
}

impl api_reqwest::RequestAuthorizer for RecordingAuthorizer {
    fn authorize(
        &self,
        request: api_reqwest::AuthorizerRequest<'_>,
    ) -> Result<api_reqwest::framework::header::HeaderMap, api_reqwest::AuthorizationError> {
        *self.request.lock().expect("recording lock") = Some(AuthorizedRequest {
            method: request.method.to_string(),
            path_and_query: request.path_and_query.to_owned(),
            body: request.body.to_vec(),
        });
        let mut headers = api_reqwest::framework::header::HeaderMap::new();
        headers.insert(
            api_reqwest::framework::header::AUTHORIZATION,
            self.credential.parse().expect("valid fixture credential"),
        );
        Ok(headers)
    }
}

async fn missing_header() -> api_macros::axum::framework::StatusCode {
    api_macros::axum::framework::StatusCode::OK
}

async fn invalid_header() -> api_macros::axum::framework::Response {
    api_macros::axum::framework::Response::builder()
        .status(api_macros::axum::framework::StatusCode::OK)
        .header("x-count", "not-a-number")
        .body(api_macros::axum::framework::Body::empty())
        .expect("valid fixture response")
}

async fn duplicate_header() -> api_macros::axum::framework::Response {
    api_macros::axum::framework::Response::builder()
        .status(api_macros::axum::framework::StatusCode::OK)
        .header("x-count", "1")
        .header("x-count", "2")
        .body(api_macros::axum::framework::Body::empty())
        .expect("valid fixture response")
}

async fn malformed() -> &'static str {
    "this is not json"
}

async fn oversized() -> String {
    "response-content-that-must-never-appear-in-debug-output".to_owned()
}

async fn slow() -> &'static str {
    tokio::time::sleep(Duration::from_millis(100)).await;
    "{}"
}

async fn manual_websocket_route(
    api_macros::axum::framework::Path(stream_id): api_macros::axum::framework::Path<u64>,
) -> String {
    format!("stream:{stream_id}")
}

#[tokio::test]
async fn websocket_declarations_skip_unary_adapters_and_mount_manual_handlers() {
    let _client = WebSocketOnlyApiClient::try_new("http://127.0.0.1:1")
        .expect("WebSocket-only contracts still provide a client with no unary methods");
    let app = api_macros::axum::websocket_route::<web_socket_only_api_operations::Events, _, _, _>(
        api_macros::axum::framework::Router::new(),
        manual_websocket_route,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = tokio::spawn(async move {
        api_macros::axum::framework::serve(listener, app)
            .await
            .expect("serve fixture WebSocket route");
    });

    let body = reqwest::get(format!("http://{address}/events/42"))
        .await
        .expect("manual route response")
        .text()
        .await
        .expect("manual route body");
    assert_eq!(body, "stream:42");
    server.abort();
}

#[tokio::test]
async fn generated_client_and_router_follow_the_normalized_contract() {
    let app = WidgetApiAxum::router(WidgetService).merge(
        api_macros::axum::framework::Router::new()
            .route("/malformed", api_macros::axum::framework::get(malformed))
            .route("/oversized", api_macros::axum::framework::get(oversized))
            .route("/slow", api_macros::axum::framework::get(slow)),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = tokio::spawn(async move {
        api_macros::axum::framework::serve(listener, app)
            .await
            .expect("serve fixture API");
    });
    let base_url = format!("http://{address}/");

    let recorded = Arc::new(Mutex::new(None));
    let client = WidgetApiClient::builder(&base_url)
        .expect("validated base URL")
        .authorizer(RecordingAuthorizer {
            request: recorded.clone(),
            credential: "Bearer top-secret".to_owned(),
        })
        .build()
        .expect("client build");

    let widget = client
        .create(
            7,
            Lookup {
                mode: "full".to_owned(),
            },
            "placeholder-overwritten-by-authorizer".to_owned(),
            CreateWidget {
                name: "alpha".to_owned(),
            },
        )
        .await
        .expect("typed success response");
    assert_eq!(
        widget,
        Widget {
            id: 7,
            name: "alpha".to_owned(),
            mode: "full".to_owned(),
        }
    );
    assert_eq!(
        recorded.lock().expect("recording lock").clone(),
        Some(AuthorizedRequest {
            method: "POST".to_owned(),
            path_and_query: "/widgets/7?mode=full".to_owned(),
            body: br#"{"name":"alpha"}"#.to_vec(),
        })
    );
    assert!(!format!("{client:?}").contains("top-secret"));

    client.delete(7).await.expect("typed empty response");

    let public_error = client
        .create(
            8,
            Lookup {
                mode: "full".to_owned(),
            },
            "placeholder".to_owned(),
            CreateWidget {
                name: "bad".to_owned(),
            },
        )
        .await
        .expect_err("typed public error");
    assert!(!format!("{public_error:?}").contains("sensitive-public-detail"));
    assert!(matches!(
        &public_error,
        api_reqwest::ClientError::Public { status, .. }
            if *status == api_reqwest::framework::StatusCode::CONFLICT
    ));
    assert_eq!(
        public_error.into_public_error(),
        Some(PublicError {
            message: "sensitive-public-detail".to_owned(),
            status: 0,
        })
    );

    let malformed_error = FailureApiClient::try_new(&base_url)
        .expect("failure client")
        .malformed()
        .await
        .expect_err("malformed JSON must fail");
    assert_eq!(
        format!("{malformed_error:?}"),
        "Failure(success response JSON was malformed)"
    );
    assert!(!format!("{malformed_error:?}").contains("this is not json"));

    let oversized_error = FailureApiClient::builder(&base_url)
        .expect("failure client builder")
        .response_body_limit(8)
        .build()
        .expect("failure client")
        .oversized()
        .await
        .expect_err("oversized body must fail");
    assert!(matches!(
        oversized_error,
        api_reqwest::ClientError::Failure(api_reqwest::ClientFailure::ResponseTooLarge {
            limit: 8
        })
    ));
    assert!(!format!("{oversized_error:?}").contains("response-content"));

    let timeout_error = FailureApiClient::builder(&base_url)
        .expect("failure client builder")
        .request_timeout(Duration::from_millis(10))
        .build()
        .expect("failure client")
        .slow()
        .await
        .expect_err("slow response must time out");
    assert!(matches!(
        timeout_error,
        api_reqwest::ClientError::Failure(api_reqwest::ClientFailure::Timeout)
    ));

    let _route_specific = widget_api_axum::create(Arc::new(WidgetService));
    server.abort();
}

#[tokio::test]
async fn declared_response_status_bodies_and_headers_round_trip_without_secret_diagnostics() {
    let app = ResponseApiAxum::router(ResponseService).merge(
        api_macros::axum::framework::Router::new()
            .route(
                "/missing-header",
                api_macros::axum::framework::get(missing_header),
            )
            .route(
                "/invalid-header",
                api_macros::axum::framework::get(invalid_header),
            )
            .route(
                "/duplicate-header",
                api_macros::axum::framework::get(duplicate_header),
            ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind declared-response fixture server");
    let address = listener.local_addr().expect("fixture address");
    let server = tokio::spawn(async move {
        api_macros::axum::framework::serve(listener, app)
            .await
            .expect("serve declared-response fixture API");
    });
    let base_url = format!("http://{address}/");
    let client = ResponseApiClient::try_new(&base_url).expect("response client");

    let fresh = client
        .conditional(Lookup {
            mode: "fresh".to_owned(),
        })
        .await
        .expect("200 declared response");
    match fresh {
        response_api_responses::Conditional::Status200 {
            body,
            header_etag,
            header_cache_control,
        } => {
            assert_eq!(body.id, 42);
            assert_eq!(header_etag, "\"widget-v1\"");
            assert_eq!(header_cache_control, "no-cache");
        }
        other => panic!("unexpected fresh variant: {other:?}"),
    }

    let cached = client
        .conditional(Lookup {
            mode: "cached".to_owned(),
        })
        .await
        .expect("304 declared response");
    assert!(matches!(
        cached,
        response_api_responses::Conditional::Status304 {
            ref header_etag,
            ref header_cache_control,
        } if header_etag == "\"widget-v1\"" && header_cache_control == "no-cache"
    ));

    let cookie = client.cookie().await.expect("cookie response");
    let cookie_debug = format!("{cookie:?}");
    assert!(!cookie_debug.contains("top-secret"));
    match cookie {
        response_api_responses::Cookie::Status200 {
            body,
            header_set_cookie,
        } => {
            assert_eq!(body.name, "cookie");
            assert_eq!(
                header_set_cookie,
                "session=top-secret; HttpOnly; SameSite=Lax"
            );
        }
    }

    let failures = ResponseFailureApiClient::try_new(&base_url).expect("failure client");
    let missing = failures
        .missing_header()
        .await
        .expect_err("missing header must fail");
    assert!(matches!(
        missing,
        api_reqwest::ClientError::Failure(api_reqwest::ClientFailure::MissingResponseHeader {
            name: "x-count"
        })
    ));
    let invalid = failures
        .invalid_header()
        .await
        .expect_err("invalid header must fail");
    assert!(matches!(
        invalid,
        api_reqwest::ClientError::Failure(api_reqwest::ClientFailure::InvalidResponseHeader {
            name: "x-count"
        })
    ));
    let duplicate = failures
        .duplicate_header()
        .await
        .expect_err("duplicate header must fail");
    assert!(matches!(
        duplicate,
        api_reqwest::ClientError::Failure(api_reqwest::ClientFailure::DuplicateResponseHeader {
            name: "x-count"
        })
    ));

    server.abort();
}

#[tokio::test]
async fn binary_adapter_preserves_exact_bytes_content_type_and_route_limit() {
    let app = binary_api_axum::upload(Arc::new(BinaryService))
        .layer(api_macros::axum::framework::DefaultBodyLimit::max(4));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind binary fixture server");
    let address = listener.local_addr().expect("binary fixture address");
    let server = tokio::spawn(async move {
        api_macros::axum::framework::serve(listener, app)
            .await
            .expect("serve binary fixture API");
    });

    let recorded = Arc::new(Mutex::new(None));
    let client = BinaryApiClient::builder(format!("http://{address}/"))
        .expect("validated base URL")
        .authorizer(RecordingAuthorizer {
            request: recorded.clone(),
            credential: "Bearer binary-test".to_owned(),
        })
        .build()
        .expect("binary client build");

    for expected in [vec![], vec![0, 1], vec![0, 255, 1, 2]] {
        let receipt = client
            .upload(
                "text/plain".to_owned(),
                api_macros::BinaryBody::from(expected.clone()),
            )
            .await
            .expect("binary body within the route limit");
        assert_eq!(receipt.bytes, expected);
        assert_eq!(receipt.content_type, "application/octet-stream");
        assert_eq!(
            recorded
                .lock()
                .expect("recording lock")
                .as_ref()
                .expect("authorized request")
                .body,
            expected
        );
    }

    let oversized = client
        .upload(
            "text/plain".to_owned(),
            api_macros::BinaryBody::from(vec![0, 1, 2, 3, 4]),
        )
        .await
        .expect_err("one byte over the route limit must be rejected");
    assert!(matches!(
        oversized,
        api_reqwest::ClientError::Public { status, .. }
            if status == api_reqwest::framework::StatusCode::PAYLOAD_TOO_LARGE
    ));
    assert_eq!(
        format!("{:?}", api_macros::BinaryBody::from(vec![1, 2, 3])),
        "BinaryBody(<redacted>)"
    );

    server.abort();
}

#[test]
fn base_urls_are_strict_and_named_adapter_types_are_public() {
    assert!(api_reqwest::BaseUrl::try_from("ftp://example.test/").is_err());
    assert!(api_reqwest::BaseUrl::try_from("https://user@example.test/").is_err());
    assert!(api_reqwest::BaseUrl::try_from("https://example.test/?token=secret").is_err());
    assert!(api_reqwest::BaseUrl::try_from("https://example.test/#fragment").is_err());

    fn assert_client(_: &WidgetApiClient) {}
    fn assert_builder(_: &WidgetApiClientBuilder) {}
    let client = WidgetApiClient::try_new("https://example.test/").expect("client");
    let builder = WidgetApiClient::builder("https://example.test/").expect("builder");
    assert_client(&client);
    assert_builder(&builder);
}
