//! Closed Workdir management observations. Operation state and recovery stay
//! with the existing Runtime/Backend receipts; neither category nor explanation
//! attests to dispatch, safe retry, occupancy release or current authorization.
use error_contract::{
    Browser, Category, ContractError, Disclosure, FailureCause, FailureFacts, FailureIdentity,
    InternalTransfer, OperationContext, ProjectFor, Storage, Telemetry, Tool, WireDetails,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! reasons {
    ($($variant:ident => ($code:literal, $category:ident, $status:literal, $explanation:literal)),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
        pub enum WorkdirManagementReason {
            $(#[serde(rename = $code)] $variant),+
        }
        impl WorkdirManagementReason {
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];
            pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $code),+ } }
            pub const fn explanation(self) -> &'static str { match self { $(Self::$variant => $explanation),+ } }
            pub const fn category(self) -> Category { match self { $(Self::$variant => Category::$category),+ } }
            /// The Runtime owner's boundary classes, independent of public prose.
            pub const fn runtime_http_status(self) -> u16 { match self { $(Self::$variant => $status),+ } }
        }
        impl std::fmt::Display for WorkdirManagementReason {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    }
}
reasons! {
    EmbeddedWorkdirUnsupported => ("embedded_worker_workdir_unsupported", InvalidInput, 400, "the embedded Runtime supports no-Workdir Workers only; select a non-embedded Runtime for filesystem attachments"),
    LegacyExternalGrantPermissions => ("external_workdir_legacy_permissions", Authorization, 400, "the External grant uses legacy non-hierarchical permissions; revoke that grant and share again with READ, WRITE, or COMMAND"),
    AttachmentRegistryNotFound => ("workdir_attachment_registry_not_found", InvalidInput, 502, "the Workdir is not registered in this Workspace; rediscover the current inventory before attaching"),
    AttachmentNotAvailable => ("workdir_attachment_not_available", Conflict, 409, "the registered Workdir materialization is not available for attachment"),
    AttachmentGrantBoundRequired => ("workdir_attachment_grant_bound_required", Authorization, 403, "this logical Workspace config Workdir requires its grant-bound attachment operation"),
    ExternalGrantUnavailable => ("workdir_external_grant_unavailable", Dependency, 502, "the External Workdir grant or its matching online, unexpired provider connection is unavailable"),
    GrantStateOutcomeUnknown => ("external_workdir_grant_state_outcome_unknown", Unknown, 503, "the grant database operation may have committed; inspect the existing grant state and do not automatically resend the mutation"),
    GrantNotFound => ("external_workdir_grant_not_found", InvalidInput, 400, "the External Workdir grant was not found in this Workspace"),
    GrantCreatorRequired => ("external_workdir_grant_creator_required", Authorization, 403, "the operation requires the grant creator's current authenticated client"),
    GrantDisplayNameInvalid => ("external_workdir_grant_display_name_invalid", InvalidInput, 400, "the display name must contain 1 to 100 bytes and no control characters"),
    GrantTtlInvalid => ("external_workdir_grant_ttl_invalid", InvalidInput, 400, "the expiry duration must be omitted or between 60 and 86400 seconds"),
    GrantProviderIdentityInvalid => ("external_workdir_grant_provider_identity_invalid", InvalidInput, 400, "the provider instance identity is invalid"),
    GrantRevoked => ("external_workdir_grant_revoked", Conflict, 400, "the grant has been revoked; a provider connection cannot restore its authority"),
    GrantExpired => ("external_workdir_grant_expired", Conflict, 400, "the grant has expired; a provider connection cannot extend its authority"),
    GrantActivationFailed => ("external_workdir_grant_activation_failed", Dependency, 500, "the provider activation could not be confirmed; inspect the current grant without repeating an unknown mutation"),
    GrantConnectionStateFailed => ("external_workdir_connection_state_failed", Dependency, 500, "the connection state update could not be confirmed; connection evidence does not revise grant authority"),
    GrantConnectionTransportFailed => ("external_workdir_connection_transport_failed", Dependency, 503, "the provider connection ended or could not deliver a frame"),
    GrantConnectionProtocolFailed => ("external_workdir_connection_protocol_failed", Dependency, 502, "the provider connection did not satisfy the registration or frame protocol"),
    GrantConnectionPersistenceFailed => ("external_workdir_connection_persistence_failed", Dependency, 500, "the connection observation could not be saved; no grant mutation or cleanup retry is authorized"),
    GrantConnectionQueryFailed => ("external_workdir_connection_query_failed", Dependency, 500, "the saved connection observation could not be read; grant authority and cleanup remain independent"),
    GrantAuditFailed => ("external_workdir_grant_audit_failed", Dependency, 500, "the audit record could not be confirmed; inspect the grant and cleanup results separately, without repeating the original mutation"),
    GrantCleanupPersistenceFailed => ("external_workdir_grant_cleanup_persistence_failed", Dependency, 500, "the cleanup observation could not be saved; the grant fence and attachment release result remain separate"),
    GrantCleanupQueryFailed => ("external_workdir_grant_cleanup_query_failed", Dependency, 500, "the saved cleanup observation could not be read; cleanup completion is unconfirmed"),
    GrantAttachmentReleaseFailed => ("external_workdir_grant_attachment_release_failed", Dependency, 500, "attachment release or Runtime projection could not be confirmed; the grant remains fenced and residual resources require inspection"),
    GrantCleanupOutcomeUnknown => ("external_workdir_grant_cleanup_outcome_unknown", Unknown, 503, "the cleanup database operation may have committed; inspect the existing grant and attachment state rather than automatically resending the mutation"),
    AttachmentConflict => ("workdir_attachment_conflict", Conflict, 409, "the attachment alias or connection lifetime conflicts with the current attachment; rediscover it before deciding recovery"),
    AttachmentPersistenceFailed => ("workdir_attachment_persistence_failed", Dependency, 500, "the Backend attachment ledger update failed; inspect the current link and independent Runtime result before deciding recovery"),
    AttachmentStateOutcomeUnknown => ("workdir_attachment_state_outcome_unknown", Unknown, 503, "the Backend attachment ledger update may have committed; inspect the current attachment and do not automatically resend or compensate"),
    AttachmentProjectionFailed => ("workdir_attachment_projection_failed", Dependency, 502, "the Worker attachment projection could not be confirmed; inspect the Backend attachment and Runtime result separately"),
    AttachmentRuntimeResultUnavailable => ("workdir_attachment_runtime_result_unavailable", Unknown, 502, "the Runtime attachment outcome is unconfirmed and no supported owner cause was observed; inspect current state without automatically resending or compensating"),
    Occupied => ("workdir_occupied", Conflict, 409, "another Worker owns the Workdir attachment; detach through that owner before attaching here"),
    NotFound => ("working_directory_not_found", InvalidInput, 404, "the Runtime Workdir was not found"),
    NotActive => ("working_directory_not_active", Conflict, 400, "the Workdir materialization is not active"),
    CreateFailed => ("working_directory_create_failed", Dependency, 400, "the Runtime could not create the materialization"),
    Exists => ("working_directory_exists", Conflict, 400, "a materialization already occupies the requested identity"),
    GitFailed => ("working_directory_git_failed", Dependency, 400, "the Git command failed"),
    GitUnavailable => ("working_directory_git_unavailable", Dependency, 400, "the Runtime could not execute Git"),
    IdInvalid => ("working_directory_id_invalid", InvalidInput, 400, "the Workdir identity is invalid"),
    ListFailed => ("working_directory_list_failed", Dependency, 400, "the Runtime could not enumerate materializations"),
    MaterializerUnsupported => ("working_directory_materializer_unsupported", InvalidInput, 400, "the requested materializer is unsupported"),
    RecordInvalid => ("working_directory_record_invalid", Conflict, 400, "the materialization record is inconsistent with its identity"),
    RecordSerializeFailed => ("working_directory_record_serialize_failed", Internal, 400, "the Runtime could not encode the materialization record"),
    RecordWriteFailed => ("working_directory_record_write_failed", Dependency, 400, "the Runtime could not save the materialization record"),
    RelativeCwdEscapeRejected => ("working_directory_relative_cwd_escape_rejected", Authorization, 400, "the requested cwd resolves outside the Workdir root"),
    RelativeCwdInvalid => ("working_directory_relative_cwd_invalid", InvalidInput, 400, "the requested cwd is not a relative directory inside the Workdir"),
    RelativeCwdUnavailable => ("working_directory_relative_cwd_unavailable", InvalidInput, 400, "the requested cwd does not identify an existing directory"),
    NonUtf8Path => ("working_directory_non_utf8_path", InvalidInput, 400, "the filesystem path is not valid UTF-8"),
    RootUnavailable => ("working_directory_root_unavailable", Dependency, 400, "the Runtime could not access the materialization root"),
    RemoteRepositoryAccessInvalid => ("working_directory_remote_repository_access_invalid", Authorization, 400, "the supplied remote Repository access is invalid"),
    RemoteRepositoryAccessRequired => ("working_directory_remote_repository_access_required", Authorization, 400, "remote Repository access authority is required"),
    RemoteRepositoryHostTrustMismatch => ("working_directory_remote_repository_host_trust_mismatch", Authorization, 400, "the remote Repository host does not match its trust authority"),
    RepositoryAccessBindingMismatch => ("working_directory_repository_access_binding_mismatch", Authorization, 400, "Repository access is bound to a different source"),
    RepositoryAccessExpired => ("working_directory_repository_access_expired", Authorization, 400, "the operation-scoped Repository access has expired"),
    RepositoryAccessNotApplicable => ("working_directory_repository_access_not_applicable", InvalidInput, 400, "Repository access does not apply to this source"),
    RepositoryAccessSetupFailed => ("working_directory_repository_access_setup_failed", Dependency, 400, "the operation-scoped Repository access could not be prepared"),
    RepositoryAccessUnavailable => ("working_directory_repository_access_unavailable", Dependency, 400, "the Workdir Repository access is unavailable"),
    RepositoryAgentFailed => ("working_directory_repository_agent_failed", Dependency, 400, "the Runtime Repository credential agent could not be prepared"),
    RepositoryAgentUnavailable => ("working_directory_repository_agent_unavailable", Dependency, 400, "the Runtime Repository credential agent is unavailable"),
    RepositoryLimitExceeded => ("working_directory_repository_limit_exceeded", InvalidInput, 400, "the Repository exceeds the Runtime object or storage limit"),
    RepositoryMaterializationAuthorityInvalid => ("working_directory_repository_materialization_authority_invalid", Authorization, 400, "the materialization authority does not match the requested Repository source"),
    RepositoryMaterializationAuthorityRequired => ("working_directory_repository_materialization_authority_required", Authorization, 400, "Repository materialization authority is required"),
    RepositoryPlainHttpUnsupported => ("working_directory_repository_plain_http_unsupported", InvalidInput, 400, "plain HTTP Repository sources are unsupported"),
    RepositoryProviderUnsupported => ("working_directory_repository_provider_unsupported", InvalidInput, 400, "the Repository provider is unsupported"),
    RepositorySelectorInvalid => ("working_directory_repository_selector_invalid", InvalidInput, 400, "the Repository selector is invalid"),
    RepositorySelectorUnresolved => ("working_directory_repository_selector_unresolved", InvalidInput, 400, "the configured Repository selector could not be resolved to a commit"),
    RepositorySourceInvalid => ("working_directory_repository_source_invalid", InvalidInput, 400, "the Repository source is invalid"),
    RepositoryTimeout => ("working_directory_repository_timeout", Dependency, 400, "the Git operation exceeded the Runtime time limit"),
    RepositoryCloneFailed => ("working_directory_repository_clone_failed", Dependency, 400, "the Runtime could not clone the Repository"),
    RepositoryCheckoutFailed => ("working_directory_repository_checkout_failed", Dependency, 400, "the Runtime could not check out the configured Repository selector"),
    RepositoryPolicyFailed => ("working_directory_repository_policy_failed", Dependency, 400, "the Runtime could not apply the Repository access policy"),
    RepositoryAccessDenied => ("working_directory_repository_access_denied", Authorization, 400, "the Git SSH operation was rejected by the Repository access policy"),
    RepositoryHostTrustFailed => ("working_directory_repository_host_trust_failed", Authorization, 400, "Git SSH host key verification failed"),
    RepositoryAuthenticationFailed => ("working_directory_repository_authentication_failed", Authorization, 400, "Git SSH authentication failed for the operation-scoped credentials"),
    RepositoryConnectionFailed => ("working_directory_repository_connection_failed", Dependency, 400, "the Git SSH connection to the Repository host failed"),
    RepositoryNotFound => ("working_directory_repository_not_found", InvalidInput, 400, "the Repository was not found or is not accessible"),
    RefNotFound => ("repository_ref_not_found", InvalidInput, 404, "the Repository provider did not return the requested ref"),
    RefProviderTimeout => ("repository_ref_provider_timeout", Dependency, 503, "the Repository ref provider exceeded the Runtime time limit"),
    RefProviderUnavailable => ("repository_ref_provider_unavailable", Dependency, 503, "the Repository ref provider is unavailable"),
    RefProviderAuthFailed => ("repository_ref_provider_auth_failed", Authorization, 403, "the Repository ref provider rejected operation-scoped authentication"),
    RefResponseInvalid => ("repository_ref_response_invalid", Dependency, 400, "the Repository ref response was invalid or ambiguous"),
    RefSelectorInvalid => ("repository_ref_selector_invalid", InvalidInput, 400, "the requested Repository ref selector is invalid"),
    CleanupMountPresent => ("working_directory_cleanup_mount_present", Conflict, 409, "a mounted resource remains; its owner must release it before normal removal"),
    CleanupMountCheckUnavailable => ("working_directory_cleanup_mount_check_unavailable", Dependency, 503, "mount-safe removal support is unavailable in the Runtime"),
    CleanupPermissionDenied => ("working_directory_cleanup_permission_denied", Authorization, 403, "filesystem permissions denied removal"),
    CleanupResourceBusy => ("working_directory_cleanup_resource_busy", Conflict, 409, "a filesystem resource is in use or changed during removal"),
    CleanupStorageUnavailable => ("working_directory_cleanup_storage_unavailable", Dependency, 503, "cleanup storage is unavailable"),
    CleanupOwnershipUnknown => ("working_directory_cleanup_ownership_unknown", Conflict, 409, "the remaining directory cannot be matched to Runtime removal authority"),
    CleanupChangesPresent => ("working_directory_cleanup_changes_present", Conflict, 409, "new or changed content remains and must be preserved or resolved before normal removal"),
    CleanupChangesUnknown => ("working_directory_cleanup_changes_unknown", Conflict, 409, "Remaining entries cannot be authorized from saved entry metadata and a usable clean checkout. Preserve residual content outside this Workdir and restore the same usable checkout, or ask its owner to remove verified residual content before retrying. Do not edit removal metadata."),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkdirManagementOperation {
    Create,
    List,
    Detail,
    Cleanup,
    RepositoryAccess,
    RepositoryRef,
    Attach,
    Detach,
    GrantCreate,
    GrantInspect,
    GrantRevoke,
    GrantConnect,
    GrantCleanup,
}
impl WorkdirManagementOperation {
    fn description(self) -> &'static str {
        match self {
            Self::Create => "Workdir creation",
            Self::Attach => "Workdir attachment",
            Self::Detach => "Workdir detachment",
            Self::GrantCreate => "External Workdir grant creation",
            Self::GrantInspect => "External Workdir grant inspection",
            Self::GrantRevoke => "External Workdir grant revocation",
            Self::GrantConnect => "External Workdir provider connection",
            Self::GrantCleanup => "External Workdir grant cleanup",
            Self::List => "Workdir inventory",
            Self::Detail => "Workdir lookup",
            Self::Cleanup => "Workdir removal",
            Self::RepositoryAccess => "Workdir Repository access",
            Self::RepositoryRef => "Repository ref observation",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkdirManagementFailure {
    pub operation: WorkdirManagementOperation,
    pub reason: WorkdirManagementReason,
}
impl FailureCause for WorkdirManagementFailure {
    fn identity(&self) -> FailureIdentity {
        FailureIdentity {
            domain: "workdir_management",
            reason: self.reason.as_str(),
            category: self.reason.category(),
            details_version: 1,
        }
    }
    fn validate(&self) -> Result<(), ContractError> {
        Ok(())
    }
}
impl WireDetails for WorkdirManagementFailure {
    fn supports(domain: &str, reason: &str, version: u16) -> bool {
        domain == "workdir_management"
            && version == 1
            && WorkdirManagementReason::ALL
                .iter()
                .any(|value| value.as_str() == reason)
    }
}
pub struct WorkdirManagementVisibility {
    pub can_view_cause: bool,
}
macro_rules! project {
    ($($audience:ty),+ $(,)?) => {$ (
        impl ProjectFor<$audience> for WorkdirManagementFailure {
            type Authorization = WorkdirManagementVisibility;
            type Details = Self;
            fn disclose(&self, facts: &FailureFacts<Self>, visibility: &Self::Authorization) -> Result<Disclosure<Self>, ContractError> {
                if !visibility.can_view_cause { return Err(ContractError::ProjectionUnavailable); }
                if let Some(disclosure) = self.inventory_disclosure(facts)? { return Ok(disclosure); }
                Ok(Disclosure::new(*self, format!("{} failed: {}", self.operation.description(), self.reason.explanation())))
            }
        }
    )+};
}
project!(Browser, Tool, InternalTransfer, Storage, Telemetry);

impl WorkdirManagementFailure {
    // Inventory references are domain-owned public IDs, not arbitrary received context.
    // Reconstruct only these fields; never forward correlation, prose or provider paths.
    fn inventory_disclosure(
        &self,
        facts: &FailureFacts<Self>,
    ) -> Result<Option<Disclosure<Self>>, ContractError> {
        if self.operation != WorkdirManagementOperation::List
            || self.reason != WorkdirManagementReason::LegacyExternalGrantPermissions
        {
            return Ok(None);
        }
        let [workdir, grant] = facts.context().operations.as_slice() else {
            return Ok(None);
        };
        if workdir.operation != "workdir.inventory"
            || grant.operation != "workdir.grant"
            || workdir.stage != "legacy_permissions"
            || grant.stage != "legacy_permissions"
        {
            return Ok(None);
        }
        let valid = |id: &str| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        };
        let (Some(workdir_id), Some(grant_id)) = (&workdir.operation_ref, &grant.operation_ref)
        else {
            return Ok(None);
        };
        if !valid(workdir_id) || !valid(grant_id) {
            return Ok(None);
        }
        let mut disclosure = Disclosure::new(
            *self,
            format!(
                "Workdir {workdir_id} was omitted because External grant {grant_id} uses legacy non-hierarchical permissions; revoke the old grant and share again with READ, WRITE, or COMMAND",
            ),
        );
        for (operation, reference) in [
            ("workdir.inventory", workdir_id),
            ("workdir.grant", grant_id),
        ] {
            let mut context = OperationContext::new(operation, "legacy_permissions")?;
            context.operation_ref = Some(reference.clone());
            disclosure.context.operations.push(context);
        }
        Ok(Some(disclosure))
    }
}
