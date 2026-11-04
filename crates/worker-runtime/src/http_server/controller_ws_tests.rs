// Included inside ws_tests. These two cases exercise the production Profile
// adapter, saved Session, paired protocol streams and Controller task/report/join;
// the other parser/filter unit cases intentionally keep their small WsBackend.

use super::controller_fixture::{ControllerFixture, NEW_MARKER, OLD_MARKER};

type ControllerSocket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;
const CONTROLLER_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

async fn controller_submit(stream: &mut ControllerSocket, request_id: &str) {
    let method = protocol::Method::submit_text(request_id, "produce the scripted reply");
    stream.send(Message::Text(serde_json::to_string(&method).unwrap().into())).await.unwrap();
}

fn assert_no_old_operational_output(event: &protocol::Event) {
    assert!(!matches!(event,
        protocol::Event::TextDelta { text } | protocol::Event::TextDone { text }
            if text.contains(OLD_MARKER)
    ), "old operational output followed a replacement snapshot: {event:?}");
    assert!(!matches!(event, protocol::Event::Shutdown), "old Shutdown followed a replacement snapshot");
}

async fn controller_output(
    stream: &mut ControllerSocket,
    request_id: &str,
    marker: &str,
    reject_old: bool,
) {
    let mut seen = Vec::new();
    tokio::time::timeout(CONTROLLER_WAIT, async {
        let (mut accepted, mut output, mut finished) = (false, false, false);
        while !(accepted && output && finished) {
            let event = next_frame(stream).await;
            seen.push(format!("{event:?}"));
            if reject_old { assert_no_old_operational_output(&event); }
            assert!(!matches!(&event,
                protocol::Event::Error { .. } | protocol::Event::SubmissionRejected { .. }
            ), "real Submit was rejected: {event:?}");
            accepted |= matches!(&event,
                protocol::Event::SubmissionAccepted { submission_request_id, .. }
                    if submission_request_id == request_id
            );
            output |= matches!(&event, protocol::Event::TextDone { text } if text == marker);
            finished |= matches!(&event,
                protocol::Event::WorkerState { snapshot }
                    if snapshot.last_finished_submission_request_id.as_deref() == Some(request_id)
                        && snapshot.state == protocol::WorkerState::Idle
            );
        }
    }).await.unwrap_or_else(|_| panic!("real Controller output deadline for {request_id}; seen {seen:?}"));
}

async fn controller_snapshot(stream: &mut ControllerSocket, saved_output: bool) {
    let snapshot = next_frame(stream).await;
    let protocol::Event::Snapshot { session, .. } = snapshot else {
        panic!("a Controller connection must start with its paired snapshot");
    };
    if saved_output {
        assert!(serde_json::to_string(&session).unwrap().contains(OLD_MARKER),
            "replacement snapshot must retain the original saved Session history");
    }
}

async fn drain_original_connection(stream: &mut ControllerSocket) {
    let mut seen = Vec::new();
    tokio::time::timeout(CONTROLLER_WAIT, async {
        loop {
            match stream.next().await {
                None | Some(Ok(Message::Close(_))) => break,
                Some(Ok(Message::Text(text))) => {
                    let event: protocol::Event = serde_json::from_str(&text).unwrap();
                    assert!(!matches!(&event,
                        protocol::Event::TextDelta { text } | protocol::Event::TextDone { text }
                            if text.contains(NEW_MARKER)
                    ), "replacement output leaked onto the original connection: {event:?}");
                    seen.push(format!("{event:?}"));
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("original connection did not end with Close/EOF: {other:?}"),
            }
        }
    }).await.unwrap_or_else(|_| panic!("original stream close deadline; buffered frames {seen:?}"));
}

#[tokio::test]
async fn protocol_ws_stop_closes_old_stream_and_restore_requires_new_connection() {
    let mut fixture = ControllerFixture::memory();
    let url = fixture.serve().await;
    let worker_ref = fixture.worker.worker_ref.clone();
    let original_transport = fixture.backend().clone().attach_worker_protocol(&worker_ref).unwrap();
    assert!(original_transport.validate().is_ok());
    let (mut old_stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
    controller_snapshot(&mut old_stream, false).await;
    controller_submit(&mut old_stream, "ws-original").await;
    controller_output(&mut old_stream, "ws-original", OLD_MARKER, false).await;
    let original_session = fixture.saved_session();
    let original = fixture.stop();
    assert!(original_transport.validate().is_err(), "original transport must be invalid after actual join");

    let restored = fixture.restore();
    assert_eq!(restored.status, crate::catalog::WorkerStatus::Idle);
    assert_eq!(fixture.saved_session().0, original_session.0);
    let replacement_transport = fixture.backend().clone().attach_worker_protocol(&worker_ref).unwrap();
    assert!(replacement_transport.validate().is_ok());
    let stale_stop = original_transport.stop(crate::execution::WorkerExecutionStopRequest {
        worker_ref: worker_ref.clone(), operation_id: original.stop_operation_id,
    });
    assert!(!stale_stop.is_accepted(), "a captured old transport must not stop the replacement");
    assert!(stale_stop.termination.is_none());
    assert!(replacement_transport.validate().is_ok());

    let (mut new_stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
    controller_snapshot(&mut new_stream, true).await;
    controller_submit(&mut new_stream, "ws-replacement").await;
    controller_output(&mut new_stream, "ws-replacement", NEW_MARKER, true).await;
    // Real Stop may leave old state/log/Shutdown frames buffered; only the bound
    // scope's report/join is termination evidence, not this socket draining.
    drain_original_connection(&mut old_stream).await;
    let replacement = fixture.finish().await;
    assert_ne!(original.execution_id, replacement.execution_id);
    assert_ne!(original.stop_operation_id, replacement.stop_operation_id);
    assert!(replacement_transport.validate().is_err());
    fixture.assert_model_calls(2);
}

#[tokio::test]
async fn protocol_ws_old_cursor_never_replays_stopped_execution_after_new_snapshot() {
    let mut fixture = ControllerFixture::memory();
    let url = fixture.serve().await;
    let worker_ref = fixture.worker.worker_ref.clone();
    let original_transport = fixture.backend().clone().attach_worker_protocol(&worker_ref).unwrap();
    let mut observation = fixture.runtime().subscribe_worker_observation().unwrap();
    let (mut old_stream, _) = connect_async(authed_ws_request(&url)).await.unwrap();
    controller_snapshot(&mut old_stream, false).await;
    controller_submit(&mut old_stream, "cursor-original").await;
    controller_output(&mut old_stream, "cursor-original", OLD_MARKER, false).await;
    let old = tokio::time::timeout(CONTROLLER_WAIT, async {
        loop {
            let event = observation.recv().await.expect("real Runtime observation relay");
            if event.worker_ref == worker_ref
                && matches!(&event.payload, protocol::Event::TextDone { text } if text == OLD_MARKER)
            {
                break event;
            }
        }
    }).await.expect("old output must have entered real Runtime observation history");
    assert!(WorkerObservationCursor::decode(&old.cursor).is_some());
    let original_session = fixture.saved_session();
    let original = fixture.stop();
    assert!(original_transport.validate().is_err());
    let restored = fixture.restore();
    assert_eq!(restored.status, crate::catalog::WorkerStatus::Idle);
    assert_eq!(fixture.saved_session().0, original_session.0);
    let replacement_transport = fixture.backend().clone().attach_worker_protocol(&worker_ref).unwrap();
    assert!(replacement_transport.validate().is_ok());

    let resume_url = format!("{url}?cursor={}", old.cursor);
    let (mut stream, _) = connect_async(authed_ws_request(&resume_url)).await.unwrap();
    controller_snapshot(&mut stream, true).await;
    controller_submit(&mut stream, "cursor-replacement").await;
    controller_output(&mut stream, "cursor-replacement", NEW_MARKER, true).await;
    drain_original_connection(&mut old_stream).await;
    let replacement = fixture.finish().await;
    assert_ne!(original.execution_id, replacement.execution_id);
    assert_ne!(original.stop_operation_id, replacement.stop_operation_id);
    assert!(replacement_transport.validate().is_err());
    fixture.assert_model_calls(2);
}
