//! Observational discovery is not execution admission. Keep policy out of the picker value;
//! only the lease owner can validate and migrate both execution snapshots before restore.

use super::*;

#[derive(Debug, Clone, Deserialize)]
pub struct StandaloneWorkerListEntry {
    pub worker_id: WorkerId,
    pub worker_name: String,
    pub cwd: StandaloneCwdIdentity,
    pub status: StandaloneWorkerStatus,
    pub updated_at_unix_ms: u64,
    // A static storage diagnostic, not policy or authority. None does not attest execution.
    #[serde(skip)]
    pub restore_blocker: Option<&'static str>,
}

#[derive(Deserialize)]
struct ListingRecord {
    schema_version: u32,
    #[serde(flatten)]
    entry: StandaloneWorkerListEntry,
    storage_key: String,
    manifest: serde_json::Value,
    #[serde(rename = "active_session_id")]
    _active_session_id: SessionId,
    #[serde(rename = "active_segment_id")]
    _active_segment_id: Option<SegmentId>,
    #[serde(rename = "created_at_unix_ms")]
    _created_at_unix_ms: u64,
    #[serde(rename = "shutdown_reason")]
    _shutdown_reason: Option<StandaloneShutdownReason>,
    subject: Option<crate::subjektiv::StandaloneSubjectBinding>,
}

impl StandaloneWorkerStore {
    pub(super) fn load_list_entry(
        &self,
        id: WorkerId,
    ) -> Result<StandaloneWorkerListEntry, StandaloneStoreError> {
        let dir = self.worker_dir(id);
        let metadata = fs::symlink_metadata(&dir).map_err(StandaloneStoreError::Io)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        if dir.join(COMMIT_MARKER).exists() {
            return Err(StandaloneStoreError::IncompleteCommit(id));
        }
        let path = dir.join(RECORD_FILE);
        let metadata = fs::symlink_metadata(&path).map_err(StandaloneStoreError::Io)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        let text = fs::read_to_string(path).map_err(StandaloneStoreError::Io)?;
        let snapshot = memory_retirement::parse_document(&text).map_err(|error| match error {
            StandaloneStoreError::Json(source) => {
                StandaloneStoreError::CorruptRecord { id, source }
            }
            error => error,
        })?;
        let record: ListingRecord = serde_json::from_value(snapshot)
            .map_err(|source| StandaloneStoreError::CorruptRecord { id, source })?;
        if record.schema_version > SCHEMA_VERSION {
            return Err(StandaloneStoreError::NewerSchema {
                id,
                found: record.schema_version,
                supported: SCHEMA_VERSION,
            });
        }
        if !matches!(record.schema_version, 1..=SCHEMA_VERSION)
            || record.entry.worker_id != id
            || !record.entry.cwd.canonical_path.is_absolute()
            || session_store::validate_worker_name(&record.storage_key).is_err()
            || record.subject.as_ref().is_some_and(|binding| {
                binding.scope_id.trim().is_empty() || binding.subject_id.trim().is_empty()
            })
        {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        let (name, restore_blocker) =
            memory_retirement::listing_worker_name(id, record.schema_version, record.manifest)?;
        if name != record.entry.worker_name {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        let mut entry = record.entry;
        entry.restore_blocker = restore_blocker;
        Ok(entry)
    }
}
