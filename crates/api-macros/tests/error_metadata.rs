#![cfg(feature = "axum")]

use api_macros::api;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower::ServiceExt;

// No Clone requirement: retaining an error must not require duplicating its typed value.
#[derive(Debug, Serialize)]
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
        Self { status, message }
    }
}

#[derive(Deserialize)]
pub struct Input {
    fail: bool,
}

#[derive(Serialize)]
pub struct Output {
    message: String,
}

#[api(axum)]
pub trait ErrorMetadataApi {
    #[post(
        "/json",
        status = 200,
        error_status = 400,
        normalize_body_errors = true
    )]
    async fn json(&self, #[body] input: Input) -> Result<Output, PublicError>;

    #[post(
        "/binary",
        status = 200,
        error_status = 400,
        normalize_body_errors = true
    )]
    async fn binary(&self, #[binary] input: api_macros::BinaryBody) -> Result<Output, PublicError>;
}

pub struct Service;
impl ErrorMetadataApi for Service {
    async fn json(&self, input: Input) -> Result<Output, PublicError> {
        if input.fail {
            Err(PublicError {
                status: 502,
                message: "upstream rejected operation".to_string(),
            })
        } else {
            Ok(Output {
                message: "ok".to_string(),
            })
        }
    }
    async fn binary(&self, _input: api_macros::BinaryBody) -> Result<Output, PublicError> {
        Ok(Output {
            message: "ok".to_string(),
        })
    }
}

#[tokio::test]
async fn generated_service_and_body_errors_retain_typed_metadata_without_changing_wire_body() {
    let app = ErrorMetadataApiAxum::router(Service).layer(axum::extract::DefaultBodyLimit::max(32));
    for (path, content_type, input, status) in [
        (
            "/json",
            "application/json",
            "{\"fail\":true}".to_string(),
            StatusCode::BAD_GATEWAY,
        ),
        (
            "/json",
            "application/json",
            "{".to_string(),
            StatusCode::BAD_REQUEST,
        ),
        (
            "/binary",
            "application/octet-stream",
            "x".repeat(33),
            StatusCode::PAYLOAD_TOO_LARGE,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", content_type)
                    .body(Body::from(input))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["content-type"], "application/json");
        let error = response
            .extensions()
            .get::<Arc<PublicError>>()
            .expect("typed error retained")
            .clone();
        assert_eq!(error.status, status.as_u16());
        assert!(!error.message.is_empty());
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"message": error.message})
        );
    }
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/json")
                .header("content-type", "application/json")
                .body(Body::from("{\"fail\":false}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.extensions().get::<Arc<PublicError>>().is_none());
    assert_eq!(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        b"{\"message\":\"ok\"}"
    );
}

#[test]
fn error_encoding_failure_does_not_retain_misleading_original_metadata() {
    struct UnencodableError;
    impl Serialize for UnencodableError {
        fn serialize<S: serde::Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom("cannot encode fixture error"))
        }
    }
    for status in [StatusCode::BAD_GATEWAY, StatusCode::INTERNAL_SERVER_ERROR] {
        let response = api_macros::axum::error_response(status, UnencodableError);
        assert_eq!(response.status(), status);
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        assert!(
            response
                .extensions()
                .get::<Arc<UnencodableError>>()
                .is_none()
        );
    }
}
