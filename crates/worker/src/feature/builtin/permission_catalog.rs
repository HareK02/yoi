//! Server-side registration of the compiled builtin operation declarations.
//!
//! This is a trusted builtin loader, not a wire-catalog importer. Namespace
//! ownership is listed here explicitly; neither a permission spelling nor a
//! management request chooses an owner. The existing operation descriptors own
//! exact names. No dotted-prefix children are inferred. Profiles/provider scopes
//! and actual operation authorization remain independent of this metadata load.

use super::super::FeatureInstallError;
use feature_permissions::{
    FeatureIdentity, NamespaceDeclaration, NamespaceId, NamespacePermissions, PermissionDefinition,
    PermissionHost, PermissionKey, PermissionReference, PermissionSetId, SetContribution,
    WorkspaceId,
};
use std::collections::BTreeMap;

const OWNERS: &[(&str, &str)] = &[
    ("filesystem", "builtin:workdir"),
    ("bash", "builtin:bash"),
    ("workdir", "builtin:workdir"),
    ("worker", "builtin:worker"),
    ("subworker", "builtin:sub-worker-control"),
    ("ticket", "builtin:ticket"),
    ("objective", "builtin:objective"),
    ("merge_request", "builtin:merge-request"),
    ("repository", "builtin:repository"),
    ("drive", "builtin:drive"),
    ("workspace_config", "builtin:workspace-config"),
    ("memory", "builtin:memory"),
    ("journal", "builtin:journal-recording"),
    ("chat", "builtin:subjektiv-chat"),
    ("session", "builtin:session-explore"),
    ("web", "builtin:web"),
    ("notification", "builtin:notification"),
];

pub fn builtin_permission_host(
    workspace: WorkspaceId,
) -> Result<PermissionHost, FeatureInstallError> {
    let invalid = |e: &dyn std::fmt::Display| FeatureInstallError::InvalidDescriptor(e.to_string());
    let mut host = PermissionHost::new(workspace);
    let mut features = BTreeMap::new();
    // MCP's scalar startup permission is the retained T-787 exception, not a
    // payload-free operation key. Do not invent mcp.* authority or a Set for it.
    // Legacy migration retains it as an explicit-configuration-required diagnostic.
    let operations = server_api::authorization::subject_permission_operations()
        .into_iter()
        .filter(|operation| operation.operation != "mcp")
        .collect::<Vec<_>>();
    for (namespace, owner) in OWNERS {
        let feature = match features.get(owner) {
            Some(feature) => feature,
            None => {
                let feature = host
                    .register_feature(FeatureIdentity::parse(*owner).map_err(|e| invalid(&e))?)
                    .map_err(|e| invalid(&e))?;
                features.entry(owner).or_insert(feature)
            }
        };
        let id = NamespaceId::parse(*namespace).map_err(|e| invalid(&e))?;
        let handle = host
            .bind_namespace(feature, id.clone())
            .map_err(|e| invalid(&e))?;
        let permissions = operations
            .iter()
            .filter(|operation| operation.operation.split('.').next() == Some(namespace))
            .map(|operation| {
                Ok(PermissionDefinition {
                    key: PermissionKey::parse(operation.operation.clone())
                        .map_err(|e| invalid(&e))?,
                    description: operation.description.clone(),
                    children: vec![],
                })
            })
            .collect::<Result<Vec<_>, FeatureInstallError>>()?;
        let keys = permissions
            .iter()
            .map(|definition| definition.key.clone())
            .collect();
        host.declare(
            &handle,
            NamespaceDeclaration {
                permissions,
                sets: vec![SetContribution {
                    set: PermissionSetId::parse(*namespace).map_err(|e| invalid(&e))?,
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: id,
                        keys,
                    })],
                }],
            },
        )
        .map_err(|e| invalid(&e))?;
    }
    // A new builtin operation needs an intentional namespace ownership decision.
    for operation in operations {
        if !OWNERS
            .iter()
            .any(|(namespace, _)| operation.operation.split('.').next() == Some(*namespace))
        {
            return Err(FeatureInstallError::InvalidDescriptor(format!(
                "builtin operation has no namespace owner: {}",
                operation.operation
            )));
        }
    }
    Ok(host)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn builtin_keys_are_complete_exact_and_have_no_inferred_children() {
        let host = builtin_permission_host(WorkspaceId::parse("workspace-a").unwrap()).unwrap();
        let catalog = host.catalog();
        let mut keys = std::collections::BTreeSet::new();
        for namespace in catalog.namespaces {
            for permission in namespace.declaration.permissions {
                assert!(permission.children.is_empty());
                assert!(keys.insert(permission.key.as_str().to_owned()));
            }
        }
        assert!(!keys.contains("mcp"));
        assert_eq!(
            keys,
            server_api::authorization::subject_permission_operations()
                .into_iter()
                .filter(|operation| operation.operation != "mcp")
                .map(|o| o.operation)
                .collect()
        );
    }
}
