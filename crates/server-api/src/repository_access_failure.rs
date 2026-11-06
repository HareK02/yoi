//! Repository access decisions, separate from repository lookup/provider failures.
//!
//! Causes are attached where the access owner makes a known decision, never
//! inferred from error prose. The original error remains local. Only closed
//! operation/reason values are disclosed after a current visibility check.
//! Category is not HTTP classification, retry authority or an operation outcome.

use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
    compose_explanation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessOperation {
    ProjectAccess,
    CreateCredential,
    RotateCredential,
    DeleteCredential,
    PutHostTrust,
    DeleteHostTrust,
    DefaultSshBinding,
    LeaseSshAccess,
}

impl RepositoryAccessOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectAccess => "project_access",
            Self::CreateCredential => "create_credential",
            Self::RotateCredential => "rotate_credential",
            Self::DeleteCredential => "delete_credential",
            Self::PutHostTrust => "put_host_trust",
            Self::DeleteHostTrust => "delete_host_trust",
            Self::DefaultSshBinding => "default_ssh_binding",
            Self::LeaseSshAccess => "lease_ssh_access",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ProjectAccess => "access configuration projection",
            Self::CreateCredential => "SSH credential creation",
            Self::RotateCredential => "SSH credential rotation",
            Self::DeleteCredential => "SSH credential deletion",
            Self::PutHostTrust => "SSH host trust creation or update",
            Self::DeleteHostTrust => "SSH host trust deletion",
            Self::DefaultSshBinding => "default SSH access binding selection",
            Self::LeaseSshAccess => "SSH materialization access leasing",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RepositoryAccessReason {
    PlainHttpSourceUnsupported,
    UnknownRepository,
    SshSourceRequired,
    BindingCredentialNotFound,
    CredentialInactive,
    BindingHostTrustNotFound,
    HostEndpointMismatch,
    CredentialAlreadyExists,
    WorkspaceDefaultCredentialImmutable,
    CredentialNotFound,
    CredentialObservationChanged,
    CredentialKeyChanged,
    CredentialReferenced,
    CredentialRetained,
    HostTrustObservationChanged,
    HostTrustNotFound,
    HostTrustKeyChanged,
    HostTrustReferenced,
    AmbiguousHostTrust,
    CredentialUnavailableOrInactive,
    HostTrustUnavailable,
}

impl RepositoryAccessReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PlainHttpSourceUnsupported => "plain_http_source_unsupported",
            Self::UnknownRepository => "unknown_repository",
            Self::SshSourceRequired => "ssh_source_required",
            Self::BindingCredentialNotFound => "binding_credential_not_found",
            Self::CredentialInactive => "credential_inactive",
            Self::BindingHostTrustNotFound => "binding_host_trust_not_found",
            Self::HostEndpointMismatch => "host_endpoint_mismatch",
            Self::CredentialAlreadyExists => "credential_already_exists",
            Self::WorkspaceDefaultCredentialImmutable => "workspace_default_credential_immutable",
            Self::CredentialNotFound => "credential_not_found",
            Self::CredentialObservationChanged => "credential_observation_changed",
            Self::CredentialKeyChanged => "credential_key_changed",
            Self::CredentialReferenced => "credential_referenced",
            Self::CredentialRetained => "credential_retained",
            Self::HostTrustObservationChanged => "host_trust_observation_changed",
            Self::HostTrustNotFound => "host_trust_not_found",
            Self::HostTrustKeyChanged => "host_trust_key_changed",
            Self::HostTrustReferenced => "host_trust_referenced",
            Self::AmbiguousHostTrust => "ambiguous_host_trust",
            Self::CredentialUnavailableOrInactive => "credential_unavailable_or_inactive",
            Self::HostTrustUnavailable => "host_trust_unavailable",
        }
    }

    pub fn explanation(self) -> &'static str {
        match self {
            Self::PlainHttpSourceUnsupported => {
                "unsupported plain HTTP repository sources cannot supply access; use HTTPS or SSH"
            }
            Self::UnknownRepository => "the configured repository was not found",
            Self::SshSourceRequired => "the access binding requires an SSH repository source",
            Self::BindingCredentialNotFound => "the configured SSH credential was not found",
            Self::CredentialInactive => "the configured SSH credential is not active",
            Self::BindingHostTrustNotFound => "the configured SSH host trust was not found",
            Self::HostEndpointMismatch => {
                "the repository SSH endpoint does not match the host trust"
            }
            Self::CredentialAlreadyExists => "the SSH credential already exists",
            Self::WorkspaceDefaultCredentialImmutable => {
                "the workspace default SSH credential is immutable"
            }
            Self::CredentialNotFound => "the SSH credential to modify was not found",
            Self::CredentialObservationChanged => {
                "the SSH credential key fingerprint or active status no longer satisfies the observation"
            }
            Self::CredentialKeyChanged => {
                "the SSH credential key fingerprint no longer satisfies the observation"
            }
            Self::CredentialReferenced => {
                "the SSH credential is referenced by active workspace configuration"
            }
            Self::CredentialRetained => {
                "the SSH credential is retained by a retryable workdir creation operation"
            }
            Self::HostTrustObservationChanged => {
                "the SSH host trust no longer satisfies the create or update observation"
            }
            Self::HostTrustNotFound => "the SSH host trust to delete was not found",
            Self::HostTrustKeyChanged => {
                "the SSH host trust key fingerprint no longer satisfies the observation"
            }
            Self::HostTrustReferenced => {
                "the SSH host trust is referenced by active workspace configuration"
            }
            Self::AmbiguousHostTrust => {
                "multiple SSH host trusts match the endpoint; an explicit access binding is required"
            }
            Self::CredentialUnavailableOrInactive => {
                "the SSH credential is unavailable or inactive"
            }
            Self::HostTrustUnavailable => "the SSH host trust is unavailable",
        }
    }

    fn category(self) -> Category {
        match self {
            Self::CredentialAlreadyExists
            | Self::WorkspaceDefaultCredentialImmutable
            | Self::CredentialObservationChanged
            | Self::CredentialKeyChanged
            | Self::CredentialReferenced
            | Self::CredentialRetained
            | Self::HostTrustObservationChanged
            | Self::HostTrustKeyChanged
            | Self::HostTrustReferenced => Category::Conflict,
            Self::PlainHttpSourceUnsupported
            | Self::UnknownRepository
            | Self::SshSourceRequired
            | Self::BindingCredentialNotFound
            | Self::CredentialInactive
            | Self::BindingHostTrustNotFound
            | Self::HostEndpointMismatch
            | Self::CredentialNotFound
            | Self::HostTrustNotFound
            | Self::AmbiguousHostTrust
            | Self::CredentialUnavailableOrInactive
            | Self::HostTrustUnavailable => Category::InvalidInput,
        }
    }
}

/// No raw resource, source URI, key, fingerprint, operation ID or lease is a field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepositoryAccessDetails {
    pub operation: RepositoryAccessOperation,
    pub reason: RepositoryAccessReason,
}

impl RepositoryAccessDetails {
    pub fn disclosure(&self) -> Result<Disclosure<Self>, ContractError> {
        let impact = format!(
            "Repository {} could not complete",
            self.operation.description()
        );
        let mut disclosure = Disclosure::new(
            *self,
            compose_explanation(self.reason.explanation(), &impact)?,
        );
        disclosure
            .context
            .operations
            .push(OperationContext::new(self.operation.as_str(), "operation")?);
        Ok(disclosure)
    }
}

impl FailureCause for RepositoryAccessDetails {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "repository_access",
            reason: self.reason.as_str(),
            category: self.reason.category(),
            details_version: 1,
        }
    }

    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}

impl WireDetails for RepositoryAccessDetails {
    fn supports(domain: &str, reason: &str, details_version: u16) -> bool {
        domain == "repository_access"
            && details_version == 1
            && serde_json::from_value::<RepositoryAccessReason>(serde_json::Value::String(
                reason.to_owned(),
            ))
            .is_ok()
    }
}

/// The authenticated boundary must check current cause visibility for each use.
#[derive(Clone, Copy, Debug, Default)]
pub struct RepositoryAccessVisibility {
    pub can_view_cause: bool,
}

macro_rules! project_for {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for RepositoryAccessDetails {
            type Authorization = RepositoryAccessVisibility;
            type Details = Self;
            fn disclose(
                &self,
                _: &FailureFacts<Self>,
                visibility: &RepositoryAccessVisibility,
            ) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause {
                    return Err(ContractError::ProjectionUnavailable);
                }
                self.disclosure()
            }
        }
    )+};
}
project_for!(Browser, Tool, InternalTransfer, Storage, Telemetry);
