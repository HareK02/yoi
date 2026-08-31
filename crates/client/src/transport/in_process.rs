use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc;

use super::Socket as SocketContract;

const CHANNEL_CAPACITY: usize = 256;

pub struct Socket {
    outgoing: mpsc::Sender<String>,
    incoming: mpsc::Receiver<String>,
}

/// Host-side endpoint paired with an in-process client transport.
pub struct Peer {
    incoming: mpsc::Receiver<String>,
    outgoing: mpsc::Sender<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SocketError {
    #[error("in-process Worker protocol transport closed")]
    Closed,
}

impl Socket {
    pub fn pair() -> (Self, Peer) {
        let (client_tx, peer_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (peer_tx, client_rx) = mpsc::channel(CHANNEL_CAPACITY);
        (
            Self {
                outgoing: client_tx,
                incoming: client_rx,
            },
            Peer {
                incoming: peer_rx,
                outgoing: peer_tx,
            },
        )
    }
}

#[async_trait]
impl SocketContract for Socket {
    type Error = SocketError;

    async fn send(&mut self, message: String) -> Result<(), Self::Error> {
        self.outgoing
            .send(message)
            .await
            .map_err(|_| SocketError::Closed)
    }

    async fn next(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.incoming.recv().await)
    }

    fn try_next(&mut self) -> Result<Option<String>, Self::Error> {
        match self.incoming.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                Ok(None)
            }
        }
    }
}

impl Peer {
    pub async fn next(&mut self) -> Option<String> {
        self.incoming.recv().await
    }

    pub async fn send(&self, message: String) -> Result<(), String> {
        self.outgoing.send(message).await.map_err(|error| error.0)
    }
}

#[cfg(test)]
mod tests {
    use protocol::stream::{decode_method, encode_event};
    use protocol::{Event, Method, WorkerStatus};

    use super::Socket;
    use crate::Client;

    #[tokio::test]
    async fn pair_carries_typed_protocol_through_generic_client() {
        let (socket, mut peer) = Socket::pair();
        let mut client = Client::new(socket);

        client
            .send(&Method::run_text("hello"))
            .await
            .expect("send method");
        assert!(matches!(
            peer.next().await.as_deref().map(decode_method),
            Some(Ok(Method::Run { .. }))
        ));

        peer.send(
            encode_event(&Event::Status {
                status: WorkerStatus::Idle,
            })
            .expect("encode event"),
        )
        .await
        .expect("send event");
        assert!(matches!(
            client.next_event().await,
            Ok(Some(Event::Status {
                status: WorkerStatus::Idle
            }))
        ));
    }
}
