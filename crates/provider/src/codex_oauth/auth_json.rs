//! `~/.codex/auth.json` の読み書き。
//!
//! Codex CLI と schema を共有するが、insomnia は知らないフィールドを
//! 失わないようファイル全体を `serde_json::Value` で保持し、必要箇所
//! のみアクセスする。書込は `mode 0o600` を再設定（Codex CLI 同様）、
//! ファイルロックは取らない（manager 側で guarded reload）。

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::error::CodexAuthError;

/// auth.json から取り出した使い回す情報のスナップショット。
#[derive(Debug, Clone)]
pub struct AuthSnapshot {
    pub access_token: String,
    pub refresh_token: String,
    pub account_id: String,
    pub id_token: String,
    pub last_refresh: Option<DateTime<Utc>>,
    /// 書き戻し時に他のフィールドを失わないため、ファイル全体を保持する。
    pub raw: Value,
}

impl AuthSnapshot {
    /// `Value` から必要フィールドを抽出。欠落・型不一致は `MalformedAuthJson`。
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
            .ok_or_else(|| CodexAuthError::MalformedAuthJson("missing tokens.refresh_token".into()))?
            .to_string();

        let id_token = tokens
            .get("id_token")
            .and_then(Value::as_str)
            .ok_or_else(|| CodexAuthError::MalformedAuthJson("missing tokens.id_token".into()))?
            .to_string();

        // account_id は tokens.account_id を優先、無ければ id_token JWT 由来
        let account_id = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                super::jwt::parse_chatgpt_claims(&id_token).and_then(|c| c.account_id)
            })
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

/// auth.json を読む。存在しなければ `NotLoggedIn`。
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

/// 既存ファイルを再読込し、`tokens.{id_token,access_token,refresh_token}` と
/// `last_refresh` を更新して書き戻す。Codex CLI の `persist_tokens` 相当。
///
/// 並行する Codex CLI / 別 insomnia プロセスが先に refresh していた場合の
/// fields を保護するため、書込前に再 load して merge する。
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
        .insert("last_refresh".into(), Value::String(Utc::now().to_rfc3339()));

    write_atomic(path, raw)?;
    AuthSnapshot::from_value(raw.clone())
}

fn write_atomic(path: &Path, value: &Value) -> Result<(), CodexAuthError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CodexAuthError::Io(format!("create_dir_all {}: {e}", parent.display()))
        })?;
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

    // 既存ファイルが緩いパーミッションだった場合に備えて 0o600 を強制し直す。
    // OpenOptions の `mode()` は新規作成時しか効かないため。
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
        // 未知フィールドが raw に保持されている
        assert_eq!(snap.raw.get("OPENAI_API_KEY").and_then(Value::as_str), Some("sk-extra"));
    }

    #[tokio::test]
    async fn account_id_falls_back_to_jwt_claim() {
        let dir = tempfile::tempdir().unwrap();
        // tokens.account_id を欠落させ、id_token JWT 内 claim から拾う
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
        let updated = persist_refreshed(&path, None, Some("new-acc".into()), Some("new-ref".into())).await.unwrap();
        assert_eq!(updated.access_token, "new-acc");
        assert_eq!(updated.refresh_token, "new-ref");
        // 未知フィールド agent_identity が保たれる
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
        // 既存ファイルを 644 に変えてから persist → 600 に直るか
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        persist_refreshed(&path, None, Some("a2".into()), None).await.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
