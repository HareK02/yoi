#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

mod model;
mod traversal;

pub use model::{
    BlobId, ContentId, Entry, Error, History, HistoryId, Object, ObjectId, Tree, TreeId, verify,
};
pub use traversal::{
    Change, ObjectReader, RestoredContent, RestoredEntry, Rewrite, TraversalLimits, diff,
    replace_path, restore,
};
