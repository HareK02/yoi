//! Adoption component tests use the real Server SQLite transaction and content
//! store. The legacy transfer callback is empty in this fixture; physical Feature
//! transfer and authenticated HTTP are separate integration coverage.
use super::*;
use crate::config_source::{MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider};
use crate::policy_config::PermissionConfigSchemaProvider;
use config_source::{ConfigTreeChange, VirtualPath};
use feature_permissions::*;

const WORKSPACE: &str = "adoption-ws";
const OWNER: &str = "adoption-owner";

fn host(children: bool) -> PermissionHost {
    let mut host = PermissionHost::new(WorkspaceId::parse(WORKSPACE).unwrap());
    let feature = host
        .register_feature(FeatureIdentity::parse("builtin:ticket").unwrap())
        .unwrap();
    let namespace = host
        .bind_namespace(&feature, NamespaceId::parse("ticket").unwrap())
        .unwrap();
    host.declare(
        &namespace,
        NamespaceDeclaration {
            permissions: vec![
                PermissionDefinition {
                    key: PermissionKey::parse("ticket.hoge").unwrap(),
                    description: String::new(),
                    children: if children {
                        vec![PermissionKey::parse("ticket.hoge.fuga").unwrap()]
                    } else {
                        vec![]
                    },
                },
                PermissionDefinition {
                    key: PermissionKey::parse("ticket.hoge.fuga").unwrap(),
                    description: String::new(),
                    children: vec![],
                },
            ],
            sets: vec![SetContribution {
                set: PermissionSetId::parse("ticket").unwrap(),
                children: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: NamespaceId::parse("ticket").unwrap(),
                    keys: vec![PermissionKey::parse("ticket.hoge").unwrap()],
                })],
            }],
        },
    )
    .unwrap();
    host
}
fn schema() -> WorkspaceConfigSchemaBundle {
    WorkspaceConfigSchemaBundle::compose([
        PermissionConfigSchemaProvider.contribution().unwrap(),
        crate::prompt_settings::PromptConfigSchemaProvider
            .contribution()
            .unwrap(),
    ])
    .unwrap()
}
fn open(path: &std::path::Path) -> SqliteWorkspaceStore {
    SqliteWorkspaceStore::open_for_test(path).unwrap()
}
fn fixture() -> (tempfile::TempDir, SqliteWorkspaceStore) {
    let dir = tempfile::tempdir().unwrap();
    let db = open(&dir.path().join("server.sqlite"));
    db.with_immediate_transaction(|tx| {
        tx.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at) VALUES('adoption-owner','user','adoption-owner','Owner','1','1'); INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at) VALUES('adoption-ws','adoption-owner','Adoption','active','1','1');")?;
        Ok(())
    }).unwrap();
    db.ensure_workspace_config_materialized_with_schema(WORKSPACE, "now", schema())
        .unwrap();
    (dir, db)
}
fn request(
    db: &SqliteWorkspaceStore,
    host: &PermissionHost,
    id: &str,
    source: Option<&str>,
) -> ConfigurationAdoptionRequest {
    let current = db.load_workspace_config(WORKSPACE).unwrap().unwrap();
    let path = VirtualPath::parse(MAIN_CONFIG_ENTRYPOINT).unwrap();
    let changes = source
        .map(|source| {
            vec![ConfigTreeChange::Update {
                path: path.clone(),
                expected_digest: current.snapshot.get(&path).unwrap().content_digest.clone(),
                content: source.into(),
            }]
        })
        .unwrap_or_default();
    ConfigurationAdoptionRequest {
        operation_id: id.into(),
        expected_legacy_export: db
            .configuration_head(WORKSPACE, OWNER)
            .unwrap()
            .is_none()
            .then(|| "fixture-empty".into()),
        expected_head: db.configuration_head(WORKSPACE, OWNER).unwrap(),
        expected_catalog_digest: catalog_digest(host).unwrap(),
        source: ConfigCommitRequest {
            base_digest: current.snapshot.digest,
            changes,
            entrypoints: vec![path],
        },
    }
}
fn adopt(
    db: &SqliteWorkspaceStore,
    host: &PermissionHost,
    request: &ConfigurationAdoptionRequest,
) -> Result<ConfigurationAdoptionResult> {
    db.adopt_configuration(
        WORKSPACE,
        OWNER,
        request,
        schema(),
        host,
        None,
        |s| {
            if s == "subject-a" {
                Ok(())
            } else {
                Err(Error::InvalidInput("unknown Subject".into()))
            }
        },
        |fingerprint| {
            Ok(ConfigurationPolicyTransfer {
                transfer_identity: fingerprint.into(),
                physical_identity: "fixture-physical".into(),
                export_digest: "fixture-empty".into(),
                policies: BTreeMap::new(),
            })
        },
    )
}
const A: &str = r#"{ permissions = { subjects = [{ subject_id = "subject-a"; execution = { namespaces = [{ namespace = "ticket"; keys = ["ticket.hoge"]; }]; }; }]; }; } as WorkspaceConfigSchema"#;
const B: &str = r#"{ permissions = { subjects = [{ subject_id = "subject-a"; execution = { namespaces = [{ namespace = "ticket"; keys = ["ticket.hoge.fuga"]; }]; }; }]; }; } as WorkspaceConfigSchema"#;

#[test]
fn editing_head_rejects_aba_and_parallel_editors_but_receipts_reconcile_lost_responses() {
    let (_dir, db) = fixture();
    let host = host(false);
    let initial = request(&db, &host, "initial", Some(A));
    let first = adopt(&db, &host, &initial).unwrap();
    let stale = request(&db, &host, "stale-a", Some(B));
    adopt(&db, &host, &request(&db, &host, "to-b", Some(B))).unwrap();
    let final_a = adopt(&db, &host, &request(&db, &host, "back-a", Some(A))).unwrap();
    assert_eq!(first.state.snapshot, final_a.state.snapshot);
    assert_ne!(first.history_head, final_a.history_head);
    assert!(adopt(&db, &host, &stale).is_err());
    // Exact replay precedes stale checks and returns only the saved old result.
    assert_eq!(adopt(&db, &host, &initial).unwrap(), first);
    assert_eq!(
        db.configuration_adoption_result(WORKSPACE, OWNER, "initial")
            .unwrap(),
        Some(first)
    );
    assert_eq!(
        db.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(final_a.history_head)
    );
    let mut changed = initial;
    changed.source.changes.clear();
    assert!(adopt(&db, &host, &changed).is_err());
}

#[test]
fn failure_at_each_storage_boundary_rolls_back_and_reopens_the_previous_adoption() {
    for (table, event) in [
        ("content_objects", "INSERT"),
        ("workspace_config_entries", "INSERT"),
        ("workspace_configuration_policies", "INSERT"),
        ("content_scopes", "UPDATE"),
        ("workspace_configuration_receipts", "INSERT"),
    ] {
        let (dir, db) = fixture();
        let host = host(false);
        let before = adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
        let policy = db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap();
        let candidate = request(&db, &host, "failing", Some(B));
        db.with_conn(|conn| {conn.execute_batch(&format!("CREATE TEMP TRIGGER injected_failure BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT,'injected {table}'); END;"))?;Ok(())}).unwrap();
        assert!(adopt(&db, &host, &candidate).is_err(), "{table}");
        drop(db);
        let reopened = open(&dir.path().join("server.sqlite"));
        assert_eq!(
            reopened.load_workspace_config(WORKSPACE).unwrap(),
            Some(before.state),
            "{table}"
        );
        assert_eq!(
            reopened.configuration_head(WORKSPACE, OWNER).unwrap(),
            Some(before.history_head),
            "{table}"
        );
        assert_eq!(
            reopened
                .adopted_subject_policy(WORKSPACE, "subject-a")
                .unwrap(),
            policy,
            "{table}"
        );
        assert!(
            reopened
                .configuration_adoption_result(WORKSPACE, OWNER, "failing")
                .unwrap()
                .is_none(),
            "{table}"
        );
        adopt(&reopened, &host, &candidate).unwrap();
    }
}

#[test]
fn invalid_references_subjects_structure_and_owner_do_not_prepare_a_transfer() {
    let (_dir, db) = fixture();
    let host = host(false);
    let initial = db.load_workspace_config(WORKSPACE).unwrap();
    for source in [
        "not valid Decodal {",
        r#"{permissions={subjects=[{subject_id="subject-a"; execution={sets=["missing"];};}];};} as WorkspaceConfigSchema"#,
        r#"{permissions={subjects=[{subject_id="subject-a"; execution={namespaces=[{namespace="ticket";keys=["ticket.unknown"];}];};}];};} as WorkspaceConfigSchema"#,
        r#"{permissions={subjects=[{subject_id="missing";}];};} as WorkspaceConfigSchema"#,
        r#"{permissions={subjects=[{subject_id="subject-a";},{subject_id="subject-a";}];};} as WorkspaceConfigSchema"#,
        r#"{permissions={subjects=[{subject_id="subject-a"; execution={namespaces=[{namespace="ticket";keys=[{key="ticket.hoge"; payload="forbidden";}];}];};}];};} as WorkspaceConfigSchema"#,
    ] {
        let request = request(&db, &host, "bad", Some(source));
        let result = db.adopt_configuration(
            WORKSPACE,
            OWNER,
            &request,
            schema(),
            &host,
            None,
            |s| {
                if s == "subject-a" {
                    Ok(())
                } else {
                    Err(Error::InvalidInput("unknown Subject".into()))
                }
            },
            |_| panic!("validation failure must precede seal"),
        );
        assert!(result.is_err(), "{source}");
        assert_eq!(db.load_workspace_config(WORKSPACE).unwrap(), initial);
        assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    }
    let request = request(&db, &host, "other-owner", None);
    assert!(
        db.adopt_configuration(
            WORKSPACE,
            "non-owner",
            &request,
            schema(),
            &host,
            None,
            |_| Ok(()),
            |_| panic!("owner refusal must precede seal")
        )
        .is_err()
    );
}

#[test]
fn omission_is_unchanged_explicit_remove_unsets_and_noop_does_not_reissue_policy_identity() {
    let (_dir, db) = fixture();
    let host = host(false);
    adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
    let original = db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap();
    let preview = db
        .preview_configuration_adoption(
            WORKSPACE,
            OWNER,
            &request(&db, &host, "noop", None),
            schema(),
            &host,
            None,
            |_| Ok(()),
        )
        .unwrap();
    assert!(preview.changed_paths.is_empty());
    assert!(preview.changed_subjects.is_empty());
    adopt(&db, &host, &request(&db, &host, "noop", None)).unwrap();
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
        original
    );
    adopt(
        &db,
        &host,
        &request(&db, &host, "omit", Some("{} as WorkspaceConfigSchema")),
    )
    .unwrap();
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
        original
    );
    adopt(
        &db,
        &host,
        &request(
            &db,
            &host,
            "remove",
            Some(r#"{permissions={remove_subjects=["subject-a"];};} as WorkspaceConfigSchema"#),
        ),
    )
    .unwrap();
    assert!(
        db.adopted_subject_policy(WORKSPACE, "subject-a")
            .unwrap()
            .is_none()
    );
}

#[test]
fn feature_children_are_explicit_and_multisegment_keys_are_preserved_in_saved_assignments() {
    for children in [false, true] {
        let (_dir, db) = fixture();
        let host = host(children);
        adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
        let policy = db
            .adopted_subject_policy(WORKSPACE, "subject-a")
            .unwrap()
            .unwrap();
        let resolution = host.resolve(Some(&policy.source.execution.assignment()));
        assert_eq!(
            resolution
                .projection()
                .allowed
                .contains_key(&PermissionKey::parse("ticket.hoge.fuga").unwrap()),
            children
        );
        assert!(
            host.resolve(Some(&policy.source.delegation.assignment()))
                .projection()
                .allowed
                .is_empty()
        );
        adopt(&db, &host, &request(&db, &host, "multi", Some(B))).unwrap();
        let policy = db
            .adopted_subject_policy(WORKSPACE, "subject-a")
            .unwrap()
            .unwrap();
        assert_eq!(
            policy.source.execution.namespaces[0].keys[0].as_str(),
            "ticket.hoge.fuga"
        );
    }
}

#[test]
fn client_operation_id_cannot_reuse_a_previous_policy_change_identity() {
    let (_dir, db) = fixture();
    let host = host(false);
    adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
    let before = db
        .adopted_subject_policy(WORKSPACE, "subject-a")
        .unwrap()
        .unwrap();
    let change = request(&db, &host, &before.change_id, Some(B));
    adopt(&db, &host, &change).unwrap();
    let after = db
        .adopted_subject_policy(WORKSPACE, "subject-a")
        .unwrap()
        .unwrap();
    assert_ne!(after.change_id, before.change_id);
    assert_ne!(after.source, before.source);
}

#[test]
fn independent_database_connections_serialize_concurrent_web_and_cli_heads() {
    let (dir, db) = fixture();
    let registry = host(false);
    adopt(&db, &registry, &request(&db, &registry, "initial", Some(A))).unwrap();
    let other = open(&dir.path().join("server.sqlite"));
    let web = request(&db, &registry, "web-edit", Some(B));
    let cli = request(
        &other,
        &registry,
        "cli-edit",
        Some("{} as WorkspaceConfigSchema"),
    );
    assert_eq!(web.expected_head, cli.expected_head);
    let barrier = std::sync::Barrier::new(2);
    let (web, cli) = std::thread::scope(|scope| {
        let web = scope.spawn(|| {
            barrier.wait();
            adopt(&db, &host(false), &web)
        });
        let cli = scope.spawn(|| {
            barrier.wait();
            adopt(&other, &host(false), &cli)
        });
        (web.join().unwrap(), cli.join().unwrap())
    });
    let (winner, loser) = match (web, cli) {
        (Ok(winner), Err(loser)) | (Err(loser), Ok(winner)) => (winner, loser),
        results => panic!("exactly one observed head may adopt: {results:?}"),
    };
    assert!(matches!(loser, Error::WorkspaceConfigConflict(_)));
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(winner.state)
    );
    assert_eq!(
        db.configuration_head(WORKSPACE, OWNER).unwrap(),
        Some(winner.history_head)
    );
    assert_eq!(
        other
            .configuration_current(WORKSPACE, OWNER)
            .unwrap()
            .history_head,
        db.configuration_head(WORKSPACE, OWNER).unwrap()
    );
}
