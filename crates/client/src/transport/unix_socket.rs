use std::io;
use std::path::Path;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::Socket as SocketContract;

pub struct Socket {
    writer: tokio::io::WriteHalf<UnixStream>,
    messages: mpsc::Receiver<io::Result<String>>,
    reader_task: JoinHandle<()>,
}

impl Socket {
    pub async fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = tokio::io::split(stream);
        let (message_tx, messages) = mpsc::channel(256);
        let reader_task = tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(message)) if message.trim().is_empty() => {}
                    Ok(Some(message)) => {
                        if message_tx.send(Ok(message)).await.is_err() {
                            return;
                        }
                    }
                    Ok(None) => return,
                    Err(error) => {
                        let _ = message_tx.send(Err(error)).await;
                        return;
                    }
                }
            }
        });
        Ok(Self {
            writer,
            messages,
            reader_task,
        })
    }
}

#[async_trait]
impl SocketContract for Socket {
    type Error = io::Error;

    async fn send(&mut self, message: String) -> Result<(), Self::Error> {
        self.writer.write_all(message.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await
    }

    async fn next(&mut self) -> Result<Option<String>, Self::Error> {
        match self.messages.recv().await {
            Some(message) => message.map(Some),
            None => Ok(None),
        }
    }

    fn try_next(&mut self) -> Result<Option<String>, Self::Error> {
        match self.messages.try_recv() {
            Ok(message) => message.map(Some),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                Ok(None)
            }
        }
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::time::Duration;

    use protocol::stream::{decode_method, encode_event};
    use protocol::{Event, Method, WorkerStatus};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::*;
    use crate::Client;

    async fn assert_peer_closed(stream: &mut UnixStream, reason: &str) {
        let mut buf = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buf))
            .await
            .expect(reason)
        {
            Ok(0) => {}
            Err(error) if error.kind() == ErrorKind::ConnectionReset => {}
            Ok(n) => panic!("server should observe peer close, read {n} byte(s)"),
            Err(error) => panic!("server read failed unexpectedly: {error}"),
        }
    }

    #[tokio::test]
    async fn client_receives_events_over_unix_socket() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("events.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let event = encode_event(&Event::Status {
                status: WorkerStatus::Idle,
            })
            .unwrap();
            stream.write_all(event.as_bytes()).await.unwrap();
            stream.write_all(b"\n").await.unwrap();
        });

        let mut client = Client::new(Socket::connect(&socket_path).await.unwrap());
        let event = tokio::time::timeout(Duration::from_secs(1), client.next_event())
            .await
            .expect("client should receive event while alive")
            .expect("transport should succeed");
        assert!(matches!(
            event,
            Some(Event::Status {
                status: WorkerStatus::Idle
            })
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn client_sends_methods_over_unix_socket() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("send.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (reader, _) = listener.accept().await.unwrap();
            BufReader::new(reader).lines().next_line().await.unwrap()
        });

        let mut client = Client::new(Socket::connect(&socket_path).await.unwrap());
        client
            .send(&Method::run_text("hello"))
            .await
            .expect("send method");

        let received = server.await.unwrap().expect("method message");
        assert!(matches!(decode_method(&received), Ok(Method::Run { .. })));
    }

    #[tokio::test]
    async fn dropping_socket_closes_server_connection() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("drop.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_peer_closed(&mut stream, "dropped socket should close promptly").await;
        });

        let socket = Socket::connect(&socket_path).await.unwrap();
        drop(socket);
        server.await.unwrap();
    }
}
