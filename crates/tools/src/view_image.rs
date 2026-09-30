//! `ViewImage` tool — attach a bounded image from the scoped Workdir.

use std::sync::Arc;

use agen::tool::{
    Attachment, ImageAttachment, Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput,
};
use async_trait::async_trait;
use serde::Deserialize;
use workdir::{ReadBytesRequest, WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter};

use crate::error::ToolsError;

/// Maximum image body accepted for one model request.
pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
/// One binary response remains within the External Workdir transport bound.
const IMAGE_READ_CHUNK_BYTES: usize = 1024 * 1024;

const DESCRIPTION: &str = "Attach an image from the selected Workdir attachment to the next model request. \
The path must be logical and Workdir-relative. Supported formats: PNG, JPEG, GIF, and WebP. \
When exactly one Workdir is attached, target_workdir may be omitted.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ViewImageParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    target_workdir: Option<String>,
    /// Logical path relative to the bound Workdir root.
    path: String,
}

struct ViewImageTool {
    router: Arc<WorkdirSessionRouter>,
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
        let selected = crate::routing::resolve_session(
            &self.router,
            input.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Read,
        )?;
        let path = WorkdirPath::new(&input.path).map_err(ToolsError::from)?;
        let bytes = read_image_bytes(selected.session.as_ref(), path.clone())
            .await
            .map_err(ToolsError::from)?;
        let mime_type = detect_image_mime(&bytes).ok_or_else(|| {
            ToolError::InvalidArgument(
                "unsupported image; expected PNG, JPEG, GIF, or WebP bytes".to_string(),
            )
        })?;
        let byte_count = bytes.len();

        Ok(ToolOutput {
            summary: format!("Attached image {path} ({mime_type}, {byte_count} bytes)"),
            content: None,
            attachments: vec![Attachment::Image(ImageAttachment::new(
                mime_type,
                Arc::<[u8]>::from(bytes),
            ))],
        })
    }
}

async fn read_image_bytes(
    session: &dyn workdir::WorkdirSession,
    path: WorkdirPath,
) -> Result<Vec<u8>, workdir::WorkdirError> {
    let mut bytes = Vec::new();
    let mut offset = 0_u64;
    let mut expected_hash = None;
    let mut expected_total = None;

    loop {
        let remaining = expected_total
            .map(|total: u64| total.saturating_sub(offset) as usize)
            .unwrap_or(IMAGE_READ_CHUNK_BYTES);
        let request_bytes = remaining.min(IMAGE_READ_CHUNK_BYTES).max(1);
        let result = session
            .read_bytes(ReadBytesRequest {
                path: path.clone(),
                offset,
                max_bytes: request_bytes,
                expected_hash,
            })
            .await?;

        if result.path != path || result.offset != offset {
            return Err(workdir::WorkdirError::Transport(
                "bounded binary read returned a mismatched path or offset".to_string(),
            ));
        }
        if result.bytes.len() > request_bytes {
            return Err(workdir::WorkdirError::Transport(
                "bounded binary read exceeded the requested response size".to_string(),
            ));
        }
        let end = result
            .offset
            .checked_add(result.bytes.len() as u64)
            .ok_or_else(|| {
                workdir::WorkdirError::Transport(
                    "bounded binary read returned an invalid byte range".to_string(),
                )
            })?;
        if end > result.total_bytes || result.eof != (end == result.total_bytes) {
            return Err(workdir::WorkdirError::Transport(
                "bounded binary read returned a truncated or invalid byte range".to_string(),
            ));
        }
        if result.total_bytes > MAX_IMAGE_BYTES as u64 {
            return Err(workdir::WorkdirError::InvalidArgument(format!(
                "image exceeds the {MAX_IMAGE_BYTES}-byte limit"
            )));
        }
        if expected_total.is_some_and(|total| total != result.total_bytes)
            || expected_hash.is_some_and(|hash| hash != result.content_hash)
        {
            return Err(workdir::WorkdirError::Conflict(
                "image changed while it was being read; retry ViewImage".to_string(),
            ));
        }
        if expected_total.is_none() {
            bytes
                .try_reserve_exact(result.total_bytes as usize)
                .map_err(|_| workdir::WorkdirError::OperationFailed)?;
            expected_total = Some(result.total_bytes);
            expected_hash = Some(result.content_hash);
        }

        if result.bytes.is_empty() && !result.eof {
            return Err(workdir::WorkdirError::Transport(
                "bounded binary read made no progress".to_string(),
            ));
        }
        bytes.extend_from_slice(&result.bytes);
        offset = end;
        if result.eof {
            if bytes.len() as u64 != result.total_bytes {
                return Err(workdir::WorkdirError::Transport(
                    "bounded binary read returned a partial image".to_string(),
                ));
            }
            return Ok(bytes);
        }
    }
}

pub fn view_image_tool(session: WorkdirSessionHandle) -> ToolDefinition {
    routed_view_image_tool(crate::routing::singleton_router(session))
}

pub fn routed_view_image_tool(router: Arc<WorkdirSessionRouter>) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(ViewImageParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("ViewImage")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(ViewImageTool {
            router: router.clone(),
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
