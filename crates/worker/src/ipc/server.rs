use std::io;
use std::io::ErrorKind;
use std::path::PathBuf;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use tokio::net::UnixListener;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

use crate::controller::WorkerHandle;
use crate::ipc::protocol_session::{
    dispatch_worker_protocol_method, live_log_entry_event, subscribe_worker_protocol_session,
};
use protocol::{ErrorCode, Event};

/// Unix socket server for Worker Protocol.
///
/// Listens on the Worker's runtime directory socket path.
/// Each client connection gets bidirectional JSONL:
/// - Client writes Method lines → forwarded to WorkerController
/// - Worker events → written as Event lines to all connected clients
pub struct SocketServer {
    accept_task: Option<JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
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
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

        let accept_task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => {
                            let handle = handle.clone();
                            connections.spawn(handle_connection(stream, handle));
                        }
                        Err(_) => break,
                    },
                    completed = connections.join_next(), if !connections.is_empty() => {
                        let _ = completed;
                    }
                }
            }
            connections.shutdown().await;
        });

        Ok(Self {
            accept_task: Some(accept_task),
            shutdown: Some(shutdown_tx),
            path,
        })
    }

    /// Stop accepting connections and join the server task. Dropping its
    /// `JoinSet` cancels every active connection task before this returns.
    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.accept_task.take() {
            let _ = task.await;
        }
        let _ = tokio::fs::remove_file(&self.path).await;
    }

    /// The socket file path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.accept_task.take();
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

async fn handle_connection(stream: tokio::net::UnixStream, handle: WorkerHandle) {
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    // Hold the in-flight stream lock while taking the session-log mirror
    // snapshot. `LogEntry::AnnotatedAssistantItem` is mirror-only for live clients,
    // so a finalized assistant block must be observed either as an already
    // committed entry or as the still-present in-flight block. This lock
    // order matches `append_entry` (in-flight clear before sink publish) and
    // keeps the snapshot/live boundary gap-free.
    let mut streams = subscribe_worker_protocol_session(&handle);
    for alert in streams.alert_snapshot {
        if writer.write_event(&Event::Alert(alert)).await.is_err() {
            return;
        }
    }

    // Send the typed snapshot up front so late attachers can
    // reconstruct view state without an extra round trip.
    if writer.write_event(&streams.snapshot_event).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            // Live session-log entries → dispatched as the role-specific
            // wire events. `SegmentLogSink` only broadcasts committed log
            // entries with live UI meaning; `UserInput` travels this lane so
            // the visible user line is ordered with `SegmentStart` rotation.
            entry = streams.log_entries.recv() => {
                match entry {
                    Ok(entry) => {
                        if let Some(event) = live_log_entry_event(entry) {
                            if writer.write_event(&event).await.is_err() {
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
            event = streams.events.recv() => {
                match event {
                    Ok(event) => {
                        if writer.write_event(&event).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            // Client methods → handle or forward to controller
            method = reader.next_method() => {
                match method {
                    Ok(Some(method)) => {
                        if let Some(response) = dispatch_worker_protocol_method(&handle, method).await
                            && writer.write_event(&response).await.is_err()
                        {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(e) if is_peer_disconnect_read_error(&e) => break,
                    Err(e) => {
                        if writer
                            .write(&Event::Error {
                                code: ErrorCode::InvalidRequest,
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
}
