//! Core component tests through the public API only. These do not stand in for
//! execution-context/provider integration or actual operation authorization.

use feature_permissions::*;
use std::collections::BTreeSet;

fn key(value: &str) -> PermissionKey {
    PermissionKey::parse(value).unwrap()
}

fn namespace(value: &str) -> NamespaceId {
    NamespaceId::parse(value).unwrap()
}

fn set(value: &str) -> PermissionReference {
    PermissionReference::Set(PermissionSetId::parse(value).unwrap())
}

fn keys(ns: &str, values: &[&str]) -> PermissionReference {
    PermissionReference::Namespace(NamespacePermissions {
        namespace: namespace(ns),
        keys: values.iter().map(|value| key(value)).collect(),
    })
}

fn assignment(grants: Vec<PermissionReference>) -> PermissionAssignment {
    PermissionAssignment {
        grants,
        ..PermissionAssignment::default()
    }
}

fn host(workspace: &str) -> PermissionHost {
    PermissionHost::new(WorkspaceId::parse(workspace).unwrap())
}

fn register(
    host: &mut PermissionHost,
    identity: &str,
    ns: &str,
) -> (FeatureHandle, NamespaceHandle) {
    let feature = host
        .register_feature(FeatureIdentity::parse(identity).unwrap())
        .unwrap();
    let handle = host.bind_namespace(&feature, namespace(ns)).unwrap();
    (feature, handle)
}

fn declaration(
    definitions: &[(&str, &[&str])],
    contributions: &[(&str, Vec<PermissionReference>)],
) -> NamespaceDeclaration {
    NamespaceDeclaration {
        permissions: definitions
            .iter()
            .map(|(name, children)| PermissionDefinition {
                key: key(name),
                description: format!("Operation {name}"),
                children: children.iter().map(|child| key(child)).collect(),
            })
            .collect(),
        sets: contributions
            .iter()
            .map(|(name, children)| SetContribution {
                set: PermissionSetId::parse(*name).unwrap(),
                children: children.clone(),
            })
            .collect(),
    }
}

fn assert_keys(resolved: &ResolvedPermissions, expected: &[&str]) {
    assert_eq!(
        resolved
            .projection()
            .allowed
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        expected.iter().map(|value| key(value)).collect()
    );
}

fn has_origin(resolved: &ResolvedPermissions, operation: &str, feature: &str, set_name: &str) {
    let paths = &resolved.projection().allowed[&key(operation)];
    assert!(!paths.is_empty());
    for path in paths {
        assert!(path.contains(&OriginStep::Feature(
            FeatureIdentity::parse(feature).unwrap()
        )));
        assert!(path.contains(&OriginStep::Set(PermissionSetId::parse(set_name).unwrap())));
        assert_eq!(path.last(), Some(&OriginStep::Key(key(operation))));
    }
}

#[test]
fn existing_set_holder_tracks_two_builtins_and_trusted_plugin_without_redistribution() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    let (_, workdir) = register(&mut host, "builtin:workdir", "workdir");
    host.declare(
        &ticket,
        declaration(
            &[("ticket.read", &[]), ("ticket.write", &[])],
            &[("companion", vec![keys("ticket", &["ticket.read"])])],
        ),
    )
    .unwrap();
    host.declare(
        &workdir,
        declaration(
            &[("workdir.read", &[])],
            &[("companion", vec![keys("workdir", &["workdir.read"])])],
        ),
    )
    .unwrap();
    // This same stored assignment is reused throughout; no per-subject update.
    let holder = assignment(vec![set("companion"), keys("ticket", &["ticket.write"])]);
    let before = host.resolve(Some(&holder));
    assert_keys(&before, &["ticket.read", "ticket.write", "workdir.read"]);

    let (plugin_feature, plugin) = register(&mut host, "plugin:external-search@1", "search");
    host.declare(
        &plugin,
        declaration(
            &[("search.query.deep", &[])],
            &[("companion", vec![keys("search", &["search.query.deep"])])],
        ),
    )
    .unwrap();
    let installed = host.resolve(Some(&holder));
    assert_keys(
        &installed,
        &[
            "ticket.read",
            "ticket.write",
            "workdir.read",
            "search.query.deep",
        ],
    );
    has_origin(&installed, "ticket.read", "builtin:ticket", "companion");
    has_origin(&installed, "workdir.read", "builtin:workdir", "companion");
    has_origin(
        &installed,
        "search.query.deep",
        "plugin:external-search@1",
        "companion",
    );
    assert_eq!(
        host.allows(&ticket, &before, &key("ticket.read")),
        Err(PermissionError::StaleResolution)
    );

    host.set_enabled(&plugin_feature, false).unwrap();
    let disabled = host.resolve(Some(&holder));
    assert_keys(&disabled, &["ticket.read", "ticket.write", "workdir.read"]);
    assert_eq!(
        disabled.projection().diagnostics,
        vec![PermissionDiagnostic::DisabledFeature(
            FeatureIdentity::parse("plugin:external-search@1").unwrap()
        )]
    );
    let direct_plugin = assignment(vec![keys("search", &["search.query.deep"])]);
    assert_keys(&host.resolve(Some(&direct_plugin)), &[]);
    host.set_enabled(&plugin_feature, true).unwrap();
    assert_keys(
        &host.resolve(Some(&holder)),
        &[
            "ticket.read",
            "ticket.write",
            "workdir.read",
            "search.query.deep",
        ],
    );
    host.remove_feature(&plugin_feature).unwrap();
    let removed = host.resolve(Some(&holder));
    assert_keys(&removed, &["ticket.read", "ticket.write", "workdir.read"]);
    assert!(removed.projection().diagnostics.is_empty());
    assert!(
        !host
            .catalog()
            .namespaces
            .iter()
            .any(|ns| ns.namespace == namespace("search"))
    );
    assert_eq!(
        host.allows(&plugin, &removed, &key("search.query.deep")),
        Err(PermissionError::ForeignHandle)
    );
}

#[test]
fn ownership_comes_from_handles_not_raw_owner_fields_or_namespace_spelling() {
    let mut host = host("workspace-a");
    let (builtin, ticket) = register(&mut host, "builtin:ticket", "ticket");
    let (plugin, search) = register(&mut host, "plugin:search", "search");
    let baseline = host.catalog();
    assert_eq!(
        host.bind_namespace(&plugin, namespace("ticket"))
            .unwrap_err(),
        PermissionError::NamespaceCollision(namespace("ticket"))
    );
    assert_eq!(
        host.register_feature(FeatureIdentity::parse("builtin:ticket").unwrap())
            .unwrap_err(),
        PermissionError::FeatureCollision(FeatureIdentity::parse("builtin:ticket").unwrap())
    );
    assert_eq!(
        host.declare(&search, declaration(&[("ticket.read", &[])], &[])),
        Err(PermissionError::Ownership(key("ticket.read")))
    );
    // The rule is symmetric: builtin identity is not permission to impersonate a Plugin.
    assert_eq!(
        host.declare(&ticket, declaration(&[("search.query", &[])], &[])),
        Err(PermissionError::Ownership(key("search.query")))
    );
    // A set contribution is no alternate way to declare another Feature's keys.
    assert_eq!(
        host.declare(
            &search,
            declaration(
                &[],
                &[("companion", vec![keys("ticket", &["ticket.read"])])]
            ),
        ),
        Err(PermissionError::ForeignHandle)
    );
    assert_eq!(
        host.declare(
            &search,
            declaration(
                &[],
                &[("companion", vec![keys("search", &["ticket.read"])])]
            ),
        ),
        Err(PermissionError::Ownership(key("ticket.read")))
    );
    assert!(
        serde_json::from_str::<NamespaceDeclaration>(
            r#"{"owner":"builtin:ticket","permissions":[],"sets":[]}"#
        )
        .is_err()
    );
    assert_eq!(host.catalog(), baseline);
    host.remove_feature(&builtin).unwrap();
    let (_, replacement) = register(&mut host, "builtin:ticket", "ticket");
    assert_eq!(
        host.declare(&ticket, declaration(&[], &[])),
        Err(PermissionError::ForeignHandle)
    );
    host.declare(&replacement, declaration(&[("ticket.read", &[])], &[]))
        .unwrap();
}

#[test]
fn namespace_registration_and_empty_namespace_assignment_do_not_grant_operations() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, declaration(&[("ticket.read", &[])], &[]))
        .unwrap();
    for policy in [None, Some(assignment(vec![keys("ticket", &[])]))] {
        let resolved = host.resolve(policy.as_ref());
        assert_keys(&resolved, &[]);
        assert_eq!(
            host.allows(&ticket, &resolved, &key("ticket.read")),
            Ok(false)
        );
    }
}

#[test]
fn handles_and_results_are_not_portable_even_between_hosts_with_the_same_workspace_id() {
    let mut original = host("workspace-a");
    let (feature, ticket) = register(&mut original, "builtin:ticket", "ticket");
    original
        .declare(&ticket, declaration(&[("ticket.read", &[])], &[]))
        .unwrap();
    let policy = assignment(vec![keys("ticket", &["ticket.read"])]);
    let resolved = original.resolve(Some(&policy));
    let child = original
        .issue_delegation(&resolved, &[key("ticket.read")])
        .unwrap();
    let job = original
        .issue_job_grant("job-a".into(), &resolved, &[key("ticket.read")])
        .unwrap();
    for workspace in ["workspace-a", "workspace-b"] {
        let mut other = host(workspace);
        let (_, local) = register(&mut other, "builtin:ticket", "ticket");
        other
            .declare(&local, declaration(&[("ticket.read", &[])], &[]))
            .unwrap();
        let local_result = other.resolve(Some(&policy));
        let local_child = other
            .issue_delegation(&local_result, &[key("ticket.read")])
            .unwrap();
        assert_eq!(
            other
                .bind_namespace(&feature, namespace("foreign"))
                .unwrap_err(),
            PermissionError::ForeignHandle
        );
        assert_eq!(
            other.set_enabled(&feature, false),
            Err(PermissionError::ForeignHandle)
        );
        assert_eq!(
            other.remove_feature(&feature),
            Err(PermissionError::ForeignHandle)
        );
        assert_eq!(
            other.declare(&ticket, declaration(&[], &[])),
            Err(PermissionError::ForeignHandle)
        );
        assert_eq!(
            other.allows(&ticket, &local_result, &key("ticket.read")),
            Err(PermissionError::ForeignHandle)
        );
        assert_eq!(
            other.allows(&local, &resolved, &key("ticket.read")),
            Err(PermissionError::ForeignHandle)
        );
        assert_eq!(
            other
                .issue_delegation(&resolved, &[key("ticket.read")])
                .unwrap_err(),
            PermissionError::ForeignHandle
        );
        assert_eq!(
            other
                .resolve_limited(&local_result, &local_result, &child)
                .unwrap_err(),
            PermissionError::ForeignHandle
        );
        assert_eq!(
            other
                .resolve_job("job-a", &local_result, &local_result, &local_child, &job)
                .unwrap_err(),
            PermissionError::ForeignHandle
        );
    }
}

fn diamond() -> NamespaceDeclaration {
    declaration(
        &[
            ("ticket.hoge", &["ticket.hoge.fuga", "ticket.branch"]),
            ("ticket.hoge.fuga", &["ticket.hoge.fuga.deep"]),
            ("ticket.branch", &["ticket.hoge.fuga.deep"]),
            ("ticket.hoge.fuga.deep", &[]),
            ("ticket.hoge.unrelated", &[]),
        ],
        &[("companion", vec![keys("ticket", &["ticket.hoge"])])],
    )
}

#[test]
fn explicit_multilevel_children_keep_distinct_paths_but_never_infer_a_prefix_grant() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, diamond()).unwrap();
    let policy = assignment(vec![set("companion"), set("companion")]);
    let resolved = host.resolve(Some(&policy));
    assert_keys(
        &resolved,
        &[
            "ticket.hoge",
            "ticket.hoge.fuga",
            "ticket.branch",
            "ticket.hoge.fuga.deep",
        ],
    );
    assert_eq!(
        host.allows(&ticket, &resolved, &key("ticket.hoge.unrelated")),
        Ok(false)
    );
    let paths = &resolved.projection().allowed[&key("ticket.hoge.fuga.deep")];
    assert_eq!(
        paths.len(),
        2,
        "duplicate grants must not duplicate provenance"
    );
    let key_paths: BTreeSet<Vec<_>> = paths
        .iter()
        .map(|path| {
            path.iter()
                .filter_map(|step| match step {
                    OriginStep::Key(key) => Some(key.clone()),
                    _ => None,
                })
                .collect()
        })
        .collect();
    assert_eq!(
        key_paths,
        BTreeSet::from([
            vec![
                key("ticket.hoge"),
                key("ticket.hoge.fuga"),
                key("ticket.hoge.fuga.deep")
            ],
            vec![
                key("ticket.hoge"),
                key("ticket.branch"),
                key("ticket.hoge.fuga.deep")
            ],
        ])
    );
    has_origin(
        &resolved,
        "ticket.hoge.fuga.deep",
        "builtin:ticket",
        "companion",
    );
    assert_eq!(
        resolved.projection(),
        host.resolve(Some(&policy)).projection(),
        "resolution and provenance ordering must be deterministic"
    );
}

#[test]
fn invalid_graph_updates_reject_atomically_without_retiring_a_valid_result() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, diamond()).unwrap();
    let baseline = host.catalog();
    let policy = assignment(vec![set("companion")]);
    let valid = host.resolve(Some(&policy));
    let mut dangling_removal = diamond();
    dangling_removal
        .permissions
        .retain(|definition| definition.key != key("ticket.hoge.fuga.deep"));
    let invalid = [
        (dangling_removal, "unknown_key"),
        (declaration(&[("ticket.a", &["ticket.a"])], &[]), "cycle"),
        (
            declaration(
                &[("ticket.a", &["ticket.b"]), ("ticket.b", &["ticket.a"])],
                &[],
            ),
            "cycle",
        ),
        (
            declaration(&[("ticket.a", &["ticket.missing"])], &[]),
            "unknown_key",
        ),
        (
            declaration(&[], &[("a", vec![set("missing")])]),
            "unknown_set",
        ),
        (
            declaration(&[], &[("a", vec![set("b")]), ("b", vec![set("a")])]),
            "cycle",
        ),
        (
            declaration(&[("ticket.a", &[]), ("ticket.a", &[])], &[]),
            "duplicate",
        ),
        (
            declaration(
                &[("ticket.a", &["ticket.b", "ticket.b"]), ("ticket.b", &[])],
                &[],
            ),
            "duplicate",
        ),
        (
            declaration(&[], &[("a", vec![]), ("a", vec![])]),
            "duplicate",
        ),
        (
            declaration(&[], &[("a", vec![keys("ticket", &["ticket.missing"])])]),
            "unknown_key",
        ),
    ];
    for (candidate, expected) in invalid {
        let error = host.declare(&ticket, candidate).unwrap_err();
        assert!(
            matches!(
                (&error, expected),
                (PermissionError::Cycle(_), "cycle")
                    | (PermissionError::UnknownKey(_), "unknown_key")
                    | (PermissionError::UnknownSet(_), "unknown_set")
                    | (PermissionError::Duplicate(_), "duplicate")
            ),
            "unexpected rejection: {error:?} for {expected}"
        );
        assert_eq!(host.catalog(), baseline);
        assert_eq!(
            host.allows(&ticket, &valid, &key("ticket.hoge.fuga.deep")),
            Ok(true)
        );
        assert_eq!(host.resolve(Some(&policy)).projection(), valid.projection());
    }
}

#[test]
fn changing_and_removing_children_withdraws_old_permissions_and_rejects_stale_results() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, diamond()).unwrap();
    let policy = assignment(vec![keys("ticket", &["ticket.hoge"])]);
    let old = host.resolve(Some(&policy));
    host.declare(
        &ticket,
        declaration(&[("ticket.hoge", &[]), ("ticket.hoge.fuga", &[])], &[]),
    )
    .unwrap();
    assert_eq!(
        host.allows(&ticket, &old, &key("ticket.hoge.fuga.deep")),
        Err(PermissionError::StaleResolution)
    );
    let fresh = host.resolve(Some(&policy));
    assert_keys(&fresh, &["ticket.hoge"]);
    assert_eq!(
        host.allows(&ticket, &fresh, &key("ticket.hoge.fuga")),
        Ok(false)
    );
    let withdrawn = assignment(vec![keys("ticket", &["ticket.hoge.fuga.deep"])]);
    let withdrawn_result = host.resolve(Some(&withdrawn));
    assert_keys(&withdrawn_result, &[]);
    assert_eq!(
        withdrawn_result.projection().diagnostics,
        vec![PermissionDiagnostic::UnknownKey(key(
            "ticket.hoge.fuga.deep"
        ))]
    );
    host.declare(&ticket, declaration(&[], &[])).unwrap();
    assert_keys(&host.resolve(Some(&policy)), &[]);
}

#[test]
fn exclusions_expand_children_and_win_over_all_paths_without_reviving_defaults() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, diamond()).unwrap();
    let mut policy = assignment(vec![
        set("companion"),
        keys("ticket", &["ticket.hoge.fuga.deep"]),
    ]);
    policy.exclusions = vec![keys("ticket", &["ticket.hoge.fuga"])];
    let resolved = host.resolve(Some(&policy));
    assert_keys(&resolved, &["ticket.hoge", "ticket.branch"]);
    assert_eq!(
        resolved.projection().diagnostics,
        vec![
            PermissionDiagnostic::Excluded(key("ticket.hoge.fuga")),
            PermissionDiagnostic::Excluded(key("ticket.hoge.fuga.deep")),
        ]
    );
    // A child request expands through the same graph and cannot bypass exclusion.
    assert_eq!(
        host.issue_delegation(&resolved, &[key("ticket.hoge")])
            .unwrap_err(),
        PermissionError::DelegationExceeded(key("ticket.hoge.fuga"))
    );
    policy.exclusions = vec![set("companion")];
    assert_keys(&host.resolve(Some(&policy)), &[]);
    policy.revoked = true;
    let revoked = host.resolve(Some(&policy));
    assert_keys(&revoked, &[]);
    assert_eq!(
        revoked.projection().diagnostics,
        vec![PermissionDiagnostic::Revoked]
    );
    let unset = host.resolve(None);
    assert_keys(&unset, &[]);
    assert_eq!(
        unset.projection().diagnostics,
        vec![PermissionDiagnostic::Unset]
    );
    let empty = host.resolve(Some(&PermissionAssignment::default()));
    assert_keys(&empty, &[]);
    assert!(empty.projection().diagnostics.is_empty());
}

#[test]
fn invalid_assignment_references_deny_the_whole_assignment_with_deterministic_diagnostics() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(&ticket, diamond()).unwrap();
    for (invalid, diagnostic) in [
        (
            set("missing"),
            PermissionDiagnostic::UnknownSet(PermissionSetId::parse("missing").unwrap()),
        ),
        (
            keys("absent", &[]),
            PermissionDiagnostic::UnknownNamespace(namespace("absent")),
        ),
        (
            keys("ticket", &["ticket.missing"]),
            PermissionDiagnostic::UnknownKey(key("ticket.missing")),
        ),
        (
            keys("ticket", &["search.query"]),
            PermissionDiagnostic::UnknownKey(key("search.query")),
        ),
    ] {
        for as_exclusion in [false, true] {
            let mut policy = assignment(vec![set("companion")]);
            if as_exclusion {
                policy.exclusions = vec![invalid.clone(), invalid.clone()];
            } else {
                policy.grants.extend([invalid.clone(), invalid.clone()]);
            }
            let resolved = host.resolve(Some(&policy));
            assert_keys(&resolved, &[]);
            assert_eq!(resolved.projection().diagnostics, vec![diagnostic.clone()]);
            assert_eq!(
                resolved.projection(),
                host.resolve(Some(&policy)).projection()
            );
        }
    }
}

#[test]
fn removed_feature_leaves_dangling_children_diagnostic_instead_of_using_cached_permissions() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    let (plugin, search) = register(&mut host, "plugin:search", "search");
    host.declare(&search, declaration(&[("search.query", &[])], &[]))
        .unwrap();
    host.declare(
        &ticket,
        declaration(&[("ticket.read", &["search.query"])], &[]),
    )
    .unwrap();
    let policy = assignment(vec![keys("ticket", &["ticket.read"])]);
    let old = host.resolve(Some(&policy));
    assert_keys(&old, &["ticket.read", "search.query"]);
    host.remove_feature(&plugin).unwrap();
    assert_eq!(
        host.allows(&ticket, &old, &key("ticket.read")),
        Err(PermissionError::StaleResolution)
    );
    let fresh = host.resolve(Some(&policy));
    assert_keys(&fresh, &[]);
    assert_eq!(
        fresh.projection().diagnostics,
        vec![PermissionDiagnostic::UnknownKey(key("search.query"))]
    );
}

#[test]
fn feature_reregistration_does_not_revive_old_child_or_job_grants_for_identical_keys() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(
        &ticket,
        declaration(
            &[("ticket.read", &[])],
            &[("companion", vec![keys("ticket", &["ticket.read"])])],
        ),
    )
    .unwrap();
    let (plugin, search) = register(&mut host, "plugin:search", "search");
    let plugin_declaration = declaration(
        &[("search.query", &[])],
        &[("companion", vec![keys("search", &["search.query"])])],
    );
    host.declare(&search, plugin_declaration.clone()).unwrap();
    let holder = assignment(vec![set("companion")]);
    let requested = [key("ticket.read"), key("search.query")];
    let original = host.resolve(Some(&holder));
    let old_child = host.issue_delegation(&original, &requested).unwrap();
    let old_job = host
        .issue_job_grant("job-a".into(), &original, &requested)
        .unwrap();
    assert_keys(
        &host
            .resolve_job("job-a", &original, &original, &old_child, &old_job)
            .unwrap(),
        &["ticket.read", "search.query"],
    );

    host.remove_feature(&plugin).unwrap();
    let removed = host.resolve(Some(&holder));
    assert_keys(&removed, &["ticket.read"]);
    assert_keys(
        &host
            .resolve_limited(&removed, &removed, &old_child)
            .unwrap(),
        &["ticket.read"],
    );
    // Reuse every public identifier and declaration, but not the registration.
    let (_, replacement) = register(&mut host, "plugin:search", "search");
    host.declare(&replacement, plugin_declaration).unwrap();
    let current = host.resolve(Some(&holder));
    assert_keys(&current, &["ticket.read", "search.query"]);
    assert_eq!(
        host.allows(&replacement, &current, &key("search.query")),
        Ok(true)
    );
    assert_eq!(
        host.allows(&replacement, &original, &key("search.query")),
        Err(PermissionError::StaleResolution)
    );
    // The key snapshot still includes search.query; its old registration is the
    // reason it cannot authorize the replacement. Unrelated Feature keys survive.
    assert_eq!(old_child.keys(), &BTreeSet::from(requested.clone()));
    assert_keys(
        &host
            .resolve_limited(&current, &current, &old_child)
            .unwrap(),
        &["ticket.read"],
    );
    let fresh_child = host.issue_delegation(&current, &requested).unwrap();
    let fresh_job = host
        .issue_job_grant("job-a".into(), &current, &requested)
        .unwrap();
    // A fresh child isolates the old Job token: the old child cannot mask a
    // missing registration check in JobGrant. Conversely use a fresh Job below.
    assert_keys(
        &host
            .resolve_job("job-a", &current, &current, &fresh_child, &old_job)
            .unwrap(),
        &["ticket.read"],
    );
    assert_keys(
        &host
            .resolve_job("job-a", &current, &current, &old_child, &fresh_job)
            .unwrap(),
        &["ticket.read"],
    );
    // Positive control: reissuance, not identifier reuse, grants the new operation.
    assert_keys(
        &host
            .resolve_job("job-a", &current, &current, &fresh_child, &fresh_job)
            .unwrap(),
        &["ticket.read", "search.query"],
    );
}

#[test]
fn issued_child_and_job_keys_are_frozen_across_set_and_children_expansion() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(
        &ticket,
        declaration(
            &[
                ("ticket.hoge", &["ticket.hoge.read"]),
                ("ticket.hoge.read", &[]),
                ("ticket.hoge.write", &[]),
            ],
            &[("companion", vec![keys("ticket", &["ticket.hoge"])])],
        ),
    )
    .unwrap();
    let holder = assignment(vec![set("companion")]);
    let old = host.resolve(Some(&holder));
    let child = host.issue_delegation(&old, &[key("ticket.hoge")]).unwrap();
    let job = host
        .issue_job_grant("job-a".into(), &old, &[key("ticket.hoge")])
        .unwrap();
    let narrow_job = host
        .issue_job_grant("job-b".into(), &old, &[key("ticket.hoge.read")])
        .unwrap();
    let frozen = BTreeSet::from([key("ticket.hoge"), key("ticket.hoge.read")]);
    assert_eq!(child.keys(), &frozen);

    host.declare(
        &ticket,
        declaration(
            &[
                ("ticket.hoge", &["ticket.hoge.read", "ticket.hoge.write"]),
                ("ticket.hoge.read", &[]),
                ("ticket.hoge.write", &[]),
            ],
            &[("companion", vec![keys("ticket", &["ticket.hoge"])])],
        ),
    )
    .unwrap();
    let (_, plugin) = register(&mut host, "plugin:search", "search");
    host.declare(
        &plugin,
        declaration(
            &[("search.query", &[])],
            &[("companion", vec![keys("search", &["search.query"])])],
        ),
    )
    .unwrap();
    assert_eq!(
        host.issue_delegation(&old, &[key("ticket.hoge")])
            .unwrap_err(),
        PermissionError::StaleResolution
    );
    assert_eq!(
        host.resolve_limited(&old, &old, &child).unwrap_err(),
        PermissionError::StaleResolution
    );
    let current = host.resolve(Some(&holder));
    assert_keys(
        &current,
        &[
            "ticket.hoge",
            "ticket.hoge.read",
            "ticket.hoge.write",
            "search.query",
        ],
    );
    assert_eq!(child.keys(), &frozen);
    let limited = host.resolve_limited(&current, &current, &child).unwrap();
    assert_keys(&limited, &["ticket.hoge", "ticket.hoge.read"]);
    assert_eq!(
        host.resolve_limited(&current, &old, &child).unwrap_err(),
        PermissionError::StaleResolution
    );
    assert_eq!(
        host.resolve_limited(&old, &current, &child).unwrap_err(),
        PermissionError::StaleResolution
    );
    assert_eq!(
        host.resolve_job("job-a", &current, &old, &child, &job)
            .unwrap_err(),
        PermissionError::StaleResolution
    );
    let current_child = host
        .issue_delegation(&current, &[key("ticket.hoge"), key("search.query")])
        .unwrap();
    assert_keys(
        &host
            .resolve_job("job-a", &current, &current, &current_child, &job)
            .unwrap(),
        &["ticket.hoge", "ticket.hoge.read"],
    );
    assert_keys(
        &host
            .resolve_job("job-b", &current, &current, &current_child, &narrow_job)
            .unwrap(),
        &["ticket.hoge.read"],
    );
    assert_eq!(
        host.resolve_job("another-job", &current, &current, &child, &job)
            .unwrap_err(),
        PermissionError::WrongJob
    );
}

#[test]
fn current_parent_execution_delegation_and_explicit_revocation_each_limit_children_and_jobs() {
    let mut host = host("workspace-a");
    let (_, ticket) = register(&mut host, "builtin:ticket", "ticket");
    host.declare(
        &ticket,
        declaration(&[("ticket.read", &[]), ("ticket.write", &[])], &[]),
    )
    .unwrap();
    let full_policy = assignment(vec![keys("ticket", &["ticket.read", "ticket.write"])]);
    let narrow_policy = assignment(vec![keys("ticket", &["ticket.read"])]);
    let revoked_policy = PermissionAssignment {
        revoked: true,
        ..full_policy.clone()
    };
    let full = host.resolve(Some(&full_policy));
    let narrow = host.resolve(Some(&narrow_policy));
    let revoked = host.resolve(Some(&revoked_policy));
    let mut child = host
        .issue_delegation(&full, &[key("ticket.read"), key("ticket.write")])
        .unwrap();
    let mut job = host
        .issue_job_grant(
            "job-a".into(),
            &full,
            &[key("ticket.read"), key("ticket.write")],
        )
        .unwrap();
    // Execution permission alone is not delegation authority (and vice versa).
    for (execution, delegation, expected) in [
        (&full, &narrow, vec!["ticket.read"]),
        (&narrow, &full, vec!["ticket.read"]),
        (&full, &revoked, vec![]),
        (&revoked, &full, vec![]),
    ] {
        assert_keys(
            &host.resolve_limited(execution, delegation, &child).unwrap(),
            &expected,
        );
        assert_keys(
            &host
                .resolve_job("job-a", execution, delegation, &child, &job)
                .unwrap(),
            &expected,
        );
    }
    assert_eq!(
        host.issue_delegation(&narrow, &[key("ticket.write")])
            .unwrap_err(),
        PermissionError::DelegationExceeded(key("ticket.write"))
    );
    assert_eq!(
        host.issue_delegation(&full, &[key("ticket.missing")])
            .unwrap_err(),
        PermissionError::UnknownKey(key("ticket.missing"))
    );
    job.revoke();
    assert_eq!(
        host.resolve_job("job-a", &full, &full, &child, &job)
            .unwrap_err(),
        PermissionError::RevokedGrant
    );
    assert_keys(
        &host.resolve_limited(&full, &full, &child).unwrap(),
        &["ticket.read", "ticket.write"],
    );
    child.revoke();
    assert_eq!(
        host.resolve_limited(&full, &full, &child).unwrap_err(),
        PermissionError::RevokedGrant
    );
}
