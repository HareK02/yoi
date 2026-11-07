use std::collections::BTreeMap;

use config_source::{ConfigContentType, ConfigEntry, ConfigTreeSnapshot, VirtualPath};
use content_history::{ContentId, Entry, History, Object, ObjectId, Tree, TreeId};
use content_history_adapters::{Error, Snapshot, settings, ticket};

fn source(path: &str, kind: ConfigContentType, text: &str) -> ConfigEntry {
    ConfigEntry::new(VirtualPath::parse(path).unwrap(), kind, text).unwrap()
}

fn config() -> ConfigTreeSnapshot {
    ConfigTreeSnapshot::from_entries([
        source(
            "main.dcdl",
            ConfigContentType::Decodal,
            "// comment\r\n{ title: \"雪\" }\r\n",
        ),
        // Explicit metadata is intentionally independent of the filename extension.
        source(
            "nested/prompt.dcdl",
            ConfigContentType::Text,
            "# Prompt\r\n<!-- keep -->\r\n\r\n",
        ),
        source("nested/深い/empty.txt", ConfigContentType::Text, ""),
    ])
    .unwrap()
}

fn document() -> ticket::Document {
    ticket::Document {
        ticket_id: "T-835".into(),
        title: "設定の履歴".into(),
        initial_author: "user:original".into(),
        initial_body: "<!-- keep -->\r\nbody\r\n".into(),
        posts: vec![
            ticket::Post {
                id: "post-20".into(),
                author: "user:alice".into(),
                body: "first\r\n".into(),
            },
            ticket::Post {
                id: "post-03".into(),
                author: "user:bob".into(),
                body: "次\n".into(),
            },
        ],
    }
}

#[test]
fn version_one_fixed_root_and_full_source_fixtures() {
    // Constants pin the adapter schema as well as the independent model's canonical encoding.
    let config = settings::prepare(&config()).unwrap();
    assert_eq!(
        config.root.to_string(),
        "cbd69a2f456e9c854d24c04725ac400de76d17101d3d1b21b19190d3f24284c9"
    );
    let ticket = ticket::prepare(&document()).unwrap();
    assert_eq!(
        ticket.root.to_string(),
        "12dfd2e11581ea17e797c29b476a2b03ad6c0561cbccb611123f37be0771ff37"
    );
    let ContentId::Blob(source) = target(
        tree(&config, child(&config, config.root, "files")),
        "main.dcdl",
    ) else {
        unreachable!()
    };
    assert_eq!(
        source.to_string(),
        "02e6cfe23152f0576d14a0806717168e544a7abb2309e64354f6bb4e62ee81bb"
    );
    assert_eq!(
        config.get(ObjectId::Blob(source)).unwrap(),
        Object::Blob("// comment\r\n{ title: \"雪\" }\r\n".as_bytes().to_vec())
    );
}

#[test]
fn config_explicit_type_change_changes_only_metadata_branch() {
    let original = config();
    let old = settings::prepare(&original).unwrap();
    let mut entries = original.entries.values().cloned().collect::<Vec<_>>();
    entries[0].content_type = ConfigContentType::Text;
    let candidate = ConfigTreeSnapshot::from_entries(entries).unwrap();
    let new = settings::prepare(&candidate).unwrap();
    assert_ne!(old.root, new.root);
    assert_eq!(
        child(&old, old.root, "files"),
        child(&new, new.root, "files")
    );
    assert_eq!(
        settings::restore(new.root, |id| new.get(id)).unwrap(),
        candidate
    );
}

#[test]
fn config_restore_rejects_missing_surplus_metadata_and_empty_directories() {
    let original = settings::prepare(&config()).unwrap();
    let mut invalid = original.clone();
    let empty = Object::Tree(Tree::map(Vec::new()).unwrap());
    let ObjectId::Tree(empty) = add(&mut invalid, empty) else {
        unreachable!()
    };
    invalid.root = change_field(&mut invalid, original.root, "types", ContentId::Tree(empty));
    assert!(settings::restore(invalid.root, |id| invalid.get(id)).is_err());
    invalid.root = change_field(&mut invalid, original.root, "files", ContentId::Tree(empty));
    assert!(settings::restore(invalid.root, |id| invalid.get(id)).is_err());

    // An empty non-root directory is not representable by ConfigTreeSnapshot.
    let ObjectId::Tree(files) = add(
        &mut invalid,
        Object::Tree(
            Tree::map(vec![Entry {
                key: "empty".into(),
                target: ContentId::Tree(empty),
            }])
            .unwrap(),
        ),
    ) else {
        unreachable!()
    };
    invalid.root = change_field(&mut invalid, original.root, "files", ContentId::Tree(files));
    assert!(settings::restore(invalid.root, |id| invalid.get(id)).is_err());
}

#[test]
fn raw_reader_surfaces_unsupported_model_formats_without_partial_restore() {
    let snapshot = ticket::prepare(&document()).unwrap();
    let result = ticket::restore(snapshot.root, "T-835", |id| {
        let mut bytes = snapshot.get(id)?.encode();
        bytes[5] = 99;
        Ok(Object::decode(&bytes)?)
    });
    assert!(matches!(result, Err(Error::Model(_))));
}

fn tree(snapshot: &Snapshot, id: TreeId) -> &Tree {
    let Object::Tree(tree) = &snapshot.objects[&ObjectId::Tree(id)] else {
        panic!("expected tree")
    };
    tree
}

fn target(tree: &Tree, key: &str) -> ContentId {
    tree.entries().iter().find(|e| e.key == key).unwrap().target
}

fn child(snapshot: &Snapshot, id: TreeId, key: &str) -> TreeId {
    let ContentId::Tree(id) = target(tree(snapshot, id), key) else {
        panic!("expected tree child")
    };
    id
}

fn add(snapshot: &mut Snapshot, object: Object) -> ObjectId {
    let id = object.id();
    snapshot.objects.insert(id, object);
    id
}

fn change_field(snapshot: &mut Snapshot, id: TreeId, key: &str, replacement: ContentId) -> TreeId {
    let entries = tree(snapshot, id)
        .entries()
        .iter()
        .map(|item| Entry {
            key: item.key.clone(),
            target: if item.key == key {
                replacement
            } else {
                item.target
            },
        })
        .collect();
    let ObjectId::Tree(id) = add(snapshot, Object::Tree(Tree::map(entries).unwrap())) else {
        unreachable!()
    };
    id
}

fn blob(snapshot: &mut Snapshot, bytes: &[u8]) -> ContentId {
    let ObjectId::Blob(id) = add(snapshot, Object::Blob(bytes.to_vec())) else {
        unreachable!()
    };
    ContentId::Blob(id)
}

#[test]
fn config_roundtrip_preserves_sources_paths_types_and_metadata() {
    let original = config();
    let snapshot = settings::prepare(&original).unwrap();
    let restored = settings::restore(snapshot.root, |id| snapshot.get(id)).unwrap();
    assert_eq!(original, restored);
    // Directory tree uses actual names, not a flat list of full paths.
    let files = child(&snapshot, snapshot.root, "files");
    assert_eq!(
        tree(&snapshot, files)
            .entries()
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        ["main.dcdl", "nested"]
    );
    let nested = child(&snapshot, files, "nested");
    assert!(matches!(
        target(tree(&snapshot, nested), "prompt.dcdl"),
        ContentId::Blob(_)
    ));
    let deep = child(&snapshot, nested, "深い");
    assert!(matches!(
        target(tree(&snapshot, deep), "empty.txt"),
        ContentId::Blob(_)
    ));
    // Reopening canonical bytes does not reserialize domain sources or alter root identity.
    let reopened = snapshot
        .objects
        .iter()
        .map(|(id, object)| (*id, Object::decode(&object.encode()).unwrap()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        settings::restore(snapshot.root, |id| reopened
            .get(&id)
            .cloned()
            .ok_or(Error::Missing(id)))
        .unwrap(),
        original
    );
    let reverse =
        ConfigTreeSnapshot::from_entries(original.entries.values().rev().cloned()).unwrap();
    assert_eq!(snapshot.root, settings::prepare(&reverse).unwrap().root);
}

#[test]
fn config_edits_names_types_additions_and_deletions_change_root_share_unchanged_blobs() {
    let original = config();
    let old = settings::prepare(&original).unwrap();
    for entries in [
        vec![source("main.dcdl", ConfigContentType::Decodal, "changed\n")],
        vec![source(
            "renamed.dcdl",
            ConfigContentType::Decodal,
            "// comment\r\n{ title: \"雪\" }\r\n",
        )],
        vec![source(
            "main.dcdl",
            ConfigContentType::Text,
            "// comment\r\n{ title: \"雪\" }\r\n",
        )],
    ] {
        assert_ne!(
            old.root,
            settings::prepare(&ConfigTreeSnapshot::from_entries(entries).unwrap())
                .unwrap()
                .root
        );
    }
    let mut edited = original.entries.values().cloned().collect::<Vec<_>>();
    edited[0] = source("main.dcdl", ConfigContentType::Decodal, "new\r\n");
    edited.push(source("extra.txt", ConfigContentType::Text, "added"));
    let new = settings::prepare(&ConfigTreeSnapshot::from_entries(edited).unwrap()).unwrap();
    let files_old = child(&old, old.root, "files");
    let files_new = child(&new, new.root, "files");
    assert_eq!(
        target(tree(&old, files_old), "nested"),
        target(tree(&new, files_new), "nested")
    );
    let mut union = old.objects.clone();
    union.extend(new.objects);
    assert_eq!(
        settings::restore(old.root, |id| union
            .get(&id)
            .cloned()
            .ok_or(Error::Missing(id)))
        .unwrap(),
        original
    );
    let empty = settings::prepare(&ConfigTreeSnapshot::empty()).unwrap();
    assert_ne!(old.root, empty.root);
    assert_eq!(
        settings::restore(empty.root, |id| empty.get(id)).unwrap(),
        ConfigTreeSnapshot::empty()
    );
}

#[test]
fn config_rejects_inconsistent_snapshot_and_file_directory_conflicts() {
    let mut invalid = config();
    invalid.digest.push('0');
    assert!(matches!(settings::prepare(&invalid), Err(Error::Config(_))));
    let mut invalid = config();
    invalid
        .entries
        .values_mut()
        .next()
        .unwrap()
        .content
        .push('!');
    assert!(matches!(settings::prepare(&invalid), Err(Error::Config(_))));
    let mut invalid = config();
    invalid.entries.values_mut().next().unwrap().path = VirtualPath::parse("other.dcdl").unwrap();
    assert!(matches!(settings::prepare(&invalid), Err(Error::Config(_))));
    let prefix_conflict = ConfigTreeSnapshot::from_entries([
        source("a", ConfigContentType::Text, "file"),
        source("a/b", ConfigContentType::Text, "nested"),
    ])
    .unwrap();
    assert!(matches!(
        settings::prepare(&prefix_conflict),
        Err(Error::PathConflict(_))
    ));
}

#[test]
fn config_restore_rejects_invalid_utf8_metadata_paths_and_ordering() {
    let original = settings::prepare(&config()).unwrap();
    let mut invalid = original.clone();
    let files = child(&invalid, invalid.root, "files");
    let bad = blob(&mut invalid, &[0xff]);
    let files = change_field(&mut invalid, files, "main.dcdl", bad);
    invalid.root = change_field(&mut invalid, original.root, "files", ContentId::Tree(files));
    assert!(matches!(
        settings::restore(invalid.root, |id| invalid.get(id)),
        Err(Error::Utf8(_))
    ));

    let mut invalid = original.clone();
    let types = child(&invalid, invalid.root, "types");
    let bad = blob(&mut invalid, b"unknown");
    let types = change_field(&mut invalid, types, "main.dcdl", bad);
    invalid.root = change_field(&mut invalid, original.root, "types", ContentId::Tree(types));
    assert!(settings::restore(invalid.root, |id| invalid.get(id)).is_err());

    for key in ["../escape", "a/b", "$builtin", ""] {
        let mut invalid = original.clone();
        let value = blob(&mut invalid, b"bad");
        // Empty keys may already be rejected by the independent model.
        let Ok(files) = Tree::map(vec![Entry {
            key: key.into(),
            target: value,
        }]) else {
            continue;
        };
        let ObjectId::Tree(files) = add(&mut invalid, Object::Tree(files)) else {
            unreachable!()
        };
        invalid.root = change_field(&mut invalid, original.root, "files", ContentId::Tree(files));
        assert!(
            settings::restore(invalid.root, |id| invalid.get(id)).is_err(),
            "{key}"
        );
    }
    let mut invalid = original.clone();
    let files = child(&invalid, invalid.root, "files");
    let ordered = Tree::sequence(tree(&invalid, files).entries().to_vec()).unwrap();
    let ObjectId::Tree(files) = add(&mut invalid, Object::Tree(ordered)) else {
        unreachable!()
    };
    invalid.root = change_field(&mut invalid, original.root, "files", ContentId::Tree(files));
    assert!(settings::restore(invalid.root, |id| invalid.get(id)).is_err());
}

#[test]
fn ticket_roundtrip_explicit_structure_and_semantic_order() {
    let original = document();
    let snapshot = ticket::prepare(&original).unwrap();
    assert_eq!(
        ticket::restore(snapshot.root, "T-835", |id| snapshot.get(id)).unwrap(),
        original
    );
    assert_eq!(
        tree(&snapshot, snapshot.root)
            .entries()
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        ["format", "initial", "posts", "ticket-id", "title"]
    );
    let posts = child(&snapshot, snapshot.root, "posts");
    assert!(tree(&snapshot, posts).is_ordered());
    assert_eq!(
        tree(&snapshot, posts)
            .entries()
            .iter()
            .map(|e| e.key.as_str())
            .collect::<Vec<_>>(),
        ["post-20", "post-03"]
    );
    let mut swapped = original.clone();
    swapped.posts.swap(0, 1);
    assert_ne!(snapshot.root, ticket::prepare(&swapped).unwrap().root);
    let mut other = original.clone();
    other.ticket_id = "T-836".into();
    assert_ne!(snapshot.root, ticket::prepare(&other).unwrap().root);
    assert!(matches!(
        ticket::restore(snapshot.root, "T-836", |id| snapshot.get(id)),
        Err(Error::TicketIdentity)
    ));
    let mut edited = original;
    edited.posts[0].body = "edited".into();
    let restored = ticket::restore(ticket::prepare(&edited).unwrap().root, "T-835", |id| {
        ticket::prepare(&edited).unwrap().get(id)
    })
    .unwrap();
    assert_eq!(restored.posts[0].id, "post-20");
    assert_eq!(restored.posts[0].author, "user:alice");
}

#[test]
fn restore_old_document_leaves_separate_business_events_and_original_authors_untouched() {
    #[derive(Debug, PartialEq)]
    struct BusinessState {
        state: &'static str,
        assignment: &'static str,
        events: Vec<&'static str>,
    }
    struct Ticket {
        document: ticket::Document,
        business: BusinessState,
    }
    let original = document();
    let old = ticket::prepare(&original).unwrap();
    let mut current = Ticket {
        document: original.clone(),
        business: BusinessState {
            state: "closed",
            assignment: "worker:new",
            events: vec!["relation added", "evidence attached"],
        },
    };
    current.document.title = "changed".into();
    current.document =
        ticket::restore(old.root, &current.document.ticket_id, |id| old.get(id)).unwrap();
    assert_eq!(current.document, original);
    assert_eq!(
        current.business,
        BusinessState {
            state: "closed",
            assignment: "worker:new",
            events: vec!["relation added", "evidence attached"]
        }
    );
    let first_edit = History {
        root: old.root,
        parent: None,
        actor: "user:editor".into(),
        timestamp_micros: 1,
    };
    let ObjectId::History(first_id) = Object::History(first_edit.clone()).id() else {
        unreachable!()
    };
    let restore_edit = History {
        root: old.root,
        parent: Some(first_id),
        actor: "user:restorer".into(),
        timestamp_micros: 2,
    };
    assert_ne!(
        Object::History(first_edit).id(),
        Object::History(restore_edit).id()
    );
    assert_eq!(current.document.posts[0].author, "user:alice");
}

#[test]
fn readers_fail_closed_on_missing_corruption_wrong_kind_and_unknown_adapter_version() {
    let snapshot = ticket::prepare(&document()).unwrap();
    for id in snapshot.objects.keys() {
        assert!(
            ticket::restore(snapshot.root, "T-835", |requested| {
                if requested == *id {
                    Err(Error::Missing(requested))
                } else {
                    snapshot.get(requested)
                }
            })
            .is_err()
        );
    }
    assert!(matches!(
        ticket::restore(snapshot.root, "T-835", |_| Ok(Object::Blob(
            b"tampered".to_vec()
        ))),
        Err(Error::HashMismatch(_))
    ));
    let mut invalid = snapshot.clone();
    let bad = blob(&mut invalid, b"ticket-document/v999");
    invalid.root = change_field(&mut invalid, snapshot.root, "format", bad);
    assert!(matches!(
        ticket::restore(invalid.root, "T-835", |id| invalid.get(id)),
        Err(Error::Format)
    ));
    let bad = blob(&mut invalid, b"not a post sequence");
    invalid.root = change_field(&mut invalid, snapshot.root, "posts", bad);
    assert!(ticket::restore(invalid.root, "T-835", |id| invalid.get(id)).is_err());
    let posts = child(&snapshot, snapshot.root, "posts");
    let unordered = Tree::map(tree(&snapshot, posts).entries().to_vec()).unwrap();
    let ObjectId::Tree(posts) = add(&mut invalid, Object::Tree(unordered)) else {
        unreachable!()
    };
    invalid.root = change_field(&mut invalid, snapshot.root, "posts", ContentId::Tree(posts));
    assert!(ticket::restore(invalid.root, "T-835", |id| invalid.get(id)).is_err());
}

#[test]
fn duplicate_business_post_ids_and_empty_ticket_ids_are_not_accepted() {
    let mut invalid = document();
    invalid.posts[1].id = invalid.posts[0].id.clone();
    assert!(ticket::prepare(&invalid).is_err());
    invalid = document();
    invalid.ticket_id.clear();
    assert!(ticket::prepare(&invalid).is_err());
}
