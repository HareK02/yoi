use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Error, Result};

pub const WORKSPACE_IDENTITY_RELATIVE_PATH: &str = ".yoi/workspace.toml";

/// Stable local Workspace identity persisted as a tracked, safe project record.
///
/// The v0 TOML schema intentionally contains identity metadata only:
/// `workspace_id`, `created_at`, and `display_name`. Unknown fields are rejected
/// instead of preserved because this loader cannot safely round-trip future local
/// runtime settings without risking accidental path or secret persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceIdentity {
    pub workspace_id: String,
    pub created_at: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceIdentityFile {
    workspace_id: String,
    created_at: String,
    display_name: String,
}

impl WorkspaceIdentity {
    pub fn load_or_init(workspace_root: impl AsRef<Path>) -> Result<Self> {
        Self::load_or_init_with_clock(workspace_root.as_ref(), || {
            Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
        })
    }

    pub fn path(workspace_root: impl AsRef<Path>) -> PathBuf {
        workspace_root
            .as_ref()
            .join(WORKSPACE_IDENTITY_RELATIVE_PATH)
    }

    pub fn parse_str(raw: &str, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let parsed: WorkspaceIdentityFile = toml::from_str(raw).map_err(|error| {
            workspace_identity_error(path, format!("failed to parse TOML: {error}"))
        })?;
        Self::from_file(parsed, path)
    }

    fn load_or_init_with_clock(
        workspace_root: &Path,
        now_utc_rfc3339: impl FnOnce() -> String,
    ) -> Result<Self> {
        let path = Self::path(workspace_root);
        match fs::read_to_string(&path) {
            Ok(raw) => Self::parse_str(&raw, &path),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                Self::init(workspace_root, &path, now_utc_rfc3339())
            }
            Err(error) => Err(Error::Io(error)),
        }
    }

    fn init(workspace_root: &Path, path: &Path, created_at: String) -> Result<Self> {
        if path.exists() {
            let raw = fs::read_to_string(path)?;
            return Self::parse_str(&raw, path);
        }
        validate_created_at(&created_at, path)?;
        let display_name = workspace_display_name_from_root(workspace_root, path)?;
        let workspace_id = Uuid::now_v7().to_string();
        let identity = Self {
            workspace_id,
            created_at,
            display_name,
        };
        identity.write_new(path)?;
        Ok(identity)
    }

    fn from_file(parsed: WorkspaceIdentityFile, path: &Path) -> Result<Self> {
        let workspace_id = validate_workspace_id(&parsed.workspace_id, path)?;
        validate_created_at(&parsed.created_at, path)?;
        validate_display_name(&parsed.display_name, path)?;
        Ok(Self {
            workspace_id,
            created_at: parsed.created_at,
            display_name: parsed.display_name,
        })
    }

    fn write_new(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(&WorkspaceIdentityFile {
            workspace_id: self.workspace_id.clone(),
            created_at: self.created_at.clone(),
            display_name: self.display_name.clone(),
        })
        .map_err(|error| {
            workspace_identity_error(path, format!("failed to encode TOML: {error}"))
        })?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, raw)?;
        if path.exists() {
            let _ = fs::remove_file(&tmp);
            let raw = fs::read_to_string(path)?;
            return Self::parse_str(&raw, path).map(|_| ());
        }
        fs::rename(tmp, path)?;
        Ok(())
    }
}

fn validate_workspace_id(value: &str, path: &Path) -> Result<String> {
    let uuid = Uuid::parse_str(value).map_err(|error| {
        workspace_identity_error(path, format!("workspace_id is not a UUID: {error}"))
    })?;
    if uuid.get_version_num() != 7 {
        return Err(workspace_identity_error(
            path,
            "workspace_id must be a UUIDv7 canonical string".to_string(),
        ));
    }
    let canonical = uuid.to_string();
    if value != canonical {
        return Err(workspace_identity_error(
            path,
            "workspace_id must use lowercase hyphenated UUID canonical form".to_string(),
        ));
    }
    Ok(canonical)
}

fn validate_created_at(value: &str, path: &Path) -> Result<()> {
    let parsed = chrono::DateTime::parse_from_rfc3339(value).map_err(|error| {
        workspace_identity_error(path, format!("created_at is not RFC3339: {error}"))
    })?;
    if parsed.offset().local_minus_utc() != 0 || !value.ends_with('Z') {
        return Err(workspace_identity_error(
            path,
            "created_at must be a UTC RFC3339 timestamp ending in Z".to_string(),
        ));
    }
    Ok(())
}

fn validate_display_name(value: &str, path: &Path) -> Result<()> {
    if value.trim().is_empty() {
        return Err(workspace_identity_error(
            path,
            "display_name must not be empty".to_string(),
        ));
    }
    if value.contains('\0') || value.chars().any(|ch| ch.is_control()) {
        return Err(workspace_identity_error(
            path,
            "display_name must not contain control characters".to_string(),
        ));
    }
    Ok(())
}

fn workspace_display_name_from_root(workspace_root: &Path, path: &Path) -> Result<String> {
    let display_name = workspace_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            workspace_identity_error(
                path,
                "workspace root must have a UTF-8 final path component".to_string(),
            )
        })?
        .to_string();
    validate_display_name(&display_name, path)?;
    Ok(display_name)
}

fn workspace_identity_error(path: &Path, message: String) -> Error {
    Error::WorkspaceIdentity(format!("{}: {message}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXED_WORKSPACE_ID: &str = "0192f0e8-4d84-7d6e-a000-000000000001";
    const FIXED_CREATED_AT: &str = "2026-06-23T06:43:28Z";

    #[test]
    fn missing_identity_file_is_created_with_safe_fields() {
        let temp = tempfile::tempdir().unwrap();
        let workspace_root = temp.path().join("example-workspace");
        fs::create_dir_all(&workspace_root).unwrap();

        let identity = WorkspaceIdentity::load_or_init_with_clock(&workspace_root, || {
            FIXED_CREATED_AT.to_string()
        })
        .unwrap();

        assert_eq!(identity.display_name, "example-workspace");
        assert_eq!(identity.created_at, FIXED_CREATED_AT);
        validate_workspace_id(
            &identity.workspace_id,
            &WorkspaceIdentity::path(&workspace_root),
        )
        .unwrap();

        let raw = fs::read_to_string(WorkspaceIdentity::path(&workspace_root)).unwrap();
        assert!(raw.contains("workspace_id"));
        assert!(raw.contains("display_name"));
        assert!(raw.contains("created_at"));
        assert!(!raw.contains(&workspace_root.to_string_lossy().to_string()));

        let reloaded = WorkspaceIdentity::load_or_init_with_clock(&workspace_root, || {
            "2026-06-24T00:00:00Z".to_string()
        })
        .unwrap();
        assert_eq!(reloaded, identity);
    }

    #[test]
    fn existing_identity_file_is_stable() {
        let temp = tempfile::tempdir().unwrap();
        let workspace_root = temp.path().join("moved-workspace");
        let yoi_dir = workspace_root.join(".yoi");
        fs::create_dir_all(&yoi_dir).unwrap();
        let path = yoi_dir.join("workspace.toml");
        let raw = format!(
            "workspace_id = \"{FIXED_WORKSPACE_ID}\"\ncreated_at = \"{FIXED_CREATED_AT}\"\ndisplay_name = \"Stable Project\"\n"
        );
        fs::write(&path, &raw).unwrap();

        let identity = WorkspaceIdentity::load_or_init_with_clock(&workspace_root, || {
            "2026-06-24T00:00:00Z".to_string()
        })
        .unwrap();

        assert_eq!(identity.workspace_id, FIXED_WORKSPACE_ID);
        assert_eq!(identity.created_at, FIXED_CREATED_AT);
        assert_eq!(identity.display_name, "Stable Project");
        assert_eq!(fs::read_to_string(path).unwrap(), raw);
    }

    #[test]
    fn invalid_identity_file_fails_closed_without_rewriting() {
        let temp = tempfile::tempdir().unwrap();
        let workspace_root = temp.path().join("invalid-workspace");
        let yoi_dir = workspace_root.join(".yoi");
        fs::create_dir_all(&yoi_dir).unwrap();
        let path = yoi_dir.join("workspace.toml");
        let raw = "workspace_id = \"not-a-uuid\"\ncreated_at = \"2026-06-23T06:43:28Z\"\ndisplay_name = \"Invalid\"\n";
        fs::write(&path, raw).unwrap();

        let error = WorkspaceIdentity::load_or_init_with_clock(&workspace_root, || {
            FIXED_CREATED_AT.to_string()
        })
        .unwrap_err();

        assert!(error.to_string().contains("workspace_id is not a UUID"));
        assert_eq!(fs::read_to_string(path).unwrap(), raw);
    }

    #[test]
    fn generated_identity_does_not_leak_parent_paths() {
        let temp = tempfile::tempdir().unwrap();
        let secret_parent = temp.path().join("user-secret-parent");
        let workspace_root = secret_parent.join("public-project-name");
        fs::create_dir_all(&workspace_root).unwrap();

        WorkspaceIdentity::load_or_init_with_clock(&workspace_root, || {
            FIXED_CREATED_AT.to_string()
        })
        .unwrap();
        let raw = fs::read_to_string(WorkspaceIdentity::path(&workspace_root)).unwrap();

        assert!(raw.contains("public-project-name"));
        assert!(!raw.contains(&secret_parent.to_string_lossy().to_string()));
        assert!(!raw.contains("user-secret-parent"));
        assert!(!raw.contains("/"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workspace.toml");
        let raw = format!(
            "workspace_id = \"{FIXED_WORKSPACE_ID}\"\ncreated_at = \"{FIXED_CREATED_AT}\"\ndisplay_name = \"Stable Project\"\nlocal_root = \"/tmp/secret\"\n"
        );

        let error = WorkspaceIdentity::parse_str(&raw, &path).unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }
}
