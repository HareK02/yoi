//! `Write` tool — create or overwrite a file.

use std::path::PathBuf;
use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;

use crate::error::ToolsError;
use crate::tracker::Tracker;
use workdir::{StatRequest, WorkdirError, WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter};

const DESCRIPTION: &str = "Create a new file or overwrite an existing one in the selected Workdir attachment. \
Missing parent directories within scope are created automatically. Existing files must have been read first \
through the same attachment. When exactly one Workdir is attached, target_workdir may be omitted.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct WriteParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    pub target_workdir: Option<String>,
    /// Logical path relative to the bound Workdir root.
    pub file_path: String,
    #[serde(flatten)]
    pub write: fs_operation::text::WriteArgs,
}

pub(crate) struct WriteTool {
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
}

#[async_trait]
impl Tool for WriteTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: WriteParams = crate::error::decode_file_input(input_json, "Write")?;

        let selected = crate::routing::resolve_session(
            &self.router,
            params.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Write,
        )?;
        let tracker = self
            .tracker
            .scoped_attachment(&selected.alias, selected.generation);
        let path = WorkdirPath::new(&params.file_path).map_err(ToolsError::from)?;
        Ok(execute_write(
            crate::file_target::FileTarget {
                session: selected.session,
                path,
                validator: None,
            },
            tracker,
            params.write.content,
            None,
            ctx,
        )
        .await?
        .output)
    }
}

/// `create_path` is a checked parent-relative create-new target, never an overwrite.
pub(crate) async fn execute_write(
    target: crate::file_target::FileTarget,
    tracker: Tracker,
    content: String,
    create_path: Option<WorkdirPath>,
    ctx: agen::tool::ToolExecutionContext,
) -> Result<crate::checkout::CheckoutToolOutput, ToolError> {
    // Provider size/access checks still run inside the checked save. The shared
    // core owns text semantics, not the provider's bounds or lifecycle.
    let content = fs_operation::text::write(content, Default::default())
        .map_err(crate::error::text_error)?
        .content;
    let path = create_path.as_ref().unwrap_or(&target.path);
    tracing::debug!(path = %path, bytes = content.len(), "Write");
    let mutation_key = PathBuf::from(path.as_str());
    let _mutation_permit = tracker.acquire_mutation(&mutation_key, &ctx).await;
    let expected_hash = if create_path.is_some() {
        None
    } else if target.validator.is_some() {
        // Native Write is existing-file-only, and always requires a prior observation.
        Some(tracker.expected_workdir_hash(path)?)
    } else {
        match target
            .session
            .stat(StatRequest { path: path.clone() })
            .await
        {
            Ok(_) => Some(tracker.expected_workdir_hash(path)?),
            Err(WorkdirError::NotFound(_)) => None,
            Err(error) => return Err(ToolsError::from(error).into()),
        }
    };
    let old_line_count = tracker.observed_workdir_line_count(path).unwrap_or(0);
    let (outcome, validator) = target
        .write(
            content.as_bytes().to_vec(),
            expected_hash,
            create_path.clone(),
        )
        .await?;
    tracker.record_change(content.lines().count(), old_line_count);
    tracker.record_workdir_content(path, content.as_bytes());
    let summary = format!(
        "{} {} ({} bytes)",
        if outcome.created {
            "Created"
        } else {
            "Overwrote"
        },
        path,
        outcome.bytes_written
    );
    Ok(crate::checkout::CheckoutToolOutput {
        output: ToolOutput {
            summary,
            content: None,
            attachments: Vec::new(),
        },
        paths: create_path.into_iter().collect(),
        validator,
    })
}

/// Factory for the `Write` tool bound to one compatibility session.
pub fn write_tool(session: WorkdirSessionHandle, tracker: Tracker) -> ToolDefinition {
    routed_write_tool(crate::routing::singleton_router(session), tracker)
}

pub(crate) fn routed_write_tool(
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(WriteParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Write")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(WriteTool {
            router: router.clone(),
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
    use workdir::LocalWorkdirSession;

    fn setup() -> (TempDir, WorkdirSessionHandle, Tracker) {
        let dir = TempDir::new().unwrap();
        let session: WorkdirSessionHandle = Arc::new(LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ));
        (dir, session, Tracker::new())
    }

    #[tokio::test]
    async fn write_creates_new_file_without_read() {
        let (dir, fs, tracker) = setup();
        let def = write_tool(fs, tracker);
        let (meta, tool) = def();
        assert_eq!(meta.name, "Write");

        let file = dir.path().join("new.txt");
        let input = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
        let read_in =
            serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() });
        reader
            .execute(&read_in.to_string(), Default::default())
            .await
            .unwrap();

        let write_def = write_tool(fs, tracker);
        let (_, writer) = write_def();
        let write_in = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
                &serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() })
                    .to_string(),
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
                    "file_path": file.file_name().unwrap().to_str().unwrap(),
                    "content": "new",
                })
                .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        match err {
            ToolError::ExecutionFailed(message) => assert_eq!(
                message,
                "The target file's content or existence changed since it was last observed; read the file again before retrying: a.txt"
            ),
            other => panic!("expected execution failure, got {other:?}"),
        }
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
        use agen::tool::ToolExecutionContext;

        let (dir, fs, tracker) = setup();
        let file = dir.path().join("ordered.txt");

        let write_def = write_tool(fs.clone(), tracker.clone());
        let (_, writer) = write_def();
        let edit_def = edit_tool(fs, tracker);
        let (_, editor) = edit_def();

        let write_in = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "content": "hello",
        });
        let edit_in = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
        use agen::tool::ToolExecutionContext;

        let (dir, fs, tracker) = setup();
        let file = dir.path().join("release.txt");
        std::fs::write(&file, "alpha").unwrap();

        let read_def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = read_def();
        reader
            .execute(
                &serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() })
                    .to_string(),
                ToolExecutionContext::new("read", "pre", 0),
            )
            .await
            .unwrap();

        let edit_def = edit_tool(fs, tracker);
        let (_, editor) = edit_def();
        let bad_edit = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "missing",
            "new_string": "beta",
        });
        let good_edit = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
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
