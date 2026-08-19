use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::{fmt, str::FromStr};
use uuid::{Uuid, Version};

pub use workdir::workspace::RuntimeWorkerRef;

/// Stable Workspace-owned Worker identity.
///
/// Runtime placement is deliberately not part of this value. New identities are
/// allocated by Workspace authority before a Runtime create request is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(Uuid);

impl WorkerId {
    pub fn now_v7() -> Self {
        Self(Uuid::now_v7())
    }

    /// Converts a legacy Runtime-local numeric id into a syntactically valid
    /// migration-only UUIDv7 value. New Worker allocation must use `now_v7`.
    pub fn from_legacy_u64(value: u64) -> Self {
        let mut bytes = [0_u8; 16];
        bytes[8..].copy_from_slice(&value.to_be_bytes());
        bytes[6] = 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes))
    }

    pub fn from_legacy_binding(workspace_id: &str, runtime_id: &str, value: u64) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(b"yoi.workspace-worker-id.v1\0");
        hasher.update(workspace_id.as_bytes());
        hasher.update([0]);
        hasher.update(runtime_id.as_bytes());
        hasher.update([0]);
        hasher.update(value.to_be_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0_u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // Migrated ids sort before normally allocated UUIDv7 values while retaining
        // deterministic collision-resistant payload bits.
        bytes[..6].fill(0);
        bytes[6] = (bytes[6] & 0x0f) | 0x70;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        Self(Uuid::from_bytes(bytes))
    }

    pub fn parse(value: &str) -> Option<Self> {
        let value = Uuid::parse_str(value).ok()?;
        (value.get_version() == Some(Version::SortRand)).then_some(Self(value))
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for WorkerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for WorkerId {
    type Err = WorkerIdParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value).ok_or(WorkerIdParseError)
    }
}

impl Serialize for WorkerId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for WorkerId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(&value).ok_or_else(|| de::Error::custom("Worker id must be a UUIDv7"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerIdParseError;

impl fmt::Display for WorkerIdParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Worker id must be a UUIDv7")
    }
}

impl std::error::Error for WorkerIdParseError {}

/// Runtime-local authority reference for Worker operations. The contained id is
/// nevertheless the Workspace-owned stable identity; the Runtime does not mint it.
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
    fn worker_id_accepts_only_uuid_v7() {
        let worker_id = WorkerId::now_v7();
        assert_eq!(WorkerId::parse(&worker_id.to_string()), Some(worker_id));
        assert!(WorkerId::parse("30").is_none());
        assert!(WorkerId::parse(&Uuid::nil().to_string()).is_none());
    }

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
