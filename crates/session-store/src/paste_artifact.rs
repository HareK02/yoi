//! Session-owned storage for large pasted-input artifacts.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use protocol::{PasteArtifactAvailability, PasteArtifactMediaType, PasteArtifactRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::StoreError;

/// Bounded storage policy applied before a large paste becomes durable input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteArtifactLimits {
    pub max_artifact_bytes: u64,
    pub max_session_bytes: u64,
    pub max_session_artifacts: u64,
}

impl Default for PasteArtifactLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 8 * 1024 * 1024,
            max_session_bytes: 64 * 1024 * 1024,
            max_session_artifacts: 1_024,
        }
    }
}

/// Integrity-bearing on-disk record. The body and metadata are committed in one
/// atomic file replacement so readers never observe a half-written artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPasteArtifact {
    pub reference: PasteArtifactRef,
    pub content: String,
}

pub(crate) fn stored_paste_usage(artifact_dir: &Path) -> Result<(u64, u64), StoreError> {
    if !artifact_dir.exists() {
        return Ok((0, 0));
    }
    let mut aggregate = 0_u64;
    let mut artifact_count = 0_u64;
    for entry in fs::read_dir(artifact_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if !name.ends_with(".json") || name.ends_with(".file.json") {
            continue;
        }
        let stored: StoredPasteArtifact = serde_json::from_slice(&fs::read(&path)?)?;
        verify(&stored, &stored.reference.artifact_id)?;
        artifact_count = artifact_count.checked_add(1).ok_or_else(|| {
            StoreError::PasteArtifactLimit("session artifact count overflow".to_string())
        })?;
        aggregate = aggregate
            .checked_add(stored.reference.byte_len)
            .ok_or_else(|| {
                StoreError::PasteArtifactLimit("session aggregate size overflow".to_string())
            })?;
    }
    Ok((aggregate, artifact_count))
}

pub(crate) fn write_to_dir(
    artifact_dir: &Path,
    source_entry_id: &str,
    content: &str,
    limits: PasteArtifactLimits,
) -> Result<PasteArtifactRef, StoreError> {
    let byte_len = content.len() as u64;
    if byte_len > limits.max_artifact_bytes {
        return Err(StoreError::PasteArtifactLimit(format!(
            "artifact has {byte_len} bytes; maximum is {}",
            limits.max_artifact_bytes
        )));
    }
    fs::create_dir_all(artifact_dir)?;
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(artifact_dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let (paste_bytes, artifact_count) = stored_paste_usage(artifact_dir)?;
    let (uploaded_bytes, uploaded_count) =
        crate::uploaded_file::stored_uploaded_file_usage(artifact_dir)?;
    let aggregate = paste_bytes.checked_add(uploaded_bytes).ok_or_else(|| {
        StoreError::PasteArtifactLimit("session aggregate size overflow".to_string())
    })?;
    let artifact_count = artifact_count.checked_add(uploaded_count).ok_or_else(|| {
        StoreError::PasteArtifactLimit("session artifact count overflow".to_string())
    })?;
    let projected = aggregate.checked_add(byte_len).ok_or_else(|| {
        StoreError::PasteArtifactLimit("session aggregate size overflow".to_string())
    })?;
    if projected > limits.max_session_bytes {
        return Err(StoreError::PasteArtifactLimit(format!(
            "session artifacts would use {projected} bytes; maximum is {}",
            limits.max_session_bytes
        )));
    }
    if artifact_count >= limits.max_session_artifacts {
        return Err(StoreError::PasteArtifactLimit(format!(
            "session already has {artifact_count} artifacts; maximum is {}",
            limits.max_session_artifacts
        )));
    }

    let artifact_id = uuid::Uuid::now_v7().to_string();
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| StoreError::PasteArtifactIntegrity(error.to_string()))?
        .as_millis() as u64;
    let reference = PasteArtifactRef {
        artifact_id: artifact_id.clone(),
        created_at_ms,
        media_type: PasteArtifactMediaType::TextPlainUtf8,
        availability: PasteArtifactAvailability::Available,
        byte_len,
        char_count: content.chars().count() as u64,
        line_count: line_count(content),
        sha256: sha256_hex(content),
        source_entry_id: source_entry_id.to_string(),
    };
    let bytes = serde_json::to_vec(&StoredPasteArtifact {
        reference: reference.clone(),
        content: content.to_string(),
    })?;
    let target = artifact_dir.join(format!("{artifact_id}.json"));
    let temporary = artifact_dir.join(format!(".{artifact_id}.tmp"));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temporary, &target) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Ok(directory) = fs::File::open(artifact_dir) {
        directory.sync_all()?;
    }
    Ok(reference)
}

pub(crate) fn read_from_dir(
    artifact_dir: &Path,
    artifact_id: &str,
) -> Result<(PasteArtifactRef, String), StoreError> {
    let parsed = uuid::Uuid::parse_str(artifact_id)
        .map_err(|_| StoreError::PasteArtifactNotFound(artifact_id.to_string()))?;
    if parsed.to_string() != artifact_id {
        return Err(StoreError::PasteArtifactNotFound(artifact_id.to_string()));
    }
    let path = artifact_dir.join(format!("{artifact_id}.json"));
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(StoreError::PasteArtifactNotFound(artifact_id.to_string()));
        }
        Err(error) => return Err(error.into()),
    };
    let stored: StoredPasteArtifact = serde_json::from_slice(&bytes)?;
    verify(&stored, artifact_id)?;
    Ok((stored.reference, stored.content))
}

fn verify(stored: &StoredPasteArtifact, artifact_id: &str) -> Result<(), StoreError> {
    let actual_digest = sha256_hex(&stored.content);
    if stored.reference.artifact_id != artifact_id
        || stored.reference.created_at_ms == 0
        || stored.reference.media_type != PasteArtifactMediaType::TextPlainUtf8
        || stored.reference.availability != PasteArtifactAvailability::Available
        || stored.reference.byte_len != stored.content.len() as u64
        || stored.reference.char_count != stored.content.chars().count() as u64
        || stored.reference.line_count != line_count(&stored.content)
        || stored.reference.sha256 != actual_digest
    {
        return Err(StoreError::PasteArtifactIntegrity(artifact_id.to_string()));
    }
    Ok(())
}

fn sha256_hex(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn line_count(content: &str) -> u64 {
    if content.is_empty() {
        0
    } else {
        content.lines().count().max(1) as u64
    }
}
