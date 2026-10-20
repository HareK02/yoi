//! Neutral explicit-allow authorization contracts. Resource containment never crosses kinds.
//!
//! # Trusted Feature boundary
//!
//! A Profile requests execution configuration, not authority. The Worker asks the
//! Host for each enabled Feature's exact activation name before initialization,
//! then trusted Feature code asks for each protected operation and its semantic
//! resource before access or effects. `ticket` on [`PermissionResource::Feature`]
//! activates Ticket; it implies neither `ticket.create` nor any future operation.
//! `ticket.create` on a Workspace permits creation there but cannot activate the
//! Feature. There are no wildcard permissions or implicit dotted-name inheritance.
//!
//! Identity is absent from the authorization request intentionally: the Host
//! authenticates the bound Runtime/Worker and resolves current source permissions,
//! body ownership and any per-Worker execution grant. A shared client, Profile,
//! remembered receipt or caller-supplied Subject claim is not authority. Features
//! depend on this contract, never on a particular Subject policy implementation.
//!
//! Supported cooperative boundaries are Feature activation, ordinary Worker
//! delegation, Ticket creation (Tool/native WIP/typed service), Workdir filesystem
//! read/write and whole-attachment command execution, and scoped result notify.
//! Workdir providers still enforce resolved paths, traversal, capability, Ticket
//! and provider ceilings. The returned scope is for provider narrowing, not a
//! transferable credential. Scoped checkout/overlap operations are refused until
//! their complete access semantics are adapted. Other Feature operations retain
//! their existing owner checks; they do not automatically acquire this operation
//! policy. Missing calls are trusted Feature bugs, not interception by a universal
//! Tool executor or protection against malicious Feature code. Connection tests
//! must exercise each exposed surface, assert a genuine permitted effect and
//! assert zero downstream calls on denial, including revocation and provider denial.
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

pub const MAX_SCOPED_PERMISSIONS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionResource {
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
    pub fn validate(&self) -> Result<(), String> {
        match self {
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
    /// `None` identifies the explicit non-Subject legacy authorization path.
    #[serde(default)]
    pub scope: Option<PermissionResource>,
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
