//! HTTP transport の単発 request / error classification テスト。
//!
//! Retry/backoff は Engine の lifecycle 管理に属するため、transport は 1 回だけ
//! request を送り、HTTP status / Retry-After を `ClientError` に載せて返す。

use futures::StreamExt;
use llm_engine::llm_client::LlmClient;
use llm_engine::llm_client::auth::AuthRequirement;
use llm_engine::llm_client::capability::ModelCapability;
use llm_engine::llm_client::error::ClientError;
use llm_engine::llm_client::event::Event;
use llm_engine::llm_client::scheme::Scheme;
use llm_engine::llm_client::transport::{HttpTransport, ResolvedAuth};
use llm_engine::llm_client::types::Request;
use serde_json::Value;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// SSE 本体は触らないテスト用 scheme。`parse_fail` を立てると
/// stream 消費中で `ClientError::Sse` を返す。
#[derive(Clone)]
struct DummyScheme {
    parse_fail: bool,
}

impl Scheme for DummyScheme {
    type State = ();

    fn default_base_url(&self) -> &'static str {
        ""
    }

    fn path(&self, _: &str) -> String {
        "/v1/chat".into()
    }

    fn required_auth(&self) -> AuthRequirement {
        AuthRequirement::None
    }

    fn build_request_body(&self, _: &str, _: &Request, _: &ModelCapability) -> Value {
        serde_json::json!({})
    }

    fn parse_sse(&self, _: &str, _: &str, _: &mut ()) -> Result<Vec<Event>, ClientError> {
        if self.parse_fail {
            Err(ClientError::Sse(
                "simulated mid-stream parse failure".into(),
            ))
        } else {
            Ok(vec![])
        }
    }

    fn default_capability(&self) -> ModelCapability {
        ModelCapability::minimal()
    }
}

fn build_transport(base_url: impl Into<String>, parse_fail: bool) -> HttpTransport<DummyScheme> {
    HttpTransport::new(
        DummyScheme { parse_fail },
        "test-model",
        base_url,
        ResolvedAuth::None,
        ModelCapability::minimal(),
    )
}

fn ok_sse() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_raw(b"".to_vec(), "text/event-stream")
}

#[tokio::test]
async fn retryable_status_returns_api_error_without_retrying() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream connect error"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ok_sse())
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false);
    match transport.stream(Request::default()).await {
        Err(ClientError::Api {
            status: Some(503), ..
        }) => {}
        Err(other) => panic!("expected Api(503), got {other:?}"),
        Ok(_) => panic!("transport must not retry internally"),
    }

    let received = server.received_requests().await.unwrap();
    assert_eq!(
        received.len(),
        1,
        "transport should send exactly one request"
    );
}

#[tokio::test]
async fn retry_after_header_is_preserved_on_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "1"))
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false);
    match transport.stream(Request::default()).await {
        Err(
            err @ ClientError::Api {
                status: Some(503), ..
            },
        ) => {
            assert_eq!(err.retry_after(), Some(Duration::from_secs(1)));
        }
        Err(other) => panic!("expected Api(503), got {other:?}"),
        Ok(_) => panic!("expected error"),
    }
}

#[tokio::test]
async fn mid_stream_sse_error_is_stream_item_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_raw(
                    b"event: data\ndata: payload\n\n".to_vec(),
                    "text/event-stream",
                ),
        )
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), true);
    let mut stream = transport
        .stream(Request::default())
        .await
        .expect("status 200 should open stream");
    let mut saw_sse_err = false;
    while let Some(item) = stream.next().await {
        if matches!(item, Err(ClientError::Sse(_))) {
            saw_sse_err = true;
        }
    }
    assert!(saw_sse_err, "expected Sse error from stream consumer");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "mid-stream Sse must not reopen stream");
}

#[tokio::test]
async fn non_retryable_status_returns_api_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false);
    match transport.stream(Request::default()).await {
        Err(ClientError::Api {
            status: Some(401), ..
        }) => {}
        Err(other) => panic!("expected Api(401), got {other:?}"),
        Ok(_) => panic!("expected error"),
    }

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
}
