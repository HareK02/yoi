//! Configuration-owned causes and disclosure, not adoption outcomes or HTTP policy.
//!
//! Original tree errors stay local. Only closed operation/reason values and local
//! explanations may cross the shared codec. All audiences need current visibility;
//! received text, paths, imports, source content and arbitrary context are not copied.
//! Source/preview/adopt/reconcile results, request identity, receipts and recovery
//! remain owned by the existing configuration adoption boundary.

use config_source::ConfigTreeError;
use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationFailureOperation {
    Source,
    Preview,
    Adopt,
    Reconcile,
}

impl ConfigurationFailureOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Preview => "preview",
            Self::Adopt => "adopt",
            Self::Reconcile => "reconcile",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Source => "source retrieval",
            Self::Preview => "change preview",
            Self::Adopt => "source and policy adoption",
            Self::Reconcile => "adoption result reconciliation",
        }
    }
}

/// Observed causes only. These do not grant retries or establish an adoption result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConfigurationFailureReason {
    InvalidPath,
    InvalidImport,
    ImportEscape,
    AlreadyExists,
    NotFound,
    EntryConflict,
    EntryDigestMismatch,
    EntryPathMismatch,
    TreeDigestMismatch,
    PathChangedMoreThanOnce,
    DuplicatePath,
    DuplicateSchemaProvider,
    DuplicateSchemaNamespace,
    InvalidSchemaContribution,
    LimitExceeded,
    CatalogChanged,
    EditingHeadChanged,
    LegacyExportChanged,
    SourceContentChanged,
    OwnerRequired,
    WorkspaceInactive,
    WorkspaceDraining,
    RequestIdentityConflict,
}

impl ConfigurationFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidPath => "invalid_path",
            Self::InvalidImport => "invalid_import",
            Self::ImportEscape => "import_escape",
            Self::AlreadyExists => "already_exists",
            Self::NotFound => "not_found",
            Self::EntryConflict => "entry_conflict",
            Self::EntryDigestMismatch => "entry_digest_mismatch",
            Self::EntryPathMismatch => "entry_path_mismatch",
            Self::TreeDigestMismatch => "tree_digest_mismatch",
            Self::PathChangedMoreThanOnce => "path_changed_more_than_once",
            Self::DuplicatePath => "duplicate_path",
            Self::DuplicateSchemaProvider => "duplicate_schema_provider",
            Self::DuplicateSchemaNamespace => "duplicate_schema_namespace",
            Self::InvalidSchemaContribution => "invalid_schema_contribution",
            Self::LimitExceeded => "limit_exceeded",
            Self::CatalogChanged => "catalog_changed",
            Self::EditingHeadChanged => "editing_head_changed",
            Self::LegacyExportChanged => "legacy_export_changed",
            Self::SourceContentChanged => "source_content_changed",
            Self::OwnerRequired => "owner_required",
            Self::WorkspaceInactive => "workspace_inactive",
            Self::WorkspaceDraining => "workspace_draining",
            Self::RequestIdentityConflict => "request_identity_conflict",
        }
    }

    fn category(self) -> Category {
        match self {
            Self::OwnerRequired => Category::Authorization,
            Self::EntryConflict
            | Self::CatalogChanged
            | Self::EditingHeadChanged
            | Self::LegacyExportChanged
            | Self::SourceContentChanged
            | Self::WorkspaceInactive
            | Self::WorkspaceDraining
            | Self::RequestIdentityConflict => Category::Conflict,
            Self::DuplicateSchemaProvider
            | Self::DuplicateSchemaNamespace
            | Self::InvalidSchemaContribution => Category::Internal,
            Self::InvalidPath
            | Self::InvalidImport
            | Self::ImportEscape
            | Self::AlreadyExists
            | Self::NotFound
            | Self::EntryDigestMismatch
            | Self::EntryPathMismatch
            | Self::TreeDigestMismatch
            | Self::PathChangedMoreThanOnce
            | Self::DuplicatePath
            | Self::LimitExceeded => Category::InvalidInput,
        }
    }

    fn explanation(self) -> &'static str {
        match self {
            Self::InvalidPath => "a virtual configuration path is invalid",
            Self::InvalidImport => "a virtual configuration import is invalid",
            Self::ImportEscape => "a configuration import escapes the virtual source tree",
            Self::AlreadyExists => "a configuration entry already exists at the requested path",
            Self::NotFound => "a requested configuration entry was not found",
            Self::EntryConflict => "a configuration entry changed since its content was observed",
            Self::EntryDigestMismatch => "a configuration entry does not match its content digest",
            Self::EntryPathMismatch => "a configuration entry path does not match its tree key",
            Self::TreeDigestMismatch => {
                "the configuration source tree does not match its content digest"
            }
            Self::PathChangedMoreThanOnce => {
                "a configuration path is changed more than once in one candidate"
            }
            Self::DuplicatePath => "the configuration source tree contains a duplicate path",
            Self::DuplicateSchemaProvider => {
                "a configuration schema provider is registered more than once"
            }
            Self::DuplicateSchemaNamespace => {
                "a configuration schema namespace has more than one owner"
            }
            Self::InvalidSchemaContribution => {
                "a configuration schema provider supplied an invalid contribution"
            }
            Self::LimitExceeded => {
                "the configuration source exceeds a supported size or count limit"
            }
            Self::CatalogChanged => "the permission catalog changed since it was observed",
            Self::EditingHeadChanged => {
                "the configuration editing history head changed since it was observed"
            }
            Self::LegacyExportChanged => {
                "the legacy configuration migration source or permission export no longer matches the observed baseline"
            }
            Self::SourceContentChanged => {
                "the configuration source content changed since it was observed"
            }
            Self::OwnerRequired => "configuration management requires the current Workspace owner",
            Self::WorkspaceInactive => "the Workspace is not active for configuration management",
            Self::WorkspaceDraining => "Workspace deletion is draining configuration requests",
            Self::RequestIdentityConflict => {
                "the configuration operation identity is already bound to a different request"
            }
        }
    }
}

impl From<&ConfigTreeError> for ConfigurationFailureReason {
    fn from(source: &ConfigTreeError) -> Self {
        // Match variants explicitly; never parse Display or publish their payloads.
        match source {
            ConfigTreeError::InvalidPath(_) => Self::InvalidPath,
            ConfigTreeError::InvalidImport(_) => Self::InvalidImport,
            ConfigTreeError::ImportEscape(_) => Self::ImportEscape,
            ConfigTreeError::AlreadyExists(_) => Self::AlreadyExists,
            ConfigTreeError::NotFound(_) => Self::NotFound,
            ConfigTreeError::EntryConflict(_) => Self::EntryConflict,
            ConfigTreeError::EntryDigestMismatch(_) => Self::EntryDigestMismatch,
            ConfigTreeError::EntryPathMismatch(_) => Self::EntryPathMismatch,
            ConfigTreeError::TreeDigestMismatch => Self::TreeDigestMismatch,
            ConfigTreeError::PathChangedMoreThanOnce(_) => Self::PathChangedMoreThanOnce,
            ConfigTreeError::DuplicatePath => Self::DuplicatePath,
            ConfigTreeError::DuplicateSchemaProvider(_) => Self::DuplicateSchemaProvider,
            ConfigTreeError::DuplicateSchemaNamespace(_) => Self::DuplicateSchemaNamespace,
            ConfigTreeError::InvalidSchemaContribution(_) => Self::InvalidSchemaContribution,
            ConfigTreeError::LimitExceeded(_) => Self::LimitExceeded,
        }
    }
}

/// The complete wire allowlist. No path, import, identifier or diagnostic is a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigurationFailureDetails {
    pub operation: ConfigurationFailureOperation,
    pub reason: ConfigurationFailureReason,
}

impl ConfigurationFailureDetails {
    fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let impact = format!(
            "Configuration {} could not complete",
            self.operation.description()
        );
        let mut disclosure = Disclosure::new(
            *self,
            compose_explanation(self.reason.explanation(), &impact)?,
        );
        disclosure.context.operations.push(OperationContext::new(
            self.operation.as_str(),
            "configuration",
        )?);
        Ok(disclosure)
    }
}

impl FailureCause for ConfigurationFailureDetails {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "configuration",
            reason: self.reason.as_str(),
            category: self.reason.category(),
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl WireDetails for ConfigurationFailureDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "configuration"
            && details_version == 1
            && serde_json::from_value::<ConfigurationFailureReason>(serde_json::Value::String(
                reason.to_owned(),
            ))
            .is_ok()
    }
}

/// Local, nonserializable owner of the original typed tree error, when present.
/// Display is also safe while callers still expose it through legacy API errors.
#[derive(Debug, thiserror::Error)]
#[error("Configuration {} could not complete: {}", .details.operation.description(), .details.reason.explanation())]
pub struct ConfigurationFailure {
    details: ConfigurationFailureDetails,
    #[source]
    source: Option<ConfigTreeError>,
}

impl ConfigurationFailure {
    /// Assign a known reason at the branch deciding it, not from an error string.
    pub fn new(
        operation: ConfigurationFailureOperation,
        reason: ConfigurationFailureReason,
    ) -> Self {
        Self {
            details: ConfigurationFailureDetails { operation, reason },
            source: None,
        }
    }

    pub fn from_tree(operation: ConfigurationFailureOperation, source: ConfigTreeError) -> Self {
        Self {
            details: ConfigurationFailureDetails {
                operation,
                reason: ConfigurationFailureReason::from(&source),
            },
            source: Some(source),
        }
    }

    pub(crate) fn with_operation(mut self, operation: ConfigurationFailureOperation) -> Self {
        self.details.operation = operation;
        self
    }

    pub fn details(&self) -> &ConfigurationFailureDetails {
        &self.details
    }

    pub fn source_error(&self) -> Option<&ConfigTreeError> {
        self.source.as_ref()
    }
}

// Callers may consume the original failure in FailureFacts::new, or copy its safe
// details into facts while retaining the source in their existing error/result.
impl FailureCause for ConfigurationFailure {
    fn identity(&self) -> FailureIdentity {
        self.details.identity()
    }

    fn validate(&self) -> Result<(), ContractError> {
        self.details.validate()
    }
}

/// A current authenticated boundary decision, not a serialized capability.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigurationFailureVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for ConfigurationFailureDetails {
            type Authorization = ConfigurationFailureVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &ConfigurationFailureVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.disclosure()
            }
        }
        impl ProjectFor<$audience> for ConfigurationFailure {
            type Authorization = ConfigurationFailureVisibility;
            type Details = ConfigurationFailureDetails;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &ConfigurationFailureVisibility,
            ) -> Result<Disclosure<Self::Details>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.details.disclosure()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);

#[cfg(test)]
mod tests;
