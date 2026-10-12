//! Module-boundary fixtures exercise the same writer/request path as ChildStdin.
use super::*;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};

#[derive(Clone, Copy)]
enum Stall {
    PartialWrite,
    Flush,
    Response,
}

struct ControlledWriter {
    stall: Stall,
    bytes: Arc<StdMutex<Vec<u8>>>,
    reached: Option<mpsc::UnboundedSender<()>>,
    dropped: Arc<AtomicBool>,
}

impl ControlledWriter {
    fn signal(&mut self) {
        if let Some(tx) = self.reached.take() {
            tx.send(()).unwrap();
        }
    }
}

impl AsyncWrite for ControlledWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if matches!(self.stall, Stall::PartialWrite) {
            if self.bytes.lock().unwrap().is_empty() {
                self.bytes.lock().unwrap().extend_from_slice(&buf[..8]);
                return Poll::Ready(Ok(8));
            }
            self.signal();
            return Poll::Pending;
        }
        self.bytes.lock().unwrap().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.signal();
        if matches!(self.stall, Stall::Flush) {
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl Drop for ControlledWriter {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}

fn fixture_client(stdin: SharedStdin) -> (McpStdioClient, mpsc::Sender<ReaderEvent>) {
    let (tx, responses) = mpsc::channel(16);
    let redactor = Redactor::new(vec!["fixture-secret".into()]);
    let client = McpStdioClient {
        server_name: "deadline-fixture".into(),
        limits: McpStdioLimits {
            request_timeout: Duration::from_millis(100),
            ..Default::default()
        },
        diagnostics: Arc::new(Mutex::new(BoundedDiagnostics::new(
            "deadline-fixture".into(),
            2,
            redactor.clone(),
        ))),
        redactor,
        list_changes: Arc::new(Mutex::new(BoundedListChanged::new(
            "deadline-fixture".into(),
        ))),
        stdin,
        child: None,
        responses,
        reader_task: tokio::spawn(std::future::pending()),
        stderr_task: tokio::spawn(std::future::pending()),
        next_id: 1,
        initialized: None,
        shutdown_started: false,
    };
    (client, tx)
}

async fn assert_pending<F: Future>(mut future: Pin<&mut F>) {
    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("fixture boundary unexpectedly completed"),
    })
    .await;
}

#[tokio::test]
async fn stdin_ownership_timeout_and_cancel_do_not_send_a_method() {
    for cancel in [false, true] {
        let bytes = Arc::new(StdMutex::new(Vec::new()));
        let dropped = Arc::new(AtomicBool::new(false));
        let stdin: SharedStdin = Arc::new(Mutex::new(Some(Box::new(ControlledWriter {
            stall: Stall::Response,
            bytes: bytes.clone(),
            reached: None,
            dropped: dropped.clone(),
        }))));
        let guard = stdin.lock().await;
        let (mut client, _tx) = fixture_client(stdin.clone());
        let mut request =
            Box::pin(client.request::<Value>(McpPhase::Running, "tools/call", json!({})));
        assert_pending(request.as_mut()).await;
        if !cancel {
            let err = timeout(Duration::from_secs(1), &mut request)
                .await
                .unwrap()
                .unwrap_err();
            assert!(matches!(err.kind, McpErrorKind::Timeout { .. }));
            assert_eq!(err.request_stage(), Some(McpRequestStage::WaitingForStdin));
            assert!(err.to_string().contains("method not sent"));
        }
        drop(request);
        assert!(bytes.lock().unwrap().is_empty());
        assert!(!dropped.load(Ordering::SeqCst));
        assert!(
            guard.is_some(),
            "cancel before ownership must preserve the stream"
        );
    }
}

#[tokio::test]
async fn interrupted_write_or_flush_closes_stdin_before_the_next_frame() {
    for (stall, stage) in [
        (Stall::PartialWrite, McpRequestStage::WritingFrame),
        (Stall::Flush, McpRequestStage::FlushingFrame),
    ] {
        for cancel in [false, true] {
            let bytes = Arc::new(StdMutex::new(Vec::new()));
            let dropped = Arc::new(AtomicBool::new(false));
            let (reached, mut rx) = mpsc::unbounded_channel();
            let stdin: SharedStdin = Arc::new(Mutex::new(Some(Box::new(ControlledWriter {
                stall,
                bytes: bytes.clone(),
                reached: Some(reached),
                dropped: dropped.clone(),
            }))));
            let (mut client, _tx) = fixture_client(stdin.clone());
            let mut request = Box::pin(client.request::<Value>(
                McpPhase::Running,
                "tools/call",
                json!({"secret": "fixture-secret"}),
            ));
            // Poll all the way to the controlled Pending, then observe its channel.
            assert_pending(request.as_mut()).await;
            timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if !cancel {
                let err = timeout(Duration::from_secs(1), &mut request)
                    .await
                    .unwrap()
                    .unwrap_err();
                assert!(matches!(err.kind, McpErrorKind::Timeout { .. }));
                assert_eq!(err.request_stage(), Some(stage));
                let message = err.to_string();
                assert!(message.contains("delivery unknown; stdin closed"));
                assert!(!message.contains("fixture-secret"));
            }
            drop(request);
            assert!(dropped.load(Ordering::SeqCst));
            assert!(stdin.lock().await.is_none());
            let before = bytes.lock().unwrap().clone();
            if matches!(stall, Stall::PartialWrite) {
                assert!(!before.contains(&b'\n'));
            } else {
                assert_eq!(before.last(), Some(&b'\n'));
            }
            let err = client
                .request::<Value>(McpPhase::Running, "next-method", json!({}))
                .await
                .unwrap_err();
            assert!(err.to_string().contains("stdin is closed"));
            assert_eq!(
                *bytes.lock().unwrap(),
                before,
                "no next frame can append after interruption"
            );
        }
    }
}

#[tokio::test]
async fn flushed_call_timeout_or_cancel_is_not_replayed_and_late_response_is_not_mixed() {
    for cancel in [false, true] {
        let bytes = Arc::new(StdMutex::new(Vec::new()));
        let (reached, mut rx) = mpsc::unbounded_channel();
        let stdin: SharedStdin = Arc::new(Mutex::new(Some(Box::new(ControlledWriter {
            stall: Stall::Response,
            bytes: bytes.clone(),
            reached: Some(reached),
            dropped: Arc::new(AtomicBool::new(false)),
        }))));
        let (mut client, tx) = fixture_client(stdin);
        let mut request =
            Box::pin(client.request::<Value>(McpPhase::Running, "tools/call", json!({})));
        assert_pending(request.as_mut()).await;
        timeout(Duration::from_secs(1), rx.recv())
            .await
            .unwrap()
            .unwrap();
        if !cancel {
            let err = timeout(Duration::from_secs(1), &mut request)
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(err.request_stage(), Some(McpRequestStage::AwaitingResponse));
            assert!(err.to_string().contains("execution unknown; do not retry"));
        }
        drop(request);
        // Reuse is safe only because the first frame completed and IDs are
        // matched. This is a different method, not a replay of the unknown call.
        tx.send(ReaderEvent::Response(ServerResponse {
            id: 1,
            result: Some(json!({"late": true})),
            error: None,
        }))
        .await
        .unwrap();
        tx.send(ReaderEvent::Response(ServerResponse {
            id: 2,
            result: Some(json!({"next": true})),
            error: None,
        }))
        .await
        .unwrap();
        let result = client
            .request::<Value>(McpPhase::Running, "next-method", json!({}))
            .await
            .unwrap();
        assert_eq!(result, json!({"next": true}));
        let captured = bytes.lock().unwrap().clone();
        let frames: Vec<Value> = captured
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["method"], "tools/call");
        assert_eq!(frames[1]["method"], "next-method");
    }
}

// Observes actual OS-pipe backpressure, not a guessed delay or just a mock
// Pending. It forwards every operation to ChildStdin without changing framing.
struct PendingObserver<W> {
    writer: W,
    accepted: usize,
    reached: Option<mpsc::UnboundedSender<()>>,
}
impl<W: AsyncWrite + Unpin> AsyncWrite for PendingObserver<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let result = Pin::new(&mut self.writer).poll_write(cx, buf);
        if let Poll::Ready(Ok(count)) = &result {
            self.accepted += count;
        }
        if result.is_pending() && self.accepted > 1024 {
            if let Some(tx) = self.reached.take() {
                tx.send(()).unwrap();
            }
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.writer).poll_shutdown(cx)
    }
}

#[cfg(unix)]
#[tokio::test]
async fn blocked_reader_reply_does_not_prevent_shutdown_process_reaping() {
    // A local stdio fixture: initialize, consume initialized, emit an unsupported
    // server request whose echoed ID exceeds pipe capacity, then never read.
    // The shell has no descendants and ignores TERM to also prove kill fallback.
    let init = json!({"jsonrpc":"2.0", "id":1, "result": {"protocolVersion": MCP_PROTOCOL_VERSION, "capabilities":{}, "serverInfo":{"name":"fixture", "version":"1"}}});
    let server_request =
        json!({"jsonrpc":"2.0", "id":"x".repeat(256 * 1024), "method":"sampling/createMessage"});
    // Avoid argv limits: generate the large protocol ID in the shell itself.
    let script = format!(
        "trap '' TERM; read -r init; printf '%s\\n' '{init}'; read -r initialized; printf '%s' '{{\"jsonrpc\":\"2.0\",\"method\":\"sampling/createMessage\",\"id\":\"'; i=0; while [ $i -lt 4096 ]; do printf '%s' '{}'; i=$((i+1)); done; printf '%s\\n' '\"}}'; while :; do :; done",
        "x".repeat(64)
    );
    assert!(server_request.to_string().len() < McpStdioLimits::default().max_stdout_line_bytes);
    let limits = McpStdioLimits {
        shutdown_timeout: Duration::from_millis(50),
        kill_timeout: Duration::from_millis(100),
        ..Default::default()
    };
    let mut client = McpStdioClient::spawn(
        McpStdioServerSpec::new("reader-blocked", "/bin/sh")
            .arg("-c")
            .arg(script),
        limits,
    )
    .await
    .unwrap();
    let pid = client.child.as_ref().unwrap().id().unwrap();
    let (reached, mut rx) = mpsc::unbounded_channel();
    {
        let mut guard = client.stdin.lock().await;
        let writer = guard.take().unwrap();
        *guard = Some(Box::new(PendingObserver {
            writer,
            accepted: 0,
            reached: Some(reached),
        }));
    }
    client.initialize().await.unwrap();
    timeout(Duration::from_secs(3), rx.recv())
        .await
        .expect("reader reply reached OS pipe Pending")
        .unwrap();
    assert!(
        client.stdin.try_lock().is_err(),
        "reader owns stdin while its reply is blocked"
    );
    let report = timeout(Duration::from_secs(1), client.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(report.terminated && report.killed);
    assert!(report.exit_status.is_some());
    assert!(client.child.is_none());
    // Tokio wait already reaped the child; an additional wait cannot find it.
    let mut status = 0;
    assert_eq!(
        unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
