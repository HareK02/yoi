//! Real AuthProvider calls against owned auth files and a loopback HTTP peer.
//! Gates place callers at response-body / reload boundaries; paused Tokio time
//! advances the injected HTTP deadline only after headers have been received.
use super::tests::{make_jwt, write_auth};
use super::*;
use serde_json::{Value, json};
use std::time::Duration as StdDuration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};
use tokio::task::{JoinHandle, JoinSet};

struct Reply {
    status: u16,
    body: Vec<u8>,
    chunked: bool,
    gate: Option<Arc<Notify>>,
    truncate: bool,
}

impl Reply {
    fn body(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            chunked: false,
            gate: None,
            truncate: false,
        }
    }

    fn gated(status: u16, body: impl Into<Vec<u8>>, gate: &Arc<Notify>) -> Self {
        Self {
            gate: Some(gate.clone()),
            ..Self::body(status, body)
        }
    }
}

struct Stub {
    endpoint: String,
    requests: mpsc::UnboundedReceiver<Value>,
    task: JoinHandle<()>,
}

impl Drop for Stub {
    fn drop(&mut self) {
        // Dropping the server's JoinSet also aborts stalled connection handlers.
        self.task.abort();
    }
}

impl Stub {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/oauth/token", listener.local_addr().unwrap());
        let (tx, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            for reply in replies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let tx = tx.clone();
                handlers.spawn(async move {
                    let mut request = Vec::new();
                    let header_end = loop {
                        let byte = stream.read_u8().await.unwrap();
                        request.push(byte);
                        if request.ends_with(b"\r\n\r\n") { break request.len(); }
                        assert!(request.len() < 16 * 1024, "fixture request headers too large");
                    };
                    let headers = std::str::from_utf8(&request).unwrap();
                    assert!(headers.starts_with("POST /oauth/token HTTP/1.1\r\n"));
                    let len: usize = headers.lines().find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                    }).unwrap();
                    request.resize(header_end + len, 0);
                    stream.read_exact(&mut request[header_end..]).await.unwrap();
                    tx.send(serde_json::from_slice(&request[header_end..]).unwrap()).unwrap();
                    let framing = if reply.chunked {
                        "Transfer-Encoding: chunked".to_string()
                    } else {
                        format!("Content-Length: {}", reply.body.len())
                    };
                    stream.write_all(format!(
                        "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\n{framing}\r\nConnection: close\r\n\r\n", reply.status
                    ).as_bytes()).await.unwrap();
                    if let Some(gate) = reply.gate { gate.notified().await; }
                    let body = if reply.truncate { &reply.body[..reply.body.len() / 2] } else { &reply.body[..] };
                    if reply.chunked {
                        for chunk in body.chunks(512) {
                            let _ = stream.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await;
                            let _ = stream.write_all(chunk).await;
                            let _ = stream.write_all(b"\r\n").await;
                        }
                        let _ = stream.write_all(b"0\r\n\r\n").await;
                    } else {
                        let _ = stream.write_all(body).await;
                    }
                });
            }
            while let Some(result) = handlers.join_next().await {
                result.unwrap();
            }
        });
        Self {
            endpoint,
            requests,
            task,
        }
    }
}

fn rotated_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "access_token": make_jwt(json!({"exp": Utc::now().timestamp() + 3600})),
        "refresh_token": "fixture-rotated-secret",
    }))
    .unwrap()
}

fn provider(dir: &std::path::Path, stub: &Stub, started: &Arc<Notify>) -> Arc<CodexAuthProvider> {
    let mut provider = CodexAuthProvider::new(dir.to_path_buf())
        .with_refresh_endpoint(&stub.endpoint)
        .with_http_client(reqwest::Client::builder().no_proxy().build().unwrap());
    provider.refresh_limits = refresh::RefreshLimits {
        timeout: StdDuration::from_secs(10),
        max_body_bytes: 1024,
        body_started: Some(started.clone()),
    };
    Arc::new(provider)
}

async fn bounded<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(StdDuration::from_secs(5), future)
        .await
        .expect("fixture/caller did not progress")
}

fn assert_rotated(path: &std::path::Path, headers: &[(HeaderName, HeaderValue)]) {
    let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(value["tokens"]["refresh_token"], "fixture-rotated-secret");
    assert_eq!(value["fixture_extra"], "preserve me");
    let auth = headers
        .iter()
        .find(|(name, _)| name == "authorization")
        .unwrap();
    assert_eq!(
        auth.1.to_str().unwrap(),
        format!(
            "Bearer {}",
            value["tokens"]["access_token"].as_str().unwrap()
        )
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

fn stale_auth(dir: &std::path::Path) -> PathBuf {
    let path = write_auth(
        dir,
        Utc::now().timestamp() - 60,
        false,
        "fixture-old-secret",
    );
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["fixture_extra"] = json!("preserve me");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}

#[tokio::test]
async fn stalled_success_and_error_bodies_timeout_without_persist_and_release_waiting_caller() {
    for status in [200, 401, 503] {
        let dir = tempfile::tempdir().unwrap();
        let path = stale_auth(dir.path());
        let original = std::fs::read(&path).unwrap();
        let gate = Arc::new(Notify::new());
        let started = Arc::new(Notify::new());
        let mut stub = Stub::start(vec![
            Reply::gated(status, b"unfinished".to_vec(), &gate),
            Reply::body(200, rotated_body()),
        ])
        .await;
        let provider = provider(dir.path(), &stub, &started);
        let first = tokio::spawn({
            let provider = provider.clone();
            async move { provider.headers().await }
        });
        bounded(started.notified()).await; // provider has received headers and is consuming the body
        let request = bounded(stub.requests.recv()).await.unwrap();
        assert_eq!(request["refresh_token"], "fixture-old-secret");
        let second = provider.headers();
        tokio::pin!(second);
        assert!(futures::poll!(&mut second).is_pending());
        assert!(provider.state.try_lock().is_err());
        assert!(
            stub.requests.try_recv().is_err(),
            "waiter must not refresh concurrently"
        );
        tokio::time::pause();
        tokio::time::advance(StdDuration::from_secs(11)).await;
        let error = first.await.unwrap().unwrap_err();
        tokio::time::resume();
        assert!(error.to_string().contains("timed out"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let headers = bounded(second).await.unwrap();
        assert_rotated(&path, &headers);
        let retry = bounded(stub.requests.recv()).await.unwrap();
        assert_eq!(retry["refresh_token"], "fixture-old-secret");
    }
}

#[tokio::test]
async fn oversized_malformed_and_incomplete_bodies_do_not_persist_and_allow_later_refresh() {
    let mut cases = vec![
        (Reply::body(200, vec![b'x'; 1025]), "size limit"),
        (
            Reply {
                chunked: true,
                ..Reply::body(200, vec![b'x'; 1025])
            },
            "size limit",
        ),
        (
            Reply {
                chunked: true,
                ..Reply::body(401, vec![b'x'; 1025])
            },
            "size limit",
        ),
        (Reply::body(503, vec![b'x'; 1025]), "size limit"),
        (
            Reply::body(
                200,
                br#"{"access_token": {"secret":"fixture-body-secret"}}"#.to_vec(),
            ),
            "decode",
        ),
        (Reply::body(200, b"{".to_vec()), "decode"),
        (Reply::body(200, b"{}".to_vec()), "incomplete"),
        (
            Reply::body(
                200,
                br#"{"access_token":"","refresh_token":"fixture-body-secret"}"#.to_vec(),
            ),
            "incomplete",
        ),
        (
            Reply {
                truncate: true,
                ..Reply::body(200, rotated_body())
            },
            "read response body",
        ),
        (
            Reply {
                truncate: true,
                ..Reply::body(401, b"fixture-body-secret".to_vec())
            },
            "read response body",
        ),
        (
            Reply::body(401, b"fixture-body-secret".to_vec()),
            "codex login",
        ),
        (Reply::body(503, b"fixture-body-secret".to_vec()), "503"),
    ];
    for (reply, expected) in cases.drain(..) {
        let dir = tempfile::tempdir().unwrap();
        let path = stale_auth(dir.path());
        let original = std::fs::read(&path).unwrap();
        let started = Arc::new(Notify::new());
        let mut stub = Stub::start(vec![reply, Reply::body(200, rotated_body())]).await;
        let provider = provider(dir.path(), &stub, &started);
        let error = bounded(provider.headers()).await.unwrap_err();
        let message = error.to_string();
        assert!(message.contains(expected), "expected {expected}: {message}");
        assert!(!message.contains("fixture-body-secret"));
        assert!(!message.contains("fixture-old-secret"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let headers = bounded(provider.headers()).await.unwrap();
        assert_rotated(&path, &headers);
        for _ in 0..2 {
            assert_eq!(
                bounded(stub.requests.recv()).await.unwrap()["refresh_token"],
                "fixture-old-secret"
            );
        }
    }
}

#[tokio::test]
async fn cancelled_body_wait_leaves_auth_unchanged_and_releases_waiting_caller() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_auth(dir.path());
    let original = std::fs::read(&path).unwrap();
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let mut stub = Stub::start(vec![
        Reply::gated(200, rotated_body(), &gate),
        Reply::body(200, rotated_body()),
    ])
    .await;
    let provider = provider(dir.path(), &stub, &started);
    let first = tokio::spawn({
        let provider = provider.clone();
        async move { provider.headers().await }
    });
    bounded(started.notified()).await;
    bounded(stub.requests.recv()).await.unwrap();
    let second = provider.headers();
    tokio::pin!(second);
    assert!(futures::poll!(&mut second).is_pending());
    assert!(stub.requests.try_recv().is_err());
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_rotated(&path, &bounded(second).await.unwrap());
    assert_eq!(
        bounded(stub.requests.recv()).await.unwrap()["refresh_token"],
        "fixture-old-secret"
    );
}

#[tokio::test]
async fn shared_callers_refresh_once_and_use_persisted_rotated_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_auth(dir.path());
    let started = Arc::new(Notify::new());
    let gate = Arc::new(Notify::new());
    let mut stub = Stub::start(vec![Reply::gated(200, rotated_body(), &gate)]).await;
    let provider = provider(dir.path(), &stub, &started);
    let first = tokio::spawn({
        let provider = provider.clone();
        async move { provider.headers().await }
    });
    bounded(started.notified()).await;
    let second = provider.headers();
    tokio::pin!(second);
    assert!(futures::poll!(&mut second).is_pending());
    assert_eq!(
        bounded(stub.requests.recv()).await.unwrap()["refresh_token"],
        "fixture-old-secret"
    );
    let mut external: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    external["concurrent_extra"] = json!("merge me");
    std::fs::write(&path, serde_json::to_vec(&external).unwrap()).unwrap();
    gate.notify_one();
    let first_headers = bounded(first).await.unwrap().unwrap();
    let second_headers = bounded(second).await.unwrap();
    assert_eq!(first_headers, second_headers);
    assert_rotated(&path, &second_headers);
    let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(persisted["concurrent_extra"], "merge me");
    assert!(
        stub.requests.try_recv().is_err(),
        "single-flight must make only one request"
    );
}

#[tokio::test]
async fn account_switch_before_guarded_reload_skips_old_account_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = stale_auth(dir.path());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let mut provider = CodexAuthProvider::new(dir.path().to_path_buf());
    let mut stub = Stub::start(vec![]).await;
    provider.refresh_endpoint = stub.endpoint.clone();
    provider.before_guarded_reload = Some(barrier.clone());
    let task = tokio::spawn(async move { provider.headers().await });
    bounded(barrier.wait()).await;
    let mut switched: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    switched["tokens"]["account_id"] = json!("switched-account");
    switched["tokens"]["refresh_token"] = json!("fixture-switched-secret");
    std::fs::write(&path, serde_json::to_vec(&switched).unwrap()).unwrap();
    let original = std::fs::read(&path).unwrap();
    barrier.wait().await;
    let headers = bounded(task).await.unwrap().unwrap();
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "chatgpt-account-id" && value == "switched-account")
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(stub.requests.try_recv().is_err());
}

#[tokio::test]
async fn account_or_token_switch_during_refresh_is_not_overwritten_by_old_response() {
    for account_changed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = stale_auth(dir.path());
        let started = Arc::new(Notify::new());
        let gate = Arc::new(Notify::new());
        let mut stub = Stub::start(vec![Reply::gated(200, rotated_body(), &gate)]).await;
        let provider = provider(dir.path(), &stub, &started);
        let task = tokio::spawn({
            let provider = provider.clone();
            async move { provider.headers().await }
        });
        bounded(started.notified()).await;
        bounded(stub.requests.recv()).await.unwrap();
        let mut switched: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if account_changed {
            switched["tokens"]["account_id"] = json!("switched-account");
        } else {
            switched["tokens"]["refresh_token"] = json!("fixture-switched-secret");
        }
        switched["tokens"]["access_token"] =
            json!(make_jwt(json!({"exp": Utc::now().timestamp() + 7200})));
        std::fs::write(&path, serde_json::to_vec(&switched).unwrap()).unwrap();
        let original = std::fs::read(&path).unwrap();
        gate.notify_one();
        let headers = bounded(task).await.unwrap().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let expected = format!(
            "Bearer {}",
            switched["tokens"]["access_token"].as_str().unwrap()
        );
        assert!(headers.iter().any(|(name, value)| name == "authorization" && value.to_str().unwrap() == expected));
        assert_eq!(bounded(provider.headers()).await.unwrap(), headers);
        assert!(stub.requests.try_recv().is_err());
    }
}
