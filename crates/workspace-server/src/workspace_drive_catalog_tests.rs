//! Integration of the production catalog owner, WorkspaceDriveHost, and cached
//! Drive transactions. Only the request callback is reduced to a real persisted
//! grant query; this does not certify HTTP authentication or Worker liveness.
//! The minimal path-only fixtures in workspace_drive_host are deliberately not
//! used here. All storage and lock probes are confined to each temporary root.

use super::*;
use crate::SqliteWorkspaceStore;
use rusqlite::Connection;
use std::fs::{File, OpenOptions, TryLockError};
use std::sync::{Arc, atomic::AtomicUsize};
use workspace_drive::{Mutation, RequestStatus};

struct CatalogDrive {
    catalog: SqliteWorkspaceStore,
    path: PathBuf,
    lease_path: PathBuf,
    drive: Drive,
    _temp: tempfile::TempDir,
}

impl CatalogDrive {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("server.db");
        // SqliteWorkspaceStore::open, not from_connection/connect_prepared_fixture:
        // the expected identity and _host_catalog lease are production owners.
        let catalog = SqliteWorkspaceStore::open(&path).unwrap();
        catalog.with_conn(|conn| {
            conn.execute_batch("INSERT INTO accounts(account_id,kind,handle,display_name,created_at,updated_at)
                VALUES('drive-owner','user','drive-owner','Owner','1','1');
                INSERT INTO workspaces(workspace_id,owner_account_id,display_name,state,created_at,updated_at)
                VALUES('drive-ws','drive-owner','Workspace','active','1','1');
                INSERT INTO worker_registry(workspace_id,runtime_id,worker_id,display_name,retention_state,created_at,updated_at)
                VALUES('drive-ws','runtime','worker','Worker','normal','1','1');
                INSERT INTO workspace_drive_grants(workspace_id,runtime_id,worker_id,access,revoked,created_by,created_at)
                VALUES('drive-ws','runtime','worker','read_write',0,'drive-owner','1');")?;
            Ok(())
        }).unwrap();
        let storage = crate::FeatureStorage::for_server_database(&path)
            .unwrap()
            .workspace("drive-ws")
            .unwrap();
        let root = drive_blob_root(temp.path(), "drive-ws").unwrap();
        let host = WorkspaceDriveHost::new(&storage).await.unwrap();
        let authority = catalog.drive_authority(&path).unwrap();
        assert!(matches!(&authority, DriveAuthority::Managed(_)));
        let drive = host
            .open(&storage, root.clone(), authority.clone())
            .await
            .unwrap();
        // Return the cached handle, not merely the initially opened connection.
        drop(drive);
        let drive = host.open(&storage, root, authority).await.unwrap();
        let lease_path = temp
            .path()
            .join("feature-storage/.lifecycle/_host_catalog.lock");
        Self {
            _temp: temp,
            catalog,
            path,
            lease_path,
            drive,
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
                return Err(workspace_drive::Error::Denied);
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

#[tokio::test]
async fn cached_drive_valid_catalog_allows_grants_and_rechecks_revocation() {
    let fixture = CatalogDrive::new().await;
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
    fixture.catalog.with_conn(|conn| {
        conn.execute("UPDATE workspace_drive_grants SET revoked=1, revoked_by='drive-owner', revoked_at='2'", [])?;
        Ok(())
    }).unwrap();
    assert!(matches!(drive.root(), Err(workspace_drive::Error::Denied)));
    assert_eq!(callbacks.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn cached_drive_missing_catalog_refuses_before_stale_grant_callback() {
    assert_cached_catalog_refuses_stale_grants(false).await;
}

#[tokio::test]
async fn cached_drive_same_path_copied_catalog_refuses_before_stale_grant_callback() {
    assert_cached_catalog_refuses_stale_grants(true).await;
}

async fn assert_cached_catalog_refuses_stale_grants(replacement: bool) {
    let fixture = CatalogDrive::new().await;
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
    if replacement {
        fixture
            .catalog
            .with_conn(|conn| {
                let mut target = Connection::open(&copy)?;
                rusqlite::backup::Backup::new(conn, &mut target)?.run_to_completion(
                    128,
                    std::time::Duration::from_millis(1),
                    None,
                )?;
                // Copy retains the UUID and grants: path/identity-row equality
                // alone cannot distinguish this from the expected physical DB.
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
    if replacement {
        std::fs::rename(&copy, &fixture.path).unwrap();
    }
    // Contrast the catalog repository's managed rejection with its cached
    // consumer. No fresh WorkspaceApi lookup/owner open occurs here.
    let entered = std::cell::Cell::new(false);
    assert!(
        fixture
            .catalog
            .with_conn(|_| {
                entered.set(true);
                Ok(())
            })
            .is_err()
    );
    assert!(!entered.get());
    assert!(matches!(
        drive.root(),
        Err(workspace_drive::Error::Storage(_))
    ));
    assert!(matches!(
        drive.mutate(
            "after",
            "owner",
            Mutation::CreateFolder {
                parent,
                name: "forbidden".into(),
            }
        ),
        Err(workspace_drive::Error::Storage(_))
    ));
    assert!(matches!(
        drive.request_status("before"),
        Err(workspace_drive::Error::Storage(_))
    ));
    assert_eq!(
        callbacks.load(Ordering::SeqCst),
        before,
        "retired grants must not be consulted"
    );
    assert_eq!(
        fixture.path.exists(),
        replacement,
        "a missing catalog must not be recreated"
    );
}

#[tokio::test]
async fn cached_drive_catalog_lease_is_held_in_commit_and_rollback_but_not_while_idle() {
    for rollback in [false, true] {
        let fixture = CatalogDrive::new().await;
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
            // Only the lease, not the catalog connection mutex, is retained.
            // This also exercises the documented order against a catalog read.
            catalog
                .with_conn(|catalog_conn| {
                    let count: i64 = catalog_conn.query_row(
                        "SELECT count(*) FROM workspace_drive_grants",
                        [],
                        |row| row.get(0),
                    )?;
                    assert_eq!(count, 1);
                    Ok(())
                })
                .unwrap();
            // Observe actual SQLite transaction exit, not only the callback or
            // a SQL effect preceding COMMIT. Hooks are test-only dependency features.
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
                // Make a real metadata change first, then deny it. Transaction
                // drop must roll it back while still retaining the catalog lease.
                conn.execute("UPDATE drive_state SET deleting=1 WHERE singleton=1", [])?;
                return Err(workspace_drive::Error::Denied);
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
            assert!(matches!(result, Err(workspace_drive::Error::Denied)));
        } else {
            result.unwrap();
        }
        assert!(after_authorizer.load(Ordering::SeqCst));
        // The transaction has committed or rolled back, not just returned from
        // its authorizer. No lease is retained across idle cached-handle time.
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
