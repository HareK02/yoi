#![doc = include_str!("../README.md")]

use std::collections::BTreeMap;

use content_history::{BlobId, ContentId, Entry, Object, ObjectId, Tree, TreeId};

pub mod settings;
pub mod ticket;

/// Complete immutable candidate for store preparation, not a saved or adopted version.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub root: TreeId,
    pub objects: BTreeMap<ObjectId, Object>,
}

impl Snapshot {
    /// In-memory reader for examples. Production readers must enforce authorization and erasure.
    pub fn get(&self, id: ObjectId) -> Result<Object, Error> {
        self.objects.get(&id).cloned().ok_or(Error::Missing(id))
    }
}

/// Restoration fails closed: no partial document is returned on any read or contract error.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Model(#[from] content_history::Error),
    #[error(transparent)]
    Config(#[from] config_source::ConfigTreeError),
    #[error("missing object: {0:?}")]
    Missing(ObjectId),
    #[error("reader returned an object not matching requested identity: {0:?}")]
    HashMismatch(ObjectId),
    #[error("invalid adapter structure: {0}")]
    Structure(&'static str),
    #[error("unsupported adapter format")]
    Format,
    #[error("content is not UTF-8")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("file/directory conflict: {0}")]
    PathConflict(String),
    #[error("Ticket identity does not match the restoration target")]
    TicketIdentity,
    #[error("authorized reader failed: {0}")]
    Read(String),
}

#[derive(Default)]
struct Builder {
    objects: BTreeMap<ObjectId, Object>,
}

impl Builder {
    fn blob(&mut self, bytes: impl AsRef<[u8]>) -> ContentId {
        let object = Object::Blob(bytes.as_ref().to_vec());
        let ObjectId::Blob(id) = object.id() else {
            unreachable!()
        };
        self.objects.insert(object.id(), object);
        ContentId::Blob(id)
    }

    fn tree(&mut self, tree: Tree) -> TreeId {
        let object = Object::Tree(tree);
        let ObjectId::Tree(id) = object.id() else {
            unreachable!()
        };
        self.objects.insert(object.id(), object);
        id
    }

    fn map(&mut self, entries: Vec<Entry>) -> Result<TreeId, Error> {
        Ok(self.tree(Tree::map(entries)?))
    }

    fn finish(self, root: TreeId) -> Snapshot {
        Snapshot {
            root,
            objects: self.objects,
        }
    }
}

fn entry(key: impl Into<String>, target: ContentId) -> Entry {
    Entry {
        key: key.into(),
        target,
    }
}

fn read_object(
    read: &mut impl FnMut(ObjectId) -> Result<Object, Error>,
    id: ObjectId,
) -> Result<Object, Error> {
    let object = read(id)?;
    if object.id() != id {
        return Err(Error::HashMismatch(id));
    }
    Ok(object)
}

fn read_tree(
    read: &mut impl FnMut(ObjectId) -> Result<Object, Error>,
    id: TreeId,
    ordered: bool,
) -> Result<Tree, Error> {
    let Object::Tree(tree) = read_object(read, ObjectId::Tree(id))? else {
        return Err(Error::Structure("expected tree"));
    };
    if tree.is_ordered() != ordered {
        return Err(Error::Structure("incorrect tree ordering"));
    }
    Ok(tree)
}

fn tree_id(target: ContentId) -> Result<TreeId, Error> {
    match target {
        ContentId::Tree(id) => Ok(id),
        _ => Err(Error::Structure("expected tree target")),
    }
}

fn blob_id(target: ContentId) -> Result<BlobId, Error> {
    match target {
        ContentId::Blob(id) => Ok(id),
        _ => Err(Error::Structure("expected blob target")),
    }
}

fn text(
    read: &mut impl FnMut(ObjectId) -> Result<Object, Error>,
    target: ContentId,
) -> Result<String, Error> {
    let Object::Blob(bytes) = read_object(read, ObjectId::Blob(blob_id(target)?))? else {
        return Err(Error::Structure("expected blob"));
    };
    Ok(String::from_utf8(bytes)?)
}

fn fields(tree: &Tree, expected: &[&str]) -> Result<(), Error> {
    if tree.is_ordered()
        || tree.entries().len() != expected.len()
        || expected
            .iter()
            .any(|key| !tree.entries().iter().any(|e| e.key == *key))
    {
        return Err(Error::Structure("unexpected fields"));
    }
    Ok(())
}

fn field(tree: &Tree, key: &str) -> Result<ContentId, Error> {
    tree.entries()
        .iter()
        .find(|e| e.key == key)
        .map(|e| e.target)
        .ok_or(Error::Structure("missing field"))
}
