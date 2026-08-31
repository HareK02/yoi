use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

use super::Socket as SocketContract;

type Writer = futures::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

pub struct Socket {
    writer: Writer,
    messages: mpsc::Receiver<Result<String, SocketError>>,
    reader_task: JoinHandle<()>,
}

#[derive(Debug, Error)]
pub enum SocketError {
    #[error("WebSocket transport failed: {0}")]
    WebSocket(#[from] tungstenite::Error),
}

impl Socket {
    pub async fn connect(request: Request<()>) -> Result<Self, SocketError> {
        let (stream, _) = connect_async(request).await?;
        let (writer, mut reader) = stream.split();
        let (message_tx, messages) = mpsc::channel(256);
        let reader_task = tokio::spawn(async move {
            loop {
                match reader.next().await {
                    Some(Ok(Message::Text(message))) => {
                        if message_tx.send(Ok(message.to_string())).await.is_err() {
                            return;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    Some(Ok(
                        Message::Binary(_)
                        | Message::Ping(_)
                        | Message::Pong(_)
                        | Message::Frame(_),
                    )) => {}
                    Some(Err(error)) => {
                        let _ = message_tx.send(Err(SocketError::WebSocket(error))).await;
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
    type Error = SocketError;

    async fn send(&mut self, message: String) -> Result<(), Self::Error> {
        self.writer.send(Message::Text(message.into())).await?;
        Ok(())
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
    use futures::{SinkExt, StreamExt};
    use protocol::stream::{decode_method, encode_event};
    use protocol::{Event, Method, WorkerStatus};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    use super::*;
    use crate::Client;

    #[tokio::test]
    async fn carries_typed_protocol_through_generic_client() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let message = socket.next().await.unwrap().unwrap();
            assert!(matches!(
                message,
                Message::Text(ref text)
                    if matches!(decode_method(text), Ok(Method::Run { .. }))
            ));
            let event = encode_event(&Event::Status {
                status: WorkerStatus::Idle,
            })
            .unwrap();
            socket.send(Message::Text(event.into())).await.unwrap();
        });

        let request = format!("ws://{address}").into_client_request().unwrap();
        let mut client = Client::new(Socket::connect(request).await.unwrap());
        client
            .send(&Method::run_text("hello"))
            .await
            .expect("send method");
        assert!(matches!(
            client.next_event().await,
            Ok(Some(Event::Status {
                status: WorkerStatus::Idle
            }))
        ));
        server.await.unwrap();
    }
}
