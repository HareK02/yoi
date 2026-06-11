//! `Bash` tool — execute shell commands in a one-shot, stateless way.
//!
//! Each call runs `bash -c <command>` via [`tokio::process::Command`].
//! The wrapper redirects all output to a file so we never have to read
//! from a pipe (which would expose us to bg-pipe hangs). There is no
//! shell session: every call starts fresh at `cwd`, so the agent must
//! chain `cd <dir> && cmd` when it wants to operate elsewhere. This
//! mirrors Claude Code's own Bash tool — predictable, no hidden state.
//!
//! Output handling: when output is short (≤ 80 lines, ≤ 12 KiB) it is
//! returned inline and the file is cleaned up. When it is longer the
//! full output is left on disk and only the **last 80 lines** are
//! returned, prefixed with the saved file's path. This sidesteps the
//! Worker's blanket `ToolOutputLimits` (default 64 KiB), which would
//! otherwise drop the *tail* of the output — usually the most useful
//! part (errors, exit messages, summary). The saved file lives under
//! a caller-supplied directory that the parent has added to the
//! `ScopedFs` allow set, so the agent can inspect it via either Read
//! or a follow-up Bash call.
//!
//! Filesystem and network access are NOT mediated by `ScopedFs`: the
//! child process can touch any path. Safety is delegated to the
//! Permission layer (deny/allow rules on the command string).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use llm_worker::tool::{Tool, ToolDefinition, ToolError, ToolMeta, ToolOutput};
use serde::Deserialize;
use tokio::process::Command;

use crate::scoped_fs::ScopedFs;

const DESCRIPTION: &str = "Execute a shell command via bash. Supports the \
full shell — pipes, redirects, command substitution, `&&`/`||`. Each call \
runs in a fresh shell rooted at the workspace; chain `cd <subdir> && cmd` \
when you need to operate elsewhere. stdout and stderr are merged. Default \
timeout 120s, max 600s.\n\n\
Output handling: when the command produces more than 80 lines (or ~12 KiB), \
the full output is saved to a file and only the LAST 80 lines are returned, \
prefixed with the saved path. The path is readable by Read; you can also \
inspect it from a follow-up Bash call (`grep ... <path>`, etc.).\n\n\
Prefer dedicated tools when one fits: Read instead of `cat`/`head`/`tail` \
on workspace files, Edit instead of `sed`/`awk` rewrites, Glob instead of \
`find <name>`, Grep instead of `grep`/`rg`. Reach for Bash when the task \
is shell-shaped: building, testing, version control, package management.";

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 600;

/// Number of trailing lines returned when output spills to a file.
const TAIL_LINES: usize = 80;

/// Inline-return budget. Outputs at or below this are returned in full;
/// above it triggers the spill-to-file path. Sized to leave headroom under
/// the Worker's 64 KiB default `ToolOutputLimits` cap so the inline path
/// reliably reaches the model intact.
const INLINE_BYTE_BUDGET: usize = 12 * 1024;

/// Maximum bytes loaded into memory from the spilled output file. The
/// file itself can be arbitrarily large; we only ever read the tail end
/// since that is what we return.
const TAIL_READ_BUDGET: usize = 256 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(crate) struct BashParams {
    /// Shell command to execute. Passed verbatim to `bash -c`.
    pub command: String,
    /// Timeout in seconds. Defaults to 120, capped at 600.
    #[serde(default)]
    pub timeout: Option<u64>,
}

pub(crate) struct BashTool {
    /// Workspace root that every invocation starts in. Snapshot of
    /// `ScopedFs::cwd()` at registration time; never mutated, since we
    /// don't track `cd` across calls.
    cwd: PathBuf,
    /// Directory to spill long outputs into. Caller is expected to have
    /// added this path to the readable scope so the agent can Read the
    /// saved files. The directory itself is created lazily.
    output_dir: PathBuf,
    /// Files we left on disk for follow-up inspection. Cleaned up on
    /// `Drop` (= session end). `std::sync::Mutex` because access is
    /// always synchronous and very brief.
    spilled_outputs: std::sync::Mutex<Vec<PathBuf>>,
}

impl Drop for BashTool {
    fn drop(&mut self) {
        if let Ok(mut paths) = self.spilled_outputs.lock() {
            for p in paths.drain(..) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    async fn execute(
        &self,
        input_json: &str,
        _ctx: llm_worker::tool::ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        let params: BashParams = serde_json::from_str(input_json)
            .map_err(|e| ToolError::InvalidArgument(format!("invalid Bash input: {e}")))?;
        let timeout_secs = params
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);

        // Persistent output file in the caller-supplied directory.
        // `keep()` opts out of auto-delete so the agent can inspect the
        // full output later; cleanup is deferred to `Drop` on this tool.
        std::fs::create_dir_all(&self.output_dir).map_err(|e| {
            ToolError::Internal(format!(
                "create bash output dir {}: {e}",
                self.output_dir.display()
            ))
        })?;
        let output_path: PathBuf = tempfile::Builder::new()
            .prefix("bash-")
            .suffix(".log")
            .tempfile_in(&self.output_dir)
            .map_err(|e| ToolError::Internal(format!("output tempfile: {e}")))?
            .into_temp_path()
            .keep()
            .map_err(|e| ToolError::Internal(format!("persist output tempfile: {e}")))?;

        let output_path_str = output_path
            .to_str()
            .ok_or_else(|| ToolError::Internal("output path is not UTF-8".into()))?;

        // Wrapper:
        //   exec >file 2>&1     redirect stdout/stderr to the output file
        //   { user_cmd }        run in a brace group (no subshell, so any
        //                       `cd` inside still affects $? capture below)
        //   __exit=$?           preserve the user command's exit code…
        //   wait 2>/dev/null    …since `wait` clobbers $?. Reaping bg jobs
        //                       guarantees the output file's writers all
        //                       close before bash itself exits.
        //   exit $__exit        propagate the user's exit
        let wrapped = format!(
            "exec >{out} 2>&1\n{{ {user_cmd}\n}}\n__yoi_exit=$?\nwait 2>/dev/null\nexit $__yoi_exit\n",
            out = shell_single_quote(output_path_str),
            user_cmd = params.command,
        );

        tracing::debug!(cmd = %params.command, cwd = %self.cwd.display(), timeout_secs, "Bash");

        let mut child = Command::new("bash")
            .arg("-c")
            .arg(&wrapped)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null()) // bash inherits — but the wrapper redirected via `exec`
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                let _ = std::fs::remove_file(&output_path);
                ToolError::ExecutionFailed(format!("spawn bash: {e}"))
            })?;

        let timeout_dur = Duration::from_secs(timeout_secs);
        let wait_result = tokio::time::timeout(timeout_dur, child.wait()).await;
        let (status, timed_out) = match wait_result {
            Ok(Ok(s)) => (Some(s), false),
            Ok(Err(e)) => {
                let _ = std::fs::remove_file(&output_path);
                return Err(ToolError::ExecutionFailed(format!("bash wait: {e}")));
            }
            Err(_) => (None, true),
        };

        // Inspect the on-disk output: total size first, tail bytes second.
        let total_bytes = std::fs::metadata(&output_path)
            .map(|m| m.len() as usize)
            .unwrap_or(0);
        let tail_bytes = read_tail_bytes(&output_path, TAIL_READ_BUDGET).unwrap_or_default();
        let tail_text = String::from_utf8_lossy(&tail_bytes).into_owned();

        let cmd_summary = truncate_for_summary(&params.command);

        if timed_out {
            // Preserve the partial output file — even cut-short logs help
            // diagnose hangs.
            let content = if total_bytes > 0 {
                let last = take_last_n_lines(&tail_text, TAIL_LINES);
                self.remember_spilled(&output_path);
                Some(format!(
                    "[partial output before timeout — full at {}]\n{last}",
                    output_path.display()
                ))
            } else {
                let _ = std::fs::remove_file(&output_path);
                None
            };
            return Ok(ToolOutput {
                summary: format!("$ {cmd_summary} (timed out after {timeout_secs}s)"),
                content,
            });
        }

        let status = status.expect("status set on the success branch");
        let summary = match status.code() {
            Some(0) => format!("$ {cmd_summary}"),
            Some(c) => format!("$ {cmd_summary} (exit {c})"),
            None => format!("$ {cmd_summary} (terminated by signal)"),
        };

        if total_bytes == 0 {
            let _ = std::fs::remove_file(&output_path);
            return Ok(ToolOutput {
                summary,
                content: None,
            });
        }

        // Inline if the whole output fits in our tail-read window AND is
        // small enough to ride under the Worker's default cap.
        let line_count = tail_text.lines().count();
        let fully_loaded = total_bytes <= tail_bytes.len();
        let fits_inline =
            fully_loaded && total_bytes <= INLINE_BYTE_BUDGET && line_count <= TAIL_LINES;

        let content = if fits_inline {
            let _ = std::fs::remove_file(&output_path);
            Some(tail_text)
        } else {
            let last = take_last_n_lines(&tail_text, TAIL_LINES);
            // When `fully_loaded` we know the exact line count; otherwise
            // the file is bigger than our read window so we report bytes
            // and an "approximate" disclaimer.
            let header = if fully_loaded {
                format!(
                    "[showing last {TAIL_LINES} of {line_count} lines — full output ({total_bytes} bytes) at {}]",
                    output_path.display()
                )
            } else {
                format!(
                    "[showing last {TAIL_LINES} lines (tail of {total_bytes}-byte output) — full at {}]",
                    output_path.display()
                )
            };
            self.remember_spilled(&output_path);
            Some(format!("{header}\n{last}"))
        };

        Ok(ToolOutput { summary, content })
    }
}

impl BashTool {
    fn remember_spilled(&self, path: &Path) {
        if let Ok(mut v) = self.spilled_outputs.lock() {
            v.push(path.to_path_buf());
        }
    }
}

/// Read up to `max_bytes` from the end of `path`. If the file is smaller
/// than `max_bytes`, the entire file is returned.
fn read_tail_bytes(path: &Path, max_bytes: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let len = f.seek(SeekFrom::End(0))?;
    let start = if len > max_bytes as u64 {
        len - max_bytes as u64
    } else {
        0
    };
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Return the last `n` lines of `text`. If `text` has `n` or fewer lines
/// (per [`str::lines`]), the input is returned as-is (no allocation).
fn take_last_n_lines(text: &str, n: usize) -> String {
    if text.is_empty() {
        return String::new();
    }
    let total = text.lines().count();
    if total <= n {
        return text.to_owned();
    }
    let skip = total - n;
    let mut count = 0usize;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            count += 1;
            if count == skip {
                return text[i + 1..].to_owned();
            }
        }
    }
    text.to_owned()
}

fn truncate_for_summary(command: &str) -> String {
    let one_line = command.lines().next().unwrap_or("");
    let mut chars = one_line.chars();
    let head: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        let mut shortened = head;
        while shortened.chars().count() > 77 {
            shortened.pop();
        }
        shortened.push_str("...");
        shortened
    } else {
        head
    }
}

/// Wrap a string in single quotes for safe inclusion in a bash command.
fn shell_single_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Factory for the `Bash` tool.
///
/// `output_dir` is where long outputs spill to; the caller is responsible
/// for arranging that the path is in the agent's readable scope. Every
/// invocation starts at `fs.cwd()` — the tool is intentionally stateless
/// w.r.t. the working directory.
pub fn bash_tool(fs: ScopedFs, output_dir: PathBuf) -> ToolDefinition {
    Arc::new(move || {
        let schema = schemars::schema_for!(BashParams);
        let schema_value = serde_json::to_value(schema).unwrap_or(serde_json::json!({}));
        let meta = ToolMeta::new("Bash")
            .description(DESCRIPTION)
            .input_schema(schema_value);
        let tool: Arc<dyn Tool> = Arc::new(BashTool {
            cwd: fs.cwd().to_path_buf(),
            output_dir: output_dir.clone(),
            spilled_outputs: std::sync::Mutex::new(Vec::new()),
        });
        (meta, tool)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use manifest::Scope;
    use tempfile::TempDir;

    /// Test harness: workspace tempdir + a separate spill tempdir kept
    /// alive for the test's lifetime. The spill dir is added to the
    /// scope as readable so callers exercise the production path.
    struct Harness {
        _workspace: TempDir,
        spill: TempDir,
        fs: ScopedFs,
    }

    fn setup() -> Harness {
        let workspace = TempDir::new().unwrap();
        let spill = TempDir::new().unwrap();
        let base = Scope::writable(workspace.path()).unwrap();
        let mut config = manifest::ScopeConfig {
            allow: base.allow_rules(),
            deny: base.deny_rules(),
        };
        config.allow.push(manifest::ScopeRule {
            target: spill.path().to_path_buf(),
            permission: manifest::Permission::Read,
            recursive: true,
        });
        let scope = Scope::from_config(&config).unwrap();
        let fs = ScopedFs::new(scope, workspace.path().to_path_buf());
        Harness {
            _workspace: workspace,
            spill,
            fs,
        }
    }

    fn make_tool(h: &Harness) -> Arc<dyn Tool> {
        let def = bash_tool(h.fs.clone(), h.spill.path().to_path_buf());
        let (_, tool) = def();
        tool
    }

    #[tokio::test]
    async fn runs_simple_command() {
        let h = setup();
        let def = bash_tool(h.fs.clone(), h.spill.path().to_path_buf());
        let (meta, tool) = def();
        assert_eq!(meta.name, "Bash");

        let inp = serde_json::json!({ "command": "echo hello" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert_eq!(out.summary, "$ echo hello");
        assert_eq!(out.content.as_deref().map(str::trim), Some("hello"));
    }

    #[tokio::test]
    async fn merges_stdout_and_stderr() {
        let h = setup();
        let tool = make_tool(&h);

        let inp = serde_json::json!({
            "command": "echo out; echo err 1>&2",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(body.contains("out"));
        assert!(body.contains("err"));
    }

    #[tokio::test]
    async fn nonzero_exit_is_reported() {
        let h = setup();
        let tool = make_tool(&h);

        let inp = serde_json::json!({ "command": "exit 7" });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(out.summary.contains("exit 7"), "summary: {}", out.summary);
        assert!(
            out.content.is_none(),
            "no output expected, got {:?}",
            out.content
        );
    }

    #[tokio::test]
    async fn cd_does_not_persist_across_calls() {
        // Stateless: a `cd` in one call must NOT leak into the next.
        let h = setup();
        let sub = h._workspace.path().join("nested");
        std::fs::create_dir(&sub).unwrap();
        let tool = make_tool(&h);

        tool.execute(
            &serde_json::json!({
                "command": format!("cd {}", sub.to_str().unwrap()),
            })
            .to_string(),
            Default::default(),
        )
        .await
        .unwrap();

        let pwd_out = tool
            .execute(
                &serde_json::json!({ "command": "pwd" }).to_string(),
                Default::default(),
            )
            .await
            .unwrap();
        let body = pwd_out.content.unwrap();
        let actual = std::fs::canonicalize(body.trim()).unwrap();
        let workspace = std::fs::canonicalize(h._workspace.path()).unwrap();
        assert_eq!(
            actual, workspace,
            "second call should start at workspace root, not the previous cd target"
        );
    }

    #[tokio::test]
    async fn timeout_kills_long_command() {
        let h = setup();
        let tool = make_tool(&h);

        let inp = serde_json::json!({
            "command": "sleep 30",
            "timeout": 1,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(
            out.summary.contains("timed out"),
            "summary: {}",
            out.summary
        );
    }

    #[tokio::test]
    async fn invalid_json_is_invalid_argument() {
        let h = setup();
        let tool = make_tool(&h);

        let err = tool
            .execute("not json", Default::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgument(_)));
    }

    #[tokio::test]
    async fn long_output_spills_and_returns_tail() {
        let h = setup();
        let spill_dir = h.spill.path().to_path_buf();
        let tool = make_tool(&h);

        // 200 lines: "line 1" .. "line 200". Tail of 80 keeps lines 121-200.
        let inp = serde_json::json!({
            "command": "for i in $(seq 1 200); do echo line $i; done",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.expect("expected content");

        assert!(
            body.contains(&format!("showing last {TAIL_LINES} of 200 lines")),
            "tail header missing in: {}",
            &body[..body.len().min(300)]
        );
        assert!(
            body.contains(spill_dir.to_str().unwrap()),
            "spill dir path missing: {body}"
        );
        // Last 80 lines are 121..200.
        assert!(body.contains("\nline 200\n"));
        assert!(body.contains("\nline 121\n"));
        // line 120 is the last *elided* line.
        assert!(!body.contains("\nline 120\n"), "elided line leaked: {body}");
    }

    #[tokio::test]
    async fn wide_short_output_still_spills_when_byte_budget_exceeded() {
        let h = setup();
        let spill_dir = h.spill.path().to_path_buf();
        let tool = make_tool(&h);

        // One single line of ~20 KiB (over INLINE_BYTE_BUDGET = 12 KiB).
        let inp = serde_json::json!({
            "command": "printf 'x%.0s' {1..20480}",
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        let body = out.content.unwrap();
        assert!(
            body.contains(spill_dir.to_str().unwrap()),
            "expected spill marker in: {}",
            &body[..body.len().min(200)]
        );
    }

    #[tokio::test]
    async fn background_job_does_not_hang() {
        let h = setup();
        let tool = make_tool(&h);

        // The wrapper's `wait` ensures we don't hang on a stray bg pipe.
        let inp = serde_json::json!({
            "command": "(sleep 0.05; echo bg) &",
            "timeout": 5,
        });
        let out = tool
            .execute(&inp.to_string(), Default::default())
            .await
            .unwrap();
        assert!(
            !out.summary.contains("timed out"),
            "summary: {}",
            out.summary
        );
    }

    #[tokio::test]
    async fn spilled_files_are_cleaned_up_on_drop() {
        let h = setup();
        let spill_dir = h.spill.path().to_path_buf();
        let tool = make_tool(&h);

        let inp = serde_json::json!({
            "command": "for i in $(seq 1 200); do echo $i; done",
        });
        tool.execute(&inp.to_string(), Default::default())
            .await
            .unwrap();

        // The spill dir should now contain exactly one bash-*.log file.
        let files_before: Vec<_> = std::fs::read_dir(&spill_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        assert_eq!(files_before.len(), 1, "expected one spilled file");
        let path = files_before.into_iter().next().unwrap();
        assert!(path.exists());

        drop(tool);
        // Drop runs synchronously; file should be gone.
        assert!(
            !path.exists(),
            "spilled file should be cleaned up on drop: {path:?}"
        );
    }
}
