use std::error::Error;

use async_trait::async_trait;

pub mod in_process;
pub mod unix_socket;
pub mod websocket;

/// Message-oriented transport for one Worker protocol connection.
///
/// Implementations own physical framing. `client::Client` owns the typed
/// Method/Event protocol encoding layered on top of these UTF-8 messages.
#[async_trait]
pub trait Socket {
    type Error: Error + Send + Sync + 'static;

    async fn send(&mut self, message: String) -> Result<(), Self::Error>;

    async fn next(&mut self) -> Result<Option<String>, Self::Error>;

    fn try_next(&mut self) -> Result<Option<String>, Self::Error>;
}
