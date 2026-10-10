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
            Err(error) if matches!(error.classification_source(), WorkdirError::NotFound(_)) => {
                None
            }
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
        listing: None,
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

    /// Stat crosses the real JSON transport boundary; mutations still use the
    /// checked local provider, so the assertions cover durable tool effects.
    #[derive(Debug)]
    struct StatErrorSession {
        local: LocalWorkdirSession,
        code: &'static str,
        depth: u8,
        stats: std::sync::atomic::AtomicUsize,
        writes: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl workdir::WorkdirSession for StatErrorSession {
        fn workdir(&self) -> &workdir::Workdir {
            self.local.workdir()
        }
        fn capabilities(&self) -> workdir::WorkdirSessionCapabilities {
            self.local.capabilities()
        }
        async fn stat(&self, _: StatRequest) -> Result<workdir::StatResult, WorkdirError> {
            use workdir::{WorkdirDenialReason as Reason, http::WorkdirTransportError};
            self.stats.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let wire = serde_json::json!({
                "code": self.code,
                "message": "safe fixture message",
                "denial_reason": (self.depth > 0).then_some(Reason::OsPermissionDenied),
            });
            let mut error = serde_json::from_value::<WorkdirTransportError>(wire)
                .unwrap()
                .into_workdir_error();
            if self.depth == 2 {
                error = WorkdirError::DenialContext {
                    reason: Reason::ReadOnlySession,
                    source: Box::new(error),
                };
            }
            Err(error)
        }
        async fn read(&self, r: workdir::ReadRequest) -> Result<workdir::ReadResult, WorkdirError> {
            self.local.read(r).await
        }
        async fn write(
            &self,
            r: workdir::WriteRequest,
        ) -> Result<workdir::WriteResult, WorkdirError> {
            self.writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.local.write(r).await
        }
        async fn edit(&self, r: workdir::EditRequest) -> Result<workdir::EditResult, WorkdirError> {
            self.local.edit(r).await
        }
        async fn list(&self, r: workdir::ListRequest) -> Result<workdir::ListResult, WorkdirError> {
            self.local.list(r).await
        }
        async fn glob(&self, r: workdir::GlobRequest) -> Result<workdir::GlobResult, WorkdirError> {
            self.local.glob(r).await
        }
        async fn grep(&self, r: workdir::GrepRequest) -> Result<workdir::GrepResult, WorkdirError> {
            self.local.grep(r).await
        }
        async fn start_command(
            &self,
            r: workdir::CommandRequest,
        ) -> Result<workdir::CommandHandle, WorkdirError> {
            self.local.start_command(r).await
        }
        async fn command_status(
            &self,
            r: workdir::CommandHandle,
        ) -> Result<workdir::CommandStatus, WorkdirError> {
            self.local.command_status(r).await
        }
        async fn command_output(
            &self,
            r: workdir::CommandOutputRequest,
        ) -> Result<workdir::CommandOutput, WorkdirError> {
            self.local.command_output(r).await
        }
        async fn cancel_command(&self, r: workdir::CommandHandle) -> Result<(), WorkdirError> {
            self.local.cancel_command(r).await
        }
        async fn close(&self) -> Result<(), WorkdirError> {
            self.local.close().await
        }
    }

    fn stat_error_session(dir: &TempDir, code: &'static str, depth: u8) -> Arc<StatErrorSession> {
        Arc::new(StatErrorSession {
            local: LocalWorkdirSession::new(
                Scope::writable(dir.path()).unwrap(),
                dir.path().to_path_buf(),
            ),
            code,
            depth,
            stats: Default::default(),
            writes: Default::default(),
        })
    }

    #[tokio::test]
    async fn write_not_found_diagnostics_preserve_create_behavior() {
        for depth in [0, 1, 2] {
            let dir = TempDir::new().unwrap();
            let session = stat_error_session(&dir, "not_found", depth);
            let (_, tool) = write_tool(session.clone(), Tracker::new())();
            let output = tool
                .execute(
                    r#"{"file_path":"new.txt","content":"hello\n"}"#,
                    Default::default(),
                )
                .await
                .unwrap_or_else(|error| panic!("depth {depth}: {error:?}"));
            assert!(output.summary.contains("Created new.txt"), "{output:?}");
            assert_eq!(
                std::fs::read_to_string(dir.path().join("new.txt")).unwrap(),
                "hello\n"
            );
            assert_eq!(session.stats.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(session.writes.load(std::sync::atomic::Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn write_stat_failures_do_not_create_or_retry_with_diagnostics() {
        for code in ["denied", "read_only", "conflict", "outcome_unknown"] {
            for depth in [0, 1, 2] {
                let dir = TempDir::new().unwrap();
                let session = stat_error_session(&dir, code, depth);
                let (_, tool) = write_tool(session.clone(), Tracker::new())();
                let error = tool
                    .execute(
                        r#"{"file_path":"new.txt","content":"hello"}"#,
                        Default::default(),
                    )
                    .await
                    .unwrap_err();
                if matches!(code, "conflict" | "outcome_unknown") {
                    assert!(
                        matches!(error, ToolError::ExecutionFailed(_)),
                        "{code}, depth {depth}: {error:?}"
                    );
                } else {
                    assert!(
                        matches!(error, ToolError::InvalidArgument(_)),
                        "{code}, depth {depth}: {error:?}"
                    );
                }
                assert!(!dir.path().join("new.txt").exists());
                assert_eq!(session.stats.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert_eq!(session.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
            }
        }
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
