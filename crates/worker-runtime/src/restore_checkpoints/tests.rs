use super::*;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::thread;

fn config(endpoint: SocketAddr) -> RestoreCheckpointConfig {
    RestoreCheckpointConfig {
        endpoint,
        worker_id: WorkerId::now_v7(),
        stage: RestoreCheckpointStage::PreparedNotCommitted,
        nonce: "checkpoint-test-handshake".into(),
    }
}

fn ack(config: &RestoreCheckpointConfig) -> RestoreCheckpointAck {
    RestoreCheckpointAck {
        nonce: config.nonce.clone(),
        stage: config.stage,
        runtime_id: "checkpoint-runtime".into(),
        worker_id: config.worker_id,
        operation_id: WorkerLifecycleOperationId::new(),
        commit: RestoreCheckpointCommit::NotCommitted,
        receipt_absent: true,
        pid: std::process::id(),
    }
}

#[test]
fn config_rejects_nonliteral_nonloopback_bad_stage_unknown_fields_and_invalid_worker() {
    let valid = serde_json::to_value(config("127.0.0.1:1234".parse().unwrap())).unwrap();
    for (field, value) in [
        ("endpoint", "localhost:1234"),
        ("endpoint", "192.0.2.1:1234"),
        ("endpoint", "0.0.0.0:1234"),
        ("endpoint", "127.0.0.1:0"),
        ("endpoint", "[::ffff:127.0.0.1]:1234"),
        ("worker_id", "not-a-worker-id"),
        ("stage", "after_terminal"),
        ("nonce", ""),
        ("nonce", "secret\nbody"),
        ("timeout", "unbounded"),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value.into();
        let parsed = serde_json::from_value::<RestoreCheckpointConfig>(invalid);
        assert!(
            parsed.is_err() || parsed.unwrap().validate().is_err(),
            "{field}"
        );
    }
    for endpoint in ["127.0.0.1:1234", "[::1]:1234"] {
        config(endpoint.parse().unwrap()).validate().unwrap();
    }
}

#[test]
fn config_file_bounds_and_errors_do_not_disclose_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    assert!(RestoreCheckpointConfig::from_path(&path).is_err());
    std::fs::write(&path, vec![b'x'; MAX_RESTORE_CHECKPOINT_CONFIG_BYTES + 1]).unwrap();
    assert!(RestoreCheckpointConfig::from_path(&path).is_err());
    std::fs::write(
        &path,
        br#"{"nonce":"private-config-body","stage":"secret"}"#,
    )
    .unwrap();
    let error = RestoreCheckpointConfig::from_path(&path)
        .unwrap_err()
        .to_string();
    assert!(!error.contains("private-config-body"));
    assert!(!error.contains("secret"));
    let mut valid = config("127.0.0.1:1234".parse().unwrap());
    valid.nonce = "x".repeat(257);
    assert!(valid.validate().is_err());
    valid.nonce = "valid".into();
    std::fs::write(&path, serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(RestoreCheckpointConfig::from_path(&path).unwrap(), valid);
    assert!(RestoreCheckpointConfig::from_path(dir.path()).is_err());
}

#[test]
fn explicit_release_eof_and_invalid_release_close_the_owned_stream() {
    for release in [Some(b'R'), None, Some(b'X')] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let config = config(listener.local_addr().unwrap());
        let expected = ack(&config);
        let sent = expected.clone();
        let task = thread::spawn(move || exchange(&config, &sent));
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(&mut stream).read_line(&mut line).unwrap();
        assert!(line.len() <= MAX_RESTORE_CHECKPOINT_ACK_BYTES);
        assert_eq!(
            serde_json::from_str::<RestoreCheckpointAck>(&line).unwrap(),
            expected
        );
        if let Some(byte) = release {
            stream.write_all(&[byte]).unwrap();
        } else {
            stream.shutdown(std::net::Shutdown::Write).unwrap();
        }
        assert_eq!(task.join().unwrap().is_ok(), release == Some(b'R'));
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    }
}

#[test]
fn absent_listener_resumes_without_waiting_or_retrying() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = config(listener.local_addr().unwrap());
    drop(listener);
    let start = Instant::now();
    wait_for_release(&config, &ack(&config));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn silent_peer_hits_fixed_deadline_and_stream_is_dropped() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = config(listener.local_addr().unwrap());
    let start = Instant::now();
    let task = thread::spawn(move || exchange(&config, &ack(&config)));
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(12)))
        .unwrap();
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(task.join().unwrap().is_err());
    assert!(start.elapsed() >= RESTORE_CHECKPOINT_TIMEOUT - Duration::from_millis(100));
    assert!(start.elapsed() < RESTORE_CHECKPOINT_TIMEOUT + Duration::from_secs(3));
    assert_eq!(reader.read(&mut [0]).unwrap(), 0);
}
