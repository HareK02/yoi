use content_history::{ContentId, Entry, History, Object, ObjectId, Tree};

pub fn objects() -> Vec<Object> {
    let empty = Object::Blob(Vec::new());
    let text = Object::Blob(b"a\r\n\xc3\xa9\0".to_vec());
    let ObjectId::Blob(empty_id) = empty.id() else {
        unreachable!()
    };
    let ObjectId::Blob(text_id) = text.id() else {
        unreachable!()
    };
    let entries = vec![
        Entry {
            key: "é".into(),
            target: ContentId::Blob(text_id),
        },
        Entry {
            key: "z".into(),
            target: ContentId::Blob(empty_id),
        },
    ];
    let map = Object::Tree(Tree::map(entries.clone()).unwrap());
    let sequence = Object::Tree(Tree::sequence(entries).unwrap());
    let ObjectId::Tree(root) = map.id() else {
        unreachable!()
    };
    let first = Object::History(History {
        root,
        parent: None,
        actor: "user:編集".into(),
        timestamp_micros: -1,
    });
    let ObjectId::History(parent) = first.id() else {
        unreachable!()
    };
    let returned = Object::History(History {
        root,
        parent: Some(parent),
        actor: "user:編集".into(),
        timestamp_micros: 0,
    });
    vec![
        empty,
        text,
        Object::Tree(Tree::map(vec![]).unwrap()),
        Object::Tree(Tree::sequence(vec![]).unwrap()),
        map,
        sequence,
        first,
        returned,
        Object::Tree(
            Tree::map(vec![Entry {
                key: "nested".into(),
                target: ContentId::Tree(root),
            }])
            .unwrap(),
        ),
    ]
}

pub fn digest_bytes(object: &Object) -> [u8; 32] {
    match object.id() {
        ObjectId::Blob(id) => *id.as_bytes(),
        ObjectId::Tree(id) => *id.as_bytes(),
        ObjectId::History(id) => *id.as_bytes(),
    }
}

pub fn parity_word(index: u32) -> u32 {
    let objects = objects();
    let Some(object) = objects.get(index as usize / 8) else {
        return u32::MAX;
    };
    let hash = digest_bytes(object);
    let offset = (index as usize % 8) * 4;
    u32::from_be_bytes(hash[offset..offset + 4].try_into().unwrap())
}
