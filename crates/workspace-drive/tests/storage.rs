use feature_storage::FeatureStorage;
use std::{
    path::Path,
    sync::{Arc, Barrier},
};
use workspace_drive::{
    Drive, Error, Kind, MAX_FILE_BYTES, MAX_READ_BYTES, Mutation, Node, RequestStatus,
};

fn open(metadata: &Path, blobs: &Path, workspace: &str) -> Drive {
    let storage = FeatureStorage::new(metadata).workspace(workspace).unwrap();
    let registered = Drive::register(&storage).unwrap();
    Drive::open(&storage, &registered, blobs).unwrap()
}
fn folder(d: &Drive, parent: &Node, name: &str) -> Node {
    d.mutate(
        name,
        "account",
        Mutation::CreateFolder {
            parent: parent.id,
            name: name.into(),
        },
    )
    .unwrap()
    .node
}
fn file(
    d: &Drive,
    parent: &Node,
    request: &str,
    name: &str,
    content_type: &str,
    bytes: &[u8],
) -> Node {
    d.mutate(
        request,
        "account",
        Mutation::CreateFile {
            parent: parent.id,
            name: name.into(),
            content_type: content_type.into(),
            bytes: bytes.to_vec(),
        },
    )
    .unwrap()
    .node
}
fn read(d: &Drive, n: &Node) -> Vec<u8> {
    d.read(n.id, n.revision, 0, MAX_READ_BYTES).unwrap().bytes
}
fn update(n: &Node, bytes: &[u8]) -> Mutation {
    Mutation::Update {
        id: n.id,
        expected_revision: n.revision,
        content_type: "text/markdown".into(),
        bytes: bytes.to_vec(),
    }
}

#[test]
fn markdown_image_empty_and_folders_survive_restart_with_stable_identity() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("metadata");
    let blobs = t.path().join("drives/ws/blobs");
    let d = open(&db, &blobs, "ws");
    let root = d.root().unwrap();
    let dir = folder(&d, &root, "日本語");
    let markdown = file(
        &d,
        &dir,
        "markdown",
        "資料.md",
        "text/markdown",
        "# 日本語\n".as_bytes(),
    );
    let image = file(
        &d,
        &dir,
        "image",
        "画像.png",
        "image/png",
        b"\x89PNG\r\n\x1a\n",
    );
    let empty = file(&d, &dir, "empty", "空", "text/plain", b"");
    assert_eq!(dir.kind, Kind::Directory);
    assert!(read(&d, &empty).is_empty());
    let latest = d
        .mutate("update", "account", update(&markdown, b"# latest"))
        .unwrap()
        .node;
    drop(d);
    let d = open(&db, &blobs, "ws");
    assert_eq!(d.root().unwrap(), root);
    assert_eq!(d.metadata(dir.id).unwrap(), dir);
    assert_eq!(d.metadata(latest.id).unwrap(), latest);
    assert_eq!(read(&d, &latest), b"# latest");
    assert_eq!(read(&d, &image), b"\x89PNG\r\n\x1a\n");
    assert_eq!(d.list(dir.id, None, 200).unwrap().nodes.len(), 3);
}

#[test]
fn relocate_and_paged_list_only_use_metadata_even_when_blob_is_missing() {
    let t = tempfile::tempdir().unwrap();
    let blobs = t.path().join("blobs");
    let d = open(&t.path().join("db"), &blobs, "ws");
    let root = d.root().unwrap();
    let a = folder(&d, &root, "a");
    let b = folder(&d, &root, "b");
    let f = file(&d, &a, "f", "f", "text/plain", b"data");
    let before = std::fs::read_dir(&blobs)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    let moved = d
        .mutate(
            "move",
            "account",
            Mutation::Relocate {
                id: f.id,
                expected_revision: f.revision,
                parent: b.id,
                name: "改名".into(),
            },
        )
        .unwrap()
        .node;
    let after = std::fs::read_dir(&blobs)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(before, after);
    assert_eq!(read(&d, &moved), b"data");
    std::fs::remove_file(blobs.join(&before[0])).unwrap();
    assert_eq!(d.list(b.id, None, 1).unwrap().nodes, vec![moved.clone()]);
    assert!(matches!(
        d.read(moved.id, moved.revision, 0, 1),
        Err(Error::Blob(_))
    ));
    let first = d.list(root.id, None, 1).unwrap();
    assert_eq!(first.nodes, vec![a]);
    let second = d.list(root.id, first.next_after, 1).unwrap();
    assert_eq!(second.nodes, vec![b]);
    assert!(second.next_after.is_none());
}

#[test]
fn parallel_revision_and_name_competitors_use_database_authority() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("db");
    let blobs = t.path().join("blobs");
    let d = open(&db, &blobs, "ws");
    let other = open(&db, &blobs, "ws");
    let root = d.root().unwrap();
    let f = file(&d, &root, "original", "f", "text/plain", b"old");
    let barrier = Arc::new(Barrier::new(2));
    let b = barrier.clone();
    let f2 = f.clone();
    let c = other.clone();
    let worker = std::thread::spawn(move || {
        b.wait();
        c.mutate("update-two", "account", update(&f2, b"two"))
    });
    barrier.wait();
    let one = d.mutate("update-one", "account", update(&f, b"one"));
    let two = worker.join().unwrap();
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(matches!(one, Err(Error::Conflict)) || matches!(two, Err(Error::Conflict)));
    let b = Arc::new(Barrier::new(2));
    let bb = b.clone();
    let r = root.clone();
    let worker = std::thread::spawn(move || {
        bb.wait();
        other.mutate(
            "same-two",
            "account",
            Mutation::CreateFolder {
                parent: r.id,
                name: "same".into(),
            },
        )
    });
    b.wait();
    let one = d.mutate(
        "same-one",
        "account",
        Mutation::CreateFolder {
            parent: root.id,
            name: "same".into(),
        },
    );
    let two = worker.join().unwrap();
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(matches!(one, Err(Error::Conflict)) || matches!(two, Err(Error::Conflict)));
}

#[test]
fn concurrent_cycle_moves_and_parent_delete_child_create_preserve_tree() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("db");
    let blobs = t.path().join("blobs");
    let d = open(&db, &blobs, "ws");
    let other = open(&db, &blobs, "ws");
    let root = d.root().unwrap();
    let a = folder(&d, &root, "a");
    let b = folder(&d, &root, "b");
    let barrier = Arc::new(Barrier::new(2));
    let bb = barrier.clone();
    let aa = a.clone();
    let b2 = b.clone();
    let c = other.clone();
    let worker = std::thread::spawn(move || {
        bb.wait();
        c.mutate(
            "b-to-a",
            "account",
            Mutation::Relocate {
                id: b2.id,
                expected_revision: b2.revision,
                parent: aa.id,
                name: "b".into(),
            },
        )
    });
    barrier.wait();
    let one = d.mutate(
        "a-to-b",
        "account",
        Mutation::Relocate {
            id: a.id,
            expected_revision: a.revision,
            parent: b.id,
            name: "a".into(),
        },
    );
    let two = worker.join().unwrap();
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let p = folder(&d, &root, "parent");
    let p2 = p.clone();
    let barrier = Arc::new(Barrier::new(2));
    let bb = barrier.clone();
    let worker = std::thread::spawn(move || {
        bb.wait();
        other.mutate(
            "create-child",
            "account",
            Mutation::CreateFolder {
                parent: p2.id,
                name: "child".into(),
            },
        )
    });
    barrier.wait();
    let deleted = d.mutate(
        "delete-parent",
        "account",
        Mutation::Delete {
            id: p.id,
            expected_revision: p.revision,
        },
    );
    let created = worker.join().unwrap();
    assert_eq!(
        usize::from(deleted.is_ok()) + usize::from(created.is_ok()),
        1
    );
    if created.is_ok() {
        assert_eq!(d.list(p.id, None, 10).unwrap().nodes.len(), 1);
    } else {
        assert!(matches!(d.metadata(p.id), Err(Error::NotFound)));
    }
}

#[test]
fn receipts_survive_response_loss_and_restart_without_double_mutation() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("db");
    let blobs = t.path().join("blobs");
    let d = open(&db, &blobs, "ws");
    let root = d.root().unwrap();
    let m = Mutation::CreateFile {
        parent: root.id,
        name: "lost-response".into(),
        content_type: "text/plain".into(),
        bytes: b"first".to_vec(),
    };
    assert!(matches!(
        d.request_status("create").unwrap(),
        RequestStatus::Uncommitted
    ));
    let committed = d.mutate("create", "account", m.clone()).unwrap();
    drop(d);
    let d = open(&db, &blobs, "ws");
    assert_eq!(
        d.request_status("create").unwrap(),
        RequestStatus::Committed {
            result: committed.clone()
        }
    );
    assert_eq!(d.mutate("create", "account", m.clone()).unwrap(), committed);
    assert!(matches!(
        d.mutate("create", "someone-else", m),
        Err(Error::Conflict)
    ));
    let update = update(&committed.node, b"second");
    let updated = d.mutate("update", "account", update.clone()).unwrap();
    assert_eq!(d.mutate("update", "account", update).unwrap(), updated);
    assert_eq!(d.list(root.id, None, 20).unwrap().nodes.len(), 1);
}

#[test]
fn deletion_recreation_does_not_reuse_id_or_silently_switch_read_revision() {
    let t = tempfile::tempdir().unwrap();
    let d = open(&t.path().join("db"), &t.path().join("blobs"), "ws");
    let root = d.root().unwrap();
    let old = file(&d, &root, "old", "file", "text/plain", b"old");
    let next = d
        .mutate("update", "account", update(&old, b"new"))
        .unwrap()
        .node;
    assert!(matches!(
        d.read(old.id, old.revision, 0, 1),
        Err(Error::Conflict)
    ));
    d.mutate(
        "delete",
        "account",
        Mutation::Delete {
            id: next.id,
            expected_revision: next.revision,
        },
    )
    .unwrap();
    let recreated = file(&d, &root, "recreated", "file", "text/plain", b"recreated");
    assert!(recreated.id > old.id);
    assert!(matches!(d.metadata(old.id), Err(Error::NotFound)));
    assert_eq!(read(&d, &recreated), b"recreated");
    assert!(d.collect(None, 200).unwrap().removed >= 2);
    assert_eq!(read(&d, &recreated), b"recreated");
}

#[test]
fn bounded_search_and_read_reject_paths_oversize_and_invalid_parents() {
    let t = tempfile::tempdir().unwrap();
    let d = open(&t.path().join("db"), &t.path().join("blobs"), "ws");
    let root = d.root().unwrap();
    for name in ["../escape", "a/b", "a\\b", ".", "..", "", "bad\0name"] {
        assert!(matches!(
            d.mutate(
                &uuid::Uuid::now_v7().to_string(),
                "account",
                Mutation::CreateFolder {
                    parent: root.id,
                    name: name.into()
                }
            ),
            Err(Error::Invalid(_))
        ));
    }
    let f = file(
        &d,
        &root,
        "f",
        "f",
        "text/markdown",
        "日本語 needle".as_bytes(),
    );
    let g = file(&d, &root, "g", "needle.png", "image/png", b"needle");
    assert!(matches!(
        d.read(f.id, f.revision, 0, MAX_READ_BYTES + 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.read(f.id, f.revision, f.size + 1, 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.mutate("huge", "account", update(&f, &vec![0; MAX_FILE_BYTES + 1])),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(d.list(f.id, None, 10), Err(Error::Invalid(_))));
    assert!(matches!(
        d.mutate(
            "invalid-parent",
            "account",
            Mutation::CreateFolder {
                parent: f.id,
                name: "no".into()
            }
        ),
        Err(Error::Invalid(_))
    ));
    let first = d.search("needle", true, None, 1).unwrap();
    assert_eq!(first.nodes, vec![f]);
    assert_eq!(first.examined, 1);
    let second = d.search("needle", true, first.next_after, 1).unwrap();
    assert_eq!(second.nodes, vec![g]);
    assert!(second.next_after.is_none());
    assert!(
        d.search("missing", true, None, 1)
            .unwrap()
            .next_after
            .is_some()
    );
}

#[test]
fn fence_purge_restart_and_workspace_boundaries_are_persistent() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("db");
    let blobs = t.path().join("drives/a/blobs");
    let a = open(&db, &blobs, "a");
    let b = open(&db, &t.path().join("drives/b/blobs"), "b");
    let ar = a.root().unwrap();
    let br = b.root().unwrap();
    let af = file(&a, &ar, "af", "f", "text/plain", b"a");
    let bf = file(&b, &br, "bf", "f", "text/plain", b"b");
    let ad = folder(&a, &ar, "folder");
    file(&a, &ad, "child", "child", "text/plain", b"child");
    a.fence().unwrap();
    drop(a);
    let a = open(&db, &blobs, "a");
    assert!(matches!(
        a.mutate("after-fence", "account", update(&af, b"bad")),
        Err(Error::Fenced)
    ));
    assert!(matches!(a.collect(None, 1), Err(Error::Fenced)));
    a.purge().unwrap();
    a.purge().unwrap();
    assert!(a.list(ar.id, None, 10).unwrap().nodes.is_empty());
    assert_eq!(std::fs::read_dir(&blobs).unwrap().count(), 0);
    assert_eq!(read(&b, &bf), b"b");
    assert_eq!(a.root().unwrap().workspace_id, "a");
    assert_eq!(b.root().unwrap().workspace_id, "b");
}

#[test]
fn stopped_writer_backup_restores_ids_revisions_receipts_and_sequence_with_blobs() {
    let t = tempfile::tempdir().unwrap();
    let metadata = t.path().join("metadata");
    let blobs = t.path().join("blobs");
    let manager = FeatureStorage::new(&metadata);
    let scope = manager.workspace("ws").unwrap();
    let registered = Drive::register(&scope).unwrap();
    let d = Drive::open(&scope, &registered, &blobs).unwrap();
    let root = d.root().unwrap();
    let f = file(&d, &root, "f", "file", "text/plain", b"one");
    let latest = d
        .mutate("update", "account", update(&f, b"two"))
        .unwrap()
        .node;
    let deleted = folder(&d, &root, "deleted");
    d.mutate(
        "delete",
        "account",
        Mutation::Delete {
            id: deleted.id,
            expected_revision: deleted.revision,
        },
    )
    .unwrap();
    // All writers/GC stopped at this boundary. FeatureStorage's DB snapshot alone
    // is not a Drive backup; copy the same stopped generation's blobs as well.
    drop(d);
    let snapshot = t.path().join("snapshot");
    scope.backup(&snapshot).unwrap();
    manager.shutdown().unwrap();
    let restored_blobs = t.path().join("restored/blobs");
    std::fs::create_dir_all(&restored_blobs).unwrap();
    for file in std::fs::read_dir(&blobs).unwrap() {
        let file = file.unwrap();
        std::fs::copy(file.path(), restored_blobs.join(file.file_name())).unwrap();
    }
    let restored = FeatureStorage::new(t.path().join("restored/metadata"));
    let scope = restored.workspace("ws").unwrap();
    scope.restore(&snapshot).unwrap();
    let registered = Drive::register(&scope).unwrap();
    let d = Drive::open(&scope, &registered, &restored_blobs).unwrap();
    assert_eq!(d.metadata(latest.id).unwrap(), latest);
    assert_eq!(read(&d, &latest), b"two");
    assert!(matches!(
        d.request_status("update").unwrap(),
        RequestStatus::Committed { .. }
    ));
    let new = folder(&d, &root, "new");
    assert!(new.id > deleted.id);
}

#[test]
fn external_ids_and_revisions_keep_full_integer_precision() {
    let max = "9223372036854775807";
    let id: workspace_drive::NodeId = serde_json::from_str(&format!("\"{max}\"")).unwrap();
    let revision: workspace_drive::Revision = serde_json::from_str(&format!("\"{max}\"")).unwrap();
    assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{max}\""));
    assert_eq!(
        serde_json::to_string(&revision).unwrap(),
        format!("\"{max}\"")
    );
    for bad in ["0", "-1", "01", "9223372036854775808"] {
        assert!(serde_json::from_str::<workspace_drive::NodeId>(&format!("\"{bad}\"")).is_err());
    }
    assert!(serde_json::from_str::<workspace_drive::NodeId>("9007199254740993").is_err());
}

#[test]
fn text_read_is_bounded_utf8_and_referenced_size_corruption_is_diagnosed() {
    let t = tempfile::tempdir().unwrap();
    let blobs = t.path().join("blobs");
    let d = open(&t.path().join("db"), &blobs, "ws");
    let root = d.root().unwrap();
    let f = file(&d, &root, "f", "f", "text/plain", "日本語".as_bytes());
    assert_eq!(d.read_text(f.id, f.revision, 0, 3).unwrap().text, "日");
    assert!(matches!(
        d.read_text(f.id, f.revision, 1, 2),
        Err(Error::Invalid(_))
    ));
    let path = std::fs::read_dir(&blobs)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&path, b"corrupt length").unwrap();
    assert!(matches!(
        d.read(f.id, f.revision, 0, 1),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        d.search("not-the-name", true, None, 10),
        Err(Error::Invalid(_))
    ));
    // Missing zero-sized references are not silently successful empty reads.
    let empty = file(&d, &root, "empty", "empty", "text/plain", b"");
    for e in std::fs::read_dir(&blobs).unwrap() {
        let e = e.unwrap();
        if e.path() != path {
            std::fs::remove_file(e.path()).unwrap();
        }
    }
    assert!(matches!(
        d.read(empty.id, empty.revision, 0, 1),
        Err(Error::Blob(_))
    ));
}

#[test]
fn simultaneous_same_request_replays_one_create_and_folder_deletion_is_empty_only() {
    let t = tempfile::tempdir().unwrap();
    let db = t.path().join("db");
    let blobs = t.path().join("blobs");
    let d = open(&db, &blobs, "ws");
    let other = open(&db, &blobs, "ws");
    let root = d.root().unwrap();
    let m = Mutation::CreateFolder {
        parent: root.id,
        name: "same".into(),
    };
    let mm = m.clone();
    let barrier = Arc::new(Barrier::new(2));
    let bb = barrier.clone();
    let worker = std::thread::spawn(move || {
        bb.wait();
        other.mutate("same-request", "account", mm).unwrap()
    });
    barrier.wait();
    let one = d.mutate("same-request", "account", m).unwrap();
    let two = worker.join().unwrap();
    assert_eq!(one, two);
    assert_eq!(d.list(root.id, None, 10).unwrap().nodes.len(), 1);
    let child = folder(&d, &one.node, "child");
    assert!(matches!(
        d.mutate(
            "nonempty",
            "account",
            Mutation::Delete {
                id: one.node.id,
                expected_revision: one.node.revision
            }
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(d.metadata(child.id).unwrap(), child);
    assert!(matches!(
        d.mutate(
            "root-delete",
            "account",
            Mutation::Delete {
                id: root.id,
                expected_revision: root.revision
            }
        ),
        Err(Error::Invalid(_))
    ));
    let changed = d
        .mutate(
            "change",
            "account",
            Mutation::Relocate {
                id: child.id,
                expected_revision: child.revision,
                parent: root.id,
                name: "child".into(),
            },
        )
        .unwrap()
        .node;
    assert!(matches!(
        d.mutate(
            "stale-delete",
            "account",
            Mutation::Delete {
                id: child.id,
                expected_revision: child.revision
            }
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(d.metadata(changed.id).unwrap(), changed);
}
