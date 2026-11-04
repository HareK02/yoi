//! Configuration content is a real directory/name Merkle hierarchy with full-source blob leaves.
//!
//! The versioned root separates `files` from `types`: explicit content types are keyed by full
//! virtual path in `types`. A type change therefore changes the root without changing source bytes.
//! Types must not be guessed from suffixes: [`ConfigEntry`] permits explicit metadata independent
//! of a filename. Digests are reconstructed through config-source, never substituted by Merkle IDs.
//!
//! Preparation validates the original snapshot, then rejects file/directory prefix conflicts that
//! a flat snapshot can represent but a directory tree cannot. Restoration rejects empty non-root
//! directories, malformed paths, unknown types, surplus/missing metadata, invalid UTF-8 and limits.
//! Both directions use existing config-source validation; neither parses nor normalizes Decodal.
//! Evaluation, policy checks and transactional adoption remain the caller's responsibility.

use std::collections::BTreeMap;

use config_source::{
    ConfigContentType, ConfigEntry, ConfigTreeSnapshot, MAX_ENTRY_COUNT, MAX_PATH_BYTES,
    MAX_TOTAL_BYTES, VirtualPath,
};
use content_history::{ContentId, Object, ObjectId, TreeId};

use crate::{Builder, Error, Snapshot, entry, field, fields, read_tree, text, tree_id};

const FORMAT: &str = "config-tree/v1";

#[derive(Default)]
struct Directory {
    children: BTreeMap<String, Node>,
}

enum Node {
    File(ContentId),
    Directory(Directory),
}

impl Directory {
    fn insert(&mut self, parts: &[&str], target: ContentId, path: &str) -> Result<(), Error> {
        let name = parts[0];
        if parts.len() == 1 {
            if self
                .children
                .insert(name.into(), Node::File(target))
                .is_some()
            {
                return Err(Error::PathConflict(path.into()));
            }
        } else {
            let child = self
                .children
                .entry(name.into())
                .or_insert_with(|| Node::Directory(Directory::default()));
            match child {
                Node::Directory(directory) => directory.insert(&parts[1..], target, path)?,
                Node::File(_) => return Err(Error::PathConflict(path.into())),
            }
        }
        Ok(())
    }

    fn build(self, builder: &mut Builder) -> Result<TreeId, Error> {
        let mut entries = Vec::new();
        for (name, node) in self.children {
            let target = match node {
                Node::File(target) => target,
                Node::Directory(directory) => ContentId::Tree(directory.build(builder)?),
            };
            entries.push(entry(name, target));
        }
        builder.map(entries)
    }
}

/// Prepare objects only after validating all original snapshot metadata.
pub fn prepare(snapshot: &ConfigTreeSnapshot) -> Result<Snapshot, Error> {
    snapshot.validate()?;
    let mut builder = Builder::default();
    let mut directory = Directory::default();
    let mut types = Vec::new();
    for (path, source) in &snapshot.entries {
        directory.insert(
            &path.as_str().split('/').collect::<Vec<_>>(),
            builder.blob(&source.content),
            path.as_str(),
        )?;
        let content_type = match source.content_type {
            ConfigContentType::Decodal => "decodal",
            ConfigContentType::Text => "text",
        };
        types.push(entry(path.as_str(), builder.blob(content_type)));
    }
    let files = directory.build(&mut builder)?;
    let types = builder.map(types)?;
    let format = builder.blob(FORMAT);
    let root = builder.map(vec![
        entry("format", format),
        entry("files", ContentId::Tree(files)),
        entry("types", ContentId::Tree(types)),
    ])?;
    Ok(builder.finish(root))
}

/// Read through an authorized reader and reconstruct validated ConfigEntry metadata and digests.
/// The returned snapshot is a candidate, not a validated evaluation or an applied configuration.
pub fn restore(
    root: TreeId,
    mut read: impl FnMut(ObjectId) -> Result<Object, Error>,
) -> Result<ConfigTreeSnapshot, Error> {
    let root = read_tree(&mut read, root, false)?;
    fields(&root, &["format", "files", "types"])?;
    if text(&mut read, field(&root, "format")?)? != FORMAT {
        return Err(Error::Format);
    }
    let types = read_tree(&mut read, tree_id(field(&root, "types")?)?, false)?;
    if types.entries().len() > MAX_ENTRY_COUNT {
        return Err(Error::Structure("too many config types"));
    }
    let mut metadata = BTreeMap::new();
    for item in types.entries() {
        let path = VirtualPath::parse(&item.key)?;
        let kind = match text(&mut read, item.target)?.as_str() {
            "decodal" => ConfigContentType::Decodal,
            "text" => ConfigContentType::Text,
            _ => return Err(Error::Structure("unknown config content type")),
        };
        metadata.insert(path, kind);
    }
    let mut entries = Vec::new();
    let mut total = 0;
    restore_directory(
        tree_id(field(&root, "files")?)?,
        "",
        &mut read,
        &mut metadata,
        &mut entries,
        &mut total,
    )?;
    if !metadata.is_empty() {
        return Err(Error::Structure("surplus config metadata"));
    }
    Ok(ConfigTreeSnapshot::from_entries(entries)?)
}

fn restore_directory(
    id: TreeId,
    prefix: &str,
    read: &mut impl FnMut(ObjectId) -> Result<Object, Error>,
    metadata: &mut BTreeMap<VirtualPath, ConfigContentType>,
    entries: &mut Vec<ConfigEntry>,
    total: &mut usize,
) -> Result<(), Error> {
    let directory = read_tree(read, id, false)?;
    if !prefix.is_empty() && directory.entries().is_empty() {
        return Err(Error::Structure("empty non-root config directory"));
    }
    // Limit directory fanout and path length even for malformed externally supplied trees.
    if directory.entries().len() > MAX_ENTRY_COUNT {
        return Err(Error::Structure("too many config children"));
    }
    for item in directory.entries() {
        if item.key.contains('/') {
            return Err(Error::Structure("config child must be one path component"));
        }
        let path = if prefix.is_empty() {
            item.key.clone()
        } else {
            format!("{prefix}/{}", item.key)
        };
        if path.len() > MAX_PATH_BYTES {
            return Err(Error::Structure("config path too long"));
        }
        let path = VirtualPath::parse(path)?;
        match item.target {
            ContentId::Tree(child) => {
                // A valid directory must lead to an as-yet unrestored metadata entry. This also
                // bounds traversal of adversarial shared subtrees by the existing entry/path limits.
                let child_prefix = format!("{path}/");
                if !metadata.keys().any(|key| key.as_str().starts_with(&child_prefix)) {
                    return Err(Error::Structure("config directory has no file metadata"));
                }
                restore_directory(child, path.as_str(), read, metadata, entries, total)?
            }
            ContentId::Blob(_) => {
                let kind = metadata
                    .remove(&path)
                    .ok_or(Error::Structure("missing config metadata"))?;
                let source = text(read, item.target)?;
                *total = total
                    .checked_add(source.len())
                    .ok_or(Error::Structure("config bytes overflow"))?;
                if *total > MAX_TOTAL_BYTES || entries.len() >= MAX_ENTRY_COUNT {
                    return Err(Error::Structure("config snapshot limits exceeded"));
                }
                entries.push(ConfigEntry::new(path, kind, source)?);
            }
        }
    }
    Ok(())
}
