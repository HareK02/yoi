//! Storage-owner/Drive integration through production `open_host_catalog` and
//! `open_with_managed_workspace_authority` on temporary SQLite files. Catalog
//! domain preparation and the authorizer are reduced to persisted grant tables
//! and queries. This does not execute Server factories, schema migrations, HTTP
//! authentication, or Worker liveness checks. Requests reuse the cached Drive
//! connection without reopening the catalog to hide stale state.
//! Run with `cargo test -p workspace-drive --test catalog_authority`.

use feature_storage::{FeatureDatabase, FeatureStorage};
use rusqlite::Connection;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use workspace_drive::{Drive, Error, Mutation, RequestStatus};

struct CatalogDrive {
    catalog: FeatureDatabase,
    path: PathBuf,
    lease_path: PathBuf,
    drive: Drive,
    _temp: tempfile::TempDir,
}

impl CatalogDrive {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        let owner = FeatureStorage::for_server_database(&path).unwrap();
        let catalog = owner.open_host_catalog::<Error>(
            &path, "catalog-test", "Host configured isolated test catalog", |conn| {
                conn.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY, state TEXT NOT NULL);
                    INSERT INTO workspaces VALUES('drive-ws', 'active');
                    CREATE TABLE workspace_drive_grants(workspace_id TEXT, runtime_id TEXT, worker_id TEXT, access TEXT, revoked INTEGER);
                    INSERT INTO workspace_drive_grants VALUES('drive-ws','runtime','worker','read_write',0);")?;
                Ok(())
            }
        ).unwrap();
        let storage = owner.workspace("drive-ws").unwrap();
        let registration = Drive::register(&storage).unwrap();
        let root = temp.path().join("blobs");
        let drive =
            Drive::open_with_managed_workspace_authority(&storage, &registration, &root, &catalog)
                .unwrap();
        let drive = drive.clone();
        let lease_path = temp
            .path()
            .join("feature-storage/.lifecycle/_host_catalog.lock");
        Self {
            catalog,
            path,
            lease_path,
            drive,
            _temp: temp,
        }
    }

    fn grant_bound(&self, callbacks: Arc<AtomicUsize>) -> Drive {
        self.drive.with_authorizer(move |conn, write| {
            callbacks.fetch_add(1, Ordering::SeqCst);
            let granted: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM yoi_workspace_authority.workspace_drive_grants
                 WHERE workspace_id='drive-ws' AND runtime_id='runtime' AND worker_id='worker'
                 AND revoked=0 AND (?1=0 OR access='read_write'))",
                [write],
                |row| row.get(0),
            )?;
            if !granted {
                return Err(Error::Denied);
            }
            Ok(())
        })
    }
}

fn catalog_lock(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

#[test]
fn cached_drive_valid_catalog_allows_grants_and_rechecks_revocation() {
    let fixture = CatalogDrive::new();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let drive = fixture.grant_bound(callbacks.clone());
    let parent = drive.root().unwrap().id;
    let committed = drive
        .mutate(
            "valid",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "allowed".into(),
            },
        )
        .unwrap();
    assert_eq!(drive.metadata(committed.node.id).unwrap(), committed.node);
    assert!(matches!(
        drive.request_status("valid").unwrap(),
        RequestStatus::Committed { .. }
    ));
    fixture
        .catalog
        .try_with_connection::<_, Error>(|conn| {
            conn.execute("UPDATE workspace_drive_grants SET revoked=1", [])?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(drive.root(), Err(Error::Denied)));
    assert_eq!(callbacks.load(Ordering::SeqCst), 5);
}

#[test]
fn cached_drive_missing_catalog_refuses_before_stale_grant_callback() {
    assert_cached_catalog_refuses_stale_grants(Replacement::Missing);
}

#[test]
fn cached_drive_different_database_at_same_path_refuses_before_stale_grant_callback() {
    assert_cached_catalog_refuses_stale_grants(Replacement::OtherDatabase);
}

#[test]
fn cached_drive_same_path_copied_catalog_refuses_before_stale_grant_callback() {
    assert_cached_catalog_refuses_stale_grants(Replacement::CopiedIdentity);
}

enum Replacement {
    Missing,
    OtherDatabase,
    CopiedIdentity,
}

fn assert_cached_catalog_refuses_stale_grants(replacement: Replacement) {
    let fixture = CatalogDrive::new();
    let callbacks = Arc::new(AtomicUsize::new(0));
    let drive = fixture.grant_bound(callbacks.clone());
    let parent = drive.root().unwrap().id;
    drive
        .mutate(
            "before",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "before".into(),
            },
        )
        .unwrap();
    let before = callbacks.load(Ordering::SeqCst);
    let copy = fixture.path.with_extension("copy");
    if matches!(replacement, Replacement::CopiedIdentity) {
        fixture
            .catalog
            .try_with_connection::<_, Error>(|conn| {
                let mut target = Connection::open(&copy)?;
                rusqlite::backup::Backup::new(conn, &mut target)?.run_to_completion(
                    128,
                    std::time::Duration::from_millis(1),
                    None,
                )?;
                // Preserve UUID and grants to expose physical replacement even
                // when a path/identity-row-only check would accept the copy.
                let original: String = conn.query_row(
                    "SELECT physical FROM __yoi_storage_identity WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )?;
                let copied: String = target.query_row(
                    "SELECT physical FROM __yoi_storage_identity WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(original, copied);
                let grants: i64 = target.query_row(
                    "SELECT count(*) FROM workspace_drive_grants WHERE revoked=0",
                    [],
                    |row| row.get(0),
                )?;
                assert_eq!(grants, 1);
                Ok(())
            })
            .unwrap();
    }
    std::fs::rename(&fixture.path, fixture.path.with_extension("retired")).unwrap();
    match replacement {
        Replacement::Missing => {}
        Replacement::CopiedIdentity => std::fs::rename(&copy, &fixture.path).unwrap(),
        Replacement::OtherDatabase => {
            let other = Connection::open(&fixture.path).unwrap();
            other
                .execute_batch(
                    "CREATE TABLE unrelated(value TEXT); INSERT INTO unrelated VALUES('retain');",
                )
                .unwrap();
        }
    }
    // Contrast the owner's managed rejection with the cached consumer.
    // No fresh catalog open occurs before these Drive requests.
    let entered = std::cell::Cell::new(false);
    assert!(
        fixture
            .catalog
            .try_with_connection::<_, Error>(|_| {
                entered.set(true);
                Ok(())
            })
            .is_err()
    );
    assert!(!entered.get());
    assert!(matches!(drive.root(), Err(Error::Storage(_))));
    assert!(matches!(
        drive.mutate(
            "after",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "forbidden".into(),
            }
        ),
        Err(Error::Storage(_))
    ));
    assert!(matches!(
        drive.request_status("before"),
        Err(Error::Storage(_))
    ));
    assert_eq!(
        callbacks.load(Ordering::SeqCst),
        before,
        "retired grants must not be consulted"
    );
    assert_eq!(
        fixture.path.exists(),
        !matches!(replacement, Replacement::Missing),
        "a missing catalog must not be recreated"
    );
    if matches!(replacement, Replacement::OtherDatabase) {
        let other = Connection::open(&fixture.path).unwrap();
        assert_eq!(
            other
                .query_row("SELECT value FROM unrelated", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "retain"
        );
        let identity_added: bool = other
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name='__yoi_storage_identity')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!identity_added, "rejected replacement must not be adopted");
    }
}

#[test]
fn cached_drive_catalog_lease_is_held_in_commit_and_rollback_but_not_while_idle() {
    for rollback in [false, true] {
        let fixture = CatalogDrive::new();
        let parent = fixture.drive.root().unwrap().id;
        let idle_lock = catalog_lock(&fixture.lease_path);
        idle_lock.try_lock().unwrap();
        idle_lock.unlock().unwrap();
        let after_authorizer = Arc::new(AtomicBool::new(false));
        let probe = after_authorizer.clone();
        let lease_path = fixture.lease_path.clone();
        let catalog = fixture.catalog.clone();
        let drive = fixture.drive.with_authorizer(move |conn, _| {
            let lock = catalog_lock(&lease_path);
            assert!(
                matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
                "cached transaction must hold catalog lease"
            );
            // Only the lifecycle lease is retained, not the catalog mutex.
            catalog
                .try_with_connection::<_, Error>(|catalog_conn| {
                    let count: i64 = catalog_conn.query_row(
                        "SELECT count(*) FROM workspace_drive_grants",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(count, 1);
                    Ok(())
                })
                .unwrap();
            // Test-only hooks observe SQLite transaction exit rather than a
            // callback/SQL effect that merely precedes COMMIT or ROLLBACK.
            let lease_path = lease_path.clone();
            let probe = probe.clone();
            if rollback {
                conn.rollback_hook(Some(move || {
                    let lock = catalog_lock(&lease_path);
                    assert!(
                        matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
                        "lease must span SQLite rollback"
                    );
                    probe.store(true, Ordering::SeqCst);
                }));
                conn.execute("UPDATE drive_state SET deleting=1 WHERE singleton=1", [])?;
                return Err(Error::Denied);
            }
            conn.commit_hook(Some(move || {
                let lock = catalog_lock(&lease_path);
                assert!(
                    matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
                    "lease must span SQLite commit"
                );
                probe.store(true, Ordering::SeqCst);
                false
            }));
            Ok(())
        });
        let result = drive.mutate(
            "lease-effect",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "transaction-effect".into(),
            },
        );
        if rollback {
            assert!(matches!(result, Err(Error::Denied)));
        } else {
            result.unwrap();
        }
        assert!(after_authorizer.load(Ordering::SeqCst));
        idle_lock.try_lock().unwrap();
        idle_lock.unlock().unwrap();
        fixture.drive.check_authorized(true).unwrap();
        assert_eq!(
            fixture.drive.list(parent, None, 10).unwrap().nodes.len(),
            usize::from(!rollback)
        );
        assert!(matches!(
            (
                fixture.drive.request_status("lease-effect").unwrap(),
                rollback
            ),
            (RequestStatus::Uncommitted, true) | (RequestStatus::Committed { .. }, false)
        ));
    }
}
