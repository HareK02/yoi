use std::io;
use std::path::Path;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{Event, Method};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct WorkerClient {
    writer: JsonLineWriter<tokio::io::WriteHalf<UnixStream>>,
    event_rx: mpsc::Receiver<Event>,
    reader_task: JoinHandle<()>,
}

impl WorkerClient {
    pub async fn connect(path: &Path) -> Result<Self, io::Error> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = tokio::io::split(stream);
        let writer = JsonLineWriter::new(writer);

        let (event_tx, event_rx) = mpsc::channel::<Event>(256);

        let reader_task = tokio::spawn(async move {
            let mut reader = JsonLineReader::new(reader);
            while let Ok(Some(event)) = reader.next::<Event>().await {
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self {
            writer,
            event_rx,
            reader_task,
        })
    }

    pub async fn send(&mut self, method: &Method) -> Result<(), io::Error> {
        self.writer.write(method).await
    }

    pub fn try_next_event(&mut self) -> Option<Event> {
        self.event_rx.try_recv().ok()
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }
}

impl Drop for WorkerClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::time::Duration;

    use protocol::{Segment, WorkerStatus};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::*;

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
    async fn receives_events_while_client_is_alive() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("events.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut writer = JsonLineWriter::new(stream);
            writer
                .write(&Event::Status {
                    status: WorkerStatus::Idle,
                })
                .await
                .unwrap();
        });

        let mut client = WorkerClient::connect(&socket_path).await.unwrap();

        let event = tokio::time::timeout(Duration::from_secs(1), client.next_event())
            .await
            .expect("client should receive event while alive");
        assert!(matches!(
            event,
            Some(Event::Status {
                status: WorkerStatus::Idle
            })
        ));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn send_writes_methods_while_client_is_alive() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("send.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut reader = JsonLineReader::new(stream);
            reader.next::<Method>().await.unwrap()
        });

        let mut client = WorkerClient::connect(&socket_path).await.unwrap();
        let method = Method::Run {
            input: vec![Segment::text("hello")],
        };
        client.send(&method).await.unwrap();

        let received = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server should receive method while client is alive")
            .unwrap();
        match received {
            Some(Method::Run { input }) => assert_eq!(input, vec![Segment::text("hello")]),
            other => panic!("expected Run method, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dropping_repeated_clients_closes_server_connections() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("drop.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..16 {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_peer_closed(
                    &mut stream,
                    "dropped client should close its socket promptly",
                )
                .await;
            }
        });

        for _ in 0..16 {
            let client = WorkerClient::connect(&socket_path).await.unwrap();
            drop(client);
        }

        server.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_client_aborts_blocked_reader_task() {
        let socket_dir = tempdir().unwrap();
        let socket_path = socket_dir.path().join("blocked-reader.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(b"{\"event\"").await.unwrap();
            assert_peer_closed(
                &mut stream,
                "aborting the blocked client reader should close the socket",
            )
            .await;
        });

        let client = WorkerClient::connect(&socket_path).await.unwrap();
        tokio::task::yield_now().await;
        drop(client);

        server.await.unwrap();
    }
}
