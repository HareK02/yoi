//! Explicit, ephemeral synchronization for owned Runtime process tests only.
//!
//! The nonce authenticates no Runtime action and is never a routing identity.
//! Socket failure, EOF and the fixed deadline only release the gate; the lifecycle
//! owner continues its original operation and remains responsible for cleanup.

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use protocol::WorkerId;
use serde::{Deserialize, Serialize};

use crate::error::RuntimeError;
use crate::execution::WorkerLifecycleOperationId;

pub const RESTORE_CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(8);
pub const MAX_RESTORE_CHECKPOINT_CONFIG_BYTES: usize = 4096;
pub const MAX_RESTORE_CHECKPOINT_ACK_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreCheckpointStage {
    PreparedNotCommitted,
    CommittedPreterminal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreCheckpointCommit {
    NotCommitted,
    Committed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreCheckpointConfig {
    pub endpoint: SocketAddr,
    pub worker_id: WorkerId,
    pub stage: RestoreCheckpointStage,
    pub nonce: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreCheckpointAck {
    pub nonce: String,
    pub stage: RestoreCheckpointStage,
    pub runtime_id: String,
    pub worker_id: WorkerId,
    pub operation_id: WorkerLifecycleOperationId,
    pub commit: RestoreCheckpointCommit,
    pub receipt_absent: bool,
    pub pid: u32,
}

fn invalid_config() -> RuntimeError {
    // Do not include serde errors, paths or input contents in process diagnostics.
    RuntimeError::InvalidRequest("invalid test restore checkpoint configuration".into())
}

impl RestoreCheckpointConfig {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        if !self.endpoint.ip().is_loopback()
            || self.endpoint.port() == 0
            || self.nonce.is_empty()
            || self.nonce.len() > 256
            || self.nonce.chars().any(char::is_control)
        {
            return Err(invalid_config());
        }
        Ok(())
    }

    pub fn from_path(path: &Path) -> Result<Self, RuntimeError> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A FIFO must not stall startup before its regular-file check.
            options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
        }
        let file = options.open(path).map_err(|_| invalid_config())?;
        let metadata = file.metadata().map_err(|_| invalid_config())?;
        if !metadata.is_file() || metadata.len() > MAX_RESTORE_CHECKPOINT_CONFIG_BYTES as u64 {
            return Err(invalid_config());
        }
        let mut bytes = Vec::new();
        file.take(MAX_RESTORE_CHECKPOINT_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid_config())?;
        if bytes.len() > MAX_RESTORE_CHECKPOINT_CONFIG_BYTES {
            return Err(invalid_config());
        }
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| invalid_config())?;
        config.validate()?;
        Ok(config)
    }
}

pub(crate) fn wait_for_release(config: &RestoreCheckpointConfig, ack: &RestoreCheckpointAck) {
    let _ = exchange(config, ack);
}

fn remaining(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::TimedOut))
}

fn exchange(config: &RestoreCheckpointConfig, ack: &RestoreCheckpointAck) -> std::io::Result<()> {
    let deadline = Instant::now() + RESTORE_CHECKPOINT_TIMEOUT;
    let mut bytes = serde_json::to_vec(ack)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_RESTORE_CHECKPOINT_ACK_BYTES {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    let mut stream = TcpStream::connect_timeout(&config.endpoint, remaining(deadline)?)?;
    // Each syscall shares one budget; partial writes cannot reset the deadline.
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?))?;
        let written = stream.write(pending)?;
        if written == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        }
        pending = &pending[written..];
    }
    stream.set_read_timeout(Some(remaining(deadline)?))?;
    let mut release = [0];
    stream.read_exact(&mut release)?;
    if release != [b'R'] {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidData));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
