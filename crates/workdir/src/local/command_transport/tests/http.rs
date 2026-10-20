//! Loopback integration of the real supervisor, byte queue/replay, local session,
//! HTTP dispatcher/client. OS wait/read are
//! deterministic faults; session open and start bind that injected process fixture.
//! Authorization is not asserted here (Runtime's signed HTTP suite owns it).

use super::*;
use crate::http::*;
use axum::{Json, Router, extract::State, routing::post};

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn operation(
    State(session): State<Arc<LocalWorkdirSession>>,
    Json(request): Json<WorkdirSessionOperationRequest>,
) -> Json<WorkdirSessionOperationResult> {
    if matches!(request.operation, WorkdirSessionOperation::CommandStart(_)) {
        return Json(WorkdirSessionOperationResult::CommandStart(CommandHandle(
            "injected".into(),
        )));
    }
    Json(
        dispatch_workdir_session_operation(session.as_ref(), request.operation)
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn pipe_and_wait_faults_cross_real_http_with_prefix_and_terminal_replay() {
    tokio::time::timeout(Duration::from_secs(20), async {
        for (stream, after_exit, wait_fault) in [
            (CommandStream::Stdout, false, false),
            (CommandStream::Stderr, false, false),
            (CommandStream::Stdout, true, false),
            (CommandStream::Stderr, true, false),
            (CommandStream::Stdout, false, true),
        ] {
            let root = TempDir::new().unwrap();
            let session = Arc::new(LocalWorkdirSession::new(
                Scope::writable(root.path()).unwrap(),
                root.path().into(),
            ));
            let ready = Gate::default();
            let exited = Gate::default();
            if wait_fault {
                ready.open();
            }
            let process = FaultProcess {
                ready: ready.clone(),
                exited: exited.clone(),
                wait_error: wait_fault.then_some(std::io::ErrorKind::InvalidData),
            };
            let fail_ready = if after_exit {
                exited
            } else {
                let gate = Gate::default();
                gate.open();
                gate
            };
            let reader = FaultReader {
                prefix: b"partial",
                prefix_read: if after_exit { ready } else { Gate::default() },
                fail_ready,
            };
            let command = start_with(
                "injected".into(),
                CommandTelemetry::new(),
                vec![],
                move |cancel, sender| async move {
                    let telemetry = CommandTelemetry::new();
                    if wait_fault {
                        supervise(
                            process,
                            &b"partial"[..],
                            &b""[..],
                            10,
                            100,
                            "injected",
                            &telemetry,
                            cancel,
                            &sender,
                        )
                        .await
                    } else {
                        match stream {
                            CommandStream::Stdout => {
                                supervise(
                                    process,
                                    reader,
                                    &b""[..],
                                    10,
                                    100,
                                    "injected",
                                    &telemetry,
                                    cancel,
                                    &sender,
                                )
                                .await
                            }
                            CommandStream::Stderr => {
                                supervise(
                                    process,
                                    &b""[..],
                                    reader,
                                    10,
                                    100,
                                    "injected",
                                    &telemetry,
                                    cancel,
                                    &sender,
                                )
                                .await
                            }
                        }
                    }
                },
            );
            session
                .inner
                .commands
                .lock()
                .await
                .insert("injected".into(), command);
            let app = Router::new()
                .route(
                    "/v1/working-directories/{id}/sessions",
                    post(|| async {
                        Json(OpenWorkdirSessionResponse {
                            session_id: WorkdirSessionId::new("fault-session").unwrap(),
                            workdir_id: crate::Workdir::new("fault-workdir").id().clone(),
                            capabilities: crate::WorkdirSessionCapabilities::ALL,
                        })
                    }),
                )
                .route("/v1/workdir-sessions/{id}/operations", post(operation))
                .with_state(session.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let _server = Server(tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            }));
            let remote: crate::WorkdirSessionHandle = Arc::new(
                RemoteWorkdirSession::open(
                    reqwest::Client::builder().no_proxy().build().unwrap(),
                    base.parse().unwrap(),
                    "fixture-token",
                    crate::Workdir::new("fault-workdir").id().clone(),
                    OpenWorkdirSessionRequest::default(),
                )
                .await
                .unwrap(),
            );
            let handle = CommandHandle("injected".into());
            let mut request = CommandOutputRequest {
                handle,
                cursor: 0,
                limit: 1,
                wait: true,
            };
            let mut bytes = Vec::new();
            let terminal = loop {
                let page = remote.command_output(request.clone()).await.unwrap();
                assert_eq!(remote.command_output(request.clone()).await.unwrap(), page);
                for chunk in &page.chunks {
                    assert_eq!(chunk.sequence, request.cursor);
                    assert_eq!(chunk.offset, request.cursor);
                    assert_eq!(chunk.stream, stream);
                    bytes.extend_from_slice(&chunk.bytes);
                    request.cursor += chunk.bytes.len() as u64;
                }
                assert_eq!(page.next_cursor, request.cursor);
                if let Some(terminal) = page.terminal {
                    break terminal;
                }
            };
            assert_eq!(bytes, b"partial");
            assert_eq!(terminal.truncation_reason, None);
            assert_eq!(terminal.stdout_bytes + terminal.stderr_bytes, 7);
            assert_eq!(
                terminal.stdout_delivered_bytes + terminal.stderr_delivered_bytes,
                7
            );
            if wait_fault {
                assert_eq!(terminal.status, CommandStatus::Failed);
                assert_eq!(terminal.exit_code, None);
                assert_eq!(
                    terminal.process_failure,
                    Some(CommandFailure {
                        stage: CommandFailureStage::Wait,
                        reason: CommandIoErrorKind::InvalidData,
                        os_code: None
                    })
                );
                assert_eq!(
                    terminal.output_collection,
                    CommandOutputCollection::Complete
                );
            } else {
                assert_eq!(terminal.status, CommandStatus::Completed);
                assert_eq!(terminal.exit_code, Some(0));
                let stage = match (stream, after_exit) {
                    (CommandStream::Stdout, false) => CommandFailureStage::StdoutRead,
                    (CommandStream::Stderr, false) => CommandFailureStage::StderrRead,
                    (CommandStream::Stdout, true) => CommandFailureStage::StdoutDrain,
                    (CommandStream::Stderr, true) => CommandFailureStage::StderrDrain,
                };
                assert_eq!(
                    terminal.output_collection,
                    CommandOutputCollection::Incomplete {
                        failure: CommandFailure {
                            stage,
                            reason: CommandIoErrorKind::PermissionDenied,
                            os_code: None
                        }
                    }
                );
            }
            let wire = serde_json::to_string(&terminal).unwrap();
            for secret in [
                "/private",
                "TOKEN",
                "secret",
                "command-body",
                "fixture-token",
            ] {
                assert!(!wire.contains(secret));
            }
            session.close().await.unwrap();
        }
    })
    .await
    .expect("bounded loopback fault projection");
}
