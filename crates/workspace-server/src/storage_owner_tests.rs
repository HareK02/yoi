//! Real Server catalog -> FeatureStorage -> Subjektiv connection tests. The
//! fixture creates saved domain data through public APIs, then removes only the
//! new physical identity metadata from a SQLite backup to represent a pre-owner
//! database. No production paths, Runtime, or authorization provider is borrowed.
use super::*;
use crate::subjektiv::{SubjectRole, SubjektivStore};
use feature_storage::FeatureStorage;

fn catalog(path: &Path) -> SqliteWorkspaceStore {
    let store = SqliteWorkspaceStore::open_for_test(path).unwrap();
    store.with_conn(|conn| {
        conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
            VALUES('storage-owner','user','storage-owner','Owner','1','1');
            INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
            VALUES('storage-workspace','storage-owner','Saved','active','1','1');")?;
        Ok(())
    }).unwrap();
    store
}

fn legacy_subject(source: &Path, destination: &Path, attribution: &str) -> String {
    let storage = FeatureStorage::new(source).scope(attribution).unwrap();
    let registration = SubjektivStore::register(&storage).unwrap();
    let database = storage.open(&registration).unwrap();
    let subject_store = SubjektivStore::from_database(database.clone()).unwrap();
    let subject = subject_store
        .create_subject_with_behavior(
            SubjectRole::new("retained").unwrap(),
            "original behavior".into(),
        )
        .unwrap();
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    database
        .with_connection(|conn| {
            let mut copy = Connection::open(destination)?;
            Backup::new(conn, &mut copy)?.run_to_completion(128, Duration::from_millis(1), None)?;
            copy.execute_batch("DROP TABLE __yoi_storage_identity;")?;
            Ok(())
        })
        .unwrap();
    subject.id
}

#[test]
fn catalog_authorized_legacy_adoption_reopens_and_preserves_domain_data() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("server.db");
    let catalog = catalog(&path);
    let owner = FeatureStorage::for_server_database(&path).unwrap();
    let scope = owner.scope("storage-workspace").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    let feature_path = temp
        .path()
        .join("feature-storage/storage-workspace/features/subjektiv.sqlite");
    let subject = legacy_subject(
        &temp.path().join("source"),
        &feature_path,
        "storage-workspace",
    );
    assert!(
        scope.open(&registration).is_err(),
        "path existence is not adoption authority"
    );
    catalog
        .adopt_workspace_feature_storage(&scope, &registration, "store_scope")
        .unwrap();
    let store = SubjektivStore::open(&scope, &registration).unwrap();
    assert_eq!(store.subject(&subject).unwrap().unwrap().id, subject);
    drop(store);
    scope.shutdown().unwrap();
    let owner = FeatureStorage::for_server_database(&path).unwrap();
    let scope = owner.scope("storage-workspace").unwrap();
    let registration = SubjektivStore::register(&scope).unwrap();
    catalog
        .adopt_workspace_feature_storage(&scope, &registration, "store_scope")
        .unwrap();
    assert_eq!(
        SubjektivStore::open(&scope, &registration)
            .unwrap()
            .subject(&subject)
            .unwrap()
            .unwrap()
            .id,
        subject
    );
}

#[test]
fn missing_catalog_or_different_feature_attribution_never_grants_adoption() {
    for attribution in ["storage-workspace", "another-workspace"] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let catalog = catalog(&path);
        let scope = FeatureStorage::for_server_database(&path)
            .unwrap()
            .scope("storage-workspace")
            .unwrap();
        let registration = SubjektivStore::register(&scope).unwrap();
        let feature_path = temp
            .path()
            .join("feature-storage/storage-workspace/features/subjektiv.sqlite");
        legacy_subject(&temp.path().join("source"), &feature_path, attribution);
        if attribution == "storage-workspace" {
            catalog
                .with_conn(|conn| {
                    conn.execute("DELETE FROM workspaces", [])?;
                    Ok(())
                })
                .unwrap();
        }
        assert!(
            catalog
                .adopt_workspace_feature_storage(&scope, &registration, "store_scope")
                .is_err()
        );
        let raw = Connection::open(&feature_path).unwrap();
        let count: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='__yoi_storage_identity'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "refusal must not mint identity");
    }
}

#[test]
fn catalog_cached_connection_rejects_replaced_file_and_missing_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("server.db");
    let store = catalog(&path);
    let replacement = temp.path().join("replacement.db");
    store
        .with_conn(|conn| {
            let mut copy = Connection::open(&replacement)?;
            Backup::new(conn, &mut copy)?.run_to_completion(128, Duration::from_millis(1), None)?;
            Ok(())
        })
        .unwrap();
    std::fs::rename(&path, temp.path().join("old.db")).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    assert!(store.with_conn(|_| Ok(())).is_err());
    assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    assert!(SqliteWorkspaceStore::open_for_test(&path).is_err());
    assert!(
        !path.exists(),
        "bound catalog must not silently CREATE a successor"
    );
}
