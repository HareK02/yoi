use serde::{Deserialize, Serialize};
use std::fmt;

/// Runtime-local Worker identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerId(u64);

impl WorkerId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn parse(value: &str) -> Option<Self> {
        value.parse::<u64>().ok().map(Self)
    }

    pub(crate) fn generated(sequence: u64) -> Self {
        Self(sequence)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Runtime-local authority reference for Worker operations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerRef {
    pub worker_id: WorkerId,
}

impl WorkerRef {
    pub fn new(worker_id: WorkerId) -> Self {
        Self { worker_id }
    }
}
