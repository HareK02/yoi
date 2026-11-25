//! Storage schema 2 -> 3, under the existing standalone execution lease and
//! create-new commit marker. Only explicitly disabled Workspace Memory is inert
//! enough to remove. This is not a manifest decoder or a Profile execution adapter.
//!
//! Preflight both files before publishing either. The typed pending marker keeps
//! exact before/after bytes; a new lease owner may roll forward only those exact
//! replacements after a crash. Unknown/ordinary commit markers remain blocked.
//! No Session, Profile archive, immutable request, fingerprint or bundle is edited.

use super::*;
use serde_json::{Value, json};

#[cfg(test)]
#[path = "memory_retirement_tests.rs"]
mod tests;

const MIGRATION_VERSION: u32 = 1;

/// Reject duplicate keys before interpreting a disabled bit or rewriting a whole
/// JSON document. Last-key-wins decoding would erase ambiguous policy/evidence.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueValue(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(
                            "ambiguous persisted JSON: duplicate object key",
                        ));
                    }
                    let UniqueValue(value) = map.next_value()?;
                    values.insert(key, value);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn parse_document(text: &str) -> Result<Value, StandaloneStoreError> {
    serde_json::from_str::<UniqueValue>(text)
        .map(|value| value.0)
        .map_err(StandaloneStoreError::Json)
}
const UNAVAILABLE: &str = "removed Workspace Memory is enabled or ambiguous; this Worker/Profile cannot execute; select a new supported Profile explicitly";

fn unavailable(id: WorkerId, reason: &'static str) -> StandaloneStoreError {
    StandaloneStoreError::WorkspaceMemoryRetirement { id, reason }
}

/// A frozen storage-only shape; `enabled` is intentionally required, not defaulted.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DisabledMemoryProfile {
    enabled: bool,
    #[serde(default, rename = "staging_tools")]
    _staging_tools: bool,
    #[serde(default, rename = "resident")]
    _resident: DisabledMemoryResident,
    #[serde(default, rename = "extraction")]
    _extraction: manifest::MemoryExtractionProfileConfig,
    #[serde(default, rename = "consolidation")]
    _consolidation: DisabledMemoryConsolidation,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DisabledMemoryResident {
    #[serde(default, rename = "inject_summary")]
    _inject_summary: bool,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DisabledMemoryConsolidation {
    #[serde(default, rename = "request_enabled")]
    _request_enabled: bool,
}

/// Remove only feature.memory, preserving the raw envelope and all other fields.
/// The ordinary typed reader still rejects this key, in every snapshot version.
fn retire_snapshot(id: WorkerId, snapshot: &mut Value) -> Result<bool, StandaloneStoreError> {
    let root = snapshot
        .as_object_mut()
        .ok_or_else(|| unavailable(id, UNAVAILABLE))?;
    let manifest = match root.get("schema_version") {
        None => root,
        Some(version) if matches!(version.as_u64(), Some(2..=5)) && root.len() == 2 => root
            .get_mut("manifest")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| unavailable(id, UNAVAILABLE))?,
        _ => {
            return Err(unavailable(
                id,
                "unsupported persisted manifest storage version; no migration was applied",
            ));
        }
    };
    // Old top-level memory authority/settings have no explicit disabled bit.
    if manifest.contains_key("memory") {
        return Err(unavailable(id, UNAVAILABLE));
    }
    let Some(feature) = manifest.get_mut("feature") else {
        return Ok(false);
    };
    let feature = feature
        .as_object_mut()
        .ok_or_else(|| unavailable(id, UNAVAILABLE))?;
    let Some(memory) = feature.get("memory") else {
        return Ok(false);
    };
    let memory = memory
        .as_object()
        .ok_or_else(|| unavailable(id, UNAVAILABLE))?;
    let disabled = if let Some(profile) = memory.get("profile") {
        if memory
            .keys()
            .any(|key| !matches!(key.as_str(), "profile" | "workspace_settings"))
            || memory
                .get("workspace_settings")
                .is_some_and(|value| !value.is_null())
        {
            false
        } else {
            serde_json::from_value::<DisabledMemoryProfile>(profile.clone())
                .is_ok_and(|profile| !profile.enabled)
        }
    } else {
        memory.get("enabled") == Some(&Value::Bool(false))
            && memory
                .keys()
                .all(|key| matches!(key.as_str(), "enabled" | "staging"))
            && memory.get("staging").is_none_or(Value::is_boolean)
    };
    if !disabled {
        return Err(unavailable(id, UNAVAILABLE));
    }
    feature.remove("memory");
    Ok(true)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementCommit {
    migration: String,
    version: u32,
    worker_id: WorkerId,
    storage_key: String,
    record_before: String,
    record_after: String,
    metadata_before: String,
    metadata_after: String,
}

fn encode(value: &Value) -> Result<String, StandaloneStoreError> {
    let mut text = serde_json::to_string_pretty(value).map_err(StandaloneStoreError::Json)?;
    text.push('\n');
    Ok(text)
}

fn regular_file(path: &Path) -> Result<String, StandaloneStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(StandaloneStoreError::Io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(StandaloneStoreError::NotDirectory);
    }
    fs::read_to_string(path).map_err(StandaloneStoreError::Io)
}

fn directory(path: &Path) -> Result<(), StandaloneStoreError> {
    let metadata = fs::symlink_metadata(path).map_err(StandaloneStoreError::Io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(StandaloneStoreError::NotDirectory);
    }
    Ok(())
}

fn metadata_path(dir: &Path, key: &str) -> Result<PathBuf, StandaloneStoreError> {
    session_store::validate_worker_name(key).map_err(|_| StandaloneStoreError::NotDirectory)?;
    directory(&dir.join(WORKER_DIR))?;
    let worker = dir.join(WORKER_DIR).join(key);
    directory(&worker)?;
    Ok(worker.join("metadata.json"))
}

impl RetirementCommit {
    fn prepare(
        id: WorkerId,
        record_before: String,
        metadata_before: String,
    ) -> Result<Self, StandaloneStoreError> {
        let mut record = parse_document(&record_before)?;
        let schema = record.get("schema_version").and_then(Value::as_u64);
        if !matches!(schema, Some(1..=3)) {
            return Err(unavailable(
                id,
                "unsupported standalone record storage version; no migration was applied",
            ));
        }
        let storage_key = record
            .get("storage_key")
            .and_then(Value::as_str)
            .ok_or(StandaloneStoreError::InvalidRecord(id))?
            .to_owned();
        session_store::validate_worker_name(&storage_key)
            .map_err(|_| StandaloneStoreError::InvalidRecord(id))?;
        let snapshot = record
            .get_mut("manifest")
            .ok_or(StandaloneStoreError::InvalidRecord(id))?;
        let record_changed = retire_snapshot(id, snapshot)?;
        // A schema-3 record was already migrated. Do not grant a decoder escape
        // hatch to new, malformed records containing removed execution policy.
        if schema == Some(3) && record_changed {
            return Err(unavailable(
                id,
                "schema-3 Worker contains removed Workspace Memory; no migration was applied",
            ));
        }
        let mut metadata = parse_document(&metadata_before)?;
        let metadata_object = metadata
            .as_object_mut()
            .ok_or(StandaloneStoreError::InvalidRecord(id))?;
        let metadata_changed = match metadata_object.get_mut("resolved_manifest_snapshot") {
            Some(Value::Null) | None => false,
            Some(snapshot) => {
                let changed = retire_snapshot(id, snapshot)?;
                // Validate the replacement without serializing it back from a
                // typed manifest: immutable nested source/bundle fields stay raw.
                manifest::read_persisted_worker_manifest_snapshot(snapshot.clone())
                    .map_err(StandaloneStoreError::Json)?;
                changed
            }
        };
        if schema == Some(3) && metadata_changed {
            return Err(unavailable(
                id,
                "schema-3 Worker metadata contains removed Workspace Memory; no migration was applied",
            ));
        }
        record["schema_version"] = json!(SCHEMA_VERSION);
        let record_after = encode(&record)?;
        let decoded = decode_worker_record(id, record_after.as_bytes())?;
        if decoded.worker_id != id
            || decoded.storage_key != storage_key
            || decoded.worker_name != decoded.manifest.worker.name
            || metadata.get("worker_name").and_then(Value::as_str) != Some(storage_key.as_str())
        {
            return Err(StandaloneStoreError::InvalidRecord(id));
        }
        serde_json::from_value::<session_store::WorkerMetadata>(metadata.clone())
            .map_err(StandaloneStoreError::Json)?;
        let metadata_after = if metadata_changed {
            encode(&metadata)?
        } else {
            metadata_before.clone()
        };
        // An already-current record is not rewritten merely for JSON formatting.
        let record_after = if schema == Some(3) && !record_changed {
            record_before.clone()
        } else {
            record_after
        };
        Ok(Self {
            migration: "disabled_workspace_memory_retirement".into(),
            version: MIGRATION_VERSION,
            worker_id: id,
            storage_key,
            record_before,
            record_after,
            metadata_before,
            metadata_after,
        })
    }

    fn validate(&self, id: WorkerId) -> Result<(), StandaloneStoreError> {
        if self.migration != "disabled_workspace_memory_retirement"
            || self.version != MIGRATION_VERSION
            || self.worker_id != id
        {
            return Err(StandaloneStoreError::IncompleteCommit(id));
        }
        let expected = Self::prepare(id, self.record_before.clone(), self.metadata_before.clone())?;
        if expected.storage_key != self.storage_key
            || expected.record_after != self.record_after
            || expected.metadata_after != self.metadata_after
        {
            return Err(StandaloneStoreError::IncompleteCommit(id));
        }
        Ok(())
    }
}

fn replace_exact(
    path: &Path,
    before: &str,
    after: &str,
    id: WorkerId,
) -> Result<(), StandaloneStoreError> {
    let current = regular_file(path)?;
    if current == after {
        return sync_directory(path.parent().expect("metadata parent"));
    }
    if current != before {
        return Err(StandaloneStoreError::MetadataChanged(id));
    }
    let parent = path.parent().expect("metadata parent");
    let temporary = parent.join(format!("retirement.{}.tmp", Uuid::now_v7()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(StandaloneStoreError::Io)?;
        file.write_all(after.as_bytes())
            .map_err(StandaloneStoreError::Io)?;
        file.sync_all().map_err(StandaloneStoreError::Io)?;
        fs::rename(&temporary, path).map_err(StandaloneStoreError::Io)?;
        sync_directory(parent)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

impl StandaloneWorkerStore {
    /// Explicit storage-owner operation; never invoked by load/list/decoder.
    /// Before bootstrapping a restored Worker, migrate both current execution
    /// snapshots, or recover precisely this migration's interrupted commit.
    pub(crate) fn migrate_disabled_workspace_memory(
        &self,
        id: WorkerId,
        lease: &StandaloneWorkerLease,
    ) -> Result<(), StandaloneStoreError> {
        let dir = self.worker_dir(id);
        directory(&dir)?;
        if lease.path != dir.join(LEASE_FILE) {
            return Err(StandaloneStoreError::LeaseOwnershipLost);
        }
        // Keep lease replacement/deletion fenced for the entire two-file commit.
        let _guard = LeaseMutationGuard::acquire(&dir)?;
        lease.verify_current_locked()?;
        let marker = dir.join(COMMIT_MARKER);
        let plan = if marker.try_exists().map_err(StandaloneStoreError::Io)? {
            let plan: RetirementCommit = serde_json::from_str(&regular_file(&marker)?)
                .map_err(|_| StandaloneStoreError::IncompleteCommit(id))?;
            plan.validate(id)?;
            plan
        } else {
            let before = regular_file(&dir.join(RECORD_FILE))?;
            let value = parse_document(&before)?;
            let key = value
                .get("storage_key")
                .and_then(Value::as_str)
                .ok_or(StandaloneStoreError::InvalidRecord(id))?;
            let metadata = regular_file(&metadata_path(&dir, key)?)?;
            let plan = RetirementCommit::prepare(id, before, metadata)?;
            if plan.record_before == plan.record_after
                && plan.metadata_before == plan.metadata_after
            {
                return Ok(());
            }
            // A partial marker is deliberately not guessed/repaired. It has not
            // authorized any replacement yet, and remains an explicit blocker.
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker)
                .map_err(StandaloneStoreError::Io)?;
            serde_json::to_writer(&mut file, &plan).map_err(StandaloneStoreError::Json)?;
            file.write_all(b"\n").map_err(StandaloneStoreError::Io)?;
            file.sync_all().map_err(StandaloneStoreError::Io)?;
            sync_directory(&dir)?;
            plan
        };
        let metadata = metadata_path(&dir, &plan.storage_key)?;
        let record_path = dir.join(RECORD_FILE);
        // Check BOTH observations before resuming either write. A different owner
        // cannot have its metadata overwritten by an old pending payload.
        for (path, before, after) in [
            (
                metadata.as_path(),
                &plan.metadata_before,
                &plan.metadata_after,
            ),
            (
                record_path.as_path(),
                &plan.record_before,
                &plan.record_after,
            ),
        ] {
            let current = regular_file(path)?;
            if current != *before && current != *after {
                return Err(StandaloneStoreError::MetadataChanged(id));
            }
        }
        replace_exact(&metadata, &plan.metadata_before, &plan.metadata_after, id)?;
        replace_exact(
            &dir.join(RECORD_FILE),
            &plan.record_before,
            &plan.record_after,
            id,
        )?;
        fs::remove_file(&marker).map_err(StandaloneStoreError::Io)?;
        sync_directory(&dir)
    }
}
