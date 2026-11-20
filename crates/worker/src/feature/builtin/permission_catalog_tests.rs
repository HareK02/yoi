//! Exercises the production builtin declarations and T-836 Host, not a running
//! Server/Runtime, persistence migration or the separate domain-authority checks.
use super::*;
use crate::feature::permissions::FeaturePermissionRegistrar;
use crate::feature::{FeatureDescriptor, FeatureId, FeatureInstallContext, FeatureModule};
use feature_permissions::*;
use server_api::authorization::{
    COMPANION_PERMISSION_SET, CONVERSATION_PERMISSION_SET, IMPLEMENTER_PERMISSION_SET,
    WORK_COORDINATION_PERMISSION_SET, WORK_EXECUTION_PERMISSION_SET,
    subject_default_delegation_assignment, subject_default_permission_assignment,
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
        assert_ne!(namespace.namespace.as_str(), "subject");
        for definition in namespace.declaration.permissions {
            assert!(
                definition
                    .children
                    .iter()
                    .all(|child| child.namespace() == namespace.namespace)
            );
            assert!(keys.insert(definition.key.as_str().to_owned()));
        }
        for contribution in namespace.declaration.sets {
            assert_ne!(contribution.set.as_str(), "subject-default");
            for child in contribution.children {
                let PermissionReference::Namespace(child) = child else {
                    panic!("builtin contributions must reference their own namespace");
                };
                assert_eq!(child.namespace, namespace.namespace);
                assert!(
                    child
                        .keys
                        .iter()
                        .all(|key| key.namespace() == namespace.namespace)
                );
            }
        }
    }
    assert!(!keys.contains("mcp"));
    assert!(!keys.contains("subject.orchestrate"));
    assert!(!keys.contains("subject.delegate"));
    // Retained legacy keys must resolve, but additions never require a second
    // entry in that editor's resource table.
    let legacy: BTreeSet<_> = server_api::authorization::subject_permission_operations()
        .into_iter()
        .filter(|o| o.operation != "mcp")
        .map(|o| o.operation)
        .collect();
    assert!(legacy.is_subset(&keys));
}

// Independent exact expectations: every other catalog key is denied. Do not
// derive these from declarations or the legacy resource-operation table.
const CONVERSATION_KEYS: &[&str] = &[
    "chat.read",
    "chat.send",
    "session.record",
    "session.list",
    "session.search",
    "session.read",
    "memory.query",
    "memory.read",
    "memory.changes.list",
    "memory.resident.read",
    "memory.resident.supply",
    "memory.staging.write",
    "memory.staging.list",
    "memory.staging.read",
    "memory.candidate.decide",
    "memory.proposal.validate",
    "memory.receipt.read",
    "memory.surface.prepare",
    "memory.surface.supply",
    "memory.surface.publish",
    "memory.surface.fail",
    "memory.extract.read",
    "memory.extract.supply",
    "memory.extract.commit",
    "memory.consolidation.request",
    "memory.consolidation.supply",
    "journal.startup.read",
    "journal.startup.supply",
    "journal.compaction.read",
    "journal.compaction.supply",
    "journal.recording.request",
    "journal.job.input.read",
    "journal.job.input.supply",
    "journal.memory.links.reconcile",
    "journal.memory.links.read",
    "journal.memory.read",
    "journal.memory.related.read",
    "journal.state.read",
    "journal.text.read",
    "journal.operation.query",
    "journal.operation.list",
    "journal.operation.coverage.read",
    "journal.entry.create",
    "journal.entry.fix",
    "journal.entry.rewrite",
    "journal.position.update",
    "journal.unchanged.confirm",
    "journal.checkpoint",
    "journal.finish",
];

const WORK_COORDINATION_KEYS: &[&str] = &[
    "notification.diagnostics.read",
    "objective.query",
    "objective.create",
    "objective.read",
    "objective.edit",
    "objective.set_state",
    "objective.link_ticket",
    "objective.unlink_ticket",
    "worker.list",
    "worker.discover",
    "worker.observe.list",
    "worker.observe",
    "worker.create",
    "worker.messaging",
    "worker.send_input",
    "worker.cancel",
    "worker.stop",
    "worker.restore",
    "worker.remove",
    "workdir.read",
    "workdir.list",
    "workdir.attachment.list",
    "workdir.attachment.read",
    "workdir.create",
    "workdir.delete",
    "repository.list",
    "repository.read",
    "merge_request.query",
    "merge_request.read",
    "merge_request.readiness",
    "merge_request.assign",
    "merge_request.complete",
    "ticket.create",
    "ticket.query",
    "ticket.list",
    "ticket.read",
    "ticket.edit_item",
    "ticket.edit_post",
    "ticket.event.add",
    "ticket.case.update",
    "ticket.dependency.check",
    "ticket.tag.list",
    "ticket.tag.create",
    "ticket.tag.rename",
    "ticket.tag.delete",
    "ticket.tags.set",
    "ticket.relation.query",
    "ticket.relation.read",
    "ticket.relation.record",
    "ticket.relation.remove",
    "ticket.assignment.assign",
    "ticket.assignment.clear",
    "ticket.assignment.update",
    "ticket.assignment.answer",
    "ticket.assignment.query",
    "ticket.assignment.history",
    "ticket.assignment.diagnostics",
    "ticket.implementation_result.write",
    "ticket.implementation_result.read",
];

const WORK_EXECUTION_KEYS: &[&str] = &[
    "filesystem.read",
    "filesystem.write",
    "bash.execute",
    "web.fetch",
    "web.search",
    "objective.query",
    "objective.read",
    "repository.list",
    "repository.read",
    "workdir.read",
    "workdir.attachment.read",
    "worker.list",
    "worker.messaging",
    "ticket.query",
    "ticket.read",
    "ticket.relation.read",
    "ticket.event.add",
    "ticket.implementation_result.write",
    "ticket.implementation_result.read",
    "merge_request.open",
    "merge_request.query",
    "merge_request.read",
    "merge_request.readiness",
    "merge_request.review",
    "merge_request.comment",
    "merge_request.publish",
];

fn assert_exact_policy(
    registry: &FeaturePermissionRegistry,
    policy: &PermissionAssignment,
    expected: &[&str],
) {
    let expected: BTreeSet<_> = expected
        .iter()
        .map(|key| PermissionKey::parse(*key).unwrap())
        .collect();
    let resolved = registry.host().resolve(Some(policy));
    assert_eq!(
        resolved
            .projection()
            .allowed
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected
    );
    // Check positives and negatives through each actual owner's handle, not just
    // the management projection. No namespace/prefix wildcard is implied.
    for namespace in registry.host().catalog().namespaces {
        let handle = registry
            .namespace_handle(
                &FeatureId::new(namespace.feature.as_str()).unwrap(),
                &namespace.namespace,
            )
            .unwrap();
        for definition in namespace.declaration.permissions {
            assert_eq!(
                registry
                    .host()
                    .allows(handle, &resolved, &definition.key)
                    .unwrap(),
                expected.contains(&definition.key),
                "{}",
                definition.key.as_str()
            );
            let unrelated =
                PermissionKey::parse(format!("{}.unrelated", definition.key.as_str())).unwrap();
            assert!(
                !registry
                    .host()
                    .allows(handle, &resolved, &unrelated)
                    .unwrap()
            );
        }
    }
}

#[test]
fn subject_defaults_are_live_sets_with_distinct_body_and_delegation_authority() {
    let registry = registry();
    assert_eq!(
        subject_default_permission_assignment(),
        PermissionAssignment {
            grants: [
                CONVERSATION_PERMISSION_SET,
                WORK_COORDINATION_PERMISSION_SET
            ]
            .into_iter()
            .map(|set| PermissionReference::Set(PermissionSetId::parse(set).unwrap()))
            .collect(),
            ..Default::default()
        }
    );
    assert_eq!(
        subject_default_delegation_assignment(),
        assignment(WORK_EXECUTION_PERMISSION_SET)
    );
    for (set, expected) in [
        (CONVERSATION_PERMISSION_SET, CONVERSATION_KEYS),
        (WORK_COORDINATION_PERMISSION_SET, WORK_COORDINATION_KEYS),
        (WORK_EXECUTION_PERMISSION_SET, WORK_EXECUTION_KEYS),
    ] {
        assert_exact_policy(&registry, &assignment(set), expected);
    }
    let body: Vec<_> = CONVERSATION_KEYS
        .iter()
        .chain(WORK_COORDINATION_KEYS)
        .copied()
        .collect();
    assert_exact_policy(&registry, &subject_default_permission_assignment(), &body);
    assert_exact_policy(
        &registry,
        &subject_default_delegation_assignment(),
        WORK_EXECUTION_KEYS,
    );
}

#[test]
fn retired_subject_references_are_not_current_catalog_aliases() {
    let registry = registry();
    for policy in [
        assignment("subject-default"),
        PermissionAssignment {
            grants: vec![PermissionReference::Namespace(NamespacePermissions {
                namespace: NamespaceId::parse("subject").unwrap(),
                keys: vec![
                    PermissionKey::parse("subject.orchestrate").unwrap(),
                    PermissionKey::parse("subject.delegate").unwrap(),
                ],
            })],
            ..Default::default()
        },
    ] {
        assert_exact_policy(&registry, &policy, &[]);
    }
}

#[test]
fn semantic_sets_keep_ownership_and_exclusions_separate() {
    let registry = registry();
    let reference =
        PermissionReference::Set(PermissionSetId::parse(WORK_COORDINATION_PERMISSION_SET).unwrap());
    let policy = assignment(WORK_COORDINATION_PERMISSION_SET);
    assert!(allows(&registry, &policy, "worker", "worker.create"));
    assert!(allows(&registry, &policy, "objective", "objective.read"));
    assert!(!allows(&registry, &policy, "bash", "bash.execute"));
    assert!(!allows(&registry, &policy, "subjektiv-chat", "chat.send"));
    assert!(
        registry
            .namespace_handle(
                &FeatureId::builtin("subject"),
                &NamespaceId::parse("subject").unwrap(),
            )
            .is_none()
    );
    let worker = registry
        .namespace_handle(
            &FeatureId::builtin("worker"),
            &NamespaceId::parse("worker").unwrap(),
        )
        .unwrap();
    assert!(matches!(
        registry.host().allows(
            worker,
            &registry.host().resolve(Some(&policy)),
            &PermissionKey::parse("ticket.read").unwrap()
        ),
        Err(PermissionError::Ownership(_))
    ));
    let mut excluded = subject_default_permission_assignment();
    excluded.exclusions.push(reference);
    assert_exact_policy(&registry, &excluded, CONVERSATION_KEYS);
    excluded.exclusions = vec![PermissionReference::Set(
        PermissionSetId::parse(CONVERSATION_PERMISSION_SET).unwrap(),
    )];
    assert_exact_policy(&registry, &excluded, WORK_COORDINATION_KEYS);
}

#[test]
fn feature_owned_execution_contribution_updates_do_not_expand_an_issued_grant() {
    let mut registry = registry();
    let policy = subject_default_delegation_assignment();
    let before = registry.host().resolve(Some(&policy));
    let read = PermissionKey::parse("ticket.read").unwrap();
    let child = registry
        .host()
        .issue_delegation(&before, &[read.clone()])
        .unwrap();
    let ticket = registry
        .namespace_handle(
            &FeatureId::builtin("ticket"),
            &NamespaceId::parse("ticket").unwrap(),
        )
        .unwrap()
        .clone();
    let mut declaration = registry
        .host()
        .catalog()
        .namespaces
        .into_iter()
        .find(|n| n.namespace.as_str() == "ticket")
        .unwrap()
        .declaration;
    let contribution = declaration
        .sets
        .iter_mut()
        .find(|set| set.set.as_str() == WORK_EXECUTION_PERMISSION_SET)
        .unwrap();
    contribution
        .children
        .push(PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("ticket").unwrap(),
            keys: vec![PermissionKey::parse("ticket.edit_item").unwrap()],
        }));
    registry.update(&ticket, declaration.clone()).unwrap();
    assert!(allows(&registry, &policy, "ticket", "ticket.edit_item"));
    let current = registry.host().resolve(Some(&policy));
    let limited = registry
        .host()
        .resolve_limited(&current, &current, &child)
        .unwrap();
    assert_eq!(
        limited
            .projection()
            .allowed
            .keys()
            .cloned()
            .collect::<Vec<_>>(),
        vec![read.clone()]
    );
    for contribution in declaration
        .sets
        .iter_mut()
        .filter(|set| set.set.as_str() == WORK_EXECUTION_PERMISSION_SET)
    {
        for reference in &mut contribution.children {
            if let PermissionReference::Namespace(namespace) = reference {
                namespace.keys.retain(|key| key != &read);
            }
        }
    }
    registry.update(&ticket, declaration).unwrap();
    let current = registry.host().resolve(Some(&policy));
    let limited = registry
        .host()
        .resolve_limited(&current, &current, &child)
        .unwrap();
    assert!(limited.projection().allowed.is_empty());
}

#[test]
fn subject_defaults_do_not_override_unset_empty_individual_excluded_or_revoked_policies() {
    let registry = registry();
    assert!(
        registry
            .host()
            .resolve(None)
            .projection()
            .allowed
            .is_empty()
    );
    assert_exact_policy(&registry, &PermissionAssignment::default(), &[]);
    let ticket_read = PermissionReference::Namespace(NamespacePermissions {
        namespace: NamespaceId::parse("ticket").unwrap(),
        keys: vec![PermissionKey::parse("ticket.read").unwrap()],
    });
    assert_exact_policy(
        &registry,
        &PermissionAssignment {
            grants: vec![ticket_read.clone()],
            ..Default::default()
        },
        &["ticket.read"],
    );
    let body: Vec<_> = CONVERSATION_KEYS
        .iter()
        .chain(WORK_COORDINATION_KEYS)
        .copied()
        .collect();
    for (policy, expected) in [
        (subject_default_permission_assignment(), body.as_slice()),
        (subject_default_delegation_assignment(), WORK_EXECUTION_KEYS),
        (assignment(CONVERSATION_PERMISSION_SET), CONVERSATION_KEYS),
        (
            assignment(WORK_COORDINATION_PERMISSION_SET),
            WORK_COORDINATION_KEYS,
        ),
    ] {
        let mut excluded = policy.clone();
        excluded.exclusions.push(ticket_read.clone());
        let remaining: Vec<_> = expected
            .iter()
            .copied()
            .filter(|key| *key != "ticket.read")
            .collect();
        assert_exact_policy(&registry, &excluded, &remaining);
        let mut excluded_set = policy.clone();
        excluded_set.exclusions = policy.grants.clone();
        assert_exact_policy(&registry, &excluded_set, &[]);
        let mut revoked = policy;
        revoked.revoked = true;
        assert_exact_policy(&registry, &revoked, &[]);
    }
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
    assert!(
        registry
            .namespace_handle(
                &FeatureId::builtin("sub-worker-control"),
                &NamespaceId::parse("subworker").unwrap(),
            )
            .is_none()
    );
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
        &assignment(WORK_COORDINATION_PERMISSION_SET),
        "ticket",
        "ticket.assignment.update"
    ));
    assert_exact_policy(&runtime, &assignment(CONVERSATION_PERMISSION_SET), &[]);
    // This isolated Ticket-only composition has no conversation contributor.
    // An unknown set invalidates the full assignment; coordination cannot bypass it.
    let defaults = runtime
        .host()
        .resolve(Some(&subject_default_permission_assignment()));
    assert!(defaults.projection().allowed.is_empty());
    assert_eq!(
        defaults.projection().diagnostics,
        vec![PermissionDiagnostic::UnknownSet(
            PermissionSetId::parse(CONVERSATION_PERMISSION_SET).unwrap()
        )]
    );
    assert!(allows(
        &registry(),
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
            CONVERSATION_PERMISSION_SET,
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
        sets: [
            COMPANION_PERMISSION_SET,
            CONVERSATION_PERMISSION_SET,
            WORK_COORDINATION_PERMISSION_SET,
            WORK_EXECUTION_PERMISSION_SET,
        ]
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
    for set in [
        CONVERSATION_PERMISSION_SET,
        WORK_COORDINATION_PERMISSION_SET,
        WORK_EXECUTION_PERMISSION_SET,
    ] {
        let mut policy = assignment(set);
        assert!(
            registry
                .host()
                .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
                .unwrap()
        );
        policy.exclusions = policy.grants.clone();
        assert!(
            !registry
                .host()
                .allows(&handle, &registry.host().resolve(Some(&policy)), &key)
                .unwrap()
        );
    }
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
