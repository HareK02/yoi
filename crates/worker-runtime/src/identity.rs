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

/// Backend-visible Worker identity, namespaced by the Runtime that owns the Worker record.
///
/// This is intentionally distinct from [`WorkerRef`], which is meaningful only inside one
/// Runtime. Do not flatten this reference into a concatenated string for authority decisions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RuntimeWorkerRef {
    pub runtime_id: String,
    pub worker_id: String,
}

impl RuntimeWorkerRef {
    pub fn new(runtime_id: impl Into<String>, worker_id: impl Into<String>) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            worker_id: worker_id.into(),
        }
    }

    pub fn local_worker_ref(&self) -> Result<WorkerRef, std::num::ParseIntError> {
        self.worker_id
            .parse::<u64>()
            .map(WorkerId::new)
            .map(WorkerRef::new)
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
            worker.local_worker_ref().unwrap(),
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
        assert!(worker.local_worker_ref().is_err());
    }
}
