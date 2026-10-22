#![cfg(feature = "http-client")]
//! Loopback fake at the HTTP provider boundary; no product process or live service.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};
use workdir::{
    StatRequest, Workdir, WorkdirDenialReason as Reason, WorkdirPath, WorkdirSession,
    http::{
        OpenWorkdirSessionRequest, RemoteWorkdirSession, WorkdirTransportError,
        WorkdirTransportErrorCode as Code,
    },
};

fn opened(workdir_id: &str) -> String {
    serde_json::to_string(&workdir::http::OpenWorkdirSessionResponse {
        session_id: workdir::http::WorkdirSessionId::new("s").unwrap(),
        workdir_id: Workdir::new(workdir_id).id().clone(),
        capabilities: workdir::WorkdirSessionCapabilities::ALL,
        environment: workdir::WorkdirSessionEnvironment::root(
            workdir::WorkdirSessionCapabilities::ALL,
        ),
    })
    .unwrap()
}

fn reply(listener: &TcpListener, status: &str, body: &str) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut stream, _) = loop {
        match listener.accept() {
            Ok(connection) => break connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "fake provider accept timed out");
                thread::yield_now();
            }
            Err(error) => panic!("fake provider accept failed: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    read_request(&mut stream);
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}
fn read_request(stream: &mut TcpStream) {
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 16384);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let length: usize = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    assert!(length < 16384);
    stream.read_exact(&mut vec![0; length]).unwrap();
}

#[tokio::test]
async fn remote_http_denials_forward_only_typed_reasons_and_old_missing_reason() {
    for (code, reason) in [
        (Code::Denied, Some(Reason::ReadOnlySession)),
        (Code::Denied, Some(Reason::OsPermissionDenied)),
        (Code::OutOfScope, Some(Reason::OsPermissionDenied)),
        (Code::Denied, None),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        // Host paths and secrets in provider-authored text must not become diagnostics.
        let mut error = serde_json::json!({"code":code, "message":"/secret/host/root bearer-secret command-secret"});
        if let Some(reason) = reason {
            error["denial_reason"] = serde_json::to_value(reason).unwrap();
        }
        let server = thread::spawn(move || {
            reply(&listener, "200 OK", &opened("w"));
            reply(&listener, "403 Forbidden", &error.to_string());
        });
        let remote = RemoteWorkdirSession::open(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            format!("http://{address}").parse().unwrap(),
            "request-secret",
            Workdir::new("w").id().clone(),
            OpenWorkdirSessionRequest::default(),
        )
        .await
        .unwrap();
        let denied = remote
            .stat(StatRequest {
                path: WorkdirPath::new("file").unwrap(),
            })
            .await
            .unwrap_err();
        assert_eq!(denied.denial_reason(), reason);
        assert!(!denied.to_string().contains("secret"));
        let forwarded = WorkdirTransportError::from_workdir_error(&denied);
        assert_eq!(forwarded.denial_reason, reason);
        assert_eq!(forwarded.code, code);
        assert_eq!(forwarded.code.http_status(), 403);
        assert_eq!(
            forwarded.message,
            if code == Code::Denied {
                "Workdir operation was denied"
            } else {
                "Workdir path is out of scope"
            }
        );
        let wire = serde_json::to_string(&forwarded).unwrap();
        assert!(!wire.contains("secret"));
        assert!(wire.len() < 256);
        server.join().unwrap();
    }
}

#[tokio::test]
async fn remote_checkout_diagnostics_preserve_mutation_outcome_and_read_failures() {
    use workdir::{CheckoutOperation as Op, CheckoutRequest, WorkdirSessionCapability};
    for code in [
        Code::Unavailable,
        Code::Transport,
        Code::Denied,
        Code::Conflict,
        Code::OutcomeUnknown,
    ] {
        for reason in [None, Some(Reason::ReadOnlySession)] {
            for operation in [
                Op::Read {
                    offset: 0,
                    limit: 1,
                    max_bytes: 10,
                },
                Op::Create {
                    path: WorkdirPath::new("new.txt").unwrap(),
                    content: b"fixture".to_vec(),
                },
                Op::Write {
                    content: b"fixture".to_vec(),
                    expected_hash: [0; 32],
                },
                Op::Edit {
                    old_string: "old".into(),
                    new_string: "new".into(),
                    replace_all: false,
                    expected_hash: [0; 32],
                },
            ] {
                let mutation = operation.capability() != WorkdirSessionCapability::Read;
                let expected_code =
                    if mutation && matches!(code, Code::Unavailable | Code::Transport) {
                        Code::OutcomeUnknown
                    } else {
                        code
                    };
                let listener = TcpListener::bind("127.0.0.1:0").unwrap();
                let address = listener.local_addr().unwrap();
                let error = serde_json::json!({"code":code, "message":"/secret/host provider-secret", "denial_reason":reason});
                let server = thread::spawn(move || {
                    reply(&listener, "200 OK", &opened("w"));
                    reply(
                        &listener,
                        &format!("{} Fixture failure", code.http_status()),
                        &error.to_string(),
                    );
                });
                let remote = RemoteWorkdirSession::open(
                    reqwest::Client::builder()
                        .no_proxy()
                        .timeout(Duration::from_secs(5))
                        .build()
                        .unwrap(),
                    format!("http://{address}").parse().unwrap(),
                    "request-secret",
                    Workdir::new("w").id().clone(),
                    OpenWorkdirSessionRequest::default(),
                )
                .await
                .unwrap();
                let error = remote
                    .checkout_execute(CheckoutRequest {
                        target: WorkdirPath::root(),
                        validator: vec![1],
                        operation,
                    })
                    .await
                    .unwrap_err();
                server.join().unwrap();
                let forwarded = WorkdirTransportError::from_workdir_error(&error);
                assert_eq!(
                    forwarded.code, expected_code,
                    "{code:?}, {reason:?}, mutation={mutation}: {error:?}"
                );
                assert_eq!(error.denial_reason(), reason);
                assert_eq!(forwarded.denial_reason, reason);
                assert_eq!(forwarded.code.http_status(), expected_code.http_status());
                if expected_code == Code::OutcomeUnknown {
                    assert!(forwarded.message.contains("inspect before retrying"));
                }
                assert!(
                    !serde_json::to_string(&forwarded)
                        .unwrap()
                        .contains("secret")
                );
            }
        }
    }
}

#[tokio::test]
async fn mismatched_http_results_keep_cause_and_mutation_uncertainty() {
    use workdir::{
        CheckoutOperation, CheckoutRequest, CommandHandle, CommandOutputRequest,
        WorkdirFailureReason as Failure,
    };
    for case in ["variant", "bytes", "mutation", "identity"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            reply(
                &listener,
                "200 OK",
                &opened(if case == "identity" {
                    "wrong-secret"
                } else {
                    "w"
                }),
            );
            if case != "identity" {
                let result = if case == "bytes" {
                    serde_json::json!({"operation":"command_output","result":{
                        "chunks": [], "next_cursor": 123, "terminal": null
                    }})
                } else {
                    serde_json::to_value(
                        workdir::http::WorkdirSessionOperationResult::CommandCancel,
                    )
                    .unwrap()
                };
                reply(&listener, "200 OK", &result.to_string());
            }
        });
        let opened = RemoteWorkdirSession::open(
            reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap(),
            format!("http://{address}").parse().unwrap(),
            "request-secret",
            Workdir::new("w").id().clone(),
            OpenWorkdirSessionRequest::default(),
        )
        .await;
        let error = if case == "identity" {
            opened.unwrap_err()
        } else {
            let remote = opened.unwrap();
            match case {
                "variant" => remote
                    .stat(StatRequest {
                        path: WorkdirPath::new("file-secret").unwrap(),
                    })
                    .await
                    .unwrap_err(),
                "bytes" => remote
                    .command_output(CommandOutputRequest {
                        handle: CommandHandle("command-secret".into()),
                        cursor: 0,
                        limit: 16,
                        wait: false,
                    })
                    .await
                    .unwrap_err(),
                "mutation" => remote
                    .checkout_execute(CheckoutRequest {
                        target: WorkdirPath::new("file-secret").unwrap(),
                        validator: vec![1],
                        operation: CheckoutOperation::Write {
                            content: b"body-secret".to_vec(),
                            expected_hash: [0; 32],
                        },
                    })
                    .await
                    .unwrap_err(),
                _ => unreachable!(),
            }
        };
        server.join().unwrap();
        let expected = if case == "identity" {
            Failure::SessionIdentityMismatch
        } else {
            Failure::OperationResultMismatch
        };
        assert_eq!(error.failure_reason(), Some(expected), "{case}: {error:?}");
        let transport = WorkdirTransportError::from_workdir_error(&error);
        assert_eq!(
            transport.code,
            if case == "mutation" {
                Code::OutcomeUnknown
            } else {
                Code::Unavailable
            }
        );
        let wire = serde_json::to_string(&transport).unwrap();
        assert!(!wire.contains("secret"), "{wire}");
        assert!(!format!("{error:?}").contains("secret"));
        let forwarded = transport.into_workdir_error();
        assert_eq!(forwarded.failure_reason(), Some(expected));
    }
}
