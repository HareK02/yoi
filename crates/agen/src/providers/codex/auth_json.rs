//! Reading and writing `~/.codex/auth.json`.
//!
//! Shares the schema with Codex CLI, but retains the entire file as
//! `serde_json::Value` and accesses only the required fields to preserve
//! unknown fields. Writes reapply `mode 0o600` (as in Codex CLI),
//! without file locking (the manager performs a guarded reload).

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::error::CodexAuthError;

/// Snapshot of reusable information extracted from auth.json.
#[derive(Debug, Clone)]
pub struct AuthSnapshot {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub id_token: String,
    pub last_refresh: Option<DateTime<Utc>>,
    /// Retain the entire file to preserve other fields when writing back.
    pub raw: Value,
}

impl AuthSnapshot {
    /// Extract required fields from `Value`. Missing fields or type mismatches yield `MalformedAuthJson`.
    pub fn from_value(raw: Value) -> Result<Self, CodexAuthError> {
        let tokens = raw
            .get("tokens")
            .ok_or_else(|| CodexAuthError::MalformedAuthJson("missing 'tokens'".into()))?;

        let access_token = tokens
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| CodexAuthError::MalformedAuthJson("missing tokens.access_token".into()))?
            .to_string();

        let refresh_token = tokens
            .get("refresh_token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CodexAuthError::MalformedAuthJson("missing tokens.refresh_token".into())
            })?
            .to_string();

        let id_token = tokens
            .get("id_token")
            .and_then(Value::as_str)
            .ok_or_else(|| CodexAuthError::MalformedAuthJson("missing tokens.id_token".into()))?
            .to_string();

        // Prefer tokens.account_id; fall back to the id_token JWT if absent
        let account_id = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| super::jwt::parse_chatgpt_claims(&id_token).and_then(|c| c.account_id))
            .ok_or_else(|| {
                CodexAuthError::MalformedAuthJson(
                    "missing account_id in both tokens and id_token claims".into(),
                )
            })?;

        let last_refresh = raw
            .get("last_refresh")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(Self {
            access_token,
            refresh_token,
            account_id,
            id_token,
            last_refresh,
            raw,
        })
    }
}

/// Read auth.json. Return `NotLoggedIn` if it does not exist.
pub async fn load(path: &Path) -> Result<AuthSnapshot, CodexAuthError> {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(CodexAuthError::NotLoggedIn(path.to_path_buf()));
        }
        Err(err) => {
            return Err(CodexAuthError::Io(format!(
                "failed to read {}: {err}",
                path.display()
            )));
        }
    };
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|e| CodexAuthError::MalformedAuthJson(format!("json parse: {e}")))?;
    AuthSnapshot::from_value(raw)
}

/// Reload the existing file, update `tokens.{id_token,access_token,refresh_token}`
/// and `last_refresh`, and write it back. Equivalent to Codex CLI's `persist_tokens`.
///
/// Reload and merge before writing to preserve fields if a concurrent
/// Codex CLI / another process has already refreshed.
pub async fn persist_refreshed(
    path: &Path,
    new_id_token: Option<String>,
    new_access_token: Option<String>,
    new_refresh_token: Option<String>,
) -> Result<AuthSnapshot, CodexAuthError> {
    let mut current = load(path).await?;
    let raw = &mut current.raw;
    let tokens = raw
        .get_mut("tokens")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CodexAuthError::MalformedAuthJson("tokens not an object".into()))?;
    if let Some(t) = new_id_token {
        tokens.insert("id_token".into(), Value::String(t));
    }
    if let Some(t) = new_access_token {
        tokens.insert("access_token".into(), Value::String(t));
    }
    if let Some(t) = new_refresh_token {
        tokens.insert("refresh_token".into(), Value::String(t));
    }
    raw.as_object_mut()
        .ok_or_else(|| CodexAuthError::MalformedAuthJson("auth.json not an object".into()))?
        .insert(
            "last_refresh".into(),
            Value::String(Utc::now().to_rfc3339()),
        );

    write_atomic(path, raw)?;
    AuthSnapshot::from_value(raw.clone())
}

fn write_atomic(path: &Path, value: &Value) -> Result<(), CodexAuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CodexAuthError::Io(format!("create_dir_all {}: {e}", parent.display())))?;
    }
    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| CodexAuthError::Io(format!("serialize: {e}")))?;
    let mut options = OpenOptions::new();
    options.truncate(true).write(true).create(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|e| CodexAuthError::Io(format!("open {}: {e}", path.display())))?;
    file.write_all(&json)
        .map_err(|e| CodexAuthError::Io(format!("write {}: {e}", path.display())))?;
    file.flush()
        .map_err(|e| CodexAuthError::Io(format!("flush {}: {e}", path.display())))?;

    // Reapply 0o600 in case the existing file had permissive permissions.
    // OpenOptions `mode()` only takes effect when creating a new file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CodexAuthError::Io(format!("chmod {}: {e}", path.display())))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_auth_json(dir: &Path, content: &str) -> std::path::PathBuf {
        let path = dir.join("auth.json");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_auth_json(
            dir.path(),
            r#"{
              "auth_mode":"ChatgptAuthTokens",
              "tokens":{
                "id_token":"h.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYWNjLTEifX0.s",
                "access_token":"acc",
                "refresh_token":"ref",
                "account_id":"acc-1"
              },
              "last_refresh":"2026-04-20T00:00:00Z",
              "OPENAI_API_KEY":"sk-extra"
            }"#,
        );
        let snap = load(&path).await.unwrap();
        assert_eq!(snap.access_token, "acc");
        assert_eq!(snap.refresh_token, "ref");
        assert_eq!(snap.account_id, "acc-1");
        assert!(snap.last_refresh.is_some());
        // Unknown fields are retained in raw
        assert_eq!(
            snap.raw.get("OPENAI_API_KEY").and_then(Value::as_str),
            Some("sk-extra")
        );
    }

    #[tokio::test]
    async fn account_id_falls_back_to_jwt_claim() {
        let dir = tempfile::tempdir().unwrap();
        // Omit tokens.account_id and obtain it from a claim in the id_token JWT
        let id_token = "h.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiZnJvbS1qd3QifX0.s";
        let path = write_auth_json(
            dir.path(),
            &format!(
                r#"{{ "tokens": {{ "id_token":"{id_token}", "access_token":"a", "refresh_token":"r" }} }}"#
            ),
        );
        let snap = load(&path).await.unwrap();
        assert_eq!(snap.account_id, "from-jwt");
    }

    #[tokio::test]
    async fn missing_file_returns_not_logged_in() {
        let dir = tempfile::tempdir().unwrap();
        let err = load(&dir.path().join("nope.json")).await.unwrap_err();
        assert!(matches!(err, CodexAuthError::NotLoggedIn(_)));
    }

    #[tokio::test]
    async fn persist_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_auth_json(
            dir.path(),
            r#"{
              "tokens":{"id_token":"h.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYSJ9fQ.s","access_token":"old-acc","refresh_token":"old-ref","account_id":"a"},
              "agent_identity":{"workspace_id":"w","agent_runtime_id":"r","agent_private_key":"k","registered_at":"x"}
            }"#,
        );
        let updated =
            persist_refreshed(&path, None, Some("new-acc".into()), Some("new-ref".into()))
                .await
                .unwrap();
        assert_eq!(updated.access_token, "new-acc");
        assert_eq!(updated.refresh_token, "new-ref");
        // The unknown field agent_identity is preserved
        let on_disk: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(on_disk.get("agent_identity").is_some());
        assert!(on_disk.get("last_refresh").is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn write_uses_mode_600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = write_auth_json(
            dir.path(),
            r#"{"tokens":{"id_token":"h.eyJodHRwczovL2FwaS5vcGVuYWkuY29tL2F1dGgiOnsiY2hhdGdwdF9hY2NvdW50X2lkIjoiYSJ9fQ.s","access_token":"a","refresh_token":"r","account_id":"a"}}"#,
        );
        // Change the existing file to 644, then persist → check that it returns to 600
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        persist_refreshed(&path, None, Some("a2".into()), None)
            .await
            .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
