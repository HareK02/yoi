use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use fs4::fs_std::FileExt;
use protocol::{UploadedFileAvailability, UploadedFileRef};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;
use unicode_properties::general_category::{GeneralCategory, UnicodeGeneralCategory};
use unicode_security::{confusable_detection::skeleton, mixed_script::MixedScript};
use uuid::Uuid;

use crate::StoreError;

type Result<T> = std::result::Result<T, StoreError>;

pub const DEFAULT_MAX_UPLOADED_FILE_BYTES: u64 = 10 * 1024 * 1024;
pub const DEFAULT_MAX_SESSION_ARTIFACT_BYTES: u64 = 32 * 1024 * 1024;
pub const DEFAULT_MAX_FILES_PER_SUBMISSION: usize = 8;
pub const DEFAULT_MAX_SESSION_UPLOADED_FILES: u64 = 256;
const MAX_FILE_NAME_CHARS: usize = 255;
const MAX_MEDIA_TYPE_BYTES: usize = 127;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadedFileLimits {
    pub max_file_bytes: u64,
    pub max_session_bytes: u64,
}

impl Default for UploadedFileLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_UPLOADED_FILE_BYTES,
            max_session_bytes: DEFAULT_MAX_SESSION_ARTIFACT_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadedFileUploadContext {
    pub upload_id: String,
    pub principal_id: String,
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredUploadedFile {
    file_name: String,
    media_type: String,
    created_at_ms: u64,
    byte_len: u64,
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    upload_context: Option<UploadedFileUploadContext>,
    content_base64: String,
}

pub(crate) fn validate_file_name(file_name: &str) -> Result<()> {
    let normalized: String = file_name.nfkc().collect();
    let has_unsafe_component = file_name
        .split('.')
        .filter(|part| !part.is_empty())
        .any(|part| {
            let confusable_skeleton: String = skeleton(part).collect();
            let ascii_confusable = part.chars().any(|ch| !ch.is_ascii())
                && confusable_skeleton.is_ascii()
                && !confusable_skeleton.eq_ignore_ascii_case(part);
            !part.is_single_script() || ascii_confusable
        });

    if file_name.is_empty()
        || file_name.chars().count() > MAX_FILE_NAME_CHARS
        || file_name == "."
        || file_name == ".."
        || normalized != file_name
        || has_unsafe_component
        || file_name.chars().any(|ch| {
            ch.is_control()
                || ch.general_category() == GeneralCategory::Format
                || matches!(ch, '/' | '\\')
        })
    {
        return Err(StoreError::InvalidUploadedFileName);
    }
    Ok(())
}

pub(crate) fn validate_media_type(media_type: &str) -> Result<()> {
    let valid = !media_type.is_empty()
        && media_type.len() <= MAX_MEDIA_TYPE_BYTES
        && media_type.is_ascii()
        && !media_type
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
        && media_type.split_once('/').is_some_and(|(kind, subtype)| {
            !kind.is_empty()
                && !subtype.is_empty()
                && kind.bytes().chain(subtype.bytes()).all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || matches!(
                            byte,
                            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                        )
                })
        });
    let allowed = media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/pdf"
                | "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
        );
    if !valid || !allowed {
        return Err(StoreError::InvalidUploadedFileMediaType);
    }
    Ok(())
}

fn normalized_file_name(file_name: &str) -> String {
    file_name.nfkc().flat_map(char::to_lowercase).collect()
}

fn validate_content(media_type: &str, content: &[u8]) -> Result<()> {
    if content.is_empty() {
        return Err(StoreError::InvalidUploadedFileMediaType);
    }
    let matches_declared_type = if media_type.starts_with("text/") {
        std::str::from_utf8(content).is_ok()
    } else {
        match media_type {
            "application/json" => serde_json::from_slice::<serde_json::Value>(content).is_ok(),
            "application/pdf" => content.starts_with(b"%PDF-"),
            "image/png" => content.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => content.starts_with(&[0xff, 0xd8, 0xff]),
            "image/gif" => content.starts_with(b"GIF87a") || content.starts_with(b"GIF89a"),
            "image/webp" => {
                content.len() >= 12 && content.starts_with(b"RIFF") && &content[8..12] == b"WEBP"
            }
            _ => false,
        }
    };
    if !matches_declared_type {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    Ok(())
}

fn record_path(dir: &Path, artifact_id: &str) -> Result<std::path::PathBuf> {
    let id = Uuid::parse_str(artifact_id).map_err(|_| StoreError::InvalidArtifactId)?;
    Ok(dir.join(format!("{id}.file.json")))
}

fn now_ms() -> Result<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StoreError::InvalidTimestamp)?
        .as_millis();
    u64::try_from(value).map_err(|_| StoreError::InvalidTimestamp)
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn stored_uploaded_file_usage(dir: &Path) -> Result<(u64, u64)> {
    if !dir.exists() {
        return Ok((0, 0));
    }
    let mut bytes = 0_u64;
    let mut count = 0_u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file()
            || !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".file.json"))
        {
            continue;
        }
        let stored: StoredUploadedFile = serde_json::from_slice(&fs::read(&path)?)?;
        bytes = bytes
            .checked_add(stored.byte_len)
            .ok_or(StoreError::ArtifactQuotaExceeded)?;
        count = count
            .checked_add(1)
            .ok_or(StoreError::ArtifactQuotaExceeded)?;
    }
    Ok((bytes, count))
}

pub(crate) fn write_uploaded_file(
    dir: &Path,
    file_name: &str,
    media_type: &str,
    content: &[u8],
    context: Option<&UploadedFileUploadContext>,
    limits: UploadedFileLimits,
) -> Result<UploadedFileRef> {
    validate_file_name(file_name)?;
    validate_media_type(media_type)?;
    validate_content(media_type, content)?;
    let byte_len = u64::try_from(content.len()).map_err(|_| StoreError::ArtifactTooLarge)?;
    let sha256 = digest(content);
    if byte_len > limits.max_file_bytes {
        return Err(StoreError::ArtifactTooLarge);
    }

    fs::create_dir_all(dir)?;
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let (paste_bytes, _) = crate::paste_artifact::stored_paste_usage(dir)?;
    let (file_bytes, file_count) = stored_uploaded_file_usage(dir)?;
    let normalized_name = normalized_file_name(file_name);
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".file.json"))
        {
            continue;
        }
        let stored: StoredUploadedFile = serde_json::from_slice(&fs::read(&path)?)?;
        let same_context = context.is_some() && stored.upload_context.as_ref() == context;
        let same_uncommitted_name = stored.source_entry_id.is_none()
            && normalized_file_name(&stored.file_name) == normalized_name;
        if same_context || same_uncommitted_name {
            if stored.file_name == file_name
                && stored.media_type == media_type
                && stored.byte_len == byte_len
                && stored.sha256 == sha256
                && stored.upload_context.as_ref() == context
            {
                let artifact_id = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".file.json"))
                    .ok_or(StoreError::InvalidArtifactId)?
                    .to_string();
                return Ok(UploadedFileRef {
                    artifact_id,
                    file_name: stored.file_name,
                    media_type: stored.media_type,
                    created_at_ms: stored.created_at_ms,
                    availability: UploadedFileAvailability::Available,
                    byte_len: stored.byte_len,
                    sha256: stored.sha256,
                    source_entry_id: None,
                });
            }
            return Err(StoreError::InvalidUploadedFileName);
        }
    }
    if file_count >= DEFAULT_MAX_SESSION_UPLOADED_FILES {
        return Err(StoreError::ArtifactQuotaExceeded);
    }
    if paste_bytes
        .checked_add(file_bytes)
        .and_then(|total| total.checked_add(byte_len))
        .is_none_or(|total| total > limits.max_session_bytes)
    {
        return Err(StoreError::ArtifactQuotaExceeded);
    }

    let artifact_id = Uuid::now_v7().to_string();
    let created_at_ms = now_ms()?;
    let stored = StoredUploadedFile {
        file_name: file_name.to_owned(),
        media_type: media_type.to_owned(),
        created_at_ms,
        byte_len,
        sha256: sha256.clone(),
        source_entry_id: None,
        upload_context: context.cloned(),
        content_base64: BASE64.encode(content),
    };
    let path = record_path(dir, &artifact_id)?;
    let temp = dir.join(format!(".{artifact_id}.file.tmp"));
    fs::write(&temp, serde_json::to_vec(&stored)?)?;
    fs::rename(&temp, &path)?;

    Ok(UploadedFileRef {
        artifact_id,
        file_name: file_name.to_owned(),
        media_type: media_type.to_owned(),
        created_at_ms,
        availability: UploadedFileAvailability::Available,
        byte_len,
        sha256,
        source_entry_id: None,
    })
}

pub(crate) fn read_uploaded_file_by_id(
    dir: &Path,
    artifact_id: &str,
) -> Result<(UploadedFileRef, Vec<u8>)> {
    let stored: StoredUploadedFile =
        serde_json::from_slice(&fs::read(record_path(dir, artifact_id)?)?)?;
    let content = BASE64
        .decode(&stored.content_base64)
        .map_err(|_| StoreError::ArtifactIntegrityMismatch)?;
    if u64::try_from(content.len()).ok() != Some(stored.byte_len)
        || digest(&content) != stored.sha256
    {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    let reference = UploadedFileRef {
        artifact_id: artifact_id.to_owned(),
        file_name: stored.file_name,
        media_type: stored.media_type,
        created_at_ms: stored.created_at_ms,
        availability: UploadedFileAvailability::Available,
        byte_len: stored.byte_len,
        sha256: stored.sha256,
        source_entry_id: stored.source_entry_id,
    };
    Ok((reference, content))
}

pub(crate) fn read_uploaded_file(dir: &Path, reference: &UploadedFileRef) -> Result<Vec<u8>> {
    let (stored_reference, content) = read_uploaded_file_by_id(dir, &reference.artifact_id)?;
    if stored_reference.file_name != reference.file_name
        || stored_reference.media_type != reference.media_type
        || stored_reference.created_at_ms != reference.created_at_ms
        || stored_reference.byte_len != reference.byte_len
        || stored_reference.sha256 != reference.sha256
        || stored_reference.source_entry_id != reference.source_entry_id
    {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    Ok(content)
}

pub(crate) fn clear_uploaded_file_binding(
    dir: &Path,
    artifact_id: &str,
    expected_source_entry_id: &str,
) -> Result<()> {
    fs::create_dir_all(dir)?;
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let path = record_path(dir, artifact_id)?;
    let mut stored: StoredUploadedFile = serde_json::from_slice(&fs::read(&path)?)?;
    if stored.source_entry_id.as_deref() != Some(expected_source_entry_id) {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    stored.source_entry_id = None;
    let temp = dir.join(format!(".{artifact_id}.file.unbind.tmp"));
    fs::write(&temp, serde_json::to_vec(&stored)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

pub(crate) fn bind_uploaded_file(
    dir: &Path,
    reference: &UploadedFileRef,
    source_entry_id: &str,
) -> Result<UploadedFileRef> {
    if source_entry_id.is_empty() || reference.source_entry_id.is_some() {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let (stored_reference, _) = read_uploaded_file_by_id(dir, &reference.artifact_id)?;
    if stored_reference.file_name != reference.file_name
        || stored_reference.media_type != reference.media_type
        || stored_reference.created_at_ms != reference.created_at_ms
        || stored_reference.byte_len != reference.byte_len
        || stored_reference.sha256 != reference.sha256
    {
        return Err(StoreError::ArtifactIntegrityMismatch);
    }
    let path = record_path(dir, &reference.artifact_id)?;
    let mut stored: StoredUploadedFile = serde_json::from_slice(&fs::read(&path)?)?;
    if stored.source_entry_id.is_some() {
        return Err(StoreError::ArtifactAlreadyCommitted);
    }
    stored.source_entry_id = Some(source_entry_id.to_owned());
    let temp = dir.join(format!(".{}.file.bind.tmp", reference.artifact_id));
    fs::write(&temp, serde_json::to_vec(&stored)?)?;
    fs::rename(&temp, path)?;
    let mut bound = reference.clone();
    bound.source_entry_id = Some(source_entry_id.to_owned());
    Ok(bound)
}

pub(crate) fn list_uploaded_file_refs(dir: &Path) -> Result<Vec<UploadedFileRef>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut refs = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let Some(artifact_id) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".file.json"))
        else {
            continue;
        };
        refs.push(read_uploaded_file_by_id(dir, artifact_id)?.0);
    }
    Ok(refs)
}

pub(crate) fn copy_committed_uploaded_files(source_dir: &Path, target_dir: &Path) -> Result<u64> {
    if !source_dir.exists() {
        return Ok(0);
    }
    fs::create_dir_all(target_dir)?;
    let target_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(target_dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&target_lock)?;
    let mut copied = 0_u64;
    for entry in fs::read_dir(source_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.ends_with(".file.json") {
            continue;
        }
        let bytes = fs::read(&path)?;
        let stored: StoredUploadedFile = serde_json::from_slice(&bytes)?;
        if stored.source_entry_id.is_none() {
            continue;
        }
        let target = target_dir.join(name);
        if target.exists() {
            let existing: StoredUploadedFile = serde_json::from_slice(&fs::read(&target)?)?;
            if existing.sha256 != stored.sha256
                || existing.file_name != stored.file_name
                || existing.source_entry_id != stored.source_entry_id
            {
                return Err(StoreError::ArtifactIntegrityMismatch);
            }
            continue;
        }
        let temp = target_dir.join(format!(".{name}.copy.tmp"));
        fs::write(&temp, &bytes)?;
        fs::rename(temp, target)?;
        copied = copied
            .checked_add(1)
            .ok_or(StoreError::ArtifactQuotaExceeded)?;
    }
    Ok(copied)
}

pub(crate) fn delete_uncommitted_uploaded_files(dir: &Path) -> Result<u64> {
    fs::create_dir_all(dir)?;
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let mut removed = 0_u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".file.json"))
        {
            continue;
        }
        let stored: StoredUploadedFile = serde_json::from_slice(&fs::read(&path)?)?;
        if stored.source_entry_id.is_none() {
            fs::remove_file(path)?;
            removed = removed
                .checked_add(1)
                .ok_or(StoreError::ArtifactQuotaExceeded)?;
        }
    }
    Ok(removed)
}

pub(crate) fn delete_uploaded_file(dir: &Path, artifact_id: &str) -> Result<bool> {
    fs::create_dir_all(dir)?;
    let aggregate_lock = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(".aggregate.lock"))?;
    FileExt::lock_exclusive(&aggregate_lock)?;
    let path = record_path(dir, artifact_id)?;
    let stored = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice::<StoredUploadedFile>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if stored.source_entry_id.is_some() {
        return Err(StoreError::ArtifactAlreadyCommitted);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
