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
        WorkdirHttpAuthorization, WorkdirHttpAuthorizationFuture, WorkdirSessionId,
    },
};

struct Provider {
    url: reqwest::Url,
    requests: mpsc::UnboundedReceiver<TcpStream>,
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
                stream.read_exact(&mut vec![0; length]).await.unwrap();
                assert!(
                    headers.starts_with("POST /v1/working-directories/w/sessions ")
                        || headers.starts_with("DELETE /v1/workdir-sessions/s ")
                );
                if tx.send(stream).is_err() {
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
    async fn request_while<F: Future>(&mut self, future: std::pin::Pin<&mut F>) -> TcpStream {
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                request = self.requests.recv() => request.expect("provider stopped"),
                _ = future => panic!("close/open completed without the expected HTTP request"),
            }
        })
        .await
        .expect("request timed out")
    }
    async fn open(&mut self, auth: Arc<dyn WorkdirHttpAuthorization>) -> RemoteWorkdirSession {
        let future = RemoteWorkdirSession::open_with_authorization(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            self.url.clone(),
            auth,
            Workdir::new("w").id().clone(),
            OpenWorkdirSessionRequest::default(),
        );
        tokio::pin!(future);
        let stream = self.request_while(future.as_mut()).await;
        let body = serde_json::to_string(&OpenWorkdirSessionResponse {
            session_id: WorkdirSessionId::new("s").unwrap(),
            workdir_id: Workdir::new("w").id().clone(),
            capabilities: workdir::WorkdirSessionCapabilities::ALL,
            environment: workdir::WorkdirSessionEnvironment::root(
                workdir::WorkdirSessionCapabilities::ALL,
            ),
        })
        .unwrap();
        reply(stream, "200 OK", &body).await;
        future.await.unwrap()
    }
}
async fn reply(mut stream: TcpStream, status: &str, body: &str) {
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
