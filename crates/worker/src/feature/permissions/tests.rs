//! Declaration bridge + real LocalWorkdirSession/provider constraints. No live
//! Server/Runtime, policy persistence or production operation-authority hookup.
use super::*;
use crate::feature::{FeatureDescriptor, FeatureInstallContext, FeatureRuntimeKind};
use feature_permissions::*;
use std::sync::{Arc, Mutex};

struct DeclaredFeature {
    descriptor: FeatureDescriptor,
    namespace: &'static str,
    key: &'static str,
    handle: Arc<Mutex<Option<NamespaceHandle>>>,
}
impl FeatureModule for DeclaredFeature {
    fn descriptor(&self) -> FeatureDescriptor {
        self.descriptor.clone()
    }
    fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        Ok(())
    }
    fn declare_permissions(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        let handle = registrar.namespace(NamespaceId::parse(self.namespace).unwrap())?;
        registrar.declare(
            &handle,
            NamespaceDeclaration {
                permissions: vec![PermissionDefinition {
                    key: PermissionKey::parse(self.key).unwrap(),
                    description: "Fixture operation".into(),
                    children: vec![],
                }],
                sets: vec![SetContribution {
                    set: PermissionSetId::parse("companion").unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse(self.namespace).unwrap(),
                        keys: vec![PermissionKey::parse(self.key).unwrap()],
                    })],
                }],
            },
        )?;
        *self.handle.lock().unwrap() = Some(handle);
        Ok(())
    }
}
fn feature(namespace: &'static str, key: &'static str, plugin: bool) -> DeclaredFeature {
    let mut descriptor = FeatureDescriptor::builtin(namespace, namespace);
    if plugin {
        descriptor.id = FeatureId::new(format!("plugin:{namespace}")).unwrap();
        descriptor.runtime = FeatureRuntimeKind::ExternalPlugin;
    }
    DeclaredFeature {
        descriptor,
        namespace,
        key,
        handle: Arc::new(Mutex::new(None)),
    }
}
fn policy() -> PermissionAssignment {
    PermissionAssignment {
        grants: vec![PermissionReference::Set(
            PermissionSetId::parse("companion").unwrap(),
        )],
        ..Default::default()
    }
}

#[test]
fn registered_modules_receive_owner_bound_handles_and_trusted_plugin_updates_existing_set_holder() {
    let ticket = feature("ticket", "ticket.hoge.fuga", false);
    let ticket_handle = ticket.handle.clone();
    let workdir = feature("workdir", "workdir.read", false);
    let mut registry = FeatureRegistryBuilder::new()
        .with_module(ticket)
        .with_module(workdir)
        .permission_registry(WorkspaceId::parse("workspace-a").unwrap())
        .unwrap();
    let assignment = policy();
    assert_eq!(
        registry
            .host()
            .resolve(Some(&assignment))
            .projection()
            .allowed
            .len(),
        2
    );
    let plugin = feature("external", "external.read.deep", true);
    let plugin_handle = plugin.handle.clone();
    registry.add_module(&plugin).unwrap();
    let resolved = registry.host().resolve(Some(&assignment));
    assert_eq!(resolved.projection().allowed.len(), 3);
    let ticket_handle = ticket_handle.lock().unwrap().clone().unwrap();
    let plugin_handle = plugin_handle.lock().unwrap().clone().unwrap();
    assert!(
        registry
            .host()
            .allows(
                &ticket_handle,
                &resolved,
                &PermissionKey::parse("ticket.hoge.fuga").unwrap()
            )
            .unwrap()
    );
    assert_eq!(
        registry.host().allows(
            &plugin_handle,
            &resolved,
            &PermissionKey::parse("ticket.hoge.fuga").unwrap()
        ),
        Err(PermissionError::Ownership(
            PermissionKey::parse("ticket.hoge.fuga").unwrap()
        ))
    );
    registry.set_enabled(&plugin.descriptor.id, false).unwrap();
    let resolved = registry.host().resolve(Some(&assignment));
    assert_eq!(resolved.projection().allowed.len(), 2);
    assert!(
        !registry
            .host()
            .allows(
                &plugin_handle,
                &resolved,
                &PermissionKey::parse("external.read.deep").unwrap()
            )
            .unwrap()
    );
    registry.remove(&plugin.descriptor.id).unwrap();
    assert_eq!(
        registry
            .host()
            .resolve(Some(&assignment))
            .projection()
            .allowed
            .len(),
        2
    );
}

#[test]
fn namespace_collision_rejects_the_entire_candidate_without_replacing_the_original_feature() {
    let mut registry = FeatureRegistryBuilder::new()
        .with_module(feature("ticket", "ticket.read", false))
        .permission_registry(WorkspaceId::parse("workspace-a").unwrap())
        .unwrap();
    let mut plugin = feature("ticket", "ticket.steal", true);
    plugin.descriptor.id = FeatureId::new("plugin:impostor").unwrap();
    assert!(registry.add_module(&plugin).is_err());
    let catalog = registry.host().catalog();
    assert_eq!(catalog.namespaces.len(), 1);
    assert_eq!(catalog.namespaces[0].feature.as_str(), "builtin:ticket");
    assert_eq!(
        catalog.namespaces[0].declaration.permissions[0]
            .key
            .as_str(),
        "ticket.read"
    );
}

#[tokio::test]
async fn allowed_operation_key_does_not_override_real_provider_scope_recursion_or_symlink_checks() {
    use manifest::{Permission, Scope, ScopeConfig, ScopeRule};
    use workdir::{LocalWorkdirSession, ReadRequest, WorkdirPath, WorkdirSession};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("checkout");
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("allowed.txt"), "allowed").unwrap();
    std::fs::write(root.join("nested/deep.txt"), "outside recursive scope").unwrap();
    std::fs::write(temp.path().join("secret.txt"), "outside root").unwrap();
    let scope = Scope::from_config(&ScopeConfig {
        allow: vec![ScopeRule {
            target: root.clone(),
            permission: Permission::Read,
            recursive: false,
            symlink_policy: Default::default(),
        }],
        deny: vec![],
    })
    .unwrap();
    let session = LocalWorkdirSession::new(scope, root.clone());
    let module = feature("workdir", "workdir.read", false);
    let handle = module.handle.clone();
    let registry = FeatureRegistryBuilder::new()
        .with_module(module)
        .permission_registry(WorkspaceId::parse("workspace-a").unwrap())
        .unwrap();
    let resolved = registry.host().resolve(Some(&policy()));
    assert!(
        registry
            .host()
            .allows(
                handle.lock().unwrap().as_ref().unwrap(),
                &resolved,
                &PermissionKey::parse("workdir.read").unwrap()
            )
            .unwrap()
    );
    let read = |path| ReadRequest {
        path: WorkdirPath::new(path).unwrap(),
        offset: 0,
        limit: 20,
        max_bytes: 1024,
    };
    assert_eq!(
        session.read(read("allowed.txt")).await.unwrap().bytes,
        b"allowed"
    );
    assert_eq!(
        session
            .read(read("nested/deep.txt"))
            .await
            .unwrap_err()
            .denial_reason(),
        Some(workdir::WorkdirDenialReason::PathOutOfScope)
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(temp.path().join("secret.txt"), root.join("escape.txt"))
            .unwrap();
        assert_eq!(
            session
                .read(read("escape.txt"))
                .await
                .unwrap_err()
                .denial_reason(),
            Some(workdir::WorkdirDenialReason::SymlinkTargetOutOfScope)
        );
    }
}

#[test]
fn registrar_rejects_a_foreign_handle_even_when_its_wire_keys_match_that_namespace() {
    struct ForeignHandleFeature(NamespaceHandle);
    impl FeatureModule for ForeignHandleFeature {
        fn descriptor(&self) -> FeatureDescriptor {
            let mut descriptor = FeatureDescriptor::builtin("impostor", "Impostor");
            descriptor.id = FeatureId::new("plugin:impostor").unwrap();
            descriptor.runtime = FeatureRuntimeKind::ExternalPlugin;
            descriptor
        }
        fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
            Ok(())
        }
        fn declare_permissions(
            &self,
            registrar: &mut FeaturePermissionRegistrar<'_>,
        ) -> Result<(), FeatureInstallError> {
            let declaration = serde_json::from_str(r#"{"permissions":[{"key":"ticket.steal","description":"forged","children":[]}],"sets":[]}"#).unwrap();
            registrar.declare(&self.0, declaration)
        }
    }
    let module = feature("ticket", "ticket.read", false);
    let handle = module.handle.clone();
    let mut registry = FeatureRegistryBuilder::new()
        .with_module(module)
        .permission_registry(WorkspaceId::parse("workspace-a").unwrap())
        .unwrap();
    let before = registry.host().catalog();
    let impostor = ForeignHandleFeature(handle.lock().unwrap().clone().unwrap());
    assert!(registry.add_module(&impostor).is_err());
    assert_eq!(before, registry.host().catalog());
}
