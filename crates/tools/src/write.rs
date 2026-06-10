//! `Write` tool — create or overwrite a file.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::scoped_fs::ScopedFs;
use crate::tracker::Tracker;

const DESCRIPTION: &str = "Create a new file or overwrite an existing one with \
the given content. Missing parent directories within scope are created \
automatically. Existing files must have been read first (via the Read tool) \
in this session. Paths must be absolute.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct WriteParams {
    /// Absolute path to the file.
    pub file_path: PathBuf,
    /// Full content to write. Overwrites any existing content.
    pub content: String,
}

pub(crate) struct WriteTool {
    fs: ScopedFs,
    tracker: Tracker,
}

#[async_trait]
impl Tool for WriteTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: WriteParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Write input: {e}")))?;

        tracing::debug!(
            path = %params.file_path.display(),
            bytes = params.content.len(),
            "Write"
        );

        let _mutation_permit = self.tracker.acquire_mutation(&params.file_path, &ctx).await;

        // Policy check: if the target already exists, it must have been
        // observed by the Read tool (via the tracker) and its current
        // contents must match the recorded hash.
        if params.file_path.exists() {
            let current = self.fs.read_bytes(&params.file_path)?;
            self.tracker.verify(&params.file_path, &current)?;
        }

        let outcome = self
            .fs
            .write(&params.file_path, params.content.as_bytes())?;

        // Refresh the history entry to reflect the newly-written content,
        // so a subsequent Edit / Write can proceed without a re-read.
        self.tracker
            .record(&params.file_path, params.content.as_bytes());

        let summary = format!(
            "{} {} ({} bytes)",
            if outcome.created {
                "Created"
            } else {
                "Overwrote"
            },
            params.file_path.display(),
            outcome.bytes_written
        );
        Ok(ToolOutput {
            summary,
            content: None,
        })
    }
}

/// Factory for the `Write` tool.
pub fn write_tool(fs: ScopedFs, tracker: Tracker) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(WriteParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Write")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WriteTool {
            fs: fs.clone(),
            tracker: tracker.clone(),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::read_tool;
    use manifest::Scope;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ScopedFs, Tracker) {
        let dir = TempDir::new().unwrap();
        let fs = ScopedFs::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        );
        (dir, fs, Tracker::new())
    }

    #[tokio::test]
    async fn write_creates_new_file_without_read() {
        let (dir, fs, tracker) = setup();
        let def = write_tool(fs, tracker);
        let (meta, tool) = def();
        assert_eq!(meta.name, "Write");

        let file = dir.path().join("new.txt");
        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "hello\n",
        });
        let out = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Created"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello\n");
    }

    #[tokio::test]
    async fn write_existing_requires_prior_read() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old").unwrap();

        let def = write_tool(fs, tracker);
        let (_, tool) = def();
        let input = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "new",
        });
        let err = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn write_existing_after_read_succeeds() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "old\n").unwrap();

        let read_def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = read_def();
        let read_in = serde_json::json!({ "file_path": file.to_str().unwrap() });
        reader
            .execute(&read_in.to_string(), Default::default())
            .await
            .unwrap();

        let write_def = write_tool(fs, tracker);
        let (_, writer) = write_def();
        let write_in = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "new\n",
        });
        let out = writer
            .execute(&write_in.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("Overwrote"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
    }

    #[tokio::test]
    async fn write_detects_external_modification_via_hash() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "v1").unwrap();

        // Read records hash of "v1".
        let read_def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = read_def();
        reader
            .execute(
                &serde_json::json!({ "file_path": file.to_str().unwrap() }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();

        // External process overwrites with a different content.
        std::fs::write(&file, "tampered").unwrap();

        let write_def = write_tool(fs, tracker);
        let (_, writer) = write_def();
        let err = writer
            .execute(
                &serde_json::json!({
                    "file_path": file.to_str().unwrap(),
                    "content": "new",
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("modified externally"), "{msg}");
    }

    #[tokio::test]
    async fn write_rejects_out_of_scope() {
        let (_dir, fs, tracker) = setup();
        let outside = TempDir::new().unwrap();

        let def = write_tool(fs, tracker);
        let (_, tool) = def();
        let input = serde_json::json!({
            "file_path": outside.path().join("x.txt").to_str().unwrap(),
            "content": "x",
        });
        let err = tool
            .execute(&input.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn write_then_edit_same_file_same_batch_uses_call_order() {
        use crate::edit::edit_tool;
        use llm_worker::tool::ToolExecutionContext;

        let (dir, fs, tracker) = setup();
        let file = dir.path().join("ordered.txt");

        let write_def = write_tool(fs.clone(), tracker.clone());
        let (_, writer) = write_def();
        let edit_def = edit_tool(fs, tracker);
        let (_, editor) = edit_def();

        let write_in = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "content": "hello",
        });
        let edit_in = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "hello",
            "new_string": "goodbye",
        });

        let write_json = write_in.to_string();
        let edit_json = edit_in.to_string();
        let (write_out, edit_out) = tokio::join!(
            writer.execute(&write_json, ToolExecutionContext::new("write", "batch", 0),),
            editor.execute(&edit_json, ToolExecutionContext::new("edit", "batch", 1)),
        );

        write_out.unwrap();
        edit_out.unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "goodbye");
    }

    #[tokio::test]
    async fn failed_same_file_mutation_releases_guard_for_followup() {
        use crate::edit::edit_tool;
        use llm_worker::tool::ToolExecutionContext;

        let (dir, fs, tracker) = setup();
        let file = dir.path().join("release.txt");
        std::fs::write(&file, "alpha").unwrap();

        let read_def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = read_def();
        reader
            .execute(
                &serde_json::json!({ "file_path": file.to_str().unwrap() }).to_string(),
                ToolExecutionContext::new("read", "pre", 0),
            )
            .await
            .unwrap();

        let edit_def = edit_tool(fs, tracker);
        let (_, editor) = edit_def();
        let bad_edit = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "missing",
            "new_string": "beta",
        });
        let good_edit = serde_json::json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "alpha",
            "new_string": "beta",
        });

        assert!(
            editor
                .execute(
                    &bad_edit.to_string(),
                    ToolExecutionContext::new("bad", "batch", 0),
                )
                .await
                .is_err()
        );
        editor
            .execute(
                &good_edit.to_string(),
                ToolExecutionContext::new("good", "batch", 1),
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "beta");
    }
}
