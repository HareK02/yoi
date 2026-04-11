use std::io;
use std::path::Path;

use protocol::stream::{JsonLineReader, JsonLineWriter};
use protocol::{Event, Method};
use tokio::net::UnixStream;
use tokio::sync::mpsc;

pub struct PodClient {
    writer: JsonLineWriter<tokio::io::WriteHalf<UnixStream>>,
    event_rx: mpsc::Receiver<Event>,
}

impl PodClient {
    pub async fn connect(path: &Path) -> Result<Self, io::Error> {
        let stream = UnixStream::connect(path).await?;
        let (reader, writer) = tokio::io::split(stream);
        let writer = JsonLineWriter::new(writer);

        let (event_tx, event_rx) = mpsc::channel::<Event>(256);

        tokio::spawn(async move {
            let mut reader = JsonLineReader::new(reader);
            while let Ok(Some(event)) = reader.next::<Event>().await {
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        Ok(Self { writer, event_rx })
    }

    pub async fn send(&mut self, method: &Method) -> Result<(), io::Error> {
        self.writer.write(method).await
    }

    pub async fn next_event(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }
}
