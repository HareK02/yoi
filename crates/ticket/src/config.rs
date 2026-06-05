//! Workspace-local Ticket orchestration configuration.
//!
//! The config file lives at `.yoi/ticket.config.toml` under a workspace root.
//! It intentionally stores lightweight string references for Profile selectors,
//! launch prompts, and workflows so this crate remains independent from `pod`
//! and `manifest` runtime resolution.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const TICKET_CONFIG_RELATIVE_PATH: &str = ".yoi/ticket.config.toml";

#[derive(Debug, Error)]
pub enum TicketConfigError {
    #[error("failed to read Ticket config {}: {source}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse Ticket config {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid Ticket config {}: {message}", .path.display())]
    Invalid { path: PathBuf, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketConfig {
    pub backend: TicketBackendConfig,
    pub roles: TicketRoleProfiles,
}

impl TicketConfig {
    pub fn default_for_workspace(workspace_root: impl AsRef<Path>) -> Self {
        let workspace_root = workspace_root.as_ref();
        Self {
            backend: TicketBackendConfig::default_for_workspace(workspace_root),
            roles: TicketRoleProfiles::default(),
        }
    }

    pub fn load_workspace(workspace_root: impl AsRef<Path>) -> Result<Self, TicketConfigError> {
        let workspace_root = workspace_root.as_ref();
        let path = workspace_root.join(TICKET_CONFIG_RELATIVE_PATH);
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default_for_workspace(workspace_root));
            }
            Err(source) => return Err(TicketConfigError::Read { path, source }),
        };
        Self::from_toml(workspace_root, &path, &content)
    }

    pub fn from_toml(
        workspace_root: impl AsRef<Path>,
        path: impl AsRef<Path>,
        content: &str,
    ) -> Result<Self, TicketConfigError> {
        let workspace_root = workspace_root.as_ref();
        let path = path.as_ref();
        let raw: RawTicketConfig =
            toml::from_str(content).map_err(|source| TicketConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        raw.resolve(workspace_root, path)
    }

    pub fn backend_root(&self) -> &Path {
        self.backend.root.as_path()
    }

    pub fn role(&self, role: TicketRole) -> &TicketRoleConfig {
        self.roles.get(role)
    }

    pub fn profile_for(&self, role: TicketRole) -> &ProfileSelectorRef {
        &self.role(role).profile
    }

    pub fn launch_prompt_for(&self, role: TicketRole) -> Option<&PromptRef> {
        self.role(role).launch_prompt.as_ref()
    }

    pub fn workflow_for(&self, role: TicketRole) -> &WorkflowRef {
        &self.role(role).workflow
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketBackendConfig {
    pub provider: TicketBackendProvider,
    pub root: PathBuf,
}

impl TicketBackendConfig {
    pub fn default_for_workspace(workspace_root: &Path) -> Self {
        Self {
            provider: TicketBackendProvider::BuiltinYoiLocal,
            root: workspace_root.join("work-items"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TicketBackendProvider {
    #[serde(rename = "builtin:yoi_local")]
    BuiltinYoiLocal,
}

impl TicketBackendProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuiltinYoiLocal => "builtin:yoi_local",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "builtin:yoi_local" => Some(Self::BuiltinYoiLocal),
            _ => None,
        }
    }
}

impl fmt::Display for TicketBackendProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketRole {
    Intake,
    Orchestrator,
    Coder,
    Reviewer,
    Investigator,
}

impl TicketRole {
    pub const ALL: [TicketRole; 5] = [
        TicketRole::Intake,
        TicketRole::Orchestrator,
        TicketRole::Coder,
        TicketRole::Reviewer,
        TicketRole::Investigator,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Orchestrator => "orchestrator",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
            Self::Investigator => "investigator",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "intake" => Some(Self::Intake),
            "orchestrator" => Some(Self::Orchestrator),
            "coder" => Some(Self::Coder),
            "reviewer" => Some(Self::Reviewer),
            "investigator" => Some(Self::Investigator),
            _ => None,
        }
    }

    pub fn default_workflow(self) -> &'static str {
        match self {
            Self::Intake => "ticket-intake-workflow",
            Self::Orchestrator => "ticket-orchestrator-routing",
            Self::Coder | Self::Reviewer => "multi-agent-workflow",
            Self::Investigator => "ticket-orchestrator-routing",
        }
    }
}

impl fmt::Display for TicketRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleProfiles {
    inner: BTreeMap<TicketRole, TicketRoleConfig>,
}

impl TicketRoleProfiles {
    pub fn get(&self, role: TicketRole) -> &TicketRoleConfig {
        self.inner
            .get(&role)
            .expect("TicketRoleProfiles always contains all fixed roles")
    }

    pub fn iter(&self) -> impl Iterator<Item = (TicketRole, &TicketRoleConfig)> {
        TicketRole::ALL
            .into_iter()
            .map(|role| (role, self.get(role)))
    }
}

impl Default for TicketRoleProfiles {
    fn default() -> Self {
        let inner = TicketRole::ALL
            .into_iter()
            .map(|role| (role, TicketRoleConfig::default_for_role(role)))
            .collect();
        Self { inner }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleConfig {
    pub profile: ProfileSelectorRef,
    pub launch_prompt: Option<PromptRef>,
    pub workflow: WorkflowRef,
}

impl TicketRoleConfig {
    pub fn default_for_role(role: TicketRole) -> Self {
        Self {
            profile: ProfileSelectorRef::inherit(),
            launch_prompt: None,
            workflow: WorkflowRef::from_static(role.default_workflow()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct ProfileSelectorRef(String);

impl ProfileSelectorRef {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = normalized_non_empty(value, "profile selector")?;
        if value.starts_with("path:")
            || value.starts_with('.')
            || value.contains('/')
            || value.ends_with(".lua")
            || value.ends_with(".nix")
        {
            return Err("profile selector must be `inherit`, `default`, a source-qualified registry selector, or an unqualified registry selector; path selectors are not supported".to_string());
        }
        if let Some((source, name)) = value.split_once(':') {
            if !matches!(source, "builtin" | "user" | "project") {
                return Err(
                    "profile selector source must be one of `builtin`, `user`, or `project`"
                        .to_string(),
                );
            }
            if name.trim().is_empty() {
                return Err("profile selector registry name must not be empty".to_string());
            }
        }
        Ok(Self(value))
    }

    pub fn inherit() -> Self {
        Self("inherit".to_string())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for ProfileSelectorRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for ProfileSelectorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for ProfileSelectorRef {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct PromptRef(String);

impl PromptRef {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        normalized_non_empty(value, "launch prompt ref").map(Self)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for PromptRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for PromptRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for PromptRef {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct WorkflowRef(String);

impl WorkflowRef {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        normalized_non_empty(value, "workflow ref").map(Self)
    }

    pub fn from_static(value: &'static str) -> Self {
        Self(value.to_string())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for WorkflowRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for WorkflowRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for WorkflowRef {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

fn normalized_non_empty(value: impl Into<String>, label: &str) -> Result<String, String> {
    let value = value.into();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("{label} must not be empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTicketConfig {
    #[serde(default)]
    backend: RawBackendConfig,
    #[serde(default)]
    roles: BTreeMap<String, RawTicketRoleConfig>,
}

impl RawTicketConfig {
    fn resolve(
        self,
        workspace_root: &Path,
        path: &Path,
    ) -> Result<TicketConfig, TicketConfigError> {
        let mut roles = TicketRoleProfiles::default();
        for (name, raw_role) in self.roles {
            let role = TicketRole::parse(&name).ok_or_else(|| TicketConfigError::Invalid {
                path: path.to_path_buf(),
                message: format!("unknown Ticket role `{name}`"),
            })?;
            roles.inner.insert(role, raw_role.resolve(role));
        }
        Ok(TicketConfig {
            backend: self.backend.resolve(workspace_root).map_err(|message| {
                TicketConfigError::Invalid {
                    path: path.to_path_buf(),
                    message,
                }
            })?,
            roles,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBackendConfig {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    root: Option<PathBuf>,
}

impl RawBackendConfig {
    fn resolve(self, workspace_root: &Path) -> Result<TicketBackendConfig, String> {
        let provider = match (self.provider, self.kind) {
            (Some(provider), None) => TicketBackendProvider::parse(&provider).ok_or_else(|| {
                format!(
                    "unsupported Ticket backend provider `{provider}`; supported provider: `builtin:yoi_local`"
                )
            })?,
            (None, Some(kind)) if kind == "local" => TicketBackendProvider::BuiltinYoiLocal,
            (None, Some(kind)) => {
                return Err(format!(
                    "unsupported legacy Ticket backend kind `{kind}`; use provider = \"builtin:yoi_local\""
                ));
            }
            (None, None) => TicketBackendProvider::BuiltinYoiLocal,
            (Some(_), Some(_)) => {
                return Err(
                    "backend.provider and legacy backend.kind are mutually exclusive; use provider = \"builtin:yoi_local\""
                        .to_string(),
                );
            }
        };
        let root = self.root.unwrap_or_else(|| PathBuf::from("work-items"));
        Ok(TicketBackendConfig {
            provider,
            root: join_if_relative(workspace_root, &root),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTicketRoleConfig {
    profile: ProfileSelectorRef,
    #[serde(default)]
    launch_prompt: Option<PromptRef>,
    #[serde(default)]
    workflow: Option<WorkflowRef>,
}

impl RawTicketRoleConfig {
    fn resolve(self, role: TicketRole) -> TicketRoleConfig {
        TicketRoleConfig {
            profile: self.profile,
            launch_prompt: self.launch_prompt,
            workflow: self
                .workflow
                .unwrap_or_else(|| WorkflowRef::from_static(role.default_workflow())),
        }
    }
}

fn join_if_relative(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_config(workspace: &Path, content: &str) {
        let dir = workspace.join(".yoi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ticket.config.toml"), content).unwrap();
    }

    #[test]
    fn missing_config_returns_documented_defaults() {
        let temp = TempDir::new().unwrap();
        let config = TicketConfig::load_workspace(temp.path()).unwrap();

        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(config.backend.root, temp.path().join("work-items"));
        for role in TicketRole::ALL {
            let role_config = config.role(role);
            assert_eq!(role_config.profile.as_str(), "inherit");
            assert!(role_config.launch_prompt.is_none());
            assert_eq!(role_config.workflow.as_str(), role.default_workflow());
        }
    }

    #[test]
    fn full_config_parses_fixed_role_refs() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = "custom-work-items"

[roles.intake]
profile = "project:intake"
launch_prompt = "$workspace/ticket/intake/launch"
workflow = "ticket-intake-workflow"

[roles.orchestrator]
profile = "project:orchestrator"
launch_prompt = "$workspace/ticket/orchestrator/launch"
workflow = "ticket-orchestrator-routing"

[roles.coder]
profile = "inherit"
launch_prompt = "$workspace/ticket/coder/launch"
workflow = "multi-agent-workflow"

[roles.reviewer]
profile = "project:reviewer"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "multi-agent-workflow"

[roles.investigator]
profile = "default"
launch_prompt = "$workspace/ticket/investigator/launch"
workflow = "ticket-orchestrator-routing"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(config.backend.root, temp.path().join("custom-work-items"));
        assert_eq!(
            config.profile_for(TicketRole::Intake).as_str(),
            "project:intake"
        );
        assert_eq!(
            config
                .launch_prompt_for(TicketRole::Reviewer)
                .unwrap()
                .as_str(),
            "$workspace/ticket/reviewer/launch"
        );
        assert_eq!(
            config.workflow_for(TicketRole::Investigator).as_str(),
            "ticket-orchestrator-routing"
        );
    }

    #[test]
    fn partial_role_config_keeps_role_defaults() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.coder]
profile = "project:coder"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        let coder = config.role(TicketRole::Coder);
        assert_eq!(coder.profile.as_str(), "project:coder");
        assert!(coder.launch_prompt.is_none());
        assert_eq!(coder.workflow.as_str(), "multi-agent-workflow");
        assert_eq!(config.profile_for(TicketRole::Reviewer).as_str(), "inherit");
    }

    #[test]
    fn unknown_roles_are_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.operator]
profile = "inherit"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(error.to_string().contains("unknown Ticket role `operator`"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.coder]
profile = "inherit"
system_instruction = "$workspace/not-supported"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(error.to_string().contains("unknown field"));
        assert!(error.to_string().contains("system_instruction"));
    }

    #[test]
    fn legacy_backend_kind_local_is_transitional_alias() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
kind = "local"
root = "legacy-work-items"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(config.backend_root(), temp.path().join("legacy-work-items"));
    }

    #[test]
    fn unsupported_backend_provider_is_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "github"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported Ticket backend provider `github`")
        );
        assert!(error.to_string().contains("builtin:yoi_local"));
    }

    #[test]
    fn unsupported_legacy_backend_kind_is_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
kind = "github"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported legacy Ticket backend kind `github`")
        );
    }

    #[test]
    fn backend_provider_and_legacy_kind_are_mutually_exclusive() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
kind = "local"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("backend.provider and legacy backend.kind are mutually exclusive")
        );
    }

    #[test]
    fn relative_backend_root_resolves_against_workspace() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
root = "nested/work-items"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend_root(), temp.path().join("nested/work-items"));
    }

    #[test]
    fn nix_profile_selector_refs_are_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.coder]
profile = "legacy.nix"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("path selectors are not supported")
        );
    }

    #[test]
    fn malformed_refs_are_reported() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.coder]
profile = "./coder.lua"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("path selectors are not supported")
        );
    }
}
