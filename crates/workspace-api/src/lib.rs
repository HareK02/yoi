//! Shared Workspace HTTP resource contracts.
//!
//! This crate owns transport DTOs exposed by the Workspace Server and consumed
//! by Rust clients. Runtime-internal projections remain in their owning crates;
//! callers must explicitly construct these Workspace-authoritative resources.

use serde::{Deserialize, Serialize};
use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

/// Public browser-authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthPublicConfig {
    pub rp_id: String,
    pub origin: String,
    pub public_base_url: String,
    pub cookie_name: String,
}

/// Authentication method that established the current request actor.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ActorAuthMethod {
    BrowserSession,
    ApiToken,
}

/// Public user identity returned by authentication operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedUser {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
}

/// Authenticated actor returned by `GET /api/auth/whoami`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RequestActor {
    pub user_id: String,
    pub account_id: String,
    pub handle: String,
    pub display_name: String,
    pub auth_method: ActorAuthMethod,
}

impl RequestActor {
    pub fn user(&self) -> AuthenticatedUser {
        AuthenticatedUser {
            user_id: self.user_id.clone(),
            account_id: self.account_id.clone(),
            handle: self.handle.clone(),
            display_name: self.display_name.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WhoamiResponse {
    pub actor: Option<RequestActor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthBootstrapUserRequest {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct AuthUserResponse {
    pub user: AuthenticatedUser,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationOptionsRequest {
    pub handle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationOptionsResponse {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub public_key: CreationChallengeResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyRegistrationCompleteRequest {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub credential: RegisterPublicKeyCredential,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginOptionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub browser_origin: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginOptionsResponse {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub public_key: RequestChallengeResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PasskeyLoginCompleteRequest {
    pub challenge_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "unknown"))]
    pub credential: PublicKeyCredential,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginStartRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub client_name: Option<String>,
}

const DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS: u64 = 24 * 60 * 60;
const DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS: u64 = 60;

#[derive(Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct DeviceLoginStartResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expires_in: u64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub interval: u64,
}

impl<'de> Deserialize<'de> for DeviceLoginStartResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            device_code: String,
            user_code: String,
            verification_uri: String,
            verification_uri_complete: String,
            expires_in: u64,
            interval: u64,
        }

        let wire = Wire::deserialize(deserializer)?;
        if wire.expires_in == 0 || wire.expires_in > DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS {
            return Err(serde::de::Error::custom(format!(
                "expires_in must be between 1 and {DEVICE_LOGIN_EXPIRES_IN_MAX_SECONDS}"
            )));
        }
        if wire.interval == 0 || wire.interval > DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS {
            return Err(serde::de::Error::custom(format!(
                "interval must be between 1 and {DEVICE_LOGIN_POLL_INTERVAL_MAX_SECONDS}"
            )));
        }
        Ok(Self {
            device_code: wire.device_code,
            user_code: wire.user_code,
            verification_uri: wire.verification_uri,
            verification_uri_complete: wire.verification_uri_complete,
            expires_in: wire.expires_in,
            interval: wire.interval,
        })
    }
}

impl std::fmt::Debug for DeviceLoginStartResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLoginStartResponse")
            .field("device_code", &"[redacted]")
            .field("user_code", &self.user_code)
            .field("verification_uri", &self.verification_uri)
            .field("verification_uri_complete", &self.verification_uri_complete)
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginApproveRequest {
    pub user_code: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeviceLoginApprovalStatus {
    Approved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginApproveResponse {
    pub status: DeviceLoginApprovalStatus,
    pub user: AuthenticatedUser,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginPollRequest {
    pub device_code: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum DeviceAccessTokenType {
    Bearer,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DeviceLoginPollStatus {
    Pending,
    Approved,
    Expired,
    Denied,
    Consumed,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceLoginPollResponse {
    pub status: DeviceLoginPollStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub token_type: Option<DeviceAccessTokenType>,
}

impl std::fmt::Debug for DeviceLoginPollResponse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceLoginPollResponse")
            .field("status", &self.status)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[redacted]"),
            )
            .field("token_type", &self.token_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum LogoutStatus {
    LoggedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct LogoutResponse {
    pub status: LogoutStatus,
}

pub const REPOSITORY_KEY_MIN_LEN: usize = 1;
pub const REPOSITORY_KEY_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKeyError {
    Length,
    Character,
    LeadingHyphen,
    TrailingHyphen,
}

impl std::fmt::Display for RepositoryKeyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Length => "must contain between 1 and 64 ASCII bytes",
            Self::Character => "must contain only lowercase ASCII letters, digits, and hyphens",
            Self::LeadingHyphen => "must not start with a hyphen",
            Self::TrailingHyphen => "must not end with a hyphen",
        })
    }
}

impl std::error::Error for RepositoryKeyError {}

/// Validate one immutable Workspace-scoped Repository key.
///
/// Keys are deliberately not normalized: callers must submit the exact canonical
/// lowercase ASCII spelling so idempotency and route identity cannot alias.
pub fn validate_repository_key(value: &str) -> Result<(), RepositoryKeyError> {
    let bytes = value.as_bytes();
    if !(REPOSITORY_KEY_MIN_LEN..=REPOSITORY_KEY_MAX_LEN).contains(&bytes.len()) {
        return Err(RepositoryKeyError::Length);
    }
    if bytes[0] == b'-' {
        return Err(RepositoryKeyError::LeadingHyphen);
    }
    if bytes[bytes.len() - 1] == b'-' {
        return Err(RepositoryKeyError::TrailingHyphen);
    }
    if !bytes
        .iter()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
    {
        return Err(RepositoryKeyError::Character);
    }
    Ok(())
}

/// Provider-neutral classification of an authoritative Repository source.
///
/// Local paths remain distinct from network Git transports so callers cannot
/// accidentally treat an unmaterialized remote as a server-local filesystem path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositorySourceKind {
    LocalPath,
    File,
    Ssh,
    Http,
    Https,
    /// A legacy value that could not be classified during migration. It remains
    /// inspectable but every provider operation must fail closed.
    Invalid,
}

impl RepositorySourceKind {
    pub const fn is_remote(self) -> bool {
        matches!(self, Self::Ssh | Self::Http | Self::Https)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalPath => "local_path",
            Self::File => "file",
            Self::Ssh => "ssh",
            Self::Http => "http",
            Self::Https => "https",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "local_path" => Self::LocalPath,
            "file" => Self::File,
            "ssh" => Self::Ssh,
            "http" => Self::Http,
            "https" => Self::Https,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// Stable Repository source identity stored by Workspace authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct RepositorySource {
    pub kind: RepositorySourceKind,
    /// Canonical source representation. This is an absolute local path for
    /// `local_path`, and a normalized URI/remote specification otherwise.
    pub uri: String,
}

/// Browser/user intent for registering one Repository in a Workspace.
///
/// The Server parses and canonicalizes `source`; callers cannot assert a
/// transport classification or supply credential material through this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceRepositoryRequest {
    pub repository_key: String,
    pub source: String,
    #[serde(default)]
    pub default_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateWorkspaceRepositoryResponse {
    pub workspace_id: String,
    pub repository_key: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryObservedStatus {
    Unverified,
    Ready,
    Invalid,
}

impl RepositoryObservedStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::Ready => "ready",
            Self::Invalid => "invalid",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "unverified" => Self::Unverified,
            "ready" => Self::Ready,
            "invalid" => Self::Invalid,
            _ => return None,
        })
    }
}

/// Public Workspace catalog item returned by `GET /api/workspaces`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceSummary {
    pub workspace_id: String,
    pub owner_account_id: String,
    pub display_name: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Public response returned by `GET /api/workspaces`.
///
/// The transparent newtype keeps the established top-level JSON array while making the
/// complete list response a named cross-crate and generated-TypeScript authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceCatalogListResponse(pub Vec<WorkspaceSummary>);

/// Public Repository record embedded in Workspace creation responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRepositoryRecord {
    pub workspace_id: String,
    pub repository_key: String,
    pub kind: String,
    pub provider: Option<String>,
    pub source: RepositorySource,
    pub default_ref: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    pub observed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Response returned after atomically creating a Workspace and its first Repository.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCreateResponse {
    pub workspace: WorkspaceSummary,
    pub repository: WorkspaceRepositoryRecord,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub config_revision: u64,
    pub request_fingerprint: String,
    pub replayed: bool,
}

/// Browser authentication configuration exposed by the scoped Workspace summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub enum WorkspaceAuthConfig {
    Passkey {
        rp_id: String,
        origin: String,
        public_base_url: String,
        cookie_name: String,
    },
}

/// Backend-authoritative permissions for the current Workspace actor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspacePermissionSummary {
    pub manage_repositories: bool,
    pub manage_secrets: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPointState {
    pub status: String,
    pub note: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceExtensionPoints {
    pub store: String,
    pub event_stream: WorkspaceExtensionPointState,
    pub host_worker_bridge: WorkspaceExtensionPointState,
    pub companion_console: WorkspaceExtensionPointState,
}

/// Scoped Workspace metadata and current-actor permission projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub record_authority: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub schema_version: i64,
    pub auth: WorkspaceAuthConfig,
    pub permissions: WorkspacePermissionSummary,
    pub extension_points: WorkspaceExtensionPoints,
}

/// Workspace identity metadata exposed by the current settings resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataSettingsResponse {
    pub workspace_id: String,
    pub display_name: String,
    pub created_at: String,
    pub revision: String,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// Compare-and-swap update for Workspace identity display metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMetadataRequest {
    pub display_name: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMetadataMutationResponse {
    pub workspace: WorkspaceMetadataSettingsResponse,
    pub diagnostics: Vec<Diagnostic>,
}

/// Read-only Profile catalog projected from one active Workspace config revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct ProfileSettingsResponse {
    pub workspace_id: String,
    pub registry_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub config_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkspaceProfileSummary>,
    pub sources: Vec<WorkspaceProfileSourceSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSummary {
    pub profile_id: String,
    pub selector: String,
    pub label: String,
    pub source_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub editable: bool,
    pub is_default: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProfileSourceSummary {
    pub profile_source_id: String,
    pub display_path: String,
    pub kind: String,
    pub content_type: String,
    pub content_digest: String,
    pub provenance: WorkspaceProfileSourceProvenance,
    pub editable: bool,
    pub revision: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub size_bytes: u64,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceProfileSourceProvenance {
    ProjectProfileSourceTree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDiagnostic {
    pub severity: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRemoteSummary {
    pub name: String,
    pub fetch_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitRepositorySummary {
    pub status: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: bool,
    pub remotes: Vec<GitRemoteSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySummary {
    pub repository_key: String,
    pub kind: String,
    pub provider: String,
    pub source: RepositorySource,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub source_revision: u64,
    pub source_fingerprint: String,
    pub observed_status: RepositoryObservedStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub record_authority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub git: Option<GitRepositorySummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub diagnostics: Option<Vec<RepositoryDiagnostic>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct GitCommitSummary {
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author_name: String,
    pub author_email: String,
    pub author_date: String,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryListResponse {
    pub workspace_id: String,
    pub items: Vec<RepositorySummary>,
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryDetailResponse {
    pub workspace_id: String,
    pub item: RepositorySummary,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryLogResponse {
    pub workspace_id: String,
    pub repository_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
    pub limit: usize,
    pub items: Vec<GitCommitSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

pub const TICKET_RELATIONS_QUERY_PATH: &str = "/tickets/relations/search";
pub const TICKET_ORCHESTRATION_PLANS_QUERY_PATH: &str = "/tickets/orchestration-plans/search";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub code: String,
    pub severity: DiagnosticSeverity,
    pub message: String,
}

/// Public Workdir materializer classification.
///
/// The value identifies stable materialization provenance without exposing a
/// provider path, Runtime handle, or session identity.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryMaterializerKind {
    #[default]
    RuntimeGitCache,
    LocalGitWorktree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryStatusKind {
    Active,
    CleanupPending,
    Corrupted,
    NotFound,
    Unknown,
}

impl WorkingDirectoryStatusKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::CleanupPending => "cleanup_pending",
            Self::Corrupted => "corrupted",
            Self::NotFound => "not_found",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for WorkingDirectoryStatusKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalRequest {
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WorkingDirectoryRemovalDisposition {
    Removed,
    Retained,
    AttentionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRemovalResponse {
    pub working_directory_id: String,
    pub disposition: WorkingDirectoryRemovalDisposition,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_category: Option<String>,
}

/// Durable Workspace occupancy projection for one Workdir.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryOccupancy {
    pub runtime_id: String,
    pub worker_id: String,
    pub display_name: String,
    pub linked_at: String,
}

/// Runtime-internal Workdir cleanup authority. This transport intentionally
/// retains the Backend-generated Repository id and is never a Workspace public
/// projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectoryCleanupTarget {
    pub kind: String,
    pub working_directory_id: String,
    pub repository_id: String,
}

/// Runtime-internal Workdir inventory transport. Workspace REST and model-facing
/// surfaces must project this through [`WorkingDirectorySummary`] so the UUID is
/// replaced with `repository_key`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct RuntimeWorkingDirectorySummary {
    pub working_directory_id: String,
    pub repository_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<RuntimeWorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

/// Public, provider-neutral Workdir inventory projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectorySummary {
    pub working_directory_id: String,
    pub repository_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub observed_at_epoch_seconds: Option<u64>,
    pub materializer_kind: WorkingDirectoryMaterializerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_target: Option<WorkingDirectoryCleanupTarget>,
    pub status: WorkingDirectoryStatusKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanliness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupied_by: Option<WorkingDirectoryOccupancy>,
}

impl WorkingDirectorySummary {
    /// Workspace-managed inventory rows carry explicit cleanup authority.
    pub fn is_workspace_managed(&self) -> bool {
        self.cleanup_target.is_some()
    }
}

/// Browser/Rust-client Workdir materialization request.
///
/// `runtime_id = None` requests Workspace default Runtime resolution and
/// `operation_id = Some(_)` fences exact replay. All four fields deliberately
/// preserve the Server's existing optionality.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    pub repository_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryListResponse {
    pub workspace_id: String,
    pub items: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryDetailResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryCreateResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub item: WorkingDirectorySummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListResponse<T> {
    pub workspace_id: String,
    pub limit: usize,
    pub items: Vec<T>,
    pub source: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct QueryPage {
    pub limit: usize,
    pub returned: usize,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub sort: String,
    pub source_limit: Option<usize>,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveEventDetail {
    pub event_ref: String,
    pub kind: String,
    pub body: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveLinkedTicketSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveResourceSummary {
    pub path: String,
    pub media_type: Option<String>,
    pub bytes: usize,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveSummary {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub summary: String,
    pub linked_tickets: Vec<String>,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveDetail {
    pub id: String,
    pub resource_key: String,
    pub title: String,
    pub state: String,
    pub revision: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub linked_tickets: Vec<String>,
    pub linked_ticket_summaries: Vec<ObjectiveLinkedTicketSummary>,
    pub resources: Vec<ObjectiveResourceSummary>,
    pub body: String,
    pub body_truncated: bool,
    pub events: Vec<ObjectiveEventDetail>,
    pub event_page: QueryPage,
    pub record_source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveCreateRequest {
    pub title: String,
    #[serde(default)]
    pub body_md: String,
    #[serde(default = "default_objective_state")]
    pub state: String,
    #[serde(default)]
    pub linked_tickets: Vec<String>,
}

fn default_objective_state() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ObjectiveEditRequest {
    pub title: Option<String>,
    pub old_string: Option<String>,
    pub new_string: Option<String>,
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveStateRequest {
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectiveLinkTicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceKind {
    EmbeddedWorkerRuntime,
    RemoteHttp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSourceStatus {
    Active,
    Reserved,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeIdentityAuthority {
    RuntimeRegistryProjection,
    ServerRuntimeConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSourceSummary {
    pub kind: RuntimeSourceKind,
    pub status: RuntimeSourceStatus,
    pub identity_authority: RuntimeIdentityAuthority,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub runtime_id: String,
    pub label: String,
    pub kind: String,
    pub status: String,
    pub source: RuntimeSourceSummary,
    #[serde(default)]
    pub host_ids: Vec<String>,
    pub worker_creation_available: bool,
    pub os: String,
    pub arch: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeManagementSummary {
    pub built_in: bool,
    pub config_managed: bool,
    pub removable: bool,
    pub endpoint_configured: bool,
    pub token_ref_configured: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceRuntimeResource {
    #[serde(flatten)]
    pub runtime: RuntimeSummary,
    pub management: RuntimeManagementSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreateRemoteRuntimeRequest {
    pub runtime_id: String,
    pub display_name: Option<String>,
    pub endpoint: String,
    pub token_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConnectionTestResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub checked_at: String,
    pub state: String,
    pub protocol_version: Option<String>,
    pub compatibility_basis: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub health_result: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerWorkspaceSummary {
    pub visibility: String,
    pub identity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerImplementationSummary {
    pub kind: String,
    pub display_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerCapabilitySummary {
    pub can_stop: bool,
    pub can_spawn_followup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceWorkerSubject {
    RuntimeWorker {
        runtime_id: String,
        worker_id: String,
    },
}

/// Bounded, model-safe projection used by privileged Workspace Worker discovery.
/// Runtime placement appears only in the typed subject required by Worker
/// control operations; provider and launch internals are intentionally omitted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkspaceWorkerDiscoveryItem {
    pub subject: WorkspaceWorkerSubject,
    pub resource_key: String,
    pub display_name: String,
    pub profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

/// Public lifecycle projection for the Workspace Companion endpoint.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionLifecycleState {
    Idle,
    Running,
    Stopped,
}

/// Public outcome of a Companion message submission.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionMessageDisposition {
    Accepted,
    Rejected,
}

/// Public, bounded transport metadata for Companion status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTransportSummary {
    pub mode: String,
    pub available: bool,
}

/// Public Workspace Companion status response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionStatusResponse {
    pub state: CompanionLifecycleState,
    pub worker: Option<WorkspaceWorkerDiscoveryItem>,
    pub transport: CompanionTransportSummary,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Public Workspace Companion message request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageRequest {
    pub content: String,
}

/// Public Workspace Companion cancellation request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionCancelRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Public Workspace Companion message response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionMessageResponse {
    pub state: CompanionMessageDisposition,
    pub message: String,
}

/// User-visible role accepted in the public Companion transcript.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CompanionTranscriptRole {
    User,
    Assistant,
}

/// One allowlisted, user-visible Companion transcript item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptItem {
    pub sequence: usize,
    pub role: CompanionTranscriptRole,
    pub content: String,
    pub created_at: String,
}

/// Bounded public Companion transcript projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CompanionTranscriptProjection {
    pub state: CompanionLifecycleState,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub next: Option<usize>,
    pub items: Vec<CompanionTranscriptItem>,
}

#[cfg(feature = "typescript")]
pub fn companion_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceWorkerSubject::decl(&config),
        WorkspaceWorkerDiscoveryItem::decl(&config),
        CompanionLifecycleState::decl(&config),
        CompanionMessageDisposition::decl(&config),
        CompanionTransportSummary::decl(&config),
        CompanionStatusResponse::decl(&config),
        CompanionMessageRequest::decl(&config),
        CompanionCancelRequest::decl(&config),
        CompanionMessageResponse::decl(&config),
        CompanionTranscriptRole::decl(&config),
        CompanionTranscriptItem::decl(&config),
        CompanionTranscriptProjection::decl(&config),
    ];

    format!(
        "// Generated by `cargo run -p workspace-api --features typescript --example generate_companion_api_types`.\n// Do not edit manually.\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWorkerDiscoveryPage {
    pub workers: Vec<WorkspaceWorkerDiscoveryItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Workspace-authoritative Worker projection.
///
/// `resource_key` is required here even though Runtime-internal Worker summaries
/// do not carry one. The Workspace Server must resolve it from Workspace
/// authority before constructing this response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
pub struct WorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub resource_key: String,
    pub host_id: String,
    #[serde(default)]
    pub display_name: String,
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    pub state: String,
    pub last_seen_at: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_directory: Option<WorkingDirectorySummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

/// Runtime-owned Worker summary embedded in Worker launch responses.
///
/// This preserves the existing launch wire shape. Workspace-owned Worker list
/// and detail responses use [`WorkerSummary`] instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchWorkerSummary {
    pub runtime_id: String,
    pub worker_id: String,
    pub host_id: String,
    pub display_name: String,
    pub label: String,
    pub profile: Option<String>,
    pub singleton_key: Option<String>,
    pub tags: Vec<String>,
    pub workspace: WorkerWorkspaceSummary,
    pub state: String,
    pub last_seen_at: Option<String>,
    pub pinned: bool,
    pub retention_state: String,
    pub implementation: WorkerImplementationSummary,
    pub capabilities: WorkerCapabilitySummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub working_directory: Option<RuntimeWorkingDirectorySummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchOptionsResponse {
    pub workspace_id: String,
    pub runtimes: Vec<WorkerLaunchRuntimeOption>,
    pub default_profile: Option<String>,
    pub profiles: Vec<WorkerLaunchProfileCandidate>,
    pub repositories: Vec<WorkingDirectoryRepositoryOption>,
    pub working_directories: Vec<WorkingDirectorySummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchRuntimeOption {
    pub runtime_id: String,
    pub display_name: String,
    pub built_in: bool,
    pub worker_creation_available: bool,
    pub working_directory_required: bool,
    pub status: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerLaunchProfileCandidate {
    pub id: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkingDirectoryRepositoryOption {
    pub repository_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub default_selector: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkerWorkingDirectorySelection {
    pub working_directory_id: String,
    #[serde(default)]
    pub relative_cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerTicketAssignmentRequest {
    pub ticket_id: String,
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceWorkerRequest {
    pub runtime_id: String,
    pub display_name: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub ticket_assignment: Option<CreateWorkspaceWorkerTicketAssignmentRequest>,
    #[serde(default)]
    pub initial_submit: Vec<protocol::Segment>,
    #[serde(default)]
    pub working_directory: Option<BrowserWorkerWorkingDirectorySelection>,
    /// Backend idempotency key used only for authenticated Worker-owned spawn/control.
    #[serde(default)]
    pub control_operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserCreateWorkerResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub console_href: String,
    pub worker: WorkerLaunchWorkerSummary,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BrowserWorkspaceOrchestratorResponse {
    pub workspace_id: String,
    pub online: bool,
    pub disposition: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional = nullable))]
    pub worker: Option<WorkerLaunchWorkerSummary>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerOperationState {
    Accepted,
    Unsupported,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRestoreResult {
    pub state: WorkerOperationState,
    pub worker: Option<WorkerSummary>,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerRestoreResponse {
    pub workspace_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub result: WorkerRestoreResult,
}

/// Public Workspace Memory document projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryDocumentResponse {
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
    pub bytes: usize,
    pub record_source: String,
}

/// Candidate kinds exposed by the Memory staging resource.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MemoryCandidateKind {
    Preference,
    WorkingAssumption,
    Constraint,
    Decision,
    OpenQuestion,
    Lesson,
}

/// Typed, bounded provenance classification for public Memory evidence anchors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum MemoryEvidenceOriginKind {
    HumanInput,
    WorkerInput,
    FlowInstruction,
    BackendInstruction,
    ModelOutput,
    ToolOutput,
    DerivedSummary,
    LegacyUnknown,
}

/// Bounded origin metadata copied from one typed Memory evidence anchor.
///
/// This is provenance only. It carries no message body, prompt, reasoning,
/// secret, tool output, or authorization authority.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(optional_fields = nullable))]
#[serde(deny_unknown_fields)]
pub struct MemoryEvidenceOrigin {
    pub kind: MemoryEvidenceOriginKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_selector: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_definition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number | null"))]
    pub flow_definition_revision: Option<u64>,
}

/// Record-level source range for one Memory staging candidate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemorySourceRef {
    pub segment_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number]"))]
    pub range: [u64; 2],
}

/// Bounded evidence snippet included in one Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingEvidence {
    pub id: String,
    pub kind: String,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number] | null"))]
    pub entry_range: Option<[u64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "typescript",
        ts(optional, type = "MemoryEvidenceOrigin | null")
    )]
    pub origin: Option<MemoryEvidenceOrigin>,
    pub excerpt: Option<String>,
    pub summary: Option<String>,
}

/// Bounded source anchor included in one Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemorySourceEvidenceRef {
    pub session_id: Option<String>,
    pub segment_id: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "[number, number] | null"))]
    pub entry_range: Option<[u64; 2]>,
    pub evidence_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "typescript",
        ts(optional, type = "MemoryEvidenceOrigin | null")
    )]
    pub origin: Option<MemoryEvidenceOrigin>,
    pub evidence_kind: Option<String>,
    pub label: Option<String>,
    pub summary: Option<String>,
}

/// Public projection of one valid Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingRecord {
    pub schema_version: u32,
    pub id: String,
    pub extract_run_id: String,
    pub source: MemorySourceRef,
    pub kind: MemoryCandidateKind,
    pub claim: String,
    pub why_useful: String,
    pub staleness: Option<String>,
    pub evidence: Vec<MemoryStagingEvidence>,
    pub source_refs: Vec<MemorySourceEvidenceRef>,
}

/// Public list entry for one valid Memory staging record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingEntry {
    pub id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub byte_len: u64,
    pub record: MemoryStagingRecord,
}

/// Public response returned by the Workspace Memory staging list resource.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MemoryStagingListResponse {
    pub limit: usize,
    pub returned_count: usize,
    pub total_valid_count: usize,
    pub invalid_count: usize,
    pub truncated: bool,
    pub order: String,
    pub record_authority: String,
    pub items: Vec<MemoryStagingEntry>,
    pub diagnostics: Vec<Diagnostic>,
}

#[cfg(feature = "typescript")]
pub fn memory_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        MemoryDocumentResponse::decl(&config),
        MemoryCandidateKind::decl(&config),
        MemoryEvidenceOriginKind::decl(&config),
        MemoryEvidenceOrigin::decl(&config),
        MemorySourceRef::decl(&config),
        MemoryStagingEvidence::decl(&config),
        MemorySourceEvidenceRef::decl(&config),
        MemoryStagingRecord::decl(&config),
        MemoryStagingEntry::decl(&config),
        MemoryStagingListResponse::decl(&config),
    ];

    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

/// Workspace-owned Memory settings returned by the shared Server API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceMemorySettings {
    pub workspace_id: String,
    pub settings_revision: u64,
    pub language: String,
}

/// Compare-and-swap update for Workspace-owned Memory settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateWorkspaceMemorySettingsRequest {
    pub expected_revision: u64,
    pub language: String,
}

/// Public metadata for one Workspace-scoped Repository SSH credential.
///
/// Secret references and secret material are deliberately not part of this DTO.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshCredential {
    pub credential_id: String,
    pub workspace_id: String,
    pub name: String,
    pub public_key_algorithm: String,
    pub public_key_fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub current_revision: u64,
    pub status: String,
    pub created_at: String,
    pub rotated_at: Option<String>,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CreateRepositorySshCredentialRequest {
    pub operation_id: String,
    pub credential_id: String,
    pub name: String,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RotateRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
    pub private_key: String,
    #[serde(default)]
    pub passphrase: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshCredentialRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
}

/// Public metadata for an explicitly pinned SSH host key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshHostTrust {
    pub host_trust_id: String,
    pub workspace_id: String,
    pub hostname: String,
    pub port: u16,
    pub key_algorithm: String,
    pub host_key: String,
    pub fingerprint: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub current_revision: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub referenced_repositories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct PutRepositorySshHostTrustRequest {
    pub operation_id: String,
    pub host_trust_id: String,
    pub hostname: String,
    pub port: u16,
    pub host_key: String,
    #[serde(default)]
    #[cfg_attr(feature = "typescript", ts(type = "number | null"))]
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositorySshHostTrustRequest {
    pub operation_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositorySshAccessBinding {
    pub repository_key: String,
    pub credential_id: String,
    pub host_trust_id: String,
    pub access: RepositoryAccessMode,
}

/// Secret-free active Repository access projection consumed by later Runtime work.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessProjection {
    pub workspace_id: String,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub config_revision: u64,
    pub projection_digest: String,
    pub bindings: Vec<RepositorySshAccessBinding>,
}

pub const SKILL_CATALOG_AUTHORITY: &str = "workspace-config-skills-v1";
pub const SKILL_API_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const SKILL_API_MAX_CATALOG_ENTRIES: usize = 500;
pub const SKILL_API_MAX_OVERRIDES: usize = 64;
pub const SKILL_API_MAX_DIAGNOSTICS: usize = 100;
pub const SKILL_API_MAX_RESOURCES: usize = 500;
pub const SKILL_API_MAX_ALLOWED_TOOLS: usize = 100;
pub const SKILL_API_MAX_NAME_BYTES: usize = 128;
pub const SKILL_API_MAX_LABEL_BYTES: usize = 4_096;
pub const SKILL_API_MAX_BODY_BYTES: usize = 1_048_576;
pub const SKILL_API_MAX_PATH_BYTES: usize = 1_024;
pub const SKILL_API_MAX_DIGEST_BYTES: usize = 128;
pub const SKILL_API_MAX_RESPONSE_BYTES: usize = 2_097_152;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillDiagnostic {
    pub severity: SkillDiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    Builtin,
    Workspace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillProvenance {
    pub kind: SkillSourceKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub virtual_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional, type = "number"))]
    pub revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub source_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub tree_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillActivationStatus {
    Active,
    Inactive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[cfg_attr(feature = "typescript", ts(rename_all = "snake_case"))]
#[serde(rename_all = "snake_case")]
pub enum SkillProjectionStatus {
    Valid,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillProjectionIdentity {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub config_revision: u64,
    pub tree_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillResourceRef {
    pub kind: String,
    pub name: String,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "typescript", ts(optional))]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub activation_status: SkillActivationStatus,
    pub projection_status: SkillProjectionStatus,
    pub provenance: SkillProvenance,
    pub overrides: Vec<SkillProvenance>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillCatalogResponse {
    pub authority: String,
    pub projection: SkillProjectionIdentity,
    pub entries: Vec<SkillCatalogEntry>,
    pub diagnostics: Vec<SkillDiagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SkillDetailResponse {
    pub authority: String,
    pub projection: SkillProjectionIdentity,
    pub name: String,
    pub description: String,
    pub provenance: SkillProvenance,
    pub overrides: Vec<SkillProvenance>,
    pub diagnostics: Vec<SkillDiagnostic>,
    pub activation_status: SkillActivationStatus,
    pub projection_status: SkillProjectionStatus,
    pub body: String,
    pub allowed_tools: Vec<String>,
    pub allowed_tools_status: String,
    pub resources: Vec<SkillResourceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillApiValidationError {
    CollectionTooLarge,
    StringTooLarge,
    InvalidProjectionIdentity,
    InvalidProvenance,
    InvalidVirtualPath,
    StaleProjection,
}

impl std::fmt::Display for SkillApiValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::CollectionTooLarge => "Skill API collection exceeds its limit",
            Self::StringTooLarge => "Skill API string exceeds its limit",
            Self::InvalidProjectionIdentity => "Skill API projection identity is invalid",
            Self::InvalidProvenance => "Skill API provenance is invalid",
            Self::InvalidVirtualPath => "Skill API virtual path is invalid",
            Self::StaleProjection => "Workspace Skill projection is stale",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SkillApiValidationError {}

impl SkillProjectionIdentity {
    fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_safe_integer(self.config_revision)?;
        validate_nonempty_string(&self.tree_digest, SKILL_API_MAX_DIGEST_BYTES)
            .map_err(|_| SkillApiValidationError::InvalidProjectionIdentity)
    }
}

impl SkillProvenance {
    fn validate(
        &self,
        projection: &SkillProjectionIdentity,
    ) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.id, SKILL_API_MAX_LABEL_BYTES)?;
        validate_optional_string(self.virtual_path.as_deref(), SKILL_API_MAX_PATH_BYTES)?;
        validate_optional_string(self.source_digest.as_deref(), SKILL_API_MAX_DIGEST_BYTES)?;
        validate_optional_string(self.tree_digest.as_deref(), SKILL_API_MAX_DIGEST_BYTES)?;
        if let Some(revision) = self.revision {
            validate_safe_integer(revision)?;
        }

        let expected_prefix = match self.kind {
            SkillSourceKind::Builtin => "builtin:",
            SkillSourceKind::Workspace => "workspace:",
        };
        if !self.id.starts_with(expected_prefix)
            || self
                .virtual_path
                .as_deref()
                .is_none_or(|path| !is_virtual_path(path))
            || self.source_digest.is_none()
        {
            return Err(SkillApiValidationError::InvalidProvenance);
        }

        match self.kind {
            SkillSourceKind::Builtin => {
                if self.revision.is_some() || self.tree_digest.is_some() {
                    return Err(SkillApiValidationError::InvalidProvenance);
                }
            }
            SkillSourceKind::Workspace => {
                let Some(revision) = self.revision else {
                    return Err(SkillApiValidationError::InvalidProvenance);
                };
                let Some(tree_digest) = self.tree_digest.as_deref() else {
                    return Err(SkillApiValidationError::InvalidProvenance);
                };
                if revision != projection.config_revision || tree_digest != projection.tree_digest {
                    return Err(SkillApiValidationError::StaleProjection);
                }
            }
        }
        Ok(())
    }
}

impl SkillCatalogEntry {
    fn validate(
        &self,
        projection: &SkillProjectionIdentity,
    ) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.name, SKILL_API_MAX_NAME_BYTES)?;
        validate_string(&self.description, SKILL_API_MAX_LABEL_BYTES)?;
        validate_collection(&self.overrides, SKILL_API_MAX_OVERRIDES)?;
        validate_diagnostics(&self.diagnostics)?;
        self.provenance.validate(projection)?;
        for provenance in &self.overrides {
            provenance.validate(projection)?;
        }
        Ok(())
    }
}

impl SkillCatalogResponse {
    pub fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.authority, SKILL_API_MAX_LABEL_BYTES)?;
        if self.authority != SKILL_CATALOG_AUTHORITY {
            return Err(SkillApiValidationError::InvalidProjectionIdentity);
        }
        self.projection.validate()?;
        validate_collection(&self.entries, SKILL_API_MAX_CATALOG_ENTRIES)?;
        validate_diagnostics(&self.diagnostics)?;
        for entry in &self.entries {
            entry.validate(&self.projection)?;
        }
        Ok(())
    }
}

impl SkillDetailResponse {
    pub fn validate(&self) -> Result<(), SkillApiValidationError> {
        validate_nonempty_string(&self.authority, SKILL_API_MAX_LABEL_BYTES)?;
        if self.authority != SKILL_CATALOG_AUTHORITY {
            return Err(SkillApiValidationError::InvalidProjectionIdentity);
        }
        self.projection.validate()?;
        validate_nonempty_string(&self.name, SKILL_API_MAX_NAME_BYTES)?;
        validate_string(&self.description, SKILL_API_MAX_LABEL_BYTES)?;
        validate_string(&self.body, SKILL_API_MAX_BODY_BYTES)?;
        validate_strings(
            &self.allowed_tools,
            SKILL_API_MAX_ALLOWED_TOOLS,
            SKILL_API_MAX_LABEL_BYTES,
        )?;
        validate_nonempty_string(&self.allowed_tools_status, SKILL_API_MAX_LABEL_BYTES)?;
        validate_resources(&self.resources)?;
        validate_collection(&self.overrides, SKILL_API_MAX_OVERRIDES)?;
        validate_diagnostics(&self.diagnostics)?;
        self.provenance.validate(&self.projection)?;
        for provenance in &self.overrides {
            provenance.validate(&self.projection)?;
        }
        Ok(())
    }
}

fn validate_safe_integer(value: u64) -> Result<(), SkillApiValidationError> {
    if value <= SKILL_API_MAX_SAFE_INTEGER {
        Ok(())
    } else {
        Err(SkillApiValidationError::InvalidProjectionIdentity)
    }
}

fn validate_collection<T>(values: &[T], limit: usize) -> Result<(), SkillApiValidationError> {
    if values.len() <= limit {
        Ok(())
    } else {
        Err(SkillApiValidationError::CollectionTooLarge)
    }
}

fn validate_string(value: &str, limit: usize) -> Result<(), SkillApiValidationError> {
    if value.len() <= limit {
        Ok(())
    } else {
        Err(SkillApiValidationError::StringTooLarge)
    }
}

fn validate_nonempty_string(value: &str, limit: usize) -> Result<(), SkillApiValidationError> {
    validate_string(value, limit)?;
    if value.is_empty() {
        Err(SkillApiValidationError::StringTooLarge)
    } else {
        Ok(())
    }
}

fn validate_optional_string(
    value: Option<&str>,
    limit: usize,
) -> Result<(), SkillApiValidationError> {
    if let Some(value) = value {
        validate_nonempty_string(value, limit)?;
    }
    Ok(())
}

fn validate_strings(
    values: &[String],
    collection_limit: usize,
    string_limit: usize,
) -> Result<(), SkillApiValidationError> {
    validate_collection(values, collection_limit)?;
    for value in values {
        validate_nonempty_string(value, string_limit)?;
    }
    Ok(())
}

fn validate_resources(resources: &[SkillResourceRef]) -> Result<(), SkillApiValidationError> {
    validate_collection(resources, SKILL_API_MAX_RESOURCES)?;
    for resource in resources {
        validate_nonempty_string(&resource.kind, SKILL_API_MAX_LABEL_BYTES)?;
        validate_nonempty_string(&resource.name, SKILL_API_MAX_PATH_BYTES)?;
        if !is_virtual_path(&resource.name) {
            return Err(SkillApiValidationError::InvalidVirtualPath);
        }
        validate_optional_string(resource.diagnostic.as_deref(), SKILL_API_MAX_LABEL_BYTES)?;
    }
    Ok(())
}

fn is_virtual_path(value: &str) -> bool {
    !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn validate_diagnostics(diagnostics: &[SkillDiagnostic]) -> Result<(), SkillApiValidationError> {
    validate_collection(diagnostics, SKILL_API_MAX_DIAGNOSTICS)?;
    for diagnostic in diagnostics {
        validate_nonempty_string(&diagnostic.code, SKILL_API_MAX_LABEL_BYTES)?;
        validate_nonempty_string(&diagnostic.message, SKILL_API_MAX_LABEL_BYTES)?;
        validate_optional_string(diagnostic.source.as_deref(), SKILL_API_MAX_PATH_BYTES)?;
    }
    Ok(())
}

#[cfg(feature = "typescript")]
pub fn catalog_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        WorkspaceSummary::decl(&config),
        WorkspaceCatalogListResponse::decl(&config),
        WorkspaceRepositoryRecord::decl(&config),
        WorkspaceCreateResponse::decl(&config),
        WorkspaceAuthConfig::decl(&config),
        WorkspacePermissionSummary::decl(&config),
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkspaceExtensionPointState::decl(&config),
        WorkspaceExtensionPoints::decl(&config),
        WorkspaceResponse::decl(&config),
        WorkspaceMetadataSettingsResponse::decl(&config),
        UpdateWorkspaceMetadataRequest::decl(&config),
        WorkspaceMetadataMutationResponse::decl(&config),
        ProfileSettingsResponse::decl(&config),
        WorkspaceProfileSummary::decl(&config),
        WorkspaceProfileSourceSummary::decl(&config),
        WorkspaceProfileSourceProvenance::decl(&config),
        RepositorySourceKind::decl(&config),
        RepositorySource::decl(&config),
        RepositoryObservedStatus::decl(&config),
        RepositoryDiagnostic::decl(&config),
        GitRemoteSummary::decl(&config),
        GitRepositorySummary::decl(&config),
        RepositorySummary::decl(&config),
        GitCommitSummary::decl(&config),
        RepositoryListResponse::decl(&config),
        RepositoryDetailResponse::decl(&config),
        RepositoryLogResponse::decl(&config),
    ]
    .map(|declaration| format!("export {declaration}"));

    format!(
        "// This file is generated by `cargo run -p workspace-api --features typescript --example generate_typescript | deno fmt -`.\n// Do not edit this file directly.\n\n{}\n",
        declarations.join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn repository_access_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        RepositorySshCredential::decl(&config),
        CreateRepositorySshCredentialRequest::decl(&config),
        RotateRepositorySshCredentialRequest::decl(&config),
        DeleteRepositorySshCredentialRequest::decl(&config),
        RepositorySshHostTrust::decl(&config),
        PutRepositorySshHostTrustRequest::decl(&config),
        DeleteRepositorySshHostTrustRequest::decl(&config),
        RepositoryAccessMode::decl(&config),
        RepositorySshAccessBinding::decl(&config),
        RepositoryAccessProjection::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn skill_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        SkillDiagnosticSeverity::decl(&config),
        SkillDiagnostic::decl(&config),
        SkillSourceKind::decl(&config),
        SkillProvenance::decl(&config),
        SkillActivationStatus::decl(&config),
        SkillProjectionStatus::decl(&config),
        SkillProjectionIdentity::decl(&config),
        SkillResourceRef::decl(&config),
        SkillCatalogEntry::decl(&config),
        SkillCatalogResponse::decl(&config),
        SkillDetailResponse::decl(&config),
    ];
    let limits = format!(
        "export const SKILL_API_AUTHORITY = \"{SKILL_CATALOG_AUTHORITY}\" as const;\n\nexport const SKILL_API_LIMITS = {{\n  maxSafeInteger: {SKILL_API_MAX_SAFE_INTEGER},\n  maxCatalogEntries: {SKILL_API_MAX_CATALOG_ENTRIES},\n  maxOverrides: {SKILL_API_MAX_OVERRIDES},\n  maxDiagnostics: {SKILL_API_MAX_DIAGNOSTICS},\n  maxResources: {SKILL_API_MAX_RESOURCES},\n  maxAllowedTools: {SKILL_API_MAX_ALLOWED_TOOLS},\n  maxNameBytes: {SKILL_API_MAX_NAME_BYTES},\n  maxLabelBytes: {SKILL_API_MAX_LABEL_BYTES},\n  maxBodyBytes: {SKILL_API_MAX_BODY_BYTES},\n  maxPathBytes: {SKILL_API_MAX_PATH_BYTES},\n  maxDigestBytes: {SKILL_API_MAX_DIGEST_BYTES},\n  maxResponseBytes: {SKILL_API_MAX_RESPONSE_BYTES},\n}} as const;"
    );
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts\n\n{limits}\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn auth_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        AuthPublicConfig::decl(&config),
        ActorAuthMethod::decl(&config),
        AuthenticatedUser::decl(&config),
        RequestActor::decl(&config),
        WhoamiResponse::decl(&config),
        AuthBootstrapUserRequest::decl(&config),
        AuthUserResponse::decl(&config),
        PasskeyRegistrationOptionsRequest::decl(&config),
        PasskeyRegistrationOptionsResponse::decl(&config),
        PasskeyRegistrationCompleteRequest::decl(&config),
        PasskeyLoginOptionsRequest::decl(&config),
        PasskeyLoginOptionsResponse::decl(&config),
        PasskeyLoginCompleteRequest::decl(&config),
        DeviceLoginStartRequest::decl(&config),
        DeviceLoginStartResponse::decl(&config),
        DeviceLoginApproveRequest::decl(&config),
        DeviceLoginApprovalStatus::decl(&config),
        DeviceLoginApproveResponse::decl(&config),
        DeviceLoginPollRequest::decl(&config),
        DeviceAccessTokenType::decl(&config),
        DeviceLoginPollStatus::decl(&config),
        DeviceLoginPollResponse::decl(&config),
        LogoutStatus::decl(&config),
        LogoutResponse::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn workdir_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkingDirectoryMaterializerKind::decl(&config),
        WorkingDirectoryStatusKind::decl(&config),
        WorkingDirectoryCleanupTarget::decl(&config),
        WorkingDirectoryOccupancy::decl(&config),
        WorkingDirectorySummary::decl(&config),
        WorkingDirectoryCreateRequest::decl(&config),
        WorkingDirectoryListResponse::decl(&config),
        WorkingDirectoryDetailResponse::decl(&config),
        WorkingDirectoryCreateResponse::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(feature = "typescript")]
pub fn worker_launch_api_typescript() -> String {
    use ts_rs::TS;

    let config = ts_rs::Config::default();
    let declarations = [
        DiagnosticSeverity::decl(&config),
        Diagnostic::decl(&config),
        WorkingDirectoryMaterializerKind::decl(&config),
        WorkingDirectoryStatusKind::decl(&config),
        WorkingDirectoryCleanupTarget::decl(&config),
        RuntimeWorkingDirectoryCleanupTarget::decl(&config),
        RuntimeWorkingDirectorySummary::decl(&config),
        WorkingDirectoryOccupancy::decl(&config),
        WorkingDirectorySummary::decl(&config),
        WorkerWorkspaceSummary::decl(&config),
        WorkerImplementationSummary::decl(&config),
        WorkerCapabilitySummary::decl(&config),
        WorkerLaunchWorkerSummary::decl(&config),
        WorkerLaunchRuntimeOption::decl(&config),
        WorkerLaunchProfileCandidate::decl(&config),
        WorkingDirectoryRepositoryOption::decl(&config),
        WorkerLaunchOptionsResponse::decl(&config),
        BrowserWorkerWorkingDirectorySelection::decl(&config),
        CreateWorkspaceWorkerTicketAssignmentRequest::decl(&config),
        CreateWorkspaceWorkerRequest::decl(&config),
        BrowserCreateWorkerResponse::decl(&config),
        BrowserWorkspaceOrchestratorResponse::decl(&config),
    ];
    format!(
        "// Generated from workspace-api. Do not edit by hand.\n// Regenerate: cargo run -q -p workspace-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts\n\nimport type {{ Segment }} from \"./protocol\";\n\n{}\n",
        declarations
            .into_iter()
            .map(|declaration| format!("export {declaration}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    )
}

#[cfg(all(test, feature = "typescript"))]
mod worker_launch_typescript_tests {
    #[test]
    fn generated_worker_launch_api_contract_is_current() {
        let expected = super::worker_launch_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/worker-launch-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Worker launch API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_worker_launch_api_types > web/workspace/src/lib/generated/worker-launch-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod memory_typescript_tests {
    #[test]
    fn generated_memory_api_contract_is_current() {
        let expected = super::memory_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/memory-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Memory API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_memory_api_types > web/workspace/src/lib/generated/memory-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
            .replace(";}", "}")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod skill_typescript_tests {
    #[test]
    fn generated_skill_api_contract_is_current() {
        let expected = super::skill_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/skill-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Skill API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_skill_api_types > web/workspace/src/lib/generated/skill-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
            .replace(";}", "}")
    }
}

#[cfg(all(test, feature = "typescript"))]
mod workdir_typescript_tests {
    #[test]
    fn generated_workdir_api_contract_is_current() {
        let expected = super::workdir_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/workdir-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Workdir API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_workdir_api_types > web/workspace/src/lib/generated/workdir-api.ts` and format the generated file",
        );
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace("=|", "=")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_projection() -> SkillProjectionIdentity {
        SkillProjectionIdentity {
            config_revision: 42,
            tree_digest: "tree-digest".to_string(),
        }
    }

    fn builtin_skill_provenance() -> SkillProvenance {
        SkillProvenance {
            kind: SkillSourceKind::Builtin,
            id: "builtin:errors".to_string(),
            virtual_path: Some("skills/errors/SKILL.md".to_string()),
            revision: None,
            source_digest: Some("builtin-source-digest".to_string()),
            tree_digest: None,
        }
    }

    fn workspace_skill_provenance() -> SkillProvenance {
        SkillProvenance {
            kind: SkillSourceKind::Workspace,
            id: "workspace:skills/release/SKILL.md".to_string(),
            virtual_path: Some("skills/release/SKILL.md".to_string()),
            revision: Some(42),
            source_digest: Some("workspace-source-digest".to_string()),
            tree_digest: Some("tree-digest".to_string()),
        }
    }

    #[test]
    fn skill_catalog_round_trips_builtin_workspace_and_invalid_projection_entries() {
        let response = SkillCatalogResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            entries: vec![
                SkillCatalogEntry {
                    name: "errors".to_string(),
                    description: "Builtin guidance".to_string(),
                    activation_status: SkillActivationStatus::Active,
                    projection_status: SkillProjectionStatus::Valid,
                    provenance: builtin_skill_provenance(),
                    overrides: vec![],
                    diagnostics: vec![],
                },
                SkillCatalogEntry {
                    name: "release".to_string(),
                    description: "Workspace guidance".to_string(),
                    activation_status: SkillActivationStatus::Inactive,
                    projection_status: SkillProjectionStatus::Invalid,
                    provenance: workspace_skill_provenance(),
                    overrides: vec![builtin_skill_provenance()],
                    diagnostics: vec![SkillDiagnostic {
                        severity: SkillDiagnosticSeverity::Error,
                        code: "invalid_projection".to_string(),
                        message: "invalid projected Skill".to_string(),
                        source: Some("skills/release/SKILL.md".to_string()),
                    }],
                },
            ],
            diagnostics: vec![],
        };

        response.validate().expect("fixture should be valid");
        let json = serde_json::to_string(&response).expect("serialize Skill catalog");
        let decoded: SkillCatalogResponse =
            serde_json::from_str(&json).expect("deserialize Skill catalog");
        assert_eq!(decoded, response);
        assert!(!json.contains("\"revision\":null"));
        assert!(!json.contains("\"tree_digest\":null"));
    }

    #[test]
    fn skill_detail_round_trips_shared_response() {
        let response = SkillDetailResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            name: "release".to_string(),
            description: "Workspace guidance".to_string(),
            body: "# Release\n".to_string(),
            allowed_tools: vec!["Bash".to_string()],
            allowed_tools_status: "experimental_hint_only".to_string(),
            resources: vec![],
            activation_status: SkillActivationStatus::Active,
            projection_status: SkillProjectionStatus::Valid,
            provenance: workspace_skill_provenance(),
            overrides: vec![],
            diagnostics: vec![],
        };

        response.validate().expect("fixture should be valid");
        let decoded: SkillDetailResponse = serde_json::from_value(
            serde_json::to_value(&response).expect("serialize Skill detail"),
        )
        .expect("deserialize Skill detail");
        assert_eq!(decoded, response);
    }

    #[test]
    fn skill_projection_validation_detects_stale_workspace_revision() {
        let mut provenance = workspace_skill_provenance();
        provenance.revision = Some(41);
        let response = SkillCatalogResponse {
            authority: "workspace-config-skills-v1".to_string(),
            projection: skill_projection(),
            entries: vec![SkillCatalogEntry {
                name: "release".to_string(),
                description: String::new(),
                activation_status: SkillActivationStatus::Active,
                projection_status: SkillProjectionStatus::Valid,
                provenance,
                overrides: vec![],
                diagnostics: vec![],
            }],
            diagnostics: vec![],
        };

        assert_eq!(
            response.validate(),
            Err(SkillApiValidationError::StaleProjection)
        );
    }

    #[test]
    fn skill_dto_rejects_unknown_fields_and_unknown_provenance_kind() {
        let unknown_field = serde_json::json!({
            "authority": "workspace-config-skills-v1",
            "projection": {"config_revision": 42, "tree_digest": "tree-digest"},
            "entries": [],
            "diagnostics": [],
            "body": "must not be accepted"
        });
        assert!(serde_json::from_value::<SkillCatalogResponse>(unknown_field).is_err());

        let mut provenance =
            serde_json::to_value(workspace_skill_provenance()).expect("serialize provenance");
        provenance["kind"] = serde_json::Value::String("newer_source_kind".to_string());
        assert!(serde_json::from_value::<SkillProvenance>(provenance).is_err());
    }

    #[test]
    fn memory_evidence_origins_round_trip_as_typed_provenance() {
        let kinds = [
            MemoryEvidenceOriginKind::HumanInput,
            MemoryEvidenceOriginKind::WorkerInput,
            MemoryEvidenceOriginKind::FlowInstruction,
            MemoryEvidenceOriginKind::BackendInstruction,
            MemoryEvidenceOriginKind::ModelOutput,
            MemoryEvidenceOriginKind::ToolOutput,
            MemoryEvidenceOriginKind::DerivedSummary,
            MemoryEvidenceOriginKind::LegacyUnknown,
        ];
        for kind in kinds {
            let origin = MemoryEvidenceOrigin {
                kind,
                account_id: Some("account-1".to_string()),
                workspace_id: Some("workspace-1".to_string()),
                runtime_id: Some("runtime-1".to_string()),
                worker_id: Some("worker-1".to_string()),
                flow_selector: Some("builtin:coder-review".to_string()),
                flow_definition_id: Some("flow-1".to_string()),
                flow_definition_revision: Some(7),
            };
            let encoded = serde_json::to_value(&origin).unwrap();
            let decoded: MemoryEvidenceOrigin = serde_json::from_value(encoded).unwrap();
            assert_eq!(decoded, origin);
        }
    }

    #[test]
    fn memory_evidence_origin_rejects_unknown_kind_and_fields() {
        assert!(
            serde_json::from_value::<MemoryEvidenceOrigin>(
                serde_json::json!({"kind": "future_origin"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<MemoryEvidenceOrigin>(serde_json::json!({
                "kind": "human_input",
                "future_field": "not current schema"
            }))
            .is_err()
        );
    }

    fn worker_launch_summary() -> WorkerLaunchWorkerSummary {
        WorkerLaunchWorkerSummary {
            runtime_id: "runtime-a".to_string(),
            worker_id: "worker-a".to_string(),
            host_id: "host-a".to_string(),
            display_name: "Worker A".to_string(),
            label: "worker-a".to_string(),
            profile: None,
            singleton_key: None,
            tags: Vec::new(),
            workspace: WorkerWorkspaceSummary {
                visibility: "workspace".to_string(),
                identity: "workspace-a".to_string(),
                workspace_id: Some("workspace-a".to_string()),
            },
            state: "idle".to_string(),
            last_seen_at: None,
            pinned: false,
            retention_state: "active".to_string(),
            implementation: WorkerImplementationSummary {
                kind: "runtime".to_string(),
                display_hint: "Runtime Worker".to_string(),
            },
            capabilities: WorkerCapabilitySummary {
                can_stop: true,
                can_spawn_followup: false,
            },
            working_directory: None,
            diagnostics: Vec::new(),
        }
    }

    #[test]
    fn worker_launch_optional_omission_and_request_shape_are_stable() {
        assert_eq!(
            serde_json::to_value(WorkingDirectoryRepositoryOption {
                repository_key: "main".to_string(),
                default_selector: None,
            })
            .unwrap(),
            serde_json::json!({ "repository_key": "main" })
        );

        let orchestrator = serde_json::to_value(BrowserWorkspaceOrchestratorResponse {
            workspace_id: "workspace-a".to_string(),
            online: false,
            disposition: "unavailable".to_string(),
            worker: None,
            diagnostics: Vec::new(),
        })
        .unwrap();
        assert_eq!(
            orchestrator,
            serde_json::json!({
                "workspace_id": "workspace-a",
                "online": false,
                "disposition": "unavailable",
                "diagnostics": [],
            })
        );

        let worker = serde_json::to_value(worker_launch_summary()).unwrap();
        assert!(
            !worker
                .as_object()
                .unwrap()
                .contains_key("working_directory")
        );
        assert_eq!(worker["profile"], serde_json::Value::Null);
        assert_eq!(worker["singleton_key"], serde_json::Value::Null);
        assert_eq!(worker["last_seen_at"], serde_json::Value::Null);

        let request = serde_json::to_value(CreateWorkspaceWorkerRequest {
            runtime_id: "runtime-a".to_string(),
            display_name: "Worker A".to_string(),
            profile: None,
            ticket_assignment: None,
            initial_submit: Vec::new(),
            working_directory: None,
            control_operation_id: None,
        })
        .unwrap();
        assert_eq!(
            request,
            serde_json::json!({
                "runtime_id": "runtime-a",
                "display_name": "Worker A",
                "profile": null,
                "ticket_assignment": null,
                "initial_submit": [],
                "working_directory": null,
                "control_operation_id": null,
            })
        );
    }

    #[test]
    fn worker_launch_request_rejects_unknown_fields() {
        let error = serde_json::from_value::<CreateWorkspaceWorkerRequest>(serde_json::json!({
            "runtime_id": "runtime-a",
            "display_name": "Worker A",
            "unexpected": true,
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn repository_key_validation_is_canonical_and_bounded() {
        let max = "a".repeat(64);
        for valid in ["a", "main", "repo-42", max.as_str()] {
            assert_eq!(validate_repository_key(valid), Ok(()), "{valid}");
        }
        let too_long = "a".repeat(65);
        for invalid in [
            "",
            "-main",
            "main-",
            "Main",
            "main_repo",
            "main.repo",
            "日本語",
            too_long.as_str(),
        ] {
            assert!(validate_repository_key(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn workspace_and_repository_response_shapes_round_trip() {
        let workspace = serde_json::json!({
            "workspace_id": "workspace-test",
            "display_name": "Test",
            "record_authority": "workspace-control-plane",
            "schema_version": 46,
            "auth": {"Passkey": {
                "rp_id": "example.test",
                "origin": "https://example.test",
                "public_base_url": "https://example.test",
                "cookie_name": "yoi_session"
            }},
            "permissions": {
                "manage_repositories": true,
                "manage_secrets": true
            },
            "extension_points": {
                "store": "sqlite",
                "event_stream": {"status": "available", "note": "ready", "diagnostics": []},
                "host_worker_bridge": {"status": "available", "note": "ready", "diagnostics": []},
                "companion_console": {"status": "available", "note": "ready", "diagnostics": []}
            }
        });
        let parsed: WorkspaceResponse = serde_json::from_value(workspace.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), workspace);

        let catalog = serde_json::json!([{
            "workspace_id": "workspace-test",
            "owner_account_id": "user-test",
            "display_name": "Test",
            "state": "active",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        }]);
        let parsed: WorkspaceCatalogListResponse = serde_json::from_value(catalog.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), catalog);

        let repositories = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });
        let parsed: RepositoryListResponse = serde_json::from_value(repositories.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), repositories);
    }

    #[test]
    fn repository_response_rejects_stale_field_aliases() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "repository_key": "main",
                "display_name": "main",
                "kind": "git",
                "provider": "git",
                "source": {"kind": "local_path", "uri": "/srv/project"},
                "source_revision": 1,
                "source_fingerprint": "sha256:test",
                "observed_status": "ready",
                "record_authority": "workspace-control-plane"
            }],
            "source": "workspace-control-plane",
            "diagnostics": []
        });

        assert!(serde_json::from_value::<RepositoryListResponse>(stale).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_catalog_typescript_keeps_public_wrappers_and_nullability() {
        let output = catalog_typescript();
        assert!(
            output.contains("export type WorkspaceCatalogListResponse = Array<WorkspaceSummary>")
        );
        assert!(output.contains("export type WorkspaceResponse ="));
        assert!(output.contains("permissions: WorkspacePermissionSummary"));
        assert!(output.contains("export type RepositoryListResponse ="));
        assert!(output.contains("items: Array<RepositorySummary>"));
        assert!(output.contains("observed_at?: string | null"));
        assert!(output.contains("export type WorkspaceMetadataSettingsResponse ="));
        assert!(output.contains("export type WorkspaceMetadataMutationResponse ="));
        assert!(output.contains("export type ProfileSettingsResponse ="));
        assert!(output.contains("config_revision?: number | null"));
        assert!(output.contains("provenance: WorkspaceProfileSourceProvenance"));
        assert!(output.contains(
            "export type WorkspaceProfileSourceProvenance = \"project_profile_source_tree\""
        ));
        assert!(!output.contains("repository_key: string, display_name"));
    }

    #[test]
    fn worker_resource_key_is_required() {
        let payload = serde_json::json!({
            "runtime_id": "arcadia",
            "worker_id": "worker-1",
            "host_id": "host",
            "display_name": "Coder",
            "label": "Coder",
            "workspace": {
                "visibility": "workspace",
                "identity": "workspace-test"
            },
            "state": "idle",
            "implementation": {"kind": "worker", "display_hint": "Coder"},
            "capabilities": {"can_stop": true, "can_spawn_followup": false}
        });

        assert!(serde_json::from_value::<WorkerSummary>(payload).is_err());
    }

    fn round_trip<T>(value: T)
    where
        T: std::fmt::Debug + PartialEq + Serialize + for<'de> Deserialize<'de>,
    {
        let encoded = serde_json::to_vec(&value).expect("fixture should serialize");
        let decoded: T = serde_json::from_slice(&encoded).expect("fixture should deserialize");
        assert_eq!(decoded, value);
    }

    #[test]
    fn workspace_metadata_and_profile_projection_fixtures_round_trip() {
        let diagnostic = Diagnostic {
            code: "profile_projection_warning".to_string(),
            severity: DiagnosticSeverity::Warning,
            message: "projected from the active config revision".to_string(),
        };
        let metadata = WorkspaceMetadataSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            display_name: "Test".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            revision: "sha256:metadata".to_string(),
            source: "workspace-config".to_string(),
            diagnostics: vec![diagnostic.clone()],
        };
        round_trip(metadata.clone());
        round_trip(UpdateWorkspaceMetadataRequest {
            display_name: "Renamed".to_string(),
            revision: metadata.revision.clone(),
        });
        round_trip(WorkspaceMetadataMutationResponse {
            workspace: metadata,
            diagnostics: vec![],
        });

        round_trip(ProfileSettingsResponse {
            workspace_id: "workspace-test".to_string(),
            registry_revision: "config-source:7:sha256:tree:sha256:projection".to_string(),
            config_revision: Some(7),
            tree_digest: Some("sha256:tree".to_string()),
            projection_digest: Some("sha256:projection".to_string()),
            default_profile: Some("workspace:coder".to_string()),
            profiles: vec![WorkspaceProfileSummary {
                profile_id: "workspace:coder".to_string(),
                selector: "workspace:coder".to_string(),
                label: "Coder".to_string(),
                source_kind: "project".to_string(),
                profile_source_id: Some("profile-source-1".to_string()),
                description: None,
                editable: true,
                is_default: true,
                diagnostics: vec![diagnostic.clone()],
            }],
            sources: vec![WorkspaceProfileSourceSummary {
                profile_source_id: "profile-source-1".to_string(),
                display_path: "profiles/coder.dcdl".to_string(),
                kind: "profile".to_string(),
                content_type: "text/x-decodal".to_string(),
                content_digest: "sha256:source".to_string(),
                provenance: WorkspaceProfileSourceProvenance::ProjectProfileSourceTree,
                editable: false,
                revision: "config-source:7".to_string(),
                size_bytes: 128,
                diagnostics: vec![],
            }],
            diagnostics: vec![diagnostic],
        });

        let absent_optional_fields = serde_json::json!({
            "workspace_id": "workspace-test",
            "registry_revision": "builtin",
            "profiles": [],
            "sources": [],
            "diagnostics": []
        });
        let decoded: ProfileSettingsResponse =
            serde_json::from_value(absent_optional_fields.clone()).unwrap();
        assert_eq!(decoded.config_revision, None);
        assert_eq!(decoded.tree_digest, None);
        assert_eq!(decoded.projection_digest, None);
        assert_eq!(decoded.default_profile, None);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            absent_optional_fields
        );
    }

    fn companion_worker() -> WorkspaceWorkerDiscoveryItem {
        WorkspaceWorkerDiscoveryItem {
            subject: WorkspaceWorkerSubject::RuntimeWorker {
                runtime_id: "arcadia".to_string(),
                worker_id: "worker-7".to_string(),
            },
            resource_key: "W-7".to_string(),
            display_name: "Companion".to_string(),
            profile: Some("builtin:companion".to_string()),
            status: Some("idle".to_string()),
        }
    }

    #[test]
    fn companion_status_fixtures_round_trip() {
        for state in [
            CompanionLifecycleState::Idle,
            CompanionLifecycleState::Running,
            CompanionLifecycleState::Stopped,
        ] {
            round_trip(CompanionStatusResponse {
                state,
                worker: Some(companion_worker()),
                transport: CompanionTransportSummary {
                    mode: "worker_runtime".to_string(),
                    available: state != CompanionLifecycleState::Stopped,
                },
                diagnostics: Vec::new(),
            });
        }
    }

    #[test]
    fn companion_message_fixtures_round_trip() {
        for state in [
            CompanionMessageDisposition::Accepted,
            CompanionMessageDisposition::Rejected,
        ] {
            round_trip(CompanionMessageResponse {
                state,
                message: if state == CompanionMessageDisposition::Accepted {
                    "accepted"
                } else {
                    "rejected"
                }
                .to_string(),
            });
        }
    }

    #[test]
    fn auth_server_fixtures_round_trip_through_shared_dtos() {
        let whoami = serde_json::json!({
            "actor": {
                "user_id": "user-1",
                "account_id": "account-1",
                "handle": "hare",
                "display_name": "Hare",
                "auth_method": "browser_session"
            }
        });
        let decoded = serde_json::from_value::<WhoamiResponse>(whoami.clone())
            .expect("server whoami fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), whoami);

        let auth_config = serde_json::json!({
            "rp_id": "yoi.example",
            "origin": "https://yoi.example",
            "public_base_url": "https://yoi.example",
            "cookie_name": "yoi_workspace_session"
        });
        let decoded = serde_json::from_value::<AuthPublicConfig>(auth_config.clone())
            .expect("server auth-config fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), auth_config);

        let auth_user = serde_json::json!({
            "user": {
                "user_id": "user-1",
                "account_id": "account-1",
                "handle": "hare",
                "display_name": "Hare"
            }
        });
        let decoded = serde_json::from_value::<AuthUserResponse>(auth_user.clone())
            .expect("server auth-user fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), auth_user);

        let registration_options = serde_json::json!({
            "challenge_id": "challenge-1",
            "public_key": {
                "publicKey": {
                    "challenge": "AQID",
                    "rp": {"id": "localhost", "name": "Yoi"},
                    "user": {"id": "BAUG", "name": "hare", "displayName": "Hare"},
                    "pubKeyCredParams": [{"type": "public-key", "alg": -7}]
                }
            }
        });
        let decoded = serde_json::from_value::<PasskeyRegistrationOptionsResponse>(
            registration_options.clone(),
        )
        .expect("server registration options fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), registration_options);

        let registration_complete = serde_json::json!({
            "challenge_id": "challenge-1",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "response": {
                    "attestationObject": "AQID",
                    "clientDataJSON": "AQID",
                    "transports": ["internal"]
                },
                "type": "public-key",
                "clientExtensionResults": {},
                "authenticatorAttachment": "platform"
            }
        });
        let decoded =
            serde_json::from_value::<PasskeyRegistrationCompleteRequest>(registration_complete)
                .expect("server registration-complete fixture should match shared DTO");
        let encoded = serde_json::to_value(decoded).unwrap();
        serde_json::from_value::<PasskeyRegistrationCompleteRequest>(encoded)
            .expect("registration-complete DTO should round-trip");

        let login_options = serde_json::json!({
            "challenge_id": "challenge-2",
            "public_key": {
                "publicKey": {
                    "challenge": "AQID",
                    "rpId": "localhost",
                    "allowCredentials": [],
                    "userVerification": "preferred"
                }
            }
        });
        let decoded = serde_json::from_value::<PasskeyLoginOptionsResponse>(login_options.clone())
            .expect("server login-options fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), login_options);

        let login_complete = serde_json::json!({
            "challenge_id": "challenge-2",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "response": {
                    "authenticatorData": "AQID",
                    "clientDataJSON": "AQID",
                    "signature": "AQID",
                    "userHandle": null
                },
                "type": "public-key",
                "clientExtensionResults": {},
                "authenticatorAttachment": "platform"
            }
        });
        let decoded = serde_json::from_value::<PasskeyLoginCompleteRequest>(login_complete)
            .expect("server login-complete fixture should match shared DTO");
        let encoded = serde_json::to_value(decoded).unwrap();
        serde_json::from_value::<PasskeyLoginCompleteRequest>(encoded)
            .expect("login-complete DTO should round-trip");

        let device_start = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://yoi.example/login/device",
            "verification_uri_complete": "https://yoi.example/login/device?user_code=ABCD-EFGH",
            "expires_in": 600,
            "interval": 2
        });
        let decoded = serde_json::from_value::<DeviceLoginStartResponse>(device_start.clone())
            .expect("server device-login fixture should match shared DTO");
        assert_eq!(serde_json::to_value(decoded).unwrap(), device_start);

        let approved_user = AuthenticatedUser {
            user_id: "user-1".to_string(),
            account_id: "account-1".to_string(),
            handle: "hare".to_string(),
            display_name: "Hare".to_string(),
        };
        round_trip(DeviceLoginApproveResponse {
            status: DeviceLoginApprovalStatus::Approved,
            user: approved_user,
        });
        for status in [
            DeviceLoginPollStatus::Pending,
            DeviceLoginPollStatus::Expired,
            DeviceLoginPollStatus::Denied,
            DeviceLoginPollStatus::Consumed,
        ] {
            round_trip(DeviceLoginPollResponse {
                status,
                access_token: None,
                token_type: None,
            });
        }
        round_trip(DeviceLoginPollResponse {
            status: DeviceLoginPollStatus::Approved,
            access_token: Some("access-secret".to_string()),
            token_type: Some(DeviceAccessTokenType::Bearer),
        });
        round_trip(LogoutResponse {
            status: LogoutStatus::LoggedOut,
        });
    }

    #[test]
    fn auth_dtos_reject_unknown_status_and_fields() {
        let unknown_status = serde_json::json!({"status": "future_status"});
        assert!(
            serde_json::from_value::<DeviceLoginPollResponse>(unknown_status).is_err(),
            "unknown device-login statuses must fail closed"
        );

        let unsafe_expiry = serde_json::json!({
            "device_code": "device-secret",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://yoi.example/login/device",
            "verification_uri_complete": "https://yoi.example/login/device?user_code=ABCD-EFGH",
            "expires_in": 0,
            "interval": 2
        });
        assert!(serde_json::from_value::<DeviceLoginStartResponse>(unsafe_expiry).is_err());

        let malformed_credential = serde_json::json!({
            "challenge_id": "challenge-1",
            "credential": {
                "id": "AQID",
                "rawId": "AQID",
                "type": "public-key",
                "response": {"clientDataJSON": 42}
            }
        });
        assert!(
            serde_json::from_value::<PasskeyLoginCompleteRequest>(malformed_credential).is_err(),
            "malformed passkey credential payloads must fail closed"
        );

        let unexpected_field = serde_json::json!({
            "actor": null,
            "access_token": "must-not-be-accepted"
        });
        assert!(serde_json::from_value::<WhoamiResponse>(unexpected_field).is_err());
    }

    #[test]
    fn device_login_debug_output_redacts_secret_material() {
        let start = DeviceLoginStartResponse {
            device_code: "device-secret".to_string(),
            user_code: "ABCD-EFGH".to_string(),
            verification_uri: "https://yoi.example/login/device".to_string(),
            verification_uri_complete: "https://yoi.example/login/device?user_code=ABCD-EFGH"
                .to_string(),
            expires_in: 600,
            interval: 2,
        };
        let start_debug = format!("{start:?}");
        assert!(!start_debug.contains("device-secret"));
        assert!(start_debug.contains("[redacted]"));

        let poll = DeviceLoginPollResponse {
            status: DeviceLoginPollStatus::Approved,
            access_token: Some("access-secret".to_string()),
            token_type: Some(DeviceAccessTokenType::Bearer),
        };
        let poll_debug = format!("{poll:?}");
        assert!(!poll_debug.contains("access-secret"));
        assert!(poll_debug.contains("[redacted]"));
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_auth_api_contract_is_current() {
        let expected = auth_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/auth-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize_typescript(&actual),
            normalize_typescript(&expected),
            "regenerate auth API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_auth_api_types > web/workspace/src/lib/generated/auth-api.ts` and format the generated file",
        );
    }

    #[test]
    fn companion_transcript_fixture_round_trips() {
        round_trip(CompanionTranscriptProjection {
            state: CompanionLifecycleState::Idle,
            start: 0,
            limit: 2,
            total: 2,
            next: None,
            items: vec![
                CompanionTranscriptItem {
                    sequence: 1,
                    role: CompanionTranscriptRole::User,
                    content: "hello".to_string(),
                    created_at: "2026-08-31T00:00:00Z".to_string(),
                },
                CompanionTranscriptItem {
                    sequence: 2,
                    role: CompanionTranscriptRole::Assistant,
                    content: "hi".to_string(),
                    created_at: "2026-08-31T00:00:01Z".to_string(),
                },
            ],
        });
    }

    #[test]
    fn companion_transcript_rejects_system_and_private_fields() {
        let public_item = CompanionTranscriptItem {
            sequence: 1,
            role: CompanionTranscriptRole::Assistant,
            content: "visible".to_string(),
            created_at: "2026-08-31T00:00:00Z".to_string(),
        };
        let public_fields = serde_json::to_value(public_item)
            .expect("public transcript item should serialize")
            .as_object()
            .expect("public transcript item should be an object")
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            public_fields,
            ["content", "created_at", "role", "sequence"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );

        let system_item = serde_json::json!({
            "sequence": 1,
            "role": "system",
            "content": "raw system prompt",
            "created_at": "2026-08-31T00:00:00Z"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(system_item).is_err());

        let private_item = serde_json::json!({
            "sequence": 1,
            "role": "assistant",
            "content": "visible",
            "created_at": "2026-08-31T00:00:00Z",
            "reasoning": "hidden",
            "credential": "secret",
            "provider_session_id": "session-private"
        });
        assert!(serde_json::from_value::<CompanionTranscriptItem>(private_item).is_err());
    }

    #[cfg(feature = "typescript")]
    #[test]
    fn generated_companion_api_contract_is_current() {
        let expected = companion_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/companion-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize_typescript(&actual),
            normalize_typescript(&expected),
            "regenerate Companion API TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_companion_api_types > web/workspace/src/lib/generated/companion-api.ts` and format the generated file",
        );
    }

    #[cfg(feature = "typescript")]
    fn normalize_typescript(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect::<String>()
            .replace(";}", "}")
            .replace("=|", "=")
    }

    #[test]
    fn workdir_create_request_preserves_optional_operation_fields() {
        let payload = serde_json::json!({"repository_key": "main"});
        let request = serde_json::from_value::<WorkingDirectoryCreateRequest>(payload)
            .expect("optional create fields may be absent");

        assert_eq!(request.runtime_id, None);
        assert_eq!(request.selector, None);
        assert_eq!(request.operation_id, None);

        let serialized = serde_json::to_value(request).expect("serialize create request");
        assert_eq!(serialized, serde_json::json!({"repository_key": "main"}));
    }

    #[test]
    fn workdir_create_request_rejects_stale_or_incomplete_json() {
        let stale = serde_json::json!({
            "repository_key": "main",
            "selector": "develop",
            "path": "/tmp/workdir"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(stale).is_err());

        let incomplete = serde_json::json!({
            "runtime_id": "arcadia",
            "operation_id": "operation-1"
        });
        assert!(serde_json::from_value::<WorkingDirectoryCreateRequest>(incomplete).is_err());
    }

    #[test]
    fn workdir_summary_omits_absent_optional_fields_on_the_wire() {
        let value = serde_json::to_value(WorkingDirectorySummary {
            working_directory_id: "workdir-1".into(),
            repository_key: "main".into(),
            creation_selector: None,
            creation_ref: None,
            creation_tree: None,
            current_selector: None,
            current_ref: None,
            current_tree: None,
            observed_at_epoch_seconds: None,
            materializer_kind: WorkingDirectoryMaterializerKind::RuntimeGitCache,
            cleanup_target: None,
            status: WorkingDirectoryStatusKind::Active,
            cleanliness: None,
            primary_worker_id: None,
            occupied_by: None,
        })
        .expect("serialize Workdir summary");
        let object = value.as_object().expect("Workdir summary object");

        for key in [
            "creation_selector",
            "creation_ref",
            "creation_tree",
            "current_selector",
            "current_ref",
            "current_tree",
            "observed_at_epoch_seconds",
            "cleanup_target",
            "cleanliness",
            "primary_worker_id",
            "occupied_by",
        ] {
            assert!(
                !object.contains_key(key),
                "absent field {key} must be omitted"
            );
        }
    }

    #[test]
    fn workdir_response_rejects_stale_occupancy_shape() {
        let stale = serde_json::json!({
            "workspace_id": "workspace-test",
            "items": [{
                "working_directory_id": "workdir-1",
                "repository_key": "main",
                "materializer_kind": "runtime_git_cache",
                "status": "active",
                "occupied_by": {
                    "runtime_worker_id": "worker-1",
                    "display_name": "Coder",
                    "linked_at": "2026-01-01T00:00:00Z"
                }
            }],
            "diagnostics": []
        });

        assert!(serde_json::from_value::<WorkingDirectoryListResponse>(stale).is_err());
    }
}

#[cfg(all(test, feature = "typescript"))]
mod typescript_tests {
    #[test]
    fn generated_repository_access_contract_is_current() {
        let expected = super::repository_access_api_typescript();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../web/workspace/src/lib/generated/repository-access-api.ts");
        let actual = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        assert_eq!(
            normalize(&actual),
            normalize(&expected),
            "regenerate Repository Access TypeScript types with `cargo run -q -p workspace-api --features typescript --example generate_repository_access_types > web/workspace/src/lib/generated/repository-access-api.ts` and format the generated file",
        );
    }

    #[test]
    fn generated_repository_access_responses_remain_secret_free() {
        use ts_rs::TS;

        let config = ts_rs::Config::default();
        for declaration in [
            super::RepositorySshCredential::decl(&config),
            super::RepositorySshHostTrust::decl(&config),
            super::RepositoryAccessProjection::decl(&config),
        ] {
            for forbidden in ["private_key", "passphrase", "secret_ref"] {
                assert!(
                    !declaration.contains(forbidden),
                    "Repository Access response declaration must not expose `{forbidden}`"
                );
            }
        }
    }

    fn normalize(value: &str) -> String {
        value
            .chars()
            .filter_map(|character| match character {
                character if character.is_whitespace() => None,
                ',' => Some(';'),
                character => Some(character),
            })
            .collect()
    }
}
