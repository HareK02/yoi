//! `ViewImage` tool — attach a bounded image from the scoped Workdir.

use std::sync::Arc;

use agen::tool::{
    Attachment, ImageAttachment, Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput,
};
use async_trait::async_trait;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use workdir::{
    ContentHash, ReadBytesRequest, WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter,
};

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
        let bytes = read_workdir_bytes(selected.session.as_ref(), path.clone(), MAX_IMAGE_BYTES)
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

/// Read arbitrary bytes through the scoped provider, bounded by `max_total_bytes`.
/// Checks every chunk's identity, range, size and hash, then verifies the assembled
/// SHA256 against the provider's pinned [`ContentHash`]. No MIME restriction applies.
pub async fn read_workdir_bytes(
    session: &dyn workdir::WorkdirSession,
    path: WorkdirPath,
    max_total_bytes: usize,
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
        if result.total_bytes > max_total_bytes as u64 {
            return Err(workdir::WorkdirError::InvalidArgument(format!(
                "file exceeds the {max_total_bytes}-byte limit"
            )));
        }
        if expected_total.is_some_and(|total| total != result.total_bytes)
            || expected_hash.is_some_and(|hash| hash != result.content_hash)
        {
            return Err(workdir::WorkdirError::Conflict(
                "file changed while it was being read; retry the read".to_string(),
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
                    "bounded binary read returned partial file bytes".to_string(),
                ));
            }
            let assembled_hash: ContentHash = Sha256::digest(&bytes).into();
            if assembled_hash != result.content_hash {
                return Err(workdir::WorkdirError::Transport(
                    "bounded binary read assembled bytes do not match the content hash".to_string(),
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

/// Detect PNG, JPEG, GIF or WebP signatures without trusting a file extension.
pub fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
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
    use manifest::Scope;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use workdir::*;

    #[derive(Debug)]
    struct BinarySession {
        local: LocalWorkdirSession,
        responses: Mutex<VecDeque<ReadBytesResult>>,
    }

    #[async_trait]
    impl WorkdirSession for BinarySession {
        fn workdir(&self) -> &Workdir {
            self.local.workdir()
        }
        fn capabilities(&self) -> WorkdirSessionCapabilities {
            self.local.capabilities()
        }
        async fn read_bytes(
            &self,
            request: ReadBytesRequest,
        ) -> Result<ReadBytesResult, WorkdirError> {
            let response = self.responses.lock().unwrap().pop_front().unwrap();
            if request.offset > 0 {
                assert!(request.expected_hash.is_some());
            }
            assert!(request.max_bytes <= IMAGE_READ_CHUNK_BYTES);
            Ok(response)
        }
        async fn stat(&self, r: StatRequest) -> Result<StatResult, WorkdirError> {
            self.local.stat(r).await
        }
        async fn read(&self, r: ReadRequest) -> Result<ReadResult, WorkdirError> {
            self.local.read(r).await
        }
        async fn write(&self, r: WriteRequest) -> Result<WriteResult, WorkdirError> {
            self.local.write(r).await
        }
        async fn edit(&self, r: EditRequest) -> Result<EditResult, WorkdirError> {
            self.local.edit(r).await
        }
        async fn list(&self, r: ListRequest) -> Result<ListResult, WorkdirError> {
            self.local.list(r).await
        }
        async fn glob(&self, r: GlobRequest) -> Result<GlobResult, WorkdirError> {
            self.local.glob(r).await
        }
        async fn grep(&self, r: GrepRequest) -> Result<GrepResult, WorkdirError> {
            self.local.grep(r).await
        }
        async fn start_command(&self, r: CommandRequest) -> Result<CommandHandle, WorkdirError> {
            self.local.start_command(r).await
        }
        async fn command_status(&self, r: CommandHandle) -> Result<CommandStatus, WorkdirError> {
            self.local.command_status(r).await
        }
        async fn command_output(
            &self,
            r: CommandOutputRequest,
        ) -> Result<CommandOutput, WorkdirError> {
            self.local.command_output(r).await
        }
        async fn cancel_command(&self, r: CommandHandle) -> Result<(), WorkdirError> {
            self.local.cancel_command(r).await
        }
        async fn close(&self) -> Result<(), WorkdirError> {
            self.local.close().await
        }
    }

    fn binary_session(dir: &TempDir, responses: Vec<ReadBytesResult>) -> BinarySession {
        BinarySession {
            local: LocalWorkdirSession::new(
                Scope::writable(dir.path()).unwrap(),
                dir.path().into(),
            ),
            responses: Mutex::new(responses.into()),
        }
    }

    fn chunk(bytes: &[u8], offset: u64, total: u64, hash: ContentHash) -> ReadBytesResult {
        ReadBytesResult {
            path: WorkdirPath::new("artifact.bin").unwrap(),
            bytes: bytes.to_vec(),
            offset,
            total_bytes: total,
            content_hash: hash,
            eof: offset + bytes.len() as u64 == total,
        }
    }

    #[tokio::test]
    async fn bounded_binary_read_rejects_bytes_with_forged_consistent_hash() {
        let dir = TempDir::new().unwrap();
        let hash = Sha256::digest(b"abcd").into();
        let session = binary_session(
            &dir,
            vec![chunk(b"ab", 0, 4, hash), chunk(b"cx", 2, 4, hash)],
        );
        let error = read_workdir_bytes(&session, WorkdirPath::new("artifact.bin").unwrap(), 4)
            .await
            .unwrap_err();
        assert!(
            matches!(error, WorkdirError::Transport(message) if message.contains("content hash"))
        );
    }

    #[tokio::test]
    async fn bounded_binary_read_rejects_invalid_provider_chunk_metadata() {
        let dir = TempDir::new().unwrap();
        let hash = Sha256::digest(b"abcd").into();
        let valid = chunk(b"abcd", 0, 4, hash);
        let mut cases = Vec::new();
        let mut wrong_path = valid.clone();
        wrong_path.path = WorkdirPath::new("other.bin").unwrap();
        cases.push(wrong_path);
        let mut wrong_offset = valid.clone();
        wrong_offset.offset = 1;
        cases.push(wrong_offset);
        let mut wrong_eof = valid.clone();
        wrong_eof.eof = false;
        cases.push(wrong_eof);
        let mut wrong_total = valid.clone();
        wrong_total.total_bytes = 3;
        cases.push(wrong_total);
        cases.push(chunk(b"", 0, 4, hash));
        cases.push(chunk(
            &vec![0; IMAGE_READ_CHUNK_BYTES + 1],
            0,
            (IMAGE_READ_CHUNK_BYTES + 1) as u64,
            hash,
        ));
        for response in cases {
            let session = binary_session(&dir, vec![response]);
            let error = read_workdir_bytes(&session, valid.path.clone(), 16 * 1024 * 1024)
                .await
                .unwrap_err();
            assert!(matches!(error, WorkdirError::Transport(_)), "{error}");
        }
    }

    #[tokio::test]
    async fn bounded_binary_read_rejects_hash_or_size_changes_between_chunks() {
        let dir = TempDir::new().unwrap();
        let hash = Sha256::digest(b"abcd").into();
        for second in [chunk(b"cd", 2, 4, [0; 32]), chunk(b"cd", 2, 5, hash)] {
            let session = binary_session(&dir, vec![chunk(b"ab", 0, 4, hash), second]);
            let error = read_workdir_bytes(&session, WorkdirPath::new("artifact.bin").unwrap(), 5)
                .await
                .unwrap_err();
            assert!(matches!(error, WorkdirError::Conflict(_)), "{error}");
        }
    }

    #[tokio::test]
    async fn bounded_binary_read_accepts_arbitrary_bytes_at_caller_limit_and_rejects_larger_files()
    {
        let dir = TempDir::new().unwrap();
        let bytes = vec![0xff; 16 * 1024 * 1024];
        std::fs::write(dir.path().join("artifact.bin"), &bytes).unwrap();
        let session =
            LocalWorkdirSession::new(Scope::writable(dir.path()).unwrap(), dir.path().into());
        let path = WorkdirPath::new("artifact.bin").unwrap();
        assert_eq!(
            read_workdir_bytes(&session, path.clone(), bytes.len())
                .await
                .unwrap(),
            bytes
        );
        assert!(matches!(
            read_workdir_bytes(&session, path.clone(), bytes.len() - 1).await,
            Err(WorkdirError::InvalidArgument(_))
        ));
        std::fs::write(dir.path().join("artifact.bin"), []).unwrap();
        assert!(
            read_workdir_bytes(&session, path, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }

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
