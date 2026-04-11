use std::io;

use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// JSONL line reader over an async byte stream.
///
/// Wraps the inner reader and deserialises each non‑empty line as `T`.
pub struct JsonLineReader<R> {
    inner: R,
}

impl<R: tokio::io::AsyncRead + Unpin> JsonLineReader<BufReader<R>> {
    /// Wrap a raw reader (internally creates a [`BufReader`]).
    pub fn new(reader: R) -> Self {
        Self {
            inner: BufReader::new(reader),
        }
    }
}

impl<R: AsyncBufRead + Unpin> JsonLineReader<R> {
    /// Read and deserialise the next non‑empty JSONL line.
    ///
    /// Returns `Ok(None)` on EOF.
    pub async fn next<T: DeserializeOwned>(&mut self) -> Result<Option<T>, io::Error> {
        let mut line = String::new();
        loop {
            line.clear();
            let n = self.inner.read_line(&mut line).await?;
            if n == 0 {
                return Ok(None); // EOF
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let value = serde_json::from_str(trimmed)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            return Ok(Some(value));
        }
    }
}

/// JSONL line writer over an async byte stream.
pub struct JsonLineWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> JsonLineWriter<W> {
    pub fn new(writer: W) -> Self {
        Self { inner: writer }
    }

    /// Serialise `value` as a single JSONL line and flush.
    pub async fn write<T: Serialize>(&mut self, value: &T) -> Result<(), io::Error> {
        let json = serde_json::to_string(value)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.inner.write_all(json.as_bytes()).await?;
        self.inner.write_all(b"\n").await?;
        self.inner.flush().await?;
        Ok(())
    }
}
