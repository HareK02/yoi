//! `ViewImage` tool — attach a bounded image from the scoped Workdir.

use std::sync::Arc;

use agen::tool::{
    Attachment, ImageAttachment, Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput,
};
use async_trait::async_trait;
use serde::Deserialize;
use workdir::{ReadRequest, WorkdirPath, WorkdirSessionHandle};

use crate::error::ToolsError;

/// Maximum image body accepted for one model request.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

const DESCRIPTION: &str = "Attach an image from the bound Workdir to the next model request. \
The path must be logical and Workdir-relative. Supported formats: PNG, JPEG, GIF, and WebP.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ViewImageParams {
    /// Logical path relative to the bound Workdir root.
    path: String,
}

struct ViewImageTool {
    session: WorkdirSessionHandle,
}

#[async_trait]
impl Tool for ViewImageTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let input: ViewImageParams = serde_json::from_str(input_json).map_err(|error| {
            ToolError::InvalidArgument(format!("invalid ViewImage input: {error}"))
        })?;
        let path = WorkdirPath::new(&input.path).map_err(ToolsError::from)?;
        let result = self
            .session
            .read(ReadRequest {
                path: path.clone(),
                offset: 0,
                limit: usize::MAX,
                // The scoped provider enforces this cap while reading, rather
                // than allocating an unbounded binary body first.
                max_bytes: MAX_IMAGE_BYTES + 1,
            })
            .await
            .map_err(ToolsError::from)?;

        if result.truncated || result.bytes.len() > MAX_IMAGE_BYTES {
            return Err(ToolError::InvalidArgument(format!(
                "image exceeds the {MAX_IMAGE_BYTES}-byte limit"
            )));
        }
        let mime_type = detect_image_mime(&result.bytes).ok_or_else(|| {
            ToolError::InvalidArgument(
                "unsupported image; expected PNG, JPEG, GIF, or WebP bytes".to_string(),
            )
        })?;
        let bytes = result.bytes.len();

        Ok(ToolOutput {
            summary: format!("Attached image {path} ({mime_type}, {bytes} bytes)"),
            content: None,
            attachments: vec![Attachment::Image(ImageAttachment::new(
                mime_type,
                Arc::<[u8]>::from(result.bytes),
            ))],
        })
    }
}

pub fn view_image_tool(session: WorkdirSessionHandle) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ViewImageParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("ViewImage")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ViewImageTool {
            session: session.clone(),
        });
        (meta, tool)
    })
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_image_signatures_without_trusting_extensions() {
        assert_eq!(
            detect_image_mime(b"\x89PNG\r\n\x1a\nbody"),
            Some("image/png")
        );
        assert_eq!(
            detect_image_mime(&[0xff, 0xd8, 0xff, 0xe0]),
            Some("image/jpeg")
        );
        assert_eq!(detect_image_mime(b"GIF89abody"), Some("image/gif"));
        assert_eq!(detect_image_mime(b"RIFF1234WEBPbody"), Some("image/webp"));
        assert_eq!(detect_image_mime(b"not an image"), None);
    }
}
