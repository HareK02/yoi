use std::error::Error;
use std::fmt;

use protocol::stream::{decode_event, encode_method};
use protocol::{Event, Method};

use crate::transport::Socket;

/// Typed Worker protocol client over an injected message transport.
pub struct Client<T> {
    socket: T,
}

#[derive(Debug)]
pub enum ClientError<E> {
    Transport(E),
    Protocol(serde_json::Error),
}

impl<T> Client<T> {
    pub fn new(socket: T) -> Self {
        Self { socket }
    }

    pub fn into_inner(self) -> T {
        self.socket
    }
}

impl<T: Socket> Client<T> {
    pub async fn send(&mut self, method: &Method) -> Result<(), ClientError<T::Error>> {
        let message = encode_method(method).map_err(ClientError::Protocol)?;
        self.socket
            .send(message)
            .await
            .map_err(ClientError::Transport)
    }

    pub async fn next_event(&mut self) -> Result<Option<Event>, ClientError<T::Error>> {
        self.socket
            .next()
            .await
            .map_err(ClientError::Transport)?
            .map(|message| decode_event(&message).map_err(ClientError::Protocol))
            .transpose()
    }

    pub fn try_next_event(&mut self) -> Result<Option<Event>, ClientError<T::Error>> {
        self.socket
            .try_next()
            .map_err(ClientError::Transport)?
            .map(|message| decode_event(&message).map_err(ClientError::Protocol))
            .transpose()
    }
}

impl<E: fmt::Display> fmt::Display for ClientError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "Worker transport error: {error}"),
            Self::Protocol(error) => write!(formatter, "Worker protocol error: {error}"),
        }
    }
}

impl<E: Error + 'static> Error for ClientError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Protocol(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::convert::Infallible;

    use async_trait::async_trait;
    use protocol::stream::{decode_method, encode_event};
    use protocol::{Event, Method, WorkerStatus};

    use super::Client;
    use crate::transport::Socket;

    #[derive(Default)]
    struct TestSocket {
        sent: Vec<String>,
        incoming: VecDeque<String>,
    }

    #[async_trait]
    impl Socket for TestSocket {
        type Error = Infallible;

        async fn send(&mut self, message: String) -> Result<(), Self::Error> {
            self.sent.push(message);
            Ok(())
        }

        async fn next(&mut self) -> Result<Option<String>, Self::Error> {
            Ok(self.incoming.pop_front())
        }

        fn try_next(&mut self) -> Result<Option<String>, Self::Error> {
            Ok(self.incoming.pop_front())
        }
    }

    #[tokio::test]
    async fn encodes_methods_and_decodes_events_above_transport() {
        let mut socket = TestSocket::default();
        socket.incoming.push_back(
            encode_event(&Event::Status {
                status: WorkerStatus::Idle,
            })
            .expect("encode event"),
        );
        let mut client = Client::new(socket);

        client
            .send(&Method::run_text("hello"))
            .await
            .expect("send method");
        assert!(matches!(
            decode_method(&client.socket.sent[0]),
            Ok(Method::Run { .. })
        ));
        assert!(matches!(
            client.next_event().await,
            Ok(Some(Event::Status {
                status: WorkerStatus::Idle
            }))
        ));
    }
}
