//! `Glob` tool — recursive file search by glob pattern, sorted by mtime.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use manifest::Scope;
use serde::Deserialize;

use crate::error::ToolsError;
use crate::scoped_fs::{ScopedFs, direct_symlink};

const DESCRIPTION: &str = "Recursively find files matching a glob pattern \
(e.g. \"**/*.rs\"). Results are sorted by modification time, newest first, \
and capped at 1000 entries. Hidden files are included. The `path` parameter \
defaults to the scope root when omitted. Paths must be absolute.";

const RESULT_LIMIT: usize = 1000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct GlobParams {
    /// Glob pattern, e.g. `"**/*.rs"`. Matched against paths relative to
    /// `path` (or the scope root if omitted).
    pub pattern: String,
    /// Absolute directory to search under. Defaults to the scope root.
    #[serde(default)]
    pub path: Option<PathBuf>,
}

pub(crate) struct GlobTool {
    fs: ScopedFs,
}

#[async_trait]
impl Tool for GlobTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: GlobParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Glob input: {e}")))?;

        tracing::debug!(
            pattern = %params.pattern,
            path = ?params.path,
            "Glob"
        );

        let base = params
            .path
            .clone()
            .unwrap_or_else(|| self.fs.cwd().to_path_buf());
        let pattern = params.pattern.clone();
        let scope = self.fs.scope().clone();

        // ignore::Walk is synchronous; run it on a blocking thread so we
        // don't stall the runtime for large trees.
        let results = tokio::task::spawn_blocking(move || run_glob(&base, &pattern, &scope))
            .await
            .map_err(|e| ToolError::Internal(format!("spawn_blocking failed: {e}")))??;

        let total = results.len();
        let (shown, truncated) = if total > RESULT_LIMIT {
            (&results[..RESULT_LIMIT], true)
        } else {
            (&results[..], false)
        };

        if shown.is_empty() {
            return Ok(ToolOutput {
                summary: format!("No files found matching {}", params.pattern),
                content: None,
            });
        }

        let mut body = String::new();
        for p in shown {
            body.push_str(&p.display().to_string());
            body.push('\n');
        }

        let summary = if truncated {
            format!(
                "Found {total}+ files matching {} (truncated to {RESULT_LIMIT})",
                params.pattern
            )
        } else {
            format!("Found {total} file(s) matching {}", params.pattern)
        };

        Ok(ToolOutput {
            summary,
            content: Some(body),
        })
    }
}

fn run_glob(base: &Path, pattern: &str, scope: &Scope) -> Result<Vec<PathBuf>, ToolsError> {
    if !base.is_absolute() {
        return Err(ToolsError::RelativePath(base.to_path_buf()));
    }
    let symlink = direct_symlink(base);
    if !scope.is_readable(base) {
        return Err(if let Some(info) = symlink.as_ref() {
            let link_parent_readable = info
                .link_path
                .parent()
                .map(|parent| scope.is_readable(parent))
                .unwrap_or(false);
            if info.target_exists && link_parent_readable {
                ToolsError::SymlinkOutOfScope {
                    path: base.to_path_buf(),
                    target: info.resolved_path.clone(),
                    required_permission: "read",
                }
            } else {
                ToolsError::OutOfScope(base.to_path_buf())
            }
        } else {
            ToolsError::OutOfScope(base.to_path_buf())
        });
    }
    if let Some(info) = symlink.as_ref() {
        if !info.target_exists {
            return Err(ToolsError::BrokenSymlink {
                path: base.to_path_buf(),
                link: info.link_path.clone(),
                target: info.target_path.clone(),
            });
        }
    }
    let base_meta = std::fs::metadata(base).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => ToolsError::NotFound(base.to_path_buf()),
        _ => ToolsError::io(base, e),
    })?;
    if !base_meta.is_dir() {
        return Err(ToolsError::InvalidArgument(format!(
            "glob search path is not a directory: {}",
            base.display()
        )));
    }
    if let Some(info) = symlink.as_ref() {
        return Err(ToolsError::SymlinkDirectoryNotTraversed {
            tool: "Glob",
            path: base.to_path_buf(),
            target: info.resolved_path.clone(),
        });
    }

    let glob = globset::Glob::new(pattern)
        .map_err(|e| ToolsError::InvalidGlob(e.to_string()))?
        .compile_matcher();

    // Glob is an explicit-pattern tool, so gitignore/hidden are *not* honored.
    let walker = ignore::WalkBuilder::new(base)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .follow_links(false)
        .build();

    let mut hits: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in walker.flatten() {
        let ft = match entry.file_type() {
            Some(ft) => ft,
            None => continue,
        };
        if !ft.is_file() {
            continue;
        }
        let rel = match entry.path().strip_prefix(base) {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !glob.is_match(rel) {
            continue;
        }
        if !scope.is_readable(entry.path()) {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        hits.push((entry.path().to_path_buf(), mtime));
    }

    hits.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(hits.into_iter().map(|(p, _)| p).collect())
}

/// Factory for the `Glob` tool.
pub fn glob_tool(fs: ScopedFs) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(GlobParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Glob")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(GlobTool { fs: fs.clone() });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;
    use tempfile::TempDir;

    fn setup() -> (TempDir, ScopedFs) {
        let dir = TempDir::new().unwrap();
        let fs = ScopedFs::new(
            Scope::writable(dir.path()).unwrap(),
            dir.path().to_path_buf(),
        );
        (dir, fs)
    }

    fn touch(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[tokio::test]
    async fn glob_finds_matching_files() {
        let (dir, fs) = setup();
        touch(&dir.path().join("a.rs"), "");
        touch(&dir.path().join("sub/b.rs"), "");
        touch(&dir.path().join("sub/c.txt"), "");

        let def = glob_tool(fs);
        let (meta, tool) = def();
        assert_eq!(meta.name, "Glob");

        let inp = serde_json::json!({ "pattern": "**/*.rs" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("2 file(s)"));
        let body = out.content.unwrap();
        assert!(body.contains("a.rs"));
        assert!(body.contains("b.rs"));
        assert!(!body.contains("c.txt"));
    }

    #[tokio::test]
    async fn glob_sorts_by_mtime_desc() {
        let (dir, fs) = setup();
        let older = dir.path().join("old.rs");
        let newer = dir.path().join("new.rs");
        touch(&older, "");
        touch(&newer, "");

        filetime::set_file_mtime(&older, filetime::FileTime::from_unix_time(1_000, 0)).unwrap();
        filetime::set_file_mtime(&newer, filetime::FileTime::from_unix_time(2_000, 0)).unwrap();

        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "*.rs" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        let new_pos = body.find("new.rs").unwrap();
        let old_pos = body.find("old.rs").unwrap();
        assert!(new_pos < old_pos, "newer file should come first:\n{body}");
    }

    #[tokio::test]
    async fn glob_empty_results() {
        let (_dir, fs) = setup();
        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "**/*.nonexistent" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("No files"));
        assert!(out.content.is_none());
    }

    #[tokio::test]
    async fn glob_invalid_pattern() {
        let (_dir, fs) = setup();
        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "[unterminated" });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn glob_filters_results_by_scope_readability() {
        use manifest::{Permission, ScopeConfig, ScopeRule};

        let dir = TempDir::new().unwrap();
        let secret_dir = dir.path().join("secret");
        std::fs::create_dir(&secret_dir).unwrap();
        touch(&dir.path().join("visible.rs"), "");
        touch(&secret_dir.join("hidden.rs"), "");

        let cfg = ScopeConfig {
            allow: vec![ScopeRule {
                target: dir.path().to_path_buf(),
                permission: Permission::Write,
                recursive: true,
            }],
            deny: vec![ScopeRule {
                target: secret_dir.clone(),
                permission: Permission::Read,
                recursive: true,
            }],
        };
        let scope = Scope::from_config(&cfg).unwrap();
        let fs = ScopedFs::new(scope, dir.path().to_path_buf());

        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "**/*.rs" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap_or_default();
        assert!(body.contains("visible.rs"));
        assert!(
            !body.contains("hidden.rs"),
            "scope-denied file leaked into glob output: {body}"
        );
    }

    #[tokio::test]
    async fn glob_honors_hidden_files() {
        let (dir, fs) = setup();
        touch(&dir.path().join(".hidden.rs"), "");
        touch(&dir.path().join("visible.rs"), "");

        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({ "pattern": "*.rs" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains(".hidden.rs"));
        assert!(body.contains("visible.rs"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn glob_reports_scope_inside_symlink_directory_is_not_traversed() {
        use std::os::unix::fs::symlink;

        let (dir, fs) = setup();
        let target = dir.path().join("target-dir");
        touch(&target.join("visible.rs"), "");
        let link = dir.path().join("external-project");
        symlink(&target, &link).unwrap();

        let def = glob_tool(fs);
        let (_, tool) = def();
        let inp = serde_json::json!({
            "path": link.to_str().unwrap(),
            "pattern": "**/*.rs",
        });
        let err = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("Glob does not follow symlink directories"),
            "{msg}"
        );
        assert!(msg.contains(&link.display().to_string()), "{msg}");
        assert!(
            msg.contains(&target.canonicalize().unwrap().display().to_string()),
            "{msg}"
        );
    }
}
