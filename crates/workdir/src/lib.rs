//! Persistent Workdir identity and Worker-bound operation sessions.
//!
//! A [`Workdir`] identifies a materialized repository execution context across
//! Worker lifetimes. A [`WorkdirSession`] is the live operation attachment
//! bound to one Worker. Tools consume sessions; they do not own Workdir
//! materialization or cleanup.

pub mod checkout;
pub use checkout::{
    CheckoutObservation, CheckoutOperation, CheckoutOutput, CheckoutRequest, CheckoutResult,
    CheckoutSearchOperation, CheckoutSearchRequest, CheckoutSearchResult,
};
pub mod external;
pub mod http;
mod local;
mod operation;
mod router;
mod scope;
pub mod workspace;

#[cfg(test)]
mod checkout_search_tests;
#[cfg(test)]
mod external_local_tests;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

pub use fs_operation::{
    BoundedReadLimits, ContentHash, EditRequest, EditResult, EntryKind, FsPath as WorkdirPath,
    GlobRequest, GlobResult, GrepOutputMode, GrepRequest, GrepResult, ListCursor, ListEntry,
    ListRequest, ListResult, ReadBytesRequest, ReadBytesResult, ReadRequest, ReadResult,
    StatRequest, StatResult, WriteRequest, WriteResult,
};
pub use http::dispatch_workdir_session_operation;
pub use local::{
    ExternalWorkdirRoot, LocalWorkdirSession, SymlinkInfo, WorkdirSessionResource, direct_symlink,
    first_symlink,
};
pub use operation::*;
pub use router::{
    ResolvedWorkdirSession, RoutedWorkdirSession, WorkdirAttachmentAlias, WorkdirRouteError,
    WorkdirRouteErrorCode, WorkdirSessionRouter,
};
pub use scope::{
    ReadOnlyWorkdirSession, WorkdirScopeAuthorizationRequest, WorkdirScopeLease,
    WorkdirScopeLeaseSet, WorkdirScopeOverlapRequest, WorkdirToolBroker, WorkdirToolBrokerRouter,
    WorkdirToolScope, WorkdirToolScopePermission, WorkdirToolScopeRule,
};

/// Persistent, opaque identity of one materialized Workdir.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Workdir {
    id: WorkdirId,
}

impl Workdir {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: WorkdirId(id.into()),
        }
    }

    pub fn id(&self) -> &WorkdirId {
        &self.id
    }
}

/// Opaque Workdir identifier assigned by the materialization authority.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct WorkdirId(String);

impl WorkdirId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WorkdirId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirSessionCapability {
    Read,
    Write,
    Edit,
    Glob,
    Grep,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkdirSessionCapabilities {
    bits: u8,
}

impl WorkdirSessionCapabilities {
    const READ: u8 = 1 << 0;
    const WRITE: u8 = 1 << 1;
    const EDIT: u8 = 1 << 2;
    const GLOB: u8 = 1 << 3;
    const GREP: u8 = 1 << 4;
    const COMMAND: u8 = 1 << 5;

    pub const EMPTY: Self = Self { bits: 0 };

    pub fn from_capabilities(
        capabilities: impl IntoIterator<Item = WorkdirSessionCapability>,
    ) -> Self {
        capabilities
            .into_iter()
            .fold(Self::EMPTY, |set, capability| set.with(capability))
    }

    pub const fn with(mut self, capability: WorkdirSessionCapability) -> Self {
        self.bits |= match capability {
            WorkdirSessionCapability::Read => Self::READ,
            WorkdirSessionCapability::Write => Self::WRITE,
            WorkdirSessionCapability::Edit => Self::EDIT,
            WorkdirSessionCapability::Glob => Self::GLOB,
            WorkdirSessionCapability::Grep => Self::GREP,
            WorkdirSessionCapability::Command => Self::COMMAND,
        };
        self
    }

    pub const ALL: Self = Self {
        bits: Self::READ | Self::WRITE | Self::EDIT | Self::GLOB | Self::GREP | Self::COMMAND,
    };

    pub const READ_ONLY: Self = Self {
        bits: Self::READ | Self::GLOB | Self::GREP,
    };

    /// Operator-facing WRITE category without implicitly granting READ or COMMAND.
    pub const WRITE_ONLY: Self = Self {
        bits: Self::WRITE | Self::EDIT,
    };

    /// Operator-facing COMMAND category without implicitly granting file access.
    pub const COMMAND_ONLY: Self = Self {
        bits: Self::COMMAND,
    };

    /// Filesystem read/write authority without process execution.
    ///
    /// External Workdirs use this capability set so operator-approved file
    /// mutation never implicitly grants command or shell authority.
    pub const READ_WRITE: Self = Self {
        bits: Self::READ | Self::WRITE | Self::EDIT | Self::GLOB | Self::GREP,
    };

    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
        }
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self {
            bits: self.bits & other.bits,
        }
    }

    pub const fn supports(self, capability: WorkdirSessionCapability) -> bool {
        let bit = match capability {
            WorkdirSessionCapability::Read => Self::READ,
            WorkdirSessionCapability::Write => Self::WRITE,
            WorkdirSessionCapability::Edit => Self::EDIT,
            WorkdirSessionCapability::Glob => Self::GLOB,
            WorkdirSessionCapability::Grep => Self::GREP,
            WorkdirSessionCapability::Command => Self::COMMAND,
        };
        self.bits & bit != 0
    }
}

pub type WriteOutcome = WriteResult;

/// Live, Worker-bound operations for one persistent [`Workdir`].
///
/// Implementations execute filesystem search and command work on the host
/// that owns the materialization. Structured requests and results never
/// contain the raw materialized root. Closing a session is terminal and does
/// not delete the persistent Workdir or its materialization.
#[async_trait]
pub trait WorkdirSession: std::fmt::Debug + Send + Sync {
    fn workdir(&self) -> &Workdir;
    fn capabilities(&self) -> WorkdirSessionCapabilities;

    /// Validate an attenuated filesystem rule at the provider boundary without
    /// exposing the resolved host path. Providers that cannot resolve symbolic
    /// links must reject resolved-policy checks rather than downgrade them.
    async fn authorize_scope_path(
        &self,
        request: WorkdirScopeAuthorizationRequest,
    ) -> Result<(), WorkdirError> {
        if request.rules.iter().any(|rule| {
            rule.symlink_policy == manifest::SymlinkPolicy::Logical
                && scope::rule_allows_path(rule, &request.path, request.permission)
        }) {
            Ok(())
        } else {
            Err(WorkdirError::denied(
                WorkdirDenialReason::ScopeResolutionUnavailable,
                "Workdir provider cannot establish resolved scope authority".to_string(),
            ))
        }
    }

    async fn scope_rules_overlap(
        &self,
        _request: WorkdirScopeOverlapRequest,
    ) -> Result<bool, WorkdirError> {
        Err(WorkdirError::denied(
            WorkdirDenialReason::ScopeComparisonUnavailable,
            "Workdir provider cannot compare resolved scope authority".to_string(),
        ))
    }

    async fn checkout_search(
        &self,
        _request: CheckoutSearchRequest,
    ) -> Result<CheckoutSearchResult, WorkdirError> {
        Err(WorkdirError::UnsupportedOperation(
            "scoped checkout traversal".into(),
        ))
    }

    async fn checkout_observe(
        &self,
        _path: WorkdirPath,
    ) -> Result<CheckoutObservation, WorkdirError> {
        Err(WorkdirError::UnsupportedOperation(
            "checkout observation".into(),
        ))
    }
    async fn checkout_execute(
        &self,
        _request: CheckoutRequest,
    ) -> Result<CheckoutResult, WorkdirError> {
        Err(WorkdirError::UnsupportedOperation(
            "checked checkout execution".into(),
        ))
    }
    async fn stat(&self, request: StatRequest) -> Result<StatResult, WorkdirError>;
    async fn read(&self, request: ReadRequest) -> Result<ReadResult, WorkdirError>;
    async fn read_bytes(
        &self,
        _request: ReadBytesRequest,
    ) -> Result<ReadBytesResult, WorkdirError> {
        Err(WorkdirError::UnsupportedOperation(
            "bounded binary reads are not supported by this provider".to_string(),
        ))
    }
    async fn write(&self, request: WriteRequest) -> Result<WriteResult, WorkdirError>;
    async fn edit(&self, request: EditRequest) -> Result<EditResult, WorkdirError>;
    async fn list(&self, request: ListRequest) -> Result<ListResult, WorkdirError>;
    async fn glob(&self, request: GlobRequest) -> Result<GlobResult, WorkdirError>;
    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, WorkdirError>;
    async fn start_command(&self, request: CommandRequest) -> Result<CommandHandle, WorkdirError>;
    async fn command_status(&self, handle: CommandHandle) -> Result<CommandStatus, WorkdirError>;
    async fn command_output(
        &self,
        request: CommandOutputRequest,
    ) -> Result<CommandOutput, WorkdirError>;
    async fn cancel_command(&self, handle: CommandHandle) -> Result<(), WorkdirError>;

    /// Subscribe to bounded provider-owned command telemetry. Implementations
    /// that do not expose live command observation may keep the default.
    fn subscribe_command_events(&self) -> Option<broadcast::Receiver<CommandEvent>> {
        None
    }

    /// Return the bounded current command state used to recover from a lagged
    /// provider subscription without replaying command output into history.
    fn command_snapshot(&self) -> Vec<CommandSnapshot> {
        Vec::new()
    }

    /// Terminal, idempotent release of this Worker-bound operation session.
    async fn close(&self) -> Result<(), WorkdirError>;
}

pub type WorkdirSessionHandle = Arc<dyn WorkdirSession>;

/// Closed diagnostic vocabulary: no request text, paths or secrets.
/// A reason describes a refusing branch, not an authorization grant.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirDenialReason {
    ScopeResolutionUnavailable,
    ScopeComparisonUnavailable,
    ScopedCapabilityDenied,
    LogicalScopeExceeded,
    InvalidScopedPath,
    ChildWriteLeaseConflict,
    EmptyScope,
    ParentCapabilityDenied,
    CommandRequiresWritableScope,
    ParentScopeExceeded,
    CwdOutsideReadableScope,
    ReadOnlySession,
    ExternalRootSymlink,
    ExternalRootChanged,
    ExternalRootMoved,
    ExternalScopeSymlink,
    AttachmentScopeExceeded,
    DelegatedScopeExceeded,
    CheckoutOutputRootExceeded,
    ProviderRootExceeded,
    ProviderSymlinkDenied,
    CheckoutTargetDenied,
    CheckoutCreateParentDenied,
    CheckoutSearchPathDenied,
    CheckoutEnumerationDenied,
    PathOutOfScope,
    SymlinkTargetOutOfScope,
    PathReadOnly,
    OsPermissionDenied,
    /// PermissionDenied without a typed provider origin or an OS error code.
    /// In particular, do not mislabel lower-layer synthetic refusals as OS errors.
    UnclassifiedPermissionDenied,
}

impl WorkdirDenialReason {
    /// Stable allowlisted label for internal diagnostics, matching the wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ScopeResolutionUnavailable => "scope_resolution_unavailable",
            Self::ScopeComparisonUnavailable => "scope_comparison_unavailable",
            Self::ScopedCapabilityDenied => "scoped_capability_denied",
            Self::LogicalScopeExceeded => "logical_scope_exceeded",
            Self::InvalidScopedPath => "invalid_scoped_path",
            Self::ChildWriteLeaseConflict => "child_write_lease_conflict",
            Self::EmptyScope => "empty_scope",
            Self::ParentCapabilityDenied => "parent_capability_denied",
            Self::CommandRequiresWritableScope => "command_requires_writable_scope",
            Self::ParentScopeExceeded => "parent_scope_exceeded",
            Self::CwdOutsideReadableScope => "cwd_outside_readable_scope",
            Self::ReadOnlySession => "read_only_session",
            Self::ExternalRootSymlink => "external_root_symlink",
            Self::ExternalRootChanged => "external_root_changed",
            Self::ExternalRootMoved => "external_root_moved",
            Self::ExternalScopeSymlink => "external_scope_symlink",
            Self::AttachmentScopeExceeded => "attachment_scope_exceeded",
            Self::DelegatedScopeExceeded => "delegated_scope_exceeded",
            Self::CheckoutOutputRootExceeded => "checkout_output_root_exceeded",
            Self::ProviderRootExceeded => "provider_root_exceeded",
            Self::ProviderSymlinkDenied => "provider_symlink_denied",
            Self::CheckoutTargetDenied => "checkout_target_denied",
            Self::CheckoutCreateParentDenied => "checkout_create_parent_denied",
            Self::CheckoutSearchPathDenied => "checkout_search_path_denied",
            Self::CheckoutEnumerationDenied => "checkout_enumeration_denied",
            Self::PathOutOfScope => "path_out_of_scope",
            Self::SymlinkTargetOutOfScope => "symlink_target_out_of_scope",
            Self::PathReadOnly => "path_read_only",
            Self::OsPermissionDenied => "os_permission_denied",
            Self::UnclassifiedPermissionDenied => "unclassified_permission_denied",
        }
    }
}

/// Original local messages remain available, but are not diagnostic wire data.
/// Missing reasons from older providers remain explicitly unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkdirDenial {
    pub reason: Option<WorkdirDenialReason>,
    pub message: String,
}

impl std::fmt::Display for WorkdirDenial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl From<String> for WorkdirDenial {
    fn from(message: String) -> Self {
        Self {
            reason: None,
            message,
        }
    }
}
impl From<&str> for WorkdirDenial {
    fn from(message: &str) -> Self {
        message.to_string().into()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WorkdirError {
    #[error("Workdir operation denied: {0}")]
    Denied(WorkdirDenial),

    /// Retain the source's existing classification and display while attaching
    /// a reason (including checkout's historical PermissionDenied mapping).
    #[error("{source}")]
    DenialContext {
        reason: WorkdirDenialReason,
        #[source]
        source: Box<WorkdirError>,
    },

    #[error("Workdir session is closed")]
    SessionClosed,

    #[error("Workdir session does not support {0:?}")]
    Unsupported(WorkdirSessionCapability),

    #[error("Workdir operation is unsupported: {0}")]
    UnsupportedOperation(String),

    #[error("invalid Workdir path: {0}")]
    InvalidPath(String),

    #[error("Workdir session is unavailable: {0}")]
    Unavailable(String),

    #[error("Workdir operation failed")]
    OperationFailed,

    #[error("Workdir transport failed: {0}")]
    Transport(String),

    #[error("{0}")]
    Conflict(String),

    /// Effects may have happened. Do not automatically retry the operation.
    #[error("Workdir operation outcome is unknown: {0}")]
    OutcomeUnknown(String),

    #[error("unknown Workdir session command: {0}")]
    UnknownCommand(String),

    #[error("path must be absolute: {}", .0.display())]
    RelativePath(PathBuf),

    #[error("path is outside allowed scope: {}", .0.display())]
    OutOfScope(PathBuf),

    #[error(
        "path resolves through a symlink outside allowed {required_permission} scope: {} -> {}; add the symlink target to the Worker {required_permission} scope, copy it into the workspace, or recreate the symlink with the correct target",
        .path.display(),
        .target.display()
    )]
    SymlinkOutOfScope {
        path: PathBuf,
        target: PathBuf,
        required_permission: &'static str,
    },

    #[error(
        "broken symlink while resolving {}: {} -> {} (target does not exist); recreate the symlink with an absolute target or a correct relative target",
        .path.display(),
        .link.display(),
        .target.display()
    )]
    BrokenSymlink {
        path: PathBuf,
        link: PathBuf,
        target: PathBuf,
    },

    #[error(
        "path resolves through a symlink to a directory, but this tool requires a file: {} -> {}; choose a file inside that directory",
        .path.display(),
        .target.display()
    )]
    SymlinkTargetIsDirectory { path: PathBuf, target: PathBuf },

    #[error("path is read-only: {}", .0.display())]
    ReadOnly(PathBuf),

    #[error("expected file but path is a directory: {}", .0.display())]
    IsDirectory(PathBuf),

    #[error("file not found: {}", .0.display())]
    NotFound(PathBuf),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("invalid glob pattern: {0}")]
    InvalidGlob(String),

    #[error("invalid regex pattern: {0}")]
    InvalidRegex(String),

    #[error("{tool} does not follow symlink directories: {} -> {}", .path.display(), .target.display())]
    SymlinkDirectoryNotTraversed {
        tool: &'static str,
        path: PathBuf,
        target: PathBuf,
    },

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl WorkdirError {
    /// The error used for operational classification, ignoring only diagnostic
    /// context (including nested context). Keep the original error for display
    /// and `denial_reason`: enrichment must not alter behavior or retry policy.
    pub fn classification_source(&self) -> &Self {
        let mut error = self;
        while let Self::DenialContext { source, .. } = error {
            error = source;
        }
        error
    }

    pub fn denied(reason: WorkdirDenialReason, message: impl Into<String>) -> Self {
        Self::Denied(WorkdirDenial {
            reason: Some(reason),
            message: message.into(),
        })
    }

    /// Safe diagnostic data only; never infer a reason from arbitrary messages.
    pub fn denial_reason(&self) -> Option<WorkdirDenialReason> {
        use WorkdirDenialReason as Reason;
        match self {
            Self::Denied(denial) => denial.reason,
            Self::DenialContext { reason, .. } => Some(*reason),
            Self::OutOfScope(_) => Some(Reason::PathOutOfScope),
            Self::SymlinkOutOfScope { .. } => Some(Reason::SymlinkTargetOutOfScope),
            Self::ReadOnly(_) => Some(Reason::PathReadOnly),
            Self::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
                Some(local::io_denial_reason(source))
            }
            _ => None,
        }
    }

    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

impl From<fs_operation::FsError> for WorkdirError {
    fn from(error: fs_operation::FsError) -> Self {
        match error {
            fs_operation::FsError::InvalidPath(message) => Self::InvalidPath(message),
            fs_operation::FsError::RelativePath(path) => Self::RelativePath(path),
            fs_operation::FsError::OutOfScope(path) => Self::OutOfScope(path),
            fs_operation::FsError::NotFound(path) => Self::NotFound(path),
            fs_operation::FsError::BrokenSymlink { path, link, target } => {
                Self::BrokenSymlink { path, link, target }
            }
            fs_operation::FsError::SymlinkOutOfScope {
                path,
                target,
                required_permission,
            } => Self::SymlinkOutOfScope {
                path,
                target,
                required_permission,
            },
            fs_operation::FsError::SymlinkDirectoryNotTraversed { tool, path, target } => {
                Self::SymlinkDirectoryNotTraversed { tool, path, target }
            }
            fs_operation::FsError::ReadOnly(path) => Self::ReadOnly(path),
            fs_operation::FsError::IsDirectory(path) => Self::IsDirectory(path),
            fs_operation::FsError::NotDirectory(path) => {
                Self::InvalidArgument(format!("path is not a directory: {}", path.display()))
            }
            fs_operation::FsError::SymlinkTargetIsDirectory { path, target } => {
                Self::SymlinkTargetIsDirectory { path, target }
            }
            fs_operation::FsError::Conflict(path) => Self::Conflict(format!(
                "The target file's content or existence changed since it was last observed; read the file again before retrying: {path}"
            )),
            fs_operation::FsError::InvalidGlob(message) => Self::InvalidGlob(message),
            fs_operation::FsError::InvalidRegex(message) => Self::InvalidRegex(message),
            fs_operation::FsError::InvalidArgument(message) => Self::InvalidArgument(message),
            fs_operation::FsError::Io { path, source } => Self::Io { path, source },
        }
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;

    fn bits(capabilities: WorkdirSessionCapabilities) -> u8 {
        serde_json::to_value(capabilities).unwrap()["bits"]
            .as_u64()
            .unwrap() as u8
    }

    #[test]
    fn external_categories_preserve_operation_bit_values() {
        assert_eq!(bits(WorkdirSessionCapabilities::READ_ONLY), 25);
        assert_eq!(bits(WorkdirSessionCapabilities::WRITE_ONLY), 6);
        assert_eq!(bits(WorkdirSessionCapabilities::READ_WRITE), 31);
        assert_eq!(bits(WorkdirSessionCapabilities::COMMAND_ONLY), 32);
        assert_eq!(bits(WorkdirSessionCapabilities::ALL), 63);
    }
}
