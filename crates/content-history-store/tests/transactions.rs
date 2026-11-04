//! Real SQLite component contracts. Authority is an exact-key Host fixture; SQL, savepoints,
//! graph validation and persistence are production implementations. No live Server DB is used.
use content_history::{ContentId, Entry, HistoryId, Object, ObjectId, Tree, TreeId};
use content_history_store::{Access, Authority, Error, SCHEMA, ScopeKey, ScopedStore};
use rusqlite::{Connection, Transaction, params};

struct Host {
    key: ScopeKey,
    erase: bool,
}
impl Authority for Host {
    fn allows(&self, key: &ScopeKey, access: Access) -> bool {
        *key == self.key && (access != Access::Erase || self.erase)
    }
}
fn key(workspace: &str, target: &str) -> ScopeKey {
    ScopeKey {
        workspace: workspace.into(),
        target: target.into(),
    }
}
fn host(target: &str) -> Host {
    Host {
        key: key("ws", target),
        erase: true,
    }
}
fn store<'a, 'c>(tx: &'a Transaction<'c>, h: &'a Host) -> ScopedStore<'a, 'c> {
    ScopedStore::new(tx, h.key.clone(), h)
}
fn setup(conn: &mut Connection) {
    conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(SCHEMA).unwrap();
    tx.execute_batch("CREATE TABLE business(source BLOB, projection TEXT, adopted BLOB)")
        .unwrap();
    tx.commit().unwrap();
}
fn candidate(s: &ScopedStore<'_, '_>, bytes: &[u8]) -> (TreeId, ObjectId) {
    let blob = Object::Blob(bytes.to_vec());
    let ObjectId::Blob(id) = blob.id() else {
        unreachable!()
    };
    s.put(blob.id(), &blob.encode()).unwrap();
    let tree = Object::Tree(
        Tree::map(vec![Entry {
            key: "source".into(),
            target: ContentId::Blob(id),
        }])
        .unwrap(),
    );
    s.put(tree.id(), &tree.encode()).unwrap();
    let ObjectId::Tree(root) = tree.id() else {
        unreachable!()
    };
    (root, blob.id())
}
fn adopt(s: &ScopedStore<'_, '_>, root: TreeId, expected: Option<HistoryId>, t: i64) -> HistoryId {
    s.adopt(root, expected, "authenticated editor".into(), t)
        .unwrap()
}

#[test]
fn reopen_full_text_restore_aba_and_stale_head_preserve_current() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("content.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    setup(&mut conn);
    let h = host("config");
    let (a, b, ha, hb, hc, blob_a) = {
        let tx = conn.transaction().unwrap();
        let s = store(&tx, &h);
        let (a, blob_a) = candidate(&s, b"# comment\r\nname = \"\xe6\x97\xa5\xe6\x9c\xac\"\r\n");
        assert!(matches!(s.read(blob_a), Err(Error::Unavailable)));
        let ha = adopt(&s, a, None, 1);
        let (b, _) = candidate(&s, b"other\n");
        let hb = adopt(&s, b, Some(ha), 2);
        let hc = adopt(&s, a, Some(hb), 3);
        assert_ne!(ha, hc);
        assert!(matches!(
            s.adopt(b, Some(ha), "stale".into(), 4),
            Err(Error::Conflict)
        ));
        assert_eq!(s.head().unwrap(), Some(hc));
        assert!(s.snapshot(ha).unwrap().contains_key(&blob_a));
        tx.commit().unwrap();
        (a, b, ha, hb, hc, blob_a)
    };
    drop(conn);
    let mut reopened = Connection::open(path).unwrap();
    let tx = reopened.transaction().unwrap();
    let s = store(&tx, &h);
    assert_ne!(a, b);
    assert_ne!(ha, hb);
    assert_eq!(s.head().unwrap(), Some(hc));
    let old = s.snapshot(ha).unwrap();
    let restored = s.snapshot(hc).unwrap();
    assert_eq!(old, restored);
    assert_eq!(
        s.read(blob_a).unwrap(),
        Object::Blob(b"# comment\r\nname = \"\xe6\x97\xa5\xe6\x9c\xac\"\r\n".to_vec())
    );
}

#[test]
fn caller_rollback_keeps_source_projection_and_head_atomic() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let h = host("config");
    let original = {
        let tx = conn.transaction().unwrap();
        let s = store(&tx, &h);
        let (root, _) = candidate(&s, b"old");
        let head = adopt(&s, root, None, 1);
        tx.execute(
            "INSERT INTO business VALUES(?1,'old projection',?2)",
            params![b"old".as_slice(), head.as_bytes().as_slice()],
        )
        .unwrap();
        tx.commit().unwrap();
        head
    };
    {
        let tx = conn.transaction().unwrap();
        let s = store(&tx, &h);
        let (root, _) = candidate(&s, b"new");
        let head = adopt(&s, root, Some(original), 2);
        tx.execute(
            "UPDATE business SET source=?1,projection='new projection',adopted=?2",
            params![b"new".as_slice(), head.as_bytes().as_slice()],
        )
        .unwrap();
        tx.rollback().unwrap();
    }
    let tx = conn.transaction().unwrap();
    let s = store(&tx, &h);
    assert_eq!(s.head().unwrap(), Some(original));
    let row: (Vec<u8>, String, Vec<u8>) = tx
        .query_row("SELECT * FROM business", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(
        row,
        (
            b"old".to_vec(),
            "old projection".into(),
            original.as_bytes().to_vec()
        )
    );
}

#[test]
fn injected_history_or_head_failure_cannot_publish_partial_adoption_even_if_error_caught() {
    for trigger in [
        "CREATE TEMP TRIGGER fail BEFORE INSERT ON content_objects WHEN NEW.kind=3 BEGIN SELECT RAISE(ABORT,'history failure'); END",
        "CREATE TEMP TRIGGER fail BEFORE UPDATE OF head ON content_scopes BEGIN SELECT RAISE(ABORT,'head failure'); END",
        "CREATE TEMP TRIGGER fail BEFORE INSERT ON content_reachable BEGIN SELECT RAISE(ABORT,'reachability failure'); END",
    ] {
        let mut conn = Connection::open_in_memory().unwrap();
        setup(&mut conn);
        let h = host("config");
        let original = {
            let tx = conn.transaction().unwrap();
            let s = store(&tx, &h);
            let (root, _) = candidate(&s, b"old");
            let head = adopt(&s, root, None, 1);
            tx.commit().unwrap();
            head
        };
        {
            let tx = conn.transaction().unwrap();
            let s = store(&tx, &h);
            let (root, blob) = candidate(&s, b"new");
            tx.execute_batch(trigger).unwrap();
            assert!(matches!(
                s.adopt(root, Some(original), "editor".into(), 2),
                Err(Error::Sql(_))
            ));
            assert_eq!(s.head().unwrap(), Some(original));
            assert!(matches!(s.read(blob), Err(Error::Unavailable)));
            tx.execute_batch("DROP TRIGGER fail").unwrap();
            tx.commit().unwrap();
        }
        let tx = conn.transaction().unwrap();
        let s = store(&tx, &h);
        assert_eq!(s.head().unwrap(), Some(original));
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM content_objects WHERE kind=3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }
}

#[test]
fn object_failure_and_missing_or_wrong_kind_children_do_not_publish_head() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let h = host("config");
    let tx = conn.transaction().unwrap();
    let s = store(&tx, &h);
    tx.execute_batch("CREATE TEMP TRIGGER fail BEFORE INSERT ON content_objects BEGIN SELECT RAISE(ABORT,'object failure'); END").unwrap();
    let blob = Object::Blob(b"not saved".to_vec());
    assert!(matches!(
        s.put(blob.id(), &blob.encode()),
        Err(Error::Sql(_))
    ));
    tx.execute_batch("DROP TRIGGER fail").unwrap();
    assert_eq!(s.head().unwrap(), None);
    let ObjectId::Blob(id) = blob.id() else {
        unreachable!()
    };
    let tree = Object::Tree(
        Tree::map(vec![Entry {
            key: "missing".into(),
            target: ContentId::Blob(id),
        }])
        .unwrap(),
    );
    s.put(tree.id(), &tree.encode()).unwrap();
    let ObjectId::Tree(root) = tree.id() else {
        unreachable!()
    };
    assert!(matches!(
        s.adopt(root, None, "e".into(), 1),
        Err(Error::Unavailable)
    ));
    s.put(blob.id(), &blob.encode()).unwrap();
    let tree = Object::Tree(
        Tree::map(vec![Entry {
            key: "wrong kind".into(),
            target: ContentId::Tree(TreeId::from_bytes(*id.as_bytes())),
        }])
        .unwrap(),
    );
    s.put(tree.id(), &tree.encode()).unwrap();
    let ObjectId::Tree(root) = tree.id() else {
        unreachable!()
    };
    assert!(matches!(
        s.adopt(root, None, "e".into(), 1),
        Err(Error::Unavailable)
    ));
    assert_eq!(s.head().unwrap(), None);
}

#[test]
fn scoped_hash_reads_and_adoption_cannot_cross_workspace_or_target() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let a = host("a");
    let b = host("b");
    let tx = conn.transaction().unwrap();
    let sa = store(&tx, &a);
    let (root, blob) = candidate(&sa, b"private");
    let head = adopt(&sa, root, None, 1);
    let sb = store(&tx, &b);
    assert!(matches!(sb.read(blob), Err(Error::Unavailable)));
    assert!(matches!(
        sb.read(ObjectId::History(head)),
        Err(Error::Unavailable)
    ));
    assert!(matches!(
        sb.adopt(root, None, "e".into(), 1),
        Err(Error::Unavailable)
    ));
    let denied = ScopedStore::new(&tx, key("another-workspace", "a"), &a);
    assert!(matches!(denied.read(blob), Err(Error::Denied)));
    assert!(matches!(denied.head(), Err(Error::Denied)));
    let denied = ScopedStore::new(&tx, b.key.clone(), &a);
    assert!(matches!(denied.read(blob), Err(Error::Denied)));
}

#[test]
fn explicit_erasure_never_revives_old_scope_and_preserves_shared_history() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let a = host("a");
    let b = host("b");
    let (root, blob, ha, hb) = {
        let tx = conn.transaction().unwrap();
        let sa = store(&tx, &a);
        let sb = store(&tx, &b);
        let (root, blob) = candidate(&sa, b"shared");
        let ha = adopt(&sa, root, None, 1);
        let (root_b, _) = candidate(&sb, b"shared");
        assert_eq!(root, root_b);
        let hb = adopt(&sb, root, None, 2);
        let (old, _) = candidate(&sb, b"new");
        adopt(&sb, old, Some(hb), 3);
        sb.collect_unadopted().unwrap();
        assert!(sb.snapshot(hb).unwrap().contains_key(&blob));
        sa.erase().unwrap();
        assert!(matches!(sa.read(blob), Err(Error::Erased)));
        assert!(matches!(sa.snapshot(ha), Err(Error::Erased)));
        assert!(sb.snapshot(hb).unwrap().contains_key(&blob));
        tx.commit().unwrap();
        (root, blob, ha, hb)
    };
    let tx = conn.transaction().unwrap();
    let sa = store(&tx, &a);
    let sb = store(&tx, &b);
    assert!(matches!(
        sa.adopt(root, Some(ha), "e".into(), 4),
        Err(Error::Erased)
    ));
    assert!(matches!(
        sa.put(
            Object::Blob(b"shared".to_vec()).id(),
            &Object::Blob(b"shared".to_vec()).encode()
        ),
        Err(Error::Erased)
    ));
    assert!(sb.snapshot(hb).unwrap().contains_key(&blob));
    sb.erase().unwrap();
    assert_eq!(
        tx.query_row("SELECT count(*) FROM content_objects", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn tampered_bytes_or_unsupported_format_fail_read_and_immutable_save() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let h = host("config");
    let tx = conn.transaction().unwrap();
    let s = store(&tx, &h);
    let (root, blob) = candidate(&s, b"original");
    let head = adopt(&s, root, None, 1);
    let original = s.read(blob).unwrap();
    assert!(matches!(
        s.put(blob, &Object::Blob(b"different".to_vec()).encode()),
        Err(Error::Model(_))
    ));
    let ObjectId::Blob(id) = blob else {
        unreachable!()
    };
    tx.execute(
        "UPDATE content_objects SET bytes=?1 WHERE kind=1 AND hash=?2",
        params![
            Object::Blob(b"tamper".to_vec()).encode(),
            id.as_bytes().as_slice()
        ],
    )
    .unwrap();
    assert!(matches!(s.snapshot(head), Err(Error::Model(_))));
    assert!(matches!(
        s.put(blob, &original.encode()),
        Err(Error::ImmutableMismatch)
    ));
    let mut bad = original.encode();
    bad[4] = 255;
    tx.execute(
        "UPDATE content_objects SET bytes=?1 WHERE kind=1 AND hash=?2",
        params![bad, id.as_bytes().as_slice()],
    )
    .unwrap();
    assert!(matches!(s.snapshot(head), Err(Error::Model(_))));
}

#[test]
fn independent_connections_reject_stale_observer_without_replacing_winner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.sqlite");
    let mut first = Connection::open(&path).unwrap();
    setup(&mut first);
    let mut second = Connection::open(&path).unwrap();
    let h = host("config");
    let initial = {
        let tx = first.transaction().unwrap();
        let s = store(&tx, &h);
        let (root, _) = candidate(&s, b"initial");
        let head = adopt(&s, root, None, 1);
        tx.commit().unwrap();
        head
    };
    // Both clients observe the same head before either submits its edit.
    for conn in [&mut first, &mut second] {
        let tx = conn.transaction().unwrap();
        assert_eq!(store(&tx, &h).head().unwrap(), Some(initial));
        tx.commit().unwrap();
    }
    let winner = {
        let tx = first.transaction().unwrap();
        let s = store(&tx, &h);
        let (root, _) = candidate(&s, b"winner");
        let winner = adopt(&s, root, Some(initial), 2);
        tx.commit().unwrap();
        winner
    };
    let tx = second.transaction().unwrap();
    let s = store(&tx, &h);
    let (root, _) = candidate(&s, b"loser");
    assert!(matches!(
        s.adopt(root, Some(initial), "loser".into(), 3),
        Err(Error::Conflict)
    ));
    assert_eq!(s.head().unwrap(), Some(winner));
    tx.commit().unwrap();
}

#[test]
fn deduplication_and_collection_preserve_other_preparations_and_all_adopted_ancestors() {
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let a = host("a");
    let b = host("b");
    let tx = conn.transaction().unwrap();
    let sa = store(&tx, &a);
    let sb = store(&tx, &b);
    let (root, blob) = candidate(&sa, b"shared");
    let first = adopt(&sa, root, None, 1);
    let count = || {
        tx.query_row("SELECT count(*) FROM content_objects", [], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap()
    };
    assert_eq!(count(), 3);
    candidate(&sa, b"shared");
    candidate(&sb, b"shared");
    assert_eq!(count(), 3);
    let (new, _) = candidate(&sa, b"new");
    adopt(&sa, new, Some(first), 2);
    candidate(&sa, b"unadopted");
    let before = count();
    sa.collect_unadopted().unwrap();
    assert_eq!(count(), before - 2);
    assert!(sa.snapshot(first).unwrap().contains_key(&blob));
    sa.erase().unwrap();
    assert_eq!(count(), 2); // B's preparation survives A's erasure.
    let b_head = adopt(&sb, root, None, 3);
    assert!(sb.snapshot(b_head).unwrap().contains_key(&blob));
}

#[test]
fn revoked_authority_is_checked_again_on_existing_scoped_handle() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Revocable(AtomicBool);
    impl Authority for Revocable {
        fn allows(&self, _: &ScopeKey, _: Access) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }
    let mut conn = Connection::open_in_memory().unwrap();
    setup(&mut conn);
    let authority = Revocable(AtomicBool::new(true));
    let tx = conn.transaction().unwrap();
    let s = ScopedStore::new(&tx, key("ws", "a"), &authority);
    let (root, blob) = candidate(&s, b"protected");
    let head = adopt(&s, root, None, 1);
    authority.0.store(false, Ordering::SeqCst);
    assert!(matches!(s.read(blob), Err(Error::Denied)));
    assert!(matches!(s.snapshot(head), Err(Error::Denied)));
    assert!(matches!(
        s.adopt(root, Some(head), "e".into(), 2),
        Err(Error::Denied)
    ));
    assert!(matches!(s.erase(), Err(Error::Denied)));
}
