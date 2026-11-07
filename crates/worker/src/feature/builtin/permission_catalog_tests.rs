//! Exercises the production builtin declarations and T-836 Host, not a running
//! Server/Runtime, persistence migration or the separate domain-authority checks.
use super::*;
use crate::feature::permissions::FeaturePermissionRegistrar;
use crate::feature::{FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureModule};
use feature_permissions::*;
use server_api::authorization::{
    COMPANION_PERMISSION_SET, IMPLEMENTER_PERMISSION_SET, subject_default_permission_assignment,
};
use std::collections::BTreeSet;

fn registry() -> FeaturePermissionRegistry {
    builtin_permission_registry(WorkspaceId::parse("workspace-a").unwrap()).unwrap()
}
fn assignment(set: &str) -> PermissionAssignment {
    PermissionAssignment {
        grants: vec![PermissionReference::Set(
            PermissionSetId::parse(set).unwrap(),
        )],
        ..Default::default()
    }
}
fn allows(
    registry: &FeaturePermissionRegistry,
    policy: &PermissionAssignment,
    feature: &str,
    key: &str,
) -> bool {
    let key = PermissionKey::parse(key).unwrap();
    let handle = registry
        .namespace_handle(&FeatureId::builtin(feature), &key.namespace())
        .unwrap();
    registry
        .host()
        .allows(handle, &registry.host().resolve(Some(policy)), &key)
        .unwrap()
}

#[test]
fn builtin_declarations_preserve_legacy_exact_keys_without_using_legacy_resource_metadata() {
    let registry = registry();
    let mut keys = BTreeSet::new();
    for namespace in registry.host().catalog().namespaces {
        assert!(
            registry
                .namespace_handle(
                    &FeatureId::new(namespace.feature.as_str()).unwrap(),
                    &namespace.namespace
                )
                .is_some()
        );
        for definition in namespace.declaration.permissions {
            assert!(definition.children.is_empty());
            assert!(keys.insert(definition.key.as_str().to_owned()));
        }
    }
    assert!(!keys.contains("mcp"));
    // Retained legacy keys must resolve, but additions never require a second
    // entry in that editor's resource table.
    let legacy: BTreeSet<_> = server_api::authorization::subject_permission_operations()
        .into_iter()
        .filter(|o| o.operation != "mcp")
        .map(|o| o.operation)
        .collect();
    assert!(legacy.is_subset(&keys));
}

#[test]
fn subject_default_reference_resolves_conversation_and_ticket_acceptance_not_provider_or_control_authority()
 {
    let registry = registry();
    let policy = subject_default_permission_assignment();
    for (feature, key) in [
        ("subjektiv-chat", "chat.read"),
        ("subjektiv-chat", "chat.send"),
        ("session-explore", "session.record"),
        ("memory", "memory.resident.supply"),
        ("journal-recording", "journal.job.input.read"),
        ("ticket", "ticket.read"),
        ("ticket", "ticket.assignment.query"),
        ("ticket", "ticket.assignment.update"),
        ("notification", "notification.diagnostics.read"),
    ] {
        assert!(allows(&registry, &policy, feature, key), "{key}");
    }
    for (feature, key) in [
        ("workdir", "filesystem.write"),
        ("bash", "bash.execute"),
        ("worker", "worker.stop"),
        ("ticket", "ticket.assignment.assign"),
        ("workspace-config", "workspace_config.commit"),
    ] {
        assert!(!allows(&registry, &policy, feature, key), "{key}");
    }
    assert_eq!(policy.grants.len(), 1);
    let empty = PermissionAssignment::default();
    assert!(!allows(&registry, &empty, "subjektiv-chat", "chat.send"));
    let mut excluded = policy.clone();
    excluded
        .exclusions
        .push(PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![PermissionKey::parse("ticket.read").unwrap()],
        }));
    assert!(!allows(&registry, &excluded, "ticket", "ticket.read"));
    assert!(allows(
        &registry,
        &excluded,
        "ticket",
        "ticket.assignment.query"
    ));
    excluded.revoked = true;
    assert!(!allows(
        &registry,
        &excluded,
        "ticket",
        "ticket.assignment.query"
    ));
    assert!(
        registry
            .host()
            .resolve(None)
            .projection()
            .allowed
            .is_empty()
    );
}

#[test]
fn shared_sets_are_feature_contributions_and_callee_handles_cannot_be_substituted() {
    let registry = registry();
    let companion = assignment(COMPANION_PERMISSION_SET);
    let implementer = assignment(IMPLEMENTER_PERMISSION_SET);
    assert!(allows(&registry, &companion, "ticket", "ticket.query"));
    assert!(allows(&registry, &companion, "objective", "objective.read"));
    assert!(!allows(&registry, &companion, "bash", "bash.execute"));
    assert!(allows(&registry, &implementer, "bash", "bash.execute"));
    assert!(allows(
        &registry,
        &implementer,
        "sub-worker-control",
        "subworker.spawn"
    ));
    let ticket = registry
        .namespace_handle(
            &FeatureId::builtin("ticket"),
            &NamespaceId::parse("ticket").unwrap(),
        )
        .unwrap();
    assert!(
        registry
            .namespace_handle(
                &FeatureId::builtin("ticket"),
                &NamespaceId::parse("objective").unwrap()
            )
            .is_none()
    );
    assert!(matches!(
        registry.host().allows(
            ticket,
            &registry.host().resolve(Some(&companion)),
            &PermissionKey::parse("objective.read").unwrap()
        ),
        Err(PermissionError::Ownership(_))
    ));
    let other_workspace =
        builtin_permission_registry(WorkspaceId::parse("workspace-b").unwrap()).unwrap();
    assert_eq!(
        other_workspace.host().allows(
            ticket,
            &other_workspace.host().resolve(Some(&companion)),
            &PermissionKey::parse("ticket.read").unwrap()
        ),
        Err(PermissionError::ForeignHandle)
    );
}

#[derive(Debug)]
struct RegistrationOnlyClient;
impl crate::worker::WorkspaceClient for RegistrationOnlyClient {
    fn workspace_id(&self) -> Option<&str> {
        Some("workspace-a")
    }
    fn kind(&self) -> &str {
        "registration-fixture"
    }
    fn is_available(&self) -> bool {
        false
    }
    fn execute(
        &self,
        _: crate::worker::WorkspaceRequest,
    ) -> Result<crate::worker::WorkspaceResponse, crate::worker::WorkspaceClientError> {
        panic!("declaration must not execute or gate Feature startup")
    }
}

#[test]
fn runtime_feature_declares_the_same_catalog_independently_of_tool_projection() {
    use crate::feature::builtin::ticket::{TicketFeature, TicketFeatureAccess};
    let client = std::sync::Arc::new(RegistrationOnlyClient);
    let runtime = FeatureRegistryBuilder::new()
        .with_module(TicketFeature::new(client, TicketFeatureAccess::read_only()))
        .permission_registry(WorkspaceId::parse("workspace-a").unwrap())
        .unwrap();
    let runtime_catalog = runtime.host().catalog();
    let builtin_catalog = registry().host().catalog();
    let ticket = builtin_catalog
        .namespaces
        .into_iter()
        .find(|n| n.namespace.as_str() == "ticket")
        .unwrap();
    assert_eq!(runtime_catalog.namespaces, vec![ticket]);
    // Read-only model projection does not define the catalog or issue authority.
    assert!(allows(
        &runtime,
        &subject_default_permission_assignment(),
        "ticket",
        "ticket.assignment.update"
    ));
    assert!(runtime.host().resolve(None).projection().allowed.is_empty());
}

#[test]
fn plugin_host_exact_capabilities_are_available_to_opted_in_sets_not_subject_default_or_other_callees()
 {
    let registry = registry();
    let expected: BTreeSet<_> = [
        "plugin.admit",
        "plugin.artifact.read",
        "plugin.service.consume",
        "plugin.log",
        "plugin.publish",
        "plugin.connection.open",
        "plugin.http.send",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let host_catalog = registry
        .host()
        .catalog()
        .namespaces
        .into_iter()
        .find(|n| n.namespace.as_str() == "plugin")
        .unwrap();
    assert_eq!(host_catalog.feature.as_str(), "builtin:plugin-host");
    assert_eq!(
        host_catalog
            .declaration
            .permissions
            .iter()
            .map(|d| d.key.as_str().to_owned())
            .collect::<BTreeSet<_>>(),
        expected
    );
    for set in [
        COMPANION_PERMISSION_SET,
        IMPLEMENTER_PERMISSION_SET,
        "plugin",
    ] {
        let policy = assignment(set);
        for key in &expected {
            assert!(
                allows(&registry, &policy, "plugin-host", key),
                "{set}: {key}"
            );
            assert!(
                !allows(
                    &registry,
                    &subject_default_permission_assignment(),
                    "plugin-host",
                    key
                ),
                "default: {key}"
            );
        }
        assert!(!allows(
            &registry,
            &policy,
            "plugin-host",
            "plugin.service.consume.unrelated"
        ));
    }
    let plugin_only = assignment("plugin");
    let resolved = registry.host().resolve(Some(&plugin_only));
    let handle = registry
        .namespace_handle(
            &FeatureId::builtin("plugin-host"),
            &NamespaceId::parse("plugin").unwrap(),
        )
        .unwrap();
    assert!(!allows(&registry, &plugin_only, "ticket", "ticket.read"));
    assert!(matches!(
        registry.host().allows(
            handle,
            &resolved,
            &PermissionKey::parse("ticket.read").unwrap()
        ),
        Err(PermissionError::Ownership(_))
    ));
    let limited = registry
        .host()
        .issue_delegation(&resolved, &[PermissionKey::parse("plugin.admit").unwrap()])
        .unwrap();
    let job = registry
        .host()
        .issue_job_grant(
            "job".into(),
            &resolved,
            &[PermissionKey::parse("plugin.admit").unwrap()],
        )
        .unwrap();
    let result = registry
        .host()
        .resolve_job("job", &resolved, &resolved, &limited, &job)
        .unwrap();
    assert!(
        registry
            .host()
            .allows(
                handle,
                &result,
                &PermissionKey::parse("plugin.admit").unwrap()
            )
            .unwrap()
    );
    assert!(
        !registry
            .host()
            .allows(
                handle,
                &result,
                &PermissionKey::parse("plugin.publish").unwrap()
            )
            .unwrap()
    );
}

#[test]
fn memory_journal_body_reader_operations_belong_to_journal_not_memory_and_do_not_enter_limited_jobs()
 {
    let registry = registry();
    let policy = subject_default_permission_assignment();
    let execution = registry.host().resolve(Some(&policy));
    let handle = registry
        .namespace_handle(
            &FeatureId::builtin("journal-recording"),
            &NamespaceId::parse("journal").unwrap(),
        )
        .unwrap();
    let memory = registry
        .namespace_handle(
            &FeatureId::builtin("memory"),
            &NamespaceId::parse("memory").unwrap(),
        )
        .unwrap();
    let input = PermissionKey::parse("journal.job.input.read").unwrap();
    let child = registry
        .host()
        .issue_delegation(&execution, &[input.clone()])
        .unwrap();
    let job = registry
        .host()
        .issue_job_grant("journal-job".into(), &execution, &[input])
        .unwrap();
    let limited = registry
        .host()
        .resolve_job("journal-job", &execution, &execution, &child, &job)
        .unwrap();
    for operation in [
        "journal.memory.links.reconcile",
        "journal.memory.links.read",
        "journal.memory.read",
        "journal.memory.related.read",
    ] {
        let key = PermissionKey::parse(operation).unwrap();
        for set in [
            "subject-default",
            COMPANION_PERMISSION_SET,
            IMPLEMENTER_PERMISSION_SET,
        ] {
            assert!(
                allows(&registry, &assignment(set), "journal-recording", operation),
                "{set}: {operation}"
            );
        }
        assert!(matches!(
            registry.host().allows(memory, &execution, &key),
            Err(PermissionError::Ownership(_))
        ));
        assert!(!registry.host().allows(handle, &limited, &key).unwrap());
    }
    assert!(!allows(
        &registry,
        &policy,
        "journal-recording",
        "journal.memory.read.unrelated"
    ));
}

struct Plugin;
impl FeatureModule for Plugin {
    fn descriptor(&self) -> FeatureDescriptor {
        let mut d = FeatureDescriptor::builtin("trusted-plugin-fixture", "Trusted plugin fixture");
        d.id = FeatureId::new("plugin:trusted-fixture").unwrap();
        d.runtime = crate::feature::FeatureRuntimeKind::ExternalPlugin;
        d
    }
    fn install(&self, _: &mut FeatureInstallContext<'_>) -> Result<(), FeatureInstallError> {
        Ok(())
    }
    fn declare_permissions(
        &self,
        registrar: &mut FeaturePermissionRegistrar<'_>,
    ) -> Result<(), FeatureInstallError> {
        let h = registrar.namespace(NamespaceId::parse("external").unwrap())?;
        registrar.declare(&h, plugin_declaration(true))
    }
}
fn plugin_declaration(children: bool) -> NamespaceDeclaration {
    NamespaceDeclaration {
        permissions: [
            "external.hoge",
            "external.hoge.fuga",
            "external.hoge.unrelated",
        ]
        .into_iter()
        .map(|key| PermissionDefinition {
            key: PermissionKey::parse(key).unwrap(),
            description: "Fixture".into(),
            children: if children && key == "external.hoge" {
                vec![PermissionKey::parse("external.hoge.fuga").unwrap()]
            } else {
                vec![]
            },
        })
        .collect(),
        sets: [COMPANION_PERMISSION_SET, "subject-default"]
            .into_iter()
            .map(|set| SetContribution {
                set: PermissionSetId::parse(set).unwrap(),
                children: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: NamespaceId::parse("external").unwrap(),
                    keys: vec![PermissionKey::parse("external.hoge").unwrap()],
                })],
            })
            .collect(),
    }
}

#[test]
fn trusted_plugin_updates_existing_builtin_set_holders_but_not_frozen_children_or_removed_contributions()
 {
    let mut registry = registry();
    let policy = subject_default_permission_assignment();
    let before = registry.host().resolve(Some(&policy));
    let mut limited = registry
        .host()
        .issue_delegation(&before, &[PermissionKey::parse("ticket.read").unwrap()])
        .unwrap();
    registry.add_module(&Plugin).unwrap();
    let id = FeatureId::new("plugin:trusted-fixture").unwrap();
    let handle = registry
        .namespace_handle(&id, &NamespaceId::parse("external").unwrap())
        .unwrap()
        .clone();
    let key = PermissionKey::parse("external.hoge.fuga").unwrap();
    assert!(
        registry
            .host()
            .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
            .unwrap()
    );
    assert!(
        !registry
            .host()
            .allows(
                &handle,
                &registry.host().resolve(Some(&policy)),
                &PermissionKey::parse("external.hoge.unrelated").unwrap()
            )
            .unwrap()
    );
    assert!(
        !registry
            .host()
            .allows(
                &handle,
                &registry
                    .host()
                    .resolve_limited(
                        &registry.host().resolve(Some(&policy)),
                        &registry.host().resolve(Some(&policy)),
                        &limited
                    )
                    .unwrap(),
                &key
            )
            .unwrap()
    );
    assert_eq!(
        registry.host().allows(&handle, &before, &key),
        Err(PermissionError::StaleResolution)
    );
    registry.update(&handle, plugin_declaration(false)).unwrap();
    assert!(
        !registry
            .host()
            .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
            .unwrap()
    );
    registry.update(&handle, plugin_declaration(true)).unwrap();
    registry.set_enabled(&id, false).unwrap();
    assert!(
        !registry
            .host()
            .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
            .unwrap()
    );
    assert!(allows(&registry, &policy, "ticket", "ticket.read"));
    registry.set_enabled(&id, true).unwrap();
    assert!(
        registry
            .host()
            .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
            .unwrap()
    );
    registry.remove(&id).unwrap();
    assert!(
        registry
            .namespace_handle(&id, &NamespaceId::parse("external").unwrap())
            .is_none()
    );
    assert_eq!(
        registry
            .host()
            .allows(&handle, &registry.host().resolve(Some(&policy)), &key),
        Err(PermissionError::ForeignHandle)
    );
    assert!(allows(&registry, &policy, "ticket", "ticket.read"));
    limited.revoke();
    assert!(matches!(
        registry.host().resolve_limited(
            &registry.host().resolve(Some(&policy)),
            &registry.host().resolve(Some(&policy)),
            &limited
        ),
        Err(PermissionError::RevokedGrant)
    ));
}
