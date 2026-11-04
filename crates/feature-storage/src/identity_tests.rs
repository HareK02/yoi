//! Component tests of FeatureStorage + owner receipts + real SQLite. The only
//! injected failures are returned validator errors or abandoned real transactions;
//! these do not claim arbitrary crash/power-loss durability. Run with the normal
//! feature-storage lib target. Unix child tests use this same test executable,
//! isolated roots, explicit child-only environment, and observed lock contention.

use super::*;
use std::cell::Cell;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

const SCOPE: &str = "identity-workspace";
const FEATURE: &str = "identity-feature";
const REQUEST: &str = "host-adoption-request";
const EVIDENCE: &str = "Host selected this legacy Workspace and Feature";
const WAIT: Duration = Duration::from_secs(15);

fn create_records(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("CREATE TABLE IF NOT EXISTS records (value TEXT NOT NULL UNIQUE);")?;
    Ok(())
}

const PLAN: &[FeatureMigration] = &[FeatureMigration::new(1, "records", create_records)];

fn registered(root: &Path) -> (FeatureStorage, RegisteredFeature) {
    let manager = FeatureStorage::new(root);
    let registration = manager
        .register(FeatureRegistration::new(FEATURE, PLAN))
        .unwrap();
    (manager, registration)
}

fn database_path(root: &Path, scope: &str) -> PathBuf {
    root.join(scope)
        .join("features")
        .join(format!("{FEATURE}.sqlite"))
}

fn legacy(root: &Path, scope: &str) -> PathBuf {
    let path = database_path(root, scope);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE records (value TEXT NOT NULL UNIQUE);
         INSERT INTO records VALUES ('preserved');
         CREATE TABLE attribution (scope TEXT NOT NULL, feature TEXT NOT NULL);",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO attribution VALUES (?1, ?2)",
            params![scope, FEATURE],
        )
        .unwrap();
    path
}

fn validate_legacy(tx: &Transaction<'_>) -> Result<()> {
    let attribution: (String, String) =
        tx.query_row("SELECT scope, feature FROM attribution", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
    if attribution != (SCOPE.into(), FEATURE.into()) {
        return Err(FeatureStorageError::operation("legacy attribution differs"));
    }
    // Exercise the actual supported shape, not just a validator invocation flag.
    let _: String = tx.query_row("SELECT value FROM records", [], |r| r.get(0))?;
    Ok(())
}

fn values(database: &FeatureDatabase) -> Vec<String> {
    database
        .with_connection(|connection| {
            Ok(connection
                .prepare("SELECT value FROM records ORDER BY value")?
                .query_map([], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?)
        })
        .unwrap()
}

fn insert(database: &FeatureDatabase, value: &str) {
    database
        .transaction(|tx| {
            tx.execute("INSERT INTO records VALUES (?1)", [value])?;
            Ok(())
        })
        .unwrap();
}

fn assert_identity_error<T>(result: Result<T>) {
    match result {
        Err(FeatureStorageError::Identity(_)) => {}
        Err(error) => panic!("expected identity refusal, got {error:?}"),
        Ok(_) => panic!("expected identity refusal, operation succeeded"),
    }
}

fn has_identity(connection: &Connection) -> bool {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='__yoi_storage_identity')",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn lifecycle_verification_never_provisions_or_adopts_optional_storage() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    scoped.verify_owned_if_present(&registration).unwrap();
    assert!(!root.exists());
    let path = legacy(&root, SCOPE);
    assert_identity_error(scoped.verify_owned_if_present(&registration));
    assert!(!has_identity(&Connection::open(&path).unwrap()));
    let database = scoped
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    let identity = database.identity().clone();
    scoped.verify_owned_if_present(&registration).unwrap();
    assert_eq!(values(&database), vec!["preserved"]);
    drop(database);
    fs::remove_file(&path).unwrap();
    assert!(matches!(scoped.verify_owned_if_present(&registration),
        Err(FeatureStorageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound));
    assert!(!path.exists());
    assert!(matches!(scoped.open_existing(&registration, &identity),
        Err(FeatureStorageError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound));
}

#[test]
fn provision_then_reopen_retains_physical_identity_and_committed_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let (manager, registration) = registered(&root);
    assert!(!root.exists(), "registration must remain lazy");
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.provision(&registration, REQUEST, EVIDENCE).unwrap();
    let identity = database.identity().clone();
    insert(&database, "acknowledged");
    assert_eq!(identity.scope, SCOPE);
    assert_eq!(identity.feature, FEATURE);
    assert!(!identity.physical.is_empty());
    drop(database);
    drop(scoped);
    drop(manager);

    let (manager, registration) = registered(&root);
    let database = manager
        .scope(SCOPE)
        .unwrap()
        .open_existing(&registration, &identity)
        .unwrap();
    assert_eq!(database.identity(), &identity);
    assert_eq!(values(&database), ["acknowledged"]);
    let scoped = manager.scope(SCOPE).unwrap();
    let replay = scoped.provision(&registration, REQUEST, EVIDENCE).unwrap();
    assert_eq!(replay.identity(), &identity);
    assert_eq!(values(&replay), ["acknowledged"]);
    assert_identity_error(scoped.provision(&registration, "different-request", EVIDENCE));
    assert_identity_error(manager.scope("another-workspace").unwrap().provision(
        &registration,
        REQUEST,
        EVIDENCE,
    ));
    assert!(!database_path(&root, "another-workspace").exists());
    let owner = Connection::open(root.join(".owner.sqlite")).unwrap();
    let receipt: (String, i64) = owner
        .query_row(
            "SELECT physical, completed FROM storage_requests WHERE scope=?1 AND feature=?2",
            params![SCOPE, FEATURE],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(receipt, (identity.physical, 1));
}

#[test]
fn unowned_legacy_requires_explicit_evidence_and_real_domain_validation() {
    let temp = tempfile::tempdir().unwrap();
    let path = legacy(temp.path(), SCOPE);
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    assert!(
        scoped.open(&registration).is_err(),
        "existence is not adoption permission"
    );
    assert!(
        scoped
            .provision(&registration, "new-provision-request", EVIDENCE)
            .is_err()
    );
    assert!(!has_identity(&Connection::open(&path).unwrap()));
    assert_identity_error(scoped.adopt_existing(&registration, REQUEST, " ", validate_legacy));
    assert!(!has_identity(&Connection::open(&path).unwrap()));
    let database = scoped
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    let identity = database.identity().clone();
    assert_eq!(values(&database), ["preserved"]);
    drop(database);
    drop(scoped);
    drop(manager);
    let (manager, registration) = registered(temp.path());
    let reopened = manager
        .scope(SCOPE)
        .unwrap()
        .open_existing(&registration, &identity)
        .unwrap();
    assert_eq!(values(&reopened), ["preserved"]);
}

#[test]
fn rejected_legacy_attribution_or_shape_rolls_back_without_completing_ownership() {
    for corrupt in [
        "UPDATE attribution SET scope='another-workspace'",
        "ALTER TABLE records RENAME COLUMN value TO unsupported",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = legacy(temp.path(), SCOPE);
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch(corrupt).unwrap();
        let (manager, registration) = registered(temp.path());
        assert!(
            manager
                .scope(SCOPE)
                .unwrap()
                .adopt_existing(&registration, REQUEST, EVIDENCE, |tx| {
                    tx.execute_batch("CREATE TABLE must_rollback (value TEXT);")?;
                    validate_legacy(tx)
                },)
                .is_err()
        );
        assert!(!has_identity(&raw));
        let leaked: bool = raw.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name IN ('must_rollback', '__yoi_feature_schema_migrations'))",
            [], |r| r.get(0),
        ).unwrap();
        assert!(
            !leaked,
            "failed validator must roll back its own writes and admission"
        );
        let catalog = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
        let completed: i64 = catalog
            .query_row(
                "SELECT completed FROM storage_requests WHERE request=?1",
                [REQUEST],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completed, 0);
    }
}

#[test]
fn adoption_response_loss_replays_same_identity_but_refuses_changed_request_or_target() {
    let temp = tempfile::tempdir().unwrap();
    legacy(temp.path(), SCOPE);
    let (manager, registration) = registered(temp.path());
    let database = manager
        .scope(SCOPE)
        .unwrap()
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    let expected = database.identity().clone();
    insert(&database, "after-admission");
    // Lose the returned handle/manager, not the committed ownership receipt.
    drop(database);
    drop(manager);
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let replay = scoped
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    assert_eq!(replay.identity(), &expected);
    assert_eq!(values(&replay), ["after-admission", "preserved"]);
    assert_identity_error(scoped.adopt_existing(
        &registration,
        "different-request",
        EVIDENCE,
        validate_legacy,
    ));
    assert_identity_error(scoped.adopt_existing(
        &registration,
        REQUEST,
        "changed-authority",
        validate_legacy,
    ));
    legacy(temp.path(), "another-workspace");
    assert_identity_error(manager.scope("another-workspace").unwrap().adopt_existing(
        &registration,
        REQUEST,
        EVIDENCE,
        |_| panic!("changed target must be refused before domain validation"),
    ));
    let owner = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
    let count: i64 = owner
        .query_row("SELECT COUNT(*) FROM storage_requests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1, "replay must not manufacture another identity");
}

#[test]
fn reserved_adoption_reconciles_at_each_real_identity_commit_boundary() {
    // 0: before identity writes, 1: identity written but rolled back,
    // 2: identity committed but owner completion missing, 3: completion response lost.
    for boundary in 0..4 {
        let temp = tempfile::tempdir().unwrap();
        let path = legacy(temp.path(), SCOPE);
        let lifecycle = owner::Lifecycle {
            root: temp.path().into(),
            scope: SCOPE.into(),
        };
        let expected = {
            let _lease = lifecycle.lock(true).unwrap();
            let binding = lifecycle
                .reserve(SCOPE, FEATURE, &path, REQUEST, "adopt", EVIDENCE)
                .unwrap();
            let mut connection =
                Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE).unwrap();
            configure_connection(&connection).unwrap();
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            validate_legacy(&tx).unwrap();
            migrate(&tx, FEATURE, FeatureRegistration::new(FEATURE, PLAN)).unwrap();
            tx.execute("INSERT INTO records VALUES ('boundary-write')", [])
                .unwrap();
            if boundary >= 1 {
                binding.persist(&tx).unwrap();
            }
            if boundary >= 2 {
                tx.commit().unwrap();
            } else {
                tx.rollback().unwrap();
            }
            if boundary == 3 {
                lifecycle.complete(&binding).unwrap();
            }
            binding.identity
        };
        let raw = Connection::open(&path).unwrap();
        assert_eq!(has_identity(&raw), boundary >= 2);
        let count: i64 = raw
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, if boundary >= 2 { 2 } else { 1 });
        drop(raw);
        let (manager, registration) = registered(temp.path());
        let scoped = manager.scope(SCOPE).unwrap();
        if boundary < 3 {
            assert_identity_error(scoped.open_existing(&registration, &expected));
        }
        let database = scoped
            .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
            .unwrap();
        assert_eq!(
            database.identity(),
            &expected,
            "boundary {boundary} changed reserved identity"
        );
        assert_eq!(values(&database).len(), count as usize);
        drop(database);
        drop(scoped);
        drop(manager);
        let (manager, registration) = registered(temp.path());
        let database = manager
            .scope(SCOPE)
            .unwrap()
            .open_existing(&registration, &expected)
            .unwrap();
        assert_eq!(values(&database).len(), count as usize);
        let catalog = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
        let receipt: (i64, i64) = catalog
            .query_row(
                "SELECT COUNT(*), SUM(completed) FROM storage_requests",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(receipt, (1, 1));
    }
}

#[test]
fn owner_completion_sqlite_failure_reconciles_the_already_committed_identity() {
    let temp = tempfile::tempdir().unwrap();
    let path = legacy(temp.path(), SCOPE);
    let (manager, registration) = registered(temp.path());
    {
        let lifecycle = manager.lifecycle(SCOPE);
        let _lease = lifecycle.lock(true).unwrap();
        assert!(lifecycle.lookup(SCOPE, FEATURE).unwrap().is_none());
        lifecycle
            .reserve(SCOPE, FEATURE, &path, REQUEST, "adopt", EVIDENCE)
            .unwrap();
        let catalog = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
        catalog
            .execute_batch(
                "CREATE TRIGGER reject_completion BEFORE UPDATE OF completed ON storage_requests
             BEGIN SELECT RAISE(ABORT, 'injected owner completion failure'); END;",
            )
            .unwrap();
    }
    let scoped = manager.scope(SCOPE).unwrap();
    assert!(matches!(
        scoped.adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy),
        Err(FeatureStorageError::OutcomeUnknown { .. })
    ));
    let raw = Connection::open(&path).unwrap();
    let physical: String = raw
        .query_row("SELECT physical FROM __yoi_storage_identity", [], |r| {
            r.get(0)
        })
        .unwrap();
    let expected = DatabaseIdentity {
        scope: SCOPE.into(),
        feature: FEATURE.into(),
        physical,
    };
    assert_identity_error(scoped.open_existing(&registration, &expected));
    drop(raw);
    drop(scoped);
    drop(manager);
    {
        let catalog = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
        let completed: i64 = catalog
            .query_row(
                "SELECT completed FROM storage_requests WHERE request=?1",
                [REQUEST],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(completed, 0);
        catalog
            .execute_batch("DROP TRIGGER reject_completion")
            .unwrap();
    }
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    assert_eq!(database.identity(), &expected);
    assert_eq!(values(&database), ["preserved"]);
    assert_eq!(
        scoped
            .open_existing(&registration, &expected)
            .unwrap()
            .identity(),
        &expected
    );
}

#[test]
fn open_existing_never_creates_an_unbound_or_missing_owned_database() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let fabricated = DatabaseIdentity {
        scope: SCOPE.into(),
        feature: FEATURE.into(),
        physical: "never-owned".into(),
    };
    assert_identity_error(scoped.open_existing(&registration, &fabricated));
    let path = database_path(temp.path(), SCOPE);
    assert!(!path.exists());
    let database = scoped.open(&registration).unwrap();
    let expected = database.identity().clone();
    drop(database);
    drop(scoped);
    drop(manager);
    fs::remove_file(&path).unwrap();
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    assert!(scoped.open_existing(&registration, &expected).is_err());
    assert!(scoped.open(&registration).is_err());
    assert!(
        !path.exists(),
        "missing owned DB must not be silently recreated"
    );
}

#[test]
fn wrong_expected_workspace_feature_or_physical_identity_never_enters_release_closure() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.open(&registration).unwrap();
    for field in 0..3 {
        let mut wrong = database.identity().clone();
        match field {
            0 => wrong.scope = "another-workspace".into(),
            1 => wrong.feature = "another-feature".into(),
            _ => wrong.physical = Uuid::now_v7().to_string(),
        }
        assert_identity_error(scoped.open_existing(&registration, &wrong));
        assert_identity_error(database.with_identity(&wrong, |_| -> Result<()> {
            panic!("release closure must not run for another target")
        }));
    }
}

#[test]
fn replaced_path_is_refused_even_when_replacement_copies_the_persisted_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.open(&registration).unwrap();
    let expected = database.identity().clone();
    insert(&database, "preserved");
    let path = database_path(temp.path(), SCOPE);
    let replacement = temp.path().join("replacement.sqlite");
    backup_database(&path, &replacement).unwrap();
    fs::rename(&path, temp.path().join("retired.sqlite")).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert_identity_error(database.with_identity(&expected, |_| -> Result<()> {
        panic!("stale inode must not release a result")
    }));
    assert_identity_error(scoped.open_existing(&registration, &expected));
    let (other, other_registration) = registered(temp.path());
    assert_identity_error(
        other
            .scope(SCOPE)
            .unwrap()
            .open_existing(&other_registration, &expected),
    );
}

#[test]
fn corrupt_persisted_identity_or_receipt_is_refused_without_repair() {
    for corrupt in [
        "UPDATE __yoi_storage_identity SET scope='wrong-workspace'",
        "UPDATE __yoi_storage_identity SET feature='wrong-feature'",
        "UPDATE __yoi_storage_identity SET physical='wrong-physical'",
        "UPDATE __yoi_storage_identity SET request='wrong-request'",
        "DELETE FROM __yoi_storage_identity",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let (manager, registration) = registered(temp.path());
        let scoped = manager.scope(SCOPE).unwrap();
        let database = scoped.open(&registration).unwrap();
        let expected = database.identity().clone();
        let raw = Connection::open(database_path(temp.path(), SCOPE)).unwrap();
        raw.execute_batch(corrupt).unwrap();
        assert_identity_error(database.with_connection(|_| -> Result<()> {
            panic!("corrupt identity must be checked before a domain query")
        }));
        assert_identity_error(scoped.open_existing(&registration, &expected));
        drop(database);
        drop(scoped);
        drop(manager);
        let (manager, registration) = registered(temp.path());
        assert_identity_error(
            manager
                .scope(SCOPE)
                .unwrap()
                .open_existing(&registration, &expected),
        );
        let still_matches: i64 = raw.query_row(
            "SELECT COUNT(*) FROM __yoi_storage_identity WHERE scope=?1 AND feature=?2 AND physical=?3",
            params![expected.scope, expected.feature, expected.physical], |r| r.get(0),
        ).unwrap();
        if !corrupt.contains("SET request=") {
            assert_eq!(still_matches, 0, "refusal must not repair the row");
        } else {
            let request: String = raw
                .query_row("SELECT request FROM __yoi_storage_identity", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(request, "wrong-request");
        }
    }
}

#[test]
fn changed_owner_receipt_invalidates_cached_handle_and_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, registration) = registered(temp.path());
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.open(&registration).unwrap();
    let identity = database.identity().clone();
    let catalog = Connection::open(temp.path().join(".owner.sqlite")).unwrap();
    catalog.execute(
        "UPDATE storage_requests SET request='replaced-owner-receipt' WHERE scope=?1 AND feature=?2",
        params![SCOPE, FEATURE],
    ).unwrap();
    assert_identity_error(database.with_identity(&identity, |_| -> Result<()> {
        panic!("cached handle must not release against a replaced owner receipt")
    }));
    assert_identity_error(scoped.open_existing(&registration, &identity));
    let (fresh, registration) = registered(temp.path());
    assert_identity_error(
        fresh
            .scope(SCOPE)
            .unwrap()
            .open_existing(&registration, &identity),
    );
}

#[test]
fn restore_preserves_data_but_assigns_a_new_physical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    let snapshot = temp.path().join("snapshot");
    let (manager, registration) = registered(&source);
    let source_scope = manager.scope(SCOPE).unwrap();
    let database = source_scope.open(&registration).unwrap();
    insert(&database, "snapshot-data");
    let original = database.identity().clone();
    source_scope.backup(&snapshot).unwrap();
    let (restored, registration) = registered(&destination);
    let scoped = restored.scope(SCOPE).unwrap();
    assert!(
        scoped.restore(&snapshot).is_err(),
        "a fresh root is not source provenance"
    );
    scoped.restore_from(&snapshot, &source_scope).unwrap();
    let database = scoped.open(&registration).unwrap();
    let identity = database.identity().clone();
    assert_eq!(identity.scope, original.scope);
    assert_eq!(identity.feature, original.feature);
    assert_ne!(identity.physical, original.physical);
    assert_eq!(values(&database), ["snapshot-data"]);
    assert_identity_error(scoped.open_existing(&registration, &original));
    drop(database);
    drop(scoped);
    drop(restored);
    let (restored, registration) = registered(&destination);
    let database = restored
        .scope(SCOPE)
        .unwrap()
        .open_existing(&registration, &identity)
        .unwrap();
    assert_eq!(values(&database), ["snapshot-data"]);
}

#[test]
fn same_owner_loss_recovery_retires_old_receipts_and_erasure_blocks_every_restore_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let snapshot = temp.path().join("snapshot");
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.provision(&registration, REQUEST, EVIDENCE).unwrap();
    insert(&database, "saved");
    let old = database.identity().clone();
    scoped.backup(&snapshot).unwrap();
    drop(database);
    manager.shutdown().unwrap();
    // Accidental data-file loss is not an authorized erase: the owner is retained.
    fs::remove_dir_all(root.join(SCOPE)).unwrap();
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    scoped.restore(&snapshot).unwrap();
    scoped.restore(&snapshot).unwrap();
    let database = scoped.open(&registration).unwrap();
    let current = database.identity().clone();
    assert_ne!(old, current);
    assert_eq!(values(&database), ["saved"]);
    assert_identity_error(scoped.open_existing(&registration, &old));
    assert_identity_error(scoped.provision(&registration, REQUEST, EVIDENCE));
    let owner = Connection::open(root.join(".owner.sqlite")).unwrap();
    let retained: (String, bool, bool) = owner
        .query_row(
            "SELECT physical,completed,current FROM storage_requests WHERE request=?1",
            [REQUEST],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(retained, (old.physical, true, false));
    drop(database);
    scoped.delete().unwrap();
    let (reopened, _) = registered(&root);
    let source = reopened.scope(SCOPE).unwrap();
    assert!(matches!(
        source.restore(&snapshot),
        Err(FeatureStorageError::ScopeMaintenance(_))
    ));
    let fresh_root = temp.path().join("fresh-target");
    let (target, _) = registered(&fresh_root);
    assert!(matches!(
        target
            .scope(SCOPE)
            .unwrap()
            .restore_from(&snapshot, &source),
        Err(FeatureStorageError::ScopeMaintenance(_))
    ));
    assert!(!fresh_root.join(SCOPE).exists());
}

#[test]
fn review_r1_unowned_incomplete_or_foreign_staging_never_becomes_a_successful_restore() {
    for partial in [true, false] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let snapshot = temp.path().join("snapshot");
        let (manager, registration) = registered(&source);
        let scoped = manager.scope(SCOPE).unwrap();
        let database = scoped.open(&registration).unwrap();
        insert(&database, "requested-source");
        scoped.backup(&snapshot).unwrap();
        let staged = destination.join(format!(".restore-{SCOPE}/features/{FEATURE}.sqlite"));
        fs::create_dir_all(staged.parent().unwrap()).unwrap();
        if partial {
            drop(Connection::open(&staged).unwrap());
        } else {
            let foreign = temp.path().join("foreign");
            let (other, registration) = registered(&foreign);
            let other_scope = other.scope(SCOPE).unwrap();
            let other_db = other_scope.open(&registration).unwrap();
            insert(&other_db, "wrong-source");
            let foreign_snapshot = temp.path().join("foreign-snapshot");
            other_scope.backup(&foreign_snapshot).unwrap();
            fs::copy(
                foreign_snapshot.join(format!("features/{FEATURE}.sqlite")),
                &staged,
            )
            .unwrap();
            other_scope.delete().unwrap();
        }
        let (target, _) = registered(&destination);
        assert!(
            target
                .scope(SCOPE)
                .unwrap()
                .restore_from(&snapshot, &scoped)
                .is_err(),
            "unowned staging must not be interpreted as a completed copy"
        );
        assert!(!destination.join(SCOPE).exists());
    }
}

#[test]
fn review_r3_adopted_legacy_live_scope_recovers_from_stopped_loss_without_erasure_undo() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    legacy(&root, SCOPE);
    let incarnation = Uuid::now_v7();
    fs::create_dir_all(root.join(".scope-locks")).unwrap();
    let record = format!("v1 live {incarnation}\n");
    fs::write(
        root.join(".scope-locks").join(format!("{SCOPE}.lock")),
        &record,
    )
    .unwrap();
    fs::write(
        root.join(SCOPE).join(".incarnation"),
        format!("{incarnation}\n"),
    )
    .unwrap();
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    let old = database.identity().clone();
    let snapshot = temp.path().join("snapshot");
    scoped.backup(&snapshot).unwrap();
    drop(database);
    manager.shutdown().unwrap();
    fs::remove_dir_all(root.join(SCOPE)).unwrap();
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    scoped.restore(&snapshot).unwrap();
    let database = scoped.open(&registration).unwrap();
    assert_eq!(values(&database), ["preserved"]);
    assert_ne!(database.identity(), &old);
    assert_eq!(
        fs::read_to_string(root.join(".scope-locks").join(format!("{SCOPE}.lock"))).unwrap(),
        record
    );
    assert_eq!(
        fs::read_to_string(root.join(SCOPE).join(".incarnation")).unwrap(),
        format!("{incarnation}\n")
    );
    drop(database);
    scoped.delete().unwrap();
    assert!(scoped.restore(&snapshot).is_err());
}

#[test]
fn review_r1_interrupted_copy_keeps_exact_source_and_blocks_competing_or_erased_source_reuse() {
    for (copied, erase_source) in [(false, false), (true, false), (true, true)] {
        let temp = tempfile::tempdir().unwrap();
        let source_root = temp.path().join("source");
        let snapshot = temp.path().join("snapshot");
        let (source, registration) = registered(&source_root);
        let source_scope = source.scope(SCOPE).unwrap();
        let database = source_scope.open(&registration).unwrap();
        insert(&database, "source-A");
        source_scope.backup(&snapshot).unwrap();
        let target_root = temp.path().join("target");
        let (target, registration) = registered(&target_root);
        let error = target
            .restore_scope_with_copy(SCOPE, &snapshot, &source_scope, |from, to| {
                drop(Connection::open(to)?);
                if copied {
                    backup_database(from, to)?;
                }
                Err(FeatureStorageError::operation("injected copy interruption"))
            })
            .unwrap_err();
        assert!(error.to_string().contains("injected copy interruption"));
        assert!(!target_root.join(SCOPE).exists());
        let stage = target_root.join(format!(".restore-{SCOPE}/features"));
        assert!(stage.join(format!("{FEATURE}.copying")).is_file());
        assert!(!stage.join(format!("{FEATURE}.sqlite")).exists());
        let owner = Connection::open(target_root.join(".owner.sqlite")).unwrap();
        let saved: (String, bool) = owner
            .query_row(
                "SELECT evidence,published FROM storage_restores WHERE scope=?1",
                [SCOPE],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(!saved.1);
        if erase_source {
            source_scope.delete().unwrap();
        }
        let (other, other_registration) = registered(&temp.path().join("other"));
        let other_scope = other.scope(SCOPE).unwrap();
        let other_database = other_scope.open(&other_registration).unwrap();
        insert(&other_database, "source-B");
        let other_snapshot = temp.path().join("other-snapshot");
        other_scope.backup(&other_snapshot).unwrap();
        assert_identity_error(
            target
                .scope(SCOPE)
                .unwrap()
                .restore_from(&other_snapshot, &other_scope),
        );
        let retained: (String, bool) = owner
            .query_row(
                "SELECT evidence,published FROM storage_restores WHERE scope=?1",
                [SCOPE],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(retained, saved);
        assert!(!target_root.join(SCOPE).exists());
        if erase_source {
            assert!(matches!(
                target
                    .scope(SCOPE)
                    .unwrap()
                    .restore_from(&snapshot, &source_scope),
                Err(FeatureStorageError::ScopeMaintenance(_))
            ));
        } else {
            // Restart, recopy incomplete bytes, and reconcile only the original source/request.
            target.shutdown().unwrap();
            let (target, registration) = registered(&target_root);
            let scoped = target.scope(SCOPE).unwrap();
            scoped.restore_from(&snapshot, &source_scope).unwrap();
            scoped.restore_from(&snapshot, &source_scope).unwrap();
            assert_eq!(values(&scoped.open(&registration).unwrap()), ["source-A"]);
        }
        let _ = registration;
    }
}

#[test]
fn review_r1_response_loss_after_publication_reconciles_the_saved_physical_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (source, registration) = registered(&temp.path().join("source"));
    let source_scope = source.scope(SCOPE).unwrap();
    let database = source_scope.open(&registration).unwrap();
    insert(&database, "committed-copy");
    let snapshot = temp.path().join("snapshot");
    source_scope.backup(&snapshot).unwrap();
    let root = temp.path().join("target");
    let (target, registration) = registered(&root);
    drop(
        target
            .scope("unrelated")
            .unwrap()
            .open(&registration)
            .unwrap(),
    );
    let owner = Connection::open(root.join(".owner.sqlite")).unwrap();
    owner.execute_batch("CREATE TRIGGER injected_restore_completion BEFORE UPDATE OF completed ON storage_requests WHEN NEW.kind='restore' BEGIN SELECT RAISE(ABORT,'injected restore completion'); END;").unwrap();
    let scoped = target.scope(SCOPE).unwrap();
    assert!(matches!(
        scoped.restore_from(&snapshot, &source_scope),
        Err(FeatureStorageError::OutcomeUnknown { .. })
    ));
    let copied = Connection::open(database_path(&root, SCOPE)).unwrap();
    let physical: String = copied
        .query_row("SELECT physical FROM __yoi_storage_identity", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(
        scoped.open(&registration).is_err(),
        "unfinished completion is not normal admission"
    );
    owner
        .execute_batch("DROP TRIGGER injected_restore_completion;")
        .unwrap();
    scoped.restore_from(&snapshot, &source_scope).unwrap();
    let database = scoped.open(&registration).unwrap();
    assert_eq!(database.identity().physical, physical);
    assert_eq!(values(&database), ["committed-copy"]);
}

struct ChildGuard(Child);

impl ChildGuard {
    fn start(root: &Path, signals: &Path) -> Self {
        Self::start_action(root, signals, None)
    }

    fn start_action(root: &Path, signals: &Path, snapshot: Option<&Path>) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "identity_tests::lifecycle_delete_child",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .current_dir(signals)
                .env("YOI_IDENTITY_DELETE_CHILD_ROOT", root)
                .env("YOI_IDENTITY_DELETE_CHILD_SIGNALS", signals)
                .env(
                    "YOI_IDENTITY_RESTORE_CHILD_SNAPSHOT",
                    snapshot.unwrap_or_else(|| Path::new("")),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }

    fn wait_for(&mut self, signal: &Path) {
        let deadline = Instant::now() + WAIT;
        loop {
            if signal.is_file() {
                return;
            }
            assert!(
                self.0.try_wait().unwrap().is_none(),
                "child exited before {}",
                signal.display()
            );
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                signal.display()
            );
            std::thread::yield_now();
        }
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "delete child failed: {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "delete child did not finish after lease release"
            );
            std::thread::yield_now();
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn lifecycle_delete_child() {
    let Some(root) = std::env::var_os("YOI_IDENTITY_DELETE_CHILD_ROOT") else {
        return;
    };
    let signals = PathBuf::from(std::env::var_os("YOI_IDENTITY_DELETE_CHILD_SIGNALS").unwrap());
    let manager = FeatureStorage::new(PathBuf::from(root));
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(
            manager
                .inner
                .root
                .join(".lifecycle")
                .join(format!("{SCOPE}.lock")),
        )
        .unwrap();
    assert!(
        matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)),
        "exclusive lifecycle lock must contend with active operation"
    );
    fs::write(signals.join("contended"), b"shared lease observed").unwrap();
    if let Some(snapshot) =
        std::env::var_os("YOI_IDENTITY_RESTORE_CHILD_SNAPSHOT").filter(|s| !s.is_empty())
    {
        assert!(matches!(
            manager
                .scope(SCOPE)
                .unwrap()
                .restore(PathBuf::from(snapshot)),
            Err(FeatureStorageError::DestinationExists(_))
        ));
        fs::write(
            signals.join("restore-rejected"),
            b"existing physical lifecycle retained",
        )
        .unwrap();
    } else {
        manager.scope(SCOPE).unwrap().delete().unwrap();
        fs::write(signals.join("deleted"), b"deletion completed").unwrap();
    }
}

#[test]
fn cross_process_restore_waits_for_short_guard_and_cannot_replace_the_live_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("storage");
    let snapshot = temp.path().join("snapshot");
    let signals = temp.path().join("signals");
    fs::create_dir(&signals).unwrap();
    let (manager, registration) = registered(&root);
    let scoped = manager.scope(SCOPE).unwrap();
    let database = scoped.open(&registration).unwrap();
    insert(&database, "still-current");
    scoped.backup(&snapshot).unwrap();
    let identity = database.identity().clone();
    let guard = database.guard(&identity).unwrap();
    let mut child = ChildGuard::start_action(&root, &signals, Some(&snapshot));
    child.wait_for(&signals.join("contended"));
    assert!(!signals.join("restore-rejected").exists());
    assert_eq!(guard.identity(), &identity);
    drop(guard);
    child.finish();
    assert!(signals.join("restore-rejected").is_file());
    assert_eq!(database.identity(), &identity);
    assert_eq!(values(&database), ["still-current"]);
}

#[test]
fn cross_process_delete_waits_for_transaction_and_release_then_persistently_fences_handles() {
    for operation in ["commit", "rollback", "release"] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("storage");
        let signals = temp.path().join("signals");
        fs::create_dir(&signals).unwrap();
        let (manager, registration) = registered(&root);
        let scoped = manager.scope(SCOPE).unwrap();
        let database = scoped.open(&registration).unwrap();
        let identity = database.identity().clone();
        insert(&database, "before-delete");
        let mut child = None;
        let mut contend = || {
            let mut process = ChildGuard::start(&root, &signals);
            process.wait_for(&signals.join("contended"));
            assert!(!signals.join("deleted").exists());
            assert!(database_path(&root, SCOPE).is_file());
            assert!(
                !manager.lifecycle(SCOPE).fenced(SCOPE).unwrap(),
                "delete may not fence an operation before lease release"
            );
            child = Some(process);
        };
        if operation == "release" {
            database
                .with_identity(&identity, |connection| {
                    contend();
                    let value: String =
                        connection.query_row("SELECT value FROM records", [], |r| r.get(0))?;
                    assert_eq!(value, "before-delete");
                    Ok(())
                })
                .unwrap();
        } else {
            let result = database.transaction(|tx| {
                tx.execute("INSERT INTO records VALUES ('transaction-write')", [])?;
                contend();
                if operation == "rollback" {
                    Err(FeatureStorageError::operation("rollback held transaction"))
                } else {
                    Ok(())
                }
            });
            if operation == "rollback" {
                assert!(matches!(result, Err(FeatureStorageError::Operation(_))));
            } else {
                result.unwrap();
            }
        }
        let mut child = child.unwrap();
        child.finish();
        assert!(signals.join("deleted").is_file());
        assert!(!root.join(SCOPE).exists());
        let entered = Cell::new(false);
        assert!(
            database
                .with_identity(&identity, |_| {
                    entered.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(
            !entered.get(),
            "stale handles must not enter release closure after delete"
        );
        assert!(
            database
                .transaction(|_| -> Result<()> { panic!("stale transaction entered") })
                .is_err()
        );
        assert!(scoped.open_existing(&registration, &identity).is_err());
        let (fresh, registration) = registered(&root);
        assert!(fresh.scope(SCOPE).unwrap().open(&registration).is_err());
        fresh.scope(SCOPE).unwrap().delete().unwrap();
        assert!(
            !root.join(SCOPE).exists(),
            "fresh manager must observe persistent deletion fence"
        );
    }
}

#[test]
fn published_physical_identity_receipt_and_erasure_fences_survive_authorized_adoption() {
    let temp = tempfile::tempdir().unwrap();
    let path = legacy(temp.path(), SCOPE);
    let physical = Uuid::now_v7().to_string();
    let fingerprint = format!("sha256:{}", "a".repeat(64));
    let connection = Connection::open(&path).unwrap();
    // Exact published physical DDL from both 9edebfa and 7b39a44 identity.rs.
    connection.execute_batch("CREATE TABLE __yoi_feature_database_identity (
        singleton INTEGER PRIMARY KEY CHECK(singleton=1), format INTEGER NOT NULL CHECK(format=1),
        scope_id TEXT NOT NULL,feature_id TEXT NOT NULL,incarnation TEXT NOT NULL);
        CREATE TABLE __yoi_feature_provisioning_receipt (singleton INTEGER PRIMARY KEY CHECK(singleton=1),request_fingerprint TEXT NOT NULL);").unwrap();
    connection
        .execute(
            "INSERT INTO __yoi_feature_database_identity VALUES(1,1,?1,?2,?3)",
            params![SCOPE, FEATURE, physical],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO __yoi_feature_provisioning_receipt VALUES(1,?1)",
            [&fingerprint],
        )
        .unwrap();
    let (manager, registration) = registered(temp.path());
    let scope = manager.scope(SCOPE).unwrap();
    let database = scope
        .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
        .unwrap();
    assert_eq!(database.identity().physical, physical);
    assert_eq!(values(&database), ["preserved"]);
    let saved: (String,String) = connection.query_row(
        "SELECT incarnation,request_fingerprint FROM __yoi_feature_database_identity CROSS JOIN __yoi_feature_provisioning_receipt",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(saved, (physical, fingerprint));
    scope.delete().unwrap();
    let (other, registration) = registered(temp.path());
    assert!(other.scope(SCOPE).unwrap().open(&registration).is_err());
    assert!(
        temp.path()
            .join(".lifecycle")
            .join(format!("{SCOPE}.deleted"))
            .exists()
    );
}

#[test]
fn published_scope_erasure_or_unfinished_lifecycle_is_not_lost_on_owner_upgrade() {
    for phase in ["deleted", "deleting", "creating", "restoring"] {
        let temp = tempfile::tempdir().unwrap();
        let path = legacy(temp.path(), SCOPE);
        fs::create_dir(temp.path().join(".scope-locks")).unwrap();
        fs::write(
            temp.path()
                .join(".scope-locks")
                .join(format!("{SCOPE}.lock")),
            format!("v1 {phase} {}\n", Uuid::now_v7()),
        )
        .unwrap();
        let (manager, registration) = registered(temp.path());
        assert!(
            manager
                .scope(SCOPE)
                .unwrap()
                .adopt_existing(&registration, REQUEST, EVIDENCE, validate_legacy)
                .is_err()
        );
        assert!(!has_identity(&Connection::open(path).unwrap()));
        assert!(!temp.path().join(".owner.sqlite").exists());
    }
}

#[test]
fn explicit_guard_pins_short_decision_without_holding_connection_or_sqlite_transaction() {
    let temp = tempfile::tempdir().unwrap();
    let (manager, registration) = registered(temp.path());
    let scope = manager.scope(SCOPE).unwrap();
    let database = scope.open(&registration).unwrap();
    let guard = database.guard(database.identity()).unwrap();
    assert_eq!(guard.identity(), database.identity());
    // Unlike a connection closure, the short guard permits consumer queries and
    // their own transaction while holding the same physical lifecycle lease.
    let reopened = scope
        .open_existing(&registration, guard.identity())
        .unwrap();
    insert(&reopened, "guarded");
    assert_eq!(values(&database), ["guarded"]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(temp.path().join(".lifecycle").join(format!("{SCOPE}.lock")))
        .unwrap();
    assert!(matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
    drop(guard);
    lock.try_lock().unwrap();
}
