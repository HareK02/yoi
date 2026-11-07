//! Builtin declarations use the same owner registrar as trusted Plugin modules.
//! Resource selectors and runtime/provider constraints deliberately live elsewhere.

use super::{FeaturePermissionRegistrar, invalid};
use crate::feature::{
    FeatureDescriptor, FeatureInstallContext, FeatureInstallError, FeatureModule,
};
use feature_permissions::{
    NamespaceDeclaration, NamespaceId, NamespacePermissions, PermissionDefinition, PermissionKey,
    PermissionReference, PermissionSetId, SetContribution,
};
use std::collections::BTreeMap;

pub(crate) struct BuiltinPermissionOperation {
    pub key: &'static str,
    pub description: &'static str,
    pub sets: &'static [&'static str],
}

pub(crate) struct BuiltinPermissionDeclaration {
    pub feature: &'static str,
    pub namespace: &'static str,
    pub operations: &'static [BuiltinPermissionOperation],
}

impl BuiltinPermissionDeclaration {
    pub(crate) fn declare(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        let namespace = NamespaceId::parse(self.namespace).map_err(invalid)?;
        let handle = registrar.namespace(namespace.clone())?;
        let mut permissions = Vec::new();
        let mut sets: BTreeMap<PermissionSetId, Vec<PermissionKey>> = BTreeMap::new();
        for operation in self.operations {
            let key = PermissionKey::parse(operation.key).map_err(invalid)?;
            permissions.push(PermissionDefinition {
                key: key.clone(),
                description: operation.description.into(),
                children: vec![],
            });
            // Namespace-named sets retain the existing explicit migration target;
            // namespace ownership itself still grants no operation.
            for set in std::iter::once(self.namespace).chain(operation.sets.iter().copied()) {
                sets.entry(PermissionSetId::parse(set).map_err(invalid)?)
                    .or_default()
                    .push(key.clone());
            }
        }
        registrar.declare(
            &handle,
            NamespaceDeclaration {
                permissions,
                sets: sets
                    .into_iter()
                    .map(|(set, keys)| SetContribution {
                        set,
                        children: vec![PermissionReference::Namespace(NamespacePermissions {
                            namespace: namespace.clone(),
                            keys,
                        })],
                    })
                    .collect(),
            },
        )
    }
}

/// Metadata-only composition does not install Tools/services or run startup gates.
/// Runtime modules use the same local declaration in `declare_permissions`.
pub(crate) struct BuiltinPermissionFeature(pub &'static BuiltinPermissionDeclaration);
impl FeatureModule for BuiltinPermissionFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        FeatureDescriptor::builtin(self.0.feature, self.0.feature)
    }

    fn declare_permissions(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        self.0.declare(registrar)
    }

    fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        Ok(())
    }
}
