//! Pure model contract tests; the fixture store substitutes only scoped byte I/O.
//! No claims about database transactions, authorization, head CAS or business adoption.
mod common;

use content_history::*;
use std::collections::BTreeMap;

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn content(object: &Object) -> ContentId {
    match object.id() {
        ObjectId::Blob(id) => ContentId::Blob(id),
        ObjectId::Tree(id) => ContentId::Tree(id),
        _ => panic!("content required"),
    }
}
fn entry(key: &str, target: ContentId) -> Entry {
    Entry {
        key: key.into(),
        target,
    }
}
fn map(entries: Vec<Entry>) -> Object {
    Object::Tree(Tree::map(entries).unwrap())
}
fn tree_id(object: &Object) -> TreeId {
    let ContentId::Tree(id) = content(object) else {
        panic!("tree required")
    };
    id
}

#[test]
fn version_one_vectors_freeze_bytes_hashes_and_parity_words() {
    let lines: Vec<_> = include_str!("fixtures/v1.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .collect();
    let objects = common::objects();
    assert_eq!(lines.len(), objects.len());
    for (index, (line, object)) in lines.iter().zip(&objects).enumerate() {
        let columns: Vec<_> = line.split('\t').collect();
        let bytes = unhex(columns[1]);
        let digest = unhex(columns[2]);
        assert_eq!(object.encode(), bytes, "{} bytes", columns[0]);
        assert_eq!(
            common::digest_bytes(object).as_slice(),
            digest,
            "{} hash",
            columns[0]
        );
        assert_eq!(Object::decode(&bytes).unwrap(), *object);
        assert_eq!(verify(object.id(), &bytes).unwrap(), *object);
        for word in 0..8 {
            assert_eq!(
                common::parity_word((index * 8 + word) as u32),
                u32::from_be_bytes(digest[word * 4..word * 4 + 4].try_into().unwrap())
            );
        }
    }
    assert_eq!(common::parity_word((objects.len() * 8) as u32), u32::MAX);
}

#[test]
fn maps_canonicalize_enumeration_but_sequences_keys_and_exact_bytes_affect_identity() {
    let blob = Object::Blob(b"x\r\n".to_vec());
    let target = content(&blob);
    let a = entry("a", target);
    let b = entry("é", target);
    assert_eq!(
        map(vec![b.clone(), a.clone()]).id(),
        map(vec![a.clone(), b.clone()]).id()
    );
    assert_ne!(
        Object::Tree(Tree::sequence(vec![a.clone(), b.clone()]).unwrap()).id(),
        Object::Tree(Tree::sequence(vec![b.clone(), a.clone()]).unwrap()).id()
    );
    assert_ne!(
        map(vec![a.clone()]).id(),
        map(vec![entry("b", target)]).id()
    );
    assert_ne!(blob.id(), Object::Blob(b"x\n".to_vec()).id());
    assert_ne!(
        Object::Blob("é".as_bytes().to_vec()).id(),
        Object::Blob("e\u{301}".as_bytes().to_vec()).id()
    );
    let ContentId::Blob(hash) = target else {
        unreachable!()
    };
    assert_ne!(
        map(vec![a.clone()]).id(),
        map(vec![entry(
            "a",
            ContentId::Tree(TreeId::from_bytes(*hash.as_bytes()))
        )])
        .id()
    );
    for result in [
        Tree::map(vec![a.clone(), a.clone()]),
        Tree::sequence(vec![a.clone(), a]),
    ] {
        assert!(matches!(result, Err(Error::DuplicateKey(_))));
    }
}

#[test]
fn decoder_rejects_all_truncations_and_trailing_bytes() {
    for object in common::objects() {
        let bytes = object.encode();
        for end in 0..bytes.len() {
            assert!(Object::decode(&bytes[..end]).is_err(), "prefix {end}");
        }
        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(Object::decode(&trailing), Err(Error::TrailingBytes));
    }
}

#[test]
fn decoder_rejects_unknown_domains_and_malicious_lengths() {
    let blob = Object::Blob(vec![]).encode();
    let mut malformed = blob.clone();
    malformed[0] = 0;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidMagic));
    malformed = blob.clone();
    malformed[5] = 2;
    assert_eq!(
        Object::decode(&malformed),
        Err(Error::UnsupportedVersion(2))
    );
    malformed = blob.clone();
    malformed[6] = 99;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidTag(99)));
    malformed = blob;
    malformed[7..15].fill(255);
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidLength));
    let target = ContentId::Blob(BlobId::from_bytes([0; 32]));
    let tree = map(vec![entry("a", target)]).encode();
    malformed = tree.clone();
    malformed[7] = 2;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidTreeMode(2)));
    malformed = tree.clone();
    malformed[8..16].fill(255);
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidLength));
    malformed = tree.clone();
    malformed[16..24].fill(255);
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidLength));
    malformed = tree.clone();
    malformed[24] = 255;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidUtf8));
    malformed = tree;
    malformed[25] = 3;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidChildKind(3)));
    let history = common::objects()[6].encode();
    malformed = history.clone();
    malformed[39] = 2;
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidParentFlag(2)));
    malformed = history;
    malformed[40..48].fill(255);
    assert_eq!(Object::decode(&malformed), Err(Error::InvalidLength));
}

#[test]
fn decoder_refuses_unsorted_maps_and_duplicate_sequences_instead_of_repairing_bytes() {
    let target = ContentId::Blob(BlobId::from_bytes([0; 32]));
    let mut sequence =
        Object::Tree(Tree::sequence(vec![entry("b", target), entry("a", target)]).unwrap())
            .encode();
    sequence[7] = 0;
    assert_eq!(Object::decode(&sequence), Err(Error::NonCanonicalMap));
    sequence[7] = 1;
    // First entry starts at 16, each single-byte-key entry occupies 42 bytes.
    sequence[66] = b'b';
    assert!(matches!(
        Object::decode(&sequence),
        Err(Error::DuplicateKey(_))
    ));
    sequence[7] = 0;
    assert_eq!(Object::decode(&sequence), Err(Error::NonCanonicalMap));
}

#[test]
fn integrity_rejects_modified_content_and_wrong_typed_identity() {
    let object = Object::Blob(b"original".to_vec());
    assert_eq!(
        verify(object.id(), &Object::Blob(b"modified".to_vec()).encode()),
        Err(Error::HashMismatch)
    );
    let ObjectId::Blob(id) = object.id() else {
        unreachable!()
    };
    assert_eq!(
        verify(
            ObjectId::Tree(TreeId::from_bytes(*id.as_bytes())),
            &object.encode()
        ),
        Err(Error::HashMismatch)
    );
    assert_eq!(id.to_string().len(), 64);
    assert!(
        id.to_string()
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
}

#[derive(Default)]
struct Memory {
    objects: BTreeMap<ObjectId, Vec<u8>>,
    reads: Vec<ObjectId>,
}
impl Memory {
    fn put(&mut self, object: Object) -> ContentId {
        self.objects.insert(object.id(), object.encode());
        content(&object)
    }
    fn root(&mut self, object: Object) -> TreeId {
        let root = tree_id(&object);
        self.put(object);
        root
    }
}
impl ObjectReader for Memory {
    fn read(&mut self, id: ObjectId) -> Result<Vec<u8>, Error> {
        self.reads.push(id);
        self.objects
            .get(&id)
            .cloned()
            .ok_or(Error::MissingObject(id))
    }
}
fn restored_entry(key: &str, content: RestoredContent) -> RestoredEntry {
    RestoredEntry {
        key: key.into(),
        content,
    }
}
fn restored_map(entries: Vec<RestoredEntry>) -> RestoredContent {
    RestoredContent::Tree {
        ordered: false,
        entries,
    }
}

#[test]
fn configuration_and_ticket_shapes_restore_exact_bytes_and_business_key_order() {
    let mut store = Memory::default();
    let source = "# comment\r\nname = \"é\"\n".as_bytes();
    let file = store.put(Object::Blob(source.to_vec()));
    let config = store.put(map(vec![entry("settings.dcdl", file)]));
    let title = store.put(Object::Blob(b"title".to_vec()));
    let body = store.put(Object::Blob(b"initial body".to_vec()));
    let posts = store.put(Object::Tree(
        Tree::sequence(vec![entry("post-z", body), entry("post-a", title)]).unwrap(),
    ));
    let ticket = store.put(map(vec![
        entry("title", title),
        entry("body", body),
        entry("posts", posts),
    ]));
    let root = store.root(map(vec![
        entry("config", config),
        entry("ticket-identity", ticket),
    ]));
    let restored = restore(&mut store, root, TraversalLimits::default()).unwrap();
    assert_eq!(
        restored,
        restored_map(vec![
            restored_entry(
                "config",
                restored_map(vec![restored_entry(
                    "settings.dcdl",
                    RestoredContent::Blob(source.to_vec())
                )])
            ),
            restored_entry(
                "ticket-identity",
                restored_map(vec![
                    restored_entry("body", RestoredContent::Blob(b"initial body".to_vec())),
                    restored_entry(
                        "posts",
                        RestoredContent::Tree {
                            ordered: true,
                            entries: vec![
                                restored_entry(
                                    "post-z",
                                    RestoredContent::Blob(b"initial body".to_vec())
                                ),
                                restored_entry("post-a", RestoredContent::Blob(b"title".to_vec()))
                            ]
                        }
                    ),
                    restored_entry("title", RestoredContent::Blob(b"title".to_vec())),
                ])
            ),
        ])
    );
}

#[test]
fn copy_on_write_path_replacement_preserves_old_fulltext_and_shares_unchanged_objects() {
    let mut store = Memory::default();
    let old_blob = store.put(Object::Blob(b"old full text".to_vec()));
    let new_blob = store.put(Object::Blob(b"new full text".to_vec()));
    let child = store.put(map(vec![
        entry("body", old_blob),
        entry("unchanged", old_blob),
    ]));
    let root = store.root(map(vec![entry("document", child)]));
    let old_snapshot = restore(&mut store, root, TraversalLimits::default()).unwrap();
    let rewrite = replace_path(
        &mut store,
        root,
        &["document", "body"],
        Some(new_blob),
        TraversalLimits::default(),
    )
    .unwrap();
    assert_eq!(rewrite.objects.len(), 2);
    assert_ne!(rewrite.root, root);
    for object in &rewrite.objects {
        store.put(object.clone());
    }
    assert_eq!(
        restore(&mut store, root, TraversalLimits::default()).unwrap(),
        old_snapshot
    );
    assert_ne!(
        restore(&mut store, rewrite.root, TraversalLimits::default()).unwrap(),
        old_snapshot
    );
    assert_eq!(
        diff(&mut store, root, rewrite.root, TraversalLimits::default()).unwrap(),
        vec![Change {
            path: vec!["document".into(), "body".into()],
            before: Some(old_blob),
            after: Some(new_blob)
        }]
    );
    let unchanged = replace_path(
        &mut store,
        root,
        &["document", "body"],
        Some(old_blob),
        TraversalLimits::default(),
    )
    .unwrap();
    assert_eq!(unchanged.root, root);
    assert!(unchanged.objects.is_empty());
    let returned = replace_path(
        &mut store,
        rewrite.root,
        &["document", "body"],
        Some(old_blob),
        TraversalLimits::default(),
    )
    .unwrap();
    assert_eq!(returned.root, root);
    let first = Object::History(History {
        root,
        parent: None,
        actor: "editor".into(),
        timestamp_micros: 0,
    });
    let ObjectId::History(parent) = first.id() else {
        unreachable!()
    };
    let restored = Object::History(History {
        root: returned.root,
        parent: Some(parent),
        actor: "editor".into(),
        timestamp_micros: 1,
    });
    assert_ne!(first.id(), restored.id());
}

#[test]
fn structural_diff_skips_equal_subtrees_without_reading_them_and_reports_add_delete() {
    let mut store = Memory::default();
    let shared = ContentId::Tree(TreeId::from_bytes([42; 32])); // Intentionally unavailable.
    let old = ContentId::Blob(BlobId::from_bytes([1; 32]));
    let new = ContentId::Blob(BlobId::from_bytes([2; 32]));
    let before = store.root(map(vec![
        entry("shared", shared),
        entry("removed", old),
        entry("edit", old),
    ]));
    let after = store.root(map(vec![
        entry("shared", shared),
        entry("added", new),
        entry("edit", new),
    ]));
    assert!(
        diff(&mut store, before, before, TraversalLimits::default())
            .unwrap()
            .is_empty()
    );
    assert!(store.reads.is_empty());
    let changes = diff(&mut store, before, after, TraversalLimits::default()).unwrap();
    assert_eq!(
        store.reads,
        vec![ObjectId::Tree(before), ObjectId::Tree(after)]
    );
    assert_eq!(
        changes,
        vec![
            Change {
                path: vec!["added".into()],
                before: None,
                after: Some(new)
            },
            Change {
                path: vec!["edit".into()],
                before: Some(old),
                after: Some(new)
            },
            Change {
                path: vec!["removed".into()],
                before: Some(old),
                after: None
            },
        ]
    );
    assert!(matches!(
        restore(&mut store, before, TraversalLimits::default()),
        Err(Error::MissingObject(_))
    ));
}

#[test]
fn sequence_order_and_tree_mode_changes_are_not_hidden_by_equal_children() {
    let mut store = Memory::default();
    let blob = ContentId::Blob(BlobId::from_bytes([1; 32]));
    let entries = vec![entry("a", blob), entry("b", blob)];
    let before = store.root(Object::Tree(Tree::sequence(entries.clone()).unwrap()));
    let after = store.root(Object::Tree(
        Tree::sequence(entries.iter().rev().cloned().collect()).unwrap(),
    ));
    let mapped = store.root(map(entries));
    for target in [after, mapped] {
        assert_eq!(
            diff(&mut store, before, target, TraversalLimits::default()).unwrap(),
            vec![Change {
                path: vec![],
                before: Some(ContentId::Tree(before)),
                after: Some(ContentId::Tree(target))
            }]
        );
    }
}

#[test]
fn replacement_uses_opaque_keys_and_preserves_sequence_positions() {
    let target = ContentId::Blob(BlobId::from_bytes([0; 32]));
    let other = ContentId::Blob(BlobId::from_bytes([1; 32]));
    let sequence = Tree::sequence(vec![entry("a/b", target), entry("", target)]).unwrap();
    let replaced = sequence.replace("a/b", Some(other)).unwrap();
    assert_eq!(
        replaced.entries(),
        &[entry("a/b", other), entry("", target)]
    );
    let appended = replaced.replace("new", Some(target)).unwrap();
    assert_eq!(appended.entries().last().unwrap().key, "new");
    assert_eq!(
        appended.replace("", None).unwrap().entries(),
        &[entry("a/b", other), entry("new", target)]
    );
    assert!(matches!(
        sequence.replace("missing", None),
        Err(Error::MissingKey(_))
    ));
    let mut store = Memory::default();
    let root = store.root(Object::Tree(sequence));
    assert!(
        replace_path(
            &mut store,
            root,
            &["a/b"],
            Some(other),
            TraversalLimits::default()
        )
        .is_ok()
    );
    assert_eq!(
        replace_path(&mut store, root, &[], None, TraversalLimits::default()),
        Err(Error::EmptyPath)
    );
    assert_eq!(
        replace_path(
            &mut store,
            root,
            &["a/b", "nested"],
            None,
            TraversalLimits::default()
        ),
        Err(Error::NotTree)
    );
    assert!(matches!(
        replace_path(
            &mut store,
            root,
            &["missing", "nested"],
            None,
            TraversalLimits::default()
        ),
        Err(Error::MissingKey(_))
    ));
}

#[test]
fn restoration_rejects_corruption_and_resource_budget_exhaustion() {
    let mut store = Memory::default();
    let blob = store.put(Object::Blob(b"full bytes".to_vec()));
    let root = store.root(map(vec![entry("leaf", blob)]));
    for limits in [
        TraversalLimits {
            max_depth: 0,
            ..TraversalLimits::default()
        },
        TraversalLimits {
            max_objects: 1,
            ..TraversalLimits::default()
        },
        TraversalLimits {
            max_bytes: 1,
            ..TraversalLimits::default()
        },
    ] {
        assert!(matches!(
            restore(&mut store, root, limits),
            Err(Error::LimitExceeded(_))
        ));
    }
    store
        .objects
        .insert(blob.into(), Object::Blob(b"tampered".to_vec()).encode());
    assert_eq!(
        restore(&mut store, root, TraversalLimits::default()),
        Err(Error::HashMismatch)
    );
    let mut denied = |_: ObjectId| Err(Error::Read("unavailable in authorized scope".into()));
    assert!(matches!(
        restore(&mut denied, root, TraversalLimits::default()),
        Err(Error::Read(_))
    ));
}

#[test]
fn path_addition_and_deletion_reuse_fulltext_without_mutating_old_roots() {
    let mut store = Memory::default();
    let blob = store.put(Object::Blob(b"complete source".to_vec()));
    let old = store.root(map(vec![entry("old", blob)]));
    let added = replace_path(
        &mut store,
        old,
        &["added"],
        Some(blob),
        TraversalLimits::default(),
    )
    .unwrap();
    for object in added.objects {
        store.put(object);
    }
    let deleted = replace_path(
        &mut store,
        added.root,
        &["old"],
        None,
        TraversalLimits::default(),
    )
    .unwrap();
    for object in deleted.objects {
        store.put(object);
    }
    assert_eq!(
        restore(&mut store, old, TraversalLimits::default()).unwrap(),
        restored_map(vec![restored_entry(
            "old",
            RestoredContent::Blob(b"complete source".to_vec())
        )])
    );
    assert_eq!(
        restore(&mut store, deleted.root, TraversalLimits::default()).unwrap(),
        restored_map(vec![restored_entry(
            "added",
            RestoredContent::Blob(b"complete source".to_vec())
        )])
    );
    assert_eq!(
        diff(&mut store, old, deleted.root, TraversalLimits::default()).unwrap(),
        vec![
            Change {
                path: vec!["added".into()],
                before: None,
                after: Some(blob)
            },
            Change {
                path: vec!["old".into()],
                before: Some(blob),
                after: None
            },
        ]
    );
}

#[test]
fn recursion_is_bounded_even_if_caller_requests_unlimited_depth() {
    let mut store = Memory::default();
    let mut root = store.root(map(vec![]));
    for _ in 0..258 {
        root = store.root(map(vec![entry("nested", ContentId::Tree(root))]));
    }
    let limits = TraversalLimits {
        max_depth: usize::MAX,
        ..TraversalLimits::default()
    };
    assert_eq!(
        restore(&mut store, root, limits),
        Err(Error::LimitExceeded("depth"))
    );
    let path = vec!["nested"; 258];
    assert_eq!(
        replace_path(&mut store, root, &path, None, limits),
        Err(Error::LimitExceeded("depth"))
    );
}
