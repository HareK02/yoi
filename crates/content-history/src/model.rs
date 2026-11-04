use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, fmt};

macro_rules! identifier {
    ($name:ident) => {
        /// SHA-256 identity of canonical versioned bytes, not an access capability.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name([u8; 32]);
        impl $name {
            pub const fn from_bytes(bytes: [u8; 32]) -> Self {
                Self(bytes)
            }
            pub const fn as_bytes(&self) -> &[u8; 32] {
                &self.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                for byte in self.0 {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({self})", stringify!($name))
            }
        }
    };
}
identifier!(BlobId);
identifier!(TreeId);
identifier!(HistoryId);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ContentId {
    Blob(BlobId),
    Tree(TreeId),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectId {
    Blob(BlobId),
    Tree(TreeId),
    History(HistoryId),
}
impl From<ContentId> for ObjectId {
    fn from(id: ContentId) -> Self {
        match id {
            ContentId::Blob(id) => Self::Blob(id),
            ContentId::Tree(id) => Self::Tree(id),
        }
    }
}

/// Failure is explicit: malformed or incomplete snapshots are never partial successes.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid object magic")]
    InvalidMagic,
    #[error("unsupported format version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid object tag {0}")]
    InvalidTag(u8),
    #[error("invalid child kind {0}")]
    InvalidChildKind(u8),
    #[error("invalid tree mode {0}")]
    InvalidTreeMode(u8),
    #[error("invalid parent flag {0}")]
    InvalidParentFlag(u8),
    #[error("truncated object")]
    Truncated,
    #[error("length does not fit remaining bytes or platform")]
    InvalidLength,
    #[error("invalid UTF-8")]
    InvalidUtf8,
    #[error("trailing bytes")]
    TrailingBytes,
    #[error("duplicate tree key {0:?}")]
    DuplicateKey(String),
    #[error("map entries are not in strict UTF-8 byte order")]
    NonCanonicalMap,
    #[error("object kind or hash does not match requested identity")]
    HashMismatch,
    #[error("missing object {0:?}")]
    MissingObject(ObjectId),
    #[error("reader failed: {0}")]
    Read(String),
    #[error("missing path key {0:?}")]
    MissingKey(String),
    #[error("path component is not a tree")]
    NotTree,
    #[error("replacement path must not be empty")]
    EmptyPath,
    #[error("traversal exceeds {0} limit")]
    LimitExceeded(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub target: ContentId,
}

/// Maps sort by exact UTF-8 bytes; sequences retain semantic order. Both have unique keys.
/// Empty strings, slashes and Unicode are opaque keys, not normalized virtual paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    ordered: bool,
    entries: Vec<Entry>,
}
impl Tree {
    pub fn map(mut entries: Vec<Entry>) -> Result<Self, Error> {
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Self::build(entries, false)
    }
    pub fn sequence(entries: Vec<Entry>) -> Result<Self, Error> {
        Self::build(entries, true)
    }
    fn build(entries: Vec<Entry>, ordered: bool) -> Result<Self, Error> {
        let mut keys = BTreeSet::new();
        for entry in &entries {
            if !keys.insert(&entry.key) {
                return Err(Error::DuplicateKey(entry.key.clone()));
            }
        }
        Ok(Self { ordered, entries })
    }
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
    pub fn is_ordered(&self) -> bool {
        self.ordered
    }
    /// Replace in place, append new sequence keys, or delete an existing key.
    /// Deleting a missing key is an error; this never mutates the old tree.
    pub fn replace(&self, key: &str, replacement: Option<ContentId>) -> Result<Self, Error> {
        let mut entries = self.entries.clone();
        match (entries.iter().position(|e| e.key == key), replacement) {
            (Some(index), Some(target)) => entries[index].target = target,
            (Some(index), None) => {
                entries.remove(index);
            }
            (None, Some(target)) => entries.push(Entry {
                key: key.into(),
                target,
            }),
            (None, None) => return Err(Error::MissingKey(key.into())),
        }
        if self.ordered {
            Self::sequence(entries)
        } else {
            Self::map(entries)
        }
    }
}

/// Caller-authenticated editor/time facts. Restoring old content creates a new record
/// whose parent is the current head; this type does not implement adoption or head CAS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct History {
    pub root: TreeId,
    pub parent: Option<HistoryId>,
    pub actor: String,
    pub timestamp_micros: i64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Object {
    Blob(Vec<u8>),
    Tree(Tree),
    History(History),
}

impl Object {
    /// Canonical format v1. See the crate documentation for the compatibility boundary.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"YCH\0\0\x01".to_vec();
        match self {
            Self::Blob(bytes) => {
                out.push(1);
                put_bytes(&mut out, bytes);
            }
            Self::Tree(tree) => {
                out.push(2);
                out.push(u8::from(tree.ordered));
                put_u64(&mut out, tree.entries.len());
                for entry in &tree.entries {
                    put_bytes(&mut out, entry.key.as_bytes());
                    match entry.target {
                        ContentId::Blob(id) => {
                            out.push(1);
                            out.extend_from_slice(id.as_bytes());
                        }
                        ContentId::Tree(id) => {
                            out.push(2);
                            out.extend_from_slice(id.as_bytes());
                        }
                    }
                }
            }
            Self::History(history) => {
                out.push(3);
                out.extend_from_slice(history.root.as_bytes());
                match history.parent {
                    Some(id) => {
                        out.push(1);
                        out.extend_from_slice(id.as_bytes());
                    }
                    None => out.push(0),
                }
                put_bytes(&mut out, history.actor.as_bytes());
                out.extend_from_slice(&history.timestamp_micros.to_be_bytes());
            }
        }
        out
    }
    /// Strict decoding rejects unknown versions, duplicate/unsorted maps and trailing bytes.
    /// Lengths are checked against input before allocation; no input-driven capacity reservation.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut input = Input(bytes);
        if input.take(4)? != b"YCH\0" {
            return Err(Error::InvalidMagic);
        }
        let version = u16::from_be_bytes(input.array()?);
        if version != 1 {
            return Err(Error::UnsupportedVersion(version));
        }
        let object = match input.byte()? {
            1 => Self::Blob(input.bytes()?.to_vec()),
            2 => {
                let ordered = match input.byte()? {
                    0 => false,
                    1 => true,
                    mode => return Err(Error::InvalidTreeMode(mode)),
                };
                let count = input.length()?;
                // Each entry needs a u64 key length, a kind and 32 digest bytes.
                if count > input.0.len() / 41 {
                    return Err(Error::InvalidLength);
                }
                let mut entries: Vec<Entry> = Vec::new();
                for _ in 0..count {
                    let key = input.string()?;
                    if !ordered && entries.last().is_some_and(|last| last.key >= key) {
                        return Err(Error::NonCanonicalMap);
                    }
                    let kind = input.byte()?;
                    let target = match kind {
                        1 => ContentId::Blob(BlobId(input.array()?)),
                        2 => ContentId::Tree(TreeId(input.array()?)),
                        _ => return Err(Error::InvalidChildKind(kind)),
                    };
                    entries.push(Entry { key, target });
                }
                Self::Tree(Tree::build(entries, ordered)?)
            }
            3 => {
                let root = TreeId(input.array()?);
                let parent = match input.byte()? {
                    0 => None,
                    1 => Some(HistoryId(input.array()?)),
                    flag => return Err(Error::InvalidParentFlag(flag)),
                };
                let actor = input.string()?;
                let timestamp_micros = i64::from_be_bytes(input.array()?);
                Self::History(History {
                    root,
                    parent,
                    actor,
                    timestamp_micros,
                })
            }
            tag => return Err(Error::InvalidTag(tag)),
        };
        if !input.0.is_empty() {
            return Err(Error::TrailingBytes);
        }
        Ok(object)
    }
    pub fn id(&self) -> ObjectId {
        let hash: [u8; 32] = Sha256::digest(self.encode()).into();
        match self {
            Self::Blob(_) => ObjectId::Blob(BlobId(hash)),
            Self::Tree(_) => ObjectId::Tree(TreeId(hash)),
            Self::History(_) => ObjectId::History(HistoryId(hash)),
        }
    }
}

/// Validate exact stored bytes and requested kind before returning any content.
/// Decoding is intentionally first, so unsupported formats remain diagnosable.
pub fn verify(id: ObjectId, bytes: &[u8]) -> Result<Object, Error> {
    let object = Object::decode(bytes)?;
    // Hash the stored preimage, not a reserialization under the current writer's format.
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    let actual = match &object {
        Object::Blob(_) => ObjectId::Blob(BlobId(hash)),
        Object::Tree(_) => ObjectId::Tree(TreeId(hash)),
        Object::History(_) => ObjectId::History(HistoryId(hash)),
    };
    if actual != id {
        return Err(Error::HashMismatch);
    }
    Ok(object)
}
fn put_u64(out: &mut Vec<u8>, length: usize) {
    out.extend_from_slice(&(length as u64).to_be_bytes());
}
fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    put_u64(out, bytes.len());
    out.extend_from_slice(bytes);
}
struct Input<'a>(&'a [u8]);
impl<'a> Input<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.0.len() {
            return Err(Error::Truncated);
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        Ok(self.take(N)?.try_into().expect("checked slice length"))
    }
    fn byte(&mut self) -> Result<u8, Error> {
        Ok(self.array::<1>()?[0])
    }
    fn length(&mut self) -> Result<usize, Error> {
        usize::try_from(u64::from_be_bytes(self.array()?)).map_err(|_| Error::InvalidLength)
    }
    fn bytes(&mut self) -> Result<&'a [u8], Error> {
        let length = self.length()?;
        if length > self.0.len() {
            return Err(Error::InvalidLength);
        }
        self.take(length)
    }
    fn string(&mut self) -> Result<String, Error> {
        std::str::from_utf8(self.bytes()?)
            .map(str::to_owned)
            .map_err(|_| Error::InvalidUtf8)
    }
}
