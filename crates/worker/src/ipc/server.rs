use std::io;
use std::io::ErrorKind;
use std::path::PathBuf;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use crate::controller::WorkerHandle;
use protocol::{Event, Method};

/// Unix socket server for Worker Protocol.
///
/// Listens on the Worker's runtime directory socket path.
/// Each client connection gets bidirectional JSONL:
/// - Client writes Method lines → forwarded to WorkerController
/// - Worker events → written as Event lines to all connected clients
pub struct SocketServer {
    _accept_task: JoinHandle<()>,
    path: PathBuf,
}

impl SocketServer {
    /// Start listening on the WorkerHandle's socket path.
    pub async fn start(handle: &WorkerHandle) -> Result<Self, io::Error> {
        let path = handle.runtime_dir.socket_path();

        // Remove stale socket file if it exists
        let _ = tokio::fs::remove_file(&path).await;

        let listener = UnixListener::bind(&path)?;
        let handle = handle.clone();

        let _accept_task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let handle = handle.clone();
                        tokio::spawn(handle_connection(stream, handle));
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self { _accept_task, path })
    }

    /// The socket file path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn is_peer_disconnect_read_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ConnectionReset
            | ErrorKind::ConnectionAborted
            | ErrorKind::BrokenPipe
            | ErrorKind::UnexpectedEof
    )
}

fn live_entry_event(entry: session_store::LogEntry) -> Option<Event> {
    match entry {
        session_store::LogEntry::SegmentStart { .. } => {
            let value = serde_json::to_value(&entry).expect("LogEntry is Serialize");
            Some(Event::SegmentRotated { entry: value })
        }
        session_store::LogEntry::UserInput { segments, .. } => {
            Some(Event::UserMessage { segments })
        }
        session_store::LogEntry::SystemItem { item, .. } => {
            let value = serde_json::to_value(&item).expect("SystemItem is Serialize");
            Some(Event::SystemItem { item: value })
        }
        session_store::LogEntry::Invoke { trigger, .. } => {
            Some(Event::InvokeStart { kind: trigger })
        }
        other => {
            // `SegmentLogSink::is_live_relevant` keeps non-live-relevant
            // variants off the broadcast lane; reaching here means the two
            // are out of sync and we silently dropped a wire event. Log so a
            // future regression surfaces instead of vanishing.
            tracing::error!(
                entry_kind = ?std::mem::discriminant(&other),
                "session-log broadcast emitted a non-live-relevant entry; \
                 sink filter and IPC dispatch are out of sync"
            );
            None
        }
    }
}

async fn handle_connection(stream: tokio::net::UnixStream, handle: WorkerHandle) {
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    // Hold the in-flight stream lock while taking the session-log mirror
    // snapshot. `LogEntry::AssistantItem` is mirror-only for live clients,
    // so a finalized assistant block must be observed either as an already
    // committed entry or as the still-present in-flight block. This lock
    // order matches `append_entry` (in-flight clear before sink publish) and
    // keeps the snapshot/live boundary gap-free.
    let (snapshot_event, mut entry_rx) = handle.snapshot_event_with_entry_subscription();
    // Atomically subscribe and snapshot buffered alerts so that warnings
    // emitted before this client connected are replayed exactly once.
    let (alert_snapshot, mut rx) = handle.alerter.subscribe_with_snapshot();
    for alert in alert_snapshot {
        if writer.write(&Event::Alert(alert)).await.is_err() {
            return;
        }
    }

    // Send the typed snapshot up front so late attachers can
    // reconstruct view state without an extra round trip.
    if writer.write(&snapshot_event).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            // Live session-log entries → dispatched as the role-specific
            // wire events. `SegmentLogSink` only broadcasts committed log
            // entries with live UI meaning; `UserInput` travels this lane so
            // the visible user line is ordered with `SegmentStart` rotation.
            entry = entry_rx.recv() => {
                match entry {
                    Ok(entry) => {
                        if let Some(event) = live_entry_event(entry) {
                            if writer.write(&event).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Slow client fell behind the broadcast buffer.
                        // Drop the connection so the next reconnect
                        // re-seeds the prefix via subscribe_with_snapshot.
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            // Broadcast events → this client
            event = rx.recv() => {
                match event {
                    Ok(event) => {
                        if writer.write(&event).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // Client methods → handle or forward to controller
            method = reader.next::<Method>() => {
                match method {
                    Ok(Some(Method::ListCompletions { kind, prefix })) => {
                        let entries = handle.completion_entries(kind, &prefix);
                        if writer
                            .write(&Event::Completions { kind, entries })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Ok(Some(method)) => {
                        let _ = handle.send(method).await;
                    }
                    Ok(None) => break,
                    Err(e) if is_peer_disconnect_read_error(&e) => break,
                    Err(e) => {
                        if writer
                            .write(&Event::Error {
                                code: protocol::ErrorCode::InvalidRequest,
                                message: format!("invalid method: {e}"),
                            })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_disconnect_read_errors_are_connection_close() {
        for kind in [
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionAborted,
            ErrorKind::BrokenPipe,
            ErrorKind::UnexpectedEof,
        ] {
            let error = io::Error::new(kind, "peer disconnected");
            assert!(
                is_peer_disconnect_read_error(&error),
                "{kind:?} should be treated as a normal peer disconnect"
            );
        }
    }

    #[test]
    fn invalid_data_is_not_peer_disconnect() {
        let error = io::Error::new(ErrorKind::InvalidData, "malformed method");
        assert!(!is_peer_disconnect_read_error(&error));
    }

    #[test]
    fn user_input_log_entry_maps_to_user_message_event() {
        let segments = vec![protocol::Segment::text("hello from log")];
        let event = live_entry_event(session_store::LogEntry::UserInput {
            ts: session_store::segment_log::now_millis(),
            segments: segments.clone(),
        })
        .expect("UserInput must be live-relevant");

        match event {
            Event::UserMessage { segments: echoed } => assert_eq!(echoed, segments),
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }
}
