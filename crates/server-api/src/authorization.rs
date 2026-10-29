//! Neutral explicit-allow authorization contracts. Resource containment never crosses kinds.
//!
//! # Trusted operation boundary
//!
//! A Profile requests local composition, not authority. Feature registration and
//! session-local state need no authorization. Trusted Feature/Host adapters check
//! each protected read, model supply, subscription or effect immediately before
//! it occurs; continued background work repeats that check. MCP alone retains
//! temporary exact activation permission on [`PermissionResource::Feature`]
//! before discovery can spawn a process.
//!
//! Identity is absent from requests: the Host authenticates the bound Runtime/Worker
//! and resolves current source policy, body ownership, delegation and Job attenuation.
//! Bound target queries resolve existing connections, never caller-supplied source
//! IDs. A remembered receipt, Profile or shared client is not authority; transport
//! failure never means standalone.
//!
//! Workspace permissions cover collection operations only, not individual resources
//! of another kind. Exact names never imply dotted descendants. Legacy Feature and
//! Workspace grants retain their wire representation and exact meaning; reading saved
//! policy/grants does not issue permissions for newly protected operations/resources.
//! Non-MCP Feature grants remain inert; no schema rewrite or baseline grants are needed.
//!
//! Authorization composes with provider checks: ownership, resolved paths, capabilities,
//! Ticket actor conditions, observation/CAS, review capability, immutable Job batch and
//! attempt/writer fences remain mandatory. Queries use identity metadata, not protected
//! bodies, and do not certify existence. Missing targets are still refused by providers.
//! Connection tests observe permitted real work and zero downstream work on refusal,
//! including revocation. Missing calls are trusted Feature bugs, not interception by a
//! universal Tool executor or a sandbox for malicious Feature code.
//!
//! # Delegation callers and recovery
//!
//! Callers such as a chat Feature supply explicit operation/resource selections
//! and retain the same public operation ID and full request after an unknown
//! result. Normal Profile/Worker allocation and control/resource grants are reused;
//! there is no Subject Worker registry. Effective grants bind immutable Profile
//! resolution, the reserved Worker and source assignment. Permission replacement
//! invalidates existing children rather than refreshing them. Body replacement,
//! retirement and restore must recheck current authority; historical result lookup
//! is not new execution permission. Cancellation acceptance is distinct from
//! terminal confirmation and does not undo completed effects.
//!
//! A child receives no Subject body identity, singleton, whole Memory or public
//! conversation-post authority. `worker.result` is explicit notify authority to a
//! named origin Worker, not arbitrary control or publication. Chat delivery and
//! conversation correlation belong to the caller integration, not this contract.
//! Unset existing Subject policies deny new Subject execution/delegation; ordinary
//! non-Subject Workers explicitly retain existing Backend/provider contracts.
//! Standalone non-Workspace activation compatibility grants no Workspace operation.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

mod catalog;
pub use catalog::*;

pub const MAX_SCOPED_PERMISSIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionResource {
    Job {
        workspace_id: String,
        job_id: String,
    },
    Ticket {
        workspace_id: String,
        ticket_id: String,
    },
    Objective {
        workspace_id: String,
        objective_id: String,
    },
    MergeRequest {
        workspace_id: String,
        merge_request_id: String,
    },
    Subject {
        workspace_id: String,
        subject_id: String,
    },
    DriveEntry {
        workspace_id: String,
        node_id: String,
    },
    DriveRequest {
        workspace_id: String,
        request_id: String,
    },
    Repository {
        workspace_id: String,
        repository_key: String,
    },
    WorkdirAttachment {
        workspace_id: String,
        connection_id: String,
    },
    WorkspaceConfig {
        workspace_id: String,
        path: String,
        recursive: bool,
    },
    Network {
        url: String,
    },
    SubWorker {
        runtime_id: String,
        worker_id: String,
        name: String,
    },
    Feature,
    Workspace {
        workspace_id: String,
    },
    Workdir {
        working_directory_id: String,
        path: String,
        recursive: bool,
    },
    Worker {
        runtime_id: String,
        worker_id: String,
    },
}

// Serde's internally tagged unit variants ignore extra fields even with deny_unknown_fields.
// A struct-shaped wire variant preserves the public unit variant while rejecting those claims.
impl<'de> Deserialize<'de> for PermissionResource {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Job {
                workspace_id: String,
                job_id: String,
            },
            Ticket {
                workspace_id: String,
                ticket_id: String,
            },
            Objective {
                workspace_id: String,
                objective_id: String,
            },
            MergeRequest {
                workspace_id: String,
                merge_request_id: String,
            },
            Subject {
                workspace_id: String,
                subject_id: String,
            },
            DriveEntry {
                workspace_id: String,
                node_id: String,
            },
            DriveRequest {
                workspace_id: String,
                request_id: String,
            },
            Repository {
                workspace_id: String,
                repository_key: String,
            },
            WorkdirAttachment {
                workspace_id: String,
                connection_id: String,
            },
            WorkspaceConfig {
                workspace_id: String,
                path: String,
                recursive: bool,
            },
            Network {
                url: String,
            },
            SubWorker {
                runtime_id: String,
                worker_id: String,
                name: String,
            },
            Feature {},
            Workspace {
                workspace_id: String,
            },
            Workdir {
                working_directory_id: String,
                path: String,
                recursive: bool,
            },
            Worker {
                runtime_id: String,
                worker_id: String,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Job {
                workspace_id,
                job_id,
            } => Self::Job {
                workspace_id,
                job_id,
            },
            Wire::Ticket {
                workspace_id,
                ticket_id,
            } => Self::Ticket {
                workspace_id,
                ticket_id,
            },
            Wire::Objective {
                workspace_id,
                objective_id,
            } => Self::Objective {
                workspace_id,
                objective_id,
            },
            Wire::MergeRequest {
                workspace_id,
                merge_request_id,
            } => Self::MergeRequest {
                workspace_id,
                merge_request_id,
            },
            Wire::Subject {
                workspace_id,
                subject_id,
            } => Self::Subject {
                workspace_id,
                subject_id,
            },
            Wire::DriveEntry {
                workspace_id,
                node_id,
            } => Self::DriveEntry {
                workspace_id,
                node_id,
            },
            Wire::DriveRequest {
                workspace_id,
                request_id,
            } => Self::DriveRequest {
                workspace_id,
                request_id,
            },
            Wire::Repository {
                workspace_id,
                repository_key,
            } => Self::Repository {
                workspace_id,
                repository_key,
            },
            Wire::WorkdirAttachment {
                workspace_id,
                connection_id,
            } => Self::WorkdirAttachment {
                workspace_id,
                connection_id,
            },
            Wire::WorkspaceConfig {
                workspace_id,
                path,
                recursive,
            } => Self::WorkspaceConfig {
                workspace_id,
                path,
                recursive,
            },
            Wire::Network { url } => Self::Network { url },
            Wire::SubWorker {
                runtime_id,
                worker_id,
                name,
            } => Self::SubWorker {
                runtime_id,
                worker_id,
                name,
            },
            Wire::Feature {} => Self::Feature,
            Wire::Workspace { workspace_id } => Self::Workspace { workspace_id },
            Wire::Workdir {
                working_directory_id,
                path,
                recursive,
            } => Self::Workdir {
                working_directory_id,
                path,
                recursive,
            },
            Wire::Worker {
                runtime_id,
                worker_id,
            } => Self::Worker {
                runtime_id,
                worker_id,
            },
        })
    }
}

impl PermissionResource {
    /// Checks Workspace membership without a target existence or data lookup.
    pub fn workspace_id(&self) -> Option<&str> {
        match self {
            Self::Job { workspace_id, .. }
            | Self::Ticket { workspace_id, .. }
            | Self::Objective { workspace_id, .. }
            | Self::MergeRequest { workspace_id, .. }
            | Self::Subject { workspace_id, .. }
            | Self::DriveEntry { workspace_id, .. }
            | Self::DriveRequest { workspace_id, .. }
            | Self::Repository { workspace_id, .. }
            | Self::WorkdirAttachment { workspace_id, .. }
            | Self::WorkspaceConfig { workspace_id, .. }
            | Self::Workspace { workspace_id } => Some(workspace_id),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Job {
                workspace_id,
                job_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(job_id)
            }
            Self::Ticket {
                workspace_id,
                ticket_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(ticket_id)
            }
            Self::Objective {
                workspace_id,
                objective_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(objective_id)
            }
            Self::MergeRequest {
                workspace_id,
                merge_request_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(merge_request_id)
            }
            Self::Subject {
                workspace_id,
                subject_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(subject_id)
            }
            Self::DriveEntry {
                workspace_id,
                node_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(node_id)
            }
            Self::DriveRequest {
                workspace_id,
                request_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(request_id)
            }
            Self::Repository {
                workspace_id,
                repository_key,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(repository_key)
            }
            Self::WorkdirAttachment {
                workspace_id,
                connection_id,
            } => {
                validate_authorization_identifier(workspace_id)?;
                validate_authorization_identifier(connection_id)
            }
            Self::WorkspaceConfig {
                workspace_id, path, ..
            } => {
                validate_authorization_identifier(workspace_id)?;
                if path.is_empty() {
                    Ok(())
                } else {
                    validate_relative_permission_path(path)
                }
            }
            Self::Network { url } => {
                let parsed = url::Url::parse(url)
                    .map_err(|_| "network resource must be an absolute HTTP(S) URL")?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || url.len() > 8192
                    || url.chars().any(char::is_control)
                {
                    return Err("network resource must be a bounded absolute HTTP(S) URL without credentials".into());
                }
                Ok(())
            }
            Self::SubWorker {
                runtime_id,
                worker_id,
                name,
            } => {
                validate_authorization_identifier(runtime_id)?;
                validate_authorization_identifier(worker_id)?;
                validate_authorization_identifier(name)
            }
            Self::Feature => Ok(()),
            Self::Workspace { workspace_id } => validate_authorization_identifier(workspace_id),
            Self::Workdir {
                working_directory_id,
                path,
                ..
            } => {
                validate_authorization_identifier(working_directory_id)?;
                validate_relative_permission_path(path)
            }
            Self::Worker {
                runtime_id,
                worker_id,
            } => {
                validate_authorization_identifier(runtime_id)?;
                validate_authorization_identifier(worker_id)
            }
        }
    }

    pub fn covers(&self, other: &Self) -> bool {
        if self.validate().is_err() || other.validate().is_err() {
            return false;
        }
        match (self, other) {
            (
                Self::Job {
                    workspace_id: a,
                    job_id: ai,
                },
                Self::Job {
                    workspace_id: b,
                    job_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::Ticket {
                    workspace_id: a,
                    ticket_id: ai,
                },
                Self::Ticket {
                    workspace_id: b,
                    ticket_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::Objective {
                    workspace_id: a,
                    objective_id: ai,
                },
                Self::Objective {
                    workspace_id: b,
                    objective_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::MergeRequest {
                    workspace_id: a,
                    merge_request_id: ai,
                },
                Self::MergeRequest {
                    workspace_id: b,
                    merge_request_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::Subject {
                    workspace_id: a,
                    subject_id: ai,
                },
                Self::Subject {
                    workspace_id: b,
                    subject_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::DriveEntry {
                    workspace_id: a,
                    node_id: ai,
                },
                Self::DriveEntry {
                    workspace_id: b,
                    node_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::DriveRequest {
                    workspace_id: a,
                    request_id: ai,
                },
                Self::DriveRequest {
                    workspace_id: b,
                    request_id: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::Repository {
                    workspace_id: a,
                    repository_key: ai,
                },
                Self::Repository {
                    workspace_id: b,
                    repository_key: bi,
                },
            ) => a == b && ai == bi,
            (
                Self::WorkdirAttachment {
                    workspace_id: a,
                    connection_id: ai,
                },
                Self::WorkdirAttachment {
                    workspace_id: b,
                    connection_id: bi,
                },
            ) => a == b && ai == bi,
            (Self::Network { url: a }, Self::Network { url: b }) => a == b,
            (
                Self::SubWorker {
                    runtime_id: a,
                    worker_id: aw,
                    name: an,
                },
                Self::SubWorker {
                    runtime_id: b,
                    worker_id: bw,
                    name: bn,
                },
            ) => a == b && aw == bw && an == bn,
            (
                Self::WorkspaceConfig {
                    workspace_id: a,
                    path: ap,
                    recursive: ar,
                },
                Self::WorkspaceConfig {
                    workspace_id: b,
                    path: bp,
                    recursive: br,
                },
            ) => {
                a == b
                    && if *ar {
                        ap.is_empty()
                            || ap == "."
                            || ap == bp
                            || bp
                                .strip_prefix(ap)
                                .is_some_and(|tail| tail.starts_with("/"))
                    } else {
                        ap == bp && !br
                    }
            }
            (Self::Feature, Self::Feature) => true,
            (Self::Workspace { workspace_id: a }, Self::Workspace { workspace_id: b }) => a == b,
            (
                Self::Worker {
                    runtime_id: ar,
                    worker_id: aw,
                },
                Self::Worker {
                    runtime_id: br,
                    worker_id: bw,
                },
            ) => ar == br && aw == bw,
            (
                Self::Workdir {
                    working_directory_id: a,
                    path: ap,
                    recursive: ar,
                },
                Self::Workdir {
                    working_directory_id: b,
                    path: bp,
                    recursive: br,
                },
            ) => {
                a == b
                    && if *ar {
                        ap == "."
                            || ap == bp
                            || bp
                                .strip_prefix(ap)
                                .is_some_and(|tail| tail.starts_with('/'))
                    } else {
                        ap == bp && !br
                    }
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ScopedPermission {
    pub operation: String,
    pub resource: PermissionResource,
}

impl ScopedPermission {
    pub fn validate(&self) -> Result<(), String> {
        validate_operation_name(&self.operation)?;
        self.resource.validate()?;
        if matches!(self.resource, PermissionResource::Feature) && self.operation.contains('.') {
            return Err("Feature resources authorize activation names only, not operations".into());
        }
        Ok(())
    }

    pub fn covers(&self, other: &Self) -> bool {
        self.validate().is_ok()
            && other.validate().is_ok()
            && self.operation == other.operation
            && self.resource.covers(&other.resource)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SubjectPermissions {
    pub change_id: String,
    pub execution: Vec<ScopedPermission>,
    pub delegation: Vec<ScopedPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SubjectPermissionsUpdate {
    pub operation_id: String,
    pub expected_change_id: Option<String>,
    pub execution: Vec<ScopedPermission>,
    pub delegation: Vec<ScopedPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerExecutionGrant {
    pub subject_id: String,
    pub origin_runtime_id: String,
    pub origin_worker_id: String,
    pub source_change_id: String,
    pub runtime_id: String,
    pub worker_id: String,
    pub profile: String,
    pub profile_digest: String,
    pub permissions: Vec<ScopedPermission>,
    pub redelegation: Vec<ScopedPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct WorkerDelegationRequest {
    pub permissions: Vec<ScopedPermission>,
    pub redelegation: Vec<ScopedPermission>,
}

impl WorkerDelegationRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_scoped_permissions(&self.permissions)?;
        validate_scoped_permissions(&self.redelegation)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct OperationAuthorizationRequest {
    pub operation: String,
    pub resource: PermissionResource,
}

impl OperationAuthorizationRequest {
    pub fn validate(&self) -> Result<(), String> {
        ScopedPermission {
            operation: self.operation.clone(),
            resource: self.resource.clone(),
        }
        .validate()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct OperationAuthorizationResponse {
    /// Matching granted scope for provider-resolved checks, not a credential.
    /// Ordinary checks return `None` on the non-Subject legacy path. Bound
    /// metadata lookups can still return the concrete target for consistency checks.
    #[serde(default)]
    pub scope: Option<PermissionResource>,
}

/// Selects existing Host metadata; caller-supplied selectors are not ownership proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum BoundAuthorizationTarget {
    Subject {},
    WorkdirAttachment {
        alias: String,
        expected_connection_id: Option<String>,
    },
    SubWorker {
        name: String,
    },
    NotificationSource {
        source_event_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct BoundOperationAuthorizationRequest {
    pub operation: String,
    pub target: BoundAuthorizationTarget,
}

pub fn validate_operation_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || !value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        })
    {
        return Err(
            "permission operation must be an exact, nonempty dot-separated name (no wildcards)"
                .into(),
        );
    }
    Ok(())
}

pub fn validate_scoped_permissions(permissions: &[ScopedPermission]) -> Result<(), String> {
    if permissions.len() > MAX_SCOPED_PERMISSIONS {
        return Err("too many scoped permissions".into());
    }
    for permission in permissions {
        permission.validate()?;
    }
    Ok(())
}

pub fn validate_authorization_identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(|c| c.is_control() || c == '*')
    {
        return Err("authorization identity must be nonempty, bounded and exact".into());
    }
    Ok(())
}

/// Paths are already-normalized lexical paths, not host filesystem paths. The Host must
/// still enforce symlink/provider boundaries; lexical containment grants no filesystem access.
pub fn validate_relative_permission_path(path: &str) -> Result<(), String> {
    if path == "." {
        return Ok(());
    }
    if path.is_empty()
        || path.len() > 4096
        || path.contains(['\\', ':'])
        || path.chars().any(char::is_control)
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("permission path must be a normalized relative path without escape".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir(path: &str, recursive: bool) -> PermissionResource {
        PermissionResource::Workdir {
            working_directory_id: "wd".into(),
            path: path.into(),
            recursive,
        }
    }

    #[test]
    fn lexical_containment_rejects_escape_and_respects_component_and_recursion_boundaries() {
        for path in [
            "",
            "/src",
            "../src",
            "src/../secret",
            "src/./a",
            "src//a",
            "src/",
            "C:/src",
            "src\\a",
        ] {
            assert!(workdir(path, true).validate().is_err(), "{path}");
        }
        let root = workdir(".", true);
        let src = workdir("src", true);
        assert!(root.covers(&src));
        assert!(src.covers(&workdir("src/a", true)));
        assert!(!src.covers(&workdir("src-other", false)));
        assert!(!workdir("src", false).covers(&src));
        assert!(workdir("src", false).covers(&workdir("src", false)));
        assert!(!workdir("src", false).covers(&workdir("src/a", false)));
        assert!(!src.covers(&PermissionResource::Workdir {
            working_directory_id: "other".into(),
            path: "src".into(),
            recursive: false
        }));
    }

    #[test]
    fn operation_hierarchy_never_implies_inheritance_or_feature_activation() {
        for name in [
            "",
            "*",
            "ticket.*",
            ".ticket",
            "ticket.",
            "ticket..read",
            "ticket read",
        ] {
            assert!(validate_operation_name(name).is_err(), "{name}");
        }
        let activation = ScopedPermission {
            operation: "ticket".into(),
            resource: PermissionResource::Feature,
        };
        let create = ScopedPermission {
            operation: "ticket.create".into(),
            resource: PermissionResource::Workspace {
                workspace_id: "ws".into(),
            },
        };
        let parent_name = ScopedPermission {
            operation: "ticket".into(),
            resource: create.resource.clone(),
        };
        assert!(!parent_name.covers(&create));
        assert!(!create.covers(&parent_name));
        assert!(!activation.covers(&create));
        assert!(!create.covers(&activation));
        assert!(
            ScopedPermission {
                operation: "ticket.create".into(),
                resource: PermissionResource::Feature
            }
            .validate()
            .is_err()
        );
        assert!(
            !PermissionResource::Workspace {
                workspace_id: "ws".into()
            }
            .covers(&workdir(".", true))
        );
    }

    #[test]
    fn wire_contract_rejects_self_claimed_identity_and_unknown_fields() {
        let request = serde_json::json!({"permissions":[],"redelegation":[],"subject_id":"forged"});
        assert!(serde_json::from_value::<WorkerDelegationRequest>(request).is_err());
        let resource: PermissionResource =
            serde_json::from_value(serde_json::json!({"kind":"feature"})).unwrap();
        assert_eq!(resource, PermissionResource::Feature);
        assert!(
            serde_json::from_value::<PermissionResource>(
                serde_json::json!({"kind":"feature","workspace_id":"ws"})
            )
            .is_err()
        );
    }

    #[test]
    fn authorization_response_carries_matching_scope_and_defaults_legacy_absence() {
        let legacy: OperationAuthorizationResponse =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(legacy.scope, None);
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::json!({"scope":null})
        );
        let granted = OperationAuthorizationResponse {
            scope: Some(workdir("src", true)),
        };
        let wire = serde_json::to_value(&granted).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({"scope":{"kind":"workdir","working_directory_id":"wd","path":"src","recursive":true}})
        );
        assert_eq!(
            serde_json::from_value::<OperationAuthorizationResponse>(wire).unwrap(),
            granted
        );
    }
}
