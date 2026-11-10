use super::*;

use config_source::ConfigSchemaContribution;
use feature_permissions::{NamespacePermissions, PermissionAssignment, PermissionReference};
use rusqlite::OptionalExtension;
use server_api::authorization::CurrentSubjectPolicy;

use crate::config_adoption::automatic;
use crate::configuration_failure::ConfigurationFailureReason;

fn saved_plan(db: &SqliteWorkspaceStore) -> Option<(String, serde_json::Value)> {
    db.with_conn(|conn| {
        let saved: Option<(String, String)> = conn
            .query_row(
                "SELECT transfer_id,plan_json FROM workspace_permission_migration_plans WHERE workspace_id=?1",
                [WORKSPACE],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(saved.map(|(id, raw)| (id, serde_json::from_str(&raw).unwrap())))
    })
    .unwrap()
}

fn fail_publication(db: &SqliteWorkspaceStore) {
    db.with_conn(|conn| {
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_automatic_publication
             BEFORE INSERT ON workspace_configuration_authorities
             BEGIN SELECT RAISE(ABORT,'injected automatic publication failure'); END;",
        )?;
        Ok(())
    })
    .unwrap();
}

fn canonical_store(db: &SqliteWorkspaceStore, features: &FeatureFixture) -> SubjektivStore {
    features
        .store
        .clone()
        .with_canonical_permission_source(Arc::new(
            crate::subject_policy_source::CanonicalSubjectPolicySource {
                store: db.clone(),
                host: host(),
            },
        ))
}

#[test]
fn automatic_migration_omits_deletion_fenced_subjects_and_preserves_retained_records() {
    let (_root, db, features, subjects) = fixture();
    replace(
        &features.store,
        &subjects.limited,
        "unsupported-before-deletion",
        vec![limited("file.unknown")],
        vec![],
    );
    features
        .store
        .begin_subject_deletion(&subjects.limited, "delete-before-migration")
        .unwrap();
    let legacy_before = legacy_records(&features);
    let seed = migration(&db, &features, &host());
    assert!(
        !seed
            .snapshot
            .sources
            .iter()
            .any(|source| source.subject_id == subjects.limited)
    );
    assert!(automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    let current = db.configuration_current(WORKSPACE, OWNER).unwrap();
    assert!(current.canonical);
    assert!(!current.policies.contains_key(&subjects.limited));
    assert_eq!(
        current.policies[&subjects.empty].change_id,
        subjects.empty_source.change_id
    );
    assert_eq!(legacy_records(&features), legacy_before);
    let canonical = canonical_store(&db, &features);
    assert!(matches!(
        canonical.current_subject_policy(&subjects.limited),
        Err(SubjektivError::SubjectNotFound(_))
    ));
    assert!(
        canonical
            .bind_worker_execution_grant_with_host(&subjects.live_grant, &host())
            .is_err()
    );
    drop(canonical);
    drop(db);
    features.close();
}

#[test]
fn deletion_after_seal_blocks_resume_without_rewriting_plan_or_failing_ordinary_selection() {
    let (_root, db, features, subjects) = fixture();
    fail_publication(&db);
    assert!(automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).is_err());
    let identity = features.store.database_identity().unwrap();
    let seal = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap();
    let plan = saved_plan(&db);
    let before = server_records(&db);
    features
        .store
        .begin_subject_deletion(&subjects.limited, "delete-after-seal")
        .unwrap();
    let error = automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap_err();
    assert!(matches!(error, Error::SubjektivSubjectNotFound(ref id) if id == &subjects.limited));
    assert!(automatic::blocked_reason(&error).is_some());
    assert_eq!(saved_plan(&db), plan);
    assert_eq!(server_records(&db), before);
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        seal
    );
    assert_eq!(
        db.worker_assignment(WORKSPACE, &host(), None).unwrap(),
        crate::policy_config::PermissionSource::default()
            .worker_default
            .assignment()
    );
    drop(db);
    features.close();
}

fn assert_preserved_policies(
    db: &SqliteWorkspaceStore,
    features: &FeatureFixture,
    subjects: &Subjects,
    seed: &MigrationSeed,
) {
    let current = db.configuration_current(WORKSPACE, OWNER).unwrap();
    assert!(current.canonical);
    assert!(current.history_head.is_some());
    assert_eq!(current.state, seed.state);
    assert_eq!(current.policies, seed.policies);
    assert_existing_data_preserved(&seed.original, &current.state);
    let canonical = canonical_store(db, features);
    assert_eq!(
        canonical.subject_permissions(&subjects.unset).unwrap(),
        None
    );
    assert_eq!(
        canonical.current_subject_policy(&subjects.unset).unwrap(),
        None
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.empty).unwrap(),
        Some(subjects.empty_source.clone())
    );
    assert_eq!(
        canonical.subject_permissions(&subjects.limited).unwrap(),
        Some(subjects.limited_source.clone())
    );
    for (subject, policy) in &seed.policies {
        assert_eq!(
            canonical.current_subject_policy(subject).unwrap(),
            Some(policy.current()),
            "{subject}"
        );
    }
    for grant in [&subjects.stale_grant, &subjects.live_grant] {
        assert_eq!(
            canonical
                .worker_execution_grant(&grant.runtime_id, &grant.worker_id)
                .unwrap(),
            Some(grant.clone())
        );
    }
}

#[test]
fn automatic_migration_preserves_policy_identities_and_grant_limits_without_owner_adoption() {
    let (_root, db, features, subjects) = fixture();
    let current_subject = features
        .store
        .create_subject(SubjectRole::new("current-format fixture").unwrap())
        .unwrap()
        .id;
    let assignment = |key| PermissionAssignment {
        grants: vec![PermissionReference::Namespace(NamespacePermissions {
            namespace: NamespaceId::parse("file").unwrap(),
            keys: vec![PermissionKey::parse(key).unwrap()],
        })],
        ..Default::default()
    };
    let current_policy = CurrentSubjectPolicy {
        reference_encoding: server_api::authorization::PermissionReferenceEncoding::FeatureSetsV2,
        change_id: "existing-current-format-change".into(),
        execution: PermissionAssignment {
            revoked: true,
            ..assignment("file.read")
        },
        delegation: PermissionAssignment {
            exclusions: assignment("file.read.text").grants,
            ..assignment("file.read.text")
        },
        legacy_constraints: None,
    };
    features
        .database
        .try_transaction::<_, SubjektivError>(|tx| {
            tx.execute(
                "INSERT INTO subject_permission_changes(subject_id,change_id,record_json) VALUES(?1,?2,?3)",
                rusqlite::params![
                    current_subject,
                    current_policy.change_id,
                    serde_json::to_string(&current_policy)?
                ],
            )?;
            tx.execute(
                "UPDATE subject_permissions SET change_id=?2 WHERE subject_id=?1",
                rusqlite::params![current_subject, current_policy.change_id],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        features
            .store
            .current_subject_policy(&current_subject)
            .unwrap(),
        Some(current_policy.clone())
    );
    let seed = migration(&db, &features, &host());
    let legacy_before = legacy_records(&features);
    let identity = features.store.database_identity().unwrap();
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert!(automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    assert_preserved_policies(&db, &features, &subjects, &seed);
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(saved_plan(&db).is_none());
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    assert_eq!(sealed.snapshot, seed.snapshot);
    db.with_conn(|conn| {
        let authority: (String, String, String) = conn.query_row(
            "SELECT transfer_identity,legacy_physical_identity,legacy_export_digest FROM workspace_configuration_authorities WHERE workspace_id=?1",
            [WORKSPACE],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(
            authority,
            (
                sealed.transfer_id.clone(),
                identity.physical_id.clone(),
                seed.snapshot.export_digest.clone()
            )
        );
        assert!(table_rows(conn, "workspace_configuration_receipts")?.is_empty());
        Ok(())
    })
    .unwrap();
    let canonical = canonical_store(&db, &features);
    let migrated_current = canonical
        .current_subject_policy(&current_subject)
        .unwrap()
        .unwrap();
    assert_eq!(migrated_current, current_policy);
    assert!(migrated_current.execution.revoked);
    assert_eq!(
        migrated_current.delegation.exclusions,
        assignment("file.read.text").grants
    );
    for lane in [&migrated_current.execution, &migrated_current.delegation] {
        assert!(host().resolve(Some(lane)).projection().allowed.is_empty());
    }
    for operation in ["file.read", "file.read.text"] {
        let mut denied = grant(
            &current_subject,
            &migrated_current.change_id,
            &format!("denied-current-{operation}"),
        );
        denied.permissions = vec![limited(operation)];
        assert!(matches!(
            canonical.bind_worker_execution_grant_with_host(&denied, &host()),
            Err(SubjektivError::AuthorizationDenied(_))
        ));
        assert_eq!(
            canonical
                .worker_execution_grant(&denied.runtime_id, &denied.worker_id)
                .unwrap(),
            None
        );
    }
    let live = grant(
        &subjects.limited,
        &subjects.live_grant.source_change_id,
        "new-live-child",
    );
    canonical
        .bind_worker_execution_grant_with_host(&live, &host())
        .unwrap();
    let stale = grant(
        &subjects.limited,
        &subjects.stale_grant.source_change_id,
        "new-stale-child",
    );
    assert!(matches!(
        canonical.bind_worker_execution_grant_with_host(&stale, &host()),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    let mut wider = live.clone();
    wider.worker_id = "new-wider-child".into();
    wider.permissions[0].resource = PermissionResource::Workdir {
        working_directory_id: "one-workdir".into(),
        path: ".".into(),
        recursive: true,
    };
    assert!(matches!(
        canonical.bind_worker_execution_grant_with_host(&wider, &host()),
        Err(SubjektivError::AuthorizationDenied(_))
    ));
    assert!(matches!(
        features
            .store
            .set_subject_permissions(&subjects.limited, &subjects.exact_legacy_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    drop(canonical);
    drop(db);
    features.close();
}

#[test]
fn automatic_migration_is_a_noop_after_commit_and_restart() {
    let (root, db, features, subjects) = fixture();
    let seed = migration(&db, &features, &host());
    let identity = features.store.database_identity().unwrap();
    let physical_identity = features.database.identity().clone();
    assert!(automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    let committed = server_records(&db);
    let legacy = legacy_records(&features);
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap();
    assert!(!automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    assert_eq!(server_records(&db), committed);
    assert_eq!(legacy_records(&features), legacy);
    drop(db);
    features.close();

    let db = server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&physical_identity));
    assert!(!automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    assert_eq!(server_records(&db), committed);
    assert_eq!(legacy_records(&features), legacy);
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        sealed
    );
    assert!(saved_plan(&db).is_none());
    assert_preserved_policies(&db, &features, &subjects, &seed);
    drop(db);
    features.close();
}

#[test]
fn adopted_permission_authority_rejects_a_different_native_database() {
    let (_root, db, features, _subjects) = fixture();
    automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap();
    let committed = server_records(&db);
    let other_root = tempfile::tempdir().unwrap();
    let other = FeatureFixture::open(other_root.path(), None);
    assert!(matches!(
        automatic::migrate(&db, &other.store, WORKSPACE, schema(), &host()),
        Err(Error::RegistryInconsistency(_))
    ));
    assert_eq!(server_records(&db), committed);
    let identity = other.store.database_identity().unwrap();
    assert!(
        other
            .store
            .permission_transfer_receipt(&identity)
            .unwrap()
            .is_none()
    );
    other.close();
    drop(db);
    features.close();
}

#[test]
fn unsupported_automatic_migration_leaves_sources_unsealed_and_old_writer_usable() {
    let (_root, db, features, subjects) = fixture();
    replace(
        &features.store,
        &subjects.limited,
        "unsupported-automatic-operation",
        vec![limited("file.unknown")],
        vec![limited("file.read.text")],
    );
    let before = server_records(&db);
    let legacy_before = legacy_records(&features);
    let schema_before = feature_schema(&features);
    let identity = features.store.database_identity().unwrap();
    let error = automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap_err();
    assert!(
        matches!(error, Error::WorkspaceConfigConflict(ref message) if message.contains("explicit mapping is required")),
        "{error}"
    );
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(feature_schema(&features), schema_before);
    assert!(saved_plan(&db).is_none());
    assert!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap()
            .is_none()
    );
    let (_, updated) = replace(
        &features.store,
        &subjects.limited,
        "old-writer-after-automatic-rejection",
        vec![],
        vec![],
    );
    assert_eq!(
        features
            .store
            .subject_permissions(&subjects.limited)
            .unwrap(),
        Some(updated)
    );
    drop(db);
    features.close();
}

#[test]
fn automatic_migration_does_not_replace_a_pending_owner_transfer() {
    let (_root, db, features, _subjects) = fixture();
    let host = host();
    let seed = migration(&db, &features, &host);
    let request = migration_request(&seed, &host);
    let before = server_records(&db);
    let legacy_before = legacy_records(&features);
    fail_publication(&db);
    let error = adopt_migration(&db, &features, &host, &seed, &request).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected automatic publication failure"),
        "{error}"
    );
    let sealed = features
        .store
        .permission_transfer_receipt(&seed.snapshot.identity)
        .unwrap()
        .unwrap();
    assert_eq!(
        sealed.transfer_id,
        request_fingerprint(WORKSPACE, OWNER, &request).unwrap()
    );
    let error = automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host).unwrap_err();
    assert!(
        matches!(error, Error::WorkspaceConfigConflict(ref message) if message.contains("pending owner permission transfer")),
        "{error}"
    );
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(saved_plan(&db).is_none());
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&seed.snapshot.identity)
            .unwrap(),
        Some(sealed)
    );
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER fail_automatic_publication")?;
        Ok(())
    })
    .unwrap();
    let adopted = adopt_migration(&db, &features, &host, &seed, &request).unwrap();
    assert_eq!(adopted.state, seed.state);
    drop(db);
    features.close();
}

#[test]
fn pending_automatic_migration_rejects_catalog_change_then_resumes_with_original_catalog() {
    let (_root, db, features, subjects) = fixture();
    let original_host = host();
    let seed = migration(&db, &features, &original_host);
    let before = server_records(&db);
    let legacy_before = legacy_records(&features);
    let identity = features.store.database_identity().unwrap();
    fail_publication(&db);
    let error =
        automatic::migrate(&db, &features.store, WORKSPACE, schema(), &original_host).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected automatic publication failure"),
        "{error}"
    );
    let saved = saved_plan(&db).unwrap();
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    assert_eq!(saved.0, sealed.transfer_id);
    assert_eq!(sealed.snapshot, seed.snapshot);
    assert_eq!(server_records(&db), before);
    db.with_conn(|conn| {
        conn.execute_batch("DROP TRIGGER fail_automatic_publication")?;
        Ok(())
    })
    .unwrap();

    let mut changed_host = original_host.clone();
    let feature = changed_host
        .register_feature(FeatureIdentity::parse("fixture:catalog-change").unwrap())
        .unwrap();
    let namespace = changed_host
        .bind_namespace(&feature, NamespaceId::parse("extra").unwrap())
        .unwrap();
    changed_host
        .declare(
            &namespace,
            NamespaceDeclaration {
                permissions: vec![PermissionDefinition {
                    key: PermissionKey::parse("extra.read").unwrap(),
                    description: String::new(),
                    children: vec![],
                }],
                sets: vec![],
            },
        )
        .unwrap();
    assert_ne!(
        catalog_digest(&changed_host).unwrap(),
        catalog_digest(&original_host).unwrap()
    );
    let error =
        automatic::migrate(&db, &features.store, WORKSPACE, schema(), &changed_host).unwrap_err();
    assert!(
        matches!(error, Error::Configuration(ref cause) if cause.details().reason == ConfigurationFailureReason::CatalogChanged),
        "{error}"
    );
    assert_eq!(saved_plan(&db), Some(saved));
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(seed.original.clone())
    );
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert!(!db.configuration_policy_is_canonical(WORKSPACE).unwrap());
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed.clone())
    );

    assert!(automatic::migrate(&db, &features.store, WORKSPACE, schema(), &original_host).unwrap());
    assert_preserved_policies(&db, &features, &subjects, &seed);
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(saved_plan(&db).is_none());
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed)
    );
    drop(db);
    features.close();
}

#[test]
fn automatic_publish_failure_retains_plan_and_seal_and_reopen_uses_frozen_schema() {
    let (root, db, features, subjects) = fixture();
    let seed = migration(&db, &features, &host());
    let before = server_records(&db);
    let legacy_before = legacy_records(&features);
    let physical_identity = features.database.identity().clone();
    let identity = features.store.database_identity().unwrap();
    fail_publication(&db);
    let error = automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("injected automatic publication failure"),
        "{error}"
    );
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(
        db.load_workspace_config(WORKSPACE).unwrap(),
        Some(seed.original.clone())
    );
    assert_eq!(db.configuration_head(WORKSPACE, OWNER).unwrap(), None);
    assert!(!db.configuration_policy_is_canonical(WORKSPACE).unwrap());
    let saved =
        saved_plan(&db).expect("plan must survive the separate seal and failed publication");
    let sealed = features
        .store
        .permission_transfer_receipt(&identity)
        .unwrap()
        .unwrap();
    assert_eq!(saved.0, sealed.transfer_id);
    assert_eq!(saved.1["editing_head"], serde_json::Value::Null);
    assert_eq!(
        saved.1["catalog_digest"],
        serde_json::to_value(catalog_digest(&host()).unwrap()).unwrap()
    );
    assert_eq!(
        saved.1["original"],
        serde_json::to_value(&seed.original).unwrap()
    );
    assert_eq!(
        saved.1["target"],
        serde_json::to_value(&seed.state).unwrap()
    );
    assert_eq!(
        saved.1["legacy"],
        serde_json::to_value(&seed.snapshot).unwrap()
    );
    assert_eq!(
        saved.1["policies"],
        serde_json::to_value(&seed.policies).unwrap()
    );
    assert_eq!(sealed.snapshot, seed.snapshot);
    assert!(matches!(
        features
            .store
            .set_subject_permissions(&subjects.limited, &subjects.exact_legacy_update),
        Err(SubjektivError::PermissionAuthoritySealed)
    ));
    let canonical = canonical_store(&db, &features);
    assert!(canonical.current_subject_policy(&subjects.limited).is_err());
    assert!(canonical.subject_permissions(&subjects.limited).is_err());
    drop(canonical);
    drop(db);
    features.close();

    let db = server(root.path());
    let features = FeatureFixture::open(&root.path().join("features"), Some(&physical_identity));
    assert_eq!(saved_plan(&db), Some(saved.clone()));
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed.clone())
    );
    let mut contributions = schema().contributions;
    contributions.push(
        ConfigSchemaContribution::new(
            "fixture:startup-schema-upgrade",
            "startup",
            "1",
            "{ startup = { enabled = Bool default true; }; }",
        )
        .unwrap(),
    );
    let newer_schema = WorkspaceConfigSchemaBundle::compose(contributions).unwrap();
    assert_ne!(
        newer_schema.fingerprint,
        seed.original.contract.schema_bundle.fingerprint
    );
    assert!(evaluate_workspace_config_state(&seed.state, newer_schema.clone()).is_err());
    let materialized = db
        .ensure_workspace_config_materialized_with_schema(
            WORKSPACE,
            "newer-schema-startup",
            newer_schema.clone(),
        )
        .unwrap();
    assert_eq!(materialized, seed.original);
    assert_eq!(saved_plan(&db), Some(saved));
    assert_eq!(server_records(&db), before);
    assert_eq!(legacy_records(&features), legacy_before);
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed.clone())
    );
    assert!(automatic::migrate(&db, &features.store, WORKSPACE, newer_schema, &host()).unwrap());
    assert_preserved_policies(&db, &features, &subjects, &seed);
    assert_eq!(legacy_records(&features), legacy_before);
    assert!(saved_plan(&db).is_none());
    assert_eq!(
        features
            .store
            .permission_transfer_receipt(&identity)
            .unwrap(),
        Some(sealed)
    );
    let committed = server_records(&db);
    assert!(!automatic::migrate(&db, &features.store, WORKSPACE, schema(), &host()).unwrap());
    assert_eq!(server_records(&db), committed);
    drop(db);
    features.close();
}
