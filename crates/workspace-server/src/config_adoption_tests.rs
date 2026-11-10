//! Adoption component tests use the real Server SQLite transaction and content
//! store. The legacy transfer callback retains a fixed native policy independently
//! of authored source; physical Feature transfer and authenticated HTTP are separate
//! integration coverage.
use super::*;
use crate::config_source::{MAIN_CONFIG_ENTRYPOINT, WorkspaceConfigSchemaProvider};
use crate::policy_config::PermissionConfigSchemaProvider;
use config_source::{ConfigTreeChange, VirtualPath};
use feature_permissions::*;
use server_api::authorization::{CurrentSubjectPolicy, PermissionReferenceEncoding};
use server_api::subject_policy::{SubjectPermissionPolicyAction, SubjectPermissionPolicyMutation};

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
            sets: ["ticket", "implementer"]
                .into_iter()
                .map(|set| SetContribution {
                    set: PermissionSetId::parse(set).unwrap(),
                    children: vec![PermissionReference::Namespace(NamespacePermissions {
                        namespace: NamespaceId::parse("ticket").unwrap(),
                        keys: vec![PermissionKey::parse("ticket.hoge").unwrap()],
                    })],
                })
                .collect(),
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
            .then(|| "fixture-native-export".into()),
        expected_head: db.configuration_head(WORKSPACE, OWNER).unwrap(),
        expected_catalog_digest: catalog_digest(host).unwrap(),
        source: ConfigCommitRequest {
            base_digest: current.snapshot.digest,
            changes,
            entrypoints: vec![path],
        },
    }
}
fn native_policy() -> AdoptedSubjectPolicy {
    AdoptedSubjectPolicy::from_current(
        "subject-a",
        &CurrentSubjectPolicy {
            change_id: "fixture-native-policy".into(),
            execution: PermissionAssignment {
                grants: vec![PermissionReference::Namespace(NamespacePermissions {
                    namespace: NamespaceId::parse("ticket").unwrap(),
                    keys: vec![PermissionKey::parse("ticket.hoge").unwrap()],
                })],
                ..Default::default()
            },
            delegation: PermissionAssignment::default(),
            legacy_constraints: None,
            reference_encoding: PermissionReferenceEncoding::FeatureSetsV2,
        },
    )
}

fn assert_native_policy(db: &SqliteWorkspaceStore) {
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
        Some(native_policy())
    );
    assert_eq!(
        db.current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap(),
        Some(native_policy().current())
    );
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
                export_digest: "fixture-native-export".into(),
                policies: BTreeMap::from([("subject-a".into(), native_policy())]),
            })
        },
    )
}
const A: &str = r#"{ permissions = { worker_default = { namespaces = [{ namespace = "ticket"; keys = ["ticket.hoge"]; }]; }; }; } as WorkspaceConfigSchema"#;
const B: &str = r#"{ permissions = { worker_default = { namespaces = [{ namespace = "ticket"; keys = ["ticket.hoge.fuga"]; }]; }; }; } as WorkspaceConfigSchema"#;

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
    assert_native_policy(&db);
}

#[test]
fn failure_at_each_storage_boundary_rolls_back_and_reopens_the_previous_adoption() {
    for (table, event) in [
        ("content_objects", "INSERT"),
        ("workspace_config_entries", "INSERT"),
        ("content_scopes", "UPDATE"),
        ("workspace_configuration_receipts", "INSERT"),
    ] {
        let (dir, db) = fixture();
        let host = host(false);
        let before = adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
        let policy = db
            .current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap();
        let candidate = request(&db, &host, "failing", Some(B));
        db.with_conn(|conn| {conn.execute_batch(&format!("CREATE TEMP TRIGGER injected_failure BEFORE {event} ON {table} BEGIN SELECT RAISE(ABORT,'injected {table}'); END;"))?;Ok(())}).unwrap();
        let error = adopt(&db, &host, &candidate).unwrap_err();
        assert!(
            error.to_string().contains(&format!("injected {table}")),
            "{table}: {error}"
        );
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
                .current_subject_permission_policy(WORKSPACE, "subject-a")
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
        assert_native_policy(&reopened);
    }
}

#[test]
fn initial_native_policy_insert_failure_rolls_back_source_authority_head_and_receipt() {
    let (dir, db) = fixture();
    let host = host(false);
    let before = db.load_workspace_config(WORKSPACE).unwrap();
    let candidate = request(&db, &host, "initial-policy-failure", Some(A));
    db.with_conn(|conn| {
        conn.execute_batch("CREATE TEMP TRIGGER injected_failure BEFORE INSERT ON workspace_configuration_policies BEGIN SELECT RAISE(ABORT,'injected policy insert'); END;")?;
        Ok(())
    }).unwrap();
    let error = adopt(&db, &host, &candidate).unwrap_err();
    assert!(
        error.to_string().contains("injected policy insert"),
        "{error}"
    );
    drop(db);
    let reopened = open(&dir.path().join("server.sqlite"));
    assert_eq!(reopened.load_workspace_config(WORKSPACE).unwrap(), before);
    assert_eq!(reopened.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert!(
        !reopened
            .configuration_policy_is_canonical(WORKSPACE)
            .unwrap()
    );
    assert!(
        reopened
            .with_conn(|conn| policies(conn, WORKSPACE))
            .unwrap()
            .is_empty()
    );
    assert!(
        reopened
            .configuration_adoption_result(WORKSPACE, OWNER, &candidate.operation_id)
            .unwrap()
            .is_none()
    );
    adopt(&reopened, &host, &candidate).unwrap();
    assert_native_policy(&reopened);
}

#[test]
fn invalid_references_structure_and_owner_do_not_prepare_a_transfer() {
    let (_dir, db) = fixture();
    let host = host(false);
    let initial = db.load_workspace_config(WORKSPACE).unwrap();
    for source in [
        "not valid Decodal {",
        r#"{permissions={worker_default={sets=["missing"];};};} as WorkspaceConfigSchema"#,
        r#"{permissions={worker_default={namespaces=[{namespace="ticket";keys=["ticket.unknown"];}];};};} as WorkspaceConfigSchema"#,
        r#"{permissions={sets=[{id="duplicate";},{id="duplicate";}];};} as WorkspaceConfigSchema"#,
        r#"{permissions={worker_default={namespaces=[{namespace="ticket";keys=[{key="ticket.hoge"; payload="forbidden";}];}];};};} as WorkspaceConfigSchema"#,
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
fn source_edits_omission_and_noop_leave_dedicated_current_policy_identity_unchanged() {
    let (_dir, db) = fixture();
    let host = host(false);
    let initial = request(&db, &host, "initial", Some(A));
    let first = adopt(&db, &host, &initial).unwrap();
    // An owner policy decision is independent of the source's creation default.
    let original = db
        .mutate_subject_permission_policy(
            WORKSPACE,
            OWNER,
            "subject-a",
            &SubjectPermissionPolicyMutation {
                operation_id: "dedicated-policy".into(),
                expected_change_id: Some(native_policy().change_id),
                action: SubjectPermissionPolicyAction::Replace {
                    execution: PermissionAssignment::default(),
                    delegation: native_policy().current().execution,
                },
            },
            &host,
        )
        .unwrap()
        .policy
        .unwrap();
    let saved = db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap();
    for (id, source) in [
        ("noop", None),
        ("new-default", Some(B)),
        ("omit", Some("{} as WorkspaceConfigSchema")),
    ] {
        let candidate = request(&db, &host, id, source);
        let preview = db
            .preview_configuration_adoption(
                WORKSPACE,
                OWNER,
                &candidate,
                schema(),
                &host,
                None,
                |_| Ok(()),
            )
            .unwrap();
        assert_eq!(preview.changed_paths.is_empty(), source.is_none());
        assert!(preview.changed_subjects.is_empty());
        adopt(&db, &host, &candidate).unwrap();
        assert_eq!(
            db.current_subject_permission_policy(WORKSPACE, "subject-a")
                .unwrap(),
            Some(original.clone())
        );
        assert_eq!(
            db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
            saved
        );
    }
    let state = db.load_workspace_config(WORKSPACE).unwrap();
    let head = db.configuration_head(WORKSPACE, OWNER).unwrap();
    assert_eq!(adopt(&db, &host, &initial).unwrap(), first);
    assert_eq!(db.load_workspace_config(WORKSPACE).unwrap(), state);
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), head);
    assert_eq!(
        db.current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap(),
        Some(original)
    );
    assert_eq!(
        db.adopted_subject_policy(WORKSPACE, "subject-a").unwrap(),
        saved
    );
}

#[test]
fn individual_config_fields_are_rejected_even_empty_without_changing_current_policy() {
    let (_dir, db) = fixture();
    let host = host(false);
    let committed = adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
    for (index, (field, source)) in [
        ("subjects", r#"{permissions={subjects=[];};} as WorkspaceConfigSchema"#),
        ("subjects", r#"{permissions={subjects=[{subject_id="subject-a";execution={};}];};} as WorkspaceConfigSchema"#),
        ("remove_subjects", r#"{permissions={remove_subjects=[];};} as WorkspaceConfigSchema"#),
        ("remove_subjects", r#"{permissions={remove_subjects=["subject-a"];};} as WorkspaceConfigSchema"#),
    ].into_iter().enumerate() {
        let id = format!("individual-field-{index}");
        let candidate = request(&db, &host, &id, Some(source));
        let preview_error = db
            .preview_configuration_adoption(
                WORKSPACE, OWNER, &candidate, schema(), &host, None,
                |_| panic!("current config must not resolve individual Subjects"),
            )
            .unwrap_err();
        assert!(preview_error.to_string().contains(field), "{source}: {preview_error}");
        let error = db
            .adopt_configuration(
                WORKSPACE, OWNER, &candidate, schema(), &host, None,
                |_| panic!("current config must not resolve individual Subjects"),
                |_| panic!("invalid source must not prepare a transfer"),
            )
            .unwrap_err();
        assert!(error.to_string().contains(field), "{source}: {error}");
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(committed.state.clone())
        );
        assert_eq!(
            db.configuration_head(WORKSPACE, OWNER).unwrap(),
            Some(committed.history_head.clone())
        );
        assert!(
            db.configuration_adoption_result(WORKSPACE, OWNER, &id)
                .unwrap()
                .is_none()
        );
        assert_native_policy(&db);
    }
}

#[test]
fn feature_children_are_explicit_and_multisegment_keys_are_preserved_in_saved_assignments() {
    for children in [false, true] {
        let (_dir, db) = fixture();
        let host = host(children);
        adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
        let (configured, assignment) = db.worker_permissions(WORKSPACE, &host, None).unwrap();
        let resolution = configured.resolve(Some(&assignment));
        assert_eq!(
            resolution
                .projection()
                .allowed
                .contains_key(&PermissionKey::parse("ticket.hoge.fuga").unwrap()),
            children
        );
        adopt(&db, &host, &request(&db, &host, "multi", Some(B))).unwrap();
        let (_, assignment) = db.worker_permissions(WORKSPACE, &host, None).unwrap();
        assert_eq!(
            assignment.grants,
            vec![PermissionReference::Namespace(NamespacePermissions {
                namespace: NamespaceId::parse("ticket").unwrap(),
                keys: vec![PermissionKey::parse("ticket.hoge.fuga").unwrap()],
            })]
        );
        assert_native_policy(&db);
    }
}

#[test]
fn client_operation_id_matching_current_policy_identity_does_not_change_that_policy() {
    let (_dir, db) = fixture();
    let host = host(false);
    adopt(&db, &host, &request(&db, &host, "initial", Some(A))).unwrap();
    let before = db
        .current_subject_permission_policy(WORKSPACE, "subject-a")
        .unwrap()
        .unwrap();
    let change = request(&db, &host, &before.change_id, Some(B));
    let result = adopt(&db, &host, &change).unwrap();
    assert_eq!(result.operation_id, before.change_id);
    assert_eq!(
        db.current_subject_permission_policy(WORKSPACE, "subject-a")
            .unwrap(),
        Some(before)
    );
    assert_native_policy(&db);
    assert_eq!(adopt(&db, &host, &change).unwrap(), result);
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
    assert!(
        matches!(loser, Error::Configuration(cause) if cause.details().reason == FailureReason::EditingHeadChanged)
    );
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
    assert_native_policy(&db);
    assert_native_policy(&other);
}

#[path = "config_adoption_failure_tests.rs"]
mod failure_contract;

mod publication_observations {
    use super::*;
    use crate::config_publication::PublicationIdentity;
    use error_contract::{FailureFacts, Storage, Tool, project_failure};
    use protocol::{background_failure::BackgroundFailure, control_failure::RuntimeControlCause};
    use server_api::{
        ConfigurationPublicationOutcome as Outcome, ConfigurationPublicationStage as Stage,
    };

    fn failure(cause: RuntimeControlCause) -> BackgroundFailure {
        let facts = FailureFacts::new(cause).unwrap();
        let projected = project_failure::<Storage, _>(&facts, &()).unwrap();
        BackgroundFailure::from_projected(&projected).unwrap()
    }

    #[test]
    fn publication_observations_reopen_with_original_receipt_and_never_reapply_on_reconcile() {
        let (dir, db) = fixture();
        let host = host(false);
        let request = request(&db, &host, "publication-observed", Some(A));
        let result = adopt(&db, &host, &request).unwrap();
        let identity = PublicationIdentity {
            request_fingerprint: request_fingerprint(WORKSPACE, OWNER, &request).unwrap(),
            receipt: result.clone(),
        };
        let timeout = failure(RuntimeControlCause::TimedOut);
        db.record_configuration_publication(
            &identity,
            Stage::Dispatch,
            None,
            None,
            Outcome::Observed,
            None,
        )
        .unwrap();
        db.record_configuration_publication(
            &identity,
            Stage::Projection,
            None,
            Some("prompt-digest"),
            Outcome::Observed,
            None,
        )
        .unwrap();
        // More than a presentation diagnostic limit: retain every Runtime, not a warning sample.
        for n in 0..25 {
            db.record_configuration_publication(
                &identity,
                Stage::Runtime,
                Some(&format!("runtime-{n:02}")),
                Some("prompt-digest"),
                Outcome::Unconfirmed,
                Some(&timeout),
            )
            .unwrap();
        }
        drop(db);
        let db = open(&dir.path().join("server.sqlite"));
        let read = db
            .configuration_publication(WORKSPACE, OWNER, &request.operation_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            read.identity.request_fingerprint,
            identity.request_fingerprint
        );
        assert_eq!(read.identity.receipt, result);
        assert_eq!(read.observations.len(), 27);
        for observation in &read.observations[2..] {
            assert_eq!(observation.outcome, Outcome::Unconfirmed);
            let projected = observation
                .failure
                .as_ref()
                .unwrap()
                .project_for::<Tool, RuntimeControlCause>(&())
                .unwrap();
            assert_eq!(*projected.details(), RuntimeControlCause::TimedOut);
        }
        assert_eq!(
            db.configuration_request_result(WORKSPACE, OWNER, &request)
                .unwrap(),
            Some(result.clone())
        );
        assert_eq!(adopt(&db, &host, &request).unwrap(), result);
        assert_eq!(
            db.load_workspace_config(WORKSPACE).unwrap(),
            Some(result.state.clone())
        );
        assert_eq!(
            db.configuration_head(WORKSPACE, OWNER).unwrap(),
            Some(result.history_head.clone())
        );
        assert_native_policy(&db);
        assert_eq!(
            db.configuration_publication(WORKSPACE, OWNER, &request.operation_id)
                .unwrap()
                .unwrap()
                .observations
                .len(),
            27
        );
        assert!(
            db.configuration_publication(WORKSPACE, "not-owner", &request.operation_id)
                .is_err()
        );
        assert!(
            db.configuration_publication("foreign-workspace", OWNER, &request.operation_id)
                .is_err()
        );
        assert!(
            db.configuration_publication(WORKSPACE, OWNER, "absent-operation")
                .unwrap()
                .is_none()
        );
        db.with_conn(|conn| {
            assert!(conn.execute("UPDATE workspace_configuration_publication_observations SET outcome='observed'", []).is_err());
            assert!(conn.execute("DELETE FROM workspace_configuration_publication_observations", []).is_err());
            assert!(conn.execute("DELETE FROM workspace_configuration_receipts", []).is_err());
            conn.execute("UPDATE workspaces SET state='deleting' WHERE workspace_id=?1", [WORKSPACE])?;
            Ok(())
        }).unwrap();
        assert!(
            db.configuration_publication(WORKSPACE, OWNER, &request.operation_id)
                .is_err()
        );
    }

    #[test]
    fn publication_writer_rejects_wrong_receipt_identity_and_invalid_stage_payloads() {
        let (_dir, db) = fixture();
        let host = host(false);
        let request = request(&db, &host, "publication-binding", Some(A));
        let result = adopt(&db, &host, &request).unwrap();
        let identity = PublicationIdentity {
            request_fingerprint: request_fingerprint(WORKSPACE, OWNER, &request).unwrap(),
            receipt: result.clone(),
        };
        for field in 0..5 {
            let mut changed = identity.clone();
            match field {
                0 => changed.receipt.workspace_id = "foreign".into(),
                1 => changed.receipt.operation_id = "different-operation".into(),
                2 => changed.request_fingerprint = "different-request".into(),
                3 => changed.receipt.history_head = "different-head".into(),
                _ => changed.receipt.state.snapshot.digest = "different-source".into(),
            }
            assert!(
                db.record_configuration_publication(
                    &changed,
                    Stage::Dispatch,
                    None,
                    None,
                    Outcome::Observed,
                    None
                )
                .is_err()
            );
        }
        let timeout = failure(RuntimeControlCause::TimedOut);
        for (stage, runtime, digest, outcome, cause) in [
            (
                Stage::Runtime,
                None,
                Some("digest"),
                Outcome::Unconfirmed,
                Some(&timeout),
            ),
            (
                Stage::Runtime,
                Some("runtime"),
                None,
                Outcome::Unconfirmed,
                Some(&timeout),
            ),
            (
                Stage::Dispatch,
                Some("runtime"),
                None,
                Outcome::Observed,
                None,
            ),
            (
                Stage::Dispatch,
                None,
                Some("digest"),
                Outcome::Observed,
                None,
            ),
            (Stage::Dispatch, None, None, Outcome::Unconfirmed, None),
            (
                Stage::Dispatch,
                None,
                None,
                Outcome::Observed,
                Some(&timeout),
            ),
            (Stage::Projection, None, None, Outcome::Observed, None),
        ] {
            assert!(
                db.record_configuration_publication(
                    &identity, stage, runtime, digest, outcome, cause
                )
                .is_err()
            );
        }
        assert!(
            db.configuration_publication(WORKSPACE, OWNER, &request.operation_id)
                .unwrap()
                .unwrap()
                .observations
                .is_empty()
        );
        assert_native_policy(&db);
        assert_eq!(
            db.configuration_request_result(WORKSPACE, OWNER, &request)
                .unwrap(),
            Some(result)
        );
    }
}
