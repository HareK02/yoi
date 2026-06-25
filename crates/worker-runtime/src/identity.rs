use serde::{Deserialize, Serialize};
use std::fmt;

/// Public Runtime identity.
///
/// This is the first half of Worker authority.  Runtime APIs that operate on a
/// Worker require this id alongside a [`WorkerId`]; socket paths, session paths,
/// and display names are deliberately not authority.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimeId(String);

impl RuntimeId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    pub(crate) fn generated(sequence: u64) -> Self {
        Self(format!("runtime-mem-{sequence:016x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RuntimeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Runtime-local Worker identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerId(String);

impl WorkerId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            None
        } else {
            Some(Self(value))
        }
    }

    pub(crate) fn generated(sequence: u64) -> Self {
        Self(format!("worker-{sequence:08x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Complete public authority reference for Worker operations.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerRef {
    pub runtime_id: RuntimeId,
    pub worker_id: WorkerId,
}

impl WorkerRef {
    pub fn new(runtime_id: RuntimeId, worker_id: WorkerId) -> Self {
        Self {
            runtime_id,
            worker_id,
        }
    }
}
