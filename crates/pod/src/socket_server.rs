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

        Ok(Self {
            _accept_task,
            path,
        })
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
    let mut rx = handle.subscribe();

    // Event writer: broadcast events → socket
    let write_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            if writer.write(&event).await.is_err() {
                break;
            }
        }
    });

    // Method reader: socket → controller
    loop {
        match reader.next::<Method>().await {
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

    // Client disconnected — stop the write task
    write_task.abort();
}
