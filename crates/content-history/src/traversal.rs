use crate::{ContentId, Error, Object, ObjectId, Tree, TreeId, verify};
use std::collections::BTreeMap;

/// Reader already bound to an authorized target and retention policy by its caller.
/// Hash possession is not authority. Revoked/erased objects must fail here even if
/// the physical bytes are deduplicated with another target. Missing data is an error.
/// The reader owns I/O size limits; traversal checks bytes after each returned buffer.
pub trait ObjectReader {
    fn read(&mut self, id: ObjectId) -> Result<Vec<u8>, Error>;
}
impl<F: FnMut(ObjectId) -> Result<Vec<u8>, Error>> ObjectReader for F {
    fn read(&mut self, id: ObjectId) -> Result<Vec<u8>, Error> {
        self(id)
    }
}

/// Work budgets count all reads (including repeated shared subtrees) and encoded bytes.
/// Root depth is zero. Depth is additionally capped at 256 to bound Rust stack use.
#[derive(Clone, Copy, Debug)]
pub struct TraversalLimits {
    pub max_depth: usize,
    pub max_objects: usize,
    pub max_bytes: usize,
}
impl Default for TraversalLimits {
    fn default() -> Self {
        Self {
            max_depth: 128,
            max_objects: 100_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}
struct Budget {
    limits: TraversalLimits,
    objects: usize,
    bytes: usize,
}
impl Budget {
    fn new(limits: TraversalLimits) -> Self {
        Self {
            limits,
            objects: 0,
            bytes: 0,
        }
    }
    fn depth(&self, depth: usize) -> Result<(), Error> {
        if depth > self.limits.max_depth.min(256) {
            return Err(Error::LimitExceeded("depth"));
        }
        Ok(())
    }
    fn read<R: ObjectReader + ?Sized>(
        &mut self,
        reader: &mut R,
        id: ObjectId,
        depth: usize,
    ) -> Result<Object, Error> {
        self.depth(depth)?;
        if self.objects >= self.limits.max_objects {
            return Err(Error::LimitExceeded("objects"));
        }
        self.objects += 1;
        let bytes = reader.read(id)?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(Error::LimitExceeded("bytes"))?;
        if self.bytes > self.limits.max_bytes {
            return Err(Error::LimitExceeded("bytes"));
        }
        verify(id, &bytes)
    }
    fn tree<R: ObjectReader + ?Sized>(
        &mut self,
        reader: &mut R,
        id: TreeId,
        depth: usize,
    ) -> Result<Tree, Error> {
        match self.read(reader, ObjectId::Tree(id), depth)? {
            Object::Tree(tree) => Ok(tree),
            _ => Err(Error::NotTree),
        }
    }
}

/// Fully materialized bytes and exact component order, not an applied business object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoredContent {
    Blob(Vec<u8>),
    Tree {
        ordered: bool,
        entries: Vec<RestoredEntry>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredEntry {
    pub key: String,
    pub content: RestoredContent,
}

/// Restore a complete content snapshot without reading or replaying history/diffs.
/// On failure no partial result is returned. This does not restore state, assignments,
/// authorization, or other business events, nor adopt a historical head.
pub fn restore<R: ObjectReader + ?Sized>(
    reader: &mut R,
    root: TreeId,
    limits: TraversalLimits,
) -> Result<RestoredContent, Error> {
    restore_content(reader, ContentId::Tree(root), &mut Budget::new(limits), 0)
}
fn restore_content<R: ObjectReader + ?Sized>(
    reader: &mut R,
    id: ContentId,
    budget: &mut Budget,
    depth: usize,
) -> Result<RestoredContent, Error> {
    match budget.read(reader, id.into(), depth)? {
        Object::Blob(bytes) => Ok(RestoredContent::Blob(bytes)),
        Object::Tree(tree) => {
            let mut entries = Vec::new();
            for entry in tree.entries() {
                entries.push(RestoredEntry {
                    key: entry.key.clone(),
                    content: restore_content(reader, entry.target, budget, depth + 1)?,
                });
            }
            Ok(RestoredContent::Tree {
                ordered: tree.is_ordered(),
                entries,
            })
        }
        Object::History(_) => unreachable!("verification enforces requested kind"),
    }
}

/// A replacement at an opaque key path. Empty path denotes the root itself.
/// Added/removed subtrees and tree mode/sequence layout changes are coarse replacements;
/// otherwise differences recurse by key. This is structural, not a text-line diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: Vec<String>,
    pub before: Option<ContentId>,
    pub after: Option<ContentId>,
}

/// Skip identical subtree hashes without even reading them. This is not a closure
/// integrity check: unchanged or added/removed subtrees are intentionally not loaded.
/// Use [`restore`] when complete snapshot integrity is needed before adoption.
/// Order is deterministic: map keys sort by UTF-8; stable-layout sequences follow order.
pub fn diff<R: ObjectReader + ?Sized>(
    reader: &mut R,
    before: TreeId,
    after: TreeId,
    limits: TraversalLimits,
) -> Result<Vec<Change>, Error> {
    let mut changes = Vec::new();
    diff_content(
        reader,
        ContentId::Tree(before),
        ContentId::Tree(after),
        &mut Vec::new(),
        &mut Budget::new(limits),
        &mut changes,
    )?;
    Ok(changes)
}
fn diff_content<R: ObjectReader + ?Sized>(
    reader: &mut R,
    before: ContentId,
    after: ContentId,
    path: &mut Vec<String>,
    budget: &mut Budget,
    changes: &mut Vec<Change>,
) -> Result<(), Error> {
    if before == after {
        return Ok(());
    }
    budget.depth(path.len())?;
    if let (ContentId::Tree(left), ContentId::Tree(right)) = (before, after) {
        let left = budget.tree(reader, left, path.len())?;
        let right = budget.tree(reader, right, path.len())?;
        let layout_changed = left.is_ordered() != right.is_ordered()
            || (left.is_ordered()
                && !left
                    .entries()
                    .iter()
                    .map(|e| &e.key)
                    .eq(right.entries().iter().map(|e| &e.key)));
        if !layout_changed {
            let left_by_key: BTreeMap<_, _> = left
                .entries()
                .iter()
                .map(|e| (e.key.as_str(), e.target))
                .collect();
            let right_by_key: BTreeMap<_, _> = right
                .entries()
                .iter()
                .map(|e| (e.key.as_str(), e.target))
                .collect();
            let keys: Vec<&str> = if left.is_ordered() {
                left.entries().iter().map(|e| e.key.as_str()).collect()
            } else {
                left_by_key
                    .keys()
                    .chain(right_by_key.keys())
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect()
            };
            for key in keys {
                path.push(key.into());
                let old = left_by_key.get(key).copied();
                let new = right_by_key.get(key).copied();
                match (old, new) {
                    (Some(old), Some(new)) => {
                        diff_content(reader, old, new, path, budget, changes)?
                    }
                    _ => changes.push(Change {
                        path: path.clone(),
                        before: old,
                        after: new,
                    }),
                }
                path.pop();
            }
            return Ok(());
        }
    }
    changes.push(Change {
        path: path.clone(),
        before: Some(before),
        after: Some(after),
    });
    Ok(())
}

/// New ancestor objects, leaf-first. The replacement object itself is caller-owned.
/// Nothing is saved or adopted; caller must persist a complete verified closure and
/// history before atomically CAS-adopting the new head in its transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    pub root: TreeId,
    pub objects: Vec<Object>,
}

/// Copy-on-write replacement along exact keys (no slash splitting/normalization).
/// Intermediate keys must exist and refer to trees. The final key may be inserted,
/// replaced, or deleted. Sequence insertion appends; replacement preserves position.
/// Only ancestors are read; this does not validate the replacement object's closure.
pub fn replace_path<R: ObjectReader + ?Sized>(
    reader: &mut R,
    root: TreeId,
    path: &[&str],
    replacement: Option<ContentId>,
    limits: TraversalLimits,
) -> Result<Rewrite, Error> {
    if path.is_empty() {
        return Err(Error::EmptyPath);
    }
    let mut budget = Budget::new(limits);
    budget.depth(path.len())?;
    let mut objects = Vec::new();
    let root = rewrite(
        reader,
        root,
        path,
        replacement,
        &mut budget,
        0,
        &mut objects,
    )?;
    Ok(Rewrite { root, objects })
}
fn rewrite<R: ObjectReader + ?Sized>(
    reader: &mut R,
    root: TreeId,
    path: &[&str],
    replacement: Option<ContentId>,
    budget: &mut Budget,
    depth: usize,
    objects: &mut Vec<Object>,
) -> Result<TreeId, Error> {
    let tree = budget.tree(reader, root, depth)?;
    let target = if path.len() == 1 {
        replacement
    } else {
        let entry = tree
            .entries()
            .iter()
            .find(|e| e.key == path[0])
            .ok_or_else(|| Error::MissingKey(path[0].into()))?;
        let ContentId::Tree(child) = entry.target else {
            return Err(Error::NotTree);
        };
        Some(ContentId::Tree(rewrite(
            reader,
            child,
            &path[1..],
            replacement,
            budget,
            depth + 1,
            objects,
        )?))
    };
    let object = Object::Tree(tree.replace(path[0], target)?);
    let ObjectId::Tree(id) = object.id() else {
        unreachable!("tree object")
    };
    if id != root {
        objects.push(object);
    }
    Ok(id)
}
