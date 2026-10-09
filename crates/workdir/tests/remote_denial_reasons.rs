#![cfg(feature = "http-client")]
//! Loopback fake at the HTTP provider boundary; no product process or live service.
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use workdir::{
    StatRequest, Workdir, WorkdirDenialReason as Reason, WorkdirPath, WorkdirSession,
    http::{
        OpenWorkdirSessionRequest, RemoteWorkdirSession, WorkdirTransportError,
        WorkdirTransportErrorCode as Code,
    },
};

fn reply(listener: &TcpListener, status: &str, body: &str) {
    let (mut stream, _) = listener.accept().unwrap();
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
            reply(
                &listener,
                "200 OK",
                r#"{"session_id":"s","workdir_id":"w","capabilities":{"bits":63}}"#,
            );
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
