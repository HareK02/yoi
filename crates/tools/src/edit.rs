//! `Edit` tool — partial string replacement with uniqueness check.

use std::path::PathBuf;
use std::sync::Arc;

use agen::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;

use crate::error::ToolsError;
use crate::tracker::Tracker;
use workdir::{WorkdirPath, WorkdirSessionHandle, WorkdirSessionRouter};

const DESCRIPTION: &str = "Replace a substring in an existing file in the selected Workdir attachment. By default \
`old_string` must be unique in the file; set `replace_all: true` to replace \
every occurrence. The file must have been read first through the same attachment. \
When exactly one Workdir is attached, target_workdir may be omitted.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct EditParams {
    /// Worker-local alias of the Workdir attachment to use.
    #[serde(default)]
    pub target_workdir: Option<String>,
    /// Logical path relative to the bound Workdir root.
    pub file_path: String,
    #[serde(flatten)]
    pub edit: fs_operation::text::EditArgs,
}

pub(crate) struct EditTool {
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
}

#[async_trait]
impl Tool for EditTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: agen::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: EditParams = crate::error::decode_file_input(input_json, "Edit")?;

        let selected = crate::routing::resolve_session(
            &self.router,
            params.target_workdir.as_deref(),
            workdir::WorkdirSessionCapability::Edit,
        )?;
        let tracker = self
            .tracker
            .scoped_attachment(&selected.alias, selected.generation);
        let path = WorkdirPath::new(&params.file_path).map_err(ToolsError::from)?;
        Ok(execute_edit(
            crate::file_target::FileTarget {
                session: selected.session,
                path,
                validator: None,
            },
            tracker,
            params,
            ctx,
        )
        .await?
        .output)
    }
}

pub(crate) async fn execute_edit(
    target: crate::file_target::FileTarget,
    tracker: Tracker,
    params: EditParams,
    ctx: agen::tool::ToolExecutionContext,
) -> Result<crate::checkout::CheckoutToolOutput, ToolError> {
    let path = &target.path;
    tracing::debug!(path = %path, replace_all = params.edit.replace_all, "Edit");
    params
        .edit
        .validate(Default::default())
        .map_err(crate::error::text_error)?;
    let mutation_key = PathBuf::from(path.as_str());
    let _mutation_permit = tracker.acquire_mutation(&mutation_key, &ctx).await;
    let expected_hash = tracker.expected_workdir_hash(path)?;
    let (result, validator) = target
        .edit(
            params.edit.old_string.clone(),
            params.edit.new_string.clone(),
            params.edit.replace_all,
            expected_hash,
        )
        .await?;
    let replacements = result.replacements;
    tracker.record_workdir_edit(
        path,
        result.content_hash,
        replacements,
        params.edit.new_string.lines().count(),
        params.edit.old_string.lines().count(),
    );
    let summary = format!(
        "Edited {} ({} replacement{})",
        path,
        replacements,
        if replacements == 1 { "" } else { "s" }
    );
    let preview = make_preview(&params.edit.new_string, &params.edit.new_string);
    Ok(crate::checkout::CheckoutToolOutput {
        output: ToolOutput {
            summary,
            content: Some(preview),
            attachments: Vec::new(),
        },
        paths: Vec::new(),
        listing: None,
        validator,
    })
}

/// Build a small line-numbered snippet centered on the first occurrence of
/// `needle` in `text`. Shows ±3 surrounding lines.
fn make_preview(text: &str, needle: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let first_needle_line = needle.lines().next().unwrap_or(needle);
    let hit = lines
        .iter()
        .position(|l| l.contains(first_needle_line))
        .unwrap_or(0);

    let start = hit.saturating_sub(3);
    let end = (hit + 4).min(lines.len());

    use std::fmt::Write as _;
    let mut out = String::new();
    for (i, line) in lines[start..end].iter().enumerate() {
        let lineno = start + i + 1;
        let _ = writeln!(&mut out, "{:>6}\t{}", lineno, line);
    }
    out
}

/// Factory for the `Edit` tool bound to one compatibility session.
pub fn edit_tool(session: WorkdirSessionHandle, tracker: Tracker) -> ToolDefinition {
    routed_edit_tool(crate::routing::singleton_router(session), tracker)
}

pub(crate) fn routed_edit_tool(
    router: Arc<WorkdirSessionRouter>,
    tracker: Tracker,
) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(EditParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Edit")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(EditTool {
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

    fn setup() -> (TempDir, WorkdirSessionHandle, Tracker) {
        let dir = TempDir::new().unwrap();
        let fs: WorkdirSessionHandle = Arc::new(workdir::LocalWorkdirSession::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ));
        (dir, fs, Tracker::new())
    }

    async fn read_first(fs: &WorkdirSessionHandle, tracker: &Tracker, file: &std::path::Path) {
        let def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = def();
        let inp = serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() });
        reader
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn edit_shared_validation_rejects_identical_text_before_read_or_mutation() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "same").unwrap();
        let (_, tool) = edit_tool(fs, tracker.clone())();
        let error = tool
            .execute(
                &serde_json::json!({"file_path":"a.txt","old_string":"same","new_string":"same"})
                    .to_string(),
                Default::default(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, ToolError::InvalidArgument(message) if message == "old_string and new_string are identical")
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), "same");
        assert_eq!(tracker.change_stat(), crate::ChangeStat::default());
    }

    #[tokio::test]
    async fn edit_unique_replacement() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "line1\nfoo bar\nline3\n").unwrap();
        read_first(&fs, &tracker, &file).await;

        let def = edit_tool(fs, tracker);
        let (meta, tool) = def();
        assert_eq!(meta.name, "Edit");

        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "foo bar",
            "new_string": "foo baz",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("1 replacement"));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "line1\nfoo baz\nline3\n"
        );
        assert!(out.content.unwrap().contains("foo baz"));
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x x x\n").unwrap();
        read_first(&fs, &tracker, &file).await;

        let def = edit_tool(fs, tracker);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "x",
            "new_string": "y",
            "replace_all": true,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("3 replacements"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "y y y\n");
    }

    #[tokio::test]
    async fn edit_not_unique() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "a a\n").unwrap();
        read_first(&fs, &tracker, &file).await;

        let def = edit_tool(fs, tracker);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn edit_string_not_found() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "hello\n").unwrap();
        read_first(&fs, &tracker, &file).await;

        let def = edit_tool(fs, tracker);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "world",
            "new_string": "x",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn edit_requires_prior_read() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "foo\n").unwrap();

        let def = edit_tool(fs, tracker);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn edit_detects_external_modification() {
        let (dir, fs, tracker) = setup();
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "foo\n").unwrap();
        read_first(&fs, &tracker, &file).await;

        // External tampering between read and edit
        std::fs::write(&file, "something else").unwrap();

        let def = edit_tool(fs, tracker);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "file_path": file.file_name().unwrap().to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
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
}
