use std::io;
use std::path::PathBuf;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

use crate::controller::PodHandle;
use protocol::{Event, Method};

/// Unix socket server for Pod Protocol.
///
/// Listens on the Pod's runtime directory socket path.
/// Each client connection gets bidirectional JSONL:
/// - Client writes Method lines → forwarded to PodController
/// - Pod events → written as Event lines to all connected clients
pub struct SocketServer {
    _accept_task: JoinHandle<()>,
    path: PathBuf,
}

impl SocketServer {
    /// Start listening on the PodHandle's socket path.
    pub async fn start(handle: &PodHandle) -> Result<Self, io::Error> {
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

async fn handle_connection(stream: tokio::net::UnixStream, handle: PodHandle) {
    let (reader, writer) = stream.into_split();
    let mut reader = JsonLineReader::new(reader);
    let mut writer = JsonLineWriter::new(writer);

    // Atomically subscribe to the session-log mirror first. The
    // returned (snapshot, rx) pair partitions the entry timeline:
    // entries committed before this call appear in `entries`, every
    // entry after lands on `entry_rx`. Doing this before the alert
    // snapshot keeps both ordering pairs internally consistent.
    let (entries_snapshot, mut entry_rx) = handle.sink.subscribe_with_snapshot();

    // Atomically subscribe and snapshot buffered alerts so that
    // warnings emitted before this client connected are replayed
    // exactly once — they appear in the snapshot, and any alert
    // arriving afterwards reaches us through `rx`.
    let (alert_snapshot, mut rx) = handle.alerter.subscribe_with_snapshot();
    for alert in alert_snapshot {
        if writer.write(&Event::Alert(alert)).await.is_err() {
            return;
        }
    }

    // Send the typed snapshot up front so late attachers can
    // reconstruct view state without an extra round trip.
    let snapshot_event = Event::Snapshot {
        entries: entries_snapshot
            .into_iter()
            .map(|e| serde_json::to_value(&e).expect("LogEntry is Serialize"))
            .collect(),
        greeting: handle.shared_state.greeting.clone(),
        status: handle.shared_state.get_status(),
    };
    if writer.write(&snapshot_event).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            // Live session-log entries → dispatched as the role-specific
            // wire events. The sink only broadcasts entries that the
            // streaming-event lane doesn't cover; everything else is
            // already on the wire via TextDelta / ToolCall* / etc., so we
            // never see (and never need to forward) other variants here.
            entry = entry_rx.recv() => {
                match entry {
                    Ok(entry) => {
                        let value = serde_json::to_value(&entry)
                            .expect("LogEntry is Serialize");
                        let outbound = match &entry {
                            session_store::LogEntry::SessionStart { .. } => {
                                Some(Event::SessionRotated { entry: value })
                            }
                            session_store::LogEntry::HookInjectedItems { .. } => {
                                Some(Event::HookInjectedItems { entry: value })
                            }
                            // Defensive: should never reach here per
                            // `SessionLogSink::is_live_relevant`.
                            _ => None,
                        };
                        if let Some(event) = outbound
                            && writer.write(&event).await.is_err()
                        {
                            break;
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
                        let entries = match kind {
                            protocol::CompletionKind::File => handle
                                .shared_state
                                .fs_view()
                                .map(|view| view.list_file_completions(&prefix))
                                .unwrap_or_default()
                                .into_iter()
                                .map(|c| protocol::CompletionEntry {
                                    value: c.path,
                                    is_dir: c.is_dir,
                                })
                                .collect(),
                            protocol::CompletionKind::Knowledge => handle
                                .shared_state
                                .list_knowledge_completions(&prefix)
                                .into_iter()
                                .map(|c| protocol::CompletionEntry {
                                    value: c.slug,
                                    is_dir: false,
                                })
                                .collect(),
                            protocol::CompletionKind::Workflow => handle
                                .shared_state
                                .list_workflow_completions(&prefix)
                                .into_iter()
                                .map(|c| protocol::CompletionEntry {
                                    value: c.slug,
                                    is_dir: false,
                                })
                                .collect(),
                        };
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
                    Err(e) => {
                        let _ = handle.send_event(Event::Error {
                            code: protocol::ErrorCode::Internal,
                            message: format!("invalid method: {e}"),
                        });
                    }
                }
            }
        }
    }
}
