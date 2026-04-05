//! [`ToolOutputProcessor`] implementation backed by a [`BlobStore`].
//!
//! Converts large tool output strings into [`ToolOutput::Stored`] and
//! persists the content via a [`BlobStore`], returning a summary with
//! a blob reference for conversation history.

use crate::blob_store::BlobStore;
use async_trait::async_trait;
use llm_worker::tool::{ToolError, ToolOutput, ToolOutputProcessor};
use std::sync::Arc;

/// A [`ToolOutputProcessor`] that stores large outputs in a [`BlobStore`].
///
/// Small outputs (≤ `INLINE_THRESHOLD` bytes) pass through unchanged.
/// Large outputs are stored as blobs, and a summary with a `[blob:<id>]`
/// reference replaces the original content in conversation history.
pub struct BlobOutputProcessor<B: BlobStore> {
    blob_store: Arc<B>,
}

impl<B: BlobStore> BlobOutputProcessor<B> {
    /// Create a new processor backed by the given blob store.
    pub fn new(blob_store: Arc<B>) -> Self {
        Self { blob_store }
    }
}

#[async_trait]
impl<B: BlobStore + 'static> ToolOutputProcessor for BlobOutputProcessor<B> {
    async fn process(&self, output: String) -> Result<String, ToolError> {
        let tool_output = ToolOutput::from(output);

        match tool_output {
            ToolOutput::Inline(s) => Ok(s),
            ToolOutput::Stored { summary, content } => {
                let blob_id = self
                    .blob_store
                    .store(&content)
                    .await
                    .map_err(|e| ToolError::Internal(format!("blob store error: {e}")))?;

                // Prepend blob reference to the summary
                Ok(format!("[blob:{blob_id}] {summary}"))
            }
        }
    }
}
