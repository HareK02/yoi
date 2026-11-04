//! Declaration-only bridge from the selected, trusted Feature modules to the
//! payload-free permission catalog. Actual policy adoption and operation checks
//! belong to the Workspace authority, not Feature activation.
//!
//! The registry builder obtains each owner from the registered module descriptor.
//! A module receives only its registrar; neither wire data nor a module-selected
//! owner string can change that registrar's registration identity.

use super::{FeatureId, FeatureInstallError, FeatureModule, FeatureRegistryBuilder};
use feature_permissions::{
    FeatureHandle, FeatureIdentity, NamespaceDeclaration, NamespaceHandle, NamespaceId,
    PermissionHost, WorkspaceId,
};
use std::collections::BTreeMap;

pub struct FeaturePermissionRegistrar<'a> {
    host: &'a mut PermissionHost,
    feature: FeatureHandle,
}

impl FeaturePermissionRegistrar<'_> {
    /// The returned handle may be retained by the Feature for exact-key queries.
    /// It is not a grant and cannot query another registered namespace.
    pub fn namespace(
        &mut self,
        namespace: NamespaceId,
    ) -> Result<NamespaceHandle, FeatureInstallError> {
        self.host
            .bind_namespace(&self.feature, namespace)
            .map_err(invalid)
    }

    pub fn declare(
        &mut self,
        namespace: &NamespaceHandle,
        declaration: NamespaceDeclaration,
    ) -> Result<(), FeatureInstallError> {
        if !namespace.is_owned_by(&self.feature) {
            return Err(invalid(
                "namespace handle belongs to another Feature registration",
            ));
        }
        self.host.declare(namespace, declaration).map_err(invalid)
    }
}

pub struct FeaturePermissionRegistry {
    host: PermissionHost,
    features: BTreeMap<FeatureId, FeatureHandle>,
}

fn invalid(error: impl std::fmt::Display) -> FeatureInstallError {
    FeatureInstallError::InvalidDescriptor(error.to_string())
}

impl FeaturePermissionRegistry {
    /// Called by the trusted Host after selecting builtin/trusted Plugin modules.
    /// Registration alone grants nothing to a Subject or Worker. A failed
    /// declaration drops the whole candidate registry, never a partial catalog.
    pub(super) fn from_builder(
        builder: &FeatureRegistryBuilder,
        workspace: WorkspaceId,
    ) -> Result<Self, FeatureInstallError> {
        builder.plan().map_err(invalid)?;
        let mut result = Self {
            host: PermissionHost::new(workspace),
            features: BTreeMap::new(),
        };
        for module in &builder.modules {
            result.register(module.as_ref())?;
        }
        Ok(result)
    }

    fn register(&mut self, module: &dyn FeatureModule) -> Result<(), FeatureInstallError> {
        let id = module.descriptor().id;
        let identity = FeatureIdentity::parse(id.as_str()).map_err(invalid)?;
        let feature = self.host.register_feature(identity).map_err(invalid)?;
        module.declare_permissions(&mut FeaturePermissionRegistrar {
            host: &mut self.host,
            feature: feature.clone(),
        })?;
        self.features.insert(id, feature);
        Ok(())
    }

    pub fn host(&self) -> &PermissionHost {
        &self.host
    }

    /// Replace a declaration only through its already bound namespace handle.
    /// Failed updates retain the prior catalog and its usable resolutions.
    pub fn update(
        &mut self,
        namespace: &NamespaceHandle,
        declaration: NamespaceDeclaration,
    ) -> Result<(), FeatureInstallError> {
        self.host.declare(namespace, declaration).map_err(invalid)
    }

    /// The trusted loader owns enable/disable, not a Feature's self-report.
    pub fn set_enabled(
        &mut self,
        feature: &FeatureId,
        enabled: bool,
    ) -> Result<(), FeatureInstallError> {
        let handle = self
            .features
            .get(feature)
            .ok_or_else(|| invalid("Feature is not registered"))?;
        self.host.set_enabled(handle, enabled).map_err(invalid)
    }

    pub fn remove(&mut self, feature: &FeatureId) -> Result<(), FeatureInstallError> {
        let handle = self
            .features
            .get(feature)
            .ok_or_else(|| invalid("Feature is not registered"))?;
        self.host.remove_feature(handle).map_err(invalid)?;
        self.features.remove(feature);
        Ok(())
    }

    /// A newly trusted Plugin can contribute without copying grants to existing
    /// subjects. The Host selects this module; guest declaration data does not.
    pub fn add_module(&mut self, module: &dyn FeatureModule) -> Result<(), FeatureInstallError> {
        // Construct on a fresh host would retire all prior handles. Instead stage
        // this registration, removing it on failure while preserving existing ones.
        let id = module.descriptor().id;
        if self.features.contains_key(&id) {
            return Err(invalid("Feature is already registered"));
        }
        let identity = FeatureIdentity::parse(id.as_str()).map_err(invalid)?;
        let feature = self.host.register_feature(identity).map_err(invalid)?;
        if let Err(error) = module.declare_permissions(&mut FeaturePermissionRegistrar {
            host: &mut self.host,
            feature: feature.clone(),
        }) {
            self.host.remove_feature(&feature).map_err(invalid)?;
            return Err(error);
        }
        self.features.insert(id, feature);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
