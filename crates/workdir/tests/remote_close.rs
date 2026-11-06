#![cfg(feature = "http-client")]
//! Remote close protocol tests with a loopback HTTP boundary. Requests and responses
//! are explicitly gated; Runtime lease integration belongs to workspace-server.
use std::{
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use workdir::{
    Workdir, WorkdirError, WorkdirSession,
    http::{
        OpenWorkdirSessionRequest, OpenWorkdirSessionResponse, RemoteWorkdirSession,
        RemoteWorkdirSessionRegistry, WorkdirHttpAuthorization, WorkdirHttpAuthorizationFuture,
    },
};

struct Request {
    stream: TcpStream,
    method: String,
    path: String,
    body: Vec<u8>,
}

struct Provider {
    url: reqwest::Url,
    requests: mpsc::UnboundedReceiver<Request>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Provider {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let (tx, requests) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut sessions = std::collections::HashSet::new();
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    bytes.push(stream.read_u8().await.unwrap());
                    assert!(bytes.len() < 16384);
                }
                let headers = String::from_utf8(bytes).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                assert!(length < 16384);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).await.unwrap();
                let mut request_line = headers.lines().next().unwrap().split_whitespace();
                let method = request_line.next().unwrap().to_string();
                let path = request_line.next().unwrap().to_string();
                assert!(
                    (method == "POST" && path == "/v1/working-directories/w/sessions")
                        || (method == "DELETE" && path.starts_with("/v1/workdir-sessions/"))
                );
                if method == "POST" {
                    let open: OpenWorkdirSessionRequest = serde_json::from_slice(&body).unwrap();
                    sessions.insert(open.session_id.as_str().to_owned());
                } else {
                    assert!(
                        sessions.contains(path.strip_prefix("/v1/workdir-sessions/").unwrap()),
                        "DELETE must address an ID from an actual open request: {path}"
                    );
                }
                if tx
                    .send(Request {
                        stream,
                        method,
                        path,
                        body,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            url,
            requests,
            task,
        }
    }
    async fn request_while<F: Future>(&mut self, future: std::pin::Pin<&mut F>) -> Request {
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                request = self.requests.recv() => request.expect("provider stopped"),
                _ = future => panic!("close/open completed without the expected HTTP request"),
            }
        })
        .await
        .expect("request timed out")
    }
    async fn request(&mut self) -> Request {
        tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
            .await
            .expect("request timed out")
            .expect("provider stopped")
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    }

    async fn open(&mut self, auth: Arc<dyn WorkdirHttpAuthorization>) -> RemoteWorkdirSession {
        self.open_in(&RemoteWorkdirSessionRegistry::default(), auth)
            .await
    }

    async fn open_in(
        &mut self,
        registry: &RemoteWorkdirSessionRegistry,
        auth: Arc<dyn WorkdirHttpAuthorization>,
    ) -> RemoteWorkdirSession {
        let future = registry.open_with_authorization(
            Self::client(),
            self.url.clone(),
            auth,
            Workdir::new("w").id().clone(),
            OpenWorkdirSessionRequest::default(),
            Duration::from_secs(30),
            None,
        );
        tokio::pin!(future);
        let request = self.request_while(future.as_mut()).await;
        opened(request).await;
        future.await.unwrap()
    }
}

async fn opened(request: Request) {
    assert_eq!(request.method, "POST");
    let open: OpenWorkdirSessionRequest = serde_json::from_slice(&request.body).unwrap();
    let body = serde_json::to_string(&OpenWorkdirSessionResponse {
        session_id: open.session_id,
        workdir_id: Workdir::new("w").id().clone(),
        capabilities: workdir::WorkdirSessionCapabilities::ALL,
        environment: workdir::WorkdirSessionEnvironment::root(
            workdir::WorkdirSessionCapabilities::ALL,
        ),
    })
    .unwrap();
    reply(request, "200 OK", &body).await;
}

async fn reply(request: Request, status: &str, body: &str) {
    let mut stream = request.stream;
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
}
#[derive(Debug, Default)]
struct Authorization {
    reject_delete: AtomicBool,
}
impl WorkdirHttpAuthorization for Authorization {
    fn bearer_token(
        &self,
        method: String,
        _: String,
        _: Vec<u8>,
    ) -> WorkdirHttpAuthorizationFuture {
        let reject = method == "DELETE" && self.reject_delete.swap(false, Ordering::SeqCst);
        Box::pin(async move {
            if reject {
                Err(WorkdirError::Unavailable(
                    "fixture authorization rejected".into(),
                ))
            } else {
                Ok("fixture-token".into())
            }
        })
    }
}

#[tokio::test]
async fn close_authorization_failure_retries_same_session_and_confirmed_close_skips_http() {
    let mut provider = Provider::new().await;
    let auth = Arc::new(Authorization::default());
    let remote = provider.open(auth.clone()).await;
    auth.reject_delete.store(true, Ordering::SeqCst);
    assert!(remote.close().await.is_err());
    assert!(provider.requests.try_recv().is_err());
    let retry = remote.close();
    tokio::pin!(retry);
    let request = provider.request_while(retry.as_mut()).await;
    reply(request, "204 No Content", "").await;
    retry.await.unwrap();
    remote.close().await.unwrap();
    assert!(provider.requests.try_recv().is_err());
}

#[derive(Debug)]
struct GatedAuthorization {
    entered: mpsc::UnboundedSender<()>,
    release: std::sync::Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl WorkdirHttpAuthorization for GatedAuthorization {
    fn bearer_token(
        &self,
        method: String,
        _: String,
        _: Vec<u8>,
    ) -> WorkdirHttpAuthorizationFuture {
        let gate = if method == "DELETE" {
            self.release.lock().unwrap().take()
        } else {
            None
        };
        let entered = self.entered.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                entered.send(()).unwrap();
                gate.await.unwrap();
            }
            Ok("fixture-token".into())
        })
    }
}

#[tokio::test]
async fn close_cancel_during_authorization_allows_an_actual_retry() {
    let mut provider = Provider::new().await;
    let (entered, mut events) = mpsc::unbounded_channel();
    let (_release, gate) = tokio::sync::oneshot::channel();
    let remote = provider
        .open(Arc::new(GatedAuthorization {
            entered,
            release: std::sync::Mutex::new(Some(gate)),
        }))
        .await;
    let mut first = Box::pin(remote.close());
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! {
            _ = events.recv() => (),
            _ = &mut first => panic!("close completed while authorization was gated"),
        }
    })
    .await
    .unwrap();
    drop(first);
    assert!(provider.requests.try_recv().is_err());
    let retry = remote.close();
    tokio::pin!(retry);
    let request = provider.request_while(retry.as_mut()).await;
    reply(request, "204 No Content", "").await;
    retry.await.unwrap();
}

#[tokio::test]
async fn close_http_rejection_or_lost_response_requires_another_delete() {
    for response in [
        Some((
            "503 Service Unavailable",
            r#"{"code":"unavailable","message":"fixture"}"#,
        )),
        None,
    ] {
        let mut provider = Provider::new().await;
        let remote = provider.open(Arc::new(Authorization::default())).await;
        let first = remote.close();
        tokio::pin!(first);
        let request = provider.request_while(first.as_mut()).await;
        if let Some((status, body)) = response {
            reply(request, status, body).await;
        } else {
            drop(request);
        } // Runtime may have deleted; no response confirms it.
        assert!(first.await.is_err());
        let retry = remote.close();
        tokio::pin!(retry);
        let request = provider.request_while(retry.as_mut()).await;
        reply(request, "204 No Content", "").await;
        retry.await.unwrap();
    }
}

#[tokio::test]
async fn close_cancel_releases_serialization_and_waiter_must_confirm_delete() {
    let mut provider = Provider::new().await;
    let remote = provider.open(Arc::new(Authorization::default())).await;
    let mut first = Box::pin(remote.close());
    let request = provider.request_while(first.as_mut()).await;
    let mut waiter = Box::pin(remote.close());
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(waiter.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(first);
    drop(request);
    let request = provider.request_while(waiter.as_mut()).await;
    reply(request, "204 No Content", "").await;
    waiter.await.unwrap();
}

#[tokio::test]
async fn concurrent_close_waits_for_confirmation_and_retries_after_failure() {
    for success in [false, true] {
        let mut provider = Provider::new().await;
        let remote = provider.open(Arc::new(Authorization::default())).await;
        let mut first = Box::pin(remote.close());
        let request = provider.request_while(first.as_mut()).await;
        let mut waiter = Box::pin(remote.close());
        assert!(
            std::future::poll_fn(|cx| Poll::Ready(waiter.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        if success {
            reply(request, "204 No Content", "").await;
            first.await.unwrap();
        } else {
            drop(request);
            assert!(first.await.is_err());
            let request = provider.request_while(waiter.as_mut()).await;
            reply(request, "204 No Content", "").await;
        }
        waiter.await.unwrap();
        assert!(provider.requests.try_recv().is_err());
    }
}

fn assert_delete(request: &Request, session_id: &workdir::http::WorkdirSessionId) {
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.path,
        format!("/v1/workdir-sessions/{}", session_id.as_str())
    );
    assert!(request.body.is_empty());
}

#[tokio::test]
async fn canceled_open_waiter_is_retained_until_response_then_closed_without_reopening() {
    let mut provider = Provider::new().await;
    let registry = RemoteWorkdirSessionRegistry::default();
    let request = OpenWorkdirSessionRequest::default();
    let session_id = request.session_id.clone();
    let opener = {
        let registry = registry.clone();
        let url = provider.url.clone();
        tokio::spawn(async move {
            registry
                .open_with_authorization(
                    Provider::client(),
                    url,
                    Arc::new(Authorization::default()),
                    Workdir::new("w").id().clone(),
                    request,
                    Duration::from_secs(30),
                    None,
                )
                .await
        })
    };
    let pending = provider.request().await;
    opener.abort();
    assert!(opener.await.unwrap_err().is_cancelled());
    // The dispatched open task still owns its lifetime. Cleanup must not race
    // ahead of its pending response and must not create a second open.
    let cleanup = registry.close_abandoned("w");
    tokio::pin!(cleanup);
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(cleanup.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert!(provider.requests.try_recv().is_err());
    opened(pending).await;
    let delete = provider.request_while(cleanup.as_mut()).await;
    assert_delete(&delete, &session_id);
    reply(delete, "204 No Content", "").await;
    cleanup.await.unwrap();
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
}

#[tokio::test]
async fn lost_open_response_closes_the_identity_chosen_before_dispatch() {
    let mut provider = Provider::new().await;
    let registry = RemoteWorkdirSessionRegistry::default();
    let request = OpenWorkdirSessionRequest::default();
    let session_id = request.session_id.clone();
    let future = registry.open_with_authorization(
        Provider::client(),
        provider.url.clone(),
        Arc::new(Authorization::default()),
        Workdir::new("w").id().clone(),
        request,
        Duration::from_secs(30),
        None,
    );
    tokio::pin!(future);
    let pending = provider.request_while(future.as_mut()).await;
    // Runtime may have admitted the request, but Backend receives no identity.
    drop(pending);
    assert!(future.await.is_err());
    let delete = provider.request().await;
    assert_delete(&delete, &session_id);
    reply(delete, "204 No Content", "").await;
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
}

#[tokio::test]
async fn failed_last_drop_close_stays_retryable_without_sweeping_live_sessions() {
    let mut provider = Provider::new().await;
    let registry = RemoteWorkdirSessionRegistry::default();
    let live = provider
        .open_in(&registry, Arc::new(Authorization::default()))
        .await;
    let abandoned = provider
        .open_in(&registry, Arc::new(Authorization::default()))
        .await;
    let abandoned_id = abandoned.session_id().clone();
    assert_ne!(live.session_id(), &abandoned_id);
    drop(abandoned);
    let delete = provider.request().await;
    assert_delete(&delete, &abandoned_id);
    reply(
        delete,
        "503 Service Unavailable",
        r#"{"code":"unavailable","message":"fixture"}"#,
    )
    .await;

    // Another Workdir's ordinary cleanup cannot claim the orphan. This also
    // leaves the live session alone even though it shares the orphan's Workdir.
    registry.close_abandoned("other").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
    let retry = registry.close_abandoned("w");
    tokio::pin!(retry);
    let delete = provider.request_while(retry.as_mut()).await;
    assert_delete(&delete, &abandoned_id);
    reply(delete, "204 No Content", "").await;
    retry.await.unwrap();
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());

    // The live handle is still responsible for its own distinct DELETE.
    let close_live = live.close();
    tokio::pin!(close_live);
    let delete = provider.request_while(close_live.as_mut()).await;
    assert_delete(&delete, live.session_id());
    reply(delete, "204 No Content", "").await;
    close_live.await.unwrap();
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
}

#[tokio::test]
async fn duplicate_open_identity_does_not_replace_or_close_a_live_retained_session() {
    let mut provider = Provider::new().await;
    let registry = RemoteWorkdirSessionRegistry::default();
    let remote = provider
        .open_in(&registry, Arc::new(Authorization::default()))
        .await;
    let duplicate = OpenWorkdirSessionRequest {
        session_id: remote.session_id().clone(),
        ..Default::default()
    };
    assert!(matches!(
        registry
            .open_with_authorization(
                Provider::client(),
                provider.url.clone(),
                Arc::new(Authorization::default()),
                Workdir::new("w").id().clone(),
                duplicate,
                Duration::from_secs(30),
                None,
            )
            .await,
        Err(WorkdirError::Conflict(_))
    ));
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
    let id = remote.session_id().clone();
    drop(remote);
    let delete = provider.request().await;
    assert_delete(&delete, &id);
    reply(delete, "204 No Content", "").await;
    registry.close_abandoned("w").await.unwrap();
    assert!(provider.requests.try_recv().is_err());
}
