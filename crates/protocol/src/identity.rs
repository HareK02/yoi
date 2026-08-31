use std::{fmt, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use sha2::{Digest, Sha256};
use uuid::{Uuid, Version};

/// Stable Worker identity independent of its current Runtime placement or
/// conversation Session.
///
/// Workspace authority allocates this ID for managed Workers. A standalone
/// Worker store allocates it locally when no Workspace authority is present.
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

    #[must_use]
    pub fn short(self) -> String {
        let simple = self.0.simple().to_string();
        simple[simple.len() - 12..].to_string()
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
    fn legacy_worker_id_mapping_is_stable() {
        assert_eq!(
            WorkerId::from_legacy_binding("workspace", "runtime", 42),
            WorkerId::from_legacy_binding("workspace", "runtime", 42)
        );
        assert_ne!(
            WorkerId::from_legacy_binding("workspace", "runtime", 42),
            WorkerId::from_legacy_binding("workspace", "runtime", 43)
        );
    }
}
