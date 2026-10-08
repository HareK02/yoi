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
    assert_eq!(d.database.schema_version().unwrap(), 1);
    let root = d.root().unwrap();
    d.mutate("a", "actor", create(root.id, "a")).unwrap();
    assert!(matches!(
        d.mutate("duplicate", "actor", create(root.id, "a")),
        Err(Error::Conflict)
    ));
    let result=d.database.try_transaction::<_,Error>(|tx| {
        tx.execute("INSERT INTO drive_nodes(workspace_id,parent_id,name,kind,revision,size,updated_by,updated_at_ms)
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
        d.read(result.node.id, result.node.revision, 0, 100)
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
    let reader = std::thread::spawn(move || {
        g.wait();
        for _ in 0..20 {
            match other.read(first.id, first.revision, 0, 100) {
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
                    expected_revision: current.revision,
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
        d.read(current.id, current.revision, 0, 1024).unwrap().bytes,
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
        d.read(child.id, child.revision, 0, 100),
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
        d.read(n.id, n.revision, 0, 100).unwrap().bytes,
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
            expected_revision: original.revision,
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
            d.read(original.id, original.revision, 0, 100)
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
            d.read(replaced.id, replaced.revision, 0, 100)
                .unwrap()
                .bytes,
            b"failed replacement"
        );
    }
}
