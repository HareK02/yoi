//! Workspace-local Ticket orchestration configuration.
//!
//! Durable Ticket policy lives under the `[ticket]` table in tracked
//! `.yoi/workspace.toml` workspace settings. The legacy
//! `.yoi/ticket.config.toml` file is only a read-only migration fallback when
//! workspace settings do not contain any Ticket policy. The config intentionally
//! stores lightweight string references for Profile selectors, launch prompts,
//! and workflows so this crate remains independent from `worker` and `manifest`
//! runtime resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WORKSPACE_SETTINGS_RELATIVE_PATH: &str = ".yoi/workspace.toml";
/// Legacy Ticket config path. This is kept as a narrow read-only migration fallback only.
pub const TICKET_CONFIG_RELATIVE_PATH: &str = ".yoi/ticket.config.toml";
/// Workspace-relative default root for the built-in local Ticket backend.
pub const DEFAULT_TICKET_BACKEND_RELATIVE_PATH: &str = ".yoi/tickets";
const DEFAULT_ORCHESTRATION_BRANCH: &str = "orchestration";
const DEFAULT_ORCHESTRATION_WORKTREE_DIR: &str = ".worktree";
const DEFAULT_ORCHESTRATION_WORKTREE_NAME: &str = "orchestration";

/// Return the explicit Workspace settings Ticket policy scaffold written by `yoi ticket init`.
///
/// The scaffold is a valid `.yoi/workspace.toml` fragment rooted at `[ticket]`.
/// It intentionally configures every fixed Ticket role with a concrete profile
/// so strict role launch planning can validate the config without runtime
/// fallback.
pub fn ticket_config_scaffold() -> String {
    let mut out = String::from(
        "[ticket]\n# Optional durable Ticket record language. When unset, generated Ticket text keeps current defaults.\n# language = \"Japanese\"\n",
    );
    out.push_str("\n[ticket.backend]\n");
    out.push_str(&format!(
        "provider = \"{}\"\n",
        TicketBackendProvider::BuiltinYoiLocal.as_str()
    ));
    out.push_str(&format!(
        "root = \"{}\"\n",
        DEFAULT_TICKET_BACKEND_RELATIVE_PATH
    ));
    out.push_str(
        "\n# Optional Panel Orchestrator worktree settings. When unset, Panel uses branch `orchestration` at `.worktree/orchestration`.\n# [ticket.orchestration]\n# branch = \"orchestration\"\n# worktree_dir = \".worktree\"\n# worktree_name = \"orchestration\"\n",
    );
    for role in TicketRole::ALL {
        out.push_str(&format!(
            "\n[ticket.roles.{role}]\nprofile = \"{}\"\nworkflow = \"{}\"\n",
            role.default_profile(),
            role.default_workflow()
        ));
    }
    out
}

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
    pub ticket: TicketRecordConfig,
    pub orchestration: TicketOrchestrationConfig,
    pub roles: TicketRoleProfiles,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TicketOrchestrationConfig {
    pub branch: Option<GitBranchName>,
    pub worktree_dir: Option<PathBuf>,
    pub worktree_name: Option<PathBuf>,
}

impl TicketOrchestrationConfig {
    pub fn branch_name(&self) -> Option<&str> {
        self.branch.as_ref().map(GitBranchName::as_str)
    }

    pub fn effective_branch_name(&self) -> &str {
        self.branch_name().unwrap_or(DEFAULT_ORCHESTRATION_BRANCH)
    }

    pub fn worktree_dir(&self) -> &Path {
        self.worktree_dir
            .as_deref()
            .unwrap_or_else(|| Path::new(DEFAULT_ORCHESTRATION_WORKTREE_DIR))
    }

    pub fn worktree_name(&self) -> &Path {
        self.worktree_name
            .as_deref()
            .unwrap_or_else(|| Path::new(DEFAULT_ORCHESTRATION_WORKTREE_NAME))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct GitBranchName(String);

impl GitBranchName {
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed != value {
            return Err("git branch name must not have leading or trailing whitespace".to_string());
        }
        validate_git_branch_name_value(trimmed)?;
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for GitBranchName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for GitBranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

fn validate_git_branch_name_value(value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("git branch name must not be empty".to_string());
    }
    if value == "@" {
        return Err("git branch name must not be `@`".to_string());
    }
    if value.starts_with('-') {
        return Err("git branch name must not start with `-`".to_string());
    }
    if value.starts_with("refs/") {
        return Err("git branch name must be a short branch name, not a full ref".to_string());
    }
    if value.starts_with('/') || value.ends_with('/') || value.contains("//") {
        return Err("git branch name must not contain empty path components".to_string());
    }
    if value.contains("..") {
        return Err("git branch name must not contain `..`".to_string());
    }
    if value.contains("@{") {
        return Err("git branch name must not contain `@{`".to_string());
    }
    if value.ends_with('.') {
        return Err("git branch name must not end with `.`".to_string());
    }

    for component in value.split('/') {
        if component.starts_with('.') {
            return Err("git branch name components must not start with `.`".to_string());
        }
        if component.ends_with(".lock") {
            return Err("git branch name components must not end with `.lock`".to_string());
        }
    }

    for ch in value.chars() {
        if ch.is_control() || matches!(ch, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\') {
            return Err(format!(
                "git branch name contains unsupported character `{}`",
                ch.escape_default()
            ));
        }
    }

    Ok(())
}

fn validate_orchestration_relative_path(path: &Path, label: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if path.is_absolute() {
        return Err(format!("{label} must be workspace-relative"));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir | Component::ParentDir => {
                return Err(format!("{label} must not contain `.` or `..` components"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{label} must be workspace-relative"));
            }
        }
    }
    Ok(())
}

impl TicketConfig {
    pub fn default_for_workspace(workspace_root: impl AsRef<Path>) -> Self {
        let workspace_root = workspace_root.as_ref();
        Self {
            backend: TicketBackendConfig::default_for_workspace(workspace_root),
            ticket: TicketRecordConfig::default(),
            orchestration: TicketOrchestrationConfig::default(),
            roles: TicketRoleProfiles::default(),
        }
    }

    pub fn load_workspace(workspace_root: impl AsRef<Path>) -> Result<Self, TicketConfigError> {
        let workspace_root = workspace_root.as_ref();
        let workspace_settings_path = workspace_root.join(WORKSPACE_SETTINGS_RELATIVE_PATH);
        match fs::read_to_string(&workspace_settings_path) {
            Ok(content) => {
                if let Some(config) = Self::from_workspace_settings_toml(
                    workspace_root,
                    &workspace_settings_path,
                    &content,
                )? {
                    return Ok(config);
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TicketConfigError::Read {
                    path: workspace_settings_path,
                    source,
                });
            }
        }

        // Narrow read-only migration fallback for pre-workspace-settings projects.
        // As soon as `.yoi/workspace.toml` contains any `[ticket]` table, the
        // workspace settings authority wins and this legacy file is ignored.
        let legacy_path = workspace_root.join(TICKET_CONFIG_RELATIVE_PATH);
        let legacy_content = match fs::read_to_string(&legacy_path) {
            Ok(content) => content,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default_for_workspace(workspace_root));
            }
            Err(source) => {
                return Err(TicketConfigError::Read {
                    path: legacy_path,
                    source,
                });
            }
        };
        Self::from_toml(workspace_root, &legacy_path, &legacy_content)
    }

    pub fn from_workspace_settings_toml(
        workspace_root: impl AsRef<Path>,
        path: impl AsRef<Path>,
        content: &str,
    ) -> Result<Option<Self>, TicketConfigError> {
        let workspace_root = workspace_root.as_ref();
        let path = path.as_ref();
        let value: toml::Value =
            toml::from_str(content).map_err(|source| TicketConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        let Some(ticket_value) = value.get("ticket").cloned() else {
            return Ok(None);
        };
        let raw: RawWorkspaceTicketConfig =
            ticket_value
                .try_into()
                .map_err(|source| TicketConfigError::Parse {
                    path: path.to_path_buf(),
                    source,
                })?;
        raw.resolve(workspace_root, path).map(Some)
    }

    pub fn workspace_settings_has_ticket_config(
        path: impl AsRef<Path>,
        content: &str,
    ) -> Result<bool, TicketConfigError> {
        let path = path.as_ref();
        let value: toml::Value =
            toml::from_str(content).map_err(|source| TicketConfigError::Parse {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(value.get("ticket").is_some())
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

    pub fn ticket_record_language(&self) -> Option<&str> {
        self.ticket
            .language
            .as_ref()
            .map(TicketRecordLanguage::as_str)
    }

    pub fn role(&self, role: TicketRole) -> &TicketRoleConfig {
        self.roles.get(role)
    }

    pub fn role_launch_config(
        &self,
        role: TicketRole,
    ) -> Result<&TicketRoleConfig, TicketRoleLaunchConfigError> {
        self.roles.launch_config(role)
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
            root: workspace_root.join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH),
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
}

impl TicketRole {
    pub const ALL: [TicketRole; 4] = [
        TicketRole::Intake,
        TicketRole::Orchestrator,
        TicketRole::Coder,
        TicketRole::Reviewer,
    ];

    pub fn supported_names() -> Vec<&'static str> {
        Self::ALL.iter().map(|role| role.as_str()).collect()
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Orchestrator => "orchestrator",
            Self::Coder => "coder",
            Self::Reviewer => "reviewer",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "intake" => Some(Self::Intake),
            "orchestrator" => Some(Self::Orchestrator),
            "coder" => Some(Self::Coder),
            "reviewer" => Some(Self::Reviewer),
            _ => None,
        }
    }

    pub fn default_workflow(self) -> &'static str {
        match self {
            Self::Intake => "ticket-intake-workflow",
            Self::Orchestrator => "ticket-orchestrator-routing",
            Self::Coder | Self::Reviewer => "multi-agent-workflow",
        }
    }

    pub fn default_profile(self) -> &'static str {
        match self {
            Self::Intake => "builtin:intake",
            Self::Orchestrator => "builtin:orchestrator",
            Self::Coder => "builtin:coder",
            Self::Reviewer => "builtin:reviewer",
        }
    }
}

impl fmt::Display for TicketRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRecordConfig {
    pub language: Option<TicketRecordLanguage>,
}

impl Default for TicketRecordConfig {
    fn default() -> Self {
        Self { language: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRecordLanguage(String);

impl TicketRecordLanguage {
    pub fn new(language: impl Into<String>) -> Result<Self, String> {
        let language = normalized_non_empty(language, "ticket record language")?;
        Ok(Self(language))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl<'de> Deserialize<'de> for TicketRecordLanguage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        TicketRecordLanguage::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRoleProfiles {
    inner: BTreeMap<TicketRole, TicketRoleConfig>,
    configured_roles: BTreeSet<TicketRole>,
    profile_configured_roles: BTreeSet<TicketRole>,
}

impl TicketRoleProfiles {
    pub fn get(&self, role: TicketRole) -> &TicketRoleConfig {
        self.inner
            .get(&role)
            .expect("TicketRoleProfiles always contains all fixed roles")
    }

    pub fn role_is_configured(&self, role: TicketRole) -> bool {
        self.configured_roles.contains(&role)
    }

    pub fn profile_is_configured(&self, role: TicketRole) -> bool {
        self.profile_configured_roles.contains(&role)
    }

    pub fn launch_config(
        &self,
        role: TicketRole,
    ) -> Result<&TicketRoleConfig, TicketRoleLaunchConfigError> {
        if !self.role_is_configured(role) {
            return Err(TicketRoleLaunchConfigError::MissingRoleTable { role });
        }
        if !self.profile_is_configured(role) {
            return Err(TicketRoleLaunchConfigError::MissingProfile { role });
        }
        let config = self.get(role);
        if config.profile.as_str() == "inherit" {
            return Err(TicketRoleLaunchConfigError::InheritProfile { role });
        }
        Ok(config)
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
        Self {
            inner,
            configured_roles: BTreeSet::new(),
            profile_configured_roles: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TicketRoleLaunchConfigError {
    #[error(
        "Ticket role `{role}` is not launch-configured; add `[ticket.roles.{role}]` with the role builtin profile or another executable concrete profile selector"
    )]
    MissingRoleTable { role: TicketRole },
    #[error(
        "Ticket role `{role}` has no launch profile; set `[ticket.roles.{role}].profile` to the role builtin profile or another executable concrete profile selector"
    )]
    MissingProfile { role: TicketRole },
    #[error(
        "Ticket role `{role}` uses `profile = \"inherit\"`; top-level Ticket role launch requires an explicit executable profile selector such as the role builtin profile or a project/user profile"
    )]
    InheritProfile { role: TicketRole },
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
            || value.ends_with(".dcdl")
            || value.ends_with(".json")
            || value.ends_with(".toml")
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
    ticket: RawTicketRecordConfig,
    #[serde(default)]
    orchestration: RawTicketOrchestrationConfig,
    #[serde(default)]
    roles: BTreeMap<String, RawTicketRoleConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWorkspaceTicketConfig {
    #[serde(default)]
    backend: RawBackendConfig,
    #[serde(default)]
    language: Option<TicketRecordLanguage>,
    #[serde(default)]
    orchestration: RawTicketOrchestrationConfig,
    #[serde(default)]
    roles: BTreeMap<String, RawTicketRoleConfig>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTicketOrchestrationConfig {
    #[serde(default)]
    branch: Option<GitBranchName>,
    #[serde(default)]
    worktree_dir: Option<PathBuf>,
    #[serde(default)]
    worktree_name: Option<PathBuf>,
}

impl RawTicketOrchestrationConfig {
    fn resolve(self) -> Result<TicketOrchestrationConfig, String> {
        if let Some(path) = &self.worktree_dir {
            validate_orchestration_relative_path(path, "orchestration.worktree_dir")?;
        }
        if let Some(path) = &self.worktree_name {
            validate_orchestration_relative_path(path, "orchestration.worktree_name")?;
        }
        Ok(TicketOrchestrationConfig {
            branch: self.branch,
            worktree_dir: self.worktree_dir,
            worktree_name: self.worktree_name,
        })
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTicketRecordConfig {
    #[serde(default)]
    language: Option<TicketRecordLanguage>,
}

impl RawTicketRecordConfig {
    fn resolve(self) -> TicketRecordConfig {
        TicketRecordConfig {
            language: self.language,
        }
    }
}

impl RawTicketConfig {
    fn resolve(
        self,
        workspace_root: &Path,
        path: &Path,
    ) -> Result<TicketConfig, TicketConfigError> {
        resolve_ticket_config_parts(
            self.backend,
            self.ticket.resolve(),
            self.orchestration,
            self.roles,
            workspace_root,
            path,
        )
    }
}

impl RawWorkspaceTicketConfig {
    fn resolve(
        self,
        workspace_root: &Path,
        path: &Path,
    ) -> Result<TicketConfig, TicketConfigError> {
        resolve_ticket_config_parts(
            self.backend,
            TicketRecordConfig {
                language: self.language,
            },
            self.orchestration,
            self.roles,
            workspace_root,
            path,
        )
    }
}

fn resolve_ticket_config_parts(
    backend: RawBackendConfig,
    ticket: TicketRecordConfig,
    orchestration: RawTicketOrchestrationConfig,
    raw_roles: BTreeMap<String, RawTicketRoleConfig>,
    workspace_root: &Path,
    path: &Path,
) -> Result<TicketConfig, TicketConfigError> {
    let mut roles = TicketRoleProfiles::default();
    for (name, raw_role) in raw_roles {
        let role = TicketRole::parse(&name).ok_or_else(|| TicketConfigError::Invalid {
            path: path.to_path_buf(),
            message: format!(
                "unsupported Ticket role `{name}`; supported fixed roles: {}",
                TicketRole::supported_names().join(", ")
            ),
        })?;
        let profile_configured = raw_role.profile.is_some();
        roles.inner.insert(role, raw_role.resolve(role));
        roles.configured_roles.insert(role);
        if profile_configured {
            roles.profile_configured_roles.insert(role);
        }
    }
    Ok(TicketConfig {
        backend: backend
            .resolve(workspace_root)
            .map_err(|message| TicketConfigError::Invalid {
                path: path.to_path_buf(),
                message,
            })?,
        ticket,
        orchestration: orchestration
            .resolve()
            .map_err(|message| TicketConfigError::Invalid {
                path: path.to_path_buf(),
                message,
            })?,
        roles,
    })
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
        let root = self
            .root
            .unwrap_or_else(|| PathBuf::from(DEFAULT_TICKET_BACKEND_RELATIVE_PATH));
        Ok(TicketBackendConfig {
            provider,
            root: join_if_relative(workspace_root, &root),
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTicketRoleConfig {
    #[serde(default)]
    profile: Option<ProfileSelectorRef>,
    #[serde(default)]
    launch_prompt: Option<PromptRef>,
    #[serde(default)]
    workflow: Option<WorkflowRef>,
}

impl RawTicketRoleConfig {
    fn resolve(self, role: TicketRole) -> TicketRoleConfig {
        TicketRoleConfig {
            profile: self.profile.unwrap_or_else(ProfileSelectorRef::inherit),
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

    fn write_workspace_settings(workspace: &Path, content: &str) {
        let dir = workspace.join(".yoi");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("workspace.toml"), content).unwrap();
    }

    #[test]
    fn missing_config_returns_documented_defaults() {
        let temp = TempDir::new().unwrap();
        let config = TicketConfig::load_workspace(temp.path()).unwrap();

        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(
            config.backend.root,
            temp.path().join(DEFAULT_TICKET_BACKEND_RELATIVE_PATH)
        );
        assert_eq!(config.ticket_record_language(), None);
        assert_eq!(config.orchestration.branch_name(), None);
        assert_eq!(
            config.orchestration.effective_branch_name(),
            "orchestration"
        );
        assert_eq!(config.orchestration.worktree_dir(), Path::new(".worktree"));
        assert_eq!(
            config.orchestration.worktree_name(),
            Path::new("orchestration")
        );
        for role in TicketRole::ALL {
            let role_config = config.role(role);
            assert_eq!(role_config.profile.as_str(), "inherit");
            assert!(role_config.launch_prompt.is_none());
            assert_eq!(role_config.workflow.as_str(), role.default_workflow());
        }
    }

    #[test]
    fn workspace_settings_take_precedence_over_legacy_ticket_config() {
        let temp = TempDir::new().unwrap();
        write_workspace_settings(
            temp.path(),
            r#"
[ticket]
language = "Japanese"

[ticket.backend]
provider = "builtin:yoi_local"
root = "workspace-tickets"
"#,
        );
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = "legacy-tickets"

[ticket]
language = "English"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend.root, temp.path().join("workspace-tickets"));
        assert_eq!(config.ticket_record_language(), Some("Japanese"));
    }

    #[test]
    fn legacy_ticket_config_is_read_only_migration_fallback() {
        let temp = TempDir::new().unwrap();
        write_workspace_settings(
            temp.path(),
            r#"
workspace_id = "00000000-0000-7000-8000-000000000000"
display_name = "legacy-fallback"
"#,
        );
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = "legacy-tickets"

[ticket]
language = "Japanese"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend.root, temp.path().join("legacy-tickets"));
        assert_eq!(config.ticket_record_language(), Some("Japanese"));
    }

    #[test]
    fn empty_workspace_ticket_table_uses_workspace_defaults_and_ignores_legacy() {
        let temp = TempDir::new().unwrap();
        write_workspace_settings(
            temp.path(),
            r#"
[ticket]
"#,
        );
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = "legacy-tickets"

[ticket]
language = "Japanese"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend.root, temp.path().join(".yoi/tickets"));
        assert_eq!(config.ticket_record_language(), None);
    }

    #[test]
    fn full_config_parses_fixed_role_refs() {
        let temp = TempDir::new().unwrap();
        write_workspace_settings(
            temp.path(),
            r#"
[ticket]
language = "Japanese"

[ticket.backend]
provider = "builtin:yoi_local"
root = "custom-tickets"

[ticket.orchestration]
branch = "orchestration/custom-panel"
worktree_dir = "custom-worktrees"
worktree_name = "custom-orchestrator"

[ticket.roles.intake]
profile = "project:intake"
launch_prompt = "$workspace/ticket/intake/launch"
workflow = "ticket-intake-workflow"

[ticket.roles.orchestrator]
profile = "project:orchestrator"
launch_prompt = "$workspace/ticket/orchestrator/launch"
workflow = "ticket-orchestrator-routing"

[ticket.roles.coder]
profile = "inherit"
launch_prompt = "$workspace/ticket/coder/launch"
workflow = "multi-agent-workflow"

[ticket.roles.reviewer]
profile = "project:reviewer"
launch_prompt = "$workspace/ticket/reviewer/launch"
workflow = "multi-agent-workflow"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(config.backend.root, temp.path().join("custom-tickets"));
        assert_eq!(config.ticket_record_language(), Some("Japanese"));
        assert_eq!(
            config.orchestration.branch_name(),
            Some("orchestration/custom-panel")
        );
        assert_eq!(
            config.orchestration.effective_branch_name(),
            "orchestration/custom-panel"
        );
        assert_eq!(
            config.orchestration.worktree_dir(),
            Path::new("custom-worktrees")
        );
        assert_eq!(
            config.orchestration.worktree_name(),
            Path::new("custom-orchestrator")
        );
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
            config.workflow_for(TicketRole::Reviewer).as_str(),
            "multi-agent-workflow"
        );
    }

    #[test]
    fn scaffold_config_includes_backend_and_all_fixed_roles() {
        let temp = TempDir::new().unwrap();
        let scaffold = ticket_config_scaffold();

        assert!(scaffold.contains("[ticket]\n"));
        assert!(scaffold.contains("[ticket.backend]\n"));
        assert!(scaffold.contains("provider = \"builtin:yoi_local\""));
        assert!(scaffold.contains("root = \".yoi/tickets\""));
        assert!(scaffold.contains("# language = \"Japanese\""));
        assert!(scaffold.contains(
            "# [ticket.orchestration]\n# branch = \"orchestration\"\n# worktree_dir = \".worktree\"\n# worktree_name = \"orchestration\""
        ));
        for role in TicketRole::ALL {
            assert!(scaffold.contains(&format!("[ticket.roles.{role}]")));
            assert!(scaffold.contains(&format!(
                "[ticket.roles.{role}]\nprofile = \"{}\"\nworkflow = \"{}\"",
                role.default_profile(),
                role.default_workflow()
            )));
        }
        assert!(!scaffold.contains("[ticket.roles.investigator]"));

        let config = TicketConfig::from_workspace_settings_toml(
            temp.path(),
            temp.path().join(WORKSPACE_SETTINGS_RELATIVE_PATH),
            &scaffold,
        )
        .unwrap()
        .unwrap();
        assert_eq!(config.backend_root(), temp.path().join(".yoi/tickets"));
        assert_eq!(config.orchestration.branch_name(), None);
        assert_eq!(
            config.orchestration.effective_branch_name(),
            "orchestration"
        );
        assert_eq!(config.orchestration.worktree_dir(), Path::new(".worktree"));
        assert_eq!(
            config.orchestration.worktree_name(),
            Path::new("orchestration")
        );
        for role in TicketRole::ALL {
            let role_config = config.role_launch_config(role).unwrap();
            assert_eq!(role_config.profile.as_str(), role.default_profile());
            assert_eq!(role_config.workflow.as_str(), role.default_workflow());
        }
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
    fn backend_only_config_is_not_role_launch_ready() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend.root, temp.path().join(".yoi/tickets"));
        assert_eq!(
            config.role_launch_config(TicketRole::Intake).unwrap_err(),
            TicketRoleLaunchConfigError::MissingRoleTable {
                role: TicketRole::Intake
            }
        );
    }

    #[test]
    fn partial_role_config_only_marks_configured_roles_launch_ready() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.intake]
profile = "builtin:default"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config
                .role_launch_config(TicketRole::Intake)
                .unwrap()
                .profile
                .as_str(),
            "builtin:default"
        );
        assert_eq!(
            config
                .role_launch_config(TicketRole::Orchestrator)
                .unwrap_err(),
            TicketRoleLaunchConfigError::MissingRoleTable {
                role: TicketRole::Orchestrator
            }
        );
    }

    #[test]
    fn orchestration_branch_config_is_validated_as_git_branch_name() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[orchestration]
branch = "orchestration/panel:bad"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(error.to_string().contains("git branch name"));
        assert!(error.to_string().contains("unsupported character"));
    }

    #[test]
    fn orchestration_branch_rejects_full_refs_and_dash_prefixes() {
        assert!(GitBranchName::new("refs/heads/orchestration/panel").is_err());
        assert!(GitBranchName::new("-orchestration-panel").is_err());
        assert_eq!(
            GitBranchName::new("orchestration/panel").unwrap().as_str(),
            "orchestration/panel"
        );
    }

    #[test]
    fn role_table_without_profile_is_not_role_launch_ready() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.orchestrator]
workflow = "ticket-orchestrator-routing"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config
                .role_launch_config(TicketRole::Orchestrator)
                .unwrap_err(),
            TicketRoleLaunchConfigError::MissingProfile {
                role: TicketRole::Orchestrator
            }
        );
    }

    #[test]
    fn inherit_profile_is_not_role_launch_ready() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.intake]
profile = "inherit"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config.role_launch_config(TicketRole::Intake).unwrap_err(),
            TicketRoleLaunchConfigError::InheritProfile {
                role: TicketRole::Intake
            }
        );
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
        assert!(
            error
                .to_string()
                .contains("unsupported Ticket role `operator`")
        );
        assert!(
            error
                .to_string()
                .contains("intake, orchestrator, coder, reviewer")
        );
    }

    #[test]
    fn stale_investigator_role_config_is_rejected() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[roles.investigator]
profile = "builtin:default"
workflow = "ticket-orchestrator-routing"
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unsupported Ticket role `investigator`")
        );
        assert!(
            error
                .to_string()
                .contains("intake, orchestrator, coder, reviewer")
        );
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
root = "legacy-tickets"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(
            config.backend.provider,
            TicketBackendProvider::BuiltinYoiLocal
        );
        assert_eq!(config.backend_root(), temp.path().join("legacy-tickets"));
    }

    #[test]
    fn rejects_empty_ticket_record_language() {
        let temp = TempDir::new().unwrap();
        write_config(
            temp.path(),
            r#"
[backend]
provider = "builtin:yoi_local"
root = ".yoi/tickets"

[ticket]
language = "   "
"#,
        );

        let error = TicketConfig::load_workspace(temp.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ticket record language must not be empty")
        );
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
root = "nested/tickets"
"#,
        );

        let config = TicketConfig::load_workspace(temp.path()).unwrap();
        assert_eq!(config.backend_root(), temp.path().join("nested/tickets"));
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
profile = "./coder.toml"
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
