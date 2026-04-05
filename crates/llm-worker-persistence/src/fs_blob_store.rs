//! Filesystem-backed blob store.
//!
//! Layout:
//! - Text blobs:       `{root}/{blob_id}.txt`
//! - Structured blobs: `{root}/{blob_id}.json`

use crate::blob_store::{new_blob_id, BlobId, BlobStore, BlobStoreError};
use llm_worker::tool::Content;
use std::path::PathBuf;
use tokio::fs;

/// Filesystem-backed blob store.
///
/// Each blob is stored as a single file. Text content uses `.txt`,
/// structured (JSON) content uses `.json`.
#[derive(Clone)]
pub struct FsBlobStore {
    root: PathBuf,
}

impl FsBlobStore {
    /// Create a new `FsBlobStore` rooted at the given directory.
    /// Creates the directory if it does not exist.
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, BlobStoreError> {
        let root = root.into();
        fs::create_dir_all(&root).await?;
        Ok(Self { root })
    }

    fn text_path(&self, id: BlobId) -> PathBuf {
        self.root.join(format!("{id}.txt"))
    }

    fn json_path(&self, id: BlobId) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    /// Resolve the actual path for a blob, checking both extensions.
    fn resolve_path(&self, id: BlobId) -> Option<(PathBuf, bool)> {
        let txt = self.text_path(id);
        if txt.exists() {
            return Some((txt, true));
        }
        let json = self.json_path(id);
        if json.exists() {
            return Some((json, false));
        }
        None
    }
}

impl BlobStore for FsBlobStore {
    async fn store(&self, content: &Content) -> Result<BlobId, BlobStoreError> {
        let id = new_blob_id();
        match content {
            Content::Text(text) => {
                fs::write(self.text_path(id), text.as_bytes()).await?;
            }
            Content::Structured(value) => {
                let json = serde_json::to_string_pretty(value)?;
                fs::write(self.json_path(id), json.as_bytes()).await?;
            }
        }
        Ok(id)
    }

    async fn load(&self, id: BlobId) -> Result<Content, BlobStoreError> {
        let (path, is_text) = self
            .resolve_path(id)
            .ok_or(BlobStoreError::NotFound(id))?;
        let bytes = fs::read_to_string(&path).await?;
        if is_text {
            Ok(Content::Text(bytes))
        } else {
            let value = serde_json::from_str(&bytes)?;
            Ok(Content::Structured(value))
        }
    }

    async fn exists(&self, id: BlobId) -> Result<bool, BlobStoreError> {
        Ok(self.resolve_path(id).is_some())
    }
}
