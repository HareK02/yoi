//! Real temporary Server SQLite/content-history coverage. Native Feature storage
//! is absent: this migration owns only current canonical Server data.
use super::*;
use crate::config_source::{MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider};
use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeChange, SnapshotEnvironment, VirtualPath,
};
use feature_permissions::*;
use server_api::authorization::{
    PermissionReferenceEncoding, PermissionResource, ScopedPermission, SubjectPermissions,
};

const WORKSPACE: &str = "semantic-ws";
const OWNER: &str = "semantic-owner";
const MAIN: &str = r#"# Preserve this comment and its subject-default spelling.
let settings = import "./settings/other.dcdl"; in {
    other = settings;
    permissions = {
        sets = [{ id = "local"; sets = ["subject-default"]; namespaces = [{ namespace = "subject"; keys = ["subject.delegate"]; }]; }];
        human_worker_default = { sets = ["local"]; exclude_sets = ["subject-default"]; exclude_namespaces = [{ namespace = "subject"; keys = ["subject.orchestrate"]; }]; };
    };
} as WorkspaceConfigSchema
"#;

fn path(value: &str) -> VirtualPath {
    VirtualPath::parse(value).unwrap()
}

fn schema(version: &str) -> WorkspaceConfigSchemaBundle {
    let mut permissions = policy_config::PermissionConfigSchemaProvider
        .contribution()
        .unwrap();
    if version != "5" {
        // Exact legacy input shape: real Subject assignments as well as the
        // schema-injected empty fields must evaluate before migration can refuse them.
        let source = r#"{
            permissions = {
                subjects = [...{
                    subject_id = String;
                    execution = {
                        sets = [...String] default [];
                        namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                        exclude_sets = [...String] default [];
                        exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                        revoked = Bool default false;
                    } default {};
                    delegation = {
                        sets = [...String] default [];
                        namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                        exclude_sets = [...String] default [];
                        exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                        revoked = Bool default false;
                    } default {};
                }] default [];
                remove_subjects = [...String] default [];
                sets = [...{
                    id = String;
                    sets = [...String] default [];
                    namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                }] default [];
                human_worker_default = {
                    sets = [...String] default [];
                    namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                    exclude_sets = [...String] default [];
                    exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                    revoked = Bool default false;
                } default {};
            };
        }"#;
        // Keep the historical shared-only schema independent of the current provider.
        let source = if version == "4" {
            r#"{ permissions = {
                sets = [...{
                    id = String;
                    sets = [...String] default [];
                    namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                }] default [];
                human_worker_default = {
                    sets = [...String] default [];
                    namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                    exclude_sets = [...String] default [];
                    exclude_namespaces = [...{ namespace = String; keys = [...String]; }] default [];
                    revoked = Bool default false;
                } default {};
            }; }"#
        } else {
            source
        };
        permissions = config_source::ConfigSchemaContribution::new(
            "builtin:permissions",
            "permissions",
            version,
            source,
        )
        .unwrap();
    }
    WorkspaceConfigSchemaBundle::compose([
        permissions,
        crate::prompt_settings::PromptConfigSchemaProvider
            .contribution()
            .unwrap(),
        config_source::ConfigSchemaContribution::new(
            "fixture:other",
            "other",
            "1",
            "{ other = { label = String default \"\"; } default {}; }",
        )
        .unwrap(),
    ])
    .unwrap()
}

fn host() -> PermissionHost {
    let mut host = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
    let feature = host
        .register_feature(FeatureIdentity::parse("fixture:semantic").unwrap())
        .unwrap();
    let namespace = host
        .bind_namespace(&feature, NamespaceId::parse("test").unwrap())
        .unwrap();
    let entries = [
        ("conversation", "test.chat"),
        ("work-coordination", "test.coordinate"),
        ("work-execution", "test.execute"),
        ("implementer", "test.implement"),
    ];
    host.declare(
        &namespace,
        NamespaceDeclaration {
            permissions: entries
                .iter()
                .map(|(_, key)| PermissionDefinition {
                    key: PermissionKey::parse(*key).unwrap(),
                    description: String::new(),
                    children: vec![],
                })
                .collect(),
            sets: entries
                .iter()
                .map(|(set, key)| SetContribution {
                    set: PermissionSetId::parse(*set).unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse("test").unwrap(),
                        keys: vec![PermissionKey::parse(*key).unwrap()],
                    })],
                })
                .collect(),
        },
    )
    .unwrap();
    host
}

fn fixture(main: &str) -> (tempfile::TempDir, SqliteWorkspaceStore) {
    fixture_with_schema(main, schema("3"))
}

fn fixture_with_schema(
    main: &str,
    bundle: WorkspaceConfigSchemaBundle,
) -> (tempfile::TempDir, SqliteWorkspaceStore) {
    fixture_with_settings(
        main,
        bundle,
        "{ label = \"subject-default and subject.delegate are just text here\"; }",
    )
}

fn fixture_with_settings(
    main: &str,
    bundle: WorkspaceConfigSchemaBundle,
    settings: &str,
) -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let dir = tempfile::tempdir().unwrap();
    let db = SqliteWorkspaceStore::open_for_test(&dir.path().join("server.sqlite")).unwrap();
    let mut state = crate::config_source::initial_state_with_schema(bundle).unwrap();
    state.snapshot = ConfigTreeSnapshot::from_entries([
        ConfigEntry::new(
            path(MAIN_CONFIG_ENTRYPOINT),
            ConfigContentType::Decodal,
            main,
        )
        .unwrap(),
        ConfigEntry::new(
            path("settings/other.dcdl"),
            ConfigContentType::Decodal,
            settings,
        )
        .unwrap(),
    ])
    .unwrap();
    state.projection_digest = SnapshotEnvironment::new(state.snapshot.clone())
        .evaluate_contract(&state.contract)
        .unwrap()
        .projection_digest;
    let constraints = SubjectPermissions {
        change_id: "keep-change".into(),
        execution: vec![ScopedPermission {
            operation: "test.chat".into(),
            resource: PermissionResource::Workdir {
                working_directory_id: "restricted-workdir".into(),
                path: "only.txt".into(),
                recursive: false,
            },
        }],
        delegation: vec![],
    };
    db.with_immediate_transaction(|tx| {
        tx.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('semantic-owner','user','semantic-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('semantic-ws','semantic-owner','Semantic','active','1','1');")?;
        if state.contract.schema_bundle.contributions.iter().any(|provider| provider.provider_id == "builtin:plugins") {
            let (contract, evaluation, _) = crate::plugin_settings::evaluate_catalog_config(
                tx, WORKSPACE, &state.snapshot, state.contract.schema_bundle.clone(),
            )?;
            state.contract = contract;
            state.projection_digest = evaluation.projection_digest;
        }
        crate::config_source::insert_materialized_state(tx, WORKSPACE, &state, "now")?;
        let authority = OwnerScope(WORKSPACE);
        let content = ScopedStore::new(tx, scope(WORKSPACE), &authority);
        let prepared = content_history_adapters::settings::prepare(&state.snapshot).map_err(content_error)?;
        for (id, object) in &prepared.objects { content.put(*id, &object.encode()).map_err(content_error)?; }
        content.adopt(prepared.root, None, OWNER.into(), 1).map_err(content_error)?;
        tx.execute("INSERT INTO workspace_configuration_authorities VALUES(?1,'old-transfer','old-physical','old-export')", [WORKSPACE])?;
        for (subject, execution) in [
            ("shown", serde_json::json!({"sets":["subject-default"]})),
            ("omitted", serde_json::json!({"sets":["subject-default"],"exclude_sets":["subject-default"],"exclude_namespaces":[{"namespace":"subject","keys":["subject.delegate"]}],"revoked":true})),
            ("empty", serde_json::json!({})),
        ] {
            let raw = serde_json::json!({
                "source":{"subject_id":subject,"execution":execution,"delegation":{}},
                "change_id":"keep-change", "legacy_constraints":constraints,
            }).to_string();
            tx.execute("INSERT INTO workspace_configuration_policies VALUES(?1,?2,?3)", params![WORKSPACE,subject,raw])?;
        }
        tx.execute("INSERT INTO workspace_configuration_receipts VALUES(?1,'historical','old-fingerprint','{\"old\":true}')", [WORKSPACE])?;
        // Previously issued grants are evidence, including their retired issuer spelling,
        // reference encoding, whitespace and revocation; never reissue from the new default.
        let historical_grant = server_api::authorization::CoreWorkerExecutionGrant {
            grant_id: "historical-grant".into(), workspace_id: WORKSPACE.into(),
            creation_operation_id: "historical-creation".into(), runtime_id: "runtime".into(),
            worker_id: "historical-worker".into(), profile: "builtin:coder".into(),
            profile_digest: "historical-profile".into(),
            assignment: PermissionAssignment::default(), reference_encoding: Default::default(),
            issuance_ceiling: None,
            source: server_api::authorization::CoreWorkerGrantSource::Human { account_id: OWNER.into() },
        };
        tx.execute(
            "INSERT INTO core_worker_execution_grants(workspace_id,grant_id,creation_operation_id,runtime_id,worker_id,grant_json,revoked) VALUES(?1,'historical-grant','historical-creation','runtime','historical-worker',?2,1)",
            params![WORKSPACE, serde_json::to_string_pretty(&historical_grant).unwrap()],
        )?;
        tx.execute("INSERT INTO core_worker_grant_revocations VALUES(?1,'historical-revocation',?2,'historical-grant')", params![WORKSPACE, OWNER])?;
        tx.execute("INSERT INTO workspace_permission_migration_plans VALUES(?1,'frozen-old','{\"reference\":\"subject-default\"}')", [WORKSPACE])?;
        Ok(())
    }).unwrap();
    (dir, db)
}

fn preserved_evidence(db: &SqliteWorkspaceStore) -> Vec<String> {
    db.with_conn(|conn| {
        let mut rows = vec![];
        for query in [
            "SELECT transfer_identity || legacy_physical_identity || legacy_export_digest FROM workspace_configuration_authorities WHERE workspace_id=?1",
            "SELECT request_fingerprint || result_json FROM workspace_configuration_receipts WHERE workspace_id=?1",
            "SELECT transfer_id || plan_json FROM workspace_permission_migration_plans WHERE workspace_id=?1",
            "SELECT grant_json || revoked FROM core_worker_execution_grants WHERE workspace_id=?1",
            "SELECT operation_key || actor_account_id || grant_id FROM core_worker_grant_revocations WHERE workspace_id=?1",
        ] { rows.push(conn.query_row(query, [WORKSPACE], |row| row.get(0))?); }
        Ok(rows)
    }).unwrap()
}

#[test]
fn shared_reference_migration_preserves_domain_policies_constraints_and_immutable_evidence() {
    let (_dir, db) = fixture(MAIN);
    let host = host();
    let before_state = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let before_head = db.with_conn(|conn| super::head(conn, WORKSPACE)).unwrap();
    let before_policies = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
    let before_evidence = preserved_evidence(&db);
    let before = SnapshotEnvironment::new(before_state.snapshot.clone())
        .evaluate_contract(&before_state.contract)
        .unwrap();
    // Empty old schema defaults may be decoded only through the legacy boundary.
    assert!(policy_config::source(&before).is_err());
    let mut expected = policy_config::legacy_source(&before)
        .unwrap()
        .shared_source();
    assert!(policy_config::migrate_source_references(&mut expected));
    assert!(semantic_references::migrate(&db, WORKSPACE, schema("5"), &host).unwrap());
    let state = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let after = SnapshotEnvironment::new(state.snapshot.clone())
        .evaluate_contract(&state.contract)
        .unwrap();
    assert_eq!(policy_config::source(&after).unwrap(), expected);
    assert_eq!(
        expected.sets[0]
            .sets
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        vec!["conversation", "work-coordination", "work-execution"]
    );
    assert!(expected.sets[0].namespaces.is_empty());
    assert!(expected.worker_default.exclude_namespaces.is_empty());
    assert_eq!(
        expected
            .worker_default
            .exclude_sets
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>(),
        vec!["conversation", "work-coordination", "work-coordination"]
    );
    assert_eq!(
        before.projections[0].data_json["other"],
        after.projections[0].data_json["other"]
    );
    let historical_base = &state
        .snapshot
        .get(&path("permission-references-base-0.dcdl"))
        .unwrap()
        .content;
    assert!(historical_base.ends_with(MAIN));
    assert!(historical_base.contains(&before_state.contract.schema_bundle.source));
    let historical_contract = config_source::ToolchainContract::with_schema_bundle(
        before_state.contract.schema_version,
        vec![path("permission-references-base-0.dcdl")],
        before_state.contract.import_policy_version,
        WorkspaceConfigSchemaBundle::empty(),
    );
    let historical = SnapshotEnvironment::new(state.snapshot.clone())
        .evaluate_contract(&historical_contract)
        .unwrap();
    assert_eq!(
        historical.projections[0].data_json,
        before.projections[0].data_json,
    );
    assert_eq!(
        state.snapshot.get(&path("settings/other.dcdl")),
        before_state.snapshot.get(&path("settings/other.dcdl"))
    );
    for entry in state.snapshot.entries.values() {
        // The pinned schema has individual field ranges, but no source records.
        if entry.path != path("permission-references-base-0.dcdl") {
            assert!(!entry.content.contains("subject_id"));
            assert!(!entry.content.contains("remove_subjects"));
        }
        for id in ["shown", "omitted", "previously-removed"] {
            assert!(!entry.content.contains(&serde_json::to_string(id).unwrap()));
        }
    }
    let current = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
    assert_eq!(
        current.keys().collect::<Vec<_>>(),
        before_policies.keys().collect::<Vec<_>>()
    );
    for (subject, policy) in &current {
        assert_eq!(policy.change_id, before_policies[subject].change_id);
        assert_eq!(
            policy.legacy_constraints,
            before_policies[subject].legacy_constraints
        );
        assert_eq!(policy.current(), before_policies[subject].current());
        assert_eq!(
            policy.reference_encoding,
            PermissionReferenceEncoding::FeatureSetsV2
        );
    }
    assert!(current["omitted"].source.execution.revoked);
    assert_eq!(
        current["empty"].source.execution,
        policy_config::AssignmentSource::default()
    );
    assert_eq!(
        current["empty"].source.delegation,
        policy_config::AssignmentSource::default()
    );
    assert!(!current.contains_key("previously-removed"));
    db.with_conn(|conn| {
        let tx = conn.unchecked_transaction()?;
        let authority = OwnerScope(WORKSPACE);
        let content = ScopedStore::new(&tx, scope(WORKSPACE), &authority);
        let head = content.head().map_err(content_error)?.unwrap();
        let content_history::Object::History(record) = content
            .read(content_history::ObjectId::History(head))
            .map_err(content_error)?
        else {
            panic!("head is not history")
        };
        assert_eq!(record.actor, "system:permission-reference-migration");
        assert_eq!(record.parent, before_head);
        content
            .snapshot(before_head.unwrap())
            .map_err(content_error)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(preserved_evidence(&db), before_evidence);
    let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
    assert!(!semantic_references::migrate(&db, WORKSPACE, schema("5"), &host).unwrap());
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
    assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), state);
    assert_eq!(preserved_evidence(&db), before_evidence);
}

#[test]
fn legacy_schema_assertions_in_imports_keep_their_stored_typed_range() {
    let settings =
        r#"({ other = { label = "historical import"; }; } as WorkspaceConfigSchema).other"#;
    let (dir, db) = fixture_with_settings(MAIN, schema("4"), settings);
    let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let objects = history_objects(&db);
    let evidence = preserved_evidence(&db);
    assert!(semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
    let after = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let pinned = &after
        .snapshot
        .get(&path("settings/other.dcdl"))
        .unwrap()
        .content;
    assert!(pinned.ends_with(settings));
    assert!(pinned.contains(&before.contract.schema_bundle.source));
    let evaluation = SnapshotEnvironment::new(after.snapshot.clone())
        .evaluate_contract(&after.contract)
        .unwrap();
    assert_eq!(
        evaluation.projections[0].data_json["other"]["label"],
        "historical import",
    );
    for provider in &before.contract.schema_bundle.contributions {
        if provider.provider_id != "builtin:permissions" {
            assert!(
                after
                    .contract
                    .schema_bundle
                    .contributions
                    .contains(provider)
            );
        }
    }
    let current_objects = history_objects(&db);
    assert!(
        objects
            .iter()
            .all(|object| current_objects.contains(object))
    );
    assert_eq!(preserved_evidence(&db), evidence);
    drop(db);
    let db = SqliteWorkspaceStore::open_for_test(&dir.path().join("server.sqlite")).unwrap();
    assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), after);
    assert!(!semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
    assert_eq!(preserved_evidence(&db), evidence);
    assert_eq!(history_objects(&db), current_objects);
}

#[test]
fn stored_subject_policies_do_not_change_authored_config_or_inject_subject_ids() {
    for main in [
        "# unchanged authored text\n{ other = import \"./settings/other.dcdl\"; } as WorkspaceConfigSchema",
        "let settings = import \"./settings/other.dcdl\"; in { other = settings; } as WorkspaceConfigSchema",
        "other = import \"./settings/other.dcdl\";",
        "{ permissions = {}; } as WorkspaceConfigSchema",
    ] {
        let (_dir, db) = fixture(main);
        let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        let before_policies = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
        assert!(semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
        let after = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        assert_eq!(after.snapshot, before.snapshot);
        let current = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
        assert_eq!(
            current.keys().collect::<Vec<_>>(),
            before_policies.keys().collect::<Vec<_>>()
        );
        for (subject, policy) in current {
            assert_eq!(policy.current(), before_policies[&subject].current());
            assert_eq!(
                policy.reference_encoding,
                PermissionReferenceEncoding::FeatureSetsV2
            );
        }
        let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
        assert!(!semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
    }
}

#[test]
fn explicit_and_computed_legacy_subject_source_blocks_without_changing_bytes_or_current_policy() {
    for (main, message) in [
        (
            "{ permissions = { subjects = [{ subject_id = \"shown\"; execution = {}; }]; }; } as WorkspaceConfigSchema",
            "typed source-range removal",
        ),
        (
            "let old = [{ subject_id = \"shown\"; execution = { revoked = true; }; }]; in { permissions = { subjects = old; }; } as WorkspaceConfigSchema",
            "typed source-range removal",
        ),
        (
            "{ permissions = { subjects = [{ subject_id = \"shown\"; delegation = { sets = [\"subject-default\"]; }; }]; }; } as WorkspaceConfigSchema",
            "typed source-range removal",
        ),
        (
            "{ permissions = { subjects = []; }; } as WorkspaceConfigSchema",
            "typed source-range removal",
        ),
        (
            "{ permissions = { remove_subjects = [\"previously-removed\"]; }; } as WorkspaceConfigSchema",
            "durable Subject-domain removal fence",
        ),
    ] {
        let (_dir, db) = fixture(main);
        let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        let before_policies = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
        let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
        let evidence = preserved_evidence(&db);
        let error = semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap_err();
        assert!(
            matches!(&error, Error::WorkspaceConfigConflict(_)),
            "{error}"
        );
        assert!(error.to_string().contains(message), "{error}");
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap().unwrap(),
            before
        );
        assert_eq!(
            db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap(),
            before_policies
        );
        for (subject, expected) in &before_policies {
            assert_eq!(expected.change_id, "keep-change");
            assert_eq!(
                db.current_subject_permission_policy(WORKSPACE, subject)
                    .unwrap(),
                Some(expected.current())
            );
        }
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
        assert_eq!(preserved_evidence(&db), evidence);
    }
}

#[test]
fn canonical_migration_storage_failures_roll_back_source_policies_and_history_then_retry() {
    for version in ["3", "4"] {
        for (table, event) in [
            ("workspace_config_entries", "INSERT"),
            ("workspace_configuration_policies", "UPDATE"),
            ("content_objects", "INSERT"),
            ("content_scopes", "UPDATE"),
        ] {
            let (dir, db) = fixture_with_schema(MAIN, schema(version));
            let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
            let policies_before = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
            let evidence = preserved_evidence(&db);
            let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
            db.with_conn(|conn| {
                conn.execute_batch(&format!(
                    "CREATE TRIGGER inject_failure BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END;"
                ))?;
                Ok(())
            }).unwrap();
            let error =
                semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap_err();
            assert!(error.to_string().contains("injected"), "{table}: {error}");
            db.with_conn(|conn| {
                conn.execute_batch("DROP TRIGGER inject_failure")?;
                Ok(())
            })
            .unwrap();
            drop(db);
            let db =
                SqliteWorkspaceStore::open_for_test(&dir.path().join("server.sqlite")).unwrap();
            assert_eq!(
                db.load_workspace_config(WORKSPACE).unwrap().unwrap(),
                before
            );
            assert_eq!(
                db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap(),
                policies_before
            );
            assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
            assert_eq!(preserved_evidence(&db), evidence);
            assert!(semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
            assert!(!semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
        }
    }
}

#[test]
fn permission_migration_preserves_unrelated_schemas_despite_live_default_drift() {
    for (main, version) in [
        (MAIN, "3"),
        (MAIN, "4"),
        ("{ permissions = {}; } as WorkspaceConfigSchema", "3"),
        ("{ permissions = {}; } as WorkspaceConfigSchema", "4"),
        (
            "{ permissions = { human_worker_default = {}; }; } as WorkspaceConfigSchema",
            "4",
        ),
    ] {
        let bundle = WorkspaceConfigSchemaBundle::compose(
            schema(version)
                .contributions
                .into_iter()
                .chain(std::iter::once(
                    crate::plugin_settings::PluginConfigSchemaProvider
                        .contribution()
                        .unwrap(),
                )),
        )
        .unwrap();
        let (_dir, db) = fixture_with_schema(main, bundle);
        let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        let before_evaluation = SnapshotEnvironment::new(before.snapshot.clone())
            .evaluate_contract(&before.contract)
            .unwrap();
        let before_policies = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
        let evidence = preserved_evidence(&db);
        let live_schema = WorkspaceConfigSchemaBundle::compose([
            policy_config::PermissionConfigSchemaProvider.contribution().unwrap(),
            crate::plugin_settings::PluginConfigSchemaProvider.contribution().unwrap(),
            config_source::ConfigSchemaContribution::new(
                "builtin:prompts", "prompts", "1",
                "{ prompts = { role = { async_chat = String default \"new bundled prompt\"; } default {}; } default {}; }",
            ).unwrap(),
            config_source::ConfigSchemaContribution::new(
                "fixture:other", "other", "2",
                "{ other = { label = String default \"\"; added = Bool default true; } default {}; }",
            ).unwrap(),
            config_source::ConfigSchemaContribution::new(
                "fixture:new", "new_feature", "1",
                "{ new_feature = Bool default true; }",
            ).unwrap(),
        ]).unwrap();
        assert!(
            semantic_references::migrate(&db, WORKSPACE, live_schema.clone(), &host()).unwrap()
        );
        let after = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        let after_evaluation = SnapshotEnvironment::new(after.snapshot.clone())
            .evaluate_contract(&after.contract)
            .unwrap();
        for provider in &before.contract.schema_bundle.contributions {
            if provider.provider_id != "builtin:permissions" {
                assert_eq!(
                    after
                        .contract
                        .schema_bundle
                        .contributions
                        .iter()
                        .find(|current| current.provider_id == provider.provider_id),
                    Some(provider),
                );
            }
        }
        assert!(
            !after
                .contract
                .schema_bundle
                .contributions
                .iter()
                .any(|provider| provider.provider_id == "fixture:new")
        );
        assert_eq!(
            before_evaluation.projections[0].data_json["prompts"],
            after_evaluation.projections[0].data_json["prompts"]
        );
        assert_eq!(
            before_evaluation.projections[0].data_json["other"],
            after_evaluation.projections[0].data_json["other"]
        );
        if !main.contains("human_worker_default") {
            assert_eq!(
                after.snapshot, before.snapshot,
                "omission needs only a schema update"
            );
        }
        for (subject, policy) in db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap() {
            assert_eq!(policy.current(), before_policies[&subject].current());
        }
        assert_eq!(preserved_evidence(&db), evidence);
        let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
        assert!(!semantic_references::migrate(&db, WORKSPACE, live_schema, &host()).unwrap());
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
        assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), after);
    }
}

#[test]
fn reference_only_proof_rejects_unrelated_projection_changes() {
    let (_dir, db) = fixture("{ permissions = {}; } as WorkspaceConfigSchema");
    let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let policies_before = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
    let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
    let incompatible = WorkspaceConfigSchemaBundle::compose([
        policy_config::PermissionConfigSchemaProvider
            .contribution()
            .unwrap(),
        crate::prompt_settings::PromptConfigSchemaProvider
            .contribution()
            .unwrap(),
        config_source::ConfigSchemaContribution::new(
            "fixture:other",
            "other",
            "2",
            "{ other = { label = String default \"\"; added = Bool default true; } default {}; }",
        )
        .unwrap(),
    ])
    .unwrap();
    let before_evaluation = SnapshotEnvironment::new(before.snapshot.clone())
        .evaluate_contract(&before.contract)
        .unwrap();
    let evaluate = |schema| {
        let contract = config_source::ToolchainContract::with_schema_bundle(
            before.contract.schema_version,
            before.contract.entrypoints.clone(),
            before.contract.import_policy_version,
            schema,
        );
        SnapshotEnvironment::new(before.snapshot.clone())
            .evaluate_contract(&contract)
            .unwrap()
    };
    semantic_references::prove_reference_only(&before, &before_evaluation, &evaluate(schema("5")))
        .unwrap();
    let error = semantic_references::prove_reference_only(
        &before,
        &before_evaluation,
        &evaluate(incompatible),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot prove reference-only source migration"),
        "{error}"
    );
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap().unwrap(),
        before
    );
    assert_eq!(
        db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap(),
        policies_before
    );
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
}

#[test]
fn reference_only_proof_normalizes_sparse_defaults_but_rejects_permission_changes() {
    for (main, permissions) in [
        ("{} as WorkspaceConfigSchema", None),
        (
            "{ permissions = {}; } as WorkspaceConfigSchema",
            Some(serde_json::json!({"sets":[]})),
        ),
        (
            "{ permissions = { human_worker_default = {}; }; } as WorkspaceConfigSchema",
            Some(serde_json::json!({"worker_default":{}})),
        ),
        (
            "{ permissions = { human_worker_default = { sets = [\"implementer\"]; }; }; } as WorkspaceConfigSchema",
            Some(serde_json::json!({"worker_default":{"sets":["implementer"]}})),
        ),
    ] {
        let (_dir, db) = fixture_with_schema(main, schema("4"));
        let original = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
        let before = SnapshotEnvironment::new(original.snapshot.clone())
            .evaluate_contract(&original.contract)
            .unwrap();
        let mut after = before.clone();
        let root = after.projections[0].data_json.as_object_mut().unwrap();
        root.remove("permissions");
        if let Some(permissions) = permissions {
            root.insert("permissions".into(), permissions);
        }
        semantic_references::prove_reference_only(&original, &before, &after).unwrap();
        for invalid in [
            serde_json::json!({"human_worker_default":{}}),
            serde_json::json!({"subjects":[]}),
            serde_json::json!({"worker_default":{"revoked":true}}),
            serde_json::json!({"worker_default":{"unknown":true}}),
            serde_json::json!({"sets":[{"id":"unexpected"}]}),
        ] {
            let mut changed = after.clone();
            changed.projections[0].data_json["permissions"] = invalid;
            assert!(
                semantic_references::prove_reference_only(&original, &before, &changed).is_err(),
                "{main}: {}",
                changed.projections[0].data_json["permissions"],
            );
        }
        let permissions = if main.contains("human_worker_default = {}") {
            serde_json::json!({})
        } else {
            serde_json::json!({"worker_default":{}})
        };
        let mut changed = after.clone();
        changed.projections[0].data_json["permissions"] = permissions;
        assert!(semantic_references::prove_reference_only(&original, &before, &changed).is_err());
    }
}

#[test]
fn current_owner_adoption_rejects_individual_fields_even_empty_and_retired_shared_references() {
    let (_dir, db) = fixture(MAIN);
    let host = host();
    semantic_references::migrate(&db, WORKSPACE, schema("5"), &host).unwrap();
    let state = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let main = state.snapshot.get(&path(MAIN_CONFIG_ENTRYPOINT)).unwrap();
    let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
    for (permissions, expected) in [
        ("human_worker_default = {};", "human_worker_default"),
        ("subjects = [];", "subjects"),
        ("remove_subjects = [];", "remove_subjects"),
        ("subjects = [{ subject_id = \"shown\"; }];", "subjects"),
        (
            "worker_default = { sets = [\"subject-default\"]; };",
            "invalid permission references",
        ),
        (
            "worker_default = { namespaces = [{ namespace = \"subject\"; keys = [\"subject.delegate\"]; }]; revoked = true; };",
            "invalid permission references",
        ),
        (
            "sets = [{ id = \"subject-default\"; namespaces = [{ namespace = \"test\"; keys = [\"test.execute\"]; }]; }];",
            "retired Subject set ID is reserved",
        ),
    ] {
        let request = ConfigurationAdoptionRequest {
            operation_id: "new-old-source".into(),
            expected_head: head.clone(),
            expected_catalog_digest: catalog_digest(&host).unwrap(),
            expected_legacy_export: None,
            source: ConfigCommitRequest {
                base_digest: state.snapshot.digest.clone(),
                entrypoints: state.contract.entrypoints.clone(),
                changes: vec![ConfigTreeChange::Update {
                    path: main.path.clone(),
                    expected_digest: main.content_digest.clone(),
                    content: format!(
                        "{{ permissions = {{ {permissions} }}; }} as WorkspaceConfigSchema"
                    ),
                }],
            },
        };
        let error = db
            .adopt_configuration(
                WORKSPACE,
                OWNER,
                &request,
                schema("5"),
                &host,
                None,
                |_| Ok(()),
                |_| panic!("canonical edit must not transfer authority"),
            )
            .unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{permissions}: {error}"
        );
        assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), state);
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
    }
}

#[test]
fn ambiguous_retired_set_definition_blocks_migration_without_changing_authority() {
    let (_dir, db) = fixture(
        r#"{ permissions = {
        sets = [{ id = "subject-default"; namespaces = [{ namespace = "test"; keys = ["test.execute"]; }]; }];
        human_worker_default = { sets = ["subject-default"]; };
    }; } as WorkspaceConfigSchema"#,
    );
    let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let policies_before = db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap();
    let evidence = preserved_evidence(&db);
    let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
    let error = semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("retired Subject set ID is reserved")
    );
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap().unwrap(),
        before
    );
    assert_eq!(
        db.with_conn(|conn| policies(conn, WORKSPACE)).unwrap(),
        policies_before
    );
    assert_eq!(preserved_evidence(&db), evidence);
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
}

#[test]
fn legacy_target_schema_blocks_instead_of_repreparing_frozen_plan_as_current_input() {
    let (_dir, db) = fixture(MAIN);
    let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let evidence = preserved_evidence(&db);
    for version in ["3", "4"] {
        let error =
            semantic_references::migrate(&db, WORKSPACE, schema(version), &host()).unwrap_err();
        assert!(matches!(error, Error::WorkspaceConfigConflict(_)));
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap().unwrap(),
            before
        );
        assert_eq!(preserved_evidence(&db), evidence);
    }
}

#[test]
fn legacy_adopted_policy_missing_encoding_round_trips_without_changing_frozen_bytes() {
    let raw = serde_json::json!({
        "source":{"subject_id":"old","execution":{"sets":["subject-default"]},"delegation":{}},
        "change_id":"unchanged", "legacy_constraints":null,
    });
    let policy: AdoptedSubjectPolicy = serde_json::from_value(raw).unwrap();
    let before = serde_json::to_string(&policy).unwrap();
    assert!(!before.contains("reference_encoding"));
    let decoded: AdoptedSubjectPolicy = serde_json::from_str(&before).unwrap();
    assert_eq!(serde_json::to_string(&decoded).unwrap(), before);
    assert_eq!(
        decoded.current().reference_encoding,
        PermissionReferenceEncoding::FeatureSetsV2
    );
    assert_eq!(decoded.current().change_id, "unchanged");
    assert_eq!(serde_json::to_string(&decoded).unwrap(), before);
}

fn history_objects(db: &SqliteWorkspaceStore) -> Vec<(i64, Vec<u8>, Vec<u8>)> {
    db.with_conn(|conn| {
        let mut query =
            conn.prepare("SELECT kind,hash,bytes FROM content_objects ORDER BY kind,hash")?;
        let rows = query.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    })
    .unwrap()
}

#[test]
fn legacy_worker_default_rename_preserves_explicit_values_omission_and_evidence_on_reopen() {
    for version in ["1", "2", "3", "4"] {
        for (permissions, expected, rewrites_source) in [
            (
                None,
                policy_config::PermissionSource::default().worker_default,
                false,
            ),
            (
                Some(""),
                policy_config::PermissionSource::default().worker_default,
                false,
            ),
            (
                Some("human_worker_default = {};"),
                policy_config::AssignmentSource::default(),
                true,
            ),
            (
                Some(
                    r#"human_worker_default = {
                    sets = ["implementer"];
                    namespaces = [{ namespace = "test"; keys = ["test.chat"]; }];
                    exclude_sets = ["conversation"];
                    exclude_namespaces = [{ namespace = "test"; keys = ["test.execute"]; }];
                    revoked = true;
                };"#,
                ),
                policy_config::AssignmentSource {
                    sets: vec![PermissionSetId::parse("implementer").unwrap()],
                    namespaces: vec![NamespacePermissions {
                        namespace: NamespaceId::parse("test").unwrap(),
                        keys: vec![PermissionKey::parse("test.chat").unwrap()],
                    }],
                    exclude_sets: vec![PermissionSetId::parse("conversation").unwrap()],
                    exclude_namespaces: vec![NamespacePermissions {
                        namespace: NamespaceId::parse("test").unwrap(),
                        keys: vec![PermissionKey::parse("test.execute").unwrap()],
                    }],
                    revoked: true,
                },
                true,
            ),
            (
                // A reference conversion elsewhere must not turn an omitted old default
                // into the old schema's injected empty assignment in the new wrapper.
                Some(r#"sets = [{ id = "local"; sets = ["subject-default"]; }];"#),
                policy_config::PermissionSource::default().worker_default,
                true,
            ),
        ] {
            let main = permissions.map_or_else(
                || "# old whole-config omission\n{} as WorkspaceConfigSchema".to_owned(),
                |fields| format!("# old authored source\n{{ permissions = {{ {fields} }}; }} as WorkspaceConfigSchema"),
            );
            let (dir, db) = fixture_with_schema(&main, schema(version));
            let before = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
            let objects = history_objects(&db);
            let evidence = preserved_evidence(&db);
            assert!(semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
            let after = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
            if rewrites_source {
                assert_ne!(after.snapshot, before.snapshot);
                assert!(
                    after
                        .snapshot
                        .get(&path(MAIN_CONFIG_ENTRYPOINT))
                        .unwrap()
                        .content
                        .contains("worker_default")
                );
                let historical_base = &after
                    .snapshot
                    .get(&path("permission-references-base-0.dcdl"))
                    .unwrap()
                    .content;
                assert!(historical_base.ends_with(&main));
                assert!(historical_base.contains(&before.contract.schema_bundle.source));
            } else {
                assert_eq!(
                    after.snapshot, before.snapshot,
                    "omission must stay unauthored"
                );
            }
            assert_eq!(
                db.worker_assignment(WORKSPACE, &host(), None).unwrap(),
                expected.assignment()
            );
            assert_eq!(preserved_evidence(&db), evidence);
            let current_objects = history_objects(&db);
            for object in &objects {
                assert!(
                    current_objects.contains(object),
                    "original history bytes changed"
                );
            }
            drop(db);
            let db =
                SqliteWorkspaceStore::open_for_test(&dir.path().join("server.sqlite")).unwrap();
            assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), after);
            assert_eq!(
                db.worker_assignment(WORKSPACE, &host(), None).unwrap(),
                expected.assignment()
            );
            let empty = PermissionAssignment::default();
            assert_eq!(
                db.worker_assignment(WORKSPACE, &host(), Some(&empty))
                    .unwrap(),
                empty
            );
            assert_eq!(preserved_evidence(&db), evidence);
            let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
            assert!(!semantic_references::migrate(&db, WORKSPACE, schema("5"), &host()).unwrap());
            assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
            assert_eq!(db.load_workspace_config(WORKSPACE).unwrap().unwrap(), after);
            assert_eq!(history_objects(&db), current_objects);
            assert_eq!(preserved_evidence(&db), evidence);
        }
    }
}
