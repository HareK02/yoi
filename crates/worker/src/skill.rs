use serde::{Deserialize, Serialize};

use crate::worker::WorkspaceClient;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDiagnostic {
    pub severity: SkillDiagnosticSeverity,
    pub code: String,
    pub message: String,
    /// Path-free authority/provenance label such as `builtin:foo` or `workspace:foo`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

impl SkillDiagnostic {
    pub fn error(
        code: impl Into<String>,
        message: impl Into<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            severity: SkillDiagnosticSeverity::Error,
            code: code.into(),
            message: message.into(),
            source,
        }
    }

    pub fn warning(
        code: impl Into<String>,
        message: impl Into<String>,
        source: Option<String>,
    ) -> Self {
        Self {
            severity: SkillDiagnosticSeverity::Warning,
            code: code.into(),
            message: message.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Builtin,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillProvenance {
    pub kind: SkillSourceKind,
    /// Stable path-free id: `builtin:<name>` or `workspace:<name>`.
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillResourceRef {
    pub kind: String,
    /// Skill-relative resource name/path. Never an absolute filesystem path.
    pub name: String,
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub overrides: Vec<SkillProvenance>,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillCatalogResponse {
    /// Authority label for diagnostics; callers must not interpret it as a path.
    pub authority: String,
    #[serde(default)]
    pub entries: Vec<SkillCatalogEntry>,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDetailResponse {
    pub name: String,
    pub description: String,
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub overrides: Vec<SkillProvenance>,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
    /// Full SKILL.md contents. This is intentionally omitted from catalog responses.
    pub body: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Explicitly documents that allowed-tools is parsed only as an experimental hint.
    pub allowed_tools_status: String,
    #[serde(default)]
    pub resources: Vec<SkillResourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillActivationResponse {
    pub name: String,
    pub provenance: SkillProvenance,
    #[serde(default)]
    pub diagnostics: Vec<SkillDiagnostic>,
    /// Full SKILL.md contents to append to Worker history on explicit activation.
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillClientError {
    #[error("workspace client is unavailable: {0}")]
    Unavailable(String),
    #[error("workspace client kind `{0}` does not expose direct Skill HTTP operations")]
    UnsupportedClient(String),
    #[error("Skill request failed: {0}")]
    Request(#[from] crate::worker::WorkspaceClientError),
    #[error("Skill API response JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Skill API returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("invalid Skill API base URL: {0}")]
    InvalidBaseUrl(String),
}

impl dyn WorkspaceClient + '_ {
    pub fn list_skills(&self) -> Result<SkillCatalogResponse, SkillClientError> {
        self.get_skill_json("skills")
    }

    pub fn read_skill(&self, name: &str) -> Result<SkillDetailResponse, SkillClientError> {
        self.get_skill_json(&format!("skills/{name}"))
    }

    pub fn activate_skill(&self, name: &str) -> Result<SkillActivationResponse, SkillClientError> {
        self.get_skill_json(&format!("skills/{name}/activate"))
    }

    fn get_skill_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, SkillClientError> {
        let workspace_id = self
            .workspace_id()
            .ok_or_else(|| SkillClientError::UnsupportedClient(self.kind().to_string()))?;
        let response = self.execute(crate::worker::WorkspaceRequest::get(format!(
            "/api/w/{workspace_id}/{path}"
        )))?;
        let status = reqwest::StatusCode::from_u16(response.status)
            .unwrap_or(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
        if !response.is_success() {
            return Err(SkillClientError::Http {
                status,
                body: response.body,
            });
        }
        Ok(serde_json::from_str(&response.body)?)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn http_workspace_client_fetches_skill_catalog_from_backend_endpoint() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert!(request_line.starts_with("GET /api/w/ws-1/skills HTTP/1.1"));
            let mut runtime_header = None;
            let mut worker_header = None;
            let mut authorization = None;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if let Some(value) = line.strip_prefix("x-yoi-runtime-id: ") {
                    runtime_header = Some(value.trim().to_string());
                }
                if let Some(value) = line.strip_prefix("x-yoi-worker-id: ") {
                    worker_header = Some(value.trim().to_string());
                }
                if let Some(value) = line.strip_prefix("authorization: ") {
                    authorization = Some(value.trim().to_string());
                }
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            assert_eq!(runtime_header, None);
            assert_eq!(worker_header, None);
            assert_eq!(authorization, None);
            let body = serde_json::json!({
                "authority": "workspace-backend-skills-v0",
                "entries": [{
                    "name": "triage-errors",
                    "description": "Use when triaging errors.",
                    "provenance": { "kind": "workspace", "id": "workspace:triage-errors" },
                    "overrides": [],
                    "diagnostics": []
                }],
                "diagnostics": []
            })
            .to_string();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let client = crate::worker::TestWorkspaceHttpClient::new("ws-1", format!("http://{addr}"));
        let catalog = (&client as &dyn WorkspaceClient).list_skills().unwrap();
        assert_eq!(catalog.entries[0].name, "triage-errors");
        assert_eq!(catalog.entries[0].provenance.id, "workspace:triage-errors");
        handle.join().unwrap();
    }
}
