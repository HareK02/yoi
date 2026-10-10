use super::*;

fn fixture(t: &tempfile::TempDir) -> Drive {
    let s = feature_storage::FeatureStorage::new(t.path().join("metadata"))
        .workspace("ws")
        .unwrap();
    let r = Drive::register(&s).unwrap();
    Drive::open(&s, &r, &t.path().join("blobs")).unwrap()
}
fn create(parent: NodeId, name: &str) -> Mutation {
    Mutation::CreateFile {
        parent,
        name: name.into(),
        content_type: "text/plain".into(),
        bytes: b"persisted".to_vec(),
    }
}

#[test]
fn migration_enforces_workspace_parent_and_unique_names() {
    let t = tempfile::tempdir().unwrap();
    let d = fixture(&t);
    assert_eq!(d.database.schema_version().unwrap(), 2);
    let root = d.root().unwrap();
    d.mutate("a", "actor", create(root.id, "a")).unwrap();
    assert!(matches!(
        d.mutate("duplicate", "actor", create(root.id, "a")),
        Err(Error::Conflict)
    ));
    let result=d.database.try_transaction::<_,Error>(|tx| {
        tx.execute("INSERT INTO drive_nodes(workspace_id,parent_id,name,kind,last_mutation_id,size,updated_by,updated_at_ms)
            VALUES ('other',?1,'bad','directory',1,0,'actor',0)",[root.id.0])?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(d.list(root.id, None, 10).unwrap().nodes.len(), 1);
}

#[test]
fn saved_blob_before_metadata_failure_is_not_published_and_restart_collects_it() {
    let t = tempfile::tempdir().unwrap();
    let d = fixture(&t);
    let root = d.root().unwrap();
    d.database.transaction(|tx| {tx.execute_batch("CREATE TRIGGER fail_node BEFORE INSERT ON drive_nodes WHEN NEW.parent_id IS NOT NULL
        BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END;")?;Ok(())}).unwrap();
    assert!(
        d.mutate("failed", "actor", create(root.id, "failed"))
            .is_err()
    );
    assert_eq!(d.blobs.list(None, 20).unwrap().len(), 1);
    assert!(d.list(root.id, None, 10).unwrap().nodes.is_empty());
    assert_eq!(
        d.request_status("failed").unwrap(),
        RequestStatus::Uncommitted
    );
    d.database
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER fail_node")?;
            Ok(())
        })
        .unwrap();
    drop(d);
    let d = fixture(&t);
    assert_eq!(d.collect(None, 20).unwrap().removed, 1);
    let result = d
        .mutate("failed", "actor", create(root.id, "failed"))
        .unwrap();
    assert_eq!(
        d.read(result.node.id, &result.node.last_mutation_id, 0, 100)
            .unwrap()
            .bytes,
        b"persisted"
    );
}

#[test]
fn commit_failure_rolls_back_metadata_and_receipt_together() {
    let t = tempfile::tempdir().unwrap();
    let d = fixture(&t);
    let root = d.root().unwrap();
    d.database.transaction(|tx| {tx.execute_batch("CREATE TABLE deferred_commit_failure (node INTEGER REFERENCES drive_nodes(id) DEFERRABLE INITIALLY DEFERRED);
        CREATE TRIGGER fail_commit AFTER INSERT ON drive_receipts BEGIN INSERT INTO deferred_commit_failure VALUES (-1); END;")?;Ok(())}).unwrap();
    assert!(
        d.mutate("failed", "actor", create(root.id, "failed"))
            .is_err()
    );
    assert!(d.list(root.id, None, 10).unwrap().nodes.is_empty());
    assert_eq!(
        d.request_status("failed").unwrap(),
        RequestStatus::Uncommitted
    );
    d.database
        .transaction(|tx| {
            tx.execute_batch("DROP TRIGGER fail_commit; DROP TABLE deferred_commit_failure")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(d.collect(None, 10).unwrap().removed, 1);
    assert!(
        !d.mutate("failed", "actor", create(root.id, "failed"))
            .unwrap()
            .deleted
    );
}

#[test]
fn collection_cannot_remove_in_flight_upload_or_selected_read() {
    // Independent managers/SQLite connections simulate multiple server processes,
    // not clones sharing a process-local lock. All calls use the same DB authority.
    let t = tempfile::tempdir().unwrap();
    let d = fixture(&t);
    let other = fixture(&t);
    let root = d.root().unwrap();
    let first = d
        .mutate("first", "actor", create(root.id, "file"))
        .unwrap()
        .node;
    let go = Arc::new(std::sync::Barrier::new(2));
    let g = go.clone();
    let selected = first.clone();
    let reader = std::thread::spawn(move || {
        g.wait();
        for _ in 0..20 {
            match other.read(selected.id, &selected.last_mutation_id, 0, 100) {
                Ok(chunk) => assert_eq!(chunk.bytes, b"persisted"),
                Err(Error::Conflict) => {}
                e => panic!("unexpected selected read outcome: {e:?}"),
            }
            other.collect(None, 200).unwrap();
        }
    });
    go.wait();
    let mut current = first;
    for i in 0..20 {
        current = d
            .mutate(
                &format!("update-{i}"),
                "actor",
                Mutation::Update {
                    id: current.id,
                    expected_mutation_id: current.last_mutation_id.clone(),
                    content_type: "text/plain".into(),
                    bytes: vec![i; 1024],
                },
            )
            .unwrap()
            .node;
    }
    reader.join().unwrap();
    d.collect(None, 200).unwrap();
    assert_eq!(d.blobs.list(None, 200).unwrap().len(), 1);
    assert_eq!(
        d.read(current.id, &current.last_mutation_id, 0, 1024)
            .unwrap()
            .bytes,
        vec![19; 1024]
    );
}

#[test]
fn attached_server_authority_serializes_deletion_with_cached_drive_mutations() {
    let t = tempfile::tempdir().unwrap();
    let authority = t.path().join("server.db");
    let mut server = Connection::open(&authority).unwrap();
    feature_storage::configure_connection(&server).unwrap();
    server.execute_batch("CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY,state TEXT NOT NULL); INSERT INTO workspaces VALUES ('ws','active');").unwrap();
    let s = feature_storage::FeatureStorage::new(t.path().join("metadata"))
        .workspace("ws")
        .unwrap();
    let reg = Drive::register(&s).unwrap();
    let d = Drive::open_with_workspace_authority(&s, &reg, &t.path().join("blobs"), &authority)
        .unwrap();
    let root = d.root().unwrap();
    let child = d
        .mutate("before", "actor", create(root.id, "before"))
        .unwrap()
        .node;
    // An IMMEDIATE Drive transaction locks both databases. Reserve deletion
    // cannot commit while a previously admitted mutation is publishing a blob.
    server.busy_timeout(std::time::Duration::ZERO).unwrap();
    d.database.transaction(|_| {
        assert!(matches!(server.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate),Err(rusqlite::Error::SqliteFailure(e,_)) if e.code==rusqlite::ErrorCode::DatabaseBusy));
        Ok(())
    }).unwrap();
    server
        .execute(
            "UPDATE workspaces SET state='deleting' WHERE workspace_id='ws'",
            [],
        )
        .unwrap();
    // No separate Drive::fence call needed, including for already issued handles.
    assert!(matches!(
        d.mutate("after", "actor", create(root.id, "after")),
        Err(Error::Fenced)
    ));
    assert!(matches!(
        d.read(child.id, &child.last_mutation_id, 0, 100),
        Err(Error::Fenced)
    ));
    assert!(matches!(d.collect(None, 10), Err(Error::Fenced)));
    d.purge().unwrap();
}

#[test]
fn restarted_collection_removes_crashed_staging_without_removing_referenced_hardlink() {
    let t = tempfile::tempdir().unwrap();
    let d = fixture(&t);
    let root = d.root().unwrap();
    let n = d
        .mutate("published", "actor", create(root.id, "file"))
        .unwrap()
        .node;
    let key: String = d
        .database
        .with_connection(|c| {
            Ok(c.query_row(
                "SELECT blob_key FROM drive_nodes WHERE id=?1",
                [n.id.0],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    let blobs = t.path().join("blobs");
    std::fs::hard_link(blobs.join(&key), blobs.join(format!("{key}#1"))).unwrap();
    std::fs::write(
        blobs.join(format!("{}#1", Uuid::now_v7())),
        b"partial upload",
    )
    .unwrap();
    drop(d);
    let d = fixture(&t);
    assert_eq!(d.collect(None, 200).unwrap().removed, 2);
    assert_eq!(
        d.read(n.id, &n.last_mutation_id, 0, 100).unwrap().bytes,
        b"persisted"
    );
    assert_eq!(std::fs::read_dir(blobs).unwrap().count(), 1);
}

#[test]
fn interrupted_full_permission_and_sync_failed_uploads_do_not_publish_metadata_or_receipt() {
    for fault in [
        blob::TestFault::PartialStagingWrite,
        blob::TestFault::UploadNoSpace,
        blob::TestFault::UploadPermissionDenied,
        blob::TestFault::FileSync,
        blob::TestFault::DirectorySync,
    ] {
        let t = tempfile::tempdir().unwrap();
        let mut d = fixture(&t);
        let root = d.root().unwrap();
        let original = d
            .mutate("original", "actor", create(root.id, "file"))
            .unwrap()
            .node;
        d.blobs = Arc::new(
            blob::BlobStore::open(&t.path().join("blobs"))
                .unwrap()
                .with_fault(fault),
        );
        let mutation = Mutation::Update {
            id: original.id,
            expected_mutation_id: original.last_mutation_id.clone(),
            content_type: "text/plain".into(),
            bytes: b"failed replacement".to_vec(),
        };
        assert!(
            matches!(
                d.mutate("failed", "actor", mutation.clone()),
                Err(Error::Blob(_))
            ),
            "fault {fault:?}"
        );
        assert_eq!(
            d.metadata(original.id).unwrap(),
            original,
            "fault {fault:?}"
        );
        assert_eq!(
            d.request_status("failed").unwrap(),
            RequestStatus::Uncommitted,
            "fault {fault:?}"
        );
        d.blobs = Arc::new(blob::BlobStore::open(&t.path().join("blobs")).unwrap());
        d.collect(None, 200).unwrap();
        assert_eq!(
            d.read(original.id, &original.last_mutation_id, 0, 100)
                .unwrap()
                .bytes,
            b"persisted"
        );
        assert_eq!(
            std::fs::read_dir(t.path().join("blobs")).unwrap().count(),
            1
        );
        let replaced = d.mutate("failed", "actor", mutation).unwrap().node;
        assert_eq!(
            d.read(replaced.id, &replaced.last_mutation_id, 0, 100)
                .unwrap()
                .bytes,
            b"failed replacement"
        );
    }
}

fn grant_fixture(t: &tempfile::TempDir) -> (Drive, Connection) {
    let authority = t.path().join("server.db");
    let server = Connection::open(&authority).unwrap();
    feature_storage::configure_connection(&server).unwrap();
    server
        .execute_batch(
            "CREATE TABLE workspaces(workspace_id TEXT PRIMARY KEY,state TEXT NOT NULL);
        INSERT INTO workspaces VALUES ('ws','active');
        CREATE TABLE grants(write_access INTEGER NOT NULL,revoked INTEGER NOT NULL);
        INSERT INTO grants VALUES (1,0);",
        )
        .unwrap();
    let storage = feature_storage::FeatureStorage::new(t.path().join("metadata"))
        .workspace("ws")
        .unwrap();
    let registration = Drive::register(&storage).unwrap();
    (
        Drive::open_with_workspace_authority(
            &storage,
            &registration,
            &t.path().join("blobs"),
            &authority,
        )
        .unwrap(),
        server,
    )
}
fn grant_authorizer(conn: &Connection, write: bool) -> Result<()> {
    assert!(
        !conn.is_autocommit(),
        "authority must be checked in the operation transaction"
    );
    let allowed: bool = conn.query_row(
        "SELECT revoked=0 AND (?1=0 OR write_access=1) FROM yoi_workspace_authority.grants",
        [write],
        |r| r.get(0),
    )?;
    if allowed { Ok(()) } else { Err(Error::Denied) }
}

#[test]
fn bound_authorizer_checks_every_read_write_and_receipt_replay_without_caching() {
    let t = tempfile::tempdir().unwrap();
    let (raw, server) = grant_fixture(&t);
    let root = raw.root().unwrap();
    let bound = raw.with_authorizer(grant_authorizer);
    let mutation = create(root.id, "file");
    let result = bound.mutate("request", "worker", mutation.clone()).unwrap();
    bound.check_authorized(true).unwrap();
    server
        .execute("UPDATE grants SET write_access=0", [])
        .unwrap();
    assert!(bound.check_authorized(false).is_ok());
    assert!(matches!(bound.check_authorized(true), Err(Error::Denied)));
    assert!(bound.root().is_ok());
    assert!(bound.metadata(result.node.id).is_ok());
    assert!(bound.list(root.id, None, 10).is_ok());
    assert!(bound.search("file", true, None, 10).is_ok());
    assert!(
        bound
            .read(result.node.id, &result.node.last_mutation_id, 0, 100)
            .is_ok()
    );
    assert!(
        bound
            .read_text(result.node.id, &result.node.last_mutation_id, 0, 100)
            .is_ok()
    );
    assert!(bound.request_status("request").is_ok());
    assert!(
        matches!(
            bound.mutate("request", "worker", mutation.clone()),
            Err(Error::Denied)
        ),
        "read-only receipt replay is a write request"
    );
    let clone = bound.clone();
    server.execute("UPDATE grants SET revoked=1", []).unwrap();
    assert!(matches!(clone.check_authorized(false), Err(Error::Denied)));
    assert!(matches!(clone.check_authorized(true), Err(Error::Denied)));
    assert!(matches!(clone.root(), Err(Error::Denied)));
    assert!(matches!(clone.metadata(result.node.id), Err(Error::Denied)));
    assert!(matches!(clone.list(root.id, None, 10), Err(Error::Denied)));
    assert!(matches!(
        clone.search("file", true, None, 10),
        Err(Error::Denied)
    ));
    assert!(matches!(
        clone.read(result.node.id, &result.node.last_mutation_id, 0, 100),
        Err(Error::Denied)
    ));
    assert!(matches!(
        clone.read_text(result.node.id, &result.node.last_mutation_id, 0, 100),
        Err(Error::Denied)
    ));
    assert!(matches!(
        clone.request_status("request"),
        Err(Error::Denied)
    ));
    assert!(matches!(
        clone.mutate("request", "worker", mutation),
        Err(Error::Denied)
    ));
    assert!(matches!(
        clone.mutate("denied", "worker", create(root.id, "denied")),
        Err(Error::Denied)
    ));
    assert_eq!(
        raw.blobs.list(None, 10).unwrap().len(),
        1,
        "denied write must not publish a blob"
    );
    assert_eq!(
        raw.request_status("denied").unwrap(),
        RequestStatus::Uncommitted
    );
    assert_eq!(raw.list(root.id, None, 10).unwrap().nodes.len(), 1);
    raw.collect(None, 10).unwrap();
}

#[test]
fn revoke_serializes_with_publication_and_cached_handle_denies_after_revoke_commit() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let t = tempfile::tempdir().unwrap();
    let (raw, mut server) = grant_fixture(&t);
    let root = raw.root().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let publishing = raw.with_authorizer(move |conn, write| {
        grant_authorizer(conn, write)?;
        entered_tx.send(()).unwrap();
        release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(())
    });
    let cached = raw.with_authorizer(grant_authorizer);
    let writer = std::thread::spawn(move || {
        publishing.mutate("publishing", "worker", create(root.id, "published"))
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    server.busy_timeout(Duration::ZERO).unwrap();
    assert!(
        matches!(server.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate),
        Err(rusqlite::Error::SqliteFailure(e,_)) if e.code==rusqlite::ErrorCode::DatabaseBusy),
        "revoke cannot acquire authority while mutation publishes"
    );
    release_tx.send(()).unwrap();
    let published = writer.join().unwrap().unwrap();
    let revoke = server
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    revoke.execute("UPDATE grants SET revoked=1", []).unwrap();
    revoke.commit().unwrap();
    assert_eq!(raw.metadata(published.node.id).unwrap(), published.node);
    assert_eq!(
        raw.request_status("publishing").unwrap(),
        RequestStatus::Committed {
            result: published.clone()
        }
    );
    assert!(matches!(
        cached.request_status("publishing"),
        Err(Error::Denied)
    ));
    assert!(matches!(
        cached.mutate("publishing", "worker", create(root.id, "published")),
        Err(Error::Denied)
    ));
    assert!(matches!(
        cached.mutate("after-revoke", "worker", create(root.id, "denied")),
        Err(Error::Denied)
    ));
    assert_eq!(raw.blobs.list(None, 10).unwrap().len(), 1);
}
