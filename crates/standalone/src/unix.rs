//! Optional operator transport owned by the Standalone Host, never by a Worker.
//!
//! The endpoint's existing parent must be private to the current Unix user. Binding never
//! removes an existing entry, including stale sockets. Recovery is an explicit operator action.
//! Peers must have that same uid; admitted methods still cross the normal local protocol
//! authorization boundary. Disconnect only drops a view, not the Host or its execution lease.

use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use client::transport::Socket;
use protocol::stream::{JsonLineWriter, decode_event, decode_method};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

/// A bound endpoint reservation. Drop removes only the exact socket created by this bind.
/// It can be reserved before starting a Host, so bind failures do not allocate Workers.
pub struct UnixEndpoint {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl UnixEndpoint {
    pub fn bind(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref();
        let name = path.file_name().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "endpoint requires a filename")
        })?;
        let parent = std::fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let metadata = std::fs::metadata(&parent)?;
        // SAFETY: geteuid has no preconditions and does not mutate process identity.
        let uid = unsafe { libc::geteuid() };
        if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "endpoint parent must be a current-user-owned private directory (mode 0700)",
            ));
        }
        let path = parent.join(name);
        let listener = UnixListener::bind(&path)?;
        let metadata = std::fs::symlink_metadata(&path)?;
        let endpoint = Self {
            listener,
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        std::fs::set_permissions(&endpoint.path, std::fs::Permissions::from_mode(0o600))?;
        Ok(endpoint)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for UnixEndpoint {
    fn drop(&mut self) {
        if let Ok(current) = std::fs::symlink_metadata(&self.path)
            && current.file_type().is_socket()
            && current.dev() == self.device
            && current.ino() == self.inode
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) struct UnixAdapter {
    task: Option<JoinHandle<io::Result<()>>>,
    stop: Option<oneshot::Sender<()>>,
}

impl UnixAdapter {
    pub(crate) fn start(endpoint: UnixEndpoint, handle: worker::WorkerHandle) -> Self {
        let (stop, mut stopped) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let result = loop {
                tokio::select! {
                    _ = &mut stopped => break Ok(()),
                    accepted = endpoint.listener.accept(), if connections.len() < 64 => {
                        match accepted {
                            Ok((stream, _)) => {
                                // The filesystem gate protects connection establishment; credentials
                                // also fence open connections to the Host's operator identity.
                                let allowed = stream.peer_cred().is_ok_and(|cred| {
                                    // SAFETY: geteuid has no preconditions.
                                    cred.uid() == unsafe { libc::geteuid() }
                                });
                                if allowed {
                                    let socket = crate::host::connect_protocol(handle.clone());
                                    connections.spawn(bridge(stream, socket));
                                }
                            }
                            Err(error) => break Err(error),
                        }
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            };
            connections.shutdown().await;
            // Keep the bound endpoint alive until every connection has been reclaimed.
            drop(endpoint);
            result
        });
        Self {
            task: Some(task),
            stop: Some(stop),
        }
    }

    pub(crate) async fn ended(&mut self) -> io::Result<()> {
        let result = self.task.as_mut().expect("adapter task").await;
        self.task.take();
        result.map_err(io::Error::other)?
    }

    pub(crate) async fn shutdown(mut self) -> io::Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if self.task.is_some() {
            self.ended().await?;
        }
        Ok(())
    }
}

impl Drop for UnixAdapter {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Sending stop lets the task join its connection set and drop its endpoint.
        // A Host dropped without confirmed shutdown keeps the production execution lease.
    }
}

async fn bridge(
    stream: UnixStream,
    mut socket: client::transport::in_process::Socket,
) -> io::Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader).lines();
    let mut writer = JsonLineWriter::new(writer);
    loop {
        tokio::select! {
            line = reader.next_line() => {
                let Some(message) = line? else { return Ok(()); };
                if message.trim().is_empty() { continue; }
                socket.send(message).await.map_err(io::Error::other)?;
            }
            event = socket.next() => {
                let Some(message) = event.map_err(io::Error::other)? else { return Ok(()); };
                let event = decode_event(&message).map_err(io::Error::other)?;
                writer.write_event(&event).await?;
            }
        }
    }
}

/// Connect a JSONL operator client. EOF disconnects without issuing Shutdown.
/// Input is decoded as Worker Methods and output contains Worker Events only.
pub async fn run_unix_client(endpoint: PathBuf) -> io::Result<()> {
    relay_unix_client(endpoint, tokio::io::stdin(), tokio::io::stdout()).await
}

/// The CLI's stream relay, also usable by embedders that own their input/output streams.
pub async fn relay_unix_client<R, W>(endpoint: PathBuf, input: R, output: W) -> io::Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let stream = UnixStream::connect(endpoint).await?;
    let (reader, writer) = stream.into_split();
    let mut input = BufReader::new(input).lines();
    let mut to_host = JsonLineWriter::new(writer);
    let mut from_host = BufReader::new(reader).lines();
    let mut output = JsonLineWriter::new(output);
    loop {
        tokio::select! {
            line = input.next_line() => {
                let Some(line) = line? else { return Ok(()); };
                if line.trim().is_empty() { continue; }
                let method = decode_method(&line).map_err(io::Error::other)?;
                to_host.write(&method).await?;
            }
            line = from_host.next_line() => {
                let Some(line) = line? else { return Ok(()); };
                let event = decode_event(&line).map_err(io::Error::other)?;
                output.write_event(&event).await?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn endpoint_requires_private_parent_and_is_owner_only() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host.sock");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let error = UnixEndpoint::bind(&path)
            .err()
            .expect("public parent rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(!path.exists());
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = UnixEndpoint::bind(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let client = UnixStream::connect(&path).await.unwrap();
        let (server, _) = endpoint.listener.accept().await.unwrap();
        assert_eq!(
            client.peer_cred().unwrap().uid(),
            server.peer_cred().unwrap().uid()
        );
        drop(endpoint);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn bind_failure_precedes_worker_state_and_startup_failure_releases_only_its_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("host.sock");
        let owner = UnixEndpoint::bind(&path).unwrap();
        let state_dir = directory.path().join("must-not-be-opened");
        let error = crate::run_headless(
            crate::HeadlessLaunch::Restore {
                state_dir: state_dir.clone(),
                worker_id: protocol::WorkerId::now_v7(),
            },
            path.clone(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, crate::HeadlessError::Transport(_)));
        assert!(!state_dir.exists());
        assert!(path.exists());
        drop(owner);
        let error = crate::run_headless(
            crate::HeadlessLaunch::Restore {
                state_dir,
                worker_id: protocol::WorkerId::now_v7(),
            },
            path.clone(),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, crate::HeadlessError::Startup(_)));
        assert!(
            !path.exists(),
            "a failed Host startup must release its own bind"
        );
    }

    #[tokio::test]
    async fn bind_failure_and_old_endpoint_drop_preserve_another_owner() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = directory.path().join("host.sock");
        let first = UnixEndpoint::bind(&path).unwrap();
        let first_inode = std::fs::metadata(&path).unwrap().ino();
        assert_eq!(
            UnixEndpoint::bind(&path).err().unwrap().kind(),
            io::ErrorKind::AddrInUse
        );
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), first_inode);
        std::fs::rename(&path, directory.path().join("retained.sock")).unwrap();
        let second = UnixEndpoint::bind(&path).unwrap();
        let second_inode = std::fs::metadata(&path).unwrap().ino();
        assert_ne!(first_inode, second_inode);
        drop(first);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), second_inode);
        let _client = UnixStream::connect(&path).await.unwrap();
        let _connection = second.listener.accept().await.unwrap();
        drop(second);
        assert!(!path.exists());
    }
}
