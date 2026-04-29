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

    loop {
        tokio::select! {
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
                    Ok(Some(Method::GetHistory)) => {
                        let items = handle.shared_state.history();
                        let segments_per_user = handle.shared_state.user_segments();
                        // Embed `segments` on user-message JSON values so
                        // the TUI can re-render typed atoms on restore.
                        // Alignment: segments are recorded only for
                        // submissions made during the live session, never
                        // for seed history loaded via `SessionStart.history`
                        // (post-compaction). The seed user_messages always
                        // come first in worker history, so the last
                        // `segments_per_user.len()` user_messages are the
                        // ones that map 1:1 to the segments list.
                        let total_user_msgs =
                            items.iter().filter(|i| i.is_user_message()).count();
                        let skip = total_user_msgs.saturating_sub(segments_per_user.len());
                        let mut user_idx = 0usize;
                        let values = items
                            .iter()
                            .map(|item| {
                                let mut value =
                                    serde_json::to_value(item).expect("Item is Serialize");
                                if item.is_user_message() {
                                    if user_idx >= skip {
                                        let seg_idx = user_idx - skip;
                                        if let Some(obj) = value.as_object_mut() {
                                            let segs = serde_json::to_value(
                                                &segments_per_user[seg_idx],
                                            )
                                            .expect("Segment is Serialize");
                                            obj.insert("segments".into(), segs);
                                        }
                                    }
                                    user_idx += 1;
                                }
                                value
                            })
                            .collect();
                        let greeting = handle.shared_state.greeting.clone();
                        if writer
                            .write(&Event::History {
                                items: values,
                                greeting,
                            })
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
