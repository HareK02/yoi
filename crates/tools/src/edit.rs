//! `Edit` tool — partial string replacement with uniqueness check.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_engine::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::error::ToolsError;
use crate::tracker::Tracker;
use workdir::{EditRequest, WorkdirHandle, WorkdirPath};

const DESCRIPTION: &str = "Replace a substring in an existing file. By default \
`old_string` must be unique in the file; set `replace_all: true` to replace \
every occurrence. The file must have been read first (via the Read tool) in \
this session. Paths are relative to the bound Workdir.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct EditParams {
    /// Logical path relative to the bound Workdir root.
    pub file_path: String,
    /// String to replace. Must be unique in the file unless `replace_all` is true.
    pub old_string: String,
    /// Replacement string. Must differ from `old_string`.
    pub new_string: String,
    /// Replace all occurrences. Defaults to false.
    #[serde(default)]
    pub replace_all: bool,
}

pub(crate) struct EditTool {
    workdir: WorkdirHandle,
    tracker: Tracker,
}

#[async_trait]
impl Tool for EditTool {
    async fn execute(
        &self,
        input_json: &str,
        ctx: llm_engine::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: EditParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Edit input: {e}")))?;

        let path = WorkdirPath::new(&params.file_path).map_err(ToolsError::from)?;
        tracing::debug!(path = %path, replace_all = params.replace_all, "Edit");

        if params.old_string.is_empty() {
            return Err(ToolError::InvalidArgument(
                "old_string must not be empty".into(),
            ));
        }
        if params.old_string == params.new_string {
            return Err(ToolError::InvalidArgument(
                "old_string and new_string are identical".into(),
            ));
        }

        let mutation_key = PathBuf::from(path.as_str());
        let _mutation_permit = self.tracker.acquire_mutation(&mutation_key, &ctx).await;
        let expected_hash = self.tracker.expected_workdir_hash(&path)?;
        let result = self
            .workdir
            .edit(EditRequest {
                path: path.clone(),
                old_string: params.old_string.clone(),
                new_string: params.new_string.clone(),
                replace_all: params.replace_all,
                expected_hash,
            })
            .await
            .map_err(ToolsError::from)?;
        self.tracker.record_workdir_hash(&path, result.content_hash);

        let summary = format!(
            "Edited {} ({} replacement{})",
            path,
            result.replacements,
            if result.replacements == 1 { "" } else { "s" }
        );
        let preview = make_preview(&params.new_string, &params.new_string);

        Ok(ToolOutput {
            summary,
            content: Some(preview),
        })
    }
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

/// Factory for the `Edit` tool.
pub fn edit_tool(workdir: WorkdirHandle, tracker: Tracker) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(EditParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Edit")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(EditTool {
            workdir: workdir.clone(),
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

    fn setup() -> (TempDir, WorkdirHandle, Tracker) {
        let dir = TempDir::new().unwrap();
        let fs: WorkdirHandle = Arc::new(workdir::LocalWorkdir::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        ));
        (dir, fs, Tracker::new())
    }

    async fn read_first(fs: &WorkdirHandle, tracker: &Tracker, file: &std::path::Path) {
        let def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = def();
        let inp = serde_json::json!({ "file_path": file.file_name().unwrap().to_str().unwrap() });
        reader
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
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
        let msg = format!("{err}");
        assert!(msg.contains("modified externally"), "{msg}");
    }
}
