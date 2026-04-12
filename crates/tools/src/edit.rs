//! `Edit` tool — partial string replacement with uniqueness check.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;

use crate::error::ToolsError;
use crate::scoped_fs::ScopedFs;
use crate::tracker::Tracker;

const DESCRIPTION: &str = "Replace a substring in an existing file. By default \
`old_string` must be unique in the file; set `replace_all: true` to replace \
every occurrence. The file must have been read first (via the Read tool) in \
this session. Paths must be absolute.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct EditParams {
    /// Absolute path to the file.
    pub file_path: PathBuf,
    /// String to replace. Must be unique in the file unless `replace_all` is true.
    pub old_string: String,
    /// Replacement string. Must differ from `old_string`.
    pub new_string: String,
    /// Replace all occurrences. Defaults to false.
    #[serde(default)]
    pub replace_all: bool,
}

pub(crate) struct EditTool {
    fs: ScopedFs,
    tracker: Tracker,
}

#[async_trait]
impl Tool for EditTool {
    async fn execute(&self, input_json: &str) -> Result<ToolOutput, ToolError> {
        let params: EditParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Edit input: {e}")))?;

        tracing::debug!(
            path = %params.file_path.display(),
            replace_all = params.replace_all,
            "Edit"
        );

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

        // Load current content and verify it matches the recorded hash.
        let current_bytes = self.fs.read_bytes(&params.file_path)?;
        self.tracker.verify(&params.file_path, &current_bytes)?;

        let current_text = std::str::from_utf8(&current_bytes).map_err(|_| {
            ToolsError::InvalidArgument(format!(
                "file is not valid UTF-8: {}",
                params.file_path.display()
            ))
        })?;

        let count = current_text.matches(&params.old_string).count();
        if count == 0 {
            return Err(ToolsError::StringNotFound {
                path: params.file_path.clone(),
            }
            .into());
        }
        if !params.replace_all && count > 1 {
            return Err(ToolsError::NotUnique {
                path: params.file_path.clone(),
                count,
            }
            .into());
        }

        let new_text = if params.replace_all {
            current_text.replace(&params.old_string, &params.new_string)
        } else {
            current_text.replacen(&params.old_string, &params.new_string, 1)
        };
        let occurrences = if params.replace_all { count } else { 1 };

        self.fs.write(&params.file_path, new_text.as_bytes())?;
        self.tracker
            .record(&params.file_path, new_text.as_bytes());

        let summary = format!(
            "Edited {} ({} replacement{})",
            params.file_path.display(),
            occurrences,
            if occurrences == 1 { "" } else { "s" }
        );
        let preview = make_preview(&new_text, &params.new_string);

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
pub fn edit_tool(fs: ScopedFs, tracker: Tracker) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(EditParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Edit")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(EditTool {
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
        let fs = ScopedFs::new(Scope::new(dir.path()).unwrap());
        (dir, fs, Tracker::new())
    }

    async fn read_first(fs: &ScopedFs, tracker: &Tracker, file: &std::path::Path) {
        let def = read_tool(fs.clone(), tracker.clone());
        let (_, reader) = def();
        let inp = serde_json::json!({ "file_path": file.to_str().unwrap() });
        reader.execute(&inp.to_string()).await.unwrap();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "foo bar",
            "new_string": "foo baz",
        });
        let out = tool.execute(&inp.to_string()).await.unwrap();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "x",
            "new_string": "y",
            "replace_all": true,
        });
        let out = tool.execute(&inp.to_string()).await.unwrap();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "a",
            "new_string": "b",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "world",
            "new_string": "x",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
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
            "file_path": file.to_str().unwrap(),
            "old_string": "foo",
            "new_string": "bar",
        });
        let err = tool.execute(&inp.to_string()).await.unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("modified externally"), "{msg}");
    }
}
