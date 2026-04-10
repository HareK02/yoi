use std::io;
use std::path::Path;

use protocol::{Event, Method};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

pub struct PodClient {
    writer: tokio::io::WriteHalf<UnixStream>,
    event_rx: mpsc::Receiver<Event>,
}

impl PodClient {
    pub async fn connect(path: &Path) -> Result<Self, io::Error> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = tokio::io::split(stream);

        let (event_tx, event_rx) = mpsc::channel::<Event>(256);

        tokio::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.is_empty() {
                    continue;
                }
                if let Ok(event) = serde_json::from_str::<Event>(&line) {
                    if event_tx.send(event).await.is_err() {
                        break;
                    }
                }
            }
        });

        Ok(Self { writer, event_rx })
    }

    pub async fn send(&mut self, method: &Method) -> Result<(), io::Error> {
        let json = serde_json::to_string(method)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.writer.write_all(json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }
}
