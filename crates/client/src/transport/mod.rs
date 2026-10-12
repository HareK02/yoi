//! Message transport for one Worker connection.
//!
//! Separates in-process, Unix socket, and WS framing from protocol Method/Event encoding.
//! Changing transports does not change ownership of Worker execution, input admission, or receipts.

use std::error::Error;

use async_trait::async_trait;

pub mod in_process;
pub mod unix_socket;
pub mod websocket;

/// UTF-8 message transport for a Worker protocol connection.
///
/// Implementations own physical framing; the higher-level Client encodes and decodes Method/Event.
#[async_trait]
pub trait Socket {
    type Error: Error + Send + Sync + 'static;

    async fn send(&mut self, message: String) -> Result<(), Self::Error>;

    async fn next(&mut self) -> Result<Option<String>, Self::Error>;

    fn try_next(&mut self) -> Result<Option<String>, Self::Error>;
}
