#![cfg(all(feature = "reqwest", feature = "axum"))]

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use api_macros::{api, reqwest as api_reqwest};
use serde::{Deserialize, Serialize};

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
    assert_eq!(
        public_error.into_public_error(),
        Some(PublicError {
            message: "sensitive-public-detail".to_owned(),
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
