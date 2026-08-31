use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use protocol::{WorkerId, WorkerIdParseError};
pub use workdir::workspace::RuntimeWorkerRef;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyWorkerIdentityMapping {
    pub workspace_id: String,
    pub runtime_id: String,
    pub legacy_worker_id: u64,
    pub worker_id: WorkerId,
}

pub fn legacy_worker_identity_mapping_digest(mappings: &[LegacyWorkerIdentityMapping]) -> String {
    let mut mappings = mappings.to_vec();
    mappings.sort_by(|left, right| {
        (
            left.workspace_id.as_str(),
            left.runtime_id.as_str(),
            left.legacy_worker_id,
            left.worker_id,
        )
            .cmp(&(
                right.workspace_id.as_str(),
                right.runtime_id.as_str(),
                right.legacy_worker_id,
                right.worker_id,
            ))
    });
    let mut hasher = Sha256::new();
    hasher.update(b"yoi.workspace-worker-migration-plan.v1\0");
    for mapping in mappings {
        hasher.update(mapping.workspace_id.as_bytes());
        hasher.update([0]);
        hasher.update(mapping.runtime_id.as_bytes());
        hasher.update([0]);
        hasher.update(mapping.legacy_worker_id.to_be_bytes());
        hasher.update(mapping.worker_id.to_string().as_bytes());
        hasher.update([b'\n']);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Runtime-local authority reference for Worker operations. The contained id is
/// nevertheless the stable Worker identity; the Runtime does not mint it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkerRef {
    pub worker_id: WorkerId,
}

impl WorkerRef {
    pub fn new(worker_id: WorkerId) -> Self {
        Self { worker_id }
    }
}

impl TryFrom<&RuntimeWorkerRef> for WorkerRef {
    type Error = WorkerIdParseError;

    fn try_from(value: &RuntimeWorkerRef) -> Result<Self, Self::Error> {
        value.worker_id.parse().map(Self::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_worker_ref_preserves_stable_worker_identity() {
        let worker_id = WorkerId::now_v7();
        let worker = RuntimeWorkerRef::new("arcadia", worker_id.to_string());
        assert_eq!(
            WorkerRef::try_from(&worker).unwrap(),
            WorkerRef::new(worker_id)
        );
        assert_eq!(
            serde_json::to_value(&worker).unwrap(),
            serde_json::json!({"runtime_id": "arcadia", "worker_id": worker_id.to_string()})
        );
    }

    #[test]
    fn runtime_worker_ref_rejects_legacy_numeric_identity() {
        let worker = RuntimeWorkerRef::new("arcadia", "30");
        assert!(WorkerRef::try_from(&worker).is_err());
    }
}
