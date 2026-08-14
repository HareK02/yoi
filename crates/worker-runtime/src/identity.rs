use serde::{Deserialize, Serialize};
use std::fmt;
pub use workdir::workspace::RuntimeWorkerRef;

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

/// Convert an opaque Workspace Worker reference only at the Runtime-local boundary.
impl TryFrom<&RuntimeWorkerRef> for WorkerRef {
    type Error = std::num::ParseIntError;

    fn try_from(value: &RuntimeWorkerRef) -> Result<Self, Self::Error> {
        value
            .worker_id
            .parse::<u64>()
            .map(WorkerId::new)
            .map(Self::new)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_worker_ref_preserves_structured_identity_and_json_fields() {
        let worker = RuntimeWorkerRef::new("arcadia", "30");
        assert_eq!(worker.runtime_id, "arcadia");
        assert_eq!(worker.worker_id, "30");
        assert_eq!(
            WorkerRef::try_from(&worker).unwrap(),
            WorkerRef::new(WorkerId::new(30))
        );
        assert_eq!(
            serde_json::to_value(&worker).unwrap(),
            serde_json::json!({"runtime_id": "arcadia", "worker_id": "30"})
        );
    }

    #[test]
    fn runtime_worker_ref_does_not_treat_composite_text_as_local_worker_id() {
        let worker = RuntimeWorkerRef::new("arcadia", "embedded-worker-runtime-5");
        assert!(WorkerRef::try_from(&worker).is_err());
    }
}
