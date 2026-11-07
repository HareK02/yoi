use super::*;
use crate::store::{ControlPlaneStore, WorkspaceRecord};
use ::feature_permissions::{PermissionAssignment, WorkspaceId};

fn assignment() -> PermissionAssignment {
    PermissionAssignment {
        grants: vec![PermissionReference::Set(
            PermissionSetId::parse("companion").unwrap(),
        )],
        exclusions: vec![],
        revoked: false,
    }
}
fn declarations(namespace: &str) -> Declarations {
    let namespace = NamespaceId::parse(namespace).unwrap();
    let root = PermissionKey::parse(format!("{namespace}.use")).unwrap();
    let leaf = PermissionKey::parse(format!("{namespace}.use.deep")).unwrap();
    BTreeMap::from([(
        namespace.clone(),
        NamespaceDeclaration {
            permissions: vec![
                PermissionDefinition {
                    key: root.clone(),
                    description: "use".into(),
                    children: vec![leaf.clone()],
                },
                PermissionDefinition {
                    key: leaf,
                    description: "explicit child".into(),
                    children: vec![],
                },
                PermissionDefinition {
                    key: PermissionKey::parse(format!("{namespace}.use.unrelated")).unwrap(),
                    description: "not a child".into(),
                    children: vec![],
                },
            ],
            sets: vec![SetContribution {
                set: PermissionSetId::parse("companion").unwrap(),
                children: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace,
                    keys: vec![root],
                })],
            }],
        },
    )])
}
fn has(registry: &FeaturePermissionRegistry, key: &str) -> bool {
    registry
        .host()
        .resolve(Some(&assignment()))
        .projection()
        .allowed
        .contains_key(&PermissionKey::parse(key).unwrap())
}

#[test]
fn trusted_reconciliation_preserves_current_sets_and_frozen_child_job_and_callee_boundaries() {
    let mut registry =
        worker::feature::permissions::builtin_permission_registry(WorkspaceId::parse("a").unwrap())
            .unwrap();
    reconcile(&mut registry, "a", BTreeMap::new()).unwrap();
    let parent = registry.host().resolve(Some(&assignment()));
    let read = PermissionKey::parse("ticket.read").unwrap();
    let child = registry
        .host()
        .issue_delegation(&parent, &[read.clone()])
        .unwrap();
    let job = registry
        .host()
        .issue_job_grant("job".into(), &parent, &[read])
        .unwrap();
    let id = format!("{LOADER_PREFIX}installed:artifact");
    let selected = BTreeMap::from([(id.clone(), declarations("example"))]);
    reconcile(&mut registry, "a", selected.clone()).unwrap();
    assert!(has(&registry, "example.use.deep"));
    assert!(
        !has(&registry, "example.use.unrelated"),
        "dot prefix is not a children edge"
    );
    assert!(has(&registry, "ticket.read"));
    let parent = registry.host().resolve(Some(&assignment()));
    let limited = registry
        .host()
        .resolve_limited(&parent, &parent, &child)
        .unwrap();
    assert!(
        !limited
            .projection()
            .allowed
            .contains_key(&PermissionKey::parse("example.use").unwrap())
    );
    let limited_job = registry
        .host()
        .resolve_job("job", &parent, &parent, &child, &job)
        .unwrap();
    assert!(
        !limited_job
            .projection()
            .allowed
            .contains_key(&PermissionKey::parse("example.use").unwrap())
    );
    let handle = registry
        .namespace_handle(
            &FeatureId::new(&id).unwrap(),
            &NamespaceId::parse("example").unwrap(),
        )
        .unwrap()
        .clone();
    assert!(
        registry
            .host()
            .allows(
                &handle,
                &parent,
                &PermissionKey::parse("example.use.deep").unwrap()
            )
            .unwrap()
    );
    assert!(
        registry
            .host()
            .allows(
                &handle,
                &parent,
                &PermissionKey::parse("plugin.admit").unwrap()
            )
            .is_err(),
        "caller namespace cannot stand in for callee"
    );
    registry
        .set_enabled(&FeatureId::new(&id).unwrap(), false)
        .unwrap();
    assert!(!has(&registry, "example.use"));
    assert!(has(&registry, "ticket.read"));
    reconcile(&mut registry, "a", BTreeMap::new()).unwrap();
    assert!(!has(&registry, "example.use"));
    let now = registry.host().resolve(Some(&assignment()));
    assert!(
        registry
            .host()
            .allows(&handle, &now, &PermissionKey::parse("example.use").unwrap())
            .is_err()
    );
    assert!(reconcile(&mut registry, "b", selected).is_err());
    assert!(has(&registry, "ticket.read"));
}

fn package(namespace: &str) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let mut manifest: plugin::PluginPackageManifest = toml::from_str(include_str!(
        "../../../../../resources/plugin/templates/rust-component-tool/plugin.toml"
    ))
    .unwrap();
    manifest.permission_namespaces = declarations(namespace);
    fs::write(
        dir.path().join("plugin.toml"),
        toml::to_string(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        dir.path().join("plugin.component.wasm"),
        include_bytes!("../../../../plugin-pdk/tests/fixtures/tool-v2.component.wasm"),
    )
    .unwrap();
    let parsed = plugin::read_plugin_directory(
        dir.path(),
        PluginSourceKind::Explicit,
        &PluginPackageLimits::default(),
    )
    .unwrap();
    plugin::pack_plugin_package(&parsed, &PluginPackageLimits::default())
        .unwrap()
        .bytes
}
fn reference(installation: &PluginOperationResponse) -> PluginReference {
    let artifact = &installation.installation.artifacts[0];
    PluginReference {
        kind: PluginReferenceKind::Config,
        owner: "workspace-config".into(),
        installation_id: installation.installation.installation_id.clone(),
        expected_last_operation: installation.operation_id.clone(),
        package_digest: artifact.package_digest.clone(),
        artifact_digest: artifact.artifact_digest.clone(),
    }
}

#[tokio::test]
async fn adopted_catalog_bytes_add_disable_reopen_revoke_and_remove_only_their_contribution() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("server.db");
    let root = dir.path().join("artifacts");
    let store = SqliteWorkspaceStore::open_for_test(&db).unwrap();
    store
        .upsert_account(&crate::store::AccountRecord {
            account_id: "owner".into(),
            kind: "user".into(),
            handle: "plugin-permissions-owner".into(),
            display_name: "Plugin permissions owner".into(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
        })
        .unwrap();
    store
        .upsert_workspace(&WorkspaceRecord {
            workspace_id: "a".into(),
            owner_account_id: "owner".into(),
            display_name: "a".into(),
            state: "active".into(),
            created_at: "2026-10-01T00:00:00Z".into(),
            updated_at: "2026-10-01T00:00:00Z".into(),
        })
        .await
        .unwrap();
    let mut registry =
        worker::feature::permissions::builtin_permission_registry(WorkspaceId::parse("a").unwrap())
            .unwrap();
    let mut installed = Vec::new();
    for namespace in ["external_one", "external_two"] {
        let upload = store
            .plugin_upload(&root, "a", "owner", &package(namespace))
            .unwrap();
        store
            .sync_plugin_permission_registry(&root, "a", &mut registry)
            .unwrap();
        assert!(
            !has(&registry, &format!("{namespace}.use")),
            "upload is not trust/enablement"
        );
        let installation = store
            .plugin_install(
                "a",
                "owner",
                &PluginInstallRequest {
                    upload_id: upload.upload_id,
                    alias: namespace.into(),
                    operation_key: namespace.into(),
                },
            )
            .unwrap();
        store
            .sync_plugin_permission_registry(&root, "a", &mut registry)
            .unwrap();
        assert!(
            !has(&registry, &format!("{namespace}.use")),
            "installation without adopted enablement is not a grant"
        );
        store
            .with_immediate_transaction(|tx| {
                plugin_reference_put(tx, "a", &reference(&installation))
            })
            .unwrap();
        store
            .sync_plugin_permission_registry(&root, "a", &mut registry)
            .unwrap();
        assert!(has(&registry, &format!("{namespace}.use.deep")));
        installed.push(installation);
    }
    // This test substitutes only the owner config adoption's existing typed pin
    // publication. No schema, new writer, or per-Subject redistributions exist.
    let first = &installed[0];
    let pin = reference(first);
    store
        .with_immediate_transaction(|tx| {
            plugin_reference_remove(
                tx,
                "a",
                pin.kind,
                &pin.owner,
                &pin.installation_id,
                &pin.artifact_digest,
            )
        })
        .unwrap();
    store
        .sync_plugin_permission_registry(&root, "a", &mut registry)
        .unwrap();
    assert!(!has(&registry, "external_one.use"));
    assert!(has(&registry, "external_two.use"));
    store
        .with_immediate_transaction(|tx| plugin_reference_put(tx, "a", &pin))
        .unwrap();
    drop(store);
    let store = SqliteWorkspaceStore::open_for_test(&db).unwrap();
    let mut reopened =
        worker::feature::permissions::builtin_permission_registry(WorkspaceId::parse("a").unwrap())
            .unwrap();
    store
        .sync_plugin_permission_registry(&root, "a", &mut reopened)
        .unwrap();
    assert!(has(&reopened, "external_one.use.deep"));
    let revoked = store
        .plugin_revoke(
            "a",
            "owner",
            &pin.installation_id,
            &PluginRevokeRequest {
                expected_last_operation: first.operation_id.clone(),
                artifact_digest: pin.artifact_digest.clone(),
                reason: "withdraw".into(),
                operation_key: "revoke".into(),
            },
        )
        .unwrap();
    store
        .sync_plugin_permission_registry(&root, "a", &mut reopened)
        .unwrap();
    assert!(!has(&reopened, "external_one.use"));
    assert!(has(&reopened, "external_two.use"));
    store
        .with_immediate_transaction(|tx| {
            plugin_reference_remove(
                tx,
                "a",
                pin.kind,
                &pin.owner,
                &pin.installation_id,
                &pin.artifact_digest,
            )
        })
        .unwrap();
    store
        .plugin_uninstall(
            "a",
            "owner",
            &pin.installation_id,
            &PluginUninstallRequest {
                expected_last_operation: revoked.operation_id,
                operation_key: "uninstall".into(),
            },
        )
        .unwrap();
    store
        .sync_plugin_permission_registry(&root, "a", &mut reopened)
        .unwrap();
    assert!(!has(&reopened, "external_one.use"));
    assert!(has(&reopened, "external_two.use"));
    assert!(has(&reopened, "ticket.read"));
}

#[test]
fn cached_manifest_rejects_changed_or_missing_artifact_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut bytes = package("cached");
    let digest = format!("sha256:{}", hash(&bytes));
    publish_blob(root, &digest, &bytes).unwrap();
    let cache = crate::plugin_catalog::ArtifactManifestCache::default();
    let namespace = NamespaceId::parse("cached").unwrap();
    for _ in 0..2 {
        assert!(
            cache
                .read(root, &digest, bytes.len() as u64)
                .unwrap()
                .permission_namespaces
                .contains_key(&namespace)
        );
    }
    bytes[0] ^= 1;
    let path = blob_path(root, &digest).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(cache.read(root, &digest, bytes.len() as u64).is_err());
    fs::remove_file(path).unwrap();
    assert!(cache.read(root, &digest, bytes.len() as u64).is_err());
}

#[test]
fn package_declaration_rejects_owner_spoof_payload_and_cycles() {
    let mut manifest: plugin::PluginPackageManifest = toml::from_str(include_str!(
        "../../../../../resources/plugin/templates/rust-component-tool/plugin.toml"
    ))
    .unwrap();
    manifest.permission_namespaces = declarations("example");
    let ns = NamespaceId::parse("example").unwrap();
    manifest
        .permission_namespaces
        .get_mut(&ns)
        .unwrap()
        .permissions[0]
        .key = PermissionKey::parse("ticket.read").unwrap();
    assert!(manifest.validate_permission_declarations().is_err());
    manifest.permission_namespaces = declarations("example");
    manifest
        .permission_namespaces
        .get_mut(&ns)
        .unwrap()
        .permissions[1]
        .children
        .push(PermissionKey::parse("example.use").unwrap());
    assert!(manifest.validate_permission_declarations().is_err());
    assert!(
        serde_json::from_value::<NamespaceDeclaration>(
            serde_json::json!({"permissions":[],"sets":[],"owner":"builtin:ticket"})
        )
        .is_err()
    );
}
