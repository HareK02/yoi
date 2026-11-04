//! Storage component fixtures: real SQLite and FeatureStorage registration/open/reopen.
//! Published extension DDL is frozen shape evidence, not a replacement domain implementation.
//! The suite proves physical retention and receipt/shape rejection, not permission adoption,
//! Job recovery, Journal release, or Host authorization. Identity/adoption races belong to the
//! FeatureStorage owner's suite. No production DB, Runtime, or external service is used.

use super::*;
use crate::{REGISTRATION, SubjectRole, SubjektivStore};
use feature_storage::{FeatureDatabase, FeatureRegistration, FeatureStorage, ScopedFeatureStorage};
use rusqlite::types::Value;
use std::path::Path;

const SCOPE: &str = "published-subjektiv-formats";
const PUBLISHED: [Format; 7] = [
    Format::Permission18,
    Format::Consolidation18,
    Format::Fixed18,
    Format::Cancellation19,
    Format::Consolidation20,
    Format::Fixed20,
    Format::Cancellation20,
];

type Receipt = (u32, String, String);
type TableRows = Vec<(String, Vec<Vec<Value>>)>;

fn open(root: &Path, registration: FeatureRegistration) -> (ScopedFeatureStorage, FeatureDatabase) {
    let scope = FeatureStorage::new(root).scope(SCOPE).unwrap();
    let registered = scope.register(registration).unwrap();
    let database = scope.open(&registered).unwrap();
    (scope, database)
}

fn receipts(connection: &Connection) -> Vec<Receipt> {
    connection
        .prepare(&format!(
            "SELECT version,name,applied_at FROM {HISTORY_TABLE} ORDER BY version"
        ))
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap()
}

fn mark_receipts(connection: &Connection) {
    // Distinct saved bytes detect delete/reinsert, even in a single wall-clock second.
    connection
        .execute(
            "UPDATE __yoi_feature_schema_migrations
             SET applied_at=printf('original-published-%d',version)",
            [],
        )
        .unwrap();
}

fn all_rows(connection: &Connection) -> TableRows {
    let names: Vec<String> = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let quoted = name.replace('"', "\"\"");
            let columns = connection
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap()
                .column_count();
            let order = (1..=columns)
                .map(|column| column.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = connection
                .prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY {order}"))
                .unwrap();
            let rows = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|column| row.get(column))
                        .collect::<std::result::Result<Vec<Value>, _>>()
                })
                .unwrap()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap();
            (name, rows)
        })
        .collect()
}

fn history(connection: &Connection) -> Vec<(u32, String)> {
    receipts(connection)
        .into_iter()
        .map(|(version, name, _)| (version, name))
        .collect()
}

fn assert_valid(connection: &mut Connection) {
    let tx = connection.transaction().unwrap();
    validate(&tx, &history(&tx)).unwrap();
    tx.commit().unwrap();
}

fn assert_foreign_keys(connection: &Connection) {
    assert!(
        connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .query([])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
}

// These fixture functions execute the historical SQL, including rebuilds and triggers. They
// never invent alternative tables with similar names or stamp a canonical receipt on old data.
fn permission_fixture(connection: &Connection) {
    connection.execute_batch(PERMISSION_SEAL_SQL).unwrap();
    for table in PERMISSION_FENCE_TABLES {
        for action in ["INSERT", "UPDATE"] {
            connection
                .execute_batch(&format!(
                    "CREATE TRIGGER permission_sealed_{table}_{action} BEFORE {action} ON {table}
                 WHEN EXISTS(SELECT 1 FROM permission_authority_seal)
                 BEGIN SELECT RAISE(ABORT, 'legacy permission authority is sealed'); END;"
                ))
                .unwrap();
        }
        connection
            .execute_batch(&format!(
                "CREATE TRIGGER permission_sealed_{table}_DELETE BEFORE DELETE ON {table}
             WHEN EXISTS(SELECT 1 FROM permission_authority_seal)
               AND NOT EXISTS(SELECT 1 FROM subject_deletions WHERE subject_id=OLD.subject_id)
             BEGIN SELECT RAISE(ABORT, 'legacy permission authority is sealed'); END;"
            ))
            .unwrap();
    }
    connection.execute_batch(
        "INSERT INTO subject_creation_receipts VALUES('creation','historical-subject','fingerprint',' {\"execution\":[]} ');
         INSERT INTO permission_authority_seal VALUES(1,'transfer',' {\"opaque_saved_export\":true} ');"
    ).unwrap();
}

fn scope_fixture(connection: &Connection) {
    connection.execute_batch(FIXED_SCOPE_SQL).unwrap();
    // Opaque saved payloads prove byte retention, not valid domain release authority.
    connection.execute_batch(
        "INSERT INTO journal_final_scopes VALUES('fixed','remove','binding','owner-json','members',1,1,'session',NULL);
         INSERT INTO journal_final_scope_sessions VALUES('fixed','session','metadata-json');
         INSERT INTO journal_final_scope_pages VALUES('fixed','','session','page-digest');
         UPDATE journal_final_scopes SET scope_digest='frozen' WHERE request_id='fixed';
         INSERT INTO journal_scope_release_receipts VALUES('fixed','release-json');"
    ).unwrap();
}

fn cancellation_fixture(connection: &Connection) {
    let before: Vec<_> = all_rows(connection)
        .into_iter()
        .filter(|(name, _)| {
            name.starts_with("journal_final_scope") || name == "journal_scope_release_receipts"
        })
        .collect();
    connection.execute_batch(CANCELLATION_SQL).unwrap();
    for (name, rows) in before {
        let after = all_rows(connection);
        assert_eq!(
            after.iter().find(|(table, _)| table == &name).unwrap().1,
            rows
        );
    }
    connection
        .execute_batch(
            r#"INSERT INTO journal_scope_cancellations VALUES('subject-release:cancel',
            '{"request_id":"subject-release:cancel","removal_operation_id":"cancel"}');"#,
        )
        .unwrap();
}

fn consolidation_fixture(connection: &Connection) {
    connection.execute_batch(CONSOLIDATION_SQL).unwrap();
    connection.execute_batch(
        "INSERT INTO consolidation_write_bindings VALUES('published-subjektiv-formats','job','attempt','subject','runtime','worker','input','grant','deadline','revoked');"
    ).unwrap();
}

fn record(connection: &Connection, version: u32, name: &str) {
    connection
        .execute(
            &format!("INSERT INTO {HISTORY_TABLE}(version,name) VALUES(?1,?2)"),
            params![version, name],
        )
        .unwrap();
}

fn install_published_fixture(connection: &Connection, format: Format) {
    match format {
        Format::Permission18 => {
            permission_fixture(connection);
            record(connection, 18, PERMISSION_18);
        }
        Format::Consolidation18 | Format::Consolidation20 => {
            consolidation_fixture(connection);
            record(connection, 18, CONSOLIDATION_18);
            if format == Format::Consolidation20 {
                scope_fixture(connection);
                cancellation_fixture(connection);
                record(connection, 20, CONVERGENCE_20);
            }
        }
        Format::Fixed18 | Format::Cancellation19 | Format::Fixed20 | Format::Cancellation20 => {
            scope_fixture(connection);
            record(connection, 18, FIXED_18);
            if format != Format::Fixed18 {
                cancellation_fixture(connection);
                if matches!(format, Format::Cancellation19 | Format::Cancellation20) {
                    record(connection, 19, CANCELLATION_19);
                }
            }
            if matches!(format, Format::Fixed20 | Format::Cancellation20) {
                consolidation_fixture(connection);
                record(connection, 20, CONVERGENCE_20);
            }
        }
        Format::Common(_) => unreachable!("common fixtures use their original registration"),
    }
}

fn published_fixture(root: &Path, format: Format) {
    let (scope, database) = open(
        root,
        FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, &MIGRATIONS[..COMMON_COUNT]),
    );
    let store = SubjektivStore::from_database(database.clone()).unwrap();
    store
        .create_subject_with_behavior(
            SubjectRole::new("retained").unwrap(),
            "原文: retained\n".into(),
        )
        .unwrap();
    database.transaction(|tx| {
        // Permanent erasure evidence is deliberately not tied to a live Subject FK.
        tx.execute_batch(
            "INSERT INTO journal_erased_sessions VALUES('erased-session','erased-subject','erasure','erased-at');
             INSERT INTO subject_deletions VALUES('erased-subject','delete-request',1,'started-at');
             INSERT INTO subject_deleted_workers VALUES('erased-subject','old-runtime','old-worker');
             INSERT INTO subject_deleted_sessions VALUES('erased-subject','erased-session');"
        )?;
        install_published_fixture(tx, format);
        mark_receipts(tx);
        Ok(())
    }).unwrap();
    drop(store);
    drop(database);
    scope.shutdown().unwrap();
}

fn database_path(root: &Path) -> std::path::PathBuf {
    root.join(SCOPE).join("features/subjektiv.sqlite")
}

#[test]
fn published_families_reopen_without_changing_shapes_rows_or_original_receipts() {
    for format in PUBLISHED {
        let temp = tempfile::tempdir().unwrap();
        published_fixture(temp.path(), format);
        let mut original = Connection::open(database_path(temp.path())).unwrap();
        assert_valid(&mut original);
        assert_eq!(recognize(&history(&original)).unwrap(), format);
        let saved_shape = domain_shape(&original).unwrap();
        let saved_rows = all_rows(&original);
        let saved_receipts = receipts(&original);
        drop(original);

        // This goes through the real registered migration engine, not a test-only planner.
        // There is intentionally no new storage/domain migration or convergence receipt.
        for _ in 0..2 {
            let (scope, database) = open(temp.path(), REGISTRATION);
            database
                .with_connection(|connection| {
                    assert_eq!(domain_shape(connection)?, saved_shape, "{format:?}");
                    assert_eq!(all_rows(connection), saved_rows, "{format:?}");
                    assert_eq!(receipts(connection), saved_receipts, "{format:?}");
                    assert_foreign_keys(connection);
                    Ok(())
                })
                .unwrap();
            drop(database);
            scope.shutdown().unwrap();
        }
    }
}

#[test]
fn common_prefixes_migrate_to_fresh_shape_and_keep_saved_receipts_and_oldest_data() {
    let fresh = tempfile::tempdir().unwrap();
    let (fresh_scope, fresh_db) = open(fresh.path(), REGISTRATION);
    let fresh_shape = fresh_db.with_connection(domain_shape).unwrap();
    drop(fresh_db);
    fresh_scope.shutdown().unwrap();

    for count in 0..=COMMON_COUNT {
        let temp = tempfile::tempdir().unwrap();
        let (old_scope, old_db) = open(
            temp.path(),
            FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, &MIGRATIONS[..count]),
        );
        let original_json = serde_json::json!({
            "schema_version": 1, "id": "oldest", "role": "preserved",
            "state": "active", "store_revision": 0,
            "created_at": "created", "updated_at": "updated"
        })
        .to_string();
        old_db.with_connection(|connection| {
            if count > 0 {
                connection.execute("INSERT INTO store_scope VALUES(1,?1)", [SCOPE])?;
            }
            if count == 1 {
                connection.execute(
                    "INSERT INTO subjects VALUES('oldest','preserved','active',0,?1,'created','updated')",
                    [&original_json],
                )?;
            }
            mark_receipts(connection);
            Ok(())
        }).unwrap();
        let saved = old_db.with_connection(|c| Ok(receipts(c))).unwrap();
        drop(old_db);
        old_scope.shutdown().unwrap();

        let (scope, db) = open(temp.path(), REGISTRATION);
        db.with_connection(|connection| {
            assert_eq!(domain_shape(connection)?, fresh_shape, "prefix {count}");
            assert_eq!(&receipts(connection)[..count], saved.as_slice());
            assert_foreign_keys(connection);
            if count > 0 {
                assert_eq!(
                    connection
                        .query_row("SELECT workspace_id FROM store_scope", [], |row| row
                            .get::<_, String>(0))?,
                    SCOPE
                );
            }
            if count == 1 {
                let archive: String = connection.query_row(
                    "SELECT row_json FROM legacy_record_archive WHERE source_table='subjects'",
                    [],
                    |row| row.get(0),
                )?;
                let archived: serde_json::Value = serde_json::from_str(&archive).unwrap();
                assert_eq!(archived["record_json"], original_json);
            }
            Ok(())
        })
        .unwrap();
        if count == 1 {
            let store = SubjektivStore::from_database(db.clone()).unwrap();
            let subject = store.subject("oldest").unwrap().unwrap();
            assert_eq!(subject.role.as_str(), "preserved");
        }
        drop(db);
        scope.shutdown().unwrap();
    }
}

#[test]
fn unknown_mixed_missing_receipts_and_shape_mismatches_refuse_without_mutation() {
    let cases = [
        (
            Format::Permission18,
            "UPDATE __yoi_feature_schema_migrations SET name='unknown' WHERE version=18",
        ),
        (
            Format::Permission18,
            "DELETE FROM __yoi_feature_schema_migrations WHERE version=18",
        ),
        (
            Format::Permission18,
            "DROP TRIGGER permission_sealed_subject_permissions_INSERT",
        ),
        (
            Format::Permission18,
            "ALTER TABLE permission_authority_seal ADD COLUMN unexpected TEXT",
        ),
        (
            Format::Permission18,
            "INSERT INTO __yoi_feature_schema_migrations(version,name) VALUES(20,'converge published Journal scope and consolidation write histories')",
        ),
        (
            Format::Consolidation18,
            "UPDATE __yoi_feature_schema_migrations SET name='fixed Journal removal scopes' WHERE version=18",
        ),
        (
            Format::Consolidation18,
            "INSERT INTO __yoi_feature_schema_migrations(version,name) VALUES(19,'retain exact Subject release scope cancellations')",
        ),
        (
            Format::Consolidation18,
            "INSERT INTO __yoi_feature_schema_migrations(version,name) VALUES(20,'converge published Journal scope and consolidation write histories')",
        ),
        (
            Format::Consolidation18,
            "DROP TRIGGER consolidation_write_binding_no_delete",
        ),
        (Format::Fixed18, "DROP TABLE journal_scope_release_receipts"),
        (
            Format::Cancellation19,
            "DELETE FROM __yoi_feature_schema_migrations WHERE version=19",
        ),
        (
            Format::Consolidation20,
            "DELETE FROM __yoi_feature_schema_migrations WHERE version=18",
        ),
        (
            Format::Fixed20,
            "UPDATE __yoi_feature_schema_migrations SET name='unknown provenance' WHERE version=20",
        ),
        (
            Format::Cancellation20,
            "DELETE FROM __yoi_feature_schema_migrations WHERE version=3",
        ),
        (
            Format::Cancellation20,
            "INSERT INTO __yoi_feature_schema_migrations(version,name) VALUES(21,'future')",
        ),
        (
            Format::Cancellation20,
            "CREATE TABLE unknown_payload(secret BLOB)",
        ),
    ];
    for (format, corrupt) in cases {
        let temp = tempfile::tempdir().unwrap();
        published_fixture(temp.path(), format);
        let mut raw = Connection::open(database_path(temp.path())).unwrap();
        raw.execute_batch(corrupt).unwrap();
        // Snapshot AFTER deliberate corruption. Rejection must not repair or adopt it.
        let saved_shape = domain_shape(&raw).unwrap();
        let saved_rows = all_rows(&raw);
        let tx = raw.transaction().unwrap();
        let error = validate(&tx, &history(&tx)).unwrap_err();
        assert!(
            matches!(error, FeatureStorageError::MigrationHistory { .. }),
            "{format:?}: {corrupt}: {error}"
        );
        drop(tx);
        drop(raw);

        let scope = FeatureStorage::new(temp.path()).scope(SCOPE).unwrap();
        let registered = scope.register(REGISTRATION).unwrap();
        let result = scope.open(&registered);
        assert!(
            matches!(result, Err(FeatureStorageError::MigrationHistory { .. })),
            "{format:?}: {corrupt}"
        );
        scope.shutdown().unwrap();
        let raw = Connection::open(database_path(temp.path())).unwrap();
        assert_eq!(domain_shape(&raw).unwrap(), saved_shape);
        assert_eq!(all_rows(&raw), saved_rows);
    }
}

#[test]
fn mixed_known_families_are_not_inferred_from_column_presence() {
    let temp = tempfile::tempdir().unwrap();
    published_fixture(temp.path(), Format::Permission18);
    let mut raw = Connection::open(database_path(temp.path())).unwrap();
    consolidation_fixture(&raw);
    let before = all_rows(&raw);
    let shape = domain_shape(&raw).unwrap();
    let tx = raw.transaction().unwrap();
    let error = validate(&tx, &history(&tx)).unwrap_err();
    assert!(error.to_string().contains("SQLite shape"));
    drop(tx);
    assert_eq!(all_rows(&raw), before);
    assert_eq!(domain_shape(&raw).unwrap(), shape);
    drop(raw);
    let scope = FeatureStorage::new(temp.path()).scope(SCOPE).unwrap();
    let registered = scope.register(REGISTRATION).unwrap();
    assert!(matches!(
        scope.open(&registered),
        Err(FeatureStorageError::MigrationHistory { .. })
    ));
    scope.shutdown().unwrap();
    let raw = Connection::open(database_path(temp.path())).unwrap();
    assert_eq!(all_rows(&raw), before);
    assert_eq!(domain_shape(&raw).unwrap(), shape);
}

#[test]
fn nonempty_schema_without_receipt_table_is_not_treated_as_new() {
    let temp = tempfile::tempdir().unwrap();
    published_fixture(temp.path(), Format::Permission18);
    let mut raw = Connection::open(database_path(temp.path())).unwrap();
    raw.execute_batch("DROP TABLE __yoi_feature_schema_migrations;")
        .unwrap();
    let before = all_rows(&raw);
    let shape = domain_shape(&raw).unwrap();
    let tx = raw.transaction().unwrap();
    assert!(matches!(
        validate(&tx, &[]),
        Err(FeatureStorageError::MigrationHistory { .. })
    ));
    drop(tx);
    drop(raw);
    let scope = FeatureStorage::new(temp.path()).scope(SCOPE).unwrap();
    let registered = scope.register(REGISTRATION).unwrap();
    assert!(matches!(
        scope.open(&registered),
        Err(FeatureStorageError::MigrationHistory { .. })
    ));
    scope.shutdown().unwrap();
    let raw = Connection::open(database_path(temp.path())).unwrap();
    // Even the engine's CREATE TABLE IF NOT EXISTS must roll back after rejection.
    assert_eq!(all_rows(&raw), before);
    assert_eq!(domain_shape(&raw).unwrap(), shape);
}

#[test]
fn common_receipt_write_failure_rolls_back_in_sqlite_then_engine_retry_succeeds() {
    let temp = tempfile::tempdir().unwrap();
    let (old_scope, old_db) = open(
        temp.path(),
        FeatureRegistration::new(SUBJEKTIV_FEATURE_ID, &MIGRATIONS[..16]),
    );
    old_db
        .with_connection(|connection| {
            connection.execute("INSERT INTO store_scope VALUES(1,?1)", [SCOPE])?;
            mark_receipts(connection);
            Ok(())
        })
        .unwrap();
    drop(old_db);
    old_scope.shutdown().unwrap();
    let raw = Connection::open(database_path(temp.path())).unwrap();
    let before_shape = domain_shape(&raw).unwrap();
    let before_rows = all_rows(&raw);
    let saved_receipts = receipts(&raw);
    drop(raw);

    // Direct SQLite transaction coverage, not engine fault injection: public opens cannot
    // install a TEMP trigger between validation and receipt insertion. Exercise the frozen
    // migration plus receipt-write failure here, then retry through the actual engine.
    let mut raw = Connection::open(database_path(temp.path())).unwrap();
    raw.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    raw.execute_batch(
        "CREATE TEMP TRIGGER fail_receipt BEFORE INSERT ON main.__yoi_feature_schema_migrations
         WHEN NEW.version=17 BEGIN SELECT RAISE(ABORT,'injected receipt failure'); END;",
    )
    .unwrap();
    {
        let tx = raw.transaction().unwrap();
        validate(&tx, &history(&tx)).unwrap();
        MIGRATIONS[16].apply(&tx).unwrap();
        assert!(
            tx.execute(
                &format!("INSERT INTO {HISTORY_TABLE}(version,name) VALUES(?1,?2)"),
                params![MIGRATIONS[16].version(), MIGRATIONS[16].name()],
            )
            .is_err()
        );
    }
    assert_eq!(domain_shape(&raw).unwrap(), before_shape);
    assert_eq!(all_rows(&raw), before_rows);
    drop(raw);
    let (scope, db) = open(temp.path(), REGISTRATION);
    db.with_connection(|connection| {
        assert_eq!(
            domain_shape(connection)?,
            reference_shape(Format::Common(17))?
        );
        assert_eq!(&receipts(connection)[..16], saved_receipts.as_slice());
        Ok(())
    })
    .unwrap();
    drop(db);
    scope.shutdown().unwrap();
}

#[test]
fn permission_transfer_extension_reopens_on_every_published_family_without_relabelling_receipts() {
    for format in PUBLISHED {
        let temp = tempfile::tempdir().unwrap();
        published_fixture(temp.path(), format);
        let (scope, db) = open(temp.path(), REGISTRATION);
        let saved = db.with_connection(|connection| Ok(receipts(connection))).unwrap();
        let old_seal = db.with_connection(|connection| {
            if format == Format::Permission18 {
                Ok(Some(connection.query_row("SELECT export_json FROM permission_authority_seal", [], |row| row.get::<_, String>(0))?))
            } else { Ok(None) }
        }).unwrap();
        db.transaction(install_permission_transfer).unwrap();
        db.with_connection(|connection| {
            assert_eq!(receipts(connection), saved);
            assert_eq!(domain_shape(connection)?, reference_shape_with_transfer(format, true)?);
            if let Some(raw) = &old_seal {
                assert_eq!(connection.query_row("SELECT export_json FROM permission_authority_seal", [], |row| row.get::<_, String>(0))?, *raw);
            }
            assert_foreign_keys(connection);
            Ok(())
        }).unwrap();
        let store = SubjektivStore::from_database(db.clone()).unwrap();
        if old_seal.is_some() {
            let identity = store.database_identity().unwrap();
            assert!(matches!(store.preview_permission_transfer(&identity), Err(crate::SubjektivError::PermissionAuthoritySealed)));
            assert!(matches!(store.seal_permission_sources(&identity, "new-transfer", "export"), Err(crate::SubjektivError::PermissionAuthoritySealed)));
            struct CanonicalEmpty;
            impl crate::CanonicalPermissionSource for CanonicalEmpty {
                fn subject_permissions(&self, workspace: &str, _subject: &str) -> crate::Result<Option<server_api::authorization::SubjectPermissions>> {
                    assert_eq!(workspace, SCOPE);
                    Ok(Some(server_api::authorization::SubjectPermissions {
                        change_id: "canonical-source".into(), execution: vec![], delegation: vec![],
                    }))
                }
            }
            let connected = store.clone().with_canonical_permission_source(std::sync::Arc::new(CanonicalEmpty));
            let subject = connected.list_subjects(1).unwrap().items[0].id.clone();
            assert_eq!(connected.subject_permissions(&subject).unwrap().unwrap().change_id, "canonical-source");
            connected.bind_worker_execution_grant(&server_api::authorization::WorkerExecutionGrant {
                subject_id: subject, source_change_id: "canonical-source".into(),
                runtime_id: "runtime".into(), worker_id: "canonical-child".into(),
                origin_runtime_id: "runtime".into(), origin_worker_id: "root".into(),
                profile: "builtin:coder".into(), profile_digest: "resolved-profile".into(),
                permissions: vec![], redelegation: vec![],
            }).unwrap();
        }
        drop(store);
        drop(db);
        scope.shutdown().unwrap();
        let (scope, db) = open(temp.path(), REGISTRATION);
        db.with_connection(|connection| {
            assert_eq!(receipts(connection), saved);
            assert_eq!(domain_shape(connection)?, reference_shape_with_transfer(format, true)?);
            assert_foreign_keys(connection);
            Ok(())
        }).unwrap();
        drop(db);
        scope.shutdown().unwrap();
    }
}

#[test]
fn permission_extension_ddl_rolls_back_as_a_unit_and_missing_fence_rejects_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let (scope, db) = open(temp.path(), REGISTRATION);
    let before_shape = db.with_connection(domain_shape).unwrap();
    let before_rows = db.with_connection(|connection| Ok(all_rows(connection))).unwrap();
    let result = db.transaction(|tx| {
        install_permission_transfer(tx)?;
        tx.execute_batch("INSERT INTO injected_missing_table VALUES(1)")?;
        Ok(())
    });
    assert!(result.is_err());
    db.with_connection(|connection| {
        assert_eq!(domain_shape(connection)?, before_shape);
        assert_eq!(all_rows(connection), before_rows);
        Ok(())
    }).unwrap();
    db.transaction(install_permission_transfer).unwrap();
    db.with_connection(|connection| {
        connection.execute_batch("DROP TRIGGER permission_transfer_fence_subject_permissions_INSERT")?;
        Ok(())
    }).unwrap();
    drop(db);
    scope.shutdown().unwrap();
    let scope = feature_storage::FeatureStorage::new(temp.path()).scope(SCOPE).unwrap();
    let registered = scope.register(REGISTRATION).unwrap();
    assert!(matches!(scope.open(&registered), Err(FeatureStorageError::MigrationHistory { .. })));
    scope.shutdown().unwrap();
}
