//! Trusted loader for adopted Plugin declarations. Upload metadata and saved plan
//! grants are never registration authority. Only current owner-adopted config pins
//! joined to live installations/artifacts select declarations from verified bytes.
use super::*;
use ::feature_permissions::{
    NamespaceDeclaration, NamespaceId, NamespacePermissions, PermissionDefinition, PermissionKey,
    PermissionReference, PermissionSetId, SetContribution,
};
use std::collections::BTreeMap;
use worker::feature::{
    FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureInstallError, FeatureModule,
    permissions::{FeaturePermissionRegistrar, FeaturePermissionRegistry},
};

const LOADER_PREFIX: &str = "plugin-catalog:";
const HOST_FEATURE: &str = "builtin:plugin-host";

type Declarations = BTreeMap<NamespaceId, NamespaceDeclaration>;

pub(crate) fn plugin_feature_id(installation: &str, digest: &str) -> Result<FeatureId> {
    FeatureId::new(format!("{LOADER_PREFIX}{installation}:{digest}")).map_err(invalid_permission)
}

struct TrustedPluginPermissions {
    id: FeatureId,
    declarations: Declarations,
}
impl FeatureModule for TrustedPluginPermissions {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut descriptor = FeatureDescriptor::builtin("plugin-host", "Plugin Host");
        descriptor.id = self.id.clone();
        descriptor
    }
    fn declare_permissions(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> std::result::Result<(), FeatureInstallError> {
        let mut handles = BTreeMap::new();
        for namespace in self.declarations.keys() {
            handles.insert(namespace.clone(), registrar.namespace(namespace.clone())?);
        }
        // Seed definitions before edges to permit multiple namespaces in either
        // lexical order. The full candidate still passes the common graph checks.
        for (namespace, declaration) in &self.declarations {
            let mut names = declaration.clone();
            for permission in &mut names.permissions {
                permission.children.clear();
            }
            for set in &mut names.sets {
                set.children.clear();
            }
            registrar.declare(&handles[namespace], names)?;
        }
        for (namespace, declaration) in &self.declarations {
            registrar.declare(&handles[namespace], declaration.clone())?;
        }
        Ok(())
    }
    fn install(
        &self,
        _: &mut FeatureInstallContext<'_>,
    ) -> std::result::Result<(), FeatureInstallError> {
        Ok(())
    }
}
fn invalid_permission(error: impl std::fmt::Display) -> Error {
    Error::Config(format!("Plugin permission declaration: {error}"))
}

/// The Host service owns these operations, not the guest Plugin that calls it.
/// Domain restrictions and immutable plan ceilings remain enforced by the callee.
fn host_module() -> TrustedPluginPermissions {
    let namespace = NamespaceId::parse("plugin").expect("static namespace");
    let keys: Vec<_> = [
        "plugin.admit",
        "plugin.artifact.read",
        "plugin.service.consume",
        "plugin.log",
        "plugin.publish",
        "plugin.connection.open",
        "plugin.http.send",
    ]
    .into_iter()
    .map(|key| PermissionKey::parse(key).expect("static key"))
    .collect();
    TrustedPluginPermissions {
        id: FeatureId::new(HOST_FEATURE).expect("static Feature"),
        declarations: BTreeMap::from([(
            namespace.clone(),
            NamespaceDeclaration {
                permissions: keys
                    .iter()
                    .map(|key| PermissionDefinition {
                        key: key.clone(),
                        description: format!("Plugin Host operation {key}"),
                        children: vec![],
                    })
                    .collect(),
                sets: ["plugin", "companion", "implementer"]
                    .into_iter()
                    .map(|set| SetContribution {
                        set: PermissionSetId::parse(set).expect("static set"),
                        children: vec![PermissionReference::Namespace(NamespacePermissions {
                            namespace: namespace.clone(),
                            keys: keys.clone(),
                        })],
                    })
                    .collect(),
            },
        )]),
    }
}

/// Reconcile only loader-owned registrations; unrelated Feature contributions and
/// retained namespace handles are untouched. Replaced artifacts get fresh handles
/// so an old limited child/Job grant cannot gain a replacement's declarations.
/// Failure withdraws stale loader contributions before returning an error, never
/// falls back to the previous, more permissive declaration.
pub(crate) fn reconcile(
    registry: &mut FeaturePermissionRegistry,
    workspace: &str,
    declarations: BTreeMap<String, Declarations>,
) -> Result<()> {
    let catalog = registry.host().catalog();
    if catalog.workspace.as_str() != workspace {
        return Err(invalid_permission("registry belongs to another Workspace"));
    }
    let host_id = FeatureId::new(HOST_FEATURE).map_err(invalid_permission)?;
    if !catalog
        .namespaces
        .iter()
        .any(|entry| entry.feature.as_str() == HOST_FEATURE)
    {
        registry
            .add_module(&host_module())
            .map_err(invalid_permission)?;
    }
    let mut existing: BTreeMap<String, Declarations> = BTreeMap::new();
    for entry in catalog.namespaces {
        if entry.feature.as_str().starts_with(LOADER_PREFIX) {
            existing
                .entry(entry.feature.to_string())
                .or_default()
                .insert(entry.namespace, entry.declaration);
        }
    }
    for (id, old) in &existing {
        if declarations.get(id) != Some(old) {
            registry
                .remove(&FeatureId::new(id).map_err(invalid_permission)?)
                .map_err(invalid_permission)?;
        }
    }
    for (id, declaration) in declarations {
        if declaration.is_empty() {
            continue;
        }
        let feature = FeatureId::new(&id).map_err(invalid_permission)?;
        if existing.get(&id) == Some(&declaration) {
            registry
                .set_enabled(&feature, true)
                .map_err(invalid_permission)?;
        } else {
            registry
                .add_module(&TrustedPluginPermissions {
                    id: feature,
                    declarations: declaration,
                })
                .map_err(invalid_permission)?;
        }
    }
    registry
        .set_enabled(&host_id, true)
        .map_err(invalid_permission)?;
    Ok(())
}

impl SqliteWorkspaceStore {
    /// Call before resolving current policy, outside any existing config-store
    /// transaction. Lock order is registry write -> catalog transaction. This is a
    /// rebuildable projection of adopted references, not a second policy writer.
    pub(crate) fn sync_plugin_permission_registry(
        &self,
        root: &Path,
        workspace: &str,
        registry: &mut FeaturePermissionRegistry,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let selected = tx.prepare("SELECT r.installation_id,r.artifact_digest,a.artifact_size \
                FROM plugin_references r JOIN plugin_installations i USING(workspace_id,installation_id) \
                JOIN plugin_artifacts a USING(workspace_id,installation_id,artifact_digest) \
                WHERE r.workspace_id=?1 AND r.kind='config' AND r.owner='workspace-config' \
                AND i.uninstalled=0 AND a.revoked=0 ORDER BY r.installation_id,r.artifact_digest")?
                .query_map([workspace], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, u64>(2)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut declarations = BTreeMap::new();
            for (installation, digest, size) in selected {
                let manifest = self.plugin_manifest_cache.read(root, &digest, size)?;
                let id = format!("{LOADER_PREFIX}{installation}:{digest}");
                declarations.insert(id, manifest.permission_namespaces.clone());
            }
            reconcile(registry, workspace, declarations)?;
            tx.commit()?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
