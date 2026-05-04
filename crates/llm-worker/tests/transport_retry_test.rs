//! HTTP transport の transient エラーリトライ挙動の integration テスト。
//!
//! 対応チケット: `tickets/llm-worker-transient-retry.md`。
//! - 503 / 529 / connect refused でリトライ発火
//! - max_attempts 上限到達でエラー
//! - `Retry-After` ヘッダで指数バックオフを上書き
//! - `parse_sse` 由来の `ClientError::Sse`（mid-stream 想定）はリトライしない

use std::time::{Duration, Instant};

use futures::StreamExt;
use llm_worker::llm_client::LlmClient;
use llm_worker::llm_client::auth::AuthRequirement;
use llm_worker::llm_client::capability::ModelCapability;
use llm_worker::llm_client::error::ClientError;
use llm_worker::llm_client::event::Event;
use llm_worker::llm_client::retry::RetryPolicy;
use llm_worker::llm_client::scheme::Scheme;
use llm_worker::llm_client::transport::{HttpTransport, ResolvedAuth};
use llm_worker::llm_client::types::Request;
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// SSE 本体は触らないテスト用 scheme。`parse_fail` を立てると
/// stream 消費中（= retry loop の外）で `ClientError::Sse` を返す。
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
            Err(ClientError::Sse("simulated mid-stream parse failure".into()))
        } else {
            Ok(vec![])
        }
    }
    fn default_capability(&self) -> ModelCapability {
        ModelCapability::minimal()
    }
}

fn fast_policy(max_attempts: u32) -> RetryPolicy {
    RetryPolicy {
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        max_attempts,
        total_timeout: Duration::from_secs(60),
    }
}

fn build_transport(
    base_url: impl Into<String>,
    parse_fail: bool,
    policy: RetryPolicy,
) -> HttpTransport<DummyScheme> {
    HttpTransport::new(
        DummyScheme { parse_fail },
        "test-model",
        base_url,
        ResolvedAuth::None,
        ModelCapability::minimal(),
    )
    .with_retry_policy(policy)
}

fn ok_sse() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_raw(b"".to_vec(), "text/event-stream")
}

#[tokio::test]
async fn retries_503_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(503).set_body_string("upstream connect error"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ok_sse())
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false, fast_policy(5));
    let mut stream = transport
        .stream(Request::default())
        .await
        .expect("stream should succeed after retries");
    while stream.next().await.is_some() {}

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 3, "two failures plus one success expected");
}

#[tokio::test]
async fn retries_529_then_exhausts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(529).set_body_string("overloaded"))
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false, fast_policy(3));
    match transport.stream(Request::default()).await {
        Err(ClientError::Api {
            status: Some(529), ..
        }) => {}
        Err(other) => panic!("expected Api(529), got {other:?}"),
        Ok(_) => panic!("expected error after exhausting retries"),
    }

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 3, "should hit max_attempts and stop");
}

#[tokio::test]
async fn connect_refused_retries_then_fails() {
    // 接続不能なローカルアドレスを使う。Linux では `Connection refused` で
    // 即時失敗するため、`fast_policy` ならテストが秒以下で終わる。
    let unreachable = "http://127.0.0.1:1";

    let transport = build_transport(unreachable, false, fast_policy(3));
    match transport.stream(Request::default()).await {
        Err(ClientError::Http(e)) => {
            assert!(
                e.is_connect() || e.is_timeout(),
                "expected connect/timeout, got {e:?}"
            );
        }
        Err(other) => panic!("expected Http error, got {other:?}"),
        Ok(_) => panic!("expected error connecting to closed port"),
    }
}

#[tokio::test]
async fn retry_after_header_overrides_backoff() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(503).insert_header("retry-after", "1"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ok_sse())
        .mount(&server)
        .await;

    // base/cap を 1ms に絞った policy で `Retry-After: 1` を観察すると、
    // 指数バックオフ単独なら 1ms 程度で終わるはずが Retry-After に従って
    // 1 秒待つ → 経過時間で override を検証できる。
    let policy = RetryPolicy {
        base: Duration::from_millis(1),
        cap: Duration::from_millis(1),
        max_attempts: 3,
        total_timeout: Duration::from_secs(10),
    };
    let transport = build_transport(server.uri(), false, policy);

    let start = Instant::now();
    let mut stream = transport.stream(Request::default()).await.expect("ok");
    while stream.next().await.is_some() {}
    let elapsed = start.elapsed();

    assert!(
        elapsed >= Duration::from_secs(1),
        "Retry-After=1 should make us wait >=1s, elapsed={elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "Retry-After=1 should not balloon, elapsed={elapsed:?}"
    );
}

#[tokio::test]
async fn mid_stream_sse_error_does_not_retry() {
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

    let transport = build_transport(server.uri(), true, fast_policy(5));
    let mut stream = transport
        .stream(Request::default())
        .await
        .expect("status 200 should bypass retry loop");
    let mut saw_sse_err = false;
    while let Some(item) = stream.next().await {
        if matches!(item, Err(ClientError::Sse(_))) {
            saw_sse_err = true;
        }
    }
    assert!(saw_sse_err, "expected Sse error from stream consumer");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "mid-stream Sse must not retry");
}

#[tokio::test]
async fn non_retryable_status_returns_immediately() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let transport = build_transport(server.uri(), false, fast_policy(5));
    match transport.stream(Request::default()).await {
        Err(ClientError::Api {
            status: Some(401), ..
        }) => {}
        Err(other) => panic!("expected Api(401), got {other:?}"),
        Ok(_) => panic!("expected error"),
    }

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "401 must not retry");
}
