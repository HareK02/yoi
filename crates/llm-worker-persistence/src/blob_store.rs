//! Blob storage abstraction for large tool outputs.
//!
//! [`BlobStore`] provides async storage and retrieval of [`Content`] blobs,
//! keeping them separate from session logs. Session logs reference blobs
//! by [`BlobId`] in tool result summaries.

use llm_worker::tool::Content;
use std::future::Future;

/// Unique blob identifier. UUID v7 (time-ordered).
pub type BlobId = uuid::Uuid;

/// Generate a new blob ID.
pub fn new_blob_id() -> BlobId {
    uuid::Uuid::now_v7()
}

/// Errors from the blob store.
#[derive(Debug, thiserror::Error)]
pub enum BlobStoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("blob not found: {0}")]
    NotFound(BlobId),
}

/// Async blob storage backend.
///
/// Stores and retrieves [`Content`] blobs independently of session logs.
/// All methods take `&self` — implementations should use interior mutability
/// when needed.
pub trait BlobStore: Send + Sync {
    /// Store content and return its assigned ID.
    fn store(
        &self,
        content: &Content,
    ) -> impl Future<Output = Result<BlobId, BlobStoreError>> + Send;

    /// Load content by ID.
    fn load(
        &self,
        id: BlobId,
    ) -> impl Future<Output = Result<Content, BlobStoreError>> + Send;

    /// Check if a blob exists.
    fn exists(
        &self,
        id: BlobId,
    ) -> impl Future<Output = Result<bool, BlobStoreError>> + Send;
}
